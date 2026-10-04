use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::Clock;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::{
    Appearance, Database, DuplicateKey, Flag, FlagChange, FlagError, FlagKind, Flags, LayoutId,
    MIGRATIONS, ModuleMigrations, Phase, Registry, UiTheme, UiThemePreference, load_appearance,
    migrate, save_appearance, system_user,
};

const CRAWLER: Flag = Flag {
    key: "catalog.shopee_crawler",
    name: "Crawler da Shopee",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "Os termos da Shopee proíbem robôs.",
    risk: "Bloqueio da conta.",
};

const AUTO_ANSWER: Flag = Flag {
    key: "marketing.auto_answer_questions",
    name: "Resposta automática",
    kind: FlagKind::ProductChoice,
    phase: Phase::One,
    reason: "Toda resposta passa por você.",
    risk: "Resposta errada pública.",
};

const UNREGISTERED: Flag = Flag {
    key: "commerce.not_declared",
    ..CRAWLER
};

fn clock_at_noon() -> Arc<ManualClock> {
    Arc::new(ManualClock::at(
        Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
    ))
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    flags: Flags,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let clock = clock_at_noon();
    let database = Database::open(&dir.path().join("mascate.db"))
        .await
        .unwrap();
    migrate(&database, clock.as_ref(), &[MIGRATIONS])
        .await
        .unwrap();
    let flags = Flags::new(
        Arc::new(database),
        Registry::new(&[&[CRAWLER], &[AUTO_ANSWER]]).unwrap(),
        clock.clone(),
        Arc::new(SequentialIds::default()),
    );
    Fixture {
        _dir: dir,
        clock,
        flags,
    }
}

async fn turn_on(flags: &Flags, flag: &Flag, by: &str) {
    let request = flags.request_turn_on(flag.key).unwrap();
    flags.turn_on(request.confirm(by)).await.unwrap();
}

#[test]
fn every_flag_starts_off_and_its_guard_refuses_the_action() {
    block_on(async {
        let Fixture { flags, .. } = fixture().await;

        for status in flags.statuses().await.unwrap() {
            assert!(!status.is_on(), "{}", status.flag.key);
            assert_eq!(status.last_change, None);
        }
        assert!(!flags.is_on(&CRAWLER).await.unwrap());
        assert!(matches!(
            flags.ensure_on(&CRAWLER).await,
            Err(FlagError::Off("Crawler da Shopee"))
        ));
    });
}

#[test]
fn statuses_follow_the_order_the_modules_declared() {
    block_on(async {
        let Fixture { flags, .. } = fixture().await;

        let keys: Vec<_> = flags
            .statuses()
            .await
            .unwrap()
            .into_iter()
            .map(|status| status.flag.key)
            .collect();

        assert_eq!(keys, [CRAWLER.key, AUTO_ANSWER.key]);
    });
}

#[test]
fn a_request_carries_the_risk_and_changes_nothing_until_confirmed() {
    block_on(async {
        let Fixture { flags, .. } = fixture().await;

        let request = flags.request_turn_on(CRAWLER.key).unwrap();
        assert_eq!(request.flag().risk, "Bloqueio da conta.");
        assert!(!flags.is_on(&CRAWLER).await.unwrap());

        // The owner cancels: the request is dropped unconfirmed.
        drop(request);
        assert!(!flags.is_on(&CRAWLER).await.unwrap());
        assert!(flags.ensure_on(&CRAWLER).await.is_err());
    });
}

#[test]
fn a_confirmed_flag_is_on_and_records_when_and_by_whom() {
    block_on(async {
        let Fixture { flags, clock, .. } = fixture().await;
        let at = clock.now();

        turn_on(&flags, &CRAWLER, "fernando").await;

        assert!(flags.is_on(&CRAWLER).await.unwrap());
        flags.ensure_on(&CRAWLER).await.unwrap();
        assert!(!flags.is_on(&AUTO_ANSWER).await.unwrap());
        let status = flags.statuses().await.unwrap().remove(0);
        assert_eq!(
            status.last_change,
            Some(FlagChange {
                on: true,
                at,
                by: "fernando".into(),
            })
        );
    });
}

#[test]
fn turning_on_again_keeps_the_first_switch() {
    block_on(async {
        let Fixture { flags, clock, .. } = fixture().await;
        let first = clock.now();
        turn_on(&flags, &CRAWLER, "fernando").await;

        clock.advance(TimeDelta::hours(1));
        turn_on(&flags, &CRAWLER, "outra pessoa").await;

        let change = flags
            .statuses()
            .await
            .unwrap()
            .remove(0)
            .last_change
            .unwrap();
        assert_eq!((change.at, change.by.as_str()), (first, "fernando"));
    });
}

#[test]
fn turning_off_needs_no_confirmation_and_the_guard_refuses_again() {
    block_on(async {
        let Fixture { flags, clock, .. } = fixture().await;
        turn_on(&flags, &CRAWLER, "fernando").await;
        clock.advance(TimeDelta::minutes(5));
        let off_at = clock.now();

        flags.turn_off(CRAWLER.key, "fernando").await.unwrap();

        assert!(!flags.is_on(&CRAWLER).await.unwrap());
        assert!(flags.ensure_on(&CRAWLER).await.is_err());
        let change = flags
            .statuses()
            .await
            .unwrap()
            .remove(0)
            .last_change
            .unwrap();
        assert_eq!((change.on, change.at), (false, off_at));
    });
}

