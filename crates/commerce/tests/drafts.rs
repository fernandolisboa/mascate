//! Listing drafts through commerce's public interface, with a real temporary
//! database and in-memory channels: a draft from a Product, its checklist,
//! the channel's validator, and publishing, which never lists a Product
//! twice.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    ChannelIssue, ChannelListing, Check, ChecklistItem, Condition, DraftAttribute, DraftEdit,
    DraftPicture, DraftStart, ListingDraft, ListingError, ListingStatus, Listings, Requirement,
    is_blocked, is_picture,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, ListingType, PlatformError};
use mascate_platform::{Database, migrate};

use common::{Channel, Publisher, Publishing, brl, category, listing, product, value};

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    listings: Listings,
    publisher: Publisher,
    channel: Channel,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
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
            _dir: dir,
            clock,
            publisher: Publisher::default(),
            channel: Channel::default(),
        }
    }

    /// A draft of the earphones, with four files in their folder.
    async fn draft(&self) -> ListingDraft {
        self.listings
            .create_draft(
                &product(7, "FON-TWS-001", "Fone Bluetooth TWS Lenovo"),
                DraftStart {
                    available_quantity: 4,
                    pictures: files(&["frente.jpg", "nota.pdf", "lado.PNG", "caixa.jpeg"]),
                },
                &self.publisher,
            )
            .await
            .unwrap()
    }

    /// The draft ready to publish: model, price, description and pictures.
    async fn ready(&self) -> ListingDraft {
        let draft = self.draft().await;
        self.clock.advance(TimeDelta::minutes(1));
        self.listings
            .save_draft(draft.id, ready_edit(&draft))
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

fn files(names: &[&str]) -> Vec<PathBuf> {
    names
        .iter()
        .map(|name| PathBuf::from(format!("/produtos/FON-TWS-001/{name}")))
        .collect()
}

fn edit(draft: &ListingDraft) -> DraftEdit {
    DraftEdit {
        title: draft.title.clone(),
        description: draft.description.clone(),
        listing_type: draft.listing_type,
        condition: draft.condition,
        price: draft.price,
        available_quantity: draft.available_quantity,
        warranty: draft.warranty.clone(),
        attributes: Vec::new(),
        pictures: draft
            .pictures
            .iter()
            .map(|picture| picture.path.clone())
            .collect(),
    }
}

fn ready_edit(draft: &ListingDraft) -> DraftEdit {
    DraftEdit {
        title: "Fone de Ouvido Bluetooth TWS Lenovo LP40".into(),
        description: "Fone sem fio com estojo de carga.\nBateria de 4 horas.".into(),
        price: brl("89.904"),
        warranty: "90 dias".into(),
        attributes: vec![value("MODEL", "LP40"), value("GTIN", "7891234567895")],
        ..edit(draft)
    }
}

fn checks(checklist: &[ChecklistItem]) -> Vec<(Check, bool)> {
    checklist
        .iter()
        .map(|item| (item.check.clone(), item.blocks))
        .collect()
}

#[test]
fn a_draft_starts_from_the_product_in_the_predicted_category_with_its_attributes() {
    block_on(async {
        let fx = Fixture::new().await;

        let draft = fx.draft().await;

        assert_eq!(draft.title, "Fone Bluetooth TWS Lenovo");
        assert_eq!(draft.seller_sku, "FON-TWS-001");
        assert_eq!(draft.product, mascate_kernel::RecordId::from_u128(7));
        assert_eq!(
            draft.category,
            Some(category("MLB196208", "Fones de Ouvido"))
        );
        assert_eq!(draft.listing_type, ListingType::Classic);
        assert_eq!(draft.condition, Condition::New);
        assert_eq!(draft.price, brl("0"));
        assert_eq!(draft.available_quantity, 4);
        assert_eq!(draft.description, "");
        assert_eq!(draft.published, None);
        // Required and recommended ones always; an optional one only with
        // the value the channel read in the name.
        let attribute = |id: &str, name: &str, requirement, value: &str| DraftAttribute {
            id: id.into(),
            name: name.into(),
            requirement,
            value: value.into(),
        };
        assert_eq!(
            draft.attributes,
            [
                attribute("BRAND", "Marca", Requirement::Required, "Lenovo"),
                attribute("MODEL", "Modelo", Requirement::Required, ""),
                attribute(
                    "GTIN",
                    "Código universal de produto",
                    Requirement::Recommended,
                    ""
                ),
                attribute("COLOR", "Cor", Requirement::Optional, "Preto"),
            ]
        );
        let pictures: Vec<_> = draft.pictures.iter().map(|p| p.path.clone()).collect();
        assert_eq!(pictures, files(&["frente.jpg", "lado.PNG", "caixa.jpeg"]));
        assert_eq!(fx.listings.drafts().await.unwrap(), [draft]);
    });
}

#[test]
fn only_jpeg_and_png_files_are_pictures() {
    for (name, picture) in [
        ("foto.jpg", true),
        ("foto.JPEG", true),
        ("foto.png", true),
        ("nota.pdf", false),
        ("print.webp", false),
        ("sem-extensao", false),
    ] {
        assert_eq!(is_picture(&PathBuf::from(name)), picture, "{name}");
    }
}

#[test]
fn a_draft_is_no_listing_in_the_channel_and_a_sync_leaves_it_alone() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.draft().await;
        fx.channel.has(vec![listing("MLB1", "Fone Bluetooth TWS")]);

        let report = fx.listings.sync(&fx.channel).await.unwrap();

        assert_eq!(report.new, 1);
        assert_eq!(report.gone, 0);
        let all = fx.listings.listings().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].listed.id, "MLB1");
        assert!(fx.listings.to_link(&[]).await.unwrap().len() == 1);
        assert_eq!(fx.listings.draft(draft.id).await.unwrap(), draft);
    });
}

