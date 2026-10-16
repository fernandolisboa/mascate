//! The owner's Shopee shop through the Open Platform v2 seller adapter: the
//! adapter against the fake server serving the answers in
//! `fixtures/shopee-seller`, written from the documentation (see its
//! README), and commerce's Sync of Listings end to end on a real temporary
//! database.

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use hmac::{Hmac, Mac};
use mascate_commerce::{
    CatalogProduct, ChannelCategory, ChannelListing, ChannelStock, FeeBasis, ListingStatus,
    Listings, OnSalesChannel, SaleFee, SalesChannel, SuggestedBy, Variation,
};
use mascate_integrations::{Pause, ShopAuthorization, ShopeeSeller};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{
    Currency, ListingType, Money, Percentage, PlatformError, RecordId, SalesChannelKind,
};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;
use sha2::Sha256;

const PARTNER: &str = "2001887";
const KEY: &str = "shpk-mascate-partner-key";
const SHOP: &str = "14701711";
/// 2026-10-16 12:00 UTC, the fake clock's time.
const NOW: &str = "1792152000";
const FIRST_ACCESS: &str = "59777174636562737266615546704c6d";
const SECOND_ACCESS: &str = "6c4f6d5a4c4a5a6e4f5a7a4d6b4e6c4a";

const FIRST_TOKEN: &str = "/api/v2/auth/token/get";
const RENEW: &str = "/api/v2/auth/access_token/get";
const ITEM_LIST: &str = "/api/v2/product/get_item_list";
const BASE_INFO: &str = "/api/v2/product/get_item_base_info";
const MODELS: &str = "/api/v2/product/get_model_list";
const CATEGORIES: &str = "/api/v2/product/get_category";
const UPDATE_PRICE: &str = "/api/v2/product/update_price";
const UPDATE_STOCK: &str = "/api/v2/product/update_stock";
const UNLIST: &str = "/api/v2/product/unlist_item";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/shopee-seller/{name}",
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

