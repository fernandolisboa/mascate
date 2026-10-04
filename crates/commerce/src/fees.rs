//! Fees and Realized Margin (#22): what the Sales Channel billed for each
//! Order, read from its billing once the Order is billed and again when the
//! Order changes (ADR 0020), and the margin each sale left. The margin is
//! the price the buyer kept paid, minus the Fees, the tax and the cost the
//! units left the ledger with (ADR 0019): the same rule as the Estimated
//! Margin (`Margin::of`). Until the billing has the Order, its Fees are the
//! ones the Order itself reports and the margin is provisional.

use std::collections::BTreeMap;
use std::ops::Range;

use chrono::TimeDelta;
use libsql::{Connection, TransactionBehavior, params};
use mascate_inventory::StockMovement;
use mascate_kernel::{
    Currency, CurrencyMismatch, Margin, Money, Percentage, PlatformError, RecordId, Timestamp,
};
use mascate_platform::{Migration, StoredRow, stored};
use rust_decimal::Decimal;

use crate::freight::split_by_value;
use crate::{LineStock, Order, OrderError, OrderLine, OrderStatus, Orders, ShipmentStatus};

pub(crate) const ADD_FEES: Migration = Migration {
    version: 8,
    name: "add the Fees of each Order",
    risky: false,
    sql: "ALTER TABLE commerce_orders ADD COLUMN refunded TEXT;
    ALTER TABLE commerce_orders ADD COLUMN shipment_seller_cost TEXT;
    ALTER TABLE commerce_orders ADD COLUMN fees_checked_at TEXT;
    CREATE TABLE commerce_order_fees (
        id           TEXT PRIMARY KEY,
        order_id     TEXT NOT NULL REFERENCES commerce_orders (id),
        ml_charge_id TEXT NOT NULL,
        kind         TEXT NOT NULL,
        description  TEXT NOT NULL,
        amount       TEXT NOT NULL,
        currency     TEXT NOT NULL,
        created_at   TEXT NOT NULL,
        updated_at   TEXT NOT NULL,
        deleted_at   TEXT
    );
    CREATE UNIQUE INDEX commerce_order_fees_by_charge
        ON commerce_order_fees (order_id, ml_charge_id)
        WHERE deleted_at IS NULL;",
};

/// The channel bills an Order some time after the sale; an Order it has
/// not billed yet is asked about again after this long.
const UNBILLED_RETRY_HOURS: i64 = 6;

/// Past this long after the sale, an Order the channel never billed stops
/// being asked about, and its margin stays provisional.
const UNBILLED_GIVE_UP_DAYS: i64 = 60;

/// What a Fee pays for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FeeKind {
    /// The channel's fee for the sale.
    SaleFee,
    /// What the channel charges the owner to ship.
    Shipping,
    /// Anything else the channel bills on the sale.
    Other,
}

impl FeeKind {
    const ALL: [FeeKind; 3] = [FeeKind::SaleFee, FeeKind::Shipping, FeeKind::Other];

    fn code(self) -> &'static str {
        match self {
            FeeKind::SaleFee => "sale_fee",
            FeeKind::Shipping => "shipping",
            FeeKind::Other => "other",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.code() == code)
    }
}

/// One charge the channel billed on an Order, or gave back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelFee {
    /// The channel's id of the charge: what keeps it from counting twice.
    pub id: String,
    pub kind: FeeKind,
    /// The channel's own words for it.
    pub description: String,
    /// Negative for a charge the channel gave back, as on a cancellation.
    pub amount: Money,
}

/// The charges the channel billed on one Order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BilledOrder {
    /// The channel's id of the Order.
    pub order: String,
    pub fees: Vec<ChannelFee>,
}

/// The billing of the owner's Sales Channel, as a Platform's adapter
/// reports it. Calls block on the network, so they run off the UI thread.
pub trait ChannelBilling: Send + Sync {
    /// What the channel billed on each of `orders` (the channel's ids). An
    /// Order the channel has not billed yet is left out, or comes without
    /// charges.
    fn billed_fees(&self, orders: &[String]) -> Result<Vec<BilledOrder>, PlatformError>;
}

