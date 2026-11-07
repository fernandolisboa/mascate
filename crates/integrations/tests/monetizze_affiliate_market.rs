//! The Monetizze affiliate market reader against a fake DevTools endpoint
//! answering with what the reading script would read on the Vitrine de
//! afiliação (see the fixtures' README), with an injected clock and pause:
//! one listing page per click, the interval, the hour of memory and the
//! day-long stop. Then the Affiliate module over it, with a real temporary
//! database: a second read of the same Vitrine changes nothing, and the
//! BARs score the sales.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliatePlatform, Affiliation, FoundProspect, ProductFormat, ProspectCategory,
    ProspectSource, ScoreArea, SignalKind, SignalOrigin, Signals,
};
use mascate_integrations::{
    AffiliateMarket, BrowserSession, FLAGS, MARKET_INTERVAL, MARKET_STOPPED_FOR,
    MONETIZZE_AFFILIATE_MARKET, MONETIZZE_AFFILIATE_MARKET_URL, MarketStop, MarketStopped,
    MonetizzeAffiliateMarket, PanelState,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, Percentage, PlatformError, Timestamp};
use mascate_platform::{Database, FlagKind, Phase, migrate};
use rust_decimal::Decimal;
use serde_json::Value;

mod browsing;

use browsing::{ClockPause, FakeDevTools, RecordedPauses};

const MARKET: &str = "https://app.monetizze.com.br/loja";
const KIT: &str = "184520";
const ENGLISH: &str = "203117";
const PROTEIN: &str = "211846";
const BUDGET: &str = "219003";

fn fixture(name: &str) -> Value {
    browsing::fixture("monetizze-affiliate-market", name)
}

fn start() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 11, 7, 20, 30, 0).unwrap()
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
    format!("{MARKET}/produto/{id}")
}

struct Setup {
    devtools: FakeDevTools,
    clock: Arc<ManualClock>,
    pause: Arc<ClockPause>,
    market: MonetizzeAffiliateMarket,
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
) -> MonetizzeAffiliateMarket {
    let browser = BrowserSession::attached(devtools.endpoint(), profile.join("navegador"))
        .with_pause(Arc::new(RecordedPauses::default()));
    std::fs::create_dir_all(profile.join("navegador")).unwrap();
    MonetizzeAffiliateMarket::new(MARKET, Arc::new(browser), clock.clone(), pause.clone())
}

fn known(key: &str, affiliation_page: Option<String>) -> FoundProspect {
    FoundProspect {
        platform: AffiliatePlatform::Monetizze,
        key: key.into(),
        name: "Kept name".into(),
        signals: Signals {
            affiliation_page,
            ..Signals::default()
        },
    }
}

#[test]
fn the_reader_is_a_restricted_feature_of_phase_3_with_monetizzes_terms_its_limits_and_risk() {
    assert!(FLAGS.contains(&MONETIZZE_AFFILIATE_MARKET));
    assert_eq!(
        MONETIZZE_AFFILIATE_MARKET.key,
        "integrations.monetizze_affiliate_market"
    );
    assert_eq!(MONETIZZE_AFFILIATE_MARKET.kind, FlagKind::RestrictedFeature);
    assert_eq!(MONETIZZE_AFFILIATE_MARKET.phase, Phase::Three);
    for limit in [
        "13/01/2026",
        "13.5",
        "13.8",
        "14.1",
        "a primeira página da Vitrine",
        "no máximo 5 páginas",
        "5 segundos",
        "1 hora",
        "não pede afiliação",
        "24 horas",
    ] {
        assert!(MONETIZZE_AFFILIATE_MARKET.reason.contains(limit), "{limit}");
    }
    assert!(
        MONETIZZE_AFFILIATE_MARKET
            .risk
            .contains("bloquear a sua conta")
    );
    assert!(MONETIZZE_AFFILIATE_MARKET.risk.contains("reter o saldo"));
    assert_eq!(MONETIZZE_AFFILIATE_MARKET_URL, MARKET);
}

