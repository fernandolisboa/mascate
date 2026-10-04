mod common;

use futures::executor::block_on;
use mascate_inventory::{LowStock, StockAdjustment};

use common::{CAPA, FONE, Fixture, entry, id};

fn low(product: u128, quantity: i64, reorder_point: u32) -> LowStock {
    LowStock {
        product: id(product),
        quantity,
        reorder_point,
    }
}

#[test]
fn nothing_is_low_without_a_reorder_point() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 1, "10")]).await.unwrap();
        f.adjust(FONE, StockAdjustment::Loss(1)).await.unwrap();
        assert!(f.inventory.low_stock().await.unwrap().is_empty());
        let stock = f.inventory.stock().await.unwrap();
        assert_eq!(stock.products[0].reorder_point, None);
        assert!(!stock.products[0].is_low());
    });
}

#[test]
fn a_product_is_low_at_or_below_its_reorder_point() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 10, "100")]).await.unwrap();
        f.inventory
            .set_reorder_point(id(FONE), Some(5))
            .await
            .unwrap();
        assert!(f.inventory.low_stock().await.unwrap().is_empty());

        f.adjust(FONE, StockAdjustment::Loss(5)).await.unwrap();
        assert_eq!(f.inventory.low_stock().await.unwrap(), [low(FONE, 5, 5)]);
        let stock = f.inventory.stock().await.unwrap();
        assert_eq!(stock.products[0].reorder_point, Some(5));
        assert!(stock.products[0].is_low());

        // A new entry lifts it back above the point.
        f.enter(&[entry(FONE, 1, "10")]).await.unwrap();
        assert!(f.inventory.low_stock().await.unwrap().is_empty());
    });
}

#[test]
fn a_product_that_never_moved_is_low_with_none_on_hand() {
    block_on(async {
        let f = Fixture::new().await;
        f.inventory
            .set_reorder_point(id(CAPA), Some(0))
            .await
            .unwrap();
        assert_eq!(f.inventory.low_stock().await.unwrap(), [low(CAPA, 0, 0)]);
    });
}

#[test]
fn the_reorder_point_can_change_and_be_cleared() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 4, "40")]).await.unwrap();
        f.inventory
            .set_reorder_point(id(FONE), Some(5))
            .await
            .unwrap();
        assert_eq!(f.inventory.low_stock().await.unwrap(), [low(FONE, 4, 5)]);

        f.inventory
            .set_reorder_point(id(FONE), Some(2))
            .await
            .unwrap();
        assert!(f.inventory.low_stock().await.unwrap().is_empty());

        f.inventory.set_reorder_point(id(FONE), None).await.unwrap();
        f.adjust(FONE, StockAdjustment::Loss(4)).await.unwrap();
        assert!(f.inventory.low_stock().await.unwrap().is_empty());
        assert_eq!(
            f.inventory.stock().await.unwrap().products[0].reorder_point,
            None
        );

        // Cleared, then set again.
        f.inventory
            .set_reorder_point(id(FONE), Some(1))
            .await
            .unwrap();
        assert_eq!(f.inventory.low_stock().await.unwrap(), [low(FONE, 0, 1)]);
    });
}

#[test]
fn the_furthest_below_its_point_comes_first() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 4, "40"), entry(CAPA, 1, "5")])
            .await
            .unwrap();
        f.inventory
            .set_reorder_point(id(FONE), Some(5))
            .await
            .unwrap();
        f.inventory
            .set_reorder_point(id(CAPA), Some(3))
            .await
            .unwrap();
        assert_eq!(
            f.inventory.low_stock().await.unwrap(),
            [low(CAPA, 1, 3), low(FONE, 4, 5)]
        );
    });
}

#[test]
fn an_adjustment_says_when_it_takes_the_product_to_its_reorder_point() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 10, "100")]).await.unwrap();
        f.inventory
            .set_reorder_point(id(FONE), Some(5))
            .await
            .unwrap();

        let above = f.adjust(FONE, StockAdjustment::Loss(4)).await.unwrap();
        assert_eq!(above.reached_reorder_point, None);

        let reached = f.adjust(FONE, StockAdjustment::Damage(1)).await.unwrap();
        assert_eq!(reached.reached_reorder_point, Some(low(FONE, 5, 5)));

        // Already there: the alert stands, nothing new to tell.
        let further = f.adjust(FONE, StockAdjustment::Loss(1)).await.unwrap();
        assert_eq!(further.reached_reorder_point, None);

        f.enter(&[entry(FONE, 10, "100")]).await.unwrap();
        let counted = f.adjust(FONE, StockAdjustment::Count(3)).await.unwrap();
        assert_eq!(counted.reached_reorder_point, Some(low(FONE, 3, 5)));

        // A count that finds units never reaches the point from above.
        let found = f.adjust(FONE, StockAdjustment::Count(4)).await.unwrap();
        assert_eq!(found.reached_reorder_point, None);
    });
}
