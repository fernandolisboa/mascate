//! The period's affiliate Commissions on the business dashboard, next to
//! the sales' Realized Margin (#81): the approved and paid ones the caller
//! read from Affiliate (ADR 0026, ADR 0030), summed per currency and never
//! converted.

use std::collections::BTreeMap;

use mascate_kernel::{Currency, Money};

/// A Commission the owner earned, as the caller read it from Affiliate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EarnedCommission {
    pub amount: Money,
    /// Already paid to the owner; else approved and still to be paid.
    pub paid: bool,
}

/// The earned Commissions in one currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommissionIncome {
    pub currency: Currency,
    pub approved: Money,
    pub paid: Money,
}

impl CommissionIncome {
    /// `commissions` summed per currency, in the order of the currency codes.
    pub fn of(commissions: &[EarnedCommission]) -> Vec<Self> {
        let mut by_currency: BTreeMap<&'static str, Self> = BTreeMap::new();
        for commission in commissions {
            let currency = commission.amount.currency();
            let income = by_currency.entry(currency.code()).or_insert(Self {
                currency,
                approved: Money::zero(currency),
                paid: Money::zero(currency),
            });
            let into = if commission.paid {
                &mut income.paid
            } else {
                &mut income.approved
            };
            *into = Money::new(into.amount() + commission.amount.amount(), currency);
        }
        by_currency.into_values().collect()
    }

    /// Approved and paid.
    pub fn earned(&self) -> Money {
        Money::new(self.approved.amount() + self.paid.amount(), self.currency)
    }
}
