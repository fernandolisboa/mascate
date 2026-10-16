//! Orders through commerce's public interface with a real temporary
//! database, the real Inventory and an in-memory Sales Channel: each Order
//! is kept once by the channel's id, its units leave the stock once and
//! come back once (by themselves on a cancellation before shipping, on the
//! owner's word for a return), the Sync after a long pause reads
//! everything since the last one, late Orders stand out, labels print, and
//! the buyer's data goes when its time is over.

mod common;

use std::sync::Arc;

use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    ChannelListing, ChannelReturn, ChannelStock, DispatchDue, LineStock, Listings, NewPurchaseLine,
    NewPurchaseOrder, Order, OrderError, OrderSettings, OrderStatus, Orders, PurchaseOrders,
    Receiving, ReturnReceipt, ReturnStatus, ReturnedItem, ShipmentStatus, StockMirror, StockShort,
    UnitsBack,
};
use mascate_inventory::{HOME_LOCATION, Inventory, LowStock, MovementReason};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, PlatformError, RecordId, SalesChannelKind, Timestamp};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;

use common::{Channel, brl, listing, order, sold, variation};

fn back(awaiting: u32, restocked: u32, unsellable: u32) -> UnitsBack {
    UnitsBack {
        awaiting,
        restocked,
        unsellable,
    }
}

fn returning(id: &str, status: ReturnStatus, item: &str, quantity: u32) -> ChannelReturn {
    ChannelReturn {
        id: id.into(),
        status,
        items: vec![ReturnedItem {
            item: item.into(),
            variation: None,
            quantity,
        }],
    }
}

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

    /// Sells `units` fones in the Order `id` and syncs, so they leave the
    /// stock.
    async fn sold_fones(&self, id: &str, units: u32) -> Order {
        let at = self.later(1);
        self.channel
            .sells(order(id, at, vec![sold("MLB1", None, units)]));
        self.sync().await;
        self.order(id).await
    }

    /// Changes the Order `id` in the channel a minute from now.
    fn channel_changes(&self, id: &str, change: impl Fn(&mut mascate_commerce::ChannelOrder)) {
        let at = self.later(1);
        self.channel.change_order(id, at, change);
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
fn a_cancelled_order_takes_nothing_and_one_cancelled_before_shipping_puts_its_units_back() {
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

        // Cancelled while still waiting to ship: the units never left the
        // owner's hands, so they go back with nobody to confirm it.
        let changed_at = fx.later(1);
        fx.channel.change_order("2000007", changed_at, |found| {
            found.status = OrderStatus::Cancelled;
        });
        let synced = fx.sync().await;
        assert_eq!((synced.changed, synced.restocked), (1, 1));
        let kept = fx.order("2000007").await;
        assert_eq!(kept.sold.status, OrderStatus::Cancelled);
        let LineStock::Taken { movement, .. } = kept.lines[0].stock else {
            panic!("{:?}", kept.lines[0].stock);
        };
        assert_eq!(kept.lines[0].back, back(0, 2, 0));
        assert_eq!(fx.units(fone()).await, 10);
        let history = fx.inventory.history(fone()).await.unwrap();
        assert_eq!(
            history[0].movement.reason,
            MovementReason::SaleCancelled { order: kept.id }
        );
        assert_eq!(history[0].movement.quantity, 2);
        assert_eq!(history[1].movement.id, movement);
        assert_eq!(
            fx.inventory.average_cost(fone()).await.unwrap(),
            Some(brl("20"))
        );

        // Read again, the cancellation puts nothing back twice.
        assert_eq!(fx.sync().await.restocked, 0);
        assert_eq!(fx.units(fone()).await, 10);
        assert_eq!(fx.order("2000007").await.lines[0].back, back(0, 2, 0));
    });
}

#[test]
fn units_back_from_a_cancellation_go_back_to_the_listing() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sold_fones("2000030", 3).await;
        fx.mirror.send(&[&fx.channel]).await.unwrap();
        fx.channel.stocks_set.lock().unwrap().clear();
        fx.channel_changes("2000030", |found| found.status = OrderStatus::Cancelled);

        fx.sync().await;
        fx.mirror.send(&[&fx.channel]).await.unwrap();

        assert_eq!(
            *fx.channel.stocks_set.lock().unwrap(),
            [(
                "MLB1".to_owned(),
                vec![ChannelStock {
                    variation: None,
                    available_quantity: 10,
                }]
            )]
        );
    });
}

