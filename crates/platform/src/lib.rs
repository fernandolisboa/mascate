//! The app platform module: where the data lives, how its schema evolves,
//! where secrets are kept and how the app's own interface looks.

mod appearance;
mod database;
mod migrations;
mod secrets;
mod system_secrets;

#[cfg(feature = "test-support")]
pub mod testing;

pub use appearance::{
    Appearance, LayoutId, ThemeFamily, ThemeMode, UiTheme, UiThemePreference, UnknownLayout,
    UnknownUiTheme, UnknownUiThemePreference, load_appearance, save_appearance,
};
pub use database::{Database, DatabaseError, default_database_path};
pub use migrations::{AppliedMigration, Migration, MigrationError, ModuleMigrations, migrate};
pub use secrets::{
    Build, Environment, Secret, SecretStore, SecretStoreError, process_environment,
    secret_store_for,
};
pub use system_secrets::SystemSecretStore;

/// This module's own tables.
pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "platform",
    migrations: &[appearance::CREATE_APPEARANCE],
};
