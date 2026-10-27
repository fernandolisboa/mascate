//! What every logged-in affiliate panel reader shares (#160, ADR 0035
//! decision 4): the tables a panel page shows, read through the
//! [`BrowserSession`] by one script that only reads; whether the page is a
//! login instead of the panel; the panel's state, as its Connection shows
//! it; and the report of sales each panel shows, read into Conversions by
//! the names its columns go by (#161). The login lives in the browser's
//! profile, never in the app.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{NaiveDate, NaiveDateTime, TimeDelta};
use mascate_affiliate::{
    AffiliatePlatform, CommissionSource, CommissionStatus, ReportedConversion,
};
use mascate_kernel::{Currency, Money, PlatformError, Timestamp, brasilia, fold, parse_amount};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::browser::BrowserSession;
use crate::crawler::asks_who_is_there;

/// Reads every table on the page, by its `<table>` or ARIA roles, as the
/// text of each cell, with the address the page ended on. It reads the
/// page and nothing else.
///
/// To record what a panel shows for its fixture, paste this into the
/// console of the browser's developer tools on the panel's page and copy
/// what it prints with `copy(...)` around it.
pub const TABLES_SCRIPT: &str = r#"(() => {
  const text = (cell) => (cell.innerText || cell.textContent || "").replace(/\s+/g, " ").trim();
  const cells = (row, which) => Array.from(row.querySelectorAll(which)).map(text);
  const tables = Array.from(document.querySelectorAll('table, [role="table"], [role="grid"]'))
    .map((table) => {
      const rows = Array.from(table.querySelectorAll('tr, [role="row"]'));
      const head = rows.find((row) => row.querySelector('th, [role="columnheader"]'));
      return {
        headers: head ? cells(head, 'th, td, [role="columnheader"]') : [],
        rows: rows
          .filter((row) => row !== head)
          .map((row) => cells(row, 'td, th, [role="cell"], [role="gridcell"], [role="rowheader"]'))
          .filter((row) => row.some((cell) => cell !== "")),
      };
    })
    .filter((table) => table.headers.length > 0);
  return { url: location.href, tables };
})()"#;

/// A panel page as the script read it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct PanelPage {
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) tables: Vec<PanelTable>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct PanelTable {
    pub(crate) headers: Vec<String>,
    #[serde(default)]
    pub(crate) rows: Vec<Vec<String>>,
}

impl PanelPage {
    /// Whether the panel sent the browser to a login or to a check of who
    /// is asking, which only the owner answers.
    pub(crate) fn is_login(&self) -> bool {
        let path = self
            .url
            .split_once("://")
            .map_or(self.url.as_str(), |(_, rest)| rest)
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        asks_who_is_there(&path)
    }
}

/// A header or a status as letters and digits only, accents dropped:
/// `Comissão` and `comissao` are the same.
fn folded(text: &str) -> String {
    text.chars().filter_map(fold).collect()
}

/// Where a panel's reading stands, as its Connection shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelState {
    /// Neither Edge nor Chrome is installed: nothing can open the panel.
    NoBrowser,
    /// The browser's profile was never opened, so there is no login yet.
    NeverLoggedIn,
    /// The last reading found a login page: the owner logs in again.
    LoggedOut,
    /// Nothing stands in the way of the next reading.
    Ready,
}

impl PanelState {
    /// Whether the hourly Sync asks the panel: a login page or a missing
    /// browser waits for the owner.
    pub fn can_read(self) -> bool {
        self == PanelState::Ready
    }
}

/// A logged-in affiliate panel read as a [`CommissionSource`], with the
/// state its Connection shows and the window the owner logs in through.
pub trait AffiliatePanel: CommissionSource {
    fn state(&self) -> PanelState;

    /// Opens the panel in the browser's window for the owner to log in by
    /// hand; the next reading tries again.
    fn open_login(&self) -> Result<(), PlatformError>;
}

/// A panel read through the shared browser, which remembers when its last
/// reading found a login page instead of the panel.
pub(crate) struct Panel {
    browser: Arc<BrowserSession>,
    logged_out: AtomicBool,
}

impl Panel {
    pub(crate) fn new(browser: Arc<BrowserSession>) -> Self {
        Self {
            browser,
            logged_out: AtomicBool::new(false),
        }
    }

    pub(crate) fn state(&self) -> PanelState {
        if !self.browser.is_installed() {
            PanelState::NoBrowser
        } else if !self.browser.has_profile() {
            PanelState::NeverLoggedIn
        } else if self.logged_out.load(Ordering::Relaxed) {
            PanelState::LoggedOut
        } else {
            PanelState::Ready
        }
    }

