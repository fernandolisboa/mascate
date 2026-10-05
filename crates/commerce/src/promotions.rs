//! Promotions (#26): the owner's own discounts, campaigns and coupons on the
//! Sales Channel, created, changed and ended from the app, and the margin
//! each listing keeps while they run. A Promotion that would leave a sale
//! below the minimum margin is refused before anything reaches the channel,
//! and creating one waits for the Reputation the channel asks for, which
//! the caller reads from marketing (ADR 0024).

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use chrono::{NaiveDate, TimeDelta};
use libsql::{Connection, Row, TransactionBehavior, params};
use mascate_inventory::Inventory;
use mascate_kernel::{
    Clock, Currency, CurrencyMismatch, IdGenerator, Money, Percentage, PlatformError, RecordId,
    Timestamp,
};
use mascate_platform::{
    Database, Migration, StoredRow, StoredValueError, load_single_row, save_single_row, stored,
};
use rust_decimal::Decimal;

use crate::pricing::check_target;
use crate::{
    ListingError, ListingStatus, Listings, MarginCheck, PriceAssumptions, Pricing, PricingError,
    SalesChannel,
};

/// The minimum margin until the owner sets another.
const DEFAULT_MINIMUM_MARGIN: Decimal = Decimal::from_parts(10, 0, 0, false, 0);

/// Minutes between two reads of the Promotions, after the first.
pub const PROMOTIONS_REFRESH_MINUTES: i64 = 60;

/// Hours Brasília time is behind UTC: Mercado Livre Brasil counts a
/// Promotion's days there, and Brazil has had no daylight saving since 2019.
const CHANNEL_HOURS_BEHIND_UTC: i64 = 3;

pub(crate) const CREATE_PROMOTIONS: Migration = Migration {
    version: 9,
    name: "create promotions",
    risky: false,
    sql: "CREATE TABLE commerce_promotion_settings (
        id             TEXT PRIMARY KEY,
        minimum_margin TEXT NOT NULL,
        created_at     TEXT NOT NULL,
        updated_at     TEXT NOT NULL,
        deleted_at     TEXT
    );
    CREATE TABLE commerce_promotion_syncs (
        id         TEXT PRIMARY KEY,
        synced_at  TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE TABLE commerce_promotions (
        id               TEXT PRIMARY KEY,
        ml_promotion_id  TEXT NOT NULL UNIQUE,
        kind             TEXT NOT NULL,
        name             TEXT NOT NULL,
        starts_on        TEXT NOT NULL,
        ends_on          TEXT NOT NULL,
        status           TEXT NOT NULL,
        coupon_kind      TEXT,
        coupon_value     TEXT,
        coupon_minimum   TEXT,
        coupon_maximum   TEXT,
        coupon_budget    TEXT,
        currency         TEXT,
        synced_at        TEXT NOT NULL,
        created_at       TEXT NOT NULL,
        updated_at       TEXT NOT NULL,
        deleted_at       TEXT
    );
    CREATE TABLE commerce_promotion_offers (
        id              TEXT PRIMARY KEY,
        ml_item_id      TEXT NOT NULL,
        kind            TEXT NOT NULL,
        ml_promotion_id TEXT NOT NULL,
        name            TEXT NOT NULL,
        status          TEXT NOT NULL,
        price           TEXT,
        currency        TEXT,
        starts_on       TEXT,
        ends_on         TEXT,
        synced_at       TEXT NOT NULL,
        created_at      TEXT NOT NULL,
        updated_at      TEXT NOT NULL,
        deleted_at      TEXT,
        UNIQUE (ml_item_id, kind, ml_promotion_id)
    );",
};

const SETTINGS_TABLE: &str = "commerce_promotion_settings";
const SETTINGS_COLUMNS: &[&str] = &["minimum_margin"];
const SYNCS_TABLE: &str = "commerce_promotion_syncs";
const SYNCS_COLUMNS: &[&str] = &["synced_at"];

/// The day it is on the Sales Channel at `at`.
pub fn channel_day(at: Timestamp) -> NaiveDate {
    (at - TimeDelta::hours(CHANNEL_HOURS_BEHIND_UTC)).date_naive()
}

/// What sort of Promotion it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PromotionKind {
    /// One listing's own lower price for a few days.
    PriceDiscount,
    /// A campaign of the owner's: the listings in it each have a lower price.
    SellerCampaign,
    /// A coupon of the owner's, taken off at checkout from purchases above a
    /// minimum.
    Coupon,
    /// A campaign the channel invited the listing to and the owner joined on
    /// the channel: the app shows it and counts its price, but does not
    /// create, change or end it.
    ChannelCampaign,
}

impl PromotionKind {
    const ALL: [PromotionKind; 4] = [
        PromotionKind::PriceDiscount,
        PromotionKind::SellerCampaign,
        PromotionKind::Coupon,
        PromotionKind::ChannelCampaign,
    ];

    fn code(self) -> &'static str {
        match self {
            PromotionKind::PriceDiscount => "price_discount",
            PromotionKind::SellerCampaign => "seller_campaign",
            PromotionKind::Coupon => "coupon",
            PromotionKind::ChannelCampaign => "channel_campaign",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.code() == code)
    }

    /// The most days it may run, its first and last included: 14 for a
    /// discount or campaign, 31 for a coupon.
    pub fn max_days(self) -> Option<i64> {
        match self {
            PromotionKind::PriceDiscount | PromotionKind::SellerCampaign => Some(14),
            PromotionKind::Coupon => Some(31),
            PromotionKind::ChannelCampaign => None,
        }
    }

    /// Whether the owner creates, changes and ends it from the app.
    pub fn is_own(self) -> bool {
        self != PromotionKind::ChannelCampaign
    }
}

/// Where a Promotion stands on the channel. Finished ones are not kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromotionStatus {
    /// Accepted, starting on its first day.
    Pending,
    /// Running.
    Started,
}

