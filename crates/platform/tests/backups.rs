use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::Clock;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::{
    BackupError, BackupSettings, Backups, DEFAULT_KEEP, Database, MAX_KEEP, MIGRATIONS, Migration,
    ModuleMigrations, RestoreError, apply_staged_restore, migrate,
};

const CATALOG_V1: Migration = Migration {
    version: 1,
    name: "create products",
    sql: "CREATE TABLE catalog_products (id TEXT PRIMARY KEY, sku TEXT NOT NULL);",
};
const CATALOG_V2: Migration = Migration {
    version: 2,
    name: "add product title",
    sql: "ALTER TABLE catalog_products ADD COLUMN title TEXT;",
};

const CATALOG_AT_V1: ModuleMigrations = ModuleMigrations {
    module: "catalog",
    migrations: &[CATALOG_V1],
};
const CATALOG_AT_V2: ModuleMigrations = ModuleMigrations {
    module: "catalog",
    migrations: &[CATALOG_V1, CATALOG_V2],
};

/// The app as released before: catalog at version 1.
const OLDER_APP: &[ModuleMigrations] = &[MIGRATIONS, CATALOG_AT_V1];
/// This app: catalog at version 2.
const THIS_APP: &[ModuleMigrations] = &[MIGRATIONS, CATALOG_AT_V2];

struct Fixture {
    dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    database: Arc<Database>,
    backups: Backups,
}

impl Fixture {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn database_path(&self) -> PathBuf {
        self.path("data").join("mascate.db")
    }

    fn default_folder(&self) -> PathBuf {
        self.path("backups")
    }

    async fn add_product(&self, sku: &str) {
        add_product(&self.database, sku).await;
    }
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(
        Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
    ));
    let database = Arc::new(
        Database::open(&dir.path().join("data").join("mascate.db"))
            .await
            .unwrap(),
    );
    migrate(&database, clock.as_ref(), THIS_APP).await.unwrap();
    let backups = Backups::new(
        database.clone(),
        THIS_APP,
        dir.path().join("backups"),
        clock.clone(),
        Arc::new(SequentialIds::default()),
    );
    Fixture {
        dir,
        clock,
        database,
        backups,
    }
}

async fn add_product(database: &Database, sku: &str) {
    database
        .connection()
        .execute(
            "INSERT INTO catalog_products (id, sku) VALUES (?1, ?1)",
            [sku],
        )
        .await
        .unwrap();
}

async fn skus_in(path: &Path) -> Vec<String> {
    let database = Database::open(path).await.unwrap();
    skus(&database).await
}

async fn skus(database: &Database) -> Vec<String> {
    let mut rows = database
        .connection()
        .query("SELECT sku FROM catalog_products ORDER BY sku", ())
        .await
        .unwrap();
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        out.push(row.get(0).unwrap());
    }
    out
}

fn file_names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn settings_start_at_the_default_folder_keeping_fourteen() {
    block_on(async {
        let f = fixture().await;

        let settings = f.backups.settings().await.unwrap();

        assert_eq!(settings.folder, f.default_folder());
        assert_eq!(settings.keep, DEFAULT_KEEP);
        assert_eq!(DEFAULT_KEEP, 14);
    });
}

#[test]
fn saved_settings_are_read_back_and_replace_the_last() {
    block_on(async {
        let f = fixture().await;
        let first = BackupSettings {
            folder: f.path("elsewhere"),
            keep: 3,
        };
        let second = BackupSettings {
            folder: f.path("usb"),
            keep: 30,
        };

        f.backups.save_settings(&first).await.unwrap();
        f.backups.save_settings(&second).await.unwrap();

        assert_eq!(f.backups.settings().await.unwrap(), second);
    });
}

#[test]
fn refuses_a_count_out_of_range_or_a_relative_folder() {
    block_on(async {
        let f = fixture().await;
        for keep in [0, MAX_KEEP + 1] {
            let error = f
                .backups
                .save_settings(&BackupSettings {
                    folder: f.path("b"),
                    keep,
                })
                .await
                .unwrap_err();
            assert!(matches!(error, BackupError::KeepOutOfRange(k) if k == keep));
        }
        let error = f
            .backups
            .save_settings(&BackupSettings {
                folder: PathBuf::from("relative/backups"),
                keep: 5,
            })
            .await
            .unwrap_err();
        assert!(matches!(error, BackupError::FolderNotAbsolute(_)));
        assert_eq!(
            f.backups.settings().await.unwrap().folder,
            f.default_folder()
        );
    });
}

#[test]
fn backup_now_copies_the_database_into_the_folder_and_says_where() {
    block_on(async {
        let f = fixture().await;
        f.add_product("SKU-1").await;

        let backup = f.backups.back_up_now().await.unwrap();

        assert_eq!(
            backup.path,
            f.default_folder().join("mascate-20261004-120000.000Z.db")
        );
        assert_eq!(backup.taken_at, f.clock.now());
        assert_eq!(skus_in(&backup.path).await, ["SKU-1"]);
        assert_eq!(f.backups.list().await.unwrap(), [backup]);
    });
}

