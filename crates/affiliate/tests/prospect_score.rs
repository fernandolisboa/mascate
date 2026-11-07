//! The Prospect score and the aposta mark, as pure functions of the
//! signals and the day: each part with its reason, the missing parts'
//! weight shared, R$ per sale over %, and every aposta threshold at its
//! edge.

use chrono::{NaiveDate, TimeDelta};
use mascate_affiliate::{
    BET_MAX_AGE_DAYS, BET_MAX_TEMPERATURE, BET_MIN_REVIEWS, ScoreArea, Signals, Support, is_bet,
    score,
};
use mascate_kernel::{Currency, Money, Percentage};
use proptest::prelude::*;
use rust_decimal::Decimal;

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 7).unwrap()
}

fn brl(amount: &str) -> Money {
    Money::new(amount.parse::<Decimal>().unwrap(), Currency::Brl)
}

fn usd(amount: &str) -> Money {
    Money::new(amount.parse::<Decimal>().unwrap(), Currency::Usd)
}

fn percent(value: &str) -> Percentage {
    Percentage::new(value.parse().unwrap()).unwrap()
}

fn decimal(value: &str) -> Decimal {
    value.parse().unwrap()
}

fn part(signals: &Signals, area: ScoreArea) -> Option<(u8, String)> {
    score(signals, today())
        .parts
        .into_iter()
        .find(|part| part.area == area)
        .map(|part| (part.points, part.reason))
}

#[test]
fn commission_scores_what_one_sale_earns_not_the_rate() {
    let cheap_high_rate = Signals {
        price: Some(brl("47.00")),
        commission_rate: Some(percent("50")),
        ..Signals::default()
    };
    let dear_low_rate = Signals {
        price: Some(brl("497.00")),
        commission_rate: Some(percent("30")),
        ..Signals::default()
    };
    let cheap = part(&cheap_high_rate, ScoreArea::Commission).unwrap();
    let dear = part(&dear_low_rate, ScoreArea::Commission).unwrap();
    assert!(dear.0 > cheap.0, "{dear:?} vs {cheap:?}");
    assert_eq!(dear.1, "R$ 149,10 por venda (30% de R$ 497,00)");
    assert_eq!(dear.0, 99);
    assert_eq!(
        score(&dear_low_rate, today()).per_sale,
        Some(brl("149.1000"))
    );
}

#[test]
fn a_commission_in_money_wins_over_price_and_rate() {
    let signals = Signals {
        price: Some(brl("497.00")),
        commission_rate: Some(percent("30")),
        commission: Some(brl("75.00")),
        ..Signals::default()
    };
    assert_eq!(
        part(&signals, ScoreArea::Commission),
        Some((50, "R$ 75,00 por venda".to_owned()))
    );
}

#[test]
fn a_dollar_commission_is_measured_against_dollars_without_converting() {
    let signals = Signals {
        commission: Some(usd("15.00")),
        ..Signals::default()
    };
    let (points, reason) = part(&signals, ScoreArea::Commission).unwrap();
    assert_eq!(points, 50);
    assert!(reason.contains("em dólar"), "{reason}");
    assert_eq!(score(&signals, today()).per_sale, Some(usd("15.00")));
}

#[test]
fn a_rate_without_a_price_gives_no_commission_part() {
    let signals = Signals {
        commission_rate: Some(percent("50")),
        ..Signals::default()
    };
    let scored = score(&signals, today());
    assert!(scored.missing.contains(&ScoreArea::Commission));
    assert_eq!(scored.per_sale, None);
}

#[test]
fn sales_use_the_temperature_and_else_students_and_reviews() {
    let hot = Signals {
        temperature: Some(80),
        students: Some(1),
        ..Signals::default()
    };
    assert_eq!(
        part(&hot, ScoreArea::Sales),
        Some((80, "temperatura 80°".to_owned()))
    );
    let public = Signals {
        students: Some(5_000),
        reviews: Some(250),
        ..Signals::default()
    };
    assert_eq!(
        part(&public, ScoreArea::Sales),
        Some((50, "5.000 alunos, 250 avaliações".to_owned()))
    );
}

#[test]
fn material_uses_the_blueprint_and_else_the_course_itself() {
    let blueprint = Signals {
        blueprint: Some(percent("92")),
        classes: Some(1),
        ..Signals::default()
    };
    assert_eq!(
        part(&blueprint, ScoreArea::Material),
        Some((92, "Blueprint 92%".to_owned()))
    );
    let course = Signals {
        classes: Some(30),
        hours: Some(decimal("10")),
        certificate: Some(true),
        video: Some(false),
        ..Signals::default()
    };
    assert_eq!(
        part(&course, ScoreArea::Material),
        Some((50, "30 aulas, 10 h, com certificado, sem vídeo".to_owned()))
    );
}

