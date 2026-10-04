//! Suppliers, their Supplier Offers and the Products the offers become.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use libsql::{Connection, Row, Value, params};
use mascate_kernel::{
    Clock, CurrencyMismatch, IdGenerator, Money, PlatformError, Record, RecordId, Timestamp,
};
use mascate_platform::{Database, Migration, StoredRow, StoredValueError, stored};

use crate::{InvalidLink, InvalidSku, OfferLink, Sku};

/// Where Supplier Offers come from. Only the owner's own typing for now;
/// Platform APIs join as their adapters land.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProductSource {
    Manual,
}

impl ProductSource {
    fn code(self) -> &'static str {
        match self {
            ProductSource::Manual => "manual",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        match code {
            "manual" => Some(ProductSource::Manual),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supplier {
    pub id: RecordId,
    pub name: String,
}

/// What the owner types to register a Supplier Offer by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSupplierOffer {
    pub supplier: RecordId,
    pub link: String,
    pub title: String,
    pub price: Money,
    /// What the Supplier charges to deliver one unit to the owner.
    pub shipping: Money,
}

/// A Supplier's price and shipping for an item at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupplierOffer {
    pub id: RecordId,
    pub supplier: Supplier,
    /// The Product the offer's link belongs to, once the owner chose one.
    pub product: Option<RecordId>,
    pub source: ProductSource,
    pub link: String,
    pub title: String,
    pub price: Money,
    pub shipping: Money,
    pub observed_at: Timestamp,
}

impl SupplierOffer {
    /// What one unit costs delivered: price plus shipping.
    pub fn total(&self) -> Money {
        self.price
            .checked_add(self.shipping)
            .expect("an offer keeps price and shipping in one currency")
    }
}

/// Every Supplier Offer registered for one link, newest first: its price
/// history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferHistory {
    pub latest: SupplierOffer,
    pub earlier: Vec<SupplierOffer>,
}

impl OfferHistory {
    /// The offers, newest first.
    pub fn offers(&self) -> impl Iterator<Item = &SupplierOffer> {
        std::iter::once(&self.latest).chain(&self.earlier)
    }

