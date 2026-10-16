//! The Mercado Livre crawler (#152, ADR 0035): searches Mercado Livre's
//! public search pages for items, as a source of Supplier Offers, through
//! the shared [`Crawler`]. It reads only what the page carries for
//! machines: the preloaded state of the search, or its JSON-LD. Built only
//! while its flag is on.

use std::str::FromStr;
use std::sync::Arc;

use mascate_catalog::{
    Found, FoundOffer, FoundShop, OfferOrder, OfferSearch, OfferSource, ProductSource,
};
use mascate_kernel::{Currency, Money, PlatformError, Timestamp, folded_words};
use mascate_platform::{Flag, FlagKind, Phase};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::Value;

use crate::answers::{decimal, page_on};
use crate::crawler::{Crawler, assigned_json, json_ld, script_json};

/// Mercado Livre's search pages, unless a test points elsewhere.
pub const LISTING_URL: &str = "https://lista.mercadolivre.com.br";

/// The crawler, off until the owner turns it on (ADR 0006).
pub const MERCADO_LIVRE_CRAWLER: Flag = Flag {
    key: "integrations.mercado_livre_crawler",
    name: "Crawler do Mercado Livre",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "Os Termos e condições de uso para desenvolvedores do Mercado Livre (7.6) proíbem \
             robôs e scraping; os termos do site não foram lidos. O robots.txt deixa ler a \
             busca, e o crawler lê devagar, respeita o robots.txt e para quando o site bloqueia.",
    risk: "O Mercado Livre pode bloquear o endereço de internet da sua casa, inclusive para a sua \
           conta de vendedor, ou sancionar a conta ligada a ele.",
};

/// Items on a search page.
const PAGE_SIZE: u32 = 48;

/// Mercado Livre's own domain: links read from a page must point here.
const DOMAIN: &str = "mercadolivre.com.br";

/// Who sells an item whose seller the page does not show.
const UNNAMED_SELLER: &str = "Mercado Livre";

/// Mercado Livre's public search, read by the crawler.
pub struct MercadoLivreCrawler {
    listing: String,
    crawler: Arc<Crawler>,
}

impl MercadoLivreCrawler {
    /// The search pages at `listing`, read by `crawler`.
    pub fn new(listing: &str, crawler: Arc<Crawler>) -> Self {
        Self {
            listing: listing.trim_end_matches('/').to_owned(),
            crawler,
        }
    }

    /// The address of `search`'s page: the words joined by dashes, then
    /// the first item of the page and the order, as Mercado Livre writes
    /// them.
    fn page_url(&self, search: &OfferSearch) -> Result<String, PlatformError> {
        if search
            .category
            .as_deref()
            .is_some_and(|c| !c.trim().is_empty())
        {
            return Err(PlatformError::Refused(
                "o crawler do Mercado Livre não filtra por categoria".into(),
            ));
        }
        if search.shop.is_some() {
            return Err(PlatformError::Refused(
                "o crawler do Mercado Livre não busca por loja".into(),
            ));
        }
        let words = folded_words(&search.keyword).join("-").to_ascii_lowercase();
        if words.is_empty() {
            return Err(PlatformError::Refused("digite o que procurar".into()));
        }
        let order = match search.order {
            OfferOrder::Relevance => "",
            OfferOrder::LowestPrice => "_OrderId_PRICE",
            OfferOrder::Sales | OfferOrder::HighestCommission => {
                return Err(PlatformError::Refused(
                    "a busca do Mercado Livre não ordena por isso".into(),
                ));
            }
        };
        let from = match search.page.max(1) {
            1 => String::new(),
            page => format!("_Desde_{}", (page - 1) * PAGE_SIZE + 1),
        };
        Ok(format!("{}/{words}{from}{order}", self.listing))
    }
}

impl OfferSource for MercadoLivreCrawler {
    fn source(&self) -> ProductSource {
        ProductSource::MercadoLivreCrawler
    }

    fn search_offers(&self, search: &OfferSearch) -> Result<Found<FoundOffer>, PlatformError> {
        let url = self.page_url(search)?;
        Ok(search_page(&self.crawler.page(&url)?))
    }

    fn search_shops(&self, _: &str, _: u32) -> Result<Found<FoundShop>, PlatformError> {
        Err(PlatformError::Refused(
            "o crawler do Mercado Livre não busca lojas".into(),
        ))
    }

    fn orders(&self) -> &[OfferOrder] {
        &[OfferOrder::Relevance, OfferOrder::LowestPrice]
    }

    fn finds_shops(&self) -> bool {
        false
    }

