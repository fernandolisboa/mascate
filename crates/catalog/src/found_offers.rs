//! Offers found by searching a Platform's catalog (#12): the search shows
//! which ones the owner already keeps, and keeping them makes Supplier
//! Offers of their shops. Each item and shop is known by the Platform's own
//! id, so keeping the same offers again changes nothing and a new price
//! joins the item's price history (ADR 0027).

use std::collections::HashSet;

use libsql::{Connection, params};
use mascate_kernel::{Money, RecordId};
use mascate_platform::{Migration, StoredRow, stored};

use crate::catalog::{OFFER_COLUMNS, offer_from, required};
use crate::{
    Catalog, CatalogError, Found, FoundOffer, FoundShop, OfferLink, OfferSearch, OfferSource,
    ProductSource, SupplierOffer,
};

pub(crate) const ADD_SOURCE_IDS: Migration = Migration {
    version: 3,
    name: "add the Platform's ids to suppliers and supplier offers",
    risky: false,
    sql: "ALTER TABLE catalog_suppliers ADD COLUMN source TEXT;
    ALTER TABLE catalog_suppliers ADD COLUMN source_id TEXT;
    CREATE UNIQUE INDEX catalog_suppliers_by_source_id
        ON catalog_suppliers (source, source_id)
        WHERE source_id IS NOT NULL AND deleted_at IS NULL;
    ALTER TABLE catalog_supplier_offers ADD COLUMN source_id TEXT;
    CREATE INDEX catalog_supplier_offers_by_source_id
        ON catalog_supplier_offers (source, source_id);",
};

/// A found offer, with the latest Supplier Offer the owner keeps for the
/// same item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchedOffer {
    pub found: FoundOffer,
    pub kept: Option<SupplierOffer>,
}

impl SearchedOffer {
    /// Whether keeping it again would change nothing: it is kept with the
    /// title, link and price found now.
    pub fn up_to_date(&self) -> bool {
        self.kept
            .as_ref()
            .is_some_and(|kept| keeps(kept, &self.found))
    }
}

/// Whether `kept` already says what `found` says.
fn keeps(kept: &SupplierOffer, found: &FoundOffer) -> bool {
    let title = required(&found.title, CatalogError::MissingTitle).unwrap_or_default();
    let link = OfferLink::parse(&found.link).map(|link| link.as_str().to_owned());
    kept.title == title && link.is_ok_and(|link| kept.link == link) && kept.price == found.price
}

/// What keeping found offers did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeptOffers {
    /// New Supplier Offers: items not kept before, or kept at another price.
    pub added: usize,
    /// Items already kept as they are now.
    pub unchanged: usize,
    /// Items without a title, a valid link or a price that is not negative.
    pub skipped: usize,
}

impl Catalog {
    /// Searches `source` for offers, each with what the owner already keeps
    /// of it.
    pub async fn search_offers(
        &self,
        source: &dyn OfferSource,
        search: &OfferSearch,
    ) -> Result<Found<SearchedOffer>, CatalogError> {
        let found = source.search_offers(search)?;
        Ok(Found {
            items: self.with_kept(source.source(), found.items).await?,
            more: found.more,
        })
    }

    /// `offers`, found in `source`, each with the latest Supplier Offer kept
    /// for it, as after keeping some of them.
    pub async fn with_kept(
        &self,
        source: ProductSource,
        offers: Vec<FoundOffer>,
    ) -> Result<Vec<SearchedOffer>, CatalogError> {
        let mut searched = Vec::with_capacity(offers.len());
        for found in offers {
            let kept = latest_kept(self.connection(), source, &found.id)
                .await?
                .map(|kept| kept.offer);
            searched.push(SearchedOffer { found, kept });
        }
        Ok(searched)
    }

