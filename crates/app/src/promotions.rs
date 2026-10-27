//! Promoções (#26, #136): the owner's Promotions on each Sales Channel, one
//! channel at a time. On Mercado Livre a listing's own price discount, a
//! campaign of theirs and a coupon; on Shopee the shop's discounts; Amazon
//! and Magalu offer sellers none by their APIs, and the screen says so.
//! The margin guard refuses any that would leave a sale below the minimum
//! margin and shows the calculation. Creating one on Mercado Livre needs
//! the Reputation that unlocks Promotions (from Marketing); each is
//! checked, shown and sent only once the owner confirms it. The active ones
//! are listed by listing, with their last day and the margin left in the
//! worst case. Read hourly after an Order Sync and on this screen's button.
//! The rules live in Commerce (ADR 0024).

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{TimeDelta, Utc};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::select::Select;
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Entity, Global, SharedString, Subscription, Task, Window, div, px,
};
use mascate_catalog::Product;
use mascate_commerce::{
    CouponDiscount, CouponTerms, DiscountPlan, Listing, ListingStatus, Offer, Promotion,
    PromotionCheck, PromotionError, PromotionKind, PromotionPlan, PromotionRequest,
    PromotionStatus, PromotionSync, Promotions, promotion_kinds,
};
use mascate_integrations::ConnectionState;
use mascate_kernel::{
    Currency, Money, Percentage, PlatformError, RecordId, SalesChannelKind, Timestamp, channel_day,
    parse_amount,
};
use mascate_marketing::SellerTool;

use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE};
use crate::forms::{Choice, Outcome, Picker, day_text, input, notice, parse_day, picker, refill};
use crate::kit;
use crate::layout;
use crate::listings::{self, breakdown_line, listings, price_failure};
use crate::parts::ScreenParts;
use crate::preferences;
use crate::pricing;
use crate::reputation::reputation;
use crate::sales_channels::{self, ChannelPorts, of_channel, on, opening, to};

/// The app's Promotions; absent when the database did not open.
pub struct AppPromotions(pub Arc<Promotions>);

impl Global for AppPromotions {}

pub(crate) fn promotions(cx: &App) -> Option<Arc<Promotions>> {
    cx.try_global::<AppPromotions>().map(|app| app.0.clone())
}

/// The Syncs of the Promotions since the app opened; the screen reads them
/// again after each.
#[derive(Default)]
pub struct PromotionSyncs {
    pub finished: u64,
}

impl Global for PromotionSyncs {}

/// Why a Promotion on `channel` was not checked, sent or read, as the
/// owner reads it. `listing` is the one being priced, when there is one.
pub(crate) fn failure(
    error: &PromotionError,
    channel: SalesChannelKind,
    listing: Option<RecordId>,
    products: &[Product],
) -> String {
    match error {
        PromotionError::Locked => format!(
            "Promoções pedem {}; sua reputação ainda não chegou lá. Veja em Reputação.",
            SellerTool::Promotions.requirement()
        ),
        PromotionError::Listing(error) => listings::failure(error),
        PromotionError::Closed => format!("O anúncio está encerrado {}.", on(channel)),
        PromotionError::UnknownPromotion(_) | PromotionError::UnknownOffer(_) => {
            "Essa promoção não existe mais; atualize a tela.".into()
        }
        PromotionError::NotOwn => format!(
            "Campanhas {} se aceitam e se encerram no próprio canal.",
            of_channel(channel)
        ),
        PromotionError::NotOffered { kind, channel } => format!(
            "{} não oferece {} pela API.",
            opening(*channel),
            kind_name(*kind, *channel).to_lowercase()
        ),
        PromotionError::NoName => "Dê um nome à promoção.".into(),
        PromotionError::EndsBeforeStart => "O último dia vem antes do primeiro.".into(),
        PromotionError::StartsInPast => "O primeiro dia já passou; comece hoje ou depois.".into(),
        PromotionError::AlreadyStarted => {
            "A promoção já começou: o primeiro dia fica como está.".into()
        }
        PromotionError::OnlyShortens(channel) => format!(
            "{} só deixa encurtar uma promoção: o primeiro dia pode ficar mais tarde e o último \
             mais cedo, nunca o contrário.",
            opening(*channel)
        ),
        PromotionError::TooLong(days) => format!(
            "Esse tipo de promoção dura no máximo {days} dias, contando o primeiro e o último."
        ),
        PromotionError::NotADiscount => "O preço da promoção precisa ser maior que zero e menor \
             que o preço do anúncio."
            .into(),
        PromotionError::InvalidCoupon => "O cupom precisa de um desconto maior que zero (em %, \
             abaixo de 100), uma compra mínima e um orçamento maiores que zero."
            .into(),
        PromotionError::AlreadyDiscounted => {
            "O anúncio já tem um desconto; encerre-o antes de criar outro.".into()
        }
        PromotionError::AlreadyIn => "O anúncio já está nessa promoção.".into(),
        PromotionError::BelowMinimum(_) => "A margem ficaria abaixo da mínima.".into(),
        PromotionError::Sending => format!(
            "Essa promoção já está a caminho {}; aguarde a resposta.",
            of_channel(channel)
        ),
        PromotionError::Pricing(error) => match listing {
            Some(listing) => price_failure(error, channel, listing, products),
            None => format!("Não consegui calcular a margem: {error}"),
        },
        PromotionError::Platform(error) => sales_channels::failure(channel, error),
        PromotionError::Currencies(_) => "Os valores estão em moedas diferentes.".into(),
        error => minimum_failure(error),
    }
}

/// Why the minimum margin, the same on every channel, was not read or
/// saved.
pub(crate) fn minimum_failure(error: &PromotionError) -> String {
    match error {
        PromotionError::InvalidMinimum => "A margem mínima vai de 0 até menos de 100%.".into(),
        error => format!("Não consegui ler ou gravar as promoções no banco: {error}"),
    }
}

/// Runs a Sync of the Promotions of every connected channel that has them,
/// off the UI thread: when `forced`, or when its last one is an hour old.
/// `Ok(None)` when none is connected or nothing was due; one channel
/// failing leaves the others read.
pub fn sync_now(cx: &mut App, forced: bool) -> Option<Task<Result<Option<PromotionSync>, String>>> {
    let promotions = promotions(cx)?;
    let reach = sales_channels::reach(cx).only(|channel| channel.promotions.is_some());
    Some(cx.background_executor().spawn(async move {
        let mut read: Option<PromotionSync> = None;
        let mut failures = Vec::new();
        for channel in reach.connected().await {
            let (kind, Some(port)) = (channel.kind(), channel.promotions) else {
                continue;
            };
            let why = |error: PromotionError| failure(&error, kind, None, &[]);
            let synced = async {
                if !forced && !promotions.is_due(kind).await? {
                    return Ok(None);
                }
                promotions.sync(port.as_ref()).await.map(Some)
            }
            .await;
            match synced {
                Ok(Some(report)) => {
                    let total = read.get_or_insert_with(PromotionSync::default);
                    total.promotions += report.promotions;
                    total.offers += report.offers;
                    total.ended += report.ended;
                }
                Ok(None) => {}
                Err(error) => failures.push(why(error)),
            }
        }
        if failures.is_empty() {
            Ok(read)
        } else {
            Err(failures.join(" "))
        }
    }))
}

