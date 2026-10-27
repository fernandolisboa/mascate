//! Programs added before their first sale, through the Affiliate module's
//! public interface with a real temporary database: a provisional Program
//! takes rules and links like any other, joins the Program a first sale
//! brings only when the owner confirms, and a Platform with one Program
//! never gets a second.

use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliatePlatform, CommissionFilter, CommissionStatus, LinkTagger,
    MIGRATIONS, ManualCommission, ManualPayout, NewAffiliateLink, NewProgram, Practice, Program,
    Promotion, PromotionRules, ReportedConversion, ReportedProgram, Standing, TagRefusal,
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

const HOTLINK: &str = "https://go.hotmart.com/X123?ap=abcd";

/// Tags a link with `src`, as Hotmart's links take it.
struct HotmartTagger;

impl LinkTagger for HotmartTagger {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Hotmart
    }

    fn tag(&self, link: &str, tag: &str) -> Result<String, TagRefusal> {
        Ok(format!("{link}&src={tag}"))
    }
}

/// A Hotmart sale of the product with Hotmart's id `product_id`.
fn hotmart_sale(key: &str, product_id: &str, product: &str) -> ReportedConversion {
    ReportedConversion {
        key: key.into(),
        program: ReportedProgram {
            id: product_id.into(),
            name: product.into(),
        },
        product: product.into(),
        sold_at: noon() - TimeDelta::hours(2),
        commission: brl("97.00"),
        status: CommissionStatus::Approved,
        platform_status: "APPROVED".into(),
        status_changed_at: None,
        tracking: Some("telegram".into()),
        raw: format!(r#"{{"transaction":"{key}"}}"#),
    }
}

fn excel() -> NewProgram {
    NewProgram {
        platform: AffiliatePlatform::Hotmart,
        name: "Excel do Zero".into(),
        product: "Curso Excel do Zero ao Avançado".into(),
    }
}

fn channel() -> RecordId {
    RecordId::from_u128(0xC1)
}

fn link_of(program: RecordId) -> NewAffiliateLink {
    NewAffiliateLink {
        program,
        product: excel().product,
        pasted: HOTLINK.into(),
        channel: channel(),
        tag: "telegram".into(),
        promotion: Promotion::default(),
    }
}

fn own_rules() -> PromotionRules {
    PromotionRules::defaults(AffiliatePlatform::Hotmart)
        .with(Practice::BoostedPosts, Standing::Allowed)
        .with(Practice::PaidSearch, Standing::Forbidden)
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
        Self {
            _dir: dir,
            clock,
            affiliate,
        }
    }

    /// The Excel course added by hand, with its own rules and link.
    async fn added_with_link(&self) -> Program {
        let added = self.affiliate.add_program(&excel()).await.unwrap();
        self.affiliate
            .set_promotion_rules(added.id, &own_rules())
            .await
            .unwrap();
        self.affiliate
            .create_link(&link_of(added.id), &HotmartTagger)
            .await
            .unwrap();
        self.clock.advance(TimeDelta::hours(1));
        added
    }

    async fn programs_on(&self, platform: AffiliatePlatform) -> Vec<Program> {
        self.affiliate
            .programs()
            .await
            .unwrap()
            .into_iter()
            .filter(|program| program.platform == platform)
            .collect()
    }
}

#[test]
fn a_program_added_by_hand_takes_rules_and_links_before_any_sale() {
    block_on(async {
        let f = Fixture::new().await;

        let added = f.added_with_link().await;

        let program = f.affiliate.program(added.id).await.unwrap();
        assert!(program.provisional);
        assert_eq!(program.name, "Excel do Zero");
        assert_eq!(program.product.as_deref(), Some(excel().product.as_str()));
        assert_eq!(program.rules, own_rules());
        let links = f.affiliate.links().await.unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].program.id, added.id);
        assert_eq!(links[0].tagged, format!("{HOTLINK}&src=telegram"));
        // Nothing sold yet: nothing to join.
        assert!(f.affiliate.joins().await.unwrap().is_empty());
    });
}

#[test]
fn adding_the_same_product_again_gives_back_the_program_added_before() {
    block_on(async {
        let f = Fixture::new().await;
        let added = f.affiliate.add_program(&excel()).await.unwrap();

        let again = f
            .affiliate
            .add_program(&NewProgram {
                name: String::new(),
                product: "  curso excel do zero ao AVANÇADO ".into(),
                ..excel()
            })
            .await
            .unwrap();

        assert_eq!(again, added);
        assert_eq!(f.programs_on(AffiliatePlatform::Hotmart).await.len(), 1);
    });
}

