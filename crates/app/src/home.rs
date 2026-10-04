use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::{Entity, SharedString, Subscription, Window, div};

use crate::appearance::look;
use crate::backups;
use crate::layout;
use crate::low_stock::LowStockArea;
use crate::order_alerts::OrderAlertsArea;
use crate::parts::ScreenParts;
use crate::reminders::RemindersArea;
use crate::updates::UpdateNotice;

/// The first screen: what needs attention today (Orders to dispatch or
/// returns to receive, and low stock; later slices add questions), a newer
/// version of the app at its top and the Reminders at its foot.
pub struct Home {
    /// Why the database is not ready, if it isn't.
    problem: Option<SharedString>,
    update: Entity<UpdateNotice>,
    pub order_alerts: Entity<OrderAlertsArea>,
    pub low_stock: Entity<LowStockArea>,
    reminders: Entity<RemindersArea>,
    _subscriptions: Vec<Subscription>,
}

impl Home {
    pub fn new(problem: Option<SharedString>, cx: &mut Context<Self>) -> Self {
        let low_stock = cx.new(LowStockArea::new);
        let order_alerts = cx.new(OrderAlertsArea::new);
        // Whether anything needs attention decides what the screen shows.
        let subscriptions = vec![
            cx.observe(&low_stock, |_, _, cx| cx.notify()),
            cx.observe(&order_alerts, |_, _, cx| cx.notify()),
        ];
        Self {
            problem,
            update: cx.new(UpdateNotice::new),
            order_alerts,
            low_stock,
            reminders: cx.new(RemindersArea::new),
            _subscriptions: subscriptions,
        }
    }

    /// Reads what needs attention again, as when the screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.order_alerts.update(cx, OrderAlertsArea::refresh);
        self.low_stock.update(cx, LowStockArea::refresh);
    }
}

impl Render for Home {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let (title, detail) = match &self.problem {
            None => (
                SharedString::from("Nada precisa da sua atenção agora"),
                SharedString::from("Pedidos, perguntas e estoque baixo vão aparecer aqui."),
            ),
            Some(problem) => ("O app não conseguiu iniciar".into(), problem.clone()),
        };
        let mut parts = ScreenParts::new("Hoje");
        parts.notices.extend(backups::restore_notice(cx));
        parts.content.push(self.update.clone().into_any_element());
        let attention =
            !self.order_alerts.read(cx).is_empty() || !self.low_stock.read(cx).is_empty();
        if self.problem.is_none() && attention {
            parts
                .content
                .push(self.order_alerts.clone().into_any_element());
            parts
                .content
                .push(self.low_stock.clone().into_any_element());
        } else {
            parts.content.push(
                v_flex()
                    .flex_1()
                    .min_h_64()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(div().text_lg().child(title))
                    .child(div().text_color(t.text2).child(detail))
                    .into_any_element(),
            );
        }
        parts
            .content
            .push(self.reminders.clone().into_any_element());
        layout::screen(parts, cx)
    }
}
