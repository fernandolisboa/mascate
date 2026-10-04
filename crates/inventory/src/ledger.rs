//! Stock Locations and the ledger of Stock Movements. Balances and the
//! Average Cost are never stored: they are read back by replaying the ledger
//! (ADR 0005).

use std::collections::BTreeMap;
use std::sync::Arc;

use libsql::{Connection, Row, TransactionBehavior, params};
use mascate_kernel::{
    Clock, Currency, CurrencyMismatch, IdGenerator, Money, Record, RecordId, Timestamp,
};
use mascate_platform::{Database, Migration, StoredRow, StoredValueError, stored};
use rust_decimal::Decimal;
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
    /// A Stock Adjustment: units the owner corrected by hand.
    Adjustment(AdjustmentKind),
    /// Units sold in an Order of a Sales Channel.
    Sale { order: RecordId },
    /// Units of a sale back on the shelf, its Order cancelled before they
    /// shipped.
    SaleCancelled { order: RecordId },
    /// Units of a sale the buyer sent back, received by the owner.
    SaleReturned { order: RecordId },
}

/// Why the owner corrected the stock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjustmentKind {
    /// Units gone: lost, stolen, never found.
    Loss,
    /// Units damaged beyond selling.
    Damage,
    /// A physical count that differed from the ledger.
    Count,
}

impl MovementReason {
    fn code(self) -> &'static str {
        match self {
            MovementReason::PurchaseReceipt { .. } => "purchase_receipt",
            MovementReason::Adjustment(AdjustmentKind::Loss) => "adjustment_loss",
            MovementReason::Adjustment(AdjustmentKind::Damage) => "adjustment_damage",
            MovementReason::Adjustment(AdjustmentKind::Count) => "adjustment_count",
            MovementReason::Sale { .. } => "sale",
            MovementReason::SaleCancelled { .. } => "sale_cancelled",
            MovementReason::SaleReturned { .. } => "sale_returned",
        }
    }

    /// The record that brought the units, when one outside the ledger did.
    fn reference(self) -> Option<RecordId> {
        match self {
            MovementReason::PurchaseReceipt { purchase_order } => Some(purchase_order),
            MovementReason::Sale { order }
            | MovementReason::SaleCancelled { order }
            | MovementReason::SaleReturned { order } => Some(order),
            MovementReason::Adjustment(_) => None,
        }
    }

    fn from_stored(code: &str, reference: RecordId) -> Option<Self> {
        match code {
            "purchase_receipt" => Some(MovementReason::PurchaseReceipt {
                purchase_order: reference,
            }),
            "adjustment_loss" => Some(MovementReason::Adjustment(AdjustmentKind::Loss)),
            "adjustment_damage" => Some(MovementReason::Adjustment(AdjustmentKind::Damage)),
            "adjustment_count" => Some(MovementReason::Adjustment(AdjustmentKind::Count)),
            "sale" => Some(MovementReason::Sale { order: reference }),
            "sale_cancelled" => Some(MovementReason::SaleCancelled { order: reference }),
            "sale_returned" => Some(MovementReason::SaleReturned { order: reference }),
            _ => None,
        }
    }
}

/// A Stock Adjustment the owner asks for, in one Stock Location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockAdjustment {
    /// This many units were lost.
    Loss(u32),
    /// This many units were damaged beyond selling.
    Damage(u32),
    /// The units physically counted there; the ledger moves by the
    /// difference.
    Count(u32),
}

impl StockAdjustment {
    fn kind(self) -> AdjustmentKind {
        match self {
            StockAdjustment::Loss(_) => AdjustmentKind::Loss,
            StockAdjustment::Damage(_) => AdjustmentKind::Damage,
            StockAdjustment::Count(_) => AdjustmentKind::Count,
        }
    }
}

/// What a Stock Adjustment did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adjusted {
    /// `None` when a count matched the ledger: nothing to correct.
    pub movement: Option<StockMovement>,
    /// Set when this adjustment took the Product from above its Reorder
    /// Point to it or below.
    pub reached_reorder_point: Option<LowStock>,
}

