//! Profit and ROAS per Promotion Channel and per product (#196, ADR 0037
//! decision 7) through the Affiliate module's public interface: Commissions
//! imported into a real temporary database and joined to their channel by
//! the Affiliate Link's marking, weighed against the spend of each channel
//! the caller hands over. Examples worked out by hand, and properties: a
//! shared spend always splits to the channel's spend exactly, profit never
//! mixes currencies, and only approved and paid Commissions earn.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliateLink, AffiliatePlatform, Commission, CommissionFilter, CommissionGroup,
    CommissionReport, CommissionStatus, GroupOrder, LinkTagger, MIGRATIONS, NewAffiliateLink,
    ProgramRef, Promotion, ReportedConversion, ReportedProgram, TagRefusal, spends_on,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{ChannelSpend, Currency, Money, RecordId, Roas, Timestamp};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

fn noon() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 13, 12, 0, 0).unwrap()
}

fn brl(amount: &str) -> Money {
    Money::new(amount.parse::<Decimal>().unwrap(), Currency::Brl)
}

fn usd(amount: &str) -> Money {
    Money::new(amount.parse::<Decimal>().unwrap(), Currency::Usd)
}

fn channel(n: u128) -> RecordId {
    RecordId::from_u128(0xC0FFEE00 + n)
}

fn spend(on: RecordId, amount: Money) -> ChannelSpend {
    ChannelSpend {
        channel: on,
        amount,
    }
}

const PHOTO: &str = "Curso de Fotografia";
const EXCEL: &str = "Excel Avançado";

/// The Meta campaign, with links to both courses.
fn meta() -> RecordId {
    channel(1)
}

/// The Telegram channel, free to post on.
fn telegram() -> RecordId {
    channel(2)
}

/// A TikTok campaign that sold nothing.
fn tiktok() -> RecordId {
    channel(3)
}

/// A boosted post that only sold a pending Commission.
fn boosted() -> RecordId {
    channel(4)
}

/// A campaign paid in reais that sold in dollars.
fn abroad() -> RecordId {
    channel(5)
}

struct Tagger(AffiliatePlatform);

impl LinkTagger for Tagger {
    fn platform(&self) -> AffiliatePlatform {
        self.0
    }

    fn tag(&self, link: &str, tag: &str) -> Result<String, TagRefusal> {
        Ok(format!("{link}?t={tag}"))
    }
}

