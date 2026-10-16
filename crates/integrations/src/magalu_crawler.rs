//! The Magalu crawler (#153, ADR 0035): searches magazineluiza.com.br's
//! public search pages for items, as a source of Supplier Offers, through
//! the shared [`Crawler`]. It reads only what the page carries for
//! machines: the page's embedded state (`__NEXT_DATA__`), or its JSON-LD.
//! Built only while its flag is on.

use std::sync::Arc;

use mascate_catalog::{
    Found, FoundOffer, FoundShop, OfferOrder, OfferSearch, OfferSource, ProductSource,
};
use mascate_kernel::{PlatformError, Timestamp, folded_words};
use mascate_platform::{Flag, FlagKind, Phase};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::Value;

use crate::answers::page_on;
use crate::crawler::{
    Crawler, is_product, json_ld, money, no_shops, number_in, script_json, search_words, walk,
};

/// Magalu's site, unless a test points elsewhere.
pub const SEARCH_URL: &str = "https://www.magazineluiza.com.br";

/// The crawler, off until the owner turns it on (ADR 0006).
pub const MAGALU_CRAWLER: Flag = Flag {
    key: "integrations.magalu_crawler",
    name: "Crawler da Magalu",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "Os termos de uso da Magalu não foram lidos: o site recusa acesso automatizado até \
             ao robots.txt, o que conta como proibição. O crawler lê devagar, respeita o \
             robots.txt e para quando o site bloqueia.",
    risk: "A Magalu pode bloquear o endereço de internet da sua casa, inclusive para a sua conta \
           de vendedor, ou sancionar as contas ligadas a ele.",
};

/// The site, as the crawler's refusals name it.
const SITE: &str = "da Magalu";

/// Magalu's own domain: links read from a page must point here.
const DOMAIN: &str = "magazineluiza.com.br";

/// Where the items' pages live, whatever address the search was read from.
const ITEM_PAGES: &str = "https://www.magazineluiza.com.br";

/// The domain of Magalu's pictures.
const PICTURES: &str = "mlcdn.com.br";

/// The size asked of a picture whose address leaves it to the reader.
const PICTURE_SIZE: &str = "800x560";

/// Who sells an item whose seller the page does not show.
const UNNAMED_SELLER: (&str, &str) = ("magazineluiza", "Magazine Luiza");

/// Magalu's public search, read by the crawler.
pub struct MagaluCrawler {
    site: String,
    crawler: Arc<Crawler>,
}

impl MagaluCrawler {
    /// The search pages of `site`, read by `crawler`.
    pub fn new(site: &str, crawler: Arc<Crawler>) -> Self {
        Self {
            site: site.trim_end_matches('/').to_owned(),
            crawler,
        }
    }

