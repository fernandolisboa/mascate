//! A Program's products linked to Products of the Catalog, through the
//! Affiliate module's public interface with a real temporary database: one
//! Product per product at most, kept by id only, on every Commission of
//! that product and nowhere else.

use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliatePlatform, Commission, CommissionFilter, CommissionStatus,
    MIGRATIONS, ManualCommission, PromotedProduct, ReportedConversion, ReportedProgram,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, RecordId, Timestamp};
use mascate_platform::{Database, migrate};
use rust_decimal::Decimal;

fn noon() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 13, 12, 0, 0).unwrap()
}

fn brl(amount: &str) -> Money {
    Money::new(amount.parse::<Decimal>().unwrap(), Currency::Brl)
}

/// A Shopee sale of `product`: one Program for the whole Platform.
fn shopee_sale(key: &str, product: &str) -> ReportedConversion {
    ReportedConversion {
        key: key.into(),
        program: ReportedProgram {
            id: "shopee".into(),
            name: "Shopee Afiliados".into(),
        },
        product: product.into(),
        sold_at: noon() - TimeDelta::days(3),
        commission: brl("4.20"),
        status: CommissionStatus::Approved,
        platform_status: "COMPLETED".into(),
        status_changed_at: None,
        tracking: None,
        raw: format!(r#"{{"conversionId":"{key}"}}"#),
    }
}

fn fone() -> RecordId {
    RecordId::from_u128(0xF0)
}

fn capa() -> RecordId {
    RecordId::from_u128(0xCA)
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    affiliate: Affiliate,
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
        affiliate
            .import(
                AffiliatePlatform::Shopee,
                &[
                    shopee_sale("S1", "Fone Bluetooth TWS"),
                    shopee_sale("S2", "Fone Bluetooth TWS "),
                    shopee_sale("S3", "Capa de celular"),
                ],
            )
            .await
            .unwrap();
        Self {
            _dir: dir,
            clock,
            affiliate,
        }
    }

    async fn promoted(&self) -> Vec<PromotedProduct> {
        self.affiliate.promoted_products().await.unwrap()
    }

    async fn promoted_one(&self, product: &str) -> PromotedProduct {
        self.promoted()
            .await
            .into_iter()
            .find(|promoted| promoted.product == product)
            .unwrap()
    }

    async fn commissions(&self) -> Vec<Commission> {
        self.affiliate
            .commissions(&CommissionFilter::default())
            .await
            .unwrap()
    }

    async fn linked_to(&self, product: &str) -> Vec<Option<RecordId>> {
        self.commissions()
            .await
            .into_iter()
            .filter(|commission| commission.product.trim() == product)
            .map(|commission| commission.linked_product)
            .collect()
    }
}

#[test]
fn each_promoted_product_is_listed_once_per_program_with_its_commissions() {
    block_on(async {
        let fixture = Fixture::new().await;

        let promoted = fixture.promoted().await;

        let listed: Vec<(&str, &str, usize, Option<RecordId>)> = promoted
            .iter()
            .map(|p| {
                (
                    p.program.name.as_str(),
                    p.product.as_str(),
                    p.commissions,
                    p.linked_to,
                )
            })
            .collect();
        assert_eq!(
            listed,
            [
                ("Shopee Afiliados", "Capa de celular", 1, None),
                ("Shopee Afiliados", "Fone Bluetooth TWS", 2, None),
            ]
        );
        assert_eq!(promoted[0].program.platform, AffiliatePlatform::Shopee);
    });
}

#[test]
fn a_linked_product_carries_the_product_id_on_each_of_its_commissions() {
    block_on(async {
        let fixture = Fixture::new().await;
        let fone_program = fixture.promoted_one("Fone Bluetooth TWS").await.program;

        fixture
            .affiliate
            .link_product(fone_program.id, "Fone Bluetooth TWS", fone())
            .await
            .unwrap();

        assert_eq!(
            fixture.promoted_one("Fone Bluetooth TWS").await.linked_to,
            Some(fone())
        );
        assert_eq!(
            fixture.linked_to("Fone Bluetooth TWS").await,
            [Some(fone()), Some(fone())]
        );
        assert_eq!(fixture.linked_to("Capa de celular").await, [None]);
    });
}

