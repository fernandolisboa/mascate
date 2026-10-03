use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::{SharedString, Window, div};

use crate::appearance::look;
use crate::layout;
use crate::parts::ScreenParts;

/// The first screen: what needs attention today. Empty until later slices
/// feed it Orders, questions, low stock and Reminders.
pub struct Home {
    /// Why the database is not ready, if it isn't.
    problem: Option<SharedString>,
}

impl Home {
    pub fn new(problem: Option<SharedString>) -> Self {
        Self { problem }
    }
}

impl Render for Home {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let (title, detail) = match &self.problem {
            None => (
                SharedString::from("Nada precisa da sua atenção agora"),
                SharedString::from(
                    "Pedidos, perguntas, estoque baixo e lembretes vão aparecer aqui.",
                ),
            ),
            Some(problem) => ("O app não conseguiu iniciar".into(), problem.clone()),
        };
        let mut parts = ScreenParts::new("Hoje");
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
        layout::screen(parts, cx)
    }
}
