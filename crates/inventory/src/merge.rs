use mascate_platform::KeyPart::Column;
use mascate_platform::RecognisedBy::{Id, Keys, Pairs};
use mascate_platform::{Condition, ModuleMerge, Reference, TableMerge, UniqueKey};

use crate::ledger::{ADJUSTMENT_REASONS, ORDER_REASONS, RECEIPT_REASONS};

/// Movements of an Order: each machine that read the Order wrote its own.
const FOR_AN_ORDER: Condition = Condition {
    column: "reason",
    values: ORDER_REASONS,
};

/// How this module's tables come together in a Merge Import (ADR 0032).
pub const MERGE: ModuleMerge = ModuleMerge {
    module: "inventory",
    tables: &[
        TableMerge::new("inventory_stock_locations", "Local de estoque", Id).label(&["name"]),
        TableMerge::new(
            "inventory_stock_movements",
            "Movimento de estoque",
            Pairs {
                when: Some(FOR_AN_ORDER),
                key: &[
                    "reason",
                    "reference_id",
                    "product_id",
                    "location_id",
                    "quantity",
                ],
            },
        )
        .references(&[
            Reference::to("product_id", "catalog_products"),
            Reference::to("location_id", "inventory_stock_locations"),
            Reference::to("reference_id", "commerce_orders").when(FOR_AN_ORDER),
            Reference::to("reference_id", "commerce_purchase_orders").when(Condition {
                column: "reason",
                values: RECEIPT_REASONS,
            }),
            // An adjustment refers to itself.
            Reference::to("reference_id", "inventory_stock_movements").when(Condition {
                column: "reason",
                values: ADJUSTMENT_REASONS,
            }),
        ])
        .label(&["product_id", "reason", "quantity"])
        .immutable(),
        TableMerge::new(
            "inventory_reorder_points",
            "Ponto de reposição",
            Keys(&[UniqueKey::every_row(&[Column("product_id")])]),
        )
        .references(&[Reference::to("product_id", "catalog_products")])
        .label(&["product_id"]),
    ],
};