#[test]
fn a_program_without_a_name_is_named_after_its_product_and_one_without_a_product_is_refused() {
    block_on(async {
        let f = Fixture::new().await;

        let added = f
            .affiliate
            .add_program(&NewProgram {
                name: " ".into(),
                ..excel()
            })
            .await
            .unwrap();
        let refused = f
            .affiliate
            .add_program(&NewProgram {
                product: "  ".into(),
                ..excel()
            })
            .await;

        assert_eq!(added.name, excel().product);
        assert!(matches!(refused, Err(AffiliateError::NoProduct)));
    });
}

#[test]
fn the_first_sale_of_an_unknown_product_is_offered_as_a_join_and_never_joined_alone() {
    block_on(async {
        let f = Fixture::new().await;
        let added = f.added_with_link().await;

        f.affiliate
            .import(
                AffiliatePlatform::Hotmart,
                &[hotmart_sale("HP1", "4410", "Excel Completo 2026")],
            )
            .await
            .unwrap();

        let joins = f.affiliate.joins().await.unwrap();
        assert_eq!(joins.len(), 1);
        assert_eq!(joins[0].provisional.id, added.id);
        assert_eq!(joins[0].sold.name, "Excel Completo 2026");
        assert_eq!(joins[0].product, "Excel Completo 2026");
        assert_eq!(joins[0].first_sold_at, noon() - TimeDelta::hours(2));
        // Until the owner answers, the sale stays on its own Program.
        let commissions = f
            .affiliate
            .commissions(&CommissionFilter::default())
            .await
            .unwrap();
        assert_eq!(commissions[0].program, joins[0].sold.id);
        assert_eq!(f.programs_on(AffiliatePlatform::Hotmart).await.len(), 2);
    });
}

#[test]
fn confirming_a_join_moves_every_sale_and_keeps_the_links_and_rules() {
    block_on(async {
        let f = Fixture::new().await;
        let added = f.added_with_link().await;
        let sales = [
            hotmart_sale("HP1", "4410", "Excel Completo 2026"),
            ReportedConversion {
                sold_at: noon() - TimeDelta::hours(1),
                status: CommissionStatus::Pending,
                ..hotmart_sale("HP2", "4410", "Excel Completo 2026")
            },
        ];
        f.affiliate
            .import(AffiliatePlatform::Hotmart, &sales)
            .await
            .unwrap();
        let first = f
            .affiliate
            .commissions(&CommissionFilter::default())
            .await
            .unwrap()
            .into_iter()
            .find(|commission| commission.status == CommissionStatus::Approved)
            .unwrap();
        let payout = f
            .affiliate
            .record_payout(
                ManualPayout {
                    platform: AffiliatePlatform::Hotmart,
                    paid_at: noon(),
                    amount: brl("97.00"),
                },
                &[first.id],
            )
            .await
            .unwrap();
        let sold = f.affiliate.joins().await.unwrap()[0].sold.id;

        let joined = f.affiliate.confirm_join(added.id, sold).await.unwrap();

        assert_eq!(joined.id, added.id);
        assert!(!joined.provisional);
        assert_eq!(joined.name, "Excel do Zero");
        assert_eq!(joined.rules, own_rules());
        let hotmart = f.programs_on(AffiliatePlatform::Hotmart).await;
        assert_eq!(hotmart, vec![joined]);
        let commissions = f
            .affiliate
            .commissions(&CommissionFilter::default())
            .await
            .unwrap();
        assert_eq!(commissions.len(), 2);
        for commission in &commissions {
            assert_eq!(commission.program, added.id);
            assert_eq!(commission.program_name, "Excel do Zero");
            // The link the owner made before the sale carries its marking.
            assert_eq!(commission.channel, Some(channel()));
        }
        let paid = commissions
            .iter()
            .find(|commission| commission.id == first.id)
            .unwrap();
        assert_eq!(paid.status, CommissionStatus::Paid);
        assert_eq!(paid.payout, Some(payout.id));
        let links = f.affiliate.links().await.unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].program.id, added.id);
        assert!(f.affiliate.joins().await.unwrap().is_empty());

        // The next Sync reads the same sales into the joined Program.
        f.clock.advance(TimeDelta::hours(1));
        let again = f
            .affiliate
            .import(AffiliatePlatform::Hotmart, &sales)
            .await
            .unwrap();

        assert_eq!((again.added, again.changed, again.unchanged), (0, 0, 2));
        assert_eq!(
            f.affiliate
                .commissions(&CommissionFilter::default())
                .await
                .unwrap(),
            commissions
        );
        assert_eq!(f.programs_on(AffiliatePlatform::Hotmart).await.len(), 1);
    });
}

