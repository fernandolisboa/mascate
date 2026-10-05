//! Listings synced from a Sales Channel and linked to Products, through
//! commerce's public interface with a real temporary database and an
//! in-memory Sales Channel.

mod common;

use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    ChannelListing, Listing, ListingError, ListingStatus, ListingSync, Listings, SuggestedBy,
    Variation,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Currency, ListingType, Money, PlatformError, RecordId};

use common::{Channel, brl, listing, product, variation};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    listings: Listings,
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
        let listings = Listings::new(database, clock.clone(), Arc::new(SequentialIds::default()));
        Self {
            _dir: dir,
            clock,
            listings,
            channel: Channel::default(),
        }
    }

    async fn sync(&self) -> ListingSync {
        self.clock.advance(TimeDelta::hours(1));
        self.listings.sync(&self.channel).await.unwrap()
    }

    async fn all(&self) -> Vec<Listing> {
        self.listings.listings().await.unwrap()
    }

    async fn find(&self, id: &str, variation: Option<&str>) -> Listing {
        self.all()
            .await
            .into_iter()
            .find(|listing| {
                listing.listed.id == id
                    && listing.listed.variation.as_ref().map(|v| v.id.as_str()) == variation
            })
            .unwrap_or_else(|| panic!("no Listing {id} {variation:?}"))
    }
}

#[test]
fn a_sync_imports_active_and_paused_listings_with_their_details() {
    block_on(async {
        let fx = Fixture::new().await;
        let paused = ChannelListing {
            status: ListingStatus::Paused,
            available_quantity: 0,
            seller_sku: Some("LUM-LED-001".into()),
            listing_type: Some(ListingType::Premium),
            ..listing("MLB2", "Luminária LED de Mesa")
        };
        fx.channel
            .has(vec![listing("MLB1", "Fone Bluetooth TWS"), paused.clone()]);

        let report = fx.sync().await;

        assert_eq!(
            report,
            ListingSync {
                read: 2,
                new: 2,
                changed: 0,
                gone: 0
            }
        );
        let all = fx.all().await;
        let listed: Vec<_> = all.iter().map(|listing| listing.listed.clone()).collect();
        assert_eq!(listed, [listing("MLB1", "Fone Bluetooth TWS"), paused]);
        assert!(all.iter().all(|listing| listing.product.is_none()));
        assert_eq!(fx.listings.last_sync().await.unwrap(), Some(fx.clock.now()));
    });
}

#[test]
fn syncing_the_same_answers_again_changes_nothing() {
    block_on(async {
        let fx = Fixture::new().await;
        let fone = listing("MLB1", "Fone Bluetooth TWS");
        fx.channel.has(vec![
            fone.clone(),
            variation(&fone, "11", "Cor: Preto"),
            listing("MLB2", "Luminária LED"),
        ]);
        fx.sync().await;
        let before = fx.all().await;

        let report = fx.sync().await;
        let after = fx.all().await;

        assert_eq!(report.new, 0);
        assert_eq!(report.changed, 0);
        assert_eq!(report.gone, 0);
        assert_eq!(after.len(), before.len());
        for (before, after) in before.iter().zip(&after) {
            assert_eq!(before.id, after.id);
            assert_eq!(before.listed, after.listed);
            assert!(after.synced_at > before.synced_at);
        }
    });
}

#[test]
fn a_new_price_or_status_in_the_channel_shows_after_the_next_sync() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel.has(vec![
            listing("MLB1", "Fone Bluetooth TWS"),
            listing("MLB2", "Luminária LED"),
        ]);
        fx.sync().await;
        fx.channel.change("MLB1", |listing| {
            listing.price = brl("79.90");
            listing.available_quantity = 2;
        });
        fx.channel
            .change("MLB2", |listing| listing.status = ListingStatus::Paused);

        let report = fx.sync().await;

        assert_eq!(report.changed, 2);
        let fone = fx.find("MLB1", None).await;
        assert_eq!(fone.listed.price, brl("79.90"));
        assert_eq!(fone.listed.available_quantity, 2);
        assert_eq!(
            fx.find("MLB2", None).await.listed.status,
            ListingStatus::Paused
        );
    });
}

