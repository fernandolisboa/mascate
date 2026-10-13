//! Eduzz's adapter (#85): the owner's affiliate Commissions and the
//! transfers of the account's balance, read from the Developer Hub API with
//! the owner's personal token, as a `CommissionSource` (ADR 0030). It only
//! reads.
//!
//! The financial statement is where Eduzz says what the owner earned: an
//! `affiliante_sale` entry (Eduzz's spelling) per Commission, a refund or
//! chargeback entry when it went back. The sales list adds the product,
//! the status and the tracking tags when it shows the sale. Both filter by
//! when something happened, so a Sync asks only for what changed since the
//! last one; a sale that changed without its Commission in that window is
//! looked up in the statement by its id.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeDelta, TimeZone as _};
use mascate_affiliate::{
    AffiliatePlatform, CommissionSource, CommissionStatus, ReportedConversion, ReportedPayout,
    ReportedProgram,
};
use mascate_kernel::{Currency, Money, PlatformError, Timestamp};
use mascate_platform::{Secret, SecretStore, http_agent};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::answers::{brasilia, decimal, numeric_id, read_json};
use crate::retry::{Attempt, PAST_A_MINUTE, Pause, ThreadPause, retrying};

/// The Developer Hub API, unless a test points elsewhere.
pub const API_URL: &str = "https://api.eduzz.com";

const PERSONAL_TOKEN: &str = "EDUZZ_PERSONAL_TOKEN";

const STATEMENT: &str = "/myeduzz/v2/financial/statement";
const SALES: &str = "/myeduzz/v1/sales";
const TRANSFERS: &str = "/myeduzz/v1/financial/transfers";

const PAGE_SIZE: u64 = 100;

/// The statement entry of an affiliate's Commission, as Eduzz spells it.
const COMMISSION: &str = "affiliante_sale";

/// The statement entries that take a Commission back. Eduzz documents the
/// chargeback as `chargerback_success`; the correct spelling is read too,
/// in case it fixes it.
const REVERSALS: [&str; 4] = [
    "refund",
    "chargeback_blocked",
    "chargerback_success",
    "chargeback_success",
];

/// Both lists filter by when something happened, so the slack only covers
/// a clock or time zone that disagrees with the app's.
const SETTLE: TimeDelta = TimeDelta::days(1);

/// Eduzz documents no limit on how far back the statement and the sales go.
const HISTORY: TimeDelta = TimeDelta::days(5 * 365);

/// How long after a sale last changed Eduzz may still credit its
/// Commission: the guarantee and the release of the money. The sales list
/// is read this far back, so a Commission credited now still finds its
/// sale.
const CREDIT_LAG: TimeDelta = TimeDelta::days(90);

/// What Eduzz's date filters take, in UTC.
const DATE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3fZ";

/// The status of a transfer once the money reached the owner's bank.
const CONFIRMED: &str = "confirmed";

/// Eduzz's Developer Hub API, as the owner's Connection sees it.
pub struct Eduzz {
    agent: ureq::Agent,
    api: String,
    store: Arc<dyn SecretStore>,
    pause: Arc<dyn Pause>,
}

impl Eduzz {
    /// The API at `api`, with the personal token kept in `store`.
    pub fn new(api: &str, user_agent: &str, store: Arc<dyn SecretStore>) -> Self {
        Self {
            agent: http_agent(api, user_agent, Duration::from_secs(30)),
            api: api.trim_end_matches('/').to_owned(),
            store,
            pause: Arc::new(ThreadPause),
        }
    }

    /// The same, waiting between retries with `pause`.
    pub fn with_pause(mut self, pause: Arc<dyn Pause>) -> Self {
        self.pause = pause;
        self
    }

    /// The statement entries credited during `during`, or every one of
    /// `sale` in it.
    fn statement(
        &self,
        during: &Range<Timestamp>,
        sale: Option<&str>,
    ) -> Result<Vec<Entry>, PlatformError> {
        let mut query = vec![
            ("startDate", date(during.start)),
            ("endDate", date(during.end)),
        ];
        if let Some(sale) = sale {
            query.push(("saleId", sale.to_owned()));
        }
        Ok(self
            .every(STATEMENT, &query)?
            .into_iter()
            .filter_map(Entry::read)
            .collect())
    }

