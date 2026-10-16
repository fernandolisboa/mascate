//! Automatic Repricing (#135, ADR 0033): turned on per listing, below its
//! price, with a minimum and a maximum the owner confirms; a minimum that
//! loses the Minimum Margin is refused. Mercado Livre reprices with its own
//! automation, and the panel shows its status and the price to win; on
//! Amazon the app follows the featured offer after each Order Sync. Shopee
//! and Magalu tell no competitor's price. Every listing's price history
//! shows below. Off behind `commerce.automatic_repricing` until the owner
//! turns it on in Configurações › Funcionalidades restritas. The rules live
//! in Commerce.

use std::sync::Arc;

use chrono::Local;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, Global, Task, Window, div, px};
use mascate_catalog::{Catalog, Product};
use mascate_commerce::{
    PriceChange, PriceChangeReason, RepricedListing, Repricing, RepricingError, RepricingLimits,
    RepricingRequest, RepricingStatus,
};
use mascate_finance::Taxes;
use mascate_kernel::{Currency, Money, RecordId, SalesChannelKind, parse_amount};

use crate::appearance::look;
use crate::catalog;
use crate::forms::{Outcome, amount_text, input, notice};
use crate::kit;
use crate::listings::{breakdown_line, channel_failure, price_failure};
use crate::pricing;
use crate::sales_channels::{self, ChannelPorts, ChannelRepricing};

/// The app's Automatic Repricing; absent when the database did not open.
pub struct AppRepricing(pub Arc<Repricing>);

impl Global for AppRepricing {}

pub fn repricing(cx: &App) -> Option<Arc<Repricing>> {
    cx.try_global::<AppRepricing>().map(|app| app.0.clone())
}

/// What a round of Automatic Repricing on every connected channel did.
#[derive(Debug, Default)]
pub struct RepricingOutcome {
    /// Prices the app sent to follow a featured offer.
    pub sent: usize,
    /// Why a channel, or a listing on it, failed; the others went on.
    pub failures: Vec<String>,
}

/// A round of Automatic Repricing off the UI thread, right after an Order
/// Sync; see `round`. `None` when the database did not open.
pub fn sync_now(cx: &mut App) -> Option<Task<RepricingOutcome>> {
    let (Some(repricing), Some(catalog), Some(taxes)) =
        (repricing(cx), catalog::catalog(cx), pricing::taxes(cx))
    else {
        return None;
    };
    let reach = sales_channels::reach(cx);
    Some(
        cx.background_executor().spawn(async move {
            round(&repricing, &catalog, &taxes, &reach.connected().await).await
        }),
    )
}

/// Reads Mercado Livre's automation of each listing repriced there and
/// follows Amazon's featured offers, once per Sync: the Average Cost the
/// Order Sync just moved is the one each Minimum Margin check uses. With
/// the flag off, each channel's repricing ends instead.
pub async fn round(
    repricing: &Repricing,
    catalog: &Catalog,
    taxes: &Taxes,
    channels: &[ChannelPorts],
) -> RepricingOutcome {
    let mut outcome = RepricingOutcome::default();
    for channel in channels {
        let kind = channel.kind();
        let done = match &channel.repricing {
            None => continue,
            Some(ChannelRepricing::Automation(port)) => {
                repricing.check_automations(port.as_ref()).await
            }
            Some(ChannelRepricing::FeaturedOffer(port)) => {
                match pricing::assumptions(catalog, taxes).await {
                    Ok(assumptions) => {
                        repricing
                            .follow_featured_offers(
                                port.as_ref(),
                                channel.listings.as_ref(),
                                assumptions,
                            )
                            .await
                    }
                    Err(error) => {
                        outcome.failures.push(error);
                        continue;
                    }
                }
            }
        };
        match done {
            Ok(done) => {
                outcome.sent += done.sent;
                outcome.failures.extend(
                    done.failed
                        .iter()
                        .map(|(_, error)| sales_channels::failure(kind, error)),
                );
            }
            Err(error) => outcome.failures.push(failure(&error, kind, None, &[])),
        }
    }
    outcome
}

