//! Compras (#13): Purchase Orders to Suppliers with their items, freight
//! and status, and receiving them into stock, whole or in part.

use std::sync::Arc;

use chrono::{Datelike as _, Local, NaiveDate};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::select::Select;
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, Global, Hsla, Window, div, px};
use mascate_catalog::{Product, Supplier};
use mascate_commerce::{
    NewPurchaseLine, NewPurchaseOrder, PurchaseOrder, PurchaseOrderError, PurchaseOrderStatus,
    PurchaseOrders, Receiving,
};
use mascate_inventory::HOME_LOCATION;
use mascate_kernel::{Currency, Money, RecordId, parse_amount};
use rust_decimal::Decimal;

use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE, day, product_choice};
use crate::forms::{Choice, Outcome, Picker, input, notice, picker, refill};
use crate::kit;
use crate::layout;
use crate::parts::ScreenParts;
use crate::stock::inventory_failure;

/// The app's Purchase Orders; absent when the database did not open.
pub struct AppPurchaseOrders(pub Arc<PurchaseOrders>);

impl Global for AppPurchaseOrders {}

pub fn purchase_orders(cx: &App) -> Option<Arc<PurchaseOrders>> {
    cx.try_global::<AppPurchaseOrders>()
        .map(|app| app.0.clone())
}

/// An item of the order being written. The price takes the order's
/// currency when it is saved.
#[derive(Clone)]
struct DraftLine {
    product: RecordId,
    quantity: u32,
    unit_price: Decimal,
}

/// What the open order's row is asking for.
enum Step {
    /// How many units of each line came in.
    Receive(Vec<(RecordId, Entity<InputState>)>),
    ConfirmCancel,
    ConfirmDelete,
}

pub struct PurchasesScreen {
    supplier: Picker,
    ordered_on: Entity<InputState>,
    freight: Entity<InputState>,
    currency: Currency,
    product: Picker,
    quantity: Entity<InputState>,
    unit_price: Entity<InputState>,
    draft: Vec<DraftLine>,
    /// The order the form is editing, if any.
    editing: Option<RecordId>,
    orders: Vec<PurchaseOrder>,
    suppliers: Vec<Supplier>,
    products: Vec<Product>,
    step: Option<(RecordId, Step)>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    form: Option<Outcome>,
    outcome: Option<Outcome>,
}

