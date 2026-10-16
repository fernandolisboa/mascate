//! The owner's Amazon listings through the Selling Partner API adapter: the
//! adapter against the fake server serving the answers in
//! `fixtures/amazon-seller`, written from the documentation (see its
//! README), and commerce's Sync of Listings end to end on a real temporary
//! database.

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    CatalogProduct, ChannelCategory, ChannelListing, ChannelStock, FeeBasis, ListingStatus,
    Listings, OnSalesChannel, SaleFee, SalesChannel, SuggestedBy,
};
use mascate_integrations::{Amazon, Pause};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{
    Currency, ListingType, Money, Percentage, PlatformError, RecordId, SalesChannelKind,
};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;

const TOKEN: &str = "/auth/o2/token";
const ITEMS: &str = "/listings/2021-08-01/items/A1MASCATE0SELLER";
const PAGE_1: &str = "/listings/2021-08-01/items/A1MASCATE0SELLER?marketplaceIds=A2Q3Y263D00KWC\
                      &includedData=summaries&sortBy=createdDate&sortOrder=ASC&pageSize=20";
const PAGE_2: &str = "/listings/2021-08-01/items/A1MASCATE0SELLER?marketplaceIds=A2Q3Y263D00KWC\
                      &includedData=summaries&sortBy=createdDate&sortOrder=ASC&pageSize=20\
                      &pageToken=eyJsYXN0U2t1IjoiQ0FORUNBLTAxIn0";
const FONE_PATCH: &str =
    "/listings/2021-08-01/items/A1MASCATE0SELLER/FONE-BT-001?marketplaceIds=A2Q3Y263D00KWC";
const FONE_FEES: &str = "/products/fees/v0/listings/FONE-BT-001/feesEstimate";

/// The SKUs in the order the pages list them.
const SKUS: [&str; 5] = [
    "FONE-BT-001",
    "CANECA-01",
    "CAMISETA-PAI",
    "CAMISETA-P",
    "FONE-FBA",
];

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/amazon-seller/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

fn percent(text: &str) -> Percentage {
    Percentage::new(Decimal::from_str(text).unwrap()).unwrap()
}

/// The search of `skus` with every detail of each listing.
fn by_sku(skus: &[&str]) -> String {
    format!(
        "{ITEMS}?marketplaceIds=A2Q3Y263D00KWC&identifiers={}&identifiersType=SKU\
         &includedData=summaries%2Coffers%2CfulfillmentAvailability%2Crelationships&pageSize=20",
        skus.join("%2C")
    )
}

fn product_type(name: &str) -> String {
    format!(
        "/definitions/2020-09-01/productTypes/{name}?marketplaceIds=A2Q3Y263D00KWC\
         &requirements=LISTING_OFFER_ONLY&locale=pt_BR"
    )
}

/// Remembers each pause instead of sleeping.
#[derive(Default)]
struct RecordedPauses(Mutex<Vec<Duration>>);

impl Pause for RecordedPauses {
    fn pause(&self, duration: Duration) {
        self.0.lock().unwrap().push(duration);
    }
}

struct Fake {
    server: FakeHttpServer,
    store: Arc<MemorySecretStore>,
    pauses: Arc<RecordedPauses>,
    adapter: Amazon,
}

impl Fake {
    /// The owner's keys pasted, on 2026-10-16 12:00 UTC.
    fn connected() -> Self {
        let fake = Self::not_connected();
        for (name, value) in [
            (
                "AMAZON_LWA_CLIENT_ID",
                "amzn1.application-oa2-client.mascate",
            ),
            ("AMAZON_LWA_CLIENT_SECRET", "amzn1.oa2-cs.v1.s3cr3t"),
            ("AMAZON_REFRESH_TOKEN", "Atzr|IwEBIA-refresh-token"),
            ("AMAZON_SELLER_ID", "A1MASCATE0SELLER"),
        ] {
            fake.store.write(name, &Secret::new(value)).unwrap();
        }
        fake.serve(TOKEN, "token.json");
        fake.serve(PAGE_1, "listings-page-1.json");
        fake.serve(PAGE_2, "listings-page-2.json");
        fake.serve(&by_sku(&SKUS), "listings-by-sku.json");
        fake.serve(&product_type("HEADPHONES"), "product-type-HEADPHONES.json");
        fake.serve(&product_type("SHIRT"), "product-type-SHIRT.json");
        fake.serve(FONE_PATCH, "patch-accepted.json");
        fake
    }

