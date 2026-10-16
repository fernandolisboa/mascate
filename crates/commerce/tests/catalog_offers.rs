//! Drafts that offer a product the channel's catalog already has (#128,
//! Amazon), through commerce's public interface with a real temporary
//! database and an in-memory channel: the GTIN finds the product, the owner
//! picks one when several come back, the checklist asks only what an offer
//! needs, and publishing never lists the Product twice.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    CatalogMatch, ChannelIssue, Check, ChecklistItem, Condition, DraftEdit, DraftKind, DraftStart,
    Gtin, ListingDraft, ListingError, ListingStatus, Listings,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{PlatformError, SalesChannelKind};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;

use common::{Publisher, Publishing, brl, catalog_item, product};

const ONE: &str = "7891234567895";
const SEVERAL: &str = "7891000000014";
const NONE: &str = "012345678905";

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    listings: Listings,
    amazon: Publisher,
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
            _dir: dir,
            clock,
            amazon: Publisher::amazon(),
        }
    }

    /// A draft on Amazon of the earphones, with a picture in their folder.
    async fn draft(&self) -> ListingDraft {
        let picture = self._dir.path().join("frente.jpg");
        std::fs::write(&picture, b"jpg").unwrap();
        self.listings
            .create_draft(
                &product(7, "FON-TWS-001", "Fone Bluetooth TWS Lenovo"),
                DraftStart {
                    available_quantity: 4,
                    pictures: vec![picture],
                },
                &self.amazon,
            )
            .await
            .unwrap()
    }

    async fn matched(&self, draft: &ListingDraft, gtin: &str) -> CatalogMatch {
        self.clock.advance(TimeDelta::minutes(1));
        self.listings
            .match_catalog_item(draft.id, gtin, &self.amazon)
            .await
            .unwrap()
    }

    /// The draft offering the earphones, at the price the owner confirms.
    async fn ready(&self) -> ListingDraft {
        let draft = self.draft().await;
        let CatalogMatch::Offered(offered) = self.matched(&draft, ONE).await else {
            panic!("one product is offered at once");
        };
        self.clock.advance(TimeDelta::minutes(1));
        self.listings
            .save_draft(
                offered.id,
                DraftEdit {
                    price: brl("129.90"),
                    ..edit(&offered)
                },
            )
            .await
            .unwrap()
    }
}

fn edit(draft: &ListingDraft) -> DraftEdit {
    DraftEdit {
        variation_tier: draft.variation_tier.clone(),
        variations: draft.variations.clone(),
        title: draft.title.clone(),
        description: draft.description.clone(),
        listing_type: draft.listing_type,
        condition: draft.condition,
        price: draft.price,
        available_quantity: draft.available_quantity,
        warranty: draft.warranty.clone(),
        attributes: Vec::new(),
        pictures: draft.pictures.iter().map(|p| p.path.clone()).collect(),
    }
}

fn checks(checklist: &[ChecklistItem]) -> Vec<(Check, bool)> {
    checklist
        .iter()
        .map(|item| (item.check.clone(), item.blocks))
        .collect()
}

fn asin(draft: &ListingDraft) -> Option<String> {
    draft
        .attributes
        .iter()
        .find(|attribute| attribute.id == "merchant_suggested_asin")
        .map(|attribute| attribute.value.clone())
}

#[test]
fn an_amazon_draft_is_an_offer_without_a_product_pictures_or_category_yet() {
    block_on(async {
        let fx = Fixture::new().await;

        let draft = fx.draft().await;

        assert_eq!(draft.channel, SalesChannelKind::Amazon);
        assert_eq!(draft.kind(), DraftKind::CatalogOffer);
        assert_eq!(draft.category, None);
        assert_eq!(draft.attributes, []);
        // The product's pictures are Amazon's.
        assert_eq!(draft.pictures, []);
        assert_eq!(fx.amazon.predictions.lock().unwrap().len(), 0);
        // No pictures or description asked: only the product and the price.
        assert_eq!(
            checks(&draft.checklist()),
            [(Check::CatalogItem, true), (Check::Price, true)]
        );
    });
}

#[test]
fn a_gtin_with_one_product_makes_the_draft_offer_it_at_once() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.draft().await;

        let matched = fx.matched(&draft, ONE).await;

        let CatalogMatch::Offered(offered) = matched else {
            panic!("expected the draft to offer the product: {matched:?}");
        };
        assert_eq!(offered.id, draft.id);
        assert_eq!(offered.title, "Fone Bluetooth TWS Lenovo LP40");
        assert_eq!(
            offered
                .category
                .as_ref()
                .map(|category| category.id.as_str()),
            Some("HEADPHONES")
        );
        assert_eq!(asin(&offered).as_deref(), Some("B0CFONE001"));
        assert_eq!(checks(&offered.checklist()), [(Check::Price, true)]);
        assert_eq!(fx.listings.draft(draft.id).await.unwrap(), *offered);
    });
}

