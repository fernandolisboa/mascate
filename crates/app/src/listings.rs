//! Anúncios (#15): the owner's Mercado Livre listings, synced, and the
//! queue of the ones still without a Product: confirm the suggested one,
//! pick another, or make a Product from the listing. Each linked listing
//! opens its price (#19): the Price Suggestion from the target margin and
//! the price simulator. A price reaches Mercado Livre only when the owner
//! sends it. The stock of each linked listing follows the app's (#18): the
//! queue shows what is still to send and why it did not go, and each
//! listing can be paused and reactivated.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::Select;
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Entity, EventEmitter, Global, Hsla, Subscription, Window, div, px,
};
use mascate_catalog::{Catalog, Product};
use mascate_commerce::{
    CatalogProduct, Listing, ListingError, ListingStatus, ListingSync, ListingToLink, Listings,
    MirroredStock, PriceAssumptions, PriceBreakdown, PriceScenario, PriceSuggestion, PricingError,
    SaleFee, StockSend, SuggestedBy,
};
use mascate_integrations::{Connection, ConnectionState};
use mascate_kernel::{Money, Percentage, RecordId, Timestamp, parse_amount};
use mascate_marketing::QualitySync;
use rust_decimal::Decimal;

use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE, product_choice};
use crate::connections::AppConnections;
use crate::drafts::{self, DraftPublished, DraftsSection};
use crate::forms::{Outcome, Picker, amount_text, input, notice, percent_text, picker, refill};
use crate::kit;
use crate::layout;
use crate::mercado_livre;
use crate::offers::OpenProduct;
use crate::parts::ScreenParts;
use crate::pricing;
use crate::quality;
use crate::stock_mirror;

/// The app's Listings; absent when the database did not open.
pub struct AppListings(pub Arc<Listings>);

impl Global for AppListings {}

pub(crate) fn listings(cx: &App) -> Option<Arc<Listings>> {
    cx.try_global::<AppListings>().map(|app| app.0.clone())
}

/// What a listing sells, as the screens that name it by Product see it.
pub(crate) struct ListingProduct {
    /// The Product a variation is linked to, the first one found.
    pub product: Option<RecordId>,
    /// That Product's name, else the listing's title.
    pub name: String,
    /// Whether any variation is still active or paused.
    pub open: bool,
}

/// What each listing sells, by the channel's id.
pub(crate) async fn listing_products(
    listings: &Listings,
    catalog: &Catalog,
) -> Result<std::collections::BTreeMap<String, ListingProduct>, String> {
    let products: std::collections::BTreeMap<RecordId, String> = catalog
        .products()
        .await
        .map_err(|e| catalog::failure(&e))?
        .into_iter()
        .map(|product| (product.id, product.name))
        .collect();
    let mut sold = std::collections::BTreeMap::new();
    for listing in listings.listings().await.map_err(|e| failure(&e))? {
        let open = listing.listed.status != ListingStatus::Closed;
        let linked = listing
            .product
            .and_then(|product| Some((product, products.get(&product)?.clone())));
        let entry = sold
            .entry(listing.listed.id)
            .or_insert_with(|| ListingProduct {
                product: None,
                name: listing.listed.title,
                open: false,
            });
        entry.open |= open;
        if let (None, Some((product, name))) = (entry.product, linked) {
            entry.product = Some(product);
            entry.name = name;
        }
    }
    Ok(sold)
}

/// The step a listing to link is in, below it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    PickProduct,
    CreateProduct,
}

/// A listing's price, open below its row.
struct PricePanel {
    listing: RecordId,
    loading: bool,
    /// Today's sale, where the simulator starts, or why there is none.
    today: Option<Result<PriceScenario, String>>,
    /// The Price Suggestion, or why there is none.
    suggestion: Option<Result<PriceSuggestion, String>>,
}

/// The simulator's fields, one per part of the sale.
struct Simulator {
    price: Entity<InputState>,
    discount: Entity<InputState>,
    fee_rate: Entity<InputState>,
    fee_fixed: Entity<InputState>,
    shipping: Entity<InputState>,
    ads: Entity<InputState>,
    tax: Entity<InputState>,
    cost: Entity<InputState>,
}

impl Simulator {
    fn new(window: &mut Window, cx: &mut Context<ListingsScreen>) -> Self {
        Self {
            price: input("0,00", window, cx),
            discount: input("0", window, cx),
            fee_rate: input("0", window, cx),
            fee_fixed: input("0,00", window, cx),
            shipping: input("0,00", window, cx),
            ads: input("0,00", window, cx),
            tax: input("0", window, cx),
            cost: input("0,00", window, cx),
        }
    }

    fn fields(&self) -> [&Entity<InputState>; 8] {
        [
            &self.price,
            &self.discount,
            &self.fee_rate,
            &self.fee_fixed,
            &self.shipping,
            &self.ads,
            &self.tax,
            &self.cost,
        ]
    }

    fn fill(&self, sale: &PriceScenario, window: &mut Window, cx: &mut App) {
        let values = [
            amount_text(sale.price),
            percent_text(sale.discount),
            percent_text(sale.sale_fee.rate),
            amount_text(sale.sale_fee.fixed),
            amount_text(sale.shipping),
            amount_text(sale.ads),
            percent_text(sale.tax),
            amount_text(sale.cost),
        ];
        for (field, value) in self.fields().into_iter().zip(values) {
            field.update(cx, |input, cx| input.set_value(value, window, cx));
        }
    }

    /// The sale as typed, in the currency of `like`; `None` while a field
    /// is not a number.
    fn read(&self, like: &PriceScenario, cx: &App) -> Option<PriceScenario> {
        let currency = like.price.currency();
        let money = |field: &Entity<InputState>| {
            parse_amount(&field.read(cx).value())
                .filter(|amount| !amount.is_sign_negative())
                .map(|amount| Money::new(amount, currency))
        };
        let rate = |field: &Entity<InputState>| Percentage::parse(&field.read(cx).value());
        Some(PriceScenario {
            price: money(&self.price)?,
            discount: rate(&self.discount)?,
            sale_fee: SaleFee {
                rate: rate(&self.fee_rate)?,
                fixed: money(&self.fee_fixed)?,
            },
            shipping: money(&self.shipping)?,
            ads: money(&self.ads)?,
            tax: rate(&self.tax)?,
            cost: money(&self.cost)?,
        })
    }
}

