//! The one engine every discovery crawler shares (ADR 0035, decision 2):
//! it reads public pages politely and stops when a site says no. It obeys
//! each host's robots.txt, waits between requests to the same host, answers
//! a page read in the last hour from memory, and stops asking a host for an
//! hour after three blocks in a row. Every request names the Mascate: no
//! proxy, no other identity, no browser imitation, no captcha solving. A
//! block is the site's answer, not an obstacle.

mod offers;
mod page_data;
mod robots;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use chrono::TimeDelta;
use mascate_kernel::{Clock, Pause, PlatformError, ThreadPause, Timestamp};
use mascate_platform::crawling_agent;
use ureq::http::Uri;

pub(crate) use offers::{
    count_before, is_product, money, no_shops, number_in, search_words, stars, walk,
};
pub(crate) use page_data::{assigned_json, json_ld, script_json};
use robots::Robots;

/// The least time between two requests to one host.
const INTERVAL: Duration = Duration::from_secs(3);
/// How long a page read answers again from memory, robots.txt included.
const KEPT_FOR: TimeDelta = TimeDelta::hours(1);
/// Blocks in a row that open a host's breaker, and for how long.
const BLOCKS_TO_OPEN: u32 = 3;
const OPEN_FOR: TimeDelta = TimeDelta::hours(1);
/// The longest a request waits for its turn at a host. A robots.txt that
/// asks for more between requests, or a queue of searches past it, is
/// refused rather than left blocking a thread.
const LONGEST_WAIT: Duration = Duration::from_secs(120);
/// Redirects followed for one page.
const MOST_REDIRECTS: usize = 3;
/// A search page is a few megabytes at most; anything bigger is not one.
const MAX_PAGE_BYTES: u64 = 8 * 1024 * 1024;
/// Words in the path a redirect leads to that mean the site wants a login
/// or a check of who is asking.
const BLOCKING_PATHS: [&str; 6] = [
    "login",
    "signin",
    "captcha",
    "challenge",
    "verification",
    "verify",
];
/// Marks of a captcha or a check of who is asking on a page, in lower
/// case: answered 200, or 503 as Amazon's robot check is.
const BLOCKING_PAGES: [&str; 8] = [
    "g-recaptcha",
    "h-captcha",
    "hcaptcha.com",
    "captcha-delivery.com",
    "/cdn-cgi/challenge-platform/",
    "cf-chl-",
    "/errors/validatecaptcha",
    "api-services-support@amazon.com",
];

/// Reads public pages for the crawlers, one instance for the whole app so
/// every crawler shares its turns, memory and breakers per host. Calls
/// block on the network and on the waits, so they run off the UI thread.
pub struct Crawler {
    agent: ureq::Agent,
    /// The product token robots.txt groups are matched against.
    token: String,
    clock: Arc<dyn Clock>,
    pause: Arc<dyn Pause>,
    hosts: Mutex<HashMap<String, Host>>,
    pages: Mutex<HashMap<String, Kept>>,
}

/// What the crawler knows of one host.
#[derive(Default)]
struct Host {
    /// Its robots.txt and when it was read.
    robots: Option<(Arc<Robots>, Timestamp)>,
    /// When the next request to it may leave.
    next: Option<Timestamp>,
    /// Blocks in a row since the last answer that was not one.
    blocks: u32,
    open_until: Option<Timestamp>,
}

struct Kept {
    body: String,
    read_at: Timestamp,
}

/// How a host answered one request.
enum Reply {
    Page(String),
    Moved(String),
    /// 403, 429, a captcha or a redirect to a login, with why.
    Blocked(String),
    /// 404, 410 or another refusal that is not a block.
    Missing,
}

/// A page's address, split as the crawler needs it.
struct Address {
    scheme: String,
    /// `host` or `host:port`, in lower case: the key of everything kept per
    /// host.
    host: String,
    /// The path with its query, as robots.txt rules match it.
    path: String,
}

