//! The Kiwify affiliate market reader against a fake DevTools endpoint
//! answering with what the reading script would read on the Marketplace
//! (see the fixtures' README), with an injected clock and pause: one
//! listing page per click, the interval, the hour of memory and the
//! day-long stop. Then the Affiliate module over it, with a real temporary
//! database: a second read of the same Marketplace changes nothing.

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
    AffiliateMarket, BrowserSession, FLAGS, KIWIFY_AFFILIATE_MARKET, KIWIFY_AFFILIATE_MARKET_URL,
    KiwifyAffiliateMarket, MARKET_INTERVAL, MARKET_STOPPED_FOR, MarketStop, MarketStopped,
    PanelState, kiwify_affiliation_page,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, Percentage, PlatformError, Timestamp};
use mascate_platform::{Database, FlagKind, Phase, migrate};
use rust_decimal::Decimal;
use serde_json::Value;

mod browsing;

use browsing::{ClockPause, FakeDevTools, RecordedPauses};

const MARKET: &str = "https://dashboard.kiwify.com.br/marketplace";
const CAKES: &str = "Qx7Lm2Pa";
const SHEETS: &str = "Rt5Nv8Kd";
const YOGA: &str = "Hz3Wq9Bc";

fn fixture(name: &str) -> Value {
    browsing::fixture("kiwify-affiliate-market", name)
}

fn start() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 11, 7, 18, 30, 0).unwrap()
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

fn page_of(code: &str) -> String {
    format!("{MARKET}/{code}")
}

struct Setup {
    devtools: FakeDevTools,
    clock: Arc<ManualClock>,
    pause: Arc<ClockPause>,
    market: KiwifyAffiliateMarket,
    profile: tempfile::TempDir,
}

/// The Marketplace through a browser at the fake endpoint, on a profile
/// the owner already opened.
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
) -> KiwifyAffiliateMarket {
    let browser = BrowserSession::attached(devtools.endpoint(), profile.join("navegador"))
        .with_pause(Arc::new(RecordedPauses::default()));
    std::fs::create_dir_all(profile.join("navegador")).unwrap();
    KiwifyAffiliateMarket::new(MARKET, Arc::new(browser), clock.clone(), pause.clone())
}

fn known(key: &str, affiliation_page: Option<String>) -> FoundProspect {
    FoundProspect {
        platform: AffiliatePlatform::Kiwify,
        key: key.into(),
        name: "Kept name".into(),
        signals: Signals {
            affiliation_page,
            ..Signals::default()
        },
    }
}

#[test]
fn the_reader_is_a_restricted_feature_of_phase_3_with_kiwifys_terms_its_limits_and_risk() {
    assert!(FLAGS.contains(&KIWIFY_AFFILIATE_MARKET));
    assert_eq!(
        KIWIFY_AFFILIATE_MARKET.key,
        "integrations.kiwify_affiliate_market"
    );
    assert_eq!(KIWIFY_AFFILIATE_MARKET.kind, FlagKind::RestrictedFeature);
    assert_eq!(KIWIFY_AFFILIATE_MARKET.phase, Phase::Three);
    for limit in [
        "14.1",
        "15.1",
        "a primeira página da lista",
        "no máximo 5 páginas",
        "5 segundos",
        "1 hora",
        "não pede afiliação",
        "24 horas",
    ] {
        assert!(KIWIFY_AFFILIATE_MARKET.reason.contains(limit), "{limit}");
    }
    assert!(
        KIWIFY_AFFILIATE_MARKET
            .risk
            .contains("bloquear a sua conta")
    );
    assert_eq!(KIWIFY_AFFILIATE_MARKET_URL, MARKET);
}

#[test]
fn each_card_is_a_prospect_with_its_commission_price_and_affiliation() {
    let s = setup();
    s.devtools.then_read(fixture("drawing.json"));
    s.devtools.then_read(fixture("listing.json"));

    let found = s.market.prospects(None).unwrap();

    let cakes = FoundProspect {
        platform: AffiliatePlatform::Kiwify,
        key: CAKES.into(),
        name: "Método Confeitaria Lucrativa".into(),
        signals: Signals {
            price: brl("197.00"),
            commission: brl("98.50"),
            commission_rate: percent("50"),
            affiliation: Some(Affiliation::Immediate),
            affiliation_page: Some(page_of(CAKES)),
            ..Signals::default()
        },
    };
    let sheets = FoundProspect {
        platform: AffiliatePlatform::Kiwify,
        key: SHEETS.into(),
        name: "Planilhas Financeiras Pro".into(),
        signals: Signals {
            price: brl("47.00"),
            commission_rate: percent("40"),
            affiliation: Some(Affiliation::OnRequest),
            affiliation_page: Some(page_of(SHEETS)),
            ..Signals::default()
        },
    };
    let yoga = FoundProspect {
        platform: AffiliatePlatform::Kiwify,
        key: YOGA.into(),
        name: "Yoga em Casa".into(),
        signals: Signals {
            affiliation_page: Some(page_of(YOGA)),
            ..Signals::default()
        },
    };
    assert_eq!(found, [cakes, sheets, yoga]);
    assert_eq!(s.devtools.navigations(), [MARKET]);
}

