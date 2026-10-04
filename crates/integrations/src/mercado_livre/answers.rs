//! The parts of Mercado Livre's answers the adapter reads.

use std::str::FromStr;

use mascate_kernel::{Currency, Money};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Number;

/// A price as Mercado Livre sends it, a JSON number with its currency id;
/// `None` when either is missing or not one the app handles. The number is
/// read from its text, never through a float.
pub(super) fn money(amount: &Option<Number>, currency: &Option<String>) -> Option<Money> {
    let text = amount.as_ref()?.to_string();
    let amount = Decimal::from_str(&text)
        .or_else(|_| Decimal::from_scientific(&text))
        .ok()?;
    Some(Money::new(
        amount,
        Currency::from_code(currency.as_deref()?)?,
    ))
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

#[derive(Deserialize)]
pub(super) struct ItemsAnswer {
    pub code: u16,
    pub body: Option<ItemAnswer>,
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
}
