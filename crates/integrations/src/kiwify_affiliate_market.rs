//! The Kiwify affiliate market reader (#222, ADR 0039 decisions 2, 3 and
//! 6): the Marketplace in Kiwify's dashboard, read through the shared
//! browser at the pace and with the stop of every market reader (see
//! `affiliate_market`): the commission, the price, the producer and whether
//! affiliation is automatic. Built only while its flag is on.
//!
//! Kiwify's help says the Marketplace lists only products with more than
//! R$ 5 thousand sold in their lifetime, ranked by recent revenue, refunds
//! and chargebacks, with filters by price, commission and category; it does
//! not say the cards show the revenue, nor name the categories, so the
//! reader reads the listing as Kiwify ranks it. The addresses and labels
//! come from that help, not from a real account: the phase 3 regression
//! pass (#116) records the real pages (see the fixtures' README).

use std::path::PathBuf;
use std::sync::Arc;

use mascate_affiliate::{
    AffiliatePlatform, FoundProspect, ProspectCategory, ProspectSource, SignalOrigin, Signals,
};
use mascate_kernel::{Clock, Pause, PlatformError, Timestamp};
use mascate_platform::{Flag, FlagKind, Phase};

use crate::affiliate_market::{
    AffiliateMarket, Card, MarketPage, MarketReader, Stopped, affiliation, count, is_code,
    labelled, labelled_all, market_script, money, named, percent,
};
use crate::affiliate_panel::PanelState;
use crate::browser::BrowserSession;

/// The Marketplace, where the owner logs in, unless a test points
/// elsewhere.
pub const MARKET_URL: &str = "https://dashboard.kiwify.com.br/marketplace";

/// The reader, off until the owner turns it on (ADR 0006).
pub const KIWIFY_AFFILIATE_MARKET: Flag = Flag {
    key: "integrations.kiwify_affiliate_market",
    name: "Leitura do marketplace de afiliados da Kiwify",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "Os termos de uso da Kiwify (versão de 29/10/2026) não citam robôs, mas os itens 14.1 \
             e 15.1 deixam a Kiwify suspender ou bloquear a conta por qualquer conduta contra os \
             termos e restringir o acesso a qualquer momento, por qualquer motivo, e o \
             marketplace só abre com o seu login. O app abre o marketplace no seu navegador, \
             num perfil só do Mascate em que você entrou à mão, só quando você clica: a primeira \
             página da lista por clique e no máximo 5 páginas, uma a cada 5 segundos ou mais, a \
             mesma página dentro de 1 hora vem da memória. Só lê o que a página mostra: não \
             clica, não pede afiliação e não resolve captcha; no primeiro login, verificação ou \
             bloqueio, para por 24 horas.",
    risk: "A Kiwify pode suspender ou bloquear a sua conta, com o saldo dentro dela (item \
           10.1.17).",
};

/// Listing pages one click reads: Kiwify's help shows no address for a
/// second page, and the reader never scrolls or clicks to draw one.
const LISTING_PAGES: usize = 1;

/// Kiwify's Marketplace, read through the shared browser.
pub struct KiwifyAffiliateMarket {
    reader: MarketReader,
}

impl KiwifyAffiliateMarket {
    /// The Marketplace at `market`, opened by `browser`, timed by `clock`
    /// and waiting with `pause`.
    pub fn new(
        market: &str,
        browser: Arc<BrowserSession>,
        clock: Arc<dyn Clock>,
        pause: Arc<dyn Pause>,
    ) -> Self {
        Self {
            reader: MarketReader::new(
                AffiliatePlatform::Kiwify,
                market,
                browser,
                clock,
                pause,
                // A card is a link to `/marketplace/{product}`.
                market_script(
                    r#"a[href*="/marketplace/"]"#,
                    r#"new URL(href).pathname.split("/marketplace/")[1].split("/")[0]"#,
                ),
                |page| labelled(&page.lines, COMMISSION).is_some(),
            ),
        }
    }

    /// Keeps a stop in `file` and starts with the one written there.
    pub fn with_stop_file(self, file: PathBuf) -> Self {
        Self {
            reader: self.reader.with_stop_file(file),
        }
    }
}

impl AffiliateMarket for KiwifyAffiliateMarket {
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

impl ProspectSource for KiwifyAffiliateMarket {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Kiwify
    }

    fn origin(&self) -> SignalOrigin {
        SignalOrigin::Market
    }

