//! The stock of each Listing follows the app's, and Listings pause and
//! reactivate, through commerce's public interface with a real temporary
//! database, the real Inventory and an in-memory Sales Channel.

mod common;

use std::sync::Arc;

use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    ChannelListing, ChannelStock, Listing, ListingError, ListingStatus, Listings, MirroredStock,
    NewPurchaseLine, NewPurchaseOrder, PurchaseOrders, Receiving, StockMirror, StockSend,
};
use mascate_inventory::{HOME_LOCATION, Inventory, StockAdjustment};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, PlatformError, RecordId};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;

use common::{Channel, brl, listing, variation};

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    inventory: Arc<Inventory>,
    orders: PurchaseOrders,
    listings: Listings,
    mirror: StockMirror,
    channel: Channel,
}

fn fone() -> RecordId {
    RecordId::from_u128(1_000_010)
}

fn capa() -> RecordId {
    RecordId::from_u128(1_000_011)
}

fn supplier() -> RecordId {
    RecordId::from_u128(1_000_001)
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
        migrate(
            &database,
            clock.as_ref(),
            &[mascate_inventory::MIGRATIONS, mascate_commerce::MIGRATIONS],
        )
        .await
        .unwrap();
        let ids = Arc::new(SequentialIds::default());
        let inventory = Arc::new(Inventory::new(database.clone(), clock.clone(), ids.clone()));
        Self {
            _dir: dir,
            orders: PurchaseOrders::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            ),
            listings: Listings::new(database.clone(), clock.clone(), ids.clone()),
            mirror: StockMirror::new(database, inventory.clone(), clock.clone(), ids),
            inventory,
            clock,
            channel: Channel::default(),
        }
    }

    /// The listings in the channel, synced, with each one linked to the
    /// Product given for it.
    async fn selling(listings: Vec<(ChannelListing, Option<RecordId>)>) -> Self {
        let fx = Self::new().await;
        fx.channel
            .has(listings.iter().map(|(listed, _)| listed.clone()).collect());
        fx.listings.sync(&fx.channel).await.unwrap();
        for (listed, product) in listings {
            if let Some(product) = product {
                let id = fx.find(&listed).await.id;
                fx.listings.link(id, product).await.unwrap();
            }
        }
        fx
    }

    /// The channel moves `listed` to `status`, and a Sync reads it.
    async fn now_in_channel(&self, listed: &ChannelListing, status: ListingStatus) {
        self.channel
            .change(&listed.id, |found| found.status = status);
        self.listings.sync(&self.channel).await.unwrap();
    }

    fn later(&self) {
        self.clock.advance(TimeDelta::minutes(5));
    }

    async fn receive(&self, product: RecordId, units: u32) {
        self.later();
        let order = self
            .orders
            .create(NewPurchaseOrder {
                supplier: supplier(),
                ordered_on: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                freight: brl("0"),
                lines: vec![NewPurchaseLine {
                    product,
                    quantity: units,
                    unit_price: brl("20.00"),
                }],
            })
            .await
            .unwrap();
        self.orders
            .receive(
                order.id,
                HOME_LOCATION,
                &[Receiving {
                    line: order.lines[0].id,
                    quantity: units,
                }],
            )
            .await
            .unwrap();
    }

    async fn adjust(&self, product: RecordId, adjustment: StockAdjustment) {
        self.later();
        self.inventory
            .adjust(product, HOME_LOCATION, adjustment, None)
            .await
            .unwrap();
    }

    async fn send(&self) -> StockSend {
        self.later();
        self.mirror.send(&self.channel).await.unwrap()
    }

    async fn find(&self, listed: &ChannelListing) -> Listing {
        self.listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .find(|listing| {
                listing.listed.id == listed.id && listing.listed.variation == listed.variation
            })
            .unwrap()
    }

    async fn stock_of(&self, listed: &ChannelListing) -> MirroredStock {
        let id = self.find(listed).await.id;
        self.mirror
            .mirrored()
            .await
            .unwrap()
            .into_iter()
            .find(|stock| stock.listing == id)
            .unwrap()
    }

    fn sent(&self) -> Vec<(String, Vec<ChannelStock>)> {
        self.channel.stocks_set.lock().unwrap().clone()
    }

    fn in_channel(&self, listed: &ChannelListing) -> ChannelListing {
        self.channel
            .listings
            .lock()
            .unwrap()
            .iter()
            .find(|found| found.id == listed.id && found.variation == listed.variation)
            .cloned()
            .unwrap()
    }
}

