//! Orders through commerce's public interface with a real temporary
//! database, the real Inventory and an in-memory Sales Channel: each Order
//! is kept once by the channel's id, its units leave the stock once, the
//! Sync after a long pause reads everything since the last one, and the
//! buyer's data goes when its time is over.

mod common;

use std::sync::Arc;

use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    ChannelListing, ChannelStock, LineStock, Listings, NewPurchaseLine, NewPurchaseOrder, Order,
    OrderError, OrderSettings, OrderStatus, Orders, PurchaseOrders, Receiving, ShipmentStatus,
    StockMirror, StockShort,
};
use mascate_inventory::{HOME_LOCATION, Inventory, LowStock, MovementReason};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, PlatformError, RecordId, Timestamp};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;

use common::{Channel, brl, listing, order, sold, variation};

fn fone() -> RecordId {
    RecordId::from_u128(1_000_010)
}

fn capa() -> RecordId {
    RecordId::from_u128(1_000_011)
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    inventory: Arc<Inventory>,
    purchases: PurchaseOrders,
    listings: Listings,
    mirror: StockMirror,
    orders: Orders,
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
            purchases: PurchaseOrders::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            ),
            listings: Listings::new(database.clone(), clock.clone(), ids.clone()),
            mirror: StockMirror::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            ),
            orders: Orders::new(database, inventory.clone(), clock.clone(), ids),
            inventory,
            clock,
            channel: Channel::default(),
        }
    }

    /// A fone listing and a capa listing with two variations, all synced and
    /// linked to their Products, 10 fones and 4 capas on hand, and a first
    /// Order Sync with nothing sold yet.
    async fn selling() -> Self {
        let fx = Self::new().await;
        let capa_listing = listing("MLB2", "Capa de silicone");
        fx.channel.has(vec![
            listing("MLB1", "Fone Bluetooth"),
            variation(&capa_listing, "71", "Cor: Preto"),
            variation(&capa_listing, "72", "Cor: Azul"),
        ]);
        fx.listings.sync(&fx.channel).await.unwrap();
        for found in fx.listings.listings().await.unwrap() {
            let product = if found.listed.id == "MLB1" {
                fone()
            } else {
                capa()
            };
            fx.listings.link(found.id, product).await.unwrap();
        }
        fx.receive(fone(), 10).await;
        fx.receive(capa(), 4).await;
        fx.sync().await;
        fx
    }

    fn later(&self, minutes: i64) -> Timestamp {
        self.clock.advance(TimeDelta::minutes(minutes));
        self.clock.now()
    }

    async fn receive(&self, product: RecordId, units: u32) {
        self.later(1);
        let order = self
            .purchases
            .create(NewPurchaseOrder {
                supplier: RecordId::from_u128(1_000_001),
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

    async fn sync(&self) -> mascate_commerce::OrderSync {
        self.later(5);
        self.orders.sync(&self.channel).await.unwrap()
    }

    async fn units(&self, product: RecordId) -> i64 {
        self.inventory
            .stock()
            .await
            .unwrap()
            .products
            .iter()
            .find(|stock| stock.product == product)
            .map_or(0, |stock| stock.valuation.quantity())
    }

    async fn order(&self, id: &str) -> Order {
        self.orders
            .orders()
            .await
            .unwrap()
            .into_iter()
            .find(|order| order.sold.id == id)
            .unwrap()
    }

    async fn sales_in_ledger(&self, product: RecordId) -> usize {
        self.inventory
            .history(product)
            .await
            .unwrap()
            .iter()
            .filter(|line| matches!(line.movement.reason, MovementReason::Sale { .. }))
            .count()
    }

    fn asked_since(&self) -> Timestamp {
        *self.channel.orders_asked.lock().unwrap().last().unwrap()
    }
}

fn stock_of(order: &Order) -> Vec<LineStock> {
    order.lines.iter().map(|line| line.stock).collect()
}

#[test]
fn the_first_sync_lists_the_recent_orders_without_taking_their_stock() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel.has(vec![listing("MLB1", "Fone Bluetooth")]);
        fx.listings.sync(&fx.channel).await.unwrap();
        let fone_listing = fx.listings.listings().await.unwrap()[0].id;
        fx.listings.link(fone_listing, fone()).await.unwrap();
        fx.receive(fone(), 5).await;
        let yesterday = fx.clock.now() - TimeDelta::days(1);
        fx.channel
            .sells(order("2000001", yesterday, vec![sold("MLB1", None, 2)]));

        let synced = fx.sync().await;

        assert_eq!(fx.asked_since(), fx.clock.now() - TimeDelta::days(30));
        assert_eq!((synced.read, synced.new, synced.taken), (1, 1, 0));
        // Sold before the app read Orders: the stock counted then had
        // already lost these units.
        assert!(synced.arrived.is_empty());
        let kept = fx.order("2000001").await;
        assert_eq!(stock_of(&kept), [LineStock::BeforeFirstSync]);
        assert_eq!(kept.sold.lines, [sold("MLB1", None, 2)]);
        assert_eq!(fx.units(fone()).await, 5);
        assert_eq!(fx.orders.last_sync().await.unwrap(), Some(fx.clock.now()));
    });
}

