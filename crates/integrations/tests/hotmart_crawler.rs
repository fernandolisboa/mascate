//! The Hotmart marketplace crawler against the fake server serving pages
//! written by hand (see the fixtures' README).

use std::str::FromStr;
use std::sync::Arc;

use futures::executor::block_on;
use mascate_catalog::{Catalog, KeptOffers, OfferOrder, OfferSearch, OfferSource, ProductSource};
use mascate_integrations::{FLAGS, HOTMART_CATEGORIES, HOTMART_CRAWLER, HotmartCrawler};
use mascate_kernel::PlatformError;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::testing::FakeHttpServer;
use mascate_platform::{Database, FlagKind, Phase, migrate};
use rust_decimal::Decimal;

mod crawling;

use crawling::{brl, pages};

const CATEGORY: &str = "/pt-br/marketplace/categorias/culinaria-e-gastronomia";

fn fixture(name: &str) -> String {
    crawling::fixture("hotmart-crawler", name)
}

struct Setup {
    server: FakeHttpServer,
    clock: Arc<ManualClock>,
    crawler: HotmartCrawler,
}

fn setup() -> Setup {
    let site = crawling::site(fixture("robots.txt"));
    let crawler = HotmartCrawler::new(
        &format!("{}/pt-br/marketplace/", site.server.url()),
        site.engine,
    );
    Setup {
        server: site.server,
        clock: site.clock,
        crawler,
    }
}

fn browse(category: Option<&str>) -> OfferSearch {
    OfferSearch {
        category: category.map(str::to_owned),
        order: OfferOrder::Relevance,
        page: 1,
        ..OfferSearch::default()
    }
}

fn stars(text: &str) -> Option<Decimal> {
    Some(Decimal::from_str(text).unwrap())
}

#[test]
fn a_category_page_yields_found_offers_keyed_by_the_hotmart_product_id_with_the_producer_as_shop() {
    let s = setup();
    s.server.serve(CATEGORY, 200, fixture("category.html"));

    let found = s
        .crawler
        .search_offers(&browse(Some("culinaria-e-gastronomia")))
        .unwrap();

    assert!(!found.more);
    let ids: Vec<&str> = found.items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, ["1418001", "154002", "659003", "663006"]);

    let cakes = &found.items[0];
    assert_eq!(cakes.title, "Escola de Bolos 3 em 1");
    assert_eq!(
        cakes.link,
        "https://hotmart.com/pt-br/marketplace/produtos/escola-de-bolos-3-em-1/Q52300001P"
    );
    assert_eq!(
        cakes.image.as_deref(),
        Some("https://hotmart.s3.amazonaws.com/product_pictures/11aa22bb/bolos.png")
    );
    assert_eq!((cakes.price, cakes.highest_price), (None, None));
    assert_eq!(cakes.rating, stars("4.87"));
    assert_eq!((cakes.sales, cakes.commission), (0, None));
    assert_eq!(
        (
            cakes.shop.id.as_str(),
            cakes.shop.name.as_str(),
            cakes.shop.link.as_str()
        ),
        (
            "AC4100001Z",
            "Ana Confeiteira",
            "https://hotmart.com/pt-br/marketplace/especialista/ana-confeiteira/AC4100001Z"
        )
    );

    let fit = &found.items[1];
    assert_eq!(fit.rating, None);
    assert_eq!(
        fit.shop.link,
        "https://hotmart.com/pt-br/marketplace/especialista/camila-fit/CF4100002Y"
    );

    let priced = &found.items[2];
    assert_eq!(priced.price, Some(brl("97")));
    assert_eq!(priced.highest_price, Some(brl("97")));
    assert_eq!(
        priced.image.as_deref(),
        Some(
            "https://static-media.hotmart.com/jkl=/300x300/hotmart/product_pictures/bolos-perfeitos.png"
        )
    );
    assert_eq!(
        priced.shop.link,
        "https://hotmart.com/pt-br/marketplace/especialista/beatriz-doces/BD4100003X"
    );

    let vegan = &found.items[3];
    assert_eq!((vegan.rating, vegan.image.as_deref()), (None, None));
    assert_eq!(
        (
            vegan.shop.id.as_str(),
            vegan.shop.name.as_str(),
            vegan.shop.link.as_str()
        ),
        (
            "larica-vegana",
            "Larica Vegana",
            "https://hotmart.com/pt-br/marketplace"
        )
    );
}

