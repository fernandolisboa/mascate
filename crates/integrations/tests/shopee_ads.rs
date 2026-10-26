//! Shopee Ads through the Open Platform v2 seller adapter, against the fake
//! server serving the answers in `fixtures/shopee-seller`, written from the
//! documentation (see its README): the shop's product-level campaigns, each
//! campaign's day placed on its Listings, a shop without the Ads
//! permission, Shopee's rate limits, and marketing's Product Ads kept over
//! a real temporary database through the adapter.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{NaiveDate, TimeZone, Utc};
use futures::executor::block_on;
use hmac::{Hmac, Mac};
use mascate_integrations::{Pause, ShopeeSeller};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, PlatformError, Roas, SalesChannelKind};
use mascate_marketing::{
    ADS_HISTORY_DAYS, AdDay, AdMetrics, AdsError, AdsSync, CampaignStatus, ChannelAd, ChannelAds,
    MIGRATIONS, ProductAds,
};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;
use sha2::Sha256;

const PARTNER: &str = "2001887";
const KEY: &str = "shpk-mascate-partner-key";
const SHOP: &str = "14701711";
/// 2026-10-16 12:00 UTC, the fake clock's time.
const NOW: i64 = 1_792_152_000;
const ACCESS: &str = "59777174636562737266615546704c6d";

const RENEW: &str = "/api/v2/auth/access_token/get";
const CAMPAIGN_IDS: &str = "/api/v2/ads/get_product_level_campaign_id_list";
const CAMPAIGN_SETTINGS: &str = "/api/v2/ads/get_product_level_campaign_setting_info";
const DAILY_PERFORMANCE: &str = "/api/v2/ads/get_product_campaign_daily_performance";

const SHOPEE: SalesChannelKind = SalesChannelKind::Shopee;

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

/// `path` signed for the shop, as Shopee checks it: the partner, the path,
/// the time, the token and the shop.
fn shop(path: &str, query: &str) -> String {
    format!(
        "{path}?partner_id={PARTNER}&timestamp={NOW}&access_token={ACCESS}&shop_id={SHOP}&sign={}{query}",
        sign(&format!("{PARTNER}{path}{NOW}{ACCESS}{SHOP}"))
    )
}

fn campaign_ids(offset: u32, limit: u32) -> String {
    shop(
        CAMPAIGN_IDS,
        &format!("&ad_type=all&offset={offset}&limit={limit}"),
    )
}

fn settings(campaigns: &str) -> String {
    shop(
        CAMPAIGN_SETTINGS,
        &format!("&info_type_list=1%2C3%2C4&campaign_id_list={campaigns}"),
    )
}

fn performance(start: &str, end: &str) -> String {
    shop(
        DAILY_PERFORMANCE,
        &format!("&start_date={start}&end_date={end}&campaign_id_list=70001%2C70002%2C70003"),
    )
}

fn october(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
}

fn brl(amount: &str) -> Money {
    Money::new(amount.parse().unwrap(), Currency::Brl)
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
    adapter: ShopeeSeller,
}

impl Fake {
    /// A connected shop with the Ads permission, three campaigns and a day
    /// of performance on 3 October.
    fn with_ads() -> Self {
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
        let pauses = Arc::new(RecordedPauses::default());
        let adapter = ShopeeSeller::new(server.url(), "Mascate/test", store, clock)
            .with_pause(pauses.clone());
        let fake = Self {
            server,
            pauses,
            adapter,
        };
        fake.serve(&public(RENEW), "access-token-get.json");
        fake.serve(&campaign_ids(0, 1), "ads-campaign-ids.json");
        fake.serve(&campaign_ids(0, 100), "ads-campaign-ids.json");
        fake.serve(
            &settings("70001%2C70002%2C70003"),
            "ads-campaign-settings.json",
        );
        fake.server.serve_any_query(
            DAILY_PERFORMANCE,
            200,
            fixture("ads-daily-performance-empty.json"),
        );
        fake.serve(
            &performance("03-10-2026", "04-10-2026"),
            "ads-daily-performance-2026-10-03.json",
        );
        fake
    }

    fn serve(&self, path: &str, name: &str) {
        self.server.serve(path, 200, fixture(name));
    }

    fn asked(&self, path: &str) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|asked| asked.starts_with(path))
            .count()
    }
}

