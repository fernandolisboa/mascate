//! Shopee's seller adapter (#129): the owner's shop through the Shopee Open
//! Platform v2, as commerce's Sales Channel (ADR 0033). It reads the
//! listings, one per model (ADR 0014), sends a price once the owner
//! approves it and the stock that follows the app's Stock Share, and
//! unlists and relists an item; it reads the Orders with their returns
//! (#130), and their Fees and money from the escrow; it arranges an Order's
//! shipment and prints its label (#131); it publishes a draft, with its
//! variations as models (#132). Every call is signed with
//! HMAC-SHA256 of the partner key; the shop's login comes from Shopee's
//! authorization page and its refresh token is single use. Shopee has no fee
//! estimate, so the sale fee is its published table. Separate from Shopee
//! Afiliados, which is another API with other keys.

mod answers;
mod escrow;
mod labels;
mod orders;
mod publisher;
mod sales_channel;

use std::sync::Arc;
use std::time::Duration;

use chrono::TimeDelta;
use hmac::{Hmac, Mac};
use mascate_kernel::{Clock, PlatformError};
use mascate_platform::{Secret, SecretStore, http_agent};
use serde::de::DeserializeOwned;
use sha2::Sha256;

use crate::answers::MAX_DOCUMENT_BYTES;
use crate::escaped::escaped;
use crate::retry::{Attempt, Backoff, Pause, ThreadPause, retrying};
use crate::single_use_tokens::{Fresh, Tokens};
use answers::{Envelope, TokenAnswer};

/// Shopee's production host for Open API calls from the Americas, Brazil
/// included, unless a test or a development build points elsewhere.
pub const API_URL: &str = "https://openplatform.shopee.com.br";

/// Shopee's sandbox, for a development build with test keys.
pub const SANDBOX_URL: &str = "https://openplatform.sandbox.test-stable.shopee.sg";

const PARTNER_ID: &str = "SHOPEE_PARTNER_ID";
const PARTNER_KEY: &str = "SHOPEE_PARTNER_KEY";
const SHOP_ID: &str = "SHOPEE_SHOP_ID";
const REFRESH_TOKEN: &str = "SHOPEE_REFRESH_TOKEN";

const AUTHORIZE: &str = "/api/v2/shop/auth_partner";
const FIRST_TOKEN: &str = "/api/v2/auth/token/get";
const RENEW_TOKEN: &str = "/api/v2/auth/access_token/get";

/// A second's wait doubling to 8, for Shopee's per-second limits.
const BACKOFF: Backoff = Backoff {
    first: Duration::from_secs(1),
    retries: 4,
};

/// A single attempt.
const ONCE: Backoff = Backoff {
    first: Duration::ZERO,
    retries: 0,
};

/// Shopee's seller API, as the owner's Connection sees it.
pub struct ShopeeSeller {
    agent: ureq::Agent,
    api: String,
    store: Arc<dyn SecretStore>,
    clock: Arc<dyn Clock>,
    pause: Arc<dyn Pause>,
    tokens: Tokens,
}

/// What the browser lands on after the owner authorizes the app on
/// Shopee's page: a one-time code and the shop's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShopAuthorization {
    pub code: String,
    pub shop_id: String,
}

impl ShopAuthorization {
    /// The code and shop id in the address the browser landed on, or in
    /// its query alone; `None` when either is missing or malformed.
    pub fn from_landing(address: &str) -> Option<Self> {
        let address = address.trim();
        let query = address.split_once('?').map_or(address, |(_, query)| query);
        let query = query.split('#').next().unwrap_or_default();
        let value = |name: &str| {
            query
                .split('&')
                .filter_map(|pair| pair.split_once('='))
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_owned())
        };
        let code = value("code")
            .filter(|code| !code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric()))?;
        let shop_id = value("shop_id").filter(|id| is_numeric_id(id))?;
        Some(Self { code, shop_id })
    }
}

