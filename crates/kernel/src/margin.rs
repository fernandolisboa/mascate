//! Margins: a sale price minus every Fee and the cost of the unit sold. The
//! same rule gives the Estimated Margin of an Opportunity or Listing and the
//! Realized Margin of an Order.

use rust_decimal::{Decimal, RoundingStrategy};

use crate::{CurrencyMismatch, Money, parse_amount};

/// A rate such as a tax or a fee, in percent: `12.5` is 12,5%.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Percentage(Decimal);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a percentage goes from 0 to 100")]
pub struct OutOfRange;

impl Percentage {
    pub const ZERO: Percentage = Percentage(Decimal::ZERO);

    /// A rate from 0 to 100 percent.
    pub fn new(percent: Decimal) -> Result<Self, OutOfRange> {
        if percent.is_sign_negative() && !percent.is_zero() || percent > Decimal::ONE_HUNDRED {
            Err(OutOfRange)
        } else {
            Ok(Self(percent.normalize()))
        }
    }

    /// Reads what the owner typed, as `12,5`, `12.5` or `12,5%`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_suffix('%').unwrap_or(text);
        Self::new(parse_amount(text)?).ok()
    }

    pub fn percent(self) -> Decimal {
        self.0
    }

    /// This share of `amount`, at full precision.
    pub fn of(self, amount: Money) -> Money {
        amount.times(self.0 / Decimal::ONE_HUNDRED)
    }

    /// As the owner reads it: `12,5%`.
    pub fn to_pt_br(self) -> String {
        format!("{}%", self.0.normalize().to_string().replace('.', ","))
    }
}

/// What is left of a sale price after its Fees and the unit's cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Margin {
    pub amount: Money,
    /// The amount as a share of the price, in percent; `None` for a price of
    /// zero, which has no share to speak of.
    pub percent: Option<Decimal>,
}

impl Margin {
    /// `price` minus each of `deductions` (Fees, the unit's cost), all in
    /// the price's currency.
    pub fn of(price: Money, deductions: &[Money]) -> Result<Self, CurrencyMismatch> {
        let deducted = Money::sum(price.currency(), deductions.iter().copied())?;
        let amount = price.checked_sub(deducted)?;
        let percent = (!price.amount().is_zero())
            .then(|| amount.amount() * Decimal::ONE_HUNDRED / price.amount());
        Ok(Self { amount, percent })
    }

    /// The percentage with one decimal place, as the owner reads it:
    /// `23,4%`; a dash without one.
    pub fn percent_to_pt_br(&self) -> String {
        match self.percent {
            Some(percent) => format!(
                "{}%",
                format!(
                    "{:.1}",
                    percent.round_dp_with_strategy(1, RoundingStrategy::MidpointAwayFromZero)
                )
                .replace('.', ",")
            ),
            None => "–".into(),
        }
    }
}
