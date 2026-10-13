//! Hotmart's adapter against the fake server serving answers written from
//! Hotmart's documentation (see the fixtures' README), and the Affiliate
//! module's Sync over it with a real temporary database.

use std::ops::Range;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliatePlatform, CommissionFilter, CommissionSource, CommissionStatus, MIGRATIONS,
    Recorded, ReportedConversion,
};
use mascate_integrations::{Hotmart, Pause};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Currency, Money, PlatformError, Timestamp};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;

/// Every status Hotmart documents, in the order the adapter asks them.
const STATUSES: [&str; 17] = [
    "WAITING_PAYMENT",
    "PRINTED_BILLET",
    "STARTED",
    "PROCESSING_TRANSACTION",
    "UNDER_ANALISYS",
    "PRE_ORDER",
    "OVERDUE",
    "BLOCKED",
    "APPROVED",
    "COMPLETE",
    "REFUNDED",
    "PARTIALLY_REFUNDED",
    "CHARGEBACK",
    "PROTESTED",
    "CANCELLED",
    "EXPIRED",
    "NO_FUNDS",
];

const HISTORY: &str = "history";
const COMMISSIONS: &str = "commissions";
const TOKEN_PATH: &str = "/security/oauth/token?grant_type=client_credentials\
                          &client_id=client-id-123&client_secret=client-secret-456";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/hotmart/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn at(month: u32, day: u32, hour: u32, minute: u32) -> Timestamp {
    Utc.with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .unwrap()
}

fn money(amount: &str, currency: Currency) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), currency)
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
    hotmart: Arc<Hotmart>,
}

/// Hotmart at the fake server, with the credentials pasted (the Basic token
/// with its word in front, as Hotmart shows it), on 2026-10-13 12:00 UTC.
fn setup() -> Setup {
    let server = FakeHttpServer::start();
    let store = Arc::new(MemorySecretStore::default());
    for (name, value) in [
        ("HOTMART_CLIENT_ID", "client-id-123"),
        ("HOTMART_CLIENT_SECRET", "client-secret-456"),
        (
            "HOTMART_BASIC",
            "Basic Y2xpZW50LWlkLTEyMzpjbGllbnQtc2VjcmV0LTQ1Ng==",
        ),
    ] {
        store.write(name, &Secret::new(value)).unwrap();
    }
    let clock = Arc::new(ManualClock::at(at(10, 13, 12, 0)));
    let pauses = Arc::new(RecordedPauses::default());
    let hotmart = Arc::new(
        Hotmart::new(
            server.url(),
            server.url(),
            "Mascate/test",
            store.clone(),
            clock.clone(),
        )
        .with_pause(pauses.clone()),
    );
    server.serve(TOKEN_PATH, 200, fixture("token.json"));
    Setup {
        server,
        store,
        clock,
        pauses,
        hotmart,
    }
}

/// September and October up to the clock.
fn window() -> Range<Timestamp> {
    at(9, 1, 0, 0)..at(10, 13, 12, 0)
}

/// The request for one page of `endpoint` in `status` during `during`.
fn path(endpoint: &str, status: &str, during: &Range<Timestamp>, page: Option<&str>) -> String {
    let mut path = format!(
        "/payments/api/v1/sales/{endpoint}?commission_as=AFFILIATE&transaction_status={status}\
         &start_date={}&end_date={}&max_results=100",
        during.start.timestamp_millis(),
        during.end.timestamp_millis()
    );
    if let Some(page) = page {
        path.push_str(&format!("&page_token={page}"));
    }
    path
}

impl Setup {
    /// Answers every status of both endpoints with an empty page.
    fn serve_nothing(&self, during: &Range<Timestamp>) {
        for status in STATUSES {
            for endpoint in [HISTORY, COMMISSIONS] {
                self.server.serve(
                    &path(endpoint, status, during, None),
                    200,
                    fixture("empty.json"),
                );
            }
        }
    }

    fn serve(&self, endpoint: &str, status: &str, during: &Range<Timestamp>, name: &str) {
        self.server
            .serve(&path(endpoint, status, during, None), 200, fixture(name));
    }

    /// The paged sales: two in the first page of approved ones, two in the
    /// next, and the shares of all four with the approved commissions.
    fn serve_pages(&self, during: &Range<Timestamp>) {
        self.serve_nothing(during);
        self.serve(HISTORY, "APPROVED", during, "history-page-1.json");
        self.server.serve(
            &path(
                HISTORY,
                "APPROVED",
                during,
                Some("eyJyb3dzIjoyLCJwYWdlIjoyfQ"),
            ),
            200,
            fixture("history-page-2.json"),
        );
        self.serve(COMMISSIONS, "APPROVED", during, "commissions-pages.json");
    }

