use std::str::FromStr;

use mascate_finance::{MarginOrder, MarginReport, SoldLine, SoldProduct};
use mascate_kernel::{Currency, Money, RecordId};
use proptest::prelude::*;
use rust_decimal::Decimal;

fn brl(text: &str) -> Money {
    Money::new(Decimal::from_str(text).unwrap(), Currency::Brl)
}

fn product(n: u128, name: &str) -> SoldProduct {
    SoldProduct {
        id: Some(RecordId::from_u128(n)),
        name: name.into(),
    }
}

fn line(order: &str, sold: SoldProduct, category: Option<&str>, revenue: &str) -> SoldLine {
    SoldLine {
        order: order.into(),
        product: sold,
        category: category.map(Into::into),
        units: 1,
        revenue: brl(revenue),
        deductions: brl("0"),
        cost: Some(brl("0")),
    }
}

fn fone() -> SoldProduct {
    product(1, "Fone Bluetooth")
}

fn capa() -> SoldProduct {
    product(2, "Capa de celular")
}

fn cabo() -> SoldProduct {
    product(3, "Cabo USB-C")
}

/// Two Fone sales that pay, a Capa sale that loses money and a Cabo sale
/// whose units never left the ledger.
fn period() -> Vec<SoldLine> {
    vec![
        SoldLine {
            deductions: brl("25.00"),
            cost: Some(brl("40.00")),
            ..line("2001", fone(), Some("Fones de Ouvido"), "100.00")
        },
        SoldLine {
            units: 2,
            deductions: brl("45.00"),
            cost: Some(brl("80.00")),
            ..line("2002", fone(), Some("Fones de Ouvido"), "180.00")
        },
        SoldLine {
            deductions: brl("12.00"),
            cost: Some(brl("25.00")),
            ..line("2003", capa(), Some("Capas"), "30.00")
        },
        SoldLine {
            deductions: brl("6.00"),
            cost: None,
            ..line("2003", cabo(), Some("Fones de Ouvido"), "20.00")
        },
    ]
}

#[test]
fn each_product_sums_its_lines_and_the_one_that_loses_money_comes_first() {
    let report = MarginReport::of(&period(), Currency::Brl, MarginOrder::Margin).unwrap();

    let names: Vec<&str> = report
        .by_product
        .iter()
        .map(|row| row.of.name.as_str())
        .collect();
    assert_eq!(names, ["Capa de celular", "Fone Bluetooth", "Cabo USB-C"]);

    let capa = &report.by_product[0];
    assert!(capa.loses());
    assert_eq!(capa.margin.unwrap().amount, brl("-7.00"));

    let fone = &report.by_product[1];
    assert!(!fone.loses());
    assert_eq!(fone.orders, 2);
    assert_eq!(fone.units, 3);
    assert_eq!(fone.revenue, brl("280.00"));
    // 280 − 70 of Fees and tax − 120 of cost.
    assert_eq!(fone.margin.unwrap().amount, brl("90.00"));
    assert_eq!(
        fone.margin.unwrap().percent.unwrap().round_dp(2),
        Decimal::from_str("32.14").unwrap()
    );

    let cabo = &report.by_product[2];
    assert_eq!(cabo.margin, None);
    assert_eq!(cabo.without_cost, 1);
    assert_eq!(cabo.revenue, brl("20.00"));
}

#[test]
fn a_category_sums_every_product_sold_in_it_and_leaves_out_lines_without_a_cost() {
    let report = MarginReport::of(&period(), Currency::Brl, MarginOrder::Margin).unwrap();

    let names: Vec<Option<&str>> = report
        .by_category
        .iter()
        .map(|row| row.of.as_deref())
        .collect();
    assert_eq!(names, [Some("Capas"), Some("Fones de Ouvido")]);

    let fones = &report.by_category[1];
    assert_eq!(fones.orders, 3);
    assert_eq!(fones.revenue, brl("300.00"));
    assert_eq!(fones.margin.unwrap().amount, brl("90.00"));
    assert_eq!(fones.without_cost, 1);
}

#[test]
fn an_order_with_two_products_counts_once_in_their_shared_category() {
    let report = MarginReport::of(&period(), Currency::Brl, MarginOrder::Margin).unwrap();

    let fones = report
        .by_category
        .iter()
        .find(|row| row.of.as_deref() == Some("Fones de Ouvido"))
        .unwrap();
    let capas = report
        .by_category
        .iter()
        .find(|row| row.of.as_deref() == Some("Capas"))
        .unwrap();

    assert_eq!(fones.orders, 3);
    assert_eq!(capas.orders, 1);
}