#[test]
fn an_order_cancelled_once_shipped_waits_for_the_owner_to_receive_it() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sold_fones("2000031", 2).await;
        fx.channel_changes("2000031", |found| {
            found.status = OrderStatus::Cancelled;
            found.shipment.as_mut().unwrap().status = ShipmentStatus::Shipped;
        });

        let synced = fx.sync().await;

        assert_eq!(synced.restocked, 0);
        let kept = fx.order("2000031").await;
        assert_eq!(kept.lines[0].back, back(2, 0, 0));
        assert_eq!(kept.units_awaiting_return(), 2);
        assert_eq!(fx.units(fone()).await, 8);

        let received = fx
            .orders
            .receive_return(kept.id, ReturnReceipt::BackInStock)
            .await
            .unwrap();

        assert_eq!((received.units, received.restocked), (2, 2));
        assert_eq!(fx.units(fone()).await, 10);
        let history = fx.inventory.history(fone()).await.unwrap();
        assert_eq!(
            history[0].movement.reason,
            MovementReason::SaleReturned { order: kept.id }
        );
        assert_eq!(fx.order("2000031").await.lines[0].back, back(0, 2, 0));
        // Nothing comes back twice: not by asking again, not by a Sync.
        assert!(matches!(
            fx.orders
                .receive_return(kept.id, ReturnReceipt::BackInStock)
                .await,
            Err(OrderError::NoReturnAwaiting)
        ));
        fx.sync().await;
        assert_eq!(fx.units(fone()).await, 10);
    });
}

#[test]
fn a_return_of_part_of_a_delivered_order_waits_for_its_units_only() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sold_fones("2000032", 3).await;
        fx.channel_changes("2000032", |found| {
            found.shipment.as_mut().unwrap().status = ShipmentStatus::Delivered;
            found.returns = vec![returning("R1", ReturnStatus::OnTheWay, "MLB1", 2)];
        });

        fx.sync().await;

        let kept = fx.order("2000032").await;
        assert_eq!(kept.sold.returns.len(), 1);
        assert_eq!(kept.lines[0].back, back(2, 0, 0));

        // They arrive damaged: the return closes, the stock stays.
        let received = fx
            .orders
            .receive_return(kept.id, ReturnReceipt::Unsellable)
            .await
            .unwrap();
        assert_eq!((received.units, received.restocked), (2, 0));
        assert_eq!(fx.units(fone()).await, 7);
        assert_eq!(fx.order("2000032").await.lines[0].back, back(0, 0, 2));

        // The channel then says the return was delivered: still settled.
        fx.channel_changes("2000032", |found| {
            found.returns[0].status = ReturnStatus::Delivered;
        });
        fx.sync().await;
        assert_eq!(fx.order("2000032").await.lines[0].back, back(0, 0, 2));
    });
}

#[test]
fn a_second_return_of_the_same_order_waits_for_its_own_units() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sold_fones("2000033", 3).await;
        fx.channel_changes("2000033", |found| {
            found.shipment.as_mut().unwrap().status = ShipmentStatus::Delivered;
            found.returns = vec![returning("R1", ReturnStatus::Delivered, "MLB1", 1)];
        });
        fx.sync().await;
        let id = fx.order("2000033").await.id;
        fx.orders
            .receive_return(id, ReturnReceipt::BackInStock)
            .await
            .unwrap();

        fx.channel_changes("2000033", |found| {
            found
                .returns
                .push(returning("R2", ReturnStatus::OnTheWay, "MLB1", 1));
        });
        fx.sync().await;

        assert_eq!(fx.order("2000033").await.lines[0].back, back(1, 1, 0));
        assert_eq!(fx.units(fone()).await, 8);
    });
}

#[test]
fn a_return_called_off_or_a_failed_delivery_changes_what_comes_back() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sold_fones("2000034", 2).await;
        fx.channel_changes("2000034", |found| {
            found.shipment.as_mut().unwrap().status = ShipmentStatus::Delivered;
            found.returns = vec![returning("R1", ReturnStatus::OnTheWay, "MLB1", 2)];
        });
        fx.sync().await;
        assert_eq!(fx.order("2000034").await.units_awaiting_return(), 2);

        fx.channel_changes("2000034", |found| {
            found.returns[0].status = ReturnStatus::Cancelled;
        });
        fx.sync().await;
        assert_eq!(fx.order("2000034").await.units_awaiting_return(), 0);

        // A shipment the carrier could not deliver comes back whole.
        fx.sold_fones("2000035", 3).await;
        fx.channel_changes("2000035", |found| {
            found.shipment.as_mut().unwrap().status = ShipmentStatus::NotDelivered;
        });
        fx.sync().await;
        assert_eq!(fx.order("2000035").await.lines[0].back, back(3, 0, 0));
        assert_eq!(fx.units(fone()).await, 5);
    });
}

