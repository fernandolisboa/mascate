//! Commissions summed up per currency: examples and properties. Nothing is
//! ever converted, reversals stay out of what was earned and cancelled
//! sales hold no money.

use chrono::{TimeZone, Utc};
use mascate_affiliate::{
    AffiliatePlatform, Commission, CommissionStatus, CurrencyTotals, totals_by_currency,
};
use mascate_kernel::{Currency, Money, RecordId};
use proptest::prelude::*;
use rust_decimal::Decimal;

fn commission(amount: Money, status: CommissionStatus) -> Commission {
    let at = Utc.with_ymd_and_hms(2026, 10, 13, 12, 0, 0).unwrap();
    Commission {
        id: RecordId::from_u128(1),
        platform: AffiliatePlatform::Hotmart,
        program: RecordId::from_u128(2),
        program_name: "Curso".into(),
        product: "Curso".into(),
        sold_at: at,
        amount,
        status,
        platform_status: String::new(),
        status_since: at,
        tracking: None,
        manual: false,
        payout: None,
        linked_product: None,
    }
}

fn brl(cents: i64) -> Money {
    Money::new(Decimal::new(cents, 2), Currency::Brl)
}

fn usd(cents: i64) -> Money {
    Money::new(Decimal::new(cents, 2), Currency::Usd)
}

#[test]
fn each_currency_sums_apart_with_reversals_out_of_what_was_earned() {
    let totals = totals_by_currency(&[
        commission(brl(9730), CommissionStatus::Approved),
        commission(usd(1999), CommissionStatus::Paid),
        commission(brl(1240), CommissionStatus::Paid),
        commission(brl(5000), CommissionStatus::Reversed),
        commission(brl(700), CommissionStatus::Pending),
        commission(brl(3000), CommissionStatus::Cancelled),
        commission(usd(500), CommissionStatus::Pending),
    ]);

    assert_eq!(
        totals,
        [
            CurrencyTotals {
                currency: Currency::Brl,
                pending: brl(700),
                approved: brl(9730),
                paid: brl(1240),
                reversed: brl(5000),
                cancelled: 1,
            },
            CurrencyTotals {
                currency: Currency::Usd,
                pending: usd(500),
                approved: usd(0),
                paid: usd(1999),
                reversed: usd(0),
                cancelled: 0,
            },
        ]
    );
    assert_eq!(totals[0].earned(), brl(10970));
    assert_eq!(totals[1].earned(), usd(1999));
    assert_eq!(totals[0].of(CommissionStatus::Reversed), Some(brl(5000)));
    assert_eq!(totals[0].of(CommissionStatus::Cancelled), None);
}

#[test]
fn no_commissions_have_no_totals() {
    assert!(totals_by_currency(&[]).is_empty());
}

fn any_commission() -> impl Strategy<Value = Commission> {
    (
        prop::sample::select(vec![Currency::Brl, Currency::Usd]),
        prop::sample::select(CommissionStatus::ALL.to_vec()),
        0i64..10_000_000,
        0u32..4,
    )
        .prop_map(|(currency, status, units, scale)| {
            commission(Money::new(Decimal::new(units, scale), currency), status)
        })
}

proptest! {
    #[test]
    fn every_amount_lands_once_in_its_own_currency_and_status(
        commissions in prop::collection::vec(any_commission(), 0..40)
    ) {
        let totals = totals_by_currency(&commissions);
        for total in &totals {
            for status in CommissionStatus::ALL {
                let expected: Decimal = commissions
                    .iter()
                    .filter(|c| c.amount.currency() == total.currency && c.status == status)
                    .map(|c| c.amount.amount())
                    .sum();
                match total.of(status) {
                    Some(sum) => {
                        prop_assert_eq!(sum.currency(), total.currency);
                        prop_assert_eq!(sum.amount(), expected);
                    }
                    None => prop_assert_eq!(
                        total.cancelled,
                        commissions
                            .iter()
                            .filter(|c| c.amount.currency() == total.currency
                                && c.status == status)
                            .count()
                    ),
                }
            }
            prop_assert_eq!(
                total.earned().amount(),
                total.approved.amount() + total.paid.amount()
            );
        }
        let mut currencies: Vec<&str> = commissions
            .iter()
            .map(|c| c.amount.currency().code())
            .collect();
        currencies.sort();
        currencies.dedup();
        let listed: Vec<&str> = totals.iter().map(|t| t.currency.code()).collect();
        prop_assert_eq!(listed, currencies);
    }

    #[test]
    fn the_order_of_the_commissions_does_not_change_the_totals(
        mut commissions in prop::collection::vec(any_commission(), 0..40)
    ) {
        let before = totals_by_currency(&commissions);
        commissions.reverse();
        prop_assert_eq!(totals_by_currency(&commissions), before);
    }

    #[test]
    fn totals_of_two_lists_add_up_to_the_totals_of_both(
        first in prop::collection::vec(any_commission(), 0..20),
        second in prop::collection::vec(any_commission(), 0..20),
    ) {
        let both: Vec<Commission> = first.iter().chain(&second).cloned().collect();
        let apart = [totals_by_currency(&first), totals_by_currency(&second)];
        for total in totals_by_currency(&both) {
            let earned: Decimal = apart
                .iter()
                .flatten()
                .filter(|part| part.currency == total.currency)
                .map(|part| part.earned().amount())
                .sum();
            prop_assert_eq!(total.earned().amount(), earned);
        }
    }
}
