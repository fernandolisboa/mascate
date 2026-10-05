//! Reputation and Reviews (#25): the owner's seller Reputation in the Sales
//! Channel, each metric against the limits that keep its color, the
//! Reviews of each listing summed up by Product with the low ones to look
//! at, and a notice, once, when the Reputation unlocks Promotions or
//! Product Ads. Read in each Sync through a port a Platform's adapter
//! implements (ADR 0013); the listings and their Products come from the
//! caller (ADR 0021). Nothing here writes to the channel (ADR 0023).

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use chrono::TimeDelta;
use libsql::{Connection, TransactionBehavior, Value, params};
use mascate_kernel::{Clock, IdGenerator, Percentage, PlatformError, RecordId, Timestamp};
use mascate_platform::{
    Database, Migration, StoredRow, StoredValueError, load_single_row, save_single_row, stored,
};
use rust_decimal::Decimal;

pub(crate) const CREATE_REPUTATION: Migration = Migration {
    version: 3,
    name: "create the seller Reputation and the Reviews",
    risky: false,
    sql: "CREATE TABLE marketing_reputation (
        id                 TEXT    PRIMARY KEY,
        color              TEXT,
        real_color         TEXT,
        protected_until    TEXT,
        period_days        INTEGER,
        sales              INTEGER NOT NULL,
        transactions       INTEGER NOT NULL,
        claims_rate        TEXT    NOT NULL,
        claims             INTEGER NOT NULL,
        cancellations_rate TEXT    NOT NULL,
        cancellations      INTEGER NOT NULL,
        delayed_rate       TEXT    NOT NULL,
        delayed            INTEGER NOT NULL,
        checked_at         TEXT    NOT NULL,
        created_at         TEXT    NOT NULL,
        updated_at         TEXT    NOT NULL,
        deleted_at         TEXT
    );
    CREATE TABLE marketing_listing_reviews (
        id          TEXT    PRIMARY KEY,
        ml_item_id  TEXT    NOT NULL,
        one_star    INTEGER NOT NULL,
        two_stars   INTEGER NOT NULL,
        three_stars INTEGER NOT NULL,
        four_stars  INTEGER NOT NULL,
        five_stars  INTEGER NOT NULL,
        checked_at  TEXT    NOT NULL,
        created_at  TEXT    NOT NULL,
        updated_at  TEXT    NOT NULL,
        deleted_at  TEXT
    );
    CREATE UNIQUE INDEX marketing_listing_reviews_by_item
        ON marketing_listing_reviews (ml_item_id)
        WHERE deleted_at IS NULL;
    CREATE TABLE marketing_low_reviews (
        id           TEXT    PRIMARY KEY,
        ml_review_id TEXT    NOT NULL,
        ml_item_id   TEXT    NOT NULL,
        rating       INTEGER NOT NULL,
        title        TEXT    NOT NULL,
        text         TEXT    NOT NULL,
        reviewed_at  TEXT    NOT NULL,
        seen_at      TEXT,
        created_at   TEXT    NOT NULL,
        updated_at   TEXT    NOT NULL,
        deleted_at   TEXT
    );
    CREATE UNIQUE INDEX marketing_low_reviews_by_review
        ON marketing_low_reviews (ml_review_id)
        WHERE deleted_at IS NULL;
    CREATE TABLE marketing_seller_tools (
        id              TEXT    PRIMARY KEY,
        tool            TEXT    NOT NULL,
        available       INTEGER NOT NULL,
        available_since TEXT,
        seen_at         TEXT,
        created_at      TEXT    NOT NULL,
        updated_at      TEXT    NOT NULL,
        deleted_at      TEXT
    );
    CREATE UNIQUE INDEX marketing_seller_tools_by_tool
        ON marketing_seller_tools (tool)
        WHERE deleted_at IS NULL;",
};

const REPUTATION_TABLE: &str = "marketing_reputation";
const REPUTATION_COLUMNS: &[&str] = &[
    "color",
    "real_color",
    "protected_until",
    "period_days",
    "sales",
    "transactions",
    "claims_rate",
    "claims",
    "cancellations_rate",
    "cancellations",
    "delayed_rate",
    "delayed",
    "checked_at",
];

/// A Review of this many stars or fewer is a low one, worth a look.
pub const LOW_RATING: u8 = 3;

/// How often the Reputation and the Reviews are read again: they move by
/// the day, so every Order Sync would only spend calls.
pub const REFRESH_MINUTES: i64 = 60;

