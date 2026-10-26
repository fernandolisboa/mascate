//! Vendas (#22): each Order with its Realized Margin, the period's totals on
//! top, and the margin of one Order Fee by Fee, which Pedidos shows in each
//! Order too. Fees come from Mercado Livre's billing after each Order Sync;
//! until it bills an Order, its margin shows as provisional. The rules live
//! in Commerce (ADR 0020). Product Ads (#27) and Shopee Ads (#183) take
//! their share of each sale on their channel and show each Product's ROAS
//! against its break-even there (ADR 0025): Mercado Livre's while the
//! Reputation unlocks them, Shopee's once Shopee lets the shop's ads be
//! read.

use std::ops::Range;

use chrono::{Datelike, Local, Months, NaiveDate, TimeZone, Utc};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Div, Entity, Hsla, SharedString, Subscription, Window, div, px};
use mascate_commerce::{FeeKind, OrderStatus, RealizedMargin, Sale, SalesPeriod, SalesSummary};
use mascate_integrations::ConnectionState;
use mascate_kernel::{Currency, Margin, Money, SalesChannelKind, Timestamp};
use mascate_marketing::{AdMetrics, AdsReport, AdsSyncState, ProductRoas, SellerTool};

use crate::ad_campaigns::AdCampaignsPanel;
use crate::ad_groups::AdGroupsPanel;
use crate::ads::{self, AdsSyncs};
use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE};
use crate::forms::{Outcome, notice};
use crate::kit;
use crate::layout;
use crate::orders::{self, OrderSyncs, sold_text};
use crate::parts::ScreenParts;
use crate::pricing;
use crate::reputation::reputation;
use crate::sales_channels;

/// Every sale, whenever it was.
pub fn ever() -> Range<Timestamp> {
    Timestamp::MIN_UTC..Timestamp::MAX_UTC
}

/// When `day` starts by the computer's calendar.
pub(crate) fn day_start(day: NaiveDate) -> Timestamp {
    Local
        .from_local_datetime(&day.and_time(chrono::NaiveTime::MIN))
        .earliest()
        .map_or(Timestamp::MIN_UTC, |at| at.with_timezone(&Utc))
}

/// The sales listed: one calendar month or all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Period {
    ThisMonth,
    LastMonth,
    All,
}

impl Period {
    pub(crate) const ALL: [Period; 3] = [Period::ThisMonth, Period::LastMonth, Period::All];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Period::ThisMonth => "Este mês",
            Period::LastMonth => "Mês passado",
            Period::All => "Tudo",
        }
    }

    /// The period's time, by the computer's calendar.
    pub(crate) fn range(self, today: NaiveDate) -> Range<Timestamp> {
        let this_month = today.with_day(1).unwrap_or(today);
        let month_after = |day: NaiveDate| day.checked_add_months(Months::new(1)).unwrap_or(day);
        let month_before = |day: NaiveDate| day.checked_sub_months(Months::new(1)).unwrap_or(day);
        match self {
            Period::ThisMonth => day_start(this_month)..day_start(month_after(this_month)),
            Period::LastMonth => day_start(month_before(this_month))..day_start(this_month),
            Period::All => ever(),
        }
    }
}

/// What one channel's part of the Product Ads section shows.
enum AdsView {
    /// The Reputation does not unlock Product Ads, or was never read.
    Locked,
    /// The channel has no ads account for the owner, or lets no app read
    /// it, as of the last Sync at that time.
    NoAccount(Timestamp),
    Reading,
    Read {
        report: AdsReport,
        last_sync: Option<AdsSyncState>,
    },
    Failed(String),
}

/// Whether a channel's ads show, as one read of the screen finds them.
enum AdsShown {
    Locked,
    Synced(Option<AdsSyncState>),
}

/// What one read of the screen brings.
struct Snapshot {
    sold: SalesPeriod,
    /// Each channel with ads that shows, in the app's order.
    ads: Vec<(SalesChannelKind, AdsShown)>,
}

