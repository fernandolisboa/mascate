use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::{Entity, SharedString, Window, div};

use crate::appearance::look;
use crate::backups;
use crate::layout;
use crate::parts::ScreenParts;
use crate::reminders::RemindersArea;
use crate::updates::UpdateNotice;

/// The first screen: what needs attention today, a newer version of the
/// app at its top and the Reminders at its foot. Later slices feed it
/// Orders, questions and low stock.
pub struct Home {
    /// Why the database is not ready, if it isn't.
    problem: Option<SharedString>,
    update: Entity<UpdateNotice>,
    reminders: Entity<RemindersArea>,
}

impl Home {
    pub fn new(problem: Option<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            problem,
            update: cx.new(UpdateNotice::new),
            reminders: cx.new(RemindersArea::new),
        }
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
        parts
            .content
            .push(self.reminders.clone().into_any_element());
        layout::screen(parts, cx)
    }
}
