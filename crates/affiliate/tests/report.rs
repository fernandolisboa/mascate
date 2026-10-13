//! The commissions dashboard through the Affiliate module's public
//! interface: Commissions recorded by a Sync and by hand in a real
//! temporary database, then grouped by Platform, Program, product and
//! tracking tag per currency. Examples, and properties: every grouping adds
//! up to the total, cancelled sales never count and reversals stay out of
//! what was earned.

use std::ops::Range;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliatePlatform, Commission, CommissionFilter, CommissionGroup, CommissionReport,
    CommissionSource, CommissionStatus, CurrencyTotals, GroupOrder, MIGRATIONS, ManualCommission,
    ProgramRef, ReportedConversion, ReportedProgram, totals_by_currency,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, PlatformError, RecordId, Timestamp};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

fn noon() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 13, 12, 0, 0).unwrap()
}

fn brl(amount: &str) -> Money {
    Money::new(amount.parse::<Decimal>().unwrap(), Currency::Brl)
}

fn usd(amount: &str) -> Money {
    Money::new(amount.parse::<Decimal>().unwrap(), Currency::Usd)
}

/// A Platform answering from memory.
struct Source {
    platform: AffiliatePlatform,
    conversions: Vec<ReportedConversion>,
}

impl CommissionSource for Source {
    fn platform(&self) -> AffiliatePlatform {
        self.platform
    }

    fn history(&self) -> TimeDelta {
        TimeDelta::days(90)
    }

    fn settle_time(&self) -> TimeDelta {
        TimeDelta::days(30)
    }

    fn conversions(&self, _: Range<Timestamp>) -> Result<Vec<ReportedConversion>, PlatformError> {
        Ok(self.conversions.clone())
    }
}

fn conversion(
    key: &str,
    program: (&str, &str),
    commission: Money,
    status: CommissionStatus,
    tracking: Option<&str>,
) -> ReportedConversion {
    ReportedConversion {
        key: key.into(),
        program: ReportedProgram {
            id: program.0.into(),
            name: program.1.into(),
        },
        product: program.1.into(),
        sold_at: noon() - TimeDelta::days(3),
        commission,
        status,
        platform_status: String::new(),
        status_changed_at: None,
        tracking: tracking.map(Into::into),
        raw: "{}".into(),
    }
}

async fn affiliate(dir: &tempfile::TempDir) -> Affiliate {
    let clock = Arc::new(ManualClock::at(noon()));
    let database = Arc::new(
        Database::open(&dir.path().join("mascate.db"))
            .await
            .unwrap(),
    );
    migrate(&database, clock.as_ref(), &[MIGRATIONS])
        .await
        .unwrap();
    Affiliate::new(database, clock, Arc::new(SequentialIds::default()))
}

/// Two Hotmart courses, Shopee items under two tags and none, a Mercado
/// Livre Commission by hand and a Hotmart sale in dollars.
async fn recorded(affiliate: &Affiliate) -> Vec<Commission> {
    let photo = ("4012345", "Curso de Fotografia");
    let excel = ("4099999", "Excel Avançado");
    let hotmart = Source {
        platform: AffiliatePlatform::Hotmart,
        conversions: vec![
            conversion(
                "HP1",
                photo,
                brl("97.30"),
                CommissionStatus::Approved,
                Some("instagram"),
            ),
            conversion(
                "HP2",
                photo,
                brl("97.30"),
                CommissionStatus::Reversed,
                Some("instagram"),
            ),
            conversion("HP3", excel, brl("45.00"), CommissionStatus::Pending, None),
            conversion(
                "HP4",
                excel,
                brl("45.00"),
                CommissionStatus::Cancelled,
                Some("telegram"),
            ),
            conversion(
                "HP5",
                photo,
                usd("19.99"),
                CommissionStatus::Paid,
                Some("youtube"),
            ),
        ],
    };
    let shopee_program = ("shopee-affiliates", "Shopee Afiliados");
    let item = |key, amount, status, tag| {
        let mut sold = conversion(key, shopee_program, brl(amount), status, tag);
        sold.product = "Fone Bluetooth".into();
        sold
    };
    let shopee = Source {
        platform: AffiliatePlatform::Shopee,
        conversions: vec![
            item("S1", "12.40", CommissionStatus::Approved, Some("telegram")),
            item("S2", "8.10", CommissionStatus::Pending, Some("  ")),
            item("S3", "3.50", CommissionStatus::Reversed, Some("telegram")),
        ],
    };
    affiliate.sync(&hotmart).await.unwrap();
    affiliate.sync(&shopee).await.unwrap();
    affiliate
        .record_manual(ManualCommission {
            platform: AffiliatePlatform::MercadoLivre,
            program: String::new(),
            product: "Fone Bluetooth".into(),
            sold_at: noon() - TimeDelta::days(1),
            amount: brl("5.00"),
            status: CommissionStatus::Paid,
        })
        .await
        .unwrap();
    affiliate
        .commissions(&CommissionFilter::default())
        .await
        .unwrap()
}