/// The documented signature, written here on its own: HMAC-SHA256 of
/// `base` with the partner key, lowercase hex.
fn sign(base: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(KEY.as_bytes()).unwrap();
    mac.update(base.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A public API call to `path`: signed without a shop.
fn public(path: &str) -> String {
    format!(
        "{path}?partner_id={PARTNER}&timestamp={NOW}&sign={}",
        sign(&format!("{PARTNER}{path}{NOW}"))
    )
}

/// A shop API call to `path` with `token`, then `query`.
fn shop(path: &str, token: &str, query: &str) -> String {
    format!(
        "{path}?partner_id={PARTNER}&timestamp={NOW}&access_token={token}&shop_id={SHOP}&sign={}{query}",
        sign(&format!("{PARTNER}{path}{NOW}{token}{SHOP}"))
    )
}

fn item_list(offset: u32) -> String {
    shop(
        ITEM_LIST,
        FIRST_ACCESS,
        &format!("&offset={offset}&page_size=100&item_status=NORMAL&item_status=UNLIST"),
    )
}

fn base_info(ids: &[&str]) -> String {
    shop(
        BASE_INFO,
        FIRST_ACCESS,
        &format!("&item_id_list={}", ids.join("%2C")),
    )
}

fn models(item: &str) -> String {
    shop(MODELS, FIRST_ACCESS, &format!("&item_id={item}"))
}

const ITEMS: [&str; 3] = ["800001", "800002", "800003"];

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
    adapter: Arc<ShopeeSeller>,
}

impl Fake {
    /// The partner's keys pasted and the shop's login stored.
    fn connected() -> Self {
        let fake = Self::with_keys();
        fake.store
            .write("SHOPEE_SHOP_ID", &Secret::new(SHOP))
            .unwrap();
        fake.store
            .write(
                "SHOPEE_REFRESH_TOKEN",
                &Secret::new("3a4b4c4d4e4f50515253545556575859"),
            )
            .unwrap();
        fake.serve(&public(RENEW), "access-token-get.json");
        fake.serve(&item_list(0), "item-list-page-1.json");
        fake.serve(&item_list(100), "item-list-page-2.json");
        fake.serve(&base_info(&ITEMS), "item-base-info.json");
        fake.serve(&models("800001"), "model-list-empty.json");
        fake.serve(&models("800002"), "model-list-camiseta.json");
        fake.serve(&models("800003"), "model-list-empty.json");
        fake.serve(
            &shop(CATEGORIES, FIRST_ACCESS, "&language=pt-br"),
            "categories.json",
        );
        for path in [UPDATE_PRICE, UPDATE_STOCK, UNLIST] {
            fake.serve(&shop(path, FIRST_ACCESS, ""), "update-ok.json");
        }
        fake
    }

    /// Only the partner's keys pasted: the owner has not logged in yet.
    fn with_keys() -> Self {
        let fake = Self::empty();
        fake.store
            .write("SHOPEE_PARTNER_ID", &Secret::new(PARTNER))
            .unwrap();
        fake.store
            .write("SHOPEE_PARTNER_KEY", &Secret::new(KEY))
            .unwrap();
        fake
    }

    fn empty() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
        ));
        let pauses = Arc::new(RecordedPauses::default());
        let adapter = Arc::new(
            ShopeeSeller::new(server.url(), "Mascate/test", store.clone(), clock)
                .with_pause(pauses.clone()),
        );
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
            .filter(|path| path.starts_with(RENEW))
            .count()
    }

    /// The JSON bodies sent to `path`, in order.
    fn bodies(&self, path: &str) -> Vec<serde_json::Value> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.path.starts_with(&format!("{path}?")))
            .map(|request| serde_json::from_str(&request.body).unwrap())
            .collect()
    }

    fn stored(&self, name: &str) -> Option<String> {
        self.store
            .read(name)
            .unwrap()
            .map(|secret| secret.expose().to_owned())
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
        id: "800001".into(),
        variation: None,
        title: "Fone de Ouvido Bluetooth Mascate Preto".into(),
        price: brl("129.9"),
        available_quantity: 7,
        status: ListingStatus::Active,
        link: Some("https://shopee.com.br/product/14701711/800001".into()),
        listing_type: Some(ListingType::default()),
        category: Some("100636".into()),
        seller_sku: Some("FONE-BT-001".into()),
    }
}

fn camiseta(model: &str, name: &str) -> ChannelListing {
    ChannelListing {
        id: "800002".into(),
        variation: Some(Variation {
            id: model.into(),
            name: name.into(),
        }),
        title: "Camiseta Básica Algodão".into(),
        price: brl("49.9"),
        link: Some("https://shopee.com.br/product/14701711/800002".into()),
        category: Some("100017".into()),
        ..fone()
    }
}

#[test]
fn a_shop_call_is_signed_over_the_documented_base_string() {
    let fake = Fake::connected();

    fake.adapter.listing_ids().unwrap();

    let first = fake
        .server
        .requests()
        .into_iter()
        .find(|path| path.starts_with(ITEM_LIST))
        .unwrap();
    // HMAC-SHA256 with the partner key of partner_id, path, timestamp,
    // access_token and shop_id, worked out apart from the app.
    assert_eq!(
        first,
        format!(
            "{ITEM_LIST}?partner_id=2001887&timestamp=1792152000\
             &access_token={FIRST_ACCESS}&shop_id=14701711\
             &sign=b05fe3aa7f24de683e8d43e46343bea47f8f52201c63d8fe9214df6a92e60942\
             &offset=0&page_size=100&item_status=NORMAL&item_status=UNLIST"
        )
    );
}

#[test]
fn the_authorization_page_is_signed_without_a_shop_and_returns_to_the_redirect() {
    let fake = Fake::with_keys();

    let url = fake
        .adapter
        .authorization_url("https://github.com/fernandolisboa")
        .unwrap();

    assert_eq!(
        url,
        format!(
            "{}/api/v2/shop/auth_partner?partner_id=2001887&timestamp=1792152000\
             &sign=3ad257576adc6e2cdedd6abb98b44872b818e4edff10f356bbe888589dc1c0fe\
             &redirect=https%3A%2F%2Fgithub.com%2Ffernandolisboa",
            fake.server.url()
        )
    );
    assert!(fake.server.requests().is_empty());
}

