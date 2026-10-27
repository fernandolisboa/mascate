//! The Amazon Associados earnings report (#83), exported from Associates
//! Central as CSV or XLSX, read into Conversions for the Affiliate module
//! (ADR 0030, decision 4). Amazon has no earnings API, and the report has
//! no order id: a row is known by its Tracking ID, ASIN, Seller, Price and
//! Date Shipped, plus its place among identical rows, so importing the same
//! file again, or one covering the same days, never duplicates.
//!
//! A shipped row is an approved Commission. A return comes as a row of its
//! own, dated when the item came back, with the units in Returns and the
//! fee negative; it reverses its share of the oldest sale of the same item
//! that still has units, from this file or from one imported before.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::RangeInclusive;

use chrono::{NaiveDate, NaiveDateTime, TimeZone as _};
use mascate_affiliate::{AffiliatePlatform, CommissionStatus, Reading, ReportedConversion};
use mascate_kernel::{Currency, Money, Timestamp, brasilia, fold, parse_amount};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::link_tags::UrlTagger;
use crate::spreadsheet::{Cell, TableError, TableRow, excel_serial, read_table};

/// The Platform the report's Conversions belong to.
pub const PLATFORM: AffiliatePlatform = AffiliatePlatform::Amazon;

/// An Amazon link carries the tracking id the owner created in the
/// Associates panel in `tag`, which the report shows per sale. Brazilian
/// tracking ids end in `-20`.
pub const LINKS: UrlTagger = UrlTagger {
    platform: PLATFORM,
    parameter: "tag",
    domains: &["amazon.com.br"],
    tag_ends_with: Some("-20"),
};

/// The report's columns. Amazon writes the currency after the money ones,
/// as in `Ad Fees($)`; that suffix is not part of the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Column {
    Category,
    Name,
    Asin,
    Seller,
    TrackingId,
    DateShipped,
    Price,
    ItemsShipped,
    Returns,
    Revenue,
    AdFees,
    DeviceTypeGroup,
}

/// Each column with the names it goes by: the English ones from Amazon's
/// documentation, then the Portuguese ones guessed for amazon.com.br until
/// the phase 2 regression pass (#76) records a real export.
const COLUMNS: [(Column, &[&str]); 12] = [
    (Column::Category, &["Category", "Categoria"]),
    (Column::Name, &["Name", "Nome", "Produto"]),
    (Column::Asin, &["ASIN"]),
    (Column::Seller, &["Seller", "Vendedor"]),
    (
        Column::TrackingId,
        &["Tracking ID", "ID de rastreamento", "ID de acompanhamento"],
    ),
    (Column::DateShipped, &["Date Shipped", "Data de envio"]),
    (Column::Price, &["Price", "Preço"]),
    (Column::ItemsShipped, &["Items Shipped", "Itens enviados"]),
    (Column::Returns, &["Returns", "Devoluções"]),
    (Column::Revenue, &["Revenue", "Receita"]),
    (
        Column::AdFees,
        &["Ad Fees", "Taxas de publicidade", "Taxa de publicidade"],
    ),
    (
        Column::DeviceTypeGroup,
        &["Device Type Group", "Grupo de tipo de dispositivo"],
    ),
];

/// Without these a row is not a Commission the app can tell apart.
const REQUIRED: [Column; 9] = [
    Column::Name,
    Column::Asin,
    Column::Seller,
    Column::TrackingId,
    Column::DateShipped,
    Column::Price,
    Column::ItemsShipped,
    Column::Returns,
    Column::AdFees,
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReportError {
    /// No header of the earnings report near the top: another report or
    /// another file.
    #[error("not an Amazon Associados earnings report")]
    NotEarningsReport,
    #[error("unreadable file: {0}")]
    Unreadable(String),
}

/// A row the import leaves out, and why, as the owner reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedRow {
    pub line: usize,
    pub reason: String,
}

/// An earnings report, read and checked, before it meets what earlier
/// imports brought.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarningsReport {
    sales: Vec<Sale>,
    returns: Vec<Return>,
    skipped: Vec<SkippedRow>,
}

/// What importing a report records, with every row left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarningsImport {
    /// The report's sales, and earlier ones a return in it reverses, the
    /// oldest first.
    pub conversions: Vec<ReportedConversion>,
    /// By line.
    pub skipped: Vec<SkippedRow>,
}

