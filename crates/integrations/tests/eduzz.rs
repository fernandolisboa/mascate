//! Eduzz's adapter against the fake server serving answers written from
//! the Developer Hub documentation (see the fixtures' README), and the
//! Affiliate module's Sync over it with a real temporary database.

mod common;

use std::ops::Range;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use common::query_value;
use futures::executor::block_on;
use mascate_affiliate::LinkTagger;
use mascate_affiliate::{
    Affiliate, AffiliatePlatform, CommissionFilter, CommissionSource, CommissionStatus, MIGRATIONS,
    Recorded, ReportedConversion,
};
use mascate_integrations::{EDUZZ_LINKS, Eduzz, Pause};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Currency, Money, PlatformError, Timestamp};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;

/// The personal token from the documentation's example.
const TOKEN: &str = "qzrUZcm4dISz/ayXgq7g9+GmusYbXHmIpJ7fbLYDIjUPtNwAN1rrsRZbeJ6e6tAlSUSy3w==";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/eduzz/{name}",
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
    eduzz: Arc<Eduzz>,
}

/// Eduzz at the fake server, with the personal token pasted, on 2026-10-13
/// 12:00 UTC.
fn setup() -> Setup {
    let server = FakeHttpServer::start();
    let store = Arc::new(MemorySecretStore::default());
    store
        .write("EDUZZ_PERSONAL_TOKEN", &Secret::new(TOKEN))
        .unwrap();
    let clock = Arc::new(ManualClock::at(at(10, 13, 12, 0)));
    let pauses = Arc::new(RecordedPauses::default());
    let eduzz = Arc::new(
        Eduzz::new(server.url(), "Mascate/test", store.clone()).with_pause(pauses.clone()),
    );
    Setup {
        server,
        store,
        clock,
        pauses,
        eduzz,
    }
}

/// The last day and a bit, as a Sync after the previous one asks it.
fn window() -> Range<Timestamp> {
    at(10, 12, 11, 0)..at(10, 13, 12, 0)
}

/// How far back Eduzz lets the app read.
const HISTORY: TimeDelta = TimeDelta::days(5 * 365);

/// How far before the window the sales are read.
const CREDIT_LAG: TimeDelta = TimeDelta::days(90);

/// A moment as the query carries it, encoded.
fn encoded(at: Timestamp) -> String {
    at.format("%Y-%m-%dT%H%%3A%M%%3A%S%.3fZ").to_string()
}

fn range(during: &Range<Timestamp>) -> String {
    format!(
        "startDate={}&endDate={}",
        encoded(during.start),
        encoded(during.end)
    )
}

fn statement_path(during: &Range<Timestamp>, page: u32) -> String {
    format!(
        "/myeduzz/v2/financial/statement?{}&page={page}&itemsPerPage=100",
        range(during)
    )
}

/// The statement of one sale, over everything Eduzz lets the app read.
fn sale_statement_path(during: &Range<Timestamp>, sale: &str) -> String {
    format!(
        "/myeduzz/v2/financial/statement?{}&saleId={sale}&page=1&itemsPerPage=100",
        range(&(during.end - HISTORY..during.end))
    )
}

/// The sales that changed from `CREDIT_LAG` before `during` to its end.
fn sales_path(during: &Range<Timestamp>, page: u32) -> String {
    let since = (during.start - CREDIT_LAG).max(during.end - HISTORY);
    format!(
        "/myeduzz/v1/sales?{}&referenceDate=updatedAt&page={page}&itemsPerPage=100",
        range(&(since..during.end))
    )
}

fn transfers_path(page: u32) -> String {
    format!("/myeduzz/v1/financial/transfers?page={page}&itemsPerPage=100")
}

impl Setup {
    /// Answers the window's statement and sales with nothing.
    fn serve_nothing(&self, during: &Range<Timestamp>) {
        self.server.serve(
            &statement_path(during, 1),
            200,
            fixture("statement-empty.json"),
        );
        self.server
            .serve(&sales_path(during, 1), 200, fixture("sales-empty.json"));
    }

