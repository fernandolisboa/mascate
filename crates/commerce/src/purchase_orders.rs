//! Purchase Orders to Suppliers, and receiving them into stock.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::NaiveDate;
use libsql::{Connection, TransactionBehavior, params};
use mascate_inventory::{Inventory, InventoryError, MovementReason, NewEntry};
use mascate_kernel::{
    Clock, Currency, CurrencyMismatch, IdGenerator, Money, Record, RecordId, Timestamp,
};
use mascate_platform::{Database, Migration, StoredRow, StoredValueError, stored};
use rust_decimal::Decimal;

use crate::freight::{freight_for_units, split_by_value};

/// What the owner types to register or edit a Purchase Order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPurchaseOrder {
    pub supplier: RecordId,
    pub ordered_on: NaiveDate,
    /// What the Supplier charged to deliver the whole order.
    pub freight: Money,
    pub lines: Vec<NewPurchaseLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPurchaseLine {
    pub product: RecordId,
    pub quantity: u32,
    pub unit_price: Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurchaseOrderStatus {
    Purchased,
    Shipped,
    /// Some units came in; the rest are still expected.
    PartlyReceived,
    Received,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseLine {
    pub id: RecordId,
    pub product: RecordId,
    pub quantity: u32,
    pub unit_price: Money,
    /// The line's share of the order's freight, split by value.
    pub freight: Money,
    pub received: u32,
}

impl PurchaseLine {
    /// Price times quantity, before freight.
    pub fn value(&self) -> Money {
        self.unit_price.times(Decimal::from(self.quantity))
    }

    /// Units still expected.
    pub fn remaining(&self) -> u32 {
        self.quantity.saturating_sub(self.received)
    }

    /// What one unit costs once delivered: its price plus its part of the
    /// freight.
    pub fn landed_unit_cost(&self) -> Money {
        self.unit_price
            .checked_add(Money::new(
                self.freight.amount() / Decimal::from(self.quantity),
                self.freight.currency(),
            ))
            .expect("a line keeps the order's currency")
    }

    /// What `units` more units cost in all: their price plus the part of the
    /// line's freight they carry.
    fn cost_of(&self, units: u32) -> Money {
        self.unit_price
            .times(Decimal::from(units))
            .checked_add(freight_for_units(
                self.freight,
                self.quantity,
                self.received,
                units,
            ))
            .expect("a line keeps the order's currency")
    }
}

/// Units of one line that came in with a receipt, and what they cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedLine {
    pub line: RecordId,
    pub quantity: u32,
    pub cost: Money,
}

/// One delivery the owner received, whole or in part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub id: RecordId,
    pub at: Timestamp,
    pub lines: Vec<ReceivedLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseOrder {
    pub id: RecordId,
    pub supplier: RecordId,
    pub ordered_on: NaiveDate,
    pub freight: Money,
    pub lines: Vec<PurchaseLine>,
    /// Oldest first.
    pub receipts: Vec<Receipt>,
    pub shipped_at: Option<Timestamp>,
    pub cancelled_at: Option<Timestamp>,
}

impl PurchaseOrder {
    pub fn status(&self) -> PurchaseOrderStatus {
        if self.cancelled_at.is_some() {
            PurchaseOrderStatus::Cancelled
        } else if self.lines.iter().all(|line| line.remaining() == 0) {
            PurchaseOrderStatus::Received
        } else if !self.receipts.is_empty() {
            PurchaseOrderStatus::PartlyReceived
        } else if self.shipped_at.is_some() {
            PurchaseOrderStatus::Shipped
        } else {
            PurchaseOrderStatus::Purchased
        }
    }

    pub fn currency(&self) -> Currency {
        self.freight.currency()
    }

    /// What the order cost: every line plus the freight.
    pub fn total(&self) -> Money {
        Money::sum(
            self.currency(),
            self.lines
                .iter()
                .map(PurchaseLine::value)
                .chain([self.freight]),
        )
        .expect("an order keeps one currency")
    }

    pub fn units(&self) -> u32 {
        self.lines.iter().map(|line| line.quantity).sum()
    }

    pub fn received_units(&self) -> u32 {
        self.lines.iter().map(|line| line.received).sum()
    }

    /// When the last units came in.
    pub fn received_at(&self) -> Option<Timestamp> {
        self.receipts.last().map(|receipt| receipt.at)
    }
}

/// Units of one line to take into stock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Receiving {
    pub line: RecordId,
    pub quantity: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum PurchaseOrderError {
    #[error("a Purchase Order needs at least one item")]
    NoLines,
    #[error("each item needs at least one unit")]
    NoUnits,
    #[error("prices and freight cannot be negative")]
    Negative,
    #[error("prices and freight must be in one currency: {0}")]
    Currencies(#[from] CurrencyMismatch),
    #[error("no Purchase Order {0}")]
    UnknownPurchaseOrder(RecordId),
    #[error("the Purchase Order has no item {0}")]
    UnknownLine(RecordId),
    #[error("units of this Purchase Order already came into stock")]
    AlreadyReceiving,
    #[error("the Purchase Order is cancelled")]
    Cancelled,
    #[error("only a Purchase Order not yet shipped can be marked as shipped")]
    NotAwaitingShipment,
    #[error("item {line} has only {remaining} units left to receive")]
    TooMany { line: RecordId, remaining: u32 },
    #[error("choose how many units came in")]
    NothingToReceive,
    #[error(transparent)]
    Stock(#[from] InventoryError),
    #[error("the Purchase Orders hold a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for PurchaseOrderError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => PurchaseOrderError::Unreadable(text),
            StoredValueError::Sql(error) => PurchaseOrderError::Sql(error),
        }
    }
}

pub(crate) const CREATE_PURCHASE_ORDERS: Migration = Migration {
    version: 1,
    name: "create purchase orders, their lines and receipts",
    risky: false,
    sql: "CREATE TABLE commerce_purchase_orders (
        id           TEXT PRIMARY KEY,
        supplier_id  TEXT NOT NULL,
        ordered_on   TEXT NOT NULL,
        freight      TEXT NOT NULL,
        currency     TEXT NOT NULL,
        shipped_at   TEXT,
        cancelled_at TEXT,
        created_at   TEXT NOT NULL,
        updated_at   TEXT NOT NULL,
        deleted_at   TEXT
    );
    CREATE TABLE commerce_purchase_order_lines (
        id                TEXT PRIMARY KEY,
        purchase_order_id TEXT    NOT NULL REFERENCES commerce_purchase_orders (id),
        position          INTEGER NOT NULL,
        product_id        TEXT    NOT NULL,
        quantity          INTEGER NOT NULL,
        unit_price        TEXT    NOT NULL,
        created_at        TEXT    NOT NULL,
        updated_at        TEXT    NOT NULL,
        deleted_at        TEXT
    );
    CREATE INDEX commerce_purchase_order_lines_by_order
        ON commerce_purchase_order_lines (purchase_order_id, position);
    CREATE TABLE commerce_purchase_receipts (
        id                TEXT PRIMARY KEY,
        purchase_order_id TEXT NOT NULL REFERENCES commerce_purchase_orders (id),
        created_at        TEXT NOT NULL,
        updated_at        TEXT NOT NULL,
        deleted_at        TEXT
    );
    CREATE TABLE commerce_purchase_receipt_lines (
        id         TEXT PRIMARY KEY,
        receipt_id TEXT    NOT NULL REFERENCES commerce_purchase_receipts (id),
        line_id    TEXT    NOT NULL REFERENCES commerce_purchase_order_lines (id),
        quantity   INTEGER NOT NULL,
        cost       TEXT    NOT NULL,
        created_at TEXT    NOT NULL,
        updated_at TEXT    NOT NULL,
        deleted_at TEXT
    );
    CREATE INDEX commerce_purchase_receipt_lines_by_receipt
        ON commerce_purchase_receipt_lines (receipt_id);",
};

