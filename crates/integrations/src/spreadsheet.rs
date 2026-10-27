//! Spreadsheets the owner exports from a Platform's panel, as CSV or XLSX,
//! read into the same rows for each report's reader (ADR 0030, decision 4).
//! CSV goes through the `csv` crate and XLSX through `calamine`; what is
//! peculiar to Brazilian exports (`;` separators, Latin-1) is handled here,
//! what a value means is the report reader's.

use std::collections::HashMap;
use std::hash::Hash;
use std::io::Cursor;
use std::str::FromStr;

use calamine::{Data, Reader as _, Xlsx};
use chrono::{NaiveDate, NaiveDateTime, TimeDelta};
use mascate_kernel::{fold, parse_amount};
use rust_decimal::Decimal;

/// One cell, as the file holds it. CSV only has text; XLSX also has
/// numbers and dates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cell {
    Empty,
    Text(String),
    Number(Decimal),
    Date(NaiveDateTime),
}

impl Cell {
    /// As written in a header.
    fn text(&self) -> String {
        match self {
            Cell::Empty => String::new(),
            Cell::Text(text) => text.clone(),
            Cell::Number(number) => number.to_string(),
            Cell::Date(date) => date.to_string(),
        }
    }
}

/// The rows under a report's header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Table {
    pub(crate) header: Vec<String>,
    pub(crate) rows: Vec<TableRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TableRow {
    /// Where the row starts in the file, from 1, as a spreadsheet app
    /// numbers it.
    pub(crate) line: usize,
    pub(crate) cells: Vec<Cell>,
}

/// A row an import leaves out, and why, as the owner reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedRow {
    pub line: usize,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum TableError {
    /// No line near the top reads as the report's header.
    #[error("no header the report reader knows")]
    NoHeader,
    #[error("unreadable spreadsheet: {0}")]
    Unreadable(String),
}

/// How many lines a report may have above its header, like a title.
const HEADER_WITHIN: usize = 10;

/// The separators Brazilian and American exports use; the header line
/// tells which one a file has.
const SEPARATORS: [u8; 3] = *b";,\t";

/// Reads a CSV or XLSX file (an XLSX is a ZIP) into the rows under the
/// first line `is_header` accepts. Rows with nothing in them are left out.
pub(crate) fn read_table(
    bytes: &[u8],
    is_header: impl Fn(&[String]) -> bool,
) -> Result<Table, TableError> {
    if bytes.starts_with(b"PK\x03\x04") {
        read_xlsx(bytes, is_header)
    } else {
        read_csv(bytes, is_header)
    }
}

fn read_csv(bytes: &[u8], is_header: impl Fn(&[String]) -> bool) -> Result<Table, TableError> {
    let text = decoded(bytes);
    for separator in SEPARATORS {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .delimiter(separator)
            .from_reader(text.as_bytes());
        let mut header = None;
        let mut rows = Vec::new();
        for (index, record) in reader.records().enumerate() {
            let record = match record {
                Ok(record) => record,
                // Another separator may read it; past the header it is broken.
                Err(_) if header.is_none() => break,
                Err(error) => return Err(TableError::Unreadable(error.to_string())),
            };
            let cells: Vec<Cell> = record.iter().map(text_cell).collect();
            if header.is_none() {
                if index >= HEADER_WITHIN {
                    break;
                }
                let names: Vec<String> = cells.iter().map(Cell::text).collect();
                if is_header(&names) {
                    header = Some(names);
                }
                continue;
            }
            if cells.iter().all(|cell| *cell == Cell::Empty) {
                continue;
            }
            let line = record
                .position()
                .map_or(index + 1, |position| position.line() as usize);
            rows.push(TableRow { line, cells });
        }
        if let Some(header) = header {
            return Ok(Table { header, rows });
        }
    }
    Err(TableError::NoHeader)
}

fn read_xlsx(bytes: &[u8], is_header: impl Fn(&[String]) -> bool) -> Result<Table, TableError> {
    let unreadable = |error: calamine::XlsxError| TableError::Unreadable(error.to_string());
    let mut workbook = Xlsx::new(Cursor::new(bytes)).map_err(unreadable)?;
    let epoch_1904 = workbook.has_1904_epoch();
    let sheet = workbook
        .worksheet_range_at(0)
        .ok_or_else(|| TableError::Unreadable("no worksheet".into()))?
        .map_err(unreadable)?;
    let first_row = sheet.start().map_or(0, |(row, _)| row as usize);
    let mut header = None;
    let mut rows = Vec::new();
    for (index, row) in sheet.rows().enumerate() {
        let cells: Vec<Cell> = row.iter().map(|data| data_cell(data, epoch_1904)).collect();
        if header.is_none() {
            if index >= HEADER_WITHIN {
                break;
            }
            let names: Vec<String> = cells.iter().map(Cell::text).collect();
            if is_header(&names) {
                header = Some(names);
            }
            continue;
        }
        if cells.iter().all(|cell| *cell == Cell::Empty) {
            continue;
        }
        rows.push(TableRow {
            line: first_row + index + 1,
            cells,
        });
    }
    match header {
        Some(header) => Ok(Table { header, rows }),
        None => Err(TableError::NoHeader),
    }
}