/// Everything the screen shows, read in one go off the UI thread.
struct Snapshot {
    listings: Vec<Listing>,
    mirrored: Vec<MirroredStock>,
    to_link: Vec<ListingToLink>,
    products: Vec<Product>,
    last_sync: Option<Timestamp>,
    connection: ConnectionState,
}

pub struct ListingsScreen {
    listings: Vec<Listing>,
    /// How each listing's stock follows the app's, by listing.
    mirrored: Vec<MirroredStock>,
    to_link: Vec<ListingToLink>,
    products: Vec<Product>,
    last_sync: Option<Timestamp>,
    connection: Option<ConnectionState>,
    /// The listing to link whose row is open, and for what.
    step: Option<(RecordId, Step)>,
    product_pick: Picker,
    product_name: Entity<InputState>,
    product_sku: Entity<InputState>,
    busy: bool,
    syncing: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    /// The outcome of the last action on the whole screen.
    outcome: Option<Outcome>,
    /// The outcome of the last step on a row.
    row_outcome: Option<Outcome>,
    price: Option<PricePanel>,
    simulator: Simulator,
    drafts: Entity<DraftsSection>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OpenProduct> for ListingsScreen {}

impl ListingsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let simulator = Simulator::new(window, cx);
        // The simulator's margin follows every keystroke.
        let drafts = cx.new(|cx| DraftsSection::new(window, cx));
        let mut subscriptions: Vec<Subscription> = simulator
            .fields()
            .into_iter()
            .map(|field| {
                cx.subscribe(field, |_, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                })
            })
            .collect();
        // A published draft is one more listing.
        subscriptions.push(cx.subscribe_in(
            &drafts,
            window,
            |this, _, _: &DraftPublished, window, cx| this.refresh(window, cx),
        ));
        let mut screen = Self {
            listings: Vec::new(),
            mirrored: Vec::new(),
            to_link: Vec::new(),
            products: Vec::new(),
            last_sync: None,
            connection: None,
            step: None,
            product_pick: picker(window, cx),
            product_name: input("Nome do produto", window, cx),
            product_sku: input("SKU", window, cx),
            busy: false,
            syncing: false,
            reads: 0,
            outcome: None,
            row_outcome: None,
            price: None,
            simulator,
            drafts,
            _subscriptions: subscriptions,
        };
        screen.refresh(window, cx);
        screen
    }

    /// Reads everything again, as when the screen comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.drafts
            .update(cx, |drafts, cx| drafts.refresh(window, cx));
        self.run(window, cx, |_, _| async { Ok(None) }, |_, _: (), _, _| {});
    }

    /// Starts a draft of `product`, asked from its sheet in Produtos.
    pub fn start_draft(&mut self, product: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.drafts
            .update(cx, |drafts, cx| drafts.start(product, window, cx));
    }

    /// Runs `change` off the UI thread, then reads the screen's data again
    /// and hands what the change returned to `then`.
    fn run<T, F, Fut>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: F,
        then: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) where
        T: Send + 'static,
        F: FnOnce(Arc<Listings>, Arc<Catalog>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<T>, String>> + Send,
    {
        let (Some(listings), Some(catalog), Some(mirror)) =
            (listings(cx), catalog::catalog(cx), stock_mirror::mirror(cx))
        else {
            return;
        };
        let connections = cx.global::<AppConnections>().0.clone();
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(listings.clone(), catalog.clone()).await;
            let read = async {
                let products = catalog.products().await.map_err(|e| catalog::failure(&e))?;
                let known: Vec<CatalogProduct> = products.iter().map(catalog_product).collect();
                Ok::<_, String>(Snapshot {
                    listings: listings.listings().await.map_err(|e| failure(&e))?,
                    mirrored: mirror.mirrored().await.map_err(|e| failure(&e))?,
                    to_link: listings.to_link(&known).await.map_err(|e| failure(&e))?,
                    last_sync: listings.last_sync().await.map_err(|e| failure(&e))?,
                    connection: connections.state(Connection::MercadoLivre),
                    products,
                })
            }
            .await;
            (changed, read)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, read_back) = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads == read {
                    this.busy = false;
                    this.syncing = false;
                    match read_back {
                        Ok(snapshot) => this.show(snapshot, window, cx),
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

    /// Shows a failure where the owner acted: the open row, or the screen.
    fn failed(&mut self, error: String) {
        let text = Outcome::Failed(error.into());
        if self.step.is_some() || self.price.is_some() {
            self.row_outcome = Some(text);
        } else {
            self.outcome = Some(text);
        }
    }

    fn show(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        refill(
            &self.product_pick,
            snapshot.products.iter().map(product_choice).collect(),
            window,
            cx,
        );
        if self.step.is_some_and(|(open, _)| {
            !snapshot
                .to_link
                .iter()
                .any(|entry| entry.listing.id == open)
        }) {
            self.step = None;
        }
        if self.price.as_ref().is_some_and(|panel| {
            !snapshot
                .listings
                .iter()
                .any(|listing| listing.id == panel.listing && listing.product.is_some())
        }) {
            self.price = None;
        }
        self.listings = snapshot.listings;
        self.mirrored = snapshot.mirrored;
        self.to_link = snapshot.to_link;
        self.products = snapshot.products;
        self.last_sync = snapshot.last_sync;
        self.connection = Some(snapshot.connection);
    }

    fn connected(&self) -> bool {
        self.connection == Some(ConnectionState::Connected)
    }

    fn product(&self, id: RecordId) -> Option<&Product> {
        self.products.iter().find(|product| product.id == id)
    }

    /// Reads the listings, sends the stock still to send, then reads the
    /// quality of each listing (#23).
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(channel), Some(mirror), Some(rated)) = (
            mercado_livre::adapter(cx),
            stock_mirror::mirror(cx),
            quality::quality(cx),
        ) else {
            return;
        };
        self.outcome = None;
        self.step = None;
        self.price = None;
        self.syncing = true;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                let report = listings
                    .sync(channel.as_ref())
                    .await
                    .map_err(|e| failure(&e))?;
                let sent = mirror
                    .send(channel.as_ref())
                    .await
                    .map_err(|e| failure(&e))?;
                let rated = quality::sync(&listings, &rated, channel.as_ref()).await;
                Ok(Some((report, sent, rated)))
            },
            |this,
             (report, sent, rated): (ListingSync, StockSend, Result<QualitySync, String>),
             _,
             _| {
                let mut text = synced(&report);
                text.push_str(&stock_sent(&sent));
                this.outcome = Some(match rated {
                    Ok(rated) => {
                        text.push(' ');
                        text.push_str(&quality::synced(&rated));
                        if rated.pending > 0 {
                            text.push_str(" Veja em Qualidade.");
                        }
                        Outcome::Done(text.into())
                    }
                    Err(error) => {
                        text.push_str(&format!(" Não li a qualidade dos anúncios: {error}"));
                        Outcome::Failed(text.into())
                    }
                });
            },
        );
    }

    /// Sends the stock in the queue now.
    fn send_stock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(channel), Some(mirror)) = (mercado_livre::adapter(cx), stock_mirror::mirror(cx))
        else {
            return;
        };
        self.outcome = None;
        self.run(
            window,
            cx,
            move |_, _| async move {
                mirror
                    .send(channel.as_ref())
                    .await
                    .map(Some)
                    .map_err(|e| failure(&e))
            },
            |this, sent: StockSend, _, _| {
                this.outcome = Some(match sent.failed {
                    0 => Outcome::Done(stock_sent(&sent).trim().to_owned().into()),
                    _ => Outcome::Failed(stock_sent(&sent).trim().to_owned().into()),
                });
            },
        );
    }

    /// Pauses or reactivates the listing in Mercado Livre, every variation
    /// of it included: only ever on the owner's click.
    fn change_status(
        &mut self,
        listing: RecordId,
        pause: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(channel) = mercado_livre::adapter(cx) else {
            return;
        };
        self.outcome = None;
        self.step = None;
        self.price = None;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                let changed = if pause {
                    listings.pause(listing, channel.as_ref()).await
                } else {
                    listings.reactivate(listing, channel.as_ref()).await
                };
                changed.map(Some).map_err(|e| failure(&e))
            },
            move |this, changed: Vec<Listing>, _, _| {
                let what = if pause { "pausado" } else { "reativado" };
                this.outcome = Some(Outcome::Done(
                    match changed.len() {
                        0 | 1 => format!("Anúncio {what} no Mercado Livre."),
                        count => format!(
                            "Anúncio {what} no Mercado Livre, com as {count} variações dele."
                        ),
                    }
                    .into(),
                ));
            },
        );
    }

    fn open_step(
        &mut self,
        listing: RecordId,
        step: Step,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step = Some((listing, step));
        self.price = None;
        self.row_outcome = None;
        self.outcome = None;
        let Some(entry) = self
            .to_link
            .iter()
            .find(|entry| entry.listing.id == listing)
        else {
            return;
        };
        match step {
            Step::PickProduct => {
                let suggested = entry.suggestion.map(|suggestion| suggestion.product);
                self.product_pick.update(cx, |select, cx| match suggested {
                    Some(product) => select.set_selected_value(&product, window, cx),
                    None => select.set_selected_index(None, window, cx),
                });
            }
            Step::CreateProduct => {
                let listed = &entry.listing.listed;
                let name = match &listed.variation {
                    Some(variation) => format!("{} ({})", listed.title, variation.name),
                    None => listed.title.clone(),
                };
                let seller_sku = listed.seller_sku.clone();
                self.product_name
                    .update(cx, |input, cx| input.set_value(name.clone(), window, cx));
                self.product_sku.update(cx, |input, cx| {
                    input.set_value(seller_sku.clone().unwrap_or_default(), window, cx)
                });
                // The seller SKU already names the Product in the channel;
                // without one, the catalog suggests a free SKU.
                if seller_sku.is_none() {
                    self.run(
                        window,
                        cx,
                        move |_, catalog| async move {
                            catalog
                                .suggest_sku(&name)
                                .await
                                .map(Some)
                                .map_err(|e| catalog::failure(&e))
                        },
                        |this, sku: mascate_catalog::Sku, window, cx| {
                            this.product_sku.update(cx, |input, cx| {
                                input.set_value(sku.to_string(), window, cx)
                            });
                        },
                    );
                }
            }
        }
        cx.notify();
    }

    fn close_step(&mut self, cx: &mut Context<Self>) {
        self.step = None;
        self.row_outcome = None;
        cx.notify();
    }

    fn link(
        &mut self,
        listing: RecordId,
        product: RecordId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .product(product)
            .map(|product| format!("{} · {}", product.sku, product.name))
            .unwrap_or_default();
        self.row_outcome = None;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                listings
                    .link(listing, product)
                    .await
                    .map(Some)
                    .map_err(|e| failure(&e))
            },
            move |this, _: Listing, _, _| {
                this.step = None;
                this.outcome = Some(Outcome::Done(format!("Anúncio vinculado a {name}.").into()));
            },
        );
    }

    fn link_picked(&mut self, listing: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        match self.product_pick.read(cx).selected_value().copied() {
            Some(product) => self.link(listing, product, window, cx),
            None => {
                self.row_outcome = Some(Outcome::Failed("Escolha o produto.".into()));
                cx.notify();
            }
        }
    }

    /// A new Product, with its folder, linked to the listing it came from.
    fn create_product(&mut self, listing: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.product_name.read(cx).value().to_string();
        let sku = self.product_sku.read(cx).value().to_string();
        self.row_outcome = None;
        self.run(
            window,
            cx,
            move |listings, catalog| async move {
                let product = catalog
                    .add_product(&name, &sku)
                    .await
                    .map_err(|e| catalog::failure(&e))?;
                listings.link(listing, product.id).await.map_err(|e| {
                    format!(
                        "O produto {} foi criado, mas não consegui vinculá-lo ao anúncio: {}",
                        product.sku,
                        failure(&e)
                    )
                })?;
                Ok(Some(product))
            },
            |this, product: Product, _, _| {
                this.step = None;
                this.outcome = Some(Outcome::Done(
                    format!(
                        "Produto {} criado, com a pasta para os arquivos dele, e vinculado ao anúncio.",
                        product.sku
                    )
                    .into(),
                ));
            },
        );
    }

    fn unlink(&mut self, listing: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.outcome = None;
        self.step = None;
        self.price = None;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                listings
                    .unlink(listing)
                    .await
                    .map(Some)
                    .map_err(|e| failure(&e))
            },
            |this, _: Listing, _, _| {
                this.outcome = Some(Outcome::Done(
                    "Anúncio desvinculado do produto; ele voltou para a fila a vincular.".into(),
                ));
            },
        );
    }

    /// Opens the listing's price below its row: today's sale, with the
    /// channel's fee at today's price, in the simulator, and the Price
    /// Suggestion. Nothing is sent to the channel.
    fn open_price(&mut self, listing: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(channel), Some(pricing), Some(taxes), Some(catalog)) = (
            mercado_livre::adapter(cx),
            pricing::pricing(cx),
            pricing::taxes(cx),
            catalog::catalog(cx),
        ) else {
            return;
        };
        self.step = None;
        self.outcome = None;
        self.row_outcome = None;
        self.price = Some(PricePanel {
            listing,
            loading: true,
            today: None,
            suggestion: None,
        });
        let reading = cx.background_executor().spawn(async move {
            let assumptions = PriceAssumptions {
                tax: taxes.rate().await.map_err(|error| {
                    format!("Não consegui ler o imposto nas configurações: {error}")
                })?,
                estimated_shipping: catalog
                    .discovery_settings()
                    .await
                    .map_err(|error| catalog::failure(&error))?
                    .estimated_shipping,
            };
            let today = pricing.today(listing, channel.as_ref(), assumptions).await;
            let suggestion = match today {
                Ok(_) => Some(
                    pricing
                        .suggest(listing, channel.as_ref(), assumptions)
                        .await,
                ),
                Err(_) => None,
            };
            Ok::<_, String>((today, suggestion))
        });
        cx.spawn_in(window, async move |this, cx| {
            let read = reading.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(panel) = this.price.as_mut().filter(|panel| panel.listing == listing)
                else {
                    return;
                };
                panel.loading = false;
                match read {
                    Ok((today, suggestion)) => {
                        if let Ok(sale) = &today {
                            this.simulator.fill(sale, window, cx);
                        }
                        let products = &this.products;
                        let why = |error: PricingError| price_failure(&error, listing, products);
                        let panel = this.price.as_mut().expect("still open");
                        panel.today = Some(today.map_err(why));
                        panel.suggestion = suggestion.map(|found| found.map_err(why));
                    }
                    Err(error) => panel.today = Some(Err(error)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn close_price(&mut self, cx: &mut Context<Self>) {
        self.price = None;
        self.row_outcome = None;
        cx.notify();
    }

    /// Takes the suggested sale into the simulator.
    fn simulate_suggestion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Ok(suggestion)) = self.price.as_ref().and_then(|p| p.suggestion.as_ref()) {
            self.simulator.fill(&suggestion.suggested, window, cx);
        }
        cx.notify();
    }

    /// Sends `price` to Mercado Livre for the listing and its variations:
    /// only ever on the owner's click.
    fn send_price(
        &mut self,
        listing: RecordId,
        price: Money,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(channel) = mercado_livre::adapter(cx) else {
            return;
        };
        self.row_outcome = None;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                listings
                    .change_price(listing, price, channel.as_ref())
                    .await
                    .map(Some)
                    .map_err(|e| failure(&e))
            },
            move |this, changed: Vec<Listing>, _, _| {
                this.price = None;
                let price = price.rounded().to_pt_br();
                this.outcome = Some(Outcome::Done(
                    match changed.len() {
                        0 | 1 => format!("Preço de {price} enviado ao Mercado Livre."),
                        count => format!(
                            "Preço de {price} enviado ao Mercado Livre para as {count} variações \
                             do anúncio."
                        ),
                    }
                    .into(),
                ));
            },
        );
    }

    fn send_simulated(&mut self, listing: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let like = self
            .price
            .as_ref()
            .and_then(|panel| panel.today.as_ref())
            .and_then(|today| today.as_ref().ok())
            .copied();
        match like.and_then(|like| self.simulator.read(&like, cx)) {
            Some(sale) if sale.price.rounded().amount() > Decimal::ZERO => {
                self.send_price(listing, sale.price, window, cx)
            }
            _ => {
                self.row_outcome = Some(Outcome::Failed(
                    "Digite um preço maior que zero e números em todos os campos.".into(),
                ));
                cx.notify();
            }
        }
    }

    fn render_price(&self, panel: &PricePanel, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let listing = panel.listing;
        let mut body = v_flex().gap_3();
        if panel.loading {
            body =
                body.child(div().text_sm().text_color(t.text2).child(
                    "Consultando a tarifa do Mercado Livre para o preço de hoje e o sugerido…",
                ));
        }
        if let Some(Err(error)) = &panel.today {
            body = body.child(kit::error_notice(error.clone(), cx));
        }
        match &panel.suggestion {
            Some(Ok(suggestion)) => {
                let product = self
                    .product(suggestion.product)
                    .map(|product| product.sku.to_string())
                    .unwrap_or_default();
                let target = format!(
                    "margem alvo de {} ({}) para {product}",
                    suggestion.target.margin.to_pt_br(),
                    if suggestion.target.own {
                        "do produto"
                    } else {
                        "padrão"
                    }
                );
                let today = suggestion.current.breakdown().ok().map(|today| {
                    format!(
                        "Hoje: {} com margem de {} ({}).",
                        suggestion.current.price.to_pt_br(),
                        today.margin.amount.to_pt_br(),
                        today.margin.percent_to_pt_br()
                    )
                });
                let shared = (suggestion.listings.len() > 1).then(|| {
                    format!(
                        "O preço vale para as {} variações do anúncio; sugerido pelo produto que \
                         precisa do preço mais alto.",
                        suggestion.listings.len()
                    )
                });
                let price = suggestion.price;
                body = body.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_baseline()
                                .child(div().text_sm().text_color(t.text2).child("Preço sugerido"))
                                .child(div().text_xl().font_semibold().child(price.to_pt_br()))
                                .child(div().text_sm().text_color(t.text2).child(target)),
                        )
                        .children(
                            suggestion
                                .suggested
                                .breakdown()
                                .ok()
                                .map(|sale| breakdown_line(&sale, cx)),
                        )
                        .children(
                            today
                                .into_iter()
                                .chain(shared)
                                .map(|text| div().text_xs().text_color(t.text2).child(text)),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .flex_wrap()
                                .pt_1()
                                .child(
                                    Button::new("approve-price")
                                        .label(format!(
                                            "Aprovar e enviar {} ao ML",
                                            price.to_pt_br()
                                        ))
                                        .icon(IconName::Check)
                                        .primary()
                                        .small()
                                        .loading(self.busy)
                                        .disabled(self.busy)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.send_price(listing, price, window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("simulate-suggestion")
                                        .label("Levar ao simulador")
                                        .outline()
                                        .small()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.simulate_suggestion(window, cx)
                                        })),
                                ),
                        ),
                );
            }
            Some(Err(error)) => {
                body = body.child(kit::info_notice(
                    format!("Sem sugestão de preço. {error}"),
                    cx,
                ));
            }
            None => {}
        }
        if let Some(Ok(like)) = &panel.today {
            body = body.child(self.render_simulator(listing, like, cx));
        }
        v_flex()
            .gap_2()
            .pt_3()
            .border_t(t.border_width)
            .border_color(t.frame)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(div().font_medium().child("Preço"))
                    .child(
                        Button::new("close-price")
                            .label("Fechar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.close_price(cx))),
                    ),
            )
            .child(body)
            .children(self.row_outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .into_any_element()
    }

    fn render_simulator(
        &self,
        listing: RecordId,
        like: &PriceScenario,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .w(px(120.))
                .child(div().text_xs().font_medium().child(label))
                .child(Input::new(input).small())
        };
        let s = &self.simulator;
        let simulated = s.read(like, cx);
        let result = match simulated.as_ref().map(PriceScenario::breakdown) {
            Some(Ok(sale)) => {
                let ink = if sale.margin.amount.is_negative() {
                    t.danger
                } else {
                    t.success
                };
                v_flex()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .child(div().text_sm().text_color(t.text2).child("Margem"))
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .text_color(ink)
                                    .child(sale.margin.amount.to_pt_br()),
                            )
                            .child(
                                div()
                                    .font_medium()
                                    .text_color(ink)
                                    .child(sale.margin.percent_to_pt_br()),
                            ),
                    )
                    .child(breakdown_line(&sale, cx))
            }
            _ => v_flex().child(div().text_sm().text_color(t.danger).child(
                "Digite números em todos os campos: valores como 89,90 e porcentagens de 0 a 100.",
            )),
        };
        let send = simulated.map(|sale| sale.price.rounded());
        v_flex()
            .gap_2()
            .child(div().font_medium().child("Simulador"))
            .child(div().text_xs().text_color(t.text2).child(
                "Começa com a venda de hoje: tarifa do Mercado Livre no preço de hoje, frete \
                 estimado a partir de R$ 79,00, imposto das configurações e custo médio do \
                 estoque. Mexa em qualquer parte para ver a margem; a tarifa não é consultada \
                 de novo.",
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(field("Preço (R$)", &s.price))
                    .child(field("Desconto (%)", &s.discount))
                    .child(field("Tarifa (%)", &s.fee_rate))
                    .child(field("Tarifa fixa (R$)", &s.fee_fixed))
                    .child(field("Frete (R$)", &s.shipping))
                    .child(field("Ads por venda (R$)", &s.ads))
                    .child(field("Imposto (%)", &s.tax))
                    .child(field("Custo (R$)", &s.cost)),
            )
            .child(result)
            .child(
                h_flex().child(
                    Button::new("send-simulated")
                        .label(match send {
                            Some(price) => format!("Enviar {} ao ML", price.to_pt_br()),
                            None => "Enviar ao ML".into(),
                        })
                        .outline()
                        .small()
                        .disabled(self.busy || send.is_none())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.send_simulated(listing, window, cx)
                        })),
                ),
            )
            .into_any_element()
    }

    fn stock_of(&self, listing: RecordId) -> Option<&MirroredStock> {
        self.mirrored.iter().find(|stock| stock.listing == listing)
    }

    /// The stock still to send, why it did not go yet, and when the last
    /// one did.
    fn render_stock(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = look(cx).tokens;
        if self.mirrored.iter().all(|stock| stock.on_hand.is_none()) {
            return None;
        }
        let queue: Vec<(&Listing, &MirroredStock)> = self
            .listings
            .iter()
            .filter_map(|listing| Some((listing, self.stock_of(listing.id)?)))
            .filter(|(_, stock)| stock.to_send.is_some())
            .collect();
        let last_sent = self
            .mirrored
            .iter()
            .filter_map(|stock| stock.last_sent)
            .map(|sent| sent.at)
            .max();
        let heading = h_flex()
            .gap_2()
            .items_baseline()
            .child(kit::section_heading(match queue.len() {
                0 => "Estoque em dia".to_owned(),
                count => format!("Estoque a enviar ({count})"),
            }))
            .child(
                div()
                    .text_xs()
                    .text_color(t.text2)
                    .child(match (queue.is_empty(), last_sent) {
                        (false, _) => "o saldo destes produtos mudou no app; vai ao Mercado Livre \
                                       agora ou no próximo Sync, e o que falhar fica aqui"
                            .to_owned(),
                        (true, Some(at)) => format!(
                            "o estoque de cada anúncio vinculado segue o saldo do app · último \
                             envio: {}",
                            catalog::day_and_time(at)
                        ),
                        (true, None) => {
                            "o estoque de cada anúncio vinculado segue o saldo do app".to_owned()
                        }
                    }),
            );
        let mut section = v_flex().gap_2().child(
            h_flex()
                .gap_3()
                .items_center()
                .justify_between()
                .child(heading)
                .when(!queue.is_empty(), |row| {
                    row.child(
                        Button::new("send-stock")
                            .label("Enviar agora")
                            .icon(IconName::ArrowUp)
                            .outline()
                            .small()
                            .disabled(self.busy || !self.connected())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.send_stock(window, cx)),
                            ),
                    )
                }),
        );
        for (listing, stock) in queue {
            let listed = &listing.listed;
            let title = match &listed.variation {
                Some(variation) => format!("{} · {}", listed.title, variation.name),
                None => listed.title.clone(),
            };
            let change = format!(
                "no ML: {} → no app: {}",
                listed.available_quantity,
                stock.to_send.unwrap_or_default()
            );
            section = section.child(
                v_flex()
                    .gap_0p5()
                    .px_3()
                    .py_2()
                    .rounded(t.radius_lg)
                    .border(t.border_width)
                    .border_color(if stock.last_failure.is_some() {
                        t.danger
                    } else {
                        t.frame
                    })
                    .bg(t.surface)
                    .child(
                        h_flex()
                            .gap_3()
                            .justify_between()
                            .child(div().min_w_0().truncate().text_sm().child(title))
                            .child(div().flex_none().text_sm().font_medium().child(change)),
                    )
                    .children(stock.last_failure.as_ref().map(|failed| {
                        div().text_xs().text_color(t.danger).child(format!(
                            "Não foi em {}: {}",
                            catalog::day_and_time(failed.at),
                            mercado_livre::failure(&failed.error)
                        ))
                    })),
            );
        }
        Some(section.into_any_element())
    }

    fn render_sync(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let when = match self.last_sync {
            Some(at) => format!("Último Sync: {}", catalog::day_and_time(at)),
            None => "Nenhum Sync ainda.".into(),
        };
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_xs().text_color(t.text2).child(when))
            .child(
                Button::new("sync-listings")
                    .label("Sincronizar")
                    .icon(IconName::RefreshCw)
                    .primary()
                    .small()
                    .loading(self.syncing)
                    .disabled(self.busy || !self.connected())
                    .on_click(cx.listener(|this, _, window, cx| this.sync(window, cx))),
            )
            .into_any_element()
    }

    /// Title, variation and the facts the channel reports, with the link.
    fn summary(&self, id: &str, listing: &Listing, cx: &App) -> gpui_kit::Div {
        let t = look(cx).tokens;
        let listed = &listing.listed;
        let mut facts = vec![
            listed.id.clone(),
            listed.price.to_pt_br(),
            stock_text(listed.available_quantity),
        ];
        facts.extend(listed.listing_type.map(|kind| kind.name().to_owned()));
        facts.extend(
            listed
                .seller_sku
                .as_ref()
                .map(|sku| format!("SKU no ML: {sku}")),
        );
        v_flex()
            .min_w_0()
            .flex_1()
            .gap_0p5()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .min_w_0()
                            .font_medium()
                            .truncate()
                            .child(listed.title.clone()),
                    )
                    .child(kit::tag(
                        status_name(listed.status),
                        status_ink(listed.status, &t),
                        cx,
                    )),
            )
            .children(
                listed
                    .variation
                    .as_ref()
                    .map(|variation| div().text_sm().child(variation.name.clone())),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_x_2()
                    .text_xs()
                    .text_color(t.text2)
                    .child(facts.join(" · "))
                    .children(listed.link.clone().map(|link| {
                        div()
                            .id(gpui_kit::SharedString::from(format!("open-{id}")))
                            .flex()
                            .gap_1()
                            .items_center()
                            .text_color(t.accent_text)
                            .cursor_pointer()
                            .child("Abrir no Mercado Livre")
                            .child(
                                gpui_kit::component::Icon::new(IconName::ExternalLink)
                                    .size(px(12.)),
                            )
                            .on_click(move |_, _, cx| cx.open_url(&link))
                    })),
            )
    }

    fn render_to_link(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let entry = &self.to_link[index];
        let id = entry.listing.id;
        let step = self
            .step
            .filter(|(open, _)| *open == id)
            .map(|(_, step)| step);
        let suggested = entry
            .suggestion
            .and_then(|suggestion| Some((suggestion, self.product(suggestion.product)?)));

        let suggestion = suggested.map(|(suggestion, product)| {
            let product_id = product.id;
            h_flex()
                .flex_wrap()
                .gap_2()
                .items_center()
                .text_sm()
                .child(div().text_color(t.text2).child("Sugestão:"))
                .child(kit::tag(product.sku.to_string(), t.accent_text, cx))
                .child(div().child(product.name.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text2)
                        .child(match suggestion.by {
                            SuggestedBy::SellerSku => "pelo SKU do anúncio",
                            SuggestedBy::Title => "pelo título",
                        }),
                )
                .when(step.is_none(), |row| {
                    row.child(
                        Button::new(("confirm-suggestion", index))
                            .label("Confirmar")
                            .icon(IconName::Check)
                            .primary()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.link(id, product_id, window, cx)
                            })),
                    )
                })
        });

        let actions = h_flex()
            .gap_2()
            .flex_wrap()
            .when(suggested.is_none(), |row| {
                row.child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("Sem sugestão de produto."),
                )
            })
            .child(
                Button::new(("pick-product", index))
                    .label(if suggested.is_some() {
                        "Escolher outro"
                    } else {
                        "Escolher produto"
                    })
                    .outline()
                    .small()
                    .disabled(self.busy || self.products.is_empty())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_step(id, Step::PickProduct, window, cx)
                    })),
            )
            .child(
                Button::new(("create-product", index))
                    .label("Criar produto")
                    .outline()
                    .small()
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_step(id, Step::CreateProduct, window, cx)
                    })),
            );

        let cancel = Button::new(("cancel-step", index))
            .label("Cancelar")
            .ghost()
            .small()
            .on_click(cx.listener(|this, _, _, cx| this.close_step(cx)));
        let step_form = step.map(|step| {
            let row = h_flex().flex_wrap().gap_2().items_end();
            match step {
                Step::PickProduct => row
                    .child(
                        div().w(px(320.)).child(
                            Select::new(&self.product_pick)
                                .search_placeholder("Buscar")
                                .small()
                                .placeholder("Produto"),
                        ),
                    )
                    .child(
                        Button::new(("confirm-link", index))
                            .label("Vincular")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.link_picked(id, window, cx)
                            })),
                    )
                    .child(cancel),
                Step::CreateProduct => row
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(320.))
                            .child(div().text_sm().font_medium().child("Nome do produto"))
                            .child(Input::new(&self.product_name).small()),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(200.))
                            .child(div().text_sm().font_medium().child("SKU"))
                            .child(Input::new(&self.product_sku).small()),
                    )
                    .child(
                        Button::new(("confirm-create", index))
                            .label("Criar e vincular")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.create_product(id, window, cx)
                            })),
                    )
                    .child(cancel),
            }
        });

        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if step.is_some() {
                t.accent_edge
            } else {
                t.frame
            })
            .bg(t.surface)
            .child(self.summary(&format!("to-link-{index}"), &entry.listing, cx))
            .children(suggestion)
            .child(actions)
            .children(step_form)
            .when(step.is_some(), |card| {
                card.children(self.row_outcome.as_ref().map(|outcome| notice(outcome, cx)))
            })
            .into_any_element()
    }

    fn render_listing(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let listing = &self.listings[index];
        let id = listing.id;
        let product = listing.product.map(|id| (id, self.product(id)));
        let linked = h_flex()
            .flex_none()
            .gap_2()
            .items_center()
            .map(|row| match product {
                Some((product_id, found)) => row
                    .child(kit::tag(
                        found
                            .map(|product| product.sku.to_string())
                            .unwrap_or_else(|| "produto removido".into()),
                        t.accent_text,
                        cx,
                    ))
                    .when(found.is_some(), |row| {
                        row.child(
                            Button::new(("open-product", index))
                                .label("Abrir ficha")
                                .icon(IconName::ChevronRight)
                                .ghost()
                                .small()
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(OpenProduct(product_id));
                                })),
                        )
                    })
                    .child(
                        Button::new(("unlink", index))
                            .label("Desvincular")
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.unlink(id, window, cx)),
                            ),
                    )
                    .when(listing.listed.status != ListingStatus::Closed, |row| {
                        row.child(
                            Button::new(("price", index))
                                .label("Preço")
                                .outline()
                                .small()
                                .disabled(self.busy || !self.connected())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_price(id, window, cx)
                                })),
                        )
                    }),
                None => row.child(kit::tag("sem produto", t.text2, cx)),
            });
        let status = match listing.listed.status {
            ListingStatus::Active => Some(("Pausar", true)),
            ListingStatus::Paused => Some(("Reativar", false)),
            _ => None,
        };
        let status_button =
            status.map(|(label, pause)| {
                Button::new(("status", index))
                    .label(label)
                    .outline()
                    .small()
                    .disabled(self.busy || !self.connected())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.change_status(id, pause, window, cx)
                    }))
            });
        let stock_line = listing
            .product
            .filter(|_| status.is_some())
            .map(|_| stock_line(self.stock_of(id)))
            .map(|text| div().text_xs().text_color(t.text2).child(text));
        let panel = self
            .price
            .as_ref()
            .filter(|panel| panel.listing == id)
            .map(|panel| self.render_price(panel, cx));
        v_flex()
            .gap_3()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if panel.is_some() {
                t.accent_edge
            } else {
                t.frame
            })
            .bg(t.surface)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(self.summary(&format!("listing-{index}"), listing, cx))
                    .child(linked)
                    .children(status_button),
            )
            .children(stock_line)
            .children(panel)
            .into_any_element()
    }
}

