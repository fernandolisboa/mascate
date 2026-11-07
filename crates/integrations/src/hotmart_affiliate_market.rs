//! The Hotmart affiliate market reader (#221, ADR 0039 decisions 2 and 3):
//! the Mercado de Afiliação, read through the shared browser at the pace
//! and with the stop of every market reader (see `affiliate_market`): the
//! temperature, the commission, the Blueprint, the satisfaction, the price
//! and whether affiliation is immediate. Built only while its flag is on.
//! The page addresses and labels come from Hotmart's public help, not from
//! a real account: the phase 3 regression pass (#116) records the real
//! pages (see the fixtures' README).

use std::path::PathBuf;
use std::sync::Arc;

use mascate_affiliate::{
    AffiliatePlatform, FoundProspect, ProspectCategory, ProspectSource, SignalOrigin, Signals,
};
use mascate_kernel::{Clock, Pause, PlatformError, Timestamp};
use mascate_platform::{Flag, FlagKind, Phase};

use crate::affiliate_market::{
    AffiliateMarket, Card, MarketPage, MarketReader, PAGES_PER_CLICK, Stopped, affiliation,
    degrees, first_number, is_code, labelled, labelled_all, market_script, money, named, percent,
    query_value, satisfaction,
};
use crate::affiliate_panel::PanelState;
use crate::browser::BrowserSession;
use crate::hotmart_crawler::hotmart_categories;

/// The Mercado de Afiliação, where the owner logs in, unless a test points
/// elsewhere.
pub const MARKET_URL: &str = "https://app.hotmart.com/market";

/// The reader, off until the owner turns it on (ADR 0006).
pub const HOTMART_AFFILIATE_MARKET: Flag = Flag {
    key: "integrations.hotmart_affiliate_market",
    name: "Leitura do mercado de afiliação da Hotmart",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "Os termos de uso da Hotmart, item 3.5(m), proíbem sistemas automatizados, robôs e \
             scripts, e o mercado de afiliação só abre com o seu login. O app abre o mercado no \
             Edge ou no Chrome, num perfil só do Mascate em que você entrou à mão, só quando você \
             clica: no máximo 5 páginas por clique, uma a cada 5 segundos ou mais, a mesma \
             página dentro de 1 hora vem da memória. Só lê o que a página mostra: não clica, \
             não pede afiliação e não resolve captcha; no primeiro login, verificação ou \
             bloqueio, para por 24 horas.",
    risk: "A Hotmart pode suspender ou encerrar a sua conta de afiliado, com as comissões \
           ainda não pagas.",
};

/// Hotmart's Mercado de Afiliação, read through the shared browser.
pub struct HotmartAffiliateMarket {
    reader: MarketReader,
}

impl HotmartAffiliateMarket {
    /// The market at `market`, opened by `browser`, timed by `clock` and
    /// waiting with `pause`.
    pub fn new(
        market: &str,
        browser: Arc<BrowserSession>,
        clock: Arc<dyn Clock>,
        pause: Arc<dyn Pause>,
    ) -> Self {
        Self {
            reader: MarketReader::new(
                AffiliatePlatform::Hotmart,
                market,
                browser,
                clock,
                pause,
                // A card is a link with `productUcode=`.
                market_script(
                    r#"a[href*="productUcode="]"#,
                    r#"new URL(href).searchParams.get("productUcode")"#,
                ),
                |page| labelled(&page.lines, TEMPERATURE).is_some(),
            ),
        }
    }

    /// Keeps a stop in `file` and starts with the one written there.
    pub fn with_stop_file(self, file: PathBuf) -> Self {
        Self {
            reader: self.reader.with_stop_file(file),
        }
    }

    /// The listing of `category`, or of the whole market, at `page`.
    fn listing_url(
        &self,
        category: Option<&ProspectCategory>,
    ) -> Result<impl Fn(usize) -> String, PlatformError> {
        let market = self.reader.market().to_owned();
        let category = match category {
            None => None,
            Some(category) => Some(
                hotmart_categories()
                    .into_iter()
                    .find(|known| known.id == category.id)
                    .ok_or_else(|| {
                        PlatformError::Refused(
                            "isso não é uma categoria do mercado de afiliação da Hotmart".into(),
                        )
                    })?
                    .id,
            ),
        };
        Ok(move |page: usize| match &category {
            None => format!("{market}?page={page}"),
            Some(id) => format!("{market}?category={id}&page={page}"),
        })
    }
}

impl AffiliateMarket for HotmartAffiliateMarket {
    fn state(&self) -> PanelState {
        self.reader.state()
    }

    fn stopped(&self) -> Option<Stopped> {
        self.reader.stopped()
    }

    fn open_login(&self) -> Result<(), PlatformError> {
        self.reader.open_login()
    }
}