    fn not_connected() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
        ));
        let pauses = Arc::new(RecordedPauses::default());
        let adapter = Amazon::new(
            server.url(),
            server.url(),
            "Mascate/test",
            store.clone(),
            clock,
        )
        .with_pause(pauses.clone());
        Self {
            server,
            store,
            pauses,
            adapter,
        }
    }

    fn serve(&self, path: &str, name: &str) {
        self.server.serve(path, 200, fixture(name));
    }

    fn renewals(&self) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|path| *path == TOKEN)
            .count()
    }

    /// The JSON bodies sent to `path`, in order.
    fn bodies(&self, path: &str) -> Vec<serde_json::Value> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.path == path)
            .map(|request| serde_json::from_str(&request.body).unwrap())
            .collect()
    }

    fn pauses(&self) -> Vec<u64> {
        self.pauses
            .0
            .lock()
            .unwrap()
            .iter()
            .map(Duration::as_secs)
            .collect()
    }
}

fn fone() -> ChannelListing {
    ChannelListing {
        id: "FONE-BT-001".into(),
        variation: None,
        title: "Fone de Ouvido Bluetooth Mascate Preto".into(),
        price: brl("129.90"),
        available_quantity: 7,
        status: ListingStatus::Active,
        link: Some("https://www.amazon.com.br/dp/B0CFONE001".into()),
        listing_type: Some(ListingType::Classic),
        category: Some("HEADPHONES".into()),
        seller_sku: Some("FONE-BT-001".into()),
    }
}

fn skus() -> Vec<String> {
    SKUS.map(String::from).to_vec()
}

#[test]
fn the_login_is_a_token_from_the_pasted_keys_sent_in_amazons_header() {
    let fake = Fake::connected();

    fake.adapter.listing_ids().unwrap();

    let received = fake.server.received();
    let token = received
        .iter()
        .find(|request| request.path == TOKEN)
        .unwrap();
    assert_eq!(token.method, "POST");
    assert_eq!(
        token.body,
        "grant_type=refresh_token&refresh_token=Atzr%7CIwEBIA-refresh-token\
         &client_id=amzn1.application-oa2-client.mascate&client_secret=amzn1.oa2-cs.v1.s3cr3t"
    );
    let searches: Vec<_> = received
        .iter()
        .filter(|request| request.path.starts_with(ITEMS))
        .collect();
    assert_eq!(searches.len(), 2);
    for search in searches {
        assert!(search.headers.contains(&(
            "x-amz-access-token".into(),
            "Atza|IwEBIA-first-access-token".into()
        )));
        // No AWS signature since 2023: the token alone.
        assert_eq!(search.authorization, None);
    }
}

#[test]
fn the_sellers_listings_are_read_page_by_page_oldest_first() {
    let fake = Fake::connected();

    let ids = fake.adapter.listing_ids().unwrap();

    assert_eq!(ids, skus());
    assert_eq!(fake.server.requests(), [TOKEN, PAGE_1, PAGE_2]);
}

