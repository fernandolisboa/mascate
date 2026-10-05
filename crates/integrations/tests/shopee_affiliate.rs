//! The Shopee Affiliate Open API's adapter against the fake server serving
//! answers written from Shopee's documentation (see the fixtures' README).

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_catalog::{
    Catalog, FoundOffer, KeptOffers, OfferOrder, OfferSearch, OfferSource, ProductSource,
};
use mascate_integrations::{Pause, ShopeeAffiliates};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, Percentage, PlatformError};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/shopee-affiliate/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

fn percent(amount: &str) -> Percentage {
    Percentage::new(Decimal::from_str(amount).unwrap()).unwrap()
}

/// Remembers each pause instead of sleeping.
#[derive(Default)]
struct RecordedPauses(Mutex<Vec<Duration>>);

impl Pause for RecordedPauses {
    fn pause(&self, duration: Duration) {
        self.0.lock().unwrap().push(duration);
    }
}

impl RecordedPauses {
    fn seconds(&self) -> Vec<u64> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(Duration::as_secs)
            .collect()
    }
}

struct Setup {
    server: FakeHttpServer,
    store: Arc<MemorySecretStore>,
    clock: Arc<ManualClock>,
    pauses: Arc<RecordedPauses>,
    shopee: ShopeeAffiliates,
}

/// Shopee at the fake server, with the AppID 123456 and the Secret
/// `demo-secret`, on 2026-10-04 12:00 UTC.
fn setup() -> Setup {
    let server = FakeHttpServer::start();
    let store = Arc::new(MemorySecretStore::default());
    store
        .write("SHOPEE_AFFILIATE_APP_ID", &Secret::new("123456"))
        .unwrap();
    store
        .write("SHOPEE_AFFILIATE_SECRET", &Secret::new("demo-secret"))
        .unwrap();
    let clock = Arc::new(ManualClock::at(
        Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
    ));
    let pauses = Arc::new(RecordedPauses::default());
    let shopee = ShopeeAffiliates::new(
        &format!("{}/graphql", server.url()),
        "Mascate/test",
        store.clone(),
        clock.clone(),
    )
    .with_pause(pauses.clone());
    Setup {
        server,
        store,
        clock,
        pauses,
        shopee,
    }
}

fn fone_search() -> OfferSearch {
    OfferSearch {
        keyword: "fone bluetooth".into(),
        page: 1,
        ..OfferSearch::default()
    }
}

