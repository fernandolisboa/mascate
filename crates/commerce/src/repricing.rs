//! Automatic Repricing (#135, ADR 0033 decision 7): with its flag on, the
//! owner turns repricing on for a Listing within a minimum and a maximum
//! they confirm, the minimum never below the price that keeps the Minimum
//! Margin (ADR 0024). Mercado Livre reprices with its own price automation,
//! and the app sends that Listing no price while it runs; on Amazon the app
//! follows the featured offer's expected price after each Sync, one price
//! per Listing at most. Shopee and Magalu tell no competitor's price, so
//! they have none. Each price the app knows a Listing changed to goes into
//! the Listing's price history, with why.

use std::collections::HashSet;
use std::sync::Arc;

use libsql::{Connection, Row, params};
use mascate_inventory::Inventory;
use mascate_kernel::{
    Clock, IdGenerator, Money, Percentage, PlatformError, RecordId, SalesChannelKind, Timestamp,
};
use mascate_platform::{
    Database, Flag, FlagError, FlagKind, Flags, Migration, Phase, Registry, StoredRow,
    StoredValueError, stored,
};
use rust_decimal::Decimal;

use crate::promotions::minimum_margin;
use crate::{
    ListingError, ListingStatus, Listings, MarginCheck, OnSalesChannel, PriceAssumptions, Pricing,
    PricingError, SalesChannel,
};

/// Changing a Listing's price without the owner confirming each change
/// (ADR 0015).
pub const AUTOMATIC_REPRICING: Flag = Flag {
    key: "commerce.automatic_repricing",
    name: "Reprecificação automática",
    kind: FlagKind::ProductChoice,
    phase: Phase::Three,
    reason: "Você decidiu que todo preço só vai ao canal depois da sua aprovação, e a \
             reprecificação automática muda o preço sem confirmar cada mudança.",
    risk: "O preço dos anúncios em que você ligar a reprecificação sobe e desce sozinho entre o \
           mínimo e o máximo que você confirmar, seguindo a concorrência, sem passar por você a \
           cada mudança.",
};

pub(crate) const CREATE_PRICE_HISTORY: Migration = Migration {
    version: 15,
    name: "create the price history and automatic repricing",
    risky: false,
    sql: "CREATE TABLE commerce_price_changes (
        id         TEXT PRIMARY KEY,
        listing_id TEXT NOT NULL,
        old_price  TEXT NOT NULL,
        new_price  TEXT NOT NULL,
        currency   TEXT NOT NULL,
        reason     TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE INDEX commerce_price_changes_by_listing
        ON commerce_price_changes (listing_id, created_at);
    CREATE TABLE commerce_repricing (
        id              TEXT PRIMARY KEY,
        sales_channel   TEXT NOT NULL,
        channel_item_id TEXT NOT NULL,
        way             TEXT NOT NULL,
        minimum         TEXT NOT NULL,
        maximum         TEXT NOT NULL,
        currency        TEXT NOT NULL,
        status          TEXT,
        status_detail   TEXT,
        winning_price   TEXT,
        checked_at      TEXT,
        created_at      TEXT NOT NULL,
        updated_at      TEXT NOT NULL,
        deleted_at      TEXT
    );
    CREATE UNIQUE INDEX commerce_repricing_by_channel_item
        ON commerce_repricing (sales_channel, channel_item_id) WHERE deleted_at IS NULL;",
};

/// Why a Listing's price changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceChangeReason {
    /// The owner approved it in the app.
    Approved,
    /// The app followed the featured offer's expected price (Amazon).
    FeaturedOffer,
    /// The channel's own price automation changed it (Mercado Livre).
    ChannelAutomation,
    /// A Sync found it changed in the channel: by the owner in the
    /// channel's panel, or by the channel.
    InChannel,
}

impl PriceChangeReason {
    const ALL: [PriceChangeReason; 4] = [
        PriceChangeReason::Approved,
        PriceChangeReason::FeaturedOffer,
        PriceChangeReason::ChannelAutomation,
        PriceChangeReason::InChannel,
    ];

