//! Clicks on Short Links (#159, ADR 0035 decision 8): read from the
//! shortener day by day in the Commissions Sync and kept by Short Link and
//! day, so reading a window again gives the same state and a day whose count
//! grew is updated. The days are Brasília's, as the Platforms count theirs.
//! The commissions dashboard sets them beside the Commissions each link
//! brought: a link without a Short Link has no clicks, and stays out of
//! clicks per Commission.

use std::collections::{BTreeMap, HashMap};
use std::ops::{Range, RangeInclusive};

use chrono::{NaiveDate, TimeDelta};
use libsql::{TransactionBehavior, Value, params};
use mascate_kernel::{PlatformError, RecordId, Timestamp, channel_day, channel_day_start};
use mascate_platform::{Migration, StoredRow, load_single_row, save_single_row, stored};
use rust_decimal::Decimal;

use crate::commissions::{Affiliate, AffiliateError, Commission};
use crate::short_links::LinkShortener;
use crate::source::CommissionStatus;

pub(crate) const CREATE_LINK_CLICKS: Migration = Migration {
    version: 7,
    name: "create the daily clicks of Short Links",
    risky: false,
    sql: "CREATE TABLE affiliate_link_clicks (
        id            TEXT    PRIMARY KEY,
        short_link_id TEXT    NOT NULL REFERENCES affiliate_short_links (id),
        day           TEXT    NOT NULL,
        clicks        INTEGER NOT NULL,
        created_at    TEXT    NOT NULL,
        updated_at    TEXT    NOT NULL,
        deleted_at    TEXT
    );
    CREATE UNIQUE INDEX affiliate_link_clicks_by_day
        ON affiliate_link_clicks (short_link_id, day)
        WHERE deleted_at IS NULL;
    CREATE TABLE affiliate_click_reads (
        id         TEXT PRIMARY KEY,
        read_at    TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );",
};

const READS_TABLE: &str = "affiliate_click_reads";
const READS_COLUMNS: &[&str] = &["read_at"];

/// Days read again before the last one kept: the shortener may still count
/// a click late, after filtering bots. Not in the documentation consulted;
/// to be checked with the real account (#116).
pub const CLICKS_SETTLE_DAYS: i64 = 3;

/// A Short Link's clicks on one day of Brasília.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayClicks {
    pub day: NaiveDate,
    pub clicks: u64,
}

/// What a read of the clicks did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClicksRead {
    /// The Short Links read.
    pub links: usize,
    /// The days new or with another count.
    pub changed: usize,
    /// The Short Links the shortener no longer knows, whose clicks stay as
    /// they were.
    pub gone: usize,
}

/// One Short Link as the read needs it.
struct ToRead {
    id: RecordId,
    key: String,
    first_day: NaiveDate,
    last_read: Option<NaiveDate>,
}

/// An Affiliate Link with a Short Link, and its clicks in a period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkClicks {
    pub link: RecordId,
    pub channel: RecordId,
    /// When the link got its Short Link: a sale before it came through the
    /// whole link, whose clicks nobody counts.
    pub shortened_at: Timestamp,
    pub clicks: u64,
}

/// Clicks and the Commissions they brought, for one link or one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClickGroup<K> {
    pub of: K,
    pub clicks: u64,
    /// The Commissions sold through a Short Link of the group, cancelled
    /// sales left out, as on the rest of the dashboard.
    pub commissions: usize,
}

impl<K> ClickGroup<K> {
    /// How many clicks each Commission took; `None` without a Commission.
    pub fn clicks_per_commission(&self) -> Option<Decimal> {
        (self.commissions > 0)
            .then(|| Decimal::from(self.clicks) / Decimal::from(self.commissions as u64))
    }
}

/// The period's clicks by Affiliate Link and by Promotion Channel, the most
/// clicked first, ties by id. Only links with a Short Link are in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClickReport {
    pub by_link: Vec<ClickGroup<RecordId>>,
    pub by_channel: Vec<ClickGroup<RecordId>>,
}

impl ClickReport {
    /// Sets `clicks` beside the `commissions` of the same period that came
    /// through each link once it had its Short Link.
    pub fn of(clicks: &[LinkClicks], commissions: &[Commission]) -> Self {
        let shortened: HashMap<RecordId, Timestamp> = clicks
            .iter()
            .map(|link| (link.link, link.shortened_at))
            .collect();
        let mut sold: HashMap<RecordId, usize> = HashMap::new();
        for commission in commissions {
            let Some(link) = commission.link else {
                continue;
            };
            if commission.status != CommissionStatus::Cancelled
                && shortened
                    .get(&link)
                    .is_some_and(|at| commission.sold_at >= *at)
            {
                *sold.entry(link).or_default() += 1;
            }
        }
        let mut by_link = Vec::new();
        let mut by_channel: BTreeMap<RecordId, ClickGroup<RecordId>> = BTreeMap::new();
        for link in clicks {
            let commissions = sold.get(&link.link).copied().unwrap_or_default();
            by_link.push(ClickGroup {
                of: link.link,
                clicks: link.clicks,
                commissions,
            });
            let channel = by_channel.entry(link.channel).or_insert(ClickGroup {
                of: link.channel,
                clicks: 0,
                commissions: 0,
            });
            channel.clicks += link.clicks;
            channel.commissions += commissions;
        }
        let mut by_channel: Vec<_> = by_channel.into_values().collect();
        most_clicked_first(&mut by_link);
        most_clicked_first(&mut by_channel);
        Self {
            by_link,
            by_channel,
        }
    }
}

fn most_clicked_first(groups: &mut [ClickGroup<RecordId>]) {
    groups.sort_by(|a, b| b.clicks.cmp(&a.clicks).then(a.of.cmp(&b.of)));
}

