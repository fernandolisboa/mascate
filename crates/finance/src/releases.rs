//! What the Mercado Pago holds of the sales and what it already released
//! (story 75), from each Order's Money Releases as the caller read them
//! from Commerce (ADR 0026).

use std::collections::BTreeMap;
use std::ops::Range;

use chrono::NaiveDate;
use mascate_kernel::{Currency, CurrencyMismatch, Money, Timestamp, channel_day};

/// One payment's money, as the wallet reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoneyRelease {
    pub amount: Money,
    pub released: bool,
    /// When it was released or is due to be; `None` while the wallet has
    /// no date yet, as before the buyer gets the item.
    pub on: Option<Timestamp>,
}

/// Money still held, when it comes, and what was released in a period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseSummary {
    /// Every payment not released yet, whenever it was sold.
    pub pending: Money,
    /// The pending money with a date, by day of the Sales Channel, soonest
    /// first.
    pub schedule: Vec<(NaiveDate, Money)>,
    /// The pending money without a date yet.
    pub undated: Money,
    /// What was released within the period.
    pub released: Money,
}

impl ReleaseSummary {
    pub fn of(
        releases: &[MoneyRelease],
        during: Range<Timestamp>,
        currency: Currency,
    ) -> Result<Self, CurrencyMismatch> {
        let pending: Vec<&MoneyRelease> = releases.iter().filter(|r| !r.released).collect();
        let mut by_day: BTreeMap<NaiveDate, Money> = BTreeMap::new();
        for release in &pending {
            if let Some(on) = release.on {
                let day = by_day
                    .entry(channel_day(on))
                    .or_insert(Money::zero(currency));
                *day = day.checked_add(release.amount)?;
            }
        }
        Ok(Self {
            pending: Money::sum(currency, pending.iter().map(|r| r.amount))?,
            schedule: by_day.into_iter().collect(),
            undated: Money::sum(
                currency,
                pending.iter().filter(|r| r.on.is_none()).map(|r| r.amount),
            )?,
            released: Money::sum(
                currency,
                releases
                    .iter()
                    .filter(|r| r.released && r.on.is_some_and(|on| during.contains(&on)))
                    .map(|r| r.amount),
            )?,
        })
    }
}
