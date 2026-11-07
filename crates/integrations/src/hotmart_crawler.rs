//! The Hotmart vitrine reader (#154, #220, ADR 0039 decision 2): reads the
//! best sellers and the category pages of hotmart.com's public marketplace,
//! and a product's page when the owner opens it, as Prospects, through the
//! shared [`Crawler`]. It reads only the page's embedded state
//! (`__NEXT_DATA__`). Built only while its flag is on.

use std::sync::Arc;

use chrono::{DateTime, NaiveDate};
use mascate_affiliate::{
    AffiliatePlatform, FoundProspect, ProspectCategory, ProspectSource, SignalOrigin, Signals,
};
use mascate_kernel::{PlatformError, Timestamp};
use mascate_platform::{Flag, FlagKind, Phase};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::Value;

use crate::answers::page_on;
use crate::crawler::{Crawler, number_in, script_json, walk};
use crate::hotmart_affiliate_market::affiliation_page;

/// The marketplace in Portuguese, unless a test points elsewhere.
pub const MARKETPLACE_URL: &str = "https://hotmart.com/pt-br/marketplace";

/// The reader, off until the owner turns it on (ADR 0006).
pub const HOTMART_CRAWLER: Flag = Flag {
    key: "integrations.hotmart_crawler",
    name: "Crawler da vitrine da Hotmart",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "Os termos de uso da Hotmart, item 3.5(m), proíbem sistemas automatizados, robôs e \
             scripts. O crawler lê devagar, só quando você clica, só as páginas que o robots.txt \
             deixa, e para quando o site bloqueia.",
    risk: "A Hotmart pode bloquear o endereço de internet da sua casa ou suspender a sua conta \
           de afiliado, com as comissões a receber.",
};

/// The top categories the marketplace shows, as its sitemap listed them on
/// 2026-10-16.
const CATEGORIES: &[(&str, &str)] = &[
    ("animais-e-pets", "Animais e pets"),
    (
        "autoconhecimento-e-espiritualidade",
        "Autoconhecimento e espiritualidade",
    ),
    (
        "carreira-e-desenvolvimento-pessoal",
        "Carreira e desenvolvimento pessoal",
    ),
    ("culinaria-e-gastronomia", "Culinária e gastronomia"),
    ("design-e-fotografia", "Design e fotografia"),
    ("educacao-infantil-e-familia", "Educação infantil e família"),
    ("engenharia-e-arquitetura", "Engenharia e arquitetura"),
    ("ensino-e-estudo-academico", "Ensino e estudo acadêmico"),
    ("financas-e-negocios", "Finanças e negócios"),
    ("manutencao-de-equipamentos", "Manutenção de equipamentos"),
    ("marketing-e-vendas", "Marketing e vendas"),
    ("moda-e-beleza", "Moda e beleza"),
    ("musica-e-artes", "Música e artes"),
    ("plantas-e-ecologia", "Plantas e ecologia"),
    ("relacionamentos", "Relacionamentos"),
    ("saude-e-esportes", "Saúde e esportes"),
    (
        "tecnologia-e-desenvolvimento-de-software",
        "Tecnologia e desenvolvimento de software",
    ),
    ("tempo-livre-e-lazer", "Tempo livre e lazer"),
];

/// Hotmart's top categories, which the vitrine and the market share.
pub(crate) fn hotmart_categories() -> Vec<ProspectCategory> {
    CATEGORIES
        .iter()
        .map(|(slug, name)| ProspectCategory {
            id: (*slug).to_owned(),
            name: (*name).to_owned(),
        })
        .collect()
}

/// Hotmart's own domain: a product page read or kept must be here.
const DOMAIN: &str = "hotmart.com";

/// Where the products' pages live, whatever address the marketplace was
/// read from.
const PAGES: &str = "https://hotmart.com/pt-br/marketplace";

/// The part of a product page's address before its slug and code.
const PRODUCT_PATH: &str = "/marketplace/produtos/";

/// The badge of a producer whose profile Hotmart verified.
const VERIFIED: &str = "VERIFIED_PROFILE";

/// The Hotmart marketplace's public pages, read by the crawler.
pub struct HotmartCrawler {
    marketplace: String,
    crawler: Arc<Crawler>,
}

impl HotmartCrawler {
    /// The marketplace at `marketplace`, read by `crawler`.
    pub fn new(marketplace: &str, crawler: Arc<Crawler>) -> Self {
        Self {
            marketplace: marketplace.trim_end_matches('/').to_owned(),
            crawler,
        }
    }

