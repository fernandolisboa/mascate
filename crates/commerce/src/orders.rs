//! Orders (#20, #21): the sales in the Sales Channel, read by an Order Sync
//! that the app runs by polling (ADR 0003, ADR 0018). Each Order is kept by
//! the channel's id, so reading it again changes nothing, and each line of
//! an Order sold from the owner's stock takes its units out of the ledger
//! once, in the same transaction that records it (ADR 0012). A cancellation
//! before shipping puts them back by itself; units the buyer sends back
//! wait for the owner to say they arrived (ADR 0019). The buyer's data
//! stays only for shipping and support, for as long as the owner chose.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::TimeDelta;
use libsql::{Connection, Row, TransactionBehavior, Value, params};
use mascate_inventory::{
    HOME_LOCATION, Inventory, InventoryError, LowStock, MovementReason, NewExit, NewReturn,
};
use mascate_kernel::{
    Clock, CurrencyMismatch, IdGenerator, ListingType, Money, PlatformError, RecordId, Timestamp,
};
use mascate_platform::{
    Database, Migration, StoredRow, StoredValueError, load_single_row, save_single_row, stored,
};

pub(crate) const CREATE_ORDERS: Migration = Migration {
    version: 6,
    name: "create orders",
    risky: false,
    sql: "CREATE TABLE commerce_orders (
        id                   TEXT PRIMARY KEY,
        ml_order_id          TEXT NOT NULL,
        ml_pack_id           TEXT,
        status               TEXT NOT NULL,
        fulfillment_mode     TEXT NOT NULL,
        ordered_at           TEXT NOT NULL,
        channel_updated_at   TEXT NOT NULL,
        total                TEXT NOT NULL,
        currency             TEXT NOT NULL,
        paid                 TEXT,
        shipping_paid        TEXT,
        ml_shipment_id       TEXT,
        shipment_status      TEXT,
        dispatch_by          TEXT,
        buyer_nickname       TEXT,
        receiver_name        TEXT,
        receiver_address     TEXT,
        receiver_city        TEXT,
        receiver_state       TEXT,
        receiver_zip_code    TEXT,
        buyer_data_erased_at TEXT,
        synced_at            TEXT NOT NULL,
        created_at           TEXT NOT NULL,
        updated_at           TEXT NOT NULL,
        deleted_at           TEXT
    );
    CREATE UNIQUE INDEX commerce_orders_by_ml_id ON commerce_orders (ml_order_id)
        WHERE deleted_at IS NULL;
    CREATE TABLE commerce_order_lines (
        id                TEXT    PRIMARY KEY,
        order_id          TEXT    NOT NULL REFERENCES commerce_orders (id),
        position          INTEGER NOT NULL,
        ml_item_id        TEXT    NOT NULL,
        ml_variation_id   TEXT,
        title             TEXT    NOT NULL,
        variation_name    TEXT,
        quantity          INTEGER NOT NULL,
        unit_price        TEXT    NOT NULL,
        sale_fee          TEXT,
        currency          TEXT    NOT NULL,
        listing_type      TEXT,
        product_id        TEXT,
        stock_movement_id TEXT,
        stock_short       TEXT,
        created_at        TEXT    NOT NULL,
        updated_at        TEXT    NOT NULL,
        deleted_at        TEXT
    );
    CREATE UNIQUE INDEX commerce_order_lines_by_item
        ON commerce_order_lines (order_id, ml_item_id, COALESCE(ml_variation_id, ''))
        WHERE deleted_at IS NULL;
    CREATE TABLE commerce_order_syncs (
        id              TEXT PRIMARY KEY,
        first_synced_at TEXT NOT NULL,
        synced_through  TEXT NOT NULL,
        created_at      TEXT NOT NULL,
        updated_at      TEXT NOT NULL,
        deleted_at      TEXT
    );
    CREATE TABLE commerce_order_settings (
        id                   TEXT    PRIMARY KEY,
        sync_every_minutes   INTEGER NOT NULL,
        keep_buyer_data_days INTEGER NOT NULL,
        created_at           TEXT    NOT NULL,
        updated_at           TEXT    NOT NULL,
        deleted_at           TEXT
    );",
};

pub(crate) const ADD_RETURNS: Migration = Migration {
    version: 7,
    name: "add returns and the dispatch warning",
    risky: false,
    sql: "ALTER TABLE commerce_order_lines ADD COLUMN restocked INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE commerce_order_lines ADD COLUMN unsellable INTEGER NOT NULL DEFAULT 0;
    CREATE TABLE commerce_order_returns (
        id              TEXT    PRIMARY KEY,
        order_id        TEXT    NOT NULL REFERENCES commerce_orders (id),
        ml_return_id    TEXT    NOT NULL,
        status          TEXT    NOT NULL,
        ml_item_id      TEXT    NOT NULL,
        ml_variation_id TEXT,
        quantity        INTEGER NOT NULL,
        created_at      TEXT    NOT NULL,
        updated_at      TEXT    NOT NULL,
        deleted_at      TEXT
    );
    CREATE UNIQUE INDEX commerce_order_returns_by_item
        ON commerce_order_returns
            (order_id, ml_return_id, ml_item_id, COALESCE(ml_variation_id, ''))
        WHERE deleted_at IS NULL;
    ALTER TABLE commerce_order_settings
        ADD COLUMN warn_before_dispatch_hours INTEGER NOT NULL DEFAULT 24;",
};

const SYNC_TABLE: &str = "commerce_order_syncs";
const SYNC_COLUMNS: &[&str] = &["first_synced_at", "synced_through"];
const SETTINGS_TABLE: &str = "commerce_order_settings";
const SETTINGS_COLUMNS: &[&str] = &[
    "sync_every_minutes",
    "keep_buyer_data_days",
    "warn_before_dispatch_hours",
];

/// How far back the first Order Sync reads: the recent Orders, to list.
const FIRST_SYNC_DAYS: i64 = 30;

/// Each Sync reads again from a little before the last one started, so an
/// Order the channel dates a moment late, or a clock a little off, is never
/// missed. Reading an Order twice changes nothing.
const OVERLAP_MINUTES: i64 = 10;

/// Where an Order stands in the Sales Channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderStatus {
    /// Bought, with the payment still to come through.
    AwaitingPayment,
    Paid,
    /// Cancelled, or turned down by the channel.
    Cancelled,
}

impl OrderStatus {
    const ALL: [OrderStatus; 3] = [
        OrderStatus::AwaitingPayment,
        OrderStatus::Paid,
        OrderStatus::Cancelled,
    ];

    pub(crate) fn code(self) -> &'static str {
        match self {
            OrderStatus::AwaitingPayment => "awaiting_payment",
            OrderStatus::Paid => "paid",
            OrderStatus::Cancelled => "cancelled",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.code() == code)
    }
}

/// Where an Order's items come from (`CONTEXT.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FulfillmentMode {
    /// The owner's own stock; every Order in phase 1.
    OwnStock,
    /// The Supplier ships it (phase 3).
    Dropship,
}

impl FulfillmentMode {
    fn code(self) -> &'static str {
        match self {
            FulfillmentMode::OwnStock => "own_stock",
            FulfillmentMode::Dropship => "dropship",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        [FulfillmentMode::OwnStock, FulfillmentMode::Dropship]
            .into_iter()
            .find(|mode| mode.code() == code)
    }
}

