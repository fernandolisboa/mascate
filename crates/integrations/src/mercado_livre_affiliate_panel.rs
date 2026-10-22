//! The Mercado Livre affiliate panel reader (#160, ADR 0035 decision 4):
//! Mercado Livre offers affiliates no API, so the app opens the panel's
//! report in the installed browser, logged in by the owner, and reads the
//! sales table it shows into Conversions and their Commissions, keyed on
//! the panel's own sale ids. Built only while its flag is on.
//!
//! The report's address, its columns and its statuses come from public
//! descriptions of the panel, not from a real account: the phase 3
//! regression pass (#116) records the real page (see the fixtures' README).

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{NaiveDate, NaiveDateTime, TimeDelta};
use mascate_affiliate::{
    AffiliatePlatform, CommissionSource, CommissionStatus, ReportedConversion, ReportedProgram,
};
use mascate_kernel::{Currency, Money, PlatformError, Timestamp, brasilia, parse_amount};
use mascate_platform::{Flag, FlagKind, Phase};
use serde_json::{Map, Value};

use crate::affiliate_panel::{AffiliatePanel, Panel, PanelState, PanelTable, folded};
use crate::browser::BrowserSession;

/// The affiliate panel, where the owner logs in, unless a test points
/// elsewhere.
pub const PANEL_URL: &str = "https://www.mercadolivre.com.br/afiliados";

/// The panel's report of sales and commissions, under the panel.
const REPORT_PATH: &str = "/relatorios";

/// The reader, off until the owner turns it on (ADR 0006).
pub const MERCADO_LIVRE_AFFILIATE_PANEL: Flag = Flag {
    key: "integrations.mercado_livre_affiliate_panel",
    name: "Leitura do painel de afiliado do Mercado Livre",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "O Mercado Livre não tem API para afiliados, e os termos do programa de afiliados \
             não foram lidos (a página só abre com login); os Termos e condições de uso para \
             desenvolvedores (7.6) proíbem robôs. O app abre o painel no Edge ou no Chrome, num \
             perfil só do Mascate em que você entrou à mão, e só lê: não clica, não digita senha \
             e não resolve captcha.",
    risk: "O Mercado Livre pode suspender ou encerrar a sua conta de afiliado, e com ela as \
           comissões ainda não pagas.",
};

/// The one Program: Mercado Livre's affiliate program.
const PROGRAM_ID: &str = "mercado-livre-affiliates";
const PROGRAM_NAME: &str = "Afiliados Mercado Livre";

/// How far back the report goes, and how long a commission still changes:
/// the delivery, the buyer's days to give up and the monthly payment.
const HISTORY_DAYS: i64 = 90;
const SETTLE_DAYS: i64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Column {
    Sale,
    SoldAt,
    Product,
    Status,
    Commission,
    Tracking,
}

/// Each column with the names it may go by in the report, until the
/// regression pass reads the real ones.
const COLUMNS: [(Column, &[&str]); 6] = [
    (
        Column::Sale,
        &[
            "ID da venda",
            "Nº da venda",
            "Número da venda",
            "Venda",
            "ID do pedido",
            "Pedido",
        ],
    ),
    (Column::SoldAt, &["Data da venda", "Data da compra", "Data"]),
    (Column::Product, &["Produto", "Anúncio", "Item"]),
    (
        Column::Status,
        &["Status", "Status da comissão", "Situação"],
    ),
    (
        Column::Commission,
        &[
            "Comissão",
            "Sua comissão",
            "Valor da comissão",
            "Ganho",
            "Ganhos",
        ],
    ),
    (Column::Tracking, &["Etiqueta", "Tag", "Etiqueta do link"]),
];

/// Without these a row is not a Commission the app can tell apart.
const REQUIRED: [Column; 5] = [
    Column::Sale,
    Column::SoldAt,
    Column::Product,
    Column::Status,
    Column::Commission,
];

/// Mercado Livre's affiliate panel, read through the shared browser.
pub struct MercadoLivreAffiliatePanel {
    panel_url: String,
    panel: Panel,
}

impl MercadoLivreAffiliatePanel {
    /// The panel at `panel_url`, opened by `browser`.
    pub fn new(panel_url: &str, browser: Arc<BrowserSession>) -> Self {
        Self {
            panel_url: panel_url.trim_end_matches('/').to_owned(),
            panel: Panel::new(browser),
        }
    }
}

impl AffiliatePanel for MercadoLivreAffiliatePanel {
    fn state(&self) -> PanelState {
        self.panel.state()
    }

    /// Mercado Livre sends a logged-out browser from the panel to its login
    /// and back.
    fn open_login(&self) -> Result<(), PlatformError> {
        self.panel.open_login(&self.panel_url)
    }
}

impl CommissionSource for MercadoLivreAffiliatePanel {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::MercadoLivre
    }

    fn history(&self) -> TimeDelta {
        TimeDelta::days(HISTORY_DAYS)
    }

    fn settle_time(&self) -> TimeDelta {
        TimeDelta::days(SETTLE_DAYS)
    }

    /// Every sale the report shows: it lists its own period, which the app
    /// cannot pick without clicking, so `during` only bounds what the
    /// Affiliate module asks.
    fn conversions(
        &self,
        _during: std::ops::Range<Timestamp>,
    ) -> Result<Vec<ReportedConversion>, PlatformError> {
        let page = self
            .panel
            .read(&format!("{}{REPORT_PATH}", self.panel_url))?;
        let (table, columns) = page.table(columns_of).ok_or_else(|| {
            PlatformError::Failed(
                "o painel abriu, mas sem a tabela de vendas com as colunas esperadas".into(),
            )
        })?;
        conversions(table, &columns)
    }
}

/// The columns at their positions in `headers`, when every required one is
/// there.
fn columns_of(headers: &[String]) -> Option<BTreeMap<Column, usize>> {
    let mut found = BTreeMap::new();
    for (index, header) in headers.iter().enumerate() {
        let header = folded(header);
        if let Some((column, _)) = COLUMNS
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
            program: ReportedProgram {
                id: PROGRAM_ID.into(),
                name: PROGRAM_NAME.into(),
            },
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
