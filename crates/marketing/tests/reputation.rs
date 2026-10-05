//! Reputation and Reviews through marketing's public interface, with a real
//! temporary database and an in-memory channel: the Reputation with each
//! metric against its limits, the Reviews summed up by Product, each low
//! Review kept once, and a notice, once, when a tool unlocks.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Percentage, PlatformError, Timestamp};
use mascate_marketing::{
    ChannelReputation, ChannelReview, ListingReviews, MIGRATIONS, MetricReading,
    PRODUCT_ADS_MIN_SALES, ProductReviews, REFRESH_MINUTES, Reputation, ReputationColor,
    ReputationError, ReputationMetric, ReputationSource, ReviewedItem, SellerTool,
};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

/// A channel answering from memory.
struct Channel {
    reputation: Mutex<ChannelReputation>,
    reviews: Mutex<BTreeMap<String, ListingReviews>>,
    /// Fails the Reviews of this listing while set.
    failing: Mutex<Option<String>>,
}

impl Channel {
    fn new(reputation: ChannelReputation) -> Self {
        Self {
            reputation: Mutex::new(reputation),
            reviews: Mutex::new(BTreeMap::new()),
            failing: Mutex::new(None),
        }
    }

    fn rates(&self, reputation: ChannelReputation) {
        *self.reputation.lock().unwrap() = reputation;
    }

    fn reviewed(&self, listing: &str, stars: [u32; 5], low: Vec<ChannelReview>) {
        self.reviews
            .lock()
            .unwrap()
            .insert(listing.into(), ListingReviews { stars, low });
    }
}

impl ReputationSource for Channel {
    fn reputation(&self) -> Result<ChannelReputation, PlatformError> {
        Ok(self.reputation.lock().unwrap().clone())
    }

    fn reviews(&self, listing: &str) -> Result<ListingReviews, PlatformError> {
        if self.failing.lock().unwrap().as_deref() == Some(listing) {
            return Err(PlatformError::RateLimited);
        }
        Ok(self
            .reviews
            .lock()
            .unwrap()
            .get(listing)
            .cloned()
            .unwrap_or_default())
    }
}

fn noon() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap()
}

fn percent(text: &str) -> Percentage {
    Percentage::parse(text).unwrap()
}

fn reading(rate: &str, count: u32) -> MetricReading {
    MetricReading {
        rate: percent(rate),
        count,
    }
}

/// A seller with `color`, 64 sales in 60 days and nothing against them.
fn seller(color: Option<ReputationColor>, transactions: u32) -> ChannelReputation {
    ChannelReputation {
        color,
        real_color: None,
        protected_until: None,
        period_days: Some(60),
        sales: 64,
        transactions,
        claims: MetricReading::NONE,
        cancellations: MetricReading::NONE,
        delayed_handling: MetricReading::NONE,
    }
}

fn review(id: &str, rating: u8, days_ago: i64) -> ChannelReview {
    ChannelReview {
        id: id.into(),
        rating,
        title: format!("Nota {rating}"),
        text: "Chegou com a caixa amassada.".into(),
        at: noon() - TimeDelta::days(days_ago),
    }
}

fn item(listing: &str, product: &str) -> ReviewedItem {
    ReviewedItem {
        listing: listing.into(),
        product: product.into(),
        name: format!("Produto {product}"),
    }
}

fn listings(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    reputation: Reputation,
    channel: Channel,
}

impl Fixture {
    async fn new() -> Self {
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
        let reputation =
            Reputation::new(database, clock.clone(), Arc::new(SequentialIds::default()));
        Self {
            _dir: dir,
            clock,
            reputation,
            channel: Channel::new(seller(None, 0)),
        }
    }

    async fn sync(&self, listings: &[String]) -> mascate_marketing::ReputationSync {
        self.reputation.sync(&self.channel, listings).await.unwrap()
    }
}