/// Where an Order's shipment stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShipmentStatus {
    /// Not ready to ship yet, such as while the payment comes through.
    Pending,
    /// Waiting for the owner to dispatch it.
    ReadyToShip,
    Shipped,
    Delivered,
    /// Came back without reaching the buyer.
    NotDelivered,
    Cancelled,
}

impl ShipmentStatus {
    const ALL: [ShipmentStatus; 6] = [
        ShipmentStatus::Pending,
        ShipmentStatus::ReadyToShip,
        ShipmentStatus::Shipped,
        ShipmentStatus::Delivered,
        ShipmentStatus::NotDelivered,
        ShipmentStatus::Cancelled,
    ];

    fn code(self) -> &'static str {
        match self {
            ShipmentStatus::Pending => "pending",
            ShipmentStatus::ReadyToShip => "ready_to_ship",
            ShipmentStatus::Shipped => "shipped",
            ShipmentStatus::Delivered => "delivered",
            ShipmentStatus::NotDelivered => "not_delivered",
            ShipmentStatus::Cancelled => "cancelled",
        }
    }

    pub(crate) fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.code() == code)
    }

    /// Whether the units already left the owner's hands.
    pub(crate) fn left_the_owner(self) -> bool {
        matches!(
            self,
            ShipmentStatus::Shipped | ShipmentStatus::Delivered | ShipmentStatus::NotDelivered
        )
    }
}

/// An Order's shipment, as the channel reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shipment {
    /// The channel's id of the shipment.
    pub id: String,
    pub status: ShipmentStatus,
    /// When the owner has to dispatch it by.
    pub dispatch_by: Option<Timestamp>,
    /// What the channel charges the owner to ship it; `None` until it says.
    /// An Order bought in a cart shares it with the others.
    pub seller_cost: Option<Money>,
}

/// Who the Order ships to: what shipping and support need, and nothing
/// more (no phone, e-mail or tax id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receiver {
    pub name: String,
    /// Street, number and complement, in one line.
    pub address: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub zip_code: Option<String>,
}

/// The buyer, as far as shipping and support need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buyer {
    /// The buyer's public name in the channel.
    pub nickname: Option<String>,
    pub receiver: Option<Receiver>,
}

/// One item of an Order, as the channel reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelOrderLine {
    /// The channel's id of the listing sold.
    pub item: String,
    /// The channel's id of the variation sold, when the listing has them.
    pub variation: Option<String>,
    pub title: String,
    /// What sets the variation apart, as "Cor: Preto".
    pub variation_name: Option<String>,
    pub quantity: u32,
    /// What the buyer paid for each unit, discounts included.
    pub unit_price: Money,
    /// What the channel keeps of each unit, as it reports it; Fees and the
    /// Realized Margin are worked out from it later (#22).
    pub sale_fee: Option<Money>,
    pub listing_type: Option<ListingType>,
}

/// Where a return the buyer opened stands in the channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReturnStatus {
    /// Opened, with the units still with the buyer or on their way back.
    OnTheWay,
    /// Delivered to the owner, as the channel reports it.
    Delivered,
    /// Called off: the units are not coming back.
    Cancelled,
}

impl ReturnStatus {
    const ALL: [ReturnStatus; 3] = [
        ReturnStatus::OnTheWay,
        ReturnStatus::Delivered,
        ReturnStatus::Cancelled,
    ];

    fn code(self) -> &'static str {
        match self {
            ReturnStatus::OnTheWay => "on_the_way",
            ReturnStatus::Delivered => "delivered",
            ReturnStatus::Cancelled => "cancelled",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.code() == code)
    }
}

/// Units of one item of an Order a return sends back.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReturnedItem {
    pub item: String,
    pub variation: Option<String>,
    pub quantity: u32,
}

/// A return of the buyer's, as the channel reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelReturn {
    /// The channel's id of the return.
    pub id: String,
    pub status: ReturnStatus,
    pub items: Vec<ReturnedItem>,
}

/// An Order as the Sales Channel reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelOrder {
    /// The channel's id of the Order: what keeps it from being read twice.
    pub id: String,
    /// The cart the Order was bought in with others, when it was.
    pub pack: Option<String>,
    pub status: OrderStatus,
    pub ordered_at: Timestamp,
    /// When the channel last changed the Order.
    pub updated_at: Timestamp,
    pub lines: Vec<ChannelOrderLine>,
    /// The items' total.
    pub total: Money,
    /// What the buyer paid, shipping included.
    pub paid: Option<Money>,
    /// What the buyer paid for shipping.
    pub shipping_paid: Option<Money>,
    /// What the channel gave back to the buyer of the items' price.
    pub refunded: Option<Money>,
    /// `None` once its time to be kept is over.
    pub buyer: Option<Buyer>,
    pub shipment: Option<Shipment>,
    /// The returns the buyer opened, if any.
    pub returns: Vec<ChannelReturn>,
}

impl ChannelOrder {
    /// Units of `line` on their way back to the owner: every one when the
    /// shipment failed or the Order was cancelled once shipped, otherwise
    /// those of the returns not called off.
    fn units_coming_back(&self, line: &ChannelOrderLine) -> u32 {
        let shipment = self.shipment.as_ref().map(|shipment| shipment.status);
        let all_back = shipment == Some(ShipmentStatus::NotDelivered)
            || (self.status == OrderStatus::Cancelled
                && shipment.is_some_and(ShipmentStatus::left_the_owner));
        if all_back {
            return line.quantity;
        }
        let returned: u32 = self
            .returns
            .iter()
            .filter(|found| found.status != ReturnStatus::Cancelled)
            .flat_map(|found| &found.items)
            .filter(|item| item.item == line.item && item.variation == line.variation)
            .map(|item| item.quantity)
            .sum();
        returned.min(line.quantity)
    }
}

/// The Orders of the owner's Sales Channel, as a Platform's adapter reports
/// them. Calls block on the network, so they run off the UI thread.
pub trait ChannelOrders: Send + Sync {
    /// Every Order of the owner's created or changed at `since` or after,
    /// each with its shipment.
    fn orders_changed_since(&self, since: Timestamp) -> Result<Vec<ChannelOrder>, PlatformError>;
}

/// The shipping labels of the owner's Orders, as a Platform's adapter prints
/// them. Calls block on the network, so they run off the UI thread.
pub trait ShippingLabels: Send + Sync {
    /// The label of the channel's shipment `shipment`, as a PDF document.
    fn label_pdf(&self, shipment: &str) -> Result<Vec<u8>, PlatformError>;
}

/// A shipping label, ready to save and print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippingLabel {
    /// A file name for it, from the Order's id in the channel.
    pub file_name: String,
    pub pdf: Vec<u8>,
}

/// Why a line's units have not left the stock yet. Each Order Sync tries
/// again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockShort {
    /// The listing sold is not among the Listings: sync them.
    NoListing,
    /// The Listing sold is not linked to a Product yet.
    NoProduct,
    /// The app has fewer units on hand than were sold.
    NotEnoughStock,
}

impl StockShort {
    fn code(self) -> &'static str {
        match self {
            StockShort::NoListing => "no_listing",
            StockShort::NoProduct => "no_product",
            StockShort::NotEnoughStock => "not_enough_stock",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        [
            StockShort::NoListing,
            StockShort::NoProduct,
            StockShort::NotEnoughStock,
        ]
        .into_iter()
        .find(|short| short.code() == code)
    }
}

