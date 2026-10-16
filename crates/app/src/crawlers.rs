//! The crawlers as the screens use them (#152, ADR 0035): the one engine
//! the app shares, the sites it can search, each built only while its
//! Restricted Feature is on, and how their failures read in Portuguese.

use std::sync::Arc;

use chrono::Local;
use gpui_kit::{App, Global};
use mascate_catalog::OfferSource;
use mascate_integrations::{
    Crawler, MERCADO_LIVRE_CRAWLER, MERCADO_LIVRE_LISTING_URL, MercadoLivreCrawler,
};
use mascate_kernel::{PlatformError, Timestamp};
use mascate_platform::{Build, Environment, Flag, FlagError, Flags};

/// The engine every crawler shares, so turns, memory and breakers hold per
/// host, and the sites Ofertas can search with it.
pub struct AppCrawlers {
    pub engine: Arc<Crawler>,
    pub sites: Vec<CrawledSite>,
}

impl Global for AppCrawlers {}

pub fn crawlers(cx: &App) -> Option<&AppCrawlers> {
    cx.try_global::<AppCrawlers>()
}

/// A site whose public pages a crawler searches.
#[derive(Clone)]
pub struct CrawledSite {
    pub name: &'static str,
    pub flag: Flag,
    /// Where its pages start.
    pub listing: String,
    build: fn(&str, Arc<Crawler>) -> Arc<dyn OfferSource>,
}

impl CrawledSite {
    /// Mercado Livre's search pages: the site's or, in a development build
    /// only, the URL in `MASCATE_MERCADO_LIVRE_LISTING_URL`, for a local fake
    /// server.
    pub fn mercado_livre(build: Build, environment: &Environment) -> Self {
        Self {
            name: "Mercado Livre",
            flag: MERCADO_LIVRE_CRAWLER,
            listing: crate::mercado_livre::local_fake(
                build,
                environment,
                "MASCATE_MERCADO_LIVRE_LISTING_URL",
            )
            .unwrap_or_else(|| MERCADO_LIVRE_LISTING_URL.to_owned()),
            build: |listing, engine| Arc::new(MercadoLivreCrawler::new(listing, engine)),
        }
    }
}

/// The site's crawler on `engine`, built only while its flag is on: off,
/// there is nothing to search with.
pub async fn source_if_on(
    flags: &Flags,
    site: &CrawledSite,
    engine: Arc<Crawler>,
) -> Result<Option<Arc<dyn OfferSource>>, FlagError> {
    Ok(flags
        .is_on(&site.flag)
        .await?
        .then(|| (site.build)(&site.listing, engine)))
}

/// Why the crawler is waiting, until `until`.
pub fn blocked_text(site: &CrawledSite, until: Timestamp) -> String {
    let until = until.with_timezone(&Local);
    let when = if until.date_naive() == Local::now().date_naive() {
        until.format("às %H:%M").to_string()
    } else {
        until.format("em %d/%m às %H:%M").to_string()
    };
    format!(
        "O {} bloqueou o crawler três vezes seguidas, então o app não pede nada ao site até \
         {when}. Bloqueio é a resposta do site: o app não tenta contornar.",
        site.name
    )
}

/// Why a search failed, as the owner reads it.
pub fn failure(site: &CrawledSite, error: &PlatformError) -> String {
    match error {
        PlatformError::Refused(why) => format!("Não li a busca: {why}."),
        PlatformError::NotFound => format!("O {} não tem página para essa busca.", site.name),
        PlatformError::RateLimited => format!(
            "Há buscas demais na fila para o {}; espere um pouco e busque de novo.",
            site.name
        ),
        PlatformError::Failed(why) => format!("Não consegui falar com o {}: {why}", site.name),
        PlatformError::NotConnected | PlatformError::Expired => {
            format!("O {} não respondeu à busca.", site.name)
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use mascate_catalog::ProductSource;
    use mascate_kernel::SystemClock;
    use mascate_kernel::testing::SequentialIds;
    use mascate_platform::{Database, Registry, migrate};

    use super::*;

    #[test]
    fn a_crawler_is_built_only_while_its_flag_is_on() {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let database = Arc::new(Database::open(&dir.path().join("m.db")).await.unwrap());
            migrate(&database, &SystemClock, &[mascate_platform::MIGRATIONS])
                .await
                .unwrap();
            let flags = Flags::new(
                database,
                Registry::new(&[mascate_integrations::FLAGS]).unwrap(),
                Arc::new(SystemClock),
                Arc::new(SequentialIds::default()),
            );
            let site = CrawledSite::mercado_livre(
                Build::Release,
                &(Box::new(|_: &str| None) as Environment),
            );
            let engine = Arc::new(Crawler::new("Mascate/test", Arc::new(SystemClock)));

            let off = source_if_on(&flags, &site, engine.clone()).await.unwrap();
            let request = flags.request_turn_on(site.flag.key).unwrap();
            flags.turn_on(request.confirm("fernando")).await.unwrap();
            let on = source_if_on(&flags, &site, engine).await.unwrap();

            assert!(off.is_none());
            assert_eq!(
                on.map(|source| source.source()),
                Some(ProductSource::MercadoLivreCrawler)
            );
            assert_eq!(site.listing, "https://lista.mercadolivre.com.br");
        });
    }
}