#[test]
fn each_metric_stands_against_the_limit_of_the_green_in_rate_and_in_sales() {
    block_on(async {
        let fixture = Fixture::new().await;
        assert_eq!(fixture.reputation.standing().await.unwrap(), None);
        fixture.channel.rates(ChannelReputation {
            claims: reading("3.12", 2),
            cancellations: reading("1.56", 1),
            delayed_handling: reading("4.69", 3),
            ..seller(Some(ReputationColor::Yellow), 66)
        });

        fixture.sync(&[]).await;

        let standing = fixture.reputation.standing().await.unwrap().unwrap();
        assert_eq!(standing.checked_at, noon());
        assert_eq!(standing.reputation.color, Some(ReputationColor::Yellow));
        let metric = |metric: ReputationMetric| {
            *standing
                .metrics
                .iter()
                .find(|standing| standing.metric == metric)
                .unwrap()
        };
        // 2 claims in 64 sales: the green takes 2% of 64, 1 claim.
        let claims = metric(ReputationMetric::Claims);
        assert_eq!(claims.green_limit, percent("2"));
        assert_eq!(claims.fits, ReputationColor::Yellow);
        assert_eq!(claims.room, Some(-1));
        assert_eq!(claims.points_to_green(), Decimal::new(-112, 2));
        // 1.5% of 64 is 0.96: not even one cancellation keeps the green.
        let cancellations = metric(ReputationMetric::Cancellations);
        assert_eq!(cancellations.fits, ReputationColor::Yellow);
        assert_eq!(cancellations.room, Some(-1));
        // 10% of 64 takes 6 delays; 3 happened.
        let delayed = metric(ReputationMetric::DelayedHandling);
        assert_eq!(delayed.fits, ReputationColor::Green);
        assert_eq!(delayed.room, Some(3));
        assert_eq!(delayed.points_to_green(), Decimal::new(531, 2));
    });
}

#[test]
fn a_rate_past_every_limit_fits_the_red_and_no_sales_leave_no_room_to_count() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.channel.rates(ChannelReputation {
            sales: 0,
            claims: reading("8.5", 0),
            ..seller(None, 0)
        });

        fixture.sync(&[]).await;

        let standing = fixture.reputation.standing().await.unwrap().unwrap();
        let claims = standing.metrics[0];
        assert_eq!(claims.metric, ReputationMetric::Claims);
        assert_eq!(claims.fits, ReputationColor::Red);
        assert_eq!(claims.room, None);
        assert_eq!(
            standing.tools,
            [
                (SellerTool::Promotions, false),
                (SellerTool::ProductAds, false)
            ]
        );
    });
}

#[test]
fn a_protected_seller_keeps_its_color_and_the_real_one_alongside() {
    block_on(async {
        let fixture = Fixture::new().await;
        let protected = ChannelReputation {
            real_color: Some(ReputationColor::Orange),
            protected_until: Some(noon() + TimeDelta::days(30)),
            period_days: None,
            ..seller(Some(ReputationColor::Green), 12)
        };
        fixture.channel.rates(protected.clone());

        fixture.sync(&[]).await;

        let standing = fixture.reputation.standing().await.unwrap().unwrap();
        assert_eq!(standing.reputation, protected);
    });
}

#[test]
fn the_reputation_is_due_again_an_hour_after_the_last_sync() {
    block_on(async {
        let fixture = Fixture::new().await;
        assert!(fixture.reputation.is_due().await.unwrap());

        fixture.sync(&[]).await;
        assert!(!fixture.reputation.is_due().await.unwrap());
        assert_eq!(fixture.reputation.last_sync().await.unwrap(), Some(noon()));

        fixture
            .clock
            .advance(TimeDelta::minutes(REFRESH_MINUTES - 1));
        assert!(!fixture.reputation.is_due().await.unwrap());
        fixture.clock.advance(TimeDelta::minutes(1));
        assert!(fixture.reputation.is_due().await.unwrap());
    });
}

#[test]
fn the_green_unlocks_promotions_and_product_ads_once() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .rates(seller(Some(ReputationColor::Green), 64));

        let synced = fixture.sync(&[]).await;
        assert_eq!(synced.color, Some(ReputationColor::Green));
        assert_eq!(
            synced.unlocked,
            [SellerTool::Promotions, SellerTool::ProductAds]
        );
        assert_eq!(
            fixture.reputation.unlocked().await.unwrap(),
            [SellerTool::Promotions, SellerTool::ProductAds]
        );

        // The same Reputation again unlocks nothing new.
        assert_eq!(fixture.sync(&[]).await.unlocked, []);
        fixture
            .reputation
            .dismiss(SellerTool::Promotions)
            .await
            .unwrap();
        assert_eq!(
            fixture.reputation.unlocked().await.unwrap(),
            [SellerTool::ProductAds]
        );
        assert_eq!(fixture.sync(&[]).await.unlocked, []);
        assert_eq!(
            fixture.reputation.unlocked().await.unwrap(),
            [SellerTool::ProductAds]
        );
    });
}

