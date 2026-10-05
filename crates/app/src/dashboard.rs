//! Painel (#28): how the business is doing in a period. Revenue, the
//! Realized Margin and the Orders come from Commerce's sales summary (ADR
//! 0020); the margin by Product and by category, the Mercado Pago's money
//! and the sales volume against its limit are summed up by Finance, fed
//! with each sale's lines, the Listings' Products and categories, and each
//! Order's Money Releases (ADR 0026); the capital in stock is Inventory's,
//! at the Average Cost.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use chrono::{Local, NaiveDate, TimeDelta};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Div, Entity, SharedString, Subscription, Window, div, px};
use mascate_catalog::Catalog;
use mascate_commerce::{Listings, Orders, Sale, SalesSummary};
use mascate_finance::{
    MarginOrder, MarginReport, MarginRow, MoneyRelease, ReleaseSummary, SoldLine, SoldProduct,
    Taxes, VolumeCheck,
};
use mascate_inventory::Inventory;
use mascate_kernel::{Currency, Money, RecordId, Timestamp};
use mascate_marketing::ProductAds;

use crate::ads::{self, AdsSyncs};
use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE};
use crate::finance::{self, VolumeSources};
use crate::forms::{Outcome, day_text, input, notice, parse_day};
use crate::kit;
use crate::layout;
use crate::listings::{self, listings};
use crate::orders::{self, OrderSyncs};
use crate::parts::ScreenParts;
use crate::pricing;
use crate::sales::{amount_text, day_start, margin_cell, money_cell, orders_text, table_row};
use crate::stock;

/// The days the dashboard sums up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Span {
    /// The last few days, today included.
    Days(u16),
    /// The days the owner typed, both included.
    Custom,
}

impl Span {
    const ALL: [Span; 4] = [Span::Days(7), Span::Days(30), Span::Days(90), Span::Custom];

    fn name(self) -> SharedString {
        match self {
            Span::Days(days) => format!("{days} dias").into(),
            Span::Custom => "Personalizado".into(),
        }
    }
}

/// From the start of `from` to the end of `to`, by the computer's calendar.
fn days(from: NaiveDate, to: NaiveDate) -> Range<Timestamp> {
    day_start(from)..day_start(to + TimeDelta::days(1))
}

/// The last `count` days, `today` included.
fn last_days(count: u16, today: NaiveDate) -> Range<Timestamp> {
    days(today - TimeDelta::days(i64::from(count) - 1), today)
}

/// What a read of the dashboard brings.
struct Snapshot {
    summary: SalesSummary,
    lines: Vec<SoldLine>,
    capital: Money,
    units: i64,
    releases: ReleaseSummary,
    volume: VolumeCheck,
}

/// What the dashboard reads from the rest of the app.
struct Sources {
    orders: Arc<Orders>,
    taxes: Arc<Taxes>,
    listings: Arc<Listings>,
    catalog: Arc<Catalog>,
    inventory: Arc<Inventory>,
    product_ads: Option<Arc<ProductAds>>,
    volume: VolumeSources,
}

impl Sources {
    fn of(cx: &App) -> Option<Self> {
        Some(Self {
            orders: orders::orders(cx)?,
            taxes: pricing::taxes(cx)?,
            listings: listings(cx)?,
            catalog: catalog::catalog(cx)?,
            inventory: stock::inventory(cx)?,
            product_ads: ads::product_ads(cx),
            volume: VolumeSources::of(cx)?,
        })
    }
}

async fn read(sources: Sources, during: Range<Timestamp>) -> Result<Snapshot, String> {
    let tax = sources
        .taxes
        .rate()
        .await
        .map_err(|e| format!("Não consegui ler a alíquota de imposto: {e}"))?;
    let ad_costs = ads::costs(sources.product_ads.as_deref()).await?;
    let period = sources
        .orders
        .sales(tax, during.clone(), &ad_costs)
        .await
        .map_err(|e| orders::failure(&e))?;
    let summary = SalesSummary::of(&period, Currency::Brl)
        .map_err(|e| format!("Vendas em moedas diferentes: {e}"))?;
    let sold_as = SoldAs::read(&sources.listings, &sources.catalog).await?;
    let lines = period
        .sales
        .iter()
        .flat_map(|sale| sold_lines(sale, &sold_as))
        .collect();
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
    let releases = ReleaseSummary::of(&releases, during, Currency::Brl)
        .map_err(|e| format!("Valores do Mercado Pago em moedas diferentes: {e}"))?;
    Ok(Snapshot {
        summary,
        lines,
        capital,
        units: stock.units(),
        releases,
        volume: finance::volume(&sources.volume).await?,
    })
}

/// A listing by the channel's id of the listing and of the variation.
type ListingKey = (String, Option<String>);

/// What a listing sold as: its Product and the name of its category.
type ListedAs = (Option<RecordId>, Option<String>);

/// What each listing sold as.
struct SoldAs {
    listings: BTreeMap<ListingKey, ListedAs>,
    products: BTreeMap<RecordId, String>,
}

