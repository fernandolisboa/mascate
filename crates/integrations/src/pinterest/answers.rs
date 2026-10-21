//! The parts of Pinterest's answers the adapter reads, as API v5
//! documents them.

use mascate_marketing::{PinterestBoard, PublishedPost};
use serde::Deserialize;

use crate::answers::numeric_id;

/// The answer to a token request.
#[derive(Debug, Deserialize)]
pub(super) struct TokenAnswer {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    /// Seconds, 30 days.
    #[serde(default)]
    pub expires_in: i64,
}

/// An error: `{code, message}` from the API, an OAuth `error` from the
/// token endpoint.
#[derive(Debug, Deserialize)]
pub(super) struct ErrorAnswer {
    pub message: Option<String>,
    pub error_description: Option<String>,
    pub error: Option<String>,
}

impl ErrorAnswer {
    /// Why Pinterest turned the request down, in its words.
    pub fn why(self) -> Option<String> {
        [self.message, self.error_description, self.error]
            .into_iter()
            .flatten()
            .find(|why| !why.trim().is_empty())
    }
}

/// `GET /v5/boards`: a page of boards and the bookmark of the next one.
#[derive(Debug, Deserialize)]
pub(super) struct BoardsPage {
    #[serde(default)]
    pub items: Vec<BoardAnswer>,
    pub bookmark: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct BoardAnswer {
    #[serde(deserialize_with = "numeric_id")]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

impl BoardAnswer {
    /// The board, when Pinterest named it.
    pub fn board(self) -> Option<PinterestBoard> {
        let name = self.name.trim().to_owned();
        (!self.id.is_empty() && !name.is_empty()).then_some(PinterestBoard { id: self.id, name })
    }
}

/// `POST /v5/pins`: the Pin created.
#[derive(Debug, Deserialize)]
pub(super) struct PinAnswer {
    #[serde(deserialize_with = "numeric_id")]
    pub id: String,
}

impl PinAnswer {
    /// The Pin and where it opens, when its id can go in an address.
    pub fn published(self) -> Option<PublishedPost> {
        let id = self.id.trim();
        (!id.is_empty() && id.bytes().all(|byte| byte.is_ascii_alphanumeric())).then(|| {
            PublishedPost {
                message_id: id.to_owned(),
                link: Some(format!("https://www.pinterest.com/pin/{id}/")),
            }
        })
    }
}
