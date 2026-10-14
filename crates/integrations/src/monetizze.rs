//! Monetizze's adapter (#82): the owner's affiliate Commissions, read from
//! API 2.1 with the Connection's consumer key, as a `CommissionSource`
//! (ADR 0030). It only reads.
//!
//! Each sale lists every share of it in `comissoes`, the producer's
//! included; the owner's is the one with the account's e-mail. Sales are
//! asked once per status, so the status comes from Monetizze's own code and
//! not from the text it shows.

use std::collections::BTreeMap;
use std::ops::Range;
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{NaiveDateTime, TimeDelta};
use mascate_affiliate::{
    AffiliatePlatform, CommissionSource, CommissionStatus, ReportedConversion, ReportedProgram,
};
use mascate_kernel::{Clock, Currency, Money, PlatformError, Timestamp, brasilia};
use mascate_platform::{Secret, SecretStore, http_agent};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::answers::{decimal, numeric_id, read_json};
use crate::link_tags::UrlTagger;
use crate::retry::{Attempt, PAST_A_MINUTE, Pause, ThreadPause, retrying};

/// API 2.1, unless a test points elsewhere.
pub const API_URL: &str = "https://api.monetizze.com.br/2.1";

/// A Monetizze link carries the marking in `src`, read back as
/// `venda.src`.
pub const LINKS: UrlTagger = UrlTagger {
    platform: AffiliatePlatform::Monetizze,
    parameter: "src",
    domains: &["monetizze.com.br"],
    tag_ends_with: None,
};

const CONSUMER_KEY: &str = "MONETIZZE_CONSUMER_KEY";
const EMAIL: &str = "MONETIZZE_EMAIL";

/// Monetizze pages sales 100 at a time.
const PAGE_SIZE: usize = 100;

/// A token lapses 15 minutes after its last use; one idle this long is
/// renewed first, so a request never leaves with a token about to lapse.
const IDLE_TOKEN: TimeDelta = TimeDelta::minutes(14);

/// Each status of a sale, by Monetizze's code, and the Commission's status
/// for it.
const STATUSES: [(u8, CommissionStatus); 6] = [
    // Aguardando pagamento.
    (1, CommissionStatus::Pending),
    // Finalizada: paid, inside the guarantee.
    (2, CommissionStatus::Approved),
    // Cancelada.
    (3, CommissionStatus::Cancelled),
    // Devolvida.
    (4, CommissionStatus::Reversed),
    // Bloqueada: held for analysis, may still be paid.
    (5, CommissionStatus::Pending),
    // Completa: past the guarantee.
    (6, CommissionStatus::Approved),
];

/// The longest guarantee a producer gives, 30 days, plus the days a boleto
/// takes to be paid and cleared.
const SETTLE_DAYS: i64 = 35;

/// Monetizze documents no limit on how far back the sales go.
const HISTORY_DAYS: i64 = 5 * 365;

const DATE_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// Monetizze's API 2.1, as the owner's Connection sees it.
pub struct Monetizze {
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
    used_at: Timestamp,
}

impl Monetizze {
    /// API 2.1 at `api`, with the consumer key and the account's e-mail kept
    /// in `store`.
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

    /// Every sale Monetizze lists in status `code` registered during
    /// `during`, page after page.
    fn every(
        &self,
        code: u8,
        during: &Range<Timestamp>,
    ) -> Result<Vec<serde_json::Value>, PlatformError> {
        let mut sales = Vec::new();
        let mut last: Vec<serde_json::Value> = Vec::new();
        for page in 1.. {
            let query = [
                ("date_min", wall_clock(during.start)),
                ("date_max", wall_clock(during.end)),
                ("status[]", code.to_string()),
                ("page", page.to_string()),
            ];
            let answer: Answer = self.get("/transactions", &query)?;
            let found = answer.page()?;
            // A page that repeats the last one means Monetizze ignored the
            // page asked, and would answer it forever.
            if found.is_empty() || found == last {
                break;
            }
            let full = found.len() >= PAGE_SIZE;
            sales.extend(found.iter().cloned());
            if !full {
                break;
            }
            last = found;
        }
        Ok(sales)
    }

