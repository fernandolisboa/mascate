//! The owner's Orders, for commerce's port: every Order created or changed
//! since a time, by polling `/orders/search` (ADR 0018), each with its
//! shipment from `/shipments/{id}`. Of the buyer it keeps only what
//! shipping and support need.

use chrono::{DateTime, SecondsFormat, Utc};
use mascate_commerce::{
    Buyer, ChannelOrder, ChannelOrderLine, ChannelOrders, OrderStatus, Receiver, Shipment,
    ShipmentStatus,
};
use mascate_kernel::{Currency, Money, PlatformError, Timestamp};
use serde_json::Number;

use super::MercadoLivre;
use super::answers::{
    Attribute, OrderAnswer, OrderItemAnswer, OrderSearch, ShipmentAnswer, UserAnswer, decimal,
    money,
};
use super::sales_channel::{listing_type, path_id};

/// Orders per page of `/orders/search`.
const SEARCH_PAGE: u32 = 50;

impl ChannelOrders for MercadoLivre {
    /// Pages through the seller's Orders changed since `since`, oldest
    /// first, and reads the shipment of each one that has it.
    fn orders_changed_since(&self, since: Timestamp) -> Result<Vec<ChannelOrder>, PlatformError> {
        let user: UserAnswer = self.get("/users/me", &[])?;
        let seller = user.id.to_string();
        let from = search_time(since);
        let mut orders = Vec::new();
        let mut offset: u32 = 0;
        loop {
            let page: OrderSearch = self.get(
                "/orders/search",
                &[
                    ("seller", &seller),
                    ("order.date_last_updated.from", &from),
                    ("sort", "date_asc"),
                    ("offset", &offset.to_string()),
                    ("limit", &SEARCH_PAGE.to_string()),
                ],
            )?;
            if page.results.is_empty() {
                break;
            }
            offset += u32::try_from(page.results.len()).unwrap_or(SEARCH_PAGE);
            for answer in page.results {
                orders.push(self.channel_order(answer)?);
            }
            if offset >= page.paging.total {
                break;
            }
        }
        Ok(orders)
    }
}

impl MercadoLivre {
    fn channel_order(&self, answer: OrderAnswer) -> Result<ChannelOrder, PlatformError> {
        let id = whole_id(&answer.id);
        let unexpected =
            |what: &str| PlatformError::Failed(format!("Mercado Livre sent the Order {id} {what}"));
        let currency = answer
            .currency_id
            .as_deref()
            .and_then(Currency::from_code)
            .ok_or_else(|| unexpected("in a currency the app does not handle"))?;
        let ordered_at = time(&answer.date_created).ok_or_else(|| unexpected("undated"))?;
        let updated_at = answer
            .last_updated
            .as_deref()
            .or(answer.date_last_updated.as_deref())
            .and_then(time)
            .unwrap_or(ordered_at);
        let lines = answer
            .order_items
            .into_iter()
            .map(|line| order_line(line, currency))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| unexpected("with an item without a price"))?;
        let total =
            amount(&answer.total_amount, currency).ok_or_else(|| unexpected("without a total"))?;
        let approved: Vec<_> = answer
            .payments
            .iter()
            .filter(|payment| payment.status.as_deref() == Some("approved"))
            .collect();
        let shipping_paid = if approved.is_empty() {
            None
        } else {
            Some(Money::new(
                approved
                    .iter()
                    .filter_map(|payment| decimal(&payment.shipping_cost))
                    .sum(),
                currency,
            ))
        };
        let shipment = match answer.shipping.and_then(|shipping| shipping.id) {
            Some(shipment) => self.shipment(&whole_id(&shipment))?,
            None => None,
        };
        let receiver = shipment.as_ref().and_then(|(_, receiver)| receiver.clone());
        let nickname = answer.buyer.and_then(|buyer| buyer.nickname);
        let buyer =
            (nickname.is_some() || receiver.is_some()).then_some(Buyer { nickname, receiver });
        Ok(ChannelOrder {
            pack: answer.pack_id.as_ref().map(whole_id),
            status: order_status(&answer.status),
            ordered_at,
            updated_at,
            lines,
            total,
            paid: amount(&answer.paid_amount, currency),
            shipping_paid,
            buyer,
            shipment: shipment.map(|(shipment, _)| shipment),
            id,
        })
    }

    /// The shipment `id` and who it goes to; `None` when Mercado Livre has
    /// not created it yet.
    fn shipment(&self, id: &str) -> Result<Option<(Shipment, Option<Receiver>)>, PlatformError> {
        let found: ShipmentAnswer = match self.get_with(
            &format!("/shipments/{}", path_id(id)?),
            &[],
            &[("x-format-new", "true")],
        ) {
            Err(PlatformError::NotFound) => return Ok(None),
            other => other?,
        };
        let dispatch_by = found
            .lead_time
            .and_then(|lead| lead.estimated_handling_limit)
            .and_then(|limit| limit.date)
            .as_deref()
            .and_then(time);
        let receiver = found.destination.and_then(|destination| {
            let address = destination.shipping_address;
            Some(Receiver {
                name: destination
                    .receiver_name
                    .filter(|name| !name.trim().is_empty())?,
                address: address.as_ref().and_then(|a| a.address_line.clone()),
                city: address
                    .as_ref()
                    .and_then(|a| a.city.as_ref())
                    .and_then(|city| city.name.clone()),
                state: address
                    .as_ref()
                    .and_then(|a| a.state.as_ref())
                    .and_then(|state| state.name.clone()),
                zip_code: address.as_ref().and_then(|a| a.zip_code.clone()),
            })
        });
        Ok(Some((
            Shipment {
                id: whole_id(&found.id),
                status: shipment_status(&found.status),
                dispatch_by,
            },
            receiver,
        )))
    }
}

