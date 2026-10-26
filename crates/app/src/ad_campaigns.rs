//! Ad Campaigns (#180) in the Product Ads section of Vendas: each campaign
//! with its status, daily budget, strategy and Target ROAS, and "Nova
//! campanha", "Pausar"/"Ativar" and "Editar". Each change is checked, shown
//! with what it may spend, and sent to Mercado Livre only on the owner's
//! confirmation; a Target ROAS below a Product's Break-even ROAS asks a
//! second time. The Maximum Daily Budget is set in Configurações › Vendas.
//! The rules live in Marketing (ADR 0036).

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Entity, SharedString, Subscription, Task, Window, div, px};
use mascate_kernel::{BreakEven, Currency, Money, PlatformError, Roas, parse_amount};
use mascate_marketing::{
    AdsError, AdvertisedProducts, BudgetSpend, CampaignChange, CampaignPlan, CampaignProduct,
    CampaignRequest, CampaignStatus, CampaignStrategy, ChannelCampaign, ConfirmedCampaign,
    LossToAccept, suggested_target,
};

use crate::ads::{self, AdsSyncs, ad_campaigns, failure, report_sources};
use crate::appearance::look;
use crate::forms::{Outcome, amount_text, input, notice};
use crate::mercado_livre;
use crate::reputation::reputation;
use crate::sales::table_row;

/// What the owner is filling in.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Form {
    New,
    /// New settings for the campaign with this channel id.
    Edit(String),
}

/// Everything the panel shows, read in one go off the UI thread.
struct Snapshot {
    campaigns: Vec<ChannelCampaign>,
    products: AdvertisedProducts,
    maximum: Money,
    unlocked: bool,
}

