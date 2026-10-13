use std::str::FromStr;

use mascate_finance::{Ahead, LinkedCommission, ResellOrPromote, SoldLine, SoldProduct};
use mascate_kernel::{Currency, Money, RecordId};
use proptest::prelude::*;
use rust_decimal::Decimal;

fn brl(text: &str) -> Money {
    Money::new(Decimal::from_str(text).unwrap(), Currency::Brl)
}

fn usd(text: &str) -> Money {
    Money::new(Decimal::from_str(text).unwrap(), Currency::Usd)
}

fn fone() -> RecordId {
    RecordId::from_u128(1)
}

fn capa() -> RecordId {
    RecordId::from_u128(2)
}

fn sold(order: &str, product: RecordId, units: u32, revenue: &str) -> SoldLine {
    SoldLine {
        order: order.into(),
        product: SoldProduct {
            id: Some(product),
            name: "Produto".into(),
        },
        category: None,
        units,
        revenue: brl(revenue),
        deductions: brl("0"),
        cost: Some(brl("0")),
    }
}

fn earned(product: RecordId, amount: Money) -> LinkedCommission {
    LinkedCommission { product, amount }
}

/// The Fone sold twice, three units, leaving R$ 75 of margin; promoting it
/// earned R$ 30 in three Commissions.
fn fone_period() -> (Vec<SoldLine>, Vec<LinkedCommission>) {
    let lines = vec![
        SoldLine {
            deductions: brl("25.00"),
            cost: Some(brl("40.00")),
            ..sold("2001", fone(), 1, "100.00")
        },
        SoldLine {
            deductions: brl("40.00"),
            cost: Some(brl("80.00")),
            ..sold("2002", fone(), 2, "160.00")
        },
    ];
    let commissions = vec![
        earned(fone(), brl("12.00")),
        earned(fone(), brl("12.00")),
        earned(fone(), brl("6.00")),
    ];
    (lines, commissions)
}

#[test]
fn a_product_shows_its_margin_next_to_what_promoting_it_earned() {
    let (lines, commissions) = fone_period();

    let compared = ResellOrPromote::of(&[fone()], &lines, &commissions).unwrap();

    assert_eq!(compared.len(), 1);
    let fone_brl = compared[0];
    assert_eq!(
        (fone_brl.product, fone_brl.currency),
        (fone(), Currency::Brl)
    );
    let reselling = fone_brl.reselling.unwrap();
    assert_eq!((reselling.orders, reselling.units), (2, 3));
    assert_eq!(reselling.revenue, brl("260.00"));
    assert_eq!(reselling.margin.unwrap().amount, brl("75.00"));
    assert_eq!(reselling.per_unit, Some(brl("25.00")));
    let promoting = fone_brl.promoting.unwrap();
    assert_eq!(promoting.commissions, 3);
    assert_eq!(promoting.earned, brl("30.00"));
    assert_eq!(promoting.per_commission, brl("10.00"));
    assert_eq!(fone_brl.ahead(), Some(Ahead::Reselling));
    assert_eq!(fone_brl.lead(), Some(brl("45.00")));
}

#[test]
fn commissions_above_the_margin_put_promoting_ahead() {
    let (lines, mut commissions) = fone_period();
    commissions.push(earned(fone(), brl("60.00")));

    let compared = ResellOrPromote::of(&[fone()], &lines, &commissions).unwrap();

    assert_eq!(compared[0].ahead(), Some(Ahead::Promoting));
    assert_eq!(compared[0].lead(), Some(brl("15.00")));
}

#[test]
fn the_same_money_on_both_sides_is_even() {
    let (lines, _) = fone_period();
    let commissions = vec![earned(fone(), brl("75"))];

    let compared = ResellOrPromote::of(&[fone()], &lines, &commissions).unwrap();

    assert_eq!(compared[0].ahead(), Some(Ahead::Even));
    assert_eq!(compared[0].lead(), Some(brl("0")));
}

#[test]
fn a_side_with_nothing_in_the_period_counts_as_zero() {
    let (lines, commissions) = fone_period();

    let only_sold = ResellOrPromote::of(&[fone()], &lines, &[]).unwrap();
    let only_promoted = ResellOrPromote::of(&[fone()], &[], &commissions).unwrap();

    assert_eq!(only_sold[0].promoting, None);
    assert_eq!(only_sold[0].ahead(), Some(Ahead::Reselling));
    assert_eq!(only_promoted[0].reselling, None);
    assert_eq!(only_promoted[0].ahead(), Some(Ahead::Promoting));
}

