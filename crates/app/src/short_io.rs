//! Short.io as Afiliação uses it (#158): the shortener's adapter, where it
//! points, and how its answers read in Portuguese, the clicks read in the
//! Commissions Sync (#159) included.

use std::sync::Arc;

use gpui_kit::{App, Global};
use mascate_affiliate::{AffiliateError, ClicksRead, ShortenedLink};
use mascate_integrations::{Connection, ShortIo};
use mascate_kernel::PlatformError;
use mascate_platform::{Build, Environment};

use crate::affiliate::failure;
use crate::connections::AppConnections;

/// Short.io's adapter.
pub struct AppShortIo(pub Arc<ShortIo>);

impl Global for AppShortIo {}

pub fn adapter(cx: &App) -> Option<Arc<ShortIo>> {
    cx.try_global::<AppShortIo>().map(|app| app.0.clone())
}

/// Where the app reaches Short.io: its API or, in a development build
/// only, the URL in `MASCATE_SHORT_IO_API_URL`, a local fake server.
pub fn api_url(build: Build, environment: &Environment) -> String {
    crate::mercado_livre::local_fake(build, environment, "MASCATE_SHORT_IO_API_URL")
        .unwrap_or_else(|| mascate_integrations::SHORT_IO_API_URL.to_owned())
}

/// Where the app reads Short.io's clicks: its statistics API or, in a
/// development build only, the same fake server as [`api_url`].
pub fn statistics_url(build: Build, environment: &Environment) -> String {
    crate::mercado_livre::local_fake(build, environment, "MASCATE_SHORT_IO_API_URL")
        .unwrap_or_else(|| mascate_integrations::SHORT_IO_STATISTICS_URL.to_owned())
}

/// Notes that Short.io refused the key, so Conexões shows it.
pub fn expired(cx: &App) {
    if let Some(app) = cx.try_global::<AppConnections>() {
        app.0.mark_expired(Connection::ShortIo);
    }
}

/// What "Encurtar" gave, as the owner reads it.
pub fn shortened_text(shortened: &ShortenedLink) -> String {
    let link = &shortened.link;
    let short = link.short.as_deref().unwrap_or_default();
    if shortened.existed {
        format!("{} já tinha link curto: {short}.", link.product)
    } else {
        format!(
            "Link curto de {} criado: {short}. Os posts de oferta deste link usam ele, menos \
             no Pinterest, que recebe o link inteiro.",
            link.product
        )
    }
}

/// Why "Encurtar" failed, as the owner reads it.
pub fn shorten_failure(error: &AffiliateError) -> String {
    let AffiliateError::Platform(error) = error else {
        return failure(error);
    };
    match error {
        PlatformError::NotConnected => "Para encurtar, cole a chave secreta e o domínio do \
                                        Short.io em Configurações › Conexões › Short.io."
            .into(),
        PlatformError::Expired => "O Short.io não aceitou a chave: crie outra chave secreta em \
                                   Integrations and API e cole em Configurações › Conexões › \
                                   Short.io."
            .into(),
        PlatformError::RateLimited => {
            "O Short.io pediu para esperar. Encurte de novo daqui a um minuto.".into()
        }
        PlatformError::Refused(why) => format!(
            "O Short.io recusou: {why}. Confira o domínio em Configurações › Conexões › \
             Short.io."
        ),
        PlatformError::NotFound => "O Short.io não achou o domínio.".into(),
        PlatformError::Failed(why) => format!("Não consegui falar com o Short.io: {why}"),
    }
}

/// "Cliques do Short.io: 2 dias novos ou mudados".
pub fn clicks_text(read: &ClicksRead) -> String {
    let changed = match read.changed {
        0 => "nada novo".to_owned(),
        1 => "1 dia novo ou mudado".to_owned(),
        count => format!("{count} dias novos ou mudados"),
    };
    let gone = match read.gone {
        0 => String::new(),
        1 => ", 1 link curto que o Short.io não tem mais".to_owned(),
        count => format!(", {count} links curtos que o Short.io não tem mais"),
    };
    format!("Cliques do Short.io: {changed}{gone}")
}

/// Why the clicks could not be read, as the owner reads it: the ones read
/// before stay on the dashboard.
pub fn clicks_failure(error: &AffiliateError) -> String {
    let why = match error {
        AffiliateError::Platform(PlatformError::Expired) => "o Short.io não aceitou a chave; \
                                                              crie outra em Integrations and \
                                                              API e cole em Configurações › \
                                                              Conexões › Short.io"
            .to_owned(),
        AffiliateError::Platform(PlatformError::RateLimited) => {
            "o Short.io pediu para esperar; o próximo Sync tenta de novo".to_owned()
        }
        AffiliateError::Platform(PlatformError::Refused(why)) => {
            format!("o Short.io recusou: {why}")
        }
        AffiliateError::Platform(PlatformError::Failed(why)) => {
            format!("não consegui falar com o Short.io: {why}")
        }
        error => failure(error),
    };
    format!("Não li os cliques dos links curtos: {why}. O painel mostra os da última leitura.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_development_build_points_at_a_fake() {
        let pointing: Environment = Box::new(|name: &str| {
            (name == "MASCATE_SHORT_IO_API_URL").then(|| "http://127.0.0.1:8158".to_owned())
        });
        assert_eq!(
            api_url(Build::Development, &pointing),
            "http://127.0.0.1:8158"
        );
        assert_eq!(
            api_url(Build::Release, &pointing),
            mascate_integrations::SHORT_IO_API_URL
        );
    }

    #[test]
    fn the_statistics_point_at_the_same_fake_in_development_only() {
        let pointing: Environment = Box::new(|name: &str| {
            (name == "MASCATE_SHORT_IO_API_URL").then(|| "http://127.0.0.1:8158".to_owned())
        });
        assert_eq!(
            statistics_url(Build::Development, &pointing),
            "http://127.0.0.1:8158"
        );
        assert_eq!(
            statistics_url(Build::Release, &pointing),
            mascate_integrations::SHORT_IO_STATISTICS_URL
        );
    }

    #[test]
    fn a_failed_read_of_the_clicks_says_the_last_ones_stay() {
        let text = clicks_failure(&AffiliateError::Platform(PlatformError::RateLimited));
        assert!(text.contains("última leitura"), "{text}");
        let text = clicks_failure(&AffiliateError::Platform(PlatformError::Expired));
        assert!(text.contains("Conexões › Short.io"), "{text}");
    }

    #[test]
    fn a_read_of_the_clicks_says_what_changed() {
        assert_eq!(
            clicks_text(&ClicksRead::default()),
            "Cliques do Short.io: nada novo"
        );
        assert_eq!(
            clicks_text(&ClicksRead {
                links: 3,
                changed: 2,
                gone: 1
            }),
            "Cliques do Short.io: 2 dias novos ou mudados, 1 link curto que o Short.io não tem \
             mais"
        );
    }

    #[test]
    fn a_missing_or_refused_key_points_at_the_connection() {
        for error in [PlatformError::NotConnected, PlatformError::Expired] {
            let text = shorten_failure(&AffiliateError::Platform(error));
            assert!(text.contains("Conexões › Short.io"), "{text}");
        }
    }
}