impl Render for ListingsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut parts = ScreenParts::new("Anúncios");
        if listings(cx).is_none() || catalog::catalog(cx).is_none() {
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
        if self.step.is_none() && self.price.is_none() {
            parts
                .notices
                .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        }

        parts.content.push(self.drafts.clone().into_any_element());
        parts.content.extend(self.render_stock(cx));

        if !self.to_link.is_empty() {
            parts.content.push(
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .child(kit::section_heading(format!(
                                "A vincular ({})",
                                self.to_link.len()
                            )))
                            .child(div().text_xs().text_color(t.text2).child(
                                "anúncios sem produto: confirme a sugestão, escolha um produto \
                                 ou crie um a partir do anúncio",
                            )),
                    )
                    .children((0..self.to_link.len()).map(|index| self.render_to_link(index, cx)))
                    .into_any_element(),
            );
        }

        let heading = match self.listings.len() {
            0 => "Anúncios no Mercado Livre".to_owned(),
            1 => "1 anúncio no Mercado Livre".to_owned(),
            count => format!("{count} anúncios no Mercado Livre"),
        };
        parts.content.push(
            v_flex()
                .gap_3()
                .child(kit::section_heading(heading))
                .when(self.listings.is_empty(), |list| {
                    list.child(div().text_sm().text_color(t.text2).child(
                        "Nenhum anúncio ainda. Sincronize para trazer os anúncios ativos e \
                         pausados da sua conta; cada variação vira um anúncio próprio, vinculado ao \
                         produto que ela vende.",
                    ))
                })
                .children((0..self.listings.len()).map(|index| self.render_listing(index, cx)))
                .into_any_element(),
        );
        layout::screen(parts, cx)
    }
}

