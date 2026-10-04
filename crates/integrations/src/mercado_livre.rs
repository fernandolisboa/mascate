//! Mercado Livre's adapter: the seller API answers with the Connection's
//! login, reports demand to the catalog and the owner's listings to
//! commerce (ADR 0013). Only official endpoints, read-only here.

mod answers;
mod sales_channel;
mod tokens;

use std::sync::Arc;
use std::time::Duration;

use mascate_catalog::{BestSeller, CatalogMatch, Competition, DemandCategory, DemandSource};
use mascate_kernel::{Clock, Currency, ListingType, Money, PlatformError};
use mascate_platform::{SecretStore, http_agent};
use serde::de::DeserializeOwned;

use answers::{
    CategoryAnswer, HighlightKind, Highlights, ItemAnswer, ListingPrices, MultigetEntry,
    ProductAnswer, ProductItems, ProductSearch, UserProductAnswer, money,
};
use tokens::Tokens;

/// The API every request goes to, unless a test points elsewhere.
pub const API_URL: &str = "https://api.mercadolibre.com";

/// Mercado Livre Brasil, the only site the app sells on.
const SITE: &str = "MLB";

/// Answers are small JSON documents; anything bigger is not one.
const MAX_ANSWER_BYTES: u64 = 2 * 1024 * 1024;

/// Listings read per catalog product: the first page is enough for a price
/// range, and `paging.total` still counts them all.
const COMPETITORS_PAGE: u32 = 50;

/// Catalog products read per search.
const SEARCH_LIMIT: u32 = 5;

/// Mercado Livre's seller API, as the owner's Connection sees it.
pub struct MercadoLivre {
    agent: ureq::Agent,
    api: String,
    tokens: Tokens,
}

impl MercadoLivre {
    /// The API at `api`, with the login kept in `store`.
    pub fn new(
        api: &str,
        user_agent: &str,
        store: Arc<dyn SecretStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            agent: http_agent(api, user_agent, Duration::from_secs(30)),
            api: api.trim_end_matches('/').to_owned(),
            tokens: Tokens::new(store, clock),
        }
    }

    /// The JSON answer to `GET path` with `query`, signed with the
    /// Connection's access token. A token Mercado Livre turns down is renewed
    /// once before the login counts as expired.
    fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, PlatformError> {
        let mut renewed = false;
        loop {
            let token = self.tokens.access_token(&self.agent, &self.api)?;
            let mut request = self
                .agent
                .get(format!("{}{path}", self.api))
                .header("Accept", "application/json")
                .header("Authorization", format!("Bearer {}", token.expose()));
            for (key, value) in query {
                request = request.query(key, value);
            }
            let response = request
                .call()
                .map_err(|error| PlatformError::Failed(error.to_string()))?;
            match response.status().as_u16() {
                200 => return read_json(response),
                401 if !renewed => {
                    self.tokens.forget(&token);
                    renewed = true;
                }
                401 => return Err(PlatformError::Expired),
                403 => return Err(PlatformError::Refused(refusal(response))),
                404 => return Err(PlatformError::NotFound),
                429 => return Err(PlatformError::RateLimited),
                status => {
                    return Err(PlatformError::Failed(format!(
                        "Mercado Livre answered {status} for {path}"
                    )));
                }
            }
        }
    }

    fn product(&self, id: &str) -> Result<BestSeller, PlatformError> {
        let product: ProductAnswer = self.get(&format!("/products/{id}"), &[])?;
        Ok(BestSeller {
            id: product.id.clone(),
            position: 0,
            title: product.name,
            catalog_product: Some(product.id),
            price: product
                .buy_box_winner
                .and_then(|winner| money(&winner.price, &winner.currency_id)),
        })
    }

    fn user_product(&self, id: &str) -> Result<BestSeller, PlatformError> {
        let product: UserProductAnswer = self.get(&format!("/user-products/{id}"), &[])?;
        Ok(BestSeller {
            id: product.id,
            position: 0,
            title: product.name,
            catalog_product: product.catalog_product_id,
            price: None,
        })
    }

    /// Single listings, read in one request.
    fn items(&self, ids: &[&str]) -> Result<Vec<BestSeller>, PlatformError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let answers: Vec<MultigetEntry> = self.get(
            "/items",
            &[
                ("ids", &ids.join(",")),
                (
                    "attributes",
                    "id,title,price,currency_id,catalog_product_id",
                ),
            ],
        )?;
        Ok(answers
            .into_iter()
            .filter(|answer| answer.code == 200)
            .filter_map(|answer| answer.body::<ItemAnswer>().ok())
            .map(|item| BestSeller {
                price: money(&item.price, &item.currency_id),
                id: item.id,
                position: 0,
                title: item.title,
                catalog_product: item.catalog_product_id,
            })
            .collect())
    }
}