#[test]
fn turning_off_a_flag_that_was_never_on_records_nothing() {
    block_on(async {
        let Fixture { flags, .. } = fixture().await;

        flags.turn_off(AUTO_ANSWER.key, "fernando").await.unwrap();

        let status = flags.statuses().await.unwrap().remove(1);
        assert_eq!(status.last_change, None);
    });
}

#[test]
fn switches_in_the_same_instant_resolve_to_the_last_one() {
    block_on(async {
        let Fixture { flags, .. } = fixture().await;

        turn_on(&flags, &CRAWLER, "fernando").await;
        flags.turn_off(CRAWLER.key, "fernando").await.unwrap();
        turn_on(&flags, &CRAWLER, "fernando").await;

        assert!(flags.is_on(&CRAWLER).await.unwrap());
        assert!(flags.statuses().await.unwrap()[0].is_on());
    });
}

#[test]
fn the_state_survives_reopening_the_database() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mascate.db");
        let clock = clock_at_noon();
        let open = || async {
            let database = Database::open(&path).await.unwrap();
            migrate(&database, clock.as_ref(), &[MIGRATIONS])
                .await
                .unwrap();
            Flags::new(
                Arc::new(database),
                Registry::new(&[&[CRAWLER, AUTO_ANSWER]]).unwrap(),
                clock.clone(),
                Arc::new(SequentialIds::default()),
            )
        };
        turn_on(&open().await, &AUTO_ANSWER, "fernando").await;

        let reopened = open().await;
        assert!(reopened.is_on(&AUTO_ANSWER).await.unwrap());
        assert!(!reopened.is_on(&CRAWLER).await.unwrap());
    });
}

#[test]
fn unregistered_flags_are_refused_everywhere() {
    block_on(async {
        let Fixture { flags, .. } = fixture().await;

        assert!(matches!(
            flags.is_on(&UNREGISTERED).await,
            Err(FlagError::Unknown(key)) if key == UNREGISTERED.key
        ));
        assert!(matches!(
            flags.ensure_on(&UNREGISTERED).await,
            Err(FlagError::Unknown(_))
        ));
        assert!(matches!(
            flags.request_turn_on(UNREGISTERED.key),
            Err(FlagError::Unknown(_))
        ));
        assert!(matches!(
            flags.turn_off(UNREGISTERED.key, "fernando").await,
            Err(FlagError::Unknown(_))
        ));
    });
}

#[test]
fn a_confirmation_from_another_registry_is_refused() {
    block_on(async {
        let Fixture { flags, .. } = fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let clock = clock_at_noon();
        let database = Database::open(&dir.path().join("other.db")).await.unwrap();
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        let other = Flags::new(
            Arc::new(database),
            Registry::new(&[&[UNREGISTERED]]).unwrap(),
            clock,
            Arc::new(SequentialIds::default()),
        );

        let confirmed = other
            .request_turn_on(UNREGISTERED.key)
            .unwrap()
            .confirm("fernando");

        assert!(matches!(
            flags.turn_on(confirmed).await,
            Err(FlagError::Unknown(_))
        ));
    });
}

#[test]
fn two_modules_declaring_the_same_key_is_an_error() {
    let same_key = Flag {
        name: "Outro nome",
        ..CRAWLER
    };

    assert_eq!(
        Registry::new(&[&[CRAWLER], &[AUTO_ANSWER, same_key]]).err(),
        Some(DuplicateKey(CRAWLER.key))
    );
}

#[test]
fn the_system_user_comes_from_the_environment() {
    let environment = |values: &'static [(&'static str, &'static str)]| {
        Box::new(move |name: &str| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }) as mascate_platform::Environment
    };

    assert_eq!(
        system_user(&environment(&[("USERNAME", "ferna")])).as_deref(),
        Some("ferna")
    );
    assert_eq!(
        system_user(&environment(&[("USER", "fernando")])).as_deref(),
        Some("fernando")
    );
    assert_eq!(
        system_user(&environment(&[("USERNAME", "  "), ("USER", "fernando")])).as_deref(),
        Some("fernando")
    );
    assert_eq!(system_user(&environment(&[])), None);
}

#[test]
fn a_database_from_the_first_release_migrates_and_keeps_the_appearance() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mascate.db");
        let clock = ManualClock::at(Utc.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap());
        let ids = SequentialIds::default();
        let chosen = Appearance {
            theme: UiThemePreference::Fixed(UiTheme::Brass),
            layout: LayoutId::Studio,
        };
        {
            let first_release = ModuleMigrations {
                module: MIGRATIONS.module,
                migrations: &MIGRATIONS.migrations[..1],
            };
            let database = Database::open(&path).await.unwrap();
            migrate(&database, &clock, &[first_release]).await.unwrap();
            save_appearance(&database, &clock, &ids, chosen)
                .await
                .unwrap();
        }

        let database = Database::open(&path).await.unwrap();
        let applied = migrate(&database, &clock, &[MIGRATIONS]).await.unwrap();

        assert_eq!(applied.len(), MIGRATIONS.migrations.len() - 1);
        assert_eq!(load_appearance(&database).await.unwrap(), chosen);
        let flags = Flags::new(
            Arc::new(database),
            Registry::new(&[&[CRAWLER]]).unwrap(),
            Arc::new(clock),
            Arc::new(ids),
        );
        assert!(!flags.is_on(&CRAWLER).await.unwrap());
    });
}