#[test]
fn without_a_prediction_the_draft_waits_for_a_category() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.publisher.predictions.lock().unwrap().clear();

        let draft = fx.draft().await;

        assert_eq!(draft.category, None);
        assert!(draft.attributes.is_empty());
        assert!(draft.checklist().contains(&ChecklistItem {
            check: Check::Category,
            blocks: true
        }));
    });
}

#[test]
fn the_checklist_blocks_on_what_is_missing_and_only_warns_on_the_rest() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.draft().await;
        let blank = fx
            .listings
            .save_draft(
                draft.id,
                DraftEdit {
                    title: "  ".into(),
                    available_quantity: 0,
                    pictures: files(&["frente.jpg", "lado.PNG"]),
                    ..edit(&draft)
                },
            )
            .await
            .unwrap();

        assert_eq!(
            checks(&blank.checklist()),
            [
                (Check::Title, true),
                (Check::Price, true),
                (
                    Check::Attribute {
                        id: "MODEL".into(),
                        name: "Modelo".into()
                    },
                    true
                ),
                (Check::Pictures { selected: 2 }, true),
                (
                    Check::Attribute {
                        id: "GTIN".into(),
                        name: "Código universal de produto".into()
                    },
                    false
                ),
                (Check::Description, false),
                (Check::Stock, false),
            ]
        );
        assert!(is_blocked(&blank.checklist()));

        let ready = fx.ready().await;
        assert_eq!(ready.checklist(), []);

        let without_gtin = fx
            .listings
            .save_draft(
                ready.id,
                DraftEdit {
                    attributes: vec![value("GTIN", "")],
                    description: String::new(),
                    ..edit(&ready)
                },
            )
            .await
            .unwrap();
        let warnings = without_gtin.checklist();
        assert_eq!(warnings.len(), 2);
        assert!(!is_blocked(&warnings));
    });
}

#[test]
fn saving_keeps_the_edits_and_the_ids_of_pictures_already_uploaded() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.draft().await;
        fx.listings
            .validate_draft(draft.id, &fx.publisher)
            .await
            .unwrap();
        fx.clock.advance(TimeDelta::minutes(1));

        let saved = fx
            .listings
            .save_draft(
                draft.id,
                DraftEdit {
                    title: " Fone TWS Lenovo LP40 ".into(),
                    listing_type: ListingType::Premium,
                    condition: Condition::Used,
                    price: brl("99.90"),
                    available_quantity: 2,
                    warranty: "3 meses".into(),
                    attributes: vec![value("MODEL", " LP40 "), value("UNKNOWN", "x")],
                    pictures: files(&["caixa.jpeg", "nova.png", "frente.jpg", "nota.pdf"]),
                    ..edit(&draft)
                },
            )
            .await
            .unwrap();

        assert_eq!(saved.title, "Fone TWS Lenovo LP40");
        assert_eq!(saved.listing_type, ListingType::Premium);
        assert_eq!(saved.condition, Condition::Used);
        assert_eq!(saved.price, brl("99.90"));
        assert_eq!(saved.available_quantity, 2);
        assert_eq!(saved.warranty, "3 meses");
        assert_eq!(saved.attributes[1].value, "LP40");
        assert_eq!(saved.attributes.len(), 4);
        assert_eq!(
            saved.pictures,
            [
                DraftPicture {
                    path: files(&["caixa.jpeg"])[0].clone(),
                    uploaded: Some("PIC-3".into())
                },
                DraftPicture {
                    path: files(&["nova.png"])[0].clone(),
                    uploaded: None
                },
                DraftPicture {
                    path: files(&["frente.jpg"])[0].clone(),
                    uploaded: Some("PIC-1".into())
                },
            ]
        );
        assert_eq!(saved.updated_at, fx.clock.now());
        assert_eq!(fx.listings.draft(draft.id).await.unwrap(), saved);
    });
}

