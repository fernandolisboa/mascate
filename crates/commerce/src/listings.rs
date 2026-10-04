//! Listings (#15): the owner's listings in the Sales Channel, read by a Sync,
//! and the Product each one sells. The Sales Channel is a port the Platform
//! adapters implement (ADR 0013). A listing with variations (colour, size)
//! becomes one Listing per variation, since each sells its own Product.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use libsql::{Connection, Row, TransactionBehavior, params};
use mascate_kernel::{
    Clock, IdGenerator, ListingType, Money, PlatformError, RecordId, Timestamp, fold, folded_words,
};
use mascate_platform::{Database, Migration, StoredRow, StoredValueError, stored};
use rust_decimal::Decimal;

use crate::{ChecklistItem, SaleFee};

/// Where a listing stands in the Sales Channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListingStatus {
    Active,
    Paused,
    /// Ended for good; the channel never sells it again.
    Closed,
    /// Held by the channel for review.
    UnderReview,
    /// Not selling for another reason the channel gives, such as a pending
    /// payment.
    Inactive,
}

impl ListingStatus {
    const ALL: [ListingStatus; 5] = [
        ListingStatus::Active,
        ListingStatus::Paused,
        ListingStatus::Closed,
        ListingStatus::UnderReview,
        ListingStatus::Inactive,
    ];

    pub(crate) fn code(self) -> &'static str {
        match self {
            ListingStatus::Active => "active",
            ListingStatus::Paused => "paused",
            ListingStatus::Closed => "closed",
            ListingStatus::UnderReview => "under_review",
            ListingStatus::Inactive => "inactive",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.code() == code)
    }
}

/// One variation of a listing that has several.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variation {
    /// The channel's id of the variation.
    pub id: String,
    /// What sets it apart, as "Cor: Preto · Tamanho: M".
    pub name: String,
}

/// A listing as the Sales Channel reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelListing {
    /// The channel's id of the listing; its variations share it.
    pub id: String,
    pub variation: Option<Variation>,
    pub title: String,
    pub price: Money,
    pub available_quantity: u32,
    pub status: ListingStatus,
    /// Where the listing opens in the browser.
    pub link: Option<String>,
    /// `None` for a type the app does not work with.
    pub listing_type: Option<ListingType>,
    /// The channel's category id.
    pub category: Option<String>,
    /// The code the owner gave it in the channel.
    pub seller_sku: Option<String>,
}

/// The Sales Channel the owner sells in, as a Platform's adapter reports it.
/// Calls block on the network, so they run off the UI thread.
pub trait SalesChannel: Send + Sync {
    /// The ids of the owner's active and paused listings.
    fn listing_ids(&self) -> Result<Vec<String>, PlatformError>;

    /// The listings with `ids`, in any status, one per variation. An id the
    /// channel no longer knows is left out.
    fn listings(&self, ids: &[String]) -> Result<Vec<ChannelListing>, PlatformError>;

    /// What the channel keeps of a sale at `price` in `category` with
    /// `listing_type`.
    fn sale_fee(
        &self,
        category: &str,
        price: Money,
        listing_type: ListingType,
    ) -> Result<SaleFee, PlatformError>;

    /// Changes the price of the listing `id`, every variation of it
    /// included, to `price` in whole cents.
    fn set_price(&self, id: &str, price: Money) -> Result<(), PlatformError>;
}

/// A listing of the owner's, as last synced, with the Product it sells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub id: RecordId,
    pub listed: ChannelListing,
    pub product: Option<RecordId>,
    pub synced_at: Timestamp,
}

/// A Product as the catalog knows it: what a link suggestion compares a
/// listing with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogProduct {
    pub id: RecordId,
    pub sku: String,
    pub name: String,
}

/// Why a Product is suggested for a listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestedBy {
    /// The listing's seller SKU is the Product's SKU.
    SellerSku,
    /// Every word of the Product's name is in the listing's title.
    Title,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkSuggestion {
    pub product: RecordId,
    pub by: SuggestedBy,
}

/// A listing still without a Product, and the one the app suggests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingToLink {
    pub listing: Listing,
    pub suggestion: Option<LinkSuggestion>,
}

/// What a Sync of listings did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListingSync {
    /// Listings the channel reported, one per variation.
    pub read: usize,
    pub new: usize,
    /// Listings whose title, price, stock, status or other details changed.
    pub changed: usize,
    /// Listings, or variations, the channel no longer has, now closed.
    pub gone: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ListingError {
    #[error("no Listing {0}")]
    UnknownListing(RecordId),
    #[error("no draft {0}")]
    UnknownDraft(RecordId),
    #[error("draft {0} is published and no longer changes")]
    Published(RecordId),
    #[error("the draft still lacks what its checklist blocks on")]
    Blocked(Vec<ChecklistItem>),
    #[error("draft {0} is published, but its description was not sent: {1}")]
    DescriptionNotSent(RecordId, PlatformError),
    #[error("a price is more than zero, in the Listing's currency")]
    InvalidPrice,
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error("the Listings hold a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for ListingError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => ListingError::Unreadable(text),
            StoredValueError::Sql(error) => ListingError::Sql(error),
        }
    }
}