/// Reads an earnings report exported as CSV or XLSX. A file whose header
/// is not the report's is refused whole.
pub fn read_earnings_report(bytes: &[u8]) -> Result<EarningsReport, ReportError> {
    let table =
        read_table(bytes, |names| columns_of(names).is_some()).map_err(|error| match error {
            TableError::NoHeader => ReportError::NotEarningsReport,
            TableError::Unreadable(why) => ReportError::Unreadable(why),
        })?;
    let columns = columns_of(&table.header).ok_or(ReportError::NotEarningsReport)?;
    let mut rows = Vec::new();
    let mut skipped = Vec::new();
    for row in &table.rows {
        match Row::read(row, &columns) {
            Ok(read) => rows.push((row.line, read)),
            Err(reason) => skipped.push(SkippedRow {
                line: row.line,
                reason,
            }),
        }
    }
    let (sales, returns) = keyed(rows);
    Ok(EarningsReport {
        sales,
        returns,
        skipped,
    })
}

impl EarningsReport {
    /// Rows of items shipped.
    pub fn sales(&self) -> usize {
        self.sales.len()
    }

    /// Rows of items returned.
    pub fn returns(&self) -> usize {
        self.returns.len()
    }

    /// The first and last Date Shipped of the rows read.
    pub fn period(&self) -> Option<RangeInclusive<NaiveDate>> {
        let days = self
            .sales
            .iter()
            .map(|sale| sale.row.date_shipped.date())
            .chain(self.returns.iter().map(|row| row.row.date_shipped.date()));
        let (first, last) = days.fold(None, |span: Option<(NaiveDate, NaiveDate)>, day| {
            Some(span.map_or((day, day), |(first, last)| (first.min(day), last.max(day))))
        })?;
        Some(first..=last)
    }

    /// The Conversions to record, given what earlier imports recorded for
    /// Amazon (`recorded`, from `Affiliate::readings`): each sale in the
    /// report, and each sale, here or before, a return in the report
    /// reverses. A return with no sale left to reverse is skipped.
    pub fn conversions(&self, recorded: &[Reading]) -> EarningsImport {
        let mut sales: BTreeMap<String, Sale> = recorded
            .iter()
            .filter_map(|reading| Some((reading.key.clone(), Sale::from_raw(reading)?)))
            .collect();
        let mut touched: HashSet<String> = HashSet::new();
        for sale in &self.sales {
            let returned = sales
                .get(&sale.key)
                .map(|kept| kept.returned.clone())
                .unwrap_or_default();
            sales.insert(
                sale.key.clone(),
                Sale {
                    returned,
                    ..sale.clone()
                },
            );
            touched.insert(sale.key.clone());
        }
        let applied: HashMap<String, String> = sales
            .values()
            .flat_map(|sale| {
                sale.returned
                    .iter()
                    .map(|returned| (returned.key.clone(), sale.key.clone()))
            })
            .collect();

        let mut skipped = self.skipped.clone();
        let mut returns: Vec<&Return> = self.returns.iter().collect();
        returns.sort_by(|a, b| (a.row.date_shipped, &a.key).cmp(&(b.row.date_shipped, &b.key)));
        for returned in returns {
            if let Some(sale) = applied.get(&returned.key) {
                touched.insert(sale.clone());
                continue;
            }
            match reverse(&mut sales, returned) {
                Some(reversed) => touched.extend(reversed),
                None => skipped.push(SkippedRow {
                    line: returned.line,
                    reason: format!(
                        "devolução de {} sem a venda no app: importe também o relatório do mês em \
                         que o item foi enviado e depois este de novo",
                        units(returned.row.returns)
                    ),
                }),
            }
        }
        skipped.sort_by_key(|row| row.line);
        let mut conversions: Vec<ReportedConversion> = sales
            .values()
            .filter(|sale| touched.contains(&sale.key))
            .map(Sale::conversion)
            .collect();
        conversions.sort_by(|a, b| (a.sold_at, &a.key).cmp(&(b.sold_at, &b.key)));
        EarningsImport {
            conversions,
            skipped,
        }
    }
}

