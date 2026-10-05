//! Promoções (#26): the owner's Promotions on Mercado Livre, a listing's
//! own price discount, a campaign of theirs and a coupon, with the margin
//! guard that refuses any that would leave a sale below the minimum margin
//! and shows the calculation. Creating one needs the Reputation that
//! unlocks Promotions (from Marketing); each is checked, shown and sent
//! only once the owner confirms it. The active ones are listed by listing,
//! with their last day and the margin left in the worst case. Read hourly
//! after an Order Sync and on this screen's button. The rules live in
//! Commerce (ADR 0024).

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
    PromotionStatus, PromotionSync, Promotions,
};
use mascate_integrations::{Connection, ConnectionState};
use mascate_kernel::{
    Currency, Money, Percentage, PlatformError, RecordId, Timestamp, channel_day, parse_amount,
};
use mascate_marketing::SellerTool;

use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE};
use crate::connections::AppConnections;
use crate::forms::{Choice, Outcome, Picker, day_text, input, notice, parse_day, picker, refill};
use crate::kit;
use crate::layout;
use crate::listings::{self, breakdown_line, listings, price_failure};
use crate::mercado_livre;
use crate::parts::ScreenParts;
use crate::pricing;
use crate::reputation::reputation;

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

/// Why a Promotion was not checked, sent or read, as the owner reads it.
/// `listing` is the one being priced, when there is one.
pub(crate) fn failure(
    error: &PromotionError,
    listing: Option<RecordId>,
    products: &[Product],
) -> String {
    match error {
        PromotionError::Locked => format!(
            "Promoções pedem {}; sua reputação ainda não chegou lá. Veja em Reputação.",
            SellerTool::Promotions.requirement()
        ),
        PromotionError::Listing(error) => listings::failure(error),
        PromotionError::Closed => "O anúncio está encerrado no Mercado Livre.".into(),
        PromotionError::UnknownPromotion(_) | PromotionError::UnknownOffer(_) => {
            "Essa promoção não existe mais; atualize a tela.".into()
        }
        PromotionError::NotOwn => "Campanhas do Mercado Livre se aceitam e se encerram no próprio \
             Mercado Livre."
            .into(),
        PromotionError::NoName => "Dê um nome à promoção.".into(),
        PromotionError::EndsBeforeStart => "O último dia vem antes do primeiro.".into(),
        PromotionError::StartsInPast => "O primeiro dia já passou; comece hoje ou depois.".into(),
        PromotionError::AlreadyStarted => {
            "A promoção já começou: o primeiro dia fica como está.".into()
        }
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
        PromotionError::InvalidMinimum => "A margem mínima vai de 0 até menos de 100%.".into(),
        PromotionError::Sending => {
            "Essa promoção já está a caminho do Mercado Livre; aguarde a resposta.".into()
        }
        PromotionError::Pricing(error) => match listing {
            Some(listing) => price_failure(error, listing, products),
            None => format!("Não consegui calcular a margem: {error}"),
        },
        PromotionError::Platform(error) => mercado_livre::failure(error),
        PromotionError::Currencies(_) => "Os valores estão em moedas diferentes.".into(),
        PromotionError::Unreadable(_) | PromotionError::Sql(_) => {
            format!("Não consegui ler ou gravar as promoções no banco: {error}")
        }
    }
}

/// Runs a Sync of the Promotions off the UI thread: when `forced`, or when
/// the last one is an hour old. `Ok(None)` when the Connection is not up or
/// nothing was due.
pub fn sync_now(cx: &mut App, forced: bool) -> Option<Task<Result<Option<PromotionSync>, String>>> {
    let (Some(promotions), Some(channel)) = (promotions(cx), mercado_livre::adapter(cx)) else {
        return None;
    };
    let connections = cx.global::<AppConnections>().0.clone();
    Some(cx.background_executor().spawn(async move {
        if connections.state(Connection::MercadoLivre) != ConnectionState::Connected {
            return Ok(None);
        }
        let why = |error: PromotionError| failure(&error, None, &[]);
        if !forced && !promotions.is_due().await.map_err(why)? {
            return Ok(None);
        }
        promotions
            .sync(channel.as_ref())
            .await
            .map(Some)
            .map_err(why)
    }))
}