pub fn finished(outcome: &RepricingOutcome) {
    for failure in &outcome.failures {
        eprintln!("automatic repricing: {failure}");
    }
}

/// " 2 preços seguiram a oferta em destaque.": what a round adds to a Sync's
/// message, nothing when no price went.
pub fn sent_text(outcome: &RepricingOutcome) -> String {
    match outcome.sent {
        0 => String::new(),
        1 => " 1 preço seguiu a oferta em destaque da Amazon.".into(),
        count => format!(" {count} preços seguiram a oferta em destaque da Amazon."),
    }
}

/// Why repricing a listing on `kind` failed, as the owner reads it.
/// `listing` is the one being repriced, when there is one.
pub fn failure(
    error: &RepricingError,
    kind: SalesChannelKind,
    listing: Option<RecordId>,
    products: &[Product],
) -> String {
    match error {
        RepricingError::Off => "A reprecificação automática está desligada. Ligue em \
             Configurações › Funcionalidades restritas."
            .into(),
        RepricingError::InvalidLimits => {
            "O mínimo precisa ser maior que zero e no máximo igual ao máximo, em reais.".into()
        }
        RepricingError::BelowMinimumMargin(check) => {
            let below = format!(
                "No mínimo de {} a margem fica abaixo da mínima de {}.",
                check.sale.price.to_pt_br(),
                check.minimum.to_pt_br()
            );
            match check.lowest {
                Some(lowest) => format!(
                    "{below} O menor preço que mantém a margem mínima é {}.",
                    lowest.to_pt_br()
                ),
                None => format!("{below} Nenhum preço mantém a margem mínima hoje."),
            }
        }
        RepricingError::OtherChannel { listing, channel } => format!(
            "Esse anúncio é {}, não {}; a lista foi atualizada.",
            sales_channels::of_channel(*listing),
            sales_channels::of_channel(*channel)
        ),
        RepricingError::Listing(error) => channel_failure(kind, error),
        RepricingError::Pricing(error) => match listing {
            Some(listing) => price_failure(error, kind, listing, products),
            None => format!("Não consegui calcular a margem: {error}"),
        },
        RepricingError::Platform(error) => sales_channels::failure(kind, error),
        RepricingError::Flag(_) | RepricingError::Unreadable(_) | RepricingError::Sql(_) => {
            format!("Não consegui ler ou gravar a reprecificação no banco: {error}")
        }
    }
}

/// Where the repricing of a listing on `kind` stands, as the owner reads it.
pub fn status_text(kind: SalesChannelKind, repriced: &RepricedListing) -> String {
    let Some(reading) = &repriced.reading else {
        return "Começa no próximo Sync.".into();
    };
    let channel = sales_channels::the(kind);
    let status = match &reading.status {
        RepricingStatus::Following if kind == SalesChannelKind::MercadoLivre => {
            format!("Ativa: {channel} ajusta o preço entre o mínimo e o máximo.")
        }
        RepricingStatus::Following => {
            "Ativa: o app segue a oferta em destaque depois de cada Sync.".into()
        }
        RepricingStatus::PausedByPromotion => {
            format!("Pausada: {channel} pausa a automação enquanto o anúncio está em promoção.")
        }
        RepricingStatus::Paused(why) if why.is_empty() => {
            format!("Pausada {}.", sales_channels::by(kind))
        }
        RepricingStatus::Paused(why) => format!("Pausada {}: {why}", sales_channels::by(kind)),
        RepricingStatus::NoCompetition => {
            "Nenhuma outra oferta disputa o destaque; o preço fica como está.".into()
        }
        RepricingStatus::NotEligible => format!(
            "{} diz que esta oferta não pode ficar em destaque; o preço fica como está.",
            sales_channels::opening(kind)
        ),
        RepricingStatus::MarginOutOfReach => "Nenhum preço entre o mínimo e o máximo mantém a \
             margem mínima agora; o preço fica como está."
            .into(),
    };
    match reading.winning_price {
        Some(price) if kind == SalesChannelKind::MercadoLivre => {
            format!("{status} Preço para ganhar: {}.", price.to_pt_br())
        }
        Some(price) => format!(
            "{status} Preço esperado da oferta em destaque: {}.",
            price.to_pt_br()
        ),
        None => status,
    }
}

