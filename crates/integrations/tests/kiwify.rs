//! Kiwify's adapter against the fake server serving answers written from
//! the public API documentation (see the fixtures' README), and the
//! Affiliate module's Sync over it with a real temporary database.

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
use mascate_integrations::{Kiwify, Pause};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Currency, Money, PlatformError, Timestamp};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore, Request};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;

const TOKEN: &str = "/v1/oauth/token";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/kiwify/{name}",
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
    kiwify: Arc<Kiwify>,
}

/// Kiwify at the fake server, with the keys pasted, on 2026-10-13 12:00
/// UTC.
fn setup() -> Setup {
    let server = FakeHttpServer::start();
    let store = Arc::new(MemorySecretStore::default());
    for (name, value) in [
        ("KIWIFY_CLIENT_ID", "be161f42-1d05-4949-8736-1a526c28672d"),
        ("KIWIFY_CLIENT_SECRET", " client-secret-456 "),
        ("KIWIFY_ACCOUNT_ID", "XvS0qfkdzCZTg8z"),
    ] {
        store.write(name, &Secret::new(value)).unwrap();
    }
    let clock = Arc::new(ManualClock::at(at(10, 13, 12, 0)));
    let pauses = Arc::new(RecordedPauses::default());
    let kiwify = Arc::new(
        Kiwify::new(server.url(), "Mascate/test", store.clone(), clock.clone())
            .with_pause(pauses.clone()),
    );
    server.serve(TOKEN, 200, fixture("token.json"));
    Setup {
        server,
        store,
        clock,
        pauses,
        kiwify,
    }
}

/// The last day and a bit, as a Sync after the previous one asks it.
fn window() -> Range<Timestamp> {
    at(10, 12, 11, 0)..at(10, 13, 12, 0)
}

/// What the adapter asks in one query at most.
const LONGEST: TimeDelta = TimeDelta::days(89);

/// How far before the changes the adapter looks for the sales.
const SALE_CHANGES_FOR: TimeDelta = TimeDelta::days(180);

/// `during` cut as the adapter cuts it.
fn windows(during: &Range<Timestamp>) -> Vec<Range<Timestamp>> {
    let mut windows = Vec::new();
    let mut start = during.start;
    while start < during.end {
        let end = (start + LONGEST).min(during.end);
        windows.push(start..end);
        start = end;
    }
    windows
}

/// A moment as the query carries it, encoded.
fn encoded(at: Timestamp) -> String {
    at.format("%Y-%m-%d%%20%H%%3A%M%%3A%S%.3f").to_string()
}

/// The request for one page of the sales made during `made` that changed
/// during `changed`.
fn sales_path(made: &Range<Timestamp>, changed: &Range<Timestamp>, page: u32) -> String {
    format!(
        "/v1/sales?start_date={}&end_date={}&updated_at_start_date={}&updated_at_end_date={}\
         &view_full_sale_details=true&page_size=100&page_number={page}",
        encoded(made.start),
        encoded(made.end),
        encoded(changed.start),
        encoded(changed.end)
    )
}

fn payouts_path(page: u32) -> String {
    format!("/v1/payouts?page_size=100&page_number={page}")
}

/// Every query a Sync over `during` makes for page one, in order.
fn sales_paths(during: &Range<Timestamp>) -> Vec<String> {
    windows(during)
        .iter()
        .flat_map(|changed| {
            windows(&(changed.start - SALE_CHANGES_FOR..changed.end))
                .into_iter()
                .map(|made| sales_path(&made, changed, 1))
                .collect::<Vec<_>>()
        })
        .collect()
}

impl Setup {
    /// Answers every query over `during` with an empty page.
    fn serve_nothing(&self, during: &Range<Timestamp>) {
        for path in sales_paths(during) {
            self.server.serve(&path, 200, fixture("empty.json"));
        }
    }

    /// Answers the query for the newest sales of `during` with `name`, and
    /// every other with an empty page.
    fn serve(&self, during: &Range<Timestamp>, name: &str) {
        self.serve_nothing(during);
        let newest = sales_paths(during).pop().unwrap();
        self.server.serve(&newest, 200, fixture(name));
    }

    fn token_requests(&self) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|path| path.as_str() == TOKEN)
            .count()
    }

    fn sales_requests(&self) -> Vec<Request> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.path.starts_with("/v1/sales"))
            .collect()
    }
}

