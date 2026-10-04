//! Estoque (#13): each Product's balance per Stock Location, its Average
//! Cost and value, the value of everything in stock, and each Product's
//! Stock Movements. Every number here is read back from the ledger.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Div, Global, SharedString, Window, div, px};
use mascate_catalog::{Product, Supplier};
use mascate_commerce::PurchaseOrder;
use mascate_inventory::{
    HistoryLine, Inventory, InventoryError, MovementReason, ProductStock, Stock, StockLocation,
};
use mascate_kernel::{Money, RecordId};

use crate::appearance::look;
use crate::catalog::{self, NO_DATABASE, day};
use crate::kit;
use crate::layout;
use crate::parts::ScreenParts;
use crate::purchases::{self, units_text};

/// The app's Inventory; absent when the database did not open.
pub struct AppInventory(pub Arc<Inventory>);

impl Global for AppInventory {}

pub fn inventory(cx: &App) -> Option<Arc<Inventory>> {
    cx.try_global::<AppInventory>().map(|app| app.0.clone())
}

/// What the screen shows, as last read.
struct Shown {
    stock: Stock,
    locations: Vec<StockLocation>,
    products: Vec<Product>,
    suppliers: Vec<Supplier>,
    orders: Vec<PurchaseOrder>,
    /// The open Product's movements, newest first.
    history: Option<(RecordId, Vec<HistoryLine>)>,
}

pub struct StockScreen {
    shown: Option<Shown>,
    /// The Product whose movements are open.
    open: Option<RecordId>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    failure: Option<SharedString>,
}

