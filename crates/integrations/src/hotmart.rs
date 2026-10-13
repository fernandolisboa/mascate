//! Hotmart's adapter (#78): the owner's affiliate Commissions, read from the
//! sales API with the Connection's client credentials, as a
//! `CommissionSource` (ADR 0030). It only reads.
//!
//! A sale's status, dates and link marking come from the sales history and
//! the owner's share from the sales commissions, joined by the transaction.
//! Without a status filter Hotmart answers only APPROVED and COMPLETE, so
//! both are asked once per status, or reversals would never be seen.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, TimeDelta};
use mascate_affiliate::{
    AffiliatePlatform, CommissionSource, CommissionStatus, ReportedConversion, ReportedProgram,
};
use mascate_kernel::{Clock, Currency, Money, PlatformError, Timestamp};
use mascate_platform::{Secret, SecretStore, http_agent};
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::answers::{numeric_id, read_json};
use crate::retry::{Attempt, Backoff, Pause, ThreadPause, retrying};

/// The sales API, unless a test points elsewhere.
pub const API_URL: &str = "https://developers.hotmart.com";

/// Where Hotmart hands out access tokens.
pub const AUTH_URL: &str = "https://api-sec-vlc.hotmart.com";

const CLIENT_ID: &str = "HOTMART_CLIENT_ID";
const CLIENT_SECRET: &str = "HOTMART_CLIENT_SECRET";
const BASIC: &str = "HOTMART_BASIC";

const HISTORY: &str = "/payments/api/v1/sales/history";
const COMMISSIONS: &str = "/payments/api/v1/sales/commissions";

/// Hotmart's default page; its pages of sales stay well under the answer
/// size limit.
const PAGE_SIZE: u32 = 100;

/// Renews this long before the access token expires, so a request never
/// leaves with a token about to lapse.
const RENEW_BEFORE: TimeDelta = TimeDelta::minutes(5);

/// The limit is 500 calls a minute, so a refused request waits 5, 10, 20
/// and then 40 seconds, past the minute, before giving up.
const BACKOFF: Backoff = Backoff {
    first: Duration::from_secs(5),
    retries: 4,
};

/// Every status a Hotmart sale takes, from its documentation.
const STATUSES: [&str; 17] = [
    "WAITING_PAYMENT",
    "PRINTED_BILLET",
    "STARTED",
    "PROCESSING_TRANSACTION",
    "UNDER_ANALISYS",
    "PRE_ORDER",
    "OVERDUE",
    "BLOCKED",
    "APPROVED",
    "COMPLETE",
    "REFUNDED",
    "PARTIALLY_REFUNDED",
    "CHARGEBACK",
    "PROTESTED",
    "CANCELLED",
    "EXPIRED",
    "NO_FUNDS",
];

/// The longest guarantee a producer can give (7, 15, 21 or 30 days) plus
/// the days a boleto takes to be paid and cleared: past it, a sale's
/// status no longer changes but for a rare late chargeback.
const SETTLE_DAYS: i64 = 35;

/// Hotmart documents no limit on how far back the sales go.
const HISTORY_DAYS: i64 = 5 * 365;

/// Hotmart's sales API, as the owner's Connection sees it.
pub struct Hotmart {
    agent: ureq::Agent,
    api: String,
    auth: String,
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

impl Hotmart {
    /// The sales API at `api`, asking tokens of `auth`, with the client
    /// credentials kept in `store`.
    pub fn new(
        api: &str,
        auth: &str,
        user_agent: &str,
        store: Arc<dyn SecretStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            agent: http_agent(api, user_agent, Duration::from_secs(30)),
            api: api.trim_end_matches('/').to_owned(),
            auth: auth.trim_end_matches('/').to_owned(),
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

    /// Every item Hotmart lists at `path` for the sales in `status` during
    /// `during`, page after page.
    fn every(
        &self,
        path: &str,
        status: &str,
        during: &Range<Timestamp>,
    ) -> Result<Vec<serde_json::Value>, PlatformError> {
        let mut items = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut query = vec![
                ("commission_as", "AFFILIATE".to_owned()),
                ("transaction_status", status.to_owned()),
                ("start_date", during.start.timestamp_millis().to_string()),
                ("end_date", during.end.timestamp_millis().to_string()),
                ("max_results", PAGE_SIZE.to_string()),
            ];
            if let Some(page_token) = &page_token {
                query.push(("page_token", page_token.clone()));
            }
            let page: Page = self.get(path, &query)?;
            items.extend(page.items);
            match page
                .page_info
                .and_then(|info| info.next_page_token)
                .filter(|next| !next.is_empty())
            {
                // A token that repeats would read the same page forever.
                Some(next) if page_token.as_ref() != Some(&next) => page_token = Some(next),
                _ => return Ok(items),
            }
        }
    }

