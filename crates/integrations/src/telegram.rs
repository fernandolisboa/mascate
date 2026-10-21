//! Telegram's Bot API adapter (#103): publishes Offer Posts on the owner's
//! channels for marketing (ADR 0031, decisions 6 and 7), mapped from Bot
//! API 10.3. Every call is `POST {API}/bot<token>/<method>`, answered with
//! the `ok`/`result`/`error_code`/`parameters` envelope; errors are told
//! apart by `error_code` and `parameters`, never by the `description` text.
//! The text goes without `parse_mode`, so nothing in it needs escaping. The
//! API has no idempotency key: only a 429, which publishes nothing, is
//! tried again, after the `retry_after` it asks for.

use std::sync::Arc;
use std::time::Duration;

use mascate_kernel::PlatformError;
use mascate_marketing::{
    ConfirmedPost, DestinationCheck, DestinationStanding, PostDestination, PostImage,
    PublishFailure, PublishedPost,
};
use mascate_platform::{Secret, SecretStore, http_agent, slow_http_agent};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::posting::{Photo, never_sent, read_photo};
use crate::retry::{Pause, ThreadPause};

/// Telegram's Bot API, unless a test points elsewhere.
pub const API_URL: &str = "https://api.telegram.org";

const BOT_TOKEN: &str = "TELEGRAM_BOT_TOKEN";

/// Answers are small JSON documents; anything bigger is not one.
const MAX_ANSWER_BYTES: u64 = 2 * 1024 * 1024;

/// The biggest photo `sendPhoto` takes as an upload.
pub const MAX_PHOTO_BYTES: u64 = 10 * 1024 * 1024;

/// How many times a 429 is waited out before giving up.
const RATE_LIMIT_RETRIES: u32 = 3;

/// A wait longer than this is not waited out: the owner tries again later.
const LONGEST_WAIT: Duration = Duration::from_secs(60);

/// A 429 without `retry_after` waits this long.
const DEFAULT_WAIT: Duration = Duration::from_secs(1);

/// The Bot API, with the owner's bot token.
pub struct Telegram {
    agent: ureq::Agent,
    api: String,
    user_agent: String,
    store: Arc<dyn SecretStore>,
    pause: Arc<dyn Pause>,
}

impl Telegram {
    /// The API at `api`, with the token kept in `store`.
    pub fn new(api: &str, user_agent: &str, store: Arc<dyn SecretStore>) -> Self {
        Self {
            agent: http_agent(api, user_agent, Duration::from_secs(30)),
            api: api.trim_end_matches('/').to_owned(),
            user_agent: user_agent.to_owned(),
            store,
            pause: Arc::new(ThreadPause),
        }
    }

    /// The same, waiting out a 429 with `pause`.
    pub fn with_pause(mut self, pause: Arc<dyn Pause>) -> Self {
        self.pause = pause;
        self
    }

    /// The same, giving up on an answer that takes longer than `timeout`.
    pub fn with_answer_timeout(mut self, timeout: Duration) -> Self {
        self.agent = slow_http_agent(&self.api, &self.user_agent, timeout);
        self
    }

    /// The bot's token, if it can go in a URL path.
    fn token(&self) -> Result<Secret, PublishFailure> {
        let token = self
            .store
            .read(BOT_TOKEN)
            .map_err(|error| PublishFailure::NotSent(error.to_string()))?
            .ok_or(PublishFailure::NotConnected)?;
        let well_formed = !token.expose().is_empty()
            && token
                .expose()
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'));
        if well_formed {
            Ok(token)
        } else {
            Err(PublishFailure::Expired)
        }
    }

    /// Calls `method`, waiting out each 429 the API asks to.
    fn call<T: DeserializeOwned>(
        &self,
        token: &Secret,
        method: &str,
        body: &Body,
    ) -> Result<T, PublishFailure> {
        let mut waits = 0;
        loop {
            match self.attempt(token, method, body) {
                Ok(result) => return Ok(result),
                Err(Refusal::Wait(wait)) if waits < RATE_LIMIT_RETRIES && wait <= LONGEST_WAIT => {
                    self.pause.pause(wait);
                    waits += 1;
                }
                Err(Refusal::Wait(_)) => return Err(PublishFailure::RateLimited),
                Err(Refusal::Failed(failure)) => return Err(failure),
            }
        }
    }

