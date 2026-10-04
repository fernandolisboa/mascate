//! Listing drafts (#16): a Listing born in the app from a Product, with the
//! category the Sales Channel predicts, the category's attributes, pictures
//! from the Product's folder and a price, checked before it goes out, and
//! published only on the owner's click. The same Listing then carries the
//! channel's id (ADR 0016).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use libsql::{Connection, TransactionBehavior, params};
use mascate_kernel::{Currency, ListingType, Money, PlatformError, RecordId, Timestamp};
use mascate_platform::{Migration, StoredRow, stored};
use rust_decimal::Decimal;

use crate::{CatalogProduct, Listing, ListingError, ListingStatus, Listings};

/// Fewer pictures than this and the listing starts below the channel's
/// basic quality.
pub const MIN_PICTURES: usize = 3;

/// The picture files a Sales Channel takes.
const PICTURE_EXTENSIONS: [&str; 3] = ["jpg", "jpeg", "png"];

/// How a draft is kept in the Listings' status column until it is published.
const DRAFT_STATUS: &str = "draft";

pub(crate) const CREATE_DRAFTS: Migration = Migration {
    version: 4,
    name: "create listing drafts",
    risky: false,
    // A draft is a row of commerce_listings without the channel's ids; these
    // tables hold what only a draft has.
    sql: "CREATE TABLE commerce_listing_drafts (
        id                  TEXT    PRIMARY KEY,
        listing_id          TEXT    NOT NULL UNIQUE,
        description         TEXT    NOT NULL,
        item_condition      TEXT    NOT NULL,
        warranty            TEXT    NOT NULL,
        category_name       TEXT,
        publishing_since    TEXT,
        description_pending INTEGER NOT NULL DEFAULT 0,
        created_at          TEXT    NOT NULL,
        updated_at          TEXT    NOT NULL,
        deleted_at          TEXT
    );
    CREATE TABLE commerce_draft_attributes (
        id           TEXT    PRIMARY KEY,
        listing_id   TEXT    NOT NULL,
        position     INTEGER NOT NULL,
        attribute_id TEXT    NOT NULL,
        name         TEXT    NOT NULL,
        requirement  TEXT    NOT NULL,
        value        TEXT    NOT NULL,
        created_at   TEXT    NOT NULL,
        updated_at   TEXT    NOT NULL,
        deleted_at   TEXT
    );
    CREATE INDEX commerce_draft_attributes_by_listing ON commerce_draft_attributes (listing_id);
    CREATE TABLE commerce_draft_pictures (
        id                 TEXT    PRIMARY KEY,
        listing_id         TEXT    NOT NULL,
        position           INTEGER NOT NULL,
        path               TEXT    NOT NULL,
        channel_picture_id TEXT,
        created_at         TEXT    NOT NULL,
        updated_at         TEXT    NOT NULL,
        deleted_at         TEXT
    );
    CREATE INDEX commerce_draft_pictures_by_listing ON commerce_draft_pictures (listing_id);",
};

/// Whether `path` is a picture file a Sales Channel takes, by its extension.
pub fn is_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            PICTURE_EXTENSIONS
                .iter()
                .any(|taken| extension.eq_ignore_ascii_case(taken))
        })
}

/// What the item is, as the channel asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Condition {
    #[default]
    New,
    Used,
}

impl Condition {
    pub const ALL: [Condition; 2] = [Condition::New, Condition::Used];

    /// How it is stored; also Mercado Livre's own ids.
    pub fn code(self) -> &'static str {
        match self {
            Condition::New => "new",
            Condition::Used => "used",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|condition| condition.code() == code)
    }

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            Condition::New => "Novo",
            Condition::Used => "Usado",
        }
    }
}

/// A category of the Sales Channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelCategory {
    pub id: String,
    pub name: String,
}

/// An attribute's value, by the channel's attribute id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeValue {
    pub id: String,
    pub value: String,
}

/// A category the channel predicts for a title, with the attribute values
/// it read in the title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryPrediction {
    pub category: ChannelCategory,
    pub attributes: Vec<AttributeValue>,
}

/// How much the channel wants an attribute filled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    /// The channel refuses the listing without it.
    Required,
    /// The listing goes out without it, but loses exposure.
    Recommended,
    Optional,
}

impl Requirement {
    const ALL: [Requirement; 3] = [
        Requirement::Required,
        Requirement::Recommended,
        Requirement::Optional,
    ];

