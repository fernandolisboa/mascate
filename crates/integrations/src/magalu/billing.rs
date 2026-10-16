//! What Magalu kept of each Order, for commerce's billing port (ADR 0020):
//! the transactions of the financial analysis, asked by the Order's number.
//! Magalu has them once the Order is delivered; until then the Order comes
//! without Fees and its margin stays provisional, with the commission the
//! Order itself reports.

use mascate_commerce::{BilledOrder, ChannelBilling, ChannelFee, FeeKind};
use mascate_kernel::{Currency, Money, PlatformError};

use super::answers::{AnalysisPage, Transaction};
use super::{MAX_OFFSET, Magalu, in_units};

const FINANCIAL_ANALYSIS: &str = "/seller/v1/financial-analysis/orders";

/// Analyses per page; the most Magalu allows.
const PAGE_SIZE: usize = 100;

impl ChannelBilling for Magalu {
    /// The Fees of each of `orders` the financial analysis has; an Order
    /// number that is not Magalu's is never asked for.
    fn billed_fees(&self, orders: &[String]) -> Result<Vec<BilledOrder>, PlatformError> {
        let mut billed = Vec::new();
        for order in orders.iter().filter(|order| is_order_code(order)) {
            let fees = self.order_fees(order)?;
            if !fees.is_empty() {
                billed.push(BilledOrder {
                    order: order.clone(),
                    fees,
                });
            }
        }
        Ok(billed)
    }
}

impl Magalu {
    /// The Fees in every transaction of the Order's analyses, page by page.
    fn order_fees(&self, order: &str) -> Result<Vec<ChannelFee>, PlatformError> {
        let limit = PAGE_SIZE.to_string();
        let mut transactions = Vec::new();
        let mut offset = 0;
        loop {
            let page: AnalysisPage = self.get(
                FINANCIAL_ANALYSIS,
                &[
                    ("order_id", order),
                    ("_offset", &offset.to_string()),
                    ("_limit", &limit),
                ],
            )?;
            let analyses = page.results.unwrap_or_default();
            let count = analyses.len();
            for analysis in analyses {
                let of_order = analysis
                    .extras
                    .and_then(|extras| extras.order_code)
                    .is_none_or(|code| code == order);
                if of_order {
                    transactions.extend(analysis.transactions.unwrap_or_default());
                }
            }
            offset += count;
            if count < PAGE_SIZE || offset >= MAX_OFFSET {
                break;
            }
        }
        let mut fees: Vec<ChannelFee> = Vec::new();
        for transaction in &transactions {
            let Some(mut fee) = fee(transaction) else {
                continue;
            };
            // Magalu gives a transaction no id of its own: what it is and
            // what it is on, numbered when the same comes twice, so a
            // second read keeps the same ids.
            let repeated = fees
                .iter()
                .filter(|known| known.id == fee.id || known.id.starts_with(&format!("{}#", fee.id)))
                .count();
            if repeated > 0 {
                fee.id = format!("{}#{}", fee.id, repeated + 1);
            }
            fees.push(fee);
        }
        Ok(fees)
    }
}

/// An Order number as Magalu gives it: digits only.
fn is_order_code(order: &str) -> bool {
    !order.is_empty() && order.len() <= 32 && order.bytes().all(|byte| byte.is_ascii_digit())
}

/// The Fee a transaction is, as a charge on the seller (positive for a
/// debit, negative for what Magalu gives back or covers); `None` for the
/// sale itself, what goes back to the buyer (the Order's refund), the
/// freight the buyer paid the store, a fixed-price discount already in the
/// price, a retention Magalu reports only, and lines for information.
fn fee(transaction: &Transaction) -> Option<ChannelFee> {
    let debit = match transaction.kind.as_str() {
        "DEBIT" => true,
        "CREDIT" => false,
        _ => return None,
    };
    let subcategory = transaction.subcategory.as_deref().unwrap_or_default();
    let (kind, description) = match (transaction.category.as_str(), subcategory) {
        ("SALE", _) | ("REFUND", "ORDER" | "PENALTY") | (_, "ABSOLUTE_DISCOUNT") => return None,
        ("SHIPPING_COST", _) if !debit => return None,
        ("COMMISSION", "TECHNOLOGY") => (FeeKind::SaleFee, "Comissão de tecnologia"),
        ("COMMISSION", "FREIGHT") => (FeeKind::SaleFee, "Comissão sobre o frete"),
        ("COMMISSION", _) => (FeeKind::SaleFee, "Comissão"),
        ("FEES" | "FEE", "PLATFORM") => (FeeKind::SaleFee, "Tarifa fixa"),
        ("FEES" | "FEE", "FREIGHT") => (FeeKind::Other, "Taxa de pagamento do frete"),
        ("FEES" | "FEE", "PAYMENT_PROCESSING") => (FeeKind::Other, "Taxa de pagamento"),
        ("FEES" | "FEE", _) => (FeeKind::Other, "Tarifa"),
        ("REFUND", "PAYMENT") => (FeeKind::Other, "Taxa de pagamento devolvida"),
        ("SHIPPING_COST", _) => (FeeKind::Shipping, "Frete"),
        ("SHIPPING_SHARE" | "SHIPPING_SUBSIDY", _) => (FeeKind::Shipping, "Frete pago pela Magalu"),
        // The seller's share of a coupon or promotion, and what Magalu
        // covers of it.
        ("DISCOUNT" | "PROMOTION", _) => (FeeKind::Other, "Desconto"),
        ("TAXES", _) => (FeeKind::Other, "Imposto retido"),
        _ => (FeeKind::Other, ""),
    };
    let description = Some(description)
        .filter(|words| !words.is_empty())
        .or(transaction.description.as_deref())
        .unwrap_or("Outra tarifa");
    let currency = Currency::from_code(&transaction.currency)?;
    let value = in_units(transaction.value, transaction.normalizer)?;
    if value.is_zero() {
        return None;
    }
    let entity = transaction.entity.as_ref();
    let id = [
        transaction.category.as_str(),
        subcategory,
        transaction.identifier.as_deref().unwrap_or_default(),
        entity
            .and_then(|entity| entity.kind.as_deref())
            .unwrap_or_default(),
        entity
            .and_then(|entity| entity.id.as_deref())
            .unwrap_or_default(),
        transaction.kind.as_str(),
    ]
    .join(":");
    Some(ChannelFee {
        id,
        kind,
        description: description.to_owned(),
        amount: Money::new(if debit { value.abs() } else { -value.abs() }, currency),
    })
}