impl PromotionStatus {
    fn code(self) -> &'static str {
        match self {
            PromotionStatus::Pending => "pending",
            PromotionStatus::Started => "started",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        [PromotionStatus::Pending, PromotionStatus::Started]
            .into_iter()
            .find(|status| status.code() == code)
    }
}

/// What a coupon takes off a purchase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CouponDiscount {
    Amount(Money),
    Percent(Percentage),
}

/// A coupon's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CouponTerms {
    pub discount: CouponDiscount,
    /// The smallest purchase it applies to.
    pub minimum_purchase: Money,
    /// The most a percentage coupon takes off one purchase.
    pub maximum_discount: Option<Money>,
    /// What the owner pays for all its uses together; it ends once spent.
    pub budget: Money,
}

impl CouponTerms {
    /// What one unit sold at `price` comes to after the coupon, in the worst
    /// case for the owner: a buyer buys only this listing, in the fewest
    /// units the minimum purchase takes, so the whole discount falls on
    /// them.
    pub fn unit_price(&self, price: Money) -> Result<Money, CurrencyMismatch> {
        let units = if price.amount() > Decimal::ZERO {
            (self.minimum_purchase.amount() / price.amount())
                .ceil()
                .max(Decimal::ONE)
        } else {
            Decimal::ONE
        };
        let off = match self.discount {
            CouponDiscount::Amount(amount) => {
                same_currency(amount, price)?;
                amount.amount() / units
            }
            CouponDiscount::Percent(rate) => {
                let share = rate.of(price).amount();
                match self.maximum_discount {
                    Some(cap) => {
                        same_currency(cap, price)?;
                        share.min(cap.amount() / units)
                    }
                    None => share,
                }
            }
        };
        Ok(Money::new(price.amount() - off, price.currency()))
    }

    fn check(&self, currency: Currency) -> Result<(), PromotionError> {
        let positive =
            |money: Money| money.currency() == currency && money.amount() > Decimal::ZERO;
        let discount = match self.discount {
            CouponDiscount::Amount(amount) => positive(amount),
            CouponDiscount::Percent(rate) => {
                rate.percent() > Decimal::ZERO && rate.percent() < Decimal::ONE_HUNDRED
            }
        };
        if discount
            && positive(self.minimum_purchase)
            && positive(self.budget)
            && self.maximum_discount.is_none_or(positive)
        {
            Ok(())
        } else {
            Err(PromotionError::InvalidCoupon)
        }
    }
}

fn same_currency(one: Money, other: Money) -> Result<(), CurrencyMismatch> {
    one.checked_add(Money::zero(other.currency())).map(|_| ())
}

/// A campaign or coupon of the owner's, as the channel reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelPromotion {
    /// The channel's id.
    pub id: String,
    /// [`PromotionKind::SellerCampaign`] or [`PromotionKind::Coupon`].
    pub kind: PromotionKind,
    pub name: String,
    pub starts: NaiveDate,
    pub ends: NaiveDate,
    pub status: PromotionStatus,
    /// A coupon's terms; `None` for a campaign, or a coupon whose terms the
    /// channel did not say.
    pub coupon: Option<CouponTerms>,
}

/// A listing in a Promotion, as the channel reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelOffer {
    /// The channel's id of the listing; its variations share the offer.
    pub listing: String,
    pub kind: PromotionKind,
    /// The channel's id of the Promotion; `None` for a price discount,
    /// which belongs to the listing alone.
    pub promotion: Option<String>,
    pub name: String,
    pub status: PromotionStatus,
    /// What a buyer pays during it; `None` for a coupon, taken at checkout.
    pub price: Option<Money>,
    pub starts: Option<NaiveDate>,
    pub ends: Option<NaiveDate>,
}

/// A campaign or coupon to create on the channel, or what an existing one
/// changes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionPlan {
    pub kind: PromotionKind,
    pub name: String,
    pub starts: NaiveDate,
    pub ends: NaiveDate,
    /// The terms of a coupon; `None` for a campaign. A change keeps them.
    pub coupon: Option<CouponTerms>,
}

/// A listing to put in a Promotion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferToJoin {
    pub kind: PromotionKind,
    /// The Promotion it joins; `None` for a price discount.
    pub promotion: Option<String>,
    /// The listing's price during it; `None` for a coupon.
    pub price: Option<Money>,
    /// A price discount's own days; a campaign's or coupon's are its own.
    pub days: Option<(NaiveDate, NaiveDate)>,
}

/// A listing's price discount the owner asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscountPlan {
    pub listing: RecordId,
    pub price: Money,
    pub starts: NaiveDate,
    pub ends: NaiveDate,
}

/// The owner's Promotions on the Sales Channel, as a Platform's adapter
/// reports and changes them. Calls block on the network, so they run off
/// the UI thread.
pub trait ChannelPromotions: Send + Sync {
    /// The owner's campaigns and coupons that have not finished.
    fn promotions(&self) -> Result<Vec<ChannelPromotion>, PlatformError>;

    /// The Promotions the listing `listing` is in, running or about to.
    fn offers(&self, listing: &str) -> Result<Vec<ChannelOffer>, PlatformError>;

    /// Creates a campaign or coupon.
    fn create(&self, plan: &PromotionPlan) -> Result<ChannelPromotion, PlatformError>;

    /// Changes the name and days of the campaign or coupon `id`.
    fn change(&self, id: &str, plan: &PromotionPlan) -> Result<(), PlatformError>;

    /// Ends the campaign or coupon `id`, taking every listing out of it.
    fn end(&self, id: &str, kind: PromotionKind) -> Result<(), PlatformError>;

    /// Puts the listing `listing` in a Promotion.
    fn join(&self, listing: &str, offer: &OfferToJoin) -> Result<(), PlatformError>;

    /// Takes the listing `listing` out of a Promotion.
    fn leave(
        &self,
        listing: &str,
        kind: PromotionKind,
        promotion: Option<&str>,
    ) -> Result<(), PlatformError>;
}

