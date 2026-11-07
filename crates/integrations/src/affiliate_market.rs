//! What every logged-in affiliate market reader shares (ADR 0039, decision
//! 3): the market only opens with the owner's login, so the app opens it in
//! the installed browser, on the Mascate's own profile where the owner
//! logged in by hand, and reads what the page already drew.
//!
//! It reads only when the owner asks, at most a few pages each time and
//! never two within seconds of each other, keeps each page for an hour and
//! stops for a day at the first login, robot check or block. It never
//! clicks, never asks for an affiliation and makes no request of its own to
//! the Platform's internal API. Each Platform's reader says where its
//! listing is, which links are product cards and which labels it reads.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, TimeDelta};
use mascate_affiliate::{
    AffiliatePlatform, Affiliation, FoundProspect, MAX_TEMPERATURE, ProspectSource,
};
use mascate_kernel::{
    Clock, Currency, Money, Pause, Percentage, PlatformError, Timestamp, fold, parse_amount,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;

use crate::affiliate_panel::{Panel, PanelState, lands_on_login};
use crate::browser::BrowserSession;

/// Pages one click reads at most, cached ones aside.
pub const PAGES_PER_CLICK: usize = 5;
/// The least time between two pages read.
pub const INTERVAL: Duration = Duration::from_secs(5);
/// How long a page read answers again from memory.
const KEPT_FOR: TimeDelta = TimeDelta::hours(1);
/// How long the reader leaves the Platform alone after a login, a robot
/// check or a block.
pub const STOPPED_FOR: TimeDelta = TimeDelta::hours(24);

/// A logged-in affiliate market as Garimpo uses it: a `ProspectSource`
/// with the Connection of the browser it reads through and its stop.
pub trait AffiliateMarket: ProspectSource {
    /// Where the market's reading stands, as its Connection shows it.
    fn state(&self) -> PanelState;

    /// Until when, and why, the reader leaves the Platform alone; `None`
    /// while it may read.
    fn stopped(&self) -> Option<Stopped>;

    /// Opens the market in the browser's window for the owner to log in, or
    /// to answer a robot check, by hand. A stop for a login or a check ends:
    /// the owner answered it. A block stays for its day.
    fn open_login(&self) -> Result<(), PlatformError>;
}

/// The script that reads a market page: the address it ended on, whether a
/// robot check shows, each product card (by its link to the product, found
/// by `product` as a CSS selector, the card being the largest block around
/// that link with no other product, products told apart by the JavaScript
/// expression `code` of `href`) as its lines of text, and the whole page's
/// lines for a product's own page. It reads the page and nothing else.
///
/// To record a page for a fixture, paste the script into the console of
/// the browser's developer tools on the market's page and copy what it
/// prints with `copy(...)` around it.
pub(crate) fn market_script(product: &str, code: &str) -> String {
    MARKET_SCRIPT
        .replace("__PRODUCT__", product)
        .replace("__CODE__", code)
}

const MARKET_SCRIPT: &str = r#"(() => {
  const clean = (text) => (text || "").replace(/\s+/g, " ").trim();
  const lines = (element) => (element.innerText || element.textContent || "")
    .split("\n").map(clean).filter((line) => line !== "");
  const product = '__PRODUCT__';
  const code = (href) => { try { return __CODE__; } catch (_) { return href; } };
  const products = (element) => new Set(Array.from(element.querySelectorAll(product)).map((a) => code(a.href)));
  const hrefs = (element) => Array.from(element.querySelectorAll("a[href]")).map((a) => a.href);
  const cards = [];
  const seen = new Set();
  for (const link of document.querySelectorAll(product)) {
    if (seen.has(code(link.href))) continue;
    seen.add(code(link.href));
    let card = link;
    while (card.parentElement && card.parentElement !== document.body && products(card.parentElement).size <= 1) {
      card = card.parentElement;
    }
    const heading = card.querySelector('h1, h2, h3, h4, h5, h6, [role="heading"]');
    cards.push({
      link: link.href,
      title: clean(heading ? heading.innerText || heading.textContent : link.innerText || link.textContent),
      links: hrefs(card),
      lines: lines(card),
    });
  }
  const heading = document.querySelector("h1");
  const captcha = !!document.querySelector('iframe[src*="captcha"], iframe[src*="challenge"], .g-recaptcha, .h-captcha, #challenge-form, [id*="captcha"]');
  return {
    url: location.href,
    title: clean(document.title),
    captcha,
    cards,
    heading: heading ? clean(heading.innerText || heading.textContent) : "",
    links: hrefs(document.body || document.documentElement).slice(0, 400),
    lines: lines(document.body || document.documentElement).slice(0, 800),
  };
})()"#;

