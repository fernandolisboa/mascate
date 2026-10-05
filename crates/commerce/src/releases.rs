//! Money Releases (#28): when the money of each Order's payment is free to
//! withdraw from the Mercado Pago, as the wallet reports it (ADR 0026).
//! After each Order Sync the app asks about the Orders whose money is not
//! released yet, keeping each payment by the wallet's id, so asking again
//! changes nothing.

use std::collections::BTreeMap;

use chrono::TimeDelta;
use libsql::{Connection, TransactionBehavior, params};
use mascate_kernel::{Money, PlatformError, RecordId, Timestamp};
use mascate_platform::{Migration, StoredRow, stored};

use crate::{OrderError, OrderStatus, Orders};

pub(crate) const CREATE_MONEY_RELEASES: Migration = Migration {
    version: 11,
    name: "create the Money Releases of each Order",
    risky: false,
    sql: "ALTER TABLE commerce_orders ADD COLUMN releases_checked_at TEXT;
    CREATE TABLE commerce_money_releases (
        id            TEXT    PRIMARY KEY,
        order_id      TEXT    NOT NULL REFERENCES commerce_orders (id),
        ml_payment_id TEXT    NOT NULL,
        amount        TEXT    NOT NULL,
        currency      TEXT    NOT NULL,
        released      INTEGER NOT NULL,
        release_at    TEXT,
        created_at    TEXT    NOT NULL,
        updated_at    TEXT    NOT NULL,
        deleted_at    TEXT
    );
    CREATE UNIQUE INDEX commerce_money_releases_by_payment
        ON commerce_money_releases (order_id, ml_payment_id) WHERE deleted_at IS NULL;",
};

/// A payment still held is asked about again after this long, once its
/// date to be released has come or while it has none.
const PENDING_RETRY_HOURS: i64 = 6;

/// Past this long after the sale, an Order whose money the wallet never
/// released stops being asked about until the Order changes.
const RELEASE_GIVE_UP_DAYS: i64 = 90;

/// One payment of an Order, as the wallet reports its money.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentRelease {
    /// The channel's id of the Order it paid.
    pub order: String,
    /// The wallet's id of the payment: what keeps it from counting twice.
    pub payment: String,
    /// What the owner receives of it, as the wallet reports.
    pub amount: Money,
    /// The money is free to withdraw.
    pub released: bool,
    /// When it was released or is due to be; `None` while the wallet has
    /// no date yet, as before the buyer gets the item.
    pub release_at: Option<Timestamp>,
}

/// The wallet the Sales Channel pays the owner into, as a Platform's
/// adapter reports it. Calls block on the network, so they run off the UI
/// thread.
pub trait ChannelPayments: Send + Sync {
    /// The payments of each of `orders` (the channel's ids) that brought
    /// money in, with when it is released. A refunded or cancelled payment
    /// brings none and is left out.
    fn releases(&self, orders: &[String]) -> Result<Vec<PaymentRelease>, PlatformError>;
}

/// What asking the wallet did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReleaseImport {
    /// Orders asked about.
    pub asked: usize,
    /// Payments the wallet reported.
    pub payments: usize,
}

/// An Order's state as the release import reads it.
struct ReleaseState {
    id: RecordId,
    ml_id: String,
    /// As stored, to write over only if no Sync changed it meanwhile.
    checked_text: Option<String>,
    checked_at: Option<Timestamp>,
    cancelled: bool,
    ordered_at: Timestamp,
    /// The payments kept, and whether each is released and when.
    kept: Vec<(bool, Option<Timestamp>)>,
}

impl ReleaseState {
    fn due(&self, now: Timestamp) -> bool {
        let Some(checked) = self.checked_at else {
            return true;
        };
        if checked > now - TimeDelta::hours(PENDING_RETRY_HOURS)
            || self.ordered_at < now - TimeDelta::days(RELEASE_GIVE_UP_DAYS)
        {
            return false;
        }
        if self.kept.is_empty() {
            // Paid but not reported yet; a cancelled Order brings nothing.
            return !self.cancelled;
        }
        self.kept
            .iter()
            .any(|&(released, at)| !released && at.is_none_or(|at| at <= now))
    }
}

