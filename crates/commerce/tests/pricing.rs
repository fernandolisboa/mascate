//! Prices through commerce's public interface: the margin a sale leaves Fee
//! by Fee (the simulator), the lowest price for a target margin, target
//! margins, and Price Suggestions with a real temporary database, Inventory
//! for the Average Cost and an in-memory Sales Channel.

mod common;

use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    ChannelListing, Listing, ListingStatus, Listings, PriceAssumptions, PriceScenario,
    PriceSuggestion, Pricing, PricingError, SaleFee, TargetMargin,
};
use mascate_inventory::{HOME_LOCATION, Inventory, MovementReason, NewEntry};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, Percentage, PlatformError, RecordId};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

use common::{Channel, brl, listing, percent, variation};

fn fee(rate: &str, fixed: &str) -> SaleFee {
    SaleFee {
        rate: percent(rate),
        fixed: brl(fixed),
    }
}

/// R$ 100 listed, 14% sale fee, R$ 20 shipping, R$ 5 of Ads, 6% tax and a
/// unit that cost R$ 35.
fn sale() -> PriceScenario {
    PriceScenario {
        price: brl("100"),
        discount: Percentage::ZERO,
        sale_fee: fee("14", "0"),
        shipping: brl("20"),
        ads: brl("5"),
        tax: percent("6"),
        cost: brl("35"),
    }
}

fn cents(amount: Decimal) -> Money {
    Money::new(amount, Currency::Brl)
}

#[test]
fn the_simulator_takes_each_fee_and_the_cost_off_the_price() {
    let breakdown = sale().breakdown().unwrap();

    assert_eq!(breakdown.sale_price, brl("100"));
    assert_eq!(breakdown.sale_fee, brl("14"));
    assert_eq!(breakdown.shipping, brl("20"));
    assert_eq!(breakdown.ads, brl("5"));
    assert_eq!(breakdown.tax, brl("6"));
    assert_eq!(breakdown.cost, brl("35"));
    assert_eq!(breakdown.margin.amount, brl("20"));
    assert_eq!(breakdown.margin.percent, Some(Decimal::from(20)));
}

#[test]
fn a_discount_lowers_the_sale_price_the_fees_are_charged_on() {
    let discounted = PriceScenario {
        discount: percent("10"),
        sale_fee: fee("14", "6.25"),
        ..sale()
    };

    let breakdown = discounted.breakdown().unwrap();

    assert_eq!(breakdown.sale_price, brl("90"));
    assert_eq!(breakdown.sale_fee, brl("18.85"));
    assert_eq!(breakdown.tax, brl("5.4"));
    assert_eq!(breakdown.margin.amount, brl("5.75"));
    assert_eq!(breakdown.margin.percent_to_pt_br(), "6,4%");
}

#[test]
fn the_lowest_price_for_a_target_is_the_first_cent_that_reaches_it() {
    // (R$ 35 cost + R$ 20 shipping + R$ 5 Ads) / (1 − 14% − 6% − 20%) = R$ 100.
    assert_eq!(
        sale().lowest_price_for(percent("20")).unwrap(),
        Some(brl("100"))
    );
    // R$ 60 / (1 − 14% − 6% − 15%) = R$ 92,307…, rounded up to the cent.
    let lower = sale().lowest_price_for(percent("15")).unwrap().unwrap();
    assert_eq!(lower, brl("92.31"));
    assert!(sale().at(lower).reaches(percent("15")).unwrap());
    assert!(!sale().at(brl("92.30")).reaches(percent("15")).unwrap());
}

#[test]
fn no_price_reaches_a_target_the_fee_and_tax_leave_no_room_for() {
    assert_eq!(sale().lowest_price_for(percent("80")).unwrap(), None);
    let all_off = PriceScenario {
        discount: percent("100"),
        ..sale()
    };
    assert_eq!(all_off.lowest_price_for(percent("0")).unwrap(), None);
}

#[test]
fn with_nothing_to_pay_the_lowest_price_is_one_cent() {
    let free = PriceScenario {
        shipping: brl("0"),
        ads: brl("0"),
        cost: brl("0"),
        ..sale()
    };
    assert_eq!(
        free.lowest_price_for(percent("30")).unwrap(),
        Some(brl("0.01"))
    );
}

