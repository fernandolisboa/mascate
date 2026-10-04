//! Opening the database at start: a Backup before every migration of a
//! database with data, and that Backup put back when a migration fails.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::{
    BackupSettings, Backups, Database, MIGRATIONS, Migration, ModuleMigrations, OpenError, Opened,
    apply_staged_restore, migrate, open_and_migrate, stage_restore_without_backup,
};

const CATALOG_V1: Migration = Migration {
    version: 1,
    name: "create products",
    risky: false,
    sql: "CREATE TABLE catalog_products (id TEXT PRIMARY KEY, sku TEXT NOT NULL);",
};
const CATALOG_V2: Migration = Migration {
    version: 2,
    name: "add product title",
    risky: false,
    sql: "ALTER TABLE catalog_products ADD COLUMN title TEXT;
          UPDATE catalog_products SET title = sku;",
};
/// Fails after the two before it ran: the database is half way.
const CATALOG_V3_BROKEN: Migration = Migration {
    version: 3,
    name: "copy titles into a table that does not exist",
    risky: true,
    sql: "INSERT INTO catalog_missing SELECT * FROM catalog_products;",
};

/// The app as released before: catalog at version 1.
const OLDER_APP: &[ModuleMigrations] = &[
    MIGRATIONS,
    ModuleMigrations {
        module: "catalog",
        migrations: &[CATALOG_V1],
    },
];
/// This app: catalog at version 2.
const THIS_APP: &[ModuleMigrations] = &[
    MIGRATIONS,
    ModuleMigrations {
        module: "catalog",
        migrations: &[CATALOG_V1, CATALOG_V2],
    },
];
/// A version whose last migration fails.
const BROKEN_APP: &[ModuleMigrations] = &[
    MIGRATIONS,
    ModuleMigrations {
        module: "catalog",
        migrations: &[CATALOG_V1, CATALOG_V2, CATALOG_V3_BROKEN],
    },
];

struct Fixture {
    dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            clock: Arc::new(ManualClock::at(
                Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
            )),
        }
    }

    fn database_path(&self) -> PathBuf {
        self.dir.path().join("data").join("mascate.db")
    }

    fn default_folder(&self) -> PathBuf {
        self.dir.path().join("backups")
    }

    async fn open(&self, modules: &'static [ModuleMigrations]) -> Result<Opened, OpenError> {
        open_and_migrate(
            &self.database_path(),
            modules,
            self.default_folder(),
            self.clock.clone(),
            Arc::new(SequentialIds::default()),
        )
        .await
    }

    /// A database the older app made and filled.
    async fn older_database(&self) {
        let database = Database::open(&self.database_path()).await.unwrap();
        migrate(&database, self.clock.as_ref(), OLDER_APP)
            .await
            .unwrap();
        database
            .connection()
            .execute_batch(
                "INSERT INTO catalog_products (id, sku) VALUES ('1', 'CAFE-01');
                 INSERT INTO catalog_products (id, sku) VALUES ('2', 'MOKA-02');",
            )
            .await
            .unwrap();
    }

    fn backups_in(&self, folder: &Path) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(folder)
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        found.sort();
        found
    }
}

/// Every table's definition and rows, in a stable order: two databases with
/// the same dump hold the same data.
async fn dump(path: &Path) -> Vec<String> {
    let database = Database::open(path).await.unwrap();
    let connection = database.connection();
    let mut lines = Vec::new();
    let mut tables = connection
        .query(
            "SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name",
            (),
        )
        .await
        .unwrap();
    let mut names = Vec::new();
    while let Some(row) = tables.next().await.unwrap() {
        let name: String = row.get(0).unwrap();
        lines.push(row.get::<String>(1).unwrap());
        if !name.starts_with("sqlite_") && !name.contains("_by_") {
            names.push(name);
        }
    }
    for name in names {
        let mut rows = connection
            .query(&format!("SELECT * FROM {name} ORDER BY 1"), ())
            .await
            .unwrap();
        while let Some(row) = rows.next().await.unwrap() {
            let values: Vec<String> = (0..row.column_count())
                .map(|at| format!("{:?}", row.get_value(at).unwrap()))
                .collect();
            lines.push(format!("{name}: {}", values.join(", ")));
        }
    }
    lines
}

