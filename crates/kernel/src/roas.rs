//! Return on ad spend: what Product Ads brought in for each real spent on
//! them, and the least of it that still pays for them, from the margin a
//! sale keeps before Ads: 1 / that margin, as a share of the price.

use rust_decimal::{Decimal, RoundingStrategy};

use crate::{CurrencyMismatch, Margin, Money};

/// Sales brought in for each unit of money spent on ads: `4` is 4x.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Roas(Decimal);

impl Roas {
    /// `attributed` sales over the `cost` of the ads that brought them;
    /// `None` when nothing was spent.
    pub fn of(attributed: Money, cost: Money) -> Result<Option<Self>, CurrencyMismatch> {
        attributed.same_currency(cost)?;
        Ok(attributed
            .amount()
            .checked_div(cost.amount())
            .filter(|_| cost.amount() > Decimal::ZERO)
            .map(|ratio| Self(ratio.max(Decimal::ZERO))))
    }

    pub fn ratio(self) -> Decimal {
        self.0
    }

    /// As the owner reads it: `4,25x`.
    pub fn to_pt_br(self) -> String {
        format!(
            "{:.2}x",
            self.0
                .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
        )
        .replace('.', ",")
    }
}

/// The least ROAS at which Product Ads pay for themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BreakEven {
    At(Roas),
    /// The sale loses money before any ad: no return pays for one.
    Never,
}

impl BreakEven {
    /// From the margin of a sale before Ads: a margin of 25% of the price
    /// pays ads that bring in 4 times their cost. Never at a margin of zero
    /// or less, or without a price to take a share of.
    pub fn of(before_ads: Margin) -> Self {
        match before_ads.percent {
            Some(percent) if percent > Decimal::ZERO => {
                Self::At(Roas(Decimal::ONE_HUNDRED / percent))
            }
            _ => Self::Never,
        }
    }

    /// Whether ads returning `roas` lose money: below the break-even, or
    /// any at all when nothing pays.
    pub fn loses_at(self, roas: Roas) -> bool {
        match self {
            Self::At(least) => roas < least,
            Self::Never => true,
        }
    }

    /// As the owner reads it: `4,00x`, or a dash when nothing pays.
    pub fn to_pt_br(self) -> String {
        match self {
            Self::At(roas) => roas.to_pt_br(),
            Self::Never => "–".into(),
        }
    }
}