/// The completed sales Product Ads asks for besides the color. Not in the
/// documentation consulted: to be checked with the real account (#37).
pub const PRODUCT_ADS_MIN_SALES: u32 = 10;

/// The color of a seller's Reputation, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReputationColor {
    Red,
    Orange,
    Yellow,
    LightGreen,
    Green,
}

impl ReputationColor {
    const ALL: [ReputationColor; 5] = [
        ReputationColor::Red,
        ReputationColor::Orange,
        ReputationColor::Yellow,
        ReputationColor::LightGreen,
        ReputationColor::Green,
    ];

    fn code(self) -> &'static str {
        match self {
            ReputationColor::Red => "red",
            ReputationColor::Orange => "orange",
            ReputationColor::Yellow => "yellow",
            ReputationColor::LightGreen => "light_green",
            ReputationColor::Green => "green",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|color| color.code() == code)
    }

    /// As Mercado Livre Brasil names it to the seller.
    pub fn name(self) -> &'static str {
        match self {
            ReputationColor::Red => "Vermelha",
            ReputationColor::Orange => "Laranja",
            ReputationColor::Yellow => "Amarela",
            ReputationColor::LightGreen => "Verde-clara",
            ReputationColor::Green => "Verde",
        }
    }
}

/// What the Reputation measures in the period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReputationMetric {
    Claims,
    Cancellations,
    DelayedHandling,
}

impl ReputationMetric {
    pub const ALL: [ReputationMetric; 3] = [
        ReputationMetric::Claims,
        ReputationMetric::Cancellations,
        ReputationMetric::DelayedHandling,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ReputationMetric::Claims => "Reclamações",
            ReputationMetric::Cancellations => "Cancelamentos",
            ReputationMetric::DelayedHandling => "Despachos com atraso",
        }
    }

    /// The highest rate, in percent, each color allows for this metric on
    /// Mercado Livre Brasil, best color first; above the last one the color
    /// is red. From the table "Limites para cada variável" of the
    /// documentation, which has no column for light green; to be checked
    /// with the real account (#37).
    fn limits(self) -> [(ReputationColor, Decimal); 3] {
        let (green, yellow, orange) = match self {
            ReputationMetric::Claims => (Decimal::new(2, 0), Decimal::new(45, 1), 8.into()),
            ReputationMetric::Cancellations => (Decimal::new(15, 1), Decimal::new(35, 1), 4.into()),
            ReputationMetric::DelayedHandling => (10.into(), 18.into(), 22.into()),
        };
        [
            (ReputationColor::Green, green),
            (ReputationColor::Yellow, yellow),
            (ReputationColor::Orange, orange),
        ]
    }
}

/// One metric as the channel measured it in the period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricReading {
    pub rate: Percentage,
    /// The sales it happened to.
    pub count: u32,
}

impl MetricReading {
    pub const NONE: MetricReading = MetricReading {
        rate: Percentage::ZERO,
        count: 0,
    };
}

/// The seller's Reputation as the channel reports it. While the channel
/// protects a new or recovering seller, the metrics are the real ones it
/// shows apart, not the zeros that protect the color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelReputation {
    /// `None` until the seller has sales enough for one.
    pub color: Option<ReputationColor>,
    /// The color the metrics alone give while a protection holds `color`.
    pub real_color: Option<ReputationColor>,
    pub protected_until: Option<Timestamp>,
    /// The days the metrics cover; `None` when the channel counts every
    /// sale.
    pub period_days: Option<u32>,
    /// Sales completed in the period.
    pub sales: u32,
    /// Sales completed ever.
    pub transactions: u32,
    pub claims: MetricReading,
    pub cancellations: MetricReading,
    pub delayed_handling: MetricReading,
}

impl ChannelReputation {
    pub fn reading(&self, metric: ReputationMetric) -> MetricReading {
        match metric {
            ReputationMetric::Claims => self.claims,
            ReputationMetric::Cancellations => self.cancellations,
            ReputationMetric::DelayedHandling => self.delayed_handling,
        }
    }
}

/// A buyer's Review of a listing, as the channel reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelReview {
    /// The channel's id of the Review: what keeps it from showing twice.
    pub id: String,
    /// From 1 to 5 stars.
    pub rating: u8,
    pub title: String,
    pub text: String,
    pub at: Timestamp,
}

