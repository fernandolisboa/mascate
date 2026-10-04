//! Settings › Funcionalidades restritas (#7): every flag with why it is off
//! and the risk of turning it on. Turning one on shows the risk and asks
//! for confirmation in place; the rule lives in the platform module.

use std::sync::Arc;

use chrono::Local;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Global, SharedString, Window, div};
use mascate_platform::{FlagChange, FlagError, FlagKind, FlagStatus, Flags, TurnOnRequest};

use crate::appearance::look;
use crate::kit;

/// The app's flags and the system user who switches them; absent when the
/// database did not open.
pub struct AppFlags {
    pub flags: Arc<Flags>,
    pub user: SharedString,
}

impl Global for AppFlags {}

fn kind_name(kind: FlagKind) -> &'static str {
    match kind {
        FlagKind::RestrictedFeature => "Termos ou lei",
        FlagKind::ProductChoice => "Escolha sua",
    }
}

fn change_line(change: &FlagChange) -> String {
    let at = change.at.with_timezone(&Local).format("%d/%m/%Y às %H:%M");
    let what = if change.on { "Ligada" } else { "Desligada" };
    format!("{what} em {at} por {}.", change.by)
}

pub struct RestrictedFeaturesSection {
    /// `None` until the first read finishes.
    statuses: Option<Vec<FlagStatus>>,
    /// The flag waiting for the owner to confirm its risk.
    confirming: Option<TurnOnRequest>,
    /// Numbers each switch or read, so a slow one never lands over a newer one.
    reads: u64,
    busy: bool,
    error: Option<SharedString>,
}

impl RestrictedFeaturesSection {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut section = Self {
            statuses: None,
            confirming: None,
            reads: 0,
            busy: false,
            error: None,
        };
        section.run(cx, |_| async { Ok(()) });
        section
    }

    fn ask_to_turn_on(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let Some(app) = cx.try_global::<AppFlags>() else {
            return;
        };
        match app.flags.request_turn_on(key) {
            Ok(request) => self.confirming = Some(request),
            Err(error) => self.error = Some(error.to_string().into()),
        }
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.confirming.take() else {
            return;
        };
        let by = cx.global::<AppFlags>().user.to_string();
        let confirmed = request.confirm(by);
        self.run(
            cx,
            move |flags| async move { flags.turn_on(confirmed).await },
        );
    }

    fn turn_off(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let by = cx.global::<AppFlags>().user.to_string();
        self.run(
            cx,
            move |flags| async move { flags.turn_off(key, &by).await },
        );
    }

    /// Runs a switch off the UI thread, then reads every flag again.
    /// Reading alone is a switch that does nothing.
    fn run<F, Fut>(&mut self, cx: &mut Context<Self>, switch: F)
    where
        F: FnOnce(Arc<Flags>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), FlagError>> + Send,
    {
        let Some(app) = cx.try_global::<AppFlags>() else {
            return;
        };
        let flags = app.flags.clone();
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        self.error = None;
        let working = cx.background_executor().spawn(async move {
            let outcome = switch(flags.clone()).await;
            (outcome, flags.statuses().await)
        });
        cx.spawn(async move |this, cx| {
            let (outcome, statuses) = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                this.busy = false;
                match statuses {
                    Ok(statuses) => this.statuses = Some(statuses),
                    Err(error) => {
                        this.error = Some(format!("Não consegui ler as flags: {error}").into());
                    }
                }
                if let Err(error) = outcome {
                    this.error = Some(format!("Não consegui mudar a flag: {error}").into());
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_flag(&self, index: usize, status: &FlagStatus, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let flag = status.flag;
        let on = status.is_on();
        let confirming = self
            .confirming
            .as_ref()
            .is_some_and(|request| request.flag().key == flag.key);

        let header = h_flex()
            .gap_2()
            .flex_wrap()
            .items_center()
            .child(div().font_medium().child(flag.name))
            .child(kit::tag(kind_name(flag.kind), t.text2, cx))
            .child(kit::tag(
                format!("Fase {}", flag.phase.number()),
                t.text2,
                cx,
            ))
            .child(div().flex_1())
            .child(if on {
                kit::tag("Ligada", t.accent_text, cx)
            } else {
                kit::tag("Desligada", t.text2, cx)
            });

        let line = |label: &'static str, text: &'static str| {
            div()
                .text_sm()
                .child(div().font_medium().child(label))
                .child(div().text_color(t.text2).child(text))
        };

        let actions = if confirming {
            v_flex()
                .gap_2()
                .p_3()
                .rounded(t.radius_lg)
                .border(t.border_width)
                .border_color(t.danger)
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .child("Ligar esta funcionalidade?"),
                )
                .child(div().text_sm().text_color(t.danger).child(flag.risk))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new(("confirm-flag", index))
                                .label("Entendo o risco, ligar")
                                .danger()
                                .small()
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                        )
                        .child(
                            Button::new(("cancel-flag", index))
                                .label("Cancelar")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.confirming = None;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element()
        } else if on {
            Button::new(("turn-off", index))
                .label("Desligar")
                .outline()
                .small()
                .loading(self.busy)
                .disabled(self.busy)
                .on_click(cx.listener(move |this, _, _, cx| this.turn_off(flag.key, cx)))
                .into_any_element()
        } else {
            Button::new(("turn-on", index))
                .label("Ligar…")
                .outline()
                .small()
                .loading(self.busy)
                .disabled(self.busy)
                .on_click(cx.listener(move |this, _, _, cx| this.ask_to_turn_on(flag.key, cx)))
                .into_any_element()
        };

        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(header)
            .child(line("Por que vem desligada", flag.reason))
            // While confirming, the risk shows in the confirmation instead.
            .when(!confirming, |card| {
                card.child(line("Risco de ligar", flag.risk))
            })
            .when_some(status.last_change.as_ref(), |card, change| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(t.text2)
                        .child(change_line(change)),
                )
            })
            .child(h_flex().child(actions))
            .into_any_element()
    }
}

impl Render for RestrictedFeaturesSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let intro = div().text_sm().text_color(t.text2).child(
            "Construídas e testadas, mas desligadas de propósito. Ligar uma delas mostra o \
             risco e pede sua confirmação.",
        );
        let body: Vec<AnyElement> = if cx.try_global::<AppFlags>().is_none() {
            vec![
                kit::error_notice(
                    "O banco de dados não abriu, então tudo continua desligado.",
                    cx,
                )
                .into_any_element(),
            ]
        } else {
            match &self.statuses {
                None => vec![
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("Lendo…")
                        .into_any_element(),
                ],
                Some(statuses) => statuses
                    .iter()
                    .enumerate()
                    .map(|(index, status)| self.render_flag(index, status, cx))
                    .collect(),
            }
        };
        v_flex()
            .gap_3()
            .child(intro)
            .when_some(self.error.clone(), |section, error| {
                section.child(kit::error_notice(error, cx))
            })
            .children(body)
    }
}
