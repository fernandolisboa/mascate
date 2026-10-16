//! The parts of Magalu's answers the adapter reads, as the Open API
//! documents them.

use serde::Deserialize;

/// ID Magalu's answer to a token request: the tokens when it worked, an
/// OAuth `error` when not.
#[derive(Debug, Deserialize)]
pub(super) struct TokenAnswer {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    /// Seconds, 2 hours.
    #[serde(default)]
    pub expires_in: i64,
    pub error: Option<String>,
    pub error_description: Option<String>,
    pub message: Option<String>,
}

impl TokenAnswer {
    /// Why ID Magalu turned the request down, in its words.
    pub fn refusal(self) -> Option<String> {
        [self.error_description, self.message, self.error]
            .into_iter()
            .flatten()
            .find(|why| !why.trim().is_empty())
    }
}

/// Every error but a validation one: `{slug, message}`; a validation error
/// (422) lists the fields in `detail`.
#[derive(Debug, Deserialize)]
pub(super) struct ErrorAnswer {
    pub slug: Option<String>,
    pub message: Option<String>,
    #[serde(default)]
    pub detail: Vec<Invalid>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Invalid {
    #[serde(default)]
    pub loc: Vec<serde_json::Value>,
    pub msg: Option<String>,
}

impl ErrorAnswer {
    /// The message, or each invalid field with what is wrong with it.
    pub fn why(self) -> Option<String> {
        if let Some(message) = self.message.filter(|message| !message.trim().is_empty()) {
            return Some(message);
        }
        let fields: Vec<String> = self
            .detail
            .into_iter()
            .filter_map(|invalid| {
                let msg = invalid.msg?;
                let at: Vec<String> = invalid
                    .loc
                    .iter()
                    .map(|part| match part {
                        serde_json::Value::String(text) => text.clone(),
                        other => other.to_string(),
                    })
                    .collect();
                Some(if at.is_empty() {
                    msg
                } else {
                    format!("{}: {msg}", at.join("."))
                })
            })
            .collect();
        (!fields.is_empty())
            .then(|| fields.join("; "))
            .or(self.slug)
    }
}

/// `portfolios/me`: the store and its sales channel.
#[derive(Debug, Deserialize)]
pub(super) struct Me {
    pub channel: ChannelRef,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct ChannelRef {
    #[serde(default)]
    pub id: String,
}

/// One page of `portfolios/skus`.
#[derive(Debug, Deserialize)]
pub(super) struct SkuPage {
    #[serde(default)]
    pub results: Vec<Sku>,
}

/// A SKU of the portfolio, as listed or read on its own.
#[derive(Debug, Deserialize)]
pub(super) struct Sku {
    pub sku: String,
    pub title: Option<String>,
    pub status: Option<String>,
    #[serde(default = "selling")]
    pub active: bool,
    /// Stocked and shipped by Magalu, out of this epic (ADR 0033).
    #[serde(default)]
    pub fulfillment: bool,
    #[serde(default)]
    pub url_marketplace: Vec<MarketplaceUrl>,
}

fn selling() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub(super) struct MarketplaceUrl {
    pub channel: String,
    pub url: String,
}

/// `portfolios/prices/{sku}`: the SKU's price in each channel.
#[derive(Debug, Deserialize)]
pub(super) struct Prices {
    #[serde(default)]
    pub results: Vec<Price>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Price {
    #[serde(default)]
    pub channel: Option<ChannelRef>,
    /// What the buyer pays, in units of `currency` times `normalizer`.
    pub price: i64,
    #[serde(default = "cents")]
    pub normalizer: i64,
    #[serde(default = "brl")]
    pub currency: String,
}

fn cents() -> i64 {
    100
}

fn brl() -> String {
    "BRL".into()
}

/// `portfolios/stocks/{sku}`: the SKU's units in each channel and
/// warehouse.
#[derive(Debug, Deserialize)]
pub(super) struct Stocks {
    #[serde(default)]
    pub results: Vec<Stock>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Stock {
    #[serde(default)]
    pub channel: Option<ChannelRef>,
    /// `AVAILABLE` to sell, or `RESERVED` for Orders not yet shipped.
    #[serde(rename = "type")]
    pub kind: String,
    pub quantity: i64,
}

/// An amount in units of the Order's currency times `normalizer`, as
/// Magalu sends every value of an Order.
#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct Amount {
    pub total: Option<i64>,
    pub normalizer: Option<i64>,
}

/// One page of `orders`.
#[derive(Debug, Deserialize)]
pub(super) struct OrderPage {
    #[serde(default)]
    pub results: Vec<OrderAnswer>,
}

/// An Order with its deliveries, each with its items.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct OrderAnswer {
    /// The Order's number on Magalu, the one the seller sees.
    #[serde(deserialize_with = "crate::answers::numeric_id")]
    pub code: String,
    /// `new`, `approved`, `cancelled` or `finished`.
    pub status: Option<String>,
    pub purchased_at: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    #[serde(default)]
    pub amounts: OrderAmounts,
    #[serde(default)]
    pub deliveries: Vec<Delivery>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct OrderAmounts {
    /// What the buyer paid, shipping included.
    pub total: Option<i64>,
    pub normalizer: Option<i64>,
    pub currency: Option<String>,
    pub freight: Option<Amount>,
}

/// A delivery: the items of the Order one parcel takes.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct Delivery {
    pub id: String,
    /// `new`, `approved`, `invoiced`, `shipped`, `delivered`, `cancelled`,
    /// or `frozen` for a while in between.
    pub status: Option<String>,
    #[serde(default)]
    pub items: Vec<DeliveryItem>,
    pub shipping: Option<Shipping>,
    #[serde(default)]
    pub invoices: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct DeliveryItem {
    pub info: ItemInfo,
    pub quantity: u32,
    pub unit_price: UnitPrice,
    pub amounts: Option<ItemAmounts>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ItemInfo {
    pub sku: String,
    pub name: Option<String>,
    #[serde(default)]
    pub attributes: Vec<Attribute>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Attribute {
    pub name: Option<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct UnitPrice {
    pub value: i64,
    #[serde(default = "cents")]
    pub normalizer: i64,
    #[serde(default = "brl")]
    pub currency: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ItemAmounts {
    /// Magalu's commission on the item, every unit together.
    pub commission: Option<Amount>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Shipping {
    pub handling_time: Option<Deadline>,
    /// When the parcel was posted: it left the seller.
    pub posting_date: Option<String>,
    pub recipient: Option<Recipient>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Deadline {
    /// A date, or a date and time, in UTC.
    pub limit_date: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Recipient {
    pub name: Option<String>,
    pub address: Option<Address>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Address {
    pub street: Option<String>,
    pub number: Option<String>,
    pub complement: Option<String>,
    pub district: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub zipcode: Option<String>,
}

/// One page of `financial-analysis/orders`.
#[derive(Debug, Deserialize)]
pub(super) struct AnalysisPage {
    pub results: Option<Vec<Analysis>>,
}

/// What Magalu kept and paid on an Order, line by line.
#[derive(Debug, Deserialize)]
pub(super) struct Analysis {
    pub extras: Option<AnalysisExtras>,
    pub transactions: Option<Vec<Transaction>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AnalysisExtras {
    /// Text in the documentation; a number is read the same.
    #[serde(default, deserialize_with = "optional_numeric_id")]
    pub order_code: Option<String>,
}

fn optional_numeric_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Ok(
        Option::<serde_json::Value>::deserialize(deserializer)?.and_then(|value| match value {
            serde_json::Value::String(text) => Some(text),
            serde_json::Value::Number(number) => Some(number.to_string()),
            _ => None,
        }),
    )
}

/// A credit to the seller, a debit, or a line for information only.
#[derive(Debug, Deserialize)]
pub(super) struct Transaction {
    pub category: String,
    pub subcategory: Option<String>,
    /// `CREDIT`, `DEBIT` or `INFO`.
    #[serde(rename = "type")]
    pub kind: String,
    pub identifier: Option<String>,
    pub description: Option<String>,
    pub value: i64,
    #[serde(default = "cents")]
    pub normalizer: i64,
    #[serde(default = "brl")]
    pub currency: String,
    pub entity: Option<Entity>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Entity {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
}