impl Address {
    fn parse(url: &str) -> Result<Self, PlatformError> {
        let unreadable = || PlatformError::Failed(format!("endereço que não dá para ler: {url}"));
        let uri: Uri = url.parse().map_err(|_| unreadable())?;
        let scheme = uri
            .scheme_str()
            .ok_or_else(unreadable)?
            .to_ascii_lowercase();
        if scheme != "https" && scheme != "http" {
            return Err(unreadable());
        }
        let host = uri.authority().ok_or_else(unreadable)?.as_str();
        if host.contains('@') {
            return Err(unreadable());
        }
        let path = uri
            .path_and_query()
            .map(|path| path.as_str())
            .filter(|path| path.starts_with('/'))
            .unwrap_or("/");
        Ok(Self {
            scheme,
            host: host.to_ascii_lowercase(),
            path: path.to_owned(),
        })
    }

    fn url(&self) -> String {
        format!("{}://{}{}", self.scheme, self.host, self.path)
    }

    fn robots(&self) -> String {
        format!("{}://{}/robots.txt", self.scheme, self.host)
    }

    /// Where a redirect from here to `location` leads; never from HTTPS
    /// down to HTTP.
    fn follow(&self, location: &str) -> Result<Address, PlatformError> {
        let location = location.trim();
        let url = if location.contains("://") {
            location.to_owned()
        } else if let Some(rest) = location.strip_prefix("//") {
            format!("{}://{rest}", self.scheme)
        } else if location.starts_with('/') {
            format!("{}://{}{location}", self.scheme, self.host)
        } else {
            let folder = self.path.split('?').next().unwrap_or("/");
            let folder = &folder[..=folder.rfind('/').unwrap_or(0)];
            format!("{}://{}{folder}{location}", self.scheme, self.host)
        };
        let next = Address::parse(&url)?;
        if self.scheme == "https" && next.scheme != "https" {
            return Err(PlatformError::Failed(format!(
                "{} mandou para um endereço sem https",
                self.host
            )));
        }
        Ok(next)
    }
}

impl Crawler {
    /// Requests go out as `user_agent`, such as `Mascate/0.5.0`; its
    /// product token is what robots.txt groups are matched against.
    pub fn new(user_agent: &str, clock: Arc<dyn Clock>) -> Self {
        Self {
            agent: crawling_agent(user_agent),
            token: user_agent
                .split('/')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase(),
            clock,
            pause: Arc::new(ThreadPause),
            hosts: Mutex::new(HashMap::new()),
            pages: Mutex::new(HashMap::new()),
        }
    }

    /// The same, waiting for each turn with `pause`.
    pub fn with_pause(mut self, pause: Arc<dyn Pause>) -> Self {
        self.pause = pause;
        self
    }

    /// Until when the host of `url` is left alone after blocking the
    /// crawler; `None` while it may be asked.
    pub fn blocked_until(&self, url: &str) -> Option<Timestamp> {
        let address = Address::parse(url).ok()?;
        let now = self.clock.now();
        self.hosts()
            .get(&address.host)
            .and_then(|host| host.open_until)
            .filter(|until| *until > now)
    }

