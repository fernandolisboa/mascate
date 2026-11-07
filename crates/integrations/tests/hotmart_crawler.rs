//! The Hotmart vitrine reader against the fake server serving pages written
//! by hand (see the fixtures' README): Prospects keyed by Hotmart's product
//! id, each signal empty when its field is missing or odd, and the product
//! page read only for one Prospect.

use std::str::FromStr;
use std::sync::Arc;

use chrono::NaiveDate;
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliatePlatform, FoundProspect, ProspectCategory, ProspectSource, SignalOrigin,
    Signals,
};
use mascate_integrations::{FLAGS, HOTMART_CRAWLER, HotmartCrawler};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, PlatformError};
use mascate_platform::testing::FakeHttpServer;
use mascate_platform::{Database, FlagKind, Phase, migrate};
use rust_decimal::Decimal;

mod crawling;

use crawling::pages;

const CATEGORY: &str = "/pt-br/marketplace/categorias/culinaria-e-gastronomia";
const CAKES_PATH: &str = "/pt-br/marketplace/produtos/escola-de-bolos-3-em-1/Q52300001P";
const CAKES_PAGE: &str =
    "https://hotmart.com/pt-br/marketplace/produtos/escola-de-bolos-3-em-1/Q52300001P";

fn fixture(name: &str) -> String {
    crawling::fixture("hotmart-crawler", name)
}

struct Setup {
    server: FakeHttpServer,
    clock: Arc<ManualClock>,
    reader: HotmartCrawler,
}

fn setup() -> Setup {
    let site = crawling::site(fixture("robots.txt"));
    let reader = HotmartCrawler::new(
        &format!("{}/pt-br/marketplace/", site.server.url()),
        site.engine,
    );
    Setup {
        server: site.server,
        clock: site.clock,
        reader,
    }
}

fn category(id: &str) -> ProspectCategory {
    ProspectCategory {
        id: id.into(),
        name: String::new(),
    }
}

fn cooking() -> ProspectCategory {
    category("culinaria-e-gastronomia")
}

fn decimal(text: &str) -> Option<Decimal> {
    Some(Decimal::from_str(text).unwrap())
}

fn day(year: i32, month: u32, day: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, day)
}

/// A Prospect kept with only the page of the product.
fn known(key: &str, page: &str) -> FoundProspect {
    FoundProspect {
        platform: AffiliatePlatform::Hotmart,
        key: key.into(),
        name: "Kept name".into(),
        signals: Signals {
            product_page: Some(page.into()),
            ..Signals::default()
        },
    }
}

#[test]
fn a_category_page_yields_prospects_keyed_by_the_hotmart_product_id_with_their_signals() {
    let s = setup();
    s.server.serve(CATEGORY, 200, fixture("category.html"));

    let found = s.reader.prospects(Some(&cooking())).unwrap();

    let keys: Vec<&str> = found.iter().map(|found| found.key.as_str()).collect();
    assert_eq!(keys, ["1418001", "154002", "659003", "663006"]);
    let cakes = &found[0];
    assert_eq!(cakes.platform, AffiliatePlatform::Hotmart);
    assert_eq!(cakes.name, "Escola de Bolos 3 em 1");
    assert_eq!(
        cakes.signals,
        Signals {
            rating: decimal("4.87"),
            reviews: Some(525),
            launched: day(2025, 9, 28),
            classes: Some(48),
            hours: decimal("12.5"),
            certificate: Some(true),
            video: Some(true),
            creator: Some("Ana Confeiteira".into()),
            product_page: Some(CAKES_PAGE.into()),
            ..Signals::default()
        }
    );
}

#[test]
fn a_missing_or_odd_field_leaves_its_signal_empty_and_never_fails_the_page() {
    let s = setup();
    s.server.serve(CATEGORY, 200, fixture("category.html"));

    let found = s.reader.prospects(Some(&cooking())).unwrap();

    let fit = &found[1].signals;
    assert_eq!(fit.rating, None, "showRating false hides the rating");
    assert_eq!(fit.reviews, Some(803));
    assert_eq!(fit.launched, day(2026, 9, 28));
    assert_eq!(fit.classes, Some(20));
    assert_eq!(fit.hours, None, "negative hours");
    assert_eq!((fit.certificate, fit.video), (Some(false), Some(false)));
    let cakes = &found[2].signals;
    assert_eq!(
        (cakes.launched, cakes.classes, cakes.reviews, cakes.video),
        (None, None, None, Some(false))
    );
    assert_eq!(cakes.rating, decimal("4.9"));
    let vegan = &found[3].signals;
    assert_eq!(
        (vegan.rating, vegan.certificate, vegan.video),
        (None, None, None)
    );
    assert_eq!(vegan.creator.as_deref(), Some("Larica Vegana"));
    assert!(found.iter().all(|found| found.signals.price.is_none()
        && found.signals.commission.is_none()
        && found.signals.temperature.is_none()));
}

