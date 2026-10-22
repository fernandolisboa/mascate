//! Short.io's adapter (#158, ADR 0035 decision 8): shortens Affiliate Links
//! on the owner's domain for the affiliate module. Each link is one
//! `POST {API}/links` with the secret API key in `Authorization`, the
//! domain, the original link and a path chosen by the caller. Short.io gives
//! back the link it already has for the same path and original link, so a
//! request whose answer got lost is safe to send again.

use std::sync::Arc;
use std::time::Duration;

use mascate_affiliate::{LinkShortener, Shortened};
use mascate_kernel::PlatformError;
use mascate_platform::{Secret, SecretStore, http_agent};
use serde::Deserialize;

use crate::answers::{page_on, read_json};
use crate::retry::{Attempt, Backoff, Pause, ThreadPause, retrying};

/// Short.io's API, unless a test points elsewhere.
pub const API_URL: &str = "https://api.short.io";

const API_KEY: &str = "SHORT_IO_API_KEY";
const DOMAIN: &str = "SHORT_IO_DOMAIN";

/// Short.io takes 50 new links a second; a 429, a failure on its side or a
/// dropped connection waits 1, 2 and then 4 seconds before giving up.
const BACKOFF: Backoff = Backoff {
    first: Duration::from_secs(1),
    retries: 3,
};

/// Short.io, with the owner's API key and domain.
pub struct ShortIo {
    agent: ureq::Agent,
    api: String,
    store: Arc<dyn SecretStore>,
    pause: Arc<dyn Pause>,
}

impl ShortIo {
    /// The API at `api`, with the key and the domain kept in `store`.
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

    fn stored(&self, name: &str) -> Result<Secret, PlatformError> {
        self.store
            .read(name)
            .map_err(|error| PlatformError::Failed(error.to_string()))?
            .ok_or(PlatformError::NotConnected)
    }

    /// The owner's domain as Short.io names it, however it was pasted.
    fn domain(&self) -> Result<String, PlatformError> {
        let pasted = self.stored(DOMAIN)?;
        let domain = pasted.expose().trim().to_ascii_lowercase();
        let domain = domain
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        let labels_ok = domain.split('.').count() >= 2
            && domain.split('.').all(|label| {
                !label.is_empty()
                    && label
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            });
        if !labels_ok {
            return Err(PlatformError::Refused(format!(
                "o domínio \"{domain}\" não é um endereço como mascate.short.gy"
            )));
        }
        Ok(domain.to_owned())
    }

    fn send(&self, key: &Secret, domain: &str, body: &str) -> Attempt<Shortened> {
        let sent = self
            .agent
            .post(format!("{}/links", self.api))
            .header("Authorization", key.expose())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .send(body);
        let response = match sent {
            Ok(response) => response,
            Err(error) => return Attempt::Again(PlatformError::Failed(error.to_string())),
        };
        match response.status().as_u16() {
            200 | 201 => Attempt::Done(read_json::<LinkAnswer>(response, "Short.io").and_then(
                |answer| {
                    let url = page_on(&answer.secure_short_url, domain).ok_or_else(|| {
                        PlatformError::Failed(format!(
                            "Short.io answered a link off {domain}: {}",
                            answer.secure_short_url
                        ))
                    })?;
                    Ok(Shortened {
                        url,
                        key: answer.id_string,
                    })
                },
            )),
            401 => Attempt::Done(Err(PlatformError::Expired)),
            429 => Attempt::Again(PlatformError::RateLimited),
            status @ 500..=599 => {
                Attempt::Again(PlatformError::Failed(format!("Short.io answered {status}")))
            }
            status @ 400..=499 => Attempt::Done(Err(PlatformError::Refused(
                read_json::<ErrorAnswer>(response, "Short.io")
                    .ok()
                    .and_then(|answer| answer.message.or(answer.error))
                    .unwrap_or_else(|| format!("Short.io answered {status}")),
            ))),
            status => Attempt::Done(Err(PlatformError::Failed(format!(
                "Short.io answered {status}"
            )))),
        }
    }
}

impl LinkShortener for ShortIo {
    fn shorten(&self, link: &str, path: &str) -> Result<Shortened, PlatformError> {
        let key = self.stored(API_KEY)?;
        let domain = self.domain()?;
        let body = serde_json::json!({
            "domain": domain,
            "originalURL": link,
            "path": path,
            "allowDuplicates": false,
        })
        .to_string();
        retrying(self.pause.as_ref(), BACKOFF, || {
            self.send(&key, &domain, &body)
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkAnswer {
    id_string: String,
    #[serde(rename = "secureShortURL")]
    secure_short_url: String,
}

#[derive(Deserialize)]
struct ErrorAnswer {
    message: Option<String>,
    error: Option<String>,
}
