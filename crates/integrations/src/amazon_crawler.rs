//! The Amazon crawler (#153, ADR 0035): searches amazon.com.br's public
//! search pages for items, as a source of Supplier Offers, through the
//! shared [`Crawler`]. The search carries no data for machines, so a small
//! bounded reader cuts out only the result blocks (`data-asin`) and the few
//! marks inside them; no HTML parser. Built only while its flag is on.

use std::str::FromStr;
use std::sync::Arc;

use mascate_catalog::{
    Found, FoundOffer, FoundShop, OfferOrder, OfferSearch, OfferSource, ProductSource,
};
use mascate_kernel::{Currency, Money, PlatformError, Timestamp};
use mascate_platform::{Flag, FlagKind, Phase};
use rust_decimal::Decimal;

use crate::answers::page_on;
use crate::crawler::{Crawler, count_before, no_shops, search_words, stars};

/// Amazon's site, unless a test points elsewhere.
pub const SEARCH_URL: &str = "https://www.amazon.com.br";

/// The crawler, off until the owner turns it on (ADR 0006).
pub const AMAZON_CRAWLER: Flag = Flag {
    key: "integrations.amazon_crawler",
    name: "Crawler da Amazon",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "As Condições de Uso da Amazon proíbem mineração de dados, robôs e ferramentas \
             parecidas de coleta e extração de dados. O robots.txt deixa ler a busca, e o \
             crawler lê devagar, respeita o robots.txt e para quando o site bloqueia.",
    risk: "A Amazon pode bloquear o endereço de internet da sua casa, ou sancionar as contas \
           ligadas a ele: a de vendedor e a de Associado.",
};

/// The site, as the crawler's refusals name it.
const SITE: &str = "da Amazon";

/// Where the items' pages live, whatever address the search was read from.
const ITEM_PAGES: &str = "https://www.amazon.com.br";

/// The domain of Amazon's pictures.
const PICTURES: &str = "media-amazon.com";

/// The mark of a search result's opening tag.
const RESULT_MARK: &str = "data-component-type=\"s-search-result\"";

/// How much of one result block is read: a result is a few kilobytes, and
/// whatever follows past this is not one.
const MAX_BLOCK_BYTES: usize = 64 * 1024;

/// Amazon's public search, read by the crawler.
pub struct AmazonCrawler {
    site: String,
    crawler: Arc<Crawler>,
}

impl AmazonCrawler {
    /// The search pages of `site`, read by `crawler`.
    pub fn new(site: &str, crawler: Arc<Crawler>) -> Self {
        Self {
            site: site.trim_end_matches('/').to_owned(),
            crawler,
        }
    }

    /// The address of `search`'s page: `/s?k=` with the words joined by
    /// `+`, then the order and the page, as Amazon writes them.
    fn page_url(&self, search: &OfferSearch) -> Result<String, PlatformError> {
        let words = search_words(search, SITE)?.join("+");
        let order = match search.order {
            OfferOrder::Relevance => "",
            OfferOrder::LowestPrice => "&s=price-asc-rank",
            OfferOrder::Sales => "&s=exact-aware-popularity-rank",
            OfferOrder::HighestCommission => {
                return Err(PlatformError::Refused(
                    "a busca da Amazon não ordena por comissão".into(),
                ));
            }
        };
        let page = match search.page.max(1) {
            1 => String::new(),
            page => format!("&page={page}"),
        };
        Ok(format!("{}/s?k={words}{order}{page}", self.site))
    }
}

impl OfferSource for AmazonCrawler {
    fn source(&self) -> ProductSource {
        ProductSource::AmazonCrawler
    }

    fn search_offers(&self, search: &OfferSearch) -> Result<Found<FoundOffer>, PlatformError> {
        let url = self.page_url(search)?;
        Ok(search_page(&self.crawler.page(&url)?))
    }

    fn search_shops(&self, _: &str, _: u32) -> Result<Found<FoundShop>, PlatformError> {
        Err(no_shops(SITE))
    }

    fn orders(&self) -> &[OfferOrder] {
        &[
            OfferOrder::Sales,
            OfferOrder::Relevance,
            OfferOrder::LowestPrice,
        ]
    }

    fn finds_shops(&self) -> bool {
        false
    }

    fn blocked_until(&self) -> Option<Timestamp> {
        self.crawler.blocked_until(&self.site)
    }
}

/// The items on a search page, each once: one per result block whose
/// ASIN reads as one.
fn search_page(html: &str) -> Found<FoundOffer> {
    let starts: Vec<usize> = html
        .match_indices(RESULT_MARK)
        .filter_map(|(at, _)| html[..at].rfind('<'))
        .collect();
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (index, &start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(html.len());
        let block = bounded(&html[start..end]);
        if let Some(item) = result_offer(block)
            && seen.insert(item.id.clone())
        {
            items.push(item);
        }
    }
    Found {
        items,
        more: has_next_page(html),
    }
}

/// The first [`MAX_BLOCK_BYTES`] of `block`, cut where a character starts.
fn bounded(block: &str) -> &str {
    if block.len() <= MAX_BLOCK_BYTES {
        return block;
    }
    let mut end = MAX_BLOCK_BYTES;
    while !block.is_char_boundary(end) {
        end -= 1;
    }
    &block[..end]
}

