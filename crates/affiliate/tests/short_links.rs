//! Short Links through the Affiliate module's public interface with a real
//! temporary database: one Short Link per Affiliate Link, a path that comes
//! from the link, the Platforms whose terms forbid a shortener refused
//! while their Restricted Feature is off, and the shortener's failures
//! keeping nothing.

use std::sync::{Arc, Mutex};

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    AMAZON_SHORT_LINK, Affiliate, AffiliateError, AffiliateLink, AffiliatePlatform,
    CommissionStatus, FLAGS, LinkShortener, LinkTagger, MERCADO_LIVRE_SHORT_LINK, MIGRATIONS,
    NewAffiliateLink, Practice, Promotion, PromotionRules, ReportedConversion, ReportedProgram,
    RuleWarning, SHOPEE_SHORT_LINK, Shortened, Standing, TagRefusal,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, PlatformError, RecordId, Timestamp};
use mascate_platform::{Database, Flag, Flags, Registry, migrate};
use rust_decimal::Decimal;

fn noon() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap()
}

/// Tags any link with `t`.
struct Tagger(AffiliatePlatform);

impl LinkTagger for Tagger {
    fn platform(&self) -> AffiliatePlatform {
        self.0
    }

    fn tag(&self, link: &str, tag: &str) -> Result<String, TagRefusal> {
        Ok(format!("{link}?t={tag}"))
    }
}

/// Shortens on `mascate.short.gy`, remembering each link and path it was
/// asked, and failing with the errors queued first.
#[derive(Default)]
struct FakeShortener {
    asked: Mutex<Vec<(String, String)>>,
    failures: Mutex<Vec<PlatformError>>,
}

impl FakeShortener {
    fn failing(error: PlatformError) -> Self {
        Self {
            failures: Mutex::new(vec![error]),
            ..Self::default()
        }
    }

    fn asked(&self) -> Vec<(String, String)> {
        self.asked.lock().unwrap().clone()
    }
}

impl LinkShortener for FakeShortener {
    fn shorten(&self, link: &str, path: &str) -> Result<Shortened, PlatformError> {
        self.asked
            .lock()
            .unwrap()
            .push((link.to_owned(), path.to_owned()));
        if let Some(error) = self.failures.lock().unwrap().pop() {
            return Err(error);
        }
        Ok(Shortened {
            url: format!("https://mascate.short.gy/{path}"),
            key: format!("lnk_{path}"),
        })
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

fn channel(n: u128) -> RecordId {
    RecordId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n)
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    affiliate: Affiliate,
    flags: Flags,
}

impl Fixture {
    /// One Program on each Platform the terms speak of.
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
            AffiliatePlatform::MercadoLivre,
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