/// A campaign or coupon of the owner's, as last synced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promotion {
    pub id: RecordId,
    pub listed: ChannelPromotion,
    pub synced_at: Timestamp,
}

/// A listing in a Promotion, as last synced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub id: RecordId,
    pub listed: ChannelOffer,
    pub synced_at: Timestamp,
}

/// The margin a Promotion leaves, and whether a coupon lowers the price it
/// was worked out at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromotionCheck {
    /// One unit at the lowest price it sells for while the listing's
    /// Promotions run, this one included.
    pub margin: MarginCheck,
    /// A coupon takes that price below the lowest price listed: the price
    /// and the lowest price are after the coupon.
    pub after_coupon: bool,
}

/// What a Sync of Promotions did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PromotionSync {
    /// Campaigns and coupons not finished.
    pub promotions: usize,
    /// Listings in a Promotion, one per listing and Promotion.
    pub offers: usize,
    /// Promotions and offers that ended since the last Sync.
    pub ended: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum PromotionError {
    #[error("the Reputation does not unlock Promotions yet")]
    Locked,
    #[error(transparent)]
    Listing(#[from] ListingError),
    #[error("the listing is closed in the Sales Channel")]
    Closed,
    #[error("no Promotion {0}")]
    UnknownPromotion(RecordId),
    #[error("no offer {0}")]
    UnknownOffer(RecordId),
    #[error("the channel's own campaigns are joined and left on the channel")]
    NotOwn,
    #[error("a Promotion needs a name")]
    NoName,
    #[error("the last day comes before the first")]
    EndsBeforeStart,
    #[error("the first day has passed")]
    StartsInPast,
    #[error("a Promotion that started keeps its first day")]
    AlreadyStarted,
    #[error("this kind of Promotion runs at most {0} days")]
    TooLong(i64),
    #[error("a Promotion's price is above zero and below the listed price")]
    NotADiscount,
    #[error(
        "a coupon takes off a positive amount or 0 to 100%, from a positive purchase and budget"
    )]
    InvalidCoupon,
    #[error("the listing already has a price discount")]
    AlreadyDiscounted,
    #[error("the listing is already in that Promotion")]
    AlreadyIn,
    #[error("the margin would fall below the minimum")]
    BelowMinimum(Box<PromotionCheck>),
    #[error("the minimum margin goes from 0 up to, not including, 100%")]
    InvalidMinimum,
    #[error("this Promotion is already on its way to the channel")]
    Sending,
    #[error(transparent)]
    Pricing(#[from] PricingError),
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error(transparent)]
    Currencies(#[from] CurrencyMismatch),
    #[error("the Promotions hold a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for PromotionError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => PromotionError::Unreadable(text),
            StoredValueError::Sql(error) => PromotionError::Sql(error),
        }
    }
}

/// What a confirmed request does on the channel.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Discount {
        listing: String,
        price: Money,
        starts: NaiveDate,
        ends: NaiveDate,
    },
    Create(PromotionPlan),
    Join {
        listing: String,
        promotion: String,
        kind: PromotionKind,
        price: Option<Money>,
    },
    Change {
        promotion: RecordId,
        id: String,
        plan: PromotionPlan,
    },
}

/// A Promotion checked and ready for the owner to read, with the margin it
/// leaves. Only [`PromotionRequest::confirm`] makes it something
/// [`Promotions::send`] takes; dropping it sends nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "a Promotion does nothing until confirmed and passed to Promotions::send"]
pub struct PromotionRequest {
    action: Action,
    check: Option<PromotionCheck>,
}

impl PromotionRequest {
    /// The margin the listing keeps; `None` for a campaign or coupon with
    /// no listing yet, or a change of name and days.
    pub fn check(&self) -> Option<&PromotionCheck> {
        self.check.as_ref()
    }

    /// The owner read what goes to the channel and confirmed it.
    pub fn confirm(self) -> ConfirmedPromotion {
        ConfirmedPromotion {
            action: self.action,
        }
    }
}

/// A Promotion the owner confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedPromotion {
    action: Action,
}

/// The owner's Promotions, the minimum margin they keep, and the guard
/// that holds them to it.
pub struct Promotions {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    listings: Listings,
    pricing: Pricing,
    /// What is on its way to the channel, so a second click never sends
    /// it again.
    sending: Mutex<HashSet<String>>,
}

