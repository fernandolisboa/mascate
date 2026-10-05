//! Promotions through commerce's public interface: the minimum margin, the
//! channel's rules for days, the margin guard (coupons in the worst case
//! included), sending once what the owner confirmed, ending, and the Sync,
//! with a real temporary database, Inventory for the Average Cost and
//! in-memory channels.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    ChannelListing, ChannelOffer, ChannelPromotion, ChannelPromotions, CouponDiscount, CouponTerms,
    DiscountPlan, Listing, ListingStatus, Listings, OfferToJoin, PriceAssumptions, PromotionError,
    PromotionKind, PromotionPlan, PromotionStatus, Promotions, channel_day,
};
use mascate_inventory::{HOME_LOCATION, Inventory, MovementReason, NewEntry};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, Percentage, PlatformError, RecordId};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

use common::{Channel, brl, listing, percent, variation};

/// The owner's Promotions on a channel answering from memory, and what it
/// was asked to do.
#[derive(Default)]
struct PromotionChannel {
    promotions: Mutex<Vec<ChannelPromotion>>,
    offers: Mutex<HashMap<String, Vec<ChannelOffer>>>,
    created: Mutex<Vec<PromotionPlan>>,
    changed: Mutex<Vec<(String, PromotionPlan)>>,
    ended: Mutex<Vec<String>>,
    joined: Mutex<Vec<(String, OfferToJoin)>>,
    left: Mutex<Vec<(String, PromotionKind, Option<String>)>>,
    /// Listings read, in order.
    read: Mutex<Vec<String>>,
    /// Fails every call while set.
    failure: Mutex<Option<PlatformError>>,
    /// Does what it is asked but the answer never arrives, once.
    loses_answer: Mutex<bool>,
}

impl PromotionChannel {
    fn check(&self) -> Result<(), PlatformError> {
        match self.failure.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn answer(&self) -> Result<(), PlatformError> {
        if std::mem::take(&mut *self.loses_answer.lock().unwrap()) {
            Err(PlatformError::Failed("the connection dropped".into()))
        } else {
            Ok(())
        }
    }

    fn has_offer(&self, offer: ChannelOffer) {
        self.offers
            .lock()
            .unwrap()
            .entry(offer.listing.clone())
            .or_default()
            .push(offer);
    }
}

impl ChannelPromotions for PromotionChannel {
    fn promotions(&self) -> Result<Vec<ChannelPromotion>, PlatformError> {
        self.check()?;
        Ok(self.promotions.lock().unwrap().clone())
    }

    fn offers(&self, listing: &str) -> Result<Vec<ChannelOffer>, PlatformError> {
        self.check()?;
        self.read.lock().unwrap().push(listing.to_owned());
        Ok(self
            .offers
            .lock()
            .unwrap()
            .get(listing)
            .cloned()
            .unwrap_or_default())
    }

    fn create(&self, plan: &PromotionPlan) -> Result<ChannelPromotion, PlatformError> {
        self.check()?;
        self.created.lock().unwrap().push(plan.clone());
        let mut promotions = self.promotions.lock().unwrap();
        let created = ChannelPromotion {
            id: format!("C-MLB{}", 100 + promotions.len()),
            kind: plan.kind,
            name: plan.name.clone(),
            starts: plan.starts,
            ends: plan.ends,
            status: PromotionStatus::Pending,
            coupon: None,
        };
        promotions.push(created.clone());
        drop(promotions);
        self.answer()?;
        Ok(created)
    }

    fn change(&self, id: &str, plan: &PromotionPlan) -> Result<(), PlatformError> {
        self.check()?;
        self.changed
            .lock()
            .unwrap()
            .push((id.to_owned(), plan.clone()));
        Ok(())
    }

    fn end(&self, id: &str, _kind: PromotionKind) -> Result<(), PlatformError> {
        self.check()?;
        self.ended.lock().unwrap().push(id.to_owned());
        let mut promotions = self.promotions.lock().unwrap();
        let before = promotions.len();
        promotions.retain(|promotion| promotion.id != id);
        if promotions.len() == before {
            return Err(PlatformError::NotFound);
        }
        for offers in self.offers.lock().unwrap().values_mut() {
            offers.retain(|offer| offer.promotion.as_deref() != Some(id));
        }
        Ok(())
    }