#[test]
fn each_listing_sold_by_the_seller_is_one_listing_and_the_parent_sku_none() {
    let fake = Fake::connected();

    let listings = fake.adapter.listings(&skus()).unwrap();

    let caneca = ChannelListing {
        id: "CANECA-01".into(),
        title: "Caneca de Cerâmica 350 ml".into(),
        price: brl("39.9"),
        available_quantity: 0,
        // Amazon has no pause: zero units take it off sale.
        status: ListingStatus::OutOfStock,
        link: Some("https://www.amazon.com.br/dp/B0CCANECA1".into()),
        category: Some("DRINKING_CUP".into()),
        seller_sku: Some("CANECA-01".into()),
        ..fone()
    };
    // A child SKU has its own price and stock; the B2B offer is not its
    // price.
    let camiseta = ChannelListing {
        id: "CAMISETA-P".into(),
        title: "Camiseta Básica Algodão Preta P".into(),
        price: brl("49.90"),
        available_quantity: 3,
        link: Some("https://www.amazon.com.br/dp/B0CCAMI00P".into()),
        category: Some("SHIRT".into()),
        seller_sku: Some("CAMISETA-P".into()),
        ..fone()
    };
    // The parent sells nothing, and Amazon ships the FBA offer itself.
    assert_eq!(listings, [fone(), caneca, camiseta]);
    assert_eq!(fake.server.requests().last().unwrap(), &by_sku(&SKUS));
}

#[test]
fn more_than_twenty_skus_are_asked_twenty_at_a_time() {
    let fake = Fake::connected();
    let many: Vec<String> = (1..=21).map(|n| format!("SKU-{n:02}")).collect();
    let first: Vec<&str> = many[..20].iter().map(String::as_str).collect();
    fake.server
        .serve(&by_sku(&first), 200, r#"{"numberOfResults":0,"items":[]}"#);
    fake.server.serve(
        &by_sku(&["SKU-21"]),
        200,
        r#"{"numberOfResults":0,"items":[]}"#,
    );

    assert_eq!(fake.adapter.listings(&many), Ok(Vec::new()));
    assert_eq!(
        fake.server.requests()[1..],
        [by_sku(&first), by_sku(&["SKU-21"])]
    );
}

#[test]
fn past_a_thousand_listings_the_next_search_starts_at_the_last_created() {
    let fake = Fake::connected();
    // A first search that reached Amazon's cap of 1,000 with 1,001 to go.
    let item = |sku: &str, created: &str| {
        format!(
            r#"{{"sku":"{sku}","summaries":[{{"marketplaceId":"A2Q3Y263D00KWC",
                "createdDate":"{created}"}}]}}"#
        )
    };
    let mut path = PAGE_1.to_owned();
    for page in 0..50 {
        let items: Vec<String> = (0..20)
            .map(|n| item(&format!("SKU-{:04}", page * 20 + n), "2026-09-01T00:00:00Z"))
            .collect();
        let next = if page < 49 {
            format!(r#","pagination":{{"nextToken":"page-{}"}}"#, page + 1)
        } else {
            String::new()
        };
        fake.server.serve(
            &path,
            200,
            format!(
                r#"{{"numberOfResults":1001,"items":[{}]{next}}}"#,
                items.join(",")
            ),
        );
        path = format!("{PAGE_1}&pageToken=page-{}", page + 1);
    }
    let after = format!("{PAGE_1}&createdAfter=2026-09-01T00%3A00%3A00Z");
    fake.server.serve(
        &after,
        200,
        format!(
            r#"{{"numberOfResults":2,"items":[{},{}]}}"#,
            item("SKU-0999", "2026-09-01T00:00:00Z"),
            item("SKU-1000", "2026-09-02T00:00:00Z")
        ),
    );

    let ids = fake.adapter.listing_ids().unwrap();

    assert_eq!(ids.len(), 1001);
    assert_eq!(ids.last().unwrap(), "SKU-1000");
    assert_eq!(fake.server.requests().last().unwrap(), &after);
}