    /// The JSON answer to `GET path` with `query`. A token Monetizze turns
    /// down is renewed once before the key counts as not accepted.
    fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, PlatformError> {
        let mut renewed = false;
        loop {
            let token = self.access_token()?;
            match retrying(self.pause.as_ref(), PAST_A_MINUTE, || {
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
            .header("TOKEN", token.expose());
        for (key, value) in query {
            request = request.query(*key, value);
        }
        match request.call() {
            Ok(response) => answered(response, path),
            Err(error) => Attempt::Again(PlatformError::Failed(error.to_string())),
        }
    }

    /// A token Monetizze still accepts, asked for first when there is none
    /// or it sat idle long enough to lapse. Using it keeps it alive.
    fn access_token(&self) -> Result<Secret, PlatformError> {
        let mut current = self.token.lock().unwrap_or_else(PoisonError::into_inner);
        let now = self.clock.now();
        let token = match current
            .take()
            .filter(|token| now - token.used_at < IDLE_TOKEN)
        {
            Some(token) => token.value,
            None => self.renew()?,
        };
        *current = Some(AccessToken {
            value: token.clone(),
            used_at: now,
        });
        Ok(token)
    }

    /// Drops `used` after Monetizze turned it down, so the next request asks
    /// for a new one. A token another request already replaced is left
    /// alone.
    fn forget(&self, used: &Secret) {
        let mut current = self.token.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_ref().is_some_and(|token| &token.value == used) {
            *current = None;
        }
    }

    fn renew(&self) -> Result<Secret, PlatformError> {
        let key = self.credential(CONSUMER_KEY)?;
        let answer: TokenAnswer = retrying(self.pause.as_ref(), PAST_A_MINUTE, || {
            match self
                .agent
                .get(format!("{}/token", self.api))
                .header("Accept", "application/json")
                .header("X_CONSUMER_KEY", key.expose())
                .call()
            {
                Ok(response) => answered(response, "/token"),
                Err(error) => Attempt::Again(PlatformError::Failed(error.to_string())),
            }
        })?;
        Ok(Secret::new(answer.token))
    }

    fn credential(&self, name: &str) -> Result<Secret, PlatformError> {
        match self.store.read(name) {
            Ok(Some(secret)) => Ok(secret),
            Ok(None) => Err(PlatformError::NotConnected),
            Err(error) => Err(PlatformError::Failed(error.to_string())),
        }
    }
}

impl CommissionSource for Monetizze {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Monetizze
    }

    fn history(&self) -> TimeDelta {
        TimeDelta::days(HISTORY_DAYS)
    }

    fn settle_time(&self) -> TimeDelta {
        TimeDelta::days(SETTLE_DAYS)
    }

    /// Monetizze filters by the date the sale was registered. A sale whose
    /// status changed between two of the readings is kept in the later one.
    fn conversions(
        &self,
        during: Range<Timestamp>,
    ) -> Result<Vec<ReportedConversion>, PlatformError> {
        let email = self.credential(EMAIL)?;
        let mut sales = BTreeMap::new();
        for (code, status) in STATUSES {
            for sale in self.every(code, &during)? {
                if let Some(conversion) = conversion(sale, code, status, email.expose()) {
                    sales.insert(conversion.key.clone(), conversion);
                }
            }
        }
        Ok(sales.into_values().collect())
    }
}

/// What Monetizze's answer to `path` means.
fn answered<T: DeserializeOwned>(
    response: ureq::http::Response<ureq::Body>,
    path: &str,
) -> Attempt<T> {
    match response.status().as_u16() {
        200 => Attempt::Done(read_json(response, "Monetizze")),
        401 => Attempt::Done(Err(PlatformError::Expired)),
        // An unknown key and a lapsed token both answer 403, as does a key
        // without permission for the method; only the message tells them
        // apart.
        403 => Attempt::Done(Err(refusal(response))),
        404 => Attempt::Done(Err(PlatformError::NotFound)),
        429 => Attempt::Again(PlatformError::RateLimited),
        status @ 500..=599 => Attempt::Again(PlatformError::Failed(format!(
            "Monetizze answered {status} for {path}"
        ))),
        status => Attempt::Done(Err(PlatformError::Failed(format!(
            "Monetizze answered {status} for {path}"
        )))),
    }
}

/// A 403 about the key or the token means the login is not accepted; any
/// other is a refusal with Monetizze's own words.
fn refusal(response: ureq::http::Response<ureq::Body>) -> PlatformError {
    let said = read_json::<ErrorAnswer>(response, "Monetizze")
        .ok()
        .and_then(|answer| answer.error)
        .map(|said| said.trim().to_owned())
        .filter(|said| !said.is_empty());
    let Some(said) = said else {
        return PlatformError::Refused("refused".into());
    };
    let lower = said.to_lowercase();
    if ["token", "chave", "credenciais", "consumer key"]
        .iter()
        .any(|about| lower.contains(about))
    {
        PlatformError::Expired
    } else {
        PlatformError::Refused(said)
    }
}

/// The owner's Commission on one sale in status `code`, unless the sale
/// brings no share of the owner's or one in a currency the app does not
/// know.
fn conversion(
    mut raw: serde_json::Value,
    code: u8,
    status: CommissionStatus,
    email: &str,
) -> Option<ReportedConversion> {
    // The buyer is no business of the affiliate's records.
    if let Some(sale) = raw
        .get_mut("venda")
        .and_then(serde_json::Value::as_object_mut)
    {
        sale.remove("comprador");
    }
    if let Some(item) = raw.as_object_mut() {
        item.remove("comprador");
    }
    let item: Item = serde_json::from_value(raw.clone()).ok()?;
    let sale = item.venda;
    let product = sale.produto.or(item.produto)?;
    let shares = sale.comissoes.or(item.comissoes).unwrap_or_default();
    let share = owners_share(&shares, email)?;
    let currency = match sale.moeda.as_deref().map(str::trim) {
        // Sales from before Monetizze sold abroad bring no currency.
        None | Some("") => Currency::Brl,
        Some(code) => Currency::from_code(code)?,
    };
    let approved = status == CommissionStatus::Approved;
    Some(ReportedConversion {
        key: sale.codigo,
        program: ReportedProgram {
            id: product.codigo,
            name: product.nome.clone(),
        },
        product: product.nome,
        sold_at: timestamp(&sale.data_inicio)?,
        commission: Money::new(amount(&share.valor)?, currency),
        status,
        platform_status: sale
            .status
            .map(|shown| shown.trim().to_owned())
            .filter(|shown| !shown.is_empty())
            .unwrap_or_else(|| code.to_string()),
        status_changed_at: sale
            .data_finalizada
            .as_deref()
            .filter(|_| approved)
            .and_then(timestamp),
        tracking: sale
            .src
            .map(|marking| marking.trim().to_owned())
            .filter(|marking| !marking.is_empty()),
        raw: raw.to_string(),
    })
}

/// The owner's row among a sale's shares: the one with the account's
/// e-mail or, when no row brings an e-mail that matches, the only affiliate
/// one. A sale with several affiliates and none of them the owner's e-mail
/// is not the owner's to count.
fn owners_share<'a>(shares: &'a [Share], email: &str) -> Option<&'a Share> {
    let email = email.trim();
    if let Some(mine) = shares.iter().find(|share| {
        share
            .email
            .as_deref()
            .is_some_and(|theirs| theirs.trim().eq_ignore_ascii_case(email))
    }) {
        return Some(mine);
    }
    let mut affiliates = shares.iter().filter(|share| {
        share
            .tipo_comissao
            .as_deref()
            .is_some_and(|kind| kind.trim().to_lowercase().starts_with("afiliado"))
    });
    match (affiliates.next(), affiliates.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

/// A date in Monetizze's format, on Brasília's clock. PHP's empty date,
/// `0000-00-00 00:00:00`, is none.
fn timestamp(text: &str) -> Option<Timestamp> {
    NaiveDateTime::parse_from_str(text.trim(), DATE_FORMAT)
        .ok()?
        .and_local_timezone(brasilia())
        .single()
        .map(|local| local.to_utc())
}

fn wall_clock(at: Timestamp) -> String {
    at.with_timezone(&brasilia())
        .format(DATE_FORMAT)
        .to_string()
}

/// Amounts come as decimal strings, sometimes as JSON numbers; neither
/// passes through a float.
fn amount(value: &serde_json::Value) -> Option<Decimal> {
    match value {
        serde_json::Value::String(text) => Decimal::from_str(text.trim()).ok(),
        serde_json::Value::Number(number) => decimal(number),
        _ => None,
    }
}

#[derive(Deserialize)]
struct TokenAnswer {
    #[serde(rename = "TOKEN")]
    token: String,
}

#[derive(Deserialize)]
struct ErrorAnswer {
    #[serde(rename = "Error")]
    error: Option<String>,
}

/// The documentation shows the answer wrapped in a list; either way is read.
#[derive(Deserialize)]
#[serde(untagged)]
enum Answer {
    Page(Page),
    Wrapped(Vec<Page>),
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    dados: Vec<serde_json::Value>,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

impl Answer {
    /// The sales on the page, unless Monetizze says the query failed.
    fn page(self) -> Result<Vec<serde_json::Value>, PlatformError> {
        let page = match self {
            Answer::Page(page) => page,
            Answer::Wrapped(pages) => match pages.into_iter().next() {
                Some(page) => page,
                None => return Ok(Vec::new()),
            },
        };
        match page
            .error
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|error| !error.is_empty())
        {
            Some(error) => Err(PlatformError::Refused(error.to_owned())),
            None => Ok(page.dados),
        }
    }
}

/// One sale as `/transactions` lists it. The field list puts the product
/// and the shares inside `venda`; the example, beside it. Both are read.
#[derive(Deserialize)]
struct Item {
    venda: Sale,
    produto: Option<Product>,
    comissoes: Option<Vec<Share>>,
}

#[derive(Deserialize)]
struct Sale {
    #[serde(deserialize_with = "numeric_id")]
    codigo: String,
    #[serde(rename = "dataInicio")]
    data_inicio: String,
    #[serde(rename = "dataFinalizada")]
    data_finalizada: Option<String>,
    status: Option<String>,
    moeda: Option<String>,
    src: Option<String>,
    produto: Option<Product>,
    comissoes: Option<Vec<Share>>,
}

#[derive(Deserialize)]
struct Product {
    #[serde(deserialize_with = "numeric_id")]
    codigo: String,
    nome: String,
}

#[derive(Deserialize)]
struct Share {
    email: Option<String>,
    tipo_comissao: Option<String>,
    valor: serde_json::Value,
}