/// What became of the stock of an Order's line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineStock {
    /// The units left the stock of `product` by `movement`.
    Taken {
        product: RecordId,
        movement: RecordId,
    },
    /// Still to take; the next Order Sync tries again.
    Short(StockShort),
    /// Sold before the first Order Sync: the stock the app had then had
    /// already lost these units, so they never leave it again.
    BeforeFirstSync,
    /// Cancelled before its units left the stock.
    Cancelled,
    /// Shipped by the Supplier, from no stock of the owner's.
    Dropship,
}

/// What became of the units of a line that came back, or are coming.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnitsBack {
    /// On their way back, or arrived without the owner saying so yet.
    pub awaiting: u32,
    /// Back on the shelf: by the cancellation, or received by the owner.
    pub restocked: u32,
    /// Received by the owner, but not fit to sell again.
    pub unsellable: u32,
}

/// One item of an Order, with what became of its stock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderLine {
    pub sold: ChannelOrderLine,
    pub stock: LineStock,
    /// Units that left the stock and are coming back, or came; only lines
    /// whose units left the stock have any.
    pub back: UnitsBack,
}

/// A sale of the owner's, as last synced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    pub id: RecordId,
    /// The Order as the channel last reported it; its lines are the ones
    /// below.
    pub sold: ChannelOrder,
    pub lines: Vec<OrderLine>,
    pub fulfillment_mode: FulfillmentMode,
    /// When the buyer's data was erased, its time to be kept over.
    pub buyer_data_erased_at: Option<Timestamp>,
    pub synced_at: Timestamp,
}

impl Order {
    /// Units sold, every line together.
    pub fn units(&self) -> u32 {
        self.lines.iter().map(|line| line.sold.quantity).sum()
    }

    /// Units on their way back that the owner has yet to say arrived.
    pub fn units_awaiting_return(&self) -> u32 {
        self.lines.iter().map(|line| line.back.awaiting).sum()
    }

    /// Whether the Order is late to dispatch, or will be within `warning`
    /// of `now`; `None` when it is not waiting for the owner to dispatch
    /// it, or has time.
    pub fn dispatch_due(&self, now: Timestamp, warning: TimeDelta) -> Option<DispatchDue> {
        if self.sold.status == OrderStatus::Cancelled {
            return None;
        }
        let shipment = self.sold.shipment.as_ref()?;
        if shipment.status != ShipmentStatus::ReadyToShip {
            return None;
        }
        let by = shipment.dispatch_by?;
        if by < now {
            Some(DispatchDue::Late)
        } else if by - now <= warning {
            Some(DispatchDue::Soon)
        } else {
            None
        }
    }
}

/// How close an Order waiting for dispatch is to its deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchDue {
    /// Past the time the channel set to dispatch it by.
    Late,
    /// Within the warning the owner chose.
    Soon,
}

/// An Order waiting for dispatch that is late or close to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchAlert {
    pub order: Order,
    pub due: DispatchDue,
}

/// What the owner found when a return arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnReceipt {
    /// The units are fit to sell again: they go back on the shelf.
    BackInStock,
    /// The units arrived damaged or incomplete: the return closes and the
    /// stock stays as it is.
    Unsellable,
}

/// What receiving a return did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnReceived {
    /// Units the return settled, every line together.
    pub units: u32,
    /// Units that went back on the shelf.
    pub restocked: u32,
}

/// What an Order Sync did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrderSync {
    /// Orders the channel reported.
    pub read: usize,
    /// Orders the app did not have yet, from before the first Sync too.
    pub new: usize,
    /// Orders whose status, shipment or other details changed.
    pub changed: usize,
    /// The new Orders sold since the first Sync, newest first: the ones to
    /// tell the owner about.
    pub arrived: Vec<Order>,
    /// Lines whose units left the stock in this Sync.
    pub taken: usize,
    /// Lines whose units are still to leave the stock.
    pub short: usize,
    /// Products the units leaving took to their Reorder Point.
    pub reached_reorder_point: Vec<LowStock>,
    /// Orders cancelled before shipping whose units went back on the shelf
    /// in this Sync.
    pub restocked: usize,
}

/// How the app reads Orders: how often, and how long the buyer's data stays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderSettings {
    pub sync_every_minutes: u32,
    pub keep_buyer_data_days: u32,
    /// How long before the time to dispatch an Order it shows as close.
    pub warn_before_dispatch_hours: u32,
}

impl OrderSettings {
    pub const SYNC_EVERY_MINUTES: std::ops::RangeInclusive<u32> = 1..=60;
    pub const KEEP_BUYER_DATA_DAYS: std::ops::RangeInclusive<u32> = 7..=1825;
    pub const WARN_BEFORE_DISPATCH_HOURS: std::ops::RangeInclusive<u32> = 1..=72;

    fn is_valid(self) -> bool {
        Self::SYNC_EVERY_MINUTES.contains(&self.sync_every_minutes)
            && Self::KEEP_BUYER_DATA_DAYS.contains(&self.keep_buyer_data_days)
            && Self::WARN_BEFORE_DISPATCH_HOURS.contains(&self.warn_before_dispatch_hours)
    }

    /// How long before the time to dispatch an Order it shows as close.
    pub fn dispatch_warning(self) -> TimeDelta {
        TimeDelta::hours(i64::from(self.warn_before_dispatch_hours))
    }
}

impl Default for OrderSettings {
    /// A Sync every 5 minutes (ADR 0003); the buyer's data for 90 days,
    /// past the channel's time for returns and claims; Orders show as
    /// close to their time to dispatch a day before it.
    fn default() -> Self {
        Self {
            sync_every_minutes: 5,
            keep_buyer_data_days: 90,
            warn_before_dispatch_hours: 24,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OrderError {
    #[error(
        "a Sync runs every 1 to 60 minutes, the buyer's data stays from 7 days to 5 years and \
         the warning before dispatch is 1 to 72 hours"
    )]
    InvalidSettings,
    #[error("no Order {0}")]
    UnknownOrder(RecordId),
    #[error("the Order has no units on their way back")]
    NoReturnAwaiting,
    #[error("the Order is not waiting to be dispatched, so it has no label to print")]
    NoLabel,
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error(transparent)]
    Stock(#[from] InventoryError),
    #[error(transparent)]
    Currencies(#[from] CurrencyMismatch),
    #[error("the Orders hold a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for OrderError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => OrderError::Unreadable(text),
            StoredValueError::Sql(error) => OrderError::Sql(error),
        }
    }
}

const ORDER_COLUMNS: &str = "id, ml_order_id, ml_pack_id, status, fulfillment_mode, ordered_at,
     channel_updated_at, total, currency, paid, shipping_paid, ml_shipment_id, shipment_status,
     dispatch_by, buyer_nickname, receiver_name, receiver_address, receiver_city,
     receiver_state, receiver_zip_code, buyer_data_erased_at, synced_at, refunded,
     shipment_seller_cost
     FROM commerce_orders WHERE deleted_at IS NULL";

