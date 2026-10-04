mod common;

use futures::executor::block_on;
use mascate_inventory::{
    HOME_LOCATION, InventoryError, LocationBalance, MovementReason, NewEntry, Valuation,
};
use mascate_kernel::{Currency, Money};
use rust_decimal::Decimal;

use common::{CAPA, FONE, Fixture, ORDER, brl, entry, id};

#[test]
fn the_owners_space_is_the_first_stock_location() {
    block_on(async {
        let f = Fixture::new().await;
        let locations = f.inventory.locations().await.unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].id, HOME_LOCATION);
        assert_eq!(locations[0].name, "Meu espaço");
    });
}

#[test]
fn nothing_is_in_stock_before_the_first_entry() {
    block_on(async {
        let f = Fixture::new().await;
        let stock = f.inventory.stock().await.unwrap();
        assert!(stock.products.is_empty());
        assert!(stock.value().is_empty());
        assert_eq!(stock.units(), 0);
        assert!(f.inventory.history(id(FONE)).await.unwrap().is_empty());
    });
}

#[test]
fn each_entry_moves_the_average_cost_by_weighted_average() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 10, "100.00")]).await.unwrap();
        f.enter(&[entry(FONE, 5, "65.00")]).await.unwrap();

        let stock = f.inventory.stock().await.unwrap();
        let fone = &stock.products[0];
        assert_eq!(fone.product, id(FONE));
        assert_eq!(fone.valuation.quantity(), 15);
        assert_eq!(fone.valuation.value(), brl("165.00"));
        assert_eq!(fone.valuation.average_cost(), Some(brl("11")));
        assert_eq!(
            fone.by_location,
            [LocationBalance {
                location: HOME_LOCATION,
                quantity: 15
            }]
        );
    });
}

#[test]
fn the_history_lists_movements_newest_first_with_the_stock_after_each() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 2, "20.00")]).await.unwrap();
        f.enter(&[entry(CAPA, 1, "3.00")]).await.unwrap();
        f.enter(&[entry(FONE, 2, "30.00")]).await.unwrap();

        let history = f.inventory.history(id(FONE)).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].movement.quantity, 2);
        assert_eq!(history[0].movement.cost, brl("30.00"));
        assert_eq!(history[0].after.quantity(), 4);
        assert_eq!(history[0].after.average_cost(), Some(brl("12.5")));
        assert_eq!(history[1].after.quantity(), 2);
        assert_eq!(history[1].after.average_cost(), Some(brl("10")));
        assert!(history[0].movement.at > history[1].movement.at);
        assert_eq!(
            history[1].movement.reason,
            MovementReason::PurchaseReceipt {
                purchase_order: id(ORDER)
            }
        );
    });
}

#[test]
fn the_value_in_stock_adds_every_product_per_currency() {
    block_on(async {
        let f = Fixture::new().await;
        let usd = NewEntry {
            cost: Money::new(Decimal::new(700, 2), Currency::Usd),
            ..entry(CAPA, 1, "0")
        };
        f.enter(&[entry(FONE, 3, "45.00"), usd]).await.unwrap();
        f.enter(&[entry(3, 1, "4.99")]).await.unwrap();

        let stock = f.inventory.stock().await.unwrap();
        assert_eq!(stock.units(), 5);
        assert_eq!(
            stock.value(),
            [
                brl("49.99"),
                Money::new(Decimal::new(700, 2), Currency::Usd)
            ]
        );
    });
}

#[test]
fn refuses_an_entry_without_units_with_a_negative_cost_or_in_an_unknown_place() {
    block_on(async {
        let f = Fixture::new().await;
        assert!(matches!(
            f.enter(&[entry(FONE, 0, "1")]).await,
            Err(InventoryError::NoUnits)
        ));
        assert!(matches!(
            f.enter(&[entry(FONE, 1, "-1")]).await,
            Err(InventoryError::NegativeCost)
        ));
        let elsewhere = NewEntry {
            location: id(999),
            ..entry(FONE, 1, "1")
        };
        assert!(matches!(
            f.enter(&[elsewhere]).await,
            Err(InventoryError::UnknownLocation(_))
        ));
        assert!(f.inventory.stock().await.unwrap().products.is_empty());
    });
}

#[test]
fn a_product_stays_valued_in_the_currency_of_its_first_entry() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 1, "10")]).await.unwrap();
        let in_dollars = NewEntry {
            cost: Money::new(Decimal::ONE, Currency::Usd),
            ..entry(FONE, 1, "0")
        };
        assert!(matches!(
            f.enter(std::slice::from_ref(&in_dollars)).await,
            Err(InventoryError::Currencies(_))
        ));
        // Within one batch too.
        assert!(matches!(
            f.enter(&[
                entry(CAPA, 1, "1"),
                NewEntry {
                    product: id(CAPA),
                    ..in_dollars
                }
            ])
            .await,
            Err(InventoryError::Currencies(_))
        ));
        assert_eq!(f.inventory.stock().await.unwrap().units(), 1);
    });
}