#[test]
fn the_address_the_browser_landed_on_gives_the_code_and_the_shop() {
    let found = ShopAuthorization::from_landing(
        " https://github.com/fernandolisboa?code=4f6b4d6a5a4c&shop_id=14701711#top ",
    );
    assert_eq!(
        found,
        Some(ShopAuthorization {
            code: "4f6b4d6a5a4c".into(),
            shop_id: "14701711".into(),
        })
    );
    // The query alone, in any order.
    assert_eq!(
        ShopAuthorization::from_landing("shop_id=14701711&code=4f6b4d6a5a4c"),
        found
    );
    for wrong in [
        "https://github.com/fernandolisboa",
        "https://github.com/fernandolisboa?code=4f6b&shop_id=",
        "https://github.com/fernandolisboa?code=4f6b&shop_id=147a",
        "https://github.com/fernandolisboa?code=4f%26b&shop_id=14701711",
        "https://github.com/fernandolisboa?shop_id=14701711",
    ] {
        assert_eq!(ShopAuthorization::from_landing(wrong), None, "{wrong}");
    }
}

#[test]
fn authorizing_keeps_the_login_in_the_store_and_uses_it_without_renewing() {
    let fake = Fake::with_keys();
    fake.serve(&public(FIRST_TOKEN), "token-get.json");
    fake.serve(&item_list(0), "item-list-page-2.json");

    fake.adapter
        .authorize(&ShopAuthorization {
            code: "4f6b4d6a5a4c".into(),
            shop_id: SHOP.into(),
        })
        .unwrap();

    assert_eq!(
        fake.bodies(FIRST_TOKEN),
        [serde_json::json!({"code": "4f6b4d6a5a4c", "shop_id": 14701711, "partner_id": 2001887})]
    );
    assert_eq!(fake.stored("SHOPEE_SHOP_ID").as_deref(), Some(SHOP));
    assert_eq!(
        fake.stored("SHOPEE_REFRESH_TOKEN").as_deref(),
        Some("4b4b4a6c4e6b4d7a4f5a6e526d5a4c79")
    );
    assert_eq!(fake.adapter.listing_ids().unwrap(), ["800003"]);
    assert_eq!(fake.renewals(), 0);
}

#[test]
fn a_code_shopee_refuses_keeps_nothing_and_carries_its_message() {
    let fake = Fake::with_keys();
    fake.serve(&public(FIRST_TOKEN), "code-refused.json");

    let refused = fake.adapter.authorize(&ShopAuthorization {
        code: "4f6b4d6a5a4c".into(),
        shop_id: SHOP.into(),
    });

    assert_eq!(refused, Err(PlatformError::Refused("Invalid code.".into())));
    assert_eq!(fake.stored("SHOPEE_SHOP_ID"), None);
    assert_eq!(fake.stored("SHOPEE_REFRESH_TOKEN"), None);
}

#[test]
fn the_items_on_sale_and_unlisted_are_read_page_by_page() {
    let fake = Fake::connected();

    let ids = fake.adapter.listing_ids().unwrap();

    assert_eq!(ids, ITEMS);
    assert_eq!(
        fake.server.requests(),
        [public(RENEW), item_list(0), item_list(100)]
    );
}

