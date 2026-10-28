//! What the Orders ask of the owner on the home screen (#21): the sales on
//! On-Order Listings still waiting for their units, to buy or bought (#208),
//! those late to dispatch or close to their time, and the returns waiting
//! for the owner to say they arrived. Read again after every Order Sync.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, EventEmitter, SharedString, Subscription, Window, div};
use mascate_commerce::{DispatchAlert, DispatchDue, OnOrderSale, Order, Orders};

use crate::appearance::look;
use crate::catalog;
use crate::kit;
use crate::orders::{OrderSyncs, asks_to_buy, due_tag, failure, on_order_text, orders, sold_text};
use crate::preferences;
use crate::purchases::{BuyForSale, units_text};
use crate::work_context::WorkContext;

/// The owner asked to see the Orders.
pub struct OpenOrders;

/// A sale on an On-Order Listing still waiting for its units, and how close
/// its Order is to its Dispatch Deadline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OnOrderAlert {
    pub sale: OnOrderSale,
    pub due: Option<DispatchDue>,
}

/// What the Orders ask of the owner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OrderAlerts {
    /// Sales on On-Order Listings waiting for their units, the soonest to
    /// dispatch first.
    pub on_order: Vec<OnOrderAlert>,
    /// Late to dispatch or close to their time, the soonest first; an
    /// Order with a sale on order shows there instead.
    pub dispatch: Vec<DispatchAlert>,
    /// Waiting for the owner to say the units came back.
    pub returning: Vec<Order>,
}

impl OrderAlerts {
    /// Reads what every channel's Orders ask of the owner.
    pub(crate) async fn read(orders: &Orders) -> Result<Self, String> {
        let (on_order, dispatch) = with_sales_on_order(
            orders.on_order_sales().await.map_err(|e| failure(&e))?,
            orders.dispatch_alerts().await.map_err(|e| failure(&e))?,
        );
        let returning = orders
            .orders()
            .await
            .map_err(|e| failure(&e))?
            .into_iter()
            .filter(|order| order.units_awaiting_return() > 0)
            .collect();
        Ok(Self {
            on_order,
            dispatch,
            returning,
        })
    }

    /// Those of the Orders on a channel `context` shows.
    pub(crate) fn in_context(&self, context: WorkContext) -> Self {
        Self {
            on_order: self
                .on_order
                .iter()
                .filter(|alert| context.shows_channel(alert.sale.order.channel))
                .cloned()
                .collect(),
            dispatch: self
                .dispatch
                .iter()
                .filter(|alert| context.shows_channel(alert.order.channel))
                .cloned()
                .collect(),
            returning: self
                .returning
                .iter()
                .filter(|order| context.shows_channel(order.channel))
                .cloned()
                .collect(),
        }
    }

    fn is_empty(&self) -> bool {
        self.on_order.is_empty() && self.dispatch.is_empty() && self.returning.is_empty()
    }
}

/// Each sale on order with how close its Order is to its time, and the
/// other Orders to dispatch: one row per Order, the purchase first.
fn with_sales_on_order(
    sales: Vec<OnOrderSale>,
    mut dispatch: Vec<DispatchAlert>,
) -> (Vec<OnOrderAlert>, Vec<DispatchAlert>) {
    let on_order: Vec<OnOrderAlert> = sales
        .into_iter()
        .map(|sale| OnOrderAlert {
            due: dispatch
                .iter()
                .find(|alert| alert.order.id == sale.order.id)
                .map(|alert| alert.due),
            sale,
        })
        .collect();
    dispatch.retain(|alert| {
        !on_order
            .iter()
            .any(|found| found.sale.order.id == alert.order.id)
    });
    (on_order, dispatch)
}

