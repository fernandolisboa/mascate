//! The crawlers as the screens use them (#152, ADR 0035): the one engine
//! the app shares, the sites it can search, each built only while its
//! Restricted Feature is on, and how their failures read in Portuguese.

use std::sync::Arc;

use chrono::Local;
use gpui_kit::{App, Global};
use mascate_catalog::OfferSource;
use mascate_integrations::{
    AMAZON_CRAWLER, AMAZON_SEARCH_URL, AmazonCrawler, Crawler, MAGALU_CRAWLER, MAGALU_SEARCH_URL,
    MERCADO_LIVRE_CRAWLER, MERCADO_LIVRE_LISTING_URL, MagaluCrawler, MercadoLivreCrawler,
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
    /// The article the name takes in Portuguese: "o" or "a".
    article: &'static str,
    pub flag: Flag,
    /// Where its pages start.
    pub listing: String,
    build: fn(&str, Arc<Crawler>) -> Arc<dyn OfferSource>,
}

impl CrawledSite {
    /// Every site, in the order Ofertas shows them: each the site's or, in a
    /// development build only, the URL in its variable, for a local fake
    /// server.
    pub fn all(build: Build, environment: &Environment) -> Vec<Self> {
        vec![
            Self::mercado_livre(build, environment),
            Self::amazon(build, environment),
            Self::magalu(build, environment),
        ]
    }

    /// Mercado Livre's search pages, or `MASCATE_MERCADO_LIVRE_LISTING_URL`.
    pub fn mercado_livre(build: Build, environment: &Environment) -> Self {
        Self {
            name: "Mercado Livre",
            article: "o",
            flag: MERCADO_LIVRE_CRAWLER,
            listing: local_or(
                build,
                environment,
                "MASCATE_MERCADO_LIVRE_LISTING_URL",
                MERCADO_LIVRE_LISTING_URL,
            ),
            build: |listing, engine| Arc::new(MercadoLivreCrawler::new(listing, engine)),
        }
    }

    /// Amazon's site, or `MASCATE_AMAZON_SEARCH_URL`.
    pub fn amazon(build: Build, environment: &Environment) -> Self {
        Self {
            name: "Amazon",
            article: "a",
            flag: AMAZON_CRAWLER,
            listing: local_or(
                build,
                environment,
                "MASCATE_AMAZON_SEARCH_URL",
                AMAZON_SEARCH_URL,
            ),
            build: |site, engine| Arc::new(AmazonCrawler::new(site, engine)),
        }
    }

    /// Magalu's site, or `MASCATE_MAGALU_SEARCH_URL`.
    pub fn magalu(build: Build, environment: &Environment) -> Self {
        Self {
            name: "Magalu",
            article: "a",
            flag: MAGALU_CRAWLER,
            listing: local_or(
                build,
                environment,
                "MASCATE_MAGALU_SEARCH_URL",
                MAGALU_SEARCH_URL,
            ),
            build: |site, engine| Arc::new(MagaluCrawler::new(site, engine)),
        }
    }

    /// "o Mercado Livre", "a Amazon".
    pub fn the(&self) -> String {
        format!("{} {}", self.article, self.name)
    }

    /// "O Mercado Livre", "A Amazon", to start a sentence.
    pub fn the_capitalized(&self) -> String {
        format!("{} {}", self.article.to_uppercase(), self.name)
    }

    /// "do Mercado Livre", "da Amazon".
    pub fn of(&self) -> String {
        format!("d{} {}", self.article, self.name)
    }

    /// "no Mercado Livre", "na Amazon".
    pub fn on(&self) -> String {
        format!("n{} {}", self.article, self.name)
    }
}

fn local_or(build: Build, environment: &Environment, variable: &str, site: &str) -> String {
    crate::mercado_livre::local_fake(build, environment, variable)
        .unwrap_or_else(|| site.to_owned())
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
        until.format("as %H:%M").to_string()
    } else {
        until.format("%d/%m, às %H:%M").to_string()
    };
    format!(
        "{} bloqueou o crawler três vezes seguidas, então o app não pede nada ao site até \
         {when}. Bloqueio é a resposta do site: o app não tenta contornar.",
        site.the_capitalized()
    )
}

/// Why a search failed, as the owner reads it.
pub fn failure(site: &CrawledSite, error: &PlatformError) -> String {
    match error {
        PlatformError::Refused(why) => format!("Não li a busca: {why}."),
        PlatformError::NotFound => {
            format!("{} não tem página para essa busca.", site.the_capitalized())
        }
        PlatformError::RateLimited => format!(
            "Há buscas demais na fila para {}; espere um pouco e busque de novo.",
            site.the()
        ),
        PlatformError::Failed(why) => format!("Não consegui falar com {}: {why}", site.the()),
        PlatformError::NotConnected | PlatformError::Expired => {
            format!("{} não respondeu à busca.", site.the_capitalized())
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

    #[test]
    fn ofertas_offers_mercado_livre_amazon_and_magalu_each_behind_its_own_flag() {
        let release = CrawledSite::all(Build::Release, &(Box::new(|_: &str| None) as Environment));
        let development = CrawledSite::all(
            Build::Development,
            &(Box::new(|name: &str| {
                (name == "MASCATE_AMAZON_SEARCH_URL").then(|| "http://127.0.0.1:8091".to_owned())
            }) as Environment),
        );

        let sites: Vec<(&str, &str, &str)> = release
            .iter()
            .map(|site| (site.name, site.flag.key, site.listing.as_str()))
            .collect();
        assert_eq!(
            sites,
            [
                (
                    "Mercado Livre",
                    "integrations.mercado_livre_crawler",
                    "https://lista.mercadolivre.com.br"
                ),
                (
                    "Amazon",
                    "integrations.amazon_crawler",
                    "https://www.amazon.com.br"
                ),
                (
                    "Magalu",
                    "integrations.magalu_crawler",
                    "https://www.magazineluiza.com.br"
                ),
            ]
        );
        assert_eq!(development[1].listing, "http://127.0.0.1:8091");
        assert_eq!(development[2].listing, "https://www.magazineluiza.com.br");
        assert_eq!(
            (
                release[1].the_capitalized(),
                release[1].of(),
                release[2].on()
            ),
            ("A Amazon".into(), "da Amazon".into(), "na Magalu".into())
        );
        assert_eq!(release[0].the(), "o Mercado Livre");
    }

    #[test]
    fn each_new_crawler_is_built_only_while_its_own_flag_is_on() {
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
            let environment = Box::new(|_: &str| None) as Environment;
            let (amazon, magalu) = (
                CrawledSite::amazon(Build::Release, &environment),
                CrawledSite::magalu(Build::Release, &environment),
            );
            let engine = Arc::new(Crawler::new("Mascate/test", Arc::new(SystemClock)));

            let request = flags.request_turn_on(amazon.flag.key).unwrap();
            assert_eq!(request.flag().risk, AMAZON_CRAWLER.risk);
            flags.turn_on(request.confirm("fernando")).await.unwrap();
            let amazon_on = source_if_on(&flags, &amazon, engine.clone()).await.unwrap();
            let magalu_off = source_if_on(&flags, &magalu, engine).await.unwrap();

            assert_eq!(
                amazon_on.map(|source| source.source()),
                Some(ProductSource::AmazonCrawler)
            );
            assert!(magalu_off.is_none());
        });
    }
}