fn totals(
    currency: Currency,
    pending: &str,
    approved: &str,
    paid: &str,
    reversed: &str,
) -> CurrencyTotals {
    let money = |amount: &str| Money::new(amount.parse::<Decimal>().unwrap(), currency);
    CurrencyTotals {
        currency,
        pending: money(pending),
        approved: money(approved),
        paid: money(paid),
        reversed: money(reversed),
        cancelled: 0,
    }
}

fn names<K>(groups: &[CommissionGroup<K>], name: impl Fn(&K) -> String) -> Vec<(String, Money)> {
    groups
        .iter()
        .map(|group| (name(&group.of), group.totals.earned()))
        .collect()
}

#[test]
fn a_periods_commissions_group_by_platform_program_product_and_tag_per_currency() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let affiliate = affiliate(&dir).await;
        let commissions = recorded(&affiliate).await;

        let reports = CommissionReport::of(&commissions, GroupOrder::Earned);

        assert_eq!(
            reports
                .iter()
                .map(|report| report.currency)
                .collect::<Vec<_>>(),
            [Currency::Brl, Currency::Usd]
        );
        let real = &reports[0];
        assert_eq!(
            real.totals,
            totals(Currency::Brl, "53.10", "109.70", "5.00", "100.80")
        );
        assert_eq!(real.totals.earned(), brl("114.70"));
        assert_eq!(real.commissions, 7);
        assert_eq!(
            names(&real.by_platform, |of| of.name().to_owned()),
            [
                ("Hotmart".to_owned(), brl("97.30")),
                ("Shopee".to_owned(), brl("12.40")),
                ("Mercado Livre".to_owned(), brl("5.00")),
            ]
        );
        assert_eq!(
            names(&real.by_program, |of| format!(
                "{} {}",
                of.platform.name(),
                of.name
            )),
            [
                ("Hotmart Curso de Fotografia".to_owned(), brl("97.30")),
                ("Shopee Shopee Afiliados".to_owned(), brl("12.40")),
                ("Mercado Livre Mercado Livre".to_owned(), brl("5.00")),
                ("Hotmart Excel Avançado".to_owned(), brl("0")),
            ]
        );
        assert_eq!(
            names(&real.by_product, Clone::clone),
            [
                ("Curso de Fotografia".to_owned(), brl("97.30")),
                ("Fone Bluetooth".to_owned(), brl("17.40")),
                ("Excel Avançado".to_owned(), brl("0")),
            ]
        );
        assert_eq!(
            real.by_tracking
                .iter()
                .map(|group| (group.of.clone(), group.totals, group.commissions))
                .collect::<Vec<_>>(),
            [
                (
                    Some("instagram".to_owned()),
                    totals(Currency::Brl, "0", "97.30", "0", "97.30"),
                    2
                ),
                (
                    Some("telegram".to_owned()),
                    totals(Currency::Brl, "0", "12.40", "0", "3.50"),
                    2
                ),
                (None, totals(Currency::Brl, "53.10", "0", "5.00", "0"), 3),
            ],
            "a blank tag is no tag, and the cancelled Excel sale tagged telegram is left out"
        );

        let dollars = &reports[1];
        assert_eq!(
            dollars.totals,
            totals(Currency::Usd, "0", "0", "19.99", "0")
        );
        assert_eq!(
            dollars.by_tracking[0].of.as_deref(),
            Some("youtube"),
            "never converted: the dollar sale stays in its own report"
        );
    });
}

