//! Clicks on Short Links through the Affiliate module's public interface
//! with a real temporary database: read day by day from a fake shortener,
//! kept by Short Link and day so a window read again changes nothing, the
//! late counts caught, a failure keeping what was read, and the clicks set
//! beside the Commissions each link brought.

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::{Arc, Mutex};

use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliateLink, AffiliatePlatform, CLICKS_SETTLE_DAYS, ClickGroup,
    ClickReport, ClicksRead, Commission, CommissionFilter, CommissionStatus, DayClicks, LinkClicks,
    LinkShortener, LinkTagger, MIGRATIONS, NewAffiliateLink, Promotion, ReportedConversion,
    ReportedProgram, Shortened, TagRefusal,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, PlatformError, RecordId, Timestamp, channel_day_start};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

/// 09:00 in Brasília on 22/10/2026.
fn noon() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap()
}

fn day(d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
}

fn clicks_on(d: u32, clicks: u64) -> DayClicks {
    DayClicks {
        day: day(d),
        clicks,
    }
}

fn ever() -> std::ops::Range<Timestamp> {
    Timestamp::MIN_UTC..Timestamp::MAX_UTC
}

fn channel(n: u128) -> RecordId {
    RecordId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n)
}

struct Tagger;

impl LinkTagger for Tagger {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Hotmart
    }

    fn tag(&self, link: &str, tag: &str) -> Result<String, TagRefusal> {
        Ok(format!("{link}?src={tag}"))
    }
}

/// Shortens with the path as the key, counts what each key was given, and
/// remembers each window it was asked; a key with a queued failure fails
/// once.
#[derive(Default)]
struct FakeShortener {
    counts: Mutex<HashMap<String, Vec<DayClicks>>>,
    failures: Mutex<HashMap<String, PlatformError>>,
    asked: Mutex<Vec<(String, RangeInclusive<NaiveDate>)>>,
}

impl FakeShortener {
    fn count(&self, key: &str, days: Vec<DayClicks>) {
        self.counts.lock().unwrap().insert(key.to_owned(), days);
    }

    fn fail(&self, key: &str, error: PlatformError) {
        self.failures.lock().unwrap().insert(key.to_owned(), error);
    }

    fn asked(&self) -> Vec<(String, RangeInclusive<NaiveDate>)> {
        std::mem::take(&mut self.asked.lock().unwrap())
    }
}

impl LinkShortener for FakeShortener {
    fn shorten(&self, _: &str, path: &str) -> Result<Shortened, PlatformError> {
        Ok(Shortened {
            url: format!("https://mascate.short.gy/{path}"),
            key: path.to_owned(),
        })
    }

    fn clicks(
        &self,
        key: &str,
        days: RangeInclusive<NaiveDate>,
    ) -> Result<Vec<DayClicks>, PlatformError> {
        self.asked
            .lock()
            .unwrap()
            .push((key.to_owned(), days.clone()));
        if let Some(error) = self.failures.lock().unwrap().remove(key) {
            return Err(error);
        }
        Ok(self
            .counts
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .unwrap_or_default())
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    affiliate: Affiliate,
    shortener: FakeShortener,
    program: RecordId,
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
        migrate(
            &database,
            clock.as_ref(),
            &[mascate_platform::MIGRATIONS, MIGRATIONS],
        )
        .await
        .unwrap();
        let affiliate = Affiliate::new(database, clock.clone(), Arc::new(SequentialIds::default()));
        affiliate
            .import(
                AffiliatePlatform::Hotmart,
                &[sale("first", "Curso de Fotografia", None, noon())],
            )
            .await
            .unwrap();
        let program = affiliate.programs().await.unwrap()[0].id;
        Self {
            _dir: dir,
            clock,
            affiliate,
            shortener: FakeShortener::default(),
            program,
        }
    }

