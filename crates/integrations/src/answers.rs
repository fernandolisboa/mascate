//! What every adapter does with a Platform's answer.

use mascate_kernel::PlatformError;
use serde::de::DeserializeOwned;

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
