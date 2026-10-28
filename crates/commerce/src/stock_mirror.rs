//! Stock Mirror (#18, #125): the stock of each published Listing follows
//! its Stock Share of the units on hand of the Product it sells. Each
//! Listing keeps the stock last sent and the last failure; what is still to
//! send is derived from the ledger every time, so a failed send is never
//! lost and sending the same stock twice writes nothing (ADR 0017). The
//! units of a Product are dealt across its Listings, so the same unit is
//! never on sale in two places (ADR 0033). An On-Order Listing takes no
//! share: it shows the orders it still takes (ADR 0038).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use libsql::{Connection, Row, TransactionBehavior, params};
use mascate_inventory::Inventory;
use mascate_kernel::{Clock, IdGenerator, PlatformError, RecordId, SalesChannelKind, Timestamp};
use mascate_platform::{Database, Migration, StoredRow, stored};

use crate::{
    ChannelRanking, ChannelStock, ListingError, ListingStatus, Listings, OrderStatus, SalesChannel,
    ShipmentStatus,
};

pub(crate) const ADD_STOCK_SENT: Migration = Migration {
    version: 5,
    name: "keep the stock sent to the channel",
    risky: false,
    sql: "CREATE TABLE commerce_listing_stock (
        id                   TEXT    PRIMARY KEY,
        listing_id           TEXT    NOT NULL UNIQUE,
        stock_sent           INTEGER,
        stock_sent_at        TEXT,
        stock_failure        TEXT,
        stock_failure_detail TEXT,
        stock_failed_at      TEXT,
        created_at           TEXT    NOT NULL,
        updated_at           TEXT    NOT NULL,
        deleted_at           TEXT
    );",
};

/// Rounds of sending one call makes at most. A movement recorded while a
/// round was out leaves the stock it sent behind; the next round sends the
/// new one.
const ROUNDS: usize = 3;

/// Stock the channel took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockSent {
    pub quantity: u32,
    pub at: Timestamp,
}

/// Why the last try to send a Listing's stock failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockNotSent {
    pub error: PlatformError,
    pub at: Timestamp,
}

/// The orders an On-Order Listing takes at once, in place of a Stock
/// Share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderSlots {
    /// How many the owner accepts.
    pub accepted: u32,
    /// Units sold on the listing, in Orders not cancelled, still waiting
    /// for dispatch.
    pub waiting: u32,
}

impl OrderSlots {
    /// The orders that still fit: what the channel is to show.
    pub fn open(self) -> u32 {
        self.accepted.saturating_sub(self.waiting)
    }
}

/// How a Listing's stock follows the app's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirroredStock {
    pub listing: RecordId,
    /// Units on hand of the Listing's Product, all Stock Locations together.
    /// `None` when the Listing takes no Stock Share: a Listing without a
    /// Product, neither on sale nor paused by the channel for lack of stock,
    /// whose Product never moved in stock, or an On-Order Listing.
    pub on_hand: Option<u32>,
    /// The Listing's Stock Share of those units: what the channel is to
    /// show. The shares of a Product's Listings add up to its units on hand.
    pub share: Option<u32>,
    /// Set on an On-Order Listing: the orders it takes, whose open ones the
    /// channel is to show while it is on sale or paused for lack of stock.
    pub on_order: Option<OrderSlots>,
    /// The units still to send: the share, or the open orders, when it
    /// differs from the last sent (or, before the first send, from the
    /// channel's) or from that less the units the Orders sold since.
    pub to_send: Option<u32>,
    pub last_sent: Option<StockSent>,
    /// Set while the stock still to send last failed to reach the channel.
    pub last_failure: Option<StockNotSent>,
}

/// What a send did, in Listings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StockSend {
    pub sent: usize,
    pub failed: usize,
    /// Raises still waiting for a lowering of the same Product to reach its
    /// channel, so no unit is on sale in two places meanwhile.
    pub held: usize,
}

/// The stock of the owner's Listings, following the Inventory.
pub struct StockMirror {
    listings: Listings,
    ranking: ChannelRanking,
    inventory: Arc<Inventory>,
}

/// A Listing as the mirror reads it.
struct Mirrored {
    stock: MirroredStock,
    channel: SalesChannelKind,
    item: String,
    variation: Option<String>,
    product: Option<RecordId>,
    /// Shows open orders, so its sends never hold or wait for a Stock Share.
    on_order: bool,
    /// The stock last sent or, before the first send, the channel's.
    baseline: u32,
    /// What the channel shows now as far as the app knows: the baseline
    /// less the units its Orders sold since.
    shown: u32,
}

