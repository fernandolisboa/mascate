//! The Hotmart affiliate market reader against a fake DevTools endpoint
//! answering with what the reading script would read on the market (see the
//! fixtures' README), with an injected clock and pause: the pages per
//! click, the interval, the hour of memory and the day-long stop. Then the
//! Affiliate module over it and the vitrine, with a real temporary
//! database: one product is one Prospect whichever reader saw it first.

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliatePlatform, Affiliation, FoundProspect, ProspectCategory, ProspectSource,
    SignalKind, SignalOrigin, Signals,
};
use mascate_integrations::{
    BrowserSession, FLAGS, HOTMART_AFFILIATE_MARKET, HOTMART_MARKET_INTERVAL,
    HOTMART_MARKET_PAGES_PER_CLICK, HOTMART_MARKET_STOPPED_FOR, HotmartAffiliateMarket,
    HotmartCrawler, MarketStop, MarketStopped, PanelState, hotmart_affiliation_page,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, Pause, Percentage, PlatformError, Timestamp};
use mascate_platform::{Database, FlagKind, Phase, migrate};
use rust_decimal::Decimal;
use serde_json::{Value, json};

mod browsing;
mod crawling;

use browsing::{FakeDevTools, RecordedPauses};

const MARKET: &str = "https://app.hotmart.com/market";
const CAKES: &str = "8a1c3e52-0f6d-4b8e-9c21-5d7a0e4f1b93";
const ENGLISH: &str = "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";
const PHOTO: &str = "0e1f2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b";

fn fixture(name: &str) -> Value {
    browsing::fixture("hotmart-affiliate-market", name)
}

fn start() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 11, 7, 15, 0, 0).unwrap()
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

fn page_of(ucode: &str) -> String {
    format!("{MARKET}/details?productUcode={ucode}")
}

/// A wait that moves the clock on, as a real one would, and is remembered.
struct ClockPause {
    clock: Arc<ManualClock>,
    waits: Mutex<Vec<Duration>>,
}

impl Pause for ClockPause {
    fn pause(&self, duration: Duration) {
        self.waits.lock().unwrap().push(duration);
        self.clock
            .advance(TimeDelta::from_std(duration).unwrap_or_default());
    }
}

impl ClockPause {
    fn waits(&self) -> Vec<Duration> {
        self.waits.lock().unwrap().clone()
    }
}

struct Setup {
    devtools: FakeDevTools,
    clock: Arc<ManualClock>,
    pause: Arc<ClockPause>,
    market: HotmartAffiliateMarket,
    profile: tempfile::TempDir,
}

/// The market through a browser at the fake endpoint, on a profile the
/// owner already opened.
fn setup() -> Setup {
    let profile = tempfile::tempdir().unwrap();
    let devtools = FakeDevTools::start();
    let clock = Arc::new(ManualClock::at(start()));
    let pause = Arc::new(ClockPause {
        clock: clock.clone(),
        waits: Mutex::new(Vec::new()),
    });
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
) -> HotmartAffiliateMarket {
    let browser = BrowserSession::attached(devtools.endpoint(), profile.join("navegador"))
        .with_pause(Arc::new(RecordedPauses::default()));
    std::fs::create_dir_all(profile.join("navegador")).unwrap();
    HotmartAffiliateMarket::new(MARKET, Arc::new(browser), clock.clone(), pause.clone())
}

/// A listing page with one product per code in `ucodes`.
fn listing(page: usize, ucodes: &[String]) -> Value {
    json!({
        "url": format!("{MARKET}?page={page}"),
        "cards": ucodes.iter().map(|ucode| json!({
            "link": page_of(ucode),
            "title": format!("Produto {ucode}"),
            "lines": ["Temperatura", "40°"],
        })).collect::<Vec<_>>(),
    })
}

fn known(key: &str, affiliation_page: Option<String>) -> FoundProspect {
    FoundProspect {
        platform: AffiliatePlatform::Hotmart,
        key: key.into(),
        name: "Kept name".into(),
        signals: Signals {
            affiliation_page,
            ..Signals::default()
        },
    }
}