pub(crate) const CREATE_LISTINGS: Migration = Migration {
    version: 2,
    name: "create listings",
    risky: false,
    // The channel ids are optional for the drafts of #16, which exist only in
    // the app until published.
    sql: "CREATE TABLE commerce_listings (
        id                 TEXT PRIMARY KEY,
        product_id         TEXT,
        ml_item_id         TEXT,
        ml_variation_id    TEXT,
        variation_name     TEXT,
        title              TEXT    NOT NULL,
        price              TEXT    NOT NULL,
        currency           TEXT    NOT NULL,
        available_quantity INTEGER NOT NULL,
        status             TEXT    NOT NULL,
        link               TEXT,
        listing_type       TEXT,
        category_id        TEXT,
        seller_sku         TEXT,
        synced_at          TEXT,
        created_at         TEXT    NOT NULL,
        updated_at         TEXT    NOT NULL,
        deleted_at         TEXT
    );
    CREATE UNIQUE INDEX commerce_listings_by_ml_id
        ON commerce_listings (ml_item_id, COALESCE(ml_variation_id, ''))
        WHERE ml_item_id IS NOT NULL AND deleted_at IS NULL;
    CREATE INDEX commerce_listings_by_product ON commerce_listings (product_id);",
};

const LISTING_COLUMNS: &str = "id, ml_item_id, ml_variation_id, variation_name, title, price,
     currency, available_quantity, status, link, listing_type, category_id, seller_sku,
     product_id, synced_at
     FROM commerce_listings WHERE deleted_at IS NULL AND ml_item_id IS NOT NULL";

/// The owner's Listings in the Sales Channel.
pub struct Listings {
    pub(crate) database: Arc<Database>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) ids: Arc<dyn IdGenerator>,
}