#[test]
fn a_new_price_is_merged_into_the_offer_for_everyone() {
    let fake = Fake::connected();

    fake.adapter
        .set_price("FONE-BT-001", brl("119.899"))
        .unwrap();

    let sent = fake.bodies(FONE_PATCH);
    assert_eq!(
        sent,
        [serde_json::json!({
            "productType": "PRODUCT",
            "patches": [{
                "op": "merge",
                "path": "/attributes/purchasable_offer",
                "value": [{
                    "marketplace_id": "A2Q3Y263D00KWC",
                    "currency": "BRL",
                    "audience": "ALL",
                    "our_price": [{"schedule": [{"value_with_tax": 119.90}]}],
                }],
            }],
        })]
    );
    let sent = fake.server.received().pop().unwrap();
    assert_eq!(sent.method, "PATCH");
    // The decimal's own text: a price never goes through a float.
    assert!(sent.body.contains(r#""value_with_tax":119.90"#));
}

#[test]
fn the_stock_is_merged_into_the_sellers_own_fulfillment() {
    let fake = Fake::connected();

    fake.adapter
        .set_stock(
            "FONE-BT-001",
            &[ChannelStock {
                variation: None,
                available_quantity: 4,
            }],
        )
        .unwrap();

    assert_eq!(
        fake.bodies(FONE_PATCH),
        [serde_json::json!({
            "productType": "PRODUCT",
            "patches": [{
                "op": "merge",
                "path": "/attributes/fulfillment_availability",
                "value": [{"fulfillment_channel_code": "DEFAULT", "quantity": 4}],
            }],
        })]
    );
}

#[test]
fn an_amazon_listing_has_no_variation_to_send_stock_to() {
    let fake = Fake::connected();

    let sent = fake.adapter.set_stock(
        "FONE-BT-001",
        &[ChannelStock {
            variation: Some("P".into()),
            available_quantity: 4,
        }],
    );

    assert_eq!(sent, Err(PlatformError::NotFound));
    assert!(fake.bodies(FONE_PATCH).is_empty());
}

#[test]
fn a_sku_goes_into_the_path_escaped() {
    let fake = Fake::connected();
    let path = "/listings/2021-08-01/items/A1MASCATE0SELLER/CANECA%2001%2FAZUL%3F\
                ?marketplaceIds=A2Q3Y263D00KWC";
    fake.serve(path, "patch-accepted.json");

    fake.adapter
        .set_stock(
            "CANECA 01/AZUL?",
            &[ChannelStock {
                variation: None,
                available_quantity: 1,
            }],
        )
        .unwrap();

    assert_eq!(fake.server.requests().last().unwrap(), path);
}

#[test]
fn pausing_sends_zero_units_and_reactivating_waits_for_the_stock_send() {
    let fake = Fake::connected();

    assert!(fake.adapter.pauses_with_zero_stock());
    fake.adapter.pause("FONE-BT-001").unwrap();
    fake.adapter.activate("FONE-BT-001").unwrap();

    let sent = fake.bodies(FONE_PATCH);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0]["patches"][0]["value"],
        serde_json::json!([{"fulfillment_channel_code": "DEFAULT", "quantity": 0}])
    );
}

#[test]
fn a_change_amazon_finds_invalid_is_refused_with_its_errors() {
    let fake = Fake::connected();
    fake.serve(FONE_PATCH, "patch-invalid.json");

    assert_eq!(
        fake.adapter.set_price("FONE-BT-001", brl("0.01")),
        Err(PlatformError::Refused(
            "O valor de 'our_price' precisa ser maior que 0.".into()
        ))
    );
}

#[test]
fn a_request_amazon_refuses_carries_its_message() {
    let fake = Fake::connected();
    fake.server.serve(PAGE_1, 400, fixture("error-400.json"));

    assert_eq!(
        fake.adapter.listing_ids(),
        Err(PlatformError::Refused(
            "Invalid input: marketplaceIds is required.".into()
        ))
    );
}

#[test]
fn a_turned_down_token_is_renewed_once_before_the_login_counts_as_expired() {
    let fake = Fake::connected();
    fake.server.serve(PAGE_1, 403, fixture("error-403.json"));
    fake.server
        .then_serve(PAGE_1, 200, fixture("listings-page-1.json"));

    assert_eq!(fake.adapter.listing_ids(), Ok(skus()));
    assert_eq!(fake.renewals(), 2);

    let fake = Fake::connected();
    fake.server.serve(PAGE_1, 403, fixture("error-403.json"));
    assert_eq!(fake.adapter.listing_ids(), Err(PlatformError::Expired));
    assert_eq!(fake.renewals(), 2);

    let fake = Fake::connected();
    fake.server.serve(PAGE_1, 401, "{}");
    assert_eq!(fake.adapter.listing_ids(), Err(PlatformError::Expired));
}

