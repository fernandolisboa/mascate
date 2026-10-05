use std::str::FromStr;

use chrono::{NaiveDate, TimeZone, Utc};
use mascate_finance::{MoneyRelease, ReleaseSummary};
use mascate_kernel::{Currency, Money, Timestamp};
use rust_decimal::Decimal;

fn brl(text: &str) -> Money {
    Money::new(Decimal::from_str(text).unwrap(), Currency::Brl)
}

fn at(day: u32, hour: u32) -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
}

fn day(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
}

fn release(amount: &str, released: bool, on: Option<Timestamp>) -> MoneyRelease {
    MoneyRelease {
        amount: brl(amount),
        released,
        on,
    }
}

#[test]
fn pending_money_comes_by_day_and_released_money_counts_within_the_period() {
    let releases = [
        release("80.00", true, Some(at(2, 15))),
        release("40.00", true, Some(at(20, 15))),
        release("55.50", false, Some(at(12, 15))),
        release("20.00", false, Some(at(12, 18))),
        release("30.00", false, Some(at(9, 15))),
        release("90.00", false, None),
    ];

    let summary = ReleaseSummary::of(&releases, at(1, 0)..at(15, 0), Currency::Brl).unwrap();

    assert_eq!(summary.pending, brl("195.50"));
    assert_eq!(
        summary.schedule,
        [(day(9), brl("30.00")), (day(12), brl("75.50"))]
    );
    assert_eq!(summary.undated, brl("90.00"));
    assert_eq!(summary.released, brl("80.00"));
}

#[test]
fn a_day_of_the_schedule_is_a_day_in_brasilia() {
    // 02:00 UTC on the 13th is still the 12th in Brasília.
    let releases = [release("10.00", false, Some(at(13, 2)))];

    let summary = ReleaseSummary::of(&releases, at(1, 0)..at(31, 0), Currency::Brl).unwrap();

    assert_eq!(summary.schedule, [(day(12), brl("10.00"))]);
}

#[test]
fn nothing_to_release_is_zero_everywhere() {
    let summary = ReleaseSummary::of(&[], at(1, 0)..at(31, 0), Currency::Brl).unwrap();

    assert_eq!(summary.pending, brl("0"));
    assert!(summary.schedule.is_empty());
    assert_eq!(summary.undated, brl("0"));
    assert_eq!(summary.released, brl("0"));
}
