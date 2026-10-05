//! The owner's Product Ads through Mercado Livre's adapter, against the
//! fake server serving the answers in `fixtures/mercado-livre`, written
//! from the documentation (see its README). What marketing does with them
//! is tested there.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use mascate_integrations::MercadoLivre;
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Currency, Money, PlatformError};
use mascate_marketing::{AdMetrics, CampaignStatus, ChannelAd, ChannelAds, ChannelCampaign};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};
use rust_decimal::Decimal;

const ADVERTISERS: &str = "/advertising/advertisers?product_id=PADS";
const CAMPAIGNS: &str =
    "/advertising/MLB/advertisers/8823/product_ads/campaigns/search?limit=50&offset=0";
const METRICS: &str =
    "clicks%2Cprints%2Ccost%2Cdirect_amount%2Cindirect_amount%2Ctotal_amount%2Cunits_quantity";

fn ads(offset: u32, day: &str) -> String {
    format!(
        "/advertising/MLB/advertisers/8823/product_ads/ads/search?limit=50&offset={offset}\
         &date_from={day}&date_to={day}&metrics={METRICS}"
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
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 5, 15, 0, 0).unwrap(),
        ));
        let adapter = MercadoLivre::new(server.url(), "Mascate/test", store, clock);
        server.serve("/oauth/token", 200, fixture("token.json"));
        Self { server, adapter }
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
        .serve(ADVERTISERS, 404, r#"{"message":"not found","status":404}"#);
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

#[test]
fn a_days_ads_come_from_every_page_with_their_metrics_exact() {
    let fake = Fake::connected();
    fake.server.serve(
        &ads(0, "2026-10-03"),
        200,
        fixture("product-ads-ads-2026-10-03.json"),
    );
    fake.server.serve(
        &ads(2, "2026-10-03"),
        200,
        fixture("product-ads-ads-2026-10-03-end.json"),
    );

    let found = fake.adapter.ads_on("8823", october(3)).unwrap();

    assert_eq!(
        found,
        [
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
            ChannelAd {
                listing: "MLB4100000003".into(),
                campaign: Some("351".into()),
                metrics: AdMetrics::zero(Currency::Brl),
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
        ]
    );
    assert_eq!(fake.versions(&ads(0, "2026-10-03")), [Some("2".into())]);
}

#[test]
fn a_day_without_ads_is_empty() {
    let fake = Fake::connected();
    fake.server.serve(
        &ads(0, "2026-10-04"),
        200,
        fixture("product-ads-ads-empty.json"),
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
        .serve(&ads(0, "2026-10-03"), 429, fixture("error-429.json"));

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
fn a_read_turned_down_for_an_old_token_goes_once_more_with_a_renewed_one() {
    let fake = Fake::connected();
    fake.server
        .serve(ADVERTISERS, 401, r#"{"message":"invalid_token"}"#);
    fake.server
        .then_serve(ADVERTISERS, 200, fixture("advertisers-pads.json"));

    assert_eq!(fake.adapter.account().unwrap(), Some("8823".into()));
    assert_eq!(fake.versions(ADVERTISERS).len(), 2);
}