    /// Every item Eduzz lists at `path` with `query`, page after page.
    fn every(&self, path: &str, query: &[(&str, String)]) -> Result<Vec<Value>, PlatformError> {
        let mut items = Vec::new();
        let mut last: Vec<Value> = Vec::new();
        for page in 1u64.. {
            let mut asked = query.to_vec();
            asked.push(("page", page.to_string()));
            asked.push(("itemsPerPage", PAGE_SIZE.to_string()));
            let answer: Page = self.get(path, &asked)?;
            // A page that repeats the last one means Eduzz ignored the page
            // asked, and would answer it forever.
            if answer.items.is_empty() || answer.items == last {
                break;
            }
            let more = answer
                .pages
                .map_or(answer.items.len() as u64 >= PAGE_SIZE, |pages| page < pages);
            items.extend(answer.items.iter().cloned());
            if !more {
                break;
            }
            last = answer.items;
        }
        Ok(items)
    }

    /// The JSON answer to `GET path` with `query`.
    fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, PlatformError> {
        let token = self.token()?;
        retrying(self.pause.as_ref(), PAST_A_MINUTE, || {
            self.getting(path, query, &token)
        })
    }

    fn getting<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        token: &Secret,
    ) -> Attempt<T> {
        let mut request = self
            .agent
            .get(format!("{}{path}", self.api))
            .header("Accept", "application/json")
            .header("Authorization", format!("bearer {}", token.expose()));
        for (key, value) in query {
            request = request.query(*key, value);
        }
        let response = match request.call() {
            Ok(response) => response,
            Err(error) => return Attempt::Again(PlatformError::Failed(error.to_string())),
        };
        match response.status().as_u16() {
            200 => Attempt::Done(read_json(response, "Eduzz")),
            // The personal token never expires: Eduzz turns it down once a
            // new one replaced it or the app was deleted.
            401 => Attempt::Done(Err(PlatformError::Expired)),
            // A scope the app lacks, or a query Eduzz turns down, in its
            // own words.
            400 | 403 | 422 => Attempt::Done(Err(refusal(response))),
            404 => Attempt::Done(Err(PlatformError::NotFound)),
            429 => Attempt::Again(PlatformError::RateLimited),
            status @ 500..=599 => Attempt::Again(PlatformError::Failed(format!(
                "Eduzz answered {status} for {path}"
            ))),
            status => Attempt::Done(Err(PlatformError::Failed(format!(
                "Eduzz answered {status} for {path}"
            )))),
        }
    }

    /// The personal token, as pasted with or without the `bearer` the
    /// documentation shows in front of it.
    fn token(&self) -> Result<Secret, PlatformError> {
        let pasted = match self.store.read(PERSONAL_TOKEN) {
            Ok(Some(secret)) => secret,
            Ok(None) => return Err(PlatformError::NotConnected),
            Err(error) => return Err(PlatformError::Failed(error.to_string())),
        };
        let token = pasted.expose().trim();
        let token = match token.get(..7) {
            Some(prefix) if prefix.eq_ignore_ascii_case("bearer ") => token[7..].trim_start(),
            _ => token,
        };
        Ok(Secret::new(token))
    }
}