#[test]
fn the_creator_part_weighs_time_badge_rating_satisfaction_and_the_owners_mark() {
    let signals = Signals {
        creator_since: Some(today() - TimeDelta::days(365 * 5)),
        creator_verified: Some(true),
        rating: Some(decimal("4")),
        satisfaction: Some(percent("90")),
        support: Some(Support::Bad),
        ..Signals::default()
    };
    assert_eq!(
        part(&signals, ScoreArea::Creator),
        Some((
            68,
            "há 5 anos na plataforma, perfil verificado, nota 4, satisfação 90%, suporte ruim, \
             pela sua marca"
                .to_owned()
        ))
    );
    let good = Signals {
        support: Some(Support::Good),
        ..signals.clone()
    };
    assert_eq!(part(&good, ScoreArea::Creator).unwrap().0, 88);
}

#[test]
fn a_full_score_weighs_the_four_parts() {
    let signals = Signals {
        commission: Some(brl("150.00")),
        temperature: Some(50),
        blueprint: Some(percent("100")),
        satisfaction: Some(percent("0")),
        ..Signals::default()
    };
    let scored = score(&signals, today());
    assert!(!scored.partial());
    // 30% × 100 + 30% × 50 + 20% × 100 + 20% × 0.
    assert_eq!(scored.total, 65);
    assert_eq!(scored.parts.len(), 4);
}

#[test]
fn a_part_without_signals_is_left_out_and_named() {
    let signals = Signals {
        commission: Some(brl("150.00")),
        temperature: Some(50),
        ..Signals::default()
    };
    let scored = score(&signals, today());
    assert!(scored.partial());
    assert_eq!(scored.missing, [ScoreArea::Material, ScoreArea::Creator]);
    // The commission and the sales share all the weight: (100 + 50) / 2.
    assert_eq!(scored.total, 75);
}

#[test]
fn nothing_known_scores_zero_with_every_part_missing() {
    let scored = score(&Signals::default(), today());
    assert_eq!(scored.total, 0);
    assert_eq!(scored.missing, ScoreArea::ALL);
}

fn bet() -> Signals {
    Signals {
        launched: Some(today() - TimeDelta::days(BET_MAX_AGE_DAYS)),
        blueprint: Some(percent("80")),
        temperature: Some(BET_MAX_TEMPERATURE),
        ..Signals::default()
    }
}

#[test]
fn an_aposta_is_new_good_and_cool_at_each_edge() {
    assert!(is_bet(&bet(), today()));
    let older = Signals {
        launched: Some(today() - TimeDelta::days(BET_MAX_AGE_DAYS + 1)),
        ..bet()
    };
    assert!(!is_bet(&older, today()));
    let unknown_age = Signals {
        launched: None,
        ..bet()
    };
    assert!(!is_bet(&unknown_age, today()));
    let weaker = Signals {
        blueprint: Some(percent("79.9")),
        ..bet()
    };
    assert!(!is_bet(&weaker, today()));
    let hotter = Signals {
        temperature: Some(BET_MAX_TEMPERATURE + 1),
        ..bet()
    };
    assert!(!is_bet(&hotter, today()));
    let unknown_temperature = Signals {
        temperature: None,
        ..bet()
    };
    assert!(is_bet(&unknown_temperature, today()));
}

#[test]
fn a_well_reviewed_rating_stands_in_for_the_blueprint() {
    let rated = Signals {
        blueprint: None,
        rating: Some(decimal("4.5")),
        reviews: Some(BET_MIN_REVIEWS),
        ..bet()
    };
    assert!(is_bet(&rated, today()));
    let lower = Signals {
        rating: Some(decimal("4.49")),
        ..rated.clone()
    };
    assert!(!is_bet(&lower, today()));
    let fewer = Signals {
        reviews: Some(BET_MIN_REVIEWS - 1),
        ..rated
    };
    assert!(!is_bet(&fewer, today()));
}

#[test]
fn a_launch_date_after_today_is_not_new() {
    let future = Signals {
        launched: Some(today() + TimeDelta::days(1)),
        ..bet()
    };
    assert!(!is_bet(&future, today()));
}

fn any_percent() -> impl Strategy<Value = Option<Percentage>> {
    proptest::option::of(
        (0u32..=1000).prop_map(|tenths| Percentage::new(Decimal::new(tenths.into(), 1)).unwrap()),
    )
}