pub struct SalesScreen {
    period: Period,
    sold: SalesPeriod,
    ads: Vec<(SalesChannelKind, AdsView)>,
    campaigns: Entity<AdCampaignsPanel>,
    ad_groups: Entity<AdGroupsPanel>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl SalesScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Each Order Sync brings new Orders and, after it, their Fees; each
        // Sync of Product Ads, what the ads cost.
        let subscriptions = vec![
            cx.observe_global::<OrderSyncs>(Self::refresh),
            cx.observe_global::<AdsSyncs>(Self::refresh),
        ];
        let mut screen = Self {
            period: Period::ThisMonth,
            sold: SalesPeriod {
                sales: Vec::new(),
                unplaced_ads: Vec::new(),
            },
            ads: Vec::new(),
            campaigns: cx.new(|cx| AdCampaignsPanel::new(window, cx)),
            ad_groups: cx.new(|cx| AdGroupsPanel::new(window, cx)),
            reads: 0,
            outcome: None,
            _subscriptions: subscriptions,
        };
        screen.refresh(cx);
        screen
    }

    /// Reads the sales of the period again, as when the screen comes into
    /// view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(orders), Some(taxes)) = (orders::orders(cx), pricing::taxes(cx)) else {
            return;
        };
        let product_ads = ads::product_ads(cx);
        let reputation = reputation(cx);
        let ad_channels = sales_channels::reach(cx)
            .only(|channel| channel.ads.is_some())
            .states();
        let sold = self.period.range(Local::now().date_naive());
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            let tax = taxes
                .rate()
                .await
                .map_err(|e| format!("Não consegui ler a alíquota de imposto: {e}"))?;
            let ad_costs = ads::costs(product_ads.as_deref()).await?;
            let ml_unlocked = match &reputation {
                Some(reputation) => ads::unlocked(reputation).await? == Some(true),
                None => false,
            };
            let mut shown = Vec::new();
            for (kind, state) in ad_channels {
                let last_sync = match &product_ads {
                    Some(product_ads) => product_ads
                        .last_sync(kind)
                        .await
                        .map_err(|e| ads::channel_failure(kind, &e))?,
                    None => None,
                };
                // Mercado Livre explains its lock; another channel shows
                // once connected or read.
                let view = match kind {
                    SalesChannelKind::MercadoLivre if !ml_unlocked => AdsShown::Locked,
                    SalesChannelKind::MercadoLivre => AdsShown::Synced(last_sync),
                    _ if state == ConnectionState::Connected || last_sync.is_some() => {
                        AdsShown::Synced(last_sync)
                    }
                    _ => continue,
                };
                shown.push((kind, view));
            }
            Ok::<_, String>(Snapshot {
                sold: orders
                    .sales(tax, sold, &ad_costs)
                    .await
                    .map_err(|e| orders::failure(&e))?,
                ads: shown,
            })
        });
        cx.spawn(async move |this, cx| {
            let read_back = reading.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                match read_back {
                    Ok(snapshot) => {
                        this.sold = snapshot.sold;
                        this.outcome = None;
                        this.show_ads(snapshot.ads, cx);
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows each channel's ads as `shown`, reading the report of those
    /// with an account; a report already on screen stays until the new
    /// one arrives.
    fn show_ads(&mut self, shown: Vec<(SalesChannelKind, AdsShown)>, cx: &mut Context<Self>) {
        let mut before = std::mem::take(&mut self.ads);
        for (kind, shown) in shown {
            let kept = before
                .iter()
                .position(|(was, _)| *was == kind)
                .map(|at| before.remove(at).1);
            let view = match shown {
                AdsShown::Locked => AdsView::Locked,
                AdsShown::Synced(Some(last)) if !last.account => AdsView::NoAccount(last.at),
                AdsShown::Synced(last_sync) => {
                    self.read_ads(kind, last_sync, cx);
                    match kept {
                        Some(read @ AdsView::Read { .. }) => read,
                        _ => AdsView::Reading,
                    }
                }
            };
            self.ads.push((kind, view));
        }
    }

    /// Sums up the period's ads on `kind` by campaign and by Product, each
    /// against its break-even on the channel's sale fees.
    fn read_ads(
        &mut self,
        kind: SalesChannelKind,
        last_sync: Option<AdsSyncState>,
        cx: &mut Context<Self>,
    ) {
        let Some(sources) = ads::report_sources(cx, kind) else {
            return;
        };
        let during = self.period.range(Local::now().date_naive());
        let read = self.reads;
        let working = cx
            .background_executor()
            .spawn(async move { ads::report(&sources, during).await });
        cx.spawn(async move |this, cx| {
            let report = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                if let (SalesChannelKind::MercadoLivre, Ok(report)) = (kind, &report) {
                    let suggested = report
                        .products
                        .iter()
                        .filter(|product| product.suggests_pausing())
                        .flat_map(|product| product.listings.iter().cloned())
                        .collect();
                    this.ad_groups
                        .update(cx, |panel, cx| panel.suggest(suggested, cx));
                }
                let Some((_, view)) = this.ads.iter_mut().find(|(was, _)| *was == kind) else {
                    return;
                };
                *view = match report {
                    Ok(report) => AdsView::Read { report, last_sync },
                    Err(error) => AdsView::Failed(error),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn pick(&mut self, period: Period, cx: &mut Context<Self>) {
        self.period = period;
        self.refresh(cx);
        cx.notify();
    }

    fn render_periods(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .gap_2()
            .children(Period::ALL.into_iter().map(|period| {
                let button = Button::new(("period", period as usize))
                    .label(period.name())
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| this.pick(period, cx)));
                if period == self.period {
                    button.primary()
                } else {
                    button.ghost()
                }
            }))
            .into_any_element()
    }

    fn render_summary(&self, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let summary = match SalesSummary::of(&self.sold, Currency::Brl) {
            Ok(summary) => summary,
            Err(error) => {
                return kit::error_notice(format!("Vendas em moedas diferentes: {error}"), cx)
                    .into_any_element();
            }
        };
        let wide_figure = |label: &'static str, text: String, min_width: f32| {
            v_flex()
                .flex_1()
                .min_w(px(min_width))
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
                        .child(text),
                )
        };
        let figure = |label, text| wide_figure(label, text, 150.);
        let margin = summary
            .margin
            .as_ref()
            .map_or_else(|| "–".into(), amount_text);
        let mut caveats = Vec::new();
        if summary.provisional > 0 {
            caveats.push(format!(
                "{} com margem provisória: o canal de venda ainda não faturou as tarifas.",
                orders_text(summary.provisional)
            ));
        }
        if summary.without_cost > 0 {
            caveats.push(format!(
                "{} fora da margem por não ter custo: as unidades não saíram do estoque do app.",
                orders_text(summary.without_cost)
            ));
        }
        if !summary.unplaced_ads.amount().is_zero() {
            caveats.push(format!(
                "{} de Product Ads em anúncios que não venderam no mês: entram na margem do \
                 período, fora dos pedidos.",
                summary.unplaced_ads.to_pt_br()
            ));
        }
        let shows_ads = !summary.ads.amount().is_zero();
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .child(figure("Pedidos", summary.orders.to_string()))
                    .child(figure("Receita", summary.revenue.to_pt_br()))
                    .child(figure("Tarifas", summary.fees.to_pt_br()))
                    .children(shows_ads.then(|| figure("Product Ads", summary.ads.to_pt_br())))
                    .child(figure("Imposto", summary.tax.to_pt_br()))
                    .child(figure("Custo", summary.cost.to_pt_br()))
                    .child(wide_figure("Margem realizada", margin, 260.)),
            )
            .children(
                caveats
                    .into_iter()
                    .map(|text| div().text_sm().text_color(t.text2).child(text)),
            )
            .into_any_element()
    }

    fn render_table(&self, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let sales = &self.sold.sales;
        if sales.is_empty() {
            return div()
                .text_sm()
                .text_color(t.text2)
                .child(
                    "Nenhuma venda neste período. Os pedidos do Mercado Livre chegam pelo Sync de \
                     pedidos, e as tarifas, pelo faturamento do Mercado Livre logo depois.",
                )
                .into_any_element();
        }
        let shows_ads = sales
            .iter()
            .any(|sale| !sale.margin.fees_of(FeeKind::Ads).amount().is_zero());
        let header = h_flex()
            .gap_3()
            .px_3()
            .text_xs()
            .text_color(t.text2)
            .child(div().w(px(90.)).flex_none().child("Data"))
            .child(div().flex_1().child("Pedido"))
            .child(money_cell("Receita"))
            .child(money_cell("Tarifas"))
            .children(shows_ads.then(|| money_cell("Product Ads")))
            .child(money_cell("Imposto"))
            .child(money_cell("Custo"))
            .child(margin_cell().child("Margem"));
        let rows = sales.iter().map(|sale| {
            let order = &sale.order;
            let margin = &sale.margin;
            let (tag, ink) = margin_tag(sale, &t);
            h_flex()
                .gap_3()
                .p_3()
                .items_center()
                .rounded(t.radius_lg)
                .border(t.border_width)
                .border_color(t.frame)
                .bg(t.surface)
                .text_sm()
                .child(
                    div()
                        .w(px(90.))
                        .flex_none()
                        .child(catalog::day(order.sold.ordered_at)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_medium().truncate().child(sold_text(order)))
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(t.text2)
                                        .child(format!("Pedido {}", order.sold.id)),
                                )
                                .children(tag.map(|tag| kit::tag(tag, ink, cx))),
                        ),
                )
                .child(money_cell(margin.revenue.to_pt_br()))
                .child(money_cell(margin.channel_fees().to_pt_br()))
                .children(shows_ads.then(|| money_cell(margin.fees_of(FeeKind::Ads).to_pt_br())))
                .child(money_cell(margin.tax.to_pt_br()))
                .child(money_cell(cost_text(margin)))
                .child(
                    margin_cell()
                        .font_semibold()
                        .text_color(margin_ink(margin, &t))
                        .child(margin_text(margin)),
                )
                .into_any_element()
        });
        v_flex()
            .gap_2()
            .child(header)
            .children(rows)
            .into_any_element()
    }
}

