//! Pinterest's adapter (#155): Offer Posts as Pins on a board of the
//! owner's account, through Pinterest API v5 (ADR 0035, decision 7). The
//! login is OAuth's authorization code: the owner authorizes the app on
//! Pinterest's page, the browser stops at `http://localhost/` with the code,
//! and the owner pastes that address back. The access token lasts 30 days
//! and the refresh token 60, renewed on every use, so renewals run one at a
//! time and the new refresh token is in the store before the access token is
//! used. A Pin carries the post's text as its description, the product as
//! its title, the photo (bytes from the Product's folder, or the Platform's
//! address) and the whole Affiliate Link: Pinterest refuses some shorteners
//! and links that redirect. `POST /v5/pins` has no idempotency key, so only
//! a 429, which creates nothing, is tried again; a 401 is answered before
//! anything is created, so it renews the login once and tries again. With
//! Trial access the Pins are seen only by the owner, until Pinterest grants
//! Standard access.

mod answers;

use std::sync::Arc;
use std::time::Duration;

use chrono::TimeDelta;
use mascate_kernel::{Clock, PlatformError};
use mascate_marketing::{
    ConfirmedPost, DestinationCheck, DestinationStanding, PinterestBoard, PostDestination,
    PostImage, PublishFailure, PublishedPost,
};
use mascate_platform::{Secret, SecretStore, http_agent, slow_http_agent};
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::answers::https_link;
use crate::base64::encoded;
use crate::escaped::escaped;
use crate::posting::{before_sending, never_sent, read_photo};
use crate::retry::{Attempt, PAST_A_MINUTE, Pause, ThreadPause, retrying};
use crate::single_use_tokens::{Fresh, Tokens};
use answers::{BoardAnswer, BoardsPage, ErrorAnswer, PinAnswer, TokenAnswer};

/// Pinterest's API, unless a test or a development build points elsewhere.
pub const API_URL: &str = "https://api.pinterest.com";

/// Pinterest's sandbox, for a development build with a sandbox token.
pub const SANDBOX_URL: &str = "https://api-sandbox.pinterest.com";

/// Where the owner authorizes the app.
pub const LOGIN_URL: &str = "https://www.pinterest.com/oauth/";

/// Where Pinterest sends the browser back with the code. Nothing answers
/// there: the owner copies the address the browser stopped at.
pub const REDIRECT: &str = "http://localhost/";

/// Reading the boards to pick one, and creating Pins on it.
pub const SCOPES: &str = "boards:read,boards:write,pins:read,pins:write";

/// The biggest photo the app sends as bytes.
pub const MAX_PHOTO_BYTES: u64 = 20 * 1024 * 1024;

const APP_ID: &str = "PINTEREST_APP_ID";
const APP_SECRET: &str = "PINTEREST_APP_SECRET";
const REFRESH_TOKEN: &str = "PINTEREST_REFRESH_TOKEN";

/// The most boards Pinterest lists on one page.
const PAGE_SIZE: &str = "250";

/// A guard against a bookmark that never ends: 50 pages of 250 boards.
const MAX_PAGES: usize = 50;

/// What the browser lands on after the owner authorizes the app on
/// Pinterest's page: a one-time code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinterestAuthorization {
    pub code: String,
}

impl PinterestAuthorization {
    /// The code in the address the browser landed on, or in its query
    /// alone; `None` when it is missing or malformed.
    pub fn from_landing(address: &str) -> Option<Self> {
        crate::landing::code_in(address).map(|code| Self { code })
    }
}

/// Pinterest's API, as the owner's Connection sees it.
pub struct Pinterest {
    agent: ureq::Agent,
    api: String,
    login: String,
    user_agent: String,
    store: Arc<dyn SecretStore>,
    clock: Arc<dyn Clock>,
    pause: Arc<dyn Pause>,
    tokens: Tokens,
}

