//! The period's earned affiliate Commissions on the business dashboard:
//! summed per currency, approved apart from paid, never converted.

use std::str::FromStr;

use mascate_finance::{CommissionIncome, EarnedCommission};
use mascate_kernel::{Currency, Money};
use proptest::prelude::*;
use rust_decimal::Decimal;

fn money(text: &str, currency: Currency) -> Money {
    Money::new(Decimal::from_str(text).unwrap(), currency)
}

fn earned(text: &str, currency: Currency, paid: bool) -> EarnedCommission {
    EarnedCommission {
        amount: money(text, currency),
        paid,
    }
}

#[test]
fn approved_and_paid_sum_apart_in_each_currency() {
    let income = CommissionIncome::of(&[
        earned("97.30", Currency::Brl, false),
        earned("19.99", Currency::Usd, true),
        earned("12.40", Currency::Brl, true),
        earned("5.00", Currency::Brl, false),
    ]);

    assert_eq!(
        income,
        [
            CommissionIncome {
                currency: Currency::Brl,
                approved: money("102.30", Currency::Brl),
                paid: money("12.40", Currency::Brl),
            },
            CommissionIncome {
                currency: Currency::Usd,
                approved: money("0", Currency::Usd),
                paid: money("19.99", Currency::Usd),
            },
        ]
    );
    assert_eq!(income[0].earned(), money("114.70", Currency::Brl));
}

#[test]
fn no_commissions_earn_nothing() {
    assert!(CommissionIncome::of(&[]).is_empty());
}

fn any_earned() -> impl Strategy<Value = EarnedCommission> {
    (
        prop::sample::select(vec![Currency::Brl, Currency::Usd]),
        0i64..10_000_000,
        0u32..4,
        any::<bool>(),
    )
        .prop_map(|(currency, units, scale, paid)| EarnedCommission {
            amount: Money::new(Decimal::new(units, scale), currency),
            paid,
        })
}

proptest! {
    #[test]
    fn each_currency_earns_exactly_what_its_commissions_add_up_to(
        commissions in prop::collection::vec(any_earned(), 0..40)
    ) {
        let income = CommissionIncome::of(&commissions);
        for currency in [Currency::Brl, Currency::Usd] {
            let of = |paid: bool| -> Decimal {
                commissions
                    .iter()
                    .filter(|c| c.amount.currency() == currency && c.paid == paid)
                    .map(|c| c.amount.amount())
                    .sum()
            };
            let found = income.iter().find(|i| i.currency == currency);
            match found {
                Some(found) => {
                    prop_assert_eq!(found.approved, Money::new(of(false), currency));
                    prop_assert_eq!(found.paid, Money::new(of(true), currency));
                    prop_assert_eq!(found.earned().amount(), of(false) + of(true));
                }
                None => prop_assert!(
                    commissions.iter().all(|c| c.amount.currency() != currency)
                ),
            }
        }
    }
}