#[test]
fn an_item_with_models_is_one_listing_per_model() {
    let fake = Fake::connected();
    let ids: Vec<String> = ITEMS.map(String::from).to_vec();

    let listings = fake.adapter.listings(&ids).unwrap();

    let preta_p = ChannelListing {
        available_quantity: 3,
        seller_sku: Some("CAMISETA-PRETA-P".into()),
        ..camiseta("900021", "Cor: Preta · Tamanho: P")
    };
    // No units: Shopee keeps it listed and sold out.
    let preta_m = ChannelListing {
        available_quantity: 0,
        status: ListingStatus::OutOfStock,
        seller_sku: Some("CAMISETA-PRETA-M".into()),
        ..camiseta("900022", "Cor: Preta · Tamanho: M")
    };
    // A model the owner turned off, without a SKU of its own.
    let branca_p = ChannelListing {
        price: brl("52.9"),
        available_quantity: 4,
        status: ListingStatus::Inactive,
        seller_sku: Some("CAMISETA".into()),
        ..camiseta("900023", "Cor: Branca · Tamanho: P")
    };
    let caneca = ChannelListing {
        id: "800003".into(),
        title: "Caneca de Cerâmica 350 ml".into(),
        price: brl("39.9"),
        available_quantity: 2,
        status: ListingStatus::Paused,
        link: Some("https://shopee.com.br/product/14701711/800003".into()),
        seller_sku: None,
        ..fone()
    };
    assert_eq!(listings, [fone(), preta_p, preta_m, branca_p, caneca]);
    // Only the item with models is asked for them.
    assert_eq!(
        fake.server.requests()[1..],
        [base_info(&ITEMS), models("800002")]
    );
}

#[test]
fn more_than_fifty_items_are_asked_fifty_at_a_time() {
    let fake = Fake::connected();
    let many: Vec<String> = (1..=51).map(|n| format!("{}", 700000 + n)).collect();
    let first: Vec<&str> = many[..50].iter().map(String::as_str).collect();
    let empty = r#"{"error":"","message":"","response":{"item_list":[]}}"#;
    fake.server.serve(&base_info(&first), 200, empty);
    fake.server.serve(&base_info(&["700051"]), 200, empty);

    assert_eq!(fake.adapter.listings(&many), Ok(Vec::new()));
    assert_eq!(
        fake.server.requests()[1..],
        [base_info(&first), base_info(&["700051"])]
    );
}

#[test]
fn an_id_that_is_not_shopees_is_never_asked() {
    let fake = Fake::connected();

    let listings = fake
        .adapter
        .listings(&["MLB123".into(), "../800001".into()])
        .unwrap();

    assert!(listings.is_empty());
    assert!(fake.server.requests().is_empty());
}

