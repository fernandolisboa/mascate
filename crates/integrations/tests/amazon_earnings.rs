//! The Amazon Associados earnings report reader over files written from the
//! documentation (see the fixtures' README), and its import into the
//! Affiliate module with a real temporary database.

mod common;

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use common::query_value;
use futures::executor::block_on;
use mascate_affiliate::LinkTagger;
use mascate_affiliate::{
    Affiliate, AffiliatePlatform, Commission, CommissionFilter, CommissionStatus, MIGRATIONS,
    Recorded, ReportedConversion,
};
use mascate_integrations::{
    AMAZON_LINKS, AMAZON_PLATFORM, EarningsImport, EarningsReport, ReportError, SkippedRow,
    read_earnings_report,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, Timestamp};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/amazon/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn report(name: &str) -> EarningsReport {
    read_earnings_report(&fixture(name)).unwrap()
}

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

/// A UTC instant in 2026.
fn utc(month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Timestamp {
    Utc.with_ymd_and_hms(2026, month, day, hour, minute, second)
        .unwrap()
}

fn by_product<'a>(
    conversions: &'a [ReportedConversion],
    product: &str,
) -> Vec<&'a ReportedConversion> {
    conversions
        .iter()
        .filter(|conversion| conversion.product == product)
        .collect()
}

fn skipped(line: usize, reason: &str) -> SkippedRow {
    SkippedRow {
        line,
        reason: reason.into(),
    }
}

const PANS: &str = "Jogo de Panelas Tramontina\nTurim 5 peças";
const HEADPHONES: &str = "Fone de Ouvido Bluetooth JBL Tune 520BT";

#[test]
fn every_row_of_the_documented_csv_becomes_a_commission_in_reais() {
    let report = report("earnings-2026-09.csv");

    assert_eq!(report.sales(), 8);
    assert_eq!(report.returns(), 1);
    assert_eq!(
        report.period(),
        Some(
            NaiveDate::from_ymd_opt(2026, 9, 3).unwrap()
                ..=NaiveDate::from_ymd_opt(2026, 9, 28).unwrap()
        )
    );
    let import = report.conversions(&[]);
    assert_eq!(import.skipped, Vec::new());
    let conversions = import.conversions;
    assert_eq!(conversions.len(), 8);

    let book = by_product(&conversions, "Livro \"Rápido e Devagar\", edição de bolso");
    assert_eq!(book.len(), 1, "quotes and a comma inside the name");
    let book = book[0];
    assert_eq!(book.program.id, "amazon-associates");
    assert_eq!(book.program.name, "Amazon Associados");
    assert_eq!(book.commission, brl("3.84"));
    assert_eq!(book.status, CommissionStatus::Approved);
    assert_eq!(book.platform_status, "Enviado");
    assert_eq!(book.tracking.as_deref(), Some("mascate-20"));
    // Date Shipped is Brasília time.
    assert_eq!(book.sold_at, utc(9, 5, 3, 0, 0));
    assert_eq!(book.status_changed_at, Some(utc(9, 5, 3, 0, 0)));

    let pans = by_product(&conversions, PANS);
    assert_eq!(pans.len(), 1, "a line break inside the name");
    assert_eq!(pans[0].commission, brl("15.60"));
    assert_eq!(pans[0].tracking.as_deref(), Some("mascate-tg-20"));

    // A whole fee without decimals.
    let lego = by_product(&conversions, "LEGO Classic 11717");
    assert_eq!(lego[0].commission, brl("6"));

    // The return of the same file reverses its sale; it is no Commission
    // of its own.
    let headphones = by_product(&conversions, HEADPHONES);
    assert_eq!(headphones.len(), 1);
    assert_eq!(headphones[0].status, CommissionStatus::Reversed);
    assert_eq!(headphones[0].platform_status, "Devolvido");
    assert_eq!(headphones[0].commission, brl("8.97"));
    assert_eq!(headphones[0].sold_at, utc(9, 3, 13, 15, 22));
    assert_eq!(headphones[0].status_changed_at, Some(utc(9, 20, 21, 44, 1)));

    // Three rows of the same item on the same day, two of them identical,
    // are three Commissions.
    let books = by_product(&conversions, "Box Harry Potter");
    assert_eq!(books.len(), 3);
    let mut keys: Vec<&str> = books.iter().map(|book| book.key.as_str()).collect();
    keys.dedup();
    assert_eq!(keys.len(), 3);
}

