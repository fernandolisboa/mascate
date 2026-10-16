//! The owner's Magalu store through the Open API adapter: the adapter
//! against the fake server serving the answers in `fixtures/magalu`,
//! written from the documentation (see its README), and commerce's Sync of
//! Listings end to end on a real temporary database.

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    ChannelListing, ChannelStock, FeeBasis, ListingStatus, Listings, OnSalesChannel, SaleFee,
    SalesChannel, SentPrice, SentPriceState,
};
use mascate_integrations::{MAGALU_SCOPES, Magalu, Pause, StoreAuthorization};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{
    Clock, Currency, ListingType, Money, Percentage, PlatformError, SalesChannelKind,
};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;

const CLIENT_ID: &str = "mascate-client-id";
const CLIENT_SECRET: &str = "mascate-client-secret";
const FIRST_REFRESH: &str = "eyJhbGciOiJSUzI1NiJ9.primeiro-refresh.assinatura";
const SECOND_REFRESH: &str = "eyJhbGciOiJSUzI1NiJ9.segundo-refresh.assinatura";
const FIRST_ACCESS: &str = "eyJhbGciOiJSUzI1NiJ9.primeiro-acesso.assinatura";
const CHANNEL: &str = "9fe0d853-732b-4e4a-a0b0-cff988ed043d";
const REDIRECT: &str = "https://github.com/fernandolisboa";

const TOKEN: &str = "/oauth/token";
const ME: &str = "/seller/v1/portfolios/me";
const SKUS: &str = "/seller/v1/portfolios/skus";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/magalu/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

fn sku_path(kind: &str, sku: &str) -> String {
    format!("/seller/v1/portfolios/{kind}/{sku}")
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
    clock: Arc<ManualClock>,
    pauses: Arc<RecordedPauses>,
    adapter: Arc<Magalu>,
}

impl Fake {
    /// The client's keys pasted, with no login yet.
    fn with_keys() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        store
            .write("MAGALU_CLIENT_ID", &Secret::new(CLIENT_ID))
            .unwrap();
        store
            .write("MAGALU_CLIENT_SECRET", &Secret::new(CLIENT_SECRET))
            .unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
        ));
        let pauses = Arc::new(RecordedPauses::default());
        let adapter = Arc::new(
            Magalu::new(
                server.url(),
                server.url(),
                "Mascate/test",
                store.clone(),
                clock.clone(),
            )
            .with_pause(pauses.clone()),
        );
        server.serve(TOKEN, 200, fixture("token.json"));
        server.serve(ME, 200, fixture("me.json"));
        Self {
            server,
            store,
            clock,
            pauses,
            adapter,
        }
    }

    /// The store's login stored, with its three SKUs on sale.
    fn connected() -> Self {
        let fake = Self::with_keys();
        fake.store
            .write("MAGALU_REFRESH_TOKEN", &Secret::new(FIRST_REFRESH))
            .unwrap();
        fake.server
            .serve_any_query(SKUS, 200, fixture("skus-page.json"));
        for sku in ["CAM-PRETA-M", "FONE-TWS", "CANECA-300"] {
            let name = sku.to_lowercase();
            for (kind, file) in [
                ("skus", format!("sku-{name}.json")),
                ("prices", format!("prices-{name}.json")),
                ("stocks", format!("stocks-{name}.json")),
            ] {
                fake.server
                    .serve_method("GET", &sku_path(kind, sku), 200, fixture(&file));
            }
        }
        fake
    }

    fn stored(&self, name: &str) -> Option<String> {
        self.store
            .read(name)
            .unwrap()
            .map(|secret| secret.expose().to_owned())
    }

    fn renewals(&self) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|path| path.as_str() == TOKEN)
            .count()
    }

    fn sent(&self, method: &str, path: &str) -> Vec<serde_json::Value> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.method == method && request.path == path)
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

#[test]
fn the_adapter_is_magalus() {
    assert_eq!(
        Fake::with_keys().adapter.sales_channel(),
        SalesChannelKind::Magalu
    );
}