    /// A link of `product` for channel `n`, tagged `canal{n}`.
    async fn link(&self, product: &str, n: u128) -> AffiliateLink {
        self.affiliate
            .create_link(
                &NewAffiliateLink {
                    program: self.program,
                    product: product.into(),
                    pasted: format!("https://go.hotmart.com/{n}"),
                    channel: channel(n),
                    tag: format!("canal{n}"),
                    promotion: Promotion::default(),
                },
                &Tagger,
            )
            .await
            .unwrap()
            .link
    }

    /// The link shortened, with its key on the fake shortener.
    async fn shortened(&self, product: &str, n: u128) -> (AffiliateLink, String) {
        let link = self.link(product, n).await;
        self.affiliate
            .shorten(link.id, &self.shortener)
            .await
            .unwrap();
        let key = self.key_of(link.id).await;
        (link, key)
    }

    async fn shortener_with_link(&self) -> (AffiliateLink, String) {
        self.shortened("Curso de Fotografia", 1).await
    }

    async fn key_of(&self, link: RecordId) -> String {
        let short = self.affiliate.link(link).await.unwrap().short.unwrap();
        short.rsplit('/').next().unwrap().to_owned()
    }

    async fn read(&self) -> ClicksRead {
        self.affiliate.read_clicks(&self.shortener).await.unwrap()
    }

    async fn clicks(&self) -> Vec<LinkClicks> {
        self.affiliate.link_clicks(ever()).await.unwrap()
    }

    /// The first link's clicks from day `from` to day `to` of October.
    async fn clicks_during(&self, from: u32, to: u32) -> u64 {
        let during = channel_day_start(day(from))..channel_day_start(day(to) + TimeDelta::days(1));
        self.affiliate.link_clicks(during).await.unwrap()[0].clicks
    }

    async fn commissions(&self) -> Vec<Commission> {
        self.affiliate
            .commissions(&CommissionFilter::default())
            .await
            .unwrap()
    }
}

fn sale(
    key: &str,
    product: &str,
    tracking: Option<&str>,
    sold_at: Timestamp,
) -> ReportedConversion {
    ReportedConversion {
        key: key.into(),
        program: ReportedProgram {
            id: "123".into(),
            name: "Fotografia".into(),
        },
        product: product.into(),
        sold_at,
        commission: Money::new(Decimal::new(4710, 2), Currency::Brl),
        status: CommissionStatus::Approved,
        platform_status: "APPROVED".into(),
        status_changed_at: None,
        tracking: tracking.map(str::to_owned),
        raw: "{}".into(),
    }
}

#[test]
fn reading_the_same_window_twice_changes_nothing_and_a_grown_day_is_updated() {
    block_on(async {
        let f = Fixture::new().await;
        let (link, key) = f.shortened("Curso de Fotografia", 1).await;
        f.shortener.count(&key, vec![clicks_on(22, 4)]);

        let first = f.read().await;
        let again = f.read().await;

        assert_eq!(
            first,
            ClicksRead {
                links: 1,
                changed: 1,
                gone: 0
            }
        );
        assert_eq!(again.changed, 0);
        assert_eq!(f.clicks().await[0].clicks, 4);

        f.shortener.count(&key, vec![clicks_on(22, 9)]);
        let grown = f.read().await;

        assert_eq!(grown.changed, 1);
        let clicks = f.clicks().await;
        assert_eq!(
            clicks,
            vec![LinkClicks {
                link: link.id,
                channel: channel(1),
                shortened_at: noon(),
                clicks: 9,
            }]
        );
    });
}