#[test]
fn the_same_report_as_xlsx_or_as_a_brazilian_csv_yields_the_same_conversions() {
    let csv = report("earnings-2026-09.csv").conversions(&[]);

    for name in [
        // Serial dates, numbers and shared strings.
        "earnings-2026-09.xlsx",
        // `;`, comma decimals, dd/mm/aaaa, pt-BR headers, Windows-1252.
        "earnings-2026-09-br.csv",
        // Tabs, a UTF-8 BOM and CRLF.
        "earnings-2026-09-tab.csv",
    ] {
        assert_eq!(report(name).conversions(&[]), csv, "{name}");
    }
}

#[test]
fn money_reads_as_the_decimal_written_never_through_float_drift() {
    for name in ["earnings-2026-09.csv", "earnings-2026-09.xlsx"] {
        let import = report(name).conversions(&[]);
        let amounts: Vec<String> = import
            .conversions
            .iter()
            .map(|conversion| conversion.commission.amount().to_string())
            .collect();
        assert_eq!(
            amounts,
            ["8.97", "3.84", "15.6", "1.3", "1.3", "1.3", "7.5", "6"],
            "{name}"
        );
    }
}

#[test]
fn a_file_that_is_not_an_earnings_report_is_refused() {
    assert_eq!(
        read_earnings_report(&fixture("orders-2026-09.csv")),
        Err(ReportError::NotEarningsReport)
    );
    assert_eq!(
        read_earnings_report(b""),
        Err(ReportError::NotEarningsReport)
    );
    assert!(matches!(
        read_earnings_report(b"PK\x03\x04not really a zip"),
        Err(ReportError::Unreadable(_))
    ));
}

/// The Affiliate module over a real temporary database.
struct Imports {
    _dir: tempfile::TempDir,
    affiliate: Affiliate,
}

impl Imports {
    fn new() -> Self {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let database = Arc::new(
                Database::open(&dir.path().join("mascate.db"))
                    .await
                    .unwrap(),
            );
            let clock = Arc::new(ManualClock::at(utc(10, 13, 12, 0, 0)));
            migrate(&database, clock.as_ref(), &[MIGRATIONS])
                .await
                .unwrap();
            Imports {
                _dir: dir,
                affiliate: Affiliate::new(database, clock, Arc::new(SequentialIds::default())),
            }
        })
    }

    /// What the app does: the report meets what earlier imports recorded.
    fn planned(&self, name: &str) -> EarningsImport {
        let readings = block_on(self.affiliate.readings(AMAZON_PLATFORM)).unwrap();
        report(name).conversions(&readings)
    }

    fn import(&self, name: &str) -> (Recorded, Vec<SkippedRow>) {
        let plan = self.planned(name);
        let recorded = block_on(self.affiliate.import(AMAZON_PLATFORM, &plan.conversions)).unwrap();
        (recorded, plan.skipped)
    }

    fn commissions(&self) -> Vec<Commission> {
        block_on(self.affiliate.commissions(&CommissionFilter::default())).unwrap()
    }

    fn commission(&self, product: &str) -> Commission {
        let found: Vec<Commission> = self
            .commissions()
            .into_iter()
            .filter(|commission| commission.product == product)
            .collect();
        assert_eq!(found.len(), 1, "{product}");
        found[0].clone()
    }
}

fn recorded(added: usize, changed: usize, unchanged: usize) -> Recorded {
    Recorded {
        added,
        changed,
        unchanged,
    }
}

#[test]
fn importing_the_same_report_again_in_either_format_changes_nothing() {
    let imports = Imports::new();

    assert_eq!(imports.import("earnings-2026-09.csv").0, recorded(8, 0, 0));
    let first = imports.commissions();
    assert_eq!(imports.import("earnings-2026-09.csv").0, recorded(0, 0, 8));
    assert_eq!(imports.import("earnings-2026-09.xlsx").0, recorded(0, 0, 8));
    assert_eq!(
        imports.import("earnings-2026-09-br.csv").0,
        recorded(0, 0, 8)
    );

    assert_eq!(imports.commissions(), first);
    let headphones = imports.commission(HEADPHONES);
    assert_eq!(headphones.platform, AffiliatePlatform::Amazon);
    assert_eq!(headphones.status, CommissionStatus::Reversed);
    assert!(!headphones.manual);
}