/// A market page as the script read it.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct MarketPage {
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) captcha: bool,
    #[serde(default)]
    pub(crate) cards: Vec<Card>,
    #[serde(default)]
    pub(crate) heading: String,
    #[serde(default)]
    pub(crate) links: Vec<String>,
    #[serde(default)]
    pub(crate) lines: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Card {
    pub(crate) link: String,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) links: Vec<String>,
    #[serde(default)]
    pub(crate) lines: Vec<String>,
}

impl Card {
    /// The card's product name: its heading, else its first line.
    pub(crate) fn name(&self) -> Option<&str> {
        Some(self.title.trim())
            .filter(|title| !title.is_empty())
            .or_else(|| self.lines.first().map(|line| line.trim()))
            .filter(|name| !name.is_empty())
    }
}

/// Words near the top of a page that mean the Platform refused it, as
/// letters and digits only.
const BLOCKED_PAGES: [&str; 6] = [
    "TOOMANYREQUESTS",
    "ACESSONEGADO",
    "ACCESSDENIED",
    "MUITASSOLICITACOES",
    "MUITASREQUISICOES",
    "403FORBIDDEN",
];
/// How many lines from the top are looked at for those words.
const BLOCK_LINES: usize = 20;

/// Why the reader stopped asking the Platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketStop {
    /// The market sent the browser to the login.
    Login,
    /// The page showed a check of who is asking (a captcha).
    Check,
    /// The Platform refused the page.
    Block,
}

impl MarketStop {
    fn code(self) -> &'static str {
        match self {
            MarketStop::Login => "login",
            MarketStop::Check => "check",
            MarketStop::Block => "block",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        [MarketStop::Login, MarketStop::Check, MarketStop::Block]
            .into_iter()
            .find(|stop| stop.code() == code)
    }

    /// Whether the owner answers it in the login window: a login or a
    /// check, not a block.
    fn answered_by_logging_in(self) -> bool {
        self != MarketStop::Block
    }
}

/// Until when, and why, the reader leaves the Platform alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped {
    pub why: MarketStop,
    pub until: Timestamp,
}

#[derive(Default)]
struct Pacing {
    last_read: Option<Timestamp>,
    kept: HashMap<String, (MarketPage, Timestamp)>,
    stopped: Option<Stopped>,
}

/// Pages one click of the owner's has read, cached ones aside.
#[derive(Default)]
struct Click {
    pages: usize,
}

/// A Platform's market read through the shared browser, at the pace and
/// with the stop every market reader keeps.
pub(crate) struct MarketReader {
    platform: AffiliatePlatform,
    market: String,
    panel: Panel,
    clock: Arc<dyn Clock>,
    pause: Arc<dyn Pause>,
    script: String,
    /// Whether a page that is neither a login, a robot check nor a listing
    /// is a product's own page, drawn and ready to read.
    product_drawn: fn(&MarketPage) -> bool,
    /// Where a stop is written, so it outlasts a restart of the app.
    stop_file: Option<PathBuf>,
    /// Held through a whole page read: one page at a time.
    turn: Mutex<()>,
    pacing: Mutex<Pacing>,
}