#[test]
fn groups_sort_by_the_amount_asked_with_ties_by_name() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let affiliate = affiliate(&dir).await;
        let commissions = recorded(&affiliate).await;

        let report = |order| CommissionReport::of(&commissions, order).remove(0);
        let platforms = |order| {
            report(order)
                .by_platform
                .iter()
                .map(|group| group.of)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            platforms(GroupOrder::Pending),
            [
                AffiliatePlatform::Hotmart,
                AffiliatePlatform::Shopee,
                AffiliatePlatform::MercadoLivre
            ]
        );
        assert_eq!(
            platforms(GroupOrder::Reversed),
            [
                AffiliatePlatform::Hotmart,
                AffiliatePlatform::Shopee,
                AffiliatePlatform::MercadoLivre
            ]
        );
        assert_eq!(
            platforms(GroupOrder::Name),
            [
                AffiliatePlatform::Hotmart,
                AffiliatePlatform::MercadoLivre,
                AffiliatePlatform::Shopee
            ]
        );
        assert_eq!(
            report(GroupOrder::Name)
                .by_tracking
                .iter()
                .map(|group| group.of.clone())
                .collect::<Vec<_>>(),
            [
                Some("instagram".to_owned()),
                Some("telegram".to_owned()),
                None
            ],
            "the untagged go last"
        );
    });
}

#[test]
fn only_cancelled_sales_make_no_report() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let affiliate = affiliate(&dir).await;
        affiliate
            .record_manual(ManualCommission {
                platform: AffiliatePlatform::Magalu,
                program: String::new(),
                product: "Air fryer".into(),
                sold_at: noon(),
                amount: brl("20.00"),
                status: CommissionStatus::Cancelled,
            })
            .await
            .unwrap();
        let commissions = affiliate
            .commissions(&CommissionFilter::default())
            .await
            .unwrap();

        assert!(CommissionReport::of(&commissions, GroupOrder::Earned).is_empty());
        assert!(CommissionReport::of(&[], GroupOrder::Earned).is_empty());
    });
}

fn any_commission() -> impl Strategy<Value = Commission> {
    (
        prop::sample::select(vec![Currency::Brl, Currency::Usd]),
        prop::sample::select(CommissionStatus::ALL.to_vec()),
        0i64..10_000_000,
        0u32..4,
        prop::sample::select(AffiliatePlatform::ALL.to_vec()),
        0u128..4,
        prop::sample::select(vec!["Curso", "Fone", "Air fryer"]),
        prop::option::of(prop::sample::select(vec!["instagram", "telegram", "", " "])),
    )
        .prop_map(
            |(currency, status, units, scale, platform, program, product, tracking)| {
                let at = noon();
                Commission {
                    id: RecordId::from_u128(1),
                    platform,
                    program: RecordId::from_u128(program),
                    program_name: format!("Programa {program}"),
                    product: product.into(),
                    sold_at: at,
                    amount: Money::new(Decimal::new(units, scale), currency),
                    status,
                    platform_status: String::new(),
                    status_since: at,
                    tracking: tracking.map(Into::into),
                    manual: false,
                    payout: None,
                    linked_product: None,
                }
            },
        )
}

fn any_order() -> impl Strategy<Value = GroupOrder> {
    prop::sample::select(vec![
        GroupOrder::Earned,
        GroupOrder::Pending,
        GroupOrder::Reversed,
        GroupOrder::Name,
    ])
}

