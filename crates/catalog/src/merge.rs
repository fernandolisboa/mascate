use mascate_platform::KeyPart::{Column, IgnoreCase};
use mascate_platform::RecognisedBy::{Id, Keys, SingleRow};
use mascate_platform::{ModuleMerge, Reference, TableMerge, UniqueKey};

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