#[test]
fn each_card_is_a_prospect_with_its_commission_bars_format_category_and_affiliation() {
    let s = setup();
    s.devtools.then_read(fixture("drawing.json"));
    s.devtools.then_read(fixture("listing.json"));

    let found = s.market.prospects(None).unwrap();

    let kit = FoundProspect {
        platform: AffiliatePlatform::Monetizze,
        key: KIT.into(),
        name: "Kit de Potes Herméticos".into(),
        signals: Signals {
            commission: brl("49.30"),
            bars: Some(100),
            format: Some(ProductFormat::Physical),
            category: Some("Casa e Decoração".into()),
            affiliation: Some(Affiliation::Immediate),
            affiliation_page: Some(page_of(KIT)),
            ..Signals::default()
        },
    };
    let english = FoundProspect {
        platform: AffiliatePlatform::Monetizze,
        key: ENGLISH.into(),
        name: "Inglês em 90 Dias".into(),
        signals: Signals {
            commission_rate: percent("60"),
            bars: Some(72),
            format: Some(ProductFormat::Digital),
            category: Some("Idiomas".into()),
            affiliation: Some(Affiliation::OnRequest),
            affiliation_page: Some(page_of(ENGLISH)),
            ..Signals::default()
        },
    };
    let protein = FoundProspect {
        platform: AffiliatePlatform::Monetizze,
        key: PROTEIN.into(),
        name: "Protein Bars 12 unidades".into(),
        signals: Signals {
            commission: brl("35.00"),
            category: Some("Saúde e Esportes".into()),
            affiliation_page: Some(page_of(PROTEIN)),
            ..Signals::default()
        },
    };
    let budget = FoundProspect {
        platform: AffiliatePlatform::Monetizze,
        key: BUDGET.into(),
        name: "Planilha de Orçamento".into(),
        signals: Signals {
            affiliation_page: Some(page_of(BUDGET)),
            ..Signals::default()
        },
    };
    assert_eq!(found, [kit, english, protein, budget]);
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
fn no_section_or_category_has_an_address_so_none_goes_into_one() {
    let s = setup();
    let hall = ProspectCategory {
        id: "hall-da-fama&page=99".into(),
        name: "Hall da Fama".into(),
    };

    assert_eq!(s.market.categories().unwrap(), []);
    assert!(matches!(
        s.market.prospects(Some(&hall)),
        Err(PlatformError::Refused(_))
    ));
    assert!(s.devtools.tab_requests().is_empty());
}

#[test]
fn a_products_own_page_adds_price_rate_and_producer_and_waits_its_turn() {
    let s = setup();
    s.devtools.then_read(fixture("listing.json"));
    s.market.prospects(None).unwrap();
    s.clock.advance(TimeDelta::seconds(1));
    s.devtools.then_read(fixture("details.json"));

    let read = s.market.details(&known(KIT, Some(page_of(KIT)))).unwrap();

    assert_eq!(
        read,
        FoundProspect {
            platform: AffiliatePlatform::Monetizze,
            key: KIT.into(),
            name: "Kit de Potes Herméticos".into(),
            signals: Signals {
                price: brl("97.00"),
                commission: brl("49.30"),
                commission_rate: percent("60"),
                bars: Some(100),
                format: Some(ProductFormat::Physical),
                category: Some("Casa e Decoração".into()),
                creator: Some("@KitdePotes".into()),
                affiliation: Some(Affiliation::Immediate),
                affiliation_page: Some(page_of(KIT)),
                ..Signals::default()
            },
        }
    );
    assert_eq!(s.devtools.navigations(), [MARKET, &page_of(KIT)]);
    assert_eq!(s.pause.waits(), [MARKET_INTERVAL - Duration::from_secs(1)]);
}

#[test]
fn only_a_product_page_under_the_vitrine_opens() {
    let s = setup();

    let refused = [
        None,
        Some("https://evil.example/loja/produto/184520".to_owned()),
        Some(format!("{MARKET}/produto/<x>/184520")),
        Some(format!("{MARKET}/../financeiro/184520")),
        Some(format!("{MARKET}/categoria/saude")),
        Some(format!("{MARKET}virtual/184520")),
    ]
    .map(|page| s.market.details(&known(KIT, page)));

    for read in refused {
        assert!(matches!(read, Err(PlatformError::Refused(_))), "{read:?}");
    }
    assert!(s.devtools.tab_requests().is_empty());
}

#[test]
fn a_login_stops_the_reader_for_a_day_until_the_owner_logs_in_again() {
    let s = setup();
    s.devtools.then_read(fixture("login.json"));

    let read = s.market.prospects(None);
    let while_stopped = s.market.details(&known(KIT, Some(page_of(KIT))));

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
fn a_robot_check_names_monetizze_and_a_block_waits_its_day_even_after_a_restart() {
    let s = setup();
    s.devtools.then_read(fixture("captcha.json"));
    let check = s.market.prospects(None);

    let Err(PlatformError::Refused(why)) = check else {
        panic!("{check:?}");
    };
    assert!(
        why.contains("a Monetizze pediu a verificação contra robôs"),
        "{why}"
    );

    let file = s.profile.path().join("monetizze-market-stop");
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
    let market = MonetizzeAffiliateMarket::new(
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
fn reading_the_vitrine_twice_keeps_one_prospect_per_product_scored_by_its_bars() {
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
        let kept = affiliate
            .prospects(Some(AffiliatePlatform::Monetizze))
            .await
            .unwrap();
        let kit = kept.iter().find(|p| p.key == KIT).unwrap().clone();
        s.devtools.then_read(fixture("details.json"));
        let read = affiliate
            .read_prospect_details(&s.market, kit.id)
            .await
            .unwrap();

        assert_eq!((first.found, first.added), (4, 4));
        assert_eq!((again.found, again.added), (4, 0));
        assert_eq!(kept.len(), 4);
        assert_eq!(
            kit.reading(SignalKind::Bars).unwrap().origin,
            SignalOrigin::Market
        );
        let sales = kit
            .score
            .parts
            .iter()
            .find(|part| part.area == ScoreArea::Sales)
            .unwrap();
        assert_eq!(sales.reason, "100 BARs");
        assert_eq!(
            (read.id, read.signals.price, read.signals.creator.as_deref()),
            (kit.id, brl("97.00"), Some("@KitdePotes"))
        );
        assert_eq!(read.signals.format, Some(ProductFormat::Physical));
    });
}
