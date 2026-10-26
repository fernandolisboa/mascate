//! The owner's Product Ads through Mercado Livre's adapter, against the
//! fake server serving the answers in `fixtures/mercado-livre`, written
//! from the documentation (see its README): reading them, writing campaigns,
//! and a Sync and a campaign change of marketing's over them. The rest of
//! what marketing does with them is tested there.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use futures::executor::block_on;
use mascate_integrations::MercadoLivre;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, PlatformError, Roas, Timestamp};
use mascate_marketing::{
    ADS_HISTORY_DAYS, AdCampaigns, AdChange, AdDay, AdGroupKind, AdMetrics, AdState, AdStatus,
    AdsError, CampaignPlan, CampaignStatus, CampaignStrategy, ChannelAd, ChannelAdCampaigns,
    ChannelAdGroup, ChannelAds, ChannelCampaign, MIGRATIONS, ProductAds,
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

/// Every Ad Group of the account, with no dates or metrics.
const ALL_GROUPS: &str =
    "/advertising/MLB/advertisers/8823/product_ads/ad_groups/search?limit=50&offset=0";
const FAMILY_ADS: &str = "/advertising/MLB/product_ads/ad_groups/1406300002/ads?limit=50&offset=0";
const CAPA_GROUP: &str = "/advertising/MLB/product_ads/ad_groups/1406300006";
const FAMILY_GROUP: &str = "/advertising/MLB/product_ads/ad_groups/1406300002";
const CAPA_OUT_OF_FONES: &str = "/advertising/MLB/product_ads/campaigns/351/ad_groups/1406300006";

const CREATE: &str = "/advertising/MLB/advertisers/8823/product_ads/campaigns";
const FONES: &str = "/advertising/MLB/product_ads/campaigns/351";

/// Removed by Mercado Livre on 2026-05-30; the fake answers it 404.
const REMOVED_ADS_SEARCH: &str = "/advertising/MLB/advertisers/8823/product_ads/ads/search";

/// The search for the Ad Group of `listing`.
fn group_of(listing: &str) -> String {
    format!("{GROUPS}?limit=50&offset=0&filters%5Bitem_ids%5D={listing}")
}

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

fn roas(ratio: &str) -> Roas {
    Roas::new(Decimal::from_str(ratio).unwrap()).unwrap()
}

fn cabos() -> CampaignPlan {
    CampaignPlan {
        name: "Cabos USB".into(),
        status: CampaignStatus::Active,
        budget: brl("19.995"),
        strategy: CampaignStrategy::Profitability,
        target: roas("4.4"),
    }
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

    /// The bodies sent with `method` to `path`, as sent.
    fn sent(&self, method: &str, path: &str) -> Vec<String> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.method == method && request.path == path)
            .map(|request| request.body)
            .collect()
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
fn the_campaigns_come_with_their_status_budget_strategy_and_target_and_a_name_even_without_one() {
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
                budget: Some(brl("30")),
                strategy: Some(CampaignStrategy::Profitability),
                target: Some(roas("5")),
            },
            ChannelCampaign {
                id: "352".into(),
                name: "Cabos".into(),
                status: CampaignStatus::Paused,
                budget: Some(brl("10")),
                strategy: Some(CampaignStrategy::Visibility),
                target: Some(roas("2.86")),
            },
            ChannelCampaign {
                budget: Some(brl("5")),
                ..ChannelCampaign::named("353", "Campanha 353", CampaignStatus::Other)
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
    campaigns: AdCampaigns,
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
            let ids = Arc::new(SequentialIds::default());
            Self {
                _dir: dir,
                ads: ProductAds::new(database.clone(), clock.clone(), ids.clone()),
                campaigns: AdCampaigns::new(database, clock, ids),
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
        .filter(|path| path.starts_with(GROUPS) && path.contains("date_from"))
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

#[test]
fn a_new_campaign_posts_the_whole_plan_in_reais_and_comes_back_as_mercado_livre_keeps_it() {
    let fake = Fake::connected();
    fake.server.serve_method(
        "POST",
        CREATE,
        201,
        fixture("product-ads-campaign-created.json"),
    );

    let created = fake.adapter.create_campaign("8823", &cabos()).unwrap();

    assert_eq!(
        fake.sent("POST", CREATE),
        [
            r#"{"name":"Cabos USB","status":"active","budget":20.00,"strategy":"profitability","roas_target":4.4,"channel":"marketplace"}"#
        ]
    );
    assert_eq!(fake.versions(CREATE), [Some("2".into())]);
    assert_eq!(
        created,
        ChannelCampaign {
            id: "355328313".into(),
            name: "Cabos USB".into(),
            status: CampaignStatus::Active,
            budget: Some(brl("20")),
            strategy: Some(CampaignStrategy::Profitability),
            target: Some(roas("4.4")),
        }
    );
}

#[test]
fn a_change_puts_the_whole_plan_and_a_pause_only_the_status() {
    let fake = Fake::connected();
    fake.server.serve_method(
        "PUT",
        FONES,
        200,
        fixture("product-ads-campaign-351-changed.json"),
    );
    let plan = CampaignPlan {
        name: "Fones".into(),
        budget: brl("35.5"),
        ..cabos()
    };

    let changed = fake.adapter.change_campaign("351", &plan).unwrap();
    assert_eq!(changed.budget, Some(brl("35.5")));

    fake.server.serve_method(
        "PUT",
        FONES,
        200,
        fixture("product-ads-campaign-351-paused.json"),
    );
    let paused = fake
        .adapter
        .set_campaign_status("351", CampaignStatus::Paused)
        .unwrap();
    assert_eq!(paused.status, CampaignStatus::Paused);

    assert_eq!(
        fake.sent("PUT", FONES),
        [
            r#"{"name":"Fones","status":"active","budget":35.5,"strategy":"profitability","roas_target":4.4}"#,
            r#"{"status":"paused"}"#,
        ]
    );
    assert_eq!(fake.versions(FONES), [Some("2".into()), Some("2".into())]);
    assert!(fake.sent("POST", CREATE).is_empty());
}

#[test]
fn an_account_without_campaign_management_by_api_answers_not_found_and_the_read_goes_on() {
    let fake = Fake::connected();
    fake.server.serve_method(
        "POST",
        CREATE,
        403,
        fixture("error-403-product-ads-campaigns.json"),
    );
    fake.server
        .serve_method("PUT", FONES, 404, r#"{"message":"not found","status":404}"#);
    fake.server
        .serve(CAMPAIGNS, 200, fixture("product-ads-campaigns.json"));

    assert_eq!(
        fake.adapter.create_campaign("8823", &cabos()),
        Err(PlatformError::NotFound)
    );
    assert_eq!(
        fake.adapter
            .set_campaign_status("351", CampaignStatus::Paused),
        Err(PlatformError::NotFound)
    );
    assert_eq!(fake.adapter.campaigns("8823").unwrap().len(), 3);
}

#[test]
fn a_busy_channel_or_a_refused_plan_is_not_taken_as_done() {
    let fake = Fake::connected();
    fake.server
        .serve_method("POST", CREATE, 429, fixture("error-429.json"));
    fake.server.serve_method(
        "PUT",
        FONES,
        400,
        fixture("error-400-product-ads-budget.json"),
    );

    assert_eq!(
        fake.adapter.create_campaign("8823", &cabos()),
        Err(PlatformError::RateLimited)
    );
    assert_eq!(
        fake.adapter.change_campaign("351", &cabos()),
        Err(PlatformError::Refused(
            "The budget is below the minimum for the site.".into()
        ))
    );
}

#[test]
fn a_campaign_write_turned_down_for_an_old_token_goes_once_more_with_a_renewed_one() {
    let fake = Fake::connected();
    fake.server
        .serve_method("POST", CREATE, 401, r#"{"message":"invalid_token"}"#);
    fake.server.then_serve(
        &format!("POST {CREATE}"),
        201,
        fixture("product-ads-campaign-created.json"),
    );

    fake.adapter.create_campaign("8823", &cabos()).unwrap();

    assert_eq!(fake.sent("POST", CREATE).len(), 2);
}

#[test]
fn a_campaign_id_that_could_leave_the_path_or_a_status_mercado_livre_lacks_is_never_sent() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter.change_campaign("351/../../users/me", &cabos()),
        Err(PlatformError::NotFound)
    );
    assert_eq!(
        fake.adapter.create_campaign("8823?x=1", &cabos()),
        Err(PlatformError::NotFound)
    );
    assert!(matches!(
        fake.adapter
            .set_campaign_status("351", CampaignStatus::Other),
        Err(PlatformError::Refused(_))
    ));
    let dollars = CampaignPlan {
        budget: Money::new(Decimal::from(20), Currency::Usd),
        ..cabos()
    };
    assert!(matches!(
        fake.adapter.create_campaign("8823", &dollars),
        Err(PlatformError::Refused(_))
    ));
    assert!(fake.server.requests().is_empty());
}

#[test]
fn a_create_whose_answer_never_arrived_is_found_by_name_and_not_made_twice() {
    let synced = Synced::new();
    synced.sync().unwrap();
    synced.fake.server.hang_up(CREATE);
    let request = block_on(synced.campaigns.prepare_campaign(cabos(), &[], true)).unwrap();

    let lost = block_on(
        synced
            .campaigns
            .send(&synced.fake.adapter, request.clone().confirm().unwrap()),
    );
    assert!(matches!(
        lost,
        Err(AdsError::Platform(PlatformError::Failed(_)))
    ));

    synced.fake.server.serve(
        CAMPAIGNS,
        200,
        fixture("product-ads-campaigns-after-create.json"),
    );
    let found = block_on(
        synced
            .campaigns
            .send(&synced.fake.adapter, request.confirm().unwrap()),
    )
    .unwrap();

    assert_eq!(found.id, "355328313");
    assert_eq!(synced.fake.sent("POST", CREATE).len(), 1);
    assert!(
        block_on(synced.campaigns.campaigns())
            .unwrap()
            .iter()
            .any(|campaign| campaign.id == "355328313")
    );
}

/// The account's Ad Groups as the fixtures report them.
fn ad_groups() -> Vec<ChannelAdGroup> {
    vec![
        ChannelAdGroup {
            id: "1406300001".into(),
            kind: AdGroupKind::Item,
            title: Some("Fone de Ouvido Bluetooth TWS".into()),
            listings: vec!["MLB4100000001".into()],
            campaign: Some("351".into()),
            status: AdStatus::Active,
        },
        fone_family(),
        ChannelAdGroup {
            id: "1406300006".into(),
            kind: AdGroupKind::Item,
            title: Some("Capa de Silicone iPhone 15".into()),
            listings: vec!["MLB4100000003".into()],
            campaign: None,
            status: AdStatus::Paused,
        },
    ]
}

fn fone_family() -> ChannelAdGroup {
    ChannelAdGroup {
        id: "1406300002".into(),
        kind: AdGroupKind::Family,
        title: Some("Fone de Ouvido Over-ear".into()),
        listings: vec!["MLB4100000004".into(), "MLB4100000005".into()],
        campaign: Some("351".into()),
        status: AdStatus::Active,
    }
}

impl Fake {
    /// The account's Ad Groups, with the family's ads.
    fn with_ad_groups(self) -> Self {
        self.server
            .serve(ALL_GROUPS, 200, fixture("product-ads-ad-groups.json"));
        self.server.serve(
            FAMILY_ADS,
            200,
            fixture("product-ads-ad-group-1406300002-ads.json"),
        );
        self
    }
}

#[test]
fn the_ad_groups_come_with_their_listings_a_family_listing_its_variations() {
    let fake = Fake::connected().with_ad_groups();

    assert_eq!(fake.adapter.ad_groups("8823").unwrap(), ad_groups());
    assert_eq!(fake.versions(ALL_GROUPS), [Some("2".into())]);
    assert_eq!(fake.versions(FAMILY_ADS), [Some("2".into())]);
}

#[test]
fn a_listings_ad_group_is_found_by_the_search_by_item() {
    let fake = Fake::connected().with_ad_groups();
    fake.server.serve(
        &group_of("MLB4100000003"),
        200,
        fixture("product-ads-ad-groups-MLB4100000003.json"),
    );
    fake.server.serve(
        &group_of("MLB4100000004"),
        200,
        fixture("product-ads-ad-groups-MLB4100000004.json"),
    );
    fake.server.serve(
        &group_of("MLB4199999999"),
        200,
        fixture("product-ads-ad-groups-empty.json"),
    );

    let capa = fake
        .adapter
        .ad_group_of("8823", "MLB4100000003")
        .unwrap()
        .unwrap();
    assert_eq!(capa.id, "1406300006");
    assert_eq!(capa.listings, ["MLB4100000003"]);
    assert_eq!(capa.campaign, None);
    assert_eq!(capa.status, AdStatus::Paused);

    let family = fake
        .adapter
        .ad_group_of("8823", "MLB4100000004")
        .unwrap()
        .unwrap();
    assert_eq!(
        family,
        ChannelAdGroup {
            title: None,
            ..fone_family()
        }
    );

    assert_eq!(fake.adapter.ad_group_of("8823", "MLB4199999999"), Ok(None));
}

#[test]
fn placing_an_ad_group_puts_its_status_and_campaign() {
    let fake = Fake::connected();
    fake.server.serve_method(
        "PUT",
        CAPA_GROUP,
        200,
        fixture("product-ads-ad-group-1406300006-placed.json"),
    );

    let placed = fake
        .adapter
        .place_ad_group("1406300006", "352", AdStatus::Active)
        .unwrap();

    assert_eq!(
        placed,
        AdState {
            campaign: Some("352".into()),
            status: AdStatus::Active,
        }
    );
    assert_eq!(
        fake.sent("PUT", CAPA_GROUP),
        [r#"{"status":"active","campaign_id":352}"#]
    );
    assert_eq!(fake.versions(CAPA_GROUP), [Some("2".into())]);
}

#[test]
fn taking_out_an_ad_group_deletes_it_from_its_campaign_and_one_already_out_is_out() {
    let fake = Fake::connected();
    fake.server
        .serve_method("DELETE", CAPA_OUT_OF_FONES, 204, "");
    assert_eq!(fake.adapter.remove_ad_group("351", "1406300006"), Ok(()));
    assert_eq!(fake.versions(CAPA_OUT_OF_FONES), [Some("2".into())]);

    fake.server.serve_method(
        "DELETE",
        CAPA_OUT_OF_FONES,
        404,
        fixture("error-404-product-ads-ad-group.json"),
    );
    assert_eq!(fake.adapter.remove_ad_group("351", "1406300006"), Ok(()));
}

#[test]
fn an_ad_group_write_the_account_cannot_make_by_api_answers_not_found() {
    let fake = Fake::connected();
    fake.server.serve_method(
        "PUT",
        CAPA_GROUP,
        403,
        fixture("error-403-product-ads-ad-group.json"),
    );
    fake.server.serve_method(
        "DELETE",
        CAPA_OUT_OF_FONES,
        403,
        fixture("error-403-product-ads-ad-group.json"),
    );

    assert_eq!(
        fake.adapter
            .place_ad_group("1406300006", "352", AdStatus::Active),
        Err(PlatformError::NotFound)
    );
    assert_eq!(
        fake.adapter.remove_ad_group("351", "1406300006"),
        Err(PlatformError::NotFound)
    );
}

#[test]
fn a_busy_channel_takes_no_ad_group_write_as_done() {
    let fake = Fake::connected();
    fake.server
        .serve_method("PUT", CAPA_GROUP, 429, fixture("error-429.json"));
    fake.server
        .serve_method("DELETE", CAPA_OUT_OF_FONES, 429, fixture("error-429.json"));
    fake.server
        .serve(&group_of("MLB4100000003"), 429, fixture("error-429.json"));

    assert_eq!(
        fake.adapter
            .place_ad_group("1406300006", "352", AdStatus::Active),
        Err(PlatformError::RateLimited)
    );
    assert_eq!(
        fake.adapter.remove_ad_group("351", "1406300006"),
        Err(PlatformError::RateLimited)
    );
    assert_eq!(
        fake.adapter.ad_group_of("8823", "MLB4100000003"),
        Err(PlatformError::RateLimited)
    );
}

#[test]
fn an_ad_group_write_turned_down_for_an_old_token_goes_once_more_with_a_renewed_one() {
    let fake = Fake::connected();
    fake.server
        .serve_method("PUT", CAPA_GROUP, 401, r#"{"message":"invalid_token"}"#);
    fake.server.then_serve(
        &format!("PUT {CAPA_GROUP}"),
        200,
        fixture("product-ads-ad-group-1406300006-placed.json"),
    );

    fake.adapter
        .place_ad_group("1406300006", "352", AdStatus::Active)
        .unwrap();

    assert_eq!(fake.sent("PUT", CAPA_GROUP).len(), 2);
}

#[test]
fn an_ad_group_or_campaign_id_that_could_leave_the_path_is_never_sent() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter
            .place_ad_group("1406300006/../../users/me", "352", AdStatus::Active),
        Err(PlatformError::NotFound)
    );
    assert!(matches!(
        fake.adapter
            .place_ad_group("1406300006", "352,\"x\":1", AdStatus::Active),
        Err(PlatformError::Refused(_))
    ));
    assert!(matches!(
        fake.adapter
            .place_ad_group("1406300006", "352", AdStatus::Other),
        Err(PlatformError::Refused(_))
    ));
    assert_eq!(
        fake.adapter.remove_ad_group("351?x=1", "1406300006"),
        Err(PlatformError::NotFound)
    );
    assert_eq!(
        fake.adapter.ad_group_of("8823/..", "MLB4100000003"),
        Err(PlatformError::NotFound)
    );
    assert!(fake.server.requests().is_empty());
}

#[test]
fn a_sync_keeps_the_ad_groups_and_a_family_moves_whole_through_marketing() {
    let synced = Synced::new();
    synced
        .fake
        .server
        .serve(ALL_GROUPS, 200, fixture("product-ads-ad-groups.json"));
    synced.fake.server.serve(
        FAMILY_ADS,
        200,
        fixture("product-ads-ad-group-1406300002-ads.json"),
    );
    synced.fake.server.serve(
        &group_of("MLB4100000004"),
        200,
        fixture("product-ads-ad-groups-MLB4100000004.json"),
    );
    synced.fake.server.serve_method(
        "PUT",
        FAMILY_GROUP,
        200,
        fixture("product-ads-ad-group-1406300002-moved.json"),
    );
    synced.sync().unwrap();
    assert_eq!(block_on(synced.campaigns.ad_groups()).unwrap(), ad_groups());

    let request = block_on(synced.campaigns.prepare_ad(
        "MLB4100000005",
        AdChange::Place {
            campaign: "352".into(),
            status: AdStatus::Paused,
        },
        true,
    ))
    .unwrap();
    assert_eq!(request.listings(), ["MLB4100000004", "MLB4100000005"]);
    let moved = block_on(
        synced
            .campaigns
            .send_ad(&synced.fake.adapter, request.confirm()),
    )
    .unwrap();

    assert_eq!(
        synced.fake.sent("PUT", FAMILY_GROUP),
        [r#"{"status":"paused","campaign_id":352}"#]
    );
    assert_eq!(moved.campaign.as_deref(), Some("352"));
    assert_eq!(moved.status, AdStatus::Paused);
    assert_eq!(moved.listings, ["MLB4100000004", "MLB4100000005"]);
    assert_eq!(moved.title.as_deref(), Some("Fone de Ouvido Over-ear"));
}