impl Promotions {
    pub fn new(
        database: Arc<Database>,
        inventory: Arc<Inventory>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            listings: Listings::new(database.clone(), clock.clone(), ids.clone()),
            pricing: Pricing::new(database.clone(), inventory, clock.clone(), ids.clone()),
            database,
            clock,
            ids,
            sending: Mutex::default(),
        }
    }

    /// The least margin a Promotion may leave a sale with: 10% until the
    /// owner sets another.
    pub async fn minimum_margin(&self) -> Result<Percentage, PromotionError> {
        let row =
            load_single_row(self.database.connection(), SETTINGS_TABLE, SETTINGS_COLUMNS).await?;
        match row {
            Some(row) => {
                let margin = row.decimal_at(0)?;
                Percentage::new(margin).map_err(|_| PromotionError::Unreadable(margin.to_string()))
            }
            None => Ok(Percentage::new(DEFAULT_MINIMUM_MARGIN).expect("10% is a percentage")),
        }
    }

    pub async fn save_minimum_margin(&self, margin: Percentage) -> Result<(), PromotionError> {
        check_target(margin).map_err(|_| PromotionError::InvalidMinimum)?;
        save_single_row(
            self.database.connection(),
            self.clock.as_ref(),
            self.ids.as_ref(),
            SETTINGS_TABLE,
            SETTINGS_COLUMNS,
            vec![margin.percent().to_string().into()],
        )
        .await?;
        Ok(())
    }

    /// When the Promotions were last read; `None` before the first Sync.
    pub async fn last_sync(&self) -> Result<Option<Timestamp>, PromotionError> {
        let row = load_single_row(self.database.connection(), SYNCS_TABLE, SYNCS_COLUMNS).await?;
        Ok(row.map(|row| row.time_at(0)).transpose()?)
    }

    /// Whether the last Sync is [`PROMOTIONS_REFRESH_MINUTES`] old, or none
    /// ran yet.
    pub async fn is_due(&self) -> Result<bool, PromotionError> {
        Ok(match self.last_sync().await? {
            Some(at) => self.clock.now() - at >= TimeDelta::minutes(PROMOTIONS_REFRESH_MINUTES),
            None => true,
        })
    }

    /// Reads the owner's campaigns and coupons and the Promotions of each
    /// open listing. Each is kept by the channel's id, so a second Sync with
    /// the same answers changes nothing; one the channel no longer reports
    /// has finished or was ended, and goes. Nothing is written if a read
    /// fails.
    pub async fn sync(
        &self,
        channel: &dyn ChannelPromotions,
    ) -> Result<PromotionSync, PromotionError> {
        let promotions = channel.promotions()?;
        let mut offers = BTreeMap::new();
        for listing in self.open_channel_ids().await? {
            let found = channel.offers(&listing)?;
            offers.insert(listing, found);
        }

        let now = self.clock.now();
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let mut report = PromotionSync {
            promotions: promotions.len(),
            offers: offers.values().map(Vec::len).sum(),
            ended: 0,
        };
        for promotion in &promotions {
            write_promotion(&transaction, self.ids.as_ref(), promotion, now).await?;
        }
        let reported: Vec<String> = promotions.iter().map(|p| p.id.clone()).collect();
        report.ended += end_unreported(
            &transaction,
            "commerce_promotions",
            "ml_promotion_id",
            &reported,
            now,
        )
        .await?;
        for (listing, found) in &offers {
            report.ended +=
                write_offers(&transaction, self.ids.as_ref(), listing, found, now).await?;
        }
        let read: Vec<String> = offers.into_keys().collect();
        report.ended += end_unreported(
            &transaction,
            "commerce_promotion_offers",
            "ml_item_id",
            &read,
            now,
        )
        .await?;
        transaction.commit().await?;
        save_single_row(
            self.database.connection(),
            self.clock.as_ref(),
            self.ids.as_ref(),
            SYNCS_TABLE,
            SYNCS_COLUMNS,
            vec![stored(now).into()],
        )
        .await?;
        Ok(report)
    }

    /// The owner's campaigns and coupons, the soonest to start first.
    pub async fn promotions(&self) -> Result<Vec<Promotion>, PromotionError> {
        read_promotions(self.database.connection(), "", ()).await
    }

    pub async fn promotion(&self, id: RecordId) -> Result<Promotion, PromotionError> {
        read_promotions(
            self.database.connection(),
            "AND id = ?1",
            params![id.to_string()],
        )
        .await?
        .pop()
        .ok_or(PromotionError::UnknownPromotion(id))
    }

    /// Every listing in a Promotion, by listing, the soonest to end first.
    pub async fn offers(&self) -> Result<Vec<Offer>, PromotionError> {
        read_offers(self.database.connection(), "", ()).await
    }

    /// The margin one unit of the channel listing `listing` keeps at the
    /// lowest price it sells for while its Promotions run: the lowest of
    /// its price and theirs, after the coupon that takes the most off.
    /// Nothing is sent to the channel.
    pub async fn margin_of(
        &self,
        listing: &str,
        channel: &dyn SalesChannel,
        assumptions: PriceAssumptions,
    ) -> Result<PromotionCheck, PromotionError> {
        let asked = self.open_variation(listing).await?;
        self.worst_case(asked, None, None, channel, assumptions)
            .await
    }

    /// Checks a price discount for a listing: the days, a price below the
    /// listed one and the margin it leaves.
    pub async fn prepare_discount(
        &self,
        plan: DiscountPlan,
        reputation_unlocks: bool,
        channel: &dyn SalesChannel,
        assumptions: PriceAssumptions,
    ) -> Result<PromotionRequest, PromotionError> {
        if !reputation_unlocks {
            return Err(PromotionError::Locked);
        }
        let listing = self.listings.listing(plan.listing).await?;
        if listing.listed.status == ListingStatus::Closed {
            return Err(PromotionError::Closed);
        }
        check_days(
            PromotionKind::PriceDiscount,
            (plan.starts, plan.ends),
            self.today(),
            None,
        )?;
        let price = discounted(plan.price, listing.listed.price)?;
        let item = listing.listed.id.clone();
        if self
            .offers_of(&item)
            .await?
            .iter()
            .any(|offer| offer.kind == PromotionKind::PriceDiscount)
        {
            return Err(PromotionError::AlreadyDiscounted);
        }
        let check = self
            .worst_case(listing.id, Some(price), None, channel, assumptions)
            .await
            .and_then(holding)?;
        Ok(PromotionRequest {
            action: Action::Discount {
                listing: item,
                price,
                starts: plan.starts,
                ends: plan.ends,
            },
            check: Some(check),
        })
    }

    /// Checks a campaign or coupon to create: its name, its days and a
    /// coupon's terms. Its listings join it afterwards, each guarded.
    pub fn prepare_promotion(
        &self,
        plan: PromotionPlan,
        reputation_unlocks: bool,
    ) -> Result<PromotionRequest, PromotionError> {
        if !reputation_unlocks {
            return Err(PromotionError::Locked);
        }
        let plan = checked_plan(plan, self.today(), None)?;
        Ok(PromotionRequest {
            action: Action::Create(plan),
            check: None,
        })
    }

    /// Checks putting a listing in a campaign, at `price`, or in a coupon,
    /// and the margin it leaves.
    pub async fn prepare_join(
        &self,
        promotion: RecordId,
        listing: RecordId,
        price: Option<Money>,
        reputation_unlocks: bool,
        channel: &dyn SalesChannel,
        assumptions: PriceAssumptions,
    ) -> Result<PromotionRequest, PromotionError> {
        if !reputation_unlocks {
            return Err(PromotionError::Locked);
        }
        let promotion = self.promotion(promotion).await?.listed;
        let listing = self.listings.listing(listing).await?;
        if listing.listed.status == ListingStatus::Closed {
            return Err(PromotionError::Closed);
        }
        let item = listing.listed.id.clone();
        if self
            .offers_of(&item)
            .await?
            .iter()
            .any(|offer| offer.promotion.as_deref() == Some(promotion.id.as_str()))
        {
            return Err(PromotionError::AlreadyIn);
        }
        let (price, coupon) = match promotion.kind {
            PromotionKind::SellerCampaign => (
                Some(discounted(
                    price.ok_or(PromotionError::NotADiscount)?,
                    listing.listed.price,
                )?),
                None,
            ),
            PromotionKind::Coupon => (None, promotion.coupon),
            _ => return Err(PromotionError::NotOwn),
        };
        let check = self
            .worst_case(listing.id, price, coupon, channel, assumptions)
            .await
            .and_then(holding)?;
        Ok(PromotionRequest {
            action: Action::Join {
                listing: item,
                promotion: promotion.id,
                kind: promotion.kind,
                price,
            },
            check: Some(check),
        })
    }

    /// Checks a new name and days for a campaign or coupon. One that
    /// started keeps its first day.
    pub async fn prepare_change(
        &self,
        promotion: RecordId,
        name: &str,
        starts: NaiveDate,
        ends: NaiveDate,
    ) -> Result<PromotionRequest, PromotionError> {
        let kept = self.promotion(promotion).await?;
        let started =
            (kept.listed.status == PromotionStatus::Started).then_some(kept.listed.starts);
        let name = name.trim();
        if name.is_empty() {
            return Err(PromotionError::NoName);
        }
        check_days(kept.listed.kind, (starts, ends), self.today(), started)?;
        let plan = PromotionPlan {
            kind: kept.listed.kind,
            name: name.to_owned(),
            starts,
            ends,
            coupon: kept.listed.coupon,
        };
        Ok(PromotionRequest {
            action: Action::Change {
                promotion,
                id: kept.listed.id,
                plan,
            },
            check: None,
        })
    }

    /// Sends what the owner confirmed, once. Before creating, the channel is
    /// asked whether it already has it, from an earlier send whose answer
    /// never arrived: then it is kept as the channel has it and nothing is
    /// created twice. What the channel reports afterwards is kept.
    pub async fn send(
        &self,
        channel: &dyn ChannelPromotions,
        promotion: ConfirmedPromotion,
    ) -> Result<(), PromotionError> {
        match promotion.action {
            Action::Discount {
                listing,
                price,
                starts,
                ends,
            } => {
                let _sending = Sending::start(&self.sending, format!("{listing}:discount"))?;
                let offer = OfferToJoin {
                    kind: PromotionKind::PriceDiscount,
                    promotion: None,
                    price: Some(price),
                    days: Some((starts, ends)),
                };
                self.join_once(channel, &listing, &offer, |offer| {
                    offer.kind == PromotionKind::PriceDiscount
                })
                .await
            }
            Action::Join {
                listing,
                promotion,
                kind,
                price,
            } => {
                let _sending = Sending::start(&self.sending, format!("{listing}:{promotion}"))?;
                let offer = OfferToJoin {
                    kind,
                    promotion: Some(promotion.clone()),
                    price,
                    days: None,
                };
                self.join_once(channel, &listing, &offer, |offer| {
                    offer.promotion.as_deref() == Some(promotion.as_str())
                })
                .await
            }
            Action::Create(plan) => {
                let _sending = Sending::start(
                    &self.sending,
                    format!("create:{}:{}", plan.kind.code(), plan.name),
                )?;
                let existing = channel.promotions()?.into_iter().find(|promotion| {
                    promotion.kind == plan.kind
                        && promotion.name == plan.name
                        && promotion.starts == plan.starts
                        && promotion.ends == plan.ends
                });
                let mut created = match existing {
                    Some(promotion) => promotion,
                    None => channel.create(&plan)?,
                };
                created.coupon = created.coupon.or(plan.coupon);
                write_promotion(
                    self.database.connection(),
                    self.ids.as_ref(),
                    &created,
                    self.clock.now(),
                )
                .await?;
                Ok(())
            }
            Action::Change {
                promotion,
                id,
                plan,
            } => {
                let _sending = Sending::start(&self.sending, format!("change:{id}"))?;
                channel.change(&id, &plan)?;
                self.database
                    .connection()
                    .execute(
                        "UPDATE commerce_promotions
                         SET name = ?1, starts_on = ?2, ends_on = ?3, updated_at = ?4
                         WHERE id = ?5",
                        params![
                            plan.name,
                            day(plan.starts),
                            day(plan.ends),
                            stored(self.clock.now()),
                            promotion.to_string()
                        ],
                    )
                    .await?;
                Ok(())
            }
        }
    }

    /// Takes a listing out of one of the owner's Promotions, or ends its
    /// price discount, on the owner's click. One the channel already ended
    /// goes as well.
    pub async fn end_offer(
        &self,
        channel: &dyn ChannelPromotions,
        offer: RecordId,
    ) -> Result<(), PromotionError> {
        let kept = read_offers(
            self.database.connection(),
            "AND id = ?1",
            params![offer.to_string()],
        )
        .await?
        .pop()
        .ok_or(PromotionError::UnknownOffer(offer))?
        .listed;
        if !kept.kind.is_own() {
            return Err(PromotionError::NotOwn);
        }
        let _sending = Sending::start(&self.sending, format!("end:{offer}"))?;
        match channel.leave(&kept.listing, kept.kind, kept.promotion.as_deref()) {
            Ok(()) | Err(PlatformError::NotFound) => {}
            Err(error) => return Err(error.into()),
        }
        self.refresh_offers(channel, &kept.listing).await
    }

    /// Ends a campaign or coupon of the owner's, every listing in it
    /// included, on the owner's click. One the channel already ended goes
    /// as well.
    pub async fn end_promotion(
        &self,
        channel: &dyn ChannelPromotions,
        promotion: RecordId,
    ) -> Result<(), PromotionError> {
        let kept = self.promotion(promotion).await?.listed;
        let _sending = Sending::start(&self.sending, format!("end:{promotion}"))?;
        match channel.end(&kept.id, kept.kind) {
            Ok(()) | Err(PlatformError::NotFound) => {}
            Err(error) => return Err(error.into()),
        }
        let now = stored(self.clock.now());
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        for table in ["commerce_promotions", "commerce_promotion_offers"] {
            transaction
                .execute(
                    &format!(
                        "UPDATE {table} SET deleted_at = ?1, updated_at = ?1
                         WHERE ml_promotion_id = ?2 AND deleted_at IS NULL"
                    ),
                    params![now.clone(), kept.id.clone()],
                )
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Puts `listing` in a Promotion unless the channel already has it there
    /// (`already`), and keeps the listing's Promotions as the channel then
    /// reports them.
    async fn join_once(
        &self,
        channel: &dyn ChannelPromotions,
        listing: &str,
        offer: &OfferToJoin,
        already: impl Fn(&ChannelOffer) -> bool,
    ) -> Result<(), PromotionError> {
        if !channel.offers(listing)?.iter().any(already) {
            channel.join(listing, offer)?;
        }
        self.refresh_offers(channel, listing).await
    }

    async fn refresh_offers(
        &self,
        channel: &dyn ChannelPromotions,
        listing: &str,
    ) -> Result<(), PromotionError> {
        let found = channel.offers(listing)?;
        write_offers(
            self.database.connection(),
            self.ids.as_ref(),
            listing,
            &found,
            self.clock.now(),
        )
        .await?;
        Ok(())
    }

    /// One unit of `listing` at the lowest price it sells for while its
    /// Promotions run, with `price` and `coupon` (a new Promotion) among
    /// them, against the minimum margin.
    async fn worst_case(
        &self,
        listing: RecordId,
        price: Option<Money>,
        coupon: Option<CouponTerms>,
        channel: &dyn SalesChannel,
        assumptions: PriceAssumptions,
    ) -> Result<PromotionCheck, PromotionError> {
        let asked = self.listings.listing(listing).await?;
        let offers = self.offers_of(&asked.listed.id).await?;
        let promotions = self.promotions().await?;
        let mut coupons: Vec<CouponTerms> = offers
            .iter()
            .filter(|offer| offer.kind == PromotionKind::Coupon)
            .filter_map(|offer| {
                promotions
                    .iter()
                    .find(|kept| Some(&kept.listed.id) == offer.promotion.as_ref())
                    .and_then(|kept| kept.listed.coupon)
            })
            .collect();
        coupons.extend(coupon);
        let listed = offers
            .iter()
            .filter_map(|offer| offer.price)
            .chain(price)
            .fold(asked.listed.price, |lowest, price| {
                if price.amount() < lowest.amount() {
                    price
                } else {
                    lowest
                }
            });
        let mut lowest = listed;
        for terms in &coupons {
            let after = terms.unit_price(listed)?;
            if after.amount() < lowest.amount() {
                lowest = after;
            }
        }
        let minimum = self.minimum_margin().await?;
        let margin = self
            .pricing
            .check_margin(listing, lowest, minimum, channel, assumptions)
            .await?;
        Ok(PromotionCheck {
            margin,
            after_coupon: lowest.amount() < listed.amount(),
        })
    }

    /// An open Listing of the channel listing `listing`.
    async fn open_variation(&self, listing: &str) -> Result<RecordId, PromotionError> {
        self.listings
            .sharing_price(listing)
            .await?
            .into_iter()
            .find(|kept| kept.listed.status != ListingStatus::Closed)
            .map(|kept| kept.id)
            .ok_or(PromotionError::Closed)
    }

    async fn offers_of(&self, listing: &str) -> Result<Vec<ChannelOffer>, PromotionError> {
        Ok(read_offers(
            self.database.connection(),
            "AND ml_item_id = ?1",
            params![listing.to_owned()],
        )
        .await?
        .into_iter()
        .map(|offer| offer.listed)
        .collect())
    }

    /// The channel's ids of the listings still open, each once.
    async fn open_channel_ids(&self) -> Result<Vec<String>, PromotionError> {
        let mut ids: Vec<String> = Vec::new();
        for listing in self.listings.listings().await? {
            if listing.listed.status != ListingStatus::Closed && !ids.contains(&listing.listed.id) {
                ids.push(listing.listed.id);
            }
        }
        Ok(ids)
    }

    fn today(&self) -> NaiveDate {
        channel_day(self.clock.now())
    }
}

/// The check of a Promotion that keeps the minimum margin; below it, the
/// Promotion is refused with the check.
fn holding(check: PromotionCheck) -> Result<PromotionCheck, PromotionError> {
    if check.margin.passes() {
        Ok(check)
    } else {
        Err(PromotionError::BelowMinimum(Box::new(check)))
    }
}

/// A Promotion's price, rounded to cents: above zero and below `listed`.
fn discounted(price: Money, listed: Money) -> Result<Money, PromotionError> {
    let price = price.rounded();
    if price.currency() == listed.currency()
        && price.amount() > Decimal::ZERO
        && price.amount() < listed.amount()
    {
        Ok(price)
    } else {
        Err(PromotionError::NotADiscount)
    }
}

/// A campaign's or coupon's plan, with its name trimmed, once its name,
/// days and terms hold. `started` is the first day of one already running,
/// which it keeps.
fn checked_plan(
    plan: PromotionPlan,
    today: NaiveDate,
    started: Option<NaiveDate>,
) -> Result<PromotionPlan, PromotionError> {
    let name = plan.name.trim().to_owned();
    if name.is_empty() {
        return Err(PromotionError::NoName);
    }
    check_days(plan.kind, (plan.starts, plan.ends), today, started)?;
    match (plan.kind, plan.coupon) {
        (PromotionKind::SellerCampaign, None) => {}
        (PromotionKind::Coupon, Some(terms)) => terms.check(terms.minimum_purchase.currency())?,
        (PromotionKind::Coupon, None) => return Err(PromotionError::InvalidCoupon),
        _ => return Err(PromotionError::NotOwn),
    }
    Ok(PromotionPlan { name, ..plan })
}

/// The channel's rules for a Promotion's days: the last on or after the
/// first, the first not past (unless it already started on it), and no
/// more days than the kind allows.
fn check_days(
    kind: PromotionKind,
    (starts, ends): (NaiveDate, NaiveDate),
    today: NaiveDate,
    started: Option<NaiveDate>,
) -> Result<(), PromotionError> {
    let max_days = kind.max_days().ok_or(PromotionError::NotOwn)?;
    if let Some(started) = started
        && starts != started
    {
        return Err(PromotionError::AlreadyStarted);
    }
    if ends < starts {
        return Err(PromotionError::EndsBeforeStart);
    }
    if (started.is_none() && starts < today) || ends < today {
        return Err(PromotionError::StartsInPast);
    }
    if (ends - starts).num_days() + 1 > max_days {
        return Err(PromotionError::TooLong(max_days));
    }
    Ok(())
}

struct Sending<'a> {
    set: &'a Mutex<HashSet<String>>,
    key: String,
}

