//! The Eduzz affiliate market reader (#223, ADR 0039 decisions 2, 3 and
//! 6): the Vitrine in MyEduzz, read through the shared browser at the pace
//! and with the stop of every market reader (see `affiliate_market`): the
//! commission per sale, the price, the rating, the warranty, the producer
//! and whether affiliation is automatic. Built only while its flag is on.
//!
//! Eduzz's help says the Vitrine has the tabs Mais Vendidos (the most sold
//! by affiliates in the last 30 days), Novidades and Produto Físico, a
//! search and filters; it names no category and gives no address for a
//! tab or a page, so the reader reads the Vitrine as it opens. MyEduzz's
//! public code shows the Vitrine at `my2.eduzz.com/showcase` and a product
//! at `/showcase/product/details/{id}`; the labels follow its product
//! card, not a real account: the phase 3 regression pass (#116) records
//! the real pages (see the fixtures' README).

use std::path::PathBuf;
use std::sync::Arc;

use mascate_affiliate::{
    AffiliatePlatform, FoundProspect, ProspectCategory, ProspectSource, SignalOrigin, Signals,
};
use mascate_kernel::{Clock, Pause, PlatformError, Timestamp, parse_amount};
use mascate_platform::{Flag, FlagKind, Phase};
use rust_decimal::Decimal;

use crate::affiliate_market::{
    AffiliateMarket, Card, MarketPage, MarketReader, Stopped, affiliation, count, first_number,
    folded, is_code, labelled, market_script, money, named, percent,
};
use crate::affiliate_panel::PanelState;
use crate::browser::BrowserSession;

/// The Vitrine, where the owner logs in, unless a test points elsewhere.
pub const MARKET_URL: &str = "https://my2.eduzz.com/showcase";

/// The reader, off until the owner turns it on (ADR 0006).
pub const EDUZZ_AFFILIATE_MARKET: Flag = Flag {
    key: "integrations.eduzz_affiliate_market",
    name: "Leitura da Vitrine de afiliados da Eduzz",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "Os termos de uso da Eduzz (lidos em 07/11/2026, sem data de versão) não citam \
             robôs, mas a parte Penalidades deixa a Eduzz suspender o acesso ou bloquear a \
             conta por qualquer conduta contra os termos, a seu critério, e a parte Direitos \
             Autorais proíbe copiar ou guardar o conteúdo da plataforma sem autorização; a \
             Vitrine só abre com o seu login. O app abre a Vitrine no seu navegador, num \
             perfil só do Mascate em que você entrou à mão, só quando você clica: a primeira \
             página da Vitrine por clique e no máximo 5 páginas, uma a cada 5 segundos ou mais, \
             a mesma página dentro de 1 hora vem da memória. Só lê o que a página mostra e \
             guarda só o nome e os números de cada produto: não clica, não pede afiliação e não \
             resolve captcha; no primeiro login, verificação ou bloqueio, para por 24 horas.",
    risk: "A Eduzz pode suspender ou bloquear a sua conta e reter o saldo dela (partes \
           Penalidades e Rescisão dos termos).",
};

/// Listing pages one click reads: Eduzz's help shows no address for a
/// second page or a tab, and the reader never scrolls or clicks to draw
/// one.
const LISTING_PAGES: usize = 1;

/// Where a product's own page sits under the Vitrine.
const PRODUCT_PATH: &str = "/product/details/";

/// Eduzz's Vitrine, read through the shared browser.
pub struct EduzzAffiliateMarket {
    reader: MarketReader,
}

impl EduzzAffiliateMarket {
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
                AffiliatePlatform::Eduzz,
                market,
                browser,
                clock,
                pause,
                // A card is a link to `/showcase/product/details/{id}`.
                market_script(
                    r#"a[href*="/product/details/"]"#,
                    r#"new URL(href).pathname.split("/product/details/")[1].split("/")[0]"#,
                ),
                |page| !commission_texts(&page.lines).is_empty(),
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

impl AffiliateMarket for EduzzAffiliateMarket {
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

impl ProspectSource for EduzzAffiliateMarket {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Eduzz
    }

    fn origin(&self) -> SignalOrigin {
        SignalOrigin::Market
    }

    /// None the app knows: Eduzz's help names no category.
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
                "o Mascate ainda não conhece as categorias da Vitrine da Eduzz".into(),
            ));
        }
        let market = self.reader.market().to_owned();
        self.reader
            .listing(LISTING_PAGES, |_| market.clone(), listed)
    }

    /// Reads the product's own page in the Vitrine, which `known` keeps the
    /// address of.
    fn details(&self, known: &FoundProspect) -> Result<FoundProspect, PlatformError> {
        let id = known
            .signals
            .affiliation_page
            .as_deref()
            .and_then(product_id)
            .ok_or_else(|| {
                PlatformError::Refused(
                    "esse produto ainda não tem o endereço da Vitrine da Eduzz: garimpe na \
                     Vitrine antes"
                        .into(),
                )
            })?;
        let page = self
            .reader
            .product(&format!("{}{PRODUCT_PATH}{id}", self.reader.market()))?;
        Ok(details(&page, &id, known))
    }

    fn blocked_until(&self) -> Option<Timestamp> {
        self.stopped().map(|stopped| stopped.until)
    }
}

