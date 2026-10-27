//! The commissions dashboard on Afiliação (#81): the period's totals per
//! currency, split by status, and the Commissions grouped by Platform,
//! Program, product, tracking tag or Promotion Channel (#101), sortable by
//! amount. Reversals show in red and stay out of what was earned; cancelled
//! sales never count. The sums are Affiliate's (`CommissionReport`); the
//! screen hands over the period's Commissions and the channels it read from
//! the Marketing, which name the channel groups. With the period's
//! Promotion Spends, each group also shows its spend, profit and ROAS, and
//! a loss worth judging suggests pausing the campaign on the Platform that
//! runs it (#196); the app never pauses anything. Below them, the clicks on
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
use mascate_kernel::{ChannelSpend, Money, RecordId, Timestamp};
use mascate_marketing::{ChannelKind, PromotionChannel};
use rust_decimal::RoundingStrategy;

use crate::appearance::{Tokens, look};
use crate::catalog::day_and_time;
use crate::kit;
use crate::sales::{money_cell, table_row};
use crate::work_context::WorkContext;

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

const ORDERS: [(GroupOrder, &str); 5] = [
    (GroupOrder::Earned, "Maior ganho"),
    (GroupOrder::Profit, "Maior lucro"),
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

impl Clicks {
    /// The clicks on the links of the Platforms `context` shows; in All,
    /// those of deleted links too.
    pub fn in_context(mut self, context: WorkContext) -> Self {
        if context == WorkContext::All {
            return self;
        }
        self.affiliate_links
            .retain(|link| context.shows_platform(link.program.platform));
        let shown = &self.affiliate_links;
        self.links
            .retain(|clicks| shown.iter().any(|link| link.id == clicks.link));
        self
    }
}

#[derive(Default)]
pub struct CommissionDashboard {
    commissions: Vec<Commission>,
    clicks: Clicks,
    /// Each live Affiliate Link's product and Promotion Channel.
    link_names: HashMap<RecordId, (String, RecordId)>,
    click_grouping: ClickGrouping,
    click_report: ClickReport,
    /// Each live Promotion Channel, by id; `None` until the Marketing
    /// answered.
    channels: Option<HashMap<RecordId, PromotionChannel>>,
    /// What each channel spent in the period, the context's part of it.
    spends: Vec<ChannelSpend>,
    grouping: Grouping,
    order: GroupOrder,
    reports: Vec<CommissionReport>,
}

impl CommissionDashboard {
    /// Sums up `commissions`, the period's, of every status, against what
    /// each channel spent, naming their channels from `channels` when the
    /// Marketing could be read.
    pub fn show(
        &mut self,
        commissions: Vec<Commission>,
        spends: Vec<ChannelSpend>,
        channels: Option<Vec<PromotionChannel>>,
        clicks: Clicks,
        cx: &mut Context<Self>,
    ) {
        self.click_report = ClickReport::of(&clicks.links, &commissions);
        self.link_names = clicks
            .affiliate_links
            .iter()
            .map(|link| (link.id, (link.product.clone(), link.channel)))
            .collect();
        self.clicks = clicks;
        self.commissions = commissions;
        self.spends = spends;
        if let Some(channels) = channels {
            self.channels = Some(
                channels
                    .into_iter()
                    .map(|channel| (channel.id, channel))
                    .collect(),
            );
        }
        self.sort(self.order, cx);
    }

    fn sort(&mut self, order: GroupOrder, cx: &mut Context<Self>) {
        self.order = order;
        let mut reports = CommissionReport::of(&self.commissions, &self.spends, order);
        for report in &mut reports {
            report.arrange_channels(order, |id| self.channel_name(id));
        }
        self.reports = reports;
        cx.notify();
    }

    /// A channel no longer in the Marketing was deleted there.
    fn channel_name(&self, id: RecordId) -> String {
        match &self.channels {
            Some(channels) => channels
                .get(&id)
                .map_or_else(|| "Canal apagado".into(), |channel| channel.name.clone()),
            None => "Canal (nome não lido)".into(),
        }
    }

    /// "Curso de Fotografia · Ofertas no Telegram": a product has a link per
    /// channel.
    fn link_name(&self, id: RecordId) -> String {
        match self.link_names.get(&id) {
            Some((product, channel)) => format!("{product} · {}", self.channel_name(*channel)),
            None => "Link apagado".into(),
        }
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
                let spent = !report.spent.amount().is_zero();
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
                    .when(spent, |figures| {
                        figures
                            .child(figure(
                                label("Gasto com divulgação", total),
                                report.spent.to_pt_br(),
                                t.text,
                                &t,
                            ))
                            .child(figure(
                                label("Lucro", total),
                                report.profit().to_pt_br(),
                                profit_ink(Some(report.profit()), &t),
                                &t,
                            ))
                            .child(figure(
                                label("ROAS", total),
                                roas_text(report.roas()),
                                t.text,
                                &t,
                            ))
                            .child(figure(
                                label("Lucro se as pendentes aprovarem", total),
                                report.profit_if_pending_approve().to_pt_br(),
                                profit_ink(Some(report.profit_if_pending_approve()), &t),
                                &t,
                            ))
                    })
            }))
            .child(div().text_xs().text_color(t.text2).child(
                "Ganho soma as comissões aprovadas e pagas; os estornos ficam fora dele e as \
                 vendas canceladas não entram em nada. Lucro é o ganho menos o gasto com \
                 divulgação lançado em Divulgação para os mesmos dias, e ROAS é o ganho dividido \
                 pelo gasto. Cada moeda soma à parte, sem conversão.",
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
                let spent = !report.spent.amount().is_zero();
                let table = Table { spent, t: &t };
                let rows = match self.grouping {
                    Grouping::Platform => {
                        table.rows(&report.by_platform, |of| (of.name().to_owned(), None))
                    }
                    Grouping::Program => table.rows(&report.by_program, |of| {
                        let name = if of.name == of.platform.name() {
                            of.name.clone()
                        } else {
                            format!("{} · {}", of.name, of.platform.name())
                        };
                        (name, Some(PAUSE_PROMOTION))
                    }),
                    Grouping::Product => {
                        table.rows(&report.by_product, |of| (of.clone(), Some(PAUSE_PROMOTION)))
                    }
                    Grouping::Tracking => table.rows(&report.by_tracking, |of| {
                        (of.clone().unwrap_or_else(|| "sem marcação".into()), None)
                    }),
                    Grouping::Channel => table.rows(&report.by_channel, |of| match of {
                        Some(id) => (self.channel_name(*id), Some(self.pause_text(*id))),
                        None => ("sem canal".into(), None),
                    }),
                };
                let unshared = (self.grouping != Grouping::Channel
                    && !report.unshared.amount().is_zero())
                .then(|| {
                    div().text_xs().text_color(t.text2).child(format!(
                        "{} gastos em canais sem ganho no período não caem em nenhum grupo \
                         daqui; veja em Canal.",
                        report.unshared.to_pt_br()
                    ))
                });
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
                    .child(header(self.grouping.name(), spent, &t))
                    .children(rows)
                    .children(unshared)
            }))
            .child(div().text_xs().text_color(t.text2).child(
                "Marcação é o que vai no link de afiliado: os subIds da Shopee, o src ou sck da \
                 Hotmart, o src da Monetizze, o sck ou src da Kiwify, o utm_content ou tracker da \
                 Eduzz, o tracking id da Amazon. Os links criados aqui levam a marcação do canal \
                 de divulgação, e Canal junta as vendas pela marcação do link, mesmo as que \
                 chegaram antes de o link existir; uma marcação sem link fica em \"sem canal\". \
                 O gasto de um canal com vários produtos é dividido entre eles pelo que cada um \
                 ganhou no período; um produto sem ganho não recebe parte.",
            ))
            .into_any_element()
    }
}