#[test]
fn a_listing_no_longer_active_or_paused_is_read_again_for_its_status() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel.has(vec![
            listing("MLB1", "Fone Bluetooth TWS"),
            listing("MLB2", "Luminária LED"),
        ]);
        fx.sync().await;
        fx.channel.change("MLB1", |listing| {
            listing.status = ListingStatus::UnderReview
        });
        fx.channel
            .change("MLB2", |listing| listing.status = ListingStatus::Closed);

        fx.sync().await;

        assert_eq!(fx.channel.last_asked(), ["MLB1", "MLB2"]);
        assert_eq!(
            fx.find("MLB1", None).await.listed.status,
            ListingStatus::UnderReview
        );
        assert_eq!(
            fx.find("MLB2", None).await.listed.status,
            ListingStatus::Closed
        );

        // A closed listing never sells again, so it is not asked for again.
        fx.sync().await;
        assert_eq!(fx.channel.last_asked(), ["MLB1"]);
    });
}

#[test]
fn a_listing_the_channel_no_longer_has_is_closed() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel.has(vec![
            listing("MLB1", "Fone Bluetooth TWS"),
            listing("MLB2", "Luminária LED"),
        ]);
        fx.sync().await;
        fx.channel.remove("MLB2", None);

        let report = fx.sync().await;

        assert_eq!(report.gone, 1);
        assert_eq!(
            fx.find("MLB2", None).await.listed.status,
            ListingStatus::Closed
        );
        assert_eq!(fx.sync().await.gone, 0);
    });
}

#[test]
fn each_variation_is_a_listing_of_its_own_and_one_taken_off_is_closed() {
    block_on(async {
        let fx = Fixture::new().await;
        let camiseta = ChannelListing {
            title: "Camiseta Básica Algodão".into(),
            ..listing("MLB3", "")
        };
        fx.channel.has(vec![
            ChannelListing {
                seller_sku: Some("CAM-PRE-M".into()),
                available_quantity: 3,
                ..variation(&camiseta, "31", "Cor: Preto · Tamanho: M")
            },
            ChannelListing {
                seller_sku: Some("CAM-BRA-M".into()),
                available_quantity: 7,
                ..variation(&camiseta, "32", "Cor: Branco · Tamanho: M")
            },
        ]);

        let first = fx.sync().await;
        fx.channel.remove("MLB3", Some("32"));
        let second = fx.sync().await;

        assert_eq!(first.new, 2);
        assert_eq!(second.gone, 1);
        let black = fx.find("MLB3", Some("31")).await;
        assert_eq!(black.listed.available_quantity, 3);
        assert_eq!(black.listed.seller_sku.as_deref(), Some("CAM-PRE-M"));
        assert_eq!(black.listed.status, ListingStatus::Active);
        assert_eq!(
            fx.find("MLB3", Some("32")).await.listed.status,
            ListingStatus::Closed
        );
    });
}

#[test]
fn a_link_to_a_product_survives_every_sync() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel.has(vec![listing("MLB1", "Fone Bluetooth TWS")]);
        fx.sync().await;
        let fone = fx.find("MLB1", None).await;
        let product = RecordId::from_u128(500);

        let linked = fx.listings.link(fone.id, product).await.unwrap();
        fx.channel.change("MLB1", |listing| {
            listing.title = "Fone Bluetooth TWS Pro".into()
        });
        fx.sync().await;

        assert_eq!(linked.product, Some(product));
        let synced = fx.find("MLB1", None).await;
        assert_eq!(synced.product, Some(product));
        assert_eq!(synced.listed.title, "Fone Bluetooth TWS Pro");
    });
}

#[test]
fn listings_to_link_leave_out_linked_and_closed_ones() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel.has(vec![
            listing("MLB1", "Fone Bluetooth TWS"),
            listing("MLB2", "Luminária LED"),
            listing("MLB3", "Carregador USB-C"),
        ]);
        fx.sync().await;
        fx.channel
            .change("MLB3", |listing| listing.status = ListingStatus::Closed);
        fx.sync().await;
        let fone = fx.find("MLB1", None).await;
        fx.listings
            .link(fone.id, RecordId::from_u128(500))
            .await
            .unwrap();

        let to_link = fx.listings.to_link(&[]).await.unwrap();

        let ids: Vec<_> = to_link
            .iter()
            .map(|entry| entry.listing.listed.id.as_str())
            .collect();
        assert_eq!(ids, ["MLB2"]);
    });
}