impl<'a> Sending<'a> {
    fn start(set: &'a Mutex<HashSet<String>>, key: String) -> Result<Self, PromotionError> {
        let mut sending = set.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !sending.insert(key.clone()) {
            return Err(PromotionError::Sending);
        }
        Ok(Self { set, key })
    }
}

impl Drop for Sending<'_> {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.key);
    }
}

fn day(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

fn read_day(text: &str) -> Result<NaiveDate, PromotionError> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .map_err(|_| PromotionError::Unreadable(text.to_owned()))
}

fn amount(money: Option<Money>) -> Option<String> {
    money.map(|money| money.amount().to_string())
}

/// Keeps a campaign or coupon by the channel's id; one ended before comes
/// back.
async fn write_promotion(
    on: &Connection,
    ids: &dyn IdGenerator,
    promotion: &ChannelPromotion,
    now: Timestamp,
) -> Result<(), PromotionError> {
    let coupon = promotion.coupon;
    let (coupon_kind, coupon_value) = match coupon.map(|terms| terms.discount) {
        Some(CouponDiscount::Amount(money)) => (Some("amount"), Some(money.amount().to_string())),
        Some(CouponDiscount::Percent(rate)) => (Some("percent"), Some(rate.percent().to_string())),
        None => (None, None),
    };
    on.execute(
        "INSERT INTO commerce_promotions
             (id, ml_promotion_id, kind, name, starts_on, ends_on, status, coupon_kind,
              coupon_value, coupon_minimum, coupon_maximum, coupon_budget, currency, synced_at,
              created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14, ?14)
         ON CONFLICT (ml_promotion_id) DO UPDATE SET
             kind = excluded.kind, name = excluded.name, starts_on = excluded.starts_on,
             ends_on = excluded.ends_on, status = excluded.status,
             coupon_kind = COALESCE(excluded.coupon_kind, coupon_kind),
             coupon_value = COALESCE(excluded.coupon_value, coupon_value),
             coupon_minimum = COALESCE(excluded.coupon_minimum, coupon_minimum),
             coupon_maximum = CASE WHEN excluded.coupon_kind IS NULL THEN coupon_maximum
                                   ELSE excluded.coupon_maximum END,
             coupon_budget = COALESCE(excluded.coupon_budget, coupon_budget),
             currency = COALESCE(excluded.currency, currency),
             synced_at = excluded.synced_at, updated_at = excluded.updated_at,
             deleted_at = NULL",
        params![
            ids.next_id().to_string(),
            promotion.id.clone(),
            promotion.kind.code(),
            promotion.name.clone(),
            day(promotion.starts),
            day(promotion.ends),
            promotion.status.code(),
            coupon_kind,
            coupon_value,
            amount(coupon.map(|terms| terms.minimum_purchase)),
            amount(coupon.and_then(|terms| terms.maximum_discount)),
            amount(coupon.map(|terms| terms.budget)),
            coupon.map(|terms| terms.minimum_purchase.currency().code()),
            stored(now)
        ],
    )
    .await?;
    Ok(())
}

