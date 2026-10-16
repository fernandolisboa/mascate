//! Automatic Repricing through commerce's public interface: the flag, the
//! limits and the Minimum Margin, Mercado Livre's own automation turned on,
//! read and turned off, Amazon's featured offer followed after a Sync, and
//! the price history, with a real temporary database, Inventory for the
//! Average Cost and in-memory channels.

mod common;

use std::sync::{Arc, Mutex};

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    AUTOMATIC_REPRICING, ChannelAutomation, ChannelListing, ChannelPriceAutomation, FLAGS,
    FeaturedOffer, FeaturedOfferPrices, Listing, ListingError, Listings, OnSalesChannel,
    PriceAssumptions, PriceChangeReason, Pricing, Promotions, Repricing, RepricingError,
    RepricingLimits, RepricingPort, RepricingStatus, RepricingWay,
};
use mascate_inventory::{HOME_LOCATION, Inventory, MovementReason, NewEntry};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Money, Percentage, PlatformError, RecordId, SalesChannelKind};
use mascate_platform::{Database, Flags, Registry, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

use common::{Channel, brl, listing, percent, variation};

/// Mercado Livre's price automation, answering from memory.
#[derive(Default)]
struct Automation {
    /// Each listing's automation, as the channel has it now.
    running: Mutex<Vec<(String, ChannelAutomation)>>,
    automated: Mutex<Vec<(String, RepricingLimits)>>,
    stopped: Mutex<Vec<String>>,
    price_to_win: Mutex<Option<Money>>,
    /// Fails every call while set.
    failure: Mutex<Option<PlatformError>>,
}

impl Automation {
    fn check(&self) -> Result<(), PlatformError> {
        match self.failure.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// The channel pauses the automation of `id`, as when the listing joins
    /// a Promotion.
    fn pauses(&self, id: &str, status: RepricingStatus) {
        for (listing, automation) in self.running.lock().unwrap().iter_mut() {
            if listing == id {
                automation.status = status.clone();
            }
        }
    }
}

impl OnSalesChannel for Automation {
    fn sales_channel(&self) -> SalesChannelKind {
        SalesChannelKind::MercadoLivre
    }
}

impl ChannelPriceAutomation for Automation {
    fn automate(
        &self,
        id: &str,
        limits: &RepricingLimits,
    ) -> Result<ChannelAutomation, PlatformError> {
        self.check()?;
        self.automated
            .lock()
            .unwrap()
            .push((id.to_owned(), *limits));
        let automation = ChannelAutomation {
            limits: *limits,
            status: RepricingStatus::Following,
        };
        let mut running = self.running.lock().unwrap();
        running.retain(|(listing, _)| listing != id);
        running.push((id.to_owned(), automation.clone()));
        Ok(automation)
    }

    fn automation(&self, id: &str) -> Result<Option<ChannelAutomation>, PlatformError> {
        self.check()?;
        Ok(self
            .running
            .lock()
            .unwrap()
            .iter()
            .find(|(listing, _)| listing == id)
            .map(|(_, automation)| automation.clone()))
    }

    fn stop(&self, id: &str) -> Result<(), PlatformError> {
        self.check()?;
        self.stopped.lock().unwrap().push(id.to_owned());
        self.running
            .lock()
            .unwrap()
            .retain(|(listing, _)| listing != id);
        Ok(())
    }

    fn price_to_win(&self, _id: &str) -> Result<Option<Money>, PlatformError> {
        self.check()?;
        Ok(*self.price_to_win.lock().unwrap())
    }
}

/// Amazon's featured offer prices, answering from memory.
#[derive(Default)]
struct Featured {
    offers: Mutex<Vec<(String, FeaturedOffer)>>,
    asked: Mutex<Vec<Vec<String>>>,
}

impl Featured {
    fn expects(&self, id: &str, offer: FeaturedOffer) {
        let mut offers = self.offers.lock().unwrap();
        offers.retain(|(listing, _)| listing != id);
        offers.push((id.to_owned(), offer));
    }
}

impl OnSalesChannel for Featured {
    fn sales_channel(&self) -> SalesChannelKind {
        SalesChannelKind::Amazon
    }
}

impl FeaturedOfferPrices for Featured {
    fn featured_offers(
        &self,
        ids: &[String],
    ) -> Result<Vec<(String, FeaturedOffer)>, PlatformError> {
        self.asked.lock().unwrap().push(ids.to_vec());
        Ok(self
            .offers
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| ids.contains(id))
            .cloned()
            .collect())
    }
}

/// 6% tax and R$ 20 of shipping on each sale with free shipping.
fn assumptions() -> PriceAssumptions {
    PriceAssumptions {
        tax: percent("6"),
        estimated_shipping: brl("20"),
    }
}

fn product(n: u128) -> RecordId {
    RecordId::from_u128(1_000 + n)
}

fn limits(minimum: &str, maximum: &str) -> RepricingLimits {
    RepricingLimits {
        minimum: brl(minimum),
        maximum: brl(maximum),
    }
}

fn priced(id: &str, price: &str) -> ChannelListing {
    ChannelListing {
        price: brl(price),
        ..listing(id, "Fone Bluetooth TWS")
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    database: Arc<Database>,
    inventory: Arc<Inventory>,
    listings: Listings,
    promotions: Promotions,
    pricing: Pricing,
    repricing: Repricing,
    flags: Flags,
    mercado_livre: Channel,
    amazon: Channel,
    automation: Automation,
    featured: Featured,
}

impl Fixture {
    /// 16 October 2026, 9 o'clock in Brasília, with the flag off.
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
            &[
                mascate_platform::MIGRATIONS,
                mascate_inventory::MIGRATIONS,
                mascate_commerce::MIGRATIONS,
            ],
        )
        .await
        .unwrap();
        let ids = Arc::new(SequentialIds::default());
        let inventory = Arc::new(Inventory::new(database.clone(), clock.clone(), ids.clone()));
        Self {
            listings: Listings::new(database.clone(), clock.clone(), ids.clone()),
            promotions: Promotions::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            ),
            pricing: Pricing::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            ),
            repricing: Repricing::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            ),
            flags: Flags::new(
                database.clone(),
                Registry::new(&[FLAGS]).unwrap(),
                clock.clone(),
                ids,
            ),
            _dir: dir,
            clock,
            database,
            inventory,
            mercado_livre: Channel::default(),
            amazon: Channel::on(SalesChannelKind::Amazon),
            automation: Automation::default(),
            featured: Featured::default(),
        }
    }

    async fn turn_on(&self) {
        self.flags
            .turn_on(
                self.flags
                    .request_turn_on(AUTOMATIC_REPRICING.key)
                    .unwrap()
                    .confirm("fernando"),
            )
            .await
            .unwrap();
    }

    async fn turn_off(&self) {
        self.flags
            .turn_off(AUTOMATIC_REPRICING.key, "fernando")
            .await
            .unwrap();
    }

    /// `listed` on `channel`, synced, each linked to Product `n`.
    async fn listed(&self, channel: &Channel, listed: Vec<(ChannelListing, u128)>) -> Vec<Listing> {
        channel.has(listed.iter().map(|(listing, _)| listing.clone()).collect());
        self.listings.sync(channel).await.unwrap();
        let mut synced = Vec::new();
        for (wanted, n) in listed {
            let found = self
                .listings
                .listings()
                .await
                .unwrap()
                .into_iter()
                .find(|l| {
                    l.channel == channel.kind
                        && l.listed.id == wanted.id
                        && l.listed.variation == wanted.variation
                })
                .unwrap();
            synced.push(self.listings.link(found.id, product(n)).await.unwrap());
        }
        synced
    }

    /// A R$ 100 listing of Product 1 on `channel`, whose units cost R$ 35:
    /// one unit keeps the 10% Minimum Margin from R$ 50 on.
    async fn one_listing(&self, channel: &Channel, id: &str) -> Listing {
        let listed = self.listed(channel, vec![(priced(id, "100"), 1)]).await;
        self.stock(1, 2, "70").await;
        listed[0].clone()
    }

    /// `units` of Product `n` that cost `cost` in all come into stock.
    async fn stock(&self, n: u128, units: u32, cost: &str) {
        let connection = self.database.connect_for_transaction().await.unwrap();
        let transaction = connection.transaction().await.unwrap();
        self.inventory
            .record_entries(
                &transaction,
                &[NewEntry {
                    product: product(n),
                    location: HOME_LOCATION,
                    quantity: units,
                    cost: brl(cost),
                    reason: MovementReason::PurchaseReceipt {
                        purchase_order: RecordId::from_u128(9),
                    },
                }],
            )
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        self.clock.advance(TimeDelta::minutes(1));
    }

    /// Repricing turned on for `listing` within `limits`, confirmed.
    async fn reprice(
        &self,
        listing: &Listing,
        limits: RepricingLimits,
        channel: &Channel,
        port: RepricingPort<'_>,
    ) -> Result<mascate_commerce::RepricedListing, RepricingError> {
        let request = self
            .repricing
            .prepare(listing.id, limits, channel, assumptions())
            .await?;
        self.repricing.turn_on(request.confirm(), port).await
    }

    async fn follow(&self) -> mascate_commerce::RepricingRound {
        self.clock.advance(TimeDelta::minutes(5));
        self.repricing
            .follow_featured_offers(&self.featured, &self.amazon, assumptions())
            .await
            .unwrap()
    }

    async fn price_of(&self, listing: &Listing) -> Money {
        self.listings
            .listing(listing.id)
            .await
            .unwrap()
            .listed
            .price
    }
}