    /// The page at `url`, from memory when it was read in the last hour,
    /// following redirects the same way.
    pub(crate) fn page(&self, url: &str) -> Result<String, PlatformError> {
        let mut address = Address::parse(url)?;
        for _ in 0..=MOST_REDIRECTS {
            let url = address.url();
            if let Some(body) = self.kept(&url) {
                return Ok(body);
            }
            self.ensure_closed(&address.host)?;
            let robots = self.robots(&address)?;
            if !robots.allows(&address.path) {
                return Err(PlatformError::Refused(format!(
                    "o robots.txt de {} não deixa ler {}",
                    address.host, address.path
                )));
            }
            if let Some(delay) = robots.delay().filter(|delay| *delay > LONGEST_WAIT) {
                return Err(PlatformError::Refused(format!(
                    "o robots.txt de {} pede {} s entre leituras, mais do que o app espera",
                    address.host,
                    delay.as_secs()
                )));
            }
            let interval = robots.delay().map_or(INTERVAL, |delay| delay.max(INTERVAL));
            match self.get(&address, interval)? {
                Reply::Page(body) => {
                    self.keep(url, &body);
                    return Ok(body);
                }
                Reply::Moved(location) => address = address.follow(&location)?,
                Reply::Blocked(why) => return Err(self.blocked(&address.host, why)),
                Reply::Missing => return Err(PlatformError::NotFound),
            }
        }
        Err(PlatformError::Failed(format!(
            "{} redirecionou mais de {MOST_REDIRECTS} vezes",
            address.host
        )))
    }

    /// The robots.txt of `address`'s host, read again after an hour. One
    /// the site does not have allows everything; one it blocks is a block.
    fn robots(&self, address: &Address) -> Result<Arc<Robots>, PlatformError> {
        let now = self.clock.now();
        if let Some((robots, _)) = self
            .hosts()
            .get(&address.host)
            .and_then(|host| host.robots.as_ref())
            .filter(|(_, read_at)| now - *read_at < KEPT_FOR)
        {
            return Ok(robots.clone());
        }
        let mut at = Address::parse(&address.robots())?;
        let mut robots = None;
        for _ in 0..=MOST_REDIRECTS {
            match self.get(&at, INTERVAL)? {
                Reply::Page(text) => {
                    robots = Some(Robots::parse(&text, &self.token));
                    break;
                }
                Reply::Missing => {
                    robots = Some(Robots::default());
                    break;
                }
                Reply::Moved(location) => at = at.follow(&location)?,
                Reply::Blocked(why) => return Err(self.blocked(&at.host, why)),
            }
        }
        let robots = Arc::new(robots.ok_or_else(|| {
            PlatformError::Failed(format!(
                "o robots.txt de {} redirecionou demais",
                address.host
            ))
        })?);
        let mut hosts = self.hosts();
        let host = hosts.entry(address.host.clone()).or_default();
        host.robots = Some((robots.clone(), self.clock.now()));
        if let (Some(delay), Some(next)) = (robots.delay(), host.next) {
            // The robots.txt request itself already waits for the delay it
            // asks for.
            let stretched = next - delta(INTERVAL) + delta(delay.max(INTERVAL));
            host.next = Some(next.max(stretched));
        }
        Ok(robots)
    }

    /// Sends one GET to `address` once its host's turn comes, then counts
    /// the answer for the breaker.
    fn get(&self, address: &Address, interval: Duration) -> Result<Reply, PlatformError> {
        self.take_turn(&address.host, interval)?;
        let reply = self.send(address)?;
        let mut hosts = self.hosts();
        let host = hosts.entry(address.host.clone()).or_default();
        match reply {
            Reply::Blocked(_) => {
                host.blocks += 1;
                if host.blocks >= BLOCKS_TO_OPEN {
                    host.blocks = 0;
                    host.open_until = Some(self.clock.now() + OPEN_FOR);
                }
            }
            _ => host.blocks = 0,
        }
        Ok(reply)
    }

