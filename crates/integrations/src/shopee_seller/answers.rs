//! The parts of Shopee's answers the adapter reads, as the Open Platform v2
//! documents them.

use serde::Deserialize;

use crate::answers::numeric_id;

/// Every shop API answer: `response` when it worked, `error` and `message`
/// when not.
#[derive(Debug, Deserialize)]
pub(super) struct Envelope<T> {
    pub error: Option<String>,
    pub message: Option<String>,
    pub response: Option<T>,
}

/// `auth/token/get` and `auth/access_token/get`, which answer flat.
#[derive(Debug, Deserialize)]
pub(super) struct TokenAnswer {
    pub error: Option<String>,
    pub message: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    /// Seconds, 4 hours.
    #[serde(default)]
    pub expire_in: i64,
}

/// One page of `product/get_item_list`.
#[derive(Debug, Deserialize)]
pub(super) struct ItemList {
    #[serde(default)]
    pub item: Vec<ItemEntry>,
    #[serde(default)]
    pub has_next_page: bool,
    #[serde(default)]
    pub next_offset: u32,
}

#[derive(Debug, Deserialize)]
pub(super) struct ItemEntry {
    #[serde(deserialize_with = "numeric_id")]
    pub item_id: String,
}

/// `product/get_item_base_info`.
#[derive(Debug, Deserialize)]
pub(super) struct ItemBaseInfo {
    #[serde(default)]
    pub item_list: Vec<Item>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Item {
    #[serde(deserialize_with = "numeric_id")]
    pub item_id: String,
    pub category_id: Option<u64>,
    pub item_name: Option<String>,
    pub item_sku: Option<String>,
    /// `NORMAL`, `UNLIST`, `REVIEWING`, `BANNED`, `SELLER_DELETE` or
    /// `SHOPEE_DELETE`.
    pub item_status: String,
    #[serde(default)]
    pub has_model: bool,
    /// Absent on an item with models: each model has its own.
    #[serde(default)]
    pub price_info: Vec<PriceInfo>,
    pub stock_info_v2: Option<StockInfo>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PriceInfo {
    pub currency: String,
    /// The price the seller sets; `current_price` has Shopee's and the
    /// seller's discounts taken off.
    pub original_price: serde_json::Number,
}

#[derive(Debug, Deserialize)]
pub(super) struct StockInfo {
    pub summary_info: Option<StockSummary>,
    #[serde(default)]
    pub seller_stock: Vec<SellerStock>,
}

#[derive(Debug, Deserialize)]
pub(super) struct StockSummary {
    pub total_available_stock: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SellerStock {
    #[serde(default)]
    pub stock: u32,
}

impl StockInfo {
    /// The units buyers can take now.
    pub fn available(&self) -> u32 {
        self.summary_info
            .as_ref()
            .and_then(|summary| summary.total_available_stock)
            .unwrap_or_else(|| self.seller_stock.iter().map(|stock| stock.stock).sum())
    }
}

/// `product/get_model_list`: an item's variations, by tier.
#[derive(Debug, Deserialize)]
pub(super) struct ModelList {
    #[serde(default)]
    pub tier_variation: Vec<TierVariation>,
    #[serde(default)]
    pub model: Vec<Model>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TierVariation {
    pub name: String,
    #[serde(default)]
    pub option_list: Vec<TierOption>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TierOption {
    pub option: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct Model {
    #[serde(deserialize_with = "numeric_id")]
    pub model_id: String,
    pub model_sku: Option<String>,
    /// `MODEL_NORMAL` or `MODEL_UNAVAILABLE`.
    pub model_status: Option<String>,
    #[serde(default)]
    pub tier_index: Vec<usize>,
    #[serde(default)]
    pub price_info: Vec<PriceInfo>,
    pub stock_info_v2: Option<StockInfo>,
}

/// What `update_price`, `update_stock` and `unlist_item` report: the parts
/// Shopee took and the ones it did not, with why.
#[derive(Debug, Deserialize)]
pub(super) struct Update {
    #[serde(default)]
    pub failure_list: Vec<Failure>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Failure {
    pub failed_reason: Option<String>,
}

/// `product/get_category`.
#[derive(Debug, Deserialize)]
pub(super) struct Categories {
    #[serde(default)]
    pub category_list: Vec<Category>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Category {
    pub category_id: u64,
    pub original_category_name: Option<String>,
    pub display_category_name: Option<String>,
}
