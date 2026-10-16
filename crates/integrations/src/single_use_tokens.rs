//! An access token renewed with a single-use refresh token, as Shopee's and
//! Magalu's: each renewal hands out the next refresh token, so renewals run
//! one at a time and the new refresh token is in the secret store before
//! the access token is used. Two renewals racing, or a crash in between,
//! would lose the login.

use std::sync::{Arc, Mutex, PoisonError};

use chrono::TimeDelta;
use mascate_kernel::{Clock, PlatformError, Timestamp};
use mascate_platform::Secret;

/// Renews this long before the access token runs out, so a request never
/// leaves with a token about to lapse.
const RENEW_BEFORE: TimeDelta = TimeDelta::minutes(5);

/// A login the Platform just handed out.
pub(crate) struct Fresh {
    pub access_token: Secret,
    pub refresh_token: Secret,
    pub expires_at: Timestamp,
}

pub(crate) struct Tokens {
    clock: Arc<dyn Clock>,
    /// Held while renewing, so a second caller waits and reuses the result.
    current: Mutex<Option<AccessToken>>,
}

struct AccessToken {
    value: Secret,
    expires_at: Timestamp,
}

impl Tokens {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            current: Mutex::new(None),
        }
    }

    /// A valid access token, from `renew` first when there is none or it
    /// is about to expire.
    pub fn access_token(
        &self,
        renew: impl FnOnce() -> Result<Fresh, PlatformError>,
    ) -> Result<Secret, PlatformError> {
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(token) = current
            .as_ref()
            .filter(|token| token.expires_at - RENEW_BEFORE > self.clock.now())
        {
            return Ok(token.value.clone());
        }
        let fresh = renew()?;
        let value = fresh.access_token.clone();
        *current = Some(AccessToken {
            value: fresh.access_token,
            expires_at: fresh.expires_at,
        });
        Ok(value)
    }

    /// Replaces the login with the one `log_in` gets, while no request can
    /// renew the old one.
    pub fn replace(
        &self,
        log_in: impl FnOnce() -> Result<Fresh, PlatformError>,
    ) -> Result<(), PlatformError> {
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        let fresh = log_in()?;
        *current = Some(AccessToken {
            value: fresh.access_token,
            expires_at: fresh.expires_at,
        });
        Ok(())
    }

    /// Drops `used` after the Platform turned it down, so the next request
    /// renews. A token another request already replaced is left alone.
    pub fn forget(&self, used: &Secret) {
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_ref().is_some_and(|token| &token.value == used) {
            *current = None;
        }
    }
}