#[test]
fn with_the_flag_off_nothing_reprices_and_turning_on_is_refused() {
    block_on(async {
        let fx = Fixture::new().await;
        let listing = fx.one_listing(&fx.mercado_livre, "MLB1").await;

        assert!(!fx.repricing.is_on().await.unwrap());
        assert!(matches!(
            fx.repricing
                .prepare(
                    listing.id,
                    limits("60", "120"),
                    &fx.mercado_livre,
                    assumptions()
                )
                .await,
            Err(RepricingError::Off)
        ));
        assert!(fx.automation.automated.lock().unwrap().is_empty());
        assert!(fx.repricing.repriced().await.unwrap().is_empty());

        // A request prepared while it was on is refused once it is off.
        fx.turn_on().await;
        let request = fx
            .repricing
            .prepare(
                listing.id,
                limits("60", "120"),
                &fx.mercado_livre,
                assumptions(),
            )
            .await
            .unwrap();
        fx.turn_off().await;
        assert!(matches!(
            fx.repricing
                .turn_on(request.confirm(), RepricingPort::Automation(&fx.automation))
                .await,
            Err(RepricingError::Off)
        ));
        assert!(fx.automation.automated.lock().unwrap().is_empty());
    });
}

#[test]
fn turning_the_flag_off_ends_every_repricing_at_the_next_round() {
    block_on(async {
        let fx = Fixture::new().await;
        let on_ml = fx.one_listing(&fx.mercado_livre, "MLB1").await;
        let on_amazon = fx
            .listed(&fx.amazon, vec![(priced("FONE-BT-001", "100"), 1)])
            .await
            .remove(0);
        fx.turn_on().await;
        fx.reprice(
            &on_ml,
            limits("60", "120"),
            &fx.mercado_livre,
            RepricingPort::Automation(&fx.automation),
        )
        .await
        .unwrap();
        fx.reprice(
            &on_amazon,
            limits("60", "120"),
            &fx.amazon,
            RepricingPort::FeaturedOffer(&fx.featured),
        )
        .await
        .unwrap();
        fx.featured
            .expects("FONE-BT-001", FeaturedOffer::Expected(brl("80")));

        fx.turn_off().await;
        let ml_round = fx
            .repricing
            .check_automations(&fx.automation)
            .await
            .unwrap();
        let amazon_round = fx.follow().await;

        assert_eq!(ml_round.ended, 1);
        assert_eq!(*fx.automation.stopped.lock().unwrap(), ["MLB1"]);
        assert_eq!(amazon_round.ended, 1);
        assert_eq!(amazon_round.sent, 0);
        assert!(fx.amazon.prices_set.lock().unwrap().is_empty());
        assert!(fx.featured.asked.lock().unwrap().is_empty());
        assert!(fx.repricing.repriced().await.unwrap().is_empty());
        // Back to prices the owner approves.
        fx.listings
            .change_price(on_ml.id, brl("95"), &fx.mercado_livre)
            .await
            .unwrap();
    });
}