#[test]
fn unlinking_puts_a_listing_back_to_link() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel.has(vec![listing("MLB1", "Fone Bluetooth TWS")]);
        fx.sync().await;
        let fone = fx.find("MLB1", None).await;
        fx.listings
            .link(fone.id, RecordId::from_u128(500))
            .await
            .unwrap();

        let unlinked = fx.listings.unlink(fone.id).await.unwrap();

        assert_eq!(unlinked.product, None);
        assert_eq!(fx.listings.to_link(&[]).await.unwrap().len(), 1);
    });
}

#[test]
fn the_seller_sku_suggests_the_product_with_that_sku() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel.has(vec![ChannelListing {
            // Typed by hand in the channel: case and separators differ.
            seller_sku: Some("fon-blu tws-001".into()),
            ..listing("MLB1", "Luminária LED")
        }]);
        fx.sync().await;
        let products = [
            product(1, "FON-BLU-TWS-001", "Fone Bluetooth TWS"),
            // Its name is in the title, but the SKU says otherwise.
            product(2, "LUM-LED-001", "Luminária LED"),
        ];

        let to_link = fx.listings.to_link(&products).await.unwrap();

        let suggestion = to_link[0].suggestion.unwrap();
        assert_eq!(suggestion.product, RecordId::from_u128(1));
        assert_eq!(suggestion.by, SuggestedBy::SellerSku);
    });
}

#[test]
fn the_title_suggests_the_product_whose_whole_name_it_holds() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel.has(vec![
            listing("MLB1", "Fone de Ouvido Bluetooth TWS Pro, Preto"),
            // An unknown seller SKU falls back to the title.
            ChannelListing {
                seller_sku: Some("XYZ-9".into()),
                ..listing("MLB2", "Luminaria Led de Mesa Articulada")
            },
            listing("MLB3", "Carregador Turbo 20W"),
        ]);
        fx.sync().await;
        let products = [
            product(1, "FON-001", "Fone Bluetooth"),
            product(2, "FON-002", "Fone de Ouvido Bluetooth TWS"),
            product(3, "LUM-001", "Luminária LED de Mesa"),
            product(4, "CAR-001", "Carregador USB-C"),
        ];

        let to_link = fx.listings.to_link(&products).await.unwrap();

        let suggested: Vec<_> = to_link
            .iter()
            .map(|entry| {
                (
                    entry.listing.listed.id.as_str(),
                    entry.suggestion.map(|s| (s.product, s.by)),
                )
            })
            .collect();
        // The name with the most words wins; accents and case aside; no name
        // fits the charger.
        assert_eq!(
            suggested,
            [
                ("MLB3", None),
                ("MLB1", Some((RecordId::from_u128(2), SuggestedBy::Title))),
                ("MLB2", Some((RecordId::from_u128(3), SuggestedBy::Title))),
            ]
        );
    });
}

#[test]
fn two_products_that_fit_as_well_suggest_neither() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel
            .has(vec![listing("MLB1", "Kit Fone Bluetooth e Capa Silicone")]);
        fx.sync().await;
        let products = [
            product(1, "FON-001", "Fone Bluetooth"),
            product(2, "CAP-001", "Capa Silicone"),
        ];

        let to_link = fx.listings.to_link(&products).await.unwrap();

        assert_eq!(to_link[0].suggestion, None);
    });
}

#[test]
fn a_variation_name_counts_as_part_of_the_title() {
    block_on(async {
        let fx = Fixture::new().await;
        let camiseta = listing("MLB3", "Camiseta Básica");
        fx.channel.has(vec![
            variation(&camiseta, "31", "Cor: Preto"),
            variation(&camiseta, "32", "Cor: Branco"),
        ]);
        fx.sync().await;
        let products = [
            product(1, "CAM-PRE", "Camiseta Básica Preta"),
            product(2, "CAM-BRA", "Camiseta Básica Branco"),
        ];

        let to_link = fx.listings.to_link(&products).await.unwrap();

        let suggested: Vec<_> = to_link
            .iter()
            .map(|entry| entry.suggestion.map(|s| s.product))
            .collect();
        // "Preta" is not "Preto": only the white one is suggested.
        assert_eq!(suggested, [Some(RecordId::from_u128(2)), None]);
    });
}

