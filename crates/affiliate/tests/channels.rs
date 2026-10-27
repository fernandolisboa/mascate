//! Commissions by Promotion Channel (#101) through the Affiliate module's
//! public interface with a real temporary database: a sale's channel is
//! the one of the Affiliate Link whose Platform and marking match its
//! tracking, read on each query, and the dashboard groups by it with the
//! sales without a channel last.

use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliatePlatform, Commission, CommissionFilter, CommissionReport, CommissionStatus,
    GroupOrder, LinkTagger, MIGRATIONS, NewAffiliateLink, Promotion, ReportedConversion,
    ReportedProgram, TagRefusal,
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

fn channel(n: u128) -> RecordId {
    RecordId::from_u128(0xC0FFEE00 + n)
}

/// Appends the marking as `t`, as a URL-tagging Platform would.
struct Tagger(AffiliatePlatform);

impl LinkTagger for Tagger {
    fn platform(&self) -> AffiliatePlatform {
        self.0
    }

    fn tag(&self, link: &str, tag: &str) -> Result<String, TagRefusal> {
        Ok(format!("{link}?t={tag}"))
    }
}

/// A Program per product, as the infoproduct Platforms report them.
fn sale(
    key: &str,
    product: &str,
    amount: &str,
    status: CommissionStatus,
    tracking: Option<&str>,
) -> ReportedConversion {
    ReportedConversion {
        key: key.into(),
        program: ReportedProgram {
            id: format!("program-{product}"),
            name: product.into(),
        },
        product: product.into(),
        sold_at: noon() - TimeDelta::days(3),
        commission: brl(amount),
        status,
        platform_status: String::new(),
        status_changed_at: None,
        tracking: tracking.map(Into::into),
        raw: "{}".into(),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
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
        let affiliate = Affiliate::new(database, clock, Arc::new(SequentialIds::default()));
        Self {
            _dir: dir,
            affiliate,
        }
    }

    async fn import(&self, platform: AffiliatePlatform, sales: &[ReportedConversion]) {
        self.affiliate.import(platform, sales).await.unwrap();
    }

    /// Links `product`'s Program on `platform` to `channel` with `tag`.
    async fn link(
        &self,
        platform: AffiliatePlatform,
        product: &str,
        channel: RecordId,
        tag: &str,
    ) -> RecordId {
        let program = self
            .affiliate
            .programs()
            .await
            .unwrap()
            .into_iter()
            .find(|program| program.platform == platform && program.name == product)
            .unwrap()
            .id;
        self.affiliate
            .create_link(
                &NewAffiliateLink {
                    program,
                    product: product.into(),
                    pasted: "https://shop.example/p/1".into(),
                    channel,
                    tag: tag.into(),
                    promotion: Promotion::default(),
                },
                &Tagger(platform),
            )
            .await
            .unwrap()
            .link
            .id
    }

    async fn commissions(&self) -> Vec<Commission> {
        self.affiliate
            .commissions(&CommissionFilter::default())
            .await
            .unwrap()
    }

    /// The channel of the one sale of `product` on `platform`.
    async fn channel_of(&self, platform: AffiliatePlatform, product: &str) -> Option<RecordId> {
        let commissions = self.commissions().await;
        let found: Vec<Option<RecordId>> = commissions
            .iter()
            .filter(|c| c.platform == platform && c.product == product)
            .map(|c| c.channel)
            .collect();
        assert_eq!(found.len(), 1, "one sale of {product} on {platform:?}");
        found[0]
    }
}

#[test]
fn a_sale_recorded_before_its_link_joins_the_channel_once_the_link_exists() {
    block_on(async {
        let fixture = Fixture::new().await;
        let telegram = channel(1);
        fixture
            .import(
                AffiliatePlatform::Hotmart,
                &[sale(
                    "H1",
                    "Curso de Fotografia",
                    "97.30",
                    CommissionStatus::Approved,
                    Some("telegram"),
                )],
            )
            .await;
        assert_eq!(
            fixture
                .channel_of(AffiliatePlatform::Hotmart, "Curso de Fotografia")
                .await,
            None
        );

        let link = fixture
            .link(
                AffiliatePlatform::Hotmart,
                "Curso de Fotografia",
                telegram,
                "telegram",
            )
            .await;
        assert_eq!(
            fixture
                .channel_of(AffiliatePlatform::Hotmart, "Curso de Fotografia")
                .await,
            Some(telegram)
        );
        let report =
            CommissionReport::of(&fixture.commissions().await, &[], GroupOrder::Earned).remove(0);
        assert_eq!(
            report
                .by_channel
                .iter()
                .map(|g| (g.of, g.totals.earned()))
                .collect::<Vec<_>>(),
            vec![(Some(telegram), brl("97.30"))]
        );
        assert_eq!(report.by_tracking[0].of.as_deref(), Some("telegram"));

        // Its clicks keep selling until the Platform settles them.
        fixture.affiliate.delete_link(link).await.unwrap();
        assert_eq!(
            fixture
                .channel_of(AffiliatePlatform::Hotmart, "Curso de Fotografia")
                .await,
            Some(telegram)
        );
    });
}

