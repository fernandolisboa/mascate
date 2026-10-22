//! Short.io's adapter against the fake server, with answers written from
//! its documentation (see the fixtures' README): the request it sends, the
//! link it keeps, its retries and refusals, and the affiliate module's
//! Short Links end to end on a real temporary database.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    AMAZON_SHORT_LINK, Affiliate, AffiliateError, AffiliatePlatform, CommissionStatus,
    LinkShortener, LinkTagger, MIGRATIONS, NewAffiliateLink, Promotion, ReportedConversion,
    ReportedProgram, Shortened, TagRefusal,
};
use mascate_integrations::{Pause, ShortIo};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, PlatformError, RecordId};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;
use serde_json::{Value, json};

const LINKS: &str = "/links";
const KEY: &str = "sk_test_4kZq2bXm";
const HOTMART_LINK: &str = "https://go.hotmart.com/X12345678A?src=telegram";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/short-io/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// Remembers each pause instead of sleeping.
#[derive(Default)]
struct RecordedPauses(Mutex<Vec<Duration>>);

impl Pause for RecordedPauses {
    fn pause(&self, duration: Duration) {
        self.0.lock().unwrap().push(duration);
    }
}

impl RecordedPauses {
    fn seconds(&self) -> Vec<u64> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(Duration::as_secs)
            .collect()
    }
}

struct Setup {
    server: FakeHttpServer,
    pauses: Arc<RecordedPauses>,
    short_io: ShortIo,
}

/// Short.io at the fake server with the key and `domain` stored.
fn setup_with_domain(domain: &str) -> Setup {
    let server = FakeHttpServer::start();
    let store = Arc::new(MemorySecretStore::default());
    store.write("SHORT_IO_API_KEY", &Secret::new(KEY)).unwrap();
    store
        .write("SHORT_IO_DOMAIN", &Secret::new(domain))
        .unwrap();
    let pauses = Arc::new(RecordedPauses::default());
    let short_io = ShortIo::new(server.url(), "Mascate/test", store).with_pause(pauses.clone());
    Setup {
        server,
        pauses,
        short_io,
    }
}

fn setup() -> Setup {
    setup_with_domain("mascate.short.gy")
}

fn links_asked(server: &FakeHttpServer) -> usize {
    server
        .requests()
        .iter()
        .filter(|path| *path == LINKS)
        .count()
}

#[test]
fn a_link_is_shortened_on_the_owners_domain_with_the_path_asked() {
    let s = setup();
    s.server.serve(LINKS, 200, fixture("link.json"));

    let shortened = s.short_io.shorten(HOTMART_LINK, "0000000b").unwrap();

    assert_eq!(
        shortened,
        Shortened {
            url: "https://mascate.short.gy/0000000b".into(),
            key: "lnk_4kZq_2bXm9Tq7hR1".into(),
        }
    );
    let sent = &s.server.received()[0];
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.path, LINKS);
    assert_eq!(sent.authorization.as_deref(), Some(KEY));
    assert!(
        sent.headers
            .contains(&("content-type".into(), "application/json".into()))
    );
    let body: Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!(
        body,
        json!({
            "domain": "mascate.short.gy",
            "originalURL": HOTMART_LINK,
            "path": "0000000b",
            "allowDuplicates": false,
        })
    );
}

#[test]
fn a_domain_pasted_as_an_address_is_sent_as_its_name() {
    let s = setup_with_domain("  https://Mascate.short.gy/ ");
    s.server.serve(LINKS, 200, fixture("link.json"));

    s.short_io.shorten(HOTMART_LINK, "0000000b").unwrap();

    let body: Value = serde_json::from_str(&s.server.received()[0].body).unwrap();
    assert_eq!(body["domain"], "mascate.short.gy");
}

#[test]
fn a_domain_that_is_not_one_is_refused_before_asking() {
    for pasted in [
        "mascate",
        "mascate short.gy",
        "mascate..short.gy",
        "a/b.short.gy",
    ] {
        let s = setup_with_domain(pasted);

        let refused = s.short_io.shorten(HOTMART_LINK, "0000000b");

        assert!(
            matches!(refused, Err(PlatformError::Refused(_))),
            "{pasted}: {refused:?}"
        );
        assert_eq!(links_asked(&s.server), 0, "{pasted}");
    }
}

