//! Inventory: Stock Locations and the append-only ledger of Stock Movements.

mod ledger;
mod valuation;

pub use ledger::{
    HOME_LOCATION, HistoryLine, Inventory, InventoryError, LocationBalance, MovementReason,
    NewEntry, ProductStock, Stock, StockLocation, StockMovement,
};
pub use valuation::Valuation;

use mascate_platform::ModuleMigrations;

/// This module's own tables.
pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "inventory",
    migrations: &[ledger::CREATE_LEDGER],
};
