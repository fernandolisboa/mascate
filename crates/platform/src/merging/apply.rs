//! "Mesclar" (ADR 0032, decisions 6 to 8): the compared Merge Import
//! written in one transaction after a Backup, and the reports of the merges
//! applied here, kept for the owner to read again.

use std::path::{Path, PathBuf};

use libsql::{TransactionBehavior, Value, params};
use mascate_kernel::{RecordId, Timestamp};
use serde::{Deserialize, Serialize};

use super::compare::{MergeError, MergeImport, MergeReport, TableWrites, attach, examine, finish};
use crate::{Backups, Migration, StoredRow, stored};

pub(crate) const CREATE_MERGES: Migration = Migration {
    version: 6,
    name: "create the merge imports applied",
    risky: false,
    sql: "CREATE TABLE platform_merges (
        id         TEXT PRIMARY KEY,
        file       TEXT NOT NULL,
        backup     TEXT NOT NULL,
        report     TEXT NOT NULL,
        follow_ups TEXT NOT NULL,
        opened_at  TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );",
};

/// A Merge Import written into this database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedMerge {
    pub id: RecordId,
    /// The other database, as the owner picked it.
    pub file: PathBuf,
    /// The Backup of this database taken just before the merge.
    pub backup: PathBuf,
    pub merged_at: Timestamp,
    /// What the merge did, worked out on the database as it was then.
    pub report: MergeReport,
    pub follow_ups: Vec<FollowUp>,
}

/// What the database cannot carry, left for the owner after a merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FollowUp {
    /// A Connection without its keys here: they stay in each machine's
    /// credential store (ADR 0008).
    ConnectHere { connection: String },
    /// A Product new here whose folder of files stayed on the other machine
    /// (ADR 0011).
    CopyFolder { product: String },
    /// A Product whose SKU the merge changed and whose folder kept the old
    /// name.
    FolderNotRenamed { product: String, reason: String },
}

const COLUMNS: &str = "id, file, backup, report, follow_ups, created_at";

impl MergeImport {
    /// Merges `file` into this database: a Backup with `backups` first, then
    /// the comparison worked out again on the database as it is now and
    /// written in one transaction, with its report. `None` when the file
    /// brings nothing to write here, and nothing was written. A failure
    /// leaves the database as it was.
    pub async fn merge(
        &self,
        file: &Path,
        backups: &Backups,
    ) -> Result<Option<AppliedMerge>, MergeError> {
        let copy = self.prepare(file).await?;
        let backup = backups.back_up_now().await.map_err(MergeError::Backup)?;
        let connection = attach(&self.database, &copy).await?;
        let merged = async {
            // Taken before reading, so no Sync writes between the report
            // and the rows it describes.
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .await?;
            // A replaced row goes out and back in under the same id: the
            // rows pointing to it hold again by the commit.
            transaction
                .execute("PRAGMA defer_foreign_keys = ON", ())
                .await?;
            let (report, writes) = examine(&transaction, self.merges).await?;
            if !report.brings_changes() {
                transaction.rollback().await?;
                return Ok(None);
            }
            for table in &writes {
                write(&transaction, table).await?;
            }
            let applied = AppliedMerge {
                id: self.ids.next_id(),
                file: file.to_path_buf(),
                backup: backup.path,
                merged_at: self.clock.now(),
                report,
                follow_ups: Vec::new(),
            };
            transaction
                .execute(
                    "INSERT INTO main.platform_merges
                        (id, file, backup, report, follow_ups, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, '[]', ?5, ?5)",
                    params![
                        applied.id.to_string(),
                        applied.file.to_string_lossy().into_owned(),
                        applied.backup.to_string_lossy().into_owned(),
                        serde_json::to_string(&applied.report)?,
                        stored(applied.merged_at),
                    ],
                )
                .await?;
            transaction.commit().await?;
            Ok(Some(applied))
        }
        .await;
        finish(connection, copy, merged).await
    }

    /// Saves what the owner still has to do after `merge`, found once it
    /// was written.
    pub async fn save_follow_ups(
        &self,
        merge: RecordId,
        follow_ups: &[FollowUp],
    ) -> Result<(), MergeError> {
        self.database
            .connection()
            .execute(
                "UPDATE platform_merges SET follow_ups = ?2, updated_at = ?3 WHERE id = ?1",
                params![
                    merge.to_string(),
                    serde_json::to_string(follow_ups)?,
                    stored(self.clock.now()),
                ],
            )
            .await?;
        Ok(())
    }

    /// The merges applied here, newest first.
    pub async fn merges(&self) -> Result<Vec<AppliedMerge>, MergeError> {
        let mut rows = self
            .database
            .connection()
            .query(
                &format!(
                    "SELECT {COLUMNS} FROM platform_merges WHERE deleted_at IS NULL
                     ORDER BY created_at DESC, id DESC"
                ),
                (),
            )
            .await?;
        let mut merges = Vec::new();
        while let Some(row) = rows.next().await? {
            merges.push(applied_from(&row)?);
        }
        Ok(merges)
    }

    /// The newest merge the owner has not seen yet, as the app opens after
    /// it; every merge counts as seen from then on.
    pub async fn open_unopened(&self) -> Result<Option<AppliedMerge>, MergeError> {
        let connection = self.database.connection();
        let mut rows = connection
            .query(
                &format!(
                    "SELECT {COLUMNS} FROM platform_merges
                     WHERE opened_at IS NULL AND deleted_at IS NULL
                     ORDER BY created_at DESC, id DESC LIMIT 1"
                ),
                (),
            )
            .await?;
        let Some(row) = rows.next().await? else {
            return Ok(None);
        };
        let newest = applied_from(&row)?;
        drop(rows);
        let now = stored(self.clock.now());
        connection
            .execute(
                "UPDATE platform_merges SET opened_at = ?1, updated_at = ?1
                 WHERE opened_at IS NULL",
                [now],
            )
            .await?;
        Ok(Some(newest))
    }
}

/// Writes one table's rows: the replaced ones leave and come back under
/// their ids, so no row holds a unique key another one is about to give up.
async fn write(connection: &libsql::Connection, writes: &TableWrites) -> Result<(), MergeError> {
    let id = writes
        .columns
        .iter()
        .position(|column| column == "id")
        .expect("every record has an id: merge_gaps checked it");
    let table = writes.table;
    for row in &writes.replaced {
        connection
            .execute(
                &format!("DELETE FROM main.{table} WHERE id = ?1"),
                [Value::from(&row[id])],
            )
            .await?;
    }
    let placeholders: Vec<String> = (1..=writes.columns.len())
        .map(|at| format!("?{at}"))
        .collect();
    let insert = format!(
        "INSERT INTO main.{table} ({}) VALUES ({})",
        writes.columns.join(", "),
        placeholders.join(", ")
    );
    for row in writes.replaced.iter().chain(&writes.new) {
        connection
            .execute(&insert, row.iter().map(Value::from).collect::<Vec<_>>())
            .await?;
    }
    Ok(())
}

fn applied_from(row: &libsql::Row) -> Result<AppliedMerge, MergeError> {
    Ok(AppliedMerge {
        id: row.id_at(0)?,
        file: PathBuf::from(row.get::<String>(1)?),
        backup: PathBuf::from(row.get::<String>(2)?),
        report: serde_json::from_str(&row.get::<String>(3)?)?,
        follow_ups: serde_json::from_str(&row.get::<String>(4)?)?,
        merged_at: row.time_at(5)?,
    })
}
