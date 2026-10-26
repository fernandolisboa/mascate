use mascate_platform::KeyPart::Column;
use mascate_platform::RecognisedBy::{Id, Keys, SingleRow};
use mascate_platform::{ModuleMerge, Reference, TableMerge, UniqueKey};

const TO_PROGRAM: Reference = Reference::to("program_id", "affiliate_programs");
const TO_COMMISSION: Reference = Reference::to("commission_id", "affiliate_commissions");

/// How this module's tables come together in a Merge Import (ADR 0032).
pub const MERGE: ModuleMerge = ModuleMerge {
    module: "affiliate",
    tables: &[
        TableMerge::new(
            "affiliate_programs",
            "Programa",
            Keys(&[UniqueKey::live(&[
                Column("platform"),
                Column("program_key"),
            ])]),
        )
        .label(&["name"]),
        TableMerge::new(
            "affiliate_conversions",
            "Conversão",
            Keys(&[UniqueKey::live(&[
                Column("platform"),
                Column("natural_key"),
            ])]),
        )
        .references(&[TO_PROGRAM])
        .label(&["platform", "product"]),
        TableMerge::new(
            "affiliate_commissions",
            "Comissão",
            Keys(&[UniqueKey::live(&[Column("conversion_id")])]),
        )
        .references(&[Reference::to("conversion_id", "affiliate_conversions")])
        .label(&["conversion_id"]),
        TableMerge::new(
            "affiliate_commission_status_changes",
            "Mudança de status de comissão",
            Id,
        )
        .references(&[TO_COMMISSION])
        .label(&["commission_id", "status"]),
        TableMerge::new(
            "affiliate_payouts",
            "Pagamento de comissões",
            Keys(&[UniqueKey::live(&[
                Column("platform"),
                Column("natural_key"),
            ])]),
        )
        .label(&["platform", "paid_at"]),
        TableMerge::new(
            "affiliate_payout_commissions",
            "Comissão paga",
            Keys(&[UniqueKey::live(&[Column("commission_id")])]),
        )
        .references(&[
            Reference::to("payout_id", "affiliate_payouts"),
            TO_COMMISSION,
        ])
        .label(&["commission_id"]),
        TableMerge::new(
            "affiliate_product_links",
            "Produto do programa",
            Keys(&[UniqueKey::live(&[Column("program_id"), Column("product")])]),
        )
        .references(&[
            TO_PROGRAM,
            Reference::to("catalog_product_id", "catalog_products"),
        ])
        .label(&["product"]),
        TableMerge::new(
            "affiliate_links",
            "Link de afiliado",
            Keys(&[UniqueKey::live(&[
                Column("program_id"),
                Column("product"),
                Column("channel_id"),
            ])]),
        )
        .references(&[
            TO_PROGRAM,
            Reference::to("channel_id", "marketing_promotion_channels"),
        ])
        .label(&["product", "channel_id"]),
        TableMerge::new(
            "affiliate_short_links",
            "Link curto",
            Keys(&[UniqueKey::live(&[Column("link_id")])]),
        )
        .references(&[Reference::to("link_id", "affiliate_links")])
        .label(&["short"]),
        TableMerge::new(
            "affiliate_link_clicks",
            "Cliques do link curto",
            Keys(&[UniqueKey::live(&[Column("short_link_id"), Column("day")])]),
        )
        .references(&[Reference::to("short_link_id", "affiliate_short_links")])
        .label(&["day", "clicks"]),
        TableMerge::new("affiliate_click_reads", "Leitura de cliques", SingleRow),
        TableMerge::new(
            "affiliate_syncs",
            "Sync de afiliado",
            Keys(&[UniqueKey::live(&[Column("platform")])]),
        )
        .label(&["platform"]),
    ],
};