#[test]
fn keys_amazon_no_longer_accepts_mean_the_login_expired() {
    let fake = Fake::connected();
    fake.server
        .serve(TOKEN, 401, fixture("token-invalid-client.json"));

    assert_eq!(fake.adapter.listing_ids(), Err(PlatformError::Expired));
    assert_eq!(fake.server.requests(), [TOKEN]);
}

#[test]
fn a_throttled_request_waits_longer_each_time_and_then_goes_through() {
    let fake = Fake::connected();
    fake.server.serve(PAGE_1, 429, fixture("error-429.json"));
    fake.server
        .then_serve(PAGE_1, 429, fixture("error-429.json"));
    fake.server
        .then_serve(PAGE_1, 200, fixture("listings-page-1.json"));

    assert_eq!(fake.adapter.listing_ids(), Ok(skus()));
    assert_eq!(fake.pauses(), [1, 2]);
}

#[test]
fn still_throttled_after_the_retries_the_answer_is_rate_limited() {
    let fake = Fake::connected();
    fake.server.serve(PAGE_1, 429, fixture("error-429.json"));

    assert_eq!(fake.adapter.listing_ids(), Err(PlatformError::RateLimited));
    assert_eq!(fake.pauses(), [1, 2, 4, 8]);
}

#[test]
fn concurrent_requests_renew_the_login_only_once() {
    let fake = Arc::new(Fake::connected());

    let workers: Vec<_> = (0..8)
        .map(|_| {
            let fake = fake.clone();
            std::thread::spawn(move || fake.adapter.listing_ids().map(|ids| ids.len()))
        })
        .collect();
    for worker in workers {
        assert_eq!(worker.join().unwrap(), Ok(5));
    }

    assert_eq!(fake.renewals(), 1);
}

#[test]
fn without_the_keys_nothing_is_requested() {
    let fake = Fake::not_connected();
    fake.store
        .write("AMAZON_SELLER_ID", &Secret::new("A1MASCATE0SELLER"))
        .unwrap();

    assert_eq!(fake.adapter.listing_ids(), Err(PlatformError::NotConnected));
    assert!(fake.server.received().is_empty());
}

#[test]
fn the_fee_is_amazons_estimate_for_the_listing_at_the_price() {
    let fake = Fake::connected();
    fake.serve(FONE_FEES, "fees-estimate.json");
    let basis = FeeBasis {
        category: "HEADPHONES".into(),
        listing_type: ListingType::Classic,
        listing: Some("FONE-BT-001".into()),
    };

    let fee = fake.adapter.sale_fee(&basis, brl("129.90")).unwrap();

    assert_eq!(fee.on(brl("129.90")).unwrap().rounded(), brl("18.89"));
    assert_eq!(fee.fixed, brl("2.0"));
    assert_eq!(
        fake.bodies(FONE_FEES),
        [serde_json::json!({
            "FeesEstimateRequest": {
                "MarketplaceId": "A2Q3Y263D00KWC",
                "IsAmazonFulfilled": false,
                "PriceToEstimateFees": {
                    "ListingPrice": {"CurrencyCode": "BRL", "Amount": 129.90},
                },
                "Identifier": "FONE-BT-001",
            }
        })]
    );
}