    fn token_requests(&self) -> usize {
        self.server
            .received()
            .iter()
            .filter(|request| request.path.starts_with("/security/oauth/token"))
            .count()
    }
}

fn by_key<'a>(conversions: &'a [ReportedConversion], key: &str) -> &'a ReportedConversion {
    conversions
        .iter()
        .find(|conversion| conversion.key == key)
        .unwrap_or_else(|| panic!("no conversion {key}"))
}

#[test]
fn asks_a_token_with_the_client_credentials_then_every_status_with_it() {
    let s = setup();
    s.serve_nothing(&window());

    let conversions = s.hotmart.conversions(window()).unwrap();

    assert!(conversions.is_empty());
    let received = s.server.received();
    let token = &received[0];
    assert_eq!(token.method, "POST");
    assert_eq!(token.path, TOKEN_PATH);
    assert_eq!(
        token.authorization.as_deref(),
        Some("Basic Y2xpZW50LWlkLTEyMzpjbGllbnQtc2VjcmV0LTQ1Ng==")
    );
    // Without a status Hotmart answers only APPROVED and COMPLETE, so every
    // status is asked of both endpoints, as an affiliate, in the window.
    let asked: Vec<&str> = received[1..]
        .iter()
        .map(|request| request.path.as_str())
        .collect();
    let expected: Vec<String> = STATUSES
        .iter()
        .flat_map(|status| {
            [
                path(HISTORY, status, &window(), None),
                path(COMMISSIONS, status, &window(), None),
            ]
        })
        .collect();
    assert_eq!(asked, expected);
    assert!(received[1..].iter().all(|request| request.method == "GET"
        && request.authorization.as_deref() == Some("Bearer H4sIAAAAAAAA-demo-access-token")));
}

#[test]
fn every_hotmart_status_maps_to_a_commission_status_and_keeps_the_raw_one() {
    let s = setup();
    s.serve_nothing(&window());
    s.serve(
        HISTORY,
        "WAITING_PAYMENT",
        &window(),
        "history-every-status.json",
    );
    s.serve(
        COMMISSIONS,
        "WAITING_PAYMENT",
        &window(),
        "commissions-every-status.json",
    );

    let conversions = s.hotmart.conversions(window()).unwrap();

    let statuses: Vec<(&str, CommissionStatus)> = conversions
        .iter()
        .map(|conversion| (conversion.platform_status.as_str(), conversion.status))
        .collect();
    use CommissionStatus::*;
    let mut expected = vec![
        ("WAITING_PAYMENT", Pending),
        ("PRINTED_BILLET", Pending),
        ("STARTED", Pending),
        ("PROCESSING_TRANSACTION", Pending),
        ("UNDER_ANALISYS", Pending),
        ("PRE_ORDER", Pending),
        ("OVERDUE", Pending),
        ("BLOCKED", Pending),
        ("APPROVED", Approved),
        ("COMPLETE", Approved),
        ("REFUNDED", Reversed),
        ("PARTIALLY_REFUNDED", Reversed),
        ("CHARGEBACK", Reversed),
        ("PROTESTED", Reversed),
        ("CANCELLED", Cancelled),
        ("EXPIRED", Cancelled),
        ("NO_FUNDS", Cancelled),
        // A status Hotmart adds later stays pending, its raw name kept.
        ("AWAITING_SOMETHING_NEW", Pending),
    ];
    let mut statuses = statuses;
    statuses.sort();
    expected.sort();
    assert_eq!(statuses, expected);
}

#[test]
fn reads_the_affiliate_share_product_dates_and_marking_of_each_sale() {
    let s = setup();
    s.serve_pages(&window());

    let conversions = s.hotmart.conversions(window()).unwrap();

    // The sale in euros is left out: the app keeps no currency it cannot
    // show.
    assert_eq!(conversions.len(), 3);
    let approved = by_key(&conversions, "HP17715690037001");
    assert_eq!(approved.program.id, "2125812");
    assert_eq!(approved.program.name, "Curso de Fotografia com Celular");
    assert_eq!(approved.product, "Curso de Fotografia com Celular");
    assert_eq!(approved.sold_at, at(9, 3, 12, 0));
    // The owner's share, never the producer's.
    assert_eq!(approved.commission, money("59.1", Currency::Brl));
    assert_eq!(approved.status, CommissionStatus::Approved);
    assert_eq!(approved.status_changed_at, Some(at(9, 3, 12, 10)));
    // The sck wins over the src when the link carried both.
    assert_eq!(approved.tracking.as_deref(), Some("instagram_bio"));
    let raw: serde_json::Value = serde_json::from_str(&approved.raw).unwrap();
    assert_eq!(raw["sale"]["purchase"]["status"], "APPROVED");
    assert_eq!(raw["commission"]["transaction"], "HP17715690037001");
    // The buyer is never kept.
    assert!(raw["sale"].get("buyer").is_none());

    // Commissions in dollars stay in dollars, without conversion.
    let complete = by_key(&conversions, "HP17715690037002");
    assert_eq!(complete.commission, money("14.7", Currency::Usd));
    // Complete is approved past the guarantee, which ended then.
    assert_eq!(complete.status, CommissionStatus::Approved);
    assert_eq!(complete.status_changed_at, Some(at(9, 11, 12, 0)));
    assert_eq!(complete.tracking, None);

    let waiting = by_key(&conversions, "HP17715690037003");
    assert_eq!(waiting.status, CommissionStatus::Pending);
    assert_eq!(waiting.status_changed_at, None);
    assert_eq!(waiting.tracking.as_deref(), Some("youtube"));
}

