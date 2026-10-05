//! Listing Copy through its two adapters against the fake server: the
//! Claude API writing the text and Mercado Livre giving a category's limits
//! and trends, with answers written from each Platform's documentation (see
//! the fixtures' READMEs), and marketing's `ListingCopy` end to end on a
//! real temporary database.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_integrations::{Anthropic, MercadoLivre, Pause};
use mascate_kernel::PlatformError;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_marketing::{
    BriefAttribute, COPY_INSTRUCTIONS, CategoryRules, CopyBrief, CopyChannel, CopyRequest,
    CopyWriter, ListingCopy, MIGRATIONS, WrittenCopy,
};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use serde_json::{Value, json};

const MESSAGES: &str = "/v1/messages";

fn fixture(platform: &str, name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/{platform}/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn anthropic_fixture(name: &str) -> String {
    fixture("anthropic", name)
}

fn mercado_livre_fixture(name: &str) -> String {
    fixture("mercado-livre", name)
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
    anthropic: Anthropic,
    mercado_livre: MercadoLivre,
}

/// Both Platforms at the fake server: the Claude API with the key
/// `sk-ant-test`, and a connected Mercado Livre seller.
fn setup() -> Setup {
    let server = FakeHttpServer::start();
    let store = Arc::new(MemorySecretStore::default());
    for (name, value) in [
        ("ANTHROPIC_API_KEY", "sk-ant-test"),
        ("ML_CLIENT_ID", "1234567890"),
        ("ML_CLIENT_SECRET", "s3cr3t"),
        ("ML_REFRESH_TOKEN", "TG-first"),
    ] {
        store.write(name, &Secret::new(value)).unwrap();
    }
    let pauses = Arc::new(RecordedPauses::default());
    let anthropic =
        Anthropic::new(server.url(), "Mascate/test", store.clone()).with_pause(pauses.clone());
    let clock = Arc::new(ManualClock::at(
        Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap(),
    ));
    let mercado_livre = MercadoLivre::new(server.url(), "Mascate/test", store, clock);
    server.serve("/oauth/token", 200, mercado_livre_fixture("token.json"));
    Setup {
        server,
        pauses,
        anthropic,
        mercado_livre,
    }
}

fn request() -> CopyRequest {
    CopyRequest {
        model: "claude-sonnet-5-5".into(),
        instructions: COPY_INSTRUCTIONS.into(),
        prompt: "<dados>\nProduto: Fone Lenovo LP40\n</dados>".into(),
    }
}

fn lenovo_copy() -> WrittenCopy {
    WrittenCopy {
        title: "Fone De Ouvido Bluetooth Sem Fio Lenovo LP40 TWS Branco".into(),
        description: "Fone de ouvido sem fio Lenovo LP40 para o dia a dia, com estojo de \
                      carga.\n\n- Conexão Bluetooth 5.0\n- Modelo TWS, um fone em cada ouvido\n\
                      - Cor branca\n\nNa caixa: 2 fones, estojo de carga e cabo USB."
            .into(),
    }
}

fn messages_sent(server: &FakeHttpServer) -> usize {
    server
        .requests()
        .iter()
        .filter(|path| *path == MESSAGES)
        .count()
}

#[test]
fn the_copy_is_asked_with_the_key_the_model_and_a_json_schema() {
    let s = setup();
    s.server
        .serve(MESSAGES, 200, anthropic_fixture("message-copy.json"));

    let copy = s.anthropic.write(&request()).unwrap();

    assert_eq!(copy, lenovo_copy());
    let sent = &s.server.received()[0];
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.path, MESSAGES);
    for header in [
        ("x-api-key", "sk-ant-test"),
        ("anthropic-version", "2023-06-01"),
        ("content-type", "application/json"),
    ] {
        assert!(
            sent.headers.contains(&(header.0.into(), header.1.into())),
            "{header:?}"
        );
    }
    let body: Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!(
        body,
        json!({
            "model": "claude-sonnet-5-5",
            "max_tokens": 16000,
            "system": COPY_INSTRUCTIONS,
            "messages": [{
                "role": "user",
                "content": "<dados>\nProduto: Fone Lenovo LP40\n</dados>"
            }],
            "output_config": {
                "format": {
                    "type": "json_schema",
                    "schema": {
                        "type": "object",
                        "properties": {
                            "title": {
                                "type": "string",
                                "description": "O título do anúncio, dentro do limite pedido."
                            },
                            "description": {
                                "type": "string",
                                "description": "A descrição do anúncio, em texto simples."
                            }
                        },
                        "required": ["title", "description"],
                        "additionalProperties": false
                    }
                }
            }
        })
    );
}

#[test]
fn without_a_key_nothing_is_sent() {
    let s = setup();
    let empty = Anthropic::new(
        s.server.url(),
        "Mascate/test",
        Arc::new(MemorySecretStore::default()),
    );

    assert_eq!(empty.write(&request()), Err(PlatformError::NotConnected));
    assert!(s.server.requests().is_empty());
}

#[test]
fn a_rate_limit_or_an_overloaded_api_is_tried_again_after_a_pause() {
    let s = setup();
    s.server
        .serve(MESSAGES, 429, anthropic_fixture("error-429.json"));
    s.server
        .then_serve(MESSAGES, 529, anthropic_fixture("error-529.json"));
    s.server
        .then_serve(MESSAGES, 200, anthropic_fixture("message-copy.json"));

    assert_eq!(s.anthropic.write(&request()), Ok(lenovo_copy()));
    assert_eq!(s.pauses.seconds(), [2, 4]);
    assert_eq!(messages_sent(&s.server), 3);
}

