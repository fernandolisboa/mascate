//! Garimpo (#219, ADR 0039): affiliate Prospects ranked by their score,
//! each part with its reason, the apostas in a filter of their own, the
//! owner's own entries from any Platform's market, the Hotmart vitrine
//! (#220) and the logged-in affiliate markets of Hotmart (#221), Kiwify
//! (#222), Eduzz (#223) and Monetizze (#224), each read on the owner's
//! click while its flag is on, and the way from a Prospect to the Passo a
//! passo or to the bin.
//! The ranking and every rule are the Affiliate's; this screen only calls
//! it.

use std::sync::Arc;

use chrono::NaiveDate;
use chrono::Utc;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, EventEmitter, SharedString, Window, div, px};
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliatePlatform, Affiliation, FoundProspect, Program, Prospect,
    ProspectCategory, ProspectSource, ProspectsRead, SignalOrigin, Signals, Support,
    prospect_key_of,
};
use mascate_integrations::{
    AffiliateMarket, HOTMART_CRAWLER, MarketStop, MarketStopped, PanelState,
};
use mascate_kernel::PlatformError;
use mascate_kernel::{Currency, Money, Percentage, RecordId, parse_amount};
use rust_decimal::Decimal;

use crate::affiliate::{affiliate, failure};
use crate::affiliate_markets::{MARKET_PLATFORMS, Market, market};
use crate::affiliate_panels::{MarketTexts, panels};
use crate::appearance::look;
use crate::catalog::NO_DATABASE;
use crate::crawlers::{self, crawlers};
use crate::forms::{Outcome, amount_text, day_text, input, notice, parse_day, percent_text};
use crate::kit;
use crate::layout;
use crate::parts::ScreenParts;
use crate::preferences;
use crate::restricted::AppFlags;

/// The vitrine the app reads, as its failures name it.
const VITRINE: &str = "a Hotmart";

/// "Quero divulgar" gave this Program: the Passo a passo opens on it.
pub struct OpenGuidedFlow(pub Program);

/// What the owner typed in "Cadastrar à mão", as text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Typed {
    pub link: String,
    pub name: String,
    pub price: String,
    pub commission: String,
    pub commission_rate: String,
    pub temperature: String,
    pub blueprint: String,
    pub satisfaction: String,
    pub rating: String,
    pub reviews: String,
    pub students: String,
    pub launched: String,
    pub classes: String,
    pub hours: String,
    pub creator: String,
    pub creator_since: String,
    pub certificate: Option<bool>,
    pub video: Option<bool>,
    pub creator_verified: Option<bool>,
    pub affiliation: Option<Affiliation>,
    pub support: Option<Support>,
}

/// The Prospect `typed` describes on `platform`, keyed by its link unless
/// it completes `known`, which keeps its own key and gets only the signals
/// the owner changed. The error names the field to fix.
pub fn typed_prospect(
    platform: AffiliatePlatform,
    typed: &Typed,
    known: Option<&Prospect>,
) -> Result<FoundProspect, String> {
    let link = typed.link.trim();
    let key = match known {
        Some(known) => known.key.clone(),
        None => prospect_key_of(link).ok_or(
            "Cole o endereço da página do produto, começando com https://, para o app não \
             cadastrar o mesmo produto duas vezes.",
        )?,
    };
    if typed.name.trim().is_empty() {
        return Err("Digite o nome do produto.".into());
    }
    let money = |text: &str, what: &str| {
        optional(text, |text| {
            parse_amount(text)
                .filter(|amount| !amount.is_sign_negative())
                .map(|amount| Money::new(amount, Currency::Brl))
        })
        .map_err(|_| format!("Digite {what} como 97,00."))
    };
    let rate = |text: &str, what: &str| {
        optional(text, Percentage::parse)
            .map_err(|_| format!("Digite {what} como porcentagem de 0 a 100, ex.: 40."))
    };
    let count = |text: &str, what: &str| {
        optional(text, |text| text.replace('.', "").parse::<u32>().ok())
            .map_err(|_| format!("Digite {what} como número inteiro, ex.: 1200."))
    };
    let decimal = |text: &str, what: &str| {
        optional(text, |text| {
            parse_amount(text).filter(|d| !d.is_sign_negative())
        })
        .map_err(|_| format!("Digite {what} como número, ex.: 4,8."))
    };
    let day = |text: &str, what: &str| {
        optional(text, parse_day_or_year).map_err(|_| format!("Digite {what} como 07/11/2026."))
    };
    let page = (!link.is_empty()).then(|| link.to_owned());
    let signals = Signals {
        price: money(&typed.price, "o preço")?,
        commission: money(&typed.commission, "a comissão em R$")?,
        commission_rate: rate(&typed.commission_rate, "a comissão em %")?,
        temperature: count(&typed.temperature, "a temperatura")?,
        blueprint: rate(&typed.blueprint, "o Blueprint")?,
        satisfaction: rate(&typed.satisfaction, "a satisfação")?,
        rating: decimal(&typed.rating, "a nota")?,
        reviews: count(&typed.reviews, "as avaliações")?,
        students: count(&typed.students, "os alunos")?,
        launched: day(&typed.launched, "quando entrou")?,
        classes: count(&typed.classes, "as aulas")?,
        hours: decimal(&typed.hours, "as horas")?,
        creator: (!typed.creator.trim().is_empty()).then(|| typed.creator.trim().to_owned()),
        creator_since: day(&typed.creator_since, "desde quando o produtor está lá")?,
        certificate: typed.certificate,
        video: typed.video,
        creator_verified: typed.creator_verified,
        affiliation: typed.affiliation,
        support: typed.support,
        product_page: page,
        ..Signals::default()
    };
    Ok(FoundProspect {
        platform,
        key,
        name: typed.name.trim().to_owned(),
        signals: match known {
            Some(known) => signals.changed_from(&known.signals),
            None => signals,
        },
    })
}

/// What the owner typed for `prospect`, to complete it.
pub fn typed_from(prospect: &Prospect) -> Typed {
    let signals = &prospect.signals;
    let money = |money: Option<Money>| money.map(amount_text).unwrap_or_default();
    let rate = |rate: Option<Percentage>| rate.map(percent_text).unwrap_or_default();
    let count = |count: Option<u32>| count.map(|c| c.to_string()).unwrap_or_default();
    let decimal = |value: Option<Decimal>| {
        value
            .map(|value| value.normalize().to_string().replace('.', ","))
            .unwrap_or_default()
    };
    let day = |day: Option<NaiveDate>| day.map(day_text).unwrap_or_default();
    Typed {
        link: signals.product_page.clone().unwrap_or_default(),
        name: prospect.name.clone(),
        price: money(signals.price),
        commission: money(signals.commission),
        commission_rate: rate(signals.commission_rate),
        temperature: count(signals.temperature),
        blueprint: rate(signals.blueprint),
        satisfaction: rate(signals.satisfaction),
        rating: decimal(signals.rating),
        reviews: count(signals.reviews),
        students: count(signals.students),
        launched: day(signals.launched),
        classes: count(signals.classes),
        hours: decimal(signals.hours),
        creator: signals.creator.clone().unwrap_or_default(),
        creator_since: day(signals.creator_since),
        certificate: signals.certificate,
        video: signals.video,
        creator_verified: signals.creator_verified,
        affiliation: signals.affiliation,
        support: signals.support,
    }
}

/// Empty is `None`; anything else must read.
fn optional<T>(text: &str, read: impl Fn(&str) -> Option<T>) -> Result<Option<T>, ()> {
    match text.trim() {
        "" => Ok(None),
        text => read(text).map(Some).ok_or(()),
    }
}

/// A day, or a year alone as its first day: the vitrine shows since when a
/// producer is there by year.
fn parse_day_or_year(text: &str) -> Option<NaiveDate> {
    parse_day(text).or_else(|| {
        text.parse::<i32>()
            .ok()
            .filter(|year| (2000..=2100).contains(year))
            .and_then(|year| NaiveDate::from_ymd_opt(year, 1, 1))
    })
}