#[test]
fn a_new_price_goes_to_every_model_read_right_before() {
    let fake = Fake::connected();

    fake.adapter.set_price("800002", brl("54.899")).unwrap();
    fake.adapter.set_price("800001", brl("119.899")).unwrap();

    assert_eq!(
        fake.bodies(UPDATE_PRICE),
        [
            serde_json::json!({"item_id": 800002, "price_list": [
                {"model_id": 900021, "original_price": 54.90},
                {"model_id": 900022, "original_price": 54.90},
                {"model_id": 900023, "original_price": 54.90},
            ]}),
            // An item without variations is model 0.
            serde_json::json!({"item_id": 800001, "price_list": [
                {"model_id": 0, "original_price": 119.90},
            ]}),
        ]
    );
    let sent = fake.server.received().pop().unwrap();
    assert_eq!(sent.method, "POST");
    // The decimal's own text: a price never goes through a float.
    assert!(sent.body.contains(r#""original_price":119.90"#));
}

#[test]
fn the_stock_goes_to_each_model_named_and_the_others_keep_theirs() {
    let fake = Fake::connected();

    fake.adapter
        .set_stock(
            "800002",
            &[ChannelStock {
                variation: Some("900022".into()),
                available_quantity: 2,
            }],
        )
        .unwrap();
    fake.adapter
        .set_stock(
            "800001",
            &[ChannelStock {
                variation: None,
                available_quantity: 0,
            }],
        )
        .unwrap();

    assert_eq!(
        fake.bodies(UPDATE_STOCK),
        [
            serde_json::json!({"item_id": 800002, "stock_list": [
                {"model_id": 900022, "seller_stock": [{"stock": 2}]},
            ]}),
            serde_json::json!({"item_id": 800001, "stock_list": [
                {"model_id": 0, "seller_stock": [{"stock": 0}]},
            ]}),
        ]
    );
    assert_eq!(
        fake.adapter.set_stock(
            "800002",
            &[ChannelStock {
                variation: Some("P".into()),
                available_quantity: 1,
            }]
        ),
        Err(PlatformError::NotFound)
    );
}

#[test]
fn pausing_unlists_the_item_and_activating_lists_it_again() {
    let fake = Fake::connected();

    fake.adapter.pause("800001").unwrap();
    fake.adapter.activate("800001").unwrap();

    assert_eq!(
        fake.bodies(UNLIST),
        [
            serde_json::json!({"item_list": [{"item_id": 800001, "unlist": true}]}),
            serde_json::json!({"item_list": [{"item_id": 800001, "unlist": false}]}),
        ]
    );
    assert!(!fake.adapter.pauses_with_zero_stock());
}

#[test]
fn a_change_shopee_refuses_names_its_reasons() {
    let fake = Fake::connected();
    fake.serve(&shop(UPDATE_PRICE, FIRST_ACCESS, ""), "update-refused.json");

    assert_eq!(
        fake.adapter.set_price("800002", brl("9999")),
        Err(PlatformError::Refused(
            "Price is out of the allowed range.".into()
        ))
    );
}

#[test]
fn a_turned_down_token_is_renewed_once_and_the_new_one_goes_through() {
    let fake = Fake::connected();
    fake.serve(&item_list(0), "error-auth.json");
    fake.server
        .then_serve(&public(RENEW), 200, fixture("access-token-get-again.json"));
    fake.serve(
        &shop(
            ITEM_LIST,
            SECOND_ACCESS,
            "&offset=0&page_size=100&item_status=NORMAL&item_status=UNLIST",
        ),
        "item-list-page-2.json",
    );

    assert_eq!(fake.adapter.listing_ids().unwrap(), ["800003"]);
    assert_eq!(fake.renewals(), 2);
    // Each renewal's refresh token replaced the one it used.
    assert_eq!(
        fake.stored("SHOPEE_REFRESH_TOKEN").as_deref(),
        Some("7a4d6a4e5a4a6b6a4d6e41494b4c4f62")
    );
}

#[test]
fn a_token_turned_down_again_after_renewing_means_the_login_expired() {
    let fake = Fake::connected();
    fake.serve(&item_list(0), "error-auth.json");
    fake.server
        .then_serve(&public(RENEW), 200, fixture("access-token-get-again.json"));
    fake.serve(
        &shop(
            ITEM_LIST,
            SECOND_ACCESS,
            "&offset=0&page_size=100&item_status=NORMAL&item_status=UNLIST",
        ),
        "error-auth.json",
    );

    assert_eq!(fake.adapter.listing_ids(), Err(PlatformError::Expired));
    assert_eq!(fake.renewals(), 2);
}

#[test]
fn the_renewed_refresh_token_is_stored_before_the_access_token_is_used() {
    let fake = Fake::connected();

    fake.adapter.listing_ids().unwrap();

    assert_eq!(
        fake.bodies(RENEW),
        [serde_json::json!({
            "refresh_token": "3a4b4c4d4e4f50515253545556575859",
            "shop_id": 14701711,
            "partner_id": 2001887,
        })]
    );
    assert_eq!(
        fake.stored("SHOPEE_REFRESH_TOKEN").as_deref(),
        Some("6f4c4b49416e4d6a6b4a5a4e6a4d7a51")
    );
}

#[test]
fn a_refresh_token_shopee_refuses_means_the_login_expired() {
    let fake = Fake::connected();
    fake.serve(&public(RENEW), "refresh-refused.json");

    assert_eq!(fake.adapter.listing_ids(), Err(PlatformError::Expired));
    assert_eq!(
        fake.stored("SHOPEE_REFRESH_TOKEN").as_deref(),
        Some("3a4b4c4d4e4f50515253545556575859")
    );
    assert_eq!(fake.server.requests(), [public(RENEW)]);
}

#[test]
fn concurrent_requests_renew_the_single_use_token_only_once() {
    let fake = Fake::connected();
    fake.server.serve_late(
        &public(RENEW),
        Duration::from_millis(300),
        200,
        fixture("access-token-get.json"),
    );

    let threads: Vec<_> = (0..2)
        .map(|_| {
            let adapter = fake.adapter.clone();
            std::thread::spawn(move || adapter.listing_ids())
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().unwrap().unwrap(), ITEMS);
    }

    // A second renewal would spend the refresh token the first one got.
    assert_eq!(fake.renewals(), 1);
}

#[test]
fn a_throttled_request_waits_longer_each_time_and_then_goes_through() {
    let fake = Fake::connected();
    let path = item_list(0);
    fake.server
        .serve(&path, 200, fixture("error-rate-limit.json"));
    fake.server
        .then_serve(&path, 429, fixture("error-rate-limit.json"));
    fake.server
        .then_serve(&path, 200, fixture("item-list-page-2.json"));

    assert_eq!(fake.adapter.listing_ids().unwrap(), ["800003"]);
    assert_eq!(fake.pauses(), [1, 2]);
}

#[test]
fn past_the_daily_limit_the_answer_is_rate_limited_without_waiting() {
    let fake = Fake::connected();
    fake.serve(&item_list(0), "error-limit.json");

    assert_eq!(fake.adapter.listing_ids(), Err(PlatformError::RateLimited));
    assert!(fake.pauses().is_empty());
}

#[test]
fn without_the_login_nothing_is_requested() {
    let fake = Fake::with_keys();
    assert_eq!(fake.adapter.listing_ids(), Err(PlatformError::NotConnected));
    let fake = Fake::empty();
    assert_eq!(
        fake.adapter
            .authorization_url("https://github.com/fernandolisboa"),
        Err(PlatformError::NotConnected)
    );
    assert!(fake.server.requests().is_empty());
}

#[test]
fn the_fee_is_shopees_published_table_by_the_price() {
    let fake = Fake::connected();
    let fee = |price: &str| {
        let basis = FeeBasis {
            category: "100636".into(),
            listing_type: ListingType::default(),
            listing: Some("800001".into()),
        };
        fake.adapter.sale_fee(&basis, brl(price)).unwrap()
    };
    let table = |rate: &str, fixed: &str| SaleFee {
        rate: percent(rate),
        fixed: brl(fixed),
    };

    assert_eq!(fee("5.00"), table("20", "2.50"));
    assert_eq!(fee("8.00"), table("20", "4"));
    assert_eq!(fee("79.99"), table("20", "4"));
    assert_eq!(fee("80.00"), table("14", "16"));
    assert_eq!(fee("199.99"), table("14", "20"));
    assert_eq!(fee("200.00"), table("14", "26"));
    assert_eq!(fee("1500.00"), table("14", "26"));
    // No call: Shopee's API has no fee estimate.
    assert!(fake.server.requests().is_empty());
}

#[test]
fn a_category_is_named_as_shopee_shows_it_in_portuguese() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter.category("100017"),
        Ok(ChannelCategory {
            id: "100017".into(),
            name: "Camisetas".into(),
        })
    );
    assert_eq!(fake.adapter.category("999"), Err(PlatformError::NotFound));
    assert_eq!(fake.adapter.category("1/2"), Err(PlatformError::NotFound));
}

