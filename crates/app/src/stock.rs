//! Estoque (#13, #14): each Product's balance per Stock Location, its
//! Average Cost and value, the value of everything in stock, and each
//! Product's Stock Movements, Stock Adjustments and Reorder Point. Every
//! number here is read back from the ledger.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Div, Entity, Global, SharedString, Window, div, px};
use mascate_catalog::{Product, Supplier};
use mascate_commerce::{Order, PurchaseOrder};
use mascate_inventory::{
    Adjusted, AdjustmentKind, HOME_LOCATION, HistoryLine, Inventory, InventoryError,
    MovementReason, ProductStock, Stock, StockAdjustment, StockLocation, StockMovement,
};
use mascate_kernel::{Money, RecordId};

use crate::appearance::look;
use crate::catalog::{self, NO_DATABASE, day};
use crate::forms::{Outcome, input, notice};
use crate::kit;
use crate::layout;
use crate::low_stock;
use crate::parts::ScreenParts;
use crate::purchases::{self, units_text};
use crate::stock_mirror;

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
    /// The Orders whose sales the open Product's history names.
    sales: Vec<Order>,
    /// The open Product's movements, newest first.
    history: Option<(RecordId, Vec<HistoryLine>)>,
}

const ADJUSTMENT_KINDS: [AdjustmentKind; 3] = [
    AdjustmentKind::Loss,
    AdjustmentKind::Damage,
    AdjustmentKind::Count,
];

pub struct StockScreen {
    shown: Option<Shown>,
    /// The Product whose movements are open.
    open: Option<RecordId>,
    /// The adjustment's reason; the owner always picks one.
    kind: Option<AdjustmentKind>,
    units: Entity<InputState>,
    note: Entity<InputState>,
    reorder_point: Entity<InputState>,
    /// The Product whose Reorder Point the field holds.
    filled: Option<RecordId>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    failure: Option<SharedString>,
    outcome: Option<Outcome>,
}

