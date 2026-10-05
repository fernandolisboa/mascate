//! The parts of Mercado Livre's answers the adapter reads.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use mascate_kernel::{Currency, Money, Timestamp};
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

/// An id Mercado Livre sends as a JSON number, as text.
pub(super) fn whole_id(number: &Number) -> String {
    number.to_string()
}

/// A time as Mercado Livre writes it, such as
/// `2026-10-04T10:30:00.000-03:00`.
pub(super) fn time(text: &str) -> Option<Timestamp> {
    DateTime::parse_from_rfc3339(text)
        .or_else(|_| DateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f%z"))
        .ok()
        .map(|at| at.with_timezone(&Utc))
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
    /// `error_message` in the promotions API.
    #[serde(alias = "error_message")]
    pub message: Option<String>,
    /// `error` or `warning`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
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
    #[serde(default)]
    pub tags: Vec<String>,
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

/// One category `/sites/{site}/domain_discovery/search` predicts.
#[derive(Deserialize)]
pub(super) struct DomainPrediction {
    pub category_id: String,
    pub category_name: String,
    /// Attribute values it read in the title.
    #[serde(default)]
    pub attributes: Vec<Attribute>,
}

/// One attribute of `/categories/{id}/attributes`.
#[derive(Deserialize)]
pub(super) struct AttributeDefinition {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub tags: AttributeTags,
}

/// The tags that say how much a category wants an attribute.
#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct AttributeTags {
    pub required: bool,
    pub catalog_required: bool,
    pub conditional_required: bool,
    pub new_required: bool,
    pub read_only: bool,
}

#[derive(Deserialize)]
pub(super) struct PictureAnswer {
    pub id: String,
}

/// A listing as `POST /items` and the `/items` multiget report its state.
#[derive(Deserialize)]
pub(super) struct ItemState {
    pub id: String,
    pub status: Option<String>,
    pub permalink: Option<String>,
}

/// A page of `/orders/search`.
#[derive(Deserialize)]
pub(super) struct OrderSearch {
    #[serde(default)]
    pub results: Vec<OrderAnswer>,
    #[serde(default)]
    pub paging: Paging,
}

/// An Order as `/orders/search` and `/orders/{id}` report it.
#[derive(Deserialize)]
pub(super) struct OrderAnswer {
    pub id: Number,
    pub status: String,
    pub date_created: String,
    pub last_updated: Option<String>,
    pub date_last_updated: Option<String>,
    pub pack_id: Option<Number>,
    #[serde(default)]
    pub order_items: Vec<OrderItemAnswer>,
    pub total_amount: Option<Number>,
    pub paid_amount: Option<Number>,
    pub currency_id: Option<String>,
    #[serde(default)]
    pub payments: Vec<PaymentAnswer>,
    pub buyer: Option<OrderBuyer>,
    pub shipping: Option<OrderShipping>,
    /// The claims the buyer opened on the Order.
    #[serde(default)]
    pub mediations: Vec<MediationAnswer>,
}

#[derive(Deserialize)]
pub(super) struct MediationAnswer {
    pub id: Number,
}

/// `/post-purchase/v2/claims/{id}/returns`.
#[derive(Deserialize)]
pub(super) struct ReturnAnswer {
    pub id: Number,
    pub status: String,
    #[serde(default)]
    pub orders: Vec<ReturnedOrderAnswer>,
}

#[derive(Deserialize)]
pub(super) struct ReturnedOrderAnswer {
    pub order_id: Number,
    pub item_id: String,
    pub variation_id: Option<Number>,
    /// Units coming back, written as text such as "1.0".
    pub return_quantity: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub(super) struct OrderItemAnswer {
    pub item: OrderItem,
    pub quantity: u32,
    pub unit_price: Option<Number>,
    pub sale_fee: Option<Number>,
    pub currency_id: Option<String>,
    pub listing_type_id: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct OrderItem {
    pub id: String,
    pub title: String,
    pub variation_id: Option<Number>,
    #[serde(default)]
    pub variation_attributes: Vec<Attribute>,
}

#[derive(Deserialize)]
pub(super) struct PaymentAnswer {
    pub status: Option<String>,
    pub shipping_cost: Option<Number>,
    /// What went back to the buyer of the payment, shipping aside.
    pub transaction_amount_refunded: Option<Number>,
}

/// `/shipments/{id}/costs`: what the buyer and each seller pay.
#[derive(Deserialize)]
pub(super) struct ShipmentCostsAnswer {
    #[serde(default)]
    pub senders: Vec<SenderCost>,
}

#[derive(Deserialize)]
pub(super) struct SenderCost {
    pub cost: Option<Number>,
}

/// `/billing/integration/group/ML/order/details`: the charges billed on
/// each Order asked for.
#[derive(Deserialize)]
pub(super) struct BillingAnswer {
    #[serde(default)]
    pub results: Vec<BilledOrderAnswer>,
}

#[derive(Deserialize)]
pub(super) struct BilledOrderAnswer {
    pub order_id: Number,
    #[serde(default)]
    pub details: Vec<BillingDetail>,
}

#[derive(Deserialize)]
pub(super) struct BillingDetail {
    pub charge_info: ChargeInfo,
    pub marketplace_info: Option<MarketplaceInfo>,
    pub currency_info: Option<CurrencyInfo>,
}

#[derive(Deserialize)]
pub(super) struct ChargeInfo {
    pub detail_id: Number,
    pub transaction_detail: Option<String>,
    pub detail_amount: Option<Number>,
    /// `CHARGE`, or `BONUS` for a charge given back.
    pub detail_type: Option<String>,
    /// Such as `CV` (the sale fee) or `CXD` (shipping); `B…` gives one back.
    pub detail_sub_type: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct MarketplaceInfo {
    pub marketplace: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct CurrencyInfo {
    pub currency_id: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct OrderBuyer {
    pub nickname: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct OrderShipping {
    pub id: Option<Number>,
}

/// A shipment as `/shipments/{id}` reports it with `x-format-new: true`.
#[derive(Deserialize)]
pub(super) struct ShipmentAnswer {
    pub id: Number,
    pub status: String,
    pub lead_time: Option<LeadTime>,
    pub destination: Option<Destination>,
}

#[derive(Deserialize)]
pub(super) struct LeadTime {
    pub estimated_handling_limit: Option<DateAnswer>,
}

#[derive(Deserialize)]
pub(super) struct DateAnswer {
    pub date: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct Destination {
    pub receiver_name: Option<String>,
    pub shipping_address: Option<ShippingAddress>,
}

#[derive(Deserialize)]
pub(super) struct ShippingAddress {
    pub address_line: Option<String>,
    pub zip_code: Option<String>,
    pub city: Option<NamedPlace>,
    pub state: Option<NamedPlace>,
}

#[derive(Deserialize)]
pub(super) struct NamedPlace {
    pub name: Option<String>,
}

/// `/item/{id}/performance`: the listing's quality, its groups of goals
/// (`buckets`), each goal (`variables`) and what reaches it (`rules`).
#[derive(Deserialize)]
pub(super) struct PerformanceAnswer {
    pub score: Option<Number>,
    pub level: Option<String>,
    pub level_wording: Option<String>,
    #[serde(default)]
    pub buckets: Vec<PerformanceBucket>,
}

#[derive(Deserialize)]
pub(super) struct PerformanceBucket {
    #[serde(default)]
    pub variables: Vec<PerformanceVariable>,
}

#[derive(Deserialize)]
pub(super) struct PerformanceVariable {
    pub title: Option<String>,
    #[serde(default)]
    pub rules: Vec<PerformanceRule>,
}

#[derive(Deserialize)]
pub(super) struct PerformanceRule {
    pub key: String,
    /// `PENDING` or `COMPLETED`.
    pub status: Option<String>,
    /// `WARNING` lowers the score until fixed; `OPPORTUNITY` raises it.
    pub mode: Option<String>,
    pub wordings: Option<RuleWordings>,
}

#[derive(Deserialize)]
pub(super) struct RuleWordings {
    pub title: Option<String>,
    pub label: Option<String>,
    pub link: Option<String>,
}

/// `/items/{id}/visits/time_window`: the listing's visits in the window.
#[derive(Deserialize)]
pub(super) struct VisitsAnswer {
    pub total_visits: u32,
}

/// A page of `/questions/search` with `api_version=4`.
#[derive(Deserialize)]
pub(super) struct QuestionSearch {
    #[serde(default)]
    pub total: u32,
    #[serde(default)]
    pub questions: Vec<QuestionAnswer>,
}

/// A question as `/questions/search` and `/questions/{id}` describe it.
#[derive(Deserialize)]
pub(super) struct QuestionAnswer {
    pub id: Number,
    pub item_id: String,
    pub status: String,
    /// Empty when Mercado Livre removed it.
    #[serde(default)]
    pub text: Option<String>,
    pub date_created: String,
    pub answer: Option<QuestionReply>,
}

#[derive(Deserialize)]
pub(super) struct QuestionReply {
    #[serde(default)]
    pub text: Option<String>,
    pub date_created: Option<String>,
}

/// The part of `/users/me` that holds the seller's Reputation.
#[derive(Deserialize)]
pub(super) struct UserReputationAnswer {
    pub seller_reputation: Option<SellerReputationAnswer>,
}

#[derive(Deserialize)]
pub(super) struct SellerReputationAnswer {
    /// Such as `5_green`; `null` until the seller has sales enough.
    pub level_id: Option<String>,
    /// The color without the protection, such as `red`.
    pub real_level: Option<String>,
    pub protection_end_date: Option<String>,
    pub transactions: Option<TransactionsAnswer>,
    pub metrics: Option<ReputationMetricsAnswer>,
}

#[derive(Deserialize)]
pub(super) struct TransactionsAnswer {
    pub completed: Option<u32>,
}

#[derive(Deserialize)]
pub(super) struct ReputationMetricsAnswer {
    pub sales: Option<SalesMetricAnswer>,
    pub claims: Option<RateMetricAnswer>,
    pub delayed_handling_time: Option<RateMetricAnswer>,
    pub cancellations: Option<RateMetricAnswer>,
}

#[derive(Deserialize)]
pub(super) struct SalesMetricAnswer {
    /// Such as `60 days`.
    pub period: Option<String>,
    pub completed: Option<u32>,
}

/// A rate as a fraction of the period's sales, with the real one apart
/// while a protection holds it at zero.
#[derive(Deserialize)]
pub(super) struct RateMetricAnswer {
    pub rate: Option<Number>,
    pub value: Option<u32>,
    pub excluded: Option<ExcludedAnswer>,
}

#[derive(Deserialize)]
pub(super) struct ExcludedAnswer {
    pub real_rate: Option<Number>,
    pub real_value: Option<u32>,
}

/// A page of `/reviews/item/{id}`.
#[derive(Deserialize)]
pub(super) struct ReviewsAnswer {
    pub paging: Option<ReviewsPaging>,
    #[serde(default)]
    pub reviews: Vec<ReviewAnswer>,
    pub rating_levels: Option<RatingLevels>,
}

#[derive(Deserialize)]
pub(super) struct ReviewsPaging {
    pub total: Option<u32>,
}

#[derive(Deserialize)]
pub(super) struct ReviewAnswer {
    pub id: Number,
    pub date_created: String,
    pub title: Option<String>,
    pub content: Option<String>,
    /// From 1 to 5 stars.
    pub rate: Option<u8>,
}

#[derive(Deserialize)]
pub(super) struct RatingLevels {
    #[serde(default)]
    pub one_star: u32,
    #[serde(default)]
    pub two_star: u32,
    #[serde(default)]
    pub three_star: u32,
    #[serde(default)]
    pub four_star: u32,
    #[serde(default)]
    pub five_star: u32,
}

/// A page of `/seller-promotions/users/{id}`: the seller's campaigns,
/// coupons and the channel's invitations.
#[derive(Deserialize)]
pub(super) struct SellerPromotionsAnswer {
    #[serde(default)]
    pub results: Vec<SellerPromotionAnswer>,
    pub paging: Option<Paging>,
}

/// A campaign or coupon, from the seller's list or read alone.
#[derive(Deserialize)]
pub(super) struct SellerPromotionAnswer {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: Option<String>,
    pub name: Option<String>,
    pub start_date: Option<String>,
    pub finish_date: Option<String>,
    /// A coupon's `FIXED_AMOUNT` or `FIXED_PERCENTAGE`.
    pub sub_type: Option<String>,
    pub fixed_amount: Option<Number>,
    pub fixed_percentage: Option<Number>,
    pub min_purchase_amount: Option<Number>,
    pub max_purchase_amount: Option<Number>,
    pub budget: Option<Number>,
}

/// One Promotion a listing is in, or invited to, from
/// `/seller-promotions/items/{id}`.
#[derive(Deserialize)]
pub(super) struct ItemPromotionAnswer {
    /// The Promotion's id; a price discount has none of its own.
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: Option<String>,
    pub name: Option<String>,
    /// What a buyer pays during it; 0 for a coupon and an invitation.
    pub price: Option<Number>,
    pub start_date: Option<String>,
    pub finish_date: Option<String>,
}

/// `/advertising/advertisers?product_id=PADS`: the accounts the user can
/// see in Product Ads.
#[derive(Deserialize)]
pub(super) struct AdvertisersAnswer {
    #[serde(default)]
    pub advertisers: Vec<AdvertiserAnswer>,
}

#[derive(Deserialize)]
pub(super) struct AdvertiserAnswer {
    pub advertiser_id: Number,
    pub site_id: Option<String>,
}

/// A page of `/advertising/{site}/advertisers/{id}/product_ads/...` search.
#[derive(Deserialize)]
pub(super) struct AdsSearchAnswer<T> {
    #[serde(default = "Vec::new")]
    pub results: Vec<T>,
    pub paging: Option<Paging>,
}

/// A Product Ads campaign.
#[derive(Deserialize)]
pub(super) struct AdCampaignAnswer {
    pub id: Number,
    pub name: Option<String>,
    pub status: Option<String>,
}

/// An ad, with its metrics in the dates asked.
#[derive(Deserialize)]
pub(super) struct AdAnswer {
    pub item_id: String,
    pub campaign_id: Option<Number>,
    pub metrics: Option<AdMetricsAnswer>,
}

#[derive(Deserialize)]
pub(super) struct AdMetricsAnswer {
    pub clicks: Option<Number>,
    pub prints: Option<Number>,
    pub cost: Option<Number>,
    pub direct_amount: Option<Number>,
    pub indirect_amount: Option<Number>,
    pub total_amount: Option<Number>,
    pub units_quantity: Option<Number>,
}
