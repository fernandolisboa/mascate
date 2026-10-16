//! The issues Amazon reports on the owner's listings through the Selling
//! Partner API adapter, against the fake server serving the answers in
//! `fixtures/amazon-seller`, written from the documentation (see its
//! README). What marketing keeps of them is tested there.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use mascate_integrations::Amazon;
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{PlatformError, SalesChannelKind};
use mascate_marketing::{ActionKind, ChannelQuality, QualityAction, QualitySource};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};

const FONE: &str = "/listings/2021-08-01/items/A1MASCATE0SELLER/FON-TWS-001";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/amazon-seller/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn fix_link(sku: &str) -> Option<String> {
    Some(format!(
        "https://sellercentral.amazon.com.br/abis/listing/edit?marketplaceID=A2Q3Y263D00KWC&sku={sku}"
    ))
}

struct Fake {
    server: FakeHttpServer,
    adapter: Amazon,
}

impl Fake {
    fn connected() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            (
                "AMAZON_LWA_CLIENT_ID",
                "amzn1.application-oa2-client.mascate",
            ),
            ("AMAZON_LWA_CLIENT_SECRET", "amzn1.oa2-cs.v1.s3cr3t"),
            ("AMAZON_REFRESH_TOKEN", "Atzr|IwEBIA-refresh-token"),
            ("AMAZON_SELLER_ID", "A1MASCATE0SELLER"),
        ] {
            store.write(name, &Secret::new(value)).unwrap();
        }
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
        ));
        let adapter = Amazon::new(server.url(), server.url(), "Mascate/test", store, clock);
        server.serve("/auth/o2/token", 200, fixture("token.json"));
        Self { server, adapter }
    }
}

#[test]
fn the_issues_of_a_listing_on_amazon_com_br_are_its_pending_actions_without_a_score() {
    let fake = Fake::connected();
    fake.server
        .serve_any_query(FONE, 200, fixture("listing-issues.json"));

    let quality = fake.adapter.quality("FON-TWS-001").unwrap();

    assert_eq!(fake.adapter.rated_channel(), SalesChannelKind::Amazon);
    assert_eq!(
        quality,
        Some(ChannelQuality {
            score: None,
            pending: vec![
                QualityAction {
                    key: "90220:item_weight,color".into(),
                    kind: ActionKind::Opportunity,
                    text: "Os atributos marcados como relevantes estão incompletos. Informe: \
                           item_weight, color"
                        .into(),
                    label: "Corrigir no Seller Central".into(),
                    link: fix_link("FON-TWS-001"),
                },
                // Enforced: out of the search until the image is fixed.
                QualityAction {
                    key: "18027".into(),
                    kind: ActionKind::Problem,
                    text: "A imagem principal tem texto, logotipo ou marca d'água não permitidos \
                           para este tipo de produto. Enquanto isso: o anúncio não aparece na \
                           busca."
                        .into(),
                    label: "Corrigir no Seller Central".into(),
                    link: fix_link("FON-TWS-001"),
                },
                // Exempt for now: a warning still.
                QualityAction {
                    key: "8541".into(),
                    kind: ActionKind::Opportunity,
                    text: "O preço está muito abaixo do preço de referência.".into(),
                    label: "Corrigir no Seller Central".into(),
                    link: fix_link("FON-TWS-001"),
                },
            ],
        })
    );
    let asked = fake
        .server
        .received()
        .into_iter()
        .find(|request| request.path.starts_with(FONE))
        .unwrap();
    assert!(asked.path.contains("includedData=issues"), "{}", asked.path);
    assert!(asked.path.contains("issueLocale=pt_BR"), "{}", asked.path);
    assert!(
        asked.path.contains("marketplaceIds=A2Q3Y263D00KWC"),
        "{}",
        asked.path
    );
}

#[test]
fn a_listing_without_issues_is_rated_with_nothing_pending_and_no_visits() {
    let fake = Fake::connected();
    fake.server.serve_any_query(
        "/listings/2021-08-01/items/A1MASCATE0SELLER/CAN-300",
        200,
        fixture("listing-issues-none.json"),
    );

    assert_eq!(
        fake.adapter.quality("CAN-300").unwrap(),
        Some(ChannelQuality {
            score: None,
            pending: Vec::new(),
        })
    );
    assert_eq!(fake.adapter.visits("CAN-300", 30).unwrap(), None);
}

#[test]
fn a_sku_amazon_no_longer_has_is_not_rated() {
    let fake = Fake::connected();
    fake.server
        .serve_any_query(FONE, 404, fixture("error-404.json"));

    assert_eq!(fake.adapter.quality("FON-TWS-001").unwrap(), None);
}

#[test]
fn a_sku_goes_escaped_into_the_path_and_the_link() {
    let fake = Fake::connected();
    fake.server.serve_any_query(
        "/listings/2021-08-01/items/A1MASCATE0SELLER/A%2FB%3F1",
        200,
        fixture("listing-issues.json"),
    );

    let quality = fake.adapter.quality("A/B?1").unwrap().unwrap();

    assert_eq!(quality.pending[0].link, fix_link("A%2FB%3F1"));
}

#[test]
fn a_refused_token_is_an_expired_connection() {
    let fake = Fake::connected();
    fake.server
        .serve_any_query(FONE, 403, fixture("error-403.json"));

    assert!(matches!(
        fake.adapter.quality("FON-TWS-001"),
        Err(PlatformError::Expired)
    ));
}
