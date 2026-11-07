//! The Eduzz affiliate market reader against a fake DevTools endpoint
//! answering with what the reading script would read on the Vitrine (see
//! the fixtures' README), with an injected clock and pause: one listing
//! page per click, the interval, the hour of memory and the day-long stop.
//! Then the Affiliate module over it, with a real temporary database: a
//! second read of the same Vitrine changes nothing.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliatePlatform, Affiliation, FoundProspect, ProspectCategory, ProspectSource,
    SignalKind, SignalOrigin, Signals,
};
use mascate_integrations::{
    AffiliateMarket, BrowserSession, EDUZZ_AFFILIATE_MARKET, EDUZZ_AFFILIATE_MARKET_URL,
    EduzzAffiliateMarket, FLAGS, MARKET_INTERVAL, MARKET_STOPPED_FOR, MarketStop, MarketStopped,
    PanelState, eduzz_affiliation_page,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, Percentage, PlatformError, Timestamp};
use mascate_platform::{Database, FlagKind, Phase, migrate};
use rust_decimal::Decimal;
use serde_json::Value;

mod browsing;

use browsing::{ClockPause, FakeDevTools, RecordedPauses};

const MARKET: &str = "https://my2.eduzz.com/showcase";
const MENTORING: &str = "2154873";
const GUITAR: &str = "1987620";
const BOOKS: &str = "2203311";
const PLANNER: &str = "2099045";

fn fixture(name: &str) -> Value {
    browsing::fixture("eduzz-affiliate-market", name)
}

fn start() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 11, 7, 19, 30, 0).unwrap()
}

fn brl(amount: &str) -> Option<Money> {
    Some(Money::new(
        Decimal::from_str(amount).unwrap(),
        Currency::Brl,
    ))
}

fn percent(value: &str) -> Option<Percentage> {
    Some(Percentage::new(Decimal::from_str(value).unwrap()).unwrap())
}

fn page_of(id: &str) -> String {
    format!("{MARKET}/product/details/{id}")
}

struct Setup {
    devtools: FakeDevTools,
    clock: Arc<ManualClock>,
    pause: Arc<ClockPause>,
    market: EduzzAffiliateMarket,
    profile: tempfile::TempDir,
}

/// The Vitrine through a browser at the fake endpoint, on a profile the
/// owner already opened.
fn setup() -> Setup {
    let profile = tempfile::tempdir().unwrap();
    let devtools = FakeDevTools::start();
    let clock = Arc::new(ManualClock::at(start()));
    let pause = Arc::new(ClockPause::on(clock.clone()));
    let market = market_on(&devtools, profile.path(), &clock, &pause);
    Setup {
        devtools,
        clock,
        pause,
        market,
        profile,
    }
}

fn market_on(
    devtools: &FakeDevTools,
    profile: &std::path::Path,
    clock: &Arc<ManualClock>,
    pause: &Arc<ClockPause>,
) -> EduzzAffiliateMarket {
    let browser = BrowserSession::attached(devtools.endpoint(), profile.join("navegador"))
        .with_pause(Arc::new(RecordedPauses::default()));
    std::fs::create_dir_all(profile.join("navegador")).unwrap();
    EduzzAffiliateMarket::new(MARKET, Arc::new(browser), clock.clone(), pause.clone())
}

fn known(key: &str, affiliation_page: Option<String>) -> FoundProspect {
    FoundProspect {
        platform: AffiliatePlatform::Eduzz,
        key: key.into(),
        name: "Kept name".into(),
        signals: Signals {
            affiliation_page,
            ..Signals::default()
        },
    }
}

#[test]
fn the_reader_is_a_restricted_feature_of_phase_3_with_eduzzs_terms_its_limits_and_risk() {
    assert!(FLAGS.contains(&EDUZZ_AFFILIATE_MARKET));
    assert_eq!(
        EDUZZ_AFFILIATE_MARKET.key,
        "integrations.eduzz_affiliate_market"
    );
    assert_eq!(EDUZZ_AFFILIATE_MARKET.kind, FlagKind::RestrictedFeature);
    assert_eq!(EDUZZ_AFFILIATE_MARKET.phase, Phase::Three);
    for limit in [
        "Penalidades",
        "Direitos Autorais",
        "a primeira página da Vitrine",
        "no máximo 5 páginas",
        "5 segundos",
        "1 hora",
        "não pede afiliação",
        "24 horas",
    ] {
        assert!(EDUZZ_AFFILIATE_MARKET.reason.contains(limit), "{limit}");
    }
    assert!(EDUZZ_AFFILIATE_MARKET.risk.contains("bloquear a sua conta"));
    assert!(EDUZZ_AFFILIATE_MARKET.risk.contains("reter o saldo"));
    assert_eq!(EDUZZ_AFFILIATE_MARKET_URL, MARKET);
}