/// Tells the screen a Sync ran.
pub fn finished(outcome: &Result<Option<PromotionSync>, String>, cx: &mut App) {
    if let Err(error) = outcome {
        eprintln!("could not sync the promotions: {error}");
    }
    cx.default_global::<PromotionSyncs>().finished += 1;
}

/// What the channel calls the kind: Shopee's campaign of the owner's is
/// the shop's discount.
pub(crate) fn kind_name(kind: PromotionKind, channel: SalesChannelKind) -> String {
    match (kind, channel) {
        (PromotionKind::PriceDiscount, _) => "Desconto no anúncio".into(),
        (PromotionKind::SellerCampaign, SalesChannelKind::Shopee) => "Desconto da loja".into(),
        (PromotionKind::SellerCampaign, _) => "Campanha do vendedor".into(),
        (PromotionKind::Coupon, _) => "Cupom".into(),
        (PromotionKind::ChannelCampaign, _) => format!("Campanha {}", of_channel(channel)),
    }
}

fn status_name(status: PromotionStatus) -> &'static str {
    match status {
        PromotionStatus::Pending => "Agendada",
        PromotionStatus::Started => "Em andamento",
    }
}

/// "05/10/2026 a 18/10/2026".
fn days_text(starts: chrono::NaiveDate, ends: chrono::NaiveDate) -> String {
    format!("{} a {}", day_text(starts), day_text(ends))
}

/// "R$ 10,00 de desconto em compras a partir de R$ 50,00 · orçamento R$ 200,00".
pub(crate) fn coupon_text(terms: &CouponTerms) -> String {
    let discount = match terms.discount {
        CouponDiscount::Amount(amount) => format!("{} de desconto", amount.to_pt_br()),
        CouponDiscount::Percent(rate) => match terms.maximum_discount {
            Some(cap) => format!("{} de desconto, até {}", rate.to_pt_br(), cap.to_pt_br()),
            None => format!("{} de desconto", rate.to_pt_br()),
        },
    };
    format!(
        "{discount} em compras a partir de {} · orçamento {}",
        terms.minimum_purchase.to_pt_br(),
        terms.budget.to_pt_br()
    )
}

/// What the owner is filling in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    New(PromotionKind),
    /// New name and days for a campaign or coupon.
    Edit(RecordId),
    /// A listing into a campaign or coupon.
    Join(RecordId),
}

/// A Promotion checked and waiting for the owner's confirmation.
struct Pending {
    request: PromotionRequest,
    /// What goes to the channel, in words.
    summary: String,
    listing: Option<RecordId>,
}

/// A Promotion the margin guard refused.
struct Blocked {
    check: PromotionCheck,
}

/// Why a Promotion was not checked: the margin guard refused it, or
/// something else went wrong.
enum NotPrepared {
    Below(Box<PromotionCheck>),
    Failed(String),
}

impl NotPrepared {
    fn from(
        error: PromotionError,
        channel: SalesChannelKind,
        listing: Option<RecordId>,
        products: &[Product],
    ) -> Self {
        match error {
            PromotionError::BelowMinimum(check) => NotPrepared::Below(check),
            error => NotPrepared::Failed(failure(&error, channel, listing, products)),
        }
    }
}

/// An "Encerrar" waiting for its confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    Offer(RecordId),
    Promotion(RecordId),
}

/// A listing by its channel and the channel's id.
type ChannelId = (SalesChannelKind, String);

/// Everything the screen shows, read in one go off the UI thread.
struct Snapshot {
    promotions: Vec<Promotion>,
    offers: Vec<Offer>,
    listings: Vec<Listing>,
    names: BTreeMap<ChannelId, String>,
    products: Vec<Product>,
    minimum: Percentage,
    unlocks: Option<bool>,
    last_syncs: BTreeMap<SalesChannelKind, Timestamp>,
    connections: Vec<(SalesChannelKind, ConnectionState)>,
}

/// The fields of the form, one per thing a Promotion may need.
struct Fields {
    listing: Picker,
    name: Entity<InputState>,
    starts: Entity<InputState>,
    ends: Entity<InputState>,
    price: Entity<InputState>,
    coupon_value: Entity<InputState>,
    coupon_minimum: Entity<InputState>,
    coupon_maximum: Entity<InputState>,
    coupon_budget: Entity<InputState>,
    coupon_percent: bool,
}

