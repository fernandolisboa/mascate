//! The Monetizze affiliate market reader (#224, ADR 0039 decisions 2, 3
//! and 6): the Vitrine de afiliação (the Loja of the old panel), read
//! through the shared browser at the pace and with the stop of every market
//! reader (see `affiliate_market`): the commission, the price, whether the
//! product is physical or digital, its category, its BARs (what ranks the
//! Hall da Fama), the producer and whether affiliation is open. Built only
//! while its flag is on.
//!
//! Monetizze's help says the Vitrine has the sections Hall da Fama
//! (ranked by BARs: sales, affiliates and their results, refunds and
//! chargebacks), Em Destaque (over R$ 250 thousand sold), Oportunidades
//! (R$ 50 to 250 thousand) and Mais Recentes, and a search by type,
//! commission, category and price; it gives no address for a section, a
//! page or a search, so the reader reads the Vitrine as it opens. The help
//! shows a product's page with its BARs, "Você ganha", category, format and
//! commission; the addresses and the card's link are not in it: the phase 3
//! regression pass (#116) records the real pages (see the fixtures'
//! README).

use std::path::PathBuf;
use std::sync::Arc;

use mascate_affiliate::{
    AffiliatePlatform, FoundProspect, ProductFormat, ProspectCategory, ProspectSource,
    SignalOrigin, Signals,
};
use mascate_kernel::{Clock, Pause, PlatformError, Timestamp};
use mascate_platform::{Flag, FlagKind, Phase};

use crate::affiliate_market::{
    AffiliateMarket, Card, MarketPage, MarketReader, Stopped, affiliation, count, folded,
    labelled_all, market_script, money, named, percent,
};
use crate::affiliate_panel::PanelState;
use crate::browser::BrowserSession;

/// The Vitrine, where the owner logs in, unless a test points elsewhere.
pub const MARKET_URL: &str = "https://app.monetizze.com.br/loja";

/// The reader, off until the owner turns it on (ADR 0006).
pub const MONETIZZE_AFFILIATE_MARKET: Flag = Flag {
    key: "integrations.monetizze_affiliate_market",
    name: "Leitura da Vitrine de afiliação da Monetizze",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "Os termos de uso da Monetizze (atualizados em 13/01/2026) não citam robôs, mas o \
             item 13.5 deixa a Monetizze rescindir os termos a qualquer momento por qualquer \
             descumprimento, o 13.8 cita como causa de bloqueio a tentativa de violar as medidas \
             de segurança da plataforma e o 14.1 veda usar o sistema, a interface e os softwares \
             dela sem autorização; a Vitrine só abre com o seu login. O app abre a Vitrine no \
             Edge ou no Chrome, num perfil só do Mascate em que você entrou à mão, só quando você \
             clica: a primeira página da Vitrine por clique e no máximo 5 páginas, uma a cada 5 \
             segundos ou mais, a mesma página dentro de 1 hora vem da memória. Só lê o que a \
             página mostra e guarda só o nome e os números de cada produto: não clica, não pede \
             afiliação e não resolve captcha; no primeiro login, verificação ou bloqueio, para \
             por 24 horas.",
    risk: "A Monetizze pode bloquear a sua conta e reter o saldo dela (itens 13.4 e 13.9 dos \
           termos).",
};

/// Listing pages one click reads: Monetizze's help shows no address for a
/// section, a second page or a search, and the reader never scrolls or
/// clicks to draw one.
const LISTING_PAGES: usize = 1;

/// Monetizze's Vitrine de afiliação, read through the shared browser.
pub struct MonetizzeAffiliateMarket {
    reader: MarketReader,
}

