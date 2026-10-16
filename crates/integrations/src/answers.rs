//! What every adapter does with a Platform's answer.

use std::str::FromStr;

use mascate_kernel::PlatformError;
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

/// Answers are small JSON documents; anything bigger is not one.
const MAX_ANSWER_BYTES: u64 = 2 * 1024 * 1024;

/// A shipping label is a page or two of PDF; anything bigger is not one.
pub(crate) const MAX_DOCUMENT_BYTES: u64 = 10 * 1024 * 1024;

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

/// A picture's address, when it is plain HTTPS: it may be opened or
/// posted as it is.
pub(crate) fn https_link(link: &str) -> Option<String> {
    let link = link.trim();
    let rest = link.strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty() && !host.contains('@') && !link.contains(char::is_whitespace))
        .then(|| link.to_owned())
}

/// `link`, when it opens a page over HTTPS on `domain` or one of its
/// subdomains: the app opens these links, so one read from a Platform must
/// never point anywhere else.
pub(crate) fn page_on(link: &str, domain: &str) -> Option<String> {
    let link = https_link(link)?;
    let host = link["https://".len()..]
        .split(['/', '?', '#'])
        .next()?
        .to_ascii_lowercase();
    (host == domain || host.ends_with(&format!(".{domain}"))).then_some(link)
}
