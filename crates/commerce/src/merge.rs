use mascate_platform::KeyPart::{Column, NullAsEmpty};
use mascate_platform::RecognisedBy::{Id, Keys, SingleRow};
use mascate_platform::{ModuleMerge, Reference, TableMerge, UniqueKey};

const TO_LISTING: Reference = Reference::to("listing_id", "commerce_listings");
const TO_ORDER: Reference = Reference::to("order_id", "commerce_orders");
const TO_PRODUCT: Reference = Reference::to("product_id", "catalog_products");

/// How this module's tables come together in a Merge Import (ADR 0032).
pub const MERGE: ModuleMerge = ModuleMerge {
    module: "commerce",
    tables: &[
        TableMerge::new(
            "commerce_listings",
            "Anúncio",
            Keys(&[UniqueKey::live(&[
                Column("ml_item_id"),
                NullAsEmpty("ml_variation_id"),
            ])]),
        )
        .references(&[TO_PRODUCT])
        .platform_ids(&["ml_item_id", "ml_variation_id", "category_id"])
        .label(&["title", "variation_name"]),
        TableMerge::new(
            "commerce_channel_categories",
            "Categoria do canal",
            Keys(&[UniqueKey::live(&[Column("ml_category_id")])]),
        )
        .platform_ids(&["ml_category_id"])
        .label(&["name"]),
        TableMerge::new(
            "commerce_listing_drafts",
            "Rascunho de anúncio",
            Keys(&[UniqueKey::every_row(&[Column("listing_id")])]),
        )
        .references(&[TO_LISTING])
        .label(&["listing_id"]),
        TableMerge::new("commerce_draft_attributes", "Atributo de rascunho", Id)
            .references(&[TO_LISTING])
            .platform_ids(&["attribute_id"])
            .label(&["listing_id", "name"]),
        TableMerge::new("commerce_draft_pictures", "Foto de rascunho", Id)
            .references(&[TO_LISTING])
            .platform_ids(&["channel_picture_id"])
            .label(&["listing_id", "path"]),
        TableMerge::new(
            "commerce_listing_stock",
            "Estoque enviado ao anúncio",
            Keys(&[UniqueKey::every_row(&[Column("listing_id")])]),
        )
        .references(&[TO_LISTING])
        .label(&["listing_id"]),
        TableMerge::new(
            "commerce_target_margins",
            "Margem-alvo",
            Keys(&[UniqueKey::every_row(&[Column("product_id")])]),
        )
        .references(&[TO_PRODUCT])
        .label(&["product_id"]),
        TableMerge::new("commerce_pricing_settings", "Opções de preço", SingleRow),
        TableMerge::new(
            "commerce_orders",
            "Pedido",
            Keys(&[UniqueKey::live(&[Column("ml_order_id")])]),
        )
        .platform_ids(&["ml_order_id", "ml_pack_id", "ml_shipment_id"])
        .label(&["ml_order_id"]),
        TableMerge::new(
            "commerce_order_lines",
            "Linha de pedido",
            Keys(&[UniqueKey::live(&[
                Column("order_id"),
                Column("ml_item_id"),
                NullAsEmpty("ml_variation_id"),
            ])]),
        )
        .references(&[
            TO_ORDER,
            TO_PRODUCT,
            Reference::to("stock_movement_id", "inventory_stock_movements"),
        ])
        .platform_ids(&["ml_item_id", "ml_variation_id"])
        .label(&["order_id", "title"]),
        TableMerge::new(
            "commerce_order_returns",
            "Devolução",
            Keys(&[UniqueKey::live(&[
                Column("order_id"),
                Column("ml_return_id"),
                Column("ml_item_id"),
                NullAsEmpty("ml_variation_id"),
            ])]),
        )
        .references(&[TO_ORDER])
        .platform_ids(&["ml_return_id", "ml_item_id", "ml_variation_id"])
        .label(&["order_id", "ml_return_id"]),
        TableMerge::new(
            "commerce_order_fees",
            "Tarifa de pedido",
            Keys(&[UniqueKey::live(&[
                Column("order_id"),
                Column("ml_charge_id"),
            ])]),
        )
        .references(&[TO_ORDER])
        .platform_ids(&["ml_charge_id"])
        .label(&["order_id", "description"]),
        TableMerge::new(
            "commerce_money_releases",
            "Liberação de dinheiro",
            Keys(&[UniqueKey::live(&[
                Column("order_id"),
                Column("ml_payment_id"),
            ])]),
        )
        .references(&[TO_ORDER])
        .platform_ids(&["ml_payment_id"])
        .label(&["order_id", "ml_payment_id"]),
        TableMerge::new("commerce_order_settings", "Opções de pedidos", SingleRow),
        TableMerge::new("commerce_order_syncs", "Sync de pedidos", SingleRow),
        TableMerge::new(
            "commerce_promotions",
            "Promoção",
            Keys(&[UniqueKey::every_row(&[Column("ml_promotion_id")])]),
        )
        .platform_ids(&["ml_promotion_id"])
        .label(&["name"]),
        TableMerge::new(
            "commerce_promotion_offers",
            "Oferta de promoção",
            Keys(&[UniqueKey::every_row(&[
                Column("ml_item_id"),
                Column("kind"),
                Column("ml_promotion_id"),
            ])]),
        )
        .platform_ids(&["ml_item_id", "ml_promotion_id"])
        .label(&["name", "ml_item_id"]),
        TableMerge::new(
            "commerce_promotion_settings",
            "Opções de promoções",
            SingleRow,
        ),
        TableMerge::new("commerce_promotion_syncs", "Sync de promoções", SingleRow),
        TableMerge::new("commerce_purchase_orders", "Ordem de compra", Id)
            .references(&[Reference::to("supplier_id", "catalog_suppliers")])
            .label(&["supplier_id", "ordered_on"]),
        TableMerge::new(
            "commerce_purchase_order_lines",
            "Linha de ordem de compra",
            Id,
        )
        .references(&[
            Reference::to("purchase_order_id", "commerce_purchase_orders"),
            TO_PRODUCT,
        ])
        .label(&["purchase_order_id", "product_id"]),
        TableMerge::new("commerce_purchase_receipts", "Recebimento", Id)
            .references(&[Reference::to(
                "purchase_order_id",
                "commerce_purchase_orders",
            )])
            .label(&["purchase_order_id"]),
        TableMerge::new(
            "commerce_purchase_receipt_lines",
            "Linha de recebimento",
            Id,
        )
        .references(&[
            Reference::to("receipt_id", "commerce_purchase_receipts"),
            Reference::to("line_id", "commerce_purchase_order_lines"),
        ])
        .label(&["line_id", "quantity"]),
    ],
};
