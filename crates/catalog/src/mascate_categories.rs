//! Mascate Categories (#237): the owner's own categories, for any theme,
//! each grouping categories of the Sales Channel at any level of its tree,
//! with optional keywords. Best sellers are read per channel category and
//! shown per Mascate Category, so "Café" can join grains, coffee makers and
//! grinders that sit in different branches.

use std::collections::HashSet;

use libsql::{Transaction, params};
use mascate_kernel::{RecordId, folded_words};
use mascate_platform::{Migration, StoredRow};

use crate::catalog::required;
use crate::{Catalog, CatalogError, DemandCategory};

/// Each followed category becomes a Mascate Category of the same name that
/// holds it; the followed category's id serves the new record, so no id is
/// made up in SQL.
pub(crate) const CREATE_MASCATE_CATEGORIES: Migration = Migration {
    version: 4,
    name: "create mascate categories over the followed demand categories",
    risky: false,
    sql: "CREATE TABLE catalog_mascate_categories (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        keywords   TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE UNIQUE INDEX catalog_mascate_categories_by_name
        ON catalog_mascate_categories (name COLLATE NOCASE) WHERE deleted_at IS NULL;
    INSERT INTO catalog_mascate_categories (id, name, keywords, created_at, updated_at)
        SELECT id, name, '', created_at, updated_at
        FROM catalog_demand_categories WHERE deleted_at IS NULL;
    ALTER TABLE catalog_demand_categories ADD COLUMN mascate_category_id TEXT;
    UPDATE catalog_demand_categories SET mascate_category_id = id WHERE deleted_at IS NULL;
    DROP INDEX catalog_demand_categories_by_ml_id;
    CREATE UNIQUE INDEX catalog_demand_categories_by_mascate_category
        ON catalog_demand_categories (mascate_category_id, ml_category_id)
        WHERE deleted_at IS NULL;",
};

/// Keywords are stored one per line.
const KEYWORD_SEPARATOR: char = '\n';

/// A category of the owner's own, grouping categories of the Sales Channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MascateCategory {
    pub id: RecordId,
    pub name: String,
    /// The Sales Channel's categories it groups, in the order they were added.
    pub channel_categories: Vec<DemandCategory>,
    /// A best seller belongs only when its title holds one of them; with
    /// none, every best seller of its channel categories belongs.
    pub keywords: Vec<String>,
}

impl MascateCategory {
    /// Whether a best seller titled `title` belongs: its title holds one of
    /// the keywords as whole words, accents and case aside.
    pub fn takes(&self, title: &str) -> bool {
        if self.keywords.is_empty() {
            return true;
        }
        let words = folded_words(title);
        self.keywords.iter().any(|keyword| {
            let wanted = folded_words(keyword);
            !wanted.is_empty()
                && words
                    .windows(wanted.len())
                    .any(|window| window == wanted.as_slice())
        })
    }
}

/// What the owner fills in to create or change a Mascate Category.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MascateCategoryDraft {
    pub name: String,
    pub channel_categories: Vec<DemandCategory>,
    pub keywords: Vec<String>,
}

/// A draft checked and tidied: spaces collapsed, repeats dropped.
struct Checked {
    name: String,
    channel_categories: Vec<DemandCategory>,
    keywords: String,
}

impl Catalog {
    /// The owner's Mascate Categories, by name.
    pub async fn mascate_categories(&self) -> Result<Vec<MascateCategory>, CatalogError> {
        let mut rows = self
            .connection()
            .query(
                "SELECT id, name, keywords FROM catalog_mascate_categories
                 WHERE deleted_at IS NULL ORDER BY name COLLATE NOCASE",
                (),
            )
            .await?;
        let mut categories = Vec::new();
        while let Some(row) = rows.next().await? {
            let keywords: String = row.get(2)?;
            categories.push(MascateCategory {
                id: row.id_at(0)?,
                name: row.get(1)?,
                channel_categories: Vec::new(),
                keywords: keywords
                    .split(KEYWORD_SEPARATOR)
                    .filter(|keyword| !keyword.is_empty())
                    .map(Into::into)
                    .collect(),
            });
        }
        let mut rows = self
            .connection()
            .query(
                "SELECT mascate_category_id, ml_category_id, name FROM catalog_demand_categories
                 WHERE deleted_at IS NULL AND mascate_category_id IS NOT NULL
                 ORDER BY created_at, id",
                (),
            )
            .await?;
        while let Some(row) = rows.next().await? {
            let owner = row.id_at(0)?;
            if let Some(category) = categories.iter_mut().find(|c| c.id == owner) {
                category.channel_categories.push(DemandCategory {
                    id: row.get(1)?,
                    name: row.get(2)?,
                });
            }
        }
        Ok(categories)
    }

