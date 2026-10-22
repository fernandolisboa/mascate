//! The commissions dashboard on Afiliação (#81): the period's totals per
//! currency, split by status, and the Commissions grouped by Platform,
//! Program, product, tracking tag or Promotion Channel (#101), sortable by
//! amount. Reversals show in red and stay out of what was earned; cancelled
//! sales never count. The sums are Affiliate's (`CommissionReport`); the
//! screen hands over the period's Commissions and the channels it read from
//! the Marketing, which name the channel groups. Below them, the clicks on
//! Short Links by link and by channel, beside the Commissions they brought
//! (#159; `ClickReport`).

use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Hsla, Window, div, px};
use mascate_affiliate::{
    AffiliateLink, ClickGroup, ClickReport, Commission, CommissionGroup, CommissionReport,
    CurrencyTotals, GroupOrder, LinkClicks,
};
use mascate_kernel::{RecordId, Timestamp};
use mascate_marketing::PromotionChannel;
use rust_decimal::RoundingStrategy;

use crate::appearance::{Tokens, look};
use crate::catalog::day_and_time;
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
    Channel,
}

impl Grouping {
    const ALL: [Grouping; 5] = [
        Grouping::Platform,
        Grouping::Program,
        Grouping::Product,
        Grouping::Tracking,
        Grouping::Channel,
    ];

    fn name(self) -> &'static str {
        match self {
            Grouping::Platform => "Plataforma",
            Grouping::Program => "Programa",
            Grouping::Product => "Produto",
            Grouping::Tracking => "Marcação",
            Grouping::Channel => "Canal",
        }
    }
}

const ORDERS: [(GroupOrder, &str); 4] = [
    (GroupOrder::Earned, "Maior ganho"),
    (GroupOrder::Pending, "Maior pendente"),
    (GroupOrder::Reversed, "Maior estorno"),
    (GroupOrder::Name, "Nome"),
];

/// What the clicks are gathered by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ClickGrouping {
    #[default]
    Channel,
    Link,
}

impl ClickGrouping {
    const ALL: [ClickGrouping; 2] = [ClickGrouping::Channel, ClickGrouping::Link];

    fn name(self) -> &'static str {
        match self {
            ClickGrouping::Channel => "Canal",
            ClickGrouping::Link => "Link",
        }
    }
}

/// The period's clicks on Short Links, as the screen read them.
#[derive(Default)]
pub struct Clicks {
    pub links: Vec<LinkClicks>,
    /// The live Affiliate Links, to name each link.
    pub affiliate_links: Vec<AffiliateLink>,
    /// When the clicks were last read through.
    pub read_at: Option<Timestamp>,
}

#[derive(Default)]
pub struct CommissionDashboard {
    commissions: Vec<Commission>,
    clicks: Clicks,
    /// Each live Affiliate Link's name: its product and Platform.
    link_names: HashMap<RecordId, String>,
    click_grouping: ClickGrouping,
    click_report: ClickReport,
    /// Each live Promotion Channel's name; `None` until the Marketing
    /// answered.
    channels: Option<HashMap<RecordId, String>>,
    grouping: Grouping,
    order: GroupOrder,
    reports: Vec<CommissionReport>,
}

impl CommissionDashboard {
    /// Sums up `commissions`, the period's, of every status, naming their
    /// channels from `channels` when the Marketing could be read.
    pub fn show(
        &mut self,
        commissions: Vec<Commission>,
        channels: Option<Vec<PromotionChannel>>,
        clicks: Clicks,
        cx: &mut Context<Self>,
    ) {
        self.click_report = ClickReport::of(&clicks.links, &commissions);
        self.link_names = clicks
            .affiliate_links
            .iter()
            .map(|link| {
                (
                    link.id,
                    format!("{} · {}", link.product, link.program.platform.name()),
                )
            })
            .collect();
        self.clicks = clicks;
        self.commissions = commissions;
        if let Some(channels) = channels {
            self.channels = Some(
                channels
                    .into_iter()
                    .map(|channel| (channel.id, channel.name))
                    .collect(),
            );
        }
        self.sort(self.order, cx);
    }

    fn sort(&mut self, order: GroupOrder, cx: &mut Context<Self>) {
        self.order = order;
        let mut reports = CommissionReport::of(&self.commissions, order);
        for report in &mut reports {
            report.arrange_channels(order, |id| self.channel_name(id));
        }
        self.reports = reports;
        cx.notify();
    }

