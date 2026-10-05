//! The Claude API's adapter (#17): writes Listing Copy for marketing
//! (ADR 0028). Each request is one `POST /v1/messages` with the owner's API
//! key, asking for a JSON answer with the title and the description; the
//! text only ever becomes a draft the owner edits.

use std::sync::Arc;
use std::time::Duration;

use mascate_kernel::PlatformError;
use mascate_marketing::{CopyRequest, CopyWriter, WrittenCopy};
use mascate_platform::{Secret, SecretStore, slow_http_agent};
use serde::Deserialize;

use crate::answers::read_json;
use crate::retry::{Attempt, Backoff, Pause, ThreadPause, retrying};

/// Anthropic's API, unless a test points elsewhere.
pub const API_URL: &str = "https://api.anthropic.com";

const API_KEY: &str = "ANTHROPIC_API_KEY";

/// The API version every request names.
const API_VERSION: &str = "2023-06-01";

/// Room for the model's thinking and a long description; a request without
/// streaming stays well under the API's timeouts at this size.
const MAX_TOKENS: u32 = 16_000;

/// A request answers only once the text is written, which takes a while
/// with thinking.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// A rate limit (429), an overloaded API (529) or another failure on
/// Anthropic's side waits 2, 4 and then 8 seconds before giving up.
const BACKOFF: Backoff = Backoff {
    first: Duration::from_secs(2),
    retries: 3,
};

/// The Claude API, with the owner's key.
pub struct Anthropic {
    agent: ureq::Agent,
    api: String,
    store: Arc<dyn SecretStore>,
    pause: Arc<dyn Pause>,
}

impl Anthropic {
    /// The API at `api`, with the key kept in `store`.
    pub fn new(api: &str, user_agent: &str, store: Arc<dyn SecretStore>) -> Self {
        Self {
            agent: slow_http_agent(api, user_agent, ANSWER_TIMEOUT),
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

    fn key(&self) -> Result<Secret, PlatformError> {
        self.store
            .read(API_KEY)
            .map_err(|error| PlatformError::Failed(error.to_string()))?
            .ok_or(PlatformError::NotConnected)
    }

    fn send(&self, key: &Secret, body: &str) -> Attempt<WrittenCopy> {
        let sent = self
            .agent
            .post(format!("{}/v1/messages", self.api))
            .header("Content-Type", "application/json")
            .header("x-api-key", key.expose())
            .header("anthropic-version", API_VERSION)
            .send(body);
        let response = match sent {
            Ok(response) => response,
            Err(error) => return Attempt::Again(PlatformError::Failed(error.to_string())),
        };
        match response.status().as_u16() {
            200 => Attempt::Done(read_json(response, "Anthropic").and_then(copy_in)),
            429 => Attempt::Again(PlatformError::RateLimited),
            status @ (500..=599) => Attempt::Again(PlatformError::Failed(format!(
                "Anthropic answered {status}"
            ))),
            401 => Attempt::Done(Err(PlatformError::Expired)),
            status @ (400 | 402..=499) => Attempt::Done(Err(PlatformError::Refused(
                read_json::<ErrorAnswer>(response, "Anthropic")
                    .map(|answer| answer.error.message)
                    .unwrap_or_else(|_| format!("Anthropic answered {status}")),
            ))),
            status => Attempt::Done(Err(PlatformError::Failed(format!(
                "Anthropic answered {status}"
            )))),
        }
    }
}

impl CopyWriter for Anthropic {
    fn write(&self, request: &CopyRequest) -> Result<WrittenCopy, PlatformError> {
        let key = self.key()?;
        let body = serde_json::json!({
            "model": request.model,
            "max_tokens": MAX_TOKENS,
            "system": request.instructions,
            "messages": [{ "role": "user", "content": request.prompt }],
            "output_config": {
                "format": {
                    "type": "json_schema",
                    "schema": {
                        "type": "object",
                        "properties": {
                            "title": {
                                "type": "string",
                                "description": "O título do anúncio, dentro do limite pedido."
                            },
                            "description": {
                                "type": "string",
                                "description": "A descrição do anúncio, em texto simples."
                            }
                        },
                        "required": ["title", "description"],
                        "additionalProperties": false
                    }
                }
            }
        })
        .to_string();
        retrying(self.pause.as_ref(), BACKOFF, || self.send(&key, &body))
    }
}

#[derive(Deserialize)]
struct MessageAnswer {
    #[serde(default)]
    content: Vec<ContentBlock>,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct CopyAnswer {
    title: String,
    description: String,
}

#[derive(Deserialize)]
struct ErrorAnswer {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct ErrorDetail {
    message: String,
}

/// The title and the description in a finished answer: its text block,
/// which the JSON schema shapes.
fn copy_in(answer: MessageAnswer) -> Result<WrittenCopy, PlatformError> {
    match answer.stop_reason.as_deref() {
        Some("refusal") => {
            return Err(PlatformError::Refused(
                "o modelo se recusou a escrever este anúncio".into(),
            ));
        }
        Some("max_tokens") => {
            return Err(PlatformError::Failed(
                "the answer reached its maximum size before it ended".into(),
            ));
        }
        _ => {}
    }
    let text = answer
        .content
        .iter()
        .find(|block| block.kind == "text")
        .map(|block| block.text.as_str())
        .ok_or_else(|| PlatformError::Failed("Anthropic answered without text".into()))?;
    let copy: CopyAnswer = serde_json::from_str(text).map_err(|error| {
        PlatformError::Failed(format!("Anthropic answered something unexpected: {error}"))
    })?;
    Ok(WrittenCopy {
        title: copy.title,
        description: copy.description,
    })
}