impl Affiliate {
    /// Reads each live Short Link's clicks through `shortener`, up to today
    /// in Brasília: from the day it was created the first time, then from
    /// [`CLICKS_SETTLE_DAYS`] before the last day kept. Each day is kept by
    /// Short Link and day, the shortener's count over the app's. A Short
    /// Link the shortener no longer knows is skipped and keeps its clicks;
    /// any other failure keeps nothing, and the last read stays as it was.
    pub async fn read_clicks(
        &self,
        shortener: &dyn LinkShortener,
    ) -> Result<ClicksRead, AffiliateError> {
        let now = self.clock.now();
        let today = channel_day(now);
        let mut read = ClicksRead::default();
        let mut days = Vec::new();
        for link in self.short_links_to_read().await? {
            let from = link.last_read.map_or(link.first_day, |last| {
                (last - TimeDelta::days(CLICKS_SETTLE_DAYS)).max(link.first_day)
            });
            if from > today {
                continue;
            }
            let asked = from..=today;
            match shortener.clicks(&link.key, asked.clone()) {
                Ok(counted) => {
                    read.links += 1;
                    days.extend(
                        last_count_of_each_day(counted, &asked)
                            .into_iter()
                            .map(|day| (link.id, day)),
                    );
                }
                Err(PlatformError::NotFound) => read.gone += 1,
                Err(error) => return Err(error.into()),
            }
        }

        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        for (link, day) in days {
            read.changed += transaction
                .execute(
                    "INSERT INTO affiliate_link_clicks
                         (id, short_link_id, day, clicks, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                     ON CONFLICT (short_link_id, day) WHERE deleted_at IS NULL
                     DO UPDATE SET clicks = excluded.clicks, updated_at = excluded.updated_at
                     WHERE clicks <> excluded.clicks",
                    params![
                        self.ids.next_id().to_string(),
                        link.to_string(),
                        day.day.to_string(),
                        i64::try_from(day.clicks).unwrap_or(i64::MAX),
                        stored(now)
                    ],
                )
                .await? as usize;
        }
        save_single_row(
            &transaction,
            self.clock.as_ref(),
            self.ids.as_ref(),
            READS_TABLE,
            READS_COLUMNS,
            vec![Value::Text(stored(now))],
        )
        .await?;
        transaction.commit().await?;
        Ok(read)
    }

    /// When the clicks were last read through; `None` before the first.
    pub async fn clicks_read_at(&self) -> Result<Option<Timestamp>, AffiliateError> {
        match load_single_row(self.database.connection(), READS_TABLE, READS_COLUMNS).await? {
            Some(row) => Ok(Some(row.time_at(0)?)),
            None => Ok(None),
        }
    }

    /// Each live Affiliate Link with a Short Link, with its clicks on the
    /// days that start `during`; one without a click there counts 0.
    pub async fn link_clicks(
        &self,
        during: Range<Timestamp>,
    ) -> Result<Vec<LinkClicks>, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT l.id, l.channel_id, s.created_at, c.day, c.clicks
                 FROM affiliate_short_links s
                 JOIN affiliate_links l ON l.id = s.link_id AND l.deleted_at IS NULL
                 LEFT JOIN affiliate_link_clicks c
                     ON c.short_link_id = s.id AND c.deleted_at IS NULL
                 WHERE s.deleted_at IS NULL
                 ORDER BY l.id",
                (),
            )
            .await?;
        let mut links: Vec<LinkClicks> = Vec::new();
        while let Some(row) = rows.next().await? {
            let id = row.id_at(0)?;
            if links.last().is_none_or(|last| last.link != id) {
                links.push(LinkClicks {
                    link: id,
                    channel: row.id_at(1)?,
                    shortened_at: row.time_at(2)?,
                    clicks: 0,
                });
            }
            let Some(day) = row.get::<Option<String>>(3)? else {
                continue;
            };
            if during.contains(&channel_day_start(read_day(&day)?)) {
                let clicks = u64::try_from(row.get::<i64>(4)?).unwrap_or_default();
                if let Some(link) = links.last_mut() {
                    link.clicks += clicks;
                }
            }
        }
        Ok(links)
    }

    async fn short_links_to_read(&self) -> Result<Vec<ToRead>, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT s.id, s.shortener_key, s.created_at,
                     (SELECT max(c.day) FROM affiliate_link_clicks c
                      WHERE c.short_link_id = s.id AND c.deleted_at IS NULL)
                 FROM affiliate_short_links s
                 JOIN affiliate_links l ON l.id = s.link_id AND l.deleted_at IS NULL
                 WHERE s.deleted_at IS NULL
                 ORDER BY s.id",
                (),
            )
            .await?;
        let mut links = Vec::new();
        while let Some(row) = rows.next().await? {
            links.push(ToRead {
                id: row.id_at(0)?,
                key: row.get(1)?,
                first_day: channel_day(row.time_at(2)?),
                last_read: row
                    .get::<Option<String>>(3)?
                    .map(|day| read_day(&day))
                    .transpose()?,
            });
        }
        Ok(links)
    }
}

/// The days of `asked` in `counted`, a day given twice as its last count.
fn last_count_of_each_day(
    counted: Vec<DayClicks>,
    asked: &RangeInclusive<NaiveDate>,
) -> Vec<DayClicks> {
    let mut by_day = BTreeMap::new();
    for day in counted {
        if asked.contains(&day.day) {
            by_day.insert(day.day, day);
        }
    }
    by_day.into_values().collect()
}

fn read_day(text: &str) -> Result<NaiveDate, AffiliateError> {
    text.parse()
        .map_err(|_| AffiliateError::Unreadable(text.to_owned()))
}
