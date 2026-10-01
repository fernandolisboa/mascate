use gpui_kit::component::{ActiveTheme, v_flex};
use gpui_kit::*;

use crate::startup;

/// The first screen: what needs attention today. Empty until later slices
/// feed it Orders, questions, low stock and Reminders.
pub struct Home {
    startup: startup::Outcome,
}

impl Home {
    pub fn new(startup: startup::Outcome) -> Self {
        Self { startup }
    }
}

impl Render for Home {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (title, detail) = match &self.startup {
            Ok(()) => (
                "Nada precisa da sua atenção agora".to_string(),
                "Pedidos, perguntas, estoque baixo e lembretes vão aparecer aqui.".to_string(),
            ),
            Err(message) => ("O app não conseguiu iniciar".to_string(), message.clone()),
        };

        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .p_8()
            .gap_6()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Hoje"),
            )
            .child(
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(div().text_lg().child(title))
                    .child(div().text_color(theme.muted_foreground).child(detail)),
            )
    }
}