#[test]
fn the_first_read_starts_on_the_day_of_the_short_link_and_later_ones_a_few_days_back() {
    block_on(async {
        let f = Fixture::new().await;
        let (_, key) = f.shortened("Curso de Fotografia", 1).await;

        f.read().await;
        assert_eq!(f.shortener.asked(), vec![(key.clone(), day(22)..=day(22))]);

        f.clock.advance(TimeDelta::days(9));
        f.read().await;
        assert_eq!(f.shortener.asked(), vec![(key.clone(), day(22)..=day(31))]);

        f.shortener
            .count(&key, vec![clicks_on(30, 2), clicks_on(31, 1)]);
        f.read().await;
        f.shortener.asked();
        f.clock.advance(TimeDelta::days(2));
        f.read().await;

        let november_2 = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        assert_eq!(
            f.shortener.asked(),
            vec![(
                key,
                day(31) - TimeDelta::days(CLICKS_SETTLE_DAYS)..=november_2
            )]
        );
    });
}

#[test]
fn a_day_outside_the_window_or_given_twice_counts_as_asked() {
    block_on(async {
        let f = Fixture::new().await;
        let (_, key) = f.shortener_with_link().await;
        f.shortener.count(
            &key,
            vec![
                clicks_on(21, 50),
                clicks_on(22, 3),
                clicks_on(22, 5),
                clicks_on(23, 70),
            ],
        );

        let read = f.read().await;

        assert_eq!(read.changed, 1);
        assert_eq!(f.clicks().await[0].clicks, 5);
    });
}

#[test]
fn a_link_without_a_short_link_is_never_asked_and_has_no_clicks() {
    block_on(async {
        let f = Fixture::new().await;
        f.link("Curso de Fotografia", 1).await;

        let read = f.read().await;

        assert_eq!(read, ClicksRead::default());
        assert!(f.shortener.asked().is_empty());
        assert!(f.clicks().await.is_empty());
        // The read went through, with nothing to read.
        assert_eq!(f.affiliate.clicks_read_at().await.unwrap(), Some(noon()));
    });
}

#[test]
fn a_short_link_without_a_click_counts_zero() {
    block_on(async {
        let f = Fixture::new().await;
        let (link, _) = f.shortener_with_link().await;

        f.read().await;

        let clicks = f.clicks().await;
        assert_eq!(clicks.len(), 1);
        assert_eq!((clicks[0].link, clicks[0].clicks), (link.id, 0));
    });
}

#[test]
fn a_failure_keeps_the_clicks_and_when_they_were_last_read() {
    block_on(async {
        let f = Fixture::new().await;
        let (_, first) = f.shortened("Curso de Fotografia", 1).await;
        let (_, second) = f.shortened("Curso de Edição", 2).await;
        f.shortener.count(&first, vec![clicks_on(22, 4)]);
        f.shortener.count(&second, vec![clicks_on(22, 6)]);
        f.read().await;
        assert_eq!(f.affiliate.clicks_read_at().await.unwrap(), Some(noon()));

        f.clock.advance(TimeDelta::hours(1));
        f.shortener.count(&first, vec![clicks_on(22, 40)]);
        f.shortener.fail(&second, PlatformError::RateLimited);
        let failed = f.affiliate.read_clicks(&f.shortener).await;

        assert!(matches!(
            failed,
            Err(AffiliateError::Platform(PlatformError::RateLimited))
        ));
        let mut kept: Vec<u64> = f.clicks().await.iter().map(|link| link.clicks).collect();
        kept.sort_unstable();
        assert_eq!(kept, [4, 6]);
        assert_eq!(f.affiliate.clicks_read_at().await.unwrap(), Some(noon()));
    });
}

#[test]
fn a_short_link_the_shortener_no_longer_has_keeps_its_clicks_and_the_rest_are_read() {
    block_on(async {
        let f = Fixture::new().await;
        let (gone, gone_key) = f.shortened("Curso de Fotografia", 1).await;
        let (_, key) = f.shortened("Curso de Edição", 2).await;
        f.shortener.count(&gone_key, vec![clicks_on(22, 4)]);
        f.read().await;

        f.shortener.fail(&gone_key, PlatformError::NotFound);
        f.shortener.count(&key, vec![clicks_on(22, 7)]);
        let read = f.read().await;

        assert_eq!(
            read,
            ClicksRead {
                links: 1,
                changed: 1,
                gone: 1
            }
        );
        let clicks = f.clicks().await;
        let of = |id: RecordId| clicks.iter().find(|link| link.link == id).unwrap().clicks;
        assert_eq!(of(gone.id), 4);
        assert_eq!(clicks.iter().map(|link| link.clicks).sum::<u64>(), 11);
    });
}

