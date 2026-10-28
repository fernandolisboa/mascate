//! Promotion Spends through the Marketing module's public interface with a
//! real temporary database: typed ones kept, edited and deleted with a
//! warning when their days cross another spend of the channel; report days
//! imported once per campaign and day, replaced by a later report and never
//! counted twice; each report campaign linked to its channel once.

use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{ChannelSpend, Currency, Money, RecordId};
use mascate_marketing::{
    CampaignLink, ChannelKind, ImportedSpends, MIGRATIONS, NewPromotionChannel, NewPromotionSpend,
    PromotionChannel, PromotionChannels, PromotionSpends, ReportedSpend, SpendError, SpendOrigin,
    SpendReport, suggested_tag,
};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

struct Fixture {
    _dir: tempfile::TempDir,
    channels: PromotionChannels,
    spends: PromotionSpends,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap(),
        ));
        let ids = Arc::new(SequentialIds::default());
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        Self {
            _dir: dir,
            channels: PromotionChannels::new(database.clone(), clock.clone(), ids.clone()),
            spends: PromotionSpends::new(database, clock, ids),
        }
    }

    async fn channel(&self, name: &str, kind: ChannelKind) -> PromotionChannel {
        self.channels
            .create(&NewPromotionChannel {
                name: name.into(),
                kind,
                default_tag: suggested_tag(name),
                keywords: Vec::new(),
                telegram_chat: None,
            })
            .await
            .unwrap()
    }

    async fn total(&self, days: std::ops::RangeInclusive<NaiveDate>) -> Vec<ChannelSpend> {
        self.spends.spent(days).await.unwrap()
    }
}

fn day(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
}

fn brl(amount: &str) -> Money {
    Money::new(amount.parse().unwrap(), Currency::Brl)
}

fn typed(channel: RecordId, first: u32, last: u32, amount: &str) -> NewPromotionSpend {
    NewPromotionSpend {
        channel,
        first_day: day(first),
        last_day: day(last),
        amount: brl(amount),
    }
}

fn meta(campaign: &str, on: u32, amount: &str) -> ReportedSpend {
    ReportedSpend {
        report: SpendReport::MetaAds,
        campaign: campaign.into(),
        day: day(on),
        amount: brl(amount),
    }
}

fn link(campaign: &str, channel: RecordId) -> CampaignLink {
    CampaignLink {
        report: SpendReport::MetaAds,
        campaign: campaign.into(),
        channel,
    }
}

fn imported(added: usize, changed: usize, unchanged: usize) -> ImportedSpends {
    ImportedSpends {
        added,
        changed,
        unchanged,
    }
}

#[test]
fn a_typed_spend_is_kept_edited_and_deleted() {
    block_on(async {
        let f = Fixture::new().await;
        let ads = f.channel("Anúncios Meta", ChannelKind::PaidSocial).await;

        let saved = f
            .spends
            .record(&typed(ads.id, 1, 10, "150.00"))
            .await
            .unwrap();
        assert!(saved.crossing.is_empty());
        assert_eq!(saved.spend.channel, ads.id);
        assert_eq!(saved.spend.origin, SpendOrigin::Typed);
        assert_eq!(saved.spend.campaign, None);
        assert_eq!(saved.spend.days(), 10);
        assert_eq!(saved.spend.amount, brl("150"));
        assert_eq!(
            f.spends.spends(ads.id).await.unwrap(),
            vec![saved.spend.clone()]
        );

        let edited = f
            .spends
            .update(saved.spend.id, &typed(ads.id, 2, 11, "160.50"))
            .await
            .unwrap();
        assert_eq!(edited.spend.id, saved.spend.id);
        assert_eq!(
            (edited.spend.first_day, edited.spend.last_day),
            (day(2), day(11))
        );
        assert_eq!(edited.spend.amount, brl("160.5"));

        f.spends.delete(saved.spend.id).await.unwrap();
        assert!(f.spends.spends(ads.id).await.unwrap().is_empty());
        assert!(matches!(
            f.spends.delete(saved.spend.id).await,
            Err(SpendError::NotFound)
        ));
    });
}

