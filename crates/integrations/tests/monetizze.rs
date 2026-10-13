//! Monetizze's adapter against the fake server serving answers written from
//! the API 2.1 documentation (see the fixtures' README), and the Affiliate
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
use mascate_integrations::{Monetizze, Pause};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Currency, Money, PlatformError, Timestamp};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;

/// Monetizze's status codes, in the order the adapter asks them.
const STATUSES: [u8; 6] = [1, 2, 3, 4, 5, 6];

const TOKEN: &str = "c278d74dc89580e88522228598d051cb";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/monetizze/{name}",
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
    monetizze: Arc<Monetizze>,
}

/// Monetizze at the fake server, with the key and the account's e-mail
/// pasted, on 2026-10-13 12:00 UTC.
fn setup() -> Setup {
    let server = FakeHttpServer::start();
    let store = Arc::new(MemorySecretStore::default());
    for (name, value) in [
        ("MONETIZZE_CONSUMER_KEY", "consumer-key-123"),
        ("MONETIZZE_EMAIL", " afiliado.mascate@exemplo.com.br "),
    ] {
        store.write(name, &Secret::new(value)).unwrap();
    }
    let clock = Arc::new(ManualClock::at(at(10, 13, 12, 0)));
    let pauses = Arc::new(RecordedPauses::default());
    let monetizze = Arc::new(
        Monetizze::new(server.url(), "Mascate/test", store.clone(), clock.clone())
            .with_pause(pauses.clone()),
    );
    server.serve("/token", 200, fixture("token.json"));
    Setup {
        server,
        store,
        clock,
        pauses,
        monetizze,
    }
}

/// September and October up to the clock.
fn window() -> Range<Timestamp> {
    at(9, 1, 3, 0)..at(10, 13, 12, 0)
}

/// The request for one page of the sales in `status` during `during`, its
/// dates on Brasília's clock.
fn path(status: u8, during: &Range<Timestamp>, page: u32) -> String {
    let brasilia = |at: Timestamp| {
        (at - TimeDelta::hours(3))
            .format("%Y-%m-%d%%20%H%%3A%M%%3A%S")
            .to_string()
    };
    format!(
        "/transactions?date_min={}&date_max={}&status%5B%5D={status}&page={page}",
        brasilia(during.start),
        brasilia(during.end)
    )
}

impl Setup {
    /// Answers every status with an empty page.
    fn serve_nothing(&self, during: &Range<Timestamp>) {
        for status in STATUSES {
            self.server
                .serve(&path(status, during, 1), 200, fixture("empty.json"));
        }
    }

    fn serve(&self, status: u8, during: &Range<Timestamp>, name: &str) {
        self.server
            .serve(&path(status, during, 1), 200, fixture(name));
    }

    /// One sale in each status, each in the answer for its own status.
    fn serve_every_status(&self, during: &Range<Timestamp>) {
        for status in STATUSES {
            self.serve(status, during, &format!("status-{status}.json"));
        }
    }

    fn token_requests(&self) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|path| path.as_str() == "/token")
            .count()
    }
}

fn by_key<'a>(conversions: &'a [ReportedConversion], key: &str) -> &'a ReportedConversion {
    conversions
        .iter()
        .find(|conversion| conversion.key == key)
        .unwrap_or_else(|| panic!("no conversion {key}"))
}

fn header<'a>(request: &'a mascate_platform::testing::Request, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

#[test]
fn asks_a_token_with_the_consumer_key_then_each_status_with_it() {
    let s = setup();
    s.serve_nothing(&window());

    let conversions = s.monetizze.conversions(window()).unwrap();

    assert!(conversions.is_empty());
    let received = s.server.received();
    let token = &received[0];
    assert_eq!(
        (token.method.as_str(), token.path.as_str()),
        ("GET", "/token")
    );
    assert_eq!(header(token, "x_consumer_key"), Some("consumer-key-123"));
    // One query per status, by the date the sale was registered, on
    // Brasília's clock: 2026-09-01 03:00 UTC is midnight there.
    assert!(path(1, &window(), 1).contains("date_min=2026-09-01%2000%3A00%3A00"));
    let asked: Vec<&str> = received[1..]
        .iter()
        .map(|request| request.path.as_str())
        .collect();
    let expected: Vec<String> = STATUSES
        .iter()
        .map(|&status| path(status, &window(), 1))
        .collect();
    assert_eq!(asked, expected);
    assert!(
        received[1..]
            .iter()
            .all(|request| request.method == "GET" && header(request, "token") == Some(TOKEN))
    );
}