impl Mirrored {
    /// Whether sending its stock shows no more units than the channel
    /// shows now.
    fn lowers(&self) -> bool {
        self.stock.to_send.is_some_and(|units| units <= self.shown)
    }
}

/// A Listing as read, before its Stock Share is dealt.
struct Read {
    mirrored: Mirrored,
    /// On sale, or paused by the channel for lack of stock.
    follows: bool,
    created_at: Timestamp,
}

/// Where a send stands while it goes.
#[derive(Default)]
struct Sending {
    sent: BTreeSet<RecordId>,
    failed: BTreeSet<RecordId>,
    held: BTreeSet<RecordId>,
    /// Channels that refused everything for the rest of the send.
    down: BTreeSet<SalesChannelKind>,
    /// The channel listings sent to, to read back.
    items: BTreeMap<SalesChannelKind, BTreeSet<String>>,
}

impl StockMirror {
    pub fn new(
        database: Arc<Database>,
        inventory: Arc<Inventory>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            ranking: ChannelRanking::new(database.clone(), clock.clone(), ids.clone()),
            listings: Listings::new(database, clock, ids),
            inventory,
        }
    }

    /// The stock of every Listing in every channel, by title like the
    /// Listings.
    pub async fn mirrored(&self) -> Result<Vec<MirroredStock>, ListingError> {
        Ok(self
            .read()
            .await?
            .into_iter()
            .map(|mirrored| mirrored.stock)
            .collect())
    }

    /// Sends every stock still to send to the Listings of `channels`, one
    /// request per channel listing, and reads the listings sent back: a
    /// channel pauses one left with none and puts it back on sale when
    /// units return. In each round the sends that show fewer units than the
    /// channel shows now (the last sent less what its Orders sold since) go
    /// first, and a Product's sends that show more wait until every
    /// lowering of that Product got to its channel, this call's `channels`
    /// or not, so a lost or failed send never puts the same unit on sale
    /// twice. A failure stays on the Listing until a later send gets
    /// through. The caller syncs every channel's Orders first, so a send
    /// never puts back on sale a unit a buyer took (ADR 0017, ADR 0033).
    pub async fn send(&self, channels: &[&dyn SalesChannel]) -> Result<StockSend, ListingError> {
        let mut sending = Sending::default();
        for _ in 0..ROUNDS {
            let mirrored = self.read().await?;
            let lowerings = self.due(&mirrored, channels, &sending, Mirrored::lowers);
            let mut tried = self.send_each(channels, lowerings, &mut sending).await?;

            let mirrored = self.read().await?;
            let lowering: BTreeSet<RecordId> = mirrored
                .iter()
                .filter(|mirrored| mirrored.lowers() && !mirrored.on_order)
                .filter_map(|mirrored| mirrored.product)
                .collect();
            sending.held.clear();
            let mut raises = Vec::new();
            for mirrored in self.due(&mirrored, channels, &sending, |due| !due.lowers()) {
                if !mirrored.on_order
                    && mirrored
                        .product
                        .is_some_and(|product| lowering.contains(&product))
                {
                    sending.held.insert(mirrored.stock.listing);
                } else {
                    raises.push(mirrored);
                }
            }
            tried |= self.send_each(channels, raises, &mut sending).await?;
            if !tried {
                break;
            }
        }
        for channel in channels {
            let on = channel.sales_channel();
            let Some(items) = sending.items.remove(&on) else {
                continue;
            };
            let ids: Vec<String> = items.into_iter().collect();
            // The stock is sent either way; the next Sync reads the status.
            if let Ok(reported) = channel.listings(&ids) {
                self.listings.take_reported(on, reported).await?;
            }
        }
        Ok(StockSend {
            sent: sending.sent.len(),
            failed: sending.failed.len(),
            held: sending.held.len(),
        })
    }

    /// The Listings of `channels` still to send that `wanted` picks, leaving
    /// out the ones that failed in this send and the channels down.
    fn due<'a>(
        &self,
        mirrored: &'a [Mirrored],
        channels: &[&dyn SalesChannel],
        sending: &Sending,
        wanted: impl Fn(&Mirrored) -> bool,
    ) -> Vec<&'a Mirrored> {
        mirrored
            .iter()
            .filter(|mirrored| {
                mirrored.stock.to_send.is_some()
                    && wanted(mirrored)
                    && !sending.failed.contains(&mirrored.stock.listing)
                    && !sending.down.contains(&mirrored.channel)
                    && channels
                        .iter()
                        .any(|channel| channel.sales_channel() == mirrored.channel)
            })
            .collect()
    }

    /// Sends `due`, one request per channel listing. Returns whether any
    /// request went out.
    async fn send_each(
        &self,
        channels: &[&dyn SalesChannel],
        due: Vec<&Mirrored>,
        sending: &mut Sending,
    ) -> Result<bool, ListingError> {
        let mut by_item: BTreeMap<(SalesChannelKind, String), Vec<&Mirrored>> = BTreeMap::new();
        for mirrored in due {
            by_item
                .entry((mirrored.channel, mirrored.item.clone()))
                .or_default()
                .push(mirrored);
        }
        let mut tried = false;
        for ((on, item), listings) in by_item {
            let Some(channel) = channels
                .iter()
                .find(|channel| channel.sales_channel() == on)
            else {
                continue;
            };
            if sending.down.contains(&on) {
                continue;
            }
            tried = true;
            let stock: Vec<ChannelStock> = listings
                .iter()
                .map(|mirrored| ChannelStock {
                    variation: mirrored.variation.clone(),
                    available_quantity: mirrored.stock.to_send.unwrap_or_default(),
                })
                .collect();
            match channel.set_stock(&item, &stock) {
                Ok(()) => {
                    for mirrored in &listings {
                        self.record_sent(&mirrored.stock).await?;
                        sending.sent.insert(mirrored.stock.listing);
                    }
                    sending.items.entry(on).or_default().insert(item);
                }
                Err(error) => {
                    for mirrored in &listings {
                        self.record_failure(mirrored.stock.listing, &error).await?;
                        sending.sent.remove(&mirrored.stock.listing);
                        sending.failed.insert(mirrored.stock.listing);
                    }
                    // The channel will refuse the others the same way.
                    if matches!(
                        error,
                        PlatformError::NotConnected
                            | PlatformError::Expired
                            | PlatformError::RateLimited
                    ) {
                        let also = self.still_due(on, &sending.failed, &error).await?;
                        sending.failed.extend(also);
                        sending.down.insert(on);
                    }
                }
            }
        }
        Ok(tried)
    }

    /// Records `error` on every Listing of `channel` still due outside
    /// `failed`, and returns them.
    async fn still_due(
        &self,
        channel: SalesChannelKind,
        failed: &BTreeSet<RecordId>,
        error: &PlatformError,
    ) -> Result<Vec<RecordId>, ListingError> {
        let mut also = Vec::new();
        for mirrored in self.read().await? {
            let listing = mirrored.stock.listing;
            if mirrored.channel == channel
                && mirrored.stock.to_send.is_some()
                && !failed.contains(&listing)
            {
                self.record_failure(listing, error).await?;
                also.push(listing);
            }
        }
        Ok(also)
    }

    async fn record_sent(&self, stock: &MirroredStock) -> Result<(), ListingError> {
        let connection = self.listings.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        record_stock_sent(
            &transaction,
            stock.listing,
            stock.to_send.unwrap_or_default(),
            self.listings.clock.now(),
            self.listings.ids.next_id(),
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn record_failure(
        &self,
        listing: RecordId,
        error: &PlatformError,
    ) -> Result<(), ListingError> {
        let (code, detail) = failure_code(error);
        self.listings
            .database
            .connection()
            .execute(
                "INSERT INTO commerce_listing_stock (id, listing_id, stock_failure,
                     stock_failure_detail, stock_failed_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?5)
                 ON CONFLICT (listing_id) DO UPDATE SET
                     stock_failure = excluded.stock_failure,
                     stock_failure_detail = excluded.stock_failure_detail,
                     stock_failed_at = excluded.stock_failed_at,
                     updated_at = excluded.updated_at",
                params![
                    self.listings.ids.next_id().to_string(),
                    listing.to_string(),
                    code,
                    detail,
                    stored(self.listings.clock.now())
                ],
            )
            .await?;
        Ok(())
    }

    async fn read(&self) -> Result<Vec<Mirrored>, ListingError> {
        let on_hand: HashMap<RecordId, u32> = self
            .inventory
            .stock()
            .await?
            .products
            .into_iter()
            .map(|stock| {
                let units = stock.valuation.quantity().max(0);
                (stock.product, u32::try_from(units).unwrap_or(u32::MAX))
            })
            .collect();
        let ranking = self.ranking.ranking().await?;
        let mut rows = self
            .listings
            .database
            .connection()
            .query(
                "SELECT listing.id, listing.channel_item_id, listing.channel_variation_id,
                     listing.product_id, listing.status, listing.available_quantity,
                     stock.stock_sent, stock.stock_sent_at, stock.stock_failure,
                     stock.stock_failure_detail, stock.stock_failed_at, listing.sales_channel,
                     listing.created_at,
                     (SELECT COALESCE(SUM(line.quantity), 0)
                      FROM commerce_order_lines line
                      JOIN commerce_orders ord
                          ON ord.id = line.order_id AND ord.deleted_at IS NULL
                      WHERE line.deleted_at IS NULL AND ord.status <> ?1
                          AND ord.sales_channel = listing.sales_channel
                          AND line.channel_item_id = listing.channel_item_id
                          AND COALESCE(line.channel_variation_id, '') =
                              COALESCE(listing.channel_variation_id, '')
                          AND ord.ordered_at >= COALESCE(stock.stock_sent_at, listing.synced_at)),
                     on_order.accepted_orders,
                     CASE WHEN on_order.id IS NULL THEN 0 ELSE
                     (SELECT COALESCE(SUM(line.quantity), 0)
                      FROM commerce_order_lines line
                      JOIN commerce_orders ord
                          ON ord.id = line.order_id AND ord.deleted_at IS NULL
                      WHERE line.deleted_at IS NULL AND ord.status <> ?1
                          AND ord.sales_channel = listing.sales_channel
                          AND line.channel_item_id = listing.channel_item_id
                          AND COALESCE(line.channel_variation_id, '') =
                              COALESCE(listing.channel_variation_id, '')
                          AND COALESCE(ord.shipment_status, '') NOT IN (?2, ?3, ?4, ?5))
                     END
                 FROM commerce_listings listing
                 LEFT JOIN commerce_listing_stock stock
                     ON stock.listing_id = listing.id AND stock.deleted_at IS NULL
                 LEFT JOIN commerce_on_order_listings on_order
                     ON on_order.listing_id = listing.id AND on_order.deleted_at IS NULL
                 WHERE listing.deleted_at IS NULL AND listing.channel_item_id IS NOT NULL
                 ORDER BY listing.title COLLATE NOCASE, listing.variation_name COLLATE NOCASE,
                     listing.sales_channel, listing.channel_item_id",
                params![
                    OrderStatus::Cancelled.code(),
                    ShipmentStatus::Shipped.code(),
                    ShipmentStatus::Delivered.code(),
                    ShipmentStatus::NotDelivered.code(),
                    ShipmentStatus::Cancelled.code()
                ],
            )
            .await?;
        let mut read = Vec::new();
        while let Some(row) = rows.next().await? {
            read.push(read_from(&row)?);
        }
        let shares = shares(&read, &on_hand, &ranking);
        Ok(read
            .into_iter()
            .map(
                |Read {
                     mut mirrored,
                     follows,
                     ..
                 }| {
                    let listing = mirrored.stock.listing;
                    let due = match (mirrored.stock.on_order, shares.get(&listing)) {
                        (Some(slots), _) => follows.then_some(slots.open()),
                        (None, Some(&share)) => {
                            mirrored.stock.on_hand = mirrored
                                .product
                                .and_then(|product| on_hand.get(&product).copied());
                            mirrored.stock.share = Some(share);
                            Some(share)
                        }
                        (None, None) => None,
                    };
                    // A sale the Orders know of took units the share may give
                    // back to the channel; one they do not know of yet is never
                    // undone, since the baseline stays.
                    mirrored.stock.to_send =
                        due.filter(|&units| units != mirrored.baseline || units != mirrored.shown);
                    // A failure for stock no longer due is history.
                    if mirrored.stock.to_send.is_none() {
                        mirrored.stock.last_failure = None;
                    }
                    mirrored
                },
            )
            .collect())
    }
}

