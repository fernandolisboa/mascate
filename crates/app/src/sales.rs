//! Vendas (#22): each Order with its Realized Margin, the period's totals on
//! top, and the margin of one Order Fee by Fee, which Pedidos shows in each
//! Order too. Fees come from Mercado Livre's billing after each Order Sync;
//! until it bills an Order, its margin shows as provisional. The rules live
//! in Commerce (ADR 0020). Product Ads (#27) take their share of each sale
//! and, while the Reputation unlocks them, show each Product's ROAS against
//! its break-even (ADR 0025).

use std::ops::Range;

use chrono::{Datelike, Local, Months, NaiveDate, TimeZone, Utc};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Div, Hsla, SharedString, Subscription, Window, div, px};
use mascate_commerce::{FeeKind, OrderStatus, RealizedMargin, Sale, SalesPeriod, SalesSummary};
use mascate_kernel::{Currency, Margin, Money, Timestamp};
use mascate_marketing::{AdMetrics, AdsReport, AdsSyncState, ProductRoas, SellerTool};

use crate::ads::{self, AdsSyncs, ReportSources};
use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE};
use crate::connections::AppConnections;
use crate::forms::{Outcome, notice};
use crate::kit;
use crate::layout;
use crate::listings::listings;
use crate::mercado_livre;
use crate::orders::{self, OrderSyncs, sold_text};
use crate::parts::ScreenParts;
use crate::pricing;
use crate::reputation::reputation;

/// Every sale, whenever it was.
pub fn ever() -> Range<Timestamp> {
    Timestamp::MIN_UTC..Timestamp::MAX_UTC
}

/// The sales listed: one calendar month or all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Period {
    ThisMonth,
    LastMonth,
    All,
}

impl Period {
    const ALL: [Period; 3] = [Period::ThisMonth, Period::LastMonth, Period::All];

    fn name(self) -> &'static str {
        match self {
            Period::ThisMonth => "Este mês",
            Period::LastMonth => "Mês passado",
            Period::All => "Tudo",
        }
    }

    /// The period's time, by the computer's calendar.
    fn range(self, today: NaiveDate) -> Range<Timestamp> {
        let this_month = today.with_day(1).unwrap_or(today);
        let start = |day: NaiveDate| {
            Local
                .from_local_datetime(&day.and_time(chrono::NaiveTime::MIN))
                .earliest()
                .map_or(Timestamp::MIN_UTC, |at| at.with_timezone(&Utc))
        };
        let month_after = |day: NaiveDate| day.checked_add_months(Months::new(1)).unwrap_or(day);
        let month_before = |day: NaiveDate| day.checked_sub_months(Months::new(1)).unwrap_or(day);
        match self {
            Period::ThisMonth => start(this_month)..start(month_after(this_month)),
            Period::LastMonth => start(month_before(this_month))..start(this_month),
            Period::All => ever(),
        }
    }
}

/// What the Product Ads section shows.
enum AdsView {
    /// The Reputation does not unlock Product Ads, or was never read.
    Locked,
    Reading,
    Read {
        report: AdsReport,
        last_sync: Option<AdsSyncState>,
    },
    Failed(String),
}

/// What one read of the screen brings.
struct Snapshot {
    sold: SalesPeriod,
    ads_unlocked: bool,
    last_ads_sync: Option<AdsSyncState>,
}