/// What Shopee reported for 3 October, campaign by campaign, on each
/// Listing.
fn october_3_ads() -> Vec<ChannelAd> {
    let ad = |listing: &str, campaign: &str, metrics| ChannelAd {
        listing: listing.into(),
        campaign: Some(campaign.into()),
        metrics,
    };
    vec![
        ad(
            "800001",
            "70001",
            AdMetrics {
                cost: brl("12.4"),
                attributed: brl("179.8"),
                clicks: 31,
                prints: 1240,
                units: 2,
            },
        ),
        // R$ 0,07 and 3 clicks of an automatic selection of two products,
        // evenly, the first taking what does not split.
        ad(
            "800002",
            "70002",
            AdMetrics {
                cost: brl("0.04"),
                attributed: brl("0"),
                clicks: 2,
                prints: 61,
                units: 0,
            },
        ),
        ad(
            "800003",
            "70002",
            AdMetrics {
                cost: brl("0.03"),
                attributed: brl("0"),
                clicks: 1,
                prints: 60,
                units: 0,
            },
        ),
        ad("800004", "70003", AdMetrics::zero(Currency::Brl)),
    ]
}

#[test]
fn a_shop_with_the_ads_permission_is_its_own_account() {
    let fake = Fake::with_ads();

    assert_eq!(fake.adapter.advertised_channel(), SHOPEE);
    assert_eq!(fake.adapter.account().unwrap(), Some(SHOP.to_owned()));
    assert_eq!(fake.asked(&campaign_ids(0, 1)), 1);
}

#[test]
fn a_shop_without_the_ads_permission_has_no_account() {
    let fake = Fake::with_ads();
    fake.serve(&campaign_ids(0, 1), "ads-permission-denied.json");

    assert_eq!(fake.adapter.account().unwrap(), None);
}

#[test]
fn the_campaigns_come_with_their_names_status_and_target_roas() {
    let fake = Fake::with_ads();

    let campaigns = fake.adapter.campaigns(SHOP).unwrap();

    let read: Vec<(&str, &str, CampaignStatus, Option<Roas>)> = campaigns
        .iter()
        .map(|found| {
            (
                found.id.as_str(),
                found.name.as_str(),
                found.status,
                found.target,
            )
        })
        .collect();
    assert_eq!(
        read,
        [
            (
                "70001",
                "Fone Bluetooth - busca",
                CampaignStatus::Active,
                Roas::new(Decimal::new(45, 1))
            ),
            ("70002", "Seleção automática", CampaignStatus::Active, None),
            ("70003", "Caneca", CampaignStatus::Paused, None),
        ]
    );
    // Shopee does not say whether the budget is the day's.
    assert!(campaigns.iter().all(|found| found.budget.is_none()));
}

#[test]
fn the_campaigns_are_read_page_by_page() {
    let fake = Fake::with_ads();
    fake.serve(&campaign_ids(0, 100), "ads-campaign-ids-page-1.json");
    fake.serve(&campaign_ids(100, 100), "ads-campaign-ids-page-2.json");

    assert_eq!(fake.adapter.campaigns(SHOP).unwrap().len(), 3);
    assert_eq!(fake.asked(&campaign_ids(100, 100)), 1);
}

#[test]
fn each_campaigns_day_lands_on_its_listings_in_cents_that_add_up() {
    let fake = Fake::with_ads();

    let ads = fake.adapter.ads_on(SHOP, october(3)).unwrap();

    assert_eq!(ads, october_3_ads());
    // Shopee refuses a range of one day: the day after comes along and is
    // left out.
    assert_eq!(fake.asked(&performance("03-10-2026", "04-10-2026")), 1);
}

#[test]
fn shopees_rate_limits_are_waited_out_and_then_reported() {
    let fake = Fake::with_ads();
    let day = performance("03-10-2026", "04-10-2026");
    fake.serve(&day, "ads-rate-limit.json");

    assert!(matches!(
        fake.adapter.ads_on(SHOP, october(3)),
        Err(PlatformError::RateLimited)
    ));
    assert!(!fake.pauses.0.lock().unwrap().is_empty());

    fake.server
        .serve(&day, 429, fixture("error-rate-limit.json"));
    assert!(matches!(
        fake.adapter.ads_on(SHOP, october(3)),
        Err(PlatformError::RateLimited)
    ));
}

#[test]
fn losing_the_ads_permission_in_the_middle_of_a_read_is_an_error() {
    let fake = Fake::with_ads();
    fake.serve(
        &performance("03-10-2026", "04-10-2026"),
        "ads-permission-denied.json",
    );

    assert!(matches!(
        fake.adapter.ads_on(SHOP, october(3)),
        Err(PlatformError::Refused(_))
    ));
}

/// Marketing's Product Ads over a real temporary database, read through the
/// adapter.
struct Synced {
    _dir: tempfile::TempDir,
    ads: ProductAds,
    fake: Fake,
}