/// Places a return's units on the oldest sales of the same item shipped
/// before it that still have units; `None`, placing nothing, when they do
/// not have enough.
fn reverse(sales: &mut BTreeMap<String, Sale>, returned: &Return) -> Option<Vec<String>> {
    let mut candidates: Vec<&Sale> = sales
        .values()
        .filter(|sale| {
            sale.row.identity() == returned.row.identity()
                && sale.row.date_shipped <= returned.row.date_shipped
                && sale.units_left() > 0
        })
        .collect();
    candidates.sort_by(|a, b| (a.row.date_shipped, &a.key).cmp(&(b.row.date_shipped, &b.key)));
    let available: u32 = candidates.iter().map(|sale| sale.units_left()).sum();
    if available < returned.row.returns {
        return None;
    }
    let mut placing = Vec::new();
    let mut left = returned.row.returns;
    for sale in candidates {
        if left == 0 {
            break;
        }
        let units = left.min(sale.units_left());
        placing.push((sale.key.clone(), units));
        left -= units;
    }
    // The fee is split by units only when a return spans sales.
    let whole = placing.len() == 1;
    for (key, units) in &placing {
        let fee = if whole {
            returned.row.ad_fees
        } else {
            returned.row.ad_fees * Decimal::from(*units) / Decimal::from(returned.row.returns)
        };
        let sale = sales.get_mut(key).expect("a candidate is a sale");
        sale.returned.push(Returned {
            key: returned.key.clone(),
            units: *units,
            ad_fees: (-fee.abs()).normalize().to_string(),
            date: returned.row.date_text(),
        });
    }
    Some(placing.into_iter().map(|(key, _)| key).collect())
}

/// "1 item", "2 itens".
fn units(count: u32) -> String {
    match count {
        1 => "1 item".into(),
        count => format!("{count} itens"),
    }
}

/// The columns at their positions in `names`, when every required one is
/// there.
fn columns_of(names: &[String]) -> Option<HashMap<Column, usize>> {
    let mut found = HashMap::new();
    for (index, name) in names.iter().enumerate() {
        let name = folded(name);
        if let Some((column, _)) = COLUMNS
            .iter()
            .find(|(_, known)| known.iter().any(|known| folded(known) == name))
        {
            found.entry(*column).or_insert(index);
        }
    }
    REQUIRED
        .iter()
        .all(|column| found.contains_key(column))
        .then_some(found)
}

/// A header name without its currency suffix, as letters and digits only:
/// `Ad Fees($)` and `ad fees` are the same column.
fn folded(name: &str) -> String {
    let name = match name.find('(') {
        Some(start) => &name[..start],
        None => name,
    };
    name.chars().filter_map(fold).collect()
}

/// One row as read, before it is told apart from the others.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Row {
    category: String,
    name: String,
    asin: String,
    seller: String,
    tracking_id: String,
    #[serde(with = "date_text")]
    date_shipped: NaiveDateTime,
    price: Decimal,
    items_shipped: u32,
    returns: u32,
    revenue: Option<Decimal>,
    ad_fees: Decimal,
    device_type_group: String,
}

