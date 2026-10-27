use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::{Database, MIGRATIONS, load_work_context, migrate, save_work_context};

fn clock() -> ManualClock {
    ManualClock::at(Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap())
}

async fn fresh(dir: &tempfile::TempDir, clock: &ManualClock) -> Database {
    let database = Database::open(&dir.path().join("mascate.db"))
        .await
        .unwrap();
    migrate(&database, clock, &[MIGRATIONS]).await.unwrap();
    database
}

#[test]
fn a_new_database_has_no_work_context_saved() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let database = fresh(&dir, &clock()).await;

        assert_eq!(load_work_context(&database).await.unwrap(), None);
    });
}

#[test]
fn the_saved_work_context_survives_reopening_the_database() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (clock, ids) = (clock(), SequentialIds::default());
        {
            let database = fresh(&dir, &clock).await;
            save_work_context(&database, &clock, &ids, "hotmart")
                .await
                .unwrap();
        }

        let database = fresh(&dir, &clock).await;
        assert_eq!(
            load_work_context(&database).await.unwrap().as_deref(),
            Some("hotmart")
        );
    });
}

#[test]
fn saving_again_replaces_the_previous_context() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (clock, ids) = (clock(), SequentialIds::default());
        let database = fresh(&dir, &clock).await;

        save_work_context(&database, &clock, &ids, "shopee")
            .await
            .unwrap();
        clock.advance(TimeDelta::minutes(1));
        save_work_context(&database, &clock, &ids, "all")
            .await
            .unwrap();

        assert_eq!(
            load_work_context(&database).await.unwrap().as_deref(),
            Some("all")
        );
    });
}

#[test]
fn a_database_waiting_for_its_migrations_reads_as_nothing_saved() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("mascate.db"))
            .await
            .unwrap();

        assert_eq!(load_work_context(&database).await.unwrap(), None);
    });
}
