//! What the Orders ask of the owner on the home screen (#21): those late to
//! dispatch or close to their time, and the returns waiting for the owner to
//! say they arrived. Read again after every Order Sync.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, EventEmitter, SharedString, Subscription, Window, div};
use mascate_commerce::{DispatchAlert, Order};

use crate::appearance::look;
use crate::catalog;
use crate::kit;
use crate::orders::{OrderSyncs, due_tag, failure, orders, sold_text};
use crate::purchases::units_text;

/// The owner asked to see the Orders.
pub struct OpenOrders;

pub struct OrderAlertsArea {
    dispatch: Vec<DispatchAlert>,
    returning: Vec<Order>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OpenOrders> for OrderAlertsArea {}

impl OrderAlertsArea {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_global::<OrderSyncs>(Self::refresh)];
        let mut area = Self {
            dispatch: Vec::new(),
            returning: Vec::new(),
            reads: 0,
            error: None,
            _subscriptions: subscriptions,
        };
        area.refresh(cx);
        area
    }

    pub fn is_empty(&self) -> bool {
        self.dispatch.is_empty() && self.returning.is_empty() && self.error.is_none()
    }

    /// Reads the Orders again, as when the home screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(orders) = orders(cx) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            let dispatch = orders.dispatch_alerts().await.map_err(|e| failure(&e))?;
            let returning: Vec<Order> = orders
                .orders()
                .await
                .map_err(|e| failure(&e))?
                .into_iter()
                .filter(|order| order.units_awaiting_return() > 0)
                .collect();
            Ok::<_, String>((dispatch, returning))
        });
        cx.spawn(async move |this, cx| {
            let read_back = reading.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                match read_back {
                    Ok((dispatch, returning)) => {
                        this.dispatch = dispatch;
                        this.returning = returning;
                        this.error = None;
                    }
                    Err(error) => {
                        this.error = Some(format!("Não consegui ler os pedidos: {error}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_row(
        &self,
        tag: (SharedString, gpui_kit::Hsla),
        title: String,
        detail: String,
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
            .child(kit::tag(tag.0, tag.1, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .child(div().truncate().font_medium().child(title))
                    .child(div().text_color(t.text2).child(detail)),
            )
            .into_any_element()
    }
}

impl Render for OrderAlertsArea {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.is_empty() {
            return div().into_any_element();
        }
        let t = look(cx).tokens;
        let mut rows = Vec::new();
        for alert in self.dispatch.clone() {
            let (text, ink) = due_tag(alert.due, &t);
            let by = alert
                .order
                .sold
                .shipment
                .as_ref()
                .and_then(|shipment| shipment.dispatch_by)
                .map(catalog::day_and_time)
                .unwrap_or_default();
            rows.push(self.render_row(
                (text.into(), ink),
                sold_text(&alert.order),
                format!("Pedido {} · despachar até {by}", alert.order.sold.id),
                cx,
            ));
        }
        for order in self.returning.clone() {
            rows.push(self.render_row(
                ("Devolução".into(), t.accent_text),
                sold_text(&order),
                format!(
                    "Pedido {} · {} voltando; confirme quando chegar",
                    order.sold.id,
                    units_text(order.units_awaiting_return())
                ),
                cx,
            ));
        }
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(kit::section_heading("Pedidos"))
                    .child(
                        Button::new("open-orders")
                            .label("Ver pedidos")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(OpenOrders))),
                    ),
            )
            .when_some(self.error.clone(), |area, error| {
                area.child(kit::error_notice(error, cx))
            })
            .children(rows)
            .into_any_element()
    }
}