#[test]
fn an_overlapping_report_adds_only_its_new_rows_and_reverses_earlier_sales() {
    let imports = Imports::new();
    imports.import("earnings-2026-09.csv");

    let (october, skipped_rows) = imports.import("earnings-2026-10.csv");

    // The Kindle is new; the pans got a return; the bottle, the LEGO and
    // the headphones' return were in September's report too.
    assert_eq!(october, recorded(1, 1, 3));
    assert_eq!(
        skipped_rows,
        [
            skipped(
                9,
                "devolução de 1 item sem a venda no app: importe também o relatório do mês em que \
                 o item foi enviado e depois este de novo"
            ),
            skipped(10, "data de envio ilegível: ontem"),
            skipped(11, "linha sem itens enviados nem devolvidos"),
        ]
    );
    assert_eq!(imports.commissions().len(), 9);
    let pans = imports.commission(PANS);
    assert_eq!(pans.status, CommissionStatus::Approved);
    assert_eq!(pans.amount, brl("7.80"));
    assert_eq!(pans.platform_status, "Devolvido em parte (1 de 2)");
    let kindle = imports.commission("Kindle Paperwhite");
    assert_eq!(kindle.amount, brl("23.97"));

    // The return is counted once, however many times the file comes in.
    assert_eq!(imports.import("earnings-2026-10.csv").0, recorded(0, 0, 5));
    assert_eq!(imports.import("earnings-2026-09.csv").0, recorded(0, 0, 8));
    assert_eq!(imports.commission(PANS).amount, brl("7.80"));
}

#[test]
fn a_return_imported_before_its_sale_waits_for_it() {
    let imports = Imports::new();

    let (october, skipped_rows) = imports.import("earnings-2026-10.csv");
    assert_eq!(october, recorded(3, 0, 0));
    let lines: Vec<usize> = skipped_rows.iter().map(|row| row.line).collect();
    // The headphones' and the pans' returns have no sale yet.
    assert_eq!(lines, [4, 7, 9, 10, 11]);

    // September brings the headphones with their own return.
    assert_eq!(imports.import("earnings-2026-09.csv").0, recorded(6, 0, 2));
    assert_eq!(
        imports.commission(HEADPHONES).status,
        CommissionStatus::Reversed
    );
    assert_eq!(imports.commission(PANS).amount, brl("15.60"));

    // Imported again, October finds the pans.
    assert_eq!(imports.import("earnings-2026-10.csv").0, recorded(0, 1, 4));
    assert_eq!(imports.commission(PANS).amount, brl("7.80"));
}

#[test]
fn a_preview_tells_what_an_import_would_do_and_keeps_nothing() {
    let imports = Imports::new();
    let plan = imports.planned("earnings-2026-09.csv");

    let preview = block_on(
        imports
            .affiliate
            .preview_import(AMAZON_PLATFORM, &plan.conversions),
    )
    .unwrap();

    assert_eq!(preview, recorded(8, 0, 0));
    assert!(imports.commissions().is_empty());
    assert_eq!(imports.import("earnings-2026-09.csv").0, preview);
}

/// One generated row: an item of a few, shipped or returned on one of two
/// days.
#[derive(Debug, Clone)]
struct GeneratedRow {
    item: usize,
    day: u32,
    shipped: u32,
    returned: u32,
    fee_cents: u32,
    device: &'static str,
}

fn generated_row() -> impl Strategy<Value = GeneratedRow> {
    (
        0..3usize,
        1..3u32,
        prop_oneof![Just((1, 0)), Just((2, 0)), Just((0, 1))],
        1..5000u32,
        prop_oneof![Just("PHONE"), Just("DESKTOP")],
    )
        .prop_map(
            |(item, day, (shipped, returned), fee_cents, device)| GeneratedRow {
                item,
                day,
                shipped,
                returned,
                fee_cents,
                device,
            },
        )
}

/// The rows as a report: `,` with dot decimals, or `;` with commas.
fn written(rows: &[GeneratedRow], brazilian: bool) -> Vec<u8> {
    let (separator, point) = if brazilian { (";", ",") } else { (",", ".") };
    let mut text = String::from("Fee-Earnings reports from 09-01-2026 to 09-30-2026\n");
    text.push_str(
        &[
            "Category",
            "Name",
            "ASIN",
            "Seller",
            "Tracking ID",
            "Date Shipped",
            "Price($)",
            "Items Shipped",
            "Returns",
            "Revenue($)",
            "Ad Fees($)",
            "Device Type Group",
        ]
        .join(separator),
    );
    text.push('\n');
    for row in rows {
        let sign = if row.returned > 0 { "-" } else { "" };
        let fee = format!(
            "{sign}{}{point}{:02}",
            row.fee_cents / 100,
            row.fee_cents % 100
        );
        let cells = [
            "Livros".to_owned(),
            format!("\"Item {}, edição\"", row.item),
            format!("B0000000{:02}", row.item),
            "Amazon.com.br".to_owned(),
            "mascate-20".to_owned(),
            format!("2026-09-{:02} 00:00:00", row.day),
            format!("19{point}90"),
            row.shipped.to_string(),
            row.returned.to_string(),
            format!("{sign}19{point}90"),
            fee,
            row.device.to_owned(),
        ];
        text.push_str(&cells.join(separator));
        text.push('\n');
    }
    text.into_bytes()
}

