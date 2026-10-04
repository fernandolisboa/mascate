//! Opening the database at start (ADR 0007): a database with data at older
//! versions is backed up before its migrations run, and put back from that
//! Backup when they fail, so a failed update never leaves it half migrated.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mascate_kernel::{Clock, IdGenerator};

use crate::backups::put_back;
use crate::migrations::has_pending;
use crate::{
    Backup, BackupError, Backups, Database, DatabaseError, MigrationError, ModuleMigrations,
    migrate,
};

/// The database, migrated, and the Backup taken before its migrations ran.
pub struct Opened {
    pub database: Arc<Database>,
    pub backup_before_migrating: Option<Backup>,
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error(transparent)]
    Open(#[from] DatabaseError),
    #[error("could not read the database's versions: {0}")]
    Versions(MigrationError),
    #[error(
        "the database was left as it was, since no Backup could be taken before migrating it: {0}"
    )]
    NoBackupBeforeMigrating(BackupError),
    #[error("{error}; the database is back as it was, from the Backup {}", backup.path.display())]
    PutBack {
        error: MigrationError,
        backup: Backup,
    },
    #[error(
        "{error}; putting the database back from the Backup {} failed too: {put_back}",
        backup.path.display()
    )]
    NotPutBack {
        error: MigrationError,
        backup: Backup,
        put_back: BackupError,
    },
    /// A new database whose first migrations failed: it held nothing to
    /// put back.
    #[error(transparent)]
    Migration(MigrationError),
}

/// Opens the database at `path`, creating it on first run, and brings every
/// module's tables up to date. When it holds data at older versions, a
/// Backup goes to the owner's Backup folder (`default_backup_folder` until
/// one is chosen) first, and a failed migration puts that Backup back.
pub async fn open_and_migrate(
    path: &Path,
    modules: &'static [ModuleMigrations],
    default_backup_folder: PathBuf,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
) -> Result<Opened, OpenError> {
    let database = Arc::new(Database::open(path).await?);
    if !has_pending(&database, modules)
        .await
        .map_err(OpenError::Versions)?
    {
        migrate(&database, clock.as_ref(), modules)
            .await
            .map_err(OpenError::Migration)?;
        return Ok(Opened {
            database,
            backup_before_migrating: None,
        });
    }

    let backups = Backups::new(
        database.clone(),
        modules,
        default_backup_folder,
        clock.clone(),
        ids,
    );
    let backup = backups
        .back_up_now()
        .await
        .map_err(OpenError::NoBackupBeforeMigrating)?;
    drop(backups);
    match migrate(&database, clock.as_ref(), modules).await {
        Ok(_) => Ok(Opened {
            database,
            backup_before_migrating: Some(backup),
        }),
        Err(error) => {
            // Closed first: Windows does not replace a file that is open.
            drop(database);
            match put_back(&backup.path, path) {
                Ok(()) => Err(OpenError::PutBack { error, backup }),
                Err(put_back) => Err(OpenError::NotPutBack {
                    error,
                    backup,
                    put_back,
                }),
            }
        }
    }
}