fn sale(
    key: &str,
    product: &str,
    commission: Money,
    status: CommissionStatus,
    tracking: &str,
) -> ReportedConversion {
    ReportedConversion {
        key: key.into(),
        program: ReportedProgram {
            id: format!("program-{product}"),
            name: product.into(),
        },
        product: product.into(),
        sold_at: noon() - TimeDelta::days(3),
        commission,
        status,
        platform_status: String::new(),
        status_changed_at: None,
        tracking: Some(tracking.into()),
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

    async fn link(&self, product: &str, channel: RecordId, tag: &str) {
        let program = self
            .affiliate
            .programs()
            .await
            .unwrap()
            .into_iter()
            .find(|program| program.name == product)
            .unwrap()
            .id;
        self.affiliate
            .create_link(
                &NewAffiliateLink {
                    program,
                    product: product.into(),
                    pasted: "https://go.hotmart.com/X1".into(),
                    channel,
                    tag: tag.into(),
                    promotion: Promotion::default(),
                },
                &Tagger(AffiliatePlatform::Hotmart),
            )
            .await
            .unwrap();
    }

    /// Hotmart sales of two courses through four channels, linked by
    /// their markings.
    async fn hotmart() -> Self {
        let fixture = Self::new().await;
        let approved = CommissionStatus::Approved;
        fixture
            .affiliate
            .import(
                AffiliatePlatform::Hotmart,
                &[
                    sale("H1", PHOTO, brl("100.00"), approved, "meta"),
                    sale("H2", PHOTO, brl("50.00"), CommissionStatus::Paid, "meta"),
                    sale("H3", PHOTO, brl("40.00"), CommissionStatus::Pending, "meta"),
                    sale("H4", EXCEL, brl("50.00"), approved, "meta"),
                    sale(
                        "H5",
                        EXCEL,
                        brl("30.00"),
                        CommissionStatus::Reversed,
                        "meta",
                    ),
                    sale(
                        "H6",
                        EXCEL,
                        brl("10.00"),
                        CommissionStatus::Cancelled,
                        "meta",
                    ),
                    sale("H7", PHOTO, brl("30.00"), approved, "telegram"),
                    sale(
                        "H8",
                        EXCEL,
                        brl("15.00"),
                        CommissionStatus::Pending,
                        "boost",
                    ),
                ],
            )
            .await
            .unwrap();
        fixture.link(PHOTO, meta(), "meta").await;
        fixture.link(EXCEL, meta(), "meta").await;
        fixture.link(PHOTO, telegram(), "telegram").await;
        fixture.link(PHOTO, tiktok(), "tiktok").await;
        fixture.link(EXCEL, boosted(), "boost").await;
        fixture
    }

    async fn commissions(&self) -> Vec<Commission> {
        self.affiliate
            .commissions(&CommissionFilter::default())
            .await
            .unwrap()
    }

    async fn links(&self) -> Vec<AffiliateLink> {
        self.affiliate.links().await.unwrap()
    }
}

/// R$ 90,00 on the Meta campaign, R$ 25,00 on TikTok and R$ 19,99 on the
/// boosted post; Telegram cost nothing.
fn spends() -> Vec<ChannelSpend> {
    vec![
        spend(meta(), brl("90.00")),
        spend(tiktok(), brl("25.00")),
        spend(boosted(), brl("19.99")),
    ]
}

fn group<K: PartialEq + std::fmt::Debug>(
    groups: &[CommissionGroup<K>],
    of: K,
) -> &CommissionGroup<K> {
    groups
        .iter()
        .find(|group| group.of == of)
        .unwrap_or_else(|| panic!("a group of {of:?}"))
}

fn ratio(text: &str) -> Option<Roas> {
    Roas::new(text.parse().unwrap())
}

#[test]
fn profit_roas_and_spend_per_sale_per_channel_match_the_hand_worked_figures() {
    block_on(async {
        let fixture = Fixture::hotmart().await;

        let reports =
            CommissionReport::of(&fixture.commissions().await, &spends(), GroupOrder::Earned);
        assert_eq!(reports.len(), 1);
        let report = &reports[0];

        // Meta: 100 + 50 paid + 50 earned; the pending 40 is not yet, the
        // reversal and the cancellation never.
        let meta = group(&report.by_channel, Some(meta()));
        assert_eq!(meta.totals.earned(), brl("200.00"));
        assert_eq!(meta.spent, brl("90.00"));
        assert_eq!(meta.profit(), Some(brl("110.00")));
        assert_eq!(meta.profit_if_pending_approve(), Some(brl("150.00")));
        assert_eq!(meta.roas().unwrap().to_pt_br(), "2,22x");
        // Four sales: the reversed and the cancelled ones fell through.
        assert_eq!(meta.sales, 4);
        assert_eq!(meta.spent_per_sale(), Some(brl("22.50")));
        assert!(!meta.suggests_pausing());

        // A sale with no spend keeps all it earned, with nothing to divide.
        let telegram = group(&report.by_channel, Some(telegram()));
        assert_eq!(telegram.profit(), Some(brl("30.00")));
        assert_eq!(telegram.roas(), None);
        assert_eq!(telegram.spent_per_sale(), None);
        assert!(!telegram.suggests_pausing());

        // A channel with no sale still shows what it spent.
        let tiktok = group(&report.by_channel, Some(tiktok()));
        assert_eq!((tiktok.commissions, tiktok.sales), (0, 0));
        assert_eq!(tiktok.profit(), Some(brl("-25.00")));
        assert_eq!(tiktok.roas(), ratio("0"));
        assert_eq!(tiktok.spent_per_sale(), None);
        assert!(tiktok.suggests_pausing());

        // Below R$ 20,00 a loss is too little to judge the campaign by.
        let boosted = group(&report.by_channel, Some(boosted()));
        assert_eq!(boosted.profit(), Some(brl("-19.99")));
        assert_eq!(boosted.profit_if_pending_approve(), Some(brl("-4.99")));
        assert_eq!(boosted.spent_per_sale(), Some(brl("19.99")));
        assert!(!boosted.suggests_pausing());

        assert_eq!(report.spent, brl("134.99"));
        assert_eq!(report.profit(), brl("95.01"));
        assert_eq!(report.profit_if_pending_approve(), brl("150.01"));
        // TikTok and the boosted post earned nothing to lay their spend on.
        assert_eq!(report.unshared, brl("44.99"));
    });
}

#[test]
fn a_shared_spend_splits_among_the_products_by_what_each_earned() {
    block_on(async {
        let fixture = Fixture::hotmart().await;

        let report =
            CommissionReport::of(&fixture.commissions().await, &spends(), GroupOrder::Profit)
                .remove(0);

        // The Meta campaign's R$ 90,00 over its R$ 200,00 earned: the photo
        // course earned 150 of it, Excel 50.
        let photo = group(&report.by_product, PHOTO.to_owned());
        assert_eq!(photo.spent, brl("67.50"));
        assert_eq!(photo.totals.earned(), brl("180.00"));
        assert_eq!(photo.profit(), Some(brl("112.50")));
        let excel = group(&report.by_product, EXCEL.to_owned());
        assert_eq!(excel.spent, brl("22.50"));
        assert_eq!(excel.profit(), Some(brl("27.50")));
        assert_eq!(excel.roas().unwrap().to_pt_br(), "2,22x");
        // The Program of each course takes the same share.
        let excel_program = report
            .by_program
            .iter()
            .find(|group| group.of.name == EXCEL)
            .unwrap();
        assert_eq!(excel_program.spent, brl("22.50"));
        // Most profit first.
        let products: Vec<&str> = report
            .by_product
            .iter()
            .map(|group| group.of.as_str())
            .collect();
        assert_eq!(products, [PHOTO, EXCEL]);
        // What fell on no product stays apart, so the spend adds up.
        let shared = Money::sum(Currency::Brl, report.by_product.iter().map(|g| g.spent)).unwrap();
        assert_eq!(shared.checked_add(report.unshared).unwrap(), report.spent);
    });
}

#[test]
fn a_dollar_commission_beside_a_spend_in_reais_makes_no_profit() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .affiliate
            .import(
                AffiliatePlatform::Hotmart,
                &[sale(
                    "U1",
                    PHOTO,
                    usd("10.00"),
                    CommissionStatus::Approved,
                    "abroad",
                )],
            )
            .await
            .unwrap();
        fixture.link(PHOTO, abroad(), "abroad").await;

        let reports = CommissionReport::of(
            &fixture.commissions().await,
            &[spend(abroad(), brl("50.00"))],
            GroupOrder::Profit,
        );

        let currencies: Vec<Currency> = reports.iter().map(|report| report.currency).collect();
        assert_eq!(currencies, [Currency::Brl, Currency::Usd]);
        let (reais, dollars) = (&reports[0], &reports[1]);
        let paid = group(&reais.by_channel, Some(abroad()));
        assert_eq!(paid.spent, brl("50.00"));
        assert!(paid.mixed_currencies);
        assert_eq!((paid.profit(), paid.roas()), (None, None));
        // Nothing converts, so R$ 50,00 against US$ 10,00 is no loss to act on.
        assert!(!paid.suggests_pausing());
        let earned = group(&dollars.by_channel, Some(abroad()));
        assert_eq!(earned.totals.earned(), usd("10.00"));
        assert_eq!((earned.profit(), earned.roas()), (None, None));
        // Each currency's totals stand alone.
        assert_eq!(reais.profit(), brl("-50.00"));
        assert_eq!(dollars.profit(), usd("10.00"));
        // Nothing was earned in reais to lay the spend on.
        assert_eq!(reais.unshared, brl("50.00"));
        assert!(reais.by_product.is_empty());
    });
}