fn any_sale() -> impl Strategy<Value = PriceScenario> {
    (
        (0i64..2_000_000, 0i64..300, 0i64..1_000),
        (0i64..8_000, 0i64..3_000, 0i64..300),
        (0i64..600, 1i64..100_000),
    )
        .prop_map(
            |((cost, fee_rate, fixed), (shipping, ads, tax), (discount, price))| PriceScenario {
                price: cents(Decimal::new(price, 2)),
                discount: Percentage::new(Decimal::new(discount, 1)).unwrap(),
                sale_fee: SaleFee {
                    rate: Percentage::new(Decimal::new(fee_rate, 1)).unwrap(),
                    fixed: cents(Decimal::new(fixed, 2)),
                },
                shipping: cents(Decimal::new(shipping, 2)),
                ads: cents(Decimal::new(ads, 2)),
                tax: Percentage::new(Decimal::new(tax, 1)).unwrap(),
                cost: cents(Decimal::new(cost, 2)),
            },
        )
}

proptest! {
    /// The suggested price leaves exactly the target margin, to the cent:
    /// it reaches the target, and one cent less does not.
    #[test]
    fn the_lowest_price_reaches_the_target_and_a_cent_less_does_not(
        sale in any_sale(),
        target in (0i64..700).prop_map(|tenths| Percentage::new(Decimal::new(tenths, 1)).unwrap()),
    ) {
        let found = sale.lowest_price_for(target).unwrap();
        let shares = sale.sale_fee.rate.percent() + sale.tax.percent() + target.percent();
        match found {
            Some(price) => {
                prop_assert!(price.amount().scale() <= 2);
                prop_assert!(sale.at(price).reaches(target).unwrap());
                let cent_less = price.amount() - Decimal::new(1, 2);
                if cent_less > Decimal::ZERO {
                    prop_assert!(!sale.at(cents(cent_less)).reaches(target).unwrap());
                }
            }
            None => prop_assert!(
                shares >= Decimal::ONE_HUNDRED || sale.discount.percent() == Decimal::ONE_HUNDRED
            ),
        }
    }

    /// The margin is the sale price minus every amount the breakdown lists.
    #[test]
    fn the_breakdown_adds_up(sale in any_sale()) {
        let b = sale.breakdown().unwrap();
        let taken = b.sale_fee.amount() + b.shipping.amount() + b.ads.amount() + b.tax.amount()
            + b.cost.amount();
        prop_assert_eq!(b.margin.amount.amount(), b.sale_price.amount() - taken);
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    database: Arc<Database>,
    inventory: Arc<Inventory>,
    listings: Listings,
    pricing: Pricing,
    channel: Channel,
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
            listings: Listings::new(database.clone(), clock.clone(), ids.clone()),
            pricing: Pricing::new(database.clone(), inventory.clone(), clock.clone(), ids),
            _dir: dir,
            clock,
            database,
            inventory,
            channel: Channel::default(),
        }
    }

    /// The channel's listings, synced, each variation linked to a Product.
    async fn listed(&self, listings: Vec<(ChannelListing, Option<u128>)>) -> Vec<Listing> {
        self.channel.has(
            listings
                .iter()
                .map(|(listing, _)| listing.clone())
                .collect(),
        );
        self.listings.sync(&self.channel).await.unwrap();
        let mut synced = Vec::new();
        for (listed, linked) in listings {
            let found = self
                .listings
                .listings()
                .await
                .unwrap()
                .into_iter()
                .find(|l| l.listed.id == listed.id && l.listed.variation == listed.variation)
                .unwrap();
            synced.push(match linked {
                Some(n) => self.listings.link(found.id, product(n)).await.unwrap(),
                None => found,
            });
        }
        synced
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

    async fn suggest(&self, listing: &Listing) -> Result<PriceSuggestion, PricingError> {
        self.pricing
            .suggest(listing.id, &self.channel, assumptions())
            .await
    }
}

fn priced(id: &str, price: &str) -> ChannelListing {
    ChannelListing {
        price: brl(price),
        ..listing(id, "Fone Bluetooth TWS")
    }
}

