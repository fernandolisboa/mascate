//! The owner's listings through Mercado Livre's adapter: the adapter against
//! the fake server serving the answers in `fixtures/mercado-livre`, written
//! from the documentation (see its README), and commerce's Sync of Listings
//! end to end on a real temporary database.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    ChannelListing, ChannelStock, ListingError, ListingStatus, ListingSync, Listings, SaleFee,
    SalesChannel, Variation,
};
use mascate_integrations::MercadoLivre;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, ListingType, Money, Percentage, PlatformError};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;

const ACTIVE: &str = "/users/1234567/items/search?status=active&search_type=scan&limit=100";
const ACTIVE_NEXT: &str = "/users/1234567/items/search?status=active&search_type=scan&limit=100\
                           &scroll_id=YXBpY29yZS1pdGVtcw%3D%3D-1";
const PAUSED: &str = "/users/1234567/items/search?status=paused&search_type=scan&limit=100";
const ITEMS: &str =
    "/items?ids=MLB4100000001%2CMLB4100000003%2CMLB4100000002&include_attributes=all";
const ITEMS_AGAIN: &str =
    "/items?ids=MLB4100000001%2CMLB4100000002%2CMLB4100000003&include_attributes=all";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/mercado-livre/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

struct Fake {
    server: FakeHttpServer,
    clock: Arc<ManualClock>,
    adapter: MercadoLivre,
}

impl Fake {
    /// A connected seller account with one active page, a second empty one,
    /// and one paused page of listings.
    fn connected() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            ("ML_CLIENT_ID", "1234567890"),
            ("ML_CLIENT_SECRET", "s3cr3t"),
            ("ML_REFRESH_TOKEN", "TG-first"),
        ] {
            store.write(name, &Secret::new(value)).unwrap();
        }
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
        ));
        let adapter = MercadoLivre::new(server.url(), "Mascate/test", store, clock.clone());
        let fake = Self {
            server,
            clock,
            adapter,
        };
        fake.serve("/oauth/token", "token.json");
        fake.serve("/users/me", "users-me.json");
        fake.serve(ACTIVE, "user-items-active.json");
        fake.serve(ACTIVE_NEXT, "user-items-active-end.json");
        fake.serve(PAUSED, "user-items-paused.json");
        fake.serve(ITEMS, "items-seller.json");
        fake
    }

    fn serve(&self, path: &str, name: &str) {
        self.server.serve(path, 200, fixture(name));
    }

    fn searches(&self) -> Vec<String> {
        self.server
            .requests()
            .into_iter()
            .filter(|path| path.contains("/items/search"))
            .collect()
    }
}

fn listing_ids() -> Vec<String> {
    ["MLB4100000001", "MLB4100000003", "MLB4100000002"]
        .map(String::from)
        .to_vec()
}

#[test]
fn the_sellers_listings_are_read_page_by_page_active_then_paused() {
    let fake = Fake::connected();

    let ids = fake.adapter.listing_ids().unwrap();

    assert_eq!(ids, listing_ids());
    assert_eq!(fake.searches(), [ACTIVE, ACTIVE_NEXT, PAUSED]);
    let signed = fake
        .server
        .received()
        .into_iter()
        .filter(|request| request.path.contains("/items/search"))
        .all(|request| {
            request
                .authorization
                .is_some_and(|a| a.starts_with("Bearer "))
        });
    assert!(signed);
}