#[test]
fn every_monetizze_status_maps_to_a_commission_status_and_keeps_the_shown_one() {
    let s = setup();
    s.serve_every_status(&window());

    let conversions = s.monetizze.conversions(window()).unwrap();

    let statuses: Vec<(&str, &str, CommissionStatus)> = conversions
        .iter()
        .map(|conversion| {
            (
                conversion.key.as_str(),
                conversion.platform_status.as_str(),
                conversion.status,
            )
        })
        .collect();
    use CommissionStatus::*;
    assert_eq!(
        statuses,
        [
            ("5001", "Aguardando pagamento", Pending),
            ("5002", "Finalizada", Approved),
            ("5003", "Cancelada", Cancelled),
            ("5004", "Devolvida", Reversed),
            ("5005", "Bloqueada", Pending),
            ("5006", "Completa", Approved),
        ]
    );
    // Only an approved sale says when it was approved: its finalization,
    // 10:30 in Brasília.
    let changed: Vec<Option<Timestamp>> = conversions
        .iter()
        .map(|conversion| conversion.status_changed_at)
        .collect();
    assert_eq!(
        changed,
        [
            None,
            Some(at(9, 2, 13, 30)),
            None,
            None,
            None,
            Some(at(9, 6, 13, 30))
        ]
    );
}

#[test]
fn the_status_comes_from_the_code_asked_not_the_text_shown() {
    let s = setup();
    s.serve_nothing(&window());
    // The approved sale's text, in the answer to the returned ones.
    s.serve(4, &window(), "status-2.json");

    let conversions = s.monetizze.conversions(window()).unwrap();

    assert_eq!(conversions[0].status, CommissionStatus::Reversed);
    assert_eq!(conversions[0].platform_status, "Finalizada");
}

#[test]
fn picks_the_owners_share_among_several_affiliates_by_the_accounts_email() {
    let s = setup();
    s.serve_nothing(&window());
    s.serve(2, &window(), "shares.json");

    let conversions = s.monetizze.conversions(window()).unwrap();

    // The sale in euros, the one with two other affiliates and the one with
    // only producers are left out.
    let keys: Vec<&str> = conversions
        .iter()
        .map(|conversion| conversion.key.as_str())
        .collect();
    assert_eq!(keys, ["7101", "7102", "7103"]);

    // Four shares, two of them affiliates': the owner's is the one with the
    // account's e-mail, whatever its case.
    let several = by_key(&conversions, "7101");
    assert_eq!(several.commission, money("59.10", Currency::Brl));
    assert_eq!(several.program.id, "12832");
    assert_eq!(several.program.name, "Curso de Fotografia com Celular");
    assert_eq!(several.product, "Curso de Fotografia com Celular");
    // 09:00 and 09:10 in Brasília.
    assert_eq!(several.sold_at, at(9, 3, 12, 0));
    assert_eq!(several.status_changed_at, Some(at(9, 3, 12, 10)));
    assert_eq!(several.tracking.as_deref(), Some("instagram_bio"));
    let raw: serde_json::Value = serde_json::from_str(&several.raw).unwrap();
    assert_eq!(raw["venda"]["codigo"], "7101");
    // The buyer is never kept.
    assert!(raw["venda"].get("comprador").is_none());

    // Shares beside the sale, as in the documentation's example, without
    // e-mails: the only affiliate one is the owner's. A JSON number and no
    // currency, from before Monetizze sold abroad, still read as reais.
    let beside = by_key(&conversions, "7102");
    assert_eq!(beside.commission, money("33.6", Currency::Brl));
    assert_eq!(beside.program.id, "12900");
    assert_eq!(beside.sold_at, at(9, 5, 0, 30));
    assert_eq!(beside.tracking, None);
    let raw: serde_json::Value = serde_json::from_str(&beside.raw).unwrap();
    assert!(raw.get("comprador").is_none());

    // Commissions in dollars stay in dollars, without conversion.
    let dollars = by_key(&conversions, "7103");
    assert_eq!(dollars.commission, money("14.70", Currency::Usd));
}

#[test]
fn follows_the_pages_until_a_short_one() {
    let s = setup();
    s.serve_nothing(&window());
    s.serve(2, &window(), "page-1.json");
    s.server
        .serve(&path(2, &window(), 2), 200, fixture("page-2.json"));

    let conversions = s.monetizze.conversions(window()).unwrap();

    assert_eq!(conversions.len(), 103);
    let pages: Vec<String> = s
        .server
        .requests()
        .into_iter()
        .filter(|path| path.contains("status%5B%5D=2&"))
        .collect();
    assert_eq!(pages, [path(2, &window(), 1), path(2, &window(), 2)]);
}

