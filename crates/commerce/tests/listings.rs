//! Listings synced from a Sales Channel and linked to Products, through
//! commerce's public interface with a real temporary database and an
//! in-memory Sales Channel.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    CatalogProduct, ChannelListing, Listing, ListingError, ListingStatus, ListingSync, Listings,
    SalesChannel, SuggestedBy, Variation,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Currency, ListingType, Money, PlatformError, RecordId};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

fn listing(id: &str, title: &str) -> ChannelListing {
    ChannelListing {
        id: id.into(),
        variation: None,
        title: title.into(),
        price: brl("89.90"),
        available_quantity: 5,
        status: ListingStatus::Active,
        link: Some(format!("https://produto.mercadolivre.com.br/{id}")),
        listing_type: Some(ListingType::Classic),
        category: Some("MLB3697".into()),
        seller_sku: None,
    }
}

fn variation(of: &ChannelListing, id: &str, name: &str) -> ChannelListing {
    ChannelListing {
        variation: Some(Variation {
            id: id.into(),
            name: name.into(),
        }),
        ..of.clone()
    }
}

fn product(id: u128, sku: &str, name: &str) -> CatalogProduct {
    CatalogProduct {
        id: RecordId::from_u128(id),
        sku: sku.into(),
        name: name.into(),
    }
}

/// A Sales Channel answering from memory: the listings it has, which of
/// them it lists as active or paused, and the ids it was asked for.
#[derive(Default)]
struct Channel {
    listings: Mutex<Vec<ChannelListing>>,
    asked: Mutex<Vec<Vec<String>>>,
    failure: Mutex<Option<PlatformError>>,
}

impl Channel {
    fn has(&self, listings: Vec<ChannelListing>) {
        *self.listings.lock().unwrap() = listings;
    }

    fn change(&self, id: &str, change: impl Fn(&mut ChannelListing)) {
        self.listings
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|listing| listing.id == id)
            .for_each(change);
    }

    fn remove(&self, id: &str, variation: Option<&str>) {
        self.listings.lock().unwrap().retain(|listing| {
            listing.id != id
                || variation.is_some_and(|wanted| {
                    listing.variation.as_ref().map(|v| v.id.as_str()) != Some(wanted)
                })
        });
    }

    fn last_asked(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_default()
    }
}

impl SalesChannel for Channel {
    fn listing_ids(&self) -> Result<Vec<String>, PlatformError> {
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        let mut ids: Vec<String> = Vec::new();
        for listing in self.listings.lock().unwrap().iter() {
            if matches!(
                listing.status,
                ListingStatus::Active | ListingStatus::Paused
            ) && !ids.contains(&listing.id)
            {
                ids.push(listing.id.clone());
            }
        }
        Ok(ids)
    }

    fn listings(&self, ids: &[String]) -> Result<Vec<ChannelListing>, PlatformError> {
        self.asked.lock().unwrap().push(ids.to_vec());
        Ok(self
            .listings
            .lock()
            .unwrap()
            .iter()
            .filter(|listing| ids.contains(&listing.id))
            .cloned()
            .collect())
    }
}

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
