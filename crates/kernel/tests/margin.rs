use std::str::FromStr;

use mascate_kernel::{Currency, Margin, Money, OutOfRange, Percentage};
use proptest::prelude::*;
use rust_decimal::Decimal;

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

fn decimal(text: &str) -> Decimal {
    Decimal::from_str(text).unwrap()
}

#[test]
fn margin_is_the_price_minus_fees_and_cost_with_its_share_of_the_price() {
    // R$ 100 sale: R$ 14 fee, R$ 20 shipping, 6% tax, R$ 35 delivered cost.
    let price = brl("100");
    let tax = Percentage::new(decimal("6")).unwrap().of(price);
    let margin = Margin::of(price, &[brl("14"), brl("20"), tax, brl("35")]).unwrap();

    assert_eq!(margin.amount, brl("25"));
    assert_eq!(margin.percent, Some(decimal("25")));
    assert_eq!(margin.percent_to_pt_br(), "25,0%");
}

#[test]
fn a_loss_is_a_negative_margin() {
    let margin = Margin::of(brl("50"), &[brl("8.50"), brl("45")]).unwrap();

    assert_eq!(margin.amount, brl("-3.50"));
    assert_eq!(margin.percent, Some(decimal("-7")));
    assert_eq!(margin.percent_to_pt_br(), "-7,0%");
}

#[test]
fn margins_keep_full_precision_and_round_only_for_display() {
    let margin = Margin::of(brl("29.90"), &[brl("10")]).unwrap();

    assert_eq!(margin.amount, brl("19.90"));
    assert_eq!(margin.percent_to_pt_br(), "66,6%");
    let third = Margin::of(brl("3"), &[brl("1")]).unwrap();
    assert_eq!(third.percent_to_pt_br(), "66,7%");
}

#[test]
fn a_free_price_has_no_percentage() {
    let margin = Margin::of(brl("0"), &[brl("5")]).unwrap();

    assert_eq!(margin.amount, brl("-5"));
    assert_eq!(margin.percent, None);
    assert_eq!(margin.percent_to_pt_br(), "–");
}

#[test]
fn margins_never_mix_currencies() {
    let usd = Money::new(decimal("1"), Currency::Usd);
    assert!(Margin::of(brl("10"), &[usd]).is_err());
}

#[test]
fn percentages_go_from_zero_to_a_hundred_and_read_as_typed() {
    assert_eq!(Percentage::new(decimal("-0.1")), Err(OutOfRange));
    assert_eq!(Percentage::new(decimal("100.01")), Err(OutOfRange));
    assert_eq!(Percentage::new(decimal("0")), Ok(Percentage::ZERO));
    assert_eq!(
        Percentage::parse(" 12,5% ").unwrap().percent(),
        decimal("12.5")
    );
    assert_eq!(Percentage::parse("6.5").unwrap().percent(), decimal("6.5"));
    assert_eq!(Percentage::parse("abc"), None);
    assert_eq!(Percentage::parse("120"), None);
    assert_eq!(Percentage::parse("12,50").unwrap().to_pt_br(), "12,5%");
    assert_eq!(
        Percentage::new(decimal("10")).unwrap().of(brl("29.90")),
        brl("2.99")
    );
}

fn cents() -> impl Strategy<Value = Money> {
    (0i64..10_000_000).prop_map(|cents| Money::new(Decimal::new(cents, 2), Currency::Brl))
}

proptest! {
    #[test]
    fn margin_plus_deductions_is_always_the_price(
        price in cents(),
        deductions in prop::collection::vec(cents(), 0..6),
    ) {
        let margin = Margin::of(price, &deductions).unwrap();
        let back = Money::sum(Currency::Brl, deductions.iter().copied().chain([margin.amount]))
            .unwrap();
        prop_assert_eq!(back, price);
    }

    #[test]
    fn a_bigger_fee_never_raises_the_margin(
        price in cents(),
        fee in cents(),
        extra in cents(),
    ) {
        let lower = Margin::of(price, &[fee.checked_add(extra).unwrap()]).unwrap();
        let higher = Margin::of(price, &[fee]).unwrap();
        prop_assert!(lower.amount.amount() <= higher.amount.amount());
    }

    #[test]
    fn the_percentage_is_the_margin_over_the_price(
        price in (1i64..10_000_000).prop_map(|c| Money::new(Decimal::new(c, 2), Currency::Brl)),
        cost in cents(),
    ) {
        let margin = Margin::of(price, &[cost]).unwrap();
        let percent = margin.percent.unwrap();
        let back = price.amount() * percent / Decimal::ONE_HUNDRED;
        prop_assert!((back - margin.amount.amount()).abs() < Decimal::new(1, 6));
    }

    #[test]
    fn a_share_of_an_amount_never_exceeds_it(amount in cents(), percent in 0u32..=10_000) {
        let rate = Percentage::new(Decimal::new(i64::from(percent), 2)).unwrap();
        let share = rate.of(amount);
        prop_assert!(share.amount() <= amount.amount());
        prop_assert!(!share.is_negative());
    }
}