#[test]
fn the_minimum_keeps_the_minimum_margin_and_the_limits_make_sense() {
    block_on(async {
        let fx = Fixture::new().await;
        let listing = fx.one_listing(&fx.mercado_livre, "MLB1").await;
        fx.turn_on().await;
        let prepare = |limits| {
            fx.repricing
                .prepare(listing.id, limits, &fx.mercado_livre, assumptions())
        };

        for wrong in [
            limits("0", "120"),
            limits("130", "120"),
            RepricingLimits {
                minimum: Money::new(Decimal::from(60), mascate_kernel::Currency::Usd),
                maximum: brl("120"),
            },
        ] {
            assert!(matches!(
                prepare(wrong).await,
                Err(RepricingError::InvalidLimits)
            ));
        }
        // R$ 49,99 × (1 − 14% fee − 6% tax) − R$ 35 keeps less than 10%.
        let Err(RepricingError::BelowMinimumMargin(check)) = prepare(limits("49.99", "120")).await
        else {
            panic!("a minimum below the Minimum Margin is refused");
        };
        assert_eq!(check.lowest, Some(brl("50.00")));
        assert!(!check.passes());

        // The Minimum Margin is the one Promotions keep.
        fx.promotions
            .save_minimum_margin(percent("20"))
            .await
            .unwrap();
        assert!(matches!(
            prepare(limits("50", "120")).await,
            Err(RepricingError::BelowMinimumMargin(_))
        ));
        fx.promotions
            .save_minimum_margin(percent("10"))
            .await
            .unwrap();

        let request = prepare(limits("50.004", "120")).await.unwrap();
        assert_eq!(request.limits(), limits("50.00", "120.00"));
        assert!(request.check().passes());
    });
}

