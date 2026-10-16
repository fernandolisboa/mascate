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

/// The answer to `patchListingsItem`: accepted for processing, or invalid
/// with the issues why.
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
