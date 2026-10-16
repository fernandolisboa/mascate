//! The owner's Magalu Orders, for commerce's Orders port (ADR 0033): every
//! Order changed since a time, read by polling `orders` by `updated_at`
//! page by page. Each Order comes with its deliveries, the parcels it goes
//! in, with the same status `deliveries` reports: the store ships each one
//! only after it sends the delivery's NF-e. Magalu pages through at most
//! 5,000 records, so a span with more is read again in halves.

use chrono::{DateTime, NaiveDate, SecondsFormat};
use mascate_commerce::{
    Buyer, ChannelOrder, ChannelOrderLine, ChannelOrders, ChannelReturn, OrderStatus, Receiver,
    ReturnStatus, ReturnedItem, Shipment, ShipmentStatus,
};
use mascate_kernel::{Currency, ListingType, Money, PlatformError, Timestamp};
use rust_decimal::Decimal;

use super::answers::{Address, Delivery, DeliveryItem, OrderAnswer, OrderPage};
use super::sales_channel::table_fee;
use super::{MAX_OFFSET, Magalu, in_units};

const ORDERS: &str = "/seller/v1/orders";

/// Orders per page; the most Magalu allows.
const PAGE_SIZE: usize = 100;

const CANCELLED: &str = "cancelled";

impl ChannelOrders for Magalu {
    /// The Orders changed from `since` until now, each once.
    fn orders_changed_since(&self, since: Timestamp) -> Result<Vec<ChannelOrder>, PlatformError> {
        let now = self.clock.now();
        let mut found: Vec<OrderAnswer> = Vec::new();
        self.changed_orders(since.min(now), now, &mut found)?;
        found.into_iter().map(channel_order).collect()
    }
}

impl Magalu {
    /// Adds to `found` the Orders changed within `from` and `to`, both
    /// included, page by page. A span with more Orders than Magalu pages
    /// through is read again in two halves.
    fn changed_orders(
        &self,
        from: Timestamp,
        to: Timestamp,
        found: &mut Vec<OrderAnswer>,
    ) -> Result<(), PlatformError> {
        let (gte, lte) = (query_time(from), query_time(to));
        let limit = PAGE_SIZE.to_string();
        let mut offset = 0;
        let past_the_last_page = loop {
            let page: OrderPage = self.get(
                ORDERS,
                &[
                    ("updated_at__gte", &gte),
                    ("updated_at__lte", &lte),
                    ("_offset", &offset.to_string()),
                    ("_limit", &limit),
                    ("_sort", "purchased_at:asc"),
                ],
            )?;
            let count = page.results.len();
            for order in page.results {
                keep_latest(found, order);
            }
            offset += count;
            if count < PAGE_SIZE {
                break false;
            }
            if offset >= MAX_OFFSET {
                break true;
            }
        };
        let middle = from + (to - from) / 2;
        // A second has no halves: what Magalu paged through is all there is
        // to read.
        if past_the_last_page && middle > from {
            self.changed_orders(from, middle, found)?;
            self.changed_orders(middle, to, found)?;
        }
        Ok(())
    }
}

/// Keeps `order` in `found` unless an Order read before is the same one
/// changed later.
fn keep_latest(found: &mut Vec<OrderAnswer>, order: OrderAnswer) {
    match found.iter_mut().find(|known| known.code == order.code) {
        Some(known) => {
            if updated(&order) >= updated(known) {
                *known = order;
            }
        }
        None => found.push(order),
    }
}

fn updated(order: &OrderAnswer) -> Option<Timestamp> {
    order.updated_at.as_deref().and_then(time)
}

