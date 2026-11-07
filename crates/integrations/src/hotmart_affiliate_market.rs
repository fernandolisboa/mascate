//! The Hotmart affiliate market reader (#221, ADR 0039 decisions 2 and 3):
//! the Mercado de Afiliação only opens with the owner's login, so the app
//! opens it in the installed browser, on the Mascate's own profile where the
//! owner logged in by hand, and reads what the page already drew: the
//! temperature, the commission, the Blueprint, the satisfaction, the price
//! and whether affiliation is immediate. Built only while its flag is on.
//!
//! It reads only when the owner asks, at most a few pages each time and
//! never two within seconds of each other, keeps each page for an hour and
//! stops for a day at the first login, robot check or block. It never
//! clicks, never asks for an affiliation and makes no request of its own to
//! Hotmart's internal API. The page addresses and labels come from
//! Hotmart's public help, not from a real account: the phase 3 regression
//! pass (#116) records the real pages (see the fixtures' README).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, TimeDelta};
use mascate_affiliate::{
    AffiliatePlatform, Affiliation, FoundProspect, MAX_TEMPERATURE, ProspectCategory,
    ProspectSource, SignalOrigin, Signals,
};
use mascate_kernel::{
    Clock, Currency, Money, Pause, Percentage, PlatformError, Timestamp, fold, parse_amount,
};
use mascate_platform::{Flag, FlagKind, Phase};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;

use crate::affiliate_panel::{Panel, PanelState, lands_on_login};
use crate::browser::BrowserSession;
use crate::hotmart_crawler::hotmart_categories;

/// The Mercado de Afiliação, where the owner logs in, unless a test points
/// elsewhere.
pub const MARKET_URL: &str = "https://app.hotmart.com/market";

/// The reader, off until the owner turns it on (ADR 0006).
pub const HOTMART_AFFILIATE_MARKET: Flag = Flag {
    key: "integrations.hotmart_affiliate_market",
    name: "Leitura do mercado de afiliação da Hotmart",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "Os termos de uso da Hotmart, item 3.5(m), proíbem sistemas automatizados, robôs e \
             scripts, e o mercado de afiliação só abre com o seu login. O app abre o mercado no \
             Edge ou no Chrome, num perfil só do Mascate em que você entrou à mão, só quando você \
             clica: no máximo 5 páginas por clique, uma a cada 5 segundos ou mais, a mesma \
             página dentro de 1 hora vem da memória. Só lê o que a página mostra: não clica, \
             não pede afiliação e não resolve captcha; no primeiro login, verificação ou \
             bloqueio, para por 24 horas.",
    risk: "A Hotmart pode suspender ou encerrar a sua conta de afiliado, com as comissões \
           ainda não pagas.",
};

/// Pages one click reads at most, cached ones aside.
pub const PAGES_PER_CLICK: usize = 5;
/// The least time between two pages read.
pub const INTERVAL: Duration = Duration::from_secs(5);
/// How long a page read answers again from memory.
const KEPT_FOR: TimeDelta = TimeDelta::hours(1);
/// How long the reader leaves Hotmart alone after a login, a robot check
/// or a block.
pub const STOPPED_FOR: TimeDelta = TimeDelta::hours(24);

