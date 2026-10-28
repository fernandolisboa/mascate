use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Entity, SharedString, Subscription, Window, div};

use crate::appearance::look;
use crate::backups;
use crate::event_alerts::EventAlertsArea;
use crate::layout;
use crate::low_stock::LowStockArea;
use crate::on_order_risk::OnOrderRiskArea;
use crate::order_alerts::OrderAlertsArea;
use crate::parts::{Place, ScreenParts};
use crate::preferences;
use crate::question_alerts::QuestionAlertsArea;
use crate::reminders::RemindersArea;
use crate::reputation_alerts::ReputationAlertsArea;
use crate::updates::UpdateNotice;
use crate::work_context::WorkContext;

/// One part of Hoje that may need attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Area {
    Orders,
    Questions,
    Reputation,
    OnOrderRisk,
    LowStock,
    SalesEvents,
}

impl Area {
    const ALL: [Area; 6] = [
        Area::Orders,
        Area::Questions,
        Area::Reputation,
        Area::OnOrderRisk,
        Area::LowStock,
        Area::SalesEvents,
    ];

    /// Whether the area shows in `context` (ADR 0037, decision 3): a sales
    /// area wherever its screen does, and what belongs to everyone, the
    /// stock and the Sales Events, always.
    fn shows_in(self, context: WorkContext) -> bool {
        match self {
            Area::Orders => Place::Orders.shows_in(context),
            Area::Questions => Place::Questions.shows_in(context),
            Area::Reputation | Area::OnOrderRisk => Place::Reputation.shows_in(context),
            Area::LowStock | Area::SalesEvents => true,
        }
    }
}

/// The first screen: what needs attention today (Orders to dispatch or
/// returns to receive, questions waiting for an answer, tools the
/// Reputation unlocked, low Reviews, the Reputation close to losing the
/// green while On-Order Listings sell, low stock and the Sales Events inside
/// their lead time), a newer version of the app at its top and the
/// Reminders at its foot. A Platform's Work Context shows only its own
/// besides what belongs to everyone.
pub struct Home {
    /// Why the database is not ready, if it isn't.
    problem: Option<SharedString>,
    update: Entity<UpdateNotice>,
    pub order_alerts: Entity<OrderAlertsArea>,
    pub question_alerts: Entity<QuestionAlertsArea>,
    pub reputation_alerts: Entity<ReputationAlertsArea>,
    on_order_risk: Entity<OnOrderRiskArea>,
    pub low_stock: Entity<LowStockArea>,
    pub event_alerts: Entity<EventAlertsArea>,
    reminders: Entity<RemindersArea>,
    _subscriptions: Vec<Subscription>,
}

impl Home {
    pub fn new(problem: Option<SharedString>, cx: &mut Context<Self>) -> Self {
        let low_stock = cx.new(LowStockArea::new);
        let order_alerts = cx.new(OrderAlertsArea::new);
        let question_alerts = cx.new(QuestionAlertsArea::new);
        let reputation_alerts = cx.new(ReputationAlertsArea::new);
        let on_order_risk = cx.new(OnOrderRiskArea::new);
        let event_alerts = cx.new(EventAlertsArea::new);
        // Whether anything needs attention decides what the screen shows.
        let subscriptions = vec![
            cx.observe(&low_stock, |_, _, cx| cx.notify()),
            cx.observe(&order_alerts, |_, _, cx| cx.notify()),
            cx.observe(&question_alerts, |_, _, cx| cx.notify()),
            cx.observe(&reputation_alerts, |_, _, cx| cx.notify()),
            cx.observe(&on_order_risk, |_, _, cx| cx.notify()),
            cx.observe(&event_alerts, |_, _, cx| cx.notify()),
        ];
        Self {
            problem,
            update: cx.new(UpdateNotice::new),
            order_alerts,
            question_alerts,
            reputation_alerts,
            on_order_risk,
            low_stock,
            event_alerts,
            reminders: cx.new(RemindersArea::new),
            _subscriptions: subscriptions,
        }
    }

    /// Reads what needs attention again, as when the screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.order_alerts.update(cx, OrderAlertsArea::refresh);
        self.question_alerts.update(cx, QuestionAlertsArea::refresh);
        self.reputation_alerts
            .update(cx, ReputationAlertsArea::refresh);
        self.on_order_risk.update(cx, OnOrderRiskArea::refresh);
        self.low_stock.update(cx, LowStockArea::refresh);
        self.event_alerts.update(cx, EventAlertsArea::refresh);
    }
}

impl Render for Home {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let (title, detail) = match &self.problem {
            None => (
                SharedString::from("Nada precisa da sua atenção agora"),
                SharedString::from(
                    "Pedidos, perguntas, avaliações baixas, estoque baixo e datas de venda \
                     chegando vão aparecer aqui.",
                ),
            ),
            Some(problem) => ("O app não conseguiu iniciar".into(), problem.clone()),
        };
        let mut parts = ScreenParts::new("Hoje");
        parts.notices.extend(backups::restore_notice(cx));
        parts.content.push(self.update.clone().into_any_element());
        let context = preferences::work_context(cx);
        let shown: Vec<(bool, AnyElement)> = Area::ALL
            .into_iter()
            .filter(|area| area.shows_in(context))
            .map(|area| match area {
                Area::Orders => (
                    self.order_alerts.read(cx).is_empty_in(context),
                    self.order_alerts.clone().into_any_element(),
                ),
                Area::Questions => (
                    self.question_alerts.read(cx).is_empty(),
                    self.question_alerts.clone().into_any_element(),
                ),
                Area::Reputation => (
                    self.reputation_alerts.read(cx).is_empty(),
                    self.reputation_alerts.clone().into_any_element(),
                ),
                Area::OnOrderRisk => (
                    self.on_order_risk.read(cx).is_empty(),
                    self.on_order_risk.clone().into_any_element(),
                ),
                Area::LowStock => (
                    self.low_stock.read(cx).is_empty(),
                    self.low_stock.clone().into_any_element(),
                ),
                Area::SalesEvents => (
                    self.event_alerts.read(cx).is_empty(),
                    self.event_alerts.clone().into_any_element(),
                ),
            })
            .collect();
        let attention = shown.iter().any(|(empty, _)| !empty);
        if self.problem.is_none() && attention {
            parts
                .content
                .extend(shown.into_iter().map(|(_, area)| area));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(context: WorkContext) -> Vec<Area> {
        Area::ALL
            .into_iter()
            .filter(|area| area.shows_in(context))
            .collect()
    }

    #[test]
    fn hoje_shows_the_context_s_sales_areas_and_what_belongs_to_everyone() {
        assert_eq!(shown(WorkContext::All), Area::ALL);
        assert_eq!(shown(WorkContext::MercadoLivre), Area::ALL);
        for context in [
            WorkContext::Shopee,
            WorkContext::Amazon,
            WorkContext::Magalu,
        ] {
            assert_eq!(
                shown(context),
                [Area::Orders, Area::LowStock, Area::SalesEvents],
                "{}",
                context.name()
            );
        }
        for context in [
            WorkContext::Hotmart,
            WorkContext::Kiwify,
            WorkContext::Monetizze,
            WorkContext::Eduzz,
        ] {
            assert_eq!(
                shown(context),
                [Area::LowStock, Area::SalesEvents],
                "{}",
                context.name()
            );
        }
    }
}