#[test]
fn without_a_category_the_best_sellers_come_from_the_front_page_without_the_banners() {
    let s = setup();
    s.server
        .serve("/pt-br/marketplace", 200, fixture("best-sellers.html"));

    let found = s.reader.prospects(None).unwrap();

    assert_eq!(pages(&s.server), ["/pt-br/marketplace"]);
    let keys: Vec<&str> = found.iter().map(|found| found.key.as_str()).collect();
    assert_eq!(keys, ["3101", "3102"]);
    let barber = &found[0].signals;
    assert_eq!(
        (barber.launched, barber.classes, barber.hours),
        (day(2026, 10, 20), Some(60), decimal("20"))
    );
    assert_eq!(found[1].signals.creator.as_deref(), Some("Planilhas & Cia"));
}

#[test]
fn reading_the_same_page_twice_gives_the_same_prospects_from_one_request() {
    let s = setup();
    s.server.serve(CATEGORY, 200, fixture("category.html"));

    let first = s.reader.prospects(Some(&cooking())).unwrap();
    let again = s.reader.prospects(Some(&cooking())).unwrap();

    assert_eq!(first, again);
    assert_eq!(
        pages(&s.server).len(),
        1,
        "the second read comes from memory"
    );
}

#[test]
fn the_categories_are_the_vitrines_and_nothing_else_goes_into_an_address() {
    let s = setup();

    let categories = s.reader.categories().unwrap();
    assert_eq!(categories.len(), 18);
    assert_eq!(categories[3].name, "Culinária e gastronomia");
    for refused in [
        category("culinaria/confeitaria"),
        category("../../admin"),
        category("https://evil.example.com/marketplace/categorias/moda"),
        category("Culinária"),
    ] {
        assert!(
            matches!(
                s.reader.prospects(Some(&refused)),
                Err(PlatformError::Refused(_))
            ),
            "{refused:?}"
        );
    }
    assert!(s.server.requests().is_empty());
    for listed in &categories {
        let _ = s.reader.prospects(Some(listed));
    }
    let read = pages(&s.server);
    assert_eq!(read.len(), 18);
    assert!(
        read.iter()
            .all(|path| path.starts_with("/pt-br/marketplace/categorias/"))
    );
}

#[test]
fn a_product_page_adds_the_students_the_warranty_and_the_producer() {
    let s = setup();
    s.server.serve(CAKES_PATH, 200, fixture("product.html"));

    let read = s
        .reader
        .details(&known("1418001", &format!("{CAKES_PAGE}?ref=x")))
        .unwrap();

    assert_eq!(pages(&s.server), [CAKES_PATH]);
    assert_eq!(
        (read.key.as_str(), read.name.as_str()),
        ("1418001", "Escola de Bolos 3 em 1")
    );
    assert_eq!(
        read.signals,
        Signals {
            rating: decimal("4.9"),
            reviews: Some(530),
            students: Some(15_230),
            launched: day(2025, 9, 28),
            warranty_days: Some(7),
            classes: Some(48),
            hours: decimal("12.5"),
            certificate: Some(true),
            video: Some(true),
            creator: Some("Ana Confeiteira".into()),
            creator_since: day(2018, 1, 1),
            creator_verified: Some(true),
            product_page: Some(CAKES_PAGE.into()),
            affiliation_page: Some(
                "https://app.hotmart.com/market/details?productUcode=\
                 8a1c3e52-0f6d-4b8e-9c21-5d7a0e4f1b93"
                    .into()
            ),
            ..Signals::default()
        }
    );
}

#[test]
fn a_pasted_product_page_gets_the_hotmart_id_and_odd_fields_stay_empty() {
    let s = setup();
    s.server.serve(CAKES_PATH, 200, fixture("product.html"));
    let brownies = "/pt-br/marketplace/produtos/brownies-perfeitos/O35100007S";
    s.server.serve(brownies, 200, fixture("product-odd.html"));

    let pasted = s
        .reader
        .details(&known("https://hotmart.com/x", CAKES_PAGE))
        .unwrap();
    let odd = s
        .reader
        .details(&known("859007", &format!("https://hotmart.com{brownies}")))
        .unwrap();

    assert_eq!(pasted.key, "1418001");
    assert_eq!(
        (odd.key.as_str(), odd.name.as_str()),
        ("859007", "Kept name")
    );
    assert_eq!(
        odd.signals,
        Signals {
            warranty_days: Some(30),
            creator: Some("Daiana Dias".into()),
            creator_since: day(2015, 3, 1),
            creator_verified: Some(false),
            product_page: Some(format!("https://hotmart.com{brownies}")),
            ..Signals::default()
        }
    );
}