impl MonetizzeAffiliateMarket {
    /// The Vitrine at `market`, opened by `browser`, timed by `clock` and
    /// waiting with `pause`.
    pub fn new(
        market: &str,
        browser: Arc<BrowserSession>,
        clock: Arc<dyn Clock>,
        pause: Arc<dyn Pause>,
    ) -> Self {
        Self {
            reader: MarketReader::new(
                AffiliatePlatform::Monetizze,
                market,
                browser,
                clock,
                pause,
                // A card is a link under the Vitrine ending in the
                // product's number.
                market_script(
                    r#"a[href*="/loja/"]"#,
                    r#"new URL(href).pathname.split("/").filter((part) => /^\d+$/.test(part)).pop() || href"#,
                ),
                |page| !labelled_all(&page.lines, COMMISSION).is_empty(),
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

impl AffiliateMarket for MonetizzeAffiliateMarket {
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

impl ProspectSource for MonetizzeAffiliateMarket {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Monetizze
    }

    fn origin(&self) -> SignalOrigin {
        SignalOrigin::Market
    }

    /// None the app can open: the help names the sections and categories
    /// but no address for them.
    fn categories(&self) -> Result<Vec<ProspectCategory>, PlatformError> {
        Ok(Vec::new())
    }

    /// The first page of the Vitrine, as it opens.
    fn prospects(
        &self,
        category: Option<&ProspectCategory>,
    ) -> Result<Vec<FoundProspect>, PlatformError> {
        if category.is_some() {
            return Err(PlatformError::Refused(
                "o Mascate ainda não conhece o endereço das categorias da Vitrine da Monetizze"
                    .into(),
            ));
        }
        let market = self.reader.market().to_owned();
        self.reader.listing(
            LISTING_PAGES,
            |_| market.clone(),
            |card| listed(card, &market),
        )
    }

    /// Reads the product's own page in the Vitrine, which `known` keeps the
    /// address of; only an address under the Vitrine opens.
    fn details(&self, known: &FoundProspect) -> Result<FoundProspect, PlatformError> {
        let market = self.reader.market();
        let page = known
            .signals
            .affiliation_page
            .as_deref()
            .and_then(|link| product_page(link, market))
            .ok_or_else(|| {
                PlatformError::Refused(
                    "esse produto ainda não tem o endereço da Vitrine da Monetizze: garimpe na \
                     Vitrine antes"
                        .into(),
                )
            })?;
        let read = self.reader.product(&page.address)?;
        Ok(details(&read, page, known))
    }

    fn blocked_until(&self) -> Option<Timestamp> {
        self.stopped().map(|stopped| stopped.until)
    }
}

/// A product's page in the Vitrine and the product's number in it.
struct ProductPage {
    address: String,
    id: String,
}

/// `link` without its query when it is a product's page under `market`:
/// a path under the Vitrine whose last number is the product's.
fn product_page(link: &str, market: &str) -> Option<ProductPage> {
    let address = link.split(['?', '#']).next()?.trim_end_matches('/');
    let path = address.strip_prefix(market)?.strip_prefix('/')?;
    let parts: Vec<&str> = path.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || !part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
    {
        return None;
    }
    let id = parts
        .iter()
        .rev()
        .find(|part| part.chars().all(|c| c.is_ascii_digit()))?;
    Some(ProductPage {
        address: address.to_owned(),
        id: (*id).to_owned(),
    })
}

/// A card of the Vitrine as a Prospect, under the product's number.
fn listed(card: &Card, market: &str) -> Option<FoundProspect> {
    let page = product_page(&card.link, market)?;
    Some(FoundProspect {
        platform: AffiliatePlatform::Monetizze,
        name: card.name()?.to_owned(),
        signals: Signals {
            affiliation_page: Some(page.address),
            ..signals(&card.lines)
        },
        key: page.id,
    })
}

/// A product's own page in the Vitrine, under the key the app keeps.
fn details(read: &MarketPage, page: ProductPage, known: &FoundProspect) -> FoundProspect {
    let name = Some(read.heading.trim())
        .filter(|heading| !heading.is_empty())
        .map_or_else(|| known.name.clone(), str::to_owned);
    FoundProspect {
        platform: AffiliatePlatform::Monetizze,
        key: known.key.clone(),
        name,
        signals: Signals {
            affiliation_page: Some(page.address),
            ..signals(&read.lines)
        },
    }
}

/// What the affiliate earns: "Você ganha até R$ 493,00 por indicação",
/// "Sua Comissão: Adesão R$ 40,00", "Comissão dos afiliados: 60,00% do
/// valor líquido da venda".
const COMMISSION: &[&str] = &["Você ganha", "Sua comissão", "Comissão"];
const PRICE: &[&str] = &["Preço do produto", "Preço"];
const CATEGORY: &[&str] = &["Categoria"];
const FORMAT: &[&str] = &["Formato", "Tipo do produto"];
const CREATOR: &[&str] = &["Produtor", "Produtora"];
/// Words of an affiliation the producer approves, then of an open one, as
/// Monetizze's help and the producer's options name them.
const ON_REQUEST: [&str; 5] = [
    "AFILIACAOFECHADA",
    "AFILIACAORESTRITA",
    "MEDIANTEAPROVACAO",
    "APROVACAODOPRODUTOR",
    "SOMENTEAFILIADOSAPROVADOS",
];
const IMMEDIATE: [&str; 4] = [
    "AFILIACAOABERTA",
    "AFILIACAOIMEDIATA",
    "AFILIACAOAUTOMATICA",
    "TODOSPODEMPROMOVER",
];

/// The signals the Vitrine shows in `lines`, each left empty when it is
/// missing or does not read.
fn signals(lines: &[String]) -> Signals {
    let commission = labelled_all(lines, COMMISSION);
    Signals {
        commission: commission.iter().find_map(|text| money(text)),
        commission_rate: commission.iter().find_map(|text| percent(text)),
        price: labelled_all(lines, PRICE).into_iter().find_map(money),
        bars: bars(lines),
        format: named(lines, FORMAT).as_deref().and_then(format),
        category: named(lines, CATEGORY),
        creator: named(lines, CREATOR),
        affiliation: affiliation(lines, &ON_REQUEST, &IMMEDIATE),
        ..Signals::default()
    }
}

/// The BARs under the product's stars: a line that is only a number and
/// "Bars" ("100 Bars"), so a product named "Protein Bars" is not one.
fn bars(lines: &[String]) -> Option<u32> {
    lines.iter().find_map(|line| {
        let folded = folded(line);
        let unit = folded.trim_start_matches(|c: char| c.is_ascii_digit());
        (unit != folded && (unit == "BARS" || unit == "BAR"))
            .then(|| count(line))
            .flatten()
    })
}

/// "Produto Físico", "Produto digital".
fn format(text: &str) -> Option<ProductFormat> {
    let folded = folded(text);
    if folded.contains("FISICO") {
        Some(ProductFormat::Physical)
    } else if folded.contains("DIGITAL") {
        Some(ProductFormat::Digital)
    } else {
        None
    }
}
