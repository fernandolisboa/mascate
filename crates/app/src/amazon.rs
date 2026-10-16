//! Amazon as a Sales Channel (#126): where the seller adapter points.

use mascate_platform::{Build, Environment};

/// Where the app asks Amazon for tokens and listings: Amazon's own hosts
/// or, in a development build only, the URL in `MASCATE_AMAZON_API_URL` for
/// both, a local fake server.
pub fn urls(build: Build, environment: &Environment) -> (String, String) {
    match crate::mercado_livre::local_fake(build, environment, "MASCATE_AMAZON_API_URL") {
        Some(fake) => (fake.clone(), fake),
        None => (
            mascate_integrations::AMAZON_API_URL.to_owned(),
            mascate_integrations::AMAZON_AUTH_URL.to_owned(),
        ),
    }
}
