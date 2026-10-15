//! The Sales Events on the home screen (#105): each event inside its lead
//! time, the soonest first, with its notes, to plan Promotions and Offer
//! Posts for it. Read again whenever the calendar changes.

use chrono::Utc;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, EventEmitter, SharedString, Subscription, Window, div};
use mascate_marketing::UpcomingEvent;

use crate::appearance::look;
use crate::kit;
use crate::sales_events::{EventChanges, days_away_text, event_date_text, failure, sales_events};

/// The owner asked to see the calendar.
pub struct OpenCalendar;

pub struct EventAlertsArea {
    near: Vec<UpcomingEvent>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OpenCalendar> for EventAlertsArea {}

impl EventAlertsArea {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_global::<EventChanges>(Self::refresh)];
        let mut area = Self {
            near: Vec::new(),
            reads: 0,
            error: None,
            _subscriptions: subscriptions,
        };
        area.refresh(cx);
        area
    }

    pub fn is_empty(&self) -> bool {
        self.near.is_empty() && self.error.is_none()
    }

    /// Reads the events inside their lead time again, as when the home
    /// screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(events) = sales_events(cx) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        let reading = cx
            .background_executor()
            .spawn(async move { events.in_lead_time(Utc::now()).await });
        cx.spawn(async move |this, cx| {
            let read_back = reading.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                match read_back {
                    Ok(near) => {
                        this.near = near;
                        this.error = None;
                    }
                    Err(error) => {
                        this.error = Some(
                            format!("Não consegui ler o calendário: {}", failure(&error)).into(),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_item(
        &self,
        index: usize,
        near: &UpcomingEvent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        h_flex()
            .gap_3()
            .items_center()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(kit::tag(days_away_text(near.days_away), t.accent_text, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .child(
                        div()
                            .truncate()
                            .font_medium()
                            .child(near.event.name.clone()),
                    )
                    .child(div().text_color(t.text2).child(event_date_text(near.date)))
                    .when(!near.event.notes.is_empty(), |cell| {
                        cell.child(div().child(near.event.notes.clone()))
                    }),
            )
            .child(
                Button::new(("open-calendar", index))
                    .label("Ver calendário")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(OpenCalendar))),
            )
            .into_any_element()
    }
}

impl Render for EventAlertsArea {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.is_empty() {
            return div().into_any_element();
        }
        let items: Vec<AnyElement> = self
            .near
            .iter()
            .enumerate()
            .map(|(index, near)| self.render_item(index, near, cx))
            .collect();
        v_flex()
            .gap_2()
            .child(kit::section_heading("Datas de venda chegando"))
            .when_some(self.error.clone(), |area, error| {
                area.child(kit::error_notice(error, cx))
            })
            .children(items)
            .into_any_element()
    }
}
