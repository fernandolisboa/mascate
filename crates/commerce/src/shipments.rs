//! Shipment confirmation (#127): on a Sales Channel that prints no label
//! for the owner, as Amazon in Brazil, the owner ships through a carrier of
//! their own and tells the channel the carrier and the tracking code, so the
//! buyer can follow the package (ADR 0033). An Order is confirmed once:
//! one already shipped, cancelled or not yet ready is refused before the
//! channel is asked.

use libsql::params;
use mascate_kernel::{PlatformError, RecordId, Timestamp};
use mascate_platform::stored;

use crate::orders::{FirstSyncs, load_orders};
use crate::{OnSalesChannel, OrderError, OrderStatus, Orders, ShipmentStatus};

/// The longest carrier name and tracking code the app sends.
const MAX_CARRIER_CHARS: usize = 64;
const MAX_TRACKING_CHARS: usize = 64;

/// A package the owner handed to a carrier, as the channel is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentShipment {
    /// The carrier's name, as "Correios".
    pub carrier: String,
    pub tracking_code: String,
    pub shipped_at: Timestamp,
}

/// Where the owner confirms the shipment of their Orders, as a Platform's
/// adapter reports it. Calls block on the network, so they run off the UI
/// thread.
pub trait ChannelShipments: OnSalesChannel + Send + Sync {
    /// Tells the channel that every item of the Order `order` (the
    /// channel's id) still to ship left in `sent`.
    fn confirm_shipment(&self, order: &str, sent: &SentShipment) -> Result<(), PlatformError>;
}

impl Orders {
    /// Tells the Order's channel that it left with `carrier` under
    /// `tracking_code`, now, and keeps it as shipped until the next Sync
    /// brings the channel's word. Refused when the Order is on another
    /// channel, cancelled, already shipped or not ready to ship, or without
    /// a carrier or a tracking code.
    pub async fn confirm_shipment(
        &self,
        shipments: &dyn ChannelShipments,
        order: RecordId,
        carrier: &str,
        tracking_code: &str,
    ) -> Result<SentShipment, OrderError> {
        let carrier = carrier.trim();
        let tracking_code = tracking_code.trim();
        if !is_sendable(carrier, MAX_CARRIER_CHARS)
            || !is_sendable(tracking_code, MAX_TRACKING_CHARS)
        {
            return Err(OrderError::NoTracking);
        }
        let found = load_orders(self.database.connection(), Some(order), &FirstSyncs::new())
            .await?
            .pop()
            .ok_or(OrderError::UnknownOrder(order))?;
        if found.channel != shipments.sales_channel() {
            return Err(OrderError::OtherChannel {
                order: found.channel,
                channel: shipments.sales_channel(),
            });
        }
        let ready = found.sold.status != OrderStatus::Cancelled
            && found
                .sold
                .shipment
                .as_ref()
                .is_some_and(|shipment| shipment.status == ShipmentStatus::ReadyToShip);
        if !ready {
            return Err(OrderError::NotAwaitingShipment);
        }
        let now = self.clock.now();
        let sent = SentShipment {
            carrier: carrier.to_owned(),
            tracking_code: tracking_code.to_owned(),
            shipped_at: now,
        };
        shipments.confirm_shipment(&found.sold.id, &sent)?;
        self.database
            .connection()
            .execute(
                "UPDATE commerce_orders SET shipment_status = ?1, updated_at = ?2
                 WHERE id = ?3 AND deleted_at IS NULL",
                params![
                    ShipmentStatus::Shipped.code(),
                    stored(now),
                    order.to_string()
                ],
            )
            .await?;
        Ok(sent)
    }
}

/// Whether `text` is something to send: present, short enough, and with no
/// line breaks or other control characters.
fn is_sendable(text: &str, max_chars: usize) -> bool {
    !text.is_empty() && text.chars().count() <= max_chars && !text.chars().any(char::is_control)
}
