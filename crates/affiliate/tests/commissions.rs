//! Conversions and Commissions through the Affiliate module's public
//! interface, with a real temporary database, an injected clock and ids,
//! and an in-memory Commission source: each Conversion kept once by its
//! natural key, every status change in the history, the Sync window, the
//! owner's manual Commissions and the income Reminder.

use std::ops::Range;
use std::sync::{Arc, Mutex};

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliatePlatform, COMMISSION_INCOME, CommissionFilter,
    CommissionSource, CommissionStatus, INCOME_WINDOW_DAYS, MIGRATIONS, ManualCommission, Recorded,
    ReportedConversion, ReportedProgram,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, PlatformError, Timestamp};
use mascate_platform::{Database, migrate};
use rust_decimal::Decimal;

const HISTORY_DAYS: i64 = 90;
const SETTLE_DAYS: i64 = 30;

/// A Platform answering from memory, remembering each window it was asked.
struct Source {
    platform: AffiliatePlatform,
    conversions: Mutex<Vec<ReportedConversion>>,
    asked: Mutex<Vec<Range<Timestamp>>>,
    failing: Mutex<bool>,
}

impl Source {
    fn new(platform: AffiliatePlatform) -> Self {
        Self {
            platform,
            conversions: Mutex::new(Vec::new()),
            asked: Mutex::new(Vec::new()),
            failing: Mutex::new(false),
        }
    }

    fn reports(&self, conversions: Vec<ReportedConversion>) {
        *self.conversions.lock().unwrap() = conversions;
    }

    fn fails(&self, failing: bool) {
        *self.failing.lock().unwrap() = failing;
    }

    fn last_window(&self) -> Range<Timestamp> {
        self.asked.lock().unwrap().last().cloned().unwrap()
    }
}

impl CommissionSource for Source {
    fn platform(&self) -> AffiliatePlatform {
        self.platform
    }

    fn history(&self) -> TimeDelta {
        TimeDelta::days(HISTORY_DAYS)
    }

    fn settle_time(&self) -> TimeDelta {
        TimeDelta::days(SETTLE_DAYS)
    }

    fn conversions(
        &self,
        during: Range<Timestamp>,
    ) -> Result<Vec<ReportedConversion>, PlatformError> {
        self.asked.lock().unwrap().push(during);
        if *self.failing.lock().unwrap() {
            return Err(PlatformError::RateLimited);
        }
        Ok(self.conversions.lock().unwrap().clone())
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

/// A Hotmart sale of the course, approved, five days ago.
fn sale(key: &str, status: CommissionStatus, raw_status: &str) -> ReportedConversion {
    ReportedConversion {
        key: key.into(),
        program: ReportedProgram {
            id: "4012345".into(),
            name: "Curso de Fotografia".into(),
        },
        product: "Curso de Fotografia".into(),
        sold_at: noon() - TimeDelta::days(5),
        commission: brl("97.30"),
        status,
        platform_status: raw_status.into(),
        status_changed_at: None,
        tracking: Some("instagram".into()),
        raw: format!(r#"{{"transaction":"{key}","status":"{raw_status}"}}"#),
    }
}

fn manual(product: &str, status: CommissionStatus, amount: Money) -> ManualCommission {
    ManualCommission {
        platform: AffiliatePlatform::MercadoLivre,
        program: String::new(),
        product: product.into(),
        sold_at: noon() - TimeDelta::days(2),
        amount,
        status,
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    affiliate: Affiliate,
    hotmart: Source,
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
            hotmart: Source::new(AffiliatePlatform::Hotmart),
        }
    }

    async fn sync(&self) -> Recorded {
        self.affiliate.sync(&self.hotmart).await.unwrap().recorded
    }

    async fn all(&self) -> Vec<mascate_affiliate::Commission> {
        self.affiliate
            .commissions(&CommissionFilter::default())
            .await
            .unwrap()
    }
}

#[test]
fn the_same_conversion_read_twice_is_one_commission_and_a_new_status_updates_it() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.hotmart.reports(vec![sale(
            "HP1",
            CommissionStatus::Pending,
            "WAITING_PAYMENT",
        )]);

        assert_eq!(
            fixture.sync().await,
            Recorded {
                added: 1,
                changed: 0,
                unchanged: 0
            }
        );
        fixture.clock.advance(TimeDelta::hours(1));
        assert_eq!(
            fixture.sync().await,
            Recorded {
                added: 0,
                changed: 0,
                unchanged: 1
            }
        );

        let commissions = fixture.all().await;
        assert_eq!(commissions.len(), 1);
        let first = commissions[0].clone();
        assert_eq!(first.platform, AffiliatePlatform::Hotmart);
        assert_eq!(first.program_name, "Curso de Fotografia");
        assert_eq!(first.amount, brl("97.30"));
        assert_eq!(first.status, CommissionStatus::Pending);
        assert_eq!(first.status_since, noon());
        assert_eq!(first.tracking.as_deref(), Some("instagram"));
        assert!(!first.manual);
        assert_eq!(
            fixture
                .affiliate
                .status_history(first.id)
                .await
                .unwrap()
                .len(),
            1
        );

        let approved_at = noon() - TimeDelta::days(1);
        fixture.hotmart.reports(vec![ReportedConversion {
            status_changed_at: Some(approved_at),
            ..sale("HP1", CommissionStatus::Approved, "APPROVED")
        }]);
        fixture.clock.advance(TimeDelta::hours(1));
        assert_eq!(fixture.sync().await.changed, 1);

        let commissions = fixture.all().await;
        assert_eq!(commissions.len(), 1);
        assert_eq!(commissions[0].id, first.id);
        assert_eq!(commissions[0].status, CommissionStatus::Approved);
        assert_eq!(commissions[0].platform_status, "APPROVED");
        assert_eq!(commissions[0].status_since, approved_at);
        let history = fixture.affiliate.status_history(first.id).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].status, CommissionStatus::Approved);
        assert_eq!(history[1].seen_at, noon() + TimeDelta::hours(2));
        assert_eq!(history[1].platform_at, Some(approved_at));
    });
}