    /// Creates a Mascate Category; the next Sync reads its best sellers.
    pub async fn create_mascate_category(
        &self,
        draft: MascateCategoryDraft,
    ) -> Result<MascateCategory, CatalogError> {
        let checked = self.check(&draft, None).await?;
        let id = self.next_id();
        let connection = self.database().connect_for_transaction().await?;
        let transaction = connection.transaction().await?;
        let now = self.now();
        transaction
            .execute(
                "INSERT INTO catalog_mascate_categories
                     (id, name, keywords, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![id.to_string(), checked.name.clone(), checked.keywords, now],
            )
            .await?;
        self.link(&transaction, id, &checked.channel_categories)
            .await?;
        transaction.commit().await?;
        self.mascate_category(id).await
    }

    /// Changes a Mascate Category's name, channel categories and keywords.
    pub async fn edit_mascate_category(
        &self,
        category: RecordId,
        draft: MascateCategoryDraft,
    ) -> Result<MascateCategory, CatalogError> {
        let checked = self.check(&draft, Some(category)).await?;
        let connection = self.database().connect_for_transaction().await?;
        let transaction = connection.transaction().await?;
        let now = self.now();
        let changed = transaction
            .execute(
                "UPDATE catalog_mascate_categories SET name = ?1, keywords = ?2, updated_at = ?3
                 WHERE id = ?4 AND deleted_at IS NULL",
                params![checked.name, checked.keywords, now, category.to_string()],
            )
            .await?;
        if changed == 0 {
            return Err(CatalogError::UnknownMascateCategory(category));
        }
        self.link(&transaction, category, &checked.channel_categories)
            .await?;
        transaction.commit().await?;
        self.mascate_category(category).await
    }

    /// Removes a Mascate Category; its best sellers stop showing unless
    /// another one holds the same channel categories.
    pub async fn remove_mascate_category(&self, category: RecordId) -> Result<(), CatalogError> {
        let connection = self.database().connect_for_transaction().await?;
        let transaction = connection.transaction().await?;
        let now = self.now();
        let removed = transaction
            .execute(
                "UPDATE catalog_mascate_categories SET deleted_at = ?1, updated_at = ?1
                 WHERE id = ?2 AND deleted_at IS NULL",
                params![now.clone(), category.to_string()],
            )
            .await?;
        if removed == 0 {
            return Err(CatalogError::UnknownMascateCategory(category));
        }
        transaction
            .execute(
                "UPDATE catalog_demand_categories SET deleted_at = ?1, updated_at = ?1
                 WHERE mascate_category_id = ?2 AND deleted_at IS NULL",
                params![now, category.to_string()],
            )
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn mascate_category(&self, id: RecordId) -> Result<MascateCategory, CatalogError> {
        self.mascate_categories()
            .await?
            .into_iter()
            .find(|category| category.id == id)
            .ok_or(CatalogError::UnknownMascateCategory(id))
    }

    /// `draft` tidied, or why it cannot be saved; `editing` is the category
    /// whose own name is not taken. Names compare as the owner reads them,
    /// accents and case aside, so "Cafe" is taken by "Café".
    async fn check(
        &self,
        draft: &MascateCategoryDraft,
        editing: Option<RecordId>,
    ) -> Result<Checked, CatalogError> {
        let name = required(&draft.name, CatalogError::MissingCategoryName)?;
        let mut seen = HashSet::new();
        let channel_categories: Vec<DemandCategory> = draft
            .channel_categories
            .iter()
            .filter(|category| seen.insert(category.id.clone()))
            .cloned()
            .collect();
        if channel_categories.is_empty() {
            return Err(CatalogError::NoChannelCategory);
        }
        let mut seen = HashSet::new();
        let keywords: Vec<String> = draft
            .keywords
            .iter()
            .map(|keyword| keyword.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|keyword| !folded_words(keyword).is_empty())
            .filter(|keyword| seen.insert(folded_words(keyword)))
            .collect();
        let wanted = folded_words(&name);
        let mut rows = self
            .connection()
            .query(
                "SELECT id, name FROM catalog_mascate_categories WHERE deleted_at IS NULL",
                (),
            )
            .await?;
        while let Some(row) = rows.next().await? {
            if Some(row.id_at(0)?) != editing && folded_words(&row.get::<String>(1)?) == wanted {
                return Err(CatalogError::CategoryNameTaken(name));
            }
        }
        Ok(Checked {
            name,
            channel_categories,
            keywords: keywords.join(&KEYWORD_SEPARATOR.to_string()),
        })
    }

    /// Makes `channel_categories` exactly the ones `category` holds.
    async fn link(
        &self,
        transaction: &Transaction,
        category: RecordId,
        channel_categories: &[DemandCategory],
    ) -> Result<(), CatalogError> {
        let now = self.now();
        let owner = category.to_string();
        for channel in channel_categories {
            let renamed = transaction
                .execute(
                    "UPDATE catalog_demand_categories SET name = ?1, updated_at = ?2
                     WHERE mascate_category_id = ?3 AND ml_category_id = ?4
                       AND deleted_at IS NULL",
                    params![
                        channel.name.clone(),
                        now.clone(),
                        owner.clone(),
                        channel.id.clone()
                    ],
                )
                .await?;
            if renamed == 0 {
                transaction
                    .execute(
                        "INSERT INTO catalog_demand_categories
                             (id, ml_category_id, name, mascate_category_id, created_at,
                              updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                        params![
                            self.next_id().to_string(),
                            channel.id.clone(),
                            channel.name.clone(),
                            owner.clone(),
                            now.clone()
                        ],
                    )
                    .await?;
            }
        }
        let mut rows = transaction
            .query(
                "SELECT ml_category_id FROM catalog_demand_categories
                 WHERE mascate_category_id = ?1 AND deleted_at IS NULL",
                params![owner.clone()],
            )
            .await?;
        let mut dropped = Vec::new();
        while let Some(row) = rows.next().await? {
            let id: String = row.get(0)?;
            if !channel_categories.iter().any(|channel| channel.id == id) {
                dropped.push(id);
            }
        }
        for id in dropped {
            transaction
                .execute(
                    "UPDATE catalog_demand_categories SET deleted_at = ?1, updated_at = ?1
                     WHERE mascate_category_id = ?2 AND ml_category_id = ?3
                       AND deleted_at IS NULL",
                    params![now.clone(), owner.clone(), id],
                )
                .await?;
        }
        Ok(())
    }
}
