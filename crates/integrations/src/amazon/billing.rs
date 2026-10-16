//! The Fees Amazon charged on each Order, for commerce's billing port (ADR
//! 0020): the transactions of the Finances API 2024-06-19
//! (`listTransactions`) filtered by the Order's id. A transaction's
//! `Expenses` breakdown holds what Amazon charged, negative, and what it
//! gave back, positive, as on a refund; its sales, taxes and refunds to the
//! buyer are not Fees. Amazon may take 48 hours to post an Order's
//! transactions: until then the Order comes without charges, and its
//! margin stays provisional.

use std::time::Duration;

use mascate_commerce::{BilledOrder, ChannelBilling, ChannelFee, FeeKind};
use mascate_kernel::{Currency, Money, PlatformError};

use super::answers::{Breakdown, Transaction, TransactionsAnswer};
use super::{Amazon, MARKETPLACE, order_segment};

const TRANSACTIONS_PATH: &str = "/finances/2024-06-19/transactions";

/// `listTransactions` allows one call every two seconds after a burst.
const BETWEEN_ORDERS: Duration = Duration::from_secs(2);

/// The breakdown that holds what Amazon charged.
const EXPENSES: &str = "expenses";

impl ChannelBilling for Amazon {
    fn billed_fees(&self, orders: &[String]) -> Result<Vec<BilledOrder>, PlatformError> {
        let mut billed = Vec::new();
        for (at, order) in orders.iter().enumerate() {
            if at > 0 {
                self.pause.pause(BETWEEN_ORDERS);
            }
            let fees = match self.order_fees(order_segment(order)?) {
                // No transaction posted for the Order yet.
                Err(PlatformError::NotFound) => Vec::new(),
                answer => answer?,
            };
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

impl Amazon {
    /// Every charge of every transaction of `order`, page by page.
    fn order_fees(&self, order: &str) -> Result<Vec<ChannelFee>, PlatformError> {
        let mut fees = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut query = vec![
                ("relatedIdentifierName", "ORDER_ID"),
                ("relatedIdentifierValue", order),
                ("marketplaceId", MARKETPLACE),
            ];
            if let Some(token) = &next_token {
                query.push(("nextToken", token));
            }
            let answer: TransactionsAnswer = self.get(TRANSACTIONS_PATH, &query)?;
            let Some(payload) = answer.payload else {
                return Ok(fees);
            };
            for transaction in &payload.transactions {
                if is_of(transaction, order) {
                    fees.extend(transaction_fees(transaction));
                }
            }
            match payload.next_token {
                // A token that repeats would read the same page forever.
                Some(next) if next_token.as_ref() != Some(&next) => next_token = Some(next),
                _ => return Ok(fees),
            }
        }
    }
}

/// Whether `transaction` is of `order`, or names no Order at all, as the
/// filter promises.
fn is_of(transaction: &Transaction, order: &str) -> bool {
    let named: Vec<&str> = transaction
        .related_identifiers
        .iter()
        .filter(|identifier| identifier.related_identifier_name.as_deref() == Some("ORDER_ID"))
        .filter_map(|identifier| identifier.related_identifier_value.as_deref())
        .collect();
    named.is_empty() || named.contains(&order)
}

/// The charges under the transaction's `Expenses`: each innermost
/// breakdown is one Fee, by the transaction and the breakdown's place.
fn transaction_fees(transaction: &Transaction) -> Vec<ChannelFee> {
    let id = transaction.transaction_id.clone().unwrap_or_else(|| {
        format!(
            "{}|{}",
            transaction.posted_date.as_deref().unwrap_or_default(),
            transaction.description.as_deref().unwrap_or_default()
        )
    });
    let mut breakdowns: Vec<&Breakdown> = transaction.breakdowns.list().iter().collect();
    if breakdowns.is_empty() {
        breakdowns = transaction
            .items
            .iter()
            .flat_map(|item| &item.breakdowns)
            .collect();
    }
    let mut fees = Vec::new();
    for breakdown in breakdowns {
        if folded(breakdown) == EXPENSES {
            charges(breakdown, &mut vec![type_of(breakdown)], &id, &mut fees);
        }
    }
    fees
}

/// The innermost breakdowns under `breakdown`, at `path`, as Fees: an
/// expense Amazon charged is a positive Fee, one it gave back a negative.
fn charges(breakdown: &Breakdown, path: &mut Vec<String>, id: &str, fees: &mut Vec<ChannelFee>) {
    if !breakdown.breakdowns.is_empty() {
        for inner in &breakdown.breakdowns {
            path.push(type_of(inner));
            charges(inner, path, id, fees);
            path.pop();
        }
        return;
    }
    let Some(amount) = &breakdown.breakdown_amount else {
        return;
    };
    let (Some(currency), Some(value)) = (
        Currency::from_code(&amount.currency_code),
        amount.currency_amount.decimal(),
    ) else {
        return;
    };
    if value.is_zero() {
        return;
    }
    let mut charge = format!("{id}:{}", path.join("/"));
    let repeated = fees
        .iter()
        .filter(|fee| fee.id == charge || fee.id.starts_with(&format!("{charge}#")))
        .count();
    if repeated > 0 {
        charge.push_str(&format!("#{}", repeated + 1));
    }
    let name = path.last().cloned().unwrap_or_default();
    fees.push(ChannelFee {
        id: charge,
        kind: fee_kind(&name),
        description: name,
        amount: Money::new(-value, currency),
    });
}

/// The referral fee, Amazon's commission on the sale, is the sale fee;
/// anything about shipping is shipping; the rest, other Fees.
fn fee_kind(breakdown_type: &str) -> FeeKind {
    let folded = fold(breakdown_type);
    if folded.contains("commission") || folded.contains("referral") {
        FeeKind::SaleFee
    } else if folded.contains("shipping") {
        FeeKind::Shipping
    } else {
        FeeKind::Other
    }
}

fn type_of(breakdown: &Breakdown) -> String {
    breakdown
        .breakdown_type
        .clone()
        .unwrap_or_else(|| "Other".into())
}

fn folded(breakdown: &Breakdown) -> String {
    fold(breakdown.breakdown_type.as_deref().unwrap_or_default())
}

/// Amazon writes the same type as `AmazonFees` or `Amazon fees`.
fn fold(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}