    fn code(self) -> &'static str {
        match self {
            Requirement::Required => "required",
            Requirement::Recommended => "recommended",
            Requirement::Optional => "optional",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.code() == code)
    }
}

/// An attribute a category of the channel has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryAttribute {
    pub id: String,
    pub name: String,
    pub requirement: Requirement,
}

/// An attribute of a draft and its value, empty while unset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftAttribute {
    pub id: String,
    pub name: String,
    pub requirement: Requirement,
    pub value: String,
}

/// A picture of a draft: a file of the Product's, and the channel's id for
/// it once uploaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftPicture {
    pub path: PathBuf,
    pub uploaded: Option<String>,
}

/// A Listing still in the app, or published with its description still to
/// send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingDraft {
    /// The Listing's id, which stays once it is published.
    pub id: RecordId,
    pub product: RecordId,
    /// The Product's SKU, sent as the seller SKU.
    pub seller_sku: String,
    pub title: String,
    pub description: String,
    pub category: Option<ChannelCategory>,
    pub listing_type: ListingType,
    pub condition: Condition,
    pub price: Money,
    pub available_quantity: u32,
    /// As "90 dias"; empty for none.
    pub warranty: String,
    pub attributes: Vec<DraftAttribute>,
    /// In order: the first is the cover.
    pub pictures: Vec<DraftPicture>,
    /// The channel's id once published.
    pub published: Option<String>,
    /// Published, but its description did not reach the channel yet.
    pub description_pending: bool,
    pub updated_at: Timestamp,
}

/// Something a draft still lacks before it goes out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    Title,
    Category,
    Price,
    /// An attribute the category has, without a value.
    Attribute {
        id: String,
        name: String,
    },
    /// Fewer than [`MIN_PICTURES`] pictures.
    Pictures {
        selected: usize,
    },
    Description,
    /// No units: the listing starts paused.
    Stock,
    /// What the channel's validator said, in its words.
    Channel(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecklistItem {
    pub check: Check,
    /// Publishing waits for it; otherwise it is a warning.
    pub blocks: bool,
}

/// What the channel's validator says of a listing about to go out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelIssue {
    pub message: String,
    pub blocks: bool,
}

/// Whether anything in `checklist` holds publishing back.
pub fn is_blocked(checklist: &[ChecklistItem]) -> bool {
    checklist.iter().any(|item| item.blocks)
}

impl ListingDraft {
    /// What the draft lacks, blocking items first, without asking the
    /// channel: title, category, price, the attributes the category
    /// requires and at least [`MIN_PICTURES`] pictures block; a missing
    /// recommended attribute, description or stock is a warning.
    pub fn checklist(&self) -> Vec<ChecklistItem> {
        let mut items = Vec::new();
        let mut add = |check, blocks| items.push(ChecklistItem { check, blocks });
        if self.title.trim().is_empty() {
            add(Check::Title, true);
        }
        if self.category.is_none() {
            add(Check::Category, true);
        }
        if self.price.amount() <= Decimal::ZERO {
            add(Check::Price, true);
        }
        for attribute in &self.attributes {
            if attribute.value.trim().is_empty() && attribute.requirement != Requirement::Optional {
                add(
                    Check::Attribute {
                        id: attribute.id.clone(),
                        name: attribute.name.clone(),
                    },
                    attribute.requirement == Requirement::Required,
                );
            }
        }
        if self.pictures.len() < MIN_PICTURES {
            add(
                Check::Pictures {
                    selected: self.pictures.len(),
                },
                true,
            );
        }
        if self.description.trim().is_empty() {
            add(Check::Description, false);
        }
        if self.available_quantity == 0 {
            add(Check::Stock, false);
        }
        items.sort_by_key(|item| !item.blocks);
        items
    }
}

/// What the owner changes in a draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftEdit {
    pub title: String,
    pub description: String,
    pub listing_type: ListingType,
    pub condition: Condition,
    pub price: Money,
    pub available_quantity: u32,
    pub warranty: String,
    /// New values by attribute id; attributes left out keep theirs.
    pub attributes: Vec<AttributeValue>,
    /// The pictures, in order; a file already uploaded keeps its id.
    pub pictures: Vec<PathBuf>,
}

/// Where a new draft starts besides the Product: the units on hand and the
/// Product's picture files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftStart {
    pub available_quantity: u32,
    pub pictures: Vec<PathBuf>,
}