#[test]
fn a_gtin_with_several_products_waits_for_the_owner_to_pick_one() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.draft().await;

        let matched = fx.matched(&draft, SEVERAL).await;

        assert_eq!(
            matched,
            CatalogMatch::Several(vec![
                catalog_item("B0CCANECA1", "Caneca 350 ml", "DRINKING_CUP"),
                catalog_item("B0CCANECA2", "Kit 2 Canecas 350 ml", "DRINKING_CUP"),
            ])
        );
        assert_eq!(fx.listings.draft(draft.id).await.unwrap(), draft);

        let picked = fx
            .listings
            .offer_catalog_item(
                draft.id,
                catalog_item("B0CCANECA2", "Kit 2 Canecas 350 ml", "DRINKING_CUP"),
                &fx.amazon,
            )
            .await
            .unwrap();
        assert_eq!(picked.title, "Kit 2 Canecas 350 ml");
        assert_eq!(asin(&picked).as_deref(), Some("B0CCANECA2"));
    });
}

#[test]
fn a_gtin_with_no_product_changes_nothing() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.draft().await;

        assert_eq!(fx.matched(&draft, NONE).await, CatalogMatch::NotFound);

        assert_eq!(fx.listings.draft(draft.id).await.unwrap(), draft);
        assert_eq!(*fx.amazon.looked_up.lock().unwrap(), [NONE]);
    });
}

#[test]
fn another_product_replaces_the_one_the_draft_offered() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;

        let moved = fx
            .listings
            .offer_catalog_item(
                draft.id,
                catalog_item("B0CCANECA1", "Caneca 350 ml", "DRINKING_CUP"),
                &fx.amazon,
            )
            .await
            .unwrap();

        assert_eq!(asin(&moved).as_deref(), Some("B0CCANECA1"));
        assert_eq!(moved.attributes.len(), 1);
        // What the owner confirmed stays.
        assert_eq!(moved.price, brl("129.90"));
        assert_eq!(moved.available_quantity, 4);
    });
}

#[test]
fn a_mistyped_gtin_is_refused_before_asking_the_channel() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.draft().await;

        for typed in ["7891234567890", "789123456789", "7891234567895x", ""] {
            let refused = fx
                .listings
                .match_catalog_item(draft.id, typed, &fx.amazon)
                .await;
            assert!(
                matches!(refused, Err(ListingError::InvalidGtin)),
                "{typed}: {refused:?}"
            );
        }
        assert_eq!(fx.amazon.looked_up.lock().unwrap().len(), 0);
    });
}

#[test]
fn a_page_of_its_own_is_not_looked_up_in_a_catalog() {
    block_on(async {
        let fx = Fixture::new().await;
        let mercado_livre = Publisher::default();
        let draft = fx
            .listings
            .create_draft(
                &product(7, "FON-TWS-001", "Fone Bluetooth TWS Lenovo"),
                DraftStart {
                    available_quantity: 4,
                    pictures: Vec::new(),
                },
                &mercado_livre,
            )
            .await
            .unwrap();

        let refused = fx
            .listings
            .match_catalog_item(draft.id, ONE, &mercado_livre)
            .await;

        assert!(matches!(refused, Err(ListingError::NotACatalogOffer(id)) if id == draft.id));
        // Nor is an Amazon draft looked up through another channel.
        let amazon_draft = fx.draft().await;
        assert!(matches!(
            fx.listings
                .match_catalog_item(amazon_draft.id, ONE, &mercado_livre)
                .await,
            Err(ListingError::OtherChannel { .. })
        ));
    });
}

#[test]
fn an_offer_keeps_no_pictures_even_when_the_owner_sends_some() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;

        let saved = fx
            .listings
            .save_draft(
                draft.id,
                DraftEdit {
                    pictures: vec![PathBuf::from("frente.jpg")],
                    ..edit(&draft)
                },
            )
            .await
            .unwrap();

        assert_eq!(saved.pictures, []);
    });
}

#[test]
fn publishing_sends_the_asin_condition_price_and_units_and_nothing_else() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        fx.clock.advance(TimeDelta::minutes(1));

        let listing = fx.listings.publish(draft.id, &fx.amazon).await.unwrap();

        assert_eq!(listing.listed.id, "FON-TWS-001");
        assert_eq!(listing.product, Some(draft.product));
        let published = fx.amazon.published.lock().unwrap().clone();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].category, "HEADPHONES");
        assert_eq!(published[0].condition, Condition::New);
        assert_eq!(published[0].price, brl("129.90"));
        assert_eq!(published[0].available_quantity, 4);
        assert_eq!(
            published[0].attributes,
            [common::value("merchant_suggested_asin", "B0CFONE001")]
        );
        assert_eq!(published[0].pictures, Vec::<String>::new());
        assert_eq!(fx.amazon.uploaded.lock().unwrap().len(), 0);
        assert_eq!(fx.amazon.described.lock().unwrap().len(), 0);
        assert_eq!(fx.listings.drafts().await.unwrap(), []);
    });
}

