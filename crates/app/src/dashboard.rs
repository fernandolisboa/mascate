//! Painel (#28): how the business is doing in a period. Revenue, the
//! Realized Margin and the Orders come from Commerce's sales summary (ADR
//! 0020); the margin by Product and by category, the Mercado Pago's money
//! and the sales volume against its limit are summed up by Finance, fed
//! with each sale's lines, the Listings' Products and categories, and each
//! Order's Money Releases (ADR 0026); the capital in stock is Inventory's,
//! at the Average Cost. Next to the margin, the period's approved and paid
//! affiliate Commissions, read from Affiliate and summed by Finance (#81),
//! and each linked Product's sale margin against its Commissions (#86).

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Div, Entity, SharedString, Subscription, Window, div, px};
use mascate_affiliate::{Affiliate, CommissionStatus};
use mascate_catalog::Catalog;
use mascate_commerce::{Listings, Orders, Sale, SalesPeriod, SalesSummary};
use mascate_finance::{
    CommissionIncome, EarnedCommission, MarginOrder, MarginReport, MarginRow, MoneyRelease,
    ReleaseSummary, SoldLine, SoldProduct, Taxes, VolumeCheck,
};
use mascate_inventory::Inventory;
use mascate_kernel::{Currency, Money, RecordId, SalesChannelKind, Timestamp};
use mascate_marketing::ProductAds;

use crate::ads::{self, AdsSyncs};
use crate::affiliate::{self, CommissionChanges};
use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE};
use crate::finance::{self, VolumeSources};
use crate::forms::{Outcome, day_text, notice};
use crate::kit;
use crate::layout;
use crate::listings::{self, listings};
use crate::orders::{self, OrderSyncs};
use crate::parts::ScreenParts;
use crate::pricing;
use crate::resell_or_promote::{self, Compared};
use crate::sales::{amount_text, margin_cell, money_cell, orders_text, table_row};
use crate::span::{SpanChanged, SpanPicker};
use crate::stock;

/// What a read of the dashboard brings.
struct Snapshot {
    summary: SalesSummary,
    lines: Vec<SoldLine>,
    capital: Money,
    units: i64,
    releases: ReleaseSummary,
    volume: VolumeCheck,
    commissions: Vec<CommissionIncome>,
    /// The linked Products, reselling against promoting (#86).
    compared: Compared,
}

/// What reading a period's sales line by line needs (#28, #86).
pub(crate) struct SaleSources {
    orders: Arc<Orders>,
    taxes: Arc<Taxes>,
    listings: Arc<Listings>,
    catalog: Arc<Catalog>,
    product_ads: Option<Arc<ProductAds>>,
}

/// A period's sales, each line as Finance sums it up, with the Products'
/// names.
pub(crate) struct SoldPeriod {
    pub period: SalesPeriod,
    pub lines: Vec<SoldLine>,
    pub names: BTreeMap<RecordId, String>,
}

impl SaleSources {
    pub(crate) fn of(cx: &App) -> Option<Self> {
        Some(Self {
            orders: orders::orders(cx)?,
            taxes: pricing::taxes(cx)?,
            listings: listings(cx)?,
            catalog: catalog::catalog(cx)?,
            product_ads: ads::product_ads(cx),
        })
    }

    pub(crate) async fn read(&self, during: Range<Timestamp>) -> Result<SoldPeriod, String> {
        let tax = self
            .taxes
            .rate()
            .await
            .map_err(|e| format!("Não consegui ler a alíquota de imposto: {e}"))?;
        let ad_costs = ads::costs(self.product_ads.as_deref()).await?;
        let period = self
            .orders
            .sales(tax, during, &ad_costs)
            .await
            .map_err(|e| orders::failure(&e))?;
        let sold_as = SoldAs::read(&self.listings, &self.catalog).await?;
        let lines = period
            .sales
            .iter()
            .flat_map(|sale| sold_lines(sale, &sold_as))
            .collect();
        Ok(SoldPeriod {
            period,
            lines,
            names: sold_as.products,
        })
    }
}