#[test]
fn a_new_order_takes_its_units_from_the_stock_once() {
    block_on(async {
        let fx = Fixture::selling().await;
        let at = fx.later(1);
        fx.channel
            .sells(order("2000002", at, vec![sold("MLB1", None, 3)]));

        let synced = fx.sync().await;

        assert_eq!((synced.new, synced.taken, synced.short), (1, 1, 0));
        assert_eq!(synced.arrived.len(), 1);
        let arrived = &synced.arrived[0];
        assert_eq!(arrived.sold.id, "2000002");
        assert_eq!(arrived.units(), 3);
        let LineStock::Taken { product, movement } = arrived.lines[0].stock else {
            panic!("the units should have left: {:?}", arrived.lines[0].stock);
        };
        assert_eq!(product, fone());
        assert_eq!(fx.units(fone()).await, 7);
        let history = fx.inventory.history(fone()).await.unwrap();
        assert_eq!(history[0].movement.id, movement);
        assert_eq!(history[0].movement.quantity, -3);
        assert_eq!(
            history[0].movement.reason,
            MovementReason::Sale { order: arrived.id }
        );

        // The next Sync reads the same Order again, inside its overlap.
        let again = fx.sync().await;
        assert_eq!(
            (again.read, again.new, again.changed, again.taken),
            (1, 0, 0, 0)
        );
        assert!(again.arrived.is_empty());
        assert_eq!(fx.units(fone()).await, 7);
        assert_eq!(fx.sales_in_ledger(fone()).await, 1);
        assert_eq!(fx.orders.orders().await.unwrap().len(), 1);
    });
}

#[test]
fn an_order_with_several_items_takes_each_from_its_product() {
    block_on(async {
        let fx = Fixture::selling().await;
        let at = fx.later(1);
        fx.channel.sells(order(
            "2000003",
            at,
            vec![
                sold("MLB1", None, 1),
                sold("MLB2", Some("71"), 2),
                sold("MLB2", Some("72"), 1),
            ],
        ));

        let synced = fx.sync().await;

        assert_eq!(synced.taken, 3);
        let kept = fx.order("2000003").await;
        assert_eq!(kept.units(), 4);
        assert_eq!(
            kept.lines
                .iter()
                .map(|line| (line.sold.variation.clone(), line.stock))
                .map(|(variation, stock)| match stock {
                    LineStock::Taken { product, .. } => (variation, product),
                    other => panic!("{other:?}"),
                })
                .collect::<Vec<_>>(),
            [
                (None, fone()),
                (Some("71".to_owned()), capa()),
                (Some("72".to_owned()), capa())
            ]
        );
        assert_eq!(fx.units(fone()).await, 9);
        assert_eq!(fx.units(capa()).await, 1);
    });
}