pub struct SalesScreen {
    period: Period,
    sold: SalesPeriod,
    ads: AdsView,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl SalesScreen {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
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
            ads: AdsView::Locked,
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
        let sold = self.period.range(Local::now().date_naive());
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            let tax = taxes
                .rate()
                .await
                .map_err(|e| format!("Não consegui ler a alíquota de imposto: {e}"))?;
            let ad_costs = ads::costs(product_ads.as_deref()).await?;
            let ads_unlocked = match &reputation {
                Some(reputation) => ads::unlocked(reputation).await? == Some(true),
                None => false,
            };
            let last_ads_sync = match &product_ads {
                Some(product_ads) => product_ads
                    .last_sync()
                    .await
                    .map_err(|e| ads::failure(&e))?,
                None => None,
            };
            Ok::<_, String>(Snapshot {
                sold: orders
                    .sales(tax, sold, &ad_costs)
                    .await
                    .map_err(|e| orders::failure(&e))?,
                ads_unlocked,
                last_ads_sync,
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
                        if snapshot.ads_unlocked {
                            this.read_ads(snapshot.last_ads_sync, cx);
                        } else {
                            this.ads = AdsView::Locked;
                        }
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Sums up the period's Product Ads by campaign and by Product, each
    /// against its break-even on Mercado Livre's sale fees.
    fn read_ads(&mut self, last_sync: Option<AdsSyncState>, cx: &mut Context<Self>) {
        let (Some(ads), Some(listings), Some(catalog), Some(pricing), Some(taxes)) = (
            ads::product_ads(cx),
            listings(cx),
            catalog::catalog(cx),
            pricing::pricing(cx),
            pricing::taxes(cx),
        ) else {
            return;
        };
        let connected = cx
            .global::<AppConnections>()
            .0
            .state(mascate_integrations::Connection::MercadoLivre)
            == mascate_integrations::ConnectionState::Connected;
        let sources = ReportSources {
            ads,
            listings,
            catalog,
            pricing,
            taxes,
            channel: mercado_livre::adapter(cx).filter(|_| connected),
        };
        let during = self.period.range(Local::now().date_naive());
        let read = self.reads;
        if !matches!(self.ads, AdsView::Read { .. }) {
            self.ads = AdsView::Reading;
        }
        let working = cx
            .background_executor()
            .spawn(async move { ads::report(&sources, during).await });
        cx.spawn(async move |this, cx| {
            let report = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                this.ads = match report {
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
                "{} com margem provisória: o Mercado Livre ainda não faturou as tarifas.",
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
    /// Product Ads in the period: hidden, with why, until the Reputation
    /// unlocks them; then each Product's ROAS against its break-even, and
    /// each campaign.
    fn render_ads(&self, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let note = |text: String| div().text_sm().text_color(t.text2).child(text);
        let body = match &self.ads {
            AdsView::Locked => note(format!(
                "O Product Ads libera com {}. Até lá esta seção fica oculta; quando a reputação \
                 chegar lá, ela mostra o custo, as vendas atribuídas e o ROAS de cada produto, e \
                 o custo entra na margem de cada venda. Acompanhe em Reputação.",
                SellerTool::ProductAds.requirement()
            ))
            .into_any_element(),
            AdsView::Reading => {
                note("Calculando o ROAS de cada produto…".into()).into_any_element()
            }
            AdsView::Failed(error) => kit::error_notice(error.clone(), cx).into_any_element(),
            AdsView::Read { report, last_sync } => self.render_report(report, *last_sync, cx),
        };
        v_flex()
            .gap_3()
            .child(kit::section_heading("Product Ads"))
            .child(body)
            .into_any_element()
    }

    fn render_report(
        &self,
        report: &AdsReport,
        last_sync: Option<AdsSyncState>,
        cx: &App,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let read = match last_sync {
            Some(last) if !last.account => format!(
                "O Mercado Livre não tem uma conta de Product Ads para você (lido em {}). \
                 Campanhas se criam no painel do Mercado Livre.",
                catalog::day_and_time(last.at)
            ),
            Some(last) => format!(
                "Lido uma vez por dia, até ontem; último Sync: {}. Campanhas se criam e mudam no \
                 painel do Mercado Livre.",
                catalog::day_and_time(last.at)
            ),
            None => "O Product Ads é lido uma vez por dia depois do Sync de pedidos.".into(),
        };
        let mut column = v_flex()
            .gap_3()
            .child(div().text_sm().text_color(t.text2).child(read));
        let Some(total) = &report.total else {
            return column
                .child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("Nenhum anúncio patrocinado com custo neste período."),
                )
                .into_any_element();
        };
        column = column.child(
            div()
                .text_sm()
                .child(format!("No período: {}.", metrics_text(total))),
        );
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
            .child(margin_cell().child(""));
        let products = report.products.iter().map(|row| product_row(row, &t, cx));
        column = column
            .child(v_flex().gap_2().child(header).children(products))
            .child(div().text_xs().text_color(t.text2).child(
                "ROAS real: vendas atribuídas pelo Mercado Livre (diretas e indiretas, como no \
                     painel) para cada real investido. ROAS de equilíbrio: 1 ÷ a margem antes de \
                     Ads, ao preço de hoje, na variação de menor margem; abaixo dele o anúncio dá \
                     prejuízo.",
            ));
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
        column.into_any_element()
    }
}

/// One Product's ads, marked when they lose money.
fn product_row(row: &ProductRoas, t: &Tokens, cx: &App) -> AnyElement {
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
        .into_any_element()
}

fn table_row(t: &Tokens) -> Div {
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
pub fn margin_detail(margin: &RealizedMargin, cx: &App) -> AnyElement {
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
        notes.push(
            "O Mercado Livre ainda não faturou este pedido: as tarifas são as que o pedido \
             informa e podem mudar."
                .to_owned(),
        );
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
fn amount_text(margin: &Margin) -> String {
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

fn orders_text(count: usize) -> String {
    match count {
        1 => "1 pedido".into(),
        count => format!("{count} pedidos"),
    }
}

fn money_cell(text: impl Into<SharedString>) -> Div {
    div()
        .w(px(100.))
        .flex_none()
        .text_right()
        .child(text.into())
}

fn margin_cell() -> Div {
    div().w(px(150.)).flex_none().text_right()
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
