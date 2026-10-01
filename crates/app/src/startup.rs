use futures::executor::block_on;
use mascate_kernel::SystemClock;
use mascate_platform::{Database, ModuleMigrations, default_database_path, migrate};

/// Every module's migrations, in dependency order. Modules add theirs here.
const MODULE_MIGRATIONS: &[ModuleMigrations] = &[];

/// Whether the database is ready, with a message for the home screen if not.
pub type Outcome = Result<(), String>;

/// Creates the database on first run and brings its schema up to date before
/// any screen opens.
pub fn prepare_database() -> Outcome {
    let path = default_database_path()
        .ok_or_else(|| "Não encontrei a pasta de dados do usuário.".to_string())?;
    block_on(async {
        let database = Database::open(&path).await?;
        migrate(&database, &SystemClock, MODULE_MIGRATIONS).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .map_err(|error| {
        format!(
            "Não consegui preparar o banco em {}: {error}",
            path.display()
        )
    })
}