/// A Fee that came out of a sale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fee {
    pub kind: FeeKind,
    /// The channel's words for a billed Fee; `None` for one the Order
    /// itself reports, before the billing has it.
    pub description: Option<String>,
    pub amount: Money,
}

/// What asking the channel's billing did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeeImport {
    /// Orders asked about.
    pub asked: usize,
    /// Orders the channel had billed.
    pub billed: usize,
}

/// What a sale left: the price the buyer kept paid, minus the Fees, the
/// tax and the cost of the units that did not come back to the shelf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealizedMargin {
    /// The items' price less what went back to the buyer; nothing for a
    /// cancelled Order.
    pub revenue: Money,
    pub fees: Vec<Fee>,
    pub tax: Money,
    /// What the units cost when they left the stock, less those back on
    /// the shelf; `None` when some never left the ledger (sold before the
    /// first Order Sync, still to leave, or shipped by the Supplier).
    pub cost: Option<Money>,
    /// `None` without the cost.
    pub margin: Option<Margin>,
    /// The channel has not billed the Order yet: the Fees are the ones the
    /// Order reports, and may still change.
    pub provisional: bool,
}

impl RealizedMargin {
    /// Every Fee together.
    pub fn fees_total(&self) -> Money {
        self.sum_fees(|_| true)
    }

    /// The Fees of one kind together.
    pub fn fees_of(&self, kind: FeeKind) -> Money {
        self.sum_fees(|fee| fee.kind == kind)
    }

    fn sum_fees(&self, keep: impl Fn(&Fee) -> bool) -> Money {
        let currency = self.revenue.currency();
        Money::new(
            self.fees
                .iter()
                .filter(|fee| keep(fee))
                .map(|fee| fee.amount.amount())
                .sum(),
            currency,
        )
    }
}

/// An Order with what it left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sale {
    pub order: Order,
    pub margin: RealizedMargin,
}

/// Several sales together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SalesSummary {
    pub orders: usize,
    pub revenue: Money,
    pub fees: Money,
    pub tax: Money,
    /// The cost of the sales that have one.
    pub cost: Money,
    /// The margin of the sales that have a cost; `None` when none has.
    pub margin: Option<Margin>,
    /// Sales whose Fees the channel has not billed yet.
    pub provisional: usize,
    /// Sales left out of the margin for want of a cost.
    pub without_cost: usize,
}

impl SalesSummary {
    pub fn of(sales: &[Sale], currency: Currency) -> Result<Self, CurrencyMismatch> {
        let total = |pick: &dyn Fn(&RealizedMargin) -> Money, sales: &[&Sale]| {
            Money::sum(currency, sales.iter().map(|sale| pick(&sale.margin)))
        };
        let all: Vec<&Sale> = sales.iter().collect();
        let costed: Vec<&Sale> = sales
            .iter()
            .filter(|sale| sale.margin.cost.is_some())
            .collect();
        let margin = if costed.is_empty() {
            None
        } else {
            Some(Margin::of(
                total(&|m| m.revenue, &costed)?,
                &[
                    total(&RealizedMargin::fees_total, &costed)?,
                    total(&|m| m.tax, &costed)?,
                    total(&|m| m.cost.unwrap_or(Money::zero(currency)), &costed)?,
                ],
            )?)
        };
        Ok(Self {
            orders: sales.len(),
            revenue: total(&|m| m.revenue, &all)?,
            fees: total(&RealizedMargin::fees_total, &all)?,
            tax: total(&|m| m.tax, &all)?,
            cost: total(&|m| m.cost.unwrap_or(Money::zero(currency)), &costed)?,
            margin,
            provisional: sales.iter().filter(|sale| sale.margin.provisional).count(),
            without_cost: sales.len() - costed.len(),
        })
    }
}

/// An Order's state as the billing import reads it.
struct FeeState {
    id: RecordId,
    ml_id: String,
    /// As stored, to write over only if no Sync changed it meanwhile.
    checked_text: Option<String>,
    checked_at: Option<Timestamp>,
    billed: bool,
    cancelled_before_shipping: bool,
    ordered_at: Timestamp,
}