    fn code(self) -> &'static str {
        match self {
            PriceChangeReason::Approved => "approved",
            PriceChangeReason::FeaturedOffer => "featured_offer",
            PriceChangeReason::ChannelAutomation => "channel_automation",
            PriceChangeReason::InChannel => "in_channel",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.code() == code)
    }
}

/// One change in a Listing's price history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceChange {
    pub listing: RecordId,
    pub old: Money,
    pub new: Money,
    pub reason: PriceChangeReason,
    pub at: Timestamp,
}

/// The prices Automatic Repricing keeps a listing between, both included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepricingLimits {
    pub minimum: Money,
    pub maximum: Money,
}

impl RepricingLimits {
    /// `price` brought within the limits.
    pub fn clamp(&self, price: Money) -> Money {
        if price.amount() < self.minimum.amount() {
            self.minimum
        } else if price.amount() > self.maximum.amount() {
            self.maximum
        } else {
            price
        }
    }

    pub fn contains(&self, price: Money) -> bool {
        price.currency() == self.minimum.currency()
            && price.amount() >= self.minimum.amount()
            && price.amount() <= self.maximum.amount()
    }
}

/// Where Automatic Repricing of a listing stands, as the channel last said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepricingStatus {
    /// Mercado Livre's automation runs; on Amazon, the app follows the
    /// featured offer.
    Following,
    /// Mercado Livre paused its automation while the listing is in a
    /// Promotion.
    PausedByPromotion,
    /// Mercado Livre paused its automation for another reason, in its own
    /// words.
    Paused(String),
    /// Amazon: no other offer competes for the featured place.
    NoCompetition,
    /// Amazon: the offer cannot be the featured one.
    NotEligible,
    /// Amazon: no price within the limits keeps the Minimum Margin now, as
    /// after the Average Cost went up; the price stays.
    MarginOutOfReach,
}

impl RepricingStatus {
    fn code(&self) -> &'static str {
        match self {
            RepricingStatus::Following => "following",
            RepricingStatus::PausedByPromotion => "paused_by_promotion",
            RepricingStatus::Paused(_) => "paused",
            RepricingStatus::NoCompetition => "no_competition",
            RepricingStatus::NotEligible => "not_eligible",
            RepricingStatus::MarginOutOfReach => "margin_out_of_reach",
        }
    }

    fn detail(&self) -> Option<String> {
        match self {
            RepricingStatus::Paused(why) => Some(why.clone()),
            _ => None,
        }
    }

    fn from_code(code: &str, detail: Option<String>) -> Option<Self> {
        Some(match code {
            "following" => RepricingStatus::Following,
            "paused_by_promotion" => RepricingStatus::PausedByPromotion,
            "paused" => RepricingStatus::Paused(detail.unwrap_or_default()),
            "no_competition" => RepricingStatus::NoCompetition,
            "not_eligible" => RepricingStatus::NotEligible,
            "margin_out_of_reach" => RepricingStatus::MarginOutOfReach,
            _ => return None,
        })
    }
}

/// What the channel said of a repriced listing at the last check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepricingReading {
    pub status: RepricingStatus,
    /// The price that wins, as information: Mercado Livre's price to win
    /// the catalog, or Amazon's featured offer's expected price.
    pub winning_price: Option<Money>,
    pub at: Timestamp,
}

/// How a channel reprices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepricingWay {
    /// The channel's own price automation, within the limits (Mercado
    /// Livre).
    ChannelAutomation,
    /// The app follows the featured offer's expected price (Amazon).
    FeaturedOffer,
}

impl RepricingWay {
    fn code(self) -> &'static str {
        match self {
            RepricingWay::ChannelAutomation => "channel_automation",
            RepricingWay::FeaturedOffer => "featured_offer",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        [RepricingWay::ChannelAutomation, RepricingWay::FeaturedOffer]
            .into_iter()
            .find(|way| way.code() == code)
    }
}

/// A channel listing under Automatic Repricing; its variations share the
/// price (ADR 0014).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepricedListing {
    pub channel: SalesChannelKind,
    /// The channel's id of the listing.
    pub listing: String,
    pub way: RepricingWay,
    pub limits: RepricingLimits,
    pub since: Timestamp,
    /// `None` before the first check.
    pub reading: Option<RepricingReading>,
}

