//! Payouts through the Affiliate module's public interface, with a real
//! temporary database, an injected clock and ids, and an in-memory source:
//! the proposal for a Payout the owner records, nothing paid before the
//! owner confirms, one Payout per Commission, deleting a Payout, the
//! difference with its sign, and the Payouts a Platform reports kept by
//! their natural key.

use std::ops::Range;
use std::sync::{Arc, Mutex};

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliatePlatform, Commission, CommissionFilter, CommissionSource,
    CommissionStatus, MIGRATIONS, ManualCommission, ManualPayout, Recorded, ReportedConversion,
    ReportedPayout, ReportedProgram,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, PlatformError, RecordId, Timestamp};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

/// A Platform that reports Payouts, answering from memory.
struct Source {
    conversions: Mutex<Vec<ReportedConversion>>,
    payouts: Mutex<Vec<ReportedPayout>>,
}

impl Source {
    fn new() -> Self {
        Self {
            conversions: Mutex::new(Vec::new()),
            payouts: Mutex::new(Vec::new()),
        }
    }

    fn reports(&self, conversions: Vec<ReportedConversion>, payouts: Vec<ReportedPayout>) {
        *self.conversions.lock().unwrap() = conversions;
        *self.payouts.lock().unwrap() = payouts;
    }
}

impl CommissionSource for Source {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Eduzz
    }

    fn history(&self) -> TimeDelta {
        TimeDelta::days(90)
    }

    fn settle_time(&self) -> TimeDelta {
        TimeDelta::days(30)
    }

    fn conversions(&self, _: Range<Timestamp>) -> Result<Vec<ReportedConversion>, PlatformError> {
        Ok(self.conversions.lock().unwrap().clone())
    }

    fn payouts(&self, _: Range<Timestamp>) -> Result<Vec<ReportedPayout>, PlatformError> {
        Ok(self.payouts.lock().unwrap().clone())
    }
}

fn noon() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 13, 12, 0, 0).unwrap()
}

fn brl(amount: &str) -> Money {
    Money::new(amount.parse::<Decimal>().unwrap(), Currency::Brl)
}

fn usd(amount: &str) -> Money {
    Money::new(amount.parse::<Decimal>().unwrap(), Currency::Usd)
}

/// A Commission recorded by hand on Monetizze, sold `days_ago`.
fn commission(
    product: &str,
    days_ago: i64,
    amount: Money,
    status: CommissionStatus,
) -> ManualCommission {
    ManualCommission {
        platform: AffiliatePlatform::Monetizze,
        program: String::new(),
        product: product.into(),
        sold_at: noon() - TimeDelta::days(days_ago),
        amount,
        status,
    }
}

fn payout(amount: Money) -> ManualPayout {
    ManualPayout {
        platform: AffiliatePlatform::Monetizze,
        paid_at: noon() - TimeDelta::days(1),
        amount,
    }
}

