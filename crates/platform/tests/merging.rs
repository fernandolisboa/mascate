//! "Comparar com outro banco" through the platform's public interface,
//! with two real temporary databases standing for two machines: records
//! are recognised by id, by unique key and by the ledger's paired key, the
//! newer change wins the whole row, what cannot come in stays out with its
//! reason, files go through the Restore's check, and nothing is written.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::Timestamp;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::{
    Condition, ConflictSide, Database, KeyPart, LeftOut, LeftOutReason, MERGE, MIGRATIONS,
    MergeConflict, MergeError, MergeGap, MergeImport, MergeReport, Migration, ModuleMerge,
    ModuleMigrations, RecognisedBy, RecordsReport, Reference, RestoreError, Side, TableMerge,
    UniqueKey, merge_gaps, migrate, stored,
};

const CATALOG_V1: Migration = Migration {
    version: 1,
    name: "create products",
    risky: false,
    sql: "CREATE TABLE catalog_products (
        id TEXT PRIMARY KEY, sku TEXT NOT NULL, name TEXT NOT NULL,
        created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT
    );
    CREATE UNIQUE INDEX catalog_products_by_sku
        ON catalog_products (sku) WHERE deleted_at IS NULL;",
};
const CATALOG_V2: Migration = Migration {
    version: 2,
    name: "add product brand",
    risky: false,
    sql: "ALTER TABLE catalog_products ADD COLUMN brand TEXT;",
};
const SALES_V1: Migration = Migration {
    version: 1,
    name: "create orders",
    risky: false,
    sql: "CREATE TABLE sales_orders (
        id TEXT PRIMARY KEY, ml_order_id TEXT NOT NULL, status TEXT NOT NULL,
        created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT
    );
    CREATE UNIQUE INDEX sales_orders_by_ml_id
        ON sales_orders (ml_order_id) WHERE deleted_at IS NULL;
    CREATE TABLE sales_order_lines (
        id TEXT PRIMARY KEY, order_id TEXT NOT NULL, ml_item_id TEXT NOT NULL,
        ml_variation_id TEXT, product_id TEXT NOT NULL, quantity INTEGER NOT NULL,
        created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT
    );
    CREATE UNIQUE INDEX sales_order_lines_by_item
        ON sales_order_lines (order_id, ml_item_id, COALESCE(ml_variation_id, ''))
        WHERE deleted_at IS NULL;
    CREATE TABLE sales_settings (
        id TEXT PRIMARY KEY, rate TEXT NOT NULL,
        created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT
    );",
};
const STOCK_V1: Migration = Migration {
    version: 1,
    name: "create the ledger",
    risky: false,
    sql: "CREATE TABLE stock_movements (
        id TEXT PRIMARY KEY, product_id TEXT NOT NULL, quantity INTEGER NOT NULL,
        reason TEXT NOT NULL, reference_id TEXT NOT NULL,
        created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT
    );
    CREATE TABLE stock_notes (
        id TEXT PRIMARY KEY, text TEXT NOT NULL,
        created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT
    );
    CREATE TABLE stock_folders (
        id TEXT PRIMARY KEY, folder TEXT NOT NULL,
        created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT
    );",
};

const SALES: ModuleMigrations = ModuleMigrations {
    module: "sales",
    migrations: &[SALES_V1],
};
const STOCK: ModuleMigrations = ModuleMigrations {
    module: "stock",
    migrations: &[STOCK_V1],
};
/// The app as released before: products without a brand.
const OLDER_APP: &[ModuleMigrations] = &[
    MIGRATIONS,
    ModuleMigrations {
        module: "catalog",
        migrations: &[CATALOG_V1],
    },
    SALES,
    STOCK,
];
const THIS_APP: &[ModuleMigrations] = &[
    MIGRATIONS,
    ModuleMigrations {
        module: "catalog",
        migrations: &[CATALOG_V1, CATALOG_V2],
    },
    SALES,
    STOCK,
];

