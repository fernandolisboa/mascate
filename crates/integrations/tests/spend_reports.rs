//! The Meta Ads Manager and Google Ads spend report readers over files
//! written from the documentation (see the fixtures' README), and their
//! import into the Marketing module with a real temporary database.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use futures::executor::block_on;
use mascate_integrations::{SkippedRow, SpendReading, SpendReportError, read_spend_report};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money};
use mascate_marketing::{
    CampaignLink, ChannelKind, ImportedSpends, MIGRATIONS, NewPromotionChannel, PromotionChannels,
    PromotionSpends, ReportedSpend, SpendReport,
};
use mascate_platform::{Database, migrate};
use rust_decimal::Decimal;

const PHOTOGRAPHY: &str = "Curso de fotografia | Conversão";
const EBOOK: &str = "Ebook receitas, tráfego";
const SEARCH: &str = "Pesquisa - Curso de fotografia";
const DISPLAY: &str = "Display - Ebook";

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn read(name: &str) -> SpendReading {
    read_spend_report(&fixture(&format!("spend-reports/{name}"))).unwrap()
}

fn refused(name: &str) -> SpendReportError {
    read_spend_report(&fixture(&format!("spend-reports/{name}"))).unwrap_err()
}

fn day(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
}

fn money(amount: &str, currency: Currency) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), currency)
}

fn spend(report: SpendReport, campaign: &str, on: u32, amount: &str) -> ReportedSpend {
    ReportedSpend {
        report,
        campaign: campaign.into(),
        day: day(on),
        amount: money(amount, Currency::Brl),
    }
}

fn meta_daily() -> Vec<ReportedSpend> {
    let meta = |campaign, on, amount| spend(SpendReport::MetaAds, campaign, on, amount);
    vec![
        meta(PHOTOGRAPHY, 1, "25.4"),
        meta(PHOTOGRAPHY, 2, "27.1"),
        meta(PHOTOGRAPHY, 3, "26"),
        meta(EBOOK, 1, "9.99"),
        meta(EBOOK, 2, "0"),
    ]
}

fn google_daily() -> Vec<ReportedSpend> {
    let google = |campaign, on, amount| spend(SpendReport::GoogleAds, campaign, on, amount);
    vec![
        google(DISPLAY, 2, "0"),
        google(SEARCH, 1, "14.32"),
        google(SEARCH, 2, "1012.5"),
    ]
}

#[test]
fn the_meta_report_by_day_reads_each_campaign_s_day_in_csv_and_xlsx() {
    for name in ["meta-daily.csv", "meta-daily.xlsx"] {
        let reading = read(name);
        assert_eq!(reading.report, SpendReport::MetaAds, "{name}");
        assert_eq!(reading.spends, meta_daily(), "{name}");
        // The XLSX summary row has no campaign: it is a total, not a skip.
        assert!(reading.skipped.is_empty(), "{name}");
        assert_eq!(reading.period(), Some(day(1)..=day(3)), "{name}");
        assert_eq!(reading.campaigns(), vec![PHOTOGRAPHY, EBOOK], "{name}");
        assert_eq!(
            reading.total(),
            vec![money("88.49", Currency::Brl)],
            "{name}"
        );
    }
}

#[test]
fn the_meta_report_in_portuguese_reads_its_day_from_the_reporting_period() {
    let reading = read("meta-daily-pt.csv");
    assert_eq!(reading.report, SpendReport::MetaAds);
    assert_eq!(reading.spends, meta_daily()[..4].to_vec());
}

#[test]
fn ad_sets_of_a_campaign_on_the_same_day_add_up_and_bad_rows_are_left_out() {
    let reading = read("meta-ad-sets.csv");
    assert_eq!(
        reading.spends,
        vec![
            spend(SpendReport::MetaAds, PHOTOGRAPHY, 5, "15"),
            ReportedSpend {
                amount: money("3.1", Currency::Usd),
                ..spend(SpendReport::MetaAds, EBOOK, 5, "0")
            },
        ]
    );
    assert_eq!(
        reading.skipped,
        vec![
            SkippedRow {
                line: 5,
                reason: "valor gasto negativo: -2.00".into(),
            },
            SkippedRow {
                line: 6,
                reason: "valor gasto ilegível: abc".into(),
            },
            SkippedRow {
                line: 7,
                reason: "moeda EUR: o app conta gasto em real (BRL) ou dólar (USD)".into(),
            },
        ]
    );
}