#[test]
fn a_line_without_a_product_waits_and_leaves_once_linked() {
    block_on(async {
        let fx = Fixture::selling().await;
        let capa_preta = fx
            .listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .find(|found| found.listed.variation.as_ref().map(|v| v.id.as_str()) == Some("71"))
            .unwrap();
        fx.listings.unlink(capa_preta.id).await.unwrap();
        let at = fx.later(1);
        fx.channel.sells(order(
            "2000004",
            at,
            vec![sold("MLB2", Some("71"), 1), sold("MLB9", None, 1)],
        ));

        let synced = fx.sync().await;

        assert_eq!((synced.taken, synced.short), (0, 2));
        // Still told about: the sale is real even with its stock to sort out.
        assert_eq!(synced.arrived.len(), 1);
        assert_eq!(
            stock_of(&fx.order("2000004").await),
            [
                LineStock::Short(StockShort::NoProduct),
                LineStock::Short(StockShort::NoListing)
            ]
        );
        assert_eq!(fx.units(capa()).await, 4);

        fx.listings.link(capa_preta.id, capa()).await.unwrap();
        let next = fx.sync().await;
        assert_eq!((next.taken, next.short), (1, 1));
        assert!(next.arrived.is_empty());
        assert_eq!(fx.units(capa()).await, 3);
        assert!(matches!(
            stock_of(&fx.order("2000004").await)[..],
            [
                LineStock::Taken { .. },
                LineStock::Short(StockShort::NoListing)
            ]
        ));
    });
}

#[test]
fn a_sale_of_more_than_the_app_has_waits_for_the_units() {
    block_on(async {
        let fx = Fixture::selling().await;
        let at = fx.later(1);
        fx.channel
            .sells(order("2000005", at, vec![sold("MLB2", Some("72"), 6)]));

        let synced = fx.sync().await;

        assert_eq!((synced.taken, synced.short), (0, 1));
        assert_eq!(
            stock_of(&fx.order("2000005").await),
            [LineStock::Short(StockShort::NotEnoughStock)]
        );
        // Never below zero.
        assert_eq!(fx.units(capa()).await, 4);

        fx.receive(capa(), 2).await;
        assert_eq!(fx.sync().await.taken, 1);
        assert_eq!(fx.units(capa()).await, 0);
    });
}

#[test]
fn a_cancelled_order_takes_nothing_and_one_cancelled_later_keeps_its_movement() {
    block_on(async {
        let fx = Fixture::selling().await;
        let at = fx.later(1);
        let mut cancelled = order("2000006", at, vec![sold("MLB1", None, 1)]);
        cancelled.status = OrderStatus::Cancelled;
        fx.channel.sells(cancelled);
        fx.channel
            .sells(order("2000007", at, vec![sold("MLB1", None, 2)]));

        fx.sync().await;
        assert_eq!(stock_of(&fx.order("2000006").await), [LineStock::Cancelled]);
        assert_eq!(fx.units(fone()).await, 8);

        // Putting the units back on a cancellation is for the returns (#21).
        let changed_at = fx.later(1);
        fx.channel.change_order("2000007", changed_at, |found| {
            found.status = OrderStatus::Cancelled;
        });
        let synced = fx.sync().await;
        assert_eq!(synced.changed, 1);
        let kept = fx.order("2000007").await;
        assert_eq!(kept.sold.status, OrderStatus::Cancelled);
        assert!(matches!(stock_of(&kept)[..], [LineStock::Taken { .. }]));
        assert_eq!(fx.units(fone()).await, 8);
    });
}

#[test]
fn a_sync_after_a_long_pause_reads_everything_since_the_last_one() {
    block_on(async {
        let fx = Fixture::selling().await;
        let last = fx.orders.last_sync().await.unwrap().unwrap();
        // The computer stays off for three days.
        fx.clock.advance(TimeDelta::days(1));
        let first = fx.clock.now();
        fx.channel
            .sells(order("2000008", first, vec![sold("MLB1", None, 1)]));
        fx.clock.advance(TimeDelta::days(2));
        let second = fx.clock.now();
        fx.channel
            .sells(order("2000009", second, vec![sold("MLB2", Some("71"), 1)]));

        let synced = fx.sync().await;

        assert_eq!(fx.asked_since(), last - TimeDelta::minutes(10));
        assert_eq!(synced.new, 2);
        assert_eq!(
            synced
                .arrived
                .iter()
                .map(|order| order.sold.id.as_str())
                .collect::<Vec<_>>(),
            ["2000009", "2000008"]
        );
        assert_eq!(fx.units(fone()).await, 9);
        assert_eq!(fx.units(capa()).await, 3);
    });
}