/// What the dashboard reads from the rest of the app.
struct Sources {
    sales: SaleSources,
    inventory: Arc<Inventory>,
    volume: VolumeSources,
    affiliate: Arc<Affiliate>,
}

impl Sources {
    fn of(cx: &App) -> Option<Self> {
        Some(Self {
            sales: SaleSources::of(cx)?,
            inventory: stock::inventory(cx)?,
            volume: VolumeSources::of(cx)?,
            affiliate: affiliate::affiliate(cx)?,
        })
    }
}

async fn read(sources: Sources, during: Range<Timestamp>) -> Result<Snapshot, String> {
    let sold = sources.sales.read(during.clone()).await?;
    let summary = SalesSummary::of(&sold.period, Currency::Brl)
        .map_err(|e| format!("Vendas em moedas diferentes: {e}"))?;
    let stock = sources
        .inventory
        .stock()
        .await
        .map_err(|e| stock::inventory_failure(&e))?;
    let capital = stock
        .value()
        .into_iter()
        .find(|value| value.currency() == Currency::Brl)
        .unwrap_or(Money::zero(Currency::Brl));
    let releases: Vec<MoneyRelease> = sources
        .sales
        .orders
        .money_releases()
        .await
        .map_err(|e| orders::failure(&e))?
        .into_iter()
        .map(|payment| MoneyRelease {
            amount: payment.amount,
            released: payment.released,
            on: payment.release_at,
        })
        .collect();
    let releases = ReleaseSummary::of(&releases, during.clone(), Currency::Brl)
        .map_err(|e| format!("Valores do Mercado Pago em moedas diferentes: {e}"))?;
    let commissions = resell_or_promote::earned(&sources.affiliate, during).await?;
    let earned: Vec<EarnedCommission> = commissions
        .iter()
        .map(|commission| EarnedCommission {
            amount: commission.amount,
            paid: commission.status == CommissionStatus::Paid,
        })
        .collect();
    let compared =
        resell_or_promote::compare_linked(&sources.affiliate, &sold, &commissions).await?;
    Ok(Snapshot {
        summary,
        lines: sold.lines,
        capital,
        units: stock.units(),
        releases,
        volume: finance::volume(&sources.volume).await?,
        commissions: CommissionIncome::of(&earned),
        compared,
    })
}

/// A listing by its channel and the channel's id of the listing and of the
/// variation.
type ListingKey = (SalesChannelKind, String, Option<String>);

/// What a listing sold as: its Product and the name of its category.
type ListedAs = (Option<RecordId>, Option<String>);

/// What each listing sold as.
struct SoldAs {
    listings: BTreeMap<ListingKey, ListedAs>,
    products: BTreeMap<RecordId, String>,
}

impl SoldAs {
    async fn read(listings: &Listings, catalog: &Catalog) -> Result<Self, String> {
        let mut names = BTreeMap::new();
        for channel in SalesChannelKind::ALL {
            let named = listings
                .category_names(channel)
                .await
                .map_err(|e| listings::failure(&e))?;
            names.insert(channel, named);
        }
        let products = catalog
            .products()
            .await
            .map_err(|e| catalog::failure(&e))?
            .into_iter()
            .map(|product| (product.id, product.name))
            .collect();
        let listings = listings
            .listings()
            .await
            .map_err(|e| listings::failure(&e))?
            .into_iter()
            .map(|listing| {
                let category = listing
                    .listed
                    .category
                    .as_ref()
                    .and_then(|id| names.get(&listing.channel)?.get(id).cloned());
                let variation = listing.listed.variation.map(|variation| variation.id);
                (
                    (listing.channel, listing.listed.id, variation),
                    (listing.product, category),
                )
            })
            .collect();
        Ok(Self { listings, products })
    }
}

