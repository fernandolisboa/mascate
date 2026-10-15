//! The Sales Events calendar through the Marketing module's public
//! interface, with a real temporary database and an injected clock and ID
//! generator: next dates across years and for moving dates, the common
//! dates recorded once, and Hoje's lead time.

use std::sync::Arc;

use chrono::{NaiveDate, TimeDelta, TimeZone, Weekday};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Timestamp, brasilia};
use mascate_marketing::{
    DateRule, EventError, MAX_LEAD_DAYS, MIGRATIONS, NewSalesEvent, SalesEvent, SalesEvents,
    default_sales_events,
};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// `day`/`month`/`year` at `hour` on Brasília's clock.
fn brt(year: i32, month: u32, day: u32, hour: u32) -> Timestamp {
    brasilia()
        .with_ymd_and_hms(year, month, day, hour, 0, 0)
        .unwrap()
        .to_utc()
}

const MOTHERS_DAY: DateRule = DateRule::NthWeekday {
    nth: 2,
    weekday: Weekday::Sun,
    month: 5,
};
const FATHERS_DAY: DateRule = DateRule::NthWeekday {
    nth: 2,
    weekday: Weekday::Sun,
    month: 8,
};
const BLACK_FRIDAY: DateRule = DateRule::AfterThanksgiving { days: 1 };
const CYBER_MONDAY: DateRule = DateRule::AfterThanksgiving { days: 4 };

struct Fixture {
    dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    ids: Arc<SequentialIds>,
    events: SalesEvents,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(brt(2026, 10, 15, 9)));
        let ids = Arc::new(SequentialIds::default());
        let events = Self::open(&dir, &clock, &ids).await;
        Self {
            dir,
            clock,
            ids,
            events,
        }
    }

    /// The calendar on the database file, as the app opens it at start.
    async fn open(
        dir: &tempfile::TempDir,
        clock: &Arc<ManualClock>,
        ids: &Arc<SequentialIds>,
    ) -> SalesEvents {
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        SalesEvents::new(database, clock.clone(), ids.clone())
    }

    /// The app closes and opens again on the same database.
    async fn restart(&mut self) {
        self.events = Self::open(&self.dir, &self.clock, &self.ids).await;
    }

    async fn create(&self, name: &str, rule: DateRule, lead_days: u32) -> SalesEvent {
        self.events
            .create(&NewSalesEvent {
                name: name.into(),
                rule,
                lead_days,
                notes: String::new(),
            })
            .await
            .unwrap()
    }

    async fn names(&self, at: Timestamp) -> Vec<String> {
        self.events
            .events(at)
            .await
            .unwrap()
            .into_iter()
            .map(|event| event.name)
            .collect()
    }
}

#[test]
fn moving_dates_are_right_year_after_year() {
    let expected = [
        // Mother's Day, Father's Day, Black Friday, Cyber Monday.
        (2026, (5, 10), (8, 9), (11, 27), (11, 30)),
        (2027, (5, 9), (8, 8), (11, 26), (11, 29)),
        (2028, (5, 14), (8, 13), (11, 24), (11, 27)),
        (2029, (5, 13), (8, 12), (11, 23), (11, 26)),
        // Thanksgiving on the 28th puts Cyber Monday in December.
        (2030, (5, 12), (8, 11), (11, 29), (12, 2)),
        (2031, (5, 11), (8, 10), (11, 28), (12, 1)),
    ];
    for (year, mothers, fathers, black_friday, cyber_monday) in expected {
        let from = date(year, 1, 1);
        for (rule, (month, day)) in [
            (MOTHERS_DAY, mothers),
            (FATHERS_DAY, fathers),
            (BLACK_FRIDAY, black_friday),
            (CYBER_MONDAY, cyber_monday),
        ] {
            assert_eq!(
                rule.next_date(from),
                Some(date(year, month, day)),
                "{rule:?} in {year}"
            );
        }
    }
}

