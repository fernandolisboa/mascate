//! Payouts (#79): the money a Platform passed on to the owner and the
//! Commissions it covers (ADR 0030, decision 6). One a source reports is
//! kept by its natural key with the Commissions the Platform names. Where
//! the Platform reports none, the owner records it, the app proposes the
//! approved Commissions it covers, oldest first, and nothing is paid until
//! the owner confirms. A Commission is covered by one Payout at most.

use std::collections::HashMap;
use std::ops::Range;

use libsql::{Connection, TransactionBehavior, params};
use mascate_kernel::{Money, RecordId, Timestamp};
use mascate_platform::{Migration, StoredRow, stored};
use rust_decimal::Decimal;

use crate::commissions::{
    Affiliate, AffiliateError, COMMISSION_QUERY, Commission, Recorded, commission_from,
    platform_at, read_stored,
};
use crate::source::{AffiliatePlatform, CommissionStatus, ReportedPayout};

pub(crate) const CREATE_PAYOUTS: Migration = Migration {
    version: 2,
    name: "create Payouts and the Commissions they cover",
    risky: false,
    sql: "CREATE TABLE affiliate_payouts (
        id          TEXT    PRIMARY KEY,
        platform    TEXT    NOT NULL,
        natural_key TEXT    NOT NULL,
        paid_at     TEXT    NOT NULL,
        amount      TEXT    NOT NULL,
        currency    TEXT    NOT NULL,
        manual      INTEGER NOT NULL,
        raw         TEXT,
        created_at  TEXT    NOT NULL,
        updated_at  TEXT    NOT NULL,
        deleted_at  TEXT
    );
    CREATE UNIQUE INDEX affiliate_payouts_by_key
        ON affiliate_payouts (platform, natural_key)
        WHERE deleted_at IS NULL;
    CREATE TABLE affiliate_payout_commissions (
        id            TEXT PRIMARY KEY,
        payout_id     TEXT NOT NULL REFERENCES affiliate_payouts (id),
        commission_id TEXT NOT NULL REFERENCES affiliate_commissions (id),
        created_at    TEXT NOT NULL,
        updated_at    TEXT NOT NULL,
        deleted_at    TEXT
    );
    CREATE UNIQUE INDEX affiliate_payout_commissions_by_commission
        ON affiliate_payout_commissions (commission_id)
        WHERE deleted_at IS NULL;
    CREATE INDEX affiliate_payout_commissions_by_payout
        ON affiliate_payout_commissions (payout_id);",
};

/// What the owner types to record a Payout from a Platform that reports
/// none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualPayout {
    pub platform: AffiliatePlatform,
    pub paid_at: Timestamp,
    pub amount: Money,
}

/// Money a Platform passed on, with the Commissions it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payout {
    pub id: RecordId,
    pub platform: AffiliatePlatform,
    pub paid_at: Timestamp,
    pub amount: Money,
    /// Recorded by the owner, who may delete it.
    pub manual: bool,
    /// The oldest sale first.
    pub covers: Vec<Commission>,
}

impl Payout {
    /// The sum of the Commissions it covers, all in its currency.
    pub fn covered(&self) -> Money {
        sum_in(self.amount, self.covers.iter())
    }

    /// What came beyond the Commissions it covers: negative when the
    /// Platform held back a fee or a tax.
    pub fn difference(&self) -> Money {
        difference(self.amount, self.covered())
    }
}

/// What a Payout the owner typed may cover, and what the app proposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayoutProposal {
    pub payout: ManualPayout,
    /// The approved Commissions of the Platform and currency no Payout
    /// covers yet, the oldest sale first.
    pub candidates: Vec<Commission>,
    /// At first the oldest candidates whose sum stays within the amount;
    /// the owner adjusts it before confirming.
    pub proposed: Vec<RecordId>,
}

impl PayoutProposal {
    /// The sum of the proposed Commissions.
    pub fn covered(&self) -> Money {
        sum_in(
            self.payout.amount,
            self.candidates
                .iter()
                .filter(|commission| self.proposed.contains(&commission.id)),
        )
    }

    /// What the Payout brings beyond the proposed Commissions.
    pub fn difference(&self) -> Money {
        difference(self.payout.amount, self.covered())
    }
}

/// The sum of `commissions`, in the currency of `amount`, which is theirs.
fn sum_in<'a>(amount: Money, commissions: impl Iterator<Item = &'a Commission>) -> Money {
    let sum: Decimal = commissions
        .map(|commission| commission.amount.amount())
        .sum();
    Money::new(sum, amount.currency())
}

