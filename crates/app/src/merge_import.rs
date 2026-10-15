//! Settings › Backup › "Comparar com outro banco" (#117): the report of
//! what a Merge Import would bring from another machine's database, with
//! nothing written. The rules live in the platform module (ADR 0032).

use chrono::Local;
use gpui_kit::component::{StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{App, Div, div};
use mascate_kernel::Timestamp;
use mascate_platform::{
    ConflictSide, LeftOut, LeftOutReason, MergeConflict, MergeReport, RecordsReport, Side,
};

use crate::appearance::look;
use crate::kit;

/// How many conflicts and records left out each kind lists before
/// summing up the rest.
const LISTED: usize = 20;

/// The module's name as the owner knows its screens.
fn module_name(module: &str) -> &str {
    match module {
        "platform" => "App",
        "catalog" => "Catálogo",
        "inventory" => "Estoque",
        "commerce" => "Vendas",
        "marketing" => "Marketing",
        "finance" => "Finanças",
        "affiliate" => "Afiliação",
        other => other,
    }
}

fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// "3 chegam · 1 muda aqui · 2 conflitos · 1 fica de fora", leaving out
/// what is zero.
pub(crate) fn counts(records: &RecordsReport) -> String {
    [
        (records.new, "chega", "chegam"),
        (records.changed_here, "muda aqui", "mudam aqui"),
        (records.conflicts.len(), "conflito", "conflitos"),
        (records.left_out.len(), "fica de fora", "ficam de fora"),
    ]
    .into_iter()
    .filter(|(count, ..)| *count > 0)
    .map(|(count, one, many)| counted(count, one, many))
    .collect::<Vec<_>>()
    .join(" · ")
}

fn when(at: Option<Timestamp>) -> String {
    at.map_or_else(
        || "sem data".to_owned(),
        |at| {
            at.with_timezone(&Local)
                .format("em %d/%m/%Y às %H:%M")
                .to_string()
        },
    )
}

fn changed(side: ConflictSide) -> String {
    let verb = if side.deleted { "excluiu" } else { "mudou" };
    format!("{verb} {}", when(side.changed_at))
}

fn named<'a>(label: &'a str, noun: &'a str) -> &'a str {
    if label.is_empty() { noun } else { label }
}

/// One Merge Conflict: when each side changed the record and which stays.
pub(crate) fn conflict_line(noun: &str, conflict: &MergeConflict) -> String {
    format!(
        "{}: este computador {}; o outro banco {}. Fica a versão {}.",
        named(&conflict.label, noun),
        changed(conflict.here),
        changed(conflict.there),
        match conflict.winner {
            Side::Here => "daqui",
            Side::There => "do outro banco",
        }
    )
}

/// One record that cannot come in, and why.
pub(crate) fn left_out_line(noun: &str, left_out: &LeftOut) -> String {
    let why = match &left_out.reason {
        LeftOutReason::KeyTaken { by } => {
            format!("{by} já usa o mesmo identificador aqui")
        }
        LeftOutReason::MissingRecord { noun } => {
            format!("aponta para um registro de {noun} que não existe em nenhum dos dois bancos")
        }
    };
    format!("{} fica de fora: {why}.", named(&left_out.label, noun))
}

/// The first lines of a list and, past [`LISTED`], how many more there are.
fn listed(lines: Vec<String>) -> Vec<String> {
    let more = lines.len().saturating_sub(LISTED);
    let mut shown: Vec<String> = lines.into_iter().take(LISTED).collect();
    if more > 0 {
        shown.push(format!("E mais {more}."));
    }
    shown
}

/// The report, grouped by module, each kind of record with its counts,
/// its conflicts and what stays out.
pub fn report_view(report: &MergeReport, cx: &App) -> Div {
    let t = look(cx).tokens;
    if report.is_empty() {
        return v_flex().child(kit::success_notice(
            "O outro banco não traz nada novo nem diferente: mesclar não mudaria nada.",
            cx,
        ));
    }
    let mut view = v_flex().gap_3();
    let mut module = "";
    for records in &report.records {
        if records.module != module {
            module = records.module;
            view = view.child(
                div()
                    .pt_2()
                    .text_sm()
                    .font_medium()
                    .text_color(t.text2)
                    .child(module_name(module).to_owned()),
            );
        }
        let lines = listed(
            records
                .conflicts
                .iter()
                .map(|conflict| conflict_line(records.noun, conflict))
                .collect(),
        );
        let left_out = listed(
            records
                .left_out
                .iter()
                .map(|left_out| left_out_line(records.noun, left_out))
                .collect(),
        );
        view = view.child(
            v_flex()
                .gap_1()
                .p_3()
                .rounded(t.radius_lg)
                .border(t.border_width)
                .border_color(t.border)
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(div().text_sm().font_medium().child(records.noun))
                        .child(div().text_sm().text_color(t.text2).child(counts(records))),
                )
                .children(
                    lines
                        .into_iter()
                        .map(|line| div().text_sm().text_color(t.text2).child(line)),
                )
                .children(
                    left_out
                        .into_iter()
                        .map(|line| div().text_sm().text_color(t.danger).child(line)),
                ),
        );
    }
    view
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    use chrono::{TimeDelta, TimeZone, Utc};
    use futures::executor::block_on;
    use mascate_catalog::Catalog;
    use mascate_commerce::{
        ChannelCategory, ChannelListing, ChannelOrder, ChannelOrderLine, ChannelOrders,
        ChannelStock, ListingStatus, Listings, NewPurchaseLine, NewPurchaseOrder, OrderStatus,
        Orders, PurchaseOrders, Receiving, SaleFee, SalesChannel,
    };
    use mascate_inventory::{HOME_LOCATION, Inventory};
    use mascate_kernel::testing::ManualClock;
    use mascate_kernel::{
        Clock, Currency, IdGenerator, ListingType, Money, PlatformError, RecordId,
    };
    use mascate_marketing::SalesEvents;
    use mascate_platform::{Database, MergeImport, merge_gaps, migrate};
    use rust_decimal::Decimal;

    use super::*;
    use crate::startup::{MODULE_MERGES, MODULE_MIGRATIONS};

    fn at(minutes: i64) -> Timestamp {
        Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap() + TimeDelta::minutes(minutes)
    }

    fn brl(amount: i64) -> Money {
        Money::new(Decimal::from(amount), Currency::Brl)
    }

    /// Ids from `start` on, so each machine hands out its own.
    struct IdsFrom(AtomicU64);

    impl IdGenerator for IdsFrom {
        fn next_id(&self) -> RecordId {
            RecordId::from_u128(u128::from(self.0.fetch_add(1, Ordering::Relaxed)))
        }
    }

    fn listing() -> ChannelListing {
        ChannelListing {
            id: "MLB1".into(),
            variation: None,
            title: "Fone Bluetooth".into(),
            price: brl(90),
            available_quantity: 10,
            status: ListingStatus::Active,
            link: None,
            listing_type: Some(ListingType::Classic),
            category: Some("MLB3697".into()),
            seller_sku: None,
        }
    }

    /// The Mercado Livre both machines read: one listing and its Orders.
    #[derive(Default)]
    struct Channel {
        orders: Mutex<Vec<ChannelOrder>>,
    }

    impl SalesChannel for Channel {
        fn listing_ids(&self) -> Result<Vec<String>, PlatformError> {
            Ok(vec![listing().id])
        }

        fn listings(&self, _: &[String]) -> Result<Vec<ChannelListing>, PlatformError> {
            Ok(vec![listing()])
        }

        fn sale_fee(&self, _: &str, _: Money, _: ListingType) -> Result<SaleFee, PlatformError> {
            Err(PlatformError::NotFound)
        }

        fn set_price(&self, _: &str, _: Money) -> Result<(), PlatformError> {
            Err(PlatformError::NotFound)
        }

        fn set_stock(&self, _: &str, _: &[ChannelStock]) -> Result<(), PlatformError> {
            Err(PlatformError::NotFound)
        }

        fn pause(&self, _: &str) -> Result<(), PlatformError> {
            Err(PlatformError::NotFound)
        }

        fn activate(&self, _: &str) -> Result<(), PlatformError> {
            Err(PlatformError::NotFound)
        }

        fn category(&self, id: &str) -> Result<ChannelCategory, PlatformError> {
            Ok(ChannelCategory {
                id: id.into(),
                name: "Fones".into(),
            })
        }
    }

    impl ChannelOrders for Channel {
        fn orders_changed_since(&self, _: Timestamp) -> Result<Vec<ChannelOrder>, PlatformError> {
            Ok(self.orders.lock().unwrap().clone())
        }
    }

    fn sold_order(at: Timestamp) -> ChannelOrder {
        ChannelOrder {
            id: "2000001".into(),
            pack: None,
            status: OrderStatus::Paid,
            ordered_at: at,
            updated_at: at,
            lines: vec![ChannelOrderLine {
                item: "MLB1".into(),
                variation: None,
                title: "Fone Bluetooth".into(),
                variation_name: None,
                quantity: 2,
                unit_price: brl(90),
                sale_fee: None,
                listing_type: Some(ListingType::Classic),
            }],
            total: brl(180),
            paid: Some(brl(180)),
            shipping_paid: None,
            refunded: None,
            buyer: None,
            shipment: None,
            returns: Vec::new(),
        }
    }

    /// One machine's database and the modules the owner uses on it.
    struct Machine {
        database: Arc<Database>,
        catalog: Catalog,
        listings: Listings,
        purchases: PurchaseOrders,
        orders: Orders,
        events: SalesEvents,
    }

    impl Machine {
        async fn open(path: &Path, clock: Arc<ManualClock>, first_id: u64) -> Self {
            let database = Arc::new(Database::open(path).await.unwrap());
            migrate(&database, clock.as_ref(), MODULE_MIGRATIONS)
                .await
                .unwrap();
            let ids: Arc<dyn IdGenerator> = Arc::new(IdsFrom(AtomicU64::new(first_id)));
            let clock: Arc<dyn Clock> = clock;
            let inventory = Arc::new(Inventory::new(database.clone(), clock.clone(), ids.clone()));
            Self {
                catalog: Catalog::new(
                    database.clone(),
                    path.with_extension("files"),
                    clock.clone(),
                    ids.clone(),
                ),
                listings: Listings::new(database.clone(), clock.clone(), ids.clone()),
                purchases: PurchaseOrders::new(
                    database.clone(),
                    inventory.clone(),
                    clock.clone(),
                    ids.clone(),
                ),
                orders: Orders::new(database.clone(), inventory, clock.clone(), ids.clone()),
                events: SalesEvents::new(database.clone(), clock, ids),
                database,
            }
        }
    }

    /// Every row of every table, to tell whether anything was written.
    async fn dump(database: &Database) -> Vec<String> {
        let connection = database.connection();
        let mut tables = Vec::new();
        let mut rows = connection
            .query("SELECT name FROM sqlite_master WHERE type = 'table'", ())
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

    fn records<'a>(report: &'a MergeReport, noun: &str) -> Option<&'a RecordsReport> {
        report.records.iter().find(|records| records.noun == noun)
    }

    #[test]
    fn every_table_of_every_module_is_declared_for_the_merge_import() {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let database = Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap();
            let clock = ManualClock::at(at(0));
            migrate(&database, &clock, MODULE_MIGRATIONS).await.unwrap();

            let gaps = merge_gaps(database.connection(), MODULE_MERGES)
                .await
                .unwrap();

            assert!(
                gaps.is_empty(),
                "{}",
                gaps.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        });
    }

    #[test]
    fn the_same_ml_order_synced_on_two_machines_compares_as_one_order_and_one_sale() {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let clock = Arc::new(ManualClock::at(at(0)));
            let channel = Channel::default();
            // The first machine sets up the product, its listing and stock.
            let first = Machine::open(&dir.path().join("first.db"), clock.clone(), 1_000).await;
            let supplier = first.catalog.add_supplier("Loja do Centro").await.unwrap();
            let fone = first
                .catalog
                .add_product("Fone Bluetooth", "FONE-01")
                .await
                .unwrap();
            first.listings.sync(&channel).await.unwrap();
            let found = first.listings.listings().await.unwrap();
            first.listings.link(found[0].id, fone.id).await.unwrap();
            // The first Order Sync, with nothing sold yet, before the export.
            first.orders.sync(&channel).await.unwrap();
            let purchase = first
                .purchases
                .create(NewPurchaseOrder {
                    supplier: supplier.id,
                    ordered_on: at(0).date_naive(),
                    freight: brl(0),
                    lines: vec![NewPurchaseLine {
                        product: fone.id,
                        quantity: 10,
                        unit_price: brl(40),
                    }],
                })
                .await
                .unwrap();
            first
                .purchases
                .receive(
                    purchase.id,
                    HOME_LOCATION,
                    &[Receiving {
                        line: purchase.lines[0].id,
                        quantity: 10,
                    }],
                )
                .await
                .unwrap();
            // Carried to the second machine as an export.
            let second_path = dir.path().join("second.db");
            first
                .database
                .connection()
                .execute("VACUUM INTO ?1", [second_path.to_str().unwrap()])
                .await
                .unwrap();
            let second = Machine::open(&second_path, clock.clone(), 2_000_000).await;

            // From then on each machine works on its own.
            clock.advance(TimeDelta::minutes(10));
            channel.orders.lock().unwrap().push(sold_order(clock.now()));
            first.orders.sync(&channel).await.unwrap();
            first.events.record_defaults().await.unwrap();
            clock.advance(TimeDelta::minutes(5));
            second.orders.sync(&channel).await.unwrap();
            second.events.record_defaults().await.unwrap();
            second.catalog.add_product("Capa", "CAPA-01").await.unwrap();
            clock.advance(TimeDelta::minutes(5));
            first.catalog.rename_sku(fone.id, "FONE-BT").await.unwrap();
            clock.advance(TimeDelta::minutes(5));
            second
                .catalog
                .rename_sku(fone.id, "FONE-BT-01")
                .await
                .unwrap();
            let export = dir.path().join("second-export.db");
            second
                .database
                .connection()
                .execute("VACUUM INTO ?1", [export.to_str().unwrap()])
                .await
                .unwrap();
            let before = dump(&first.database).await;
            let merge = MergeImport::new(
                first.database.clone(),
                MODULE_MIGRATIONS,
                MODULE_MERGES,
                clock.clone(),
                Arc::new(IdsFrom(AtomicU64::new(9_000_000))),
            );

            let report = merge.compare(&export).await.unwrap();

            let products = records(&report, "Produto").unwrap();
            assert_eq!((products.new, products.changed_here), (1, 1));
            assert_eq!(products.conflicts.len(), 1);
            assert_eq!(products.conflicts[0].label, "FONE-BT-01 · Fone Bluetooth");
            assert_eq!(products.conflicts[0].winner, Side::There);
            // The Order each machine read is one Order: its line and its
            // sale are the same, whatever the time each Sync wrote them.
            let orders = records(&report, "Pedido");
            assert!(orders.is_none_or(|orders| orders.new == 0), "{report:#?}");
            assert!(records(&report, "Linha de pedido").is_none(), "{report:#?}");
            assert!(
                records(&report, "Movimento de estoque").is_none(),
                "{report:#?}"
            );
            // The common dates each machine recorded are the same dates.
            assert!(records(&report, "Data comercial").is_none(), "{report:#?}");
            assert!(
                report
                    .records
                    .iter()
                    .all(|records| records.left_out.is_empty()),
                "{report:#?}"
            );
            assert_eq!(dump(&first.database).await, before);
        });
    }

    #[test]
    fn each_released_sample_database_compares_with_this_version() {
        block_on(async {
            let released = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/released-databases");
            let dir = tempfile::tempdir().unwrap();
            let database = Arc::new(
                Database::open(&dir.path().join("mascate.db"))
                    .await
                    .unwrap(),
            );
            let clock = Arc::new(ManualClock::at(at(0)));
            migrate(&database, clock.as_ref(), MODULE_MIGRATIONS)
                .await
                .unwrap();
            let merge = MergeImport::new(
                database,
                MODULE_MIGRATIONS,
                MODULE_MERGES,
                clock,
                Arc::new(IdsFrom(AtomicU64::new(1))),
            );
            let mut samples: Vec<_> = std::fs::read_dir(&released)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "db"))
                .collect();
            samples.sort();
            assert!(!samples.is_empty());

            for sample in samples {
                let before = std::fs::read(&sample).unwrap();

                let report = merge.compare(&sample).await.unwrap();

                // Every sample from 0.2.0 on holds one Product; 0.1.0 had none.
                let products = records(&report, "Produto").map_or(0, |products| products.new);
                let has_catalog = sample.file_stem().is_some_and(|name| name != "0.1.0");
                assert_eq!(products, usize::from(has_catalog), "{}", sample.display());
                assert!(!report.is_empty(), "{}", sample.display());
                assert!(
                    report.records.iter().all(|r| r.left_out.is_empty()),
                    "{}: {report:#?}",
                    sample.display()
                );
                assert_eq!(std::fs::read(&sample).unwrap(), before);
            }
        });
    }

    fn conflict(here_deleted: bool, winner: Side) -> MergeConflict {
        MergeConflict {
            label: "FONE-01 · Fone".into(),
            here: ConflictSide {
                changed_at: Some(at(5)),
                deleted: here_deleted,
            },
            there: ConflictSide {
                changed_at: None,
                deleted: false,
            },
            winner,
        }
    }

    #[test]
    fn counts_leave_out_zeros_and_agree_in_number() {
        let records = RecordsReport {
            module: "catalog",
            noun: "Produto",
            new: 3,
            changed_here: 1,
            conflicts: vec![conflict(false, Side::Here)],
            left_out: Vec::new(),
        };

        assert_eq!(counts(&records), "3 chegam · 1 muda aqui · 1 conflito");
    }

    #[test]
    fn a_conflict_says_what_each_side_did_and_which_version_stays() {
        let line = conflict_line("Produto", &conflict(true, Side::Here));

        assert!(
            line.starts_with("FONE-01 · Fone: este computador excluiu em "),
            "{line}"
        );
        assert!(
            line.ends_with("; o outro banco mudou sem data. Fica a versão daqui."),
            "{line}"
        );
    }

    #[test]
    fn a_record_left_out_says_why_and_falls_back_to_its_noun() {
        let missing = LeftOut {
            label: String::new(),
            reason: LeftOutReason::MissingRecord { noun: "Pedido" },
        };
        let taken = LeftOut {
            label: "CABO · Fone".into(),
            reason: LeftOutReason::KeyTaken {
                by: "CABO · Cabo".into(),
            },
        };

        assert_eq!(
            left_out_line("Linha de pedido", &missing),
            "Linha de pedido fica de fora: aponta para um registro de Pedido que não existe em \
             nenhum dos dois bancos."
        );
        assert_eq!(
            left_out_line("Produto", &taken),
            "CABO · Fone fica de fora: CABO · Cabo já usa o mesmo identificador aqui."
        );
    }

    #[test]
    fn long_lists_sum_up_the_rest() {
        let lines: Vec<String> = (0..25).map(|n| n.to_string()).collect();

        let shown = listed(lines);

        assert_eq!(shown.len(), LISTED + 1);
        assert_eq!(shown.last().unwrap(), "E mais 5.");
    }
}