impl CommissionSource for Eduzz {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Eduzz
    }

    fn history(&self) -> TimeDelta {
        HISTORY
    }

    fn settle_time(&self) -> TimeDelta {
        SETTLE
    }

    /// The sales whose Commission was credited or taken back during
    /// `during`, and the ones Eduzz says changed then, each with every
    /// statement entry it has and the sale when the list shows it.
    fn conversions(
        &self,
        during: Range<Timestamp>,
    ) -> Result<Vec<ReportedConversion>, PlatformError> {
        let earliest = during.end - HISTORY;
        let mut entries: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
        for entry in self.statement(&during, None)? {
            if entry.counts() {
                entries.entry(entry.sale.clone()).or_default().push(entry);
            }
        }
        let since = (during.start - CREDIT_LAG).max(earliest);
        let sales: BTreeMap<String, Sale> = self
            .every(
                SALES,
                &[
                    ("startDate", date(since)),
                    ("endDate", date(during.end)),
                    ("referenceDate", "updatedAt".to_owned()),
                ],
            )?
            .into_iter()
            .filter_map(Sale::read)
            .map(|sale| (sale.id.clone(), sale))
            .collect();

        // A sale without an affiliate pays no Commission, so a change to it
        // is not looked up.
        let changed = sales
            .values()
            .filter(|sale| {
                sale.affiliated && sale.updated_at.is_some_and(|at| during.contains(&at))
            })
            .map(|sale| sale.id.clone());
        let touched: BTreeSet<String> = entries.keys().cloned().chain(changed).collect();
        let mut conversions = Vec::new();
        for sale in touched {
            let mut read = entries.remove(&sale).unwrap_or_default();
            if !read.iter().any(Entry::is_commission) {
                read = self
                    .statement(&(earliest..during.end), Some(&sale))?
                    .into_iter()
                    .filter(|entry| entry.counts() && entry.sale == sale)
                    .collect();
            }
            if let Some(conversion) = conversion(&sale, read, sales.get(&sale)) {
                conversions.push(conversion);
            }
        }
        Ok(conversions)
    }

    /// The transfers that reached the owner's bank during `during`. Eduzz
    /// does not say which sales a transfer pays: the owner confirms those
    /// (ADR 0030).
    fn payouts(&self, during: Range<Timestamp>) -> Result<Vec<ReportedPayout>, PlatformError> {
        Ok(self
            .every(TRANSFERS, &[])?
            .into_iter()
            .filter_map(payout)
            .filter(|payout| during.contains(&payout.paid_at))
            .collect())
    }
}

/// A moment as Eduzz's filters take it.
fn date(at: Timestamp) -> String {
    at.format(DATE_FORMAT).to_string()
}

/// Eduzz's refusal in its own words.
fn refusal(response: ureq::http::Response<ureq::Body>) -> PlatformError {
    let message = read_json::<ErrorAnswer>(response, "Eduzz")
        .ok()
        .and_then(|answer| answer.message)
        .map(|message| message.trim().to_owned())
        .filter(|message| !message.is_empty());
    PlatformError::Refused(message.unwrap_or_else(|| "refused".to_owned()))
}