pub struct PromotionsScreen {
    /// The Sales Channel shown.
    channel: SalesChannelKind,
    promotions: Vec<Promotion>,
    offers: Vec<Offer>,
    /// The open listings, one variation each, for the pickers.
    listings: Vec<Listing>,
    names: BTreeMap<ChannelId, String>,
    products: Vec<Product>,
    minimum: Option<Percentage>,
    /// Whether the Reputation unlocks Mercado Livre's Promotions; `None`
    /// before it is read.
    unlocks: Option<bool>,
    last_syncs: BTreeMap<SalesChannelKind, Timestamp>,
    /// Each channel the app knows, with its Connection.
    connections: Vec<(SalesChannelKind, ConnectionState)>,
    /// The worst-case margin of each listing in a Promotion, or why there
    /// is none.
    margins: BTreeMap<ChannelId, Result<PromotionCheck, String>>,
    fields: Fields,
    form: Option<Form>,
    pending: Option<Pending>,
    blocked: Option<Blocked>,
    ending: Option<Ending>,
    form_outcome: Option<Outcome>,
    syncing: bool,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl PromotionsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_global_in::<PromotionSyncs>(window, Self::refresh)];
        let fields = Fields {
            listing: picker(window, cx),
            name: input("Nome", window, cx),
            starts: input("dd/mm/aaaa", window, cx),
            ends: input("dd/mm/aaaa", window, cx),
            price: input("0,00", window, cx),
            coupon_value: input("0", window, cx),
            coupon_minimum: input("0,00", window, cx),
            coupon_maximum: input("sem limite", window, cx),
            coupon_budget: input("0,00", window, cx),
            coupon_percent: false,
        };
        let mut screen = Self {
            channel: preferences::work_context(cx)
                .sales_channel()
                .unwrap_or(SalesChannelKind::MercadoLivre),
            promotions: Vec::new(),
            offers: Vec::new(),
            listings: Vec::new(),
            names: BTreeMap::new(),
            products: Vec::new(),
            minimum: None,
            unlocks: None,
            last_syncs: BTreeMap::new(),
            connections: Vec::new(),
            margins: BTreeMap::new(),
            fields,
            form: None,
            pending: None,
            blocked: None,
            ending: None,
            form_outcome: None,
            syncing: false,
            busy: false,
            reads: 0,
            outcome: None,
            _subscriptions: subscriptions,
        };
        screen.refresh(window, cx);
        screen
    }

    /// Reads everything again, as when the screen comes into view, then
    /// works out the margin of each listing in a Promotion.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(promotions), Some(listings), Some(catalog), Some(reputation)) = (
            promotions(cx),
            listings(cx),
            catalog::catalog(cx),
            reputation(cx),
        ) else {
            return;
        };
        let reach = sales_channels::reach(cx);
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            let why = |error: PromotionError| minimum_failure(&error);
            let products = catalog.products().await.map_err(|e| catalog::failure(&e))?;
            let mut open: Vec<Listing> = Vec::new();
            let mut names = BTreeMap::new();
            for listing in listings
                .listings()
                .await
                .map_err(|e| listings::failure(&e))?
            {
                let key = (listing.channel, listing.listed.id.clone());
                let product = listing
                    .product
                    .and_then(|id| products.iter().find(|product| product.id == id));
                if let Some(product) = product {
                    names.insert(key.clone(), product.name.clone());
                } else {
                    names
                        .entry(key.clone())
                        .or_insert_with(|| listing.listed.title.clone());
                }
                if listing.listed.status != ListingStatus::Closed
                    && !open
                        .iter()
                        .any(|kept| (kept.channel, kept.listed.id.clone()) == key)
                {
                    open.push(listing);
                }
            }
            let connections = reach.states();
            let mut last_syncs = BTreeMap::new();
            for (kind, _) in &connections {
                if let Some(at) = promotions.last_sync(*kind).await.map_err(why)? {
                    last_syncs.insert(*kind, at);
                }
            }
            let standing = reputation
                .standing()
                .await
                .map_err(|e| crate::reputation::failure(&e))?;
            Ok::<_, String>(Snapshot {
                promotions: promotions.promotions().await.map_err(why)?,
                offers: promotions.offers().await.map_err(why)?,
                listings: open,
                names,
                products,
                minimum: promotions.minimum_margin().await.map_err(why)?,
                unlocks: standing.map(|standing| {
                    standing
                        .tools
                        .iter()
                        .any(|(tool, unlocked)| *tool == SellerTool::Promotions && *unlocked)
                }),
                last_syncs,
                connections,
            })
        });
        cx.spawn_in(window, async move |this, cx| {
            let snapshot = reading.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads != read {
                    return;
                }
                match snapshot {
                    Ok(snapshot) => this.show(snapshot, window, cx),
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn show(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        self.promotions = snapshot.promotions;
        self.offers = snapshot.offers;
        self.listings = snapshot.listings;
        self.names = snapshot.names;
        self.products = snapshot.products;
        self.minimum = Some(snapshot.minimum);
        self.unlocks = snapshot.unlocks;
        self.last_syncs = snapshot.last_syncs;
        self.connections = snapshot.connections;
        self.margins.clear();
        self.refill_listings(window, cx);
        self.read_margins(cx);
    }

    /// The picker offers the open listings on the channel shown.
    fn refill_listings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choices = self
            .listings
            .iter()
            .filter(|listing| listing.channel == self.channel)
            .map(|listing| Choice {
                id: listing.id,
                title: listing_title(&self.names, listing.channel, &listing.listed.id).into(),
            })
            .collect();
        refill(&self.fields.listing, choices, window, cx);
    }

    /// Follows a new Work Context: a Platform's context shows its channel,
    /// closing what was open on another without sending it; All keeps the
    /// channel on screen.
    pub fn follow_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(kind) = preferences::work_context(cx).sales_channel() {
            self.pick_channel(kind, window, cx);
        }
        cx.notify();
    }

    /// Shows the channel `kind`, closing what was open on the one before.
    fn pick_channel(
        &mut self,
        kind: SalesChannelKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.channel == kind {
            return;
        }
        self.channel = kind;
        self.form = None;
        self.pending = None;
        self.blocked = None;
        self.ending = None;
        self.form_outcome = None;
        self.refill_listings(window, cx);
        cx.notify();
    }

    /// The ports of the channel shown, when the app has its adapter.
    fn ports(&self, cx: &App) -> Option<ChannelPorts> {
        sales_channels::of(cx, self.channel)
    }

    /// The channel shown's Promotions port, when it offers Promotions.
    fn promotions_port(&self, cx: &App) -> Option<Arc<dyn mascate_commerce::ChannelPromotions>> {
        self.ports(cx)?.promotions
    }

    /// Works out, on each connected channel's sale fees, the margin each
    /// listing in a Promotion keeps at its lowest price.
    fn read_margins(&mut self, cx: &mut Context<Self>) {
        let (Some(promotions), Some(catalog), Some(taxes)) =
            (promotions(cx), catalog::catalog(cx), pricing::taxes(cx))
        else {
            return;
        };
        let mut items: Vec<(ChannelId, Option<RecordId>, ChannelPorts)> = Vec::new();
        for offer in &self.offers {
            let key = (offer.channel, offer.listed.listing.clone());
            if items.iter().any(|(item, _, _)| *item == key) || !self.connected(offer.channel) {
                continue;
            }
            if let Some(ports) = sales_channels::of(cx, offer.channel) {
                let record = self.listing_record(offer.channel, &offer.listed.listing);
                items.push((key, record, ports));
            }
        }
        if items.is_empty() {
            return;
        }
        let products = self.products.clone();
        let read = self.reads;
        let working = cx.background_executor().spawn(async move {
            let assumptions = pricing::assumptions(&catalog, &taxes).await?;
            let mut margins = BTreeMap::new();
            for (key, listing, ports) in items {
                let margin = promotions
                    .margin_of(&key.1, ports.listings.as_ref(), assumptions)
                    .await
                    .map_err(|error| failure(&error, key.0, listing, &products));
                margins.insert(key, margin);
            }
            Ok::<_, String>(margins)
        });
        cx.spawn(async move |this, cx| {
            let margins = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                match margins {
                    Ok(margins) => this.margins = margins,
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn listing_record(&self, channel: SalesChannelKind, item: &str) -> Option<RecordId> {
        self.listings
            .iter()
            .find(|listing| listing.channel == channel && listing.listed.id == item)
            .map(|listing| listing.id)
    }

    fn connected(&self, kind: SalesChannelKind) -> bool {
        self.connections
            .iter()
            .any(|(each, state)| *each == kind && *state == ConnectionState::Connected)
    }

    /// Whether the owner may create Promotions on the channel shown: on
    /// Mercado Livre only once the Reputation unlocks them.
    fn unlocked(&self) -> bool {
        self.channel != SalesChannelKind::MercadoLivre || self.unlocks == Some(true)
    }

    /// Reads the Promotions from every connected channel now.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(round) = sync_now(cx, true) else {
            return;
        };
        self.syncing = true;
        self.outcome = None;
        cx.spawn(async move |this, cx| {
            let outcome = round.await;
            let _ = this.update(cx, |this, cx| {
                this.syncing = false;
                this.outcome = Some(match &outcome {
                    Ok(Some(report)) => Outcome::Done(synced_text(report).into()),
                    Ok(None) => Outcome::Failed(
                        sales_channels::failure(this.channel, &PlatformError::NotConnected).into(),
                    ),
                    Err(error) => Outcome::Failed(error.clone().into()),
                });
                finished(&outcome, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open_form(&mut self, form: Form, window: &mut Window, cx: &mut Context<Self>) {
        self.form = Some(form);
        self.pending = None;
        self.blocked = None;
        self.ending = None;
        self.form_outcome = None;
        let today = channel_day(Utc::now());
        let (name, starts, ends) = match form {
            Form::Edit(id) => match self.promotion(id) {
                Some(promotion) => (
                    promotion.listed.name.clone(),
                    promotion.listed.starts,
                    promotion.listed.ends,
                ),
                None => return,
            },
            _ => (String::new(), today, today + TimeDelta::days(6)),
        };
        let fields = &self.fields;
        let values = [
            (&fields.name, name),
            (&fields.starts, day_text(starts)),
            (&fields.ends, day_text(ends)),
            (&fields.price, String::new()),
        ];
        for (field, value) in values {
            field.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        if let Form::New(_) | Form::Join(_) = form {
            self.fields
                .listing
                .update(cx, |select, cx| select.set_selected_index(None, window, cx));
        }
        cx.notify();
    }

    fn close_form(&mut self, cx: &mut Context<Self>) {
        self.form = None;
        self.pending = None;
        self.blocked = None;
        self.form_outcome = None;
        cx.notify();
    }

    fn promotion(&self, id: RecordId) -> Option<&Promotion> {
        self.promotions.iter().find(|promotion| promotion.id == id)
    }

    fn picked_listing(&self, cx: &App) -> Option<&Listing> {
        let picked = self.fields.listing.read(cx).selected_value().copied()?;
        self.listings.iter().find(|listing| listing.id == picked)
    }

    fn name_of(&self, listing: &Listing) -> String {
        listing_title(&self.names, listing.channel, &listing.listed.id)
    }

    fn fail_form(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.form_outcome = Some(Outcome::Failed(text.into()));
        cx.notify();
    }

    /// Checks what the form holds: the days, the price, the coupon and the
    /// margin it leaves. Nothing goes to the channel yet.
    fn check(&mut self, cx: &mut Context<Self>) {
        let (Some(form), Some(promotions), Some(ports), Some(catalog), Some(taxes)) = (
            self.form,
            promotions(cx),
            self.ports(cx),
            catalog::catalog(cx),
            pricing::taxes(cx),
        ) else {
            return;
        };
        let on_channel = self.channel;
        let channel = ports.listings;
        let value = |field: &Entity<InputState>| field.read(cx).value().to_string();
        let (Some(starts), Some(ends)) = (
            parse_day(&value(&self.fields.starts)),
            parse_day(&value(&self.fields.ends)),
        ) else {
            return self.fail_form("Digite o primeiro e o último dia como 05/10/2026.", cx);
        };
        let name = value(&self.fields.name).trim().to_owned();
        let unlocks = self.unlocked();
        let days = days_text(starts, ends);
        let products = self.products.clone();
        type Preparing = Task<Result<PromotionRequest, NotPrepared>>;
        let (preparing, summary, listing): (Preparing, String, Option<RecordId>) = match form {
            Form::New(PromotionKind::PriceDiscount) => {
                let Some(listing) = self.picked_listing(cx) else {
                    return self.fail_form("Escolha o anúncio.", cx);
                };
                let currency = listing.listed.price.currency();
                let Some(price) = parse_amount(&value(&self.fields.price)) else {
                    return self.fail_form("Digite o preço da promoção, como 89,90.", cx);
                };
                let price = Money::new(price, currency);
                let summary = format!(
                    "Desconto em {}: de {} por {}, de {days}.",
                    self.name_of(listing),
                    listing.listed.price.to_pt_br(),
                    price.to_pt_br()
                );
                let plan = DiscountPlan {
                    listing: listing.id,
                    price,
                    starts,
                    ends,
                };
                let id = listing.id;
                (
                    cx.background_executor().spawn(async move {
                        let assumptions = pricing::assumptions(&catalog, &taxes)
                            .await
                            .map_err(NotPrepared::Failed)?;
                        promotions
                            .prepare_discount(plan, unlocks, channel.as_ref(), assumptions)
                            .await
                            .map_err(|error| {
                                NotPrepared::from(error, on_channel, Some(id), &products)
                            })
                    }),
                    summary,
                    Some(id),
                )
            }
            Form::New(kind) => {
                let coupon = if kind == PromotionKind::Coupon {
                    match self.coupon_terms(cx) {
                        Some(terms) => Some(terms),
                        None => {
                            return self.fail_form(
                                "Digite o desconto, a compra mínima e o orçamento do cupom em \
                                 números, como 10 ou 49,90.",
                                cx,
                            );
                        }
                    }
                } else {
                    None
                };
                let summary = match &coupon {
                    Some(terms) => format!(
                        "Cupom “{name}”: {}, de {days}. Os anúncios entram nele depois, cada um \
                         com a margem conferida.",
                        coupon_text(terms)
                    ),
                    None => format!(
                        "{} “{name}”, de {days}. Os anúncios entram depois, cada um com seu \
                         preço e a margem conferida.",
                        kind_name(kind, on_channel)
                    ),
                };
                let plan = PromotionPlan {
                    kind,
                    name: name.clone(),
                    starts,
                    ends,
                    coupon,
                };
                let prepared = promotions
                    .prepare_promotion(on_channel, plan, unlocks)
                    .map_err(|error| NotPrepared::from(error, on_channel, None, &products));
                (Task::ready(prepared), summary, None)
            }
            Form::Edit(id) => {
                let kind = self.promotion(id).map_or("Promoção".into(), |promotion| {
                    kind_name(promotion.listed.kind, on_channel)
                });
                let summary = format!("{kind} “{name}” passa a valer de {days}.");
                (
                    cx.background_executor().spawn(async move {
                        promotions
                            .prepare_change(id, &name, starts, ends)
                            .await
                            .map_err(|error| NotPrepared::from(error, on_channel, None, &products))
                    }),
                    summary,
                    None,
                )
            }
            Form::Join(id) => {
                let Some(promotion) = self.promotion(id) else {
                    return self.fail_form("Essa promoção não existe mais; atualize a tela.", cx);
                };
                let Some(listing) = self.picked_listing(cx) else {
                    return self.fail_form("Escolha o anúncio.", cx);
                };
                let (price, summary) = if promotion.listed.kind == PromotionKind::SellerCampaign {
                    let Some(price) = parse_amount(&value(&self.fields.price)) else {
                        return self.fail_form("Digite o preço na promoção, como 89,90.", cx);
                    };
                    let price = Money::new(price, listing.listed.price.currency());
                    let summary = format!(
                        "{} entra {} “{}”: de {} por {}.",
                        self.name_of(listing),
                        in_campaign(on_channel),
                        promotion.listed.name,
                        listing.listed.price.to_pt_br(),
                        price.to_pt_br()
                    );
                    (Some(price), summary)
                } else {
                    let summary = format!(
                        "{} entra no cupom “{}”.",
                        self.name_of(listing),
                        promotion.listed.name
                    );
                    (None, summary)
                };
                let listing = listing.id;
                (
                    cx.background_executor().spawn(async move {
                        let assumptions = pricing::assumptions(&catalog, &taxes)
                            .await
                            .map_err(NotPrepared::Failed)?;
                        promotions
                            .prepare_join(
                                id,
                                listing,
                                price,
                                unlocks,
                                channel.as_ref(),
                                assumptions,
                            )
                            .await
                            .map_err(|error| {
                                NotPrepared::from(error, on_channel, Some(listing), &products)
                            })
                    }),
                    summary,
                    Some(listing),
                )
            }
        };
        self.busy = true;
        self.pending = None;
        self.blocked = None;
        self.form_outcome = None;
        cx.spawn(async move |this, cx| {
            let prepared = preparing.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if this.form != Some(form) {
                    return;
                }
                match prepared {
                    Ok(request) => {
                        this.pending = Some(Pending {
                            request,
                            summary,
                            listing,
                        })
                    }
                    Err(NotPrepared::Below(check)) => {
                        this.blocked = Some(Blocked { check: *check })
                    }
                    Err(NotPrepared::Failed(error)) => {
                        this.form_outcome = Some(Outcome::Failed(error.into()))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The coupon as typed; `None` while a field is not a number.
    fn coupon_terms(&self, cx: &App) -> Option<CouponTerms> {
        let fields = &self.fields;
        let text = |field: &Entity<InputState>| field.read(cx).value().to_string();
        let money = |field: &Entity<InputState>| {
            parse_amount(&text(field)).map(|amount| Money::new(amount, Currency::Brl))
        };
        let discount = if fields.coupon_percent {
            CouponDiscount::Percent(Percentage::parse(&text(&fields.coupon_value))?)
        } else {
            CouponDiscount::Amount(money(&fields.coupon_value)?)
        };
        let maximum_discount = match text(&fields.coupon_maximum).trim() {
            _ if !fields.coupon_percent => None,
            "" => None,
            _ => Some(money(&fields.coupon_maximum)?),
        };
        Some(CouponTerms {
            discount,
            minimum_purchase: money(&fields.coupon_minimum)?,
            maximum_discount,
            budget: money(&fields.coupon_budget)?,
        })
    }

    /// Sends the checked Promotion: only ever on the owner's click.
    fn send(&mut self, cx: &mut Context<Self>) {
        let (Some(pending), Some(promotions), Some(channel)) = (
            self.pending.as_ref(),
            promotions(cx),
            self.promotions_port(cx),
        ) else {
            return;
        };
        let kind = pending.request.channel();
        let confirmed = pending.request.clone().confirm();
        let summary = pending.summary.clone();
        let listing = pending.listing;
        self.busy = true;
        self.form_outcome = None;
        let sending = cx
            .background_executor()
            .spawn(async move { promotions.send(channel.as_ref(), confirmed).await });
        cx.spawn(async move |this, cx| {
            let sent = sending.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match sent {
                    Ok(()) => {
                        this.form = None;
                        this.pending = None;
                        this.outcome = Some(Outcome::Done(
                            format!("Enviado {}. {summary}", to(kind)).into(),
                        ));
                        cx.default_global::<PromotionSyncs>().finished += 1;
                    }
                    Err(error) => {
                        this.form_outcome = Some(Outcome::Failed(
                            failure(&error, kind, listing, &this.products).into(),
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn ask_end(&mut self, ending: Ending, cx: &mut Context<Self>) {
        self.ending = Some(ending);
        cx.notify();
    }

    /// Ends a Promotion or takes a listing out of one: only ever on the
    /// owner's confirmed click.
    fn end(&mut self, ending: Ending, cx: &mut Context<Self>) {
        let (Some(promotions), Some(channel)) = (promotions(cx), self.promotions_port(cx)) else {
            return;
        };
        let kind = self.channel;
        self.busy = true;
        self.outcome = None;
        let ending_it = cx.background_executor().spawn(async move {
            match ending {
                Ending::Offer(offer) => promotions.end_offer(channel.as_ref(), offer).await,
                Ending::Promotion(promotion) => {
                    promotions.end_promotion(channel.as_ref(), promotion).await
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let ended = ending_it.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.ending = None;
                this.outcome = Some(match ended {
                    Ok(()) => {
                        cx.default_global::<PromotionSyncs>().finished += 1;
                        Outcome::Done(format!("Encerrada {}.", on(kind)).into())
                    }
                    Err(error) => {
                        Outcome::Failed(failure(&error, kind, None, &this.products).into())
                    }
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A chip for each Sales Channel the app knows.
    fn render_channels(&self, cx: &mut Context<Self>) -> AnyElement {
        let chips = self
            .connections
            .iter()
            .enumerate()
            .map(|(index, (kind, _))| {
                let kind = *kind;
                let button = Button::new(("promotion-channel", index))
                    .label(kind.name())
                    .small()
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.pick_channel(kind, window, cx)),
                    );
                if kind == self.channel {
                    button.primary()
                } else {
                    button.outline()
                }
            });
        h_flex()
            .flex_wrap()
            .gap_1()
            .children(chips)
            .into_any_element()
    }

    fn render_sync(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let when = match self.last_syncs.get(&self.channel) {
            Some(at) => format!("Lidas em {}", catalog::day_and_time(*at)),
            None => "Promoções ainda não lidas.".into(),
        };
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_xs().text_color(t.text2).child(when))
            .child(
                Button::new("sync-promotions")
                    .label("Atualizar")
                    .icon(IconName::RefreshCw)
                    .primary()
                    .small()
                    .loading(self.syncing)
                    .disabled(self.syncing || !self.connected(self.channel))
                    .on_click(cx.listener(|this, _, _, cx| this.sync(cx))),
            )
            .into_any_element()
    }

    fn render_new(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let unlocked = self.unlocked();
        let channel = self.channel;
        let buttons = promotion_kinds(channel).iter().map(|&kind| {
            let open = self.form == Some(Form::New(kind));
            let button = Button::new(SharedString::from(format!("new-{kind:?}")))
                .label(kind_name(kind, channel))
                .icon(IconName::Plus)
                .small()
                .disabled(!unlocked || self.busy)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_form(Form::New(kind), window, cx)
                }));
            if open {
                button.primary()
            } else {
                button.outline()
            }
        });
        let gate = match self.unlocks {
            _ if channel != SalesChannelKind::MercadoLivre => None,
            Some(true) => None,
            Some(false) => Some(format!(
                "Criar promoções pede {}; sua reputação ainda não chegou lá. Veja em Reputação; \
                 as promoções que já existem continuam abaixo.",
                SellerTool::Promotions.requirement()
            )),
            None => Some(format!(
                "Criar promoções pede {}, e a reputação ainda não foi lida. Conecte a conta e \
                 atualize em Reputação.",
                SellerTool::Promotions.requirement()
            )),
        };
        let minimum = self
            .minimum
            .map(|minimum| minimum_rule(minimum, self.channel));
        let mut body = v_flex()
            .gap_3()
            .child(h_flex().gap_2().flex_wrap().children(buttons))
            .children(gate.map(|gate| kit::info_notice(gate, cx)))
            .children(minimum.map(|text| div().text_xs().text_color(t.text2).child(text)));
        if let Some(form @ Form::New(_)) = self.form {
            body = body.child(self.render_form(form, cx));
        }
        section("Nova promoção", body.into_any_element(), &t)
    }

    fn render_form(&self, form: Form, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let fields = &self.fields;
        let field = |label: &'static str, input: &Entity<InputState>, width: f32| {
            v_flex()
                .gap_1()
                .w(px(width))
                .child(div().text_sm().font_medium().child(label))
                .child(Input::new(input).small())
        };
        let listing_field = || {
            v_flex()
                .gap_1()
                .w(px(320.))
                .child(div().text_sm().font_medium().child("Anúncio"))
                .child(
                    Select::new(&fields.listing)
                        .search_placeholder("Buscar")
                        .small()
                        .placeholder("Anúncio"),
                )
        };
        let kind = match form {
            Form::New(kind) => Some(kind),
            Form::Edit(_) => None,
            Form::Join(id) => self.promotion(id).map(|promotion| promotion.listed.kind),
        };
        let channel = self.channel;
        let max_days = match form {
            Form::New(kind) => kind.max_days(channel),
            Form::Edit(id) => self
                .promotion(id)
                .and_then(|promotion| promotion.listed.kind.max_days(channel)),
            Form::Join(_) => None,
        };
        let mut row = h_flex().flex_wrap().gap_3().items_end();
        match form {
            Form::New(PromotionKind::PriceDiscount) => {
                row = row
                    .child(listing_field())
                    .child(field("Preço na promoção (R$)", &fields.price, 190.))
                    .child(field("Primeiro dia", &fields.starts, 130.))
                    .child(field("Último dia", &fields.ends, 130.));
            }
            Form::New(_) | Form::Edit(_) => {
                row = row
                    .child(field("Nome", &fields.name, 260.))
                    .child(field("Primeiro dia", &fields.starts, 130.))
                    .child(field("Último dia", &fields.ends, 130.));
            }
            Form::Join(_) => {
                row = row.child(listing_field());
                if kind == Some(PromotionKind::SellerCampaign) {
                    row = row.child(field("Preço na promoção (R$)", &fields.price, 190.));
                }
            }
        }
        let coupon = (form == Form::New(PromotionKind::Coupon)).then(|| {
            let percent = fields.coupon_percent;
            let toggle = |label: &'static str, on: bool| {
                let button = Button::new(SharedString::from(format!("coupon-{label}")))
                    .label(label)
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.fields.coupon_percent = on;
                        cx.notify();
                    }));
                if percent == on {
                    button.primary()
                } else {
                    button.outline()
                }
            };
            h_flex()
                .flex_wrap()
                .gap_3()
                .items_end()
                .child(
                    v_flex()
                        .gap_1()
                        .child(div().text_sm().font_medium().child("Desconto em"))
                        .child(
                            h_flex()
                                .gap_1()
                                .child(toggle("R$", false))
                                .child(toggle("%", true)),
                        ),
                )
                .child(field(
                    if percent {
                        "Desconto (%)"
                    } else {
                        "Desconto (R$)"
                    },
                    &fields.coupon_value,
                    130.,
                ))
                .child(field("Compra mínima (R$)", &fields.coupon_minimum, 150.))
                .when(percent, |row| {
                    row.child(field("Desconto máximo (R$)", &fields.coupon_maximum, 160.))
                })
                .child(field("Orçamento (R$)", &fields.coupon_budget, 140.))
        });
        let rule = max_days.map(|days| {
            let mut rule = format!(
                "Até {days} dias, contando o primeiro e o último, em dias de Brasília. Nada vai {} \
                 antes de você conferir e confirmar.",
                to(channel)
            );
            if channel == SalesChannelKind::Shopee {
                rule.push_str(
                    " A Shopee começa um desconto pelo menos uma hora depois de criado, e depois só \
                     deixa encurtá-lo.",
                );
            }
            rule
        });
        let buttons = h_flex()
            .gap_2()
            .child(
                Button::new("check-promotion")
                    .label("Conferir margem")
                    .icon(IconName::Search)
                    .outline()
                    .small()
                    .loading(self.busy && self.pending.is_none())
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.check(cx))),
            )
            .child(
                Button::new("close-promotion")
                    .label("Cancelar")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.close_form(cx))),
            );
        v_flex()
            .gap_3()
            .pt_3()
            .border_t(t.border_width)
            .border_color(t.frame)
            .child(row)
            .children(coupon)
            .children(rule.map(|rule| div().text_xs().text_color(t.text2).child(rule)))
            .child(buttons)
            .children(
                self.pending
                    .as_ref()
                    .map(|pending| self.render_pending(pending, cx)),
            )
            .children(
                self.blocked
                    .as_ref()
                    .map(|blocked| self.render_blocked(blocked, cx)),
            )
            .children(
                self.form_outcome
                    .as_ref()
                    .map(|outcome| notice(outcome, cx)),
            )
            .into_any_element()
    }

    fn render_pending(&self, pending: &Pending, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let margin = pending.request.check().map(|check| {
            let sale = &check.margin.breakdown;
            v_flex()
                .gap_1()
                .child(div().text_sm().text_color(t.success).child(format!(
                    "Margem no pior caso: {} ({}) por unidade, mínima de {}{}.",
                    sale.margin.amount.to_pt_br(),
                    sale.margin.percent_to_pt_br(),
                    check.margin.minimum.to_pt_br(),
                    if check.after_coupon {
                        ", já com o cupom"
                    } else {
                        ""
                    }
                )))
                .child(breakdown_line(sale, cx))
        });
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.accent)
            .child(
                div()
                    .font_medium()
                    .child("Conferido. Confirme para enviar:"),
            )
            .child(div().text_sm().child(pending.summary.clone()))
            .children(margin)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("send-promotion")
                            .label(format!(
                                "Confirmar e enviar {}",
                                to(pending.request.channel())
                            ))
                            .icon(IconName::Check)
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
                    )
                    .child(
                        Button::new("back-promotion")
                            .label("Voltar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pending = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_blocked(&self, blocked: &Blocked, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let check = &blocked.check;
        let sale = &check.margin.breakdown;
        let after = if check.after_coupon {
            " depois do cupom"
        } else {
            ""
        };
        let lowest = match check.margin.lowest {
            Some(lowest) => format!(
                "O menor preço por unidade{after} que mantém a margem mínima é {}.",
                lowest.to_pt_br()
            ),
            None => "Nenhum preço mantém a margem mínima: tarifa, imposto e margem mínima somam \
                     100% ou mais."
                .into(),
        };
        let variation = self
            .products
            .iter()
            .find(|product| product.id == check.margin.product)
            .map(|product| {
                format!(
                    "Conta feita para {}, a variação de menor margem.",
                    product.sku
                )
            });
        let worst = check.after_coupon.then_some(
            "Pior caso do cupom: um comprador leva só este anúncio, nas menos unidades que a \
             compra mínima pede, e o desconto inteiro cai sobre elas.",
        );
        v_flex()
            .gap_1()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.danger)
            .child(div().font_medium().text_color(t.danger).child(format!(
                "Bloqueada: a margem ficaria em {} ({}) por unidade, abaixo da mínima de {}.",
                sale.margin.amount.to_pt_br(),
                sale.margin.percent_to_pt_br(),
                check.margin.minimum.to_pt_br()
            )))
            .child(breakdown_line(sale, cx))
            .child(div().text_sm().child(lowest))
            .children(
                variation
                    .into_iter()
                    .map(SharedString::from)
                    .chain(worst.map(SharedString::from))
                    .map(|text| div().text_xs().text_color(t.text2).child(text)),
            )
            .into_any_element()
    }

    fn render_end_confirm(&self, ending: Ending, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .text_sm()
                    .text_color(t.danger)
                    .child(format!("Encerrar {} agora?", on(self.channel))),
            )
            .child(
                Button::new("confirm-end")
                    .label("Sim, encerrar")
                    .danger()
                    .small()
                    .loading(self.busy)
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, _, cx| this.end(ending, cx))),
            )
            .child(
                Button::new("cancel-end")
                    .label("Cancelar")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ending = None;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_promotions(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let shown: Vec<(usize, &Promotion)> = self
            .promotions
            .iter()
            .enumerate()
            .filter(|(_, promotion)| promotion.channel == self.channel)
            .collect();
        let title = match self.channel {
            SalesChannelKind::Shopee => "Descontos da loja",
            _ => "Campanhas e cupons",
        };
        let body = if shown.is_empty() {
            muted(
                format!(
                    "Nenhuma promoção sua em andamento ou agendada. Crie acima; as que você criar \
                     {} aparecem depois de Atualizar.",
                    on(self.channel)
                ),
                &t,
            )
        } else {
            v_flex()
                .gap_2()
                .children(
                    shown
                        .into_iter()
                        .map(|(index, promotion)| self.render_promotion(index, promotion, cx)),
                )
                .into_any_element()
        };
        section(title, body, &t)
    }

    fn render_promotion(
        &self,
        index: usize,
        promotion: &Promotion,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let id = promotion.id;
        let listed = &promotion.listed;
        let listings = self
            .offers
            .iter()
            .filter(|offer| {
                offer.channel == promotion.channel
                    && offer.listed.promotion.as_deref() == Some(listed.id.as_str())
            })
            .count();
        let mut facts = vec![days_text(listed.starts, listed.ends)];
        if let Some(terms) = &listed.coupon {
            facts.push(coupon_text(terms));
        }
        facts.push(match listings {
            0 => "nenhum anúncio".into(),
            1 => "1 anúncio".into(),
            count => format!("{count} anúncios"),
        });
        let open = matches!(self.form, Some(Form::Edit(form) | Form::Join(form)) if form == id);
        let actions = h_flex()
            .gap_1()
            .flex_none()
            .child(
                Button::new(("join-promotion", index))
                    .label("Adicionar anúncio")
                    .ghost()
                    .small()
                    .disabled(!self.unlocked() || self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_form(Form::Join(id), window, cx)
                    })),
            )
            .child(
                Button::new(("edit-promotion", index))
                    .label("Editar")
                    .ghost()
                    .small()
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_form(Form::Edit(id), window, cx)
                    })),
            )
            .child(
                Button::new(("end-promotion", index))
                    .label("Encerrar")
                    .ghost()
                    .small()
                    .disabled(self.busy)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.ask_end(Ending::Promotion(id), cx)),
                    ),
            );
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(kit::tag(
                        kind_name(listed.kind, promotion.channel),
                        t.accent_text,
                        cx,
                    ))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().font_medium().truncate().child(listed.name.clone()))
                            .child(div().text_xs().text_color(t.text2).child(facts.join(" · "))),
                    )
                    .child(kit::tag(
                        status_name(listed.status),
                        status_ink(listed.status, &t),
                        cx,
                    ))
                    .child(actions),
            )
            .when(self.ending == Some(Ending::Promotion(id)), |card| {
                card.child(self.render_end_confirm(Ending::Promotion(id), cx))
            })
            .when_some(self.form.filter(|_| open), |card, form| {
                card.child(self.render_form(form, cx))
            })
            .into_any_element()
    }

    fn render_offers(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let mut by_listing: Vec<(&str, Vec<(usize, &Offer)>)> = Vec::new();
        for (index, offer) in self.offers.iter().enumerate() {
            if offer.channel != self.channel {
                continue;
            }
            let item = offer.listed.listing.as_str();
            match by_listing.iter_mut().find(|(each, _)| *each == item) {
                Some((_, offers)) => offers.push((index, offer)),
                None => by_listing.push((item, vec![(index, offer)])),
            }
        }
        let body = if by_listing.is_empty() {
            muted("Nenhum anúncio em promoção agora.", &t)
        } else {
            v_flex()
                .gap_2()
                .children(
                    by_listing
                        .into_iter()
                        .map(|(item, offers)| self.render_listing(item, &offers, cx)),
                )
                .into_any_element()
        };
        section("Anúncios em promoção", body, &t)
    }

    fn render_listing(
        &self,
        item: &str,
        offers: &[(usize, &Offer)],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let channel = self.channel;
        let (margin, ink) = match self.margins.get(&(channel, item.to_owned())) {
            Some(Ok(check)) => {
                let sale = &check.margin.breakdown;
                let passes = check.margin.passes();
                (
                    format!(
                        "Margem no pior caso: {} ({}) por unidade a {}{}; mínima de {}.",
                        sale.margin.amount.to_pt_br(),
                        sale.margin.percent_to_pt_br(),
                        sale.sale_price.to_pt_br(),
                        if check.after_coupon {
                            " depois do cupom"
                        } else {
                            ""
                        },
                        check.margin.minimum.to_pt_br()
                    ),
                    if passes { t.success } else { t.danger },
                )
            }
            Some(Err(error)) => (format!("Sem margem: {error}"), t.text2),
            None if self.connected(channel) => ("Calculando a margem…".into(), t.text2),
            None => (
                format!(
                    "A margem é calculada com a tarifa {}; conecte a conta.",
                    of_channel(channel)
                ),
                t.text2,
            ),
        };
        let rows = offers
            .iter()
            .map(|(index, offer)| {
                let listed = &offer.listed;
                let id = offer.id;
                let mut facts = Vec::new();
                if let Some(price) = listed.price {
                    facts.push(format!("por {}", price.to_pt_br()));
                }
                if let Some(ends) = listed.ends {
                    facts.push(format!("até {}", day_text(ends)));
                }
                h_flex()
                    .gap_3()
                    .items_center()
                    .pl_3()
                    .border_l_2()
                    .border_color(t.border)
                    .child(kit::tag(kind_name(listed.kind, channel), t.accent_text, cx))
                    .child(div().flex_1().min_w_0().text_sm().truncate().child(
                        if facts.is_empty() {
                            listed.name.clone()
                        } else {
                            format!("{} · {}", listed.name, facts.join(" · "))
                        },
                    ))
                    .child(kit::tag(
                        status_name(listed.status),
                        status_ink(listed.status, &t),
                        cx,
                    ))
                    .child(if listed.kind.is_own() {
                        Button::new(("end-offer", *index))
                            .label("Encerrar")
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ask_end(Ending::Offer(id), cx)
                            }))
                            .into_any_element()
                    } else {
                        div()
                            .text_xs()
                            .text_color(t.text2)
                            .child(on(channel))
                            .into_any_element()
                    })
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let confirming = offers
            .iter()
            .find(|(_, offer)| self.ending == Some(Ending::Offer(offer.id)))
            .map(|(_, offer)| self.render_end_confirm(Ending::Offer(offer.id), cx));
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(
                v_flex()
                    .gap_0p5()
                    .child(div().font_medium().truncate().child(listing_title(
                        &self.names,
                        channel,
                        item,
                    )))
                    .child(div().text_xs().text_color(ink).child(margin)),
            )
            .children(rows)
            .children(confirming)
            .into_any_element()
    }
}