#[test]
fn a_sale_that_lost_money_loses_to_no_commission_at_all() {
    let lines = vec![SoldLine {
        deductions: brl("12.00"),
        cost: Some(brl("25.00")),
        ..sold("2003", capa(), 1, "30.00")
    }];

    let compared = ResellOrPromote::of(&[capa()], &lines, &[]).unwrap();

    assert_eq!(
        compared[0].reselling.unwrap().margin.unwrap().amount,
        brl("-7.00")
    );
    assert_eq!(compared[0].ahead(), Some(Ahead::Promoting));
}

#[test]
fn without_a_cost_there_is_no_margin_to_compare() {
    let lines = vec![SoldLine {
        cost: None,
        ..sold("2004", capa(), 2, "60.00")
    }];
    let commissions = vec![earned(capa(), brl("5.00"))];

    let compared = ResellOrPromote::of(&[capa()], &lines, &commissions).unwrap();

    let reselling = compared[0].reselling.unwrap();
    assert_eq!(reselling.margin, None);
    assert_eq!(reselling.per_unit, None);
    assert_eq!(reselling.without_cost, 1);
    assert_eq!(compared[0].ahead(), None);
    assert_eq!(compared[0].lead(), None);
}

#[test]
fn the_per_unit_margin_counts_only_the_units_with_a_cost() {
    let lines = vec![
        SoldLine {
            deductions: brl("10.00"),
            cost: Some(brl("20.00")),
            ..sold("2005", capa(), 2, "60.00")
        },
        SoldLine {
            cost: None,
            ..sold("2006", capa(), 5, "150.00")
        },
    ];

    let compared = ResellOrPromote::of(&[capa()], &lines, &[]).unwrap();

    let reselling = compared[0].reselling.unwrap();
    assert_eq!(reselling.units, 7);
    assert_eq!(reselling.per_unit, Some(brl("15.00")));
}

#[test]
fn each_currency_is_compared_on_its_own_and_never_converted() {
    let (lines, mut commissions) = fone_period();
    commissions.push(earned(fone(), usd("9.00")));

    let compared = ResellOrPromote::of(&[fone()], &lines, &commissions).unwrap();

    let currencies: Vec<Currency> = compared.iter().map(|c| c.currency).collect();
    assert_eq!(currencies, [Currency::Brl, Currency::Usd]);
    assert_eq!(compared[0].promoting.unwrap().earned, brl("30.00"));
    assert_eq!(compared[1].reselling, None);
    assert_eq!(compared[1].promoting.unwrap().earned, usd("9.00"));
    assert_eq!(compared[1].ahead(), Some(Ahead::Promoting));
}

#[test]
fn only_the_products_asked_for_come_back_in_their_order() {
    let (mut lines, mut commissions) = fone_period();
    lines.push(sold("2007", capa(), 1, "30.00"));
    lines.push(SoldLine {
        product: SoldProduct {
            id: None,
            name: "Anúncio sem produto".into(),
        },
        ..sold("2008", capa(), 1, "30.00")
    });
    commissions.push(earned(capa(), brl("3.00")));
    let cabo = RecordId::from_u128(3);

    let fone_only = ResellOrPromote::of(&[fone()], &lines, &commissions).unwrap();
    let both = ResellOrPromote::of(&[capa(), cabo, fone(), capa()], &lines, &commissions).unwrap();

    assert_eq!(fone_only.len(), 1);
    assert_eq!(fone_only[0].reselling.unwrap().orders, 2);
    let products: Vec<RecordId> = both.iter().map(|c| c.product).collect();
    assert_eq!(products, [capa(), fone()]);
    assert_eq!(both[0].reselling.unwrap().revenue, brl("30.00"));
}

fn amount(currency: Currency) -> impl Strategy<Value = Money> {
    (-50_000i64..200_000).prop_map(move |cents| Money::new(Decimal::new(cents, 2), currency))
}

fn currency() -> impl Strategy<Value = Currency> {
    prop_oneof![Just(Currency::Brl), Just(Currency::Usd)]
}

fn lines() -> impl Strategy<Value = Vec<SoldLine>> {
    prop::collection::vec(
        (0u8..6, 0u128..4, 1u32..5, currency(), any::<bool>()),
        0..30,
    )
    .prop_flat_map(|drawn| {
        drawn
            .into_iter()
            .map(|(order, product, units, currency, costed)| {
                (amount(currency), amount(currency), amount(currency)).prop_map(
                    move |(revenue, deductions, cost)| SoldLine {
                        order: order.to_string(),
                        product: SoldProduct {
                            id: (product > 0).then(|| RecordId::from_u128(product)),
                            name: format!("Produto {product}"),
                        },
                        category: None,
                        units,
                        revenue,
                        deductions,
                        cost: costed.then_some(cost),
                    },
                )
            })
            .collect::<Vec<_>>()
    })
}

