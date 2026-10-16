//! The Magalu crawler against the fake server serving search pages written
//! by hand (see the fixtures' README).

use std::str::FromStr;
use std::sync::Arc;

use futures::executor::block_on;
use mascate_catalog::{Catalog, KeptOffers, OfferOrder, OfferSearch, OfferSource, ProductSource};
use mascate_integrations::{FLAGS, MAGALU_CRAWLER, MagaluCrawler};
use mascate_kernel::PlatformError;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::testing::FakeHttpServer;
use mascate_platform::{Database, FlagKind, Phase, migrate};
use rust_decimal::Decimal;

mod crawling;

use crawling::{brl, pages};

fn fixture(name: &str) -> String {
    crawling::fixture("magalu-crawler", name)
}

struct Setup {
    server: FakeHttpServer,
    clock: Arc<ManualClock>,
    crawler: MagaluCrawler,
}

fn setup() -> Setup {
    let site = crawling::site(fixture("robots.txt"));
    let crawler = MagaluCrawler::new(site.server.url(), site.engine);
    Setup {
        server: site.server,
        clock: site.clock,
        crawler,
    }
}

fn search(keyword: &str) -> OfferSearch {
    OfferSearch {
        keyword: keyword.into(),
        order: OfferOrder::Relevance,
        page: 1,
        ..OfferSearch::default()
    }
}

#[test]
fn a_search_page_yields_found_offers_keyed_by_the_magalu_product_id() {
    let s = setup();
    s.server
        .serve("/busca/fone+bluetooth/", 200, fixture("search.html"));

    let found = s.crawler.search_offers(&search("Fone Bluetooth")).unwrap();

    assert!(found.more);
    let ids: Vec<&str> = found.items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, ["237361800", "hk2b2c6c55", "ag3d4e5f6g", "cc1d2e3f4g"]);
    let jbl = &found.items[0];
    assert_eq!(jbl.title, "Fone de Ouvido Bluetooth JBL Tune 520BT");
    assert_eq!(
        jbl.link,
        "https://www.magazineluiza.com.br/fone-de-ouvido-bluetooth-jbl-tune-520bt/p/237361800/te/fbtj/"
    );
    assert_eq!(
        jbl.image.as_deref(),
        Some(
            "https://a-static.mlcdn.com.br/800x560/fone-de-ouvido-bluetooth-jbl-tune-520bt/magazineluiza/237361800/7a1b2c3d.jpg"
        )
    );
    assert_eq!(jbl.price, Some(brl("229.00")));
    assert_eq!(jbl.highest_price, Some(brl("229.00")));
    assert_eq!(jbl.rating, Some(Decimal::from_str("4.8").unwrap()));
    assert_eq!(
        (jbl.shop.id.as_str(), jbl.shop.name.as_str()),
        ("magazineluiza", "Magazine Luiza")
    );
    assert_eq!((jbl.sales, jbl.commission), (0, None));

    let tws = &found.items[1];
    assert_eq!(
        tws.link,
        "https://www.magazineluiza.com.br/fone-sem-fio-tws-com-case-carregador/p/hk2b2c6c55/te/fbtj/"
    );
    assert_eq!(tws.price, Some(brl("89.90")));
    assert_eq!(tws.rating, None);
    assert_eq!(
        (tws.shop.id.as_str(), tws.shop.name.as_str()),
        ("fonesbr", "Fones BR")
    );

    let unavailable = &found.items[2];
    assert_eq!(
        (unavailable.price, unavailable.image.as_deref()),
        (None, None)
    );
    assert_eq!(unavailable.shop.name, "gamerstore");

    assert_eq!(found.items[3].shop.name, "Magazine Luiza");
}

#[test]
fn a_later_page_and_the_order_go_in_the_address() {
    let s = setup();
    s.server.serve(
        "/busca/fone+bluetooth/?page=3&sortOrientation=asc&sortType=price",
        200,
        fixture("search-page-3.html"),
    );
    s.server.serve(
        "/busca/cafeteira+eletrica/?sortOrientation=desc&sortType=soldQuantity",
        200,
        fixture("search-page-3.html"),
    );

    let lowest = s
        .crawler
        .search_offers(&OfferSearch {
            order: OfferOrder::LowestPrice,
            page: 3,
            ..search("fone  bluetooth!")
        })
        .unwrap();
    s.crawler
        .search_offers(&OfferSearch {
            order: OfferOrder::Sales,
            ..search("Cafeteira Elétrica")
        })
        .unwrap();

    assert_eq!(
        pages(&s.server),
        [
            "/busca/fone+bluetooth/?page=3&sortOrientation=asc&sortType=price",
            "/busca/cafeteira+eletrica/?sortOrientation=desc&sortType=soldQuantity"
        ]
    );
    assert!(!lowest.more);
    assert_eq!(
        lowest.items[0].title,
        "Fone Bluetooth Esportivo à Prova d'Água"
    );
    assert_eq!(lowest.items[0].price, Some(brl("119.90")));
}

