//! Fees and Realized Margin through commerce's public interface with a real
//! temporary database, the real Inventory and an in-memory Sales Channel
//! and billing: billed Fees are kept once by the channel's id of each
//! charge, an Order not billed yet has a provisional margin from what it
//! reports, the cost is the one the units left the ledger with, and units
//! back on the shelf leave the margin.

mod common;

use std::sync::Arc;

use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    AdCost, BilledOrder, ChannelFee, ChannelOrderLine, ChannelReturn, FeeImport, FeeKind, Listings,
    NewPurchaseLine, NewPurchaseOrder, OrderStatus, Orders, PurchaseOrders, RealizedMargin,
    Receiving, ReturnReceipt, ReturnStatus, ReturnedItem, Sale, SalesPeriod, SalesSummary,
    ShipmentStatus,
};
use mascate_inventory::{HOME_LOCATION, Inventory};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Currency, Money, Percentage, RecordId, Timestamp, channel_day_start};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

use common::{Channel, brl, listing, order, percent, sold};

fn fone() -> RecordId {
    RecordId::from_u128(1_000_010)
}

fn ever() -> std::ops::Range<Timestamp> {
    Timestamp::MIN_UTC..Timestamp::MAX_UTC
}

fn charge(id: &str, kind: FeeKind, description: &str, amount: &str) -> ChannelFee {
    ChannelFee {
        id: id.into(),
        kind,
        description: description.into(),
        amount: brl(amount),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    inventory: Arc<Inventory>,
    purchases: PurchaseOrders,
    orders: Orders,
    channel: Channel,
}

impl Fixture {
    /// A fone listing linked to its Product, 10 fones on hand at R$ 20,00
    /// each, and a first Order Sync with nothing sold yet.
    async fn selling() -> Self {
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
        let listings = Listings::new(database.clone(), clock.clone(), ids.clone());
        let fx = Self {
            _dir: dir,
            purchases: PurchaseOrders::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            ),
            orders: Orders::new(database, inventory.clone(), clock.clone(), ids),
            inventory,
            clock,
            channel: Channel::default(),
        };
        fx.channel.has(vec![listing("MLB1", "Fone Bluetooth")]);
        listings.sync(&fx.channel).await.unwrap();
        let found = listings.listings().await.unwrap();
        listings.link(found[0].id, fone()).await.unwrap();
        fx.receive(10, "20.00").await;
        fx.sync().await;
        fx
    }

    fn later(&self, minutes: i64) -> Timestamp {
        self.clock.advance(TimeDelta::minutes(minutes));
        self.clock.now()
    }

    async fn receive(&self, units: u32, unit_price: &str) {
        self.later(1);
        let order = self
            .purchases
            .create(NewPurchaseOrder {
                supplier: RecordId::from_u128(1_000_001),
                ordered_on: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                freight: brl("0"),
                lines: vec![NewPurchaseLine {
                    product: fone(),
                    quantity: units,
                    unit_price: brl(unit_price),
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

    async fn sync(&self) {
        self.later(5);
        self.orders.sync(&self.channel).await.unwrap();
    }

    /// A buyer buys `units` fones now, at R$ 89,90 each.
    async fn sell(&self, id: &str, units: u32) {
        let at = self.later(1);
        self.channel
            .sells(order(id, at, vec![sold("MLB1", None, units)]));
        self.sync().await;
    }

    fn bill(&self, order: &str, fees: Vec<ChannelFee>) {
        let mut billed = self.channel.billed.lock().unwrap();
        billed.retain(|found| found.order != order);
        billed.push(BilledOrder {
            order: order.into(),
            fees,
        });
    }

    async fn import(&self) -> FeeImport {
        self.orders.import_fees(&self.channel).await.unwrap()
    }

    fn times_billing_asked(&self) -> usize {
        self.channel.billing_asked.lock().unwrap().len()
    }

    async fn order_id(&self, id: &str) -> RecordId {
        self.orders
            .orders()
            .await
            .unwrap()
            .into_iter()
            .find(|order| order.sold.id == id)
            .unwrap()
            .id
    }

    async fn margin(&self, id: &str, tax: &str) -> RealizedMargin {
        self.orders
            .sales(percent(tax), ever(), &[])
            .await
            .unwrap()
            .sales
            .into_iter()
            .find(|sale| sale.order.sold.id == id)
            .unwrap()
            .margin
    }
}

fn amount_of(margin: &RealizedMargin) -> Money {
    margin.margin.unwrap().amount
}

#[test]
fn an_order_not_billed_yet_has_a_provisional_margin_from_what_it_reports() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sell("O1", 2).await;

        assert_eq!(
            fx.import().await,
            FeeImport {
                asked: 1,
                billed: 0
            }
        );
        let margin = fx.margin("O1", "10").await;

        assert!(margin.provisional);
        assert_eq!(margin.revenue, brl("179.80"));
        // R$ 12,59 a unit, as the Order reports it, and what the shipment
        // costs the owner.
        assert_eq!(margin.fees_of(FeeKind::SaleFee), brl("25.18"));
        assert_eq!(margin.fees_of(FeeKind::Shipping), brl("21.45"));
        assert_eq!(margin.tax, brl("17.98"));
        assert_eq!(margin.cost, Some(brl("40.00")));
        assert_eq!(amount_of(&margin), brl("75.19"));
        assert_eq!(margin.margin.unwrap().percent_to_pt_br(), "41,8%");
    });
}

#[test]
fn billed_fees_replace_the_reported_ones_and_reading_them_again_changes_nothing() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sell("O1", 2).await;
        fx.bill(
            "O1",
            vec![
                charge("71-CV", FeeKind::SaleFee, "Tarifa de venda", "25.18"),
                charge("72-CXD", FeeKind::Shipping, "Tarifa de envio", "18.00"),
                charge("73-CFONPN", FeeKind::Other, "Taxa de parcelamento", "1.50"),
            ],
        );

        assert_eq!(
            fx.import().await,
            FeeImport {
                asked: 1,
                billed: 1
            }
        );
        let margin = fx.margin("O1", "10").await;

        assert!(!margin.provisional);
        assert_eq!(margin.fees.len(), 3);
        assert_eq!(margin.fees_total(), brl("44.68"));
        assert_eq!(margin.fees_of(FeeKind::Shipping), brl("18.00"));
        assert_eq!(
            margin
                .fees
                .iter()
                .find(|fee| fee.kind == FeeKind::Other)
                .and_then(|fee| fee.description.as_deref()),
            Some("Taxa de parcelamento")
        );
        assert_eq!(amount_of(&margin), brl("77.14"));

        // A billed Order that did not change is not asked about again.
        let asked = fx.times_billing_asked();
        assert_eq!(fx.import().await, FeeImport::default());
        fx.later(60 * 24);
        assert_eq!(fx.import().await, FeeImport::default());
        assert_eq!(fx.times_billing_asked(), asked);
        assert_eq!(fx.margin("O1", "10").await, margin);
    });
}

