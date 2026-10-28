//! Result Promises as the owner reads them (#197, ADR 0037 decision 9): the
//! warning an Offer Post shows, and "Conferir texto" on the Divulgação
//! screen for a text the owner pastes into an ad elsewhere. The app only
//! warns; the patterns live in the Marketing.

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{Sizable as _, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{Entity, Subscription, Window, div};
use mascate_marketing::{ResultPromise, promises_in};

use crate::appearance::look;
use crate::kit;

/// A Result Promise of a text, as the owner reads it.
pub(crate) fn promise_text(promise: &ResultPromise) -> String {
    format!(
        "O texto promete resultado: {}. A plataforma do anúncio pode reprovar o texto ou \
         bloquear a conta; é só um aviso, e você decide se usa assim.",
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
        Err(promises.iter().map(promise_text).collect())
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
        let findings = findings(&self.text.read(cx).value());
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
            .children(findings.map(|found| {
                match found {
                    Ok(()) => kit::success_notice("Nenhuma promessa de resultado no texto.", cx)
                        .into_any_element(),
                    Err(warnings) => v_flex()
                        .gap_1()
                        .children(
                            warnings
                                .into_iter()
                                .map(|warning| kit::info_notice(warning, cx)),
                        )
                        .into_any_element(),
                }
            }))
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
        assert_eq!(warnings.len(), 2);
        assert!(
            warnings[0].contains("“Ganhe R$ 500 por dia”"),
            "{}",
            warnings[0]
        );
        assert!(warnings[0].contains("Padrões de Publicidade da Meta"));
        assert!(warnings[1].contains("“Perca 10 kg”"), "{}", warnings[1]);
        assert!(warnings[1].ends_with("você decide se usa assim."));
    }
}