/// A listing as it goes to the channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingToPublish {
    pub title: String,
    pub category: String,
    pub listing_type: ListingType,
    pub condition: Condition,
    /// In whole cents.
    pub price: Money,
    pub available_quantity: u32,
    pub seller_sku: String,
    pub warranty: Option<String>,
    /// Only the attributes with a value.
    pub attributes: Vec<AttributeValue>,
    /// The channel's ids of the uploaded pictures, cover first.
    pub pictures: Vec<String>,
}

/// A listing the channel has: one just published, or one found by its
/// seller SKU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedListing {
    pub id: String,
    pub status: ListingStatus,
    pub link: Option<String>,
}

/// The Sales Channel as the place listings are published. Calls block on
/// the network, so they run off the UI thread. Nothing calls `publish`
/// without the owner's click.
pub trait ListingPublisher: Send + Sync {
    /// The currency the channel sells in.
    fn currency(&self) -> Currency;

    /// The categories the channel predicts for `title`, likeliest first.
    fn predict_categories(&self, title: &str) -> Result<Vec<CategoryPrediction>, PlatformError>;

    /// The attributes of `category` the seller can fill.
    fn category_attributes(&self, category: &str) -> Result<Vec<CategoryAttribute>, PlatformError>;

    /// Uploads the picture file at `path`; returns the channel's id for it.
    fn upload_picture(&self, path: &Path) -> Result<String, PlatformError>;

    /// What the channel's validator says of `listing`; nothing is created.
    fn validate(&self, listing: &ListingToPublish) -> Result<Vec<ChannelIssue>, PlatformError>;

    /// Creates the listing in the channel.
    fn publish(&self, listing: &ListingToPublish) -> Result<PublishedListing, PlatformError>;

    /// Sets the description of the listing `id`.
    fn describe(&self, id: &str, description: &str) -> Result<(), PlatformError>;

    /// The owner's listings with `seller_sku`, in any status.
    fn find_by_seller_sku(&self, seller_sku: &str) -> Result<Vec<PublishedListing>, PlatformError>;
}

const DRAFT_COLUMNS: &str = "l.id, l.product_id, l.seller_sku, l.title, d.description,
     l.category_id, d.category_name, l.listing_type, d.item_condition, l.price, l.currency,
     l.available_quantity, d.warranty, l.ml_item_id, l.updated_at, d.description_pending
     FROM commerce_listings l
     JOIN commerce_listing_drafts d ON d.listing_id = l.id AND d.deleted_at IS NULL
     WHERE l.deleted_at IS NULL";