#[test]
fn an_order_changed_in_the_channel_is_updated_in_place() {
    block_on(async {
        let fx = Fixture::selling().await;
        let at = fx.later(1);
        fx.channel
            .sells(order("2000010", at, vec![sold("MLB1", None, 1)]));
        fx.sync().await;

        let shipped_at = fx.later(30);
        fx.channel.change_order("2000010", shipped_at, |found| {
            if let Some(shipment) = found.shipment.as_mut() {
                shipment.status = ShipmentStatus::Shipped;
            }
        });
        let synced = fx.sync().await;

        assert_eq!((synced.new, synced.changed, synced.taken), (0, 1, 0));
        let kept = fx.order("2000010").await;
        assert_eq!(
            kept.sold.shipment.map(|shipment| shipment.status),
            Some(ShipmentStatus::Shipped)
        );
        assert_eq!(kept.sold.updated_at, shipped_at);
        assert_eq!(fx.units(fone()).await, 9);
    });
}

#[test]
fn an_order_the_channel_repeats_in_one_answer_counts_once() {
    block_on(async {
        let fx = Fixture::selling().await;
        let at = fx.later(1);
        let sale = order("2000011", at, vec![sold("MLB1", None, 1)]);
        fx.channel.sells(sale.clone());
        fx.channel.orders.lock().unwrap().push(sale);

        let synced = fx.sync().await;

        assert_eq!((synced.read, synced.new, synced.taken), (1, 1, 1));
        assert_eq!(fx.units(fone()).await, 9);
    });
}

#[test]
fn a_sale_sends_the_right_stock_to_the_listing() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.mirror.send(&fx.channel).await.unwrap();
        fx.channel.stocks_set.lock().unwrap().clear();
        let at = fx.later(1);
        // Mercado Livre takes the 3 units off the listing at once: 7 left.
        fx.channel
            .sells(order("2000012", at, vec![sold("MLB1", None, 3)]));

        // Orders first, then the stock (ADR 0017).
        fx.sync().await;
        let sent = fx.mirror.send(&fx.channel).await.unwrap();

        assert_eq!(sent.sent, 1);
        assert_eq!(
            *fx.channel.stocks_set.lock().unwrap(),
            [(
                "MLB1".to_owned(),
                vec![ChannelStock {
                    variation: None,
                    available_quantity: 7,
                }]
            )]
        );
        // Sending again writes nothing: the app and the channel agree.
        assert_eq!(fx.mirror.send(&fx.channel).await.unwrap().sent, 0);
    });
}

#[test]
fn a_sale_says_when_it_takes_a_product_to_its_reorder_point() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.inventory
            .set_reorder_point(fone(), Some(8))
            .await
            .unwrap();
        let at = fx.later(1);
        fx.channel
            .sells(order("2000013", at, vec![sold("MLB1", None, 1)]));
        fx.channel
            .sells(order("2000014", at, vec![sold("MLB1", None, 1)]));

        let synced = fx.sync().await;

        assert_eq!(
            synced.reached_reorder_point,
            [LowStock {
                product: fone(),
                quantity: 8,
                reorder_point: 8,
            }]
        );
    });
}

#[test]
fn the_buyer_data_goes_when_its_time_is_over_and_never_comes_back() {
    block_on(async {
        let fx = Fixture::selling().await;
        let at = fx.later(1);
        fx.channel
            .sells(order("2000015", at, vec![sold("MLB1", None, 1)]));
        fx.sync().await;
        let kept = fx.order("2000015").await;
        assert_eq!(
            kept.sold
                .buyer
                .and_then(|buyer| buyer.receiver)
                .map(|r| r.name),
            Some("Ana Souza".to_owned())
        );

        fx.clock.advance(TimeDelta::days(91));
        fx.sync().await;
        let erased = fx.order("2000015").await;
        assert_eq!(erased.sold.buyer, None);
        assert!(erased.buyer_data_erased_at.is_some());

        // The channel changes the old Order: its buyer stays erased.
        let changed_at = fx.later(1);
        fx.channel.change_order("2000015", changed_at, |found| {
            if let Some(shipment) = found.shipment.as_mut() {
                shipment.status = ShipmentStatus::Delivered;
            }
        });
        assert_eq!(fx.sync().await.changed, 1);
        let still = fx.order("2000015").await;
        assert_eq!(still.sold.buyer, None);
        assert_eq!(
            still.sold.shipment.map(|shipment| shipment.status),
            Some(ShipmentStatus::Delivered)
        );
    });
}