    fn join(&self, listing: &str, offer: &OfferToJoin) -> Result<(), PlatformError> {
        self.check()?;
        self.joined
            .lock()
            .unwrap()
            .push((listing.to_owned(), offer.clone()));
        let promotion = self
            .promotions
            .lock()
            .unwrap()
            .iter()
            .find(|promotion| Some(&promotion.id) == offer.promotion.as_ref())
            .cloned();
        let days = offer
            .days
            .or(promotion.as_ref().map(|p| (p.starts, p.ends)));
        self.has_offer(ChannelOffer {
            listing: listing.to_owned(),
            kind: offer.kind,
            promotion: offer.promotion.clone(),
            name: promotion.map(|p| p.name).unwrap_or_default(),
            status: PromotionStatus::Pending,
            price: offer.price,
            starts: days.map(|(starts, _)| starts),
            ends: days.map(|(_, ends)| ends),
        });
        self.answer()
    }

    fn leave(
        &self,
        listing: &str,
        kind: PromotionKind,
        promotion: Option<&str>,
    ) -> Result<(), PlatformError> {
        self.check()?;
        self.left
            .lock()
            .unwrap()
            .push((listing.to_owned(), kind, promotion.map(str::to_owned)));
        if let Some(offers) = self.offers.lock().unwrap().get_mut(listing) {
            offers.retain(|offer| !(offer.kind == kind && offer.promotion.as_deref() == promotion));
        }
        Ok(())
    }
}

fn on(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
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

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    database: Arc<Database>,
    inventory: Arc<Inventory>,
    listings: Listings,
    promotions: Promotions,
    sales: Channel,
    channel: PromotionChannel,
}

impl Fixture {
    /// 4 October 2026, 9 o'clock in Brasília.
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
            listings: Listings::new(database.clone(), clock.clone(), ids.clone()),
            promotions: Promotions::new(database.clone(), inventory.clone(), clock.clone(), ids),
            _dir: dir,
            clock,
            database,
            inventory,
            sales: Channel::default(),
            channel: PromotionChannel::default(),
        }
    }

    /// The channel's listings, synced, each linked to Product `n`.
    async fn listed(&self, listings: Vec<(ChannelListing, u128)>) -> Vec<Listing> {
        self.sales.has(
            listings
                .iter()
                .map(|(listing, _)| listing.clone())
                .collect(),
        );
        self.listings.sync(&self.sales).await.unwrap();
        let mut synced = Vec::new();
        for (listed, n) in listings {
            let found = self
                .listings
                .listings()
                .await
                .unwrap()
                .into_iter()
                .find(|l| l.listed.id == listed.id && l.listed.variation == listed.variation)
                .unwrap();
            synced.push(self.listings.link(found.id, product(n)).await.unwrap());
        }
        synced
    }

    /// One R$ 100 listing of Product 1, whose units cost R$ 35.
    async fn one_listing(&self) -> Listing {
        let listed = self.listed(vec![(priced("MLB1", "100"), 1)]).await;
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

    async fn discount(
        &self,
        listing: &Listing,
        price: &str,
        days: (u32, u32),
    ) -> Result<mascate_commerce::PromotionRequest, PromotionError> {
        self.promotions
            .prepare_discount(
                DiscountPlan {
                    listing: listing.id,
                    price: brl(price),
                    starts: on(days.0),
                    ends: on(days.1),
                },
                true,
                &self.sales,
                assumptions(),
            )
            .await
    }

    /// Creates the campaign or coupon `plan` on the channel through the
    /// app, and returns it as kept.
    async fn created(&self, plan: PromotionPlan) -> mascate_commerce::Promotion {
        let request = self
            .promotions
            .prepare_promotion(plan.clone(), true)
            .unwrap();
        self.promotions
            .send(&self.channel, request.confirm())
            .await
            .unwrap();
        self.promotions
            .promotions()
            .await
            .unwrap()
            .into_iter()
            .find(|kept| kept.listed.name == plan.name)
            .unwrap()
    }
}

fn priced(id: &str, price: &str) -> ChannelListing {
    ChannelListing {
        price: brl(price),
        ..listing(id, "Fone Bluetooth TWS")
    }
}

