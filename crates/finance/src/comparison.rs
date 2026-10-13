//! Reselling a Product against promoting it (#86, story 91): in a period,
//! the Realized Margin its sales left next to the Commissions earned
//! promoting it, per currency and never converted. The caller hands over
//! the sale lines (Commerce) and the earned Commissions of the products it
//! linked to the Product (Affiliate), the way the dashboard is fed (ADR
//! 0026, ADR 0030).

use std::cmp::Ordering;
use std::collections::BTreeMap;

use mascate_kernel::{Currency, CurrencyMismatch, Margin, Money, RecordId};
use rust_decimal::Decimal;

use crate::margins::{SoldLine, row};

/// An approved or paid Commission of a product the owner linked to a
/// Product.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkedCommission {
    pub product: RecordId,
    pub amount: Money,
}

/// What selling a Product left in one currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reselling {
    pub orders: usize,
    pub units: u32,
    pub revenue: Money,
    /// The margin of the lines that have a cost; `None` without one.
    pub margin: Option<Margin>,
    /// Lines left out of the margin for want of a cost.
    pub without_cost: usize,
    /// The margin over the units that have a cost.
    pub per_unit: Option<Money>,
}

/// What promoting a Product earned in one currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Promoting {
    pub commissions: usize,
    pub earned: Money,
    pub per_commission: Money,
}

/// Which side left more money in the period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ahead {
    Reselling,
    Promoting,
    Even,
}

/// A Product's sales and Commissions in one currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResellOrPromote {
    pub product: RecordId,
    pub currency: Currency,
    /// `None` when it sold nothing in this currency.
    pub reselling: Option<Reselling>,
    /// `None` when it earned no Commission in this currency.
    pub promoting: Option<Promoting>,
}

impl ResellOrPromote {
    /// One comparison per Product of `products` and currency with a sale or
    /// a Commission, in the order of `products` and then of the currency
    /// codes. Lines and Commissions of other Products are left out.
    pub fn of(
        products: &[RecordId],
        lines: &[SoldLine],
        commissions: &[LinkedCommission],
    ) -> Result<Vec<Self>, CurrencyMismatch> {
        let mut sold: BTreeMap<(RecordId, &'static str), Vec<&SoldLine>> = BTreeMap::new();
        for line in lines {
            if let Some(product) = line.product.id.filter(|id| products.contains(id)) {
                sold.entry((product, line.revenue.currency().code()))
                    .or_default()
                    .push(line);
            }
        }
        let mut earned: BTreeMap<(RecordId, &'static str), Vec<Money>> = BTreeMap::new();
        for commission in commissions {
            if products.contains(&commission.product) {
                earned
                    .entry((commission.product, commission.amount.currency().code()))
                    .or_default()
                    .push(commission.amount);
            }
        }
        let mut compared = Vec::new();
        for product in dedup(products) {
            let mut currencies: Vec<&'static str> = sold
                .keys()
                .chain(earned.keys())
                .filter(|(of, _)| *of == product)
                .map(|(_, code)| *code)
                .collect();
            currencies.sort_unstable();
            currencies.dedup();
            for code in currencies {
                let currency = Currency::from_code(code).expect("read from a Money");
                let reselling = sold
                    .get(&(product, code))
                    .map(|lines| reselling(lines, currency))
                    .transpose()?;
                let promoting = earned
                    .get(&(product, code))
                    .map(|amounts| promoting(amounts, currency))
                    .transpose()?;
                compared.push(Self {
                    product,
                    currency,
                    reselling,
                    promoting,
                });
            }
        }
        Ok(compared)
    }

    /// The side that left more money: the sales' Realized Margin against
    /// the Commissions earned, a side with nothing counting as zero. `None`
    /// while no sale has a cost to work the margin out.
    pub fn ahead(&self) -> Option<Ahead> {
        let (resold, promoted) = self.sides()?;
        Some(match resold.cmp(&promoted) {
            Ordering::Greater => Ahead::Reselling,
            Ordering::Less => Ahead::Promoting,
            Ordering::Equal => Ahead::Even,
        })
    }

    /// How much more the side ahead left; `None` when [`Self::ahead`] is.
    pub fn lead(&self) -> Option<Money> {
        let (resold, promoted) = self.sides()?;
        Some(Money::new((resold - promoted).abs(), self.currency))
    }

    /// The margin and the Commissions earned, nothing counting as zero.
    fn sides(&self) -> Option<(Decimal, Decimal)> {
        let resold = match &self.reselling {
            Some(reselling) => reselling.margin?.amount.amount(),
            None => Decimal::ZERO,
        };
        let promoted = self
            .promoting
            .map_or(Decimal::ZERO, |promoting| promoting.earned.amount());
        Some((resold, promoted))
    }
}

fn dedup(products: &[RecordId]) -> Vec<RecordId> {
    let mut seen = Vec::with_capacity(products.len());
    for product in products {
        if !seen.contains(product) {
            seen.push(*product);
        }
    }
    seen
}

fn reselling(lines: &[&SoldLine], currency: Currency) -> Result<Reselling, CurrencyMismatch> {
    let sales = row((), lines, currency)?;
    let costed_units: u32 = lines
        .iter()
        .filter(|line| line.cost.is_some())
        .map(|line| line.units)
        .sum();
    let per_unit = sales.margin.filter(|_| costed_units > 0).map(|margin| {
        Money::new(
            margin.amount.amount() / Decimal::from(costed_units),
            currency,
        )
    });
    Ok(Reselling {
        orders: sales.orders,
        units: sales.units,
        revenue: sales.revenue,
        margin: sales.margin,
        without_cost: sales.without_cost,
        per_unit,
    })
}

fn promoting(amounts: &[Money], currency: Currency) -> Result<Promoting, CurrencyMismatch> {
    let earned = Money::sum(currency, amounts.iter().copied())?;
    let count = amounts.len();
    Ok(Promoting {
        commissions: count,
        earned,
        per_commission: Money::new(earned.amount() / Decimal::from(count), currency),
    })
}
