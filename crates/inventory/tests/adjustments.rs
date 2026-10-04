mod common;

use futures::executor::block_on;
use mascate_inventory::{
    AdjustmentKind, HOME_LOCATION, InventoryError, MovementReason, StockAdjustment,
};
use rust_decimal::Decimal;

use common::{CAPA, FONE, Fixture, brl, entry, id};

#[test]
fn a_loss_takes_units_out_at_the_average_cost_and_leaves_it_unchanged() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 10, "100.00")]).await.unwrap();

        let adjusted = f.adjust(FONE, StockAdjustment::Loss(3)).await.unwrap();

        let movement = adjusted.movement.unwrap();
        assert_eq!(movement.quantity, -3);
        assert_eq!(movement.cost, brl("-30.00"));
        assert_eq!(
            movement.reason,
            MovementReason::Adjustment(AdjustmentKind::Loss)
        );
        assert_eq!(f.units(FONE).await, 7);
        assert_eq!(f.average_cost(FONE).await, Some(brl("10")));
        let stock = f.inventory.stock().await.unwrap();
        assert_eq!(stock.products[0].valuation.value(), brl("70.00"));
    });
}

#[test]
fn damage_keeps_the_owners_note_with_the_movement() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 5, "50.00")]).await.unwrap();
        f.inventory
            .adjust(
                id(FONE),
                HOME_LOCATION,
                StockAdjustment::Damage(1),
                Some("  caixa molhada  "),
            )
            .await
            .unwrap();
        f.inventory
            .adjust(
                id(FONE),
                HOME_LOCATION,
                StockAdjustment::Damage(1),
                Some(" "),
            )
            .await
            .unwrap();

        let history = f.inventory.history(id(FONE)).await.unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[1].movement.note.as_deref(), Some("caixa molhada"));
        assert_eq!(
            history[1].movement.reason,
            MovementReason::Adjustment(AdjustmentKind::Damage)
        );
        assert_eq!(history[0].movement.note, None);
        assert_eq!(history[0].after.quantity(), 3);
        assert_eq!(history[0].after.average_cost(), Some(brl("10")));
    });
}

#[test]
fn a_count_records_the_difference_from_the_ledger_either_way() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 10, "120.00")]).await.unwrap();

        let fewer = f.adjust(FONE, StockAdjustment::Count(8)).await.unwrap();
        let fewer = fewer.movement.unwrap();
        assert_eq!(fewer.quantity, -2);
        assert_eq!(fewer.cost, brl("-24"));
        assert_eq!(
            fewer.reason,
            MovementReason::Adjustment(AdjustmentKind::Count)
        );
        assert_eq!(f.units(FONE).await, 8);

        let more = f.adjust(FONE, StockAdjustment::Count(12)).await.unwrap();
        let more = more.movement.unwrap();
        assert_eq!(more.quantity, 4);
        assert_eq!(more.cost, brl("48"));
        assert_eq!(f.units(FONE).await, 12);
        assert_eq!(f.average_cost(FONE).await, Some(brl("12")));
    });
}

#[test]
fn a_count_that_matches_the_ledger_records_nothing() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 4, "40")]).await.unwrap();
        let adjusted = f.adjust(FONE, StockAdjustment::Count(4)).await.unwrap();
        assert_eq!(adjusted.movement, None);
        assert_eq!(adjusted.reached_reorder_point, None);
        assert_eq!(f.inventory.history(id(FONE)).await.unwrap().len(), 1);
    });
}

#[test]
fn refuses_to_take_out_more_than_is_on_hand_and_writes_nothing() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 3, "30")]).await.unwrap();
        assert!(matches!(
            f.adjust(FONE, StockAdjustment::Loss(4)).await,
            Err(InventoryError::NotEnoughStock { on_hand: 3 })
        ));
        assert!(matches!(
            f.adjust(CAPA, StockAdjustment::Damage(1)).await,
            Err(InventoryError::NotEnoughStock { on_hand: 0 })
        ));
        assert_eq!(f.units(FONE).await, 3);
        assert_eq!(f.inventory.history(id(FONE)).await.unwrap().len(), 1);
    });
}

#[test]
fn refuses_an_adjustment_of_no_units_or_in_an_unknown_place() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 3, "30")]).await.unwrap();
        assert!(matches!(
            f.adjust(FONE, StockAdjustment::Loss(0)).await,
            Err(InventoryError::NoUnits)
        ));
        assert!(matches!(
            f.adjust(FONE, StockAdjustment::Damage(0)).await,
            Err(InventoryError::NoUnits)
        ));
        assert!(matches!(
            f.inventory
                .adjust(id(FONE), id(999), StockAdjustment::Count(1), None)
                .await,
            Err(InventoryError::UnknownLocation(_))
        ));
        assert_eq!(f.inventory.history(id(FONE)).await.unwrap().len(), 1);
    });
}

#[test]
fn units_found_after_the_stock_ran_out_come_back_at_the_last_average_cost() {
    block_on(async {
        let f = Fixture::new().await;
        f.enter(&[entry(FONE, 4, "48")]).await.unwrap();
        f.adjust(FONE, StockAdjustment::Loss(4)).await.unwrap();
        assert_eq!(f.units(FONE).await, 0);
        assert_eq!(f.average_cost(FONE).await, None);
        let stock = f.inventory.stock().await.unwrap();
        assert!(stock.value()[0].amount().is_zero());

        f.adjust(FONE, StockAdjustment::Count(2)).await.unwrap();
        assert_eq!(f.units(FONE).await, 2);
        assert_eq!(f.average_cost(FONE).await, Some(brl("12")));
    });
}