    /// The window of the fixtures: two pages of statement, the sales list
    /// and the statement of each sale credited before it.
    fn serve_window(&self, during: &Range<Timestamp>) {
        self.server.serve(
            &statement_path(during, 1),
            200,
            fixture("statement-page-1.json"),
        );
        self.server.serve(
            &statement_path(during, 2),
            200,
            fixture("statement-page-2.json"),
        );
        self.server
            .serve(&sales_path(during, 1), 200, fixture("sales.json"));
        for sale in ["1004", "1005", "1007"] {
            self.server.serve(
                &sale_statement_path(during, sale),
                200,
                fixture(&format!("statement-sale-{sale}.json")),
            );
        }
        // Paid, but not credited yet.
        self.server.serve(
            &sale_statement_path(during, "1009"),
            200,
            fixture("statement-empty.json"),
        );
    }

    fn sale_lookups(&self) -> Vec<String> {
        self.server
            .requests()
            .into_iter()
            .filter(|path| path.contains("saleId="))
            .collect()
    }
}

fn by_key<'a>(conversions: &'a [ReportedConversion], key: &str) -> &'a ReportedConversion {
    conversions
        .iter()
        .find(|conversion| conversion.key == key)
        .unwrap_or_else(|| panic!("no conversion {key}"))
}

fn keys(conversions: &[ReportedConversion]) -> Vec<&str> {
    conversions
        .iter()
        .map(|conversion| conversion.key.as_str())
        .collect()
}

#[test]
fn reads_the_statement_and_the_sales_of_the_window_with_the_personal_token() {
    let s = setup();
    s.serve_nothing(&window());

    let conversions = s.eduzz.conversions(window()).unwrap();

    assert!(conversions.is_empty());
    let received = s.server.received();
    let asked: Vec<&str> = received
        .iter()
        .map(|request| request.path.as_str())
        .collect();
    assert_eq!(
        asked,
        [statement_path(&window(), 1), sales_path(&window(), 1)]
    );
    assert!(
        asked[0].contains("startDate=2026-10-12T11%3A00%3A00.000Z")
            && asked[0].contains("endDate=2026-10-13T12%3A00%3A00.000Z")
    );
    // The sales from 90 days before, by the date they last changed.
    assert!(asked[1].contains("startDate=2026-07-14T11%3A00%3A00.000Z"));
    assert!(received.iter().all(|request| {
        request.method == "GET"
            && request.authorization.as_deref() == Some(format!("bearer {TOKEN}").as_str())
    }));
}

#[test]
fn a_token_pasted_with_bearer_in_front_is_sent_once() {
    let s = setup();
    s.store
        .write(
            "EDUZZ_PERSONAL_TOKEN",
            &Secret::new(format!(" Bearer {TOKEN}\n")),
        )
        .unwrap();
    s.serve_nothing(&window());

    s.eduzz.conversions(window()).unwrap();

    let received = s.server.received();
    assert_eq!(
        received[0].authorization.as_deref(),
        Some(format!("bearer {TOKEN}").as_str())
    );
}