/// What a pause suggests outside a channel: a product or Program may run
/// on several.
const PAUSE_PROMOTION: &str = "Gasto maior que o ganho: pause a divulgação paga dele no painel \
                               do anúncio.";

impl CommissionDashboard {
    /// Where to pause a losing channel, by the Platform it runs on.
    fn pause_text(&self, id: RecordId) -> &'static str {
        let kind = self
            .channels
            .as_ref()
            .and_then(|channels| channels.get(&id))
            .map(|channel| channel.kind);
        match kind {
            Some(ChannelKind::PaidSocial) => {
                "Gasto maior que o ganho: pausar campanha no painel da Meta (ou do TikTok)."
            }
            Some(ChannelKind::PaidSearch) => {
                "Gasto maior que o ganho: pausar campanha no painel do Google Ads."
            }
            _ => "Gasto maior que o ganho: pausar a divulgação paga deste canal.",
        }
    }
}

/// One currency's table of groups.
struct Table<'a> {
    /// Something was spent in the currency, so profit shows.
    spent: bool,
    t: &'a Tokens,
}

impl Table<'_> {
    /// A row per group, in their order, named by `name` with what to say
    /// when it suggests pausing.
    fn rows<K>(
        &self,
        groups: &[CommissionGroup<K>],
        name: impl Fn(&K) -> (String, Option<&'static str>),
    ) -> Vec<AnyElement> {
        groups
            .iter()
            .map(|group| {
                let (name, pause) = name(&group.of);
                self.row(name, pause.filter(|_| group.suggests_pausing()), group)
            })
            .collect()
    }

    fn row<K>(
        &self,
        name: String,
        pause: Option<&'static str>,
        group: &CommissionGroup<K>,
    ) -> AnyElement {
        let t = self.t;
        let totals = &group.totals;
        let detail = (self.spent && (!group.spent.amount().is_zero() || group.mixed_currencies))
            .then(|| div().text_xs().text_color(t.text2).child(spend_text(group)));
        let pause = pause.map(|text| div().text_xs().text_color(t.danger).child(text));
        table_row(t)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().truncate().font_medium().child(name))
                    .children(detail)
                    .children(pause),
            )
            .child(money_cell(group.commissions.to_string()))
            .child(money_cell(totals.pending.to_pt_br()))
            .child(money_cell(totals.earned().to_pt_br()).font_semibold())
            .child(money_cell(totals.reversed.to_pt_br()).text_color(reversed_ink(totals, t)))
            .when(self.spent, |row| {
                row.child(
                    money_cell(money_text(group.profit()))
                        .font_semibold()
                        .text_color(profit_ink(group.profit(), t)),
                )
            })
            .into_any_element()
    }
}