#[test]
fn without_amazons_estimate_the_fee_is_the_highest_of_its_published_table() {
    let fake = Fake::connected();
    let path = "/products/fees/v0/listings/CANECA-01/feesEstimate";
    fake.serve(path, "fees-estimate-client-error.json");
    let caneca = FeeBasis {
        category: "DRINKING_CUP".into(),
        listing_type: ListingType::Classic,
        listing: Some("CANECA-01".into()),
    };
    let draft = FeeBasis {
        listing: None,
        ..caneca.clone()
    };

    let table = SaleFee {
        rate: percent("15"),
        fixed: brl("0"),
    };
    assert_eq!(fake.adapter.sale_fee(&caneca, brl("39.90")), Ok(table));
    assert_eq!(fake.adapter.sale_fee(&draft, brl("39.90")), Ok(table));
    // At least R$ 2,00 a unit.
    assert_eq!(
        fake.adapter.sale_fee(&draft, brl("9.90")),
        Ok(SaleFee {
            rate: percent("0"),
            fixed: brl("2"),
        })
    );
    assert_eq!(
        fake.server
            .requests()
            .iter()
            .filter(|asked| asked.contains("feesEstimate"))
            .count(),
        1
    );
}

#[test]
fn a_listings_category_is_its_product_type_named_in_portuguese() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter.category("HEADPHONES"),
        Ok(ChannelCategory {
            id: "HEADPHONES".into(),
            name: "Fones de ouvido".into(),
        })
    );
    assert_eq!(fake.adapter.category("../x"), Err(PlatformError::NotFound));
}

/// Mercado Livre with one listing, so a Sync of Amazon is seen to leave it.
struct MercadoLivre;

impl OnSalesChannel for MercadoLivre {
    fn sales_channel(&self) -> SalesChannelKind {
        SalesChannelKind::MercadoLivre
    }
}

impl SalesChannel for MercadoLivre {
    fn listing_ids(&self) -> Result<Vec<String>, PlatformError> {
        Ok(vec!["MLB4100000001".into()])
    }

    fn listings(&self, _: &[String]) -> Result<Vec<ChannelListing>, PlatformError> {
        Ok(vec![ChannelListing {
            id: "MLB4100000001".into(),
            link: None,
            category: None,
            ..fone()
        }])
    }

    fn sale_fee(&self, _: &FeeBasis, _: Money) -> Result<SaleFee, PlatformError> {
        Err(PlatformError::NotFound)
    }

    fn set_price(&self, _: &str, _: Money) -> Result<(), PlatformError> {
        Err(PlatformError::NotFound)
    }

    fn set_stock(&self, _: &str, _: &[ChannelStock]) -> Result<(), PlatformError> {
        Err(PlatformError::NotFound)
    }

    fn pause(&self, _: &str) -> Result<(), PlatformError> {
        Err(PlatformError::NotFound)
    }

    fn activate(&self, _: &str) -> Result<(), PlatformError> {
        Err(PlatformError::NotFound)
    }

    fn category(&self, _: &str) -> Result<ChannelCategory, PlatformError> {
        Err(PlatformError::NotFound)
    }
}

struct Synced {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    listings: Listings,
}

async fn synced(fake: &Fake) -> Synced {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(
        Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
    ));
    let database = Arc::new(
        Database::open(&dir.path().join("mascate.db"))
            .await
            .unwrap(),
    );
    migrate(&database, clock.as_ref(), &[mascate_commerce::MIGRATIONS])
        .await
        .unwrap();
    let listings = Listings::new(database, clock.clone(), Arc::new(SequentialIds::default()));
    listings.sync(&MercadoLivre).await.unwrap();
    listings.sync(&fake.adapter).await.unwrap();
    Synced {
        _dir: dir,
        clock,
        listings,
    }
}