/// The partner's id, digits only, and its key.
struct Partner {
    id: String,
    key: Secret,
}

impl Partner {
    /// Shopee's signature of `base`: HMAC-SHA256 with the partner key, in
    /// lowercase hex.
    fn sign(&self, base: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.key.expose().as_bytes())
            .expect("HMAC takes a key of any length");
        mac.update(base.as_bytes());
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

impl ShopeeSeller {
    /// The API at `api`, with the keys and the shop's login kept in `store`.
    pub fn new(
        api: &str,
        user_agent: &str,
        store: Arc<dyn SecretStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            agent: http_agent(api, user_agent, Duration::from_secs(30)),
            api: api.trim_end_matches('/').to_owned(),
            tokens: Tokens::new(clock.clone()),
            store,
            clock,
            pause: Arc::new(ThreadPause),
        }
    }

    /// The same, waiting between retries with `pause`.
    pub fn with_pause(mut self, pause: Arc<dyn Pause>) -> Self {
        self.pause = pause;
        self
    }

    /// Shopee's page where the owner logs in to the shop and authorizes the
    /// app, sending the browser to `redirect` with a code and the shop's
    /// id. Shopee takes the link for 5 minutes, so it is made on each
    /// click.
    pub fn authorization_url(&self, redirect: &str) -> Result<String, PlatformError> {
        let partner = self.partner()?;
        let timestamp = self.timestamp();
        let sign = partner.sign(&format!("{}{AUTHORIZE}{timestamp}", partner.id));
        Ok(format!(
            "{}{AUTHORIZE}?partner_id={}&timestamp={timestamp}&sign={sign}&redirect={}",
            self.api,
            partner.id,
            escaped(redirect)
        ))
    }

    /// Trades the one-time code of `authorization` for the shop's login,
    /// kept in the store before anything else uses it.
    pub fn authorize(&self, authorization: &ShopAuthorization) -> Result<(), PlatformError> {
        let partner = self.partner()?;
        self.tokens.replace(|| {
            let body = format!(
                r#"{{"code":{},"shop_id":{},"partner_id":{}}}"#,
                serde_json::Value::from(authorization.code.as_str()),
                authorization.shop_id,
                partner.id
            );
            let answer = self.public_call(&partner, FIRST_TOKEN, &body)?;
            let fresh = self.fresh(answer, |error, message| match error {
                "error_rate_limit" | "error_limit" => PlatformError::RateLimited,
                // A code past its 10 minutes or already used.
                _ => PlatformError::Refused(message.unwrap_or(error).to_owned()),
            })?;
            self.write(SHOP_ID, &authorization.shop_id)?;
            self.write(REFRESH_TOKEN, fresh.refresh_token.expose())?;
            Ok(fresh)
        })
    }

    /// The shop's id, digits only.
    fn shop_id(&self) -> Result<String, PlatformError> {
        let shop = self.read(SHOP_ID)?;
        let shop = shop.expose().trim();
        if is_numeric_id(shop) {
            Ok(shop.to_owned())
        } else {
            Err(PlatformError::Refused(
                "the Shopee shop id has only digits".into(),
            ))
        }
    }

    fn partner(&self) -> Result<Partner, PlatformError> {
        let id = self.read(PARTNER_ID)?;
        let id = id.expose().trim();
        if !is_numeric_id(id) {
            return Err(PlatformError::Refused(
                "the Shopee partner id has only digits".into(),
            ));
        }
        Ok(Partner {
            id: id.to_owned(),
            key: self.read(PARTNER_KEY)?,
        })
    }

    fn read(&self, name: &str) -> Result<Secret, PlatformError> {
        match self.store.read(name) {
            Ok(Some(secret)) => Ok(secret),
            Ok(None) => Err(PlatformError::NotConnected),
            Err(error) => Err(PlatformError::Failed(error.to_string())),
        }
    }