const SALE: Condition = Condition {
    column: "reason",
    values: &["sale"],
};
const ADJUSTMENT: Condition = Condition {
    column: "reason",
    values: &["adjustment"],
};
const PRODUCTS: TableMerge = TableMerge::new(
    "catalog_products",
    "Produto",
    RecognisedBy::Keys(&[UniqueKey::live(&[KeyPart::Column("sku")])]),
)
.label(&["sku", "name"]);
const ORDER_LINES: TableMerge = TableMerge::new(
    "sales_order_lines",
    "Linha de pedido",
    RecognisedBy::Keys(&[UniqueKey::live(&[
        KeyPart::Column("order_id"),
        KeyPart::Column("ml_item_id"),
        KeyPart::NullAsEmpty("ml_variation_id"),
    ])]),
)
.references(&[
    Reference::to("order_id", "sales_orders"),
    Reference::to("product_id", "catalog_products"),
])
.platform_ids(&["ml_item_id", "ml_variation_id"])
.label(&["order_id", "product_id"]);
const NOTES: TableMerge =
    TableMerge::new("stock_notes", "Anotação", RecognisedBy::Id).label(&["text"]);

/// Each table points to tables declared after it, so the order is the
/// references', not the declarations'.
const MERGES: &[ModuleMerge] = &[
    MERGE,
    ModuleMerge {
        module: "catalog",
        tables: &[PRODUCTS],
    },
    ModuleMerge {
        module: "stock",
        tables: &[
            TableMerge::new(
                "stock_movements",
                "Movimento de estoque",
                RecognisedBy::Pairs {
                    when: Some(SALE),
                    key: &["reason", "reference_id", "product_id", "quantity"],
                },
            )
            .references(&[
                Reference::to("product_id", "catalog_products"),
                Reference::to("reference_id", "sales_orders").when(SALE),
                Reference::to("reference_id", "stock_movements").when(ADJUSTMENT),
            ])
            .label(&["product_id", "quantity"])
            .immutable(),
            NOTES,
            TableMerge::new("stock_folders", "Pasta", RecognisedBy::ThisMachine).label(&["folder"]),
        ],
    },
    ModuleMerge {
        module: "sales",
        tables: &[
            ORDER_LINES,
            TableMerge::new(
                "sales_orders",
                "Pedido",
                RecognisedBy::Keys(&[UniqueKey::live(&[KeyPart::Column("ml_order_id")])]),
            )
            .platform_ids(&["ml_order_id"])
            .label(&["ml_order_id"]),
            TableMerge::new("sales_settings", "Opções de venda", RecognisedBy::SingleRow),
        ],
    },
];

fn at(minutes: i64) -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap() + TimeDelta::minutes(minutes)
}

struct Fixture {
    dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    here: Arc<Database>,
    there: Option<Database>,
    merges: MergeImport,
}

impl Fixture {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn there(&self) -> &Database {
        self.there.as_ref().expect("the other database is open")
    }

    /// The other machine's database exported to a file, as the owner
    /// carries it over.
    async fn other_file(&mut self) -> PathBuf {
        let file = self.path("export").join("mascate-2026-10-15.db");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let there = self.there.take().expect("the other database is open");
        there
            .connection()
            .execute("VACUUM INTO ?1", [file.to_str().unwrap()])
            .await
            .unwrap();
        file
    }

    async fn compare(&self, file: &Path) -> Result<MergeReport, MergeError> {
        self.merges.compare(file).await
    }
}

async fn fixture_with(there_app: &'static [ModuleMigrations]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(at(0)));
    let here = Arc::new(
        Database::open(&dir.path().join("data").join("mascate.db"))
            .await
            .unwrap(),
    );
    migrate(&here, clock.as_ref(), THIS_APP).await.unwrap();
    let there = Database::open(&dir.path().join("other").join("mascate.db"))
        .await
        .unwrap();
    migrate(&there, clock.as_ref(), there_app).await.unwrap();
    let merges = MergeImport::new(
        here.clone(),
        THIS_APP,
        MERGES,
        clock.clone(),
        Arc::new(SequentialIds::default()),
    );
    Fixture {
        dir,
        clock,
        here,
        there: Some(there),
        merges,
    }
}

