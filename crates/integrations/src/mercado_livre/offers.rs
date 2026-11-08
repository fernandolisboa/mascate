//! Mercado Livre as a place to buy (#235): Ofertas searches its catalog by
//! keyword through the official API with the seller's login, and each
//! catalog product comes with its cheapest listing and that listing's
//! seller. The public search pages send a robot to login or a check, and
//! the API answers the connected seller, so with a Connection this replaces
//! the crawler.

use std::collections::HashMap;

use mascate_catalog::{
    Found, FoundOffer, FoundShop, OfferOrder, OfferSearch, OfferSource, ProductSource,
};
use mascate_kernel::{Money, PlatformError};

use super::answers::{ProductItems, ProductSearch, PublicUser, money};
use super::{MercadoLivre, SITE};
use crate::answers::{https_link, page_on};

/// Catalog products per page of a search: each costs a request for its
/// listings, so a page stays short.
pub(super) const OFFERS_PAGE: u32 = 10;

/// Listings read per catalog product to find the cheapest one.
const LISTINGS_READ: u32 = 50;

/// Mercado Livre Brasil's site, where a catalog product's page lives.
const SITE_URL: &str = "https://www.mercadolivre.com.br";

/// The domain every link read from the API must open on.
const DOMAIN: &str = "mercadolivre.com.br";

impl OfferSource for MercadoLivre {
    fn source(&self) -> ProductSource {
        ProductSource::MercadoLivre
    }

    /// The catalog products matching the keyword, each priced by its
    /// cheapest listing. A product nobody sells right now is left out: it
    /// cannot be bought.
    fn search_offers(&self, search: &OfferSearch) -> Result<Found<FoundOffer>, PlatformError> {
        let keyword = search.keyword.trim();
        if keyword.is_empty() {
            return Err(PlatformError::Refused(
                "a busca do Mercado Livre pede uma palavra".into(),
            ));
        }
        if search.shop.is_some() || search.category.is_some() {
            return Err(PlatformError::Refused(
                "a busca do Mercado Livre pela API não filtra por loja nem por categoria".into(),
            ));
        }
        if search.order != OfferOrder::Relevance {
            return Err(PlatformError::Refused(
                "a busca do Mercado Livre pela API só ordena por relevância".into(),
            ));
        }
        let offset = (search.page.max(1) - 1) * OFFERS_PAGE;
        let found: ProductSearch = self.get(
            "/products/search",
            &[
                ("status", "active"),
                ("site_id", SITE),
                ("q", keyword),
                ("limit", &OFFERS_PAGE.to_string()),
                ("offset", &offset.to_string()),
            ],
        )?;
        let read = offset + found.results.len() as u32;
        let more = !found.results.is_empty() && read < found.paging.total;
        let mut sellers: HashMap<u64, FoundShop> = HashMap::new();
        let mut items = Vec::new();
        // The id goes into a request path and the offer's link: only the
        // Platform's own shape of id passes.
        let products = found
            .results
            .into_iter()
            .filter(|product| is_catalog_id(&product.id));
        for product in products {
            let Some((price, seller)) = self.cheapest_listing(&product.id)? else {
                continue;
            };
            let shop = match sellers.get(&seller) {
                Some(shop) => shop.clone(),
                None => {
                    let shop = self.seller(seller)?;
                    sellers.insert(seller, shop.clone());
                    shop
                }
            };
            items.push(FoundOffer {
                link: format!("{SITE_URL}/p/{}", product.id),
                image: product
                    .pictures
                    .into_iter()
                    .filter_map(|picture| picture.url.as_deref().and_then(https_link))
                    .next(),
                id: product.id,
                title: product.name,
                shop,
                price: Some(price),
                highest_price: Some(price),
                sales: 0,
                commission: None,
                rating: None,
            });
        }
        Ok(Found { items, more })
    }

    fn search_shops(&self, _: &str, _: u32) -> Result<Found<FoundShop>, PlatformError> {
        Err(PlatformError::Refused(
            "a busca do Mercado Livre pela API não busca lojas".into(),
        ))
    }

    fn orders(&self) -> &[OfferOrder] {
        &[OfferOrder::Relevance]
    }

    fn finds_shops(&self) -> bool {
        false
    }
}

impl MercadoLivre {
    /// The lowest price among the first listings of catalog product
    /// `product`, with its seller; `None` when no listing has a price.
    fn cheapest_listing(&self, product: &str) -> Result<Option<(Money, u64)>, PlatformError> {
        let listings: ProductItems = match self.get(
            &format!("/products/{product}/items"),
            &[("limit", &LISTINGS_READ.to_string())],
        ) {
            Ok(listings) => listings,
            Err(PlatformError::NotFound) => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(listings
            .results
            .iter()
            .filter_map(|item| Some((money(&item.price, &item.currency_id)?, item.seller_id?)))
            .min_by(|(a, _), (b, _)| a.amount().cmp(&b.amount())))
    }

    /// The seller `id` as a shop: its nickname and profile page, when
    /// Mercado Livre shows them.
    fn seller(&self, id: u64) -> Result<FoundShop, PlatformError> {
        let user: Option<PublicUser> = match self.get(&format!("/users/{id}"), &[]) {
            Ok(user) => Some(user),
            Err(PlatformError::NotFound) => None,
            Err(error) => return Err(error),
        };
        let (nickname, permalink) = user
            .map(|user| (user.nickname, user.permalink))
            .unwrap_or_default();
        Ok(FoundShop {
            id: id.to_string(),
            name: nickname
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| format!("Vendedor {id} do Mercado Livre")),
            link: permalink
                .and_then(|link| page_on(&link.replacen("http://", "https://", 1), DOMAIN))
                .unwrap_or_else(|| SITE_URL.to_owned()),
            commission: None,
            rating: None,
        })
    }
}

/// A catalog product id as Mercado Livre writes it: the site and digits,
/// like `MLB19615318`.
fn is_catalog_id(id: &str) -> bool {
    id.strip_prefix(SITE)
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
}