pub struct OrderAlertsArea {
    alerts: OrderAlerts,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OpenOrders> for OrderAlertsArea {}

impl EventEmitter<BuyForSale> for OrderAlertsArea {}

impl OrderAlertsArea {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_global::<OrderSyncs>(Self::refresh)];
        let mut area = Self {
            alerts: OrderAlerts::default(),
            reads: 0,
            error: None,
            _subscriptions: subscriptions,
        };
        area.refresh(cx);
        area
    }

    /// Whether nothing on a channel `context` shows asks for the owner.
    pub fn is_empty_in(&self, context: WorkContext) -> bool {
        self.alerts.in_context(context).is_empty() && self.error.is_none()
    }

    /// Reads the Orders again, as when the home screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(orders) = orders(cx) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        let reading = cx
            .background_executor()
            .spawn(async move { OrderAlerts::read(&orders).await });
        cx.spawn(async move |this, cx| {
            let read_back = reading.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                match read_back {
                    Ok(alerts) => {
                        this.alerts = alerts;
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
        action: Option<AnyElement>,
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
            .children(action)
            .into_any_element()
    }
}

impl Render for OrderAlertsArea {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let context = preferences::work_context(cx);
        if self.is_empty_in(context) {
            return div().into_any_element();
        }
        let t = look(cx).tokens;
        let OrderAlerts {
            on_order,
            dispatch,
            returning,
        } = self.alerts.in_context(context);
        let mut rows = Vec::new();
        for (index, OnOrderAlert { sale, due }) in on_order.into_iter().enumerate() {
            let (text, ink) = due.map_or(("Sob encomenda", t.accent_text), |due| due_tag(due, &t));
            let title = sold_text(&sale.order);
            let detail = format!("Pedido {} · {}", sale.order.sold.id, on_order_text(&sale));
            let action = asks_to_buy(&sale).then(|| {
                Button::new(("buy-for-sale", index))
                    .label("Comprar para este pedido")
                    .primary()
                    .small()
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(BuyForSale(sale.clone()))))
                    .into_any_element()
            });
            rows.push(self.render_row((text.into(), ink), title, detail, action, cx));
        }
        for alert in dispatch {
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
                None,
                cx,
            ));
        }
        for order in returning {
            rows.push(self.render_row(
                ("Devolução".into(), t.accent_text),
                sold_text(&order),
                format!(
                    "Pedido {} · {} voltando; confirme quando chegar",
                    order.sold.id,
                    units_text(order.units_awaiting_return())
                ),
                None,
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

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use mascate_kernel::SalesChannelKind;

    use super::*;
    use crate::testing::{Store, sold_on_order};

    fn channels(alerts: &OrderAlerts) -> Vec<SalesChannelKind> {
        alerts
            .dispatch
            .iter()
            .map(|alert| alert.order.channel)
            .collect()
    }

    #[test]
    fn an_order_sold_on_order_shows_once_with_its_purchase_and_how_close_it_is() {
        let close = sold_on_order(10);
        let far = sold_on_order(20);
        let other = sold_on_order(30).order;
        let dispatch = vec![
            DispatchAlert {
                order: close.order.clone(),
                due: DispatchDue::Late,
            },
            DispatchAlert {
                order: other.clone(),
                due: DispatchDue::Soon,
            },
        ];

        let (on_order, dispatch) = with_sales_on_order(vec![close.clone(), far.clone()], dispatch);

        assert_eq!(
            on_order,
            [
                OnOrderAlert {
                    sale: close,
                    due: Some(DispatchDue::Late)
                },
                OnOrderAlert {
                    sale: far,
                    due: None
                },
            ]
        );
        assert_eq!(dispatch.len(), 1);
        assert_eq!(dispatch[0].order, other);
    }

    #[test]
    fn hoje_in_a_channel_s_context_asks_only_for_that_channel_s_orders() {
        block_on(async {
            let store = Store::selling_and_promoting().await;

            let alerts = OrderAlerts::read(&store.orders).await.unwrap();

            assert_eq!(
                channels(&alerts.in_context(WorkContext::All)),
                [SalesChannelKind::MercadoLivre, SalesChannelKind::Shopee]
            );
            assert_eq!(alerts.in_context(WorkContext::All), alerts);
            assert_eq!(
                channels(&alerts.in_context(WorkContext::MercadoLivre)),
                [SalesChannelKind::MercadoLivre]
            );
            assert_eq!(
                channels(&alerts.in_context(WorkContext::Shopee)),
                [SalesChannelKind::Shopee]
            );
            for context in [WorkContext::Amazon, WorkContext::Hotmart] {
                assert!(alerts.in_context(context).is_empty(), "{}", context.name());
            }
        });
    }
}
