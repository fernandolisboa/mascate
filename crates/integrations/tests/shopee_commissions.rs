//! Shopee Afiliados as a commission source (#80) against the fake server
//! serving answers written from Shopee's documentation (see the fixtures'
//! README): one Conversion per item, the status of every order and fraud
//! combination, paging by `scrollId`, the validated Payout, and the
//! Affiliate module recording both over a real temporary database.

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliatePlatform, CommissionFilter, CommissionSource, CommissionStatus, MIGRATIONS,
    Recorded, ReportedConversion,
};
use mascate_integrations::{Pause, ShopeeAffiliates};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Currency, Money, PlatformError, Timestamp};
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

fn at(seconds: i64) -> Timestamp {
    DateTime::from_timestamp(seconds, 0).unwrap()
}

/// Remembers each pause and moves the clock past it, as a real one would.
struct Pauses {
    clock: Arc<ManualClock>,
    taken: Mutex<Vec<Duration>>,
}

impl Pause for Pauses {
    fn pause(&self, duration: Duration) {
        self.taken.lock().unwrap().push(duration);
        self.clock.advance(TimeDelta::from_std(duration).unwrap());
    }
}

impl Pauses {
    fn seconds(&self) -> Vec<u64> {
        self.taken
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
    pauses: Arc<Pauses>,
    shopee: ShopeeAffiliates,
}

/// Shopee at the fake server with the AppID and Secret, on 2026-10-13
/// 12:00 UTC.
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
        Utc.with_ymd_and_hms(2026, 10, 13, 12, 0, 0).unwrap(),
    ));
    let pauses = Arc::new(Pauses {
        clock: clock.clone(),
        taken: Mutex::new(Vec::new()),
    });
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

impl Setup {
    fn serve_both_pages(&self) {
        self.server
            .serve("/graphql", 200, fixture("conversion-report.json"));
        self.server
            .then_serve("/graphql", 200, fixture("conversion-report-end.json"));
    }

    fn last_month(&self) -> std::ops::Range<Timestamp> {
        self.clock.now() - TimeDelta::days(30)..self.clock.now()
    }

    fn conversions(&self) -> Vec<ReportedConversion> {
        self.shopee.conversions(self.last_month()).unwrap()
    }

    /// The GraphQL query each request carried.
    fn queries(&self) -> Vec<String> {
        self.server
            .received()
            .iter()
            .map(|request| {
                let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
                body["query"].as_str().unwrap().to_owned()
            })
            .collect()
    }
}

fn find<'a>(conversions: &'a [ReportedConversion], key: &str) -> &'a ReportedConversion {
    conversions
        .iter()
        .find(|conversion| conversion.key == key)
        .unwrap_or_else(|| panic!("no conversion {key}"))
}

const FONE: &str = "81726354001:260902ABCD1234:22334455667:180001";
const CAPA: &str = "81726354001:260902ABCD1234:22334455668:0";
const CARREGADOR: &str = "81726354002:260928EFGH5678:33445566778:190002";
const CABO: &str = "81726354002:260928IJKL9012:33445566779:0";
const RELOGIO: &str = "81726354003:260905MNOP3456:44556677889:200003";
const MOUSE: &str = "81726354004:260908QRST7890:55667788990:210004";
const RECARGA: &str = "81726354005:260910UVWX1234:66778899001:0";

#[test]
fn each_item_sold_is_one_conversion_with_its_commission_marking_and_dates() {
    let s = setup();
    s.serve_both_pages();

    let conversions = s.conversions();

    // The item without a commission is left out.
    assert_eq!(conversions.len(), 7);
    let fone = find(&conversions, FONE);
    assert_eq!(fone.program.id, "shopee-affiliates");
    assert_eq!(fone.program.name, "Shopee Afiliados");
    assert_eq!(fone.product, "Fone de Ouvido Bluetooth TWS");
    assert_eq!(fone.commission, brl("4.50"));
    assert_eq!(fone.sold_at, at(1788350400));
    assert_eq!(fone.status_changed_at, Some(at(1789905600)));
    assert_eq!(fone.platform_status, "COMPLETED/VERIFIED");
    // The first subId, the Promotion Channel's marking: `utmContent` joins
    // the five with dashes.
    assert_eq!(fone.tracking.as_deref(), Some("instagram"));
    let raw: serde_json::Value = serde_json::from_str(&fone.raw).unwrap();
    assert_eq!(raw["conversion"]["conversionId"], 81726354001u64);
    assert!(raw["conversion"].get("orders").is_none());
    assert_eq!(raw["order"]["orderId"], "260902ABCD1234");
    assert!(raw["order"].get("items").is_none());
    assert_eq!(raw["item"]["itemId"], 22334455667u64);

    assert_eq!(
        find(&conversions, RELOGIO).tracking.as_deref(),
        Some("whatsapp")
    );
    let carregador = find(&conversions, CARREGADOR);
    assert_eq!(carregador.tracking, None);
    assert_eq!(carregador.status_changed_at, None);
    // Ids sent as text make the same key as numbers would.
    assert_eq!(find(&conversions, MOUSE).commission, brl("3.00"));
    // Shopee dates the completion, not a later fraud verdict.
    assert_eq!(find(&conversions, MOUSE).status_changed_at, None);
}