fn whole(units: u32) -> Vec<ChannelStock> {
    vec![ChannelStock {
        variation: None,
        available_quantity: units,
    }]
}

#[test]
fn a_receipt_sends_the_units_on_hand_to_the_linked_listing() {
    block_on(async {
        let fone_listing = listing("MLB1", "Fone Bluetooth");
        let fx = Fixture::selling(vec![(fone_listing.clone(), Some(fone()))]).await;
        fx.receive(fone(), 8).await;

        let due = fx.stock_of(&fone_listing).await;
        assert_eq!(due.on_hand, Some(8));
        assert_eq!(due.to_send, Some(8));
        assert_eq!(due.last_sent, None);

        let report = fx.send().await;
        assert_eq!(report, StockSend { sent: 1, failed: 0 });
        assert_eq!(fx.sent(), [("MLB1".to_owned(), whole(8))]);
        let after = fx.stock_of(&fone_listing).await;
        assert_eq!(after.to_send, None);
        let sent = after.last_sent.unwrap();
        assert_eq!(sent.quantity, 8);
        assert_eq!(sent.at, fx.clock.now());
        assert_eq!(fx.find(&fone_listing).await.listed.available_quantity, 8);
    });
}

#[test]
fn sending_the_same_stock_again_writes_nothing() {
    block_on(async {
        let fone_listing = listing("MLB1", "Fone Bluetooth");
        let fx = Fixture::selling(vec![(fone_listing.clone(), Some(fone()))]).await;
        fx.receive(fone(), 8).await;
        fx.send().await;

        assert_eq!(fx.send().await, StockSend::default());
        assert_eq!(fx.sent().len(), 1);
    });
}

#[test]
fn a_loss_and_a_count_send_the_stock_left() {
    block_on(async {
        let fone_listing = listing("MLB1", "Fone Bluetooth");
        let fx = Fixture::selling(vec![(fone_listing.clone(), Some(fone()))]).await;
        fx.receive(fone(), 8).await;
        fx.send().await;

        fx.adjust(fone(), StockAdjustment::Loss(3)).await;
        fx.send().await;
        fx.adjust(fone(), StockAdjustment::Count(2)).await;
        fx.send().await;

        let sent: Vec<u32> = fx
            .sent()
            .into_iter()
            .map(|(_, stock)| stock[0].available_quantity)
            .collect();
        assert_eq!(sent, [8, 5, 2]);
    });
}

#[test]
fn no_stock_left_pauses_the_listing_and_units_back_reactivate_it() {
    block_on(async {
        let fone_listing = listing("MLB1", "Fone Bluetooth");
        let fx = Fixture::selling(vec![(fone_listing.clone(), Some(fone()))]).await;
        fx.receive(fone(), 2).await;
        fx.send().await;

        fx.adjust(fone(), StockAdjustment::Damage(2)).await;
        fx.send().await;
        let emptied = fx.find(&fone_listing).await.listed;
        assert_eq!(emptied.available_quantity, 0);
        assert_eq!(emptied.status, ListingStatus::Paused);

        fx.receive(fone(), 4).await;
        fx.send().await;
        let refilled = fx.find(&fone_listing).await.listed;
        assert_eq!(refilled.available_quantity, 4);
        assert_eq!(refilled.status, ListingStatus::Active);
    });
}

#[test]
fn a_listing_the_owner_paused_stays_paused_when_units_come_in() {
    block_on(async {
        let fone_listing = listing("MLB1", "Fone Bluetooth");
        let fx = Fixture::selling(vec![(fone_listing.clone(), Some(fone()))]).await;
        let id = fx.find(&fone_listing).await.id;
        fx.listings.pause(id, &fx.channel).await.unwrap();

        fx.receive(fone(), 6).await;
        fx.send().await;

        let listed = fx.find(&fone_listing).await.listed;
        assert_eq!(listed.available_quantity, 6);
        assert_eq!(listed.status, ListingStatus::Paused);
    });
}