const LINE_COLUMNS: &str = "order_id, ml_item_id, ml_variation_id, title, variation_name,
     quantity, unit_price, sale_fee, currency, listing_type, product_id, stock_movement_id,
     stock_short, restocked, unsellable
     FROM commerce_order_lines WHERE deleted_at IS NULL";

const RETURN_COLUMNS: &str = "order_id, ml_return_id, status, ml_item_id, ml_variation_id,
     quantity
     FROM commerce_order_returns WHERE deleted_at IS NULL";

/// The first Order Sync and how far the last one read.
struct SyncState {
    first_synced_at: Timestamp,
    synced_through: Timestamp,
}

/// The owner's Orders in the Sales Channel.
pub struct Orders {
    pub(crate) database: Arc<Database>,
    pub(crate) inventory: Arc<Inventory>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) ids: Arc<dyn IdGenerator>,
}

impl Orders {
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

    /// Reads every Order created or changed since the last Sync (the last
    /// 30 days on the first), keeps each by the channel's id, and takes out
    /// of the stock the units of each line sold since the first Sync that
    /// have not left yet: the same Order read again never takes them twice.
    /// The time the computer was off is covered by the next Sync, which
    /// reads from where the last one stopped. Buyer data past its time is
    /// erased on the way.
    pub async fn sync(&self, channel: &dyn ChannelOrders) -> Result<OrderSync, OrderError> {
        let started = self.clock.now();
        let state = self.sync_state().await?;
        let since = match &state {
            Some(state) => state.synced_through - TimeDelta::minutes(OVERLAP_MINUTES),
            None => started - TimeDelta::days(FIRST_SYNC_DAYS),
        };
        let mut fresh: BTreeMap<String, ChannelOrder> = BTreeMap::new();
        for order in channel.orders_changed_since(since)? {
            match fresh.get(&order.id) {
                Some(known) if known.updated_at > order.updated_at => {}
                _ => {
                    fresh.insert(order.id.clone(), order);
                }
            }
        }
        let first_synced_at = state.map_or(started, |state| state.first_synced_at);
        let settings = self.settings().await?;
        let keep_since = started - TimeDelta::days(i64::from(settings.keep_buyer_data_days));

        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let mut report = OrderSync {
            read: fresh.len(),
            ..OrderSync::default()
        };
        let mut arrived = Vec::new();
        for mut reported in fresh.into_values() {
            merge_repeated_lines(&mut reported);
            sort_returns(&mut reported);
            if reported.ordered_at < keep_since {
                reported.buyer = None;
            }
            match read_orders(
                &transaction,
                "AND ml_order_id = ?1",
                params![reported.id.clone()],
            )
            .await?
            .pop()
            {
                Some(known) if same_as_kept(&known, &reported) => {
                    transaction
                        .execute(
                            "UPDATE commerce_orders SET synced_at = ?1 WHERE id = ?2",
                            params![stored(started), known.id.to_string()],
                        )
                        .await?;
                }
                Some(known) => {
                    report.changed += 1;
                    write_order(&transaction, known.id, &reported, started).await?;
                    self.write_lines(&transaction, known.id, &reported.lines, started)
                        .await?;
                    self.write_returns(&transaction, known.id, &reported.returns, started)
                        .await?;
                }
                None => {
                    report.new += 1;
                    let id = self.ids.next_id();
                    insert_order(&transaction, id, &reported, started).await?;
                    self.write_lines(&transaction, id, &reported.lines, started)
                        .await?;
                    self.write_returns(&transaction, id, &reported.returns, started)
                        .await?;
                    if reported.ordered_at >= first_synced_at {
                        arrived.push(id);
                    }
                }
            }
        }
        let taken = self
            .take_stock(&transaction, first_synced_at, started)
            .await?;
        report.taken = taken.lines;
        report.reached_reorder_point = taken.reached_reorder_point;
        report.restocked = self.restock_cancelled(&transaction, started).await?;
        erase_buyer_data(&transaction, keep_since, started).await?;
        save_single_row(
            &transaction,
            self.clock.as_ref(),
            self.ids.as_ref(),
            SYNC_TABLE,
            SYNC_COLUMNS,
            vec![
                Value::Text(stored(first_synced_at)),
                Value::Text(stored(started)),
            ],
        )
        .await?;
        transaction.commit().await?;

        let orders = self.orders().await?;
        report.short = orders
            .iter()
            .flat_map(|order| &order.lines)
            .filter(|line| matches!(line.stock, LineStock::Short(_)))
            .count();
        report.arrived = orders
            .into_iter()
            .filter(|order| arrived.contains(&order.id))
            .collect();
        Ok(report)
    }

    /// Every Order, newest first.
    pub async fn orders(&self) -> Result<Vec<Order>, OrderError> {
        let first_synced_at = self.sync_state().await?.map(|state| state.first_synced_at);
        load_orders(self.database.connection(), None, first_synced_at).await
    }

    /// Units sold of each listing (the channel's id, every variation
    /// together) in the Orders bought since `since`, cancelled ones left
    /// out.
    pub async fn units_sold_since(
        &self,
        since: Timestamp,
    ) -> Result<BTreeMap<String, u32>, OrderError> {
        let mut sold = BTreeMap::new();
        for order in self.orders().await? {
            if order.sold.status == OrderStatus::Cancelled || order.sold.ordered_at < since {
                continue;
            }
            for line in &order.lines {
                *sold.entry(line.sold.item.clone()).or_default() += line.sold.quantity;
            }
        }
        Ok(sold)
    }

    /// The Orders waiting for the owner to dispatch them that are late or
    /// within the warning the owner chose, the closest to their time first.
    pub async fn dispatch_alerts(&self) -> Result<Vec<DispatchAlert>, OrderError> {
        let now = self.clock.now();
        let warning = self.settings().await?.dispatch_warning();
        let mut alerts: Vec<DispatchAlert> = self
            .orders()
            .await?
            .into_iter()
            .filter_map(|order| {
                order
                    .dispatch_due(now, warning)
                    .map(|due| DispatchAlert { order, due })
            })
            .collect();
        alerts.sort_by_key(|alert| {
            (
                alert
                    .order
                    .sold
                    .shipment
                    .as_ref()
                    .and_then(|s| s.dispatch_by),
                alert.order.sold.id.clone(),
            )
        });
        Ok(alerts)
    }

    /// The label of an Order waiting to be dispatched, as a PDF to save and
    /// print.
    pub async fn shipping_label(
        &self,
        labels: &dyn ShippingLabels,
        order: RecordId,
    ) -> Result<ShippingLabel, OrderError> {
        let found = load_orders(self.database.connection(), Some(order), None)
            .await?
            .pop()
            .ok_or(OrderError::UnknownOrder(order))?;
        let shipment = match &found.sold.shipment {
            Some(shipment)
                if shipment.status == ShipmentStatus::ReadyToShip
                    && found.sold.status != OrderStatus::Cancelled =>
            {
                shipment
            }
            _ => return Err(OrderError::NoLabel),
        };
        let pdf = labels.label_pdf(&shipment.id)?;
        if !pdf.starts_with(b"%PDF-") {
            return Err(PlatformError::Failed(
                "the channel sent a label that is not a PDF document".into(),
            )
            .into());
        }
        let name: String = found
            .sold
            .id
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        Ok(ShippingLabel {
            file_name: format!("etiqueta-{name}.pdf"),
            pdf,
        })
    }

