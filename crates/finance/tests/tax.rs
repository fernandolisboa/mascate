use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_finance::{MIGRATIONS, Taxes};
use mascate_kernel::Percentage;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::{Database, migrate};
use rust_decimal::Decimal;

#[test]
fn the_tax_rate_is_zero_until_the_owner_sets_one_and_keeps_the_last_saved() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
        ));
        let database = Arc::new(Database::open(&dir.path().join("m.db")).await.unwrap());
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        let taxes = Taxes::new(database, clock, Arc::new(SequentialIds::default()));

        assert_eq!(taxes.rate().await.unwrap(), Percentage::ZERO);
        let simples = Percentage::new(Decimal::from_str("6").unwrap()).unwrap();
        taxes.save_rate(simples).await.unwrap();
        let higher = Percentage::new(Decimal::from_str("11.2").unwrap()).unwrap();
        taxes.save_rate(higher).await.unwrap();

        assert_eq!(taxes.rate().await.unwrap(), higher);
    });
}
