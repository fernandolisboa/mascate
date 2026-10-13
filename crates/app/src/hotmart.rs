//! Hotmart as the Commissions Sync reads it (#78): where the adapter
//! points.

use mascate_platform::{Build, Environment};

/// Where the app asks Hotmart for tokens and sales: Hotmart's own hosts or,
/// in a development build only, the URL in `MASCATE_HOTMART_API_URL` for
/// both, a local fake server.
pub fn urls(build: Build, environment: &Environment) -> (String, String) {
    match crate::mercado_livre::local_fake(build, environment, "MASCATE_HOTMART_API_URL") {
        Some(fake) => (fake.clone(), fake),
        None => (
            mascate_integrations::HOTMART_API_URL.to_owned(),
            mascate_integrations::HOTMART_AUTH_URL.to_owned(),
        ),
    }
}
