use std::sync::Arc;

use futures::executor::block_on;
use mascate_kernel::{SystemClock, UuidV7Generator};
use mascate_platform::{
    Appearance, Database, Flag, Flags, ModuleMigrations, Registry, Reminder, Reminders,
    default_database_path, load_appearance, migrate,
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
}

/// Creates the database on first run, brings its schema up to date and reads
/// the saved appearance, before any window opens so the first frame is
/// already in the owner's theme and layout.
pub fn prepare() -> Outcome {
    let path = default_database_path()
        .ok_or_else(|| "Não encontrei a pasta de dados do usuário.".to_string())?;
    block_on(async {
        let database = Arc::new(Database::open(&path).await?);
        migrate(&database, &SystemClock, MODULE_MIGRATIONS).await?;
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
        Ok::<_, Box<dyn std::error::Error>>(Started {
            database,
            appearance,
            flags: Arc::new(flags),
            reminders: Arc::new(reminders),
        })
    })
    .map_err(|error| {
        format!(
            "Não consegui preparar o banco em {}: {error}",
            path.display()
        )
    })
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