/// The time as Magalu's filters take it, `2021-06-18T03:05:54+00:00`.
fn query_time(at: Timestamp) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// The Order as commerce keeps it. Every delivery's items are its lines; a
/// delivery Magalu cancelled in an Order that goes on brings its units back
/// as a return the owner confirms.
fn channel_order(order: OrderAnswer) -> Result<ChannelOrder, PlatformError> {
    let id = order.code.clone();
    let unexpected =
        |what: &str| PlatformError::Failed(format!("Magalu sent the Order {id} {what}"));
    let currency = Currency::from_code(order.amounts.currency.as_deref().unwrap_or("BRL"))
        .ok_or_else(|| unexpected("in a currency the app does not handle"))?;
    let ordered_at = order
        .purchased_at
        .as_deref()
        .or(order.created_at.as_deref())
        .and_then(time)
        .ok_or_else(|| unexpected("undated"))?;
    let updated_at = updated(&order).unwrap_or(ordered_at);
    let status = match order.status.as_deref() {
        Some("approved" | "finished") => OrderStatus::Paid,
        Some(CANCELLED) => OrderStatus::Cancelled,
        // `new` and any status Magalu adds: bought, the units held for it.
        _ => OrderStatus::AwaitingPayment,
    };
    let mut lines = Vec::new();
    let mut total = Decimal::ZERO;
    let mut refunded = Decimal::ZERO;
    let mut returns = Vec::new();
    for delivery in &order.deliveries {
        let mut items = Vec::new();
        for item in delivery.items.iter().filter(|item| item.quantity > 0) {
            let line = order_line(item, currency)
                .ok_or_else(|| unexpected("with an item in another currency"))?;
            let value = line.unit_price.amount() * Decimal::from(line.quantity);
            total += value;
            if is_cancelled(delivery) {
                refunded += value;
                items.push(ReturnedItem {
                    item: line.item.clone(),
                    variation: None,
                    quantity: line.quantity,
                });
            }
            lines.push(line);
        }
        if is_cancelled(delivery) && status != OrderStatus::Cancelled && !items.is_empty() {
            returns.push(ChannelReturn {
                id: delivery.id.clone(),
                // Posted, the parcel comes back; not posted, it never left.
                status: if posted(delivery) {
                    ReturnStatus::OnTheWay
                } else {
                    ReturnStatus::Delivered
                },
                items,
            });
        }
    }
    let main = order
        .deliveries
        .iter()
        .find(|delivery| !is_cancelled(delivery))
        .or(order.deliveries.first());
    let money = |amount: Option<i64>, normalizer: Option<i64>| {
        in_units(amount?, normalizer.unwrap_or(100))
            .filter(|amount| *amount > Decimal::ZERO)
            .map(|amount| Money::new(amount, currency))
    };
    let freight = order.amounts.freight.as_ref();
    Ok(ChannelOrder {
        id: order.code.clone(),
        pack: None,
        status,
        ordered_at,
        updated_at,
        lines,
        total: Money::new(total, currency),
        paid: money(order.amounts.total, order.amounts.normalizer),
        shipping_paid: freight.and_then(|freight| money(freight.total, freight.normalizer)),
        refunded: (status != OrderStatus::Cancelled && refunded > Decimal::ZERO)
            .then(|| Money::new(refunded, currency)),
        buyer: main.and_then(buyer),
        shipment: main.map(|delivery| Shipment {
            id: delivery.id.clone(),
            status: shipment_status(delivery, status),
            dispatch_by: delivery
                .shipping
                .as_ref()
                .and_then(|shipping| shipping.handling_time.as_ref())
                .and_then(|deadline| deadline.limit_date.as_deref())
                .and_then(time),
            // Magalu bills the shipping in its financial analysis: a Fee.
            seller_cost: None,
        }),
        returns,
    })
}

fn is_cancelled(delivery: &Delivery) -> bool {
    delivery.status.as_deref() == Some(CANCELLED)
}

fn posted(delivery: &Delivery) -> bool {
    delivery
        .shipping
        .as_ref()
        .and_then(|shipping| shipping.posting_date.as_deref())
        .is_some_and(|posted| !posted.trim().is_empty())
}