impl SalesScreen {
    /// The ads of each channel in the period: Mercado Livre's hidden, with
    /// why, until the Reputation unlocks them, and a channel's without an
    /// account with how to get one; then each Product's ROAS against its
    /// break-even, and each campaign.
    fn render_ads(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let note = |text: String| div().text_sm().text_color(t.text2).child(text);
        let mut section = v_flex().gap_3().child(kit::section_heading("Product Ads"));
        for (kind, view) in &self.ads {
            let body = match view {
                AdsView::Locked => note(format!(
                    "O Product Ads libera com {}. Até lá esta parte fica oculta; quando a \
                     reputação chegar lá, ela mostra o custo, as vendas atribuídas e o ROAS de \
                     cada produto, e o custo entra na margem de cada venda. Acompanhe em \
                     Reputação.",
                    SellerTool::ProductAds.requirement()
                ))
                .into_any_element(),
                AdsView::NoAccount(at) => note(no_account(*kind, *at)).into_any_element(),
                AdsView::Reading => {
                    note("Calculando o ROAS de cada produto…".into()).into_any_element()
                }
                AdsView::Failed(error) => kit::error_notice(error.clone(), cx).into_any_element(),
                AdsView::Read { report, last_sync } => {
                    self.render_report(*kind, report, *last_sync, cx)
                }
            };
            section = section.child(
                v_flex()
                    .gap_2()
                    .child(div().font_medium().child(kind.name()))
                    .child(body),
            );
        }
        section.into_any_element()
    }