#[test]
fn a_price_in_another_currency_or_below_zero_is_refused() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.draft().await;
        let negative = DraftEdit {
            price: brl("-1"),
            ..edit(&draft)
        };
        assert!(matches!(
            fx.listings.save_draft(draft.id, negative).await,
            Err(ListingError::InvalidPrice)
        ));
        let dollars = DraftEdit {
            price: mascate_kernel::Money::new(10.into(), mascate_kernel::Currency::Usd),
            ..edit(&draft)
        };
        assert!(matches!(
            fx.listings.save_draft(draft.id, dollars).await,
            Err(ListingError::InvalidPrice)
        ));
    });
}

#[test]
fn another_category_brings_its_attributes_and_keeps_the_values_both_share() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        fx.publisher.attributes.lock().unwrap().insert(
            "MLB3697".into(),
            vec![
                common::attribute("BRAND", "Marca", Requirement::Required),
                common::attribute("LINE", "Linha", Requirement::Required),
                common::attribute("MODEL", "Modelo", Requirement::Optional),
            ],
        );

        let moved = fx
            .listings
            .choose_category(
                draft.id,
                category("MLB3697", "Fones"),
                &[value("LINE", "LivePods")],
                &fx.publisher,
            )
            .await
            .unwrap();

        assert_eq!(moved.category, Some(category("MLB3697", "Fones")));
        let values: Vec<_> = moved
            .attributes
            .iter()
            .map(|a| (a.id.as_str(), a.value.as_str()))
            .collect();
        assert_eq!(
            values,
            [("BRAND", "Lenovo"), ("LINE", "LivePods"), ("MODEL", "LP40")]
        );
    });
}

#[test]
fn validating_uploads_the_pictures_once_and_adds_what_the_channel_says() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        *fx.publisher.issues.lock().unwrap() = vec![
            ChannelIssue {
                message: "The title is too long".into(),
                blocks: true,
            },
            ChannelIssue {
                message: "Add more pictures".into(),
                blocks: false,
            },
        ];

        let checklist = fx
            .listings
            .validate_draft(draft.id, &fx.publisher)
            .await
            .unwrap();
        fx.listings
            .validate_draft(draft.id, &fx.publisher)
            .await
            .unwrap();

        assert_eq!(
            checks(&checklist),
            [
                (Check::Channel("The title is too long".into()), true),
                (Check::Channel("Add more pictures".into()), false),
            ]
        );
        assert_eq!(fx.publisher.uploaded.lock().unwrap().len(), 3);
        assert_eq!(fx.publisher.publish_calls(), 0);
        let sent = fx.publisher.validated.lock().unwrap()[0].clone();
        assert_eq!(sent.pictures, ["PIC-1", "PIC-2", "PIC-3"]);
    });
}

#[test]
fn publishing_waits_while_the_checklist_blocks_and_sends_nothing() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.draft().await;

        let refused = fx.publish(&draft).await;

        match refused {
            Err(ListingError::Blocked(checklist)) => assert!(is_blocked(&checklist)),
            other => panic!("expected Blocked, got {other:?}"),
        }
        assert!(fx.publisher.uploaded.lock().unwrap().is_empty());
        assert!(fx.publisher.validated.lock().unwrap().is_empty());
        assert_eq!(fx.publisher.publish_calls(), 0);
    });
}

#[test]
fn publishing_waits_while_the_channels_validator_blocks_but_not_for_its_warnings() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        *fx.publisher.issues.lock().unwrap() = vec![ChannelIssue {
            message: "Category requires the attribute LINE".into(),
            blocks: true,
        }];

        let refused = fx.publish(&draft).await;

        match refused {
            Err(ListingError::Blocked(checklist)) => assert_eq!(
                checks(&checklist),
                [(
                    Check::Channel("Category requires the attribute LINE".into()),
                    true
                )]
            ),
            other => panic!("expected Blocked, got {other:?}"),
        }
        assert_eq!(fx.publisher.publish_calls(), 0);

        *fx.publisher.issues.lock().unwrap() = vec![ChannelIssue {
            message: "Add a video".into(),
            blocks: false,
        }];
        fx.publish(&draft).await.unwrap();
        assert_eq!(fx.publisher.publish_calls(), 1);
    });
}

