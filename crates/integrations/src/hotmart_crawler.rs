//! The Hotmart marketplace crawler (#154, ADR 0035): reads the best sellers
//! and the category pages of hotmart.com's public marketplace, as a source
//! of Supplier Offers, through the shared [`Crawler`]. It reads only the
//! page's embedded state (`__NEXT_DATA__`). Built only while its flag is on.

use std::sync::Arc;

use mascate_catalog::{
    Found, FoundOffer, FoundShop, OfferOrder, OfferSearch, OfferSource, ProductSource,
};
use mascate_kernel::{PlatformError, Timestamp, folded_words};
use mascate_platform::{Flag, FlagKind, Phase};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::answers::page_on;
use crate::crawler::{Crawler, money, no_shops, number_in, script_json, walk};

/// The marketplace in Portuguese, unless a test points elsewhere.
pub const MARKETPLACE_URL: &str = "https://hotmart.com/pt-br/marketplace";

/// The crawler, off until the owner turns it on (ADR 0006).
pub const HOTMART_CRAWLER: Flag = Flag {
    key: "integrations.hotmart_crawler",
    name: "Crawler da vitrine da Hotmart",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "Os termos de uso da Hotmart, item 3.5(m), proíbem sistemas automatizados, robôs e \
             scripts. O crawler lê devagar, só as páginas que o robots.txt deixa, e para quando \
             o site bloqueia.",
    risk: "A Hotmart pode bloquear o endereço de internet da sua casa ou suspender a sua conta \
           de afiliado, com as comissões a receber.",
};

/// A category of the marketplace and its subcategories, by the slugs of
/// their pages and the names the marketplace shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketplaceCategory {
    pub slug: &'static str,
    pub name: &'static str,
    pub subcategories: &'static [(&'static str, &'static str)],
}

/// The marketplace's categories, as its sitemap listed them on 2026-10-16.
/// A category missing here still works as a link or a slug.
pub const HOTMART_CATEGORIES: &[MarketplaceCategory] = &[
    category("animais-e-pets", "Animais e pets", &[("pet", "Pet")]),
    category(
        "autoconhecimento-e-espiritualidade",
        "Autoconhecimento e espiritualidade",
        &[
            ("astrologia", "Astrologia"),
            ("espiritualidade", "Espiritualidade"),
            ("meditacao", "Meditação"),
            ("teologia", "Teologia"),
        ],
    ),
    category(
        "carreira-e-desenvolvimento-pessoal",
        "Carreira e desenvolvimento pessoal",
        &[
            ("carreira", "Carreira"),
            ("coaching", "Coaching"),
            ("desenvolvimento-pessoal", "Desenvolvimento pessoal"),
            ("oratoria", "Oratória"),
        ],
    ),
    category(
        "culinaria-e-gastronomia",
        "Culinária e gastronomia",
        &[
            ("confeitaria", "Confeitaria"),
            ("drinks", "Drinks"),
            ("gastronomia", "Gastronomia"),
            ("receitas", "Receitas"),
        ],
    ),
    category(
        "design-e-fotografia",
        "Design e fotografia",
        &[("design", "Design"), ("fotografia", "Fotografia")],
    ),
    category(
        "educacao-infantil-e-familia",
        "Educação infantil e família",
        &[("maternidade", "Maternidade")],
    ),
    category(
        "engenharia-e-arquitetura",
        "Engenharia e arquitetura",
        &[
            ("arquitetura", "Arquitetura"),
            ("seguranca-do-trabalho", "Segurança do trabalho"),
        ],
    ),
    category(
        "ensino-e-estudo-academico",
        "Ensino e estudo acadêmico",
        &[
            ("concurso", "Concurso"),
            ("direito", "Direito"),
            ("educacao", "Educação"),
            ("enem", "Enem"),
            ("idiomas", "Idiomas"),
            ("libras", "Libras"),
            ("literatura", "Literatura"),
            ("politica", "Política"),
            ("psicologia", "Psicologia"),
        ],
    ),
    category(
        "financas-e-negocios",
        "Finanças e negócios",
        &[
            ("contabilidade", "Contabilidade"),
            ("empreendedorismo", "Empreendedorismo"),
            ("financas", "Finanças"),
            ("investimentos", "Investimentos"),
        ],
    ),
    category(
        "manutencao-de-equipamentos",
        "Manutenção de equipamentos",
        &[("manutencao", "Manutenção")],
    ),
    category(
        "marketing-e-vendas",
        "Marketing e vendas",
        &[("marketing-digital", "Marketing digital")],
    ),
    category(
        "moda-e-beleza",
        "Moda e beleza",
        &[
            ("cabelo", "Cabelo"),
            ("estetica", "Estética"),
            ("manicure", "Manicure"),
            ("maquiagem", "Maquiagem"),
            ("moda", "Moda"),
        ],
    ),
    category(
        "musica-e-artes",
        "Música e artes",
        &[
            ("artesanato", "Artesanato"),
            ("desenho", "Desenho"),
            ("musica", "Música"),
        ],
    ),
    category(
        "plantas-e-ecologia",
        "Plantas e ecologia",
        &[("agricultura", "Agricultura"), ("plantas", "Plantas")],
    ),
    category(
        "relacionamentos",
        "Relacionamentos",
        &[("relacionamento", "Relacionamento")],
    ),
    category(
        "saude-e-esportes",
        "Saúde e esportes",
        &[
            ("emagrecimento", "Emagrecimento"),
            ("esporte", "Esporte"),
            ("fisioterapia", "Fisioterapia"),
            ("saude", "Saúde"),
        ],
    ),
    category(
        "tecnologia-e-desenvolvimento-de-software",
        "Tecnologia e desenvolvimento de software",
        &[
            ("excel", "Excel"),
            ("programacao", "Programação"),
            ("site", "Site"),
        ],
    ),
    category(
        "tempo-livre-e-lazer",
        "Tempo livre e lazer",
        &[("turismo", "Turismo")],
    ),
];

