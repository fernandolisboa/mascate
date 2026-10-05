//! The Shopee Affiliate Open API's adapter (#12): searches Shopee's catalog
//! for items and shops as a source of Supplier Offers (ADR 0027). Every
//! request is a GraphQL query signed with the Connection's AppID and
//! Secret; the API only reads, so nothing ever goes to Shopee but searches.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use mascate_catalog::{
    Found, FoundOffer, FoundShop, OfferOrder, OfferSearch, OfferSource, ProductSource,
};
use mascate_kernel::{Clock, Currency, Money, Percentage, PlatformError};
use mascate_platform::{Secret, SecretStore, http_agent};
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};

use crate::answers::read_json;
use crate::retry::{Attempt, Backoff, Pause, ThreadPause, retrying};

/// Shopee Brasil's endpoint, unless a test points elsewhere.
pub const API_URL: &str = "https://open-api.affiliate.shopee.com.br/graphql";

const APP_ID: &str = "SHOPEE_AFFILIATE_APP_ID";
const SECRET: &str = "SHOPEE_AFFILIATE_SECRET";

/// Results per page.
const PAGE_SIZE: u32 = 20;

/// Shopee publishes no rate limit, only the error for passing it, so a
/// refused request waits 2, 4 and then 8 seconds before giving up.
const BACKOFF: Backoff = Backoff {
    first: Duration::from_secs(2),
    retries: 3,
};

/// Shopee's error codes, from the Open API's error list.
const SYSTEM_ERROR: i64 = 10000;
/// A wrong signature, AppID or Secret, a disabled app, or a timestamp more
/// than a few minutes off.
const IDENTITY_ERROR: i64 = 10020;
const RATE_LIMIT_EXCEEDED: i64 = 10030;
/// Access denied, a frozen or blocked affiliate account, or no access to
/// the Open API yet.
const NO_ACCESS: std::ops::RangeInclusive<i64> = 10031..=10035;
/// The request itself: a business rule or a wrong parameter.
const BUSINESS_ERRORS: std::ops::RangeInclusive<i64> = 11000..=11999;

/// The Shopee Affiliate Open API, as the owner's Connection sees it.
pub struct ShopeeAffiliates {
    agent: ureq::Agent,
    api: String,
    store: Arc<dyn SecretStore>,
    clock: Arc<dyn Clock>,
    pause: Arc<dyn Pause>,
}

impl ShopeeAffiliates {
    /// The API at `api`, with the AppID and Secret kept in `store`.
    pub fn new(
        api: &str,
        user_agent: &str,
        store: Arc<dyn SecretStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            agent: http_agent(api, user_agent, Duration::from_secs(30)),
            api: api.to_owned(),
            store,
            clock,
            pause: Arc::new(ThreadPause),
        }
    }

    /// The same, waiting between retries with `pause`.
    pub fn with_pause(mut self, pause: Arc<dyn Pause>) -> Self {
        self.pause = pause;
        self
    }

    /// What `operation` answers to the GraphQL `query`. A rate limit or a
    /// failure on Shopee's side is tried again after a pause.
    fn query<T: DeserializeOwned>(&self, operation: &str, query: &str) -> Result<T, PlatformError> {
        let (app_id, secret) = self.credentials()?;
        let body = serde_json::json!({ "query": query }).to_string();
        retrying(self.pause.as_ref(), BACKOFF, || {
            self.send(&app_id, &secret, &body, operation)
        })
    }

    fn credentials(&self) -> Result<(Secret, Secret), PlatformError> {
        let read = |name| {
            self.store
                .read(name)
                .map_err(|error| PlatformError::Failed(error.to_string()))?
                .ok_or(PlatformError::NotConnected)
        };
        Ok((read(APP_ID)?, read(SECRET)?))
    }

    fn send<T: DeserializeOwned>(
        &self,
        app_id: &Secret,
        secret: &Secret,
        body: &str,
        operation: &str,
    ) -> Attempt<T> {
        let timestamp = self.clock.now().timestamp();
        let sent = self
            .agent
            .post(&self.api)
            .header("Content-Type", "application/json")
            .header(
                "Authorization",
                authorization(app_id.expose(), timestamp, body, secret.expose()),
            )
            .send(body);
        let response = match sent {
            Ok(response) => response,
            Err(error) => return Attempt::Again(PlatformError::Failed(error.to_string())),
        };
        match response.status().as_u16() {
            200 => {}
            429 => return Attempt::Again(PlatformError::RateLimited),
            status @ 500..=599 => {
                return Attempt::Again(PlatformError::Failed(format!("Shopee answered {status}")));
            }
            401 => return Attempt::Done(Err(PlatformError::Expired)),
            403 => return Attempt::Done(Err(PlatformError::Refused("access denied".into()))),
            status => {
                return Attempt::Done(Err(PlatformError::Failed(format!(
                    "Shopee answered {status}"
                ))));
            }
        }
        let answer: GraphQlAnswer = match read_json(response, "Shopee") {
            Ok(answer) => answer,
            Err(error) => return Attempt::Done(Err(error)),
        };
        if let Some(error) = answer.errors.into_iter().next() {
            return refused(error);
        }
        let data = answer
            .data
            .and_then(|mut data| data.get_mut(operation).map(serde_json::Value::take))
            .unwrap_or_default();
        Attempt::Done(serde_json::from_value(data).map_err(|error| {
            PlatformError::Failed(format!("Shopee answered something unexpected: {error}"))
        }))
    }
}

