use chrono::{TimeDelta, TimeZone, Utc};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, IdGenerator, Record, UuidV7Generator};
use uuid::Uuid;

fn clock() -> ManualClock {
    ManualClock::at(Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap())
}

#[test]
fn new_record_gets_an_id_and_equal_timestamps() {
    let clock = clock();
    let record = Record::new(&SequentialIds::default(), &clock);

    assert_eq!(record.id, Uuid::from_u128(1));
    assert_eq!(record.created_at, clock.now());
    assert_eq!(record.updated_at, record.created_at);
    assert!(!record.is_deleted());
}

#[test]
fn touch_moves_only_updated_at() {
    let clock = clock();
    let mut record = Record::new(&SequentialIds::default(), &clock);
    clock.advance(TimeDelta::minutes(5));

    record.touch(&clock);

    assert_eq!(record.updated_at - record.created_at, TimeDelta::minutes(5));
}

#[test]
fn soft_delete_keeps_the_row_and_counts_as_an_update() {
    let clock = clock();
    let mut record = Record::new(&SequentialIds::default(), &clock);
    clock.advance(TimeDelta::seconds(1));

    record.soft_delete(&clock);

    assert!(record.is_deleted());
    assert_eq!(record.deleted_at, Some(record.updated_at));
    assert!(record.updated_at > record.created_at);
}

#[test]
fn restore_clears_the_deletion_and_touches() {
    let clock = clock();
    let mut record = Record::new(&SequentialIds::default(), &clock);
    record.soft_delete(&clock);
    clock.advance(TimeDelta::seconds(1));

    record.restore(&clock);

    assert!(!record.is_deleted());
    assert_eq!(record.updated_at - record.created_at, TimeDelta::seconds(1));
}

#[test]
fn sequential_ids_are_distinct_and_predictable() {
    let ids = SequentialIds::default();
    assert_eq!(ids.next_id(), Uuid::from_u128(1));
    assert_eq!(ids.next_id(), Uuid::from_u128(2));
}

#[test]
fn production_ids_are_uuid_v7_and_time_ordered() {
    let ids = UuidV7Generator;
    let first = ids.next_id();
    let second = ids.next_id();

    assert_eq!(first.get_version_num(), 7);
    assert!(second > first);
}