#[test]
fn publishing_creates_the_listing_and_the_listing_is_published_with_its_product() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;

        let listing = fx.publish(&draft).await.unwrap();

        let sent = fx.publisher.published.lock().unwrap()[0].clone();
        assert_eq!(sent.title, "Fone de Ouvido Bluetooth TWS Lenovo LP40");
        assert_eq!(sent.category, "MLB196208");
        assert_eq!(sent.listing_type, ListingType::Classic);
        assert_eq!(sent.condition, Condition::New);
        assert_eq!(sent.price, brl("89.90"));
        assert_eq!(sent.available_quantity, 4);
        assert_eq!(sent.seller_sku, "FON-TWS-001");
        assert_eq!(sent.warranty.as_deref(), Some("90 dias"));
        assert_eq!(
            sent.attributes,
            [
                value("BRAND", "Lenovo"),
                value("MODEL", "LP40"),
                value("GTIN", "7891234567895"),
                value("COLOR", "Preto"),
            ]
        );
        assert_eq!(sent.pictures, ["PIC-1", "PIC-2", "PIC-3"]);
        assert_eq!(
            *fx.publisher.described.lock().unwrap(),
            [(
                "MLB9000".to_owned(),
                "Fone sem fio com estojo de carga.\nBateria de 4 horas.".to_owned()
            )]
        );

        assert_eq!(listing.id, draft.id);
        assert_eq!(listing.product, Some(draft.product));
        assert_eq!(listing.listed.id, "MLB9000");
        assert_eq!(listing.listed.status, ListingStatus::Active);
        assert_eq!(listing.listed.price, brl("89.90"));
        assert_eq!(listing.listed.seller_sku.as_deref(), Some("FON-TWS-001"));
        assert_eq!(listing.synced_at, fx.clock.now());
        assert_eq!(fx.listings.listings().await.unwrap(), [listing]);
        assert!(fx.listings.drafts().await.unwrap().is_empty());
    });
}

#[test]
fn a_published_draft_no_longer_changes_and_publishing_it_again_creates_nothing() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        let first = fx.publish(&draft).await.unwrap();

        let again = fx.publish(&draft).await.unwrap();

        assert_eq!(again.listed, first.listed);
        assert_eq!(fx.publisher.publish_calls(), 1);
        assert_eq!(fx.publisher.described.lock().unwrap().len(), 1);
        assert!(matches!(
            fx.listings.save_draft(draft.id, edit(&draft)).await,
            Err(ListingError::Published(_))
        ));
        assert!(matches!(
            fx.listings.discard_draft(draft.id).await,
            Err(ListingError::Published(_))
        ));
        assert_eq!(fx.listings.listings().await.unwrap().len(), 1);
    });
}

#[test]
fn a_description_the_channel_refuses_is_all_the_next_publish_sends() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        *fx.publisher.describing.lock().unwrap() = Some(PlatformError::RateLimited);

        let failed = fx.publish(&draft).await;

        assert!(matches!(
            failed,
            Err(ListingError::DescriptionNotSent(id, PlatformError::RateLimited)) if id == draft.id
        ));
        let pending = fx.listings.drafts().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].published.as_deref(), Some("MLB9000"));
        assert!(pending[0].description_pending);
        assert_eq!(fx.listings.listings().await.unwrap().len(), 1);

        *fx.publisher.describing.lock().unwrap() = None;
        fx.publish(&draft).await.unwrap();

        assert_eq!(fx.publisher.publish_calls(), 1);
        assert_eq!(fx.publisher.described.lock().unwrap().len(), 1);
        assert!(fx.listings.drafts().await.unwrap().is_empty());
    });
}

#[test]
fn a_draft_without_a_description_is_done_once_the_listing_is_created() {
    block_on(async {
        let fx = Fixture::new().await;
        let ready = fx.ready().await;
        let draft = fx
            .listings
            .save_draft(
                ready.id,
                DraftEdit {
                    description: String::new(),
                    ..edit(&ready)
                },
            )
            .await
            .unwrap();

        fx.publish(&draft).await.unwrap();

        assert!(fx.publisher.described.lock().unwrap().is_empty());
        assert!(fx.listings.drafts().await.unwrap().is_empty());
    });
}