#[test]
fn a_page_that_repeats_the_last_one_ends_the_reading() {
    let s = setup();
    s.serve_nothing(&window());
    s.serve(2, &window(), "page-1.json");
    s.server
        .serve(&path(2, &window(), 2), 200, fixture("page-1.json"));

    let conversions = s.monetizze.conversions(window()).unwrap();

    assert_eq!(conversions.len(), 100);
    assert!(!s.server.requests().contains(&path(2, &window(), 3)));
}

#[test]
fn an_error_in_the_answer_is_a_refusal() {
    let s = setup();
    s.serve_nothing(&window());
    s.serve(1, &window(), "error-in-answer.json");

    let failed = s.monetizze.conversions(window()).unwrap_err();

    assert_eq!(failed, PlatformError::Refused("Data inválida".into()));
}

#[test]
fn a_rate_limit_waits_longer_each_time_then_goes_on() {
    let s = setup();
    s.serve_nothing(&window());
    let first = path(1, &window(), 1);
    s.server.serve(&first, 429, fixture("error-429.json"));
    s.server.then_serve(&first, 429, fixture("error-429.json"));
    s.server.then_serve(&first, 200, fixture("empty.json"));

    s.monetizze.conversions(window()).unwrap();

    assert_eq!(s.pauses.seconds(), [5, 10]);
}

#[test]
fn a_rate_limit_that_never_lifts_gives_up_after_four_retries() {
    let s = setup();
    s.serve_nothing(&window());
    s.server
        .serve(&path(1, &window(), 1), 429, fixture("error-429.json"));

    let failed = s.monetizze.conversions(window()).unwrap_err();

    assert_eq!(failed, PlatformError::RateLimited);
    assert_eq!(s.pauses.seconds(), [5, 10, 20, 40]);
}

#[test]
fn a_server_error_is_tried_again() {
    let s = setup();
    s.serve_nothing(&window());
    let first = path(1, &window(), 1);
    s.server.serve(&first, 503, "");
    s.server.then_serve(&first, 200, fixture("empty.json"));

    s.monetizze.conversions(window()).unwrap();

    assert_eq!(s.pauses.seconds(), [5]);
}

#[test]
fn a_lapsed_token_is_renewed_once_and_the_reading_goes_on() {
    let s = setup();
    s.serve_nothing(&window());
    let first = path(1, &window(), 1);
    s.server
        .serve(&first, 403, fixture("error-invalid-token.json"));
    s.server.then_serve(&first, 200, fixture("empty.json"));

    s.monetizze.conversions(window()).unwrap();

    assert_eq!(s.token_requests(), 2);
}

#[test]
fn a_token_turned_down_twice_counts_as_a_key_not_accepted() {
    let s = setup();
    s.serve_nothing(&window());
    s.server.serve(
        &path(1, &window(), 1),
        403,
        fixture("error-invalid-token.json"),
    );

    let failed = s.monetizze.conversions(window()).unwrap_err();

    assert_eq!(failed, PlatformError::Expired);
    assert_eq!(s.token_requests(), 2);
}

#[test]
fn an_invalid_key_counts_as_not_accepted() {
    let s = setup();
    s.serve_nothing(&window());
    s.server.serve(
        &path(1, &window(), 1),
        403,
        fixture("error-invalid-key.json"),
    );

    let failed = s.monetizze.conversions(window()).unwrap_err();

    assert_eq!(failed, PlatformError::Expired);
}

#[test]
fn a_key_monetizze_does_not_accept_asks_nothing_else() {
    let s = setup();
    s.server.serve("/token", 403, fixture("token-refused.json"));

    let failed = s.monetizze.conversions(window()).unwrap_err();

    assert_eq!(failed, PlatformError::Expired);
    assert_eq!(s.server.requests(), ["/token"]);
}

#[test]
fn a_key_without_permission_is_refused_in_monetizzes_words() {
    let s = setup();
    s.serve_nothing(&window());
    s.server.serve(
        &path(1, &window(), 1),
        403,
        fixture("error-access-denied.json"),
    );

    let failed = s.monetizze.conversions(window()).unwrap_err();

    assert_eq!(failed, PlatformError::Refused("001-Acesso negado!".into()));
    assert_eq!(s.token_requests(), 1);
}

