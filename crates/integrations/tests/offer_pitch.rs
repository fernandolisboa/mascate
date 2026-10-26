//! The Pitch of an Offer Post written by the Claude API's adapter against
//! the fake server, with answers written from Anthropic's documentation
//! (see the fixtures' README), and marketing's `OfferPosts` end to end on a
//! real temporary database.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_integrations::{Anthropic, Pause};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, PlatformError, RecordId};
use mascate_marketing::{
    ChannelKind, CopyRequest, CopyShape, CopyWriter, MIGRATIONS, NewPromotionChannel, OfferFacts,
    OfferPosts, OfferPrice, PITCH_INSTRUCTIONS, PromotionChannels, Written,
};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;
use serde_json::{Value, json};

const MESSAGES: &str = "/v1/messages";
const PITCH: &str = "Fotos de revista com o celular que você já tem no bolso";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/anthropic/{name}",
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
    anthropic: Anthropic,
}

/// The Claude API at the fake server, with the key `sk-ant-test`.
fn setup() -> Setup {
    let server = FakeHttpServer::start();
    let store = Arc::new(MemorySecretStore::default());
    store
        .write("ANTHROPIC_API_KEY", &Secret::new("sk-ant-test"))
        .unwrap();
    let pauses = Arc::new(RecordedPauses::default());
    let anthropic = Anthropic::new(server.url(), "Mascate/test", store).with_pause(pauses.clone());
    Setup {
        server,
        pauses,
        anthropic,
    }
}

fn request() -> CopyRequest {
    CopyRequest {
        model: "claude-sonnet-5-5".into(),
        instructions: PITCH_INSTRUCTIONS.into(),
        prompt: "<dados>\nProduto: Curso de Fotografia\n</dados>".into(),
        shape: CopyShape::Pitch,
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
fn the_pitch_is_asked_with_its_own_json_schema() {
    let s = setup();
    s.server.serve(MESSAGES, 200, fixture("message-pitch.json"));

    assert_eq!(
        s.anthropic.write(&request()),
        Ok(Written::Pitch(PITCH.into()))
    );
    let sent = &s.server.received()[0];
    assert!(
        sent.headers
            .contains(&("x-api-key".into(), "sk-ant-test".into()))
    );
    let body: Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!(
        body,
        json!({
            "model": "claude-sonnet-5-5",
            "max_tokens": 16000,
            "system": PITCH_INSTRUCTIONS,
            "messages": [{
                "role": "user",
                "content": "<dados>\nProduto: Curso de Fotografia\n</dados>"
            }],
            "output_config": {
                "format": {
                    "type": "json_schema",
                    "schema": {
                        "type": "object",
                        "properties": {
                            "pitch": {
                                "type": "string",
                                "description": "A chamada do post: uma frase, numa linha, dentro do limite pedido."
                            }
                        },
                        "required": ["pitch"],
                        "additionalProperties": false
                    }
                }
            }
        })
    );
}

#[test]
fn a_rate_limit_or_an_overloaded_api_is_tried_again_after_a_pause() {
    let s = setup();
    s.server.serve(MESSAGES, 429, fixture("error-429.json"));
    s.server
        .then_serve(MESSAGES, 529, fixture("error-529.json"));
    s.server
        .then_serve(MESSAGES, 200, fixture("message-pitch.json"));

    assert_eq!(
        s.anthropic.write(&request()),
        Ok(Written::Pitch(PITCH.into()))
    );
    assert_eq!(s.pauses.seconds(), [2, 4]);
    assert_eq!(messages_sent(&s.server), 3);
}

#[test]
fn a_key_anthropic_turns_down_is_expired_and_not_tried_again() {
    let s = setup();
    s.server.serve(MESSAGES, 401, fixture("error-401.json"));

    assert_eq!(s.anthropic.write(&request()), Err(PlatformError::Expired));
    assert_eq!(messages_sent(&s.server), 1);
}

#[test]
fn a_declined_answer_or_one_in_another_shape_is_no_pitch() {
    let s = setup();
    s.server
        .serve(MESSAGES, 200, fixture("message-refusal.json"));
    assert_eq!(
        s.anthropic.write(&request()),
        Err(PlatformError::Refused(
            "o modelo se recusou a escrever esta chamada".into()
        ))
    );

    s.server.serve(MESSAGES, 200, fixture("message-copy.json"));
    assert!(matches!(
        s.anthropic.write(&request()),
        Err(PlatformError::Failed(_))
    ));
}

#[test]
fn an_offer_post_gets_its_pitch_from_the_claude_api() {
    let s = setup();
    s.server.serve(MESSAGES, 200, fixture("message-pitch.json"));

    let (plain, pitched) = block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 26, 12, 0, 0).unwrap(),
        ));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        let ids = Arc::new(SequentialIds::default());
        let channel = PromotionChannels::new(database.clone(), clock.clone(), ids.clone())
            .create(&NewPromotionChannel {
                name: "Ofertas de cursos".into(),
                kind: ChannelKind::TelegramChannel,
                default_tag: "telegram".into(),
                keywords: Vec::new(),
                telegram_chat: None,
            })
            .await
            .unwrap();
        let posts = OfferPosts::new(database, clock, ids);
        let facts = OfferFacts {
            link_id: RecordId::from_u128(900),
            link: "https://go.hotmart.com/X12345678Y?src=telegram".into(),
            product: "Curso de Fotografia".into(),
            platform: "Hotmart".into(),
            price: Some(OfferPrice {
                price: Money::new(Decimal::new(14990, 2), Currency::Brl),
                before: None,
                seen_at: Utc.with_ymd_and_hms(2026, 10, 26, 11, 30, 0).unwrap(),
            }),
            image: None,
            price_needs_time: false,
            forbidden_contacts: Vec::new(),
        };
        let plain = posts.draft(&facts, &channel).await.unwrap();
        let pitched = posts.write_pitch(&s.anthropic, &plain, None).await.unwrap();
        (plain, pitched)
    });

    assert_eq!(pitched.pitch, PITCH);
    assert_eq!(
        pitched.draft.text,
        plain.text.replacen(
            "Curso de Fotografia\n",
            &format!("Curso de Fotografia\n{PITCH}\n"),
            1
        )
    );
    let sent = s
        .server
        .received()
        .into_iter()
        .find(|request| request.path == MESSAGES)
        .unwrap();
    let body: Value = serde_json::from_str(&sent.body).unwrap();
    let prompt = body["messages"][0]["content"].as_str().unwrap();
    assert!(prompt.contains("Onde o post sai: Canal do Telegram"));
    assert!(prompt.contains("Com desconto: não"));
    assert!(!prompt.contains("149"));
}
