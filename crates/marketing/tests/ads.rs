//! Product Ads through marketing's public interface, with a real temporary
//! database and an in-memory channel: a daily Sync keeps each ad's day once
//! by listing and day, reads back only what can still change, keeps
//! nothing when a read fails, and a report sums a period by campaign and
//! by Product against the break-even the caller hands in.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{
    BreakEven, Currency, Margin, Money, PlatformError, Timestamp, channel_day_start,
};
use mascate_marketing::{
    ADS_HISTORY_DAYS, ADS_SETTLE_DAYS, AdDay, AdMetrics, AdsSync, AdsSyncState, AdvertisedListing,
    CampaignStatus, ChannelAd, ChannelAds, ChannelCampaign, MIGRATIONS, ProductAds,
};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

fn on(month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, month, day).unwrap()
}

fn noon() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap()
}

fn metrics(cost: &str, attributed: &str, clicks: u64) -> AdMetrics {
    AdMetrics {
        cost: brl(cost),
        attributed: brl(attributed),
        clicks,
        prints: clicks * 40,
        units: u64::from(!attributed.starts_with('0')),
    }
}

fn ad(listing: &str, campaign: &str, metrics: AdMetrics) -> ChannelAd {
    ChannelAd {
        listing: listing.into(),
        campaign: Some(campaign.into()),
        metrics,
    }
}

/// Ads that break even at `percent` of margin before Ads.
fn break_even(percent: &str) -> BreakEven {
    BreakEven::of(
        Margin::of(
            brl("100"),
            &[brl("100")
                .times(Decimal::ONE - Decimal::from_str(percent).unwrap() / Decimal::ONE_HUNDRED)],
        )
        .unwrap(),
    )
}

fn advertised(listing: &str, product: &str, break_even: Option<BreakEven>) -> AdvertisedListing {
    AdvertisedListing {
        listing: listing.into(),
        product: product.into(),
        name: format!("Produto {product}"),
        break_even,
    }
}

/// A channel answering from memory.
#[derive(Default)]
struct Channel {
    account: Mutex<Option<String>>,
    campaigns: Mutex<Vec<ChannelCampaign>>,
    days: Mutex<BTreeMap<NaiveDate, Vec<ChannelAd>>>,
    asked: Mutex<Vec<NaiveDate>>,
    failing: Mutex<Option<NaiveDate>>,
}

impl Channel {
    fn with_account() -> Self {
        let channel = Self::default();
        *channel.account.lock().unwrap() = Some("8823".into());
        *channel.campaigns.lock().unwrap() = vec![
            ChannelCampaign {
                id: "351".into(),
                name: "Fones".into(),
                status: CampaignStatus::Active,
            },
            ChannelCampaign {
                id: "352".into(),
                name: "Cabos".into(),
                status: CampaignStatus::Paused,
            },
        ];
        channel
    }

    fn reports(&self, day: NaiveDate, ads: Vec<ChannelAd>) {
        self.days.lock().unwrap().insert(day, ads);
    }

    fn asked(&self) -> Vec<NaiveDate> {
        std::mem::take(&mut *self.asked.lock().unwrap())
    }
}

impl ChannelAds for Channel {
    fn account(&self) -> Result<Option<String>, PlatformError> {
        Ok(self.account.lock().unwrap().clone())
    }

    fn campaigns(&self, account: &str) -> Result<Vec<ChannelCampaign>, PlatformError> {
        assert_eq!(account, "8823");
        Ok(self.campaigns.lock().unwrap().clone())
    }

    fn ads_on(&self, account: &str, day: NaiveDate) -> Result<Vec<ChannelAd>, PlatformError> {
        assert_eq!(account, "8823");
        self.asked.lock().unwrap().push(day);
        if *self.failing.lock().unwrap() == Some(day) {
            return Err(PlatformError::RateLimited);
        }
        Ok(self
            .days
            .lock()
            .unwrap()
            .get(&day)
            .cloned()
            .unwrap_or_default())
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    ads: ProductAds,
    channel: Channel,
}

impl Fixture {
    async fn new(channel: Channel) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(noon()));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        Self {
            ads: ProductAds::new(database, clock.clone(), Arc::new(SequentialIds::default())),
            _dir: dir,
            clock,
            channel,
        }
    }

    async fn sync(&self) -> AdsSync {
        self.ads.sync(&self.channel).await.unwrap()
    }

    fn later(&self, by: TimeDelta) {
        self.clock.advance(by);
    }
}