    fn render_report(
        &self,
        kind: SalesChannelKind,
        report: &AdsReport,
        last_sync: Option<AdsSyncState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let read = match last_sync {
            Some(last) => format!(
                "Lido uma vez por dia, até ontem; último Sync: {}.",
                catalog::day_and_time(last.at)
            ),
            None => format!(
                "Os anúncios {} são lidos uma vez por dia depois do Sync de pedidos.",
                sales_channels::of_channel(kind)
            ),
        };
        let mut column = v_flex()
            .gap_3()
            .child(div().text_sm().text_color(t.text2).child(read));
        // Campaigns and their ads are managed on Mercado Livre, with the
        // account a Sync found.
        let managed =
            kind == SalesChannelKind::MercadoLivre && last_sync.is_some_and(|last| last.account);
        let campaigns = managed.then(|| {
            v_flex()
                .gap_4()
                .child(self.campaigns.clone())
                .child(self.ad_groups.clone())
        });
        let Some(total) = &report.total else {
            return column
                .child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("Nenhum anúncio patrocinado com custo neste período."),
                )
                .children(campaigns)
                .into_any_element();
        };
        column = column.child(
            div()
                .text_sm()
                .child(format!("No período: {}.", metrics_text(total))),
        );
        // The suggested pauses get a column of their own, so the figures
        // stay under their headings.
        let pausing = managed && report.products.iter().any(ProductRoas::suggests_pausing);
        let header = h_flex()
            .gap_3()
            .px_3()
            .text_xs()
            .text_color(t.text2)
            .child(div().flex_1().child("Produto"))
            .child(money_cell("Investimento"))
            .child(money_cell("Vendas atribuídas"))
            .child(money_cell("ROAS real"))
            .child(money_cell("ROAS de equilíbrio"))
            .child(margin_cell().child(""))
            .children(pausing.then(action_cell));
        let products: Vec<AnyElement> = report
            .products
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let pause = pausing.then(|| {
                    let listings = row.listings.clone();
                    action_cell().children(row.suggests_pausing().then(|| {
                        Button::new(("pause-losing-ad", index))
                            .label("Pausar anúncio")
                            .outline()
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ad_groups
                                    .update(cx, |panel, cx| panel.pause_any(&listings, cx))
                            }))
                    }))
                });
                product_row(row, pause, &t, cx)
            })
            .collect();
        column = column
            .child(v_flex().gap_2().child(header).children(products))
            .child(div().text_xs().text_color(t.text2).child(roas_note(kind)));
        if !report.campaigns.is_empty() {
            let header = h_flex()
                .gap_3()
                .px_3()
                .text_xs()
                .text_color(t.text2)
                .child(div().flex_1().child("Campanha"))
                .child(money_cell("Investimento"))
                .child(money_cell("Cliques"))
                .child(money_cell("Vendas atribuídas"))
                .child(money_cell("ROAS"));
            let rows = report.campaigns.iter().map(|campaign| {
                let name = match &campaign.campaign {
                    Some(found) => format!("{} · {}", found.name, found.status.name()),
                    None => format!("Campanha {} · encerrada", campaign.id),
                };
                table_row(&t)
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .child(money_cell(campaign.metrics.cost.to_pt_br()))
                    .child(money_cell(campaign.metrics.clicks.to_string()))
                    .child(money_cell(campaign.metrics.attributed.to_pt_br()))
                    .child(money_cell(roas_text(&campaign.metrics)))
            });
            column = column.child(v_flex().gap_2().child(header).children(rows));
        }
        column.children(campaigns).into_any_element()
    }
}

