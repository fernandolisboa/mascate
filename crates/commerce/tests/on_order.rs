//! On-Order Listings (#207, #208, ADR 0038): the owner marks a Mercado
//! Livre listing to sell before buying; the channel shows the days to be
//! ready and the listing shows the orders that still fit instead of a
//! Stock Share. A sale on it asks for the purchase, linked to the Order
//! line, and waits for its units. Through commerce's public interface with a real temporary
//! database, the real Inventory and an in-memory Sales Channel.

mod common;

use std::sync::Arc;

use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    ChannelListing, ChannelOrder, ChannelStock, LineStock, ListingError, Listings, MirroredStock,
    NewPurchaseLine, NewPurchaseOrder, OnOrderSale, OnOrderTerms, OrderSlots, OrderStatus,
    OrderSync, Orders, PurchaseOrder, PurchaseOrderError, PurchaseOrderStatus, PurchaseOrders,
    Receiving, SalesChannel, ShipmentStatus, StockMirror, StockShort,
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
        let order = self.purchases.create(fones(units)).await.unwrap();
        self.receive_all(&order).await;
    }

    /// Takes every unit of `order` into stock.
    async fn receive_all(&self, order: &PurchaseOrder) {
        self.later();
        self.purchases
            .receive(
                order.id,
                HOME_LOCATION,
                &[Receiving {
                    line: order.lines[0].id,
                    quantity: order.lines[0].quantity,
                }],
            )
            .await
            .unwrap();
    }

    /// A buyer buys `units` of `item`, which the Orders read; returns the
    /// Order's id.
    async fn buys(&self, item: &str, units: u32) -> String {
        self.buys_and_syncs(item, units).await.0
    }

    /// A buyer buys `units` of `item`; returns the Order's id and what the
    /// Order Sync that read it did.
    async fn buys_and_syncs(&self, item: &str, units: u32) -> (String, OrderSync) {
        self.later();
        let id = {
            let mut sales = self.sales.lock().unwrap();
            *sales += 1;
            format!("200000000{}", *sales)
        };
        self.ml
            .sells(order(&id, self.clock.now(), vec![sold(item, None, units)]));
        (id, self.sync().await)
    }

    async fn sync(&self) -> OrderSync {
        self.later();
        self.orders.sync(&self.ml).await.unwrap()
    }

    async fn on_order_sales(&self) -> Vec<OnOrderSale> {
        self.orders.on_order_sales().await.unwrap()
    }

    /// Buys the units `sale` misses, linked to its line.
    async fn buy_for(&self, sale: &OnOrderSale) -> Result<PurchaseOrder, PurchaseOrderError> {
        self.later();
        self.purchases
            .create_for_order_line(sale.line.id, fones(sale.missing))
            .await
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

/// `units` fones bought from the supplier at R$ 20,00 each.
fn fones(units: u32) -> NewPurchaseOrder {
    NewPurchaseOrder {
        supplier: supplier(),
        ordered_on: NaiveDate::from_ymd_opt(2026, 10, 27).unwrap(),
        freight: brl("0"),
        lines: vec![NewPurchaseLine {
            product: fone(),
            quantity: units,
            unit_price: brl("20.00"),
        }],
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

#[test]
fn a_sale_on_an_on_order_listing_asks_for_the_purchase_never_says_sold_without_stock() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;
        fx.mark("MLB1", terms(15, 3)).await.unwrap();

        let (id, synced) = fx.buys_and_syncs("MLB1", 2).await;

        assert!(synced.sold_without_stock.is_empty());
        assert_eq!(synced.sold_on_order.len(), 1);
        assert_eq!(synced.sold_on_order[0].order.sold.id, id);
        let sales = fx.on_order_sales().await;
        assert_eq!(sales.len(), 1);
        let sale = &sales[0];
        assert_eq!(sale.order.sold.id, id);
        assert_eq!(sale.line.stock, LineStock::Short(StockShort::OnOrder));
        assert_eq!(
            (sale.product, sale.missing, sale.purchase),
            (fone(), 2, None)
        );
        assert_eq!(
            sale.order.dispatch_by(),
            sale.order.sold.shipment.as_ref().unwrap().dispatch_by
        );
        // The next Syncs keep it waiting, and never tell it again.
        let again = fx.sync().await;
        assert!(again.sold_on_order.is_empty() && again.sold_without_stock.is_empty());
        assert_eq!(fx.on_order_sales().await.len(), 1);
    });
}

#[test]
fn a_sale_before_the_mark_or_after_it_comes_off_stays_sold_without_stock() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;
        let (_, before) = fx.buys_and_syncs("MLB1", 1).await;
        fx.mark("MLB1", terms(15, 3)).await.unwrap();
        let (_, marked) = fx.buys_and_syncs("MLB1", 1).await;
        fx.unmark("MLB1").await.unwrap();
        let (_, after) = fx.buys_and_syncs("MLB1", 1).await;

        assert_eq!(before.sold_without_stock.len(), 1);
        assert_eq!(marked.sold_on_order.len(), 1);
        assert!(marked.sold_without_stock.is_empty());
        assert_eq!(after.sold_without_stock.len(), 1);
        assert!(after.sold_on_order.is_empty());
        // The sale made on order still waits for its purchase once the
        // listing sells from stock again.
        let sales = fx.on_order_sales().await;
        assert_eq!(sales.len(), 1);
        assert_eq!(
            sales[0].order.sold.id,
            marked.sold_on_order[0].order.sold.id
        );
    });
}