/// An item as one result block shows it: the ASIN on the block, the
/// longest heading as its title, the first price that is not a crossed-out
/// one, the picture, the stars and the purchases of the last month. A block
/// without a price is an item the page shows without one, not a failure.
fn result_offer(block: &str) -> Option<FoundOffer> {
    let tags = tags(block);
    let asin = tags.first()?.attribute("data-asin")?;
    if asin.len() != 10
        || !asin
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return None;
    }
    let title = tags
        .iter()
        .filter(|tag| tag.name == "h2")
        .map(|tag| text_until(block, tag, "</h2"))
        .max_by_key(String::len)
        .filter(|title| !title.is_empty())?;
    let price = tags
        .iter()
        .position(|tag| tag.has_class("a-price") && !tag.has_class("a-text-price"))
        .and_then(|at| tags[at..].iter().find(|tag| tag.has_class("a-offscreen")))
        .and_then(|tag| reais(&text_until(block, tag, "</span")));
    let image = tags
        .iter()
        .find(|tag| tag.name == "img" && tag.has_class("s-image"))
        .and_then(|tag| tag.attribute("src"))
        .and_then(|src| page_on(&src, PICTURES));
    let rating = tags
        .iter()
        .find(|tag| tag.has_class("a-icon-alt"))
        .and_then(|tag| stars(&text_until(block, tag, "</span")));
    let sales = tags
        .iter()
        .filter(|tag| tag.name == "span")
        .map(|tag| text_until(block, tag, "</span"))
        .find(|text| text.contains("no mês passado"))
        .map(|text| count_before(&text, "compra"))
        .unwrap_or(0);
    Some(FoundOffer {
        link: format!("{ITEM_PAGES}/dp/{asin}"),
        id: asin,
        title,
        image,
        shop: FoundShop {
            id: "amazon".into(),
            name: "Amazon".into(),
            link: ITEM_PAGES.into(),
            commission: None,
            rating: None,
        },
        price,
        highest_price: price,
        sales,
        commission: None,
        rating,
    })
}

/// Whether the page links to a next one: Amazon keeps the "next" control
/// on the last page too, marked disabled and without a link.
fn has_next_page(html: &str) -> bool {
    tags(html)
        .iter()
        .find(|tag| tag.has_class("s-pagination-next"))
        .is_some_and(|tag| tag.name == "a" && !tag.has_class("s-pagination-disabled"))
}

/// An amount in reais as the page writes it for people, such as
/// `R$ 1.299,90`; another currency is no price the app reads.
fn reais(text: &str) -> Option<Money> {
    let amount = text.trim().strip_prefix("R$")?.trim();
    if amount.is_empty()
        || !amount
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
    {
        return None;
    }
    Decimal::from_str(&amount.replace('.', "").replace(',', "."))
        .ok()
        .map(|amount| Money::new(amount, Currency::Brl))
}

/// An opening tag: its name in lower case and its attributes as written.
struct Tag<'a> {
    name: String,
    attributes: &'a str,
    /// Where the text after it starts.
    end: usize,
}

impl Tag<'_> {
    /// The value of the attribute `name`, its entities decoded.
    fn attribute(&self, name: &str) -> Option<String> {
        let mut rest = self.attributes;
        loop {
            let at = rest.find(name)?;
            let before = rest[..at].chars().next_back();
            let after = rest[at + name.len()..].trim_start();
            rest = &rest[at + name.len()..];
            if !before.is_some_and(char::is_whitespace) {
                continue;
            }
            let Some(value) = after.strip_prefix('=') else {
                continue;
            };
            let value = value.trim_start();
            let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
            let value = &value[1..];
            return Some(decoded(&value[..value.find(quote)?]));
        }
    }

    fn has_class(&self, class: &str) -> bool {
        self.attribute("class")
            .is_some_and(|classes| classes.split_whitespace().any(|word| word == class))
    }
}

/// Every opening tag in `html`, in order.
fn tags(html: &str) -> Vec<Tag<'_>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(start) = html[from..].find('<').map(|at| from + at) {
        let Some(close) = html[start..].find('>').map(|at| start + at) else {
            break;
        };
        let inside = &html[start + 1..close];
        let name_end = inside
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(inside.len());
        let name = &inside[..name_end];
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()) {
            found.push(Tag {
                name: name.to_ascii_lowercase(),
                attributes: &inside[name_end..],
                end: close + 1,
            });
        }
        from = close + 1;
    }
    found
}

/// The text after `tag` up to `closing`, without the tags inside it, its
/// entities decoded and its spaces collapsed.
fn text_until(html: &str, tag: &Tag, closing: &str) -> String {
    let rest = &html[tag.end..];
    let inner = &rest[..rest.find(closing).unwrap_or(rest.len())];
    let mut text = String::new();
    let mut inside = false;
    for c in inner.chars() {
        match c {
            '<' => inside = true,
            '>' => {
                inside = false;
                text.push(' ');
            }
            c if !inside => text.push(c),
            _ => {}
        }
    }
    decoded(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` with the HTML entities a result block uses written out.
fn decoded(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let entity = rest[1..]
            .find(';')
            .filter(|end| *end <= 10)
            .map(|end| &rest[1..=end]);
        let character = entity.and_then(|entity| match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#')?.parse().ok())
                .and_then(char::from_u32),
        });
        match (entity, character) {
            (Some(entity), Some(character)) => {
                out.push(character);
                rest = &rest[entity.len() + 2..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}
