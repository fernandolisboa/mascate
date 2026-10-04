//! A catalog on a real temporary database and folder, with a clock and ids
//! the test controls. Each test file uses part of it.
#![allow(dead_code)]

use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use mascate_catalog::{Catalog, MIGRATIONS, NewSupplierOffer, Supplier, SupplierOffer};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money};
use mascate_platform::{Database, migrate};
use rust_decimal::Decimal;

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub clock: Arc<ManualClock>,
    pub catalog: Catalog,
}

impl Fixture {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
        ));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        let catalog = Catalog::new(
            database,
            dir.path().join("produtos"),
            clock.clone(),
            Arc::new(SequentialIds::default()),
        );
        Self {
            dir,
            clock,
            catalog,
        }
    }

    pub fn a_day_later(&self) {
        self.clock.advance(TimeDelta::days(1));
    }

    pub async fn supplier(&self, name: &str) -> Supplier {
        self.catalog.add_supplier(name).await.unwrap()
    }

    /// Registers an offer at `link` for `price` plus R$ 5 shipping.
    pub async fn offer(&self, supplier: &Supplier, link: &str, price: &str) -> SupplierOffer {
        self.catalog
            .register_offer(new_offer(
                supplier,
                link,
                "Fone de Ouvido Bluetooth TWS",
                price,
            ))
            .await
            .unwrap()
    }
}

pub fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

pub fn new_offer(supplier: &Supplier, link: &str, title: &str, price: &str) -> NewSupplierOffer {
    NewSupplierOffer {
        supplier: supplier.id,
        link: link.into(),
        title: title.into(),
        price: brl(price),
        shipping: brl("5"),
    }
}