#[test]
fn each_card_is_a_prospect_with_its_commission_per_sale_and_affiliation() {
    let s = setup();
    s.devtools.then_read(fixture("drawing.json"));
    s.devtools.then_read(fixture("listing.json"));

    let found = s.market.prospects(None).unwrap();

    let mentoring = FoundProspect {
        platform: AffiliatePlatform::Eduzz,
        key: MENTORING.into(),
        name: "Mentoria Finanças em Família".into(),
        signals: Signals {
            commission: brl("98.50"),
            affiliation_page: Some(page_of(MENTORING)),
            ..Signals::default()
        },
    };
    let guitar = FoundProspect {
        platform: AffiliatePlatform::Eduzz,
        key: GUITAR.into(),
        name: "Curso de Violão do Zero".into(),
        signals: Signals {
            commission_rate: percent("40"),
            affiliation: Some(Affiliation::OnRequest),
            affiliation_page: Some(page_of(GUITAR)),
            ..Signals::default()
        },
    };
    let books = FoundProspect {
        platform: AffiliatePlatform::Eduzz,
        key: BOOKS.into(),
        name: "Clube do Livro Infantil".into(),
        signals: Signals {
            commission: brl("19.90"),
            affiliation_page: Some(page_of(BOOKS)),
            ..Signals::default()
        },
    };
    let planner = FoundProspect {
        platform: AffiliatePlatform::Eduzz,
        key: PLANNER.into(),
        name: "Planner Digital".into(),
        signals: Signals {
            affiliation_page: Some(page_of(PLANNER)),
            ..Signals::default()
        },
    };
    assert_eq!(found, [mentoring, guitar, books, planner]);
    assert_eq!(s.devtools.navigations(), [MARKET]);
}

#[test]
fn one_click_reads_only_the_first_page_and_the_same_hour_answers_from_memory() {
    let s = setup();
    s.devtools.then_read(fixture("listing.json"));

    let first = s.market.prospects(None).unwrap();
    s.clock.advance(TimeDelta::minutes(30));
    let again = s.market.prospects(None).unwrap();
    s.clock.advance(TimeDelta::minutes(31));
    s.market.prospects(None).unwrap();

    assert_eq!(again, first);
    assert_eq!(s.devtools.navigations(), [MARKET, MARKET]);
    assert!(s.pause.waits().is_empty());
}

#[test]
fn eduzz_names_no_category_so_none_goes_into_an_address() {
    let s = setup();
    let odd = ProspectCategory {
        id: "x&page=99".into(),
        name: "Finanças".into(),
    };

    assert_eq!(s.market.categories().unwrap(), []);
    assert!(matches!(
        s.market.prospects(Some(&odd)),
        Err(PlatformError::Refused(_))
    ));
    assert!(s.devtools.tab_requests().is_empty());
}

#[test]
fn a_products_own_page_adds_price_rating_warranty_and_producer_and_waits_its_turn() {
    let s = setup();
    s.devtools.then_read(fixture("listing.json"));
    s.market.prospects(None).unwrap();
    s.clock.advance(TimeDelta::seconds(1));
    s.devtools.then_read(fixture("details.json"));

    let read = s
        .market
        .details(&known(MENTORING, Some(page_of(MENTORING))))
        .unwrap();

    assert_eq!(
        read,
        FoundProspect {
            platform: AffiliatePlatform::Eduzz,
            key: MENTORING.into(),
            name: "Mentoria Finanças em Família".into(),
            signals: Signals {
                price: brl("197.00"),
                commission: brl("108.35"),
                commission_rate: percent("55"),
                rating: Some(Decimal::from_str("4.6").unwrap()),
                reviews: Some(23),
                warranty_days: Some(7),
                creator: Some("Ana Prado".into()),
                affiliation: Some(Affiliation::Immediate),
                affiliation_page: Some(page_of(MENTORING)),
                ..Signals::default()
            },
        }
    );
    assert_eq!(s.devtools.navigations(), [MARKET, &page_of(MENTORING)]);
    assert_eq!(s.pause.waits(), [MARKET_INTERVAL - Duration::from_secs(1)]);
}

#[test]
fn a_product_without_its_vitrine_address_is_refused_without_opening_anything() {
    let s = setup();

    let without = s.market.details(&known(MENTORING, None));
    let odd = s.market.details(&known(
        MENTORING,
        Some("https://evil.example/showcase/product/details/<x>".into()),
    ));
    let elsewhere = s.market.details(&known(
        MENTORING,
        Some("https://my2.eduzz.com/producer/sales".into()),
    ));

    assert!(matches!(without, Err(PlatformError::Refused(_))));
    assert!(matches!(odd, Err(PlatformError::Refused(_))));
    assert!(matches!(elsewhere, Err(PlatformError::Refused(_))));
    assert!(s.devtools.tab_requests().is_empty());
}