fn campaign(name: &str, days: (u32, u32)) -> PromotionPlan {
    PromotionPlan {
        kind: PromotionKind::SellerCampaign,
        name: name.into(),
        starts: on(days.0),
        ends: on(days.1),
        coupon: None,
    }
}

fn coupon(name: &str, terms: CouponTerms) -> PromotionPlan {
    PromotionPlan {
        kind: PromotionKind::Coupon,
        name: name.into(),
        starts: on(5),
        ends: on(20),
        coupon: Some(terms),
    }
}

/// R$ 20 off purchases from R$ 50, R$ 1.000 of budget.
fn twenty_off() -> CouponTerms {
    CouponTerms {
        discount: CouponDiscount::Amount(brl("20")),
        minimum_purchase: brl("50"),
        maximum_discount: None,
        budget: brl("1000"),
    }
}

#[test]
fn the_channel_counts_days_in_brasilia_time() {
    let late_on_the_4th = Utc.with_ymd_and_hms(2026, 10, 5, 2, 59, 0).unwrap();
    let early_on_the_5th = Utc.with_ymd_and_hms(2026, 10, 5, 3, 0, 0).unwrap();

    assert_eq!(channel_day(late_on_the_4th), on(4));
    assert_eq!(channel_day(early_on_the_5th), on(5));
}

#[test]
fn a_percentage_coupon_takes_its_share_up_to_its_cap() {
    let ten_percent = CouponTerms {
        discount: CouponDiscount::Percent(percent("10")),
        minimum_purchase: brl("50"),
        maximum_discount: Some(brl("15")),
        budget: brl("500"),
    };

    assert_eq!(ten_percent.unit_price(brl("100")).unwrap(), brl("90"));
    // R$ 300 is one unit: 10% is R$ 30, capped at R$ 15.
    assert_eq!(ten_percent.unit_price(brl("300")).unwrap(), brl("285"));
    // R$ 20 takes 3 units to reach R$ 50: R$ 2 each, under the R$ 5 cap.
    assert_eq!(ten_percent.unit_price(brl("20")).unwrap(), brl("18"));
}

#[test]
fn an_amount_coupon_falls_on_the_fewest_units_that_reach_its_minimum() {
    // R$ 100 reaches R$ 50 alone: the whole R$ 20 on one unit.
    assert_eq!(twenty_off().unit_price(brl("100")).unwrap(), brl("80"));
    // R$ 20 takes 3 units to reach R$ 50: R$ 20 split three ways.
    assert_eq!(
        twenty_off().unit_price(brl("20")).unwrap().rounded(),
        brl("13.33")
    );
}

fn cents(amount: i64) -> Money {
    Money::new(Decimal::new(amount, 2), Currency::Brl)
}

proptest! {
    /// A coupon never raises a price, never takes more than its amount or
    /// its share off one unit, and takes at least its share (or its amount
    /// spread over the fewest units) when nothing caps it.
    #[test]
    fn a_coupon_takes_no_more_than_it_offers(
        price in 1i64..1_000_000,
        off in 1i64..100_000,
        rate in 1i64..1_000,
        minimum in 1i64..1_000_000,
    ) {
        let price = cents(price);
        let amount = CouponTerms {
            discount: CouponDiscount::Amount(cents(off)),
            minimum_purchase: cents(minimum),
            maximum_discount: None,
            budget: cents(100_000_000),
        };
        let share = CouponTerms {
            discount: CouponDiscount::Percent(Percentage::new(Decimal::new(rate, 1)).unwrap()),
            ..amount
        };

        let after_amount = amount.unit_price(price).unwrap();
        let after_share = share.unit_price(price).unwrap();

        prop_assert!(after_amount.amount() <= price.amount());
        prop_assert!(after_amount.amount() >= price.amount() - cents(off).amount());
        prop_assert_eq!(
            after_share.amount(),
            price.amount() - Decimal::new(rate, 1) * price.amount() / Decimal::ONE_HUNDRED
        );
    }
}

#[test]
fn the_minimum_margin_starts_at_ten_percent_and_stays_below_a_hundred() {
    block_on(async {
        let fx = Fixture::new().await;
        assert_eq!(fx.promotions.minimum_margin().await.unwrap(), percent("10"));

        fx.promotions
            .save_minimum_margin(percent("12.5"))
            .await
            .unwrap();
        let refused = fx.promotions.save_minimum_margin(percent("100")).await;

        assert!(matches!(refused, Err(PromotionError::InvalidMinimum)));
        assert_eq!(
            fx.promotions.minimum_margin().await.unwrap(),
            percent("12.5")
        );
    });
}

