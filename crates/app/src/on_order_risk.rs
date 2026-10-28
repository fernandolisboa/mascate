//! The Reputation against the On-Order Listings on the home screen (#209,
//! ADR 0038, decision 5): while a listing sells on order, a warning when
//! one more cancellation or late dispatch takes the green away, or one took
//! it already, with the numbers and a button to pause them all. Nothing
//! pauses or comes back without the owner's click, and pausing keeps the
//! mark. Once the green has room again, the paused ones are a quiet line
//! with the button to reactivate them. Read again after every Sync of the
//! Reputation and of the Orders.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, SharedString, Subscription, Window, div};
use mascate_commerce::{Listing, ListingStatus, OnOrderStatusChange};
use mascate_kernel::SalesChannelKind;
use mascate_marketing::{GreenAtRisk, ReputationMetric};

use crate::appearance::look;
use crate::kit;
use crate::listings::{channel_failure, listings};
use crate::orders::OrderSyncs;
use crate::reputation::{ReputationSyncs, count_text, failure, reputation};
use crate::sales_channels;
use crate::stock_mirror;

/// On-Order Listings are Mercado Livre's, as its Reputation is.
const CHANNEL: SalesChannelKind = SalesChannelKind::MercadoLivre;

/// Pausing the On-Order Listings, or putting them back on sale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Pause,
    Reactivate,
}

