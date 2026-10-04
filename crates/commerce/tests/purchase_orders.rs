use std::str::FromStr;
use std::sync::Arc;

use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    NewPurchaseLine, NewPurchaseOrder, PurchaseOrder, PurchaseOrderError, PurchaseOrderStatus,
    PurchaseOrders, Receiving,
};
use mascate_inventory::{HOME_LOCATION, Inventory, InventoryError, MovementReason};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, RecordId};
use mascate_platform::{Database, migrate};
use rust_decimal::Decimal;

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    inventory: Arc<Inventory>,
    orders: PurchaseOrders,
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
        let orders = PurchaseOrders::new(database, inventory.clone(), clock.clone(), ids);
        Self {
            _dir: dir,
            clock,
            inventory,
            orders,
        }
    }

    fn later(&self) {
        self.clock.advance(TimeDelta::hours(1));
    }

    async fn receive(
        &self,
        order: &PurchaseOrder,
        units: &[(usize, u32)],
    ) -> Result<PurchaseOrder, PurchaseOrderError> {
        self.later();
        let receiving: Vec<Receiving> = units
            .iter()
            .map(|&(line, quantity)| Receiving {
                line: order.lines[line].id,
                quantity,
            })
            .collect();
        self.orders
            .receive(order.id, HOME_LOCATION, &receiving)
            .await
    }

    async fn units_in_stock(&self, product: RecordId) -> i64 {
        self.inventory
            .stock()
            .await
            .unwrap()
            .products
            .iter()
            .find(|stock| stock.product == product)
            .map_or(0, |stock| stock.valuation.quantity())
    }
}

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

/// Catalog ids, as the owner picks them on screen.
fn catalog_id(n: u128) -> RecordId {
    RecordId::from_u128(1_000_000 + n)
}

fn supplier() -> RecordId {
    catalog_id(1)
}

fn fone() -> RecordId {
    catalog_id(10)
}

fn capa() -> RecordId {
    catalog_id(11)
}

fn line(product: RecordId, quantity: u32, unit_price: &str) -> NewPurchaseLine {
    NewPurchaseLine {
        product,
        quantity,
        unit_price: brl(unit_price),
    }
}

/// 10 fones at R$ 20 and 20 capas at R$ 5, with R$ 30 of freight: R$ 200 and
/// R$ 100 of goods, so the fones carry R$ 20 of freight and the capas R$ 10.
fn order() -> NewPurchaseOrder {
    NewPurchaseOrder {
        supplier: supplier(),
        ordered_on: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
        freight: brl("30.00"),
        lines: vec![line(fone(), 10, "20.00"), line(capa(), 20, "5.00")],
    }
}

#[test]
fn a_new_order_is_purchased_with_its_freight_split_by_value() {
    block_on(async {
        let f = Fixture::new().await;
        let created = f.orders.create(order()).await.unwrap();

        assert_eq!(created.status(), PurchaseOrderStatus::Purchased);
        assert_eq!(created.supplier, supplier());
        assert_eq!(
            created.ordered_on,
            NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()
        );
        assert_eq!(created.total(), brl("330.00"));
        assert_eq!(created.units(), 30);
        assert_eq!(created.received_units(), 0);
        let freight: Vec<Money> = created.lines.iter().map(|line| line.freight).collect();
        assert_eq!(freight, [brl("20.00"), brl("10.00")]);
        assert_eq!(created.lines[0].landed_unit_cost(), brl("22"));
        assert_eq!(created.lines[1].landed_unit_cost(), brl("5.5"));
        assert_eq!(f.orders.purchase_order(created.id).await.unwrap(), created);
    });
}

#[test]
fn orders_list_the_most_recently_ordered_first() {
    block_on(async {
        let f = Fixture::new().await;
        let older = f.orders.create(order()).await.unwrap();
        let newer = f
            .orders
            .create(NewPurchaseOrder {
                ordered_on: NaiveDate::from_ymd_opt(2026, 10, 3).unwrap(),
                ..order()
            })
            .await
            .unwrap();
        let listed: Vec<RecordId> = f
            .orders
            .purchase_orders()
            .await
            .unwrap()
            .iter()
            .map(|order| order.id)
            .collect();
        assert_eq!(listed, [newer.id, older.id]);
    });
}