#[test]
fn nothing_is_prepared_before_the_reputation_unlocks_promotions() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx.one_listing().await;

        let discount = fx
            .promotions
            .prepare_discount(
                DiscountPlan {
                    listing: listed.id,
                    price: brl("90"),
                    starts: on(4),
                    ends: on(10),
                },
                false,
                &fx.sales,
                assumptions(),
            )
            .await;
        let created = fx
            .promotions
            .prepare_promotion(campaign("Semana do fone", (5, 10)), false);

        assert!(matches!(discount, Err(PromotionError::Locked)));
        assert!(matches!(created, Err(PromotionError::Locked)));
        assert!(fx.sales.fee_asked.lock().unwrap().is_empty());
    });
}

#[test]
fn the_days_follow_the_channel_rules_for_each_kind() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx.one_listing().await;

        assert!(matches!(
            fx.discount(&listed, "90", (10, 9)).await,
            Err(PromotionError::EndsBeforeStart)
        ));
        assert!(matches!(
            fx.discount(&listed, "90", (3, 9)).await,
            Err(PromotionError::StartsInPast)
        ));
        // 14 days, today included, is the most; a 15th is one too many.
        assert!(fx.discount(&listed, "90", (4, 17)).await.is_ok());
        assert!(matches!(
            fx.discount(&listed, "90", (4, 18)).await,
            Err(PromotionError::TooLong(14))
        ));
        assert!(
            fx.promotions
                .prepare_promotion(campaign("Duas semanas", (5, 18)), true)
                .is_ok()
        );
        assert!(matches!(
            fx.promotions
                .prepare_promotion(campaign("Mais de duas", (5, 19)), true),
            Err(PromotionError::TooLong(14))
        ));
        let month = |ends| PromotionPlan {
            ends: on(ends),
            ..coupon("Cupom do mês", twenty_off())
        };
        assert!(fx.promotions.prepare_promotion(month(31), true).is_ok());
        let november = PromotionPlan {
            starts: on(5),
            ends: NaiveDate::from_ymd_opt(2026, 11, 5).unwrap(),
            ..month(31)
        };
        assert!(matches!(
            fx.promotions.prepare_promotion(november, true),
            Err(PromotionError::TooLong(31))
        ));
    });
}

#[test]
fn a_campaign_needs_a_name_and_a_coupon_sound_terms() {
    block_on(async {
        let fx = Fixture::new().await;

        let unnamed = fx
            .promotions
            .prepare_promotion(campaign("   ", (5, 10)), true);
        let free = fx.promotions.prepare_promotion(
            coupon(
                "Grátis",
                CouponTerms {
                    discount: CouponDiscount::Percent(percent("100")),
                    ..twenty_off()
                },
            ),
            true,
        );
        let no_budget = fx.promotions.prepare_promotion(
            coupon(
                "Sem verba",
                CouponTerms {
                    budget: brl("0"),
                    ..twenty_off()
                },
            ),
            true,
        );
        let no_terms = fx.promotions.prepare_promotion(
            PromotionPlan {
                coupon: None,
                ..coupon("Sem termos", twenty_off())
            },
            true,
        );

        assert!(matches!(unnamed, Err(PromotionError::NoName)));
        assert!(matches!(free, Err(PromotionError::InvalidCoupon)));
        assert!(matches!(no_budget, Err(PromotionError::InvalidCoupon)));
        assert!(matches!(no_terms, Err(PromotionError::InvalidCoupon)));
    });
}

#[test]
fn a_discount_lowers_the_price() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx.one_listing().await;

        assert!(matches!(
            fx.discount(&listed, "100", (4, 10)).await,
            Err(PromotionError::NotADiscount)
        ));
        assert!(matches!(
            fx.discount(&listed, "0", (4, 10)).await,
            Err(PromotionError::NotADiscount)
        ));
    });
}