/// An Eduzz sale, approved.
fn sale(key: &str, amount: Money) -> ReportedConversion {
    ReportedConversion {
        key: key.into(),
        program: ReportedProgram {
            id: "77001".into(),
            name: "Curso de Excel".into(),
        },
        product: "Curso de Excel".into(),
        sold_at: noon() - TimeDelta::days(20),
        commission: amount,
        status: CommissionStatus::Approved,
        platform_status: "paid".into(),
        status_changed_at: None,
        tracking: None,
        raw: format!(r#"{{"sale":"{key}"}}"#),
    }
}

fn transfer(key: &str, amount: Money, covers: &[&str]) -> ReportedPayout {
    ReportedPayout {
        key: key.into(),
        paid_at: noon() - TimeDelta::days(2),
        amount,
        covers: covers.iter().map(|key| (*key).to_owned()).collect(),
        raw: format!(r#"{{"transfer":"{key}"}}"#),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    affiliate: Affiliate,
    eduzz: Source,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(noon()));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        let affiliate = Affiliate::new(database, clock.clone(), Arc::new(SequentialIds::default()));
        Self {
            _dir: dir,
            clock,
            affiliate,
            eduzz: Source::new(),
        }
    }

    async fn record(&self, manual: ManualCommission) -> Commission {
        self.affiliate.record_manual(manual).await.unwrap()
    }

    async fn status(&self, id: RecordId) -> CommissionStatus {
        self.affiliate.commission(id).await.unwrap().status
    }

    async fn sync(&self) -> (Recorded, Recorded) {
        let sync = self.affiliate.sync(&self.eduzz).await.unwrap();
        (sync.recorded, sync.payouts)
    }

    async fn eduzz_commission(&self) -> Commission {
        self.affiliate
            .commissions(&CommissionFilter::default())
            .await
            .unwrap()
            .into_iter()
            .find(|commission| commission.platform == AffiliatePlatform::Eduzz)
            .unwrap()
    }
}

fn ids(commissions: &[&Commission]) -> Vec<RecordId> {
    commissions.iter().map(|commission| commission.id).collect()
}

#[test]
fn the_proposal_takes_approved_commissions_of_the_platform_and_currency_oldest_first_up_to_the_amount()
 {
    block_on(async {
        let fixture = Fixture::new().await;
        let oldest = fixture
            .record(commission(
                "A",
                40,
                brl("30.00"),
                CommissionStatus::Approved,
            ))
            .await;
        let older = fixture
            .record(commission(
                "B",
                30,
                brl("20.00"),
                CommissionStatus::Approved,
            ))
            .await;
        let past_the_amount = fixture
            .record(commission(
                "C",
                20,
                brl("60.00"),
                CommissionStatus::Approved,
            ))
            .await;
        let small_but_newer = fixture
            .record(commission("D", 10, brl("5.00"), CommissionStatus::Approved))
            .await;
        // None of these can be in a Monetizze Payout in reais.
        fixture
            .record(commission("E", 50, brl("10.00"), CommissionStatus::Pending))
            .await;
        fixture
            .record(commission("F", 50, brl("10.00"), CommissionStatus::Paid))
            .await;
        fixture
            .record(commission(
                "G",
                50,
                brl("10.00"),
                CommissionStatus::Reversed,
            ))
            .await;
        fixture
            .record(commission(
                "H",
                50,
                usd("10.00"),
                CommissionStatus::Approved,
            ))
            .await;
        fixture
            .record(ManualCommission {
                platform: AffiliatePlatform::Hotmart,
                ..commission("I", 50, brl("10.00"), CommissionStatus::Approved)
            })
            .await;

        let proposal = fixture
            .affiliate
            .propose_payout(&payout(brl("80.00")))
            .await
            .unwrap();

        assert_eq!(
            ids(&proposal.candidates.iter().collect::<Vec<_>>()),
            ids(&[&oldest, &older, &past_the_amount, &small_but_newer])
        );
        // 30 + 20 fit in 80; 60 more would pass it, and the proposal stops
        // there rather than skipping ahead to a newer one.
        assert_eq!(proposal.proposed, ids(&[&oldest, &older]));
        assert_eq!(proposal.payout, payout(brl("80.00")));
        assert_eq!(proposal.covered(), brl("50.00"));
        assert_eq!(proposal.difference(), brl("30.00"));
    });
}

#[test]
fn nothing_is_paid_until_the_owner_confirms_and_then_the_payout_date_is_in_the_history() {
    block_on(async {
        let fixture = Fixture::new().await;
        let first = fixture
            .record(commission(
                "A",
                40,
                brl("30.00"),
                CommissionStatus::Approved,
            ))
            .await;
        let second = fixture
            .record(commission(
                "B",
                30,
                brl("20.00"),
                CommissionStatus::Approved,
            ))
            .await;

        let proposal = fixture
            .affiliate
            .propose_payout(&payout(brl("50.00")))
            .await
            .unwrap();
        assert_eq!(proposal.proposed, ids(&[&first, &second]));
        assert_eq!(fixture.status(first.id).await, CommissionStatus::Approved);
        assert_eq!(fixture.status(second.id).await, CommissionStatus::Approved);
        assert!(
            fixture
                .affiliate
                .payouts(Timestamp::MIN_UTC..Timestamp::MAX_UTC)
                .await
                .unwrap()
                .is_empty()
        );

        // The owner adjusts: only the first one came in this Payout.
        fixture.clock.advance(TimeDelta::hours(1));
        let recorded = fixture
            .affiliate
            .record_payout(payout(brl("50.00")), &[first.id])
            .await
            .unwrap();

        assert!(recorded.manual);
        assert_eq!(recorded.platform, AffiliatePlatform::Monetizze);
        assert_eq!(
            ids(&recorded.covers.iter().collect::<Vec<_>>()),
            vec![first.id]
        );
        assert_eq!(fixture.status(first.id).await, CommissionStatus::Paid);
        assert_eq!(fixture.status(second.id).await, CommissionStatus::Approved);
        let paid = fixture.affiliate.commission(first.id).await.unwrap();
        assert_eq!(paid.status_since, noon() - TimeDelta::days(1));
        assert_eq!(paid.payout, Some(recorded.id));
        assert_eq!(
            fixture
                .affiliate
                .commission(second.id)
                .await
                .unwrap()
                .payout,
            None
        );
        let history = fixture.affiliate.status_history(first.id).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].status, CommissionStatus::Paid);
        assert_eq!(history[1].seen_at, noon() + TimeDelta::hours(1));
        assert_eq!(history[1].platform_at, Some(noon() - TimeDelta::days(1)));
    });
}