#[test]
fn linking_an_unknown_listing_fails() {
    block_on(async {
        let fx = Fixture::new().await;
        let unknown: RecordId = RecordId::from_u128(999);

        let linked = fx.listings.link(unknown, RecordId::from_u128(1)).await;
        let unlinked = fx.listings.unlink(unknown).await;

        assert!(matches!(linked, Err(ListingError::UnknownListing(id)) if id == unknown));
        assert!(matches!(unlinked, Err(ListingError::UnknownListing(id)) if id == unknown));
    });
}

#[test]
fn a_failed_sync_keeps_what_was_there() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel.has(vec![listing("MLB1", "Fone Bluetooth TWS")]);
        fx.sync().await;
        let before = fx.all().await;
        *fx.channel.failure.lock().unwrap() = Some(PlatformError::Expired);

        let failed = fx.listings.sync(&fx.channel).await;

        assert!(matches!(
            failed,
            Err(ListingError::Platform(PlatformError::Expired))
        ));
        assert_eq!(fx.all().await, before);
    });
}

#[test]
fn before_the_first_sync_there_is_nothing() {
    block_on(async {
        let fx = Fixture::new().await;

        let report = fx.sync().await;

        assert_eq!(report, ListingSync::default());
        assert!(fx.all().await.is_empty());
        assert_eq!(fx.listings.last_sync().await.unwrap(), None);
        // An empty channel is not even asked for listings.
        assert!(fx.channel.asked.lock().unwrap().is_empty());
    });
}

fn any_listing() -> impl Strategy<Value = ChannelListing> {
    (
        1u32..6,
        proptest::option::of(1u32..3),
        "[A-Za-z ]{1,12}",
        0u32..100_000,
        0u32..50,
        prop_oneof![Just(ListingStatus::Active), Just(ListingStatus::Paused)],
    )
        .prop_map(|(item, variation, title, cents, stock, status)| {
            let base = ChannelListing {
                title,
                price: Money::new(Decimal::new(i64::from(cents), 2), Currency::Brl),
                available_quantity: stock,
                status,
                ..listing(&format!("MLB{item}"), "")
            };
            match variation {
                Some(id) => ChannelListing {
                    variation: Some(Variation {
                        id: id.to_string(),
                        name: format!("Opção {id}"),
                    }),
                    ..base
                },
                None => base,
            }
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Re-running a Sync with the same answers yields the same Listings,
    /// one per listing and variation the channel reported.
    #[test]
    fn re_syncing_never_duplicates(listings in proptest::collection::vec(any_listing(), 0..8)) {
        block_on(async {
            let fx = Fixture::new().await;
            fx.channel.has(listings.clone());
            fx.sync().await;
            let first = fx.all().await;
            let again = fx.sync().await;
            let second = fx.all().await;

            let mut keys: Vec<_> = listings
                .iter()
                .map(|l| (l.id.clone(), l.variation.as_ref().map(|v| v.id.clone())))
                .collect();
            keys.sort();
            keys.dedup();
            assert_eq!(second.len(), keys.len());
            assert_eq!(again.new, 0);
            let ids = |all: &[Listing]| all.iter().map(|l| l.id).collect::<Vec<_>>();
            let mut before = ids(&first);
            let mut after = ids(&second);
            before.sort();
            after.sort();
            assert_eq!(before, after);
        });
    }
}

#[test]
fn a_new_price_goes_to_the_channel_in_cents_and_every_variation_takes_it() {
    block_on(async {
        let fx = Fixture::new().await;
        let base = listing("MLB1", "Camiseta Básica");
        fx.channel.has(vec![
            variation(&base, "11", "Cor: Preto"),
            variation(&base, "12", "Cor: Branco"),
            listing("MLB2", "Fone Bluetooth TWS"),
        ]);
        fx.sync().await;
        let black = fx.find("MLB1", Some("11")).await;

        let changed = fx
            .listings
            .change_price(black.id, brl("99.904"), &fx.channel)
            .await
            .unwrap();

        assert_eq!(
            *fx.channel.prices_set.lock().unwrap(),
            [("MLB1".to_owned(), brl("99.90"))]
        );
        assert_eq!(changed.len(), 2);
        assert!(changed.iter().all(|l| l.listed.price == brl("99.90")));
        assert_eq!(fx.find("MLB1", Some("12")).await.listed.price, brl("99.90"));
        assert_eq!(fx.find("MLB2", None).await.listed.price, brl("89.90"));
        // The channel now reports the same price: the next Sync sees no change.
        assert_eq!(fx.sync().await.changed, 0);
    });
}

#[test]
fn a_price_of_zero_or_in_another_currency_never_reaches_the_channel() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel.has(vec![listing("MLB1", "Fone Bluetooth TWS")]);
        fx.sync().await;
        let id = fx.find("MLB1", None).await.id;

        for price in [
            brl("0"),
            brl("0.004"),
            brl("-10"),
            Money::new(Decimal::from(10), Currency::Usd),
        ] {
            let refused = fx.listings.change_price(id, price, &fx.channel).await;
            assert!(
                matches!(refused, Err(ListingError::InvalidPrice)),
                "{price}"
            );
        }
        assert!(fx.channel.prices_set.lock().unwrap().is_empty());
        assert_eq!(fx.find("MLB1", None).await.listed.price, brl("89.90"));
    });
}