/// Mercado Livre with one listing, to show a Shopee Sync leaves it alone.
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
    listings.sync(fake.adapter.as_ref()).await.unwrap();
    Synced {
        _dir: dir,
        clock,
        listings,
    }
}

#[test]
fn a_sync_reads_each_model_names_the_categories_and_suggests_by_sku() {
    block_on(async {
        let fake = Fake::connected();
        let synced = synced(&fake).await;

        let mut shopee: Vec<(String, Option<String>, ListingStatus)> = synced
            .listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .filter(|listing| listing.channel == SalesChannelKind::Shopee)
            .map(|listing| {
                (
                    listing.listed.id,
                    listing.listed.variation.map(|variation| variation.id),
                    listing.listed.status,
                )
            })
            .collect();
        let expected = [
            ("800001".to_owned(), None, ListingStatus::Active),
            (
                "800002".to_owned(),
                Some("900021".to_owned()),
                ListingStatus::Active,
            ),
            (
                "800002".to_owned(),
                Some("900022".to_owned()),
                ListingStatus::OutOfStock,
            ),
            (
                "800002".to_owned(),
                Some("900023".to_owned()),
                ListingStatus::Inactive,
            ),
            ("800003".to_owned(), None, ListingStatus::Paused),
        ];
        shopee.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        assert_eq!(shopee, expected);
        let names = synced
            .listings
            .category_names(SalesChannelKind::Shopee)
            .await
            .unwrap();
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            [
                ("100017".into(), "Camisetas".into()),
                ("100636".into(), "Fones de Ouvido".into())
            ]
        );

        let products = [CatalogProduct {
            id: RecordId::from_u128(7),
            sku: "camiseta preta m".into(),
            name: "Camiseta preta M".into(),
        }];
        let to_link = synced.listings.to_link(&products).await.unwrap();
        let preta_m = to_link
            .iter()
            .find(|row| {
                row.listing.listed.variation.as_ref().map(|v| v.id.as_str()) == Some("900022")
            })
            .unwrap();
        assert_eq!(
            preta_m
                .suggestion
                .map(|suggestion| (suggestion.product, suggestion.by)),
            Some((RecordId::from_u128(7), SuggestedBy::SellerSku))
        );
    });
}

