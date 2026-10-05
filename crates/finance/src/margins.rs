//! Realized Margin by Product and by category (story 72): the lines of a
//! period's sales, as the caller worked them out from Commerce and the
//! catalog (ADR 0026), summed per Product and per category of the Sales
//! Channel, the ones that lose money marked.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use mascate_kernel::{Currency, CurrencyMismatch, Margin, Money, RecordId};

/// What a Product sold as: the Product, or the listing's title for a
/// listing not linked to one yet.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SoldProduct {
    /// `None` for a listing without a Product, which counts by its title.
    pub id: Option<RecordId>,
    /// The Product's name, or the listing's title without one.
    pub name: String,
}

/// One line of a sale, with its share of what the Order left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoldLine {
    /// The channel's id of the Order: each row counts it once.
    pub order: String,
    pub product: SoldProduct,
    /// The name of the Sales Channel's category of the listing sold, when
    /// known.
    pub category: Option<String>,
    pub units: u32,
    pub revenue: Money,
    /// The Fees, Product Ads and tax that came out of it.
    pub deductions: Money,
    /// What the units cost; `None` when they never left the ledger, which
    /// keeps the line out of the margin.
    pub cost: Option<Money>,
}

/// The sales of one Product or one category in a period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarginRow<K> {
    pub of: K,
    pub orders: usize,
    pub units: u32,
    pub revenue: Money,
    /// The margin of the lines that have a cost; `None` without one.
    pub margin: Option<Margin>,
    /// Lines left out of the margin for want of a cost.
    pub without_cost: usize,
}

impl<K> MarginRow<K> {
    /// Its sales lost money.
    pub fn loses(&self) -> bool {
        self.margin
            .as_ref()
            .is_some_and(|margin| margin.amount.is_negative())
    }
}

/// How the rows are ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MarginOrder {
    /// The smallest margin first, so what loses money tops the list.
    #[default]
    Margin,
    /// The smallest margin as a share of the revenue first.
    Percent,
    /// The most revenue first.
    Revenue,
    /// By name, those without one last.
    Name,
}

/// A period's sales by Product and by category of the Sales Channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarginReport {
    pub by_product: Vec<MarginRow<SoldProduct>>,
    /// `None` for listings whose category is not known.
    pub by_category: Vec<MarginRow<Option<String>>>,
}

impl MarginReport {
    /// Sums `lines` per Product and per category, in `order`. A row's
    /// margin is its revenue less the deductions and cost of the lines that
    /// have a cost, the way the period's own margin is worked out; a row
    /// without such a line has none.
    pub fn of(
        lines: &[SoldLine],
        currency: Currency,
        order: MarginOrder,
    ) -> Result<Self, CurrencyMismatch> {
        let mut by_product = rows(lines, currency, |line| line.product.clone())?;
        let mut by_category = rows(lines, currency, |line| line.category.clone())?;
        sort(&mut by_product, order, |of| Some(of.name.as_str()));
        sort(&mut by_category, order, |of| of.as_deref());
        Ok(Self {
            by_product,
            by_category,
        })
    }
}

fn rows<K: Ord + Clone>(
    lines: &[SoldLine],
    currency: Currency,
    key: impl Fn(&SoldLine) -> K,
) -> Result<Vec<MarginRow<K>>, CurrencyMismatch> {
    let mut grouped: BTreeMap<K, Vec<&SoldLine>> = BTreeMap::new();
    for line in lines {
        grouped.entry(key(line)).or_default().push(line);
    }
    grouped
        .into_iter()
        .map(|(of, lines)| row(of, &lines, currency))
        .collect()
}

fn row<K>(
    of: K,
    lines: &[&SoldLine],
    currency: Currency,
) -> Result<MarginRow<K>, CurrencyMismatch> {
    let mut orders: Vec<&str> = lines.iter().map(|line| line.order.as_str()).collect();
    orders.sort_unstable();
    orders.dedup();
    let costed: Vec<(&SoldLine, Money)> = lines
        .iter()
        .filter_map(|line| line.cost.map(|cost| (*line, cost)))
        .collect();
    let margin = if costed.is_empty() {
        None
    } else {
        let sum = |pick: fn(&SoldLine, Money) -> Money| {
            Money::sum(
                currency,
                costed.iter().map(|(line, cost)| pick(line, *cost)),
            )
        };
        Some(Margin::of(
            sum(|line, _| line.revenue)?,
            &[sum(|line, _| line.deductions)?, sum(|_, cost| cost)?],
        )?)
    };
    Ok(MarginRow {
        of,
        orders: orders.len(),
        units: lines.iter().map(|line| line.units).sum(),
        revenue: Money::sum(currency, lines.iter().map(|line| line.revenue))?,
        margin,
        without_cost: lines.len() - costed.len(),
    })
}

/// Orders `rows` by `order`; ties, and rows without the figure ordered by,
/// go by name. Rows without a margin go after those with one.
fn sort<K>(rows: &mut [MarginRow<K>], order: MarginOrder, name: impl Fn(&K) -> Option<&str>) {
    rows.sort_by(|a, b| {
        let first = match order {
            MarginOrder::Margin => least_first(
                a.margin.map(|m| m.amount.amount()),
                b.margin.map(|m| m.amount.amount()),
            ),
            MarginOrder::Percent => least_first(
                a.margin.and_then(|m| m.percent),
                b.margin.and_then(|m| m.percent),
            ),
            MarginOrder::Revenue => b.revenue.amount().cmp(&a.revenue.amount()),
            MarginOrder::Name => Ordering::Equal,
        };
        first.then_with(|| least_first(name(&a.of), name(&b.of)))
    });
}

/// Smallest first, `None` last.
fn least_first<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}