impl Render for PromotionsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut parts = ScreenParts::new("Promoções");
        if promotions(cx).is_none() || listings(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        let offered = !promotion_kinds(self.channel).is_empty();
        if offered {
            parts.actions.push(self.render_sync(cx));
        }
        if let Some((kind, state)) = self
            .connections
            .iter()
            .find(|(kind, _)| *kind == self.channel)
            .filter(|(_, state)| offered && *state != ConnectionState::Connected)
        {
            parts.notices.push(
                kit::info_notice(sales_channels::not_connected(*kind, state), cx)
                    .into_any_element(),
            );
        }
        parts
            .notices
            .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        // A Platform's context fixes the channel: no tabs to pick another.
        if preferences::work_context(cx).sales_channel().is_none() {
            parts.content.push(self.render_channels(cx));
        }
        if offered {
            parts.content.push(self.render_new(cx));
            parts.content.push(self.render_promotions(cx));
            parts.content.push(self.render_offers(cx));
        } else {
            parts
                .content
                .push(kit::info_notice(no_promotions(self.channel), cx).into_any_element());
        }
        layout::screen(parts, cx)
    }
}

/// One Sync's line for the screen that ran it.
fn synced_text(report: &PromotionSync) -> String {
    let mut text = format!(
        "Promoções lidas: {} campanhas e cupons, {} anúncios em promoção.",
        report.promotions, report.offers
    );
    if report.ended > 0 {
        text.push_str(&format!(
            " {} encerradas desde a última leitura.",
            report.ended
        ));
    }
    text
}