pub struct OnOrderRiskArea {
    at_risk: Vec<GreenAtRisk>,
    /// The On-Order Listings, one per channel listing.
    on_order: Vec<Listing>,
    /// The action waiting for the owner to confirm it.
    confirming: Option<Action>,
    busy: bool,
    outcome: Option<(Action, OnOrderStatusChange)>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl OnOrderRiskArea {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe_global::<ReputationSyncs>(Self::refresh),
            cx.observe_global::<OrderSyncs>(Self::refresh),
        ];
        let mut area = Self {
            at_risk: Vec::new(),
            on_order: Vec::new(),
            confirming: None,
            busy: false,
            outcome: None,
            reads: 0,
            error: None,
            _subscriptions: subscriptions,
        };
        area.refresh(cx);
        area
    }

    pub fn is_empty(&self) -> bool {
        self.error.is_none()
            && self.outcome.is_none()
            && (self.on_order.is_empty() || (self.at_risk.is_empty() && self.paused() == 0))
    }

    /// The On-Order Listings a pause takes off sale.
    fn on_sale(&self) -> usize {
        count(&self.on_order, |status| {
            matches!(status, ListingStatus::Active | ListingStatus::OutOfStock)
        })
    }

    fn paused(&self) -> usize {
        count(&self.on_order, |status| status == ListingStatus::Paused)
    }

    /// Reads the Reputation and the On-Order Listings again, as when the
    /// home screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(reputation), Some(listings)) = (reputation(cx), listings(cx)) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            let on_order = listings
                .on_order_on(CHANNEL)
                .await
                .map_err(|e| channel_failure(CHANNEL, &e))?;
            let at_risk = reputation
                .standing()
                .await
                .map_err(|e| failure(&e))?
                .map(|standing| standing.green_at_risk())
                .unwrap_or_default();
            Ok::<_, String>((at_risk, on_order))
        });
        cx.spawn(async move |this, cx| {
            let read_back = reading.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                match read_back {
                    Ok((at_risk, on_order)) => {
                        this.at_risk = at_risk;
                        this.on_order = on_order;
                        this.error = None;
                    }
                    Err(error) => {
                        this.error = Some(
                            format!("Não consegui ler os anúncios sob encomenda: {error}").into(),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn ask(&mut self, action: Action, cx: &mut Context<Self>) {
        self.confirming = Some(action);
        self.outcome = None;
        cx.notify();
    }

    /// Pauses or reactivates every On-Order Listing: only ever on the
    /// owner's confirmed click.
    fn run(&mut self, action: Action, cx: &mut Context<Self>) {
        let (Some(listings), Some(channel)) = (listings(cx), sales_channels::of(cx, CHANNEL))
        else {
            return;
        };
        self.confirming = None;
        self.busy = true;
        cx.notify();
        let running = cx.background_executor().spawn(async move {
            match action {
                Action::Pause => listings.pause_on_order(channel.listings.as_ref()).await,
                Action::Reactivate => {
                    listings
                        .reactivate_on_order(channel.listings.as_ref())
                        .await
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let ran = running.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match ran {
                    Ok(change) => {
                        if !change.changed.is_empty() {
                            // Their units go to, or come back from, the
                            // channel with the next send.
                            stock_mirror::send_after_movement(cx);
                        }
                        this.outcome = Some((action, change));
                    }
                    Err(error) => this.error = Some(channel_failure(CHANNEL, &error).into()),
                }
                this.refresh(cx);
            });
        })
        .detach();
    }
}

fn count(listings: &[Listing], status: impl Fn(ListingStatus) -> bool) -> usize {
    listings
        .iter()
        .filter(|listing| status(listing.listed.status))
        .count()
}

fn cases_text(metric: ReputationMetric, cases: u32) -> String {
    match metric {
        ReputationMetric::Cancellations => count_text(cases, "cancelamento", "cancelamentos"),
        ReputationMetric::DelayedHandling => {
            count_text(cases, "despacho com atraso", "despachos com atraso")
        }
        ReputationMetric::Claims => count_text(cases, "reclamação", "reclamações"),
    }
}

/// "Mais 1 cancelamento tira a sua reputação verde", or that it is gone.
fn risk_title(risk: &GreenAtRisk) -> String {
    let lost = match risk.metric {
        ReputationMetric::Cancellations => "Os cancelamentos já tiraram",
        ReputationMetric::DelayedHandling => "Os despachos com atraso já tiraram",
        ReputationMetric::Claims => "As reclamações já tiraram",
    };
    if risk.lost() {
        format!("{lost} a sua reputação verde")
    } else {
        format!(
            "Mais {} tira a sua reputação verde",
            cases_text(risk.metric, 1)
        )
    }
}

/// "3 cancelamentos em 200 vendas concluídas nos últimos 60 dias; a verde
/// aceita até 3."
fn risk_numbers(risk: &GreenAtRisk) -> String {
    let sales = count_text(risk.sales, "venda concluída", "vendas concluídas");
    let period = match risk.period_days {
        Some(days) => format!("{sales} nos últimos {days} dias"),
        None => sales,
    };
    let allowed = match risk.allowed {
        0 => "a verde não aceita nenhum".to_owned(),
        allowed => format!("a verde aceita até {allowed}"),
    };
    format!(
        "{} em {period}; {allowed}.",
        cases_text(risk.metric, risk.cases)
    )
}

fn listings_text(count: usize) -> String {
    count_text(
        u32::try_from(count).unwrap_or(u32::MAX),
        "anúncio sob encomenda",
        "anúncios sob encomenda",
    )
}

/// "O seu anúncio sob encomenda", "Os seus 3 anúncios sob encomenda".
fn yours(count: usize) -> String {
    match count {
        1 => "O seu anúncio sob encomenda".to_owned(),
        count => format!("Os seus {count} anúncios sob encomenda"),
    }
}

fn is(count: usize) -> &'static str {
    match count {
        1 => "está",
        _ => "estão",
    }
}

fn paused_word(count: usize) -> &'static str {
    match count {
        1 => "pausado",
        _ => "pausados",
    }
}

/// What the owner confirms: the channel and how many listings.
fn confirm_text(action: Action, count: usize) -> String {
    let listings = listings_text(count);
    let on = sales_channels::on(CHANNEL);
    match action {
        Action::Pause => format!(
            "Pausar {listings} {on}? Eles continuam sob encomenda; você reativa quando quiser."
        ),
        Action::Reactivate => format!("Reativar {listings} {on}?"),
    }
}

/// What a pause or a reactivation did, one line per listing left as it was.
fn outcome_lines(action: Action, change: &OnOrderStatusChange) -> (Option<String>, Vec<String>) {
    let on = sales_channels::on(CHANNEL);
    let done = match (action, change.changed.len()) {
        (_, 0) => None,
        (Action::Pause, 1) => Some(format!("1 anúncio sob encomenda pausado {on}.")),
        (Action::Pause, count) => Some(format!("{count} anúncios sob encomenda pausados {on}.")),
        (Action::Reactivate, 1) => Some(format!("1 anúncio sob encomenda reativado {on}.")),
        (Action::Reactivate, count) => {
            Some(format!("{count} anúncios sob encomenda reativados {on}."))
        }
    };
    let verb = match action {
        Action::Pause => "pausar",
        Action::Reactivate => "reativar",
    };
    let failed = change
        .failed
        .iter()
        .map(|(listing, error)| {
            format!(
                "Não consegui {verb} “{}” ({}): {}",
                listing.listed.title,
                listing.listed.id,
                channel_failure(CHANNEL, error)
            )
        })
        .collect();
    (done, failed)
}

impl Render for OnOrderRiskArea {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.is_empty() {
            return div().into_any_element();
        }
        let t = look(cx).tokens;
        let (on_sale, paused) = (self.on_sale(), self.paused());
        let at_risk = !self.at_risk.is_empty() && !self.on_order.is_empty();
        let risks: Vec<AnyElement> = self
            .at_risk
            .iter()
            .filter(|_| at_risk)
            .map(|risk| {
                v_flex()
                    .text_sm()
                    .child(div().font_medium().child(risk_title(risk)))
                    .child(div().text_color(t.text2).child(risk_numbers(risk)))
                    .into_any_element()
            })
            .collect();
        let summary = if at_risk {
            Some(match (on_sale, paused) {
                (0, 0) => format!(
                    "{} não {} à venda agora {}.",
                    yours(self.on_order.len()),
                    is(self.on_order.len()),
                    sales_channels::on(CHANNEL)
                ),
                (0, paused) => format!(
                    "{} {} {} {}.",
                    yours(paused),
                    is(paused),
                    paused_word(paused),
                    sales_channels::on(CHANNEL)
                ),
                (count, _) => format!(
                    "Você vende {} {}: uma venda neles que atrase ou seja cancelada pesa na \
                     reputação. Pausar não tira o sob encomenda.",
                    listings_text(count),
                    sales_channels::on(CHANNEL)
                ),
            })
        } else if paused > 0 {
            Some(format!(
                "A reputação tem folga de novo para cancelamentos e despachos com atraso, e {} \
                 {} {} {}.",
                yours(paused).to_lowercase(),
                match paused {
                    1 => "continua",
                    _ => "continuam",
                },
                paused_word(paused),
                sales_channels::on(CHANNEL)
            ))
        } else {
            None
        };
        let actions = h_flex()
            .gap_2()
            .flex_wrap()
            .items_center()
            .when(at_risk && on_sale > 0 && self.confirming.is_none(), |row| {
                row.child(
                    Button::new("pause-on-order")
                        .label("Pausar anúncios sob encomenda")
                        .danger()
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.ask(Action::Pause, cx))),
                )
            })
            .when(paused > 0 && self.confirming.is_none(), |row| {
                row.child(
                    Button::new("reactivate-on-order")
                        .label("Reativar")
                        .outline()
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.ask(Action::Reactivate, cx))),
                )
            })
            .when_some(self.confirming, |row, action| {
                let count = match action {
                    Action::Pause => on_sale,
                    Action::Reactivate => paused,
                };
                let confirm = Button::new("confirm-on-order")
                    .label(match action {
                        Action::Pause => "Pausar",
                        Action::Reactivate => "Reativar",
                    })
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| this.run(action, cx)));
                row.child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child(confirm_text(action, count)),
                )
                .child(match action {
                    Action::Pause => confirm.danger(),
                    Action::Reactivate => confirm.primary(),
                })
                .child(
                    Button::new("cancel-on-order")
                        .label("Cancelar")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirming = None;
                            cx.notify();
                        })),
                )
            });
        let (done, failed) = self
            .outcome
            .as_ref()
            .map(|(action, change)| outcome_lines(*action, change))
            .unwrap_or_default();
        v_flex()
            .gap_2()
            .child(kit::section_heading("Reputação e anúncios sob encomenda"))
            .when_some(self.error.clone(), |area, error| {
                area.child(kit::error_notice(error, cx))
            })
            .when_some(summary, |area, summary| {
                area.child(
                    v_flex()
                        .gap_2()
                        .p_3()
                        .rounded(t.radius_lg)
                        .border(t.border_width)
                        .border_color(if at_risk { t.danger } else { t.frame })
                        .bg(t.surface)
                        .children(risks)
                        .child(div().text_sm().text_color(t.text2).child(summary))
                        .child(actions),
                )
            })
            .when_some(done, |area, done| area.child(kit::success_notice(done, cx)))
            .children(failed.into_iter().map(|line| kit::error_notice(line, cx)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn risk(metric: ReputationMetric, cases: u32, allowed: u32) -> GreenAtRisk {
        GreenAtRisk {
            metric,
            sales: 200,
            period_days: Some(60),
            cases,
            allowed,
        }
    }

    #[test]
    fn the_warning_says_what_one_more_case_does_with_the_numbers() {
        let edge = risk(ReputationMetric::Cancellations, 3, 3);
        assert_eq!(
            risk_title(&edge),
            "Mais 1 cancelamento tira a sua reputação verde"
        );
        assert_eq!(
            risk_numbers(&edge),
            "3 cancelamentos em 200 vendas concluídas nos últimos 60 dias; a verde aceita até 3."
        );

        let late = risk(ReputationMetric::DelayedHandling, 21, 20);
        assert_eq!(
            risk_title(&late),
            "Os despachos com atraso já tiraram a sua reputação verde"
        );
        assert_eq!(
            risk_numbers(&GreenAtRisk {
                sales: 40,
                period_days: None,
                ..risk(ReputationMetric::Cancellations, 0, 0)
            }),
            "0 cancelamentos em 40 vendas concluídas; a verde não aceita nenhum."
        );
    }

    #[test]
    fn the_confirmation_names_the_channel_and_how_many_listings() {
        assert_eq!(
            confirm_text(Action::Pause, 3),
            "Pausar 3 anúncios sob encomenda no Mercado Livre? Eles continuam sob encomenda; \
             você reativa quando quiser."
        );
        assert_eq!(
            confirm_text(Action::Reactivate, 1),
            "Reativar 1 anúncio sob encomenda no Mercado Livre?"
        );
    }
}
