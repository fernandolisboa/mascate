//! Shopee Afiliados as a `LinkTagger` (#100) against the fake server serving
//! answers written from third-party copies of Shopee's documentation (see
//! the fixtures' README): the Promotion Channel's marking as the first
//! subId of `generateShortLink`, the short link as the tagged link, the
//! marking back as the Conversion's tracking, and the Affiliate module
//! asking Shopee once per link over a real temporary database.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliatePlatform, CommissionFilter, FLAGS, LinkTagger, MIGRATIONS,
    NewAffiliateLink, Promotion, Reach, SHOPEE_PAID_SEARCH, TagRefusal,
};
use mascate_integrations::{Pause, ShopeeAffiliates};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{PlatformError, RecordId};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Flags, Registry, Secret, SecretStore, migrate};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/shopee-affiliate/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

const FONE: &str = "https://shopee.com.br/Fone-de-Ouvido-Bluetooth-TWS-i.987654321.22334455667";
const SHORT: &str = "https://s.shopee.com.br/5AbCdEfGhI";

/// Remembers each pause and moves the clock past it, as a real one would.
struct Pauses {
    clock: Arc<ManualClock>,
    taken: Mutex<Vec<Duration>>,
}

impl Pause for Pauses {
    fn pause(&self, duration: Duration) {
        self.taken.lock().unwrap().push(duration);
        self.clock.advance(TimeDelta::from_std(duration).unwrap());
    }
}

struct Setup {
    server: FakeHttpServer,
    store: Arc<MemorySecretStore>,
    clock: Arc<ManualClock>,
    pauses: Arc<Pauses>,
    shopee: ShopeeAffiliates,
}

/// Shopee at the fake server with the AppID and Secret, on 2026-10-13
/// 12:00 UTC.
fn setup() -> Setup {
    let server = FakeHttpServer::start();
    let store = Arc::new(MemorySecretStore::default());
    store
        .write("SHOPEE_AFFILIATE_APP_ID", &Secret::new("123456"))
        .unwrap();
    store
        .write("SHOPEE_AFFILIATE_SECRET", &Secret::new("demo-secret"))
        .unwrap();
    let clock = Arc::new(ManualClock::at(
        Utc.with_ymd_and_hms(2026, 10, 13, 12, 0, 0).unwrap(),
    ));
    let pauses = Arc::new(Pauses {
        clock: clock.clone(),
        taken: Mutex::new(Vec::new()),
    });
    let shopee = ShopeeAffiliates::new(
        &format!("{}/graphql", server.url()),
        "Mascate/test",
        store.clone(),
        clock.clone(),
    )
    .with_pause(pauses.clone());
    Setup {
        server,
        store,
        clock,
        pauses,
        shopee,
    }
}

impl Setup {
    fn pauses(&self) -> Vec<u64> {
        self.pauses
            .taken
            .lock()
            .unwrap()
            .iter()
            .map(Duration::as_secs)
            .collect()
    }

    /// The GraphQL each request sent.
    fn queries(&self) -> Vec<String> {
        self.server
            .received()
            .into_iter()
            .map(|request| {
                let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
                body["query"].as_str().unwrap().to_owned()
            })
            .collect()
    }
}

#[test]
fn the_marking_goes_in_the_first_subid_and_the_short_link_is_the_tagged_link() {
    let s = setup();
    s.server.serve("/graphql", 200, fixture("short-link.json"));

    let tagged = s.shopee.tag(&format!("  {FONE} "), "telegram").unwrap();

    assert_eq!(tagged, SHORT);
    assert_eq!(
        s.queries(),
        [format!(
            "mutation{{generateShortLink(input: {{originUrl: \"{FONE}\", subIds: \
             [\"telegram\"]}}){{shortLink}}}}"
        )]
    );
    // Signed like every other request.
    let authorization = s.server.received()[0].authorization.clone().unwrap();
    assert!(authorization.starts_with("SHA256 Credential=123456, Timestamp=1791892800, "));
}

#[test]
fn shopee_afiliados_is_the_program_of_every_link() {
    let program = setup().shopee.program().unwrap();

    assert_eq!(program.id, "shopee-affiliates");
    assert_eq!(program.name, "Shopee Afiliados");
    assert_eq!(setup().shopee.platform(), AffiliatePlatform::Shopee);
}

