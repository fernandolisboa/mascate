//! The Connection's access token, from Login with Amazon. The refresh token
//! the owner pastes does not change on renewal, so nothing is written back;
//! renewals still run one at a time, so requests racing for a token share
//! one renewal.

use std::sync::{Arc, Mutex, PoisonError};

use chrono::TimeDelta;
use mascate_kernel::{Clock, PlatformError, Timestamp};
use mascate_platform::{Secret, SecretStore};

use super::answers::TokenAnswer;
use super::{BACKOFF, read_json};
use crate::retry::{Attempt, Pause, retrying};

const CLIENT_ID: &str = "AMAZON_LWA_CLIENT_ID";
const CLIENT_SECRET: &str = "AMAZON_LWA_CLIENT_SECRET";
const REFRESH_TOKEN: &str = "AMAZON_REFRESH_TOKEN";

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
    pub fn access_token(
        &self,
        agent: &ureq::Agent,
        auth: &str,
        pause: &dyn Pause,
    ) -> Result<Secret, PlatformError> {
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(token) = current
            .as_ref()
            .filter(|token| token.expires_at - RENEW_BEFORE > self.clock.now())
        {
            return Ok(token.value.clone());
        }
        let renewed = self.renew(agent, auth, pause)?;
        let value = renewed.value.clone();
        *current = Some(renewed);
        Ok(value)
    }

    /// Drops `used` after Amazon turned it down, so the next request
    /// renews. A token another request already replaced is left alone.
    pub fn forget(&self, used: &Secret) {
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_ref().is_some_and(|token| &token.value == used) {
            *current = None;
        }
    }

    fn renew(
        &self,
        agent: &ureq::Agent,
        auth: &str,
        pause: &dyn Pause,
    ) -> Result<AccessToken, PlatformError> {
        let read = |name| match self.store.read(name) {
            Ok(Some(secret)) => Ok(secret),
            Ok(None) => Err(PlatformError::NotConnected),
            Err(error) => Err(PlatformError::Failed(error.to_string())),
        };
        let (client_id, client_secret, refresh_token) =
            (read(CLIENT_ID)?, read(CLIENT_SECRET)?, read(REFRESH_TOKEN)?);
        let answer: TokenAnswer = retrying(pause, BACKOFF, || {
            let sent = agent
                .post(format!("{auth}/auth/o2/token"))
                .header("Accept", "application/json")
                .send_form([
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token.expose()),
                    ("client_id", client_id.expose()),
                    ("client_secret", client_secret.expose()),
                ]);
            let response = match sent {
                Ok(response) => response,
                Err(error) => return Attempt::Again(PlatformError::Failed(error.to_string())),
            };
            match response.status().as_u16() {
                200 => Attempt::Done(read_json(response)),
                // `invalid_grant`: the refresh token was revoked;
                // `invalid_client`: the client secret expired, as it does
                // every 180 days, or was rotated.
                400 | 401 | 403 => Attempt::Done(Err(PlatformError::Expired)),
                429 => Attempt::Again(PlatformError::RateLimited),
                status @ 500..=599 => Attempt::Again(PlatformError::Failed(format!(
                    "Amazon answered {status} when renewing the login"
                ))),
                status => Attempt::Done(Err(PlatformError::Failed(format!(
                    "Amazon answered {status} when renewing the login"
                )))),
            }
        })?;
        Ok(AccessToken {
            value: Secret::new(answer.access_token),
            expires_at: self.clock.now() + TimeDelta::seconds(answer.expires_in),
        })
    }
}
