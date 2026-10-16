//! The shop's listings with issues and Shopee's rating of the shop through
//! the Open Platform v2 seller adapter, against the fake server serving the
//! answers in `fixtures/shopee-seller`, written from the documentation (see
//! its README). What marketing keeps of them is tested there.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use hmac::{Hmac, Mac};
use mascate_integrations::ShopeeSeller;
use mascate_kernel::SalesChannelKind;
use mascate_kernel::testing::ManualClock;
use mascate_marketing::{
    ActionKind, ChannelQuality, Comparator, MetricGroup, MetricTarget, MetricUnit,
    PerformanceMetric, PerformanceRating, QualitySource,
};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};
use rust_decimal::Decimal;
use sha2::Sha256;

const PARTNER: &str = "2001887";
const KEY: &str = "shpk-mascate-partner-key";
const SHOP: &str = "14701711";
/// 2026-10-16 12:00 UTC, the fake clock's time.
const NOW: i64 = 1_792_152_000;
const ACCESS: &str = "59777174636562737266615546704c6d";

const RENEW: &str = "/api/v2/auth/access_token/get";
const LISTINGS_WITH_ISSUES: &str = "/api/v2/account_health/get_listings_with_issues";
const SHOP_PERFORMANCE: &str = "/api/v2/account_health/get_shop_performance";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/shopee-seller/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn sign(base: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(KEY.as_bytes()).unwrap();
    mac.update(base.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn public(path: &str) -> String {
    format!(
        "{path}?partner_id={PARTNER}&timestamp={NOW}&sign={}",
        sign(&format!("{PARTNER}{path}{NOW}"))
    )
}

fn shop(path: &str, query: &str) -> String {
    format!(
        "{path}?partner_id={PARTNER}&timestamp={NOW}&access_token={ACCESS}&shop_id={SHOP}&sign={}{query}",
        sign(&format!("{PARTNER}{path}{NOW}{ACCESS}{SHOP}"))
    )
}

fn page(number: u32) -> String {
    shop(
        LISTINGS_WITH_ISSUES,
        &format!("&page_no={number}&page_size=100"),
    )
}

fn decimal(text: &str) -> Option<Decimal> {
    Some(text.parse().unwrap())
}

struct Fake {
    server: FakeHttpServer,
    adapter: ShopeeSeller,
}

impl Fake {
    fn connected() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            ("SHOPEE_PARTNER_ID", PARTNER),
            ("SHOPEE_PARTNER_KEY", KEY),
            ("SHOPEE_SHOP_ID", SHOP),
            ("SHOPEE_REFRESH_TOKEN", "3a4b4c4d4e4f50515253545556575859"),
        ] {
            store.write(name, &Secret::new(value)).unwrap();
        }
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
        ));
        let adapter = ShopeeSeller::new(server.url(), "Mascate/test", store, clock);
        server.serve(&public(RENEW), 200, fixture("access-token-get.json"));
        Self { server, adapter }
    }

    fn serve(&self, path: &str, name: &str) {
        self.server.serve(path, 200, fixture(name));
    }

    fn asked(&self, path: &str) -> usize {
        self.server
            .received()
            .iter()
            .filter(|request| request.path.starts_with(path))
            .count()
    }
}

#[test]
fn the_shops_listings_with_issues_are_read_once_and_each_listing_gets_its_own() {
    let fake = Fake::connected();
    fake.serve(&page(1), "listings-with-issues-page-1.json");
    fake.serve(&page(2), "listings-with-issues-page-2.json");

    let qualities = fake
        .adapter
        .qualities(&["3000000001".to_owned(), "3000000002".to_owned()])
        .unwrap();

    assert_eq!(fake.adapter.rated_channel(), SalesChannelKind::Shopee);
    assert_eq!(fake.asked(LISTINGS_WITH_ISSUES), 2);
    let issues = qualities[0].as_ref().unwrap();
    assert_eq!(issues.score, None);
    let found: Vec<(&str, ActionKind)> = issues
        .pending
        .iter()
        .map(|action| (action.key.as_str(), action.kind))
        .collect();
    assert_eq!(
        found,
        [("4", ActionKind::Problem), ("7", ActionKind::Opportunity)]
    );
    assert!(issues.pending[0].text.contains("imagem"));
    assert_eq!(
        issues.pending[0].link.as_deref(),
        Some("https://seller.shopee.com.br/portal/product/3000000001")
    );
    // A listing Shopee does not report has nothing pending.
    assert_eq!(
        qualities[1],
        Some(ChannelQuality {
            score: None,
            pending: Vec::new(),
        })
    );
    assert_eq!(fake.adapter.visits("3000000001", 30).unwrap(), None);
}

#[test]
fn one_listings_quality_reads_the_same_list() {
    let fake = Fake::connected();
    fake.serve(&page(1), "listings-with-issues-empty.json");

    let quality = fake.adapter.quality("3000000001").unwrap().unwrap();

    assert!(quality.pending.is_empty());
    assert_eq!(fake.asked(LISTINGS_WITH_ISSUES), 1);
}

#[test]
fn no_listings_asks_nothing() {
    let fake = Fake::connected();

    assert!(fake.adapter.qualities(&[]).unwrap().is_empty());
    assert_eq!(fake.asked(LISTINGS_WITH_ISSUES), 0);
}

#[test]
fn the_shops_performance_comes_with_each_metric_and_its_target() {
    let fake = Fake::connected();
    fake.serve(&shop(SHOP_PERFORMANCE, ""), "shop-performance.json");

    let performance = fake.adapter.performance().unwrap().unwrap();

    assert_eq!(
        performance.rating,
        Some(PerformanceRating::ImprovementNeeded)
    );
    // The group and the metric of an unknown kind are left out.
    assert_eq!(
        performance.metrics,
        vec![
            PerformanceMetric {
                key: "1".into(),
                group: MetricGroup::Fulfillment,
                name: "Late Shipment Rate (All Channels)".into(),
                current: decimal("12.5"),
                previous: decimal("8.0"),
                unit: MetricUnit::Percent,
                target: Some(MetricTarget {
                    value: Decimal::from(10),
                    comparator: Comparator::Below,
                }),
            },
            PerformanceMetric {
                key: "3".into(),
                group: MetricGroup::Fulfillment,
                name: "Non-Fulfilment Rate (All Channels)".into(),
                current: decimal("1.2"),
                previous: decimal("0"),
                unit: MetricUnit::Percent,
                target: Some(MetricTarget {
                    value: Decimal::from(10),
                    comparator: Comparator::Below,
                }),
            },
            PerformanceMetric {
                key: "4".into(),
                group: MetricGroup::Fulfillment,
                name: "Preparation Time".into(),
                current: None,
                previous: None,
                unit: MetricUnit::Days,
                target: Some(MetricTarget {
                    value: Decimal::from(2),
                    comparator: Comparator::AtMost,
                }),
            },
            PerformanceMetric {
                key: "11".into(),
                group: MetricGroup::CustomerService,
                name: "Chat Response Rate".into(),
                current: decimal("92.31"),
                previous: decimal("95"),
                unit: MetricUnit::Percent,
                target: Some(MetricTarget {
                    value: Decimal::from(90),
                    comparator: Comparator::AtLeast,
                }),
            },
        ]
    );
    assert_eq!(performance.below_target(), 1);
}
