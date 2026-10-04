use mascate_kernel::{Currency, CurrencyMismatch, Money};
use rust_decimal::Decimal;

/// A Product's stock: the units on hand and what they cost in all, valued
/// at the moving weighted average (ADR 0005). Keeping the total rather than
/// the unit cost means no rounding builds up from one entry to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Valuation {
    quantity: i64,
    value: Money,
}

impl Valuation {
    /// No stock yet, in the currency its entries will be valued in.
    pub fn empty(currency: Currency) -> Self {
        Self {
            quantity: 0,
            value: Money::zero(currency),
        }
    }

    pub fn quantity(&self) -> i64 {
        self.quantity
    }

    /// What the units on hand cost in all.
    pub fn value(&self) -> Money {
        self.value
    }

    /// The Average Cost: what one unit on hand cost. `None` with nothing on
    /// hand.
    pub fn average_cost(&self) -> Option<Money> {
        (self.quantity > 0).then(|| {
            Money::new(
                self.value.amount() / Decimal::from(self.quantity),
                self.value.currency(),
            )
        })
    }

    /// Takes in `quantity` units that cost `cost` in all. The Average Cost
    /// moves toward their unit cost in proportion to how many came in.
    pub fn enter(self, quantity: u32, cost: Money) -> Result<Self, CurrencyMismatch> {
        self.moved(i64::from(quantity), cost)
    }

    /// What `units` units carry out of stock at the Average Cost, as the
    /// negative cost of the movement that takes them; `None` when fewer are
    /// on hand. Taking them leaves the Average Cost where it was.
    pub fn exit_cost(&self, units: u32) -> Option<Money> {
        let units = i64::from(units);
        if units > self.quantity {
            return None;
        }
        // The last units take all that is left, so no rounding stays behind.
        let share = if units == self.quantity {
            self.value.amount()
        } else {
            self.value.amount() * Decimal::from(units) / Decimal::from(self.quantity)
        };
        Some(Money::new(-share, self.value.currency()))
    }

    /// Applies one Stock Movement: units and cost both signed, negative for
    /// what leaves.
    pub(crate) fn moved(self, quantity: i64, cost: Money) -> Result<Self, CurrencyMismatch> {
        Ok(Self {
            quantity: self.quantity + quantity,
            value: self.value.checked_add(cost)?,
        })
    }
}