#[test]
fn a_typed_spend_needs_a_live_channel_a_period_and_an_amount_above_zero() {
    block_on(async {
        let f = Fixture::new().await;
        let ads = f.channel("Anúncios Meta", ChannelKind::PaidSocial).await;
        assert!(matches!(
            f.spends.record(&typed(ads.id, 10, 9, "10")).await,
            Err(SpendError::BadPeriod)
        ));
        assert!(matches!(
            f.spends.record(&typed(ads.id, 1, 1, "0")).await,
            Err(SpendError::BadAmount)
        ));
        assert!(matches!(
            f.spends.record(&typed(ads.id, 1, 1, "-5")).await,
            Err(SpendError::BadAmount)
        ));
        f.channels.delete(ads.id).await.unwrap();
        assert!(matches!(
            f.spends.record(&typed(ads.id, 1, 1, "5")).await,
            Err(SpendError::NoSuchChannel)
        ));
    });
}

#[test]
fn a_spend_crossing_another_of_the_same_channel_warns_and_is_still_kept() {
    block_on(async {
        let f = Fixture::new().await;
        let ads = f.channel("Anúncios Meta", ChannelKind::PaidSocial).await;
        let other = f.channel("Google", ChannelKind::PaidSearch).await;
        let first = f.spends.record(&typed(ads.id, 1, 10, "100")).await.unwrap();
        f.spends
            .record(&typed(other.id, 1, 31, "50"))
            .await
            .unwrap();
        f.spends
            .import(&[meta("Curso", 12, "8")], &[link("Curso", ads.id)])
            .await
            .unwrap();

        let apart = f.spends.record(&typed(ads.id, 11, 11, "5")).await.unwrap();
        assert!(apart.crossing.is_empty());

        let crossing = f.spends.record(&typed(ads.id, 10, 12, "30")).await.unwrap();
        let crossed: Vec<(NaiveDate, NaiveDate, SpendOrigin)> = crossing
            .crossing
            .iter()
            .map(|spend| (spend.first_day, spend.last_day, spend.origin))
            .collect();
        assert_eq!(
            crossed,
            vec![
                (day(1), day(10), SpendOrigin::Typed),
                (day(11), day(11), SpendOrigin::Typed),
                (day(12), day(12), SpendOrigin::Report(SpendReport::MetaAds)),
            ]
        );
        assert_eq!(f.spends.spends(ads.id).await.unwrap().len(), 4);
        assert_eq!(crossing.crossing[0].id, first.spend.id);
    });
}

#[test]
fn a_spend_read_from_a_report_changes_only_by_importing_again() {
    block_on(async {
        let f = Fixture::new().await;
        let ads = f.channel("Anúncios Meta", ChannelKind::PaidSocial).await;
        f.spends
            .import(&[meta("Curso", 5, "12.30")], &[link("Curso", ads.id)])
            .await
            .unwrap();
        let read = f.spends.spends(ads.id).await.unwrap().remove(0);
        assert_eq!(read.origin, SpendOrigin::Report(SpendReport::MetaAds));
        assert_eq!(read.campaign.as_deref(), Some("Curso"));
        assert!(matches!(
            f.spends.update(read.id, &typed(ads.id, 5, 5, "1")).await,
            Err(SpendError::FromReport)
        ));
        assert!(matches!(
            f.spends.delete(read.id).await,
            Err(SpendError::FromReport)
        ));
    });
}

#[test]
fn importing_the_same_report_twice_changes_nothing() {
    block_on(async {
        let f = Fixture::new().await;
        let ads = f.channel("Anúncios Meta", ChannelKind::PaidSocial).await;
        let report = [
            meta("Curso de fotografia", 1, "10.50"),
            meta("Curso de fotografia", 2, "12"),
            meta("Ebook", 1, "3.33"),
        ];
        let links = [link("Curso de fotografia", ads.id), link("Ebook", ads.id)];
        assert_eq!(
            f.spends.preview_import(&report, &links).await.unwrap(),
            imported(3, 0, 0)
        );
        assert_eq!(
            f.spends.import(&report, &links).await.unwrap(),
            imported(3, 0, 0)
        );
        let once = f.spends.spends(ads.id).await.unwrap();

        assert_eq!(f.spends.unlinked(&report).await.unwrap(), Vec::new());
        assert_eq!(
            f.spends.import(&report, &[]).await.unwrap(),
            imported(0, 0, 3)
        );
        assert_eq!(f.spends.spends(ads.id).await.unwrap(), once);
        assert_eq!(
            f.total(day(1)..=day(31)).await,
            vec![ChannelSpend {
                channel: ads.id,
                amount: brl("25.83"),
            }]
        );
    });
}