    /// The cheapest delivered price ever seen at this link. Every offer at
    /// one link shares the latest one's currency only by convention, so
    /// offers in another currency are left out.
    pub fn lowest_total(&self) -> Money {
        let currency = self.latest.price.currency();
        self.offers()
            .map(SupplierOffer::total)
            .filter(|total| total.currency() == currency)
            .min_by_key(|total| total.amount())
            .unwrap_or_else(|| self.latest.total())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Product {
    pub id: RecordId,
    pub sku: Sku,
    pub name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("the supplier needs a name")]
    MissingSupplierName,
    #[error("the Supplier Offer needs a title")]
    MissingTitle,
    #[error("the Product needs a name")]
    MissingProductName,
    #[error("a supplier named {0} already exists")]
    SupplierExists(String),
    #[error("no supplier {0}")]
    UnknownSupplier(RecordId),
    #[error("no Supplier Offer {0}")]
    UnknownOffer(RecordId),
    #[error("no Product {0}")]
    UnknownProduct(RecordId),
    #[error("no Opportunity {0} to discard")]
    UnknownOpportunity(RecordId),
    #[error("say why the Opportunity is discarded")]
    MissingReason,
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error(transparent)]
    InvalidLink(#[from] InvalidLink),
    #[error(transparent)]
    InvalidSku(#[from] InvalidSku),
    #[error("another Product already has the SKU {0}")]
    SkuTaken(Sku),
    #[error("{} already exists and belongs to no Product; move it away first", .0.display())]
    FolderTaken(PathBuf),
    #[error("a price cannot be negative")]
    Negative,
    #[error("price and shipping must be in one currency: {0}")]
    Currencies(#[from] CurrencyMismatch),
    #[error("could not use the folder {}: {source}", path.display())]
    Folder {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("the catalog holds a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

pub(crate) const CREATE_CATALOG: Migration = Migration {
    version: 1,
    name: "create suppliers, supplier offers and products",
    risky: false,
    sql: "CREATE TABLE catalog_suppliers (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE UNIQUE INDEX catalog_suppliers_by_name
        ON catalog_suppliers (name COLLATE NOCASE) WHERE deleted_at IS NULL;
    CREATE TABLE catalog_products (
        id         TEXT PRIMARY KEY,
        sku        TEXT NOT NULL,
        name       TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE UNIQUE INDEX catalog_products_by_sku
        ON catalog_products (sku) WHERE deleted_at IS NULL;
    CREATE TABLE catalog_supplier_offers (
        id          TEXT PRIMARY KEY,
        supplier_id TEXT NOT NULL REFERENCES catalog_suppliers (id),
        product_id  TEXT REFERENCES catalog_products (id),
        source      TEXT NOT NULL,
        link        TEXT NOT NULL,
        link_key    TEXT NOT NULL,
        title       TEXT NOT NULL,
        price       TEXT NOT NULL,
        shipping    TEXT NOT NULL,
        currency    TEXT NOT NULL,
        observed_at TEXT NOT NULL,
        created_at  TEXT NOT NULL,
        updated_at  TEXT NOT NULL,
        deleted_at  TEXT
    );
    CREATE INDEX catalog_supplier_offers_by_link
        ON catalog_supplier_offers (link_key, observed_at);
    CREATE INDEX catalog_supplier_offers_by_product
        ON catalog_supplier_offers (product_id);",
};

/// The columns [`offer_from`] reads, then the link's key.
pub(crate) const OFFER_COLUMNS: &str =
    "o.id, o.product_id, o.source, o.link, o.title, o.price, o.shipping,
     o.currency, o.observed_at, s.id, s.name, o.link_key
     FROM catalog_supplier_offers o JOIN catalog_suppliers s ON s.id = o.supplier_id
     WHERE o.deleted_at IS NULL";

/// The catalog: Suppliers, Supplier Offers, Products and each Product's
/// folder of files under `files_root`, one folder per SKU.
pub struct Catalog {
    database: Arc<Database>,
    files_root: PathBuf,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl Catalog {
    pub fn new(
        database: Arc<Database>,
        files_root: PathBuf,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            database,
            files_root,
            clock,
            ids,
        }
    }

    pub(crate) fn connection(&self) -> &Connection {
        self.database.connection()
    }

    pub(crate) fn database(&self) -> &Database {
        &self.database
    }

    pub(crate) fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    pub(crate) fn ids(&self) -> &dyn IdGenerator {
        self.ids.as_ref()
    }

    pub(crate) fn next_id(&self) -> RecordId {
        self.ids.next_id()
    }

    pub(crate) fn new_record(&self) -> Record {
        Record::new(self.ids.as_ref(), self.clock.as_ref())
    }

    pub(crate) fn now(&self) -> String {
        stored(self.clock.now())
    }

    /// Registers a Supplier. Names are unique, ignoring case.
    pub async fn add_supplier(&self, name: &str) -> Result<Supplier, CatalogError> {
        let name = required(name, CatalogError::MissingSupplierName)?;
        let mut taken = self
            .connection()
            .query(
                "SELECT 1 FROM catalog_suppliers
                 WHERE name = ?1 COLLATE NOCASE AND deleted_at IS NULL",
                params![name.clone()],
            )
            .await?;
        if taken.next().await?.is_some() {
            return Err(CatalogError::SupplierExists(name));
        }
        let record = self.new_record();
        let at = stored(record.created_at);
        self.connection()
            .execute(
                "INSERT INTO catalog_suppliers (id, name, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)",
                params![record.id.to_string(), name.clone(), at],
            )
            .await?;
        Ok(Supplier {
            id: record.id,
            name,
        })
    }

    /// Every Supplier, by name.
    pub async fn suppliers(&self) -> Result<Vec<Supplier>, CatalogError> {
        let mut rows = self
            .connection()
            .query(
                "SELECT id, name FROM catalog_suppliers WHERE deleted_at IS NULL
                 ORDER BY name COLLATE NOCASE",
                (),
            )
            .await?;
        let mut suppliers = Vec::new();
        while let Some(row) = rows.next().await? {
            suppliers.push(Supplier {
                id: row.id_at(0)?,
                name: row.get(1)?,
            });
        }
        Ok(suppliers)
    }

    /// Registers a Supplier Offer typed by the owner, priced now. A link
    /// seen before adds to that link's price history and joins its Product.
    pub async fn register_offer(
        &self,
        offer: NewSupplierOffer,
    ) -> Result<SupplierOffer, CatalogError> {
        let link = OfferLink::parse(&offer.link)?;
        let title = required(&offer.title, CatalogError::MissingTitle)?;
        offer.price.checked_add(offer.shipping)?;
        if offer.price.is_negative() || offer.shipping.is_negative() {
            return Err(CatalogError::Negative);
        }
        self.supplier(offer.supplier).await?;
        let record = self.new_record();
        let at = stored(record.created_at);
        self.connection()
            .execute(
                "INSERT INTO catalog_supplier_offers
                     (id, supplier_id, product_id, source, link, link_key, title, price,
                      shipping, currency, observed_at, created_at, updated_at)
                 SELECT ?1, ?2,
                        (SELECT product_id FROM catalog_supplier_offers
                         WHERE link_key = ?5 AND product_id IS NOT NULL AND deleted_at IS NULL
                         ORDER BY observed_at DESC, id DESC LIMIT 1),
                        ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?10",
                params![
                    record.id.to_string(),
                    offer.supplier.to_string(),
                    ProductSource::Manual.code(),
                    link.as_str(),
                    link.key(),
                    title,
                    offer.price.amount().to_string(),
                    offer.shipping.amount().to_string(),
                    offer.price.currency().code(),
                    at
                ],
            )
            .await?;
        self.offer(record.id).await
    }

    pub async fn offer(&self, id: RecordId) -> Result<SupplierOffer, CatalogError> {
        let mut rows = self
            .connection()
            .query(
                &format!("SELECT {OFFER_COLUMNS} AND o.id = ?1"),
                params![id.to_string()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => offer_from(&row),
            None => Err(CatalogError::UnknownOffer(id)),
        }
    }

    /// Every link's price history, the most recently priced first.
    pub async fn offers(&self) -> Result<Vec<OfferHistory>, CatalogError> {
        self.histories(format!("SELECT {OFFER_COLUMNS}"), Vec::new())
            .await
    }

    /// The price history of every link that belongs to `product`, the most
    /// recently priced first.
    pub async fn product_offers(
        &self,
        product: RecordId,
    ) -> Result<Vec<OfferHistory>, CatalogError> {
        self.histories(
            format!("SELECT {OFFER_COLUMNS} AND o.product_id = ?1"),
            vec![Value::Text(product.to_string())],
        )
        .await
    }

    async fn histories(
        &self,
        query: String,
        values: Vec<Value>,
    ) -> Result<Vec<OfferHistory>, CatalogError> {
        let mut rows = self
            .connection()
            .query(
                &format!("{query} ORDER BY o.observed_at DESC, o.id DESC"),
                values,
            )
            .await?;
        let mut histories: Vec<(String, OfferHistory)> = Vec::new();
        while let Some(row) = rows.next().await? {
            let offer = offer_from(&row)?;
            let key: String = row.get(11)?;
            match histories.iter_mut().find(|(known, _)| *known == key) {
                Some((_, history)) => history.earlier.push(offer),
                None => histories.push((
                    key,
                    OfferHistory {
                        latest: offer,
                        earlier: Vec::new(),
                    },
                )),
            }
        }
        Ok(histories.into_iter().map(|(_, history)| history).collect())
    }

    /// A free SKU for a Product named `name`, numbered after its stem.
    /// A number whose folder already exists is skipped, so a new Product
    /// never inherits files that are not its own.
    pub async fn suggest_sku(&self, name: &str) -> Result<Sku, CatalogError> {
        let stem = Sku::stem_for(name);
        let mut rows = self
            .connection()
            .query(
                "SELECT sku FROM catalog_products WHERE deleted_at IS NULL AND sku LIKE ?1",
                params![format!("{stem}-%")],
            )
            .await?;
        let mut taken = Vec::new();
        while let Some(row) = rows.next().await? {
            taken.push(row.get::<String>(0)?);
        }
        let sku = (1..)
            .map(|number| Sku::numbered(&stem, number))
            .find(|sku| {
                !taken.iter().any(|known| known == sku.as_str()) && !self.folder_of(sku).exists()
            })
            .expect("some number is free");
        Ok(sku)
    }

    /// Makes a Product from a Supplier Offer, with its folder. Every offer
    /// at the same link joins the new Product.
    pub async fn create_product(
        &self,
        from_offer: RecordId,
        name: &str,
        sku: &str,
    ) -> Result<Product, CatalogError> {
        self.offer(from_offer).await?;
        self.insert_product(name, sku, Some(from_offer)).await
    }

    /// Makes a Product that no Supplier Offer leads to yet, such as one
    /// already sold in a Sales Channel, with its folder.
    pub async fn add_product(&self, name: &str, sku: &str) -> Result<Product, CatalogError> {
        self.insert_product(name, sku, None).await
    }

    async fn insert_product(
        &self,
        name: &str,
        sku: &str,
        from_offer: Option<RecordId>,
    ) -> Result<Product, CatalogError> {
        let name = required(name, CatalogError::MissingProductName)?;
        let sku = Sku::parse(sku)?;
        if self
            .product_with_sku(self.connection(), &sku)
            .await?
            .is_some()
        {
            return Err(CatalogError::SkuTaken(sku));
        }
        // A folder already named after the SKU is adopted with what it holds.
        let folder = self.folder_of(&sku);
        let made_folder = !folder.exists();
        create_folder(&folder)?;
        let record = self.new_record();
        let created = async {
            let connection = self.database.connect_for_transaction().await?;
            let transaction = connection.transaction().await?;
            if self.product_with_sku(&transaction, &sku).await?.is_some() {
                return Err(CatalogError::SkuTaken(sku.clone()));
            }
            let at = stored(record.created_at);
            transaction
                .execute(
                    "INSERT INTO catalog_products (id, sku, name, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                    params![record.id.to_string(), sku.as_str(), name.clone(), at],
                )
                .await?;
            if let Some(offer) = from_offer {
                link_offers(&transaction, offer, record.id, &self.now()).await?;
            }
            transaction.commit().await?;
            Ok(())
        };
        if let Err(error) = created.await {
            if made_folder {
                // Removes only an empty folder: nothing was copied into it yet.
                let _ = std::fs::remove_dir(&folder);
            }
            return Err(error);
        }
        Ok(Product {
            id: record.id,
            sku,
            name,
        })
    }

    /// Moves a Supplier Offer's link, with its whole price history, to
    /// `product`.
    pub async fn link_offer(&self, offer: RecordId, product: RecordId) -> Result<(), CatalogError> {
        self.offer(offer).await?;
        self.product(product).await?;
        link_offers(self.connection(), offer, product, &self.now()).await
    }

    /// Every Product, by SKU.
    pub async fn products(&self) -> Result<Vec<Product>, CatalogError> {
        let mut rows = self
            .connection()
            .query(
                "SELECT id, sku, name FROM catalog_products WHERE deleted_at IS NULL ORDER BY sku",
                (),
            )
            .await?;
        let mut products = Vec::new();
        while let Some(row) = rows.next().await? {
            products.push(product_from(&row)?);
        }
        Ok(products)
    }

    pub async fn product(&self, id: RecordId) -> Result<Product, CatalogError> {
        let mut rows = self
            .connection()
            .query(
                "SELECT id, sku, name FROM catalog_products WHERE id = ?1 AND deleted_at IS NULL",
                params![id.to_string()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => product_from(&row),
            None => Err(CatalogError::UnknownProduct(id)),
        }
    }

    /// Gives a Product a new SKU and renames its folder to match, so its
    /// files stay with it. Nothing changes when the folder cannot move.
    pub async fn rename_sku(&self, product: RecordId, sku: &str) -> Result<Product, CatalogError> {
        let current = self.product(product).await?;
        let sku = Sku::parse(sku)?;
        if sku == current.sku {
            return Ok(current);
        }
        if self
            .product_with_sku(self.connection(), &sku)
            .await?
            .is_some()
        {
            return Err(CatalogError::SkuTaken(sku));
        }
        let (from, to) = (self.folder_of(&current.sku), self.folder_of(&sku));
        if to.exists() {
            return Err(CatalogError::FolderTaken(to));
        }
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection.transaction().await?;
        if self.product_with_sku(&transaction, &sku).await?.is_some() {
            return Err(CatalogError::SkuTaken(sku));
        }
        transaction
            .execute(
                "UPDATE catalog_products SET sku = ?1, updated_at = ?2 WHERE id = ?3",
                params![sku.as_str(), self.now(), product.to_string()],
            )
            .await?;
        let moved = from.exists();
        if moved {
            std::fs::rename(&from, &to).map_err(|source| CatalogError::Folder {
                path: from.clone(),
                source,
            })?;
        } else {
            create_folder(&to)?;
        }
        if let Err(error) = transaction.commit().await {
            if moved {
                let _ = std::fs::rename(&to, &from);
            }
            return Err(error.into());
        }
        Ok(Product { sku, ..current })
    }

    /// The Product's folder, where its files live.
    pub fn folder(&self, product: &Product) -> PathBuf {
        self.folder_of(&product.sku)
    }

    pub(crate) fn folder_of(&self, sku: &Sku) -> PathBuf {
        self.files_root.join(sku.as_str())
    }

    async fn supplier(&self, id: RecordId) -> Result<Supplier, CatalogError> {
        let mut rows = self
            .connection()
            .query(
                "SELECT id, name FROM catalog_suppliers WHERE id = ?1 AND deleted_at IS NULL",
                params![id.to_string()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Supplier {
                id: row.id_at(0)?,
                name: row.get(1)?,
            }),
            None => Err(CatalogError::UnknownSupplier(id)),
        }
    }

    async fn product_with_sku(
        &self,
        connection: &Connection,
        sku: &Sku,
    ) -> Result<Option<RecordId>, CatalogError> {
        let mut rows = connection
            .query(
                "SELECT id FROM catalog_products WHERE sku = ?1 AND deleted_at IS NULL",
                params![sku.as_str()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some(row.id_at(0)?)),
            None => Ok(None),
        }
    }
}

/// Points every offer at `offer`'s link to `product`.
async fn link_offers(
    connection: &Connection,
    offer: RecordId,
    product: RecordId,
    now: &str,
) -> Result<(), CatalogError> {
    connection
        .execute(
            "UPDATE catalog_supplier_offers SET product_id = ?1, updated_at = ?2
             WHERE deleted_at IS NULL
               AND link_key = (SELECT link_key FROM catalog_supplier_offers WHERE id = ?3)",
            params![product.to_string(), now, offer.to_string()],
        )
        .await?;
    Ok(())
}

pub(crate) fn create_folder(path: &Path) -> Result<(), CatalogError> {
    std::fs::create_dir_all(path).map_err(|source| CatalogError::Folder {
        path: path.to_path_buf(),
        source,
    })
}

/// `text` trimmed with its runs of spaces made one; `missing` when nothing
/// is left.
pub(crate) fn required(text: &str, missing: CatalogError) -> Result<String, CatalogError> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        Err(missing)
    } else {
        Ok(text)
    }
}

impl From<StoredValueError> for CatalogError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => CatalogError::Unreadable(text),
            StoredValueError::Sql(error) => CatalogError::Sql(error),
        }
    }
}

fn product_from(row: &Row) -> Result<Product, CatalogError> {
    let sku: String = row.get(1)?;
    Ok(Product {
        id: row.id_at(0)?,
        sku: Sku::parse(&sku).map_err(|_| CatalogError::Unreadable(sku))?,
        name: row.get(2)?,
    })
}

pub(crate) fn offer_from(row: &Row) -> Result<SupplierOffer, CatalogError> {
    let source: String = row.get(2)?;
    let currency = row.currency_at(7)?;
    Ok(SupplierOffer {
        id: row.id_at(0)?,
        product: row.optional_id_at(1)?,
        source: ProductSource::from_code(&source).ok_or(CatalogError::Unreadable(source))?,
        link: row.get(3)?,
        title: row.get(4)?,
        price: Money::new(row.decimal_at(5)?, currency),
        shipping: Money::new(row.decimal_at(6)?, currency),
        observed_at: row.time_at(8)?,
        supplier: Supplier {
            id: row.id_at(9)?,
            name: row.get(10)?,
        },
    })
}