/// A channel's own price automation on a listing, as it reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelAutomation {
    pub limits: RepricingLimits,
    pub status: RepricingStatus,
}

/// A Sales Channel that reprices a listing itself within limits, as
/// Mercado Livre's price automation. Calls block on the network.
pub trait ChannelPriceAutomation: OnSalesChannel + Send + Sync {
    /// Turns the channel's automation on for the listing `id` within
    /// `limits`, or changes the limits of the one it has.
    fn automate(
        &self,
        id: &str,
        limits: &RepricingLimits,
    ) -> Result<ChannelAutomation, PlatformError>;

    /// The listing's automation; `None` when it has none.
    fn automation(&self, id: &str) -> Result<Option<ChannelAutomation>, PlatformError>;

    /// Ends the listing's automation; one without it counts as ended.
    fn stop(&self, id: &str) -> Result<(), PlatformError>;

    /// The price the listing needs to win its catalog page, when the
    /// channel tells one.
    fn price_to_win(&self, id: &str) -> Result<Option<Money>, PlatformError>;
}

/// What a channel expects of a listing's featured offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeaturedOffer {
    /// At or below this price the offer can expect to be the featured one.
    Expected(Money),
    NoCompetition,
    NotEligible,
}

/// A Sales Channel that tells the price at which each listing's offer can
/// expect to be the featured one, as Amazon. Calls block on the network.
pub trait FeaturedOfferPrices: OnSalesChannel + Send + Sync {
    /// The featured offer of each listing in `ids`, by id; a listing left
    /// out of the answer is not eligible.
    fn featured_offers(
        &self,
        ids: &[String],
    ) -> Result<Vec<(String, FeaturedOffer)>, PlatformError>;
}

/// The port a channel reprices through.
#[derive(Clone, Copy)]
pub enum RepricingPort<'a> {
    Automation(&'a dyn ChannelPriceAutomation),
    FeaturedOffer(&'a dyn FeaturedOfferPrices),
}

impl RepricingPort<'_> {
    pub fn sales_channel(&self) -> SalesChannelKind {
        match self {
            RepricingPort::Automation(port) => port.sales_channel(),
            RepricingPort::FeaturedOffer(port) => port.sales_channel(),
        }
    }

    fn way(&self) -> RepricingWay {
        match self {
            RepricingPort::Automation(_) => RepricingWay::ChannelAutomation,
            RepricingPort::FeaturedOffer(_) => RepricingWay::FeaturedOffer,
        }
    }
}

/// Limits checked for a listing, ready for the owner to read: the price
/// that keeps the Minimum Margin and the sale at the minimum. Only
/// [`RepricingRequest::confirm`] makes it something [`Repricing::turn_on`]
/// takes; dropping it changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "repricing does nothing until confirmed and passed to Repricing::turn_on"]
pub struct RepricingRequest {
    channel: SalesChannelKind,
    item: String,
    limits: RepricingLimits,
    check: MarginCheck,
}

impl RepricingRequest {
    pub fn limits(&self) -> RepricingLimits {
        self.limits
    }

    /// The sale of one unit at the minimum, against the Minimum Margin,
    /// with the lowest price that keeps it.
    pub fn check(&self) -> &MarginCheck {
        &self.check
    }

    /// The owner read the limits and confirmed them.
    pub fn confirm(self) -> ConfirmedRepricing {
        ConfirmedRepricing { request: self }
    }
}

/// Limits the owner confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedRepricing {
    request: RepricingRequest,
}

/// What a round of Automatic Repricing on one channel did.
#[derive(Debug, Default)]
pub struct RepricingRound {
    /// Listings checked.
    pub checked: usize,
    /// Prices sent, one per listing at most.
    pub sent: usize,
    /// Listings whose automation the channel no longer has, or that the
    /// flag turned off: no longer repriced.
    pub ended: usize,
    /// Each listing the channel failed on, with why; the others went on.
    pub failed: Vec<(String, PlatformError)>,
}