#[test]
fn a_changed_order_is_asked_again_and_charges_given_back_lower_the_fees() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sell("O1", 2).await;
        fx.bill(
            "O1",
            vec![
                charge("71-CV", FeeKind::SaleFee, "Tarifa de venda", "25.18"),
                charge("72-CXD", FeeKind::Shipping, "Tarifa de envio", "18.00"),
                charge("73-CFONPN", FeeKind::Other, "Taxa de parcelamento", "1.50"),
            ],
        );
        fx.import().await;

        // The buyer sends one fone back and gets its price back; the
        // channel gives back that unit's sale fee and drops a charge.
        let at = fx.later(60);
        fx.channel.change_order("O1", at, |order| {
            order.refunded = Some(brl("89.90"));
            if let Some(shipment) = order.shipment.as_mut() {
                shipment.status = ShipmentStatus::Delivered;
            }
            order.returns = vec![ChannelReturn {
                id: "R1".into(),
                status: ReturnStatus::Delivered,
                items: vec![ReturnedItem {
                    item: "MLB1".into(),
                    variation: None,
                    quantity: 1,
                }],
            }];
        });
        fx.bill(
            "O1",
            vec![
                charge("71-CV", FeeKind::SaleFee, "Tarifa de venda", "25.18"),
                charge("72-CXD", FeeKind::Shipping, "Tarifa de envio", "18.00"),
                charge("74-BV", FeeKind::SaleFee, "Tarifa devolvida", "-12.59"),
            ],
        );
        fx.sync().await;

        assert_eq!(fx.import().await.billed, 1);
        let margin = fx.margin("O1", "10").await;
        assert_eq!(margin.revenue, brl("89.90"));
        assert_eq!(margin.fees_total(), brl("30.59"));
        assert_eq!(margin.fees_of(FeeKind::Other), brl("0"));
        assert_eq!(margin.tax, brl("8.99"));
        // The unit coming back is still a cost until it is on the shelf.
        assert_eq!(margin.cost, Some(brl("40.00")));
        assert_eq!(amount_of(&margin), brl("10.32"));

        let order = fx.order_id("O1").await;
        fx.orders
            .receive_return(order, ReturnReceipt::BackInStock)
            .await
            .unwrap();
        let margin = fx.margin("O1", "10").await;
        assert_eq!(margin.cost, Some(brl("20.00")));
        assert_eq!(amount_of(&margin), brl("30.32"));
    });
}

