use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::{
    Database, DuplicateKey, MIGRATIONS, Registry, Reminder, ReminderError, ReminderTopic,
    Reminders, migrate,
};

const CNPJ: Reminder = Reminder {
    key: "finance.selling_without_cnpj",
    topic: ReminderTopic::Fiscal,
    title: "Vendas sem CNPJ",
    text: "Lembrete fiscal.",
    reappears_after_days: 30,
};

const LGPD: Reminder = Reminder {
    key: "commerce.buyer_personal_data",
    topic: ReminderTopic::Legal,
    title: "Dados de compradores",
    text: "Lembrete de LGPD.",
    reappears_after_days: 90,
};

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    reminders: Reminders,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(
        Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
    ));
    let database = Database::open(&dir.path().join("mascate.db"))
        .await
        .unwrap();
    migrate(&database, clock.as_ref(), &[MIGRATIONS])
        .await
        .unwrap();
    let reminders = Reminders::new(
        Arc::new(database),
        Registry::new(&[&[CNPJ], &[LGPD]]).unwrap(),
        clock.clone(),
        Arc::new(SequentialIds::default()),
    );
    Fixture {
        _dir: dir,
        clock,
        reminders,
    }
}

fn keys(reminders: &[Reminder]) -> Vec<&'static str> {
    reminders.iter().map(|reminder| reminder.key).collect()
}

#[test]
fn every_reminder_shows_until_dismissed() {
    block_on(async {
        let Fixture { reminders, .. } = fixture().await;

        assert_eq!(
            keys(&reminders.showing().await.unwrap()),
            [CNPJ.key, LGPD.key]
        );
    });
}

#[test]
fn a_dismissed_reminder_hides_only_itself() {
    block_on(async {
        let Fixture { reminders, .. } = fixture().await;

        reminders.dismiss(CNPJ.key).await.unwrap();

        assert_eq!(keys(&reminders.showing().await.unwrap()), [LGPD.key]);
    });
}

#[test]
fn a_dismissed_reminder_comes_back_when_its_interval_runs_out() {
    block_on(async {
        let Fixture {
            reminders, clock, ..
        } = fixture().await;
        reminders.dismiss(CNPJ.key).await.unwrap();
        reminders.dismiss(LGPD.key).await.unwrap();

        clock.advance(TimeDelta::days(30) - TimeDelta::seconds(1));
        assert!(reminders.showing().await.unwrap().is_empty());

        clock.advance(TimeDelta::seconds(1));
        assert_eq!(keys(&reminders.showing().await.unwrap()), [CNPJ.key]);

        clock.advance(TimeDelta::days(60));
        assert_eq!(
            keys(&reminders.showing().await.unwrap()),
            [CNPJ.key, LGPD.key]
        );
    });
}

#[test]
fn dismissing_again_counts_the_interval_from_the_last_dismissal() {
    block_on(async {
        let Fixture {
            reminders, clock, ..
        } = fixture().await;
        reminders.dismiss(CNPJ.key).await.unwrap();
        clock.advance(TimeDelta::days(30));
        reminders.dismiss(CNPJ.key).await.unwrap();

        clock.advance(TimeDelta::days(29));
        assert!(!keys(&reminders.showing().await.unwrap()).contains(&CNPJ.key));

        clock.advance(TimeDelta::days(1));
        assert!(keys(&reminders.showing().await.unwrap()).contains(&CNPJ.key));
    });
}

#[test]
fn dismissals_survive_reopening_the_database() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mascate.db");
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
        ));
        let open = || async {
            let database = Database::open(&path).await.unwrap();
            migrate(&database, clock.as_ref(), &[MIGRATIONS])
                .await
                .unwrap();
            Reminders::new(
                Arc::new(database),
                Registry::new(&[&[CNPJ, LGPD]]).unwrap(),
                clock.clone(),
                Arc::new(SequentialIds::default()),
            )
        };
        open().await.dismiss(LGPD.key).await.unwrap();

        let reopened = open().await;
        assert_eq!(keys(&reopened.showing().await.unwrap()), [CNPJ.key]);
    });
}

#[test]
fn dismissing_an_unregistered_reminder_is_refused() {
    block_on(async {
        let Fixture { reminders, .. } = fixture().await;

        assert!(matches!(
            reminders.dismiss("finance.nowhere").await,
            Err(ReminderError::Unknown(key)) if key == "finance.nowhere"
        ));
        assert_eq!(reminders.showing().await.unwrap().len(), 2);
    });
}

#[test]
fn two_modules_declaring_the_same_key_is_an_error() {
    let same_key = Reminder {
        title: "Outro",
        ..LGPD
    };

    assert_eq!(
        Registry::new(&[&[CNPJ, LGPD], &[same_key]]).err(),
        Some(DuplicateKey(LGPD.key))
    );
}