impl Pinterest {
    /// The API at `api`, logging in at `login`, with the app's keys and the
    /// owner's login kept in `store`.
    pub fn new(
        api: &str,
        login: &str,
        user_agent: &str,
        store: Arc<dyn SecretStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            agent: http_agent(api, user_agent, Duration::from_secs(60)),
            api: api.trim_end_matches('/').to_owned(),
            login: login.to_owned(),
            user_agent: user_agent.to_owned(),
            tokens: Tokens::new(clock.clone()),
            store,
            clock,
            pause: Arc::new(ThreadPause),
        }
    }

    /// The same, waiting between retries with `pause`.
    pub fn with_pause(mut self, pause: Arc<dyn Pause>) -> Self {
        self.pause = pause;
        self
    }

    /// The same, giving up on an answer that takes longer than `timeout`.
    pub fn with_answer_timeout(mut self, timeout: Duration) -> Self {
        self.agent = slow_http_agent(&self.api, &self.user_agent, timeout);
        self
    }

    /// Pinterest's page where the owner logs in and authorizes the app,
    /// sending the browser to [`REDIRECT`] with a code.
    pub fn authorization_url(&self) -> Result<String, PlatformError> {
        let app = self.read(APP_ID)?;
        Ok(format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope={}",
            self.login,
            escaped(app.expose().trim()),
            escaped(REDIRECT),
            escaped(SCOPES)
        ))
    }

    /// Trades the one-time code of `authorization` for the owner's login,
    /// kept in the store before anything else uses it.
    pub fn authorize(&self, authorization: &PinterestAuthorization) -> Result<(), PlatformError> {
        let basic = self.basic()?;
        self.tokens.replace(|| {
            let answer = self.token_call(
                &basic,
                &[
                    ("grant_type", "authorization_code"),
                    ("code", &authorization.code),
                    ("redirect_uri", REDIRECT),
                ],
            )?;
            let fresh = match answer {
                Ok(answer) => self.fresh(answer, None)?,
                // A code past its time, already used, or given for another
                // address.
                Err(why) => return Err(PlatformError::Refused(why)),
            };
            self.write(REFRESH_TOKEN, fresh.refresh_token.expose())?;
            Ok(fresh)
        })
    }

    /// A new access token from the stored refresh token, the next refresh
    /// token written to the store before it is returned.
    fn renew(&self) -> Result<Fresh, PlatformError> {
        let basic = self.basic()?;
        let refresh_token = self.read(REFRESH_TOKEN)?;
        let answer = self.token_call(
            &basic,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.expose()),
            ],
        )?;
        // The refresh token lapsed after 60 days unused, the owner took the
        // authorization back, or the app's secret changed.
        let answer = answer.map_err(|_| PlatformError::Expired)?;
        let fresh = self.fresh(answer, Some(refresh_token))?;
        self.write(REFRESH_TOKEN, fresh.refresh_token.expose())?;
        Ok(fresh)
    }

    /// The app's keys as HTTP Basic credentials.
    fn basic(&self) -> Result<Secret, PlatformError> {
        let (app, secret) = (self.read(APP_ID)?, self.read(APP_SECRET)?);
        let pair = format!("{}:{}", app.expose().trim(), secret.expose().trim());
        Ok(Secret::new(format!("Basic {}", encoded(pair.as_bytes()))))
    }

    /// Pinterest's answer to the token request with `form`: the tokens, or
    /// its reason for turning the request down; retried while it throttles
    /// or fails for a moment.
    fn token_call(
        &self,
        basic: &Secret,
        form: &[(&str, &str)],
    ) -> Result<Result<TokenAnswer, String>, PlatformError> {
        retrying(self.pause.as_ref(), PAST_A_MINUTE, || {
            let sent = self
                .agent
                .post(format!("{}/v5/oauth/token", self.api))
                .header("Accept", "application/json")
                .header("Authorization", basic.expose())
                .send_form(form.iter().copied());
            let response = match sent {
                Ok(response) => response,
                // The secrets travel in a header and the body, which no
                // error quotes.
                Err(error) => return Attempt::Again(PlatformError::Failed(error.to_string())),
            };
            match response.status().as_u16() {
                200 => Attempt::Done(read_json(response).map(Ok)),
                400 | 401 | 403 => Attempt::Done(Ok(Err(read_json::<ErrorAnswer>(response)
                    .ok()
                    .and_then(ErrorAnswer::why)
                    .unwrap_or_else(|| "refused".into())))),
                429 => Attempt::Again(PlatformError::RateLimited),
                status @ 500..=599 => Attempt::Again(PlatformError::Failed(format!(
                    "Pinterest answered {status} for the login"
                ))),
                status => Attempt::Done(Err(PlatformError::Failed(format!(
                    "Pinterest answered {status} for the login"
                )))),
            }
        })
    }

    /// The tokens in `answer`; a renewal that hands out no new refresh
    /// token keeps `previous`.
    fn fresh(&self, answer: TokenAnswer, previous: Option<Secret>) -> Result<Fresh, PlatformError> {
        let refresh_token = answer
            .refresh_token
            .filter(|token| !token.is_empty())
            .map(Secret::new)
            .or(previous);
        match (answer.access_token, refresh_token) {
            (Some(access), Some(refresh)) if !access.is_empty() => Ok(Fresh {
                access_token: Secret::new(access),
                refresh_token: refresh,
                expires_at: self.clock.now() + TimeDelta::seconds(answer.expires_in),
            }),
            _ => Err(PlatformError::Failed(
                "Pinterest answered a login without its tokens".into(),
            )),
        }
    }

    fn read(&self, name: &str) -> Result<Secret, PlatformError> {
        match self.store.read(name) {
            Ok(Some(secret)) if !secret.expose().trim().is_empty() => Ok(secret),
            Ok(_) => Err(PlatformError::NotConnected),
            Err(error) => Err(PlatformError::Failed(error.to_string())),
        }
    }

    fn write(&self, name: &str, value: &str) -> Result<(), PlatformError> {
        self.store
            .write(name, &Secret::new(value))
            .map_err(|error| {
                PlatformError::Failed(format!("could not keep the Pinterest login: {error}"))
            })
    }

    /// The boards of the owner's account, every page of them, in
    /// Pinterest's order.
    pub fn boards(&self) -> Result<Vec<PinterestBoard>, PlatformError> {
        let mut boards = Vec::new();
        let mut bookmark: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query = vec![("page_size", PAGE_SIZE)];
            if let Some(bookmark) = bookmark.as_deref() {
                query.push(("bookmark", bookmark));
            }
            let page: BoardsPage = self.get("/v5/boards", &query)?;
            boards.extend(page.items.into_iter().filter_map(BoardAnswer::board));
            bookmark = page.bookmark.filter(|bookmark| !bookmark.is_empty());
            if bookmark.is_none() {
                return Ok(boards);
            }
        }
        Err(PlatformError::Failed(format!(
            "Pinterest kept listing boards past {MAX_PAGES} pages"
        )))
    }

    /// The JSON answer to `GET path` with `query`.
    fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, PlatformError> {
        let mut renewed = false;
        loop {
            let token = self.tokens.access_token(|| self.renew())?;
            let answer = retrying(self.pause.as_ref(), PAST_A_MINUTE, || {
                let mut request = self
                    .agent
                    .get(format!("{}{path}", self.api))
                    .header("Accept", "application/json")
                    .header("Authorization", format!("Bearer {}", token.expose()));
                for (key, value) in query {
                    request = request.query(*key, *value);
                }
                match request.call() {
                    Ok(response) => attempt(response, path),
                    Err(error) => Attempt::Again(PlatformError::Failed(error.to_string())),
                }
            });
            match answer {
                Err(PlatformError::Expired) if !renewed => {
                    self.tokens.forget(&token);
                    renewed = true;
                }
                answer => return answer.and_then(read_json),
            }
        }
    }

    /// The Pin `post` makes: its board, title, description, alt text, link
    /// and photo. Nothing has been sent when it fails.
    fn pin(post: &ConfirmedPost) -> Result<String, PublishFailure> {
        let media = match post.image() {
            Some(PostImage::File(path)) => {
                let photo = read_photo(path, "Pinterest", MAX_PHOTO_BYTES)?;
                json!({
                    "source_type": "image_base64",
                    "content_type": photo.content_type,
                    "data": encoded(&photo.bytes),
                })
            }
            Some(PostImage::Url(url)) => {
                let url = https_link(url).ok_or_else(|| {
                    PublishFailure::NotSent(
                        "o endereço da foto não é HTTPS, e o Pinterest só busca foto assim".into(),
                    )
                })?;
                json!({ "source_type": "image_url", "url": url })
            }
            None => {
                return Err(PublishFailure::NotSent(
                    "o Pinterest só publica Pin com foto".into(),
                ));
            }
        };
        Ok(json!({
            "board_id": post.chat(),
            "title": post.title(),
            "description": post.text(),
            "alt_text": post.title(),
            "link": post.link(),
            "media_source": media,
        })
        .to_string())
    }
}