#[test]
fn a_platform_s_context_counts_its_part_of_each_channel_s_spend() {
    block_on(async {
        let fixture = Fixture::hotmart().await;
        let shopee = ProgramRef {
            id: RecordId::from_u128(77),
            platform: AffiliatePlatform::Shopee,
            name: "Shopee".into(),
        };
        let mut commissions = fixture.commissions().await;
        // A Shopee sale on the Meta campaign too: R$ 50,00 of R$ 250,00.
        let mut shopee_sale = commissions[0].clone();
        shopee_sale.id = RecordId::from_u128(78);
        shopee_sale.platform = AffiliatePlatform::Shopee;
        shopee_sale.program = shopee.id;
        shopee_sale.status = CommissionStatus::Approved;
        shopee_sale.amount = brl("50.00");
        shopee_sale.channel = Some(meta());
        commissions.push(shopee_sale);
        let mut links = fixture.links().await;
        // The TikTok campaign also had a Shopee link: its spend, with no
        // sale, falls on both Platforms evenly.
        let mut shopee_link = links[0].clone();
        shopee_link.program = shopee;
        shopee_link.channel = tiktok();
        links.push(shopee_link);

        // By channel, in the order of their ids.
        let on = |platform| -> Vec<(RecordId, Money)> {
            spends_on(platform, &spends(), &commissions, &links)
                .into_iter()
                .map(|spend| (spend.channel, spend.amount))
                .collect()
        };

        assert_eq!(
            on(AffiliatePlatform::Hotmart),
            [
                (meta(), brl("72.00")),
                (tiktok(), brl("12.50")),
                (boosted(), brl("19.99")),
            ]
        );
        assert_eq!(
            on(AffiliatePlatform::Shopee),
            [(meta(), brl("18.00")), (tiktok(), brl("12.50"))]
        );
        // A Platform with no link nor sale on a channel takes none of it.
        assert_eq!(on(AffiliatePlatform::Kiwify), []);
    });
}

