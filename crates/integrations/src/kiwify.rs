//! Kiwify's adapter (#84): the owner's affiliate Commissions and the
//! withdrawals of the account's balance, read from the public API with the
//! Connection's client credentials, as a `CommissionSource` (ADR 0030). It
//! only reads.
//!
//! Kiwify filters sales by the date they last changed, so a Sync asks only
//! for what changed since the last one. Each query also needs the dates the
//! sales were made, at most 90 days apart: the adapter asks those in
//! windows going back as long as a sale still changes.

use std::collections::BTreeMap;
use std::ops::Range;
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, TimeDelta};
use mascate_affiliate::{
    AffiliatePlatform, CommissionSource, CommissionStatus, ReportedConversion, ReportedPayout,
    ReportedProgram,
};
use mascate_kernel::{Clock, Currency, Money, PlatformError, Timestamp};
use mascate_platform::{Secret, SecretStore, http_agent};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::answers::{decimal, read_json};
use crate::retry::{Attempt, PAST_A_MINUTE, Pause, ThreadPause, retrying};

/// The public API, unless a test points elsewhere.
pub const API_URL: &str = "https://public-api.kiwify.com";

const CLIENT_ID: &str = "KIWIFY_CLIENT_ID";
const CLIENT_SECRET: &str = "KIWIFY_CLIENT_SECRET";
const ACCOUNT_ID: &str = "KIWIFY_ACCOUNT_ID";

const PAGE_SIZE: usize = 100;

/// Renews this long before the access token expires, so a request never
/// leaves with a token about to lapse.
const RENEW_BEFORE: TimeDelta = TimeDelta::minutes(5);

/// A day short of the 90 Kiwify allows between the two dates of a query,
/// however it counts the ends.
const LONGEST_QUERY: TimeDelta = TimeDelta::days(89);

/// How long after the sale its status still changes: a chargeback can come
/// months after the purchase, past any guarantee.
const SALE_CHANGES_FOR: TimeDelta = TimeDelta::days(180);

/// Kiwify filters by the date of the last change, so the slack only covers
/// a clock or time zone that disagrees with the app's.
const SETTLE: TimeDelta = TimeDelta::days(1);

/// Kiwify documents no limit on how far back the sales go.
const HISTORY: TimeDelta = TimeDelta::days(5 * 365);

/// What Kiwify takes in its date filters.
const DATE_FORMAT: &str = "%Y-%m-%d %H:%M:%S%.3f";

/// The status of a withdrawal once the money left; any other is still
/// under review or was turned down.
const PAID_OUT: [&str; 3] = ["paid", "approved", "completed"];

/// Kiwify's public API, as the owner's Connection sees it.
pub struct Kiwify {
    agent: ureq::Agent,
    api: String,
    store: Arc<dyn SecretStore>,
    clock: Arc<dyn Clock>,
    pause: Arc<dyn Pause>,
    /// Held while renewing, so a second caller waits and reuses the result.
    token: Mutex<Option<AccessToken>>,
}

struct AccessToken {
    value: Secret,
    expires_at: Timestamp,
}

