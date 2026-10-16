//! Amazon's adapter (#126, #127): the owner's seller account through the
//! Selling Partner API, as commerce's Sales Channel (ADR 0033). It reads the
//! listings, sends a price once the owner approves it and the stock that
//! follows the app's Stock Share, and asks Amazon for the fee of each
//! listing. Amazon has no pause: taking a listing off sale sends zero
//! units. It reads the Orders the owner ships, their Fees from Amazon's
//! finances, and confirms each shipment with its tracking code. Only
//! official endpoints, with the Login with Amazon token alone (no AWS
//! signing since 2023).

mod answers;
mod billing;
mod orders;
mod sales_channel;
mod shipments;
mod tokens;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mascate_kernel::{Clock, PlatformError};
use mascate_platform::{Secret, SecretStore, http_agent};
use serde::de::DeserializeOwned;

use crate::retry::{Attempt, Backoff, Pause, ThreadPause, retrying};
use answers::ErrorAnswer;
use tokens::Tokens;

/// The Selling Partner API for North and South America, Brazil included,
/// unless a test points elsewhere.
pub const API_URL: &str = "https://sellingpartnerapi-na.amazon.com";

/// Where Login with Amazon hands out access tokens.
pub const AUTH_URL: &str = "https://api.amazon.com";

/// Amazon.com.br, the only store the app sells on.
const MARKETPLACE: &str = "A2Q3Y263D00KWC";

const SELLER_ID: &str = "AMAZON_SELLER_ID";

/// Listings allows 5 calls a second and the fee estimate 1: a second's
/// wait doubling to 8 outlasts both.
const BACKOFF: Backoff = Backoff {
    first: Duration::from_secs(1),
    retries: 4,
};

/// Amazon's Selling Partner API, as the owner's Connection sees it.
pub struct Amazon {
    agent: ureq::Agent,
    api: String,
    auth: String,
    store: Arc<dyn SecretStore>,
    pause: Arc<dyn Pause>,
    tokens: Tokens,
    /// Amazon turned the buyer's address down: the app lacks the
    /// Direct-to-Consumer Shipping role, so later searches go without it.
    recipient_refused: AtomicBool,
}

impl Amazon {
    /// The API at `api`, asking tokens of `auth`, with the credentials kept
    /// in `store`.
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
            tokens: Tokens::new(store.clone(), clock),
            store,
            pause: Arc::new(ThreadPause),
            recipient_refused: AtomicBool::new(false),
        }
    }

    /// The same, waiting between retries with `pause`.
    pub fn with_pause(mut self, pause: Arc<dyn Pause>) -> Self {
        self.pause = pause;
        self
    }

    /// The seller's id, which every listing path carries: letters and
    /// digits only, so no other path is ever reached.
    fn seller(&self) -> Result<String, PlatformError> {
        let seller = match self.store.read(SELLER_ID) {
            Ok(Some(seller)) => seller,
            Ok(None) => return Err(PlatformError::NotConnected),
            Err(error) => return Err(PlatformError::Failed(error.to_string())),
        };
        let seller = seller.expose().trim();
        if !seller.is_empty() && seller.chars().all(|c| c.is_ascii_alphanumeric()) {
            Ok(seller.to_owned())
        } else {
            Err(PlatformError::Refused(
                "the Amazon seller id has only letters and digits".into(),
            ))
        }
    }

    /// The JSON answer to `GET path` with `query`.
    fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, PlatformError> {
        let response = self.signed(path, |token| {
            let mut request = self
                .agent
                .get(format!("{}{path}", self.api))
                .header("Accept", "application/json")
                .header("x-amz-access-token", token.expose());
            for (key, value) in query {
                request = request.query(*key, *value);
            }
            request.call()
        })?;
        read_json(response)
    }

    /// The JSON answer to `method path` with `query` and the JSON `body`.
    fn send<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        body: &str,
    ) -> Result<T, PlatformError> {
        read_json(self.request(method, path, query, body)?)
    }

    /// `POST path` with the JSON `body`, for an operation that answers
    /// nothing but its success.
    fn post_without_answer(&self, path: &str, body: &str) -> Result<(), PlatformError> {
        self.request("POST", path, &[], body).map(drop)
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        body: &str,
    ) -> Result<ureq::http::Response<ureq::Body>, PlatformError> {
        self.signed(path, |token| {
            let url = format!("{}{path}", self.api);
            let mut request = match method {
                "PATCH" => self.agent.patch(url),
                _ => self.agent.post(url),
            }
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("x-amz-access-token", token.expose());
            for (key, value) in query {
                request = request.query(*key, *value);
            }
            request.send(body)
        })
    }

    /// A successful answer to the request `send` makes with the
    /// Connection's access token, retried while Amazon throttles or fails
    /// for a moment. A token Amazon turns down is renewed once before the
    /// login counts as expired.
    fn signed(
        &self,
        path: &str,
        send: impl Fn(&Secret) -> Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<ureq::http::Response<ureq::Body>, PlatformError> {
        let mut renewed = false;
        loop {
            let token = self
                .tokens
                .access_token(&self.agent, &self.auth, self.pause.as_ref())?;
            let answer = retrying(self.pause.as_ref(), BACKOFF, || match send(&token) {
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

/// What Amazon's answer for `path` means, and whether to ask again.
fn attempt(
    response: ureq::http::Response<ureq::Body>,
    path: &str,
) -> Attempt<ureq::http::Response<ureq::Body>> {
    match response.status().as_u16() {
        200..=299 => Attempt::Done(Ok(response)),
        // Amazon answers an expired or revoked token with 403.
        401 | 403 => Attempt::Done(Err(PlatformError::Expired)),
        400 | 409 | 413 | 415 => Attempt::Done(Err(PlatformError::Refused(refusal(response)))),
        404 => Attempt::Done(Err(PlatformError::NotFound)),
        429 => Attempt::Again(PlatformError::RateLimited),
        status @ 500..=599 => Attempt::Again(PlatformError::Failed(format!(
            "Amazon answered {status} for {path}"
        ))),
        status => Attempt::Done(Err(PlatformError::Failed(format!(
            "Amazon answered {status} for {path}"
        )))),
    }
}

fn read_json<T: DeserializeOwned>(
    response: ureq::http::Response<ureq::Body>,
) -> Result<T, PlatformError> {
    crate::answers::read_json(response, "Amazon")
}

/// Amazon's own word for a refusal: the messages of the errors it lists.
fn refusal(response: ureq::http::Response<ureq::Body>) -> String {
    read_json::<ErrorAnswer>(response)
        .ok()
        .map(|answer| {
            answer
                .errors
                .into_iter()
                .filter_map(|error| error.message)
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|why| !why.is_empty())
        .unwrap_or_else(|| "refused".into())
}

/// An Amazon Order id as a path segment: digits and dashes only, as its
/// 3-7-7 form, so no other path is ever reached.
fn order_segment(order: &str) -> Result<&str, PlatformError> {
    if !order.is_empty() && order.chars().all(|c| c.is_ascii_digit() || c == '-') {
        Ok(order)
    } else {
        Err(PlatformError::NotFound)
    }
}

/// `text` as one segment of a path: anything but letters, digits and
/// `-._~` escaped, as Amazon asks of a SKU.
fn path_segment(text: &str) -> String {
    let mut segment = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            segment.push(char::from(byte));
        } else {
            segment.push_str(&format!("%{byte:02X}"));
        }
    }
    segment
}
