//! On-Order Listings (#207, ADR 0038): a Mercado Livre listing the owner
//! marks to sell before buying. The channel shows how many days the item
//! takes to be ready, and the listing takes as many orders at once as the
//! owner accepts: the Stock Mirror sends it the orders that still fit
//! instead of a Stock Share. Only the owner marks and unmarks; the app
//! never does it by itself. A sale on it waits for the units of a Purchase
//! Order the owner buys for it (#208).

use std::collections::HashMap;

use libsql::{Connection, TransactionBehavior, params};
use mascate_kernel::{PlatformError, RecordId, Timestamp};
use mascate_platform::{Migration, StoredRow, stored};

use crate::{
    LineStock, ListingError, Listings, OnSalesChannel, Order, OrderError, OrderLine, OrderStatus,
    Orders, StockShort,
};

pub(crate) const CREATE_ON_ORDER_LISTINGS: Migration = Migration {
    version: 16,
    name: "create on-order listings",
    risky: false,
    sql: "CREATE TABLE commerce_on_order_listings (
        id                TEXT    PRIMARY KEY,
        listing_id        TEXT    NOT NULL,
        availability_days INTEGER NOT NULL,
        accepted_orders   INTEGER NOT NULL,
        created_at        TEXT    NOT NULL,
        updated_at        TEXT    NOT NULL,
        deleted_at        TEXT
    );
    CREATE UNIQUE INDEX commerce_on_order_listings_by_listing
        ON commerce_on_order_listings (listing_id) WHERE deleted_at IS NULL;",
};

/// The most days Mercado Livre lets a listing take to have the item ready.
pub const MAX_AVAILABILITY_DAYS: u32 = 60;

/// A Sales Channel that shows on a listing how long the item takes to be
/// ready, as Mercado Livre's `MANUFACTURING_TIME`. Calls block on the
/// network.
pub trait ListingAvailability: OnSalesChannel + Send + Sync {
    /// Shows on the listing `id`, every variation of it included, that the
    /// item takes `days` to be ready; `None` takes that off.
    fn set_availability(&self, id: &str, days: Option<u32>) -> Result<(), PlatformError>;
}

/// What the owner sets when marking a listing as on-order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnOrderTerms {
    /// How many days the item takes to be ready, 1 to
    /// [`MAX_AVAILABILITY_DAYS`].
    pub availability_days: u32,
    /// How many orders the listing takes at once, waiting for dispatch.
    pub accepted_orders: u32,
}

/// A Listing the owner marked as on-order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnOrderListing {
    pub listing: RecordId,
    pub terms: OnOrderTerms,
    pub marked_at: Timestamp,
}

impl Listings {
    /// Every On-Order Listing, oldest mark first.
    pub async fn on_order(&self) -> Result<Vec<OnOrderListing>, ListingError> {
        read_marks(self.database.connection(), None).await
    }

