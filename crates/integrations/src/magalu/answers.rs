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