#[test]
fn the_units_on_hand_go_to_the_oldest_sales_first_and_the_rest_is_missing() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;
        fx.mark("MLB1", terms(15, 5)).await.unwrap();
        fx.buys("MLB1", 3).await;
        fx.buys("MLB1", 2).await;
        // One unit came in, too few for either sale to leave the stock.
        fx.receive(1).await;
        fx.sync().await;

        let missing: Vec<u32> = fx
            .on_order_sales()
            .await
            .iter()
            .map(|sale| sale.missing)
            .collect();

        assert_eq!(missing, [2, 2]);
    });
}

#[test]
fn a_purchase_bought_for_the_sale_is_linked_and_its_receipt_lets_the_next_sync_take_the_units() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;
        fx.mark("MLB1", terms(15, 3)).await.unwrap();
        let id = fx.buys("MLB1", 2).await;
        let sale = fx.on_order_sales().await.remove(0);

        let bought = fx.buy_for(&sale).await.unwrap();

        assert_eq!(bought.order_line, Some(sale.line.id));
        assert_eq!(bought.units(), 2);
        let waiting = fx.on_order_sales().await.remove(0);
        assert_eq!(waiting.purchase, Some(bought.id));
        assert!(matches!(
            fx.buy_for(&waiting).await,
            Err(PurchaseOrderError::AlreadyBought)
        ));

        fx.receive_all(&bought).await;
        let synced = fx.sync().await;

        assert_eq!(synced.taken, 1);
        assert!(fx.on_order_sales().await.is_empty());
        let order = fx
            .orders
            .orders()
            .await
            .unwrap()
            .into_iter()
            .find(|order| order.sold.id == id)
            .unwrap();
        assert!(matches!(
            order.lines[0].stock,
            LineStock::Taken { product, .. } if product == fone()
        ));
    });
}

#[test]
fn a_cancelled_purchase_lets_the_owner_buy_for_the_sale_again() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;
        fx.mark("MLB1", terms(15, 3)).await.unwrap();
        fx.buys("MLB1", 1).await;
        let sale = fx.on_order_sales().await.remove(0);
        let first = fx.buy_for(&sale).await.unwrap();

        fx.purchases.cancel(first.id).await.unwrap();

        assert_eq!(fx.on_order_sales().await[0].purchase, None);
        let second = fx.buy_for(&sale).await.unwrap();
        assert_eq!(fx.on_order_sales().await[0].purchase, Some(second.id));
        // Editing the purchase keeps it linked.
        let edited = fx.purchases.update(second.id, fones(2)).await.unwrap();
        assert_eq!(edited.order_line, Some(sale.line.id));
    });
}

#[test]
fn a_purchase_is_never_linked_to_a_line_the_orders_do_not_have() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;
        let unknown = RecordId::from_u128(9_999);

        let refused = fx.purchases.create_for_order_line(unknown, fones(1)).await;

        assert!(matches!(
            refused,
            Err(PurchaseOrderError::UnknownOrderLine(line)) if line == unknown
        ));
        assert!(fx.purchases.purchase_orders().await.unwrap().is_empty());
    });
}

#[test]
fn a_cancelled_order_drops_its_sale_and_leaves_its_purchase_as_the_owner_left_it() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;
        fx.mark("MLB1", terms(15, 3)).await.unwrap();
        let id = fx.buys("MLB1", 1).await;
        let sale = fx.on_order_sales().await.remove(0);
        let bought = fx.buy_for(&sale).await.unwrap();

        fx.order_changes(&id, |order| order.status = OrderStatus::Cancelled)
            .await;

        assert!(fx.on_order_sales().await.is_empty());
        let kept = fx.purchases.purchase_order(bought.id).await.unwrap();
        assert_eq!(kept.status(), PurchaseOrderStatus::Purchased);
        assert_eq!(kept.order_line, Some(sale.line.id));
    });
}

#[test]
fn the_sale_closest_to_its_dispatch_deadline_comes_first() {
    block_on(async {
        let fx = Fixture::selling(vec![empty("MLB1")]).await;
        fx.mark("MLB1", terms(15, 3)).await.unwrap();
        let later = fx.buys("MLB1", 1).await;
        let sooner = fx.buys("MLB1", 1).await;
        fx.order_changes(&later, |order| {
            let shipment = order.shipment.as_mut().unwrap();
            shipment.dispatch_by = shipment.dispatch_by.map(|by| by + TimeDelta::days(10));
        })
        .await;

        let ids: Vec<String> = fx
            .on_order_sales()
            .await
            .into_iter()
            .map(|sale| sale.order.sold.id)
            .collect();

        assert_eq!(ids, [sooner, later]);
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