#[test]
fn a_new_database_is_created_without_a_backup() {
    block_on(async {
        let fixture = Fixture::new();
        let opened = fixture.open(THIS_APP).await.unwrap();
        assert_eq!(opened.backup_before_migrating, None);
        assert!(fixture.backups_in(&fixture.default_folder()).is_empty());
    });
}

#[test]
fn an_up_to_date_database_opens_without_a_backup() {
    block_on(async {
        let fixture = Fixture::new();
        drop(fixture.open(THIS_APP).await.unwrap());
        let opened = fixture.open(THIS_APP).await.unwrap();
        assert_eq!(opened.backup_before_migrating, None);
    });
}

#[test]
fn a_database_from_an_older_version_is_backed_up_before_migrating() {
    block_on(async {
        let fixture = Fixture::new();
        fixture.older_database().await;
        let before = dump(&fixture.database_path()).await;

        let opened = fixture.open(THIS_APP).await.unwrap();

        let backup = opened.backup_before_migrating.unwrap();
        assert_eq!(
            fixture.backups_in(&fixture.default_folder()),
            [backup.path.clone()]
        );
        assert_eq!(dump(&backup.path).await, before);
        let mut rows = opened
            .database
            .connection()
            .query("SELECT title FROM catalog_products ORDER BY id", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next()
                .await
                .unwrap()
                .unwrap()
                .get::<String>(0)
                .unwrap(),
            "CAFE-01"
        );
    });
}

#[test]
fn the_backup_before_migrating_goes_to_the_folder_the_owner_chose() {
    block_on(async {
        let fixture = Fixture::new();
        fixture.older_database().await;
        let chosen = fixture.dir.path().join("pendrive");
        {
            let database = Arc::new(Database::open(&fixture.database_path()).await.unwrap());
            Backups::new(
                database,
                OLDER_APP,
                fixture.default_folder(),
                fixture.clock.clone(),
                Arc::new(SequentialIds::default()),
            )
            .save_settings(&BackupSettings {
                folder: chosen.clone(),
                keep: 3,
            })
            .await
            .unwrap();
        }

        let opened = fixture.open(THIS_APP).await.unwrap();

        assert_eq!(
            opened.backup_before_migrating.unwrap().path.parent(),
            Some(chosen.as_path())
        );
    });
}

#[test]
fn a_failed_migration_puts_the_database_back_exactly_as_it_was() {
    block_on(async {
        let fixture = Fixture::new();
        fixture.older_database().await;
        let before = dump(&fixture.database_path()).await;

        let Err(OpenError::PutBack { error, backup }) = fixture.open(BROKEN_APP).await else {
            panic!("the broken migration must fail and be put back");
        };

        assert!(error.to_string().contains("catalog 3"), "{error}");
        assert_eq!(dump(&fixture.database_path()).await, before);
        // The Backup stays for the owner, and the version before opens it.
        assert!(backup.path.is_file());
        let reopened = fixture.open(OLDER_APP).await.unwrap();
        assert_eq!(reopened.backup_before_migrating, None);
    });
}

#[test]
fn without_a_backup_the_database_is_not_migrated() {
    block_on(async {
        let fixture = Fixture::new();
        fixture.older_database().await;
        let before = dump(&fixture.database_path()).await;
        // A file where the Backup folder should be.
        std::fs::write(fixture.default_folder(), "not a folder").unwrap();

        let error = fixture.open(THIS_APP).await.err().unwrap();

        assert!(
            matches!(error, OpenError::NoBackupBeforeMigrating(_)),
            "{error}"
        );
        assert_eq!(dump(&fixture.database_path()).await, before);
    });
}

#[test]
fn a_database_that_does_not_open_can_still_be_restored() {
    block_on(async {
        let fixture = Fixture::new();
        fixture.older_database().await;
        let good = fixture.dir.path().join("good.db");
        std::fs::copy(fixture.database_path(), &good).unwrap();
        std::fs::write(fixture.database_path(), "garbage, not a database").unwrap();
        assert!(fixture.open(OLDER_APP).await.is_err());

        stage_restore_without_backup(&fixture.database_path(), OLDER_APP, &good)
            .await
            .unwrap();
        assert!(apply_staged_restore(&fixture.database_path()).unwrap());

        fixture.open(OLDER_APP).await.unwrap();
        assert_eq!(dump(&fixture.database_path()).await, dump(&good).await);
    });
}
