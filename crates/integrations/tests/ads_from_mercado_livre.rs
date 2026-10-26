//! The owner's Product Ads through Mercado Livre's adapter, against the
//! fake server serving the answers in `fixtures/mercado-livre`, written
//! from the documentation (see its README), and a Sync of marketing's
//! Product Ads over them. The rest of what marketing does with them is
//! tested there.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use futures::executor::block_on;
use mascate_integrations::MercadoLivre;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, PlatformError, Timestamp};
use mascate_marketing::{
    ADS_HISTORY_DAYS, AdDay, AdMetrics, AdsError, CampaignStatus, ChannelAd, ChannelAds,
    ChannelCampaign, MIGRATIONS, ProductAds,
};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;

const ADVERTISERS: &str = "/advertising/advertisers?product_id=PADS";
const CAMPAIGNS: &str =
    "/advertising/MLB/advertisers/8823/product_ads/campaigns/search?limit=50&offset=0";
const METRICS: &str =
    "clicks%2Cprints%2Ccost%2Cdirect_amount%2Cindirect_amount%2Ctotal_amount%2Cunits_quantity";

const GROUPS: &str = "/advertising/MLB/advertisers/8823/product_ads/ad_groups/search";

/// Removed by Mercado Livre on 2026-05-30; the fake answers it 404.
const REMOVED_ADS_SEARCH: &str = "/advertising/MLB/advertisers/8823/product_ads/ads/search";

fn day_query(offset: u32, day: &str) -> String {
    format!("limit=50&offset={offset}&date_from={day}&date_to={day}&metrics={METRICS}")
}

fn groups(offset: u32, day: &str) -> String {
    format!("{GROUPS}?{}", day_query(offset, day))
}

fn group_ads(group: &str, day: &str) -> String {
    format!(
        "/advertising/MLB/product_ads/ad_groups/{group}/ads?{}",
        day_query(0, day)
    )
}

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/mercado-livre/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

fn october(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
}

fn now() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 5, 15, 0, 0).unwrap()
}

struct Fake {
    server: FakeHttpServer,
    adapter: MercadoLivre,
}

impl Fake {
    fn connected() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            ("ML_CLIENT_ID", "1234567890"),
            ("ML_CLIENT_SECRET", "s3cr3t"),
            ("ML_REFRESH_TOKEN", "TG-first"),
        ] {
            store.write(name, &Secret::new(value)).unwrap();
        }
        let clock = Arc::new(ManualClock::at(now()));
        let adapter = MercadoLivre::new(server.url(), "Mascate/test", store, clock);
        server.serve("/oauth/token", 200, fixture("token.json"));
        server.serve_any_query(
            REMOVED_ADS_SEARCH,
            404,
            r#"{"message":"not found","status":404}"#,
        );
        Self { server, adapter }
    }

    /// The ad groups of 3 October 2026 in two pages, with the ads of the
    /// family and the catalog product that spent or sold that day.
    fn with_october_3(self) -> Self {
        let day = "2026-10-03";
        for (path, name) in [
            (groups(0, day), "product-ads-ad-groups-2026-10-03.json"),
            (groups(2, day), "product-ads-ad-groups-2026-10-03-end.json"),
            (
                group_ads("1406300002", day),
                "product-ads-ad-group-1406300002-ads-2026-10-03.json",
            ),
            (
                group_ads("1406300005", day),
                "product-ads-ad-group-1406300005-ads-2026-10-03.json",
            ),
        ] {
            self.server.serve(&path, 200, fixture(name));
        }
        self
    }

    /// Whether any request went to `path`, whatever its query.
    fn asked(&self, path: &str) -> bool {
        self.server
            .requests()
            .iter()
            .any(|requested| requested.split('?').next() == Some(path))
    }

    /// The `api-version` header of each request to `path`.
    fn versions(&self, path: &str) -> Vec<Option<String>> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.path == path)
            .map(|request| {
                request
                    .headers
                    .into_iter()
                    .find(|(name, _)| name == "api-version")
                    .map(|(_, value)| value)
            })
            .collect()
    }
}

#[test]
fn the_account_is_the_product_ads_advertiser_of_mercado_livre_brasil() {
    let fake = Fake::connected();
    fake.server
        .serve(ADVERTISERS, 200, fixture("advertisers-pads.json"));

    assert_eq!(fake.adapter.account().unwrap(), Some("8823".into()));
    assert_eq!(fake.versions(ADVERTISERS), [Some("1".into())]);
}

#[test]
fn a_seller_product_ads_never_unlocked_for_has_no_account() {
    let fake = Fake::connected();
    fake.server
        .serve(ADVERTISERS, 200, fixture("advertisers-empty.json"));
    assert_eq!(fake.adapter.account().unwrap(), None);

    let fake = Fake::connected();
    fake.server
        .serve(ADVERTISERS, 404, fixture("error-404-no-product-ads.json"));
    assert_eq!(fake.adapter.account().unwrap(), None);
}