#[test]
fn a_backup_holds_the_state_when_it_was_taken() {
    block_on(async {
        let f = fixture().await;
        f.add_product("SKU-1").await;
        let backup = f.backups.back_up_now().await.unwrap();

        f.add_product("SKU-2").await;

        assert_eq!(skus_in(&backup.path).await, ["SKU-1"]);
        assert_eq!(skus(&f.database).await, ["SKU-1", "SKU-2"]);
    });
}

#[test]
fn two_backups_in_the_same_millisecond_both_stay() {
    block_on(async {
        let f = fixture().await;

        let first = f.backups.back_up_now().await.unwrap();
        let second = f.backups.back_up_now().await.unwrap();

        assert_ne!(first.path, second.path);
        assert_eq!(f.backups.list().await.unwrap(), [second, first]);
    });
}

#[test]
fn keeps_only_the_newest_and_leaves_other_files_alone() {
    block_on(async {
        let f = fixture().await;
        let folder = f.path("kept");
        f.backups
            .save_settings(&BackupSettings {
                folder: folder.clone(),
                keep: 3,
            })
            .await
            .unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("notes.txt"), "mine").unwrap();
        std::fs::write(folder.join("mascate.db"), "not a backup name").unwrap();

        let mut taken = Vec::new();
        for _ in 0..5 {
            taken.push(f.backups.back_up_now().await.unwrap());
            f.clock.advance(TimeDelta::hours(1));
        }

        let kept = f.backups.list().await.unwrap();
        let newest: Vec<_> = taken.iter().rev().take(3).cloned().collect();
        assert_eq!(kept, newest);
        assert_eq!(
            file_names(&folder),
            [
                "mascate-20261004-140000.000Z.db",
                "mascate-20261004-150000.000Z.db",
                "mascate-20261004-160000.000Z.db",
                "mascate.db",
                "notes.txt",
            ]
        );
    });
}

#[test]
fn a_lower_count_prunes_on_the_next_backup() {
    block_on(async {
        let f = fixture().await;
        for _ in 0..4 {
            f.backups.back_up_now().await.unwrap();
            f.clock.advance(TimeDelta::hours(1));
        }
        f.backups
            .save_settings(&BackupSettings {
                folder: f.default_folder(),
                keep: 1,
            })
            .await
            .unwrap();
        assert_eq!(f.backups.list().await.unwrap().len(), 4);

        let newest = f.backups.back_up_now().await.unwrap();

        assert_eq!(f.backups.list().await.unwrap(), [newest]);
    });
}

#[test]
fn the_daily_backup_runs_once_a_day() {
    block_on(async {
        let f = fixture().await;

        let first = f.backups.back_up_if_due().await.unwrap();
        assert!(first.is_some());

        f.clock.advance(TimeDelta::hours(23));
        assert_eq!(f.backups.back_up_if_due().await.unwrap(), None);

        f.clock.advance(TimeDelta::hours(1));
        let next = f.backups.back_up_if_due().await.unwrap();
        assert_eq!(next.map(|backup| backup.taken_at), Some(f.clock.now()));
        assert_eq!(f.backups.list().await.unwrap().len(), 2);
    });
}

#[test]
fn a_manual_backup_counts_as_the_days_backup() {
    block_on(async {
        let f = fixture().await;
        f.backups.back_up_now().await.unwrap();

        f.clock.advance(TimeDelta::hours(2));

        assert_eq!(f.backups.back_up_if_due().await.unwrap(), None);
    });
}

#[test]
fn a_backup_from_the_future_does_not_trigger_one_every_check() {
    block_on(async {
        let f = fixture().await;
        f.clock.advance(TimeDelta::days(3));
        f.backups.back_up_now().await.unwrap();

        f.clock.advance(TimeDelta::days(-3));

        assert_eq!(f.backups.back_up_if_due().await.unwrap(), None);
    });
}