impl PurchasesScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ordered_on = input("dd/mm/aaaa", window, cx);
        ordered_on.update(cx, |field, cx| field.set_value(today(), window, cx));
        let mut screen = Self {
            supplier: picker(window, cx),
            ordered_on,
            freight: input("0,00", window, cx),
            currency: Currency::Brl,
            product: picker(window, cx),
            quantity: input("1", window, cx),
            unit_price: input("29,90", window, cx),
            draft: Vec::new(),
            editing: None,
            orders: Vec::new(),
            suppliers: Vec::new(),
            products: Vec::new(),
            step: None,
            busy: false,
            reads: 0,
            form: None,
            outcome: None,
        };
        screen.refresh(window, cx);
        screen
    }

    /// Reads the orders and the catalog again, as when the screen comes into
    /// view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, |_| async { Ok(None) }, |_, _: (), _, _| {});
    }

    /// Runs `change` off the UI thread, then reads everything again and
    /// hands what the change returned to `then`.
    fn run<T, F, Fut>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: F,
        then: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) where
        T: Send + 'static,
        F: FnOnce(Arc<PurchaseOrders>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<T>, PurchaseOrderError>> + Send,
    {
        let (Some(orders), Some(catalog)) = (purchase_orders(cx), catalog::catalog(cx)) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(orders.clone()).await.map_err(|e| failure(&e));
            let read = async {
                let suppliers = catalog
                    .suppliers()
                    .await
                    .map_err(|e| catalog::failure(&e))?;
                let products = catalog.products().await.map_err(|e| catalog::failure(&e))?;
                let orders = orders.purchase_orders().await.map_err(|e| failure(&e))?;
                Ok::<_, String>((suppliers, products, orders))
            }
            .await;
            (changed, read)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, read_back) = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads == read {
                    this.busy = false;
                    match read_back {
                        Ok((suppliers, products, orders)) => {
                            this.show(suppliers, products, orders, window, cx)
                        }
                        Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                    }
                }
                match changed {
                    Ok(Some(value)) => then(this, value, window, cx),
                    Ok(None) => {}
                    Err(error) => this.failed(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Shows a failure where the owner acted: the form, or the open row.
    fn failed(&mut self, error: String) {
        let text = Outcome::Failed(error.into());
        if self.step.is_some() {
            self.outcome = Some(text);
        } else {
            self.form = Some(text);
        }
    }

    fn show(
        &mut self,
        suppliers: Vec<Supplier>,
        products: Vec<Product>,
        orders: Vec<PurchaseOrder>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let supplier_choices = suppliers
            .iter()
            .map(|supplier| Choice {
                id: supplier.id,
                title: supplier.name.clone().into(),
            })
            .collect();
        refill(&self.supplier, supplier_choices, window, cx);
        refill(
            &self.product,
            products.iter().map(product_choice).collect(),
            window,
            cx,
        );
        if self
            .step
            .as_ref()
            .is_some_and(|(open, _)| !orders.iter().any(|order| order.id == *open))
        {
            self.step = None;
        }
        self.suppliers = suppliers;
        self.products = products;
        self.orders = orders;
    }

    fn supplier_name(&self, id: RecordId) -> String {
        self.suppliers
            .iter()
            .find(|supplier| supplier.id == id)
            .map_or_else(|| "Fornecedor removido".into(), |s| s.name.clone())
    }

    fn product_label(&self, id: RecordId) -> (String, String) {
        self.products
            .iter()
            .find(|product| product.id == id)
            .map_or_else(
                || ("?".into(), "Produto removido".into()),
                |product| (product.sku.to_string(), product.name.clone()),
            )
    }

    fn add_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form = None;
        let Some(&product) = self.product.read(cx).selected_value() else {
            self.form = Some(Outcome::Failed("Escolha o produto do item.".into()));
            cx.notify();
            return;
        };
        let quantity = self.quantity.read(cx).value().trim().parse::<u32>().ok();
        let unit_price = parse_amount(&self.unit_price.read(cx).value());
        let (Some(quantity @ 1..), Some(unit_price)) = (quantity, unit_price) else {
            self.form = Some(Outcome::Failed(
                "Digite a quantidade (1 ou mais) e o preço de cada unidade, como 29,90.".into(),
            ));
            cx.notify();
            return;
        };
        self.draft.push(DraftLine {
            product,
            quantity,
            unit_price,
        });
        for field in [&self.quantity, &self.unit_price] {
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
        self.product
            .update(cx, |select, cx| select.set_selected_index(None, window, cx));
        cx.notify();
    }

    /// The order the form describes, or why it cannot be one yet.
    fn written_order(&self, cx: &App) -> Result<NewPurchaseOrder, String> {
        let supplier = *self
            .supplier
            .read(cx)
            .selected_value()
            .ok_or("Escolha o fornecedor; cadastre fornecedores novos em Ofertas.")?;
        let ordered_on = parse_day(&self.ordered_on.read(cx).value())
            .ok_or("Digite a data da compra como 01/10/2026.")?;
        let freight = self.freight.read(cx).value();
        let freight = if freight.trim().is_empty() {
            Some(Decimal::ZERO)
        } else {
            parse_amount(&freight)
        }
        .ok_or("Digite o frete como 12,50 (vazio conta como zero).")?;
        if self.draft.is_empty() {
            return Err("Adicione pelo menos um item à ordem de compra.".into());
        }
        Ok(NewPurchaseOrder {
            supplier,
            ordered_on,
            freight: Money::new(freight, self.currency),
            lines: self
                .draft
                .iter()
                .map(|line| NewPurchaseLine {
                    product: line.product,
                    quantity: line.quantity,
                    unit_price: Money::new(line.unit_price, self.currency),
                })
                .collect(),
        })
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step = None;
        let order = match self.written_order(cx) {
            Ok(order) => order,
            Err(why) => {
                self.form = Some(Outcome::Failed(why.into()));
                cx.notify();
                return;
            }
        };
        self.form = None;
        let editing = self.editing;
        self.run(
            window,
            cx,
            move |orders| async move {
                match editing {
                    Some(id) => orders.update(id, order).await.map(Some),
                    None => orders.create(order).await.map(Some),
                }
            },
            move |this, _: PurchaseOrder, window, cx| {
                this.clear_form(window, cx);
                this.form = Some(Outcome::Done(
                    match editing {
                        Some(_) => "Ordem de compra atualizada.",
                        None => "Ordem de compra registrada como comprada.",
                    }
                    .into(),
                ));
            },
        );
    }

    fn clear_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.draft.clear();
        self.ordered_on
            .update(cx, |field, cx| field.set_value(today(), window, cx));
        for field in [&self.freight, &self.quantity, &self.unit_price] {
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
        self.product
            .update(cx, |select, cx| select.set_selected_index(None, window, cx));
    }

    /// Loads an order into the form to change it.
    fn edit(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(order) = self.orders.iter().find(|order| order.id == id).cloned() else {
            return;
        };
        self.step = None;
        self.outcome = None;
        self.form = None;
        self.editing = Some(id);
        self.currency = order.currency();
        self.draft = order
            .lines
            .iter()
            .map(|line| DraftLine {
                product: line.product,
                quantity: line.quantity,
                unit_price: line.unit_price.amount(),
            })
            .collect();
        self.supplier.update(cx, |select, cx| {
            select.set_selected_value(&order.supplier, window, cx)
        });
        let ordered_on = order.ordered_on.format("%d/%m/%Y").to_string();
        let freight = typed(order.freight);
        self.ordered_on
            .update(cx, |field, cx| field.set_value(ordered_on, window, cx));
        self.freight
            .update(cx, |field, cx| field.set_value(freight, window, cx));
        cx.notify();
    }

    fn mark_shipped(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.outcome = None;
        self.run(
            window,
            cx,
            move |orders| async move { orders.mark_shipped(id).await.map(Some) },
            |this, _: PurchaseOrder, _, _| {
                this.outcome = Some(Outcome::Done(
                    "Ordem de compra marcada como enviada.".into(),
                ))
            },
        );
    }

    fn open_step(&mut self, id: RecordId, step: Step, cx: &mut Context<Self>) {
        self.outcome = None;
        self.form = None;
        self.step = Some((id, step));
        cx.notify();
    }

    fn start_receiving(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(order) = self.orders.iter().find(|order| order.id == id) else {
            return;
        };
        let fields = order
            .lines
            .iter()
            .filter(|line| line.remaining() > 0)
            .map(|line| {
                let remaining = line.remaining().to_string();
                let field = input("0", window, cx);
                field.update(cx, |field, cx| field.set_value(remaining, window, cx));
                (line.id, field)
            })
            .collect();
        self.open_step(id, Step::Receive(fields), cx);
    }

    fn receive(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, Step::Receive(fields))) = &self.step else {
            return;
        };
        let mut receiving = Vec::new();
        for (line, field) in fields {
            let text = field.read(cx).value();
            let quantity = if text.trim().is_empty() {
                Some(0)
            } else {
                text.trim().parse::<u32>().ok()
            };
            let Some(quantity) = quantity else {
                self.outcome = Some(Outcome::Failed(
                    "Digite quantas unidades chegaram de cada item (0 para as que não vieram)."
                        .into(),
                ));
                cx.notify();
                return;
            };
            receiving.push(Receiving {
                line: *line,
                quantity,
            });
        }
        self.outcome = None;
        self.run(
            window,
            cx,
            move |orders| async move {
                orders
                    .receive(id, HOME_LOCATION, &receiving)
                    .await
                    .map(Some)
            },
            |this, order: PurchaseOrder, _, _| {
                this.step = None;
                let units: u32 = order
                    .receipts
                    .last()
                    .map_or(0, |receipt| receipt.lines.iter().map(|l| l.quantity).sum());
                let rest = match order.status() {
                    PurchaseOrderStatus::Received => "A ordem de compra foi toda recebida.",
                    _ => "O restante continua a caminho.",
                };
                this.outcome = Some(Outcome::Done(
                    format!("{} entraram no estoque. {rest}", units_text(units)).into(),
                ));
            },
        );
    }

    fn cancel(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.run(
            window,
            cx,
            move |orders| async move { orders.cancel(id).await.map(Some) },
            |this, _: PurchaseOrder, _, _| {
                this.step = None;
                this.outcome = Some(Outcome::Done("Ordem de compra cancelada.".into()));
            },
        );
    }

    fn delete(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.run(
            window,
            cx,
            move |orders| async move { orders.delete(id).await.map(Some) },
            move |this, (), window, cx| {
                this.step = None;
                if this.editing == Some(id) {
                    this.clear_form(window, cx);
                }
                this.outcome = Some(Outcome::Done("Ordem de compra excluída.".into()));
            },
        );
    }

    fn render_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, input: AnyElement| {
            v_flex()
                .gap_1()
                .child(div().text_sm().font_medium().child(label))
                .child(input)
        };
        let currency = |value: Currency| {
            let on = self.currency == value;
            Button::new(("purchase-currency", value as usize))
                .label(value.symbol())
                .small()
                .map(|button| {
                    if on {
                        button.primary()
                    } else {
                        button.outline()
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.currency = value;
                    cx.notify();
                }))
        };
        let money = |amount: Decimal| Money::new(amount, self.currency);
        let items_value = self
            .draft
            .iter()
            .map(|line| line.unit_price * Decimal::from(line.quantity))
            .sum::<Decimal>();

        let lines = self
            .draft
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let (sku, name) = self.product_label(line.product);
                h_flex()
                    .gap_3()
                    .items_center()
                    .py_1()
                    .border_b(t.border_width)
                    .border_color(t.border)
                    .child(kit::tag(sku, t.accent_text, cx))
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(name))
                    .child(div().text_sm().child(format!(
                        "{} × {} = {}",
                        line.quantity,
                        money(line.unit_price).to_pt_br(),
                        money(line.unit_price * Decimal::from(line.quantity)).to_pt_br()
                    )))
                    .child(
                        Button::new(("remove-line", index))
                            .label("Remover")
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.draft.remove(index);
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let freight = parse_amount(&self.freight.read(cx).value()).unwrap_or_default();
        let summary = (!self.draft.is_empty()).then(|| {
            div().text_sm().text_color(t.text2).child(format!(
                "Total {} = itens {} + frete {}, rateado entre os itens pelo valor.",
                money(items_value + freight).to_pt_br(),
                money(items_value).to_pt_br(),
                money(freight).to_pt_br()
            ))
        });

        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if self.editing.is_some() {
                t.accent_edge
            } else {
                t.frame
            })
            .bg(t.surface)
            .child(kit::section_heading(if self.editing.is_some() {
                "Editar ordem de compra"
            } else {
                "Nova ordem de compra"
            }))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(
                        div().w(px(260.)).child(field(
                            "Fornecedor",
                            Select::new(&self.supplier)
                                .search_placeholder("Buscar")
                                .small()
                                .placeholder("Escolha o fornecedor")
                                .empty(|_, _| {
                                    div()
                                        .p_2()
                                        .text_sm()
                                        .child("Cadastre fornecedores em Ofertas.")
                                })
                                .into_any_element(),
                        )),
                    )
                    .child(div().w(px(130.)).child(field(
                        "Data da compra",
                        Input::new(&self.ordered_on).small().into_any_element(),
                    )))
                    .child(div().w(px(130.)).child(field(
                        "Frete",
                        Input::new(&self.freight).small().into_any_element(),
                    )))
                    .child(field(
                        "Moeda",
                        h_flex()
                            .gap_1()
                            .child(currency(Currency::Brl))
                            .child(currency(Currency::Usd))
                            .into_any_element(),
                    )),
            )
            .child(div().text_sm().font_medium().child("Itens"))
            .when(self.draft.is_empty(), |form| {
                form.child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("Nenhum item ainda: escolha o produto, a quantidade e o preço."),
                )
            })
            .children(lines)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(
                        div().w(px(300.)).child(field(
                            "Produto",
                            Select::new(&self.product)
                                .search_placeholder("Buscar")
                                .small()
                                .placeholder("Escolha o produto")
                                .empty(|_, _| {
                                    div()
                                        .p_2()
                                        .text_sm()
                                        .child("Crie produtos a partir de ofertas em Ofertas.")
                                })
                                .into_any_element(),
                        )),
                    )
                    .child(div().w(px(100.)).child(field(
                        "Quantidade",
                        Input::new(&self.quantity).small().into_any_element(),
                    )))
                    .child(div().w(px(130.)).child(field(
                        "Preço unitário",
                        Input::new(&self.unit_price).small().into_any_element(),
                    )))
                    .child(
                        Button::new("add-line")
                            .label("Adicionar item")
                            .outline()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.add_line(window, cx))),
                    ),
            )
            .children(summary)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("save-purchase-order")
                            .label(if self.editing.is_some() {
                                "Salvar alterações"
                            } else {
                                "Registrar ordem de compra"
                            })
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .when(self.editing.is_some(), |row| {
                        row.child(
                            Button::new("stop-editing")
                                .label("Descartar alterações")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_form(window, cx);
                                    this.form = None;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .children(self.form.as_ref().map(|outcome| notice(outcome, cx)))
            .into_any_element()
    }

    fn render_order(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let order = &self.orders[index];
        let id = order.id;
        let status = order.status();
        let step = self
            .step
            .as_ref()
            .filter(|(open, _)| *open == id)
            .map(|(_, step)| step);

        let mut facts = vec![
            format!("Comprada em {}", order.ordered_on.format("%d/%m/%Y")),
            units_text(order.units()),
            format!("frete {}", order.freight.to_pt_br()),
        ];
        if let Some(at) = order.shipped_at {
            facts.push(format!("enviada em {}", day(at)));
        }
        if let Some(at) = order.cancelled_at {
            facts.push(format!("cancelada em {}", day(at)));
        }

        let lines = order
            .lines
            .iter()
            .map(|line| {
                let (sku, name) = self.product_label(line.product);
                let receiving = match step {
                    Some(Step::Receive(fields)) => fields
                        .iter()
                        .find(|(open, _)| *open == line.id)
                        .map(|(_, field)| field.clone()),
                    _ => None,
                };
                h_flex()
                    .gap_3()
                    .items_center()
                    .py_1()
                    .border_t(t.border_width)
                    .border_color(t.border)
                    .child(kit::tag(sku, t.accent_text, cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().text_sm().child(name))
                            .child(div().text_xs().text_color(t.text2).child(format!(
                                "{} × {} · frete rateado {} · {} por unidade com frete",
                                line.quantity,
                                line.unit_price.to_pt_br(),
                                line.freight.to_pt_br(),
                                line.landed_unit_cost().to_pt_br()
                            ))),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(if line.remaining() == 0 {
                                t.success
                            } else {
                                t.text2
                            })
                            .child(format!("{} de {} recebidas", line.received, line.quantity)),
                    )
                    .children(receiving.map(|field| {
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(div().text_xs().text_color(t.text2).child("chegaram"))
                            .child(div().w(px(70.)).child(Input::new(&field).small()))
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let receipts = order
            .receipts
            .iter()
            .map(|receipt| {
                let units: u32 = receipt.lines.iter().map(|line| line.quantity).sum();
                let cost = Money::sum(order.currency(), receipt.lines.iter().map(|line| line.cost))
                    .expect("a receipt keeps the order's currency");
                div().text_xs().text_color(t.text2).child(format!(
                    "Entrada em {}: {} no estoque, {}",
                    day(receipt.at),
                    units_text(units),
                    cost.to_pt_br()
                ))
            })
            .collect::<Vec<_>>();

        let button = |id: (&'static str, usize), label: &'static str| {
            Button::new(id).label(label).small().disabled(self.busy)
        };
        let open = status != PurchaseOrderStatus::Cancelled && order.receipts.is_empty();
        let actions = match step {
            Some(Step::Receive(_)) => h_flex()
                .gap_2()
                .flex_wrap()
                .child(
                    button(("confirm-receive", index), "Dar entrada no estoque")
                        .primary()
                        .loading(self.busy)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.receive(id, window, cx)),
                        ),
                )
                .child(cancel_step(index, cx)),
            Some(Step::ConfirmCancel) => h_flex()
                .gap_2()
                .flex_wrap()
                .items_center()
                .child(div().text_sm().child("Cancelar esta ordem de compra?"))
                .child(
                    button(("confirm-cancel", index), "Cancelar ordem")
                        .danger()
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.cancel(id, window, cx)),
                        ),
                )
                .child(cancel_step(index, cx)),
            Some(Step::ConfirmDelete) => h_flex()
                .gap_2()
                .flex_wrap()
                .items_center()
                .child(
                    div()
                        .text_sm()
                        .child("Excluir esta ordem de compra do app?"),
                )
                .child(
                    button(("confirm-delete", index), "Excluir")
                        .danger()
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.delete(id, window, cx)),
                        ),
                )
                .child(cancel_step(index, cx)),
            None => {
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .when(
                        matches!(
                            status,
                            PurchaseOrderStatus::Purchased
                                | PurchaseOrderStatus::Shipped
                                | PurchaseOrderStatus::PartlyReceived
                        ),
                        |row| {
                            row.child(button(("receive", index), "Receber").primary().on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.start_receiving(id, window, cx)
                                }),
                            ))
                        },
                    )
                    .when(status == PurchaseOrderStatus::Purchased, |row| {
                        row.child(
                            button(("mark-shipped", index), "Marcar como enviada")
                                .outline()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.mark_shipped(id, window, cx)
                                })),
                        )
                    })
                    .when(open, |row| {
                        row.child(button(("edit", index), "Editar").ghost().on_click(
                            cx.listener(move |this, _, window, cx| this.edit(id, window, cx)),
                        ))
                        .child(
                            button(("cancel", index), "Cancelar")
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_step(id, Step::ConfirmCancel, cx)
                                })),
                        )
                    })
                    .when(order.receipts.is_empty(), |row| {
                        row.child(button(("delete", index), "Excluir").ghost().on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.open_step(id, Step::ConfirmDelete, cx)
                            }),
                        ))
                    })
            }
        };

        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if step.is_some() || self.editing == Some(id) {
                t.accent_edge
            } else {
                t.frame
            })
            .bg(t.surface)
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .font_medium()
                                            .child(self.supplier_name(order.supplier)),
                                    )
                                    .child(kit::tag(
                                        status_name(status),
                                        status_ink(status, &t),
                                        cx,
                                    )),
                            )
                            .child(div().text_xs().text_color(t.text2).child(facts.join(" · "))),
                    )
                    .child(div().font_semibold().child(order.total().to_pt_br())),
            )
            .children(lines)
            .children(receipts)
            .child(actions)
            .when(step.is_some(), |card| {
                card.children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            })
            .into_any_element()
    }
}