impl Synced {
    fn new(fake: Fake) -> Self {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let database = Arc::new(
                Database::open(&dir.path().join("mascate.db"))
                    .await
                    .unwrap(),
            );
            let clock = Arc::new(ManualClock::at(
                Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
            ));
            migrate(&database, clock.as_ref(), &[MIGRATIONS])
                .await
                .unwrap();
            Self {
                _dir: dir,
                ads: ProductAds::new(database, clock, Arc::new(SequentialIds::default())),
                fake,
            }
        })
    }

    fn sync(&self) -> Result<AdsSync, AdsError> {
        block_on(self.ads.sync(&self.fake.adapter))
    }

    fn costs(&self) -> Vec<AdDay> {
        block_on(self.ads.costs()).unwrap()
    }
}

#[test]
fn a_sync_keeps_each_shopee_listings_cost_of_the_day_and_reading_it_again_changes_nothing() {
    let synced = Synced::new(Fake::with_ads());

    let read = synced.sync().unwrap();

    assert!(read.account);
    // The paused campaign did nothing that day.
    assert_eq!(read.ads, 3);
    let days_asked = synced.fake.asked(DAILY_PERFORMANCE);
    assert_eq!(days_asked, usize::try_from(ADS_HISTORY_DAYS).unwrap());
    let first = synced.costs();
    let cost = |listing: &str, amount: &str| AdDay {
        channel: SHOPEE,
        listing: listing.into(),
        day: october(3),
        cost: brl(amount),
    };
    assert_eq!(
        first,
        [
            cost("800001", "12.4"),
            cost("800002", "0.04"),
            cost("800003", "0.03"),
        ]
    );

    synced.sync().unwrap();
    assert_eq!(synced.costs(), first);
}

#[test]
fn a_shop_without_the_ads_permission_writes_nothing() {
    let fake = Fake::with_ads();
    fake.serve(&campaign_ids(0, 1), "ads-permission-denied.json");
    let synced = Synced::new(fake);

    assert_eq!(synced.sync().unwrap(), AdsSync::default());

    assert!(synced.costs().is_empty());
    assert_eq!(synced.fake.asked(DAILY_PERFORMANCE), 0);
    let last = block_on(synced.ads.last_sync(SHOPEE)).unwrap().unwrap();
    assert!(!last.account);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// However a campaign's day splits across its products, every cent of
    /// its cost and of its sales, and every click, lands on one Listing.
    #[test]
    fn a_campaigns_day_splits_without_losing_a_cent(
        products in 1usize..6,
        expense in 0i64..100_000,
        gmv in 0i64..1_000_000,
        clicks in 0u64..500,
    ) {
        let fake = Fake::with_ads();
        let items: Vec<String> = (1..=products).map(|at| format!("80000{at}")).collect();
        fake.server.serve(
            &settings("70001%2C70002%2C70003"),
            200,
            format!(
                r#"{{"error": "", "response": {{"campaign_list": [{{"campaign_id": 70001,
                    "common_info": {{"ad_name": "Todos", "campaign_status": "ongoing",
                    "item_id_list": [{}]}}}}]}}}}"#,
                items.join(", ")
            ),
        );
        fake.server.serve(
            &shop(
                DAILY_PERFORMANCE,
                "&start_date=03-10-2026&end_date=04-10-2026&campaign_id_list=70001",
            ),
            200,
            format!(
                r#"{{"error": "", "response": [{{"region": "BR", "campaign_list": [{{
                    "campaign_id": 70001, "metrics_list": [{{"date": "03-10-2026",
                    "impression": 0, "clicks": {clicks}, "expense": {},
                    "broad_gmv": {}, "broad_order_amount": 0}}]}}]}}]}}"#,
                Decimal::new(expense, 2),
                Decimal::new(gmv, 2)
            ),
        );

        let ads = fake.adapter.ads_on(SHOP, october(3)).unwrap();

        prop_assert_eq!(ads.len(), products);
        let sum = |pick: fn(&ChannelAd) -> Money| {
            Money::sum(Currency::Brl, ads.iter().map(pick)).unwrap()
        };
        prop_assert_eq!(sum(|ad| ad.metrics.cost).amount(), Decimal::new(expense, 2));
        prop_assert_eq!(sum(|ad| ad.metrics.attributed).amount(), Decimal::new(gmv, 2));
        prop_assert_eq!(ads.iter().map(|ad| ad.metrics.clicks).sum::<u64>(), clicks);
        let least = ads.iter().map(|ad| ad.metrics.cost.amount()).min().unwrap();
        let most = ads.iter().map(|ad| ad.metrics.cost.amount()).max().unwrap();
        prop_assert!(most - least <= Decimal::new(1, 2));
    }
}
