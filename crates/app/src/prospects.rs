//! Garimpo (#219, ADR 0039): affiliate Prospects ranked by their score,
//! each part with its reason, the apostas in a filter of their own, the
//! owner's own entries from any Platform's market, the Hotmart vitrine
//! read on the owner's click while its flag is on (#220), and the way from
//! a Prospect to the Passo a passo or to the bin. The ranking and every
//! rule are the Affiliate's; this screen only calls it.

use std::sync::Arc;

use chrono::NaiveDate;
use chrono::Utc;
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
use mascate_integrations::HOTMART_CRAWLER;
use mascate_kernel::{Currency, Money, Percentage, RecordId, parse_amount};
use rust_decimal::Decimal;

use crate::affiliate::{affiliate, failure};
use crate::affiliate_markets::{MARKET_PLATFORMS, Market, market};
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
    /// The vitrine's category "Garimpar" reads; `None` for its best
    /// sellers.
    category: Option<ProspectCategory>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    /// The same, for the reads of the vitrine's flag.
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
        self.read_vitrine_flag(cx);
        self.run(window, cx, |_| async { Ok(Changed::Nothing) });
    }

    /// Reads the vitrine's flag again: on, it builds the reader; off, it
    /// drops it.
    fn read_vitrine_flag(&mut self, cx: &mut Context<Self>) {
        let (Some(app), Some(crawlers)) = (cx.try_global::<AppFlags>(), crawlers(cx)) else {
            return;
        };
        let flags = app.flags.clone();
        let (vitrine, engine) = (crawlers.hotmart_vitrine.clone(), crawlers.engine.clone());
        self.flag_reads += 1;
        let reading = self.flag_reads;
        let working = cx
            .background_executor()
            .spawn(async move { crawlers::vitrine_if_on(&flags, &vitrine, engine).await });
        cx.spawn(async move |this, cx| {
            let source = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.flag_reads == reading {
                    this.vitrine = source.ok().flatten();
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
            Ok(Changed::Read(read_text(category.as_ref(), read)))
        });
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
                    "O Mascate ainda não lê esse mercado sozinho. Passe por ele e traga os \
                     números para cá:",
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
                .child(div().text_xs().text_color(t.text2).child(
                    "O Mascate ainda não lê os mercados sozinho. Abra o de cada plataforma e \
                     traga os números em Cadastrar à mão; escolha uma plataforma acima para ver \
                     o passo a passo dela.",
                ))
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
        let chip = |index: usize, label: String, category: Option<ProspectCategory>| {
            let on = self.category.as_ref().map(|c| &c.id) == category.as_ref().map(|c| &c.id);
            Button::new(("vitrine-category", index))
                .label(label)
                .small()
                .map(|b| if on { b.primary() } else { b.ghost() })
                .disabled(off)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.category = category.clone();
                    cx.notify();
                }))
        };
        let categories = source.categories().unwrap_or_default();
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
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child(chip(0, "Mais vendidos".into(), None))
                    .children(categories.into_iter().enumerate().map(|(index, category)| {
                        chip(index + 1, category.name.clone(), Some(category))
                    })),
            )
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
        } else if self.vitrine.is_some() && self.shows_vitrine(cx) {
            "Nenhum produto no garimpo ainda. Escolha uma categoria da vitrine e clique em \
             Garimpar, ou traga os números do mercado em Cadastrar à mão."
        } else {
            "Nenhum produto no garimpo ainda. Abra o mercado da plataforma, escolha os que \
             parecem bons e traga os números em Cadastrar à mão."
        };
        let mut list = v_flex()
            .gap_3()
            .children(platforms)
            .children(vitrine)
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

/// What one "Garimpar" in `category` (the best sellers without one) did.
pub fn read_text(category: Option<&ProspectCategory>, read: ProspectsRead) -> String {
    let place = category.map_or("nos mais vendidos".to_owned(), |c| format!("em {}", c.name));
    if read.found == 0 {
        return format!("A vitrine da Hotmart não mostrou nenhum produto {place}.");
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
    format!("Li {found} da vitrine {place}: {added}.{dismissed}")
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
    use super::*;

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
            read_text(Some(&cooking), read(24, 5, 2)),
            "Li 24 produtos da vitrine em Culinária e gastronomia: 5 novos no garimpo. 2 que \
             você descartou continuam de fora."
        );
        assert_eq!(
            read_text(None, read(1, 0, 0)),
            "Li 1 produto da vitrine nos mais vendidos: nenhum novo; os que já estavam foram \
             atualizados."
        );
        assert_eq!(
            read_text(None, read(3, 1, 1)),
            "Li 3 produtos da vitrine nos mais vendidos: 1 novo no garimpo. 1 que você \
             descartou continua de fora."
        );
        assert_eq!(
            read_text(Some(&cooking), read(0, 0, 0)),
            "A vitrine da Hotmart não mostrou nenhum produto em Culinária e gastronomia."
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
