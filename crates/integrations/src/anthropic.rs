//! The Claude API's adapter (#17): writes Listing Copy and the Pitch of an
//! Offer Post for marketing (ADR 0028, ADR 0036). Each request is one
//! `POST /v1/messages` with the owner's API key, asking for a JSON answer in
//! the request's shape: the title and the description, or the Pitch. The
//! text only ever becomes a draft the owner edits.

use std::sync::Arc;
use std::time::Duration;

use mascate_kernel::PlatformError;
use mascate_marketing::{CopyRequest, CopyShape, CopyWriter, Written, WrittenCopy};
use mascate_platform::{Secret, SecretStore, slow_http_agent};
use serde::Deserialize;
use serde_json::{Value, json};

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

    fn send(&self, key: &Secret, body: &str, shape: CopyShape) -> Attempt<Written> {
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
            200 => Attempt::Done(
                read_json(response, "Anthropic").and_then(|answer| written_in(answer, shape)),
            ),
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
    fn write(&self, request: &CopyRequest) -> Result<Written, PlatformError> {
        let key = self.key()?;
        let body = json!({
            "model": request.model,
            "max_tokens": MAX_TOKENS,
            "system": request.instructions,
            "messages": [{ "role": "user", "content": request.prompt }],
            "output_config": {
                "format": {
                    "type": "json_schema",
                    "schema": schema(request.shape)
                }
            }
        })
        .to_string();
        retrying(self.pause.as_ref(), BACKOFF, || {
            self.send(&key, &body, request.shape)
        })
    }
}

/// The JSON schema of the answer to a request for `shape`.
fn schema(shape: CopyShape) -> Value {
    match shape {
        CopyShape::Listing => json!({
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
        }),
        CopyShape::Pitch => json!({
            "type": "object",
            "properties": {
                "pitch": {
                    "type": "string",
                    "description": "A chamada do post: uma frase, numa linha, dentro do limite pedido."
                }
            },
            "required": ["pitch"],
            "additionalProperties": false
        }),
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
struct PitchAnswer {
    pitch: String,
}

#[derive(Deserialize)]
struct ErrorAnswer {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct ErrorDetail {
    message: String,
}

/// What a finished answer wrote in `shape`: its text block, which the JSON
/// schema shapes.
fn written_in(answer: MessageAnswer, shape: CopyShape) -> Result<Written, PlatformError> {
    match answer.stop_reason.as_deref() {
        Some("refusal") => {
            return Err(PlatformError::Refused(
                match shape {
                    CopyShape::Listing => "o modelo se recusou a escrever este anúncio",
                    CopyShape::Pitch => "o modelo se recusou a escrever esta chamada",
                }
                .into(),
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
    let unexpected = |error: serde_json::Error| {
        PlatformError::Failed(format!("Anthropic answered something unexpected: {error}"))
    };
    Ok(match shape {
        CopyShape::Listing => {
            let copy: CopyAnswer = serde_json::from_str(text).map_err(unexpected)?;
            Written::Listing(WrittenCopy {
                title: copy.title,
                description: copy.description,
            })
        }
        CopyShape::Pitch => {
            let answer: PitchAnswer = serde_json::from_str(text).map_err(unexpected)?;
            Written::Pitch(answer.pitch)
        }
    })
}