impl MarketReader {
    /// `platform`'s market at `market`, opened by `browser` and read by
    /// `script`, timed by `clock` and waiting with `pause`.
    pub(crate) fn new(
        platform: AffiliatePlatform,
        market: &str,
        browser: Arc<BrowserSession>,
        clock: Arc<dyn Clock>,
        pause: Arc<dyn Pause>,
        script: String,
        product_drawn: fn(&MarketPage) -> bool,
    ) -> Self {
        Self {
            platform,
            market: market.trim_end_matches('/').to_owned(),
            panel: Panel::new(browser),
            clock,
            pause,
            script,
            product_drawn,
            stop_file: None,
            turn: Mutex::new(()),
            pacing: Mutex::new(Pacing::default()),
        }
    }

    /// The market's address, without a trailing slash.
    pub(crate) fn market(&self) -> &str {
        &self.market
    }

    /// Keeps a stop in `file` and starts with the one written there.
    pub(crate) fn with_stop_file(mut self, file: PathBuf) -> Self {
        self.pacing().stopped = std::fs::read_to_string(&file)
            .ok()
            .as_deref()
            .and_then(read_stop);
        self.stop_file = Some(file);
        self
    }

    pub(crate) fn state(&self) -> PanelState {
        self.panel.state()
    }

    pub(crate) fn stopped(&self) -> Option<Stopped> {
        let now = self.clock.now();
        self.pacing().stopped.filter(|stopped| stopped.until > now)
    }

    pub(crate) fn open_login(&self) -> Result<(), PlatformError> {
        self.panel.open_login(&self.market)?;
        let mut pacing = self.pacing();
        if pacing
            .stopped
            .is_some_and(|stopped| stopped.why.answered_by_logging_in())
        {
            pacing.stopped = None;
            drop(pacing);
            self.write_stop(None);
        }
        Ok(())
    }

    /// The listing's pages in order, `url` giving each page's address from
    /// 1, up to `pages` and the click's pages, until one shows nothing new;
    /// each card is a Prospect by `listed`. A stop after the first page
    /// keeps what was read; the stop itself shows on the screen.
    pub(crate) fn listing(
        &self,
        pages: usize,
        url: impl Fn(usize) -> String,
        listed: impl Fn(&Card) -> Option<FoundProspect>,
    ) -> Result<Vec<FoundProspect>, PlatformError> {
        let mut click = Click::default();
        let mut found: Vec<FoundProspect> = Vec::new();
        let mut seen = HashSet::new();
        for number in 1..=pages.min(PAGES_PER_CLICK) {
            let page = match self.page(&url(number), &mut click) {
                Ok(Some(page)) => page,
                Ok(None) => break,
                Err(_) if !found.is_empty() => break,
                Err(error) => return Err(error),
            };
            let before = found.len();
            found.extend(
                page.cards
                    .iter()
                    .filter_map(&listed)
                    .filter(|prospect| seen.insert(prospect.key.clone())),
            );
            if found.len() == before {
                break;
            }
        }
        Ok(found)
    }

    /// A product's own page at `url`, read on its own click.
    pub(crate) fn product(&self, url: &str) -> Result<MarketPage, PlatformError> {
        self.page(url, &mut Click::default())?
            .ok_or_else(|| PlatformError::Failed("a página do produto não foi lida".into()))
    }

