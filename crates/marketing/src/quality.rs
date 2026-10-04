//! Listing Quality (#23): how the Sales Channel rates each of the owner's
//! listings and what it says is missing, read in each Sync through a port a
//! Platform's adapter implements (ADR 0013), and the listings to work on
//! first, by sales and visits. The listings and their sales come from the
//! caller (ADR 0021): Marketing never reads Commerce. The panel only points
//! and links; any change reaches the channel from the owner's own hands.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use chrono::TimeDelta;
use libsql::{Connection, TransactionBehavior, params};
use mascate_kernel::{Clock, IdGenerator, PlatformError, RecordId, Timestamp};
use mascate_platform::{Database, Migration, StoredRow, StoredValueError, stored};

pub(crate) const CREATE_LISTING_QUALITY: Migration = Migration {
    version: 1,
    name: "create the Listing Quality",
    risky: false,
    sql: "CREATE TABLE marketing_listing_quality (
        id          TEXT PRIMARY KEY,
        ml_item_id  TEXT    NOT NULL,
        score       INTEGER,
        level       TEXT,
        visits      INTEGER NOT NULL,
        checked_at  TEXT    NOT NULL,
        created_at  TEXT    NOT NULL,
        updated_at  TEXT    NOT NULL,
        deleted_at  TEXT
    );
    CREATE UNIQUE INDEX marketing_listing_quality_by_item
        ON marketing_listing_quality (ml_item_id)
        WHERE deleted_at IS NULL;
    CREATE TABLE marketing_quality_actions (
        id          TEXT PRIMARY KEY,
        quality_id  TEXT    NOT NULL REFERENCES marketing_listing_quality (id),
        ml_rule_key TEXT    NOT NULL,
        kind        TEXT    NOT NULL,
        text        TEXT    NOT NULL,
        label       TEXT    NOT NULL,
        link        TEXT,
        position    INTEGER NOT NULL,
        created_at  TEXT    NOT NULL,
        updated_at  TEXT    NOT NULL,
        deleted_at  TEXT
    );
    CREATE UNIQUE INDEX marketing_quality_actions_by_rule
        ON marketing_quality_actions (quality_id, ml_rule_key)
        WHERE deleted_at IS NULL;",
};

/// The days of sales and visits that weigh on which listing comes first.
pub const IMPACT_DAYS: u32 = 30;

/// How the Sales Channel ranks a listing's quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QualityLevel {
    Basic,
    Standard,
    Professional,
}

impl QualityLevel {
    const ALL: [QualityLevel; 3] = [
        QualityLevel::Basic,
        QualityLevel::Standard,
        QualityLevel::Professional,
    ];

    fn code(self) -> &'static str {
        match self {
            QualityLevel::Basic => "basic",
            QualityLevel::Standard => "standard",
            QualityLevel::Professional => "professional",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.code() == code)
    }

    /// As Mercado Livre Brasil names it to the seller.
    pub fn name(self) -> &'static str {
        match self {
            QualityLevel::Basic => "Básica",
            QualityLevel::Standard => "Satisfatória",
            QualityLevel::Professional => "Profissional",
        }
    }
}

/// How much a pending action weighs on the listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ActionKind {
    /// Lowers the score until it is fixed.
    Problem,
    /// Raises the score once done.
    Opportunity,
}

impl ActionKind {
    const ALL: [ActionKind; 2] = [ActionKind::Problem, ActionKind::Opportunity];

    fn code(self) -> &'static str {
        match self {
            ActionKind::Problem => "problem",
            ActionKind::Opportunity => "opportunity",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.code() == code)
    }
}

/// Something the channel says the listing still lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityAction {
    /// The channel's id of the rule: what keeps it from showing twice.
    pub key: String,
    pub kind: ActionKind,
    /// What to do, in the channel's words.
    pub text: String,
    /// The words for the link.
    pub label: String,
    /// Where the channel lets the owner fix it.
    pub link: Option<String>,
}

