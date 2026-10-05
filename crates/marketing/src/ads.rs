//! Product Ads (#27): what the owner's ads on the Sales Channel cost and
//! brought in, listing by listing and day by day, read once a day through a
//! port a Platform's adapter implements (ADR 0013), and each Product's ROAS
//! against the least that pays for its ads. Campaigns are created and
//! changed in the channel's own panel: nothing here writes to it. The
//! listings, their Products and break-even come from the caller (ADR 0021),
//! and the caller hands the cost to Commerce for the Realized Margin (ADR
//! 0025).

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use chrono::{NaiveDate, TimeDelta};
use libsql::{Connection, TransactionBehavior, Value, params};
use mascate_kernel::{
    BreakEven, Clock, Currency, CurrencyMismatch, IdGenerator, Money, PlatformError, Roas,
    Timestamp, channel_day, channel_day_start,
};
use mascate_platform::{
    Database, Migration, StoredRow, StoredValueError, load_single_row, save_single_row, stored,
};

pub(crate) const CREATE_PRODUCT_ADS: Migration = Migration {
    version: 4,
    name: "create the Product Ads metrics",
    risky: false,
    sql: "CREATE TABLE marketing_ads_syncs (
        id           TEXT PRIMARY KEY,
        account      TEXT,
        read_through TEXT,
        synced_at    TEXT NOT NULL,
        created_at   TEXT NOT NULL,
        updated_at   TEXT NOT NULL,
        deleted_at   TEXT
    );
    CREATE TABLE marketing_ad_campaigns (
        id             TEXT PRIMARY KEY,
        ml_campaign_id TEXT NOT NULL,
        name           TEXT NOT NULL,
        status         TEXT NOT NULL,
        created_at     TEXT NOT NULL,
        updated_at     TEXT NOT NULL,
        deleted_at     TEXT
    );
    CREATE UNIQUE INDEX marketing_ad_campaigns_by_campaign
        ON marketing_ad_campaigns (ml_campaign_id)
        WHERE deleted_at IS NULL;
    CREATE TABLE marketing_ad_days (
        id             TEXT    PRIMARY KEY,
        ml_item_id     TEXT    NOT NULL,
        day            TEXT    NOT NULL,
        ml_campaign_id TEXT,
        cost           TEXT    NOT NULL,
        attributed     TEXT    NOT NULL,
        currency       TEXT    NOT NULL,
        clicks         INTEGER NOT NULL,
        prints         INTEGER NOT NULL,
        units          INTEGER NOT NULL,
        created_at     TEXT    NOT NULL,
        updated_at     TEXT    NOT NULL,
        deleted_at     TEXT
    );
    CREATE UNIQUE INDEX marketing_ad_days_by_item_and_day
        ON marketing_ad_days (ml_item_id, day)
        WHERE deleted_at IS NULL;",
};

const SYNCS_TABLE: &str = "marketing_ads_syncs";
const SYNCS_COLUMNS: &[&str] = &["account", "read_through", "synced_at"];

/// How often Product Ads are read: the channel reports them by the day.
pub const ADS_REFRESH_MINUTES: i64 = 24 * 60;

/// Days the first Sync reads back: this month and the last, whole.
pub const ADS_HISTORY_DAYS: i64 = 62;

/// Days read again in each Sync: a sale the channel attributes to a click
/// can come some days after it. Not in the documentation consulted: to be
/// checked with the real account (#37).
pub const ADS_SETTLE_DAYS: i64 = 7;

/// How the channel runs a campaign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CampaignStatus {
    Active,
    Paused,
    /// Any other state the channel reports, such as one being set up.
    Other,
}

impl CampaignStatus {
    const ALL: [CampaignStatus; 3] = [
        CampaignStatus::Active,
        CampaignStatus::Paused,
        CampaignStatus::Other,
    ];

    fn code(self) -> &'static str {
        match self {
            CampaignStatus::Active => "active",
            CampaignStatus::Paused => "paused",
            CampaignStatus::Other => "other",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.code() == code)
    }

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            CampaignStatus::Active => "Ativa",
            CampaignStatus::Paused => "Pausada",
            CampaignStatus::Other => "Outro estado",
        }
    }
}

/// A Product Ads campaign, as the channel reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelCampaign {
    pub id: String,
    pub name: String,
    pub status: CampaignStatus,
}

/// What ads cost and brought in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdMetrics {
    pub cost: Money,
    /// The sales the channel attributes to the ads: of the advertised
    /// listing and of the owner's other listings bought after a click.
    pub attributed: Money,
    pub clicks: u64,
    /// Times the ads were shown.
    pub prints: u64,
    /// Units the attributed sales sold.
    pub units: u64,
}