#[test]
fn the_token_lives_while_used_and_is_renewed_after_sitting_idle() {
    let s = setup();
    s.serve_nothing(&window());

    s.monetizze.conversions(window()).unwrap();
    // Each use keeps it alive for another 15 minutes.
    for _ in 0..3 {
        s.clock.advance(TimeDelta::minutes(13));
        s.monetizze.conversions(window()).unwrap();
    }
    assert_eq!(s.token_requests(), 1);

    // An hour later, as the next Sync comes, it has lapsed.
    s.clock.advance(TimeDelta::hours(1));
    s.monetizze.conversions(window()).unwrap();
    assert_eq!(s.token_requests(), 2);
}

#[test]
fn two_calls_racing_for_a_token_ask_for_it_once() {
    let s = setup();
    s.serve_nothing(&window());

    let racing: Vec<_> = (0..2)
        .map(|_| {
            let monetizze = s.monetizze.clone();
            std::thread::spawn(move || monetizze.conversions(window()))
        })
        .collect();
    for call in racing {
        call.join().unwrap().unwrap();
    }

    assert_eq!(s.token_requests(), 1);
}

#[test]
fn without_the_key_or_the_email_nothing_is_asked() {
    for missing in ["MONETIZZE_CONSUMER_KEY", "MONETIZZE_EMAIL"] {
        let s = setup();
        s.store.remove(missing).unwrap();

        let failed = s.monetizze.conversions(window()).unwrap_err();

        assert_eq!(failed, PlatformError::NotConnected);
        assert!(s.server.requests().is_empty());
    }
}

#[test]
fn monetizze_declares_its_platform_reach_and_settle_time() {
    let s = setup();

    assert_eq!(s.monetizze.platform(), AffiliatePlatform::Monetizze);
    // The longest guarantee, 30 days, and the days a boleto takes.
    assert_eq!(s.monetizze.settle_time(), TimeDelta::days(35));
    assert!(s.monetizze.history() >= TimeDelta::days(365));
}

/// The Affiliate module over a real temporary database, synced from
/// Monetizze.
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
    // The first Sync asks everything Monetizze lets the app read.
    let first = s.clock.now() - s.monetizze.history()..s.clock.now();
    s.serve_every_status(&first);

    let sync = block_on(synced.affiliate.sync(s.monetizze.as_ref())).unwrap();

    assert_eq!(sync.window, first);
    assert_eq!(
        sync.recorded,
        Recorded {
            added: 6,
            changed: 0,
            unchanged: 0
        }
    );
    // The next one rereads what can still change, the same sales again.
    let settling = s.clock.now() - TimeDelta::days(35)..s.clock.now();
    s.serve_every_status(&settling);
    let again = block_on(synced.affiliate.sync(s.monetizze.as_ref())).unwrap();
    assert_eq!(again.window, settling);
    assert_eq!(
        again.recorded,
        Recorded {
            added: 0,
            changed: 0,
            unchanged: 6
        }
    );
    let commissions = block_on(synced.affiliate.commissions(&CommissionFilter::default())).unwrap();
    assert_eq!(commissions.len(), 6);
    assert!(
        commissions
            .iter()
            .all(|commission| commission.amount == money("78.80", Currency::Brl))
    );
}

#[test]
fn a_returned_sale_reverses_the_same_commission() {
    let s = setup();
    let synced = affiliate(&s);
    let first = s.clock.now() - s.monetizze.history()..s.clock.now();
    s.serve_nothing(&first);
    s.serve(2, &first, "status-2.json");
    block_on(synced.affiliate.sync(s.monetizze.as_ref())).unwrap();

    // An hour later the buyer returned it.
    s.clock.advance(TimeDelta::hours(1));
    let next = s.clock.now() - TimeDelta::days(35) - TimeDelta::hours(1)..s.clock.now();
    s.serve_nothing(&next);
    s.serve(4, &next, "returned.json");
    let sync = block_on(synced.affiliate.sync(s.monetizze.as_ref())).unwrap();

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
    assert_eq!(commissions.len(), 1);
    let returned = &commissions[0];
    assert_eq!(returned.status, CommissionStatus::Reversed);
    assert_eq!(returned.platform_status, "Devolvida");
    assert_eq!(returned.amount, money("78.80", Currency::Brl));
    let history = block_on(synced.affiliate.status_history(returned.id)).unwrap();
    let statuses: Vec<CommissionStatus> = history.iter().map(|change| change.status).collect();
    assert_eq!(
        statuses,
        [CommissionStatus::Approved, CommissionStatus::Reversed]
    );
}