#[derive(Debug, thiserror::Error)]
pub enum RepricingError {
    #[error("Automatic Repricing is turned off")]
    Off,
    #[error("the minimum is above zero and up to the maximum, in the Listing's currency")]
    InvalidLimits,
    #[error("the minimum keeps less than the Minimum Margin")]
    BelowMinimumMargin(Box<MarginCheck>),
    #[error("the Listing is on {listing:?}, not on {channel:?}")]
    OtherChannel {
        listing: SalesChannelKind,
        channel: SalesChannelKind,
    },
    #[error(transparent)]
    Listing(#[from] ListingError),
    #[error(transparent)]
    Pricing(#[from] PricingError),
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error(transparent)]
    Flag(#[from] FlagError),
    #[error("the repricing holds a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for RepricingError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => RepricingError::Unreadable(text),
            StoredValueError::Sql(error) => RepricingError::Sql(error),
        }
    }
}

const REPRICED_COLUMNS: &str = "sales_channel, channel_item_id, way, minimum, maximum, currency,
     status, status_detail, winning_price, checked_at, created_at
     FROM commerce_repricing WHERE deleted_at IS NULL";

/// Automatic Repricing and the price history of each Listing.
pub struct Repricing {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    flags: Flags,
    listings: Listings,
    pricing: Pricing,
}

impl Repricing {
    pub fn new(
        database: Arc<Database>,
        inventory: Arc<Inventory>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        let registry =
            Registry::new(&[crate::FLAGS]).expect("the Commerce's flags have distinct keys");
        Self {
            flags: Flags::new(database.clone(), registry, clock.clone(), ids.clone()),
            listings: Listings::new(database.clone(), clock.clone(), ids.clone()),
            pricing: Pricing::new(database.clone(), inventory, clock.clone(), ids.clone()),
            database,
            clock,
            ids,
        }
    }

    /// Whether the owner turned Automatic Repricing on.
    pub async fn is_on(&self) -> Result<bool, RepricingError> {
        Ok(self.flags.is_on(&AUTOMATIC_REPRICING).await?)
    }

    async fn ensure_on(&self) -> Result<(), RepricingError> {
        match self.flags.ensure_on(&AUTOMATIC_REPRICING).await {
            Err(FlagError::Off(_)) => Err(RepricingError::Off),
            done => Ok(done?),
        }
    }

    /// Checks `limits` for `listing`, every variation of it included: the
    /// flag on, a minimum above zero and up to the maximum, and one unit
    /// at the minimum keeping the Minimum Margin after the channel's fee,
    /// shipping, the tax and the Average Cost. Nothing is sent.
    pub async fn prepare(
        &self,
        listing: RecordId,
        limits: RepricingLimits,
        channel: &dyn SalesChannel,
        assumptions: PriceAssumptions,
    ) -> Result<RepricingRequest, RepricingError> {
        self.ensure_on().await?;
        let asked = self
            .listings
            .listing_on(listing, channel.sales_channel())
            .await?;
        let limits = RepricingLimits {
            minimum: limits.minimum.rounded(),
            maximum: limits.maximum.rounded(),
        };
        let currency = asked.listed.price.currency();
        if limits.minimum.currency() != currency
            || limits.maximum.currency() != currency
            || limits.minimum.amount() <= Decimal::ZERO
            || limits.minimum.amount() > limits.maximum.amount()
        {
            return Err(RepricingError::InvalidLimits);
        }
        let minimum = minimum_margin(self.database.connection()).await?;
        let check = self
            .pricing
            .check_margin(listing, limits.minimum, minimum, channel, assumptions)
            .await?;
        if !check.passes() {
            return Err(RepricingError::BelowMinimumMargin(Box::new(check)));
        }
        Ok(RepricingRequest {
            channel: asked.channel,
            item: asked.listed.id,
            limits,
            check,
        })
    }