#[test]
fn a_passed_date_moves_to_the_next_year_and_the_day_itself_counts() {
    let christmas = DateRule::Yearly { day: 25, month: 12 };
    assert_eq!(
        christmas.next_date(date(2026, 12, 25)),
        Some(date(2026, 12, 25))
    );
    assert_eq!(
        christmas.next_date(date(2026, 12, 26)),
        Some(date(2027, 12, 25))
    );
    assert_eq!(
        MOTHERS_DAY.next_date(date(2026, 5, 11)),
        Some(date(2027, 5, 9))
    );
    assert_eq!(
        CYBER_MONDAY.next_date(date(2030, 12, 1)),
        Some(date(2030, 12, 2)),
        "Cyber Monday in December is still that year's"
    );
    assert_eq!(
        BLACK_FRIDAY.next_date(date(2026, 11, 28)),
        Some(date(2027, 11, 26))
    );
}

#[test]
fn the_29th_of_february_waits_for_a_leap_year() {
    let leap_day = DateRule::Yearly { day: 29, month: 2 };
    assert_eq!(
        leap_day.next_date(date(2026, 3, 1)),
        Some(date(2028, 2, 29))
    );
    assert_eq!(
        leap_day.next_date(date(2096, 3, 1)),
        Some(date(2104, 2, 29)),
        "2100 is not a leap year"
    );
}

#[test]
fn a_single_date_has_no_next_date_once_past() {
    let prime_day = DateRule::Once(date(2026, 7, 8));
    assert_eq!(
        prime_day.next_date(date(2026, 7, 8)),
        Some(date(2026, 7, 8))
    );
    assert_eq!(
        prime_day.next_date(date(2025, 1, 1)),
        Some(date(2026, 7, 8))
    );
    assert_eq!(prime_day.next_date(date(2026, 7, 9)), None);
}

#[test]
fn the_common_dates_are_recorded_once_and_a_deleted_one_stays_deleted() {
    block_on(async {
        let mut fixture = Fixture::new().await;
        assert!(fixture.events.record_defaults().await.unwrap());
        assert!(!fixture.events.record_defaults().await.unwrap());
        let at = fixture.clock.now();
        let events = fixture.events.events(at).await.unwrap();
        assert_eq!(events.len(), default_sales_events().len());
        let mut names: Vec<String> = events.iter().map(|event| event.name.clone()).collect();
        names.sort();
        for name in [
            "Dia do Consumidor",
            "Shopee 4.4",
            "Shopee 12.12",
            "Dia das Mães",
            "Dia dos Pais",
            "Dia das Crianças",
            "Black Friday",
            "Cyber Monday",
            "Natal",
        ] {
            assert!(
                names.contains(&name.to_owned()),
                "{name} is on the calendar"
            );
        }
        let consumer = events
            .iter()
            .find(|event| event.name == "Dia do Consumidor")
            .unwrap();
        assert_eq!(consumer.rule, DateRule::Yearly { day: 15, month: 3 });

        let christmas = events.iter().find(|event| event.name == "Natal").unwrap();
        fixture.events.delete(christmas.id).await.unwrap();
        fixture.restart().await;
        assert!(!fixture.events.record_defaults().await.unwrap());
        let names = fixture.names(at).await;
        assert_eq!(names.len(), default_sales_events().len() - 1);
        assert!(
            !names.contains(&"Natal".to_owned()),
            "a deleted date never comes back"
        );
    });
}

#[test]
fn two_first_opens_at_once_record_the_common_dates_once() {
    block_on(async {
        let fixture = Fixture::new().await;
        let other = Fixture::open(&fixture.dir, &fixture.clock, &fixture.ids).await;
        let (first, second) =
            futures::join!(fixture.events.record_defaults(), other.record_defaults());
        assert!(
            first.unwrap() ^ second.unwrap(),
            "exactly one of them records"
        );
        assert_eq!(
            fixture.names(fixture.clock.now()).await.len(),
            default_sales_events().len()
        );
    });
}

#[test]
fn an_edited_common_date_keeps_the_edit_after_restarting() {
    block_on(async {
        let mut fixture = Fixture::new().await;
        fixture.events.record_defaults().await.unwrap();
        let at = fixture.clock.now();
        let black_friday = fixture
            .events
            .events(at)
            .await
            .unwrap()
            .into_iter()
            .find(|event| event.name == "Black Friday")
            .unwrap();
        fixture
            .events
            .update(
                black_friday.id,
                &NewSalesEvent {
                    name: " Black Friday da Shopee ".into(),
                    rule: BLACK_FRIDAY,
                    lead_days: 30,
                    notes: "Separar os cupons na semana anterior.".into(),
                },
            )
            .await
            .unwrap();
        fixture.restart().await;
        fixture.events.record_defaults().await.unwrap();
        let edited = fixture.events.event(black_friday.id).await.unwrap();
        assert_eq!(edited.name, "Black Friday da Shopee");
        assert_eq!(edited.lead_days, 30);
        assert_eq!(edited.notes, "Separar os cupons na semana anterior.");
        assert!(!fixture.names(at).await.contains(&"Black Friday".to_owned()));
    });
}