#[test]
fn each_listing_comes_with_its_details_and_each_variation_on_its_own() {
    let fake = Fake::connected();

    let listings = fake.adapter.listings(&listing_ids()).unwrap();

    let link = |id: &str, slug: &str| {
        Some(format!(
            "https://produto.mercadolivre.com.br/MLB-{}-{slug}-_JM",
            &id[3..]
        ))
    };
    let camiseta = ChannelListing {
        id: "MLB4100000003".into(),
        variation: None,
        title: "Camiseta Básica Algodão".into(),
        price: brl("49.9"),
        available_quantity: 10,
        status: ListingStatus::Active,
        link: link("MLB4100000003", "camiseta-basica-algodo"),
        listing_type: Some(ListingType::Premium),
        category: Some("MLB31447".into()),
        seller_sku: None,
    };
    assert_eq!(
        listings,
        [
            ChannelListing {
                id: "MLB4100000001".into(),
                variation: None,
                title: "Fone De Ouvido Bluetooth Tws Pro Preto".into(),
                price: brl("89.9"),
                available_quantity: 12,
                status: ListingStatus::Active,
                link: link("MLB4100000001", "fone-de-ouvido-bluetooth-tws-pro-preto"),
                listing_type: Some(ListingType::Classic),
                category: Some("MLB3697".into()),
                // From the SELLER_SKU attribute.
                seller_sku: Some("FON-BLU-TWS-001".into()),
            },
            ChannelListing {
                variation: Some(Variation {
                    id: "175000000031".into(),
                    name: "Cor: Preto · Tamanho: M".into(),
                }),
                available_quantity: 4,
                seller_sku: Some("CAM-PRE-M".into()),
                ..camiseta.clone()
            },
            ChannelListing {
                variation: Some(Variation {
                    id: "175000000032".into(),
                    name: "Cor: Branco · Tamanho: M".into(),
                }),
                available_quantity: 6,
                // From the variation's seller_custom_field.
                seller_sku: Some("CAM-BRA-M".into()),
                ..camiseta
            },
            ChannelListing {
                id: "MLB4100000002".into(),
                variation: None,
                title: "Luminária Led De Mesa Articulada".into(),
                price: brl("129"),
                available_quantity: 0,
                status: ListingStatus::Paused,
                link: link("MLB4100000002", "luminaria-led-de-mesa-articulada"),
                // A free listing: not a type the app works with.
                listing_type: None,
                category: Some("MLB1582".into()),
                seller_sku: Some("LUM-LED-001".into()),
            },
        ]
    );
}

#[test]
fn ids_are_read_twenty_at_a_time() {
    let fake = Fake::connected();
    let ids: Vec<String> = (1..=21).map(|n| format!("MLB{n}")).collect();
    let first: Vec<&str> = ids[..20].iter().map(String::as_str).collect();
    fake.server.serve(
        &format!("/items?ids={}&include_attributes=all", first.join("%2C")),
        200,
        "[]",
    );
    fake.server
        .serve("/items?ids=MLB21&include_attributes=all", 200, "[]");

    let listings = fake.adapter.listings(&ids).unwrap();

    assert!(listings.is_empty());
    let multigets = fake
        .server
        .requests()
        .into_iter()
        .filter(|path| path.starts_with("/items?"))
        .count();
    assert_eq!(multigets, 2);
}

#[test]
fn a_listing_mercado_livre_no_longer_has_is_left_out() {
    let fake = Fake::connected();
    fake.serve(ITEMS_AGAIN, "items-seller-gone.json");
    let ids = ["MLB4100000001", "MLB4100000002", "MLB4100000003"].map(String::from);

    let listings = fake.adapter.listings(&ids).unwrap();

    let read: Vec<_> = listings
        .iter()
        .map(|listing| (listing.id.as_str(), listing.status))
        .collect();
    assert_eq!(
        read,
        [
            ("MLB4100000001", ListingStatus::Closed),
            ("MLB4100000002", ListingStatus::Paused)
        ]
    );
}

#[test]
fn waiting_asked_by_mercado_livre_stops_the_read() {
    let fake = Fake::connected();
    fake.server.serve(ACTIVE, 429, fixture("error-429.json"));

    assert_eq!(fake.adapter.listing_ids(), Err(PlatformError::RateLimited));
}

#[test]
fn without_a_connection_nothing_is_read() {
    let server = FakeHttpServer::start();
    let adapter = MercadoLivre::new(
        server.url(),
        "Mascate/test",
        Arc::new(MemorySecretStore::default()),
        Arc::new(ManualClock::at(Utc::now())),
    );

    assert_eq!(adapter.listing_ids(), Err(PlatformError::NotConnected));
    assert!(server.requests().is_empty());
}

