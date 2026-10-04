use std::str::FromStr;

use mascate_commerce::{freight_for_units, split_by_value};
use mascate_kernel::{Currency, Money};
use rust_decimal::Decimal;

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

fn values(amounts: &[&str]) -> Vec<Decimal> {
    amounts
        .iter()
        .map(|amount| Decimal::from_str(amount).unwrap())
        .collect()
}

#[test]
fn freight_goes_to_each_line_in_proportion_to_its_value() {
    assert_eq!(
        split_by_value(brl("30.00"), &values(&["100", "200"])),
        [brl("10.00"), brl("20.00")]
    );
}

#[test]
fn the_cent_left_by_rounding_goes_to_the_line_that_lost_most() {
    assert_eq!(
        split_by_value(brl("10.00"), &values(&["1", "1", "1"])),
        [brl("3.34"), brl("3.33"), brl("3.33")]
    );
    assert_eq!(
        split_by_value(brl("1.00"), &values(&["10", "20", "70"])),
        [brl("0.10"), brl("0.20"), brl("0.70")]
    );
    assert_eq!(
        split_by_value(brl("0.05"), &values(&["1", "2"])),
        [brl("0.02"), brl("0.03")]
    );
}

#[test]
fn a_line_worth_nothing_takes_no_freight_unless_every_line_is_worth_nothing() {
    assert_eq!(
        split_by_value(brl("9.00"), &values(&["0", "50"])),
        [brl("0.00"), brl("9.00")]
    );
    assert_eq!(
        split_by_value(brl("9.00"), &values(&["0", "0", "0"])),
        [brl("3.00"), brl("3.00"), brl("3.00")]
    );
}

#[test]
fn freight_typed_finer_than_cents_splits_in_its_own_unit() {
    let shares = split_by_value(brl("0.005"), &values(&["1", "1"]));
    assert_eq!(shares, [brl("0.003"), brl("0.002")]);
}

#[test]
fn no_freight_or_no_lines_split_into_nothing() {
    assert_eq!(
        split_by_value(brl("0"), &values(&["5", "7"])),
        [brl("0.00"), brl("0.00")]
    );
    assert!(split_by_value(brl("12.00"), &[]).is_empty());
}

#[test]
fn units_received_in_steps_carry_the_line_freight_on_a_running_total() {
    let share = brl("10.00");
    let first = freight_for_units(share, 3, 0, 1);
    let second = freight_for_units(share, 3, 1, 1);
    let third = freight_for_units(share, 3, 2, 1);
    assert_eq!([first, second, third], [brl("3.33"), brl("3.34"), brl("3.33")]);
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    fn cents(amount: i64) -> Money {
        Money::new(Decimal::new(amount, 2), Currency::Brl)
    }

    proptest! {
        #[test]
        fn the_shares_add_up_to_exactly_the_freight(
            freight in 0i64..10_000_000,
            line_values in proptest::collection::vec(0i64..100_000_000, 1..15),
        ) {
            let freight = cents(freight);
            let values: Vec<Decimal> = line_values.iter().map(|&v| Decimal::new(v, 2)).collect();
            let shares = split_by_value(freight, &values);
            prop_assert_eq!(shares.len(), values.len());
            prop_assert_eq!(Money::sum(Currency::Brl, shares.clone()).unwrap(), freight);
            prop_assert!(shares.iter().all(|share| !share.is_negative()));
        }

        #[test]
        fn each_share_is_within_a_cent_of_its_exact_part(
            freight in 0i64..10_000_000,
            line_values in proptest::collection::vec(1i64..100_000_000, 1..15),
        ) {
            let freight = cents(freight);
            let values: Vec<Decimal> = line_values.iter().map(|&v| Decimal::new(v, 2)).collect();
            let total: Decimal = values.iter().sum();
            for (share, value) in split_by_value(freight, &values).iter().zip(&values) {
                let exact = freight.amount() * value / total;
                prop_assert!((share.amount() - exact).abs() < Decimal::new(1, 2));
            }
        }

        #[test]
        fn a_line_received_in_any_steps_carries_exactly_its_share(
            share in 0i64..10_000_000,
            steps in proptest::collection::vec(1u32..20, 1..10),
        ) {
            let share = cents(share);
            let ordered: u32 = steps.iter().sum();
            let mut received = 0;
            let mut parts = Vec::new();
            for &receiving in &steps {
                parts.push(freight_for_units(share, ordered, received, receiving));
                received += receiving;
            }
            prop_assert!(parts.iter().all(|part| !part.is_negative()));
            prop_assert_eq!(Money::sum(Currency::Brl, parts).unwrap(), share);
        }
    }
}
