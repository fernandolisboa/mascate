//! The owner's Amazon Orders, for commerce's Orders port (ADR 0033): every
//! Order the seller ships, created or changed since a time, by polling
//! `searchOrders` of the Orders API 2026-01-01 (`lastUpdatedAfter`,
//! `fulfilledBy=MERCHANT`); Orders Amazon ships from its own centers are
//! left out of this epic. Of the buyer it keeps only who and where to ship
//! to, and Amazon tells that only to an app with the Direct-to-Consumer
//! Shipping role: without it the Order comes without the address, which
//! stays in Seller Central.

use std::sync::atomic::Ordering;

use chrono::{DateTime, SecondsFormat, Utc};
use mascate_commerce::{
    Buyer, ChannelOrder, ChannelOrderLine, ChannelOrders, OrderStatus, Receiver, Shipment,
    ShipmentStatus,
};
use mascate_kernel::{Currency, ListingType, Money, PlatformError, Timestamp};
use rust_decimal::Decimal;

use super::answers::{
    DeliveryAddress, OrderAnswer, OrderItem, OrderSearch, Price, ProceedsBreakdown,
};
use super::sales_channel::table_fee;
use super::{Amazon, MARKETPLACE};

pub(super) const SEARCH_PATH: &str = "/orders/2026-01-01/orders";

/// The most Orders a page of `searchOrders` takes.
const PAGE_SIZE: &str = "100";

/// What each Order comes with: who receives it, the money, the time to
/// ship by and the packages.
const INCLUDED: &str = "RECIPIENT,PROCEEDS,FULFILLMENT,PACKAGES";

/// The same without the recipient, for an app Amazon refuses it to.
const INCLUDED_WITHOUT_RECIPIENT: &str = "PROCEEDS,FULFILLMENT,PACKAGES";

impl ChannelOrders for Amazon {
    /// Pages through the Orders changed since `since`. A page token lasts
    /// 24 hours; one Amazon turns down starts the search over once, and the
    /// same Order read twice keeps its latest change.
    fn orders_changed_since(&self, since: Timestamp) -> Result<Vec<ChannelOrder>, PlatformError> {
        let after = since.to_rfc3339_opts(SecondsFormat::Secs, true);
        let mut restarted = false;
        let mut orders: Vec<ChannelOrder> = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let page = match self.search_page(&after, page_token.as_deref()) {
                Ok(page) => page,
                Err(PlatformError::Refused(_)) if page_token.is_some() && !restarted => {
                    restarted = true;
                    page_token = None;
                    continue;
                }
                Err(error) => return Err(error),
            };
            for answer in page.orders {
                let order = channel_order(answer)?;
                match orders.iter_mut().find(|known| known.id == order.id) {
                    Some(known) if known.updated_at <= order.updated_at => *known = order,
                    Some(_) => {}
                    None => orders.push(order),
                }
            }
            match page.pagination.and_then(|pagination| pagination.next_token) {
                // A token that repeats would read the same page forever.
                Some(next) if page_token.as_ref() != Some(&next) => page_token = Some(next),
                _ => return Ok(orders),
            }
        }
    }
}

impl Amazon {
    /// One page of the search, with the recipient unless Amazon turned it
    /// down for want of the Direct-to-Consumer Shipping role: then without,
    /// from then on.
    fn search_page(
        &self,
        after: &str,
        page_token: Option<&str>,
    ) -> Result<OrderSearch, PlatformError> {
        if !self.recipient_refused.load(Ordering::Relaxed) {
            match self.search(after, page_token, INCLUDED) {
                Err(PlatformError::Expired) => {}
                answer => return answer,
            }
            let answer = self.search(after, page_token, INCLUDED_WITHOUT_RECIPIENT)?;
            self.recipient_refused.store(true, Ordering::Relaxed);
            return Ok(answer);
        }
        self.search(after, page_token, INCLUDED_WITHOUT_RECIPIENT)
    }

    fn search(
        &self,
        after: &str,
        page_token: Option<&str>,
        included: &str,
    ) -> Result<OrderSearch, PlatformError> {
        let mut query = vec![
            ("marketplaceIds", MARKETPLACE),
            ("fulfilledBy", "MERCHANT"),
            ("lastUpdatedAfter", after),
            ("includedData", included),
            ("maxResultsPerPage", PAGE_SIZE),
        ];
        if let Some(token) = page_token {
            query.push(("paginationToken", token));
        }
        self.get(SEARCH_PATH, &query)
    }
}