#[test]
fn before_the_first_send_the_channels_stock_counts_as_sent() {
    block_on(async {
        // The channel already has 5, as many as come in.
        let fone_listing = listing("MLB1", "Fone Bluetooth");
        let fx = Fixture::selling(vec![(fone_listing.clone(), Some(fone()))]).await;
        fx.receive(fone(), 5).await;

        assert_eq!(fx.stock_of(&fone_listing).await.to_send, None);
        assert_eq!(fx.send().await, StockSend::default());
        assert!(fx.sent().is_empty());
    });
}

#[test]
fn a_sale_in_the_channel_is_not_undone_while_the_app_stock_stays() {
    block_on(async {
        let fone_listing = listing("MLB1", "Fone Bluetooth");
        let fx = Fixture::selling(vec![(fone_listing.clone(), Some(fone()))]).await;
        fx.receive(fone(), 8).await;
        fx.send().await;
        fx.channel
            .change("MLB1", |listing| listing.available_quantity = 7);
        fx.listings.sync(&fx.channel).await.unwrap();

        assert_eq!(fx.stock_of(&fone_listing).await.to_send, None);
        assert_eq!(fx.send().await, StockSend::default());
    });
}

#[test]
fn only_linked_selling_listings_of_products_in_stock_follow_the_app() {
    block_on(async {
        let linked = listing("MLB1", "Fone Bluetooth");
        let never_stocked = listing("MLB2", "Capa de celular");
        let unlinked = listing("MLB3", "Carregador");
        let review = listing("MLB4", "Fone Bluetooth Pro");
        let fx = Fixture::selling(vec![
            (linked.clone(), Some(fone())),
            (never_stocked.clone(), Some(capa())),
            (unlinked.clone(), None),
            (review.clone(), Some(fone())),
        ])
        .await;
        fx.now_in_channel(&review, ListingStatus::UnderReview).await;
        fx.receive(fone(), 9).await;

        assert_eq!(fx.stock_of(&linked).await.to_send, Some(9));
        for other in [&never_stocked, &unlinked, &review] {
            let stock = fx.stock_of(other).await;
            assert_eq!(stock.on_hand, None, "{}", other.id);
            assert_eq!(stock.to_send, None, "{}", other.id);
        }
        fx.send().await;
        assert_eq!(fx.sent(), [("MLB1".to_owned(), whole(9))]);
    });
}

#[test]
fn each_variation_gets_the_stock_of_its_own_product_in_one_request() {
    block_on(async {
        let base = listing("MLB7", "Fone Bluetooth");
        let black = ChannelListing {
            available_quantity: 1,
            ..variation(&base, "101", "Cor: Preto")
        };
        let white = ChannelListing {
            available_quantity: 1,
            ..variation(&base, "102", "Cor: Branco")
        };
        let fx = Fixture::selling(vec![
            (black.clone(), Some(fone())),
            (white.clone(), Some(capa())),
        ])
        .await;
        fx.receive(fone(), 3).await;
        fx.receive(capa(), 4).await;
        fx.send().await;

        let sent = fx.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "MLB7");
        let mut stock = sent[0].1.clone();
        stock.sort_by(|a, b| a.variation.cmp(&b.variation));
        assert_eq!(
            stock,
            [
                ChannelStock {
                    variation: Some("101".into()),
                    available_quantity: 3
                },
                ChannelStock {
                    variation: Some("102".into()),
                    available_quantity: 4
                },
            ]
        );

        fx.adjust(capa(), StockAdjustment::Loss(1)).await;
        fx.send().await;
        assert_eq!(
            fx.sent()[1],
            (
                "MLB7".to_owned(),
                vec![ChannelStock {
                    variation: Some("102".into()),
                    available_quantity: 3
                }]
            )
        );
    });
}

#[test]
fn a_failed_send_stays_in_the_queue_until_one_gets_through() {
    block_on(async {
        let fone_listing = listing("MLB1", "Fone Bluetooth");
        let fx = Fixture::selling(vec![(fone_listing.clone(), Some(fone()))]).await;
        fx.receive(fone(), 8).await;
        let refused = PlatformError::Failed("timeout".into());
        *fx.channel.stock_failure.lock().unwrap() = Some(refused.clone());

        assert_eq!(fx.send().await, StockSend { sent: 0, failed: 1 });
        let queued = fx.stock_of(&fone_listing).await;
        assert_eq!(queued.to_send, Some(8));
        let failure = queued.last_failure.unwrap();
        assert_eq!(failure.error, refused);
        assert_eq!(failure.at, fx.clock.now());

        // Another movement while it waits: the queue holds the newest stock.
        fx.adjust(fone(), StockAdjustment::Loss(1)).await;
        assert_eq!(fx.stock_of(&fone_listing).await.to_send, Some(7));

        *fx.channel.stock_failure.lock().unwrap() = None;
        assert_eq!(fx.send().await, StockSend { sent: 1, failed: 0 });
        let sent = fx.stock_of(&fone_listing).await;
        assert_eq!(sent.to_send, None);
        assert_eq!(sent.last_failure, None);
        assert_eq!(fx.sent(), [("MLB1".to_owned(), whole(7))]);
    });
}