#[test]
fn the_google_ads_report_by_day_reads_each_campaign_s_day_without_the_totals() {
    for name in [
        "google-daily.csv",
        "google-daily-excel.csv",
        "google-daily.xlsx",
    ] {
        let reading = read(name);
        assert_eq!(reading.report, SpendReport::GoogleAds, "{name}");
        assert_eq!(reading.spends, google_daily(), "{name}");
        assert_eq!(reading.skipped, Vec::new(), "{name}");
        assert_eq!(
            reading.total(),
            vec![money("1026.82", Currency::Brl)],
            "{name}"
        );
    }
}

#[test]
fn a_report_without_the_day_breakdown_is_refused_whole() {
    assert_eq!(
        refused("meta-period.csv"),
        SpendReportError::NotDaily(SpendReport::MetaAds)
    );
    assert_eq!(
        refused("google-period.csv"),
        SpendReportError::NotDaily(SpendReport::GoogleAds)
    );
}

#[test]
fn a_report_that_does_not_say_its_currency_is_refused() {
    assert_eq!(
        refused("google-no-currency.csv"),
        SpendReportError::NoCurrency(SpendReport::GoogleAds)
    );
}

#[test]
fn another_file_is_not_a_spend_report() {
    assert_eq!(
        read_spend_report(&fixture("amazon/earnings-2026-09.csv")).unwrap_err(),
        SpendReportError::NotSpendReport
    );
    assert_eq!(
        read_spend_report(b"").unwrap_err(),
        SpendReportError::NotSpendReport
    );
}

#[test]
fn importing_a_report_again_or_a_corrected_one_never_counts_a_day_twice() {
    block_on(async {
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
        let channels = PromotionChannels::new(database.clone(), clock.clone(), ids.clone());
        let spends = PromotionSpends::new(database, clock, ids);
        let channel = channels
            .create(&NewPromotionChannel {
                name: "Anúncios Meta".into(),
                kind: ChannelKind::PaidSocial,
                default_tag: "meta_ads".into(),
                keywords: Vec::new(),
                telegram_chat: None,
            })
            .await
            .unwrap();

        let daily = read("meta-daily.csv");
        let links: Vec<CampaignLink> = spends
            .unlinked(&daily.spends)
            .await
            .unwrap()
            .into_iter()
            .map(|(report, campaign)| CampaignLink {
                report,
                campaign,
                channel: channel.id,
            })
            .collect();
        assert_eq!(links.len(), 2);
        let imported = |added, changed, unchanged| ImportedSpends {
            added,
            changed,
            unchanged,
        };
        assert_eq!(
            spends.import(&daily.spends, &links).await.unwrap(),
            imported(5, 0, 0)
        );
        // The same days from the XLSX and the Portuguese export.
        assert_eq!(
            spends
                .import(&read("meta-daily.xlsx").spends, &[])
                .await
                .unwrap(),
            imported(0, 0, 5)
        );
        assert_eq!(
            spends
                .import(&read("meta-daily-pt.csv").spends, &[])
                .await
                .unwrap(),
            imported(0, 0, 4)
        );
        // The Platform corrected a day.
        let corrected = vec![ReportedSpend {
            amount: money("26.9", Currency::Brl),
            ..meta_daily()[1].clone()
        }];
        assert_eq!(
            spends.import(&corrected, &[]).await.unwrap(),
            imported(0, 1, 0)
        );
        let spent = spends.spent(day(1)..=day(31)).await.unwrap();
        assert_eq!(spent.len(), 1);
        assert_eq!(spent[0].amount, money("88.29", Currency::Brl));
    });
}
