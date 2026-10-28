//! On-Order Listings (#207, ADR 0038): the owner marks a Mercado Livre
//! listing to sell before buying; the channel shows the days to be ready
//! and the listing shows the orders that still fit instead of a Stock
//! Share. Through commerce's public interface with a real temporary
//! database, the real Inventory and an in-memory Sales Channel.

mod common;

use std::sync::Arc;

use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    ChannelListing, ChannelOrder, ChannelStock, ListingError, Listings, MirroredStock,
    NewPurchaseLine, NewPurchaseOrder, OnOrderTerms, OrderSlots, OrderStatus, Orders,
    PurchaseOrders, Receiving, SalesChannel, ShipmentStatus, StockMirror,
};
use mascate_inventory::{HOME_LOCATION, Inventory};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, PlatformError, RecordId, SalesChannelKind};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;

use common::{Channel, brl, listing, order, sold, variation};

fn fone() -> RecordId {
    RecordId::from_u128(1_000_010)
}

fn supplier() -> RecordId {
    RecordId::from_u128(1_000_001)
}

fn terms(availability_days: u32, accepted_orders: u32) -> OnOrderTerms {
    OnOrderTerms {
        availability_days,
        accepted_orders,
    }
}

/// A listing with nothing on sale yet.
fn empty(id: &str) -> ChannelListing {
    ChannelListing {
        available_quantity: 0,
        ..listing(id, "Fone Bluetooth")
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    purchases: PurchaseOrders,
    listings: Listings,
    orders: Orders,
    mirror: StockMirror,
    ml: Channel,
    sales: std::sync::Mutex<u32>,
}

impl Fixture {
    /// The fone sold on Mercado Livre by `listed`, synced and linked, with
    /// the first Order Sync done.
    async fn selling(listed: Vec<ChannelListing>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap(),
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
        let fx = Self {
            _dir: dir,
            purchases: PurchaseOrders::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            ),
            listings: Listings::new(database.clone(), clock.clone(), ids.clone()),
            orders: Orders::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            ),
            mirror: StockMirror::new(database, inventory, clock.clone(), ids),
            clock,
            ml: Channel::default(),
            sales: std::sync::Mutex::new(0),
        };
        for listed in listed {
            fx.ml.listings.lock().unwrap().push(listed);
            fx.later();
            fx.listings.sync(&fx.ml).await.unwrap();
        }
        fx.orders.sync(&fx.ml).await.unwrap();
        for listing in fx.listings.listings().await.unwrap() {
            fx.listings.link(listing.id, fone()).await.unwrap();
        }
        fx
    }

    fn later(&self) {
        self.clock.advance(TimeDelta::minutes(5));
    }

    async fn id_of(&self, item: &str, variation: Option<&str>) -> RecordId {
        self.listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .find(|listing| {
                listing.listed.id == item
                    && listing.listed.variation.as_ref().map(|v| v.id.as_str()) == variation
            })
            .unwrap()
            .id
    }

    async fn mark(&self, item: &str, terms: OnOrderTerms) -> Result<usize, ListingError> {
        self.later();
        let id = self.id_of(item, None).await;
        self.listings
            .mark_on_order(id, terms, &self.ml)
            .await
            .map(|marked| marked.len())
    }

    async fn unmark(&self, item: &str) -> Result<usize, ListingError> {
        self.later();
        let id = self.id_of(item, None).await;
        self.listings
            .unmark_on_order(id, &self.ml)
            .await
            .map(|unmarked| unmarked.len())
    }

    async fn receive(&self, units: u32) {
        self.later();
        let order = self
            .purchases
            .create(NewPurchaseOrder {
                supplier: supplier(),
                ordered_on: NaiveDate::from_ymd_opt(2026, 10, 27).unwrap(),
                freight: brl("0"),
                lines: vec![NewPurchaseLine {
                    product: fone(),
                    quantity: units,
                    unit_price: brl("20.00"),
                }],
            })
            .await
            .unwrap();
        self.purchases
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

    /// A buyer buys `units` of `item`, which the Orders read; returns the
    /// Order's id.
    async fn buys(&self, item: &str, units: u32) -> String {
        self.later();
        let id = {
            let mut sales = self.sales.lock().unwrap();
            *sales += 1;
            format!("200000000{}", *sales)
        };
        self.ml
            .sells(order(&id, self.clock.now(), vec![sold(item, None, units)]));
        self.later();
        self.orders.sync(&self.ml).await.unwrap();
        id
    }

    /// The channel changes the Order `id`, which the Orders read.
    async fn order_changes(&self, id: &str, change: impl Fn(&mut ChannelOrder)) {
        self.later();
        self.ml.change_order(id, self.clock.now(), change);
        self.later();
        self.orders.sync(&self.ml).await.unwrap();
    }

    async fn send(&self) {
        self.later();
        let channels: [&dyn SalesChannel; 1] = [&self.ml];
        self.mirror.send(&channels).await.unwrap();
    }

    async fn stock_of(&self, item: &str) -> MirroredStock {
        let id = self.id_of(item, None).await;
        self.mirror
            .mirrored()
            .await
            .unwrap()
            .into_iter()
            .find(|stock| stock.listing == id)
            .unwrap()
    }

    /// What the channel shows of `item` now.
    fn shown(&self, item: &str) -> u32 {
        self.ml
            .listings
            .lock()
            .unwrap()
            .iter()
            .find(|listed| listed.id == item)
            .unwrap()
            .available_quantity
    }

    fn days_sent(&self) -> Vec<(String, Option<u32>)> {
        self.ml.availability_set.lock().unwrap().clone()
    }
}