impl Kiwify {
    /// The public API at `api`, with the client credentials and the account
    /// id kept in `store`.
    pub fn new(
        api: &str,
        user_agent: &str,
        store: Arc<dyn SecretStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            agent: http_agent(api, user_agent, Duration::from_secs(30)),
            api: api.trim_end_matches('/').to_owned(),
            store,
            clock,
            pause: Arc::new(ThreadPause),
            token: Mutex::new(None),
        }
    }

    /// The same, waiting between retries with `pause`.
    pub fn with_pause(mut self, pause: Arc<dyn Pause>) -> Self {
        self.pause = pause;
        self
    }

    /// Every item Kiwify lists at `path` with `query`, page after page.
    fn every(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<serde_json::Value>, PlatformError> {
        let mut items = Vec::new();
        let mut last: Vec<serde_json::Value> = Vec::new();
        for page in 1.. {
            let mut asked = query.to_vec();
            asked.push(("page_size", PAGE_SIZE.to_string()));
            asked.push(("page_number", page.to_string()));
            let answer: Page = self.get(path, &asked)?;
            // A page that repeats the last one means Kiwify ignored the page
            // asked, and would answer it forever.
            if answer.data.is_empty() || answer.data == last {
                break;
            }
            // The page Kiwify served, which may be smaller than the one
            // asked; a page shorter than that is the last.
            let size = answer
                .pagination
                .and_then(|pagination| pagination.page_size)
                .filter(|&size| size > 0)
                .map_or(PAGE_SIZE, |size| size.min(PAGE_SIZE as u64) as usize);
            let full = answer.data.len() >= size;
            items.extend(answer.data.iter().cloned());
            if !full {
                break;
            }
            last = answer.data;
        }
        Ok(items)
    }

    /// The JSON answer to `GET path` with `query`. A token Kiwify turns down
    /// is renewed once before the credentials count as not accepted.
    fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, PlatformError> {
        let account = self.credential(ACCOUNT_ID)?;
        let mut renewed = false;
        loop {
            let token = self.access_token()?;
            match retrying(self.pause.as_ref(), PAST_A_MINUTE, || {
                self.getting(path, query, &token, &account)
            }) {
                Err(PlatformError::Expired) if !renewed => {
                    self.forget(&token);
                    renewed = true;
                }
                answer => return answer,
            }
        }
    }

    fn getting<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        token: &Secret,
        account: &Secret,
    ) -> Attempt<T> {
        let mut request = self
            .agent
            .get(format!("{}/v1{path}", self.api))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", token.expose()))
            .header("x-kiwify-account-id", account.expose().trim());
        for (key, value) in query {
            request = request.query(*key, value);
        }
        let response = match request.call() {
            Ok(response) => response,
            Err(error) => return Attempt::Again(PlatformError::Failed(error.to_string())),
        };
        match response.status().as_u16() {
            200 => Attempt::Done(read_json(response, "Kiwify")),
            401 => Attempt::Done(Err(PlatformError::Expired)),
            // Kiwify answers a token it no longer accepts with a 400 too;
            // only the error code tells it from a bad query.
            400 | 403 => Attempt::Done(Err(refusal(response))),
            404 => Attempt::Done(Err(PlatformError::NotFound)),
            429 => Attempt::Again(PlatformError::RateLimited),
            status @ 500..=599 => Attempt::Again(PlatformError::Failed(format!(
                "Kiwify answered {status} for {path}"
            ))),
            status => Attempt::Done(Err(PlatformError::Failed(format!(
                "Kiwify answered {status} for {path}"
            )))),
        }
    }

    /// A valid access token, asked for first when there is none or it is
    /// about to expire.
    fn access_token(&self) -> Result<Secret, PlatformError> {
        let mut current = self.token.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(token) = current
            .as_ref()
            .filter(|token| token.expires_at - RENEW_BEFORE > self.clock.now())
        {
            return Ok(token.value.clone());
        }
        let renewed = self.renew()?;
        let value = renewed.value.clone();
        *current = Some(renewed);
        Ok(value)
    }

    /// Drops `used` after Kiwify turned it down, so the next request asks
    /// for a new one. A token another request already replaced is left
    /// alone.
    fn forget(&self, used: &Secret) {
        let mut current = self.token.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_ref().is_some_and(|token| &token.value == used) {
            *current = None;
        }
    }

    fn renew(&self) -> Result<AccessToken, PlatformError> {
        let (client_id, client_secret) =
            (self.credential(CLIENT_ID)?, self.credential(CLIENT_SECRET)?);
        let answer: TokenAnswer = retrying(self.pause.as_ref(), PAST_A_MINUTE, || {
            let sent = self
                .agent
                .post(format!("{}/v1/oauth/token", self.api))
                .header("Accept", "application/json")
                .send_form([
                    ("client_id", client_id.expose().trim()),
                    ("client_secret", client_secret.expose().trim()),
                ]);
            let response = match sent {
                Ok(response) => response,
                Err(error) => return Attempt::Again(PlatformError::Failed(error.to_string())),
            };
            match response.status().as_u16() {
                200 => Attempt::Done(read_json(response, "Kiwify")),
                // Credentials Kiwify does not know, or deleted ones.
                400 | 401 | 403 => Attempt::Done(Err(PlatformError::Expired)),
                429 => Attempt::Again(PlatformError::RateLimited),
                status @ 500..=599 => Attempt::Again(PlatformError::Failed(format!(
                    "Kiwify answered {status} when asked for a token"
                ))),
                status => Attempt::Done(Err(PlatformError::Failed(format!(
                    "Kiwify answered {status} when asked for a token"
                )))),
            }
        })?;
        Ok(AccessToken {
            value: Secret::new(answer.access_token),
            expires_at: self.clock.now() + TimeDelta::seconds(answer.expires_in.seconds()),
        })
    }

    fn credential(&self, name: &str) -> Result<Secret, PlatformError> {
        match self.store.read(name) {
            Ok(Some(secret)) => Ok(secret),
            Ok(None) => Err(PlatformError::NotConnected),
            Err(error) => Err(PlatformError::Failed(error.to_string())),
        }
    }
}

