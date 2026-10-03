//! The app platform module: where the data lives, how its schema evolves and
//! how the app's own interface looks.

mod appearance;
mod database;
mod migrations;

pub use appearance::{
    Appearance, LayoutId, ThemeFamily, ThemeMode, UiTheme, UiThemePreference, UnknownLayout,
    UnknownUiTheme, UnknownUiThemePreference, load_appearance, save_appearance,
};
pub use database::{Database, DatabaseError, default_database_path};
pub use migrations::{AppliedMigration, Migration, MigrationError, ModuleMigrations, migrate};

/// This module's own tables.
pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "platform",
    migrations: &[appearance::CREATE_APPEARANCE],
};