#[test]
fn a_login_stops_the_reader_for_a_day_until_the_owner_logs_in_again() {
    let s = setup();
    s.devtools.then_read(fixture("login.json"));

    let read = s.market.prospects(None);
    let while_stopped = s
        .market
        .details(&known(MENTORING, Some(page_of(MENTORING))));

    assert!(matches!(read, Err(PlatformError::Expired)));
    assert!(matches!(while_stopped, Err(PlatformError::Expired)));
    assert_eq!(s.devtools.navigations().len(), 1);
    assert_eq!(s.market.state(), PanelState::LoggedOut);
    assert_eq!(
        s.market.stopped(),
        Some(MarketStopped {
            why: MarketStop::Login,
            until: start() + MARKET_STOPPED_FOR,
        })
    );
    assert_eq!(s.market.blocked_until(), Some(start() + MARKET_STOPPED_FOR));

    s.market.open_login().unwrap();

    assert_eq!(s.market.stopped(), None);
    assert_eq!(s.market.state(), PanelState::Ready);
}

#[test]
fn a_robot_check_names_eduzz_and_a_block_waits_its_day_even_after_a_restart() {
    let s = setup();
    s.devtools.then_read(fixture("captcha.json"));
    let check = s.market.prospects(None);

    let Err(PlatformError::Refused(why)) = check else {
        panic!("{check:?}");
    };
    assert!(
        why.contains("a Eduzz pediu a verificação contra robôs"),
        "{why}"
    );

    let file = s.profile.path().join("eduzz-market-stop");
    let market =
        market_on(&s.devtools, s.profile.path(), &s.clock, &s.pause).with_stop_file(file.clone());
    s.devtools.then_read(fixture("blocked.json"));
    assert!(matches!(
        market.prospects(None),
        Err(PlatformError::RateLimited)
    ));
    market.open_login().unwrap();
    let restarted =
        market_on(&s.devtools, s.profile.path(), &s.clock, &s.pause).with_stop_file(file.clone());

    assert_eq!(
        restarted.stopped().map(|stopped| stopped.why),
        Some(MarketStop::Block)
    );
    s.clock.advance(MARKET_STOPPED_FOR);
    s.devtools.then_read(fixture("listing.json"));
    assert_eq!(restarted.prospects(None).unwrap().len(), 4);
}

#[test]
fn without_the_owners_login_in_the_profile_nothing_opens() {
    let devtools = FakeDevTools::start();
    let profile = tempfile::tempdir().unwrap();
    let browser = BrowserSession::attached(devtools.endpoint(), profile.path().join("nunca"));
    let market = EduzzAffiliateMarket::new(
        MARKET,
        Arc::new(browser),
        Arc::new(ManualClock::at(start())),
        Arc::new(RecordedPauses::default()),
    );

    assert_eq!(market.state(), PanelState::NeverLoggedIn);
    assert!(matches!(
        market.prospects(None),
        Err(PlatformError::NotConnected)
    ));
    assert!(devtools.tab_requests().is_empty());
}

#[test]
fn a_products_affiliation_page_is_its_vitrine_page() {
    assert_eq!(eduzz_affiliation_page(MENTORING), Some(page_of(MENTORING)));
    assert_eq!(eduzz_affiliation_page("x/../y"), None);
    assert_eq!(eduzz_affiliation_page(""), None);
}

#[test]
fn reading_the_vitrine_twice_keeps_one_prospect_per_product_with_market_signals() {
    block_on(async {
        let s = setup();
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&dir.path().join("m.db")).await.unwrap());
        migrate(
            &database,
            s.clock.as_ref(),
            &[mascate_affiliate::MIGRATIONS],
        )
        .await
        .unwrap();
        let affiliate = Affiliate::new(
            database,
            s.clock.clone(),
            Arc::new(SequentialIds::default()),
        );
        s.devtools.then_read(fixture("listing.json"));

        let first = affiliate.read_prospects(&s.market, None).await.unwrap();
        s.clock.advance(TimeDelta::hours(2));
        let again = affiliate.read_prospects(&s.market, None).await.unwrap();
        let mentoring = affiliate
            .prospects(Some(AffiliatePlatform::Eduzz))
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.key == MENTORING)
            .unwrap();
        s.devtools.then_read(fixture("details.json"));
        let read = affiliate
            .read_prospect_details(&s.market, mentoring.id)
            .await
            .unwrap();

        assert_eq!((first.found, first.added), (4, 4));
        assert_eq!((again.found, again.added), (4, 0));
        assert_eq!(
            affiliate
                .prospects(Some(AffiliatePlatform::Eduzz))
                .await
                .unwrap()
                .len(),
            4
        );
        assert_eq!(
            mentoring.reading(SignalKind::Commission).unwrap().origin,
            SignalOrigin::Market
        );
        assert_eq!(mentoring.signals.affiliation_page, Some(page_of(MENTORING)));
        assert_eq!(
            (read.id, read.signals.commission, read.signals.rating),
            (
                mentoring.id,
                brl("108.35"),
                Some(Decimal::from_str("4.6").unwrap())
            )
        );
        assert_eq!(read.signals.creator.as_deref(), Some("Ana Prado"));
    });
}
