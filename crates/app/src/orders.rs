//! Pedidos (#20, #21): the owner's Mercado Livre Orders, read by an Order
//! Sync when the app opens and every few minutes after, window open or in
//! the tray (ADR 0018). Each new Order comes with a system notification, and
//! its units leave the stock; the stock of the listings follows right after.
//! Each Order waiting for dispatch prints its label and shows when it is
//! late or close; units on their way back wait for the owner to say they
//! arrived (ADR 0019). After each Sync the app asks Mercado Livre's billing
//! for the Orders' Fees, and each Order shows its Realized Margin (ADR
//! 0020). Settings › Pedidos sets how often, how long the buyer's data stays
//! and how early the warning comes. The rules live in Commerce.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Entity, Global, Hsla, Subscription, SystemNotification, Task, Window, div, px,
};
use mascate_catalog::Product;
use mascate_commerce::{
    DispatchDue, LineStock, Order, OrderError, OrderLine, OrderSettings, OrderStatus, OrderSync,
    Orders, RealizedMargin, ReturnReceipt, Sale, ShipmentStatus, ShippingLabel, StockShort,
};
use mascate_integrations::{Connection, ConnectionState};
use mascate_kernel::{RecordId, Timestamp};
use mascate_platform::default_owner_folder;

use crate::ads;
use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE};
use crate::connections::AppConnections;
use crate::forms::{Outcome, input, notice};
use crate::kit;
use crate::layout;
use crate::low_stock;
use crate::mercado_livre;
use crate::parts::ScreenParts;
use crate::pricing;
use crate::promotions;
use crate::purchases::units_text;
use crate::questions;
use crate::reputation;
use crate::sales;
use crate::stock::product_label;
use crate::stock_mirror;

/// The app's Orders; absent when the database did not open.
pub struct AppOrders(pub Arc<Orders>);

impl Global for AppOrders {}

pub fn orders(cx: &App) -> Option<Arc<Orders>> {
    cx.try_global::<AppOrders>().map(|app| app.0.clone())
}

/// The Order Syncs since the app opened; the Pedidos screen reads the
/// Orders again after each.
#[derive(Default)]
pub struct OrderSyncs {
    pub finished: u64,
    /// Why the last one failed, until one goes through.
    pub failure: Option<String>,
}

impl Global for OrderSyncs {}

/// What one Order Sync did, with the Products its notifications name.
pub struct Round {
    pub synced: OrderSync,
    products: Vec<Product>,
}

/// Why reading or saving the Orders failed, as the owner reads it.
pub fn failure(error: &OrderError) -> String {
    match error {
        OrderError::Platform(error) => mercado_livre::failure(error),
        OrderError::InvalidSettings => "O Sync roda a cada 1 a 60 minutos, os dados do \
                                        comprador ficam de 7 a 1825 dias e o aviso de despacho \
                                        vem de 1 a 72 horas antes."
            .into(),
        OrderError::NoLabel => "Este pedido não está esperando despacho, então não tem etiqueta \
                                para imprimir. Sincronize os pedidos para ver o estado atual."
            .into(),
        OrderError::NoReturnAwaiting => {
            "Este pedido não tem unidades voltando; nada mudou no estoque.".into()
        }
        error => format!("Não consegui ler ou gravar os pedidos no banco: {error}"),
    }
}

/// Erases the buyer data past its time, then runs an Order Sync now and
/// every few minutes after, for as long as the app runs, window open or
/// not, each followed by a Sync of the buyers' questions (#24) and, hourly,
/// of the Reputation and the Reviews (#25) and the Promotions (#26). The
/// first one covers the time the app was closed.
pub fn start_polling(cx: &mut App) {
    let Some(orders) = orders(cx) else {
        return;
    };
    cx.set_global(OrderSyncs::default());
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        let erasing = orders.clone();
        if let Err(error) = executor
            .spawn(async move { erasing.erase_expired_buyer_data().await })
            .await
        {
            eprintln!("could not erase the expired buyer data: {error}");
        }
        loop {
            if let Some(round) = cx.update(sync_now) {
                let outcome = round.await;
                cx.update(|cx| finished(&outcome, cx));
            }
            if let Some(round) = cx.update(questions::sync_now) {
                let outcome = round.await;
                cx.update(|cx| questions::finished(&outcome, cx));
            }
            if let Some(round) = cx.update(|cx| reputation::sync_now(cx, false)) {
                let outcome = round.await;
                cx.update(|cx| reputation::finished(&outcome, cx));
            }
            if let Some(round) = cx.update(|cx| promotions::sync_now(cx, false)) {
                let outcome = round.await;
                cx.update(|cx| promotions::finished(&outcome, cx));
            }
            if let Some(round) = cx.update(|cx| ads::sync_now(cx, false)) {
                let outcome = round.await;
                cx.update(|cx| ads::finished(&outcome, cx));
            }
            let reading = orders.clone();
            let every = executor
                .spawn(async move { reading.settings().await })
                .await
                .unwrap_or_default()
                .sync_every_minutes;
            executor
                .timer(Duration::from_secs(u64::from(every) * 60))
                .await;
        }
    })
    .detach();
}