#[test]
fn only_units_that_left_the_stock_come_back_to_it() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel.has(vec![fone_listing()]);
        fx.listings.sync(&fx.channel).await.unwrap();
        let id = fx.listings.listings().await.unwrap()[0].id;
        fx.listings.link(id, fone()).await.unwrap();
        fx.receive(fone(), 5).await;
        let yesterday = fx.clock.now() - TimeDelta::days(1);
        let mut before = order("2000036", yesterday, vec![sold("MLB1", None, 1)]);
        before.shipment.as_mut().unwrap().status = ShipmentStatus::NotDelivered;
        before.status = OrderStatus::Cancelled;
        fx.channel.sells(before);

        let synced = fx.sync().await;

        // Sold before the first Sync: the ledger never took it, so nothing
        // waits and nothing goes back.
        assert_eq!(synced.restocked, 0);
        let kept = fx.order("2000036").await;
        assert_eq!(kept.units_awaiting_return(), 0);
        assert!(matches!(
            fx.orders
                .receive_return(kept.id, ReturnReceipt::BackInStock)
                .await,
            Err(OrderError::NoReturnAwaiting)
        ));
        assert_eq!(fx.units(fone()).await, 5);
    });
}

#[test]
fn receiving_the_return_of_an_unknown_order_is_refused() {
    block_on(async {
        let fx = Fixture::new().await;
        let unknown = RecordId::from_u128(77);
        assert!(matches!(
            fx.orders
                .receive_return(unknown, ReturnReceipt::BackInStock)
                .await,
            Err(OrderError::UnknownOrder(order)) if order == unknown
        ));
    });
}

#[test]
fn late_orders_and_those_close_to_their_time_stand_out() {
    block_on(async {
        let fx = Fixture::selling().await;
        let now = fx.later(1);
        let due = |id: &str, hours: i64| {
            let mut sale = order(id, now, vec![sold("MLB1", None, 1)]);
            sale.shipment.as_mut().unwrap().dispatch_by = Some(now + TimeDelta::hours(hours));
            sale
        };
        fx.channel.sells(due("2000040", 30));
        fx.channel.sells(due("2000041", 20));
        fx.channel.sells(due("2000042", -2));
        let mut shipped = due("2000043", -5);
        shipped.shipment.as_mut().unwrap().status = ShipmentStatus::Shipped;
        fx.channel.sells(shipped);
        let mut cancelled = due("2000044", -5);
        cancelled.status = OrderStatus::Cancelled;
        fx.channel.sells(cancelled);
        fx.sync().await;

        let alerts = fx.orders.dispatch_alerts().await.unwrap();

        assert_eq!(
            alerts
                .iter()
                .map(|alert| (alert.order.sold.id.as_str(), alert.due))
                .collect::<Vec<_>>(),
            [
                ("2000042", DispatchDue::Late),
                ("2000041", DispatchDue::Soon)
            ]
        );

        // Warned two days ahead, the third one shows too.
        fx.orders
            .save_settings(OrderSettings {
                warn_before_dispatch_hours: 48,
                ..OrderSettings::default()
            })
            .await
            .unwrap();
        assert_eq!(fx.orders.dispatch_alerts().await.unwrap().len(), 3);

        // Time passes: the one close is now late.
        fx.clock.advance(TimeDelta::hours(21));
        let alerts = fx.orders.dispatch_alerts().await.unwrap();
        assert_eq!(
            alerts.iter().map(|alert| alert.due).collect::<Vec<_>>(),
            [DispatchDue::Late, DispatchDue::Late, DispatchDue::Soon]
        );
    });
}

#[test]
fn the_label_of_an_order_waiting_to_ship_comes_as_a_pdf() {
    block_on(async {
        let fx = Fixture::selling().await;
        let kept = fx.sold_fones("2000050", 1).await;

        let label = fx
            .orders
            .shipping_label(&fx.channel, kept.id)
            .await
            .unwrap();

        assert_eq!(label.file_name, "etiqueta-2000050.pdf");
        assert!(label.pdf.starts_with(b"%PDF-"));
        assert_eq!(*fx.channel.labels_asked.lock().unwrap(), ["42000050"]);
    });
}