fn any_commissions() -> impl Strategy<Value = Vec<Commission>> {
    let one = (
        prop::sample::select(vec![Currency::Brl, Currency::Usd]),
        prop::sample::select(CommissionStatus::ALL.to_vec()),
        0i64..10_000_000,
        0u32..4,
        prop::sample::select(vec![
            AffiliatePlatform::Hotmart,
            AffiliatePlatform::Shopee,
            AffiliatePlatform::Kiwify,
        ]),
        prop::sample::select(vec!["Curso", "Fone", "Air fryer"]),
        prop::option::of(0u128..4),
    );
    prop::collection::vec(one, 0..30).prop_map(|drawn| {
        drawn
            .into_iter()
            .enumerate()
            .map(
                |(at, (currency, status, units, scale, platform, product, on))| Commission {
                    id: RecordId::from_u128(at as u128 + 1),
                    platform,
                    program: RecordId::from_u128(platform as u128 + 100),
                    program_name: platform.name().into(),
                    product: product.into(),
                    sold_at: noon(),
                    amount: Money::new(Decimal::new(units, scale), currency),
                    status,
                    platform_status: String::new(),
                    status_since: noon(),
                    tracking: None,
                    manual: false,
                    payout: None,
                    linked_product: None,
                    channel: on.map(channel),
                    link: None,
                },
            )
            .collect()
    })
}

fn any_spends() -> impl Strategy<Value = Vec<ChannelSpend>> {
    let one = (
        0u128..5,
        prop::sample::select(vec![Currency::Brl, Currency::Usd]),
        1i64..10_000_000,
        0u32..5,
    );
    prop::collection::vec(one, 0..8).prop_map(|drawn| {
        drawn
            .into_iter()
            .map(|(on, currency, units, scale)| {
                spend(
                    channel(on),
                    Money::new(Decimal::new(units, scale), currency),
                )
            })
            .collect()
    })
}

/// What `spends` sum to per channel and currency.
fn per_channel(spends: &[ChannelSpend]) -> BTreeMap<(RecordId, &'static str), Decimal> {
    let mut sums = BTreeMap::new();
    for spend in spends {
        *sums
            .entry((spend.channel, spend.amount.currency().code()))
            .or_insert(Decimal::ZERO) += spend.amount.amount();
    }
    sums
}