    /// A link of the Platform's Program for channel `n`, tagged `canal{n}`.
    async fn link(&self, platform: AffiliatePlatform, n: u128) -> AffiliateLink {
        self.affiliate
            .create_link(
                &NewAffiliateLink {
                    program: self.program(platform).await,
                    product: "Curso de Fotografia".into(),
                    pasted: "https://shop.example/p/123".into(),
                    channel: channel(n),
                    tag: format!("canal{n}"),
                    promotion: Promotion::default(),
                },
                &Tagger(platform),
            )
            .await
            .unwrap()
            .link
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

fn shortener_warning(standing: Standing) -> RuleWarning {
    RuleWarning::Practice {
        practice: Practice::Shortener,
        standing,
    }
}

#[test]
fn shortening_the_same_affiliate_link_twice_yields_one_short_link() {
    block_on(async {
        let f = Fixture::new().await;
        let link = f.link(AffiliatePlatform::Hotmart, 1).await;
        let shortener = FakeShortener::default();

        let first = f.affiliate.shorten(link.id, &shortener).await.unwrap();
        let again = f.affiliate.shorten(link.id, &shortener).await.unwrap();

        assert!(!first.existed);
        assert!(again.existed);
        let (asked_link, path) = shortener.asked()[0].clone();
        assert_eq!(asked_link, link.tagged);
        let short = format!("https://mascate.short.gy/{path}");
        assert_eq!(first.link.short.as_deref(), Some(short.as_str()));
        assert_eq!(again.link, first.link);
        assert_eq!(
            first.link,
            AffiliateLink {
                short: Some(short),
                ..link.clone()
            }
        );
        // The shortener is asked once; the list shows the one Short Link.
        assert_eq!(shortener.asked().len(), 1);
        assert_eq!(f.affiliate.links().await.unwrap(), vec![first.link.clone()]);
        assert_eq!(f.affiliate.link(link.id).await.unwrap(), first.link);
    });
}

#[test]
fn each_link_gets_its_own_short_path_of_letters_and_digits() {
    block_on(async {
        let f = Fixture::new().await;
        let one = f.link(AffiliatePlatform::Hotmart, 1).await;
        let other = f.link(AffiliatePlatform::Hotmart, 2).await;
        let shortener = FakeShortener::default();

        f.affiliate.shorten(one.id, &shortener).await.unwrap();
        f.affiliate.shorten(other.id, &shortener).await.unwrap();

        let paths: Vec<String> = shortener
            .asked()
            .into_iter()
            .map(|(_, path)| path)
            .collect();
        assert_ne!(paths[0], paths[1]);
        for path in &paths {
            assert_eq!(path.len(), 8, "{path}");
            assert!(
                path.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "{path}"
            );
        }
    });
}

#[test]
fn a_failed_shortening_keeps_nothing_and_asks_the_same_path_again() {
    block_on(async {
        let f = Fixture::new().await;
        let link = f.link(AffiliatePlatform::Kiwify, 1).await;
        let shortener = FakeShortener::failing(PlatformError::Expired);

        let failed = f.affiliate.shorten(link.id, &shortener).await;

        assert!(matches!(
            failed,
            Err(AffiliateError::Platform(PlatformError::Expired))
        ));
        assert_eq!(f.affiliate.link(link.id).await.unwrap().short, None);

        // The same path again, so a link the shortener made before its
        // answer got lost comes back instead of a second one.
        f.affiliate.shorten(link.id, &shortener).await.unwrap();
        let asked = shortener.asked();
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0], asked[1]);
    });
}

#[test]
fn amazon_shopee_and_mercado_livre_links_are_refused_while_their_flags_are_off() {
    block_on(async {
        let f = Fixture::new().await;
        let shortener = FakeShortener::default();
        for (platform, flag) in [
            (AffiliatePlatform::Amazon, AMAZON_SHORT_LINK),
            (AffiliatePlatform::Shopee, SHOPEE_SHORT_LINK),
            (AffiliatePlatform::MercadoLivre, MERCADO_LIVRE_SHORT_LINK),
        ] {
            let link = f.link(platform, 1).await;

            let refused = f.affiliate.shorten(link.id, &shortener).await;

            assert!(
                matches!(refused, Err(AffiliateError::Restricted(refusing)) if refusing == flag),
                "{platform:?}"
            );
            assert_eq!(f.affiliate.link(link.id).await.unwrap().short, None);
        }
        assert!(shortener.asked().is_empty());
    });
}

#[test]
fn a_flag_the_owner_turns_on_lets_the_short_link_through_with_its_warnings() {
    block_on(async {
        let f = Fixture::new().await;
        let link = f.link(AffiliatePlatform::MercadoLivre, 1).await;
        f.turn_on(MERCADO_LIVRE_SHORT_LINK).await;

        let shortened = f
            .affiliate
            .shorten(link.id, &FakeShortener::default())
            .await
            .unwrap();

        assert!(shortened.link.short.is_some());
        // Mercado Livre's own rules forbid a shortener too.
        assert_eq!(
            shortened.warnings,
            [
                shortener_warning(Standing::Forbidden),
                RuleWarning::Restricted(MERCADO_LIVRE_SHORT_LINK),
            ]
        );
    });
}

#[test]
fn hotmart_and_kiwify_go_through_and_an_unread_shortener_rule_warns() {
    block_on(async {
        let f = Fixture::new().await;
        let hotmart = f.link(AffiliatePlatform::Hotmart, 1).await;
        let kiwify = f.link(AffiliatePlatform::Kiwify, 1).await;
        let kiwify_program = f.program(AffiliatePlatform::Kiwify).await;
        f.affiliate
            .set_promotion_rules(
                kiwify_program,
                &PromotionRules::defaults(AffiliatePlatform::Kiwify)
                    .with(Practice::Shortener, Standing::Allowed),
            )
            .await
            .unwrap();
        let shortener = FakeShortener::default();

        let unread = f.affiliate.shorten(hotmart.id, &shortener).await.unwrap();
        let allowed = f.affiliate.shorten(kiwify.id, &shortener).await.unwrap();

        assert_eq!(unread.warnings, [shortener_warning(Standing::Unknown)]);
        assert!(unread.link.short.is_some());
        assert!(allowed.warnings.is_empty());
        assert!(allowed.link.short.is_some());
    });
}

#[test]
fn a_flag_turned_off_again_refuses_even_the_short_link_already_made() {
    block_on(async {
        let f = Fixture::new().await;
        let link = f.link(AffiliatePlatform::Amazon, 1).await;
        f.turn_on(AMAZON_SHORT_LINK).await;
        let shortener = FakeShortener::default();
        f.affiliate.shorten(link.id, &shortener).await.unwrap();
        f.flags
            .turn_off(AMAZON_SHORT_LINK.key, "fernando")
            .await
            .unwrap();

        let refused = f.affiliate.shorten(link.id, &shortener).await;

        assert!(matches!(
            refused,
            Err(AffiliateError::Restricted(AMAZON_SHORT_LINK))
        ));
        assert_eq!(shortener.asked().len(), 1);
    });
}

#[test]
fn only_a_live_link_is_shortened() {
    block_on(async {
        let f = Fixture::new().await;
        let link = f.link(AffiliatePlatform::Hotmart, 1).await;
        f.affiliate.delete_link(link.id).await.unwrap();
        let shortener = FakeShortener::default();

        for id in [link.id, channel(99)] {
            assert!(matches!(
                f.affiliate.shorten(id, &shortener).await,
                Err(AffiliateError::NoSuchLink)
            ));
        }
        assert!(shortener.asked().is_empty());
    });
}