#[test]
fn every_product_takes_the_default_target_margin_until_it_has_its_own() {
    block_on(async {
        let fx = Fixture::new().await;
        let default = TargetMargin {
            margin: percent("20"),
            own: false,
        };
        assert_eq!(fx.pricing.target_margin(product(1)).await.unwrap(), default);

        fx.pricing
            .save_default_target_margin(percent("25"))
            .await
            .unwrap();
        fx.pricing
            .set_target_margin(product(1), Some(percent("12.5")))
            .await
            .unwrap();

        assert_eq!(
            fx.pricing.target_margin(product(1)).await.unwrap(),
            TargetMargin {
                margin: percent("12.5"),
                own: true
            }
        );
        assert_eq!(
            fx.pricing.target_margin(product(2)).await.unwrap(),
            TargetMargin {
                margin: percent("25"),
                own: false
            }
        );

        fx.pricing
            .set_target_margin(product(1), None)
            .await
            .unwrap();
        fx.pricing
            .set_target_margin(product(1), Some(percent("30")))
            .await
            .unwrap();
        fx.pricing
            .set_target_margin(product(1), None)
            .await
            .unwrap();

        assert_eq!(
            fx.pricing.target_margin(product(1)).await.unwrap(),
            TargetMargin {
                margin: percent("25"),
                own: false
            }
        );
    });
}

#[test]
fn a_target_margin_of_a_hundred_percent_is_refused() {
    block_on(async {
        let fx = Fixture::new().await;

        let own = fx
            .pricing
            .set_target_margin(product(1), Some(percent("100")))
            .await;
        let default = fx.pricing.save_default_target_margin(percent("100")).await;

        assert!(matches!(own, Err(PricingError::InvalidTarget)));
        assert!(matches!(default, Err(PricingError::InvalidTarget)));
        assert_eq!(
            fx.pricing.default_target_margin().await.unwrap(),
            percent("20")
        );
    });
}

#[test]
fn the_suggestion_covers_the_fee_shipping_tax_and_average_cost_and_sends_nothing() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx.listed(vec![(priced("MLB1", "89.90"), Some(1))]).await;
        // Two units at R$ 30 and two at R$ 40: R$ 35 each.
        fx.stock(1, 2, "60").await;
        fx.stock(1, 2, "80").await;

        let suggestion = fx.suggest(&listed[0]).await.unwrap();

        // (R$ 35 + R$ 20 shipping) / (1 − 14% − 6% − 20%) = R$ 91,666…
        assert_eq!(suggestion.price, brl("91.67"));
        assert_eq!(suggestion.listings, [listed[0].id]);
        assert_eq!(suggestion.product, product(1));
        assert_eq!(suggestion.target.margin, percent("20"));
        let at_price = suggestion.suggested.breakdown().unwrap();
        assert_eq!(at_price.sale_fee, brl("12.8338"));
        assert_eq!(at_price.shipping, brl("20"));
        assert_eq!(at_price.tax, brl("5.5002"));
        assert_eq!(at_price.cost, brl("35"));
        assert_eq!(at_price.margin.amount, brl("18.3360"));
        assert!(suggestion.suggested.reaches(percent("20")).unwrap());
        assert_eq!(suggestion.current.price, brl("89.90"));
        assert!(!suggestion.current.reaches(percent("20")).unwrap());
        assert_eq!(
            *fx.channel.fee_asked.lock().unwrap(),
            [brl("89.90"), brl("91.67")]
        );
        assert!(fx.channel.prices_set.lock().unwrap().is_empty());
        assert_eq!(
            fx.listings
                .listing(listed[0].id)
                .await
                .unwrap()
                .listed
                .price,
            brl("89.90")
        );
    });
}

#[test]
fn a_cheap_product_drops_below_the_free_shipping_price_and_pays_no_shipping() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx.listed(vec![(priced("MLB1", "120"), Some(1))]).await;
        fx.stock(1, 1, "10").await;

        let suggestion = fx.suggest(&listed[0]).await.unwrap();

        // With shipping, R$ 30 / 0,6 = R$ 50, where the buyer pays it:
        // R$ 10 / 0,6 = R$ 16,67.
        assert_eq!(suggestion.price, brl("16.67"));
        assert_eq!(suggestion.suggested.shipping, brl("0"));
        assert_eq!(suggestion.current.shipping, brl("20"));
    });
}