    /// Turns Automatic Repricing on with the limits the owner confirmed,
    /// or changes them: on Mercado Livre the channel's automation starts
    /// at once; on Amazon the next Sync follows the featured offer.
    pub async fn turn_on(
        &self,
        confirmed: ConfirmedRepricing,
        port: RepricingPort<'_>,
    ) -> Result<RepricedListing, RepricingError> {
        self.ensure_on().await?;
        let request = confirmed.request;
        on_channel(request.channel, port.sales_channel())?;
        let reading = match port {
            RepricingPort::Automation(automation) => {
                let started = automation.automate(&request.item, &request.limits)?;
                let winning_price = automation.price_to_win(&request.item).unwrap_or(None);
                Some(RepricingReading {
                    status: started.status,
                    winning_price,
                    at: self.clock.now(),
                })
            }
            RepricingPort::FeaturedOffer(_) => None,
        };
        let now = stored(self.clock.now());
        self.database
            .connection()
            .execute(
                "INSERT INTO commerce_repricing
                     (id, sales_channel, channel_item_id, way, minimum, maximum, currency,
                      created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 ON CONFLICT (sales_channel, channel_item_id) WHERE deleted_at IS NULL
                 DO UPDATE SET way = excluded.way, minimum = excluded.minimum,
                     maximum = excluded.maximum, currency = excluded.currency,
                     updated_at = excluded.updated_at",
                params![
                    self.ids.next_id().to_string(),
                    request.channel.code(),
                    request.item.clone(),
                    port.way().code(),
                    request.limits.minimum.amount().to_string(),
                    request.limits.maximum.amount().to_string(),
                    request.limits.minimum.currency().code(),
                    now
                ],
            )
            .await?;
        if let Some(reading) = &reading {
            self.save_reading(request.channel, &request.item, reading)
                .await?;
        }
        self.repriced_item(request.channel, &request.item)
            .await?
            .ok_or_else(|| RepricingError::Unreadable(request.item))
    }

    /// Turns Automatic Repricing off for `listing`, every variation of it
    /// included: Mercado Livre's automation ends, and the price stays where
    /// it is. Off is the safe side, so the flag is not asked.
    pub async fn turn_off(
        &self,
        listing: RecordId,
        port: RepricingPort<'_>,
    ) -> Result<(), RepricingError> {
        let asked = self.listings.listing(listing).await?;
        on_channel(asked.channel, port.sales_channel())?;
        let Some(repriced) = self.repriced_item(asked.channel, &asked.listed.id).await? else {
            return Ok(());
        };
        self.end(&repriced, port).await
    }

    /// Every listing under Automatic Repricing.
    pub async fn repriced(&self) -> Result<Vec<RepricedListing>, RepricingError> {
        read_repriced(self.database.connection(), "", ()).await
    }

    /// The repricing of `listing`'s channel listing; `None` when it is not
    /// repriced.
    pub async fn repricing_of(
        &self,
        listing: RecordId,
    ) -> Result<Option<RepricedListing>, RepricingError> {
        let asked = self.listings.listing(listing).await?;
        self.repriced_item(asked.channel, &asked.listed.id).await
    }

    /// The price history of `listing`, the newest change first.
    pub async fn price_history(
        &self,
        listing: RecordId,
    ) -> Result<Vec<PriceChange>, RepricingError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT listing_id, old_price, new_price, currency, reason, created_at
                 FROM commerce_price_changes
                 WHERE listing_id = ?1 AND deleted_at IS NULL
                 ORDER BY created_at DESC, id DESC",
                params![listing.to_string()],
            )
            .await?;
        let mut history = Vec::new();
        while let Some(row) = rows.next().await? {
            history.push(change_from(&row)?);
        }
        Ok(history)
    }

    /// Reads Mercado Livre's automation of each listing repriced there, and
    /// its price to win. A listing whose automation the channel no longer
    /// has is no longer repriced. With the flag off, every automation the
    /// app turned on ends instead.
    pub async fn check_automations(
        &self,
        port: &dyn ChannelPriceAutomation,
    ) -> Result<RepricingRound, RepricingError> {
        let channel = port.sales_channel();
        let mut round = RepricingRound::default();
        if !self.is_on().await? {
            return self.stand_down(RepricingPort::Automation(port)).await;
        }
        for repriced in self.repriced_on(channel).await? {
            round.checked += 1;
            let automation = match port.automation(&repriced.listing) {
                Ok(automation) => automation,
                Err(error) => {
                    round.failed.push((repriced.listing, error));
                    continue;
                }
            };
            let Some(automation) = automation else {
                self.forget(channel, &repriced.listing).await?;
                round.ended += 1;
                continue;
            };
            let winning_price = match port.price_to_win(&repriced.listing) {
                Ok(price) => price,
                Err(PlatformError::NotFound) => None,
                Err(error) => {
                    round.failed.push((repriced.listing.clone(), error));
                    None
                }
            };
            let reading = RepricingReading {
                status: automation.status,
                winning_price,
                at: self.clock.now(),
            };
            self.save_reading(channel, &repriced.listing, &reading)
                .await?;
        }
        Ok(round)
    }

    /// Follows the featured offer of each listing repriced on `channel`
    /// that is on sale: its expected price, brought within the listing's
    /// limits, goes to the channel when it keeps the Minimum Margin and
    /// differs from today's; when it does not keep it, the lowest price
    /// within the limits that does goes instead, or none. One price per
    /// listing at most, each in the price history. With the flag off,
    /// nothing is sent and every listing stops being repriced.
    pub async fn follow_featured_offers(
        &self,
        prices: &dyn FeaturedOfferPrices,
        channel: &dyn SalesChannel,
        assumptions: PriceAssumptions,
    ) -> Result<RepricingRound, RepricingError> {
        let on = channel.sales_channel();
        on_channel(prices.sales_channel(), on)?;
        if !self.is_on().await? {
            return self.stand_down(RepricingPort::FeaturedOffer(prices)).await;
        }
        let mut round = RepricingRound::default();
        let mut following = Vec::new();
        for repriced in self.repriced_on(on).await? {
            let listings = self.listings.sharing_price(on, &repriced.listing).await?;
            let Some(listing) = listings
                .into_iter()
                .find(|listing| listing.listed.status == ListingStatus::Active)
            else {
                continue;
            };
            following.push((repriced, listing));
        }
        if following.is_empty() {
            return Ok(round);
        }
        let ids: Vec<String> = following
            .iter()
            .map(|(repriced, _)| repriced.listing.clone())
            .collect();
        let offers = prices.featured_offers(&ids)?;
        let minimum = minimum_margin(self.database.connection()).await?;
        for (repriced, listing) in following {
            round.checked += 1;
            let offer = offers
                .iter()
                .find(|(id, _)| *id == repriced.listing)
                .map_or(FeaturedOffer::NotEligible, |(_, offer)| *offer);
            let (status, winning_price) = match offer {
                FeaturedOffer::Expected(expected)
                    if expected.currency() == repriced.limits.minimum.currency() =>
                {
                    let target = repriced.limits.clamp(expected.rounded());
                    let price = self
                        .keeping_margin(
                            listing.id,
                            target,
                            &repriced.limits,
                            minimum,
                            channel,
                            assumptions,
                        )
                        .await?;
                    let status = match price {
                        Some(price) if price != listing.listed.price => {
                            match self
                                .listings
                                .send_price(
                                    &listing,
                                    price,
                                    channel,
                                    PriceChangeReason::FeaturedOffer,
                                )
                                .await
                            {
                                Ok(_) => round.sent += 1,
                                Err(ListingError::Platform(error)) => {
                                    round.failed.push((repriced.listing.clone(), error))
                                }
                                Err(error) => return Err(error.into()),
                            }
                            RepricingStatus::Following
                        }
                        Some(_) => RepricingStatus::Following,
                        None => RepricingStatus::MarginOutOfReach,
                    };
                    (status, Some(expected))
                }
                FeaturedOffer::Expected(_) | FeaturedOffer::NotEligible => {
                    (RepricingStatus::NotEligible, None)
                }
                FeaturedOffer::NoCompetition => (RepricingStatus::NoCompetition, None),
            };
            let reading = RepricingReading {
                status,
                winning_price,
                at: self.clock.now(),
            };
            self.save_reading(on, &repriced.listing, &reading).await?;
        }
        Ok(round)
    }

    /// `target` when one unit there keeps `minimum`; else the lowest price
    /// that keeps it, when that is above `target` and within `limits`;
    /// else none.
    async fn keeping_margin(
        &self,
        listing: RecordId,
        target: Money,
        limits: &RepricingLimits,
        minimum: Percentage,
        channel: &dyn SalesChannel,
        assumptions: PriceAssumptions,
    ) -> Result<Option<Money>, RepricingError> {
        let check = self
            .pricing
            .check_margin(listing, target, minimum, channel, assumptions)
            .await?;
        if check.passes() {
            return Ok(Some(target));
        }
        let Some(lowest) = check
            .lowest
            .filter(|lowest| lowest.amount() > target.amount() && limits.contains(*lowest))
        else {
            return Ok(None);
        };
        let at_lowest = self
            .pricing
            .check_margin(listing, lowest, minimum, channel, assumptions)
            .await?;
        Ok(at_lowest.passes().then_some(lowest))
    }

    /// With the flag off: every listing repriced on the port's channel
    /// stops, its automation ended on Mercado Livre.
    async fn stand_down(&self, port: RepricingPort<'_>) -> Result<RepricingRound, RepricingError> {
        let mut round = RepricingRound::default();
        for repriced in self.repriced_on(port.sales_channel()).await? {
            match self.end(&repriced, port).await {
                Ok(()) => round.ended += 1,
                Err(RepricingError::Platform(error)) => {
                    round.failed.push((repriced.listing, error))
                }
                Err(error) => return Err(error),
            }
        }
        Ok(round)
    }

    async fn end(
        &self,
        repriced: &RepricedListing,
        port: RepricingPort<'_>,
    ) -> Result<(), RepricingError> {
        if let RepricingPort::Automation(automation) = port {
            automation.stop(&repriced.listing)?;
        }
        self.forget(repriced.channel, &repriced.listing).await
    }

    async fn forget(&self, channel: SalesChannelKind, item: &str) -> Result<(), RepricingError> {
        self.database
            .connection()
            .execute(
                "UPDATE commerce_repricing SET deleted_at = ?1, updated_at = ?1
                 WHERE sales_channel = ?2 AND channel_item_id = ?3 AND deleted_at IS NULL",
                params![stored(self.clock.now()), channel.code(), item.to_owned()],
            )
            .await?;
        Ok(())
    }

    async fn save_reading(
        &self,
        channel: SalesChannelKind,
        item: &str,
        reading: &RepricingReading,
    ) -> Result<(), RepricingError> {
        self.database
            .connection()
            .execute(
                "UPDATE commerce_repricing
                 SET status = ?1, status_detail = ?2, winning_price = ?3, checked_at = ?4,
                     updated_at = ?4
                 WHERE sales_channel = ?5 AND channel_item_id = ?6 AND deleted_at IS NULL",
                params![
                    reading.status.code(),
                    reading.status.detail(),
                    reading
                        .winning_price
                        .map(|price| price.amount().to_string()),
                    stored(reading.at),
                    channel.code(),
                    item.to_owned()
                ],
            )
            .await?;
        Ok(())
    }

    async fn repriced_on(
        &self,
        channel: SalesChannelKind,
    ) -> Result<Vec<RepricedListing>, RepricingError> {
        read_repriced(
            self.database.connection(),
            "AND sales_channel = ?1",
            params![channel.code()],
        )
        .await
    }

    async fn repriced_item(
        &self,
        channel: SalesChannelKind,
        item: &str,
    ) -> Result<Option<RepricedListing>, RepricingError> {
        Ok(read_repriced(
            self.database.connection(),
            "AND sales_channel = ?1 AND channel_item_id = ?2",
            params![channel.code(), item.to_owned()],
        )
        .await?
        .pop())
    }
}

