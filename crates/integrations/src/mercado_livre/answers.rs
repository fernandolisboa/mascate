//! The parts of Mercado Livre's answers the adapter reads.

use std::str::FromStr;

use mascate_kernel::{Currency, Money};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Number;

/// A price as Mercado Livre sends it, a JSON number with its currency id;
/// `None` when either is missing or not one the app handles. The number is
/// read from its text, never through a float.
pub(super) fn money(amount: &Option<Number>, currency: &Option<String>) -> Option<Money> {
    Some(Money::new(
        decimal(amount)?,
        Currency::from_code(currency.as_deref()?)?,
    ))
}

/// A JSON number, read from its text, never through a float.
pub(super) fn decimal(number: &Option<Number>) -> Option<Decimal> {
    let text = number.as_ref()?.to_string();
    Decimal::from_str(&text)
        .or_else(|_| Decimal::from_scientific(&text))
        .ok()
}

#[derive(Deserialize)]
pub(super) struct TokenAnswer {
    pub access_token: String,
    pub expires_in: i64,
    pub refresh_token: String,
}

#[derive(Deserialize)]
pub(super) struct ErrorAnswer {
    pub message: Option<String>,
    pub error: Option<String>,
    /// What exactly was wrong with a request, when Mercado Livre says.
    #[serde(default)]
    pub cause: Vec<ErrorCause>,
}

#[derive(Deserialize)]
pub(super) struct ErrorCause {
    pub message: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct CategoryAnswer {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub path_from_root: Vec<CategoryRef>,
}

#[derive(Deserialize)]
pub(super) struct CategoryRef {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
pub(super) struct Highlights {
    #[serde(default)]
    pub content: Vec<Highlight>,
}

#[derive(Deserialize)]
pub(super) struct Highlight {
    pub id: String,
    pub position: u32,
    #[serde(rename = "type")]
    pub kind: HighlightKind,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum HighlightKind {
    /// A product of Mercado Livre's catalog.
    Product,
    /// A product a seller created, with an `MLBU` id.
    UserProduct,
    /// A single listing outside the catalog.
    Item,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
pub(super) struct ProductAnswer {
    pub id: String,
    pub name: String,
    pub buy_box_winner: Option<BuyBoxWinner>,
}

#[derive(Deserialize)]
pub(super) struct BuyBoxWinner {
    pub price: Option<Number>,
    pub currency_id: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct UserProductAnswer {
    pub id: String,
    pub name: String,
    pub catalog_product_id: Option<String>,
}

/// One entry of a multiget such as `/items?ids=...`: each id answers with
/// its own status, and the body of one not found is an error, not the
/// resource.
#[derive(Deserialize)]
pub(super) struct MultigetEntry {
    pub code: u16,
    #[serde(default)]
    pub body: serde_json::Value,
}

impl MultigetEntry {
    pub fn body<T: DeserializeOwned>(self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.body)
    }
}

#[derive(Deserialize)]
pub(super) struct ItemAnswer {
    pub id: String,
    pub title: String,
    pub price: Option<Number>,
    pub currency_id: Option<String>,
    pub catalog_product_id: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ProductSearch {
    #[serde(default)]
    pub results: Vec<FoundProduct>,
}

#[derive(Deserialize)]
pub(super) struct FoundProduct {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
pub(super) struct ProductItems {
    #[serde(default)]
    pub paging: Paging,
    #[serde(default)]
    pub results: Vec<CompetingItem>,
}

#[derive(Deserialize, Default)]
pub(super) struct Paging {
    #[serde(default)]
    pub total: u32,
}

#[derive(Deserialize)]
pub(super) struct CompetingItem {
    pub category_id: Option<String>,
    pub price: Option<Number>,
    pub currency_id: Option<String>,
}

/// One listing type's prices, or all of them when the request named none.
#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum ListingPrices {
    One(ListingPrice),
    Many(Vec<ListingPrice>),
}

impl ListingPrices {
    pub fn for_listing(self, listing_type: &str) -> Option<ListingPrice> {
        match self {
            ListingPrices::One(price) => {
                (price.listing_type_id.as_deref() == Some(listing_type)).then_some(price)
            }
            ListingPrices::Many(prices) => prices
                .into_iter()
                .find(|price| price.listing_type_id.as_deref() == Some(listing_type)),
        }
    }
}

#[derive(Deserialize)]
pub(super) struct ListingPrice {
    pub listing_type_id: Option<String>,
    pub sale_fee_amount: Option<Number>,
    pub currency_id: Option<String>,
    pub sale_fee_details: Option<SaleFeeDetails>,
}

/// How the sale fee is made up: a percentage of the price plus a fixed
/// amount, which Mercado Livre charges below a price.
#[derive(Deserialize)]
pub(super) struct SaleFeeDetails {
    pub percentage_fee: Option<Number>,
    pub fixed_fee: Option<Number>,
}

/// A listing's variation ids, as `GET /items/{id}` reports them.
#[derive(Deserialize)]
pub(super) struct ItemVariationIds {
    #[serde(default)]
    pub variations: Vec<VariationId>,
}

#[derive(Deserialize)]
pub(super) struct VariationId {
    pub id: u64,
}

#[derive(Deserialize)]
pub(super) struct UserAnswer {
    pub id: u64,
}

/// A page of `/users/{id}/items/search` with `search_type=scan`.
#[derive(Deserialize)]
pub(super) struct SellerItems {
    #[serde(default)]
    pub results: Vec<String>,
    pub scroll_id: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct SellerItem {
    pub id: String,
    pub title: String,
    pub price: Option<Number>,
    pub currency_id: Option<String>,
    #[serde(default)]
    pub available_quantity: u32,
    pub status: String,
    pub permalink: Option<String>,
    pub listing_type_id: Option<String>,
    pub category_id: Option<String>,
    pub seller_custom_field: Option<String>,
    #[serde(default)]
    pub attributes: Vec<Attribute>,
    #[serde(default)]
    pub variations: Vec<ItemVariation>,
}

#[derive(Deserialize)]
pub(super) struct ItemVariation {
    pub id: u64,
    #[serde(default)]
    pub attribute_combinations: Vec<Attribute>,
    pub price: Option<Number>,
    #[serde(default)]
    pub available_quantity: u32,
    pub seller_custom_field: Option<String>,
    #[serde(default)]
    pub attributes: Vec<Attribute>,
}

#[derive(Deserialize)]
pub(super) struct Attribute {
    pub id: Option<String>,
    pub name: Option<String>,
    pub value_name: Option<String>,
}