#[test]
fn a_discount_below_the_minimum_margin_is_refused_with_the_lowest_price_that_keeps_it() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx.one_listing().await;

        let refused = fx.discount(&listed, "45", (4, 10)).await;

        // R$ 45: 14% fee, no shipping below R$ 79, 6% tax and R$ 35 of cost
        // leave R$ 1, 2,2% of the price.
        let Err(PromotionError::BelowMinimum(check)) = refused else {
            panic!("expected a refusal, got {refused:?}");
        };
        assert!(!check.after_coupon);
        let margin = check.margin;
        assert!(!margin.passes());
        assert_eq!(margin.breakdown.sale_price, brl("45"));
        assert_eq!(margin.breakdown.sale_fee, brl("6.30"));
        assert_eq!(margin.breakdown.shipping, brl("0"));
        assert_eq!(margin.breakdown.tax, brl("2.70"));
        assert_eq!(margin.breakdown.cost, brl("35"));
        assert_eq!(margin.breakdown.margin.amount, brl("1"));
        assert_eq!(margin.minimum, percent("10"));
        // R$ 35 / (1 − 14% − 6% − 10%) = R$ 50.
        assert_eq!(margin.lowest, Some(brl("50")));
        assert!(fx.discount(&listed, "50", (4, 10)).await.is_ok());
        assert!(matches!(
            fx.discount(&listed, "49.99", (4, 10)).await,
            Err(PromotionError::BelowMinimum(_))
        ));
        assert!(fx.channel.joined.lock().unwrap().is_empty());
    });
}

#[test]
fn the_variation_that_keeps_the_least_margin_guards_the_listing() {
    block_on(async {
        let fx = Fixture::new().await;
        let black = priced("MLB1", "100");
        let white = variation(&black, "11", "Cor: Branco");
        let listed = fx
            .listed(vec![(variation(&black, "10", "Cor: Preto"), 1), (white, 2)])
            .await;
        fx.stock(1, 1, "35").await;
        fx.stock(2, 1, "45").await;

        let refused = fx.discount(&listed[0], "60", (4, 10)).await;

        // R$ 60 leaves R$ 3 on the white one (R$ 45 cost) and R$ 13 on the
        // black: the white one sets the floor of R$ 45 / 70% = R$ 64,29.
        let Err(PromotionError::BelowMinimum(check)) = refused else {
            panic!("expected a refusal, got {refused:?}");
        };
        assert_eq!(check.margin.product, product(2));
        assert_eq!(check.margin.breakdown.margin.amount, brl("3"));
        assert_eq!(check.margin.lowest, Some(brl("64.29")));
    });
}

#[test]
fn a_confirmed_discount_reaches_the_channel_once() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx.one_listing().await;
        let request = fx.discount(&listed, "90", (4, 10)).await.unwrap();
        // R$ 90 − R$ 12,60 − R$ 20 − R$ 5,40 − R$ 35 = R$ 17, 18,9%.
        assert_eq!(
            request.check().unwrap().margin.breakdown.margin.amount,
            brl("17")
        );
        // The first send gets through, but its answer is lost.
        *fx.channel.loses_answer.lock().unwrap() = true;
        let lost = fx
            .promotions
            .send(&fx.channel, request.clone().confirm())
            .await;
        assert!(matches!(lost, Err(PromotionError::Platform(_))));

        fx.promotions
            .send(&fx.channel, request.confirm())
            .await
            .unwrap();

        let joined = fx.channel.joined.lock().unwrap().clone();
        assert_eq!(joined.len(), 1);
        assert_eq!(
            joined[0],
            (
                "MLB1".to_owned(),
                OfferToJoin {
                    kind: PromotionKind::PriceDiscount,
                    promotion: None,
                    price: Some(brl("90")),
                    days: Some((on(4), on(10))),
                }
            )
        );
        let offers = fx.promotions.offers().await.unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].listed.price, Some(brl("90")));
        assert_eq!(offers[0].listed.ends, Some(on(10)));
        assert!(matches!(
            fx.discount(&listed, "85", (4, 10)).await,
            Err(PromotionError::AlreadyDiscounted)
        ));
    });
}