/// Why `kind` shows no ads, as its Sync at `at` found, and how to change
/// it.
fn no_account(kind: SalesChannelKind, at: Timestamp) -> String {
    let read = catalog::day_and_time(at);
    match kind {
        SalesChannelKind::Shopee => format!(
            "A Shopee não libera a API de Ads para a sua loja (lido em {read}). Para o custo do \
             Shopee Ads entrar na margem das vendas da Shopee, peça ao suporte da Shopee, pela \
             Central do Vendedor, a permissão de Ads da Open Platform para o seu app (o do \
             Partner ID em Configurações › Conexões). Liberada a permissão, o app lê o custo \
             uma vez por dia."
        ),
        _ => format!(
            "{} não tem uma conta de Product Ads para você (lido em {read}). Ative o Product \
             Ads em Minha conta › Publicidade {} para criar campanhas aqui.",
            sales_channels::opening(kind),
            sales_channels::on(kind)
        ),
    }
}

/// How the report's ROAS reads on `kind`.
fn roas_note(kind: SalesChannelKind) -> String {
    let attributed = match kind {
        SalesChannelKind::Shopee => {
            "vendas atribuídas pela Shopee (do produto e de outros da loja, até 7 dias depois do \
             clique, como no painel)"
        }
        _ => "vendas atribuídas pelo Mercado Livre (diretas e indiretas, como no painel)",
    };
    let split = match kind {
        SalesChannelKind::Shopee => {
            " A Shopee informa o custo por campanha: o de uma campanha com vários produtos é \
             dividido igualmente entre eles."
        }
        _ => "",
    };
    format!(
        "ROAS real: {attributed} para cada real investido. ROAS de equilíbrio: 1 ÷ a margem \
         antes de Ads, ao preço de hoje, na variação de menor margem; abaixo dele o anúncio dá \
         prejuízo.{split}"
    )
}