/// The Order as commerce keeps it.
fn channel_order(answer: OrderAnswer) -> Result<ChannelOrder, PlatformError> {
    let id = answer.order_id.clone();
    let unexpected =
        |what: &str| PlatformError::Failed(format!("Amazon sent the Order {id} {what}"));
    let ordered_at = time(&answer.created_time).ok_or_else(|| unexpected("undated"))?;
    let updated_at = time(&answer.last_updated_time).unwrap_or(ordered_at);
    let currency = order_currency(&answer)
        .ok_or_else(|| unexpected("in a currency the app does not handle"))?;
    let fulfillment = answer.fulfillment.as_ref();
    let fulfillment_status = fulfillment.map_or("", |found| found.fulfillment_status.as_str());
    let (status, shipment_status) = statuses(fulfillment_status, &answer);
    let mut lines = Vec::new();
    let mut total = Decimal::ZERO;
    for item in &answer.order_items {
        // A cancelled Order can list its items with no units: nothing to
        // take or give back for them, and the units the line once took stay
        // as they were kept.
        if item.quantity_ordered == 0 {
            continue;
        }
        let net = item_net(item, currency);
        total += net;
        lines.push(order_line(item, net, currency)?);
    }
    let proceeds = answer.proceeds.as_ref();
    let paid = proceeds
        .and_then(|proceeds| proceeds.grand_total.as_ref())
        .and_then(|price| in_currency(price, currency))
        .map(|amount| Money::new(amount, currency));
    let shipping_paid = proceeds
        .map(|proceeds| sum_of(&proceeds.breakdowns, "SHIPPING", currency))
        .filter(|amount| !amount.is_zero())
        .map(|amount| Money::new(amount, currency));
    let buyer = answer
        .recipient
        .as_ref()
        .and_then(|recipient| recipient.delivery_address.as_ref())
        .and_then(receiver)
        .map(|receiver| Buyer {
            nickname: None,
            receiver: Some(receiver),
        });
    Ok(ChannelOrder {
        id: answer.order_id.clone(),
        pack: None,
        status,
        ordered_at,
        updated_at,
        lines,
        total: Money::new(total, currency),
        paid,
        shipping_paid,
        refunded: None,
        buyer,
        // Amazon has no shipment of its own: the Order is the shipment.
        shipment: Some(Shipment {
            id: answer.order_id.clone(),
            status: shipment_status,
            dispatch_by: fulfillment
                .and_then(|found| found.ship_by_window.as_ref())
                .and_then(|window| window.latest_date_time.as_deref())
                .and_then(time),
            // The owner pays a carrier of their own, outside Amazon.
            seller_cost: None,
        }),
        returns: Vec::new(),
    })
}

/// The Order's status and its shipment's. An Order cancelled after a
/// package left counts as shipped, so its units wait for the owner to say
/// they came back (ADR 0019); a package Amazon could not deliver is on its
/// way back.
fn statuses(fulfillment_status: &str, answer: &OrderAnswer) -> (OrderStatus, ShipmentStatus) {
    let packages: Vec<(&str, Option<&str>)> = answer
        .packages
        .iter()
        .filter_map(|package| package.package_status.as_ref())
        .map(|status| (status.status.as_str(), status.detailed_status.as_deref()))
        .collect();
    let came_back = packages.iter().any(|(status, detail)| {
        *status == "UNDELIVERABLE"
            || matches!(*detail, Some("RETURNING_TO_SELLER" | "RETURNED_TO_SELLER"))
    });
    let delivered =
        !packages.is_empty() && packages.iter().all(|(status, _)| *status == "DELIVERED");
    let left = answer
        .packages
        .iter()
        .any(|package| package.ship_time.is_some())
        || answer.order_items.iter().any(|item| {
            item.fulfillment
                .as_ref()
                .and_then(|fulfillment| fulfillment.quantity_fulfilled)
                .is_some_and(|units| units > 0)
        });
    let shipped = if came_back {
        ShipmentStatus::NotDelivered
    } else if delivered {
        ShipmentStatus::Delivered
    } else {
        ShipmentStatus::Shipped
    };
    match fulfillment_status {
        "UNSHIPPED" => (OrderStatus::Paid, ShipmentStatus::ReadyToShip),
        // Part of it left: what is left the owner confirms in Seller
        // Central.
        "PARTIALLY_SHIPPED" | "SHIPPED" => (OrderStatus::Paid, shipped),
        "CANCELLED" if left => (OrderStatus::Cancelled, shipped),
        "CANCELLED" | "UNFULFILLABLE" => (OrderStatus::Cancelled, ShipmentStatus::Cancelled),
        // `PENDING`, `PENDING_AVAILABILITY` and any status Amazon adds:
        // bought, the units held for it, not ready to ship.
        _ => (OrderStatus::AwaitingPayment, ShipmentStatus::Pending),
    }
}