fn spent<K>(groups: &[CommissionGroup<K>]) -> Decimal {
    groups.iter().map(|group| group.spent.amount()).sum()
}

proptest! {
    #[test]
    fn a_shared_spend_always_splits_to_the_channel_s_spend_exactly(
        commissions in any_commissions(),
        spends in any_spends(),
    ) {
        let reports = CommissionReport::of(&commissions, &spends, GroupOrder::Earned);
        let expected = per_channel(&spends);
        let mut reported = BTreeMap::new();
        for report in &reports {
            for group in &report.by_channel {
                if let Some(channel) = group.of
                    && !group.spent.amount().is_zero()
                {
                    reported.insert((channel, report.currency.code()), group.spent.amount());
                }
            }
            prop_assert_eq!(spent(&report.by_channel), report.spent.amount());
            // Every other grouping holds the shares, which add up to all
            // but what no Commission could take.
            let shared = report.spent.amount() - report.unshared.amount();
            prop_assert_eq!(spent(&report.by_platform), shared);
            prop_assert_eq!(spent(&report.by_program), shared);
            prop_assert_eq!(spent(&report.by_product), shared);
            prop_assert_eq!(spent(&report.by_tracking), shared);
            for group in &report.by_product {
                prop_assert!(!group.spent.is_negative());
                // A product that earned nothing takes no share.
                if group.totals.earned().amount().is_zero() {
                    prop_assert!(group.spent.amount().is_zero());
                }
            }
        }
        prop_assert_eq!(reported, expected);
    }

    #[test]
    fn profit_never_mixes_currencies_and_only_approved_and_paid_earn(
        commissions in any_commissions(),
        spends in any_spends(),
    ) {
        let reports = CommissionReport::of(&commissions, &spends, GroupOrder::Profit);
        let mut currencies_of: BTreeMap<Option<RecordId>, (usize, bool)> = BTreeMap::new();
        for report in &reports {
            for group in &report.by_channel {
                let (count, spent) = currencies_of.entry(group.of).or_default();
                *count += 1;
                *spent |= !group.spent.amount().is_zero();
            }
        }
        for report in &reports {
            let earned: Decimal = commissions
                .iter()
                .filter(|c| c.amount.currency() == report.currency && c.status.is_earned())
                .map(|c| c.amount.amount())
                .sum();
            prop_assert_eq!(report.totals.earned().amount(), earned);
            prop_assert_eq!(
                report.profit().amount(),
                earned - report.spent.amount()
            );
            for group in &report.by_channel {
                let (count, spent) = currencies_of[&group.of];
                let mixed = count > 1 && spent;
                prop_assert_eq!(group.profit().is_none(), mixed);
                if let Some(profit) = group.profit() {
                    prop_assert_eq!(profit.currency(), report.currency);
                    prop_assert_eq!(
                        profit.amount(),
                        group.totals.earned().amount() - group.spent.amount()
                    );
                } else {
                    prop_assert!(!group.suggests_pausing());
                }
            }
        }
    }

    #[test]
    fn each_platform_s_part_of_a_spend_adds_up_to_the_spend(
        commissions in any_commissions(),
        spends in any_spends(),
    ) {
        let platforms = [
            AffiliatePlatform::Hotmart,
            AffiliatePlatform::Shopee,
            AffiliatePlatform::Kiwify,
        ];
        let mut parts: Vec<ChannelSpend> = Vec::new();
        for platform in platforms {
            parts.extend(spends_on(platform, &spends, &commissions, &[]));
        }
        // With no link, a channel nobody earned on falls on no Platform.
        let earned_on: Vec<(RecordId, &'static str)> = commissions
            .iter()
            .filter(|c| c.status.is_earned() && !c.amount.amount().is_zero())
            .filter_map(|c| Some((c.channel?, c.amount.currency().code())))
            .collect();
        let mut expected = per_channel(&spends);
        expected.retain(|key, _| earned_on.contains(key));
        prop_assert_eq!(per_channel(&parts), expected);
    }
}