#[test]
fn the_difference_shows_with_its_sign() {
    block_on(async {
        let fixture = Fixture::new().await;
        let first = fixture
            .record(commission(
                "A",
                40,
                brl("30.00"),
                CommissionStatus::Approved,
            ))
            .await;
        let second = fixture
            .record(commission(
                "B",
                30,
                brl("20.00"),
                CommissionStatus::Approved,
            ))
            .await;
        let third = fixture
            .record(commission(
                "C",
                20,
                brl("10.00"),
                CommissionStatus::Approved,
            ))
            .await;

        // The Platform held back R$ 2,50 from R$ 50,00.
        let withheld = fixture
            .affiliate
            .record_payout(payout(brl("47.50")), &[first.id, second.id])
            .await
            .unwrap();
        assert_eq!(withheld.covered(), brl("50.00"));
        assert_eq!(withheld.difference(), brl("-2.50"));

        // More came than the Commissions it covers.
        let more = fixture
            .affiliate
            .record_payout(payout(brl("12.00")), &[third.id])
            .await
            .unwrap();
        assert_eq!(more.difference(), brl("2.00"));

        let nothing_covered = fixture
            .affiliate
            .record_payout(payout(brl("8.00")), &[])
            .await
            .unwrap();
        assert_eq!(nothing_covered.covered(), brl("0"));
        assert_eq!(nothing_covered.difference(), brl("8.00"));
    });
}

#[test]
fn a_commission_is_covered_by_one_payout_at_most() {
    block_on(async {
        let fixture = Fixture::new().await;
        let first = fixture
            .record(commission(
                "A",
                40,
                brl("30.00"),
                CommissionStatus::Approved,
            ))
            .await;
        fixture
            .affiliate
            .record_payout(payout(brl("30.00")), &[first.id])
            .await
            .unwrap();

        let again = fixture
            .affiliate
            .record_payout(payout(brl("30.00")), &[first.id])
            .await;
        assert!(matches!(again, Err(AffiliateError::NotPayable)));
        let proposal = fixture
            .affiliate
            .propose_payout(&payout(brl("30.00")))
            .await
            .unwrap();
        assert!(proposal.candidates.is_empty());
        assert_eq!(
            fixture
                .affiliate
                .payouts(Timestamp::MIN_UTC..Timestamp::MAX_UTC)
                .await
                .unwrap()
                .len(),
            1
        );
    });
}