#[test]
fn only_a_shopee_page_over_https_is_sent_and_never_rewritten() {
    let s = setup();
    s.server.serve("/graphql", 200, fixture("short-link.json"));

    for (link, refusal) in [
        ("shopee.com.br/produto", TagRefusal::NotALink),
        ("http://shopee.com.br/produto", TagRefusal::NotALink),
        (
            "https://shopee.com.br.example.net/produto",
            TagRefusal::ForeignDomain(AffiliatePlatform::Shopee),
        ),
        (
            "https://shopee.com.br@example.net/produto",
            TagRefusal::NotALink,
        ),
        (
            "https://shope.ee/5AbCdEfGhI",
            TagRefusal::ForeignDomain(AffiliatePlatform::Shopee),
        ),
    ] {
        assert_eq!(s.shopee.tag(link, "telegram"), Err(refusal), "{link}");
    }
    assert!(s.server.received().is_empty());

    // A shop, a subdomain and a link with its own parameters go as pasted.
    for link in [
        "https://shopee.com.br/shop/123456",
        "https://s.shopee.com.br/produto?sp_atk=abc&xptdk=def",
    ] {
        assert_eq!(s.shopee.tag(link, "telegram"), Ok(SHORT.to_owned()));
        assert!(
            s.queries()
                .last()
                .unwrap()
                .contains(&format!("originUrl: \"{link}\""))
        );
    }
}

#[test]
fn a_marking_shopee_cannot_carry_whole_is_refused_before_asking() {
    let s = setup();

    for tag in [
        "canal_telegram",
        "whatsapp-grupo",
        "",
        "a".repeat(21).as_str(),
    ] {
        assert_eq!(
            s.shopee.tag(FONE, tag),
            Err(TagRefusal::BadTag(AffiliatePlatform::Shopee)),
            "{tag}"
        );
    }
    assert!(s.server.received().is_empty());
}

#[test]
fn a_link_shopee_rejects_is_a_refusal_with_shopees_reason() {
    let s = setup();
    s.server
        .serve("/graphql", 200, fixture("error-11001-url.json"));

    assert_eq!(
        s.shopee.tag(FONE, "telegram"),
        Err(TagRefusal::Platform(PlatformError::Refused(
            "Params Error: invalid originUrl".into()
        )))
    );
}

#[test]
fn a_rate_limit_waits_longer_each_time_and_then_gives_up() {
    let s = setup();
    s.server.serve("/graphql", 200, fixture("error-10030.json"));

    assert_eq!(
        s.shopee.tag(FONE, "telegram"),
        Err(TagRefusal::Platform(PlatformError::RateLimited))
    );
    assert_eq!(s.pauses(), [2, 4, 8]);
    assert_eq!(s.server.received().len(), 4);
}

#[test]
fn a_rate_limit_that_lifts_gives_the_link_after_the_pause() {
    let s = setup();
    s.server.serve("/graphql", 200, fixture("error-10030.json"));
    s.server
        .then_serve("/graphql", 200, fixture("short-link.json"));

    assert_eq!(s.shopee.tag(FONE, "telegram"), Ok(SHORT.to_owned()));
    assert_eq!(s.pauses(), [2]);
    // Signed again at the new time.
    let signatures: Vec<String> = s
        .server
        .received()
        .into_iter()
        .map(|request| request.authorization.unwrap())
        .collect();
    assert_ne!(signatures[0], signatures[1]);
}

#[test]
fn a_refused_signature_means_shopee_no_longer_accepts_the_keys() {
    let s = setup();
    s.server.serve("/graphql", 200, fixture("error-10020.json"));

    assert_eq!(
        s.shopee.tag(FONE, "telegram"),
        Err(TagRefusal::Platform(PlatformError::Expired))
    );
    assert!(s.pauses().is_empty());
}

#[test]
fn without_the_keys_no_link_is_asked_for() {
    let s = setup();
    s.store.remove("SHOPEE_AFFILIATE_SECRET").unwrap();

    assert_eq!(
        s.shopee.tag(FONE, "telegram"),
        Err(TagRefusal::Platform(PlatformError::NotConnected))
    );
    assert!(s.server.received().is_empty());
}

#[test]
fn a_short_link_off_shopee_is_never_given_to_the_owner() {
    let s = setup();
    s.server
        .serve("/graphql", 200, fixture("short-link-off-shopee.json"));

    assert!(matches!(
        s.shopee.tag(FONE, "telegram"),
        Err(TagRefusal::Platform(PlatformError::Failed(_)))
    ));
}

struct Linked {
    _dir: tempfile::TempDir,
    affiliate: Affiliate,
    flags: Flags,
}

fn linked(s: &Setup) -> Linked {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(
            &database,
            s.clock.as_ref(),
            &[mascate_platform::MIGRATIONS, MIGRATIONS],
        )
        .await
        .unwrap();
        let ids = Arc::new(SequentialIds::default());
        Linked {
            _dir: dir,
            affiliate: Affiliate::new(database.clone(), s.clock.clone(), ids.clone()),
            flags: Flags::new(
                database,
                Registry::new(&[FLAGS]).unwrap(),
                s.clock.clone(),
                ids,
            ),
        }
    })
}