fn commissions() -> impl Strategy<Value = Vec<LinkedCommission>> {
    prop::collection::vec(
        (1u128..4, currency()).prop_flat_map(|(product, currency)| {
            (0i64..50_000).prop_map(move |cents| LinkedCommission {
                product: RecordId::from_u128(product),
                amount: Money::new(Decimal::new(cents, 2), currency),
            })
        }),
        0..30,
    )
}

fn asked() -> impl Strategy<Value = Vec<RecordId>> {
    prop::collection::vec((0u128..5).prop_map(RecordId::from_u128), 0..5)
}

proptest! {
    #[test]
    fn both_sides_add_up_to_what_the_asked_products_sold_and_earned(
        products in asked(),
        lines in lines(),
        commissions in commissions(),
    ) {
        let compared = ResellOrPromote::of(&products, &lines, &commissions).unwrap();

        for currency in [Currency::Brl, Currency::Usd] {
            let asked_line = |line: &&SoldLine| {
                line.revenue.currency() == currency
                    && line.product.id.is_some_and(|id| products.contains(&id))
            };
            let revenue: Decimal = lines.iter().filter(asked_line).map(|l| l.revenue.amount()).sum();
            let margin: Decimal = lines
                .iter()
                .filter(asked_line)
                .filter_map(|l| Some(l.revenue.amount() - l.deductions.amount() - l.cost?.amount()))
                .sum();
            let asked_commissions: Vec<&LinkedCommission> = commissions
                .iter()
                .filter(|c| c.amount.currency() == currency && products.contains(&c.product))
                .collect();
            let earned: Decimal = asked_commissions.iter().map(|c| c.amount.amount()).sum();

            let of_currency: Vec<&ResellOrPromote> =
                compared.iter().filter(|c| c.currency == currency).collect();
            let compared_revenue: Decimal = of_currency
                .iter()
                .filter_map(|c| Some(c.reselling?.revenue.amount()))
                .sum();
            let compared_margin: Decimal = of_currency
                .iter()
                .filter_map(|c| Some(c.reselling?.margin?.amount.amount()))
                .sum();
            let compared_earned: Decimal = of_currency
                .iter()
                .filter_map(|c| Some(c.promoting?.earned.amount()))
                .sum();
            let compared_count: usize = of_currency
                .iter()
                .filter_map(|c| Some(c.promoting?.commissions))
                .sum();
            prop_assert_eq!(compared_revenue, revenue);
            prop_assert_eq!(compared_margin, margin);
            prop_assert_eq!(compared_earned, earned);
            prop_assert_eq!(compared_count, asked_commissions.len());
        }
        for comparison in &compared {
            prop_assert!(products.contains(&comparison.product));
            prop_assert!(comparison.reselling.is_some() || comparison.promoting.is_some());
        }
    }

    #[test]
    fn the_side_ahead_is_the_one_that_left_more_money(
        products in asked(),
        lines in lines(),
        commissions in commissions(),
    ) {
        let compared = ResellOrPromote::of(&products, &lines, &commissions).unwrap();

        for comparison in compared {
            let margin = match comparison.reselling {
                Some(reselling) => reselling.margin.map(|m| m.amount.amount()),
                None => Some(Decimal::ZERO),
            };
            let earned = comparison.promoting.map_or(Decimal::ZERO, |p| p.earned.amount());
            let expected = margin.map(|margin| {
                if margin > earned {
                    Ahead::Reselling
                } else if margin < earned {
                    Ahead::Promoting
                } else {
                    Ahead::Even
                }
            });
            prop_assert_eq!(comparison.ahead(), expected);
            prop_assert_eq!(
                comparison.lead().map(|lead| lead.amount()),
                margin.map(|margin| (margin - earned).abs())
            );
        }
    }

    #[test]
    fn the_order_of_the_lines_and_commissions_changes_nothing(
        products in asked(),
        lines in lines(),
        commissions in commissions(),
    ) {
        let forward = ResellOrPromote::of(&products, &lines, &commissions).unwrap();
        let mut lines = lines;
        let mut commissions = commissions;
        lines.reverse();
        commissions.reverse();

        prop_assert_eq!(ResellOrPromote::of(&products, &lines, &commissions).unwrap(), forward);
    }

    #[test]
    fn the_averages_times_their_counts_give_the_totals_back(
        products in asked(),
        lines in lines(),
        commissions in commissions(),
    ) {
        let compared = ResellOrPromote::of(&products, &lines, &commissions).unwrap();

        for comparison in compared {
            if let Some(promoting) = comparison.promoting {
                let back = promoting.per_commission.amount() * Decimal::from(promoting.commissions);
                prop_assert!((back - promoting.earned.amount()).abs() < Decimal::new(1, 6));
            }
        }
    }
}