/// The fields of "Cadastrar à mão", as text inputs.
struct Form {
    /// The Prospect being completed; `None` for a new one.
    known: Option<Prospect>,
    platform: AffiliatePlatform,
    link: Entity<InputState>,
    name: Entity<InputState>,
    price: Entity<InputState>,
    commission: Entity<InputState>,
    commission_rate: Entity<InputState>,
    temperature: Entity<InputState>,
    blueprint: Entity<InputState>,
    satisfaction: Entity<InputState>,
    rating: Entity<InputState>,
    reviews: Entity<InputState>,
    students: Entity<InputState>,
    launched: Entity<InputState>,
    classes: Entity<InputState>,
    hours: Entity<InputState>,
    creator: Entity<InputState>,
    creator_since: Entity<InputState>,
    certificate: Option<bool>,
    video: Option<bool>,
    creator_verified: Option<bool>,
    affiliation: Option<Affiliation>,
    support: Option<Support>,
    outcome: Option<Outcome>,
}

impl Form {
    fn new(
        platform: AffiliatePlatform,
        known: Option<Prospect>,
        window: &mut Window,
        cx: &mut Context<ProspectsScreen>,
    ) -> Self {
        let typed = known.as_ref().map(typed_from).unwrap_or_default();
        let mut field = |placeholder: &'static str, value: &str| {
            let state = input(placeholder, window, cx);
            let value = value.to_owned();
            state.update(cx, |state, cx| state.set_value(value, window, cx));
            state
        };
        Self {
            platform,
            link: field(
                "https://hotmart.com/pt-br/marketplace/produtos/…",
                &typed.link,
            ),
            name: field("Curso de Excel do Zero", &typed.name),
            price: field("497,00", &typed.price),
            commission: field("149,10", &typed.commission),
            commission_rate: field("30", &typed.commission_rate),
            temperature: field("35", &typed.temperature),
            blueprint: field("85", &typed.blueprint),
            satisfaction: field("90", &typed.satisfaction),
            rating: field("4,8", &typed.rating),
            reviews: field("963", &typed.reviews),
            students: field("12000", &typed.students),
            launched: field("28/09/2026", &typed.launched),
            classes: field("40", &typed.classes),
            hours: field("12", &typed.hours),
            creator: field("Nome do produtor", &typed.creator),
            creator_since: field("2019", &typed.creator_since),
            certificate: typed.certificate,
            video: typed.video,
            creator_verified: typed.creator_verified,
            affiliation: typed.affiliation,
            support: typed.support,
            known,
            outcome: None,
        }
    }

    fn typed(&self, cx: &Context<ProspectsScreen>) -> Typed {
        let text = |input: &Entity<InputState>| input.read(cx).value().to_string();
        Typed {
            link: text(&self.link),
            name: text(&self.name),
            price: text(&self.price),
            commission: text(&self.commission),
            commission_rate: text(&self.commission_rate),
            temperature: text(&self.temperature),
            blueprint: text(&self.blueprint),
            satisfaction: text(&self.satisfaction),
            rating: text(&self.rating),
            reviews: text(&self.reviews),
            students: text(&self.students),
            launched: text(&self.launched),
            classes: text(&self.classes),
            hours: text(&self.hours),
            creator: text(&self.creator),
            creator_since: text(&self.creator_since),
            certificate: self.certificate,
            video: self.video,
            creator_verified: self.creator_verified,
            affiliation: self.affiliation,
            support: self.support,
        }
    }
}

/// What a background change hands back.
enum Changed {
    Nothing,
    Saved(String),
    Read(String),
    /// A reader's failure, already in the owner's words.
    Failed(String),
    Dismissed(String),
    Promoted(Program),
}

pub struct ProspectsScreen {
    prospects: Vec<Prospect>,
    dismissed: u64,
    /// The Platform picked in All; `None` shows every Platform.
    picked: Option<AffiliatePlatform>,
    only_bets: bool,
    /// The Prospect whose "Descartar" is open.
    dismissing: Option<RecordId>,
    reason: Entity<InputState>,
    form: Option<Form>,
    /// The Hotmart vitrine's reader, while its flag is on.
    vitrine: Option<Arc<dyn ProspectSource>>,
    /// The logged-in affiliate markets whose flag is on, each with its
    /// reader.
    markets: Vec<Arc<dyn AffiliateMarket>>,
    /// The vitrine's category "Garimpar" reads; `None` for its best
    /// sellers.
    category: Option<ProspectCategory>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    /// The same, for the reads of the readers' flags.
    flag_reads: u64,
    outcome: Option<Outcome>,
    row_outcome: Option<Outcome>,
}

impl EventEmitter<OpenGuidedFlow> for ProspectsScreen {}