    /// Settles the units of `order` on their way back, as the owner found
    /// them on arrival: fit to sell, they go back on the shelf at the cost
    /// they left with; unsellable, the return closes and the stock stays.
    /// Units settled never come back twice.
    pub async fn receive_return(
        &self,
        order: RecordId,
        receipt: ReturnReceipt,
    ) -> Result<ReturnReceived, OrderError> {
        let now = self.clock.now();
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let found = load_orders(&transaction, Some(order), None)
            .await?
            .pop()
            .ok_or(OrderError::UnknownOrder(order))?;
        let mut received = ReturnReceived {
            units: 0,
            restocked: 0,
        };
        for line in &found.lines {
            let awaiting = line.back.awaiting;
            let LineStock::Taken { movement, .. } = line.stock else {
                continue;
            };
            if awaiting == 0 {
                continue;
            }
            let settled = match receipt {
                ReturnReceipt::BackInStock => {
                    self.inventory
                        .record_return(
                            &transaction,
                            &NewReturn {
                                exit: movement,
                                quantity: awaiting,
                                reason: MovementReason::SaleReturned { order: found.id },
                            },
                        )
                        .await?;
                    received.restocked += awaiting;
                    Settled::Restocked
                }
                ReturnReceipt::Unsellable => Settled::Unsellable,
            };
            let sold = (line.sold.item.as_str(), line.sold.variation.as_deref());
            settle(&transaction, found.id, sold, settled, awaiting, now).await?;
            received.units += awaiting;
        }
        if received.units == 0 {
            return Err(OrderError::NoReturnAwaiting);
        }
        transaction.commit().await?;
        Ok(received)
    }

    /// When the Orders were last synced; `None` before the first Sync.
    pub async fn last_sync(&self) -> Result<Option<Timestamp>, OrderError> {
        Ok(self.sync_state().await?.map(|state| state.synced_through))
    }

    pub async fn settings(&self) -> Result<OrderSettings, OrderError> {
        match load_single_row(self.database.connection(), SETTINGS_TABLE, SETTINGS_COLUMNS).await? {
            Some(row) => {
                let settings = OrderSettings {
                    sync_every_minutes: row.get(0)?,
                    keep_buyer_data_days: row.get(1)?,
                    warn_before_dispatch_hours: row.get(2)?,
                };
                Ok(if settings.is_valid() {
                    settings
                } else {
                    OrderSettings::default()
                })
            }
            None => Ok(OrderSettings::default()),
        }
    }

    /// Saves the settings and, with a shorter time to keep the buyer's
    /// data, erases what is now past it.
    pub async fn save_settings(&self, settings: OrderSettings) -> Result<(), OrderError> {
        if !settings.is_valid() {
            return Err(OrderError::InvalidSettings);
        }
        save_single_row(
            self.database.connection(),
            self.clock.as_ref(),
            self.ids.as_ref(),
            SETTINGS_TABLE,
            SETTINGS_COLUMNS,
            vec![
                Value::Integer(i64::from(settings.sync_every_minutes)),
                Value::Integer(i64::from(settings.keep_buyer_data_days)),
                Value::Integer(i64::from(settings.warn_before_dispatch_hours)),
            ],
        )
        .await?;
        self.erase_expired_buyer_data().await?;
        Ok(())
    }

    /// Erases the buyer's data of every Order sold longer ago than the
    /// owner keeps it, and returns how many Orders lost it. Each Sync does
    /// it too; the app also calls it on opening.
    pub async fn erase_expired_buyer_data(&self) -> Result<usize, OrderError> {
        let now = self.clock.now();
        let days = self.settings().await?.keep_buyer_data_days;
        erase_buyer_data(
            self.database.connection(),
            now - TimeDelta::days(i64::from(days)),
            now,
        )
        .await
    }

    async fn sync_state(&self) -> Result<Option<SyncState>, OrderError> {
        match load_single_row(self.database.connection(), SYNC_TABLE, SYNC_COLUMNS).await? {
            Some(row) => Ok(Some(SyncState {
                first_synced_at: row.time_at(0)?,
                synced_through: row.time_at(1)?,
            })),
            None => Ok(None),
        }
    }

