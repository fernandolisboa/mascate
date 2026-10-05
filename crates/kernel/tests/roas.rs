use std::str::FromStr;

use mascate_kernel::{BreakEven, Currency, Margin, Money, Roas, channel_day, channel_day_start};
use proptest::prelude::*;
use rust_decimal::Decimal;

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

fn decimal(text: &str) -> Decimal {
    Decimal::from_str(text).unwrap()
}

fn roas(attributed: &str, cost: &str) -> Roas {
    Roas::of(brl(attributed), brl(cost)).unwrap().unwrap()
}

#[test]
fn roas_is_the_sales_the_ads_brought_over_what_they_cost() {
    let found = roas("850", "200");

    assert_eq!(found.ratio(), decimal("4.25"));
    assert_eq!(found.to_pt_br(), "4,25x");
    assert_eq!(roas("100", "300").to_pt_br(), "0,33x");
    assert_eq!(roas("0", "15").ratio(), Decimal::ZERO);
}

#[test]
fn ads_that_cost_nothing_have_no_roas() {
    assert_eq!(Roas::of(brl("120"), brl("0")).unwrap(), None);
}

#[test]
fn a_25_percent_margin_before_ads_breaks_even_at_4x() {
    // R$ 100 sale: R$ 75 of fees, tax and cost leave R$ 25 for ads.
    let margin = Margin::of(brl("100"), &[brl("75")]).unwrap();

    let break_even = BreakEven::of(margin);

    assert_eq!(break_even, BreakEven::At(roas("4", "1")));
    assert_eq!(break_even.to_pt_br(), "4,00x");
    assert!(break_even.loses_at(roas("3.99", "1")));
    assert!(!break_even.loses_at(roas("4", "1")));
    assert!(!break_even.loses_at(roas("6.5", "1")));
}

#[test]
fn a_sale_without_margin_before_ads_never_pays_for_an_ad() {
    for margin in [
        Margin::of(brl("100"), &[brl("100")]).unwrap(),
        Margin::of(brl("100"), &[brl("130")]).unwrap(),
        Margin::of(brl("0"), &[]).unwrap(),
    ] {
        let break_even = BreakEven::of(margin);

        assert_eq!(break_even, BreakEven::Never);
        assert_eq!(break_even.to_pt_br(), "–");
        assert!(break_even.loses_at(roas("35", "1")));
    }
}

#[test]
fn the_channel_day_runs_from_three_in_the_morning_utc() {
    let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
    let start = channel_day_start(day);

    assert_eq!(start.to_rfc3339(), "2026-10-05T03:00:00+00:00");
    assert_eq!(channel_day(start), day);
    assert_eq!(
        channel_day(start - chrono::TimeDelta::seconds(1)),
        day.pred_opt().unwrap()
    );
}

proptest! {
    #[test]
    fn a_positive_margin_breaks_even_at_one_or_more(
        price_cents in 1i64..10_000_000,
        kept_share in 1u32..=10_000,
    ) {
        let price = Money::new(Decimal::new(price_cents, 2), Currency::Brl);
        // Deduct all but `kept_share` hundredths of a percent of the price.
        let deducted = price.times(Decimal::ONE - Decimal::new(i64::from(kept_share), 4));
        let margin = Margin::of(price, &[deducted]).unwrap();

        match BreakEven::of(margin) {
            BreakEven::At(least) => {
                prop_assert!(least.ratio() >= Decimal::ONE);
                // Ads returning exactly the break-even spend the whole margin.
                let spend = margin.amount;
                let brought = spend.times(least.ratio());
                prop_assert!((brought.amount() - price.amount()).abs() < Decimal::new(1, 6));
            }
            BreakEven::Never => prop_assert!(margin.amount.amount() <= Decimal::ZERO),
        }
    }

    #[test]
    fn roas_never_goes_below_zero(attributed in 0i64..10_000_000, cost in 1i64..10_000_000) {
        let found = Roas::of(
            Money::new(Decimal::new(attributed, 2), Currency::Brl),
            Money::new(Decimal::new(cost, 2), Currency::Brl),
        )
        .unwrap()
        .unwrap();
        prop_assert!(found.ratio() >= Decimal::ZERO);
    }
}