#[test]
fn units_of_a_product_that_never_came_in_cannot_be_found_by_a_count() {
    block_on(async {
        let f = Fixture::new().await;
        assert!(matches!(
            f.adjust(FONE, StockAdjustment::Count(2)).await,
            Err(InventoryError::NoCostBasis)
        ));
        let none = f.adjust(FONE, StockAdjustment::Count(0)).await.unwrap();
        assert_eq!(none.movement, None);
        assert!(f.inventory.history(id(FONE)).await.unwrap().is_empty());
    });
}

#[test]
fn the_last_units_out_take_all_the_value_left() {
    block_on(async {
        let f = Fixture::new().await;
        // 100 / 3 never divides evenly.
        f.enter(&[entry(FONE, 3, "100")]).await.unwrap();
        f.adjust(FONE, StockAdjustment::Loss(1)).await.unwrap();
        f.adjust(FONE, StockAdjustment::Damage(2)).await.unwrap();
        let history = f.inventory.history(id(FONE)).await.unwrap();
        assert_eq!(history[0].after.quantity(), 0);
        assert!(history[0].after.value().amount().is_zero());
        let out: Decimal = history[..2]
            .iter()
            .map(|line| line.movement.cost.amount())
            .sum();
        assert_eq!(out, Decimal::from(-100));
    });
}

mod properties {
    use super::*;
    use mascate_inventory::Valuation;
    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum Step {
        Enter(u32, i64),
        Lose(u32),
        Damage(u32),
        Count(u32),
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            (1u32..30, 0i64..1_000_000).prop_map(|(units, cents)| Step::Enter(units, cents)),
            (0u32..40).prop_map(Step::Lose),
            (0u32..40).prop_map(Step::Damage),
            (0u32..60).prop_map(Step::Count),
        ]
    }

    async fn valuation(f: &Fixture) -> Option<Valuation> {
        f.inventory
            .history(id(FONE))
            .await
            .unwrap()
            .first()
            .map(|line| line.after)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        /// Only an explicit adjustment moves the balance down, and never
        /// below zero; a refused one changes nothing.
        #[test]
        fn the_balance_never_goes_negative_and_adjustments_keep_the_average_cost(
            steps in proptest::collection::vec(step(), 1..25)
        ) {
            block_on(async {
                let f = Fixture::new().await;
                // Division keeps 28 digits; allow its last ones.
                let slack = Decimal::new(1, 18);
                for step in &steps {
                    let before = valuation(&f).await;
                    let units_before = before.map_or(0, |v| v.quantity());
                    let outcome = match *step {
                        Step::Enter(units, cents) => {
                            let cost = Decimal::new(cents, 2).to_string();
                            f.enter(&[entry(FONE, units, &cost)]).await
                        }
                        Step::Lose(units) => {
                            f.adjust(FONE, StockAdjustment::Loss(units)).await.map(|_| ())
                        }
                        Step::Damage(units) => {
                            f.adjust(FONE, StockAdjustment::Damage(units)).await.map(|_| ())
                        }
                        Step::Count(counted) => {
                            f.adjust(FONE, StockAdjustment::Count(counted)).await.map(|_| ())
                        }
                    };
                    let after = valuation(&f).await;
                    let units_after = after.map_or(0, |v| v.quantity());
                    assert!(units_after >= 0, "{steps:?}");
                    if let Some(after) = after {
                        assert!(!after.value().is_negative(), "{steps:?}");
                        if units_after == 0 {
                            assert!(after.value().amount().is_zero(), "{steps:?}");
                        }
                    }
                    match (outcome, step) {
                        (Err(_), _) => assert_eq!(before, after, "{steps:?}"),
                        (Ok(()), Step::Count(counted)) => {
                            assert_eq!(units_after, i64::from(*counted), "{steps:?}")
                        }
                        (Ok(()), Step::Lose(units) | Step::Damage(units)) => {
                            assert_eq!(units_after, units_before - i64::from(*units), "{steps:?}")
                        }
                        (Ok(()), Step::Enter(..)) => {}
                    }
                    if let (false, Some(old), Some(new)) = (
                        matches!(step, Step::Enter(..)),
                        before.and_then(|v| v.average_cost()),
                        after.and_then(|v| v.average_cost()),
                    ) {
                        let drift = (old.amount() - new.amount()).abs();
                        assert!(drift <= slack, "{steps:?}: {old} to {new}");
                    }
                }
            });
        }
    }
}

#[test]
fn the_average_cost_outlives_the_last_unit_and_is_none_before_the_first() {
    block_on(async {
        let f = Fixture::new().await;
        assert_eq!(f.inventory.average_cost(id(FONE)).await.unwrap(), None);
        f.enter(&[entry(FONE, 2, "50.00")]).await.unwrap();
        f.enter(&[entry(FONE, 2, "70.00")]).await.unwrap();
        assert_eq!(
            f.inventory.average_cost(id(FONE)).await.unwrap(),
            Some(brl("30"))
        );

        f.adjust(FONE, StockAdjustment::Loss(4)).await.unwrap();

        assert_eq!(f.units(FONE).await, 0);
        assert_eq!(
            f.inventory.average_cost(id(FONE)).await.unwrap(),
            Some(brl("30"))
        );
        assert_eq!(f.inventory.average_cost(id(CAPA)).await.unwrap(), None);
    });
}