/// One Product's ads, marked when they lose money, with the suggested
/// pause when they lose enough to judge.
fn product_row(row: &ProductRoas, pause: Option<Div>, t: &Tokens, cx: &App) -> AnyElement {
    let (verdict, ink) = if row.loses() {
        ("dá prejuízo", t.danger)
    } else if row.break_even.is_none() {
        ("equilíbrio desconhecido", t.text2)
    } else {
        ("se paga", t.text2)
    };
    table_row(t)
        .when(row.loses(), |row| row.border_color(t.danger))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_medium()
                .child(row.name.clone()),
        )
        .child(money_cell(row.metrics.cost.to_pt_br()))
        .child(money_cell(row.metrics.attributed.to_pt_br()))
        .child(
            money_cell(roas_text(&row.metrics))
                .font_semibold()
                .text_color(if row.loses() { t.danger } else { t.text }),
        )
        .child(money_cell(
            row.break_even
                .map_or_else(|| "–".to_owned(), |least| least.to_pt_br()),
        ))
        .child(
            margin_cell()
                .flex()
                .justify_end()
                .child(kit::tag(verdict, ink, cx)),
        )
        .children(pause)
        .into_any_element()
}

pub(crate) fn table_row(t: &Tokens) -> Div {
    h_flex()
        .gap_3()
        .p_3()
        .items_center()
        .rounded(t.radius_lg)
        .border(t.border_width)
        .border_color(t.frame)
        .bg(t.surface)
        .text_sm()
}

/// "R$ 41,00 investidos, R$ 210,00 em vendas atribuídas, ROAS 5,12x, 59
/// cliques".
fn metrics_text(metrics: &AdMetrics) -> String {
    format!(
        "{} investidos, {} em vendas atribuídas, ROAS {}, {} cliques",
        metrics.cost.to_pt_br(),
        metrics.attributed.to_pt_br(),
        roas_text(metrics),
        metrics.clicks
    )
}

fn roas_text(metrics: &AdMetrics) -> String {
    metrics
        .roas()
        .map_or_else(|| "–".to_owned(), |roas| roas.to_pt_br())
}

impl Render for SalesScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut parts = ScreenParts::new("Vendas");
        if orders::orders(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        parts.actions.push(self.render_periods(cx));
        if let Some(outcome) = &self.outcome {
            parts.notices.push(notice(outcome, cx));
        }
        parts.content.push(self.render_summary(cx));
        parts.content.push(
            v_flex()
                .gap_3()
                .child(kit::section_heading(match self.sold.sales.len() {
                    1 => "1 venda".to_owned(),
                    count => format!("{count} vendas"),
                }))
                .child(self.render_table(cx))
                .into_any_element(),
        );
        parts.content.push(self.render_ads(cx));
        layout::screen(parts, cx)
    }
}

