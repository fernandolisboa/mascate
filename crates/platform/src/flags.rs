//! Feature flags (ADR 0006). The module that builds a feature declares its
//! flag in code, with why it is off and what turning it on risks; whether
//! it is on lives in the database, as a history of who switched it and when.

use std::collections::HashMap;
use std::sync::Arc;

use libsql::params;
use mascate_kernel::{Clock, IdGenerator, Record, Timestamp};

use crate::stored_time::{read_stored, stored};
use crate::{Database, Environment, Migration, Registered, Registry};

/// Why a feature sits behind a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlagKind {
    /// Forbidden by law or by a Platform's terms, or terms nobody has read
    /// yet (ADR 0006).
    RestrictedFeature,
    /// Allowed, but the owner decides whether the app may do it alone.
    ProductChoice,
}

/// The phase of the plan that brings the feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Phase {
    One,
    Two,
    Three,
}

impl Phase {
    pub fn number(self) -> u8 {
        match self {
            Phase::One => 1,
            Phase::Two => 2,
            Phase::Three => 3,
        }
    }
}

/// A feature that is built and tested but starts off. The texts are shown
/// to the owner as they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flag {
    /// Stable code, used in storage: `<module>.<feature>`.
    pub key: &'static str,
    pub name: &'static str,
    pub kind: FlagKind,
    pub phase: Phase,
    /// Why it is off.
    pub reason: &'static str,
    /// What can go wrong once it is on.
    pub risk: &'static str,
}

impl Registered for Flag {
    fn key(&self) -> &'static str {
        self.key
    }
}

/// One switch of a flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagChange {
    pub on: bool,
    pub at: Timestamp,
    /// The system user who switched it.
    pub by: String,
}

/// A flag and its last switch; `None` means it was never turned on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagStatus {
    pub flag: Flag,
    pub last_change: Option<FlagChange>,
}

impl FlagStatus {
    pub fn is_on(&self) -> bool {
        self.last_change.as_ref().is_some_and(|change| change.on)
    }
}

/// The step between asking to turn a flag on and turning it on: it carries
/// the risk to show, and only [`TurnOnRequest::confirm`] makes it something
/// [`Flags::turn_on`] accepts. Dropping it changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "a request does nothing until confirmed and passed to Flags::turn_on"]
pub struct TurnOnRequest {
    flag: Flag,
}

impl TurnOnRequest {
    pub fn flag(&self) -> Flag {
        self.flag
    }

    /// The owner read the risk and confirmed, as the system user `by`.
    pub fn confirm(self, by: impl Into<String>) -> ConfirmedTurnOn {
        ConfirmedTurnOn {
            key: self.flag.key,
            by: by.into(),
        }
    }
}

/// A turn-on the owner confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedTurnOn {
    key: &'static str,
    by: String,
}