impl OfferSource for ShopeeAffiliates {
    fn source(&self) -> ProductSource {
        ProductSource::ShopeeAffiliate
    }

    fn search_offers(&self, search: &OfferSearch) -> Result<Found<FoundOffer>, PlatformError> {
        let mut arguments = Vec::new();
        let keyword = search.keyword.trim();
        if !keyword.is_empty() {
            arguments.push(format!("keyword: {}", string(keyword)));
        }
        if let Some(category) = search.category.as_deref().filter(|c| !c.trim().is_empty()) {
            let id = category_id(category).ok_or_else(|| {
                PlatformError::Refused(
                    "a categoria precisa ser o número ou o link de uma categoria da Shopee".into(),
                )
            })?;
            arguments.push(format!("productCatId: {id}"));
        }
        if let Some(shop) = search.shop.as_deref() {
            let shop = shop.trim();
            if !number(shop) {
                return Err(PlatformError::Refused(
                    "a loja precisa ser um número".into(),
                ));
            }
            arguments.push(format!("shopId: {shop}"));
        }
        arguments.push(format!("sortType: {}", sort_type(search.order)));
        arguments.push(format!("page: {}", search.page.max(1)));
        arguments.push(format!("limit: {PAGE_SIZE}"));
        let found: Connection<ProductNode> = self.query(
            "productOfferV2",
            &format!(
                "{{productOfferV2({}){{nodes{{itemId productName productLink priceMin priceMax \
                 sales commissionRate ratingStar shopId shopName}}pageInfo{{hasNextPage}}}}}}",
                arguments.join(", ")
            ),
        )?;
        Ok(Found {
            items: found
                .nodes
                .into_iter()
                .filter_map(ProductNode::offer)
                .collect(),
            more: found.page_info.has_next_page,
        })
    }

    fn search_shops(&self, keyword: &str, page: u32) -> Result<Found<FoundShop>, PlatformError> {
        let found: Connection<ShopNode> = self.query(
            "shopOfferV2",
            &format!(
                "{{shopOfferV2(keyword: {}, sortType: {POPULAR_SHOPS}, page: {}, \
                 limit: {PAGE_SIZE}){{nodes{{shopId shopName originalLink commissionRate \
                 ratingStar}}pageInfo{{hasNextPage}}}}}}",
                string(keyword.trim()),
                page.max(1)
            ),
        )?;
        Ok(Found {
            items: found.nodes.into_iter().filter_map(ShopNode::shop).collect(),
            more: found.page_info.has_next_page,
        })
    }
}

/// `shopOfferV2`'s sort by the most popular shops.
const POPULAR_SHOPS: u8 = 3;

/// `productOfferV2`'s `sortType` for `order`.
fn sort_type(order: OfferOrder) -> u8 {
    match order {
        OfferOrder::Relevance => 1,
        OfferOrder::Sales => 2,
        OfferOrder::LowestPrice => 4,
        OfferOrder::HighestCommission => 5,
    }
}

/// The `Authorization` header Shopee checks: SHA256 of the AppID, the Unix
/// timestamp in seconds, the exact body sent and the Secret, in hex.
fn authorization(app_id: &str, timestamp: i64, body: &str, secret: &str) -> String {
    let signature = Sha256::digest(format!("{app_id}{timestamp}{body}{secret}"));
    format!("SHA256 Credential={app_id}, Timestamp={timestamp}, Signature={signature:x}")
}

/// `text` as a GraphQL string literal; JSON's escaping is GraphQL's.
fn string(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

/// The category id in what the owner typed: the number itself, or the last
/// number after `cat.` in a category page's link, such as
/// `https://shopee.com.br/Celulares-cat.11059988`.
fn category_id(text: &str) -> Option<u64> {
    let text = text.trim();
    let id = match text.rfind("cat.") {
        Some(at) => text[at + 4..]
            .split(['/', '?', '#'])
            .next()?
            .rsplit('.')
            .next()?,
        None => text,
    };
    if !number(id) {
        return None;
    }
    id.parse().ok()
}

/// What a GraphQL error means, by Shopee's code.
fn refused<T>(error: GraphQlError) -> Attempt<T> {
    let code = error.extensions.as_ref().and_then(|e| e.code);
    let message = error
        .extensions
        .and_then(|e| e.message)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or(error.message);
    match code {
        Some(RATE_LIMIT_EXCEEDED) => Attempt::Again(PlatformError::RateLimited),
        Some(SYSTEM_ERROR) => Attempt::Again(PlatformError::Failed(message)),
        Some(IDENTITY_ERROR) => Attempt::Done(Err(PlatformError::Expired)),
        Some(code) if NO_ACCESS.contains(&code) || BUSINESS_ERRORS.contains(&code) => {
            Attempt::Done(Err(PlatformError::Refused(message)))
        }
        _ => Attempt::Done(Err(PlatformError::Failed(message))),
    }
}

#[derive(Deserialize)]
struct GraphQlAnswer {
    data: Option<serde_json::Value>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Deserialize)]