    /// The address of `category`'s page, or the front page with the best
    /// sellers. Only the listed categories are asked for, so nothing the
    /// app did not write goes into an address.
    fn listing_url(&self, category: Option<&ProspectCategory>) -> Result<String, PlatformError> {
        match category {
            None => Ok(self.marketplace.clone()),
            Some(category) => CATEGORIES
                .iter()
                .find(|(slug, _)| *slug == category.id)
                .map(|(slug, _)| format!("{}/categorias/{slug}", self.marketplace))
                .ok_or_else(|| {
                    PlatformError::Refused("isso não é uma categoria da vitrine da Hotmart".into())
                }),
        }
    }
}

impl ProspectSource for HotmartCrawler {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Hotmart
    }

    fn origin(&self) -> SignalOrigin {
        SignalOrigin::Vitrine
    }

    fn categories(&self) -> Result<Vec<ProspectCategory>, PlatformError> {
        Ok(hotmart_categories())
    }

    /// The page's state holds only the first products of a category; the
    /// next ones come from a search the robots.txt keeps the crawler out
    /// of, so one page is all there is.
    fn prospects(
        &self,
        category: Option<&ProspectCategory>,
    ) -> Result<Vec<FoundProspect>, PlatformError> {
        let url = self.listing_url(category)?;
        Ok(listing(&self.crawler.page(&url)?))
    }

    /// Reads the product page `known` keeps the address of: the students,
    /// the warranty and the producer, which the listing does not show.
    fn details(&self, known: &FoundProspect) -> Result<FoundProspect, PlatformError> {
        let (slug, code) = known
            .signals
            .product_page
            .as_deref()
            .and_then(product_page)
            .ok_or_else(|| {
                PlatformError::Refused(
                    "esse produto não tem o endereço de uma página da vitrine da Hotmart".into(),
                )
            })?;
        let html = self
            .crawler
            .page(&format!("{}/produtos/{slug}/{code}", self.marketplace))?;
        Ok(product(&html, &slug, &code, known))
    }

    fn blocked_until(&self) -> Option<Timestamp> {
        self.crawler.blocked_until(&self.marketplace)
    }
}

/// The slug and code of a product page on hotmart.com, from its address.
fn product_page(link: &str) -> Option<(String, String)> {
    let link = page_on(link, DOMAIN)?;
    let path = link.split(['?', '#']).next()?;
    let (_, rest) = path.split_once(PRODUCT_PATH)?;
    let (slug, code) = rest.trim_end_matches('/').split_once('/')?;
    (is_slug(slug) && is_code(code)).then(|| (slug.to_owned(), code.to_owned()))
}

/// The products on a marketplace page: its best sellers or a category's
/// results, in the page's order, each once. The rest of the state, such as
/// the campaign banners, is left out.
fn listing(html: &str) -> Vec<FoundProspect> {
    let mut found = Vec::new();
    if let Some(state) = script_json(html, "__NEXT_DATA__") {
        walk(&state, &mut |value| {
            for key in ["bestSellers", "results"] {
                if let Some(products) = value.get(key).and_then(Value::as_array) {
                    found.extend(products.iter().filter_map(listed));
                }
            }
        });
    }
    let mut seen = std::collections::HashSet::new();
    found.retain(|prospect: &FoundProspect| seen.insert(prospect.key.clone()));
    found
}

/// A product as the state lists it, keyed by its id on Hotmart. A field
/// missing or odd leaves its signal empty; a product not for sale, or
/// without an id, a title or a page, is left out. The listing has no price
/// (the checkout gives it later), no commission and no temperature.
fn listed(value: &Value) -> Option<FoundProspect> {
    let key = whole(value.get("productId")?)?;
    if value.get("salesEnabled").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let name = text(value.get("title"))?;
    let slug = text(value.get("slug")).filter(|slug| is_slug(slug))?;
    let code = text(value.get("producerReferenceCode")).filter(|code| is_code(code))?;
    let shows_rating = value.get("showRating").and_then(Value::as_bool) != Some(false);
    Some(FoundProspect {
        platform: AffiliatePlatform::Hotmart,
        key: key.to_string(),
        name: name.to_owned(),
        signals: Signals {
            rating: value.get("rating").filter(|_| shows_rating).and_then(stars),
            reviews: value.get("totalReviews").and_then(count),
            launched: value.get("ingressDate").and_then(day),
            classes: value.get("totalClasses").and_then(count),
            hours: value
                .get("totalHours")
                .and_then(number_in)
                .filter(|hours| !hours.is_sign_negative())
                .map(|hours| hours.round_dp(1)),
            certificate: value.get("certification").and_then(Value::as_bool),
            video: value
                .get("videoLink")
                .map(|link| text(Some(link)).is_some()),
            creator: creator(value).map(str::to_owned),
            product_page: Some(format!("{PAGES}/produtos/{slug}/{code}")),
            ..Signals::default()
        },
    })
}

