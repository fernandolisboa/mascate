//! Affiliate Links through the Affiliate module's public interface with a
//! real temporary database: one link per Program, product and Promotion
//! Channel, a marking that belongs to one channel per Platform, the rules
//! checked before tagging, and the adapter's refusals passed on.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliatePlatform, CommissionStatus, FLAGS, LinkTagger, MIGRATIONS,
    NewAffiliateLink, Practice, Promotion, Reach, ReportedConversion, ReportedProgram, RuleWarning,
    SHOPEE_PAID_SEARCH, Standing, TagRefusal,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, RecordId, Timestamp};
use mascate_platform::{Database, Flag, Flags, Registry, migrate};
use rust_decimal::Decimal;

fn noon() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 13, 12, 0, 0).unwrap()
}

const SHOP: &str = "https://shop.example/p/123";

/// Tags links of `shop.example` with `t`, counting each call.
struct FakeTagger {
    platform: AffiliatePlatform,
    calls: AtomicUsize,
}

impl FakeTagger {
    fn of(platform: AffiliatePlatform) -> Self {
        Self {
            platform,
            calls: AtomicUsize::new(0),
        }
    }
}

impl LinkTagger for FakeTagger {
    fn platform(&self) -> AffiliatePlatform {
        self.platform
    }

    fn tag(&self, link: &str, tag: &str) -> Result<String, TagRefusal> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !link.starts_with("https://shop.example/") {
            return Err(TagRefusal::ForeignDomain(self.platform));
        }
        Ok(format!("{link}?t={tag}"))
    }
}

fn sale(platform: AffiliatePlatform) -> ReportedConversion {
    ReportedConversion {
        key: platform.name().into(),
        program: ReportedProgram {
            id: platform.name().into(),
            name: format!("Programa {}", platform.name()),
        },
        product: "Curso de Fotografia".into(),
        sold_at: noon() - TimeDelta::days(3),
        commission: Money::new(Decimal::new(4710, 2), Currency::Brl),
        status: CommissionStatus::Pending,
        platform_status: "WAITING".into(),
        status_changed_at: None,
        tracking: None,
        raw: "{}".into(),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    affiliate: Affiliate,
    flags: Flags,
}

impl Fixture {
    /// One Program on each Platform.
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(noon()));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(
            &database,
            clock.as_ref(),
            &[mascate_platform::MIGRATIONS, MIGRATIONS],
        )
        .await
        .unwrap();
        let ids = Arc::new(SequentialIds::default());
        let affiliate = Affiliate::new(database.clone(), clock.clone(), ids.clone());
        let flags = Flags::new(
            database,
            Registry::new(&[FLAGS]).unwrap(),
            clock.clone(),
            ids,
        );
        for platform in [
            AffiliatePlatform::Hotmart,
            AffiliatePlatform::Kiwify,
            AffiliatePlatform::Shopee,
            AffiliatePlatform::Amazon,
        ] {
            affiliate.import(platform, &[sale(platform)]).await.unwrap();
        }
        Self {
            _dir: dir,
            clock,
            affiliate,
            flags,
        }
    }

    async fn program(&self, platform: AffiliatePlatform) -> RecordId {
        self.affiliate
            .programs()
            .await
            .unwrap()
            .into_iter()
            .find(|program| program.platform == platform)
            .unwrap()
            .id
    }

    async fn turn_on(&self, flag: Flag) {
        self.flags
            .turn_on(
                self.flags
                    .request_turn_on(flag.key)
                    .unwrap()
                    .confirm("fernando"),
            )
            .await
            .unwrap();
        self.clock.advance(TimeDelta::seconds(1));
    }
}

fn channel(n: u128) -> RecordId {
    RecordId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n)
}

fn link(program: RecordId, channel: RecordId, tag: &str) -> NewAffiliateLink {
    NewAffiliateLink {
        program,
        product: "Curso de Fotografia".into(),
        pasted: SHOP.into(),
        channel,
        tag: tag.into(),
        promotion: Promotion::default(),
    }
}

#[test]
fn a_link_is_tagged_by_the_adapter_and_listed_with_its_program() {
    block_on(async {
        let f = Fixture::new().await;
        let hotmart = f.program(AffiliatePlatform::Hotmart).await;
        let tagger = FakeTagger::of(AffiliatePlatform::Hotmart);

        let created = f
            .affiliate
            .create_link(
                &NewAffiliateLink {
                    pasted: format!("  {SHOP}  "),
                    product: " Curso de Fotografia ".into(),
                    tag: " telegram ".into(),
                    ..link(hotmart, channel(1), "")
                },
                &tagger,
            )
            .await
            .unwrap();

        assert!(!created.existed);
        let made = &created.link;
        assert_eq!(made.program.id, hotmart);
        assert_eq!(made.program.platform, AffiliatePlatform::Hotmart);
        assert_eq!(made.program.name, "Programa Hotmart");
        assert_eq!(made.product, "Curso de Fotografia");
        assert_eq!(made.channel, channel(1));
        assert_eq!(made.tag, "telegram");
        assert_eq!(made.pasted, SHOP);
        assert_eq!(made.tagged, format!("{SHOP}?t=telegram"));
        assert_eq!(made.created_at, noon());
        assert_eq!(f.affiliate.links().await.unwrap(), vec![made.clone()]);
    });
}