    fn write(&self, name: &str, value: &str) -> Result<(), PlatformError> {
        self.store
            .write(name, &Secret::new(value))
            .map_err(|error| {
                PlatformError::Failed(format!("could not keep the Shopee login: {error}"))
            })
    }

    fn timestamp(&self) -> i64 {
        self.clock.now().timestamp()
    }

    /// A new access token from the stored single-use refresh token, the
    /// next refresh token written to the store before it is returned.
    fn renew(&self) -> Result<Fresh, PlatformError> {
        let partner = self.partner()?;
        let shop = self.shop_id()?;
        let refresh_token = self.read(REFRESH_TOKEN)?;
        let body = format!(
            r#"{{"refresh_token":{},"shop_id":{shop},"partner_id":{}}}"#,
            serde_json::Value::from(refresh_token.expose()),
            partner.id
        );
        let answer = self.public_call(&partner, RENEW_TOKEN, &body)?;
        let fresh = self.fresh(answer, |error, _| match error {
            "error_rate_limit" | "error_limit" => PlatformError::RateLimited,
            // The refresh token was used, is past its 30 days, or the shop
            // took the authorization back.
            _ => PlatformError::Expired,
        })?;
        self.write(REFRESH_TOKEN, fresh.refresh_token.expose())?;
        Ok(fresh)
    }

    /// The tokens in `answer`, or the error `refused` makes of Shopee's.
    fn fresh(
        &self,
        answer: TokenAnswer,
        refused: impl Fn(&str, Option<&str>) -> PlatformError,
    ) -> Result<Fresh, PlatformError> {
        if let Some(error) = answer.error.as_deref().filter(|error| !error.is_empty()) {
            return Err(refused(error, answer.message.as_deref()));
        }
        match (answer.access_token, answer.refresh_token) {
            (Some(access), Some(refresh)) if !access.is_empty() && !refresh.is_empty() => {
                Ok(Fresh {
                    access_token: Secret::new(access),
                    refresh_token: Secret::new(refresh),
                    expires_at: self.clock.now() + TimeDelta::seconds(answer.expire_in),
                })
            }
            _ => Err(PlatformError::Failed(
                "Shopee answered a login without its tokens".into(),
            )),
        }
    }

    /// `POST path` of a public API, signed without a shop: the tokens'.
    fn public_call(
        &self,
        partner: &Partner,
        path: &str,
        body: &str,
    ) -> Result<TokenAnswer, PlatformError> {
        retrying(self.pause.as_ref(), BACKOFF, || {
            let timestamp = self.timestamp().to_string();
            let sign = partner.sign(&format!("{}{path}{timestamp}", partner.id));
            let sent = self
                .agent
                .post(format!("{}{path}", self.api))
                .query("partner_id", &partner.id)
                .query("timestamp", &timestamp)
                .query("sign", &sign)
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .send(body);
            let response = match sent {
                Ok(response) => response,
                Err(error) => return Attempt::Again(PlatformError::Failed(error.to_string())),
            };
            let status = response.status().as_u16();
            match read_json::<TokenAnswer>(response) {
                Ok(answer) if answer.error.as_deref() == Some("error_rate_limit") => {
                    Attempt::Again(PlatformError::RateLimited)
                }
                Ok(answer) if status < 500 => Attempt::Done(Ok(answer)),
                _ => again_or_done(status, path),
            }
        })
    }

    /// The `response` of Shopee's answer to `GET path` with `query`.
    fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, PlatformError> {
        self.signed(path, BACKOFF, attempt, |common| {
            let mut request = self
                .agent
                .get(format!("{}{path}", self.api))
                .header("Accept", "application/json");
            for (key, value) in common.iter().chain(query) {
                request = request.query(*key, *value);
            }
            request.call()
        })
    }

    /// The `response` of Shopee's answer to `POST path` with the JSON
    /// `body`.
    fn post<T: DeserializeOwned>(&self, path: &str, body: &str) -> Result<T, PlatformError> {
        self.signed(path, BACKOFF, attempt, |common| {
            self.send_post(path, common, body)
        })
    }