fn catalog_product(product: &Product) -> CatalogProduct {
    CatalogProduct {
        id: product.id,
        sku: product.sku.to_string(),
        name: product.name.clone(),
    }
}

fn status_name(status: ListingStatus) -> &'static str {
    match status {
        ListingStatus::Active => "Ativo",
        ListingStatus::Paused => "Pausado",
        ListingStatus::Closed => "Encerrado",
        ListingStatus::UnderReview => "Em revisão",
        ListingStatus::Inactive => "Inativo",
    }
}

fn status_ink(status: ListingStatus, t: &Tokens) -> Hsla {
    match status {
        ListingStatus::Active => t.success,
        ListingStatus::UnderReview | ListingStatus::Inactive => t.danger,
        ListingStatus::Paused | ListingStatus::Closed => t.text2,
    }
}

fn stock_text(units: u32) -> String {
    match units {
        0 => "sem estoque no ML".into(),
        1 => "1 em estoque no ML".into(),
        units => format!("{units} em estoque no ML"),
    }
}

/// How a listing's stock follows the app's, in one line.
fn stock_line(stock: Option<&MirroredStock>) -> String {
    let Some((stock, on_hand)) = stock.and_then(|stock| Some((stock, stock.on_hand?))) else {
        return "O estoque deste anúncio ainda não segue o app: o produto nunca entrou no \
                estoque. Ele passa a seguir no primeiro recebimento em Compras."
            .into();
    };
    match (stock.to_send, stock.last_sent) {
        (Some(_), _) => format!(
            "Segue o app: {} no app, na fila para enviar.",
            units(on_hand)
        ),
        (None, Some(sent)) => format!(
            "Segue o app: estoque de {} enviado ao Mercado Livre em {}.",
            units(on_hand),
            catalog::day_and_time(sent.at)
        ),
        (None, None) => format!("Segue o app: {}, igual ao Mercado Livre.", units(on_hand)),
    }
}