#[test]
fn statement_and_sales_converge_on_one_conversion_per_sale() {
    let s = setup();
    s.serve_window(&window());

    let conversions = s.eduzz.conversions(window()).unwrap();

    // The producer's own sale, the entries without a sale, the Commission in
    // euro, the sale without an affiliate and the one not credited yet are
    // left out.
    assert_eq!(
        keys(&conversions),
        ["1001", "1003", "1004", "1005", "1007", "1010"]
    );

    // Only in the statement: its line is the product.
    let alone = by_key(&conversions, "1001");
    assert_eq!(alone.commission, money("48.5", Currency::Brl));
    assert_eq!(alone.status, CommissionStatus::Approved);
    assert_eq!(alone.platform_status, "affiliante_sale");
    assert_eq!(alone.program.id, "eduzz");
    assert_eq!(alone.program.name, "Eduzz");
    assert_eq!(alone.product, "Comissão de afiliado referente à venda 1001");
    assert_eq!(alone.sold_at, at(10, 12, 15, 0));
    assert_eq!(alone.status_changed_at, Some(at(10, 12, 15, 0)));
    assert_eq!(alone.tracking, None);

    // Listed too: the product, the status and the first tracker not blank.
    let listed = by_key(&conversions, "1003");
    assert_eq!(listed.commission, money("37.9", Currency::Brl));
    assert_eq!(listed.status, CommissionStatus::Approved);
    assert_eq!(listed.platform_status, "paid");
    assert_eq!(listed.program.id, "2001");
    assert_eq!(listed.program.name, "Curso de Excel do Zero");
    assert_eq!(listed.product, "Curso de Excel do Zero");
    assert_eq!(listed.sold_at, at(10, 12, 14, 0));
    assert_eq!(listed.status_changed_at, Some(at(10, 12, 14, 5)));
    assert_eq!(listed.tracking.as_deref(), Some("insta-bio"));

    // Credited now for a sale that last changed in August.
    let late = by_key(&conversions, "1010");
    assert_eq!(late.commission, money("89.4", Currency::Brl));
    assert_eq!(late.program.name, "Mentoria em Vendas");
    assert_eq!(late.tracking.as_deref(), Some("wpp-grupo"));
    assert_eq!(late.sold_at, at(8, 20, 17, 0));

    // Changed in the window, credited in September: looked up by its id.
    let waiting = by_key(&conversions, "1007");
    assert_eq!(waiting.commission, money("23.6", Currency::Brl));
    assert_eq!(waiting.status, CommissionStatus::Pending);
    assert_eq!(waiting.platform_status, "waitingRefund");
    assert_eq!(waiting.status_changed_at, None);
    assert_eq!(waiting.tracking.as_deref(), Some("yt-video-3"));
}