#[test]
fn refuses_an_order_without_items_units_or_one_currency_or_with_negative_amounts() {
    block_on(async {
        let f = Fixture::new().await;
        let refused = |order: NewPurchaseOrder| {
            let orders = &f.orders;
            async move { orders.create(order).await.unwrap_err() }
        };
        assert!(matches!(
            refused(NewPurchaseOrder {
                lines: Vec::new(),
                ..order()
            })
            .await,
            PurchaseOrderError::NoLines
        ));
        assert!(matches!(
            refused(NewPurchaseOrder {
                lines: vec![line(fone(), 0, "1")],
                ..order()
            })
            .await,
            PurchaseOrderError::NoUnits
        ));
        assert!(matches!(
            refused(NewPurchaseOrder {
                freight: brl("-1"),
                ..order()
            })
            .await,
            PurchaseOrderError::Negative
        ));
        assert!(matches!(
            refused(NewPurchaseOrder {
                lines: vec![line(fone(), 1, "-0.01")],
                ..order()
            })
            .await,
            PurchaseOrderError::Negative
        ));
        assert!(matches!(
            refused(NewPurchaseOrder {
                freight: Money::zero(Currency::Usd),
                ..order()
            })
            .await,
            PurchaseOrderError::Currencies(_)
        ));
        assert!(f.orders.purchase_orders().await.unwrap().is_empty());
    });
}

#[test]
fn an_order_follows_shipped_partly_received_and_received() {
    block_on(async {
        let f = Fixture::new().await;
        let created = f.orders.create(order()).await.unwrap();

        let shipped = f.orders.mark_shipped(created.id).await.unwrap();
        assert_eq!(shipped.status(), PurchaseOrderStatus::Shipped);
        assert!(shipped.shipped_at.is_some());
        assert!(matches!(
            f.orders.mark_shipped(created.id).await,
            Err(PurchaseOrderError::NotAwaitingShipment)
        ));

        let partly = f.receive(&shipped, &[(0, 4)]).await.unwrap();
        assert_eq!(partly.status(), PurchaseOrderStatus::PartlyReceived);
        assert_eq!(partly.lines[0].received, 4);
        assert_eq!(partly.lines[0].remaining(), 6);
        assert_eq!(partly.received_units(), 4);

        let received = f.receive(&partly, &[(0, 6), (1, 20)]).await.unwrap();
        assert_eq!(received.status(), PurchaseOrderStatus::Received);
        assert_eq!(received.receipts.len(), 2);
        assert_eq!(received.received_at(), Some(received.receipts[1].at));
        assert_eq!(received.receipts[1].lines.len(), 2);
    });
}

#[test]
fn receiving_brings_units_into_stock_at_price_plus_their_freight() {
    block_on(async {
        let f = Fixture::new().await;
        let created = f.orders.create(order()).await.unwrap();
        let partly = f.receive(&created, &[(0, 4)]).await.unwrap();

        assert_eq!(f.units_in_stock(fone()).await, 4);
        let history = f.inventory.history(fone()).await.unwrap();
        assert_eq!(history.len(), 1);
        let entry = &history[0].movement;
        assert_eq!(entry.location, HOME_LOCATION);
        assert_eq!(entry.quantity, 4);
        // 4 × R$ 20 plus 4/10 of the line's R$ 20 of freight.
        assert_eq!(entry.cost, brl("88.00"));
        assert_eq!(history[0].after.average_cost(), Some(brl("22")));
        assert_eq!(
            entry.reason,
            MovementReason::PurchaseReceipt {
                purchase_order: created.id
            }
        );
        assert_eq!(partly.receipts[0].lines[0].cost, brl("88.00"));
    });
}

#[test]
fn a_fully_received_order_puts_its_whole_cost_into_stock() {
    block_on(async {
        let f = Fixture::new().await;
        let created = f
            .orders
            .create(NewPurchaseOrder {
                freight: brl("10.00"),
                lines: vec![line(fone(), 3, "7.00"), line(capa(), 3, "7.00")],
                ..order()
            })
            .await
            .unwrap();
        let mut current = created.clone();
        for _ in 0..3 {
            current = f.receive(&current, &[(0, 1), (1, 1)]).await.unwrap();
        }
        assert_eq!(current.status(), PurchaseOrderStatus::Received);
        let value = Money::sum(Currency::Brl, f.inventory.stock().await.unwrap().value()).unwrap();
        assert_eq!(value, created.total());
    });
}