#[test]
fn the_sales_links_and_product_links_join_the_added_program_unless_it_has_its_own() {
    block_on(async {
        let f = Fixture::new().await;
        let added = f.added_with_link().await;
        f.affiliate
            .import(
                AffiliatePlatform::Hotmart,
                &[hotmart_sale("HP1", "4410", "Excel Completo 2026")],
            )
            .await
            .unwrap();
        let sold = f.affiliate.joins().await.unwrap()[0].sold.id;
        // Made on the sale's Program while the owner had not answered: one
        // for the same product and channel as the added Program's, one for
        // another channel.
        let other_channel = RecordId::from_u128(0xC2);
        f.affiliate
            .create_link(
                &NewAffiliateLink {
                    pasted: format!("{HOTLINK}&sold=1"),
                    ..link_of(sold)
                },
                &HotmartTagger,
            )
            .await
            .unwrap();
        f.affiliate
            .create_link(
                &NewAffiliateLink {
                    channel: other_channel,
                    tag: "instagram".into(),
                    ..link_of(sold)
                },
                &HotmartTagger,
            )
            .await
            .unwrap();
        let product = RecordId::from_u128(0xD1);
        f.affiliate
            .link_product(sold, "Excel Completo 2026", product)
            .await
            .unwrap();

        f.affiliate.confirm_join(added.id, sold).await.unwrap();

        let links = f.affiliate.links().await.unwrap();
        assert_eq!(links.len(), 2);
        assert!(links.iter().all(|link| link.program.id == added.id));
        assert!(links.iter().all(|link| link.pasted == HOTLINK));
        assert!(links.iter().any(|link| link.channel == other_channel));
        assert_eq!(
            f.affiliate
                .linked_product(added.id, "Excel Completo 2026")
                .await
                .unwrap(),
            Some(product)
        );
    });
}

#[test]
fn declining_keeps_both_programs_and_never_asks_the_same_pair_again() {
    block_on(async {
        let f = Fixture::new().await;
        let added = f.added_with_link().await;
        f.affiliate
            .import(
                AffiliatePlatform::Hotmart,
                &[hotmart_sale("HP1", "4410", "Planilhas Financeiras")],
            )
            .await
            .unwrap();
        let sold = f.affiliate.joins().await.unwrap()[0].sold.id;

        f.affiliate.decline_join(added.id, sold).await.unwrap();

        assert!(f.affiliate.joins().await.unwrap().is_empty());
        assert_eq!(f.programs_on(AffiliatePlatform::Hotmart).await.len(), 2);
        assert!(f.affiliate.program(added.id).await.unwrap().provisional);
        // Another sale of the declined product asks nothing.
        f.affiliate
            .import(
                AffiliatePlatform::Hotmart,
                &[hotmart_sale("HP2", "4410", "Planilhas Financeiras")],
            )
            .await
            .unwrap();
        assert!(f.affiliate.joins().await.unwrap().is_empty());
        // Answered once, the pair takes no other answer.
        assert!(matches!(
            f.affiliate.confirm_join(added.id, sold).await,
            Err(AffiliateError::NotJoinable)
        ));
        assert!(matches!(
            f.affiliate.decline_join(added.id, sold).await,
            Err(AffiliateError::NotJoinable)
        ));

        // The first sale of yet another product is asked about.
        f.clock.advance(TimeDelta::hours(1));
        f.affiliate
            .import(
                AffiliatePlatform::Hotmart,
                &[hotmart_sale("HP3", "4411", "Excel Completo 2026")],
            )
            .await
            .unwrap();
        let joins = f.affiliate.joins().await.unwrap();
        assert_eq!(joins.len(), 1);
        assert_eq!(joins[0].product, "Excel Completo 2026");
    });
}

#[test]
fn with_two_added_programs_the_owner_picks_which_one_a_sale_joins() {
    block_on(async {
        let f = Fixture::new().await;
        let excel = f.affiliate.add_program(&excel()).await.unwrap();
        let word = f
            .affiliate
            .add_program(&NewProgram {
                platform: AffiliatePlatform::Hotmart,
                name: "Word".into(),
                product: "Curso de Word".into(),
            })
            .await
            .unwrap();
        f.clock.advance(TimeDelta::hours(1));
        f.affiliate
            .import(
                AffiliatePlatform::Hotmart,
                &[hotmart_sale("HP1", "4410", "Word Essencial")],
            )
            .await
            .unwrap();

        let joins = f.affiliate.joins().await.unwrap();
        let offered: Vec<RecordId> = joins.iter().map(|join| join.provisional.id).collect();
        assert_eq!(offered.len(), 2);
        assert!(offered.contains(&excel.id) && offered.contains(&word.id));
        // Nothing moved until the owner picks.
        assert!(f.affiliate.program(excel.id).await.unwrap().provisional);
        assert!(f.affiliate.program(word.id).await.unwrap().provisional);

        f.affiliate
            .confirm_join(word.id, joins[0].sold.id)
            .await
            .unwrap();

        assert!(f.affiliate.joins().await.unwrap().is_empty());
        assert!(!f.affiliate.program(word.id).await.unwrap().provisional);
        assert!(f.affiliate.program(excel.id).await.unwrap().provisional);
    });
}