    /// Opens `login` in the browser's window for the owner to log in; the
    /// next reading tries again.
    pub(crate) fn open_login(&self, login: &str) -> Result<(), PlatformError> {
        self.browser.open_window(login)?;
        self.logged_out.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Reads the tables at `url` once a table is drawn, or the login page:
    /// without a browser, the profile never opened or the login gone, it
    /// says so instead.
    pub(crate) fn read(&self, url: &str) -> Result<PanelPage, PlatformError> {
        match self.state() {
            PanelState::NoBrowser => {
                return Err(PlatformError::Refused(
                    "não há Edge nem Chrome instalado neste computador".into(),
                ));
            }
            PanelState::NeverLoggedIn => return Err(PlatformError::NotConnected),
            PanelState::LoggedOut | PanelState::Ready => {}
        }
        let ready = |value: &Value| {
            PanelPage::deserialize(value)
                .is_ok_and(|page| page.is_login() || !page.tables.is_empty())
        };
        let value = self.browser.read(url, TABLES_SCRIPT, &ready)?;
        let page = PanelPage::deserialize(&value).map_err(|error| {
            PlatformError::Failed(format!(
                "a leitura do painel voltou sem as tabelas: {error}"
            ))
        })?;
        let logged_out = page.is_login();
        self.logged_out.store(logged_out, Ordering::Relaxed);
        if logged_out {
            return Err(PlatformError::Expired);
        }
        Ok(page)
    }

    /// Every sale `report` shows under the panel at `panel_url`: the report
    /// lists its own period, which the app cannot pick without clicking.
    pub(crate) fn conversions(
        &self,
        panel_url: &str,
        report: &SalesReport,
    ) -> Result<Vec<ReportedConversion>, PlatformError> {
        let page = self.read(&format!("{panel_url}{}", report.path))?;
        let (table, columns) = page
            .tables
            .iter()
            .find_map(|table| {
                report
                    .columns_of(&table.headers)
                    .map(|found| (table, found))
            })
            .ok_or_else(|| {
                PlatformError::Failed(
                    "o painel abriu, mas sem a tabela de vendas com as colunas esperadas".into(),
                )
            })?;
        report.conversions(table, &columns)
    }
}

/// What a column of a sales report holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Column {
    Sale,
    SoldAt,
    Product,
    Status,
    Commission,
    Tracking,
}

/// Without these a row is not a Commission the app can tell apart.
const REQUIRED: [Column; 5] = [
    Column::Sale,
    Column::SoldAt,
    Column::Product,
    Column::Status,
    Column::Commission,
];

/// A panel's report of sales and commissions: where it is under the
/// panel, the names each column may go by until the regression pass reads
/// the real ones, the one Program its sales are of, how far back it goes
/// and how long a commission still changes.
pub(crate) struct SalesReport {
    pub(crate) path: &'static str,
    pub(crate) columns: &'static [(Column, &'static [&'static str])],
    /// Its Platform, which has one Program for every sale.
    pub(crate) platform: AffiliatePlatform,
    pub(crate) history_days: i64,
    pub(crate) settle_days: i64,
}

impl SalesReport {
    pub(crate) fn history(&self) -> TimeDelta {
        TimeDelta::days(self.history_days)
    }

    pub(crate) fn settle_time(&self) -> TimeDelta {
        TimeDelta::days(self.settle_days)
    }

    /// The columns at their positions in `headers`, when every required one
    /// is there.
    fn columns_of(&self, headers: &[String]) -> Option<BTreeMap<Column, usize>> {
        let mut found = BTreeMap::new();
        for (index, header) in headers.iter().enumerate() {
            let header = folded(header);
            if let Some((column, _)) = self
                .columns
                .iter()
                .find(|(_, names)| names.iter().any(|name| folded(name) == header))
            {
                found.entry(*column).or_insert(index);
            }
        }
        REQUIRED
            .iter()
            .all(|column| found.contains_key(column))
            .then_some(found)
    }

