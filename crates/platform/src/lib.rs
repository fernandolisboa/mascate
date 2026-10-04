//! The app platform module: where the data lives, how its schema evolves,
//! where secrets are kept, how the app's own interface looks, and the flags
//! and Reminders every module declares.

mod appearance;
mod database;
mod flags;
mod migrations;
mod registry;
mod reminders;
mod secrets;
mod stored_time;
mod system_secrets;

#[cfg(feature = "test-support")]
pub mod testing;

pub use appearance::{
    Appearance, LayoutId, ThemeFamily, ThemeMode, UiTheme, UiThemePreference, UnknownLayout,
    UnknownUiTheme, UnknownUiThemePreference, load_appearance, save_appearance,
};
pub use database::{Database, DatabaseError, default_database_path};
pub use flags::{
    ConfirmedTurnOn, Flag, FlagChange, FlagError, FlagKind, FlagStatus, Flags, Phase,
    TurnOnRequest, system_user,
};
pub use migrations::{AppliedMigration, Migration, MigrationError, ModuleMigrations, migrate};
pub use registry::{DuplicateKey, Registered, Registry};
pub use reminders::{Reminder, ReminderError, ReminderTopic, Reminders};
pub use secrets::{
    Build, Environment, Secret, SecretStore, SecretStoreError, process_environment,
    secret_store_for,
};
pub use system_secrets::SystemSecretStore;

/// This module's own tables.
pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "platform",
    migrations: &[
        appearance::CREATE_APPEARANCE,
        flags::CREATE_FLAG_CHANGES,
        reminders::CREATE_REMINDER_DISMISSALS,
    ],
};