    fn blocked_until(&self) -> Option<Timestamp> {
        self.crawler.blocked_until(&self.listing)
    }
}

/// The items on a search page: from its preloaded state, else from its
/// JSON-LD; each item once.
fn search_page(html: &str) -> Found<FoundOffer> {
    let state = script_json(html, "__PRELOADED_STATE__")
        .or_else(|| assigned_json(html, "__PRELOADED_STATE__"));
    let mut items = Vec::new();
    let mut more = false;
    if let Some(state) = &state {
        walk(state, &mut |value| {
            if let Some(card) = value.get("polycard") {
                items.extend(polycard_offer(card));
            }
            if let Some(next) = value.get("next_page") {
                more |= next.get("show").and_then(Value::as_bool).unwrap_or(false);
            }
        });
    }
    if items.is_empty() {
        for block in json_ld(html) {
            walk(&block, &mut |value| {
                if is_product(value) {
                    items.extend(ld_offer(value));
                }
            });
        }
    }
    more |= html.contains("rel=\"next\"");
    let mut seen = std::collections::HashSet::new();
    items.retain(|item: &FoundOffer| seen.insert(item.id.clone()));
    Found { items, more }
}

/// Calls `visit` on `value` and on every object and array inside it.
fn walk(value: &Value, visit: &mut impl FnMut(&Value)) {
    visit(value);
    match value {
        Value::Object(fields) => fields.values().for_each(|inner| walk(inner, visit)),
        Value::Array(values) => values.iter().for_each(|inner| walk(inner, visit)),
        _ => {}
    }
}

/// An item of the search as the page's state describes it: a card with
/// its metadata, its pictures and components for the title, the price, the
/// seller and the reviews.
fn polycard_offer(card: &Value) -> Option<FoundOffer> {
    let metadata = card.get("metadata")?;
    let id = item_id(metadata.get("id")?.as_str()?)?;
    let link = item_link(metadata.get("url")?.as_str()?)?;
    let components = card.get("components")?.as_array()?;
    let component = |kind: &str| {
        components
            .iter()
            .find(|component| component.get("type").and_then(Value::as_str) == Some(kind))
            .and_then(|component| component.get(kind))
    };
    let title = component("title")?.get("text")?.as_str()?.trim().to_owned();
    let price = component("price")?.get("current_price")?;
    let price = money(price.get("value")?, price.get("currency"))?;
    let seller = component("seller").and_then(filled).and_then(seller_name);
    let reviews = component("review_compacted").and_then(filled);
    let image = card
        .pointer("/pictures/pictures/0/id")
        .and_then(Value::as_str)
        .filter(|id| {
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .map(|id| format!("https://http2.mlstatic.com/D_NQ_NP_{id}-O.jpg"));
    Some(FoundOffer {
        id,
        title,
        link,
        image,
        shop: shop(seller),
        price,
        highest_price: price,
        sales: reviews.as_deref().map(sold).unwrap_or(0),
        commission: None,
        rating: reviews.as_deref().and_then(rating),
    })
}

/// A component's text with its placeholders filled: `"Por {seller}"` with
/// the value whose key is `seller`; icons and unknown placeholders dropped.
fn filled(part: &Value) -> Option<String> {
    let mut text = part.get("text")?.as_str()?.to_owned();
    for value in part
        .get("values")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(key), Some(label)) = (
            value.get("key").and_then(Value::as_str),
            value.pointer("/label/text").and_then(Value::as_str),
        ) else {
            continue;
        };
        text = text.replace(&format!("{{{key}}}"), label);
    }
    let mut clean = String::new();
    let mut inside = false;
    for c in text.chars() {
        match c {
            '{' => inside = true,
            '}' => inside = false,
            c if !inside => clean.push(c),
            _ => {}
        }
    }
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    (!clean.is_empty()).then_some(clean)
}