#[test]
fn an_api_still_overloaded_after_three_retries_fails() {
    let s = setup();
    s.server
        .serve(MESSAGES, 529, anthropic_fixture("error-529.json"));

    let failed = s.anthropic.write(&request());

    assert_eq!(
        failed,
        Err(PlatformError::Failed("Anthropic answered 529".into()))
    );
    assert_eq!(s.pauses.seconds(), [2, 4, 8]);
    assert_eq!(messages_sent(&s.server), 4);
}

#[test]
fn a_key_anthropic_turns_down_is_expired_and_not_tried_again() {
    let s = setup();
    s.server
        .serve(MESSAGES, 401, anthropic_fixture("error-401.json"));

    assert_eq!(s.anthropic.write(&request()), Err(PlatformError::Expired));
    assert_eq!(messages_sent(&s.server), 1);
}

#[test]
fn a_refused_request_carries_anthropics_reason() {
    for (status, name, reason) in [
        (404, "error-404-model.json", "model: claude-sonnet-9"),
        (400, "error-400.json", "max_tokens: Field required"),
    ] {
        let s = setup();
        s.server.serve(MESSAGES, status, anthropic_fixture(name));

        assert_eq!(
            s.anthropic.write(&request()),
            Err(PlatformError::Refused(reason.into()))
        );
        assert_eq!(messages_sent(&s.server), 1);
        assert!(s.pauses.seconds().is_empty());
    }
}

#[test]
fn a_declined_or_cut_answer_never_becomes_a_copy() {
    let s = setup();
    s.server
        .serve(MESSAGES, 200, anthropic_fixture("message-refusal.json"));
    assert_eq!(
        s.anthropic.write(&request()),
        Err(PlatformError::Refused(
            "o modelo se recusou a escrever este anúncio".into()
        ))
    );

    s.server
        .serve(MESSAGES, 200, anthropic_fixture("message-max-tokens.json"));
    assert!(matches!(
        s.anthropic.write(&request()),
        Err(PlatformError::Failed(_))
    ));
}

#[test]
fn a_categorys_limits_come_from_its_settings_or_mercado_livres_usual_ones() {
    let s = setup();
    s.server.serve(
        "/categories/MLB196208",
        200,
        mercado_livre_fixture("category-MLB196208.json"),
    );
    s.server.serve(
        "/categories/MLB3697",
        200,
        mercado_livre_fixture("category-MLB3697.json"),
    );

    let usual = CategoryRules {
        max_title_chars: 60,
        max_description_chars: 50_000,
    };
    assert_eq!(s.mercado_livre.category_rules("MLB196208"), Ok(usual));
    assert_eq!(s.mercado_livre.category_rules("MLB3697"), Ok(usual));
    assert_eq!(
        s.mercado_livre.category_rules("../users/me"),
        Err(PlatformError::NotFound)
    );
}

#[test]
fn trends_are_the_keywords_buyers_search_in_the_category() {
    let s = setup();
    s.server.serve(
        "/trends/MLB/MLB196208",
        200,
        mercado_livre_fixture("trends-MLB196208.json"),
    );
    s.server.serve(
        "/trends/MLB/MLB1234",
        404,
        mercado_livre_fixture("error-404-trends.json"),
    );

    let trends = s.mercado_livre.trends("MLB196208").unwrap();

    assert_eq!(trends.len(), 10);
    assert_eq!(
        trends[..3],
        ["fone bluetooth", "fone de ouvido bluetooth", "fone sem fio"]
    );
    assert_eq!(
        s.mercado_livre.trends("MLB1234"),
        Err(PlatformError::NotFound)
    );
    let sent = s
        .server
        .received()
        .into_iter()
        .find(|request| request.path == "/trends/MLB/MLB196208")
        .unwrap();
    assert!(
        sent.authorization
            .is_some_and(|value| value.starts_with("Bearer "))
    );
}

#[test]
fn a_draft_gets_its_copy_written_from_both_platforms() {
    let s = setup();
    s.server.serve(
        "/categories/MLB196208",
        200,
        mercado_livre_fixture("category-MLB196208.json"),
    );
    s.server.serve(
        "/trends/MLB/MLB196208",
        200,
        mercado_livre_fixture("trends-MLB196208.json"),
    );
    s.server
        .serve(MESSAGES, 200, anthropic_fixture("message-copy.json"));
    let brief = CopyBrief {
        product: "Fone Lenovo LP40".into(),
        offers: vec!["Fone Bluetooth Lenovo LP40 TWS Original".into()],
        category: "MLB196208".into(),
        category_name: "Fones de Ouvido Bluetooth".into(),
        condition: "Novo".into(),
        attributes: vec![BriefAttribute {
            name: "Marca".into(),
            value: "Lenovo".into(),
        }],
    };

    let copy = block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap(),
        ));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        ListingCopy::new(database, clock, Arc::new(SequentialIds::default()))
            .write(&s.mercado_livre, &s.anthropic, &brief)
            .await
            .unwrap()
    });

    assert_eq!(copy.title, lenovo_copy().title);
    assert_eq!(copy.description, lenovo_copy().description);
    assert_eq!(
        copy.trends_in_title,
        [
            "fone bluetooth",
            "fone de ouvido bluetooth",
            "fone sem fio",
            "fone tws",
            "fone lenovo"
        ]
    );
    let sent = s
        .server
        .received()
        .into_iter()
        .find(|request| request.path == MESSAGES)
        .unwrap();
    let body: Value = serde_json::from_str(&sent.body).unwrap();
    let prompt = body["messages"][0]["content"].as_str().unwrap();
    assert!(prompt.contains("título com até 60 caracteres"));
    assert!(prompt.contains("- fone com cancelamento de ruido"));
    assert!(prompt.contains("- Marca: Lenovo"));
}