#[test]
fn keeping_the_buyer_data_for_less_time_erases_it_at_once() {
    block_on(async {
        let fx = Fixture::selling().await;
        assert_eq!(
            fx.orders.settings().await.unwrap(),
            OrderSettings {
                sync_every_minutes: 5,
                keep_buyer_data_days: 90,
            }
        );
        let at = fx.later(1);
        fx.channel
            .sells(order("2000016", at, vec![sold("MLB1", None, 1)]));
        fx.sync().await;
        fx.clock.advance(TimeDelta::days(10));
        assert_eq!(fx.orders.erase_expired_buyer_data().await.unwrap(), 0);

        let settings = OrderSettings {
            sync_every_minutes: 15,
            keep_buyer_data_days: 7,
        };
        fx.orders.save_settings(settings).await.unwrap();

        assert_eq!(fx.orders.settings().await.unwrap(), settings);
        assert_eq!(fx.order("2000016").await.sold.buyer, None);
    });
}

#[test]
fn settings_out_of_range_are_refused() {
    block_on(async {
        let fx = Fixture::new().await;
        for wrong in [
            OrderSettings {
                sync_every_minutes: 0,
                keep_buyer_data_days: 90,
            },
            OrderSettings {
                sync_every_minutes: 61,
                keep_buyer_data_days: 90,
            },
            OrderSettings {
                sync_every_minutes: 5,
                keep_buyer_data_days: 6,
            },
        ] {
            assert!(matches!(
                fx.orders.save_settings(wrong).await,
                Err(OrderError::InvalidSettings)
            ));
        }
        assert_eq!(
            fx.orders.settings().await.unwrap(),
            OrderSettings::default()
        );
    });
}

#[test]
fn a_failed_sync_changes_nothing_and_the_next_reads_from_the_same_place() {
    block_on(async {
        let fx = Fixture::selling().await;
        let last = fx.orders.last_sync().await.unwrap();
        let at = fx.later(1);
        fx.channel
            .sells(order("2000017", at, vec![sold("MLB1", None, 1)]));
        *fx.channel.failure.lock().unwrap() = Some(PlatformError::RateLimited);

        fx.later(5);
        assert!(matches!(
            fx.orders.sync(&fx.channel).await,
            Err(OrderError::Platform(PlatformError::RateLimited))
        ));
        assert_eq!(fx.orders.last_sync().await.unwrap(), last);
        assert!(fx.orders.orders().await.unwrap().is_empty());

        *fx.channel.failure.lock().unwrap() = None;
        let synced = fx.sync().await;
        assert_eq!(fx.asked_since(), last.unwrap() - TimeDelta::minutes(10));
        assert_eq!(synced.arrived.len(), 1);
        assert_eq!(fx.units(fone()).await, 9);
    });
}

fn fone_listing() -> ChannelListing {
    listing("MLB1", "Fone Bluetooth")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// However often the same Orders are read, and in whatever order they
    /// arrive, the stock loses exactly the units of the ones not cancelled.
    #[test]
    fn rereading_orders_never_takes_their_units_twice(
        sales in prop::collection::vec((1u32..4, any::<bool>()), 1..6),
        rereads in 1usize..4,
    ) {
        block_on(async {
            let fx = Fixture::new().await;
            fx.channel.has(vec![fone_listing()]);
            fx.listings.sync(&fx.channel).await.unwrap();
            let id = fx.listings.listings().await.unwrap()[0].id;
            fx.listings.link(id, fone()).await.unwrap();
            fx.receive(fone(), 30).await;
            fx.sync().await;
            let mut expected = 30;
            for (n, (units, cancelled)) in sales.iter().enumerate() {
                let at = fx.later(1);
                let mut sale = order(&format!("30{n}"), at, vec![sold("MLB1", None, *units)]);
                if *cancelled {
                    sale.status = OrderStatus::Cancelled;
                } else {
                    expected -= i64::from(*units);
                }
                fx.channel.sells(sale);
                if n % 2 == 0 {
                    fx.sync().await;
                }
            }
            for _ in 0..rereads {
                fx.sync().await;
            }
            prop_assert_eq!(fx.units(fone()).await, expected);
            prop_assert_eq!(
                fx.sales_in_ledger(fone()).await,
                sales.iter().filter(|(_, cancelled)| !cancelled).count()
            );
            prop_assert_eq!(fx.orders.orders().await.unwrap().len(), sales.len());
            Ok(())
        })?;
    }
}