/// Purchase Orders, and the Inventory their receipts bring units into.
pub struct PurchaseOrders {
    database: Arc<Database>,
    inventory: Arc<Inventory>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl PurchaseOrders {
    pub fn new(
        database: Arc<Database>,
        inventory: Arc<Inventory>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            database,
            inventory,
            clock,
            ids,
        }
    }

    fn new_record(&self) -> Record {
        Record::new(self.ids.as_ref(), self.clock.as_ref())
    }

    fn now(&self) -> String {
        stored(self.clock.now())
    }

    /// A connection of its own for a transaction. Each one starts
    /// immediate, taking the write lock first, so what it checks still holds
    /// when it writes.
    async fn writer(&self) -> Result<mascate_platform::Connection, PurchaseOrderError> {
        Ok(self.database.connect_for_transaction().await?)
    }

    /// Registers a Purchase Order, as purchased.
    pub async fn create(
        &self,
        order: NewPurchaseOrder,
    ) -> Result<PurchaseOrder, PurchaseOrderError> {
        check(&order)?;
        let record = self.new_record();
        let connection = self.writer().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let at = stored(record.created_at);
        transaction
            .execute(
                "INSERT INTO commerce_purchase_orders
                     (id, supplier_id, ordered_on, freight, currency, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![
                    record.id.to_string(),
                    order.supplier.to_string(),
                    day(order.ordered_on),
                    order.freight.amount().to_string(),
                    order.freight.currency().code(),
                    at
                ],
            )
            .await?;
        self.insert_lines(&transaction, record.id, &order.lines)
            .await?;
        transaction.commit().await?;
        self.purchase_order(record.id).await
    }

    /// Replaces a Purchase Order's Supplier, date, freight and items, while
    /// none of its units came in.
    pub async fn update(
        &self,
        id: RecordId,
        order: NewPurchaseOrder,
    ) -> Result<PurchaseOrder, PurchaseOrderError> {
        check(&order)?;
        let connection = self.writer().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let current = read_one(&transaction, id).await?;
        refuse_cancelled(&current)?;
        refuse_received(&current)?;
        let now = self.now();
        transaction
            .execute(
                "UPDATE commerce_purchase_orders
                 SET supplier_id = ?1, ordered_on = ?2, freight = ?3, currency = ?4,
                     updated_at = ?5
                 WHERE id = ?6",
                params![
                    order.supplier.to_string(),
                    day(order.ordered_on),
                    order.freight.amount().to_string(),
                    order.freight.currency().code(),
                    now.clone(),
                    id.to_string()
                ],
            )
            .await?;
        transaction
            .execute(
                "UPDATE commerce_purchase_order_lines SET deleted_at = ?1, updated_at = ?1
                 WHERE purchase_order_id = ?2 AND deleted_at IS NULL",
                params![now, id.to_string()],
            )
            .await?;
        self.insert_lines(&transaction, id, &order.lines).await?;
        transaction.commit().await?;
        self.purchase_order(id).await
    }

    /// Marks a purchased order as on its way.
    pub async fn mark_shipped(&self, id: RecordId) -> Result<PurchaseOrder, PurchaseOrderError> {
        self.change(id, |order| match order.status() {
            PurchaseOrderStatus::Purchased => Ok("shipped_at"),
            PurchaseOrderStatus::Cancelled => Err(PurchaseOrderError::Cancelled),
            _ => Err(PurchaseOrderError::NotAwaitingShipment),
        })
        .await
    }

    /// Cancels a Purchase Order none of whose units came in.
    pub async fn cancel(&self, id: RecordId) -> Result<PurchaseOrder, PurchaseOrderError> {
        self.change(id, |order| {
            refuse_cancelled(order)?;
            refuse_received(order)?;
            Ok("cancelled_at")
        })
        .await
    }

    /// Removes a Purchase Order registered by mistake. One whose units came
    /// in stays: the stock they brought points to it.
    pub async fn delete(&self, id: RecordId) -> Result<(), PurchaseOrderError> {
        self.change(id, |order| {
            refuse_received(order)?;
            Ok("deleted_at")
        })
        .await?;
        Ok(())
    }

    /// Sets the time column `allowed` names to now, once it accepts the
    /// order as it stands.
    async fn change(
        &self,
        id: RecordId,
        allowed: impl FnOnce(&PurchaseOrder) -> Result<&'static str, PurchaseOrderError>,
    ) -> Result<PurchaseOrder, PurchaseOrderError> {
        let connection = self.writer().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let current = read_one(&transaction, id).await?;
        let column = allowed(&current)?;
        let now = self.now();
        transaction
            .execute(
                &format!(
                    "UPDATE commerce_purchase_orders SET {column} = ?1, updated_at = ?1
                     WHERE id = ?2"
                ),
                params![now, id.to_string()],
            )
            .await?;
        transaction.commit().await?;
        match column {
            "deleted_at" => Ok(current),
            _ => self.purchase_order(id).await,
        }
    }

    /// Takes units of a Purchase Order into stock at `location`. Each line's
    /// units enter at their price plus the part of the freight they carry,
    /// and the receipt and its Stock Movements are written together or not
    /// at all. Lines left out, or with zero units, stay as they are.
    pub async fn receive(
        &self,
        id: RecordId,
        location: RecordId,
        receiving: &[Receiving],
    ) -> Result<PurchaseOrder, PurchaseOrderError> {
        let mut wanted: BTreeMap<RecordId, u32> = BTreeMap::new();
        for receiving in receiving.iter().filter(|r| r.quantity > 0) {
            *wanted.entry(receiving.line).or_default() += receiving.quantity;
        }
        if wanted.is_empty() {
            return Err(PurchaseOrderError::NothingToReceive);
        }
        let connection = self.writer().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let order = read_one(&transaction, id).await?;
        refuse_cancelled(&order)?;
        let mut received = Vec::new();
        for (&line_id, &quantity) in &wanted {
            let line = order
                .lines
                .iter()
                .find(|line| line.id == line_id)
                .ok_or(PurchaseOrderError::UnknownLine(line_id))?;
            if quantity > line.remaining() {
                return Err(PurchaseOrderError::TooMany {
                    line: line_id,
                    remaining: line.remaining(),
                });
            }
            received.push((line, quantity, line.cost_of(quantity)));
        }

        let receipt = self.new_record();
        let at = stored(receipt.created_at);
        transaction
            .execute(
                "INSERT INTO commerce_purchase_receipts
                     (id, purchase_order_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)",
                params![receipt.id.to_string(), id.to_string(), at.clone()],
            )
            .await?;
        for (line, quantity, cost) in &received {
            transaction
                .execute(
                    "INSERT INTO commerce_purchase_receipt_lines
                         (id, receipt_id, line_id, quantity, cost, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                    params![
                        self.ids.next_id().to_string(),
                        receipt.id.to_string(),
                        line.id.to_string(),
                        i64::from(*quantity),
                        cost.amount().to_string(),
                        at.clone()
                    ],
                )
                .await?;
        }
        let entries: Vec<NewEntry> = received
            .iter()
            .map(|(line, quantity, cost)| NewEntry {
                product: line.product,
                location,
                quantity: *quantity,
                cost: *cost,
                reason: MovementReason::PurchaseReceipt { purchase_order: id },
            })
            .collect();
        self.inventory
            .record_entries(&transaction, &entries)
            .await?;
        transaction
            .execute(
                "UPDATE commerce_purchase_orders SET updated_at = ?1 WHERE id = ?2",
                params![at, id.to_string()],
            )
            .await?;
        transaction.commit().await?;
        self.purchase_order(id).await
    }

    pub async fn purchase_order(&self, id: RecordId) -> Result<PurchaseOrder, PurchaseOrderError> {
        read_one(self.database.connection(), id).await
    }

    /// Every Purchase Order, the most recently ordered first.
    pub async fn purchase_orders(&self) -> Result<Vec<PurchaseOrder>, PurchaseOrderError> {
        read(self.database.connection(), None).await
    }

    async fn insert_lines(
        &self,
        on: &Connection,
        order: RecordId,
        lines: &[NewPurchaseLine],
    ) -> Result<(), PurchaseOrderError> {
        let at = self.now();
        for (position, line) in lines.iter().enumerate() {
            on.execute(
                "INSERT INTO commerce_purchase_order_lines
                     (id, purchase_order_id, position, product_id, quantity, unit_price,
                      created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![
                    self.ids.next_id().to_string(),
                    order.to_string(),
                    position as i64,
                    line.product.to_string(),
                    i64::from(line.quantity),
                    line.unit_price.amount().to_string(),
                    at.clone()
                ],
            )
            .await?;
        }
        Ok(())
    }
}

fn check(order: &NewPurchaseOrder) -> Result<(), PurchaseOrderError> {
    if order.lines.is_empty() {
        return Err(PurchaseOrderError::NoLines);
    }
    for line in &order.lines {
        if line.quantity == 0 {
            return Err(PurchaseOrderError::NoUnits);
        }
        order.freight.checked_add(line.unit_price)?;
        if line.unit_price.is_negative() {
            return Err(PurchaseOrderError::Negative);
        }
    }
    if order.freight.is_negative() {
        return Err(PurchaseOrderError::Negative);
    }
    Ok(())
}

fn refuse_cancelled(order: &PurchaseOrder) -> Result<(), PurchaseOrderError> {
    match order.status() {
        PurchaseOrderStatus::Cancelled => Err(PurchaseOrderError::Cancelled),
        _ => Ok(()),
    }
}

fn refuse_received(order: &PurchaseOrder) -> Result<(), PurchaseOrderError> {
    if order.receipts.is_empty() {
        Ok(())
    } else {
        Err(PurchaseOrderError::AlreadyReceiving)
    }
}

fn day(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

async fn read_one(on: &Connection, id: RecordId) -> Result<PurchaseOrder, PurchaseOrderError> {
    read(on, Some(id))
        .await?
        .pop()
        .ok_or(PurchaseOrderError::UnknownPurchaseOrder(id))
}

/// Reads every order, or only `only`, with its lines and receipts.
async fn read(
    on: &Connection,
    only: Option<RecordId>,
) -> Result<Vec<PurchaseOrder>, PurchaseOrderError> {
    let only = only.map(|id| id.to_string());
    let mut rows = on
        .query(
            "SELECT id, supplier_id, ordered_on, freight, currency, shipped_at, cancelled_at
             FROM commerce_purchase_orders
             WHERE deleted_at IS NULL AND (?1 IS NULL OR id = ?1)
             ORDER BY ordered_on DESC, created_at DESC, id DESC",
            params![only.clone()],
        )
        .await?;
    let mut orders = Vec::new();
    while let Some(row) = rows.next().await? {
        let ordered_on: String = row.get(2)?;
        let currency = row.currency_at(4)?;
        orders.push(PurchaseOrder {
            id: row.id_at(0)?,
            supplier: row.id_at(1)?,
            ordered_on: NaiveDate::parse_from_str(&ordered_on, "%Y-%m-%d")
                .map_err(|_| PurchaseOrderError::Unreadable(ordered_on))?,
            freight: Money::new(row.decimal_at(3)?, currency),
            lines: Vec::new(),
            receipts: Vec::new(),
            shipped_at: row.optional_time_at(5)?,
            cancelled_at: row.optional_time_at(6)?,
        });
    }

    let mut rows = on
        .query(
            "SELECT l.purchase_order_id, l.id, l.product_id, l.quantity, l.unit_price
             FROM commerce_purchase_order_lines l
             JOIN commerce_purchase_orders o ON o.id = l.purchase_order_id
             WHERE l.deleted_at IS NULL AND o.deleted_at IS NULL
               AND (?1 IS NULL OR o.id = ?1)
             ORDER BY l.position",
            params![only.clone()],
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let order = row.id_at(0)?;
        let Some(order) = orders.iter_mut().find(|known| known.id == order) else {
            continue;
        };
        let currency = order.currency();
        order.lines.push(PurchaseLine {
            id: row.id_at(1)?,
            product: row.id_at(2)?,
            quantity: row.get(3)?,
            unit_price: Money::new(row.decimal_at(4)?, currency),
            freight: Money::zero(currency),
            received: 0,
        });
    }

    let mut rows = on
        .query(
            "SELECT r.purchase_order_id, r.id, r.created_at, rl.line_id, rl.quantity, rl.cost
             FROM commerce_purchase_receipt_lines rl
             JOIN commerce_purchase_receipts r ON r.id = rl.receipt_id
             JOIN commerce_purchase_orders o ON o.id = r.purchase_order_id
             WHERE rl.deleted_at IS NULL AND r.deleted_at IS NULL AND o.deleted_at IS NULL
               AND (?1 IS NULL OR o.id = ?1)
             ORDER BY r.created_at, r.id, rl.id",
            params![only],
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let order = row.id_at(0)?;
        let Some(order) = orders.iter_mut().find(|known| known.id == order) else {
            continue;
        };
        let receipt = row.id_at(1)?;
        let received = ReceivedLine {
            line: row.id_at(3)?,
            quantity: row.get(4)?,
            cost: Money::new(row.decimal_at(5)?, order.currency()),
        };
        if let Some(line) = order.lines.iter_mut().find(|l| l.id == received.line) {
            line.received += received.quantity;
        }
        match order.receipts.iter_mut().find(|known| known.id == receipt) {
            Some(known) => known.lines.push(received),
            None => order.receipts.push(Receipt {
                id: receipt,
                at: row.time_at(2)?,
                lines: vec![received],
            }),
        }
    }

    for order in &mut orders {
        let values: Vec<Decimal> = order
            .lines
            .iter()
            .map(|line| line.value().amount())
            .collect();
        for (line, share) in order
            .lines
            .iter_mut()
            .zip(split_by_value(order.freight, &values))
        {
            line.freight = share;
        }
    }
    Ok(orders)
}