#[test]
fn any_status_the_platform_reports_is_taken_even_reversed_back_to_approved() {
    block_on(async {
        let fixture = Fixture::new().await;
        let readings = [
            (CommissionStatus::Approved, "APPROVED"),
            (CommissionStatus::Reversed, "CHARGEBACK"),
            (CommissionStatus::Approved, "APPROVED"),
            (CommissionStatus::Paid, "COMPLETE"),
            (CommissionStatus::Cancelled, "CANCELLED"),
        ];
        for (status, raw) in readings {
            fixture.hotmart.reports(vec![sale("HP1", status, raw)]);
            fixture.sync().await;
            fixture.clock.advance(TimeDelta::minutes(60));
        }

        let commission = fixture.all().await.remove(0);
        assert_eq!(commission.status, CommissionStatus::Cancelled);
        let history: Vec<CommissionStatus> = fixture
            .affiliate
            .status_history(commission.id)
            .await
            .unwrap()
            .iter()
            .map(|change| change.status)
            .collect();
        assert_eq!(history, readings.map(|(status, _)| status));
    });
}

#[test]
fn a_new_platform_status_with_the_same_commission_status_is_history_too() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .hotmart
            .reports(vec![sale("HP1", CommissionStatus::Approved, "APPROVED")]);
        fixture.sync().await;
        fixture
            .hotmart
            .reports(vec![sale("HP1", CommissionStatus::Approved, "COMPLETE")]);
        assert_eq!(fixture.sync().await.changed, 1);

        let commission = fixture.all().await.remove(0);
        let history = fixture
            .affiliate
            .status_history(commission.id)
            .await
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].platform_status, "COMPLETE");
    });
}

#[test]
fn the_platforms_answer_is_kept_as_last_read() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.hotmart.reports(vec![sale(
            "HP1",
            CommissionStatus::Pending,
            "WAITING_PAYMENT",
        )]);
        fixture.sync().await;
        let commission = fixture.all().await.remove(0);
        assert_eq!(
            fixture.affiliate.last_reading(commission.id).await.unwrap(),
            Some(r#"{"transaction":"HP1","status":"WAITING_PAYMENT"}"#.into())
        );

        fixture
            .hotmart
            .reports(vec![sale("HP1", CommissionStatus::Approved, "APPROVED")]);
        fixture.sync().await;
        assert_eq!(
            fixture.affiliate.last_reading(commission.id).await.unwrap(),
            Some(r#"{"transaction":"HP1","status":"APPROVED"}"#.into())
        );
    });
}

#[test]
fn a_conversion_missing_from_a_later_answer_stays() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.hotmart.reports(vec![
            sale("HP1", CommissionStatus::Approved, "APPROVED"),
            sale("HP2", CommissionStatus::Pending, "WAITING_PAYMENT"),
        ]);
        fixture.sync().await;
        fixture.hotmart.reports(Vec::new());
        fixture.clock.advance(TimeDelta::days(200));
        fixture.sync().await;

        assert_eq!(fixture.all().await.len(), 2);
    });
}

