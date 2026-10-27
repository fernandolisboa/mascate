//! The spend reports the owner exports from the Meta Ads Manager and from
//! Google Ads, as CSV or XLSX, read into Promotion Spends for the
//! Marketing (#195, ADR 0037 decision 6). Neither is read through an API:
//! the owner exports the report broken down by day and picks the file.
//!
//! Each row is one campaign on one day; rows of the same campaign and day
//! (a report by ad set or by ad) are added up. A report without the day
//! breakdown is refused whole, because a period's total cannot be told
//! apart by day and would count twice against a daily one. The column names
//! follow both Platforms' help in English; the Portuguese ones are guessed
//! until the phase 3 regression pass (#116) records real exports.

use std::collections::{BTreeMap, HashMap};
use std::ops::RangeInclusive;

use chrono::NaiveDate;
use mascate_kernel::{Currency, Money};
use mascate_marketing::{ReportedSpend, SpendReport};
use rust_decimal::Decimal;

use crate::spreadsheet::{
    Cell, SkippedRow, TableError, TableRow, columns_of, date_time, decimal, folded_header,
    header_suffix, read_table,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Column {
    Campaign,
    Day,
    Spent,
    Currency,
    ReportingStarts,
    ReportingEnds,
}

/// The Ads Manager's columns. The amount spent carries the account's
/// currency in its name, as in `Amount spent (BRL)`.
const META_COLUMNS: [(Column, &[&str]); 6] = [
    (Column::Campaign, &["Campaign name", "Nome da campanha"]),
    (Column::Day, &["Day", "Dia"]),
    (
        Column::Spent,
        &[
            "Amount spent",
            "Valor usado",
            "Valor gasto",
            "Quantia gasta",
        ],
    ),
    (Column::Currency, &["Currency", "Moeda"]),
    (
        Column::ReportingStarts,
        &["Reporting starts", "Início dos relatórios"],
    ),
    (
        Column::ReportingEnds,
        &["Reporting ends", "Término dos relatórios"],
    ),
];

const META_REQUIRED: [Column; 4] = [
    Column::Campaign,
    Column::Spent,
    Column::ReportingStarts,
    Column::ReportingEnds,
];

/// Google Ads' columns. The day is a segment; without it the report has
/// one row per campaign for the whole period.
const GOOGLE_COLUMNS: [(Column, &[&str]); 4] = [
    (Column::Campaign, &["Campaign", "Campanha"]),
    (Column::Day, &["Day", "Dia"]),
    (Column::Spent, &["Cost", "Custo"]),
    (Column::Currency, &["Currency code", "Código da moeda"]),
];

const GOOGLE_REQUIRED: [Column; 2] = [Column::Campaign, Column::Spent];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpendReportError {
    /// No header of either report near the top: another report or file.
    #[error("not a Meta Ads Manager or Google Ads spend report")]
    NotSpendReport,
    /// The report has the whole period in one row per campaign.
    #[error("the {0:?} report is not broken down by day")]
    NotDaily(SpendReport),
    /// No column says which currency the amounts are in.
    #[error("the {0:?} report does not say its currency")]
    NoCurrency(SpendReport),
    #[error("unreadable file: {0}")]
    Unreadable(String),
}

/// A spend report, read and checked: each campaign's spend per day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendReading {
    pub report: SpendReport,
    /// By campaign, then day.
    pub spends: Vec<ReportedSpend>,
    /// By line.
    pub skipped: Vec<SkippedRow>,
}

impl SpendReading {
    /// The first and last day of the spends read.
    pub fn period(&self) -> Option<RangeInclusive<NaiveDate>> {
        let first = self.spends.iter().map(|spend| spend.day).min()?;
        let last = self.spends.iter().map(|spend| spend.day).max()?;
        Some(first..=last)
    }

    /// The report's campaigns, each once, by name.
    pub fn campaigns(&self) -> Vec<String> {
        let mut campaigns: Vec<String> = self
            .spends
            .iter()
            .map(|spend| spend.campaign.clone())
            .collect();
        campaigns.dedup();
        campaigns
    }

    /// What the report adds up to, one amount per currency.
    pub fn total(&self) -> Vec<Money> {
        let mut totals: BTreeMap<&'static str, Money> = BTreeMap::new();
        for spend in &self.spends {
            let currency = spend.amount.currency();
            let total = totals
                .entry(currency.code())
                .or_insert(Money::zero(currency));
            *total = total
                .checked_add(spend.amount)
                .expect("the same currency, by the key");
        }
        totals.into_values().collect()
    }
}