#[test]
fn a_campaign_is_created_once_and_its_listings_join_guarded() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx.one_listing().await;
        let plan = campaign("  Semana do fone  ", (5, 11));
        let request = fx.promotions.prepare_promotion(plan, true).unwrap();
        assert!(request.check().is_none());
        *fx.channel.loses_answer.lock().unwrap() = true;
        assert!(
            fx.promotions
                .send(&fx.channel, request.clone().confirm())
                .await
                .is_err()
        );
        fx.promotions
            .send(&fx.channel, request.confirm())
            .await
            .unwrap();

        assert_eq!(fx.channel.created.lock().unwrap().len(), 1);
        assert_eq!(fx.channel.created.lock().unwrap()[0].name, "Semana do fone");
        let kept = fx.promotions.promotions().await.unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].listed.id, "C-MLB100");
        assert_eq!(kept[0].listed.status, PromotionStatus::Pending);

        let too_low = fx
            .promotions
            .prepare_join(
                kept[0].id,
                listed.id,
                Some(brl("45")),
                true,
                &fx.sales,
                assumptions(),
            )
            .await;
        assert!(matches!(too_low, Err(PromotionError::BelowMinimum(_))));
        let join = fx
            .promotions
            .prepare_join(
                kept[0].id,
                listed.id,
                Some(brl("85")),
                true,
                &fx.sales,
                assumptions(),
            )
            .await
            .unwrap();
        fx.promotions
            .send(&fx.channel, join.confirm())
            .await
            .unwrap();

        let offers = fx.promotions.offers().await.unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].listed.promotion.as_deref(), Some("C-MLB100"));
        assert_eq!(offers[0].listed.name, "Semana do fone");
        assert_eq!(offers[0].listed.ends, Some(on(11)));
        assert!(matches!(
            fx.promotions
                .prepare_join(
                    kept[0].id,
                    listed.id,
                    Some(brl("80")),
                    true,
                    &fx.sales,
                    assumptions()
                )
                .await,
            Err(PromotionError::AlreadyIn)
        ));
    });
}

#[test]
fn a_coupon_is_guarded_at_the_price_a_buyer_of_this_listing_alone_pays() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx.one_listing().await;
        let kept = fx.created(coupon("Vinte off", twenty_off())).await;
        assert_eq!(kept.listed.coupon, Some(twenty_off()));
        let big = fx
            .created(coupon(
                "Cinquenta off",
                CouponTerms {
                    discount: CouponDiscount::Amount(brl("50")),
                    ..twenty_off()
                },
            ))
            .await;

        let join = fx
            .promotions
            .prepare_join(kept.id, listed.id, None, true, &fx.sales, assumptions())
            .await
            .unwrap();
        // R$ 100 − R$ 20 = R$ 80 a unit; free shipping still applies.
        let check = *join.check().unwrap();
        assert!(check.after_coupon);
        assert_eq!(check.margin.breakdown.sale_price, brl("80"));
        assert_eq!(check.margin.breakdown.margin.amount, brl("9"));
        let refused = fx
            .promotions
            .prepare_join(big.id, listed.id, None, true, &fx.sales, assumptions())
            .await;
        // R$ 50 off leaves R$ 50 a unit, below the free shipping price:
        // R$ 5, just the 10%.
        let Ok(big_join) = refused else {
            panic!("expected R$ 50 a unit to keep 10%, got {refused:?}");
        };
        assert_eq!(
            big_join.check().unwrap().margin.breakdown.sale_price,
            brl("50")
        );

        fx.promotions
            .send(&fx.channel, join.confirm())
            .await
            .unwrap();
        // With the coupon on, a discount to R$ 85 sells at R$ 65 a unit.
        let discount = fx.discount(&listed, "85", (5, 10)).await.unwrap();
        let check = discount.check().unwrap();
        assert!(check.after_coupon);
        assert_eq!(check.margin.breakdown.sale_price, brl("65"));
        assert_eq!(
            fx.channel.joined.lock().unwrap()[0].1,
            OfferToJoin {
                kind: PromotionKind::Coupon,
                promotion: Some(kept.listed.id.clone()),
                price: None,
                days: None,
            }
        );
    });
}