impl StockScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            shown: None,
            open: None,
            kind: None,
            units: input("Quantidade", window, cx),
            note: input("Observação (opcional)", window, cx),
            reorder_point: input("Sem alerta", window, cx),
            filled: None,
            busy: false,
            reads: 0,
            failure: None,
            outcome: None,
        };
        screen.refresh(window, cx);
        screen
    }

    /// Opens a Product's movements, or the whole stock with `None`.
    pub fn open(&mut self, product: Option<RecordId>, window: &mut Window, cx: &mut Context<Self>) {
        if self.open != product {
            self.kind = None;
            self.outcome = None;
            self.filled = None;
            for field in [&self.units, &self.note] {
                field.update(cx, |field, cx| field.set_value("", window, cx));
            }
        }
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
        let sales = crate::orders::orders(cx);
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
                sales: match (&sales, open) {
                    (Some(sales), Some(_)) => sales
                        .orders()
                        .await
                        .map_err(|e| crate::orders::failure(&e))?,
                    _ => Vec::new(),
                },
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
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads != read {
                    return;
                }
                match shown {
                    Ok(shown) => {
                        this.fill_reorder_point(&shown, window, cx);
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

    /// Puts the open Product's Reorder Point in its field once per opening,
    /// so a read never overwrites what the owner is typing.
    fn fill_reorder_point(&mut self, shown: &Shown, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.filter(|open| self.filled != Some(*open)) else {
            return;
        };
        let point = shown
            .stock
            .products
            .iter()
            .find(|stock| stock.product == open)
            .and_then(|stock| stock.reorder_point)
            .map(|point| point.to_string())
            .unwrap_or_default();
        self.reorder_point
            .update(cx, |field, cx| field.set_value(point, window, cx));
        self.filled = Some(open);
    }

    /// Runs `change` off the UI thread, hands what it returned to `then`
    /// and reads the stock again.
    fn change<T, F, Fut>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: F,
        then: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) where
        T: Send + 'static,
        F: FnOnce(Arc<Inventory>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, InventoryError>> + Send,
    {
        let Some(inventory) = inventory(cx) else {
            return;
        };
        self.busy = true;
        self.outcome = None;
        let working = cx
            .background_executor()
            .spawn(async move { change(inventory).await.map_err(|e| inventory_failure(&e)) });
        cx.spawn_in(window, async move |this, cx| {
            let changed = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match changed {
                    Ok(value) => then(this, value, window, cx),
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                this.refresh(window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn failed(&mut self, text: &'static str, cx: &mut Context<Self>) {
        self.outcome = Some(Outcome::Failed(text.into()));
        cx.notify();
    }

    fn adjust(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(product) = self.open else {
            return;
        };
        let Some(kind) = self.kind else {
            return self.failed("Escolha o motivo do ajuste: perda, avaria ou contagem.", cx);
        };
        let units = self.units.read(cx).value().trim().parse::<u32>().ok();
        let adjustment = match (kind, units) {
            (AdjustmentKind::Loss, Some(units @ 1..)) => StockAdjustment::Loss(units),
            (AdjustmentKind::Damage, Some(units @ 1..)) => StockAdjustment::Damage(units),
            (AdjustmentKind::Count, Some(counted)) => StockAdjustment::Count(counted),
            (AdjustmentKind::Count, None) => {
                return self.failed(
                    "Digite quantas unidades você contou (0 se não sobrou nenhuma).",
                    cx,
                );
            }
            _ => return self.failed("Digite quantas unidades saíram (1 ou mais).", cx),
        };
        let note = self.note.read(cx).value().to_string();
        self.change(
            window,
            cx,
            move |inventory| async move {
                inventory
                    .adjust(product, HOME_LOCATION, adjustment, Some(&note))
                    .await
            },
            |this, adjusted: Adjusted, window, cx| {
                this.kind = None;
                for field in [&this.units, &this.note] {
                    field.update(cx, |field, cx| field.set_value("", window, cx));
                }
                this.outcome = Some(Outcome::Done(adjusted_text(&adjusted).into()));
                if adjusted.movement.is_some() {
                    stock_mirror::send_after_movement(cx);
                }
                if let (Some(low), Some(shown)) = (adjusted.reached_reorder_point, &this.shown) {
                    low_stock::notify(low, &shown.products, cx);
                }
            },
        );
    }

    fn save_reorder_point(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(product) = self.open else {
            return;
        };
        let text = self.reorder_point.read(cx).value().trim().to_owned();
        let point = match text.parse::<u32>() {
            _ if text.is_empty() => None,
            Ok(point) => Some(point),
            Err(_) => {
                return self.failed(
                    "Digite o ponto de reposição como um número inteiro, ou deixe vazio para \
                     não ter alerta.",
                    cx,
                );
            }
        };
        self.change(
            window,
            cx,
            move |inventory| async move { inventory.set_reorder_point(product, point).await },
            move |this, (), _, _| {
                this.outcome = Some(Outcome::Done(
                    match point {
                        Some(point) => format!(
                            "Ponto de reposição salvo: com {} ou menos, o produto aparece em \
                             Hoje.",
                            units_text(i64::from(point))
                        ),
                        None => {
                            "Ponto de reposição removido; este produto não gera alerta.".to_owned()
                        }
                    }
                    .into(),
                ));
            },
        );
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let card = || {
            v_flex()
                .flex_1()
                .min_w(px(300.))
                .gap_3()
                .p_4()
                .rounded(t.radius_lg)
                .border(t.border_width)
                .border_color(t.frame)
                .bg(t.surface)
        };
        let field = |label: &'static str, input: AnyElement| {
            v_flex()
                .gap_1()
                .child(div().text_sm().font_medium().child(label))
                .child(input)
        };
        let kinds = ADJUSTMENT_KINDS.into_iter().map(|kind| {
            Button::new(("adjustment-kind", kind as usize))
                .label(kind_name(kind))
                .small()
                .map(|button| {
                    if self.kind == Some(kind) {
                        button.primary()
                    } else {
                        button.outline()
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.kind = Some(kind);
                    cx.notify();
                }))
        });
        let (units_label, hint) = match self.kind {
            Some(AdjustmentKind::Loss) => (
                "Unidades perdidas",
                "As unidades saem pelo custo médio, que não muda.",
            ),
            Some(AdjustmentKind::Damage) => (
                "Unidades com avaria",
                "As unidades saem pelo custo médio, que não muda.",
            ),
            Some(AdjustmentKind::Count) => (
                "Unidades contadas",
                "Digite o saldo físico; o app registra a diferença.",
            ),
            None => ("Unidades", "Escolha o motivo do ajuste."),
        };
        let adjust = card()
            .child(kit::section_heading("Ajustar estoque"))
            .child(h_flex().gap_2().flex_wrap().children(kinds))
            .child(
                h_flex()
                    .gap_3()
                    .items_end()
                    .child(field(
                        units_label,
                        div()
                            .w(px(140.))
                            .child(Input::new(&self.units).small())
                            .into_any_element(),
                    ))
                    .child(div().flex_1().child(field(
                        "Observação",
                        Input::new(&self.note).small().into_any_element(),
                    ))),
            )
            .child(div().text_sm().text_color(t.text2).child(hint))
            .child(
                h_flex().child(
                    Button::new("adjust-stock")
                        .label("Registrar ajuste")
                        .primary()
                        .small()
                        .loading(self.busy)
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.adjust(window, cx))),
                ),
            );
        let reorder = card()
            .child(kit::section_heading("Ponto de reposição"))
            .child(div().text_sm().text_color(t.text2).child(
                "Com o saldo neste número ou abaixo, o produto aparece em Hoje e o app avisa \
                 quando um ajuste o leva até aqui. Vazio, não há alerta.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(140.))
                            .child(Input::new(&self.reorder_point).small()),
                    )
                    .child(
                        Button::new("save-reorder-point")
                            .label("Salvar ponto")
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_reorder_point(window, cx)
                            })),
                    ),
            );
        h_flex()
            .flex_wrap()
            .gap_3()
            .items_start()
            .child(adjust)
            .child(reorder)
            .into_any_element()
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
                                    .child(div().min_w_0().truncate().child(name))
                                    .when(stock.is_low(), |cell| {
                                        cell.child(kit::tag("estoque baixo", t.danger, cx))
                                    }),
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
            let (what, source) = reason_text(shown, movement);
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

pub fn product_label(products: &[Product], id: RecordId) -> (String, String) {
    products
        .iter()
        .find(|product| product.id == id)
        .map_or_else(
            || ("?".into(), "Produto removido".into()),
            |product| (product.sku.to_string(), product.name.clone()),
        )
}

/// What a movement was and where it came from: "Entrada da compra de
/// 01/10/2026" and "Loja Fones", or "Avaria" and the owner's note.
fn reason_text(shown: &Shown, movement: &StockMovement) -> (String, Option<String>) {
    match movement.reason {
        MovementReason::Adjustment(kind) => (
            format!("Ajuste: {}", kind_name(kind)),
            movement.note.clone(),
        ),
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
        MovementReason::Sale { order } => (
            "Venda no Mercado Livre".to_owned(),
            shown
                .sales
                .iter()
                .find(|sale| sale.id == order)
                .map(|sale| format!("Pedido {}", sale.sold.id)),
        ),
    }
}

fn kind_name(kind: AdjustmentKind) -> &'static str {
    match kind {
        AdjustmentKind::Loss => "Perda",
        AdjustmentKind::Damage => "Avaria",
        AdjustmentKind::Count => "Contagem",
    }
}

/// What an adjustment did, as the owner reads it.
fn adjusted_text(adjusted: &Adjusted) -> String {
    let Some(movement) = &adjusted.movement else {
        return "A contagem confere com o saldo; nada a ajustar.".into();
    };
    let units = units_text(movement.quantity.abs());
    let mut text = match movement.reason {
        MovementReason::Adjustment(AdjustmentKind::Loss) => {
            format!("{units} saíram do estoque como perda.")
        }
        MovementReason::Adjustment(AdjustmentKind::Damage) => {
            format!("{units} saíram do estoque como avaria.")
        }
        MovementReason::Adjustment(AdjustmentKind::Count) if movement.quantity > 0 => {
            format!("A contagem achou {units} a mais; o saldo foi corrigido.")
        }
        MovementReason::Adjustment(AdjustmentKind::Count) => {
            format!("A contagem achou {units} a menos; o saldo foi corrigido.")
        }
        MovementReason::PurchaseReceipt { .. } | MovementReason::Sale { .. } => {
            "Estoque ajustado.".into()
        }
    };
    if adjusted.reached_reorder_point.is_some() {
        text.push_str(" O produto chegou ao ponto de reposição.");
    }
    text
}

/// Why reading or writing the stock failed, as the owner reads it.
pub fn inventory_failure(error: &InventoryError) -> String {
    match error {
        InventoryError::NoUnits => {
            "Uma entrada ou um ajuste precisa de pelo menos 1 unidade.".into()
        }
        InventoryError::NotEnoughStock { on_hand } => format!(
            "Só há {} deste produto aqui. Se o físico for outro, use Contagem.",
            units_text(*on_hand)
        ),
        InventoryError::NoCostBasis => {
            "Este produto nunca entrou no estoque, então o app não sabe quanto custam as \
             unidades achadas. Registre a compra em Compras e receba-a."
                .into()
        }
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
        parts
            .notices
            .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
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
                let mut history = self.render_history(shown, history, cx).into_iter();
                parts.content.extend(history.next());
                parts.content.push(self.render_controls(cx));
                parts.content.extend(history);
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
