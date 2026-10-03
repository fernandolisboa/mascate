use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::{
    Appearance, Database, LayoutId, MIGRATIONS, UiTheme, UiThemePreference, load_appearance,
    migrate, save_appearance,
};

fn clock() -> ManualClock {
    ManualClock::at(Utc.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap())
}

async fn fresh(dir: &tempfile::TempDir, clock: &ManualClock) -> Database {
    let database = Database::open(&dir.path().join("mascate.db"))
        .await
        .unwrap();
    migrate(&database, clock, &[MIGRATIONS]).await.unwrap();
    database
}

#[test]
fn a_new_database_follows_the_system_in_the_workspace_layout() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let database = fresh(&dir, &clock()).await;

        let appearance = load_appearance(&database).await.unwrap();

        assert_eq!(appearance, Appearance::default());
        assert_eq!(appearance.theme, UiThemePreference::default());
        assert_eq!(appearance.layout, LayoutId::Workspace);
    });
}

#[test]
fn the_saved_appearance_survives_reopening_the_database() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (clock, ids) = (clock(), SequentialIds::default());
        let chosen = Appearance {
            theme: UiThemePreference::Fixed(UiTheme::BlackGold),
            layout: LayoutId::Studio,
        };
        {
            let database = fresh(&dir, &clock).await;
            save_appearance(&database, &clock, &ids, chosen)
                .await
                .unwrap();
        }

        let database = fresh(&dir, &clock).await;
        assert_eq!(load_appearance(&database).await.unwrap(), chosen);
    });
}

#[test]
fn saving_again_replaces_the_previous_choice() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (clock, ids) = (clock(), SequentialIds::default());
        let database = fresh(&dir, &clock).await;
        let first = Appearance {
            theme: UiThemePreference::Fixed(UiTheme::Phosphor),
            layout: LayoutId::Studio,
        };
        let second = Appearance {
            theme: UiThemePreference::FollowSystem {
                light: UiTheme::Brass,
                dark: UiTheme::BlackGold,
            },
            layout: LayoutId::Workspace,
        };

        save_appearance(&database, &clock, &ids, first)
            .await
            .unwrap();
        clock.advance(TimeDelta::minutes(1));
        save_appearance(&database, &clock, &ids, second)
            .await
            .unwrap();

        assert_eq!(load_appearance(&database).await.unwrap(), second);
    });
}