    /// The JSON answer to `GET path` with `query`. A token Hotmart turns
    /// down is renewed once before the credentials count as expired.
    fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, PlatformError> {
        let mut renewed = false;
        loop {
            let token = self.access_token()?;
            match retrying(self.pause.as_ref(), BACKOFF, || {
                self.getting(path, query, &token)
            }) {
                Err(PlatformError::Expired) if !renewed => {
                    self.forget(&token);
                    renewed = true;
                }
                answer => return answer,
            }
        }
    }

    fn getting<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        token: &Secret,
    ) -> Attempt<T> {
        let mut request = self
            .agent
            .get(format!("{}{path}", self.api))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", token.expose()));
        for (key, value) in query {
            request = request.query(*key, value);
        }
        let response = match request.call() {
            Ok(response) => response,
            Err(error) => return Attempt::Again(PlatformError::Failed(error.to_string())),
        };
        match response.status().as_u16() {
            200 => Attempt::Done(read_json(response, "Hotmart")),
            401 => Attempt::Done(Err(PlatformError::Expired)),
            400 | 403 => Attempt::Done(Err(PlatformError::Refused(refusal(response)))),
            404 => Attempt::Done(Err(PlatformError::NotFound)),
            429 => Attempt::Again(PlatformError::RateLimited),
            status @ 500..=599 => {
                Attempt::Again(PlatformError::Failed(format!("Hotmart answered {status}")))
            }
            status => Attempt::Done(Err(PlatformError::Failed(format!(
                "Hotmart answered {status} for {path}"
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

    /// Drops `used` after Hotmart turned it down, so the next request asks
    /// for a new one. A token another request already replaced is left
    /// alone.
    fn forget(&self, used: &Secret) {
        let mut current = self.token.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_ref().is_some_and(|token| &token.value == used) {
            *current = None;
        }
    }

    fn renew(&self) -> Result<AccessToken, PlatformError> {
        let read = |name| match self.store.read(name) {
            Ok(Some(secret)) => Ok(secret),
            Ok(None) => Err(PlatformError::NotConnected),
            Err(error) => Err(PlatformError::Failed(error.to_string())),
        };
        let (client_id, client_secret, basic) =
            (read(CLIENT_ID)?, read(CLIENT_SECRET)?, read(BASIC)?);
        let authorization = format!("Basic {}", basic_token(basic.expose()));
        let answer: TokenAnswer = retrying(self.pause.as_ref(), BACKOFF, || {
            let sent = self
                .agent
                .post(format!("{}/security/oauth/token", self.auth))
                .query("grant_type", "client_credentials")
                .query("client_id", client_id.expose())
                .query("client_secret", client_secret.expose())
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .header("Authorization", &authorization)
                .send_empty();
            let response = match sent {
                Ok(response) => response,
                Err(error) => return Attempt::Again(PlatformError::Failed(error.to_string())),
            };
            match response.status().as_u16() {
                200 => Attempt::Done(read_json(response, "Hotmart")),
                // Credentials Hotmart does not know, or deleted ones.
                400 | 401 | 403 => Attempt::Done(Err(PlatformError::Expired)),
                429 => Attempt::Again(PlatformError::RateLimited),
                status @ 500..=599 => Attempt::Again(PlatformError::Failed(format!(
                    "Hotmart answered {status} when asked for a token"
                ))),
                status => Attempt::Done(Err(PlatformError::Failed(format!(
                    "Hotmart answered {status} when asked for a token"
                )))),
            }
        })?;
        Ok(AccessToken {
            value: Secret::new(answer.access_token),
            expires_at: self.clock.now() + TimeDelta::seconds(answer.expires_in),
        })
    }
}

impl CommissionSource for Hotmart {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Hotmart
    }

    fn history(&self) -> TimeDelta {
        TimeDelta::days(HISTORY_DAYS)
    }

    fn settle_time(&self) -> TimeDelta {
        TimeDelta::days(SETTLE_DAYS)
    }

    /// Hotmart filters by the date of the order. A sale whose status
    /// changed between the two readings, so that its share was not read, is
    /// left for the next Sync.
    fn conversions(
        &self,
        during: Range<Timestamp>,
    ) -> Result<Vec<ReportedConversion>, PlatformError> {
        let mut sales = BTreeMap::new();
        let mut shares = HashMap::new();
        for status in STATUSES {
            for mut sale in self.every(HISTORY, status, &during)? {
                // The buyer is no business of the affiliate's records.
                if let Some(sale) = sale.as_object_mut() {
                    sale.remove("buyer");
                }
                if let Some(transaction) = text_at(&sale, "/purchase/transaction") {
                    sales.insert(transaction, sale);
                }
            }
            for share in self.every(COMMISSIONS, status, &during)? {
                if let Some(transaction) = text_at(&share, "/transaction") {
                    shares.insert(transaction, share);
                }
            }
        }
        Ok(sales
            .into_iter()
            .filter_map(|(transaction, sale)| {
                conversion(transaction.clone(), sale, shares.remove(&transaction)?)
            })
            .collect())
    }
}

/// What the owner's Commission is for one sale, unless the sale brings no
/// affiliate share or one in a currency the app does not know.
fn conversion(
    transaction: String,
    sale: serde_json::Value,
    share: serde_json::Value,
) -> Option<ReportedConversion> {
    let read: Sale = serde_json::from_value(sale.clone()).ok()?;
    let shared: Shares = serde_json::from_value(share.clone()).ok()?;
    let commission = shared
        .commissions
        .into_iter()
        .find(|commission| commission.source == "AFFILIATE")?
        .commission;
    let commission = Money::new(
        decimal(&commission.value)?,
        Currency::from_code(commission.currency_value.trim())?,
    );
    let purchase = read.purchase;
    let status_changed_at = match purchase.status.as_str() {
        "APPROVED" => purchase.approved_date.and_then(millis),
        // Complete is approved past the guarantee.
        "COMPLETE" => purchase.warranty_expire_date.and_then(millis),
        _ => None,
    };
    let tracking = purchase.tracking.and_then(|tracking| {
        [tracking.source_sck, tracking.source]
            .into_iter()
            .flatten()
            .map(|marking| marking.trim().to_owned())
            .find(|marking| !marking.is_empty())
    });
    Some(ReportedConversion {
        key: transaction,
        program: ReportedProgram {
            id: read.product.id,
            name: read.product.name.clone(),
        },
        product: read.product.name,
        sold_at: millis(purchase.order_date)?,
        commission,
        status: status(&purchase.status),
        platform_status: purchase.status,
        status_changed_at,
        tracking,
        raw: serde_json::json!({ "sale": sale, "commission": share }).to_string(),
    })
}

/// The Commission's status for a sale in Hotmart's `status`. One Hotmart
/// adds later stays pending until the app learns it; the raw status is
/// kept either way.
fn status(status: &str) -> CommissionStatus {
    match status {
        "APPROVED" | "COMPLETE" => CommissionStatus::Approved,
        "REFUNDED" | "PARTIALLY_REFUNDED" | "CHARGEBACK" | "PROTESTED" => {
            CommissionStatus::Reversed
        }
        "CANCELLED" | "EXPIRED" | "NO_FUNDS" => CommissionStatus::Cancelled,
        _ => CommissionStatus::Pending,
    }
}

/// The Basic token as Hotmart's credentials page shows it, with or without
/// the word `Basic` in front.
fn basic_token(pasted: &str) -> &str {
    let pasted = pasted.trim();
    match pasted.get(..6) {
        Some(word) if word.eq_ignore_ascii_case("basic ") => pasted[6..].trim_start(),
        _ => pasted,
    }
}

fn text_at(value: &serde_json::Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Hotmart's dates, in milliseconds since 1970 UTC.
fn millis(millis: i64) -> Option<Timestamp> {
    DateTime::from_timestamp_millis(millis)
}

/// Amounts come as JSON numbers; their text, not a float, becomes the
/// decimal.
fn decimal(number: &serde_json::Number) -> Option<Decimal> {
    let text = number.to_string();
    Decimal::from_str(&text)
        .or_else(|_| Decimal::from_scientific(&text))
        .ok()
}

/// What Hotmart says when it refuses a request, when it says.
fn refusal(response: ureq::http::Response<ureq::Body>) -> String {
    read_json::<ErrorAnswer>(response, "Hotmart")
        .ok()
        .and_then(|answer| answer.error_description.or(answer.message).or(answer.error))
        .unwrap_or_else(|| "refused".into())
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    expires_in: i64,
}

#[derive(Deserialize)]
struct ErrorAnswer {
    error: Option<String>,
    error_description: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    items: Vec<serde_json::Value>,
    page_info: Option<PageInfo>,
}

#[derive(Deserialize)]
struct PageInfo {
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct Sale {
    product: Product,
    purchase: Purchase,
}

#[derive(Deserialize)]
struct Product {
    #[serde(deserialize_with = "numeric_id")]
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct Purchase {
    status: String,
    order_date: i64,
    approved_date: Option<i64>,
    warranty_expire_date: Option<i64>,
    tracking: Option<Tracking>,
}

#[derive(Deserialize)]
struct Tracking {
    source_sck: Option<String>,
    source: Option<String>,
}

#[derive(Deserialize)]
struct Shares {
    #[serde(default)]
    commissions: Vec<Share>,
}

#[derive(Deserialize)]
struct Share {
    commission: ShareValue,
    #[serde(default)]
    source: String,
}

#[derive(Deserialize)]
struct ShareValue {
    value: serde_json::Number,
    currency_value: String,
}