#[test]
fn every_order_status_and_fraud_combination_maps_to_a_commission_status() {
    let s = setup();
    s.serve_both_pages();

    let conversions = s.conversions();

    let status = |key| find(&conversions, key).status;
    assert_eq!(status(FONE), CommissionStatus::Approved);
    assert_eq!(status(CAPA), CommissionStatus::Approved);
    assert_eq!(status(CARREGADOR), CommissionStatus::Pending);
    assert_eq!(status(CABO), CommissionStatus::Pending);
    assert_eq!(status(RELOGIO), CommissionStatus::Cancelled);
    // Fraud reverses a completed order; so does a refund on one.
    assert_eq!(status(MOUSE), CommissionStatus::Reversed);
    assert_eq!(find(&conversions, MOUSE).platform_status, "COMPLETED/FRAUD");
    assert_eq!(status(RECARGA), CommissionStatus::Reversed);
}

#[test]
fn the_status_mapping_covers_each_order_status_with_each_fraud_status() {
    let orders = [
        "UNPAID",
        "PENDING",
        "COMPLETED",
        "CANCELLED",
        "SOMETHING_NEW",
    ];
    let frauds = ["", "UNVERIFIED", "VERIFIED", "FRAUD"];
    for order in orders {
        for fraud in frauds {
            for refunded in [false, true] {
                let s = setup();
                s.server
                    .serve("/graphql", 200, single_item(order, fraud, refunded));
                let conversions = s.conversions();
                let expected = match (order, fraud, refunded) {
                    (_, "FRAUD", _) => CommissionStatus::Reversed,
                    ("COMPLETED", _, true) => CommissionStatus::Reversed,
                    ("COMPLETED", _, false) => CommissionStatus::Approved,
                    ("CANCELLED", ..) => CommissionStatus::Cancelled,
                    _ => CommissionStatus::Pending,
                };
                assert_eq!(
                    conversions[0].status, expected,
                    "{order} {fraud} refunded: {refunded}"
                );
            }
        }
    }
}

/// A report of one item in an order with `order` status.
fn single_item(order: &str, fraud: &str, refunded: bool) -> String {
    serde_json::json!({
        "data": {"conversionReport": {
            "nodes": [{
                "purchaseTime": 1788350400,
                "conversionId": 1,
                "utmContent": "",
                "orders": [{
                    "orderId": "A1",
                    "orderStatus": order,
                    "items": [{
                        "itemId": 2,
                        "modelId": 3,
                        "itemName": "Item",
                        "completeTime": 1789905600,
                        "itemTotalCommission": "1.00",
                        "refundAmount": if refunded { "2.00" } else { "0" },
                        "fraudStatus": fraud,
                    }],
                }],
            }],
            "pageInfo": {"hasNextPage": false, "scrollId": ""},
        }}
    })
    .to_string()
}

#[test]
fn pages_follow_the_scroll_id_without_pauses() {
    let s = setup();
    s.serve_both_pages();
    let window = s.last_month();

    s.shopee.conversions(window.clone()).unwrap();

    let queries = s.queries();
    assert_eq!(queries.len(), 2);
    let range = format!(
        "purchaseTimeStart: {}, purchaseTimeEnd: {}, limit: 500",
        window.start.timestamp(),
        window.end.timestamp()
    );
    assert!(queries[0].starts_with(&format!("{{conversionReport({range}){{")));
    assert!(queries[1].starts_with(&format!(
        "{{conversionReport({range}, scrollId: \"c2Nyb2xsLTE6YWJjZGVm\"){{"
    )));
    assert!(queries[0].contains("pageInfo{hasNextPage scrollId}"));
    assert!(s.pauses.seconds().is_empty());
}

