//! Instagram's adapter (#156): Offer Posts on the owner's professional
//! account through the Instagram API with Instagram login,
//! `graph.instagram.com` v25.0 (ADR 0035, decisions 5 and 6). The login is
//! the token the Meta app dashboard generates, pasted by the owner: no
//! address to come back to and no client secret in the app. The token lasts
//! 60 days; the app renews it with `refresh_access_token` once it is a day
//! old and less than 30 days from running out, and keeps its term in the
//! secret store beside it. Publishing takes two steps: a container with the
//! photo's address and the caption, which Instagram fetches and readies,
//! then `media_publish`. The container's id is kept on the post in between,
//! so a post whose answer never came can be checked later by the container's
//! status. Instagram fetches the photo itself, so only a photo already on
//! the internet goes. Nothing before `media_publish` puts anything on the
//! account, so every failure up to it is a post that did not go out.

mod answers;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::TimeDelta;
use mascate_kernel::{Clock, PlatformError, Timestamp};
use mascate_marketing::{
    ConfirmedPost, DestinationCheck, DestinationStanding, PostDestination, PostFate, PostImage,
    PublishFailure, PublishedPost,
};
use mascate_platform::{Secret, SecretStore, http_agent, read_stored, slow_http_agent, stored};
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::answers::{https_link, page_on};
use crate::posting::{before_sending, never_sent};
use crate::retry::{Attempt, PAST_A_MINUTE, Pause, ThreadPause, retrying};
use answers::{
    Account, ContainerAnswer, Created, ErrorAnswer, GraphError, LimitAnswer, MeAnswer, MediaAnswer,
    TokenAnswer,
};

/// Instagram's API, unless a test or a development build points elsewhere.
pub const API_URL: &str = "https://graph.instagram.com";

/// The Graph API version the adapter was mapped from.
const VERSION: &str = "v25.0";

/// How long a token from the dashboard, or a renewed one, lasts.
pub const TOKEN_DAYS: i64 = 60;

/// Instagram renews only a token at least this old.
const RENEWABLE_AFTER: TimeDelta = TimeDelta::hours(24);

/// The app renews a token this close to running out.
const RENEW_WITHIN: TimeDelta = TimeDelta::days(30);

/// How long the app waits for Instagram to ready the photo, as the
/// documentation advises.
const READYING_FOR: Duration = Duration::from_secs(5 * 60);

/// The first wait between two looks at the container, doubling up to
/// [`LONGEST_LOOK_WAIT`].
const FIRST_LOOK_WAIT: Duration = Duration::from_secs(2);
const LONGEST_LOOK_WAIT: Duration = Duration::from_secs(60);

const ACCESS_TOKEN: &str = "INSTAGRAM_ACCESS_TOKEN";

/// When the stored token was handed out and until when it lasts. Not a
/// credential the owner pastes: the adapter keeps it beside the token.
const TOKEN_TERM: &str = "INSTAGRAM_TOKEN_TERM";

/// The owner's professional account, as Instagram names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstagramAccount {
    /// Instagram's id of the account, which publishing paths name.
    pub user_id: String,
    pub username: String,
}

impl InstagramAccount {
    /// The account's profile, where its posts show.
    pub fn profile(&self) -> String {
        format!("https://www.instagram.com/{}/", self.username)
    }
}

/// From when to when the stored token lasts, as far as the app knows: the
/// dashboard does not say when it made a token, so a pasted one counts from
/// when it was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Term {
    since: Timestamp,
    until: Timestamp,
}

impl Term {
    fn from(since: Timestamp, seconds: i64) -> Self {
        Self {
            since,
            until: since + TimeDelta::seconds(seconds),
        }
    }

    fn read(text: &str) -> Option<Self> {
        let (since, until) = text.split_once(' ')?;
        Some(Self {
            since: read_stored(since)?,
            until: read_stored(until)?,
        })
    }

    fn text(self) -> String {
        format!("{} {}", stored(self.since), stored(self.until))
    }

    fn renewable_at(self, now: Timestamp) -> bool {
        now - self.since >= RENEWABLE_AFTER && self.until - now < RENEW_WITHIN
    }
}

/// What a Graph API error means for the request.
enum Refusal {
    /// The token is no longer valid: code 190.
    Expired,
    /// Instagram throttles the app or the account; nothing was done.
    Throttled,
    /// The account published as many posts as the API allows for now.
    QuotaUsed,
    /// Instagram failed on its side.
    Transient(String),
    /// Instagram turned the request down, in its words.
    Refused(String),
}