#[test]
fn a_later_report_replaces_a_corrected_day_and_adds_the_new_ones() {
    block_on(async {
        let f = Fixture::new().await;
        let ads = f.channel("Anúncios Meta", ChannelKind::PaidSocial).await;
        f.spends
            .import(
                &[meta("Curso", 1, "10"), meta("Curso", 2, "20")],
                &[link("Curso", ads.id)],
            )
            .await
            .unwrap();
        let overlapping = [meta("Curso", 2, "21.40"), meta("Curso", 3, "30")];
        assert_eq!(
            f.spends.preview_import(&overlapping, &[]).await.unwrap(),
            imported(1, 1, 0)
        );
        assert_eq!(
            f.spends.import(&overlapping, &[]).await.unwrap(),
            imported(1, 1, 0)
        );
        let days: Vec<(NaiveDate, Money)> = f
            .spends
            .spends(ads.id)
            .await
            .unwrap()
            .iter()
            .map(|spend| (spend.first_day, spend.amount))
            .collect();
        assert_eq!(
            days,
            vec![
                (day(3), brl("30")),
                (day(2), brl("21.4")),
                (day(1), brl("10")),
            ]
        );
    });
}

#[test]
fn a_campaign_is_linked_once_and_an_unlinked_one_refuses_the_import() {
    block_on(async {
        let f = Fixture::new().await;
        let ads = f.channel("Anúncios Meta", ChannelKind::PaidSocial).await;
        let report = [
            meta("Curso", 1, "10"),
            meta("Ebook", 1, "5"),
            meta("Curso", 2, "10"),
        ];
        assert_eq!(
            f.spends.unlinked(&report).await.unwrap(),
            vec![
                (SpendReport::MetaAds, "Curso".to_owned()),
                (SpendReport::MetaAds, "Ebook".to_owned()),
            ]
        );
        assert!(matches!(
            f.spends.import(&report, &[link("Curso", ads.id)]).await,
            Err(SpendError::UnlinkedCampaign(campaign)) if campaign == "Ebook"
        ));
        // Nothing of the refused import stayed, not even its link.
        assert!(f.spends.spends(ads.id).await.unwrap().is_empty());
        assert_eq!(f.spends.unlinked(&report).await.unwrap().len(), 2);

        f.spends
            .import(&report, &[link("Curso", ads.id), link("Ebook", ads.id)])
            .await
            .unwrap();
        assert!(f.spends.unlinked(&report).await.unwrap().is_empty());
        // The same name in the other report is another campaign.
        let google = ReportedSpend {
            report: SpendReport::GoogleAds,
            ..meta("Curso", 1, "1")
        };
        assert_eq!(
            f.spends.unlinked(&[google]).await.unwrap(),
            vec![(SpendReport::GoogleAds, "Curso".to_owned())]
        );
    });
}

#[test]
fn a_campaign_whose_channel_was_deleted_is_asked_again_and_its_days_move() {
    block_on(async {
        let f = Fixture::new().await;
        let old = f.channel("Meta antigo", ChannelKind::PaidSocial).await;
        let new = f.channel("Meta novo", ChannelKind::PaidSocial).await;
        f.spends
            .import(&[meta("Curso", 1, "10")], &[link("Curso", old.id)])
            .await
            .unwrap();
        f.channels.delete(old.id).await.unwrap();
        assert!(f.total(day(1)..=day(31)).await.is_empty());

        let report = [meta("Curso", 1, "10")];
        assert_eq!(f.spends.unlinked(&report).await.unwrap().len(), 1);
        assert_eq!(
            f.spends
                .import(&report, &[link("Curso", new.id)])
                .await
                .unwrap(),
            imported(0, 1, 0)
        );
        assert_eq!(f.spends.spends(new.id).await.unwrap().len(), 1);
        assert_eq!(
            f.total(day(1)..=day(31)).await,
            vec![ChannelSpend {
                channel: new.id,
                amount: brl("10"),
            }]
        );
    });
}