async fn fixture() -> Fixture {
    fixture_with(THIS_APP).await
}

async fn insert(database: &Database, table: &str, columns: &[&str], values: &[&str]) {
    let placeholders: Vec<String> = (1..=values.len()).map(|at| format!("?{at}")).collect();
    database
        .connection()
        .execute(
            &format!(
                "INSERT INTO {table} ({}) VALUES ({})",
                columns.join(", "),
                placeholders.join(", ")
            ),
            values
                .iter()
                .map(|value| libsql::Value::Text((*value).to_owned()))
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
}

/// A product saved at `saved`, created at minute 0.
async fn product(database: &Database, id: &str, sku: &str, name: &str, saved: i64) {
    insert(
        database,
        "catalog_products",
        &["id", "sku", "name", "created_at", "updated_at"],
        &[id, sku, name, &stored(at(0)), &stored(at(saved))],
    )
    .await;
}

async fn delete_product(database: &Database, id: &str, at_minute: i64) {
    database
        .connection()
        .execute(
            "UPDATE catalog_products SET deleted_at = ?2, updated_at = ?2 WHERE id = ?1",
            [id.to_owned(), stored(at(at_minute))],
        )
        .await
        .unwrap();
}

async fn order(database: &Database, id: &str, ml_id: &str, status: &str, saved: i64) {
    insert(
        database,
        "sales_orders",
        &["id", "ml_order_id", "status", "created_at", "updated_at"],
        &[id, ml_id, status, &stored(at(saved)), &stored(at(saved))],
    )
    .await;
}

async fn line(database: &Database, id: &str, order: &str, product: &str, saved: i64) {
    insert(
        database,
        "sales_order_lines",
        &[
            "id",
            "order_id",
            "ml_item_id",
            "product_id",
            "quantity",
            "created_at",
            "updated_at",
        ],
        &[
            id,
            order,
            "MLB1",
            product,
            "2",
            &stored(at(saved)),
            &stored(at(saved)),
        ],
    )
    .await;
}

async fn movement(
    database: &Database,
    id: &str,
    product: &str,
    reason: &str,
    reference: &str,
    saved: i64,
) {
    insert(
        database,
        "stock_movements",
        &[
            "id",
            "product_id",
            "quantity",
            "reason",
            "reference_id",
            "created_at",
            "updated_at",
        ],
        &[
            id,
            product,
            "-2",
            reason,
            reference,
            &stored(at(saved)),
            &stored(at(saved)),
        ],
    )
    .await;
}

async fn row(database: &Database, table: &str, id: &str, column: &str, value: &str, saved: i64) {
    insert(
        database,
        table,
        &["id", column, "created_at", "updated_at"],
        &[id, value, &stored(at(saved)), &stored(at(saved))],
    )
    .await;
}

/// Every row of every table, as text, to tell whether anything changed.
async fn dump(database: &Database) -> Vec<String> {
    let connection = database.connection();
    let mut tables = Vec::new();
    let mut rows = connection
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
            (),
        )
        .await
        .unwrap();
    while let Some(row) = rows.next().await.unwrap() {
        tables.push(row.get::<String>(0).unwrap());
    }
    let mut dumped = Vec::new();
    for table in tables {
        let mut rows = connection
            .query(&format!("SELECT * FROM {table}"), ())
            .await
            .unwrap();
        while let Some(row) = rows.next().await.unwrap() {
            let cells: Vec<String> = (0..row.column_count())
                .map(|at| format!("{:?}", row.get_value(at).unwrap()))
                .collect();
            dumped.push(format!("{table}: {}", cells.join(", ")));
        }
    }
    dumped.sort();
    dumped
}

fn records<'a>(report: &'a MergeReport, noun: &str) -> &'a RecordsReport {
    report
        .records
        .iter()
        .find(|records| records.noun == noun)
        .unwrap_or_else(|| panic!("no {noun} in {report:#?}"))
}

