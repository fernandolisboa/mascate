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

    /// How the owner reads the currency in Brazilian Portuguese.
    pub fn symbol(self) -> &'static str {
        match self {
            Currency::Brl => "R$",
            Currency::Usd => "US$",
        }
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

    /// The amount as the owner reads it: `R$ 1.234,56`, rounded to cents.
    pub fn to_pt_br(self) -> String {
        let rounded = self.rounded().amount;
        let digits = self.currency.minor_unit_digits() as usize;
        let plain = format!("{:.*}", digits, rounded.abs());
        let (whole, fraction) = plain.split_once('.').unwrap_or((&plain, ""));
        let mut grouped = String::new();
        for (index, digit) in whole.chars().enumerate() {
            if index > 0 && (whole.len() - index) % 3 == 0 {
                grouped.push('.');
            }
            grouped.push(digit);
        }
        let sign = if rounded.is_sign_negative() && !rounded.is_zero() {
            "-"
        } else {
            ""
        };
        format!("{sign}{} {grouped},{fraction}", self.currency.symbol())
    }

    pub(crate) fn same_currency(&self, other: Money) -> Result<(), CurrencyMismatch> {
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

/// Reads an amount the owner typed, as Brazilians write it (`1.234,56`) or
/// with a dot for decimals (`29.90`), with or without the currency symbol.
/// When only dots appear, a dot followed by exactly three digits groups
/// thousands, as in `1.234`. `None` when the text is not an amount.
pub fn parse_amount(text: &str) -> Option<Decimal> {
    let mut text = text.trim();
    let negative = text.starts_with('-');
    if negative {
        text = text[1..].trim_start();
    }
    for symbol in ["US$", "R$", "$"] {
        if let Some(rest) = text.strip_prefix(symbol) {
            text = rest.trim_start_matches(|c: char| c.is_whitespace());
            break;
        }
    }
    if text.is_empty()
        || !text
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
    {
        return None;
    }
    let decimal_separator = match (text.rfind(','), text.rfind('.')) {
        (Some(comma), Some(dot)) => Some(if comma > dot { ',' } else { '.' }),
        (Some(_), None) if text.matches(',').count() == 1 => Some(','),
        (Some(_), None) => return None,
        (None, Some(dot)) => {
            let single = text.matches('.').count() == 1;
            let groups_thousands = text.len() - dot - 1 == 3 && !text.starts_with("0.");
            (single && !groups_thousands).then_some('.')
        }
        (None, None) => None,
    };
    let (whole, fraction) = match decimal_separator {
        Some(separator) => text.rsplit_once(separator)?,
        None => (text, ""),
    };
    let whole = ungrouped(whole)?;
    if fraction.contains(['.', ',']) {
        return None;
    }
    let plain = if fraction.is_empty() {
        whole
    } else {
        format!("{whole}.{fraction}")
    };
    let amount: Decimal = plain.parse().ok()?;
    Some(if negative { -amount } else { amount })
}

/// The integer part without its thousands separators, which must group by
/// three: `1.234.567`, never `12.34`.
fn ungrouped(whole: &str) -> Option<String> {
    let separator = whole.chars().find(|c| !c.is_ascii_digit());
    let Some(separator) = separator else {
        return (!whole.is_empty()).then(|| whole.to_owned());
    };
    let mut groups = whole.split(separator);
    let first = groups.next()?;
    if first.is_empty() || first.len() > 3 || !first.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut digits = first.to_owned();
    for group in groups {
        if group.len() != 3 || !group.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        digits.push_str(group);
    }
    Some(digits)
}