/// Runs an Order Sync off the UI thread and then sends the stock still to
/// send, Orders first (ADR 0017). `Ok(None)` when the Connection is not up.
pub fn sync_now(cx: &mut App) -> Option<Task<Result<Option<Round>, String>>> {
    let (Some(orders), Some(mirror), Some(channel), Some(catalog)) = (
        orders(cx),
        stock_mirror::mirror(cx),
        mercado_livre::adapter(cx),
        catalog::catalog(cx),
    ) else {
        return None;
    };
    let connections = cx.global::<AppConnections>().0.clone();
    Some(cx.background_executor().spawn(async move {
        if connections.state(Connection::MercadoLivre) != ConnectionState::Connected {
            return Ok(None);
        }
        let synced = orders
            .sync(channel.as_ref())
            .await
            .map_err(|e| failure(&e))?;
        if let Err(error) = orders.import_fees(channel.as_ref()).await {
            eprintln!("could not read the Orders' Fees: {error}");
        }
        if let Err(error) = orders.import_releases(channel.as_ref()).await {
            eprintln!("could not read the Orders' Money Releases: {error}");
        }
        if let Err(error) = mirror.send(channel.as_ref()).await {
            eprintln!("could not send the listings' stock: {error}");
        }
        let products = if synced.reached_reorder_point.is_empty() {
            Vec::new()
        } else {
            catalog.products().await.unwrap_or_default()
        };
        Ok(Some(Round { synced, products }))
    }))
}

/// Tells the owner what an Order Sync brought, and the screens that it ran.
pub fn finished(outcome: &Result<Option<Round>, String>, cx: &mut App) {
    if let Ok(Some(round)) = outcome {
        notify_arrived(&round.synced.arrived, cx);
        for low in &round.synced.reached_reorder_point {
            low_stock::notify(*low, &round.products, cx);
        }
    }
    let failure = outcome.as_ref().err().cloned();
    let syncs = cx.default_global::<OrderSyncs>();
    syncs.finished += 1;
    syncs.failure = failure;
}

/// Above this many new Orders in one Sync, as after days closed, one
/// notification sums them up instead of one each.
const NOTIFIED_ONE_BY_ONE: usize = 3;

/// A system notification for each new Order, or one for all of them when
/// many arrived at once.
fn notify_arrived(arrived: &[Order], cx: &App) {
    if arrived.len() > NOTIFIED_ONE_BY_ONE {
        let body = arrived
            .iter()
            .take(NOTIFIED_ONE_BY_ONE)
            .map(sold_text)
            .collect::<Vec<_>>()
            .join("; ");
        cx.show_system_notification(SystemNotification {
            tag: "new-orders".into(),
            title: format!("{} pedidos novos no Mercado Livre", arrived.len()).into(),
            body: format!("{body} e outros.").into(),
            actions: Vec::new(),
        });
        return;
    }
    for order in arrived {
        let mut body = format!("{} · {}", sold_text(order), order.sold.total.to_pt_br());
        if let Some(by) = dispatch_by(order) {
            body.push_str(&format!(". Despachar até {}", catalog::day_and_time(by)));
        }
        cx.show_system_notification(SystemNotification {
            tag: format!("order-{}", order.sold.id).into(),
            title: "Pedido novo no Mercado Livre".into(),
            body: format!("{body}.").into(),
            actions: Vec::new(),
        });
    }
}