#[test]
fn the_average_cost_blends_units_from_two_orders() {
    block_on(async {
        let f = Fixture::new().await;
        let first = f.orders.create(order()).await.unwrap();
        f.receive(&first, &[(0, 10)]).await.unwrap();
        let second = f
            .orders
            .create(NewPurchaseOrder {
                freight: brl("0"),
                lines: vec![line(fone(), 5, "28.00")],
                ..order()
            })
            .await
            .unwrap();
        f.receive(&second, &[(0, 5)]).await.unwrap();

        let stock = f.inventory.stock().await.unwrap();
        let fone_stock = stock.products.iter().find(|s| s.product == fone()).unwrap();
        // (10 × R$ 22 + 5 × R$ 28) / 15.
        assert_eq!(fone_stock.valuation.quantity(), 15);
        assert_eq!(fone_stock.valuation.value(), brl("360.00"));
        assert_eq!(fone_stock.valuation.average_cost(), Some(brl("24")));
    });
}

#[test]
fn refuses_more_units_than_remain_and_writes_nothing() {
    block_on(async {
        let f = Fixture::new().await;
        let created = f.orders.create(order()).await.unwrap();
        let error = f.receive(&created, &[(1, 5), (0, 11)]).await.unwrap_err();
        assert!(matches!(
            error,
            PurchaseOrderError::TooMany { remaining: 10, .. }
        ));
        // The same line twice counts as one.
        assert!(matches!(
            f.receive(&created, &[(0, 6), (0, 6)]).await,
            Err(PurchaseOrderError::TooMany { .. })
        ));
        assert_eq!(f.units_in_stock(capa()).await, 0);
        let unchanged = f.orders.purchase_order(created.id).await.unwrap();
        assert!(unchanged.receipts.is_empty());
    });
}

#[test]
fn refuses_a_receipt_with_no_units_or_a_line_of_another_order() {
    block_on(async {
        let f = Fixture::new().await;
        let created = f.orders.create(order()).await.unwrap();
        let other = f.orders.create(order()).await.unwrap();
        assert!(matches!(
            f.receive(&created, &[(0, 0)]).await,
            Err(PurchaseOrderError::NothingToReceive)
        ));
        assert!(matches!(
            f.orders
                .receive(
                    created.id,
                    HOME_LOCATION,
                    &[Receiving {
                        line: other.lines[0].id,
                        quantity: 1
                    }]
                )
                .await,
            Err(PurchaseOrderError::UnknownLine(_))
        ));
    });
}

#[test]
fn a_receipt_the_inventory_refuses_leaves_the_order_as_it_was() {
    block_on(async {
        let f = Fixture::new().await;
        let created = f.orders.create(order()).await.unwrap();
        let elsewhere = catalog_id(999);
        let error = f
            .orders
            .receive(
                created.id,
                elsewhere,
                &[Receiving {
                    line: created.lines[0].id,
                    quantity: 1,
                }],
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            PurchaseOrderError::Stock(InventoryError::UnknownLocation(_))
        ));
        let unchanged = f.orders.purchase_order(created.id).await.unwrap();
        assert_eq!(unchanged, created);
    });
}

