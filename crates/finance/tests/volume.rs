use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_finance::{MIGRATIONS, REMINDERS, SALES_VOLUME, SalesVolume, VolumeError};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money};
use mascate_platform::{Database, Registry, ReminderShows, Reminders, migrate};
use rust_decimal::Decimal;

fn brl(text: &str) -> Money {
    Money::new(Decimal::from_str(text).unwrap(), Currency::Brl)
}

struct Fixture {
    _dir: tempfile::TempDir,
    volume: SalesVolume,
    reminders: Reminders,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(
        Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap(),
    ));
    let database = Arc::new(Database::open(&dir.path().join("m.db")).await.unwrap());
    migrate(
        &database,
        clock.as_ref(),
        &[mascate_platform::MIGRATIONS, MIGRATIONS],
    )
    .await
    .unwrap();
    let ids = Arc::new(SequentialIds::default());
    Fixture {
        _dir: dir,
        volume: SalesVolume::new(database.clone(), clock.clone(), ids.clone()),
        reminders: Reminders::new(database, Registry::new(&[REMINDERS]).unwrap(), clock, ids),
    }
}

#[test]
fn the_limit_starts_at_the_mei_ceiling_and_keeps_the_last_saved() {
    block_on(async {
        let Fixture { volume, .. } = fixture().await;

        assert_eq!(volume.limit().await.unwrap(), brl("81000"));
        volume.save_limit(brl("50000")).await.unwrap();
        volume.save_limit(brl("60000.50")).await.unwrap();

        assert_eq!(volume.limit().await.unwrap(), brl("60000.50"));
    });
}

#[test]
fn a_limit_of_zero_or_less_is_refused() {
    block_on(async {
        let Fixture { volume, .. } = fixture().await;

        assert!(matches!(
            volume.save_limit(brl("0")).await,
            Err(VolumeError::InvalidLimit)
        ));
        assert!(matches!(
            volume.save_limit(brl("-1")).await,
            Err(VolumeError::InvalidLimit)
        ));
        assert_eq!(volume.limit().await.unwrap(), brl("81000"));
    });
}

#[test]
fn only_sales_past_the_limit_raise_the_reminder() {
    block_on(async {
        let Fixture {
            volume, reminders, ..
        } = fixture().await;
        volume.save_limit(brl("50000")).await.unwrap();

        assert!(!volume.check(brl("50000")).await.unwrap().passed());
        let passed = volume.check(brl("50000.01")).await.unwrap();
        assert!(passed.passed());
        assert_eq!(passed.limit, brl("50000"));

        assert_eq!(SALES_VOLUME.shows, ReminderShows::WhenRaised);
        let showing = |raised: Vec<&'static str>| {
            let reminders = &reminders;
            async move {
                reminders
                    .showing(&raised)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|reminder| reminder.key)
                    .collect::<Vec<_>>()
            }
        };
        assert!(!showing(vec![]).await.contains(&SALES_VOLUME.key));
        assert!(
            showing(vec![SALES_VOLUME.key])
                .await
                .contains(&SALES_VOLUME.key)
        );
    });
}
