//! On-Order Listings through Mercado Livre's adapter (#207, ADR 0038): the
//! days to be ready go as the sale term `MANUFACTURING_TIME`, against the
//! fake server serving the answers in `fixtures/mercado-livre`, written
//! from the documentation (see its README), and the mark end to end on a
//! real temporary database.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{ListingAvailability, ListingError, Listings, OnOrderTerms};
use mascate_integrations::MercadoLivre;
use mascate_kernel::PlatformError;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};

const ITEM: &str = "/items/MLB4100000001";
const SALE_TERMS: &str = "/categories/MLB3697/sale_terms";
const SET_15_DAYS: &str = r#"{"sale_terms":[{"id":"MANUFACTURING_TIME","value_name":"15 días"}]}"#;
const ACTIVE: &str = "/users/1234567/items/search?status=active&search_type=scan&limit=100";
const ACTIVE_NEXT: &str = "/users/1234567/items/search?status=active&search_type=scan&limit=100\
                           &scroll_id=YXBpY29yZS1pdGVtcw%3D%3D-1";
const PAUSED: &str = "/users/1234567/items/search?status=paused&search_type=scan&limit=100";
const ITEMS: &str =
    "/items?ids=MLB4100000001%2CMLB4100000003%2CMLB4100000002&include_attributes=all";
const TAKEN_OFF: &str =
    r#"{"sale_terms":[{"id":"MANUFACTURING_TIME","value_id":null,"value_name":null}]}"#;

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/mercado-livre/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

struct Fake {
    server: FakeHttpServer,
    clock: Arc<ManualClock>,
    adapter: MercadoLivre,
}

impl Fake {
    /// A connected seller account whose listing MLB4100000001 is in a
    /// category that takes the days, and that takes them.
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
            Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap(),
        ));
        let adapter = MercadoLivre::new(server.url(), "Mascate/test", store, clock.clone());
        server.serve("/oauth/token", 200, fixture("token.json"));
        server.serve(ITEM, 200, fixture("item-MLB4100000001.json"));
        server.serve(SALE_TERMS, 200, fixture("category-sale-terms-MLB3697.json"));
        server.serve_method(
            "PUT",
            ITEM,
            200,
            fixture("item-manufacturing-time-set.json"),
        );
        Self {
            server,
            clock,
            adapter,
        }
    }

    /// The PUTs Mercado Livre received, with their bodies, in order.
    fn puts(&self) -> Vec<(String, String)> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.method == "PUT")
            .map(|request| (request.path, request.body))
            .collect()
    }
}

#[test]
fn the_days_go_as_manufacturing_time_in_the_unit_the_category_names() {
    let fake = Fake::connected();

    fake.adapter
        .set_availability("MLB4100000001", Some(15))
        .unwrap();

    assert_eq!(fake.puts(), [(ITEM.to_owned(), SET_15_DAYS.to_owned())]);
    assert!(fake.server.requests().contains(&SALE_TERMS.to_owned()));
}

#[test]
fn taking_the_days_off_sends_the_term_with_null_values() {
    let fake = Fake::connected();

    fake.adapter
        .set_availability("MLB4100000001", None)
        .unwrap();

    assert_eq!(fake.puts(), [(ITEM.to_owned(), TAKEN_OFF.to_owned())]);
    assert!(!fake.server.requests().contains(&SALE_TERMS.to_owned()));
}

#[test]
fn a_listing_with_flex_or_in_full_is_refused_with_the_reason() {
    let fake = Fake::connected();
    fake.server.serve_method(
        "PUT",
        ITEM,
        200,
        fixture("item-manufacturing-time-dropped.json"),
    );

    let refused = fake.adapter.set_availability("MLB4100000001", Some(15));

    assert_eq!(
        refused,
        Err(PlatformError::Refused(
            "anúncio com Mercado Envios Flex ou no Full não aceita prazo de disponibilidade \
             (MANUFACTURING_TIME will be deleted because the item is in fulfillment)"
                .into()
        ))
    );
}

