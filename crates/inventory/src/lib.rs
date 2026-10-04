//! Inventory: Stock Locations, the append-only ledger of Stock Movements
//! and each Product's Reorder Point.

mod ledger;
mod valuation;

pub use ledger::{
    Adjusted, AdjustmentKind, Exited, HOME_LOCATION, HistoryLine, Inventory, InventoryError,
    LocationBalance, LowStock, MovementReason, NewEntry, NewExit, NewReturn, ProductStock, Stock,
    StockAdjustment, StockLocation, StockMovement,
};
pub use valuation::Valuation;

use mascate_platform::ModuleMigrations;

/// This module's own tables.
pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "inventory",
    migrations: &[
        ledger::CREATE_LEDGER,
        ledger::ADD_ADJUSTMENTS_AND_REORDER_POINTS,
    ],
};