/// Each line of `sale` as Finance sums it up: its Product, or the title
/// sold for a listing without one, and its listing's category.
fn sold_lines(sale: &Sale, sold_as: &SoldAs) -> Vec<SoldLine> {
    sale.order
        .lines
        .iter()
        .zip(&sale.lines)
        .map(|(line, share)| {
            let key = (
                sale.order.channel,
                share.item.clone(),
                share.variation.clone(),
            );
            let (product, category) = sold_as.listings.get(&key).cloned().unwrap_or_default();
            let name = product
                .and_then(|id| sold_as.products.get(&id).cloned())
                .unwrap_or_else(|| line.sold.title.clone());
            SoldLine {
                order: sale.order.sold.id.clone(),
                product: SoldProduct { id: product, name },
                category,
                units: share.units,
                revenue: share.revenue,
                deductions: Money::new(
                    share.fees.amount() + share.tax.amount(),
                    share.revenue.currency(),
                ),
                cost: share.cost,
            }
        })
        .collect()
}

pub struct DashboardScreen {
    span: Entity<SpanPicker>,
    order: MarginOrder,
    snapshot: Option<Snapshot>,
    report: Option<MarginReport>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl DashboardScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let span = cx.new(|cx| SpanPicker::new(window, cx));
        // Each Order Sync brings Orders, their Fees and their Money
        // Releases; each Sync of Product Ads, what the ads cost; each change
        // to the Commissions, what affiliation earned.
        let subscriptions = vec![
            cx.observe_global::<OrderSyncs>(Self::refresh),
            cx.observe_global::<AdsSyncs>(Self::refresh),
            cx.observe_global::<CommissionChanges>(Self::refresh),
            cx.subscribe(&span, |this, _, _: &SpanChanged, cx| this.refresh(cx)),
        ];
        let mut screen = Self {
            span,
            order: MarginOrder::default(),
            snapshot: None,
            report: None,
            reads: 0,
            outcome: None,
            _subscriptions: subscriptions,
        };
        screen.refresh(cx);
        screen
    }

    /// Reads the period again, as when the screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(sources) = Sources::of(cx) else {
            return;
        };
        let during = self.span.read(cx).during();
        self.reads += 1;
        let read_number = self.reads;
        let reading = cx
            .background_executor()
            .spawn(async move { read(sources, during).await });
        cx.spawn(async move |this, cx| {
            let read_back = reading.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read_number {
                    return;
                }
                match read_back {
                    Ok(snapshot) => {
                        this.outcome = None;
                        this.snapshot = Some(snapshot);
                        this.sort(this.order);
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sort(&mut self, order: MarginOrder) {
        self.order = order;
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        match MarginReport::of(&snapshot.lines, Currency::Brl, order) {
            Ok(report) => self.report = Some(report),
            Err(error) => {
                self.report = None;
                self.outcome = Some(Outcome::Failed(
                    format!("Vendas em moedas diferentes: {error}").into(),
                ));
            }
        }
    }

    fn render_figures(&self, snapshot: &Snapshot, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let summary = &snapshot.summary;
        let margin = summary
            .margin
            .as_ref()
            .map_or_else(|| "–".into(), amount_text);
        let margin_ink = match &summary.margin {
            Some(found) if found.amount.is_negative() => t.danger,
            _ => t.text,
        };
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
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .items_stretch()
                    .gap_3()
                    .child(figure("Receita", summary.revenue.to_pt_br(), &t))
                    .child(
                        figure("Margem realizada", margin, &t)
                            .min_w(px(260.))
                            .text_color(margin_ink),
                    )
                    .children(commission_figures(&snapshot.commissions, &t))
                    .child(figure("Pedidos", summary.orders.to_string(), &t))
                    .child(
                        figure("Capital em estoque", snapshot.capital.to_pt_br(), &t).child(
                            div()
                                .text_xs()
                                .text_color(t.text2)
                                .child(units_text(snapshot.units)),
                        ),
                    ),
            )
            .children(
                caveats
                    .into_iter()
                    .map(|text| div().text_sm().text_color(t.text2).child(text)),
            )
            .child(div().text_xs().text_color(t.text2).child(
                "Comissões de afiliado: as aprovadas e pagas das vendas do período, na moeda da \
                 plataforma e sem conversão; estornos e vendas canceladas ficam fora. O detalhe \
                 fica em Afiliação. Capital em estoque: as unidades de hoje pelo custo médio, \
                 qualquer que seja o período.",
            ))
            .into_any_element()
    }

    fn render_margins(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(report) = &self.report else {
            return div().into_any_element();
        };
        let t = look(cx).tokens;
        let orders = [
            (MarginOrder::Margin, "Menor margem"),
            (MarginOrder::Percent, "Menor margem %"),
            (MarginOrder::Revenue, "Maior receita"),
            (MarginOrder::Name, "Nome"),
        ];
        let sorting = h_flex()
            .gap_2()
            .items_center()
            .child(div().text_sm().text_color(t.text2).child("Ordenar por"))
            .children(orders.into_iter().enumerate().map(|(at, (order, name))| {
                let button = Button::new(("order", at))
                    .label(name)
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.sort(order);
                        cx.notify();
                    }));
                if order == self.order {
                    button.primary()
                } else {
                    button.ghost()
                }
            }));
        if report.by_product.is_empty() {
            return v_flex()
                .gap_3()
                .child(kit::section_heading("Margem por produto e categoria"))
                .child(div().text_sm().text_color(t.text2).child(
                    "Nenhuma venda neste período. As vendas chegam pelo Sync de pedidos do \
                     Mercado Livre.",
                ))
                .into_any_element();
        }
        let products = report.by_product.iter().map(|row| {
            let name = match row.of.id {
                Some(_) => row.of.name.clone(),
                None => format!("{} (anúncio sem produto)", row.of.name),
            };
            margin_row(name, row, &t, cx)
        });
        let categories = report.by_category.iter().map(|row| {
            let name = row
                .of
                .clone()
                .unwrap_or_else(|| "Categoria desconhecida".into());
            margin_row(name, row, &t, cx)
        });
        v_flex()
            .gap_3()
            .child(kit::section_heading("Margem por produto"))
            .child(sorting)
            .child(
                v_flex()
                    .gap_2()
                    .child(margin_header("Produto", &t))
                    .children(products),
            )
            .child(kit::section_heading("Margem por categoria"))
            .child(
                v_flex()
                    .gap_2()
                    .child(margin_header("Categoria do Mercado Livre", &t))
                    .children(categories),
            )
            .child(div().text_xs().text_color(t.text2).child(
                "A margem de cada linha é a realizada: receita menos tarifas, Product Ads, \
                 imposto e custo. Num pedido com mais de um item, receita, tarifas e imposto se \
                 dividem pelo valor de cada item. Vendas sem custo ficam fora da margem; a \
                 categoria é a do anúncio no Mercado Livre, lida no Sync de anúncios.",
            ))
            .into_any_element()
    }

    fn render_releases(&self, snapshot: &Snapshot, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let releases = &snapshot.releases;
        let mut column = v_flex()
            .gap_3()
            .child(kit::section_heading("Mercado Pago"))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .child(figure("A liberar", releases.pending.to_pt_br(), &t))
                    .child(figure(
                        "Liberado no período",
                        releases.released.to_pt_br(),
                        &t,
                    )),
            );
        let mut next: Vec<String> = releases
            .schedule
            .iter()
            .map(|(day, amount)| format!("{}: {}", day_text(*day), amount.to_pt_br()))
            .collect();
        if !releases.undated.amount().is_zero() {
            next.push(format!(
                "sem data ainda (o Mercado Pago marca depois da entrega): {}",
                releases.undated.to_pt_br()
            ));
        }
        if !next.is_empty() {
            column = column.child(
                div()
                    .text_sm()
                    .child(format!("Quando cai: {}.", next.join(" · "))),
            );
        }
        column
            .child(div().text_xs().text_color(t.text2).child(
                "O valor de cada venda do Mercado Livre é o líquido que o Mercado Pago informa \
                 para o pagamento, lido depois de cada Sync de pedidos. Outras entradas da conta \
                 do Mercado Pago não entram aqui.",
            ))
            .into_any_element()
    }

    fn render_volume(&self, volume: &VolumeCheck, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let text = format!(
            "Vendas dos últimos 12 meses: {} de um limite de {} (Configurações › Finanças).",
            volume.sold.to_pt_br(),
            volume.limit.to_pt_br()
        );
        let row = h_flex()
            .gap_2()
            .items_center()
            .text_sm()
            .child(div().child(text));
        if volume.passed() {
            row.child(kit::tag(
                "passou do limite: veja o lembrete em Hoje",
                t.accent_text,
                cx,
            ))
            .into_any_element()
        } else {
            row.into_any_element()
        }
    }
}