impl StockScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            shown: None,
            open: None,
            reads: 0,
            failure: None,
        };
        screen.refresh(window, cx);
        screen
    }

    fn open(&mut self, product: Option<RecordId>, window: &mut Window, cx: &mut Context<Self>) {
        self.open = product;
        self.refresh(window, cx);
    }

    /// Reads the ledger again, as when the screen comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(inventory), Some(catalog), Some(orders)) = (
            inventory(cx),
            catalog::catalog(cx),
            purchases::purchase_orders(cx),
        ) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        let open = self.open;
        let working = cx.background_executor().spawn(async move {
            let stock_failure = |error: InventoryError| inventory_failure(&error);
            Ok::<_, String>(Shown {
                stock: inventory.stock().await.map_err(stock_failure)?,
                locations: inventory.locations().await.map_err(stock_failure)?,
                products: catalog.products().await.map_err(|e| catalog::failure(&e))?,
                suppliers: catalog
                    .suppliers()
                    .await
                    .map_err(|e| catalog::failure(&e))?,
                orders: orders
                    .purchase_orders()
                    .await
                    .map_err(|e| purchases::failure(&e))?,
                history: match open {
                    Some(product) => Some((
                        product,
                        inventory.history(product).await.map_err(stock_failure)?,
                    )),
                    None => None,
                },
            })
        });
        cx.spawn_in(window, async move |this, cx| {
            let shown = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                match shown {
                    Ok(shown) => {
                        this.shown = Some(shown);
                        this.failure = None;
                    }
                    Err(error) => this.failure = Some(error.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_summary(&self, shown: &Shown, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let value = match shown.stock.value().as_slice() {
            [] => "R$ 0,00".to_owned(),
            values => values
                .iter()
                .map(|value| value.to_pt_br())
                .collect::<Vec<_>>()
                .join(" + "),
        };
        let with_stock = shown
            .stock
            .products
            .iter()
            .filter(|stock| stock.valuation.quantity() > 0)
            .count();
        let figure = |label: &'static str, text: String| {
            v_flex()
                .flex_1()
                .min_w(px(180.))
                .gap_1()
                .p_4()
                .rounded(t.radius_lg)
                .border(t.border_width)
                .border_color(t.frame)
                .bg(t.surface)
                .child(div().text_sm().text_color(t.text2).child(label))
                .child(div().text_xl().font_semibold().child(text))
        };
        h_flex()
            .flex_wrap()
            .gap_3()
            .child(figure("Valor em estoque, a custo médio", value))
            .child(figure(
                "Unidades em estoque",
                shown.stock.units().to_string(),
            ))
            .child(figure("Produtos com estoque", with_stock.to_string()))
            .into_any_element()
    }

    fn render_table(&self, shown: &Shown, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        if shown.stock.products.is_empty() {
            return div()
                .text_sm()
                .text_color(t.text2)
                .child(
                    "Nada em estoque ainda. Registre uma ordem de compra em Compras e use \
                     Receber quando ela chegar.",
                )
                .into_any_element();
        }
        let mut rows: Vec<(String, String, &ProductStock)> = shown
            .stock
            .products
            .iter()
            .map(|stock| {
                let (sku, name) = product_label(&shown.products, stock.product);
                (sku, name, stock)
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        let header = h_flex()
            .gap_3()
            .px_3()
            .text_xs()
            .text_color(t.text2)
            .child(div().flex_1().child("Produto"))
            .children(
                shown
                    .locations
                    .iter()
                    .map(|location| number_cell(location.name.clone())),
            )
            .child(number_cell("Total"))
            .child(money_cell("Custo médio"))
            .child(money_cell("Valor"))
            // Over the rows' chevron.
            .child(div().w(px(16.)).flex_none());
        v_flex()
            .gap_2()
            .child(header)
            .children(
                rows.into_iter()
                    .enumerate()
                    .map(|(index, (sku, name, stock))| {
                        let product = stock.product;
                        let valuation = stock.valuation;
                        h_flex()
                            .id(("stock-row", index))
                            .gap_3()
                            .p_3()
                            .items_center()
                            .rounded(t.radius_lg)
                            .border(t.border_width)
                            .border_color(t.frame)
                            .bg(t.surface)
                            .cursor_pointer()
                            .hover(|row| row.border_color(t.accent_edge))
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_2()
                                    .items_center()
                                    .child(kit::tag(sku, t.accent_text, cx))
                                    .child(div().min_w_0().truncate().child(name)),
                            )
                            .children(shown.locations.iter().map(|location| {
                                let quantity = stock
                                    .by_location
                                    .iter()
                                    .find(|balance| balance.location == location.id)
                                    .map_or(0, |balance| balance.quantity);
                                number_cell(quantity.to_string())
                            }))
                            .child(number_cell(valuation.quantity().to_string()).font_semibold())
                            .child(money_cell(cost_text(valuation.average_cost())))
                            .child(money_cell(valuation.value().to_pt_br()))
                            .child(
                                Icon::new(IconName::ChevronRight)
                                    .size(px(16.))
                                    .text_color(t.text2),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open(Some(product), window, cx)
                            }))
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    fn render_history(&self, shown: &Shown, history: &[HistoryLine], cx: &App) -> Vec<AnyElement> {
        let t = look(cx).tokens;
        let now = history.first().map(|line| line.after);
        let summary = div().text_sm().text_color(t.text2).child(match now {
            Some(now) => format!(
                "Saldo {} · custo médio {} · valor {}",
                units_text(now.quantity()),
                cost_text(now.average_cost()),
                now.value().to_pt_br()
            ),
            None => "Este produto ainda não teve movimentos de estoque.".to_owned(),
        });
        let header = h_flex()
            .gap_3()
            .px_3()
            .text_xs()
            .text_color(t.text2)
            .child(div().w(px(90.)).child("Data"))
            .child(div().flex_1().child("Movimento"))
            .child(number_cell("Quantidade"))
            .child(money_cell("Custo"))
            .child(number_cell("Saldo"))
            .child(money_cell("Custo médio"));
        let rows = history.iter().map(|line| {
            let movement = &line.movement;
            let location = shown
                .locations
                .iter()
                .find(|location| location.id == movement.location)
                .map_or("Local removido", |location| location.name.as_str());
            let (what, source) = reason_text(shown, movement.reason);
            let detail = match source {
                Some(source) => format!("{source} · {location}"),
                None => location.to_owned(),
            };
            h_flex()
                .gap_3()
                .p_3()
                .items_center()
                .rounded(t.radius_lg)
                .border(t.border_width)
                .border_color(t.frame)
                .bg(t.surface)
                .text_sm()
                .child(div().w(px(90.)).child(day(movement.at)))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_medium().child(what))
                        .child(div().text_xs().text_color(t.text2).child(detail)),
                )
                .child(number_cell(format!("{:+}", movement.quantity)).text_color(
                    if movement.quantity >= 0 {
                        t.success
                    } else {
                        t.danger
                    },
                ))
                .child(money_cell(movement.cost.to_pt_br()))
                .child(number_cell(line.after.quantity().to_string()))
                .child(money_cell(cost_text(line.after.average_cost())))
                .into_any_element()
        });
        let mut elements = vec![summary.into_any_element()];
        if !history.is_empty() {
            elements.push(
                v_flex()
                    .gap_2()
                    .child(header)
                    .children(rows)
                    .into_any_element(),
            );
        }
        elements
    }
}

fn number_cell(text: impl Into<SharedString>) -> Div {
    div().w(px(80.)).flex_none().text_right().child(text.into())
}

fn money_cell(text: impl Into<SharedString>) -> Div {
    div()
        .w(px(120.))
        .flex_none()
        .text_right()
        .child(text.into())
}

fn cost_text(cost: Option<Money>) -> String {
    cost.map_or_else(|| "—".to_owned(), Money::to_pt_br)
}

fn product_label(products: &[Product], id: RecordId) -> (String, String) {
    products
        .iter()
        .find(|product| product.id == id)
        .map_or_else(
            || ("?".into(), "Produto removido".into()),
            |product| (product.sku.to_string(), product.name.clone()),
        )
}

/// What a movement was and where it came from: "Entrada da compra de
/// 01/10/2026" and "Loja Fones".
fn reason_text(shown: &Shown, reason: MovementReason) -> (String, Option<String>) {
    match reason {
        MovementReason::PurchaseReceipt { purchase_order } => {
            match shown.orders.iter().find(|order| order.id == purchase_order) {
                Some(order) => {
                    let supplier = shown
                        .suppliers
                        .iter()
                        .find(|supplier| supplier.id == order.supplier)
                        .map_or("Fornecedor removido", |supplier| supplier.name.as_str());
                    (
                        format!(
                            "Entrada da compra de {}",
                            order.ordered_on.format("%d/%m/%Y")
                        ),
                        Some(supplier.to_owned()),
                    )
                }
                None => ("Entrada de uma compra".to_owned(), None),
            }
        }
    }
}

/// Why reading or writing the stock failed, as the owner reads it.
pub fn inventory_failure(error: &InventoryError) -> String {
    match error {
        InventoryError::NoUnits => "Uma entrada precisa de pelo menos 1 unidade.".into(),
        InventoryError::NegativeCost => "Um custo não pode ser negativo.".into(),
        InventoryError::UnknownLocation(_) => {
            "Esse local de estoque não existe mais; a lista foi atualizada.".into()
        }
        InventoryError::Currencies(_) => {
            "O estoque deste produto está em outra moeda; registre a compra na mesma moeda das \
             anteriores."
                .into()
        }
        InventoryError::Unreadable(_) | InventoryError::Sql(_) => {
            format!("Não consegui ler ou gravar o estoque: {error}")
        }
    }
}

impl Render for StockScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self
            .open
            .zip(self.shown.as_ref())
            .map(|(id, shown)| product_label(&shown.products, id));
        let mut parts = match &open {
            Some((sku, name)) => ScreenParts::new(format!("{name} · {sku}")),
            None => ScreenParts::new("Estoque"),
        };
        if inventory(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        parts.notices.extend(
            self.failure
                .clone()
                .map(|failure| kit::error_notice(failure, cx).into_any_element()),
        );
        if self.open.is_some() {
            parts.actions.push(
                Button::new("back-to-stock")
                    .label("Todo o estoque")
                    .icon(IconName::ArrowLeft)
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| this.open(None, window, cx)))
                    .into_any_element(),
            );
        }
        let Some(shown) = &self.shown else {
            return layout::screen(parts, cx);
        };
        match (self.open, &shown.history) {
            (Some(open), Some((product, history))) if open == *product => {
                parts
                    .content
                    .extend(self.render_history(shown, history, cx));
            }
            (Some(_), _) => {}
            (None, _) => {
                parts.content.push(self.render_summary(shown, cx));
                parts.content.push(self.render_table(shown, cx));
            }
        }
        layout::screen(parts, cx)
    }
}