#[test]
fn a_key_repeated_in_one_answer_counts_once_as_its_last_reading() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.hotmart.reports(vec![
            sale("HP1", CommissionStatus::Pending, "WAITING_PAYMENT"),
            sale("HP1", CommissionStatus::Approved, "APPROVED"),
        ]);
        assert_eq!(fixture.sync().await.added, 1);

        let commissions = fixture.all().await;
        assert_eq!(commissions.len(), 1);
        assert_eq!(commissions[0].status, CommissionStatus::Approved);
    });
}

#[test]
fn the_same_key_on_another_platform_is_another_conversion() {
    block_on(async {
        let fixture = Fixture::new().await;
        let kiwify = Source::new(AffiliatePlatform::Kiwify);
        fixture
            .hotmart
            .reports(vec![sale("123", CommissionStatus::Approved, "APPROVED")]);
        kiwify.reports(vec![sale("123", CommissionStatus::Approved, "paid")]);
        fixture.sync().await;
        fixture.affiliate.sync(&kiwify).await.unwrap();

        let platforms: Vec<AffiliatePlatform> =
            fixture.all().await.iter().map(|c| c.platform).collect();
        assert_eq!(platforms.len(), 2);
        assert!(platforms.contains(&AffiliatePlatform::Hotmart));
        assert!(platforms.contains(&AffiliatePlatform::Kiwify));
    });
}

#[test]
fn the_first_sync_reads_the_whole_history_and_the_next_ones_from_the_last_start_less_the_settle_time()
 {
    block_on(async {
        let fixture = Fixture::new().await;
        assert_eq!(
            fixture
                .affiliate
                .last_sync(AffiliatePlatform::Hotmart)
                .await
                .unwrap(),
            None
        );

        fixture.sync().await;
        assert_eq!(
            fixture.hotmart.last_window(),
            noon() - TimeDelta::days(HISTORY_DAYS)..noon()
        );
        let last = fixture
            .affiliate
            .last_sync(AffiliatePlatform::Hotmart)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(last.started_at, noon());

        // An hour later: back to the settle time before the last start.
        fixture.clock.advance(TimeDelta::hours(1));
        fixture.sync().await;
        let second = noon() + TimeDelta::hours(1);
        assert_eq!(
            fixture.hotmart.last_window(),
            noon() - TimeDelta::days(SETTLE_DAYS)..second
        );

        // A failed Sync moves nothing: the next one starts from the last
        // that went through.
        fixture.clock.advance(TimeDelta::days(10));
        fixture.hotmart.fails(true);
        assert!(matches!(
            fixture.affiliate.sync(&fixture.hotmart).await,
            Err(AffiliateError::Platform(PlatformError::RateLimited))
        ));
        fixture.hotmart.fails(false);
        fixture.clock.advance(TimeDelta::days(10));
        fixture.sync().await;
        let fourth = second + TimeDelta::days(20);
        assert_eq!(
            fixture.hotmart.last_window(),
            second - TimeDelta::days(SETTLE_DAYS)..fourth
        );

        // Away longer than the Platform keeps: as far back as it lets.
        fixture.clock.advance(TimeDelta::days(400));
        fixture.sync().await;
        let now = fourth + TimeDelta::days(400);
        assert_eq!(
            fixture.hotmart.last_window(),
            now - TimeDelta::days(HISTORY_DAYS)..now
        );
    });
}

#[test]
fn each_platform_keeps_its_own_sync_window() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.sync().await;
        fixture.clock.advance(TimeDelta::hours(1));
        let kiwify = Source::new(AffiliatePlatform::Kiwify);
        fixture.affiliate.sync(&kiwify).await.unwrap();

        let now = noon() + TimeDelta::hours(1);
        assert_eq!(
            kiwify.last_window(),
            now - TimeDelta::days(HISTORY_DAYS)..now
        );
    });
}

#[test]
fn a_failed_sync_keeps_nothing() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .hotmart
            .reports(vec![sale("HP1", CommissionStatus::Approved, "APPROVED")]);
        fixture.hotmart.fails(true);
        assert!(fixture.affiliate.sync(&fixture.hotmart).await.is_err());

        assert!(fixture.all().await.is_empty());
        assert_eq!(
            fixture
                .affiliate
                .last_sync(AffiliatePlatform::Hotmart)
                .await
                .unwrap(),
            None
        );
    });
}