fn conflict(label: &str, here: (i64, bool), there: (i64, bool), winner: Side) -> MergeConflict {
    MergeConflict {
        label: label.into(),
        here: ConflictSide {
            changed_at: Some(at(here.0)),
            deleted: here.1,
        },
        there: ConflictSide {
            changed_at: Some(at(there.0)),
            deleted: there.1,
        },
        winner,
    }
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
fn crossed_changes_bring_new_records_and_the_newer_side_of_each_conflict() {
    block_on(async {
        let mut f = fixture().await;
        let (here, there) = (f.here.as_ref(), f.there());
        // Products both machines had from one export, then each changed.
        product(here, "p1", "FONE", "Fone BT", 2).await;
        product(there, "p1", "FONE", "Fone Bluetooth", 3).await;
        product(here, "p2", "CAPA", "Capa preta", 5).await;
        product(there, "p2", "CAPA", "Capa azul", 4).await;
        product(here, "p3", "CABO", "Cabo", 0).await;
        delete_product(here, "p3", 6).await;
        product(there, "p3", "CABO", "Cabo USB-C", 5).await;
        product(here, "p4", "PILHA", "Pilha AA", 5).await;
        product(there, "p4", "PILHA", "Pilha", 0).await;
        delete_product(there, "p4", 6).await;
        product(here, "p5", "MOUSE", "Mouse", 1).await;
        product(there, "p5", "MOUSE", "Mouse", 1).await;
        product(here, "p6", "TECLADO", "Teclado ABNT", 7).await;
        product(there, "p6", "TECLADO", "Teclado US", 7).await;
        // Each machine's new records.
        product(here, "p7", "SUPORTE", "Suporte", 8).await;
        product(there, "p8", "PELICULA", "Película", 8).await;
        row(here, "stock_notes", "n1", "text", "Conferir caixa", 8).await;
        row(there, "stock_notes", "n2", "text", "Pedir mais capas", 8).await;
        // Each machine saved its own settings and its own folder.
        row(here, "sales_settings", "s1", "rate", "6", 1).await;
        row(there, "sales_settings", "s2", "rate", "8", 2).await;
        row(here, "stock_folders", "f1", "folder", "C:\\Mascate", 1).await;
        row(there, "stock_folders", "f2", "folder", "/home/fernando", 2).await;
        let file = f.other_file().await;
        let before_here = dump(&f.here).await;
        let before_there = std::fs::read(&file).unwrap();

        let report = f.compare(&file).await.unwrap();

        assert_eq!(
            *records(&report, "Produto"),
            RecordsReport {
                module: "catalog",
                noun: "Produto",
                new: 1,
                changed_here: 2,
                conflicts: vec![
                    conflict("CABO · Cabo USB-C", (6, true), (5, false), Side::Here),
                    conflict("CAPA · Capa azul", (5, false), (4, false), Side::Here),
                    conflict("FONE · Fone Bluetooth", (2, false), (3, false), Side::There),
                    conflict("PILHA · Pilha", (5, false), (6, true), Side::There),
                    // An exact tie keeps this machine's.
                    conflict("TECLADO · Teclado US", (7, false), (7, false), Side::Here),
                ],
                left_out: vec![],
            }
        );
        let notes = records(&report, "Anotação");
        assert_eq!((notes.new, notes.changed_here), (1, 0));
        let settings = records(&report, "Opções de venda");
        assert_eq!(settings.changed_here, 1);
        assert_eq!(
            settings.conflicts,
            [conflict("", (1, false), (2, false), Side::There)]
        );
        assert!(report.records.iter().all(|records| records.noun != "Pasta"));
        assert_eq!(
            report.records.iter().map(|r| r.module).collect::<Vec<_>>(),
            ["catalog", "stock", "sales"]
        );
        // Neither database was written.
        assert_eq!(dump(&f.here).await, before_here);
        assert_eq!(std::fs::read(&file).unwrap(), before_there);
    });
}

#[test]
fn the_same_order_synced_on_both_machines_is_one_order_with_one_line_and_one_sale() {
    block_on(async {
        let mut f = fixture().await;
        let (here, there) = (f.here.as_ref(), f.there());
        product(here, "p1", "FONE", "Fone", 0).await;
        product(there, "p1", "FONE", "Fone", 0).await;
        order(here, "o-here", "2000001", "paid", 10).await;
        line(here, "l-here", "o-here", "p1", 10).await;
        movement(here, "m-here", "p1", "sale", "o-here", 10).await;
        order(there, "o-there", "2000001", "shipped", 20).await;
        line(there, "l-there", "o-there", "p1", 20).await;
        movement(there, "m-there", "p1", "sale", "o-there", 20).await;
        // A line only the other machine read, of the order it won.
        insert(
            there,
            "sales_order_lines",
            &[
                "id",
                "order_id",
                "ml_item_id",
                "ml_variation_id",
                "product_id",
                "quantity",
                "created_at",
                "updated_at",
            ],
            &[
                "l-there-2",
                "o-there",
                "MLB1",
                "71",
                "p1",
                "1",
                &stored(at(20)),
                &stored(at(20)),
            ],
        )
        .await;
        // A count made only on the other machine.
        movement(there, "m-count", "p1", "adjustment", "m-count", 21).await;
        let file = f.other_file().await;

        let report = f.compare(&file).await.unwrap();

        assert_eq!(
            *records(&report, "Pedido"),
            RecordsReport {
                module: "sales",
                noun: "Pedido",
                new: 0,
                changed_here: 1,
                conflicts: vec![conflict("2000001", (10, false), (20, false), Side::There)],
                left_out: vec![],
            }
        );
        // The line matches through the order's id here, the sale through
        // the ledger's key: only the variation's line and the count are new.
        let lines = records(&report, "Linha de pedido");
        assert_eq!((lines.new, lines.changed_here), (1, 0));
        assert!(lines.conflicts.is_empty() && lines.left_out.is_empty());
        let movements = records(&report, "Movimento de estoque");
        assert_eq!((movements.new, movements.changed_here), (1, 0));
        assert!(movements.conflicts.is_empty() && movements.left_out.is_empty());
    });
}

#[test]
fn equal_sales_of_one_order_pair_one_to_one_in_recorded_order() {
    block_on(async {
        let mut f = fixture().await;
        let (here, there) = (f.here.as_ref(), f.there());
        product(here, "p1", "FONE", "Fone", 0).await;
        product(there, "p1", "FONE", "Fone", 0).await;
        order(here, "o-here", "2000001", "paid", 10).await;
        movement(here, "m-here", "p1", "sale", "o-here", 10).await;
        order(there, "o-there", "2000001", "paid", 10).await;
        movement(there, "m-there-1", "p1", "sale", "o-there", 11).await;
        movement(there, "m-there-2", "p1", "sale", "o-there", 12).await;
        let file = f.other_file().await;

        let report = f.compare(&file).await.unwrap();

        assert_eq!(records(&report, "Movimento de estoque").new, 1);
        assert!(report.records.iter().all(|r| r.noun != "Pedido"));
    });
}

#[test]
fn a_taken_sku_and_a_line_of_a_missing_order_stay_out_with_their_reason() {
    block_on(async {
        let mut f = fixture().await;
        let (here, there) = (f.here.as_ref(), f.there());
        product(here, "p1", "FONE", "Fone", 0).await;
        // The other machine moved the fone to a SKU this one gave a new product.
        product(there, "p1", "CABO", "Fone", 5).await;
        product(here, "p7", "CABO", "Cabo", 3).await;
        line(there, "l-ghost", "o-ghost", "p1", 6).await;
        // A new order whose line comes in with it.
        order(there, "o-new", "2000009", "paid", 7).await;
        line(there, "l-new", "o-new", "p1", 7).await;
        let file = f.other_file().await;

        let report = f.compare(&file).await.unwrap();

        let products = records(&report, "Produto");
        assert_eq!(products.changed_here, 0);
        // The other side's change is newer, but staying out leaves this one.
        assert_eq!(
            products
                .conflicts
                .iter()
                .map(|c| c.winner)
                .collect::<Vec<_>>(),
            [Side::Here]
        );
        assert_eq!(
            products.left_out,
            [LeftOut {
                label: "CABO · Fone".into(),
                reason: LeftOutReason::KeyTaken {
                    by: "CABO · Cabo".into()
                },
            }]
        );
        let lines = records(&report, "Linha de pedido");
        assert_eq!(lines.new, 1);
        assert_eq!(
            lines.left_out,
            [LeftOut {
                label: "FONE · Fone".into(),
                reason: LeftOutReason::MissingRecord { noun: "Pedido" },
            }]
        );
    });
}

#[test]
fn an_older_file_is_compared_through_a_migrated_copy_and_left_byte_for_byte() {
    block_on(async {
        let mut f = fixture_with(OLDER_APP).await;
        product(&f.here, "p1", "FONE", "Fone", 0).await;
        insert(
            f.there(),
            "catalog_products",
            &["id", "sku", "name", "created_at", "updated_at"],
            &["p1", "FONE", "Fone", &stored(at(0)), &stored(at(0))],
        )
        .await;
        insert(
            f.there(),
            "catalog_products",
            &["id", "sku", "name", "created_at", "updated_at"],
            &["p2", "CAPA", "Capa", &stored(at(1)), &stored(at(1))],
        )
        .await;
        let file = f.other_file().await;
        let before = std::fs::read(&file).unwrap();
        let export_before = file_names(&f.path("export"));
        let folder_before = file_names(&f.path("data"));

        let report = f.compare(&file).await.unwrap();

        let products = records(&report, "Produto");
        assert_eq!((products.new, products.conflicts.len()), (1, 0));
        assert_eq!(std::fs::read(&file).unwrap(), before);
        assert_eq!(file_names(&f.path("export")), export_before);
        assert_eq!(file_names(&f.path("data")), folder_before);
    });
}

#[test]
fn the_other_machines_own_database_file_is_read_through_a_copy_and_its_folder_left_alone() {
    block_on(async {
        let mut f = fixture().await;
        product(f.there(), "p8", "PELICULA", "Película", 8).await;
        f.there = None;
        let file = f.path("other").join("mascate.db");
        let before = std::fs::read(&file).unwrap();
        let folder_before = file_names(&f.path("other"));

        let report = f.compare(&file).await.unwrap();

        assert_eq!(records(&report, "Produto").new, 1);
        assert_eq!(std::fs::read(&file).unwrap(), before);
        assert_eq!(file_names(&f.path("other")), folder_before);
    });
}

#[test]
fn the_same_file_twice_gives_the_same_report() {
    block_on(async {
        let mut f = fixture().await;
        product(f.there(), "p8", "PELICULA", "Película", 8).await;
        let file = f.other_file().await;

        let first = f.compare(&file).await.unwrap();
        let second = f.compare(&file).await.unwrap();

        assert_eq!(first, second);
        assert_eq!(records(&first, "Produto").new, 1);
    });
}

#[test]
fn a_database_with_nothing_new_or_different_reports_nothing() {
    block_on(async {
        let mut f = fixture().await;
        product(&f.here, "p1", "FONE", "Fone", 0).await;
        product(f.there(), "p1", "FONE", "Fone", 0).await;
        let file = f.other_file().await;

        assert!(f.compare(&file).await.unwrap().is_empty());
    });
}

#[test]
fn files_the_restore_refuses_are_refused_with_its_reasons() {
    block_on(async {
        let mut f = fixture().await;
        let photo = f.path("photo.jpg");
        std::fs::write(&photo, vec![0xFF_u8; 8192]).unwrap();
        let newer = f.path("newer.db");
        let newer_database = Database::open(&newer).await.unwrap();
        migrate(
            &newer_database,
            f.clock.as_ref(),
            &[
                MIGRATIONS,
                ModuleMigrations {
                    module: "catalog",
                    migrations: &[
                        CATALOG_V1,
                        CATALOG_V2,
                        Migration {
                            version: 3,
                            name: "add product color",
                            risky: false,
                            sql: "ALTER TABLE catalog_products ADD COLUMN color TEXT;",
                        },
                    ],
                },
            ],
        )
        .await
        .unwrap();
        drop(newer_database);
        let damaged = f.other_file().await;
        let mut bytes = std::fs::read(&damaged).unwrap();
        for byte in bytes.iter_mut().skip(4096) {
            *byte = 0xA5;
        }
        std::fs::write(&damaged, bytes).unwrap();

        assert!(matches!(
            f.compare(&photo).await,
            Err(MergeError::Refused(RestoreError::NotMascate(path))) if path == photo
        ));
        assert!(matches!(
            f.compare(&newer).await,
            Err(MergeError::Refused(RestoreError::NewerThanApp {
                found: 3,
                known: 2,
                ..
            }))
        ));
        assert!(matches!(
            f.compare(&damaged).await,
            Err(MergeError::Refused(RestoreError::Damaged { .. }))
        ));
        assert!(matches!(
            f.compare(&f.path("data").join("mascate.db")).await,
            Err(MergeError::OwnDatabase(_))
        ));
        assert!(
            file_names(&f.path("data"))
                .iter()
                .all(|name| !name.starts_with(".mascate-merge"))
        );
    });
}

#[test]
fn the_declarations_cover_the_schema() {
    block_on(async {
        let f = fixture().await;

        let gaps = merge_gaps(f.here.connection(), MERGES).await.unwrap();

        assert!(gaps.is_empty(), "{gaps:#?}");
    });
}

#[test]
fn an_undeclared_table_an_unclassified_id_or_a_key_without_its_index_is_a_gap() {
    block_on(async {
        let mut f = fixture().await;
        const INCOMPLETE: &[ModuleMerge] = &[
            MERGE,
            ModuleMerge {
                module: "catalog",
                tables: &[TableMerge::new(
                    "catalog_products",
                    "Produto",
                    RecognisedBy::Keys(&[UniqueKey::live(&[KeyPart::Column("name")])]),
                )],
            },
            ModuleMerge {
                module: "sales",
                tables: &[
                    ORDER_LINES.references(&[Reference::to("order_id", "sales_orders")]),
                    TableMerge::new("sales_orders", "Pedido", RecognisedBy::Id)
                        .platform_ids(&["ml_order_id"]),
                    TableMerge::new("sales_settings", "Opções de venda", RecognisedBy::SingleRow),
                ],
            },
            ModuleMerge {
                module: "stock",
                tables: &[NOTES],
            },
        ];
        let merges = MergeImport::new(
            f.here.clone(),
            THIS_APP,
            INCOMPLETE,
            f.clock.clone(),
            Arc::new(SequentialIds::default()),
        );

        let gaps = merge_gaps(f.here.connection(), INCOMPLETE).await.unwrap();

        for gap in [
            MergeGap::UndeclaredTable("stock_movements".into()),
            MergeGap::UndeclaredTable("stock_folders".into()),
            MergeGap::UnclassifiedId {
                table: "sales_order_lines",
                column: "product_id".into(),
            },
            MergeGap::UndeclaredKey {
                table: "catalog_products",
                index: "catalog_products_by_sku".into(),
            },
            MergeGap::KeyWithoutIndex {
                table: "catalog_products",
                key: "(name) WHERE deleted_at IS NULL".into(),
            },
            MergeGap::UndeclaredKey {
                table: "sales_orders",
                index: "sales_orders_by_ml_id".into(),
            },
        ] {
            assert!(gaps.contains(&gap), "{gap:?} not in {gaps:#?}");
        }
        assert_eq!(gaps.len(), 6, "{gaps:#?}");
        let file = f.other_file().await;
        assert!(matches!(
            merges.compare(&file).await,
            Err(MergeError::Undeclared(_))
        ));
    });
}