/// Where the delivery stands. Paid, it waits for the store's NF-e before
/// it can ship: a fiscal matter the app reminds and never holds the stock
/// for. A delivery cancelled once posted counts as shipped, so its units
/// wait for the owner to say they came back (ADR 0019).
fn shipment_status(delivery: &Delivery, order: OrderStatus) -> ShipmentStatus {
    let posted = posted(delivery);
    match delivery.status.as_deref() {
        Some("new") => ShipmentStatus::Pending,
        Some("approved") => ShipmentStatus::AwaitingInvoice,
        Some("invoiced") => ShipmentStatus::ReadyToShip,
        Some("shipped") => ShipmentStatus::Shipped,
        Some("delivered") => ShipmentStatus::Delivered,
        Some(CANCELLED) if posted => ShipmentStatus::Shipped,
        Some(CANCELLED) => ShipmentStatus::Cancelled,
        // `frozen` for a while, or a status Magalu adds: what the delivery
        // shows says how far it went.
        _ if posted => ShipmentStatus::Shipped,
        _ if !delivery.invoices.is_empty() => ShipmentStatus::ReadyToShip,
        _ if order == OrderStatus::Paid => ShipmentStatus::AwaitingInvoice,
        _ => ShipmentStatus::Pending,
    }
}

/// One item of the Order, sold by its SKU. Its sale fee is the commission
/// Magalu put on the Order, or the published table without one, until the
/// financial analysis brings the real Fees (ADR 0020). `None` when the
/// item is in another currency than the Order.
fn order_line(item: &DeliveryItem, currency: Currency) -> Option<ChannelOrderLine> {
    if Currency::from_code(&item.unit_price.currency) != Some(currency) {
        return None;
    }
    let unit = in_units(item.unit_price.value, item.unit_price.normalizer)
        .filter(|unit| *unit > Decimal::ZERO)
        .unwrap_or(Decimal::ZERO);
    let unit_price = Money::new(unit, currency);
    let commission = item
        .amounts
        .as_ref()
        .and_then(|amounts| amounts.commission.as_ref())
        .and_then(|commission| in_units(commission.total?, commission.normalizer.unwrap_or(100)))
        .filter(|commission| *commission > Decimal::ZERO)
        .map(|commission| commission / Decimal::from(item.quantity));
    let sale_fee = match commission {
        Some(fee) => fee,
        None => {
            let fee = table_fee(unit_price).ok()?;
            fee.rate.of(unit_price).amount() + fee.fixed.amount()
        }
    };
    let attributes: Vec<String> = item
        .info
        .attributes
        .iter()
        .filter_map(|attribute| {
            let (name, value) = (attribute.name.as_deref()?, attribute.value.as_deref()?);
            Some(format!("{}: {}", name.trim(), value.trim()))
        })
        .collect();
    Some(ChannelOrderLine {
        item: item.info.sku.clone(),
        variation: None,
        title: item
            .info
            .name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| item.info.sku.clone()),
        variation_name: (!attributes.is_empty()).then(|| attributes.join(", ")),
        quantity: item.quantity,
        unit_price,
        sale_fee: Some(Money::new(sale_fee, currency)),
        listing_type: Some(ListingType::default()),
    })
}

/// Who the parcel goes to: the name and address shipping needs, no phone,
/// e-mail or tax id.
fn buyer(delivery: &Delivery) -> Option<Buyer> {
    let recipient = delivery.shipping.as_ref()?.recipient.as_ref()?;
    let name = recipient.name.as_deref()?.trim();
    if name.is_empty() {
        return None;
    }
    let address = recipient.address.as_ref();
    let field = |pick: fn(&Address) -> &Option<String>| {
        address
            .and_then(|address| pick(address).as_deref())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let line: Vec<String> = [
        field(|address| &address.street),
        field(|address| &address.number),
        field(|address| &address.complement),
        field(|address| &address.district),
    ]
    .into_iter()
    .flatten()
    .collect();
    Some(Buyer {
        nickname: None,
        receiver: Some(Receiver {
            name: name.to_owned(),
            address: (!line.is_empty()).then(|| line.join(", ")),
            city: field(|address| &address.city),
            state: field(|address| &address.state),
            zip_code: field(|address| &address.zipcode),
        }),
    })
}

/// A time Magalu sends: a date and time with its offset, or a date alone,
/// taken as its start in UTC.
fn time(text: &str) -> Option<Timestamp> {
    let text = text.trim();
    DateTime::parse_from_rfc3339(text)
        .map(|at| at.to_utc())
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .ok()
                .and_then(|day| day.and_hms_opt(0, 0, 0))
                .map(|at| at.and_utc())
        })
}
