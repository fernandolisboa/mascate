use mascate_platform::KeyPart::{Column, IgnoreCase};
use mascate_platform::RecognisedBy::{Id, Keys, SingleRow};
use mascate_platform::{FollowUp, ModuleMerge, Reference, TableMerge, UniqueKey};

use crate::catalog::create_folder;
use crate::{Catalog, CatalogError, Product, Sku};

/// How this module's tables come together in a Merge Import (ADR 0032).
pub const MERGE: ModuleMerge = ModuleMerge {
    module: "catalog",
    tables: &[
        TableMerge::new(
            "catalog_products",
            "Produto",
            Keys(&[UniqueKey::live(&[Column("sku")])]),
        )
        .label(&["sku", "name"]),
        TableMerge::new(
            "catalog_suppliers",
            "Fornecedor",
            Keys(&[
                UniqueKey::live(&[IgnoreCase("name")]),
                UniqueKey::live(&[Column("source"), Column("source_id")]),
            ]),
        )
        .platform_ids(&["source_id"])
        .label(&["name"]),
        TableMerge::new("catalog_supplier_offers", "Oferta de fornecedor", Id)
            .references(&[
                Reference::to("supplier_id", "catalog_suppliers"),
                Reference::to("product_id", "catalog_products"),
            ])
            .platform_ids(&["source_id"])
            .label(&["title"]),
        TableMerge::new(
            "catalog_demand_categories",
            "Categoria seguida",
            Keys(&[UniqueKey::live(&[Column("ml_category_id")])]),
        )
        .platform_ids(&["ml_category_id"])
        .label(&["name"]),
        TableMerge::new(
            "catalog_best_sellers",
            "Mais vendido",
            Keys(&[UniqueKey::live(&[
                Column("ml_category_id"),
                Column("ml_id"),
            ])]),
        )
        .platform_ids(&["ml_category_id", "ml_id", "catalog_product_id"])
        .label(&["title"]),
        TableMerge::new(
            "catalog_opportunities",
            "Oportunidade",
            Keys(&[UniqueKey::live(&[
                Column("link_key"),
                Column("ml_product_id"),
            ])]),
        )
        .platform_ids(&["ml_product_id", "category_id"])
        .label(&["product_name"]),
        TableMerge::new(
            "catalog_discovery_settings",
            "Opções de descoberta",
            SingleRow,
        ),
    ],
};

impl Catalog {
    /// Brings the Product folders in line with a Merge Import just written
    /// (ADR 0032, decision 8), given the Products as they were before it:
    /// a folder follows its Product's new SKU as in [`Catalog::rename_sku`],
    /// and what the owner has to do by hand comes back: the folders that
    /// could not follow, and the new Products whose folder is on the other
    /// machine.
    pub async fn follow_merge(&self, before: &[Product]) -> Result<Vec<FollowUp>, CatalogError> {
        let after = self.products().await?;
        let mut follow_ups = Vec::new();
        let mut renames: Vec<(Sku, &Product)> = after
            .iter()
            .filter_map(|product| {
                let earlier = before.iter().find(|earlier| earlier.id == product.id)?;
                (earlier.sku != product.sku).then(|| (earlier.sku.clone(), product))
            })
            .collect();
        // A folder named after a SKU another rename gives up waits for it;
        // names that only swap among themselves never free up.
        loop {
            let waiting = renames.len();
            let leaving: Vec<_> = renames
                .iter()
                .map(|(from, _)| self.folder_of(from))
                .collect();
            renames.retain(|(from, product)| {
                let (from, to) = (self.folder_of(from), self.folder(product));
                if leaving.contains(&to) {
                    return true;
                }
                let renamed = match (from.exists(), to.exists()) {
                    (true, true) => Err(CatalogError::FolderTaken(to)),
                    (true, false) => {
                        std::fs::rename(&from, &to).map_err(|source| CatalogError::Folder {
                            path: from.clone(),
                            source,
                        })
                    }
                    // The folder was already under the new name.
                    (false, true) => Ok(()),
                    (false, false) => create_folder(&to),
                };
                if let Err(error) = renamed {
                    follow_ups.push(not_renamed(product, &error));
                }
                false
            });
            if renames.len() == waiting {
                break;
            }
        }
        for (_, product) in renames {
            let error = CatalogError::FolderTaken(self.folder(product));
            follow_ups.push(not_renamed(product, &error));
        }
        for product in after
            .iter()
            .filter(|product| before.iter().all(|earlier| earlier.id != product.id))
        {
            if !self.folder(product).exists() {
                follow_ups.push(FollowUp::CopyFolder {
                    product: label(product),
                });
            }
        }
        Ok(follow_ups)
    }
}

/// The Product as the Merge Import's report names it.
fn label(product: &Product) -> String {
    format!("{} · {}", product.sku, product.name)
}

fn not_renamed(product: &Product, error: &CatalogError) -> FollowUp {
    FollowUp::FolderNotRenamed {
        product: label(product),
        reason: error.to_string(),
    }
}