impl Row {
    fn read(row: &TableRow, columns: &HashMap<Column, usize>) -> Result<Row, String> {
        let cell = |column: Column| {
            columns
                .get(&column)
                .and_then(|&index| row.cells.get(index))
                .unwrap_or(&Cell::Empty)
        };
        let text = |column: Column| match cell(column) {
            Cell::Empty => String::new(),
            Cell::Text(text) => text.clone(),
            Cell::Number(number) => number.normalize().to_string(),
            Cell::Date(date) => date.format(DATE_FORMAT).to_string(),
        };
        let asin = text(Column::Asin).to_uppercase();
        if asin.is_empty() {
            return Err("linha sem ASIN".into());
        }
        let name = text(Column::Name);
        if name.is_empty() {
            return Err("linha sem o nome do produto".into());
        }
        let date_shipped = date(cell(Column::DateShipped))
            .ok_or_else(|| format!("data de envio ilegível: {}", text(Column::DateShipped)))?;
        let money = |column: Column, label: &str| {
            decimal(cell(column)).ok_or_else(|| format!("{label} ilegível: {}", text(column)))
        };
        let count = |column: Column, label: &str| {
            let number = money(column, label)?;
            u32::try_from(number.normalize().mantissa())
                .ok()
                .filter(|_| number.normalize().scale() == 0)
                .ok_or_else(|| format!("{label} não é um número inteiro: {}", text(column)))
        };
        let read = Row {
            category: text(Column::Category),
            name,
            asin,
            seller: text(Column::Seller),
            tracking_id: text(Column::TrackingId),
            date_shipped,
            price: money(Column::Price, "preço")?.normalize(),
            items_shipped: count(Column::ItemsShipped, "itens enviados")?,
            returns: count(Column::Returns, "devoluções")?,
            revenue: decimal(cell(Column::Revenue)).map(|revenue| revenue.normalize()),
            ad_fees: money(Column::AdFees, "comissão (Ad Fees)")?.normalize(),
            device_type_group: text(Column::DeviceTypeGroup),
        };
        if read.items_shipped == 0 && read.returns == 0 {
            return Err("linha sem itens enviados nem devolvidos".into());
        }
        if read.items_shipped > 0 && read.ad_fees.is_sign_negative() {
            return Err("venda com comissão negativa".into());
        }
        Ok(read)
    }

    /// What tells one item apart in the report.
    fn identity(&self) -> (&str, &str, &str, Decimal) {
        (&self.tracking_id, &self.asin, &self.seller, self.price)
    }

    fn date_text(&self) -> String {
        self.date_shipped.format(DATE_FORMAT).to_string()
    }

    /// Amazon's dates are Brasília time (UTC-3, no daylight saving since
    /// 2019).
    fn when(&self) -> Timestamp {
        brasilia()
            .from_local_datetime(&self.date_shipped)
            .single()
            .expect("a fixed offset has one local time")
            .to_utc()
    }
}

const DATE_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

mod date_text {
    use chrono::NaiveDateTime;
    use serde::{Deserialize as _, Deserializer, Serializer};

    use super::DATE_FORMAT;

    pub(super) fn serialize<S: Serializer>(
        date: &NaiveDateTime,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&date.format(DATE_FORMAT).to_string())
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<NaiveDateTime, D::Error> {
        let text = String::deserialize(deserializer)?;
        NaiveDateTime::parse_from_str(&text, DATE_FORMAT).map_err(serde::de::Error::custom)
    }
}

/// A money or count cell: a number in XLSX, text in CSV written either way,
/// `1.234,56` or `1234.56`.
fn decimal(cell: &Cell) -> Option<Decimal> {
    match cell {
        Cell::Number(number) => Some(*number),
        Cell::Text(text) => parse_amount(text),
        Cell::Empty | Cell::Date(_) => None,
    }
}

/// A date cell: a date in XLSX (or its serial number when the cell is not
/// formatted as a date), text in CSV in the ways Amazon and Excel write it.
fn date(cell: &Cell) -> Option<NaiveDateTime> {
    const WITH_TIME: [&str; 5] = [
        DATE_FORMAT,
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%d/%m/%Y %H:%M:%S",
        "%d/%m/%Y %H:%M",
    ];
    const DAY_ONLY: [&str; 2] = ["%Y-%m-%d", "%d/%m/%Y"];
    match cell {
        Cell::Date(date) => Some(*date),
        Cell::Number(serial) => excel_serial(serial.to_string().parse().ok()?),
        Cell::Text(text) => WITH_TIME
            .iter()
            .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
            .or_else(|| {
                DAY_ONLY.iter().find_map(|format| {
                    NaiveDate::parse_from_str(text, format)
                        .ok()?
                        .and_hms_opt(0, 0, 0)
                })
            }),
        Cell::Empty => None,
    }
}

/// A shipped row with its key and the returns placed on it so far.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Sale {
    key: String,
    row: Row,
    returned: Vec<Returned>,
}

/// A returned row with its key.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Return {
    key: String,
    line: usize,
    row: Row,
}

/// Units of a return placed on a sale, kept in the sale's raw reading so a
/// later import knows the return was counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Returned {
    key: String,
    units: u32,
    /// The return's share of the fee, negative.
    ad_fees: String,
    date: String,
}

/// What a sale keeps as its raw reading.
#[derive(Serialize, Deserialize)]
struct Raw {
    row: Row,
    returned: Vec<Returned>,
}

