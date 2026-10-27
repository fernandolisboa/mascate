//! The period's affiliate Commissions on the business dashboard, next to
//! the sales' Realized Margin (#81): the approved and paid ones the caller
//! read from Affiliate (ADR 0026, ADR 0030), less the Promotion Spends it
//! read from the Marketing (#196, ADR 0037), summed per currency and never
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

/// The earned Commissions and the Promotion Spends in one currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommissionIncome {
    pub currency: Currency,
    pub approved: Money,
    pub paid: Money,
    /// What promoting cost.
    pub spent: Money,
}

impl CommissionIncome {
    /// `commissions` and `spends` summed per currency, in the order of the
    /// currency codes.
    pub fn of(commissions: &[EarnedCommission], spends: &[Money]) -> Vec<Self> {
        let mut by_currency: BTreeMap<&'static str, Self> = BTreeMap::new();
        let mut add = |amount: Money, into: fn(&mut Self) -> &mut Money| {
            let currency = amount.currency();
            let income = by_currency.entry(currency.code()).or_insert(Self {
                currency,
                approved: Money::zero(currency),
                paid: Money::zero(currency),
                spent: Money::zero(currency),
            });
            let into = into(income);
            *into = Money::new(into.amount() + amount.amount(), currency);
        };
        for commission in commissions {
            if commission.paid {
                add(commission.amount, |income| &mut income.paid);
            } else {
                add(commission.amount, |income| &mut income.approved);
            }
        }
        for spend in spends {
            add(*spend, |income| &mut income.spent);
        }
        by_currency.into_values().collect()
    }

    /// Approved and paid.
    pub fn earned(&self) -> Money {
        Money::new(self.approved.amount() + self.paid.amount(), self.currency)
    }

    /// What affiliation brought in: earned less what promoting cost.
    pub fn net(&self) -> Money {
        Money::new(self.earned().amount() - self.spent.amount(), self.currency)
    }
}
