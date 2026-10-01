use std::str::FromStr;

use mascate_kernel::{Currency, CurrencyMismatch, Money};
use rust_decimal::Decimal;

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

fn usd(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Usd)
}

#[test]
fn adds_and_subtracts_exactly_where_floats_would_drift() {
    let total = brl("0.10").checked_add(brl("0.20")).unwrap();
    assert_eq!(total, brl("0.30"));
    assert_eq!(total.checked_sub(brl("0.30")).unwrap(), brl("0"));
}

#[test]
fn refuses_to_mix_currencies() {
    assert_eq!(
        brl("10").checked_add(usd("1")),
        Err(CurrencyMismatch {
            left: "BRL",
            right: "USD"
        })
    );
    assert!(brl("10").checked_sub(usd("1")).is_err());
    assert!(Money::sum(Currency::Brl, [brl("1"), usd("1")]).is_err());
}

#[test]
fn sum_of_nothing_is_zero_in_the_requested_currency() {
    assert_eq!(
        Money::sum(Currency::Usd, []).unwrap(),
        Money::zero(Currency::Usd)
    );
}

#[test]
fn sums_many_amounts() {
    let total = Money::sum(Currency::Brl, [brl("19.90"), brl("5.05"), brl("-2.00")]).unwrap();
    assert_eq!(total, brl("22.95"));
}

#[test]
fn multiplication_keeps_full_precision_until_rounded() {
    // A 16.5% fee on R$ 49.90 is R$ 8.2335.
    let fee = brl("49.90").times(Decimal::from_str("0.165").unwrap());
    assert_eq!(fee.amount(), Decimal::from_str("8.2335").unwrap());
    assert_eq!(fee.rounded(), brl("8.23"));
}

#[test]
fn rounds_half_away_from_zero_to_cents() {
    assert_eq!(brl("0.005").rounded(), brl("0.01"));
    assert_eq!(brl("0.015").rounded(), brl("0.02"));
    assert_eq!(brl("-0.005").rounded(), brl("-0.01"));
    assert_eq!(brl("1.004").rounded(), brl("1.00"));
}

#[test]
fn displays_code_and_two_decimals() {
    assert_eq!(brl("1234.5").to_string(), "BRL 1234.50");
    assert_eq!(usd("0.125").to_string(), "USD 0.13");
    assert_eq!(brl("-3").to_string(), "BRL -3.00");
    assert_eq!(brl("-0.004").to_string(), "BRL 0.00");
}

#[test]
fn negative_zero_is_not_negative() {
    assert!(brl("-1").is_negative());
    assert!(!brl("0").is_negative());
    assert!(!brl("-0.00").is_negative());
}

#[test]
fn currency_codes_round_trip() {
    for currency in [Currency::Brl, Currency::Usd] {
        assert_eq!(Currency::from_code(currency.code()), Some(currency));
    }
    assert_eq!(Currency::from_code("brl"), None);
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    fn any_brl() -> impl Strategy<Value = Money> {
        // Up to ten million reais with four decimal places, either sign.
        (-100_000_000_000i64..100_000_000_000i64)
            .prop_map(|units| Money::new(Decimal::new(units, 4), Currency::Brl))
    }

    proptest! {
        #[test]
        fn addition_is_commutative(a in any_brl(), b in any_brl()) {
            prop_assert_eq!(a.checked_add(b).unwrap(), b.checked_add(a).unwrap());
        }

        #[test]
        fn addition_is_associative(a in any_brl(), b in any_brl(), c in any_brl()) {
            let left = a.checked_add(b).unwrap().checked_add(c).unwrap();
            let right = a.checked_add(b.checked_add(c).unwrap()).unwrap();
            prop_assert_eq!(left, right);
        }

        #[test]
        fn subtracting_what_was_added_gives_back_the_original(a in any_brl(), b in any_brl()) {
            prop_assert_eq!(a.checked_add(b).unwrap().checked_sub(b).unwrap(), a);
        }

        #[test]
        fn rounding_is_idempotent_and_off_by_at_most_half_a_cent(a in any_brl()) {
            let once = a.rounded();
            prop_assert_eq!(once.rounded(), once);
            let error = (once.amount() - a.amount()).abs();
            prop_assert!(error <= Decimal::new(5, 3));
        }

        #[test]
        fn sum_matches_pairwise_addition(items in proptest::collection::vec(any_brl(), 0..20)) {
            let expected = items
                .iter()
                .fold(Money::zero(Currency::Brl), |acc, m| acc.checked_add(*m).unwrap());
            prop_assert_eq!(Money::sum(Currency::Brl, items).unwrap(), expected);
        }
    }
}