pub struct AdCampaignsPanel {
    campaigns: Vec<ChannelCampaign>,
    products: AdvertisedProducts,
    maximum: Option<Money>,
    unlocked: bool,
    form: Option<Form>,
    name: Entity<InputState>,
    budget: Entity<InputState>,
    target: Entity<InputState>,
    strategy: CampaignStrategy,
    /// A change checked and waiting for the owner's confirmation.
    pending: Option<CampaignRequest>,
    /// A confirmed change that loses money, waiting for the second one.
    loss: Option<LossToAccept>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl AdCampaignsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_global::<AdsSyncs>(Self::refresh)];
        let mut panel = Self {
            campaigns: Vec::new(),
            products: AdvertisedProducts::default(),
            maximum: None,
            unlocked: false,
            form: None,
            name: input("Nome", window, cx),
            budget: input("20,00", window, cx),
            target: input("4,5", window, cx),
            strategy: CampaignStrategy::Profitability,
            pending: None,
            loss: None,
            busy: false,
            reads: 0,
            outcome: None,
            _subscriptions: subscriptions,
        };
        panel.refresh(cx);
        panel
    }

    /// Reads the campaigns, the Maximum Daily Budget and the Products each
    /// campaign advertised again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(campaigns), Some(sources), Some(reputation)) =
            (ad_campaigns(cx), report_sources(cx), reputation(cx))
        else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            Ok::<_, String>(Snapshot {
                campaigns: campaigns.campaigns().await.map_err(|e| failure(&e))?,
                products: ads::advertised_products(&sources).await?,
                maximum: campaigns
                    .maximum_daily_budget()
                    .await
                    .map_err(|e| failure(&e))?,
                unlocked: ads::unlocked(&reputation).await? == Some(true),
            })
        });
        cx.spawn(async move |this, cx| {
            let snapshot = reading.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                match snapshot {
                    Ok(snapshot) => {
                        this.campaigns = snapshot.campaigns;
                        this.products = snapshot.products;
                        this.maximum = Some(snapshot.maximum);
                        this.unlocked = snapshot.unlocked;
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The Products of the campaign the form or a change is about: those
    /// the campaign advertised, or for a new one all the owner advertises.
    fn products_of(&self, campaign: Option<&str>) -> &[CampaignProduct] {
        match campaign {
            Some(id) => self.products.by_campaign.get(id).map_or(&[], Vec::as_slice),
            None => &self.products.all,
        }
    }

    fn campaign(&self, id: &str) -> Option<&ChannelCampaign> {
        self.campaigns.iter().find(|campaign| campaign.id == id)
    }

    fn open_form(&mut self, form: Form, window: &mut Window, cx: &mut Context<Self>) {
        let editing = match &form {
            Form::Edit(id) => self.campaign(id).cloned(),
            Form::New => None,
        };
        let products = self.products_of(editing.as_ref().map(|c| c.id.as_str()));
        let target = editing
            .as_ref()
            .and_then(|campaign| campaign.target)
            .or_else(|| suggested_target(products));
        let values = [
            (
                &self.name,
                editing
                    .as_ref()
                    .map_or_else(String::new, |campaign| campaign.name.clone()),
            ),
            (
                &self.budget,
                editing
                    .as_ref()
                    .and_then(|campaign| campaign.budget)
                    .map_or_else(String::new, amount_text),
            ),
            (&self.target, target.map_or_else(String::new, target_text)),
        ];
        for (field, value) in values {
            field.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        self.strategy = editing
            .as_ref()
            .and_then(|campaign| campaign.strategy)
            .unwrap_or(CampaignStrategy::Profitability);
        self.form = Some(form);
        self.pending = None;
        self.loss = None;
        self.outcome = None;
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.form = None;
        self.pending = None;
        self.loss = None;
        cx.notify();
    }

    fn use_suggestion(&mut self, target: Roas, window: &mut Window, cx: &mut Context<Self>) {
        self.target.update(cx, |input, cx| {
            input.set_value(target_text(target), window, cx)
        });
        cx.notify();
    }

    fn fail(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.outcome = Some(Outcome::Failed(text.into()));
        cx.notify();
    }

    /// Checks what the form holds. Nothing goes to Mercado Livre yet.
    fn check(&mut self, cx: &mut Context<Self>) {
        let (Some(form), Some(campaigns)) = (self.form.clone(), ad_campaigns(cx)) else {
            return;
        };
        let value = |field: &Entity<InputState>| field.read(cx).value().to_string();
        let Some(budget) = parse_amount(&value(&self.budget)) else {
            return self.fail("Digite o orçamento diário em reais, como 20,00.", cx);
        };
        let Some(target) = parse_amount(&value(&self.target)).and_then(Roas::new) else {
            return self.fail("Digite o ROAS alvo como 4,5 (de 1 a 35).", cx);
        };
        let editing = match &form {
            Form::Edit(id) => self.campaign(id).cloned(),
            Form::New => None,
        };
        let plan = CampaignPlan {
            name: value(&self.name),
            status: match editing.as_ref().map(|campaign| campaign.status) {
                Some(CampaignStatus::Paused) => CampaignStatus::Paused,
                _ => CampaignStatus::Active,
            },
            budget: Money::new(budget, Currency::Brl),
            strategy: self.strategy,
            target,
        };
        let products = self
            .products_of(editing.as_ref().map(|c| c.id.as_str()))
            .to_vec();
        let unlocked = self.unlocked;
        self.prepare(
            cx.background_executor().spawn(async move {
                match form {
                    Form::New => campaigns.prepare_campaign(plan, &products, unlocked).await,
                    Form::Edit(id) => {
                        campaigns
                            .prepare_change(&id, plan, &products, unlocked)
                            .await
                    }
                }
            }),
            cx,
        );
    }

    /// Checks pausing or activating a campaign.
    fn ask_status(&mut self, id: String, status: CampaignStatus, cx: &mut Context<Self>) {
        let Some(campaigns) = ad_campaigns(cx) else {
            return;
        };
        self.form = None;
        let products = self.products_of(Some(&id)).to_vec();
        let unlocked = self.unlocked;
        self.prepare(
            cx.background_executor().spawn(async move {
                campaigns
                    .prepare_status(&id, status, &products, unlocked)
                    .await
            }),
            cx,
        );
    }

    fn prepare(
        &mut self,
        preparing: Task<Result<CampaignRequest, AdsError>>,
        cx: &mut Context<Self>,
    ) {
        self.busy = true;
        self.pending = None;
        self.loss = None;
        self.outcome = None;
        cx.spawn(async move |this, cx| {
            let prepared = preparing.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match prepared {
                    Ok(request) => this.pending = Some(request),
                    Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The owner confirmed the change: it goes, unless it loses money on a
    /// Product, which asks once more.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.pending.take() else {
            return;
        };
        match request.confirm() {
            Ok(confirmed) => self.send(confirmed, cx),
            Err(loss) => {
                self.loss = Some(loss);
                cx.notify();
            }
        }
    }

    fn accept_loss(&mut self, cx: &mut Context<Self>) {
        if let Some(loss) = self.loss.take() {
            self.send(loss.accept_loss(), cx);
        }
    }

    /// Sends the confirmed change: only ever on the owner's click.
    fn send(&mut self, confirmed: ConfirmedCampaign, cx: &mut Context<Self>) {
        let (Some(campaigns), Some(channel)) = (ad_campaigns(cx), mercado_livre::adapter(cx))
        else {
            return self.fail(mercado_livre::failure(&PlatformError::NotConnected), cx);
        };
        self.busy = true;
        self.outcome = None;
        let sending = cx
            .background_executor()
            .spawn(async move { campaigns.send(channel.as_ref(), confirmed).await });
        cx.spawn(async move |this, cx| {
            let sent = sending.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match sent {
                    Ok(campaign) => {
                        this.form = None;
                        this.outcome = Some(Outcome::Done(
                            format!("Enviado ao Mercado Livre: {}.", campaign_line(&campaign))
                                .into(),
                        ));
                        cx.default_global::<AdsSyncs>().finished += 1;
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_campaign(
        &self,
        index: usize,
        campaign: &ChannelCampaign,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let products = self.products_of(Some(&campaign.id));
        let advertised = (!products.is_empty()).then(|| {
            div().text_xs().text_color(t.text2).child(format!(
                "Produtos: {}.",
                products
                    .iter()
                    .map(product_text)
                    .collect::<Vec<_>>()
                    .join("; ")
            ))
        });
        let runs = matches!(
            campaign.status,
            CampaignStatus::Active | CampaignStatus::Paused
        );
        let (status_label, next) = match campaign.status {
            CampaignStatus::Active => ("Pausar", CampaignStatus::Paused),
            _ => ("Ativar", CampaignStatus::Active),
        };
        let id = campaign.id.clone();
        let edit_id = campaign.id.clone();
        let buttons =
            runs.then(|| {
                h_flex()
                    .gap_1()
                    .flex_none()
                    .child(
                        Button::new(("campaign-status", index))
                            .label(status_label)
                            .outline()
                            .small()
                            .disabled(self.busy || !self.unlocked)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ask_status(id.clone(), next, cx)
                            })),
                    )
                    .child(
                        Button::new(("campaign-edit", index))
                            .label("Editar")
                            .ghost()
                            .small()
                            .disabled(self.busy || !self.unlocked)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_form(Form::Edit(edit_id.clone()), window, cx)
                            })),
                    )
            });
        v_flex()
            .gap_1()
            .child(
                table_row(&t)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_medium()
                            .child(campaign_line(campaign)),
                    )
                    .children(buttons),
            )
            .children(advertised.map(|line| line.px_3()))
            .into_any_element()
    }

    fn render_form(&self, form: &Form, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, input: &Entity<InputState>, width: f32| {
            v_flex()
                .gap_1()
                .w(px(width))
                .child(div().text_sm().font_medium().child(label))
                .child(Input::new(input).small())
        };
        let strategies = CampaignStrategy::ALL.into_iter().map(|strategy| {
            let button = Button::new(SharedString::from(format!("strategy-{}", strategy.code())))
                .label(strategy.name())
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.strategy = strategy;
                    cx.notify();
                }));
            if strategy == self.strategy {
                button.primary()
            } else {
                button.outline()
            }
        });
        let campaign = match form {
            Form::Edit(id) => Some(id.as_str()),
            Form::New => None,
        };
        let products = self.products_of(campaign);
        let suggestion = match suggested_target(products) {
            Some(target) => h_flex()
                .gap_2()
                .items_center()
                .flex_wrap()
                .child(div().text_sm().child(format!(
                    "ROAS alvo sugerido: {} — o maior ROAS de equilíbrio entre {}, arredondado \
                     para cima.",
                    target.to_pt_br(),
                    match campaign {
                        Some(_) => "os produtos desta campanha",
                        None => "os produtos que você anuncia hoje",
                    }
                )))
                .child(
                    Button::new("use-suggested-target")
                        .label("Usar sugestão")
                        .ghost()
                        .small()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.use_suggestion(target, window, cx)
                        })),
                )
                .into_any_element(),
            None => div()
                .text_sm()
                .text_color(t.text2)
                .child(
                    "Sem um ROAS de equilíbrio conhecido para sugerir: nenhum anúncio lido nesta \
                     campanha tem custo e preço no app. Abaixo do ROAS de equilíbrio de um \
                     produto, cada venda pelo anúncio dá prejuízo.",
                )
                .into_any_element(),
        };
        let maximum = self.maximum.map(|maximum| {
            div().text_xs().text_color(t.text2).child(format!(
                "Teto do orçamento diário: {}; mude em Configurações › Vendas › Product Ads. \
                 Estratégia: rentabilidade mostra menos anúncios a quem tende a comprar; \
                 crescimento equilibra; visibilidade mostra a mais gente, para anúncio novo.",
                maximum.to_pt_br()
            ))
        });
        v_flex()
            .gap_3()
            .pt_3()
            .border_t(t.border_width)
            .border_color(t.frame)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(field("Nome", &self.name, 240.))
                    .child(field("Orçamento diário (R$)", &self.budget, 170.))
                    .child(field("ROAS alvo (x)", &self.target, 120.)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().font_medium().child("Estratégia"))
                    .child(h_flex().gap_1().flex_wrap().children(strategies)),
            )
            .child(suggestion)
            .children(maximum)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("check-campaign")
                            .label("Conferir")
                            .icon(IconName::Search)
                            .outline()
                            .small()
                            .loading(self.busy && self.pending.is_none())
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.check(cx))),
                    )
                    .child(
                        Button::new("close-campaign")
                            .label("Cancelar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_pending(&self, request: &CampaignRequest, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let spend = request
            .spend()
            .map(|spend| div().text_sm().child(spend_text(&spend)));
        let suggested = request.suggested().map(|target| {
            div().text_xs().text_color(t.text2).child(format!(
                "ROAS alvo sugerido pelos produtos: {}.",
                target.to_pt_br()
            ))
        });
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.accent)
            .child(
                div()
                    .font_medium()
                    .child("Conferido. Confirme para enviar:"),
            )
            .child(div().text_sm().child(request_text(request)))
            .children(spend)
            .children(suggested)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("send-campaign")
                            .label("Confirmar e enviar ao Mercado Livre")
                            .icon(IconName::Check)
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                    )
                    .child(
                        Button::new("back-campaign")
                            .label("Voltar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pending = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_loss(&self, loss: &LossToAccept, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let request = loss.request();
        let target = request.target();
        let lead = match target {
            Some(target) => format!(
                "Com ROAS alvo de {}, cada venda pelo anúncio dá prejuízo em:",
                target.to_pt_br()
            ),
            None => "Cada venda pelo anúncio dá prejuízo em:".to_owned(),
        };
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.danger)
            .child(
                div()
                    .font_medium()
                    .text_color(t.danger)
                    .child("Segunda confirmação: esta campanha dá prejuízo"),
            )
            .child(div().text_sm().child(lead))
            .children(request.losing().iter().map(|product| {
                div()
                    .text_sm()
                    .pl_3()
                    .child(format!("• {}", product_text(product)))
            }))
            .child(div().text_xs().text_color(t.text2).child(
                "O ROAS alvo é a meta que o Mercado Livre persegue. Às vezes vale um prejuízo \
                 planejado para dar visibilidade a um anúncio novo; o ROAS real aparece todo dia \
                 nesta seção, com a marca \"dá prejuízo\".",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("accept-loss")
                            .label("Enviar mesmo assim")
                            .danger()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.accept_loss(cx))),
                    )
                    .child(
                        Button::new("refuse-loss")
                            .label("Voltar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.loss = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for AdCampaignsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let new = Button::new("new-campaign")
            .label("Nova campanha")
            .icon(IconName::Plus)
            .small()
            .disabled(self.busy || !self.unlocked)
            .on_click(cx.listener(|this, _, window, cx| this.open_form(Form::New, window, cx)));
        let new = if self.form == Some(Form::New) {
            new.primary()
        } else {
            new.outline()
        };
        let rows: Vec<AnyElement> = self
            .campaigns
            .clone()
            .iter()
            .enumerate()
            .map(|(index, campaign)| self.render_campaign(index, campaign, cx))
            .collect();
        let empty = rows.is_empty().then(|| {
            div()
                .text_sm()
                .text_color(t.text2)
                .child("Nenhuma campanha no Mercado Livre ainda.")
        });
        let form = self.form.clone().map(|form| self.render_form(&form, cx));
        let pending = self
            .pending
            .clone()
            .map(|request| self.render_pending(&request, cx));
        let loss = self.loss.clone().map(|loss| self.render_loss(&loss, cx));
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(div().font_medium().child("Campanhas"))
                    .child(new),
            )
            .child(div().text_xs().text_color(t.text2).child(
                "Nada muda no Mercado Livre sem a sua confirmação: cada campanha nova, pausa, \
                 ativação ou edição é conferida e mostrada antes de enviar.",
            ))
            .children(empty)
            .children(rows)
            .children(form)
            .children(pending)
            .children(loss)
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
    }
}

