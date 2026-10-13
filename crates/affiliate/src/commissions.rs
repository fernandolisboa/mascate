//! Conversions and their Commissions (#77): recorded from a Platform's
//! report in each Sync, or by the owner by hand, through one path with one
//! rule, the Platform's natural key (ADR 0030). Each status change is kept
//! with the time it was seen; nothing is deleted because a Platform stopped
//! reporting it. A Commission a Payout covers stays paid while the
//! Platform still calls it approved (#79).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use chrono::TimeDelta;
use libsql::{Connection, TransactionBehavior, params};
use mascate_kernel::{Clock, IdGenerator, Money, PlatformError, RecordId, Timestamp, fold};
use mascate_platform::{
    Database, Flag, FlagError, Flags, Migration, Registry, StoredRow, StoredValueError, stored,
};

use crate::source::{
    AffiliatePlatform, CommissionSource, CommissionStatus, ReportedConversion, ReportedProgram,
};
use crate::{COMMISSION_INCOME, FLAGS};

pub(crate) const CREATE_COMMISSIONS: Migration = Migration {
    version: 1,
    name: "create Programs, Conversions and Commissions",
    risky: false,
    sql: "CREATE TABLE affiliate_programs (
        id          TEXT PRIMARY KEY,
        platform    TEXT NOT NULL,
        program_key TEXT NOT NULL,
        name        TEXT NOT NULL,
        created_at  TEXT NOT NULL,
        updated_at  TEXT NOT NULL,
        deleted_at  TEXT
    );
    CREATE UNIQUE INDEX affiliate_programs_by_key
        ON affiliate_programs (platform, program_key)
        WHERE deleted_at IS NULL;
    CREATE TABLE affiliate_conversions (
        id          TEXT    PRIMARY KEY,
        platform    TEXT    NOT NULL,
        natural_key TEXT    NOT NULL,
        program_id  TEXT    NOT NULL REFERENCES affiliate_programs (id),
        product     TEXT    NOT NULL,
        sold_at     TEXT    NOT NULL,
        tracking    TEXT,
        manual      INTEGER NOT NULL,
        raw         TEXT,
        created_at  TEXT    NOT NULL,
        updated_at  TEXT    NOT NULL,
        deleted_at  TEXT
    );
    CREATE UNIQUE INDEX affiliate_conversions_by_key
        ON affiliate_conversions (platform, natural_key)
        WHERE deleted_at IS NULL;
    CREATE TABLE affiliate_commissions (
        id              TEXT PRIMARY KEY,
        conversion_id   TEXT NOT NULL REFERENCES affiliate_conversions (id),
        amount          TEXT NOT NULL,
        currency        TEXT NOT NULL,
        status          TEXT NOT NULL,
        platform_status TEXT NOT NULL,
        status_since    TEXT NOT NULL,
        created_at      TEXT NOT NULL,
        updated_at      TEXT NOT NULL,
        deleted_at      TEXT
    );
    CREATE UNIQUE INDEX affiliate_commissions_by_conversion
        ON affiliate_commissions (conversion_id)
        WHERE deleted_at IS NULL;
    CREATE INDEX affiliate_commissions_by_status
        ON affiliate_commissions (status);
    CREATE TABLE affiliate_commission_status_changes (
        id              TEXT PRIMARY KEY,
        commission_id   TEXT NOT NULL REFERENCES affiliate_commissions (id),
        status          TEXT NOT NULL,
        platform_status TEXT NOT NULL,
        seen_at         TEXT NOT NULL,
        platform_at     TEXT,
        created_at      TEXT NOT NULL,
        updated_at      TEXT NOT NULL,
        deleted_at      TEXT
    );
    CREATE INDEX affiliate_commission_status_changes_by_commission
        ON affiliate_commission_status_changes (commission_id, seen_at);
    CREATE TABLE affiliate_syncs (
        id          TEXT PRIMARY KEY,
        platform    TEXT NOT NULL,
        started_at  TEXT NOT NULL,
        finished_at TEXT NOT NULL,
        created_at  TEXT NOT NULL,
        updated_at  TEXT NOT NULL,
        deleted_at  TEXT
    );
    CREATE UNIQUE INDEX affiliate_syncs_by_platform
        ON affiliate_syncs (platform)
        WHERE deleted_at IS NULL;",
};