const fn category(
    slug: &'static str,
    name: &'static str,
    subcategories: &'static [(&'static str, &'static str)],
) -> MarketplaceCategory {
    MarketplaceCategory {
        slug,
        name,
        subcategories,
    }
}

/// The site, as the crawler's refusals name it.
const SITE: &str = "da Hotmart";

/// Hotmart's own domain: links and pictures read from a page must be here.
const DOMAIN: &str = "hotmart.com";

/// Where the original pictures of the products live.
const PICTURES: &str = "hotmart.s3.amazonaws.com";

/// Where the items' and the producers' pages live, whatever address the
/// marketplace was read from.
const PAGES: &str = "https://hotmart.com/pt-br/marketplace";

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

    /// The address of `search`'s page: the marketplace's front page for its
    /// best sellers, else the category's page. Hotmart's robots.txt
    /// disallows every address with `q=`, its search by words, so a search
    /// with words is refused before any request.
    fn page_url(&self, search: &OfferSearch) -> Result<String, PlatformError> {
        if !folded_words(&search.keyword).is_empty() {
            return Err(PlatformError::Refused(
                "o robots.txt da Hotmart proíbe robôs na busca por palavras; escolha uma \
                 categoria"
                    .into(),
            ));
        }
        if search.shop.is_some() {
            return Err(no_shops(SITE));
        }
        if search.order != OfferOrder::Relevance {
            return Err(PlatformError::Refused(
                "a vitrine da Hotmart vem numa ordem só".into(),
            ));
        }
        match search
            .category
            .as_deref()
            .map(str::trim)
            .filter(|category| !category.is_empty())
        {
            None => Ok(self.marketplace.clone()),
            Some(category) => Ok(format!(
                "{}/categorias/{}",
                self.marketplace,
                category_path(category)?
            )),
        }
    }
}

impl OfferSource for HotmartCrawler {
    fn source(&self) -> ProductSource {
        ProductSource::HotmartCrawler
    }

    /// The page's state holds only the first items of a category; the
    /// next ones come from a search the robots.txt keeps the crawler out
    /// of, so a later page finds nothing.
    fn search_offers(&self, search: &OfferSearch) -> Result<Found<FoundOffer>, PlatformError> {
        let url = self.page_url(search)?;
        if search.page > 1 {
            return Ok(Found {
                items: Vec::new(),
                more: false,
            });
        }
        Ok(marketplace_page(&self.crawler.page(&url)?))
    }

    fn search_shops(&self, _: &str, _: u32) -> Result<Found<FoundShop>, PlatformError> {
        Err(no_shops(SITE))
    }

    fn orders(&self) -> &[OfferOrder] {
        &[OfferOrder::Relevance]
    }

    fn finds_shops(&self) -> bool {
        false
    }

    fn blocked_until(&self) -> Option<Timestamp> {
        self.crawler.blocked_until(&self.marketplace)
    }
}