impl ProspectSource for HotmartAffiliateMarket {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Hotmart
    }

    fn origin(&self) -> SignalOrigin {
        SignalOrigin::Market
    }

    fn categories(&self) -> Result<Vec<ProspectCategory>, PlatformError> {
        Ok(hotmart_categories())
    }

    /// The listing's pages in order, up to the click's pages, until one
    /// shows nothing new.
    fn prospects(
        &self,
        category: Option<&ProspectCategory>,
    ) -> Result<Vec<FoundProspect>, PlatformError> {
        let url = self.listing_url(category)?;
        self.reader.listing(PAGES_PER_CLICK, url, listed)
    }

    /// Reads the product's own page in the market, which `known` keeps the
    /// address of: what the card shows and the producer.
    fn details(&self, known: &FoundProspect) -> Result<FoundProspect, PlatformError> {
        let ucode = known
            .signals
            .affiliation_page
            .as_deref()
            .and_then(product_ucode)
            .ok_or_else(|| {
                PlatformError::Refused(
                    "esse produto ainda não tem o endereço do mercado de afiliação da Hotmart: \
                     garimpe no mercado ou leia a página do produto na vitrine antes"
                        .into(),
                )
            })?;
        let page = self.reader.product(&format!(
            "{}/details?productUcode={ucode}",
            self.reader.market()
        ))?;
        Ok(details(&page, &ucode, known))
    }

    fn blocked_until(&self) -> Option<Timestamp> {
        self.stopped().map(|stopped| stopped.until)
    }
}

/// The address of a product in the market, where the owner asks for the
/// affiliation: one form for every reader, so the vitrine and the market
/// name the same product the same way.
pub fn affiliation_page(ucode: &str) -> Option<String> {
    is_code(ucode).then(|| format!("{MARKET_URL}/details?productUcode={ucode}"))
}

/// The product's code in the market (`productUcode`), from a link to it.
fn product_ucode(link: &str) -> Option<String> {
    query_value(link, "productUcode").filter(|ucode| is_code(ucode))
}

/// A card of the listing as a Prospect: under Hotmart's product id when the
/// card shows it, as the vitrine keys it, else under the market's code.
fn listed(card: &Card) -> Option<FoundProspect> {
    let ucode = product_ucode(&card.link)?;
    let name = card.name()?;
    Some(FoundProspect {
        platform: AffiliatePlatform::Hotmart,
        key: product_id(&card.links, &card.lines).unwrap_or_else(|| ucode.clone()),
        name: name.to_owned(),
        signals: Signals {
            affiliation_page: affiliation_page(&ucode),
            ..signals(&card.lines)
        },
    })
}

/// A product's own page in the market, under `known`'s key unless the page
/// shows Hotmart's product id.
fn details(page: &MarketPage, ucode: &str, known: &FoundProspect) -> FoundProspect {
    let name = Some(page.heading.trim())
        .filter(|heading| !heading.is_empty())
        .map_or_else(|| known.name.clone(), str::to_owned);
    FoundProspect {
        platform: AffiliatePlatform::Hotmart,
        key: product_id(&page.links, &page.lines).unwrap_or_else(|| known.key.clone()),
        name,
        signals: Signals {
            affiliation_page: affiliation_page(ucode),
            ..signals(&page.lines)
        },
    }
}

const TEMPERATURE: &[&str] = &["Temperatura"];
const COMMISSION: &[&str] = &[
    "Comissão",
    "Sua comissão",
    "Ganhe até",
    "Você ganha",
    "Receba até",
];
const PRICE: &[&str] = &["Preço", "Valor do produto"];
const BLUEPRINT: &[&str] = &["Blueprint"];
const SATISFACTION: &[&str] = &["Satisfação"];
const CREATOR: &[&str] = &["Produtor", "Produtora", "Autor", "Autora"];
const PRODUCT_ID: &[&str] = &["ID do produto", "Código do produto"];
/// Words of an affiliation the producer approves, then of one at once.
const ON_REQUEST: [&str; 4] = [
    "AFILIACAOCOMAPROVACAO",
    "AFILIACAOMEDIANTEAPROVACAO",
    "SOLICITARAFILIACAO",
    "AGUARDAAPROVACAO",
];
const IMMEDIATE: [&str; 2] = ["AFILIACAOIMEDIATA", "AFILIESEAGORA"];

/// The signals the market shows in `lines`, each left empty when it is
/// missing or does not read.
fn signals(lines: &[String]) -> Signals {
    let commission = labelled_all(lines, COMMISSION);
    Signals {
        temperature: labelled(lines, TEMPERATURE).and_then(degrees),
        commission: commission.iter().find_map(|text| money(text)),
        commission_rate: commission.iter().find_map(|text| percent(text)),
        price: labelled(lines, PRICE).and_then(money),
        blueprint: labelled(lines, BLUEPRINT).and_then(percent),
        satisfaction: labelled(lines, SATISFACTION).and_then(satisfaction),
        creator: named(lines, CREATOR),
        affiliation: affiliation(lines, &ON_REQUEST, &IMMEDIATE),
        ..Signals::default()
    }
}

/// Hotmart's product id, from a link (`productId=`) or a labelled line.
fn product_id(links: &[String], lines: &[String]) -> Option<String> {
    let is_id = |text: &str| !text.is_empty() && text.chars().all(|c| c.is_ascii_digit());
    links
        .iter()
        .find_map(|link| query_value(link, "productId").filter(|id| is_id(id)))
        .or_else(|| {
            labelled(lines, PRODUCT_ID).and_then(|text| {
                let id = first_number(text)?;
                is_id(&id).then_some(id)
            })
        })
}