#[test]
fn a_payout_covers_only_approved_commissions_of_its_platform_and_currency_and_is_more_than_zero() {
    block_on(async {
        let fixture = Fixture::new().await;
        let pending = fixture
            .record(commission("A", 40, brl("30.00"), CommissionStatus::Pending))
            .await;
        let dollars = fixture
            .record(commission(
                "B",
                40,
                usd("30.00"),
                CommissionStatus::Approved,
            ))
            .await;
        let hotmart = fixture
            .record(ManualCommission {
                platform: AffiliatePlatform::Hotmart,
                ..commission("C", 40, brl("30.00"), CommissionStatus::Approved)
            })
            .await;
        let fine = fixture
            .record(commission(
                "D",
                40,
                brl("30.00"),
                CommissionStatus::Approved,
            ))
            .await;

        for wrong in [pending.id, dollars.id, hotmart.id] {
            let recorded = fixture
                .affiliate
                .record_payout(payout(brl("30.00")), &[fine.id, wrong])
                .await;
            assert!(matches!(recorded, Err(AffiliateError::NotPayable)));
        }
        // A refused Payout paid nothing.
        assert_eq!(fixture.status(fine.id).await, CommissionStatus::Approved);
        for amount in [brl("0"), brl("-1.00")] {
            assert!(matches!(
                fixture
                    .affiliate
                    .record_payout(payout(amount), &[fine.id])
                    .await,
                Err(AffiliateError::NoPayoutAmount)
            ));
            assert!(matches!(
                fixture.affiliate.propose_payout(&payout(amount)).await,
                Err(AffiliateError::NoPayoutAmount)
            ));
        }
        assert!(matches!(
            fixture
                .affiliate
                .record_payout(payout(brl("30.00")), &[RecordId::from_u128(999)])
                .await,
            Err(AffiliateError::NotFound)
        ));
    });
}

#[test]
fn deleting_a_payout_the_owner_recorded_returns_its_commissions_to_approved() {
    block_on(async {
        let fixture = Fixture::new().await;
        let first = fixture
            .record(commission(
                "A",
                40,
                brl("30.00"),
                CommissionStatus::Approved,
            ))
            .await;
        let recorded = fixture
            .affiliate
            .record_payout(payout(brl("30.00")), &[first.id])
            .await
            .unwrap();
        fixture.clock.advance(TimeDelta::hours(2));

        fixture.affiliate.delete_payout(recorded.id).await.unwrap();

        let returned = fixture.affiliate.commission(first.id).await.unwrap();
        assert_eq!(returned.status, CommissionStatus::Approved);
        assert_eq!(returned.payout, None);
        let statuses: Vec<CommissionStatus> = fixture
            .affiliate
            .status_history(first.id)
            .await
            .unwrap()
            .iter()
            .map(|change| change.status)
            .collect();
        assert_eq!(
            statuses,
            [
                CommissionStatus::Approved,
                CommissionStatus::Paid,
                CommissionStatus::Approved
            ]
        );
        assert!(
            fixture
                .affiliate
                .payouts(Timestamp::MIN_UTC..Timestamp::MAX_UTC)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            fixture.affiliate.delete_payout(recorded.id).await,
            Err(AffiliateError::NoSuchPayout)
        ));
        // Free again, it can go into another Payout.
        fixture
            .affiliate
            .record_payout(payout(brl("30.00")), &[first.id])
            .await
            .unwrap();
    });
}

#[test]
fn a_commission_a_payout_covers_is_changed_only_after_the_payout_is_deleted() {
    block_on(async {
        let fixture = Fixture::new().await;
        let first = fixture
            .record(commission(
                "A",
                40,
                brl("30.00"),
                CommissionStatus::Approved,
            ))
            .await;
        let recorded = fixture
            .affiliate
            .record_payout(payout(brl("30.00")), &[first.id])
            .await
            .unwrap();

        assert!(matches!(
            fixture
                .affiliate
                .edit_manual(
                    first.id,
                    commission("A", 40, brl("31.00"), CommissionStatus::Approved)
                )
                .await,
            Err(AffiliateError::Covered)
        ));
        assert!(matches!(
            fixture.affiliate.delete_manual(first.id).await,
            Err(AffiliateError::Covered)
        ));

        fixture.affiliate.delete_payout(recorded.id).await.unwrap();
        fixture.affiliate.delete_manual(first.id).await.unwrap();
    });
}