/// The margin of one Order Fee by Fee, as each Order in Pedidos shows it.
pub fn margin_detail(margin: &RealizedMargin, channel: SalesChannelKind, cx: &App) -> AnyElement {
    let t = look(cx).tokens;
    let title = if margin.provisional {
        "Margem provisória"
    } else {
        "Margem realizada"
    };
    let mut parts = vec![format!("Receita {}", margin.revenue.to_pt_br())];
    for (kind, name) in [
        (FeeKind::SaleFee, "tarifa de venda"),
        (FeeKind::Shipping, "frete"),
        (FeeKind::Other, "outras tarifas"),
        (FeeKind::Ads, "Product Ads"),
    ] {
        if margin.fees.iter().any(|fee| fee.kind == kind) {
            parts.push(format!("{name} {}", margin.fees_of(kind).to_pt_br()));
        }
    }
    parts.push(format!("imposto {}", margin.tax.to_pt_br()));
    parts.push(format!("custo {}", cost_text(margin)));
    let others: Vec<String> = margin
        .fees
        .iter()
        .filter(|fee| fee.kind == FeeKind::Other)
        .filter_map(|fee| {
            Some(format!(
                "{} {}",
                fee.description.as_deref()?,
                fee.amount.to_pt_br()
            ))
        })
        .collect();
    let mut notes = Vec::new();
    if !others.is_empty() {
        notes.push(format!("Outras tarifas: {}.", others.join("; ")));
    }
    if margin.provisional {
        notes.push(format!(
            "{} ainda não faturou este pedido: as tarifas são as que o pedido informa e \
             podem mudar.",
            sales_channels::opening(channel)
        ));
    }
    if margin.cost.is_none() {
        notes.push(
            "Sem custo: as unidades não saíram do estoque do app (vendidas antes do primeiro \
             Sync, ainda sem baixa ou enviadas pelo fornecedor)."
                .to_owned(),
        );
    }
    v_flex()
        .gap_1()
        .pl_3()
        .border_l_2()
        .border_color(t.border)
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .child(div().font_medium().child(title))
                .child(
                    div()
                        .font_semibold()
                        .text_color(margin_ink(margin, &t))
                        .child(margin_text(margin)),
                ),
        )
        .child(div().text_xs().text_color(t.text2).child(parts.join(" − ")))
        .children(
            notes
                .into_iter()
                .map(|note| div().text_xs().text_color(t.text2).child(note)),
        )
        .into_any_element()
}

/// "R$ 32,10 (17,9%)"; a dash without a cost.
fn margin_text(margin: &RealizedMargin) -> String {
    margin
        .margin
        .as_ref()
        .map_or_else(|| "–".into(), amount_text)
}

/// "R$ 32,10 (17,9%)", or the value alone when nothing was received to
/// take a percent of.
pub(crate) fn amount_text(margin: &Margin) -> String {
    match margin.percent {
        Some(_) => format!(
            "{} ({})",
            margin.amount.to_pt_br(),
            margin.percent_to_pt_br()
        ),
        None => margin.amount.to_pt_br(),
    }
}

fn margin_ink(margin: &RealizedMargin, t: &Tokens) -> Hsla {
    match &margin.margin {
        Some(found) if found.amount.is_negative() => t.danger,
        Some(_) => t.text,
        None => t.text2,
    }
}

fn cost_text(margin: &RealizedMargin) -> String {
    margin
        .cost
        .map_or_else(|| "sem custo".to_owned(), Money::to_pt_br)
}

/// What sets a sale apart in the list: cancelled, provisional or without a
/// cost.
fn margin_tag(sale: &Sale, t: &Tokens) -> (Option<SharedString>, Hsla) {
    if sale.order.sold.status == OrderStatus::Cancelled {
        (Some("cancelado".into()), t.text2)
    } else if sale.margin.provisional {
        (Some("margem provisória".into()), t.accent_text)
    } else if sale.margin.cost.is_none() {
        (Some("sem custo".into()), t.text2)
    } else {
        (None, t.text2)
    }
}

pub(crate) fn orders_text(count: usize) -> String {
    match count {
        1 => "1 pedido".into(),
        count => format!("{count} pedidos"),
    }
}

pub(crate) fn money_cell(text: impl Into<SharedString>) -> Div {
    div()
        .w(px(100.))
        .flex_none()
        .text_right()
        .child(text.into())
}

pub(crate) fn margin_cell() -> Div {
    div().w(px(150.)).flex_none().text_right()
}

/// Room for a row's suggested action.
fn action_cell() -> Div {
    div().w(px(140.)).flex_none().flex().justify_end()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_month_runs_from_its_first_day_to_the_next_month() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        let local = |y, m, d| {
            Local
                .with_ymd_and_hms(y, m, d, 0, 0, 0)
                .earliest()
                .unwrap()
                .with_timezone(&Utc)
        };

        assert_eq!(
            Period::ThisMonth.range(today),
            local(2026, 10, 1)..local(2026, 11, 1)
        );
        assert_eq!(
            Period::LastMonth.range(today),
            local(2026, 9, 1)..local(2026, 10, 1)
        );
        assert_eq!(Period::All.range(today), ever());
    }
}
