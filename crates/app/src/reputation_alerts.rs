//! The Reputation on the home screen (#25): a notice, until the owner reads
//! it, for each tool the Reputation unlocked, and the low Reviews the owner
//! has not looked at, the newest first. Read again after every Sync of the
//! Reputation.

use std::collections::BTreeMap;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, EventEmitter, SharedString, Subscription, Window, div};
use mascate_marketing::{LowReview, SellerTool};

use crate::appearance::look;
use crate::catalog;
use crate::kit;
use crate::listings::{self, listings};
use crate::reputation::{
    ReputationSyncs, failure, reputation, review_line, stars_text, unlocked_title,
};

/// The low Reviews shown one by one; the rest are counted.
const SHOWN: usize = 5;

/// The owner asked to see the Reputation.
pub struct OpenReputation;

pub struct ReputationAlertsArea {
    unlocked: Vec<SellerTool>,
    low: Vec<LowReview>,
    names: BTreeMap<String, String>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OpenReputation> for ReputationAlertsArea {}

impl ReputationAlertsArea {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_global::<ReputationSyncs>(Self::refresh)];
        let mut area = Self {
            unlocked: Vec::new(),
            low: Vec::new(),
            names: BTreeMap::new(),
            reads: 0,
            error: None,
            _subscriptions: subscriptions,
        };
        area.refresh(cx);
        area
    }

    pub fn is_empty(&self) -> bool {
        self.unlocked.is_empty() && self.low.is_empty() && self.error.is_none()
    }

    /// Reads the notices again, as when the home screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(reputation), Some(listings), Some(catalog)) =
            (reputation(cx), listings(cx), catalog::catalog(cx))
        else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            let unlocked = reputation.unlocked().await.map_err(|e| failure(&e))?;
            let low: Vec<LowReview> = reputation
                .low_reviews()
                .await
                .map_err(|e| failure(&e))?
                .into_iter()
                .filter(|low| !low.seen)
                .collect();
            let names = if low.is_empty() {
                BTreeMap::new()
            } else {
                listings::listing_products(&listings, &catalog)
                    .await?
                    .into_iter()
                    .map(|(listing, sold)| (listing, sold.name))
                    .collect()
            };
            Ok::<_, String>((unlocked, low, names))
        });
        cx.spawn(async move |this, cx| {
            let read_back = reading.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                match read_back {
                    Ok((unlocked, low, names)) => {
                        this.unlocked = unlocked;
                        this.low = low;
                        this.names = names;
                        this.error = None;
                    }
                    Err(error) => {
                        this.error = Some(format!("Não consegui ler a reputação: {error}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn dismiss(&mut self, tool: SellerTool, cx: &mut Context<Self>) {
        let Some(reputation) = reputation(cx) else {
            return;
        };
        let dismissing = cx
            .background_executor()
            .spawn(async move { reputation.dismiss(tool).await });
        cx.spawn(async move |this, cx| {
            let dismissed = dismissing.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = dismissed {
                    this.error = Some(failure(&error).into());
                }
                cx.default_global::<ReputationSyncs>().finished += 1;
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for ReputationAlertsArea {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.is_empty() {
            return div().into_any_element();
        }
        let t = look(cx).tokens;
        let unlocked: Vec<AnyElement> = self
            .unlocked
            .iter()
            .map(|tool| {
                let tool = *tool;
                h_flex()
                    .gap_3()
                    .items_center()
                    .p_3()
                    .rounded(t.radius_lg)
                    .border(t.border_width)
                    .border_color(t.success)
                    .bg(t.surface)
                    .child(kit::tag("Liberado", t.success, cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .child(div().font_medium().child(unlocked_title(tool)))
                            .child(div().text_color(t.text2).child(format!(
                                "Sua reputação chegou ao que o Mercado Livre pede: {}.",
                                tool.requirement()
                            ))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("dismiss-{}", tool.name())))
                            .label("Entendi")
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| this.dismiss(tool, cx))),
                    )
                    .into_any_element()
            })
            .collect();
        let low: Vec<AnyElement> = self
            .low
            .iter()
            .take(SHOWN)
            .map(|low| {
                h_flex()
                    .gap_3()
                    .items_center()
                    .p_3()
                    .rounded(t.radius_lg)
                    .border(t.border_width)
                    .border_color(t.frame)
                    .bg(t.surface)
                    .child(kit::tag(stars_text(low.review.rating), t.danger, cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .child(div().truncate().font_medium().child(format!(
                                "“{}”",
                                review_line(&low.review.title, &low.review.text)
                            )))
                            .child(div().text_color(t.text2).child(
                                match self.names.get(&low.listing) {
                                    Some(name) => format!("{name} · {}", low.listing),
                                    None => format!("Anúncio {}", low.listing),
                                },
                            )),
                    )
                    .into_any_element()
            })
            .collect();
        let more = self.low.len().saturating_sub(SHOWN);
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(kit::section_heading("Reputação e avaliações"))
                    .child(
                        Button::new("open-reputation")
                            .label("Ver reputação")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(OpenReputation))),
                    ),
            )
            .when_some(self.error.clone(), |area, error| {
                area.child(kit::error_notice(error, cx))
            })
            .children(unlocked)
            .children(low)
            .when(more > 0, |area| {
                area.child(div().text_sm().text_color(t.text2).child(match more {
                    1 => "E mais 1 avaliação baixa.".to_owned(),
                    more => format!("E mais {more} avaliações baixas."),
                }))
            })
            .into_any_element()
    }
}
