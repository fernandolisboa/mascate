//! Units of a sale coming back, through the Inventory's public interface on
//! a real temporary database: they return where they left from, at the
//! cost they left with, and never more than left.

mod common;

use futures::executor::block_on;
use mascate_inventory::{HOME_LOCATION, InventoryError, MovementReason};
use mascate_kernel::RecordId;
use proptest::prelude::*;

use common::{FONE, Fixture, SALE, brl, entry, id};

#[test]
fn units_come_back_at_the_cost_they_left_with() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 2, "20"), entry(FONE, 2, "30")])
            .await
            .unwrap();
        let sold = f.sell(FONE, 3).await.unwrap();
        // Dearer units arrive after the sale: the Average Cost moves.
        f.enter(&[entry(FONE, 1, "40")]).await.unwrap();

        let back = f.put_back(sold.movement.id, 2).await.unwrap();

        assert_eq!(back.quantity, 2);
        assert_eq!(back.cost, brl("25"));
        assert_eq!(back.location, HOME_LOCATION);
        assert_eq!(back.product, id(FONE));
        assert_eq!(
            back.reason,
            MovementReason::SaleReturned { order: id(SALE) }
        );
        assert_eq!(f.units(FONE).await, 4);
        // 12,50 left on the shelf, 40 that arrived, 25 that came back.
        assert_eq!(f.average_cost(FONE).await, Some(brl("19.375")));
        let history = f.inventory.history(id(FONE)).await.unwrap();
        assert_eq!(history[0].movement, back);
    });
}

#[test]
fn every_unit_back_puts_the_ledger_where_it_was() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 3, "10")]).await.unwrap();
        let sold = f.sell(FONE, 3).await.unwrap();

        f.put_back(sold.movement.id, 3).await.unwrap();

        assert_eq!(f.units(FONE).await, 3);
        assert_eq!(f.inventory.stock().await.unwrap().value(), [brl("10")]);
    });
}

#[test]
fn no_more_units_come_back_than_left() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 3, "30")]).await.unwrap();
        let sold = f.sell(FONE, 2).await.unwrap();

        assert!(matches!(
            f.put_back(sold.movement.id, 3).await,
            Err(InventoryError::ReturnsMoreThanLeft { left: 2 })
        ));
        assert!(matches!(
            f.put_back(sold.movement.id, 0).await,
            Err(InventoryError::NoUnits)
        ));
        assert_eq!(f.units(FONE).await, 1);
    });
}

#[test]
fn only_units_that_left_come_back() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 3, "30")]).await.unwrap();
        let entered = f.inventory.history(id(FONE)).await.unwrap()[0].movement.id;
        let unknown = RecordId::from_u128(9);

        assert!(matches!(
            f.put_back(entered, 1).await,
            Err(InventoryError::UnknownExit(exit)) if exit == entered
        ));
        assert!(matches!(
            f.put_back(unknown, 1).await,
            Err(InventoryError::UnknownExit(exit)) if exit == unknown
        ));
        assert_eq!(f.units(FONE).await, 3);
    });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// Whatever came in and left, putting every unit back that left leaves
    /// the stock's value as if the sale never happened.
    #[test]
    fn putting_back_a_sale_undoes_its_value(
        lots in prop::collection::vec((1u32..20, 1u32..5000), 1..5),
        sold_share in 1u32..=100,
        split in 0u32..=100,
    ) {
        block_on(async {
            let f = Fixture::new().await;
            let entries: Vec<_> = lots
                .iter()
                .map(|(units, cents)| entry(FONE, *units, &format!("{}.{:02}", cents / 100, cents % 100)))
                .collect();
            f.enter(&entries).await.unwrap();
            let before = f.inventory.stock().await.unwrap().value();
            let on_hand: u32 = lots.iter().map(|(units, _)| units).sum();
            let sold = (on_hand * sold_share / 100).max(1);
            let exit = f.sell(FONE, sold).await.unwrap().movement.id;
            let first = (sold * split / 100).min(sold);

            if first > 0 {
                f.put_back(exit, first).await.unwrap();
            }
            if sold > first {
                f.put_back(exit, sold - first).await.unwrap();
            }

            let after = f.inventory.stock().await.unwrap().value();
            prop_assert_eq!(f.units(FONE).await, i64::from(on_hand));
            prop_assert_eq!(after[0].rounded(), before[0].rounded());
            Ok(())
        })?;
    }
}
