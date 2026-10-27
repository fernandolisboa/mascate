//! Promotion Spends (#195, ADR 0037 decision 6): what the owner spent to
//! promote on a Promotion Channel in some days, typed or read from the
//! report the Meta Ads Manager or Google Ads exports. The app records it and
//! never spends or pauses anything. A report row is one campaign on one day,
//! known by its report, campaign and day: importing it again replaces that
//! day's amount, because the Platform corrects spend later. Each report
//! campaign is linked to a Promotion Channel once, on its first import.

use std::collections::{BTreeMap, HashMap};
use std::ops::RangeInclusive;
use std::sync::Arc;

use chrono::NaiveDate;
use libsql::{Connection, TransactionBehavior, params};
use mascate_kernel::{Clock, IdGenerator, Money, Record, RecordId, Timestamp};
use mascate_platform::{Database, Migration, StoredRow, StoredValueError, stored};
use rust_decimal::Decimal;

pub(crate) const CREATE_PROMOTION_SPENDS: Migration = Migration {
    version: 16,
    name: "create the Promotion Spends and the report campaigns' channels",
    risky: false,
    sql: "CREATE TABLE marketing_promotion_spends (
        id         TEXT PRIMARY KEY,
        channel_id TEXT NOT NULL,
        first_day  TEXT NOT NULL,
        last_day   TEXT NOT NULL,
        amount     TEXT NOT NULL,
        currency   TEXT NOT NULL,
        origin     TEXT NOT NULL,
        campaign   TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE UNIQUE INDEX marketing_promotion_spends_by_report_day
        ON marketing_promotion_spends (origin, campaign, first_day)
        WHERE deleted_at IS NULL;
    CREATE TABLE marketing_report_campaigns (
        id         TEXT PRIMARY KEY,
        report     TEXT NOT NULL,
        campaign   TEXT NOT NULL,
        channel_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE UNIQUE INDEX marketing_report_campaigns_by_campaign
        ON marketing_report_campaigns (report, campaign)
        WHERE deleted_at IS NULL;",
};

/// The report a spend was read from: what the Platform of the ad exports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SpendReport {
    MetaAds,
    GoogleAds,
}

impl SpendReport {
    pub const ALL: [SpendReport; 2] = [SpendReport::MetaAds, SpendReport::GoogleAds];

    fn code(self) -> &'static str {
        match self {
            SpendReport::MetaAds => "meta_ads",
            SpendReport::GoogleAds => "google_ads",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|report| report.code() == code)
    }

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            SpendReport::MetaAds => "Gerenciador de Anúncios da Meta",
            SpendReport::GoogleAds => "Google Ads",
        }
    }
}

/// Where a Promotion Spend came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpendOrigin {
    /// The owner typed it: edited and deleted on the screen.
    Typed,
    /// Read from a report: changed only by importing it again.
    Report(SpendReport),
}

const TYPED: &str = "typed";

impl SpendOrigin {
    fn code(self) -> &'static str {
        match self {
            SpendOrigin::Typed => TYPED,
            SpendOrigin::Report(report) => report.code(),
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        match code {
            TYPED => Some(SpendOrigin::Typed),
            code => SpendReport::from_code(code).map(SpendOrigin::Report),
        }
    }
}

/// Money the owner spent to promote on a Promotion Channel from
/// `first_day` to `last_day`, both on Brasília's calendar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionSpend {
    pub id: RecordId,
    pub channel: RecordId,
    pub first_day: NaiveDate,
    pub last_day: NaiveDate,
    pub amount: Money,
    pub origin: SpendOrigin,
    /// The report's campaign; only on a spend read from a report.
    pub campaign: Option<String>,
}

impl PromotionSpend {
    /// How many days it covers, both ends included.
    pub fn days(&self) -> i64 {
        days_in(self.first_day, self.last_day)
    }

    /// Its share of `days`: the amount spread evenly over its own days,
    /// never rounded.
    fn share_of(&self, days: &RangeInclusive<NaiveDate>) -> Option<Money> {
        let first = self.first_day.max(*days.start());
        let last = self.last_day.min(*days.end());
        if first > last {
            return None;
        }
        let shared = days_in(first, last);
        Some(if shared == self.days() {
            self.amount
        } else {
            // Multiplied first, so a third of a third stays as exact as it can.
            Money::new(
                self.amount.amount() * Decimal::from(shared) / Decimal::from(self.days()),
                self.amount.currency(),
            )
        })
    }
}

