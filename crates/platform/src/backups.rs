//! Backups (#6): consistent copies of the open database in a folder the owner
//! picks, one a day and on demand, keeping the newest few. An export is the
//! same copy at a path of the owner's choosing; a restore stages a checked
//! file that replaces the database the next time the app starts. Secrets live
//! in the system store, never in the database, so no copy holds them.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{NaiveDateTime, TimeDelta};
use directories::{ProjectDirs, UserDirs};
use libsql::{Connection, OpenFlags, Value};
use mascate_kernel::{Clock, IdGenerator, Timestamp};

use crate::{Database, Migration, ModuleMigrations, single_row};

pub const DEFAULT_KEEP: u16 = 14;
pub const MAX_KEEP: u16 = 365;

const NAME_PREFIX: &str = "mascate-";
const NAME_SUFFIX: &str = ".db";
const NAME_TIME: &str = "%Y%m%d-%H%M%S%.3f";
const STAGED_SUFFIX: &str = ".restore";

/// Where Backups go and how many to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupSettings {
    pub folder: PathBuf,
    pub keep: u16,
}

/// One copy in the Backup folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backup {
    pub path: PathBuf,
    pub taken_at: Timestamp,
}

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("keep between 1 and {MAX_KEEP} backups, not {0}")]
    KeepOutOfRange(u16),
    #[error("the backup folder must be a full path, not {0}")]
    FolderNotAbsolute(PathBuf),
    #[error("the path {0} is not valid text")]
    PathNotText(PathBuf),
    #[error("could not write {path}: {source}")]
    File { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("{0} is not a Mascate database")]
    NotMascate(PathBuf),
    #[error("{path} is damaged: {detail}")]
    Damaged { path: PathBuf, detail: String },
    #[error(
        "{path} comes from a newer Mascate (module {module} at version {found}, \
         this app knows {known})"
    )]
    NewerThanApp {
        path: PathBuf,
        module: String,
        found: u32,
        known: u32,
    },
    #[error(transparent)]
    Backup(#[from] BackupError),
}

const TABLE: &str = "platform_backup_settings";
const COLUMNS: &[&str] = &["folder", "keep"];

pub(crate) const CREATE_BACKUP_SETTINGS: Migration = Migration {
    version: 4,
    name: "create backup settings",
    sql: "CREATE TABLE platform_backup_settings (
        id         TEXT PRIMARY KEY,
        folder     TEXT NOT NULL,
        keep       INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );",
};

/// `Mascate\Backups` in the user's Documents, where the owner finds it and
/// a synced Documents folder carries it off the machine; the data folder
/// when the OS reports no Documents.
pub fn default_backup_folder() -> Option<PathBuf> {
    UserDirs::new()
        .and_then(|dirs| {
            dirs.document_dir()
                .map(|documents| documents.join("Mascate").join("Backups"))
        })
        .or_else(|| {
            ProjectDirs::from("", "", "Mascate").map(|dirs| dirs.data_dir().join("backups"))
        })
}

/// Takes, lists and prunes Backups of one database, and stages restores
/// into it.
pub struct Backups {
    database: Arc<Database>,
    /// Every module's migrations, to refuse a file from a newer app.
    modules: &'static [ModuleMigrations],
    default_folder: PathBuf,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl Backups {
    pub fn new(
        database: Arc<Database>,
        modules: &'static [ModuleMigrations],
        default_folder: PathBuf,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            database,
            modules,
            default_folder,
            clock,
            ids,
        }
    }

    /// The saved settings, or the default folder and [`DEFAULT_KEEP`].
    pub async fn settings(&self) -> Result<BackupSettings, BackupError> {
        let Some(row) = single_row::load(self.database.connection(), TABLE, COLUMNS).await? else {
            return Ok(BackupSettings {
                folder: self.default_folder.clone(),
                keep: DEFAULT_KEEP,
            });
        };
        let keep = row.get::<u32>(1)?;
        Ok(BackupSettings {
            folder: PathBuf::from(row.get::<String>(0)?),
            keep: u16::try_from(keep).unwrap_or(MAX_KEEP).clamp(1, MAX_KEEP),
        })
    }

    /// Saves new settings; the next Backup prunes down to the new count.
    pub async fn save_settings(&self, settings: &BackupSettings) -> Result<(), BackupError> {
        if !(1..=MAX_KEEP).contains(&settings.keep) {
            return Err(BackupError::KeepOutOfRange(settings.keep));
        }
        if !settings.folder.is_absolute() {
            return Err(BackupError::FolderNotAbsolute(settings.folder.clone()));
        }
        single_row::save(
            self.database.connection(),
            self.clock.as_ref(),
            self.ids.as_ref(),
            TABLE,
            COLUMNS,
            vec![
                Value::Text(text(&settings.folder)?.to_owned()),
                Value::Integer(settings.keep.into()),
            ],
        )
        .await?;
        Ok(())
    }

    /// The Backups in the folder, newest first. Other files there are not
    /// Backups and are left alone.
    pub async fn list(&self) -> Result<Vec<Backup>, BackupError> {
        list_folder(&self.settings().await?.folder)
    }