impl ProspectsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            prospects: Vec::new(),
            dismissed: 0,
            picked: None,
            only_bets: false,
            dismissing: None,
            reason: input("Por que não serve? Ex.: nicho saturado", window, cx),
            form: None,
            vitrine: None,
            markets: Vec::new(),
            category: None,
            busy: false,
            reads: 0,
            flag_reads: 0,
            outcome: None,
            row_outcome: None,
        };
        screen.refresh(window, cx);
        screen
    }

    /// Reads everything again, as when the screen comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.read_flags(cx);
        self.run(window, cx, |_| async { Ok(Changed::Nothing) });
    }

    /// Reads the readers' flags again: on, each reader is built; off, it is
    /// dropped and nothing opens for it.
    fn read_flags(&mut self, cx: &mut Context<Self>) {
        let Some(app) = cx.try_global::<AppFlags>() else {
            return;
        };
        let flags = app.flags.clone();
        let vitrine = crawlers(cx)
            .map(|crawlers| (crawlers.hotmart_vitrine.clone(), crawlers.engine.clone()));
        let panels = panels(cx);
        self.flag_reads += 1;
        let reading = self.flag_reads;
        let working = cx.background_executor().spawn(async move {
            let vitrine = match vitrine {
                Some((vitrine, engine)) => crawlers::vitrine_if_on(&flags, &vitrine, engine)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            let markets = match panels {
                Some(panels) => panels
                    .markets_on(&flags)
                    .await
                    .map(|on| on.into_iter().map(|(_, market)| market).collect())
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            (vitrine, markets)
        });
        cx.spawn(async move |this, cx| {
            let (vitrine, markets) = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.flag_reads == reading {
                    this.vitrine = vitrine;
                    this.markets = markets;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Whether the vitrine's card shows: in the Platform's own list, or in
    /// every Platform's.
    fn shows_vitrine(&self, cx: &App) -> bool {
        matches!(self.platform(cx), None | Some(AffiliatePlatform::Hotmart))
    }

    /// "Garimpar": reads the vitrine's chosen category once and keeps what
    /// it shows.
    fn garimpar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.vitrine.clone() else {
            return;
        };
        if source.blocked_until().is_some() {
            cx.notify();
            return;
        }
        let category = self.category.clone();
        self.outcome = None;
        self.run(window, cx, move |affiliate| async move {
            let read = affiliate
                .read_prospects(source.as_ref(), category.as_ref())
                .await?;
            Ok(Changed::Read(read_text(
                &vitrine_read(),
                category.as_ref(),
                read,
            )))
        });
    }

    /// The reader of `platform`'s logged-in market, while its flag is on.
    fn market(&self, platform: AffiliatePlatform) -> Option<Arc<dyn AffiliateMarket>> {
        self.markets
            .iter()
            .find(|market| market.platform() == platform)
            .cloned()
    }

    /// "Garimpar no mercado": reads `platform`'s market listing, in the
    /// chosen category when the market has it, a few pages at most, and
    /// keeps what it shows.
    fn garimpar_market(
        &mut self,
        platform: AffiliatePlatform,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(market), Some(site)) = (self.market(platform), market_site(platform, cx)) else {
            return;
        };
        if market.stopped().is_some() {
            cx.notify();
            return;
        }
        let category = self.category.clone().filter(|category| {
            market
                .categories()
                .unwrap_or_default()
                .iter()
                .any(|known| known.id == category.id)
        });
        let from = market_read(&site);
        self.outcome = None;
        self.run(window, cx, move |affiliate| async move {
            match affiliate
                .read_prospects(market.as_ref(), category.as_ref())
                .await
            {
                Ok(read) => Ok(Changed::Read(read_text(&from, category.as_ref(), read))),
                Err(AffiliateError::Platform(error)) => {
                    Ok(Changed::Failed(market_failure(platform, &error)))
                }
                Err(error) => Err(error),
            }
        });
    }

    /// Reads the page of `prospect` in its Platform's market.
    fn read_in_market(
        &mut self,
        prospect: RecordId,
        platform: AffiliatePlatform,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(market), Some(site)) = (self.market(platform), market_site(platform, cx)) else {
            return;
        };
        if market.stopped().is_some() {
            cx.notify();
            return;
        }
        let called = site.called;
        self.outcome = None;
        self.run(window, cx, move |affiliate| async move {
            match affiliate
                .read_prospect_details(market.as_ref(), prospect)
                .await
            {
                Ok(read) => Ok(Changed::Read(format!(
                    "{}: {called} trouxe os números da página do produto para a nota.",
                    read.name
                ))),
                Err(AffiliateError::Platform(error)) => {
                    Ok(Changed::Failed(market_failure(platform, &error)))
                }
                Err(error) => Err(error),
            }
        });
    }

    /// "Entrar na Hotmart": opens `platform`'s market in the browser's
    /// window for the owner to log in, or answer a robot check, by hand.
    fn log_in_market(
        &mut self,
        platform: AffiliatePlatform,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(market) = self.market(platform) else {
            return;
        };
        self.outcome = None;
        self.busy = true;
        let opening = cx
            .background_executor()
            .spawn(async move { market.open_login() });
        cx.spawn_in(window, async move |this, cx| {
            let opened = opening.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                let name = platform.name();
                this.outcome = Some(match opened {
                    Ok(()) => Outcome::Done(
                        format!(
                            "Janela aberta no mercado da {name}. Entre com a sua conta, confira \
                             que o mercado aparece e feche a janela; depois, Garimpar no mercado."
                        )
                        .into(),
                    ),
                    Err(error) => Outcome::Failed(market_failure(platform, &error).into()),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Reads the product page of `prospect` from the vitrine.
    fn read_page(&mut self, prospect: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.vitrine.clone() else {
            return;
        };
        if source.blocked_until().is_some() {
            cx.notify();
            return;
        }
        self.outcome = None;
        self.run(window, cx, move |affiliate| async move {
            let read = affiliate
                .read_prospect_details(source.as_ref(), prospect)
                .await?;
            Ok(Changed::Read(format!(
                "{}: a página do produto trouxe alunos, garantia e o perfil do produtor para a \
                 nota.",
                read.name
            )))
        });
    }

    /// Follows a new Work Context: the Platform picked in All and what was
    /// open close, and the list reads the context's Platform.
    pub fn follow_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picked = None;
        self.form = None;
        self.dismissing = None;
        self.outcome = None;
        self.refresh(window, cx);
    }

    /// The Platform the list shows: the Work Context's, else the one
    /// picked in All.
    fn platform(&self, cx: &App) -> Option<AffiliatePlatform> {
        preferences::work_context(cx)
            .affiliate_platform()
            .or(self.picked)
    }

    fn run<F, Fut>(&mut self, window: &mut Window, cx: &mut Context<Self>, change: F)
    where
        F: FnOnce(Arc<Affiliate>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Changed, AffiliateError>> + Send,
    {
        let Some(affiliate) = affiliate(cx) else {
            return;
        };
        let platform = self.platform(cx);
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(affiliate.clone()).await;
            let read = async {
                Ok::<_, AffiliateError>((
                    affiliate.prospects(platform).await?,
                    affiliate.dismissed_prospects(platform).await?,
                ))
            }
            .await;
            (changed, read)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, read_back) = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads == read {
                    this.busy = false;
                    match read_back {
                        Ok((prospects, dismissed)) => {
                            this.prospects = prospects;
                            this.dismissed = dismissed;
                        }
                        Err(error) => {
                            this.outcome = Some(Outcome::Failed(
                                format!("Não consegui ler o garimpo: {error}").into(),
                            ))
                        }
                    }
                }
                match changed {
                    Ok(Changed::Nothing) => {}
                    Ok(Changed::Saved(text)) => {
                        this.form = None;
                        this.outcome = Some(Outcome::Done(text.into()));
                    }
                    Ok(Changed::Read(text)) => this.outcome = Some(Outcome::Done(text.into())),
                    Ok(Changed::Failed(text)) => this.outcome = Some(Outcome::Failed(text.into())),
                    Ok(Changed::Dismissed(text)) => {
                        this.dismissing = None;
                        this.outcome = Some(Outcome::Done(text.into()));
                    }
                    Ok(Changed::Promoted(program)) => {
                        this.outcome = None;
                        cx.emit(OpenGuidedFlow(program));
                    }
                    Err(error) => this.failed(&error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn failed(&mut self, error: &AffiliateError) {
        let text = match error {
            AffiliateError::Platform(error) => crawlers::failure(VITRINE, error),
            error => failure(error),
        };
        if let Some(form) = self.form.as_mut() {
            form.outcome = Some(Outcome::Failed(text.into()));
        } else if self.dismissing.is_some() {
            self.row_outcome = Some(Outcome::Failed(text.into()));
        } else {
            self.outcome = Some(Outcome::Failed(text.into()));
        }
    }

    fn open_form(&mut self, known: Option<Prospect>, window: &mut Window, cx: &mut Context<Self>) {
        let platform = known
            .as_ref()
            .map(|known| known.platform)
            .or(self.platform(cx))
            .unwrap_or(AffiliatePlatform::Hotmart);
        self.dismissing = None;
        self.outcome = None;
        self.form = Some(Form::new(platform, known, window, cx));
        cx.notify();
    }

    fn save_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.form.as_mut() else {
            return;
        };
        let typed = form.typed(cx);
        let found = match typed_prospect(form.platform, &typed, form.known.as_ref()) {
            Ok(found) => found,
            Err(text) => {
                form.outcome = Some(Outcome::Failed(text.into()));
                cx.notify();
                return;
            }
        };
        form.outcome = None;
        let completing = form.known.is_some();
        self.run(window, cx, move |affiliate| async move {
            affiliate.save_prospect(&found, SignalOrigin::Typed).await?;
            Ok(Changed::Saved(if completing {
                format!("{} atualizado com os números que você digitou.", found.name)
            } else {
                format!(
                    "{} cadastrado; a nota já considera o que você digitou.",
                    found.name
                )
            }))
        });
    }

    fn dismiss(&mut self, prospect: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let reason = self.reason.read(cx).value().to_string();
        let name = self
            .prospects
            .iter()
            .find(|shown| shown.id == prospect)
            .map(|shown| shown.name.clone())
            .unwrap_or_default();
        self.row_outcome = None;
        self.run(window, cx, move |affiliate| async move {
            affiliate.dismiss_prospect(prospect, &reason).await?;
            Ok(Changed::Dismissed(format!(
                "{name} descartado; ele não volta nem numa nova leitura."
            )))
        });
    }

    fn promote(&mut self, prospect: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.outcome = None;
        self.run(window, cx, move |affiliate| async move {
            Ok(Changed::Promoted(
                affiliate.promote_prospect(prospect).await?,
            ))
        });
    }
}

impl ProspectsScreen {
    fn render_platforms(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if preferences::work_context(cx).affiliate_platform().is_some() {
            return None;
        }
        let chip = |index: usize, label: &'static str, value: Option<AffiliatePlatform>| {
            Button::new(("prospect-platform", index))
                .label(label)
                .small()
                .map(|b| {
                    if self.picked == value {
                        b.primary()
                    } else {
                        b.outline()
                    }
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.picked = value;
                    this.refresh(window, cx);
                }))
        };
        Some(
            h_flex()
                .flex_wrap()
                .gap_1()
                .child(chip(0, "Todas", None))
                .children(
                    MARKET_PLATFORMS
                        .iter()
                        .enumerate()
                        .map(|(index, platform)| chip(index + 1, platform.name(), Some(*platform))),
                )
                .into_any_element(),
        )
    }

    fn render_market(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let card = v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface);
        match self.platform(cx).and_then(market) {
            Some(Market {
                platform,
                url,
                steps,
            }) => card
                .child(
                    h_flex()
                        .gap_3()
                        .flex_wrap()
                        .items_center()
                        .child(
                            div()
                                .font_medium()
                                .child(format!("Onde garimpar na {}", platform.name())),
                        )
                        .child(kit::external_link(
                            "open-market",
                            format!("Abrir o mercado de afiliação da {}", platform.name()),
                            url,
                            cx,
                        )),
                )
                .child(div().text_xs().text_color(t.text2).child(
                    if self.market(platform).is_some() {
                        "O Mascate lê esse mercado pelo cartão acima; o que ele não ler, traga \
                         para cá:"
                    } else {
                        "O Mascate ainda não lê esse mercado sozinho. Passe por ele e traga os \
                         números para cá:"
                    },
                ))
                .children(steps.iter().enumerate().map(|(index, step)| {
                    h_flex()
                        .gap_2()
                        .items_start()
                        .text_sm()
                        .child(
                            div()
                                .flex_none()
                                .w(px(18.))
                                .text_color(t.accent_text)
                                .child(format!("{}.", index + 1)),
                        )
                        .child(div().min_w_0().flex_1().child(*step))
                }))
                .into_any_element(),
            None => card
                .child(div().font_medium().child("Onde garimpar"))
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text2)
                        .child(where_to_look_text(&self.markets_read())),
                )
                .child(
                    h_flex().flex_wrap().gap_3().children(
                        MARKET_PLATFORMS
                            .iter()
                            .filter_map(|platform| market(*platform))
                            .map(|market| {
                                kit::external_link(
                                    SharedString::from(format!(
                                        "open-market-{}",
                                        market.platform.name()
                                    )),
                                    format!("Mercado da {}", market.platform.name()),
                                    market.url,
                                    cx,
                                )
                            }),
                    ),
                )
                .into_any_element(),
        }
    }

    /// The categories of `source` to pick from, the first chip, named
    /// `everything`, for none.
    fn render_categories(
        &self,
        source: &dyn ProspectSource,
        everything: &str,
        off: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chip = |index: usize, label: String, category: Option<ProspectCategory>| {
            let on = self.category.as_ref().map(|c| &c.id) == category.as_ref().map(|c| &c.id);
            Button::new(("prospect-category", index))
                .label(label)
                .small()
                .map(|b| if on { b.primary() } else { b.ghost() })
                .disabled(off)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.category = category.clone();
                    cx.notify();
                }))
        };
        h_flex()
            .flex_wrap()
            .gap_1()
            .child(chip(0, everything.to_owned(), None))
            .children(
                source
                    .categories()
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .map(|(index, category)| {
                        chip(index + 1, category.name.clone(), Some(category))
                    }),
            )
            .into_any_element()
    }

    /// The Platforms whose market the app reads now, in the Work
    /// Context's order.
    fn markets_read(&self) -> Vec<AffiliatePlatform> {
        MARKET_PLATFORMS
            .into_iter()
            .filter(|platform| self.market(*platform).is_some())
            .collect()
    }

    /// The logged-in market of each Platform the list shows: in a
    /// Platform's own list, whether its flag is on or not; in every
    /// Platform's, only those that are on.
    fn render_logged_markets(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(panels) = panels(cx) else {
            return Vec::new();
        };
        let shown = self.platform(cx);
        panels
            .markets
            .iter()
            .enumerate()
            .map(|(index, site)| (index, site.texts))
            .filter(|(_, site)| match shown {
                Some(platform) => platform == site.platform,
                None => self.market(site.platform).is_some(),
            })
            .map(|(index, site)| self.render_logged_market(index, &site, cx))
            .collect()
    }

    /// A Platform's logged-in affiliate market: while its flag is on, its
    /// Connection, "Entrar na …", "Garimpar no mercado" and its stop; else
    /// what the flag does and why it is off.
    fn render_logged_market(
        &self,
        index: usize,
        site: &MarketTexts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let platform = site.platform;
        let name = platform.name();
        let card = v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface);
        let title = format!("Garimpar {} da {name}", site.called_in());
        let Some(market) = self.market(platform) else {
            return card
                .child(div().font_medium().child(title))
                .child(div().text_sm().child(format!(
                    "Ligando \"{}\" em Configurações › Funcionalidades restritas, o Mascate lê \
                     {} da {name}, com o seu login, quando você clica: {}.",
                    site.flag.name, site.called, site.reads
                )))
                .child(div().text_xs().text_color(t.text2).child(format!(
                    "Fica desligado porque: {} {}",
                    site.flag.reason, site.flag.risk
                )))
                .into_any_element();
        };
        let state = market.state();
        let stopped = market.stopped();
        let can_open = state != PanelState::NoBrowser;
        let can_read = matches!(state, PanelState::Ready | PanelState::LoggedOut);
        let off = self.busy || stopped.is_some() || !can_read;
        let (label, ink) = match state {
            PanelState::NoBrowser => ("sem navegador", t.danger),
            PanelState::NeverLoggedIn => ("falta entrar", t.danger),
            PanelState::LoggedOut => ("entre de novo", t.danger),
            PanelState::Ready => ("conectado", t.success),
        };
        let has_categories = market
            .categories()
            .is_ok_and(|categories| !categories.is_empty());
        // The vitrine's card shows the same Platform's categories already.
        let vitrine_picks = self.shows_vitrine(cx)
            && self
                .vitrine
                .as_ref()
                .is_some_and(|vitrine| vitrine.platform() == platform);
        card.child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .items_center()
                .child(div().font_medium().child(title))
                .child(kit::tag(label, ink, cx))
                .child(kit::tag("Leitura ligada", t.danger, cx)),
        )
        .child(
            div()
                .text_xs()
                .text_color(t.text2)
                .child(market_state_text(site, state)),
        )
        .children(stopped.map(|stopped| kit::error_notice(stopped_text(platform, stopped), cx)))
        .when(has_categories && !vitrine_picks && can_read, |card| {
            card.child(self.render_categories(market.as_ref(), "Todas", off, cx))
        })
        .when(has_categories && vitrine_picks && can_read, |card| {
            card.child(
                div()
                    .text_xs()
                    .text_color(t.text2)
                    .child("Lê a categoria escolhida acima, na vitrine."),
            )
        })
        .child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .when(can_open, |row| {
                    row.child(
                        Button::new(("log-in-market", index))
                            .label(format!("Entrar na {name}"))
                            .icon(IconName::ExternalLink)
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.log_in_market(platform, window, cx)
                            })),
                    )
                })
                .when(can_read, |row| {
                    row.child(
                        Button::new(("garimpar-market", index))
                            .label("Garimpar no mercado")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(off)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.garimpar_market(platform, window, cx)
                            })),
                    )
                }),
        )
        .into_any_element()
    }

    /// The Hotmart vitrine: its categories and "Garimpar" while its flag is
    /// on, else what the flag does and why it is off.
    fn render_vitrine(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.shows_vitrine(cx) {
            return None;
        }
        let t = look(cx).tokens;
        let card = v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface);
        let Some(source) = self.vitrine.clone() else {
            return Some(
                card.child(div().font_medium().child("Garimpar na vitrine da Hotmart"))
                    .child(div().text_sm().child(format!(
                        "Ligando \"{}\" em Configurações › Funcionalidades restritas, o Mascate \
                         lê a vitrine pública da Hotmart quando você clica em Garimpar: nota, \
                         avaliações, quando o produto entrou, aulas, horas, certificado e o \
                         produtor, e dá a nota de cada um.",
                        HOTMART_CRAWLER.name
                    )))
                    .child(div().text_xs().text_color(t.text2).child(format!(
                        "Fica desligado porque: {} {}",
                        HOTMART_CRAWLER.reason, HOTMART_CRAWLER.risk
                    )))
                    .into_any_element(),
            );
        };
        let blocked = source.blocked_until().filter(|until| *until > Utc::now());
        let off = self.busy || blocked.is_some();
        Some(
            card.child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .items_center()
                    .child(div().font_medium().child("Garimpar na vitrine da Hotmart"))
                    .child(kit::tag("Crawler ligado", t.danger, cx)),
            )
            .child(div().text_xs().text_color(t.text2).child(
                "Lê a vitrine pública só quando você clica: uma página a cada 3 segundos ou \
                 mais, e a mesma página dentro de 1 hora vem da memória. Traz nota, avaliações, \
                 quando entrou, aulas, horas, certificado e o produtor; Ler a página do produto \
                 soma alunos, garantia e o perfil do produtor. Comissão e temperatura só o \
                 mercado de afiliação mostra.",
            ))
            .children(
                blocked.map(|until| kit::error_notice(crawlers::blocked_text(VITRINE, until), cx)),
            )
            .child(self.render_categories(source.as_ref(), "Mais vendidos", off, cx))
            .child(
                h_flex().child(
                    Button::new("garimpar")
                        .label("Garimpar")
                        .primary()
                        .small()
                        .loading(self.busy)
                        .disabled(off)
                        .on_click(cx.listener(|this, _, window, cx| this.garimpar(window, cx))),
                ),
            )
            .into_any_element(),
        )
    }

    fn render_form(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.form.as_ref()?;
        let t = look(cx).tokens;
        let field = |label: &'static str, input: &Entity<InputState>, width: f32| {
            v_flex()
                .gap_1()
                .w(px(width))
                .child(div().text_xs().text_color(t.text2).child(label))
                .child(Input::new(input).small())
        };
        let group = |title: &'static str| div().text_sm().font_medium().child(title);
        let choice = |id: &'static str, label: &'static str, on: bool, pick: fn(&mut Form)| {
            Button::new(id)
                .label(label)
                .small()
                .map(|b| if on { b.primary() } else { b.outline() })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = this.form.as_mut() {
                        pick(form);
                    }
                    cx.notify();
                }))
        };
        let yes_no = |label: &'static str,
                      value: Option<bool>,
                      ids: [&'static str; 3],
                      set: [fn(&mut Form); 3]| {
            h_flex()
                .gap_1()
                .items_center()
                .child(div().w(px(170.)).text_xs().text_color(t.text2).child(label))
                .child(choice(ids[0], "Não sei", value.is_none(), set[0]))
                .child(choice(ids[1], "Sim", value == Some(true), set[1]))
                .child(choice(ids[2], "Não", value == Some(false), set[2]))
        };
        let title = match &form.known {
            Some(known) => format!("Completar {}", known.name),
            None => format!("Cadastrar à mão · {}", form.platform.name()),
        };
        let platform_chips = (form.known.is_none()
            && preferences::work_context(cx).affiliate_platform().is_none())
        .then(|| {
            h_flex()
                .flex_wrap()
                .gap_1()
                .children(
                    MARKET_PLATFORMS
                        .iter()
                        .enumerate()
                        .map(|(index, platform)| {
                            let platform = *platform;
                            Button::new(("form-platform", index))
                                .label(platform.name())
                                .small()
                                .map(|b| {
                                    if form.platform == platform {
                                        b.primary()
                                    } else {
                                        b.outline()
                                    }
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(form) = this.form.as_mut() {
                                        form.platform = platform;
                                    }
                                    cx.notify();
                                }))
                        }),
                )
        });
        Some(
            v_flex()
                .gap_3()
                .p_3()
                .rounded(t.radius_lg)
                .border(t.border_width)
                .border_color(t.accent_edge)
                .bg(t.surface)
                .child(div().font_medium().child(title))
                .children(platform_chips)
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(field("Endereço da página do produto", &form.link, 420.))
                        .child(field("Nome do produto", &form.name, 260.)),
                )
                .child(group("Comissão"))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(field("Preço (R$)", &form.price, 110.))
                        .child(field("Comissão (R$)", &form.commission, 110.))
                        .child(field("Comissão (%)", &form.commission_rate, 110.)),
                )
                .child(group("Vendas"))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(field("Temperatura (°)", &form.temperature, 110.))
                        .child(field("Alunos", &form.students, 110.))
                        .child(field("Avaliações", &form.reviews, 110.))
                        .child(field("Entrou em", &form.launched, 120.)),
                )
                .child(group("Material"))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(field("Blueprint (%)", &form.blueprint, 110.))
                        .child(field("Aulas", &form.classes, 110.))
                        .child(field("Horas", &form.hours, 110.)),
                )
                .child(yes_no(
                    "Certificado",
                    form.certificate,
                    ["certificate-unknown", "certificate-yes", "certificate-no"],
                    [
                        |f| f.certificate = None,
                        |f| f.certificate = Some(true),
                        |f| f.certificate = Some(false),
                    ],
                ))
                .child(yes_no(
                    "Vídeo de apresentação",
                    form.video,
                    ["video-unknown", "video-yes", "video-no"],
                    [
                        |f| f.video = None,
                        |f| f.video = Some(true),
                        |f| f.video = Some(false),
                    ],
                ))
                .child(group("Criador"))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(field("Produtor", &form.creator, 200.))
                        .child(field("Na plataforma desde", &form.creator_since, 140.))
                        .child(field("Nota (0 a 5)", &form.rating, 100.))
                        .child(field("Satisfação (%)", &form.satisfaction, 110.)),
                )
                .child(yes_no(
                    "Perfil verificado",
                    form.creator_verified,
                    ["verified-unknown", "verified-yes", "verified-no"],
                    [
                        |f| f.creator_verified = None,
                        |f| f.creator_verified = Some(true),
                        |f| f.creator_verified = Some(false),
                    ],
                ))
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            div()
                                .w(px(170.))
                                .text_xs()
                                .text_color(t.text2)
                                .child("Suporte"),
                        )
                        .child(choice(
                            "support-unknown",
                            "Não sei",
                            form.support.is_none(),
                            |f| f.support = None,
                        ))
                        .child(choice(
                            "support-good",
                            "Bom",
                            form.support == Some(Support::Good),
                            |f| f.support = Some(Support::Good),
                        ))
                        .child(choice(
                            "support-bad",
                            "Ruim",
                            form.support == Some(Support::Bad),
                            |f| f.support = Some(Support::Bad),
                        )),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            div()
                                .w(px(170.))
                                .text_xs()
                                .text_color(t.text2)
                                .child("Afiliação"),
                        )
                        .child(choice(
                            "affiliation-unknown",
                            "Não sei",
                            form.affiliation.is_none(),
                            |f| f.affiliation = None,
                        ))
                        .child(choice(
                            "affiliation-immediate",
                            "Imediata",
                            form.affiliation == Some(Affiliation::Immediate),
                            |f| f.affiliation = Some(Affiliation::Immediate),
                        ))
                        .child(choice(
                            "affiliation-request",
                            "Com aprovação",
                            form.affiliation == Some(Affiliation::OnRequest),
                            |f| f.affiliation = Some(Affiliation::OnRequest),
                        )),
                )
                .children(form.outcome.as_ref().map(|outcome| notice(outcome, cx)))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("save-prospect")
                                .label("Salvar e pontuar")
                                .primary()
                                .small()
                                .loading(self.busy)
                                .disabled(self.busy)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.save_form(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("cancel-prospect")
                                .label("Cancelar")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.form = None;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_prospect(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let prospect = &self.prospects[index];
        let id = prospect.id;
        let platform = prospect.platform;
        let dismissing = self.dismissing == Some(id);
        let all = preferences::work_context(cx).affiliate_platform().is_none();
        let mut tags = h_flex().flex_wrap().gap_1();
        if all {
            tags = tags.child(kit::tag(prospect.platform.name(), t.text2, cx));
        }
        if prospect.bet {
            tags = tags.child(kit::tag("aposta", t.accent_text, cx));
        }
        if prospect.program.is_some() {
            tags = tags.child(kit::tag(
                if prospect.chosen {
                    "escolhido"
                } else {
                    "já divulga"
                },
                t.success,
                cx,
            ));
        }
        if prospect.score.partial() {
            tags = tags.child(kit::tag("nota parcial", t.text2, cx));
        }
        if let Some(format) = prospect.signals.format {
            tags = tags.child(kit::tag(format.name(), t.text2, cx));
        }
        if let Some(category) = &prospect.signals.category {
            tags = tags.child(kit::tag(category.clone(), t.text2, cx));
        }
        if let Some(affiliation) = prospect.signals.affiliation {
            tags = tags.child(kit::tag(
                match affiliation {
                    Affiliation::Immediate => "afiliação imediata",
                    Affiliation::OnRequest => "afiliação com aprovação",
                },
                t.text2,
                cx,
            ));
        }
        let parts = prospect.score.parts.iter().map(|part| {
            h_flex()
                .gap_2()
                .items_start()
                .text_sm()
                .child(
                    div()
                        .flex_none()
                        .w(px(110.))
                        .text_color(t.text2)
                        .child(format!("{} {}", capitalized(part.area.name()), part.points)),
                )
                .child(div().min_w_0().flex_1().child(part.reason.clone()))
        });
        let missing = (!prospect.score.missing.is_empty()).then(|| {
            let wanted: Vec<&str> = prospect
                .score
                .missing
                .iter()
                .map(|area| area.wanted())
                .collect();
            div().text_xs().text_color(t.text2).child(format!(
                "Fica de fora da nota, sem sinal: {}. Complete os números para uma nota inteira.",
                wanted.join("; ")
            ))
        });
        let mut origins: Vec<SignalOrigin> = Vec::new();
        for reading in &prospect.readings {
            if !origins.contains(&reading.origin) {
                origins.push(reading.origin);
            }
        }
        let origins = (!origins.is_empty()).then(|| {
            let names: Vec<&str> = origins.iter().map(|origin| origin.name()).collect();
            div()
                .text_xs()
                .text_color(t.text2)
                .child(format!("Sinais: {}.", names.join(", ")))
        });
        let affiliation_page = prospect
            .signals
            .affiliation_page
            .clone()
            .map(|page| ("Abrir página de afiliação".to_owned(), page))
            .or_else(|| {
                market(prospect.platform).map(|market| {
                    (
                        format!(
                            "Abrir o mercado da {} para se afiliar",
                            prospect.platform.name()
                        ),
                        market.url.to_owned(),
                    )
                })
            });
        let shown = prospect.clone();
        let actions = h_flex()
            .gap_2()
            .flex_wrap()
            .items_center()
            .child(
                Button::new(("promote", index))
                    .label(if prospect.program.is_some() {
                        "Abrir no Passo a passo"
                    } else {
                        "Quero divulgar"
                    })
                    .primary()
                    .small()
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| this.promote(id, window, cx))),
            )
            .child(
                Button::new(("complete", index))
                    .label("Completar números")
                    .outline()
                    .small()
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_form(Some(shown.clone()), window, cx)
                    })),
            )
            .when(
                self.vitrine.is_some()
                    && prospect.platform == AffiliatePlatform::Hotmart
                    && prospect.signals.product_page.is_some(),
                |row| {
                    row.child(
                        Button::new(("read-page", index))
                            .label("Ler a página do produto")
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.read_page(id, window, cx)
                            })),
                    )
                },
            )
            .when(
                self.market(prospect.platform).is_some()
                    && prospect.signals.affiliation_page.is_some(),
                |row| {
                    row.child(
                        Button::new(("read-market", index))
                            .label("Ler no mercado")
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.read_in_market(id, platform, window, cx)
                            })),
                    )
                },
            )
            .when(!dismissing, |row| {
                row.child(
                    Button::new(("dismiss", index))
                        .label("Descartar")
                        .ghost()
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.dismissing = Some(id);
                            this.row_outcome = None;
                            this.reason
                                .update(cx, |input, cx| input.set_value("", window, cx));
                            cx.notify();
                        })),
                )
            })
            .children(prospect.signals.product_page.clone().map(|page| {
                kit::external_link(
                    SharedString::from(format!("product-page-{index}")),
                    "Abrir página do produto",
                    page,
                    cx,
                )
            }))
            .children(affiliation_page.map(|(label, page)| {
                kit::external_link(
                    SharedString::from(format!("affiliation-page-{index}")),
                    label,
                    page,
                    cx,
                )
            }));
        let dismiss_form = dismissing.then(|| {
            h_flex()
                .flex_wrap()
                .gap_2()
                .items_end()
                .child(
                    v_flex()
                        .gap_1()
                        .w(px(420.))
                        .child(div().text_sm().font_medium().child("Motivo"))
                        .child(Input::new(&self.reason).small()),
                )
                .child(
                    Button::new(("confirm-dismiss", index))
                        .label("Descartar")
                        .danger()
                        .small()
                        .loading(self.busy)
                        .disabled(self.busy)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.dismiss(id, window, cx)),
                        ),
                )
                .child(
                    Button::new(("cancel-dismiss", index))
                        .label("Cancelar")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.dismissing = None;
                            this.row_outcome = None;
                            cx.notify();
                        })),
                )
        });
        let score_ink = match prospect.score.total {
            70.. => t.success,
            40.. => t.accent_text,
            _ => t.text2,
        };
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if dismissing { t.accent_edge } else { t.frame })
            .bg(t.surface)
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .justify_between()
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .gap_1()
                            .child(div().font_medium().child(prospect.name.clone()))
                            .child(tags),
                    )
                    .child(
                        v_flex()
                            .flex_none()
                            .items_end()
                            .child(
                                div()
                                    .text_xl()
                                    .font_semibold()
                                    .text_color(score_ink)
                                    .child(prospect.score.total.to_string()),
                            )
                            .child(div().text_xs().text_color(t.text2).child("de 100")),
                    ),
            )
            .children(parts)
            .children(missing)
            .children(origins)
            .child(actions)
            .children(dismiss_form)
            .when(dismissing, |card| {
                card.children(self.row_outcome.as_ref().map(|outcome| notice(outcome, cx)))
            })
            .into_any_element()
    }
}