#[test]
fn payouts_are_listed_by_payment_date_the_latest_first() {
    block_on(async {
        let fixture = Fixture::new().await;
        let earlier = fixture
            .affiliate
            .record_payout(
                ManualPayout {
                    paid_at: noon() - TimeDelta::days(40),
                    ..payout(brl("10.00"))
                },
                &[],
            )
            .await
            .unwrap();
        let later = fixture
            .affiliate
            .record_payout(payout(brl("20.00")), &[])
            .await
            .unwrap();

        let all = fixture
            .affiliate
            .payouts(Timestamp::MIN_UTC..Timestamp::MAX_UTC)
            .await
            .unwrap();
        assert_eq!(
            all.iter().map(|payout| payout.id).collect::<Vec<_>>(),
            [later.id, earlier.id]
        );
        let this_month = fixture
            .affiliate
            .payouts(noon() - TimeDelta::days(12)..noon())
            .await
            .unwrap();
        assert_eq!(this_month, [later]);
    });
}

#[test]
fn payouts_a_platform_reports_are_kept_once_by_their_key_and_pay_what_they_cover() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.eduzz.reports(
            vec![sale("S1", brl("40.00"))],
            vec![transfer("T1", brl("38.00"), &["S1"])],
        );

        let (_, payouts) = fixture.sync().await;
        assert_eq!(
            payouts,
            Recorded {
                added: 1,
                changed: 0,
                unchanged: 0
            }
        );
        fixture.clock.advance(TimeDelta::hours(1));
        let (recorded, payouts) = fixture.sync().await;
        assert_eq!(payouts.unchanged, 1);
        // Eduzz still calls the sale paid by the buyer: it stays paid out.
        assert_eq!(recorded.unchanged, 1);

        let all = fixture
            .affiliate
            .payouts(Timestamp::MIN_UTC..Timestamp::MAX_UTC)
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
        let reported = &all[0];
        assert!(!reported.manual);
        assert_eq!(reported.platform, AffiliatePlatform::Eduzz);
        assert_eq!(reported.paid_at, noon() - TimeDelta::days(2));
        assert_eq!(reported.difference(), brl("-2.00"));
        let sold = fixture.eduzz_commission().await;
        assert_eq!(reported.covers, std::slice::from_ref(&sold));
        assert_eq!(sold.status, CommissionStatus::Paid);
        assert!(matches!(
            fixture.affiliate.delete_payout(reported.id).await,
            Err(AffiliateError::NotManualPayout)
        ));

        // A changed amount updates the same Payout.
        fixture.eduzz.reports(
            vec![sale("S1", brl("40.00"))],
            vec![transfer("T1", brl("39.00"), &["S1"])],
        );
        let (_, payouts) = fixture.sync().await;
        assert_eq!(payouts.changed, 1);
        let all = fixture
            .affiliate
            .payouts(Timestamp::MIN_UTC..Timestamp::MAX_UTC)
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].amount, brl("39.00"));
    });
}

#[test]
fn a_platform_payout_links_a_commission_once_it_is_known_and_a_reversal_still_comes_through() {
    block_on(async {
        let fixture = Fixture::new().await;
        // The transfer arrives before the sale it covers.
        fixture
            .eduzz
            .reports(Vec::new(), vec![transfer("T1", brl("40.00"), &["S1"])]);
        fixture.sync().await;
        fixture.eduzz.reports(
            vec![sale("S1", brl("40.00"))],
            vec![transfer("T1", brl("40.00"), &["S1"])],
        );
        let (_, payouts) = fixture.sync().await;
        assert_eq!(payouts.changed, 1);
        let paid = fixture.eduzz_commission().await;
        assert_eq!(paid.status, CommissionStatus::Paid);

        fixture.eduzz.reports(
            vec![ReportedConversion {
                status: CommissionStatus::Reversed,
                platform_status: "refunded".into(),
                ..sale("S1", brl("40.00"))
            }],
            vec![transfer("T1", brl("40.00"), &["S1"])],
        );
        fixture.sync().await;
        assert_eq!(fixture.status(paid.id).await, CommissionStatus::Reversed);
    });
}