#[test]
fn a_refused_batch_writes_nothing() {
    block_on(async {
        let f = Fixture::new().await;
        assert!(
            f.enter(&[entry(FONE, 2, "20"), entry(CAPA, 0, "1")])
                .await
                .is_err()
        );
        assert!(f.inventory.stock().await.unwrap().products.is_empty());
    });
}

#[test]
fn entries_leave_with_the_callers_transaction_when_it_rolls_back() {
    block_on(async {
        let f = Fixture::new().await;
        let connection = f.database.connect_for_transaction().await.unwrap();
        let transaction = connection.transaction().await.unwrap();
        f.inventory
            .record_entries(&transaction, &[entry(FONE, 2, "20")])
            .await
            .unwrap();
        transaction.rollback().await.unwrap();
        assert!(f.inventory.stock().await.unwrap().products.is_empty());
    });
}

#[test]
fn the_ledger_refuses_edits_and_deletes_even_from_outside_the_module() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 2, "20")]).await.unwrap();
        let connection = f.database.connection();
        assert!(
            connection
                .execute("UPDATE inventory_stock_movements SET quantity = 99", ())
                .await
                .is_err()
        );
        assert!(
            connection
                .execute("DELETE FROM inventory_stock_movements", ())
                .await
                .is_err()
        );
        assert_eq!(f.inventory.stock().await.unwrap().units(), 2);
    });
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    fn entries() -> impl Strategy<Value = Vec<(u128, u32, i64)>> {
        // Product, units and total cost in cents.
        proptest::collection::vec((1u128..4, 1u32..50, 0i64..1_000_000), 1..12)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn the_balance_is_always_the_sum_of_the_movements(batches in proptest::collection::vec(entries(), 1..4)) {
            block_on(async {
                let f = Fixture::new().await;
                for batch in &batches {
                    let entries: Vec<NewEntry> = batch
                        .iter()
                        .map(|&(product, quantity, cents)| NewEntry {
                            cost: Money::new(Decimal::new(cents, 2), Currency::Brl),
                            ..entry(product, quantity, "0")
                        })
                        .collect();
                    f.enter(&entries).await.unwrap();
                }
                let stock = f.inventory.stock().await.unwrap();
                for product in &stock.products {
                    let history = f.inventory.history(product.product).await.unwrap();
                    let moved: i64 = history.iter().map(|line| line.movement.quantity).sum();
                    let cost = Money::sum(
                        Currency::Brl,
                        history.iter().map(|line| line.movement.cost),
                    )
                    .unwrap();
                    assert_eq!(product.valuation.quantity(), moved);
                    assert_eq!(product.valuation.value(), cost);
                    let by_location: i64 =
                        product.by_location.iter().map(|balance| balance.quantity).sum();
                    assert_eq!(by_location, moved);
                    assert_eq!(history[0].after, product.valuation);
                }
                let all: i64 = batches.iter().flatten().map(|&(_, q, _)| i64::from(q)).sum();
                assert_eq!(stock.units(), all);
            });
        }

        #[test]
        fn the_average_cost_lies_between_the_cheapest_and_dearest_unit_cost(
            entries in proptest::collection::vec((1u32..100, 0i64..10_000_000), 1..20)
        ) {
            let mut valuation = Valuation::empty(Currency::Brl);
            let mut unit_costs = Vec::new();
            for &(quantity, cents) in &entries {
                let cost = Money::new(Decimal::new(cents, 2), Currency::Brl);
                valuation = valuation.enter(quantity, cost).unwrap();
                unit_costs.push(cost.amount() / Decimal::from(quantity));
            }
            let average = valuation.average_cost().unwrap().amount();
            let cheapest = unit_costs.iter().min().unwrap();
            let dearest = unit_costs.iter().max().unwrap();
            // Division keeps 28 digits; allow its last one.
            let slack = Decimal::new(1, 20);
            prop_assert!(average >= cheapest - slack && average <= dearest + slack);
            let units: i64 = entries.iter().map(|&(q, _)| i64::from(q)).sum();
            prop_assert_eq!(valuation.quantity(), units);
            let value: i64 = entries.iter().map(|&(_, c)| c).sum();
            prop_assert_eq!(valuation.value().amount(), Decimal::new(value, 2));
        }
    }
}