impl Listings {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            database,
            clock,
            ids,
        }
    }

    /// Reads the owner's active and paused listings, and the ones synced
    /// before that are no longer either, so a listing paused, closed or held
    /// for review in the channel shows it here. Each listing is kept by the
    /// channel's id, so a second Sync with the same answers changes nothing;
    /// the Product a Listing is linked to always stays.
    pub async fn sync(&self, channel: &dyn SalesChannel) -> Result<ListingSync, ListingError> {
        let mut wanted = channel.listing_ids()?;
        let mut seen: HashSet<String> = wanted.iter().cloned().collect();
        for id in self.open_channel_ids().await? {
            if seen.insert(id.clone()) {
                wanted.push(id);
            }
        }
        let reported = if wanted.is_empty() {
            Vec::new()
        } else {
            channel.listings(&wanted)?
        };
        let mut fresh: BTreeMap<(String, Option<String>), ChannelListing> = BTreeMap::new();
        for listing in reported {
            fresh.insert(key_of(&listing), listing);
        }

        let now = self.clock.now();
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let known = read(&transaction, "", ()).await?;
        let mut report = ListingSync {
            read: fresh.len(),
            ..ListingSync::default()
        };
        for listing in &known {
            let key = key_of(&listing.listed);
            match fresh.remove(&key) {
                Some(reported) if reported == listing.listed => {
                    transaction
                        .execute(
                            "UPDATE commerce_listings SET synced_at = ?1 WHERE id = ?2",
                            params![stored(now), listing.id.to_string()],
                        )
                        .await?;
                }
                Some(reported) => {
                    report.changed += 1;
                    write_reported(&transaction, listing.id, &reported, now).await?;
                }
                // Asked for, but the channel no longer has it: deleted, or a
                // variation taken off the listing.
                None if seen.contains(&key.0) && listing.listed.status != ListingStatus::Closed => {
                    report.gone += 1;
                    transaction
                        .execute(
                            "UPDATE commerce_listings SET status = ?1, synced_at = ?2,
                                 updated_at = ?2
                             WHERE id = ?3",
                            params![
                                ListingStatus::Closed.code(),
                                stored(now),
                                listing.id.to_string()
                            ],
                        )
                        .await?;
                }
                None => {}
            }
        }
        for reported in fresh.into_values() {
            report.new += 1;
            transaction
                .execute(
                    "INSERT INTO commerce_listings
                         (id, ml_item_id, ml_variation_id, variation_name, title, price,
                          currency, available_quantity, status, link, listing_type,
                          category_id, seller_sku, synced_at, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                             ?14, ?14)",
                    params![
                        self.ids.next_id().to_string(),
                        reported.id.clone(),
                        reported.variation.as_ref().map(|v| v.id.clone()),
                        reported.variation.as_ref().map(|v| v.name.clone()),
                        reported.title.clone(),
                        reported.price.amount().to_string(),
                        reported.price.currency().code(),
                        reported.available_quantity,
                        reported.status.code(),
                        reported.link.clone(),
                        reported.listing_type.map(ListingType::code),
                        reported.category.clone(),
                        reported.seller_sku.clone(),
                        stored(now)
                    ],
                )
                .await?;
        }
        transaction.commit().await?;
        Ok(report)
    }

    /// When the listings were last synced; `None` before the first Sync.
    pub async fn last_sync(&self) -> Result<Option<Timestamp>, ListingError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT MAX(synced_at) FROM commerce_listings WHERE deleted_at IS NULL",
                (),
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(row.optional_time_at(0)?),
            None => Ok(None),
        }
    }

    /// Every Listing, by title.
    pub async fn listings(&self) -> Result<Vec<Listing>, ListingError> {
        read(self.database.connection(), "", ()).await
    }

    pub async fn listing(&self, id: RecordId) -> Result<Listing, ListingError> {
        read(
            self.database.connection(),
            "AND id = ?1",
            params![id.to_string()],
        )
        .await?
        .pop()
        .ok_or(ListingError::UnknownListing(id))
    }

    /// The Listings still without a Product, apart from closed ones, each
    /// with the Product the app suggests among `products`: the one whose SKU
    /// is the listing's seller SKU or, failing that, the one whose name is
    /// in the title. Two Products that fit as well suggest neither.
    pub async fn to_link(
        &self,
        products: &[CatalogProduct],
    ) -> Result<Vec<ListingToLink>, ListingError> {
        Ok(self
            .listings()
            .await?
            .into_iter()
            .filter(|listing| {
                listing.product.is_none() && listing.listed.status != ListingStatus::Closed
            })
            .map(|listing| ListingToLink {
                suggestion: suggest(&listing.listed, products),
                listing,
            })
            .collect())
    }

    /// Links a Listing to the Product it sells, replacing any earlier link.
    /// The caller knows the Product exists: the catalog owns Products.
    pub async fn link(
        &self,
        listing: RecordId,
        product: RecordId,
    ) -> Result<Listing, ListingError> {
        self.set_product(listing, Some(product)).await
    }

    /// Takes a Listing back to the ones still to link.
    pub async fn unlink(&self, listing: RecordId) -> Result<Listing, ListingError> {
        self.set_product(listing, None).await
    }

    /// Sends `price`, rounded to cents, to the channel as the price of the
    /// Listing and of every variation of the same listing, which share it
    /// (ADR 0014), and keeps it once the channel takes it. Returns those
    /// Listings. Only the owner's own action calls this: nothing in the app
    /// changes a price on its own.
    pub async fn change_price(
        &self,
        listing: RecordId,
        price: Money,
        channel: &dyn SalesChannel,
    ) -> Result<Vec<Listing>, ListingError> {
        let listed = self.listing(listing).await?.listed;
        let price = price.rounded();
        if price.currency() != listed.price.currency() || price.amount() <= Decimal::ZERO {
            return Err(ListingError::InvalidPrice);
        }
        channel.set_price(&listed.id, price)?;
        self.database
            .connection()
            .execute(
                "UPDATE commerce_listings SET price = ?1, updated_at = ?2
                 WHERE ml_item_id = ?3 AND deleted_at IS NULL",
                params![
                    price.amount().to_string(),
                    stored(self.clock.now()),
                    listed.id.clone()
                ],
            )
            .await?;
        self.sharing_price(&listed.id).await
    }

    /// The Listings of the channel listing `id`: one, or one per variation.
    pub(crate) async fn sharing_price(&self, id: &str) -> Result<Vec<Listing>, ListingError> {
        read(
            self.database.connection(),
            "AND ml_item_id = ?1",
            params![id.to_owned()],
        )
        .await
    }

    async fn set_product(
        &self,
        listing: RecordId,
        product: Option<RecordId>,
    ) -> Result<Listing, ListingError> {
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE commerce_listings SET product_id = ?1, updated_at = ?2
                 WHERE id = ?3 AND deleted_at IS NULL",
                params![
                    product.map(|id| id.to_string()),
                    stored(self.clock.now()),
                    listing.to_string()
                ],
            )
            .await?;
        if changed == 0 {
            return Err(ListingError::UnknownListing(listing));
        }
        self.listing(listing).await
    }

    /// The channel ids of the Listings a Sync still reads: every one not
    /// closed.
    async fn open_channel_ids(&self) -> Result<Vec<String>, ListingError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT DISTINCT ml_item_id FROM commerce_listings
                 WHERE deleted_at IS NULL AND ml_item_id IS NOT NULL AND status <> ?1
                 ORDER BY ml_item_id",
                params![ListingStatus::Closed.code()],
            )
            .await?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next().await? {
            ids.push(row.get(0)?);
        }
        Ok(ids)
    }
}