fn by_key<'a>(conversions: &'a [ReportedConversion], key: &str) -> &'a ReportedConversion {
    conversions
        .iter()
        .find(|conversion| conversion.key == key)
        .unwrap_or_else(|| panic!("no conversion {key}"))
}

/// The id the fixtures give sale `n`.
fn sale(n: u64) -> String {
    format!("5cbb3832-7b5f-4cf2-b04e-{n:012}")
}

fn header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// The two dates of a query parameter pair, decoded.
fn dates(path: &str, start: &str, end: &str) -> Range<Timestamp> {
    let value = |name: &str| {
        let text = path
            .split(['?', '&'])
            .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
            .unwrap()
            .replace("%20", " ")
            .replace("%3A", ":");
        chrono::NaiveDateTime::parse_from_str(&text, "%Y-%m-%d %H:%M:%S%.3f")
            .unwrap()
            .and_utc()
    };
    value(start)..value(end)
}

#[test]
fn asks_a_token_with_the_client_credentials_then_the_sales_with_it_and_the_account() {
    let s = setup();
    s.serve_nothing(&window());

    let conversions = s.kiwify.conversions(window()).unwrap();

    assert!(conversions.is_empty());
    let received = s.server.received();
    let token = &received[0];
    assert_eq!(
        (token.method.as_str(), token.path.as_str()),
        ("POST", TOKEN)
    );
    assert_eq!(
        header(token, "content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        token.body,
        "client_id=be161f42-1d05-4949-8736-1a526c28672d&client_secret=client-secret-456"
    );
    let asked: Vec<&str> = received[1..]
        .iter()
        .map(|request| request.path.as_str())
        .collect();
    assert_eq!(asked, sales_paths(&window()));
    assert!(received[1..].iter().all(|request| {
        request.method == "GET"
            && request.authorization.as_deref() == Some("Bearer kiwify-access-token-1")
            && header(request, "x-kiwify-account-id") == Some("XvS0qfkdzCZTg8z")
    }));
}

#[test]
fn asks_what_changed_in_windows_kiwify_accepts_going_back_as_long_as_a_sale_changes() {
    let s = setup();
    // A first Sync over five years, cut in windows of 89 days.
    let during = s.clock.now() - s.kiwify.history()..s.clock.now();
    s.serve_nothing(&during);

    s.kiwify.conversions(during.clone()).unwrap();

    let requests = s.sales_requests();
    assert_eq!(requests.len(), sales_paths(&during).len());
    let mut changed_windows: Vec<Range<Timestamp>> = Vec::new();
    for request in &requests {
        let changed = dates(
            &request.path,
            "updated_at_start_date",
            "updated_at_end_date",
        );
        let made = dates(&request.path, "start_date", "end_date");
        assert!(changed.end - changed.start <= TimeDelta::days(90));
        assert!(made.end - made.start <= TimeDelta::days(90));
        // No sale changes before it is made, nor long after.
        assert!(made.start < changed.end && made.end <= changed.end);
        assert!(made.end > changed.start - SALE_CHANGES_FOR);
        if changed_windows.last() != Some(&changed) {
            changed_windows.push(changed);
        }
    }
    // The changes asked cover the window once, with no gap.
    assert_eq!(changed_windows.first().unwrap().start, during.start);
    assert_eq!(changed_windows.last().unwrap().end, during.end);
    assert!(
        changed_windows
            .windows(2)
            .all(|pair| pair[0].end == pair[1].start)
    );
    // Each window of changes asks the sales made from 180 days before it
    // up to its end, with no gap.
    for changed in &changed_windows {
        let made: Vec<Range<Timestamp>> = requests
            .iter()
            .filter(|request| {
                &dates(
                    &request.path,
                    "updated_at_start_date",
                    "updated_at_end_date",
                ) == changed
            })
            .map(|request| dates(&request.path, "start_date", "end_date"))
            .collect();
        assert_eq!(
            made.first().unwrap().start,
            changed.start - SALE_CHANGES_FOR
        );
        assert_eq!(made.last().unwrap().end, changed.end);
        assert!(made.windows(2).all(|pair| pair[0].end == pair[1].start));
    }
    // An hourly Sync asks three queries: the day of changes, over the sales
    // of the last 180 days.
    assert_eq!(sales_paths(&window()).len(), 3);
    assert!(sales_paths(&window())[0].contains("start_date=2026-04-15%2011%3A00%3A00.000"));
}

#[test]
fn every_status_maps_to_a_commission_status() {
    let s = setup();
    s.serve(&window(), "sales-every-status.json");

    let conversions = s.kiwify.conversions(window()).unwrap();

    let expected = [
        ("waiting_payment", CommissionStatus::Pending),
        ("pending", CommissionStatus::Pending),
        ("processing", CommissionStatus::Pending),
        ("authorized", CommissionStatus::Pending),
        ("refund_requested", CommissionStatus::Pending),
        ("pending_refund", CommissionStatus::Pending),
        ("approved", CommissionStatus::Approved),
        ("paid", CommissionStatus::Approved),
        ("refunded", CommissionStatus::Reversed),
        ("chargedback", CommissionStatus::Reversed),
        ("refused", CommissionStatus::Cancelled),
    ];
    assert_eq!(conversions.len(), expected.len());
    for (n, (platform_status, status)) in expected.into_iter().enumerate() {
        let conversion = by_key(&conversions, &sale(n as u64 + 1));
        assert_eq!(conversion.platform_status, platform_status);
        assert_eq!(conversion.status, status, "{platform_status}");
        let changed_at = match status {
            CommissionStatus::Approved => Some(Utc.with_ymd_and_hms(2026, 9, 10, 14, 25, 0)),
            CommissionStatus::Reversed => Some(Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0)),
            _ => None,
        }
        .map(|at| at.unwrap());
        assert_eq!(
            conversion.status_changed_at, changed_at,
            "{platform_status}"
        );
    }
}

