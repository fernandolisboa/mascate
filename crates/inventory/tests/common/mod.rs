//! An Inventory on a real temporary database, with a clock and ids the test
//! controls. Each test file uses part of it.
#![allow(dead_code)]

use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use mascate_inventory::{
    Adjusted, HOME_LOCATION, Inventory, InventoryError, MIGRATIONS, MovementReason, NewEntry,
    StockAdjustment,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, RecordId};
use mascate_platform::{Database, migrate};
use rust_decimal::Decimal;
use uuid::Uuid;

pub struct Fixture {
    _dir: tempfile::TempDir,
    pub clock: Arc<ManualClock>,
    pub database: Arc<Database>,
    pub inventory: Inventory,
}

impl Fixture {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
        ));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        let inventory = Inventory::new(
            database.clone(),
            clock.clone(),
            Arc::new(SequentialIds::default()),
        );
        Self {
            _dir: dir,
            clock,
            database,
            inventory,
        }
    }

    /// Records entries on a transaction of their own, as a caller would.
    pub async fn enter(&self, entries: &[NewEntry]) -> Result<(), InventoryError> {
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection.transaction().await?;
        self.inventory.record_entries(&transaction, entries).await?;
        transaction.commit().await?;
        self.clock.advance(TimeDelta::minutes(1));
        Ok(())
    }

    /// Adjusts a Product's stock in the owner's space.
    pub async fn adjust(
        &self,
        product: u128,
        adjustment: StockAdjustment,
    ) -> Result<Adjusted, InventoryError> {
        let adjusted = self
            .inventory
            .adjust(id(product), HOME_LOCATION, adjustment, None)
            .await;
        self.clock.advance(TimeDelta::minutes(1));
        adjusted
    }

    /// Units of a Product on hand, all locations together.
    pub async fn units(&self, product: u128) -> i64 {
        self.inventory
            .stock()
            .await
            .unwrap()
            .products
            .iter()
            .find(|stock| stock.product == id(product))
            .map_or(0, |stock| stock.valuation.quantity())
    }

    pub async fn average_cost(&self, product: u128) -> Option<Money> {
        self.inventory
            .stock()
            .await
            .unwrap()
            .products
            .iter()
            .find(|stock| stock.product == id(product))
            .and_then(|stock| stock.valuation.average_cost())
    }
}

pub fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

pub fn id(n: u128) -> RecordId {
    Uuid::from_u128(1_000_000 + n)
}

pub const FONE: u128 = 1;
pub const CAPA: u128 = 2;
pub const ORDER: u128 = 100;

pub fn entry(product: u128, quantity: u32, cost: &str) -> NewEntry {
    NewEntry {
        product: id(product),
        location: HOME_LOCATION,
        quantity,
        cost: brl(cost),
        reason: MovementReason::PurchaseReceipt {
            purchase_order: id(ORDER),
        },
    }
}