#[test]
fn the_login_page_asks_for_the_store_and_every_scope() {
    let fake = Fake::with_keys();

    let link = fake.adapter.authorization_url(REDIRECT).unwrap();

    assert_eq!(
        link,
        format!(
            "{}/login?client_id={CLIENT_ID}&redirect_uri=https%3A%2F%2Fgithub.com%2Ffernandolisboa&scope={}&response_type=code&choose_tenants=true",
            fake.server.url(),
            MAGALU_SCOPES.replace(':', "%3A").replace(' ', "%20")
        )
    );
    assert!(MAGALU_SCOPES.contains("open:portfolio-prices-seller:write"));
    assert!(MAGALU_SCOPES.contains("open:portfolio-stocks-seller:write"));
    assert!(MAGALU_SCOPES.contains("open:order-delivery-seller:read"));
    assert!(fake.server.requests().is_empty());
}

#[test]
fn the_code_comes_out_of_the_address_the_browser_landed_on() {
    let code = |address: &str| StoreAuthorization::from_landing(address).map(|found| found.code);

    assert_eq!(
        code("https://github.com/fernandolisboa?code=ab12-CD.34_ef~&state=x").as_deref(),
        Some("ab12-CD.34_ef~")
    );
    assert_eq!(code("  code=xyz#top ").as_deref(), Some("xyz"));
    assert_eq!(code("https://github.com/fernandolisboa"), None);
    assert_eq!(code("https://github.com/fernandolisboa?code="), None);
    assert_eq!(code("https://github.com/fernandolisboa?code=a%20b"), None);
}