/// Reads what a market page drew: the address it ended on, whether a
/// robot check shows, each product card (by its link to the product's
/// details, the card being the largest block around that link with no
/// other product) as its lines of text, and the whole page's lines for a
/// product's own page. It reads the page and nothing else.
///
/// To record a page for a fixture, paste this into the console of the
/// browser's developer tools on the market's page and copy what it prints
/// with `copy(...)` around it.
pub const MARKET_SCRIPT: &str = r#"(() => {
  const clean = (text) => (text || "").replace(/\s+/g, " ").trim();
  const lines = (element) => (element.innerText || element.textContent || "")
    .split("\n").map(clean).filter((line) => line !== "");
  const product = 'a[href*="productUcode="]';
  const ucode = (href) => { try { return new URL(href).searchParams.get("productUcode"); } catch (_) { return href; } };
  const products = (element) => new Set(Array.from(element.querySelectorAll(product)).map((a) => ucode(a.href)));
  const hrefs = (element) => Array.from(element.querySelectorAll("a[href]")).map((a) => a.href);
  const cards = [];
  const seen = new Set();
  for (const link of document.querySelectorAll(product)) {
    if (seen.has(ucode(link.href))) continue;
    seen.add(ucode(link.href));
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
struct MarketPage {
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    captcha: bool,
    #[serde(default)]
    cards: Vec<Card>,
    #[serde(default)]
    heading: String,
    #[serde(default)]
    links: Vec<String>,
    #[serde(default)]
    lines: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct Card {
    link: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    links: Vec<String>,
    #[serde(default)]
    lines: Vec<String>,
}

/// Words near the top of a page that mean Hotmart refused it, as letters
/// and digits only.
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

/// Why the reader stopped asking Hotmart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketStop {
    /// The market sent the browser to the login.
    Login,
    /// The page showed a check of who is asking (a captcha).
    Check,
    /// Hotmart refused the page.
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

/// Until when, and why, the reader leaves Hotmart alone.
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

/// Hotmart's Mercado de Afiliação, read through the shared browser.
pub struct HotmartAffiliateMarket {
    market: String,
    panel: Panel,
    clock: Arc<dyn Clock>,
    pause: Arc<dyn Pause>,
    /// Where a stop is written, so it outlasts a restart of the app.
    stop_file: Option<PathBuf>,
    /// Held through a whole page read: one page at a time.
    turn: Mutex<()>,
    pacing: Mutex<Pacing>,
}

impl HotmartAffiliateMarket {
    /// The market at `market`, opened by `browser`, timed by `clock` and
    /// waiting with `pause`.
    pub fn new(
        market: &str,
        browser: Arc<BrowserSession>,
        clock: Arc<dyn Clock>,
        pause: Arc<dyn Pause>,
    ) -> Self {
        Self {
            market: market.trim_end_matches('/').to_owned(),
            panel: Panel::new(browser),
            clock,
            pause,
            stop_file: None,
            turn: Mutex::new(()),
            pacing: Mutex::new(Pacing::default()),
        }
    }

    /// Keeps a stop in `file` and starts with the one written there.
    pub fn with_stop_file(mut self, file: PathBuf) -> Self {
        self.pacing().stopped = std::fs::read_to_string(&file)
            .ok()
            .as_deref()
            .and_then(read_stop);
        self.stop_file = Some(file);
        self
    }

    /// Where the market's reading stands, as its Connection shows it.
    pub fn state(&self) -> PanelState {
        self.panel.state()
    }

    /// Until when, and why, the reader leaves Hotmart alone; `None` while
    /// it may read.
    pub fn stopped(&self) -> Option<Stopped> {
        let now = self.clock.now();
        self.pacing().stopped.filter(|stopped| stopped.until > now)
    }

    /// Opens the market in the browser's window for the owner to log in, or
    /// to answer a robot check, by hand. A stop for a login or a check ends:
    /// the owner answered it. A block stays for its day.
    pub fn open_login(&self) -> Result<(), PlatformError> {
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

    /// The listing of `category`, or of the whole market, at `page`.
    fn listing_url(
        &self,
        category: Option<&ProspectCategory>,
        page: usize,
    ) -> Result<String, PlatformError> {
        match category {
            None => Ok(format!("{}?page={page}", self.market)),
            Some(category) => hotmart_categories()
                .into_iter()
                .find(|known| known.id == category.id)
                .map(|known| format!("{}?category={}&page={page}", self.market, known.id))
                .ok_or_else(|| {
                    PlatformError::Refused(
                        "isso não é uma categoria do mercado de afiliação da Hotmart".into(),
                    )
                }),
        }
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
                return Err(stopped_error(stopped.why));
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
        let read = self.panel.evaluate(url, MARKET_SCRIPT, &is_drawn);
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
            return Err(stopped_error(why));
        }
        self.pacing()
            .kept
            .insert(url.to_owned(), (page.clone(), read_at));
        Ok(Some(page))
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

impl ProspectSource for HotmartAffiliateMarket {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Hotmart
    }

    fn origin(&self) -> SignalOrigin {
        SignalOrigin::Market
    }

    fn categories(&self) -> Result<Vec<ProspectCategory>, PlatformError> {
        Ok(hotmart_categories())
    }

    /// The listing's pages in order, up to the click's pages, until one
    /// shows nothing new. A stop after the first page keeps what was read;
    /// the stop itself shows on the screen.
    fn prospects(
        &self,
        category: Option<&ProspectCategory>,
    ) -> Result<Vec<FoundProspect>, PlatformError> {
        let mut click = Click::default();
        let mut found: Vec<FoundProspect> = Vec::new();
        let mut seen = HashSet::new();
        for number in 1..=PAGES_PER_CLICK {
            let page = match self.page(&self.listing_url(category, number)?, &mut click) {
                Ok(Some(page)) => page,
                Ok(None) => break,
                Err(_) if !found.is_empty() => break,
                Err(error) => return Err(error),
            };
            let before = found.len();
            found.extend(
                page.cards
                    .iter()
                    .filter_map(listed)
                    .filter(|prospect| seen.insert(prospect.key.clone())),
            );
            if found.len() == before {
                break;
            }
        }
        Ok(found)
    }

    /// Reads the product's own page in the market, which `known` keeps the
    /// address of: what the card shows and the producer.
    fn details(&self, known: &FoundProspect) -> Result<FoundProspect, PlatformError> {
        let ucode = known
            .signals
            .affiliation_page
            .as_deref()
            .and_then(product_ucode)
            .ok_or_else(|| {
                PlatformError::Refused(
                    "esse produto ainda não tem o endereço do mercado de afiliação da Hotmart: \
                     garimpe no mercado ou leia a página do produto na vitrine antes"
                        .into(),
                )
            })?;
        let url = format!("{}/details?productUcode={ucode}", self.market);
        let page = self
            .page(&url, &mut Click::default())?
            .ok_or_else(|| PlatformError::Failed("a página do produto não foi lida".into()))?;
        Ok(details(&page, &ucode, known))
    }

    fn blocked_until(&self) -> Option<Timestamp> {
        self.stopped().map(|stopped| stopped.until)
    }
}

/// The address of a product in the market, where the owner asks for the
/// affiliation: one form for every reader, so the vitrine and the market
/// name the same product the same way.
pub fn affiliation_page(ucode: &str) -> Option<String> {
    is_ucode(ucode).then(|| format!("{MARKET_URL}/details?productUcode={ucode}"))
}

/// The product's code in the market (`productUcode`), from a link to it.
fn product_ucode(link: &str) -> Option<String> {
    query_value(link, "productUcode").filter(|ucode| is_ucode(ucode))
}

/// The value of `name` in the query of `link`.
fn query_value(link: &str, name: &str) -> Option<String> {
    let query = link.split_once('?')?.1.split('#').next()?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| value.to_owned())
    })
}