/// Instagram's API, as the owner's Connection sees it.
pub struct Instagram {
    agent: ureq::Agent,
    api: String,
    user_agent: String,
    store: Arc<dyn SecretStore>,
    clock: Arc<dyn Clock>,
    pause: Arc<dyn Pause>,
    /// Held while the token is read and renewed, so two requests never
    /// renew it at once.
    renewing: Mutex<()>,
}

impl Instagram {
    /// The API at `api`, with the owner's token kept in `store`.
    pub fn new(
        api: &str,
        user_agent: &str,
        store: Arc<dyn SecretStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            agent: http_agent(api, user_agent, Duration::from_secs(60)),
            api: api.trim_end_matches('/').to_owned(),
            user_agent: user_agent.to_owned(),
            store,
            clock,
            pause: Arc::new(ThreadPause),
            renewing: Mutex::new(()),
        }
    }

    /// The same, waiting between retries and looks at a container with
    /// `pause`.
    pub fn with_pause(mut self, pause: Arc<dyn Pause>) -> Self {
        self.pause = pause;
        self
    }

    /// The same, giving up on an answer that takes longer than `timeout`.
    pub fn with_answer_timeout(mut self, timeout: Duration) -> Self {
        self.agent = slow_http_agent(&self.api, &self.user_agent, timeout);
        self
    }

    /// Notes that the owner just pasted a token from the dashboard: it
    /// lasts [`TOKEN_DAYS`] from now.
    pub fn token_pasted(&self) -> Result<(), PlatformError> {
        let _renewing = self.lock();
        self.write(
            TOKEN_TERM,
            &Term::from(self.clock.now(), TOKEN_DAYS * 24 * 60 * 60).text(),
        )
    }

    /// Until when the stored token lasts, as far as the app knows; `None`
    /// with no token stored.
    pub fn valid_until(&self) -> Result<Option<Timestamp>, PlatformError> {
        let _renewing = self.lock();
        match self.read(ACCESS_TOKEN) {
            Ok(_) => Ok(Some(self.term().until)),
            Err(PlatformError::NotConnected) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The account the token belongs to.
    pub fn account(&self) -> Result<InstagramAccount, PlatformError> {
        let me: MeAnswer = self.get("me", &[("fields", "user_id,username")])?;
        let account = me.account().and_then(|Account { user_id, username }| {
            let user_id = user_id.filter(|id| is_numeric_id(id))?;
            let username = username
                .map(|name| name.trim().to_owned())
                .filter(|name| is_username(name))?;
            Some(InstagramAccount { user_id, username })
        });
        account.ok_or_else(|| {
            PlatformError::Failed("Instagram answered without the account's id and name".into())
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.renewing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The token to call with, renewed first when it is inside the window.
    /// A renewal that fails for a moment keeps the old token, still valid.
    fn token(&self) -> Result<Secret, PlatformError> {
        let _renewing = self.lock();
        let token = self.read(ACCESS_TOKEN)?;
        if !self.term().renewable_at(self.clock.now()) {
            return Ok(token);
        }
        match self.renew(&token) {
            Ok(renewed) => Ok(renewed),
            Err(PlatformError::Expired) => Err(PlatformError::Expired),
            Err(_) => Ok(token),
        }
    }

    /// The stored term; a token with none, or with one the store or this
    /// app cannot read, counts from now.
    fn term(&self) -> Term {
        let stored = self.store.read(TOKEN_TERM).ok().flatten();
        if let Some(term) = stored.and_then(|text| Term::read(text.expose())) {
            return term;
        }
        let term = Term::from(self.clock.now(), TOKEN_DAYS * 24 * 60 * 60);
        // The term only says when to renew; a store that cannot keep it
        // must not stop a post.
        let _ = self.write(TOKEN_TERM, &term.text());
        term
    }

    /// A new token for `token`, in the store with its term before it is
    /// used.
    fn renew(&self, token: &Secret) -> Result<Secret, PlatformError> {
        let answer = retrying(self.pause.as_ref(), PAST_A_MINUTE, || {
            // The token travels in the query, as the endpoint asks, so no
            // error of this request is ever quoted.
            let sent = self
                .agent
                .get(format!("{}/refresh_access_token", self.api))
                .header("Accept", "application/json")
                .query("grant_type", "ig_refresh_token")
                .query("access_token", token.expose())
                .call();
            match sent {
                Ok(response) => attempt(response),
                Err(_) => Attempt::Again(PlatformError::Failed(
                    "could not reach Instagram to renew the token".into(),
                )),
            }
        })?;
        let fresh: TokenAnswer = read_json(answer)?;
        let renewed = fresh
            .access_token
            .filter(|token| !token.trim().is_empty() && fresh.expires_in > 0)
            .ok_or_else(|| {
                PlatformError::Failed("Instagram renewed the token without a new one".into())
            })?;
        self.write(ACCESS_TOKEN, renewed.trim())?;
        self.write(
            TOKEN_TERM,
            &Term::from(self.clock.now(), fresh.expires_in).text(),
        )?;
        Ok(Secret::new(renewed.trim()))
    }

    fn read(&self, name: &str) -> Result<Secret, PlatformError> {
        match self.store.read(name) {
            Ok(Some(secret)) if !secret.expose().trim().is_empty() => {
                Ok(Secret::new(secret.expose().trim()))
            }
            Ok(_) => Err(PlatformError::NotConnected),
            Err(error) => Err(PlatformError::Failed(error.to_string())),
        }
    }

    fn write(&self, name: &str, value: &str) -> Result<(), PlatformError> {
        self.store
            .write(name, &Secret::new(value))
            .map_err(|error| {
                PlatformError::Failed(format!("could not keep the Instagram token: {error}"))
            })
    }

    /// The JSON answer to `GET /v25.0/{path}` with `query`, asked again
    /// while Instagram throttles or fails for a moment.
    fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, PlatformError> {
        let token = self.token()?;
        let response = retrying(self.pause.as_ref(), PAST_A_MINUTE, || {
            let mut request = self
                .agent
                .get(format!("{}/{VERSION}/{path}", self.api))
                .header("Accept", "application/json")
                .header("Authorization", format!("Bearer {}", token.expose()));
            for (key, value) in query {
                request = request.query(*key, *value);
            }
            match request.call() {
                Ok(response) => attempt(response),
                Err(error) => Attempt::Again(PlatformError::Failed(error.to_string())),
            }
        })?;
        read_json(response)
    }

    /// `POST /v25.0/{path}` with the JSON `body`: Instagram's answer, or
    /// what kept the request from being answered. A throttled request did
    /// nothing, so it is sent again after a wait.
    fn post(&self, path: &str, body: &serde_json::Value) -> Result<Created, Posting> {
        let token = self.token().map_err(Posting::Login)?;
        let body = body.to_string();
        let mut waits = 0;
        let mut wait = PAST_A_MINUTE.first;
        loop {
            let sent = self
                .agent
                .post(format!("{}/{VERSION}/{path}", self.api))
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {}", token.expose()))
                .send(&body);
            let response = match sent {
                Ok(response) => response,
                Err(error) if never_sent(&error) => {
                    return Err(Posting::NeverSent(error.to_string()));
                }
                Err(error) => return Err(Posting::NoAnswer(error.to_string())),
            };
            let status = response.status().as_u16();
            if (200..300).contains(&status) {
                return read_json::<Created>(response)
                    .ok()
                    .filter(|created| is_numeric_id(&created.id))
                    .ok_or_else(|| {
                        Posting::NoAnswer("o Instagram respondeu sem o id do que criou".into())
                    });
            }
            match refusal(status, response) {
                Refusal::Throttled if waits < PAST_A_MINUTE.retries => {
                    self.pause.pause(wait);
                    wait *= 2;
                    waits += 1;
                }
                refusal => return Err(Posting::Refused(refusal)),
            }
        }
    }

    /// The container's state now.
    fn container(&self, container: &str) -> Result<ContainerAnswer, PlatformError> {
        self.get(container, &[("fields", "status_code,status")])
    }

    /// Looks at the container until Instagram has readied it, for up to
    /// [`READYING_FOR`]: whether it is already published.
    fn wait_until_ready(&self, container: &str) -> Result<bool, PublishFailure> {
        let mut waited = Duration::ZERO;
        let mut wait = FIRST_LOOK_WAIT;
        loop {
            let found = self.container(container).map_err(before_sending)?;
            match found.status_code.as_str() {
                "FINISHED" => return Ok(false),
                "PUBLISHED" => return Ok(true),
                "IN_PROGRESS" if waited < READYING_FOR => {
                    self.pause.pause(wait);
                    waited += wait;
                    wait = (wait * 2).min(LONGEST_LOOK_WAIT);
                }
                "IN_PROGRESS" => {
                    return Err(PublishFailure::NotSent(
                        "o Instagram não terminou de preparar a foto em 5 minutos".into(),
                    ));
                }
                "ERROR" => {
                    let status = found.status.unwrap_or_default();
                    let why = status
                        .split(|c: char| !c.is_ascii_digit())
                        .filter_map(|digits| digits.parse().ok())
                        .find_map(photo_problem)
                        .map(str::to_owned)
                        .or_else(|| Some(status.trim().to_owned()).filter(|why| !why.is_empty()))
                        .unwrap_or_else(|| "o Instagram não conseguiu preparar a foto".into());
                    return Err(PublishFailure::Refused(why));
                }
                "EXPIRED" => {
                    return Err(PublishFailure::NotSent(
                        "o post preparado no Instagram expirou antes de sair".into(),
                    ));
                }
                other => {
                    return Err(PublishFailure::NotSent(format!(
                        "o Instagram respondeu um estado que o app não conhece: {other}"
                    )));
                }
            }
        }
    }

    /// Where the published `media` opens: its permalink, or the account's
    /// profile when Instagram does not say.
    fn permalink(&self, media: &str, account: &InstagramAccount) -> String {
        self.get::<MediaAnswer>(media, &[("fields", "permalink")])
            .ok()
            .and_then(|answer| answer.permalink)
            .and_then(|link| page_on(&link, "instagram.com"))
            .unwrap_or_else(|| account.profile())
    }
}

/// What kept a `POST` from being answered.
enum Posting {
    /// Getting the token failed; nothing was sent.
    Login(PlatformError),
    /// The request never left.
    NeverSent(String),
    /// The request left and no answer came back.
    NoAnswer(String),
    Refused(Refusal),
}

impl PostDestination for Instagram {
    /// Whether the token reaches a professional account: the account's
    /// name. Instagram has no place to check beyond it.
    fn check(&self, _chat: &str) -> Result<DestinationCheck, PlatformError> {
        let account = self.account()?;
        Ok(DestinationCheck {
            title: format!("@{}", account.username),
            standing: DestinationStanding::CanPost,
        })
    }

    /// Checks the quota and creates the container with the photo's address
    /// and the caption. A container publishes nothing on its own: it lapses
    /// after 24 hours.
    fn stage(&self, post: &ConfirmedPost) -> Result<Option<String>, PublishFailure> {
        let photo = match post.image() {
            Some(PostImage::Url(url)) => https_link(url).ok_or_else(|| {
                PublishFailure::NotSent(
                    "o endereço da foto não é HTTPS, e o Instagram só busca foto assim".into(),
                )
            })?,
            Some(PostImage::File(_)) => {
                return Err(PublishFailure::NotSent(
                    "o Instagram só publica foto que já está na internet, e esta está na pasta \
                     do produto"
                        .into(),
                ));
            }
            None => {
                return Err(PublishFailure::NotSent(
                    "o Instagram só publica post com foto".into(),
                ));
            }
        };
        let account = self.account().map_err(before_sending)?;
        let limit: LimitAnswer = self
            .get(
                &format!("{}/content_publishing_limit", account.user_id),
                &[("fields", "quota_usage,config")],
            )
            .map_err(before_sending)?;
        if limit.used_up() {
            return Err(PublishFailure::QuotaUsed);
        }
        let created = self.post(
            &format!("{}/media", account.user_id),
            &json!({ "image_url": photo, "caption": post.text() }),
        );
        match created {
            Ok(container) => Ok(Some(container.id)),
            Err(Posting::Login(error)) => Err(before_sending(error)),
            // An unanswered container publishes nothing.
            Err(Posting::NeverSent(why) | Posting::NoAnswer(why)) => {
                Err(PublishFailure::NotSent(why))
            }
            Err(Posting::Refused(refusal)) => Err(match refusal {
                Refusal::Expired => PublishFailure::Expired,
                Refusal::Throttled => PublishFailure::RateLimited,
                Refusal::QuotaUsed => PublishFailure::QuotaUsed,
                Refusal::Transient(why) => PublishFailure::NotSent(why),
                Refusal::Refused(why) => PublishFailure::Refused(why),
            }),
        }
    }

    /// Waits for Instagram to ready the container [`Self::stage`] created,
    /// then publishes it. Only an unanswered `media_publish` leaves the post
    /// unconfirmed.
    fn publish(&self, post: &ConfirmedPost) -> Result<PublishedPost, PublishFailure> {
        let container = post
            .staged()
            .filter(|container| is_numeric_id(container))
            .ok_or_else(|| PublishFailure::NotSent("o Instagram não preparou o post".into()))?;
        let account = self.account().map_err(before_sending)?;
        if self.wait_until_ready(container)? {
            return Ok(PublishedPost {
                message_id: container.to_owned(),
                link: Some(account.profile()),
            });
        }
        let published = self.post(
            &format!("{}/media_publish", account.user_id),
            &json!({ "creation_id": container }),
        );
        let media = match published {
            Ok(media) => media.id,
            Err(Posting::Login(error)) => return Err(before_sending(error)),
            Err(Posting::NeverSent(why)) => return Err(PublishFailure::NotSent(why)),
            Err(Posting::NoAnswer(why)) => return Err(PublishFailure::Unconfirmed(why)),
            Err(Posting::Refused(refusal)) => {
                return Err(match refusal {
                    Refusal::Expired => PublishFailure::Expired,
                    Refusal::Throttled => PublishFailure::RateLimited,
                    Refusal::QuotaUsed => PublishFailure::QuotaUsed,
                    Refusal::Transient(why) => PublishFailure::Unconfirmed(why),
                    Refusal::Refused(why) => PublishFailure::Refused(why),
                });
            }
        };
        let link = self.permalink(&media, &account);
        Ok(PublishedPost {
            message_id: media,
            link: Some(link),
        })
    }

    /// The container's status says what became of the post: published,
    /// still ready to publish, failed or lapsed.
    fn fate(&self, container: &str) -> Result<PostFate, PlatformError> {
        if !is_numeric_id(container) {
            return Ok(PostFate::Unknown);
        }
        let found = self.container(container)?;
        Ok(match found.status_code.as_str() {
            "PUBLISHED" => PostFate::Published(PublishedPost {
                message_id: container.to_owned(),
                link: Some(self.account()?.profile()),
            }),
            "FINISHED" | "ERROR" | "EXPIRED" => PostFate::NotPublished,
            _ => PostFate::Unknown,
        })
    }
}

/// What Instagram's answer to a `GET` means, and whether to ask again.
fn attempt(
    response: ureq::http::Response<ureq::Body>,
) -> Attempt<ureq::http::Response<ureq::Body>> {
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        return Attempt::Done(Ok(response));
    }
    match refusal(status, response) {
        Refusal::Expired => Attempt::Done(Err(PlatformError::Expired)),
        Refusal::Throttled => Attempt::Again(PlatformError::RateLimited),
        Refusal::QuotaUsed => Attempt::Done(Err(PlatformError::RateLimited)),
        Refusal::Transient(why) => Attempt::Again(PlatformError::Failed(why)),
        Refusal::Refused(why) => Attempt::Done(Err(PlatformError::Refused(why))),
    }
}

/// What the error Instagram answered with `status` means. Errors are told
/// apart by their code, never by the message.
fn refusal(status: u16, response: ureq::http::Response<ureq::Body>) -> Refusal {
    let error: Option<GraphError> = read_json::<ErrorAnswer>(response)
        .ok()
        .and_then(|answer| answer.error);
    let why = error
        .as_ref()
        .and_then(|error| {
            photo_problem(error.error_subcode)
                .map(str::to_owned)
                .or_else(|| error.why())
        })
        .unwrap_or_else(|| format!("o Instagram respondeu {status}"));
    let (code, subcode, transient) = error.as_ref().map_or((0, 0, false), |error| {
        (error.code, error.error_subcode, error.is_transient)
    });
    match (code, subcode) {
        // An invalid or lapsed token, or a session the owner ended.
        (190 | 102, _) => Refusal::Expired,
        _ if status == 401 => Refusal::Expired,
        // The app's, the account's or a call's rate limit.
        (4 | 17 | 32 | 613, _) => Refusal::Throttled,
        _ if status == 429 => Refusal::Throttled,
        // "You reached maximum number of posts".
        (9, _) | (_, 2207042) => Refusal::QuotaUsed,
        _ if transient || status >= 500 => Refusal::Transient(why),
        _ => Refusal::Refused(why),
    }
}

/// What a photo refusal's subcode means, as Instagram's error reference
/// explains it.
fn photo_problem(subcode: i64) -> Option<&'static str> {
    Some(match subcode {
        2207003 => "o Instagram demorou demais para baixar a foto",
        2207004 => "a foto passa de 8 MB, o maior arquivo que o Instagram baixa",
        2207005 => "o Instagram só aceita foto em JPEG",
        2207009 => "a proporção da foto fica fora do que o Instagram aceita, de 4:5 a 1,91:1",
        2207052 => "o Instagram não conseguiu baixar a foto do endereço dela",
        _ => return None,
    })
}

fn read_json<T: DeserializeOwned>(
    response: ureq::http::Response<ureq::Body>,
) -> Result<T, PlatformError> {
    crate::answers::read_json(response, "Instagram")
}

/// Instagram's ids are digits, so an id read from it, or kept on a post,
/// never reaches another path.
fn is_numeric_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|byte| byte.is_ascii_digit())
}

/// Instagram's usernames: letters, digits, `.` and `_`, up to 30.
fn is_username(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 30
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'_')
}