#[test]
fn the_code_is_traded_for_the_login_and_its_refresh_token_is_stored() {
    let fake = Fake::with_keys();

    fake.adapter
        .authorize(
            &StoreAuthorization {
                code: "codigo-de-uso-unico".into(),
            },
            REDIRECT,
        )
        .unwrap();

    assert_eq!(
        fake.sent("POST", TOKEN),
        [serde_json::json!({
            "client_id": CLIENT_ID,
            "client_secret": CLIENT_SECRET,
            "redirect_uri": REDIRECT,
            "code": "codigo-de-uso-unico",
            "grant_type": "authorization_code",
        })]
    );
    assert_eq!(
        fake.stored("MAGALU_REFRESH_TOKEN").as_deref(),
        Some(SECOND_REFRESH)
    );
    // The login just traded serves the next call without a renewal.
    fake.server.serve_any_query(SKUS, 200, r#"{"results": []}"#);
    fake.adapter.listing_ids().unwrap();
    assert_eq!(fake.renewals(), 1);
}

#[test]
fn a_refused_code_keeps_no_login() {
    let fake = Fake::with_keys();
    fake.server.serve(TOKEN, 400, fixture("token-refused.json"));

    let refused = fake.adapter.authorize(
        &StoreAuthorization {
            code: "vencido".into(),
        },
        REDIRECT,
    );

    assert_eq!(
        refused,
        Err(PlatformError::Refused(
            "The provided authorization grant is invalid, expired or revoked.".into()
        ))
    );
    assert_eq!(fake.stored("MAGALU_REFRESH_TOKEN"), None);
}

#[test]
fn without_the_keys_or_the_login_nothing_is_requested() {
    let fake = Fake::with_keys();
    assert_eq!(fake.adapter.listing_ids(), Err(PlatformError::NotConnected));
    fake.store.remove("MAGALU_CLIENT_ID").unwrap();
    assert_eq!(
        fake.adapter.authorization_url(REDIRECT),
        Err(PlatformError::NotConnected)
    );
    assert!(fake.server.requests().is_empty());
}

#[test]
fn the_renewed_refresh_token_is_stored_before_the_access_token_is_used() {
    let fake = Fake::connected();

    fake.adapter.listing_ids().unwrap();

    let renewal = fake
        .server
        .received()
        .into_iter()
        .find(|request| request.path == TOKEN)
        .unwrap();
    assert_eq!(
        renewal.body,
        format!(
            "grant_type=refresh_token&client_id={CLIENT_ID}&client_secret={CLIENT_SECRET}&refresh_token={FIRST_REFRESH}"
        )
    );
    assert_eq!(
        fake.stored("MAGALU_REFRESH_TOKEN").as_deref(),
        Some(SECOND_REFRESH)
    );
    let listed = fake
        .server
        .received()
        .into_iter()
        .find(|request| request.path.starts_with(SKUS))
        .unwrap();
    assert_eq!(listed.authorization, Some(format!("Bearer {FIRST_ACCESS}")));
}

#[test]
fn a_refresh_token_magalu_refuses_means_the_login_expired() {
    let fake = Fake::connected();
    fake.server.serve(TOKEN, 400, fixture("token-refused.json"));

    assert_eq!(fake.adapter.listing_ids(), Err(PlatformError::Expired));
    assert_eq!(
        fake.stored("MAGALU_REFRESH_TOKEN").as_deref(),
        Some(FIRST_REFRESH)
    );
}

#[test]
fn a_renewal_without_a_new_refresh_token_keeps_the_one_stored() {
    let fake = Fake::connected();
    fake.server.serve(
        TOKEN,
        200,
        r#"{"access_token": "acesso-sem-refresh", "token_type": "Bearer", "expires_in": 7200}"#,
    );

    fake.adapter.listing_ids().unwrap();

    assert_eq!(
        fake.stored("MAGALU_REFRESH_TOKEN").as_deref(),
        Some(FIRST_REFRESH)
    );
}

#[test]
fn a_token_turned_down_is_renewed_once_and_the_call_goes_again() {
    let fake = Fake::connected();
    fake.server
        .serve_any_query(SKUS, 401, fixture("error-401.json"));
    fake.server
        .then_serve(&format!("{SKUS}?*"), 200, fixture("skus-page.json"));
    fake.server
        .then_serve(TOKEN, 200, fixture("token-renewed.json"));

    assert_eq!(
        fake.adapter.listing_ids().unwrap(),
        ["CAM-PRETA-M", "FONE-TWS", "CANECA-300"]
    );
    assert_eq!(fake.renewals(), 2);
    assert_eq!(
        fake.stored("MAGALU_REFRESH_TOKEN").as_deref(),
        Some("eyJhbGciOiJSUzI1NiJ9.terceiro-refresh.assinatura")
    );
}

#[test]
fn a_second_401_after_the_renewal_means_the_login_expired() {
    let fake = Fake::connected();
    fake.server
        .serve_any_query(SKUS, 401, fixture("error-401.json"));

    assert_eq!(fake.adapter.listing_ids(), Err(PlatformError::Expired));
    assert_eq!(fake.renewals(), 2);
}

#[test]
fn two_concurrent_calls_renew_the_single_use_token_only_once() {
    let fake = Fake::connected();
    fake.server.serve_late(
        TOKEN,
        Duration::from_millis(300),
        200,
        fixture("token.json"),
    );

    let threads: Vec<_> = (0..2)
        .map(|_| {
            let adapter = fake.adapter.clone();
            std::thread::spawn(move || adapter.listing_ids())
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().unwrap().unwrap().len(), 3);
    }

    // A second renewal would spend the refresh token the first one got.
    assert_eq!(fake.renewals(), 1);
}

#[test]
fn an_access_token_about_to_expire_is_renewed_first() {
    let fake = Fake::connected();
    fake.adapter.listing_ids().unwrap();
    fake.server.serve(TOKEN, 200, fixture("token-renewed.json"));

    fake.clock.advance(TimeDelta::minutes(116));
    fake.adapter.listing_ids().unwrap();

    assert_eq!(fake.renewals(), 2);
}

#[test]
fn too_many_requests_waits_past_a_minute_and_then_goes_through() {
    let fake = Fake::connected();
    let page = format!("{SKUS}?*");
    fake.server
        .serve_any_query(SKUS, 429, fixture("error-429.json"));
    fake.server
        .then_serve(&page, 429, fixture("error-429.json"));
    fake.server
        .then_serve(&page, 200, fixture("skus-page.json"));

    assert_eq!(fake.adapter.listing_ids().unwrap().len(), 3);
    assert_eq!(fake.pauses(), [5, 10]);
}

#[test]
fn still_too_many_requests_after_every_wait_is_rate_limited() {
    let fake = Fake::connected();
    fake.server
        .serve_any_query(SKUS, 429, fixture("error-429.json"));

    assert_eq!(fake.adapter.listing_ids(), Err(PlatformError::RateLimited));
    assert_eq!(fake.pauses(), [5, 10, 20, 40]);
}

#[test]
fn the_portfolio_is_paged_a_hundred_at_a_time_leaving_out_deleted_and_fulfillment_skus() {
    let fake = Fake::connected();
    let page = |skus: Vec<serde_json::Value>| serde_json::json!({ "results": skus }).to_string();
    let sku = |code: String| serde_json::json!({"sku": code, "status": "PUBLISHED"});
    fake.server.serve(
        &format!("{SKUS}?_offset=0&_limit=100&_sort=created_at%3Aasc"),
        200,
        page((1..=100).map(|n| sku(format!("SKU-{n:03}"))).collect()),
    );
    fake.server.serve(
        &format!("{SKUS}?_offset=100&_limit=100&_sort=created_at%3Aasc"),
        200,
        fixture("skus-page.json"),
    );

    let ids = fake.adapter.listing_ids().unwrap();

    assert_eq!(ids.len(), 103);
    assert_eq!(ids[0], "SKU-001");
    assert_eq!(ids[100..], ["CAM-PRETA-M", "FONE-TWS", "CANECA-300"]);
}

#[test]
fn each_sku_is_a_listing_with_its_price_and_stock_in_the_stores_channel() {
    let fake = Fake::connected();

    let listings = fake
        .adapter
        .listings(&["CAM-PRETA-M".into(), "FONE-TWS".into(), "CANECA-300".into()])
        .unwrap();

    let camiseta = ChannelListing {
        id: "CAM-PRETA-M".into(),
        variation: None,
        title: "Camiseta Básica Preta M".into(),
        // The other channel's R$ 59,90 and 40 units are not the store's;
        // two warehouses add up, the reserved unit is already sold.
        price: brl("49.90"),
        available_quantity: 7,
        status: ListingStatus::Active,
        link: Some("https://www.magazineluiza.com.br/p/cam-preta-m/".into()),
        listing_type: Some(ListingType::default()),
        category: None,
        seller_sku: Some("CAM-PRETA-M".into()),
    };
    assert_eq!(listings[0], camiseta);
    assert_eq!(
        (
            listings[1].price,
            listings[1].available_quantity,
            listings[1].status
        ),
        (brl("89.90"), 0, ListingStatus::OutOfStock)
    );
    // Turned off in Magalu's panel: the owner's pause.
    assert_eq!(listings[2].status, ListingStatus::Paused);
}

#[test]
fn a_sku_magalu_no_longer_has_or_without_a_price_is_left_out() {
    let fake = Fake::connected();
    fake.server.serve_method(
        "GET",
        &sku_path("prices", "FONE-TWS"),
        200,
        r#"{"results": [{"channel": {"id": "outro-canal"}, "price": 100, "normalizer": 100}]}"#,
    );

    let listings = fake
        .adapter
        .listings(&["SUMIU".into(), "FONE-TWS".into(), "CANECA-300".into()])
        .unwrap();

    assert_eq!(
        listings.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(),
        ["CANECA-300"]
    );
}

#[test]
fn a_price_goes_in_cents_as_both_prices_to_the_stores_channel() {
    let fake = Fake::connected();
    let path = sku_path("prices", "CAM-PRETA-M");
    fake.server
        .serve_method("PATCH", &path, 202, fixture("accepted.json"));

    fake.adapter
        .set_price("CAM-PRETA-M", brl("54.904"))
        .unwrap();

    assert_eq!(
        fake.sent("PATCH", &path),
        [serde_json::json!({
            "price": 5490,
            "list_price": 5490,
            "normalizer": 100,
            "currency": "BRL",
            "channel": {"id": CHANNEL},
        })]
    );
    assert!(fake.adapter.applies_price_later());
}

#[test]
fn a_price_magalu_finds_invalid_is_refused_with_its_fields() {
    let fake = Fake::connected();
    fake.server.serve_method(
        "PATCH",
        &sku_path("prices", "CAM-PRETA-M"),
        422,
        fixture("error-422.json"),
    );

    assert_eq!(
        fake.adapter.set_price("CAM-PRETA-M", brl("54.90")),
        Err(PlatformError::Refused(
            "body.price: ensure this value is greater than 0".into()
        ))
    );
}

#[test]
fn a_call_the_login_has_no_scope_for_is_refused_with_magalus_word() {
    let fake = Fake::connected();
    fake.server.serve_method(
        "PATCH",
        &sku_path("stocks", "CAM-PRETA-M"),
        403,
        fixture("error-403.json"),
    );

    assert_eq!(
        fake.adapter.pause("CAM-PRETA-M"),
        Err(PlatformError::Refused("Forbidden".into()))
    );
}

#[test]
fn the_stock_goes_as_the_units_available_and_pausing_sends_zero() {
    let fake = Fake::connected();
    let path = sku_path("stocks", "CAM-PRETA-M");
    fake.server
        .serve_method("PATCH", &path, 202, fixture("accepted.json"));

    fake.adapter
        .set_stock(
            "CAM-PRETA-M",
            &[ChannelStock {
                variation: None,
                available_quantity: 3,
            }],
        )
        .unwrap();
    fake.adapter.pause("CAM-PRETA-M").unwrap();
    fake.adapter.activate("CAM-PRETA-M").unwrap();

    let body = |quantity: u32| serde_json::json!({"quantity": quantity, "type": "AVAILABLE", "channel": {"id": CHANNEL}});
    assert_eq!(fake.sent("PATCH", &path), [body(3), body(0)]);
    assert!(fake.adapter.pauses_with_zero_stock());
    // The store's channel is asked once.
    assert_eq!(
        fake.server
            .requests()
            .iter()
            .filter(|path| path.as_str() == ME)
            .count(),
        1
    );
}

#[test]
fn a_sku_with_a_path_of_its_own_never_reaches_another_path() {
    let fake = Fake::connected();
    fake.server.serve_method(
        "PATCH",
        "/seller/v1/portfolios/prices/A%2F..%2Fx",
        202,
        fixture("accepted.json"),
    );

    fake.adapter.set_price("A/../x", brl("10")).unwrap();
    assert_eq!(
        fake.adapter.set_price(&"X".repeat(33), brl("10")),
        Err(PlatformError::NotFound)
    );
}

#[test]
fn the_fee_is_the_highest_of_magalus_published_commissions() {
    let fake = Fake::connected();
    let fee = |price: &str| {
        let basis = FeeBasis {
            category: String::new(),
            listing_type: ListingType::default(),
            listing: Some("CAM-PRETA-M".into()),
        };
        fake.adapter.sale_fee(&basis, brl(price)).unwrap()
    };
    let table = |fixed: &str| SaleFee {
        rate: Percentage::new(Decimal::from(20)).unwrap(),
        fixed: brl(fixed),
    };

    assert_eq!(fee("10.00"), table("0"));
    assert_eq!(fee("10.01"), table("5"));
    assert_eq!(fee("249.90"), table("5"));
    assert!(fake.server.requests().is_empty());
}

/// The store's three SKUs synced into commerce on a real database.
struct Synced {
    _dir: tempfile::TempDir,
    fake: Fake,
    listings: Listings,
}

impl Synced {
    async fn new() -> Self {
        let fake = Fake::connected();
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        let clock: Arc<dyn Clock> = fake.clock.clone();
        migrate(&database, clock.as_ref(), &[mascate_commerce::MIGRATIONS])
            .await
            .unwrap();
        let listings = Listings::new(database, clock, Arc::new(SequentialIds::default()));
        listings.sync(fake.adapter.as_ref()).await.unwrap();
        Self {
            _dir: dir,
            fake,
            listings,
        }
    }

    async fn camiseta(&self) -> mascate_commerce::Listing {
        self.listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .find(|listing| listing.listed.id == "CAM-PRETA-M")
            .unwrap()
    }

    /// Magalu now answers the camiseta's price as `cents`.
    fn shows_price(&self, cents: u32) {
        self.fake.server.serve_method(
            "GET",
            &sku_path("prices", "CAM-PRETA-M"),
            200,
            format!(
                r#"{{"results": [{{"channel": {{"id": "{CHANNEL}"}}, "price": {cents}, "list_price": {cents}, "normalizer": 100, "currency": "BRL"}}]}}"#
            ),
        );
    }
}

#[test]
fn a_sync_brings_the_skus_into_commerce_and_a_second_changes_nothing() {
    block_on(async {
        let synced = Synced::new().await;

        let listings = synced.listings.listings().await.unwrap();
        assert_eq!(listings.len(), 3);
        assert!(
            listings
                .iter()
                .all(|l| l.channel == SalesChannelKind::Magalu)
        );

        let again = synced
            .listings
            .sync(synced.fake.adapter.as_ref())
            .await
            .unwrap();
        assert_eq!((again.new, again.changed, again.gone), (0, 0, 0));
    });
}

#[test]
fn a_price_magalu_accepts_shows_as_sent_until_the_next_sync_confirms_it() {
    block_on(async {
        let synced = Synced::new().await;
        synced.fake.server.serve_method(
            "PATCH",
            &sku_path("prices", "CAM-PRETA-M"),
            202,
            fixture("accepted.json"),
        );
        let sent_at = synced.fake.clock.now();

        let changed = synced
            .listings
            .change_price(
                synced.camiseta().await.id,
                brl("54.90"),
                synced.fake.adapter.as_ref(),
            )
            .await
            .unwrap();

        assert_eq!(changed[0].listed.price, brl("49.90"));
        assert_eq!(
            changed[0].sent_price,
            Some(SentPrice {
                price: brl("54.90"),
                sent_at,
                state: SentPriceState::Waiting,
            })
        );

        synced.shows_price(5490);
        synced.fake.clock.advance(TimeDelta::minutes(20));
        synced
            .listings
            .sync(synced.fake.adapter.as_ref())
            .await
            .unwrap();

        let confirmed = synced.camiseta().await;
        assert_eq!(confirmed.listed.price, brl("54.90"));
        assert_eq!(confirmed.sent_price, None);
    });
}

#[test]
fn a_price_magalu_never_applied_is_flagged_by_the_sync_after_the_window() {
    block_on(async {
        let synced = Synced::new().await;
        synced.fake.server.serve_method(
            "PATCH",
            &sku_path("prices", "CAM-PRETA-M"),
            202,
            fixture("accepted.json"),
        );
        synced
            .listings
            .change_price(
                synced.camiseta().await.id,
                brl("54.90"),
                synced.fake.adapter.as_ref(),
            )
            .await
            .unwrap();

        synced.fake.clock.advance(TimeDelta::minutes(20));
        synced
            .listings
            .sync(synced.fake.adapter.as_ref())
            .await
            .unwrap();

        let flagged = synced.camiseta().await;
        assert_eq!(flagged.listed.price, brl("49.90"));
        assert_eq!(
            flagged.sent_price.map(|sent| sent.state),
            Some(SentPriceState::NotApplied)
        );
    });
}