fn days_in(first: NaiveDate, last: NaiveDate) -> i64 {
    (last - first).num_days() + 1
}

/// A spend as the owner types it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPromotionSpend {
    pub channel: RecordId,
    pub first_day: NaiveDate,
    pub last_day: NaiveDate,
    pub amount: Money,
}

/// A typed spend as kept, with the other spends of its channel whose days
/// cross its own: they may count the same money twice, so the screen warns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedSpend {
    pub spend: PromotionSpend,
    pub crossing: Vec<PromotionSpend>,
}

/// One campaign's spend on one day, as a report has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportedSpend {
    pub report: SpendReport,
    pub campaign: String,
    pub day: NaiveDate,
    pub amount: Money,
}

/// A report campaign and the Promotion Channel its spend belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignLink {
    pub report: SpendReport,
    pub campaign: String,
    pub channel: RecordId,
}

/// What importing a report records, by campaign and day.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportedSpends {
    /// Days no import had brought.
    pub added: usize,
    /// Days whose amount the report corrected.
    pub changed: usize,
    /// Days already in the app as the report has them.
    pub unchanged: usize,
}

/// What a channel spent in some days, in one currency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSpend {
    pub channel: RecordId,
    pub amount: Money,
}

#[derive(Debug, thiserror::Error)]
pub enum SpendError {
    #[error("the last day comes before the first")]
    BadPeriod,
    #[error("a spend is an amount above zero")]
    BadAmount,
    #[error("a report campaign needs its name")]
    BadCampaign,
    #[error("no such Promotion Channel")]
    NoSuchChannel,
    #[error("no such Promotion Spend")]
    NotFound,
    #[error("a spend read from a report changes only by importing it again")]
    FromReport,
    #[error("the campaign {0} is not linked to a Promotion Channel")]
    UnlinkedCampaign(String),
    #[error("the spends hold a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for SpendError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => SpendError::Unreadable(text),
            StoredValueError::Sql(error) => SpendError::Sql(error),
        }
    }
}

const SPEND_QUERY: &str = "SELECT id, channel_id, first_day, last_day, amount, currency, origin,
         campaign
     FROM marketing_promotion_spends WHERE deleted_at IS NULL";