#[test]
fn refund_and_chargeback_entries_reverse_the_commission_of_the_sale_they_name() {
    let s = setup();
    s.serve_window(&window());

    let conversions = s.eduzz.conversions(window()).unwrap();

    let refunded = by_key(&conversions, "1004");
    assert_eq!(refunded.status, CommissionStatus::Reversed);
    assert_eq!(refunded.platform_status, "refund");
    // The Commission credited in September, not the refund's negative line.
    assert_eq!(refunded.commission, money("29.7", Currency::Brl));
    assert_eq!(refunded.sold_at, at(9, 28, 13, 0));
    assert_eq!(refunded.status_changed_at, Some(at(10, 12, 20, 0)));

    let charged_back = by_key(&conversions, "1005");
    assert_eq!(charged_back.status, CommissionStatus::Reversed);
    assert_eq!(charged_back.platform_status, "chargerback_success");
    assert_eq!(charged_back.commission, money("59.1", Currency::Brl));
    assert_eq!(charged_back.status_changed_at, Some(at(10, 13, 1, 15)));
    let raw: serde_json::Value = serde_json::from_str(&charged_back.raw).unwrap();
    let entries: Vec<&str> = raw["statement"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect();
    assert_eq!(entries, ["640105", "700006"]);
}

#[test]
fn looks_up_only_the_affiliate_sales_whose_commission_is_outside_the_window() {
    let s = setup();
    s.serve_window(&window());

    s.eduzz.conversions(window()).unwrap();

    // 1001, 1003 and 1010 were credited in the window; 1008 has no
    // affiliate, so it pays the owner no Commission.
    assert_eq!(
        s.sale_lookups(),
        ["1004", "1005", "1007", "1009"].map(|sale| sale_statement_path(&window(), sale))
    );
}

#[test]
fn every_sale_status_maps_to_a_commission_status() {
    let s = setup();
    s.server.serve(
        &statement_path(&window(), 1),
        200,
        fixture("statement-every-status.json"),
    );
    s.server.serve(
        &sales_path(&window(), 1),
        200,
        fixture("sales-every-status.json"),
    );

    let conversions = s.eduzz.conversions(window()).unwrap();

    let expected = [
        ("2001", "open", CommissionStatus::Pending),
        ("2002", "processing", CommissionStatus::Pending),
        ("2003", "paid", CommissionStatus::Approved),
        ("2004", "canceled", CommissionStatus::Cancelled),
        ("2005", "waitingRefund", CommissionStatus::Pending),
        ("2006", "refunded", CommissionStatus::Reversed),
        ("2007", "duplicated", CommissionStatus::Cancelled),
        ("2008", "expired", CommissionStatus::Cancelled),
        ("2009", "recovering", CommissionStatus::Pending),
        ("2010", "waitingPayment", CommissionStatus::Pending),
        ("2011", "scheduled", CommissionStatus::Pending),
        ("2012", "negotiated", CommissionStatus::Pending),
    ];
    assert_eq!(conversions.len(), expected.len());
    for (key, platform_status, status) in expected {
        let conversion = by_key(&conversions, key);
        assert_eq!(conversion.platform_status, platform_status, "{key}");
        assert_eq!(conversion.status, status, "{key}");
    }
    assert_eq!(
        by_key(&conversions, "2003").status_changed_at,
        Some(at(10, 12, 11, 35))
    );
    // Reversed by the sale's own status: when Eduzz refunded it.
    assert_eq!(
        by_key(&conversions, "2006").status_changed_at,
        Some(at(10, 13, 9, 0))
    );
    assert_eq!(by_key(&conversions, "2004").status_changed_at, None);
    // Every one was credited in the window: nothing to look up.
    assert!(s.sale_lookups().is_empty());
}

#[test]
fn amounts_become_exact_decimals_and_installments_add_up() {
    let s = setup();
    s.server.serve(
        &statement_path(&window(), 1),
        200,
        fixture("statement-amounts.json"),
    );
    s.server
        .serve(&sales_path(&window(), 1), 200, fixture("sales-empty.json"));

    let conversions = s.eduzz.conversions(window()).unwrap();

    // Text and a sale id sent as a number.
    assert_eq!(
        by_key(&conversions, "3001").commission,
        money("19.90", Currency::Brl)
    );
    // Credited twice: 25 + 25.01.
    let twice = by_key(&conversions, "3002");
    assert_eq!(twice.commission, money("50.01", Currency::Brl));
    assert_eq!(twice.sold_at, at(10, 12, 13, 0));
    // In dollars, with a blank line: the sale names the product.
    let dollars = by_key(&conversions, "3003");
    assert_eq!(dollars.commission, money("7.5", Currency::Usd));
    assert_eq!(dollars.product, "Venda 3003");
}

#[test]
fn the_buyer_payment_and_other_people_paid_are_not_kept() {
    let s = setup();
    s.server.serve(
        &statement_path(&window(), 1),
        200,
        fixture("statement-every-status.json"),
    );
    s.server.serve(
        &sales_path(&window(), 1),
        200,
        fixture("sales-every-status.json"),
    );

    let conversions = s.eduzz.conversions(window()).unwrap();

    for conversion in &conversions {
        for private in [
            "comprador@example.com",
            "12345678900",
            "produtora@example.com",
            "fernando@example.com",
            "edz.la",
            "Não fiz essa compra",
        ] {
            assert!(
                !conversion.raw.contains(private),
                "{} keeps {private}",
                conversion.key
            );
        }
    }
    let raw: serde_json::Value = serde_json::from_str(&by_key(&conversions, "2006").raw).unwrap();
    assert_eq!(raw["sale"]["product"]["name"], "Curso de Excel do Zero");
    assert_eq!(
        raw["sale"]["refund"]["refundedAt"],
        "2026-10-13T09:00:00.000Z"
    );
    assert_eq!(raw["statement"][0]["type"], "affiliante_sale");
}

#[test]
fn reading_the_same_window_again_gives_the_same_conversions() {
    let s = setup();
    s.serve_window(&window());

    let first = s.eduzz.conversions(window()).unwrap();
    let again = s.eduzz.conversions(window()).unwrap();

    assert_eq!(first, again);
}

#[test]
fn stops_when_eduzz_answers_the_same_page_again() {
    let s = setup();
    // Eduzz says there are two pages but serves the first twice.
    s.server.serve(
        &statement_path(&window(), 1),
        200,
        fixture("statement-page-1.json"),
    );
    s.server.serve(
        &statement_path(&window(), 2),
        200,
        fixture("statement-page-1.json"),
    );
    s.server
        .serve(&sales_path(&window(), 1), 200, fixture("sales-empty.json"));
    s.server.serve(
        &sale_statement_path(&window(), "1004"),
        200,
        fixture("statement-sale-1004.json"),
    );

    let conversions = s.eduzz.conversions(window()).unwrap();

    assert_eq!(keys(&conversions), ["1001", "1003", "1004"]);
    assert_eq!(
        s.server
            .requests()
            .iter()
            .filter(
                |path| path.starts_with("/myeduzz/v2/financial/statement?startDate")
                    && !path.contains("saleId")
            )
            .count(),
        2
    );
}

#[test]
fn a_first_read_goes_back_as_far_as_eduzz_lets() {
    let s = setup();
    let first = s.clock.now() - s.eduzz.history()..s.clock.now();
    s.serve_nothing(&first);

    s.eduzz.conversions(first.clone()).unwrap();

    // The sales are not asked before the history either.
    assert_eq!(
        s.server.requests(),
        [statement_path(&first, 1), sales_path(&first, 1)]
    );
    assert!(sales_path(&first, 1).contains(&range(&first)));
}

#[test]
fn a_token_eduzz_turns_down_is_not_accepted() {
    let s = setup();
    s.server.serve(
        &statement_path(&window(), 1),
        401,
        fixture("error-401.json"),
    );

    let answer = s.eduzz.conversions(window());

    assert_eq!(answer, Err(PlatformError::Expired));
    assert_eq!(s.server.requests().len(), 1);
}

#[test]
fn a_scope_the_app_lacks_is_refused_in_eduzz_words() {
    let s = setup();
    s.server.serve(
        &statement_path(&window(), 1),
        403,
        fixture("error-403.json"),
    );

    let answer = s.eduzz.conversions(window());

    assert_eq!(
        answer,
        Err(PlatformError::Refused(
            "O aplicativo não tem o escopo myeduzz_financial_read".into()
        ))
    );
}

#[test]
fn without_the_token_the_connection_is_not_set_up() {
    let s = setup();
    s.store.remove("EDUZZ_PERSONAL_TOKEN").unwrap();

    assert_eq!(
        s.eduzz.conversions(window()),
        Err(PlatformError::NotConnected)
    );
    assert_eq!(s.eduzz.payouts(window()), Err(PlatformError::NotConnected));
    assert!(s.server.requests().is_empty());
}

#[test]
fn waits_past_a_minute_when_eduzz_limits_the_calls() {
    let s = setup();
    let statement = statement_path(&window(), 1);
    s.server.serve(&statement, 429, fixture("error-429.json"));
    s.server
        .then_serve(&statement, 429, fixture("error-429.json"));
    s.server
        .then_serve(&statement, 200, fixture("statement-empty.json"));
    s.server
        .serve(&sales_path(&window(), 1), 200, fixture("sales-empty.json"));

    let conversions = s.eduzz.conversions(window()).unwrap();

    assert!(conversions.is_empty());
    assert_eq!(s.pauses.seconds(), [5, 10]);
}

#[test]
fn gives_up_after_waiting_past_a_minute() {
    let s = setup();
    s.server.serve(
        &statement_path(&window(), 1),
        429,
        fixture("error-429.json"),
    );

    let answer = s.eduzz.conversions(window());

    assert_eq!(answer, Err(PlatformError::RateLimited));
    assert_eq!(s.pauses.seconds(), [5, 10, 20, 40]);
    assert_eq!(s.server.requests().len(), 5);
}

#[test]
fn reports_the_transfers_that_reached_the_bank_in_the_window() {
    let s = setup();
    s.server
        .serve(&transfers_path(1), 200, fixture("transfers.json"));

    let payouts = s.eduzz.payouts(window()).unwrap();

    // Pending, canceled and confirmed back in June are left out.
    let keys: Vec<&str> = payouts.iter().map(|payout| payout.key.as_str()).collect();
    assert_eq!(keys, ["900001", "900004"]);
    assert_eq!(payouts[0].amount, money("1250.4", Currency::Brl));
    assert_eq!(payouts[0].paid_at, at(10, 12, 17, 20));
    // A bare date is midnight in Brasília.
    assert_eq!(payouts[1].amount, money("45.9", Currency::Brl));
    assert_eq!(payouts[1].paid_at, at(10, 13, 3, 0));
    // Eduzz does not say which sales a transfer pays, and the bank account
    // is not kept.
    assert!(payouts.iter().all(|payout| payout.covers.is_empty()));
    assert!(
        payouts
            .iter()
            .all(|payout| !payout.raw.contains("bankAccount"))
    );
    assert_eq!(s.server.requests(), [transfers_path(1)]);
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
fn a_sync_records_each_sale_once_and_the_same_answer_changes_nothing() {
    block_on(async {
        let s = setup();
        let synced = affiliate(s.clock.clone()).await;
        let first = s.clock.now() - s.eduzz.history()..s.clock.now();
        s.server.serve(
            &statement_path(&first, 1),
            200,
            fixture("statement-every-status.json"),
        );
        s.server.serve(
            &sales_path(&first, 1),
            200,
            fixture("sales-every-status.json"),
        );
        s.server
            .serve(&transfers_path(1), 200, fixture("transfers.json"));

        let sync = synced.affiliate.sync(s.eduzz.as_ref()).await.unwrap();

        assert_eq!(sync.platform, AffiliatePlatform::Eduzz);
        assert_eq!(sync.window, first);
        assert_eq!(sync.recorded.added, 12);
        // Every confirmed transfer in five years, June's included.
        assert_eq!(sync.payouts.added, 3);

        // An hour later: the hour, plus a day of slack for the clocks.
        s.clock.advance(TimeDelta::hours(1));
        let again = s.clock.now() - TimeDelta::days(1) - TimeDelta::hours(1)..s.clock.now();
        s.server.serve(
            &statement_path(&again, 1),
            200,
            fixture("statement-every-status.json"),
        );
        s.server.serve(
            &sales_path(&again, 1),
            200,
            fixture("sales-every-status.json"),
        );
        let sync = synced.affiliate.sync(s.eduzz.as_ref()).await.unwrap();

        assert_eq!(sync.window, again);
        assert_eq!(
            sync.recorded,
            Recorded {
                added: 0,
                changed: 0,
                unchanged: 12
            }
        );
        assert_eq!(
            sync.payouts,
            Recorded {
                added: 0,
                changed: 0,
                unchanged: 2
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
    });
}

#[test]
fn a_refund_in_a_later_sync_reverses_the_commission_it_names() {
    block_on(async {
        let s = setup();
        // September's Sync recorded sale 1004's Commission.
        let clock = Arc::new(ManualClock::at(at(9, 28, 14, 0)));
        let synced = affiliate(clock.clone()).await;
        let september = clock.now() - s.eduzz.history()..clock.now();
        s.server.serve(
            &statement_path(&september, 1),
            200,
            r#"{"pages":1,"page":1,"itemsPerPage":100,"totalItems":1,"items":[
                {"id":"690104","creditDate":"2026-09-28T13:00:00.000Z",
                 "description":"Comissão de afiliado referente à venda 1004","saleId":"1004",
                 "type":"affiliante_sale","value":{"currency":"BRL","value":29.7}}]}"#,
        );
        s.server
            .serve(&sales_path(&september, 1), 200, fixture("sales-empty.json"));
        s.server
            .serve(&transfers_path(1), 200, fixture("statement-empty.json"));
        synced.affiliate.sync(s.eduzz.as_ref()).await.unwrap();

        // October: the refund, among the window's other entries.
        clock.advance(at(10, 13, 12, 0) - clock.now());
        let october = at(9, 27, 14, 0)..clock.now();
        s.serve_window(&october);
        let sync = synced.affiliate.sync(s.eduzz.as_ref()).await.unwrap();

        assert_eq!(sync.window, october);
        assert_eq!(sync.recorded.changed, 1);
        let commissions = synced
            .affiliate
            .commissions(&CommissionFilter::default())
            .await
            .unwrap();
        let refunded: Vec<_> = commissions
            .iter()
            .filter(|commission| commission.amount == money("29.7", Currency::Brl))
            .collect();
        assert_eq!(refunded.len(), 1);
        assert_eq!(refunded[0].status, CommissionStatus::Reversed);
        assert_eq!(refunded[0].platform_status, "refund");
        assert_eq!(refunded[0].status_since, at(10, 12, 20, 0));
    });
}

#[test]
fn the_owner_confirms_which_commissions_a_transfer_pays() {
    block_on(async {
        let s = setup();
        let synced = affiliate(s.clock.clone()).await;
        let first = s.clock.now() - s.eduzz.history()..s.clock.now();
        s.server.serve(
            &statement_path(&first, 1),
            200,
            fixture("statement-every-status.json"),
        );
        s.server.serve(
            &sales_path(&first, 1),
            200,
            fixture("sales-every-status.json"),
        );
        s.server
            .serve(&transfers_path(1), 200, fixture("transfers.json"));
        synced.affiliate.sync(s.eduzz.as_ref()).await.unwrap();
        let transfer = synced
            .affiliate
            .payouts(Timestamp::MIN_UTC..Timestamp::MAX_UTC)
            .await
            .unwrap()
            .into_iter()
            .find(|payout| payout.amount == money("45.9", Currency::Brl))
            .unwrap();

        let proposal = synced.affiliate.propose_cover(transfer.id).await.unwrap();
        // The only approved sale: R$ 10,03.
        assert_eq!(proposal.candidates.len(), 1);
        let covered = synced
            .affiliate
            .cover_payout(transfer.id, &proposal.proposed)
            .await
            .unwrap();

        assert_eq!(covered.covered(), money("10.03", Currency::Brl));
        assert_eq!(covered.difference(), money("35.87", Currency::Brl));
        assert!(
            covered
                .covers
                .iter()
                .all(|commission| commission.status == CommissionStatus::Paid)
        );
    });
}

/// What the app's link carries in `utm_content` comes back in the sale's
/// `utm.content` and wins over the trackers.
#[test]
fn a_tagged_eduzz_link_comes_back_as_the_sales_marking() {
    let tagged = EDUZZ_LINKS
        .tag(
            "https://sun.eduzz.com/2001?a=88231&utm_source=x",
            "telegram",
        )
        .unwrap();
    assert_eq!(
        tagged,
        "https://sun.eduzz.com/2001?a=88231&utm_source=x&utm_content=telegram"
    );
    let content = query_value(&tagged, "utm_content").unwrap();
    let s = setup();
    s.serve_window(&window());
    s.server.serve(
        &sales_path(&window(), 1),
        200,
        fixture("sales.json").replacen(
            r#""content": null"#,
            &format!(r#""content": "{content}""#),
            1,
        ),
    );

    let conversions = s.eduzz.conversions(window()).unwrap();

    // Its tracker2 is insta-bio.
    assert_eq!(
        by_key(&conversions, "1003").tracking.as_deref(),
        Some("telegram")
    );
}