#[test]
fn marking_sends_the_days_once_and_the_orders_accepted_never() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;

        assert_eq!(fx.mark("MLB1", terms(15, 3)).await.unwrap(), 1);
        assert_eq!(fx.mark("MLB1", terms(15, 3)).await.unwrap(), 1);
        fx.mark("MLB1", terms(15, 5)).await.unwrap();

        assert_eq!(fx.days_sent(), [("MLB1".to_owned(), Some(15))]);
        let marks = fx.listings.on_order().await.unwrap();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].listing, fx.id_of("MLB1", None).await);
        assert_eq!(marks[0].terms, terms(15, 5));

        fx.mark("MLB1", terms(20, 5)).await.unwrap();

        assert_eq!(
            fx.days_sent(),
            [("MLB1".to_owned(), Some(15)), ("MLB1".to_owned(), Some(20))]
        );
        assert_eq!(fx.listings.on_order().await.unwrap()[0].terms, terms(20, 5));
    });
}

#[test]
fn a_refusal_from_the_channel_keeps_the_listing_unmarked() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;
        fx.receive(4).await;
        *fx.ml.availability_refusal.lock().unwrap() = Some(PlatformError::Refused(
            "anúncio com Flex ou no Full não aceita prazo de disponibilidade".into(),
        ));

        let refused = fx.mark("MLB1", terms(15, 3)).await;

        assert!(matches!(
            refused,
            Err(ListingError::Platform(PlatformError::Refused(why))) if why.contains("Flex")
        ));
        assert!(fx.listings.on_order().await.unwrap().is_empty());
        let stock = fx.stock_of("MLB1").await;
        assert_eq!((stock.share, stock.on_order), (Some(4), None));
    });
}

#[test]
fn days_outside_1_to_60_and_no_orders_accepted_are_refused_before_the_channel() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;

        assert!(matches!(
            fx.mark("MLB1", terms(0, 3)).await,
            Err(ListingError::InvalidAvailability)
        ));
        assert!(matches!(
            fx.mark("MLB1", terms(61, 3)).await,
            Err(ListingError::InvalidAvailability)
        ));
        assert!(matches!(
            fx.mark("MLB1", terms(15, 0)).await,
            Err(ListingError::NoOrdersAccepted)
        ));
        assert_eq!(fx.mark("MLB1", terms(60, 1)).await.unwrap(), 1);
        assert_eq!(fx.days_sent(), [("MLB1".to_owned(), Some(60))]);
    });
}

