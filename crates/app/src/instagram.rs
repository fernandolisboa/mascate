//! Instagram as Divulgação uses it (#156): the API's adapter, where it
//! points, and how its answers read in Portuguese.

use std::sync::Arc;

use gpui_kit::{App, Global};
use mascate_integrations::{Connection, ConnectionState, Instagram};
use mascate_kernel::{PlatformError, Timestamp};
use mascate_marketing::PublishFailure;
use mascate_platform::{Build, Environment};

use crate::catalog::day;
use crate::connections::AppConnections;

/// Instagram's adapter.
pub struct AppInstagram(pub Arc<Instagram>);

impl Global for AppInstagram {}

pub fn adapter(cx: &App) -> Option<Arc<Instagram>> {
    cx.try_global::<AppInstagram>().map(|app| app.0.clone())
}

/// Where the app reaches the API: Instagram's or, in a development build
/// only, the URL in `MASCATE_INSTAGRAM_API_URL`, a local fake server.
pub fn api_url(build: Build, environment: &Environment) -> String {
    crate::mercado_livre::local_fake(build, environment, "MASCATE_INSTAGRAM_API_URL")
        .unwrap_or_else(|| mascate_integrations::INSTAGRAM_API_URL.to_owned())
}

/// The Instagram Connection's state now.
pub fn state(cx: &App) -> Option<ConnectionState> {
    cx.try_global::<AppConnections>()
        .map(|app| app.0.state(Connection::Instagram))
}

/// Notes that Instagram refused the token, so Conexões shows it.
pub fn expired(cx: &App) {
    if let Some(app) = cx.try_global::<AppConnections>() {
        app.0.mark_expired(Connection::Instagram);
    }
}

/// Why posts cannot go out while the Connection is in `state`; `None` once
/// it is connected.
pub fn publishing_off(state: &ConnectionState) -> Option<&'static str> {
    match state {
        ConnectionState::Connected => None,
        ConnectionState::Expired => Some(
            "O Instagram não aceitou mais o token: gere outro no painel do app na Meta e cole em \
             Configurações › Conexões › Instagram.",
        ),
        ConnectionState::Failed { .. } => {
            Some("Não consegui ler o token do Instagram no cofre do sistema.")
        }
        ConnectionState::NotConfigured | ConnectionState::AwaitingApproval => Some(
            "Para publicar pelo app, cole o token da sua conta em Configurações › Conexões › \
             Instagram.",
        ),
    }
}

/// Until when the token lasts, on the Connection's card.
pub fn valid_until_text(until: Timestamp) -> String {
    format!(
        "O token vale até {}. O app renova sozinho quando faltar menos de 30 dias, sempre que \
         falar com o Instagram.",
        day(until)
    )
}

/// Why a post did not go out, or may not have, as the owner reads it.
pub fn publish_failure(failure: &PublishFailure) -> String {
    match failure {
        PublishFailure::NotConnected => {
            "Nada saiu: cole o token da sua conta em Configurações › Conexões › Instagram.".into()
        }
        PublishFailure::Expired => "Nada saiu: o Instagram não aceitou mais o token. Gere outro \
                                    no painel do app na Meta e cole em Configurações › Conexões \
                                    › Instagram."
            .into(),
        PublishFailure::Refused(why) => {
            format!("Nada saiu: o Instagram recusou o post ({why}). Confira a foto e o texto.")
        }
        PublishFailure::RateLimited => {
            "Nada saiu: o Instagram pediu para esperar mais do que o app espera. Publique de \
             novo daqui a alguns minutos."
                .into()
        }
        PublishFailure::QuotaUsed => "Nada saiu: a conta já publicou pelo app o máximo de posts \
                                      que o Instagram deixa em 24 horas. Publique de novo mais \
                                      tarde."
            .into(),
        PublishFailure::NotSent(why) => format!("Nada saiu: {why}."),
        PublishFailure::Unconfirmed(why) => format!(
            "O post pode ter saído: o pedido chegou ao Instagram, mas a resposta não ({why}). \
             Use Conferir no Instagram antes de publicar de novo."
        ),
        PublishFailure::Interrupted => "O post pode ter saído: o app fechou enquanto ele era \
                                        enviado. Use Conferir no Instagram antes de publicar de \
                                        novo."
            .into(),
        PublishFailure::NotPublished => "O Instagram confirmou que este post não saiu. Publique \
                                         de novo quando quiser."
            .into(),
    }
}

/// Why "Conferir no Instagram" could not read the post's fate.
pub fn fate_failure(error: &PlatformError) -> String {
    match error {
        PlatformError::NotConnected => {
            "Cole o token da sua conta em Configurações › Conexões › Instagram para conferir."
                .into()
        }
        PlatformError::Expired => "O Instagram não aceitou mais o token: gere outro no painel do \
                                   app na Meta e cole em Configurações › Conexões › Instagram."
            .into(),
        PlatformError::RateLimited => {
            "O Instagram pediu para esperar. Confira de novo daqui a um minuto.".into()
        }
        PlatformError::Refused(why) => format!("O Instagram recusou a consulta: {why}."),
        PlatformError::NotFound => "O Instagram não achou o post preparado.".into(),
        PlatformError::Failed(why) => format!("Não consegui falar com o Instagram: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_development_build_points_at_a_fake() {
        let pointing: Environment = Box::new(|name: &str| {
            (name == "MASCATE_INSTAGRAM_API_URL").then(|| "http://127.0.0.1:8098".to_owned())
        });
        assert_eq!(
            api_url(Build::Development, &pointing),
            "http://127.0.0.1:8098"
        );
        assert_eq!(
            api_url(Build::Release, &pointing),
            mascate_integrations::INSTAGRAM_API_URL
        );
    }

    #[test]
    fn a_post_that_may_be_out_points_at_the_check() {
        for failure in [
            PublishFailure::Unconfirmed("timeout".into()),
            PublishFailure::Interrupted,
        ] {
            let text = publish_failure(&failure);
            assert!(text.starts_with("O post pode ter saído"), "{text}");
            assert!(text.contains("Conferir no Instagram"), "{text}");
        }
    }
}