/// "Gasto R$ 90,00 · ROAS 2,22x · R$ 22,50 por venda · R$ 150,00 se as
/// pendentes aprovarem".
fn spend_text<K>(group: &CommissionGroup<K>) -> String {
    if group.mixed_currencies {
        return format!(
            "Gasto {} · ganho e gasto em moedas diferentes: sem lucro, nada é convertido",
            group.spent.to_pt_br()
        );
    }
    let mut parts = vec![
        format!("Gasto {}", group.spent.to_pt_br()),
        format!("ROAS {}", roas_text(group.roas())),
    ];
    if let Some(per_sale) = group.spent_per_sale() {
        parts.push(format!("{} por venda", per_sale.to_pt_br()));
    }
    if !group.totals.pending.amount().is_zero() {
        parts.push(format!(
            "{} se as pendentes aprovarem",
            money_text(group.profit_if_pending_approve())
        ));
    }
    parts.join(" · ")
}

fn money_text(money: Option<Money>) -> String {
    money.map_or_else(|| "–".into(), Money::to_pt_br)
}

/// `2,22x`, or a dash with nothing spent.
fn roas_text(roas: Option<mascate_kernel::Roas>) -> String {
    roas.map_or_else(|| "–".into(), |roas| roas.to_pt_br())
}

fn profit_ink(profit: Option<Money>, t: &Tokens) -> Hsla {
    if profit.is_some_and(|profit| profit.is_negative()) {
        t.danger
    } else {
        t.text
    }
}