#[test]
fn units_back_unfit_to_sell_stay_a_cost() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sell("O1", 2).await;
        let at = fx.later(60);
        fx.channel.change_order("O1", at, |order| {
            if let Some(shipment) = order.shipment.as_mut() {
                shipment.status = ShipmentStatus::NotDelivered;
            }
        });
        fx.sync().await;

        let order = fx.order_id("O1").await;
        fx.orders
            .receive_return(order, ReturnReceipt::Unsellable)
            .await
            .unwrap();

        assert_eq!(fx.margin("O1", "0").await.cost, Some(brl("40.00")));
    });
}

#[test]
fn the_cost_is_what_the_units_left_with_whatever_the_average_cost_does_after() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sell("O1", 2).await;
        // Dearer fones arrive after the sale: the Average Cost moves.
        fx.receive(10, "50.00").await;
        assert_ne!(
            fx.inventory.average_cost(fone()).await.unwrap(),
            Some(brl("20.00"))
        );

        assert_eq!(fx.margin("O1", "0").await.cost, Some(brl("40.00")));
    });
}

#[test]
fn an_order_cancelled_before_shipping_leaves_nothing_and_is_not_asked_about() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sell("O1", 1).await;
        fx.import().await;
        let at = fx.later(30);
        fx.channel.change_order("O1", at, |order| {
            order.status = OrderStatus::Cancelled;
            if let Some(shipment) = order.shipment.as_mut() {
                shipment.status = ShipmentStatus::Cancelled;
            }
        });
        fx.sync().await;

        let asked = fx.times_billing_asked();
        assert_eq!(fx.import().await, FeeImport::default());
        assert_eq!(fx.times_billing_asked(), asked);
        let margin = fx.margin("O1", "10").await;
        assert!(!margin.provisional);
        assert_eq!(margin.revenue, brl("0"));
        assert!(margin.fees.is_empty());
        assert_eq!(margin.tax, brl("0"));
        assert_eq!(margin.cost, Some(brl("0")));
        assert_eq!(amount_of(&margin), brl("0"));
        assert_eq!(margin.margin.unwrap().percent, None);
    });
}

#[test]
fn an_order_not_billed_is_asked_again_every_six_hours_for_sixty_days() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sell("O1", 1).await;
        assert_eq!(fx.import().await.asked, 1);

        fx.later(5 * 60);
        assert_eq!(fx.import().await.asked, 0);
        fx.later(60);
        assert_eq!(fx.import().await.asked, 1);

        fx.later(61 * 24 * 60);
        assert_eq!(fx.import().await.asked, 0);
        assert!(fx.margin("O1", "0").await.provisional);
    });
}

#[test]
fn an_order_sold_before_the_first_sync_has_no_cost_and_no_margin() {
    block_on(async {
        let fx = Fixture::selling().await;
        let now = fx.later(1);
        let mut before = order("O0", now - TimeDelta::days(2), vec![sold("MLB1", None, 1)]);
        before.updated_at = now;
        fx.channel.sells(before);
        fx.sync().await;

        let margin = fx.margin("O0", "0").await;

        assert_eq!(margin.revenue, brl("89.90"));
        assert_eq!(margin.cost, None);
        assert_eq!(margin.margin, None);
    });
}