#[test]
fn an_attempt_left_without_an_answer_is_found_by_its_seller_sku_next_time() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        fx.publisher.publishes(Publishing::CreatesUnanswered);

        assert!(matches!(
            fx.publish(&draft).await,
            Err(ListingError::Platform(PlatformError::Failed(_)))
        ));
        fx.publisher.publishes(Publishing::Creates);
        let listing = fx.publish(&draft).await.unwrap();

        assert_eq!(fx.publisher.publish_calls(), 1);
        assert_eq!(*fx.publisher.searched.lock().unwrap(), ["FON-TWS-001"]);
        assert_eq!(listing.listed.id, "MLB9000");
        assert_eq!(listing.product, Some(draft.product));
        assert_eq!(fx.publisher.described.lock().unwrap().len(), 1);
    });
}

#[test]
fn a_lost_attempt_a_sync_brought_in_meanwhile_becomes_the_drafts_listing() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        fx.publisher.publishes(Publishing::CreatesUnanswered);
        fx.publish(&draft).await.unwrap_err();
        fx.channel.has(vec![ChannelListing {
            seller_sku: Some("FON-TWS-001".into()),
            ..listing("MLB9000", "Fone de Ouvido Bluetooth TWS Lenovo LP40")
        }]);
        fx.listings.sync(&fx.channel).await.unwrap();
        assert_eq!(fx.listings.to_link(&[]).await.unwrap().len(), 1);

        fx.publisher.publishes(Publishing::Creates);
        let listing = fx.publish(&draft).await.unwrap();

        assert_eq!(fx.publisher.publish_calls(), 1);
        let all = fx.listings.listings().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, draft.id);
        assert_eq!(listing.product, Some(draft.product));
        assert!(fx.listings.to_link(&[]).await.unwrap().is_empty());

        // The next Sync keeps reading it as this Listing.
        let report = fx.listings.sync(&fx.channel).await.unwrap();
        assert_eq!(report.new, 0);
        assert_eq!(fx.listings.listings().await.unwrap()[0].id, draft.id);
    });
}

#[test]
fn a_listing_of_the_same_product_the_app_already_knows_is_not_taken_for_a_lost_one() {
    block_on(async {
        let fx = Fixture::new().await;
        let first = fx.ready().await;
        fx.publish(&first).await.unwrap();
        let second = fx.ready().await;
        fx.publisher.publishes(Publishing::CreatesUnanswered);
        fx.publish(&second).await.unwrap_err();
        // Pretend the lost one never reached the channel.
        fx.publisher.listings.lock().unwrap().pop();
        fx.publisher.publishes(Publishing::Creates);

        let listing = fx.publish(&second).await.unwrap();

        assert_eq!(fx.publisher.publish_calls(), 3);
        assert_ne!(listing.listed.id, "MLB9000");
        assert_eq!(fx.listings.listings().await.unwrap().len(), 2);
    });
}

#[test]
fn a_refused_attempt_created_nothing_so_the_next_one_publishes_without_looking() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        fx.publisher
            .publishes(Publishing::Fails(PlatformError::Refused("price".into())));

        assert!(matches!(
            fx.publish(&draft).await,
            Err(ListingError::Platform(PlatformError::Refused(_)))
        ));
        fx.publisher.publishes(Publishing::Creates);
        fx.publish(&draft).await.unwrap();

        assert_eq!(fx.publisher.publish_calls(), 2);
        assert!(fx.publisher.searched.lock().unwrap().is_empty());
    });
}

#[test]
fn a_draft_thrown_away_is_gone() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.draft().await;
        let kept = fx.draft().await;

        fx.listings.discard_draft(draft.id).await.unwrap();

        assert_eq!(fx.listings.drafts().await.unwrap(), [kept]);
        assert!(matches!(
            fx.listings.draft(draft.id).await,
            Err(ListingError::UnknownDraft(_))
        ));
        assert!(matches!(
            fx.listings.publish(draft.id, &fx.publisher).await,
            Err(ListingError::UnknownDraft(_))
        ));
    });
}

#[test]
fn without_the_channel_no_draft_is_made() {
    block_on(async {
        let fx = Fixture::new().await;
        *fx.publisher.failure.lock().unwrap() = Some(PlatformError::NotConnected);

        let refused = fx
            .listings
            .create_draft(
                &product(7, "FON-TWS-001", "Fone"),
                DraftStart {
                    available_quantity: 0,
                    pictures: Vec::new(),
                },
                &fx.publisher,
            )
            .await;

        assert!(matches!(
            refused,
            Err(ListingError::Platform(PlatformError::NotConnected))
        ));
        assert!(fx.listings.drafts().await.unwrap().is_empty());
    });
}