#[test]
fn a_page_without_its_state_is_read_from_its_json_ld() {
    let s = setup();
    s.server.serve(
        "/busca/caixa+de+som+bluetooth/",
        200,
        fixture("search-json-ld.html"),
    );

    let found = s
        .crawler
        .search_offers(&search("caixa de som bluetooth"))
        .unwrap();

    assert!(found.more);
    let ids: Vec<&str> = found.items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, ["238877001", "kd5e6f7g8h"]);
    let jbl = &found.items[0];
    assert_eq!(jbl.price, Some(brl("299.00")));
    assert_eq!(jbl.rating, Some(Decimal::from_str("4.9").unwrap()));
    assert_eq!(jbl.shop.name, "Magazine Luiza");
    let portable = &found.items[1];
    assert_eq!(portable.shop.name, "Som Total");
    assert_eq!(
        portable.image.as_deref(),
        Some("https://a-static.mlcdn.com.br/800x560/caixa-portatil/somtotal/cx01/4d5e6f.jpg")
    );
}

#[test]
fn found_offers_kept_twice_are_one_supplier_offer_each_and_one_supplier_per_seller() {
    block_on(async {
        let s = setup();
        s.server
            .serve("/busca/fone+bluetooth/", 200, fixture("search.html"));
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
            .search_offers(&s.crawler, &search("fone bluetooth"))
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
                added: 3,
                unchanged: 0,
                skipped: 1
            }
        );
        assert_eq!(kept_again.added, 0);
        assert_eq!(kept_again.unchanged, 3);
        let offers = catalog.offers().await.unwrap();
        assert_eq!(offers.len(), 3);
        assert!(
            offers
                .iter()
                .all(|offer| offer.latest.source == ProductSource::MagaluCrawler)
        );
        let mut suppliers: Vec<String> = catalog
            .suppliers()
            .await
            .unwrap()
            .into_iter()
            .map(|supplier| supplier.name)
            .collect();
        suppliers.sort();
        assert_eq!(suppliers, ["Fones BR", "Magazine Luiza"]);
    });
}

#[test]
fn an_access_denied_page_or_a_robot_check_counts_toward_the_breaker() {
    for (status, page) in [(403, "access-denied.html"), (200, "verification.html")] {
        let s = setup();
        s.server
            .serve("/busca/fone+bluetooth/", status, fixture(page));

        for _ in 0..3 {
            let blocked = s.crawler.search_offers(&search("fone bluetooth"));
            assert!(matches!(blocked, Err(PlatformError::Refused(_))), "{page}");
        }
        let while_open = s.crawler.search_offers(&search("fone bluetooth"));

        assert!(matches!(while_open, Err(PlatformError::Refused(_))));
        assert_eq!(pages(&s.server).len(), 3, "{page}");
        assert!(s.crawler.blocked_until().is_some(), "{page}");
    }
}

#[test]
fn a_robots_txt_the_site_refuses_blocks_the_whole_site() {
    let s = setup();
    s.server
        .serve("/robots.txt", 403, fixture("access-denied.html"));
    s.server.serve("/busca/fone/", 200, fixture("search.html"));

    for _ in 0..3 {
        let _ = s.crawler.search_offers(&search("fone"));
    }

    assert!(pages(&s.server).is_empty());
    assert!(s.crawler.blocked_until().is_some());
}

#[test]
fn the_crawler_offers_no_shops_categories_or_commission_order() {
    let s = setup();

    assert!(!s.crawler.finds_shops());
    assert_eq!(
        s.crawler.orders(),
        [
            OfferOrder::Sales,
            OfferOrder::Relevance,
            OfferOrder::LowestPrice
        ]
    );
    assert!(s.crawler.search_shops("fones", 1).is_err());
    for refused in [
        OfferSearch {
            order: OfferOrder::HighestCommission,
            ..search("fone")
        },
        OfferSearch {
            category: Some("audio".into()),
            ..search("fone")
        },
        OfferSearch {
            shop: Some("fonesbr".into()),
            ..search("fone")
        },
        search(" !? "),
    ] {
        assert!(matches!(
            s.crawler.search_offers(&refused),
            Err(PlatformError::Refused(_))
        ));
    }
    assert!(s.server.requests().is_empty());
}

#[test]
fn the_crawler_is_a_restricted_feature_of_phase_3_with_its_reason_and_risk() {
    assert!(FLAGS.contains(&MAGALU_CRAWLER));
    assert_eq!(MAGALU_CRAWLER.key, "integrations.magalu_crawler");
    assert_eq!(MAGALU_CRAWLER.kind, FlagKind::RestrictedFeature);
    assert_eq!(MAGALU_CRAWLER.phase, Phase::Three);
    assert!(MAGALU_CRAWLER.reason.contains("recusa acesso automatizado"));
    assert!(MAGALU_CRAWLER.risk.contains("bloquear"));
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(48))]

    /// Whatever the owner types, the crawler asks only for `/busca/` with
    /// lower-case letters, digits and `+` on the search's own host.
    #[test]
    fn any_keyword_becomes_a_plain_path_on_the_search_host(keyword in "\\PC{0,40}") {
        let s = setup();
        let _ = s.crawler.search_offers(&search(&keyword));
        for path in pages(&s.server) {
            let words = path
                .strip_prefix("/busca/")
                .and_then(|rest| rest.strip_suffix('/'))
                .unwrap();
            proptest::prop_assert!(!words.is_empty());
            proptest::prop_assert!(words
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '+'));
        }
    }
}