fn difference(amount: Money, covered: Money) -> Money {
    Money::new(amount.amount() - covered.amount(), amount.currency())
}

impl Affiliate {
    /// The Commissions `payout` may cover and the ones it most likely does:
    /// from the oldest sale on, while the sum stays within its amount.
    /// Nothing changes until the owner confirms with [`Self::record_payout`].
    pub async fn propose_payout(
        &self,
        payout: &ManualPayout,
    ) -> Result<PayoutProposal, AffiliateError> {
        if payout.amount.amount() <= Decimal::ZERO {
            return Err(AffiliateError::NoPayoutAmount);
        }
        let mut rows = self
            .database
            .connection()
            .query(
                &format!(
                    "{COMMISSION_QUERY} AND v.platform = ?1 AND c.currency = ?2 AND c.status = ?3
                     AND NOT EXISTS (SELECT 1 FROM affiliate_payout_commissions l
                                     WHERE l.commission_id = c.id AND l.deleted_at IS NULL)
                     ORDER BY v.sold_at, c.id"
                ),
                params![
                    payout.platform.code(),
                    payout.amount.currency().code(),
                    CommissionStatus::Approved.code()
                ],
            )
            .await?;
        let mut candidates = Vec::new();
        while let Some(row) = rows.next().await? {
            candidates.push(commission_from(&row)?);
        }
        let proposed = oldest_within(&candidates, payout.amount.amount());
        Ok(PayoutProposal {
            payout: *payout,
            candidates,
            proposed,
        })
    }

    /// Records a Payout the owner confirmed, covering `covers`: each turns
    /// paid, with the Payout's date in its history. Each must be approved,
    /// of the Payout's Platform and currency, and covered by no other.
    pub async fn record_payout(
        &self,
        payout: ManualPayout,
        covers: &[RecordId],
    ) -> Result<Payout, AffiliateError> {
        if payout.amount.amount() <= Decimal::ZERO {
            return Err(AffiliateError::NoPayoutAmount);
        }
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let mut covered = Vec::new();
        for id in covers {
            if covered.contains(id) {
                continue;
            }
            let kept = read_stored(&transaction, "c.id = ?1", params![id.to_string()])
                .await?
                .ok_or(AffiliateError::NotFound)?;
            let payable = kept.platform == payout.platform.code()
                && kept.amount.currency() == payout.amount.currency()
                && kept.status == CommissionStatus::Approved
                && !kept.covered;
            if !payable {
                return Err(AffiliateError::NotPayable);
            }
            covered.push(*id);
        }
        let id = self.ids.next_id();
        let now = self.clock.now();
        transaction
            .execute(
                "INSERT INTO affiliate_payouts
                     (id, platform, natural_key, paid_at, amount, currency, manual,
                      created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7)",
                params![
                    id.to_string(),
                    payout.platform.code(),
                    self.ids.next_id().to_string(),
                    stored(payout.paid_at),
                    payout.amount.amount().to_string(),
                    payout.amount.currency().code(),
                    stored(now)
                ],
            )
            .await?;
        for commission in covered {
            self.cover(&transaction, id, commission, payout.paid_at, now)
                .await?;
        }
        transaction.commit().await?;
        self.payout(id).await
    }

