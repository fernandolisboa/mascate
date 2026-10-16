//! The owner's Shopee Orders, for commerce's Orders port (ADR 0033): every
//! Order changed since a time, read by polling `get_order_list` by
//! `update_time` in windows of at most 15 days, the most Shopee takes, then
//! `get_order_detail` 50 at a time. The buyer's returns come from
//! `get_return_list` over the same windows, so a return that changes
//! brings its Order along. Shopee masks the buyer's address for an app off
//! its IP list, and the app needs none: Shopee prints the label.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeDelta};
use mascate_commerce::{
    Buyer, ChannelOrder, ChannelOrderLine, ChannelOrders, ChannelReturn, OrderStatus, ReturnStatus,
    ReturnedItem, Shipment, ShipmentStatus,
};
use mascate_kernel::{Currency, ListingType, Money, PlatformError, Timestamp};
use rust_decimal::Decimal;

use super::answers::{OrderDetail, OrderDetails, OrderItem, OrderList, ReturnAnswer, ReturnList};
use super::sales_channel::{NO_MODEL, table_fee};
use super::{ShopeeSeller, is_order_sn};

const ORDER_LIST: &str = "/api/v2/order/get_order_list";
const ORDER_DETAIL: &str = "/api/v2/order/get_order_detail";
const RETURN_LIST: &str = "/api/v2/returns/get_return_list";

/// The longest span `get_order_list` and `get_return_list` take.
const WINDOW_DAYS: i64 = 15;

/// Orders and returns per page; the most Shopee allows.
const PAGE_SIZE: &str = "100";

/// Order numbers per `get_order_detail`; the most Shopee allows.
const DETAIL_LIMIT: usize = 50;

/// What each Order comes with besides Shopee's defaults.
const DETAIL_FIELDS: &str = "buyer_username,item_list,pickup_done_time,total_amount";

/// A span of Shopee's seconds, both ends included.
type Window = (i64, i64);

impl ChannelOrders for ShopeeSeller {
    /// The Orders changed since `since` and the Orders whose returns
    /// changed, each with every return the buyer opened.
    fn orders_changed_since(&self, since: Timestamp) -> Result<Vec<ChannelOrder>, PlatformError> {
        let spans = windows(since.timestamp(), self.timestamp());
        let mut numbers: Vec<String> = Vec::new();
        for &window in &spans {
            self.changed_orders(window, &mut numbers)?;
        }
        let mut returns: BTreeMap<String, Vec<ReturnAnswer>> = BTreeMap::new();
        for &window in &spans {
            self.changed_returns(window, &mut returns, |_| true)?;
        }
        for order in returns.keys() {
            if !numbers.contains(order) {
                numbers.push(order.clone());
            }
        }
        let details = self.order_details(&numbers)?;
        self.earlier_returns(&details, since.timestamp(), &mut returns)?;
        details
            .into_iter()
            .map(|detail| {
                let found = returns.remove(&detail.order_sn).unwrap_or_default();
                channel_order(detail, &found)
            })
            .collect()
    }
}

impl ShopeeSeller {
    /// The details of `orders`, 50 at a time; an Order number that is not
    /// Shopee's is never asked for.
    pub(super) fn order_details(
        &self,
        orders: &[String],
    ) -> Result<Vec<OrderDetail>, PlatformError> {
        let orders: Vec<&str> = orders
            .iter()
            .map(String::as_str)
            .filter(|order| is_order_sn(order))
            .collect();
        let mut details = Vec::new();
        for chunk in orders.chunks(DETAIL_LIMIT) {
            let found: OrderDetails = self.get(
                ORDER_DETAIL,
                &[
                    ("order_sn_list", &chunk.join(",")),
                    ("response_optional_fields", DETAIL_FIELDS),
                ],
            )?;
            details.extend(found.order_list);
        }
        Ok(details)
    }

    /// Adds to `numbers` the Orders changed within `window`, page by page.
    fn changed_orders(
        &self,
        window: Window,
        numbers: &mut Vec<String>,
    ) -> Result<(), PlatformError> {
        let (from, to) = (window.0.to_string(), window.1.to_string());
        let mut cursor = String::new();
        loop {
            let mut query = vec![
                ("time_range_field", "update_time"),
                ("time_from", from.as_str()),
                ("time_to", to.as_str()),
                ("page_size", PAGE_SIZE),
            ];
            if !cursor.is_empty() {
                query.push(("cursor", cursor.as_str()));
            }
            let page: OrderList = self.get(ORDER_LIST, &query)?;
            for order in page.order_list {
                if !numbers.contains(&order.order_sn) {
                    numbers.push(order.order_sn);
                }
            }
            // A cursor that does not move would read the same page forever.
            if !page.more || page.next_cursor.is_empty() || page.next_cursor == cursor {
                return Ok(());
            }
            cursor = page.next_cursor;
        }
    }

