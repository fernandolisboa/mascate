//! Short.io's adapter (#158, ADR 0035 decision 8): shortens Affiliate Links
//! on the owner's domain for the affiliate module. Each link is one
//! `POST {API}/links` with the secret API key in `Authorization`, the
//! domain, the original link and a path chosen by the caller. Short.io gives
//! back the link it already has for the same path and original link, so a
//! request whose answer got lost is safe to send again. The clicks (#159)
//! come from its statistics API, one `GET` per link with Brasília's days.

use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, TimeDelta};
use mascate_affiliate::{DayClicks, LinkShortener, Shortened};
use mascate_kernel::{PlatformError, channel_day};
use mascate_platform::{Secret, SecretStore, http_agent};
use serde::Deserialize;

use crate::answers::{page_on, read_json};
use crate::escaped::escaped;
use crate::retry::{Attempt, Backoff, Pause, ThreadPause, retrying};

/// Short.io's API, unless a test points elsewhere.
pub const API_URL: &str = "https://api.short.io";

/// Short.io's statistics API, on a host of its own.
pub const STATISTICS_URL: &str = "https://statistics.short.io";

/// The time zone Short.io counts the days in: Brasília's, as the app's.
const TIME_ZONE: &str = "America/Sao_Paulo";

const API_KEY: &str = "SHORT_IO_API_KEY";
const DOMAIN: &str = "SHORT_IO_DOMAIN";

/// Short.io takes 50 new links a second; a 429, a failure on its side or a
/// dropped connection waits 1, 2 and then 4 seconds before giving up.
const BACKOFF: Backoff = Backoff {
    first: Duration::from_secs(1),
    retries: 3,
};

/// Short.io, with the owner's API key and domain.
pub struct ShortIo {
    agent: ureq::Agent,
    api: String,
    statistics: String,
    store: Arc<dyn SecretStore>,
    pause: Arc<dyn Pause>,
}

impl ShortIo {
    /// The API at `api` and its statistics at `statistics`, with the key
    /// and the domain kept in `store`.
    pub fn new(api: &str, statistics: &str, user_agent: &str, store: Arc<dyn SecretStore>) -> Self {
        Self {
            agent: http_agent(api, user_agent, Duration::from_secs(30)),
            api: api.trim_end_matches('/').to_owned(),
            statistics: statistics.trim_end_matches('/').to_owned(),
            store,
            pause: Arc::new(ThreadPause),
        }
    }

    /// The same, waiting between retries with `pause`.
    pub fn with_pause(mut self, pause: Arc<dyn Pause>) -> Self {
        self.pause = pause;
        self
    }

    fn stored(&self, name: &str) -> Result<Secret, PlatformError> {
        self.store
            .read(name)
            .map_err(|error| PlatformError::Failed(error.to_string()))?
            .ok_or(PlatformError::NotConnected)
    }

    /// The owner's domain as Short.io names it, however it was pasted.
    fn domain(&self) -> Result<String, PlatformError> {
        let pasted = self.stored(DOMAIN)?;
        let domain = pasted.expose().trim().to_ascii_lowercase();
        let domain = domain
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        let labels_ok = domain.split('.').count() >= 2
            && domain.split('.').all(|label| {
                !label.is_empty()
                    && label
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            });
        if !labels_ok {
            return Err(PlatformError::Refused(format!(
                "o domínio \"{domain}\" não é um endereço como mascate.short.gy"
            )));
        }
        Ok(domain.to_owned())
    }

    fn send(&self, key: &Secret, domain: &str, body: &str) -> Attempt<Shortened> {
        let sent = self
            .agent
            .post(format!("{}/links", self.api))
            .header("Authorization", key.expose())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .send(body);
        let response = match sent {
            Ok(response) => response,
            Err(error) => return Attempt::Again(PlatformError::Failed(error.to_string())),
        };
        match response.status().as_u16() {
            200 | 201 => Attempt::Done(read_json::<LinkAnswer>(response, "Short.io").and_then(
                |answer| {
                    let url = page_on(&answer.secure_short_url, domain).ok_or_else(|| {
                        PlatformError::Failed(format!(
                            "Short.io answered a link off {domain}: {}",
                            answer.secure_short_url
                        ))
                    })?;
                    Ok(Shortened {
                        url,
                        key: answer.id_string,
                    })
                },
            )),
            _ => failed(response),
        }
    }