/// "Fones · Ativa · R$ 30,00 por dia · Rentabilidade · ROAS alvo 5,00x".
fn campaign_line(campaign: &ChannelCampaign) -> String {
    let mut parts = vec![campaign.name.clone(), campaign.status.name().to_owned()];
    if let Some(budget) = campaign.budget {
        parts.push(format!("{} por dia", budget.to_pt_br()));
    }
    if let Some(strategy) = campaign.strategy {
        parts.push(strategy.name().to_owned());
    }
    if let Some(target) = campaign.target {
        parts.push(format!("ROAS alvo {}", target.to_pt_br()));
    }
    parts.join(" · ")
}

/// What goes to Mercado Livre, in words.
fn request_text(request: &CampaignRequest) -> String {
    let name = request.name();
    match (request.change(), request.plan()) {
        (CampaignChange::Pause, _) => format!(
            "Pausar a campanha “{name}” no Mercado Livre: os anúncios dela param de aparecer até \
             você ativar de novo."
        ),
        (CampaignChange::Activate, _) => {
            format!(
                "Ativar a campanha “{name}” no Mercado Livre: os anúncios dela voltam a gastar."
            )
        }
        (change, Some(plan)) => format!(
            "{} “{name}” no Mercado Livre: {}, {} por dia, {}, ROAS alvo {}.",
            if change == CampaignChange::Create {
                "Nova campanha"
            } else {
                "Campanha"
            },
            plan.status.name().to_lowercase(),
            plan.budget.to_pt_br(),
            plan.strategy.name().to_lowercase(),
            plan.target.to_pt_br()
        ),
        (_, None) => format!("Campanha “{name}”."),
    }
}