#[test]
fn a_payout_a_platform_reports_outside_a_sync_is_kept_once_by_its_key() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.eduzz.reports(
            vec![sale("S1", brl("40.00")), sale("S2", brl("10.00"))],
            Vec::new(),
        );
        fixture.sync().await;
        let validated = transfer("V1", brl("47.50"), &["S1", "S2", "unknown"]);

        let first = fixture
            .affiliate
            .record_platform_payouts(AffiliatePlatform::Eduzz, std::slice::from_ref(&validated))
            .await
            .unwrap();
        let again = fixture
            .affiliate
            .record_platform_payouts(AffiliatePlatform::Eduzz, &[validated])
            .await
            .unwrap();

        assert_eq!(first.added, 1);
        assert_eq!(again.unchanged, 1);
        let all = fixture
            .affiliate
            .payouts(Timestamp::MIN_UTC..Timestamp::MAX_UTC)
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
        assert!(!all[0].manual);
        assert_eq!(all[0].covers.len(), 2);
        assert!(
            all[0]
                .covers
                .iter()
                .all(|commission| commission.status == CommissionStatus::Paid)
        );
        assert_eq!(all[0].difference(), brl("-2.50"));
    });
}

#[test]
fn a_commission_the_owner_paid_stays_paid_when_the_platform_calls_it_approved_again() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .eduzz
            .reports(vec![sale("S1", brl("40.00"))], Vec::new());
        fixture.sync().await;
        let sold = fixture.eduzz_commission().await;
        fixture
            .affiliate
            .record_payout(
                ManualPayout {
                    platform: AffiliatePlatform::Eduzz,
                    paid_at: noon(),
                    amount: brl("40.00"),
                },
                &[sold.id],
            )
            .await
            .unwrap();

        fixture.eduzz.reports(
            vec![ReportedConversion {
                platform_status: "paid_out".into(),
                ..sale("S1", brl("40.00"))
            }],
            Vec::new(),
        );
        fixture.sync().await;

        let commission = fixture.affiliate.commission(sold.id).await.unwrap();
        assert_eq!(commission.status, CommissionStatus::Paid);
        assert_eq!(commission.platform_status, "paid_out");
    });
}

fn any_status() -> impl Strategy<Value = CommissionStatus> {
    prop::sample::select(CommissionStatus::ALL.to_vec())
}

fn any_commission() -> impl Strategy<Value = ManualCommission> {
    (
        prop::sample::select(vec![
            AffiliatePlatform::Monetizze,
            AffiliatePlatform::Hotmart,
        ]),
        prop::sample::select(vec![Currency::Brl, Currency::Usd]),
        any_status(),
        1i64..20_000,
        0i64..60,
    )
        .prop_map(
            |(platform, currency, status, cents, days_ago)| ManualCommission {
                platform,
                amount: Money::new(Decimal::new(cents, 2), currency),
                ..commission("Produto", days_ago, brl("0"), status)
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn the_proposal_is_the_oldest_approved_commissions_that_fit_in_the_amount(
        commissions in prop::collection::vec(any_commission(), 0..12),
        cents in 1i64..60_000,
    ) {
        let amount = Money::new(Decimal::new(cents, 2), Currency::Brl);
        let (eligible, proposal) = block_on(async {
            let fixture = Fixture::new().await;
            let mut eligible = Vec::new();
            for manual in &commissions {
                let recorded = fixture.record(manual.clone()).await;
                if manual.platform == AffiliatePlatform::Monetizze
                    && manual.amount.currency() == Currency::Brl
                    && manual.status == CommissionStatus::Approved
                {
                    eligible.push(recorded);
                }
            }
            eligible.sort_by_key(|commission| (commission.sold_at, commission.id));
            let proposal = fixture.affiliate.propose_payout(&payout(amount)).await.unwrap();
            (eligible, proposal)
        });

        prop_assert_eq!(&proposal.candidates, &eligible);
        let count = proposal.proposed.len();
        prop_assert_eq!(
            &proposal.proposed,
            &eligible[..count].iter().map(|c| c.id).collect::<Vec<_>>()
        );
        let sum: Decimal = eligible[..count].iter().map(|c| c.amount.amount()).sum();
        prop_assert!(sum <= amount.amount());
        if let Some(next) = eligible.get(count) {
            prop_assert!(sum + next.amount.amount() > amount.amount());
        }
    }
}