#[test]
fn the_same_tag_on_two_platforms_maps_to_each_platforms_own_link() {
    block_on(async {
        let fixture = Fixture::new().await;
        let (instagram, whatsapp) = (channel(1), channel(2));
        let course = |key| {
            sale(
                key,
                "Curso",
                "40.00",
                CommissionStatus::Approved,
                Some("promo"),
            )
        };
        fixture
            .import(AffiliatePlatform::Hotmart, &[course("H1")])
            .await;
        fixture
            .import(AffiliatePlatform::Kiwify, &[course("K1")])
            .await;
        fixture
            .import(AffiliatePlatform::Monetizze, &[course("M1")])
            .await;
        fixture
            .link(AffiliatePlatform::Hotmart, "Curso", instagram, "promo")
            .await;
        fixture
            .link(AffiliatePlatform::Kiwify, "Curso", whatsapp, "promo")
            .await;

        assert_eq!(
            fixture
                .channel_of(AffiliatePlatform::Hotmart, "Curso")
                .await,
            Some(instagram)
        );
        assert_eq!(
            fixture.channel_of(AffiliatePlatform::Kiwify, "Curso").await,
            Some(whatsapp)
        );
        // No Monetizze link carries it: the tag only shows by tracking.
        assert_eq!(
            fixture
                .channel_of(AffiliatePlatform::Monetizze, "Curso")
                .await,
            None
        );
        let report =
            CommissionReport::of(&fixture.commissions().await, &[], GroupOrder::Name).remove(0);
        assert_eq!(report.by_channel.last().unwrap().of, None);
        assert_eq!(report.by_tracking.len(), 1);
        assert_eq!(report.by_tracking[0].commissions, 3);
    });
}

#[test]
fn a_channel_reuses_its_marking_for_the_sales_of_its_other_products() {
    block_on(async {
        let fixture = Fixture::new().await;
        let telegram = channel(1);
        fixture
            .import(
                AffiliatePlatform::Hotmart,
                &[
                    sale(
                        "H1",
                        "Curso",
                        "40.00",
                        CommissionStatus::Approved,
                        Some("tg"),
                    ),
                    sale("H2", "Ebook", "9.90", CommissionStatus::Pending, Some("tg")),
                ],
            )
            .await;
        fixture
            .link(AffiliatePlatform::Hotmart, "Curso", telegram, "tg")
            .await;

        assert_eq!(
            fixture
                .channel_of(AffiliatePlatform::Hotmart, "Ebook")
                .await,
            Some(telegram)
        );
    });
}

#[test]
fn the_marking_matches_whatever_case_and_spaces_the_platform_sends_back() {
    block_on(async {
        let fixture = Fixture::new().await;
        let shopee = channel(1);
        fixture
            .import(
                AffiliatePlatform::Shopee,
                &[sale(
                    "S1",
                    "Fone",
                    "12.40",
                    CommissionStatus::Approved,
                    Some(" OFERTAS1 "),
                )],
            )
            .await;
        fixture
            .link(AffiliatePlatform::Shopee, "Fone", shopee, "ofertas1")
            .await;

        assert_eq!(
            fixture.channel_of(AffiliatePlatform::Shopee, "Fone").await,
            Some(shopee)
        );
    });
}

#[test]
fn channels_come_most_earned_first_with_the_sales_without_a_channel_last() {
    block_on(async {
        let fixture = Fixture::new().await;
        let (telegram, instagram) = (channel(1), channel(2));
        fixture
            .import(
                AffiliatePlatform::Hotmart,
                &[
                    sale(
                        "H1",
                        "Curso",
                        "40.00",
                        CommissionStatus::Approved,
                        Some("tg"),
                    ),
                    sale(
                        "H2",
                        "Ebook",
                        "90.00",
                        CommissionStatus::Approved,
                        Some("ig"),
                    ),
                    sale("H3", "Planilha", "500.00", CommissionStatus::Paid, None),
                    sale(
                        "H4",
                        "Mentoria",
                        "70.00",
                        CommissionStatus::Paid,
                        Some("site"),
                    ),
                    sale(
                        "H5",
                        "Curso",
                        "60.00",
                        CommissionStatus::Cancelled,
                        Some("tg"),
                    ),
                ],
            )
            .await;
        fixture
            .link(AffiliatePlatform::Hotmart, "Curso", telegram, "tg")
            .await;
        fixture
            .link(AffiliatePlatform::Hotmart, "Ebook", instagram, "ig")
            .await;
        let commissions = fixture.commissions().await;
        let by_channel = |report: &CommissionReport| {
            report
                .by_channel
                .iter()
                .map(|g| (g.of, g.totals.earned(), g.commissions))
                .collect::<Vec<_>>()
        };

        let mut report = CommissionReport::of(&commissions, &[], GroupOrder::Earned).remove(0);
        assert_eq!(
            by_channel(&report),
            vec![
                (Some(instagram), brl("90.00"), 1),
                (Some(telegram), brl("40.00"), 1),
                (None, brl("570.00"), 2),
            ]
        );

        // Named by the caller, who reads the channels from the Marketing.
        let name = |id: RecordId| {
            if id == telegram {
                "Canal do Telegram"
            } else {
                "Instagram"
            }
        };
        report.arrange_channels(GroupOrder::Name, |id| name(id).to_owned());
        assert_eq!(
            report.by_channel.iter().map(|g| g.of).collect::<Vec<_>>(),
            vec![Some(telegram), Some(instagram), None]
        );
        report.arrange_channels(GroupOrder::Earned, |id| name(id).to_owned());
        assert_eq!(
            report.by_channel.iter().map(|g| g.of).collect::<Vec<_>>(),
            vec![Some(instagram), Some(telegram), None]
        );
    });
}