fn units(count: u32) -> String {
    match count {
        1 => "1 unidade".into(),
        count => format!("{count} unidades"),
    }
}

/// What a send of stock did, as a sentence to follow another, or nothing.
fn stock_sent(sent: &StockSend) -> String {
    let mut text = String::new();
    match sent.sent {
        0 => {}
        1 => text.push_str(" Estoque enviado a 1 anúncio."),
        count => text.push_str(&format!(" Estoque enviado a {count} anúncios.")),
    }
    match sent.failed {
        0 => {}
        1 => text.push_str(" 1 anúncio não recebeu o estoque e ficou na fila."),
        count => text.push_str(&format!(
            " {count} anúncios não receberam o estoque e ficaram na fila."
        )),
    }
    if text.is_empty() {
        text.push_str(" Nenhum estoque a enviar.");
    }
    text
}

/// What the Sync did, in one line.
fn synced(report: &ListingSync) -> String {
    let mut text = format!(
        "Sync concluído: {} anúncios lidos, {} novos e {} com mudanças.",
        report.read, report.new, report.changed
    );
    if report.gone > 0 {
        text.push_str(&format!(
            " {} que o Mercado Livre não tem mais ficaram como encerrados.",
            report.gone
        ));
    }
    text
}

pub(crate) fn failure(error: &ListingError) -> String {
    match error {
        ListingError::UnknownListing(_) => {
            "Esse anúncio não existe mais; a lista foi atualizada.".into()
        }
        ListingError::InvalidPrice => "O preço precisa ser maior que zero, em reais.".into(),
        ListingError::NotActive => "Só um anúncio ativo pode ser pausado; sincronize para ver o \
             status de agora."
            .into(),
        ListingError::NotPaused => "Só um anúncio pausado pode ser reativado; sincronize para ver \
             o status de agora."
            .into(),
        ListingError::NoStockToSell => "O anúncio está sem estoque no Mercado Livre e \
             continuaria pausado. Dê entrada no estoque em Compras: quando o estoque chega ao \
             anúncio, o Mercado Livre reativa sozinho o que pausou por falta de estoque."
            .into(),
        ListingError::Stock(_) => format!("Não consegui ler o estoque: {error}"),
        ListingError::Platform(error) => mercado_livre::failure(error),
        ListingError::Unreadable(_) | ListingError::Sql(_) => {
            format!("Não consegui ler ou gravar no banco: {error}")
        }
        ListingError::UnknownDraft(_)
        | ListingError::Published(_)
        | ListingError::Blocked(_)
        | ListingError::Picture { .. }
        | ListingError::DescriptionNotSent(..) => drafts::failure(error),
    }
}

