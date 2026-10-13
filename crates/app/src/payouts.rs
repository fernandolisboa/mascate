//! Repasses on the Afiliação screen (#79): each Payout with the Commissions
//! it covers and the difference, and "Registrar repasse" for Platforms that
//! report none. The app proposes the approved Commissions the Payout
//! covers, oldest first; the owner adjusts and confirms, and only then are
//! they paid. The rules live in Affiliate (ADR 0030).

use std::ops::Range;
use std::sync::Arc;

use chrono::Local;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::select::Select;
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, EventEmitter, SharedString, Window, div, px};
use mascate_affiliate::{
    Affiliate, AffiliateError, Commission, ManualPayout, Payout, PayoutProposal,
};
use mascate_kernel::{Currency, Money, RecordId, Timestamp, parse_amount};

use crate::affiliate::{PlatformSelect, affiliate, failure, platform_select};
use crate::appearance::look;
use crate::catalog::day;
use crate::forms::{Outcome, input, notice, parse_day};
use crate::kit;
use crate::sales::day_start;

/// A Payout was recorded or deleted: the Commissions it covers changed
/// status.
pub struct PayoutsChanged;

pub struct PayoutsSection {
    paid: Range<Timestamp>,
    payouts: Vec<Payout>,
    platform: PlatformSelect,
    paid_on: Entity<InputState>,
    amount: Entity<InputState>,
    currency: Currency,
    /// What the owner is confirming, as they adjust it.
    proposal: Option<PayoutProposal>,
    /// The Payout asking to confirm its deletion.
    deleting: Option<RecordId>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    form: Option<Outcome>,
    outcome: Option<Outcome>,
}

impl EventEmitter<PayoutsChanged> for PayoutsSection {}