struct GraphQlError {
    #[serde(default)]
    message: String,
    extensions: Option<ErrorExtensions>,
}

#[derive(Deserialize)]
struct ErrorExtensions {
    code: Option<i64>,
    message: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connection<T> {
    #[serde(default = "Vec::new")]
    nodes: Vec<T>,
    page_info: PageInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    #[serde(default)]
    has_next_page: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProductNode {
    #[serde(deserialize_with = "id")]
    item_id: String,
    product_name: String,
    product_link: String,
    price_min: Option<String>,
    price_max: Option<String>,
    #[serde(default)]
    sales: u32,
    commission_rate: Option<String>,
    rating_star: Option<String>,
    #[serde(deserialize_with = "id")]
    shop_id: String,
    shop_name: String,
}

impl ProductNode {
    /// The offer, unless its ids are not numbers or its lowest price is
    /// missing or unreadable.
    fn offer(self) -> Option<FoundOffer> {
        if !number(&self.item_id) || !number(&self.shop_id) {
            return None;
        }
        let price = brl(self.price_min.as_deref()?)?;
        let highest_price = self
            .price_max
            .as_deref()
            .and_then(brl)
            .filter(|highest| highest.amount() >= price.amount())
            .unwrap_or(price);
        let link = shopee_page(&self.product_link).unwrap_or_else(|| {
            format!(
                "https://shopee.com.br/product/{}/{}",
                self.shop_id, self.item_id
            )
        });
        Some(FoundOffer {
            shop: FoundShop {
                link: shop_page(&self.shop_id),
                id: self.shop_id,
                name: self.shop_name,
                commission: None,
                rating: None,
            },
            id: self.item_id,
            title: self.product_name,
            link,
            price,
            highest_price,
            sales: self.sales,
            commission: rate(self.commission_rate.as_deref()),
            rating: stars(self.rating_star.as_deref()),
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShopNode {
    #[serde(deserialize_with = "id")]
    shop_id: String,
    shop_name: String,
    original_link: Option<String>,
    commission_rate: Option<String>,
    rating_star: Option<String>,
}

impl ShopNode {
    /// The shop, unless its id is not a number.
    fn shop(self) -> Option<FoundShop> {
        if !number(&self.shop_id) {
            return None;
        }
        Some(FoundShop {
            link: self
                .original_link
                .as_deref()
                .and_then(shopee_page)
                .unwrap_or_else(|| shop_page(&self.shop_id)),
            id: self.shop_id,
            name: self.shop_name,
            commission: rate(self.commission_rate.as_deref()),
            rating: stars(self.rating_star.as_deref()),
        })
    }
}

fn number(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
}

/// `link`, when it opens a page of Shopee Brasil over HTTPS: the app opens
/// these links, so one from an answer must never point anywhere else.
fn shopee_page(link: &str) -> Option<String> {
    let rest = link.trim().strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next()?.to_ascii_lowercase();
    (host == "shopee.com.br" || host.ends_with(".shopee.com.br")).then(|| link.trim().to_owned())
}

/// The page of the shop `id`; `productOfferV2` brings no link to it.
fn shop_page(id: &str) -> String {
    format!("https://shopee.com.br/shop/{id}")
}

/// Shopee's Int64 ids, which a client may also see as strings.
fn id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Number(u64),
        Text(String),
    }
    Ok(match Id::deserialize(deserializer)? {
        Id::Number(number) => number.to_string(),
        Id::Text(text) => text,
    })
}

/// Prices come as decimal text in reais, such as `"29.90"`.
fn brl(text: &str) -> Option<Money> {
    Decimal::from_str(text.trim())
        .ok()
        .filter(|amount| !amount.is_sign_negative())
        .map(|amount| Money::new(amount, Currency::Brl))
}

/// Rates come as a fraction, such as `"0.1"` for 10%.
fn rate(text: Option<&str>) -> Option<Percentage> {
    let fraction = Decimal::from_str(text?.trim()).ok()?;
    Percentage::new(fraction * Decimal::ONE_HUNDRED).ok()
}

/// Stars from 0 to 5, such as `"4.9"`.
fn stars(text: Option<&str>) -> Option<Decimal> {
    Decimal::from_str(text?.trim())
        .ok()
        .filter(|stars| *stars >= Decimal::ZERO && *stars <= Decimal::from(5))
}