impl CommissionSource for Kiwify {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Kiwify
    }

    fn history(&self) -> TimeDelta {
        HISTORY
    }

    fn settle_time(&self) -> TimeDelta {
        SETTLE
    }

    /// Kiwify filters by the date the sale last changed. Each window of
    /// changes is asked for the sales made in it and as long before it as a
    /// sale still changes.
    fn conversions(
        &self,
        during: Range<Timestamp>,
    ) -> Result<Vec<ReportedConversion>, PlatformError> {
        let mut sales = BTreeMap::new();
        for changed in windows(during) {
            for made in windows(changed.start - SALE_CHANGES_FOR..changed.end) {
                let query = [
                    ("start_date", date(made.start)),
                    ("end_date", date(made.end)),
                    ("updated_at_start_date", date(changed.start)),
                    ("updated_at_end_date", date(changed.end)),
                    ("view_full_sale_details", "true".to_owned()),
                ];
                for sale in self.every("/sales", &query)? {
                    if let Some(conversion) = conversion(sale) {
                        sales.insert(conversion.key.clone(), conversion);
                    }
                }
            }
        }
        Ok(sales.into_values().collect())
    }

    /// The withdrawals whose money left during `during`. Kiwify does not
    /// say which sales a withdrawal pays: the owner confirms those
    /// (ADR 0030).
    fn payouts(&self, during: Range<Timestamp>) -> Result<Vec<ReportedPayout>, PlatformError> {
        Ok(self
            .every("/payouts", &[])?
            .into_iter()
            .filter_map(payout)
            .filter(|payout| during.contains(&payout.paid_at))
            .collect())
    }
}

/// `during` cut into windows Kiwify accepts in one query, oldest first.
fn windows(during: Range<Timestamp>) -> Vec<Range<Timestamp>> {
    let mut windows = Vec::new();
    let mut start = during.start;
    while start < during.end {
        let end = (start + LONGEST_QUERY).min(during.end);
        windows.push(start..end);
        start = end;
    }
    windows
}

/// A moment as Kiwify's filters take it, in UTC.
fn date(at: Timestamp) -> String {
    at.format(DATE_FORMAT).to_string()
}

/// What Kiwify's refusal means: a token it no longer accepts, or a query
/// it turns down in its own words.
fn refusal(response: ureq::http::Response<ureq::Body>) -> PlatformError {
    let answer = read_json::<ErrorAnswer>(response, "Kiwify").ok();
    let error = answer
        .as_ref()
        .and_then(|answer| answer.error.as_deref())
        .unwrap_or_default();
    let message = answer
        .as_ref()
        .and_then(|answer| answer.message.as_deref())
        .map(str::trim)
        .filter(|message| !message.is_empty());
    if error == "auth_error"
        || message.is_some_and(|message| message.to_lowercase().contains("token"))
    {
        return PlatformError::Expired;
    }
    PlatformError::Refused(message.unwrap_or("refused").to_owned())
}

/// The owner's Commission on one sale, unless the owner is not its
/// affiliate or it is in a currency the app does not know.
fn conversion(mut raw: serde_json::Value) -> Option<ReportedConversion> {
    if let Some(sale) = raw.as_object_mut() {
        // The buyer, the partners and the card are no business of the
        // affiliate's records.
        for private in [
            "customer",
            "revenue_partners",
            "card_last_digits",
            "boleto_url",
        ] {
            sale.remove(private);
        }
        if let Some(share) = sale
            .get_mut("affiliate_commission")
            .and_then(serde_json::Value::as_object_mut)
        {
            share.remove("document");
        }
    }
    let sale: Sale = serde_json::from_value(raw.clone()).ok()?;
    // On a sale of the account's own product the affiliate share is someone
    // else's.
    if sale
        .sale_type
        .as_deref()
        .is_some_and(|kind| kind.to_lowercase().contains("producer"))
    {
        return None;
    }
    let cents = cents(&sale.affiliate_commission?.amount?)?;
    let currency = match sale.currency.as_deref().map(str::trim) {
        None | Some("") => Currency::Brl,
        Some(code) => Currency::from_code(&code.to_uppercase())?,
    };
    let product = sale.product?;
    let status = status(&sale.status);
    let status_changed_at = match status {
        CommissionStatus::Approved => sale.approved_date.as_deref().and_then(timestamp),
        CommissionStatus::Reversed => sale.refunded_at.as_deref().and_then(timestamp),
        _ => None,
    };
    let tracking = sale.tracking.and_then(|tracking| {
        [
            tracking.sck,
            tracking.src,
            tracking.s1,
            tracking.s2,
            tracking.s3,
        ]
        .into_iter()
        .flatten()
        .map(|marking| marking.trim().to_owned())
        .find(|marking| !marking.is_empty())
    });
    Some(ReportedConversion {
        key: sale.id,
        program: ReportedProgram {
            id: product.id,
            name: product.name.clone(),
        },
        product: product.name,
        sold_at: timestamp(&sale.created_at)?,
        commission: Money::new(cents / Decimal::ONE_HUNDRED, currency),
        status,
        platform_status: sale.status,
        status_changed_at,
        tracking,
        raw: raw.to_string(),
    })
}

