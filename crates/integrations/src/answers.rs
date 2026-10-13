//! What every adapter does with a Platform's answer.

use std::str::FromStr;

use mascate_kernel::PlatformError;
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

/// Answers are small JSON documents; anything bigger is not one.
const MAX_ANSWER_BYTES: u64 = 2 * 1024 * 1024;

/// The JSON document `platform` answered.
pub(crate) fn read_json<T: DeserializeOwned>(
    response: ureq::http::Response<ureq::Body>,
    platform: &str,
) -> Result<T, PlatformError> {
    let bytes = response
        .into_body()
        .into_with_config()
        .limit(MAX_ANSWER_BYTES)
        .read_to_vec()
        .map_err(|error| PlatformError::Failed(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        PlatformError::Failed(format!("{platform} answered something unexpected: {error}"))
    })
}

/// A Platform's numeric id, which a client may also see as a string, as
/// text.
pub(crate) fn numeric_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Number(u64),
        Text(String),
    }
    Ok(match Id::deserialize(deserializer)? {
        Id::Number(number) => number.to_string(),
        Id::Text(text) => text,
    })
}

/// An amount a Platform sends as a JSON number: its text, not a float,
/// becomes the decimal.
pub(crate) fn decimal(number: &serde_json::Number) -> Option<Decimal> {
    let text = number.to_string();
    Decimal::from_str(&text)
        .or_else(|_| Decimal::from_scientific(&text))
        .ok()
}