/// A Hotmart code such as a UUID: letters, digits and dashes.
fn is_ucode(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Whether the page is ready to read: the login, a robot check, product
/// cards, or a product's own page.
fn is_drawn(value: &Value) -> bool {
    MarketPage::deserialize(value).is_ok_and(|page| {
        lands_on_login(&page.url)
            || page.captcha
            || !page.cards.is_empty()
            || labelled(&page.lines, TEMPERATURE).is_some()
    })
}

fn refused(page: &MarketPage) -> bool {
    std::iter::once(&page.title)
        .chain(page.lines.iter().take(BLOCK_LINES))
        .map(|line| folded(line))
        .any(|line| BLOCKED_PAGES.iter().any(|word| line.contains(word)))
}

fn stopped_error(why: MarketStop) -> PlatformError {
    match why {
        MarketStop::Login => PlatformError::Expired,
        MarketStop::Check => PlatformError::Refused(
            "a Hotmart pediu a verificação contra robôs, que só você responde".into(),
        ),
        MarketStop::Block => PlatformError::RateLimited,
    }
}

fn read_stop(text: &str) -> Option<Stopped> {
    let (why, until) = text.trim().split_once(' ')?;
    Some(Stopped {
        why: MarketStop::from_code(why)?,
        until: DateTime::parse_from_rfc3339(until).ok()?.to_utc(),
    })
}

/// A card of the listing as a Prospect: under Hotmart's product id when the
/// card shows it, as the vitrine keys it, else under the market's code.
fn listed(card: &Card) -> Option<FoundProspect> {
    let ucode = product_ucode(&card.link)?;
    let name = Some(card.title.trim())
        .filter(|title| !title.is_empty())
        .or_else(|| card.lines.first().map(|line| line.trim()))
        .filter(|name| !name.is_empty())?;
    Some(FoundProspect {
        platform: AffiliatePlatform::Hotmart,
        key: product_id(&card.links, &card.lines).unwrap_or_else(|| ucode.clone()),
        name: name.to_owned(),
        signals: Signals {
            affiliation_page: affiliation_page(&ucode),
            ..signals(&card.lines)
        },
    })
}

/// A product's own page in the market, under `known`'s key unless the page
/// shows Hotmart's product id.
fn details(page: &MarketPage, ucode: &str, known: &FoundProspect) -> FoundProspect {
    let name = Some(page.heading.trim())
        .filter(|heading| !heading.is_empty())
        .map_or_else(|| known.name.clone(), str::to_owned);
    FoundProspect {
        platform: AffiliatePlatform::Hotmart,
        key: product_id(&page.links, &page.lines).unwrap_or_else(|| known.key.clone()),
        name,
        signals: Signals {
            affiliation_page: affiliation_page(ucode),
            ..signals(&page.lines)
        },
    }
}

const TEMPERATURE: &[&str] = &["Temperatura"];
const COMMISSION: &[&str] = &[
    "Comissão",
    "Sua comissão",
    "Ganhe até",
    "Você ganha",
    "Receba até",
];
const PRICE: &[&str] = &["Preço", "Valor do produto"];
const BLUEPRINT: &[&str] = &["Blueprint"];
const SATISFACTION: &[&str] = &["Satisfação"];
const CREATOR: &[&str] = &["Produtor", "Produtora", "Autor", "Autora"];
const PRODUCT_ID: &[&str] = &["ID do produto", "Código do produto"];
/// Words of an affiliation the producer approves, then of one at once.
const ON_REQUEST: [&str; 4] = [
    "AFILIACAOCOMAPROVACAO",
    "AFILIACAOMEDIANTEAPROVACAO",
    "SOLICITARAFILIACAO",
    "AGUARDAAPROVACAO",
];
const IMMEDIATE: [&str; 2] = ["AFILIACAOIMEDIATA", "AFILIESEAGORA"];

/// The signals the market shows in `lines`, each left empty when it is
/// missing or does not read.
fn signals(lines: &[String]) -> Signals {
    let commission = labelled_all(lines, COMMISSION);
    Signals {
        temperature: labelled(lines, TEMPERATURE).and_then(degrees),
        commission: commission.iter().find_map(|text| money(text)),
        commission_rate: commission.iter().find_map(|text| percent(text)),
        price: labelled(lines, PRICE).and_then(money),
        blueprint: labelled(lines, BLUEPRINT).and_then(percent),
        satisfaction: labelled(lines, SATISFACTION).and_then(satisfaction),
        creator: named(lines, CREATOR),
        affiliation: affiliation(lines),
        ..Signals::default()
    }
}

/// Every text that follows one of `labels` in `lines`: the line itself
/// when the value is on it ("Temperatura 87°", "Blueprint: 85%"), else the
/// next line.
fn labelled_all<'a>(lines: &'a [String], labels: &[&str]) -> Vec<&'a str> {
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