#[test]
fn a_started_promotion_keeps_its_first_day_when_it_changes() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.channel
            .promotions
            .lock()
            .unwrap()
            .push(ChannelPromotion {
                id: "C-MLB7".into(),
                kind: PromotionKind::SellerCampaign,
                name: "Semana do fone".into(),
                starts: on(1),
                ends: on(10),
                status: PromotionStatus::Started,
                coupon: None,
            });
        fx.promotions.sync(&fx.channel).await.unwrap();
        let kept = fx.promotions.promotions().await.unwrap()[0].id;

        let moved = fx
            .promotions
            .prepare_change(kept, "Semana do fone", on(4), on(12))
            .await;
        let too_long = fx
            .promotions
            .prepare_change(kept, "Semana do fone", on(1), on(15))
            .await;
        let change = fx
            .promotions
            .prepare_change(kept, " Quinzena do fone ", on(1), on(14))
            .await
            .unwrap();
        fx.promotions
            .send(&fx.channel, change.confirm())
            .await
            .unwrap();

        assert!(matches!(moved, Err(PromotionError::AlreadyStarted)));
        assert!(matches!(too_long, Err(PromotionError::TooLong(14))));
        let changed = fx.channel.changed.lock().unwrap().clone();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].0, "C-MLB7");
        assert_eq!(changed[0].1.name, "Quinzena do fone");
        let kept = fx.promotions.promotion(kept).await.unwrap().listed;
        assert_eq!(kept.name, "Quinzena do fone");
        assert_eq!((kept.starts, kept.ends), (on(1), on(14)));
    });
}

#[test]
fn ending_takes_the_listing_out_and_a_campaign_ends_with_its_listings() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx.one_listing().await;
        let kept = fx.created(campaign("Semana do fone", (5, 11))).await;
        let join = fx
            .promotions
            .prepare_join(
                kept.id,
                listed.id,
                Some(brl("85")),
                true,
                &fx.sales,
                assumptions(),
            )
            .await
            .unwrap();
        fx.promotions
            .send(&fx.channel, join.confirm())
            .await
            .unwrap();
        let discount = fx.discount(&listed, "90", (4, 10)).await.unwrap();
        fx.promotions
            .send(&fx.channel, discount.confirm())
            .await
            .unwrap();
        let offers = fx.promotions.offers().await.unwrap();
        assert_eq!(offers.len(), 2);
        let own_discount = offers
            .iter()
            .find(|offer| offer.listed.kind == PromotionKind::PriceDiscount)
            .unwrap();

        fx.promotions
            .end_offer(&fx.channel, own_discount.id)
            .await
            .unwrap();
        assert_eq!(
            fx.channel.left.lock().unwrap()[0],
            ("MLB1".to_owned(), PromotionKind::PriceDiscount, None)
        );
        assert_eq!(fx.promotions.offers().await.unwrap().len(), 1);

        fx.promotions
            .end_promotion(&fx.channel, kept.id)
            .await
            .unwrap();
        assert_eq!(*fx.channel.ended.lock().unwrap(), ["C-MLB100"]);
        assert!(fx.promotions.promotions().await.unwrap().is_empty());
        assert!(fx.promotions.offers().await.unwrap().is_empty());
    });
}

#[test]
fn the_channels_own_campaigns_are_shown_but_not_ended_from_the_app() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx.one_listing().await;
        fx.channel.has_offer(ChannelOffer {
            listing: "MLB1".into(),
            kind: PromotionKind::ChannelCampaign,
            promotion: Some("P-MLB1806019".into()),
            name: "HOTSALE".into(),
            status: PromotionStatus::Started,
            price: Some(brl("88")),
            starts: Some(on(1)),
            ends: Some(on(8)),
        });
        fx.promotions.sync(&fx.channel).await.unwrap();
        let offer = fx.promotions.offers().await.unwrap()[0].clone();

        let ended = fx.promotions.end_offer(&fx.channel, offer.id).await;
        // HOTSALE's R$ 88 is the lowest price the listing sells at now.
        let margin = fx
            .promotions
            .margin_of("MLB1", &fx.sales, assumptions())
            .await
            .unwrap();

        assert!(matches!(ended, Err(PromotionError::NotOwn)));
        assert!(fx.channel.left.lock().unwrap().is_empty());
        assert_eq!(margin.margin.breakdown.sale_price, brl("88"));
        assert!(margin.margin.passes());
        assert_eq!(listed.listed.price, brl("100"));
    });
}