fn figure(label: impl Into<SharedString>, text: String, t: &Tokens) -> Div {
    v_flex()
        .flex_1()
        .min_w(px(170.))
        .gap_1()
        .p_4()
        .rounded(t.radius_lg)
        .border(t.border_width)
        .border_color(t.frame)
        .bg(t.surface)
        .child(div().text_sm().text_color(t.text2).child(label.into()))
        .child(
            div()
                .text_xl()
                .font_semibold()
                .whitespace_nowrap()
                .child(text),
        )
}

/// The period's earned Commissions, one figure per currency; none earned
/// shows as zero reais.
fn commission_figures(income: &[CommissionIncome], t: &Tokens) -> Vec<Div> {
    let several = income.len() > 1;
    if income.is_empty() {
        return vec![figure(
            "Comissões de afiliado",
            Money::zero(Currency::Brl).to_pt_br(),
            t,
        )];
    }
    income
        .iter()
        .map(|income| {
            let label = if several {
                format!("Comissões de afiliado em {}", income.currency.code())
            } else {
                "Comissões de afiliado".to_owned()
            };
            figure(label, income.earned().to_pt_br(), t).child(
                div().text_xs().text_color(t.text2).child(format!(
                    "aprovadas {} · pagas {}",
                    income.approved.to_pt_br(),
                    income.paid.to_pt_br()
                )),
            )
        })
        .collect()
}