#[test]
fn amounts_in_cents_become_exact_decimals_in_the_sales_currency() {
    let s = setup();
    s.serve(&window(), "sales-shares.json");

    let conversions = s.kiwify.conversions(window()).unwrap();

    let first = by_key(&conversions, &sale(101));
    assert_eq!(first.commission, money("49.90", Currency::Brl));
    assert_eq!(first.commission.amount().to_string(), "49.90");
    assert_eq!(first.tracking.as_deref(), Some("instagram-bio"));
    assert_eq!(first.program.id, "aaa86f40-d7ae-11ed-acc6-e1c45591a30e");
    assert_eq!(first.program.name, "Curso de Fotografia");
    assert_eq!(first.product, "Curso de Fotografia");
    assert_eq!(first.sold_at, at(9, 10, 14, 20));
    // Cents sent as text, with no sale type, and the marking in s2.
    let second = by_key(&conversions, &sale(102));
    assert_eq!(second.commission, money("123.45", Currency::Brl));
    assert_eq!(second.tracking.as_deref(), Some("telegram"));
    assert_eq!(second.product, "Ebook de Receitas");
    assert_eq!(
        by_key(&conversions, &sale(103)).commission,
        money("15.00", Currency::Usd)
    );
    let small = by_key(&conversions, &sale(107));
    assert_eq!(small.commission, money("0.99", Currency::Brl));
    assert_eq!(small.tracking, None);
    // The producer's own sales, a sale with no affiliate and one in euros
    // are not the owner's Commissions.
    let keys: Vec<&str> = conversions.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(keys.len(), 4);
    for left_out in [104, 105, 106] {
        assert!(!keys.contains(&sale(left_out).as_str()));
    }
}

#[test]
fn the_buyer_partners_card_and_the_owners_document_are_not_kept() {
    let s = setup();
    s.serve(&window(), "sales-shares.json");

    let conversions = s.kiwify.conversions(window()).unwrap();

    let raw: serde_json::Value =
        serde_json::from_str(&by_key(&conversions, &sale(101)).raw).unwrap();
    for private in [
        "customer",
        "revenue_partners",
        "card_last_digits",
        "boleto_url",
    ] {
        assert!(raw.get(private).is_none(), "{private} kept");
    }
    assert!(raw["affiliate_commission"].get("document").is_none());
    assert_eq!(raw["affiliate_commission"]["amount"], 4990);
    assert_eq!(raw["status"], "paid");
    let text = &by_key(&conversions, &sale(101)).raw;
    assert!(!text.contains("comprador@exemplo.com.br"));
    assert!(!text.contains("11122233344"));
}