#[test]
fn a_sync_imports_the_sellers_listings_and_the_next_one_follows_mercado_livre() {
    block_on(async {
        let fake = Fake::connected();
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&dir.path().join("m.db")).await.unwrap());
        migrate(
            &database,
            fake.clock.as_ref(),
            &[mascate_commerce::MIGRATIONS],
        )
        .await
        .unwrap();
        let listings = Listings::new(
            database,
            fake.clock.clone(),
            Arc::new(SequentialIds::default()),
        );

        let first = listings.sync(&fake.adapter).await.unwrap();
        let again = listings.sync(&fake.adapter).await.unwrap();
        // The fone closes with a new price, the shirt is deleted, the lamp
        // stays paused; none is active or paused but the lamp.
        fake.clock.advance(TimeDelta::hours(1));
        fake.serve(ACTIVE, "user-items-active-end.json");
        fake.serve(PAUSED, "user-items-active-end.json");
        fake.serve(ITEMS_AGAIN, "items-seller-gone.json");
        let later = listings.sync(&fake.adapter).await.unwrap();

        assert_eq!(
            first,
            ListingSync {
                read: 4,
                new: 4,
                changed: 0,
                gone: 0
            }
        );
        assert_eq!(
            again,
            ListingSync {
                read: 4,
                ..ListingSync::default()
            }
        );
        assert_eq!(
            later,
            ListingSync {
                read: 2,
                new: 0,
                changed: 1,
                gone: 2
            }
        );
        let state: Vec<_> = listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .map(|listing| {
                (
                    listing.listed.id,
                    listing.listed.variation.map(|v| v.id),
                    listing.listed.status,
                    listing.listed.price,
                )
            })
            .collect();
        // By title, then variation.
        assert_eq!(
            state,
            [
                (
                    "MLB4100000003".into(),
                    Some("175000000032".into()),
                    ListingStatus::Closed,
                    brl("49.9")
                ),
                (
                    "MLB4100000003".into(),
                    Some("175000000031".into()),
                    ListingStatus::Closed,
                    brl("49.9")
                ),
                (
                    "MLB4100000001".into(),
                    None,
                    ListingStatus::Closed,
                    brl("79.9")
                ),
                (
                    "MLB4100000002".into(),
                    None,
                    ListingStatus::Paused,
                    brl("129")
                ),
            ]
        );
    });
}

fn fee_at(price: &str) -> String {
    format!(
        "/sites/MLB/listing_prices?price={price}&listing_type_id=gold_special&category_id=MLB3697"
    )
}

fn percent(text: &str) -> Percentage {
    Percentage::new(Decimal::from_str(text).unwrap()).unwrap()
}

#[test]
fn the_sale_fee_comes_as_its_percentage_and_its_fixed_part() {
    let fake = Fake::connected();
    fake.serve(&fee_at("49.90"), "listing-prices-fixed-fee.json");
    fake.serve(&fee_at("100"), "listing-prices.json");

    let below = fake
        .adapter
        .sale_fee("MLB3697", brl("49.90"), ListingType::Classic)
        .unwrap();
    let above = fake
        .adapter
        .sale_fee("MLB3697", brl("100"), ListingType::Classic)
        .unwrap();

    assert_eq!(
        below,
        SaleFee {
            rate: percent("12"),
            fixed: brl("6.25")
        }
    );
    assert_eq!(
        above,
        SaleFee {
            rate: percent("14"),
            fixed: brl("0")
        }
    );
}

#[test]
fn without_its_details_the_whole_sale_fee_is_a_share_of_the_price() {
    let fake = Fake::connected();
    fake.serve(&fee_at("100"), "listing-prices-no-details.json");

    let fee = fake
        .adapter
        .sale_fee("MLB3697", brl("100"), ListingType::Classic)
        .unwrap();

    assert_eq!(
        fee,
        SaleFee {
            rate: percent("14"),
            fixed: brl("0")
        }
    );
}