#[test]
fn a_deleted_link_is_no_longer_read_nor_shown() {
    block_on(async {
        let f = Fixture::new().await;
        let (link, _) = f.shortener_with_link().await;
        f.affiliate.delete_link(link.id).await.unwrap();

        f.read().await;

        assert!(f.shortener.asked().is_empty());
        assert!(f.clicks().await.is_empty());
    });
}

#[test]
fn a_period_sums_the_days_that_start_in_it_in_brasilia() {
    block_on(async {
        let f = Fixture::new().await;
        let (_, key) = f.shortener_with_link().await;
        f.clock.advance(TimeDelta::days(3));
        f.shortener.count(
            &key,
            vec![
                clicks_on(22, 1),
                clicks_on(23, 10),
                clicks_on(24, 100),
                clicks_on(25, 1000),
            ],
        );
        f.read().await;

        assert_eq!(f.clicks_during(23, 24).await, 110);
        assert_eq!(f.clicks_during(22, 25).await, 1111);
        assert_eq!(f.clicks_during(26, 27).await, 0);
    });
}

#[test]
fn a_sale_joins_the_link_of_its_product_or_the_only_one_with_its_marking() {
    block_on(async {
        let f = Fixture::new().await;
        let photo = f.link("Curso de Fotografia", 1).await;
        let editing = f.link("Curso de Edição", 1).await;
        let alone = f.link("Curso de Fotografia", 2).await;
        f.affiliate
            .import(
                AffiliatePlatform::Hotmart,
                &[
                    sale(
                        "by-product",
                        " curso de fotografia ",
                        Some("canal1"),
                        noon(),
                    ),
                    sale("ambiguous", "Outro curso", Some("canal1"), noon()),
                    sale("only-one", "Outro curso", Some("CANAL2"), noon()),
                    sale("untagged", "Curso de Edição", None, noon()),
                ],
            )
            .await
            .unwrap();

        let commissions = f.commissions().await;
        let link_of = |product: &str, tracking: Option<&str>| {
            commissions
                .iter()
                .find(|c| c.product == product && c.tracking.as_deref() == tracking)
                .unwrap()
                .link
        };

        // The product decides between the channel's two links.
        assert_eq!(
            link_of(" curso de fotografia ", Some("canal1")),
            Some(photo.id)
        );
        // Another product with a marking two links share joins neither.
        assert_eq!(link_of("Outro curso", Some("canal1")), None);
        assert_eq!(link_of("Outro curso", Some("CANAL2")), Some(alone.id));
        assert_eq!(link_of("Curso de Edição", None), None);
        assert_ne!(photo.id, editing.id);
    });
}