fn ever() -> std::ops::Range<Timestamp> {
    Timestamp::MIN_UTC..Timestamp::MAX_UTC
}

fn october() -> std::ops::Range<Timestamp> {
    channel_day_start(on(10, 1))..channel_day_start(on(11, 1))
}

#[test]
fn without_a_product_ads_account_nothing_is_read_and_nothing_kept() {
    block_on(async {
        let fx = Fixture::new(Channel::default()).await;
        assert!(fx.ads.is_due().await.unwrap());

        assert_eq!(fx.sync().await, AdsSync::default());

        assert!(fx.channel.asked().is_empty());
        assert_eq!(
            fx.ads.last_sync().await.unwrap(),
            Some(AdsSyncState {
                at: noon(),
                account: false
            })
        );
        assert!(fx.ads.costs().await.unwrap().is_empty());
        assert_eq!(fx.ads.report(ever(), &[]).await.unwrap().total, None);
        assert!(!fx.ads.is_due().await.unwrap());
    });
}

#[test]
fn the_first_sync_reads_two_months_back_to_yesterday_and_keeps_each_ad_by_day() {
    block_on(async {
        let fx = Fixture::new(Channel::with_account()).await;
        fx.channel.reports(
            on(10, 3),
            vec![
                ad("MLB1", "351", metrics("12.40", "179.80", 31)),
                ad("MLB2", "352", metrics("0", "0", 0)),
            ],
        );
        fx.channel
            .reports(on(9, 28), vec![ad("MLB1", "351", metrics("8.10", "0", 19))]);

        let synced = fx.sync().await;

        let asked = fx.channel.asked();
        // Yesterday on the channel is 4 October; today is still running.
        assert_eq!(asked.last(), Some(&on(10, 4)));
        assert_eq!(asked.len(), usize::try_from(ADS_HISTORY_DAYS).unwrap());
        assert_eq!(asked[0], on(8, 4));
        assert_eq!(
            synced,
            AdsSync {
                account: true,
                days: asked.len(),
                ads: 2,
            }
        );
        // An ad that did nothing that day is not kept.
        assert_eq!(
            fx.ads.costs().await.unwrap(),
            [
                AdDay {
                    listing: "MLB1".into(),
                    day: on(9, 28),
                    cost: brl("8.10"),
                },
                AdDay {
                    listing: "MLB1".into(),
                    day: on(10, 3),
                    cost: brl("12.40"),
                },
            ]
        );
    });
}

#[test]
fn a_sync_a_day_later_reads_back_the_days_a_sale_can_still_reach_and_changes_nothing_else() {
    block_on(async {
        let fx = Fixture::new(Channel::with_account()).await;
        fx.channel
            .reports(on(9, 1), vec![ad("MLB1", "351", metrics("5.00", "0", 9))]);
        fx.channel.reports(
            on(10, 3),
            vec![ad("MLB1", "351", metrics("12.40", "0", 31))],
        );
        fx.sync().await;
        fx.channel.asked();
        let before = fx.ads.report(ever(), &[]).await.unwrap();

        // The channel attributes a sale to the click of the 3rd, and reports
        // the 5th.
        fx.channel.reports(
            on(10, 3),
            vec![ad("MLB1", "351", metrics("12.40", "89.90", 31))],
        );
        fx.channel
            .reports(on(10, 5), vec![ad("MLB2", "352", metrics("3.30", "0", 4))]);
        fx.later(TimeDelta::days(1));
        assert!(fx.ads.is_due().await.unwrap());
        fx.sync().await;

        let asked = fx.channel.asked();
        let settle = usize::try_from(ADS_SETTLE_DAYS).unwrap();
        assert_eq!(asked.len(), settle + 1);
        assert_eq!(asked[0], on(10, 4) - TimeDelta::days(ADS_SETTLE_DAYS - 1));
        assert_eq!(asked.last(), Some(&on(10, 5)));
        let after = fx.ads.report(ever(), &[]).await.unwrap();
        let total = after.total.unwrap();
        assert_eq!(
            total.cost,
            before.total.unwrap().cost.checked_add(brl("3.30")).unwrap()
        );
        assert_eq!(total.attributed, brl("89.90"));
        // The 1st of September is past the days read again, and stays.
        assert!(
            fx.ads
                .costs()
                .await
                .unwrap()
                .iter()
                .any(|day| day.day == on(9, 1))
        );
    });
}

