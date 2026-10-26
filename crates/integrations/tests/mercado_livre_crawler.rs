//! The crawler engine and the Mercado Livre crawler against the fake server
//! serving search pages written by hand (see the fixtures' README).

use std::str::FromStr;
use std::sync::Arc;

use chrono::TimeDelta;
use futures::executor::block_on;
use mascate_catalog::{Catalog, KeptOffers, OfferOrder, OfferSearch, OfferSource, ProductSource};
use mascate_integrations::{FLAGS, MERCADO_LIVRE_CRAWLER, MercadoLivreCrawler};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, PlatformError};
use mascate_platform::testing::FakeHttpServer;
use mascate_platform::{Database, FlagKind, Phase, migrate};
use rust_decimal::Decimal;

mod crawling;

use crawling::{ClockPause, brl, pages};

fn fixture(name: &str) -> String {
    crawling::fixture("mercado-livre-crawler", name)
}

struct Setup {
    server: FakeHttpServer,
    clock: Arc<ManualClock>,
    pauses: Arc<ClockPause>,
    crawler: MercadoLivreCrawler,
}

/// The crawler at the fake server, with its robots.txt, on 2026-10-16
/// 12:00 UTC.
fn setup() -> Setup {
    let site = crawling::site(fixture("robots.txt"));
    let crawler = MercadoLivreCrawler::new(site.server.url(), site.engine);
    Setup {
        server: site.server,
        clock: site.clock,
        pauses: site.pauses,
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
fn a_search_page_yields_found_offers_with_id_title_link_image_and_price() {
    let s = setup();
    s.server
        .serve("/fone-bluetooth", 200, fixture("search.html"));

    let found = s.crawler.search_offers(&search("Fone Bluetooth")).unwrap();

    assert!(found.more);
    let ids: Vec<&str> = found.items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, ["MLB4012345678", "MLB4023456789", "MLB4056789012"]);
    let fone = &found.items[0];
    assert_eq!(fone.title, "Fone de Ouvido Bluetooth TWS");
    assert_eq!(
        fone.link,
        "https://produto.mercadolivre.com.br/MLB-4012345678-fone-de-ouvido-bluetooth-tws-_JM"
    );
    assert_eq!(
        fone.image.as_deref(),
        Some("https://http2.mlstatic.com/D_NQ_NP_612345-MLA79876543210_102024-O.jpg")
    );
    assert_eq!(fone.price, Some(brl("89.9")));
    assert_eq!(fone.highest_price, Some(brl("89.9")));
    assert_eq!(fone.shop.name, "Casa dos Fones");
    assert_eq!(fone.shop.id, "casa-dos-fones");
    assert_eq!(fone.sales, 10_000);
    assert_eq!(fone.rating, Some(Decimal::from_str("4.8").unwrap()));
    assert_eq!(fone.commission, None);

    let official = &found.items[1];
    assert_eq!(
        official.link,
        "https://www.mercadolivre.com.br/fone-de-ouvido-xiaomi-redmi-buds/p/MLB22446688"
    );
    assert_eq!(official.shop.name, "Xiaomi Loja Oficial");
    assert_eq!(official.price, Some(brl("149")));
    assert_eq!((official.sales, official.rating), (500, None));

    let unnamed = &found.items[2];
    assert_eq!(unnamed.title, "Fone com Fio P2");
    assert_eq!(unnamed.shop.name, "Mercado Livre");
    assert_eq!(unnamed.price, Some(brl("24.5")));
    assert_eq!((unnamed.image.as_deref(), unnamed.sales), (None, 0));
}

#[test]
fn a_later_page_and_the_lowest_price_go_in_the_address() {
    let s = setup();
    s.server.serve(
        "/fone-bluetooth_Desde_49_OrderId_PRICE",
        200,
        fixture("search-page-2.html"),
    );

    let found = s
        .crawler
        .search_offers(&OfferSearch {
            order: OfferOrder::LowestPrice,
            page: 2,
            ..search("fone  bluetooth!")
        })
        .unwrap();

    assert_eq!(pages(&s.server), ["/fone-bluetooth_Desde_49_OrderId_PRICE"]);
    assert!(!found.more);
    let gamer = &found.items[1];
    assert_eq!(gamer.title, "Fone Gamer RGB {com} \"chaves\"");
    assert_eq!(gamer.price, Some(brl("199.99")));
    assert_eq!(gamer.sales, 1_500);
    assert_eq!(gamer.rating, Some(Decimal::from_str("4.5").unwrap()));
    assert_eq!(found.items[0].shop.name, "Casa dos Fones");
}

#[test]
fn accents_and_case_leave_the_address() {
    let s = setup();
    s.server
        .serve("/cafeteira-eletrica", 200, fixture("search-page-2.html"));

    s.crawler
        .search_offers(&search("Cafeteira Elétrica"))
        .unwrap();

    assert_eq!(pages(&s.server), ["/cafeteira-eletrica"]);
}

#[test]
fn a_page_without_its_state_is_read_from_its_json_ld() {
    let s = setup();
    s.server.serve(
        "/caixa-de-som-bluetooth",
        200,
        fixture("search-json-ld.html"),
    );

    let found = s
        .crawler
        .search_offers(&search("caixa de som bluetooth"))
        .unwrap();

    assert!(found.more);
    let ids: Vec<&str> = found.items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, ["MLB4055555555", "MLB22334455"]);
    let speaker = &found.items[0];
    assert_eq!(speaker.price, Some(brl("199.90")));
    assert_eq!(speaker.shop.name, "Som Total");
    assert_eq!(speaker.rating, Some(Decimal::from_str("4.6").unwrap()));
    assert_eq!(
        speaker.image.as_deref(),
        Some("https://http2.mlstatic.com/D_NQ_NP_777777-MLA70000000000_012025-O.jpg")
    );
    let portable = &found.items[1];
    assert_eq!(portable.price, Some(brl("120")));
    assert_eq!(portable.highest_price, Some(brl("180")));
    assert_eq!(portable.shop.name, "Mercado Livre");
    assert_eq!(
        portable.image.as_deref(),
        Some("https://http2.mlstatic.com/D_NQ_NP_888888-MLA71111111111_022025-O.jpg")
    );
}