/// The address of a product in the Vitrine, where the owner asks for the
/// affiliation.
pub fn affiliation_page(id: &str) -> Option<String> {
    is_code(id).then(|| format!("{MARKET_URL}{PRODUCT_PATH}{id}"))
}

/// The product's id in the Vitrine, from a link to it: the part of the
/// path after `/showcase/product/details/`.
fn product_id(link: &str) -> Option<String> {
    let path = link.split(['?', '#']).next()?;
    let (_, rest) = path.split_once(&format!("/showcase{PRODUCT_PATH}"))?;
    let id = rest.split('/').next()?;
    is_code(id).then(|| id.to_owned())
}

/// A card of the Vitrine as a Prospect, under the product's id.
fn listed(card: &Card) -> Option<FoundProspect> {
    let id = product_id(&card.link)?;
    Some(FoundProspect {
        platform: AffiliatePlatform::Eduzz,
        name: card.name()?.to_owned(),
        signals: Signals {
            affiliation_page: affiliation_page(&id),
            ..signals(&card.lines)
        },
        key: id,
    })
}

/// A product's own page in the Vitrine, under its id.
fn details(page: &MarketPage, id: &str, known: &FoundProspect) -> FoundProspect {
    let name = Some(page.heading.trim())
        .filter(|heading| !heading.is_empty())
        .map_or_else(|| known.name.clone(), str::to_owned);
    FoundProspect {
        platform: AffiliatePlatform::Eduzz,
        key: known.key.clone(),
        name,
        signals: Signals {
            affiliation_page: affiliation_page(id),
            ..signals(&page.lines)
        },
    }
}

const COMMISSION: &[&str] = &["Comissão", "Comissão por venda", "Você ganha"];
const PRICE: &[&str] = &["Preço", "Valor do produto"];
const RATING: &[&str] = &["Avaliação", "Nota"];
const WARRANTY: &[&str] = &["Garantia"];
const CREATOR: &[&str] = &["Produtor", "Produtora"];
/// Words of an affiliation the producer approves by hand, then of an
/// automatic one, as Eduzz's help names them.
const ON_REQUEST: [&str; 4] = [
    "AUTOMATICAINATIVA",
    "AFILIACAOMANUAL",
    "AFILIACAOCOMAPROVACAO",
    "APROVACAODOPRODUTOR",
];
const IMMEDIATE: [&str; 2] = ["AFILIACAOAUTOMATICA", "AFILIACAOIMEDIATA"];
/// The highest rating, in stars.
const MAX_STARS: Decimal = Decimal::from_parts(5, 0, 0, false, 0);

/// The signals the Vitrine shows in `lines`, each left empty when it is
/// missing or does not read.
fn signals(lines: &[String]) -> Signals {
    let commission = commission_texts(lines);
    let rating = labelled(lines, RATING);
    Signals {
        commission: commission
            .iter()
            .filter(|text| !recurring(text))
            .chain(commission.iter().filter(|text| recurring(text)))
            .find_map(|text| money(text)),
        commission_rate: commission.iter().find_map(|text| percent(text)),
        price: labelled(lines, PRICE).and_then(money),
        rating: rating.and_then(stars),
        reviews: rating.and_then(reviews),
        warranty_days: labelled(lines, WARRANTY).and_then(count),
        creator: named(lines, CREATOR),
        affiliation: affiliation(lines, &ON_REQUEST, &IMMEDIATE),
        ..Signals::default()
    }
}

/// The values under the commission's label: on its own line ("Comissão:
/// R$ 98,50"), and then every amount or rate on the lines right after it,
/// since a product with a subscription shows the recurring commission
/// ("R$ 30,00 rec.") above the one of the sale.
fn commission_texts(lines: &[String]) -> Vec<&str> {
    let labels: Vec<String> = COMMISSION.iter().map(|label| folded(label)).collect();
    let is_value = |line: &str| line.contains("R$") || line.contains('%');
    let mut texts = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !labels.iter().any(|label| folded(line).starts_with(label)) {
            continue;
        }
        if is_value(line) {
            texts.push(line.as_str());
        }
        texts.extend(
            lines[index + 1..]
                .iter()
                .map(String::as_str)
                .take_while(|line| is_value(line)),
        );
    }
    texts
}

/// Whether `text` is the commission of each invoice of a subscription,
/// which Eduzz marks "rec.".
fn recurring(text: &str) -> bool {
    let text = text.trim().trim_end_matches('.').to_lowercase();
    text.ends_with(" rec") || text.contains("recorrente")
}

/// The stars of a rating, from 0 to 5: "4,6 (23 avaliações)".
fn stars(text: &str) -> Option<Decimal> {
    parse_amount(&first_number(text)?)
        .filter(|stars| !stars.is_sign_negative() && *stars <= MAX_STARS)
}

/// How many ratings: the count in brackets after the stars.
fn reviews(text: &str) -> Option<u32> {
    count(text.split_once('(')?.1)
}