/// A listing's quality as the channel last worked it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelQuality {
    /// From 0 to 100.
    pub score: u8,
    pub level: QualityLevel,
    /// The actions still pending, in the channel's order.
    pub pending: Vec<QualityAction>,
}

/// How a Sales Channel rates the owner's listings, as a Platform's adapter
/// reports it. Calls block on the network, so they run off the UI thread.
pub trait QualitySource: Send + Sync {
    /// The quality of the listing `listing` (the channel's id); `None`
    /// while the channel has not worked it out yet.
    fn quality(&self, listing: &str) -> Result<Option<ChannelQuality>, PlatformError>;

    /// The visits the listing had in the last `days` days.
    fn visits(&self, listing: &str, days: u32) -> Result<u32, PlatformError>;
}

/// A listing the owner has in the Sales Channel, as the caller knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedItem {
    /// The channel's id of the listing; its variations share it.
    pub listing: String,
    pub title: String,
    /// Where the listing opens in the browser.
    pub link: Option<String>,
    /// Units sold in the last [`IMPACT_DAYS`] days, every variation
    /// together.
    pub units_sold: u32,
}

/// What the panel knows of a listing's quality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rating {
    /// Not asked for yet: no Sync since the listing arrived.
    NotChecked,
    /// Asked for, but the channel has not worked it out yet.
    NotRated,
    Rated(ChannelQuality),
}

/// A listing on the quality panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityRow {
    pub listing: ListedItem,
    pub rating: Rating,
    /// Visits in the last [`IMPACT_DAYS`] days, as of the last Sync.
    pub visits: Option<u32>,
    pub checked_at: Option<Timestamp>,
}

impl QualityRow {
    /// The actions still pending, the problems first.
    pub fn pending(&self) -> &[QualityAction] {
        match &self.rating {
            Rating::Rated(quality) => &quality.pending,
            _ => &[],
        }
    }
}

/// What a Sync of Listing Quality did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QualitySync {
    /// Listings asked about.
    pub checked: usize,
    /// Listings the channel has not rated yet.
    pub not_rated: usize,
    /// Pending actions, every listing together.
    pub pending: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum QualityError {
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error("the Listing Quality holds a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for QualityError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => QualityError::Unreadable(text),
            StoredValueError::Sql(error) => QualityError::Sql(error),
        }
    }
}

