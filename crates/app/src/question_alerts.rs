//! The questions waiting for the owner's answer on the home screen (#24),
//! the longest waiting first. Read again after every Sync of questions.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, EventEmitter, SharedString, Subscription, Window, div};
use std::collections::BTreeMap;

use mascate_kernel::Timestamp;
use mascate_marketing::Question;

use crate::appearance::look;
use crate::catalog;
use crate::kit;
use crate::listings::listings;
use crate::questions::{
    QuestionSyncs, failure, product_names, questions, waiting_ink, waiting_text,
};

/// The questions shown one by one; the rest are counted.
const SHOWN: usize = 5;

/// The owner asked to see the questions.
pub struct OpenQuestions;

pub struct QuestionAlertsArea {
    waiting: Vec<Question>,
    names: BTreeMap<String, String>,
    now: Option<Timestamp>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OpenQuestions> for QuestionAlertsArea {}

impl QuestionAlertsArea {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_global::<QuestionSyncs>(Self::refresh)];
        let mut area = Self {
            waiting: Vec::new(),
            names: BTreeMap::new(),
            now: None,
            reads: 0,
            error: None,
            _subscriptions: subscriptions,
        };
        area.refresh(cx);
        area
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty() && self.error.is_none()
    }

    /// Reads the questions again, as when the home screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(questions), Some(listings), Some(catalog)) =
            (questions(cx), listings(cx), catalog::catalog(cx))
        else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            let waiting: Vec<Question> = questions
                .inbox()
                .await
                .map_err(|e| failure(&e))?
                .into_iter()
                .filter(Question::is_unanswered)
                .collect();
            let names = product_names(&listings, &catalog).await?;
            Ok::<_, String>((waiting, names, questions.now()))
        });
        cx.spawn(async move |this, cx| {
            let read_back = reading.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                match read_back {
                    Ok((waiting, names, now)) => {
                        this.waiting = waiting;
                        this.names = names;
                        this.now = Some(now);
                        this.error = None;
                    }
                    Err(error) => {
                        this.error = Some(format!("Não consegui ler as perguntas: {error}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for QuestionAlertsArea {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.is_empty() {
            return div().into_any_element();
        }
        let t = look(cx).tokens;
        let rows: Vec<AnyElement> = self
            .waiting
            .iter()
            .take(SHOWN)
            .map(|question| {
                let waiting = self
                    .now
                    .and_then(|now| question.waiting(now))
                    .unwrap_or_default();
                h_flex()
                    .gap_3()
                    .items_center()
                    .p_3()
                    .rounded(t.radius_lg)
                    .border(t.border_width)
                    .border_color(t.frame)
                    .bg(t.surface)
                    .child(kit::tag(
                        format!("Espera {}", waiting_text(waiting)),
                        waiting_ink(waiting, &t),
                        cx,
                    ))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .child(
                                div()
                                    .truncate()
                                    .font_medium()
                                    .child(format!("“{}”", question.asked.text)),
                            )
                            .child(div().text_color(t.text2).child(
                                match self.names.get(&question.asked.listing) {
                                    Some(name) => {
                                        format!("{name} · {}", question.asked.listing)
                                    }
                                    None => format!("Anúncio {}", question.asked.listing),
                                },
                            )),
                    )
                    .into_any_element()
            })
            .collect();
        let more = self.waiting.len().saturating_sub(SHOWN);
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(kit::section_heading("Perguntas sem resposta"))
                    .child(
                        Button::new("open-questions")
                            .label("Responder")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(OpenQuestions))),
                    ),
            )
            .when_some(self.error.clone(), |area, error| {
                area.child(kit::error_notice(error, cx))
            })
            .children(rows)
            .when(more > 0, |area| {
                area.child(div().text_sm().text_color(t.text2).child(match more {
                    1 => "E mais 1 pergunta.".to_owned(),
                    more => format!("E mais {more} perguntas."),
                }))
            })
            .into_any_element()
    }
}
