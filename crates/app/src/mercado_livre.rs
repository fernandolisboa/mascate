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

/// Why a call to Mercado Livre failed, as the owner reads it.
pub fn failure(error: &PlatformError) -> String {
    sales_channels::failure(SalesChannelKind::MercadoLivre, error)
}