#[test]
fn without_a_category_the_best_sellers_come_from_the_front_page_without_the_banners() {
    let s = setup();
    s.server
        .serve("/pt-br/marketplace", 200, fixture("best-sellers.html"));

    let found = s.crawler.search_offers(&browse(None)).unwrap();

    assert_eq!(pages(&s.server), ["/pt-br/marketplace"]);
    let ids: Vec<&str> = found.items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, ["3101", "3102"]);
    let barber = &found.items[0];
    assert_eq!(barber.rating, stars("4.78"));
    assert_eq!(
        (barber.shop.id.as_str(), barber.shop.name.as_str()),
        ("PN9001AAAA", "Paulo Navalha")
    );
    let excel = &found.items[1];
    assert_eq!(
        excel.image.as_deref(),
        Some("https://static-media.hotmart.com/def=/300x300/hotmart/product_pictures/excel.png")
    );
    assert_eq!(
        excel.shop.link,
        "https://hotmart.com/pt-br/marketplace/especialista/planilhas-e-cia/PC9002BBBB"
    );
}

#[test]
fn a_subcategory_or_a_category_link_goes_in_the_address() {
    let s = setup();
    s.server.serve(
        &format!("{CATEGORY}/confeitaria"),
        200,
        fixture("subcategory.html"),
    );
    s.server.serve(
        "/pt-br/marketplace/categorias/saude-e-esportes",
        200,
        fixture("subcategory.html"),
    );

    let sweets = s
        .crawler
        .search_offers(&browse(Some(" culinaria-e-gastronomia/confeitaria/ ")))
        .unwrap();
    s.crawler
        .search_offers(&browse(Some(
            "https://hotmart.com/pt-br/marketplace/categorias/saude-e-esportes?utm_source=x#top",
        )))
        .unwrap();

    assert_eq!(
        pages(&s.server),
        [
            format!("{CATEGORY}/confeitaria"),
            "/pt-br/marketplace/categorias/saude-e-esportes".to_owned()
        ]
    );
    let brownies = &sweets.items[0];
    assert_eq!(
        (brownies.id.as_str(), brownies.rating),
        ("859007", stars("4.7"))
    );
    assert_eq!(
        (brownies.shop.id.as_str(), brownies.shop.name.as_str()),
        ("daiana-dias", "Daiana Dias")
    );
}

#[test]
fn a_keyword_search_is_refused_without_a_request_because_robots_txt_disallows_it() {
    let s = setup();
    s.server.serve(CATEGORY, 200, fixture("category.html"));

    let refused = s.crawler.search_offers(&OfferSearch {
        keyword: "bolo de pote".into(),
        ..browse(Some("culinaria-e-gastronomia"))
    });

    match refused {
        Err(PlatformError::Refused(why)) => assert!(why.contains("robots.txt"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(s.server.requests().is_empty());
}

#[test]
fn shops_other_orders_and_what_is_not_a_category_are_refused_without_a_request() {
    let s = setup();

    assert!(!s.crawler.finds_shops());
    assert_eq!(s.crawler.orders(), [OfferOrder::Relevance]);
    assert!(s.crawler.search_shops("bolos", 1).is_err());
    for refused in [
        OfferSearch {
            order: OfferOrder::Sales,
            ..browse(None)
        },
        OfferSearch {
            shop: Some("AC4100001Z".into()),
            ..browse(None)
        },
        browse(Some("culinaria/confeitaria/bolos")),
        browse(Some("../../admin")),
        browse(Some("Culinária")),
        browse(Some("https://evil.example.com/marketplace/categorias/moda")),
        browse(Some(
            "https://hotmart.com/pt-br/marketplace/produtos?q=bolo",
        )),
    ] {
        assert!(
            matches!(
                s.crawler.search_offers(&refused),
                Err(PlatformError::Refused(_))
            ),
            "{refused:?}"
        );
    }
    assert!(s.server.requests().is_empty());
}

#[test]
fn a_later_page_finds_nothing_without_a_request() {
    let s = setup();

    let later = s
        .crawler
        .search_offers(&OfferSearch {
            page: 2,
            ..browse(Some("culinaria-e-gastronomia"))
        })
        .unwrap();

    assert!(later.items.is_empty());
    assert!(!later.more);
    assert!(s.server.requests().is_empty());
}

#[test]
fn only_a_found_offer_with_a_price_is_kept_once_with_its_producer_as_supplier() {
    block_on(async {
        let s = setup();
        s.server.serve(CATEGORY, 200, fixture("category.html"));
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&dir.path().join("m.db")).await.unwrap());
        migrate(&database, s.clock.as_ref(), &[mascate_catalog::MIGRATIONS])
            .await
            .unwrap();
        let catalog = Catalog::new(
            database,
            dir.path().join("produtos"),
            s.clock.clone(),
            Arc::new(SequentialIds::default()),
        );

        let found = catalog
            .search_offers(&s.crawler, &browse(Some("culinaria-e-gastronomia")))
            .await
            .unwrap();
        let offers: Vec<_> = found.items.iter().map(|item| item.found.clone()).collect();
        let kept = catalog
            .keep_found_offers(s.crawler.source(), &offers)
            .await
            .unwrap();
        let kept_again = catalog
            .keep_found_offers(s.crawler.source(), &offers)
            .await
            .unwrap();

        assert_eq!(
            kept,
            KeptOffers {
                added: 1,
                unchanged: 0,
                skipped: 3
            }
        );
        assert_eq!((kept_again.added, kept_again.unchanged), (0, 1));
        let offers = catalog.offers().await.unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].latest.source, ProductSource::HotmartCrawler);
        assert_eq!(offers[0].latest.price, brl("97"));
        let suppliers: Vec<String> = catalog
            .suppliers()
            .await
            .unwrap()
            .into_iter()
            .map(|supplier| supplier.name)
            .collect();
        assert_eq!(suppliers, ["Beatriz Doces"]);
    });
}

