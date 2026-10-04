//! Low stock (#14): the Products at or below their Reorder Point on the home
//! screen, and the system notification when a change takes one there.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, EventEmitter, SharedString, SystemNotification, Window, div};
use mascate_catalog::Product;
use mascate_inventory::LowStock;
use mascate_kernel::RecordId;

use crate::appearance::look;
use crate::catalog;
use crate::kit;
use crate::purchases::units_text;
use crate::stock::{self, inventory_failure, product_label};

/// The owner asked to see a Product's stock.
pub struct OpenStock(pub RecordId);

pub struct LowStockArea {
    low: Vec<LowStock>,
    products: Vec<Product>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    error: Option<SharedString>,
}

impl EventEmitter<OpenStock> for LowStockArea {}

impl LowStockArea {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut area = Self {
            low: Vec::new(),
            products: Vec::new(),
            reads: 0,
            error: None,
        };
        area.refresh(cx);
        area
    }

    pub fn is_empty(&self) -> bool {
        self.low.is_empty() && self.error.is_none()
    }

    /// Reads the low stock again, as when the home screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(inventory), Some(catalog)) = (stock::inventory(cx), catalog::catalog(cx)) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        let working = cx.background_executor().spawn(async move {
            let low = inventory
                .low_stock()
                .await
                .map_err(|e| inventory_failure(&e))?;
            let products = catalog.products().await.map_err(|e| catalog::failure(&e))?;
            Ok::<_, String>((low, products))
        });
        cx.spawn(async move |this, cx| {
            let read_back = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                match read_back {
                    Ok((low, products)) => {
                        this.low = low;
                        this.products = products;
                        this.error = None;
                    }
                    Err(error) => {
                        this.error =
                            Some(format!("Não consegui ler o estoque baixo: {error}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_item(&self, index: usize, low: LowStock, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let (sku, name) = product_label(&self.products, low.product);
        h_flex()
            .gap_3()
            .items_center()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(kit::tag(sku, t.accent_text, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .child(div().truncate().font_medium().child(name))
                    .child(
                        div()
                            .text_color(t.text2)
                            .child(situation(low.quantity, low.reorder_point)),
                    ),
            )
            .child(
                Button::new(("open-low-stock", index))
                    .label("Ver estoque")
                    .ghost()
                    .small()
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenStock(low.product)))),
            )
            .into_any_element()
    }
}

/// "2 unidades em estoque · ponto de reposição 5".
fn situation(quantity: i64, reorder_point: u32) -> String {
    let on_hand = match quantity {
        0 => "Sem estoque".to_owned(),
        quantity => format!("{} em estoque", units_text(quantity)),
    };
    format!("{on_hand} · ponto de reposição {reorder_point}")
}

/// Tells the system's notification center that a Product reached its
/// Reorder Point, so the owner hears of it with the window in the tray.
pub fn notify(low: LowStock, products: &[Product], cx: &App) {
    let (sku, name) = product_label(products, low.product);
    cx.show_system_notification(SystemNotification {
        // One per Product: a newer alert replaces the older.
        tag: format!("low-stock-{}", low.product).into(),
        title: format!("Estoque baixo: {name}").into(),
        body: format!(
            "{sku}: {}. Hora de comprar mais.",
            situation(low.quantity, low.reorder_point)
        )
        .into(),
        actions: Vec::new(),
    });
}

impl Render for LowStockArea {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.is_empty() {
            return div().into_any_element();
        }
        let items: Vec<AnyElement> = self
            .low
            .clone()
            .into_iter()
            .enumerate()
            .map(|(index, low)| self.render_item(index, low, cx))
            .collect();
        v_flex()
            .gap_2()
            .child(kit::section_heading("Estoque baixo"))
            .when_some(self.error.clone(), |area, error| {
                area.child(kit::error_notice(error, cx))
            })
            .children(items)
            .into_any_element()
    }
}