impl Sale {
    fn from_raw(reading: &Reading) -> Option<Sale> {
        let raw: Raw = serde_json::from_str(&reading.raw).ok()?;
        Some(Sale {
            key: reading.key.clone(),
            row: raw.row,
            returned: raw.returned,
        })
    }

    fn returned_units(&self) -> u32 {
        self.row.returns + self.returned.iter().map(|r| r.units).sum::<u32>()
    }

    fn units_left(&self) -> u32 {
        self.row.items_shipped.saturating_sub(self.returned_units())
    }

    fn conversion(&self) -> ReportedConversion {
        let returned = self.returned_units();
        let shipped = self.row.items_shipped;
        let last_return = self
            .returned
            .iter()
            .filter_map(|r| NaiveDateTime::parse_from_str(&r.date, DATE_FORMAT).ok())
            .max();
        let (status, platform_status, amount, changed) = if returned >= shipped {
            let when = last_return.map_or(self.row.date_shipped, |date| date);
            (
                CommissionStatus::Reversed,
                "Devolvido".to_owned(),
                self.row.ad_fees,
                when,
            )
        } else {
            let fees: Decimal = self
                .returned
                .iter()
                .filter_map(|r| r.ad_fees.parse::<Decimal>().ok())
                .sum();
            let status = if returned == 0 {
                "Enviado".to_owned()
            } else {
                format!("Devolvido em parte ({returned} de {shipped})")
            };
            (
                CommissionStatus::Approved,
                status,
                (self.row.ad_fees + fees).max(Decimal::ZERO),
                self.row.date_shipped,
            )
        };
        let when = |date: NaiveDateTime| {
            Row {
                date_shipped: date,
                ..self.row.clone()
            }
            .when()
        };
        let raw = Raw {
            row: self.row.clone(),
            returned: self.returned.clone(),
        };
        ReportedConversion {
            key: self.key.clone(),
            // Amazon has one Program for every product.
            program: AffiliatePlatform::Amazon
                .own_program()
                .expect("Amazon has one Program"),
            product: self.row.name.clone(),
            sold_at: self.row.when(),
            commission: Money::new(amount.normalize(), Currency::Brl),
            status,
            platform_status,
            status_changed_at: Some(when(changed)),
            tracking: Some(self.row.tracking_id.clone()).filter(|tracking| !tracking.is_empty()),
            raw: serde_json::to_string(&raw).expect("a report row serializes"),
        }
    }
}

/// Gives every row its natural key: its identity and Date Shipped, plus
/// its place among identical ones, counted in the order of their other
/// columns so the file's row order never changes a key.
fn keyed(rows: Vec<(usize, Row)>) -> (Vec<Sale>, Vec<Return>) {
    let mut groups: BTreeMap<(bool, String), Vec<(usize, Row)>> = BTreeMap::new();
    for (line, row) in rows {
        let shipped = row.items_shipped > 0;
        let identity = key_of(shipped, &row, 0);
        groups
            .entry((shipped, identity))
            .or_default()
            .push((line, row));
    }
    let mut sales = Vec::new();
    let mut returns = Vec::new();
    for ((shipped, _), mut group) in groups {
        group.sort_by_cached_key(|(_, row)| serde_json::to_string(row).expect("a row serializes"));
        for (place, (line, row)) in group.into_iter().enumerate() {
            let key = key_of(shipped, &row, place + 1);
            if shipped {
                sales.push((
                    line,
                    Sale {
                        key,
                        row,
                        returned: Vec::new(),
                    },
                ));
            } else {
                returns.push(Return { key, line, row });
            }
        }
    }
    sales.sort_by_key(|(line, _)| *line);
    returns.sort_by_key(|returned| returned.line);
    (sales.into_iter().map(|(_, sale)| sale).collect(), returns)
}

/// A JSON list, so no value can run into the next.
fn key_of(shipped: bool, row: &Row, place: usize) -> String {
    serde_json::to_string(&(
        if shipped { "shipped" } else { "returned" },
        &row.tracking_id,
        &row.asin,
        &row.seller,
        row.price.to_string(),
        row.date_text(),
        place,
    ))
    .expect("a key serializes")
}
