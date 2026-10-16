//! The parts of Amazon's answers the adapter reads, as the Selling Partner
//! API documents them.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct TokenAnswer {
    pub access_token: String,
    pub expires_in: i64,
}

/// `{"errors":[{"code","message","details"}]}`, the body of every refusal.
#[derive(Debug, Deserialize)]
pub(super) struct ErrorAnswer {
    #[serde(default)]
    pub errors: Vec<ErrorEntry>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ErrorEntry {
    pub message: Option<String>,
}

/// One page of `searchListingsItems`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ItemSearch {
    #[serde(default)]
    pub number_of_results: u32,
    pub pagination: Option<Pagination>,
    #[serde(default)]
    pub items: Vec<Item>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Pagination {
    pub next_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Item {
    pub sku: String,
    #[serde(default)]
    pub summaries: Vec<Summary>,
    #[serde(default)]
    pub offers: Vec<Offer>,
    #[serde(default)]
    pub fulfillment_availability: Vec<Availability>,
    #[serde(default)]
    pub relationships: Vec<MarketplaceRelationships>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Summary {
    pub marketplace_id: String,
    pub asin: Option<String>,
    pub product_type: Option<String>,
    /// `BUYABLE` and `DISCOVERABLE`, or neither.
    #[serde(default)]
    pub status: Vec<String>,
    pub item_name: Option<String>,
    pub created_date: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Offer {
    pub marketplace_id: String,
    pub offer_type: Option<String>,
    pub price: Option<Price>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Price {
    /// The reference names it `currencyCode`; some of Amazon's own examples
    /// write `currency`.
    #[serde(alias = "currency")]
    pub currency_code: String,
    pub amount: Amount,
}

/// A decimal Amazon sends as text, or as a JSON number.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum Amount {
    Text(String),
    Number(serde_json::Number),
}

impl Amount {
    pub fn decimal(&self) -> Option<Decimal> {
        match self {
            Amount::Text(text) => Decimal::from_str(text.trim()).ok(),
            Amount::Number(number) => crate::answers::decimal(number),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Availability {
    /// `DEFAULT` for the seller's own fulfillment; Amazon's centers have
    /// their own codes.
    pub fulfillment_channel_code: String,
    pub quantity: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MarketplaceRelationships {
    #[serde(default)]
    pub relationships: Vec<Relationship>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Relationship {
    #[serde(default)]
    pub child_skus: Vec<String>,
}

/// The answer to `patchListingsItem` and `putListingsItem`: accepted for
/// processing (or valid, in a preview), or invalid with the issues why.
#[derive(Debug, Deserialize)]
pub(super) struct Submission {
    pub status: String,
    #[serde(default)]
    pub issues: Vec<Issue>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Issue {
    pub message: Option<String>,
    pub severity: Option<String>,
}

/// `getListingsItem` with `includedData=issues`: what Amazon says is wrong
/// with a listing.
#[derive(Debug, Deserialize)]
pub(super) struct ItemIssues {
    #[serde(default)]
    pub issues: Vec<ListingIssue>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ListingIssue {
    pub code: Option<String>,
    pub message: Option<String>,
    pub severity: Option<String>,
    #[serde(default)]
    pub attribute_names: Vec<String>,
    pub enforcements: Option<Enforcements>,
    #[serde(default)]
    pub marketplace_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Enforcements {
    #[serde(default)]
    pub actions: Vec<EnforcementAction>,
    pub exemption: Option<Exemption>,
}

#[derive(Debug, Deserialize)]
pub(super) struct EnforcementAction {
    pub action: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct Exemption {
    pub status: String,
}

/// One page of `searchCatalogItems` (Catalog Items 2022-04-01).
#[derive(Debug, Deserialize)]
pub(super) struct CatalogSearch {
    #[serde(default)]
    pub items: Vec<CatalogAnswer>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CatalogAnswer {
    pub asin: String,
    #[serde(default)]
    pub summaries: Vec<CatalogSummary>,
    #[serde(default)]
    pub product_types: Vec<CatalogProductType>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CatalogSummary {
    pub marketplace_id: String,
    pub brand: Option<String>,
    pub item_name: Option<String>,
    pub browse_classification: Option<BrowseClassification>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowseClassification {
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CatalogProductType {
    pub marketplace_id: Option<String>,
    pub product_type: Option<String>,
}

/// A product type's definition, of which only its name is read.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProductTypeDefinition {
    pub product_type: String,
    pub display_name: String,
}

/// The answer to `getMyFeesEstimateForSKU`.
#[derive(Debug, Deserialize)]
pub(super) struct FeesAnswer {
    pub payload: Option<FeesPayload>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct FeesPayload {
    pub fees_estimate_result: Option<FeesEstimateResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct FeesEstimateResult {
    /// `Success`, `ClientError` or `ServiceError`.
    pub status: String,
    pub fees_estimate: Option<FeesEstimate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct FeesEstimate {
    pub total_fees_estimate: Option<FeesMoney>,
    #[serde(default)]
    pub fee_detail_list: Vec<FeeDetail>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct FeeDetail {
    pub fee_type: String,
    pub final_fee: Option<FeesMoney>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct FeesMoney {
    pub currency_code: String,
    pub amount: Amount,
}

/// One page of `searchOrders` (Orders 2026-01-01).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OrderSearch {
    #[serde(default)]
    pub orders: Vec<OrderAnswer>,
    pub pagination: Option<Pagination>,
}

/// The answer to `getOrder` (Orders 2026-01-01).
#[derive(Debug, Deserialize)]
pub(super) struct OrderFound {
    pub order: OrderAnswer,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OrderAnswer {
    pub order_id: String,
    pub created_time: String,
    pub last_updated_time: String,
    pub recipient: Option<Recipient>,
    pub proceeds: Option<OrderProceeds>,
    pub fulfillment: Option<OrderFulfillment>,
    #[serde(default)]
    pub order_items: Vec<OrderItem>,
    #[serde(default)]
    pub packages: Vec<OrderPackage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Recipient {
    pub delivery_address: Option<DeliveryAddress>,
}

/// Only what shipping needs is read: no phone, no company, no tax id.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeliveryAddress {
    pub name: Option<String>,
    pub address_line1: Option<String>,
    pub address_line2: Option<String>,
    pub address_line3: Option<String>,
    pub city: Option<String>,
    pub state_or_region: Option<String>,
    pub postal_code: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OrderProceeds {
    pub grand_total: Option<Price>,
    #[serde(default)]
    pub breakdowns: Vec<ProceedsBreakdown>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ProceedsBreakdown {
    /// `ITEM`, `SHIPPING`, `DISCOUNT`, `TAX` and others.
    #[serde(rename = "type")]
    pub kind: String,
    pub subtotal: Price,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OrderFulfillment {
    pub fulfillment_status: String,
    pub ship_by_window: Option<TimeWindow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TimeWindow {
    pub latest_date_time: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OrderItem {
    pub order_item_id: String,
    pub quantity_ordered: u32,
    pub product: ItemProduct,
    pub proceeds: Option<ItemProceeds>,
    pub fulfillment: Option<ItemFulfillment>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ItemProduct {
    pub asin: Option<String>,
    pub title: Option<String>,
    pub seller_sku: Option<String>,
    pub price: Option<ItemPrice>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ItemPrice {
    pub unit_price: Option<Price>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ItemProceeds {
    #[serde(default)]
    pub breakdowns: Vec<ProceedsBreakdown>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ItemFulfillment {
    pub quantity_fulfilled: Option<u32>,
    pub quantity_unfulfilled: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OrderPackage {
    pub package_status: Option<PackageStatus>,
    pub ship_time: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PackageStatus {
    pub status: String,
    pub detailed_status: Option<String>,
}

/// One page of `listTransactions` (Finances 2024-06-19).
#[derive(Debug, Deserialize)]
pub(super) struct TransactionsAnswer {
    pub payload: Option<TransactionsPayload>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TransactionsPayload {
    pub next_token: Option<String>,
    #[serde(default)]
    pub transactions: Vec<Transaction>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Transaction {
    pub transaction_id: Option<String>,
    pub description: Option<String>,
    pub posted_date: Option<String>,
    #[serde(default)]
    pub related_identifiers: Vec<RelatedIdentifier>,
    #[serde(default)]
    pub breakdowns: Breakdowns,
    #[serde(default)]
    pub items: Vec<TransactionItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RelatedIdentifier {
    pub related_identifier_name: Option<String>,
    pub related_identifier_value: Option<String>,
}

/// The reference lists a transaction's breakdowns; its full example wraps
/// them in an object: either reads.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum Breakdowns {
    List(Vec<Breakdown>),
    Wrapped {
        #[serde(default)]
        breakdowns: Vec<Breakdown>,
    },
}

impl Default for Breakdowns {
    fn default() -> Self {
        Breakdowns::List(Vec::new())
    }
}

impl Breakdowns {
    pub fn list(&self) -> &[Breakdown] {
        match self {
            Breakdowns::List(list) | Breakdowns::Wrapped { breakdowns: list } => list,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct TransactionItem {
    #[serde(default)]
    pub breakdowns: Vec<Breakdown>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Breakdown {
    pub breakdown_type: Option<String>,
    pub breakdown_amount: Option<FinanceAmount>,
    #[serde(default)]
    pub breakdowns: Vec<Breakdown>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FinanceAmount {
    pub currency_code: String,
    pub currency_amount: Amount,
}