    /// Copies the database into the folder now, then removes the oldest
    /// Backups beyond the count to keep.
    pub async fn back_up_now(&self) -> Result<Backup, BackupError> {
        let settings = self.settings().await?;
        create_folder(&settings.folder)?;
        let mut taken_at = self.clock.now();
        let mut path = settings.folder.join(backup_name(taken_at));
        // Two Backups in the same millisecond: the later one takes the next.
        while path.exists() {
            taken_at += TimeDelta::milliseconds(1);
            path = settings.folder.join(backup_name(taken_at));
        }
        self.copy_to(&path).await?;
        for old in list_folder(&settings.folder)?
            .into_iter()
            .skip(settings.keep.into())
        {
            // The new Backup is safe already; a file that will not go away
            // now goes on a later Backup.
            if let Err(error) = std::fs::remove_file(&old.path) {
                eprintln!(
                    "could not remove old backup {}: {error}",
                    old.path.display()
                );
            }
        }
        Ok(Backup { path, taken_at })
    }

    /// The daily Backup: taken when the folder has none from the last 24
    /// hours. A Backup dated in the future, after the clock went back,
    /// counts as recent.
    pub async fn back_up_if_due(&self) -> Result<Option<Backup>, BackupError> {
        let now = self.clock.now();
        let newest = self.list().await?.into_iter().next();
        if newest.is_some_and(|backup| backup.taken_at > now - TimeDelta::days(1)) {
            return Ok(None);
        }
        self.back_up_now().await.map(Some)
    }

    /// Writes the same copy as a Backup to `to`, to carry to another machine.
    /// It is not counted among the Backups.
    pub async fn export(&self, to: &Path) -> Result<(), BackupError> {
        if let Some(folder) = to.parent() {
            create_folder(folder)?;
        }
        self.copy_to(to).await
    }

    /// Checks that `file` is a Mascate database this app can open, stages a
    /// copy of it to replace the database on the next start, and takes a
    /// Backup of the current database. Returns that Backup. Nothing changes
    /// when the check fails.
    pub async fn stage_restore(&self, file: &Path) -> Result<Backup, RestoreError> {
        let staged = staged_restore_path(self.database.path());
        {
            // Copied before the Backup below, whose pruning may remove `file`
            // when it is the oldest Backup in the folder.
            let candidate = self.check_restore(file).await?;
            copy_database(
                &candidate.connection,
                &staged,
                &self.temporary_beside(&staged),
            )
            .await?;
        }
        self.back_up_now().await.map_err(|error| {
            // No restore without a Backup of what it replaces.
            let _ = std::fs::remove_file(&staged);
            error.into()
        })
    }

    async fn check_restore(&self, file: &Path) -> Result<ReadOnlyFile, RestoreError> {
        let not_mascate = || RestoreError::NotMascate(file.to_path_buf());
        if !file.is_file() {
            return Err(not_mascate());
        }
        let database = libsql::Builder::new_local(file)
            .flags(OpenFlags::SQLITE_OPEN_READ_ONLY)
            .build()
            .await
            .map_err(|_| not_mascate())?;
        let connection = database.connect().map_err(|_| not_mascate())?;
        // The file is input: its views and triggers run no SQL functions here.
        connection
            .execute("PRAGMA trusted_schema = OFF", ())
            .await
            .map_err(|_| not_mascate())?;

        let integrity = first_text(&connection, "PRAGMA integrity_check")
            .await
            .map_err(|error| match error {
                libsql::Error::SqliteFailure(SQLITE_CORRUPT, detail) => RestoreError::Damaged {
                    path: file.to_path_buf(),
                    detail,
                },
                _ => not_mascate(),
            })?;
        if integrity.as_deref() != Some("ok") {
            return Err(RestoreError::Damaged {
                path: file.to_path_buf(),
                detail: integrity.unwrap_or_default(),
            });
        }

        let has_versions = first_text(
            &connection,
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
        )
        .await
        .map_err(BackupError::from)?;
        if has_versions.is_none() {
            return Err(not_mascate());
        }
        let mut rows = connection
            .query(
                "SELECT module, MAX(version) FROM schema_migrations GROUP BY module",
                (),
            )
            .await
            .map_err(BackupError::from)?;
        let mut has_platform = false;
        while let Some(row) = rows.next().await.map_err(BackupError::from)? {
            let module = row.get::<String>(0).map_err(BackupError::from)?;
            let found = row.get::<u32>(1).map_err(BackupError::from)?;
            let known = self
                .modules
                .iter()
                .find(|known| known.module == module)
                .map_or(0, ModuleMigrations::latest_version);
            if found > known {
                return Err(RestoreError::NewerThanApp {
                    path: file.to_path_buf(),
                    module,
                    found,
                    known,
                });
            }
            has_platform |= module == crate::MIGRATIONS.module;
        }
        if !has_platform {
            return Err(not_mascate());
        }
        Ok(ReadOnlyFile {
            _database: database,
            connection,
        })
    }