    /// Marks `listing` as on-order with `terms`, every variation of the same
    /// channel listing included: the channel shows the days on the whole
    /// listing, and each variation takes `accepted_orders` of its own. The
    /// days go to the channel first, and only once it took them is the
    /// mark kept, so a refusal leaves the Listing as it was. Days the
    /// channel already shows are not sent again; changing only the orders
    /// accepted sends nothing. Only the owner's own action calls this.
    pub async fn mark_on_order(
        &self,
        listing: RecordId,
        terms: OnOrderTerms,
        channel: &dyn ListingAvailability,
    ) -> Result<Vec<OnOrderListing>, ListingError> {
        if !(1..=MAX_AVAILABILITY_DAYS).contains(&terms.availability_days) {
            return Err(ListingError::InvalidAvailability);
        }
        if terms.accepted_orders == 0 {
            return Err(ListingError::NoOrdersAccepted);
        }
        let on = channel.sales_channel();
        let item = self.listing_on(listing, on).await?.listed.id;
        let siblings: Vec<RecordId> = self
            .sharing_price(on, &item)
            .await?
            .into_iter()
            .map(|sibling| sibling.id)
            .collect();
        let marks = self.marks_of(&siblings).await?;
        let shown = marks.len() == siblings.len()
            && marks
                .iter()
                .all(|mark| mark.terms.availability_days == terms.availability_days);
        if !shown {
            channel.set_availability(&item, Some(terms.availability_days))?;
        }

        let now = stored(self.clock.now());
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        for sibling in &siblings {
            transaction
                .execute(
                    "INSERT INTO commerce_on_order_listings
                         (id, listing_id, availability_days, accepted_orders, created_at,
                          updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                     ON CONFLICT (listing_id) WHERE deleted_at IS NULL DO UPDATE SET
                         availability_days = excluded.availability_days,
                         accepted_orders = excluded.accepted_orders,
                         updated_at = excluded.updated_at
                     WHERE availability_days <> excluded.availability_days
                         OR accepted_orders <> excluded.accepted_orders",
                    params![
                        self.ids.next_id().to_string(),
                        sibling.to_string(),
                        terms.availability_days,
                        terms.accepted_orders,
                        now.clone()
                    ],
                )
                .await?;
        }
        transaction.commit().await?;
        self.marks_of(&siblings).await
    }

    /// Takes the on-order mark off `listing` and every variation of the
    /// same channel listing: the days leave the channel first, and only
    /// then is the mark dropped, so a refusal leaves both as they were. The
    /// Listings take their Stock Share again from the next send. Returns
    /// the Listings unmarked; a Listing without the mark sends nothing.
    /// Only the owner's own action calls this.
    pub async fn unmark_on_order(
        &self,
        listing: RecordId,
        channel: &dyn ListingAvailability,
    ) -> Result<Vec<RecordId>, ListingError> {
        let on = channel.sales_channel();
        let item = self.listing_on(listing, on).await?.listed.id;
        let siblings: Vec<RecordId> = self
            .sharing_price(on, &item)
            .await?
            .into_iter()
            .map(|sibling| sibling.id)
            .collect();
        let marked: Vec<RecordId> = self
            .marks_of(&siblings)
            .await?
            .into_iter()
            .map(|mark| mark.listing)
            .collect();
        if marked.is_empty() {
            return Ok(marked);
        }
        channel.set_availability(&item, None)?;

        let now = stored(self.clock.now());
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        for unmarked in &marked {
            transaction
                .execute(
                    "UPDATE commerce_on_order_listings SET deleted_at = ?1, updated_at = ?1
                     WHERE listing_id = ?2 AND deleted_at IS NULL",
                    params![now.clone(), unmarked.to_string()],
                )
                .await?;
        }
        transaction.commit().await?;
        Ok(marked)
    }

    async fn marks_of(&self, listings: &[RecordId]) -> Result<Vec<OnOrderListing>, ListingError> {
        let mut marks = Vec::new();
        for listing in listings {
            marks.extend(read_marks(self.database.connection(), Some(*listing)).await?);
        }
        Ok(marks)
    }
}

async fn read_marks(
    on: &Connection,
    listing: Option<RecordId>,
) -> Result<Vec<OnOrderListing>, ListingError> {
    let mut rows = on
        .query(
            "SELECT listing_id, availability_days, accepted_orders, created_at
             FROM commerce_on_order_listings
             WHERE deleted_at IS NULL AND (?1 IS NULL OR listing_id = ?1)
             ORDER BY created_at, listing_id",
            params![listing.map(|id| id.to_string())],
        )
        .await?;
    let mut marks = Vec::new();
    while let Some(row) = rows.next().await? {
        marks.push(OnOrderListing {
            listing: row.id_at(0)?,
            terms: OnOrderTerms {
                availability_days: row.get(1)?,
                accepted_orders: row.get(2)?,
            },
            marked_at: row.time_at(3)?,
        });
    }
    Ok(marks)
}

/// A sale on an On-Order Listing whose units have not left the stock yet
/// (ADR 0038, decision 4): the owner buys them, and the next Sync after
/// they come in takes them. Gone once the units leave or the Order is
/// cancelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnOrderSale {
    pub order: Order,
    pub line: OrderLine,
    /// The Product the Listing sold.
    pub product: RecordId,
    /// Units the stock does not cover: the units on hand go to the older
    /// sales still waiting for theirs first.
    pub missing: u32,
    /// The Purchase Order bought for the sale, while not cancelled.
    pub purchase: Option<RecordId>,
}