#[test]
fn an_item_gone_from_shopee_closes_and_mercado_livre_keeps_its_own() {
    block_on(async {
        let fake = Fake::connected();
        let synced = synced(&fake).await;
        // The cup was deleted on Shopee: the list and the details leave it
        // out.
        fake.server.serve(
            &item_list(100),
            200,
            r#"{"error":"","message":"","response":{"item":[],"has_next_page":false}}"#,
        );
        let without_caneca: serde_json::Value = {
            let mut info: serde_json::Value =
                serde_json::from_str(&fixture("item-base-info.json")).unwrap();
            info["response"]["item_list"]
                .as_array_mut()
                .unwrap()
                .truncate(2);
            info
        };
        fake.server.serve(
            &base_info(&["800001", "800002", "800003"]),
            200,
            without_caneca.to_string(),
        );
        synced.clock.advance(TimeDelta::minutes(5));

        let sync = synced.listings.sync(fake.adapter.as_ref()).await.unwrap();

        assert_eq!(sync.gone, 1);
        let statuses: Vec<(SalesChannelKind, String, ListingStatus)> = synced
            .listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .map(|listing| (listing.channel, listing.listed.id, listing.listed.status))
            .collect();
        assert!(statuses.contains(&(
            SalesChannelKind::Shopee,
            "800003".into(),
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
fn a_price_reaches_shopee_only_when_the_owner_changes_it() {
    block_on(async {
        let fake = Fake::connected();
        let synced = synced(&fake).await;
        synced.listings.sync(fake.adapter.as_ref()).await.unwrap();
        assert!(fake.bodies(UPDATE_PRICE).is_empty());
        let preta_p = synced
            .listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .find(|listing| {
                listing.listed.variation.as_ref().map(|v| v.id.as_str()) == Some("900021")
            })
            .unwrap();

        let changed = synced
            .listings
            .change_price(preta_p.id, brl("54.90"), fake.adapter.as_ref())
            .await
            .unwrap();

        // The models of an item share its price.
        assert_eq!(changed.len(), 3);
        assert!(
            changed
                .iter()
                .all(|listing| listing.listed.price == brl("54.90"))
        );
        assert_eq!(fake.bodies(UPDATE_PRICE).len(), 1);
    });
}