#[test]
fn the_suggestion_follows_the_fee_into_the_band_its_price_falls_in() {
    block_on(async {
        let fx = Fixture::new().await;
        // Below R$ 79 the channel adds R$ 6,25 to each sale.
        *fx.channel.fee_bands.lock().unwrap() = vec![(brl("0"), fee("14", "6.25"))];
        fx.channel.charges_from("79", fee("14", "0"));
        let listed = fx.listed(vec![(priced("MLB1", "89.90"), Some(1))]).await;
        fx.stock(1, 1, "10").await;

        let suggestion = fx.suggest(&listed[0]).await.unwrap();

        // (R$ 10 + R$ 6,25) / 0,6 = R$ 27,083…
        assert_eq!(suggestion.price, brl("27.09"));
        assert_eq!(suggestion.suggested.sale_fee, fee("14", "6.25"));
        assert_eq!(suggestion.current.sale_fee, fee("14", "0"));
        assert!(suggestion.suggested.reaches(percent("20")).unwrap());
    });
}

#[test]
fn variations_share_the_price_that_gives_each_its_target() {
    block_on(async {
        let fx = Fixture::new().await;
        let base = priced("MLB1", "99.90");
        let listed = fx
            .listed(vec![
                (variation(&base, "11", "Cor: Preto"), Some(1)),
                (variation(&base, "12", "Cor: Branco"), Some(2)),
            ])
            .await;
        fx.stock(1, 1, "35").await;
        fx.stock(2, 1, "50").await;
        fx.pricing
            .set_target_margin(product(1), Some(percent("30")))
            .await
            .unwrap();

        let suggestion = fx.suggest(&listed[0]).await.unwrap();

        // Product 1: R$ 55 / 0,5 = R$ 110. Product 2: R$ 70 / 0,6 = R$ 116,67.
        assert_eq!(suggestion.price, brl("116.67"));
        assert_eq!(suggestion.product, product(2));
        assert_eq!(suggestion.target.margin, percent("20"));
        let mut together = suggestion.listings.clone();
        together.sort();
        assert_eq!(together, [listed[0].id, listed[1].id]);
    });
}

#[test]
fn a_variation_without_a_product_or_a_product_never_stocked_has_no_suggestion() {
    block_on(async {
        let fx = Fixture::new().await;
        let base = priced("MLB1", "99.90");
        let listed = fx
            .listed(vec![
                (variation(&base, "11", "Cor: Preto"), Some(1)),
                (variation(&base, "12", "Cor: Branco"), None),
                (priced("MLB2", "50"), Some(3)),
            ])
            .await;
        fx.stock(1, 1, "35").await;

        let unlinked = fx.suggest(&listed[0]).await;
        let unstocked = fx.suggest(&listed[2]).await;

        assert!(matches!(unlinked, Err(PricingError::NotLinked(id)) if id == listed[1].id));
        assert!(matches!(unstocked, Err(PricingError::NoCost(id)) if id == product(3)));
    });
}

#[test]
fn a_closed_variation_is_left_out_and_a_closed_listing_has_no_suggestion() {
    block_on(async {
        let fx = Fixture::new().await;
        let base = priced("MLB1", "99.90");
        let listed = fx
            .listed(vec![
                (variation(&base, "11", "Cor: Preto"), Some(1)),
                (variation(&base, "12", "Cor: Branco"), None),
                (priced("MLB2", "50"), Some(1)),
            ])
            .await;
        fx.stock(1, 1, "35").await;
        fx.channel.remove("MLB1", Some("12"));
        fx.channel
            .change("MLB2", |listing| listing.status = ListingStatus::Closed);
        fx.listings.sync(&fx.channel).await.unwrap();

        let open = fx.suggest(&listed[0]).await.unwrap();
        let ended = fx.suggest(&listed[2]).await;

        assert_eq!(open.listings, [listed[0].id]);
        assert!(matches!(ended, Err(PricingError::Closed)));
    });
}

#[test]
fn a_listing_without_category_or_a_target_out_of_reach_has_no_suggestion() {
    block_on(async {
        let fx = Fixture::new().await;
        let uncategorized = ChannelListing {
            category: None,
            ..priced("MLB1", "50")
        };
        let listed = fx
            .listed(vec![
                (uncategorized, Some(1)),
                (priced("MLB2", "50"), Some(2)),
            ])
            .await;
        fx.stock(1, 1, "10").await;
        fx.stock(2, 1, "10").await;
        fx.pricing
            .set_target_margin(product(2), Some(percent("80")))
            .await
            .unwrap();

        let no_fee = fx.suggest(&listed[0]).await;
        let out_of_reach = fx.suggest(&listed[1]).await;

        assert!(matches!(no_fee, Err(PricingError::NoFeeBasis)));
        assert!(matches!(out_of_reach, Err(PricingError::Unreachable)));
    });
}