#[test]
fn linking_again_moves_the_product_to_the_new_product_never_to_two() {
    block_on(async {
        let fixture = Fixture::new().await;
        let program = fixture.promoted_one("Fone Bluetooth TWS").await.program.id;
        fixture
            .affiliate
            .link_product(program, "Fone Bluetooth TWS", capa())
            .await
            .unwrap();
        fixture.clock.advance(TimeDelta::minutes(1));

        fixture
            .affiliate
            .link_product(program, " Fone Bluetooth TWS ", fone())
            .await
            .unwrap();
        fixture
            .affiliate
            .link_product(program, "Fone Bluetooth TWS", fone())
            .await
            .unwrap();

        assert_eq!(
            fixture.linked_to("Fone Bluetooth TWS").await,
            [Some(fone()), Some(fone())]
        );
        assert_eq!(fixture.promoted().await.len(), 2);
    });
}

#[test]
fn unlinking_takes_the_product_off_and_unlinking_again_changes_nothing() {
    block_on(async {
        let fixture = Fixture::new().await;
        let program = fixture.promoted_one("Fone Bluetooth TWS").await.program.id;
        fixture
            .affiliate
            .link_product(program, "Fone Bluetooth TWS", fone())
            .await
            .unwrap();

        fixture
            .affiliate
            .unlink_product(program, "Fone Bluetooth TWS")
            .await
            .unwrap();
        fixture
            .affiliate
            .unlink_product(program, "Fone Bluetooth TWS")
            .await
            .unwrap();

        assert_eq!(fixture.linked_to("Fone Bluetooth TWS").await, [None, None]);
        assert_eq!(
            fixture.promoted_one("Fone Bluetooth TWS").await.linked_to,
            None
        );

        fixture
            .affiliate
            .link_product(program, "Fone Bluetooth TWS", capa())
            .await
            .unwrap();
        assert_eq!(
            fixture.promoted_one("Fone Bluetooth TWS").await.linked_to,
            Some(capa())
        );
    });
}

#[test]
fn several_products_may_link_to_the_same_product() {
    block_on(async {
        let fixture = Fixture::new().await;
        let program = fixture.promoted_one("Fone Bluetooth TWS").await.program.id;
        let manual = fixture
            .affiliate
            .record_manual(ManualCommission {
                platform: AffiliatePlatform::MercadoLivre,
                program: String::new(),
                product: "Fone Bluetooth".into(),
                sold_at: noon() - TimeDelta::days(1),
                amount: brl("8.00"),
                status: CommissionStatus::Approved,
            })
            .await
            .unwrap();

        fixture
            .affiliate
            .link_product(program, "Fone Bluetooth TWS", fone())
            .await
            .unwrap();
        fixture
            .affiliate
            .link_product(manual.program, "Fone Bluetooth", fone())
            .await
            .unwrap();

        let linked: Vec<RecordId> = fixture
            .commissions()
            .await
            .into_iter()
            .filter_map(|commission| commission.linked_product)
            .collect();
        assert_eq!(linked, [fone(), fone(), fone()]);
        assert_eq!(
            fixture
                .affiliate
                .commission(manual.id)
                .await
                .unwrap()
                .linked_product,
            Some(fone())
        );
    });
}

#[test]
fn a_link_follows_new_commissions_of_the_same_product() {
    block_on(async {
        let fixture = Fixture::new().await;
        let program = fixture.promoted_one("Capa de celular").await.program.id;
        fixture
            .affiliate
            .link_product(program, "Capa de celular", capa())
            .await
            .unwrap();

        fixture
            .affiliate
            .import(
                AffiliatePlatform::Shopee,
                &[shopee_sale("S4", "Capa de celular")],
            )
            .await
            .unwrap();

        assert_eq!(
            fixture.linked_to("Capa de celular").await,
            [Some(capa()), Some(capa())]
        );
    });
}

#[test]
fn a_blank_product_or_an_unknown_program_is_refused() {
    block_on(async {
        let fixture = Fixture::new().await;
        let program = fixture.promoted_one("Capa de celular").await.program.id;

        assert!(matches!(
            fixture.affiliate.link_product(program, "  ", capa()).await,
            Err(AffiliateError::NoProduct)
        ));
        assert!(matches!(
            fixture
                .affiliate
                .link_product(RecordId::from_u128(0xDEAD_BEEF), "Capa de celular", capa())
                .await,
            Err(AffiliateError::NoSuchProgram)
        ));
        assert!(
            fixture
                .promoted()
                .await
                .iter()
                .all(|promoted| promoted.linked_to.is_none())
        );
    });
}