/// A category as the path after `/categorias/`: its slug, or the slugs of
/// a category and its subcategory, given as such or as the link to its
/// page on hotmart.com.
fn category_path(category: &str) -> Result<String, PlatformError> {
    let path = if category.contains("://") {
        page_on(category, DOMAIN)
            .as_deref()
            .and_then(|link| link.split(['?', '#']).next())
            .and_then(|link| link.split_once("/marketplace/categorias/"))
            .map(|(_, path)| path.to_owned())
    } else {
        Some(category.to_owned())
    };
    let slugs: Vec<&str> = path
        .as_deref()
        .unwrap_or_default()
        .trim_matches('/')
        .split('/')
        .collect();
    if (1..=2).contains(&slugs.len()) && slugs.iter().all(|slug| is_slug(slug)) {
        Ok(slugs.join("/"))
    } else {
        Err(PlatformError::Refused(
            "isso não é uma categoria da vitrine da Hotmart".into(),
        ))
    }
}

/// The products on a marketplace page: its best sellers or a category's
/// results, in the page's order, each once. The rest of the state, such as
/// the campaign banners, is left out.
fn marketplace_page(html: &str) -> Found<FoundOffer> {
    let mut items = Vec::new();
    if let Some(state) = script_json(html, "__NEXT_DATA__") {
        walk(&state, &mut |value| {
            for key in ["bestSellers", "results"] {
                if let Some(products) = value.get(key).and_then(Value::as_array) {
                    items.extend(products.iter().filter_map(state_offer));
                }
            }
        });
    }
    let mut seen = std::collections::HashSet::new();
    items.retain(|item: &FoundOffer| seen.insert(item.id.clone()));
    Found { items, more: false }
}

/// A product as the page's state describes it: its id, title, page,
/// picture, stars and producer. The marketplace's state carries no price
/// (the page asks the checkout for it afterwards), so an item is found
/// without one unless the state says; a product not for sale is left out.
fn state_offer(value: &Value) -> Option<FoundOffer> {
    let id = number_in(value.get("productId")?)?;
    if id.is_sign_negative() || !id.fract().is_zero() {
        return None;
    }
    if value.get("salesEnabled").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let title = value.get("title")?.as_str()?.trim();
    let slug = value.get("slug")?.as_str()?.trim();
    let code = value.get("producerReferenceCode")?.as_str()?.trim();
    if title.is_empty() || !is_slug(slug) || !is_code(code) {
        return None;
    }
    let price = value.get("price").and_then(|amount| money(amount, None));
    let image = ["avatar", "finalAvatar"]
        .iter()
        .filter_map(|key| value.get(*key)?.as_str())
        .find_map(picture);
    let shows_rating = value.get("showRating").and_then(Value::as_bool) != Some(false);
    let rating = value
        .get("rating")
        .filter(|_| shows_rating)
        .and_then(number_in)
        .filter(|stars| !stars.is_zero() && (Decimal::ZERO..=Decimal::from(5)).contains(stars))
        .map(|stars| stars.round_dp(2));
    Some(FoundOffer {
        id: id.to_string(),
        title: title.to_owned(),
        link: format!("{PAGES}/produtos/{slug}/{code}"),
        image,
        shop: producer(value)?,
        price,
        highest_price: price,
        sales: 0,
        commission: None,
        rating,
    })
}

/// The product's producer as a shop: by its public id on Hotmart, with
/// the link to its page when the state gives one.
fn producer(product: &Value) -> Option<FoundShop> {
    let owner = product.get("owner");
    let name = text(owner.and_then(|owner| owner.get("name")))
        .or_else(|| text(product.get("ownerName")))
        .or_else(|| text(product.get("authorName")))?;
    let public_id = text(owner.and_then(|owner| owner.get("publicId"))).filter(|id| is_code(id));
    let id = public_id
        .map(str::to_owned)
        .or_else(|| {
            owner
                .and_then(|owner| number_in(owner.get("id")?))
                .map(|id| id.to_string())
        })
        .unwrap_or_else(|| folded_words(name).join("-").to_ascii_lowercase());
    let slug = text(owner.and_then(|owner| owner.get("slug"))).filter(|slug| is_slug(slug));
    let link = text(owner.and_then(|owner| owner.get("creatorPage")))
        .and_then(|page| page_on(page, DOMAIN))
        .or_else(|| Some(format!("{PAGES}/especialista/{}/{}", slug?, public_id?)))
        .unwrap_or_else(|| PAGES.to_owned());
    Some(FoundShop {
        id,
        name: name.to_owned(),
        link,
        commission: None,
        rating: None,
    })
}

fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// A picture on Hotmart's servers, over HTTPS.
fn picture(address: &str) -> Option<String> {
    page_on(address, DOMAIN).or_else(|| page_on(address, PICTURES))
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
