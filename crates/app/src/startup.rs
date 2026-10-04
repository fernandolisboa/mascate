use std::path::Path;
use std::sync::Arc;

use futures::executor::block_on;
use mascate_kernel::{SystemClock, UuidV7Generator};
use mascate_platform::{
    Appearance, BackupSettings, Backups, Database, Flag, Flags, ModuleMigrations, Registry,
    Reminder, Reminders, apply_staged_restore, default_backup_folder, default_database_path,
    load_appearance, migrate, undo_applied_restore,
};

/// Every module's migrations, in dependency order. Modules add theirs here.
const MODULE_MIGRATIONS: &[ModuleMigrations] = &[mascate_platform::MIGRATIONS];

/// Every module's flags, in the order the settings screen lists them.
const MODULE_FLAGS: &[&[Flag]] = &[mascate_marketing::FLAGS];

/// Every module's Reminders, in the order the home screen shows them.
const MODULE_REMINDERS: &[&[Reminder]] = &[mascate_finance::REMINDERS, mascate_commerce::REMINDERS];

/// The ready database and what is read from it before the first window, or
/// a message for the home screen saying why the database is not ready.
pub type Outcome = Result<Started, String>;

#[derive(Clone)]
pub struct Started {
    pub database: Arc<Database>,
    pub appearance: Appearance,
    pub flags: Arc<Flags>,
    pub reminders: Arc<Reminders>,
    pub backups: Arc<Backups>,
    pub backup_settings: BackupSettings,
    /// What became of a restore staged before the restart, if one was.
    pub restore: Option<Result<(), String>>,
}

/// Swaps in a staged restore, creates the database on first run, brings its
/// schema up to date (a restored file from an older version included) and
/// reads the saved appearance, before any window opens so the first frame
/// is already in the owner's theme and layout.
pub fn prepare() -> Outcome {
    let path = default_database_path()
        .ok_or_else(|| "Não encontrei a pasta de dados do usuário.".to_string())?;
    let mut restore = match apply_staged_restore(&path) {
        Ok(true) => Some(Ok(())),
        Ok(false) => None,
        // The swap puts the current database back when it fails.
        Err(error) => Some(Err(format!(
            "Não consegui restaurar o Backup escolhido; o banco continua o de antes. \
             Tente restaurar de novo: {error}"
        ))),
    };
    let backup_folder = default_backup_folder()
        .or_else(|| path.parent().map(|data| data.join("backups")))
        .ok_or_else(|| "Não encontrei uma pasta para os Backups.".to_string())?;
    block_on(async {
        let database = match open_migrated(&path).await {
            Err(error) if matches!(restore, Some(Ok(()))) => {
                undo_applied_restore(&path)?;
                restore = Some(Err(format!(
                    "O arquivo restaurado não abriu nesta versão do app, então o banco de \
                     antes voltou: {error}"
                )));
                open_migrated(&path).await?
            }
            opened => opened?,
        };
        let appearance = load_appearance(&database).await.unwrap_or_else(|error| {
            // A look that cannot be read is no reason to give up the database.
            eprintln!("could not read the saved appearance: {error}");
            Appearance::default()
        });
        let flags = Flags::new(
            database.clone(),
            Registry::new(MODULE_FLAGS)?,
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
        );
        let reminders = Reminders::new(
            database.clone(),
            Registry::new(MODULE_REMINDERS)?,
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
        );
        let backups = Backups::new(
            database.clone(),
            MODULE_MIGRATIONS,
            backup_folder,
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
        );
        let backup_settings = backups.settings().await?;
        Ok::<_, Box<dyn std::error::Error>>(Started {
            database,
            appearance,
            flags: Arc::new(flags),
            reminders: Arc::new(reminders),
            backups: Arc::new(backups),
            backup_settings,
            restore,
        })
    })
    .map_err(|error| {
        format!(
            "Não consegui preparar o banco em {}: {error}",
            path.display()
        )
    })
}

async fn open_migrated(path: &Path) -> Result<Arc<Database>, Box<dyn std::error::Error>> {
    let database = Database::open(path).await?;
    migrate(&database, &SystemClock, MODULE_MIGRATIONS).await?;
    Ok(Arc::new(database))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_module_declares_flags_and_reminders_under_distinct_keys() {
        assert!(Registry::new(MODULE_FLAGS).is_ok());
        assert!(Registry::new(MODULE_REMINDERS).is_ok());
    }
}