#[test]
fn rows_order_by_revenue_by_percent_or_by_name() {
    let lines = period();

    let by_revenue = MarginReport::of(&lines, Currency::Brl, MarginOrder::Revenue).unwrap();
    let by_percent = MarginReport::of(&lines, Currency::Brl, MarginOrder::Percent).unwrap();
    let by_name = MarginReport::of(&lines, Currency::Brl, MarginOrder::Name).unwrap();

    let names = |report: &MarginReport| -> Vec<String> {
        report
            .by_product
            .iter()
            .map(|row| row.of.name.clone())
            .collect()
    };
    assert_eq!(
        names(&by_revenue),
        ["Fone Bluetooth", "Capa de celular", "Cabo USB-C"]
    );
    assert_eq!(
        names(&by_percent),
        ["Capa de celular", "Fone Bluetooth", "Cabo USB-C"]
    );
    assert_eq!(
        names(&by_name),
        ["Cabo USB-C", "Capa de celular", "Fone Bluetooth"]
    );
}

#[test]
fn listings_without_a_product_count_by_title_and_unknown_categories_go_last() {
    let unlinked = SoldProduct {
        id: None,
        name: "Anúncio sem produto".into(),
    };
    let lines = vec![
        line("2001", unlinked.clone(), None, "50.00"),
        line("2002", unlinked.clone(), None, "70.00"),
        line("2003", fone(), Some("Fones de Ouvido"), "10.00"),
    ];

    let report = MarginReport::of(&lines, Currency::Brl, MarginOrder::Name).unwrap();

    assert_eq!(report.by_product[0].of, unlinked);
    assert_eq!(report.by_product[0].orders, 2);
    assert_eq!(report.by_product[0].revenue, brl("120.00"));
    assert_eq!(
        report.by_category.last().map(|row| row.of.clone()),
        Some(None)
    );
}

#[test]
fn no_sales_give_no_rows() {
    let report = MarginReport::of(&[], Currency::Brl, MarginOrder::Margin).unwrap();

    assert!(report.by_product.is_empty());
    assert!(report.by_category.is_empty());
}

#[test]
fn a_line_in_another_currency_is_refused() {
    let lines = vec![SoldLine {
        revenue: Money::new(Decimal::from(10), Currency::Usd),
        ..line("2001", fone(), None, "0")
    }];

    assert!(MarginReport::of(&lines, Currency::Brl, MarginOrder::Margin).is_err());
}

fn cents() -> impl Strategy<Value = Money> {
    (-50_000i64..200_000).prop_map(|cents| Money::new(Decimal::new(cents, 2), Currency::Brl))
}

fn lines() -> impl Strategy<Value = Vec<SoldLine>> {
    prop::collection::vec(
        (
            0u8..6,
            0u128..4,
            prop::option::of(0u8..3),
            1u32..5,
            cents(),
            cents(),
            prop::option::of(cents()),
        ),
        0..30,
    )
    .prop_map(|drawn| {
        drawn
            .into_iter()
            .map(
                |(order, product, category, units, revenue, deductions, cost)| SoldLine {
                    order: order.to_string(),
                    product: SoldProduct {
                        id: Some(RecordId::from_u128(product)),
                        name: format!("Produto {product}"),
                    },
                    category: category.map(|c| format!("Categoria {c}")),
                    units,
                    revenue,
                    deductions,
                    cost,
                },
            )
            .collect()
    })
}

proptest! {
    #[test]
    fn products_and_categories_each_add_up_to_the_whole_period(lines in lines()) {
        let report = MarginReport::of(&lines, Currency::Brl, MarginOrder::Margin).unwrap();
        let revenue: Decimal = lines.iter().map(|line| line.revenue.amount()).sum();
        let margin: Decimal = lines
            .iter()
            .filter_map(|line| {
                Some(line.revenue.amount() - line.deductions.amount() - line.cost?.amount())
            })
            .sum();
        let units: u32 = lines.iter().map(|line| line.units).sum();

        for rows_revenue in [
            report.by_product.iter().map(|row| row.revenue.amount()).sum::<Decimal>(),
            report.by_category.iter().map(|row| row.revenue.amount()).sum::<Decimal>(),
        ] {
            prop_assert_eq!(rows_revenue, revenue);
        }
        let product_margin: Decimal = report
            .by_product
            .iter()
            .filter_map(|row| Some(row.margin?.amount.amount()))
            .sum();
        prop_assert_eq!(product_margin, margin);
        prop_assert_eq!(report.by_product.iter().map(|row| row.units).sum::<u32>(), units);
    }

    #[test]
    fn ordered_by_margin_no_row_comes_before_a_smaller_one(lines in lines()) {
        let report = MarginReport::of(&lines, Currency::Brl, MarginOrder::Margin).unwrap();
        let margins: Vec<Decimal> = report
            .by_product
            .iter()
            .filter_map(|row| Some(row.margin?.amount.amount()))
            .collect();

        prop_assert!(margins.windows(2).all(|pair| pair[0] <= pair[1]));
        let first_without = report.by_product.iter().position(|row| row.margin.is_none());
        if let Some(first) = first_without {
            prop_assert!(report.by_product[first..].iter().all(|row| row.margin.is_none()));
        }
        prop_assert!(report
            .by_product
            .iter()
            .all(|row| row.loses() == row.margin.is_some_and(|m| m.amount.is_negative())));
    }
}