#[test]
fn reading_the_same_days_again_keeps_the_same_ads() {
    block_on(async {
        let fx = Fixture::new(Channel::with_account()).await;
        fx.channel.reports(
            on(10, 2),
            vec![
                ad("MLB1", "351", metrics("4.00", "0", 7)),
                ad("MLB2", "352", metrics("2.50", "0", 3)),
            ],
        );
        fx.sync().await;
        let first = fx.ads.report(ever(), &[]).await.unwrap();

        fx.later(TimeDelta::minutes(5));
        fx.sync().await;

        assert_eq!(fx.ads.report(ever(), &[]).await.unwrap(), first);
        assert_eq!(fx.ads.costs().await.unwrap().len(), 2);
    });
}

#[test]
fn an_ad_the_channel_no_longer_reports_for_a_day_is_gone() {
    block_on(async {
        let fx = Fixture::new(Channel::with_account()).await;
        fx.channel.reports(
            on(10, 2),
            vec![
                ad("MLB1", "351", metrics("4.00", "0", 7)),
                ad("MLB2", "352", metrics("2.50", "0", 3)),
            ],
        );
        fx.sync().await;

        fx.channel
            .reports(on(10, 2), vec![ad("MLB1", "351", metrics("4.20", "0", 8))]);
        fx.later(TimeDelta::minutes(5));
        fx.sync().await;

        assert_eq!(
            fx.ads.costs().await.unwrap(),
            [AdDay {
                listing: "MLB1".into(),
                day: on(10, 2),
                cost: brl("4.20"),
            }]
        );
    });
}

#[test]
fn a_day_that_fails_to_read_keeps_nothing_of_the_sync() {
    block_on(async {
        let fx = Fixture::new(Channel::with_account()).await;
        fx.channel
            .reports(on(10, 2), vec![ad("MLB1", "351", metrics("4.00", "0", 7))]);
        *fx.channel.failing.lock().unwrap() = Some(on(10, 3));

        let failed = fx.ads.sync(&fx.channel).await;

        assert!(failed.is_err());
        assert!(fx.ads.costs().await.unwrap().is_empty());
        assert_eq!(fx.ads.last_sync().await.unwrap(), None);
    });
}