#[test]
fn follows_the_page_token_to_the_last_page() {
    let s = setup();
    s.serve_pages(&window());

    s.hotmart.conversions(window()).unwrap();

    let pages: Vec<String> = s
        .server
        .requests()
        .into_iter()
        .filter(|path| path.contains("history?") && path.contains("=APPROVED&"))
        .collect();
    assert_eq!(
        pages,
        [
            path(HISTORY, "APPROVED", &window(), None),
            path(
                HISTORY,
                "APPROVED",
                &window(),
                Some("eyJyb3dzIjoyLCJwYWdlIjoyfQ")
            ),
        ]
    );
}

#[test]
fn a_rate_limit_waits_longer_each_time_then_goes_on() {
    let s = setup();
    s.serve_nothing(&window());
    let first = path(HISTORY, "WAITING_PAYMENT", &window(), None);
    s.server.serve(&first, 429, fixture("error-429.json"));
    s.server.then_serve(&first, 429, fixture("error-429.json"));
    s.server.then_serve(&first, 200, fixture("empty.json"));

    s.hotmart.conversions(window()).unwrap();

    assert_eq!(s.pauses.seconds(), [5, 10]);
}

#[test]
fn a_rate_limit_that_never_lifts_gives_up_after_four_retries() {
    let s = setup();
    s.serve_nothing(&window());
    s.server.serve(
        &path(HISTORY, "WAITING_PAYMENT", &window(), None),
        429,
        fixture("error-429.json"),
    );

    let failed = s.hotmart.conversions(window()).unwrap_err();

    assert_eq!(failed, PlatformError::RateLimited);
    assert_eq!(s.pauses.seconds(), [5, 10, 20, 40]);
}

#[test]
fn a_server_error_is_tried_again() {
    let s = setup();
    s.serve_nothing(&window());
    let first = path(HISTORY, "WAITING_PAYMENT", &window(), None);
    s.server.serve(&first, 503, "");
    s.server.then_serve(&first, 200, fixture("empty.json"));

    s.hotmart.conversions(window()).unwrap();

    assert_eq!(s.pauses.seconds(), [5]);
}

#[test]
fn a_token_turned_down_is_renewed_once_before_the_credentials_count_as_expired() {
    let s = setup();
    s.serve_nothing(&window());
    s.server.serve(
        &path(HISTORY, "WAITING_PAYMENT", &window(), None),
        401,
        fixture("error-401.json"),
    );

    let failed = s.hotmart.conversions(window()).unwrap_err();

    assert_eq!(failed, PlatformError::Expired);
    assert_eq!(s.token_requests(), 2);
}

#[test]
fn a_token_turned_down_once_works_after_renewal() {
    let s = setup();
    s.serve_nothing(&window());
    let first = path(HISTORY, "WAITING_PAYMENT", &window(), None);
    s.server.serve(&first, 401, fixture("error-401.json"));
    s.server.then_serve(&first, 200, fixture("empty.json"));

    s.hotmart.conversions(window()).unwrap();

    assert_eq!(s.token_requests(), 2);
}

#[test]
fn the_token_is_reused_until_shortly_before_it_expires() {
    let s = setup();
    s.serve_nothing(&window());

    s.hotmart.conversions(window()).unwrap();
    // Hotmart's token lasts 172799 seconds, about two days.
    s.clock.advance(TimeDelta::hours(47));
    s.hotmart.conversions(window()).unwrap();
    assert_eq!(s.token_requests(), 1);

    s.clock.advance(TimeDelta::minutes(57));
    s.hotmart.conversions(window()).unwrap();
    assert_eq!(s.token_requests(), 2);
}