/// A listing's Reviews, as the channel reports them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListingReviews {
    /// How many Reviews gave each number of stars, one star first.
    pub stars: [u32; 5],
    /// Every Review of [`LOW_RATING`] stars or fewer.
    pub low: Vec<ChannelReview>,
}

/// How a Sales Channel rates the seller and what buyers say of each
/// listing, as a Platform's adapter reports it. Calls block on the network,
/// so they run off the UI thread.
pub trait ReputationSource: Send + Sync {
    fn reputation(&self) -> Result<ChannelReputation, PlatformError>;

    /// The Reviews of the listing `listing` (the channel's id).
    fn reviews(&self, listing: &str) -> Result<ListingReviews, PlatformError>;
}

/// A channel tool the Reputation unlocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SellerTool {
    Promotions,
    ProductAds,
}

impl SellerTool {
    pub const ALL: [SellerTool; 2] = [SellerTool::Promotions, SellerTool::ProductAds];

    fn code(self) -> &'static str {
        match self {
            SellerTool::Promotions => "promotions",
            SellerTool::ProductAds => "product_ads",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.code() == code)
    }

    pub fn name(self) -> &'static str {
        match self {
            SellerTool::Promotions => "Promoções",
            SellerTool::ProductAds => "Product Ads",
        }
    }

    /// What it takes, in the owner's words.
    pub fn requirement(self) -> String {
        match self {
            SellerTool::Promotions => "reputação verde".into(),
            SellerTool::ProductAds => {
                format!("reputação amarela ou melhor e {PRODUCT_ADS_MIN_SALES} vendas concluídas")
            }
        }
    }

    /// Whether `reputation` unlocks it: Promotions with the green, Product
    /// Ads with the yellow or better and [`PRODUCT_ADS_MIN_SALES`].
    pub fn unlocked_by(self, reputation: &ChannelReputation) -> bool {
        match self {
            SellerTool::Promotions => reputation.color == Some(ReputationColor::Green),
            SellerTool::ProductAds => {
                reputation.color >= Some(ReputationColor::Yellow)
                    && reputation.transactions >= PRODUCT_ADS_MIN_SALES
            }
        }
    }
}

/// How one metric stands against its limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricStanding {
    pub metric: ReputationMetric,
    pub reading: MetricReading,
    /// The highest rate that keeps the green.
    pub green_limit: Percentage,
    /// The best color the rate allows on its own.
    pub fits: ReputationColor,
    /// How many more cases the sales of the period take before the rate
    /// passes the green limit; below zero, how many cases it is past it.
    /// `None` without sales in the period.
    pub room: Option<i64>,
}

impl MetricStanding {
    fn of(metric: ReputationMetric, reading: MetricReading, sales: u32) -> Self {
        let limits = metric.limits();
        let fits = limits
            .iter()
            .find(|(_, limit)| reading.rate.percent() <= *limit)
            .map_or(ReputationColor::Red, |(color, _)| *color);
        let green = limits[0].1;
        let room = (sales > 0).then(|| {
            let allowed = (green * Decimal::from(sales) / Decimal::ONE_HUNDRED).floor();
            i64::try_from(allowed).unwrap_or(i64::MAX) - i64::from(reading.count)
        });
        Self {
            metric,
            reading,
            green_limit: Percentage::new(green).unwrap_or(Percentage::ZERO),
            fits,
            room,
        }
    }

    /// Percentage points from the rate to the green limit; below zero when
    /// the rate is past it.
    pub fn points_to_green(&self) -> Decimal {
        self.green_limit.percent() - self.reading.rate.percent()
    }
}

/// The Reputation as last read, each metric against its limits and what it
/// unlocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReputationStanding {
    pub reputation: ChannelReputation,
    pub metrics: Vec<MetricStanding>,
    /// Each tool and whether the Reputation unlocks it.
    pub tools: Vec<(SellerTool, bool)>,
    pub checked_at: Timestamp,
}

impl ReputationStanding {
    fn of(reputation: ChannelReputation, checked_at: Timestamp) -> Self {
        let metrics = ReputationMetric::ALL
            .into_iter()
            .map(|metric| MetricStanding::of(metric, reputation.reading(metric), reputation.sales))
            .collect();
        let tools = SellerTool::ALL
            .into_iter()
            .map(|tool| (tool, tool.unlocked_by(&reputation)))
            .collect();
        Self {
            reputation,
            metrics,
            tools,
            checked_at,
        }
    }
}