/// A product page: what the listing shows of the product, when the state
/// carries it again, and what only the page shows, with the product's
/// address in the affiliate market, from its code there (`ucode`). Under
/// `known`'s key unless the page gives Hotmart's id, as for a link the
/// owner pasted.
fn product(html: &str, slug: &str, code: &str, known: &FoundProspect) -> FoundProspect {
    let mut found = FoundProspect {
        signals: Signals {
            product_page: Some(format!("{PAGES}/produtos/{slug}/{code}")),
            ..Signals::default()
        },
        ..known.clone()
    };
    let Some(state) = script_json(html, "__NEXT_DATA__") else {
        return found;
    };
    let mut listed_here = None;
    let mut ucode = None;
    let first = |key: &str| {
        let mut hit = None;
        walk(&state, &mut |value| {
            if hit.is_none() {
                hit = value.get(key).cloned();
            }
        });
        hit
    };
    walk(&state, &mut |value| {
        if listed_here.is_none()
            && text(value.get("producerReferenceCode")) == Some(code)
            && text(value.get("slug")) == Some(slug)
        {
            listed_here = listed(value);
        }
        if ucode.is_none() && text(value.get("producerReferenceCode")) == Some(code) {
            ucode = text(value.get("ucode")).map(str::to_owned);
        }
    });
    let students = first("totalUsers").as_ref().and_then(count);
    let warranty = first("dataSheet")
        .as_ref()
        .and_then(|sheet| sheet.get("warranty").and_then(days));
    let hotmarter = first("hotmarter");
    let review = first("customerReview");
    if let Some(listed) = listed_here {
        found.key = listed.key;
        found.name = listed.name;
        found.signals = listed.signals;
    }
    let signals = &mut found.signals;
    signals.affiliation_page = ucode.as_deref().and_then(affiliation_page);
    signals.students = students;
    signals.warranty_days = warranty;
    if let Some(hotmarter) = &hotmarter {
        signals.creator_since = hotmarter.get("userSince").and_then(day);
        signals.creator_verified = hotmarter
            .get("activeBadges")
            .and_then(Value::as_array)
            .map(|badges| badges.iter().any(is_verified));
        if signals.creator.is_none() {
            signals.creator = text(hotmarter.get("name")).map(str::to_owned);
        }
    }
    if let Some(review) = &review {
        if let Some(rating) = review.get("rating").and_then(stars) {
            signals.rating = Some(rating);
        }
        if let Some(reviews) = review.get("totalReviews").and_then(count) {
            signals.reviews = Some(reviews);
        }
    }
    found
}

fn is_verified(badge: &Value) -> bool {
    badge.as_str() == Some(VERIFIED)
        || ["type", "name", "key"]
            .iter()
            .any(|field| badge.get(*field).and_then(Value::as_str) == Some(VERIFIED))
}

/// The producer's name, as the listing gives it.
fn creator(product: &Value) -> Option<&str> {
    text(product.get("owner").and_then(|owner| owner.get("name")))
        .or_else(|| text(product.get("ownerName")))
        .or_else(|| text(product.get("authorName")))
}

fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// A whole number from 0 up, as a number or as text.
fn whole(value: &Value) -> Option<u64> {
    number_in(value)
        .filter(|number| !number.is_sign_negative() && number.fract().is_zero())
        .and_then(|number| number.to_u64())
}

fn count(value: &Value) -> Option<u32> {
    whole(value).and_then(|count| u32::try_from(count).ok())
}

/// Stars from 0 to 5, two decimals; zero is no rating yet.
fn stars(value: &Value) -> Option<Decimal> {
    number_in(value)
        .filter(|stars| !stars.is_zero() && (Decimal::ZERO..=Decimal::from(5)).contains(stars))
        .map(|stars| stars.round_dp(2))
}

/// Days, as a number or as text such as "7 dias".
fn days(value: &Value) -> Option<u32> {
    count(value).or_else(|| {
        let digits: String = value
            .as_str()?
            .trim()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    })
}

/// A day as Hotmart's state writes one: milliseconds since 1970, an ISO
/// date or date and time, or a year alone as its first day.
fn day(value: &Value) -> Option<NaiveDate> {
    if let Some(number) = whole(value) {
        return match number {
            2000..=2100 => NaiveDate::from_ymd_opt(i32::try_from(number).ok()?, 1, 1),
            _ => DateTime::from_timestamp_millis(i64::try_from(number).ok()?)
                .map(|time| time.date_naive()),
        };
    }
    let text = value.as_str()?.trim();
    NaiveDate::parse_from_str(text.get(..10)?, "%Y-%m-%d").ok()
}

/// A slug of Hotmart's pages: lower-case letters, digits and dashes.
fn is_slug(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 200
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A code of Hotmart's, such as `Q52311595P`: letters and digits only.
fn is_code(text: &str) -> bool {
    !text.is_empty() && text.len() <= 32 && text.chars().all(|c| c.is_ascii_alphanumeric())
}