/// The quality of the owner's listings and where to work first.
pub struct ListingQuality {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl ListingQuality {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            database,
            clock,
            ids,
        }
    }

    /// Since when sales count for [`ListedItem::units_sold`].
    pub fn impact_since(&self) -> Timestamp {
        self.clock.now() - TimeDelta::days(i64::from(IMPACT_DAYS))
    }

    /// Asks the channel for the quality and the visits of each of
    /// `listings` (the channel's ids, each once), and keeps them by the
    /// channel's id of the listing and of each rule: the same answers again
    /// change nothing, and an action the channel no longer reports is done.
    /// Nothing is kept unless every listing was read.
    pub async fn sync(
        &self,
        source: &dyn QualitySource,
        listings: &[String],
    ) -> Result<QualitySync, QualityError> {
        let mut seen = HashSet::new();
        let mut read = Vec::new();
        for listing in listings.iter().filter(|id| seen.insert(id.as_str())) {
            let quality = source.quality(listing)?;
            let visits = source.visits(listing, IMPACT_DAYS)?;
            read.push((listing, quality, visits));
        }

        let now = self.clock.now();
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let mut report = QualitySync::default();
        for (listing, quality, visits) in read {
            report.checked += 1;
            match &quality {
                Some(quality) => report.pending += quality.pending.len(),
                None => report.not_rated += 1,
            }
            let id = self
                .write_quality(&transaction, listing, quality.as_ref(), visits, now)
                .await?;
            let pending = quality.map(|quality| quality.pending).unwrap_or_default();
            self.write_actions(&transaction, id, &pending, now).await?;
        }
        transaction.commit().await?;
        Ok(report)
    }

    /// When the quality was last read; `None` before the first Sync.
    pub async fn last_sync(&self) -> Result<Option<Timestamp>, QualityError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT MAX(checked_at) FROM marketing_listing_quality WHERE deleted_at IS NULL",
                (),
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(row.optional_time_at(0)?),
            None => Ok(None),
        }
    }

    /// Each of `listings` (the first of each channel id) with its quality,
    /// where to work first on top: the listings with something pending, by
    /// units sold, then visits, then the lowest score; then the ones not
    /// rated yet, and last the ones with nothing pending.
    pub async fn panel(&self, listings: &[ListedItem]) -> Result<Vec<QualityRow>, QualityError> {
        let mut known = read_quality(self.database.connection()).await?;
        let mut seen = HashSet::new();
        let mut rows: Vec<QualityRow> = listings
            .iter()
            .filter(|listed| seen.insert(listed.listing.as_str()))
            .map(|listed| match known.remove(&listed.listing) {
                Some(stored) => QualityRow {
                    listing: listed.clone(),
                    rating: stored.rating,
                    visits: Some(stored.visits),
                    checked_at: Some(stored.checked_at),
                },
                None => QualityRow {
                    listing: listed.clone(),
                    rating: Rating::NotChecked,
                    visits: None,
                    checked_at: None,
                },
            })
            .collect();
        rows.sort_by(|a, b| {
            urgency(a)
                .cmp(&urgency(b))
                .then(b.listing.units_sold.cmp(&a.listing.units_sold))
                .then(b.visits.unwrap_or(0).cmp(&a.visits.unwrap_or(0)))
                .then(score(a).cmp(&score(b)))
                .then(a.listing.title.cmp(&b.listing.title))
        });
        Ok(rows)
    }

    /// Writes a listing's score, level and visits by the channel's id, and
    /// returns the row's id.
    async fn write_quality(
        &self,
        on: &Connection,
        listing: &str,
        quality: Option<&ChannelQuality>,
        visits: u32,
        now: Timestamp,
    ) -> Result<RecordId, QualityError> {
        on.execute(
            "INSERT INTO marketing_listing_quality
                 (id, ml_item_id, score, level, visits, checked_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6)
             ON CONFLICT (ml_item_id) WHERE deleted_at IS NULL
             DO UPDATE SET score = excluded.score, level = excluded.level,
                 visits = excluded.visits, checked_at = excluded.checked_at,
                 updated_at = excluded.updated_at",
            params![
                self.ids.next_id().to_string(),
                listing,
                quality.map(|quality| u32::from(quality.score)),
                quality.map(|quality| quality.level.code()),
                visits,
                stored(now)
            ],
        )
        .await?;
        let mut rows = on
            .query(
                "SELECT id FROM marketing_listing_quality
                 WHERE ml_item_id = ?1 AND deleted_at IS NULL",
                params![listing],
            )
            .await?;
        let row = rows
            .next()
            .await?
            .ok_or_else(|| QualityError::Unreadable(listing.to_owned()))?;
        Ok(row.id_at(0)?)
    }

    /// Writes the pending actions over a listing's: each by the channel's
    /// id of its rule, and those the channel no longer reports gone.
    async fn write_actions(
        &self,
        on: &Connection,
        quality: RecordId,
        pending: &[QualityAction],
        now: Timestamp,
    ) -> Result<(), QualityError> {
        let mut kept = HashSet::new();
        for (position, action) in pending.iter().enumerate() {
            if !kept.insert(action.key.as_str()) {
                continue;
            }
            on.execute(
                "INSERT INTO marketing_quality_actions
                     (id, quality_id, ml_rule_key, kind, text, label, link, position,
                      created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
                 ON CONFLICT (quality_id, ml_rule_key) WHERE deleted_at IS NULL
                 DO UPDATE SET kind = excluded.kind, text = excluded.text,
                     label = excluded.label, link = excluded.link,
                     position = excluded.position, updated_at = excluded.updated_at",
                params![
                    self.ids.next_id().to_string(),
                    quality.to_string(),
                    action.key.clone(),
                    action.kind.code(),
                    action.text.clone(),
                    action.label.clone(),
                    action.link.clone(),
                    position as i64,
                    stored(now)
                ],
            )
            .await?;
        }
        let mut rows = on
            .query(
                "SELECT id, ml_rule_key FROM marketing_quality_actions
                 WHERE quality_id = ?1 AND deleted_at IS NULL",
                params![quality.to_string()],
            )
            .await?;
        let mut done = Vec::new();
        while let Some(row) = rows.next().await? {
            if !kept.contains(row.get::<String>(1)?.as_str()) {
                done.push(row.id_at(0)?);
            }
        }
        for id in done {
            on.execute(
                "UPDATE marketing_quality_actions SET deleted_at = ?1, updated_at = ?1
                 WHERE id = ?2",
                params![stored(now), id.to_string()],
            )
            .await?;
        }
        Ok(())
    }
}