#[test]
fn a_backup_taken_during_concurrent_writes_opens_whole() {
    let f = block_on(fixture());
    let path = f.database_path();
    let stop = Arc::new(AtomicBool::new(false));
    // Another connection writes pairs of rows in transactions; a consistent
    // copy never holds half a pair.
    let writer = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            block_on(async {
                let database = Database::open(&path).await.unwrap();
                let mut pair = 0;
                while !stop.load(Ordering::Relaxed) {
                    let transaction = database.connection().transaction().await.unwrap();
                    for side in ["a", "b"] {
                        transaction
                            .execute(
                                "INSERT INTO catalog_products (id, sku) VALUES (?1, ?1)",
                                [format!("{pair:06}-{side}")],
                            )
                            .await
                            .unwrap();
                    }
                    transaction.commit().await.unwrap();
                    pair += 1;
                }
                pair
            })
        })
    };

    let backups: Vec<_> = (0..10)
        .map(|_| {
            f.clock.advance(TimeDelta::seconds(1));
            block_on(f.backups.back_up_now()).unwrap()
        })
        .collect();
    stop.store(true, Ordering::Relaxed);
    let pairs_written = writer.join().unwrap();

    assert!(pairs_written > 0);
    for backup in backups {
        block_on(async {
            let copy = Database::open(&backup.path).await.unwrap();
            let mut rows = copy
                .connection()
                .query("PRAGMA integrity_check", ())
                .await
                .unwrap();
            let check: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
            assert_eq!(check, "ok");
            let skus = skus(&copy).await;
            assert_eq!(skus.len() % 2, 0, "{}", backup.path.display());
        });
    }
}

#[test]
fn export_writes_the_same_copy_where_asked_and_is_not_a_backup() {
    block_on(async {
        let f = fixture().await;
        f.add_product("SKU-1").await;
        let to = f.path("pendrive").join("mascate-export.db");

        f.backups.export(&to).await.unwrap();

        assert_eq!(skus_in(&to).await, ["SKU-1"]);
        assert!(f.backups.list().await.unwrap().is_empty());
        assert_eq!(file_names(&f.path("pendrive")), ["mascate-export.db"]);
    });
}

#[test]
fn export_replaces_a_file_the_owner_chose_to_overwrite() {
    block_on(async {
        let f = fixture().await;
        let to = f.path("mascate-export.db");
        std::fs::write(&to, "old").unwrap();
        f.add_product("SKU-1").await;

        f.backups.export(&to).await.unwrap();

        assert_eq!(skus_in(&to).await, ["SKU-1"]);
    });
}

/// A database of the older app, holding `sku`, at `path`.
async fn older_database(path: &Path, sku: &str) {
    let clock = ManualClock::at(Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).unwrap());
    let database = Database::open(path).await.unwrap();
    migrate(&database, &clock, OLDER_APP).await.unwrap();
    add_product(&database, sku).await;
}

#[test]
fn a_restored_database_from_an_older_version_is_migrated_on_the_next_start() {
    block_on(async {
        let f = fixture().await;
        f.add_product("CURRENT").await;
        let file = f.path("from-old-pc.db");
        older_database(&file, "OLD-PC").await;

        let safety = f.backups.stage_restore(&file).await.unwrap();
        assert_eq!(skus_in(&safety.path).await, ["CURRENT"]);
        // Until the app starts again, the database is the same.
        assert_eq!(skus(&f.database).await, ["CURRENT"]);

        let Fixture {
            dir,
            database,
            backups,
            clock,
        } = f;
        let path = database.path().to_path_buf();
        drop((backups, database));
        assert!(apply_staged_restore(&path).unwrap());
        let restored = Database::open(&path).await.unwrap();
        let applied = migrate(&restored, clock.as_ref(), THIS_APP).await.unwrap();

        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].name, "add product title");
        assert_eq!(skus(&restored).await, ["OLD-PC"]);
        restored
            .connection()
            .execute("UPDATE catalog_products SET title = 'migrated'", ())
            .await
            .unwrap();
        assert!(!dir.path().join("data").join("mascate.db.restore").exists());
    });
}

#[test]
fn restoring_drops_the_journal_of_the_replaced_database() {
    block_on(async {
        let f = fixture().await;
        let file = f.path("backup.db");
        older_database(&file, "RESTORED").await;
        f.backups.stage_restore(&file).await.unwrap();
        let path = f.database_path();
        // A journal left by an app that did not close cleanly.
        std::fs::write(PathBuf::from(format!("{}-wal", path.display())), "stale").unwrap();
        drop(f.backups);
        drop(f.database);

        assert!(apply_staged_restore(&path).unwrap());

        assert!(!PathBuf::from(format!("{}-wal", path.display())).exists());
        assert_eq!(skus_in(&path).await, ["RESTORED"]);
    });
}

#[test]
fn starting_without_a_staged_restore_changes_nothing() {
    block_on(async {
        let f = fixture().await;
        f.add_product("CURRENT").await;
        let path = f.database_path();
        drop(f.backups);
        drop(f.database);

        assert!(!apply_staged_restore(&path).unwrap());

        assert_eq!(skus_in(&path).await, ["CURRENT"]);
    });
}

/// Asserts the restore was refused and nothing was staged or backed up.
async fn assert_refused(f: &Fixture, file: &Path) -> RestoreError {
    let error = f.backups.stage_restore(file).await.unwrap_err();
    assert!(!f.path("data").join("mascate.db.restore").exists());
    assert!(f.backups.list().await.unwrap().is_empty());
    error
}