#[test]
fn the_campaigns_come_with_their_status_and_a_name_even_without_one() {
    let fake = Fake::connected();
    fake.server
        .serve(CAMPAIGNS, 200, fixture("product-ads-campaigns.json"));

    let campaigns = fake.adapter.campaigns("8823").unwrap();

    assert_eq!(
        campaigns,
        [
            ChannelCampaign {
                id: "351".into(),
                name: "Fones".into(),
                status: CampaignStatus::Active,
            },
            ChannelCampaign {
                id: "352".into(),
                name: "Cabos".into(),
                status: CampaignStatus::Paused,
            },
            ChannelCampaign {
                id: "353".into(),
                name: "Campanha 353".into(),
                status: CampaignStatus::Other,
            },
        ]
    );
    assert_eq!(fake.versions(CAMPAIGNS), [Some("2".into())]);
}

/// The listings' metrics of 3 October 2026, as the fixtures report them.
fn october_3_ads() -> Vec<ChannelAd> {
    vec![
        ChannelAd {
            listing: "MLB4100000001".into(),
            campaign: Some("351".into()),
            metrics: AdMetrics {
                cost: brl("12.4"),
                attributed: brl("179.8"),
                clicks: 31,
                prints: 1240,
                units: 2,
            },
        },
        // The family's variations, each with its own metrics.
        ChannelAd {
            listing: "MLB4100000004".into(),
            campaign: Some("351".into()),
            metrics: AdMetrics {
                cost: brl("5"),
                attributed: brl("59.9"),
                clicks: 12,
                prints: 600,
                units: 1,
            },
        },
        ChannelAd {
            listing: "MLB4100000005".into(),
            campaign: Some("351".into()),
            metrics: AdMetrics {
                cost: brl("2.5"),
                attributed: brl("59.9"),
                clicks: 6,
                prints: 300,
                units: 1,
            },
        },
        // Without `total_amount`, the direct and indirect sales together.
        ChannelAd {
            listing: "MLB4100000002".into(),
            campaign: Some("352".into()),
            metrics: AdMetrics {
                cost: brl("3.3"),
                attributed: brl("29.9"),
                clicks: 8,
                prints: 320,
                units: 1,
            },
        },
        // A catalog product that sold without spending is still split.
        ChannelAd {
            listing: "MLB4100000006".into(),
            campaign: Some("352".into()),
            metrics: AdMetrics {
                cost: brl("0"),
                attributed: brl("89.9"),
                clicks: 0,
                prints: 15,
                units: 1,
            },
        },
    ]
}

#[test]
fn a_days_ad_groups_come_from_every_page_and_families_split_into_their_listings() {
    let fake = Fake::connected().with_october_3();

    let found = fake.adapter.ads_on("8823", october(3)).unwrap();

    assert_eq!(found, october_3_ads());
    assert_eq!(fake.versions(&groups(0, "2026-10-03")), [Some("2".into())]);
    assert_eq!(
        fake.versions(&group_ads("1406300002", "2026-10-03")),
        [Some("2".into())]
    );
}

#[test]
fn a_family_that_neither_spent_nor_sold_that_day_costs_no_call() {
    let fake = Fake::connected().with_october_3();

    fake.adapter.ads_on("8823", october(3)).unwrap();

    assert!(!fake.asked("/advertising/MLB/product_ads/ad_groups/1406300004/ads"));
    assert!(!fake.asked("/advertising/MLB/product_ads/ad_groups/1406300001/ads"));
    assert!(!fake.asked(REMOVED_ADS_SEARCH));
}

#[test]
fn a_day_without_ad_groups_is_empty() {
    let fake = Fake::connected();
    fake.server.serve(
        &groups(0, "2026-10-04"),
        200,
        fixture("product-ads-ad-groups-empty.json"),
    );

    assert!(fake.adapter.ads_on("8823", october(4)).unwrap().is_empty());
}

#[test]
fn an_account_id_that_could_leave_the_path_is_never_requested() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter.campaigns("8823/../../users/me"),
        Err(PlatformError::NotFound)
    );
    assert_eq!(
        fake.adapter.ads_on("8823?x=1", october(3)),
        Err(PlatformError::NotFound)
    );
    assert!(fake.server.requests().is_empty());
}

#[test]
fn a_busy_or_refusing_channel_fails_the_read() {
    let fake = Fake::connected();
    fake.server
        .serve(ADVERTISERS, 429, fixture("error-429.json"));
    fake.server.serve(CAMPAIGNS, 403, fixture("error-403.json"));
    fake.server
        .serve(&groups(0, "2026-10-03"), 429, fixture("error-429.json"));

    assert_eq!(fake.adapter.account(), Err(PlatformError::RateLimited));
    assert!(matches!(
        fake.adapter.campaigns("8823"),
        Err(PlatformError::Refused(_))
    ));
    assert_eq!(
        fake.adapter.ads_on("8823", october(3)),
        Err(PlatformError::RateLimited)
    );
}