impl SoldAs {
    async fn read(listings: &Listings, catalog: &Catalog) -> Result<Self, String> {
        let names = listings
            .category_names()
            .await
            .map_err(|e| listings::failure(&e))?;
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
                    .and_then(|id| names.get(id).cloned());
                let variation = listing.listed.variation.map(|variation| variation.id);
                ((listing.listed.id, variation), (listing.product, category))
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
            let key = (share.item.clone(), share.variation.clone());
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
    span: Span,
    from: Entity<InputState>,
    to: Entity<InputState>,
    /// The custom days last applied.
    custom: Option<(NaiveDate, NaiveDate)>,
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
        // Each Order Sync brings Orders, their Fees and their Money
        // Releases; each Sync of Product Ads, what the ads cost.
        let subscriptions = vec![
            cx.observe_global::<OrderSyncs>(Self::refresh),
            cx.observe_global::<AdsSyncs>(Self::refresh),
        ];
        let today = Local::now().date_naive();
        let from = input("dd/mm/aaaa", window, cx);
        let to = input("dd/mm/aaaa", window, cx);
        let month_ago = day_text(today - TimeDelta::days(29));
        from.update(cx, |input, cx| input.set_value(month_ago, window, cx));
        to.update(cx, |input, cx| input.set_value(day_text(today), window, cx));
        let mut screen = Self {
            span: Span::Days(30),
            from,
            to,
            custom: None,
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

    fn during(&self) -> Option<Range<Timestamp>> {
        let today = Local::now().date_naive();
        match self.span {
            Span::Days(count) => Some(last_days(count, today)),
            Span::Custom => self.custom.map(|(from, to)| days(from, to)),
        }
    }

    /// Reads the period again, as when the screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(sources), Some(during)) = (Sources::of(cx), self.during()) else {
            return;
        };
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

    fn pick(&mut self, span: Span, cx: &mut Context<Self>) {
        self.span = span;
        if span == Span::Custom && self.custom.is_none() {
            self.apply_custom(cx);
        } else {
            self.refresh(cx);
        }
        cx.notify();
    }

    fn apply_custom(&mut self, cx: &mut Context<Self>) {
        let from = parse_day(&self.from.read(cx).value());
        let to = parse_day(&self.to.read(cx).value());
        match (from, to) {
            (Some(from), Some(to)) if from <= to => {
                self.custom = Some((from, to));
                self.refresh(cx);
            }
            _ => {
                self.outcome = Some(Outcome::Failed(
                    "Digite o primeiro e o último dia do período (ex.: 01/10/2026 e \
                     31/10/2026), o primeiro antes do último."
                        .into(),
                ));
            }
        }
        cx.notify();
    }

    fn render_spans(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .gap_2()
            .children(Span::ALL.into_iter().enumerate().map(|(at, span)| {
                let button = Button::new(("span", at))
                    .label(span.name())
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| this.pick(span, cx)));
                if span == self.span {
                    button.primary()
                } else {
                    button.ghost()
                }
            }))
            .into_any_element()
    }

    fn render_custom(&self, cx: &mut Context<Self>) -> AnyElement {
        let field = |label: &'static str, state: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .w(px(150.))
                .child(div().text_sm().font_medium().child(label))
                .child(Input::new(state).small())
        };
        h_flex()
            .flex_wrap()
            .gap_3()
            .items_end()
            .child(field("De", &self.from))
            .child(field("Até", &self.to))
            .child(
                Button::new("apply-days")
                    .label("Aplicar")
                    .outline()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.apply_custom(cx))),
            )
            .into_any_element()
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
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .child(figure("Receita", summary.revenue.to_pt_br(), &t))
                    .child(
                        figure("Margem realizada", margin, &t)
                            .min_w(px(260.))
                            .text_color(margin_ink),
                    )
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
                "Capital em estoque: as unidades de hoje pelo custo médio, qualquer que seja o \
                 período.",
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

fn figure(label: &'static str, text: String, t: &Tokens) -> Div {
    v_flex()
        .flex_1()
        .min_w(px(170.))
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
        parts.actions.push(self.render_spans(cx));
        if let Some(outcome) = &self.outcome {
            parts.notices.push(notice(outcome, cx));
        }
        if self.span == Span::Custom {
            parts.content.push(self.render_custom(cx));
        }
        let Some(snapshot) = self.snapshot.take() else {
            return layout::screen(parts, cx);
        };
        parts.content.push(self.render_figures(&snapshot, cx));
        parts.content.push(self.render_volume(&snapshot.volume, cx));
        parts.content.push(self.render_margins(cx));
        parts.content.push(self.render_releases(&snapshot, cx));
        self.snapshot = Some(snapshot);
        layout::screen(parts, cx)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn the_last_days_end_tomorrow_and_include_today() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let local = |month, day| {
            Local
                .with_ymd_and_hms(2026, month, day, 0, 0, 0)
                .earliest()
                .unwrap()
                .with_timezone(&Utc)
        };

        assert_eq!(last_days(7, today), local(9, 29)..local(10, 6));
    }
}
