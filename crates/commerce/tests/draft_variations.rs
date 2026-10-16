//! Drafts with variations through commerce's public interface, with a real
//! temporary database and an in-memory Shopee: each variation a Product of
//! its own, published as one listing whose models become each Product's
//! Listing, and a retry that never sends the listing or its variations
//! twice.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    ChannelListing, Check, ChecklistItem, DraftEdit, DraftStart, DraftVariation, ListingDraft,
    ListingError, ListingStatus, Listings, MAX_VARIATIONS, PublishedVariation, Variation,
    VariationToPublish, VariationsToPublish, takes_variations,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{PlatformError, RecordId, SalesChannelKind};
use mascate_platform::{Database, migrate};

use common::{Channel, Publisher, Publishing, brl, listing, product, value};

struct Fixture {
    dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    listings: Listings,
    publisher: Publisher,
    channel: Channel,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
        ));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[mascate_commerce::MIGRATIONS])
            .await
            .unwrap();
        Self {
            listings: Listings::new(database, clock.clone(), Arc::new(SequentialIds::default())),
            dir,
            clock,
            publisher: Publisher::shopee(),
            channel: Channel {
                kind: SalesChannelKind::Shopee,
                ..Channel::default()
            },
        }
    }

    fn pictures(&self) -> Vec<PathBuf> {
        let folder = self.dir.path().join("produtos").join("CAM-PRE-M");
        std::fs::create_dir_all(&folder).unwrap();
        ["frente.jpg", "costas.jpg", "etiqueta.png"]
            .iter()
            .map(|name| {
                let path = folder.join(name);
                std::fs::write(&path, name.as_bytes()).unwrap();
                path
            })
            .collect()
    }

    /// A Shopee draft of the black T-shirt, with 4 units and three pictures.
    async fn draft(&self) -> ListingDraft {
        self.listings
            .create_draft(
                &product(1, "CAM-PRE-M", "Camiseta Básica Preta M"),
                DraftStart {
                    available_quantity: 4,
                    pictures: self.pictures(),
                },
                &self.publisher,
            )
            .await
            .unwrap()
    }

    /// The draft ready, sold in black (4 units), white (2) and blue (none).
    async fn ready(&self) -> ListingDraft {
        let draft = self.draft().await;
        self.clock.advance(TimeDelta::minutes(1));
        self.listings
            .save_draft(
                draft.id,
                DraftEdit {
                    variation_tier: " Cor ".into(),
                    variations: colours(),
                    ..ready_edit(&draft)
                },
            )
            .await
            .unwrap()
    }

    async fn publish(
        &self,
        draft: &ListingDraft,
    ) -> Result<mascate_commerce::Listing, ListingError> {
        self.clock.advance(TimeDelta::minutes(1));
        self.listings.publish(draft.id, &self.publisher).await
    }
}

fn variation(product: u128, sku: &str, option: &str, units: u32) -> DraftVariation {
    DraftVariation {
        product: RecordId::from_u128(product),
        seller_sku: sku.into(),
        option: option.into(),
        available_quantity: units,
    }
}

fn colours() -> Vec<DraftVariation> {
    vec![
        variation(1, "CAM-PRE-M", " Preto ", 4),
        variation(2, "CAM-BRA-M", "Branco", 2),
        variation(3, "CAM-AZU-M", "Azul", 0),
    ]
}

fn ready_edit(draft: &ListingDraft) -> DraftEdit {
    DraftEdit {
        title: "Camiseta Básica 100% Algodão".into(),
        description: "Malha 30.1, gola careca.".into(),
        listing_type: draft.listing_type,
        condition: draft.condition,
        price: brl("49.904"),
        available_quantity: draft.available_quantity,
        warranty: String::new(),
        attributes: vec![
            value("MODEL", "Básica"),
            value("BRAND", "Hering"),
            value("GTIN", "7891234567895"),
        ],
        pictures: draft.pictures.iter().map(|p| p.path.clone()).collect(),
        variation_tier: draft.variation_tier.clone(),
        variations: draft.variations.clone(),
    }
}