impl PayoutsSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let paid_on = input("dd/mm/aaaa", window, cx);
        paid_on.update(cx, |field, cx| field.set_value(today(), window, cx));
        Self {
            paid: Timestamp::MIN_UTC..Timestamp::MAX_UTC,
            payouts: Vec::new(),
            platform: platform_select(window, cx),
            paid_on,
            amount: input("120,00", window, cx),
            currency: Currency::Brl,
            proposal: None,
            deleting: None,
            busy: false,
            reads: 0,
            form: None,
            outcome: None,
        }
    }

    /// Reads the Payouts made during `paid` again.
    pub fn refresh(&mut self, paid: Range<Timestamp>, window: &mut Window, cx: &mut Context<Self>) {
        self.paid = paid;
        self.run(window, cx, |_| async { Ok(None) }, |_, _: (), _, _| {});
    }

    /// Runs `change` off the UI thread, then reads the Payouts again and
    /// hands what the change returned to `then`.
    fn run<T, F, Fut>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: F,
        then: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) where
        T: Send + 'static,
        F: FnOnce(Arc<Affiliate>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<T>, AffiliateError>> + Send,
    {
        let Some(affiliate) = affiliate(cx) else {
            return;
        };
        let paid = self.paid.clone();
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(affiliate.clone()).await;
            (changed, affiliate.payouts(paid).await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, payouts) = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads == read {
                    this.busy = false;
                    match payouts {
                        Ok(payouts) => this.payouts = payouts,
                        Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                    }
                }
                match changed {
                    Ok(Some(value)) => then(this, value, window, cx),
                    Ok(None) => {}
                    Err(error) => this.form = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The Payout the form describes, or why it cannot be one yet.
    fn written(&self, cx: &App) -> Result<ManualPayout, &'static str> {
        let platform = *self
            .platform
            .read(cx)
            .selected_value()
            .ok_or("Escolha a plataforma.")?;
        let paid_on = parse_day(&self.paid_on.read(cx).value())
            .ok_or("Digite a data do repasse como 01/10/2026.")?;
        let amount = parse_amount(&self.amount.read(cx).value())
            .filter(|amount| amount.is_sign_positive() && !amount.is_zero())
            .ok_or("Digite o valor do repasse como 120,00.")?;
        Ok(ManualPayout {
            platform,
            paid_at: day_start(paid_on),
            amount: Money::new(amount, self.currency),
        })
    }

    fn propose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let payout = match self.written(cx) {
            Ok(payout) => payout,
            Err(why) => {
                self.form = Some(Outcome::Failed(why.into()));
                cx.notify();
                return;
            }
        };
        self.form = None;
        self.proposal = None;
        self.run(
            window,
            cx,
            move |affiliate| async move { affiliate.propose_payout(&payout).await.map(Some) },
            |this, proposal: PayoutProposal, _, _| this.proposal = Some(proposal),
        );
    }

    fn toggle(&mut self, commission: RecordId, cx: &mut Context<Self>) {
        if let Some(proposal) = &mut self.proposal {
            match proposal.proposed.iter().position(|id| *id == commission) {
                Some(at) => {
                    proposal.proposed.remove(at);
                }
                None => proposal.proposed.push(commission),
            }
            cx.notify();
        }
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(proposal) = &self.proposal else {
            return;
        };
        let payout = proposal.payout;
        let covers = proposal.proposed.clone();
        self.form = None;
        self.outcome = None;
        self.run(
            window,
            cx,
            move |affiliate| async move { affiliate.record_payout(payout, &covers).await.map(Some) },
            |this, recorded: Payout, window, cx| {
                this.proposal = None;
                this.amount
                    .update(cx, |field, cx| field.set_value("", window, cx));
                this.form = Some(Outcome::Done(recorded_text(recorded.covers.len()).into()));
                cx.emit(PayoutsChanged);
            },
        );
    }

    fn delete(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.outcome = None;
        self.run(
            window,
            cx,
            move |affiliate| async move { affiliate.delete_payout(id).await.map(Some) },
            |this, (), _, cx| {
                this.deleting = None;
                this.outcome = Some(Outcome::Done(
                    "Repasse excluído; as comissões que ele cobria voltaram a aprovadas.".into(),
                ));
                cx.emit(PayoutsChanged);
            },
        );
    }

    fn render_payout(&self, payout: &Payout, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let id = payout.id;
        let actions = payout.manual.then(|| {
            let button = |name: &'static str, label: &'static str| {
                Button::new(SharedString::from(format!("{name}-{id}")))
                    .label(label)
                    .ghost()
                    .small()
                    .disabled(self.busy)
            };
            if self.deleting == Some(id) {
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .child("Excluir? As comissões voltam a aprovadas."),
                    )
                    .child(
                        button("confirm-delete-payout", "Excluir")
                            .danger()
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.delete(id, window, cx)),
                            ),
                    )
                    .child(button("keep-payout", "Voltar").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.deleting = None;
                            cx.notify();
                        },
                    )))
            } else {
                h_flex().child(button("delete-payout", "Excluir").on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.deleting = Some(id);
                        cx.notify();
                    },
                )))
            }
        });
        let covers = payout.covers.iter().map(|commission| {
            h_flex()
                .gap_2()
                .text_xs()
                .text_color(t.text2)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(commission_line(commission)),
                )
                .child(
                    div()
                        .whitespace_nowrap()
                        .child(commission.amount.to_pt_br()),
                )
        });
        v_flex()
            .gap_1()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(kit::tag(payout.platform.name(), t.accent_text, cx))
                                    .child(
                                        div()
                                            .font_medium()
                                            .child(format!("repasse em {}", day(payout.paid_at))),
                                    )
                                    .when(payout.manual, |row| {
                                        row.child(kit::tag("à mão", t.text2, cx))
                                    }),
                            )
                            .child(div().text_xs().text_color(t.text2).child(format!(
                                "cobre {}: {} · diferença {}",
                                covered_text(payout.covers.len()),
                                payout.covered().to_pt_br(),
                                signed(payout.difference())
                            ))),
                    )
                    .children(actions)
                    .child(
                        div()
                            .font_semibold()
                            .whitespace_nowrap()
                            .child(payout.amount.to_pt_br()),
                    ),
            )
            .children(covers)
            .into_any_element()
    }

    fn render_proposal(&self, proposal: &PayoutProposal, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        if proposal.candidates.is_empty() {
            return v_flex()
                .gap_2()
                .child(div().text_sm().text_color(t.text2).child(
                    "Nenhuma comissão aprovada dessa plataforma e moeda está sem repasse. O \
                     repasse pode ser registrado assim mesmo, sem cobrir comissões.",
                ))
                .child(self.render_confirm(proposal, cx))
                .into_any_element();
        }
        let entity = cx.entity().downgrade();
        let candidates = proposal
            .candidates
            .iter()
            .enumerate()
            .map(|(index, commission)| {
                let id = commission.id;
                let this = entity.clone();
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div().flex_1().min_w_0().child(
                            Checkbox::new(("payout-commission", index))
                                .label(commission_line(commission))
                                .checked(proposal.proposed.contains(&id))
                                .on_click(move |_, _, cx| {
                                    let _ = this.update(cx, |this, cx| this.toggle(id, cx));
                                }),
                        ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .whitespace_nowrap()
                            .child(commission.amount.to_pt_br()),
                    )
            });
        v_flex()
            .gap_2()
            .child(div().text_sm().text_color(t.text2).child(format!(
                "Comissões aprovadas dessa plataforma e moeda sem repasse, da venda mais \
                 antiga para a mais nova. Marquei as que cabem em {}; ajuste antes de \
                 confirmar.",
                proposal.payout.amount.to_pt_br()
            )))
            .children(candidates)
            .child(div().text_sm().font_medium().child(format!(
                "{}: {} · repasse {} · diferença {}",
                covered_text(proposal.proposed.len()),
                proposal.covered().to_pt_br(),
                proposal.payout.amount.to_pt_br(),
                signed(proposal.difference())
            )))
            .child(div().text_xs().text_color(t.text2).child(
                "Diferença negativa costuma ser taxa ou imposto retido pela plataforma; ela \
                 não impede o registro.",
            ))
            .child(self.render_confirm(proposal, cx))
            .into_any_element()
    }

    fn render_confirm(&self, proposal: &PayoutProposal, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .gap_2()
            .child(
                Button::new("confirm-payout")
                    .label(format!(
                        "Confirmar repasse de {} em {}",
                        proposal.payout.amount.to_pt_br(),
                        day(proposal.payout.paid_at)
                    ))
                    .primary()
                    .small()
                    .loading(self.busy)
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
            )
            .child(
                Button::new("discard-payout")
                    .label("Descartar")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.proposal = None;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, input: AnyElement| {
            v_flex()
                .gap_1()
                .child(div().text_sm().font_medium().child(label))
                .child(input)
        };
        let currency = |value: Currency| {
            let button = Button::new(("payout-currency", value as usize))
                .label(value.symbol())
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.currency = value;
                    cx.notify();
                }));
            if self.currency == value {
                button.primary()
            } else {
                button.outline()
            }
        };
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if self.proposal.is_some() {
                t.accent_edge
            } else {
                t.frame
            })
            .bg(t.surface)
            .child(kit::section_heading("Registrar repasse"))
            .child(div().text_sm().text_color(t.text2).child(
                "Para plataformas que não informam repasses. O app propõe as comissões \
                 aprovadas mais antigas que cabem no valor; nada fica pago antes de você \
                 confirmar.",
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(div().w(px(180.)).child(field(
                        "Plataforma",
                        Select::new(&self.platform).small().into_any_element(),
                    )))
                    .child(div().w(px(130.)).child(field(
                        "Data do repasse",
                        Input::new(&self.paid_on).small().into_any_element(),
                    )))
                    .child(div().w(px(130.)).child(field(
                        "Valor",
                        Input::new(&self.amount).small().into_any_element(),
                    )))
                    .child(field(
                        "Moeda",
                        h_flex()
                            .gap_1()
                            .child(currency(Currency::Brl))
                            .child(currency(Currency::Usd))
                            .into_any_element(),
                    ))
                    .child(
                        Button::new("propose-payout")
                            .label("Propor comissões")
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.propose(window, cx))),
                    ),
            )
            .children(
                self.proposal
                    .as_ref()
                    .map(|proposal| self.render_proposal(proposal, cx)),
            )
            .children(self.form.as_ref().map(|outcome| notice(outcome, cx)))
            .into_any_element()
    }
}