impl DemandSource for MercadoLivre {
    fn currency(&self) -> Currency {
        Currency::Brl
    }

    fn categories(&self) -> Result<Vec<DemandCategory>, PlatformError> {
        let categories: Vec<CategoryAnswer> =
            self.get(&format!("/sites/{SITE}/categories"), &[])?;
        Ok(categories
            .into_iter()
            .map(|category| DemandCategory {
                id: category.id,
                name: category.name,
            })
            .collect())
    }

    /// Resolves each of the top 20 into a title, its catalog product and a
    /// price. An entry Mercado Livre no longer shows is left out.
    fn best_sellers(&self, category: &str) -> Result<Vec<BestSeller>, PlatformError> {
        let highlights: Highlights =
            match self.get(&format!("/highlights/{SITE}/category/{category}"), &[]) {
                // A category too small for a ranking has none.
                Err(PlatformError::NotFound) => return Ok(Vec::new()),
                other => other?,
            };
        let item_ids: Vec<&str> = highlights
            .content
            .iter()
            .filter(|entry| entry.kind == HighlightKind::Item)
            .map(|entry| entry.id.as_str())
            .collect();
        let items = match self.items(&item_ids) {
            Err(PlatformError::NotFound | PlatformError::Refused(_)) => Vec::new(),
            other => other?,
        };
        let mut best_sellers = Vec::new();
        for entry in &highlights.content {
            let resolved = match entry.kind {
                HighlightKind::Product => self.product(&entry.id),
                HighlightKind::UserProduct => self.user_product(&entry.id),
                HighlightKind::Item => items
                    .iter()
                    .find(|item| item.id == entry.id)
                    .cloned()
                    .ok_or(PlatformError::NotFound),
                HighlightKind::Other => continue,
            };
            match resolved {
                Ok(best_seller) => best_sellers.push(BestSeller {
                    position: entry.position,
                    ..best_seller
                }),
                Err(PlatformError::NotFound | PlatformError::Refused(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(best_sellers)
    }

    fn search(&self, query: &str) -> Result<Vec<CatalogMatch>, PlatformError> {
        let found: ProductSearch = self.get(
            "/products/search",
            &[
                ("status", "active"),
                ("site_id", SITE),
                ("q", query),
                ("limit", &SEARCH_LIMIT.to_string()),
            ],
        )?;
        Ok(found
            .results
            .into_iter()
            .map(|product| CatalogMatch {
                id: product.id,
                name: product.name,
            })
            .collect())
    }

    fn competition(&self, product: &str) -> Result<Competition, PlatformError> {
        let items: ProductItems = self.get(
            &format!("/products/{product}/items"),
            &[("limit", &COMPETITORS_PAGE.to_string())],
        )?;
        let category = items
            .results
            .iter()
            .find_map(|item| item.category_id.clone());
        let prices = items
            .results
            .iter()
            .filter_map(|item| money(&item.price, &item.currency_id))
            .collect::<Vec<Money>>();
        Ok(Competition {
            sellers: items.paging.total.max(items.results.len() as u32),
            prices,
            category,
        })
    }

    fn root_category(&self, category: &str) -> Result<DemandCategory, PlatformError> {
        let found: CategoryAnswer = self.get(&format!("/categories/{category}"), &[])?;
        let root = found.path_from_root.into_iter().next();
        Ok(match root {
            Some(root) => DemandCategory {
                id: root.id,
                name: root.name,
            },
            None => DemandCategory {
                id: found.id,
                name: found.name,
            },
        })
    }

    fn sale_fee(
        &self,
        category: &str,
        price: Money,
        listing: ListingType,
    ) -> Result<Money, PlatformError> {
        let prices: ListingPrices = self.get(
            &format!("/sites/{SITE}/listing_prices"),
            &[
                ("price", &price.rounded().amount().to_string()),
                ("listing_type_id", sales_channel::listing_type_id(listing)),
                ("category_id", category),
            ],
        )?;
        prices
            .for_listing(sales_channel::listing_type_id(listing))
            .and_then(|found| money(&found.sale_fee_amount, &found.currency_id))
            .ok_or(PlatformError::NotFound)
    }
}

fn read_json<T: DeserializeOwned>(
    response: ureq::http::Response<ureq::Body>,
) -> Result<T, PlatformError> {
    let bytes = response
        .into_body()
        .into_with_config()
        .limit(MAX_ANSWER_BYTES)
        .read_to_vec()
        .map_err(|error| PlatformError::Failed(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        PlatformError::Failed(format!(
            "Mercado Livre answered something unexpected: {error}"
        ))
    })
}

/// Mercado Livre's own word for a 403, when it gives one.
fn refusal(response: ureq::http::Response<ureq::Body>) -> String {
    read_json::<answers::ErrorAnswer>(response)
        .ok()
        .and_then(|answer| answer.message.or(answer.error))
        .unwrap_or_else(|| "forbidden".into())
}