#[test]
fn a_commission_by_hand_of_an_unknown_product_is_offered_too() {
    block_on(async {
        let f = Fixture::new().await;
        let added = f.added_with_link().await;

        let recorded = f
            .affiliate
            .record_manual(ManualCommission {
                platform: AffiliatePlatform::Hotmart,
                program: String::new(),
                product: "Excel Completo".into(),
                sold_at: noon(),
                amount: brl("50.00"),
                status: CommissionStatus::Pending,
            })
            .await
            .unwrap();
        let joins = f.affiliate.joins().await.unwrap();
        assert_eq!(joins.len(), 1);

        f.affiliate
            .confirm_join(added.id, joins[0].sold.id)
            .await
            .unwrap();

        let commission = f.affiliate.commission(recorded.id).await.unwrap();
        assert_eq!(commission.program, added.id);
        assert!(commission.manual);
        // The next Commission by hand of the Platform lands there too.
        let next = f
            .affiliate
            .record_manual(ManualCommission {
                platform: AffiliatePlatform::Hotmart,
                program: String::new(),
                product: "Excel Completo".into(),
                sold_at: noon() + TimeDelta::hours(1),
                amount: brl("50.00"),
                status: CommissionStatus::Pending,
            })
            .await
            .unwrap();
        assert_eq!(next.program, added.id);
        assert!(f.affiliate.joins().await.unwrap().is_empty());
    });
}

#[test]
fn a_program_sold_before_the_owner_added_one_is_never_offered() {
    block_on(async {
        let f = Fixture::new().await;
        f.affiliate
            .import(
                AffiliatePlatform::Hotmart,
                &[hotmart_sale("HP1", "4410", "Planilhas Financeiras")],
            )
            .await
            .unwrap();
        f.clock.advance(TimeDelta::hours(1));

        f.affiliate.add_program(&excel()).await.unwrap();

        assert!(f.affiliate.joins().await.unwrap().is_empty());
        // A sale on another Platform is never offered either.
        f.clock.advance(TimeDelta::hours(1));
        f.affiliate
            .import(
                AffiliatePlatform::Kiwify,
                &[hotmart_sale("KW1", "kw-1", "Excel na Kiwify")],
            )
            .await
            .unwrap();
        assert!(f.affiliate.joins().await.unwrap().is_empty());
    });
}

#[test]
fn a_platform_with_one_program_never_gets_a_second_one() {
    block_on(async {
        let f = Fixture::new().await;
        for platform in [AffiliatePlatform::Shopee, AffiliatePlatform::Amazon] {
            let own = platform.own_program().unwrap();

            let first = f
                .affiliate
                .add_program(&NewProgram {
                    platform,
                    name: "Fone".into(),
                    product: "Fone Bluetooth".into(),
                })
                .await
                .unwrap();
            let second = f
                .affiliate
                .add_program(&NewProgram {
                    platform,
                    name: "Capa".into(),
                    product: "Capa de celular".into(),
                })
                .await
                .unwrap();
            // The first sale lands on the same Program.
            f.affiliate
                .import(
                    platform,
                    &[ReportedConversion {
                        program: own.clone(),
                        ..hotmart_sale(platform.name(), "", "Fone Bluetooth")
                    }],
                )
                .await
                .unwrap();

            assert_eq!(first, second);
            assert!(!first.provisional);
            assert_eq!(first.name, own.name);
            assert_eq!(f.programs_on(platform).await, vec![first]);
        }
        assert!(f.affiliate.joins().await.unwrap().is_empty());
    });
}

#[test]
fn on_a_platform_with_one_program_the_one_a_commission_by_hand_made_is_given_back() {
    block_on(async {
        let f = Fixture::new().await;
        let recorded = f
            .affiliate
            .record_manual(ManualCommission {
                platform: AffiliatePlatform::Shopee,
                program: String::new(),
                product: "Fone Bluetooth".into(),
                sold_at: noon(),
                amount: brl("4.20"),
                status: CommissionStatus::Approved,
            })
            .await
            .unwrap();

        let added = f
            .affiliate
            .add_program(&NewProgram {
                platform: AffiliatePlatform::Shopee,
                name: String::new(),
                product: "Capa de celular".into(),
            })
            .await
            .unwrap();

        assert_eq!(added.id, recorded.program);
        assert_eq!(f.programs_on(AffiliatePlatform::Shopee).await.len(), 1);
    });
}