/// "O Mercado Livre pode gastar até R$ 40,00 num dia ... e cerca de
/// R$ 600,00 no mês."
fn spend_text(spend: &BudgetSpend) -> String {
    format!(
        "Orçamento de {} por dia. O Mercado Livre pode gastar até {} num dia, usando a sobra de \
         dias anteriores, e cerca de {} no mês.",
        spend.daily.to_pt_br(),
        spend.most_in_a_day.to_pt_br(),
        spend.about_a_month.to_pt_br()
    )
}

/// "Fone Bluetooth (equilíbrio 4,32x)".
fn product_text(product: &CampaignProduct) -> String {
    let break_even = match product.break_even {
        Some(BreakEven::At(roas)) => format!("equilíbrio {}", roas.to_pt_br()),
        Some(BreakEven::Never) => "nenhum ROAS paga o anúncio".to_owned(),
        None => "equilíbrio desconhecido".to_owned(),
    };
    format!("{} ({break_even})", product.name)
}

/// "4,4": a Target ROAS as the owner types it.
fn target_text(target: Roas) -> String {
    target.ratio().normalize().to_string().replace('.', ",")
}

/// Where the owner sets the Maximum Daily Budget, in Configurações › Vendas.
pub struct AdsSettingsSection {
    maximum: Entity<InputState>,
    busy: bool,
    outcome: Option<Outcome>,
}