impl Render for ProspectsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut parts = ScreenParts::new("Garimpo");
        if affiliate(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        parts.actions.push(
            Button::new("add-prospect")
                .label("Cadastrar à mão")
                .outline()
                .small()
                .disabled(self.busy || self.form.is_some())
                .on_click(cx.listener(|this, _, window, cx| this.open_form(None, window, cx)))
                .into_any_element(),
        );
        parts
            .notices
            .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        let bets = self.prospects.iter().filter(|p| p.bet).count();
        let shown: Vec<usize> = (0..self.prospects.len())
            .filter(|index| !self.only_bets || self.prospects[*index].bet)
            .collect();
        let platforms = self.render_platforms(cx);
        let vitrine = self.render_vitrine(cx);
        let logged_markets = self.render_logged_markets(cx);
        let market = self.render_market(cx);
        let form = self.render_form(cx);
        let filter = |id: &'static str, label: String, on: bool, only_bets: bool| {
            Button::new(id)
                .label(label)
                .small()
                .map(|b| if on { b.primary() } else { b.outline() })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.only_bets = only_bets;
                    cx.notify();
                }))
        };
        let empty = if self.only_bets {
            "Nenhuma aposta ainda: produto que entrou há até 90 dias, com Blueprint de 80% ou \
             nota 4,5 com 10 avaliações, e temperatura até 40°."
        } else if (self.vitrine.is_some() && self.shows_vitrine(cx))
            || self
                .platform(cx)
                .map_or(!self.markets.is_empty(), |platform| {
                    self.market(platform).is_some()
                })
        {
            "Nenhum produto no garimpo ainda. Clique em Garimpar, com uma categoria onde o \
             cartão oferece, ou traga os números do mercado em Cadastrar à mão."
        } else {
            "Nenhum produto no garimpo ainda. Abra o mercado da plataforma, escolha os que \
             parecem bons e traga os números em Cadastrar à mão."
        };
        let mut list = v_flex()
            .gap_3()
            .children(platforms)
            .children(vitrine)
            .children(logged_markets)
            .child(market)
            .children(form)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(filter(
                        "all-prospects",
                        format!("Todos ({})", self.prospects.len()),
                        !self.only_bets,
                        false,
                    ))
                    .child(filter(
                        "bet-prospects",
                        format!("Apostas ({bets})"),
                        self.only_bets,
                        true,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text2)
                            .child("pela nota: comissão por venda, vendas, material e criador"),
                    ),
            );
        if shown.is_empty() {
            list = list.child(div().text_sm().text_color(t.text2).child(empty));
        }
        list = list.children(
            shown
                .into_iter()
                .map(|index| self.render_prospect(index, cx)),
        );
        if self.dismissed > 0 {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(t.text2)
                    .child(match self.dismissed {
                        1 => "1 produto descartado não aparece mais.".to_owned(),
                        count => format!("{count} produtos descartados não aparecem mais."),
                    }),
            );
        }
        parts.content.push(list.into_any_element());
        layout::screen(parts, cx)
    }
}