#[test]
fn a_tool_lost_and_reached_again_unlocks_again() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .rates(seller(Some(ReputationColor::Green), 64));
        fixture.sync(&[]).await;
        fixture
            .reputation
            .dismiss(SellerTool::Promotions)
            .await
            .unwrap();

        fixture
            .channel
            .rates(seller(Some(ReputationColor::Yellow), 64));
        assert_eq!(fixture.sync(&[]).await.unlocked, []);
        assert_eq!(
            fixture.reputation.unlocked().await.unwrap(),
            [SellerTool::ProductAds]
        );

        fixture
            .channel
            .rates(seller(Some(ReputationColor::Green), 64));
        assert_eq!(fixture.sync(&[]).await.unlocked, [SellerTool::Promotions]);
        assert_eq!(
            fixture.reputation.unlocked().await.unwrap(),
            [SellerTool::Promotions, SellerTool::ProductAds]
        );
    });
}

#[test]
fn product_ads_takes_the_yellow_and_the_minimum_of_sales() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.channel.rates(seller(
            Some(ReputationColor::Yellow),
            PRODUCT_ADS_MIN_SALES - 1,
        ));
        assert_eq!(fixture.sync(&[]).await.unlocked, []);

        fixture
            .channel
            .rates(seller(Some(ReputationColor::Orange), 64));
        assert_eq!(fixture.sync(&[]).await.unlocked, []);

        fixture
            .channel
            .rates(seller(Some(ReputationColor::Yellow), PRODUCT_ADS_MIN_SALES));
        assert_eq!(fixture.sync(&[]).await.unlocked, [SellerTool::ProductAds]);
        let standing = fixture.reputation.standing().await.unwrap().unwrap();
        assert_eq!(
            standing.tools,
            [
                (SellerTool::Promotions, false),
                (SellerTool::ProductAds, true)
            ]
        );
    });
}

#[test]
fn a_new_seller_without_a_color_unlocks_nothing() {
    block_on(async {
        let fixture = Fixture::new().await;
        assert_eq!(fixture.sync(&[]).await.unlocked, []);
        assert_eq!(fixture.reputation.unlocked().await.unwrap(), []);
    });
}

#[test]
fn a_low_review_arrives_once_and_reviews_add_up_by_product() {
    block_on(async {
        let fixture = Fixture::new().await;
        // Two listings of the same Product, and one of another.
        fixture.channel.reviewed(
            "MLB1",
            [0, 1, 1, 0, 1],
            vec![review("R2", 2, 3), review("R3", 3, 1)],
        );
        fixture
            .channel
            .reviewed("MLB2", [1, 0, 0, 0, 3], vec![review("R4", 1, 2)]);
        fixture.channel.reviewed("MLB3", [0, 0, 0, 2, 4], vec![]);
        let open = listings(&["MLB1", "MLB2", "MLB3", "MLB1"]);

        let synced = fixture.sync(&open).await;
        assert_eq!(synced.reviewed, 3);
        let arrived: Vec<&str> = synced
            .arrived
            .iter()
            .map(|arrived| arrived.review.id.as_str())
            .collect();
        assert_eq!(arrived, ["R2", "R4", "R3"]);
        assert_eq!(synced.arrived[1].listing, "MLB2");

        // The same answers again bring nothing new.
        assert_eq!(fixture.sync(&open).await.arrived, []);

        let products = fixture
            .reputation
            .reviews(&[
                item("MLB3", "B"),
                item("MLB1", "A"),
                item("MLB2", "A"),
                item("MLB9", "C"),
            ])
            .await
            .unwrap();
        assert_eq!(
            products,
            [
                ProductReviews {
                    product: "A".into(),
                    name: "Produto A".into(),
                    listings: listings(&["MLB1", "MLB2"]),
                    stars: [1, 1, 1, 0, 4],
                    unseen_low: 3,
                },
                ProductReviews {
                    product: "B".into(),
                    name: "Produto B".into(),
                    listings: listings(&["MLB3"]),
                    stars: [0, 0, 0, 2, 4],
                    unseen_low: 0,
                },
                ProductReviews {
                    product: "C".into(),
                    name: "Produto C".into(),
                    listings: listings(&["MLB9"]),
                    stars: [0; 5],
                    unseen_low: 0,
                },
            ]
        );
        // (1 + 2 + 3 + 5 × 4) / 7
        assert_eq!(
            products[0].average().unwrap().round_dp(2),
            Decimal::new(371, 2)
        );
        assert_eq!(products[0].count(), 7);
        assert_eq!(products[0].low(), 3);
        assert_eq!(products[2].average(), None);
    });
}