/// "Fone Bluetooth · MLB4100000001".
fn listing_title(
    names: &BTreeMap<ChannelId, String>,
    channel: SalesChannelKind,
    item: &str,
) -> String {
    match names.get(&(channel, item.to_owned())) {
        Some(name) => format!("{name} · {item}"),
        None => format!("Anúncio {item}"),
    }
}

/// "na campanha", "no desconto": where a listing goes in a campaign of the
/// owner's.
fn in_campaign(channel: SalesChannelKind) -> &'static str {
    match channel {
        SalesChannelKind::Shopee => "no desconto",
        _ => "na campanha",
    }
}

/// Why the channel shows no Promotions, and where the owner runs them.
fn no_promotions(channel: SalesChannelKind) -> String {
    format!(
        "{} não oferece promoções de vendedor pela API: crie cupons e ofertas no painel {} se \
         quiser. Os preços dos seus anúncios lá seguem conferidos pela margem em Anúncios e \
         Preços.",
        opening(channel),
        of_channel(channel)
    )
}

fn status_ink(status: PromotionStatus, t: &Tokens) -> gpui_kit::Hsla {
    match status {
        PromotionStatus::Pending => t.text2,
        PromotionStatus::Started => t.success,
    }
}

fn section(title: &str, body: AnyElement, t: &Tokens) -> AnyElement {
    v_flex()
        .gap_2()
        .child(kit::section_heading(title.to_owned()))
        .child(
            div()
                .p_3()
                .rounded(t.radius_lg)
                .border(t.border_width)
                .border_color(t.frame)
                .bg(t.surface)
                .child(body),
        )
        .into_any_element()
}