#[test]
fn a_lost_login_queues_every_listing_due_without_asking_again() {
    block_on(async {
        let fone_listing = listing("MLB1", "Fone Bluetooth");
        let capa_listing = listing("MLB2", "Capa de celular");
        let fx = Fixture::selling(vec![
            (fone_listing.clone(), Some(fone())),
            (capa_listing.clone(), Some(capa())),
        ])
        .await;
        fx.receive(fone(), 8).await;
        fx.receive(capa(), 9).await;
        *fx.channel.stock_failure.lock().unwrap() = Some(PlatformError::Expired);

        assert_eq!(fx.send().await, StockSend { sent: 0, failed: 2 });
        for listed in [&fone_listing, &capa_listing] {
            let stock = fx.stock_of(listed).await;
            assert!(stock.to_send.is_some());
            assert_eq!(stock.last_failure.unwrap().error, PlatformError::Expired);
        }
    });
}

#[test]
fn a_failure_for_stock_no_longer_due_is_dropped() {
    block_on(async {
        let fone_listing = listing("MLB1", "Fone Bluetooth");
        let fx = Fixture::selling(vec![(fone_listing.clone(), Some(fone()))]).await;
        fx.receive(fone(), 8).await;
        *fx.channel.stock_failure.lock().unwrap() = Some(PlatformError::RateLimited);
        fx.send().await;

        // Back to the 5 the channel has.
        fx.adjust(fone(), StockAdjustment::Count(5)).await;

        let stock = fx.stock_of(&fone_listing).await;
        assert_eq!(stock.to_send, None);
        assert_eq!(stock.last_failure, None);
    });
}

#[test]
fn linking_another_product_follows_that_products_stock() {
    block_on(async {
        let fone_listing = listing("MLB1", "Fone Bluetooth");
        let fx = Fixture::selling(vec![(fone_listing.clone(), Some(fone()))]).await;
        fx.receive(fone(), 8).await;
        fx.receive(capa(), 20).await;
        fx.send().await;

        let id = fx.find(&fone_listing).await.id;
        fx.listings.link(id, capa()).await.unwrap();

        let relinked = fx.stock_of(&fone_listing).await;
        assert_eq!(relinked.last_sent, None);
        assert_eq!(relinked.to_send, Some(20));
    });
}

#[test]
fn pausing_takes_every_variation_off_sale_and_reactivating_puts_them_back() {
    block_on(async {
        let base = listing("MLB7", "Fone Bluetooth");
        let black = variation(&base, "101", "Cor: Preto");
        let white = variation(&base, "102", "Cor: Branco");
        let fx = Fixture::selling(vec![(black.clone(), None), (white.clone(), None)]).await;
        let id = fx.find(&black).await.id;

        let paused = fx.listings.pause(id, &fx.channel).await.unwrap();
        assert_eq!(paused.len(), 2);
        assert!(
            paused
                .iter()
                .all(|listing| listing.listed.status == ListingStatus::Paused)
        );
        assert_eq!(fx.find(&white).await.listed.status, ListingStatus::Paused);

        let active = fx.listings.reactivate(id, &fx.channel).await.unwrap();
        assert!(
            active
                .iter()
                .all(|listing| listing.listed.status == ListingStatus::Active)
        );
        assert_eq!(
            *fx.channel.statuses_set.lock().unwrap(),
            [("MLB7".to_owned(), true), ("MLB7".to_owned(), false)]
        );
        // The next Sync reads the same.
        fx.listings.sync(&fx.channel).await.unwrap();
        assert_eq!(fx.find(&white).await.listed.status, ListingStatus::Active);
    });
}