/// Each Conversion by its key.
fn keyed(import: EarningsImport) -> BTreeMap<String, ReportedConversion> {
    import
        .conversions
        .into_iter()
        .map(|conversion| (conversion.key.clone(), conversion))
        .collect()
}

proptest! {
    #[test]
    fn reordering_the_rows_never_changes_a_key(
        (rows, shuffled) in prop::collection::vec(generated_row(), 1..12)
            .prop_flat_map(|rows| (Just(rows.clone()), Just(rows).prop_shuffle()))
    ) {
        let original = read_earnings_report(&written(&rows, false)).unwrap();
        let reordered = read_earnings_report(&written(&shuffled, false)).unwrap();

        let original = keyed(original.conversions(&[]));
        prop_assert_eq!(&keyed(reordered.conversions(&[])), &original);
        // Every shipped row is a Commission of its own, identical or not.
        prop_assert_eq!(original.len(), rows.iter().filter(|row| row.shipped > 0).count());
    }

    #[test]
    fn a_brazilian_csv_reads_as_the_american_one(
        rows in prop::collection::vec(generated_row(), 1..12)
    ) {
        let american = read_earnings_report(&written(&rows, false)).unwrap();
        let brazilian = read_earnings_report(&written(&rows, true)).unwrap();

        prop_assert_eq!(brazilian.conversions(&[]), american.conversions(&[]));
    }

    #[test]
    fn importing_in_any_order_ends_in_the_same_commissions(
        (rows, shuffled) in prop::collection::vec(generated_row(), 1..10)
            .prop_flat_map(|rows| (Just(rows.clone()), Just(rows).prop_shuffle()))
    ) {
        let original = Imports::new();
        let reordered = Imports::new();
        let report = read_earnings_report(&written(&rows, false)).unwrap();
        let shuffled = read_earnings_report(&written(&shuffled, false)).unwrap();
        let import = |imports: &Imports, report: &EarningsReport| {
            let readings = block_on(imports.affiliate.readings(AMAZON_PLATFORM)).unwrap();
            let plan = report.conversions(&readings);
            block_on(imports.affiliate.import(AMAZON_PLATFORM, &plan.conversions)).unwrap()
        };

        import(&original, &report);
        import(&reordered, &shuffled);
        let again = import(&reordered, &report);

        prop_assert_eq!(again.added + again.changed, 0);
        let summary = |imports: &Imports| {
            let mut commissions: Vec<(String, Money, CommissionStatus)> = imports
                .commissions()
                .into_iter()
                .map(|commission| (commission.product, commission.amount, commission.status))
                .collect();
            commissions.sort_by_key(|(product, amount, status)| (product.clone(), amount.amount(), *status));
            commissions
        };
        prop_assert_eq!(summary(&reordered), summary(&original));
    }
}

/// The tracking id the app's link carries in `tag` is the one the report
/// shows for the sale.
#[test]
fn a_tagged_amazon_link_comes_back_as_the_sales_marking() {
    let tagged = AMAZON_LINKS
        .tag(
            "https://www.amazon.com.br/dp/B0C1234567?tag=antigo-20&th=1",
            "mascate-tg-20",
        )
        .unwrap();
    assert_eq!(
        tagged,
        "https://www.amazon.com.br/dp/B0C1234567?tag=mascate-tg-20&th=1"
    );
    let tracking_id = query_value(&tagged, "tag").unwrap();
    let written = String::from_utf8(fixture("earnings-2026-09.csv"))
        .unwrap()
        .replacen(",mascate-20,", &format!(",{tracking_id},"), 1);

    let import = read_earnings_report(written.as_bytes())
        .unwrap()
        .conversions(&[]);

    let headphones = by_product(
        &import.conversions,
        "Fone de Ouvido Bluetooth JBL Tune 520BT",
    );
    assert_eq!(headphones[0].tracking.as_deref(), Some("mascate-tg-20"));
}
