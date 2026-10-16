//! Mercado Livre's own price automation through its adapter, against the
//! fake server serving the answers in `fixtures/mercado-livre`, written
//! from the documentation (see its README): turned on with the best rule,
//! changed, read active and paused by a Promotion, ended, and the price to
//! win. What commerce does with them is tested there.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use mascate_commerce::{
    ChannelAutomation, ChannelPriceAutomation, RepricingLimits, RepricingStatus, SalesChannel,
};
use mascate_integrations::MercadoLivre;
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Currency, Money, PlatformError};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};
use rust_decimal::Decimal;
use serde_json::json;

const FONE: &str = "MLB4100000001";
const AUTOMATION: &str = "/pricing-automation/items/MLB4100000001/automation";
const RULES: &str = "/pricing-automation/items/MLB4100000001/rules";
const PRICE_TO_WIN: &str = "/items/MLB4100000001/price_to_win?version=v2";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/mercado-livre/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn brl(amount: &str) -> Money {
    Money::new(amount.parse::<Decimal>().unwrap(), Currency::Brl)
}

fn limits(minimum: &str, maximum: &str) -> RepricingLimits {
    RepricingLimits {
        minimum: brl(minimum),
        maximum: brl(maximum),
    }
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
            Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
        ));
        let adapter = MercadoLivre::new(server.url(), "Mascate/test", store, clock);
        server.serve("/oauth/token", 200, fixture("token.json"));
        server.serve(RULES, 200, fixture("pricing-automation-rules.json"));
        Self { server, adapter }
    }

    /// The bodies sent with `method` to `path`, as JSON.
    fn sent(&self, method: &str, path: &str) -> Vec<serde_json::Value> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.method == method && request.path == path)
            .map(|request| serde_json::from_str(&request.body).unwrap())
            .collect()
    }

    fn methods(&self, path: &str) -> Vec<String> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.path == path)
            .map(|request| request.method)
            .collect()
    }
}

