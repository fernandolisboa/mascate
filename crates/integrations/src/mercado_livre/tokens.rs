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

    /// Trades the one-time `code` the seller's authorization gave, for
    /// `redirect`, for the login: the refresh token is in the store before
    /// the access token serves anything.
    pub fn authorize(
        &self,
        agent: &ureq::Agent,
        api: &str,
        code: &str,
        redirect: &str,
    ) -> Result<(), PlatformError> {
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        let (client_id, client_secret) = (self.read(CLIENT_ID)?, self.read(CLIENT_SECRET)?);
        // Refused: a code past its time, already used, or given for another
        // address; or keys Mercado Livre does not know.
        let answer = self.trade(
            agent,
            api,
            &[
                ("grant_type", "authorization_code"),
                ("client_id", client_id.expose()),
                ("client_secret", client_secret.expose()),
                ("code", code),
                ("redirect_uri", redirect),
            ],
        )?;
        *current = Some(self.keep(answer)?);
        Ok(())
    }

    fn renew(&self, agent: &ureq::Agent, api: &str) -> Result<AccessToken, PlatformError> {
        let (client_id, client_secret, refresh_token) = (
            self.read(CLIENT_ID)?,
            self.read(CLIENT_SECRET)?,
            self.read(REFRESH_TOKEN)?,
        );
        let answer = self.trade(
            agent,
            api,
            &[
                ("grant_type", "refresh_token"),
                ("client_id", client_id.expose()),
                ("client_secret", client_secret.expose()),
                ("refresh_token", refresh_token.expose()),
            ],
        );
        match answer {
            Ok(answer) => self.keep(answer),
            // `invalid_grant`: the refresh token was used, revoked or expired.
            Err(Refusal::Turned(_)) => Err(PlatformError::Expired),
            Err(refusal) => Err(refusal.into()),
        }
    }

    pub fn client_id(&self) -> Result<Secret, PlatformError> {
        self.read(CLIENT_ID)
    }

    fn read(&self, name: &str) -> Result<Secret, PlatformError> {
        match self.store.read(name) {
            Ok(Some(secret)) => Ok(secret),
            Ok(None) => Err(PlatformError::NotConnected),
            Err(error) => Err(PlatformError::Failed(error.to_string())),
        }
    }

    /// Mercado Livre's answer to a token request with `form`.
    fn trade(
        &self,
        agent: &ureq::Agent,
        api: &str,
        form: &[(&str, &str)],
    ) -> Result<TokenAnswer, Refusal> {
        let response = agent
            .post(format!("{api}/oauth/token"))
            .header("Accept", "application/json")
            .send_form(form.iter().copied())
            .map_err(|error| Refusal::Platform(PlatformError::Failed(error.to_string())))?;
        match response.status().as_u16() {
            200 => read_json(response).map_err(Refusal::Platform),
            400 | 401 | 403 => {
                let answer = read_json::<ErrorAnswer>(response).ok();
                Err(match answer {
                    Some(answer) if answer.error.as_deref() == Some("invalid_client") => {
                        Refusal::UnknownApp
                    }
                    answer => Refusal::Turned(
                        answer
                            .and_then(|answer| {
                                answer.error_description.or(answer.message).or(answer.error)
                            })
                            .unwrap_or_else(|| "refused".into()),
                    ),
                })
            }
            429 => Err(Refusal::Platform(PlatformError::RateLimited)),
            status => Err(Refusal::Platform(PlatformError::Failed(format!(
                "Mercado Livre answered {status} to the login"
            )))),
        }
    }

    /// Stores the new single-use refresh token before its access token is
    /// handed out.
    fn keep(&self, answer: TokenAnswer) -> Result<AccessToken, PlatformError> {
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

/// Why a token request gave no tokens.
enum Refusal {
    /// `invalid_client`: the Client ID or Secret is wrong.
    UnknownApp,
    /// Mercado Livre turned the code or refresh token down, in its words.
    Turned(String),
    Platform(PlatformError),
}

impl From<Refusal> for PlatformError {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::UnknownApp => {
                PlatformError::Refused("Mercado Livre does not know this app".into())
            }
            Refusal::Turned(why) => PlatformError::Refused(why),
            Refusal::Platform(error) => error,
        }
    }
}