fn channel() -> RecordId {
    RecordId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001)
}

fn fone_link(program: RecordId, tag: &str) -> NewAffiliateLink {
    NewAffiliateLink {
        program,
        product: "Fone de Ouvido Bluetooth TWS".into(),
        pasted: FONE.into(),
        channel: channel(),
        tag: tag.into(),
        promotion: Promotion::default(),
    }
}

#[test]
fn the_marking_comes_back_as_the_conversions_tracking() {
    let s = setup();
    let l = linked(&s);
    s.server.serve("/graphql", 200, fixture("short-link.json"));
    let program = block_on(l.affiliate.program_of(&s.shopee))
        .unwrap()
        .unwrap();

    // The conversion fixture's sale came from a link with `instagram` as
    // its first subId and `story` as its second.
    let created = block_on(
        l.affiliate
            .create_link(&fone_link(program.id, "instagram"), &s.shopee),
    )
    .unwrap();
    s.server
        .serve("/graphql", 200, fixture("conversion-report.json"));
    s.server
        .then_serve("/graphql", 200, fixture("conversion-report-end.json"));
    block_on(l.affiliate.sync(&s.shopee)).unwrap();

    let commissions = block_on(l.affiliate.commissions(&CommissionFilter::default())).unwrap();
    let fone = commissions
        .iter()
        .find(|commission| commission.product == created.link.product)
        .unwrap();
    assert_eq!(fone.tracking.as_deref(), Some(created.link.tag.as_str()));
    // The Sync lands on the Program the link was made for.
    assert_eq!(fone.program, program.id);
    assert_eq!(block_on(l.affiliate.programs()).unwrap().len(), 1);
}

#[test]
fn creating_the_same_shopee_link_twice_asks_shopee_once() {
    let s = setup();
    let l = linked(&s);
    s.server.serve("/graphql", 200, fixture("short-link.json"));
    let program = block_on(l.affiliate.program_of(&s.shopee))
        .unwrap()
        .unwrap();

    let first = block_on(
        l.affiliate
            .create_link(&fone_link(program.id, "telegram"), &s.shopee),
    )
    .unwrap();
    let again = block_on(
        l.affiliate
            .create_link(&fone_link(program.id, "telegram"), &s.shopee),
    )
    .unwrap();

    assert_eq!(first.link.tagged, SHORT);
    assert!(again.existed);
    assert_eq!(again.link, first.link);
    assert_eq!(s.server.received().len(), 1);
}

#[test]
fn without_the_keys_creating_a_shopee_link_explains_and_keeps_nothing() {
    let s = setup();
    let l = linked(&s);
    s.store.remove("SHOPEE_AFFILIATE_APP_ID").unwrap();
    let program = block_on(l.affiliate.program_of(&s.shopee))
        .unwrap()
        .unwrap();

    let failed = block_on(
        l.affiliate
            .create_link(&fone_link(program.id, "telegram"), &s.shopee),
    );

    assert!(matches!(
        failed,
        Err(AffiliateError::Platform(PlatformError::NotConnected))
    ));
    assert!(block_on(l.affiliate.links()).unwrap().is_empty());
    assert!(s.server.received().is_empty());
}

#[test]
fn a_shopee_link_for_paid_search_waits_for_its_restricted_feature() {
    let s = setup();
    let l = linked(&s);
    s.server.serve("/graphql", 200, fixture("short-link.json"));
    let program = block_on(l.affiliate.program_of(&s.shopee))
        .unwrap()
        .unwrap();
    let paid_search = NewAffiliateLink {
        promotion: Promotion {
            reach: Reach::PaidSearch,
            ..Promotion::default()
        },
        ..fone_link(program.id, "googleads")
    };

    let refused = block_on(l.affiliate.create_link(&paid_search, &s.shopee));
    assert!(matches!(refused, Err(AffiliateError::Restricted(flag)) if flag == SHOPEE_PAID_SEARCH));
    assert!(s.server.received().is_empty());

    block_on(
        l.flags.turn_on(
            l.flags
                .request_turn_on(SHOPEE_PAID_SEARCH.key)
                .unwrap()
                .confirm("fernando"),
        ),
    )
    .unwrap();
    let created = block_on(l.affiliate.create_link(&paid_search, &s.shopee)).unwrap();
    assert_eq!(created.link.tagged, SHORT);
    assert_eq!(s.server.received().len(), 1);
}
