//! The Fees Mercado Livre billed on each Order, for commerce's billing port
//! (ADR 0020): `/billing/integration/group/ML/order/details`, up to 60
//! Orders a call, as Mercado Livre's billing guide allows. Commerce asks
//! only for Orders it has not seen billed, or that changed, since the guide
//! warns against asking again for Orders already processed.

use super::MercadoLivre;
use super::answers::{BillingAnswer, BillingDetail, decimal};
use super::orders::whole_id;
use super::sales_channel::path_id;
use mascate_commerce::{BilledOrder, ChannelBilling, ChannelFee, FeeKind};
use mascate_kernel::{Currency, Money, PlatformError};

/// The most Orders one call may ask for.
const ORDERS_PER_CALL: usize = 60;

impl ChannelBilling for MercadoLivre {
    /// While Mercado Livre is still putting a bill together (206), the
    /// Orders asked for count as not billed yet.
    fn billed_fees(&self, orders: &[String]) -> Result<Vec<BilledOrder>, PlatformError> {
        let mut billed = Vec::new();
        for asked in orders.chunks(ORDERS_PER_CALL) {
            let ids = asked
                .iter()
                .map(|order| path_id(order))
                .collect::<Result<Vec<_>, _>>()?
                .join(",");
            let answer: Option<BillingAnswer> = self.get_complete(
                "/billing/integration/group/ML/order/details",
                &[("order_ids", &ids)],
            )?;
            for found in answer.into_iter().flat_map(|answer| answer.results) {
                billed.push(BilledOrder {
                    order: whole_id(&found.order_id),
                    fees: found.details.iter().filter_map(channel_fee).collect(),
                });
            }
        }
        Ok(billed)
    }
}

/// A charge as commerce keeps it; `None` without an amount.
fn channel_fee(detail: &BillingDetail) -> Option<ChannelFee> {
    let charge = &detail.charge_info;
    let amount = decimal(&charge.detail_amount)?.abs();
    let currency = detail
        .currency_info
        .as_ref()
        .and_then(|info| info.currency_id.as_deref())
        .map_or(Some(Currency::Brl), Currency::from_code)?;
    let given_back = charge.detail_type.as_deref() == Some("BONUS");
    let sub_type = charge.detail_sub_type.as_deref().unwrap_or_default();
    let marketplace = detail
        .marketplace_info
        .as_ref()
        .and_then(|info| info.marketplace.as_deref());
    Some(ChannelFee {
        id: format!("{}-{sub_type}", whole_id(&charge.detail_id)),
        kind: fee_kind(sub_type, marketplace),
        description: charge
            .transaction_detail
            .clone()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| sub_type.to_owned()),
        amount: Money::new(if given_back { -amount } else { amount }, currency),
    })
}

/// `CV` and `BV` are the sale fee and its bonus; Mercado Envios charges
/// come from the `SHIPPING` marketplace, as `CXD` and `BXD`.
fn fee_kind(sub_type: &str, marketplace: Option<&str>) -> FeeKind {
    match sub_type {
        "CV" | "BV" => FeeKind::SaleFee,
        _ if marketplace == Some("SHIPPING")
            || sub_type.starts_with("CX")
            || sub_type.starts_with("BX") =>
        {
            FeeKind::Shipping
        }
        _ => FeeKind::Other,
    }
}
