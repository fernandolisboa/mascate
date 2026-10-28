//! Anúncios (#15): the owner's Mercado Livre listings, synced, and the
//! queue of the ones still without a Product: confirm the suggested one,
//! pick another, or make a Product from the listing. Each linked listing
//! opens its price (#19): the Price Suggestion from the target margin and
//! the price simulator. A price reaches Mercado Livre only when the owner
//! sends it. The stock of each linked listing follows the app's (#18): the
//! queue shows what is still to send and why it did not go, and each
//! listing can be paused and reactivated. A Mercado Livre listing can be
//! sold on order (#207, ADR 0038): the owner sets the days to be ready and
//! how many orders it takes at once, and its stock becomes the orders that
//! still fit.

use std::collections::{BTreeMap, BTreeSet};
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
    CatalogProduct, Listing, ListingAvailability, ListingError, ListingStatus, ListingSync,
    ListingToLink, Listings, MAX_AVAILABILITY_DAYS, MirroredStock, OnOrderListing, OnOrderTerms,
    OrderSlots, PriceBreakdown, PriceScenario, PriceSuggestion, PricingError, SaleFee,
    SalesChannel, SentPrice, SentPriceState, StockSend, SuggestedBy,
};
use mascate_integrations::ConnectionState;
use mascate_kernel::{
    Money, Percentage, PlatformError, RecordId, SalesChannelKind, Timestamp, parse_amount,
};
use rust_decimal::Decimal;

use crate::ad_groups;
use crate::ads;
use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE, product_choice};
use crate::drafts::{self, DraftPublished, DraftsSection};
use crate::forms::{Outcome, Picker, amount_text, input, notice, percent_text, picker, refill};
use crate::kit;
use crate::layout;
use crate::offers::OpenProduct;
use crate::orders;
use crate::parts::ScreenParts;
use crate::preferences;
use crate::pricing;
use crate::quality;
use crate::repricing::{self, RepricingPanel};
use crate::sales_channels::{self, ChannelFilter};
use crate::stock_mirror;
use crate::work_context;

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

/// The Listings on `kind`.
pub(crate) async fn channel_listings(
    listings: &Listings,
    kind: SalesChannelKind,
) -> Result<Vec<Listing>, String> {
    Ok(listings
        .listings()
        .await
        .map_err(|e| failure(&e))?
        .into_iter()
        .filter(|listing| listing.channel == kind)
        .collect())
}

/// What each Mercado Livre listing sells, by the channel's id: the only
/// ones questions and Reputation read, which only Mercado Livre offers by
/// API (ADR 0033).
pub(crate) async fn listing_products(
    listings: &Listings,
    catalog: &Catalog,
) -> Result<std::collections::BTreeMap<String, ListingProduct>, String> {
    listing_products_on(listings, catalog, SalesChannelKind::MercadoLivre).await
}