fn any_brl() -> impl Strategy<Value = Option<Money>> {
    proptest::option::of(
        (0u32..100_000).prop_map(|cents| Money::new(Decimal::new(cents.into(), 2), Currency::Brl)),
    )
}

fn any_signals() -> impl Strategy<Value = Signals> {
    (
        (
            any_brl(),
            any_brl(),
            any_percent(),
            proptest::option::of(0u32..=150),
        ),
        (
            any_percent(),
            any_percent(),
            proptest::option::of(0u32..=50),
            proptest::option::of(0u32..2000),
        ),
        (
            proptest::option::of(0u32..50_000),
            proptest::option::of(0u32..200),
            proptest::option::of(0u32..400),
        ),
        (
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
        ),
        (
            proptest::option::of(0i64..4000),
            proptest::option::of(prop_oneof![Just(Support::Good), Just(Support::Bad)]),
        ),
    )
        .prop_map(
            |(
                (price, commission, commission_rate, temperature),
                (blueprint, satisfaction, rating_tenths, reviews),
                (students, classes, hours_tenths),
                (certificate, video, creator_verified),
                (creator_days, support),
            )| Signals {
                price,
                commission,
                commission_rate,
                temperature,
                blueprint,
                satisfaction,
                rating: rating_tenths.map(|tenths| Decimal::new(tenths.into(), 1)),
                reviews,
                students,
                classes,
                hours: hours_tenths.map(|tenths| Decimal::new(tenths.into(), 1)),
                certificate,
                video,
                creator_verified,
                creator_since: creator_days.map(|days| today() - TimeDelta::days(days)),
                support,
                ..Signals::default()
            },
        )
}

/// `signals` with one signal raised, when it has it.
fn raised(signals: &Signals, which: usize, by: u32) -> Signals {
    let mut up = signals.clone();
    let more_money = |money: Option<Money>| {
        money.map(|m| Money::new(m.amount() + Decimal::from(by), m.currency()))
    };
    let more_percent = |rate: Option<Percentage>| {
        rate.map(|rate| {
            Percentage::new((rate.percent() + Decimal::from(by)).min(Decimal::ONE_HUNDRED)).unwrap()
        })
    };
    let more = |count: Option<u32>| count.map(|count| count + by);
    match which {
        0 => up.price = more_money(up.price),
        1 => up.commission = more_money(up.commission),
        2 => up.commission_rate = more_percent(up.commission_rate),
        3 => up.temperature = up.temperature.map(|t| (t + by).min(150)),
        4 => up.blueprint = more_percent(up.blueprint),
        5 => up.satisfaction = more_percent(up.satisfaction),
        6 => {
            up.rating = up
                .rating
                .map(|r| (r + Decimal::new(by.into(), 2)).min(Decimal::from(5)))
        }
        7 => up.reviews = more(up.reviews),
        8 => up.students = more(up.students),
        9 => up.classes = more(up.classes),
        10 => up.hours = up.hours.map(|h| h + Decimal::from(by)),
        11 => up.certificate = up.certificate.map(|_| true),
        12 => up.video = up.video.map(|_| true),
        13 => up.creator_verified = up.creator_verified.map(|_| true),
        14 => {
            up.creator_since = up
                .creator_since
                .map(|since| since - TimeDelta::days(by.into()))
        }
        _ => up.support = up.support.map(|_| Support::Good),
    }
    up
}

proptest! {
    #[test]
    fn the_score_stays_between_0_and_100_with_parts_that_add_up(signals in any_signals()) {
        let scored = score(&signals, today());
        prop_assert!(scored.total <= 100);
        prop_assert_eq!(scored.parts.len() + scored.missing.len(), 4);
        for part in &scored.parts {
            prop_assert!(part.points <= 100);
            prop_assert!(!part.reason.is_empty());
        }
        prop_assert_eq!(scored.partial(), !scored.missing.is_empty());
    }

    #[test]
    fn the_same_signals_on_the_same_day_give_the_same_score(signals in any_signals()) {
        prop_assert_eq!(score(&signals, today()), score(&signals.clone(), today()));
        prop_assert_eq!(is_bet(&signals, today()), is_bet(&signals.clone(), today()));
    }

    #[test]
    fn a_better_signal_never_lowers_the_score(
        signals in any_signals(),
        which in 0usize..16,
        by in 1u32..500,
    ) {
        let before = score(&signals, today());
        let after = score(&raised(&signals, which, by), today());
        prop_assert!(after.total >= before.total, "{} -> {}", before.total, after.total);
        for (low, high) in before.parts.iter().zip(&after.parts) {
            prop_assert!(high.points >= low.points, "{:?}: {} -> {}", low.area, low.points, high.points);
        }
    }
}