impl AdMetrics {
    pub fn zero(currency: Currency) -> Self {
        Self {
            cost: Money::zero(currency),
            attributed: Money::zero(currency),
            clicks: 0,
            prints: 0,
            units: 0,
        }
    }

    /// What the attributed sales brought in for each real spent; `None`
    /// when nothing was spent.
    pub fn roas(&self) -> Option<Roas> {
        Roas::of(self.attributed, self.cost).ok().flatten()
    }

    fn is_empty(&self) -> bool {
        self.cost.amount().is_zero()
            && self.attributed.amount().is_zero()
            && self.clicks == 0
            && self.prints == 0
            && self.units == 0
    }

    fn add(&mut self, other: &AdMetrics) -> Result<(), CurrencyMismatch> {
        self.cost = self.cost.checked_add(other.cost)?;
        self.attributed = self.attributed.checked_add(other.attributed)?;
        self.clicks += other.clicks;
        self.prints += other.prints;
        self.units += other.units;
        Ok(())
    }
}

/// One ad's day, as the channel reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelAd {
    /// The channel's id of the advertised listing.
    pub listing: String,
    /// The channel's id of the campaign it runs in.
    pub campaign: Option<String>,
    pub metrics: AdMetrics,
}

/// The owner's Product Ads, as a Platform's adapter reports them. Calls
/// block on the network, so they run off the UI thread.
pub trait ChannelAds: Send + Sync {
    /// The owner's Product Ads account; `None` when the channel has none
    /// for them, as before the Reputation unlocks Product Ads.
    fn account(&self) -> Result<Option<String>, PlatformError>;

    /// The account's campaigns.
    fn campaigns(&self, account: &str) -> Result<Vec<ChannelCampaign>, PlatformError>;

    /// Each ad's metrics on `day` of the channel; an ad that did nothing
    /// that day may be left out.
    fn ads_on(&self, account: &str, day: NaiveDate) -> Result<Vec<ChannelAd>, PlatformError>;
}

/// What a Sync of Product Ads read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdsSync {
    /// The channel has a Product Ads account for the owner.
    pub account: bool,
    /// Days read.
    pub days: usize,
    /// Ads with something on one of those days, day by day.
    pub ads: usize,
}

/// When Product Ads were last read, and whether the owner had an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdsSyncState {
    pub at: Timestamp,
    pub account: bool,
}

/// What Product Ads cost on one listing in one day of the channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdDay {
    /// The channel's id of the listing.
    pub listing: String,
    pub day: NaiveDate,
    pub cost: Money,
}

/// A listing of the owner's, as the caller knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertisedListing {
    /// The channel's id of the listing.
    pub listing: String,
    /// What groups listings into one row: the Product the listing sells,
    /// or the listing's own id when its variations sell several.
    pub product: String,
    pub name: String,
    /// `None` when the caller could not work it out, as for a Product
    /// never stocked.
    pub break_even: Option<BreakEven>,
}

/// A campaign's ads together in a period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignAds {
    /// The channel's id.
    pub id: String,
    /// `None` for a campaign the channel no longer lists.
    pub campaign: Option<ChannelCampaign>,
    pub metrics: AdMetrics,
}

/// A Product's ads together in a period, against the least ROAS that pays
/// for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductRoas {
    pub product: String,
    pub name: String,
    /// The channel's ids of its advertised listings.
    pub listings: Vec<String>,
    pub metrics: AdMetrics,
    /// The highest of its listings'; `None` when one is not known.
    pub break_even: Option<BreakEven>,
}

impl ProductRoas {
    pub fn roas(&self) -> Option<Roas> {
        self.metrics.roas()
    }

    /// Whether the ads lose money: what they spent brought in less than
    /// the break-even.
    pub fn loses(&self) -> bool {
        match (self.roas(), self.break_even) {
            (Some(roas), Some(break_even)) => break_even.loses_at(roas),
            _ => false,
        }
    }
}

/// Product Ads in a period, by campaign and by Product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdsReport {
    /// The most spent first.
    pub campaigns: Vec<CampaignAds>,
    /// Those losing money first, then the most spent.
    pub products: Vec<ProductRoas>,
    /// `None` without any ads in the period.
    pub total: Option<AdMetrics>,
}

#[derive(Debug, thiserror::Error)]
pub enum AdsError {
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error(transparent)]
    Currencies(#[from] CurrencyMismatch),
    #[error("the Product Ads hold a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for AdsError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => AdsError::Unreadable(text),
            StoredValueError::Sql(error) => AdsError::Sql(error),
        }
    }
}