#[test]
fn an_order_with_nothing_received_can_be_edited_cancelled_or_deleted() {
    block_on(async {
        let f = Fixture::new().await;
        let created = f.orders.create(order()).await.unwrap();

        f.later();
        let edited = f
            .orders
            .update(
                created.id,
                NewPurchaseOrder {
                    freight: brl("12.00"),
                    lines: vec![line(capa(), 6, "4.50")],
                    ..order()
                },
            )
            .await
            .unwrap();
        assert_eq!(edited.id, created.id);
        assert_eq!(edited.lines.len(), 1);
        assert_eq!(edited.lines[0].product, capa());
        assert_eq!(edited.lines[0].freight, brl("12.00"));
        assert_eq!(edited.total(), brl("39.00"));

        let cancelled = f.orders.cancel(created.id).await.unwrap();
        assert_eq!(cancelled.status(), PurchaseOrderStatus::Cancelled);
        assert!(matches!(
            f.receive(&cancelled, &[(0, 1)]).await,
            Err(PurchaseOrderError::Cancelled)
        ));
        assert!(matches!(
            f.orders.update(created.id, order()).await,
            Err(PurchaseOrderError::Cancelled)
        ));
        assert!(matches!(
            f.orders.mark_shipped(created.id).await,
            Err(PurchaseOrderError::Cancelled)
        ));

        f.orders.delete(created.id).await.unwrap();
        assert!(f.orders.purchase_orders().await.unwrap().is_empty());
        assert!(matches!(
            f.orders.purchase_order(created.id).await,
            Err(PurchaseOrderError::UnknownPurchaseOrder(_))
        ));
    });
}

#[test]
fn an_order_whose_units_came_in_can_no_longer_change() {
    block_on(async {
        let f = Fixture::new().await;
        let created = f.orders.create(order()).await.unwrap();
        f.receive(&created, &[(1, 1)]).await.unwrap();

        assert!(matches!(
            f.orders.update(created.id, order()).await,
            Err(PurchaseOrderError::AlreadyReceiving)
        ));
        assert!(matches!(
            f.orders.cancel(created.id).await,
            Err(PurchaseOrderError::AlreadyReceiving)
        ));
        assert!(matches!(
            f.orders.delete(created.id).await,
            Err(PurchaseOrderError::AlreadyReceiving)
        ));
        assert!(matches!(
            f.orders.mark_shipped(created.id).await,
            Err(PurchaseOrderError::NotAwaitingShipment)
        ));
        assert_eq!(f.units_in_stock(capa()).await, 1);
    });
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        /// However the units arrive, the stock ends with every unit ordered
        /// and exactly what the order cost, freight included, and each
        /// Product's balance is the sum of its movements.
        #[test]
        fn receiving_in_any_steps_brings_in_every_unit_and_the_whole_cost(
            freight in 0i64..100_000,
            lines in proptest::collection::vec((1u32..12, 0i64..50_000), 1..5),
            steps in proptest::collection::vec(proptest::collection::vec(0u32..6, 5), 1..6),
        ) {
            block_on(async {
                let f = Fixture::new().await;
                let new_lines: Vec<NewPurchaseLine> = lines
                    .iter()
                    .enumerate()
                    .map(|(index, &(quantity, cents))| NewPurchaseLine {
                        product: catalog_id(100 + index as u128),
                        quantity,
                        unit_price: Money::new(Decimal::new(cents, 2), Currency::Brl),
                    })
                    .collect();
                let created = f
                    .orders
                    .create(NewPurchaseOrder {
                        freight: Money::new(Decimal::new(freight, 2), Currency::Brl),
                        lines: new_lines,
                        ..order()
                    })
                    .await
                    .unwrap();
                let mut current = created.clone();
                for step in steps.iter().chain([&vec![u32::MAX; 5]]) {
                    let units: Vec<(usize, u32)> = current
                        .lines
                        .iter()
                        .enumerate()
                        .map(|(index, line)| (index, step[index].min(line.remaining())))
                        .collect();
                    if units.iter().all(|&(_, quantity)| quantity == 0) {
                        continue;
                    }
                    current = f.receive(&current, &units).await.unwrap();
                }
                assert_eq!(current.status(), PurchaseOrderStatus::Received);

                let stock = f.inventory.stock().await.unwrap();
                let value = Money::sum(Currency::Brl, stock.value()).unwrap();
                assert_eq!(value, created.total());
                for line in &created.lines {
                    let history = f.inventory.history(line.product).await.unwrap();
                    let moved: i64 = history.iter().map(|h| h.movement.quantity).sum();
                    assert_eq!(moved, i64::from(line.quantity));
                    assert_eq!(f.units_in_stock(line.product).await, moved);
                    let cost = Money::sum(Currency::Brl, history.iter().map(|h| h.movement.cost))
                        .unwrap();
                    assert_eq!(cost, line.value().checked_add(line.freight).unwrap());
                }
            });
        }
    }
}