fn cancel_step(index: usize, cx: &mut Context<PurchasesScreen>) -> Button {
    Button::new(("close-step", index))
        .label("Voltar")
        .ghost()
        .small()
        .on_click(cx.listener(|this, _, _, cx| {
            this.step = None;
            this.outcome = None;
            cx.notify();
        }))
}

pub fn status_name(status: PurchaseOrderStatus) -> &'static str {
    match status {
        PurchaseOrderStatus::Purchased => "comprada",
        PurchaseOrderStatus::Shipped => "enviada",
        PurchaseOrderStatus::PartlyReceived => "recebida em parte",
        PurchaseOrderStatus::Received => "recebida",
        PurchaseOrderStatus::Cancelled => "cancelada",
    }
}

fn status_ink(status: PurchaseOrderStatus, t: &Tokens) -> Hsla {
    match status {
        PurchaseOrderStatus::Purchased => t.text2,
        PurchaseOrderStatus::Shipped | PurchaseOrderStatus::PartlyReceived => t.accent_text,
        PurchaseOrderStatus::Received => t.success,
        PurchaseOrderStatus::Cancelled => t.danger,
    }
}

/// "1 unidade", "4 unidades".
pub fn units_text(units: impl Into<i64>) -> String {
    match units.into() {
        1 => "1 unidade".into(),
        units => format!("{units} unidades"),
    }
}

