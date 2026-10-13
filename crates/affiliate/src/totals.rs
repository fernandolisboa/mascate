//! Commissions summed up by currency (ADR 0030): a Platform may pay in
//! dollars, and a rate the app made up would be money nobody received, so
//! nothing is ever converted.

use mascate_kernel::{Currency, Money};

use crate::commissions::Commission;
use crate::source::CommissionStatus;

/// The Commissions in one currency, by status. Cancelled ones only count:
/// there never was money in them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrencyTotals {
    pub currency: Currency,
    pub pending: Money,
    pub approved: Money,
    pub paid: Money,
    pub reversed: Money,
    pub cancelled: usize,
}

impl CurrencyTotals {
    fn new(currency: Currency) -> Self {
        Self {
            currency,
            pending: Money::zero(currency),
            approved: Money::zero(currency),
            paid: Money::zero(currency),
            reversed: Money::zero(currency),
            cancelled: 0,
        }
    }

    /// What the owner earned: approved and paid, reversals left out.
    pub fn earned(&self) -> Money {
        Money::new(self.approved.amount() + self.paid.amount(), self.currency)
    }

    /// The sum of the Commissions in `status`; `None` for cancelled ones,
    /// which hold no money.
    pub fn of(&self, status: CommissionStatus) -> Option<Money> {
        match status {
            CommissionStatus::Pending => Some(self.pending),
            CommissionStatus::Approved => Some(self.approved),
            CommissionStatus::Paid => Some(self.paid),
            CommissionStatus::Reversed => Some(self.reversed),
            CommissionStatus::Cancelled => None,
        }
    }

    fn add(&mut self, commission: &Commission) {
        let amount = commission.amount.amount();
        let into = match commission.status {
            CommissionStatus::Pending => &mut self.pending,
            CommissionStatus::Approved => &mut self.approved,
            CommissionStatus::Paid => &mut self.paid,
            CommissionStatus::Reversed => &mut self.reversed,
            CommissionStatus::Cancelled => {
                self.cancelled += 1;
                return;
            }
        };
        *into = Money::new(into.amount() + amount, self.currency);
    }
}

/// `commissions` summed up per currency, in the order of the currency codes.
pub fn totals_by_currency(commissions: &[Commission]) -> Vec<CurrencyTotals> {
    let mut totals: Vec<CurrencyTotals> = Vec::new();
    for commission in commissions {
        let currency = commission.amount.currency();
        let index = match totals.iter().position(|total| total.currency == currency) {
            Some(index) => index,
            None => {
                totals.push(CurrencyTotals::new(currency));
                totals.len() - 1
            }
        };
        totals[index].add(commission);
    }
    totals.sort_by_key(|total| total.currency.code());
    totals
}