#[test]
fn a_category_without_the_term_is_refused_before_any_change() {
    let fake = Fake::connected();
    fake.server.serve(
        SALE_TERMS,
        200,
        fixture("category-sale-terms-without-manufacturing-time.json"),
    );

    let refused = fake.adapter.set_availability("MLB4100000001", Some(15));

    assert_eq!(
        refused,
        Err(PlatformError::Refused(
            "a categoria deste anúncio não aceita prazo de disponibilidade".into()
        ))
    );
    assert!(fake.puts().is_empty());
}

#[test]
fn days_mercado_livre_turns_down_say_why() {
    let fake = Fake::connected();
    fake.server.serve_method(
        "PUT",
        ITEM,
        400,
        fixture("error-400-manufacturing-time.json"),
    );

    let refused = fake.adapter.set_availability("MLB4100000001", Some(61));

    assert_eq!(
        refused,
        Err(PlatformError::Refused(
            "MANUFACTURING_TIME cannot be greater than 60 days".into()
        ))
    );
}

#[test]
fn a_forbidden_or_rate_limited_answer_is_told_apart() {
    let fake = Fake::connected();
    fake.server
        .serve_method("PUT", ITEM, 403, fixture("error-403.json"));
    assert_eq!(
        fake.adapter.set_availability("MLB4100000001", None),
        Err(PlatformError::Refused("forbidden".into()))
    );

    fake.server
        .serve_method("PUT", ITEM, 429, fixture("error-429.json"));
    assert_eq!(
        fake.adapter.set_availability("MLB4100000001", None),
        Err(PlatformError::RateLimited)
    );
    fake.server
        .serve(SALE_TERMS, 429, fixture("error-429.json"));
    assert_eq!(
        fake.adapter.set_availability("MLB4100000001", Some(15)),
        Err(PlatformError::RateLimited)
    );
}

#[test]
fn days_turned_down_for_an_old_token_go_once_more_with_a_renewed_one() {
    let fake = Fake::connected();
    fake.server
        .serve_method("PUT", ITEM, 401, r#"{"message":"invalid_token"}"#);
    fake.server.then_serve(
        &format!("PUT {ITEM}"),
        200,
        fixture("item-manufacturing-time-set.json"),
    );

    fake.adapter
        .set_availability("MLB4100000001", Some(15))
        .unwrap();

    assert_eq!(fake.puts().len(), 2);
    let tokens = fake
        .server
        .requests()
        .into_iter()
        .filter(|path| path == "/oauth/token")
        .count();
    assert_eq!(tokens, 2);
}

#[test]
fn an_id_that_could_leave_the_path_is_never_sent() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter.set_availability("../users/me", Some(15)),
        Err(PlatformError::NotFound)
    );
    assert_eq!(
        fake.adapter.set_availability("MLB1/../x", None),
        Err(PlatformError::NotFound)
    );
    assert!(fake.server.requests().is_empty());
}

#[test]
fn a_listing_marked_from_the_app_keeps_the_mark_only_once_mercado_livre_took_the_days() {
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
        for (path, name) in [
            ("/users/me", "users-me.json"),
            (ACTIVE, "user-items-active.json"),
            (ACTIVE_NEXT, "user-items-active-end.json"),
            (PAUSED, "user-items-paused.json"),
            (ITEMS, "items-seller.json"),
        ] {
            fake.server.serve(path, 200, fixture(name));
        }
        listings.sync(&fake.adapter).await.unwrap();
        let fone = listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .find(|listing| listing.listed.id == "MLB4100000001")
            .unwrap()
            .id;
        let terms = OnOrderTerms {
            availability_days: 15,
            accepted_orders: 3,
        };

        fake.server.serve_method(
            "PUT",
            ITEM,
            200,
            fixture("item-manufacturing-time-dropped.json"),
        );
        let refused = listings.mark_on_order(fone, terms, &fake.adapter).await;
        assert!(matches!(
            refused,
            Err(ListingError::Platform(PlatformError::Refused(_)))
        ));
        assert!(listings.on_order().await.unwrap().is_empty());

        fake.server.serve_method(
            "PUT",
            ITEM,
            200,
            fixture("item-manufacturing-time-set.json"),
        );
        let marked = listings
            .mark_on_order(fone, terms, &fake.adapter)
            .await
            .unwrap();
        assert_eq!(marked.len(), 1);
        assert_eq!(marked[0].terms, terms);
        assert_eq!(fake.puts().last().unwrap().1, SET_15_DAYS);
    });
}