/// The days an earned Commission keeps the income Reminder up.
pub const INCOME_WINDOW_DAYS: i64 = 365;

/// What the owner types to record a Commission by hand, for Platforms
/// without an API or a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualCommission {
    pub platform: AffiliatePlatform,
    /// Empty means the Platform's own name.
    pub program: String,
    pub product: String,
    pub sold_at: Timestamp,
    pub amount: Money,
    pub status: CommissionStatus,
}

/// A Commission with the Conversion it pays for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commission {
    pub id: RecordId,
    pub platform: AffiliatePlatform,
    pub program: RecordId,
    pub program_name: String,
    pub product: String,
    pub sold_at: Timestamp,
    pub amount: Money,
    pub status: CommissionStatus,
    /// The sale's status as the Platform names it; empty when recorded by
    /// hand.
    pub platform_status: String,
    /// When the status last changed: the Platform's date when it gave one,
    /// else when the app saw it.
    pub status_since: Timestamp,
    pub tracking: Option<String>,
    /// Recorded by the owner, who may edit or delete it while no Payout
    /// covers it.
    pub manual: bool,
    /// The Payout that covers it, if one does.
    pub payout: Option<RecordId>,
    /// The Catalog's Product the owner linked its product to (#86).
    pub linked_product: Option<RecordId>,
}

/// A Conversion as a Platform last reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// Its natural key on the Platform.
    pub key: String,
    /// The Platform's answer for it, as the adapter kept it.
    pub raw: String,
}

/// One status a Commission took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusChange {
    pub status: CommissionStatus,
    pub platform_status: String,
    pub seen_at: Timestamp,
    /// When the Platform says it happened, when it says.
    pub platform_at: Option<Timestamp>,
}

/// Which Commissions to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommissionFilter {
    /// Empty lists every status.
    pub statuses: Vec<CommissionStatus>,
    /// By the date of the sale.
    pub sold: Range<Timestamp>,
}

impl Default for CommissionFilter {
    fn default() -> Self {
        Self {
            statuses: Vec::new(),
            sold: Timestamp::MIN_UTC..Timestamp::MAX_UTC,
        }
    }
}

/// What recording a batch of Conversions did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Recorded {
    pub added: usize,
    pub changed: usize,
    pub unchanged: usize,
}

/// What a Sync of one source did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffiliateSync {
    pub platform: AffiliatePlatform,
    /// The time asked of the Platform.
    pub window: Range<Timestamp>,
    pub recorded: Recorded,
    /// The Payouts the Platform reported, on Platforms that report them.
    pub payouts: Recorded,
}

impl AffiliateSync {
    /// Something new or changed came in.
    pub fn changed_anything(&self) -> bool {
        self.recorded.added + self.recorded.changed + self.payouts.added + self.payouts.changed > 0
    }
}

/// The last Sync of a Platform that went through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastSync {
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
}

#[derive(Debug, thiserror::Error)]
pub enum AffiliateError {
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error("no such Commission")]
    NotFound,
    #[error("only a Commission recorded by hand can be changed")]
    NotManual,
    #[error("a Commission a Payout covers changes only after that Payout is deleted")]
    Covered,
    #[error("no such Payout")]
    NoSuchPayout,
    #[error("only a Payout recorded by hand can be deleted")]
    NotManualPayout,
    #[error("a Payout is more than zero")]
    NoPayoutAmount,
    #[error("a Payout covers approved Commissions of its Platform and currency, once")]
    NotPayable,
    #[error(
        "only a Payout a Platform reported without naming its Commissions takes the ones the \
         owner confirms"
    )]
    NotCoverable,
    #[error("the product is missing")]
    NoProduct,
    #[error("no such Program")]
    NoSuchProgram,
    /// The promotion needs a Restricted Feature that is off.
    #[error("{}", .0.reason)]
    Restricted(Flag),
    #[error(transparent)]
    Flag(#[from] FlagError),
    #[error("a Commission is never negative")]
    Negative,
    #[error("the Commissions hold a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for AffiliateError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => AffiliateError::Unreadable(text),
            StoredValueError::Sql(error) => AffiliateError::Sql(error),
        }
    }
}

