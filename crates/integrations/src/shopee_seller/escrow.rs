//! What Shopee keeps of each Order and what it pays the seller, for
//! commerce's billing and wallet ports (ADR 0020, ADR 0026): the escrow of
//! `get_escrow_detail`. Shopee changes the escrow until the Order is
//! completed, so until then the Order comes without Fees and its margin
//! stays provisional, and its money is still held; once completed, the
//! escrow's Fees are the real ones and the money is released to the
//! seller's Shopee balance.

use mascate_commerce::{
    BilledOrder, ChannelBilling, ChannelFee, ChannelPayments, FeeKind, PaymentRelease,
};
use mascate_kernel::{Currency, Money, PlatformError};
use rust_decimal::Decimal;

use super::ShopeeSeller;
use super::answers::{Escrow, OrderDetail, OrderIncome};

const ESCROW_DETAIL: &str = "/api/v2/payment/get_escrow_detail";

/// The status of an Order whose escrow Shopee settled.
const COMPLETED: &str = "COMPLETED";

impl ChannelBilling for ShopeeSeller {
    /// The escrow's Fees of each of `orders` Shopee completed.
    fn billed_fees(&self, orders: &[String]) -> Result<Vec<BilledOrder>, PlatformError> {
        let mut billed = Vec::new();
        for detail in self.order_details(orders)? {
            if detail.order_status != COMPLETED {
                continue;
            }
            let Some((income, currency)) = self.income(&detail)? else {
                continue;
            };
            let fees = escrow_fees(&income, currency);
            if !fees.is_empty() {
                billed.push(BilledOrder {
                    order: detail.order_sn,
                    fees,
                });
            }
        }
        Ok(billed)
    }
}

impl ChannelPayments for ShopeeSeller {
    /// The escrow of each of `orders` that pays the seller something, as
    /// one payment by the Order's number: released, and dated by the
    /// Order's last change, once Shopee completed it; held and undated
    /// before. An unpaid or cancelled Order pays nothing.
    fn releases(&self, orders: &[String]) -> Result<Vec<PaymentRelease>, PlatformError> {
        let mut releases = Vec::new();
        for detail in self.order_details(orders)? {
            if matches!(
                detail.order_status.as_str(),
                "UNPAID" | "PENDING" | "CANCELLED"
            ) {
                continue;
            }
            let Some((income, currency)) = self.income(&detail)? else {
                continue;
            };
            let Some(amount) = income
                .escrow_amount_after_adjustment
                .as_ref()
                .or(income.escrow_amount.as_ref())
                .and_then(crate::answers::decimal)
                .filter(|amount| *amount > Decimal::ZERO)
            else {
                continue;
            };
            let released = detail.order_status == COMPLETED;
            releases.push(PaymentRelease {
                order: detail.order_sn.clone(),
                payment: detail.order_sn,
                amount: Money::new(amount, currency),
                released,
                release_at: if released {
                    detail
                        .update_time
                        .and_then(|at| chrono::DateTime::from_timestamp(at, 0))
                } else {
                    None
                },
            });
        }
        Ok(releases)
    }
}

impl ShopeeSeller {
    /// The escrow of the Order, in its currency; `None` while Shopee has
    /// none.
    fn income(
        &self,
        detail: &OrderDetail,
    ) -> Result<Option<(OrderIncome, Currency)>, PlatformError> {
        let Some(currency) = detail.currency.as_deref().and_then(Currency::from_code) else {
            return Ok(None);
        };
        match self.get::<Escrow>(ESCROW_DETAIL, &[("order_sn", &detail.order_sn)]) {
            Ok(escrow) => Ok(escrow.order_income.map(|income| (income, currency))),
            Err(PlatformError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

/// The escrow's charges on the seller, each by its field so a second read
/// changes nothing: the commission, the service and per-order fees, the
/// transaction fee, the affiliates' commission, a campaign's fee, and what
/// the seller bears of the shipping, there and back.
fn escrow_fees(income: &OrderIncome, currency: Currency) -> Vec<ChannelFee> {
    let amount = |number: &Option<serde_json::Number>| {
        number
            .as_ref()
            .and_then(crate::answers::decimal)
            .unwrap_or(Decimal::ZERO)
    };
    // What the parcel cost less what the buyer, Shopee and the carrier
    // covered; negative when they covered more.
    let shipping = amount(&income.actual_shipping_fee)
        - amount(&income.buyer_paid_shipping_fee)
        - amount(&income.shopee_shipping_rebate)
        - amount(&income.shipping_fee_discount_from_3pl);
    [
        (
            "commission_fee",
            FeeKind::SaleFee,
            "Comissão",
            amount(&income.commission_fee),
        ),
        (
            "service_fee",
            FeeKind::SaleFee,
            "Taxa de serviço",
            amount(&income.service_fee),
        ),
        (
            "seller_order_processing_fee",
            FeeKind::SaleFee,
            "Tarifa por pedido",
            amount(&income.seller_order_processing_fee),
        ),
        (
            "seller_transaction_fee",
            FeeKind::Other,
            "Taxa de transação",
            amount(&income.seller_transaction_fee),
        ),
        (
            "order_ams_commission_fee",
            FeeKind::Other,
            "Comissão de afiliados",
            amount(&income.order_ams_commission_fee),
        ),
        (
            "campaign_fee",
            FeeKind::Other,
            "Taxa de campanha",
            amount(&income.campaign_fee),
        ),
        ("shipping", FeeKind::Shipping, "Frete", shipping),
        (
            "reverse_shipping_fee",
            FeeKind::Shipping,
            "Frete da devolução",
            amount(&income.reverse_shipping_fee),
        ),
        (
            "final_return_to_seller_shipping_fee",
            FeeKind::Shipping,
            "Frete da entrega que não deu certo",
            amount(&income.final_return_to_seller_shipping_fee),
        ),
    ]
    .into_iter()
    .filter(|(_, _, _, value)| !value.is_zero())
    .map(|(id, kind, description, value)| ChannelFee {
        id: id.to_owned(),
        kind,
        description: description.to_owned(),
        amount: Money::new(value, currency),
    })
    .collect()
}