#[test]
fn a_port_reprices_only_its_own_channel() {
    block_on(async {
        let fx = Fixture::new().await;
        let on_amazon = fx
            .listed(&fx.amazon, vec![(priced("FONE-BT-001", "100"), 1)])
            .await
            .remove(0);
        fx.stock(1, 2, "70").await;
        fx.turn_on().await;

        let request = fx
            .repricing
            .prepare(on_amazon.id, limits("60", "120"), &fx.amazon, assumptions())
            .await
            .unwrap();
        assert!(matches!(
            fx.repricing
                .turn_on(request.confirm(), RepricingPort::Automation(&fx.automation))
                .await,
            Err(RepricingError::OtherChannel { .. })
        ));
        assert!(fx.automation.automated.lock().unwrap().is_empty());
    });
}

#[test]
fn mercado_livre_reprices_with_its_own_automation_and_no_price_goes_to_it_meanwhile() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx
            .listed(
                &fx.mercado_livre,
                vec![
                    (variation(&priced("MLB1", "100"), "V1", "Cor: Preto"), 1),
                    (variation(&priced("MLB1", "100"), "V2", "Cor: Branco"), 1),
                ],
            )
            .await;
        fx.stock(1, 2, "70").await;
        fx.turn_on().await;
        *fx.automation.price_to_win.lock().unwrap() = Some(brl("92.50"));

        let repriced = fx
            .reprice(
                &listed[0],
                limits("60", "120"),
                &fx.mercado_livre,
                RepricingPort::Automation(&fx.automation),
            )
            .await
            .unwrap();

        assert_eq!(
            *fx.automation.automated.lock().unwrap(),
            [("MLB1".to_owned(), limits("60", "120"))]
        );
        assert_eq!(repriced.way, RepricingWay::ChannelAutomation);
        assert_eq!(repriced.limits, limits("60", "120"));
        let reading = repriced.reading.unwrap();
        assert_eq!(reading.status, RepricingStatus::Following);
        assert_eq!(reading.winning_price, Some(brl("92.50")));
        // Both variations share the repricing.
        assert_eq!(
            fx.repricing.repricing_of(listed[1].id).await.unwrap(),
            fx.repricing.repricing_of(listed[0].id).await.unwrap()
        );

        // An approval is refused before the channel is asked.
        assert!(matches!(
            fx.listings
                .change_price(listed[1].id, brl("95"), &fx.mercado_livre)
                .await,
            Err(ListingError::Repriced)
        ));
        assert!(fx.mercado_livre.prices_set.lock().unwrap().is_empty());

        // The channel lowers the price; the Sync keeps it in the history.
        fx.clock.advance(TimeDelta::minutes(10));
        fx.mercado_livre
            .change("MLB1", |listing| listing.price = brl("92.50"));
        fx.listings.sync(&fx.mercado_livre).await.unwrap();
        let history = fx.repricing.price_history(listed[0].id).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].reason, PriceChangeReason::ChannelAutomation);
        assert_eq!((history[0].old, history[0].new), (brl("100"), brl("92.50")));
        assert_eq!(history[0].at, fx.clock.now());
        assert_eq!(
            fx.repricing.price_history(listed[1].id).await.unwrap()[0].reason,
            PriceChangeReason::ChannelAutomation
        );
    });
}

