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

/// One page of `order/get_order_list`.
#[derive(Debug, Deserialize)]
pub(super) struct OrderList {
    #[serde(default)]
    pub order_list: Vec<OrderEntry>,
    #[serde(default)]
    pub more: bool,
    #[serde(default)]
    pub next_cursor: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct OrderEntry {
    pub order_sn: String,
}

/// `order/get_order_detail`.
#[derive(Debug, Deserialize)]
pub(super) struct OrderDetails {
    #[serde(default)]
    pub order_list: Vec<OrderDetail>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OrderDetail {
    pub order_sn: String,
    pub currency: Option<String>,
    /// `UNPAID`, `PENDING`, `READY_TO_SHIP`, `PROCESSED`, `RETRY_SHIP`,
    /// `SHIPPED`, `TO_CONFIRM_RECEIVE`, `IN_CANCEL`, `CANCELLED`,
    /// `TO_RETURN`, `COMPLETED` or `INVOICE_PENDING`.
    pub order_status: String,
    pub create_time: i64,
    pub update_time: Option<i64>,
    /// The time to ship it by.
    pub ship_by_date: Option<i64>,
    /// What the buyer paid, shipping included; only once paid.
    pub total_amount: Option<serde_json::Number>,
    pub buyer_username: Option<String>,
    /// When the carrier picked the parcel up; 0 or absent before.
    pub pickup_done_time: Option<i64>,
    #[serde(default)]
    pub item_list: Vec<OrderItem>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OrderItem {
    #[serde(deserialize_with = "numeric_id")]
    pub item_id: String,
    pub item_name: Option<String>,
    /// 0 for an item without variations.
    #[serde(deserialize_with = "numeric_id")]
    pub model_id: String,
    pub model_name: Option<String>,
    #[serde(default)]
    pub model_quantity_purchased: u32,
    pub model_original_price: Option<serde_json::Number>,
    /// 0 on an item of a bundle deal, whose discount is not split by item.
    pub model_discounted_price: Option<serde_json::Number>,
    /// Units under a return the buyer asked for.
    #[serde(default)]
    pub return_requested_qty: u32,
    /// Units the buyer already returned.
    #[serde(default)]
    pub returned_qty: u32,
}

/// One page of `returns/get_return_list`.
#[derive(Debug, Deserialize)]
pub(super) struct ReturnList {
    #[serde(default, rename = "return")]
    pub returns: Vec<ReturnAnswer>,
    #[serde(default)]
    pub more: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ReturnAnswer {
    #[serde(deserialize_with = "numeric_id")]
    pub return_sn: String,
    pub order_sn: String,
    /// `REQUESTED`, `ACCEPTED`, `CANCELLED`, `JUDGING`, `CLOSED`,
    /// `PROCESSING` or `SELLER_DISPUTE`.
    pub status: String,
    /// What the buyer gets back.
    pub refund_amount: Option<serde_json::Number>,
    pub currency: Option<String>,
    /// The items travel back to the seller; a refund only leaves them with
    /// the buyer.
    #[serde(default)]
    pub needs_logistics: bool,
    #[serde(default)]
    pub item: Vec<ReturnItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ReturnItem {
    #[serde(deserialize_with = "numeric_id")]
    pub item_id: String,
    #[serde(deserialize_with = "numeric_id")]
    pub model_id: String,
    /// Units of the item.
    #[serde(default)]
    pub amount: u32,
}

/// `payment/get_escrow_detail`.
#[derive(Debug, Deserialize)]
pub(super) struct Escrow {
    pub order_income: Option<OrderIncome>,
}

/// What Shopee keeps of the Order and what it pays the seller, as the
/// escrow tells it. Every fee is positive; Shopee settles it at the end.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct OrderIncome {
    pub escrow_amount: Option<serde_json::Number>,
    pub escrow_amount_after_adjustment: Option<serde_json::Number>,
    pub commission_fee: Option<serde_json::Number>,
    pub service_fee: Option<serde_json::Number>,
    pub seller_order_processing_fee: Option<serde_json::Number>,
    pub seller_transaction_fee: Option<serde_json::Number>,
    pub order_ams_commission_fee: Option<serde_json::Number>,
    pub campaign_fee: Option<serde_json::Number>,
    pub actual_shipping_fee: Option<serde_json::Number>,
    pub buyer_paid_shipping_fee: Option<serde_json::Number>,
    pub shopee_shipping_rebate: Option<serde_json::Number>,
    pub shipping_fee_discount_from_3pl: Option<serde_json::Number>,
    pub reverse_shipping_fee: Option<serde_json::Number>,
    pub final_return_to_seller_shipping_fee: Option<serde_json::Number>,
}