impl AdsSettingsSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let section = Self {
            maximum: input("50,00", window, cx),
            busy: false,
            outcome: None,
        };
        section.load(window, cx);
        section
    }

    fn load(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(campaigns) = ad_campaigns(cx) else {
            return;
        };
        let reading = cx
            .background_executor()
            .spawn(async move { campaigns.maximum_daily_budget().await });
        cx.spawn_in(window, async move |this, cx| {
            let read = reading.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match read {
                    Ok(maximum) => this.maximum.update(cx, |input, cx| {
                        input.set_value(amount_text(maximum), window, cx)
                    }),
                    Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(campaigns) = ad_campaigns(cx) else {
            return;
        };
        let Some(amount) = parse_amount(&self.maximum.read(cx).value()) else {
            self.outcome = Some(Outcome::Failed(
                "Digite o teto em reais, como 50,00.".into(),
            ));
            cx.notify();
            return;
        };
        let maximum = Money::new(amount, Currency::Brl);
        self.busy = true;
        self.outcome = None;
        let saving = cx
            .background_executor()
            .spawn(async move { campaigns.save_maximum_daily_budget(maximum).await });
        cx.spawn(async move |this, cx| {
            let saved = saving.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.outcome = Some(match saved {
                    Ok(()) => Outcome::Done(
                        format!(
                            "Salvo: nenhuma campanha passa de {} por dia.",
                            maximum.to_pt_br()
                        )
                        .into(),
                    ),
                    Err(error) => Outcome::Failed(failure(&error).into()),
                });
                cx.default_global::<AdsSyncs>().finished += 1;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for AdsSettingsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(div().text_sm().text_color(t.text2).child(
                "O teto do orçamento diário recusa qualquer campanha do Product Ads com orçamento \
                 acima dele, para um zero a mais digitado não virar gasto. O Mercado Livre ainda \
                 pode gastar até o dobro num dia, usando a sobra de dias anteriores.",
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(220.))
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .child("Teto do orçamento diário (R$)"),
                            )
                            .child(Input::new(&self.maximum).small()),
                    )
                    .child(
                        Button::new("save-ads-settings")
                            .label("Salvar")
                            .outline()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use mascate_marketing::{CampaignStrategy, MIN_TARGET_ROAS};
    use rust_decimal::Decimal;

    use super::*;

    fn brl(amount: &str) -> Money {
        Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
    }

    #[test]
    fn a_campaign_reads_with_what_the_channel_said_of_it() {
        let campaign = ChannelCampaign {
            id: "351".into(),
            name: "Fones".into(),
            status: CampaignStatus::Active,
            budget: Some(brl("30")),
            strategy: Some(CampaignStrategy::Profitability),
            target: Roas::new(Decimal::from(5)),
        };
        assert_eq!(
            campaign_line(&campaign),
            "Fones · Ativa · R$ 30,00 por dia · Rentabilidade · ROAS alvo 5,00x"
        );
        assert_eq!(
            campaign_line(&ChannelCampaign::named(
                "353",
                "Campanha 353",
                CampaignStatus::Other
            )),
            "Campanha 353 · Outro estado"
        );
    }

    #[test]
    fn the_spend_reads_the_day_the_worst_day_and_the_month() {
        assert_eq!(
            spend_text(&BudgetSpend::of(brl("20"))),
            "Orçamento de R$ 20,00 por dia. O Mercado Livre pode gastar até R$ 40,00 num dia, \
             usando a sobra de dias anteriores, e cerca de R$ 600,00 no mês."
        );
    }

    #[test]
    fn a_target_is_typed_back_as_the_owner_writes_it() {
        assert_eq!(target_text(Roas::new(Decimal::new(44, 1)).unwrap()), "4,4");
        assert_eq!(target_text(Roas::new(MIN_TARGET_ROAS).unwrap()), "1");
    }
}