    /// Deletes a Payout the owner recorded: the Commissions it covered are
    /// approved again, with that in their history.
    pub async fn delete_payout(&self, payout: RecordId) -> Result<(), AffiliateError> {
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let mut rows = transaction
            .query(
                "SELECT manual FROM affiliate_payouts WHERE id = ?1 AND deleted_at IS NULL",
                params![payout.to_string()],
            )
            .await?;
        let row = rows.next().await?.ok_or(AffiliateError::NoSuchPayout)?;
        if row.get::<i64>(0)? != 1 {
            return Err(AffiliateError::NotManualPayout);
        }
        drop(rows);
        let now = self.clock.now();
        let mut rows = transaction
            .query(
                "SELECT l.commission_id FROM affiliate_payout_commissions l
                 JOIN affiliate_commissions c ON c.id = l.commission_id
                 WHERE l.payout_id = ?1 AND l.deleted_at IS NULL AND c.status = ?2",
                params![payout.to_string(), CommissionStatus::Paid.code()],
            )
            .await?;
        let mut paid = Vec::new();
        while let Some(row) = rows.next().await? {
            paid.push(row.id_at(0)?);
        }
        drop(rows);
        for commission in paid {
            self.change_status(
                &transaction,
                commission,
                CommissionStatus::Approved,
                None,
                now,
            )
            .await?;
        }
        transaction
            .execute(
                "UPDATE affiliate_payout_commissions SET deleted_at = ?1, updated_at = ?1
                 WHERE payout_id = ?2 AND deleted_at IS NULL",
                params![stored(now), payout.to_string()],
            )
            .await?;
        transaction
            .execute(
                "UPDATE affiliate_payouts SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                params![stored(now), payout.to_string()],
            )
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// The Payouts made during `paid`, the latest first.
    pub async fn payouts(&self, paid: Range<Timestamp>) -> Result<Vec<Payout>, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                &format!("{PAYOUT_QUERY} ORDER BY paid_at DESC, id DESC"),
                (),
            )
            .await?;
        let mut payouts = Vec::new();
        while let Some(row) = rows.next().await? {
            let payout = payout_from(&row)?;
            // Filtered here: MIN_UTC and MAX_UTC don't sort as text.
            if paid.contains(&payout.paid_at) {
                payouts.push(payout);
            }
        }
        for payout in &mut payouts {
            payout.covers = self.covered_by(payout.id).await?;
        }
        Ok(payouts)
    }

    pub async fn payout(&self, payout: RecordId) -> Result<Payout, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                &format!("{PAYOUT_QUERY} AND id = ?1"),
                params![payout.to_string()],
            )
            .await?;
        let row = rows.next().await?.ok_or(AffiliateError::NoSuchPayout)?;
        let mut payout = payout_from(&row)?;
        payout.covers = self.covered_by(payout.id).await?;
        Ok(payout)
    }

    /// Keeps the Payouts a source reported by their natural key, each with
    /// the Commissions the Platform says it covers. One the Platform names
    /// that is unknown yet, in another currency or covered by another Payout
    /// is left out; reading the Payout again links it once it can be.
    pub(crate) async fn record_reported_payouts(
        &self,
        on: &Connection,
        platform: AffiliatePlatform,
        reported: &[ReportedPayout],
    ) -> Result<Recorded, AffiliateError> {
        let mut last: HashMap<&str, usize> = HashMap::new();
        for (index, payout) in reported.iter().enumerate() {
            last.insert(payout.key.as_str(), index);
        }
        let now = self.clock.now();
        let mut recorded = Recorded::default();
        for (index, payout) in reported.iter().enumerate() {
            if last.get(payout.key.as_str()) != Some(&index) || payout.key.is_empty() {
                continue;
            }
            let mut rows = on
                .query(
                    "SELECT id, manual, paid_at, amount, currency, raw FROM affiliate_payouts
                     WHERE platform = ?1 AND natural_key = ?2 AND deleted_at IS NULL",
                    params![platform.code(), payout.key.clone()],
                )
                .await?;
            let existing = match rows.next().await? {
                Some(row) => Some((
                    row.id_at(0)?,
                    row.get::<i64>(1)? == 1,
                    row.time_at(2)?,
                    Money::new(row.decimal_at(3)?, row.currency_at(4)?),
                    row.get::<Option<String>>(5)?,
                )),
                None => None,
            };
            drop(rows);
            // Whether it is new, and whether anything about it changed.
            let (id, added, mut changed) = match existing {
                Some((_, true, ..)) => {
                    recorded.unchanged += 1;
                    continue;
                }
                Some((id, false, paid_at, amount, raw)) => {
                    let same = paid_at == payout.paid_at
                        && amount == payout.amount
                        && raw.as_deref() == Some(payout.raw.as_str());
                    if !same {
                        on.execute(
                            "UPDATE affiliate_payouts
                             SET paid_at = ?1, amount = ?2, currency = ?3, raw = ?4,
                                 updated_at = ?5
                             WHERE id = ?6",
                            params![
                                stored(payout.paid_at),
                                payout.amount.amount().to_string(),
                                payout.amount.currency().code(),
                                payout.raw.clone(),
                                stored(now),
                                id.to_string()
                            ],
                        )
                        .await?;
                    }
                    (id, false, !same)
                }
                None => {
                    let id = self.ids.next_id();
                    on.execute(
                        "INSERT INTO affiliate_payouts
                             (id, platform, natural_key, paid_at, amount, currency, manual, raw,
                              created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, ?8)",
                        params![
                            id.to_string(),
                            platform.code(),
                            payout.key.clone(),
                            stored(payout.paid_at),
                            payout.amount.amount().to_string(),
                            payout.amount.currency().code(),
                            payout.raw.clone(),
                            stored(now)
                        ],
                    )
                    .await?;
                    (id, true, false)
                }
            };
            for key in &payout.covers {
                let Some(kept) = read_stored(
                    on,
                    "v.platform = ?1 AND v.natural_key = ?2 AND v.manual = 0",
                    params![platform.code(), key.clone()],
                )
                .await?
                else {
                    continue;
                };
                if kept.covered || kept.amount.currency() != payout.amount.currency() {
                    continue;
                }
                self.cover(on, id, kept.commission, payout.paid_at, now)
                    .await?;
                changed = true;
            }
            if added {
                recorded.added += 1;
            } else if changed {
                recorded.changed += 1;
            } else {
                recorded.unchanged += 1;
            }
        }
        Ok(recorded)
    }

    /// Links `commission` to `payout` and, unless the Platform reversed or
    /// cancelled it, makes it paid as of the Payout's date.
    async fn cover(
        &self,
        on: &Connection,
        payout: RecordId,
        commission: RecordId,
        paid_at: Timestamp,
        now: Timestamp,
    ) -> Result<(), AffiliateError> {
        on.execute(
            "INSERT INTO affiliate_payout_commissions
                 (id, payout_id, commission_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![
                self.ids.next_id().to_string(),
                payout.to_string(),
                commission.to_string(),
                stored(now)
            ],
        )
        .await?;
        let mut rows = on
            .query(
                "SELECT status FROM affiliate_commissions WHERE id = ?1",
                params![commission.to_string()],
            )
            .await?;
        let status: String = rows
            .next()
            .await?
            .expect("the Commission was just read")
            .get(0)?;
        drop(rows);
        let payable = [CommissionStatus::Pending, CommissionStatus::Approved]
            .iter()
            .any(|payable| payable.code() == status);
        if payable {
            self.change_status(on, commission, CommissionStatus::Paid, Some(paid_at), now)
                .await?;
        }
        Ok(())
    }

    /// Sets the status of `commission` as the app saw it change, keeping
    /// the Platform's own status, and adds it to the history.
    async fn change_status(
        &self,
        on: &Connection,
        commission: RecordId,
        status: CommissionStatus,
        platform_at: Option<Timestamp>,
        now: Timestamp,
    ) -> Result<(), AffiliateError> {
        on.execute(
            "UPDATE affiliate_commissions SET status = ?1, status_since = ?2, updated_at = ?3
             WHERE id = ?4",
            params![
                status.code(),
                stored(platform_at.unwrap_or(now)),
                stored(now),
                commission.to_string()
            ],
        )
        .await?;
        on.execute(
            "INSERT INTO affiliate_commission_status_changes
                 (id, commission_id, status, platform_status, seen_at, platform_at,
                  created_at, updated_at)
             SELECT ?1, id, status, platform_status, ?2, ?3, ?2, ?2
             FROM affiliate_commissions WHERE id = ?4",
            params![
                self.ids.next_id().to_string(),
                stored(now),
                platform_at.map(stored),
                commission.to_string()
            ],
        )
        .await?;
        Ok(())
    }

    async fn covered_by(&self, payout: RecordId) -> Result<Vec<Commission>, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                &format!(
                    "{COMMISSION_QUERY} AND c.id IN (
                         SELECT commission_id FROM affiliate_payout_commissions
                         WHERE payout_id = ?1 AND deleted_at IS NULL)
                     ORDER BY v.sold_at, c.id"
                ),
                params![payout.to_string()],
            )
            .await?;
        let mut commissions = Vec::new();
        while let Some(row) = rows.next().await? {
            commissions.push(commission_from(&row)?);
        }
        Ok(commissions)
    }
}

const PAYOUT_QUERY: &str = "SELECT id, platform, paid_at, amount, currency, manual
     FROM affiliate_payouts WHERE deleted_at IS NULL";

fn payout_from(row: &libsql::Row) -> Result<Payout, AffiliateError> {
    Ok(Payout {
        id: row.id_at(0)?,
        platform: platform_at(row, 1)?,
        paid_at: row.time_at(2)?,
        amount: Money::new(row.decimal_at(3)?, row.currency_at(4)?),
        manual: row.get::<i64>(5)? == 1,
        covers: Vec::new(),
    })
}

/// The first `candidates` whose running sum stays within `amount`: a
/// Platform pays the oldest Commissions first, so the proposal stops at the
/// first one that would go past it.
fn oldest_within(candidates: &[Commission], amount: Decimal) -> Vec<RecordId> {
    let mut sum = Decimal::ZERO;
    candidates
        .iter()
        .take_while(|commission| {
            sum += commission.amount.amount();
            sum <= amount
        })
        .map(|commission| commission.id)
        .collect()
}
