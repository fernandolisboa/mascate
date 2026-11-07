//! Mercado Livre as the screens that read it share it: the adapter, where
//! it points, and how its failures read in Portuguese.

use std::sync::Arc;

use gpui_kit::{App, Global};
use mascate_integrations::{ConnectionState, MercadoLivre};
use mascate_kernel::{PlatformError, SalesChannelKind};
use mascate_platform::{Build, Environment};

use crate::sales_channels;

/// Mercado Livre's adapter, shared by the screens that read it.
pub struct AppMercadoLivre(pub Arc<MercadoLivre>);

impl Global for AppMercadoLivre {}

/// The adapter; absent before the window's globals are set.
pub fn adapter(cx: &App) -> Option<Arc<MercadoLivre>> {
    cx.try_global::<AppMercadoLivre>().map(|app| app.0.clone())
}

/// Where the app reaches Mercado Livre: its API or, in a development build
/// only, the URL in `MASCATE_ML_API_URL`, for a local fake server.
pub fn api_url(build: Build, environment: &Environment) -> String {
    local_fake(build, environment, FAKE_API_URL)
        .unwrap_or_else(|| mascate_integrations::MERCADO_LIVRE_API_URL.to_owned())
}

/// Where the app reads Mercado Pago's payments: its API or, in a
/// development build, the same local fake server as Mercado Livre.
pub fn payments_api_url(build: Build, environment: &Environment) -> String {
    local_fake(build, environment, FAKE_API_URL)
        .unwrap_or_else(|| mascate_integrations::MERCADO_PAGO_API_URL.to_owned())
}

/// Where the seller authorizes the app: Mercado Livre's page or, in a
/// development build, the same local fake server's `/authorization`.
pub fn authorization_page(build: Build, environment: &Environment) -> String {
    local_fake(build, environment, FAKE_API_URL).map_or_else(
        || mascate_integrations::MERCADO_LIVRE_AUTHORIZATION_PAGE.to_owned(),
        |fake| format!("{}/authorization", fake.trim_end_matches('/')),
    )
}

const FAKE_API_URL: &str = "MASCATE_ML_API_URL";

/// The URL in the environment variable `variable`, in a development build
/// only: a local fake server standing in for a Platform.
pub fn local_fake(build: Build, environment: &Environment, variable: &str) -> Option<String> {
    match (build, environment(variable)) {
        (Build::Development, Some(url)) if !url.trim().is_empty() => Some(url.trim().to_owned()),
        _ => None,
    }
}

/// Why the Connection is not usable, and what is shown meanwhile.
pub fn not_connected(state: &ConnectionState) -> String {
    sales_channels::not_connected(SalesChannelKind::MercadoLivre, state)
}

/// Why opening Mercado Livre's authorization page or trading its code
/// failed.
pub fn login_failure(error: &PlatformError) -> String {
    match error {
        PlatformError::NotConnected => {
            "Salve o Client ID e o Client Secret antes de entrar no Mercado Livre.".into()
        }
        PlatformError::Refused(why) => format!(
            "O Mercado Livre recusou o código ({why}). Ele vale uma vez só, por poucos minutos, \
             e só para o endereço cadastrado na aplicação do DevCenter: clique em Entrar no \
             Mercado Livre de novo e cole o endereço novo. Se o motivo for a aplicação, confira \
             o Client ID e o Client Secret."
        ),
        PlatformError::RateLimited => {
            "O Mercado Livre pediu para esperar. Tente de novo daqui a alguns minutos.".into()
        }
        other => failure(other),
    }
}

/// Why a call to Mercado Livre failed, as the owner reads it.
pub fn failure(error: &PlatformError) -> String {
    sales_channels::failure(SalesChannelKind::MercadoLivre, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seller_authorizes_on_mercado_livre_unless_a_development_build_points_elsewhere() {
        let pointing = |url: &'static str| -> Environment {
            Box::new(move |name: &str| (name == "MASCATE_ML_API_URL").then(|| url.to_owned()))
        };
        assert_eq!(
            authorization_page(Build::Release, &pointing("http://127.0.0.1:8090")),
            "https://auth.mercadolivre.com.br/authorization"
        );
        assert_eq!(
            authorization_page(Build::Development, &pointing("http://127.0.0.1:8090/")),
            "http://127.0.0.1:8090/authorization"
        );
        assert_eq!(
            authorization_page(
                Build::Development,
                &(Box::new(|_: &str| None) as Environment)
            ),
            "https://auth.mercadolivre.com.br/authorization"
        );
    }
}