    /// The page at `url`: from memory when read in the last hour, else read
    /// once the interval since the last page has passed, unless `click`
    /// already read its pages (`None`). A login, a robot check or a block
    /// stops the reader for a day.
    fn page(&self, url: &str, click: &mut Click) -> Result<Option<MarketPage>, PlatformError> {
        let _turn = self.turn.lock().unwrap_or_else(PoisonError::into_inner);
        let now = self.clock.now();
        let wait = {
            let pacing = self.pacing();
            if let Some(stopped) = pacing.stopped.filter(|stopped| stopped.until > now) {
                return Err(self.stopped_error(stopped.why));
            }
            if let Some((page, _)) = pacing
                .kept
                .get(url)
                .filter(|(_, read_at)| now - *read_at < KEPT_FOR)
            {
                return Ok(Some(page.clone()));
            }
            if click.pages >= PAGES_PER_CLICK {
                return Ok(None);
            }
            pacing
                .last_read
                .map(|last| last + TimeDelta::from_std(INTERVAL).unwrap_or_default() - now)
                .filter(|wait| *wait > TimeDelta::zero())
        };
        if let Some(wait) = wait.and_then(|wait| wait.to_std().ok()) {
            self.pause.pause(wait);
        }
        click.pages += 1;
        let product_drawn = self.product_drawn;
        let drawn = move |value: &Value| {
            MarketPage::deserialize(value).is_ok_and(|page| {
                lands_on_login(&page.url)
                    || page.captcha
                    || !page.cards.is_empty()
                    || product_drawn(&page)
            })
        };
        let read = self.panel.evaluate(url, &self.script, &drawn);
        let read_at = self.clock.now();
        self.pacing().last_read = Some(read_at);
        let page = MarketPage::deserialize(&read?).map_err(|error| {
            PlatformError::Failed(format!("a leitura do mercado voltou sem a página: {error}"))
        })?;
        let logged_out = lands_on_login(&page.url);
        self.panel.set_logged_out(logged_out);
        let stop = if logged_out {
            Some(MarketStop::Login)
        } else if page.captcha {
            Some(MarketStop::Check)
        } else if refused(&page) {
            Some(MarketStop::Block)
        } else {
            None
        };
        if let Some(why) = stop {
            let stopped = Stopped {
                why,
                until: read_at + STOPPED_FOR,
            };
            self.pacing().stopped = Some(stopped);
            self.write_stop(Some(stopped));
            return Err(self.stopped_error(why));
        }
        self.pacing()
            .kept
            .insert(url.to_owned(), (page.clone(), read_at));
        Ok(Some(page))
    }

    fn stopped_error(&self, why: MarketStop) -> PlatformError {
        match why {
            MarketStop::Login => PlatformError::Expired,
            MarketStop::Check => PlatformError::Refused(format!(
                "a {} pediu a verificação contra robôs, que só você responde",
                self.platform.name()
            )),
            MarketStop::Block => PlatformError::RateLimited,
        }
    }

    fn write_stop(&self, stopped: Option<Stopped>) {
        let Some(file) = &self.stop_file else {
            return;
        };
        // A stop that could not be written still holds while the app runs.
        let _ = match stopped {
            Some(stopped) => std::fs::write(
                file,
                format!("{} {}", stopped.why.code(), stopped.until.to_rfc3339()),
            ),
            None => std::fs::remove_file(file),
        };
    }

    fn pacing(&self) -> MutexGuard<'_, Pacing> {
        self.pacing.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn refused(page: &MarketPage) -> bool {
    std::iter::once(&page.title)
        .chain(page.lines.iter().take(BLOCK_LINES))
        .map(|line| folded(line))
        .any(|line| BLOCKED_PAGES.iter().any(|word| line.contains(word)))
}

fn read_stop(text: &str) -> Option<Stopped> {
    let (why, until) = text.trim().split_once(' ')?;
    Some(Stopped {
        why: MarketStop::from_code(why)?,
        until: DateTime::parse_from_rfc3339(until).ok()?.to_utc(),
    })
}

/// The value of `name` in the query of `link`.
pub(crate) fn query_value(link: &str, name: &str) -> Option<String> {
    let query = link.split_once('?')?.1.split('#').next()?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| value.to_owned())
    })
}

