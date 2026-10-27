//! A Program the owner adds before its first sale (#194, ADR 0037 decision
//! 5), so an Affiliate Link can exist right away. On a Platform with one
//! Program per product its key is provisional: when the first Conversion of
//! a product the app does not know arrives there, the Affiliate offers the
//! pair as a join, and only the owner's confirmation makes them one. Two
//! products with similar names are common on Hotmart, so nothing is joined
//! by the app.

use libsql::{Connection, TransactionBehavior, params};
use mascate_kernel::{RecordId, Timestamp, folded_words};
use mascate_platform::{Migration, StoredRow, stored};

use crate::commissions::{Affiliate, AffiliateError, platform_at};
use crate::report::ProgramRef;
use crate::rules::Program;
use crate::source::AffiliatePlatform;

pub(crate) const ADD_PROVISIONAL_PROGRAMS: Migration = Migration {
    version: 8,
    name: "add Programs added before their first sale and the joins declined",
    risky: false,
    sql: "ALTER TABLE affiliate_programs ADD COLUMN provisional INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE affiliate_programs ADD COLUMN product TEXT;
    CREATE TABLE affiliate_declined_joins (
        id             TEXT PRIMARY KEY,
        provisional_id TEXT NOT NULL REFERENCES affiliate_programs (id),
        sold_id        TEXT NOT NULL REFERENCES affiliate_programs (id),
        created_at     TEXT NOT NULL,
        updated_at     TEXT NOT NULL,
        deleted_at     TEXT
    );
    CREATE UNIQUE INDEX affiliate_declined_joins_by_pair
        ON affiliate_declined_joins (provisional_id, sold_id)
        WHERE deleted_at IS NULL;",
};

/// A Program as the owner adds it, from the Platform's affiliation page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProgram {
    pub platform: AffiliatePlatform,
    /// Empty means the product's name.
    pub name: String,
    /// As the Platform names it.
    pub product: String,
}

/// A Program the owner added and the Program a first sale brought on the
/// same Platform, which may be the same affiliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramJoin {
    /// The one the owner added, still provisional.
    pub provisional: Program,
    /// The one the sale brought.
    pub sold: ProgramRef,
    /// The product of its first sale, as the Platform names it.
    pub product: String,
    pub first_sold_at: Timestamp,
}

/// Every pair to ask about: a live provisional Program and a live Program
/// of the same Platform born from a sale since it was added, unless the
/// owner already said they are different. One the owner added and joined
/// keeps its product, so it is never offered again.
const JOIN_QUERY: &str = "SELECT a.id, s.id, s.platform, s.name, f.product, f.sold_at
     FROM affiliate_programs a
     JOIN affiliate_programs s
         ON s.platform = a.platform AND s.product IS NULL AND s.deleted_at IS NULL
         AND s.created_at >= a.created_at
     JOIN affiliate_conversions f ON f.id = (
         SELECT v.id FROM affiliate_conversions v
         WHERE v.program_id = s.id AND v.deleted_at IS NULL
         ORDER BY v.sold_at, v.id LIMIT 1)
     WHERE a.provisional = 1 AND a.deleted_at IS NULL
       AND NOT EXISTS (SELECT 1 FROM affiliate_declined_joins d
                       WHERE d.provisional_id = a.id AND d.sold_id = s.id
                           AND d.deleted_at IS NULL)";