#[test]
fn a_report_with_a_negative_amount_or_no_campaign_name_is_refused() {
    block_on(async {
        let f = Fixture::new().await;
        let ads = f.channel("Anúncios Meta", ChannelKind::PaidSocial).await;
        assert!(matches!(
            f.spends
                .import(&[meta("Curso", 1, "-1")], &[link("Curso", ads.id)])
                .await,
            Err(SpendError::BadAmount)
        ));
        assert!(matches!(
            f.spends
                .import(&[meta("  ", 1, "1")], &[link("Curso", ads.id)])
                .await,
            Err(SpendError::BadCampaign)
        ));
        assert!(matches!(
            f.spends.import(&[], &[link(" ", ads.id)]).await,
            Err(SpendError::BadCampaign)
        ));
    });
}

#[test]
fn what_a_channel_spent_counts_the_share_of_a_spend_inside_the_days_unrounded() {
    block_on(async {
        let f = Fixture::new().await;
        let ads = f.channel("Anúncios Meta", ChannelKind::PaidSocial).await;
        let google = f.channel("Google", ChannelKind::PaidSearch).await;
        // R$ 100 over three days: R$ 33,333… a day.
        f.spends.record(&typed(ads.id, 9, 11, "100")).await.unwrap();
        f.spends
            .record(&NewPromotionSpend {
                amount: Money::new(Decimal::new(20, 0), Currency::Usd),
                ..typed(ads.id, 10, 10, "1")
            })
            .await
            .unwrap();
        f.spends
            .record(&typed(google.id, 1, 31, "62"))
            .await
            .unwrap();

        let spent = f.total(day(10)..=day(11)).await;
        let of = |channel: RecordId, currency: Currency| {
            spent
                .iter()
                .find(|spend| spend.channel == channel && spend.amount.currency() == currency)
                .map(|spend| spend.amount.amount())
        };
        assert_eq!(
            of(ads.id, Currency::Brl),
            Some(Decimal::from(100) * Decimal::from(2) / Decimal::from(3))
        );
        assert_eq!(of(ads.id, Currency::Usd), Some(Decimal::from(20)));
        assert_eq!(of(google.id, Currency::Brl), Some(Decimal::from(4)));
        assert_eq!(spent.len(), 3);
        // The whole spend inside the days counts whole.
        assert_eq!(of_whole(&f, ads.id).await, Some(Decimal::from(100)));
    });
}

async fn of_whole(f: &Fixture, channel: RecordId) -> Option<Decimal> {
    f.total(day(1)..=day(31))
        .await
        .into_iter()
        .find(|spend| spend.channel == channel && spend.amount.currency() == Currency::Brl)
        .map(|spend| spend.amount.amount())
}

fn report_days() -> impl Strategy<Value = Vec<(u8, u32, u32)>> {
    prop::collection::vec((0u8..3, 1u32..=28, 0u32..10_000), 0..20)
}

fn as_report(rows: &[(u8, u32, u32)]) -> Vec<ReportedSpend> {
    rows.iter()
        .map(|&(campaign, on, cents)| ReportedSpend {
            amount: Money::new(Decimal::new(i64::from(cents), 2), Currency::Brl),
            ..meta(&format!("Campanha {campaign}"), on, "0")
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// However reports overlap, each campaign's day counts once, with the
    /// amount of the last report that had it.
    #[test]
    fn overlapping_reports_never_count_a_day_twice(
        first in report_days(),
        second in report_days(),
        again in any::<bool>(),
    ) {
        block_on(async {
            let f = Fixture::new().await;
            let ads = f.channel("Anúncios Meta", ChannelKind::PaidSocial).await;
            let links: Vec<CampaignLink> =
                (0..3).map(|n| link(&format!("Campanha {n}"), ads.id)).collect();
            let mut expected = std::collections::BTreeMap::new();
            let mut reports = vec![as_report(&first), as_report(&second)];
            if again {
                reports.push(as_report(&first));
            }
            for report in &reports {
                f.spends.import(report, &links).await.unwrap();
                for spend in report {
                    expected.insert((spend.campaign.clone(), spend.day), spend.amount.amount());
                }
            }
            let kept = f.spends.spends(ads.id).await.unwrap();
            prop_assert_eq!(kept.len(), expected.len());
            let total: Decimal = expected.values().sum();
            let spent = f.total(day(1)..=day(31)).await;
            let counted = spent.first().map_or(Decimal::ZERO, |spend| spend.amount.amount());
            prop_assert_eq!(counted, total);
            Ok(())
        })?;
    }
}