/// A Conversion as kept, to compare a new reading against.
pub(crate) struct Kept {
    conversion: RecordId,
    pub(crate) commission: RecordId,
    manual: bool,
    pub(crate) platform: String,
    program: RecordId,
    product: String,
    sold_at: String,
    tracking: Option<String>,
    raw: Option<String>,
    pub(crate) amount: Money,
    pub(crate) status: CommissionStatus,
    platform_status: String,
    /// A Payout covers it.
    pub(crate) covered: bool,
}

/// The owner's affiliate Programs, Conversions and Commissions.
pub struct Affiliate {
    pub(crate) database: Arc<Database>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) ids: Arc<dyn IdGenerator>,
    /// This module's Restricted Features, as the owner switched them in
    /// the shared flag history.
    pub(crate) flags: Flags,
}

impl Affiliate {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        let registry = Registry::new(&[FLAGS]).expect("the Affiliate's flags have distinct keys");
        Self {
            flags: Flags::new(database.clone(), registry, clock.clone(), ids.clone()),
            database,
            clock,
            ids,
        }
    }

    /// Reads `source` from the start of its last successful Sync minus the
    /// time a status still changes there, until now; the first time, as far
    /// back as it lets. Each Conversion is recorded by its natural key:
    /// read again, it changes nothing; changed, the same row is updated and
    /// a new status goes to the history. The Payouts the Platform reports
    /// are kept the same way, by their own key. Nothing is kept unless
    /// everything was read.
    pub async fn sync(
        &self,
        source: &dyn CommissionSource,
    ) -> Result<AffiliateSync, AffiliateError> {
        let platform = source.platform();
        let started = self.clock.now();
        let earliest = started - source.history();
        let from = match self.last_sync(platform).await? {
            Some(last) => (last.started_at - source.settle_time()).max(earliest),
            None => earliest,
        };
        let window = from..started;
        let reported = source.conversions(window.clone())?;
        let reported_payouts = source.payouts(window.clone())?;

        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let recorded = self
            .record_on(&transaction, platform, &reported, false)
            .await?;
        let payouts = self
            .record_reported_payouts(&transaction, platform, &reported_payouts)
            .await?;
        transaction
            .execute(
                "INSERT INTO affiliate_syncs
                     (id, platform, started_at, finished_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?4)
                 ON CONFLICT (platform) WHERE deleted_at IS NULL
                 DO UPDATE SET started_at = excluded.started_at,
                     finished_at = excluded.finished_at, updated_at = excluded.updated_at",
                params![
                    self.ids.next_id().to_string(),
                    platform.code(),
                    stored(started),
                    stored(self.clock.now())
                ],
            )
            .await?;
        transaction.commit().await?;
        Ok(AffiliateSync {
            platform,
            window,
            recorded,
            payouts,
        })
    }

    /// Records the Conversions read from a file the owner exported from
    /// `platform`, in one transaction and by the same natural-key rule as a
    /// Sync: importing the same file again changes nothing.
    pub async fn import(
        &self,
        platform: AffiliatePlatform,
        reported: &[ReportedConversion],
    ) -> Result<Recorded, AffiliateError> {
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let recorded = self
            .record_on(&transaction, platform, reported, false)
            .await?;
        transaction.commit().await?;
        Ok(recorded)
    }

    /// What [`Affiliate::import`] would do with `reported`, keeping nothing.
    pub async fn preview_import(
        &self,
        platform: AffiliatePlatform,
        reported: &[ReportedConversion],
    ) -> Result<Recorded, AffiliateError> {
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .await?;
        let recorded = self
            .record_on(&transaction, platform, reported, false)
            .await?;
        transaction.rollback().await?;
        Ok(recorded)
    }

    /// Every Conversion read from `platform`, by its key, with the
    /// Platform's answer as last read: what a file reader needs to know of
    /// the files imported before.
    pub async fn readings(
        &self,
        platform: AffiliatePlatform,
    ) -> Result<Vec<Reading>, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT natural_key, raw FROM affiliate_conversions
                 WHERE platform = ?1 AND manual = 0 AND deleted_at IS NULL
                 ORDER BY sold_at, natural_key",
                params![platform.code()],
            )
            .await?;
        let mut readings = Vec::new();
        while let Some(row) = rows.next().await? {
            readings.push(Reading {
                key: row.get(0)?,
                raw: row.get::<Option<String>>(1)?.unwrap_or_default(),
            });
        }
        Ok(readings)
    }

    /// The last Sync of `platform` that went through; `None` before the
    /// first.
    pub async fn last_sync(
        &self,
        platform: AffiliatePlatform,
    ) -> Result<Option<LastSync>, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT started_at, finished_at FROM affiliate_syncs
                 WHERE platform = ?1 AND deleted_at IS NULL",
                params![platform.code()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some(LastSync {
                started_at: row.time_at(0)?,
                finished_at: row.time_at(1)?,
            })),
            None => Ok(None),
        }
    }

    /// Records a Commission the owner typed.
    pub async fn record_manual(
        &self,
        manual: ManualCommission,
    ) -> Result<Commission, AffiliateError> {
        let key = self.ids.next_id().to_string();
        let reported = manual_reading(&manual, key.clone())?;
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        self.record_on(&transaction, manual.platform, &[reported], true)
            .await?;
        transaction.commit().await?;
        self.find_by_key(manual.platform, &key).await
    }

    /// Changes a Commission the owner recorded by hand; one read from a
    /// Platform is the Platform's to change, and one a Payout covers waits
    /// for that Payout to be deleted.
    pub async fn edit_manual(
        &self,
        commission: RecordId,
        manual: ManualCommission,
    ) -> Result<Commission, AffiliateError> {
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let kept = read_stored(&transaction, "c.id = ?1", params![commission.to_string()])
            .await?
            .ok_or(AffiliateError::NotFound)?;
        if !kept.manual {
            return Err(AffiliateError::NotManual);
        }
        if kept.covered {
            return Err(AffiliateError::Covered);
        }
        let reported = manual_reading(&manual, String::new())?;
        self.write(&transaction, manual.platform, &reported, true, Some(kept))
            .await?;
        transaction.commit().await?;
        self.commission(commission).await
    }

    /// Deletes a Commission the owner recorded by hand and no Payout covers.
    pub async fn delete_manual(&self, commission: RecordId) -> Result<(), AffiliateError> {
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let kept = read_stored(&transaction, "c.id = ?1", params![commission.to_string()])
            .await?
            .ok_or(AffiliateError::NotFound)?;
        if !kept.manual {
            return Err(AffiliateError::NotManual);
        }
        if kept.covered {
            return Err(AffiliateError::Covered);
        }
        let now = stored(self.clock.now());
        transaction
            .execute(
                "UPDATE affiliate_commissions SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                params![now.clone(), kept.commission.to_string()],
            )
            .await?;
        transaction
            .execute(
                "UPDATE affiliate_conversions SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                params![now, kept.conversion.to_string()],
            )
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// The Commissions `filter` picks, the latest sale first.
    pub async fn commissions(
        &self,
        filter: &CommissionFilter,
    ) -> Result<Vec<Commission>, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                &format!("{COMMISSION_QUERY} ORDER BY v.sold_at DESC, c.id DESC"),
                (),
            )
            .await?;
        let mut commissions = Vec::new();
        while let Some(row) = rows.next().await? {
            let commission = commission_from(&row)?;
            if filter.sold.contains(&commission.sold_at)
                && (filter.statuses.is_empty() || filter.statuses.contains(&commission.status))
            {
                commissions.push(commission);
            }
        }
        Ok(commissions)
    }

    pub async fn commission(&self, commission: RecordId) -> Result<Commission, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                &format!("{COMMISSION_QUERY} AND c.id = ?1"),
                params![commission.to_string()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => commission_from(&row),
            None => Err(AffiliateError::NotFound),
        }
    }

    /// Every status `commission` took, the first one first.
    pub async fn status_history(
        &self,
        commission: RecordId,
    ) -> Result<Vec<StatusChange>, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT status, platform_status, seen_at, platform_at
                 FROM affiliate_commission_status_changes
                 WHERE commission_id = ?1 AND deleted_at IS NULL
                 ORDER BY seen_at, id",
                params![commission.to_string()],
            )
            .await?;
        let mut changes = Vec::new();
        while let Some(row) = rows.next().await? {
            changes.push(StatusChange {
                status: status_at(&row, 0)?,
                platform_status: row.get(1)?,
                seen_at: row.time_at(2)?,
                platform_at: row.optional_time_at(3)?,
            });
        }
        Ok(changes)
    }

    /// The Platform's answer for `commission` as last read; `None` for one
    /// recorded by hand.
    pub async fn last_reading(
        &self,
        commission: RecordId,
    ) -> Result<Option<String>, AffiliateError> {
        let kept = read_stored(
            self.database.connection(),
            "c.id = ?1",
            params![commission.to_string()],
        )
        .await?
        .ok_or(AffiliateError::NotFound)?;
        Ok(kept.raw)
    }

    /// The Reminders the module raises now: the one about income tax, while
    /// a Commission earned in the last [`INCOME_WINDOW_DAYS`] exists.
    pub async fn raised_reminders(&self) -> Result<Vec<&'static str>, AffiliateError> {
        let since = self.clock.now() - TimeDelta::days(INCOME_WINDOW_DAYS);
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT 1 FROM affiliate_commissions c
                 JOIN affiliate_conversions v ON v.id = c.conversion_id
                 WHERE c.deleted_at IS NULL AND v.deleted_at IS NULL
                   AND c.status IN (?1, ?2) AND v.sold_at >= ?3
                 LIMIT 1",
                params![
                    CommissionStatus::Approved.code(),
                    CommissionStatus::Paid.code(),
                    stored(since)
                ],
            )
            .await?;
        Ok(match rows.next().await? {
            Some(_) => vec![COMMISSION_INCOME.key],
            None => Vec::new(),
        })
    }

    async fn find_by_key(
        &self,
        platform: AffiliatePlatform,
        key: &str,
    ) -> Result<Commission, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                &format!("{COMMISSION_QUERY} AND v.platform = ?1 AND v.natural_key = ?2"),
                params![platform.code(), key],
            )
            .await?;
        match rows.next().await? {
            Some(row) => commission_from(&row),
            None => Err(AffiliateError::NotFound),
        }
    }

    /// The one path every Conversion takes into the app, from a Sync, a
    /// file or the owner's hand. A key repeated in `reported` counts once,
    /// as its last reading. A key the owner recorded by hand is never
    /// touched by a reading from a Platform, nor the other way round.
    async fn record_on(
        &self,
        on: &Connection,
        platform: AffiliatePlatform,
        reported: &[ReportedConversion],
        manual: bool,
    ) -> Result<Recorded, AffiliateError> {
        let mut last: HashMap<&str, usize> = HashMap::new();
        for (index, conversion) in reported.iter().enumerate() {
            last.insert(conversion.key.as_str(), index);
        }
        let mut recorded = Recorded::default();
        for (index, conversion) in reported.iter().enumerate() {
            if last.get(conversion.key.as_str()) != Some(&index) || conversion.key.is_empty() {
                continue;
            }
            let existing = read_stored(
                on,
                "v.platform = ?1 AND v.natural_key = ?2",
                params![platform.code(), conversion.key.clone()],
            )
            .await?;
            if existing.as_ref().is_some_and(|kept| kept.manual != manual) {
                recorded.unchanged += 1;
                continue;
            }
            match self
                .write(on, platform, conversion, manual, existing)
                .await?
            {
                Write::Added => recorded.added += 1,
                Write::Changed => recorded.changed += 1,
                Write::Unchanged => recorded.unchanged += 1,
            }
        }
        Ok(recorded)
    }

    /// Writes one reading over what is stored for it, if anything: a new
    /// Conversion, the same one changed, or nothing when nothing changed.
    /// Editing a manual one keeps its natural key.
    async fn write(
        &self,
        on: &Connection,
        platform: AffiliatePlatform,
        reported: &ReportedConversion,
        manual: bool,
        existing: Option<Kept>,
    ) -> Result<Write, AffiliateError> {
        let now = self.clock.now();
        let program = self
            .program_for(on, platform, &reported.program, now)
            .await?;
        let raw = (!manual).then(|| reported.raw.clone());
        let sold_at = stored(reported.sold_at);
        let Some(kept) = existing else {
            let conversion = self.ids.next_id();
            let commission = self.ids.next_id();
            on.execute(
                "INSERT INTO affiliate_conversions
                     (id, platform, natural_key, program_id, product, sold_at, tracking,
                      manual, raw, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![
                    conversion.to_string(),
                    platform.code(),
                    reported.key.clone(),
                    program.to_string(),
                    reported.product.clone(),
                    sold_at,
                    reported.tracking.clone(),
                    i64::from(manual),
                    raw,
                    stored(now)
                ],
            )
            .await?;
            on.execute(
                "INSERT INTO affiliate_commissions
                     (id, conversion_id, amount, currency, status, platform_status,
                      status_since, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    commission.to_string(),
                    conversion.to_string(),
                    reported.commission.amount().to_string(),
                    reported.commission.currency().code(),
                    reported.status.code(),
                    reported.platform_status.clone(),
                    stored(reported.status_changed_at.unwrap_or(now)),
                    stored(now)
                ],
            )
            .await?;
            self.add_change(on, commission, reported.status, reported, now)
                .await?;
            return Ok(Write::Added);
        };

        // A Payout is proof the money came: the Platform calling it approved
        // again does not undo that, a reversal does.
        let status = match reported.status {
            CommissionStatus::Pending | CommissionStatus::Approved if kept.covered => {
                CommissionStatus::Paid
            }
            status => status,
        };
        let status_changed =
            kept.status != status || kept.platform_status != reported.platform_status;
        let same = kept.platform == platform.code()
            && kept.program == program
            && kept.product == reported.product
            && kept.sold_at == sold_at
            && kept.tracking == reported.tracking
            && kept.raw == raw
            && kept.amount == reported.commission
            && !status_changed;
        if same {
            return Ok(Write::Unchanged);
        }
        on.execute(
            "UPDATE affiliate_conversions
             SET platform = ?1, program_id = ?2, product = ?3, sold_at = ?4, tracking = ?5,
                 raw = ?6, updated_at = ?7
             WHERE id = ?8",
            params![
                platform.code(),
                program.to_string(),
                reported.product.clone(),
                sold_at,
                reported.tracking.clone(),
                raw,
                stored(now),
                kept.conversion.to_string()
            ],
        )
        .await?;
        on.execute(
            "UPDATE affiliate_commissions
             SET amount = ?1, currency = ?2, status = ?3, platform_status = ?4,
                 status_since = CASE WHEN ?5 THEN ?6 ELSE status_since END, updated_at = ?7
             WHERE id = ?8",
            params![
                reported.commission.amount().to_string(),
                reported.commission.currency().code(),
                status.code(),
                reported.platform_status.clone(),
                i64::from(status_changed),
                stored(reported.status_changed_at.unwrap_or(now)),
                stored(now),
                kept.commission.to_string()
            ],
        )
        .await?;
        if status_changed {
            self.add_change(on, kept.commission, status, reported, now)
                .await?;
        }
        Ok(Write::Changed)
    }

    async fn add_change(
        &self,
        on: &Connection,
        commission: RecordId,
        status: CommissionStatus,
        reported: &ReportedConversion,
        now: Timestamp,
    ) -> Result<(), AffiliateError> {
        on.execute(
            "INSERT INTO affiliate_commission_status_changes
                 (id, commission_id, status, platform_status, seen_at, platform_at,
                  created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?5, ?5)",
            params![
                self.ids.next_id().to_string(),
                commission.to_string(),
                status.code(),
                reported.platform_status.clone(),
                stored(now),
                reported.status_changed_at.map(stored)
            ],
        )
        .await?;
        Ok(())
    }

    /// The Program `reported` names, created the first time it shows up.
    /// Its name stays the one it was born with: the owner renames it.
    async fn program_for(
        &self,
        on: &Connection,
        platform: AffiliatePlatform,
        reported: &ReportedProgram,
        now: Timestamp,
    ) -> Result<RecordId, AffiliateError> {
        on.execute(
            "INSERT INTO affiliate_programs
                 (id, platform, program_key, name, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT (platform, program_key) WHERE deleted_at IS NULL DO NOTHING",
            params![
                self.ids.next_id().to_string(),
                platform.code(),
                reported.id.clone(),
                reported.name.clone(),
                stored(now)
            ],
        )
        .await?;
        let mut rows = on
            .query(
                "SELECT id FROM affiliate_programs
                 WHERE platform = ?1 AND program_key = ?2 AND deleted_at IS NULL",
                params![platform.code(), reported.id.clone()],
            )
            .await?;
        let row = rows.next().await?.expect("the Program was just written");
        Ok(row.id_at(0)?)
    }
}

enum Write {
    Added,
    Changed,
    Unchanged,
}

pub(crate) const COMMISSION_QUERY: &str =
    "SELECT c.id, v.platform, p.id, p.name, v.product, v.sold_at,
         c.amount, c.currency, c.status, c.platform_status, c.status_since, v.tracking, v.manual,
         (SELECT l.payout_id FROM affiliate_payout_commissions l
          WHERE l.commission_id = c.id AND l.deleted_at IS NULL),
         k.catalog_product_id
     FROM affiliate_commissions c
     JOIN affiliate_conversions v ON v.id = c.conversion_id
     JOIN affiliate_programs p ON p.id = v.program_id
     LEFT JOIN affiliate_product_links k
         ON k.program_id = p.id AND k.product = trim(v.product) AND k.deleted_at IS NULL
     WHERE c.deleted_at IS NULL AND v.deleted_at IS NULL";

pub(crate) fn commission_from(row: &libsql::Row) -> Result<Commission, AffiliateError> {
    Ok(Commission {
        id: row.id_at(0)?,
        platform: platform_at(row, 1)?,
        program: row.id_at(2)?,
        program_name: row.get(3)?,
        product: row.get(4)?,
        sold_at: row.time_at(5)?,
        amount: Money::new(row.decimal_at(6)?, row.currency_at(7)?),
        status: status_at(row, 8)?,
        platform_status: row.get(9)?,
        status_since: row.time_at(10)?,
        tracking: row.get(11)?,
        manual: row.get::<i64>(12)? == 1,
        payout: row.optional_id_at(13)?,
        linked_product: row.optional_id_at(14)?,
    })
}

pub(crate) async fn read_stored(
    on: &Connection,
    condition: &str,
    params: impl libsql::params::IntoParams,
) -> Result<Option<Kept>, AffiliateError> {
    let mut rows = on
        .query(
            &format!(
                "SELECT v.id, c.id, v.manual, v.platform, v.program_id, v.product, v.sold_at,
                     v.tracking, v.raw, c.amount, c.currency, c.status, c.platform_status,
                     EXISTS (SELECT 1 FROM affiliate_payout_commissions l
                             WHERE l.commission_id = c.id AND l.deleted_at IS NULL)
                 FROM affiliate_commissions c
                 JOIN affiliate_conversions v ON v.id = c.conversion_id
                 WHERE c.deleted_at IS NULL AND v.deleted_at IS NULL AND {condition}"
            ),
            params,
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(Kept {
        conversion: row.id_at(0)?,
        commission: row.id_at(1)?,
        manual: row.get::<i64>(2)? == 1,
        platform: row.get(3)?,
        program: row.id_at(4)?,
        product: row.get(5)?,
        sold_at: row.get(6)?,
        tracking: row.get(7)?,
        raw: row.get(8)?,
        amount: Money::new(row.decimal_at(9)?, row.currency_at(10)?),
        status: status_at(&row, 11)?,
        platform_status: row.get(12)?,
        covered: row.get::<i64>(13)? == 1,
    }))
}

/// What the owner typed, as a reading of the Platform: the same path and
/// the same natural-key rule as a Sync.
fn manual_reading(
    manual: &ManualCommission,
    key: String,
) -> Result<ReportedConversion, AffiliateError> {
    let product = manual.product.trim();
    if product.is_empty() {
        return Err(AffiliateError::NoProduct);
    }
    if manual.amount.is_negative() {
        return Err(AffiliateError::Negative);
    }
    let program = match manual.program.trim() {
        "" => manual.platform.name(),
        typed => typed,
    };
    let folded: String = program.chars().filter_map(fold).collect();
    Ok(ReportedConversion {
        key,
        program: ReportedProgram {
            // Typed names with the same letters are one Program.
            id: format!("manual:{folded}"),
            name: program.to_owned(),
        },
        product: product.to_owned(),
        sold_at: manual.sold_at,
        commission: manual.amount,
        status: manual.status,
        platform_status: String::new(),
        status_changed_at: None,
        tracking: None,
        raw: String::new(),
    })
}

pub(crate) fn platform_at(row: &libsql::Row, at: i32) -> Result<AffiliatePlatform, AffiliateError> {
    let code = row.get::<String>(at)?;
    AffiliatePlatform::from_code(&code).ok_or(AffiliateError::Unreadable(code))
}

fn status_at(row: &libsql::Row, at: i32) -> Result<CommissionStatus, AffiliateError> {
    let code = row.get::<String>(at)?;
    CommissionStatus::from_code(&code).ok_or(AffiliateError::Unreadable(code))
}