/// Keeps the Promotions the channel reports for `listing`, each by the
/// listing, kind and Promotion, and ends the listing's others. Returns how
/// many ended.
async fn write_offers(
    on: &Connection,
    ids: &dyn IdGenerator,
    listing: &str,
    offers: &[ChannelOffer],
    now: Timestamp,
) -> Result<usize, PromotionError> {
    let mut kept = Vec::new();
    for offer in offers.iter().filter(|offer| offer.listing == listing) {
        let promotion = offer.promotion.clone().unwrap_or_default();
        on.execute(
            "INSERT INTO commerce_promotion_offers
                 (id, ml_item_id, kind, ml_promotion_id, name, status, price, currency,
                  starts_on, ends_on, synced_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?11)
             ON CONFLICT (ml_item_id, kind, ml_promotion_id) DO UPDATE SET
                 name = excluded.name, status = excluded.status, price = excluded.price,
                 currency = excluded.currency, starts_on = excluded.starts_on,
                 ends_on = excluded.ends_on, synced_at = excluded.synced_at,
                 updated_at = excluded.updated_at, deleted_at = NULL",
            params![
                ids.next_id().to_string(),
                listing.to_owned(),
                offer.kind.code(),
                promotion.clone(),
                offer.name.clone(),
                offer.status.code(),
                amount(offer.price),
                offer.price.map(|price| price.currency().code()),
                offer.starts.map(day),
                offer.ends.map(day),
                stored(now)
            ],
        )
        .await?;
        kept.push(format!("{}:{promotion}", offer.kind.code()));
    }
    let mut rows = on
        .query(
            "SELECT id, kind || ':' || ml_promotion_id FROM commerce_promotion_offers
             WHERE ml_item_id = ?1 AND deleted_at IS NULL",
            params![listing.to_owned()],
        )
        .await?;
    let mut ended = Vec::new();
    while let Some(row) = rows.next().await? {
        let key: String = row.get(1)?;
        if !kept.contains(&key) {
            ended.push(row.get::<String>(0)?);
        }
    }
    for id in &ended {
        on.execute(
            "UPDATE commerce_promotion_offers SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
            params![stored(now), id.clone()],
        )
        .await?;
    }
    Ok(ended.len())
}

