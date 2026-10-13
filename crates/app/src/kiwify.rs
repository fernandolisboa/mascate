//! Kiwify as the Commissions Sync reads it (#84): where the adapter points.

use mascate_platform::{Build, Environment};

/// Where the app asks Kiwify for tokens, sales and withdrawals: the public
/// API or, in a development build only, the URL in `MASCATE_KIWIFY_API_URL`,
/// a local fake server.
pub fn api_url(build: Build, environment: &Environment) -> String {
    crate::mercado_livre::local_fake(build, environment, "MASCATE_KIWIFY_API_URL")
        .unwrap_or_else(|| mascate_integrations::KIWIFY_API_URL.to_owned())
}