    /// The address of `search`'s page: `/busca/` with the words joined by
    /// `+`, then the page and the order, as Magalu writes them.
    fn page_url(&self, search: &OfferSearch) -> Result<String, PlatformError> {
        let words = search_words(search, SITE)?.join("+");
        let mut query = Vec::new();
        match search.page.max(1) {
            1 => {}
            page => query.push(format!("page={page}")),
        }
        match search.order {
            OfferOrder::Relevance => {}
            OfferOrder::LowestPrice => query.push("sortOrientation=asc&sortType=price".into()),
            OfferOrder::Sales => query.push("sortOrientation=desc&sortType=soldQuantity".into()),
            OfferOrder::HighestCommission => {
                return Err(PlatformError::Refused(
                    "a busca da Magalu não ordena por comissão".into(),
                ));
            }
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        Ok(format!("{}/busca/{words}/{query}", self.site))
    }
}

impl OfferSource for MagaluCrawler {
    fn source(&self) -> ProductSource {
        ProductSource::MagaluCrawler
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

/// The items on a search page: the `products` of its embedded state, in
/// the page's order and without the ads beside them, else its JSON-LD;
/// each item once.
fn search_page(html: &str) -> Found<FoundOffer> {
    let mut items = Vec::new();
    let mut more = false;
    if let Some(state) = script_json(html, "__NEXT_DATA__") {
        walk(&state, &mut |value| {
            if let Some(products) = value.get("products").and_then(Value::as_array) {
                items.extend(products.iter().filter_map(state_offer));
            }
            if let Some(pagination) = value.get("pagination") {
                more |= has_more(pagination);
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

/// An item as the page's state describes it: its id, title, path, price,
/// picture, stars and seller. An item without a price is found without
/// one.
fn state_offer(value: &Value) -> Option<FoundOffer> {
    let id = product_id(value.get("id")?.as_str()?)?;
    let title = value.get("title")?.as_str()?.trim();
    let path = value.get("path").or_else(|| value.get("url"))?.as_str()?;
    if title.is_empty() {
        return None;
    }
    let link = item_link(path)?;
    let prices = value.get("price");
    let price = prices
        .and_then(|prices| prices.get("bestPrice").or_else(|| prices.get("price")))
        .and_then(|amount| money(amount, None));
    let image = value.get("image").and_then(Value::as_str).and_then(picture);
    let rating = value
        .pointer("/rating/score")
        .and_then(number_in)
        .filter(|stars| !stars.is_zero() && (Decimal::ZERO..=Decimal::from(5)).contains(stars));
    let seller = value.get("seller");
    let shop = shop(
        seller.and_then(|seller| seller.get("id")?.as_str()),
        seller.and_then(|seller| seller.get("description")?.as_str()),
    );
    Some(FoundOffer {
        id,
        title: title.to_owned(),
        link,
        image,
        shop,
        price,
        highest_price: price,
        sales: 0,
        commission: None,
        rating,
    })
}

/// Whether the state's pagination says more items follow this page.
fn has_more(pagination: &Value) -> bool {
    let number = |key: &str| pagination.get(key).and_then(number_in)?.to_u64();
    match (number("page"), number("size"), number("records")) {
        (Some(page), Some(size), Some(records)) => page.saturating_mul(size) < records,
        _ => false,
    }
}

/// An item as a JSON-LD Product with its Offer: the id comes from its
/// page's address.
fn ld_offer(product: &Value) -> Option<FoundOffer> {
    let offer = match product.get("offers") {
        Some(Value::Array(offers)) => offers.first(),
        offer => offer,
    };
    let link = product
        .get("url")
        .or_else(|| offer?.get("url"))
        .and_then(Value::as_str)
        .and_then(item_link)?;
    let id = link
        .split("/p/")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .and_then(product_id)?;
    let price = offer.and_then(|offer| {
        money(
            offer.get("price").or_else(|| offer.get("lowPrice"))?,
            offer.get("priceCurrency"),
        )
    });
    let image = match product.get("image") {
        Some(Value::Array(images)) => images.first(),
        image => image,
    }
    .and_then(|image| image.as_str().or_else(|| image.get("url")?.as_str()))
    .and_then(picture);
    let rating = product
        .pointer("/aggregateRating/ratingValue")
        .and_then(number_in)
        .filter(|stars| (Decimal::ZERO..=Decimal::from(5)).contains(stars));
    let seller = offer
        .and_then(|offer| offer.pointer("/seller/name"))
        .and_then(Value::as_str);
    Some(FoundOffer {
        id,
        title: product.get("name")?.as_str()?.trim().to_owned(),
        link,
        image,
        shop: shop(None, seller),
        price,
        highest_price: price,
        sales: 0,
        commission: None,
        rating,
    })
}

/// A Magalu product id, such as `237361800` or `hk2b2c6c55`: letters and
/// digits only.
fn product_id(text: &str) -> Option<String> {
    let id = text.trim();
    (!id.is_empty() && id.len() <= 32 && id.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| id.to_owned())
}

/// An item's page on Magalu over HTTPS, from its path or full address,
/// without what follows `?` or `#`.
fn item_link(path: &str) -> Option<String> {
    let path = path.trim().split(['?', '#']).next()?;
    let url = if path.contains("://") {
        path.to_owned()
    } else {
        format!("{ITEM_PAGES}/{}", path.trim_start_matches('/'))
    };
    page_on(&url, DOMAIN)
}

/// A picture on Magalu's servers, its size filled in where the address
/// leaves it as `{w}x{h}`.
fn picture(address: &str) -> Option<String> {
    page_on(&address.replace("{w}x{h}", PICTURE_SIZE), PICTURES)
}

/// The seller as a shop, by its id on Magalu when the page shows it, else
/// by its name; Magalu itself when the page shows neither.
fn shop(id: Option<&str>, name: Option<&str>) -> FoundShop {
    let (id, name) = match (present(id), present(name)) {
        (Some(id), name) => (id.to_owned(), name.unwrap_or(id).to_owned()),
        (None, Some(name)) => (
            folded_words(name).join("-").to_ascii_lowercase(),
            name.to_owned(),
        ),
        (None, None) => (UNNAMED_SELLER.0.to_owned(), UNNAMED_SELLER.1.to_owned()),
    };
    FoundShop {
        id,
        name,
        link: ITEM_PAGES.to_owned(),
        commission: None,
        rating: None,
    }
}

fn present(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|text| !text.is_empty())
}