#[test]
fn the_owner_records_edits_and_deletes_a_commission_by_hand() {
    block_on(async {
        let fixture = Fixture::new().await;
        let recorded = fixture
            .affiliate
            .record_manual(manual(
                "Fone Bluetooth",
                CommissionStatus::Pending,
                brl("12.40"),
            ))
            .await
            .unwrap();
        assert!(recorded.manual);
        assert_eq!(recorded.platform, AffiliatePlatform::MercadoLivre);
        assert_eq!(recorded.program_name, "Mercado Livre");
        assert_eq!(recorded.product, "Fone Bluetooth");
        assert_eq!(recorded.amount, brl("12.40"));
        assert_eq!(recorded.status, CommissionStatus::Pending);
        assert_eq!(recorded.platform_status, "");
        assert_eq!(
            fixture.affiliate.last_reading(recorded.id).await.unwrap(),
            None
        );

        fixture.clock.advance(TimeDelta::days(1));
        let edited = fixture
            .affiliate
            .edit_manual(
                recorded.id,
                ManualCommission {
                    platform: AffiliatePlatform::Magalu,
                    program: "Parceiro Magalu".into(),
                    ..manual("Fone Bluetooth JBL", CommissionStatus::Approved, brl("13"))
                },
            )
            .await
            .unwrap();
        assert_eq!(edited.id, recorded.id);
        assert_eq!(edited.platform, AffiliatePlatform::Magalu);
        assert_eq!(edited.program_name, "Parceiro Magalu");
        assert_eq!(edited.product, "Fone Bluetooth JBL");
        assert_eq!(edited.amount, brl("13"));
        assert_eq!(edited.status, CommissionStatus::Approved);
        assert_eq!(edited.status_since, noon() + TimeDelta::days(1));
        let history = fixture.affiliate.status_history(recorded.id).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(fixture.all().await, [edited]);

        fixture.affiliate.delete_manual(recorded.id).await.unwrap();
        assert!(fixture.all().await.is_empty());
        assert!(matches!(
            fixture.affiliate.commission(recorded.id).await,
            Err(AffiliateError::NotFound)
        ));
        assert!(matches!(
            fixture.affiliate.delete_manual(recorded.id).await,
            Err(AffiliateError::NotFound)
        ));
    });
}

#[test]
fn a_commission_from_a_platform_cannot_be_edited_or_deleted() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .hotmart
            .reports(vec![sale("HP1", CommissionStatus::Approved, "APPROVED")]);
        fixture.sync().await;
        let read = fixture.all().await.remove(0);

        assert!(matches!(
            fixture
                .affiliate
                .edit_manual(
                    read.id,
                    manual("Curso", CommissionStatus::Reversed, brl("1"))
                )
                .await,
            Err(AffiliateError::NotManual)
        ));
        assert!(matches!(
            fixture.affiliate.delete_manual(read.id).await,
            Err(AffiliateError::NotManual)
        ));
        assert_eq!(fixture.all().await, [read]);
    });
}

#[test]
fn a_platform_reading_never_touches_a_commission_recorded_by_hand() {
    block_on(async {
        let fixture = Fixture::new().await;
        let recorded = fixture
            .affiliate
            .record_manual(ManualCommission {
                platform: AffiliatePlatform::Hotmart,
                ..manual("Curso", CommissionStatus::Pending, brl("50"))
            })
            .await
            .unwrap();
        // Its natural key is the first id the app handed out.
        let key = mascate_kernel::RecordId::from_u128(1).to_string();
        fixture
            .hotmart
            .reports(vec![sale(&key, CommissionStatus::Reversed, "REFUNDED")]);

        assert_eq!(fixture.sync().await.unchanged, 1);
        assert_eq!(fixture.all().await, [recorded]);
    });
}

#[test]
fn a_manual_commission_needs_a_product_and_an_amount_not_below_zero() {
    block_on(async {
        let fixture = Fixture::new().await;
        assert!(matches!(
            fixture
                .affiliate
                .record_manual(manual("  ", CommissionStatus::Pending, brl("1")))
                .await,
            Err(AffiliateError::NoProduct)
        ));
        assert!(matches!(
            fixture
                .affiliate
                .record_manual(manual("Fone", CommissionStatus::Pending, brl("-1")))
                .await,
            Err(AffiliateError::Negative)
        ));
        let free = fixture
            .affiliate
            .record_manual(manual("Fone", CommissionStatus::Cancelled, brl("0")))
            .await
            .unwrap();
        assert_eq!(free.amount, brl("0"));
        assert_eq!(fixture.all().await.len(), 1);
    });
}