/// "R$ 91,67 − tarifa R$ 12,83 − frete R$ 20,00 − …": where the money of a
/// sale goes.
fn breakdown_line(sale: &PriceBreakdown, cx: &App) -> gpui_kit::Div {
    let t = look(cx).tokens;
    div().text_xs().text_color(t.text2).child(format!(
        "{} − tarifa {} − frete {} − Ads {} − imposto {} − custo {} = margem {} ({})",
        sale.sale_price.to_pt_br(),
        sale.sale_fee.to_pt_br(),
        sale.shipping.to_pt_br(),
        sale.ads.to_pt_br(),
        sale.tax.to_pt_br(),
        sale.cost.to_pt_br(),
        sale.margin.amount.to_pt_br(),
        sale.margin.percent_to_pt_br(),
    ))
}

/// Why a listing has no price to work out, as the owner reads it.
fn price_failure(error: &PricingError, listing: RecordId, products: &[Product]) -> String {
    let sku = |id: RecordId| {
        products
            .iter()
            .find(|product| product.id == id)
            .map(|product| product.sku.to_string())
            .unwrap_or_else(|| "do anúncio".into())
    };
    match error {
        PricingError::Listing(error) => failure(error),
        PricingError::Closed => "O anúncio está encerrado no Mercado Livre.".into(),
        PricingError::NotLinked(id) if *id == listing => {
            "Vincule o anúncio a um produto para ter o custo.".into()
        }
        PricingError::NotLinked(_) => "Uma variação deste anúncio está sem produto; vincule-a \
             antes, porque o preço vale para todas as variações."
            .into(),
        PricingError::NoCost(product) => format!(
            "O produto {} ainda não entrou no estoque, então não tem custo médio. Registre o \
             recebimento em Compras.",
            sku(*product)
        ),
        PricingError::NoFeeBasis => "O Mercado Livre não informou a categoria ou o tipo do \
             anúncio; sincronize de novo."
            .into(),
        PricingError::Unreachable => "Nenhum preço chega à margem alvo: tarifa, imposto e \
             margem alvo somam 100% ou mais. Baixe a margem alvo na ficha do produto ou em \
             Configurações."
            .into(),
        PricingError::Unsettled => "A tarifa do Mercado Livre mudou a cada preço consultado; \
             use o simulador."
            .into(),
        PricingError::InvalidTarget => pricing::target_failure(error),
        PricingError::Currencies(_) => "O custo e o preço estão em moedas diferentes.".into(),
        PricingError::Platform(error) => mercado_livre::failure(error),
        PricingError::Stock(_) | PricingError::Unreadable(_) | PricingError::Sql(_) => {
            format!("Não consegui ler ou gravar no banco: {error}")
        }
    }
}