#[test]
fn pages_until_one_shorter_than_kiwify_serves() {
    let s = setup();
    s.serve_nothing(&window());
    let newest = windows(&(window().start - SALE_CHANGES_FOR..window().end))
        .pop()
        .unwrap();
    s.server.serve(
        &sales_path(&newest, &window(), 1),
        200,
        fixture("sales-page-1.json"),
    );
    s.server.serve(
        &sales_path(&newest, &window(), 2),
        200,
        fixture("sales-page-2.json"),
    );

    let conversions = s.kiwify.conversions(window()).unwrap();

    assert_eq!(conversions.len(), 103);
    assert!(
        !s.server
            .requests()
            .contains(&sales_path(&newest, &window(), 3))
    );
}

#[test]
fn pages_by_the_size_kiwify_serves_when_it_is_smaller_than_asked() {
    let s = setup();
    s.serve_nothing(&window());
    let newest = windows(&(window().start - SALE_CHANGES_FOR..window().end))
        .pop()
        .unwrap();
    s.server.serve(
        &sales_path(&newest, &window(), 1),
        200,
        fixture("sales-small-page-1.json"),
    );
    s.server.serve(
        &sales_path(&newest, &window(), 2),
        200,
        fixture("sales-small-page-2.json"),
    );

    let conversions = s.kiwify.conversions(window()).unwrap();

    assert_eq!(conversions.len(), 14);
}

#[test]
fn stops_when_kiwify_answers_the_same_page_again() {
    let s = setup();
    s.serve_nothing(&window());
    let newest = windows(&(window().start - SALE_CHANGES_FOR..window().end))
        .pop()
        .unwrap();
    for page in [1, 2] {
        s.server.serve(
            &sales_path(&newest, &window(), page),
            200,
            fixture("sales-page-1.json"),
        );
    }

    let conversions = s.kiwify.conversions(window()).unwrap();

    assert_eq!(conversions.len(), 100);
    assert!(
        !s.server
            .requests()
            .contains(&sales_path(&newest, &window(), 3))
    );
}

#[test]
fn reuses_the_token_until_it_is_about_to_expire() {
    let s = setup();
    s.serve_nothing(&window());

    s.kiwify.conversions(window()).unwrap();
    s.clock.advance(TimeDelta::hours(23));
    let later = s.clock.now() - TimeDelta::days(1)..s.clock.now();
    s.serve_nothing(&later);
    s.kiwify.conversions(later.clone()).unwrap();
    assert_eq!(s.token_requests(), 1);

    // 24 hours after it was given, less the margin.
    s.clock.advance(TimeDelta::minutes(56));
    let last = s.clock.now() - TimeDelta::days(1)..s.clock.now();
    s.serve_nothing(&last);
    s.kiwify.conversions(last).unwrap();
    assert_eq!(s.token_requests(), 2);
}

#[test]
fn reads_a_token_lifetime_given_as_text() {
    let s = setup();
    s.server
        .serve(TOKEN, 200, fixture("token-text-expiry.json"));
    s.serve_nothing(&window());

    s.kiwify.conversions(window()).unwrap();
    s.kiwify.conversions(window()).unwrap();

    assert_eq!(s.token_requests(), 1);
    assert!(
        s.sales_requests()
            .iter()
            .all(|request| request.authorization.as_deref() == Some("Bearer kiwify-access-token-2"))
    );
}

#[test]
fn a_token_kiwify_turns_down_is_renewed_once() {
    let s = setup();
    s.serve_nothing(&window());
    let first = sales_paths(&window()).remove(0);
    s.server
        .serve(&first, 400, fixture("error-invalid-token.json"));
    s.server.then_serve(&first, 200, fixture("empty.json"));

    s.kiwify.conversions(window()).unwrap();

    assert_eq!(s.token_requests(), 2);
}

#[test]
fn a_token_turned_down_again_means_the_keys_are_not_accepted() {
    let s = setup();
    s.serve_nothing(&window());
    let first = sales_paths(&window()).remove(0);
    s.server
        .serve(&first, 401, fixture("error-invalid-token.json"));

    assert_eq!(
        s.kiwify.conversions(window()).unwrap_err(),
        PlatformError::Expired
    );
    assert_eq!(s.token_requests(), 2);
}

