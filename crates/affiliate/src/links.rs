//! The same Product on the sale and affiliate sides (#86, story 91): the
//! owner links a Program's product to a Product of the Catalog, to compare
//! reselling it with promoting it. Affiliate keeps only the Product's id and
//! never reads the Catalog; the comparison is built by the caller (ADR 0030,
//! decision 8).

use libsql::{TransactionBehavior, params};
use mascate_kernel::RecordId;
use mascate_platform::{Migration, StoredRow, stored};

use crate::commissions::{Affiliate, AffiliateError, platform_at};
use crate::report::ProgramRef;

pub(crate) const CREATE_PRODUCT_LINKS: Migration = Migration {
    version: 3,
    name: "create the links from a Program's products to Products",
    risky: false,
    sql: "CREATE TABLE affiliate_product_links (
        id                 TEXT PRIMARY KEY,
        program_id         TEXT NOT NULL REFERENCES affiliate_programs (id),
        product            TEXT NOT NULL,
        catalog_product_id TEXT NOT NULL,
        created_at         TEXT NOT NULL,
        updated_at         TEXT NOT NULL,
        deleted_at         TEXT
    );
    CREATE UNIQUE INDEX affiliate_product_links_by_product
        ON affiliate_product_links (program_id, product)
        WHERE deleted_at IS NULL;",
};

/// A product the owner promotes through a Program, with the Product it is
/// linked to, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotedProduct {
    pub program: ProgramRef,
    /// As the Platform names it.
    pub product: String,
    /// The Catalog's Product.
    pub linked_to: Option<RecordId>,
    /// Its Commissions, of every status.
    pub commissions: usize,
}

impl Affiliate {
    /// Every product with a Commission, per Program, by Platform, Program
    /// and product name.
    pub async fn promoted_products(&self) -> Result<Vec<PromotedProduct>, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT p.id, p.platform, p.name, trim(v.product), k.catalog_product_id,
                     COUNT(c.id)
                 FROM affiliate_commissions c
                 JOIN affiliate_conversions v ON v.id = c.conversion_id
                 JOIN affiliate_programs p ON p.id = v.program_id
                 LEFT JOIN affiliate_product_links k
                     ON k.program_id = p.id AND k.product = trim(v.product)
                     AND k.deleted_at IS NULL
                 WHERE c.deleted_at IS NULL AND v.deleted_at IS NULL
                 GROUP BY p.id, trim(v.product)",
                (),
            )
            .await?;
        let mut products = Vec::new();
        while let Some(row) = rows.next().await? {
            products.push(PromotedProduct {
                program: ProgramRef {
                    id: row.id_at(0)?,
                    platform: platform_at(&row, 1)?,
                    name: row.get(2)?,
                },
                product: row.get(3)?,
                linked_to: row.optional_id_at(4)?,
                commissions: usize::try_from(row.get::<i64>(5)?).unwrap_or(0),
            });
        }
        products.sort_by(|a, b| {
            (a.program.platform.name(), &a.program.name, &a.product).cmp(&(
                b.program.platform.name(),
                &b.program.name,
                &b.product,
            ))
        });
        Ok(products)
    }

    /// Links `product` of `program` to the Catalog's Product `to`, in place
    /// of any Product it was linked to: a product links to one Product at
    /// most.
    pub async fn link_product(
        &self,
        program: RecordId,
        product: &str,
        to: RecordId,
    ) -> Result<(), AffiliateError> {
        let product = linkable(product)?;
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let known = transaction
            .query(
                "SELECT 1 FROM affiliate_programs WHERE id = ?1 AND deleted_at IS NULL",
                params![program.to_string()],
            )
            .await?
            .next()
            .await?
            .is_some();
        if !known {
            return Err(AffiliateError::NoSuchProgram);
        }
        transaction
            .execute(
                "INSERT INTO affiliate_product_links
                     (id, program_id, product, catalog_product_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT (program_id, product) WHERE deleted_at IS NULL
                 DO UPDATE SET catalog_product_id = excluded.catalog_product_id,
                     updated_at = excluded.updated_at
                 WHERE catalog_product_id <> excluded.catalog_product_id",
                params![
                    self.ids.next_id().to_string(),
                    program.to_string(),
                    product,
                    to.to_string(),
                    stored(self.clock.now())
                ],
            )
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Takes `product` of `program` off the Product it was linked to; one
    /// not linked stays as it is.
    pub async fn unlink_product(
        &self,
        program: RecordId,
        product: &str,
    ) -> Result<(), AffiliateError> {
        let product = linkable(product)?;
        self.database
            .connection()
            .execute(
                "UPDATE affiliate_product_links SET deleted_at = ?1, updated_at = ?1
                 WHERE program_id = ?2 AND product = ?3 AND deleted_at IS NULL",
                params![stored(self.clock.now()), program.to_string(), product],
            )
            .await?;
        Ok(())
    }
}

/// A product's name as a link keeps it: without the spaces around it, as
/// the database trims the Conversions' product to match it.
fn linkable(product: &str) -> Result<&str, AffiliateError> {
    if product.trim().is_empty() {
        return Err(AffiliateError::NoProduct);
    }
    Ok(product.trim_matches(' '))
}