#[test]
fn without_the_key_or_the_domain_nothing_is_asked() {
    let server = FakeHttpServer::start();
    for stored in [None, Some("SHORT_IO_API_KEY"), Some("SHORT_IO_DOMAIN")] {
        let store = Arc::new(MemorySecretStore::default());
        if let Some(name) = stored {
            store.write(name, &Secret::new("x.short.gy")).unwrap();
        }
        let short_io = ShortIo::new(server.url(), "Mascate/test", store);

        assert_eq!(
            short_io.shorten(HOTMART_LINK, "0000000b"),
            Err(PlatformError::NotConnected),
            "{stored:?}"
        );
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_key_short_io_does_not_accept_expires_the_connection_at_once() {
    let s = setup();
    s.server.serve(LINKS, 401, fixture("error-401.json"));

    assert_eq!(
        s.short_io.shorten(HOTMART_LINK, "0000000b"),
        Err(PlatformError::Expired)
    );
    assert_eq!(links_asked(&s.server), 1);
}

#[test]
fn a_rate_limit_waits_and_asks_again() {
    let s = setup();
    s.server.serve(LINKS, 429, fixture("error-429.json"));
    s.server.then_serve(LINKS, 429, fixture("error-429.json"));
    s.server.then_serve(LINKS, 200, fixture("link.json"));

    let shortened = s.short_io.shorten(HOTMART_LINK, "0000000b").unwrap();

    assert_eq!(shortened.url, "https://mascate.short.gy/0000000b");
    assert_eq!(s.pauses.seconds(), [1, 2]);
    assert_eq!(links_asked(&s.server), 3);
}

#[test]
fn a_rate_limit_that_stays_gives_up_after_three_retries() {
    let s = setup();
    s.server.serve(LINKS, 429, fixture("error-429.json"));

    assert_eq!(
        s.short_io.shorten(HOTMART_LINK, "0000000b"),
        Err(PlatformError::RateLimited)
    );
    assert_eq!(s.pauses.seconds(), [1, 2, 4]);
    assert_eq!(links_asked(&s.server), 4);
}

#[test]
fn a_dropped_connection_or_a_failure_on_its_side_is_asked_again() {
    let s = setup();
    s.server.hang_up(LINKS);
    s.server.then_serve(LINKS, 503, "");
    s.server.then_serve(LINKS, 200, fixture("link.json"));

    let shortened = s.short_io.shorten(HOTMART_LINK, "0000000b").unwrap();

    assert_eq!(shortened.key, "lnk_4kZq_2bXm9Tq7hR1");
    assert_eq!(links_asked(&s.server), 3);
}

#[test]
fn a_refusal_passes_on_short_ios_words() {
    for (status, file, words) in [
        (400, "error-400.json", "originalURL must be a valid URL"),
        (
            403,
            "error-403.json",
            "API key has no access to domain mascate.short.gy",
        ),
        (409, "error-409.json", "Link with this path already exists"),
    ] {
        let s = setup();
        s.server.serve(LINKS, status, fixture(file));

        assert_eq!(
            s.short_io.shorten(HOTMART_LINK, "0000000b"),
            Err(PlatformError::Refused(words.into())),
            "{status}"
        );
        assert_eq!(links_asked(&s.server), 1, "{status}");
    }
}

#[test]
fn a_refusal_without_words_names_the_status() {
    let s = setup();
    s.server.serve(LINKS, 402, "");

    assert_eq!(
        s.short_io.shorten(HOTMART_LINK, "0000000b"),
        Err(PlatformError::Refused("Short.io answered 402".into()))
    );
}

#[test]
fn a_short_link_off_the_owners_domain_is_not_kept() {
    let s = setup();
    s.server.serve(LINKS, 200, fixture("link-off-domain.json"));

    assert!(matches!(
        s.short_io.shorten(HOTMART_LINK, "0000000b"),
        Err(PlatformError::Failed(_))
    ));
}

/// Tags any link with `src`, as Hotmart's tagger does.
struct Tagger(AffiliatePlatform);

impl LinkTagger for Tagger {
    fn platform(&self) -> AffiliatePlatform {
        self.0
    }

    fn tag(&self, link: &str, tag: &str) -> Result<String, TagRefusal> {
        Ok(format!("{link}?src={tag}"))
    }
}

/// The affiliate module with a Program on `platform` and one link for a
/// Telegram channel.
async fn affiliate_with_link(
    dir: &tempfile::TempDir,
    platform: AffiliatePlatform,
) -> (Affiliate, RecordId) {
    let clock = Arc::new(ManualClock::at(
        Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap(),
    ));
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
    let affiliate = Affiliate::new(database, clock, Arc::new(SequentialIds::default()));
    affiliate
        .import(
            platform,
            &[ReportedConversion {
                key: "HP1".into(),
                program: ReportedProgram {
                    id: "123".into(),
                    name: "Curso de Fotografia".into(),
                },
                product: "Curso de Fotografia".into(),
                sold_at: Utc.with_ymd_and_hms(2026, 10, 20, 12, 0, 0).unwrap() - TimeDelta::days(1),
                commission: Money::new(Decimal::new(4710, 2), Currency::Brl),
                status: CommissionStatus::Pending,
                platform_status: "WAITING".into(),
                status_changed_at: None,
                tracking: None,
                raw: "{}".into(),
            }],
        )
        .await
        .unwrap();
    let program = affiliate.programs().await.unwrap()[0].id;
    let link = affiliate
        .create_link(
            &NewAffiliateLink {
                program,
                product: "Curso de Fotografia".into(),
                pasted: "https://go.hotmart.com/X12345678A".into(),
                channel: RecordId::from_u128(1),
                tag: "telegram".into(),
                promotion: Promotion::default(),
            },
            &Tagger(platform),
        )
        .await
        .unwrap()
        .link;
    (affiliate, link.id)
}

#[test]
fn shortening_an_affiliate_link_twice_through_short_io_asks_once_and_keeps_one() {
    block_on(async {
        let s = setup();
        s.server.serve(LINKS, 200, fixture("link.json"));
        let dir = tempfile::tempdir().unwrap();
        let (affiliate, link) = affiliate_with_link(&dir, AffiliatePlatform::Hotmart).await;

        let first = affiliate.shorten(link, &s.short_io).await.unwrap();
        let again = affiliate.shorten(link, &s.short_io).await.unwrap();

        assert_eq!(
            first.link.short.as_deref(),
            Some("https://mascate.short.gy/0000000b")
        );
        assert!(again.existed);
        assert_eq!(again.link, first.link);
        assert_eq!(links_asked(&s.server), 1);
        let body: Value = serde_json::from_str(&s.server.received()[0].body).unwrap();
        assert_eq!(body["originalURL"], HOTMART_LINK);
        assert_eq!(affiliate.links().await.unwrap(), vec![first.link]);
    });
}

#[test]
fn an_amazon_link_never_reaches_short_io_while_its_flag_is_off() {
    block_on(async {
        let s = setup();
        s.server.serve(LINKS, 200, fixture("link.json"));
        let dir = tempfile::tempdir().unwrap();
        let (affiliate, link) = affiliate_with_link(&dir, AffiliatePlatform::Amazon).await;

        let refused = affiliate.shorten(link, &s.short_io).await;

        assert!(matches!(
            refused,
            Err(AffiliateError::Restricted(AMAZON_SHORT_LINK))
        ));
        assert!(s.server.requests().is_empty());
    });
}

#[test]
fn a_key_refused_while_shortening_keeps_no_short_link() {
    block_on(async {
        let s = setup();
        s.server.serve(LINKS, 401, fixture("error-401.json"));
        let dir = tempfile::tempdir().unwrap();
        let (affiliate, link) = affiliate_with_link(&dir, AffiliatePlatform::Hotmart).await;

        let refused = affiliate.shorten(link, &s.short_io).await;

        assert!(matches!(
            refused,
            Err(AffiliateError::Platform(PlatformError::Expired))
        ));
        assert_eq!(affiliate.link(link).await.unwrap().short, None);
    });
}
