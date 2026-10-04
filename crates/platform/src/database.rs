use std::path::{Path, PathBuf};

use directories::{ProjectDirs, UserDirs};

#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("could not create the data folder {path}: {source}")]
    CreateFolder {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

/// The local database file (ADR 0003), opened once and shared by every module.
pub struct Database {
    path: PathBuf,
    database: libsql::Database,
    connection: libsql::Connection,
}

impl Database {
    /// Opens the database at `path`, creating the file and its folder on first run.
    pub async fn open(path: &Path) -> Result<Self, DatabaseError> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|source| DatabaseError::CreateFolder {
                path: folder.to_path_buf(),
                source,
            })?;
        }
        let database = libsql::Builder::new_local(path).build().await?;
        let connection = database.connect()?;
        // WAL lets the UI read while a Sync writes.
        connection.query("PRAGMA journal_mode = WAL", ()).await?;
        configure(&connection).await?;
        Ok(Self {
            path: path.to_path_buf(),
            database,
            connection,
        })
    }

    pub fn connection(&self) -> &libsql::Connection {
        &self.connection
    }

    /// A connection of its own to the same file.
    pub(crate) fn connect(&self) -> Result<libsql::Connection, libsql::Error> {
        self.database.connect()
    }

    /// A connection of its own, set up like the shared one, for a
    /// transaction: on the shared connection another task's statements
    /// would run inside it.
    pub async fn connect_for_transaction(&self) -> Result<libsql::Connection, libsql::Error> {
        let connection = self.connect()?;
        configure(&connection).await?;
        Ok(connection)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Foreign keys are checked per connection; the timeout absorbs short lock waits.
async fn configure(connection: &libsql::Connection) -> Result<(), libsql::Error> {
    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
        .await?;
    Ok(())
}

/// `mascate.db` in the user's data folder: `%APPDATA%\Mascate\data` on Windows,
/// `$XDG_DATA_HOME/mascate` on Linux. `None` when the OS reports no home folder.
pub fn default_database_path() -> Option<PathBuf> {
    ProjectDirs::from("", "", "Mascate").map(|dirs| dirs.data_dir().join("mascate.db"))
}

/// A folder the owner browses: `Documents/Mascate/<name>`, or `<name>` in
/// lower case in the app's data folder when the OS reports no Documents
/// folder. `None` when it reports neither.
pub fn default_owner_folder(name: &str) -> Option<PathBuf> {
    UserDirs::new()
        .and_then(|dirs| {
            dirs.document_dir()
                .map(|documents| documents.join("Mascate").join(name))
        })
        .or_else(|| {
            ProjectDirs::from("", "", "Mascate")
                .map(|dirs| dirs.data_dir().join(name.to_lowercase()))
        })
}