/// A Platform's product code: letters, digits and dashes, as a UUID.
pub(crate) fn is_code(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Every text that follows one of `labels` in `lines`: the line itself
/// when the value is on it ("Temperatura 87°", "Blueprint: 85%"), else the
/// next line.
pub(crate) fn labelled_all<'a>(lines: &'a [String], labels: &[&str]) -> Vec<&'a str> {
    let labels: Vec<String> = labels.iter().map(|label| folded(label)).collect();
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_folded = folded(line);
            let label = labels
                .iter()
                .find(|label| line_folded.starts_with(*label))?;
            if line_folded == *label {
                lines.get(index + 1).map(String::as_str)
            } else {
                Some(line.as_str())
            }
        })
        .collect()
}

pub(crate) fn labelled<'a>(lines: &'a [String], labels: &[&str]) -> Option<&'a str> {
    labelled_all(lines, labels).into_iter().next()
}

/// A name after one of `labels`: after a colon on the label's line, or on
/// the next line.
pub(crate) fn named(lines: &[String], labels: &[&str]) -> Option<String> {
    let labels: Vec<String> = labels.iter().map(|label| folded(label)).collect();
    lines.iter().enumerate().find_map(|(index, line)| {
        let (head, rest) = match line.split_once(':') {
            Some((head, rest)) => (head, Some(rest)),
            None => (line.as_str(), None),
        };
        if !labels.contains(&folded(head)) {
            return None;
        }
        rest.or_else(|| lines.get(index + 1).map(String::as_str))
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
    })
}

/// How the producer takes a new affiliate, by the words of an affiliation
/// the producer approves (`on_request`), then of one at once
/// (`immediate`), each as letters and digits only.
pub(crate) fn affiliation(
    lines: &[String],
    on_request: &[&str],
    immediate: &[&str],
) -> Option<Affiliation> {
    let folded_lines: Vec<String> = lines.iter().map(|line| folded(line)).collect();
    let has = |words: &[&str]| {
        folded_lines
            .iter()
            .any(|line| words.iter().any(|word| line.contains(word)))
    };
    if has(on_request) {
        Some(Affiliation::OnRequest)
    } else if has(immediate) {
        Some(Affiliation::Immediate)
    } else {
        None
    }
}

/// The first number in `text`, as written: `87`, `4,6`, `1.234,56`.
pub(crate) fn first_number(text: &str) -> Option<String> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let number: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect();
    Some(number.trim_end_matches([',', '.']).to_owned())
}

/// A whole count: `7`, `7 dias`.
pub(crate) fn count(text: &str) -> Option<u32> {
    first_number(text)?.parse().ok()
}

/// Degrees from 0 to Hotmart's 150°.
pub(crate) fn degrees(text: &str) -> Option<u32> {
    count(text).filter(|degrees| *degrees <= MAX_TEMPERATURE)
}

/// The number just before a `%`: `50%`, `12,5 %`.
pub(crate) fn percent(text: &str) -> Option<Percentage> {
    let before = text[..text.find('%')?].trim_end();
    let start = before
        .rfind(|c: char| !(c.is_ascii_digit() || c == ',' || c == '.'))
        .map_or(0, |index| index + 1);
    Percentage::new(parse_amount(&before[start..])?).ok()
}

/// An amount after `R$` or `US$`, in its currency.
pub(crate) fn money(text: &str) -> Option<Money> {
    let (currency, at) = text
        .find("US$")
        .map(|at| (Currency::Usd, at + 3))
        .or_else(|| text.find("R$").map(|at| (Currency::Brl, at + 2)))?;
    let amount = parse_amount(&first_number(&text[at..])?)?;
    (!amount.is_sign_negative()).then(|| Money::new(amount, currency))
}

/// Satisfaction as a percentage, or as stars from 0 to 5 turned into one.
pub(crate) fn satisfaction(text: &str) -> Option<Percentage> {
    percent(text).or_else(|| {
        let stars = parse_amount(&first_number(text)?)?;
        (stars <= Decimal::from(5))
            .then(|| Percentage::new(stars * Decimal::from(20)).ok())
            .flatten()
    })
}

/// Letters and digits only, accents dropped, in upper case.
pub(crate) fn folded(text: &str) -> String {
    text.chars().filter_map(fold).collect()
}