/// "2× Fone Bluetooth", and "+ 1 item" for each other line.
pub fn sold_text(order: &Order) -> String {
    let mut lines = order.lines.iter();
    let Some(first) = lines.next() else {
        return format!("Pedido {}", order.sold.id);
    };
    let mut text = format!("{}× {}", first.sold.quantity, first.sold.title);
    match lines.count() {
        0 => {}
        1 => text.push_str(" + 1 item"),
        more => text.push_str(&format!(" + {more} itens")),
    }
    text
}

fn dispatch_by(order: &Order) -> Option<Timestamp> {
    order
        .sold
        .shipment
        .as_ref()
        .filter(|shipment| shipment.status == ShipmentStatus::ReadyToShip)
        .and_then(|shipment| shipment.dispatch_by)
}

/// "3 pedidos novos. 2 itens baixados do estoque." What is still short
/// shows on the screen, with its reason.
fn synced_text(synced: &OrderSync) -> String {
    let mut text = match (synced.new, synced.changed) {
        (0, 0) => "Nenhum pedido novo ou alterado.".to_owned(),
        (0, 1) => "1 pedido alterado.".to_owned(),
        (0, changed) => format!("{changed} pedidos alterados."),
        (1, _) => "1 pedido novo.".to_owned(),
        (new, _) => format!("{new} pedidos novos."),
    };
    match synced.taken {
        0 => {}
        1 => text.push_str(" 1 item baixado do estoque."),
        taken => text.push_str(&format!(" {taken} itens baixados do estoque.")),
    }
    match synced.restocked {
        0 => {}
        1 => text.push_str(" 1 pedido cancelado antes do envio devolveu as unidades ao estoque."),
        restocked => text.push_str(&format!(
            " {restocked} pedidos cancelados antes do envio devolveram as unidades ao estoque."
        )),
    }
    text
}

/// Where labels go: `Documentos/Mascate/Etiquetas`.
const LABELS_FOLDER: &str = "Etiquetas";

/// Downloads the label of `order` off the UI thread, saves it in the
/// labels folder and opens it in the system's PDF viewer, ready to print.
/// Says what happened.
pub fn print_label(order: RecordId, cx: &mut App) -> Option<Task<Result<String, String>>> {
    let (Some(orders), Some(labels)) = (orders(cx), mercado_livre::adapter(cx)) else {
        return None;
    };
    let folder = default_owner_folder(LABELS_FOLDER)?;
    let saving = cx.background_executor().spawn(async move {
        let label: ShippingLabel = orders
            .shipping_label(labels.as_ref(), order)
            .await
            .map_err(|e| failure(&e))?;
        std::fs::create_dir_all(&folder)
            .and_then(|()| std::fs::write(folder.join(&label.file_name), &label.pdf))
            .map_err(|error| {
                format!(
                    "Não consegui salvar a etiqueta em {}: {error}",
                    folder.display()
                )
            })?;
        Ok::<_, String>(folder.join(&label.file_name))
    });
    Some(cx.spawn(async move |cx| {
        let path = saving.await?;
        cx.update(|cx| cx.open_with_system(&path));
        Ok(format!(
            "Etiqueta salva em {} e aberta para imprimir.",
            path.display()
        ))
    }))
}

/// "Atrasado" or "Despachar logo", in the ink that says how urgent.
pub fn due_tag(due: DispatchDue, t: &Tokens) -> (&'static str, Hsla) {
    match due {
        DispatchDue::Late => ("Atrasado", t.danger),
        DispatchDue::Soon => ("Prazo perto", t.accent_text),
    }
}

fn items_text(count: usize) -> String {
    match count {
        1 => "1 item".into(),
        count => format!("{count} itens"),
    }
}

/// Everything the screen shows, read in one go off the UI thread.
struct Snapshot {
    sales: Vec<Sale>,
    products: Vec<Product>,
    last_sync: Option<Timestamp>,
    settings: OrderSettings,
    connection: ConnectionState,
}

