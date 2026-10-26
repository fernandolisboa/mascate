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

impl<T> Envelope<T> {
    /// The same answer, its `response` left aside as an empty one.
    pub fn without_response(self) -> Envelope<()> {
        Envelope {
            error: self.error,
            message: self.message,
            response: self.response.map(|_| ()),
        }
    }
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
    /// Only when asked for: the parcels the Order ships in.
    #[serde(default)]
    pub package_list: Vec<PackageEntry>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PackageEntry {
    pub package_number: String,
}

/// `order/get_package_detail`.
#[derive(Debug, Deserialize)]
pub(super) struct PackageDetails {
    #[serde(default)]
    pub package_list: Vec<PackageDetail>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PackageDetail {
    pub package_number: String,
    /// `LOGISTICS_READY` until the shipment is arranged,
    /// `LOGISTICS_REQUEST_CREATED` after, `LOGISTICS_PICKUP_RETRY` when the
    /// carrier missed the pickup and it is arranged again, and others once
    /// it left or was cancelled.
    #[serde(default)]
    pub fulfillment_status: String,
    /// Arranged already while still `LOGISTICS_READY`: Shopee is making
    /// the tracking number.
    #[serde(default)]
    pub is_shipment_arranged: bool,
}

/// `logistics/get_shipping_parameter`: the ways the parcel can go and what
/// each needs.
#[derive(Debug, Deserialize)]
pub(super) struct ShippingParameter {
    #[serde(default)]
    pub info_needed: InfoNeeded,
    pub pickup: Option<PickupOptions>,
}

/// The details each way needs; a way absent is not offered.
#[derive(Debug, Default, Deserialize)]
pub(super) struct InfoNeeded {
    pub dropoff: Option<Vec<String>>,
    pub pickup: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PickupOptions {
    #[serde(default)]
    pub address_list: Vec<PickupAddress>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PickupAddress {
    pub address_id: u64,
    /// `default_address`, `pickup_address`, `return_address`.
    #[serde(default)]
    pub address_flag: Vec<String>,
    #[serde(default)]
    pub time_slot_list: Vec<TimeSlot>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TimeSlot {
    #[serde(deserialize_with = "numeric_id")]
    pub pickup_time_id: String,
    /// `recommended` on the one Shopee suggests.
    #[serde(default)]
    pub flags: Vec<String>,
}

/// `logistics/get_tracking_number`: empty while the carrier has not given
/// one yet.
#[derive(Debug, Deserialize)]
pub(super) struct TrackingNumber {
    #[serde(default)]
    pub tracking_number: String,
}

/// `logistics/get_shipping_document_parameter`,
/// `create_shipping_document` and `get_shipping_document_result`: one
/// result per package.
#[derive(Debug, Deserialize)]
pub(super) struct DocumentResults {
    #[serde(default)]
    pub result_list: Vec<DocumentResult>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DocumentResult {
    pub suggest_shipping_document_type: Option<String>,
    /// `READY`, `PROCESSING` or `FAILED`.
    pub status: Option<String>,
    pub fail_error: Option<String>,
    pub fail_message: Option<String>,
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

/// `product/category_recommend`: the categories Shopee sees in a title,
/// likeliest first.
#[derive(Debug, Deserialize)]
pub(super) struct CategoryRecommendation {
    #[serde(default)]
    pub category_id: Vec<u64>,
}

/// `product/get_attribute_tree`, one entry per category asked.
#[derive(Debug, Deserialize)]
pub(super) struct AttributeTrees {
    #[serde(default)]
    pub list: Vec<AttributeTree>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AttributeTree {
    #[serde(default)]
    pub attribute_tree: Vec<AttributeNode>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AttributeNode {
    pub attribute_id: u64,
    #[serde(default)]
    pub mandatory: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub multi_lang: Vec<Translation>,
    #[serde(default)]
    pub attribute_value_list: Vec<AttributeOption>,
    pub attribute_info: Option<AttributeInfo>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AttributeOption {
    pub value_id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub multi_lang: Vec<Translation>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Translation {
    pub language: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct AttributeInfo {
    /// 1 single drop-down, 2 single combo box, 3 free text, 4 multiple
    /// drop-down, 5 multiple combo box.
    pub input_type: Option<u8>,
    /// 2 for a quantity with its unit.
    pub format_type: Option<u8>,
    #[serde(default)]
    pub attribute_unit_list: Vec<String>,
}

/// One page of `product/get_brand_list`.
#[derive(Debug, Deserialize)]
pub(super) struct BrandList {
    #[serde(default)]
    pub brand_list: Vec<Brand>,
    #[serde(default)]
    pub has_next_page: bool,
    #[serde(default)]
    pub next_offset: u64,
    #[serde(default)]
    pub is_mandatory: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct Brand {
    pub brand_id: u64,
    #[serde(default)]
    pub original_brand_name: String,
    pub display_brand_name: Option<String>,
}

/// `logistics/get_channel_list`: the shop's shipping channels.
#[derive(Debug, Deserialize)]
pub(super) struct LogisticsChannels {
    #[serde(default)]
    pub logistics_channel_list: Vec<LogisticsChannel>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LogisticsChannel {
    pub logistics_channel_id: u64,
    #[serde(default)]
    pub enabled: bool,
    /// `SIZE_SELECTION`, `SIZE_INPUT`, `FIXED_DEFAULT_PRICE` or
    /// `CUSTOM_PRICE`.
    pub fee_type: Option<String>,
    /// 0 for a channel buyers pick at checkout; another channel's id for
    /// one that fulfils it.
    #[serde(default)]
    pub mask_channel_id: u64,
    pub weight_limit: Option<WeightLimit>,
}

#[derive(Debug, Deserialize)]
pub(super) struct WeightLimit {
    /// Kilograms; 0 or none for no limit.
    pub item_max_weight: Option<serde_json::Number>,
    pub item_min_weight: Option<serde_json::Number>,
}

/// `media_space/upload_image`.
#[derive(Debug, Deserialize)]
pub(super) struct UploadedImage {
    pub image_info: Option<ImageInfo>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ImageInfo {
    pub image_id: String,
}

/// `product/add_item`: the item Shopee created.
#[derive(Debug, Deserialize)]
pub(super) struct AddedItem {
    #[serde(deserialize_with = "numeric_id")]
    pub item_id: String,
    pub item_status: Option<String>,
}

/// One page of `product/search_item`.
#[derive(Debug, Deserialize)]
pub(super) struct FoundItems {
    #[serde(default)]
    pub item_id_list: Vec<u64>,
    /// Empty on the last page.
    #[serde(default)]
    pub next_offset: String,
}

/// One page of `account_health/get_listings_with_issues`.
#[derive(Debug, Deserialize)]
pub(super) struct ListingsWithIssues {
    #[serde(default)]
    pub listing_list: Vec<ListingWithIssue>,
    #[serde(default)]
    pub total_count: usize,
}

#[derive(Debug, Deserialize)]
pub(super) struct ListingWithIssue {
    #[serde(deserialize_with = "numeric_id")]
    pub item_id: String,
    /// 1 prohibited, 2 counterfeit, 3 spam, 4 inappropriate image,
    /// 5 insufficient info, 6 Mall listing improvement, 7 other
    /// improvement.
    pub reason: i64,
}

/// `account_health/get_shop_performance`.
#[derive(Debug, Deserialize)]
pub(super) struct ShopPerformance {
    pub overall_performance: Option<OverallPerformance>,
    #[serde(default)]
    pub metric_list: Vec<Metric>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OverallPerformance {
    /// 1 poor, 2 improvement needed, 3 good, 4 excellent.
    pub rating: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Metric {
    /// 1 fulfillment, 2 listings, 3 customer service.
    pub metric_type: i64,
    /// Below zero, a group of metrics rather than one.
    pub metric_id: i64,
    pub metric_name: Option<String>,
    pub current_period: Option<serde_json::Number>,
    pub last_period: Option<serde_json::Number>,
    /// 1 number, 2 percentage, 3 seconds, 4 days, 5 hours.
    pub unit: Option<i64>,
    pub target: Option<MetricTarget>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MetricTarget {
    pub value: Option<serde_json::Number>,
    pub comparator: Option<String>,
}

/// `discount/get_discount_list`: a page of the shop's discounts.
#[derive(Debug, Deserialize)]
pub(super) struct DiscountList {
    #[serde(default)]
    pub discount_list: Vec<Discount>,
    #[serde(default)]
    pub more: bool,
}

/// One discount; `get_discount` adds a page of its items.
#[derive(Debug, Deserialize)]
pub(super) struct Discount {
    #[serde(deserialize_with = "numeric_id")]
    pub discount_id: String,
    pub discount_name: Option<String>,
    /// `upcoming`, `ongoing` or `expired`.
    pub status: Option<String>,
    pub start_time: i64,
    pub end_time: i64,
    /// `get_discount`'s page of the items in it.
    #[serde(default)]
    pub item_list: Vec<DiscountItem>,
    /// Whether `get_discount` has another page of items.
    #[serde(default)]
    pub more: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct DiscountItem {
    #[serde(deserialize_with = "numeric_id")]
    pub item_id: String,
    #[serde(default)]
    pub model_list: Vec<DiscountModel>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DiscountModel {
    #[serde(deserialize_with = "numeric_id")]
    pub model_id: String,
}

/// `discount/add_discount`.
#[derive(Debug, Deserialize)]
pub(super) struct AddedDiscount {
    #[serde(deserialize_with = "numeric_id")]
    pub discount_id: String,
}

/// `discount/add_discount_item` and `discount/delete_discount_item`: Shopee
/// answers success even when it refused some models, naming each in
/// `error_list`.
#[derive(Debug, Deserialize)]
pub(super) struct DiscountItems {
    #[serde(default)]
    pub error_list: Vec<DiscountItemError>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DiscountItemError {
    pub fail_message: Option<String>,
    pub fail_error: Option<String>,
}

/// `product/get_item_promotion`.
#[derive(Debug, Deserialize)]
pub(super) struct ItemPromotions {
    #[serde(default)]
    pub success_list: Vec<ItemPromotion>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ItemPromotion {
    #[serde(deserialize_with = "numeric_id")]
    pub item_id: String,
    #[serde(default)]
    pub promotion: Vec<PromotionOnItem>,
}

/// One Promotion on one model of the item, or on the item without models.
#[derive(Debug, Deserialize)]
pub(super) struct PromotionOnItem {
    /// Shopee's name for the kind, as `Campaign` or `Flash Sale`.
    pub promotion_type: Option<String>,
    #[serde(deserialize_with = "numeric_id")]
    pub promotion_id: String,
    pub start_time: Option<i64>,
    pub end_time: Option<i64>,
    #[serde(default)]
    pub promotion_price_info: Vec<PromotionPrice>,
    /// `ongoing` or `upcoming`.
    pub promotion_staging: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PromotionPrice {
    pub promotion_price: Option<serde_json::Number>,
}

/// One page of `ads/get_product_level_campaign_id_list`.
#[derive(Debug, Deserialize)]
pub(super) struct AdCampaignIds {
    #[serde(default)]
    pub campaign_list: Vec<AdCampaignId>,
    #[serde(default)]
    pub has_next_page: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdCampaignId {
    #[serde(deserialize_with = "numeric_id")]
    pub campaign_id: String,
}

/// `ads/get_product_level_campaign_setting_info`.
#[derive(Debug, Deserialize)]
pub(super) struct AdCampaignSettings {
    #[serde(default)]
    pub campaign_list: Vec<AdCampaignSetting>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdCampaignSetting {
    #[serde(deserialize_with = "numeric_id")]
    pub campaign_id: String,
    pub common_info: Option<AdCampaignCommon>,
    pub auto_bidding_info: Option<AdAutoBidding>,
    /// The products Shopee picked for a campaign with automatic selection.
    #[serde(default)]
    pub auto_product_ads_info: Vec<AdAutoProduct>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdCampaignCommon {
    pub ad_name: Option<String>,
    /// `ongoing`, `scheduled`, `ended`, `paused`, `deleted` or `closed`.
    pub campaign_status: Option<String>,
    #[serde(default)]
    pub item_id_list: Vec<u64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdAutoBidding {
    pub roas_target: Option<serde_json::Number>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdAutoProduct {
    pub item_id: u64,
}

/// One shop of `ads/get_product_campaign_daily_performance`, whose
/// `response` is a list.
#[derive(Debug, Deserialize)]
pub(super) struct AdShopPerformance {
    /// `BR` for a Brazilian shop.
    pub region: Option<String>,
    #[serde(default)]
    pub campaign_list: Vec<AdCampaignPerformance>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdCampaignPerformance {
    #[serde(deserialize_with = "numeric_id")]
    pub campaign_id: String,
    #[serde(default)]
    pub metrics_list: Vec<AdDayMetrics>,
}

/// A campaign's day. Shopee counts a sale within 7 days of a click; the
/// broad figures take the shop's other products bought after it too.
#[derive(Debug, Deserialize)]
pub(super) struct AdDayMetrics {
    /// `DD-MM-YYYY`.
    pub date: String,
    #[serde(default)]
    pub impression: u64,
    #[serde(default)]
    pub clicks: u64,
    pub expense: Option<serde_json::Number>,
    pub broad_gmv: Option<serde_json::Number>,
    /// Units the attributed sales sold.
    #[serde(default)]
    pub broad_order_amount: u64,
}
