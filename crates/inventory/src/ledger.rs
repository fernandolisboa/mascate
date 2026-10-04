//! Stock Locations and the ledger of Stock Movements. Balances and the
//! Average Cost are never stored: they are read back by replaying the ledger
//! (ADR 0005).

use std::collections::BTreeMap;
use std::sync::Arc;

use libsql::{Connection, Row, params};
use mascate_kernel::{
    Clock, Currency, CurrencyMismatch, IdGenerator, Money, Record, RecordId, Timestamp,
};
use mascate_platform::{Database, Migration, StoredRow, StoredValueError, stored};
use uuid::Uuid;

use crate::Valuation;

/// The owner's own space, the first Stock Location. Its id is fixed so the
/// same place keeps one id on every machine, ready for a Merge Import.
pub const HOME_LOCATION: RecordId = Uuid::from_u128(0x01a10435_d800_7000_8000_000000000001);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockLocation {
    pub id: RecordId,
    pub name: String,
}

/// Why units moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovementReason {
    /// Units that arrived from a Purchase Order.
    PurchaseReceipt { purchase_order: RecordId },
}

impl MovementReason {
    fn code(self) -> &'static str {
        match self {
            MovementReason::PurchaseReceipt { .. } => "purchase_receipt",
        }
    }

    fn reference(self) -> RecordId {
        match self {
            MovementReason::PurchaseReceipt { purchase_order } => purchase_order,
        }
    }

    fn from_stored(code: &str, reference: RecordId) -> Option<Self> {
        match code {
            "purchase_receipt" => Some(MovementReason::PurchaseReceipt {
                purchase_order: reference,
            }),
            _ => None,
        }
    }
}

/// Units coming into a Stock Location, with what they cost in all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEntry {
    pub product: RecordId,
    pub location: RecordId,
    pub quantity: u32,
    pub cost: Money,
    pub reason: MovementReason,
}

/// One immutable line of the ledger. Entries count up; later slices add the
/// movements that count down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockMovement {
    pub id: RecordId,
    pub product: RecordId,
    pub location: RecordId,
    pub quantity: i64,
    /// What the units cost in all.
    pub cost: Money,
    pub reason: MovementReason,
    pub at: Timestamp,
}

/// A Stock Movement with the Product's stock right after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryLine {
    pub movement: StockMovement,
    pub after: Valuation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationBalance {
    pub location: RecordId,
    pub quantity: i64,
}

/// A Product's units in each Stock Location and what they are worth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductStock {
    pub product: RecordId,
    /// Every location the Product ever moved in, by location id.
    pub by_location: Vec<LocationBalance>,
    pub valuation: Valuation,
}

/// The stock of every Product that ever moved, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stock {
    pub products: Vec<ProductStock>,
}

impl Stock {
    /// What the stock on hand cost, one total per currency, in the order
    /// the currencies first appear.
    pub fn value(&self) -> Vec<Money> {
        let mut totals: Vec<Money> = Vec::new();
        for value in self.products.iter().map(|stock| stock.valuation.value()) {
            match totals
                .iter_mut()
                .find(|total| total.currency() == value.currency())
            {
                Some(total) => *total = total.checked_add(value).expect("same currency"),
                None => totals.push(value),
            }
        }
        totals
    }