#[test]
fn credentials_kiwify_does_not_know_are_not_accepted() {
    let s = setup();
    s.server.serve(TOKEN, 400, fixture("token-refused.json"));

    assert_eq!(
        s.kiwify.conversions(window()).unwrap_err(),
        PlatformError::Expired
    );
    assert_eq!(s.sales_requests().len(), 0);
}

#[test]
fn a_query_kiwify_turns_down_is_refused_in_its_words() {
    let s = setup();
    s.serve_nothing(&window());
    let first = sales_paths(&window()).remove(0);
    s.server.serve(&first, 400, fixture("error-bad-query.json"));

    assert_eq!(
        s.kiwify.conversions(window()).unwrap_err(),
        PlatformError::Refused(
            "O período máximo entre o start_date e o end_date é de 90 dias".into()
        )
    );
    assert_eq!(s.token_requests(), 1);
}

#[test]
fn without_every_key_the_connection_is_not_set_up() {
    for missing in [
        "KIWIFY_CLIENT_ID",
        "KIWIFY_CLIENT_SECRET",
        "KIWIFY_ACCOUNT_ID",
    ] {
        let s = setup();
        s.serve_nothing(&window());
        s.store.remove(missing).unwrap();

        assert_eq!(
            s.kiwify.conversions(window()).unwrap_err(),
            PlatformError::NotConnected,
            "{missing}"
        );
        assert!(s.sales_requests().is_empty());
    }
}

#[test]
fn waits_past_a_minute_when_kiwify_limits_the_calls() {
    let s = setup();
    s.serve_nothing(&window());
    let first = sales_paths(&window()).remove(0);
    s.server.serve(&first, 429, fixture("error-429.json"));
    s.server.then_serve(&first, 429, fixture("error-429.json"));
    s.server.then_serve(&first, 200, fixture("empty.json"));

    s.kiwify.conversions(window()).unwrap();

    assert_eq!(s.pauses.seconds(), vec![5, 10]);
}

#[test]
fn gives_up_after_waiting_past_a_minute() {
    let s = setup();
    s.serve_nothing(&window());
    let first = sales_paths(&window()).remove(0);
    s.server.serve(&first, 429, fixture("error-429.json"));

    assert_eq!(
        s.kiwify.conversions(window()).unwrap_err(),
        PlatformError::RateLimited
    );
    assert_eq!(s.pauses.seconds(), vec![5, 10, 20, 40]);
}

#[test]
fn reports_the_withdrawals_whose_money_left_in_the_window() {
    let s = setup();
    s.server
        .serve(&payouts_path(1), 200, fixture("payouts.json"));

    let payouts = s
        .kiwify
        .payouts(at(10, 1, 0, 0)..at(10, 13, 12, 0))
        .unwrap();

    // Under review, rejected and paid long before are left out.
    let keys: Vec<&str> = payouts.iter().map(|payout| payout.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "4d569345-2002-4620-9e73-8d1a255af5c0",
            "8606aae0-e036-4e55-998a-e0a8de7bf54f"
        ]
    );
    assert_eq!(payouts[0].amount, money("996.33", Currency::Brl));
    assert_eq!(
        payouts[0].paid_at,
        Utc.with_ymd_and_hms(2026, 10, 10, 14, 15, 24).unwrap() + TimeDelta::milliseconds(570)
    );
    assert_eq!(payouts[1].amount, money("48.90", Currency::Brl));
    // Kiwify does not say which sales a withdrawal pays.
    assert!(payouts.iter().all(|payout| payout.covers.is_empty()));
    let received = s.server.received();
    let asked = received.last().unwrap();
    assert_eq!(asked.path, payouts_path(1));
    assert_eq!(
        header(asked, "x-kiwify-account-id"),
        Some("XvS0qfkdzCZTg8z")
    );
}

struct Synced {
    _dir: tempfile::TempDir,
    affiliate: Affiliate,
}

async fn affiliate(clock: Arc<ManualClock>) -> Synced {
    let dir = tempfile::tempdir().unwrap();
    let database = Arc::new(
        Database::open(&dir.path().join("mascate.db"))
            .await
            .unwrap(),
    );
    migrate(&database, clock.as_ref(), &[MIGRATIONS])
        .await
        .unwrap();
    Synced {
        affiliate: Affiliate::new(database, clock, Arc::new(SequentialIds::default())),
        _dir: dir,
    }
}