#[test]
fn the_reader_is_a_restricted_feature_of_phase_3_with_its_limits_and_risk() {
    assert!(FLAGS.contains(&HOTMART_AFFILIATE_MARKET));
    assert_eq!(
        HOTMART_AFFILIATE_MARKET.key,
        "integrations.hotmart_affiliate_market"
    );
    assert_eq!(HOTMART_AFFILIATE_MARKET.kind, FlagKind::RestrictedFeature);
    assert_eq!(HOTMART_AFFILIATE_MARKET.phase, Phase::Three);
    for limit in [
        "3.5(m)",
        "no máximo 5 páginas",
        "5 segundos",
        "1 hora",
        "não pede afiliação",
        "24 horas",
    ] {
        assert!(HOTMART_AFFILIATE_MARKET.reason.contains(limit), "{limit}");
    }
    assert!(HOTMART_AFFILIATE_MARKET.risk.contains("conta de afiliado"));
    assert_eq!(HOTMART_MARKET_PAGES_PER_CLICK, 5);
    assert_eq!(HOTMART_MARKET_INTERVAL, Duration::from_secs(5));
    assert_eq!(HOTMART_MARKET_STOPPED_FOR, TimeDelta::hours(24));
}

#[test]
fn each_card_is_a_prospect_with_the_signals_only_the_market_shows() {
    let s = setup();
    s.devtools.then_read(fixture("drawing.json"));
    s.devtools.then_read(fixture("listing.json"));
    s.devtools.then_read(fixture("listing-page-2.json"));
    s.devtools.then_read(fixture("listing-page-3.json"));

    let found = s.market.prospects(None).unwrap();

    let cakes = FoundProspect {
        platform: AffiliatePlatform::Hotmart,
        key: CAKES.into(),
        name: "Escola de Bolos 3 em 1".into(),
        signals: Signals {
            temperature: Some(87),
            commission: brl("148.50"),
            commission_rate: percent("50"),
            price: brl("297.00"),
            blueprint: percent("85"),
            satisfaction: percent("92"),
            affiliation: Some(Affiliation::Immediate),
            affiliation_page: Some(page_of(CAKES)),
            ..Signals::default()
        },
    };
    let excel = FoundProspect {
        platform: AffiliatePlatform::Hotmart,
        key: "4021".into(),
        name: "Excel do Zero ao Avançado".into(),
        signals: Signals {
            temperature: Some(12),
            commission: Some(Money::new(Decimal::from(30), Currency::Usd)),
            commission_rate: percent("40"),
            blueprint: percent("92"),
            satisfaction: percent("97"),
            affiliation: Some(Affiliation::OnRequest),
            affiliation_page: Some(page_of("c4d5e6f7-8091-4a2b-b3c4-d5e6f7a8b9c0")),
            ..Signals::default()
        },
    };
    let photo = FoundProspect {
        platform: AffiliatePlatform::Hotmart,
        key: PHOTO.into(),
        name: "Fotografia com Celular".into(),
        signals: Signals {
            affiliation_page: Some(page_of(PHOTO)),
            ..Signals::default()
        },
    };
    let english = FoundProspect {
        platform: AffiliatePlatform::Hotmart,
        key: ENGLISH.into(),
        name: "Inglês em 90 Dias".into(),
        signals: Signals {
            temperature: Some(3),
            commission: brl("59.40"),
            commission_rate: percent("60"),
            affiliation: Some(Affiliation::Immediate),
            affiliation_page: Some(page_of(ENGLISH)),
            ..Signals::default()
        },
    };
    assert_eq!(found, [cakes, excel, photo, english]);
    assert_eq!(
        s.devtools.navigations(),
        [
            format!("{MARKET}?page=1"),
            format!("{MARKET}?page=2"),
            format!("{MARKET}?page=3"),
        ]
    );
}

#[test]
fn one_click_reads_at_most_five_pages_five_seconds_apart() {
    let s = setup();
    for page in 1..=7 {
        let ucodes: Vec<String> = (0..3).map(|n| format!("p{page}-{n}")).collect();
        s.devtools.then_read(listing(page, &ucodes));
    }

    let found = s.market.prospects(None).unwrap();

    assert_eq!(found.len(), 15);
    assert_eq!(
        s.devtools.navigations().len(),
        HOTMART_MARKET_PAGES_PER_CLICK
    );
    assert_eq!(s.pause.waits(), vec![HOTMART_MARKET_INTERVAL; 4]);
}

#[test]
fn a_page_read_in_the_last_hour_comes_from_memory_and_the_next_one_waits_its_turn() {
    let s = setup();
    s.devtools.then_read(fixture("details.json"));
    let cakes = known("1418001", Some(page_of(CAKES)));

    let first = s.market.details(&cakes).unwrap();
    s.clock.advance(TimeDelta::seconds(2));
    let again = s.market.details(&cakes).unwrap();
    s.market
        .details(&known(ENGLISH, Some(page_of(ENGLISH))))
        .unwrap();
    s.clock.advance(TimeDelta::hours(1));
    s.market.details(&cakes).unwrap();

    assert_eq!(again, first);
    assert_eq!(
        s.devtools.navigations(),
        [page_of(CAKES), page_of(ENGLISH), page_of(CAKES)]
    );
    assert_eq!(s.pause.waits(), [Duration::from_secs(3)]);
}

