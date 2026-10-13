//! The commissions dashboard on Afiliação (#81): the period's totals per
//! currency, split by status, and the Commissions grouped by Platform,
//! Program, product or tracking tag, sortable by amount. Reversals show in
//! red and stay out of what was earned; cancelled sales never count. The
//! sums are Affiliate's (`CommissionReport`); the screen hands over the
//! period's Commissions. The tag stands in for the Promotion Channel until
//! Affiliate Links (#30).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Hsla, Window, div, px};
use mascate_affiliate::{
    Commission, CommissionGroup, CommissionReport, CurrencyTotals, GroupOrder,
};

use crate::appearance::{Tokens, look};
use crate::kit;
use crate::sales::{money_cell, table_row};

/// What the groups gather.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Grouping {
    #[default]
    Platform,
    Program,
    Product,
    Tracking,
}

impl Grouping {
    const ALL: [Grouping; 4] = [
        Grouping::Platform,
        Grouping::Program,
        Grouping::Product,
        Grouping::Tracking,
    ];

    fn name(self) -> &'static str {
        match self {
            Grouping::Platform => "Plataforma",
            Grouping::Program => "Programa",
            Grouping::Product => "Produto",
            Grouping::Tracking => "Marcação",
        }
    }
}

const ORDERS: [(GroupOrder, &str); 4] = [
    (GroupOrder::Earned, "Maior ganho"),
    (GroupOrder::Pending, "Maior pendente"),
    (GroupOrder::Reversed, "Maior estorno"),
    (GroupOrder::Name, "Nome"),
];

#[derive(Default)]
pub struct CommissionDashboard {
    commissions: Vec<Commission>,
    grouping: Grouping,
    order: GroupOrder,
    reports: Vec<CommissionReport>,
}

impl CommissionDashboard {
    /// Sums up `commissions`, the period's, of every status.
    pub fn show(&mut self, commissions: Vec<Commission>, cx: &mut Context<Self>) {
        self.commissions = commissions;
        self.sort(self.order, cx);
    }

    fn sort(&mut self, order: GroupOrder, cx: &mut Context<Self>) {
        self.order = order;
        self.reports = CommissionReport::of(&self.commissions, order);
        cx.notify();
    }

    fn group(&mut self, grouping: Grouping, cx: &mut Context<Self>) {
        self.grouping = grouping;
        cx.notify();
    }

    fn render_totals(&self, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let several = self.reports.len() > 1;
        let label = |name: &str, total: &CurrencyTotals| {
            if several {
                format!("{name} em {}", total.currency.code())
            } else {
                name.to_owned()
            }
        };
        v_flex()
            .gap_2()
            .children(self.reports.iter().map(|report| {
                let total = &report.totals;
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .child(figure(
                        label("Ganho", total),
                        total.earned().to_pt_br(),
                        t.text,
                        &t,
                    ))
                    .child(figure(
                        label("Pendente", total),
                        total.pending.to_pt_br(),
                        t.text,
                        &t,
                    ))
                    .child(figure(
                        label("Paga", total),
                        total.paid.to_pt_br(),
                        t.text,
                        &t,
                    ))
                    .child(figure(
                        label("Estornada", total),
                        total.reversed.to_pt_br(),
                        reversed_ink(total, &t),
                        &t,
                    ))
            }))
            .child(div().text_xs().text_color(t.text2).child(
                "Ganho soma as comissões aprovadas e pagas; os estornos ficam fora dele e as \
                 vendas canceladas não entram em nada. Cada moeda soma à parte, sem conversão.",
            ))
            .into_any_element()
    }