#[test]
fn found_offers_kept_twice_are_one_supplier_offer_from_the_crawler() {
    block_on(async {
        let s = setup();
        s.server
            .serve("/fone-bluetooth", 200, fixture("search.html"));
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
        let fone = vec![found.items[0].found.clone()];
        let kept = catalog
            .keep_found_offers(s.crawler.source(), &fone)
            .await
            .unwrap();
        let kept_again = catalog
            .keep_found_offers(s.crawler.source(), &fone)
            .await
            .unwrap();
        let again = catalog
            .search_offers(&s.crawler, &search("fone bluetooth"))
            .await
            .unwrap();

        assert_eq!(kept.added, 1);
        assert_eq!(
            kept_again,
            KeptOffers {
                added: 0,
                unchanged: 1,
                skipped: 0
            }
        );
        assert!(again.items[0].up_to_date());
        let offers = catalog.offers().await.unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].latest.source, ProductSource::MercadoLivreCrawler);
        assert_eq!(offers[0].latest.supplier.name, "Casa dos Fones");
    });
}

#[test]
fn a_path_robots_txt_disallows_is_never_requested() {
    let s = setup();
    s.server.serve(
        "/robots.txt",
        200,
        "User-agent: *\nDisallow: /fone\nAllow: /fone-bluetooth$\n",
    );
    s.server
        .serve("/fone-bluetooth", 200, fixture("search.html"));
    s.server.serve("/fone-com-fio", 200, fixture("search.html"));

    let refused = s.crawler.search_offers(&search("fone com fio"));
    let allowed = s.crawler.search_offers(&search("fone bluetooth"));

    assert!(matches!(refused, Err(PlatformError::Refused(why)) if why.contains("robots.txt")));
    assert!(allowed.is_ok());
    assert_eq!(pages(&s.server), ["/fone-bluetooth"]);
}

#[test]
fn the_group_that_names_the_mascate_wins_over_every_robot() {
    let s = setup();
    s.server.serve(
        "/robots.txt",
        200,
        "User-agent: *\nAllow: /\n\nUser-agent: Googlebot\nUser-agent: mascate\nDisallow: /\n",
    );
    s.server
        .serve("/fone-bluetooth", 200, fixture("search.html"));

    let refused = s.crawler.search_offers(&search("fone bluetooth"));

    assert!(matches!(refused, Err(PlatformError::Refused(_))));
    assert!(pages(&s.server).is_empty());
}

#[test]
fn requests_name_the_mascate() {
    let s = setup();
    s.server
        .serve("/fone-bluetooth", 200, fixture("search.html"));

    s.crawler.search_offers(&search("fone bluetooth")).unwrap();

    for request in s.server.received() {
        let agent = request
            .headers
            .iter()
            .find(|(name, _)| name == "user-agent")
            .map(|(_, value)| value.as_str());
        assert_eq!(agent, Some("Mascate/0.5.0"), "{}", request.path);
    }
}