impl PostDestination for Pinterest {
    /// Whether the board is still on the owner's account.
    fn check(&self, board: &str) -> Result<DestinationCheck, PlatformError> {
        let found: BoardAnswer = self.get(&format!("/v5/boards/{}", board_segment(board)?), &[])?;
        Ok(DestinationCheck {
            title: found.name,
            standing: DestinationStanding::CanPost,
        })
    }

    fn publish(&self, post: &ConfirmedPost) -> Result<PublishedPost, PublishFailure> {
        board_segment(post.chat()).map_err(|_| {
            PublishFailure::Refused("a pasta do canal não é uma pasta do Pinterest".into())
        })?;
        let body = Self::pin(post)?;
        let mut renewed = false;
        let mut waits = 0;
        let mut wait = PAST_A_MINUTE.first;
        loop {
            let token = self
                .tokens
                .access_token(|| self.renew())
                .map_err(before_sending)?;
            let sent = self
                .agent
                .post(format!("{}/v5/pins", self.api))
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {}", token.expose()))
                .send(&body);
            let response = sent.map_err(|error| {
                if never_sent(&error) {
                    PublishFailure::NotSent(error.to_string())
                } else {
                    PublishFailure::Unconfirmed(error.to_string())
                }
            })?;
            match response.status().as_u16() {
                200 | 201 => {
                    let pin: PinAnswer = read_json(response).map_err(|_| {
                        PublishFailure::Unconfirmed("o Pinterest respondeu sem o id do Pin".into())
                    })?;
                    return pin.published().ok_or_else(|| {
                        PublishFailure::Unconfirmed("o Pinterest respondeu sem o id do Pin".into())
                    });
                }
                401 if !renewed => {
                    self.tokens.forget(&token);
                    renewed = true;
                }
                401 => return Err(PublishFailure::Expired),
                429 if waits < PAST_A_MINUTE.retries => {
                    self.pause.pause(wait);
                    wait *= 2;
                    waits += 1;
                }
                429 => return Err(PublishFailure::RateLimited),
                status @ 500..=599 => {
                    return Err(PublishFailure::Unconfirmed(
                        refusal(response)
                            .unwrap_or_else(|| format!("o Pinterest respondeu {status}")),
                    ));
                }
                status => {
                    return Err(PublishFailure::Refused(
                        refusal(response)
                            .unwrap_or_else(|| format!("o Pinterest respondeu {status}")),
                    ));
                }
            }
        }
    }
}