#[test]
fn a_listing_on_another_channel_is_never_marked_through_mercado_livre() {
    block_on(async {
        let fx = Fixture::selling(vec![]).await;
        let amazon = Channel::on(SalesChannelKind::Amazon);
        amazon.has(vec![empty("B0FONE")]);
        fx.listings.sync(&amazon).await.unwrap();
        let id = fx.listings.listings().await.unwrap()[0].id;

        let refused = fx.listings.mark_on_order(id, terms(15, 3), &fx.ml).await;

        assert!(matches!(
            refused,
            Err(ListingError::OtherChannel {
                listing: SalesChannelKind::Amazon,
                channel: SalesChannelKind::MercadoLivre
            })
        ));
        assert!(fx.days_sent().is_empty());
    });
}

#[test]
fn unmarking_takes_the_days_off_first_and_brings_the_stock_share_back() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;
        fx.receive(4).await;
        fx.mark("MLB1", terms(15, 3)).await.unwrap();
        fx.send().await;
        assert_eq!(fx.shown("MLB1"), 3);

        *fx.ml.failure.lock().unwrap() = Some(PlatformError::RateLimited);
        assert!(matches!(
            fx.unmark("MLB1").await,
            Err(ListingError::Platform(PlatformError::RateLimited))
        ));
        assert_eq!(fx.listings.on_order().await.unwrap().len(), 1);
        *fx.ml.failure.lock().unwrap() = None;

        assert_eq!(fx.unmark("MLB1").await.unwrap(), 1);
        assert_eq!(fx.unmark("MLB1").await.unwrap(), 0);

        assert_eq!(
            fx.days_sent(),
            [("MLB1".to_owned(), Some(15)), ("MLB1".to_owned(), None)]
        );
        assert!(fx.listings.on_order().await.unwrap().is_empty());
        fx.send().await;
        assert_eq!(fx.shown("MLB1"), 4);
        assert_eq!(fx.stock_of("MLB1").await.share, Some(4));

        // Marked again, it is a new mark with the days sent again.
        fx.mark("MLB1", terms(10, 2)).await.unwrap();
        assert_eq!(fx.days_sent().last().unwrap().1, Some(10));
        assert_eq!(fx.listings.on_order().await.unwrap()[0].terms, terms(10, 2));
    });
}

#[test]
fn marking_a_variation_marks_the_whole_listing_in_one_call() {
    block_on(async {
        let shirt = listing("MLB3", "Camiseta");
        let fx = Fixture::selling(vec![
            variation(&shirt, "175000000031", "Cor: Preto"),
            variation(&shirt, "175000000032", "Cor: Branco"),
        ])
        .await;
        let black = fx.id_of("MLB3", Some("175000000031")).await;

        let marked = fx
            .listings
            .mark_on_order(black, terms(7, 2), &fx.ml)
            .await
            .unwrap();

        assert_eq!(marked.len(), 2);
        assert!(marked.iter().all(|mark| mark.terms == terms(7, 2)));
        assert_eq!(fx.days_sent(), [("MLB3".to_owned(), Some(7))]);
        fx.send().await;
        let sent = fx.ml.stocks_set.lock().unwrap().clone();
        let mut sent: Vec<ChannelStock> = sent.into_iter().flat_map(|(_, stock)| stock).collect();
        sent.sort_by(|a, b| a.variation.cmp(&b.variation));
        assert_eq!(
            sent,
            [
                ChannelStock {
                    variation: Some("175000000031".into()),
                    available_quantity: 2
                },
                ChannelStock {
                    variation: Some("175000000032".into()),
                    available_quantity: 2
                },
            ]
        );

        let unmarked = fx.listings.unmark_on_order(black, &fx.ml).await.unwrap();
        assert_eq!(unmarked.len(), 2);
        assert_eq!(fx.days_sent().len(), 2);
    });
}