fn order_line(line: OrderItemAnswer, currency: Currency) -> Option<ChannelOrderLine> {
    let line_currency = line
        .currency_id
        .as_deref()
        .and_then(Currency::from_code)
        .unwrap_or(currency);
    Some(ChannelOrderLine {
        variation: line.item.variation_id.as_ref().map(whole_id),
        variation_name: variation_name(&line.item.variation_attributes),
        title: line.item.title,
        item: line.item.id,
        quantity: line.quantity,
        unit_price: money(&line.unit_price, &Some(line_currency.code().to_owned()))?,
        sale_fee: amount(&line.sale_fee, line_currency),
        listing_type: line.listing_type_id.as_deref().and_then(listing_type),
    })
}

fn amount(number: &Option<Number>, currency: Currency) -> Option<Money> {
    decimal(number).map(|amount| Money::new(amount, currency))
}

/// "Cor: Preto · Tamanho: M"; `None` for an item without variations.
fn variation_name(attributes: &[Attribute]) -> Option<String> {
    let parts: Vec<String> = attributes
        .iter()
        .filter_map(|attribute| {
            let value = attribute.value_name.as_deref()?;
            Some(match attribute.name.as_deref() {
                Some(name) => format!("{name}: {value}"),
                None => value.to_owned(),
            })
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// An id Mercado Livre sends as a JSON number, as text.
fn whole_id(number: &Number) -> String {
    number.to_string()
}

/// A time as Mercado Livre writes it, such as
/// `2026-10-04T10:30:00.000-03:00`.
fn time(text: &str) -> Option<Timestamp> {
    DateTime::parse_from_rfc3339(text)
        .or_else(|_| DateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f%z"))
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// A time for a search filter, in UTC as the documentation writes them.
fn search_time(at: Timestamp) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
        .replace('Z', "-00:00")
}

fn order_status(code: &str) -> OrderStatus {
    match code {
        "paid" | "partially_refunded" => OrderStatus::Paid,
        "cancelled" | "invalid" | "pending_cancel" => OrderStatus::Cancelled,
        // `confirmed`, `payment_required`, `payment_in_process`,
        // `partially_paid` and any new one: the units stay sold until the
        // channel says otherwise.
        _ => OrderStatus::AwaitingPayment,
    }
}

fn shipment_status(code: &str) -> ShipmentStatus {
    match code {
        "handling" | "ready_to_ship" => ShipmentStatus::ReadyToShip,
        "shipped" => ShipmentStatus::Shipped,
        "delivered" => ShipmentStatus::Delivered,
        "not_delivered" => ShipmentStatus::NotDelivered,
        "cancelled" => ShipmentStatus::Cancelled,
        // `pending` and any new one.
        _ => ShipmentStatus::Pending,
    }
}