fn on_channel(listing: SalesChannelKind, channel: SalesChannelKind) -> Result<(), RepricingError> {
    if listing == channel {
        Ok(())
    } else {
        Err(RepricingError::OtherChannel { listing, channel })
    }
}

/// The ways `channel`'s listings are repriced, by the channel's id.
pub(crate) async fn repricing_ways(
    on: &Connection,
    channel: SalesChannelKind,
) -> Result<Vec<(String, RepricingWay)>, StoredValueError> {
    let mut rows = on
        .query(
            "SELECT channel_item_id, way FROM commerce_repricing
             WHERE deleted_at IS NULL AND sales_channel = ?1",
            params![channel.code()],
        )
        .await?;
    let mut ways = Vec::new();
    while let Some(row) = rows.next().await? {
        let way: String = row.get(1)?;
        ways.push((
            row.get(0)?,
            RepricingWay::from_code(&way).ok_or(StoredValueError::Unreadable(way))?,
        ));
    }
    Ok(ways)
}

/// The ids of `channel`'s listings the channel's own automation reprices.
pub(crate) async fn automated_items(
    on: &Connection,
    channel: SalesChannelKind,
) -> Result<HashSet<String>, StoredValueError> {
    Ok(repricing_ways(on, channel)
        .await?
        .into_iter()
        .filter(|(_, way)| *way == RepricingWay::ChannelAutomation)
        .map(|(item, _)| item)
        .collect())
}