#[test]
fn every_request_is_signed_with_the_appid_timestamp_body_and_secret() {
    let s = setup();
    s.server
        .serve("/graphql", 200, fixture("product-offers.json"));

    s.shopee.search_offers(&fone_search()).unwrap();

    let request = &s.server.received()[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/graphql");
    assert!(
        request
            .headers
            .contains(&("content-type".into(), "application/json".into()))
    );
    // The known vector, worked out apart from the app with Python's hashlib:
    // sha256("123456" + "1791115200" + body + "demo-secret").
    assert_eq!(
        request.body,
        r#"{"query":"{productOfferV2(keyword: \"fone bluetooth\", sortType: 2, page: 1, limit: 20){nodes{itemId productName productLink priceMin priceMax sales commissionRate ratingStar shopId shopName}pageInfo{hasNextPage}}}"}"#
    );
    assert_eq!(
        request.authorization.as_deref(),
        Some(
            "SHA256 Credential=123456, Timestamp=1791115200, \
             Signature=33e4a47224748efa57c66f6b06acc467aba0660c65d1989faa3901c9be54037a"
        )
    );
}

#[test]
fn found_items_carry_their_price_sales_commission_and_shop() {
    let s = setup();
    s.server
        .serve("/graphql", 200, fixture("product-offers.json"));

    let found = s.shopee.search_offers(&fone_search()).unwrap();

    assert!(found.more);
    // The third item has no price, so it is no offer.
    assert_eq!(found.items.len(), 2);
    let fone = &found.items[0];
    assert_eq!(fone.id, "22334455667");
    assert_eq!(
        fone.title,
        "Fone de Ouvido Bluetooth TWS Sem Fio com Case Carregador"
    );
    assert_eq!(
        fone.link,
        "https://shopee.com.br/product/339216014/22334455667"
    );
    assert_eq!(fone.price, brl("29.90"));
    assert_eq!(fone.highest_price, brl("29.90"));
    assert_eq!(fone.sales, 12_400);
    assert_eq!(fone.commission, Some(percent("10")));
    assert_eq!(fone.rating, Some(Decimal::from_str("4.9").unwrap()));
    assert_eq!(fone.shop.id, "339216014");
    assert_eq!(fone.shop.name, "Loja Fones Oficial");
    assert_eq!(fone.shop.link, "https://shopee.com.br/shop/339216014");
    let luminaria = &found.items[1];
    assert_eq!(luminaria.price, brl("45.50"));
    assert_eq!(luminaria.highest_price, brl("59.90"));
    assert_eq!(luminaria.commission, Some(percent("6")));
    assert_eq!(s.shopee.source(), ProductSource::ShopeeAffiliate);
}

#[test]
fn searches_by_category_link_shop_order_and_page() {
    let s = setup();
    s.server
        .serve("/graphql", 200, fixture("product-offers-end.json"));

    let found = s
        .shopee
        .search_offers(&OfferSearch {
            keyword: "  ".into(),
            category: Some("https://shopee.com.br/Celulares-cat.11059988.11060040?page=2".into()),
            shop: Some("339216014".into()),
            order: OfferOrder::LowestPrice,
            page: 2,
        })
        .unwrap();

    assert!(!found.more);
    let body = &s.server.received()[0].body;
    assert!(body.contains(
        "productOfferV2(productCatId: 11060040, shopId: 339216014, sortType: 4, page: 2, \
         limit: 20)"
    ));
    for (order, sort) in [
        (OfferOrder::Relevance, 1),
        (OfferOrder::HighestCommission, 5),
    ] {
        s.shopee
            .search_offers(&OfferSearch {
                category: Some(" 11059988 ".into()),
                order,
                ..fone_search()
            })
            .unwrap();
        let body = s.server.received().last().unwrap().body.clone();
        assert!(body.contains(&format!(
            "keyword: \\\"fone bluetooth\\\", productCatId: 11059988, sortType: {sort}"
        )));
    }
}

#[test]
fn a_keyword_cannot_break_out_of_the_query() {
    let s = setup();
    s.server
        .serve("/graphql", 200, fixture("product-offers-end.json"));

    s.shopee
        .search_offers(&OfferSearch {
            keyword: r#"fone") { __typename } x("#.into(),
            ..fone_search()
        })
        .unwrap();

    let body: serde_json::Value = serde_json::from_str(&s.server.received()[0].body).unwrap();
    assert!(
        body["query"]
            .as_str()
            .unwrap()
            .starts_with(r#"{productOfferV2(keyword: "fone\") { __typename } x(", sortType: 2"#)
    );
}

#[test]
fn a_category_that_is_not_shopees_is_refused_before_asking() {
    let s = setup();

    let refused = s.shopee.search_offers(&OfferSearch {
        category: Some("https://shopee.com.br/Celulares".into()),
        ..fone_search()
    });

    assert!(matches!(refused, Err(PlatformError::Refused(_))));
    assert!(s.server.received().is_empty());
}

#[test]
fn shops_carry_their_commission_rating_and_page() {
    let s = setup();
    s.server.serve("/graphql", 200, fixture("shop-offers.json"));

    let found = s.shopee.search_shops("fones", 1).unwrap();

    assert!(!found.more);
    assert_eq!(found.items.len(), 2);
    assert_eq!(found.items[0].id, "339216014");
    assert_eq!(found.items[0].name, "Loja Fones Oficial");
    assert_eq!(
        found.items[0].link,
        "https://shopee.com.br/lojafonesoficial"
    );
    assert_eq!(found.items[0].commission, Some(percent("5")));
    assert_eq!(
        found.items[0].rating,
        Some(Decimal::from_str("4.8").unwrap())
    );
    assert_eq!(found.items[1].link, "https://shopee.com.br/shop/880044556");
    assert_eq!(found.items[1].rating, None);
    assert!(
        s.server.received()[0]
            .body
            .contains(r#"shopOfferV2(keyword: \"fones\", sortType: 3, page: 1, limit: 20)"#)
    );
}

#[test]
fn without_both_keys_the_search_is_off_and_asks_nothing() {
    let s = setup();
    s.store.remove("SHOPEE_AFFILIATE_SECRET").unwrap();

    assert_eq!(
        s.shopee.search_offers(&fone_search()),
        Err(PlatformError::NotConnected)
    );
    s.store.remove("SHOPEE_AFFILIATE_APP_ID").unwrap();
    assert_eq!(
        s.shopee.search_shops("fones", 1),
        Err(PlatformError::NotConnected)
    );
    assert!(s.server.received().is_empty());
}

#[test]
fn a_refused_signature_means_shopee_no_longer_accepts_the_keys() {
    let s = setup();
    s.server.serve("/graphql", 200, fixture("error-10020.json"));

    assert_eq!(
        s.shopee.search_offers(&fone_search()),
        Err(PlatformError::Expired)
    );
    assert_eq!(s.server.received().len(), 1);
    assert!(s.pauses.seconds().is_empty());
}

#[test]
fn no_access_to_the_open_api_yet_is_a_refusal_with_shopees_reason() {
    let s = setup();
    s.server.serve("/graphql", 200, fixture("error-10035.json"));

    let refused = s.shopee.search_offers(&fone_search());

    assert!(
        matches!(refused, Err(PlatformError::Refused(why)) if why.starts_with("You currently do not have access"))
    );
    assert_eq!(s.server.received().len(), 1);
}

#[test]
fn a_rate_limit_waits_longer_each_time_and_then_gives_up() {
    let s = setup();
    s.server.serve("/graphql", 200, fixture("error-10030.json"));

    assert_eq!(
        s.shopee.search_offers(&fone_search()),
        Err(PlatformError::RateLimited)
    );
    assert_eq!(s.server.received().len(), 4);
    assert_eq!(s.pauses.seconds(), [2, 4, 8]);
}

#[test]
fn a_rate_limit_that_lifts_answers_after_the_pause_with_a_fresh_signature() {
    let s = setup();
    s.server.serve("/graphql", 200, fixture("error-10030.json"));
    s.server
        .then_serve("/graphql", 429, r#"{"message":"Too Many Requests"}"#);
    s.server.then_serve("/graphql", 503, "Service Unavailable");
    s.server
        .then_serve("/graphql", 200, fixture("product-offers.json"));
    let clock = s.clock.clone();
    let pauses = Arc::new(AdvancingPauses(clock));
    let shopee = ShopeeAffiliates::new(
        &format!("{}/graphql", s.server.url()),
        "Mascate/test",
        s.store.clone(),
        s.clock.clone(),
    )
    .with_pause(pauses);

    let found = shopee.search_offers(&fone_search()).unwrap();

    assert_eq!(found.items.len(), 2);
    let timestamps: Vec<String> = s
        .server
        .received()
        .iter()
        .map(|request| {
            let header = request.authorization.clone().unwrap();
            header.split("Timestamp=").nth(1).unwrap()[..10].to_owned()
        })
        .collect();
    assert_eq!(
        timestamps,
        ["1791115200", "1791115202", "1791115206", "1791115214"]
    );
}

/// Moves the clock on by each pause, as real time would.
struct AdvancingPauses(Arc<ManualClock>);

impl Pause for AdvancingPauses {
    fn pause(&self, duration: Duration) {
        self.0
            .advance(chrono::TimeDelta::from_std(duration).unwrap());
    }
}

#[test]
fn a_server_error_that_lasts_is_a_failure_after_the_retries() {
    let s = setup();
    s.server.serve("/graphql", 502, "Bad Gateway");

    assert!(matches!(
        s.shopee.search_offers(&fone_search()),
        Err(PlatformError::Failed(_))
    ));
    assert_eq!(s.pauses.seconds(), [2, 4, 8]);
}

#[test]
fn found_offers_kept_twice_are_supplier_offers_once() {
    block_on(async {
        let s = setup();
        s.server
            .serve("/graphql", 200, fixture("product-offers.json"));
        s.server
            .then_serve("/graphql", 200, fixture("product-offers-end.json"));
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&dir.path().join("m.db")).await.unwrap());
        migrate(&database, s.clock.as_ref(), &[mascate_catalog::MIGRATIONS])
            .await
            .unwrap();
        let catalog = Catalog::new(
            database,
            dir.path().join("produtos"),
            s.clock.clone(),
            Arc::new(SequentialIds::default()),
        );

        let first = catalog
            .search_offers(&s.shopee, &fone_search())
            .await
            .unwrap();
        let offers: Vec<FoundOffer> = first.items.iter().map(|i| i.found.clone()).collect();
        let kept = catalog
            .keep_found_offers(ProductSource::ShopeeAffiliate, &offers)
            .await
            .unwrap();
        // The next page repeats the fone, as a page can when Shopee
        // reorders between requests.
        let second = catalog
            .search_offers(
                &s.shopee,
                &OfferSearch {
                    page: 2,
                    ..fone_search()
                },
            )
            .await
            .unwrap();
        let again: Vec<FoundOffer> = second.items.iter().map(|i| i.found.clone()).collect();
        let kept_again = catalog
            .keep_found_offers(ProductSource::ShopeeAffiliate, &again)
            .await
            .unwrap();

        assert_eq!(kept.added, 2);
        assert_eq!(
            kept_again,
            KeptOffers {
                added: 0,
                unchanged: 1,
                skipped: 0
            }
        );
        assert!(second.items[0].kept.is_some());
        assert_eq!(catalog.offers().await.unwrap().len(), 2);
        let names: Vec<String> = catalog
            .suppliers()
            .await
            .unwrap()
            .into_iter()
            .map(|supplier| supplier.name)
            .collect();
        assert_eq!(names, ["Casa Clara", "Loja Fones Oficial"]);
    });
}