    /// Keeps `offers`, found in `source`, as Supplier Offers priced now, each
    /// shop as a Supplier. The Platform does not say what delivery costs,
    /// so shipping starts at zero.
    pub async fn keep_found_offers(
        &self,
        source: ProductSource,
        offers: &[FoundOffer],
    ) -> Result<KeptOffers, CatalogError> {
        let connection = self.database().connect_for_transaction().await?;
        let transaction = connection.transaction().await?;
        let mut report = KeptOffers::default();
        let mut seen = HashSet::new();
        for offer in offers {
            if !seen.insert(offer.id.as_str()) {
                report.unchanged += 1;
                continue;
            }
            let (Ok(link), Ok(title)) = (
                OfferLink::parse(&offer.link),
                required(&offer.title, CatalogError::MissingTitle),
            ) else {
                report.skipped += 1;
                continue;
            };
            if offer.price.is_negative() || offer.id.trim().is_empty() {
                report.skipped += 1;
                continue;
            }
            let supplier = self
                .source_supplier(&transaction, source, &offer.shop)
                .await?;
            let previous = latest_kept(&transaction, source, &offer.id).await?;
            if previous
                .as_ref()
                .is_some_and(|kept| keeps(&kept.offer, offer))
            {
                report.unchanged += 1;
                continue;
            }
            let shipping = Money::zero(offer.price.currency());
            let key = previous
                .map(|previous| previous.key)
                .unwrap_or_else(|| link.key().to_owned());
            let record = self.new_record();
            transaction
                .execute(
                    "INSERT INTO catalog_supplier_offers
                         (id, supplier_id, product_id, source, source_id, link, link_key, title,
                          price, shipping, currency, observed_at, created_at, updated_at)
                     SELECT ?1, ?2,
                            (SELECT product_id FROM catalog_supplier_offers
                             WHERE link_key = ?6 AND product_id IS NOT NULL AND deleted_at IS NULL
                             ORDER BY observed_at DESC, id DESC LIMIT 1),
                            ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?11",
                    params![
                        record.id.to_string(),
                        supplier.to_string(),
                        source.code(),
                        offer.id.clone(),
                        link.as_str(),
                        key,
                        title,
                        offer.price.amount().to_string(),
                        shipping.amount().to_string(),
                        offer.price.currency().code(),
                        stored(record.created_at)
                    ],
                )
                .await?;
            report.added += 1;
        }
        transaction.commit().await?;
        Ok(report)
    }

    /// The Supplier that stands for `shop`: the one kept for its id, else a
    /// Supplier the owner registered by hand under the same name (the same
    /// shop, now with its id), else a new one. A shop renamed on the
    /// Platform renames its Supplier when the new name is free.
    async fn source_supplier(
        &self,
        connection: &Connection,
        source: ProductSource,
        shop: &FoundShop,
    ) -> Result<RecordId, CatalogError> {
        let name = required(&shop.name, CatalogError::MissingSupplierName)
            .unwrap_or_else(|_| format!("Loja {}", shop.id.trim()));
        let now = self.now();
        let mut rows = connection
            .query(
                "SELECT id, name FROM catalog_suppliers
                 WHERE source = ?1 AND source_id = ?2 AND deleted_at IS NULL",
                params![source.code(), shop.id.clone()],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            let id: RecordId = row.id_at(0)?;
            let known: String = row.get(1)?;
            if known != name && !name_taken(connection, &name, Some(id)).await? {
                connection
                    .execute(
                        "UPDATE catalog_suppliers SET name = ?1, updated_at = ?2 WHERE id = ?3",
                        params![name, now, id.to_string()],
                    )
                    .await?;
            }
            return Ok(id);
        }
        let mut rows = connection
            .query(
                "SELECT id FROM catalog_suppliers
                 WHERE name = ?1 COLLATE NOCASE AND source_id IS NULL AND deleted_at IS NULL",
                params![name.clone()],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            let id: RecordId = row.id_at(0)?;
            connection
                .execute(
                    "UPDATE catalog_suppliers SET source = ?1, source_id = ?2, updated_at = ?3
                     WHERE id = ?4",
                    params![source.code(), shop.id.clone(), now, id.to_string()],
                )
                .await?;
            return Ok(id);
        }
        // Another shop of the same name keeps it; this one adds its id.
        let name = if name_taken(connection, &name, None).await? {
            format!("{name} ({})", shop.id.trim())
        } else {
            name
        };
        let record = self.new_record();
        connection
            .execute(
                "INSERT INTO catalog_suppliers
                     (id, name, source, source_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![
                    record.id.to_string(),
                    name,
                    source.code(),
                    shop.id.clone(),
                    stored(record.created_at)
                ],
            )
            .await?;
        Ok(record.id)
    }
}

/// The latest offer kept for an item, with its link's key.
struct Kept {
    offer: SupplierOffer,
    key: String,
}

async fn latest_kept(
    connection: &Connection,
    source: ProductSource,
    id: &str,
) -> Result<Option<Kept>, CatalogError> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {OFFER_COLUMNS} AND o.source = ?1 AND o.source_id = ?2
                 ORDER BY o.observed_at DESC, o.id DESC LIMIT 1"
            ),
            params![source.code(), id],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(Kept {
            offer: offer_from(&row)?,
            key: row.get(11)?,
        })),
        None => Ok(None),
    }
}

/// Whether a Supplier other than `except` is named `name`, ignoring case.
async fn name_taken(
    connection: &Connection,
    name: &str,
    except: Option<RecordId>,
) -> Result<bool, CatalogError> {
    let mut rows = connection
        .query(
            "SELECT 1 FROM catalog_suppliers
             WHERE name = ?1 COLLATE NOCASE AND deleted_at IS NULL AND id IS NOT ?2",
            params![name, except.map(|id| id.to_string())],
        )
        .await?;
    Ok(rows.next().await?.is_some())
}