/// What each listing on `kind` sells, by the channel's id.
pub(crate) async fn listing_products_on(
    listings: &Listings,
    catalog: &Catalog,
    kind: SalesChannelKind,
) -> Result<std::collections::BTreeMap<String, ListingProduct>, String> {
    let products: std::collections::BTreeMap<RecordId, String> = catalog
        .products()
        .await
        .map_err(|e| catalog::failure(&e))?
        .into_iter()
        .map(|product| (product.id, product.name))
        .collect();
    let mut sold = std::collections::BTreeMap::new();
    for listing in channel_listings(listings, kind).await? {
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
    /// Its Automatic Repricing and price history (#135).
    repricing: Entity<RepricingPanel>,
    _repricing_changes: Subscription,
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
    on_order: Vec<OnOrderListing>,
    to_link: Vec<ListingToLink>,
    products: Vec<Product>,
    last_sync: Option<Timestamp>,
    connections: Vec<(SalesChannelKind, ConnectionState)>,
    ads: BTreeMap<String, String>,
}

pub struct ListingsScreen {
    listings: Vec<Listing>,
    /// How each listing's stock follows the app's, by listing.
    mirrored: Vec<MirroredStock>,
    /// The listings sold on order, with their terms.
    on_order: Vec<OnOrderListing>,
    to_link: Vec<ListingToLink>,
    products: Vec<Product>,
    last_sync: Option<Timestamp>,
    /// Each Sales Channel's Connection, once read.
    connections: Vec<(SalesChannelKind, ConnectionState)>,
    /// Where each Mercado Livre listing's ad stands in Product Ads, by its
    /// id.
    ads: BTreeMap<String, String>,
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
    /// The listing whose on-order terms are open below its row.
    on_order_form: Option<RecordId>,
    on_order_days: Entity<InputState>,
    on_order_accepted: Entity<InputState>,
    simulator: Simulator,
    drafts: Entity<DraftsSection>,
    channel_filter: ChannelFilter,
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
            on_order: Vec::new(),
            to_link: Vec::new(),
            products: Vec::new(),
            last_sync: None,
            connections: Vec::new(),
            ads: BTreeMap::new(),
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
            on_order_form: None,
            on_order_days: input("15", window, cx),
            on_order_accepted: input("3", window, cx),
            simulator,
            drafts,
            channel_filter: ChannelFilter::default(),
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

    /// Follows a new Work Context: a listing's open row or price panel on a
    /// channel it does not show closes without sending, as does a draft's
    /// editor; the drafts themselves stay saved.
    pub fn follow_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let context = preferences::work_context(cx);
        let channel_of = |id: RecordId| {
            self.listings
                .iter()
                .chain(self.to_link.iter().map(|entry| &entry.listing))
                .find(|listing| listing.id == id)
                .map(|listing| listing.channel)
        };
        let (mut step, mut price) = (self.step.take(), self.price.take());
        let mut on_order_form = self.on_order_form.take();
        work_context::close_outside(&mut step, context, |(id, _)| channel_of(*id));
        work_context::close_outside(&mut price, context, |panel| channel_of(panel.listing));
        work_context::close_outside(&mut on_order_form, context, |id| channel_of(*id));
        (self.step, self.price, self.on_order_form) = (step, price, on_order_form);
        self.drafts
            .update(cx, |drafts, cx| drafts.follow_context(window, cx));
        cx.notify();
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
        let reach = sales_channels::reach(cx);
        let ad_campaigns = ads::ad_campaigns(cx);
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
                    on_order: listings.on_order().await.map_err(|e| failure(&e))?,
                    to_link: listings.to_link(&known).await.map_err(|e| failure(&e))?,
                    last_sync: listings.last_sync().await.map_err(|e| failure(&e))?,
                    connections: reach.states(),
                    ads: ad_groups::ad_lines(ad_campaigns.as_deref()).await?,
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
        if self.row_open() {
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
        if self.on_order_form.is_some_and(|open| {
            !snapshot
                .listings
                .iter()
                .any(|listing| listing.id == open && listing.listed.status != ListingStatus::Closed)
        }) {
            self.on_order_form = None;
        }
        self.listings = snapshot.listings;
        self.mirrored = snapshot.mirrored;
        self.on_order = snapshot.on_order;
        self.to_link = snapshot.to_link;
        self.products = snapshot.products;
        self.last_sync = snapshot.last_sync;
        self.connections = snapshot.connections;
        self.ads = snapshot.ads;
    }

    /// Whether a row is open below a listing, where its outcome shows.
    fn row_open(&self) -> bool {
        self.step.is_some() || self.price.is_some() || self.on_order_form.is_some()
    }

    /// Whether any Sales Channel is connected.
    fn connected(&self) -> bool {
        sales_channels::any_connected(&self.connections)
    }

    fn product(&self, id: RecordId) -> Option<&Product> {
        self.products.iter().find(|product| product.id == id)
    }

    /// Reads the listings of every connected Sales Channel, sends the stock
    /// still to send, then reads the quality of each listing on the channels
    /// that rate them (#23, #136).
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(mirror), Some(rated), Some(kept)) = (
            stock_mirror::mirror(cx),
            quality::quality(cx),
            orders::orders(cx),
        ) else {
            return;
        };
        let reach = sales_channels::reach(cx);
        let repricing = repricing::repricing(cx).zip(pricing::taxes(cx));
        self.outcome = None;
        self.step = None;
        self.price = None;
        self.on_order_form = None;
        self.syncing = true;
        self.run(
            window,
            cx,
            move |listings, catalog| async move {
                let channels = reach.connected().await;
                let mut report = ListingSync::default();
                let mut failures = Vec::new();
                for channel in &channels {
                    match listings.sync(channel.listings.as_ref()).await {
                        Ok(synced) => {
                            report.read += synced.read;
                            report.new += synced.new;
                            report.changed += synced.changed;
                            report.gone += synced.gone;
                        }
                        Err(error) => failures.push(channel_failure(channel.kind(), &error)),
                    }
                }
                if !channels.is_empty() && failures.len() == channels.len() {
                    return Err(failures.join(" "));
                }
                let round = orders::round(&kept, &mirror, &catalog, &channels).await;
                let repriced = match &repricing {
                    Some((repricing, taxes)) => {
                        repricing::round(repricing, &catalog, taxes, &channels).await
                    }
                    None => Default::default(),
                };
                let rated = quality::sync_channels(&listings, &rated, &channels).await;
                Ok(Some((report, failures, round, repriced, rated)))
            },
            |this,
             (report, failures, round, repriced, rated): (
                ListingSync,
                Vec<String>,
                orders::RoundOutcome,
                repricing::RepricingOutcome,
                quality::ChannelsRead,
            ),
             _,
             cx| {
                let mut text = synced(&report);
                text.push_str(&stock_sent(&sent_in(&round)));
                text.push_str(&repricing::sent_text(&repriced));
                orders::finished(&round, cx);
                repricing::finished(&repriced);
                for failed in &failures {
                    text.push(' ');
                    text.push_str(failed);
                }
                let mut failed = !failures.is_empty();
                if !rated.is_empty() {
                    let (read, quality_failed) = quality::synced_channels(&rated);
                    text.push(' ');
                    text.push_str(&read);
                    if rated.iter().any(|(_, read)| {
                        read.as_ref()
                            .is_ok_and(|read| read.pending > 0 || read.below_target > Some(0))
                    }) {
                        text.push_str(" Veja em Qualidade.");
                    }
                    failed |= quality_failed;
                }
                this.outcome = Some(if failed {
                    Outcome::Failed(text.into())
                } else {
                    Outcome::Done(text.into())
                });
            },
        );
    }

    /// Sends the stock in the queue now, right after the Orders of every
    /// channel.
    fn send_stock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(mirror), Some(kept)) = (stock_mirror::mirror(cx), orders::orders(cx)) else {
            return;
        };
        let reach = sales_channels::reach(cx);
        self.outcome = None;
        self.run(
            window,
            cx,
            move |_, catalog| async move {
                let channels = reach.connected().await;
                Ok(Some(
                    orders::round(&kept, &mirror, &catalog, &channels).await,
                ))
            },
            |this, round: orders::RoundOutcome, _, cx| {
                let sent = sent_in(&round);
                let text = stock_sent(&sent).trim().to_owned();
                this.outcome = Some(match (&round, sent.failed) {
                    (Ok(_), 0) => Outcome::Done(text.into()),
                    (Ok(_), _) => Outcome::Failed(text.into()),
                    (Err(error), _) => Outcome::Failed(error.clone().into()),
                });
                orders::finished(&round, cx);
            },
        );
    }

    /// The Sales Channel `listing` is on, with the adapter that reaches it.
    fn channel_of(
        &self,
        listing: RecordId,
        cx: &App,
    ) -> Option<(SalesChannelKind, Arc<dyn SalesChannel>)> {
        let kind = self
            .listings
            .iter()
            .find(|known| known.id == listing)?
            .channel;
        sales_channels::of(cx, kind).map(|channel| (kind, channel.listings))
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
        let Some((kind, channel)) = self.channel_of(listing, cx) else {
            return;
        };
        self.outcome = None;
        self.step = None;
        self.price = None;
        self.on_order_form = None;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                let changed = if pause {
                    listings.pause(listing, channel.as_ref()).await
                } else {
                    listings.reactivate(listing, channel.as_ref()).await
                };
                changed.map(Some).map_err(|e| channel_failure(kind, &e))
            },
            move |this, changed: Vec<Listing>, _, cx| {
                // Its share goes to the product's other listings, or comes
                // back from them.
                stock_mirror::send_after_movement(cx);
                let what = if pause { "pausado" } else { "reativado" };
                let on = sales_channels::on(kind);
                this.outcome = Some(Outcome::Done(
                    match changed.len() {
                        0 | 1 => format!("Anúncio {what} {on}."),
                        count => format!("Anúncio {what} {on}, com as {count} variações dele."),
                    }
                    .into(),
                ));
            },
        );
    }

    /// The on-order mark of `listing`, when it is sold on order.
    fn on_order_of(&self, listing: RecordId) -> Option<&OnOrderListing> {
        self.on_order.iter().find(|mark| mark.listing == listing)
    }

    /// The Sales Channel `listing` is on, with the port its days to be
    /// ready go through; `None` on a channel that shows none.
    fn availability_of(
        &self,
        listing: RecordId,
        cx: &App,
    ) -> Option<(SalesChannelKind, Arc<dyn ListingAvailability>)> {
        let kind = self
            .listings
            .iter()
            .find(|known| known.id == listing)?
            .channel;
        sales_channels::of(cx, kind)?
            .availability
            .map(|port| (kind, port))
    }

    /// Opens the on-order terms below the listing's row, filled with the
    /// ones it has when it is sold on order already.
    fn open_on_order(&mut self, listing: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.step = None;
        self.price = None;
        self.outcome = None;
        self.row_outcome = None;
        self.on_order_form = Some(listing);
        let (days, accepted) = self
            .on_order_of(listing)
            .map(|mark| {
                (
                    mark.terms.availability_days.to_string(),
                    mark.terms.accepted_orders.to_string(),
                )
            })
            .unwrap_or_default();
        self.on_order_days
            .update(cx, |input, cx| input.set_value(days, window, cx));
        self.on_order_accepted
            .update(cx, |input, cx| input.set_value(accepted, window, cx));
        cx.notify();
    }

    fn close_on_order(&mut self, cx: &mut Context<Self>) {
        self.on_order_form = None;
        self.row_outcome = None;
        cx.notify();
    }

    /// Sells the listing on order with the terms typed, every variation of
    /// it included: the days go to the channel only on the owner's click,
    /// and its stock becomes the orders that still fit.
    fn mark_on_order(&mut self, listing: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((kind, channel)) = self.availability_of(listing, cx) else {
            return;
        };
        let typed = |field: &Entity<InputState>| field.read(cx).value().trim().parse::<u32>().ok();
        let (Some(availability_days), Some(accepted_orders)) =
            (typed(&self.on_order_days), typed(&self.on_order_accepted))
        else {
            self.row_outcome = Some(Outcome::Failed(
                "Digite em números inteiros os dias para ter o produto (1 a 60) e quantas \
                 encomendas o anúncio aceita ao mesmo tempo."
                    .into(),
            ));
            cx.notify();
            return;
        };
        let terms = OnOrderTerms {
            availability_days,
            accepted_orders,
        };
        self.row_outcome = None;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                listings
                    .mark_on_order(listing, terms, channel.as_ref())
                    .await
                    .map(Some)
                    .map_err(|e| on_order_failure(kind, &e))
            },
            move |this, marked: Vec<OnOrderListing>, _, cx| {
                this.on_order_form = None;
                // Its share goes to the product's other listings, and the
                // orders that still fit go to it.
                stock_mirror::send_after_movement(cx);
                this.outcome = Some(Outcome::Done(marked_text(kind, terms, marked.len()).into()));
            },
        );
    }

    /// Stops selling the listing on order, every variation of it included:
    /// the days leave the channel first, only on the owner's click.
    fn unmark_on_order(&mut self, listing: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((kind, channel)) = self.availability_of(listing, cx) else {
            return;
        };
        self.outcome = None;
        self.step = None;
        self.price = None;
        self.on_order_form = None;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                listings
                    .unmark_on_order(listing, channel.as_ref())
                    .await
                    .map(Some)
                    .map_err(|e| on_order_failure(kind, &e))
            },
            move |this, unmarked: Vec<RecordId>, _, cx| {
                stock_mirror::send_after_movement(cx);
                let on = sales_channels::on(kind);
                let variations = match unmarked.len() {
                    0 | 1 => String::new(),
                    count => format!(", com as {count} variações dele"),
                };
                this.outcome = Some(Outcome::Done(
                    format!(
                        "O anúncio deixou de vender sob encomenda {on}{variations}: o prazo saiu \
                         do anúncio, e ele volta a receber parte do estoque do produto no \
                         próximo envio."
                    )
                    .into(),
                ));
            },
        );
    }

    /// The on-order terms of `listing`, below its row: what selling on
    /// order does, the days and the orders accepted. On a channel without
    /// days to be ready, why it is not offered.
    fn render_on_order(&self, listing: &Listing, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let id = listing.id;
        let kind = listing.channel;
        let close = Button::new("close-on-order")
            .label(if self.availability_of(id, cx).is_some() {
                "Cancelar"
            } else {
                "Fechar"
            })
            .ghost()
            .small()
            .on_click(cx.listener(|this, _, _, cx| this.close_on_order(cx)));
        let heading = div().font_medium().child("Vender sob encomenda");
        let body = if self.availability_of(id, cx).is_none() {
            v_flex()
                .gap_2()
                .child(kit::info_notice(
                    format!(
                        "Vender sob encomenda só funciona no Mercado Livre: {} não mostra, pela \
                         API, um prazo para ter o produto no anúncio.",
                        sales_channels::the(kind)
                    ),
                    cx,
                ))
                .child(h_flex().child(close))
        } else {
            let marked = self.on_order_of(id).is_some();
            let siblings = self
                .listings
                .iter()
                .filter(|other| other.channel == kind && other.listed.id == listing.listed.id)
                .count();
            let field = |label: &'static str, input: &Entity<InputState>| {
                v_flex()
                    .gap_1()
                    .w(px(200.))
                    .child(div().text_xs().font_medium().child(label))
                    .child(Input::new(input).small())
            };
            v_flex()
                .gap_2()
                .child(div().text_xs().text_color(t.text2).child(format!(
                    "Você vende antes e compra depois da venda; o produto passa pela sua casa e \
                     sai com a etiqueta do Mercado Livre. O anúncio mostra o prazo para ter o \
                     produto (até {MAX_AVAILABILITY_DAYS} dias) e deixa de receber parte do \
                     estoque: o estoque dele no canal passa a ser as encomendas que ainda cabem. \
                     O Mercado Livre mostra antes os anúncios com estoque e não aceita o prazo \
                     em anúncio com Flex ou no Full."
                )))
                .when(siblings > 1, |form| {
                    form.child(div().text_xs().text_color(t.text2).child(format!(
                        "O prazo vale para as {siblings} variações do anúncio, e cada uma aceita \
                         as suas encomendas."
                    )))
                })
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .items_end()
                        .child(field("Dias para ter o produto", &self.on_order_days))
                        .child(field("Encomendas ao mesmo tempo", &self.on_order_accepted))
                        .child(
                            Button::new("confirm-on-order")
                                .label(if marked {
                                    "Salvar"
                                } else {
                                    "Vender sob encomenda"
                                })
                                .primary()
                                .small()
                                .loading(self.busy)
                                .disabled(self.busy || !self.connected())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.mark_on_order(id, window, cx)
                                })),
                        )
                        .child(close),
                )
        };
        v_flex()
            .gap_2()
            .pt_3()
            .border_t(t.border_width)
            .border_color(t.frame)
            .child(heading)
            .child(body)
            .children(self.row_outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .into_any_element()
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
        self.on_order_form = None;
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
        self.on_order_form = None;
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
        let (Some((kind, channel)), Some(pricing), Some(taxes), Some(catalog)) = (
            self.channel_of(listing, cx),
            pricing::pricing(cx),
            pricing::taxes(cx),
            catalog::catalog(cx),
        ) else {
            return;
        };
        self.step = None;
        self.on_order_form = None;
        self.outcome = None;
        self.row_outcome = None;
        let products = self.products.clone();
        let repricing = cx.new(|cx| RepricingPanel::new(listing, kind, products, window, cx));
        // The price buttons follow whether repricing is on.
        let repricing_changes = cx.observe(&repricing, |_, _, cx| cx.notify());
        self.price = Some(PricePanel {
            listing,
            loading: true,
            repricing,
            _repricing_changes: repricing_changes,
            today: None,
            suggestion: None,
        });
        let reading = cx.background_executor().spawn(async move {
            let assumptions = pricing::assumptions(&catalog, &taxes).await?;
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
                        let why =
                            |error: PricingError| price_failure(&error, kind, listing, products);
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

    /// Sends `price` to the listing's channel for it and its variations:
    /// only ever on the owner's click.
    fn send_price(
        &mut self,
        listing: RecordId,
        price: Money,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((kind, channel)) = self.channel_of(listing, cx) else {
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
                    .map_err(|e| channel_failure(kind, &e))
            },
            move |this, changed: Vec<Listing>, _, _| {
                this.price = None;
                let price = price.rounded().to_pt_br();
                let to = sales_channels::to(kind);
                let mut done = match changed.len() {
                    0 | 1 => format!("Preço de {price} enviado {to}."),
                    count => {
                        format!(
                            "Preço de {price} enviado {to} para as {count} variações do anúncio."
                        )
                    }
                };
                if changed.iter().any(|listing| listing.sent_price.is_some()) {
                    done.push_str(&format!(
                        " {} aplica o preço depois; o próximo Sync confirma.",
                        sales_channels::opening(kind)
                    ));
                }
                this.outcome = Some(Outcome::Done(done.into()));
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

    /// The Sales Channel `listing` is on, Mercado Livre while unknown.
    fn kind_of(&self, listing: RecordId) -> SalesChannelKind {
        self.listings
            .iter()
            .find(|known| known.id == listing)
            .map_or(SalesChannelKind::MercadoLivre, |known| known.channel)
    }

    fn render_price(&self, panel: &PricePanel, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let listing = panel.listing;
        let kind = self.kind_of(listing);
        let repriced = panel.repricing.read(cx).is_repriced();
        let mut body = v_flex().gap_3();
        if panel.loading {
            body = body.child(div().text_sm().text_color(t.text2).child(format!(
                "Consultando a tarifa {} para o preço de hoje e o sugerido…",
                sales_channels::of_channel(kind)
            )));
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
                                .when(repriced, |row| {
                                    row.child(div().text_xs().text_color(t.text2).child(
                                        "A reprecificação automática cuida do preço deste \
                                         anúncio; desligue-a abaixo para aprovar um preço seu.",
                                    ))
                                })
                                .when(!repriced, |row| {
                                    row.child(
                                        Button::new("approve-price")
                                            .label(format!(
                                                "Aprovar e enviar {} {}",
                                                price.to_pt_br(),
                                                send_to(kind)
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
                                })
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
            body = body.child(self.render_simulator(listing, like, repriced, cx));
        }
        body = body.child(panel.repricing.clone());
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
        repriced: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let kind = self.kind_of(listing);
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
                "Começa com a venda de hoje: tarifa do canal no preço de hoje, frete \
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
                            Some(price) => {
                                format!("Enviar {} {}", price.to_pt_br(), send_to(kind))
                            }
                            None => format!("Enviar {}", send_to(kind)),
                        })
                        .outline()
                        .small()
                        .disabled(self.busy || repriced || send.is_none())
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
        if self
            .mirrored
            .iter()
            .all(|stock| stock.on_hand.is_none() && stock.on_order.is_none())
        {
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
                        (false, _) => "vai no próximo Sync, as partes que diminuem antes das \
                                       que aumentam; o que falhar fica aqui"
                            .to_owned(),
                        (true, Some(at)) => format!(
                            "o saldo de cada produto é dividido entre os anúncios dele · \
                             último envio: {}",
                            catalog::day_and_time(at)
                        ),
                        (true, None) => {
                            "o saldo de cada produto é dividido entre os anúncios dele".to_owned()
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
                "{}: {} → {}: {}",
                sales_channels::on(listing.channel),
                listed.available_quantity,
                if stock.on_order.is_some() {
                    "encomendas"
                } else {
                    "parte"
                },
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
                            sales_channels::failure(listing.channel, &failed.error)
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
            stock_text(listed.available_quantity, listing.channel),
        ];
        // Classic and Premium are Mercado Livre's own listing types.
        if listing.channel == SalesChannelKind::MercadoLivre {
            facts.extend(listed.listing_type.map(|kind| kind.name().to_owned()));
        }
        facts.extend(
            listed
                .seller_sku
                .as_ref()
                .map(|sku| format!("SKU {}: {sku}", sales_channels::on(listing.channel))),
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
                    .child(kit::tag(listing.channel.name(), t.text2, cx))
                    .child(kit::tag(
                        status_name(listed.status),
                        status_ink(listed.status, &t),
                        cx,
                    ))
                    .children(self.on_order_of(listing.id).map(|mark| {
                        kit::tag(
                            format!("Sob encomenda · {}", days(mark.terms.availability_days)),
                            t.accent_text,
                            cx,
                        )
                    })),
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
                    .child(div().min_w_0().child(facts.join(" · ")))
                    .children(listed.link.clone().map(|link| {
                        div()
                            .id(gpui_kit::SharedString::from(format!("open-{id}")))
                            .flex()
                            .gap_1()
                            .items_center()
                            .text_color(t.accent_text)
                            .cursor_pointer()
                            .child(format!("Abrir {}", sales_channels::on(listing.channel)))
                            .child(
                                gpui_kit::component::Icon::new(IconName::ExternalLink)
                                    .size(px(12.)),
                            )
                            .on_click(move |_, _, cx| cx.open_url(&link))
                    })),
            )
            .children(
                (listing.channel == SalesChannelKind::MercadoLivre)
                    .then(|| self.ads.get(&listed.id))
                    .flatten()
                    .map(|line| div().text_xs().text_color(t.text2).child(line.clone())),
            )
            .children(listing.sent_price.as_ref().map(|sent| {
                let ink = match sent.state {
                    SentPriceState::Waiting => t.text2,
                    SentPriceState::NotApplied => t.danger,
                };
                div()
                    .text_xs()
                    .text_color(ink)
                    .child(sent_price_text(sent, listing))
            }))
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
        let follows = matches!(
            listing.listed.status,
            ListingStatus::Active | ListingStatus::Paused | ListingStatus::OutOfStock
        );
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
        let on_order = self.on_order_of(id);
        let stock_line = match (on_order, self.stock_of(id)) {
            (Some(mark), Some(stock)) if follows => Some(on_order_line(listing, mark, stock)),
            _ => listing
                .product
                .filter(|_| follows)
                .map(|_| stock_line(listing, self.stock_of(id))),
        }
        .map(|text| div().text_xs().text_color(t.text2).child(text));
        let on_order_actions = (listing.listed.status != ListingStatus::Closed).then(|| {
            let sells_on_order = self.availability_of(id, cx).is_some();
            h_flex().gap_2().flex_wrap().map(|row| match on_order {
                Some(_) => row
                    .child(
                        Button::new(("change-on-order", index))
                            .label("Mudar prazo ou encomendas")
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_on_order(id, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("unmark-on-order", index))
                            .label("Deixar de vender sob encomenda")
                            .ghost()
                            .small()
                            .disabled(self.busy || !self.connected())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.unmark_on_order(id, window, cx)
                            })),
                    ),
                None => row.child(
                    Button::new(("open-on-order", index))
                        .label("Vender sob encomenda")
                        .ghost()
                        .small()
                        .disabled(self.busy || (sells_on_order && !self.connected()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_on_order(id, window, cx)
                        })),
                ),
            })
        });
        let on_order_form =
            (self.on_order_form == Some(id)).then(|| self.render_on_order(listing, cx));
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
            .border_color(if panel.is_some() || on_order_form.is_some() {
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
            .children(on_order_actions)
            .children(on_order_form)
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
        let context = preferences::work_context(cx);
        let states = sales_channels::shown_states(&self.connections, context);
        for text in sales_channels::connection_notices(&states) {
            parts
                .notices
                .push(kit::info_notice(text, cx).into_any_element());
        }
        if !self.row_open() {
            parts
                .notices
                .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        }

        parts.content.push(self.drafts.clone().into_any_element());
        // The stock is one, split across every channel's listings, and goes
        // out only after every channel's Orders (ADR 0017, ADR 0033): its
        // queue shows whole in every context, each row naming its channel.
        parts.content.extend(self.render_stock(cx));

        let to_link: Vec<usize> = (0..self.to_link.len())
            .filter(|index| context.shows_channel(self.to_link[*index].listing.channel))
            .collect();
        if !to_link.is_empty() {
            parts.content.push(
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .child(kit::section_heading(format!(
                                "A vincular ({})",
                                to_link.len()
                            )))
                            .child(div().text_xs().text_color(t.text2).child(
                                "anúncios sem produto: confirme a sugestão, escolha um produto \
                                 ou crie um a partir do anúncio",
                            )),
                    )
                    .children(
                        to_link
                            .into_iter()
                            .map(|index| self.render_to_link(index, cx)),
                    )
                    .into_any_element(),
            );
        }

        let held: BTreeSet<SalesChannelKind> = self
            .listings
            .iter()
            .map(|listing| listing.channel)
            .collect();
        let filter = self.channel_filter.in_context(context);
        let shown: Vec<usize> = (0..self.listings.len())
            .filter(|index| filter.shows(self.listings[*index].channel, &held))
            .collect();
        let mut heading = match shown.len() {
            0 => "Anúncios".to_owned(),
            1 => "1 anúncio".to_owned(),
            count => format!("{count} anúncios"),
        };
        let single = match held.iter().collect::<Vec<_>>()[..] {
            [only] => Some(*only),
            _ => None,
        };
        if let Some(only) = filter.only(&held).or(single) {
            heading = format!("{heading} {}", sales_channels::on(only));
        }
        let chips = filter.chips(
            "listing-channel",
            &held,
            cx,
            |this: &mut Self, filter, cx| {
                this.channel_filter = filter;
                cx.notify();
            },
        );
        parts.content.push(
            v_flex()
                .gap_3()
                .child(kit::section_heading(heading))
                .children(chips)
                .when(shown.is_empty(), |list| {
                    let on = filter
                        .only(&held)
                        .map(|kind| format!(" {}", sales_channels::on(kind)))
                        .unwrap_or_default();
                    list.child(div().text_sm().text_color(t.text2).child(format!(
                        "Nenhum anúncio{on} ainda. Sincronize para trazer os anúncios ativos e \
                         pausados da sua conta; cada variação vira um anúncio próprio, vinculado \
                         ao produto que ela vende."
                    )))
                })
                .children(
                    shown
                        .into_iter()
                        .map(|index| self.render_listing(index, cx)),
                )
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
        ListingStatus::OutOfStock => "Sem estoque",
        ListingStatus::Closed => "Encerrado",
        ListingStatus::UnderReview => "Em revisão",
        ListingStatus::Inactive => "Inativo",
    }
}

fn status_ink(status: ListingStatus, t: &Tokens) -> Hsla {
    match status {
        ListingStatus::Active => t.success,
        ListingStatus::UnderReview | ListingStatus::Inactive => t.danger,
        ListingStatus::Paused | ListingStatus::OutOfStock | ListingStatus::Closed => t.text2,
    }
}

/// Where a price sent to a channel that applies it later stands.
fn sent_price_text(sent: &SentPrice, listing: &Listing) -> String {
    let price = sent.price.to_pt_br();
    let at = catalog::day_and_time(sent.sent_at);
    match sent.state {
        SentPriceState::Waiting => format!(
            "Preço de {price} enviado em {at}; {} ainda aplica, e o próximo Sync confirma.",
            sales_channels::the(listing.channel)
        ),
        SentPriceState::NotApplied => format!(
            "{} não aplicou o preço de {price} enviado em {at}: o anúncio continua a {}. \
             Envie de novo ou confira no painel {}.",
            sales_channels::opening(listing.channel),
            listing.listed.price.to_pt_br(),
            sales_channels::of_channel(listing.channel)
        ),
    }
}

fn stock_text(units: u32, channel: SalesChannelKind) -> String {
    let on = sales_channels::on(channel);
    match units {
        0 => format!("sem estoque {on}"),
        units => format!("{units} em estoque {on}"),
    }
}

/// How a listing's stock follows its Stock Share of the app's, in one
/// line.
fn stock_line(listing: &Listing, stock: Option<&MirroredStock>) -> String {
    if listing.listed.status == ListingStatus::Paused {
        return "Pausado por você: não recebe parte do estoque do produto, que fica para os \
                outros anúncios dele."
            .into();
    }
    let Some((stock, on_hand, share)) =
        stock.and_then(|stock| Some((stock, stock.on_hand?, stock.share?)))
    else {
        return "O estoque deste anúncio ainda não segue o app: o produto nunca entrou no \
                estoque. Ele passa a seguir no primeiro recebimento em Compras."
            .into();
    };
    let channel = listing.channel;
    let part = format!("Parte deste anúncio: {} de {}", share, units(on_hand));
    match (stock.to_send, stock.last_sent) {
        (Some(_), _) => format!("{part}, na fila para enviar."),
        (None, Some(sent)) => format!(
            "{part}, enviada {} em {}.",
            sales_channels::to(channel),
            catalog::day_and_time(sent.at)
        ),
        (None, None) => format!("{part}, igual {}.", sales_channels::to(channel)),
    }
}

/// How an On-Order Listing's stock follows the orders it takes, in one
/// line.
fn on_order_line(listing: &Listing, mark: &OnOrderListing, stock: &MirroredStock) -> String {
    let on = sales_channels::on(listing.channel);
    let accepted = orders(mark.terms.accepted_orders);
    if listing.listed.status == ListingStatus::Paused {
        return format!(
            "Sob encomenda, com prazo de {} e até {accepted} ao mesmo tempo. Pausado por você: \
             ao reativar, mostra as encomendas que ainda cabem.",
            days(mark.terms.availability_days)
        );
    }
    let slots = stock.on_order.unwrap_or(OrderSlots {
        accepted: mark.terms.accepted_orders,
        waiting: 0,
    });
    let waiting = match slots.waiting {
        0 => "nenhuma esperando despacho".to_owned(),
        1 => "1 unidade esperando despacho".to_owned(),
        count => format!("{count} unidades esperando despacho"),
    };
    let open = match slots.open() {
        0 => format!("nenhuma vaga {on}"),
        1 => format!("1 vaga {on}"),
        count => format!("{count} vagas {on}"),
    };
    let shown = match (stock.to_send, stock.last_sent) {
        (Some(_), _) => format!("{open}, na fila para enviar"),
        (None, Some(sent)) => format!("{open}, enviadas em {}", catalog::day_and_time(sent.at)),
        (None, None) => open,
    };
    format!(
        "Sob encomenda: aceita até {accepted} ao mesmo tempo, {waiting}; {shown}. Não recebe \
         parte do estoque do produto."
    )
}

fn days(count: u32) -> String {
    match count {
        1 => "1 dia".into(),
        count => format!("{count} dias"),
    }
}

fn orders(count: u32) -> String {
    match count {
        1 => "1 encomenda".into(),
        count => format!("{count} encomendas"),
    }
}

/// What marking a listing as on-order did, in one line.
fn marked_text(kind: SalesChannelKind, terms: OnOrderTerms, listings: usize) -> String {
    let variations = match listings {
        0 | 1 => String::new(),
        count => format!(", com as {count} variações dele"),
    };
    format!(
        "Anúncio à venda sob encomenda {}{variations}: prazo de {} para ter o produto e até {} \
         ao mesmo tempo. O estoque dele no canal vai no próximo envio.",
        sales_channels::on(kind),
        days(terms.availability_days),
        orders(terms.accepted_orders)
    )
}

/// Why selling a listing on order, or no longer, failed, as the owner
/// reads it.
fn on_order_failure(kind: SalesChannelKind, error: &ListingError) -> String {
    match error {
        ListingError::Platform(PlatformError::Refused(why)) => format!(
            "{} recusou o prazo para ter o produto: {why}. O anúncio continua como estava.",
            sales_channels::opening(kind)
        ),
        _ => channel_failure(kind, error),
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
    match sent.held {
        0 => {}
        1 => text.push_str(
            " 1 anúncio espera a parte de outro anúncio do produto diminuir antes de subir.",
        ),
        count => text.push_str(&format!(
            " {count} anúncios esperam a parte de outros anúncios do produto diminuir antes de \
             subir."
        )),
    }
    if text.is_empty() {
        text.push_str(" Nenhum estoque a enviar.");
    }
    text
}

/// "ao ML", "à Amazon": where a price goes, short enough for a button.
fn send_to(kind: SalesChannelKind) -> String {
    match kind {
        SalesChannelKind::MercadoLivre => "ao ML".into(),
        other => sales_channels::to(other),
    }
}

/// The stock a round of Orders and stock sent; none when it did not run.
fn sent_in(round: &orders::RoundOutcome) -> StockSend {
    match round {
        Ok(Some(round)) => round.sent,
        _ => StockSend::default(),
    }
}

/// What the Sync did, in one line.
fn synced(report: &ListingSync) -> String {
    let mut text = format!(
        "Sync concluído: {} anúncios lidos, {} novos e {} com mudanças.",
        report.read, report.new, report.changed
    );
    if report.gone > 0 {
        text.push_str(&format!(
            " {} que o canal de venda não tem mais ficaram como encerrados.",
            report.gone
        ));
    }
    text
}

pub(crate) fn failure(error: &ListingError) -> String {
    channel_failure(SalesChannelKind::MercadoLivre, error)
}

/// Why an action on a listing of `kind` failed, as the owner reads it.
pub(crate) fn channel_failure(kind: SalesChannelKind, error: &ListingError) -> String {
    match error {
        ListingError::UnknownListing(_) => {
            "Esse anúncio não existe mais; a lista foi atualizada.".into()
        }
        ListingError::InvalidPrice => "O preço precisa ser maior que zero, em reais.".into(),
        ListingError::Repriced => "A reprecificação automática cuida do preço deste anúncio; \
             desligue-a abaixo para enviar um preço seu."
            .into(),
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
        ListingError::InvalidAvailability => {
            format!("O prazo para ter o produto vai de 1 a {MAX_AVAILABILITY_DAYS} dias.")
        }
        ListingError::NoOrdersAccepted => {
            "O anúncio precisa aceitar pelo menos 1 encomenda ao mesmo tempo.".into()
        }
        ListingError::OtherChannel { listing, channel } => format!(
            "Esse anúncio é {}, não {}; a lista foi atualizada.",
            sales_channels::of_channel(*listing),
            sales_channels::of_channel(*channel)
        ),
        ListingError::NotTheRanking => "A ordem precisa ter cada canal conectado uma vez só; \
             a lista foi atualizada."
            .into(),
        ListingError::Stock(_) => format!("Não consegui ler o estoque: {error}"),
        ListingError::Platform(error) => sales_channels::failure(kind, error),
        ListingError::Unreadable(_) | ListingError::Sql(_) => {
            format!("Não consegui ler ou gravar no banco: {error}")
        }
        ListingError::UnknownDraft(_)
        | ListingError::Published(_)
        | ListingError::Blocked(_)
        | ListingError::Picture { .. }
        | ListingError::DescriptionNotSent(..)
        | ListingError::InvalidGtin
        | ListingError::NotACatalogOffer(_)
        | ListingError::VariationsNotSent(..)
        | ListingError::NoVariations(_)
        | ListingError::RepeatedVariation(_)
        | ListingError::VariationsWithoutTheDraftProduct => drafts::failure(kind, error),
    }
}

/// "R$ 91,67 − tarifa R$ 12,83 − frete R$ 20,00 − …": where the money of a
/// sale goes.
pub(crate) fn breakdown_line(sale: &PriceBreakdown, cx: &App) -> gpui_kit::Div {
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
pub(crate) fn price_failure(
    error: &PricingError,
    kind: SalesChannelKind,
    listing: RecordId,
    products: &[Product],
) -> String {
    let sku = |id: RecordId| {
        products
            .iter()
            .find(|product| product.id == id)
            .map(|product| product.sku.to_string())
            .unwrap_or_else(|| "do anúncio".into())
    };
    match error {
        PricingError::Listing(error) => channel_failure(kind, error),
        PricingError::Closed => format!("O anúncio está encerrado {}.", sales_channels::on(kind)),
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
        PricingError::NoFeeBasis => format!(
            "{} não informou a categoria ou o tipo do anúncio; sincronize de novo.",
            sales_channels::opening(kind)
        ),
        PricingError::Unreachable => "Nenhum preço chega à margem alvo: tarifa, imposto e \
             margem alvo somam 100% ou mais. Baixe a margem alvo na ficha do produto ou em \
             Configurações."
            .into(),
        PricingError::Unsettled => format!(
            "A tarifa {} mudou a cada preço consultado; use o simulador.",
            sales_channels::of_channel(kind)
        ),
        PricingError::InvalidTarget => pricing::target_failure(error),
        PricingError::Currencies(_) => "O custo e o preço estão em moedas diferentes.".into(),
        PricingError::Platform(error) => sales_channels::failure(kind, error),
        PricingError::Stock(_) | PricingError::Unreadable(_) | PricingError::Sql(_) => {
            format!("Não consegui ler ou gravar no banco: {error}")
        }
    }
}