    fn render_groups(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let chip = |id: (&'static str, usize), label: &'static str, on: bool| {
            let button = Button::new(id).label(label).small();
            if on { button.primary() } else { button.ghost() }
        };
        let groupings = Grouping::ALL.into_iter().enumerate().map(|(at, grouping)| {
            chip(("grouping", at), grouping.name(), self.grouping == grouping)
                .on_click(cx.listener(move |this, _, _, cx| this.group(grouping, cx)))
        });
        let orders = ORDERS.into_iter().enumerate().map(|(at, (order, name))| {
            chip(("group-order", at), name, self.order == order)
                .on_click(cx.listener(move |this, _, _, cx| this.sort(order, cx)))
        });
        let several = self.reports.len() > 1;
        v_flex()
            .gap_3()
            .child(kit::section_heading("O que rende mais"))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_4()
                    .child(h_flex().gap_1().children(groupings))
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(div().text_sm().text_color(t.text2).child("Ordenar por"))
                            .children(orders),
                    ),
            )
            .children(self.reports.iter().map(|report| {
                let rows = match self.grouping {
                    Grouping::Platform => rows(&report.by_platform, &t, |of| of.name().to_owned()),
                    Grouping::Program => rows(&report.by_program, &t, |of| {
                        if of.name == of.platform.name() {
                            of.name.clone()
                        } else {
                            format!("{} · {}", of.name, of.platform.name())
                        }
                    }),
                    Grouping::Product => rows(&report.by_product, &t, Clone::clone),
                    Grouping::Tracking => rows(&report.by_tracking, &t, |of| {
                        of.clone().unwrap_or_else(|| "sem marcação".into())
                    }),
                };
                v_flex()
                    .gap_2()
                    .when(several, |table| {
                        table.child(
                            div()
                                .text_sm()
                                .font_medium()
                                .child(format!("Em {}", report.currency.code())),
                        )
                    })
                    .child(header(self.grouping.name(), &t))
                    .children(rows)
            }))
            .child(div().text_xs().text_color(t.text2).child(
                "Marcação é o que vai no link de afiliado: os subIds da Shopee, o src ou sck da \
                 Hotmart, o src da Monetizze, o sck ou src da Kiwify, o utm_content ou tracker da \
                 Eduzz, o tracking id da Amazon. Os links criados aqui levam a marcação do canal \
                 de divulgação.",
            ))
            .into_any_element()
    }
}

/// A row per group, named by `name`, in their order.
fn rows<K>(
    groups: &[CommissionGroup<K>],
    t: &Tokens,
    name: impl Fn(&K) -> String,
) -> Vec<AnyElement> {
    groups
        .iter()
        .map(|group| row(name(&group.of), group, t))
        .collect()
}

fn header(first: &'static str, t: &Tokens) -> AnyElement {
    h_flex()
        .gap_3()
        .px_3()
        .text_xs()
        .text_color(t.text2)
        .child(div().flex_1().child(first))
        .child(money_cell("Comissões"))
        .child(money_cell("Pendente"))
        .child(money_cell("Ganho"))
        .child(money_cell("Estornada"))
        .into_any_element()
}

fn row<K>(name: String, group: &CommissionGroup<K>, t: &Tokens) -> AnyElement {
    let totals = &group.totals;
    table_row(t)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_medium()
                .child(name),
        )
        .child(money_cell(group.commissions.to_string()))
        .child(money_cell(totals.pending.to_pt_br()))
        .child(money_cell(totals.earned().to_pt_br()).font_semibold())
        .child(money_cell(totals.reversed.to_pt_br()).text_color(reversed_ink(totals, t)))
        .into_any_element()
}

fn reversed_ink(totals: &CurrencyTotals, t: &Tokens) -> Hsla {
    if totals.reversed.amount().is_zero() {
        t.text
    } else {
        t.danger
    }
}

fn figure(label: String, text: String, ink: Hsla, t: &Tokens) -> AnyElement {
    v_flex()
        .flex_1()
        .min_w(px(150.))
        .gap_1()
        .p_4()
        .rounded(t.radius_lg)
        .border(t.border_width)
        .border_color(t.frame)
        .bg(t.surface)
        .child(div().text_sm().text_color(t.text2).child(label))
        .child(
            div()
                .text_xl()
                .font_semibold()
                .whitespace_nowrap()
                .text_color(ink)
                .child(text),
        )
        .into_any_element()
}

impl Render for CommissionDashboard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.reports.is_empty() {
            return div().into_any_element();
        }
        v_flex()
            .gap_5()
            .child(self.render_totals(cx))
            .child(self.render_groups(cx))
            .into_any_element()
    }
}