#[test]
fn clicks_per_commission_counts_the_sales_through_the_short_link_only() {
    block_on(async {
        let f = Fixture::new().await;
        let (photo, photo_key) = f.shortened("Curso de Fotografia", 1).await;
        let (editing, editing_key) = f.shortened("Curso de Edição", 1).await;
        // A link of the same channel without a Short Link: its sale stays
        // out of the ratio.
        f.link("Curso de Música", 1).await;
        let (other, other_key) = f.shortened("Curso de Fotografia", 2).await;
        f.shortener.count(&photo_key, vec![clicks_on(22, 30)]);
        f.shortener.count(&editing_key, vec![clicks_on(22, 10)]);
        f.shortener.count(&other_key, vec![clicks_on(22, 5)]);
        f.read().await;
        let after = noon() + TimeDelta::hours(1);
        let mut cancelled = sale("cancelled", "Curso de Fotografia", Some("canal1"), after);
        cancelled.status = CommissionStatus::Cancelled;
        f.affiliate
            .import(
                AffiliatePlatform::Hotmart,
                &[
                    sale("photo-1", "Curso de Fotografia", Some("canal1"), after),
                    sale("photo-2", "Curso de Fotografia", Some("canal1"), after),
                    sale(
                        "before-the-short-link",
                        "Curso de Fotografia",
                        Some("canal1"),
                        noon() - TimeDelta::days(1),
                    ),
                    cancelled,
                    sale("music", "Curso de Música", Some("canal1"), after),
                ],
            )
            .await
            .unwrap();

        let report = ClickReport::of(&f.clicks().await, &f.commissions().await);

        assert_eq!(
            report.by_link,
            vec![
                ClickGroup {
                    of: photo.id,
                    clicks: 30,
                    commissions: 2
                },
                ClickGroup {
                    of: editing.id,
                    clicks: 10,
                    commissions: 0
                },
                ClickGroup {
                    of: other.id,
                    clicks: 5,
                    commissions: 0
                },
            ]
        );
        assert_eq!(
            report.by_channel,
            vec![
                ClickGroup {
                    of: channel(1),
                    clicks: 40,
                    commissions: 2
                },
                ClickGroup {
                    of: channel(2),
                    clicks: 5,
                    commissions: 0
                },
            ]
        );
        assert_eq!(
            report.by_channel[0].clicks_per_commission(),
            Some(Decimal::from(20))
        );
        assert_eq!(report.by_channel[1].clicks_per_commission(), None);
    });
}

fn any_link_clicks() -> impl Strategy<Value = Vec<LinkClicks>> {
    prop::collection::vec((0u128..4, 0u64..10_000), 0..12).prop_map(|links| {
        links
            .into_iter()
            .enumerate()
            .map(|(at, (channel_n, clicks))| LinkClicks {
                link: RecordId::from_u128(at as u128 + 100),
                channel: channel(channel_n),
                shortened_at: noon(),
                clicks,
            })
            .collect()
    })
}

proptest! {
    #[test]
    fn the_channels_add_up_to_their_links_and_none_is_counted_twice(
        clicks in any_link_clicks(),
        sales in prop::collection::vec((0u128..14, any::<bool>()), 0..30),
    ) {
        let commissions: Vec<Commission> = sales
            .iter()
            .enumerate()
            .map(|(at, (link, before))| Commission {
                id: RecordId::from_u128(at as u128 + 1_000),
                platform: AffiliatePlatform::Hotmart,
                program: RecordId::from_u128(1),
                program_name: "Fotografia".into(),
                product: "Curso".into(),
                sold_at: if *before { noon() - TimeDelta::seconds(1) } else { noon() },
                amount: Money::new(Decimal::ONE, Currency::Brl),
                status: CommissionStatus::Approved,
                platform_status: String::new(),
                status_since: noon(),
                tracking: None,
                manual: false,
                payout: None,
                linked_product: None,
                channel: None,
                link: Some(RecordId::from_u128(link + 100)),
            })
            .collect();

        let report = ClickReport::of(&clicks, &commissions);

        let sum = |groups: &[ClickGroup<RecordId>]| {
            (
                groups.iter().map(|g| g.clicks).sum::<u64>(),
                groups.iter().map(|g| g.commissions).sum::<usize>(),
            )
        };
        prop_assert_eq!(sum(&report.by_link), sum(&report.by_channel));
        prop_assert_eq!(sum(&report.by_link).0, clicks.iter().map(|l| l.clicks).sum::<u64>());
        let through_short_links = sales
            .iter()
            .filter(|(link, before)| (*link as usize) < clicks.len() && !before)
            .count();
        prop_assert_eq!(sum(&report.by_link).1, through_short_links);
        prop_assert!(report.by_link.windows(2).all(|w| w[0].clicks >= w[1].clicks));
        prop_assert!(report.by_channel.windows(2).all(|w| w[0].clicks >= w[1].clicks));
    }
}