#[test]
fn a_sync_asks_only_what_changed_since_the_last_one_and_the_same_answer_changes_nothing() {
    block_on(async {
        let s = setup();
        let synced = affiliate(s.clock.clone()).await;
        let first = s.clock.now() - s.kiwify.history()..s.clock.now();
        s.serve(&first, "sales-every-status.json");
        s.server
            .serve(&payouts_path(1), 200, fixture("payouts.json"));

        let sync = synced.affiliate.sync(s.kiwify.as_ref()).await.unwrap();

        assert_eq!(sync.platform, AffiliatePlatform::Kiwify);
        assert_eq!(sync.window, first);
        assert_eq!(sync.recorded.added, 11);
        // Every withdrawal paid in five years, the one in June included.
        assert_eq!(sync.payouts.added, 3);

        // An hour later: the hour, plus a day of slack for the clocks.
        s.clock.advance(TimeDelta::hours(1));
        let again = s.clock.now() - TimeDelta::days(1) - TimeDelta::hours(1)..s.clock.now();
        s.serve(&again, "sales-every-status.json");
        let sync = synced.affiliate.sync(s.kiwify.as_ref()).await.unwrap();

        assert_eq!(sync.window, again);
        assert_eq!(
            sync.recorded,
            Recorded {
                added: 0,
                changed: 0,
                unchanged: 11
            }
        );
        // No withdrawal changed in the last day.
        assert_eq!(sync.payouts, Recorded::default());
        // Reading every withdrawal again keeps each once, by its id.
        let again = s.kiwify.payouts(first.clone()).unwrap();
        let recorded = synced
            .affiliate
            .record_platform_payouts(AffiliatePlatform::Kiwify, &again)
            .await
            .unwrap();
        assert_eq!(
            recorded,
            Recorded {
                added: 0,
                changed: 0,
                unchanged: 3
            }
        );
        assert_eq!(
            synced
                .affiliate
                .payouts(Timestamp::MIN_UTC..Timestamp::MAX_UTC)
                .await
                .unwrap()
                .len(),
            3
        );

        // A refund since: the same Commission turns reversed.
        s.clock.advance(TimeDelta::hours(1));
        let then = s.clock.now() - TimeDelta::days(1) - TimeDelta::hours(1)..s.clock.now();
        s.serve(&then, "sales-refunded.json");
        let sync = synced.affiliate.sync(s.kiwify.as_ref()).await.unwrap();

        assert_eq!(sync.recorded.changed, 1);
        let commissions = synced
            .affiliate
            .commissions(&CommissionFilter::default())
            .await
            .unwrap();
        assert_eq!(commissions.len(), 11);
        let refunded = commissions
            .iter()
            .find(|commission| commission.amount == money("10.08", Currency::Brl))
            .unwrap();
        assert_eq!(refunded.status, CommissionStatus::Reversed);
        assert_eq!(refunded.platform_status, "refunded");
        assert_eq!(refunded.status_since, at(10, 13, 10, 30));
    });
}

#[test]
fn the_owner_confirms_which_commissions_a_withdrawal_pays() {
    block_on(async {
        let s = setup();
        let synced = affiliate(s.clock.clone()).await;
        let first = s.clock.now() - s.kiwify.history()..s.clock.now();
        s.serve(&first, "sales-every-status.json");
        s.server
            .serve(&payouts_path(1), 200, fixture("payouts.json"));
        synced.affiliate.sync(s.kiwify.as_ref()).await.unwrap();
        let withdrawal = synced
            .affiliate
            .payouts(Timestamp::MIN_UTC..Timestamp::MAX_UTC)
            .await
            .unwrap()
            .into_iter()
            .find(|payout| payout.amount == money("48.90", Currency::Brl))
            .unwrap();

        let proposal = synced.affiliate.propose_cover(withdrawal.id).await.unwrap();
        // The approved and paid sales: R$ 10,07 and R$ 10,08.
        assert_eq!(proposal.candidates.len(), 2);
        assert_eq!(proposal.proposed.len(), 2);
        let covered = synced
            .affiliate
            .cover_payout(withdrawal.id, &proposal.proposed)
            .await
            .unwrap();

        assert_eq!(covered.covered(), money("20.15", Currency::Brl));
        assert_eq!(covered.difference(), money("28.75", Currency::Brl));
        assert!(
            covered
                .covers
                .iter()
                .all(|commission| commission.status == CommissionStatus::Paid)
        );
    });
}