    fn attempt<T: DeserializeOwned>(
        &self,
        token: &Secret,
        method: &str,
        body: &Body,
    ) -> Result<T, Refusal> {
        let url = format!("{}/bot{}/{method}", self.api, token.expose());
        let request = self.agent.post(url);
        let sent = match body {
            Body::Json(value) => request
                .header("Content-Type", "application/json")
                .send(value.to_string()),
            Body::Multipart { boundary, bytes } => request
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .send(bytes.as_slice()),
        };
        let response = sent.map_err(|error| {
            let text = hidden(error.to_string(), token);
            Refusal::Failed(if never_sent(&error) {
                PublishFailure::NotSent(text)
            } else {
                PublishFailure::Unconfirmed(text)
            })
        })?;
        let status = response.status().as_u16();
        let bytes = response
            .into_body()
            .into_with_config()
            .limit(MAX_ANSWER_BYTES)
            .read_to_vec()
            .map_err(|error| {
                Refusal::Failed(PublishFailure::Unconfirmed(hidden(
                    error.to_string(),
                    token,
                )))
            })?;
        let envelope: Option<Envelope<T>> = serde_json::from_slice(&bytes).ok();
        let Some(envelope) = envelope else {
            return Err(Refusal::Failed(PublishFailure::Unconfirmed(format!(
                "o Telegram respondeu {status} sem a resposta da Bot API"
            ))));
        };
        if envelope.ok {
            return envelope.result.ok_or_else(|| {
                Refusal::Failed(PublishFailure::Unconfirmed(
                    "o Telegram respondeu ok sem o post".into(),
                ))
            });
        }
        let code = envelope.error_code.unwrap_or(status);
        let description = envelope
            .description
            .unwrap_or_else(|| format!("o Telegram respondeu {code}"));
        Err(match code {
            429 => Refusal::Wait(
                envelope
                    .parameters
                    .and_then(|parameters| parameters.retry_after)
                    .map_or(DEFAULT_WAIT, Duration::from_secs),
            ),
            // A token Telegram does not know is 401; one it cannot even
            // read as a token is 404 on every method.
            401 | 404 => Refusal::Failed(PublishFailure::Expired),
            500..=599 => Refusal::Failed(PublishFailure::Unconfirmed(description)),
            _ => Refusal::Failed(PublishFailure::Refused(description)),
        })
    }
}

impl PostDestination for Telegram {
    fn check(&self, chat: &str) -> Result<DestinationCheck, PlatformError> {
        let checked = || {
            let token = self.token()?;
            let bot: User = self.call(&token, "getMe", &Body::Json(json!({})))?;
            let found: Chat = self.call(
                &token,
                "getChat",
                &Body::Json(json!({ "chat_id": chat_id(chat) })),
            )?;
            let title = found.title.clone().unwrap_or_else(|| chat.to_owned());
            if found.kind != "channel" {
                return Ok(DestinationCheck {
                    title,
                    standing: DestinationStanding::NotAChannel,
                });
            }
            let member: ChatMember = self.call(
                &token,
                "getChatMember",
                &Body::Json(json!({ "chat_id": chat_id(chat), "user_id": bot.id })),
            )?;
            let standing = match member.status.as_str() {
                "creator" => DestinationStanding::CanPost,
                "administrator" if member.can_post_messages == Some(true) => {
                    DestinationStanding::CanPost
                }
                "administrator" => DestinationStanding::CannotPost,
                _ => DestinationStanding::NotAdministrator,
            };
            Ok(DestinationCheck { title, standing })
        };
        checked().map_err(|failure| match failure {
            PublishFailure::NotConnected => PlatformError::NotConnected,
            PublishFailure::Expired => PlatformError::Expired,
            PublishFailure::RateLimited | PublishFailure::QuotaUsed => PlatformError::RateLimited,
            PublishFailure::Refused(why) => PlatformError::Refused(why),
            PublishFailure::NotSent(why) | PublishFailure::Unconfirmed(why) => {
                PlatformError::Failed(why)
            }
            PublishFailure::Interrupted | PublishFailure::NotPublished => {
                PlatformError::Failed(failure.to_string())
            }
        })
    }