fn muted(text: impl Into<SharedString>, t: &Tokens) -> AnyElement {
    div()
        .text_sm()
        .text_color(t.text2)
        .child(text.into())
        .into_any_element()
}

/// The Margin Guard's rule, naming the coupon only where the channel has coupons.
fn minimum_rule(minimum: Percentage, channel: SalesChannelKind) -> String {
    let after_coupon = if promotion_kinds(channel).contains(&PromotionKind::Coupon) {
        ", depois do cupom,"
    } else {
        ""
    };
    format!(
        "Margem mínima: {}. Nenhuma promoção passa se uma unidade vendida no menor \
         preço{after_coupon} ficar com margem abaixo dela. Mude em Configurações › Vendas › \
         Preços e margens.",
        minimum.to_pt_br()
    )
}

#[cfg(test)]
mod tests {
    use mascate_kernel::Currency;
    use rust_decimal::Decimal;

    use super::*;

    fn brl(amount: i64) -> Money {
        Money::new(Decimal::from(amount), Currency::Brl)
    }

    #[test]
    fn the_minimum_rule_names_the_coupon_only_where_the_channel_has_coupons() {
        let minimum = Percentage::new(10.into()).unwrap();
        assert!(
            minimum_rule(minimum, SalesChannelKind::MercadoLivre)
                .contains("no menor preço, depois do cupom, ficar")
        );
        assert!(
            minimum_rule(minimum, SalesChannelKind::Shopee)
                .contains("no menor preço ficar com margem abaixo dela")
        );
    }