/// Each status summed over `groups`.
fn added_up<K>(groups: &[CommissionGroup<K>], currency: Currency) -> (CurrencyTotals, usize) {
    let mut sum = CurrencyTotals {
        currency,
        pending: Money::zero(currency),
        approved: Money::zero(currency),
        paid: Money::zero(currency),
        reversed: Money::zero(currency),
        cancelled: 0,
    };
    let mut count = 0;
    for group in groups {
        for status in CommissionStatus::ALL {
            let (Some(into), Some(add)) = (
                match status {
                    CommissionStatus::Pending => Some(&mut sum.pending),
                    CommissionStatus::Approved => Some(&mut sum.approved),
                    CommissionStatus::Paid => Some(&mut sum.paid),
                    CommissionStatus::Reversed => Some(&mut sum.reversed),
                    CommissionStatus::Cancelled => None,
                },
                group.totals.of(status),
            ) else {
                continue;
            };
            *into = into.checked_add(add).unwrap();
        }
        sum.cancelled += group.totals.cancelled;
        count += group.commissions;
    }
    (sum, count)
}

proptest! {
    #[test]
    fn every_grouping_adds_up_to_the_total(
        commissions in prop::collection::vec(any_commission(), 0..40),
        order in any_order(),
    ) {
        for report in CommissionReport::of(&commissions, order) {
            let expected = (report.totals, report.commissions);
            prop_assert_eq!(added_up(&report.by_platform, report.currency), expected);
            prop_assert_eq!(added_up(&report.by_program, report.currency), expected);
            prop_assert_eq!(added_up(&report.by_product, report.currency), expected);
            prop_assert_eq!(added_up(&report.by_tracking, report.currency), expected);
        }
    }

    #[test]
    fn cancelled_sales_never_count_and_reversals_stay_out_of_what_was_earned(
        commissions in prop::collection::vec(any_commission(), 0..40),
        order in any_order(),
    ) {
        let counted: Vec<Commission> = commissions
            .iter()
            .filter(|c| c.status != CommissionStatus::Cancelled)
            .cloned()
            .collect();
        let reports = CommissionReport::of(&commissions, order);
        prop_assert_eq!(
            reports.iter().map(|report| report.totals).collect::<Vec<_>>(),
            totals_by_currency(&counted)
        );
        for report in &reports {
            prop_assert_eq!(report.totals.cancelled, 0);
            prop_assert_eq!(
                report.totals.earned().amount(),
                report.totals.approved.amount() + report.totals.paid.amount()
            );
            prop_assert_eq!(
                report.commissions,
                counted
                    .iter()
                    .filter(|c| c.amount.currency() == report.currency)
                    .count()
            );
        }
    }

    #[test]
    fn groups_come_most_first_by_the_amount_asked(
        commissions in prop::collection::vec(any_commission(), 0..40),
    ) {
        for report in CommissionReport::of(&commissions, GroupOrder::Earned) {
            let most_first = report.by_product.windows(2).all(|pair| {
                pair[0].totals.earned().amount() >= pair[1].totals.earned().amount()
            });
            prop_assert!(most_first);
        }
        for report in CommissionReport::of(&commissions, GroupOrder::Reversed) {
            let most_first = report.by_tracking.windows(2).all(|pair| {
                pair[0].totals.reversed.amount() >= pair[1].totals.reversed.amount()
            });
            prop_assert!(most_first);
        }
        for report in CommissionReport::of(&commissions, GroupOrder::Name) {
            let programs: Vec<&ProgramRef> = report.by_program.iter().map(|g| &g.of).collect();
            prop_assert!(programs.windows(2).all(|pair| pair[0].name <= pair[1].name));
        }
    }

    #[test]
    fn a_blank_tag_is_no_tag(
        commissions in prop::collection::vec(any_commission(), 0..40),
    ) {
        for report in CommissionReport::of(&commissions, GroupOrder::Name) {
            for group in &report.by_tracking {
                prop_assert!(group.of.as_deref().is_none_or(|tag| !tag.trim().is_empty()));
            }
        }
    }
}