/// "Curso de Fotografia · venda em 01/09/2026".
fn commission_line(commission: &Commission) -> String {
    format!(
        "{} · venda em {}",
        commission.product,
        day(commission.sold_at)
    )
}

/// "+R$ 2,00", "-R$ 2,50", "R$ 0,00".
fn signed(amount: Money) -> String {
    if amount.amount().is_sign_positive() && !amount.amount().is_zero() {
        format!("+{}", amount.to_pt_br())
    } else {
        amount.to_pt_br()
    }
}

/// "nenhuma comissão", "1 comissão", "3 comissões".
fn covered_text(count: usize) -> String {
    match count {
        0 => "nenhuma comissão".into(),
        1 => "1 comissão".into(),
        count => format!("{count} comissões"),
    }
}

/// What recording a Payout that covers `count` Commissions did.
fn recorded_text(count: usize) -> &'static str {
    match count {
        0 => "Repasse registrado, sem cobrir comissões.",
        1 => "Repasse registrado; a comissão ficou paga.",
        _ => "Repasse registrado; as comissões ficaram pagas.",
    }
}

fn today() -> String {
    Local::now().format("%d/%m/%Y").to_string()
}

impl Render for PayoutsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let payouts: Vec<AnyElement> = self
            .payouts
            .iter()
            .map(|payout| self.render_payout(payout, cx))
            .collect();
        v_flex()
            .gap_3()
            .child(kit::section_heading(format!(
                "Repasses ({})",
                self.payouts.len()
            )))
            .child(div().text_sm().text_color(t.text2).child(
                "O que cada plataforma repassou e as comissões que cada repasse cobre. A \
                 diferença mostra o que a plataforma reteve (negativa) ou pagou a mais.",
            ))
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .when(payouts.is_empty(), |list| {
                list.child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("Nenhum repasse neste período."),
                )
            })
            .children(payouts)
            .child(self.render_form(cx))
    }
}