/// What Pinterest's answer for `path` means, and whether to ask again.
fn attempt(
    response: ureq::http::Response<ureq::Body>,
    path: &str,
) -> Attempt<ureq::http::Response<ureq::Body>> {
    match response.status().as_u16() {
        200..=299 => Attempt::Done(Ok(response)),
        401 => Attempt::Done(Err(PlatformError::Expired)),
        // The login lacks a scope the call needs.
        403 => Attempt::Done(Err(PlatformError::Refused(
            refusal(response).unwrap_or_else(|| "refused".into()),
        ))),
        404 => Attempt::Done(Err(PlatformError::NotFound)),
        429 => Attempt::Again(PlatformError::RateLimited),
        status @ 500..=599 => Attempt::Again(PlatformError::Failed(format!(
            "Pinterest answered {status} for {path}"
        ))),
        status => Attempt::Done(Err(PlatformError::Failed(format!(
            "Pinterest answered {status} for {path}"
        )))),
    }
}

fn read_json<T: DeserializeOwned>(
    response: ureq::http::Response<ureq::Body>,
) -> Result<T, PlatformError> {
    crate::answers::read_json(response, "Pinterest")
}

/// Pinterest's own words for a refusal.
fn refusal(response: ureq::http::Response<ureq::Body>) -> Option<String> {
    read_json::<ErrorAnswer>(response)
        .ok()
        .and_then(ErrorAnswer::why)
}

/// A board's id as a path segment: Pinterest's ids are digits, so no other
/// path is ever reached.
fn board_segment(board: &str) -> Result<&str, PlatformError> {
    let board = board.trim();
    if board.is_empty() || board.len() > 32 || !board.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(PlatformError::NotFound);
    }
    Ok(board)
}