fn labelled<'a>(lines: &'a [String], labels: &[&str]) -> Option<&'a str> {
    labelled_all(lines, labels).into_iter().next()
}

/// A name after one of `labels`: after a colon on the label's line, or on
/// the next line.
fn named(lines: &[String], labels: &[&str]) -> Option<String> {
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

/// Hotmart's product id, from a link (`productId=`) or a labelled line.
fn product_id(links: &[String], lines: &[String]) -> Option<String> {
    let is_id = |text: &str| !text.is_empty() && text.chars().all(|c| c.is_ascii_digit());
    links
        .iter()
        .find_map(|link| query_value(link, "productId").filter(|id| is_id(id)))
        .or_else(|| {
            labelled(lines, PRODUCT_ID).and_then(|text| {
                let id = first_number(text)?;
                is_id(&id).then_some(id)
            })
        })
}

fn affiliation(lines: &[String]) -> Option<Affiliation> {
    let folded_lines: Vec<String> = lines.iter().map(|line| folded(line)).collect();
    let has = |words: &[&str]| {
        folded_lines
            .iter()
            .any(|line| words.iter().any(|word| line.contains(word)))
    };
    if has(&ON_REQUEST) {
        Some(Affiliation::OnRequest)
    } else if has(&IMMEDIATE) {
        Some(Affiliation::Immediate)
    } else {
        None
    }
}

/// The first number in `text`, as written: `87`, `4,6`, `1.234,56`.
fn first_number(text: &str) -> Option<String> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let number: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect();
    Some(number.trim_end_matches([',', '.']).to_owned())
}

/// Degrees from 0 to Hotmart's 150°.
fn degrees(text: &str) -> Option<u32> {
    first_number(text)?
        .parse()
        .ok()
        .filter(|degrees| *degrees <= MAX_TEMPERATURE)
}

/// The number just before a `%`: `50%`, `12,5 %`.
fn percent(text: &str) -> Option<Percentage> {
    let before = text[..text.find('%')?].trim_end();
    let start = before
        .rfind(|c: char| !(c.is_ascii_digit() || c == ',' || c == '.'))
        .map_or(0, |index| index + 1);
    Percentage::new(parse_amount(&before[start..])?).ok()
}

/// An amount after `R$` or `US$`, in its currency.
fn money(text: &str) -> Option<Money> {
    let (currency, at) = text
        .find("US$")
        .map(|at| (Currency::Usd, at + 3))
        .or_else(|| text.find("R$").map(|at| (Currency::Brl, at + 2)))?;
    let amount = parse_amount(&first_number(&text[at..])?)?;
    (!amount.is_sign_negative()).then(|| Money::new(amount, currency))
}

/// Satisfaction as a percentage, or as stars from 0 to 5 turned into one.
fn satisfaction(text: &str) -> Option<Percentage> {
    percent(text).or_else(|| {
        let stars = parse_amount(&first_number(text)?)?;
        (stars <= Decimal::from(5))
            .then(|| Percentage::new(stars * Decimal::from(20)).ok())
            .flatten()
    })
}

/// Letters and digits only, accents dropped, in upper case.
fn folded(text: &str) -> String {
    text.chars().filter_map(fold).collect()
}