impl FeeState {
    fn due(&self, now: Timestamp) -> bool {
        if self.cancelled_before_shipping && !self.billed {
            // Nothing was billed, or it all came back: nothing to ask.
            return false;
        }
        let Some(checked) = self.checked_at else {
            return true;
        };
        !self.billed
            && checked <= now - TimeDelta::hours(UNBILLED_RETRY_HOURS)
            && self.ordered_at >= now - TimeDelta::days(UNBILLED_GIVE_UP_DAYS)
    }
}

impl Orders {
    /// Asks the channel's billing for the Fees of each Order not asked
    /// about since it last changed, and of each Order not billed yet, every
    /// few hours for its first 60 days, and keeps them by the channel's id
    /// of each charge: asking again changes nothing. An Order cancelled
    /// before it shipped, and never billed, is not asked about.
    pub async fn import_fees(&self, billing: &dyn ChannelBilling) -> Result<FeeImport, OrderError> {
        let now = self.clock.now();
        let due: Vec<FeeState> = fee_states(self.database.connection())
            .await?
            .into_iter()
            .filter(|state| state.due(now))
            .collect();
        if due.is_empty() {
            return Ok(FeeImport::default());
        }
        let asked: Vec<String> = due.iter().map(|state| state.ml_id.clone()).collect();
        let mut billed: BTreeMap<String, Vec<ChannelFee>> = BTreeMap::new();
        for found in billing.billed_fees(&asked)? {
            billed.entry(found.order).or_default().extend(found.fees);
        }

        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let mut report = FeeImport {
            asked: due.len(),
            billed: 0,
        };
        for state in &due {
            if let Some(fees) = billed.remove(&state.ml_id).filter(|fees| !fees.is_empty()) {
                self.write_fees(&transaction, state.id, &fees, now).await?;
                report.billed += 1;
            }
            // Only if no Order Sync changed the Order meanwhile: a change
            // asks again.
            transaction
                .execute(
                    "UPDATE commerce_orders SET fees_checked_at = ?1
                     WHERE id = ?2 AND COALESCE(fees_checked_at, '') = COALESCE(?3, '')",
                    params![
                        stored(now),
                        state.id.to_string(),
                        state.checked_text.clone()
                    ],
                )
                .await?;
        }
        transaction.commit().await?;
        Ok(report)
    }

    /// The Orders sold within `sold`, newest first, each with its Realized
    /// Margin at the tax rate `tax`.
    pub async fn sales(
        &self,
        tax: Percentage,
        sold: Range<Timestamp>,
    ) -> Result<Vec<Sale>, OrderError> {
        let orders = self.orders().await?;
        let mut billed = read_fees(self.database.connection()).await?;
        let exits: Vec<RecordId> = orders
            .iter()
            .flat_map(|order| &order.lines)
            .filter_map(|line| match line.stock {
                LineStock::Taken { movement, .. } => Some(movement),
                _ => None,
            })
            .collect();
        let exits: BTreeMap<RecordId, StockMovement> = self
            .inventory
            .movements(&exits)
            .await?
            .into_iter()
            .map(|movement| (movement.id, movement))
            .collect();
        let shipping = shipping_shares(&orders);
        orders
            .into_iter()
            .filter(|order| sold.contains(&order.sold.ordered_at))
            .map(|order| {
                let margin = realized_margin(
                    &order,
                    billed.remove(&order.id),
                    shipping.get(&order.id).copied(),
                    &exits,
                    tax,
                )?;
                Ok(Sale { order, margin })
            })
            .collect()
    }

