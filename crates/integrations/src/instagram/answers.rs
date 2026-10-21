//! The parts of Instagram's answers the adapter reads, as the Instagram
//! Platform's reference documents them for API v25.0.

use serde::Deserialize;

use crate::answers::numeric_id;

/// A Graph API error: `{"error": {message, code, error_subcode, …}}`.
#[derive(Debug, Deserialize)]
pub(super) struct ErrorAnswer {
    pub error: Option<GraphError>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GraphError {
    pub message: Option<String>,
    /// Meant for the account's owner, in Instagram's words.
    pub error_user_msg: Option<String>,
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub error_subcode: i64,
    #[serde(default)]
    pub is_transient: bool,
}

impl GraphError {
    /// Why Instagram turned the request down, in its words.
    pub fn why(&self) -> Option<String> {
        [&self.error_user_msg, &self.message]
            .into_iter()
            .flatten()
            .map(|why| why.trim())
            .find(|why| !why.is_empty())
            .map(str::to_owned)
    }
}

/// `GET /refresh_access_token`: the token, valid for `expires_in` seconds.
#[derive(Debug, Deserialize)]
pub(super) struct TokenAnswer {
    pub access_token: Option<String>,
    #[serde(default)]
    pub expires_in: i64,
}

/// `GET /me?fields=user_id,username`. The reference shows the fields at the
/// top, its guide inside `data`; both are read.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum MeAnswer {
    Listed { data: Vec<Account> },
    Flat(Account),
}

#[derive(Debug, Deserialize)]
pub(super) struct Account {
    #[serde(default, deserialize_with = "optional_id")]
    pub user_id: Option<String>,
    pub username: Option<String>,
}

impl MeAnswer {
    pub fn account(self) -> Option<Account> {
        match self {
            MeAnswer::Listed { data } => data.into_iter().next(),
            MeAnswer::Flat(account) => Some(account),
        }
    }
}

/// `GET /{ig-user-id}/content_publishing_limit?fields=quota_usage,config`.
#[derive(Debug, Deserialize)]
pub(super) struct LimitAnswer {
    #[serde(default)]
    pub data: Vec<Limit>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Limit {
    #[serde(default)]
    pub quota_usage: u64,
    pub config: Option<LimitConfig>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LimitConfig {
    #[serde(default)]
    pub quota_total: u64,
}

impl LimitAnswer {
    /// Whether the posts of the last 24 hours reached the quota.
    pub fn used_up(&self) -> bool {
        self.data.first().is_some_and(|limit| {
            limit.config.as_ref().is_some_and(|config| {
                config.quota_total > 0 && limit.quota_usage >= config.quota_total
            })
        })
    }
}

/// The id Instagram gave a container or a media object.
#[derive(Debug, Deserialize)]
pub(super) struct Created {
    #[serde(deserialize_with = "numeric_id")]
    pub id: String,
}

/// `GET /{container-id}?fields=status_code,status`.
#[derive(Debug, Deserialize)]
pub(super) struct ContainerAnswer {
    #[serde(default)]
    pub status_code: String,
    /// What went wrong, when `status_code` is `ERROR`.
    pub status: Option<String>,
}

/// `GET /{media-id}?fields=permalink`.
#[derive(Debug, Deserialize)]
pub(super) struct MediaAnswer {
    pub permalink: Option<String>,
}

fn optional_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    numeric_id(deserializer).map(Some)
}