/// A line still waiting for its units, as the purchase it asks for sees
/// it.
struct Waiting {
    line: RecordId,
    product: RecordId,
    quantity: u32,
    purchase: Option<RecordId>,
}

impl Orders {
    /// Every sale on an On-Order Listing still waiting for its units, the
    /// closest to its Dispatch Deadline first.
    pub async fn on_order_sales(&self) -> Result<Vec<OnOrderSale>, OrderError> {
        let waiting = waiting_lines(self.database.connection()).await?;
        let mut on_hand: HashMap<RecordId, u32> = self
            .inventory
            .stock()
            .await?
            .products
            .into_iter()
            .map(|stock| {
                let units = stock.valuation.quantity().max(0);
                (stock.product, u32::try_from(units).unwrap_or(u32::MAX))
            })
            .collect();
        let mut missing = HashMap::new();
        for line in &waiting {
            let left = on_hand.entry(line.product).or_default();
            let covered = line.quantity.min(*left);
            *left -= covered;
            missing.insert(line.line, line.quantity - covered);
        }
        let mut sales = Vec::new();
        for order in self.orders().await? {
            for line in &order.lines {
                if line.stock != LineStock::Short(StockShort::OnOrder) {
                    continue;
                }
                let Some(found) = waiting.iter().find(|found| found.line == line.id) else {
                    continue;
                };
                sales.push(OnOrderSale {
                    order: order.clone(),
                    line: line.clone(),
                    product: found.product,
                    missing: missing[&line.id],
                    purchase: found.purchase,
                });
            }
        }
        sales.sort_by(|a, b| {
            let by = |sale: &OnOrderSale| {
                let by = sale.order.dispatch_by();
                (by.is_none(), by)
            };
            by(a)
                .cmp(&by(b))
                .then_with(|| a.order.sold.id.cmp(&b.order.sold.id))
        });
        Ok(sales)
    }
}

/// The lines of Orders not cancelled still waiting for units on hand, the
/// oldest Order first, with the Product their Listing sells and the
/// Purchase Order bought for each, if any.
async fn waiting_lines(on: &Connection) -> Result<Vec<Waiting>, OrderError> {
    let mut rows = on
        .query(
            "SELECT line.id, listing.product_id, line.quantity,
                 (SELECT purchase.id FROM commerce_purchase_order_sales sale
                  JOIN commerce_purchase_orders purchase ON purchase.id = sale.purchase_order_id
                  WHERE sale.order_line_id = line.id AND sale.deleted_at IS NULL
                      AND purchase.deleted_at IS NULL AND purchase.cancelled_at IS NULL
                  ORDER BY purchase.created_at DESC, purchase.id DESC LIMIT 1)
             FROM commerce_order_lines line
             JOIN commerce_orders ord ON ord.id = line.order_id AND ord.deleted_at IS NULL
             JOIN commerce_listings listing
                 ON listing.sales_channel = ord.sales_channel
                 AND listing.channel_item_id = line.channel_item_id
                 AND COALESCE(listing.channel_variation_id, '') =
                     COALESCE(line.channel_variation_id, '')
                 AND listing.deleted_at IS NULL
             WHERE line.deleted_at IS NULL AND line.stock_movement_id IS NULL
                 AND line.stock_short IN (?1, ?2) AND ord.status <> ?3
                 AND listing.product_id IS NOT NULL
             ORDER BY ord.ordered_at, ord.id, line.position",
            params![
                StockShort::NotEnoughStock.code(),
                StockShort::OnOrder.code(),
                OrderStatus::Cancelled.code()
            ],
        )
        .await?;
    let mut waiting = Vec::new();
    while let Some(row) = rows.next().await? {
        waiting.push(Waiting {
            line: row.id_at(0)?,
            product: row.id_at(1)?,
            quantity: row.get(2)?,
            purchase: row.optional_id_at(3)?,
        });
    }
    Ok(waiting)
}