#[test]
fn a_round_reads_the_automation_paused_by_a_promotion_and_one_ended_in_the_channel() {
    block_on(async {
        let fx = Fixture::new().await;
        let first = fx.one_listing(&fx.mercado_livre, "MLB1").await;
        let second = fx
            .listed(
                &fx.mercado_livre,
                vec![(priced("MLB1", "100"), 1), (priced("MLB2", "100"), 1)],
            )
            .await
            .remove(1);
        fx.turn_on().await;
        for listing in [&first, &second] {
            fx.reprice(
                listing,
                limits("60", "120"),
                &fx.mercado_livre,
                RepricingPort::Automation(&fx.automation),
            )
            .await
            .unwrap();
        }

        fx.automation
            .pauses("MLB1", RepricingStatus::PausedByPromotion);
        fx.automation
            .running
            .lock()
            .unwrap()
            .retain(|(id, _)| id != "MLB2");
        *fx.automation.price_to_win.lock().unwrap() = Some(brl("88"));
        fx.clock.advance(TimeDelta::minutes(5));
        let round = fx
            .repricing
            .check_automations(&fx.automation)
            .await
            .unwrap();

        assert_eq!((round.checked, round.ended), (2, 1));
        assert!(round.failed.is_empty());
        let paused = fx.repricing.repricing_of(first.id).await.unwrap().unwrap();
        let reading = paused.reading.unwrap();
        assert_eq!(reading.status, RepricingStatus::PausedByPromotion);
        assert_eq!(reading.winning_price, Some(brl("88")));
        assert_eq!(reading.at, fx.clock.now());
        // The owner ended it in the channel: the app sends prices again.
        assert!(
            fx.repricing
                .repricing_of(second.id)
                .await
                .unwrap()
                .is_none()
        );
        fx.listings
            .change_price(second.id, brl("97"), &fx.mercado_livre)
            .await
            .unwrap();

        // A failing channel leaves the reading as it was.
        *fx.automation.failure.lock().unwrap() = Some(PlatformError::RateLimited);
        let failed = fx
            .repricing
            .check_automations(&fx.automation)
            .await
            .unwrap();
        assert_eq!(failed.failed.len(), 1);
        assert_eq!(
            fx.repricing
                .repricing_of(first.id)
                .await
                .unwrap()
                .unwrap()
                .reading
                .unwrap()
                .at,
            reading.at
        );
    });
}

