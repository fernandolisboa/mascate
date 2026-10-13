//! Monetizze as the Commissions Sync reads it (#82): where the adapter
//! points.

use mascate_platform::{Build, Environment};

/// Where the app asks Monetizze for tokens and sales: API 2.1 or, in a
/// development build only, the URL in `MASCATE_MONETIZZE_API_URL`, a local
/// fake server.
pub fn api_url(build: Build, environment: &Environment) -> String {
    crate::mercado_livre::local_fake(build, environment, "MASCATE_MONETIZZE_API_URL")
        .unwrap_or_else(|| mascate_integrations::MONETIZZE_API_URL.to_owned())
}