#[test]
fn a_forbidden_page_three_times_opens_the_breaker() {
    let s = setup();
    s.server.serve(CATEGORY, 403, fixture("forbidden.html"));

    for _ in 0..3 {
        let blocked = s
            .crawler
            .search_offers(&browse(Some("culinaria-e-gastronomia")));
        assert!(matches!(blocked, Err(PlatformError::Refused(_))));
    }
    let while_open = s
        .crawler
        .search_offers(&browse(Some("culinaria-e-gastronomia")));

    assert!(matches!(while_open, Err(PlatformError::Refused(_))));
    assert_eq!(pages(&s.server).len(), 3);
    assert!(s.crawler.blocked_until().is_some());
}

#[test]
fn every_listed_category_and_subcategory_is_a_page_the_crawler_reads() {
    let s = setup();

    for category in HOTMART_CATEGORIES {
        assert!(!category.name.is_empty());
        for path in std::iter::once(category.slug.to_owned()).chain(
            category
                .subcategories
                .iter()
                .map(|(slug, _)| format!("{}/{slug}", category.slug)),
        ) {
            let _ = s.crawler.search_offers(&browse(Some(&path)));
        }
    }

    let read = pages(&s.server);
    let subcategories: usize = HOTMART_CATEGORIES
        .iter()
        .map(|category| category.subcategories.len())
        .sum();
    assert_eq!(HOTMART_CATEGORIES.len(), 18);
    assert_eq!(read.len(), HOTMART_CATEGORIES.len() + subcategories);
    assert!(
        read.iter()
            .all(|path| path.starts_with("/pt-br/marketplace/categorias/"))
    );
}

#[test]
fn the_crawler_is_a_restricted_feature_of_phase_3_with_its_reason_and_risk() {
    assert!(FLAGS.contains(&HOTMART_CRAWLER));
    assert_eq!(HOTMART_CRAWLER.key, "integrations.hotmart_crawler");
    assert_eq!(HOTMART_CRAWLER.kind, FlagKind::RestrictedFeature);
    assert_eq!(HOTMART_CRAWLER.phase, Phase::Three);
    assert!(HOTMART_CRAWLER.reason.contains("3.5(m)"));
    assert!(HOTMART_CRAWLER.risk.contains("conta de afiliado"));
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(48))]

    /// Whatever the category says, the crawler asks only for a category
    /// page of plain slugs on the marketplace's own host.
    #[test]
    fn any_category_becomes_a_plain_category_path(category in "\\PC{0,60}") {
        let s = setup();
        let _ = s.crawler.search_offers(&browse(Some(&category)));
        for path in pages(&s.server) {
            let slugs = path
                .strip_prefix("/pt-br/marketplace/categorias/")
                .or_else(|| (path == "/pt-br/marketplace").then_some(""))
                .unwrap();
            proptest::prop_assert!(slugs
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '/'));
            proptest::prop_assert!(!slugs.contains("//"));
        }
    }
}