#[test]
fn orders_shipped_together_share_what_the_shipment_costs_by_value() {
    block_on(async {
        let fx = Fixture::selling().await;
        let at = fx.later(1);
        for (id, units) in [("O1", 2), ("O2", 1)] {
            let mut together = order(id, at, vec![sold("MLB1", None, units)]);
            together.pack = Some("P1".into());
            if let Some(shipment) = together.shipment.as_mut() {
                shipment.id = "SHARED".into();
            }
            fx.channel.sells(together);
        }
        fx.sync().await;

        let first = fx.margin("O1", "0").await.fees_of(FeeKind::Shipping);
        let second = fx.margin("O2", "0").await.fees_of(FeeKind::Shipping);

        assert_eq!(first, brl("14.30"));
        assert_eq!(second, brl("7.15"));
        assert_eq!(first.checked_add(second).unwrap(), brl("21.45"));
    });
}

#[test]
fn sales_are_the_orders_sold_within_the_period_newest_first_and_add_up() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sell("O1", 1).await;
        let from = fx.later(1);
        fx.sell("O2", 2).await;
        fx.sell("O3", 1).await;
        let to = fx.later(1);
        fx.sell("O4", 1).await;

        let sales = fx.orders.sales(percent("0"), from..to, &[]).await.unwrap();

        let ids: Vec<&str> = sales
            .sales
            .iter()
            .map(|sale| sale.order.sold.id.as_str())
            .collect();
        assert_eq!(ids, ["O3", "O2"]);
        let summary = SalesSummary::of(&sales, Currency::Brl).unwrap();
        assert_eq!(summary.orders, 2);
        assert_eq!(summary.revenue, brl("269.70"));
        assert_eq!(summary.cost, brl("60.00"));
        assert_eq!(summary.provisional, 2);
        assert_eq!(summary.without_cost, 0);
        // 269,70 − 3 × 12,59 − 2 × 21,45 − 60,00.
        assert_eq!(summary.margin.unwrap().amount, brl("129.03"));
    });
}

#[test]
fn a_summary_leaves_sales_without_a_cost_out_of_the_margin() {
    block_on(async {
        let fx = Fixture::selling().await;
        let now = fx.later(1);
        let mut before = order("O0", now - TimeDelta::days(2), vec![sold("MLB1", None, 1)]);
        before.updated_at = now;
        fx.channel.sells(before);
        fx.sell("O1", 1).await;

        let sales = fx.orders.sales(percent("0"), ever(), &[]).await.unwrap();
        let summary = SalesSummary::of(&sales, Currency::Brl).unwrap();

        assert_eq!(summary.orders, 2);
        assert_eq!(summary.without_cost, 1);
        assert_eq!(summary.revenue, brl("179.80"));
        // Only O1: 89,90 − 12,59 − 21,45 − 20,00.
        assert_eq!(summary.margin.unwrap().amount, brl("35.86"));
    });
}

fn ad(listing: &str, month: u32, day: u32, cost: &str) -> AdCost {
    AdCost {
        listing: listing.into(),
        day: NaiveDate::from_ymd_opt(2026, month, day).unwrap(),
        cost: brl(cost),
    }
}

fn sale_of<'a>(period: &'a SalesPeriod, id: &str) -> &'a Sale {
    period
        .sales
        .iter()
        .find(|sale| sale.order.sold.id == id)
        .unwrap()
}

#[test]
fn a_listings_ads_in_a_month_split_across_its_sales_that_month_by_value() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sell("O1", 1).await;
        fx.sell("O2", 2).await;
        let ads = [ad("MLB1", 10, 1, "10.00"), ad("MLB1", 10, 3, "20.00")];

        let period = fx.orders.sales(percent("0"), ever(), &ads).await.unwrap();

        let one = &sale_of(&period, "O1").margin;
        let two = &sale_of(&period, "O2").margin;
        // R$ 30 split 89,90 to 179,80.
        assert_eq!(one.fees_of(FeeKind::Ads), brl("10.00"));
        assert_eq!(two.fees_of(FeeKind::Ads), brl("20.00"));
        assert_eq!(one.channel_fees(), brl("34.04"));
        // 89,90 − 12,59 − 21,45 − 20,00 − 10,00.
        assert_eq!(amount_of(one), brl("25.86"));
        assert!(period.unplaced_ads.is_empty());
        let summary = SalesSummary::of(&period, Currency::Brl).unwrap();
        assert_eq!(summary.ads, brl("30.00"));
        assert_eq!(summary.unplaced_ads, brl("0"));
        assert_eq!(summary.fees, brl("80.67"));
    });
}