#[test]
fn every_kind_of_rule_is_kept_as_written() {
    block_on(async {
        let fixture = Fixture::new().await;
        for rule in [
            DateRule::Yearly { day: 29, month: 2 },
            DateRule::NthWeekday {
                nth: 4,
                weekday: Weekday::Sat,
                month: 11,
            },
            DateRule::AfterThanksgiving { days: 0 },
            DateRule::Once(date(2027, 7, 13)),
        ] {
            let created = fixture.create("Evento", rule, 0).await;
            assert_eq!(fixture.events.event(created.id).await.unwrap().rule, rule);
        }
    });
}

#[test]
fn upcoming_counts_days_on_brasilias_clock() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.events.record_defaults().await.unwrap();
        fixture
            .create("Aniversário da loja", DateRule::Once(date(2026, 10, 15)), 0)
            .await;
        // 23:00 on the 14th in Brasília, already the 15th in UTC.
        let from = brt(2026, 10, 14, 23);
        assert_eq!(from.date_naive(), date(2026, 10, 15));

        let upcoming = fixture.events.upcoming(from, 45).await.unwrap();
        let read: Vec<(&str, NaiveDate, u32)> = upcoming
            .iter()
            .map(|next| (next.event.name.as_str(), next.date, next.days_away))
            .collect();
        assert_eq!(
            read,
            vec![
                ("Aniversário da loja", date(2026, 10, 15), 1),
                ("Shopee 11.11", date(2026, 11, 11), 28),
                ("Black Friday", date(2026, 11, 27), 44),
            ],
            "Dia das Crianças and Shopee 10.10 passed; Cyber Monday is 47 days away"
        );
    });
}

#[test]
fn hoje_shows_only_events_inside_their_lead_time() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.events.record_defaults().await.unwrap();
        let shown = |at: Timestamp| {
            let events = &fixture.events;
            async move {
                events
                    .in_lead_time(at)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|next| (next.event.name, next.days_away))
                    .collect::<Vec<_>>()
            }
        };
        // Black Friday (lead 21) is 21 days away; Shopee 11.11 (lead 3) is 5
        // days away, Cyber Monday (lead 7) 24 and Natal (lead 21) 49.
        assert_eq!(
            shown(brt(2026, 11, 6, 9)).await,
            vec![("Black Friday".to_owned(), 21)]
        );
        assert_eq!(
            shown(brt(2026, 11, 8, 9)).await,
            vec![
                ("Shopee 11.11".to_owned(), 3),
                ("Black Friday".to_owned(), 19)
            ]
        );
        assert_eq!(
            shown(brt(2026, 11, 11, 23)).await,
            vec![
                ("Shopee 11.11".to_owned(), 0),
                ("Black Friday".to_owned(), 16)
            ],
            "the day itself still shows"
        );
        assert!(
            shown(brt(2026, 9, 1, 9)).await.is_empty(),
            "Shopee 9.9 is 8 days away with a lead of 3"
        );
    });
}

#[test]
fn the_calendar_lists_by_next_date_with_past_single_dates_last() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .create("Natal", DateRule::Yearly { day: 25, month: 12 }, 21)
            .await;
        fixture
            .create("Prime Day", DateRule::Once(date(2026, 7, 8)), 7)
            .await;
        fixture.create("Black Friday", BLACK_FRIDAY, 21).await;
        fixture.create("Dia das Mães", MOTHERS_DAY, 14).await;
        assert_eq!(
            fixture.names(brt(2026, 10, 15, 9)).await,
            vec!["Black Friday", "Natal", "Dia das Mães", "Prime Day"]
        );
    });
}