    #[test]
    fn a_coupon_reads_as_the_owner_typed_it() {
        let fixed = CouponTerms {
            discount: CouponDiscount::Amount(brl(10)),
            minimum_purchase: brl(50),
            maximum_discount: None,
            budget: brl(200),
        };
        assert_eq!(
            coupon_text(&fixed),
            "R$ 10,00 de desconto em compras a partir de R$ 50,00 · orçamento R$ 200,00"
        );
        let capped = CouponTerms {
            discount: CouponDiscount::Percent(Percentage::new(15.into()).unwrap()),
            maximum_discount: Some(brl(20)),
            ..fixed
        };
        assert_eq!(
            coupon_text(&capped),
            "15% de desconto, até R$ 20,00 em compras a partir de R$ 50,00 · orçamento R$ 200,00"
        );
    }

    #[test]
    fn a_listing_reads_by_its_product_and_id() {
        let ml = SalesChannelKind::MercadoLivre;
        let names = BTreeMap::from([((ml, "MLB1".to_owned()), "Fone".to_owned())]);
        assert_eq!(listing_title(&names, ml, "MLB1"), "Fone · MLB1");
        assert_eq!(listing_title(&names, ml, "MLB2"), "Anúncio MLB2");
        assert_eq!(
            listing_title(&names, SalesChannelKind::Shopee, "MLB1"),
            "Anúncio MLB1"
        );
    }

    #[test]
    fn each_channel_names_its_promotions_in_its_own_words() {
        assert_eq!(
            kind_name(PromotionKind::SellerCampaign, SalesChannelKind::Shopee),
            "Desconto da loja"
        );
        assert_eq!(
            kind_name(PromotionKind::ChannelCampaign, SalesChannelKind::Shopee),
            "Campanha da Shopee"
        );
        assert_eq!(
            no_promotions(SalesChannelKind::Magalu),
            "A Magalu não oferece promoções de vendedor pela API: crie cupons e ofertas no \
             painel da Magalu se quiser. Os preços dos seus anúncios lá seguem conferidos pela \
             margem em Anúncios e Preços."
        );
    }
}