/// The owner's Product Ads: what they cost and brought in, read from the
/// channel once a day.
pub struct ProductAds {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl ProductAds {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            database,
            clock,
            ids,
        }
    }

    /// Whether the last Sync is [`ADS_REFRESH_MINUTES`] old, or there was
    /// none.
    pub async fn is_due(&self) -> Result<bool, AdsError> {
        Ok(match self.last_sync().await? {
            Some(last) => self.clock.now() - last.at >= TimeDelta::minutes(ADS_REFRESH_MINUTES),
            None => true,
        })
    }

    /// When Product Ads were last read; `None` before the first Sync.
    pub async fn last_sync(&self) -> Result<Option<AdsSyncState>, AdsError> {
        Ok(self.sync_row().await?.map(|(state, _)| state))
    }

    /// Reads the campaigns and each ad's metrics day by day, up to
    /// yesterday on the channel: the [`ADS_HISTORY_DAYS`] before it the
    /// first time, then from [`ADS_SETTLE_DAYS`] before the last day read.
    /// Each day is kept by listing and day: reading it again changes
    /// nothing, and an ad the channel no longer reports for it is gone.
    /// Nothing is kept unless everything was read.
    pub async fn sync(&self, channel: &dyn ChannelAds) -> Result<AdsSync, AdsError> {
        let now = self.clock.now();
        let Some(account) = channel.account()? else {
            self.save_sync(None, None, now).await?;
            return Ok(AdsSync::default());
        };
        let campaigns = channel.campaigns(&account)?;
        let yesterday = channel_day(now) - TimeDelta::days(1);
        let earliest = yesterday - TimeDelta::days(ADS_HISTORY_DAYS - 1);
        let first = match self.sync_row().await?.and_then(|(_, through)| through) {
            Some(through) => (through - TimeDelta::days(ADS_SETTLE_DAYS - 1)).max(earliest),
            None => earliest,
        };
        let mut read = Vec::new();
        let mut day = first;
        while day <= yesterday {
            read.push((day, channel.ads_on(&account, day)?));
            day += TimeDelta::days(1);
        }

        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        self.write_campaigns(&transaction, &campaigns, now).await?;
        let mut report = AdsSync {
            account: true,
            days: read.len(),
            ads: 0,
        };
        for (day, ads) in &read {
            report.ads += self.write_day(&transaction, *day, ads, now).await?;
        }
        save_single_row(
            &transaction,
            self.clock.as_ref(),
            self.ids.as_ref(),
            SYNCS_TABLE,
            SYNCS_COLUMNS,
            sync_values(Some(&account), Some(yesterday), now),
        )
        .await?;
        transaction.commit().await?;
        Ok(report)
    }

    /// What Product Ads cost, by listing and day, every day read: what
    /// Commerce splits across the sales.
    pub async fn costs(&self) -> Result<Vec<AdDay>, AdsError> {
        Ok(self
            .days()
            .await?
            .into_iter()
            .map(|row| AdDay {
                listing: row.listing,
                day: row.day,
                cost: row.metrics.cost,
            })
            .collect())
    }

    /// The ads of the days that start `during`, by campaign and by
    /// Product: `listings` say which Product each listing sells and the
    /// least ROAS that pays for its ads. A listing the caller does not
    /// know is a row of its own.
    pub async fn report(
        &self,
        during: Range<Timestamp>,
        listings: &[AdvertisedListing],
    ) -> Result<AdsReport, AdsError> {
        let days: Vec<StoredDay> = self
            .days()
            .await?
            .into_iter()
            .filter(|row| during.contains(&channel_day_start(row.day)))
            .collect();
        let campaigns = self.campaigns().await?;
        let mut total: Option<AdMetrics> = None;
        let mut by_campaign: BTreeMap<String, AdMetrics> = BTreeMap::new();
        let mut by_product: BTreeMap<String, ProductRoas> = BTreeMap::new();
        for row in &days {
            add_to(&mut total, &row.metrics)?;
            if let Some(campaign) = &row.campaign {
                add_into(&mut by_campaign, campaign, &row.metrics)?;
            }
            let known = listings.iter().find(|known| known.listing == row.listing);
            let break_even = known.and_then(|known| known.break_even);
            let key = known.map_or(&row.listing, |known| &known.product);
            match by_product.get_mut(key) {
                Some(entry) => {
                    entry.metrics.add(&row.metrics)?;
                    if !entry.listings.contains(&row.listing) {
                        entry.listings.push(row.listing.clone());
                        entry.break_even = entry.break_even.zip(break_even).map(worst);
                    }
                }
                None => {
                    by_product.insert(
                        key.clone(),
                        ProductRoas {
                            product: key.clone(),
                            name: known.map_or_else(|| row.listing.clone(), |k| k.name.clone()),
                            listings: vec![row.listing.clone()],
                            metrics: row.metrics,
                            break_even,
                        },
                    );
                }
            }
        }
        let mut campaigns: Vec<CampaignAds> = by_campaign
            .into_iter()
            .map(|(id, metrics)| CampaignAds {
                campaign: campaigns.get(&id).cloned(),
                id,
                metrics,
            })
            .collect();
        campaigns.sort_by_key(|campaign| std::cmp::Reverse(campaign.metrics.cost.amount()));
        let mut products: Vec<ProductRoas> = by_product.into_values().collect();
        products.sort_by(|a, b| {
            b.loses()
                .cmp(&a.loses())
                .then(b.metrics.cost.amount().cmp(&a.metrics.cost.amount()))
        });
        Ok(AdsReport {
            campaigns,
            products,
            total,
        })
    }

    async fn sync_row(&self) -> Result<Option<(AdsSyncState, Option<NaiveDate>)>, AdsError> {
        let Some(row) =
            load_single_row(self.database.connection(), SYNCS_TABLE, SYNCS_COLUMNS).await?
        else {
            return Ok(None);
        };
        let account: Option<String> = row.get(0)?;
        let through = row
            .get::<Option<String>>(1)?
            .map(|text| read_day(&text))
            .transpose()?;
        Ok(Some((
            AdsSyncState {
                at: row.time_at(2)?,
                account: account.is_some(),
            },
            through,
        )))
    }

    async fn save_sync(
        &self,
        account: Option<&str>,
        through: Option<NaiveDate>,
        now: Timestamp,
    ) -> Result<(), AdsError> {
        save_single_row(
            self.database.connection(),
            self.clock.as_ref(),
            self.ids.as_ref(),
            SYNCS_TABLE,
            SYNCS_COLUMNS,
            sync_values(account, through, now),
        )
        .await?;
        Ok(())
    }

    /// Writes the channel's campaigns over the app's: each by its id, and
    /// those the channel no longer lists gone.
    async fn write_campaigns(
        &self,
        on: &Connection,
        campaigns: &[ChannelCampaign],
        now: Timestamp,
    ) -> Result<(), AdsError> {
        for campaign in campaigns {
            on.execute(
                "INSERT INTO marketing_ad_campaigns
                     (id, ml_campaign_id, name, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT (ml_campaign_id) WHERE deleted_at IS NULL
                 DO UPDATE SET name = excluded.name, status = excluded.status,
                     updated_at = excluded.updated_at",
                params![
                    self.ids.next_id().to_string(),
                    campaign.id.clone(),
                    campaign.name.clone(),
                    campaign.status.code(),
                    stored(now)
                ],
            )
            .await?;
        }
        on.execute(
            "UPDATE marketing_ad_campaigns SET deleted_at = ?1, updated_at = ?1
             WHERE deleted_at IS NULL AND updated_at < ?1",
            params![stored(now)],
        )
        .await?;
        Ok(())
    }

    /// Writes the channel's ads of `day` over the app's, and returns how
    /// many did something; an ad that did nothing is not kept.
    async fn write_day(
        &self,
        on: &Connection,
        day: NaiveDate,
        ads: &[ChannelAd],
        now: Timestamp,
    ) -> Result<usize, AdsError> {
        let day_text = day.to_string();
        let mut written = 0;
        for ad in ads.iter().filter(|ad| !ad.metrics.is_empty()) {
            let metrics = &ad.metrics;
            if metrics.attributed.currency() != metrics.cost.currency() {
                return Err(CurrencyMismatch {
                    left: metrics.cost.currency().code(),
                    right: metrics.attributed.currency().code(),
                }
                .into());
            }
            on.execute(
                "INSERT INTO marketing_ad_days
                     (id, ml_item_id, day, ml_campaign_id, cost, attributed, currency,
                      clicks, prints, units, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)
                 ON CONFLICT (ml_item_id, day) WHERE deleted_at IS NULL
                 DO UPDATE SET ml_campaign_id = excluded.ml_campaign_id,
                     cost = excluded.cost, attributed = excluded.attributed,
                     currency = excluded.currency, clicks = excluded.clicks,
                     prints = excluded.prints, units = excluded.units,
                     updated_at = excluded.updated_at",
                vec![
                    Value::Text(self.ids.next_id().to_string()),
                    Value::Text(ad.listing.clone()),
                    Value::Text(day_text.clone()),
                    ad.campaign.clone().map_or(Value::Null, Value::Text),
                    Value::Text(metrics.cost.amount().to_string()),
                    Value::Text(metrics.attributed.amount().to_string()),
                    Value::Text(metrics.cost.currency().code().into()),
                    Value::Integer(stored_count(metrics.clicks)?),
                    Value::Integer(stored_count(metrics.prints)?),
                    Value::Integer(stored_count(metrics.units)?),
                    Value::Text(stored(now)),
                ],
            )
            .await?;
            written += 1;
        }
        on.execute(
            "UPDATE marketing_ad_days SET deleted_at = ?1, updated_at = ?1
             WHERE day = ?2 AND deleted_at IS NULL AND updated_at < ?1",
            params![stored(now), day_text],
        )
        .await?;
        Ok(written)
    }

    async fn days(&self) -> Result<Vec<StoredDay>, AdsError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT ml_item_id, day, ml_campaign_id, cost, attributed, currency,
                     clicks, prints, units
                 FROM marketing_ad_days WHERE deleted_at IS NULL
                 ORDER BY day, ml_item_id",
                (),
            )
            .await?;
        let mut days = Vec::new();
        while let Some(row) = rows.next().await? {
            let currency = row.currency_at(5)?;
            days.push(StoredDay {
                listing: row.get(0)?,
                day: read_day(&row.get::<String>(1)?)?,
                campaign: row.get(2)?,
                metrics: AdMetrics {
                    cost: Money::new(row.decimal_at(3)?, currency),
                    attributed: Money::new(row.decimal_at(4)?, currency),
                    clicks: read_count(row.get(6)?)?,
                    prints: read_count(row.get(7)?)?,
                    units: read_count(row.get(8)?)?,
                },
            });
        }
        Ok(days)
    }

    async fn campaigns(&self) -> Result<BTreeMap<String, ChannelCampaign>, AdsError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT ml_campaign_id, name, status FROM marketing_ad_campaigns
                 WHERE deleted_at IS NULL",
                (),
            )
            .await?;
        let mut campaigns = BTreeMap::new();
        while let Some(row) = rows.next().await? {
            let id: String = row.get(0)?;
            let status: String = row.get(2)?;
            campaigns.insert(
                id.clone(),
                ChannelCampaign {
                    id,
                    name: row.get(1)?,
                    status: CampaignStatus::from_code(&status)
                        .ok_or(AdsError::Unreadable(status))?,
                },
            );
        }
        Ok(campaigns)
    }
}