#[test]
fn a_page_off_hotmart_or_not_a_product_is_refused_without_a_request() {
    let s = setup();

    for page in [
        "https://evil.example.com/pt-br/marketplace/produtos/bolos/Q1",
        "http://hotmart.com/pt-br/marketplace/produtos/bolos/Q1",
        "https://hotmart.com/pt-br/marketplace/categorias/moda",
        "https://hotmart.com/pt-br/marketplace/produtos/../admin/Q1",
        "https://hotmart.com/pt-br/marketplace/produtos/bolos/Q1%2F..",
    ] {
        assert!(
            matches!(
                s.reader.details(&known("1", page)),
                Err(PlatformError::Refused(_))
            ),
            "{page}"
        );
    }
    let without_page = FoundProspect {
        signals: Signals::default(),
        ..known("1", CAKES_PAGE)
    };
    assert!(s.reader.details(&without_page).is_err());
    assert!(s.server.requests().is_empty());
}

#[test]
fn a_forbidden_page_three_times_opens_the_breaker_and_says_until_when() {
    let s = setup();
    s.server.serve(CATEGORY, 403, fixture("forbidden.html"));

    for _ in 0..3 {
        assert!(matches!(
            s.reader.prospects(Some(&cooking())),
            Err(PlatformError::Refused(_))
        ));
    }
    let while_open = s.reader.details(&known("1418001", CAKES_PAGE));

    assert!(matches!(while_open, Err(PlatformError::Refused(_))));
    assert_eq!(pages(&s.server).len(), 3);
    assert!(
        s.reader
            .blocked_until()
            .is_some_and(|until| until > s.clock.now())
    );
}

#[test]
fn read_twice_into_the_affiliate_the_same_prospects_stay_and_a_dismissed_one_stays_out() {
    block_on(async {
        let s = setup();
        s.server.serve(CATEGORY, 200, fixture("category.html"));
        s.server.serve(CAKES_PATH, 200, fixture("product.html"));
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

        let first = affiliate
            .read_prospects(&s.reader, Some(&cooking()))
            .await
            .unwrap();
        let listed = affiliate.prospects(None).await.unwrap();
        let fit = listed.iter().find(|p| p.key == "154002").unwrap().id;
        affiliate
            .dismiss_prospect(fit, "fora do meu nicho")
            .await
            .unwrap();
        let again = affiliate
            .read_prospects(&s.reader, Some(&cooking()))
            .await
            .unwrap();
        let cakes = listed.iter().find(|p| p.key == "1418001").unwrap().id;
        let detailed = affiliate
            .read_prospect_details(&s.reader, cakes)
            .await
            .unwrap();

        assert_eq!((first.found, first.added), (4, 4));
        assert_eq!((again.found, again.added, again.dismissed), (4, 0, 1));
        let shown: Vec<String> = affiliate
            .prospects(None)
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.key)
            .collect();
        assert_eq!(shown.len(), 3);
        assert!(!shown.contains(&"154002".to_owned()));
        assert_eq!(detailed.signals.students, Some(15_230));
        assert_eq!(
            detailed
                .reading(mascate_affiliate::SignalKind::Students)
                .unwrap()
                .origin,
            SignalOrigin::Vitrine
        );
    });
}

#[test]
fn the_reader_is_a_restricted_feature_of_phase_3_with_its_reason_and_risk() {
    let s = setup();
    assert!(FLAGS.contains(&HOTMART_CRAWLER));
    assert_eq!(HOTMART_CRAWLER.key, "integrations.hotmart_crawler");
    assert_eq!(HOTMART_CRAWLER.kind, FlagKind::RestrictedFeature);
    assert_eq!(HOTMART_CRAWLER.phase, Phase::Three);
    assert!(HOTMART_CRAWLER.reason.contains("3.5(m)"));
    assert!(HOTMART_CRAWLER.risk.contains("conta de afiliado"));
    assert_eq!(s.reader.platform(), AffiliatePlatform::Hotmart);
    assert_eq!(s.reader.origin(), SignalOrigin::Vitrine);
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(48))]

    /// Whatever page a Prospect keeps, the reader asks only for a product
    /// page of plain slugs and codes on the marketplace's own host.
    #[test]
    fn any_kept_page_becomes_a_plain_product_path(page in "\\PC{0,80}") {
        let s = setup();
        let _ = s.reader.details(&known("1", &format!("https://hotmart.com/pt-br/marketplace/produtos/{page}")));
        for path in pages(&s.server) {
            let rest = path.strip_prefix("/pt-br/marketplace/produtos/").unwrap();
            let (slug, code) = rest.split_once('/').unwrap();
            proptest::prop_assert!(slug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
            proptest::prop_assert!(code.chars().all(|c| c.is_ascii_alphanumeric()));
        }
    }
}
