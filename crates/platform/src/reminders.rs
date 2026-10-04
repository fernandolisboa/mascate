//! Reminders: fiscal and legal notes that never block anything (ADR 0006).
//! Each module declares its own in code; the owner dismisses one and it
//! comes back after the interval it declares.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::TimeDelta;
use libsql::params;
use mascate_kernel::{Clock, IdGenerator, Record};

use crate::stored_time::{read_stored, stored};
use crate::{Database, Migration, Registered, Registry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReminderTopic {
    Fiscal,
    Legal,
}

/// A note shown to the owner as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reminder {
    /// Stable code, used in storage: `<module>.<subject>`.
    pub key: &'static str,
    pub topic: ReminderTopic,
    pub title: &'static str,
    pub text: &'static str,
    /// Days a dismissal lasts before the Reminder shows again.
    pub reappears_after_days: u16,
}

impl Registered for Reminder {
    fn key(&self) -> &'static str {
        self.key
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReminderError {
    #[error("no reminder {0} is registered")]
    Unknown(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

pub(crate) const CREATE_REMINDER_DISMISSALS: Migration = Migration {
    version: 3,
    name: "create reminder dismissals",
    sql: "CREATE TABLE platform_reminder_dismissals (
        id         TEXT PRIMARY KEY,
        reminder   TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE INDEX platform_reminder_dismissals_by_reminder
        ON platform_reminder_dismissals (reminder, created_at);",
};

/// Every Reminder the app knows and which of them show now.
pub struct Reminders {
    database: Arc<Database>,
    registry: Registry<Reminder>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl Reminders {
    pub fn new(
        database: Arc<Database>,
        registry: Registry<Reminder>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            database,
            registry,
            clock,
            ids,
        }
    }

    /// The Reminders to show now, in declaration order: those never
    /// dismissed and those whose last dismissal has run out.
    pub async fn showing(&self) -> Result<Vec<Reminder>, ReminderError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT reminder, MAX(created_at) FROM platform_reminder_dismissals
                 WHERE deleted_at IS NULL GROUP BY reminder",
                (),
            )
            .await?;
        let mut dismissed = HashMap::new();
        while let Some(row) = rows.next().await? {
            dismissed.insert(row.get::<String>(0)?, row.get::<String>(1)?);
        }
        let now = self.clock.now();
        Ok(self
            .registry
            .all()
            .iter()
            .copied()
            .filter(|reminder| {
                // A dismissal time that cannot be read counts as none: a
                // Reminder that shows too often beats one that never does.
                dismissed
                    .get(reminder.key)
                    .and_then(|at| read_stored(at))
                    .is_none_or(|at| {
                        now >= at + TimeDelta::days(i64::from(reminder.reappears_after_days))
                    })
            })
            .collect())
    }

    /// Hides `key` until its interval runs out, counted from now.
    pub async fn dismiss(&self, key: &str) -> Result<(), ReminderError> {
        let reminder = self
            .registry
            .get(key)
            .ok_or_else(|| ReminderError::Unknown(key.to_owned()))?;
        let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
        let at = stored(record.created_at);
        self.database
            .connection()
            .execute(
                "INSERT INTO platform_reminder_dismissals (id, reminder, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)",
                params![record.id.to_string(), reminder.key, at],
            )
            .await?;
        Ok(())
    }
}