#[test]
fn a_low_review_the_owner_looked_at_stays_seen_and_one_the_channel_drops_goes() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.channel.reviewed(
            "MLB1",
            [1, 0, 1, 0, 0],
            vec![review("R1", 1, 5), review("R3", 3, 1)],
        );
        let open = listings(&["MLB1"]);
        fixture.sync(&open).await;

        let low = fixture.reputation.low_reviews().await.unwrap();
        assert_eq!(low.len(), 2);
        assert_eq!(low[0].review.id, "R3", "newest first");
        fixture.reputation.mark_seen(low[0].id).await.unwrap();
        // Seeing it twice is no error.
        fixture.reputation.mark_seen(low[0].id).await.unwrap();

        let low = fixture.reputation.low_reviews().await.unwrap();
        assert_eq!(low[0].review.id, "R1", "not seen yet first");
        assert!(!low[0].seen);
        assert!(low[1].seen);

        fixture
            .channel
            .reviewed("MLB1", [0, 0, 1, 0, 0], vec![review("R3", 3, 1)]);
        assert_eq!(fixture.sync(&open).await.arrived, []);
        let low = fixture.reputation.low_reviews().await.unwrap();
        assert_eq!(low.len(), 1);
        assert_eq!(low[0].review.id, "R3");
        assert!(low[0].seen, "the Sync keeps what the owner looked at");
    });
}

#[test]
fn marking_an_unknown_review_seen_is_not_found() {
    block_on(async {
        let fixture = Fixture::new().await;
        let error = fixture
            .reputation
            .mark_seen(uuid_for_test())
            .await
            .unwrap_err();
        assert!(matches!(error, ReputationError::NotFound), "{error:?}");
    });
}

fn uuid_for_test() -> mascate_kernel::RecordId {
    use mascate_kernel::IdGenerator;
    let ids = SequentialIds::default();
    for _ in 0..99 {
        ids.next_id();
    }
    ids.next_id()
}

#[test]
fn nothing_is_kept_when_the_reviews_of_a_listing_fail() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .rates(seller(Some(ReputationColor::Green), 64));
        fixture
            .channel
            .reviewed("MLB1", [1, 0, 0, 0, 0], vec![review("R1", 1, 1)]);
        *fixture.channel.failing.lock().unwrap() = Some("MLB2".into());

        let error = fixture
            .reputation
            .sync(&fixture.channel, &listings(&["MLB1", "MLB2"]))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ReputationError::Platform(PlatformError::RateLimited)
        ));
        assert_eq!(fixture.reputation.standing().await.unwrap(), None);
        assert_eq!(fixture.reputation.low_reviews().await.unwrap(), []);
        assert_eq!(fixture.reputation.unlocked().await.unwrap(), []);
    });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// The room is exactly how many more cases keep the rate at or under
    /// the green limit in the period's sales.
    #[test]
    fn the_room_to_the_green_is_the_last_case_that_still_fits(
        sales in 1u32..5_000,
        count in 0u32..400,
    ) {
        block_on(async {
            let fixture = Fixture::new().await;
            fixture.channel.rates(ChannelReputation {
                sales,
                claims: MetricReading { rate: Percentage::ZERO, count },
                ..seller(None, 0)
            });
            fixture.sync(&[]).await;
            let standing = fixture.reputation.standing().await.unwrap().unwrap();
            let claims = standing.metrics[0];
            let room = claims.room.unwrap();
            let limit = claims.green_limit.percent();
            let rate = |cases: i64| Decimal::from(cases) * Decimal::ONE_HUNDRED / Decimal::from(sales);
            let last = i64::from(count) + room;
            if last >= 0 {
                prop_assert!(rate(last) <= limit);
            }
            prop_assert!(rate(last + 1) > limit);
            Ok(())
        })?;
    }
}