/// UTF-8, with or without a BOM; UTF-16 with its BOM, as Google Ads writes
/// its "Excel .csv"; anything else is Windows-1252, the Latin-1 that Excel
/// on Windows writes for Brazilian Portuguese.
fn decoded(bytes: &[u8]) -> String {
    if let Some(text) = utf16(bytes) {
        return text;
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().map(|&byte| windows_1252(byte)).collect(),
    }
}

fn utf16(bytes: &[u8]) -> Option<String> {
    let little_endian = match bytes.get(..2)? {
        b"\xFF\xFE" => true,
        b"\xFE\xFF" => false,
        _ => return None,
    };
    let units: Vec<u16> = bytes[2..]
        .chunks_exact(2)
        .map(|pair| {
            let pair = [pair[0], pair[1]];
            if little_endian {
                u16::from_le_bytes(pair)
            } else {
                u16::from_be_bytes(pair)
            }
        })
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// Windows-1252 matches Latin-1 except for 0x80-0x9F, where it has the
/// curly quotes, dashes and the euro sign.
fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž',
        '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}',
        'ž', 'Ÿ',
    ];
    match byte {
        0x80..=0x9F => HIGH[usize::from(byte - 0x80)],
        byte => char::from(byte),
    }
}

/// A CSV value: trimmed, with line breaks as `\n` whatever the file used.
fn text_cell(value: &str) -> Cell {
    let value = value.trim();
    if value.is_empty() {
        Cell::Empty
    } else {
        Cell::Text(value.replace("\r\n", "\n").replace('\r', "\n"))
    }
}

fn data_cell(data: &Data, epoch_1904: bool) -> Cell {
    match data {
        Data::Empty => Cell::Empty,
        Data::String(text) | Data::DurationIso(text) => text_cell(text),
        Data::Int(number) => Cell::Number(Decimal::from(*number)),
        Data::Float(number) => float_cell(*number),
        Data::Bool(value) => Cell::Text(value.to_string()),
        Data::DateTime(date) if date.is_datetime() => {
            let days = if epoch_1904 { 1462.0 } else { 0.0 };
            excel_serial(date.as_f64() + days).map_or(Cell::Empty, Cell::Date)
        }
        Data::DateTime(duration) => float_cell(duration.as_f64()),
        Data::DateTimeIso(text) => iso_date(text).map_or_else(|| text_cell(text), Cell::Date),
        Data::Error(error) => Cell::Text(format!("#{error:?}")),
    }
}

/// A number as Excel shows it: the shortest decimal that reads back as the
/// same float, so `2.16` stays `2.16` and never `2.1600000000000001`.
fn float_cell(number: f64) -> Cell {
    if !number.is_finite() {
        return Cell::Text(number.to_string());
    }
    Decimal::from_str(&number.to_string())
        .map_or_else(|_| Cell::Text(number.to_string()), Cell::Number)
}

/// The date and time of an Excel serial number (days since 1899-12-30, the
/// time as the fraction), to the nearest second.
pub(crate) fn excel_serial(serial: f64) -> Option<NaiveDateTime> {
    if !serial.is_finite() || !(0.0..2_958_466.0).contains(&serial) {
        return None;
    }
    let seconds = (serial * 86_400.0).round() as i64;
    NaiveDate::from_ymd_opt(1899, 12, 30)?
        .and_hms_opt(0, 0, 0)?
        .checked_add_signed(TimeDelta::seconds(seconds))
}

fn iso_date(text: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
        })
}

/// The columns at their positions in `names`, each known by any of its
/// names in `known`, when every `required` one is there.
pub(crate) fn columns_of<C: Copy + Eq + Hash>(
    names: &[String],
    known: &[(C, &[&str])],
    required: &[C],
) -> Option<HashMap<C, usize>> {
    let mut found = HashMap::new();
    for (index, name) in names.iter().enumerate() {
        let name = folded_header(name);
        if let Some((column, _)) = known
            .iter()
            .find(|(_, names)| names.iter().any(|known| folded_header(known) == name))
        {
            found.entry(*column).or_insert(index);
        }
    }
    required
        .iter()
        .all(|column| found.contains_key(column))
        .then_some(found)
}

/// A header name without its currency suffix, as letters and digits only:
/// `Ad Fees($)`, `Amount spent (BRL)` and `ad fees` read as their names.
pub(crate) fn folded_header(name: &str) -> String {
    let name = match name.find('(') {
        Some(start) => &name[..start],
        None => name,
    };
    name.chars().filter_map(fold).collect()
}

/// The currency suffix of a header, as in `Amount spent (BRL)`.
pub(crate) fn header_suffix(name: &str) -> Option<&str> {
    let start = name.find('(')? + 1;
    let end = start + name[start..].find(')')?;
    Some(name[start..end].trim()).filter(|suffix| !suffix.is_empty())
}

/// A money or count cell: a number in XLSX, text in CSV written either way,
/// `1.234,56` or `1234.56`.
pub(crate) fn decimal(cell: &Cell) -> Option<Decimal> {
    match cell {
        Cell::Number(number) => Some(*number),
        Cell::Text(text) => parse_amount(text),
        Cell::Empty | Cell::Date(_) => None,
    }
}

/// A date cell: a date in XLSX (or its serial number when the cell is not
/// formatted as a date), text in CSV in the ways the Platforms and Excel
/// write it.
pub(crate) fn date_time(cell: &Cell) -> Option<NaiveDateTime> {
    const WITH_TIME: [&str; 5] = [
        "%Y-%m-%d %H:%M:%S",
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