impl Listings {
    /// A draft of a Listing for `product`: titled with its name, in the
    /// category the channel predicts likeliest for it with the attribute
    /// values the channel read in the name, Classic, new, with `start`'s
    /// units and pictures, and no price yet. The channel is asked before
    /// anything is written.
    pub async fn create_draft(
        &self,
        product: &CatalogProduct,
        start: DraftStart,
        publisher: &dyn ListingPublisher,
    ) -> Result<ListingDraft, ListingError> {
        let prediction = publisher
            .predict_categories(&product.name)?
            .into_iter()
            .next();
        let attributes = match &prediction {
            Some(prediction) => draft_attributes(
                publisher.category_attributes(&prediction.category.id)?,
                &prediction.attributes,
            ),
            None => Vec::new(),
        };
        let pictures: Vec<DraftPicture> = start
            .pictures
            .into_iter()
            .filter(|path| is_picture(path))
            .map(|path| DraftPicture {
                path,
                uploaded: None,
            })
            .collect();
        let id = self.ids.next_id();
        let now = stored(self.clock.now());
        let category = prediction.map(|prediction| prediction.category);
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        transaction
            .execute(
                "INSERT INTO commerce_listings
                     (id, product_id, title, price, currency, available_quantity, status,
                      listing_type, category_id, seller_sku, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
                params![
                    id.to_string(),
                    product.id.to_string(),
                    product.name.clone(),
                    Decimal::ZERO.to_string(),
                    publisher.currency().code(),
                    start.available_quantity,
                    DRAFT_STATUS,
                    ListingType::default().code(),
                    category.as_ref().map(|category| category.id.clone()),
                    product.sku.clone(),
                    now.clone()
                ],
            )
            .await?;
        transaction
            .execute(
                "INSERT INTO commerce_listing_drafts
                     (id, listing_id, description, item_condition, warranty, category_name,
                      created_at, updated_at)
                 VALUES (?1, ?2, '', ?3, '', ?4, ?5, ?5)",
                params![
                    self.ids.next_id().to_string(),
                    id.to_string(),
                    Condition::default().code(),
                    category.as_ref().map(|category| category.name.clone()),
                    now.clone()
                ],
            )
            .await?;
        self.write_attributes(&transaction, id, &attributes, &now)
            .await?;
        self.write_pictures(&transaction, id, &pictures, &now)
            .await?;
        transaction.commit().await?;
        self.draft(id).await
    }

    /// The drafts not yet published, and the published ones whose
    /// description is still to send, by title.
    pub async fn drafts(&self) -> Result<Vec<ListingDraft>, ListingError> {
        self.read_drafts(
            "AND (l.ml_item_id IS NULL OR d.description_pending = 1)",
            (),
        )
        .await
    }

    pub async fn draft(&self, id: RecordId) -> Result<ListingDraft, ListingError> {
        self.read_drafts("AND l.id = ?1", params![id.to_string()])
            .await?
            .pop()
            .ok_or(ListingError::UnknownDraft(id))
    }

    /// The categories the channel predicts for `title`, likeliest first.
    pub fn predict_categories(
        &self,
        title: &str,
        publisher: &dyn ListingPublisher,
    ) -> Result<Vec<CategoryPrediction>, ListingError> {
        Ok(publisher.predict_categories(title)?)
    }

    /// Moves the draft to `category`, with that category's attributes: a
    /// value typed for an attribute the new category also has stays, and
    /// `guessed` fills the empty ones.
    pub async fn choose_category(
        &self,
        id: RecordId,
        category: ChannelCategory,
        guessed: &[AttributeValue],
        publisher: &dyn ListingPublisher,
    ) -> Result<ListingDraft, ListingError> {
        let draft = self.editable(id).await?;
        let mut kept: Vec<AttributeValue> = draft
            .attributes
            .iter()
            .filter(|attribute| !attribute.value.trim().is_empty())
            .map(|attribute| AttributeValue {
                id: attribute.id.clone(),
                value: attribute.value.clone(),
            })
            .collect();
        kept.extend(guessed.iter().cloned());
        let attributes = draft_attributes(publisher.category_attributes(&category.id)?, &kept);
        let now = stored(self.clock.now());
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        transaction
            .execute(
                "UPDATE commerce_listings SET category_id = ?1, updated_at = ?2 WHERE id = ?3",
                params![category.id.clone(), now.clone(), id.to_string()],
            )
            .await?;
        transaction
            .execute(
                "UPDATE commerce_listing_drafts SET category_name = ?1, updated_at = ?2
                 WHERE listing_id = ?3 AND deleted_at IS NULL",
                params![category.name.clone(), now.clone(), id.to_string()],
            )
            .await?;
        transaction
            .execute(
                "UPDATE commerce_draft_attributes SET deleted_at = ?1, updated_at = ?1
                 WHERE listing_id = ?2 AND deleted_at IS NULL",
                params![now.clone(), id.to_string()],
            )
            .await?;
        self.write_attributes(&transaction, id, &attributes, &now)
            .await?;
        transaction.commit().await?;
        self.draft(id).await
    }

    /// Saves what the owner changed in the draft. A picture already
    /// uploaded keeps the channel's id; only files of the channel's kinds
    /// are kept.
    pub async fn save_draft(
        &self,
        id: RecordId,
        edit: DraftEdit,
    ) -> Result<ListingDraft, ListingError> {
        let draft = self.editable(id).await?;
        if edit.price.currency() != draft.price.currency() || edit.price.amount() < Decimal::ZERO {
            return Err(ListingError::InvalidPrice);
        }
        let uploaded: HashMap<PathBuf, String> = draft
            .pictures
            .into_iter()
            .filter_map(|picture| Some((picture.path, picture.uploaded?)))
            .collect();
        let mut pictures: Vec<DraftPicture> = Vec::new();
        for path in edit.pictures.into_iter().filter(|path| is_picture(path)) {
            if !pictures.iter().any(|picture| picture.path == path) {
                pictures.push(DraftPicture {
                    uploaded: uploaded.get(&path).cloned(),
                    path,
                });
            }
        }
        let now = stored(self.clock.now());
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        transaction
            .execute(
                "UPDATE commerce_listings
                 SET title = ?1, listing_type = ?2, price = ?3, available_quantity = ?4,
                     updated_at = ?5
                 WHERE id = ?6",
                params![
                    edit.title.trim().to_owned(),
                    edit.listing_type.code(),
                    edit.price.amount().to_string(),
                    edit.available_quantity,
                    now.clone(),
                    id.to_string()
                ],
            )
            .await?;
        transaction
            .execute(
                "UPDATE commerce_listing_drafts
                 SET description = ?1, item_condition = ?2, warranty = ?3, updated_at = ?4
                 WHERE listing_id = ?5 AND deleted_at IS NULL",
                params![
                    edit.description.trim().to_owned(),
                    edit.condition.code(),
                    edit.warranty.trim().to_owned(),
                    now.clone(),
                    id.to_string()
                ],
            )
            .await?;
        for attribute in &edit.attributes {
            transaction
                .execute(
                    "UPDATE commerce_draft_attributes SET value = ?1, updated_at = ?2
                     WHERE listing_id = ?3 AND attribute_id = ?4 AND deleted_at IS NULL",
                    params![
                        attribute.value.trim().to_owned(),
                        now.clone(),
                        id.to_string(),
                        attribute.id.clone()
                    ],
                )
                .await?;
        }
        transaction
            .execute(
                "UPDATE commerce_draft_pictures SET deleted_at = ?1, updated_at = ?1
                 WHERE listing_id = ?2 AND deleted_at IS NULL",
                params![now.clone(), id.to_string()],
            )
            .await?;
        self.write_pictures(&transaction, id, &pictures, &now)
            .await?;
        transaction.commit().await?;
        self.draft(id).await
    }

    /// Throws a draft away. A published one is a Listing in the channel
    /// and stays.
    pub async fn discard_draft(&self, id: RecordId) -> Result<(), ListingError> {
        self.editable(id).await?;
        let now = stored(self.clock.now());
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        for (table, column) in [
            ("commerce_listings", "id"),
            ("commerce_listing_drafts", "listing_id"),
            ("commerce_draft_attributes", "listing_id"),
            ("commerce_draft_pictures", "listing_id"),
        ] {
            transaction
                .execute(
                    &format!(
                        "UPDATE {table} SET deleted_at = ?1, updated_at = ?1
                         WHERE {column} = ?2 AND deleted_at IS NULL"
                    ),
                    params![now.clone(), id.to_string()],
                )
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    /// The draft's checklist with what the channel's validator says of it.
    /// The pictures not yet uploaded go up first, since the validator reads
    /// them; nothing is published. Without a category there is nothing to
    /// validate yet.
    pub async fn validate_draft(
        &self,
        id: RecordId,
        publisher: &dyn ListingPublisher,
    ) -> Result<Vec<ChecklistItem>, ListingError> {
        let draft = self
            .upload_pictures(self.editable(id).await?, publisher)
            .await?;
        let mut checklist = draft.checklist();
        if let Some(listing) = to_publish(&draft) {
            checklist.extend(publisher.validate(&listing)?.into_iter().map(|issue| {
                ChecklistItem {
                    check: Check::Channel(issue.message),
                    blocks: issue.blocks,
                }
            }));
        }
        checklist.sort_by_key(|item| !item.blocks);
        Ok(checklist)
    }

    /// Publishes the draft on the owner's click: once nothing in its
    /// checklist blocks, its pictures go up, the channel's validator passes
    /// it, and the channel creates the listing; then its description goes.
    /// The Listing keeps the channel's id and links to the draft's Product.
    ///
    /// Publishing again never lists the Product twice: a draft already in
    /// the channel only sends what is left of it, and one whose earlier
    /// attempt ended without an answer is first looked for in the channel
    /// by its seller SKU.
    pub async fn publish(
        &self,
        id: RecordId,
        publisher: &dyn ListingPublisher,
    ) -> Result<Listing, ListingError> {
        let mut draft = self.draft(id).await?;
        if draft.published.is_none() {
            draft = self.create_in_channel(draft, publisher).await?;
        }
        let channel_id = draft.published.clone().expect("in the channel by now");
        if draft.description_pending {
            publisher
                .describe(&channel_id, &draft.description)
                .map_err(|error| ListingError::DescriptionNotSent(id, error))?;
        }
        self.database
            .connection()
            .execute(
                "UPDATE commerce_listing_drafts SET description_pending = 0, updated_at = ?1
                 WHERE listing_id = ?2 AND deleted_at IS NULL",
                params![stored(self.clock.now()), id.to_string()],
            )
            .await?;
        self.listing(id).await
    }

    async fn create_in_channel(
        &self,
        draft: ListingDraft,
        publisher: &dyn ListingPublisher,
    ) -> Result<ListingDraft, ListingError> {
        let id = draft.id;
        let checklist = draft.checklist();
        if is_blocked(&checklist) {
            return Err(ListingError::Blocked(checklist));
        }
        if self.publishing_since(id).await?.is_some()
            && let Some((found, synced)) = self.lost_publication(&draft, publisher).await?
        {
            self.keep_published(&draft, &found, synced).await?;
            return self.draft(id).await;
        }
        let draft = self.upload_pictures(draft, publisher).await?;
        let listing = to_publish(&draft).expect("the checklist asks for a category");
        let issues = publisher.validate(&listing)?;
        if issues.iter().any(|issue| issue.blocks) {
            let mut checklist = checklist;
            checklist.extend(issues.into_iter().map(|issue| ChecklistItem {
                check: Check::Channel(issue.message),
                blocks: issue.blocks,
            }));
            checklist.sort_by_key(|item| !item.blocks);
            return Err(ListingError::Blocked(checklist));
        }
        self.set_publishing_since(id, Some(self.clock.now()))
            .await?;
        match publisher.publish(&listing) {
            Ok(published) => {
                self.keep_published(&draft, &published, None).await?;
                self.draft(id).await
            }
            // No answer: the channel may have created it; the next attempt
            // looks for it first.
            Err(error @ PlatformError::Failed(_)) => Err(error.into()),
            Err(error) => {
                self.set_publishing_since(id, None).await?;
                Err(error.into())
            }
        }
    }

    /// The listing an earlier attempt created without the app hearing
    /// back: the one with the draft's seller SKU that no Listing in the app
    /// holds, or that a Sync brought in since without a Product. Also
    /// returns that synced Listing, which the draft replaces.
    async fn lost_publication(
        &self,
        draft: &ListingDraft,
        publisher: &dyn ListingPublisher,
    ) -> Result<Option<(PublishedListing, Option<RecordId>)>, ListingError> {
        for found in publisher.find_by_seller_sku(&draft.seller_sku)? {
            let mut rows = self
                .database
                .connection()
                .query(
                    "SELECT l.id, l.product_id, d.id FROM commerce_listings l
                     LEFT JOIN commerce_listing_drafts d
                         ON d.listing_id = l.id AND d.deleted_at IS NULL
                     WHERE l.ml_item_id = ?1 AND l.deleted_at IS NULL",
                    params![found.id.clone()],
                )
                .await?;
            let mut holders = Vec::new();
            while let Some(row) = rows.next().await? {
                holders.push((
                    row.id_at(0)?,
                    row.optional_id_at(1)?,
                    row.get::<Option<String>>(2)?,
                ));
            }
            match holders[..] {
                [] => return Ok(Some((found, None))),
                [(synced, None, None)] => return Ok(Some((found, Some(synced)))),
                _ => {}
            }
        }
        Ok(None)
    }

    /// Gives the draft's Listing the channel's id, in place of `synced`,
    /// the Listing a Sync made of the same listing, if any.
    async fn keep_published(
        &self,
        draft: &ListingDraft,
        published: &PublishedListing,
        synced: Option<RecordId>,
    ) -> Result<(), ListingError> {
        let now = stored(self.clock.now());
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        if let Some(synced) = synced {
            transaction
                .execute(
                    "UPDATE commerce_listings SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                    params![now.clone(), synced.to_string()],
                )
                .await?;
        }
        transaction
            .execute(
                "UPDATE commerce_listings
                 SET ml_item_id = ?1, status = ?2, link = ?3, price = ?4, synced_at = ?5,
                     updated_at = ?5
                 WHERE id = ?6",
                params![
                    published.id.clone(),
                    published.status.code(),
                    published.link.clone(),
                    draft.price.rounded().amount().to_string(),
                    now.clone(),
                    draft.id.to_string()
                ],
            )
            .await?;
        transaction
            .execute(
                "UPDATE commerce_listing_drafts
                 SET publishing_since = NULL, description_pending = ?1, updated_at = ?2
                 WHERE listing_id = ?3 AND deleted_at IS NULL",
                params![
                    i64::from(!draft.description.is_empty()),
                    now,
                    draft.id.to_string()
                ],
            )
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Uploads the draft's pictures the channel does not have yet, keeping
    /// each id as it comes, so a failure halfway loses none.
    async fn upload_pictures(
        &self,
        mut draft: ListingDraft,
        publisher: &dyn ListingPublisher,
    ) -> Result<ListingDraft, ListingError> {
        for picture in draft
            .pictures
            .iter_mut()
            .filter(|picture| picture.uploaded.is_none())
        {
            let uploaded = publisher.upload_picture(&picture.path)?;
            self.database
                .connection()
                .execute(
                    "UPDATE commerce_draft_pictures SET channel_picture_id = ?1, updated_at = ?2
                     WHERE listing_id = ?3 AND path = ?4 AND deleted_at IS NULL",
                    params![
                        uploaded.clone(),
                        stored(self.clock.now()),
                        draft.id.to_string(),
                        path_text(&picture.path)
                    ],
                )
                .await?;
            picture.uploaded = Some(uploaded);
        }
        Ok(draft)
    }

    /// The draft, if it can still change: not yet in the channel.
    async fn editable(&self, id: RecordId) -> Result<ListingDraft, ListingError> {
        let draft = self.draft(id).await?;
        match draft.published {
            Some(_) => Err(ListingError::Published(id)),
            None => Ok(draft),
        }
    }

    async fn publishing_since(&self, id: RecordId) -> Result<Option<Timestamp>, ListingError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT publishing_since FROM commerce_listing_drafts
                 WHERE listing_id = ?1 AND deleted_at IS NULL",
                params![id.to_string()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(row.optional_time_at(0)?),
            None => Ok(None),
        }
    }

    async fn set_publishing_since(
        &self,
        id: RecordId,
        since: Option<Timestamp>,
    ) -> Result<(), ListingError> {
        self.database
            .connection()
            .execute(
                "UPDATE commerce_listing_drafts SET publishing_since = ?1, updated_at = ?2
                 WHERE listing_id = ?3 AND deleted_at IS NULL",
                params![since.map(stored), stored(self.clock.now()), id.to_string()],
            )
            .await?;
        Ok(())
    }

    async fn write_attributes(
        &self,
        on: &Connection,
        id: RecordId,
        attributes: &[DraftAttribute],
        now: &str,
    ) -> Result<(), ListingError> {
        for (position, attribute) in attributes.iter().enumerate() {
            on.execute(
                "INSERT INTO commerce_draft_attributes
                     (id, listing_id, position, attribute_id, name, requirement, value,
                      created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    self.ids.next_id().to_string(),
                    id.to_string(),
                    position as i64,
                    attribute.id.clone(),
                    attribute.name.clone(),
                    attribute.requirement.code(),
                    attribute.value.clone(),
                    now.to_owned()
                ],
            )
            .await?;
        }
        Ok(())
    }

    async fn write_pictures(
        &self,
        on: &Connection,
        id: RecordId,
        pictures: &[DraftPicture],
        now: &str,
    ) -> Result<(), ListingError> {
        for (position, picture) in pictures.iter().enumerate() {
            on.execute(
                "INSERT INTO commerce_draft_pictures
                     (id, listing_id, position, path, channel_picture_id, created_at,
                      updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![
                    self.ids.next_id().to_string(),
                    id.to_string(),
                    position as i64,
                    path_text(&picture.path),
                    picture.uploaded.clone(),
                    now.to_owned()
                ],
            )
            .await?;
        }
        Ok(())
    }

    async fn read_drafts(
        &self,
        filter: &str,
        params: impl libsql::params::IntoParams,
    ) -> Result<Vec<ListingDraft>, ListingError> {
        let connection = self.database.connection();
        let mut rows = connection
            .query(
                &format!("SELECT {DRAFT_COLUMNS} {filter} ORDER BY l.title COLLATE NOCASE, l.id"),
                params,
            )
            .await?;
        let mut drafts = Vec::new();
        while let Some(row) = rows.next().await? {
            let listing_type: String = row.get(7)?;
            let condition: String = row.get(8)?;
            let category = match (row.get::<Option<String>>(5)?, row.get::<Option<String>>(6)?) {
                (Some(id), name) => Some(ChannelCategory {
                    name: name.unwrap_or_else(|| id.clone()),
                    id,
                }),
                (None, _) => None,
            };
            drafts.push(ListingDraft {
                id: row.id_at(0)?,
                product: row
                    .optional_id_at(1)?
                    .ok_or_else(|| ListingError::Unreadable("a draft without a Product".into()))?,
                seller_sku: row.get::<Option<String>>(2)?.unwrap_or_default(),
                title: row.get(3)?,
                description: row.get(4)?,
                category,
                listing_type: ListingType::from_code(&listing_type)
                    .ok_or(ListingError::Unreadable(listing_type))?,
                condition: Condition::from_code(&condition)
                    .ok_or(ListingError::Unreadable(condition))?,
                price: Money::new(row.decimal_at(9)?, row.currency_at(10)?),
                available_quantity: row.get(11)?,
                warranty: row.get(12)?,
                published: row.get(13)?,
                updated_at: row.time_at(14)?,
                description_pending: row.get::<i64>(15)? != 0,
                attributes: Vec::new(),
                pictures: Vec::new(),
            });
        }
        for draft in &mut drafts {
            draft.attributes = attributes_of(connection, draft.id).await?;
            draft.pictures = pictures_of(connection, draft.id).await?;
        }
        Ok(drafts)
    }
}

/// The attributes a draft in a category shows: the ones the category
/// requires or recommends, and any other with a value in `values`, which
/// also fill them. The channel's order is kept.
fn draft_attributes(
    category: Vec<CategoryAttribute>,
    values: &[AttributeValue],
) -> Vec<DraftAttribute> {
    category
        .into_iter()
        .filter_map(|attribute| {
            let value = values
                .iter()
                .find(|value| value.id == attribute.id && !value.value.trim().is_empty())
                .map(|value| value.value.trim().to_owned())
                .unwrap_or_default();
            (attribute.requirement != Requirement::Optional || !value.is_empty()).then(|| {
                DraftAttribute {
                    id: attribute.id,
                    name: attribute.name,
                    requirement: attribute.requirement,
                    value,
                }
            })
        })
        .collect()
}

/// The draft as it goes to the channel; `None` without a category.
fn to_publish(draft: &ListingDraft) -> Option<ListingToPublish> {
    Some(ListingToPublish {
        title: draft.title.clone(),
        category: draft.category.as_ref()?.id.clone(),
        listing_type: draft.listing_type,
        condition: draft.condition,
        price: draft.price.rounded(),
        available_quantity: draft.available_quantity,
        seller_sku: draft.seller_sku.clone(),
        warranty: Some(draft.warranty.clone()).filter(|warranty| !warranty.is_empty()),
        attributes: draft
            .attributes
            .iter()
            .filter(|attribute| !attribute.value.is_empty())
            .map(|attribute| AttributeValue {
                id: attribute.id.clone(),
                value: attribute.value.clone(),
            })
            .collect(),
        pictures: draft
            .pictures
            .iter()
            .filter_map(|picture| picture.uploaded.clone())
            .collect(),
    })
}

async fn attributes_of(on: &Connection, id: RecordId) -> Result<Vec<DraftAttribute>, ListingError> {
    let mut rows = on
        .query(
            "SELECT attribute_id, name, requirement, value FROM commerce_draft_attributes
             WHERE listing_id = ?1 AND deleted_at IS NULL ORDER BY position",
            params![id.to_string()],
        )
        .await?;
    let mut attributes = Vec::new();
    while let Some(row) = rows.next().await? {
        let requirement: String = row.get(2)?;
        attributes.push(DraftAttribute {
            id: row.get(0)?,
            name: row.get(1)?,
            requirement: Requirement::from_code(&requirement)
                .ok_or(ListingError::Unreadable(requirement))?,
            value: row.get(3)?,
        });
    }
    Ok(attributes)
}

async fn pictures_of(on: &Connection, id: RecordId) -> Result<Vec<DraftPicture>, ListingError> {
    let mut rows = on
        .query(
            "SELECT path, channel_picture_id FROM commerce_draft_pictures
             WHERE listing_id = ?1 AND deleted_at IS NULL ORDER BY position",
            params![id.to_string()],
        )
        .await?;
    let mut pictures = Vec::new();
    while let Some(row) = rows.next().await? {
        pictures.push(DraftPicture {
            path: PathBuf::from(row.get::<String>(0)?),
            uploaded: row.get(1)?,
        });
    }
    Ok(pictures)
}

/// A picture's path as stored. The Product's folder is the app's own, so
/// its paths are valid text on every system it runs on.
fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
