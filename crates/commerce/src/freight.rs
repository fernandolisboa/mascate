//! How a Purchase Order's freight reaches the cost of its units: split
//! across the lines by value (ADR 0005), then across each line's receipts.

use mascate_kernel::Money;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};

/// Splits `freight` across lines in proportion to their `values`. Shares
/// are whole cents (or the freight's own finer unit), and the cents left by
/// rounding down go to the lines that lost the most, so the shares always
/// add up to exactly `freight`. Lines worth nothing take no freight, unless
/// every line is worth nothing: then they share it equally.
pub fn split_by_value(freight: Money, values: &[Decimal]) -> Vec<Money> {
    if values.is_empty() {
        return Vec::new();
    }
    let total: Decimal = values.iter().sum();
    let weights: Vec<Decimal> = if total.is_zero() {
        vec![Decimal::ONE; values.len()]
    } else {
        values.to_vec()
    };
    let total = if total.is_zero() {
        Decimal::from(values.len())
    } else {
        total
    };
    let scale = smallest_unit(freight);
    let unit = Decimal::new(1, scale);
    let units = freight.amount() / unit;
    let exact: Vec<Decimal> = weights
        .iter()
        .map(|weight| weight / total * units)
        .collect();
    let mut shares: Vec<Decimal> = exact.iter().map(|share| share.floor()).collect();
    let left = units - shares.iter().sum::<Decimal>();
    let mut by_remainder: Vec<usize> = (0..shares.len()).collect();
    by_remainder.sort_by(|&a, &b| (exact[b] - shares[b]).cmp(&(exact[a] - shares[a])));
    for &line in by_remainder.iter().take(left.to_usize().unwrap_or(0)) {
        shares[line] += Decimal::ONE;
    }
    shares
        .into_iter()
        .map(|share| {
            let mut amount = share * unit;
            amount.rescale(scale);
            Money::new(amount, freight.currency())
        })
        .collect()
}

/// The part of a line's `share` of freight that `receiving` units carry,
/// when `received` of the line's `ordered` units came in before. Rounded on
/// the running total, so the parts of a line received in any number of
/// steps add up to exactly its share.
pub fn freight_for_units(share: Money, ordered: u32, received: u32, receiving: u32) -> Money {
    if ordered == 0 {
        return Money::zero(share.currency());
    }
    let scale = smallest_unit(share);
    let carried = |units: u32| {
        (share.amount() * Decimal::from(units) / Decimal::from(ordered))
            .round_dp_with_strategy(scale, RoundingStrategy::MidpointAwayFromZero)
    };
    Money::new(
        carried(received + receiving) - carried(received),
        share.currency(),
    )
}

/// The decimal places freight is split in: cents, or finer when the amount
/// was typed finer.
fn smallest_unit(amount: Money) -> u32 {
    amount
        .amount()
        .scale()
        .max(amount.currency().minor_unit_digits())
}