    /// Units on hand, all Products together.
    pub fn units(&self) -> i64 {
        self.products
            .iter()
            .map(|stock| stock.valuation.quantity())
            .sum()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InventoryError {
    #[error("an entry needs at least one unit")]
    NoUnits,
    #[error("a cost cannot be negative")]
    NegativeCost,
    #[error("no Stock Location {0}")]
    UnknownLocation(RecordId),
    #[error("the Product's stock is valued in another currency: {0}")]
    Currencies(#[from] CurrencyMismatch),
    #[error("the ledger holds a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for InventoryError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => InventoryError::Unreadable(text),
            StoredValueError::Sql(error) => InventoryError::Sql(error),
        }
    }
}

pub(crate) const CREATE_LEDGER: Migration = Migration {
    version: 1,
    name: "create stock locations and the stock movement ledger",
    risky: false,
    sql: "CREATE TABLE inventory_stock_locations (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    INSERT INTO inventory_stock_locations (id, name, created_at, updated_at)
    VALUES ('01a10435-d800-7000-8000-000000000001', 'Meu espaço',
            '2026-10-04T00:00:00.000000Z', '2026-10-04T00:00:00.000000Z');
    CREATE TABLE inventory_stock_movements (
        id           TEXT PRIMARY KEY,
        product_id   TEXT    NOT NULL,
        location_id  TEXT    NOT NULL REFERENCES inventory_stock_locations (id),
        quantity     INTEGER NOT NULL,
        cost         TEXT    NOT NULL,
        currency     TEXT    NOT NULL,
        reason       TEXT    NOT NULL,
        reference_id TEXT    NOT NULL,
        created_at   TEXT    NOT NULL,
        updated_at   TEXT    NOT NULL,
        deleted_at   TEXT
    );
    CREATE INDEX inventory_stock_movements_by_product
        ON inventory_stock_movements (product_id, created_at);
    CREATE TRIGGER inventory_stock_movements_are_immutable_on_update
        BEFORE UPDATE ON inventory_stock_movements
        BEGIN SELECT RAISE(ABORT, 'stock movements are immutable'); END;
    CREATE TRIGGER inventory_stock_movements_are_immutable_on_delete
        BEFORE DELETE ON inventory_stock_movements
        BEGIN SELECT RAISE(ABORT, 'stock movements are immutable'); END;",
};

const MOVEMENT_COLUMNS: &str = "id, product_id, location_id, quantity, cost, currency, reason,
     reference_id, created_at
     FROM inventory_stock_movements WHERE deleted_at IS NULL";

/// The order the ledger replays in: when each movement was recorded.
const IN_LEDGER_ORDER: &str = "ORDER BY created_at, id";

/// The ledger of Stock Movements and the Stock Locations they move in.
pub struct Inventory {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl Inventory {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            database,
            clock,
            ids,
        }
    }

    fn connection(&self) -> &Connection {
        self.database.connection()
    }

    /// Every Stock Location, by name.
    pub async fn locations(&self) -> Result<Vec<StockLocation>, InventoryError> {
        let mut rows = self
            .connection()
            .query(
                "SELECT id, name FROM inventory_stock_locations WHERE deleted_at IS NULL
                 ORDER BY name",
                (),
            )
            .await?;
        let mut locations = Vec::new();
        while let Some(row) = rows.next().await? {
            locations.push(StockLocation {
                id: row.id_at(0)?,
                name: row.get(1)?,
            });
        }
        Ok(locations)
    }