#[test]
fn a_price_the_channel_refuses_leaves_the_listing_as_it_was() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel.has(vec![listing("MLB1", "Fone Bluetooth TWS")]);
        fx.sync().await;
        let id = fx.find("MLB1", None).await.id;
        *fx.channel.failure.lock().unwrap() =
            Some(PlatformError::Refused("price is too low".into()));

        let refused = fx.listings.change_price(id, brl("5"), &fx.channel).await;

        assert!(matches!(
            refused,
            Err(ListingError::Platform(PlatformError::Refused(_)))
        ));
        assert_eq!(fx.find("MLB1", None).await.listed.price, brl("89.90"));
    });
}

#[test]
fn each_category_is_named_once_from_the_channel() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel
            .categories
            .lock()
            .unwrap()
            .insert("MLB1055".into(), "Capas".into());
        let capa = ChannelListing {
            category: Some("MLB1055".into()),
            ..listing("MLB3", "Capa de celular")
        };
        let gone = ChannelListing {
            category: Some("MLB0000".into()),
            ..listing("MLB4", "Categoria extinta")
        };
        fx.channel.has(vec![
            listing("MLB1", "Fone Bluetooth TWS"),
            listing("MLB2", "Fone com fio"),
            capa,
            gone,
        ]);

        fx.sync().await;
        fx.sync().await;

        let names = fx.listings.category_names().await.unwrap();
        assert_eq!(
            names.get("MLB3697").map(String::as_str),
            Some("Fones de Ouvido")
        );
        assert_eq!(names.get("MLB1055").map(String::as_str), Some("Capas"));
        assert_eq!(names.get("MLB0000"), None);
        // Known names are not asked again; the one the channel lacks is.
        assert_eq!(
            *fx.channel.categories_asked.lock().unwrap(),
            ["MLB3697", "MLB1055", "MLB0000", "MLB0000"]
        );
    });
}

#[test]
fn a_category_the_channel_fails_to_name_fails_the_sync_and_keeps_nothing() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel.has(vec![listing("MLB1", "Fone Bluetooth TWS")]);
        fx.sync().await;
        let capa = ChannelListing {
            category: Some("MLB1055".into()),
            ..listing("MLB3", "Capa de celular")
        };
        fx.channel
            .has(vec![listing("MLB1", "Fone Bluetooth TWS"), capa]);
        *fx.channel.category_failure.lock().unwrap() = Some(PlatformError::RateLimited);

        let failed = fx.listings.sync(&fx.channel).await;

        assert!(matches!(
            failed,
            Err(ListingError::Platform(PlatformError::RateLimited))
        ));
        assert_eq!(fx.all().await.len(), 1);
    });
}