#[derive(Debug, thiserror::Error)]
pub enum FlagError {
    #[error("no flag {0} is registered")]
    Unknown(String),
    #[error("{0} is turned off")]
    Off(&'static str),
    #[error("flag {key} has an unreadable change time {at:?}")]
    UnreadableTime { key: String, at: String },
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

pub(crate) const CREATE_FLAG_CHANGES: Migration = Migration {
    version: 2,
    name: "create flag changes",
    risky: false,
    sql: "CREATE TABLE platform_flag_changes (
        id         TEXT PRIMARY KEY,
        flag       TEXT NOT NULL,
        turned_on  INTEGER NOT NULL,
        changed_by TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE INDEX platform_flag_changes_by_flag ON platform_flag_changes (flag, created_at);",
};

/// Every flag the app knows and whether each is on. Code behind a flag asks
/// here before acting.
pub struct Flags {
    database: Arc<Database>,
    registry: Registry<Flag>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl Flags {
    pub fn new(
        database: Arc<Database>,
        registry: Registry<Flag>,
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

    /// Every flag in the order the modules declared them, with its last switch.
    pub async fn statuses(&self) -> Result<Vec<FlagStatus>, FlagError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT flag, turned_on, changed_by, created_at FROM platform_flag_changes
                 WHERE deleted_at IS NULL ORDER BY created_at, id",
                (),
            )
            .await?;
        let mut last: HashMap<String, FlagChange> = HashMap::new();
        while let Some(row) = rows.next().await? {
            let key = row.get::<String>(0)?;
            let change = change_from(&key, row.get::<i64>(1)?, row.get(2)?, row.get(3)?)?;
            last.insert(key, change);
        }
        Ok(self
            .registry
            .all()
            .iter()
            .map(|&flag| FlagStatus {
                flag,
                last_change: last.remove(flag.key),
            })
            .collect())
    }

    pub async fn is_on(&self, flag: &Flag) -> Result<bool, FlagError> {
        self.known(flag.key)?;
        Ok(self
            .last_change(flag.key)
            .await?
            .is_some_and(|change| change.on))
    }

    /// The guard for code behind `flag`: fails unless the flag is on.
    pub async fn ensure_on(&self, flag: &Flag) -> Result<(), FlagError> {
        if self.is_on(flag).await? {
            Ok(())
        } else {
            Err(FlagError::Off(flag.name))
        }
    }

    /// Starts turning `key` on; nothing changes until the request is confirmed.
    pub fn request_turn_on(&self, key: &str) -> Result<TurnOnRequest, FlagError> {
        Ok(TurnOnRequest {
            flag: self.known(key)?,
        })
    }

    /// Turns a flag on. Already on, it keeps its first switch.
    pub async fn turn_on(&self, confirmed: ConfirmedTurnOn) -> Result<(), FlagError> {
        let flag = self.known(confirmed.key)?;
        self.switch(flag, true, &confirmed.by).await
    }

    /// Turns a flag off; no confirmation, since off is the safe side.
    pub async fn turn_off(&self, key: &str, by: &str) -> Result<(), FlagError> {
        let flag = self.known(key)?;
        self.switch(flag, false, by).await
    }

    fn known(&self, key: &str) -> Result<Flag, FlagError> {
        self.registry
            .get(key)
            .ok_or_else(|| FlagError::Unknown(key.to_owned()))
    }

    async fn last_change(&self, key: &str) -> Result<Option<FlagChange>, FlagError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT turned_on, changed_by, created_at FROM platform_flag_changes
                 WHERE flag = ?1 AND deleted_at IS NULL ORDER BY created_at DESC, id DESC LIMIT 1",
                params![key],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some(change_from(
                key,
                row.get::<i64>(0)?,
                row.get(1)?,
                row.get(2)?,
            )?)),
            None => Ok(None),
        }
    }

    /// Records a switch unless the flag is already in that state, in one
    /// statement so two racing switches cannot both record the same state.
    async fn switch(&self, flag: Flag, on: bool, by: &str) -> Result<(), FlagError> {
        let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
        let at = stored(record.created_at);
        self.database
            .connection()
            .execute(
                "INSERT INTO platform_flag_changes
                     (id, flag, turned_on, changed_by, created_at, updated_at)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?5
                 WHERE COALESCE((SELECT turned_on FROM platform_flag_changes
                                 WHERE flag = ?2 AND deleted_at IS NULL
                                 ORDER BY created_at DESC, id DESC LIMIT 1), 0) != ?3",
                params![record.id.to_string(), flag.key, i64::from(on), by, at],
            )
            .await?;
        Ok(())
    }
}

fn change_from(key: &str, on: i64, by: String, at: String) -> Result<FlagChange, FlagError> {
    let Some(at) = read_stored(&at) else {
        return Err(FlagError::UnreadableTime {
            key: key.to_owned(),
            at,
        });
    };
    Ok(FlagChange {
        on: on != 0,
        at,
        by,
    })
}

/// The name of the system user running the app, for "who turned it on".
pub fn system_user(environment: &Environment) -> Option<String> {
    ["USERNAME", "USER"]
        .into_iter()
        .filter_map(environment)
        .map(|user| user.trim().to_owned())
        .find(|user| !user.is_empty())
}