/// The market of `platform`, whether its flag is on or not.
fn market_site(platform: AffiliatePlatform, cx: &App) -> Option<MarketTexts> {
    panels(cx)?.market_site(platform).map(|site| site.texts)
}

/// Why reading `platform`'s affiliate market failed, as the owner reads it.
pub fn market_failure(platform: AffiliatePlatform, error: &PlatformError) -> String {
    let name = platform.name();
    match error {
        PlatformError::NotConnected => format!(
            "Falta entrar na {name} uma vez: clique em Entrar na {name} e entre com a sua conta \
             na janela que abrir."
        ),
        PlatformError::Expired => format!(
            "A {name} pediu login. Clique em Entrar na {name}, entre na janela que abrir e \
             garimpe de novo."
        ),
        PlatformError::RateLimited => {
            format!("A {name} recusou a página, então o app parou de ler o mercado.")
        }
        PlatformError::Refused(why) => format!("Não li o mercado da {name}: {why}."),
        PlatformError::NotFound => format!("O mercado da {name} não abriu essa página."),
        PlatformError::Failed(why) => format!("Não consegui ler o mercado da {name}: {why}"),
    }
}

/// Why `platform`'s market reading stopped, and until when.
pub fn stopped_text(platform: AffiliatePlatform, stopped: MarketStopped) -> String {
    let name = platform.name();
    let until = crawlers::until_text(stopped.until);
    match stopped.why {
        MarketStop::Login => format!(
            "A última leitura caiu no login da {name}, então o app não abre o mercado até \
             {until}, ou até você entrar de novo por Entrar na {name}."
        ),
        MarketStop::Check => format!(
            "A {name} pediu a verificação contra robôs, então o app não abre o mercado até \
             {until}. Se quiser antes, clique em Entrar na {name} e responda você mesmo na \
             janela."
        ),
        MarketStop::Block => format!(
            "A {name} recusou a leitura, então o app não abre o mercado até {until}. Bloqueio \
             é a resposta do site: o app não tenta contornar."
        ),
    }
}