#[test]
fn programs_typed_with_the_same_letters_are_one_program() {
    block_on(async {
        let fixture = Fixture::new().await;
        let first = fixture
            .affiliate
            .record_manual(ManualCommission {
                program: "Natura Consultoria".into(),
                platform: AffiliatePlatform::Other,
                ..manual("Perfume", CommissionStatus::Approved, brl("20"))
            })
            .await
            .unwrap();
        let second = fixture
            .affiliate
            .record_manual(ManualCommission {
                program: " natura  consultoria ".into(),
                platform: AffiliatePlatform::Other,
                ..manual("Hidratante", CommissionStatus::Approved, brl("8"))
            })
            .await
            .unwrap();

        assert_eq!(first.program, second.program);
        assert_eq!(second.program_name, "Natura Consultoria");
    });
}

#[test]
fn commissions_are_listed_by_status_and_period_the_latest_sale_first() {
    block_on(async {
        let fixture = Fixture::new().await;
        let at = |days_ago: i64| noon() - TimeDelta::days(days_ago);
        for (product, status, days_ago) in [
            ("Antigo", CommissionStatus::Paid, 40),
            ("Estornado", CommissionStatus::Reversed, 10),
            ("Recente", CommissionStatus::Approved, 1),
            ("Boleto", CommissionStatus::Cancelled, 3),
        ] {
            fixture
                .affiliate
                .record_manual(ManualCommission {
                    sold_at: at(days_ago),
                    ..manual(product, status, brl("10"))
                })
                .await
                .unwrap();
        }
        let products = |listed: Vec<mascate_affiliate::Commission>| -> Vec<String> {
            listed.into_iter().map(|c| c.product).collect()
        };

        assert_eq!(
            products(fixture.all().await),
            ["Recente", "Boleto", "Estornado", "Antigo"]
        );
        let last_month = CommissionFilter {
            statuses: Vec::new(),
            sold: at(30)..noon(),
        };
        assert_eq!(
            products(fixture.affiliate.commissions(&last_month).await.unwrap()),
            ["Recente", "Boleto", "Estornado"]
        );
        let reversed_or_paid = CommissionFilter {
            statuses: vec![CommissionStatus::Reversed, CommissionStatus::Paid],
            ..CommissionFilter::default()
        };
        assert_eq!(
            products(
                fixture
                    .affiliate
                    .commissions(&reversed_or_paid)
                    .await
                    .unwrap()
            ),
            ["Estornado", "Antigo"]
        );
    });
}

#[test]
fn money_keeps_its_currency_from_the_platform_to_the_list() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.hotmart.reports(vec![ReportedConversion {
            commission: usd("19.995"),
            ..sale("HP1", CommissionStatus::Approved, "APPROVED")
        }]);
        fixture.sync().await;
        fixture.sync().await;

        let commission = fixture.all().await.remove(0);
        assert_eq!(commission.amount, usd("19.995"));
        assert_eq!(commission.amount.currency(), Currency::Usd);
    });
}

#[test]
fn the_income_reminder_shows_only_while_a_commission_earned_in_the_last_year_exists() {
    block_on(async {
        let fixture = Fixture::new().await;
        let raised = || async { fixture.affiliate.raised_reminders().await.unwrap() };
        assert!(raised().await.is_empty());

        for status in [CommissionStatus::Pending, CommissionStatus::Reversed] {
            fixture
                .affiliate
                .record_manual(manual("Fone", status, brl("10")))
                .await
                .unwrap();
        }
        assert!(raised().await.is_empty());

        let approved = fixture
            .affiliate
            .record_manual(manual("Fone", CommissionStatus::Approved, brl("10")))
            .await
            .unwrap();
        assert_eq!(raised().await, [COMMISSION_INCOME.key]);

        // A year after the sale, it no longer counts.
        fixture
            .clock
            .advance(TimeDelta::days(INCOME_WINDOW_DAYS) - TimeDelta::days(2));
        assert_eq!(raised().await, [COMMISSION_INCOME.key]);
        fixture.clock.advance(TimeDelta::days(1));
        assert!(raised().await.is_empty());

        // A paid one counts as well, and a deleted one does not.
        fixture.affiliate.delete_manual(approved.id).await.unwrap();
        let paid = fixture
            .affiliate
            .record_manual(ManualCommission {
                sold_at: noon() + TimeDelta::days(INCOME_WINDOW_DAYS - 1),
                ..manual("Fone", CommissionStatus::Paid, brl("10"))
            })
            .await
            .unwrap();
        assert_eq!(raised().await, [COMMISSION_INCOME.key]);
        fixture.affiliate.delete_manual(paid.id).await.unwrap();
        assert!(raised().await.is_empty());
    });
}
