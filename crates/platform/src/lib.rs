//! The app platform module: where the data lives, how its schema evolves,
//! how it is backed up and restored, how the app updates itself, where
//! secrets are kept, how the app's own interface looks, and the flags and
//! Reminders every module declares.

mod appearance;
mod backups;
mod database;
mod flags;
mod migrations;
mod opening;
mod registry;
mod releases;
mod reminders;
mod secrets;
mod single_row;
mod stored_row;
mod stored_time;
mod system_secrets;
mod updates;

#[cfg(feature = "test-support")]
pub mod testing;

pub use appearance::{
    Appearance, LayoutId, ThemeFamily, ThemeMode, UiTheme, UiThemePreference, UnknownLayout,
    UnknownUiTheme, UnknownUiThemePreference, load_appearance, save_appearance,
};
pub use backups::{
    Backup, BackupError, BackupSettings, Backups, DEFAULT_KEEP, MAX_KEEP, RestoreError,
    apply_staged_restore, default_backup_folder, stage_restore_without_backup,
    undo_applied_restore,
};
pub use database::{Database, DatabaseError, default_database_path, default_owner_folder};
pub use flags::{
    ConfirmedTurnOn, Flag, FlagChange, FlagError, FlagKind, FlagStatus, Flags, Phase,
    TurnOnRequest, system_user,
};
pub use migrations::{
    AppliedMigration, Migration, MigrationError, MigrationId, ModuleMigrations, migrate,
};
pub use opening::{OpenError, Opened, open_and_migrate};
pub use registry::{DuplicateKey, Registered, Registry};
pub use releases::{
    Installer, MANIFEST_NAME, RELEASES_PAGE, Release, ReleaseChannel, ReleaseError, Target, Version,
};
pub use reminders::{Reminder, ReminderError, ReminderTopic, Reminders};
pub use secrets::{
    Build, Environment, Secret, SecretStore, SecretStoreError, process_environment,
    secret_store_for,
};
pub use stored_row::{StoredRow, StoredValueError};
pub use stored_time::{read_stored, stored};
pub use system_secrets::SystemSecretStore;
pub use updates::{
    Finish, Installation, UpdateError, UpdateSettings, Updater, load_update_settings,
    run_installer_after_exit, save_update_settings,
};

/// This module's own tables.
pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "platform",
    migrations: &[
        appearance::CREATE_APPEARANCE,
        flags::CREATE_FLAG_CHANGES,
        reminders::CREATE_REMINDER_DISMISSALS,
        backups::CREATE_BACKUP_SETTINGS,
        updates::CREATE_UPDATE_SETTINGS,
    ],
};