fn key_of(listing: &ChannelListing) -> (String, Option<String>) {
    (
        listing.id.clone(),
        listing
            .variation
            .as_ref()
            .map(|variation| variation.id.clone()),
    )
}

/// Writes what the channel reported over a Listing; its Product stays.
async fn write_reported(
    on: &Connection,
    id: RecordId,
    reported: &ChannelListing,
    now: Timestamp,
) -> Result<(), ListingError> {
    on.execute(
        "UPDATE commerce_listings
         SET variation_name = ?1, title = ?2, price = ?3, currency = ?4,
             available_quantity = ?5, status = ?6, link = ?7, listing_type = ?8,
             category_id = ?9, seller_sku = ?10, synced_at = ?11, updated_at = ?11
         WHERE id = ?12",
        params![
            reported.variation.as_ref().map(|v| v.name.clone()),
            reported.title.clone(),
            reported.price.amount().to_string(),
            reported.price.currency().code(),
            reported.available_quantity,
            reported.status.code(),
            reported.link.clone(),
            reported.listing_type.map(ListingType::code),
            reported.category.clone(),
            reported.seller_sku.clone(),
            stored(now),
            id.to_string()
        ],
    )
    .await?;
    Ok(())
}

async fn read(
    on: &Connection,
    filter: &str,
    params: impl libsql::params::IntoParams,
) -> Result<Vec<Listing>, ListingError> {
    let mut rows = on
        .query(
            &format!(
                "SELECT {LISTING_COLUMNS} {filter}
                 ORDER BY title COLLATE NOCASE, variation_name COLLATE NOCASE, ml_item_id"
            ),
            params,
        )
        .await?;
    let mut listings = Vec::new();
    while let Some(row) = rows.next().await? {
        listings.push(listing_from(&row)?);
    }
    Ok(listings)
}

fn listing_from(row: &Row) -> Result<Listing, ListingError> {
    let variation = match (row.get::<Option<String>>(2)?, row.get::<Option<String>>(3)?) {
        (Some(id), name) => Some(Variation {
            id,
            name: name.unwrap_or_default(),
        }),
        (None, _) => None,
    };
    let status: String = row.get(8)?;
    let listing_type: Option<String> = row.get(10)?;
    Ok(Listing {
        id: row.id_at(0)?,
        listed: ChannelListing {
            id: row.get(1)?,
            variation,
            title: row.get(4)?,
            price: Money::new(row.decimal_at(5)?, row.currency_at(6)?),
            available_quantity: row.get(7)?,
            status: ListingStatus::from_code(&status).ok_or(ListingError::Unreadable(status))?,
            link: row.get(9)?,
            listing_type: listing_type.and_then(|code| ListingType::from_code(&code)),
            category: row.get(11)?,
            seller_sku: row.get(12)?,
        },
        product: row.optional_id_at(13)?,
        synced_at: row.time_at(14)?,
    })
}

fn suggest(listing: &ChannelListing, products: &[CatalogProduct]) -> Option<LinkSuggestion> {
    if let Some(seller_sku) = listing.seller_sku.as_deref().map(sku_key)
        && !seller_sku.is_empty()
    {
        let by_sku: Vec<_> = products
            .iter()
            .filter(|product| sku_key(&product.sku) == seller_sku)
            .collect();
        if let [product] = by_sku[..] {
            return Some(LinkSuggestion {
                product: product.id,
                by: SuggestedBy::SellerSku,
            });
        }
    }
    let mut title: HashSet<String> = folded_words(&listing.title).into_iter().collect();
    if let Some(variation) = &listing.variation {
        title.extend(folded_words(&variation.name));
    }
    let mut best: Option<(usize, RecordId)> = None;
    let mut tied = false;
    for product in products {
        let words: HashSet<String> = folded_words(&product.name).into_iter().collect();
        if words.is_empty() || !words.is_subset(&title) {
            continue;
        }
        match best {
            Some((most, _)) if words.len() < most => {}
            Some((most, _)) if words.len() == most => tied = true,
            _ => {
                best = Some((words.len(), product.id));
                tied = false;
            }
        }
    }
    best.filter(|_| !tied).map(|(_, product)| LinkSuggestion {
        product,
        by: SuggestedBy::Title,
    })
}

/// A SKU's letters and digits only, so "fon-ouv 001" and "FON-OUV-001" are
/// the same code.
fn sku_key(sku: &str) -> String {
    sku.chars().filter_map(fold).collect()
}
