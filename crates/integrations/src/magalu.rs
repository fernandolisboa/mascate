//! Magalu's adapter (#133): the owner's Magalu store through the Magalu
//! Open API, as commerce's Sales Channel (ADR 0033). It reads the SKUs of
//! the store's portfolio, each one a Listing with its own price and stock,
//! sends a price once the owner approves it and the stock that follows the
//! app's Stock Share. The store's login comes from ID Magalu's page, whose
//! refresh token is treated as single use. Magalu takes a price to apply it
//! later (202), so the next Sync confirms it; it has no pause of its own,
//! so taking a SKU off sale sends zero units. It gives no fee estimate: the
//! sale fee is the highest of its published commissions. Its Orders (#134)
//! come with their deliveries, and what Magalu kept of each comes from its
//! financial analysis once the Order is delivered.

mod answers;
mod billing;
mod orders;
mod sales_channel;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::TimeDelta;
use mascate_kernel::{Clock, PlatformError};
use mascate_platform::{Secret, SecretStore, http_agent};
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;

use crate::escaped::escaped;
use crate::retry::{Attempt, PAST_A_MINUTE, Pause, ThreadPause, retrying};
use crate::single_use_tokens::{Fresh, Tokens};
use answers::{ErrorAnswer, Me, TokenAnswer};

/// Magalu's Open API, unless a test or a development build points
/// elsewhere.
pub const API_URL: &str = "https://api.magalu.com";

/// Magalu's sandbox, for a development build with a sandbox client.
pub const SANDBOX_URL: &str = "https://api-sandbox.magalu.com";

/// ID Magalu, where the owner logs in and the app gets its tokens.
pub const ID_URL: &str = "https://id.magalu.com";

const CLIENT_ID: &str = "MAGALU_CLIENT_ID";
const CLIENT_SECRET: &str = "MAGALU_CLIENT_SECRET";
const REFRESH_TOKEN: &str = "MAGALU_REFRESH_TOKEN";

/// What the owner lets the app do on the store: the portfolio's SKUs,
/// prices and stocks, the Orders and their deliveries, and the financial
/// analysis of each Order. The client made with `idm` needs the same.
pub const SCOPES: &str = "open:portfolio-skus-seller:read open:portfolio-prices-seller:read \
     open:portfolio-prices-seller:write open:portfolio-stocks-seller:read \
     open:portfolio-stocks-seller:write open:order-order-seller:read \
     open:order-delivery-seller:read open:order-financial-report-seller:read";

/// Magalu pages through at most 5,000 records of a list.
const MAX_OFFSET: usize = 5_000;

/// Magalu's own error slugs come with the status, as `{slug, message}`.
const TOO_MANY_REQUESTS: &str = "TOO_MANY_REQUESTS";

/// Magalu's Open API, as the owner's Connection sees it.
pub struct Magalu {
    agent: ureq::Agent,
    api: String,
    id: String,
    store: Arc<dyn SecretStore>,
    clock: Arc<dyn Clock>,
    pause: Arc<dyn Pause>,
    tokens: Tokens,
    /// The store's sales channel at Magalu, read once per login: prices
    /// and stocks are kept per channel.
    channel: Mutex<Option<String>>,
}

/// What the browser lands on after the owner authorizes the app on ID
/// Magalu's page: a one-time code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreAuthorization {
    pub code: String,
}

impl StoreAuthorization {
    /// The code in the address the browser landed on, or in its query
    /// alone; `None` when it is missing or malformed.
    pub fn from_landing(address: &str) -> Option<Self> {
        let address = address.trim();
        let query = address.split_once('?').map_or(address, |(_, query)| query);
        let query = query.split('#').next().unwrap_or_default();
        let code = query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == "code")
            .map(|(_, value)| value)?;
        let well_formed = !code.is_empty()
            && code.len() <= 4096
            && code
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte));
        well_formed.then(|| Self {
            code: code.to_owned(),
        })
    }
}