    fn publish(&self, post: &ConfirmedPost) -> Result<PublishedPost, PublishFailure> {
        let token = self.token()?;
        let chat = chat_id(post.chat());
        let (method, body) = match post.image() {
            None => (
                "sendMessage",
                Body::Json(json!({ "chat_id": chat, "text": post.text() })),
            ),
            Some(PostImage::Url(url)) => (
                "sendPhoto",
                Body::Json(json!({ "chat_id": chat, "photo": url, "caption": post.text() })),
            ),
            Some(PostImage::File(path)) => (
                "sendPhoto",
                multipart(
                    &[("chat_id", post.chat()), ("caption", post.text())],
                    &read_photo(path, "Telegram", MAX_PHOTO_BYTES)?,
                ),
            ),
        };
        let message: Message = self.call(&token, method, &body)?;
        Ok(PublishedPost {
            message_id: message.message_id.to_string(),
            link: message.link(),
        })
    }
}

/// A request body.
enum Body {
    Json(Value),
    Multipart { boundary: String, bytes: Vec<u8> },
}

/// Why an attempt did not give a result.
enum Refusal {
    /// Telegram published nothing and asked to wait this long.
    Wait(Duration),
    Failed(PublishFailure),
}

/// `text` without the bot token, which is part of every URL.
fn hidden(text: String, token: &Secret) -> String {
    text.replace(token.expose(), "<token>")
}

/// A chat as the API takes it: `@name` as text, a channel's id as a
/// number.
fn chat_id(chat: &str) -> Value {
    match chat.parse::<i64>() {
        Ok(id) => json!(id),
        Err(_) => json!(chat),
    }
}

/// A `multipart/form-data` body with `fields` and `photo` as the `photo`
/// file, under a boundary found in none of them.
fn multipart(fields: &[(&str, &str)], photo: &Photo) -> Body {
    let mut boundary = String::from("MascateBoundary");
    let mut tries = 0u32;
    while contains(&photo.bytes, boundary.as_bytes())
        || fields
            .iter()
            .any(|(_, value)| value.contains(boundary.as_str()))
    {
        tries += 1;
        boundary = format!("MascateBoundary{tries:08x}");
    }
    let mut bytes = Vec::with_capacity(photo.bytes.len() + 1024);
    for (name, value) in fields {
        bytes.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    bytes.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"photo\"; \
             filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
            photo.name, photo.content_type
        )
        .as_bytes(),
    );
    bytes.extend_from_slice(&photo.bytes);
    bytes.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Body::Multipart { boundary, bytes }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[derive(Deserialize)]
struct Envelope<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
    error_code: Option<u16>,
    parameters: Option<Parameters>,
}

#[derive(Deserialize)]
struct Parameters {
    retry_after: Option<u64>,
}

#[derive(Deserialize)]
struct User {
    id: i64,
}

#[derive(Deserialize)]
struct Chat {
    id: i64,
    #[serde(rename = "type")]
    kind: String,
    title: Option<String>,
    username: Option<String>,
}

#[derive(Deserialize)]
struct ChatMember {
    status: String,
    can_post_messages: Option<bool>,
}

#[derive(Deserialize)]
struct Message {
    message_id: i64,
    chat: Chat,
}

impl Message {
    /// `t.me/<name>/<id>` on a public channel, `t.me/c/<id without
    /// -100>/<id>` on a private one, where only members open it.
    fn link(&self) -> Option<String> {
        if let Some(username) = &self.chat.username {
            return Some(format!("https://t.me/{username}/{}", self.message_id));
        }
        self.chat
            .id
            .to_string()
            .strip_prefix("-100")
            .map(|channel| format!("https://t.me/c/{channel}/{}", self.message_id))
    }
}
