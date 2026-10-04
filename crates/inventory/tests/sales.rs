//! Units leaving the ledger for a sale, through the Inventory's public
//! interface on a real temporary database: they leave at the Average Cost,
//! never below zero, and say when they reach the Reorder Point.

mod common;

use futures::executor::block_on;
use mascate_inventory::{HOME_LOCATION, InventoryError, LowStock, MovementReason};
use proptest::prelude::*;

use common::{FONE, Fixture, SALE, brl, entry, id};

#[test]
fn a_sale_takes_units_out_at_the_average_cost() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 2, "20"), entry(FONE, 2, "30")])
            .await
            .unwrap();

        let sold = f.sell(FONE, 3).await.unwrap();

        assert_eq!(sold.movement.quantity, -3);
        assert_eq!(sold.movement.cost, brl("-37.5"));
        assert_eq!(sold.movement.location, HOME_LOCATION);
        assert_eq!(
            sold.movement.reason,
            MovementReason::Sale { order: id(SALE) }
        );
        assert_eq!(f.units(FONE).await, 1);
        // Leaving at the Average Cost leaves it where it was.
        assert_eq!(f.average_cost(FONE).await, Some(brl("12.5")));
        let history = f.inventory.history(id(FONE)).await.unwrap();
        assert_eq!(history[0].movement, sold.movement);
    });
}

#[test]
fn a_sale_of_more_than_is_on_hand_writes_nothing() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 2, "20")]).await.unwrap();

        assert!(matches!(
            f.sell(FONE, 3).await,
            Err(InventoryError::NotEnoughStock { on_hand: 2 })
        ));
        assert_eq!(f.units(FONE).await, 2);
    });
}

#[test]
fn a_product_never_in_stock_has_nothing_to_sell() {
    block_on(async {
        let f = Fixture::new().await;
        assert!(matches!(
            f.sell(FONE, 1).await,
            Err(InventoryError::NotEnoughStock { on_hand: 0 })
        ));
        assert!(matches!(
            f.sell(FONE, 0).await,
            Err(InventoryError::NoUnits)
        ));
    });
}

#[test]
fn a_sale_says_when_it_takes_the_product_to_its_reorder_point() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 6, "60")]).await.unwrap();
        f.inventory
            .set_reorder_point(id(FONE), Some(3))
            .await
            .unwrap();

        assert_eq!(f.sell(FONE, 2).await.unwrap().reached_reorder_point, None);
        assert_eq!(
            f.sell(FONE, 2).await.unwrap().reached_reorder_point,
            Some(LowStock {
                product: id(FONE),
                quantity: 2,
                reorder_point: 3,
            })
        );
        // Already below: nothing new to tell.
        assert_eq!(f.sell(FONE, 1).await.unwrap().reached_reorder_point, None);
    });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// However the sales come, the balance never drops below zero and what
    /// is left is worth what came in minus what left.
    #[test]
    fn sales_never_take_the_balance_below_zero(
        entered in 1u32..20,
        sales in prop::collection::vec(1u32..8, 1..8),
    ) {
        block_on(async {
            let f = Fixture::new().await;
            f.enter(&[entry(FONE, entered, "70")]).await.unwrap();
            let mut on_hand = i64::from(entered);
            let mut left = brl("70");
            for units in sales {
                match f.sell(FONE, units).await {
                    Ok(sold) => {
                        on_hand -= i64::from(units);
                        left = left.checked_add(sold.movement.cost).unwrap();
                    }
                    Err(InventoryError::NotEnoughStock { on_hand: there }) => {
                        prop_assert_eq!(there, on_hand);
                        prop_assert!(i64::from(units) > on_hand);
                    }
                    Err(error) => panic!("{error}"),
                }
                prop_assert!(on_hand >= 0);
                prop_assert_eq!(f.units(FONE).await, on_hand);
            }
            let stock = f.inventory.stock().await.unwrap();
            prop_assert_eq!(stock.products[0].valuation.value(), left);
            Ok(())
        })?;
    }
}

#[test]
fn a_sale_reads_back_with_the_cost_it_left_with() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 4, "20")]).await.unwrap();
        let first = f.sell(FONE, 1).await.unwrap();
        let second = f.sell(FONE, 2).await.unwrap();
        // Dearer units after the sales move the Average Cost, not them.
        f.enter(&[entry(FONE, 2, "50")]).await.unwrap();

        let found = f
            .inventory
            .movements(&[second.movement.id, id(SALE), first.movement.id])
            .await
            .unwrap();

        assert_eq!(found, [first.movement, second.movement]);
        assert_eq!(found[1].cost, brl("-10"));
        assert!(f.inventory.movements(&[]).await.unwrap().is_empty());
    });
}