/// Why a price changed, as the history shows it.
pub fn reason_text(kind: SalesChannelKind, reason: PriceChangeReason) -> String {
    match reason {
        PriceChangeReason::Approved => "aprovado por você".into(),
        PriceChangeReason::FeaturedOffer => "seguiu a oferta em destaque".into(),
        PriceChangeReason::ChannelAutomation => {
            format!("automação de preço {}", sales_channels::of_channel(kind))
        }
        PriceChangeReason::InChannel => format!("mudou {}", sales_channels::on(kind)),
    }
}

/// "16/10/2026 14:05 · R$ 80,00 → R$ 87,35 · seguiu a oferta em destaque".
pub fn change_text(kind: SalesChannelKind, change: &PriceChange) -> String {
    format!(
        "{} · {} → {} · {}",
        change.at.with_timezone(&Local).format("%d/%m/%Y %H:%M"),
        change.old.to_pt_br(),
        change.new.to_pt_br(),
        reason_text(kind, change.reason)
    )
}

/// What turning repricing on will do on `kind`, for the owner to confirm.
fn turn_on_text(kind: SalesChannelKind, limits: &RepricingLimits) -> String {
    let between = format!(
        "entre {} e {}",
        limits.minimum.to_pt_br(),
        limits.maximum.to_pt_br()
    );
    match kind {
        SalesChannelKind::MercadoLivre => format!(
            "{} passa a mudar o preço sozinho {between}, seguindo o menor preço dentro e fora \
             dele. Enquanto estiver ligada, o app não envia preço a este anúncio.",
            sales_channels::opening(kind)
        ),
        _ => format!(
            "Depois de cada Sync o app leva o preço ao da oferta em destaque {between}, sem \
             baixar da margem mínima; no máximo um preço por Sync."
        ),
    }
}

/// The limits as the owner typed them, in reais.
fn parse_limits(minimum: &str, maximum: &str) -> Option<RepricingLimits> {
    let money = |text: &str| Some(Money::new(parse_amount(text)?, Currency::Brl));
    Some(RepricingLimits {
        minimum: money(minimum)?,
        maximum: money(maximum)?,
    })
}

/// How many changes the history shows.
const HISTORY_SHOWN: usize = 10;

/// A listing's Automatic Repricing and price history, below its price.
pub struct RepricingPanel {
    listing: RecordId,
    kind: SalesChannelKind,
    /// The Products the failures name by SKU.
    products: Vec<Product>,
    /// `None` until the first read finishes.
    on: Option<bool>,
    repriced: Option<RepricedListing>,
    history: Vec<PriceChange>,
    minimum: Entity<InputState>,
    maximum: Entity<InputState>,
    /// Limits checked, waiting for the owner to confirm them.
    confirming: Option<RepricingRequest>,
    busy: bool,
    outcome: Option<Outcome>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
}

impl RepricingPanel {
    pub fn new(
        listing: RecordId,
        kind: SalesChannelKind,
        products: Vec<Product>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self {
            listing,
            kind,
            products,
            on: None,
            repriced: None,
            history: Vec::new(),
            minimum: input("Mínimo (R$)", window, cx),
            maximum: input("Máximo (R$)", window, cx),
            confirming: None,
            busy: false,
            outcome: None,
            reads: 0,
        };
        panel.read(window, cx);
        panel
    }

    /// Whether repricing sets this listing's price, so no price of the
    /// owner's goes.
    pub fn is_repriced(&self) -> bool {
        self.repriced.is_some()
    }