/// A listing the owner has in the channel and the Product it sells, as the
/// caller knows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedItem {
    /// The channel's id of the listing.
    pub listing: String,
    /// What groups listings into one Product: the caller's key for it.
    pub product: String,
    /// The Product's name, or the listing's title when it has none.
    pub name: String,
}

/// The Reviews of a Product, its listings together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductReviews {
    pub product: String,
    pub name: String,
    /// The channel's ids of its listings.
    pub listings: Vec<String>,
    /// How many Reviews gave each number of stars, one star first.
    pub stars: [u32; 5],
    /// Low Reviews the owner has not looked at yet.
    pub unseen_low: usize,
}

impl ProductReviews {
    pub fn count(&self) -> u32 {
        self.stars.iter().sum()
    }

    /// The mean of the stars; `None` without Reviews.
    pub fn average(&self) -> Option<Decimal> {
        let count = self.count();
        (count > 0).then(|| {
            let total: u32 = (1..).zip(self.stars).map(|(stars, n)| stars * n).sum();
            Decimal::from(total) / Decimal::from(count)
        })
    }

    pub fn low(&self) -> u32 {
        self.stars[..usize::from(LOW_RATING)].iter().sum()
    }
}

/// A low Review as the app keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LowReview {
    pub id: RecordId,
    /// The channel's id of the listing reviewed.
    pub listing: String,
    pub review: ChannelReview,
    /// Whether the owner already looked at it.
    pub seen: bool,
}

/// A low Review the app had not seen before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrivedReview {
    pub listing: String,
    pub review: ChannelReview,
}

/// What a Sync of the Reputation and the Reviews did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReputationSync {
    pub color: Option<ReputationColor>,
    /// Tools the Reputation unlocked since the last Sync.
    pub unlocked: Vec<SellerTool>,
    /// Low Reviews the app had not seen before, oldest first.
    pub arrived: Vec<ArrivedReview>,
    /// Listings whose Reviews were read.
    pub reviewed: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ReputationError {
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error("no such Review")]
    NotFound,
    #[error("the Reputation holds a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for ReputationError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => ReputationError::Unreadable(text),
            StoredValueError::Sql(error) => ReputationError::Sql(error),
        }
    }
}