/// One item as commerce keeps it. Its sale fee is Amazon's published table
/// until the finances bring the real Fees (ADR 0020).
fn order_line(
    item: &OrderItem,
    net: Decimal,
    currency: Currency,
) -> Result<ChannelOrderLine, PlatformError> {
    let product = &item.product;
    let listing = product
        .seller_sku
        .clone()
        .or_else(|| product.asin.clone())
        .unwrap_or_else(|| item.order_item_id.clone());
    let unit_price = Money::new(net / Decimal::from(item.quantity_ordered), currency);
    let fee = table_fee(unit_price)?;
    Ok(ChannelOrderLine {
        title: product
            .title
            .clone()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| listing.clone()),
        item: listing,
        variation: None,
        variation_name: None,
        quantity: item.quantity_ordered,
        unit_price,
        sale_fee: Some(Money::new(
            fee.rate.of(unit_price).amount() + fee.fixed.amount(),
            currency,
        )),
        listing_type: Some(ListingType::default()),
    })
}

/// What the buyer pays for the item, discounts taken: its `ITEM` proceeds
/// less its `DISCOUNT`, whichever sign Amazon gives the discount; the unit
/// price times the units for an Order Amazon has not priced yet, as one
/// still pending; nothing without either.
fn item_net(item: &OrderItem, currency: Currency) -> Decimal {
    let breakdowns = item
        .proceeds
        .as_ref()
        .map(|proceeds| proceeds.breakdowns.as_slice())
        .unwrap_or_default();
    if breakdowns.iter().any(|breakdown| breakdown.kind == "ITEM") {
        return sum_of(breakdowns, "ITEM", currency)
            - sum_of(breakdowns, "DISCOUNT", currency).abs();
    }
    item.product
        .price
        .as_ref()
        .and_then(|price| price.unit_price.as_ref())
        .and_then(|price| in_currency(price, currency))
        .map_or(Decimal::ZERO, |unit| {
            unit * Decimal::from(item.quantity_ordered)
        })
}

/// The breakdowns of `kind` together, in `currency`.
fn sum_of(breakdowns: &[ProceedsBreakdown], kind: &str, currency: Currency) -> Decimal {
    breakdowns
        .iter()
        .filter(|breakdown| breakdown.kind == kind)
        .filter_map(|breakdown| in_currency(&breakdown.subtotal, currency))
        .sum()
}

/// The currency of the Order's money: its total's, or its first item's.
fn order_currency(answer: &OrderAnswer) -> Option<Currency> {
    let total = answer
        .proceeds
        .as_ref()
        .and_then(|proceeds| proceeds.grand_total.as_ref());
    let item = answer.order_items.iter().find_map(|item| {
        item.proceeds
            .as_ref()
            .and_then(|proceeds| proceeds.breakdowns.first())
            .map(|breakdown| &breakdown.subtotal)
            .or_else(|| {
                item.product
                    .price
                    .as_ref()
                    .and_then(|price| price.unit_price.as_ref())
            })
    });
    match total.or(item) {
        Some(price) => Currency::from_code(&price.currency_code),
        // Nothing priced yet, as an Order still pending: the store's.
        None => Some(Currency::Brl),
    }
}

fn in_currency(price: &Price, currency: Currency) -> Option<Decimal> {
    (price.currency_code == currency.code())
        .then(|| price.amount.decimal())
        .flatten()
}

/// Who receives the Order, when Amazon tells it: with the role, the name
/// and the street; without it, at most the city, which says nothing to
/// ship to.
fn receiver(address: &DeliveryAddress) -> Option<Receiver> {
    let present = |text: &Option<String>| {
        text.as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let name = present(&address.name)?;
    let street: Vec<String> = [
        &address.address_line1,
        &address.address_line2,
        &address.address_line3,
    ]
    .into_iter()
    .filter_map(present)
    .collect();
    Some(Receiver {
        name,
        address: (!street.is_empty()).then(|| street.join(", ")),
        city: present(&address.city),
        state: present(&address.state_or_region),
        zip_code: present(&address.postal_code),
    })
}

fn time(text: &str) -> Option<Timestamp> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}