#[test]
fn a_sync_reads_the_amazon_listings_names_their_categories_and_suggests_by_sku() {
    block_on(async {
        let fake = Fake::connected();
        let synced = synced(&fake).await;

        let amazon: Vec<_> = synced
            .listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .filter(|listing| listing.channel == SalesChannelKind::Amazon)
            .collect();
        let read: Vec<(&str, ListingStatus)> = amazon
            .iter()
            .map(|listing| (listing.listed.id.as_str(), listing.listed.status))
            .collect();
        assert_eq!(
            read,
            [
                ("CAMISETA-P", ListingStatus::Active),
                ("CANECA-01", ListingStatus::OutOfStock),
                ("FONE-BT-001", ListingStatus::Active),
            ]
        );
        let names = synced
            .listings
            .category_names(SalesChannelKind::Amazon)
            .await
            .unwrap();
        // The cup's product type has no definition here: it stays unnamed.
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            [
                ("HEADPHONES".into(), "Fones de ouvido".into()),
                ("SHIRT".into(), "Camisa".into())
            ]
        );

        let products = [CatalogProduct {
            id: RecordId::from_u128(7),
            sku: "fone bt 001".into(),
            name: "Fone Bluetooth".into(),
        }];
        let to_link = synced.listings.to_link(&products).await.unwrap();
        let fone = to_link
            .iter()
            .find(|row| row.listing.listed.id == "FONE-BT-001")
            .unwrap();
        assert_eq!(
            fone.suggestion
                .map(|suggestion| (suggestion.product, suggestion.by)),
            Some((RecordId::from_u128(7), SuggestedBy::SellerSku))
        );
    });
}

#[test]
fn a_listing_gone_from_amazon_closes_and_mercado_livre_keeps_its_own() {
    block_on(async {
        let fake = Fake::connected();
        let synced = synced(&fake).await;
        fake.serve(PAGE_1, "listings-page-1-caneca-gone.json");
        fake.serve(
            &by_sku(&[
                "FONE-BT-001",
                "CAMISETA-PAI",
                "CAMISETA-P",
                "FONE-FBA",
                "CANECA-01",
            ]),
            "listings-by-sku-caneca-gone.json",
        );
        synced.clock.advance(TimeDelta::minutes(5));

        let sync = synced.listings.sync(&fake.adapter).await.unwrap();

        assert_eq!((sync.read, sync.new, sync.gone), (2, 0, 1));
        let statuses: Vec<(SalesChannelKind, String, ListingStatus)> = synced
            .listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .map(|listing| (listing.channel, listing.listed.id, listing.listed.status))
            .collect();
        assert!(statuses.contains(&(
            SalesChannelKind::Amazon,
            "CANECA-01".into(),
            ListingStatus::Closed
        )));
        assert!(statuses.contains(&(
            SalesChannelKind::MercadoLivre,
            "MLB4100000001".into(),
            ListingStatus::Active
        )));
    });
}

#[test]
fn a_price_reaches_amazon_only_when_the_owner_changes_it() {
    block_on(async {
        let fake = Fake::connected();
        let synced = synced(&fake).await;
        synced.listings.sync(&fake.adapter).await.unwrap();
        assert!(fake.bodies(FONE_PATCH).is_empty());
        let fone = synced
            .listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .find(|listing| listing.listed.id == "FONE-BT-001")
            .unwrap();

        let changed = synced
            .listings
            .change_price(fone.id, brl("119.90"), &fake.adapter)
            .await
            .unwrap();

        assert_eq!(changed[0].listed.price, brl("119.90"));
        let sent = fake.bodies(FONE_PATCH);
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0]["patches"][0]["path"],
            "/attributes/purchasable_offer"
        );
    });
}

#[test]
fn the_owners_pause_on_amazon_stays_while_amazon_reports_zero_units() {
    block_on(async {
        let fake = Fake::connected();
        let synced = synced(&fake).await;
        let fone = synced
            .listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .find(|listing| listing.listed.id == "FONE-BT-001")
            .unwrap();

        synced.listings.pause(fone.id, &fake.adapter).await.unwrap();
        // Amazon now shows the fone with no units, off sale.
        let zero =
            fixture("listings-by-sku.json").replacen(r#""quantity": 7"#, r#""quantity": 0"#, 1);
        fake.server.serve(&by_sku(&SKUS), 200, zero);
        synced.listings.sync(&fake.adapter).await.unwrap();

        let paused = synced.listings.listing(fone.id).await.unwrap();
        assert_eq!(paused.listed.status, ListingStatus::Paused);
        assert_eq!(paused.listed.available_quantity, 0);

        let back = synced
            .listings
            .reactivate(fone.id, &fake.adapter)
            .await
            .unwrap();
        assert_eq!(back[0].listed.status, ListingStatus::Active);
    });
}