    /// Appends entries to the ledger on `on`, the caller's transaction, so
    /// they commit together with what brought the units in. Nothing is
    /// written when any entry is refused.
    pub async fn record_entries(
        &self,
        on: &Connection,
        entries: &[NewEntry],
    ) -> Result<Vec<StockMovement>, InventoryError> {
        let mut currencies = BTreeMap::new();
        for entry in entries {
            if entry.quantity == 0 {
                return Err(InventoryError::NoUnits);
            }
            if entry.cost.is_negative() {
                return Err(InventoryError::NegativeCost);
            }
            known_location(on, entry.location).await?;
            let currency = match currencies.get(&entry.product) {
                Some(&currency) => Some(currency),
                None => valued_in(on, entry.product).await?,
            };
            if let Some(currency) = currency {
                Money::zero(currency).checked_add(entry.cost)?;
            }
            currencies.insert(entry.product, entry.cost.currency());
        }
        let mut movements = Vec::with_capacity(entries.len());
        for entry in entries {
            let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
            let at = stored(record.created_at);
            on.execute(
                "INSERT INTO inventory_stock_movements
                     (id, product_id, location_id, quantity, cost, currency, reason,
                      reference_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                params![
                    record.id.to_string(),
                    entry.product.to_string(),
                    entry.location.to_string(),
                    i64::from(entry.quantity),
                    entry.cost.amount().to_string(),
                    entry.cost.currency().code(),
                    entry.reason.code(),
                    entry.reason.reference().to_string(),
                    at
                ],
            )
            .await?;
            movements.push(StockMovement {
                id: record.id,
                product: entry.product,
                location: entry.location,
                quantity: i64::from(entry.quantity),
                cost: entry.cost,
                reason: entry.reason,
                at: record.created_at,
            });
        }
        Ok(movements)
    }

    /// A Product's Stock Movements, newest first, each with the stock right
    /// after it.
    pub async fn history(&self, product: RecordId) -> Result<Vec<HistoryLine>, InventoryError> {
        let movements = self
            .movements(
                &format!("SELECT {MOVEMENT_COLUMNS} AND product_id = ?1 {IN_LEDGER_ORDER}"),
                params![product.to_string()],
            )
            .await?;
        let mut valuation = None;
        let mut lines = Vec::with_capacity(movements.len());
        for movement in movements {
            let after = apply(valuation, &movement)?;
            valuation = Some(after);
            lines.push(HistoryLine { movement, after });
        }
        lines.reverse();
        Ok(lines)
    }

    /// Every Product's stock, replayed from the ledger.
    pub async fn stock(&self) -> Result<Stock, InventoryError> {
        let movements = self
            .movements(&format!("SELECT {MOVEMENT_COLUMNS} {IN_LEDGER_ORDER}"), ())
            .await?;
        let mut products: Vec<ProductStock> = Vec::new();
        for movement in movements {
            let index = match products.iter().position(|p| p.product == movement.product) {
                Some(index) => index,
                None => {
                    products.push(ProductStock {
                        product: movement.product,
                        by_location: Vec::new(),
                        valuation: Valuation::empty(movement.cost.currency()),
                    });
                    products.len() - 1
                }
            };
            let stock = &mut products[index];
            stock.valuation = apply(Some(stock.valuation), &movement)?;
            match stock
                .by_location
                .iter_mut()
                .find(|balance| balance.location == movement.location)
            {
                Some(balance) => balance.quantity += movement.quantity,
                None => stock.by_location.push(LocationBalance {
                    location: movement.location,
                    quantity: movement.quantity,
                }),
            }
        }
        for stock in &mut products {
            stock.by_location.sort_by_key(|balance| balance.location);
        }
        Ok(Stock { products })
    }

    async fn movements(
        &self,
        query: &str,
        values: impl libsql::params::IntoParams,
    ) -> Result<Vec<StockMovement>, InventoryError> {
        let mut rows = self.connection().query(query, values).await?;
        let mut movements = Vec::new();
        while let Some(row) = rows.next().await? {
            movements.push(movement_from(&row)?);
        }
        Ok(movements)
    }
}

/// The stock after `movement`, from the stock before it (`None` before the
/// Product's first movement).
fn apply(before: Option<Valuation>, movement: &StockMovement) -> Result<Valuation, InventoryError> {
    let before = before.unwrap_or_else(|| Valuation::empty(movement.cost.currency()));
    let quantity = u32::try_from(movement.quantity)
        .map_err(|_| InventoryError::Unreadable(movement.quantity.to_string()))?;
    Ok(before.enter(quantity, movement.cost)?)
}

async fn known_location(on: &Connection, location: RecordId) -> Result<(), InventoryError> {
    let mut rows = on
        .query(
            "SELECT 1 FROM inventory_stock_locations WHERE id = ?1 AND deleted_at IS NULL",
            params![location.to_string()],
        )
        .await?;
    match rows.next().await? {
        Some(_) => Ok(()),
        None => Err(InventoryError::UnknownLocation(location)),
    }
}

/// The currency a Product's stock is valued in: its first entry's.
async fn valued_in(
    on: &Connection,
    product: RecordId,
) -> Result<Option<Currency>, InventoryError> {
    let mut rows = on
        .query(
            &format!(
                "SELECT currency FROM inventory_stock_movements
                 WHERE product_id = ?1 AND deleted_at IS NULL {IN_LEDGER_ORDER} LIMIT 1"
            ),
            params![product.to_string()],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row.currency_at(0)?)),
        None => Ok(None),
    }
}

fn movement_from(row: &Row) -> Result<StockMovement, InventoryError> {
    let reason: String = row.get(6)?;
    let reference = row.id_at(7)?;
    Ok(StockMovement {
        id: row.id_at(0)?,
        product: row.id_at(1)?,
        location: row.id_at(2)?,
        quantity: row.get(3)?,
        cost: Money::new(row.decimal_at(4)?, row.currency_at(5)?),
        reason: MovementReason::from_stored(&reason, reference)
            .ok_or(InventoryError::Unreadable(reason))?,
        at: row.time_at(8)?,
    })
}