    fn port(&self, cx: &App) -> Option<ChannelRepricing> {
        sales_channels::of(cx, self.kind).and_then(|channel| channel.repricing)
    }

    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repricing) = repricing(cx) else {
            return;
        };
        let listing = self.listing;
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            Ok::<_, RepricingError>((
                repricing.is_on().await?,
                repricing.repricing_of(listing).await?,
                repricing.price_history(listing).await?,
            ))
        });
        cx.spawn_in(window, async move |this, cx| {
            let read_back = reading.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads != read {
                    return;
                }
                match read_back {
                    Ok((on, repriced, history)) => {
                        if let Some(repriced) = &repriced {
                            for (field, amount) in [
                                (&this.minimum, repriced.limits.minimum),
                                (&this.maximum, repriced.limits.maximum),
                            ] {
                                field.update(cx, |field, cx| {
                                    field.set_value(amount_text(amount), window, cx)
                                });
                            }
                        }
                        this.on = Some(on);
                        this.repriced = repriced;
                        this.history = history;
                    }
                    Err(error) => {
                        this.outcome = Some(Outcome::Failed(
                            failure(&error, this.kind, Some(this.listing), &this.products).into(),
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Checks the typed limits against the Minimum Margin; nothing is sent.
    fn check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(repricing), Some(channel), Some(catalog), Some(taxes)) = (
            repricing(cx),
            sales_channels::of(cx, self.kind),
            catalog::catalog(cx),
            pricing::taxes(cx),
        ) else {
            return;
        };
        let Some(limits) = parse_limits(
            &self.minimum.read(cx).value(),
            &self.maximum.read(cx).value(),
        ) else {
            self.outcome = Some(Outcome::Failed(
                "Digite o mínimo e o máximo em reais, como 59,90.".into(),
            ));
            cx.notify();
            return;
        };
        let listing = self.listing;
        self.busy = true;
        self.outcome = None;
        let checking = cx.background_executor().spawn(async move {
            let assumptions = pricing::assumptions(&catalog, &taxes).await.map_err(Err)?;
            repricing
                .prepare(listing, limits, channel.listings.as_ref(), assumptions)
                .await
                .map_err(Ok)
        });
        cx.spawn_in(window, async move |this, cx| {
            let checked = checking.await;
            let _ = this.update_in(cx, |this, _, cx| {
                this.busy = false;
                match checked {
                    Ok(request) => this.confirming = Some(request),
                    Err(error) => {
                        let text = match error {
                            Ok(error) => failure(&error, this.kind, Some(listing), &this.products),
                            Err(text) => text,
                        };
                        this.outcome = Some(Outcome::Failed(text.into()));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Turns repricing on with the limits the owner just confirmed.
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(repricing), Some(port), Some(request)) =
            (repricing(cx), self.port(cx), self.confirming.take())
        else {
            return;
        };
        let confirmed = request.confirm();
        self.act(
            window,
            cx,
            "Reprecificação automática ligada.",
            async move { repricing.turn_on(confirmed, port.port()).await.map(|_| ()) },
        );
    }

    /// Turns repricing off; the price stays where it is.
    fn turn_off(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(repricing), Some(port)) = (repricing(cx), self.port(cx)) else {
            return;
        };
        let listing = self.listing;
        self.confirming = None;
        self.act(
            window,
            cx,
            "Reprecificação automática desligada; o preço fica onde está.",
            async move { repricing.turn_off(listing, port.port()).await },
        );
    }

    fn act(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: &'static str,
        acting: impl Future<Output = Result<(), RepricingError>> + Send + 'static,
    ) {
        self.busy = true;
        self.outcome = None;
        let acting = cx.background_executor().spawn(acting);
        cx.spawn_in(window, async move |this, cx| {
            let acted = acting.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.outcome = Some(match acted {
                    Ok(()) => Outcome::Done(done.into()),
                    Err(error) => Outcome::Failed(
                        failure(&error, this.kind, Some(this.listing), &this.products).into(),
                    ),
                });
                this.read(window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.confirming = None;
        cx.notify();
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .w(px(120.))
                .child(div().text_xs().font_medium().child(label))
                .child(Input::new(input).small())
        };
        let mut controls = v_flex().gap_2();
        if let Some(repriced) = &self.repriced {
            controls = controls
                .child(div().text_sm().child(status_text(self.kind, repriced)))
                .child(div().text_xs().text_color(t.text2).child(format!(
                    "Entre {} e {}, desde {}.",
                    repriced.limits.minimum.to_pt_br(),
                    repriced.limits.maximum.to_pt_br(),
                    repriced.since.with_timezone(&Local).format("%d/%m/%Y")
                )));
        }
        if let Some(request) = &self.confirming {
            let check = request.check();
            let lowest = check.lowest.map(|lowest| {
                format!(
                    "O menor preço que mantém a margem mínima de {} é {}.",
                    check.minimum.to_pt_br(),
                    lowest.to_pt_br()
                )
            });
            return controls
                .child(
                    div()
                        .text_sm()
                        .child(turn_on_text(self.kind, &request.limits())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text2)
                        .child("Venda no mínimo:"),
                )
                .child(breakdown_line(&check.breakdown, cx))
                .children(lowest.map(|text| div().text_xs().text_color(t.text2).child(text)))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("confirm-repricing")
                                .label("Confirmar e ligar")
                                .icon(IconName::Check)
                                .primary()
                                .small()
                                .loading(self.busy)
                                .disabled(self.busy)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.confirm(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("cancel-repricing")
                                .label("Cancelar")
                                .ghost()
                                .small()
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                        ),
                )
                .into_any_element();
        }
        if self.on == Some(false) {
            let off = if self.repriced.is_some() {
                "A reprecificação automática foi desligada em Configurações › Funcionalidades \
                 restritas; ela se encerra neste anúncio no próximo Sync."
            } else {
                "Desligada em Configurações › Funcionalidades restritas. Ligue lá para o preço \
                 deste anúncio seguir a concorrência entre um mínimo e um máximo que você \
                 confirma."
            };
            return controls.child(kit::info_notice(off, cx)).into_any_element();
        }
        let check_label = if self.repriced.is_some() {
            "Mudar os limites"
        } else {
            "Conferir os limites"
        };
        controls
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .flex_wrap()
                    .child(field("Mínimo (R$)", &self.minimum))
                    .child(field("Máximo (R$)", &self.maximum))
                    .child(
                        Button::new("check-repricing")
                            .label(check_label)
                            .outline()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy || self.on.is_none())
                            .on_click(cx.listener(|this, _, window, cx| this.check(window, cx))),
                    )
                    .children(self.repriced.is_some().then(|| {
                        Button::new("turn-off-repricing")
                            .label("Desligar")
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.turn_off(window, cx)))
                    })),
            )
            .child(div().text_xs().text_color(t.text2).child(
                "O mínimo precisa manter a margem mínima de Configurações › Preços e margens \
                 depois da tarifa, do frete, do imposto e do custo médio.",
            ))
            .into_any_element()
    }

    fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let mut history = v_flex()
            .gap_1()
            .child(div().text_sm().font_medium().child("Histórico de preço"));
        if self.history.is_empty() {
            history = history.child(
                div()
                    .text_xs()
                    .text_color(t.text2)
                    .child("Nenhuma mudança de preço registrada ainda."),
            );
        }
        history
            .children(self.history.iter().take(HISTORY_SHOWN).map(|change| {
                div()
                    .text_xs()
                    .text_color(t.text2)
                    .child(change_text(self.kind, change))
            }))
            .into_any_element()
    }
}

impl Render for RepricingPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut panel = v_flex()
            .gap_2()
            .child(div().font_medium().child("Reprecificação automática"));
        panel = match (self.on, self.port(cx)) {
            (_, None) => panel.child(kit::info_notice(
                format!(
                    "{} não informa pela API o preço dos concorrentes, então a reprecificação \
                     automática não vale para este anúncio.",
                    sales_channels::opening(self.kind)
                ),
                cx,
            )),
            (None, Some(_)) => panel.child(
                div()
                    .text_sm()
                    .text_color(t.text2)
                    .child("Lendo a reprecificação…"),
            ),
            (Some(_), Some(_)) => panel.child(self.render_controls(cx)),
        };
        panel
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .child(self.render_history(cx))
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use mascate_commerce::RepricingReading;
    use mascate_commerce::RepricingWay;

    use super::*;

    fn brl(amount: &str) -> Money {
        Money::new(amount.parse().unwrap(), Currency::Brl)
    }

    fn repriced(
        kind: SalesChannelKind,
        status: RepricingStatus,
        winning: Option<&str>,
    ) -> RepricedListing {
        let at = Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap();
        RepricedListing {
            channel: kind,
            listing: "MLB4100000001".into(),
            way: RepricingWay::ChannelAutomation,
            limits: RepricingLimits {
                minimum: brl("60"),
                maximum: brl("120"),
            },
            since: at,
            reading: Some(RepricingReading {
                status,
                winning_price: winning.map(brl),
                at,
            }),
        }
    }

    #[test]
    fn the_limits_read_as_the_owner_types_them() {
        assert_eq!(
            parse_limits("59,90", "R$ 120"),
            Some(RepricingLimits {
                minimum: brl("59.90"),
                maximum: brl("120"),
            })
        );
        assert_eq!(parse_limits("", "120"), None);
        assert_eq!(parse_limits("60", "cento e vinte"), None);
    }

    #[test]
    fn the_status_names_the_channel_its_pause_and_the_winning_price() {
        let ml = SalesChannelKind::MercadoLivre;
        assert_eq!(
            status_text(ml, &repriced(ml, RepricingStatus::Following, Some("92.50"))),
            "Ativa: o Mercado Livre ajusta o preço entre o mínimo e o máximo. Preço para \
             ganhar: R$ 92,50."
        );
        assert!(
            status_text(ml, &repriced(ml, RepricingStatus::PausedByPromotion, None))
                .starts_with("Pausada: o Mercado Livre pausa a automação enquanto o anúncio")
        );
        assert_eq!(
            status_text(
                ml,
                &repriced(
                    ml,
                    RepricingStatus::Paused("Item paused message".into()),
                    None
                )
            ),
            "Pausada pelo Mercado Livre: Item paused message"
        );
        let amazon = SalesChannelKind::Amazon;
        assert_eq!(
            status_text(
                amazon,
                &repriced(amazon, RepricingStatus::Following, Some("87.35"))
            ),
            "Ativa: o app segue a oferta em destaque depois de cada Sync. Preço esperado da \
             oferta em destaque: R$ 87,35."
        );
        let mut waiting = repriced(amazon, RepricingStatus::Following, None);
        waiting.reading = None;
        assert_eq!(status_text(amazon, &waiting), "Começa no próximo Sync.");
    }

    #[test]
    fn each_change_says_why() {
        let ml = SalesChannelKind::MercadoLivre;
        assert_eq!(
            reason_text(ml, PriceChangeReason::ChannelAutomation),
            "automação de preço do Mercado Livre"
        );
        assert_eq!(
            reason_text(SalesChannelKind::Amazon, PriceChangeReason::InChannel),
            "mudou na Amazon"
        );
        assert_eq!(
            reason_text(ml, PriceChangeReason::Approved),
            "aprovado por você"
        );
    }

    #[test]
    fn a_failure_says_what_to_do() {
        assert_eq!(
            failure(
                &RepricingError::InvalidLimits,
                SalesChannelKind::Amazon,
                None,
                &[]
            ),
            "O mínimo precisa ser maior que zero e no máximo igual ao máximo, em reais."
        );
        assert!(
            failure(&RepricingError::Off, SalesChannelKind::Amazon, None, &[])
                .contains("Funcionalidades restritas")
        );
    }

    #[test]
    fn a_round_tells_how_many_prices_followed_the_featured_offer() {
        let mut outcome = RepricingOutcome::default();
        assert_eq!(sent_text(&outcome), "");
        outcome.sent = 2;
        assert_eq!(
            sent_text(&outcome),
            " 2 preços seguiram a oferta em destaque da Amazon."
        );
    }
}