#[test]
fn a_new_price_is_put_on_the_listing_in_cents() {
    let fake = Fake::connected();
    fake.serve("/items/MLB4100000001", "item-MLB4100000001.json");

    fake.adapter
        .set_price("MLB4100000001", brl("91.666"))
        .unwrap();

    let put: Vec<_> = fake
        .server
        .received()
        .into_iter()
        .filter(|request| request.method == "PUT")
        .collect();
    assert_eq!(put.len(), 1);
    assert_eq!(put[0].path, "/items/MLB4100000001");
    assert_eq!(put[0].body, r#"{"price":91.67}"#);
    assert!(
        put[0]
            .authorization
            .as_deref()
            .is_some_and(|a| a.starts_with("Bearer "))
    );
}

#[test]
fn every_variation_mercado_livre_has_now_takes_the_new_price() {
    let fake = Fake::connected();
    // A third variation, added in Mercado Livre after the last Sync: left
    // out of the request, Mercado Livre would delete it.
    fake.serve("/items/MLB4100000003", "item-MLB4100000003.json");

    fake.adapter
        .set_price("MLB4100000003", brl("59.9"))
        .unwrap();

    let put = fake
        .server
        .received()
        .into_iter()
        .find(|request| request.method == "PUT")
        .unwrap();
    assert_eq!(
        put.body,
        r#"{"variations":[{"id":175000000031,"price":59.9},{"id":175000000032,"price":59.9},{"id":175000000033,"price":59.9}]}"#
    );
}

#[test]
fn a_price_mercado_livre_refuses_says_why() {
    let fake = Fake::connected();
    fake.serve("/items/MLB4100000001", "item-MLB4100000001.json");
    fake.server.serve_method(
        "PUT",
        "/items/MLB4100000001",
        400,
        fixture("error-400-price.json"),
    );

    let refused = fake.adapter.set_price("MLB4100000001", brl("1"));

    assert_eq!(
        refused,
        Err(PlatformError::Refused(
            "The price is lower than the minimum allowed for the category".into()
        ))
    );
}

#[test]
fn an_id_that_is_not_mercado_livres_never_reaches_the_api() {
    let fake = Fake::connected();

    let refused = fake.adapter.set_price("../oauth/token", brl("10"));

    assert_eq!(refused, Err(PlatformError::NotFound));
    assert!(
        fake.server
            .requests()
            .iter()
            .all(|path| !path.starts_with("/items"))
    );
}

#[test]
fn an_approved_price_reaches_mercado_livre_and_the_listing_keeps_it() {
    block_on(async {
        let fake = Fake::connected();
        fake.serve("/items/MLB4100000003", "item-MLB4100000003.json");
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&dir.path().join("m.db")).await.unwrap());
        migrate(
            &database,
            fake.clock.as_ref(),
            &[mascate_commerce::MIGRATIONS],
        )
        .await
        .unwrap();
        let listings = Listings::new(
            database,
            fake.clock.clone(),
            Arc::new(SequentialIds::default()),
        );
        listings.sync(&fake.adapter).await.unwrap();
        let shirt = listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .find(|listing| listing.listed.id == "MLB4100000003")
            .unwrap();

        let changed = listings
            .change_price(shirt.id, brl("59.9"), &fake.adapter)
            .await
            .unwrap();

        assert_eq!(changed.len(), 2);
        assert!(
            changed
                .iter()
                .all(|listing| listing.listed.price == brl("59.9"))
        );
        fake.server.serve_method(
            "PUT",
            "/items/MLB4100000003",
            400,
            fixture("error-400-price.json"),
        );
        let refused = listings
            .change_price(shirt.id, brl("1"), &fake.adapter)
            .await;
        assert!(matches!(
            refused,
            Err(ListingError::Platform(PlatformError::Refused(_)))
        ));
        assert_eq!(
            listings.listing(shirt.id).await.unwrap().listed.price,
            brl("59.9")
        );
    });
}

impl Fake {
    /// The bodies of the PUTs Mercado Livre received, in order.
    fn puts(&self) -> Vec<(String, String)> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.method == "PUT")
            .map(|request| (request.path, request.body))
            .collect()
    }
}

fn stock(variation: Option<&str>, available_quantity: u32) -> ChannelStock {
    ChannelStock {
        variation: variation.map(String::from),
        available_quantity,
    }
}

#[test]
fn the_stock_of_a_listing_without_variations_is_its_available_quantity() {
    let fake = Fake::connected();
    fake.serve("/items/MLB4100000001", "item-MLB4100000001.json");

    fake.adapter
        .set_stock("MLB4100000001", &[stock(None, 7)])
        .unwrap();

    assert_eq!(
        fake.puts(),
        [(
            "/items/MLB4100000001".to_owned(),
            r#"{"available_quantity":7}"#.to_owned()
        )]
    );
}

#[test]
fn the_stock_goes_to_the_variations_named_and_every_other_one_is_kept() {
    let fake = Fake::connected();
    fake.serve("/items/MLB4100000003", "item-MLB4100000003.json");

    fake.adapter
        .set_stock(
            "MLB4100000003",
            &[
                stock(Some("175000000032"), 0),
                stock(Some("175000000031"), 9),
            ],
        )
        .unwrap();

    // Left out, the third variation would be deleted.
    assert_eq!(
        fake.puts(),
        [(
            "/items/MLB4100000003".to_owned(),
            r#"{"variations":[{"id":175000000031,"available_quantity":9},{"id":175000000032,"available_quantity":0},{"id":175000000033}]}"#
                .to_owned()
        )]
    );
}