    /// The rows as Conversions, one per sale id. A row without an id (a
    /// total) is left out; rows sharing an id, one per item, add up.
    fn conversions(
        &self,
        table: &PanelTable,
        columns: &BTreeMap<Column, usize>,
    ) -> Result<Vec<ReportedConversion>, PlatformError> {
        let mut sales: Vec<ReportedConversion> = Vec::new();
        for row in &table.rows {
            let cell = |column: Column| {
                columns
                    .get(&column)
                    .and_then(|&index| row.get(index))
                    .map(|text| text.trim())
                    .unwrap_or_default()
            };
            let key = cell(Column::Sale).trim_start_matches('#').trim();
            if key.is_empty() {
                continue;
            }
            let unreadable = |what: &str, text: &str| {
                PlatformError::Failed(format!(
                    "a venda {key} do painel tem {what} que o app não lê: \"{text}\""
                ))
            };
            let sold_at = sold_at(cell(Column::SoldAt))
                .ok_or_else(|| unreadable("uma data", cell(Column::SoldAt)))?;
            let commission = parse_amount(cell(Column::Commission))
                .ok_or_else(|| unreadable("uma comissão", cell(Column::Commission)))?;
            let platform_status = cell(Column::Status).to_owned();
            let product = cell(Column::Product).to_owned();
            if let Some(sale) = sales.iter_mut().find(|sale| sale.key == key) {
                sale.commission = Money::new(sale.commission.amount() + commission, Currency::Brl);
                sale.product = format!("{} + {product}", sale.product);
                sale.raw = with_row(&sale.raw, table, row);
                continue;
            }
            sales.push(ReportedConversion {
                key: key.to_owned(),
                program: self
                    .platform
                    .own_program()
                    .expect("a panel's Platform has one Program"),
                product,
                sold_at,
                commission: Money::new(commission, Currency::Brl),
                status: status(&platform_status),
                platform_status,
                status_changed_at: None,
                tracking: Some(cell(Column::Tracking))
                    .filter(|tracking| !tracking.is_empty())
                    .map(str::to_owned),
                raw: with_row("[]", table, row),
            });
        }
        Ok(sales)
    }
}

/// `raw`, a JSON list of rows by header, with `row` added.
fn with_row(raw: &str, table: &PanelTable, row: &[String]) -> String {
    let mut rows: Vec<Value> = serde_json::from_str(raw).unwrap_or_default();
    let named: Map<String, Value> = table
        .headers
        .iter()
        .zip(row)
        .map(|(header, cell)| (header.clone(), Value::String(cell.clone())))
        .collect();
    rows.push(Value::Object(named));
    Value::Array(rows).to_string()
}

/// The Commission's status from the one the panel shows, the most
/// decisive word first: a "pagamento pendente" is still pending. A status
/// the app does not know counts as pending, never as earned.
fn status(shown: &str) -> CommissionStatus {
    let shown = folded(shown);
    let has = |words: &[&str]| words.iter().any(|word| shown.contains(word));
    if has(&["CANCEL", "RECUS"]) {
        CommissionStatus::Cancelled
    } else if has(&["DEVOL", "ESTORN", "REEMBOLS", "REVERT"]) {
        CommissionStatus::Reversed
    } else if has(&["PEND", "PROCESS", "ANALISE", "AGUARD"]) {
        CommissionStatus::Pending
    } else if has(&["PAG"]) {
        CommissionStatus::Paid
    } else if has(&["APROV", "CONFIRM", "DISPON", "LIBERAD"]) {
        CommissionStatus::Approved
    } else {
        CommissionStatus::Pending
    }
}

/// When the sale happened, in Brasília time as the panel shows it: `21/10/2026`,
/// with or without the time, or `21 out. 2026`. A day alone is its start.
fn sold_at(text: &str) -> Option<Timestamp> {
    // Two-digit years first: `%Y` would read `26` as the year 26.
    const WITH_TIME: [&str; 4] = [
        "%d/%m/%y %H:%M",
        "%d/%m/%Y %H:%M:%S",
        "%d/%m/%Y %H:%M",
        "%d/%m/%Y às %H:%M",
    ];
    const DAY_ONLY: [&str; 2] = ["%d/%m/%y", "%d/%m/%Y"];
    let local = WITH_TIME
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
        .or_else(|| {
            DAY_ONLY
                .iter()
                .find_map(|format| NaiveDate::parse_from_str(text, format).ok())
                .or_else(|| day_with_month_name(text))
                .and_then(|day| day.and_hms_opt(0, 0, 0))
        })?;
    local
        .and_local_timezone(brasilia())
        .single()
        .map(|at| at.to_utc())
}

/// `21 out. 2026`, `21 de outubro de 2026`.
fn day_with_month_name(text: &str) -> Option<NaiveDate> {
    const MONTHS: [&str; 12] = [
        "JAN", "FEV", "MAR", "ABR", "MAI", "JUN", "JUL", "AGO", "SET", "OUT", "NOV", "DEZ",
    ];
    let words: Vec<String> = text
        .split(|c: char| c.is_whitespace() || c == '.')
        .map(folded)
        .filter(|word| !word.is_empty() && word != "DE")
        .collect();
    let [day, month, year] = words.as_slice() else {
        return None;
    };
    let month = MONTHS.iter().position(|name| month.starts_with(name))? as u32 + 1;
    NaiveDate::from_ymd_opt(year.parse().ok()?, month, day.parse().ok()?)
}