#[test]
fn turning_repricing_off_ends_the_automation_and_approvals_go_again() {
    block_on(async {
        let fx = Fixture::new().await;
        let listing = fx.one_listing(&fx.mercado_livre, "MLB1").await;
        fx.turn_on().await;
        fx.reprice(
            &listing,
            limits("60", "120"),
            &fx.mercado_livre,
            RepricingPort::Automation(&fx.automation),
        )
        .await
        .unwrap();
        // New limits change the same automation.
        fx.reprice(
            &listing,
            limits("70", "110"),
            &fx.mercado_livre,
            RepricingPort::Automation(&fx.automation),
        )
        .await
        .unwrap();
        assert_eq!(fx.repricing.repriced().await.unwrap().len(), 1);
        assert_eq!(
            fx.repricing.repriced().await.unwrap()[0].limits,
            limits("70", "110")
        );

        fx.repricing
            .turn_off(listing.id, RepricingPort::Automation(&fx.automation))
            .await
            .unwrap();
        // Twice changes nothing.
        fx.repricing
            .turn_off(listing.id, RepricingPort::Automation(&fx.automation))
            .await
            .unwrap();

        assert_eq!(*fx.automation.stopped.lock().unwrap(), ["MLB1"]);
        assert!(
            fx.repricing
                .repricing_of(listing.id)
                .await
                .unwrap()
                .is_none()
        );
        fx.listings
            .change_price(listing.id, brl("95"), &fx.mercado_livre)
            .await
            .unwrap();
        let history = fx.repricing.price_history(listing.id).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].reason, PriceChangeReason::Approved);
        assert_eq!((history[0].old, history[0].new), (brl("100"), brl("95")));
    });
}

#[test]
fn amazon_follows_the_featured_offer_within_the_limits_once_per_sync() {
    block_on(async {
        let fx = Fixture::new().await;
        let listing = fx
            .listed(&fx.amazon, vec![(priced("FONE-BT-001", "100"), 1)])
            .await
            .remove(0);
        fx.stock(1, 2, "70").await;
        fx.turn_on().await;
        let repriced = fx
            .reprice(
                &listing,
                limits("60", "120"),
                &fx.amazon,
                RepricingPort::FeaturedOffer(&fx.featured),
            )
            .await
            .unwrap();
        assert_eq!(repriced.way, RepricingWay::FeaturedOffer);
        assert!(repriced.reading.is_none());

        let mut followed = Vec::new();
        for (expected, price) in [("87.351", "87.35"), ("45", "60"), ("150", "120")] {
            fx.featured
                .expects("FONE-BT-001", FeaturedOffer::Expected(brl(expected)));
            let round = fx.follow().await;
            assert_eq!((round.checked, round.sent), (1, 1));
            assert_eq!(fx.price_of(&listing).await, brl(price));
            followed.push(price);
        }
        // The same answer again sends nothing.
        let round = fx.follow().await;
        assert_eq!(round.sent, 0);

        assert_eq!(
            *fx.amazon.prices_set.lock().unwrap(),
            [
                ("FONE-BT-001".to_owned(), brl("87.35")),
                ("FONE-BT-001".to_owned(), brl("60")),
                ("FONE-BT-001".to_owned(), brl("120")),
            ]
        );
        let history = fx.repricing.price_history(listing.id).await.unwrap();
        assert_eq!(history.len(), 3);
        assert!(
            history
                .iter()
                .all(|change| change.reason == PriceChangeReason::FeaturedOffer)
        );
        assert_eq!((history[0].old, history[0].new), (brl("60"), brl("120")));
        assert_eq!((history[2].old, history[2].new), (brl("100"), brl("87.35")));
        let reading = fx
            .repricing
            .repricing_of(listing.id)
            .await
            .unwrap()
            .unwrap()
            .reading
            .unwrap();
        assert_eq!(reading.status, RepricingStatus::Following);
        assert_eq!(reading.winning_price, Some(brl("150")));
        // An approval waits until repricing is off.
        assert!(matches!(
            fx.listings
                .change_price(listing.id, brl("99"), &fx.amazon)
                .await,
            Err(ListingError::Repriced)
        ));
        assert_eq!(fx.amazon.prices_set.lock().unwrap().len(), 3);
        // The next Sync shows the price the app sent: no other change.
        fx.listings.sync(&fx.amazon).await.unwrap();
        assert_eq!(
            fx.repricing.price_history(listing.id).await.unwrap().len(),
            3
        );
    });
}

