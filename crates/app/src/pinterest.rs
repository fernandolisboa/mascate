//! Pinterest as Divulgação uses it (#155): the API's adapter, where it
//! points, the boards a Pinterest channel picks from, and how its answers
//! read in Portuguese.

use std::sync::Arc;

use gpui_kit::{App, Global};
use mascate_integrations::{Connection, ConnectionState, Pinterest};
use mascate_kernel::PlatformError;
use mascate_marketing::PublishFailure;
use mascate_platform::{Build, Environment};

use crate::connections::AppConnections;

/// Pinterest's adapter.
pub struct AppPinterest(pub Arc<Pinterest>);

impl Global for AppPinterest {}

pub fn adapter(cx: &App) -> Option<Arc<Pinterest>> {
    cx.try_global::<AppPinterest>().map(|app| app.0.clone())
}

/// Where the app reaches the API and the login page: Pinterest's own or,
/// in a development build only, the URL in `MASCATE_PINTEREST_API_URL`:
/// Pinterest's sandbox, with the usual login page, or a local fake server
/// for both.
pub fn urls(build: Build, environment: &Environment) -> (String, String) {
    let login = mascate_integrations::PINTEREST_LOGIN_URL.to_owned();
    match crate::mercado_livre::local_fake(build, environment, "MASCATE_PINTEREST_API_URL") {
        Some(url) if url == mascate_integrations::PINTEREST_SANDBOX_URL => (url, login),
        Some(fake) => (
            fake.clone(),
            format!("{}/oauth/", fake.trim_end_matches('/')),
        ),
        None => (mascate_integrations::PINTEREST_API_URL.to_owned(), login),
    }
}

/// The Pinterest Connection's state now.
pub fn state(cx: &App) -> Option<ConnectionState> {
    cx.try_global::<AppConnections>()
        .map(|app| app.0.state(Connection::Pinterest))
}

/// Notes that Pinterest refused the login, so Conexões shows it.
pub fn expired(cx: &App) {
    if let Some(app) = cx.try_global::<AppConnections>() {
        app.0.mark_expired(Connection::Pinterest);
    }
}

/// Why Pins cannot go out while the Connection is in `state`; `None` once
/// it is connected.
pub fn publishing_off(state: &ConnectionState) -> Option<&'static str> {
    match state {
        ConnectionState::Connected => None,
        ConnectionState::Expired => Some(
            "O Pinterest não aceitou mais o login: entre de novo em Configurações › Conexões › \
             Pinterest.",
        ),
        ConnectionState::Failed { .. } => {
            Some("Não consegui ler o login do Pinterest no cofre do sistema.")
        }
        ConnectionState::NotConfigured | ConnectionState::AwaitingApproval => Some(
            "Para publicar pelo app, conecte a sua conta em Configurações › Conexões › \
             Pinterest.",
        ),
    }
}

/// Why opening Pinterest's page or trading its code failed.
pub fn login_failure(error: &PlatformError) -> String {
    match error {
        PlatformError::NotConnected => {
            "Salve o App ID e o App secret antes de entrar no Pinterest.".into()
        }
        PlatformError::Expired => {
            "O Pinterest não aceitou as chaves: confira o App ID e o App secret do app em My \
             apps."
                .into()
        }
        PlatformError::Refused(why) => format!(
            "O Pinterest recusou o código ({why}). Ele vale uma vez só e só para \
             http://localhost/, cadastrado em Redirect URIs: clique em Entrar no Pinterest de \
             novo e cole o endereço novo."
        ),
        PlatformError::RateLimited => {
            "O Pinterest pediu para esperar. Tente de novo daqui a alguns minutos.".into()
        }
        PlatformError::NotFound => "O Pinterest não encontrou o app.".into(),
        PlatformError::Failed(why) => format!("Não consegui falar com o Pinterest: {why}"),
    }
}

/// Why reading the boards failed, as the owner reads it.
pub fn boards_failure(error: &PlatformError) -> String {
    match error {
        PlatformError::NotConnected => {
            "Conecte a sua conta em Configurações › Conexões › Pinterest para ver as pastas.".into()
        }
        PlatformError::Expired => "O Pinterest não aceitou mais o login: entre de novo em \
                                   Configurações › Conexões › Pinterest."
            .into(),
        PlatformError::Refused(why) => format!(
            "O Pinterest recusou ({why}). Entre de novo em Configurações › Conexões › Pinterest \
             e autorize ler as pastas."
        ),
        PlatformError::RateLimited => {
            "O Pinterest pediu para esperar. Tente de novo daqui a um minuto.".into()
        }
        PlatformError::NotFound => "O Pinterest não achou as pastas da conta.".into(),
        PlatformError::Failed(why) => format!("Não consegui falar com o Pinterest: {why}"),
    }
}

/// Why a Pin did not go out, or may not have, as the owner reads it.
pub fn publish_failure(failure: &PublishFailure) -> String {
    match failure {
        PublishFailure::NotConnected => {
            "Nada saiu: conecte a sua conta em Configurações › Conexões › Pinterest.".into()
        }
        PublishFailure::Expired => "Nada saiu: o Pinterest não aceitou mais o login. Entre de \
                                    novo em Configurações › Conexões › Pinterest."
            .into(),
        PublishFailure::Refused(why) => format!(
            "Nada saiu: o Pinterest recusou o Pin ({why}). Confira a pasta do canal e a foto."
        ),
        PublishFailure::RateLimited => {
            "Nada saiu: o Pinterest pediu para esperar mais do que o app espera. Publique de \
             novo daqui a alguns minutos."
                .into()
        }
        PublishFailure::NotSent(why) => format!("Nada saiu: {why}."),
        PublishFailure::Unconfirmed(why) => format!(
            "O Pin pode ter saído: o pedido chegou ao Pinterest, mas a resposta não ({why}). \
             Confira a pasta antes de publicar de novo."
        ),
        PublishFailure::Interrupted => "O Pin pode ter saído: o app fechou enquanto ele era \
                                        enviado. Confira a pasta antes de publicar de novo."
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_development_build_can_point_at_the_sandbox_or_a_fake() {
        let pointing = |url: &'static str| -> Environment {
            Box::new(move |name: &str| {
                (name == "MASCATE_PINTEREST_API_URL").then(|| url.to_owned())
            })
        };
        assert_eq!(
            urls(
                Build::Development,
                &pointing(mascate_integrations::PINTEREST_SANDBOX_URL)
            ),
            (
                mascate_integrations::PINTEREST_SANDBOX_URL.to_owned(),
                mascate_integrations::PINTEREST_LOGIN_URL.to_owned()
            )
        );
        assert_eq!(
            urls(Build::Development, &pointing("http://127.0.0.1:8097")),
            (
                "http://127.0.0.1:8097".to_owned(),
                "http://127.0.0.1:8097/oauth/".to_owned()
            )
        );
        assert_eq!(
            urls(Build::Release, &pointing("http://127.0.0.1:8097")),
            (
                mascate_integrations::PINTEREST_API_URL.to_owned(),
                mascate_integrations::PINTEREST_LOGIN_URL.to_owned()
            )
        );
    }

    #[test]
    fn a_pin_that_may_be_out_says_to_look_at_the_board() {
        let text = publish_failure(&PublishFailure::Unconfirmed("timeout".into()));
        assert!(text.starts_with("O Pin pode ter saído"), "{text}");
        assert!(text.contains("Confira a pasta"), "{text}");
    }
}