#[test]
fn two_searches_within_the_interval_wait_for_it() {
    let s = setup();
    s.server
        .serve("/fone-bluetooth", 200, fixture("search.html"));
    s.server.serve("/fone-com-fio", 200, fixture("search.html"));

    s.crawler.search_offers(&search("fone bluetooth")).unwrap();
    s.clock.advance(TimeDelta::seconds(1));
    s.crawler.search_offers(&search("fone com fio")).unwrap();

    // robots.txt, then 3 s to the first search, then 2 s more to the second.
    assert_eq!(s.pauses.seconds(), [3, 2]);
}

#[test]
fn crawl_delay_stretches_the_interval() {
    let s = setup();
    s.server.serve(
        "/robots.txt",
        200,
        "User-agent: *\nCrawl-delay: 10\nAllow: /\n",
    );
    s.server
        .serve("/fone-bluetooth", 200, fixture("search.html"));
    s.server.serve("/fone-com-fio", 200, fixture("search.html"));

    s.crawler.search_offers(&search("fone bluetooth")).unwrap();
    s.crawler.search_offers(&search("fone com fio")).unwrap();

    assert_eq!(s.pauses.seconds(), [10, 10]);
}

#[test]
fn a_crawl_delay_past_two_minutes_is_refused_rather_than_waited_for() {
    let s = setup();
    s.server
        .serve("/robots.txt", 200, "User-agent: *\nCrawl-delay: 3600\n");
    s.server
        .serve("/fone-bluetooth", 200, fixture("search.html"));

    let refused = s.crawler.search_offers(&search("fone bluetooth"));

    assert!(matches!(refused, Err(PlatformError::Refused(why)) if why.contains("3600 s")));
    assert!(pages(&s.server).is_empty());
    assert!(s.pauses.seconds().is_empty());
}

#[test]
fn the_same_search_within_an_hour_is_answered_from_memory() {
    let s = setup();
    s.server
        .serve("/fone-bluetooth", 200, fixture("search.html"));

    let first = s.crawler.search_offers(&search("fone bluetooth")).unwrap();
    s.clock.advance(TimeDelta::minutes(59));
    let again = s.crawler.search_offers(&search("fone bluetooth")).unwrap();
    let requests_within_the_hour = s.server.requests().len();
    s.clock.advance(TimeDelta::minutes(2));
    s.crawler.search_offers(&search("fone bluetooth")).unwrap();

    assert_eq!(first, again);
    assert_eq!(requests_within_the_hour, 2);
    // After the hour, robots.txt and the page are read again.
    assert_eq!(
        s.server.requests(),
        [
            "/robots.txt",
            "/fone-bluetooth",
            "/robots.txt",
            "/fone-bluetooth"
        ]
    );
}

#[test]
fn three_blocks_in_a_row_open_the_breaker_for_an_hour() {
    let s = setup();
    s.server.serve("/fone-bluetooth", 403, "Forbidden");

    for _ in 0..3 {
        let blocked = s.crawler.search_offers(&search("fone bluetooth"));
        assert!(matches!(blocked, Err(PlatformError::Refused(_))));
    }
    let opened_at = s.clock.now();
    let while_open = s.crawler.search_offers(&search("fone bluetooth"));

    assert!(matches!(while_open, Err(PlatformError::Refused(_))));
    assert_eq!(pages(&s.server).len(), 3);
    assert_eq!(
        s.crawler.blocked_until(),
        Some(opened_at + TimeDelta::hours(1))
    );

    s.clock.advance(TimeDelta::hours(1));
    s.server
        .serve("/fone-bluetooth", 200, fixture("search.html"));
    assert_eq!(s.crawler.blocked_until(), None);
    assert!(s.crawler.search_offers(&search("fone bluetooth")).is_ok());
    assert_eq!(pages(&s.server).len(), 4);
}

#[test]
fn a_success_between_blocks_resets_the_count() {
    let s = setup();
    s.server.serve("/fone-bluetooth", 429, "Too Many Requests");
    s.server.serve("/fone-com-fio", 200, fixture("search.html"));

    let _ = s.crawler.search_offers(&search("fone bluetooth"));
    let _ = s.crawler.search_offers(&search("fone bluetooth"));
    s.crawler.search_offers(&search("fone com fio")).unwrap();
    let _ = s.crawler.search_offers(&search("fone bluetooth"));
    let _ = s.crawler.search_offers(&search("fone bluetooth"));

    assert_eq!(s.crawler.blocked_until(), None);
    assert_eq!(pages(&s.server).len(), 5);
}

#[test]
fn a_redirect_to_the_login_is_a_block_and_the_login_is_never_read() {
    let s = setup();
    s.server.redirect(
        "/fone-bluetooth",
        "/jms/mlb/lgz/login?go=https%3A%2F%2Flista.mercadolivre.com.br%2Ffone-bluetooth",
    );

    for _ in 0..3 {
        let _ = s.crawler.search_offers(&search("fone bluetooth"));
    }

    assert!(s.crawler.blocked_until().is_some());
    assert!(
        pages(&s.server)
            .iter()
            .all(|path| path == "/fone-bluetooth")
    );
}