/// The owner's Promotion Spends.
pub struct PromotionSpends {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl PromotionSpends {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            database,
            clock,
            ids,
        }
    }

    /// The spends of `channel`, the latest days first.
    pub async fn spends(&self, channel: RecordId) -> Result<Vec<PromotionSpend>, SpendError> {
        read_spends(
            self.database.connection(),
            &format!(
                "{SPEND_QUERY} AND channel_id = ?1
                 ORDER BY last_day DESC, first_day DESC, campaign, created_at"
            ),
            params![channel.to_string()],
        )
        .await
    }

    pub async fn spend(&self, id: RecordId) -> Result<PromotionSpend, SpendError> {
        read_spends(
            self.database.connection(),
            &format!("{SPEND_QUERY} AND id = ?1"),
            params![id.to_string()],
        )
        .await?
        .pop()
        .ok_or(SpendError::NotFound)
    }

    /// What each live channel spent in `days`, one entry per currency it
    /// spent in. A spend crossing the edge counts its share of the days
    /// inside, spread evenly; nothing is rounded.
    pub async fn spent(
        &self,
        days: RangeInclusive<NaiveDate>,
    ) -> Result<Vec<ChannelSpend>, SpendError> {
        let spends = read_spends(
            self.database.connection(),
            "SELECT s.id, s.channel_id, s.first_day, s.last_day, s.amount, s.currency, s.origin,
                    s.campaign
             FROM marketing_promotion_spends s
             JOIN marketing_promotion_channels c ON c.id = s.channel_id AND c.deleted_at IS NULL
             WHERE s.deleted_at IS NULL AND s.first_day <= ?2 AND s.last_day >= ?1",
            params![days.start().to_string(), days.end().to_string()],
        )
        .await?;
        let mut sums: BTreeMap<(RecordId, &'static str), Money> = BTreeMap::new();
        for spend in &spends {
            let Some(share) = spend.share_of(&days) else {
                continue;
            };
            let key = (spend.channel, share.currency().code());
            let sum = sums.entry(key).or_insert(Money::zero(share.currency()));
            *sum = sum
                .checked_add(share)
                .expect("the same currency, by the key");
        }
        Ok(sums
            .into_iter()
            .map(|((channel, _), amount)| ChannelSpend { channel, amount })
            .collect())
    }

    /// Keeps a spend the owner typed.
    pub async fn record(&self, spend: &NewPromotionSpend) -> Result<SavedSpend, SpendError> {
        let spend = checked(spend)?;
        self.ensure_channel(spend.channel).await?;
        let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
        self.database
            .connection()
            .execute(
                "INSERT INTO marketing_promotion_spends
                     (id, channel_id, first_day, last_day, amount, currency, origin, created_at,
                      updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    record.id.to_string(),
                    spend.channel.to_string(),
                    spend.first_day.to_string(),
                    spend.last_day.to_string(),
                    spend.amount.amount().to_string(),
                    spend.amount.currency().code(),
                    SpendOrigin::Typed.code(),
                    stored(record.created_at)
                ],
            )
            .await?;
        self.saved(record.id).await
    }

    /// Changes a typed spend; a spend read from a report changes only by
    /// importing it again.
    pub async fn update(
        &self,
        id: RecordId,
        spend: &NewPromotionSpend,
    ) -> Result<SavedSpend, SpendError> {
        let spend = checked(spend)?;
        self.ensure_typed(id).await?;
        self.ensure_channel(spend.channel).await?;
        self.database
            .connection()
            .execute(
                "UPDATE marketing_promotion_spends
                 SET channel_id = ?1, first_day = ?2, last_day = ?3, amount = ?4, currency = ?5,
                     updated_at = ?6
                 WHERE id = ?7 AND deleted_at IS NULL",
                params![
                    spend.channel.to_string(),
                    spend.first_day.to_string(),
                    spend.last_day.to_string(),
                    spend.amount.amount().to_string(),
                    spend.amount.currency().code(),
                    stored(self.clock.now()),
                    id.to_string()
                ],
            )
            .await?;
        self.saved(id).await
    }

    /// Deletes a typed spend.
    pub async fn delete(&self, id: RecordId) -> Result<(), SpendError> {
        self.ensure_typed(id).await?;
        self.database
            .connection()
            .execute(
                "UPDATE marketing_promotion_spends SET deleted_at = ?1, updated_at = ?1
                 WHERE id = ?2 AND deleted_at IS NULL",
                params![stored(self.clock.now()), id.to_string()],
            )
            .await?;
        Ok(())
    }

    /// The campaigns of `spends` with no live Promotion Channel yet, each
    /// once, in the order they first appear: the screen asks which channel
    /// each one is before importing.
    pub async fn unlinked(
        &self,
        spends: &[ReportedSpend],
    ) -> Result<Vec<(SpendReport, String)>, SpendError> {
        let links = campaign_links(self.database.connection()).await?;
        let mut unlinked: Vec<(SpendReport, String)> = Vec::new();
        for spend in spends {
            let key = (spend.report, spend.campaign.trim().to_owned());
            if !links.contains_key(&key) && !unlinked.contains(&key) {
                unlinked.push(key);
            }
        }
        Ok(unlinked)
    }

    /// What importing `spends`, with the campaigns linked in `links`, would
    /// record, without recording anything.
    pub async fn preview_import(
        &self,
        spends: &[ReportedSpend],
        links: &[CampaignLink],
    ) -> Result<ImportedSpends, SpendError> {
        let connection = self.database.connection();
        let linked = linked_with(connection, links).await?;
        Ok(planned(connection, spends, &linked).await?.0)
    }

    /// Records `spends`, linking the campaigns in `links` first, in one
    /// transaction: each campaign's day is added, or replaced by the
    /// report's amount, so importing the same report again changes nothing.
    /// A campaign with no channel refuses the whole import.
    pub async fn import(
        &self,
        spends: &[ReportedSpend],
        links: &[CampaignLink],
    ) -> Result<ImportedSpends, SpendError> {
        let now = self.clock.now();
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        for link in links {
            self.link(&transaction, link, now).await?;
        }
        let linked = campaign_links(&transaction).await?;
        let (imported, plan) = planned(&transaction, spends, &linked).await?;
        for step in plan {
            match step {
                Step::Add(spend, channel) => {
                    let id = self.ids.next_id();
                    transaction
                        .execute(
                            "INSERT INTO marketing_promotion_spends
                                 (id, channel_id, first_day, last_day, amount, currency, origin,
                                  campaign, created_at, updated_at)
                             VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                            params![
                                id.to_string(),
                                channel.to_string(),
                                spend.day.to_string(),
                                spend.amount.amount().to_string(),
                                spend.amount.currency().code(),
                                spend.report.code(),
                                spend.campaign,
                                stored(now)
                            ],
                        )
                        .await?;
                }
                Step::Change(id, spend, channel) => {
                    transaction
                        .execute(
                            "UPDATE marketing_promotion_spends
                             SET channel_id = ?1, amount = ?2, currency = ?3, updated_at = ?4
                             WHERE id = ?5",
                            params![
                                channel.to_string(),
                                spend.amount.amount().to_string(),
                                spend.amount.currency().code(),
                                stored(now),
                                id.to_string()
                            ],
                        )
                        .await?;
                }
            }
        }
        transaction.commit().await?;
        Ok(imported)
    }

    /// Links a campaign to its channel, in place of a link to a channel
    /// deleted since.
    async fn link(
        &self,
        on: &Connection,
        link: &CampaignLink,
        now: Timestamp,
    ) -> Result<(), SpendError> {
        let campaign = link.campaign.trim();
        if campaign.is_empty() {
            return Err(SpendError::BadCampaign);
        }
        if !channel_is_live(on, link.channel).await? {
            return Err(SpendError::NoSuchChannel);
        }
        on.execute(
            "UPDATE marketing_report_campaigns SET deleted_at = ?1, updated_at = ?1
             WHERE report = ?2 AND campaign = ?3 AND deleted_at IS NULL",
            params![stored(now), link.report.code(), campaign],
        )
        .await?;
        on.execute(
            "INSERT INTO marketing_report_campaigns
                 (id, report, campaign, channel_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![
                self.ids.next_id().to_string(),
                link.report.code(),
                campaign,
                link.channel.to_string(),
                stored(now)
            ],
        )
        .await?;
        Ok(())
    }

    async fn saved(&self, id: RecordId) -> Result<SavedSpend, SpendError> {
        let spend = self.spend(id).await?;
        let crossing = read_spends(
            self.database.connection(),
            &format!(
                "{SPEND_QUERY} AND channel_id = ?1 AND id <> ?2 AND first_day <= ?4
                     AND last_day >= ?3
                 ORDER BY first_day, campaign, created_at"
            ),
            params![
                spend.channel.to_string(),
                id.to_string(),
                spend.first_day.to_string(),
                spend.last_day.to_string()
            ],
        )
        .await?;
        Ok(SavedSpend { spend, crossing })
    }

    async fn ensure_channel(&self, channel: RecordId) -> Result<(), SpendError> {
        if channel_is_live(self.database.connection(), channel).await? {
            Ok(())
        } else {
            Err(SpendError::NoSuchChannel)
        }
    }

    async fn ensure_typed(&self, id: RecordId) -> Result<(), SpendError> {
        match self.spend(id).await?.origin {
            SpendOrigin::Typed => Ok(()),
            SpendOrigin::Report(_) => Err(SpendError::FromReport),
        }
    }
}

fn checked(spend: &NewPromotionSpend) -> Result<NewPromotionSpend, SpendError> {
    if spend.last_day < spend.first_day {
        return Err(SpendError::BadPeriod);
    }
    if spend.amount.amount() <= Decimal::ZERO {
        return Err(SpendError::BadAmount);
    }
    Ok(NewPromotionSpend {
        amount: Money::new(spend.amount.amount().normalize(), spend.amount.currency()),
        ..spend.clone()
    })
}

/// What importing does to one campaign's day.
enum Step {
    Add(ReportedSpend, RecordId),
    Change(RecordId, ReportedSpend, RecordId),
}

/// Each spend's step against what earlier imports recorded, and the
/// counts. A day repeated in `spends` counts once, as its last amount.
async fn planned(
    on: &Connection,
    spends: &[ReportedSpend],
    linked: &HashMap<(SpendReport, String), RecordId>,
) -> Result<(ImportedSpends, Vec<Step>), SpendError> {
    let mut last: BTreeMap<(SpendReport, String, NaiveDate), ReportedSpend> = BTreeMap::new();
    for spend in spends {
        let campaign = spend.campaign.trim().to_owned();
        if campaign.is_empty() {
            return Err(SpendError::BadCampaign);
        }
        if spend.amount.is_negative() {
            return Err(SpendError::BadAmount);
        }
        last.insert(
            (spend.report, campaign.clone(), spend.day),
            ReportedSpend {
                campaign,
                amount: Money::new(spend.amount.amount().normalize(), spend.amount.currency()),
                ..spend.clone()
            },
        );
    }
    let mut imported = ImportedSpends::default();
    let mut plan = Vec::new();
    for spend in last.into_values() {
        let channel = *linked
            .get(&(spend.report, spend.campaign.clone()))
            .ok_or_else(|| SpendError::UnlinkedCampaign(spend.campaign.clone()))?;
        let kept = read_spends(
            on,
            &format!("{SPEND_QUERY} AND origin = ?1 AND campaign = ?2 AND first_day = ?3"),
            params![
                spend.report.code(),
                spend.campaign.clone(),
                spend.day.to_string()
            ],
        )
        .await?
        .pop();
        match kept {
            None => {
                imported.added += 1;
                plan.push(Step::Add(spend, channel));
            }
            Some(kept) if kept.amount == spend.amount && kept.channel == channel => {
                imported.unchanged += 1;
            }
            Some(kept) => {
                imported.changed += 1;
                plan.push(Step::Change(kept.id, spend, channel));
            }
        }
    }
    Ok((imported, plan))
}

/// The live link of each report campaign to a live channel.
async fn campaign_links(
    on: &Connection,
) -> Result<HashMap<(SpendReport, String), RecordId>, SpendError> {
    let mut rows = on
        .query(
            "SELECT l.report, l.campaign, l.channel_id
             FROM marketing_report_campaigns l
             JOIN marketing_promotion_channels c ON c.id = l.channel_id AND c.deleted_at IS NULL
             WHERE l.deleted_at IS NULL",
            (),
        )
        .await?;
    let mut links = HashMap::new();
    while let Some(row) = rows.next().await? {
        let report: String = row.get(0)?;
        let report = SpendReport::from_code(&report).ok_or(SpendError::Unreadable(report))?;
        links.insert((report, row.get(1)?), row.id_at(2)?);
    }
    Ok(links)
}

/// The kept links with `links` over them, as importing would see them.
async fn linked_with(
    on: &Connection,
    links: &[CampaignLink],
) -> Result<HashMap<(SpendReport, String), RecordId>, SpendError> {
    let mut linked = campaign_links(on).await?;
    for link in links {
        if !channel_is_live(on, link.channel).await? {
            return Err(SpendError::NoSuchChannel);
        }
        linked.insert((link.report, link.campaign.trim().to_owned()), link.channel);
    }
    Ok(linked)
}

async fn channel_is_live(on: &Connection, channel: RecordId) -> Result<bool, SpendError> {
    let mut rows = on
        .query(
            "SELECT 1 FROM marketing_promotion_channels WHERE id = ?1 AND deleted_at IS NULL",
            params![channel.to_string()],
        )
        .await?;
    Ok(rows.next().await?.is_some())
}

async fn read_spends(
    on: &Connection,
    query: &str,
    params: impl libsql::params::IntoParams,
) -> Result<Vec<PromotionSpend>, SpendError> {
    let mut rows = on.query(query, params).await?;
    let mut spends = Vec::new();
    while let Some(row) = rows.next().await? {
        spends.push(spend_from(&row)?);
    }
    Ok(spends)
}

fn spend_from(row: &libsql::Row) -> Result<PromotionSpend, SpendError> {
    let day = |at: i32| -> Result<NaiveDate, SpendError> {
        let text: String = row.get(at)?;
        text.parse().map_err(|_| SpendError::Unreadable(text))
    };
    let origin: String = row.get(6)?;
    Ok(PromotionSpend {
        id: row.id_at(0)?,
        channel: row.id_at(1)?,
        first_day: day(2)?,
        last_day: day(3)?,
        amount: Money::new(row.decimal_at(4)?, row.currency_at(5)?),
        origin: SpendOrigin::from_code(&origin).ok_or(SpendError::Unreadable(origin))?,
        campaign: row.get(7)?,
    })
}
