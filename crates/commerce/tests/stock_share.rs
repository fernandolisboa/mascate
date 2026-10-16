//! The Stock Share (#125, ADR 0033): a Product's units on hand are dealt
//! across its Listings on every Sales Channel, so the same unit is never on
//! sale in two places, even with sales coming in on two channels between
//! Syncs. Through commerce's public interface with a real temporary
//! database, the real Inventory and two in-memory Sales Channels.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    ChannelListing, ChannelRanking, LineStock, ListingStatus, Listings, MirroredStock,
    NewPurchaseLine, NewPurchaseOrder, Orders, PurchaseOrders, Receiving, SalesChannel,
    StockMirror, StockSend, StockShort,
};
use mascate_inventory::{HOME_LOCATION, Inventory, StockAdjustment};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, PlatformError, RecordId, SalesChannelKind};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;

use common::{Channel, StockLog, brl, listing, order, sold};

const ML: SalesChannelKind = SalesChannelKind::MercadoLivre;
const AMAZON: SalesChannelKind = SalesChannelKind::Amazon;

fn fone() -> RecordId {
    RecordId::from_u128(1_000_010)
}

fn supplier() -> RecordId {
    RecordId::from_u128(1_000_001)
}

/// A listing with nothing on sale yet, so nothing shows more than the app
/// has before the first send.
fn empty(id: &str) -> ChannelListing {
    ChannelListing {
        available_quantity: 0,
        ..listing(id, "Fone Bluetooth")
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    inventory: Arc<Inventory>,
    purchases: PurchaseOrders,
    listings: Listings,
    orders: Orders,
    ranking: ChannelRanking,
    mirror: StockMirror,
    ml: Channel,
    amazon: Channel,
    /// Every stock set on either channel, in order.
    log: StockLog,
    sales: Mutex<u32>,
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
        migrate(
            &database,
            clock.as_ref(),
            &[mascate_inventory::MIGRATIONS, mascate_commerce::MIGRATIONS],
        )
        .await
        .unwrap();
        let ids = Arc::new(SequentialIds::default());
        let inventory = Arc::new(Inventory::new(database.clone(), clock.clone(), ids.clone()));
        let log = Arc::new(Mutex::new(Vec::new()));
        Self {
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
            ranking: ChannelRanking::new(database.clone(), clock.clone(), ids.clone()),
            mirror: StockMirror::new(database, inventory.clone(), clock.clone(), ids),
            inventory,
            clock,
            ml: Channel {
                stock_log: log.clone(),
                ..Channel::on(ML)
            },
            amazon: Channel {
                stock_log: log.clone(),
                ..Channel::on(AMAZON)
            },
            log,
            sales: Mutex::new(0),
        }
    }

    /// The fone sold on `ml` and `amazon` by these listings, synced and
    /// linked in that order, with both channels ranked and their first
    /// Order Sync done.
    async fn selling(ml: &[&str], amazon: &[&str]) -> Self {
        let fx = Self::new().await;
        fx.ranking.connected(AMAZON).await.unwrap();
        for (channel, ids) in [(&fx.ml, ml), (&fx.amazon, amazon)] {
            for id in ids {
                channel.listings.lock().unwrap().push(empty(id));
                fx.later();
                fx.listings.sync(channel).await.unwrap();
            }
            fx.orders.sync(channel).await.unwrap();
        }
        for listing in fx.listings.listings().await.unwrap() {
            fx.listings.link(listing.id, fone()).await.unwrap();
        }
        fx
    }

    fn channel(&self, kind: SalesChannelKind) -> &Channel {
        if kind == ML { &self.ml } else { &self.amazon }
    }

    fn later(&self) {
        self.clock.advance(TimeDelta::minutes(5));
    }

    async fn receive(&self, units: u32) {
        self.later();
        let order = self
            .purchases
            .create(NewPurchaseOrder {
                supplier: supplier(),
                ordered_on: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
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

    async fn send(&self) -> StockSend {
        self.later();
        let channels: [&dyn SalesChannel; 2] = [&self.ml, &self.amazon];
        self.mirror.send(&channels).await.unwrap()
    }

    async fn sync_orders(&self, kind: SalesChannelKind) -> mascate_commerce::OrderSync {
        self.later();
        self.orders.sync(self.channel(kind)).await.unwrap()
    }

    /// A buyer on `kind` buys `units` of `item`, when it shows them.
    fn buys(&self, kind: SalesChannelKind, item: &str, units: u32) -> bool {
        let channel = self.channel(kind);
        let shown = channel
            .listings
            .lock()
            .unwrap()
            .iter()
            .find(|listed| listed.id == item)
            .map_or(0, |listed| listed.available_quantity);
        if shown < units {
            return false;
        }
        self.later();
        let mut sales = self.sales.lock().unwrap();
        *sales += 1;
        channel.sells(order(
            &format!("{}-{}", kind.code(), *sales),
            self.clock.now(),
            vec![sold(item, None, units)],
        ));
        true
    }

    async fn on_hand(&self) -> u32 {
        self.inventory
            .stock()
            .await
            .unwrap()
            .products
            .iter()
            .find(|stock| stock.product == fone())
            .map_or(0, |stock| {
                u32::try_from(stock.valuation.quantity()).unwrap()
            })
    }

    /// What both channels show buyers, all listings together.
    fn shown(&self) -> u32 {
        [&self.ml, &self.amazon]
            .iter()
            .flat_map(|channel| channel.listings.lock().unwrap().clone())
            .map(|listed| listed.available_quantity)
            .sum()
    }

    /// Each channel listing's share, by its id.
    async fn shares(&self) -> HashMap<String, Option<u32>> {
        let listings = self.listings.listings().await.unwrap();
        self.mirror
            .mirrored()
            .await
            .unwrap()
            .into_iter()
            .map(|stock: MirroredStock| {
                let listing = listings.iter().find(|found| found.id == stock.listing);
                (listing.unwrap().listed.id.clone(), stock.share)
            })
            .collect()
    }

    fn logged(&self) -> Vec<(SalesChannelKind, String, u32)> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .map(|(kind, item, stock)| (*kind, item.clone(), stock[0].available_quantity))
            .collect()
    }
}

fn shares(pairs: &[(&str, u32)]) -> HashMap<String, Option<u32>> {
    pairs
        .iter()
        .map(|(id, share)| ((*id).to_owned(), Some(*share)))
        .collect()
}

#[test]
fn the_units_are_dealt_one_at_a_time_in_the_channels_order() {
    block_on(async {
        let fx = Fixture::selling(&["MLB1"], &["B0FONE1"]).await;
        fx.receive(5).await;

        assert_eq!(fx.shares().await, shares(&[("MLB1", 3), ("B0FONE1", 2)]));

        fx.ranking.rank(&[AMAZON, ML]).await.unwrap();
        assert_eq!(fx.shares().await, shares(&[("MLB1", 2), ("B0FONE1", 3)]));
    });
}

#[test]
fn with_one_unit_only_the_first_listing_shows_it() {
    block_on(async {
        let fx = Fixture::selling(&["MLB1", "MLB2"], &["B0FONE1"]).await;
        fx.receive(1).await;
        fx.send().await;

        assert_eq!(
            fx.shares().await,
            shares(&[("MLB1", 1), ("MLB2", 0), ("B0FONE1", 0)])
        );
        assert_eq!(fx.shown(), 1);
    });
}

#[test]
fn within_a_channel_the_oldest_listing_comes_first() {
    block_on(async {
        // MLB9 was synced before MLB1.
        let fx = Fixture::selling(&["MLB9", "MLB1"], &[]).await;
        fx.receive(3).await;

        assert_eq!(fx.shares().await, shares(&[("MLB9", 2), ("MLB1", 1)]));
    });
}

#[test]
fn shares_that_go_down_are_sent_before_shares_that_go_up() {
    block_on(async {
        let fx = Fixture::selling(&["MLB1"], &["B0FONE1"]).await;
        fx.receive(4).await;
        fx.send().await;
        fx.log.lock().unwrap().clear();

        // Amazon sells both its units: 2 left, one for each channel.
        assert!(fx.buys(AMAZON, "B0FONE1", 2));
        fx.sync_orders(AMAZON).await;
        let sent = fx.send().await;

        assert_eq!(
            sent,
            StockSend {
                sent: 2,
                failed: 0,
                held: 0
            }
        );
        assert_eq!(
            fx.logged(),
            [(ML, "MLB1".into(), 1), (AMAZON, "B0FONE1".into(), 1)]
        );
    });
}

#[test]
fn a_lowering_that_fails_holds_the_raise_on_the_other_channel_until_it_gets_through() {
    block_on(async {
        let fx = Fixture::selling(&["MLB1"], &["B0FONE1"]).await;
        fx.receive(4).await;
        fx.send().await;
        fx.log.lock().unwrap().clear();
        assert!(fx.buys(AMAZON, "B0FONE1", 2));
        fx.sync_orders(AMAZON).await;
        *fx.ml.stock_failure.lock().unwrap() = Some(PlatformError::Failed("timeout".into()));

        assert_eq!(
            fx.send().await,
            StockSend {
                sent: 0,
                failed: 1,
                held: 1
            }
        );
        // Mercado Livre still shows the 2 units on hand; Amazon none.
        assert!(fx.logged().is_empty());
        assert_eq!(fx.shown(), 2);

        // A send that fails again still holds it.
        assert_eq!(fx.send().await.held, 1);
        *fx.ml.stock_failure.lock().unwrap() = None;
        assert_eq!(
            fx.send().await,
            StockSend {
                sent: 2,
                failed: 0,
                held: 0
            }
        );
        assert_eq!(
            fx.logged(),
            [(ML, "MLB1".into(), 1), (AMAZON, "B0FONE1".into(), 1)]
        );
    });
}

#[test]
fn a_lowering_on_a_channel_left_out_of_the_send_holds_the_raise() {
    block_on(async {
        let fx = Fixture::selling(&["MLB1"], &["B0FONE1"]).await;
        fx.receive(4).await;
        fx.send().await;
        assert!(fx.buys(AMAZON, "B0FONE1", 2));
        fx.sync_orders(AMAZON).await;
        fx.log.lock().unwrap().clear();

        // Mercado Livre is not connected now.
        let only_amazon: [&dyn SalesChannel; 1] = [&fx.amazon];
        let sent = fx.mirror.send(&only_amazon).await.unwrap();

        assert_eq!(
            sent,
            StockSend {
                sent: 0,
                failed: 0,
                held: 1
            }
        );
        assert!(fx.logged().is_empty());
    });
}

#[test]
fn a_listing_paused_for_lack_of_stock_gets_its_share_back_when_units_arrive() {
    block_on(async {
        let fx = Fixture::selling(&["MLB1"], &["B0FONE1"]).await;
        fx.receive(2).await;
        fx.send().await;
        assert!(fx.buys(AMAZON, "B0FONE1", 1));
        fx.sync_orders(AMAZON).await;
        fx.send().await;
        // 1 unit left: Mercado Livre shows it, Amazon is paused for lack of
        // stock.
        let amazon = fx.amazon.listings.lock().unwrap()[0].clone();
        assert_eq!(amazon.status, mascate_commerce::ListingStatus::OutOfStock);
        assert_eq!(fx.shares().await, shares(&[("MLB1", 1), ("B0FONE1", 0)]));

        fx.receive(3).await;
        fx.send().await;

        assert_eq!(fx.shares().await, shares(&[("MLB1", 2), ("B0FONE1", 2)]));
        let amazon = fx.amazon.listings.lock().unwrap()[0].clone();
        assert_eq!(amazon.status, mascate_commerce::ListingStatus::Active);
        assert_eq!(amazon.available_quantity, 2);
    });
}

#[test]
fn a_listing_the_owner_paused_leaves_its_share_to_the_others() {
    block_on(async {
        let fx = Fixture::selling(&["MLB1"], &["B0FONE1"]).await;
        fx.receive(4).await;
        fx.send().await;
        let ml = fx
            .listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .find(|listing| listing.channel == ML)
            .unwrap();

        fx.listings.pause(ml.id, &fx.ml).await.unwrap();
        fx.send().await;

        let shares = fx.shares().await;
        assert_eq!(shares["MLB1"], None);
        assert_eq!(shares["B0FONE1"], Some(4));
    });
}

#[test]
fn on_a_channel_without_a_pause_the_owners_pause_is_zero_units_until_reactivated() {
    block_on(async {
        let mut fx = Fixture::selling(&["MLB1"], &["B0FONE1"]).await;
        fx.amazon.zero_pause = true;
        fx.receive(4).await;
        fx.send().await;
        let amazon = fx
            .listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .find(|listing| listing.channel == AMAZON)
            .unwrap();

        fx.listings.pause(amazon.id, &fx.amazon).await.unwrap();
        fx.later();
        // Amazon reports the zero units as out of stock; the pause stays.
        fx.listings.sync(&fx.amazon).await.unwrap();
        fx.send().await;

        let paused = fx.listings.listing(amazon.id).await.unwrap();
        assert_eq!(paused.listed.status, ListingStatus::Paused);
        let dealt = fx.shares().await;
        assert_eq!((dealt["MLB1"], dealt["B0FONE1"]), (Some(4), None));
        assert_eq!(fx.shown(), 4);

        // Reactivated with no units on Amazon: the next send brings them.
        fx.listings.reactivate(amazon.id, &fx.amazon).await.unwrap();
        fx.send().await;

        assert_eq!(fx.shares().await, shares(&[("MLB1", 2), ("B0FONE1", 2)]));
        let listed = fx.amazon.listings.lock().unwrap()[0].clone();
        assert_eq!(
            (listed.status, listed.available_quantity),
            (ListingStatus::Active, 2)
        );
        assert_eq!(fx.shown(), 4);
    });
}

#[test]
fn sold_without_stock_is_told_once_for_the_line() {
    block_on(async {
        let fx = Fixture::selling(&["MLB1"], &["B0FONE1"]).await;
        fx.receive(2).await;
        fx.send().await;
        // A unit lost on the shelf before the next send: the channels still
        // show 2, and both sell.
        fx.later();
        fx.inventory
            .adjust(fone(), HOME_LOCATION, StockAdjustment::Loss(1), None)
            .await
            .unwrap();
        assert!(fx.buys(ML, "MLB1", 1));
        assert!(fx.buys(AMAZON, "B0FONE1", 1));
        fx.sync_orders(ML).await;

        let synced = fx.sync_orders(AMAZON).await;
        assert_eq!(synced.sold_without_stock.len(), 1);
        let told = &synced.sold_without_stock[0];
        assert_eq!(told.order.channel, AMAZON);
        assert_eq!(told.line.sold.item, "B0FONE1");
        assert_eq!(
            told.line.stock,
            LineStock::Short(StockShort::NotEnoughStock)
        );

        // Still short on the next Sync, but told already.
        assert!(fx.sync_orders(AMAZON).await.sold_without_stock.is_empty());
        fx.receive(1).await;
        let covered = fx.sync_orders(AMAZON).await;
        assert!(covered.sold_without_stock.is_empty());
        assert_eq!(covered.taken, 1);
    });
}

/// What happens to the fone's stock next.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// A buyer on the channel buys units of one of its listings, when it
    /// shows them.
    Buys(SalesChannelKind, usize, u32),
    SyncOrders(SalesChannelKind),
    /// The app's round: every channel's Orders, then the stock that
    /// changed, as after a movement or on the Sync timer.
    Round,
    Receive(u32),
    /// The channel's stock sends fail, or work again.
    Down(SalesChannelKind, bool),
}

fn any_channel() -> impl Strategy<Value = SalesChannelKind> {
    prop_oneof![Just(ML), Just(AMAZON)]
}

fn any_step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => (any_channel(), 0usize..2, 1u32..4)
            .prop_map(|(kind, listing, units)| Step::Buys(kind, listing, units)),
        2 => any_channel().prop_map(Step::SyncOrders),
        2 => Just(Step::Round),
        1 => (1u32..6).prop_map(Step::Receive),
        1 => (any_channel(), any::<bool>()).prop_map(|(kind, down)| Step::Down(kind, down)),
    ]
}