/// Ends the rows of `table` whose `key` is not among `reported`. Returns
/// how many ended.
async fn end_unreported(
    on: &Connection,
    table: &str,
    key: &str,
    reported: &[String],
    now: Timestamp,
) -> Result<usize, PromotionError> {
    let mut rows = on
        .query(
            &format!("SELECT id, {key} FROM {table} WHERE deleted_at IS NULL"),
            (),
        )
        .await?;
    let mut ended = Vec::new();
    while let Some(row) = rows.next().await? {
        let value: String = row.get(1)?;
        if !reported.contains(&value) {
            ended.push(row.get::<String>(0)?);
        }
    }
    for id in &ended {
        on.execute(
            &format!("UPDATE {table} SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2"),
            params![stored(now), id.clone()],
        )
        .await?;
    }
    Ok(ended.len())
}

async fn read_promotions(
    on: &Connection,
    only: &str,
    params: impl libsql::params::IntoParams,
) -> Result<Vec<Promotion>, PromotionError> {
    let mut rows = on
        .query(
            &format!(
                "SELECT id, ml_promotion_id, kind, name, starts_on, ends_on, status, coupon_kind,
                        coupon_value, coupon_minimum, coupon_maximum, coupon_budget, currency,
                        synced_at
                 FROM commerce_promotions WHERE deleted_at IS NULL {only}
                 ORDER BY starts_on, name"
            ),
            params,
        )
        .await?;
    let mut promotions = Vec::new();
    while let Some(row) = rows.next().await? {
        promotions.push(promotion_from(&row)?);
    }
    Ok(promotions)
}