    /// Writes the channel's charges over an Order's: each by its id, and
    /// those the channel no longer reports gone.
    async fn write_fees(
        &self,
        on: &Connection,
        order: RecordId,
        fees: &[ChannelFee],
        now: Timestamp,
    ) -> Result<(), OrderError> {
        for fee in fees {
            on.execute(
                "INSERT INTO commerce_order_fees
                     (id, order_id, ml_charge_id, kind, description, amount, currency,
                      created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 ON CONFLICT (order_id, ml_charge_id) WHERE deleted_at IS NULL
                 DO UPDATE SET kind = excluded.kind, description = excluded.description,
                     amount = excluded.amount, currency = excluded.currency,
                     updated_at = excluded.updated_at",
                params![
                    self.ids.next_id().to_string(),
                    order.to_string(),
                    fee.id.clone(),
                    fee.kind.code(),
                    fee.description.clone(),
                    fee.amount.amount().to_string(),
                    fee.amount.currency().code(),
                    stored(now)
                ],
            )
            .await?;
        }
        on.execute(
            "UPDATE commerce_order_fees SET deleted_at = ?1, updated_at = ?1
             WHERE order_id = ?2 AND deleted_at IS NULL AND updated_at < ?1",
            params![stored(now), order.to_string()],
        )
        .await?;
        Ok(())
    }
}

/// What each Order's lines, status and billing say to the billing import.
async fn fee_states(on: &Connection) -> Result<Vec<FeeState>, OrderError> {
    let mut rows = on
        .query(
            "SELECT ord.id, ord.ml_order_id, ord.fees_checked_at, ord.status,
                 ord.shipment_status, ord.ordered_at,
                 EXISTS (SELECT 1 FROM commerce_order_fees fee
                         WHERE fee.order_id = ord.id AND fee.deleted_at IS NULL)
             FROM commerce_orders ord WHERE ord.deleted_at IS NULL
             ORDER BY ord.ordered_at, ord.id",
            (),
        )
        .await?;
    let mut states = Vec::new();
    while let Some(row) = rows.next().await? {
        let cancelled = row.get::<String>(3)? == OrderStatus::Cancelled.code();
        let shipped = match row.get::<Option<String>>(4)? {
            Some(code) => ShipmentStatus::from_code(&code)
                .ok_or(OrderError::Unreadable(code))?
                .left_the_owner(),
            None => false,
        };
        states.push(FeeState {
            id: row.id_at(0)?,
            ml_id: row.get(1)?,
            checked_text: row.get(2)?,
            checked_at: row.optional_time_at(2)?,
            cancelled_before_shipping: cancelled && !shipped,
            ordered_at: row.time_at(5)?,
            billed: row.get::<i64>(6)? != 0,
        });
    }
    Ok(states)
}

/// The billed Fees of every Order, by Order.
async fn read_fees(on: &Connection) -> Result<BTreeMap<RecordId, Vec<Fee>>, OrderError> {
    let mut rows = on
        .query(
            "SELECT order_id, kind, description, amount, currency FROM commerce_order_fees
             WHERE deleted_at IS NULL ORDER BY order_id, ml_charge_id",
            (),
        )
        .await?;
    let mut fees: BTreeMap<RecordId, Vec<Fee>> = BTreeMap::new();
    while let Some(row) = rows.next().await? {
        let kind: String = row.get(1)?;
        fees.entry(row.id_at(0)?).or_default().push(Fee {
            kind: FeeKind::from_code(&kind).ok_or(OrderError::Unreadable(kind))?,
            description: Some(row.get(2)?),
            amount: Money::new(row.decimal_at(3)?, row.currency_at(4)?),
        });
    }
    Ok(fees)
}

/// What each Order pays of the shipment it ships in: all of it, or, for
/// Orders bought together in one shipment, a share by the items' value.
fn shipping_shares(orders: &[Order]) -> BTreeMap<RecordId, Money> {
    let mut by_shipment: BTreeMap<&str, (Money, Vec<&Order>)> = BTreeMap::new();
    for order in orders {
        let Some(shipment) = &order.sold.shipment else {
            continue;
        };
        let Some(cost) = shipment.seller_cost else {
            continue;
        };
        by_shipment
            .entry(shipment.id.as_str())
            .or_insert((cost, Vec::new()))
            .1
            .push(order);
    }
    let mut shares = BTreeMap::new();
    for (cost, sharing) in by_shipment.into_values() {
        let values: Vec<Decimal> = sharing
            .iter()
            .map(|order| order.sold.total.amount())
            .collect();
        for (order, share) in sharing.iter().zip(split_by_value(cost, &values)) {
            shares.insert(order.id, share);
        }
    }
    shares
}

/// The Realized Margin of `order`, from its `billed` Fees when the channel
/// has billed it, otherwise from what it reports; its `shipping` share; and
/// the cost its units left the ledger with.
fn realized_margin(
    order: &Order,
    billed: Option<Vec<Fee>>,
    shipping: Option<Money>,
    exits: &BTreeMap<RecordId, StockMovement>,
    tax: Percentage,
) -> Result<RealizedMargin, OrderError> {
    let sold = &order.sold;
    let currency = sold.total.currency();
    let cancelled = sold.status == OrderStatus::Cancelled;
    let revenue = if cancelled {
        Money::zero(currency)
    } else {
        let refunded = sold.refunded.unwrap_or(Money::zero(currency));
        let refunded = Money::new(refunded.amount().min(sold.total.amount()), currency);
        sold.total.checked_sub(refunded)?
    };
    let shipped = sold
        .shipment
        .as_ref()
        .is_some_and(|shipment| shipment.status.left_the_owner());
    // Cancelled before it shipped, nothing is left to bill.
    let provisional = billed.is_none() && !(cancelled && !shipped);
    let fees = match billed {
        Some(fees) => fees,
        None => reported_fees(order, shipped, shipping)?,
    };
    let tax = tax.of(revenue);
    let cost = order
        .lines
        .iter()
        .map(|line| line_cost(line, cancelled, exits))
        .collect::<Option<Vec<Money>>>()
        .map(|costs| Money::sum(currency, costs))
        .transpose()?;
    let margin = match cost {
        Some(cost) => {
            let fees_total = Money::sum(currency, fees.iter().map(|fee| fee.amount))?;
            Some(Margin::of(revenue, &[fees_total, tax, cost])?)
        }
        None => None,
    };
    Ok(RealizedMargin {
        revenue,
        fees,
        tax,
        cost,
        margin,
        provisional,
    })
}

/// The Fees an Order reports before the channel bills it: the sale fee of
/// each unit, which a cancellation gives back, and the Order's share of
/// what its shipment costs, once the units left the owner or it was not
/// cancelled.
fn reported_fees(
    order: &Order,
    shipped: bool,
    shipping: Option<Money>,
) -> Result<Vec<Fee>, OrderError> {
    let sold = &order.sold;
    let currency = sold.total.currency();
    let cancelled = sold.status == OrderStatus::Cancelled;
    let mut fees = Vec::new();
    if !cancelled {
        let sale_fee = Money::sum(
            currency,
            order.lines.iter().filter_map(|line| {
                line.sold
                    .sale_fee
                    .map(|fee| fee.times(Decimal::from(line.sold.quantity)))
            }),
        )?;
        fees.push(Fee {
            kind: FeeKind::SaleFee,
            description: None,
            amount: sale_fee,
        });
    }
    if let Some(shipping) = shipping.filter(|_| !cancelled || shipped) {
        fees.push(Fee {
            kind: FeeKind::Shipping,
            description: None,
            amount: shipping,
        });
    }
    Ok(fees)
}

/// What a line's units cost when they left the stock, less those back on
/// the shelf: the units that came back unfit to sell stay a cost. Nothing
/// for a cancelled Order whose units never left; `None` for units that
/// never left the ledger otherwise.
fn line_cost(
    line: &OrderLine,
    cancelled: bool,
    exits: &BTreeMap<RecordId, StockMovement>,
) -> Option<Money> {
    match line.stock {
        LineStock::Taken { movement, .. } => {
            let exit = exits.get(&movement)?;
            let left = exit.quantity.unsigned_abs();
            if left == 0 {
                return None;
            }
            let kept = left.saturating_sub(u64::from(line.back.restocked));
            Some(Money::new(
                -exit.cost.amount() * Decimal::from(kept) / Decimal::from(left),
                exit.cost.currency(),
            ))
        }
        _ if cancelled => Some(Money::zero(line.sold.unit_price.currency())),
        _ => None,
    }
}