const ML_LISTINGS: [&str; 2] = ["MLB1", "MLB2"];
const AMAZON_LISTINGS: [&str; 2] = ["B0FONE1", "B0FONE2"];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// For any units on hand and any Listings across two channels, the
    /// shares add up to the units on hand, none is negative, and in the
    /// round-robin order each is the next one's or one more.
    #[test]
    fn the_shares_add_up_to_the_units_on_hand_and_differ_by_at_most_one(
        units in 0u32..40,
        on_ml in 0usize..3,
        on_amazon in 0usize..3,
        amazon_first in any::<bool>(),
    ) {
        prop_assume!(on_ml + on_amazon > 0);
        block_on(async {
            let ml = &["MLB1", "MLB2", "MLB3"][..on_ml];
            let amazon = &["B0FONE1", "B0FONE2", "B0FONE3"][..on_amazon];
            let fx = Fixture::selling(ml, amazon).await;
            if amazon_first {
                fx.ranking.rank(&[AMAZON, ML]).await.unwrap();
            }
            fx.receive(units.max(1)).await;
            if units == 0 {
                fx.later();
                fx.inventory
                    .adjust(fone(), HOME_LOCATION, StockAdjustment::Count(0), None)
                    .await
                    .unwrap();
            }

            let dealt = fx.shares().await;
            let (first, second) = if amazon_first { (amazon, ml) } else { (ml, amazon) };
            let in_order: Vec<u32> = first
                .iter()
                .chain(second)
                .map(|id| dealt[*id].unwrap())
                .collect();
            prop_assert_eq!(in_order.iter().sum::<u32>(), units);
            prop_assert!(in_order.windows(2).all(|pair| pair[0] == pair[1] || pair[0] == pair[1] + 1));
            prop_assert!(in_order[0] - in_order[in_order.len() - 1] <= 1);
            Ok(())
        })?;
    }

    /// The #32 criterion: sales on two channels interleaved with Order
    /// Syncs, stock sends and failed sends never sell more units than the
    /// ledger had, and what the channels show never adds up to more than
    /// the units on hand, not even halfway through a send.
    #[test]
    fn sales_on_two_channels_never_sell_a_unit_twice(
        received in 1u32..10,
        steps in prop::collection::vec(any_step(), 1..30),
    ) {
        block_on(async {
            let fx = Fixture::selling(&ML_LISTINGS, &AMAZON_LISTINGS).await;
            fx.receive(received).await;
            fx.send().await;
            let mut received = received;
            let mut sold = 0;
            for step in steps {
                let before: HashMap<(SalesChannelKind, String), u32> = [&fx.ml, &fx.amazon]
                    .iter()
                    .flat_map(|channel| {
                        channel
                            .listings
                            .lock()
                            .unwrap()
                            .iter()
                            .map(|listed| ((channel.kind, listed.id.clone()), listed.available_quantity))
                            .collect::<Vec<_>>()
                    })
                    .collect();
                fx.log.lock().unwrap().clear();
                match step {
                    Step::Buys(kind, listing, units) => {
                        let item = if kind == ML { ML_LISTINGS[listing] } else { AMAZON_LISTINGS[listing] };
                        if fx.buys(kind, item, units) {
                            sold += units;
                        }
                    }
                    Step::SyncOrders(kind) => {
                        fx.sync_orders(kind).await;
                    }
                    Step::Round => {
                        fx.sync_orders(ML).await;
                        fx.sync_orders(AMAZON).await;
                        fx.send().await;
                    }
                    Step::Receive(units) => {
                        fx.receive(units).await;
                        received += units;
                    }
                    Step::Down(kind, down) => {
                        *fx.channel(kind).stock_failure.lock().unwrap() =
                            down.then(|| PlatformError::Failed("down".into()));
                    }
                }
                let on_hand = fx.on_hand().await;
                // Units sold that no Order Sync took from the ledger yet.
                let pending = sold - (received - on_hand);
                // Replayed one stock set at a time, the channels never
                // showed more than the units on hand not sold yet.
                let mut shown = before;
                for (kind, item, units) in fx.logged() {
                    shown.insert((kind, item), units);
                    prop_assert!(
                        shown.values().sum::<u32>() + pending <= on_hand,
                        "{:?}: {:?} with {} pending", step, shown, pending
                    );
                }
                prop_assert!(
                    fx.shown() + pending <= on_hand,
                    "{:?}: {} shown, {} pending, {} on hand", step, fx.shown(), pending, on_hand
                );
            }
            for kind in [ML, AMAZON] {
                *fx.channel(kind).stock_failure.lock().unwrap() = None;
                fx.sync_orders(kind).await;
            }
            fx.send().await;

            let on_hand = fx.on_hand().await;
            prop_assert_eq!(on_hand, received - sold);
            prop_assert_eq!(fx.shown(), on_hand);
            for order in fx.orders.orders().await.unwrap() {
                for line in order.lines {
                    prop_assert!(matches!(line.stock, LineStock::Taken { .. }), "{:?}", line.stock);
                }
            }
            Ok(())
        })?;
    }
}