    fn ask_clicks(&self, key: &Secret, url: &str) -> Attempt<Vec<DayClicks>> {
        let sent = self
            .agent
            .get(url)
            .header("Authorization", key.expose())
            .header("Accept", "application/json")
            .call();
        let response = match sent {
            Ok(response) => response,
            Err(error) => return Attempt::Again(PlatformError::Failed(error.to_string())),
        };
        match response.status().as_u16() {
            200 => Attempt::Done(
                read_json::<StatisticsAnswer>(response, "Short.io").and_then(days_counted),
            ),
            404 => Attempt::Done(Err(PlatformError::NotFound)),
            _ => failed(response),
        }
    }
}

/// What an answer other than the one asked means: 401 is a key Short.io no
/// longer accepts, 429 and its own failures are tried again, and any other
/// refusal comes in its words.
fn failed<T>(response: ureq::http::Response<ureq::Body>) -> Attempt<T> {
    match response.status().as_u16() {
        401 => Attempt::Done(Err(PlatformError::Expired)),
        429 => Attempt::Again(PlatformError::RateLimited),
        status @ 500..=599 => {
            Attempt::Again(PlatformError::Failed(format!("Short.io answered {status}")))
        }
        status @ 400..=499 => Attempt::Done(Err(PlatformError::Refused(
            read_json::<ErrorAnswer>(response, "Short.io")
                .ok()
                .and_then(|answer| answer.message.or(answer.error))
                .unwrap_or_else(|| format!("Short.io answered {status}")),
        ))),
        status => Attempt::Done(Err(PlatformError::Failed(format!(
            "Short.io answered {status}"
        )))),
    }
}

/// Each day of the chart, by the day it starts in Brasília: Short.io dates a
/// point with the day itself or with the instant the day starts.
fn days_counted(answer: StatisticsAnswer) -> Result<Vec<DayClicks>, PlatformError> {
    let points = answer
        .click_statistics
        .datasets
        .into_iter()
        .next()
        .map(|dataset| dataset.data)
        .unwrap_or_default();
    points
        .into_iter()
        .map(|point| {
            let day = point
                .x
                .parse::<NaiveDate>()
                .ok()
                .or_else(|| {
                    DateTime::parse_from_rfc3339(&point.x)
                        .ok()
                        .map(|at| channel_day(at.to_utc()))
                })
                .ok_or_else(|| {
                    PlatformError::Failed(format!("Short.io dated clicks as {}", point.x))
                })?;
            let clicks = point
                .y
                .as_u64()
                .or_else(|| point.y.as_f64().map(|y| y.max(0.0).round() as u64))
                .or_else(|| point.y.as_str().and_then(|y| y.trim().parse().ok()))
                .ok_or_else(|| {
                    PlatformError::Failed(format!("Short.io counted clicks as {}", point.y))
                })?;
            Ok(DayClicks { day, clicks })
        })
        .collect()
}

impl LinkShortener for ShortIo {
    fn shorten(&self, link: &str, path: &str) -> Result<Shortened, PlatformError> {
        let key = self.stored(API_KEY)?;
        let domain = self.domain()?;
        let body = serde_json::json!({
            "domain": domain,
            "originalURL": link,
            "path": path,
            "allowDuplicates": false,
        })
        .to_string();
        retrying(self.pause.as_ref(), BACKOFF, || {
            self.send(&key, &domain, &body)
        })
    }

    fn clicks(
        &self,
        key: &str,
        days: RangeInclusive<NaiveDate>,
    ) -> Result<Vec<DayClicks>, PlatformError> {
        let secret = self.stored(API_KEY)?;
        // The end date is read as "clicks before it", so it is the day
        // after the last one asked; a day outside `days` is dropped later.
        let url = format!(
            "{}/statistics/link/{}?period=custom&startDate={}&endDate={}&tz={}\
             &clicksChartInterval=day&skipTops=true",
            self.statistics,
            escaped(key),
            days.start(),
            *days.end() + TimeDelta::days(1),
            escaped(TIME_ZONE),
        );
        retrying(self.pause.as_ref(), BACKOFF, || {
            self.ask_clicks(&secret, &url)
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkAnswer {
    id_string: String,
    #[serde(rename = "secureShortURL")]
    secure_short_url: String,
}

#[derive(Deserialize)]
struct ErrorAnswer {
    message: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatisticsAnswer {
    click_statistics: ClickStatistics,
}

#[derive(Deserialize)]
struct ClickStatistics {
    #[serde(default)]
    datasets: Vec<Dataset>,
}

#[derive(Deserialize)]
struct Dataset {
    #[serde(default)]
    data: Vec<Point>,
}

#[derive(Deserialize)]
struct Point {
    x: String,
    y: serde_json::Value,
}