/// Which group of the panel a row falls in: something to do first, then
/// not rated yet, then nothing pending.
fn urgency(row: &QualityRow) -> u8 {
    match &row.rating {
        Rating::Rated(quality) if !quality.pending.is_empty() => 0,
        Rating::NotChecked | Rating::NotRated => 1,
        Rating::Rated(_) => 2,
    }
}

fn score(row: &QualityRow) -> u8 {
    match &row.rating {
        Rating::Rated(quality) => quality.score,
        _ => u8::MAX,
    }
}

/// A listing's quality as kept.
struct Stored {
    rating: Rating,
    visits: u32,
    checked_at: Timestamp,
}

/// Every listing's quality as kept, by the channel's id, with its pending
/// actions, the problems first.
async fn read_quality(on: &Connection) -> Result<BTreeMap<String, Stored>, QualityError> {
    let mut actions: BTreeMap<RecordId, Vec<(ActionKind, i64, QualityAction)>> = BTreeMap::new();
    let mut rows = on
        .query(
            "SELECT quality_id, ml_rule_key, kind, text, label, link, position
             FROM marketing_quality_actions WHERE deleted_at IS NULL",
            (),
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let code = row.get::<String>(2)?;
        let kind = ActionKind::from_code(&code).ok_or(QualityError::Unreadable(code))?;
        actions.entry(row.id_at(0)?).or_default().push((
            kind,
            row.get::<i64>(6)?,
            QualityAction {
                key: row.get(1)?,
                kind,
                text: row.get(3)?,
                label: row.get(4)?,
                link: row.get(5)?,
            },
        ));
    }

    let mut known = BTreeMap::new();
    let mut rows = on
        .query(
            "SELECT id, ml_item_id, score, level, visits, checked_at
             FROM marketing_listing_quality WHERE deleted_at IS NULL",
            (),
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let id = row.id_at(0)?;
        let rating = match (row.get::<Option<u32>>(2)?, row.get::<Option<String>>(3)?) {
            (Some(score), Some(level)) => {
                let mut pending = actions.remove(&id).unwrap_or_default();
                pending.sort_by_key(|(kind, position, _)| (*kind, *position));
                Rating::Rated(ChannelQuality {
                    score: u8::try_from(score)
                        .map_err(|_| QualityError::Unreadable(score.to_string()))?,
                    level: QualityLevel::from_code(&level)
                        .ok_or(QualityError::Unreadable(level))?,
                    pending: pending.into_iter().map(|(_, _, action)| action).collect(),
                })
            }
            _ => Rating::NotRated,
        };
        known.insert(
            row.get::<String>(1)?,
            Stored {
                rating,
                visits: row.get(4)?,
                checked_at: row.time_at(5)?,
            },
        );
    }
    Ok(known)
}