#[test]
fn one_click_reads_only_the_first_listing_page_and_the_same_hour_answers_from_memory() {
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
fn kiwify_names_no_category_so_none_goes_into_an_address() {
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
fn a_products_own_page_adds_the_producer_and_the_warranty_and_waits_its_turn() {
    let s = setup();
    s.devtools.then_read(fixture("listing.json"));
    s.market.prospects(None).unwrap();
    s.clock.advance(TimeDelta::seconds(1));
    s.devtools.then_read(fixture("details.json"));

    let read = s
        .market
        .details(&known(CAKES, Some(page_of(CAKES))))
        .unwrap();

    assert_eq!(
        read,
        FoundProspect {
            platform: AffiliatePlatform::Kiwify,
            key: CAKES.into(),
            name: "Método Confeitaria Lucrativa".into(),
            signals: Signals {
                price: brl("197.00"),
                commission: brl("108.35"),
                commission_rate: percent("55"),
                warranty_days: Some(7),
                creator: Some("Bia Doces".into()),
                affiliation: Some(Affiliation::Immediate),
                affiliation_page: Some(page_of(CAKES)),
                ..Signals::default()
            },
        }
    );
    assert_eq!(s.devtools.navigations(), [MARKET, &page_of(CAKES)]);
    assert_eq!(s.pause.waits(), [MARKET_INTERVAL - Duration::from_secs(1)]);
}

#[test]
fn a_product_without_its_marketplace_address_is_refused_without_opening_anything() {
    let s = setup();

    let without = s.market.details(&known(CAKES, None));
    let odd = s.market.details(&known(
        CAKES,
        Some("https://evil.example/marketplace/<x>".into()),
    ));
    let elsewhere = s.market.details(&known(
        CAKES,
        Some("https://dashboard.kiwify.com.br/vendas".into()),
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
    let while_stopped = s.market.details(&known(CAKES, Some(page_of(CAKES))));

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
fn a_robot_check_names_kiwify_and_a_block_waits_its_day_even_after_a_restart() {
    let s = setup();
    s.devtools.then_read(fixture("captcha.json"));
    let check = s.market.prospects(None);

    let Err(PlatformError::Refused(why)) = check else {
        panic!("{check:?}");
    };
    assert!(
        why.contains("a Kiwify pediu a verificação contra robôs"),
        "{why}"
    );

    let file = s.profile.path().join("kiwify-market-stop");
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
    assert_eq!(restarted.prospects(None).unwrap().len(), 3);
}

#[test]
fn without_the_owners_login_in_the_profile_nothing_opens() {
    let devtools = FakeDevTools::start();
    let profile = tempfile::tempdir().unwrap();
    let browser = BrowserSession::attached(devtools.endpoint(), profile.path().join("nunca"));
    let market = KiwifyAffiliateMarket::new(
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
fn a_products_affiliation_page_is_its_marketplace_page() {
    assert_eq!(kiwify_affiliation_page(CAKES), Some(page_of(CAKES)));
    assert_eq!(kiwify_affiliation_page("x/../y"), None);
    assert_eq!(kiwify_affiliation_page(""), None);
}

#[test]
fn reading_the_marketplace_twice_keeps_one_prospect_per_product_with_market_signals() {
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
        let cakes = affiliate
            .prospects(Some(AffiliatePlatform::Kiwify))
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.key == CAKES)
            .unwrap();
        s.devtools.then_read(fixture("details.json"));
        let read = affiliate
            .read_prospect_details(&s.market, cakes.id)
            .await
            .unwrap();

        assert_eq!((first.found, first.added), (3, 3));
        assert_eq!((again.found, again.added), (3, 0));
        assert_eq!(
            affiliate
                .prospects(Some(AffiliatePlatform::Kiwify))
                .await
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            cakes.reading(SignalKind::Commission).unwrap().origin,
            SignalOrigin::Market
        );
        assert_eq!(cakes.signals.affiliation_page, Some(page_of(CAKES)));
        assert_eq!(
            (read.id, read.signals.commission),
            (cakes.id, brl("108.35"))
        );
        assert_eq!(read.signals.creator.as_deref(), Some("Bia Doces"));
    });
}
