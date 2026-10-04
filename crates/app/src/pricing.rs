//! Settings › Preços e margens (#19): the tax rate every margin uses (story
//! 73) and the target margin of every Product without one of its own, which
//! Price Suggestions start from. The rules live in Finance and Commerce.

use std::sync::Arc;

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{App, Entity, Global, Window, div, px};
use mascate_commerce::{Pricing, PricingError};
use mascate_finance::Taxes;
use mascate_kernel::Percentage;
use rust_decimal::Decimal;

use crate::appearance::look;
use crate::forms::{Outcome, input, notice, percent_text};

/// The tax rate setting, shared by every screen that works out a margin.
pub struct AppTaxes(pub Arc<Taxes>);

impl Global for AppTaxes {}

/// Target margins and Price Suggestions.
pub struct AppPricing(pub Arc<Pricing>);

impl Global for AppPricing {}

pub fn taxes(cx: &App) -> Option<Arc<Taxes>> {
    cx.try_global::<AppTaxes>().map(|app| app.0.clone())
}

pub fn pricing(cx: &App) -> Option<Arc<Pricing>> {
    cx.try_global::<AppPricing>().map(|app| app.0.clone())
}

/// A target margin as the owner typed it: from 0 up to, not including, 100%.
pub fn parse_target(text: &str) -> Option<Percentage> {
    Percentage::parse(text).filter(|margin| margin.percent() < Decimal::ONE_HUNDRED)
}

/// Why a target margin was not saved, as the owner reads it.
pub fn target_failure(error: &PricingError) -> String {
    match error {
        PricingError::InvalidTarget => "A margem alvo vai de 0 até menos de 100%.".into(),
        error => format!("Não consegui ler ou gravar no banco: {error}"),
    }
}

pub struct PricingSection {
    tax: Entity<InputState>,
    target: Entity<InputState>,
    busy: bool,
    outcome: Option<Outcome>,
}

impl PricingSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let section = Self {
            tax: input("0", window, cx),
            target: input("20", window, cx),
            busy: false,
            outcome: None,
        };
        section.load(window, cx);
        section
    }

    fn load(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(taxes), Some(pricing)) = (taxes(cx), pricing(cx)) else {
            return;
        };
        let reading = cx.background_executor().spawn(async move {
            let tax = taxes.rate().await.map_err(|error| error.to_string())?;
            let target = pricing
                .default_target_margin()
                .await
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((tax, target))
        });
        cx.spawn_in(window, async move |this, cx| {
            let read = reading.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match read {
                    Ok((tax, target)) => {
                        this.tax.update(cx, |input, cx| {
                            input.set_value(percent_text(tax), window, cx)
                        });
                        this.target.update(cx, |input, cx| {
                            input.set_value(percent_text(target), window, cx)
                        });
                    }
                    Err(error) => {
                        this.outcome = Some(Outcome::Failed(
                            format!("Não consegui ler os preços e margens: {error}").into(),
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let (Some(taxes), Some(pricing)) = (taxes(cx), pricing(cx)) else {
            return;
        };
        let tax = Percentage::parse(&self.tax.read(cx).value());
        let target = parse_target(&self.target.read(cx).value());
        let (Some(tax), Some(target)) = (tax, target) else {
            self.outcome = Some(Outcome::Failed(
                "Digite o imposto de 0 a 100% e a margem alvo de 0 até menos de 100% (ex.: 6 \
                 ou 12,5)."
                    .into(),
            ));
            cx.notify();
            return;
        };
        self.busy = true;
        self.outcome = None;
        let saving = cx.background_executor().spawn(async move {
            taxes
                .save_rate(tax)
                .await
                .map_err(|error| format!("Não consegui salvar o imposto: {error}"))?;
            pricing
                .save_default_target_margin(target)
                .await
                .map_err(|error| target_failure(&error))
        });
        cx.spawn(async move |this, cx| {
            let saved = saving.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.outcome = Some(match saved {
                    Ok(()) => Outcome::Done(
                        format!(
                            "Salvo: imposto de {} em todas as margens e margem alvo padrão de {}.",
                            tax.to_pt_br(),
                            target.to_pt_br()
                        )
                        .into(),
                    ),
                    Err(error) => Outcome::Failed(error.into()),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for PricingSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .w(px(180.))
                .child(div().text_sm().font_medium().child(label))
                .child(Input::new(input).small())
        };
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(div().text_sm().text_color(t.text2).child(
                "O imposto sai de cada venda em todos os cálculos de margem: Oportunidades, \
                 sugestão de preço e simulador. Fica em 0% enquanto não houver CNPJ. A margem \
                 alvo padrão vale para todo produto sem margem alvo própria (na ficha do \
                 produto); a sugestão de preço parte dela.",
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(field("Imposto (%)", &self.tax))
                    .child(field("Margem alvo padrão (%)", &self.target))
                    .child(
                        Button::new("save-pricing")
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