/// The owner's Commission on `sale`, from its statement entries and the
/// sale when the list shows it; none until Eduzz credits it, nor in a
/// currency the app does not know.
fn conversion(
    sale: &str,
    mut entries: Vec<Entry>,
    listed: Option<&Sale>,
) -> Option<ReportedConversion> {
    entries.sort_by(|a, b| (a.credited_at, &a.id).cmp(&(b.credited_at, &b.id)));
    entries.dedup_by(|a, b| a.id == b.id);
    let credits: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.is_commission())
        .collect();
    let first = credits.first()?;
    let currency = Currency::from_code(&first.currency)?;
    if credits
        .iter()
        .any(|credit| credit.currency != first.currency)
    {
        return None;
    }
    let commission = credits.iter().map(|credit| credit.amount).sum::<Decimal>();
    let reversal = entries.iter().rev().find(|entry| !entry.is_commission());

    let (status, platform_status, status_changed_at) = match (reversal, listed) {
        (Some(reversal), _) => (
            CommissionStatus::Reversed,
            reversal.kind.clone(),
            Some(reversal.credited_at),
        ),
        (None, Some(listed)) => {
            let status = status(&listed.status);
            let changed_at = match status {
                CommissionStatus::Approved => listed.paid_at.or(Some(first.credited_at)),
                CommissionStatus::Reversed => listed.refunded_at,
                _ => None,
            };
            (status, listed.status.clone(), changed_at)
        }
        // Eduzz credits a Commission once the buyer paid.
        (None, None) => (
            CommissionStatus::Approved,
            COMMISSION.to_owned(),
            Some(first.credited_at),
        ),
    };

    let (program, product, sold_at, tracking) =
        match listed.and_then(|listed| listed.product.as_ref().map(|product| (listed, product))) {
            Some((listed, product)) => (
                ReportedProgram {
                    id: product.id.clone(),
                    name: product.name.clone(),
                },
                product.name.clone(),
                listed.created_at,
                listed.tracking.clone(),
            ),
            // The statement names no product: its line is all the owner gets.
            None => (
                ReportedProgram {
                    id: "eduzz".to_owned(),
                    name: "Eduzz".to_owned(),
                },
                first
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("Venda {sale}")),
                first.credited_at,
                None,
            ),
        };
    let raw = json!({
        "sale": listed.map_or(Value::Null, |listed| listed.raw.clone()),
        "statement": entries.iter().map(|entry| entry.raw.clone()).collect::<Vec<_>>(),
    });
    Some(ReportedConversion {
        key: sale.to_owned(),
        program,
        product,
        sold_at,
        commission: Money::new(commission, currency),
        status,
        platform_status,
        status_changed_at,
        tracking,
        raw: raw.to_string(),
    })
}

/// The Commission's status for a sale in Eduzz's `status`. One Eduzz adds
/// later stays pending until the app learns it; the raw status is kept
/// either way.
fn status(status: &str) -> CommissionStatus {
    match status {
        "paid" => CommissionStatus::Approved,
        "refunded" => CommissionStatus::Reversed,
        "canceled" | "expired" | "duplicated" => CommissionStatus::Cancelled,
        // open, processing, waitingRefund, recovering, waitingPayment,
        // scheduled and negotiated.
        _ => CommissionStatus::Pending,
    }
}

/// A transfer that reached the owner's bank.
fn payout(mut raw: Value) -> Option<ReportedPayout> {
    let read: Transfer = serde_json::from_value(raw.clone()).ok()?;
    if read.status.trim() != CONFIRMED {
        return None;
    }
    let paid_at = [read.completed_at, read.requested_at, read.created_at]
        .into_iter()
        .flatten()
        .find_map(|at| timestamp(&at))?;
    // The bank account is no business of the affiliate's records.
    if let Some(transfer) = raw.as_object_mut() {
        transfer.remove("bankAccount");
    }
    Some(ReportedPayout {
        key: read.id,
        paid_at,
        amount: read.value.money()?,
        covers: Vec::new(),
        raw: raw.to_string(),
    })
}

/// An amount as a JSON number or as text; neither passes through a float.
fn amount(value: &Value) -> Option<Decimal> {
    match value {
        Value::Number(number) => decimal(number),
        Value::String(text) => Decimal::from_str(text.trim()).ok(),
        _ => None,
    }
}

/// An id Eduzz sends as a number or as text, as text.
fn id(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => Some(text.trim().to_owned()),
        _ => None,
    }
    .filter(|id| !id.is_empty())
}

/// Eduzz's dates: ISO 8601 with the zone, or a bare date or date and time
/// on Brasília's clock.
fn timestamp(text: &str) -> Option<Timestamp> {
    let text = text.trim();
    if let Ok(at) = DateTime::parse_from_rfc3339(text) {
        return Some(at.to_utc());
    }
    let local = NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f"))
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .ok()
                .and_then(|day| day.and_hms_opt(0, 0, 0))
        })?;
    brasilia()
        .from_local_datetime(&local)
        .single()
        .map(|at| at.to_utc())
}

/// One statement entry about a sale, with what Eduzz answered for it.
struct Entry {
    id: String,
    sale: String,
    kind: String,
    credited_at: Timestamp,
    description: Option<String>,
    amount: Decimal,
    currency: String,
    raw: Value,
}

