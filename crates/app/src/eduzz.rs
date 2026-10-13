//! Eduzz as the Commissions Sync reads it (#85): where the adapter points.

use mascate_platform::{Build, Environment};

/// Where the app asks Eduzz for the statement, the sales and the transfers:
/// the Developer Hub API or, in a development build only, the URL in
/// `MASCATE_EDUZZ_API_URL`, a local fake server.
pub fn api_url(build: Build, environment: &Environment) -> String {
    crate::mercado_livre::local_fake(build, environment, "MASCATE_EDUZZ_API_URL")
        .unwrap_or_else(|| mascate_integrations::EDUZZ_API_URL.to_owned())
}