    async fn copy_to(&self, path: &Path) -> Result<(), BackupError> {
        copy_database(
            self.database.connection(),
            path,
            &self.temporary_beside(path),
        )
        .await
    }

    /// A unique name next to `path`, so a copy appears whole under its own
    /// name or not at all.
    fn temporary_beside(&self, path: &Path) -> PathBuf {
        path.with_file_name(format!(".mascate-{}.tmp", self.ids.next_id()))
    }
}

/// Swaps a restore staged by [`Backups::stage_restore`] into place. Runs at
/// start, before the database at `database_path` opens; `false` when no
/// restore was staged.
pub fn apply_staged_restore(database_path: &Path) -> Result<bool, BackupError> {
    let staged = staged_restore_path(database_path);
    if !staged.exists() {
        return Ok(false);
    }
    // The replaced database's journal would otherwise be replayed into the
    // restored one. Its content is in the Backup taken when staging.
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = with_suffix(database_path, suffix);
        match std::fs::remove_file(&sidecar) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Err(BackupError::File {
                    path: sidecar,
                    source: error,
                });
            }
            _ => {}
        }
    }
    std::fs::rename(&staged, database_path).map_err(|source| BackupError::File {
        path: database_path.to_path_buf(),
        source,
    })?;
    Ok(true)
}

const SQLITE_CORRUPT: std::ffi::c_int = 11;

/// A file opened only to check and copy it.
struct ReadOnlyFile {
    // Keeps the underlying database alive for as long as the connection.
    _database: libsql::Database,
    connection: Connection,
}

fn staged_restore_path(database_path: &Path) -> PathBuf {
    with_suffix(database_path, STAGED_SUFFIX)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn backup_name(taken_at: Timestamp) -> String {
    format!("{NAME_PREFIX}{}Z{NAME_SUFFIX}", taken_at.format(NAME_TIME))
}

fn backup_time(name: &str) -> Option<Timestamp> {
    let stamp = name
        .strip_prefix(NAME_PREFIX)?
        .strip_suffix(NAME_SUFFIX)?
        .strip_suffix('Z')?;
    NaiveDateTime::parse_from_str(stamp, NAME_TIME)
        .ok()
        .map(|at| at.and_utc())
}

fn list_folder(folder: &Path) -> Result<Vec<Backup>, BackupError> {
    let file_error = |source| BackupError::File {
        path: folder.to_path_buf(),
        source,
    };
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(file_error(error)),
    };
    let mut backups = Vec::new();
    for entry in entries {
        let entry = entry.map_err(file_error)?;
        let taken_at = entry.file_name().to_str().and_then(backup_time);
        if let Some(taken_at) = taken_at.filter(|_| entry.path().is_file()) {
            backups.push(Backup {
                path: entry.path(),
                taken_at,
            });
        }
    }
    backups.sort_by_key(|backup| std::cmp::Reverse(backup.taken_at));
    Ok(backups)
}

fn create_folder(folder: &Path) -> Result<(), BackupError> {
    std::fs::create_dir_all(folder).map_err(|source| BackupError::File {
        path: folder.to_path_buf(),
        source,
    })
}

fn text(path: &Path) -> Result<&str, BackupError> {
    path.to_str()
        .ok_or_else(|| BackupError::PathNotText(path.to_path_buf()))
}

/// Writes a consistent copy of the database behind `connection` to `to`,
/// through `temporary` in the same folder. `VACUUM INTO` reads one snapshot,
/// so writes made meanwhile are either all in the copy or not at all.
async fn copy_database(
    connection: &Connection,
    to: &Path,
    temporary: &Path,
) -> Result<(), BackupError> {
    let copied = connection
        .execute("VACUUM INTO ?1", [text(temporary)?])
        .await;
    if let Err(error) = copied {
        let _ = std::fs::remove_file(temporary);
        return Err(error.into());
    }
    std::fs::rename(temporary, to).map_err(|source| {
        let _ = std::fs::remove_file(temporary);
        BackupError::File {
            path: to.to_path_buf(),
            source,
        }
    })
}

async fn first_text(connection: &Connection, sql: &str) -> Result<Option<String>, libsql::Error> {
    let mut rows = connection.query(sql, ()).await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row.get::<String>(0)?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn backup_names_carry_their_time_to_the_millisecond() {
        let at = Utc.with_ymd_and_hms(2026, 10, 4, 14, 5, 9).unwrap() + TimeDelta::milliseconds(42);
        let name = backup_name(at);
        assert_eq!(name, "mascate-20261004-140509.042Z.db");
        assert_eq!(backup_time(&name), Some(at));
    }

    #[test]
    fn other_files_are_not_backups() {
        for name in [
            "mascate.db",
            "mascate-20261004-140509.042Z.db.tmp",
            ".mascate-0190.tmp",
            "mascate-yesterday.db",
            "notes-20261004-140509.042Z.db",
        ] {
            assert_eq!(backup_time(name), None, "{name}");
        }
    }
}