#[test]
fn turning_the_automation_on_takes_the_rule_that_looks_inside_and_outside_mercado_livre() {
    let fake = Fake::connected();
    fake.server.serve_method(
        "GET",
        AUTOMATION,
        404,
        fixture("error-404-pricing-automation.json"),
    );
    fake.server.serve_method(
        "POST",
        AUTOMATION,
        200,
        fixture("pricing-automation-active.json"),
    );

    let automation = fake
        .adapter
        .automate(FONE, &limits("59.90", "120"))
        .unwrap();

    assert_eq!(
        automation,
        ChannelAutomation {
            limits: limits("60", "120"),
            status: RepricingStatus::Following,
        }
    );
    assert_eq!(
        fake.sent("POST", AUTOMATION),
        [json!({"rule_id": "INT_EXT", "min_price": 59.90, "max_price": 120})]
    );
    // The amounts go as decimal text, never through a float.
    let posted = fake
        .server
        .received()
        .into_iter()
        .find(|request| request.method == "POST" && request.path == AUTOMATION)
        .unwrap()
        .body;
    assert!(posted.contains(r#""min_price":59.90"#), "{posted}");
}

#[test]
fn an_item_with_only_the_inside_rule_takes_it() {
    let fake = Fake::connected();
    let automation = "/pricing-automation/items/MLB4100000003/automation";
    fake.server.serve(
        "/pricing-automation/items/MLB4100000003/rules",
        200,
        fixture("pricing-automation-rules-int.json"),
    );
    fake.server.serve_method(
        "GET",
        automation,
        404,
        fixture("error-404-pricing-automation.json"),
    );
    fake.server.serve_method(
        "POST",
        automation,
        200,
        fixture("pricing-automation-active.json"),
    );

    fake.adapter
        .automate("MLB4100000003", &limits("60", "120"))
        .unwrap();

    assert_eq!(fake.sent("POST", automation)[0]["rule_id"], "INT");
}

#[test]
fn new_limits_change_the_automation_the_item_has_with_its_rule() {
    let fake = Fake::connected();
    fake.server.serve_method(
        "GET",
        AUTOMATION,
        200,
        fixture("pricing-automation-active.json"),
    );
    fake.server.serve_method(
        "PUT",
        AUTOMATION,
        200,
        fixture("pricing-automation-changed.json"),
    );

    let automation = fake.adapter.automate(FONE, &limits("70", "110")).unwrap();

    assert_eq!(automation.limits, limits("70", "110"));
    assert_eq!(fake.methods(AUTOMATION), ["GET", "PUT"]);
    assert_eq!(
        fake.sent("PUT", AUTOMATION),
        [json!({"rule_id": "INT_EXT", "min_price": 70, "max_price": 110})]
    );
    assert!(!fake.server.requests().iter().any(|path| path == RULES));
}

#[test]
fn an_item_mercado_livre_does_not_automate_is_refused_with_its_reason() {
    let fake = Fake::connected();
    fake.server
        .serve(RULES, 412, fixture("error-412-not-automatizable.json"));

    assert_eq!(
        fake.adapter.automate(FONE, &limits("60", "120")),
        Err(PlatformError::Refused("Item is not automatizable".into()))
    );
    assert!(fake.sent("POST", AUTOMATION).is_empty());
}

#[test]
fn the_automation_reads_active_paused_by_a_promotion_paused_otherwise_or_gone() {
    let fake = Fake::connected();
    fake.server
        .serve(AUTOMATION, 200, fixture("pricing-automation-active.json"));
    assert_eq!(
        fake.adapter.automation(FONE).unwrap().unwrap().status,
        RepricingStatus::Following
    );

    fake.server.serve(
        AUTOMATION,
        200,
        fixture("pricing-automation-paused-promotion.json"),
    );
    let paused = fake.adapter.automation(FONE).unwrap().unwrap();
    assert_eq!(paused.status, RepricingStatus::PausedByPromotion);
    assert_eq!(paused.limits, limits("60", "120"));

    fake.server.serve(
        AUTOMATION,
        200,
        fixture("pricing-automation-paused-inactive.json"),
    );
    assert_eq!(
        fake.adapter.automation(FONE).unwrap().unwrap().status,
        RepricingStatus::Paused("Item paused message".into())
    );

    fake.server.serve(
        AUTOMATION,
        404,
        fixture("error-404-pricing-automation.json"),
    );
    assert_eq!(fake.adapter.automation(FONE).unwrap(), None);
}

#[test]
fn ending_the_automation_deletes_it_and_one_already_gone_counts_as_ended() {
    let fake = Fake::connected();
    fake.server.serve_method("DELETE", AUTOMATION, 200, "");

    fake.adapter.stop(FONE).unwrap();
    assert_eq!(fake.methods(AUTOMATION), ["DELETE"]);

    fake.server.serve_method(
        "DELETE",
        AUTOMATION,
        404,
        fixture("error-404-pricing-automation.json"),
    );
    fake.adapter.stop(FONE).unwrap();
}

#[test]
fn the_price_to_win_is_information_and_an_item_off_the_catalog_has_none() {
    let fake = Fake::connected();
    fake.server.serve(
        PRICE_TO_WIN,
        200,
        fixture("price-to-win-MLB4100000001.json"),
    );
    assert_eq!(fake.adapter.price_to_win(FONE).unwrap(), Some(brl("92.50")));

    fake.server.serve(
        PRICE_TO_WIN,
        404,
        fixture("error-404-pricing-automation.json"),
    );
    assert_eq!(fake.adapter.price_to_win(FONE).unwrap(), None);
}

#[test]
fn an_id_that_is_not_an_item_never_reaches_the_api() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter.automate("../users/me", &limits("60", "120")),
        Err(PlatformError::NotFound)
    );
    assert_eq!(fake.adapter.stop("MLB1?x=1"), Err(PlatformError::NotFound));
    assert!(
        fake.server
            .requests()
            .iter()
            .all(|path| path == "/oauth/token")
    );
}

#[test]
fn a_price_sent_to_an_automated_item_is_refused_with_mercado_livres_reason() {
    let fake = Fake::connected();
    fake.server.serve(
        "/items/MLB4100000001",
        200,
        fixture("item-MLB4100000001.json"),
    );
    fake.server.serve_method(
        "PUT",
        "/items/MLB4100000001",
        400,
        fixture("error-400-price-not-modifiable.json"),
    );

    assert_eq!(
        fake.adapter.set_price(FONE, brl("95")),
        Err(PlatformError::Refused(
            "Cannot modify price on items with dynamic pricing".into()
        ))
    );
}
