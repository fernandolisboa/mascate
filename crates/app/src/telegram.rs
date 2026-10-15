//! Telegram as Divulgação uses it (#103): the Bot API's adapter, where it
//! points, and how its answers read in Portuguese.

use std::sync::Arc;

use gpui_kit::{App, Global};
use mascate_integrations::{Connection, ConnectionState, Telegram};
use mascate_kernel::PlatformError;
use mascate_marketing::{DestinationCheck, DestinationStanding, PublishFailure};
use mascate_platform::{Build, Environment};

use crate::connections::AppConnections;

/// The Bot API's adapter.
pub struct AppTelegram(pub Arc<Telegram>);

impl Global for AppTelegram {}

pub fn adapter(cx: &App) -> Option<Arc<Telegram>> {
    cx.try_global::<AppTelegram>().map(|app| app.0.clone())
}

/// Where the app reaches the Bot API: Telegram's or, in a development
/// build only, the URL in `MASCATE_TELEGRAM_API_URL`, a local fake server.
pub fn api_url(build: Build, environment: &Environment) -> String {
    crate::mercado_livre::local_fake(build, environment, "MASCATE_TELEGRAM_API_URL")
        .unwrap_or_else(|| mascate_integrations::TELEGRAM_API_URL.to_owned())
}

/// Why posts cannot go out while the Connection is in `state`; `None` once
/// it is connected.
pub fn publishing_off(state: &ConnectionState) -> Option<&'static str> {
    match state {
        ConnectionState::Connected => None,
        ConnectionState::Expired => Some(
            "O Telegram não aceitou o token do bot: cole o token atual em Configurações › \
             Conexões › Telegram.",
        ),
        ConnectionState::Failed { .. } => {
            Some("Não consegui ler o token do bot no cofre do sistema.")
        }
        ConnectionState::NotConfigured | ConnectionState::AwaitingApproval => Some(
            "Para publicar pelo app, cole o token do bot em Configurações › Conexões › \
             Telegram.",
        ),
    }
}

/// The Telegram Connection's state now.
pub fn state(cx: &App) -> Option<ConnectionState> {
    cx.try_global::<AppConnections>()
        .map(|app| app.0.state(Connection::Telegram))
}

/// Notes that Telegram refused the stored token, so Conexões shows it.
pub fn expired(cx: &App) {
    if let Some(app) = cx.try_global::<AppConnections>() {
        app.0.mark_expired(Connection::Telegram);
    }
}

/// What "Conferir canal" found, as the owner reads it.
pub fn check_text(check: &DestinationCheck) -> String {
    let title = &check.title;
    match check.standing {
        DestinationStanding::CanPost => {
            format!("Tudo certo: o bot é administrador de {title} e pode publicar.")
        }
        DestinationStanding::CannotPost => format!(
            "O bot é administrador de {title}, mas sem a permissão Publicar mensagens: ligue-a \
             em Administradores, no canal."
        ),
        DestinationStanding::NotAdministrator => format!(
            "O bot não é administrador de {title}: adicione-o em Administradores, no canal, com \
             a permissão Publicar mensagens."
        ),
        DestinationStanding::NotAChannel => {
            format!("{title} é um grupo, não um canal: informe o @nome ou o id -100… de um canal.")
        }
    }
}

/// Why "Conferir canal" failed, as the owner reads it.
pub fn check_failure(error: &PlatformError) -> String {
    match error {
        PlatformError::NotConnected => {
            "Cole o token do bot em Configurações › Conexões › Telegram para conferir.".into()
        }
        PlatformError::Expired => "O Telegram não aceitou o token do bot: confira em \
                                   Configurações › Conexões › Telegram."
            .into(),
        PlatformError::RateLimited => {
            "O Telegram pediu para esperar. Confira de novo daqui a um minuto.".into()
        }
        PlatformError::Refused(why) => format!(
            "O Telegram recusou: {why}. Confira o chat do canal e se o bot foi adicionado a ele."
        ),
        PlatformError::NotFound => "O Telegram não achou o canal.".into(),
        PlatformError::Failed(why) => format!("Não consegui falar com o Telegram: {why}"),
    }
}

/// Why a post did not go out, or may not have, as the owner reads it.
pub fn publish_failure(failure: &PublishFailure) -> String {
    match failure {
        PublishFailure::NotConnected => {
            "Nada saiu: falta o token do bot em Configurações › Conexões › Telegram.".into()
        }
        PublishFailure::Expired => "Nada saiu: o Telegram não aceitou o token do bot. Cole o \
                                    token atual em Configurações › Conexões › Telegram."
            .into(),
        PublishFailure::Refused(why) => format!(
            "Nada saiu: o Telegram recusou o post ({why}). Confira o canal e se o bot pode \
             publicar nele."
        ),
        PublishFailure::RateLimited => {
            "Nada saiu: o Telegram pediu para esperar mais do que o app espera. Publique de \
             novo daqui a alguns minutos."
                .into()
        }
        PublishFailure::NotSent(why) => format!("Nada saiu: {why}."),
        PublishFailure::Unconfirmed(why) => format!(
            "O post pode ter saído: o pedido chegou ao Telegram, mas a resposta não ({why}). \
             Confira o canal antes de publicar de novo."
        ),
        PublishFailure::Interrupted => "O post pode ter saído: o app fechou enquanto ele era \
                                        enviado. Confira o canal antes de publicar de novo."
            .into(),
    }
}