#[test]
fn amazon_keeps_the_price_without_competition_off_sale_or_when_not_eligible() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx
            .listed(
                &fx.amazon,
                vec![
                    (priced("FONE-BT-001", "100"), 1),
                    (priced("CANECA-01", "100"), 1),
                    (priced("CAMISETA-P", "100"), 1),
                ],
            )
            .await;
        fx.stock(1, 6, "210").await;
        fx.turn_on().await;
        for listing in &listed {
            fx.reprice(
                listing,
                limits("60", "120"),
                &fx.amazon,
                RepricingPort::FeaturedOffer(&fx.featured),
            )
            .await
            .unwrap();
        }
        fx.featured
            .expects("FONE-BT-001", FeaturedOffer::NoCompetition);
        // CANECA-01 is left out of the answer; CAMISETA-P sells no more.
        fx.amazon.change("CAMISETA-P", |listing| {
            listing.available_quantity = 0;
            listing.status = mascate_commerce::ListingStatus::OutOfStock;
        });
        fx.listings.sync(&fx.amazon).await.unwrap();
        fx.featured
            .expects("CAMISETA-P", FeaturedOffer::Expected(brl("70")));

        let round = fx.follow().await;

        assert_eq!((round.checked, round.sent), (2, 0));
        assert_eq!(
            *fx.featured.asked.lock().unwrap(),
            [vec!["CANECA-01".to_owned(), "FONE-BT-001".to_owned()]]
        );
        let mut statuses = Vec::new();
        for listing in &listed {
            statuses.push(
                fx.repricing
                    .repricing_of(listing.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .reading
                    .map(|reading| reading.status),
            );
        }
        assert_eq!(
            statuses,
            [
                Some(RepricingStatus::NoCompetition),
                Some(RepricingStatus::NotEligible),
                None
            ]
        );
        assert!(fx.amazon.prices_set.lock().unwrap().is_empty());
    });
}

#[test]
fn amazon_never_follows_below_the_minimum_margin_once_the_cost_went_up() {
    block_on(async {
        let fx = Fixture::new().await;
        let listing = fx
            .listed(&fx.amazon, vec![(priced("FONE-BT-001", "70"), 1)])
            .await
            .remove(0);
        fx.stock(1, 2, "70").await;
        fx.turn_on().await;
        fx.reprice(
            &listing,
            limits("50", "75"),
            &fx.amazon,
            RepricingPort::FeaturedOffer(&fx.featured),
        )
        .await
        .unwrap();
        // Two more units at R$ 45 each: the Average Cost goes to R$ 40, and
        // a unit keeps 10% only from R$ 57,15 on.
        fx.stock(1, 2, "90").await;
        fx.featured
            .expects("FONE-BT-001", FeaturedOffer::Expected(brl("52")));

        let round = fx.follow().await;

        assert_eq!(round.sent, 1);
        assert_eq!(fx.price_of(&listing).await, brl("57.15"));

        // With a maximum below that price, the price stays.
        fx.reprice(
            &listing,
            limits("57.15", "75"),
            &fx.amazon,
            RepricingPort::FeaturedOffer(&fx.featured),
        )
        .await
        .unwrap();
        // One more unit at R$ 200: R$ 72 a unit, R$ 102,86 to keep 10%.
        fx.stock(1, 1, "200").await;
        let round = fx.follow().await;
        assert_eq!(round.sent, 0);
        assert_eq!(
            fx.repricing
                .repricing_of(listing.id)
                .await
                .unwrap()
                .unwrap()
                .reading
                .unwrap()
                .status,
            RepricingStatus::MarginOutOfReach
        );
    });
}