    fn send(&self, address: &Address) -> Result<Reply, PlatformError> {
        let response = self
            .agent
            .get(address.url())
            .header("Accept", "text/html,application/xhtml+xml,text/plain")
            .header("Accept-Language", "pt-BR,pt;q=0.9")
            .call()
            .map_err(|error| PlatformError::Failed(error.to_string()))?;
        let status = response.status().as_u16();
        match status {
            200..=299 => {
                let body = response
                    .into_body()
                    .into_with_config()
                    .limit(MAX_PAGE_BYTES)
                    .read_to_string()
                    .map_err(|error| PlatformError::Failed(error.to_string()))?;
                Ok(if robot_check(&body) {
                    Reply::Blocked(ROBOT_CHECK.into())
                } else {
                    Reply::Page(body)
                })
            }
            301 | 302 | 303 | 307 | 308 => {
                let location = response
                    .headers()
                    .get("Location")
                    .and_then(|location| location.to_str().ok())
                    .ok_or_else(|| {
                        PlatformError::Failed(format!(
                            "{} redirecionou sem dizer para onde",
                            address.host
                        ))
                    })?;
                let path = location
                    .split(['?', '#'])
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                Ok(if BLOCKING_PATHS.iter().any(|word| path.contains(word)) {
                    Reply::Blocked("mandou para o login ou para uma verificação".into())
                } else {
                    Reply::Moved(location.to_owned())
                })
            }
            401 | 403 => Ok(Reply::Blocked(format!("recusou o acesso ({status})"))),
            429 => Ok(Reply::Blocked("pediu para esperar (429)".into())),
            400..=499 => Ok(Reply::Missing),
            status => {
                let body = response
                    .into_body()
                    .into_with_config()
                    .limit(MAX_PAGE_BYTES)
                    .read_to_string()
                    .unwrap_or_default();
                if robot_check(&body) {
                    Ok(Reply::Blocked(ROBOT_CHECK.into()))
                } else {
                    Err(PlatformError::Failed(format!(
                        "{} respondeu {status}",
                        address.host
                    )))
                }
            }
        }
    }

    /// Waits until the next request to `host` may leave, and keeps the one
    /// after it at least `interval` later.
    fn take_turn(&self, host: &str, interval: Duration) -> Result<(), PlatformError> {
        let wait = {
            let mut hosts = self.hosts();
            let state = hosts.entry(host.to_owned()).or_default();
            let now = self.clock.now();
            let at = state.next.filter(|next| *next > now).unwrap_or(now);
            let wait = (at - now).to_std().unwrap_or_default();
            if wait > LONGEST_WAIT {
                return Err(PlatformError::RateLimited);
            }
            state.next = Some(at + delta(interval));
            wait
        };
        if !wait.is_zero() {
            self.pause.pause(wait);
        }
        Ok(())
    }

    fn ensure_closed(&self, host: &str) -> Result<(), PlatformError> {
        let now = self.clock.now();
        match self.hosts().get(host).and_then(|state| state.open_until) {
            Some(until) if until > now => Err(PlatformError::Refused(format!(
                "{host} bloqueou o crawler, que espera antes de pedir de novo"
            ))),
            _ => Ok(()),
        }
    }

    fn blocked(&self, host: &str, why: String) -> PlatformError {
        PlatformError::Refused(format!("{host} {why}"))
    }

    fn kept(&self, url: &str) -> Option<String> {
        let now = self.clock.now();
        self.pages()
            .get(url)
            .filter(|kept| now - kept.read_at < KEPT_FOR)
            .map(|kept| kept.body.clone())
    }

    fn keep(&self, url: String, body: &str) {
        let now = self.clock.now();
        let mut pages = self.pages();
        pages.retain(|_, kept| now - kept.read_at < KEPT_FOR);
        pages.insert(
            url,
            Kept {
                body: body.to_owned(),
                read_at: now,
            },
        );
    }

    fn hosts(&self) -> MutexGuard<'_, HashMap<String, Host>> {
        self.hosts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn pages(&self) -> MutexGuard<'_, HashMap<String, Kept>> {
        self.pages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

const ROBOT_CHECK: &str = "respondeu com uma verificação de robô";

/// Whether `body` is a captcha or a check of who is asking.
fn robot_check(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    BLOCKING_PAGES.iter().any(|mark| lower.contains(mark))
}

fn delta(duration: Duration) -> TimeDelta {
    TimeDelta::from_std(duration).unwrap_or(TimeDelta::MAX)
}