    /// Adds to `returns` the returns changed within `window` of the Orders
    /// `wanted` takes, page by page; one return read twice is kept once.
    fn changed_returns(
        &self,
        window: Window,
        returns: &mut BTreeMap<String, Vec<ReturnAnswer>>,
        wanted: impl Fn(&str) -> bool,
    ) -> Result<(), PlatformError> {
        let (from, to) = (window.0.to_string(), window.1.to_string());
        let mut page_no = 0;
        loop {
            let at = page_no.to_string();
            let page: ReturnList = self.get(
                RETURN_LIST,
                &[
                    ("page_no", at.as_str()),
                    ("page_size", PAGE_SIZE),
                    ("update_time_from", from.as_str()),
                    ("update_time_to", to.as_str()),
                ],
            )?;
            let empty = page.returns.is_empty();
            for found in page.returns {
                if !wanted(&found.order_sn) {
                    continue;
                }
                let kept = returns.entry(found.order_sn.clone()).or_default();
                match kept
                    .iter_mut()
                    .find(|known| known.return_sn == found.return_sn)
                {
                    Some(known) => *known = found,
                    None => kept.push(found),
                }
            }
            // An empty page saying there is more would read forever.
            if !page.more || empty {
                return Ok(());
            }
            page_no += 1;
        }
    }

    /// The returns of the Orders that have units under a return none of
    /// the windows read: the return changed before them. Reads back, 15
    /// days at a time, until each is found or the oldest such Order was
    /// made, since no return comes before its Order.
    fn earlier_returns(
        &self,
        details: &[OrderDetail],
        before: i64,
        returns: &mut BTreeMap<String, Vec<ReturnAnswer>>,
    ) -> Result<(), PlatformError> {
        let missing: Vec<&OrderDetail> = details
            .iter()
            .filter(|detail| has_returns(detail) && !returns.contains_key(&detail.order_sn))
            .collect();
        let Some(oldest) = missing.iter().map(|detail| detail.create_time).min() else {
            return Ok(());
        };
        let mut to = before;
        while to > oldest
            && missing
                .iter()
                .any(|detail| !returns.contains_key(&detail.order_sn))
        {
            let from = (to - window_seconds()).max(oldest);
            self.changed_returns((from, to), returns, |order| {
                missing.iter().any(|detail| detail.order_sn == order)
            })?;
            to = from;
        }
        Ok(())
    }
}

/// `since` to `now` in spans of at most 15 days, oldest first.
fn windows(since: i64, now: i64) -> Vec<Window> {
    let mut spans = Vec::new();
    let mut from = since.min(now);
    loop {
        let to = (from + window_seconds()).min(now);
        spans.push((from, to));
        if to >= now {
            return spans;
        }
        from = to;
    }
}

fn window_seconds() -> i64 {
    TimeDelta::days(WINDOW_DAYS).num_seconds()
}

/// Whether Shopee counts units of the Order under a return.
fn has_returns(detail: &OrderDetail) -> bool {
    detail.order_status == "TO_RETURN"
        || detail
            .item_list
            .iter()
            .any(|item| item.return_requested_qty + item.returned_qty > 0)
}

/// The Order as commerce keeps it, with its returns.
fn channel_order(
    detail: OrderDetail,
    returns: &[ReturnAnswer],
) -> Result<ChannelOrder, PlatformError> {
    let id = detail.order_sn.clone();
    let unexpected =
        |what: &str| PlatformError::Failed(format!("Shopee sent the Order {id} {what}"));
    let currency = detail
        .currency
        .as_deref()
        .and_then(Currency::from_code)
        .ok_or_else(|| unexpected("in a currency the app does not handle"))?;
    let ordered_at = time(detail.create_time).ok_or_else(|| unexpected("undated"))?;
    let updated_at = detail.update_time.and_then(time).unwrap_or(ordered_at);
    let (status, shipment_status) = statuses(&detail);
    let mut lines = Vec::new();
    let mut total = Decimal::ZERO;
    for item in &detail.item_list {
        if item.model_quantity_purchased == 0 {
            continue;
        }
        let line = order_line(item, currency)?;
        total += line.unit_price.amount() * Decimal::from(line.quantity);
        lines.push(line);
    }
    let paid = detail
        .total_amount
        .as_ref()
        .and_then(crate::answers::decimal)
        .filter(|amount| *amount > Decimal::ZERO)
        .map(|amount| Money::new(amount, currency));
    let refunds: Vec<Decimal> = returns
        .iter()
        .filter(|found| found.status == "ACCEPTED")
        .filter(|found| {
            found
                .currency
                .as_deref()
                .is_none_or(|code| code == currency.code())
        })
        .filter_map(|found| {
            found
                .refund_amount
                .as_ref()
                .and_then(crate::answers::decimal)
        })
        .collect();
    let buyer = detail
        .buyer_username
        .as_deref()
        .map(str::trim)
        // A name Shopee masks says nothing.
        .filter(|name| !name.is_empty() && !name.contains('*'))
        .map(|name| Buyer {
            nickname: Some(name.to_owned()),
            receiver: None,
        });
    Ok(ChannelOrder {
        id: detail.order_sn.clone(),
        pack: None,
        status,
        ordered_at,
        updated_at,
        lines,
        total: Money::new(total, currency),
        paid,
        shipping_paid: None,
        refunded: (!refunds.is_empty()).then(|| Money::new(refunds.into_iter().sum(), currency)),
        buyer,
        // Shopee ships the Order as one parcel by its number.
        shipment: Some(Shipment {
            id: detail.order_sn.clone(),
            status: shipment_status,
            dispatch_by: detail.ship_by_date.filter(|at| *at > 0).and_then(time),
            // Shopee takes the shipping out of the escrow: a Fee.
            seller_cost: None,
        }),
        returns: returns.iter().map(channel_return).collect(),
    })
}