/// What the state of `site`'s reading means for the owner.
fn market_state_text(site: &MarketTexts, state: PanelState) -> String {
    let name = site.platform.name();
    match state {
        PanelState::NoBrowser => "Não há Edge nem Chrome instalado neste computador, e o app lê \
                                  o mercado por um deles. Instale um e abra o app de novo."
            .to_owned(),
        PanelState::NeverLoggedIn => format!(
            "Falta entrar uma vez: clique em Entrar na {name}, entre com a sua conta na janela \
             que abrir e feche a janela quando o mercado aparecer. A senha fica só no navegador."
        ),
        PanelState::LoggedOut => format!(
            "A última leitura caiu no login da {name}. Clique em Entrar na {name} e entre de \
             novo na janela."
        ),
        PanelState::Ready => format!(
            "Conectado pelo navegador do Mascate, num perfil só dele. Lê só quando você clica, \
             {}, uma página a cada 5 segundos ou mais, e a mesma página dentro de 1 hora vem da \
             memória. Traz {}; não clica, não pede afiliação e não resolve captcha.",
            site.pages, site.reads
        ),
    }
}

/// Where a "Garimpar" read, as its result names it.
pub struct ReadFrom {
    /// "a vitrine da Hotmart", to start a sentence.
    pub the: String,
    /// "da vitrine".
    pub of: String,
    /// "nos mais vendidos", without a category.
    pub everything: &'static str,
}