/// Keeps `quantity` as the stock `listing` last took in its channel, and
/// as its units there; `id` is the stock row's, should it be the first.
pub(crate) async fn record_stock_sent(
    on: &Connection,
    listing: RecordId,
    quantity: u32,
    now: Timestamp,
    id: RecordId,
) -> Result<(), ListingError> {
    let now = stored(now);
    on.execute(
        "UPDATE commerce_listings SET available_quantity = ?1, updated_at = ?2 WHERE id = ?3",
        params![quantity, now.clone(), listing.to_string()],
    )
    .await?;
    on.execute(
        "INSERT INTO commerce_listing_stock
             (id, listing_id, stock_sent, stock_sent_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4, ?4)
         ON CONFLICT (listing_id) DO UPDATE SET
             stock_sent = excluded.stock_sent, stock_sent_at = excluded.stock_sent_at,
             stock_failure = NULL, stock_failure_detail = NULL,
             stock_failed_at = NULL, updated_at = excluded.updated_at",
        params![id.to_string(), listing.to_string(), quantity, now],
    )
    .await?;
    Ok(())
}

/// The Stock Share of each Listing that takes one: each Product's units
/// dealt one at a time, round robin, over its Listings in the channels'
/// ranking and, within a channel, oldest first (ADR 0033). Products that
/// never moved in stock are left out.
fn shares(
    read: &[Read],
    on_hand: &HashMap<RecordId, u32>,
    ranking: &[SalesChannelKind],
) -> HashMap<RecordId, u32> {
    let rank = |channel: SalesChannelKind| {
        ranking
            .iter()
            .position(|ranked| *ranked == channel)
            .unwrap_or(ranking.len())
    };
    let mut by_product: BTreeMap<RecordId, Vec<&Read>> = BTreeMap::new();
    for listing in read
        .iter()
        .filter(|listing| listing.follows && !listing.mirrored.on_order)
    {
        if let Some(product) = listing.mirrored.product
            && on_hand.contains_key(&product)
        {
            by_product.entry(product).or_default().push(listing);
        }
    }
    let mut shares = HashMap::new();
    for (product, mut listings) in by_product {
        listings.sort_by_key(|listing| {
            (
                rank(listing.mirrored.channel),
                listing.mirrored.channel,
                listing.created_at,
                listing.mirrored.stock.listing,
            )
        });
        let units = on_hand[&product];
        let count = u32::try_from(listings.len()).unwrap_or(u32::MAX);
        for (place, listing) in (0..).zip(listings) {
            let extra = u32::from(place < units % count);
            shares.insert(listing.mirrored.stock.listing, units / count + extra);
        }
    }
    shares
}