/// Reads a spend report of the Meta Ads Manager or of Google Ads, exported
/// as CSV or XLSX; which one it is comes from its header.
pub fn read_spend_report(bytes: &[u8]) -> Result<SpendReading, SpendReportError> {
    let table =
        read_table(bytes, |names| report_of(names).is_some()).map_err(|error| match error {
            TableError::NoHeader => SpendReportError::NotSpendReport,
            TableError::Unreadable(why) => SpendReportError::Unreadable(why),
        })?;
    let (report, columns) = report_of(&table.header).ok_or(SpendReportError::NotSpendReport)?;
    let header_currency = columns
        .get(&Column::Spent)
        .and_then(|&at| header_suffix(&table.header[at]))
        .map(str::to_owned);
    if !columns.contains_key(&Column::Currency) && header_currency.is_none() {
        return Err(SpendReportError::NoCurrency(report));
    }
    let reader = Reader {
        report,
        columns,
        header_currency,
    };
    let mut days: BTreeMap<(String, NaiveDate), (usize, Money)> = BTreeMap::new();
    let mut skipped = Vec::new();
    for row in &table.rows {
        let read = match reader.read(row) {
            Ok(Some(read)) => read,
            Ok(None) => continue,
            Err(Refusal::NotDaily) => return Err(SpendReportError::NotDaily(report)),
            Err(Refusal::Row(reason)) => {
                skipped.push(SkippedRow {
                    line: row.line,
                    reason,
                });
                continue;
            }
        };
        match days.get_mut(&(read.campaign.clone(), read.day)) {
            None => {
                days.insert((read.campaign, read.day), (row.line, read.amount));
            }
            Some((_, sum)) => match sum.checked_add(read.amount) {
                Ok(added) => *sum = added,
                Err(_) => skipped.push(SkippedRow {
                    line: row.line,
                    reason: format!(
                        "{} em {} numa moeda diferente das outras linhas do mesmo dia",
                        read.campaign,
                        read.day.format("%d/%m/%Y")
                    ),
                }),
            },
        }
    }
    skipped.sort_by_key(|row| row.line);
    Ok(SpendReading {
        report,
        spends: days
            .into_iter()
            .map(|((campaign, day), (_, amount))| ReportedSpend {
                report,
                campaign,
                day,
                amount: Money::new(amount.amount().normalize(), amount.currency()),
            })
            .collect(),
        skipped,
    })
}

/// Which report a header belongs to, with its columns.
fn report_of(names: &[String]) -> Option<(SpendReport, HashMap<Column, usize>)> {
    columns_of(names, &META_COLUMNS, &META_REQUIRED)
        .map(|columns| (SpendReport::MetaAds, columns))
        .or_else(|| {
            columns_of(names, &GOOGLE_COLUMNS, &GOOGLE_REQUIRED)
                .map(|columns| (SpendReport::GoogleAds, columns))
        })
}

/// One campaign's spend on one day, from one row.
struct Read {
    campaign: String,
    day: NaiveDate,
    amount: Money,
}

enum Refusal {
    /// The row covers more than one day: the whole report is refused.
    NotDaily,
    /// The row is left out, for this reason.
    Row(String),
}

struct Reader {
    report: SpendReport,
    columns: HashMap<Column, usize>,
    header_currency: Option<String>,
}

impl Reader {
    /// The row's spend; `None` for a row that is not a campaign's, like a
    /// report's totals.
    fn read(&self, row: &TableRow) -> Result<Option<Read>, Refusal> {
        let cell = |column: Column| {
            self.columns
                .get(&column)
                .and_then(|&at| row.cells.get(at))
                .unwrap_or(&Cell::Empty)
        };
        let text = |column: Column| match cell(column) {
            Cell::Empty => String::new(),
            Cell::Text(text) => text.clone(),
            Cell::Number(number) => number.normalize().to_string(),
            Cell::Date(date) => date.format("%Y-%m-%d").to_string(),
        };
        let campaign = text(Column::Campaign).trim().to_owned();
        if self.is_total(&campaign, &text(Column::Day)) {
            return Ok(None);
        }
        let day = self.day(&cell)?;
        let currency = match cell(Column::Currency) {
            Cell::Empty => self.header_currency.clone().unwrap_or_default(),
            _ => text(Column::Currency).trim().to_owned(),
        };
        let currency = Currency::from_code(&currency.to_uppercase()).ok_or_else(|| {
            Refusal::Row(format!(
                "moeda {currency}: o app conta gasto em real (BRL) ou dólar (USD)"
            ))
        })?;
        let amount = match cell(Column::Spent) {
            Cell::Empty => Decimal::ZERO,
            spent => decimal(spent).ok_or_else(|| {
                Refusal::Row(format!("valor gasto ilegível: {}", text(Column::Spent)))
            })?,
        };
        if amount.is_sign_negative() && !amount.is_zero() {
            return Err(Refusal::Row(format!(
                "valor gasto negativo: {}",
                text(Column::Spent)
            )));
        }
        Ok(Some(Read {
            campaign,
            day,
            amount: Money::new(amount, currency),
        }))
    }

    /// A totals row: Google Ads writes "Total: …" where the day goes, and
    /// the Ads Manager leaves the campaign empty.
    fn is_total(&self, campaign: &str, day: &str) -> bool {
        campaign.is_empty() || folded_header(day).starts_with("TOTAL")
    }

    /// The row's day: the Day column, or the reporting period when it
    /// starts and ends on the same day.
    fn day<'a>(&self, cell: &impl Fn(Column) -> &'a Cell) -> Result<NaiveDate, Refusal> {
        let day_of = |column: Column| date_time(cell(column)).map(|at| at.date());
        if self.columns.contains_key(&Column::Day) {
            return day_of(Column::Day).ok_or_else(|| Refusal::Row("dia ilegível".into()));
        }
        if self.report == SpendReport::GoogleAds {
            return Err(Refusal::NotDaily);
        }
        let starts = day_of(Column::ReportingStarts);
        let ends = day_of(Column::ReportingEnds);
        match (starts, ends) {
            (Some(starts), Some(ends)) if starts == ends => Ok(starts),
            (Some(_), Some(_)) => Err(Refusal::NotDaily),
            _ => Err(Refusal::Row(
                "início ou término do relatório ilegível".into(),
            )),
        }
    }
}