#[test]
fn two_calls_racing_for_a_token_ask_for_it_once() {
    let s = setup();
    s.serve_nothing(&window());

    let racing: Vec<_> = (0..2)
        .map(|_| {
            let hotmart = s.hotmart.clone();
            std::thread::spawn(move || hotmart.conversions(window()))
        })
        .collect();
    for call in racing {
        call.join().unwrap().unwrap();
    }

    assert_eq!(s.token_requests(), 1);
}

#[test]
fn credentials_hotmart_does_not_accept_count_as_expired() {
    let s = setup();
    s.server
        .serve(TOKEN_PATH, 401, fixture("token-refused.json"));

    let failed = s.hotmart.conversions(window()).unwrap_err();

    assert_eq!(failed, PlatformError::Expired);
    assert_eq!(s.server.requests().len(), 1);
}

#[test]
fn without_credentials_nothing_is_asked() {
    let s = setup();
    s.store.remove("HOTMART_BASIC").unwrap();

    let failed = s.hotmart.conversions(window()).unwrap_err();

    assert_eq!(failed, PlatformError::NotConnected);
    assert!(s.server.requests().is_empty());
}

#[test]
fn hotmart_declares_its_platform_reach_and_settle_time() {
    let s = setup();

    assert_eq!(s.hotmart.platform(), AffiliatePlatform::Hotmart);
    // The longest guarantee, 30 days, and the days a boleto takes.
    assert_eq!(s.hotmart.settle_time(), TimeDelta::days(35));
    assert!(s.hotmart.history() >= TimeDelta::days(365));
}

/// The Affiliate module over a real temporary database, synced from Hotmart.
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
fn syncing_the_same_sales_again_changes_nothing() {
    let s = setup();
    let synced = affiliate(&s);
    // The first Sync asks everything Hotmart lets the app read.
    let first = s.clock.now() - s.hotmart.history()..s.clock.now();
    s.serve_pages(&first);

    let sync = block_on(synced.affiliate.sync(s.hotmart.as_ref())).unwrap();

    assert_eq!(sync.window, first);
    assert_eq!(
        sync.recorded,
        Recorded {
            added: 3,
            changed: 0,
            unchanged: 0
        }
    );
    // The next one rereads what can still change, the same sales again.
    let settling = s.clock.now() - TimeDelta::days(35)..s.clock.now();
    s.serve_pages(&settling);
    let again = block_on(synced.affiliate.sync(s.hotmart.as_ref())).unwrap();
    assert_eq!(again.window, settling);
    assert_eq!(
        again.recorded,
        Recorded {
            added: 0,
            changed: 0,
            unchanged: 3
        }
    );
    let commissions = block_on(synced.affiliate.commissions(&CommissionFilter::default())).unwrap();
    assert_eq!(commissions.len(), 3);
    let dollars: Vec<Money> = commissions
        .iter()
        .filter(|commission| commission.amount.currency() == Currency::Usd)
        .map(|commission| commission.amount)
        .collect();
    assert_eq!(dollars, [money("14.7", Currency::Usd)]);
}

#[test]
fn a_refund_on_hotmart_reverses_the_same_commission() {
    let s = setup();
    let synced = affiliate(&s);
    let first = s.clock.now() - s.hotmart.history()..s.clock.now();
    s.serve_pages(&first);
    block_on(synced.affiliate.sync(s.hotmart.as_ref())).unwrap();

    // An hour later the buyer asked for the money back.
    s.clock.advance(TimeDelta::hours(1));
    let next = s.clock.now() - TimeDelta::days(35) - TimeDelta::hours(1)..s.clock.now();
    s.serve_nothing(&next);
    s.serve(HISTORY, "REFUNDED", &next, "history-refunded.json");
    s.serve(COMMISSIONS, "REFUNDED", &next, "commissions-refunded.json");
    let sync = block_on(synced.affiliate.sync(s.hotmart.as_ref())).unwrap();

    assert_eq!(sync.window, next);
    assert_eq!(
        sync.recorded,
        Recorded {
            added: 0,
            changed: 1,
            unchanged: 0
        }
    );
    let commissions = block_on(synced.affiliate.commissions(&CommissionFilter::default())).unwrap();
    assert_eq!(commissions.len(), 3);
    let refunded = commissions
        .iter()
        .find(|commission| commission.status == CommissionStatus::Reversed)
        .unwrap();
    assert_eq!(refunded.platform_status, "REFUNDED");
    assert_eq!(refunded.amount, money("59.1", Currency::Brl));
    let history = block_on(synced.affiliate.status_history(refunded.id)).unwrap();
    let statuses: Vec<CommissionStatus> = history.iter().map(|change| change.status).collect();
    assert_eq!(
        statuses,
        [CommissionStatus::Approved, CommissionStatus::Reversed]
    );
}