    /// None the app knows: Kiwify's help names no category.
    fn categories(&self) -> Result<Vec<ProspectCategory>, PlatformError> {
        Ok(Vec::new())
    }

    /// The first page of the listing, as Kiwify ranks it.
    fn prospects(
        &self,
        category: Option<&ProspectCategory>,
    ) -> Result<Vec<FoundProspect>, PlatformError> {
        if category.is_some() {
            return Err(PlatformError::Refused(
                "o Mascate ainda não conhece as categorias do marketplace da Kiwify".into(),
            ));
        }
        let market = self.reader.market().to_owned();
        self.reader
            .listing(LISTING_PAGES, |_| market.clone(), listed)
    }

    /// Reads the product's own page in the Marketplace, which `known` keeps
    /// the address of.
    fn details(&self, known: &FoundProspect) -> Result<FoundProspect, PlatformError> {
        let code = known
            .signals
            .affiliation_page
            .as_deref()
            .and_then(product_code)
            .ok_or_else(|| {
                PlatformError::Refused(
                    "esse produto ainda não tem o endereço do marketplace da Kiwify: garimpe no \
                     marketplace antes"
                        .into(),
                )
            })?;
        let page = self
            .reader
            .product(&format!("{}/{code}", self.reader.market()))?;
        Ok(details(&page, &code, known))
    }

    fn blocked_until(&self) -> Option<Timestamp> {
        self.stopped().map(|stopped| stopped.until)
    }
}

/// The address of a product in the Marketplace, where the owner asks for
/// the affiliation.
pub fn affiliation_page(code: &str) -> Option<String> {
    is_code(code).then(|| format!("{MARKET_URL}/{code}"))
}

/// The product's code in the Marketplace, from a link to it: the part of
/// the path after `/marketplace/`.
fn product_code(link: &str) -> Option<String> {
    let path = link.split(['?', '#']).next()?;
    let (_, rest) = path.split_once("/marketplace/")?;
    let code = rest.split('/').next()?;
    is_code(code).then(|| code.to_owned())
}

/// A card of the listing as a Prospect, under the Marketplace's code.
fn listed(card: &Card) -> Option<FoundProspect> {
    let code = product_code(&card.link)?;
    Some(FoundProspect {
        platform: AffiliatePlatform::Kiwify,
        name: card.name()?.to_owned(),
        signals: Signals {
            affiliation_page: affiliation_page(&code),
            ..signals(&card.lines)
        },
        key: code,
    })
}

/// A product's own page in the Marketplace, under its code.
fn details(page: &MarketPage, code: &str, known: &FoundProspect) -> FoundProspect {
    let name = Some(page.heading.trim())
        .filter(|heading| !heading.is_empty())
        .map_or_else(|| known.name.clone(), str::to_owned);
    FoundProspect {
        platform: AffiliatePlatform::Kiwify,
        key: known.key.clone(),
        name,
        signals: Signals {
            affiliation_page: affiliation_page(code),
            ..signals(&page.lines)
        },
    }
}

const COMMISSION: &[&str] = &["Comissão", "Sua comissão", "Ganhe até", "Você recebe"];
const PRICE: &[&str] = &["Preço", "Valor do produto"];
const WARRANTY: &[&str] = &["Garantia"];
const CREATOR: &[&str] = &["Produtor", "Produtora", "Autor", "Autora"];
/// Words of an affiliation the producer approves by hand, then of an
/// automatic one, as Kiwify's help names them.
const ON_REQUEST: [&str; 3] = [
    "APROVACAOMANUAL",
    "AFILIACAOMANUAL",
    "AGUARDAAPROVACAODOPRODUTOR",
];
const IMMEDIATE: [&str; 4] = [
    "APROVACAOAUTOMATICA",
    "AFILIACAOAUTOMATICA",
    "APROVACAOINSTANTANEA",
    "AFILIACAOINSTANTANEA",
];

/// The signals the Marketplace shows in `lines`, each left empty when it
/// is missing or does not read.
fn signals(lines: &[String]) -> Signals {
    let commission = labelled_all(lines, COMMISSION);
    Signals {
        commission: commission.iter().find_map(|text| money(text)),
        commission_rate: commission.iter().find_map(|text| percent(text)),
        price: labelled(lines, PRICE).and_then(money),
        warranty_days: labelled(lines, WARRANTY).and_then(count),
        creator: named(lines, CREATOR),
        affiliation: affiliation(lines, &ON_REQUEST, &IMMEDIATE),
        ..Signals::default()
    }
}
