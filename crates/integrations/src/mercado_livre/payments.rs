//! When the money of each Order is released, for commerce's wallet port
//! (ADR 0026): the Order's payments from `/orders/{id}`, then each approved
//! one from the Mercado Pago's `/v1/payments/{id}`, which the seller's
//! Mercado Livre login can read.

use mascate_commerce::{ChannelPayments, PaymentRelease};
use mascate_kernel::{Currency, Money, PlatformError};

use super::MercadoLivre;
use super::answers::{OrderAnswer, WalletPaymentAnswer, decimal, time, whole_id};
use super::sales_channel::path_id;

impl ChannelPayments for MercadoLivre {
    /// An Order Mercado Livre no longer has is left out.
    fn releases(&self, orders: &[String]) -> Result<Vec<PaymentRelease>, PlatformError> {
        let mut releases = Vec::new();
        for order in orders {
            let found: OrderAnswer = match self.get(&format!("/orders/{}", path_id(order)?), &[]) {
                Err(PlatformError::NotFound) => continue,
                other => other?,
            };
            let approved = found
                .payments
                .iter()
                .filter(|payment| payment.status.as_deref() == Some("approved"))
                .filter_map(|payment| payment.id.as_ref().map(whole_id));
            for payment in approved {
                let answer: WalletPaymentAnswer = self.get_payment(&payment)?;
                releases.extend(release(order, answer));
            }
        }
        Ok(releases)
    }
}

/// The money of a payment as commerce keeps it; `None` for one that brings
/// none, refunded or cancelled since the Order was read, or without an
/// amount.
fn release(order: &str, answer: WalletPaymentAnswer) -> Option<PaymentRelease> {
    if answer.status.as_deref() != Some("approved") {
        return None;
    }
    let currency = answer
        .currency_id
        .as_deref()
        .map_or(Some(Currency::Brl), Currency::from_code)?;
    let net = answer
        .transaction_details
        .as_ref()
        .and_then(|details| decimal(&details.net_received_amount))
        .filter(|net| !net.is_zero());
    let amount = net.or_else(|| decimal(&answer.transaction_amount))?;
    Some(PaymentRelease {
        order: order.to_owned(),
        payment: whole_id(&answer.id),
        amount: Money::new(amount, currency),
        released: answer.money_release_status.as_deref() == Some("released"),
        release_at: answer.money_release_date.as_deref().and_then(time),
    })
}

impl MercadoLivre {
    /// The Mercado Pago's payment `id`.
    fn get_payment(&self, id: &str) -> Result<WalletPaymentAnswer, PlatformError> {
        let path = format!("/v1/payments/{}", path_id(id)?);
        let url = format!("{}{path}", self.payments_api);
        let response = self.signed(&path, |bearer| {
            self.agent
                .get(&url)
                .header("Accept", "application/json")
                .header("Authorization", bearer)
                .call()
        })?;
        super::read_json(response)
    }
}