fn read_from(row: &Row) -> Result<Read, ListingError> {
    let status: String = row.get(4)?;
    let status = ListingStatus::from_code(&status)
        .ok_or_else(|| ListingError::Unreadable(status.clone()))?;
    let in_channel: u32 = row.get(5)?;
    let last_sent = match row.get::<Option<u32>>(6)? {
        Some(quantity) => Some(StockSent {
            quantity,
            at: row.time_at(7)?,
        }),
        None => None,
    };
    let last_failure = match row.get::<Option<String>>(8)? {
        Some(code) => Some(StockNotSent {
            error: failure_from(&code, row.get(9)?),
            at: row.time_at(10)?,
        }),
        None => None,
    };
    let baseline = last_sent.map_or(in_channel, |sent| sent.quantity);
    let sold_since: u32 = row.get(13)?;
    let on_order = match row.get::<Option<u32>>(14)? {
        Some(accepted) => Some(OrderSlots {
            accepted,
            waiting: row.get(15)?,
        }),
        None => None,
    };
    Ok(Read {
        mirrored: Mirrored {
            stock: MirroredStock {
                listing: row.id_at(0)?,
                on_hand: None,
                share: None,
                on_order,
                to_send: None,
                last_sent,
                last_failure,
            },
            channel: row.sales_channel_at(11)?,
            item: row.get(1)?,
            variation: row.get(2)?,
            product: row.optional_id_at(3)?,
            on_order: on_order.is_some(),
            baseline,
            shown: baseline.saturating_sub(sold_since),
        },
        follows: matches!(status, ListingStatus::Active | ListingStatus::OutOfStock),
        created_at: row.time_at(12)?,
    })
}

fn failure_code(error: &PlatformError) -> (&'static str, Option<String>) {
    match error {
        PlatformError::NotConnected => ("not_connected", None),
        PlatformError::Expired => ("expired", None),
        PlatformError::RateLimited => ("rate_limited", None),
        PlatformError::Refused(why) => ("refused", Some(why.clone())),
        PlatformError::NotFound => ("not_found", None),
        PlatformError::Failed(why) => ("failed", Some(why.clone())),
    }
}

fn failure_from(code: &str, detail: Option<String>) -> PlatformError {
    match code {
        "not_connected" => PlatformError::NotConnected,
        "expired" => PlatformError::Expired,
        "rate_limited" => PlatformError::RateLimited,
        "refused" => PlatformError::Refused(detail.unwrap_or_default()),
        "not_found" => PlatformError::NotFound,
        _ => PlatformError::Failed(detail.unwrap_or_default()),
    }
}