fn checks(checklist: &[ChecklistItem]) -> Vec<(Check, bool)> {
    checklist
        .iter()
        .map(|item| (item.check.clone(), item.blocks))
        .collect()
}

#[test]
fn only_shopee_takes_variations_from_the_app() {
    assert!(takes_variations(SalesChannelKind::Shopee));
    for channel in [
        SalesChannelKind::MercadoLivre,
        SalesChannelKind::Amazon,
        SalesChannelKind::Magalu,
    ] {
        assert!(!takes_variations(channel), "{channel:?}");
    }
}

#[test]
fn a_draft_keeps_its_variations_and_its_units_follow_its_products_variation() {
    block_on(async {
        let fx = Fixture::new().await;

        let draft = fx.ready().await;

        assert_eq!(draft.variation_tier, "Cor");
        assert_eq!(
            draft.variations,
            [
                variation(1, "CAM-PRE-M", "Preto", 4),
                variation(2, "CAM-BRA-M", "Branco", 2),
                variation(3, "CAM-AZU-M", "Azul", 0),
            ]
        );
        assert_eq!(draft.available_quantity, 4);
        assert_eq!(draft.units(), 6);
        assert!(!draft.variations_pending);
        assert_eq!(
            fx.listings.drafts().await.unwrap(),
            std::slice::from_ref(&draft)
        );

        // Without variations again, the draft's own units stand.
        let single = fx
            .listings
            .save_draft(
                draft.id,
                DraftEdit {
                    available_quantity: 3,
                    variations: Vec::new(),
                    ..ready_edit(&draft)
                },
            )
            .await
            .unwrap();
        assert!(single.variations.is_empty());
        assert_eq!(single.variation_tier, "");
        assert_eq!(single.units(), 3);
    });
}

#[test]
fn variations_go_only_to_shopee_each_product_once_and_with_the_drafts_own() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.draft().await;
        let with = |variations| DraftEdit {
            variation_tier: "Cor".into(),
            variations,
            ..ready_edit(&draft)
        };

        assert!(matches!(
            fx.listings
                .save_draft(
                    draft.id,
                    with(vec![
                        variation(1, "CAM-PRE-M", "Preto", 4),
                        variation(1, "CAM-PRE-M", "Preto de novo", 4),
                    ])
                )
                .await,
            Err(ListingError::RepeatedVariation(product)) if product == RecordId::from_u128(1)
        ));
        assert!(matches!(
            fx.listings
                .save_draft(draft.id, with(vec![variation(2, "CAM-BRA-M", "Branco", 2)]))
                .await,
            Err(ListingError::VariationsWithoutTheDraftProduct)
        ));

        let on_mercado_livre = Publisher::default();
        let other = fx
            .listings
            .create_draft(
                &product(2, "CAM-BRA-M", "Camiseta Básica Branca M"),
                DraftStart {
                    available_quantity: 2,
                    pictures: Vec::new(),
                },
                &on_mercado_livre,
            )
            .await
            .unwrap();
        assert!(matches!(
            fx.listings
                .save_draft(
                    other.id,
                    DraftEdit {
                        variation_tier: "Cor".into(),
                        variations: vec![variation(2, "CAM-BRA-M", "Branco", 2)],
                        ..ready_edit(&other)
                    }
                )
                .await,
            Err(ListingError::NoVariations(SalesChannelKind::MercadoLivre))
        ));
        assert_eq!(fx.listings.draft(draft.id).await.unwrap(), draft);
    });
}

#[test]
fn the_checklist_blocks_on_a_missing_tier_or_option_a_repeated_one_and_too_many() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.draft().await;
        let saved = fx
            .listings
            .save_draft(
                draft.id,
                DraftEdit {
                    variation_tier: "  ".into(),
                    variations: vec![
                        variation(1, "CAM-PRE-M", "Preto", 0),
                        variation(2, "CAM-BRA-M", " ", 0),
                        variation(3, "CAM-AZU-M", "preto", 0),
                    ],
                    ..ready_edit(&draft)
                },
            )
            .await
            .unwrap();

        assert_eq!(
            checks(&saved.checklist()),
            [
                (Check::VariationTier, true),
                (
                    Check::VariationOption {
                        seller_sku: "CAM-BRA-M".into()
                    },
                    true
                ),
                (Check::RepeatedOption("preto".into()), true),
                (Check::Stock, false),
            ]
        );

        let many = ListingDraft {
            variation_tier: "Cor".into(),
            variations: (0..=MAX_VARIATIONS as u128)
                .map(|at| variation(at + 1, &format!("SKU-{at}"), &format!("Cor {at}"), 1))
                .collect(),
            ..saved
        };
        assert_eq!(
            checks(&many.checklist()),
            [(Check::TooManyVariations, true)]
        );
    });
}

