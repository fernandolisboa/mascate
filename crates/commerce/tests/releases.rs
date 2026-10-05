//! Money Releases through commerce's public interface with a real temporary
//! database and an in-memory wallet: each payment is kept once by the
//! wallet's id, an Order is asked about again only while its money is held
//! and its date has come, and a changed Order is asked about again.

mod common;

use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{OrderStatus, Orders, PaymentRelease, ReleaseImport};
use mascate_inventory::Inventory;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Timestamp};
use mascate_platform::{Database, migrate};

use common::{Channel, brl, order, sold};

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    orders: Orders,
    channel: Channel,
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
            _dir: dir,
            orders: Orders::new(database, inventory, clock.clone(), ids),
            clock,
            channel: Channel::default(),
        }
    }

    fn later(&self, hours: i64) -> Timestamp {
        self.clock.advance(TimeDelta::hours(hours));
        self.clock.now()
    }

    async fn sell(&self, id: &str) {
        let at = self.later(1);
        self.channel
            .sells(order(id, at, vec![sold("MLB1", None, 1)]));
        self.orders.sync(&self.channel).await.unwrap();
    }

    fn pays(
        &self,
        order: &str,
        payment: &str,
        amount: &str,
        released: bool,
        at: Option<Timestamp>,
    ) {
        let mut payments = self.channel.payments.lock().unwrap();
        payments.retain(|found| found.payment != payment);
        payments.push(PaymentRelease {
            order: order.into(),
            payment: payment.into(),
            amount: brl(amount),
            released,
            release_at: at,
        });
    }

    async fn import(&self) -> ReleaseImport {
        self.orders.import_releases(&self.channel).await.unwrap()
    }

    fn last_asked(&self) -> Option<Vec<String>> {
        self.channel.payments_asked.lock().unwrap().last().cloned()
    }

    fn times_asked(&self) -> usize {
        self.channel.payments_asked.lock().unwrap().len()
    }
}

#[test]
fn each_payment_is_kept_once_with_when_its_money_is_released() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.sell("O1").await;
        fx.sell("O2").await;
        let due = fx.clock.now() + TimeDelta::days(12);
        fx.pays("O1", "P1", "77.31", false, Some(due));
        fx.pays("O2", "P2", "40.00", false, None);

        assert_eq!(
            fx.import().await,
            ReleaseImport {
                asked: 2,
                payments: 2
            }
        );
        fx.later(7);
        fx.import().await;

        let kept = fx.orders.money_releases().await.unwrap();
        assert_eq!(
            kept,
            [
                PaymentRelease {
                    order: "O1".into(),
                    payment: "P1".into(),
                    amount: brl("77.31"),
                    released: false,
                    release_at: Some(due),
                },
                PaymentRelease {
                    order: "O2".into(),
                    payment: "P2".into(),
                    amount: brl("40.00"),
                    released: false,
                    release_at: None,
                },
            ]
        );
    });
}

#[test]
fn money_held_is_asked_about_again_once_its_date_comes_and_never_once_released() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.sell("O1").await;
        let due = fx.clock.now() + TimeDelta::days(3);
        fx.pays("O1", "P1", "77.31", false, Some(due));
        fx.import().await;

        fx.later(7);
        assert_eq!(fx.import().await, ReleaseImport::default());

        fx.clock.advance(TimeDelta::days(3));
        fx.pays("O1", "P1", "77.31", true, Some(due));
        assert_eq!(fx.import().await.asked, 1);
        assert!(fx.orders.money_releases().await.unwrap()[0].released);

        fx.later(24 * 10);
        let asked = fx.times_asked();
        fx.import().await;
        assert_eq!(fx.times_asked(), asked);
    });
}

#[test]
fn money_without_a_date_is_asked_about_every_few_hours() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.sell("O1").await;
        fx.pays("O1", "P1", "77.31", false, None);
        fx.import().await;

        fx.later(5);
        assert_eq!(fx.import().await.asked, 0);
        fx.later(1);
        assert_eq!(fx.import().await.asked, 1);
        assert_eq!(fx.last_asked(), Some(vec!["O1".to_owned()]));
    });
}

#[test]
fn a_changed_order_is_asked_about_again_and_a_payment_no_longer_reported_goes() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.sell("O1").await;
        fx.pays("O1", "P1", "77.31", true, Some(fx.clock.now()));
        fx.import().await;

        let at = fx.later(1);
        fx.channel.change_order("O1", at, |order| {
            order.status = OrderStatus::Cancelled;
        });
        fx.channel.payments.lock().unwrap().clear();
        fx.orders.sync(&fx.channel).await.unwrap();

        assert_eq!(fx.import().await.asked, 1);
        assert!(fx.orders.money_releases().await.unwrap().is_empty());
        fx.later(24);
        assert_eq!(
            fx.import().await.asked,
            0,
            "a cancelled Order without money is not asked about again"
        );
    });
}

#[test]
fn an_order_paid_but_not_reported_yet_is_asked_about_until_it_is() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.sell("O1").await;
        assert_eq!(
            fx.import().await,
            ReleaseImport {
                asked: 1,
                payments: 0
            }
        );

        fx.later(6);
        fx.pays("O1", "P1", "77.31", false, None);
        assert_eq!(fx.import().await.payments, 1);
    });
}

#[test]
fn an_old_order_with_money_held_stops_being_asked_about() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.sell("O1").await;
        fx.pays("O1", "P1", "77.31", false, None);
        fx.import().await;

        fx.clock.advance(TimeDelta::days(91));
        assert_eq!(fx.import().await.asked, 0);
    });
}