#[test]
fn a_sync_keeps_what_the_channel_reports_and_drops_what_ended() {
    block_on(async {
        let fx = Fixture::new().await;
        let open = fx
            .listed(vec![(priced("MLB1", "100"), 1), (priced("MLB2", "60"), 2)])
            .await;
        fx.sales.change("MLB2", |listing| {
            listing.status = ListingStatus::Closed;
        });
        fx.listings.sync(&fx.sales).await.unwrap();
        fx.channel
            .promotions
            .lock()
            .unwrap()
            .push(ChannelPromotion {
                id: "C-MLB7".into(),
                kind: PromotionKind::SellerCampaign,
                name: "Semana do fone".into(),
                starts: on(5),
                ends: on(11),
                status: PromotionStatus::Pending,
                coupon: None,
            });
        fx.channel.has_offer(ChannelOffer {
            listing: "MLB1".into(),
            kind: PromotionKind::SellerCampaign,
            promotion: Some("C-MLB7".into()),
            name: "Semana do fone".into(),
            status: PromotionStatus::Pending,
            price: Some(brl("85")),
            starts: Some(on(5)),
            ends: Some(on(11)),
        });
        assert!(fx.promotions.is_due().await.unwrap());

        let first = fx.promotions.sync(&fx.channel).await.unwrap();
        let kept = fx.promotions.offers().await.unwrap();
        let again = fx.promotions.sync(&fx.channel).await.unwrap();

        assert_eq!((first.promotions, first.offers, first.ended), (1, 1, 0));
        assert_eq!(again.ended, 0);
        let unchanged = |offers: Vec<mascate_commerce::Offer>| {
            offers
                .into_iter()
                .map(|offer| (offer.id, offer.listed))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            unchanged(fx.promotions.offers().await.unwrap()),
            unchanged(kept)
        );
        // The closed listing is never asked about.
        assert!(
            fx.channel
                .read
                .lock()
                .unwrap()
                .iter()
                .all(|read| read == "MLB1")
        );
        assert!(!fx.promotions.is_due().await.unwrap());
        assert_eq!(open.len(), 2);

        // The campaign finished on the channel.
        fx.channel.promotions.lock().unwrap().clear();
        fx.channel.offers.lock().unwrap().clear();
        let ended = fx.promotions.sync(&fx.channel).await.unwrap();
        assert_eq!(ended.ended, 2);
        assert!(fx.promotions.promotions().await.unwrap().is_empty());
        assert!(fx.promotions.offers().await.unwrap().is_empty());
    });
}

#[test]
fn a_failed_read_writes_nothing() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.one_listing().await;
        fx.channel.has_offer(ChannelOffer {
            listing: "MLB1".into(),
            kind: PromotionKind::PriceDiscount,
            promotion: None,
            name: String::new(),
            status: PromotionStatus::Started,
            price: Some(brl("90")),
            starts: Some(on(1)),
            ends: Some(on(10)),
        });
        fx.promotions.sync(&fx.channel).await.unwrap();
        *fx.channel.failure.lock().unwrap() = Some(PlatformError::RateLimited);

        let failed = fx.promotions.sync(&fx.channel).await;

        assert!(matches!(
            failed,
            Err(PromotionError::Platform(PlatformError::RateLimited))
        ));
        assert_eq!(fx.promotions.offers().await.unwrap().len(), 1);
    });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// The guard accepts a discount exactly when the sale keeps the
    /// minimum margin, and the lowest price it names always does.
    #[test]
    fn the_guard_lets_through_only_what_keeps_the_minimum(
        price in 100i64..9_999,
        minimum in 0i64..400,
    ) {
        block_on(async {
            let fx = Fixture::new().await;
            let listed = fx.one_listing().await;
            fx.promotions
                .save_minimum_margin(Percentage::new(Decimal::new(minimum, 1)).unwrap())
                .await
                .unwrap();

            let price = Decimal::new(price, 2).to_string();
            match fx.discount(&listed, &price, (4, 10)).await {
                Ok(request) => assert!(request.check().unwrap().margin.passes()),
                Err(PromotionError::BelowMinimum(check)) => {
                    assert!(!check.margin.passes());
                    let lowest = check.margin.lowest.unwrap();
                    if lowest.amount() < Decimal::from(100) {
                        let at_lowest = fx
                            .discount(&listed, &lowest.amount().to_string(), (4, 10))
                            .await
                            .unwrap();
                        assert!(at_lowest.check().unwrap().margin.passes());
                    }
                }
                Err(error) => panic!("{error:?}"),
            }
        });
    }
}
