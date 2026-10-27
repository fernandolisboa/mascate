//! The question on the Afiliação screen when a first sale arrives on a
//! Platform where the owner added a Program by hand (#194, ADR 0037
//! decision 5): "A venda de {produto} é da afiliação {nome}?". Only the
//! owner's answer joins the two; with two added Programs each one is asked
//! about, and the owner picks.

use std::sync::Arc;

use chrono::Local;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, EventEmitter, Subscription, Window, div};
use mascate_affiliate::{Affiliate, AffiliateError, ProgramJoin};
use mascate_kernel::RecordId;

use crate::affiliate::{CommissionChanges, affiliate, failure};
use crate::appearance::look;
use crate::forms::{Outcome, day_text, notice};
use crate::kit;
use crate::preferences;

/// The owner joined two Programs: the sale's Commissions changed Program.
pub struct ProgramsJoined;

pub struct ProgramJoinsSection {
    joins: Vec<ProgramJoin>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ProgramsJoined> for ProgramJoinsSection {}

impl ProgramJoinsSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            // A Commission by hand may be the first sale of a new product.
            cx.observe_global_in::<CommissionChanges>(window, Self::refresh),
        ];
        Self {
            joins: Vec::new(),
            busy: false,
            reads: 0,
            outcome: None,
            _subscriptions: subscriptions,
        }
    }

    /// Reads the joins waiting for the owner again.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, |_| async { Ok(()) }, |_, _| {});
    }

    /// Follows a new Work Context: what the last answer said stays with
    /// the context it was given in.
    pub fn follow_context(&mut self, cx: &mut Context<Self>) {
        self.outcome = None;
        cx.notify();
    }

    /// Runs `change` off the UI thread, then reads the joins again and
    /// calls `done` when `change` went through.
    fn run<F, Fut>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: F,
        done: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) where
        F: FnOnce(Arc<Affiliate>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), AffiliateError>> + Send,
    {
        let Some(affiliate) = affiliate(cx) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(affiliate.clone()).await;
            (changed, affiliate.joins().await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, joins) = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads == read {
                    this.busy = false;
                    match joins {
                        Ok(joins) => this.joins = joins,
                        Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                    }
                }
                match changed {
                    Ok(()) => done(this, cx),
                    Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn confirm(&mut self, join: &ProgramJoin, window: &mut Window, cx: &mut Context<Self>) {
        let (provisional, sold) = (join.provisional.id, join.sold.id);
        let text = format!(
            "As vendas de {} agora são da afiliação {}, com os links e as regras dela.",
            join.product, join.provisional.name
        );
        self.outcome = None;
        self.run(
            window,
            cx,
            move |affiliate| async move {
                affiliate.confirm_join(provisional, sold).await.map(|_| ())
            },
            move |this, cx| {
                this.outcome = Some(Outcome::Done(text.into()));
                cx.emit(ProgramsJoined);
            },
        );
    }

    fn decline(
        &mut self,
        provisional: RecordId,
        sold: RecordId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.outcome = None;
        self.run(
            window,
            cx,
            move |affiliate| async move { affiliate.decline_join(provisional, sold).await },
            |this, _| {
                this.outcome = Some(Outcome::Done(
                    "Ficam como afiliações diferentes; o app não pergunta de novo.".into(),
                ));
            },
        );
    }

    fn render_join(&self, index: usize, join: &ProgramJoin, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let sold_on = day_text(join.first_sold_at.with_timezone(&Local).date_naive());
        let added_product = join
            .provisional
            .product
            .clone()
            .unwrap_or_else(|| join.provisional.name.clone());
        let (provisional, sold) = (join.provisional.id, join.sold.id);
        let confirmed = join.clone();
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.accent)
            .bg(t.raised)
            .child(div().font_medium().child(format!(
                "A venda de {} é da afiliação {}?",
                join.product, join.provisional.name
            )))
            .child(div().text_sm().text_color(t.text2).child(format!(
                "Primeira venda de {} na {} em {sold_on}. Você cadastrou {} para o produto \
                 \"{added_product}\". Se for a mesma, as comissões dessa venda passam para \
                 ela, com os links e as regras que você já fez.",
                join.product,
                join.sold.platform.name(),
                join.provisional.name
            )))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(("join-same", index))
                            .label("É a mesma")
                            .primary()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm(&confirmed, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("join-different", index))
                            .label("São diferentes")
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.decline(provisional, sold, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for ProgramJoinsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let context = preferences::work_context(cx);
        let joins: Vec<ProgramJoin> = self
            .joins
            .iter()
            .filter(|join| context.shows_platform(join.sold.platform))
            .cloned()
            .collect();
        let shown = !joins.is_empty() || self.outcome.is_some();
        let rows: Vec<AnyElement> = joins
            .iter()
            .enumerate()
            .map(|(index, join)| self.render_join(index, join, cx))
            .collect();
        v_flex()
            .gap_3()
            .when(!joins.is_empty(), |section| {
                section.child(kit::section_heading("Vendas novas para conferir"))
            })
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .children(rows)
            .when(!shown, |section| section.hidden())
    }
}