#[test]
fn a_stock_for_a_variation_mercado_livre_no_longer_has_is_never_sent() {
    let fake = Fake::connected();
    fake.serve("/items/MLB4100000003", "item-MLB4100000003.json");
    fake.serve("/items/MLB4100000001", "item-MLB4100000001.json");

    let gone = fake
        .adapter
        .set_stock("MLB4100000003", &[stock(Some("175000000099"), 2)]);
    let whole_on_variations = fake.adapter.set_stock("MLB4100000003", &[stock(None, 2)]);
    let variation_on_whole = fake
        .adapter
        .set_stock("MLB4100000001", &[stock(Some("175000000031"), 2)]);

    assert_eq!(gone, Err(PlatformError::NotFound));
    assert_eq!(whole_on_variations, Err(PlatformError::NotFound));
    assert_eq!(variation_on_whole, Err(PlatformError::NotFound));
    assert!(fake.puts().is_empty());
}

#[test]
fn pausing_and_reactivating_put_the_listings_status() {
    let fake = Fake::connected();
    fake.serve("/items/MLB4100000003", "item-MLB4100000003.json");

    fake.adapter.pause("MLB4100000003").unwrap();
    fake.adapter.activate("MLB4100000003").unwrap();

    assert_eq!(
        fake.puts(),
        [
            (
                "/items/MLB4100000003".to_owned(),
                r#"{"status":"paused"}"#.to_owned()
            ),
            (
                "/items/MLB4100000003".to_owned(),
                r#"{"status":"active"}"#.to_owned()
            ),
        ]
    );
}

#[test]
fn stock_and_status_never_reach_a_path_that_is_not_a_listings() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter.set_stock("../users/me", &[stock(None, 1)]),
        Err(PlatformError::NotFound)
    );
    assert_eq!(
        fake.adapter.pause("MLB1/../x"),
        Err(PlatformError::NotFound)
    );
    assert_eq!(fake.adapter.activate(""), Err(PlatformError::NotFound));
    assert!(
        fake.server
            .requests()
            .iter()
            .all(|path| !path.starts_with("/items"))
    );
}

#[test]
fn a_stock_mercado_livre_refuses_says_why() {
    let fake = Fake::connected();
    fake.serve("/items/MLB4100000001", "item-MLB4100000001.json");
    fake.server.serve_method(
        "PUT",
        "/items/MLB4100000001",
        400,
        fixture("error-400-stock.json"),
    );

    let refused = fake.adapter.set_stock("MLB4100000001", &[stock(None, 3)]);

    assert_eq!(
        refused,
        Err(PlatformError::Refused(
            "The item is closed and its stock cannot change".into()
        ))
    );
}

#[test]
fn a_listing_paused_from_the_app_is_paused_in_mercado_livre_and_on_screen() {
    block_on(async {
        let fake = Fake::connected();
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&dir.path().join("m.db")).await.unwrap());
        migrate(
            &database,
            fake.clock.as_ref(),
            &[mascate_commerce::MIGRATIONS],
        )
        .await
        .unwrap();
        let listings = Listings::new(
            database,
            fake.clock.clone(),
            Arc::new(SequentialIds::default()),
        );
        listings.sync(&fake.adapter).await.unwrap();
        fake.serve("/items/MLB4100000003", "item-MLB4100000003.json");
        let shirt = listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .find(|listing| listing.listed.id == "MLB4100000003")
            .unwrap();

        let paused = listings.pause(shirt.id, &fake.adapter).await.unwrap();

        assert_eq!(paused.len(), 2);
        assert!(
            paused
                .iter()
                .all(|listing| listing.listed.status == ListingStatus::Paused)
        );
        fake.server.serve_method(
            "PUT",
            "/items/MLB4100000003",
            403,
            fixture("error-403.json"),
        );
        let refused = listings.reactivate(shirt.id, &fake.adapter).await;
        assert!(matches!(
            refused,
            Err(ListingError::Platform(PlatformError::Refused(_)))
        ));
        assert_eq!(
            listings.listing(shirt.id).await.unwrap().listed.status,
            ListingStatus::Paused
        );
    });
}