#[test]
fn the_channel_shows_the_orders_accepted_less_the_units_waiting_for_dispatch() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;
        fx.mark("MLB1", terms(15, 3)).await.unwrap();
        fx.send().await;
        assert_eq!(fx.shown("MLB1"), 3);

        let first = fx.buys("MLB1", 1).await;
        let stock = fx.stock_of("MLB1").await;
        assert_eq!(
            stock.on_order,
            Some(OrderSlots {
                accepted: 3,
                waiting: 1
            })
        );
        fx.send().await;
        assert_eq!(fx.shown("MLB1"), 2);

        fx.order_changes(&first, |order| {
            order.shipment.as_mut().unwrap().status = ShipmentStatus::Shipped;
        })
        .await;
        fx.send().await;
        assert_eq!(fx.shown("MLB1"), 3);

        let second = fx.buys("MLB1", 2).await;
        fx.send().await;
        assert_eq!(fx.shown("MLB1"), 1);
        fx.order_changes(&second, |order| order.status = OrderStatus::Cancelled)
            .await;
        fx.send().await;
        assert_eq!(fx.shown("MLB1"), 3);

        // Sending the same again writes nothing.
        let sends = fx.ml.stocks_set.lock().unwrap().len();
        fx.send().await;
        assert_eq!(fx.ml.stocks_set.lock().unwrap().len(), sends);
    });
}

#[test]
fn more_units_waiting_than_orders_accepted_show_none_and_pause_the_listing() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;
        fx.mark("MLB1", terms(15, 3)).await.unwrap();
        fx.send().await;
        fx.buys("MLB1", 3).await;
        fx.mark("MLB1", terms(15, 2)).await.unwrap();

        let stock = fx.stock_of("MLB1").await;
        assert_eq!(stock.on_order.unwrap().open(), 0);
        fx.send().await;
        assert_eq!(fx.shown("MLB1"), 0);
    });
}

#[test]
fn the_products_units_go_to_its_other_listings_never_to_the_on_order_one() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1"), empty("MLB2")]).await;
        fx.mark("MLB2", terms(20, 3)).await.unwrap();

        fx.receive(5).await;
        fx.send().await;

        assert_eq!(fx.shown("MLB1"), 5);
        assert_eq!(fx.shown("MLB2"), 3);
        let on_order = fx.stock_of("MLB2").await;
        assert_eq!((on_order.on_hand, on_order.share), (None, None));
        assert_eq!(fx.stock_of("MLB1").await.share, Some(5));
    });
}

#[test]
fn an_on_order_listing_paused_by_the_owner_gets_nothing_sent() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;
        fx.mark("MLB1", terms(15, 3)).await.unwrap();
        fx.send().await;
        let id = fx.id_of("MLB1", None).await;
        fx.listings.pause(id, &fx.ml).await.unwrap();

        fx.buys("MLB1", 1).await;
        let stock = fx.stock_of("MLB1").await;

        assert_eq!(stock.to_send, None);
        assert!(stock.on_order.is_some());
    });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// However many units are on hand, orders accepted and units waiting,
    /// the other listings share every unit and the On-Order Listing shows
    /// the orders that still fit, never below zero.
    #[test]
    fn units_and_open_orders_never_mix(
        on_hand in 0u32..12,
        accepted in 1u32..6,
        sales in proptest::collection::vec(1u32..3, 0..4),
    ) {
        block_on(async {
            let fx = Fixture::selling(vec![empty("MLB1"), empty("MLB2"), empty("MLB3")]).await;
            fx.mark("MLB3", terms(10, accepted)).await.unwrap();
            if on_hand > 0 {
                fx.receive(on_hand).await;
            }
            fx.send().await;
            let mut waiting = 0;
            for units in sales {
                if fx.shown("MLB3") >= units {
                    fx.buys("MLB3", units).await;
                    waiting += units;
                }
            }
            fx.send().await;

            let shares: u32 = [fx.stock_of("MLB1").await, fx.stock_of("MLB2").await]
                .iter()
                .map(|stock| stock.share.unwrap_or_default())
                .sum();
            let on_hand_now = fx.stock_of("MLB1").await.on_hand.unwrap_or_default();
            prop_assert_eq!(shares, on_hand_now);
            prop_assert_eq!(fx.shown("MLB3"), accepted.saturating_sub(waiting));
            Ok(())
        })?;
    }
}