#[test]
fn an_event_is_refused_when_its_fields_have_no_valid_form() {
    block_on(async {
        let fixture = Fixture::new().await;
        let event = |name: &str, rule: DateRule, lead_days: u32, notes: String| NewSalesEvent {
            name: name.into(),
            rule,
            lead_days,
            notes,
        };
        let christmas = DateRule::Yearly { day: 25, month: 12 };
        for (refused, expected) in [
            (event("  ", christmas, 7, String::new()), "BadName"),
            (
                event(&"x".repeat(61), christmas, 7, String::new()),
                "BadName",
            ),
            (
                event(
                    "Dia 31",
                    DateRule::Yearly { day: 31, month: 2 },
                    7,
                    String::new(),
                ),
                "BadRule",
            ),
            (
                event(
                    "Quinto domingo",
                    DateRule::NthWeekday {
                        nth: 5,
                        weekday: Weekday::Sun,
                        month: 3,
                    },
                    7,
                    String::new(),
                ),
                "BadRule",
            ),
            (
                event(
                    "Mês 13",
                    DateRule::NthWeekday {
                        nth: 1,
                        weekday: Weekday::Mon,
                        month: 13,
                    },
                    7,
                    String::new(),
                ),
                "BadRule",
            ),
            (
                event(
                    "Longe",
                    DateRule::AfterThanksgiving { days: 31 },
                    7,
                    String::new(),
                ),
                "BadRule",
            ),
            (
                event("Natal", christmas, MAX_LEAD_DAYS + 1, String::new()),
                "BadLeadDays",
            ),
            (event("Natal", christmas, 7, "x".repeat(501)), "LongNotes"),
        ] {
            let error = fixture.events.create(&refused).await.unwrap_err();
            assert_eq!(format!("{error:?}"), expected, "{refused:?}");
        }
        assert!(fixture.names(brt(2026, 10, 15, 9)).await.is_empty());
    });
}

#[test]
fn deleting_twice_finds_nothing_the_second_time() {
    block_on(async {
        let fixture = Fixture::new().await;
        let natal = fixture
            .create("Natal", DateRule::Yearly { day: 25, month: 12 }, 21)
            .await;
        fixture.events.delete(natal.id).await.unwrap();
        assert!(matches!(
            fixture.events.delete(natal.id).await,
            Err(EventError::NotFound)
        ));
        assert!(matches!(
            fixture.events.event(natal.id).await,
            Err(EventError::NotFound)
        ));
        assert!(matches!(
            fixture
                .events
                .update(
                    natal.id,
                    &NewSalesEvent {
                        name: "Natal".into(),
                        rule: DateRule::Yearly { day: 25, month: 12 },
                        lead_days: 21,
                        notes: String::new(),
                    },
                )
                .await,
            Err(EventError::NotFound)
        ));
    });
}

/// A day from 2020 to the end of 2101.
fn any_day() -> impl Strategy<Value = NaiveDate> {
    (0i64..82 * 366).prop_map(|offset| date(2020, 1, 1) + TimeDelta::days(offset))
}

fn any_rule() -> impl Strategy<Value = DateRule> {
    let weekdays = prop::sample::select(vec![
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ]);
    prop_oneof![
        // Every day of a leap year, 29 February included.
        (0i64..366).prop_map(|offset| {
            let day = date(2000, 1, 1) + TimeDelta::days(offset);
            DateRule::Yearly {
                day: chrono::Datelike::day(&day),
                month: chrono::Datelike::month(&day),
            }
        }),
        (1u8..=4, weekdays, 1u32..=12).prop_map(|(nth, weekday, month)| DateRule::NthWeekday {
            nth,
            weekday,
            month
        }),
        (0u8..=30).prop_map(|days| DateRule::AfterThanksgiving { days }),
        any_day().prop_map(DateRule::Once),
    ]
}

proptest! {
    #[test]
    fn the_next_date_is_never_before_from_and_is_the_first_the_rule_falls_on(
        rule in any_rule(),
        from in any_day(),
    ) {
        match rule.next_date(from) {
            Some(next) => {
                prop_assert!(next >= from, "never before from");
                prop_assert!(rule.falls_on(next), "a day the rule falls on");
                let mut day = from;
                while day < next {
                    prop_assert!(!rule.falls_on(day), "{day} comes first");
                    day += TimeDelta::days(1);
                }
            }
            None => prop_assert!(
                matches!(rule, DateRule::Once(single) if single < from),
                "only a single date already past has none"
            ),
        }
    }
}