#[test]
fn a_verification_page_is_a_block() {
    let s = setup();
    s.server
        .serve("/fone-bluetooth", 200, fixture("verification.html"));

    for _ in 0..3 {
        let blocked = s.crawler.search_offers(&search("fone bluetooth"));
        assert!(matches!(blocked, Err(PlatformError::Refused(_))));
    }

    assert!(s.crawler.blocked_until().is_some());
}

#[test]
fn a_plain_redirect_is_followed_within_robots_txt() {
    let s = setup();
    s.server.redirect("/fone-bluetooth", "/fone-bluetooth-novo");
    s.server
        .serve("/fone-bluetooth-novo", 200, fixture("search.html"));
    s.server
        .redirect("/fone-com-fio", "/fone-com-fio_NoIndex_True");

    let followed = s.crawler.search_offers(&search("fone bluetooth")).unwrap();
    let disallowed = s.crawler.search_offers(&search("fone com fio"));

    assert_eq!(followed.items.len(), 3);
    assert!(matches!(disallowed, Err(PlatformError::Refused(why)) if why.contains("robots.txt")));
    assert_eq!(
        pages(&s.server),
        ["/fone-bluetooth", "/fone-bluetooth-novo", "/fone-com-fio"]
    );
}

#[test]
fn a_site_without_robots_txt_may_be_read() {
    let s = setup();
    s.server.serve("/robots.txt", 404, "Not Found");
    s.server
        .serve("/fone-bluetooth", 200, fixture("search.html"));

    assert!(s.crawler.search_offers(&search("fone bluetooth")).is_ok());
}

#[test]
fn a_robots_txt_the_site_refuses_is_a_block_and_nothing_else_is_read() {
    let s = setup();
    s.server.serve("/robots.txt", 403, "Forbidden");
    s.server
        .serve("/fone-bluetooth", 200, fixture("search.html"));

    for _ in 0..3 {
        let blocked = s.crawler.search_offers(&search("fone bluetooth"));
        assert!(matches!(blocked, Err(PlatformError::Refused(_))));
    }

    assert!(pages(&s.server).is_empty());
    assert!(s.crawler.blocked_until().is_some());
}

#[test]
fn a_failure_on_the_site_is_neither_kept_nor_a_block() {
    let s = setup();
    s.server
        .serve("/fone-bluetooth", 503, "Service Unavailable");

    for _ in 0..3 {
        let failed = s.crawler.search_offers(&search("fone bluetooth"));
        assert!(matches!(failed, Err(PlatformError::Failed(_))));
    }

    assert_eq!(s.crawler.blocked_until(), None);
    assert_eq!(pages(&s.server).len(), 3);
}

#[test]
fn the_crawler_offers_no_shops_categories_or_orders_the_page_lacks() {
    let s = setup();

    assert!(!s.crawler.finds_shops());
    assert_eq!(
        s.crawler.orders(),
        [OfferOrder::Relevance, OfferOrder::LowestPrice]
    );
    assert!(s.crawler.search_shops("fones", 1).is_err());
    for refused in [
        OfferSearch {
            order: OfferOrder::Sales,
            ..search("fone")
        },
        OfferSearch {
            category: Some("MLB1055".into()),
            ..search("fone")
        },
        OfferSearch {
            shop: Some("casa-dos-fones".into()),
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
fn the_crawler_is_a_restricted_feature_of_phase_3_that_starts_off() {
    assert!(FLAGS.contains(&MERCADO_LIVRE_CRAWLER));
    assert_eq!(
        MERCADO_LIVRE_CRAWLER.key,
        "integrations.mercado_livre_crawler"
    );
    assert_eq!(MERCADO_LIVRE_CRAWLER.kind, FlagKind::RestrictedFeature);
    assert_eq!(MERCADO_LIVRE_CRAWLER.phase, Phase::Three);
    assert!(MERCADO_LIVRE_CRAWLER.reason.contains("7.6"));
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(48))]

    /// Whatever the owner types, the crawler asks only for a path of
    /// lower-case letters, digits and dashes on the search's own host.
    #[test]
    fn any_keyword_becomes_a_plain_path_on_the_search_host(keyword in "\\PC{0,40}") {
        let s = setup();
        let _ = s.crawler.search_offers(&search(&keyword));
        for path in pages(&s.server) {
            let words = path.strip_prefix('/').unwrap();
            proptest::prop_assert!(!words.is_empty());
            proptest::prop_assert!(words
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        }
    }
}