#[test]
fn creating_the_same_link_twice_yields_one() {
    block_on(async {
        let f = Fixture::new().await;
        let hotmart = f.program(AffiliatePlatform::Hotmart).await;
        let tagger = FakeTagger::of(AffiliatePlatform::Hotmart);
        let first = f
            .affiliate
            .create_link(&link(hotmart, channel(1), "telegram"), &tagger)
            .await
            .unwrap();

        // Pasted again, even another link of the product, it is the same one.
        let again = f
            .affiliate
            .create_link(
                &NewAffiliateLink {
                    pasted: "https://shop.example/p/123?outro=1".into(),
                    ..link(hotmart, channel(1), "telegram")
                },
                &tagger,
            )
            .await
            .unwrap();

        assert!(again.existed);
        assert_eq!(again.link, first.link);
        assert_eq!(f.affiliate.links().await.unwrap().len(), 1);
        // Nothing is tagged twice: on Shopee tagging is a call to its API.
        assert_eq!(tagger.calls.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn a_marking_belongs_to_one_channel_per_platform() {
    block_on(async {
        let f = Fixture::new().await;
        let hotmart = f.program(AffiliatePlatform::Hotmart).await;
        let kiwify = f.program(AffiliatePlatform::Kiwify).await;
        let hotmart_tagger = FakeTagger::of(AffiliatePlatform::Hotmart);
        f.affiliate
            .create_link(&link(hotmart, channel(1), "telegram"), &hotmart_tagger)
            .await
            .unwrap();

        // Another channel cannot take it on the same Platform: its sales
        // would land on the first channel.
        let taken = f
            .affiliate
            .create_link(
                &NewAffiliateLink {
                    product: "Outro curso".into(),
                    ..link(hotmart, channel(2), "telegram")
                },
                &hotmart_tagger,
            )
            .await;
        assert!(matches!(taken, Err(AffiliateError::TagTaken(tag)) if tag == "telegram"));

        // The same channel reuses its marking for another product.
        let other_product = f
            .affiliate
            .create_link(
                &NewAffiliateLink {
                    product: "Outro curso".into(),
                    ..link(hotmart, channel(1), "telegram")
                },
                &hotmart_tagger,
            )
            .await
            .unwrap();
        assert!(!other_product.existed);

        // Another Platform has markings of its own.
        f.affiliate
            .create_link(
                &link(kiwify, channel(2), "telegram"),
                &FakeTagger::of(AffiliatePlatform::Kiwify),
            )
            .await
            .unwrap();
        assert_eq!(f.affiliate.links().await.unwrap().len(), 3);
    });
}

#[test]
fn a_deleted_link_keeps_its_marking_with_its_channel() {
    block_on(async {
        let f = Fixture::new().await;
        let hotmart = f.program(AffiliatePlatform::Hotmart).await;
        let tagger = FakeTagger::of(AffiliatePlatform::Hotmart);
        let first = f
            .affiliate
            .create_link(&link(hotmart, channel(1), "telegram"), &tagger)
            .await
            .unwrap();

        f.affiliate.delete_link(first.link.id).await.unwrap();

        assert!(f.affiliate.links().await.unwrap().is_empty());
        // Sales from its clicks still arrive with the marking.
        assert!(matches!(
            f.affiliate
                .create_link(&link(hotmart, channel(2), "telegram"), &tagger)
                .await,
            Err(AffiliateError::TagTaken(_))
        ));
        // The same channel makes it again, as a new link.
        let again = f
            .affiliate
            .create_link(&link(hotmart, channel(1), "telegram"), &tagger)
            .await
            .unwrap();
        assert!(!again.existed);
        assert_ne!(again.link.id, first.link.id);
        assert!(matches!(
            f.affiliate.delete_link(first.link.id).await,
            Err(AffiliateError::NoSuchLink)
        ));
    });
}

#[test]
fn a_paid_search_channel_on_a_shopee_program_is_refused_while_its_flag_is_off() {
    block_on(async {
        let f = Fixture::new().await;
        let shopee = f.program(AffiliatePlatform::Shopee).await;
        let tagger = FakeTagger::of(AffiliatePlatform::Shopee);
        let paid_search = NewAffiliateLink {
            promotion: Promotion {
                reach: Reach::PaidSearch,
                keywords: vec!["fone bluetooth".into()],
                ..Promotion::default()
            },
            ..link(shopee, channel(1), "google_ads")
        };

        let refused = f.affiliate.create_link(&paid_search, &tagger).await;

        assert!(matches!(
            refused,
            Err(AffiliateError::Restricted(flag)) if flag == SHOPEE_PAID_SEARCH
        ));
        assert!(f.affiliate.links().await.unwrap().is_empty());
        assert_eq!(tagger.calls.load(Ordering::SeqCst), 0);

        f.turn_on(SHOPEE_PAID_SEARCH).await;
        let created = f
            .affiliate
            .create_link(&paid_search, &tagger)
            .await
            .unwrap();
        assert_eq!(
            created.warnings,
            vec![
                RuleWarning::Practice {
                    practice: Practice::PaidSearch,
                    standing: Standing::Forbidden,
                },
                RuleWarning::Restricted(SHOPEE_PAID_SEARCH),
            ]
        );
    });
}

#[test]
fn a_rule_the_channel_may_break_only_warns() {
    block_on(async {
        let f = Fixture::new().await;
        let hotmart = f.program(AffiliatePlatform::Hotmart).await;
        let tagger = FakeTagger::of(AffiliatePlatform::Hotmart);

        let boosted = f
            .affiliate
            .create_link(
                &NewAffiliateLink {
                    promotion: Promotion {
                        reach: Reach::BoostedPost,
                        ..Promotion::default()
                    },
                    ..link(hotmart, channel(1), "instagram")
                },
                &tagger,
            )
            .await
            .unwrap();

        // The producer's rules are unread until the owner reads them.
        assert_eq!(
            boosted.warnings,
            vec![RuleWarning::Practice {
                practice: Practice::BoostedPosts,
                standing: Standing::Unknown,
            }]
        );
        assert_eq!(f.affiliate.links().await.unwrap().len(), 1);
        // The owner's own channel breaks nothing.
        let own = f
            .affiliate
            .create_link(
                &NewAffiliateLink {
                    product: "Outro curso".into(),
                    ..link(hotmart, channel(2), "telegram")
                },
                &tagger,
            )
            .await
            .unwrap();
        assert!(own.warnings.is_empty());
    });
}

#[test]
fn what_the_adapter_refuses_is_not_kept() {
    block_on(async {
        let f = Fixture::new().await;
        let amazon = f.program(AffiliatePlatform::Amazon).await;

        let refused = f
            .affiliate
            .create_link(
                &NewAffiliateLink {
                    pasted: "https://other.example/p/123".into(),
                    ..link(amazon, channel(1), "fernando-20")
                },
                &FakeTagger::of(AffiliatePlatform::Amazon),
            )
            .await;

        assert!(matches!(
            refused,
            Err(AffiliateError::Tag(TagRefusal::ForeignDomain(
                AffiliatePlatform::Amazon
            )))
        ));
        assert!(f.affiliate.links().await.unwrap().is_empty());
    });
}

#[test]
fn a_link_needs_a_product_a_marking_a_program_and_its_platforms_tagger() {
    block_on(async {
        let f = Fixture::new().await;
        let hotmart = f.program(AffiliatePlatform::Hotmart).await;
        let tagger = FakeTagger::of(AffiliatePlatform::Hotmart);
        let refused = |new: NewAffiliateLink| {
            let affiliate = &f.affiliate;
            let tagger = &tagger;
            async move { affiliate.create_link(&new, tagger).await.unwrap_err() }
        };

        assert!(matches!(
            refused(NewAffiliateLink {
                product: "  ".into(),
                ..link(hotmart, channel(1), "telegram")
            })
            .await,
            AffiliateError::NoProduct
        ));
        for tag in ["", "Telegram", "tele gram", "telegram!", &"a".repeat(41)] {
            assert!(
                matches!(
                    refused(link(hotmart, channel(1), tag)).await,
                    AffiliateError::BadTag
                ),
                "{tag}"
            );
        }
        assert!(matches!(
            refused(link(channel(9), channel(1), "telegram")).await,
            AffiliateError::NoSuchProgram
        ));
        let kiwify = f.program(AffiliatePlatform::Kiwify).await;
        assert!(matches!(
            refused(link(kiwify, channel(1), "telegram")).await,
            AffiliateError::WrongTagger
        ));
        assert!(f.affiliate.links().await.unwrap().is_empty());
        // Up to 40 characters, with digits, `_` and `-`.
        let longest = format!("{}-20", "a_1".repeat(12)) + "x";
        assert_eq!(longest.len(), 40);
        f.affiliate
            .create_link(&link(hotmart, channel(1), &longest), &tagger)
            .await
            .unwrap();
    });
}