/// Keeps `listing`'s change from `old` to `new` in its price history.
pub(crate) async fn record_price_change(
    on: &Connection,
    id: RecordId,
    listing: RecordId,
    (old, new): (Money, Money),
    reason: PriceChangeReason,
    at: Timestamp,
) -> Result<(), libsql::Error> {
    on.execute(
        "INSERT INTO commerce_price_changes
             (id, listing_id, old_price, new_price, currency, reason, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![
            id.to_string(),
            listing.to_string(),
            old.amount().to_string(),
            new.amount().to_string(),
            new.currency().code(),
            reason.code(),
            stored(at)
        ],
    )
    .await?;
    Ok(())
}

async fn read_repriced(
    on: &Connection,
    filter: &str,
    params: impl libsql::params::IntoParams,
) -> Result<Vec<RepricedListing>, RepricingError> {
    let mut rows = on
        .query(
            &format!("SELECT {REPRICED_COLUMNS} {filter} ORDER BY sales_channel, channel_item_id"),
            params,
        )
        .await?;
    let mut repriced = Vec::new();
    while let Some(row) = rows.next().await? {
        repriced.push(repriced_from(&row)?);
    }
    Ok(repriced)
}

fn repriced_from(row: &Row) -> Result<RepricedListing, RepricingError> {
    let way: String = row.get(2)?;
    let currency = row.currency_at(5)?;
    let reading = match row.get::<Option<String>>(6)? {
        Some(code) => {
            let detail: Option<String> = row.get(7)?;
            Some(RepricingReading {
                status: RepricingStatus::from_code(&code, detail)
                    .ok_or(RepricingError::Unreadable(code))?,
                winning_price: match row.get::<Option<String>>(8)? {
                    Some(_) => Some(Money::new(row.decimal_at(8)?, currency)),
                    None => None,
                },
                at: row.time_at(9)?,
            })
        }
        None => None,
    };
    Ok(RepricedListing {
        channel: row.sales_channel_at(0)?,
        listing: row.get(1)?,
        way: RepricingWay::from_code(&way).ok_or(RepricingError::Unreadable(way))?,
        limits: RepricingLimits {
            minimum: Money::new(row.decimal_at(3)?, currency),
            maximum: Money::new(row.decimal_at(4)?, currency),
        },
        since: row.time_at(10)?,
        reading,
    })
}

fn change_from(row: &Row) -> Result<PriceChange, RepricingError> {
    let currency = row.currency_at(3)?;
    let reason: String = row.get(4)?;
    Ok(PriceChange {
        listing: row.id_at(0)?,
        old: Money::new(row.decimal_at(1)?, currency),
        new: Money::new(row.decimal_at(2)?, currency),
        reason: PriceChangeReason::from_code(&reason).ok_or(RepricingError::Unreadable(reason))?,
        at: row.time_at(5)?,
    })
}