#[test]
fn publishing_sends_the_listing_then_its_variations_each_a_listing_of_its_product() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;

        let published = fx.publish(&draft).await.unwrap();

        let sent = fx.publisher.published.lock().unwrap()[0].clone();
        let tier = VariationsToPublish {
            tier: "Cor".into(),
            options: vec![
                VariationToPublish {
                    option: "Preto".into(),
                    seller_sku: "CAM-PRE-M".into(),
                    available_quantity: 4,
                },
                VariationToPublish {
                    option: "Branco".into(),
                    seller_sku: "CAM-BRA-M".into(),
                    available_quantity: 2,
                },
                VariationToPublish {
                    option: "Azul".into(),
                    seller_sku: "CAM-AZU-M".into(),
                    available_quantity: 0,
                },
            ],
        };
        assert_eq!(sent.variations.as_ref(), Some(&tier));
        assert_eq!(sent.available_quantity, 6);
        assert_eq!(sent.seller_sku, "CAM-PRE-M");
        assert_eq!(sent.description, "Malha 30.1, gola careca.");
        assert_eq!(sent.price, brl("49.90"));
        let added = fx.publisher.added.lock().unwrap().clone();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].0, "MLB9000");
        assert_eq!(added[0].1, sent);

        assert_eq!(published.id, draft.id);
        assert_eq!(published.product, Some(RecordId::from_u128(1)));
        assert_eq!(
            published.listed.variation,
            Some(Variation {
                id: "MLB9000-1".into(),
                name: "Cor: Preto".into()
            })
        );
        assert_eq!(published.listed.available_quantity, 4);
        assert_eq!(published.listed.status, ListingStatus::Active);

        let all = fx.listings.listings().await.unwrap();
        let mut sold: Vec<_> = all
            .iter()
            .map(|listing| {
                (
                    listing.product,
                    listing.listed.id.clone(),
                    listing.listed.variation.clone().map(|v| v.name),
                    listing.listed.seller_sku.clone(),
                    listing.listed.available_quantity,
                    listing.listed.status,
                    listing.listed.price,
                )
            })
            .collect();
        sold.sort_by_key(|row| row.3.clone());
        let row = |product: u128, name: &str, sku: &str, units, status| {
            (
                Some(RecordId::from_u128(product)),
                "MLB9000".to_owned(),
                Some(name.to_owned()),
                Some(sku.to_owned()),
                units,
                status,
                brl("49.90"),
            )
        };
        assert_eq!(
            sold,
            [
                row(3, "Cor: Azul", "CAM-AZU-M", 0, ListingStatus::OutOfStock),
                row(2, "Cor: Branco", "CAM-BRA-M", 2, ListingStatus::Active),
                row(1, "Cor: Preto", "CAM-PRE-M", 4, ListingStatus::Active),
            ]
        );
        assert!(
            all.iter()
                .all(|listing| listing.channel == SalesChannelKind::Shopee)
        );
        assert!(fx.listings.drafts().await.unwrap().is_empty());

        // Publishing again sends nothing, and a Sync reads the same Listings.
        fx.publish(&draft).await.unwrap();
        assert_eq!(fx.publisher.publish_calls(), 1);
        assert_eq!(fx.publisher.added.lock().unwrap().len(), 1);
        fx.channel.has(
            sold.iter()
                .map(|(_, id, name, sku, units, status, price)| ChannelListing {
                    variation: Some(Variation {
                        id: format!(
                            "MLB9000-{}",
                            match sku.as_deref() {
                                Some("CAM-PRE-M") => 1,
                                Some("CAM-BRA-M") => 2,
                                _ => 3,
                            }
                        ),
                        name: name.clone().unwrap(),
                    }),
                    seller_sku: sku.clone(),
                    available_quantity: *units,
                    status: *status,
                    price: *price,
                    ..listing(id, "Camiseta Básica 100% Algodão")
                })
                .collect(),
        );
        let report = fx.listings.sync(&fx.channel).await.unwrap();
        assert_eq!(report.new, 0);
        assert_eq!(fx.listings.listings().await.unwrap().len(), 3);
    });
}

