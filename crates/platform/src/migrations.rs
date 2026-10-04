use libsql::params;
use mascate_kernel::Clock;

use crate::Database;

/// One versioned schema change owned by a module. Released migrations never change;
/// a fix is a new migration.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    /// Rewrites or drops data the owner has, rather than only adding to the
    /// schema. A silent update never brings one in (ADR 0007).
    pub risky: bool,
    pub sql: &'static str,
}

/// A module's migrations, in ascending version order. Each module owns its own
/// tables, so versions are counted per module.
#[derive(Debug, Clone, Copy)]
pub struct ModuleMigrations {
    pub module: &'static str,
    pub migrations: &'static [Migration],
}

impl ModuleMigrations {
    /// The version this app brings the module's tables to.
    pub fn latest_version(&self) -> u32 {
        self.migrations.last().map_or(0, |m| m.version)
    }

    /// The risky migrations among this module's.
    pub fn risky(&self) -> impl Iterator<Item = MigrationId> + '_ {
        self.migrations
            .iter()
            .filter(|migration| migration.risky)
            .map(|migration| MigrationId {
                module: self.module.to_owned(),
                version: migration.version,
            })
    }
}

/// Names one migration across app versions, as a Release declares the risky
/// ones it brings.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MigrationId {
    pub module: String,
    pub version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedMigration {
    pub module: &'static str,
    pub version: u32,
    pub name: &'static str,
}

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("migrations of module {module} must have strictly increasing versions starting at 1")]
    BadOrder { module: &'static str },
    #[error(
        "the database has module {module} at version {found}, newer than this app's {known}; \
         update the app or restore a backup"
    )]
    DatabaseNewerThanApp {
        module: &'static str,
        found: u32,
        known: u32,
    },
    #[error("migration {module} {version} ({name}) failed: {source}")]
    Failed {
        module: &'static str,
        version: u32,
        name: &'static str,
        source: libsql::Error,
    },
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

const CREATE_VERSION_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS schema_migrations (
        module     TEXT    NOT NULL,
        version    INTEGER NOT NULL,
        name       TEXT    NOT NULL,
        applied_at TEXT    NOT NULL,
        PRIMARY KEY (module, version)
    )";

/// Brings every module's tables up to date. Each migration runs in its own
/// transaction together with its version row, so a failure leaves the database
/// at the last good version. Running it again applies nothing.
pub async fn migrate(
    database: &Database,
    clock: &dyn Clock,
    modules: &[ModuleMigrations],
) -> Result<Vec<AppliedMigration>, MigrationError> {
    let connection = database.connection();
    connection.execute(CREATE_VERSION_TABLE, ()).await?;

    let mut applied = Vec::new();
    for module in modules {
        check_order(module)?;
        let current = current_version(connection, module.module).await?;
        let known = module.latest_version();
        if current > known {
            return Err(MigrationError::DatabaseNewerThanApp {
                module: module.module,
                found: current,
                known,
            });
        }
        for migration in module.migrations.iter().filter(|m| m.version > current) {
            apply(connection, clock, module.module, migration).await?;
            applied.push(AppliedMigration {
                module: module.module,
                version: migration.version,
                name: migration.name,
            });
        }
    }
    Ok(applied)
}

/// Whether the database already holds data at older versions of some
/// module's tables: false for a new database and for one that is up to date.
pub(crate) async fn has_pending(
    database: &Database,
    modules: &[ModuleMigrations],
) -> Result<bool, MigrationError> {
    let connection = database.connection();
    let mut tables = connection
        .query(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
            (),
        )
        .await?;
    if tables.next().await?.is_none() {
        return Ok(false);
    }
    let mut applied = connection
        .query("SELECT 1 FROM schema_migrations LIMIT 1", ())
        .await?;
    if applied.next().await?.is_none() {
        return Ok(false);
    }
    for module in modules {
        if current_version(connection, module.module).await? < module.latest_version() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn check_order(module: &ModuleMigrations) -> Result<(), MigrationError> {
    let in_order = module
        .migrations
        .iter()
        .zip(1u32..)
        .all(|(migration, expected)| migration.version == expected);
    if in_order {
        Ok(())
    } else {
        Err(MigrationError::BadOrder {
            module: module.module,
        })
    }
}

async fn current_version(
    connection: &libsql::Connection,
    module: &str,
) -> Result<u32, MigrationError> {
    let mut rows = connection
        .query(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations WHERE module = ?1",
            params![module],
        )
        .await?;
    let row = rows.next().await?.expect("aggregate always returns a row");
    Ok(row.get::<u32>(0)?)
}

async fn apply(
    connection: &libsql::Connection,
    clock: &dyn Clock,
    module: &'static str,
    migration: &Migration,
) -> Result<(), MigrationError> {
    let failed = |source| MigrationError::Failed {
        module,
        version: migration.version,
        name: migration.name,
        source,
    };
    let transaction = connection.transaction().await?;
    transaction
        .execute_batch(migration.sql)
        .await
        .map_err(failed)?;
    transaction
        .execute(
            "INSERT INTO schema_migrations (module, version, name, applied_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                module,
                migration.version,
                migration.name,
                clock.now().to_rfc3339()
            ],
        )
        .await?;
    transaction.commit().await?;
    Ok(())
}
