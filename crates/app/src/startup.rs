use std::sync::Arc;

use futures::executor::block_on;
use mascate_kernel::SystemClock;
use mascate_platform::{
    Appearance, Database, ModuleMigrations, default_database_path, load_appearance, migrate,
};

/// Every module's migrations, in dependency order. Modules add theirs here.
const MODULE_MIGRATIONS: &[ModuleMigrations] = &[mascate_platform::MIGRATIONS];

/// The ready database and the saved appearance, or a message for the home
/// screen saying why the database is not ready.
pub type Outcome = Result<Started, String>;

#[derive(Clone)]
pub struct Started {
    pub database: Arc<Database>,
    pub appearance: Appearance,
}

/// Creates the database on first run, brings its schema up to date and reads
/// the saved appearance, before any window opens so the first frame is
/// already in the owner's theme and layout.
pub fn prepare() -> Outcome {
    let path = default_database_path()
        .ok_or_else(|| "Não encontrei a pasta de dados do usuário.".to_string())?;
    block_on(async {
        let database = Database::open(&path).await?;
        migrate(&database, &SystemClock, MODULE_MIGRATIONS).await?;
        let appearance = load_appearance(&database).await?;
        Ok::<_, Box<dyn std::error::Error>>(Started {
            database: Arc::new(database),
            appearance,
        })
    })
    .map_err(|error| {
        format!(
            "Não consegui preparar o banco em {}: {error}",
            path.display()
        )
    })
}