#[test]
fn an_expired_scroll_id_starts_the_reading_over_from_the_first_page() {
    let s = setup();
    s.server
        .serve("/graphql", 200, fixture("conversion-report.json"));
    s.server
        .then_serve("/graphql", 200, fixture("error-11001-scroll.json"));
    s.serve_then_both_pages();

    let conversions = s.conversions();

    // Nothing from the first attempt counts twice.
    assert_eq!(conversions.len(), 7);
    let queries = s.queries();
    assert_eq!(queries.len(), 4);
    assert!(!queries[2].contains("scrollId:"));
    assert!(queries[3].contains("scrollId:"));
    // Shopee asks 30 seconds between two queries without a scrollId.
    assert_eq!(s.pauses.seconds(), [30]);
}

impl Setup {
    fn serve_then_both_pages(&self) {
        self.server
            .then_serve("/graphql", 200, fixture("conversion-report.json"));
        self.server
            .then_serve("/graphql", 200, fixture("conversion-report-end.json"));
    }
}

#[test]
fn a_scroll_id_that_keeps_expiring_fails_the_reading() {
    let s = setup();
    s.server
        .serve("/graphql", 200, fixture("conversion-report.json"));
    for _ in 0..2 {
        s.server
            .then_serve("/graphql", 200, fixture("error-11001-scroll.json"));
        s.server
            .then_serve("/graphql", 200, fixture("conversion-report.json"));
    }
    s.server
        .then_serve("/graphql", 200, fixture("error-11001-scroll.json"));

    let error = s.shopee.conversions(s.last_month()).unwrap_err();

    assert!(matches!(error, PlatformError::Refused(_)), "{error:?}");
    assert_eq!(s.server.received().len(), 6);
}

#[test]
fn a_report_asked_again_within_30_seconds_waits_out_the_rest() {
    let s = setup();
    s.server
        .serve("/graphql", 200, fixture("conversion-report-end.json"));

    s.conversions();
    s.clock.advance(TimeDelta::seconds(12));
    s.conversions();
    s.clock.advance(TimeDelta::seconds(31));
    s.conversions();

    assert_eq!(s.pauses.seconds(), [18]);
}

#[test]
fn a_rate_limit_waits_with_the_backoff_and_the_30_seconds_between_first_pages() {
    let s = setup();
    s.server.serve("/graphql", 200, fixture("error-10030.json"));
    s.server
        .then_serve("/graphql", 200, fixture("conversion-report-end.json"));

    let conversions = s.conversions();

    assert_eq!(conversions.len(), 3);
    // The backoff's 2 seconds, then the rest of the 30 since the first try.
    assert_eq!(s.pauses.seconds(), [2, 28]);
    let requests = s.server.received();
    assert_eq!(requests.len(), 2);
    // Signed again with the time it left.
    assert_ne!(requests[0].authorization, requests[1].authorization);
}

#[test]
fn a_refused_signature_means_shopee_no_longer_accepts_the_keys() {
    let s = setup();
    s.server.serve("/graphql", 200, fixture("error-10020.json"));

    let error = s.shopee.conversions(s.last_month()).unwrap_err();

    assert_eq!(error, PlatformError::Expired);
    assert_eq!(s.server.received().len(), 1);
}

#[test]
fn a_window_longer_than_shopee_lets_read_is_refused_before_asking() {
    let s = setup();
    let now = s.clock.now();

    let error = s
        .shopee
        .conversions(now - TimeDelta::days(90)..now)
        .unwrap_err();

    assert!(matches!(error, PlatformError::Refused(_)), "{error:?}");
    assert!(s.server.received().is_empty());
    // Three months back is what Shopee lets the app read, and a status no
    // longer changes past the monthly validation.
    assert_eq!(s.shopee.history(), TimeDelta::days(89));
    assert!(s.shopee.settle_time() < s.shopee.history());
    assert_eq!(s.shopee.platform(), AffiliatePlatform::Shopee);
}

#[test]
fn without_both_keys_shopee_is_not_connected_and_asks_nothing() {
    let s = setup();
    s.store.remove("SHOPEE_AFFILIATE_SECRET").unwrap();

    assert_eq!(
        s.shopee.conversions(s.last_month()).unwrap_err(),
        PlatformError::NotConnected
    );
    assert_eq!(
        s.shopee
            .validated_payout("4455667788", s.clock.now())
            .unwrap_err(),
        PlatformError::NotConnected
    );
    assert!(s.server.received().is_empty());
}

