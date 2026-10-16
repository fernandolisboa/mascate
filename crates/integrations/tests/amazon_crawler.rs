//! The Amazon crawler against the fake server serving search pages written
//! by hand (see the fixtures' README).

use std::str::FromStr;
use std::sync::Arc;

use chrono::TimeDelta;
use futures::executor::block_on;
use mascate_catalog::{Catalog, KeptOffers, OfferOrder, OfferSearch, OfferSource, ProductSource};
use mascate_integrations::{AMAZON_CRAWLER, AmazonCrawler, FLAGS, MagaluCrawler};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, PlatformError};
use mascate_platform::testing::FakeHttpServer;
use mascate_platform::{Database, FlagKind, Phase, migrate};
use rust_decimal::Decimal;

mod crawling;

use crawling::{brl, pages};

fn fixture(name: &str) -> String {
    crawling::fixture("amazon-crawler", name)
}

struct Setup {
    server: FakeHttpServer,
    clock: Arc<ManualClock>,
    crawler: AmazonCrawler,
}

fn setup() -> Setup {
    let site = crawling::site(fixture("robots.txt"));
    let crawler = AmazonCrawler::new(site.server.url(), site.engine);
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
fn a_search_page_yields_found_offers_keyed_by_asin() {
    let s = setup();
    s.server
        .serve("/s?k=fone+bluetooth", 200, fixture("search.html"));

    let found = s.crawler.search_offers(&search("Fone Bluetooth")).unwrap();

    assert!(found.more);
    let ids: Vec<&str> = found.items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(
        ids,
        ["B0CHX1W1XY", "B0D5NKQ8ZT", "B09TESTE12", "B0USD00001"]
    );
    let fone = &found.items[0];
    assert_eq!(fone.title, "Fone de Ouvido Bluetooth TWS com Estojo");
    assert_eq!(fone.link, "https://www.amazon.com.br/dp/B0CHX1W1XY");
    assert_eq!(
        fone.image.as_deref(),
        Some("https://m.media-amazon.com/images/I/61fone1AbCL._AC_UL320_.jpg")
    );
    assert_eq!(fone.price, Some(brl("129.90")));
    assert_eq!(fone.highest_price, Some(brl("129.90")));
    assert_eq!(fone.rating, Some(Decimal::from_str("4.5").unwrap()));
    assert_eq!(fone.sales, 1_000);
    assert_eq!(
        (fone.shop.id.as_str(), fone.shop.name.as_str()),
        ("amazon", "Amazon")
    );
    assert_eq!(fone.commission, None);

    let studio = &found.items[1];
    assert_eq!(
        studio.title,
        "Fone Over-Ear \"Studio\" Bluetooth & Cabo P2 – Preto"
    );
    assert_eq!(studio.price, Some(brl("1299.00")));
    assert_eq!(studio.sales, 500);
    assert_eq!(studio.link, "https://www.amazon.com.br/dp/B0D5NKQ8ZT");
}

#[test]
fn a_result_without_a_price_is_found_without_one_and_not_kept() {
    block_on(async {
        let s = setup();
        s.server
            .serve("/s?k=fone+bluetooth", 200, fixture("search.html"));
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

        let without_price = &found.items[2].found;
        assert_eq!(without_price.title, "Fone com Fio Intra-auricular");
        assert_eq!(
            (without_price.price, without_price.highest_price),
            (None, None)
        );
        assert_eq!(without_price.image, None);
        assert_eq!(found.items[3].found.price, None);
        assert_eq!(
            kept,
            KeptOffers {
                added: 2,
                unchanged: 0,
                skipped: 2
            }
        );
        assert_eq!(
            kept_again,
            KeptOffers {
                added: 0,
                unchanged: 2,
                skipped: 2
            }
        );
        let offers = catalog.offers().await.unwrap();
        assert_eq!(offers.len(), 2);
        assert!(
            offers
                .iter()
                .all(|offer| offer.latest.source == ProductSource::AmazonCrawler
                    && offer.latest.supplier.name == "Amazon")
        );
    });
}

#[test]
fn a_later_page_and_the_order_go_in_the_address() {
    let s = setup();
    s.server.serve(
        "/s?k=fone+bluetooth&s=price-asc-rank&page=2",
        200,
        fixture("search-page-2.html"),
    );
    s.server.serve(
        "/s?k=cafeteira+eletrica&s=exact-aware-popularity-rank",
        200,
        fixture("search-page-2.html"),
    );

    let lowest = s
        .crawler
        .search_offers(&OfferSearch {
            order: OfferOrder::LowestPrice,
            page: 2,
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
            "/s?k=fone+bluetooth&s=price-asc-rank&page=2",
            "/s?k=cafeteira+eletrica&s=exact-aware-popularity-rank"
        ]
    );
    assert!(!lowest.more);
    let basic = &lowest.items[0];
    assert_eq!(basic.id, "B0BLUETOO2");
    assert_eq!(basic.price, Some(brl("39.99")));
    assert_eq!((basic.sales, basic.rating), (1_000, None));
}

#[test]
fn crawlers_on_the_one_engine_share_the_turns_of_a_host() {
    let site = crawling::site(fixture("robots.txt"));
    let amazon = AmazonCrawler::new(site.server.url(), site.engine.clone());
    let magalu = MagaluCrawler::new(site.server.url(), site.engine);
    site.server
        .serve("/s?k=fone", 200, fixture("search-page-2.html"));
    site.server
        .serve("/busca/fone/", 200, "<html><body></body></html>");

    amazon.search_offers(&search("fone")).unwrap();
    magalu.search_offers(&search("fone")).unwrap();

    assert_eq!(site.pauses.seconds(), [3, 3]);
}

#[test]
fn a_page_without_result_blocks_finds_nothing() {
    let s = setup();
    s.server.serve(
        "/s?k=xyzzy",
        200,
        "<html><body><span>Nenhum resultado para xyzzy.</span></body></html>",
    );

    let found = s.crawler.search_offers(&search("xyzzy")).unwrap();

    assert!(found.items.is_empty());
    assert!(!found.more);
}

#[test]
fn a_result_block_past_its_bound_is_cut_and_the_page_still_reads() {
    let s = setup();
    let filler = "<span>texto</span>".repeat(10_000);
    let page = format!(
        "<div data-asin=\"B0GRANDE01\" data-component-type=\"s-search-result\">\
         <h2><span>Fone Grande</span></h2>{filler}\
         <span class=\"a-price\"><span class=\"a-offscreen\">R$ 10,00</span></span></div>\
         <div data-asin=\"B0PEQUENO1\" data-component-type=\"s-search-result\">\
         <h2><span>Fone Pequeno</span></h2>\
         <span class=\"a-price\"><span class=\"a-offscreen\">R$ 20,00</span></span></div>"
    );
    s.server.serve("/s?k=fone", 200, page);

    let found = s.crawler.search_offers(&search("fone")).unwrap();

    assert_eq!(found.items.len(), 2);
    assert_eq!(found.items[0].price, None);
    assert_eq!(found.items[1].price, Some(brl("20.00")));
}

#[test]
fn a_robot_check_counts_toward_the_breaker_whatever_its_status() {
    for status in [503, 200] {
        let s = setup();
        s.server
            .serve("/s?k=fone+bluetooth", status, fixture("robot-check.html"));

        for _ in 0..3 {
            let blocked = s.crawler.search_offers(&search("fone bluetooth"));
            assert!(
                matches!(blocked, Err(PlatformError::Refused(ref why)) if why.contains("robô")),
                "{status}: {blocked:?}"
            );
        }
        let opened_at = s.clock.now();
        let while_open = s.crawler.search_offers(&search("fone bluetooth"));

        assert!(matches!(while_open, Err(PlatformError::Refused(_))));
        assert_eq!(pages(&s.server).len(), 3);
        assert_eq!(
            s.crawler.blocked_until(),
            Some(opened_at + TimeDelta::hours(1))
        );
    }
}

#[test]
fn a_refused_access_counts_toward_the_breaker() {
    let s = setup();
    s.server.serve("/s?k=fone", 403, "Forbidden");

    for _ in 0..3 {
        let _ = s.crawler.search_offers(&search("fone"));
    }

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
            category: Some("16243862011".into()),
            ..search("fone")
        },
        OfferSearch {
            shop: Some("amazon".into()),
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
    assert!(FLAGS.contains(&AMAZON_CRAWLER));
    assert_eq!(AMAZON_CRAWLER.key, "integrations.amazon_crawler");
    assert_eq!(AMAZON_CRAWLER.kind, FlagKind::RestrictedFeature);
    assert_eq!(AMAZON_CRAWLER.phase, Phase::Three);
    assert!(AMAZON_CRAWLER.reason.contains("Condições de Uso"));
    assert!(AMAZON_CRAWLER.risk.contains("Associado"));
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(48))]

    /// Whatever the owner types, the crawler asks only for `/s?k=` with
    /// lower-case letters, digits and `+` on the search's own host.
    #[test]
    fn any_keyword_becomes_a_plain_query_on_the_search_host(keyword in "\\PC{0,40}") {
        let s = setup();
        let _ = s.crawler.search_offers(&search(&keyword));
        for path in pages(&s.server) {
            let words = path.strip_prefix("/s?k=").unwrap();
            proptest::prop_assert!(!words.is_empty());
            proptest::prop_assert!(words
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '+'));
        }
    }
}