    /// `POST path` with the JSON `body`, sent once: an answer lost on the
    /// way or a moment's failure is an error, never a second call, for a
    /// step that must not happen twice. Its caller reads back what Shopee
    /// kept before asking again.
    fn post_once(&self, path: &str, body: &str) -> Result<(), PlatformError> {
        self.signed(path, ONCE, acknowledged, |common| {
            self.send_post(path, common, body)
        })
    }

    /// The `response` of Shopee's answer to `POST path` with the JSON
    /// `body`, sent once, as [`Self::post_once`]: for a call that creates
    /// something.
    fn post_once_for<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &str,
    ) -> Result<T, PlatformError> {
        self.signed(path, ONCE, attempt, |common| {
            self.send_post(path, common, body)
        })
    }

    /// The document Shopee answers to `POST path` with the JSON `body`.
    fn post_for_document(&self, path: &str, body: &str) -> Result<Vec<u8>, PlatformError> {
        self.signed(path, BACKOFF, document, |common| {
            self.send_post(path, common, body)
        })
    }

    fn send_post(
        &self,
        path: &str,
        common: &[(&str, &str)],
        body: &str,
    ) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
        let mut request = self
            .agent
            .post(format!("{}{path}", self.api))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json");
        for (key, value) in common {
            request = request.query(*key, *value);
        }
        request.send(body)
    }

    /// The answer to the shop API call `send` makes with the common
    /// parameters, as `read` makes it out, signed afresh on each attempt,
    /// retried by `backoff` while Shopee throttles or fails for a moment.
    /// A token Shopee turns down is renewed once before the login counts
    /// as expired.
    fn signed<T>(
        &self,
        path: &str,
        backoff: Backoff,
        read: impl Fn(ureq::http::Response<ureq::Body>, &str) -> Attempt<T>,
        send: impl Fn(&[(&str, &str)]) -> Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<T, PlatformError> {
        let partner = self.partner()?;
        let shop = self.shop_id()?;
        let mut renewed = false;
        loop {
            let token = self.tokens.access_token(|| self.renew())?;
            let answer = retrying(self.pause.as_ref(), backoff, || {
                let timestamp = self.timestamp().to_string();
                let sign = partner.sign(&format!(
                    "{}{path}{timestamp}{}{shop}",
                    partner.id,
                    token.expose()
                ));
                let common = [
                    ("partner_id", partner.id.as_str()),
                    ("timestamp", timestamp.as_str()),
                    ("access_token", token.expose()),
                    ("shop_id", shop.as_str()),
                    ("sign", sign.as_str()),
                ];
                match send(&common) {
                    Ok(response) => read(response, path),
                    // The token travels in the query, which a transport
                    // error may quote: it never reaches a message.
                    Err(error) => Attempt::Again(PlatformError::Failed(
                        error.to_string().replace(token.expose(), "…"),
                    )),
                }
            });
            match answer {
                Err(PlatformError::Expired) if !renewed => {
                    self.tokens.forget(&token);
                    renewed = true;
                }
                answer => return answer,
            }
        }
    }
}

/// What Shopee's answer for `path` means, and whether to ask again.
fn attempt<T: DeserializeOwned>(
    response: ureq::http::Response<ureq::Body>,
    path: &str,
) -> Attempt<T> {
    let status = response.status().as_u16();
    match read_json::<Envelope<T>>(response) {
        Ok(envelope) => answered(status, envelope, path),
        Err(_) => again_or_done(status, path),
    }
}

/// Whether Shopee took the call to `path`: some calls answer an empty
/// `response`, or none.
fn acknowledged(response: ureq::http::Response<ureq::Body>, path: &str) -> Attempt<()> {
    let status = response.status().as_u16();
    match read_json::<Envelope<serde_json::Value>>(response) {
        Ok(envelope) => answered(
            status,
            Envelope {
                response: Some(()),
                ..envelope.without_response()
            },
            path,
        ),
        Err(_) => again_or_done(status, path),
    }
}