    /// A channel no longer in the Marketing was deleted there.
    fn channel_name(&self, id: RecordId) -> String {
        match &self.channels {
            Some(names) => names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| "Canal apagado".into()),
            None => "Canal (nome não lido)".into(),
        }
    }

    fn link_name(&self, id: RecordId) -> String {
        self.link_names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "Link apagado".into())
    }

    fn group_clicks(&mut self, grouping: ClickGrouping, cx: &mut Context<Self>) {
        self.click_grouping = grouping;
        cx.notify();
    }

    fn render_clicks(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let groupings = ClickGrouping::ALL
            .into_iter()
            .enumerate()
            .map(|(at, grouping)| {
                let button = Button::new(("click-grouping", at))
                    .label(grouping.name())
                    .small();
                let button = if self.click_grouping == grouping {
                    button.primary()
                } else {
                    button.ghost()
                };
                button.on_click(cx.listener(move |this, _, _, cx| this.group_clicks(grouping, cx)))
            });
        let rows: Vec<AnyElement> = match self.click_grouping {
            ClickGrouping::Channel => self
                .click_report
                .by_channel
                .iter()
                .map(|group| click_row(self.channel_name(group.of), group, &t))
                .collect(),
            ClickGrouping::Link => self
                .click_report
                .by_link
                .iter()
                .map(|group| click_row(self.link_name(group.of), group, &t))
                .collect(),
        };
        let read = match self.clicks.read_at {
            Some(at) => format!("Cliques lidos do Short.io em {}.", day_and_time(at)),
            None => "Os cliques ainda não foram lidos do Short.io.".into(),
        };
        v_flex()
            .gap_3()
            .child(kit::section_heading("Cliques nos links curtos"))
            .child(h_flex().gap_1().children(groupings))
            .child(
                h_flex()
                    .gap_3()
                    .px_3()
                    .text_xs()
                    .text_color(t.text2)
                    .child(div().flex_1().child(self.click_grouping.name()))
                    .child(money_cell("Cliques"))
                    .child(money_cell("Comissões"))
                    .child(money_cell("Cliques por comissão")),
            )
            .children(rows)
            .child(div().text_xs().text_color(t.text2).child(format!(
                "{read} Contam só os cliques no link curto, lidos a cada hora com as comissões; \
                 um link sem link curto fica fora, e o Pinterest recebe o link inteiro. \
                 Comissões são as vendas que vieram pelo link depois que ele ganhou o link \
                 curto, as canceladas fora."
            )))
            .into_any_element()
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
                    Grouping::Channel => rows(&report.by_channel, &t, |of| {
                        of.map_or_else(|| "sem canal".into(), |id| self.channel_name(id))
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
                 de divulgação, e Canal junta as vendas pela marcação do link, mesmo as que \
                 chegaram antes de o link existir; uma marcação sem link fica em \"sem canal\".",
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

fn click_row<K>(name: String, group: &ClickGroup<K>, t: &Tokens) -> AnyElement {
    table_row(t)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_medium()
                .child(name),
        )
        .child(money_cell(group.clicks.to_string()).font_semibold())
        .child(money_cell(group.commissions.to_string()))
        .child(money_cell(clicks_per_commission_text(group)))
        .into_any_element()
}

/// `12,5`, or a dash before the first Commission.
fn clicks_per_commission_text<K>(group: &ClickGroup<K>) -> String {
    match group.clicks_per_commission() {
        Some(ratio) => ratio
            .round_dp_with_strategy(1, RoundingStrategy::MidpointAwayFromZero)
            .normalize()
            .to_string()
            .replace('.', ","),
        None => "–".into(),
    }
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
        let clicked = !self.click_report.by_link.is_empty();
        if self.reports.is_empty() && !clicked {
            return div().into_any_element();
        }
        v_flex()
            .gap_5()
            .when(!self.reports.is_empty(), |dashboard| {
                dashboard
                    .child(self.render_totals(cx))
                    .child(self.render_groups(cx))
            })
            .when(clicked, |dashboard| dashboard.child(self.render_clicks(cx)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(clicks: u64, commissions: usize) -> ClickGroup<()> {
        ClickGroup {
            of: (),
            clicks,
            commissions,
        }
    }

    #[test]
    fn clicks_per_commission_reads_with_one_decimal_or_a_dash() {
        assert_eq!(clicks_per_commission_text(&group(25, 2)), "12,5");
        assert_eq!(clicks_per_commission_text(&group(40, 2)), "20");
        assert_eq!(clicks_per_commission_text(&group(10, 3)), "3,3");
        assert_eq!(clicks_per_commission_text(&group(7, 0)), "–");
    }
}