impl Affiliate {
    /// Adds the Program of `new.product` before its first sale. On a
    /// Platform with one Program the Platform's own is given back, created
    /// the way a Sync would when nothing brought it yet; elsewhere the
    /// Program is provisional until the owner joins it to a sale's. Adding
    /// the same product again gives back the one added before.
    pub async fn add_program(&self, new: &NewProgram) -> Result<Program, AffiliateError> {
        let product = new.product.trim();
        if product.is_empty() {
            return Err(AffiliateError::NoProduct);
        }
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let now = self.clock.now();
        let id = match new.platform.own_program() {
            Some(own) => match any_program_of(&transaction, new.platform, &own.id).await? {
                Some(kept) => kept,
                None => {
                    self.program_for(&transaction, new.platform, &own, now)
                        .await?
                }
            },
            None => match added_before(&transaction, new.platform, product).await? {
                Some(kept) => kept,
                None => {
                    let id = self.ids.next_id();
                    let name = match new.name.trim() {
                        "" => product,
                        typed => typed,
                    };
                    transaction
                        .execute(
                            "INSERT INTO affiliate_programs
                                 (id, platform, program_key, name, provisional, product,
                                  created_at, updated_at)
                             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?6)",
                            params![
                                id.to_string(),
                                new.platform.code(),
                                format!("provisional:{id}"),
                                name,
                                product,
                                stored(now)
                            ],
                        )
                        .await?;
                    id
                }
            },
        };
        transaction.commit().await?;
        self.program(id).await
    }

    /// The joins waiting for the owner, the oldest sale first.
    pub async fn joins(&self) -> Result<Vec<ProgramJoin>, AffiliateError> {
        let pairs = join_pairs(self.database.connection(), "", ()).await?;
        let mut joins = Vec::with_capacity(pairs.len());
        for (provisional, sold, product, first_sold_at) in pairs {
            joins.push(ProgramJoin {
                provisional: self.program(provisional).await?,
                sold,
                product,
                first_sold_at,
            });
        }
        joins.sort_by(|a, b| {
            (a.first_sold_at, a.sold.id, a.provisional.id).cmp(&(
                b.first_sold_at,
                b.sold.id,
                b.provisional.id,
            ))
        });
        Ok(joins)
    }

    /// The owner says `sold` is the Program they added as `provisional`. In
    /// one transaction `provisional` takes the Platform's key, the sale's
    /// Conversions with their Commissions, and the links and Product links
    /// it did not have; `sold` is deleted. The name, the rules and the
    /// links of `provisional` stay.
    pub async fn confirm_join(
        &self,
        provisional: RecordId,
        sold: RecordId,
    ) -> Result<Program, AffiliateError> {
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        ensure_offered(&transaction, provisional, sold).await?;
        let now = stored(self.clock.now());
        let (kept, sold) = (provisional.to_string(), sold.to_string());
        transaction
            .execute(
                "UPDATE affiliate_programs SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                params![now.clone(), sold.clone()],
            )
            .await?;
        transaction
            .execute(
                "UPDATE affiliate_programs
                 SET program_key = (SELECT program_key FROM affiliate_programs WHERE id = ?2),
                     provisional = 0, updated_at = ?3
                 WHERE id = ?1",
                params![kept.clone(), sold.clone(), now.clone()],
            )
            .await?;
        transaction
            .execute(
                "UPDATE affiliate_conversions SET program_id = ?1, updated_at = ?3
                 WHERE program_id = ?2",
                params![kept.clone(), sold.clone(), now.clone()],
            )
            .await?;
        transaction
            .execute(
                "UPDATE affiliate_links SET program_id = ?1, updated_at = ?3
                 WHERE program_id = ?2 AND deleted_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM affiliate_links k
                                   WHERE k.program_id = ?1 AND k.product = affiliate_links.product
                                       AND k.channel_id = affiliate_links.channel_id
                                       AND k.deleted_at IS NULL)",
                params![kept.clone(), sold.clone(), now.clone()],
            )
            .await?;
        transaction
            .execute(
                "UPDATE affiliate_product_links SET program_id = ?1, updated_at = ?3
                 WHERE program_id = ?2 AND deleted_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM affiliate_product_links k
                                   WHERE k.program_id = ?1
                                       AND k.product = affiliate_product_links.product
                                       AND k.deleted_at IS NULL)",
                params![kept.clone(), sold.clone(), now.clone()],
            )
            .await?;
        // What the provisional Program already had for the same product
        // and channel wins; the sale's copies go with their Program.
        for table in ["affiliate_links", "affiliate_product_links"] {
            transaction
                .execute(
                    &format!(
                        "UPDATE {table} SET deleted_at = ?1, updated_at = ?1
                         WHERE program_id = ?2 AND deleted_at IS NULL"
                    ),
                    params![now.clone(), sold.clone()],
                )
                .await?;
        }
        transaction.commit().await?;
        self.program(provisional).await
    }

    /// The owner says `sold` is not the Program they added as
    /// `provisional`: both stay, and the pair is not offered again.
    pub async fn decline_join(
        &self,
        provisional: RecordId,
        sold: RecordId,
    ) -> Result<(), AffiliateError> {
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        ensure_offered(&transaction, provisional, sold).await?;
        transaction
            .execute(
                "INSERT INTO affiliate_declined_joins
                     (id, provisional_id, sold_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![
                    self.ids.next_id().to_string(),
                    provisional.to_string(),
                    sold.to_string(),
                    stored(self.clock.now())
                ],
            )
            .await?;
        transaction.commit().await?;
        Ok(())
    }
}