#[test]
fn ads_in_a_month_without_a_sale_stay_apart_and_count_in_their_period() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sell("O1", 1).await;
        let ads = [
            ad("MLB1", 9, 20, "15.00"),
            ad("MLB9", 10, 2, "5.00"),
            ad("MLB1", 10, 2, "8.00"),
        ];
        let october = channel_day_start(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap())
            ..channel_day_start(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());

        let all = fx.orders.sales(percent("0"), ever(), &ads).await.unwrap();
        let this_month = fx.orders.sales(percent("0"), october, &ads).await.unwrap();

        assert_eq!(
            sale_of(&all, "O1").margin.fees_of(FeeKind::Ads),
            brl("8.00")
        );
        assert_eq!(all.unplaced_ads, [ads[0].clone(), ads[1].clone()]);
        assert_eq!(this_month.unplaced_ads, [ads[1].clone()]);
        let summary = SalesSummary::of(&this_month, Currency::Brl).unwrap();
        assert_eq!(summary.ads, brl("13.00"));
        assert_eq!(summary.unplaced_ads, brl("5.00"));
        // O1's 89,90 − 12,59 − 21,45 − 20,00 − 8,00, less the R$ 5 no sale took.
        assert_eq!(summary.margin.unwrap().amount, brl("22.86"));
    });
}

#[test]
fn a_cancelled_order_takes_no_ads() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sell("O1", 1).await;
        let at = fx.later(1);
        let mut cancelled = order("O2", at, vec![sold("MLB1", None, 3)]);
        cancelled.status = OrderStatus::Cancelled;
        fx.channel.sells(cancelled);
        fx.sync().await;

        let period = fx
            .orders
            .sales(percent("0"), ever(), &[ad("MLB1", 10, 4, "12.00")])
            .await
            .unwrap();

        assert_eq!(
            sale_of(&period, "O1").margin.fees_of(FeeKind::Ads),
            brl("12.00")
        );
        assert_eq!(
            sale_of(&period, "O2").margin.fees_of(FeeKind::Ads),
            brl("0")
        );
    });
}

#[test]
fn ads_only_ever_come_from_the_caller() {
    block_on(async {
        let fx = Fixture::selling().await;
        fx.sell("O1", 1).await;
        let ads = [ad("MLB1", 10, 4, "7.77")];

        let with = fx.orders.sales(percent("0"), ever(), &ads).await.unwrap();
        let again = fx.orders.sales(percent("0"), ever(), &ads).await.unwrap();
        let without = fx.orders.sales(percent("0"), ever(), &[]).await.unwrap();

        assert_eq!(with, again);
        assert_eq!(
            sale_of(&without, "O1").margin.fees_of(FeeKind::Ads),
            brl("0")
        );
    });
}