/// An amount as the owner types it back: `12,50`.
fn typed(amount: Money) -> String {
    amount.amount().to_string().replace('.', ",")
}

fn today() -> String {
    Local::now().format("%d/%m/%Y").to_string()
}

/// A day as the owner types it: `01/10/2026`, `1/10/26` or `2026-10-01`.
fn parse_day(text: &str) -> Option<NaiveDate> {
    let text = text.trim();
    ["%d/%m/%Y", "%Y-%m-%d"]
        .iter()
        .find_map(|format| NaiveDate::parse_from_str(text, format).ok())
        .filter(|date| date.year() >= 2000)
        .or_else(|| NaiveDate::parse_from_str(text, "%d/%m/%y").ok())
}

/// Why a Purchase Order action failed, as the owner reads it.
pub fn failure(error: &PurchaseOrderError) -> String {
    match error {
        PurchaseOrderError::NoLines => "Adicione pelo menos um item à ordem de compra.".into(),
        PurchaseOrderError::NoUnits => "Cada item precisa de pelo menos 1 unidade.".into(),
        PurchaseOrderError::Negative => "Preços e frete não podem ser negativos.".into(),
        PurchaseOrderError::Currencies(_) => "Preços e frete precisam estar na mesma moeda.".into(),
        PurchaseOrderError::UnknownPurchaseOrder(_) | PurchaseOrderError::UnknownLine(_) => {
            "Essa ordem de compra mudou; a lista foi atualizada.".into()
        }
        PurchaseOrderError::AlreadyReceiving => {
            "Unidades desta ordem já entraram no estoque, então ela não muda mais.".into()
        }
        PurchaseOrderError::Cancelled => "Esta ordem de compra está cancelada.".into(),
        PurchaseOrderError::NotAwaitingShipment => {
            "Só uma ordem ainda não enviada pode ser marcada como enviada.".into()
        }
        PurchaseOrderError::TooMany { remaining, .. } => format!(
            "Um dos itens só tem {} a receber.",
            units_text(i64::from(*remaining))
        ),
        PurchaseOrderError::NothingToReceive => {
            "Digite quantas unidades chegaram de pelo menos um item.".into()
        }
        PurchaseOrderError::Stock(error) => inventory_failure(error),
        PurchaseOrderError::Unreadable(_) | PurchaseOrderError::Sql(_) => {
            format!("Não consegui ler ou gravar no banco: {error}")
        }
    }
}