#[test]
fn an_order_not_waiting_to_ship_has_no_label() {
    block_on(async {
        let fx = Fixture::selling().await;
        let kept = fx.sold_fones("2000051", 1).await;
        fx.channel_changes("2000051", |found| {
            found.shipment.as_mut().unwrap().status = ShipmentStatus::Shipped;
        });
        fx.sync().await;

        assert!(matches!(
            fx.orders.shipping_label(&fx.channel, kept.id).await,
            Err(OrderError::NoLabel)
        ));
        assert!(fx.channel.labels_asked.lock().unwrap().is_empty());
    });
}

#[test]
fn a_label_that_is_not_a_pdf_is_refused() {
    block_on(async {
        let fx = Fixture::selling().await;
        let kept = fx.sold_fones("2000052", 1).await;
        *fx.channel.label_answer.lock().unwrap() = Some(b"<html>login</html>".to_vec());

        assert!(matches!(
            fx.orders.shipping_label(&fx.channel, kept.id).await,
            Err(OrderError::Platform(PlatformError::Failed(_)))
        ));
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
        fx.mirror.send(&[&fx.channel]).await.unwrap();
        fx.channel.stocks_set.lock().unwrap().clear();
        let at = fx.later(1);
        // Mercado Livre takes the 3 units off the listing at once: 7 left.
        fx.channel
            .sells(order("2000012", at, vec![sold("MLB1", None, 3)]));

        // Orders first, then the stock (ADR 0017).
        fx.sync().await;
        let sent = fx.mirror.send(&[&fx.channel]).await.unwrap();

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
        assert_eq!(fx.mirror.send(&[&fx.channel]).await.unwrap().sent, 0);
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
                warn_before_dispatch_hours: 24,
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
            warn_before_dispatch_hours: 6,
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
                ..OrderSettings::default()
            },
            OrderSettings {
                sync_every_minutes: 61,
                ..OrderSettings::default()
            },
            OrderSettings {
                keep_buyer_data_days: 6,
                ..OrderSettings::default()
            },
            OrderSettings {
                warn_before_dispatch_hours: 0,
                ..OrderSettings::default()
            },
            OrderSettings {
                warn_before_dispatch_hours: 73,
                ..OrderSettings::default()
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

/// What can happen to an Order after its units left the stock.
#[derive(Debug, Clone, Copy)]
enum AfterSale {
    CancelBeforeShipping,
    CancelOnceShipped,
    FailedDelivery,
    Return(u32),
    Receive(bool),
    Reread,
}

fn after_sale() -> impl Strategy<Value = AfterSale> {
    prop_oneof![
        Just(AfterSale::CancelBeforeShipping),
        Just(AfterSale::CancelOnceShipped),
        Just(AfterSale::FailedDelivery),
        (1u32..4).prop_map(AfterSale::Return),
        any::<bool>().prop_map(AfterSale::Receive),
        Just(AfterSale::Reread),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Whatever the channel and the owner do after a sale, in any order and
    /// however often, no more units come back than left, and the stock is
    /// what came in minus what left plus what came back.
    #[test]
    fn units_never_come_back_more_than_they_left(
        units in 1u32..4,
        events in prop::collection::vec(after_sale(), 1..8),
    ) {
        block_on(async {
            let fx = Fixture::selling().await;
            let id = fx.sold_fones("2000060", units).await.id;
            let mut returns = 0;
            for event in events {
                match event {
                    AfterSale::CancelBeforeShipping => fx.channel_changes("2000060", |found| {
                        found.status = OrderStatus::Cancelled;
                    }),
                    AfterSale::CancelOnceShipped => fx.channel_changes("2000060", |found| {
                        found.status = OrderStatus::Cancelled;
                        found.shipment.as_mut().unwrap().status = ShipmentStatus::Shipped;
                    }),
                    AfterSale::FailedDelivery => fx.channel_changes("2000060", |found| {
                        found.shipment.as_mut().unwrap().status = ShipmentStatus::NotDelivered;
                    }),
                    AfterSale::Return(quantity) => {
                        returns += 1;
                        let id = format!("R{returns}");
                        fx.channel_changes("2000060", |found| {
                            found.shipment.as_mut().unwrap().status = ShipmentStatus::Delivered;
                            found
                                .returns
                                .push(returning(&id, ReturnStatus::OnTheWay, "MLB1", quantity));
                        });
                    }
                    AfterSale::Receive(fit) => {
                        let receipt = if fit {
                            ReturnReceipt::BackInStock
                        } else {
                            ReturnReceipt::Unsellable
                        };
                        match fx.orders.receive_return(id, receipt).await {
                            Ok(_) | Err(OrderError::NoReturnAwaiting) => {}
                            Err(error) => panic!("{error}"),
                        }
                    }
                    AfterSale::Reread => {}
                }
                fx.sync().await;
            }
            let kept = fx.order("2000060").await;
            let line = kept.lines[0].back;
            prop_assert!(line.restocked + line.unsellable <= units);
            prop_assert!(line.awaiting + line.restocked + line.unsellable <= units);
            prop_assert_eq!(
                fx.units(fone()).await,
                10 - i64::from(units) + i64::from(line.restocked)
            );
            Ok(())
        })?;
    }
}

#[test]
fn units_sold_count_each_listing_with_its_variations_since_a_time_without_cancellations() {
    block_on(async {
        let fx = Fixture::selling().await;
        let before = fx.later(1);
        fx.channel
            .sells(order("2000020", before, vec![sold("MLB1", None, 5)]));
        let since = fx.later(10);
        let at = fx.later(1);
        fx.channel.sells(order(
            "2000021",
            at,
            vec![sold("MLB1", None, 1), sold("MLB2", Some("71"), 1)],
        ));
        fx.channel
            .sells(order("2000022", at, vec![sold("MLB2", Some("72"), 2)]));
        let mut cancelled = order("2000023", at, vec![sold("MLB1", None, 3)]);
        cancelled.status = OrderStatus::Cancelled;
        fx.channel.sells(cancelled);
        fx.sync().await;

        let sold = fx
            .orders
            .units_sold_since(SalesChannelKind::MercadoLivre, since)
            .await
            .unwrap();

        assert_eq!(
            sold.into_iter().collect::<Vec<_>>(),
            [("MLB1".to_owned(), 1), ("MLB2".to_owned(), 3)]
        );
    });
}

#[test]
fn an_order_sync_of_one_channel_never_touches_the_orders_of_another() {
    block_on(async {
        let fx = Fixture::selling().await;
        let amazon = Channel::on(SalesChannelKind::Amazon);
        amazon.has(vec![listing("MLB1", "Fone Bluetooth")]);
        fx.listings.sync(&amazon).await.unwrap();
        let on_amazon = fx
            .listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .find(|found| found.channel == SalesChannelKind::Amazon)
            .unwrap();
        fx.listings.link(on_amazon.id, fone()).await.unwrap();
        // Sold on Amazon after Mercado Livre's first Sync but before
        // Amazon's: the stock counted when Amazon connected had lost it.
        let before = fx.later(1);
        amazon.sells(order("2999999", before, vec![sold("MLB1", None, 1)]));
        fx.later(5);
        let first = fx.orders.sync(&amazon).await.unwrap();
        assert_eq!((first.new, first.taken), (1, 0));
        assert!(first.arrived.is_empty());

        // The same Order id on both channels is two Orders.
        let at = fx.later(1);
        fx.channel
            .sells(order("3000001", at, vec![sold("MLB1", None, 2)]));
        amazon.sells(order("3000001", at, vec![sold("MLB1", None, 1)]));
        let on_ml = fx.sync().await;
        assert_eq!((on_ml.read, on_ml.new, on_ml.taken), (1, 1, 1));
        fx.later(5);
        let synced = fx.orders.sync(&amazon).await.unwrap();
        assert_eq!((synced.read, synced.new, synced.taken), (2, 1, 1));
        assert_eq!(synced.arrived[0].channel, SalesChannelKind::Amazon);
        assert_eq!(fx.units(fone()).await, 7);

        let orders = fx.orders.orders().await.unwrap();
        let of = |channel: SalesChannelKind, id: &str| {
            orders
                .iter()
                .find(|kept| kept.channel == channel && kept.sold.id == id)
                .unwrap_or_else(|| panic!("no Order {id} on {channel:?}"))
                .clone()
        };
        assert_eq!(orders.len(), 3);
        assert_eq!(of(SalesChannelKind::MercadoLivre, "3000001").units(), 2);
        assert_eq!(of(SalesChannelKind::Amazon, "3000001").units(), 1);
        assert_eq!(
            stock_of(&of(SalesChannelKind::Amazon, "2999999")),
            [LineStock::BeforeFirstSync]
        );

        // Mercado Livre cancels its Order: Amazon's keeps its status and
        // its units out of the stock.
        fx.channel_changes("3000001", |found| found.status = OrderStatus::Cancelled);
        let cancelled = fx.sync().await;
        assert_eq!((cancelled.changed, cancelled.restocked), (1, 1));
        let amazon_order = fx
            .orders
            .orders()
            .await
            .unwrap()
            .into_iter()
            .find(|kept| kept.channel == SalesChannelKind::Amazon && kept.sold.id == "3000001")
            .unwrap();
        assert_eq!(amazon_order.sold.status, OrderStatus::Paid);
        assert!(matches!(
            amazon_order.lines[0].stock,
            LineStock::Taken { .. }
        ));
        assert_eq!(fx.units(fone()).await, 9);
    });
}

/// An Amazon channel whose listing "FONE-1" sells the fones, synced and
/// linked, with its first Order Sync done.
async fn on_amazon(fx: &Fixture) -> Channel {
    let amazon = Channel::on(SalesChannelKind::Amazon);
    amazon.has(vec![listing("FONE-1", "Fone Bluetooth")]);
    fx.listings.sync(&amazon).await.unwrap();
    let found = fx
        .listings
        .listings()
        .await
        .unwrap()
        .into_iter()
        .find(|found| found.channel == SalesChannelKind::Amazon)
        .unwrap();
    fx.listings.link(found.id, fone()).await.unwrap();
    fx.later(5);
    fx.orders.sync(&amazon).await.unwrap();
    amazon
}

#[test]
fn an_amazon_order_read_twice_changes_nothing_and_takes_its_units_once() {
    block_on(async {
        let fx = Fixture::selling().await;
        let amazon = on_amazon(&fx).await;
        let at = fx.later(1);
        amazon.sells(order(
            "701-0000001-0000001",
            at,
            vec![sold("FONE-1", None, 2)],
        ));

        fx.later(5);
        let first = fx.orders.sync(&amazon).await.unwrap();
        fx.later(5);
        let again = fx.orders.sync(&amazon).await.unwrap();

        assert_eq!((first.new, first.taken), (1, 1));
        assert_eq!(
            (again.read, again.new, again.changed, again.taken),
            (1, 0, 0, 0)
        );
        assert_eq!(fx.units(fone()).await, 8);
        assert_eq!(fx.sales_in_ledger(fone()).await, 1);
    });
}

#[test]
fn a_confirmed_shipment_sends_the_carrier_and_tracking_code_once() {
    block_on(async {
        let fx = Fixture::selling().await;
        let amazon = on_amazon(&fx).await;
        let at = fx.later(1);
        amazon.sells(order(
            "701-0000002-0000002",
            at,
            vec![sold("FONE-1", None, 1)],
        ));
        fx.later(5);
        fx.orders.sync(&amazon).await.unwrap();
        let sold_order = fx.order("701-0000002-0000002").await;

        let now = fx.later(30);
        let sent = fx
            .orders
            .confirm_shipment(&amazon, sold_order.id, " Correios ", " AA123456789BR ")
            .await
            .unwrap();

        assert_eq!(sent.carrier, "Correios");
        assert_eq!(sent.tracking_code, "AA123456789BR");
        assert_eq!(sent.shipped_at, now);
        assert_eq!(
            *amazon.shipments_confirmed.lock().unwrap(),
            [("701-0000002-0000002".to_owned(), sent)]
        );
        let shipped = fx.order("701-0000002-0000002").await;
        assert_eq!(
            shipped.sold.shipment.map(|shipment| shipment.status),
            Some(ShipmentStatus::Shipped)
        );
        // Already shipped: refused before Amazon is asked again.
        assert!(matches!(
            fx.orders
                .confirm_shipment(&amazon, sold_order.id, "Correios", "AA123456789BR")
                .await,
            Err(OrderError::NotAwaitingShipment)
        ));
        assert_eq!(amazon.shipments_confirmed.lock().unwrap().len(), 1);
    });
}

#[test]
fn a_shipment_is_not_confirmed_without_tracking_on_a_cancelled_order_or_on_another_channel() {
    block_on(async {
        let fx = Fixture::selling().await;
        let amazon = on_amazon(&fx).await;
        let at = fx.later(1);
        amazon.sells(order(
            "701-0000003-0000003",
            at,
            vec![sold("FONE-1", None, 1)],
        ));
        amazon.sells(order(
            "701-0000004-0000004",
            at,
            vec![sold("FONE-1", None, 1)],
        ));
        fx.later(5);
        fx.orders.sync(&amazon).await.unwrap();
        let ml_order = fx.sold_fones("2000001", 1).await;
        let waiting = fx.order("701-0000003-0000003").await;

        for (carrier, tracking) in [("Correios", "  "), ("", "AA1"), ("Correios", "AA\n1")] {
            assert!(matches!(
                fx.orders
                    .confirm_shipment(&amazon, waiting.id, carrier, tracking)
                    .await,
                Err(OrderError::NoTracking)
            ));
        }
        assert!(matches!(
            fx.orders
                .confirm_shipment(&amazon, ml_order.id, "Correios", "AA1")
                .await,
            Err(OrderError::OtherChannel { .. })
        ));
        let at = fx.later(1);
        amazon.change_order("701-0000004-0000004", at, |found| {
            found.status = OrderStatus::Cancelled;
        });
        fx.later(5);
        fx.orders.sync(&amazon).await.unwrap();
        let cancelled = fx.order("701-0000004-0000004").await;
        assert!(matches!(
            fx.orders
                .confirm_shipment(&amazon, cancelled.id, "Correios", "AA1")
                .await,
            Err(OrderError::NotAwaitingShipment)
        ));
        // A channel that fails leaves the Order waiting for the dispatch.
        *amazon.failure.lock().unwrap() = Some(PlatformError::RateLimited);
        assert!(matches!(
            fx.orders
                .confirm_shipment(&amazon, waiting.id, "Correios", "AA1")
                .await,
            Err(OrderError::Platform(PlatformError::RateLimited))
        ));
        assert_eq!(
            fx.order("701-0000003-0000003")
                .await
                .sold
                .shipment
                .map(|shipment| shipment.status),
            Some(ShipmentStatus::ReadyToShip)
        );
        assert!(amazon.shipments_confirmed.lock().unwrap().is_empty());
    });
}

#[test]
fn the_amazon_invoice_reminder_is_raised_while_an_amazon_order_stands() {
    block_on(async {
        let fx = Fixture::selling().await;
        let amazon = on_amazon(&fx).await;
        fx.sold_fones("2000002", 1).await;
        assert!(fx.orders.raised_reminders().await.unwrap().is_empty());

        let at = fx.later(1);
        amazon.sells(order(
            "701-0000005-0000005",
            at,
            vec![sold("FONE-1", None, 1)],
        ));
        fx.later(5);
        fx.orders.sync(&amazon).await.unwrap();
        assert_eq!(
            fx.orders.raised_reminders().await.unwrap(),
            [mascate_commerce::AMAZON_SALE_INVOICE.key]
        );

        let at = fx.later(1);
        amazon.change_order("701-0000005-0000005", at, |found| {
            found.status = OrderStatus::Cancelled;
        });
        fx.later(5);
        fx.orders.sync(&amazon).await.unwrap();
        assert!(fx.orders.raised_reminders().await.unwrap().is_empty());
    });
}

/// A Shopee channel whose item "800001" sells the fones, synced and linked,
/// with its first Order Sync done.
async fn on_shopee(fx: &Fixture) -> Channel {
    let shopee = Channel::on(SalesChannelKind::Shopee);
    shopee.has(vec![listing("800001", "Fone Bluetooth")]);
    fx.listings.sync(&shopee).await.unwrap();
    let found = fx
        .listings
        .listings()
        .await
        .unwrap()
        .into_iter()
        .find(|found| found.channel == SalesChannelKind::Shopee)
        .unwrap();
    fx.listings.link(found.id, fone()).await.unwrap();
    fx.later(5);
    fx.orders.sync(&shopee).await.unwrap();
    shopee
}

#[test]
fn a_shopee_order_read_in_two_overlapping_windows_changes_nothing_and_takes_its_units_once() {
    block_on(async {
        let fx = Fixture::selling().await;
        let shopee = on_shopee(&fx).await;
        let at = fx.later(1);
        let sold_on_shopee = order("261014RDY0001A", at, vec![sold("800001", None, 2)]);
        shopee.sells(sold_on_shopee.clone());
        // Shopee's windows meet: the Order on the line between them comes
        // in both.
        shopee.orders.lock().unwrap().push(sold_on_shopee);

        fx.later(5);
        let first = fx.orders.sync(&shopee).await.unwrap();
        // The next Sync reads again from a little before this one.
        fx.later(5);
        let again = fx.orders.sync(&shopee).await.unwrap();

        assert_eq!((first.read, first.new, first.taken), (1, 1, 1));
        assert_eq!(
            (again.read, again.new, again.changed, again.taken),
            (1, 0, 0, 0)
        );
        assert_eq!(fx.units(fone()).await, 8);
        assert_eq!(fx.sales_in_ledger(fone()).await, 1);
    });
}

#[test]
fn a_shopee_return_with_units_waits_for_the_owner_and_puts_them_back_once() {
    block_on(async {
        let fx = Fixture::selling().await;
        let shopee = on_shopee(&fx).await;
        let at = fx.later(1);
        shopee.sells(order("260925RET0004D", at, vec![sold("800001", None, 2)]));
        fx.later(5);
        fx.orders.sync(&shopee).await.unwrap();
        let at = fx.later(1);
        shopee.change_order("260925RET0004D", at, |found| {
            found.shipment.as_mut().unwrap().status = ShipmentStatus::Delivered;
            found.returns = vec![returning(
                "2610130000001",
                ReturnStatus::OnTheWay,
                "800001",
                1,
            )];
        });
        fx.later(5);
        fx.orders.sync(&shopee).await.unwrap();

        let waiting = fx.order("260925RET0004D").await;
        assert_eq!(waiting.units_awaiting_return(), 1);
        assert_eq!(fx.units(fone()).await, 8);

        let received = fx
            .orders
            .receive_return(waiting.id, ReturnReceipt::BackInStock)
            .await
            .unwrap();
        assert_eq!((received.units, received.restocked), (1, 1));
        assert_eq!(fx.units(fone()).await, 9);

        // Shopee accepts it later: read again, nothing comes back twice.
        let at = fx.later(1);
        shopee.change_order("260925RET0004D", at, |found| {
            found.returns[0].status = ReturnStatus::Delivered;
        });
        fx.later(5);
        fx.orders.sync(&shopee).await.unwrap();
        let settled = fx.order("260925RET0004D").await;
        assert_eq!(settled.lines[0].back, back(0, 1, 0));
        assert!(matches!(
            fx.orders
                .receive_return(settled.id, ReturnReceipt::BackInStock)
                .await,
            Err(OrderError::NoReturnAwaiting)
        ));
        assert_eq!(fx.units(fone()).await, 9);
    });
}

#[test]
fn a_shopee_order_waiting_for_its_invoice_raises_the_reminder_and_keeps_its_time_to_ship() {
    block_on(async {
        let fx = Fixture::selling().await;
        let shopee = on_shopee(&fx).await;
        let at = fx.later(1);
        let mut waiting = order("261015NFE0002B", at, vec![sold("800001", None, 1)]);
        let shipment = waiting.shipment.as_mut().unwrap();
        shipment.status = ShipmentStatus::AwaitingInvoice;
        shipment.dispatch_by = Some(at + TimeDelta::hours(2));
        shopee.sells(waiting);
        fx.later(5);
        fx.orders.sync(&shopee).await.unwrap();

        // The units leave the stock: the NF-e never holds the sale back.
        assert_eq!(fx.units(fone()).await, 9);
        assert_eq!(
            fx.orders.raised_reminders().await.unwrap(),
            [mascate_commerce::SHOPEE_SALE_INVOICE.key]
        );
        let alerts = fx.orders.dispatch_alerts().await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].order.sold.id, "261015NFE0002B");
        assert_eq!(alerts[0].due, DispatchDue::Soon);
        // Shopee prints no label before the NF-e.
        let kept = fx.order("261015NFE0002B").await;
        assert!(matches!(
            fx.orders.shipping_label(&shopee, kept.id).await,
            Err(OrderError::NoLabel)
        ));

        // The owner sent the NF-e: Shopee lets it ship.
        let at = fx.later(1);
        shopee.change_order("261015NFE0002B", at, |found| {
            found.shipment.as_mut().unwrap().status = ShipmentStatus::ReadyToShip;
        });
        fx.later(5);
        fx.orders.sync(&shopee).await.unwrap();
        assert!(fx.orders.raised_reminders().await.unwrap().is_empty());
        assert_eq!(fx.units(fone()).await, 9);
    });
}