/// The Hotmart vitrine, as a read names it.
pub fn vitrine_read() -> ReadFrom {
    ReadFrom {
        the: "a vitrine da Hotmart".into(),
        of: "da vitrine".into(),
        everything: "nos mais vendidos",
    }
}

/// `site`'s market, as a read names it.
pub fn market_read(site: &MarketTexts) -> ReadFrom {
    ReadFrom {
        the: format!("{} da {}", site.called, site.platform.name()),
        of: format!("d{}", site.called),
        everything: site.everything,
    }
}

/// What one "Garimpar" from `from` in `category` (everything without one)
/// did.
pub fn read_text(
    from: &ReadFrom,
    category: Option<&ProspectCategory>,
    read: ProspectsRead,
) -> String {
    let place = category.map_or(from.everything.to_owned(), |c| format!("em {}", c.name));
    if read.found == 0 {
        return format!(
            "{} não mostrou nenhum produto {place}.",
            capitalized(&from.the)
        );
    }
    let found = match read.found {
        1 => "1 produto".to_owned(),
        count => format!("{count} produtos"),
    };
    let added = match read.added {
        0 => "nenhum novo; os que já estavam foram atualizados".to_owned(),
        1 => "1 novo no garimpo".to_owned(),
        count => format!("{count} novos no garimpo"),
    };
    let dismissed = match read.dismissed {
        0 => String::new(),
        1 => " 1 que você descartou continua de fora.".to_owned(),
        count => format!(" {count} que você descartou continuam de fora."),
    };
    format!("Li {found} {} {place}: {added}.{dismissed}", from.of)
}

