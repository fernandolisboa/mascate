//! Shipment confirmation on Amazon, for commerce's shipments port (#127):
//! Amazon's label API does not serve Brazil, so the owner ships with a
//! carrier of their own and confirms it with `confirmShipment` (Orders v0,
//! the only version that has it), naming the carrier and the tracking code.
//! The items and their units still to ship come from `getOrder` of the
//! Orders API 2026-01-01, read just before.

use chrono::SecondsFormat;
use mascate_commerce::{ChannelShipments, SentShipment};
use mascate_kernel::PlatformError;
use serde_json::json;

use super::answers::OrderFound;
use super::orders::SEARCH_PATH;
use super::{Amazon, MARKETPLACE, order_segment};

/// Brazil's carriers have no code of their own in the API reference: the
/// carrier goes by name under `Other`, which every store takes.
const OTHER_CARRIER: &str = "Other";

impl ChannelShipments for Amazon {
    /// Confirms every unit still to ship in one package. An Order with
    /// nothing left to ship is refused without telling Amazon anything.
    fn confirm_shipment(&self, order: &str, sent: &SentShipment) -> Result<(), PlatformError> {
        let order = order_segment(order)?;
        let found: OrderFound = self.get(
            &format!("{SEARCH_PATH}/{order}"),
            &[("includedData", "FULFILLMENT,PACKAGES")],
        )?;
        let items: Vec<serde_json::Value> = found
            .order
            .order_items
            .iter()
            .filter_map(|item| {
                let to_ship = item
                    .fulfillment
                    .as_ref()
                    .and_then(|fulfillment| fulfillment.quantity_unfulfilled)
                    .unwrap_or(item.quantity_ordered);
                (to_ship > 0)
                    .then(|| json!({"orderItemId": item.order_item_id, "quantity": to_ship}))
            })
            .collect();
        let status = found
            .order
            .fulfillment
            .as_ref()
            .map(|fulfillment| fulfillment.fulfillment_status.as_str());
        if items.is_empty() || !matches!(status, Some("UNSHIPPED" | "PARTIALLY_SHIPPED")) {
            return Err(PlatformError::Refused(
                "Amazon has nothing left to ship in this Order".into(),
            ));
        }
        // Unique within the Order, and only positive numbers.
        let package = found.order.packages.len() + 1;
        let body = json!({
            "marketplaceId": MARKETPLACE,
            "packageDetail": {
                "packageReferenceId": package.to_string(),
                "carrierCode": OTHER_CARRIER,
                "carrierName": sent.carrier,
                "trackingNumber": sent.tracking_code,
                "shipDate": sent.shipped_at.to_rfc3339_opts(SecondsFormat::Secs, true),
                "orderItems": items,
            },
        });
        self.post_without_answer(
            &format!("/orders/v0/orders/{order}/shipmentConfirmation"),
            &body.to_string(),
        )
    }
}