/// The Commission's status for a sale in Kiwify's `status`. One Kiwify
/// adds later stays pending until the app learns it; the raw status is
/// kept either way.
fn status(status: &str) -> CommissionStatus {
    match status {
        "approved" | "paid" => CommissionStatus::Approved,
        "refunded" | "chargedback" => CommissionStatus::Reversed,
        "refused" => CommissionStatus::Cancelled,
        // waiting_payment, pending, processing, authorized,
        // refund_requested and pending_refund.
        _ => CommissionStatus::Pending,
    }
}

/// A withdrawal whose money left, in reais: Kiwify pays out only to
/// Brazilian accounts.
fn payout(raw: serde_json::Value) -> Option<ReportedPayout> {
    let read: Withdrawal = serde_json::from_value(raw.clone()).ok()?;
    if !PAID_OUT.contains(&read.status.trim().to_lowercase().as_str()) {
        return None;
    }
    let paid_at = read
        .updated_at
        .as_deref()
        .and_then(timestamp)
        .or_else(|| read.created_at.as_deref().and_then(timestamp))?;
    Some(ReportedPayout {
        key: read.id,
        paid_at,
        amount: Money::new(cents(&read.amount)? / Decimal::ONE_HUNDRED, Currency::Brl),
        covers: Vec::new(),
        raw: raw.to_string(),
    })
}

/// Amounts in cents, as a JSON number or as text; neither passes through a
/// float.
fn cents(value: &serde_json::Value) -> Option<Decimal> {
    match value {
        serde_json::Value::Number(number) => decimal(number),
        serde_json::Value::String(text) => Decimal::from_str(text.trim()).ok(),
        _ => None,
    }
}

/// Kiwify's dates: ISO 8601 with the zone, or its own `2020-07-10
/// 15:00:00.000` in UTC.
fn timestamp(text: &str) -> Option<Timestamp> {
    let text = text.trim();
    DateTime::parse_from_rfc3339(text)
        .map(|at| at.to_utc())
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
                .ok()
                .map(|at| at.and_utc())
        })
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    expires_in: Seconds,
}

/// The documentation gives `expires_in` as a number in the example and as
/// text in the schema.
#[derive(Deserialize)]
#[serde(untagged)]
enum Seconds {
    Number(i64),
    Text(String),
}

impl Seconds {
    fn seconds(&self) -> i64 {
        match self {
            Seconds::Number(seconds) => *seconds,
            // A token whose lifetime can't be read is renewed on the next
            // call rather than trusted.
            Seconds::Text(text) => text.trim().parse().unwrap_or(0),
        }
    }
}

#[derive(Deserialize)]
struct ErrorAnswer {
    error: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct Page {
    pagination: Option<Pagination>,
    #[serde(default)]
    data: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Pagination {
    page_size: Option<u64>,
}

#[derive(Deserialize)]
struct Sale {
    id: String,
    created_at: String,
    status: String,
    currency: Option<String>,
    product: Option<Product>,
    approved_date: Option<String>,
    refunded_at: Option<String>,
    sale_type: Option<String>,
    tracking: Option<Tracking>,
    affiliate_commission: Option<Share>,
}

#[derive(Deserialize)]
struct Product {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct Tracking {
    sck: Option<String>,
    src: Option<String>,
    s1: Option<String>,
    s2: Option<String>,
    s3: Option<String>,
}

#[derive(Deserialize)]
struct Share {
    amount: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Withdrawal {
    id: String,
    amount: serde_json::Value,
    status: String,
    created_at: Option<String>,
    updated_at: Option<String>,
}