/// What "Onde garimpar" says in every Platform's list, given the
/// Platforms whose market the app reads now.
pub fn where_to_look_text(read: &[AffiliatePlatform]) -> String {
    let names: Vec<String> = read
        .iter()
        .map(|platform| format!("da {}", platform.name()))
        .collect();
    let choose = "escolha uma plataforma acima para ver o passo a passo dela.";
    match names.as_slice() {
        [] => format!(
            "O Mascate ainda não lê os mercados sozinho. Abra o de cada plataforma e traga os \
             números em Cadastrar à mão; {choose}"
        ),
        [one] => format!(
            "O Mascate lê sozinho só o mercado {one}, pelo cartão acima. Abra o de cada outra \
             plataforma e traga os números em Cadastrar à mão; {choose}"
        ),
        [many @ .., last] => format!(
            "O Mascate lê sozinho os mercados {} e {last}, pelos cartões acima. Abra o de cada \
             outra plataforma e traga os números em Cadastrar à mão; {choose}",
            many.join(", ")
        ),
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use mascate_platform::{Build, Environment};

    use super::*;
    use crate::affiliate_panels::MarketSite;

    fn typed() -> Typed {
        Typed {
            link: "https://hotmart.com/pt-br/marketplace/produtos/excel/X123?ref=1".into(),
            name: " Excel do Zero ".into(),
            price: "497,00".into(),
            commission_rate: "30".into(),
            temperature: "35".into(),
            reviews: "1.234".into(),
            rating: "4,8".into(),
            launched: "28/09/2026".into(),
            creator_since: "2019".into(),
            certificate: Some(true),
            support: Some(Support::Good),
            ..Typed::default()
        }
    }

    #[test]
    fn what_the_owner_types_becomes_a_prospect_keyed_by_its_link() {
        let found = typed_prospect(AffiliatePlatform::Hotmart, &typed(), None).unwrap();
        assert_eq!(
            found.key,
            "https://hotmart.com/pt-br/marketplace/produtos/excel/X123"
        );
        assert_eq!(found.name, "Excel do Zero");
        let signals = found.signals;
        assert_eq!(
            signals.price,
            Some(Money::new(Decimal::new(49700, 2), Currency::Brl))
        );
        assert_eq!(signals.commission_rate, Percentage::parse("30"));
        assert_eq!(signals.temperature, Some(35));
        assert_eq!(signals.reviews, Some(1234));
        assert_eq!(signals.rating, Some(Decimal::new(48, 1)));
        assert_eq!(signals.launched, NaiveDate::from_ymd_opt(2026, 9, 28));
        assert_eq!(signals.creator_since, NaiveDate::from_ymd_opt(2019, 1, 1));
        assert_eq!(signals.certificate, Some(true));
        assert_eq!(signals.commission, None);
        assert_eq!(
            signals.product_page.as_deref(),
            Some("https://hotmart.com/pt-br/marketplace/produtos/excel/X123?ref=1")
        );
    }

    #[test]
    fn a_read_of_the_vitrine_says_what_it_found_and_what_stays_out() {
        let cooking = ProspectCategory {
            id: "culinaria-e-gastronomia".into(),
            name: "Culinária e gastronomia".into(),
        };
        let read = |found, added, dismissed| ProspectsRead {
            found,
            added,
            dismissed,
        };

        assert_eq!(
            read_text(&vitrine_read(), Some(&cooking), read(24, 5, 2)),
            "Li 24 produtos da vitrine em Culinária e gastronomia: 5 novos no garimpo. 2 que \
             você descartou continuam de fora."
        );
        assert_eq!(
            read_text(&vitrine_read(), None, read(1, 0, 0)),
            "Li 1 produto da vitrine nos mais vendidos: nenhum novo; os que já estavam foram \
             atualizados."
        );
        assert_eq!(
            read_text(&vitrine_read(), None, read(3, 1, 1)),
            "Li 3 produtos da vitrine nos mais vendidos: 1 novo no garimpo. 1 que você \
             descartou continua de fora."
        );
        assert_eq!(
            read_text(&vitrine_read(), Some(&cooking), read(0, 0, 0)),
            "A vitrine da Hotmart não mostrou nenhum produto em Culinária e gastronomia."
        );
    }

    fn texts(platform: AffiliatePlatform) -> MarketTexts {
        MarketSite::all(
            Build::Release,
            &(Box::new(|_: &str| None) as Environment),
            None,
        )
        .into_iter()
        .map(|site| site.texts)
        .find(|texts| texts.platform == platform)
        .unwrap()
    }

    #[test]
    fn a_read_of_a_market_names_the_market_and_where_it_looked() {
        let read = ProspectsRead {
            found: 12,
            added: 3,
            dismissed: 0,
        };
        let hotmart = market_read(&texts(AffiliatePlatform::Hotmart));
        let kiwify = market_read(&texts(AffiliatePlatform::Kiwify));
        assert_eq!(
            read_text(&hotmart, None, read),
            "Li 12 produtos do mercado de afiliação em todas as categorias: 3 novos no garimpo."
        );
        assert_eq!(
            read_text(&hotmart, None, ProspectsRead::default()),
            "O mercado de afiliação da Hotmart não mostrou nenhum produto em todas as categorias."
        );
        assert_eq!(
            read_text(&kiwify, None, read),
            "Li 12 produtos do marketplace na primeira página: 3 novos no garimpo."
        );
        assert_eq!(
            read_text(&kiwify, None, ProspectsRead::default()),
            "O marketplace da Kiwify não mostrou nenhum produto na primeira página."
        );
        assert_eq!(
            texts(AffiliatePlatform::Kiwify).called_in(),
            "no marketplace"
        );
        assert_eq!(texts(AffiliatePlatform::Eduzz).called_in(), "na Vitrine");
        let eduzz = market_read(&texts(AffiliatePlatform::Eduzz));
        assert_eq!(
            read_text(&eduzz, None, read),
            "Li 12 produtos da Vitrine na primeira página: 3 novos no garimpo."
        );
        assert_eq!(
            read_text(&eduzz, None, ProspectsRead::default()),
            "A Vitrine da Eduzz não mostrou nenhum produto na primeira página."
        );
        let monetizze = market_read(&texts(AffiliatePlatform::Monetizze));
        assert_eq!(
            texts(AffiliatePlatform::Monetizze).called_in(),
            "na Vitrine de afiliação"
        );
        assert_eq!(
            read_text(&monetizze, None, read),
            "Li 12 produtos da Vitrine de afiliação na primeira página: 3 novos no garimpo."
        );
        assert_eq!(
            read_text(&monetizze, None, ProspectsRead::default()),
            "A Vitrine de afiliação da Monetizze não mostrou nenhum produto na primeira página."
        );
    }

    #[test]
    fn each_market_says_why_it_stopped_and_what_the_owner_can_do_by_its_own_name() {
        let until = Utc::now() + chrono::TimeDelta::hours(24);
        for platform in MARKET_PLATFORMS {
            let name = platform.name();
            let stopped = |why| stopped_text(platform, MarketStopped { why, until });
            assert!(
                stopped(MarketStop::Login)
                    .contains(&format!("entrar de novo por Entrar na {name}"))
            );
            assert!(stopped(MarketStop::Check).contains("responda você mesmo"));
            assert!(stopped(MarketStop::Block).contains("o app não tenta contornar"));
            assert!(stopped(MarketStop::Block).contains(&crawlers::until_text(until)));
            assert!(
                market_failure(platform, &PlatformError::NotConnected)
                    .contains(&format!("Entrar na {name}"))
            );
            assert!(market_failure(platform, &PlatformError::Expired).contains("pediu login"));
            let site = texts(platform);
            assert!(market_state_text(&site, PanelState::Ready).contains(site.reads));
            assert!(market_state_text(&site, PanelState::Ready).contains(site.pages));
        }
    }

    #[test]
    fn where_to_look_names_every_market_the_app_reads() {
        assert!(where_to_look_text(&[]).starts_with("O Mascate ainda não lê os mercados"));
        assert!(
            where_to_look_text(&[AffiliatePlatform::Kiwify])
                .starts_with("O Mascate lê sozinho só o mercado da Kiwify, pelo cartão acima.")
        );
        assert!(
            where_to_look_text(&[AffiliatePlatform::Hotmart, AffiliatePlatform::Kiwify])
                .starts_with(
                    "O Mascate lê sozinho os mercados da Hotmart e da Kiwify, pelos cartões acima."
                )
        );
    }

    #[test]
    fn a_field_that_does_not_read_is_named() {
        let bad = Typed {
            temperature: "quente".into(),
            ..typed()
        };
        let error = typed_prospect(AffiliatePlatform::Hotmart, &bad, None).unwrap_err();
        assert!(error.contains("temperatura"), "{error}");
        let no_link = Typed {
            link: "hotmart.com/x".into(),
            ..typed()
        };
        assert!(typed_prospect(AffiliatePlatform::Hotmart, &no_link, None).is_err());
        let negative = Typed {
            price: "-1".into(),
            ..typed()
        };
        assert!(typed_prospect(AffiliatePlatform::Hotmart, &negative, None).is_err());
    }

    #[test]
    fn completing_a_prospect_sends_only_what_changed_under_its_own_key() {
        let found = typed_prospect(AffiliatePlatform::Hotmart, &typed(), None).unwrap();
        let known = Prospect {
            id: RecordId::from_u128(1),
            platform: AffiliatePlatform::Hotmart,
            key: "4021".into(),
            name: found.name.clone(),
            signals: found.signals.clone(),
            readings: Vec::new(),
            score: mascate_affiliate::score(&found.signals, NaiveDate::MIN),
            bet: false,
            program: None,
            chosen: false,
            dismissed: None,
        };
        let mut again = typed_from(&known);
        assert_eq!(
            typed_prospect(AffiliatePlatform::Hotmart, &again, Some(&known))
                .unwrap()
                .signals,
            Signals::default()
        );
        again.commission = "120".into();
        let changed = typed_prospect(AffiliatePlatform::Hotmart, &again, Some(&known)).unwrap();
        assert_eq!(changed.key, "4021");
        assert_eq!(
            changed.signals,
            Signals {
                commission: Some(Money::new(Decimal::from(120), Currency::Brl)),
                ..Signals::default()
            }
        );
    }
}
