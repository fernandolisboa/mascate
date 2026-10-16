//! The featured offer's expected price of the owner's Amazon offers through
//! the Selling Partner API adapter, against the fake server serving the
//! answers in `fixtures/amazon-seller`, written from the documentation (see
//! its README). What commerce does with them is tested there.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use mascate_commerce::{FeaturedOffer, FeaturedOfferPrices};
use mascate_integrations::{Amazon, Pause};
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Currency, Money, PlatformError};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};
use rust_decimal::Decimal;
use serde_json::json;

const BATCH: &str = "/batches/products/pricing/2022-05-01/offer/featuredOfferExpectedPrice";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/amazon-seller/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn brl(amount: &str) -> Money {
    Money::new(amount.parse::<Decimal>().unwrap(), Currency::Brl)
}

fn skus(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// Remembers each pause instead of sleeping.
#[derive(Default)]
struct RecordedPauses(Mutex<Vec<Duration>>);

impl Pause for RecordedPauses {
    fn pause(&self, duration: Duration) {
        self.0.lock().unwrap().push(duration);
    }
}

struct Fake {
    server: FakeHttpServer,
    pauses: Arc<RecordedPauses>,
    adapter: Amazon,
}

impl Fake {
    /// The owner's keys pasted, on 2026-10-16 12:00 UTC.
    fn connected() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            (
                "AMAZON_LWA_CLIENT_ID",
                "amzn1.application-oa2-client.mascate",
            ),
            ("AMAZON_LWA_CLIENT_SECRET", "amzn1.oa2-cs.v1.s3cr3t"),
            ("AMAZON_REFRESH_TOKEN", "Atzr|IwEBIA-refresh-token"),
            ("AMAZON_SELLER_ID", "A1MASCATE0SELLER"),
        ] {
            store.write(name, &Secret::new(value)).unwrap();
        }
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
        ));
        let pauses = Arc::new(RecordedPauses::default());
        let adapter = Amazon::new(server.url(), server.url(), "Mascate/test", store, clock)
            .with_pause(pauses.clone());
        server.serve("/auth/o2/token", 200, fixture("token.json"));
        Self {
            server,
            pauses,
            adapter,
        }
    }

    fn batches(&self) -> Vec<serde_json::Value> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.path == BATCH)
            .map(|request| serde_json::from_str(&request.body).unwrap())
            .collect()
    }
}

#[test]
fn each_offer_comes_with_its_expected_price_no_competition_or_not_eligible() {
    let fake = Fake::connected();
    fake.server
        .serve(BATCH, 200, fixture("featured-offer-expected-price.json"));

    let offers = fake
        .adapter
        .featured_offers(&skus(&[
            "FONE-BT-001",
            "CANECA-01",
            "CAMISETA-P",
            "ANTIGO-1",
        ]))
        .unwrap();

    assert_eq!(
        offers,
        [
            (
                "FONE-BT-001".to_owned(),
                FeaturedOffer::Expected(brl("87.35"))
            ),
            ("CANECA-01".to_owned(), FeaturedOffer::NoCompetition),
            ("CAMISETA-P".to_owned(), FeaturedOffer::NotEligible),
            ("ANTIGO-1".to_owned(), FeaturedOffer::NotEligible),
        ]
    );
    let request = |sku: &str| {
        json!({
            "uri": "/products/pricing/2022-05-01/offer/featuredOfferExpectedPrice",
            "method": "GET",
            "marketplaceId": "A2Q3Y263D00KWC",
            "sku": sku,
        })
    };
    assert_eq!(
        fake.batches(),
        [json!({"requests": [
            request("FONE-BT-001"),
            request("CANECA-01"),
            request("CAMISETA-P"),
            request("ANTIGO-1"),
        ]})]
    );
    let sent = fake
        .server
        .received()
        .into_iter()
        .find(|request| request.path == BATCH)
        .unwrap();
    assert_eq!(sent.method, "POST");
}

#[test]
fn a_sku_the_answer_does_not_name_is_left_for_commerce_to_count_as_not_eligible() {
    let fake = Fake::connected();
    fake.server
        .serve(BATCH, 200, fixture("featured-offer-expected-price.json"));

    let offers = fake
        .adapter
        .featured_offers(&skus(&["FONE-BT-001"]))
        .unwrap();

    assert_eq!(
        offers,
        [(
            "FONE-BT-001".to_owned(),
            FeaturedOffer::Expected(brl("87.35"))
        )]
    );
}

#[test]
fn more_than_forty_skus_go_in_batches_half_a_minute_apart() {
    let fake = Fake::connected();
    fake.server
        .serve(BATCH, 200, fixture("featured-offer-expected-price.json"));
    let many: Vec<String> = (1..=41).map(|n| format!("SKU-{n}")).collect();

    fake.adapter.featured_offers(&many).unwrap();

    let batches = fake.batches();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0]["requests"].as_array().unwrap().len(), 40);
    assert_eq!(batches[1]["requests"][0]["sku"], "SKU-41");
    assert_eq!(*fake.pauses.0.lock().unwrap(), [Duration::from_secs(30)]);
}

#[test]
fn throttling_waits_and_then_says_so() {
    let fake = Fake::connected();
    fake.server.serve(BATCH, 429, fixture("error-429.json"));

    assert_eq!(
        fake.adapter.featured_offers(&skus(&["FONE-BT-001"])),
        Err(PlatformError::RateLimited)
    );
    assert!(!fake.pauses.0.lock().unwrap().is_empty());
}