fn seller_name(text: String) -> Option<String> {
    let name = ["Vendido por ", "Por ", "por "]
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
        .unwrap_or(&text)
        .trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// The seller as a shop, known by its name: the page shows no seller id.
fn shop(seller: Option<String>) -> FoundShop {
    let name = seller.unwrap_or_else(|| UNNAMED_SELLER.to_owned());
    FoundShop {
        id: folded_words(&name).join("-").to_ascii_lowercase(),
        name,
        link: format!("https://www.{DOMAIN}"),
        commission: None,
        rating: None,
    }
}

/// The stars in a review line such as `"4.8 | +10mil vendidos"`.
fn rating(text: &str) -> Option<Decimal> {
    let first = text.split(['|', ' ', '(']).find(|word| !word.is_empty())?;
    Decimal::from_str(&first.replace(',', "."))
        .ok()
        .filter(|stars| (Decimal::ZERO..=Decimal::from(5)).contains(stars))
}

/// The units sold in a line such as `"+10mil vendidos"`, `"+500 vendidos"`
/// or `"1.234 vendidos"`; 0 when it does not say.
fn sold(text: &str) -> u32 {
    let lower = text.to_lowercase();
    let Some(before) = lower.find("vendid").map(|at| &lower[..at]) else {
        return 0;
    };
    let count = before.rsplit('|').next().unwrap_or_default().trim();
    let count = count.trim_start_matches('+').trim();
    let digits: String = count
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .collect();
    let unit = count[digits.len()..].trim();
    let factor = if unit.starts_with("mil") && !unit.starts_with("milh") {
        1_000
    } else if unit.starts_with("mi") {
        1_000_000
    } else {
        1
    };
    let number = if factor == 1 {
        digits.replace(['.', ','], "")
    } else {
        digits.replace('.', "").replace(',', ".")
    };
    Decimal::from_str(&number)
        .ok()
        .map(|number| number * Decimal::from(factor))
        .and_then(|units| units.trunc().to_u32())
        .unwrap_or(0)
}

/// Whether a JSON-LD node is a schema.org Product.
fn is_product(value: &Value) -> bool {
    match value.get("@type") {
        Some(Value::String(kind)) => kind == "Product",
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "Product"),
        _ => false,
    }
}

/// An item as a JSON-LD Product with its Offer (or AggregateOffer).
fn ld_offer(product: &Value) -> Option<FoundOffer> {
    let offer = match product.get("offers")? {
        Value::Array(offers) => offers.first()?,
        offer => offer,
    };
    let link = product
        .get("url")
        .or_else(|| offer.get("url"))
        .and_then(Value::as_str)
        .and_then(item_link)?;
    let id = item_id(&link).or_else(|| item_id(product.get("sku")?.as_str()?))?;
    let currency = offer.get("priceCurrency");
    let price = money(
        offer.get("price").or_else(|| offer.get("lowPrice"))?,
        currency,
    )?;
    let highest_price = offer
        .get("highPrice")
        .and_then(|high| money(high, currency))
        .filter(|high| high.amount() >= price.amount())
        .unwrap_or(price);
    let image = match product.get("image") {
        Some(Value::Array(images)) => images.first(),
        image => image,
    }
    .and_then(|image| image.as_str().or_else(|| image.get("url")?.as_str()))
    .and_then(|image| page_on(image, "mlstatic.com"));
    let rating = product
        .pointer("/aggregateRating/ratingValue")
        .and_then(number_in)
        .filter(|stars| (Decimal::ZERO..=Decimal::from(5)).contains(stars));
    let seller = offer
        .pointer("/seller/name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    Some(FoundOffer {
        id,
        title: product.get("name")?.as_str()?.trim().to_owned(),
        link,
        image,
        shop: shop(seller),
        price,
        highest_price,
        sales: 0,
        commission: None,
        rating,
    })
}

/// The item's id in Mercado Livre's form, `MLB` and its digits, from an id
/// such as `MLB4012345678` or a link with `MLB-4012345678` in it.
fn item_id(text: &str) -> Option<String> {
    let upper = text.to_ascii_uppercase();
    let mut from = 0;
    while let Some(at) = upper[from..].find("MLB").map(|at| from + at) {
        let rest = &upper[at + 3..];
        let digits: String = rest
            .trim_start_matches('-')
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if !digits.is_empty() {
            return Some(format!("MLB{digits}"));
        }
        from = at + 3;
    }
    None
}

/// An item's page on Mercado Livre over HTTPS, without what follows `#`.
fn item_link(url: &str) -> Option<String> {
    let url = url.trim().split('#').next()?;
    let url = if url.contains("://") {
        url.to_owned()
    } else {
        format!("https://{}", url.trim_start_matches('/'))
    };
    page_on(&url, DOMAIN)
}

/// An amount written as a JSON number or text, in `currency` (reais when
/// the page does not say); never negative.
fn money(amount: &Value, currency: Option<&Value>) -> Option<Money> {
    let currency = match currency.and_then(Value::as_str) {
        Some(code) => Currency::from_code(code)?,
        None => Currency::Brl,
    };
    number_in(amount)
        .filter(|amount| !amount.is_sign_negative())
        .map(|amount| Money::new(amount, currency))
}

fn number_in(value: &Value) -> Option<Decimal> {
    match value {
        Value::Number(number) => decimal(number),
        Value::String(text) => Decimal::from_str(text.trim()).ok(),
        _ => None,
    }
}