/// The Order's status and its shipment's. A cancellation the buyer only
/// asked for keeps the units out until Shopee cancels: the seller may turn
/// it down. An Order cancelled after the carrier picked it up counts as
/// shipped, so its units wait for the owner to say they came back (ADR
/// 0019).
fn statuses(detail: &OrderDetail) -> (OrderStatus, ShipmentStatus) {
    let picked_up = detail.pickup_done_time.is_some_and(|at| at > 0);
    let before_pickup = if picked_up {
        ShipmentStatus::Shipped
    } else {
        ShipmentStatus::ReadyToShip
    };
    match detail.order_status.as_str() {
        "READY_TO_SHIP" | "PROCESSED" | "RETRY_SHIP" | "IN_CANCEL" => {
            (OrderStatus::Paid, before_pickup)
        }
        // Outside the Correios Shopee waits for the NF-e before the parcel
        // can go.
        "INVOICE_PENDING" => (OrderStatus::Paid, ShipmentStatus::AwaitingInvoice),
        "SHIPPED" => (OrderStatus::Paid, ShipmentStatus::Shipped),
        // Delivered, waiting for the buyer to confirm, done, or with a
        // return the returns tell.
        "TO_CONFIRM_RECEIVE" | "COMPLETED" | "TO_RETURN" => {
            (OrderStatus::Paid, ShipmentStatus::Delivered)
        }
        "CANCELLED" if picked_up => (OrderStatus::Cancelled, ShipmentStatus::Shipped),
        "CANCELLED" => (OrderStatus::Cancelled, ShipmentStatus::Cancelled),
        // `UNPAID`, `PENDING` and any status Shopee adds: bought, the units
        // held for it, not ready to ship.
        _ => (OrderStatus::AwaitingPayment, ShipmentStatus::Pending),
    }
}

/// One item of the Order. Its sale fee is Shopee's published table until
/// the escrow brings the real Fees (ADR 0020).
fn order_line(item: &OrderItem, currency: Currency) -> Result<ChannelOrderLine, PlatformError> {
    let price = |number: &Option<serde_json::Number>| {
        number
            .as_ref()
            .and_then(crate::answers::decimal)
            .filter(|amount| *amount > Decimal::ZERO)
    };
    let unit = price(&item.model_discounted_price)
        .or_else(|| price(&item.model_original_price))
        .unwrap_or(Decimal::ZERO);
    let unit_price = Money::new(unit, currency);
    let fee = table_fee(unit_price)?;
    Ok(ChannelOrderLine {
        item: item.item_id.clone(),
        variation: variation(&item.model_id),
        title: item
            .item_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| item.item_id.clone()),
        variation_name: item
            .model_name
            .clone()
            .filter(|name| !name.trim().is_empty() && item.model_id != NO_MODEL),
        quantity: item.model_quantity_purchased,
        unit_price,
        sale_fee: Some(Money::new(
            fee.rate.of(unit_price).amount() + fee.fixed.amount(),
            currency,
        )),
        listing_type: Some(ListingType::default()),
    })
}

/// A return as commerce keeps it. A refund without sending the items back
/// returns no units.
fn channel_return(found: &ReturnAnswer) -> ChannelReturn {
    let status = match found.status.as_str() {
        // Withdrawn by the buyer, or closed without a refund.
        "CANCELLED" | "CLOSED" => ReturnStatus::Cancelled,
        "ACCEPTED" => ReturnStatus::Delivered,
        _ => ReturnStatus::OnTheWay,
    };
    let items = if found.needs_logistics {
        found
            .item
            .iter()
            .filter(|item| item.amount > 0)
            .map(|item| ReturnedItem {
                item: item.item_id.clone(),
                variation: variation(&item.model_id),
                quantity: item.amount,
            })
            .collect()
    } else {
        Vec::new()
    };
    ChannelReturn {
        id: found.return_sn.clone(),
        status,
        items,
    }
}

/// The variation of a line: the model, or none for model 0, as the
/// Listings keep it.
fn variation(model: &str) -> Option<String> {
    (model != NO_MODEL).then(|| model.to_owned())
}

fn time(seconds: i64) -> Option<Timestamp> {
    DateTime::from_timestamp(seconds, 0)
}