#[test]
fn a_channel_failure_is_the_suggestions_failure() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx.listed(vec![(priced("MLB1", "50"), Some(1))]).await;
        fx.stock(1, 1, "10").await;
        *fx.channel.failure.lock().unwrap() = Some(PlatformError::RateLimited);

        let failed = fx.suggest(&listed[0]).await;

        assert!(matches!(
            failed,
            Err(PricingError::Platform(PlatformError::RateLimited))
        ));
    });
}

#[test]
fn the_simulator_starts_from_todays_sale_of_the_listing() {
    block_on(async {
        let fx = Fixture::new().await;
        let listed = fx
            .listed(vec![
                (priced("MLB1", "89.90"), Some(1)),
                (priced("MLB2", "50"), None),
            ])
            .await;
        fx.stock(1, 2, "70").await;

        let today = fx
            .pricing
            .today(listed[0].id, &fx.channel, assumptions())
            .await
            .unwrap();
        let unlinked = fx
            .pricing
            .today(listed[1].id, &fx.channel, assumptions())
            .await;

        assert_eq!(
            today,
            PriceScenario {
                price: brl("89.90"),
                discount: Percentage::ZERO,
                sale_fee: fee("14", "0"),
                shipping: brl("20"),
                ads: brl("0"),
                tax: percent("6"),
                cost: brl("35"),
            }
        );
        assert!(matches!(unlinked, Err(PricingError::NotLinked(id)) if id == listed[1].id));
        assert!(fx.channel.prices_set.lock().unwrap().is_empty());
    });
}

#[test]
fn approving_the_suggestion_sends_its_price_to_the_channel() {
    block_on(async {
        let fx = Fixture::new().await;
        let base = priced("MLB1", "89.90");
        let listed = fx
            .listed(vec![
                (variation(&base, "11", "Cor: Preto"), Some(1)),
                (variation(&base, "12", "Cor: Branco"), Some(1)),
            ])
            .await;
        fx.stock(1, 2, "70").await;
        let suggestion = fx.suggest(&listed[1]).await.unwrap();

        fx.listings
            .change_price(listed[1].id, suggestion.price, &fx.channel)
            .await
            .unwrap();

        assert_eq!(
            *fx.channel.prices_set.lock().unwrap(),
            [("MLB1".to_owned(), brl("91.67"))]
        );
        for listing in &listed {
            let now = fx.listings.listing(listing.id).await.unwrap();
            assert_eq!(now.listed.price, brl("91.67"));
        }
        let again = fx.suggest(&listed[0]).await.unwrap();
        assert_eq!(again.price, brl("91.67"));
        assert!(again.current.reaches(percent("20")).unwrap());
    });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Whatever the cost, target and starting price, the suggestion reaches
    /// the target with the fee and shipping that apply at it, and one cent
    /// less does not.
    #[test]
    fn a_suggestion_gives_exactly_the_target_margin(
        cost in 1i64..50_000,
        target in 0i64..50,
        today in 100i64..100_000,
    ) {
        block_on(async {
            let fx = Fixture::new().await;
            *fx.channel.fee_bands.lock().unwrap() = vec![(brl("0"), fee("12", "6.25"))];
            fx.channel.charges_from("79", fee("14", "0"));
            let target = Percentage::new(Decimal::from(target)).unwrap();
            fx.pricing.save_default_target_margin(target).await.unwrap();
            let listed = fx
                .listed(vec![(
                    ChannelListing {
                        price: cents(Decimal::new(today, 2)),
                        ..listing("MLB1", "Fone")
                    },
                    Some(1),
                )])
                .await;
            fx.stock(1, 1, &Decimal::new(cost, 2).to_string()).await;

            let suggestion = fx.suggest(&listed[0]).await.unwrap();

            let at = suggestion.suggested;
            assert_eq!(at.price, suggestion.price);
            assert!(at.reaches(target).unwrap());
            let cent_less = suggestion.price.amount() - Decimal::new(1, 2);
            if cent_less > Decimal::ZERO {
                assert!(!at.at(cents(cent_less)).reaches(target).unwrap());
            }
            let fee_there = if suggestion.price.amount() >= Decimal::from(79) {
                fee("14", "0")
            } else {
                fee("12", "6.25")
            };
            assert_eq!(at.sale_fee, fee_there);
        });
    }
}
