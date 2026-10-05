//! The Reminders on the home screen (#7): fiscal and legal notes in a
//! quiet corner, each dismissable until its interval runs out. Some show
//! only while raised, like the sales volume one (#28); each Order Sync
//! checks again.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Global, SharedString, Subscription, Window, div};
use mascate_platform::{Reminder, ReminderError, ReminderTopic, Reminders};

use crate::appearance::look;
use crate::finance::{self, VolumeSources};
use crate::kit;
use crate::orders::OrderSyncs;

/// The app's Reminders; absent when the database did not open.
pub struct AppReminders(pub Arc<Reminders>);

impl Global for AppReminders {}

fn topic_name(topic: ReminderTopic) -> &'static str {
    match topic {
        ReminderTopic::Fiscal => "Fiscal",
        ReminderTopic::Legal => "Jurídico",
    }
}

pub struct RemindersArea {
    showing: Vec<Reminder>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl RemindersArea {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_global::<OrderSyncs>(Self::refresh)];
        let mut area = Self {
            showing: Vec::new(),
            reads: 0,
            error: None,
            _subscriptions: subscriptions,
        };
        area.refresh(cx);
        area
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.run(cx, |_| async { Ok(()) });
    }

    fn dismiss(&mut self, key: &'static str, cx: &mut Context<Self>) {
        // Gone at once; the read that follows confirms it.
        self.showing.retain(|reminder| reminder.key != key);
        self.run(
            cx,
            move |reminders| async move { reminders.dismiss(key).await },
        );
    }

    /// Runs a change off the UI thread, then reads what shows now.
    fn run<F, Fut>(&mut self, cx: &mut Context<Self>, change: F)
    where
        F: FnOnce(Arc<Reminders>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), ReminderError>> + Send,
    {
        let Some(app) = cx.try_global::<AppReminders>() else {
            return;
        };
        let reminders = app.0.clone();
        let sources = VolumeSources::of(cx);
        self.reads += 1;
        let read = self.reads;
        let working = cx.background_executor().spawn(async move {
            let outcome = change(reminders.clone()).await;
            let raised = finance::raised_reminders(sources).await;
            (outcome, reminders.showing(&raised).await)
        });
        cx.spawn(async move |this, cx| {
            let (outcome, showing) = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                this.error = None;
                match showing {
                    Ok(showing) => this.showing = showing,
                    Err(error) => {
                        this.error = Some(format!("Não consegui ler os lembretes: {error}").into());
                    }
                }
                if let Err(error) = outcome {
                    this.error = Some(format!("Não consegui dispensar o lembrete: {error}").into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_reminder(&self, reminder: Reminder, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let days = reminder.reappears_after_days;
        h_flex()
            .gap_3()
            .items_start()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .text_sm()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(kit::tag(topic_name(reminder.topic), t.text2, cx))
                            .child(div().font_medium().child(reminder.title)),
                    )
                    .child(div().text_color(t.text2).child(reminder.text)),
            )
            .child(
                Button::new(reminder.key)
                    .label("Dispensar")
                    .tooltip(format!("Volta em {days} dias."))
                    .ghost()
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| this.dismiss(reminder.key, cx))),
            )
            .into_any_element()
    }
}

impl Render for RemindersArea {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        if self.showing.is_empty() && self.error.is_none() {
            return div().into_any_element();
        }
        let items: Vec<AnyElement> = self
            .showing
            .clone()
            .into_iter()
            .map(|reminder| self.render_reminder(reminder, cx))
            .collect();
        v_flex()
            .gap_2()
            .child(div().text_sm().text_color(t.text2).child("Lembretes"))
            .when_some(self.error.clone(), |area, error| {
                area.child(kit::error_notice(error, cx))
            })
            .children(items)
            .into_any_element()
    }
}