impl Orders {
    /// Asks the wallet about each Order never asked about since it last
    /// changed and, every few hours for its first 90 days, about each one
    /// with money still held whose date has come or is not known. Keeps
    /// each payment by the wallet's id; a payment the wallet no longer
    /// reports for an Order asked about goes.
    pub async fn import_releases(
        &self,
        wallet: &dyn ChannelPayments,
    ) -> Result<ReleaseImport, OrderError> {
        let now = self.clock.now();
        let due: Vec<ReleaseState> = release_states(self.database.connection())
            .await?
            .into_iter()
            .filter(|state| state.due(now))
            .collect();
        if due.is_empty() {
            return Ok(ReleaseImport::default());
        }
        let asked: Vec<String> = due.iter().map(|state| state.ml_id.clone()).collect();
        let mut reported: BTreeMap<String, Vec<PaymentRelease>> = BTreeMap::new();
        for payment in wallet.releases(&asked)? {
            reported
                .entry(payment.order.clone())
                .or_default()
                .push(payment);
        }

        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let mut report = ReleaseImport {
            asked: due.len(),
            payments: 0,
        };
        for state in &due {
            let payments = reported.remove(&state.ml_id).unwrap_or_default();
            report.payments += payments.len();
            self.write_releases(&transaction, state.id, &payments, now)
                .await?;
            // Only if no Order Sync changed the Order meanwhile: a change
            // asks again.
            transaction
                .execute(
                    "UPDATE commerce_orders SET releases_checked_at = ?1
                     WHERE id = ?2 AND COALESCE(releases_checked_at, '') = COALESCE(?3, '')",
                    params![
                        stored(now),
                        state.id.to_string(),
                        state.checked_text.clone()
                    ],
                )
                .await?;
        }
        transaction.commit().await?;
        Ok(report)
    }

    /// Every payment kept, of every Order, the soonest to be released
    /// first and those without a date last.
    pub async fn money_releases(&self) -> Result<Vec<PaymentRelease>, OrderError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT ord.ml_order_id, rel.ml_payment_id, rel.amount, rel.currency,
                     rel.released, rel.release_at
                 FROM commerce_money_releases rel
                 JOIN commerce_orders ord ON ord.id = rel.order_id
                 WHERE rel.deleted_at IS NULL AND ord.deleted_at IS NULL
                 ORDER BY rel.release_at IS NULL, rel.release_at, rel.ml_payment_id",
                (),
            )
            .await?;
        let mut releases = Vec::new();
        while let Some(row) = rows.next().await? {
            releases.push(PaymentRelease {
                order: row.get(0)?,
                payment: row.get(1)?,
                amount: Money::new(row.decimal_at(2)?, row.currency_at(3)?),
                released: row.get::<i64>(4)? != 0,
                release_at: row.optional_time_at(5)?,
            });
        }
        Ok(releases)
    }

    /// Writes the wallet's payments over an Order's: each by its id, and
    /// those the wallet no longer reports gone.
    async fn write_releases(
        &self,
        on: &Connection,
        order: RecordId,
        payments: &[PaymentRelease],
        now: Timestamp,
    ) -> Result<(), OrderError> {
        for payment in payments {
            on.execute(
                "INSERT INTO commerce_money_releases
                     (id, order_id, ml_payment_id, amount, currency, released, release_at,
                      created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 ON CONFLICT (order_id, ml_payment_id) WHERE deleted_at IS NULL
                 DO UPDATE SET amount = excluded.amount, currency = excluded.currency,
                     released = excluded.released, release_at = excluded.release_at,
                     updated_at = excluded.updated_at",
                params![
                    self.ids.next_id().to_string(),
                    order.to_string(),
                    payment.payment.clone(),
                    payment.amount.amount().to_string(),
                    payment.amount.currency().code(),
                    i64::from(payment.released),
                    payment.release_at.map(stored),
                    stored(now)
                ],
            )
            .await?;
        }
        on.execute(
            "UPDATE commerce_money_releases SET deleted_at = ?1, updated_at = ?1
             WHERE order_id = ?2 AND deleted_at IS NULL AND updated_at < ?1",
            params![stored(now), order.to_string()],
        )
        .await?;
        Ok(())
    }
}

/// What each Order's status and kept payments say to the release import.
async fn release_states(on: &Connection) -> Result<Vec<ReleaseState>, OrderError> {
    let mut rows = on
        .query(
            "SELECT ord.id, ord.ml_order_id, ord.releases_checked_at, ord.status,
                 ord.ordered_at, rel.released, rel.release_at
             FROM commerce_orders ord
             LEFT JOIN commerce_money_releases rel
                 ON rel.order_id = ord.id AND rel.deleted_at IS NULL
             WHERE ord.deleted_at IS NULL
             ORDER BY ord.ordered_at, ord.id",
            (),
        )
        .await?;
    let mut states: Vec<ReleaseState> = Vec::new();
    while let Some(row) = rows.next().await? {
        let id = row.id_at(0)?;
        if states.last().is_none_or(|state| state.id != id) {
            states.push(ReleaseState {
                id,
                ml_id: row.get(1)?,
                checked_text: row.get(2)?,
                checked_at: row.optional_time_at(2)?,
                cancelled: row.get::<String>(3)? == OrderStatus::Cancelled.code(),
                ordered_at: row.time_at(4)?,
                kept: Vec::new(),
            });
        }
        if let Some(released) = row.get::<Option<i64>>(5)? {
            let state = states.last_mut().expect("pushed above");
            state.kept.push((released != 0, row.optional_time_at(6)?));
        }
    }
    Ok(states)
}