#[test]
fn variations_whose_answer_was_lost_are_all_the_next_publish_sends() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        fx.publisher.adds_variations(Publishing::CreatesUnanswered);

        let error = fx.publish(&draft).await.unwrap_err();

        assert!(matches!(
            error,
            ListingError::VariationsNotSent(id, PlatformError::Failed(_)) if id == draft.id
        ));
        let pending = fx.listings.drafts().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].variations_pending);
        assert_eq!(pending[0].published.as_deref(), Some("MLB9000"));
        assert_eq!(fx.listings.listings().await.unwrap().len(), 1);

        fx.publisher.adds_variations(Publishing::Creates);
        fx.publish(&draft).await.unwrap();

        assert_eq!(fx.publisher.publish_calls(), 1);
        assert_eq!(fx.publisher.added.lock().unwrap().len(), 2);
        assert_eq!(fx.listings.listings().await.unwrap().len(), 3);
        assert!(fx.listings.drafts().await.unwrap().is_empty());
    });
}

#[test]
fn a_sync_between_the_lost_answer_and_the_retry_leaves_one_listing_per_variation() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        fx.publisher.adds_variations(Publishing::CreatesUnanswered);
        fx.publish(&draft).await.unwrap_err();
        // Shopee already has the models, which a Sync brings in unlinked.
        fx.channel.has(
            [("CAM-PRE-M", "Preto", 1), ("CAM-BRA-M", "Branco", 2)]
                .into_iter()
                .map(|(sku, option, at)| ChannelListing {
                    variation: Some(Variation {
                        id: format!("MLB9000-{at}"),
                        name: format!("Cor: {option}"),
                    }),
                    seller_sku: Some(sku.into()),
                    ..listing("MLB9000", "Camiseta Básica 100% Algodão")
                })
                .collect(),
        );
        fx.listings.sync(&fx.channel).await.unwrap();

        fx.publish(&draft).await.unwrap();

        let all = fx.listings.listings().await.unwrap();
        let mut by_variation: Vec<_> = all
            .iter()
            .map(|listing| {
                (
                    listing.listed.variation.as_ref().map(|v| v.id.clone()),
                    listing.product,
                )
            })
            .collect();
        by_variation.sort();
        assert_eq!(
            by_variation,
            [
                (Some("MLB9000-1".into()), Some(RecordId::from_u128(1))),
                (Some("MLB9000-2".into()), Some(RecordId::from_u128(2))),
                (Some("MLB9000-3".into()), Some(RecordId::from_u128(3))),
            ]
        );
        // The draft's own variation is still the draft's Listing.
        assert!(all.iter().any(|listing| listing.id == draft.id
            && listing.listed.variation.as_ref().map(|v| v.id.as_str()) == Some("MLB9000-1")
            && listing.listed.status == ListingStatus::Active));
        let report = fx.listings.sync(&fx.channel).await.unwrap();
        assert_eq!(report.new, 0);
    });
}

#[test]
fn a_variation_the_channel_did_not_send_back_leaves_the_draft_pending() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        fx.publisher.variations.lock().unwrap().insert(
            "MLB9000".into(),
            vec![PublishedVariation {
                id: "MLB9000-1".into(),
                seller_sku: "CAM-PRE-M".into(),
                name: "Cor: Preto".into(),
                status: ListingStatus::Active,
            }],
        );

        let error = fx.publish(&draft).await.unwrap_err();

        assert!(matches!(error, ListingError::VariationsNotSent(_, _)));
        assert!(fx.listings.drafts().await.unwrap()[0].variations_pending);
        assert_eq!(fx.listings.listings().await.unwrap().len(), 1);
    });
}