#[test]
fn a_validation_id_is_the_payout_of_the_net_commission_covering_each_item() {
    let s = setup();
    s.server
        .serve("/graphql", 200, fixture("validated-report.json"));
    let paid_at = s.clock.now() - TimeDelta::days(3);

    let payout = s.shopee.validated_payout(" 4455667788 ", paid_at).unwrap();

    assert_eq!(payout.key, "4455667788");
    assert_eq!(payout.paid_at, paid_at);
    // The net of each conversion: 5.40 after an MCN fee, plus 2.00.
    assert_eq!(payout.amount, brl("7.40"));
    assert_eq!(
        payout.covers,
        [FONE, CAPA, "81726350999:260715CDEF9999:11223344556:0"]
    );
    let query = &s.queries()[0];
    assert!(query.starts_with("{validatedReport(validationId: 4455667788, limit: 500){"));
}

#[test]
fn a_validation_id_that_is_not_a_number_is_refused_before_asking() {
    let s = setup();

    let error = s
        .shopee
        .validated_payout("abc\"){x}", s.clock.now())
        .unwrap_err();

    assert!(matches!(error, PlatformError::Refused(_)), "{error:?}");
    assert!(s.server.received().is_empty());
}

#[test]
fn a_validation_id_shopee_does_not_know_is_not_found() {
    let s = setup();
    s.server
        .serve("/graphql", 200, fixture("validated-report-empty.json"));

    assert_eq!(
        s.shopee
            .validated_payout("4455667788", s.clock.now())
            .unwrap_err(),
        PlatformError::NotFound
    );
}

/// The Affiliate module over a real temporary database.
struct Synced {
    _dir: tempfile::TempDir,
    affiliate: Affiliate,
}

fn affiliate(s: &Setup) -> Synced {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, s.clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        Synced {
            _dir: dir,
            affiliate: Affiliate::new(
                database,
                s.clock.clone(),
                Arc::new(SequentialIds::default()),
            ),
        }
    })
}

#[test]
fn syncing_the_same_window_again_changes_nothing() {
    let s = setup();
    let synced = affiliate(&s);
    s.serve_both_pages();
    s.serve_then_both_pages();

    let first = block_on(synced.affiliate.sync(&s.shopee)).unwrap();
    s.clock.advance(TimeDelta::hours(1));
    let again = block_on(synced.affiliate.sync(&s.shopee)).unwrap();

    // The first Sync reads the three months Shopee keeps.
    assert_eq!(first.window.end - first.window.start, TimeDelta::days(89));
    assert_eq!(
        first.recorded,
        Recorded {
            added: 7,
            changed: 0,
            unchanged: 0
        }
    );
    // The next one, since the last minus the time a status still changes.
    assert_eq!(
        again.window.start,
        first.window.end - s.shopee.settle_time()
    );
    assert_eq!(
        again.recorded,
        Recorded {
            added: 0,
            changed: 0,
            unchanged: 7
        }
    );
    let commissions = block_on(synced.affiliate.commissions(&CommissionFilter::default())).unwrap();
    assert_eq!(commissions.len(), 7);
    assert!(commissions.iter().all(|commission| {
        commission.platform == AffiliatePlatform::Shopee
            && commission.program_name == "Shopee Afiliados"
    }));
}

#[test]
fn the_same_validation_id_twice_is_one_payout_that_pays_what_it_covers() {
    let s = setup();
    let synced = affiliate(&s);
    s.serve_both_pages();
    block_on(synced.affiliate.sync(&s.shopee)).unwrap();
    s.server
        .serve("/graphql", 200, fixture("validated-report.json"));
    let paid_at = s.clock.now();

    for _ in 0..2 {
        let payout = s.shopee.validated_payout("4455667788", paid_at).unwrap();
        block_on(
            synced
                .affiliate
                .record_platform_payouts(AffiliatePlatform::Shopee, &[payout]),
        )
        .unwrap();
    }

    let payouts = block_on(
        synced
            .affiliate
            .payouts(Timestamp::MIN_UTC..Timestamp::MAX_UTC),
    )
    .unwrap();
    assert_eq!(payouts.len(), 1);
    let payout = &payouts[0];
    assert_eq!(payout.amount, brl("7.40"));
    // The conversion from before the app read Shopee stays out.
    assert_eq!(payout.covers.len(), 2);
    assert!(
        payout
            .covers
            .iter()
            .all(|commission| commission.status == CommissionStatus::Paid)
    );
    assert_eq!(payout.difference(), brl("1.70"));
}
