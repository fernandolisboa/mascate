//! Result Promises as the owner reads them (#197, ADR 0037 decision 9): the
//! warning an Offer Post shows, and "Conferir texto" on the Divulgação
//! screen for a text the owner pastes into an ad elsewhere, which the
//! Passo a passo's Divulgar step checks too (#198). The app only warns; the
//! patterns live in the Marketing.

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{Sizable as _, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, Subscription, Window, div};
use mascate_marketing::{ResultPromise, promises_in};

use crate::appearance::look;
use crate::kit;

/// What the owner reads under the Result Promises of a text.
const ONLY_A_WARNING: &str = "A plataforma do anúncio pode reprovar o texto ou bloquear a conta; é \
                              só um aviso, e você decide se usa assim.";

/// A Result Promise of a post, as the owner reads it.
pub(crate) fn promise_text(promise: &ResultPromise) -> String {
    format!(
        "O texto promete resultado: {}. {ONLY_A_WARNING}",
        promise.describe()
    )
}

/// What "Conferir texto" says of `text`: nothing while it is empty.
fn findings(text: &str) -> Option<Result<(), Vec<String>>> {
    if text.trim().is_empty() {
        return None;
    }
    let promises = promises_in(text);
    Some(if promises.is_empty() {
        Ok(())
    } else {
        Err(promises.iter().map(ResultPromise::describe).collect())
    })
}

/// What a text pasted into an ad promises, as the owner reads it beside the
/// text: nothing while it is empty.
pub(crate) fn findings_notice(text: &str, cx: &App) -> Option<AnyElement> {
    let t = look(cx).tokens;
    findings(text).map(|found| match found {
        Ok(()) => {
            kit::success_notice("Nenhuma promessa de resultado no texto.", cx).into_any_element()
        }
        Err(warnings) => v_flex()
            .gap_1()
            .children(
                warnings
                    .into_iter()
                    .map(|warning| kit::info_notice(warning, cx)),
            )
            .child(div().text_sm().text_color(t.text2).child(ONLY_A_WARNING))
            .into_any_element(),
    })
}

/// "Conferir texto": checks as the owner types or pastes.
pub struct TextCheckSection {
    text: Entity<TextareaState>,
    _changes: Subscription,
}

impl TextCheckSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 14)
                .placeholder("Cole aqui o texto do anúncio")
        });
        let changes = cx.subscribe_in(&text, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            text,
            _changes: changes,
        }
    }
}

impl Render for TextCheckSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let findings = findings_notice(&self.text.read(cx).value(), cx);
        v_flex()
            .gap_3()
            .child(kit::section_heading("Conferir texto"))
            .child(div().text_sm().text_color(t.text2).child(
                "Para o texto que você cola num anúncio fora do app (Meta, Google, TikTok): o app \
                 avisa quando ele promete ganho de dinheiro, perda de peso, cura, resultado \
                 garantido, ausência de risco ou antes e depois, que essas plataformas reprovam. \
                 Os posts de oferta já são conferidos sozinhos. Nada é enviado daqui.",
            ))
            .child(Textarea::new(&self.text).small())
            .children(findings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_text_says_nothing_and_an_honest_one_says_so() {
        assert_eq!(findings("  \n"), None);
        assert_eq!(
            findings("Curso de fotografia com garantia de 7 dias"),
            Some(Ok(()))
        );
    }

    #[test]
    fn each_promise_is_told_with_its_phrase_and_source() {
        let Some(Err(warnings)) = findings("Ganhe R$ 500 por dia. Perca 10 kg!") else {
            panic!("two promises");
        };
        assert_eq!(
            warnings,
            [
                "“Ganhe R$ 500 por dia” promete ganho de dinheiro; fonte: Padrões de Publicidade \
                 da Meta, resultados irreais",
                "“Perca 10 kg” promete perda de peso; fonte: Padrões de Publicidade da Meta, \
                 resultados irreais",
            ]
        );
    }

    #[test]
    fn a_post_s_promise_says_it_is_only_a_warning() {
        let promise = promises_in("Risco zero").remove(0);

        let text = promise_text(&promise);

        assert!(
            text.starts_with("O texto promete resultado: “Risco zero”"),
            "{text}"
        );
        assert!(text.ends_with(ONLY_A_WARNING), "{text}");
    }
}