#[test]
fn a_products_own_page_adds_the_producer_and_hotmarts_product_id() {
    let s = setup();
    s.devtools.then_read(fixture("details.json"));

    let read = s
        .market
        .details(&known(CAKES, Some(page_of(CAKES))))
        .unwrap();

    assert_eq!(
        read,
        FoundProspect {
            platform: AffiliatePlatform::Hotmart,
            key: "1418001".into(),
            name: "Escola de Bolos 3 em 1".into(),
            signals: Signals {
                temperature: Some(91),
                commission: brl("148.50"),
                commission_rate: percent("50"),
                price: brl("297.00"),
                blueprint: percent("85"),
                satisfaction: percent("96"),
                affiliation: Some(Affiliation::Immediate),
                creator: Some("Ana Confeiteira".into()),
                affiliation_page: Some(page_of(CAKES)),
                ..Signals::default()
            },
        }
    );
}

#[test]
fn a_product_without_its_market_address_is_refused_without_opening_anything() {
    let s = setup();
    let pasted = known(
        "4021",
        Some("https://evil.example/?productUcode=<x>".into()),
    );

    let without = s.market.details(&known("4021", None));
    let odd = s.market.details(&pasted);

    assert!(matches!(without, Err(PlatformError::Refused(_))));
    assert!(matches!(odd, Err(PlatformError::Refused(_))));
    assert!(s.devtools.tab_requests().is_empty());
}

#[test]
fn a_category_outside_hotmarts_list_never_goes_into_an_address() {
    let s = setup();
    let odd = ProspectCategory {
        id: "x&page=99".into(),
        name: String::new(),
    };
    s.devtools.then_read(listing(1, &[]));

    assert!(matches!(
        s.market.prospects(Some(&odd)),
        Err(PlatformError::Refused(_))
    ));
    let cooking = s.market.categories().unwrap().into_iter().nth(3).unwrap();
    s.market.prospects(Some(&cooking)).unwrap();
    assert_eq!(
        s.devtools.navigations(),
        [format!("{MARKET}?category={}&page=1", cooking.id)]
    );
}

#[test]
fn a_login_stops_the_reader_for_a_day_until_the_owner_logs_in_again() {
    let s = setup();
    s.devtools.then_read(fixture("login.json"));

    let read = s.market.prospects(None);
    let while_stopped = s.market.details(&known(CAKES, Some(page_of(CAKES))));

    assert!(matches!(read, Err(PlatformError::Expired)));
    assert!(matches!(while_stopped, Err(PlatformError::Expired)));
    assert_eq!(s.devtools.navigations().len(), 1);
    assert_eq!(s.market.state(), PanelState::LoggedOut);
    assert_eq!(
        s.market.stopped(),
        Some(MarketStopped {
            why: MarketStop::Login,
            until: start() + TimeDelta::hours(24),
        })
    );
    assert_eq!(
        s.market.blocked_until(),
        Some(start() + TimeDelta::hours(24))
    );

    s.market.open_login().unwrap();

    assert_eq!(s.market.stopped(), None);
    assert_eq!(s.market.state(), PanelState::Ready);
}

#[test]
fn a_robot_check_or_a_block_stops_the_reader_and_only_the_day_lifts_a_block() {
    for (page, why) in [
        ("captcha.json", MarketStop::Check),
        ("blocked.json", MarketStop::Block),
    ] {
        let s = setup();
        s.devtools.then_read(fixture(page));

        let read = s.market.prospects(None);
        s.market.open_login().unwrap();
        let after_login = s.market.stopped().map(|stopped| stopped.why);
        s.clock.advance(TimeDelta::hours(24));
        s.devtools.then_read(fixture("listing.json"));
        let next_day = s.market.prospects(None);

        assert!(read.is_err(), "{page}");
        match why {
            MarketStop::Check => assert_eq!(after_login, None),
            _ => assert_eq!(after_login, Some(MarketStop::Block)),
        }
        assert!(next_day.is_ok(), "{page}");
    }
}

#[test]
fn a_stop_after_the_first_page_keeps_what_was_read() {
    let s = setup();
    s.devtools.then_read(fixture("listing.json"));
    s.devtools.then_read(fixture("blocked.json"));

    let found = s.market.prospects(None).unwrap();

    assert_eq!(found.len(), 3);
    assert_eq!(
        s.market.stopped().map(|stopped| stopped.why),
        Some(MarketStop::Block)
    );
}