fn promotion_from(row: &Row) -> Result<Promotion, PromotionError> {
    let kind: String = row.get(2)?;
    let status: String = row.get(6)?;
    let currency: Option<String> = row.get(12)?;
    let coupon_kind: Option<String> = row.get(7)?;
    let coupon = match (coupon_kind.as_deref(), currency) {
        (Some(coupon_kind), Some(currency)) => {
            let currency = Currency::from_code(&currency)
                .ok_or_else(|| PromotionError::Unreadable(currency.clone()))?;
            let money = |at: i32| -> Result<Money, PromotionError> {
                Ok(Money::new(row.decimal_at(at)?, currency))
            };
            let value = row.decimal_at(8)?;
            let discount = match coupon_kind {
                "amount" => CouponDiscount::Amount(Money::new(value, currency)),
                "percent" => CouponDiscount::Percent(
                    Percentage::new(value)
                        .map_err(|_| PromotionError::Unreadable(value.to_string()))?,
                ),
                other => return Err(PromotionError::Unreadable(other.to_owned())),
            };
            let maximum: Option<String> = row.get(10)?;
            Some(CouponTerms {
                discount,
                minimum_purchase: money(9)?,
                maximum_discount: maximum.map(|_| money(10)).transpose()?,
                budget: money(11)?,
            })
        }
        _ => None,
    };
    let starts: String = row.get(4)?;
    let ends: String = row.get(5)?;
    Ok(Promotion {
        id: row.id_at(0)?,
        listed: ChannelPromotion {
            id: row.get(1)?,
            kind: PromotionKind::from_code(&kind).ok_or(PromotionError::Unreadable(kind))?,
            name: row.get(3)?,
            starts: read_day(&starts)?,
            ends: read_day(&ends)?,
            status: PromotionStatus::from_code(&status)
                .ok_or(PromotionError::Unreadable(status))?,
            coupon,
        },
        synced_at: row.time_at(13)?,
    })
}

async fn read_offers(
    on: &Connection,
    only: &str,
    params: impl libsql::params::IntoParams,
) -> Result<Vec<Offer>, PromotionError> {
    let mut rows = on
        .query(
            &format!(
                "SELECT id, ml_item_id, kind, ml_promotion_id, name, status, price, currency,
                        starts_on, ends_on, synced_at
                 FROM commerce_promotion_offers WHERE deleted_at IS NULL {only}
                 ORDER BY ml_item_id, ends_on IS NULL, ends_on, kind"
            ),
            params,
        )
        .await?;
    let mut offers = Vec::new();
    while let Some(row) = rows.next().await? {
        let kind: String = row.get(2)?;
        let status: String = row.get(5)?;
        let promotion: String = row.get(3)?;
        let currency: Option<String> = row.get(7)?;
        let price = match currency {
            Some(currency) => Some(Money::new(
                row.decimal_at(6)?,
                Currency::from_code(&currency).ok_or(PromotionError::Unreadable(currency))?,
            )),
            None => None,
        };
        let starts: Option<String> = row.get(8)?;
        let ends: Option<String> = row.get(9)?;
        offers.push(Offer {
            id: row.id_at(0)?,
            listed: ChannelOffer {
                listing: row.get(1)?,
                kind: PromotionKind::from_code(&kind).ok_or(PromotionError::Unreadable(kind))?,
                promotion: (!promotion.is_empty()).then_some(promotion),
                name: row.get(4)?,
                status: PromotionStatus::from_code(&status)
                    .ok_or(PromotionError::Unreadable(status))?,
                price,
                starts: starts.as_deref().map(read_day).transpose()?,
                ends: ends.as_deref().map(read_day).transpose()?,
            },
            synced_at: row.time_at(10)?,
        });
    }
    Ok(offers)
}