impl Magalu {
    /// The API at `api`, logging in at `id`, with the client's keys and the
    /// store's login kept in `store`.
    pub fn new(
        api: &str,
        id: &str,
        user_agent: &str,
        store: Arc<dyn SecretStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            agent: http_agent(api, user_agent, Duration::from_secs(30)),
            api: api.trim_end_matches('/').to_owned(),
            id: id.trim_end_matches('/').to_owned(),
            tokens: Tokens::new(clock.clone()),
            store,
            clock,
            pause: Arc::new(ThreadPause),
            channel: Mutex::new(None),
        }
    }

    /// The same, waiting between retries with `pause`.
    pub fn with_pause(mut self, pause: Arc<dyn Pause>) -> Self {
        self.pause = pause;
        self
    }

    /// ID Magalu's page where the owner logs in, picks the store and
    /// authorizes the app, sending the browser to `redirect` with a code.
    pub fn authorization_url(&self, redirect: &str) -> Result<String, PlatformError> {
        let client = self.read(CLIENT_ID)?;
        Ok(format!(
            "{}/login?client_id={}&redirect_uri={}&scope={}&response_type=code&choose_tenants=true",
            self.id,
            escaped(client.expose().trim()),
            escaped(redirect),
            escaped(SCOPES)
        ))
    }

    /// Trades the one-time code of `authorization`, given for `redirect`,
    /// for the store's login, kept in the store before anything else uses
    /// it.
    pub fn authorize(
        &self,
        authorization: &StoreAuthorization,
        redirect: &str,
    ) -> Result<(), PlatformError> {
        let (client_id, client_secret) = (self.read(CLIENT_ID)?, self.read(CLIENT_SECRET)?);
        self.tokens.replace(|| {
            let body = serde_json::json!({
                "client_id": client_id.expose().trim(),
                "client_secret": client_secret.expose().trim(),
                "redirect_uri": redirect,
                "code": authorization.code,
                "grant_type": "authorization_code",
            })
            .to_string();
            let answer = self.token_call(|| {
                self.agent
                    .post(format!("{}/oauth/token", self.id))
                    .header("Accept", "application/json")
                    .header("Content-Type", "application/json")
                    .send(&body)
            })?;
            let fresh = match answer {
                Ok(answer) => self.fresh(answer, None)?,
                // A code past its time, already used, or given for another
                // address.
                Err(why) => return Err(PlatformError::Refused(why)),
            };
            self.write(REFRESH_TOKEN, fresh.refresh_token.expose())?;
            self.forget_channel();
            Ok(fresh)
        })
    }

    /// A new access token from the stored refresh token, the next refresh
    /// token written to the store before it is returned.
    fn renew(&self) -> Result<Fresh, PlatformError> {
        let (client_id, client_secret) = (self.read(CLIENT_ID)?, self.read(CLIENT_SECRET)?);
        let refresh_token = self.read(REFRESH_TOKEN)?;
        let answer = self.token_call(|| {
            self.agent
                .post(format!("{}/oauth/token", self.id))
                .header("Accept", "application/json")
                .send_form([
                    ("grant_type", "refresh_token"),
                    ("client_id", client_id.expose().trim()),
                    ("client_secret", client_secret.expose().trim()),
                    ("refresh_token", refresh_token.expose()),
                ])
        })?;
        // The refresh token was used, expired, or the store took the
        // authorization back; or the client's secret changed.
        let answer = answer.map_err(|_| PlatformError::Expired)?;
        let fresh = self.fresh(answer, Some(refresh_token))?;
        self.write(REFRESH_TOKEN, fresh.refresh_token.expose())?;
        Ok(fresh)
    }

    /// ID Magalu's answer to the token request `send` makes: the tokens, or
    /// its reason for turning the request down; retried while it throttles
    /// or fails for a moment.
    fn token_call(
        &self,
        send: impl Fn() -> Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Result<TokenAnswer, String>, PlatformError> {
        retrying(self.pause.as_ref(), PAST_A_MINUTE, || {
            let response = match send() {
                Ok(response) => response,
                // The secrets travel in the body, which no error quotes.
                Err(error) => return Attempt::Again(PlatformError::Failed(error.to_string())),
            };
            match response.status().as_u16() {
                200 => Attempt::Done(read_json(response).map(Ok)),
                400 | 401 | 403 => Attempt::Done(Ok(Err(read_json::<TokenAnswer>(response)
                    .ok()
                    .and_then(TokenAnswer::refusal)
                    .unwrap_or_else(|| "refused".into())))),
                429 => Attempt::Again(PlatformError::RateLimited),
                status @ 500..=599 => Attempt::Again(PlatformError::Failed(format!(
                    "ID Magalu answered {status} for the login"
                ))),
                status => Attempt::Done(Err(PlatformError::Failed(format!(
                    "ID Magalu answered {status} for the login"
                )))),
            }
        })
    }

    /// The tokens in `answer`; a renewal that hands out no new refresh
    /// token keeps `previous`.
    fn fresh(&self, answer: TokenAnswer, previous: Option<Secret>) -> Result<Fresh, PlatformError> {
        let refresh_token = answer
            .refresh_token
            .filter(|token| !token.is_empty())
            .map(Secret::new)
            .or(previous);
        match (answer.access_token, refresh_token) {
            (Some(access), Some(refresh)) if !access.is_empty() => Ok(Fresh {
                access_token: Secret::new(access),
                refresh_token: refresh,
                expires_at: self.clock.now() + TimeDelta::seconds(answer.expires_in),
            }),
            _ => Err(PlatformError::Failed(
                "ID Magalu answered a login without its tokens".into(),
            )),
        }
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
                PlatformError::Failed(format!("could not keep the Magalu login: {error}"))
            })
    }

    /// The store's sales channel at Magalu, asked once.
    fn channel(&self) -> Result<String, PlatformError> {
        if let Some(channel) = self
            .channel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return Ok(channel);
        }
        let me: Me = self.get("/seller/v1/portfolios/me", &[])?;
        let channel = me.channel.id;
        if channel.is_empty() {
            return Err(PlatformError::Failed(
                "Magalu answered the store without its channel".into(),
            ));
        }
        *self.channel.lock().unwrap_or_else(PoisonError::into_inner) = Some(channel.clone());
        Ok(channel)
    }

    fn forget_channel(&self) {
        *self.channel.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// The JSON answer to `GET path` with `query`.
    fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, PlatformError> {
        let response = self.authorized(path, |token| {
            let mut request = self
                .agent
                .get(format!("{}{path}", self.api))
                .header("Accept", "application/json")
                .header("Authorization", format!("Bearer {}", token.expose()));
            for (key, value) in query {
                request = request.query(*key, *value);
            }
            request.call()
        })?;
        read_json(response)
    }

    /// `PATCH path` with the JSON `body`, which Magalu takes to apply
    /// later.
    fn patch(&self, path: &str, body: &str) -> Result<(), PlatformError> {
        self.authorized(path, |token| {
            self.agent
                .patch(format!("{}{path}", self.api))
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {}", token.expose()))
                .send(body)
        })
        .map(drop)
    }

    /// A successful answer to the request `send` makes with the store's
    /// access token, retried while Magalu throttles or fails for a moment.
    /// A token Magalu turns down is renewed once before the login counts as
    /// expired.
    fn authorized(
        &self,
        path: &str,
        send: impl Fn(&Secret) -> Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<ureq::http::Response<ureq::Body>, PlatformError> {
        let mut renewed = false;
        loop {
            let token = self.tokens.access_token(|| self.renew())?;
            let answer = retrying(self.pause.as_ref(), PAST_A_MINUTE, || match send(&token) {
                Ok(response) => attempt(response, path),
                Err(error) => Attempt::Again(PlatformError::Failed(error.to_string())),
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

/// What Magalu's answer for `path` means, and whether to ask again.
fn attempt(
    response: ureq::http::Response<ureq::Body>,
    path: &str,
) -> Attempt<ureq::http::Response<ureq::Body>> {
    match response.status().as_u16() {
        200..=299 => Attempt::Done(Ok(response)),
        401 => Attempt::Done(Err(PlatformError::Expired)),
        // The login lacks a scope the call needs, or the store lost access
        // to it.
        403 | 422 => Attempt::Done(Err(PlatformError::Refused(refusal(response)))),
        404 => Attempt::Done(Err(PlatformError::NotFound)),
        429 => Attempt::Again(PlatformError::RateLimited),
        status @ 500..=599 => Attempt::Again(PlatformError::Failed(format!(
            "Magalu answered {status} for {path}"
        ))),
        status => {
            let answer = read_json::<ErrorAnswer>(response).ok();
            if answer.as_ref().and_then(|answer| answer.slug.as_deref()) == Some(TOO_MANY_REQUESTS)
            {
                return Attempt::Again(PlatformError::RateLimited);
            }
            Attempt::Done(Err(PlatformError::Failed(format!(
                "Magalu answered {status} for {path}"
            ))))
        }
    }
}

fn read_json<T: DeserializeOwned>(
    response: ureq::http::Response<ureq::Body>,
) -> Result<T, PlatformError> {
    crate::answers::read_json(response, "Magalu")
}

/// Magalu's own word for a refusal: its message, or the fields its
/// validation names.
fn refusal(response: ureq::http::Response<ureq::Body>) -> String {
    read_json::<ErrorAnswer>(response)
        .ok()
        .and_then(ErrorAnswer::why)
        .unwrap_or_else(|| "refused".into())
}

/// `value` in units of its currency, Magalu sending it times `normalizer`
/// (100 for cents); `None` without a normalizer.
fn in_units(value: i64, normalizer: i64) -> Option<Decimal> {
    (normalizer > 0).then(|| Decimal::from(value) / Decimal::from(normalizer))
}

/// A SKU as a path segment: Magalu's SKUs are at most 32 characters, and
/// anything but letters, digits and `-._~` is escaped, so no other path is
/// ever reached.
fn sku_segment(sku: &str) -> Result<String, PlatformError> {
    if sku.is_empty() || sku.chars().count() > 32 {
        return Err(PlatformError::NotFound);
    }
    Ok(escaped(sku))
}
