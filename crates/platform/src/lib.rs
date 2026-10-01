//! The app platform module: where the data lives and how its schema evolves.

mod database;
mod migrations;

pub use database::{Database, DatabaseError, default_database_path};
pub use migrations::{AppliedMigration, Migration, MigrationError, ModuleMigrations, migrate};
