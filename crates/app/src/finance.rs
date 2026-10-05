//! Finance as the screens share it (#28): the sales volume limit, the
//! Reminder it raises and the setting in Configurações › Finanças. The
//! sales come from Commerce and the rule from Finance (ADR 0026).

use std::sync::Arc;

use chrono::TimeDelta;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{App, Entity, Global, Window, div, px};
use mascate_commerce::{Orders, SalesSummary};
use mascate_finance::{
    SALES_VOLUME, SalesVolume, Taxes, VOLUME_WINDOW_DAYS, VolumeCheck, VolumeError,
};
use mascate_kernel::{Clock, Currency, Money, SystemClock, Timestamp, parse_amount};
use mascate_marketing::ProductAds;

use crate::ads;
use crate::appearance::look;
use crate::forms::{Outcome, amount_text, input, notice};
use crate::orders;
use crate::pricing;

/// The app's sales volume limit; absent when the database did not open.
pub struct AppSalesVolume(pub Arc<SalesVolume>);

impl Global for AppSalesVolume {}

pub(crate) fn sales_volume(cx: &App) -> Option<Arc<SalesVolume>> {
    cx.try_global::<AppSalesVolume>().map(|app| app.0.clone())
}

/// What the volume check needs from the rest of the app.
pub(crate) struct VolumeSources {
    pub orders: Arc<Orders>,
    pub taxes: Arc<Taxes>,
    pub volume: Arc<SalesVolume>,
    pub product_ads: Option<Arc<ProductAds>>,
}

impl VolumeSources {
    pub fn of(cx: &App) -> Option<Self> {
        Some(Self {
            orders: orders::orders(cx)?,
            taxes: pricing::taxes(cx)?,
            volume: sales_volume(cx)?,
            product_ads: ads::product_ads(cx),
        })
    }
}

/// The sales of the last 12 months against the owner's limit.
pub(crate) async fn volume(sources: &VolumeSources) -> Result<VolumeCheck, String> {
    let tax = sources
        .taxes
        .rate()
        .await
        .map_err(|e| format!("Não consegui ler a alíquota de imposto: {e}"))?;
    let ad_costs = ads::costs(sources.product_ads.as_deref()).await?;
    let since = SystemClock.now() - TimeDelta::days(VOLUME_WINDOW_DAYS);
    let period = sources
        .orders
        .sales(tax, since..Timestamp::MAX_UTC, &ad_costs)
        .await
        .map_err(|e| orders::failure(&e))?;
    let sold = SalesSummary::of(&period, Currency::Brl)
        .map_err(|e| format!("Vendas em moedas diferentes: {e}"))?
        .revenue;
    sources
        .volume
        .check(sold)
        .await
        .map_err(|e| format!("Não consegui ler o limite de vendas: {e}"))
}

/// The Reminders Finance raises now: the sales volume one, while the sales
/// of the last 12 months are past the limit. A failure raises nothing.
pub(crate) async fn raised_reminders(sources: Option<VolumeSources>) -> Vec<&'static str> {
    let Some(sources) = sources else {
        return Vec::new();
    };
    match volume(&sources).await {
        Ok(check) if check.passed() => vec![SALES_VOLUME.key],
        Ok(_) => Vec::new(),
        Err(error) => {
            eprintln!("could not check the sales volume: {error}");
            Vec::new()
        }
    }
}

/// Configurações › Finanças: the sales limit for the formalization
/// Reminder.
pub struct FinanceSection {
    limit: Entity<InputState>,
    busy: bool,
    outcome: Option<Outcome>,
}

impl FinanceSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let section = Self {
            limit: input("81.000,00", window, cx),
            busy: false,
            outcome: None,
        };
        section.load(window, cx);
        section
    }

    fn load(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(volume) = sales_volume(cx) else {
            return;
        };
        let reading = cx
            .background_executor()
            .spawn(async move { volume.limit().await });
        cx.spawn_in(window, async move |this, cx| {
            let read = reading.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match read {
                    Ok(limit) => this.limit.update(cx, |input, cx| {
                        input.set_value(amount_text(limit), window, cx)
                    }),
                    Err(error) => {
                        this.outcome = Some(Outcome::Failed(
                            format!("Não consegui ler o limite de vendas: {error}").into(),
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(volume) = sales_volume(cx) else {
            return;
        };
        let typed = parse_amount(&self.limit.read(cx).value())
            .map(|amount| Money::new(amount, Currency::Brl));
        let Some(limit) = typed else {
            self.outcome = Some(Outcome::Failed(
                "Digite o limite em reais, acima de zero (ex.: 81.000,00).".into(),
            ));
            cx.notify();
            return;
        };
        self.busy = true;
        self.outcome = None;
        let saving = cx
            .background_executor()
            .spawn(async move { volume.save_limit(limit).await });
        cx.spawn(async move |this, cx| {
            let saved = saving.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.outcome = Some(match saved {
                    Ok(()) => Outcome::Done(
                        format!(
                            "Salvo: o lembrete aparece quando as vendas dos últimos 12 meses \
                             passarem de {}.",
                            limit.to_pt_br()
                        )
                        .into(),
                    ),
                    Err(VolumeError::InvalidLimit) => {
                        Outcome::Failed("O limite precisa ser acima de zero.".into())
                    }
                    Err(error) => Outcome::Failed(
                        format!("Não consegui salvar o limite de vendas: {error}").into(),
                    ),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for FinanceSection {
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
                "Quando as vendas dos últimos 12 meses passarem deste limite, a tela Hoje mostra \
                 um lembrete sobre formalizar o negócio. Ele não bloqueia nada; o Painel mostra \
                 quanto já foi vendido.",
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(260.))
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .child("Limite de vendas em 12 meses (R$)"),
                            )
                            .child(Input::new(&self.limit).small()),
                    )
                    .child(
                        Button::new("save-finance")
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