fn header(first: &'static str, spent: bool, t: &Tokens) -> AnyElement {
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
        .when(spent, |header| header.child(money_cell("Lucro")))
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
    use mascate_affiliate::{AffiliatePlatform, ProgramRef};
    use mascate_kernel::Currency;
    use rust_decimal::Decimal;

    use super::*;

    fn brl(amount: &str) -> Money {
        Money::new(amount.parse::<Decimal>().unwrap(), Currency::Brl)
    }

    fn promotion_channel(id: u128, kind: ChannelKind) -> PromotionChannel {
        PromotionChannel {
            id: RecordId::from_u128(id),
            name: format!("Canal {id}"),
            kind,
            default_tag: format!("canal{id}"),
            keywords: Vec::new(),
            telegram_chat: None,
            posting_times: Vec::new(),
            pinterest_board: None,
        }
    }

    #[test]
    fn a_losing_channel_points_to_the_panel_of_the_platform_that_runs_it() {
        let dashboard = CommissionDashboard {
            channels: Some(
                [
                    promotion_channel(1, ChannelKind::PaidSocial),
                    promotion_channel(2, ChannelKind::PaidSearch),
                    promotion_channel(3, ChannelKind::Instagram),
                ]
                .into_iter()
                .map(|channel| (channel.id, channel))
                .collect(),
            ),
            ..CommissionDashboard::default()
        };
        let pause = |id| dashboard.pause_text(RecordId::from_u128(id));

        assert!(pause(1).contains("painel da Meta"));
        assert!(pause(2).contains("painel do Google Ads"));
        assert!(pause(3).contains("divulgação paga deste canal"));
        // A deleted channel's kind is not known any more.
        assert_eq!(pause(4), pause(3));
    }

    fn spending(spent: &str, sales: usize, mixed: bool) -> CommissionGroup<()> {
        let mut totals = CurrencyTotals {
            currency: Currency::Brl,
            pending: brl("40"),
            approved: brl("150"),
            paid: brl("50"),
            reversed: brl("0"),
            cancelled: 0,
        };
        if sales == 0 {
            totals.pending = brl("0");
        }
        CommissionGroup {
            of: (),
            totals,
            commissions: sales,
            sales,
            spent: brl(spent),
            mixed_currencies: mixed,
        }
    }

    #[test]
    fn a_group_s_spend_reads_with_its_roas_cost_per_sale_and_pending_profit() {
        assert_eq!(
            spend_text(&spending("90", 4, false)),
            "Gasto R$ 90,00 · ROAS 2,22x · R$ 22,50 por venda · R$ 150,00 se as pendentes \
             aprovarem"
        );
        assert_eq!(
            spend_text(&spending("90", 0, false)),
            "Gasto R$ 90,00 · ROAS 2,22x"
        );
        assert_eq!(
            spend_text(&spending("90", 4, true)),
            "Gasto R$ 90,00 · ganho e gasto em moedas diferentes: sem lucro, nada é convertido"
        );
    }

    fn group(clicks: u64, commissions: usize) -> ClickGroup<()> {
        ClickGroup {
            of: (),
            clicks,
            commissions,
        }
    }

    fn link(id: u128, platform: AffiliatePlatform) -> AffiliateLink {
        AffiliateLink {
            id: RecordId::from_u128(id),
            program: ProgramRef {
                id: RecordId::from_u128(id + 100),
                platform,
                name: platform.name().into(),
            },
            product: "Curso".into(),
            channel: RecordId::from_u128(9),
            tag: "canal".into(),
            pasted: String::new(),
            tagged: String::new(),
            short: None,
            created_at: Timestamp::UNIX_EPOCH,
        }
    }

    fn clicked(link: u128) -> LinkClicks {
        LinkClicks {
            link: RecordId::from_u128(link),
            channel: RecordId::from_u128(9),
            shortened_at: Timestamp::UNIX_EPOCH,
            clicks: 5,
        }
    }

    /// The live Hotmart and Kiwify links and a deleted one, each clicked.
    fn clicks() -> Clicks {
        Clicks {
            links: vec![clicked(1), clicked(2), clicked(3)],
            affiliate_links: vec![
                link(1, AffiliatePlatform::Hotmart),
                link(2, AffiliatePlatform::Kiwify),
            ],
            read_at: None,
        }
    }

    fn clicked_links(clicks: &Clicks) -> Vec<RecordId> {
        clicks.links.iter().map(|clicks| clicks.link).collect()
    }

    #[test]
    fn a_platform_s_context_counts_only_the_clicks_on_its_links() {
        let hotmart = clicks().in_context(WorkContext::Hotmart);
        assert_eq!(clicked_links(&hotmart), [RecordId::from_u128(1)]);
        assert_eq!(
            hotmart.affiliate_links,
            [link(1, AffiliatePlatform::Hotmart)]
        );
        let shopee = clicks().in_context(WorkContext::Shopee);
        assert!(shopee.links.is_empty() && shopee.affiliate_links.is_empty());
        // All keeps the clicks of a deleted link too.
        assert_eq!(
            clicked_links(&clicks().in_context(WorkContext::All)),
            clicked_links(&clicks())
        );
    }

    #[test]
    fn clicks_per_commission_reads_with_one_decimal_or_a_dash() {
        assert_eq!(clicks_per_commission_text(&group(25, 2)), "12,5");
        assert_eq!(clicks_per_commission_text(&group(40, 2)), "20");
        assert_eq!(clicks_per_commission_text(&group(10, 3)), "3,3");
        assert_eq!(clicks_per_commission_text(&group(7, 0)), "–");
    }
}