impl Entry {
    fn read(raw: Value) -> Option<Self> {
        let read: StatementItem = serde_json::from_value(raw.clone()).ok()?;
        Some(Self {
            id: read.id,
            sale: read.sale_id.as_ref().and_then(id).unwrap_or_default(),
            kind: read.kind.trim().to_owned(),
            credited_at: timestamp(&read.credit_date)?,
            description: read
                .description
                .map(|description| description.trim().to_owned())
                .filter(|description| !description.is_empty()),
            amount: amount(&read.value.value)?,
            currency: read.value.currency.trim().to_uppercase(),
            raw,
        })
    }

    fn is_commission(&self) -> bool {
        self.kind == COMMISSION
    }

    /// An entry this adapter reads: a Commission or its reversal, of a sale.
    fn counts(&self) -> bool {
        !self.sale.is_empty() && (self.is_commission() || REVERSALS.contains(&self.kind.as_str()))
    }
}

/// A sale as the list shows it, with the answer kept without the buyer,
/// the payment link and the other people paid by it.
struct Sale {
    id: String,
    status: String,
    created_at: Timestamp,
    updated_at: Option<Timestamp>,
    paid_at: Option<Timestamp>,
    refunded_at: Option<Timestamp>,
    affiliated: bool,
    product: Option<Product>,
    tracking: Option<String>,
    raw: Value,
}

impl Sale {
    fn read(mut raw: Value) -> Option<Self> {
        if let Some(sale) = raw.as_object_mut() {
            for private in ["buyer", "recipient", "payment", "affiliates", "partners"] {
                sale.remove(private);
            }
            if let Some(refund) = sale.get_mut("refund").and_then(Value::as_object_mut) {
                refund.remove("motive");
                refund.remove("requesterName");
            }
        }
        let read: SaleItem = serde_json::from_value(raw.clone()).ok()?;
        let when = |text: Option<String>| text.as_deref().and_then(timestamp);
        Some(Self {
            id: id(&read.id)?,
            status: read.status.trim().to_owned(),
            created_at: timestamp(&read.created_at)?,
            updated_at: when(read.updated_at),
            paid_at: when(read.paid_at),
            refunded_at: when(read.refund.and_then(|refund| refund.refunded_at)),
            affiliated: read.affiliate.is_some(),
            product: read.product,
            tracking: [read.tracker, read.tracker2, read.tracker3]
                .into_iter()
                .flatten()
                .map(|tracker| tracker.trim().to_owned())
                .find(|tracker| !tracker.is_empty()),
            raw,
        })
    }
}

#[derive(Deserialize)]
struct ErrorAnswer {
    message: Option<String>,
}

#[derive(Deserialize)]
struct Page {
    pages: Option<u64>,
    #[serde(default)]
    items: Vec<Value>,
}

#[derive(Deserialize)]
struct Amount {
    currency: String,
    value: Value,
}

impl Amount {
    fn money(&self) -> Option<Money> {
        Some(Money::new(
            amount(&self.value)?,
            Currency::from_code(&self.currency.trim().to_uppercase())?,
        ))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatementItem {
    #[serde(deserialize_with = "numeric_id")]
    id: String,
    credit_date: String,
    description: Option<String>,
    sale_id: Option<Value>,
    #[serde(rename = "type")]
    kind: String,
    value: Amount,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaleItem {
    id: Value,
    status: String,
    created_at: String,
    updated_at: Option<String>,
    paid_at: Option<String>,
    refund: Option<Refund>,
    affiliate: Option<Value>,
    product: Option<Product>,
    tracker: Option<String>,
    tracker2: Option<String>,
    tracker3: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Refund {
    refunded_at: Option<String>,
}

#[derive(Deserialize)]
struct Product {
    #[serde(deserialize_with = "numeric_id")]
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Transfer {
    #[serde(deserialize_with = "numeric_id")]
    id: String,
    value: Amount,
    status: String,
    requested_at: Option<String>,
    completed_at: Option<String>,
    created_at: Option<String>,
}
