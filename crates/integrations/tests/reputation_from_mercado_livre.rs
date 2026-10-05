//! The seller's Reputation and the listings' Reviews through Mercado
//! Livre's adapter, against the fake server serving the answers in
//! `fixtures/mercado-livre`, written from the documentation (see its
//! README). What marketing does with them is tested there.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use mascate_integrations::MercadoLivre;
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Percentage, PlatformError, Timestamp};
use mascate_marketing::{
    ChannelReputation, ChannelReview, ListingReviews, MetricReading, ReputationColor,
    ReputationSource,
};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};

const REVIEWS: &str = "/reviews/item/MLB4100000001?limit=50&offset=0";
const REVIEWS_NEXT: &str = "/reviews/item/MLB4100000001?limit=50&offset=2";
const GOOD_REVIEWS: &str = "/reviews/item/MLB4100000003?limit=50&offset=0";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/mercado-livre/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn at(text: &str) -> Timestamp {
    chrono::DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

fn reading(rate: &str, count: u32) -> MetricReading {
    MetricReading {
        rate: Percentage::parse(rate).unwrap(),
        count,
    }
}

fn review(id: &str, rating: u8, title: &str, text: &str, when: &str) -> ChannelReview {
    ChannelReview {
        id: id.into(),
        rating,
        title: title.into(),
        text: text.into(),
        at: at(when),
    }
}

struct Fake {
    server: FakeHttpServer,
    adapter: MercadoLivre,
}

impl Fake {
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
            Utc.with_ymd_and_hms(2026, 10, 5, 15, 0, 0).unwrap(),
        ));
        let adapter = MercadoLivre::new(server.url(), "Mascate/test", store, clock);
        server.serve("/oauth/token", 200, fixture("token.json"));
        Self { server, adapter }
    }
}

#[test]
fn the_reputation_comes_with_the_real_metrics_a_protection_hides() {
    let fake = Fake::connected();
    fake.server
        .serve("/users/me", 200, fixture("users-me-reputation.json"));

    let reputation = fake.adapter.reputation().unwrap();

    assert_eq!(
        reputation,
        ChannelReputation {
            color: Some(ReputationColor::Green),
            real_color: Some(ReputationColor::Yellow),
            protected_until: Some(at("2026-11-30T00:00:00-03:00")),
            period_days: Some(60),
            sales: 64,
            transactions: 66,
            claims: reading("3.12", 2),
            cancellations: reading("1.56", 1),
            delayed_handling: reading("4.69", 3),
        }
    );
}

#[test]
fn a_seller_without_sales_enough_has_no_color_yet() {
    let fake = Fake::connected();
    fake.server
        .serve("/users/me", 200, fixture("users-me.json"));

    let reputation = fake.adapter.reputation().unwrap();

    assert_eq!(reputation.color, None);
    assert_eq!(reputation.real_color, None);
    assert_eq!(reputation.sales, 0);
    assert_eq!(reputation.claims, MetricReading::NONE);
}

#[test]
fn the_low_reviews_come_from_every_page_until_the_count_is_complete() {
    let fake = Fake::connected();
    fake.server
        .serve(REVIEWS, 200, fixture("reviews-MLB4100000001.json"));
    fake.server
        .serve(REVIEWS_NEXT, 200, fixture("reviews-MLB4100000001-end.json"));

    let reviews = fake.adapter.reviews("MLB4100000001").unwrap();

    assert_eq!(
        reviews,
        ListingReviews {
            stars: [0, 1, 1, 0, 1],
            low: vec![
                review(
                    "52001000002",
                    2,
                    "Parou de carregar",
                    "O lado esquerdo parou de carregar depois de uma semana.",
                    "2026-10-03T09:40:00Z",
                ),
                review(
                    "52001000003",
                    3,
                    "Razoável",
                    "Bom pelo preço, mas o case risca fácil.",
                    "2026-10-04T18:05:10Z",
                ),
            ],
        }
    );
}

#[test]
fn without_low_reviews_the_first_page_is_enough() {
    let fake = Fake::connected();
    fake.server
        .serve(GOOD_REVIEWS, 200, fixture("reviews-MLB4100000003.json"));

    let reviews = fake.adapter.reviews("MLB4100000003").unwrap();

    assert_eq!(reviews.stars, [0, 0, 0, 1, 3]);
    assert_eq!(reviews.low, []);
    let pages = fake
        .server
        .requests()
        .into_iter()
        .filter(|path| path.starts_with("/reviews/"))
        .count();
    assert_eq!(pages, 1);
}

#[test]
fn a_listing_nobody_reviewed_has_no_reviews() {
    let fake = Fake::connected();
    fake.server.serve(
        "/reviews/item/MLB4100000009?limit=50&offset=0",
        404,
        fixture("error-404-reviews.json"),
    );

    assert_eq!(
        fake.adapter.reviews("MLB4100000009").unwrap(),
        ListingReviews::default()
    );
}

#[test]
fn an_id_that_could_leave_the_path_is_never_requested() {
    let fake = Fake::connected();

    let error = fake.adapter.reviews("MLB1/../../users/me").unwrap_err();

    assert_eq!(error, PlatformError::NotFound);
    assert!(fake.server.requests().is_empty());
}

#[test]
fn a_busy_channel_is_rate_limited() {
    let fake = Fake::connected();
    fake.server
        .serve("/users/me", 429, fixture("error-429.json"));
    fake.server.serve(REVIEWS, 429, fixture("error-429.json"));

    assert_eq!(
        fake.adapter.reputation().unwrap_err(),
        PlatformError::RateLimited
    );
    assert_eq!(
        fake.adapter.reviews("MLB4100000001").unwrap_err(),
        PlatformError::RateLimited
    );
}