impl Render for PurchasesScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut parts = ScreenParts::new("Compras");
        if purchase_orders(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        // A step's outcome shows on its row; a finished one, on top.
        if self.step.is_none() {
            parts
                .notices
                .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        }
        parts.content.push(self.render_form(cx));
        parts.content.push(
            v_flex()
                .gap_3()
                .child(kit::section_heading("Ordens de compra"))
                .when(self.orders.is_empty(), |list| {
                    list.child(div().text_sm().text_color(t.text2).child(
                        "Nenhuma ordem de compra ainda. Registre o que você comprou de um \
                         fornecedor; quando chegar, use Receber para dar entrada no estoque.",
                    ))
                })
                .children((0..self.orders.len()).map(|index| self.render_order(index, cx)))
                .into_any_element(),
        );
        layout::screen(parts, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_days_as_brazilians_type_them() {
        let first_of_october = NaiveDate::from_ymd_opt(2026, 10, 1);
        assert_eq!(parse_day("01/10/2026"), first_of_october);
        assert_eq!(parse_day(" 1/10/2026 "), first_of_october);
        assert_eq!(parse_day("1/10/26"), first_of_october);
        assert_eq!(parse_day("2026-10-01"), first_of_october);
        assert_eq!(parse_day("31/02/2026"), None);
        assert_eq!(parse_day("10/2026"), None);
    }

    #[test]
    fn an_amount_reads_back_as_typed() {
        let amount = Money::new(Decimal::new(1250, 2), Currency::Brl);
        assert_eq!(typed(amount), "12,50");
        assert_eq!(parse_amount(&typed(amount)), Some(amount.amount()));
    }
}