fn margin_header(first: &'static str, t: &Tokens) -> AnyElement {
    h_flex()
        .gap_3()
        .px_3()
        .text_xs()
        .text_color(t.text2)
        .child(div().flex_1().child(first))
        .child(money_cell("Pedidos"))
        .child(money_cell("Unidades"))
        .child(money_cell("Receita"))
        .child(margin_cell().child("Margem"))
        .child(margin_cell().child(""))
        .into_any_element()
}

/// One Product or category, marked when it loses money.
fn margin_row<K>(name: String, row: &MarginRow<K>, t: &Tokens, cx: &App) -> AnyElement {
    let margin = row.margin.as_ref().map_or_else(|| "–".into(), amount_text);
    let (verdict, ink) = if row.loses() {
        (Some("dá prejuízo"), t.danger)
    } else if row.margin.is_none() {
        (Some("sem custo"), t.text2)
    } else {
        (None, t.text2)
    };
    table_row(t)
        .when(row.loses(), |line| line.border_color(t.danger))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_medium()
                .child(name),
        )
        .child(money_cell(row.orders.to_string()))
        .child(money_cell(row.units.to_string()))
        .child(money_cell(row.revenue.to_pt_br()))
        .child(
            margin_cell()
                .font_semibold()
                .text_color(if row.loses() { t.danger } else { t.text })
                .child(margin),
        )
        .child(
            margin_cell()
                .flex()
                .justify_end()
                .children(verdict.map(|verdict| kit::tag(verdict, ink, cx))),
        )
        .into_any_element()
}

fn units_text(units: i64) -> String {
    match units {
        1 => "1 unidade em estoque".into(),
        units => format!("{units} unidades em estoque"),
    }
}

impl Render for DashboardScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut parts = ScreenParts::new("Painel");
        if orders::orders(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        parts.actions.push(self.span.clone().into_any_element());
        if let Some(outcome) = &self.outcome {
            parts.notices.push(notice(outcome, cx));
        }
        let Some(snapshot) = self.snapshot.take() else {
            return layout::screen(parts, cx);
        };
        parts.content.push(self.render_figures(&snapshot, cx));
        parts.content.push(self.render_volume(&snapshot.volume, cx));
        parts.content.push(self.render_margins(cx));
        parts
            .content
            .extend(resell_or_promote::painel_section(&snapshot.compared, cx));
        parts.content.push(self.render_releases(&snapshot, cx));
        self.snapshot = Some(snapshot);
        layout::screen(parts, cx)
    }
}