/// The owner's seller Reputation, the Reviews of the listings and the tools
/// the Reputation unlocks.
pub struct Reputation {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl Reputation {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            database,
            clock,
            ids,
        }
    }

    /// Whether the last Sync is [`REFRESH_MINUTES`] old, or there was none.
    pub async fn is_due(&self) -> Result<bool, ReputationError> {
        Ok(match self.last_sync().await? {
            Some(at) => self.clock.now() - at >= TimeDelta::minutes(REFRESH_MINUTES),
            None => true,
        })
    }

    /// Reads the Reputation and the Reviews of each of `listings` (the
    /// channel's ids, each once). Each low Review is kept by the channel's
    /// id: the same answers again change nothing, and one the channel no
    /// longer reports is gone. A tool counts as unlocked once each time
    /// the Reputation reaches it. Nothing is kept unless everything was
    /// read.
    pub async fn sync(
        &self,
        source: &dyn ReputationSource,
        listings: &[String],
    ) -> Result<ReputationSync, ReputationError> {
        let reputation = source.reputation()?;
        let mut seen = HashSet::new();
        let mut read = Vec::new();
        for listing in listings.iter().filter(|id| seen.insert(id.as_str())) {
            read.push((listing, source.reviews(listing)?));
        }

        let now = self.clock.now();
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        save_single_row(
            &transaction,
            self.clock.as_ref(),
            self.ids.as_ref(),
            REPUTATION_TABLE,
            REPUTATION_COLUMNS,
            reputation_values(&reputation, now),
        )
        .await?;
        let mut report = ReputationSync {
            color: reputation.color,
            reviewed: read.len(),
            ..ReputationSync::default()
        };
        for (listing, reviews) in read {
            self.write_reviews(&transaction, listing, &reviews, now, &mut report.arrived)
                .await?;
        }
        for tool in SellerTool::ALL {
            if self
                .write_tool(&transaction, tool, tool.unlocked_by(&reputation), now)
                .await?
            {
                report.unlocked.push(tool);
            }
        }
        transaction.commit().await?;
        report.arrived.sort_by_key(|arrived| arrived.review.at);
        Ok(report)
    }

    /// When the Reputation was last read; `None` before the first Sync.
    pub async fn last_sync(&self) -> Result<Option<Timestamp>, ReputationError> {
        Ok(self.standing().await?.map(|standing| standing.checked_at))
    }

    /// The Reputation as last read; `None` before the first Sync.
    pub async fn standing(&self) -> Result<Option<ReputationStanding>, ReputationError> {
        let Some(row) = load_single_row(
            self.database.connection(),
            REPUTATION_TABLE,
            REPUTATION_COLUMNS,
        )
        .await?
        else {
            return Ok(None);
        };
        let color = |at: i32| -> Result<Option<ReputationColor>, ReputationError> {
            row.get::<Option<String>>(at)?
                .map(|code| {
                    ReputationColor::from_code(&code).ok_or(ReputationError::Unreadable(code))
                })
                .transpose()
        };
        let reading = |rate: i32, count: i32| -> Result<MetricReading, ReputationError> {
            let percent = row.decimal_at(rate)?;
            Ok(MetricReading {
                rate: Percentage::new(percent)
                    .map_err(|_| ReputationError::Unreadable(percent.to_string()))?,
                count: row.get(count)?,
            })
        };
        let reputation = ChannelReputation {
            color: color(0)?,
            real_color: color(1)?,
            protected_until: row.optional_time_at(2)?,
            period_days: row.get(3)?,
            sales: row.get(4)?,
            transactions: row.get(5)?,
            claims: reading(6, 7)?,
            cancellations: reading(8, 9)?,
            delayed_handling: reading(10, 11)?,
        };
        Ok(Some(ReputationStanding::of(reputation, row.time_at(12)?)))
    }

    /// The Reviews of each Product `items` sell (the first item of each
    /// listing), the ones with low Reviews still to look at first, then
    /// the lowest average; Products without Reviews last.
    pub async fn reviews(
        &self,
        items: &[ReviewedItem],
    ) -> Result<Vec<ProductReviews>, ReputationError> {
        let on = self.database.connection();
        let stars = read_stars(on).await?;
        let mut unseen: BTreeMap<String, usize> = BTreeMap::new();
        for low in read_low_reviews(on).await? {
            if !low.seen {
                *unseen.entry(low.listing).or_default() += 1;
            }
        }
        let mut seen = HashSet::new();
        let mut products: BTreeMap<&str, ProductReviews> = BTreeMap::new();
        for item in items
            .iter()
            .filter(|item| seen.insert(item.listing.as_str()))
        {
            let product = products
                .entry(item.product.as_str())
                .or_insert_with(|| ProductReviews {
                    product: item.product.clone(),
                    name: item.name.clone(),
                    listings: Vec::new(),
                    stars: [0; 5],
                    unseen_low: 0,
                });
            product.listings.push(item.listing.clone());
            if let Some(listed) = stars.get(&item.listing) {
                for (sum, count) in product.stars.iter_mut().zip(listed) {
                    *sum += count;
                }
            }
            product.unseen_low += unseen.get(&item.listing).copied().unwrap_or(0);
        }
        let mut products: Vec<ProductReviews> = products.into_values().collect();
        products.sort_by(|a, b| {
            (a.unseen_low == 0)
                .cmp(&(b.unseen_low == 0))
                .then((a.count() == 0).cmp(&(b.count() == 0)))
                .then(a.average().cmp(&b.average()))
                .then(a.name.cmp(&b.name))
        });
        Ok(products)
    }

    /// Every low Review kept: those the owner has not looked at first, the
    /// newest on top.
    pub async fn low_reviews(&self) -> Result<Vec<LowReview>, ReputationError> {
        let mut reviews = read_low_reviews(self.database.connection()).await?;
        reviews.sort_by(|a, b| a.seen.cmp(&b.seen).then(b.review.at.cmp(&a.review.at)));
        Ok(reviews)
    }

    /// The owner looked at the low Review `review`.
    pub async fn mark_seen(&self, review: RecordId) -> Result<(), ReputationError> {
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE marketing_low_reviews SET seen_at = ?1, updated_at = ?1
                 WHERE id = ?2 AND deleted_at IS NULL AND seen_at IS NULL",
                params![stored(self.clock.now()), review.to_string()],
            )
            .await?;
        if changed == 0 && !self.has_low_review(review).await? {
            return Err(ReputationError::NotFound);
        }
        Ok(())
    }

    /// The tools the Reputation unlocked that the owner has not dismissed
    /// the notice of yet.
    pub async fn unlocked(&self) -> Result<Vec<SellerTool>, ReputationError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT tool FROM marketing_seller_tools
                 WHERE deleted_at IS NULL AND available = 1 AND seen_at IS NULL",
                (),
            )
            .await?;
        let mut tools = Vec::new();
        while let Some(row) = rows.next().await? {
            let code = row.get::<String>(0)?;
            tools.push(SellerTool::from_code(&code).ok_or(ReputationError::Unreadable(code))?);
        }
        tools.sort();
        Ok(tools)
    }

    /// The owner read the notice that `tool` is unlocked.
    pub async fn dismiss(&self, tool: SellerTool) -> Result<(), ReputationError> {
        self.database
            .connection()
            .execute(
                "UPDATE marketing_seller_tools SET seen_at = ?1, updated_at = ?1
                 WHERE tool = ?2 AND deleted_at IS NULL AND seen_at IS NULL",
                params![stored(self.clock.now()), tool.code()],
            )
            .await?;
        Ok(())
    }

    async fn has_low_review(&self, review: RecordId) -> Result<bool, ReputationError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT 1 FROM marketing_low_reviews WHERE id = ?1 AND deleted_at IS NULL",
                params![review.to_string()],
            )
            .await?;
        Ok(rows.next().await?.is_some())
    }

    /// Writes a listing's stars and its low Reviews by the channel's ids,
    /// the ones it no longer reports gone, and adds the new ones to
    /// `arrived`.
    async fn write_reviews(
        &self,
        on: &Connection,
        listing: &str,
        reviews: &ListingReviews,
        now: Timestamp,
        arrived: &mut Vec<ArrivedReview>,
    ) -> Result<(), ReputationError> {
        let [one, two, three, four, five] = reviews.stars;
        on.execute(
            "INSERT INTO marketing_listing_reviews
                 (id, ml_item_id, one_star, two_stars, three_stars, four_stars, five_stars,
                  checked_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?8)
             ON CONFLICT (ml_item_id) WHERE deleted_at IS NULL
             DO UPDATE SET one_star = excluded.one_star, two_stars = excluded.two_stars,
                 three_stars = excluded.three_stars, four_stars = excluded.four_stars,
                 five_stars = excluded.five_stars, checked_at = excluded.checked_at,
                 updated_at = excluded.updated_at",
            params![
                self.ids.next_id().to_string(),
                listing,
                one,
                two,
                three,
                four,
                five,
                stored(now)
            ],
        )
        .await?;

        let mut kept = HashSet::new();
        for review in reviews
            .low
            .iter()
            .filter(|review| review.rating <= LOW_RATING)
        {
            if !kept.insert(review.id.as_str()) {
                continue;
            }
            let mut rows = on
                .query(
                    "SELECT 1 FROM marketing_low_reviews
                     WHERE ml_review_id = ?1 AND deleted_at IS NULL",
                    params![review.id.clone()],
                )
                .await?;
            if rows.next().await?.is_none() {
                arrived.push(ArrivedReview {
                    listing: listing.to_owned(),
                    review: review.clone(),
                });
            }
            drop(rows);
            on.execute(
                "INSERT INTO marketing_low_reviews
                     (id, ml_review_id, ml_item_id, rating, title, text, reviewed_at,
                      created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 ON CONFLICT (ml_review_id) WHERE deleted_at IS NULL
                 DO UPDATE SET ml_item_id = excluded.ml_item_id, rating = excluded.rating,
                     title = excluded.title, text = excluded.text,
                     reviewed_at = excluded.reviewed_at, updated_at = excluded.updated_at",
                params![
                    self.ids.next_id().to_string(),
                    review.id.clone(),
                    listing,
                    u32::from(review.rating),
                    review.title.clone(),
                    review.text.clone(),
                    stored(review.at),
                    stored(now)
                ],
            )
            .await?;
        }

        let mut rows = on
            .query(
                "SELECT id, ml_review_id FROM marketing_low_reviews
                 WHERE ml_item_id = ?1 AND deleted_at IS NULL",
                params![listing],
            )
            .await?;
        let mut gone = Vec::new();
        while let Some(row) = rows.next().await? {
            if !kept.contains(row.get::<String>(1)?.as_str()) {
                gone.push(row.id_at(0)?);
            }
        }
        drop(rows);
        for id in gone {
            on.execute(
                "UPDATE marketing_low_reviews SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                params![stored(now), id.to_string()],
            )
            .await?;
        }
        Ok(())
    }

    /// Writes whether `tool` is unlocked now; `true` when it just became
    /// so. A tool that locks again clears its notice, so the next unlock
    /// tells the owner again.
    async fn write_tool(
        &self,
        on: &Connection,
        tool: SellerTool,
        unlocked: bool,
        now: Timestamp,
    ) -> Result<bool, ReputationError> {
        let mut rows = on
            .query(
                "SELECT available FROM marketing_seller_tools
                 WHERE tool = ?1 AND deleted_at IS NULL",
                params![tool.code()],
            )
            .await?;
        let was = match rows.next().await? {
            Some(row) => Some(row.get::<i64>(0)? == 1),
            None => None,
        };
        drop(rows);
        if was == Some(unlocked) {
            return Ok(false);
        }
        let since = unlocked.then(|| stored(now));
        match was {
            None => {
                on.execute(
                    "INSERT INTO marketing_seller_tools
                         (id, tool, available, available_since, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                    params![
                        self.ids.next_id().to_string(),
                        tool.code(),
                        i64::from(unlocked),
                        since,
                        stored(now)
                    ],
                )
                .await?;
            }
            Some(_) => {
                on.execute(
                    "UPDATE marketing_seller_tools
                     SET available = ?1, available_since = ?2, seen_at = NULL, updated_at = ?3
                     WHERE tool = ?4 AND deleted_at IS NULL",
                    params![i64::from(unlocked), since, stored(now), tool.code()],
                )
                .await?;
            }
        }
        Ok(unlocked)
    }
}

fn reputation_values(reputation: &ChannelReputation, now: Timestamp) -> Vec<Value> {
    let color = |color: Option<ReputationColor>| {
        color.map_or(Value::Null, |color| Value::Text(color.code().into()))
    };
    let rate = |reading: MetricReading| Value::Text(reading.rate.percent().to_string());
    let count = |reading: MetricReading| Value::Integer(reading.count.into());
    vec![
        color(reputation.color),
        color(reputation.real_color),
        reputation
            .protected_until
            .map_or(Value::Null, |at| Value::Text(stored(at))),
        reputation
            .period_days
            .map_or(Value::Null, |days| Value::Integer(days.into())),
        Value::Integer(reputation.sales.into()),
        Value::Integer(reputation.transactions.into()),
        rate(reputation.claims),
        count(reputation.claims),
        rate(reputation.cancellations),
        count(reputation.cancellations),
        rate(reputation.delayed_handling),
        count(reputation.delayed_handling),
        Value::Text(stored(now)),
    ]
}

/// Every listing's stars as kept, by the channel's id.
async fn read_stars(on: &Connection) -> Result<BTreeMap<String, [u32; 5]>, ReputationError> {
    let mut rows = on
        .query(
            "SELECT ml_item_id, one_star, two_stars, three_stars, four_stars, five_stars
             FROM marketing_listing_reviews WHERE deleted_at IS NULL",
            (),
        )
        .await?;
    let mut stars = BTreeMap::new();
    while let Some(row) = rows.next().await? {
        stars.insert(
            row.get::<String>(0)?,
            [
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ],
        );
    }
    Ok(stars)
}

/// Every low Review kept, in no particular order.
async fn read_low_reviews(on: &Connection) -> Result<Vec<LowReview>, ReputationError> {
    let mut rows = on
        .query(
            "SELECT id, ml_review_id, ml_item_id, rating, title, text, reviewed_at, seen_at
             FROM marketing_low_reviews WHERE deleted_at IS NULL",
            (),
        )
        .await?;
    let mut reviews = Vec::new();
    while let Some(row) = rows.next().await? {
        let rating = row.get::<u32>(3)?;
        reviews.push(LowReview {
            id: row.id_at(0)?,
            listing: row.get(2)?,
            review: ChannelReview {
                id: row.get(1)?,
                rating: u8::try_from(rating)
                    .map_err(|_| ReputationError::Unreadable(rating.to_string()))?,
                title: row.get(4)?,
                text: row.get(5)?,
                at: row.time_at(6)?,
            },
            seen: row.optional_time_at(7)?.is_some(),
        });
    }
    Ok(reviews)
}