/// Units leaving a Stock Location for a record outside the ledger, such as
/// an Order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewExit {
    pub product: RecordId,
    pub location: RecordId,
    pub quantity: u32,
    pub reason: MovementReason,
}

/// What a Stock Movement out did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exited {
    pub movement: StockMovement,
    /// Set when the units leaving took the Product from above its Reorder
    /// Point to it or below.
    pub reached_reorder_point: Option<LowStock>,
}

/// Units that left by `exit` coming back to where they left from, such as
/// those of a cancelled Order or of a return the owner received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewReturn {
    /// The Stock Movement the units left by.
    pub exit: RecordId,
    pub quantity: u32,
    pub reason: MovementReason,
}

/// A Product at or below its Reorder Point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LowStock {
    pub product: RecordId,
    /// Units on hand, all Stock Locations together.
    pub quantity: i64,
    pub reorder_point: u32,
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

/// One immutable line of the ledger. Entries count up; adjustments count
/// either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockMovement {
    pub id: RecordId,
    pub product: RecordId,
    pub location: RecordId,
    pub quantity: i64,
    /// What the units cost in all; negative for units that left, which
    /// leave at the Average Cost.
    pub cost: Money,
    pub reason: MovementReason,
    /// What the owner wrote about an adjustment.
    pub note: Option<String>,
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
    pub reorder_point: Option<u32>,
}