pub struct OrdersScreen {
    orders: Vec<Order>,
    /// Each Order's Realized Margin, by the Order.
    margins: BTreeMap<RecordId, RealizedMargin>,
    products: Vec<Product>,
    last_sync: Option<Timestamp>,
    settings: OrderSettings,
    connection: Option<ConnectionState>,
    syncing: bool,
    /// The Order whose label or return is on its way.
    working_on: Option<RecordId>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl OrdersScreen {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        // A Sync in the background brings Orders this screen has not read,
        // and one of Product Ads their share of what the ads cost.
        let subscriptions = vec![
            cx.observe_global::<OrderSyncs>(Self::refresh),
            cx.observe_global::<ads::AdsSyncs>(Self::refresh),
        ];
        let mut screen = Self {
            orders: Vec::new(),
            margins: BTreeMap::new(),
            products: Vec::new(),
            last_sync: None,
            settings: OrderSettings::default(),
            connection: None,
            syncing: false,
            working_on: None,
            reads: 0,
            outcome: None,
            _subscriptions: subscriptions,
        };
        screen.refresh(cx);
        screen
    }

    /// Reads the Orders again, as when the screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(orders), Some(catalog), Some(taxes)) =
            (orders(cx), catalog::catalog(cx), pricing::taxes(cx))
        else {
            return;
        };
        let connections = cx.global::<AppConnections>().0.clone();
        let product_ads = ads::product_ads(cx);
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            let tax = taxes
                .rate()
                .await
                .map_err(|e| format!("Não consegui ler a alíquota de imposto: {e}"))?;
            let ad_costs = ads::costs(product_ads.as_deref()).await?;
            Ok::<_, String>(Snapshot {
                sales: orders
                    .sales(tax, sales::ever(), &ad_costs)
                    .await
                    .map_err(|e| failure(&e))?
                    .sales,
                products: catalog.products().await.map_err(|e| catalog::failure(&e))?,
                last_sync: orders.last_sync().await.map_err(|e| failure(&e))?,
                settings: orders.settings().await.map_err(|e| failure(&e))?,
                connection: connections.state(Connection::MercadoLivre),
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
                        this.margins = snapshot
                            .sales
                            .iter()
                            .map(|sale| (sale.order.id, sale.margin.clone()))
                            .collect();
                        this.orders = snapshot.sales.into_iter().map(|sale| sale.order).collect();
                        this.products = snapshot.products;
                        this.last_sync = snapshot.last_sync;
                        this.settings = snapshot.settings;
                        this.connection = Some(snapshot.connection);
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(round) = sync_now(cx) else {
            return;
        };
        self.syncing = true;
        self.outcome = None;
        cx.spawn(async move |this, cx| {
            let outcome = round.await;
            let _ = this.update(cx, |this, cx| {
                this.syncing = false;
                this.outcome = Some(match &outcome {
                    Ok(Some(round)) => Outcome::Done(synced_text(&round.synced).into()),
                    Ok(None) => Outcome::Failed(
                        mercado_livre::failure(&mascate_kernel::PlatformError::NotConnected).into(),
                    ),
                    Err(error) => Outcome::Failed(error.clone().into()),
                });
                // Reads the Orders again, through the screen's observer.
                finished(&outcome, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn connected(&self) -> bool {
        self.connection == Some(ConnectionState::Connected)
    }

    fn print_label(&mut self, order: RecordId, cx: &mut Context<Self>) {
        let Some(printing) = print_label(order, cx) else {
            return;
        };
        self.working_on = Some(order);
        self.outcome = None;
        cx.spawn(async move |this, cx| {
            let printed = printing.await;
            let _ = this.update(cx, |this, cx| {
                this.working_on = None;
                this.outcome = Some(match printed {
                    Ok(done) => Outcome::Done(done.into()),
                    Err(error) => Outcome::Failed(error.into()),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn receive_return(&mut self, order: RecordId, receipt: ReturnReceipt, cx: &mut Context<Self>) {
        let Some(orders) = orders(cx) else {
            return;
        };
        self.working_on = Some(order);
        self.outcome = None;
        let receiving = cx.background_executor().spawn(async move {
            orders
                .receive_return(order, receipt)
                .await
                .map_err(|e| failure(&e))
        });
        cx.spawn(async move |this, cx| {
            let received = receiving.await;
            let _ = this.update(cx, |this, cx| {
                this.working_on = None;
                this.outcome = Some(match received {
                    Ok(received) if received.restocked > 0 => {
                        stock_mirror::send_after_movement(cx);
                        Outcome::Done(
                            format!("{} de volta ao estoque.", units_text(received.restocked))
                                .into(),
                        )
                    }
                    Ok(received) => Outcome::Done(
                        format!(
                            "Devolução fechada: {} sem condição de venda, o estoque não mudou.",
                            units_text(received.units)
                        )
                        .into(),
                    ),
                    Err(error) => Outcome::Failed(error.into()),
                });
                this.refresh(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn render_sync(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let when = match self.last_sync {
            Some(at) => format!(
                "Último Sync: {} · a cada {} min",
                catalog::day_and_time(at),
                self.settings.sync_every_minutes
            ),
            None => "Nenhum Sync ainda.".into(),
        };
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_xs().text_color(t.text2).child(when))
            .child(
                Button::new("sync-orders")
                    .label("Sincronizar")
                    .icon(IconName::RefreshCw)
                    .primary()
                    .small()
                    .loading(self.syncing)
                    .disabled(self.syncing || !self.connected())
                    .on_click(cx.listener(|this, _, _, cx| this.sync(cx))),
            )
            .into_any_element()
    }

    fn render_order(&self, index: usize, order: &Order, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let sold = &order.sold;
        let mut facts = vec![catalog::day_and_time(sold.ordered_at)];
        facts.extend(sold.pack.as_ref().map(|pack| format!("carrinho {pack}")));
        let buyer = match (&sold.buyer, order.buyer_data_erased_at) {
            (_, Some(at)) => format!(
                "Dados do comprador apagados em {} (prazo de {} dias)",
                catalog::day_and_time(at),
                self.settings_days()
            ),
            (Some(buyer), None) => {
                let mut parts: Vec<String> = buyer.nickname.iter().cloned().collect();
                if let Some(receiver) = &buyer.receiver {
                    let mut to = receiver.name.clone();
                    let place: Vec<&str> = [receiver.city.as_deref(), receiver.state.as_deref()]
                        .into_iter()
                        .flatten()
                        .collect();
                    if !place.is_empty() {
                        to.push_str(&format!(", {}", place.join(" / ")));
                    }
                    parts.push(format!("para {to}"));
                }
                parts.join(" · ")
            }
            (None, None) => "Comprador sem dados".into(),
        };
        let shipment = sold.shipment.as_ref().map(|shipment| {
            let mut text = shipment_name(shipment.status).to_owned();
            if let Some(by) = shipment
                .dispatch_by
                .filter(|_| shipment.status == ShipmentStatus::ReadyToShip)
            {
                text.push_str(&format!(" até {}", catalog::day_and_time(by)));
            }
            (text, shipment_ink(shipment.status, &t))
        });
        let due = order
            .dispatch_due(chrono::Utc::now(), self.settings.dispatch_warning())
            .map(|due| due_tag(due, &t));
        let actions = self.render_actions(index, order, cx);
        let lines: Vec<AnyElement> = order
            .lines
            .iter()
            .map(|line| self.render_line(line, cx))
            .collect();
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .font_medium()
                                    .truncate()
                                    .child(format!("Pedido {}", sold.id)),
                            )
                            .child(kit::tag(
                                status_name(sold.status),
                                status_ink(sold.status, &t),
                                cx,
                            ))
                            .children(shipment.map(|(text, ink)| kit::tag(text, ink, cx)))
                            .children(due.map(|(text, ink)| kit::tag(text, ink, cx))),
                    )
                    .child(div().flex_none().font_medium().child(sold.total.to_pt_br())),
            )
            .child(div().text_xs().text_color(t.text2).child(facts.join(" · ")))
            .child(div().text_sm().child(buyer))
            .children(lines)
            .children(
                self.margins
                    .get(&order.id)
                    .map(|margin| sales::margin_detail(margin, cx)),
            )
            .children(actions)
            .into_any_element()
    }

    /// The label to print while the Order waits for dispatch, and the
    /// owner's word on units on their way back.
    fn render_actions(
        &self,
        index: usize,
        order: &Order,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = look(cx).tokens;
        let id = order.id;
        let busy = self.working_on.is_some();
        let waiting = order.sold.status != OrderStatus::Cancelled
            && order
                .sold
                .shipment
                .as_ref()
                .is_some_and(|shipment| shipment.status == ShipmentStatus::ReadyToShip);
        let awaiting = order.units_awaiting_return();
        if !waiting && awaiting == 0 {
            return None;
        }
        let key = |name: &'static str| (name, index);
        Some(
            h_flex()
                .flex_wrap()
                .gap_2()
                .items_center()
                .when(waiting, |row| {
                    row.child(
                        Button::new(key("print-label"))
                            .label("Baixar etiqueta")
                            .icon(IconName::ArrowDown)
                            .outline()
                            .small()
                            .loading(self.working_on == Some(id))
                            .disabled(busy || !self.connected())
                            .on_click(cx.listener(move |this, _, _, cx| this.print_label(id, cx))),
                    )
                })
                .when(awaiting > 0, |row| {
                    row.child(div().text_sm().text_color(t.accent_text).child(format!(
                        "{} voltando. Quando chegar, confirme:",
                        units_text(awaiting)
                    )))
                    .child(
                        Button::new(key("return-in-stock"))
                            .label("Chegou, volta ao estoque")
                            .primary()
                            .small()
                            .loading(self.working_on == Some(id))
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.receive_return(id, ReturnReceipt::BackInStock, cx)
                            })),
                    )
                    .child(
                        Button::new(key("return-unsellable"))
                            .label("Chegou sem condição de venda")
                            .ghost()
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.receive_return(id, ReturnReceipt::Unsellable, cx)
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    fn settings_days(&self) -> u32 {
        self.settings.keep_buyer_data_days
    }

    fn render_line(&self, line: &OrderLine, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let sold = &line.sold;
        let mut title = format!("{}× {}", sold.quantity, sold.title);
        if let Some(variation) = &sold.variation_name {
            title.push_str(&format!(" ({variation})"));
        }
        let (stock, ink) = self.stock_text(line, &t);
        h_flex()
            .gap_3()
            .items_start()
            .justify_between()
            .pl_3()
            .border_l_2()
            .border_color(t.border)
            .child(
                v_flex()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().text_sm().truncate().child(title))
                    .child(div().text_xs().text_color(ink).child(stock)),
            )
            .child(
                div()
                    .flex_none()
                    .text_sm()
                    .text_color(t.text2)
                    .child(format!("{} cada", sold.unit_price.to_pt_br())),
            )
            .into_any_element()
    }

    fn stock_text(&self, line: &OrderLine, t: &Tokens) -> (String, Hsla) {
        match line.stock {
            LineStock::Taken { product, .. } => {
                let (sku, _) = product_label(&self.products, product);
                let mut text = match line.sold.quantity {
                    1 => format!("1 unidade saiu do estoque de {sku}"),
                    units => format!("{units} unidades saíram do estoque de {sku}"),
                };
                let back = line.back;
                if back.restocked > 0 {
                    text.push_str(&format!("; {} de volta", units_text(back.restocked)));
                }
                if back.unsellable > 0 {
                    text.push_str(&format!(
                        "; {} sem condição de venda",
                        units_text(back.unsellable)
                    ));
                }
                if back.awaiting > 0 {
                    text.push_str(&format!("; {} voltando", units_text(back.awaiting)));
                    return (text, t.accent_text);
                }
                (text, t.success)
            }
            LineStock::Short(StockShort::NoListing) => (
                "Sem baixa: o anúncio ainda não está em Anúncios. Sincronize os anúncios e \
                 vincule-o a um produto; a baixa sai no próximo Sync."
                    .into(),
                t.danger,
            ),
            LineStock::Short(StockShort::NoProduct) => (
                "Sem baixa: o anúncio não está vinculado a um produto. Vincule em Anúncios; a \
                 baixa sai no próximo Sync."
                    .into(),
                t.danger,
            ),
            LineStock::Short(StockShort::NotEnoughStock) => (
                "Sem baixa: o estoque do app tem menos unidades do que as vendidas. Registre a \
                 entrada ou a contagem em Estoque; a baixa sai no próximo Sync."
                    .into(),
                t.danger,
            ),
            LineStock::BeforeFirstSync => (
                "Vendido antes do primeiro Sync de pedidos: o estoque já não tinha estas \
                 unidades."
                    .into(),
                t.text2,
            ),
            LineStock::Cancelled => ("Cancelado antes da baixa.".into(), t.text2),
            LineStock::Dropship => ("Enviado pelo fornecedor.".into(), t.text2),
        }
    }
}

fn status_name(status: OrderStatus) -> &'static str {
    match status {
        OrderStatus::AwaitingPayment => "Aguardando pagamento",
        OrderStatus::Paid => "Pago",
        OrderStatus::Cancelled => "Cancelado",
    }
}

fn status_ink(status: OrderStatus, t: &Tokens) -> Hsla {
    match status {
        OrderStatus::Paid => t.success,
        OrderStatus::AwaitingPayment => t.accent_text,
        OrderStatus::Cancelled => t.text2,
    }
}

fn shipment_name(status: ShipmentStatus) -> &'static str {
    match status {
        ShipmentStatus::Pending => "Envio pendente",
        ShipmentStatus::ReadyToShip => "Despachar",
        ShipmentStatus::Shipped => "Enviado",
        ShipmentStatus::Delivered => "Entregue",
        ShipmentStatus::NotDelivered => "Não entregue",
        ShipmentStatus::Cancelled => "Envio cancelado",
    }
}

fn shipment_ink(status: ShipmentStatus, t: &Tokens) -> Hsla {
    match status {
        ShipmentStatus::ReadyToShip => t.accent_text,
        ShipmentStatus::Delivered => t.success,
        ShipmentStatus::NotDelivered => t.danger,
        ShipmentStatus::Pending | ShipmentStatus::Shipped | ShipmentStatus::Cancelled => t.text2,
    }
}

impl Render for OrdersScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut parts = ScreenParts::new("Pedidos");
        if orders(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        parts.actions.push(self.render_sync(cx));
        if let Some(state) = &self.connection
            && *state != ConnectionState::Connected
        {
            parts
                .notices
                .push(kit::info_notice(mercado_livre::not_connected(state), cx).into_any_element());
        }
        match &self.outcome {
            Some(outcome) => parts.notices.push(notice(outcome, cx)),
            None => parts.notices.extend(
                cx.try_global::<OrderSyncs>()
                    .and_then(|syncs| syncs.failure.clone())
                    .map(|error| {
                        kit::error_notice(format!("O último Sync de pedidos falhou: {error}"), cx)
                            .into_any_element()
                    }),
            ),
        }
        let short = self
            .orders
            .iter()
            .flat_map(|order| &order.lines)
            .filter(|line| matches!(line.stock, LineStock::Short(_)))
            .count();
        if short > 0 {
            parts.notices.push(
                kit::error_notice(
                    format!(
                        "{} ainda sem baixa no estoque; o motivo está em cada pedido.",
                        items_text(short)
                    ),
                    cx,
                )
                .into_any_element(),
            );
        }
        let heading = match self.orders.len() {
            0 => "Pedidos no Mercado Livre".to_owned(),
            1 => "1 pedido".to_owned(),
            count => format!("{count} pedidos"),
        };
        let cards: Vec<AnyElement> = self
            .orders
            .iter()
            .enumerate()
            .map(|(index, order)| self.render_order(index, order, cx))
            .collect();
        parts.content.push(
            v_flex()
                .gap_3()
                .child(kit::section_heading(heading))
                .when(self.orders.is_empty(), |list| {
                    list.child(div().text_sm().text_color(t.text2).child(format!(
                        "Nenhum pedido ainda. Com a conta do Mercado Livre conectada, o app busca \
                         os pedidos ao abrir e a cada {} minutos, também na bandeja; cada pedido \
                         novo dá baixa no estoque e chega como notificação.",
                        self.settings.sync_every_minutes
                    )))
                })
                .children(cards)
                .into_any_element(),
        );
        layout::screen(parts, cx)
    }
}

/// Settings › Pedidos: how often the app reads Orders and how long the
/// buyer's data stays.
pub struct OrderSettingsSection {
    every: Entity<InputState>,
    keep: Entity<InputState>,
    warn: Entity<InputState>,
    busy: bool,
    outcome: Option<Outcome>,
}

impl OrderSettingsSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let section = Self {
            every: input("5", window, cx),
            keep: input("90", window, cx),
            warn: input("24", window, cx),
            busy: false,
            outcome: None,
        };
        section.load(window, cx);
        section
    }

    fn load(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(orders) = orders(cx) else {
            return;
        };
        let reading = cx
            .background_executor()
            .spawn(async move { orders.settings().await.map_err(|e| failure(&e)) });
        cx.spawn_in(window, async move |this, cx| {
            let read = reading.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match read {
                    Ok(settings) => {
                        this.every.update(cx, |input, cx| {
                            input.set_value(settings.sync_every_minutes.to_string(), window, cx)
                        });
                        this.keep.update(cx, |input, cx| {
                            input.set_value(settings.keep_buyer_data_days.to_string(), window, cx)
                        });
                        this.warn.update(cx, |input, cx| {
                            let hours = settings.warn_before_dispatch_hours.to_string();
                            input.set_value(hours, window, cx)
                        });
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(orders) = orders(cx) else {
            return;
        };
        let number = |field: &Entity<InputState>| field.read(cx).value().trim().parse::<u32>();
        let (Ok(every), Ok(keep), Ok(warn)) =
            (number(&self.every), number(&self.keep), number(&self.warn))
        else {
            self.outcome = Some(Outcome::Failed(
                "Digite os minutos entre os Syncs (1 a 60), os dias para guardar os dados do \
                 comprador (7 a 1825) e as horas de aviso antes do prazo de despacho (1 a 72)."
                    .into(),
            ));
            cx.notify();
            return;
        };
        let settings = OrderSettings {
            sync_every_minutes: every,
            keep_buyer_data_days: keep,
            warn_before_dispatch_hours: warn,
        };
        self.busy = true;
        self.outcome = None;
        let saving = cx.background_executor().spawn(async move {
            orders
                .save_settings(settings)
                .await
                .map_err(|e| failure(&e))
        });
        cx.spawn(async move |this, cx| {
            let saved = saving.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.outcome = Some(match saved {
                    Ok(()) => Outcome::Done(
                        format!(
                            "Salvo: Sync de pedidos a cada {every} min a partir do próximo, \
                             dados do comprador por {keep} dias e aviso de despacho {warn} h \
                             antes do prazo."
                        )
                        .into(),
                    ),
                    Err(error) => Outcome::Failed(error.into()),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for OrderSettingsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .w(px(260.))
                .child(div().text_sm().font_medium().child(label))
                .child(Input::new(input).small())
        };
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(div().text_sm().text_color(t.text2).child(
                "Com o app aberto ou na bandeja, os pedidos do Mercado Livre chegam por Sync: ao \
                 abrir o app (cobrindo o tempo em que ficou fechado) e depois a cada intervalo. \
                 Do comprador o app guarda só apelido, nome de quem recebe e endereço de entrega, \
                 e apaga esses dados dos pedidos mais antigos que o prazo. Pedidos a despachar \
                 aparecem em Hoje quando o prazo chega perto, e atrasados quando passa.",
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(field("Sync a cada (minutos)", &self.every))
                    .child(field("Guardar dados do comprador (dias)", &self.keep))
                    .child(field(
                        "Avisar antes do prazo de despacho (horas)",
                        &self.warn,
                    ))
                    .child(
                        Button::new("save-order-settings")
                            .label("Salvar")
                            .outline()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sync(new: usize, changed: usize, taken: usize, short: usize) -> OrderSync {
        OrderSync {
            new,
            changed,
            taken,
            short,
            ..OrderSync::default()
        }
    }

    #[test]
    fn a_sync_reads_in_portuguese() {
        assert_eq!(
            synced_text(&sync(0, 0, 0, 0)),
            "Nenhum pedido novo ou alterado."
        );
        assert_eq!(
            synced_text(&sync(2, 1, 3, 0)),
            "2 pedidos novos. 3 itens baixados do estoque."
        );
        // What is still short shows on the screen, not in the Sync's line.
        assert_eq!(synced_text(&sync(1, 0, 0, 1)), "1 pedido novo.");
        assert_eq!(synced_text(&sync(0, 2, 0, 0)), "2 pedidos alterados.");
        assert_eq!(
            synced_text(&OrderSync {
                restocked: 1,
                ..sync(0, 1, 0, 0)
            }),
            "1 pedido alterado. 1 pedido cancelado antes do envio devolveu as unidades ao \
             estoque."
        );
    }
}