#[test]
fn the_channels_issues_are_checklist_errors_that_hold_the_offer_back() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        *fx.amazon.issues.lock().unwrap() = vec![
            ChannelIssue {
                message: "Você não tem aprovação para vender nesta marca.".into(),
                blocks: true,
            },
            ChannelIssue {
                message: "O preço não é competitivo.".into(),
                blocks: false,
            },
        ];

        let checklist = fx
            .listings
            .validate_draft(draft.id, &fx.amazon)
            .await
            .unwrap();
        let refused = fx.listings.publish(draft.id, &fx.amazon).await;

        let expected = [
            (
                Check::Channel("Você não tem aprovação para vender nesta marca.".into()),
                true,
            ),
            (Check::Channel("O preço não é competitivo.".into()), false),
        ];
        assert_eq!(checks(&checklist), expected);
        let Err(ListingError::Blocked(blocked)) = refused else {
            panic!("expected the checklist to hold it back: {refused:?}");
        };
        assert_eq!(checks(&blocked), expected);
        assert_eq!(fx.amazon.publish_calls(), 0);
    });
}

#[test]
fn a_lost_answer_and_a_retry_adopt_the_offer_the_first_attempt_created() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        fx.amazon.publishes(Publishing::CreatesUnanswered);
        fx.clock.advance(TimeDelta::minutes(1));

        let lost = fx.listings.publish(draft.id, &fx.amazon).await;
        assert!(matches!(
            lost,
            Err(ListingError::Platform(PlatformError::Failed(_)))
        ));

        fx.amazon.publishes(Publishing::Creates);
        fx.clock.advance(TimeDelta::minutes(5));
        let listing = fx.listings.publish(draft.id, &fx.amazon).await.unwrap();

        assert_eq!(listing.listed.id, "FON-TWS-001");
        assert_eq!(listing.listed.status, ListingStatus::Active);
        assert_eq!(fx.amazon.publish_calls(), 1);
        assert_eq!(*fx.amazon.searched.lock().unwrap(), ["FON-TWS-001"]);
    });
}

#[test]
fn a_published_offer_no_longer_changes_its_product() {
    block_on(async {
        let fx = Fixture::new().await;
        let draft = fx.ready().await;
        fx.listings.publish(draft.id, &fx.amazon).await.unwrap();

        let refused = fx
            .listings
            .offer_catalog_item(
                draft.id,
                catalog_item("B0CCANECA1", "Caneca 350 ml", "DRINKING_CUP"),
                &fx.amazon,
            )
            .await;

        assert!(matches!(refused, Err(ListingError::Published(_))));
    });
}

#[test]
fn gtins_of_each_length_with_the_right_check_digit_are_read() {
    for digits in [
        "78912342",
        "012345678905",
        "7891234567895",
        "17891234567892",
    ] {
        assert_eq!(Gtin::parse(digits).unwrap().digits(), digits);
    }
    assert_eq!(
        Gtin::parse("  7891234567895 ").unwrap().digits(),
        "7891234567895"
    );
    for wrong in [
        "7891234567894",
        "789123456",
        "78912345678950",
        "7891234-67895",
    ] {
        assert_eq!(Gtin::parse(wrong), None, "{wrong}");
    }
}

/// The GS1 check digit of `digits`, computed independently: from the
/// right, weights 3 and 1 alternate.
fn check_digit(digits: &[u8]) -> u8 {
    let sum: u32 = digits
        .iter()
        .rev()
        .zip([3u32, 1].iter().cycle())
        .map(|(digit, weight)| u32::from(*digit) * weight)
        .sum();
    ((10 - sum % 10) % 10) as u8
}

proptest! {
    #[test]
    fn a_barcode_is_a_gtin_only_with_its_own_check_digit(
        body in prop::sample::select(vec![7usize, 11, 12, 13])
            .prop_flat_map(|length| prop::collection::vec(0u8..10, length)),
        wrong in 1u8..10,
    ) {
        let right = check_digit(&body);
        let text = |check: u8| {
            body.iter().chain([check].iter()).map(|digit| char::from(b'0' + digit)).collect::<String>()
        };
        prop_assert!(Gtin::parse(&text(right)).is_some());
        prop_assert_eq!(Gtin::parse(&text((right + wrong) % 10)), None);
    }
}
