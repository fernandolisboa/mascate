use std::fmt;

use rust_decimal::{Decimal, RoundingStrategy};

/// ISO 4217 currencies the app handles. Add one only when a Platform pays in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Currency {
    Brl,
    Usd,
}

impl Currency {
    pub fn code(self) -> &'static str {
        match self {
            Currency::Brl => "BRL",
            Currency::Usd => "USD",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "BRL" => Some(Currency::Brl),
            "USD" => Some(Currency::Usd),
            _ => None,
        }
    }

    pub fn minor_unit_digits(self) -> u32 {
        2
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cannot combine {left} with {right}")]
pub struct CurrencyMismatch {
    pub left: &'static str,
    pub right: &'static str,
}

/// An exact decimal amount in an explicit currency. Never a float (ADR 0004),
/// and never silently mixed with another currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Money {
    amount: Decimal,
    currency: Currency,
}

impl Money {
    pub fn new(amount: Decimal, currency: Currency) -> Self {
        Self { amount, currency }
    }

    pub fn zero(currency: Currency) -> Self {
        Self::new(Decimal::ZERO, currency)
    }

    pub fn amount(&self) -> Decimal {
        self.amount
    }

    pub fn currency(&self) -> Currency {
        self.currency
    }

    pub fn is_negative(&self) -> bool {
        self.amount.is_sign_negative() && !self.amount.is_zero()
    }

    pub fn checked_add(self, other: Money) -> Result<Money, CurrencyMismatch> {
        self.same_currency(other)?;
        Ok(Money::new(self.amount + other.amount, self.currency))
    }

    pub fn checked_sub(self, other: Money) -> Result<Money, CurrencyMismatch> {
        self.same_currency(other)?;
        Ok(Money::new(self.amount - other.amount, self.currency))
    }

    /// Multiplies by a factor such as a fee rate or a quantity, keeping full
    /// precision. Round only when the value leaves the app or is shown.
    pub fn times(self, factor: Decimal) -> Money {
        Money::new(self.amount * factor, self.currency)
    }

    /// Rounds to the currency's minor unit, half away from zero (cents).
    pub fn rounded(self) -> Money {
        let amount = self.amount.round_dp_with_strategy(
            self.currency.minor_unit_digits(),
            RoundingStrategy::MidpointAwayFromZero,
        );
        Money::new(amount, self.currency)
    }

    /// Sums amounts that must all share `currency`; an empty list is zero.
    pub fn sum<I>(currency: Currency, items: I) -> Result<Money, CurrencyMismatch>
    where
        I: IntoIterator<Item = Money>,
    {
        items
            .into_iter()
            .try_fold(Money::zero(currency), Money::checked_add)
    }

    fn same_currency(&self, other: Money) -> Result<(), CurrencyMismatch> {
        if self.currency == other.currency {
            Ok(())
        } else {
            Err(CurrencyMismatch {
                left: self.currency.code(),
                right: other.currency.code(),
            })
        }
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut amount = self.rounded().amount;
        // -0.004 rounds to zero; show it without a minus sign.
        if amount.is_zero() {
            amount.set_sign_positive(true);
        }
        write!(
            f,
            "{} {:.*}",
            self.currency.code(),
            self.currency.minor_unit_digits() as usize,
            amount
        )
    }
}
