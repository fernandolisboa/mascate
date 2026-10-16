use mascate_platform::KeyPart::Column;
use mascate_platform::RecognisedBy::{Id, Keys, Pairs, SingleRow};
use mascate_platform::{ModuleMerge, Reference, TableMerge, UniqueKey};

const TO_CHANNEL: Reference = Reference::to("channel_id", "marketing_promotion_channels");

/// How this module's tables come together in a Merge Import (ADR 0032).
pub const MERGE: ModuleMerge = ModuleMerge {
    module: "marketing",
    tables: &[
        TableMerge::new(
            "marketing_listing_quality",
            "Qualidade do anúncio",
            Keys(&[UniqueKey::live(&[
                Column("sales_channel"),
                Column("channel_item_id"),
            ])]),
        )
        .platform_ids(&["channel_item_id"])
        .label(&["sales_channel", "channel_item_id"]),
        TableMerge::new(
            "marketing_quality_actions",
            "Ação de qualidade",
            Keys(&[UniqueKey::live(&[
                Column("quality_id"),
                Column("channel_rule_key"),
            ])]),
        )
        .references(&[Reference::to("quality_id", "marketing_listing_quality")])
        .label(&["quality_id", "label"]),
        TableMerge::new(
            "marketing_shop_performance",
            "Desempenho da loja",
            Keys(&[UniqueKey::live(&[Column("sales_channel")])]),
        )
        .label(&["sales_channel"]),
        TableMerge::new(
            "marketing_performance_metrics",
            "Métrica de desempenho",
            Keys(&[UniqueKey::live(&[
                Column("performance_id"),
                Column("channel_metric_id"),
            ])]),
        )
        .references(&[Reference::to(
            "performance_id",
            "marketing_shop_performance",
        )])
        .platform_ids(&["channel_metric_id"])
        .label(&["name"]),
        TableMerge::new(
            "marketing_questions",
            "Pergunta",
            Keys(&[UniqueKey::live(&[
                Column("sales_channel"),
                Column("channel_question_id"),
            ])]),
        )
        .platform_ids(&["channel_question_id", "channel_item_id"])
        .label(&["text"]),
        TableMerge::new("marketing_question_syncs", "Sync de perguntas", SingleRow),
        TableMerge::new(
            "marketing_question_settings",
            "Opções de perguntas",
            SingleRow,
        ),
        TableMerge::new("marketing_reply_templates", "Resposta pronta", Id).label(&["name"]),
        TableMerge::new("marketing_reputation", "Reputação", SingleRow),
        TableMerge::new(
            "marketing_listing_reviews",
            "Opiniões do anúncio",
            Keys(&[UniqueKey::live(&[
                Column("sales_channel"),
                Column("channel_item_id"),
            ])]),
        )
        .platform_ids(&["channel_item_id"])
        .label(&["sales_channel", "channel_item_id"]),
        TableMerge::new(
            "marketing_low_reviews",
            "Opinião baixa",
            Keys(&[UniqueKey::live(&[
                Column("sales_channel"),
                Column("channel_review_id"),
            ])]),
        )
        .platform_ids(&["channel_review_id", "channel_item_id"])
        .label(&["title"]),
        TableMerge::new(
            "marketing_seller_tools",
            "Ferramenta do vendedor",
            Keys(&[UniqueKey::live(&[Column("tool")])]),
        )
        .label(&["tool"]),
        TableMerge::new(
            "marketing_ad_campaigns",
            "Campanha de Product Ads",
            Keys(&[UniqueKey::live(&[
                Column("sales_channel"),
                Column("channel_campaign_id"),
            ])]),
        )
        .platform_ids(&["channel_campaign_id"])
        .label(&["name"]),
        TableMerge::new(
            "marketing_ad_days",
            "Dia de Product Ads",
            Keys(&[UniqueKey::live(&[
                Column("sales_channel"),
                Column("channel_item_id"),
                Column("day"),
            ])]),
        )
        .platform_ids(&["channel_item_id", "channel_campaign_id"])
        .label(&["channel_item_id", "day"]),
        TableMerge::new("marketing_ads_syncs", "Sync de Product Ads", SingleRow),
        TableMerge::new("marketing_copy_settings", "Opções de texto", SingleRow),
        TableMerge::new(
            "marketing_promotion_channels",
            "Canal de divulgação",
            Keys(&[UniqueKey::live(&[Column("default_tag")])]),
        )
        .label(&["name"]),
        TableMerge::new(
            "marketing_post_templates",
            "Modelo de post",
            Keys(&[UniqueKey::live(&[Column("channel_id")])]),
        )
        .references(&[TO_CHANNEL])
        .label(&["channel_id"]),
        TableMerge::new("marketing_offer_posts", "Post de oferta", Id)
            .references(&[
                TO_CHANNEL,
                Reference::to("affiliate_link_id", "affiliate_links"),
            ])
            .platform_ids(&["message_id"])
            .label(&["channel_id", "product"]),
        TableMerge::new("marketing_queue_settings", "Fila de publicação", SingleRow),
        // The common dates each machine recorded on its first calendar:
        // the same name and rule are the same date.
        TableMerge::new(
            "marketing_sales_events",
            "Data comercial",
            Pairs {
                when: None,
                key: &["name", "rule"],
            },
        )
        .label(&["name"]),
        TableMerge::new(
            "marketing_sales_event_settings",
            "Opções do calendário",
            SingleRow,
        ),
    ],
};