#[test]
fn only_an_active_listing_pauses_and_only_a_paused_one_reactivates() {
    block_on(async {
        let active = listing("MLB1", "Fone Bluetooth");
        let closed = listing("MLB2", "Capa de celular");
        let fx = Fixture::selling(vec![(active.clone(), None), (closed.clone(), None)]).await;
        fx.now_in_channel(&closed, ListingStatus::Closed).await;
        let active = fx.find(&active).await.id;
        let closed = fx.find(&closed).await.id;

        assert!(matches!(
            fx.listings.reactivate(active, &fx.channel).await,
            Err(ListingError::NotPaused)
        ));
        assert!(matches!(
            fx.listings.pause(closed, &fx.channel).await,
            Err(ListingError::NotActive)
        ));
        assert!(matches!(
            fx.listings.reactivate(closed, &fx.channel).await,
            Err(ListingError::NotPaused)
        ));
        assert!(fx.channel.statuses_set.lock().unwrap().is_empty());
    });
}

#[test]
fn a_listing_without_units_stays_paused() {
    block_on(async {
        let empty = ChannelListing {
            status: ListingStatus::Paused,
            available_quantity: 0,
            ..listing("MLB1", "Fone Bluetooth")
        };
        let fx = Fixture::selling(vec![(empty.clone(), None)]).await;
        let id = fx.find(&empty).await.id;

        assert!(matches!(
            fx.listings.reactivate(id, &fx.channel).await,
            Err(ListingError::NoStockToSell)
        ));
        assert!(fx.channel.statuses_set.lock().unwrap().is_empty());
    });
}

#[test]
fn a_status_change_the_channel_refuses_leaves_the_listing_as_it_was() {
    block_on(async {
        let active = listing("MLB1", "Fone Bluetooth");
        let fx = Fixture::selling(vec![(active.clone(), None)]).await;
        let id = fx.find(&active).await.id;
        *fx.channel.failure.lock().unwrap() = Some(PlatformError::Refused("no".into()));

        assert!(matches!(
            fx.listings.pause(id, &fx.channel).await,
            Err(ListingError::Platform(PlatformError::Refused(_)))
        ));
        assert_eq!(fx.find(&active).await.listed.status, ListingStatus::Active);
    });
}

/// A movement of the fone's stock.
#[derive(Debug, Clone, Copy)]
enum Move {
    Receive(u32),
    Loss(u32),
    Count(u32),
    /// The app sends what changed, as after a movement or a Sync.
    Send,
    /// The next sends fail, or work again.
    ChannelDown(bool),
}

fn any_move() -> impl Strategy<Value = Move> {
    prop_oneof![
        (1u32..20).prop_map(Move::Receive),
        (1u32..20).prop_map(Move::Loss),
        (0u32..20).prop_map(Move::Count),
        Just(Move::Send),
        any::<bool>().prop_map(Move::ChannelDown),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// Whatever moved and whatever failed on the way, once a send gets
    /// through the channel has the units on hand.
    #[test]
    fn once_a_send_gets_through_the_channel_has_the_units_on_hand(
        moves in prop::collection::vec(any_move(), 1..12)
    ) {
        block_on(async {
            let fone_listing = listing("MLB1", "Fone Bluetooth");
            let fx = Fixture::selling(vec![(fone_listing.clone(), Some(fone()))]).await;
            fx.receive(fone(), 1).await;
            for step in moves {
                match step {
                    Move::Receive(units) => fx.receive(fone(), units).await,
                    Move::Loss(units) => {
                        fx.later();
                        // More than on hand is refused, and nothing moves.
                        let _ = fx
                            .inventory
                            .adjust(fone(), HOME_LOCATION, StockAdjustment::Loss(units), None)
                            .await;
                    }
                    Move::Count(units) => fx.adjust(fone(), StockAdjustment::Count(units)).await,
                    Move::Send => {
                        fx.send().await;
                    }
                    Move::ChannelDown(down) => {
                        *fx.channel.stock_failure.lock().unwrap() =
                            down.then(|| PlatformError::Failed("down".into()));
                    }
                }
            }
            *fx.channel.stock_failure.lock().unwrap() = None;
            fx.send().await;

            let on_hand = fx.stock_of(&fone_listing).await.on_hand.unwrap();
            prop_assert_eq!(fx.in_channel(&fone_listing).available_quantity, on_hand);
            prop_assert_eq!(fx.stock_of(&fone_listing).await.to_send, None);
            Ok(())
        })?;
    }
}