/// Tells the screen a Sync ran.
pub fn finished(outcome: &Result<Option<PromotionSync>, String>, cx: &mut App) {
    if let Err(error) = outcome {
        eprintln!("could not sync the promotions: {error}");
    }
    cx.default_global::<PromotionSyncs>().finished += 1;
}

pub(crate) fn kind_name(kind: PromotionKind) -> &'static str {
    match kind {
        PromotionKind::PriceDiscount => "Desconto no anúncio",
        PromotionKind::SellerCampaign => "Campanha do vendedor",
        PromotionKind::Coupon => "Cupom",
        PromotionKind::ChannelCampaign => "Campanha do Mercado Livre",
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
    /// What goes to Mercado Livre, in words.
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
    fn from(error: PromotionError, listing: Option<RecordId>, products: &[Product]) -> Self {
        match error {
            PromotionError::BelowMinimum(check) => NotPrepared::Below(check),
            error => NotPrepared::Failed(failure(&error, listing, products)),
        }
    }
}

/// An "Encerrar" waiting for its confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    Offer(RecordId),
    Promotion(RecordId),
}

/// Everything the screen shows, read in one go off the UI thread.
struct Snapshot {
    promotions: Vec<Promotion>,
    offers: Vec<Offer>,
    listings: Vec<Listing>,
    names: BTreeMap<String, String>,
    products: Vec<Product>,
    minimum: Percentage,
    unlocks: Option<bool>,
    last_sync: Option<Timestamp>,
    connection: ConnectionState,
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
    promotions: Vec<Promotion>,
    offers: Vec<Offer>,
    /// The open listings, one variation each, for the pickers.
    listings: Vec<Listing>,
    names: BTreeMap<String, String>,
    products: Vec<Product>,
    minimum: Option<Percentage>,
    /// Whether the Reputation unlocks Promotions; `None` before it is read.
    unlocks: Option<bool>,
    last_sync: Option<Timestamp>,
    connection: Option<ConnectionState>,
    /// The worst-case margin of each listing in a Promotion, by its id on
    /// the channel, or why there is none.
    margins: BTreeMap<String, Result<PromotionCheck, String>>,
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
            promotions: Vec::new(),
            offers: Vec::new(),
            listings: Vec::new(),
            names: BTreeMap::new(),
            products: Vec::new(),
            minimum: None,
            unlocks: None,
            last_sync: None,
            connection: None,
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
        let connections = cx.global::<AppConnections>().0.clone();
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            let why = |error: PromotionError| failure(&error, None, &[]);
            let names = listings::listing_products(&listings, &catalog)
                .await?
                .into_iter()
                .map(|(listing, sold)| (listing, sold.name))
                .collect();
            let mut open: Vec<Listing> = Vec::new();
            for listing in listings
                .listings()
                .await
                .map_err(|e| listings::failure(&e))?
            {
                if listing.listed.status != ListingStatus::Closed
                    && !open.iter().any(|kept| kept.listed.id == listing.listed.id)
                {
                    open.push(listing);
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
                products: catalog.products().await.map_err(|e| catalog::failure(&e))?,
                minimum: promotions.minimum_margin().await.map_err(why)?,
                unlocks: standing.map(|standing| {
                    standing
                        .tools
                        .iter()
                        .any(|(tool, unlocked)| *tool == SellerTool::Promotions && *unlocked)
                }),
                last_sync: promotions.last_sync().await.map_err(why)?,
                connection: connections.state(Connection::MercadoLivre),
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
        let choices = snapshot
            .listings
            .iter()
            .map(|listing| Choice {
                id: listing.id,
                title: listing_title(&snapshot.names, &listing.listed.id).into(),
            })
            .collect();
        refill(&self.fields.listing, choices, window, cx);
        self.promotions = snapshot.promotions;
        self.offers = snapshot.offers;
        self.listings = snapshot.listings;
        self.names = snapshot.names;
        self.products = snapshot.products;
        self.minimum = Some(snapshot.minimum);
        self.unlocks = snapshot.unlocks;
        self.last_sync = snapshot.last_sync;
        self.connection = Some(snapshot.connection);
        self.margins.clear();
        self.read_margins(cx);
    }

    /// Works out, on Mercado Livre's sale fees, the margin each listing in
    /// a Promotion keeps at its lowest price.
    fn read_margins(&mut self, cx: &mut Context<Self>) {
        let (Some(promotions), Some(channel), Some(catalog), Some(taxes)) = (
            promotions(cx),
            mercado_livre::adapter(cx),
            catalog::catalog(cx),
            pricing::taxes(cx),
        ) else {
            return;
        };
        if !self.connected() {
            return;
        }
        let mut items: Vec<(String, Option<RecordId>)> = Vec::new();
        for offer in &self.offers {
            if !items.iter().any(|(item, _)| *item == offer.listed.listing) {
                items.push((
                    offer.listed.listing.clone(),
                    self.listing_record(&offer.listed.listing),
                ));
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
            for (item, listing) in items {
                let margin = promotions
                    .margin_of(&item, channel.as_ref(), assumptions)
                    .await
                    .map_err(|error| failure(&error, listing, &products));
                margins.insert(item, margin);
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

    fn listing_record(&self, item: &str) -> Option<RecordId> {
        self.listings
            .iter()
            .find(|listing| listing.listed.id == item)
            .map(|listing| listing.id)
    }

    fn connected(&self) -> bool {
        self.connection == Some(ConnectionState::Connected)
    }

    fn unlocked(&self) -> bool {
        self.unlocks == Some(true)
    }

    /// Reads the Promotions from Mercado Livre now.
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
                    Ok(None) => {
                        Outcome::Failed(mercado_livre::failure(&PlatformError::NotConnected).into())
                    }
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
        listing_title(&self.names, &listing.listed.id)
    }

    fn fail_form(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.form_outcome = Some(Outcome::Failed(text.into()));
        cx.notify();
    }

    /// Checks what the form holds: the days, the price, the coupon and the
    /// margin it leaves. Nothing goes to Mercado Livre yet.
    fn check(&mut self, cx: &mut Context<Self>) {
        let (Some(form), Some(promotions), Some(channel), Some(catalog), Some(taxes)) = (
            self.form,
            promotions(cx),
            mercado_livre::adapter(cx),
            catalog::catalog(cx),
            pricing::taxes(cx),
        ) else {
            return;
        };
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
                            .map_err(|error| NotPrepared::from(error, Some(id), &products))
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
                        "Campanha “{name}”, de {days}. Os anúncios entram nela depois, cada um \
                         com seu preço e a margem conferida."
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
                    .prepare_promotion(plan, unlocks)
                    .map_err(|error| NotPrepared::from(error, None, &products));
                (Task::ready(prepared), summary, None)
            }
            Form::Edit(id) => {
                let kind = self
                    .promotion(id)
                    .map_or("Promoção", |promotion| kind_name(promotion.listed.kind));
                let summary = format!("{kind} “{name}” passa a valer de {days}.");
                (
                    cx.background_executor().spawn(async move {
                        promotions
                            .prepare_change(id, &name, starts, ends)
                            .await
                            .map_err(|error| NotPrepared::from(error, None, &products))
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
                        return self.fail_form("Digite o preço na campanha, como 89,90.", cx);
                    };
                    let price = Money::new(price, listing.listed.price.currency());
                    let summary = format!(
                        "{} entra na campanha “{}”: de {} por {}.",
                        self.name_of(listing),
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
                            .map_err(|error| NotPrepared::from(error, Some(listing), &products))
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
            mercado_livre::adapter(cx),
        ) else {
            return;
        };
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
                            format!("Enviado ao Mercado Livre. {summary}").into(),
                        ));
                        cx.default_global::<PromotionSyncs>().finished += 1;
                    }
                    Err(error) => {
                        this.form_outcome = Some(Outcome::Failed(
                            failure(&error, listing, &this.products).into(),
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
        let (Some(promotions), Some(channel)) = (promotions(cx), mercado_livre::adapter(cx)) else {
            return;
        };
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
                        Outcome::Done("Encerrada no Mercado Livre.".into())
                    }
                    Err(error) => Outcome::Failed(failure(&error, None, &this.products).into()),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_sync(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let when = match self.last_sync {
            Some(at) => format!("Lidas em {}", catalog::day_and_time(at)),
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
                    .disabled(self.syncing || !self.connected())
                    .on_click(cx.listener(|this, _, _, cx| this.sync(cx))),
            )
            .into_any_element()
    }

    fn render_new(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let unlocked = self.unlocked();
        let kinds = [
            PromotionKind::PriceDiscount,
            PromotionKind::SellerCampaign,
            PromotionKind::Coupon,
        ];
        let buttons = kinds.into_iter().map(|kind| {
            let open = self.form == Some(Form::New(kind));
            let button = Button::new(SharedString::from(format!("new-{kind:?}")))
                .label(kind_name(kind))
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
        let minimum = self.minimum.map(|minimum| {
            format!(
                "Margem mínima: {}. Nenhuma promoção passa se uma unidade vendida no menor preço, \
                 depois do cupom, ficar com margem abaixo dela. Mude em Configurações › Preços e \
                 margens.",
                minimum.to_pt_br()
            )
        });
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
        let max_days = match form {
            Form::New(kind) => kind.max_days(),
            Form::Edit(id) => self
                .promotion(id)
                .and_then(|promotion| promotion.listed.kind.max_days()),
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
                    row = row.child(field("Preço na campanha (R$)", &fields.price, 190.));
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
            format!(
                "Até {days} dias, contando o primeiro e o último, em dias de Brasília. Nada vai ao \
                 Mercado Livre antes de você conferir e confirmar."
            )
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
                            .label("Confirmar e enviar ao ML")
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
                    .child("Encerrar no Mercado Livre agora?"),
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
        let body = if self.promotions.is_empty() {
            muted(
                "Nenhuma campanha ou cupom seu em andamento ou agendado. Crie acima; os que você \
                 criar no Mercado Livre aparecem depois de Atualizar.",
                &t,
            )
        } else {
            v_flex()
                .gap_2()
                .children(
                    self.promotions
                        .iter()
                        .enumerate()
                        .map(|(index, promotion)| self.render_promotion(index, promotion, cx)),
                )
                .into_any_element()
        };
        section("Campanhas e cupons", body, &t)
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
            .filter(|offer| offer.listed.promotion.as_deref() == Some(listed.id.as_str()))
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
                    .child(kit::tag(kind_name(listed.kind), t.accent_text, cx))
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
        let (margin, ink) = match self.margins.get(item) {
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
            None if self.connected() => ("Calculando a margem…".into(), t.text2),
            None => (
                "A margem é calculada com a tarifa do Mercado Livre; conecte a conta.".into(),
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
                    .child(kit::tag(kind_name(listed.kind), t.accent_text, cx))
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
                            .child("no Mercado Livre")
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
                    .child(
                        div()
                            .font_medium()
                            .truncate()
                            .child(listing_title(&self.names, item)),
                    )
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
        parts.actions.push(self.render_sync(cx));
        if let Some(state) = &self.connection
            && *state != ConnectionState::Connected
        {
            parts
                .notices
                .push(kit::info_notice(mercado_livre::not_connected(state), cx).into_any_element());
        }
        parts
            .notices
            .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        parts.content.push(self.render_new(cx));
        parts.content.push(self.render_promotions(cx));
        parts.content.push(self.render_offers(cx));
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
fn listing_title(names: &BTreeMap<String, String>, item: &str) -> String {
    match names.get(item) {
        Some(name) => format!("{name} · {item}"),
        None => format!("Anúncio {item}"),
    }
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

#[cfg(test)]
mod tests {
    use mascate_kernel::Currency;
    use rust_decimal::Decimal;

    use super::*;

    fn brl(amount: i64) -> Money {
        Money::new(Decimal::from(amount), Currency::Brl)
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
        let names = BTreeMap::from([("MLB1".to_owned(), "Fone".to_owned())]);
        assert_eq!(listing_title(&names, "MLB1"), "Fone · MLB1");
        assert_eq!(listing_title(&names, "MLB2"), "Anúncio MLB2");
    }
}