#[test]
fn a_stop_outlasts_a_restart_of_the_app() {
    let s = setup();
    let file = s.profile.path().join("hotmart-market-stop");
    let market =
        market_on(&s.devtools, s.profile.path(), &s.clock, &s.pause).with_stop_file(file.clone());
    s.devtools.then_read(fixture("blocked.json"));
    assert!(market.prospects(None).is_err());

    let restarted =
        market_on(&s.devtools, s.profile.path(), &s.clock, &s.pause).with_stop_file(file.clone());
    let read = restarted.prospects(None);

    assert!(matches!(read, Err(PlatformError::RateLimited)));
    assert_eq!(s.devtools.navigations().len(), 1);
    assert_eq!(
        restarted.stopped(),
        Some(MarketStopped {
            why: MarketStop::Block,
            until: start() + TimeDelta::hours(24),
        })
    );
}

#[test]
fn without_the_owners_login_in_the_profile_nothing_opens() {
    let devtools = FakeDevTools::start();
    let profile = tempfile::tempdir().unwrap();
    let browser = BrowserSession::attached(devtools.endpoint(), profile.path().join("nunca"));
    let clock = Arc::new(ManualClock::at(start()));
    let market = HotmartAffiliateMarket::new(
        MARKET,
        Arc::new(browser),
        clock.clone(),
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
fn the_vitrine_and_the_market_name_a_products_affiliation_page_alike() {
    assert_eq!(hotmart_affiliation_page(CAKES), Some(page_of(CAKES)));
    assert_eq!(hotmart_affiliation_page("x&y=1"), None);
    assert_eq!(hotmart_affiliation_page(""), None);
}

#[test]
fn a_product_read_in_the_market_and_in_the_vitrine_is_one_prospect_with_both_sets_of_signals() {
    block_on(async {
        let s = setup();
        let site = crawling::site(crawling::fixture("hotmart-crawler", "robots.txt"));
        site.server.serve(
            "/pt-br/marketplace/categorias/culinaria-e-gastronomia",
            200,
            crawling::fixture("hotmart-crawler", "category.html"),
        );
        site.server.serve(
            "/pt-br/marketplace/produtos/escola-de-bolos-3-em-1/Q52300001P",
            200,
            crawling::fixture("hotmart-crawler", "product.html"),
        );
        let vitrine = HotmartCrawler::new(
            &format!("{}/pt-br/marketplace", site.server.url()),
            site.engine,
        );
        let cooking = vitrine.categories().unwrap().into_iter().nth(3).unwrap();
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
        s.devtools.then_read(fixture("listing-page-2.json"));
        s.devtools.then_read(fixture("listing-page-3.json"));

        affiliate.read_prospects(&s.market, None).await.unwrap();
        affiliate
            .read_prospects(&vitrine, Some(&cooking))
            .await
            .unwrap();
        let both = affiliate.prospects(None).await.unwrap().len();
        let from_vitrine = affiliate
            .prospects(None)
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.key == "1418001")
            .unwrap();
        let read = affiliate
            .read_prospect_details(&vitrine, from_vitrine.id)
            .await
            .unwrap();
        s.clock.advance(TimeDelta::hours(2));
        s.devtools.then_read(fixture("listing.json"));
        let again = affiliate.read_prospects(&s.market, None).await.unwrap();

        let cakes: Vec<_> = affiliate
            .prospects(None)
            .await
            .unwrap()
            .into_iter()
            .filter(|p| p.name.starts_with("Escola de Bolos"))
            .collect();
        assert_eq!(both, 8);
        assert_eq!(affiliate.prospects(None).await.unwrap().len(), 7);
        assert_eq!(cakes.len(), 1);
        let cakes = &cakes[0];
        assert_eq!((cakes.id, cakes.key.as_str()), (read.id, "1418001"));
        assert_eq!(again.added, 0);
        assert_eq!(cakes.signals.temperature, Some(87));
        assert_eq!(cakes.signals.students, Some(15_230));
        let origin = |kind| cakes.reading(kind).unwrap().origin;
        assert_eq!(origin(SignalKind::Temperature), SignalOrigin::Market);
        assert_eq!(origin(SignalKind::Blueprint), SignalOrigin::Market);
        assert_eq!(origin(SignalKind::Students), SignalOrigin::Vitrine);
        assert_eq!(origin(SignalKind::Rating), SignalOrigin::Vitrine);
        assert_eq!(cakes.signals.affiliation_page, Some(page_of(CAKES)));
    });
}