/// One ad's day as kept.
struct StoredDay {
    listing: String,
    day: NaiveDate,
    campaign: Option<String>,
    metrics: AdMetrics,
}

/// The break-even that asks more of the ads: none pays at all, or the
/// higher ROAS.
fn worst((a, b): (BreakEven, BreakEven)) -> BreakEven {
    match (a, b) {
        (BreakEven::At(a), BreakEven::At(b)) => BreakEven::At(a.max(b)),
        _ => BreakEven::Never,
    }
}

fn add_to(total: &mut Option<AdMetrics>, metrics: &AdMetrics) -> Result<(), CurrencyMismatch> {
    match total {
        Some(total) => total.add(metrics),
        None => {
            *total = Some(*metrics);
            Ok(())
        }
    }
}

fn add_into(
    by: &mut BTreeMap<String, AdMetrics>,
    key: &str,
    metrics: &AdMetrics,
) -> Result<(), CurrencyMismatch> {
    match by.get_mut(key) {
        Some(kept) => kept.add(metrics),
        None => {
            by.insert(key.to_owned(), *metrics);
            Ok(())
        }
    }
}

fn sync_values(account: Option<&str>, through: Option<NaiveDate>, now: Timestamp) -> Vec<Value> {
    vec![
        account.map_or(Value::Null, |account| Value::Text(account.to_owned())),
        through.map_or(Value::Null, |day| Value::Text(day.to_string())),
        Value::Text(stored(now)),
    ]
}

fn read_day(text: &str) -> Result<NaiveDate, AdsError> {
    text.parse()
        .map_err(|_| AdsError::Unreadable(text.to_owned()))
}

fn stored_count(count: u64) -> Result<i64, AdsError> {
    i64::try_from(count).map_err(|_| AdsError::Unreadable(count.to_string()))
}

fn read_count(count: i64) -> Result<u64, AdsError> {
    u64::try_from(count).map_err(|_| AdsError::Unreadable(count.to_string()))
}
