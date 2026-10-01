use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::ManualClock;
use mascate_platform::{
    AppliedMigration, Database, Migration, MigrationError, ModuleMigrations, migrate,
};

fn clock() -> ManualClock {
    ManualClock::at(Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap())
}

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
const BROKEN_V3: Migration = Migration {
    version: 3,
    name: "half applied",
    sql: "CREATE TABLE catalog_half (id TEXT); THIS IS NOT SQL;",
};

fn catalog(migrations: &'static [Migration]) -> ModuleMigrations {
    ModuleMigrations {
        module: "catalog",
        migrations,
    }
}

async fn versions(database: &Database) -> Vec<(String, u32, String, String)> {
    let mut rows = database
        .connection()
        .query(
            "SELECT module, version, name, applied_at FROM schema_migrations ORDER BY module, version",
            (),
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        out.push((
            row.get(0).unwrap(),
            row.get(1).unwrap(),
            row.get(2).unwrap(),
            row.get(3).unwrap(),
        ));
    }
    out
}

async fn table_exists(database: &Database, table: &str) -> bool {
    let mut rows = database
        .connection()
        .query(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
        )
        .await
        .unwrap();
    rows.next().await.unwrap().is_some()
}

#[test]
fn first_run_creates_the_file_its_folder_and_the_version_table() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("mascate.db");

        let database = Database::open(&path).await.unwrap();
        let applied = migrate(&database, &clock(), &[]).await.unwrap();

        assert!(path.exists());
        assert!(applied.is_empty());
        assert!(table_exists(&database, "schema_migrations").await);
    });
}

#[test]
fn applies_pending_migrations_in_order_and_records_them() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("m.db")).await.unwrap();

        let applied = migrate(&database, &clock(), &[catalog(&[CATALOG_V1, CATALOG_V2])])
            .await
            .unwrap();

        assert_eq!(
            applied,
            vec![
                AppliedMigration {
                    module: "catalog",
                    version: 1,
                    name: "create products"
                },
                AppliedMigration {
                    module: "catalog",
                    version: 2,
                    name: "add product title"
                },
            ]
        );
        let recorded = versions(&database).await;
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].3, "2026-10-01T12:00:00+00:00");
    });
}

#[test]
fn running_again_applies_nothing() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("m.db")).await.unwrap();
        let modules = [catalog(&[CATALOG_V1, CATALOG_V2])];

        migrate(&database, &clock(), &modules).await.unwrap();
        let second = migrate(&database, &clock(), &modules).await.unwrap();

        assert!(second.is_empty());
        assert_eq!(versions(&database).await.len(), 2);
    });
}

#[test]
fn a_new_release_applies_only_what_is_new_and_keeps_data() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.db");
        {
            let database = Database::open(&path).await.unwrap();
            migrate(&database, &clock(), &[catalog(&[CATALOG_V1])])
                .await
                .unwrap();
            database
                .connection()
                .execute(
                    "INSERT INTO catalog_products (id, sku) VALUES ('p1', 'SKU-1')",
                    (),
                )
                .await
                .unwrap();
        }

        let database = Database::open(&path).await.unwrap();
        let applied = migrate(&database, &clock(), &[catalog(&[CATALOG_V1, CATALOG_V2])])
            .await
            .unwrap();

        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].version, 2);
        let mut rows = database
            .connection()
            .query("SELECT sku, title FROM catalog_products", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "SKU-1");
        assert_eq!(row.get::<Option<String>>(1).unwrap(), None);
    });
}

#[test]
fn a_failing_migration_rolls_back_and_keeps_the_last_good_version() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("m.db")).await.unwrap();

        let error = migrate(
            &database,
            &clock(),
            &[catalog(&[CATALOG_V1, CATALOG_V2, BROKEN_V3])],
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            MigrationError::Failed {
                module: "catalog",
                version: 3,
                ..
            }
        ));
        assert_eq!(versions(&database).await.len(), 2);
        assert!(!table_exists(&database, "catalog_half").await);
    });
}

#[test]
fn versions_are_counted_per_module() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("m.db")).await.unwrap();
        const INVENTORY_V1: Migration = Migration {
            version: 1,
            name: "create locations",
            sql: "CREATE TABLE inventory_locations (id TEXT PRIMARY KEY);",
        };

        let applied = migrate(
            &database,
            &clock(),
            &[
                catalog(&[CATALOG_V1]),
                ModuleMigrations {
                    module: "inventory",
                    migrations: &[INVENTORY_V1],
                },
            ],
        )
        .await
        .unwrap();

        assert_eq!(applied.len(), 2);
        assert!(table_exists(&database, "inventory_locations").await);
    });
}

#[test]
fn refuses_a_database_written_by_a_newer_app() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("m.db")).await.unwrap();
        migrate(&database, &clock(), &[catalog(&[CATALOG_V1, CATALOG_V2])])
            .await
            .unwrap();

        let error = migrate(&database, &clock(), &[catalog(&[CATALOG_V1])])
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            MigrationError::DatabaseNewerThanApp {
                module: "catalog",
                found: 2,
                known: 1
            }
        ));
    });
}

#[test]
fn rejects_gaps_or_disorder_before_touching_the_schema() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("m.db")).await.unwrap();

        let error = migrate(&database, &clock(), &[catalog(&[CATALOG_V2, CATALOG_V1])])
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            MigrationError::BadOrder { module: "catalog" }
        ));
        assert!(!table_exists(&database, "catalog_products").await);
    });
}

#[test]
fn foreign_keys_are_enforced() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&dir.path().join("m.db")).await.unwrap();
        let connection = database.connection();
        connection
            .execute_batch(
                "CREATE TABLE parent (id TEXT PRIMARY KEY);
                 CREATE TABLE child (id TEXT PRIMARY KEY, parent_id TEXT REFERENCES parent(id));",
            )
            .await
            .unwrap();

        let orphan = connection
            .execute(
                "INSERT INTO child (id, parent_id) VALUES ('c', 'missing')",
                (),
            )
            .await;

        assert!(orphan.is_err());
    });
}