/// The document Shopee sends for `path`, or the error its envelope tells.
fn document(response: ureq::http::Response<ureq::Body>, path: &str) -> Attempt<Vec<u8>> {
    let status = response.status().as_u16();
    let Ok(bytes) = response
        .into_body()
        .into_with_config()
        .limit(MAX_DOCUMENT_BYTES)
        .read_to_vec()
    else {
        return again_or_done(status, path);
    };
    if (200..300).contains(&status) && !bytes.trim_ascii_start().starts_with(b"{") {
        return Attempt::Done(Ok(bytes));
    }
    match serde_json::from_slice::<Envelope<serde_json::Value>>(&bytes) {
        Ok(envelope) => match answered(status, envelope.without_response(), path) {
            Attempt::Done(Ok(())) => Attempt::Done(Err(PlatformError::Failed(format!(
                "Shopee answered {path} without the document"
            )))),
            Attempt::Done(Err(error)) => Attempt::Done(Err(error)),
            Attempt::Again(error) => Attempt::Again(error),
        },
        Err(_) => again_or_done(status, path),
    }
}

/// What Shopee's `envelope` for `path` means, and whether to ask again.
/// Its `error` decides when there is one, whatever the HTTP status.
fn answered<T>(status: u16, envelope: Envelope<T>, path: &str) -> Attempt<T> {
    match envelope.error.as_deref().filter(|error| !error.is_empty()) {
        None if (200..300).contains(&status) => match envelope.response {
            Some(answer) => Attempt::Done(Ok(answer)),
            None => Attempt::Done(Err(PlatformError::Failed(format!(
                "Shopee answered {path} without a response"
            )))),
        },
        None => again_or_done(status, path),
        Some("error_auth") => Attempt::Done(Err(PlatformError::Expired)),
        Some("error_rate_limit") => Attempt::Again(PlatformError::RateLimited),
        // The app's daily quota: nothing goes through until Shopee resets
        // it, so waiting a few seconds would not help.
        Some("error_limit") => Attempt::Done(Err(PlatformError::RateLimited)),
        Some("error_server") => {
            Attempt::Again(PlatformError::Failed(format!("Shopee failed on {path}")))
        }
        Some("error_item_not_found" | "error_not_found") => {
            Attempt::Done(Err(PlatformError::NotFound))
        }
        Some(error) => Attempt::Done(Err(PlatformError::Refused(
            envelope
                .message
                .filter(|message| !message.trim().is_empty())
                .unwrap_or_else(|| error.to_owned()),
        ))),
    }
}

/// What an answer without a readable envelope means, by its status.
fn again_or_done<T>(status: u16, path: &str) -> Attempt<T> {
    match status {
        401 | 403 => Attempt::Done(Err(PlatformError::Expired)),
        404 => Attempt::Done(Err(PlatformError::NotFound)),
        429 => Attempt::Again(PlatformError::RateLimited),
        500..=599 => Attempt::Again(PlatformError::Failed(format!(
            "Shopee answered {status} for {path}"
        ))),
        _ => Attempt::Done(Err(PlatformError::Failed(format!(
            "Shopee answered {status} for {path}"
        )))),
    }
}

fn read_json<T: DeserializeOwned>(
    response: ureq::http::Response<ureq::Body>,
) -> Result<T, PlatformError> {
    crate::answers::read_json(response, "Shopee")
}

/// Shopee's ids are numbers: digits only, so no other path or body is ever
/// reached.
fn is_numeric_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.chars().all(|c| c.is_ascii_digit())
}

/// Shopee's Order numbers are letters and digits, as `201218V2Y6E59M`.
fn is_order_sn(order: &str) -> bool {
    !order.is_empty() && order.len() <= 32 && order.chars().all(|c| c.is_ascii_alphanumeric())
}
