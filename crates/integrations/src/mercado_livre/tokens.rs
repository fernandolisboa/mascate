//! The Connection's access token. Mercado Livre hands out a new single-use
//! refresh token on every renewal, so renewals run one at a time and the new
//! refresh token is in the secret store before the access token is used:
//! two renewals racing, or a crash in between, would lose the login.

use std::sync::{Arc, Mutex, PoisonError};

use chrono::TimeDelta;
use mascate_kernel::{Clock, PlatformError, Timestamp};
use mascate_platform::{Secret, SecretStore};

use super::answers::{ErrorAnswer, TokenAnswer};
use super::read_json;

const CLIENT_ID: &str = "ML_CLIENT_ID";
const CLIENT_SECRET: &str = "ML_CLIENT_SECRET";
const REFRESH_TOKEN: &str = "ML_REFRESH_TOKEN";

/// Renews this long before the access token expires, so a request never
/// leaves with a token about to lapse.
const RENEW_BEFORE: TimeDelta = TimeDelta::minutes(5);

pub(super) struct Tokens {
    store: Arc<dyn SecretStore>,
    clock: Arc<dyn Clock>,
    /// Held while renewing, so a second caller waits and reuses the result.
    current: Mutex<Option<AccessToken>>,
}

struct AccessToken {
    value: Secret,
    expires_at: Timestamp,
}

impl Tokens {
    pub fn new(store: Arc<dyn SecretStore>, clock: Arc<dyn Clock>) -> Self {
        Self {
            store,
            clock,
            current: Mutex::new(None),
        }
    }

    /// A valid access token, renewed first when there is none or it is
    /// about to expire.
    pub fn access_token(&self, agent: &ureq::Agent, api: &str) -> Result<Secret, PlatformError> {
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(token) = current
            .as_ref()
            .filter(|token| token.expires_at - RENEW_BEFORE > self.clock.now())
        {
            return Ok(token.value.clone());
        }
        let renewed = self.renew(agent, api)?;
        let value = renewed.value.clone();
        *current = Some(renewed);
        Ok(value)
    }

    /// Drops `used` after Mercado Livre turned it down, so the next request
    /// renews. A token another request already replaced is left alone.
    pub fn forget(&self, used: &Secret) {
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_ref().is_some_and(|token| &token.value == used) {
            *current = None;
        }
    }

    fn renew(&self, agent: &ureq::Agent, api: &str) -> Result<AccessToken, PlatformError> {
        let read = |name| match self.store.read(name) {
            Ok(Some(secret)) => Ok(secret),
            Ok(None) => Err(PlatformError::NotConnected),
            Err(error) => Err(PlatformError::Failed(error.to_string())),
        };
        let (client_id, client_secret, refresh_token) =
            (read(CLIENT_ID)?, read(CLIENT_SECRET)?, read(REFRESH_TOKEN)?);
        let response = agent
            .post(format!("{api}/oauth/token"))
            .header("Accept", "application/json")
            .send_form([
                ("grant_type", "refresh_token"),
                ("client_id", client_id.expose()),
                ("client_secret", client_secret.expose()),
                ("refresh_token", refresh_token.expose()),
            ])
            .map_err(|error| PlatformError::Failed(error.to_string()))?;
        match response.status().as_u16() {
            200 => {}
            // `invalid_grant`: the refresh token was used, revoked or expired.
            400 | 401 | 403 => {
                let answer = read_json::<ErrorAnswer>(response).ok();
                return Err(match answer.and_then(|answer| answer.error).as_deref() {
                    Some("invalid_client") => {
                        PlatformError::Refused("Mercado Livre does not know this app".into())
                    }
                    _ => PlatformError::Expired,
                });
            }
            429 => return Err(PlatformError::RateLimited),
            status => {
                return Err(PlatformError::Failed(format!(
                    "Mercado Livre answered {status} when renewing the login"
                )));
            }
        }
        let answer: TokenAnswer = read_json(response)?;
        self.store
            .write(REFRESH_TOKEN, &Secret::new(answer.refresh_token))
            .map_err(|error| {
                PlatformError::Failed(format!("could not keep the renewed login: {error}"))
            })?;
        Ok(AccessToken {
            value: Secret::new(answer.access_token),
            expires_at: self.clock.now() + TimeDelta::seconds(answer.expires_in),
        })
    }
}
