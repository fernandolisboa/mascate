//! Stock Mirror (#18): the stock of each published Listing follows the
//! units on hand of the Product it sells. Each Listing keeps the stock last
//! sent and the last failure; what is still to send is derived from the
//! ledger every time, so a failed send is never lost and sending the same
//! stock twice writes nothing (ADR 0017).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use libsql::{Row, params};
use mascate_inventory::Inventory;
use mascate_kernel::{Clock, IdGenerator, PlatformError, RecordId, Timestamp};
use mascate_platform::{Database, Migration, StoredRow, stored};

use crate::{ChannelStock, ListingError, ListingStatus, Listings, SalesChannel};

pub(crate) const ADD_STOCK_SENT: Migration = Migration {
    version: 5,
    name: "keep the stock sent to the channel",
    risky: false,
    sql: "ALTER TABLE commerce_listings ADD COLUMN stock_sent INTEGER;
    ALTER TABLE commerce_listings ADD COLUMN stock_sent_at TEXT;
    ALTER TABLE commerce_listings ADD COLUMN stock_failure TEXT;
    ALTER TABLE commerce_listings ADD COLUMN stock_failure_detail TEXT;
    ALTER TABLE commerce_listings ADD COLUMN stock_failed_at TEXT;",
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

/// How a Listing's stock follows the app's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirroredStock {
    pub listing: RecordId,
    /// Units on hand of the Listing's Product, all Stock Locations together.
    /// `None` when the stock is not mirrored: a Listing without a Product,
    /// neither active nor paused, or whose Product never moved in stock.
    pub on_hand: Option<u32>,
    /// The units still to send: the ones on hand, when they differ from the
    /// last sent or, before the first send, from the channel's.
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
}

/// The stock of the owner's Listings, following the Inventory.
pub struct StockMirror {
    listings: Listings,
    inventory: Arc<Inventory>,
}

/// A Listing as the mirror reads it.
struct Mirrored {
    stock: MirroredStock,
    item: String,
    variation: Option<String>,
}

impl StockMirror {
    pub fn new(
        database: Arc<Database>,
        inventory: Arc<Inventory>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            listings: Listings::new(database, clock, ids),
            inventory,
        }
    }

    /// The stock of every Listing in the channel, by title like the
    /// Listings.
    pub async fn mirrored(&self) -> Result<Vec<MirroredStock>, ListingError> {
        Ok(self
            .read()
            .await?
            .into_iter()
            .map(|mirrored| mirrored.stock)
            .collect())
    }

    /// Sends every stock still to send, one request per channel listing,
    /// and reads the listings sent back: the channel pauses one left with
    /// none and puts it back on sale when units return. A failure stays on
    /// the Listing until a later send gets through; the app calls this
    /// after each stock movement and each Sync.
    pub async fn send(&self, channel: &dyn SalesChannel) -> Result<StockSend, ListingError> {
        let mut sent = BTreeSet::new();
        let mut failed = BTreeSet::new();
        let mut items_sent = BTreeSet::new();
        'rounds: for _ in 0..ROUNDS {
            let mut due: BTreeMap<String, Vec<Mirrored>> = BTreeMap::new();
            for mirrored in self.read().await? {
                if mirrored.stock.to_send.is_some() && !failed.contains(&mirrored.stock.listing) {
                    due.entry(mirrored.item.clone()).or_default().push(mirrored);
                }
            }
            if due.is_empty() {
                break;
            }
            for (item, listings) in due {
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
                            sent.insert(mirrored.stock.listing);
                        }
                        items_sent.insert(item);
                    }
                    Err(error) => {
                        for mirrored in &listings {
                            self.record_failure(mirrored.stock.listing, &error).await?;
                            sent.remove(&mirrored.stock.listing);
                            failed.insert(mirrored.stock.listing);
                        }
                        // The channel will refuse the others the same way.
                        if matches!(
                            error,
                            PlatformError::NotConnected
                                | PlatformError::Expired
                                | PlatformError::RateLimited
                        ) {
                            failed.extend(self.still_due(&failed, &error).await?);
                            break 'rounds;
                        }
                    }
                }
            }
        }
        if !items_sent.is_empty() {
            let ids: Vec<String> = items_sent.into_iter().collect();
            // The stock is sent either way; the next Sync reads the status.
            if let Ok(reported) = channel.listings(&ids) {
                self.listings.take_reported(reported).await?;
            }
        }
        Ok(StockSend {
            sent: sent.len(),
            failed: failed.len(),
        })
    }

    /// Records `error` on every Listing still due outside `failed`, and
    /// returns them.
    async fn still_due(
        &self,
        failed: &BTreeSet<RecordId>,
        error: &PlatformError,
    ) -> Result<Vec<RecordId>, ListingError> {
        let mut also = Vec::new();
        for mirrored in self.read().await? {
            let listing = mirrored.stock.listing;
            if mirrored.stock.to_send.is_some() && !failed.contains(&listing) {
                self.record_failure(listing, error).await?;
                also.push(listing);
            }
        }
        Ok(also)
    }

    async fn record_sent(&self, stock: &MirroredStock) -> Result<(), ListingError> {
        let quantity = stock.to_send.unwrap_or_default();
        self.listings
            .database
            .connection()
            .execute(
                "UPDATE commerce_listings
                 SET stock_sent = ?1, available_quantity = ?1, stock_sent_at = ?2,
                     stock_failure = NULL, stock_failure_detail = NULL,
                     stock_failed_at = NULL, updated_at = ?2
                 WHERE id = ?3",
                params![
                    quantity,
                    stored(self.listings.clock.now()),
                    stock.listing.to_string()
                ],
            )
            .await?;
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
                "UPDATE commerce_listings
                 SET stock_failure = ?1, stock_failure_detail = ?2, stock_failed_at = ?3,
                     updated_at = ?3
                 WHERE id = ?4",
                params![
                    code,
                    detail,
                    stored(self.listings.clock.now()),
                    listing.to_string()
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
        let mut rows = self
            .listings
            .database
            .connection()
            .query(
                "SELECT id, ml_item_id, ml_variation_id, product_id, status,
                     available_quantity, stock_sent, stock_sent_at, stock_failure,
                     stock_failure_detail, stock_failed_at
                 FROM commerce_listings WHERE deleted_at IS NULL AND ml_item_id IS NOT NULL
                 ORDER BY title COLLATE NOCASE, variation_name COLLATE NOCASE, ml_item_id",
                (),
            )
            .await?;
        let mut mirrored = Vec::new();
        while let Some(row) = rows.next().await? {
            mirrored.push(mirrored_from(&row, &on_hand)?);
        }
        Ok(mirrored)
    }
}

fn mirrored_from(row: &Row, on_hand: &HashMap<RecordId, u32>) -> Result<Mirrored, ListingError> {
    let status: String = row.get(4)?;
    let selling = ListingStatus::from_code(&status)
        .ok_or_else(|| ListingError::Unreadable(status.clone()))
        .map(|status| matches!(status, ListingStatus::Active | ListingStatus::Paused))?;
    let product = row.optional_id_at(3)?;
    let units = product
        .filter(|_| selling)
        .and_then(|product| on_hand.get(&product).copied());
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
    let to_send = units.filter(|&units| units != baseline);
    Ok(Mirrored {
        stock: MirroredStock {
            listing: row.id_at(0)?,
            on_hand: units,
            to_send,
            last_sent,
            // A failure for stock no longer due is history.
            last_failure: last_failure.filter(|_| to_send.is_some()),
        },
        item: row.get(1)?,
        variation: row.get(2)?,
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
