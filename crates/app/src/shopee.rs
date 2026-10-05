//! Shopee Afiliados as the Ofertas screen uses it (#12): the adapter, where
//! it points, and how its failures read in Portuguese.

use std::sync::Arc;

use gpui_kit::{App, Global};
use mascate_integrations::{ConnectionState, ShopeeAffiliates};
use mascate_kernel::PlatformError;
use mascate_platform::{Build, Environment};

/// The Shopee Affiliate Open API's adapter.
pub struct AppShopee(pub Arc<ShopeeAffiliates>);

impl Global for AppShopee {}

pub fn adapter(cx: &App) -> Option<Arc<ShopeeAffiliates>> {
    cx.try_global::<AppShopee>().map(|app| app.0.clone())
}

/// Where the app reaches the Open API: Shopee's or, in a development build
/// only, the URL in `MASCATE_SHOPEE_API_URL`, for a local fake server.
pub fn api_url(build: Build, environment: &Environment) -> String {
    crate::mercado_livre::local_fake(build, environment, "MASCATE_SHOPEE_API_URL")
        .unwrap_or_else(|| mascate_integrations::SHOPEE_AFFILIATE_API_URL.to_owned())
}

/// Why the search is off while the Connection is in `state`; `None` once
/// it is connected.
pub fn search_off(state: &ConnectionState) -> Option<&'static str> {
    match state {
        ConnectionState::Connected => None,
        ConnectionState::AwaitingApproval => Some(
            "A busca na Shopee liga quando a Shopee aprovar o seu acesso à Open API de \
             afiliados e você colar o AppID e o Secret em Configurações › Conexões. Enquanto \
             isso, registre as ofertas à mão abaixo.",
        ),
        ConnectionState::Failed { .. } => Some(
            "Não consegui ler as chaves da Shopee no cofre do sistema; a busca fica desligada. \
             O cadastro à mão abaixo continua funcionando.",
        ),
        ConnectionState::NotConfigured | ConnectionState::Expired => Some(
            "Falta o AppID ou o Secret da Shopee em Configurações › Conexões; a busca fica \
             desligada até lá. O cadastro à mão abaixo continua funcionando.",
        ),
    }
}

/// Why a search failed, as the owner reads it.
pub fn failure(error: &PlatformError) -> String {
    match error {
        PlatformError::NotConnected => {
            "Cole o AppID e o Secret da Shopee em Configurações › Conexões para buscar.".into()
        }
        PlatformError::Expired => {
            "A Shopee não aceitou a assinatura: confira o AppID e o Secret em Configurações › \
             Conexões e se o relógio do computador está certo."
                .into()
        }
        PlatformError::RateLimited => {
            "A Shopee pediu para esperar e continuou pedindo depois de três tentativas. Busque \
             de novo daqui a alguns minutos."
                .into()
        }
        PlatformError::Refused(why) => format!("A Shopee recusou a busca: {why}"),
        PlatformError::NotFound => "A Shopee não encontrou o que foi pedido.".into(),
        PlatformError::Failed(why) => format!("Não consegui falar com a Shopee: {why}"),
    }
}