/// The pairs [`JOIN_QUERY`] finds, narrowed by `condition`.
async fn join_pairs(
    on: &Connection,
    condition: &str,
    params: impl libsql::params::IntoParams,
) -> Result<Vec<(RecordId, ProgramRef, String, Timestamp)>, AffiliateError> {
    let mut rows = on
        .query(&format!("{JOIN_QUERY} {condition}"), params)
        .await?;
    let mut pairs = Vec::new();
    while let Some(row) = rows.next().await? {
        pairs.push((
            row.id_at(0)?,
            ProgramRef {
                id: row.id_at(1)?,
                platform: platform_at(&row, 2)?,
                name: row.get(3)?,
            },
            row.get(4)?,
            row.time_at(5)?,
        ));
    }
    Ok(pairs)
}

/// Fails unless `provisional` and `sold` are a pair waiting for the owner.
async fn ensure_offered(
    on: &Connection,
    provisional: RecordId,
    sold: RecordId,
) -> Result<(), AffiliateError> {
    let pairs = join_pairs(
        on,
        "AND a.id = ?1 AND s.id = ?2",
        params![provisional.to_string(), sold.to_string()],
    )
    .await?;
    if pairs.is_empty() {
        return Err(AffiliateError::NotJoinable);
    }
    Ok(())
}

/// The Program of a Platform with one: the one with the Platform's own key,
/// else one a Commission by hand created.
async fn any_program_of(
    on: &Connection,
    platform: AffiliatePlatform,
    own_key: &str,
) -> Result<Option<RecordId>, AffiliateError> {
    let mut rows = on
        .query(
            "SELECT id FROM affiliate_programs
             WHERE platform = ?1 AND deleted_at IS NULL
             ORDER BY program_key <> ?2, created_at, id
             LIMIT 1",
            params![platform.code(), own_key],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row.id_at(0)?)),
        None => Ok(None),
    }
}

/// The provisional Program of `product` on `platform` added before, case,
/// accents and spacing aside.
async fn added_before(
    on: &Connection,
    platform: AffiliatePlatform,
    product: &str,
) -> Result<Option<RecordId>, AffiliateError> {
    let mut rows = on
        .query(
            "SELECT id, product FROM affiliate_programs
             WHERE platform = ?1 AND provisional = 1 AND deleted_at IS NULL
             ORDER BY created_at, id",
            params![platform.code()],
        )
        .await?;
    let wanted = folded_words(product);
    while let Some(row) = rows.next().await? {
        let added: Option<String> = row.get(1)?;
        if added.is_some_and(|added| folded_words(&added) == wanted) {
            return Ok(Some(row.id_at(0)?));
        }
    }
    Ok(None)
}