fn margin_of(sale: &Sale) -> Money {
    sale.margin.margin.unwrap().amount
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// However Product Ads fall across listings and months, the sales'
    /// shares and what no sale took add up to exactly what the Ads cost.
    #[test]
    fn every_cent_of_ads_is_placed_once(
        sales in prop::collection::vec(1u32..4, 0..4),
        ads in prop::collection::vec((any::<bool>(), 9u32..=10, 1u32..=28, 1i64..5000), 1..8),
    ) {
        block_on(async {
            let fx = Fixture::selling().await;
            for (index, units) in sales.iter().enumerate() {
                fx.sell(&format!("O{index}"), *units).await;
            }
            let ads: Vec<AdCost> = ads
                .iter()
                .map(|(other, month, day, cents)| AdCost {
                    listing: if *other { "MLB9" } else { "MLB1" }.into(),
                    day: NaiveDate::from_ymd_opt(2026, *month, *day).unwrap(),
                    cost: Money::new(Decimal::new(*cents, 2), Currency::Brl),
                })
                .collect();

            let period = fx.orders.sales(percent("0"), ever(), &ads).await.unwrap();

            let spent = Money::sum(Currency::Brl, ads.iter().map(|ad| ad.cost)).unwrap();
            let summary = SalesSummary::of(&period, Currency::Brl).unwrap();
            prop_assert_eq!(summary.ads, spent);
            for sale in &period.sales {
                let margin = &sale.margin;
                let expected = margin
                    .revenue
                    .checked_sub(margin.fees_total())
                    .and_then(|left| left.checked_sub(margin.tax))
                    .and_then(|left| left.checked_sub(margin.cost.unwrap()))
                    .unwrap();
                prop_assert_eq!(margin_of(sale), expected);
            }
            Ok(())
        })?;
    }

    /// Whatever was sold, billed and taxed, each margin is the revenue less
    /// every Fee, the tax and the cost, the Fees by kind add up to all of
    /// them, and the summary's margin is the sales' margins together.
    #[test]
    fn a_margin_is_the_revenue_less_fees_tax_and_cost(
        sales in prop::collection::vec((1u32..4, any::<bool>(), 0u32..3000), 1..5),
        tax in 0u32..30,
    ) {
        block_on(async {
            let fx = Fixture::selling().await;
            // Enough fones for every sale to leave the stock.
            fx.receive(10, "20.00").await;
            for (index, (units, billed, cents)) in sales.iter().enumerate() {
                let id = format!("O{index}");
                fx.sell(&id, *units).await;
                if *billed {
                    fx.bill(&id, vec![
                        charge("1-CV", FeeKind::SaleFee, "Tarifa de venda", "10.00"),
                        ChannelFee {
                            amount: Money::new(Decimal::new(i64::from(*cents), 2), Currency::Brl),
                            ..charge("2-CXD", FeeKind::Shipping, "Envio", "0")
                        },
                    ]);
                }
            }
            fx.import().await;
            let tax = Percentage::new(Decimal::from(tax)).unwrap();

            let found = fx.orders.sales(tax, ever(), &[]).await.unwrap();

            let mut total = brl("0");
            for sale in &found.sales {
                let margin = &sale.margin;
                let expected = margin
                    .revenue
                    .checked_sub(margin.fees_total())
                    .and_then(|left| left.checked_sub(margin.tax))
                    .and_then(|left| left.checked_sub(margin.cost.unwrap()))
                    .unwrap();
                prop_assert_eq!(margin_of(sale), expected);
                let by_kind = Money::sum(
                    Currency::Brl,
                    [FeeKind::SaleFee, FeeKind::Shipping, FeeKind::Other]
                        .map(|kind| margin.fees_of(kind)),
                )
                .unwrap();
                prop_assert_eq!(by_kind, margin.fees_total());
                prop_assert_eq!(margin.tax, tax.of(margin.revenue));
                total = total.checked_add(margin_of(sale)).unwrap();
            }
            let summary = SalesSummary::of(&found, Currency::Brl).unwrap();
            prop_assert_eq!(summary.margin.unwrap().amount, total);
            Ok(())
        })?;
    }
}

#[test]
fn each_line_takes_a_share_of_the_order_by_value_and_its_own_cost() {
    block_on(async {
        let fx = Fixture::selling().await;
        let at = fx.later(1);
        let capa = ChannelOrderLine {
            unit_price: brl("29.90"),
            sale_fee: Some(brl("4.19")),
            ..sold("MLB9", None, 1)
        };
        fx.channel
            .sells(order("O1", at, vec![sold("MLB1", None, 1), capa]));
        fx.sync().await;
        let ads = [ad("MLB1", 10, 4, "11.98")];

        let period = fx.orders.sales(percent("6"), ever(), &ads).await.unwrap();

        let sale = sale_of(&period, "O1");
        let [fone, capa] = &sale.lines[..] else {
            panic!("{:?}", sale.lines);
        };
        assert_eq!((fone.item.as_str(), fone.units), ("MLB1", 1));
        assert_eq!(fone.revenue, brl("89.90"));
        assert_eq!(capa.revenue, brl("29.90"));
        // The fone's units left the stock at R$ 20,00; the capa's listing is
        // not among the Listings, so its units never did.
        assert_eq!(fone.cost, Some(brl("20.00")));
        assert_eq!(capa.cost, None);
        let total = |pick: fn(&mascate_commerce::LineMargin) -> Money| {
            Money::sum(Currency::Brl, sale.lines.iter().map(pick)).unwrap()
        };
        assert_eq!(total(|line| line.revenue), sale.margin.revenue);
        assert_eq!(total(|line| line.fees), sale.margin.fees_total());
        assert_eq!(total(|line| line.tax), sale.margin.tax);
        assert_eq!(fone.tax.rounded(), brl("5.39"));
    });
}