impl ProductStock {
    /// At or below its Reorder Point.
    pub fn is_low(&self) -> bool {
        self.reorder_point
            .is_some_and(|point| self.valuation.quantity() <= i64::from(point))
    }
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
    #[error("an entry or adjustment needs at least one unit")]
    NoUnits,
    #[error("only {on_hand} units are on hand there")]
    NotEnoughStock { on_hand: i64 },
    #[error("the Product never came into stock, so the units found have no cost")]
    NoCostBasis,
    #[error("a cost cannot be negative")]
    NegativeCost,
    #[error("no Stock Movement {0} took units out")]
    UnknownExit(RecordId),
    #[error("only {left} units left by that movement")]
    ReturnsMoreThanLeft { left: u32 },
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

pub(crate) const ADD_ADJUSTMENTS_AND_REORDER_POINTS: Migration = Migration {
    version: 2,
    name: "add adjustment notes and reorder points",
    risky: false,
    sql: "ALTER TABLE inventory_stock_movements ADD COLUMN note TEXT;
    CREATE TABLE inventory_reorder_points (
        id         TEXT    PRIMARY KEY,
        product_id TEXT    NOT NULL UNIQUE,
        quantity   INTEGER NOT NULL CHECK (quantity >= 0),
        created_at TEXT    NOT NULL,
        updated_at TEXT    NOT NULL,
        deleted_at TEXT
    );",
};

const MOVEMENT_COLUMNS: &str = "id, product_id, location_id, quantity, cost, currency, reason,
     reference_id, created_at, note
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
            let movement = self.movement(
                entry.product,
                entry.location,
                i64::from(entry.quantity),
                entry.cost,
                entry.reason,
                None,
            );
            insert(on, &movement).await?;
            movements.push(movement);
        }
        Ok(movements)
    }

    /// Records a Stock Adjustment of `product` in `location`. Units leave and
    /// come back at the Average Cost, so an adjustment never changes it, and
    /// the balance there never drops below zero. A count that matches the
    /// ledger records nothing.
    pub async fn adjust(
        &self,
        product: RecordId,
        location: RecordId,
        adjustment: StockAdjustment,
        note: Option<&str>,
    ) -> Result<Adjusted, InventoryError> {
        let connection = self.database.connect_for_transaction().await?;
        // Immediate: the balance it checks still holds when it writes.
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        known_location(&transaction, location).await?;
        let on_hand = balance_in(&transaction, product, location).await?;
        let change = match adjustment {
            StockAdjustment::Loss(0) | StockAdjustment::Damage(0) => {
                return Err(InventoryError::NoUnits);
            }
            StockAdjustment::Loss(units) | StockAdjustment::Damage(units) => -i64::from(units),
            StockAdjustment::Count(counted) => i64::from(counted) - on_hand,
        };
        if change == 0 {
            return Ok(Adjusted {
                movement: None,
                reached_reorder_point: None,
            });
        }
        let note = note.map(str::trim).filter(|note| !note.is_empty());
        let moved = self
            .change(
                &transaction,
                Change {
                    product,
                    location,
                    on_hand,
                    by: change,
                    reason: MovementReason::Adjustment(adjustment.kind()),
                    note: note.map(str::to_owned),
                },
            )
            .await?;
        transaction.commit().await?;
        Ok(Adjusted {
            movement: Some(moved.movement),
            reached_reorder_point: moved.reached_reorder_point,
        })
    }

    /// Takes units out of the ledger on `on`, the caller's transaction, so
    /// they commit together with what took them, such as an Order. They
    /// leave at the Average Cost, and the balance there never drops below
    /// zero: with fewer units on hand nothing is written.
    pub async fn record_exit(
        &self,
        on: &Connection,
        exit: &NewExit,
    ) -> Result<Exited, InventoryError> {
        if exit.quantity == 0 {
            return Err(InventoryError::NoUnits);
        }
        known_location(on, exit.location).await?;
        let on_hand = balance_in(on, exit.product, exit.location).await?;
        self.change(
            on,
            Change {
                product: exit.product,
                location: exit.location,
                on_hand,
                by: -i64::from(exit.quantity),
                reason: exit.reason,
                note: None,
            },
        )
        .await
    }

    /// Puts back units that left by an exit, on `on`, the caller's
    /// transaction, so they commit together with what brought them back.
    /// They come back to the location they left and at the cost they left
    /// with, so the ledger reads as if they never left. The caller keeps
    /// track of what already came back: the ledger only refuses more units
    /// than the exit took.
    pub async fn record_return(
        &self,
        on: &Connection,
        back: &NewReturn,
    ) -> Result<StockMovement, InventoryError> {
        if back.quantity == 0 {
            return Err(InventoryError::NoUnits);
        }
        let exit = movements(
            on,
            &format!("SELECT {MOVEMENT_COLUMNS} AND id = ?1"),
            params![back.exit.to_string()],
        )
        .await?
        .pop()
        .filter(|exit| exit.quantity < 0)
        .ok_or(InventoryError::UnknownExit(back.exit))?;
        let left = u32::try_from(exit.quantity.unsigned_abs())
            .map_err(|_| InventoryError::Unreadable(exit.quantity.to_string()))?;
        if back.quantity > left {
            return Err(InventoryError::ReturnsMoreThanLeft { left });
        }
        let cost = Money::new(
            -exit.cost.amount() * Decimal::from(back.quantity) / Decimal::from(left),
            exit.cost.currency(),
        );
        let movement = self.movement(
            exit.product,
            exit.location,
            i64::from(back.quantity),
            cost,
            back.reason,
            None,
        );
        insert(on, &movement).await?;
        Ok(movement)
    }

    /// Sets the Reorder Point of `product`, or clears it with `None`.
    pub async fn set_reorder_point(
        &self,
        product: RecordId,
        point: Option<u32>,
    ) -> Result<(), InventoryError> {
        let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
        let at = stored(record.created_at);
        match point {
            Some(point) => {
                self.connection()
                    .execute(
                        "INSERT INTO inventory_reorder_points
                             (id, product_id, quantity, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?4)
                         ON CONFLICT (product_id) DO UPDATE SET
                             quantity = excluded.quantity,
                             updated_at = excluded.updated_at,
                             deleted_at = NULL",
                        params![
                            record.id.to_string(),
                            product.to_string(),
                            i64::from(point),
                            at
                        ],
                    )
                    .await?;
            }
            None => {
                self.connection()
                    .execute(
                        "UPDATE inventory_reorder_points SET deleted_at = ?1, updated_at = ?1
                         WHERE product_id = ?2 AND deleted_at IS NULL",
                        params![at, product.to_string()],
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// Every Product at or below its Reorder Point, the furthest below
    /// first. A Product that never moved counts as none on hand.
    pub async fn low_stock(&self) -> Result<Vec<LowStock>, InventoryError> {
        let stock = self.stock().await?;
        let mut low: Vec<LowStock> = reorder_points(self.connection())
            .await?
            .into_iter()
            .filter_map(|(product, reorder_point)| {
                let quantity = stock
                    .products
                    .iter()
                    .find(|stock| stock.product == product)
                    .map_or(0, |stock| stock.valuation.quantity());
                (quantity <= i64::from(reorder_point)).then_some(LowStock {
                    product,
                    quantity,
                    reorder_point,
                })
            })
            .collect();
        low.sort_by_key(|low| (low.quantity - i64::from(low.reorder_point), low.product));
        Ok(low)
    }

    /// A Product's Stock Movements, newest first, each with the stock right
    /// after it.
    pub async fn history(&self, product: RecordId) -> Result<Vec<HistoryLine>, InventoryError> {
        let mut lines = replay(self.connection(), product).await?.lines;
        lines.reverse();
        Ok(lines)
    }

    /// What one unit of `product` cost: the Average Cost of the units on
    /// hand or, with none left, of the last ones there were. `None` when the
    /// Product never came into stock.
    pub async fn average_cost(&self, product: RecordId) -> Result<Option<Money>, InventoryError> {
        Ok(replay(self.connection(), product).await?.basis)
    }

    /// Every Product's stock, replayed from the ledger.
    pub async fn stock(&self) -> Result<Stock, InventoryError> {
        let movements = movements(
            self.connection(),
            &format!("SELECT {MOVEMENT_COLUMNS} {IN_LEDGER_ORDER}"),
            (),
        )
        .await?;
        let points = reorder_points(self.connection()).await?;
        let mut products: Vec<ProductStock> = Vec::new();
        for movement in movements {
            let index = match products.iter().position(|p| p.product == movement.product) {
                Some(index) => index,
                None => {
                    products.push(ProductStock {
                        product: movement.product,
                        by_location: Vec::new(),
                        valuation: Valuation::empty(movement.cost.currency()),
                        reorder_point: points.get(&movement.product).copied(),
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

    /// Records `change` of a Product's units in one location: units leave
    /// at the Average Cost and come back at the cost of the last units on
    /// hand, so the Average Cost never moves. Also says whether the change
    /// took the Product to its Reorder Point.
    async fn change(&self, on: &Connection, change: Change) -> Result<Exited, InventoryError> {
        let Change {
            product,
            location,
            on_hand,
            by,
            reason,
            note,
        } = change;
        if on_hand + by < 0 {
            return Err(InventoryError::NotEnoughStock { on_hand });
        }
        let replay = replay(on, product).await?;
        let units = u32::try_from(by.unsigned_abs())
            .map_err(|_| InventoryError::Unreadable(by.to_string()))?;
        let cost = if by < 0 {
            replay
                .valuation
                .and_then(|valuation| valuation.exit_cost(units))
                .ok_or(InventoryError::NotEnoughStock { on_hand })?
        } else {
            replay
                .basis
                .ok_or(InventoryError::NoCostBasis)?
                .times(Decimal::from(units))
        };
        let movement = self.movement(product, location, by, cost, reason, note);
        insert(on, &movement).await?;
        let before = replay.valuation.map_or(0, |valuation| valuation.quantity());
        let reached_reorder_point = match reorder_point_of(on, product).await? {
            Some(point) if before > i64::from(point) && before + by <= i64::from(point) => {
                Some(LowStock {
                    product,
                    quantity: before + by,
                    reorder_point: point,
                })
            }
            _ => None,
        };
        Ok(Exited {
            movement,
            reached_reorder_point,
        })
    }

    /// A new movement, dated now.
    fn movement(
        &self,
        product: RecordId,
        location: RecordId,
        quantity: i64,
        cost: Money,
        reason: MovementReason,
        note: Option<String>,
    ) -> StockMovement {
        let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
        StockMovement {
            id: record.id,
            product,
            location,
            quantity,
            cost,
            reason,
            note,
            at: record.created_at,
        }
    }
}

/// Units of a Product to add (or, negative, take) in one location.
struct Change {
    product: RecordId,
    location: RecordId,
    /// Units there now.
    on_hand: i64,
    by: i64,
    reason: MovementReason,
    note: Option<String>,
}

/// A Product's ledger replayed in order.
struct Replay {
    lines: Vec<HistoryLine>,
    /// The stock now; `None` before the first movement.
    valuation: Option<Valuation>,
    /// The Average Cost of the last moment with units on hand: what units
    /// found by a count cost.
    basis: Option<Money>,
}

async fn replay(on: &Connection, product: RecordId) -> Result<Replay, InventoryError> {
    let movements = movements(
        on,
        &format!("SELECT {MOVEMENT_COLUMNS} AND product_id = ?1 {IN_LEDGER_ORDER}"),
        params![product.to_string()],
    )
    .await?;
    let mut replay = Replay {
        lines: Vec::with_capacity(movements.len()),
        valuation: None,
        basis: None,
    };
    for movement in movements {
        let after = apply(replay.valuation, &movement)?;
        replay.valuation = Some(after);
        replay.basis = after.average_cost().or(replay.basis);
        replay.lines.push(HistoryLine { movement, after });
    }
    Ok(replay)
}

/// The stock after `movement`, from the stock before it (`None` before the
/// Product's first movement).
fn apply(before: Option<Valuation>, movement: &StockMovement) -> Result<Valuation, InventoryError> {
    let before = before.unwrap_or_else(|| Valuation::empty(movement.cost.currency()));
    Ok(before.moved(movement.quantity, movement.cost)?)
}

async fn insert(on: &Connection, movement: &StockMovement) -> Result<(), InventoryError> {
    let at = stored(movement.at);
    // An adjustment answers to nothing outside the ledger: it names itself.
    let reference = movement.reason.reference().unwrap_or(movement.id);
    on.execute(
        "INSERT INTO inventory_stock_movements
             (id, product_id, location_id, quantity, cost, currency, reason,
              reference_id, note, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        params![
            movement.id.to_string(),
            movement.product.to_string(),
            movement.location.to_string(),
            movement.quantity,
            movement.cost.amount().to_string(),
            movement.cost.currency().code(),
            movement.reason.code(),
            reference.to_string(),
            movement.note.clone(),
            at
        ],
    )
    .await?;
    Ok(())
}

async fn movements(
    on: &Connection,
    query: &str,
    values: impl libsql::params::IntoParams,
) -> Result<Vec<StockMovement>, InventoryError> {
    let mut rows = on.query(query, values).await?;
    let mut movements = Vec::new();
    while let Some(row) = rows.next().await? {
        movements.push(movement_from(&row)?);
    }
    Ok(movements)
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

/// Units of `product` on hand in `location`.
async fn balance_in(
    on: &Connection,
    product: RecordId,
    location: RecordId,
) -> Result<i64, InventoryError> {
    let mut rows = on
        .query(
            "SELECT COALESCE(SUM(quantity), 0) FROM inventory_stock_movements
             WHERE product_id = ?1 AND location_id = ?2 AND deleted_at IS NULL",
            params![product.to_string(), location.to_string()],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(row.get(0)?),
        None => Ok(0),
    }
}

/// The currency a Product's stock is valued in: its first entry's.
async fn valued_in(on: &Connection, product: RecordId) -> Result<Option<Currency>, InventoryError> {
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

async fn reorder_point_of(
    on: &Connection,
    product: RecordId,
) -> Result<Option<u32>, InventoryError> {
    let mut rows = on
        .query(
            "SELECT quantity FROM inventory_reorder_points
             WHERE product_id = ?1 AND deleted_at IS NULL",
            params![product.to_string()],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(point_from(row.get(0)?)?)),
        None => Ok(None),
    }
}

async fn reorder_points(on: &Connection) -> Result<BTreeMap<RecordId, u32>, InventoryError> {
    let mut rows = on
        .query(
            "SELECT product_id, quantity FROM inventory_reorder_points WHERE deleted_at IS NULL",
            (),
        )
        .await?;
    let mut points = BTreeMap::new();
    while let Some(row) = rows.next().await? {
        points.insert(row.id_at(0)?, point_from(row.get(1)?)?);
    }
    Ok(points)
}

fn point_from(stored: i64) -> Result<u32, InventoryError> {
    u32::try_from(stored).map_err(|_| InventoryError::Unreadable(stored.to_string()))
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
        note: row.get(9)?,
    })
}