#[test]
fn a_price_applied_later_goes_into_the_history_once() {
    block_on(async {
        let fx = Fixture::new().await;
        let magalu = Channel {
            late_prices: true,
            ..Channel::on(SalesChannelKind::Magalu)
        };
        let listing = fx
            .listed(&magalu, vec![(priced("CAM-PRETA-M", "100"), 1)])
            .await
            .remove(0);
        fx.stock(1, 2, "70").await;

        fx.listings
            .change_price(listing.id, brl("90"), &magalu)
            .await
            .unwrap();
        magalu.change("CAM-PRETA-M", |listing| listing.price = brl("90"));
        fx.clock.advance(TimeDelta::minutes(5));
        fx.listings.sync(&magalu).await.unwrap();
        // The owner changes it in Magalu's panel later.
        magalu.change("CAM-PRETA-M", |listing| listing.price = brl("85"));
        fx.clock.advance(TimeDelta::minutes(5));
        fx.listings.sync(&magalu).await.unwrap();

        let history = fx.repricing.price_history(listing.id).await.unwrap();
        let reasons: Vec<_> = history.iter().map(|change| change.reason).collect();
        assert_eq!(
            reasons,
            [PriceChangeReason::InChannel, PriceChangeReason::Approved]
        );
        assert_eq!((history[0].old, history[0].new), (brl("90"), brl("85")));
        assert_eq!((history[1].old, history[1].new), (brl("100"), brl("90")));
    });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Whatever Amazon expects and whatever a unit costs, the price the
    /// app follows to is within the limits and keeps the Minimum Margin.
    #[test]
    fn the_amazon_loop_stays_within_the_limits_and_the_minimum_margin(
        minimum in 5_000i64..9_000,
        spread in 0i64..6_000,
        expected in 1i64..20_000,
        cost in 2_000i64..6_000,
    ) {
        block_on(async {
            let fx = Fixture::new().await;
            let listing = fx
                .listed(&fx.amazon, vec![(priced("FONE-BT-001", "100"), 1)])
                .await
                .remove(0);
            // R$ 35 a unit until the turn on, then the cost under test.
            fx.stock(1, 2, "70").await;
            fx.turn_on().await;
            let limits = RepricingLimits {
                minimum: Money::new(Decimal::new(minimum, 2), mascate_kernel::Currency::Brl),
                maximum: Money::new(
                    Decimal::new(minimum + spread, 2),
                    mascate_kernel::Currency::Brl,
                ),
            };
            if fx
                .reprice(&listing, limits, &fx.amazon, RepricingPort::FeaturedOffer(&fx.featured))
                .await
                .is_err()
            {
                return;
            }
            fx.stock(1, 2, &Decimal::new(cost * 2, 2).to_string()).await;
            fx.featured.expects(
                "FONE-BT-001",
                FeaturedOffer::Expected(Money::new(
                    Decimal::new(expected, 2),
                    mascate_kernel::Currency::Brl,
                )),
            );

            fx.follow().await;

            let minimum_margin: Percentage = fx.promotions.minimum_margin().await.unwrap();
            let prices_set = fx.amazon.prices_set.lock().unwrap().clone();
            assert!(prices_set.len() <= 1);
            for (_, sent) in prices_set {
                assert!(limits.contains(sent), "{sent} outside {limits:?}");
                let check = fx
                    .pricing
                    .check_margin(listing.id, sent, minimum_margin, &fx.amazon, assumptions())
                    .await
                    .unwrap();
                assert!(check.passes(), "{sent} keeps less than the Minimum Margin");
            }
        });
    }
}