#[test]
fn a_family_whose_ads_cannot_be_read_fails_the_whole_day() {
    let fake = Fake::connected().with_october_3();
    fake.server.serve(
        &group_ads("1406300002", "2026-10-03"),
        429,
        fixture("error-429.json"),
    );

    assert_eq!(
        fake.adapter.ads_on("8823", october(3)),
        Err(PlatformError::RateLimited)
    );
}

#[test]
fn a_read_turned_down_for_an_old_token_goes_once_more_with_a_renewed_one() {
    let fake = Fake::connected();
    fake.server
        .serve(ADVERTISERS, 401, r#"{"message":"invalid_token"}"#);
    fake.server
        .then_serve(ADVERTISERS, 200, fixture("advertisers-pads.json"));

    assert_eq!(fake.adapter.account().unwrap(), Some("8823".into()));
    assert_eq!(fake.versions(ADVERTISERS).len(), 2);

    let fake = Fake::connected().with_october_3();
    let first_page = groups(0, "2026-10-03");
    fake.server
        .serve(&first_page, 401, r#"{"message":"invalid_token"}"#);
    fake.server.then_serve(
        &first_page,
        200,
        fixture("product-ads-ad-groups-2026-10-03.json"),
    );

    assert_eq!(
        fake.adapter.ads_on("8823", october(3)).unwrap(),
        october_3_ads()
    );
    assert_eq!(fake.versions(&first_page).len(), 2);
}

/// Marketing's Product Ads over a real temporary database, read through the
/// adapter: every day of the first Sync has no ad groups but 3 October.
struct Synced {
    _dir: tempfile::TempDir,
    ads: ProductAds,
    fake: Fake,
}

impl Synced {
    fn new() -> Self {
        let fake = Fake::connected().with_october_3();
        fake.server
            .serve(ADVERTISERS, 200, fixture("advertisers-pads.json"));
        fake.server
            .serve(CAMPAIGNS, 200, fixture("product-ads-campaigns.json"));
        fake.server
            .serve_any_query(GROUPS, 200, fixture("product-ads-ad-groups-empty.json"));
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let database = Arc::new(
                Database::open(&dir.path().join("mascate.db"))
                    .await
                    .unwrap(),
            );
            let clock = Arc::new(ManualClock::at(now()));
            migrate(&database, clock.as_ref(), &[MIGRATIONS])
                .await
                .unwrap();
            Self {
                _dir: dir,
                ads: ProductAds::new(database, clock, Arc::new(SequentialIds::default())),
                fake,
            }
        })
    }

    fn sync(&self) -> Result<usize, AdsError> {
        block_on(self.ads.sync(&self.fake.adapter)).map(|read| read.ads)
    }

    fn costs(&self) -> Vec<AdDay> {
        block_on(self.ads.costs()).unwrap()
    }
}

fn october_3_costs() -> Vec<AdDay> {
    let mut costs: Vec<AdDay> = october_3_ads()
        .into_iter()
        .map(|ad| AdDay {
            listing: ad.listing,
            day: october(3),
            cost: ad.metrics.cost,
        })
        .collect();
    costs.sort_by(|left, right| left.listing.cmp(&right.listing));
    costs
}

#[test]
fn a_sync_keeps_each_listings_cost_of_the_day_and_reading_it_again_changes_nothing() {
    let synced = Synced::new();

    assert_eq!(synced.sync().unwrap(), october_3_ads().len());
    let first = synced.costs();
    assert_eq!(first, october_3_costs());
    let days_asked = synced
        .fake
        .server
        .requests()
        .iter()
        .filter(|path| path.starts_with(GROUPS))
        .count();
    assert_eq!(days_asked, usize::try_from(ADS_HISTORY_DAYS).unwrap() + 1);

    synced.sync().unwrap();
    assert_eq!(synced.costs(), first);
    assert!(!synced.fake.asked(REMOVED_ADS_SEARCH));
}

#[test]
fn a_page_that_fails_keeps_nothing_and_the_next_sync_reads_the_day_again() {
    let synced = Synced::new();
    let family = group_ads("1406300002", "2026-10-03");
    synced
        .fake
        .server
        .serve(&family, 429, fixture("error-429.json"));

    assert!(matches!(
        synced.sync(),
        Err(AdsError::Platform(PlatformError::RateLimited))
    ));
    assert!(synced.costs().is_empty());
    assert_eq!(block_on(synced.ads.last_sync()).unwrap(), None);

    synced.fake.server.serve(
        &family,
        200,
        fixture("product-ads-ad-group-1406300002-ads-2026-10-03.json"),
    );
    synced.sync().unwrap();
    assert_eq!(synced.costs(), october_3_costs());
}