#[test]
fn refuses_a_file_that_is_not_a_database() {
    block_on(async {
        let f = fixture().await;
        let file = f.path("photo.jpg");
        std::fs::write(&file, vec![0xFF_u8; 8192]).unwrap();

        let error = assert_refused(&f, &file).await;

        assert!(matches!(error, RestoreError::NotMascate(path) if path == file));
    });
}

#[test]
fn refuses_a_missing_file() {
    block_on(async {
        let f = fixture().await;
        let file = f.path("gone.db");

        let error = assert_refused(&f, &file).await;

        assert!(matches!(error, RestoreError::NotMascate(_)));
        assert!(!file.exists());
    });
}

#[test]
fn refuses_a_database_of_another_app() {
    block_on(async {
        let f = fixture().await;
        let file = f.path("other.db");
        let other = Database::open(&file).await.unwrap();
        other
            .connection()
            .execute("CREATE TABLE notes (body TEXT)", ())
            .await
            .unwrap();
        drop(other);

        let error = assert_refused(&f, &file).await;

        assert!(matches!(error, RestoreError::NotMascate(_)));
    });
}

#[test]
fn refuses_a_database_from_a_newer_version() {
    block_on(async {
        let f = fixture().await;
        let file = f.path("newer.db");
        let clock = ManualClock::at(Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap());
        let newer = Database::open(&file).await.unwrap();
        const CATALOG_V3: Migration = Migration {
            version: 3,
            name: "add product brand",
            sql: "ALTER TABLE catalog_products ADD COLUMN brand TEXT;",
        };
        migrate(
            &newer,
            &clock,
            &[
                MIGRATIONS,
                ModuleMigrations {
                    module: "catalog",
                    migrations: &[CATALOG_V1, CATALOG_V2, CATALOG_V3],
                },
            ],
        )
        .await
        .unwrap();
        drop(newer);

        let error = assert_refused(&f, &file).await;

        assert!(matches!(
            error,
            RestoreError::NewerThanApp { ref module, found: 3, known: 2, .. } if module == "catalog"
        ));
    });
}

#[test]
fn refuses_a_database_with_a_module_this_version_does_not_know() {
    block_on(async {
        let f = fixture().await;
        let file = f.path("newer.db");
        let clock = ManualClock::at(Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap());
        let newer = Database::open(&file).await.unwrap();
        migrate(
            &newer,
            &clock,
            &[
                MIGRATIONS,
                ModuleMigrations {
                    module: "loyalty",
                    migrations: &[Migration {
                        version: 1,
                        name: "create points",
                        sql: "CREATE TABLE loyalty_points (id TEXT PRIMARY KEY);",
                    }],
                },
            ],
        )
        .await
        .unwrap();
        drop(newer);

        let error = assert_refused(&f, &file).await;

        assert!(matches!(
            error,
            RestoreError::NewerThanApp { ref module, found: 1, known: 0, .. } if module == "loyalty"
        ));
    });
}

#[test]
fn checking_a_file_leaves_it_untouched() {
    block_on(async {
        let f = fixture().await;
        let file = f.path("backup.db");
        older_database(&file, "OLD").await;
        let before = std::fs::read(&file).unwrap();

        f.backups.stage_restore(&file).await.unwrap();

        assert_eq!(std::fs::read(&file).unwrap(), before);
    });
}

#[test]
fn refuses_a_damaged_database() {
    block_on(async {
        let f = fixture().await;
        let file = f.path("damaged.db");
        older_database(&file, "OLD").await;
        let mut bytes = std::fs::read(&file).unwrap();
        // Garbage over every page but the header page.
        for byte in bytes.iter_mut().skip(4096) {
            *byte = 0xA5;
        }
        std::fs::write(&file, bytes).unwrap();

        let error = assert_refused(&f, &file).await;

        assert!(matches!(error, RestoreError::Damaged { .. }), "{error:?}");
    });
}

#[test]
fn restoring_the_oldest_backup_survives_the_safety_backup_pruning_it() {
    block_on(async {
        let f = fixture().await;
        f.backups
            .save_settings(&BackupSettings {
                folder: f.default_folder(),
                keep: 2,
            })
            .await
            .unwrap();
        f.add_product("FIRST").await;
        let oldest = f.backups.back_up_now().await.unwrap();
        f.clock.advance(TimeDelta::hours(1));
        f.add_product("SECOND").await;
        f.backups.back_up_now().await.unwrap();
        f.clock.advance(TimeDelta::hours(1));

        f.backups.stage_restore(&oldest.path).await.unwrap();

        let path = f.database_path();
        drop(f.backups);
        drop(f.database);
        assert!(apply_staged_restore(&path).unwrap());
        assert_eq!(skus_in(&path).await, ["FIRST"]);
    });
}