#[test]
fn a_report_sums_the_period_by_campaign_and_by_product_losers_first() {
    block_on(async {
        let fx = Fixture::new(Channel::with_account()).await;
        fx.channel.reports(
            on(9, 30),
            vec![ad("MLB1", "351", metrics("50.00", "100.00", 90))],
        );
        fx.channel.reports(
            on(10, 2),
            vec![
                // Fone: R$ 30 brought R$ 90 (3x), short of the 4x it needs.
                ad("MLB1", "351", metrics("10.00", "40.00", 20)),
                ad("MLB3", "351", metrics("20.00", "50.00", 25)),
                // Cabo: R$ 10 brought R$ 120 (12x), above its 5x.
                ad("MLB2", "352", metrics("10.00", "120.00", 12)),
                // A listing the caller does not know.
                ad("MLB9", "353", metrics("1.00", "0", 2)),
            ],
        );
        fx.sync().await;
        let listings = [
            advertised("MLB1", "fone", Some(break_even("40"))),
            advertised("MLB3", "fone", Some(break_even("25"))),
            advertised("MLB2", "cabo", Some(break_even("20"))),
        ];

        let report = fx.ads.report(october(), &listings).await.unwrap();

        let total = report.total.unwrap();
        assert_eq!(total.cost, brl("41.00"));
        assert_eq!(total.attributed, brl("210.00"));
        assert_eq!(total.clicks, 59);
        let campaigns: Vec<(&str, Option<&str>, Money)> = report
            .campaigns
            .iter()
            .map(|found| {
                (
                    found.id.as_str(),
                    found.campaign.as_ref().map(|c| c.name.as_str()),
                    found.metrics.cost,
                )
            })
            .collect();
        assert_eq!(
            campaigns,
            [
                ("351", Some("Fones"), brl("30.00")),
                ("352", Some("Cabos"), brl("10.00")),
                ("353", None, brl("1.00")),
            ]
        );
        let fone = &report.products[0];
        assert_eq!(fone.product, "fone");
        assert_eq!(fone.listings, ["MLB1", "MLB3"]);
        assert_eq!(fone.metrics.cost, brl("30.00"));
        assert_eq!(fone.roas().unwrap().to_pt_br(), "3,00x");
        // The listing that asks more of its ads sets the Product's.
        assert_eq!(fone.break_even, Some(break_even("25")));
        assert!(fone.loses());
        let cabo = &report.products[1];
        assert_eq!(cabo.product, "cabo");
        assert_eq!(cabo.roas().unwrap().to_pt_br(), "12,00x");
        assert!(!cabo.loses());
        let unknown = &report.products[2];
        assert_eq!(unknown.name, "MLB9");
        assert_eq!(unknown.break_even, None);
        assert!(!unknown.loses());
    });
}

#[test]
fn ads_that_spend_on_a_product_losing_before_ads_always_lose() {
    block_on(async {
        let fx = Fixture::new(Channel::with_account()).await;
        fx.channel.reports(
            on(10, 2),
            vec![ad("MLB1", "351", metrics("1.00", "35.00", 3))],
        );
        fx.sync().await;

        let report = fx
            .ads
            .report(
                october(),
                &[advertised("MLB1", "fone", Some(BreakEven::Never))],
            )
            .await
            .unwrap();

        assert!(report.products[0].loses());
    });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// However the ads fall, the Products' and the campaigns' metrics add
    /// up to the period's.
    #[test]
    fn a_report_adds_up(
        days in prop::collection::vec(
            (1u32..=4, 0usize..3, 0usize..2, 0i64..5000, 0i64..50_000, 0u64..90),
            1..12,
        ),
    ) {
        block_on(async {
            let fx = Fixture::new(Channel::with_account()).await;
            let mut by_day: BTreeMap<NaiveDate, Vec<ChannelAd>> = BTreeMap::new();
            for (day, listing, campaign, cost, attributed, clicks) in &days {
                let listing = format!("MLB{listing}");
                let ads = by_day.entry(on(10, *day)).or_default();
                if ads.iter().any(|ad| ad.listing == listing) {
                    continue;
                }
                ads.push(ChannelAd {
                    listing,
                    campaign: Some(["351", "352"][*campaign].into()),
                    metrics: AdMetrics {
                        cost: Money::new(Decimal::new(*cost, 2), Currency::Brl),
                        attributed: Money::new(Decimal::new(*attributed, 2), Currency::Brl),
                        clicks: *clicks,
                        prints: *clicks,
                        units: 0,
                    },
                });
            }
            for (day, ads) in by_day {
                fx.channel.reports(day, ads);
            }
            fx.sync().await;
            let listings = [
                advertised("MLB0", "fone", None),
                advertised("MLB1", "fone", None),
            ];

            let report = fx.ads.report(october(), &listings).await.unwrap();

            if let Some(total) = report.total {
                let products = Money::sum(
                    Currency::Brl,
                    report.products.iter().map(|found| found.metrics.cost),
                )
                .unwrap();
                let campaigns = Money::sum(
                    Currency::Brl,
                    report.campaigns.iter().map(|found| found.metrics.cost),
                )
                .unwrap();
                prop_assert_eq!(products, total.cost);
                prop_assert_eq!(campaigns, total.cost);
                let clicks: u64 = report.products.iter().map(|found| found.metrics.clicks).sum();
                prop_assert_eq!(clicks, total.clicks);
            }
            Ok(())
        })?;
    }
}