    /// Writes `lines` over the Order's, keeping what each already took from
    /// the stock.
    async fn write_lines(
        &self,
        on: &Connection,
        order: RecordId,
        lines: &[ChannelOrderLine],
        now: Timestamp,
    ) -> Result<(), OrderError> {
        for (position, line) in lines.iter().enumerate() {
            on.execute(
                "INSERT INTO commerce_order_lines
                     (id, order_id, position, ml_item_id, ml_variation_id, title,
                      variation_name, quantity, unit_price, sale_fee, currency, listing_type,
                      created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
                 ON CONFLICT (order_id, ml_item_id, COALESCE(ml_variation_id, ''))
                     WHERE deleted_at IS NULL
                 DO UPDATE SET position = excluded.position, title = excluded.title,
                     variation_name = excluded.variation_name, quantity = excluded.quantity,
                     unit_price = excluded.unit_price, sale_fee = excluded.sale_fee,
                     currency = excluded.currency, listing_type = excluded.listing_type,
                     updated_at = excluded.updated_at",
                params![
                    self.ids.next_id().to_string(),
                    order.to_string(),
                    i64::try_from(position).unwrap_or(i64::MAX),
                    line.item.clone(),
                    line.variation.clone(),
                    line.title.clone(),
                    line.variation_name.clone(),
                    line.quantity,
                    line.unit_price.amount().to_string(),
                    line.sale_fee.map(|fee| fee.amount().to_string()),
                    line.unit_price.currency().code(),
                    line.listing_type.map(ListingType::code),
                    stored(now)
                ],
            )
            .await?;
        }
        Ok(())
    }

    /// Writes `returns` over the Order's: each item of each return by the
    /// channel's ids, and those the channel no longer reports gone.
    async fn write_returns(
        &self,
        on: &Connection,
        order: RecordId,
        returns: &[ChannelReturn],
        now: Timestamp,
    ) -> Result<(), OrderError> {
        for found in returns {
            for item in &found.items {
                on.execute(
                    "INSERT INTO commerce_order_returns
                         (id, order_id, ml_return_id, status, ml_item_id, ml_variation_id,
                          quantity, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                     ON CONFLICT (order_id, ml_return_id, ml_item_id,
                                  COALESCE(ml_variation_id, ''))
                         WHERE deleted_at IS NULL
                     DO UPDATE SET status = excluded.status, quantity = excluded.quantity,
                         updated_at = excluded.updated_at",
                    params![
                        self.ids.next_id().to_string(),
                        order.to_string(),
                        found.id.clone(),
                        found.status.code(),
                        item.item.clone(),
                        item.variation.clone(),
                        item.quantity,
                        stored(now)
                    ],
                )
                .await?;
            }
        }
        on.execute(
            "UPDATE commerce_order_returns SET deleted_at = ?1, updated_at = ?1
             WHERE order_id = ?2 AND deleted_at IS NULL AND updated_at < ?1",
            params![stored(now), order.to_string()],
        )
        .await?;
        Ok(())
    }

    /// Puts back on the shelf the units of every Order cancelled before
    /// shipping that are not back yet, and returns how many Orders they
    /// were.
    async fn restock_cancelled(
        &self,
        on: &Connection,
        now: Timestamp,
    ) -> Result<usize, OrderError> {
        let mut rows = on
            .query(
                "SELECT line.order_id, line.ml_item_id, line.ml_variation_id,
                     line.stock_movement_id, line.quantity - line.restocked - line.unsellable,
                     ord.shipment_status
                 FROM commerce_order_lines line
                 JOIN commerce_orders ord ON ord.id = line.order_id AND ord.deleted_at IS NULL
                 WHERE line.deleted_at IS NULL AND line.stock_movement_id IS NOT NULL
                     AND ord.status = ?1
                     AND line.quantity > line.restocked + line.unsellable
                 ORDER BY ord.ordered_at, ord.id, line.position",
                params![OrderStatus::Cancelled.code()],
            )
            .await?;
        let mut due = Vec::new();
        while let Some(row) = rows.next().await? {
            let shipment = match row.get::<Option<String>>(5)? {
                Some(code) => {
                    Some(ShipmentStatus::from_code(&code).ok_or(OrderError::Unreadable(code))?)
                }
                None => None,
            };
            if shipment.is_some_and(ShipmentStatus::left_the_owner) {
                continue;
            }
            due.push((
                row.id_at(0)?,
                row.get::<String>(1)?,
                row.get::<Option<String>>(2)?,
                row.id_at(3)?,
                row.get::<u32>(4)?,
            ));
        }
        let mut orders = Vec::new();
        for (order, item, variation, exit, units) in due {
            self.inventory
                .record_return(
                    on,
                    &NewReturn {
                        exit,
                        quantity: units,
                        reason: MovementReason::SaleCancelled { order },
                    },
                )
                .await?;
            let sold = (item.as_str(), variation.as_deref());
            settle(on, order, sold, Settled::Restocked, units, now).await?;
            if !orders.contains(&order) {
                orders.push(order);
            }
        }
        Ok(orders.len())
    }

    /// Takes out of the stock the units of every line still to take: sold
    /// since the first Sync, from the owner's stock, in an Order not
    /// cancelled. A line whose Listing has no Product, or whose Product has
    /// too few units, keeps why and waits for the next Sync.
    async fn take_stock(
        &self,
        on: &Connection,
        first_synced_at: Timestamp,
        now: Timestamp,
    ) -> Result<Taken, OrderError> {
        let mut rows = on
            .query(
                "SELECT line.id, line.order_id, line.ml_item_id, line.ml_variation_id,
                     line.quantity, listing.id, listing.product_id
                 FROM commerce_order_lines line
                 JOIN commerce_orders ord ON ord.id = line.order_id AND ord.deleted_at IS NULL
                 LEFT JOIN commerce_listings listing
                     ON listing.ml_item_id = line.ml_item_id
                     AND COALESCE(listing.ml_variation_id, '') =
                         COALESCE(line.ml_variation_id, '')
                     AND listing.deleted_at IS NULL
                 WHERE line.deleted_at IS NULL AND line.stock_movement_id IS NULL
                     AND ord.status <> ?1 AND ord.fulfillment_mode = ?2
                     AND ord.ordered_at >= ?3
                 ORDER BY ord.ordered_at, ord.id, line.position",
                params![
                    OrderStatus::Cancelled.code(),
                    FulfillmentMode::OwnStock.code(),
                    stored(first_synced_at)
                ],
            )
            .await?;
        let mut due = Vec::new();
        while let Some(row) = rows.next().await? {
            due.push(DueLine {
                line: row.id_at(0)?,
                order: row.id_at(1)?,
                quantity: row.get(4)?,
                listed: row.optional_id_at(5)?.is_some(),
                product: row.optional_id_at(6)?,
            });
        }
        let mut taken = Taken::default();
        for due in due {
            let outcome = match due.product {
                None if !due.listed => Err(StockShort::NoListing),
                None => Err(StockShort::NoProduct),
                Some(product) => {
                    let exit = NewExit {
                        product,
                        location: HOME_LOCATION,
                        quantity: due.quantity,
                        reason: MovementReason::Sale { order: due.order },
                    };
                    match self.inventory.record_exit(on, &exit).await {
                        Ok(exited) => Ok((product, exited)),
                        Err(InventoryError::NotEnoughStock { .. }) => {
                            Err(StockShort::NotEnoughStock)
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            };
            match outcome {
                Ok((product, exited)) => {
                    on.execute(
                        "UPDATE commerce_order_lines SET product_id = ?1,
                             stock_movement_id = ?2, stock_short = NULL, updated_at = ?3
                         WHERE id = ?4",
                        params![
                            product.to_string(),
                            exited.movement.id.to_string(),
                            stored(now),
                            due.line.to_string()
                        ],
                    )
                    .await?;
                    taken.lines += 1;
                    taken
                        .reached_reorder_point
                        .retain(|low| low.product != product);
                    taken
                        .reached_reorder_point
                        .extend(exited.reached_reorder_point);
                }
                Err(short) => {
                    on.execute(
                        "UPDATE commerce_order_lines SET stock_short = ?1, updated_at = ?2
                         WHERE id = ?3 AND COALESCE(stock_short, '') <> ?1",
                        params![short.code(), stored(now), due.line.to_string()],
                    )
                    .await?;
                }
            }
        }
        Ok(taken)
    }
}

/// A line whose units are still to leave the stock.
struct DueLine {
    line: RecordId,
    order: RecordId,
    quantity: u32,
    /// Whether the listing sold is among the Listings.
    listed: bool,
    product: Option<RecordId>,
}

#[derive(Default)]
struct Taken {
    lines: usize,
    reached_reorder_point: Vec<LowStock>,
}

/// How units on their way back were settled.
#[derive(Clone, Copy)]
enum Settled {
    Restocked,
    Unsellable,
}

/// Adds `units` to what was `settled` of the line of `order` that sold
/// `item` and `variation`.
async fn settle(
    on: &Connection,
    order: RecordId,
    (item, variation): (&str, Option<&str>),
    settled: Settled,
    units: u32,
    now: Timestamp,
) -> Result<(), OrderError> {
    let column = match settled {
        Settled::Restocked => "restocked",
        Settled::Unsellable => "unsellable",
    };
    on.execute(
        &format!(
            "UPDATE commerce_order_lines SET {column} = {column} + ?1, updated_at = ?2
             WHERE order_id = ?3 AND ml_item_id = ?4
                 AND COALESCE(ml_variation_id, '') = COALESCE(?5, '') AND deleted_at IS NULL"
        ),
        params![units, stored(now), order.to_string(), item, variation],
    )
    .await?;
    Ok(())
}

/// The channel's returns in one order, so an Order read again compares the
/// same.
fn sort_returns(order: &mut ChannelOrder) {
    for found in &mut order.returns {
        found.items.sort();
    }
    order.returns.sort_by(|a, b| a.id.cmp(&b.id));
}

/// The channel lists one item per listing and variation; should it repeat
/// one, its units count once, in one line.
fn merge_repeated_lines(order: &mut ChannelOrder) {
    let mut merged: Vec<ChannelOrderLine> = Vec::with_capacity(order.lines.len());
    for line in order.lines.drain(..) {
        match merged
            .iter_mut()
            .find(|kept| kept.item == line.item && kept.variation == line.variation)
        {
            Some(kept) => kept.quantity += line.quantity,
            None => merged.push(line),
        }
    }
    order.lines = merged;
}

/// Whether the Order kept is what the channel reports now.
fn same_as_kept(kept: &Order, reported: &ChannelOrder) -> bool {
    kept.sold.id == reported.id
        && kept.sold.pack == reported.pack
        && kept.sold.status == reported.status
        && kept.sold.ordered_at == reported.ordered_at
        && kept.sold.updated_at == reported.updated_at
        && kept.sold.total == reported.total
        && kept.sold.paid == reported.paid
        && kept.sold.shipping_paid == reported.shipping_paid
        && kept.sold.refunded == reported.refunded
        && kept.sold.shipment == reported.shipment
        && kept.sold.returns == reported.returns
        && (kept.sold.buyer == reported.buyer || kept.buyer_data_erased_at.is_some())
}

/// The values of an Order's own columns, in `ORDER_COLUMNS` order from the
/// channel's id on, up to the buyer's data, then the refund and what the
/// shipment costs the owner.
fn order_values(order: &ChannelOrder) -> Vec<Value> {
    let text = |value: Option<String>| value.map_or(Value::Null, Value::Text);
    let amount = |money: Option<Money>| text(money.map(|money| money.amount().to_string()));
    let receiver = order
        .buyer
        .as_ref()
        .and_then(|buyer| buyer.receiver.as_ref());
    vec![
        Value::Text(order.id.clone()),
        text(order.pack.clone()),
        Value::Text(order.status.code().into()),
        Value::Text(stored(order.ordered_at)),
        Value::Text(stored(order.updated_at)),
        Value::Text(order.total.amount().to_string()),
        Value::Text(order.total.currency().code().into()),
        amount(order.paid),
        amount(order.shipping_paid),
        text(order.shipment.as_ref().map(|shipment| shipment.id.clone())),
        text(
            order
                .shipment
                .as_ref()
                .map(|shipment| shipment.status.code().to_owned()),
        ),
        text(
            order
                .shipment
                .as_ref()
                .and_then(|shipment| shipment.dispatch_by)
                .map(stored),
        ),
        text(
            order
                .buyer
                .as_ref()
                .and_then(|buyer| buyer.nickname.clone()),
        ),
        text(receiver.map(|receiver| receiver.name.clone())),
        text(receiver.and_then(|receiver| receiver.address.clone())),
        text(receiver.and_then(|receiver| receiver.city.clone())),
        text(receiver.and_then(|receiver| receiver.state.clone())),
        text(receiver.and_then(|receiver| receiver.zip_code.clone())),
        amount(order.refunded),
        amount(
            order
                .shipment
                .as_ref()
                .and_then(|shipment| shipment.seller_cost),
        ),
    ]
}

async fn insert_order(
    on: &Connection,
    id: RecordId,
    order: &ChannelOrder,
    now: Timestamp,
) -> Result<(), OrderError> {
    let mut values = vec![Value::Text(id.to_string())];
    values.extend(order_values(order));
    values.extend([
        Value::Text(FulfillmentMode::OwnStock.code().into()),
        Value::Text(stored(now)),
    ]);
    on.execute(
        "INSERT INTO commerce_orders
             (id, ml_order_id, ml_pack_id, status, ordered_at, channel_updated_at, total,
              currency, paid, shipping_paid, ml_shipment_id, shipment_status, dispatch_by,
              buyer_nickname, receiver_name, receiver_address, receiver_city, receiver_state,
              receiver_zip_code, refunded, shipment_seller_cost, fulfillment_mode, synced_at,
              created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                 ?18, ?19, ?20, ?21, ?22, ?23, ?23, ?23)",
        values,
    )
    .await?;
    Ok(())
}

/// Writes what the channel reports over an Order. Buyer data once erased
/// stays erased. A changed Order has its Fees and its Money Releases asked
/// for again, since a cancellation or a refund brings the channel's
/// reversals.
async fn write_order(
    on: &Connection,
    id: RecordId,
    order: &ChannelOrder,
    now: Timestamp,
) -> Result<(), OrderError> {
    let mut values = order_values(order);
    values.extend([Value::Text(stored(now)), Value::Text(id.to_string())]);
    on.execute(
        "UPDATE commerce_orders SET ml_order_id = ?1, ml_pack_id = ?2, status = ?3,
             ordered_at = ?4, channel_updated_at = ?5, total = ?6, currency = ?7, paid = ?8,
             shipping_paid = ?9, ml_shipment_id = ?10, shipment_status = ?11,
             dispatch_by = ?12,
             buyer_nickname = CASE WHEN buyer_data_erased_at IS NULL THEN ?13 END,
             receiver_name = CASE WHEN buyer_data_erased_at IS NULL THEN ?14 END,
             receiver_address = CASE WHEN buyer_data_erased_at IS NULL THEN ?15 END,
             receiver_city = CASE WHEN buyer_data_erased_at IS NULL THEN ?16 END,
             receiver_state = CASE WHEN buyer_data_erased_at IS NULL THEN ?17 END,
             receiver_zip_code = CASE WHEN buyer_data_erased_at IS NULL THEN ?18 END,
             refunded = ?19, shipment_seller_cost = ?20, fees_checked_at = NULL,
             releases_checked_at = NULL, synced_at = ?21, updated_at = ?21
         WHERE id = ?22",
        values,
    )
    .await?;
    Ok(())
}

/// Erases the buyer's data of every Order sold before `before`, and returns
/// how many Orders lost it.
async fn erase_buyer_data(
    on: &Connection,
    before: Timestamp,
    now: Timestamp,
) -> Result<usize, OrderError> {
    let erased = on
        .execute(
            "UPDATE commerce_orders SET buyer_nickname = NULL, receiver_name = NULL,
                 receiver_address = NULL, receiver_city = NULL, receiver_state = NULL,
                 receiver_zip_code = NULL, buyer_data_erased_at = ?1, updated_at = ?1
             WHERE deleted_at IS NULL AND buyer_data_erased_at IS NULL AND ordered_at < ?2",
            params![stored(now), stored(before)],
        )
        .await?;
    Ok(usize::try_from(erased).unwrap_or(usize::MAX))
}

async fn read_orders(
    on: &Connection,
    filter: &str,
    params: impl libsql::params::IntoParams,
) -> Result<Vec<Order>, OrderError> {
    let mut rows = on
        .query(
            &format!("SELECT {ORDER_COLUMNS} {filter} ORDER BY ordered_at DESC, ml_order_id DESC"),
            params,
        )
        .await?;
    let mut orders = Vec::new();
    while let Some(row) = rows.next().await? {
        orders.push(order_from(&row)?);
    }
    Ok(orders)
}

fn order_from(row: &Row) -> Result<Order, OrderError> {
    let currency = row.currency_at(8)?;
    let amount = |at: i32| -> Result<Option<Money>, OrderError> {
        Ok(match row.get::<Option<String>>(at)? {
            Some(_) => Some(Money::new(row.decimal_at(at)?, currency)),
            None => None,
        })
    };
    let status: String = row.get(3)?;
    let mode: String = row.get(4)?;
    let shipment = match row.get::<Option<String>>(11)? {
        Some(id) => {
            let status: String = row.get(12)?;
            Some(Shipment {
                id,
                status: ShipmentStatus::from_code(&status).ok_or(OrderError::Unreadable(status))?,
                dispatch_by: row.optional_time_at(13)?,
                seller_cost: amount(23)?,
            })
        }
        None => None,
    };
    let erased_at = row.optional_time_at(20)?;
    let receiver = match row.get::<Option<String>>(15)? {
        Some(name) => Some(Receiver {
            name,
            address: row.get(16)?,
            city: row.get(17)?,
            state: row.get(18)?,
            zip_code: row.get(19)?,
        }),
        None => None,
    };
    let nickname: Option<String> = row.get(14)?;
    let buyer = match (erased_at, nickname, receiver) {
        (Some(_), _, _) | (None, None, None) => None,
        (None, nickname, receiver) => Some(Buyer { nickname, receiver }),
    };
    Ok(Order {
        id: row.id_at(0)?,
        sold: ChannelOrder {
            id: row.get(1)?,
            pack: row.get(2)?,
            status: OrderStatus::from_code(&status).ok_or(OrderError::Unreadable(status))?,
            ordered_at: row.time_at(5)?,
            updated_at: row.time_at(6)?,
            lines: Vec::new(),
            total: Money::new(row.decimal_at(7)?, currency),
            paid: amount(9)?,
            shipping_paid: amount(10)?,
            refunded: amount(22)?,
            buyer,
            shipment,
            returns: Vec::new(),
        },
        lines: Vec::new(),
        fulfillment_mode: FulfillmentMode::from_code(&mode).ok_or(OrderError::Unreadable(mode))?,
        buyer_data_erased_at: erased_at,
        synced_at: row.time_at(21)?,
    })
}

/// The Orders, newest first, with their lines and returns: every one, or
/// only `order`.
async fn load_orders(
    on: &Connection,
    order: Option<RecordId>,
    first_synced_at: Option<Timestamp>,
) -> Result<Vec<Order>, OrderError> {
    let (by_id, by_order, values) = match order {
        Some(order) => (
            "AND id = ?1",
            "AND order_id = ?1",
            vec![Value::Text(order.to_string())],
        ),
        None => ("", "", Vec::new()),
    };
    let mut orders = read_orders(on, by_id, values.clone()).await?;
    let mut lines = read_lines(on, by_order, values.clone()).await?;
    let mut returns = read_returns(on, by_order, values).await?;
    for order in &mut orders {
        order.sold.returns = returns.remove(&order.id).unwrap_or_default();
        let kept = lines.remove(&order.id).unwrap_or_default();
        order.sold.lines = kept.iter().map(|(sold, _)| sold.clone()).collect();
        order.lines = kept
            .into_iter()
            .map(|(sold, kept)| OrderLine {
                back: units_back(&order.sold, &sold, &kept),
                stock: line_stock(order, &kept, first_synced_at),
                sold,
            })
            .collect();
    }
    Ok(orders)
}

/// What a line kept of its stock: the Product and movement, or why not,
/// and what came back of it.
struct KeptStock {
    taken: Option<(RecordId, RecordId)>,
    short: Option<StockShort>,
    restocked: u32,
    unsellable: u32,
}

/// The Orders' lines, in the channel's order, by Order.
async fn read_lines(
    on: &Connection,
    filter: &str,
    values: Vec<Value>,
) -> Result<BTreeMap<RecordId, Vec<(ChannelOrderLine, KeptStock)>>, OrderError> {
    let mut rows = on
        .query(
            &format!("SELECT {LINE_COLUMNS} {filter} ORDER BY order_id, position"),
            values,
        )
        .await?;
    let mut lines: BTreeMap<RecordId, Vec<(ChannelOrderLine, KeptStock)>> = BTreeMap::new();
    while let Some(row) = rows.next().await? {
        let currency = row.currency_at(8)?;
        let listing_type: Option<String> = row.get(9)?;
        let sale_fee = match row.get::<Option<String>>(7)? {
            Some(_) => Some(Money::new(row.decimal_at(7)?, currency)),
            None => None,
        };
        let taken = match (row.optional_id_at(10)?, row.optional_id_at(11)?) {
            (Some(product), Some(movement)) => Some((product, movement)),
            _ => None,
        };
        let short = match row.get::<Option<String>>(12)? {
            Some(code) => Some(StockShort::from_code(&code).ok_or(OrderError::Unreadable(code))?),
            None => None,
        };
        lines.entry(row.id_at(0)?).or_default().push((
            ChannelOrderLine {
                item: row.get(1)?,
                variation: row.get(2)?,
                title: row.get(3)?,
                variation_name: row.get(4)?,
                quantity: row.get(5)?,
                unit_price: Money::new(row.decimal_at(6)?, currency),
                sale_fee,
                listing_type: listing_type.and_then(|code| ListingType::from_code(&code)),
            },
            KeptStock {
                taken,
                short,
                restocked: row.get(13)?,
                unsellable: row.get(14)?,
            },
        ));
    }
    Ok(lines)
}

/// The Orders' returns, each with its items, by Order.
async fn read_returns(
    on: &Connection,
    filter: &str,
    values: Vec<Value>,
) -> Result<BTreeMap<RecordId, Vec<ChannelReturn>>, OrderError> {
    let mut rows = on
        .query(
            &format!(
                "SELECT {RETURN_COLUMNS} {filter}
                 ORDER BY order_id, ml_return_id, ml_item_id, COALESCE(ml_variation_id, '')"
            ),
            values,
        )
        .await?;
    let mut returns: BTreeMap<RecordId, Vec<ChannelReturn>> = BTreeMap::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(1)?;
        let status: String = row.get(2)?;
        let status = ReturnStatus::from_code(&status).ok_or(OrderError::Unreadable(status))?;
        let item = ReturnedItem {
            item: row.get(3)?,
            variation: row.get(4)?,
            quantity: row.get(5)?,
        };
        let of_order = returns.entry(row.id_at(0)?).or_default();
        match of_order.last_mut() {
            Some(last) if last.id == id => last.items.push(item),
            _ => of_order.push(ChannelReturn {
                id,
                status,
                items: vec![item],
            }),
        }
    }
    Ok(returns)
}

/// What came back of a line's units, or is on its way: only units that
/// left the stock come back to it.
fn units_back(order: &ChannelOrder, line: &ChannelOrderLine, kept: &KeptStock) -> UnitsBack {
    let settled = kept.restocked + kept.unsellable;
    UnitsBack {
        awaiting: if kept.taken.is_some() {
            order.units_coming_back(line).saturating_sub(settled)
        } else {
            0
        },
        restocked: kept.restocked,
        unsellable: kept.unsellable,
    }
}

/// What became of a line's stock, from what it kept and its Order.
fn line_stock(order: &Order, kept: &KeptStock, first_synced_at: Option<Timestamp>) -> LineStock {
    if let Some((product, movement)) = kept.taken {
        return LineStock::Taken { product, movement };
    }
    if order.fulfillment_mode == FulfillmentMode::Dropship {
        return LineStock::Dropship;
    }
    if order.sold.status == OrderStatus::Cancelled {
        return LineStock::Cancelled;
    }
    if first_synced_at.is_none_or(|first| order.sold.ordered_at < first) {
        return LineStock::BeforeFirstSync;
    }
    LineStock::Short(kept.short.unwrap_or(StockShort::NoListing))
}
