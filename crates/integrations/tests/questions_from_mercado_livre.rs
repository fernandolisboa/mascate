//! Buyers' questions on the owner's listings through Mercado Livre's
//! adapter, against the fake server serving the answers in
//! `fixtures/mercado-livre`, written from the documentation (see its
//! README). What marketing does with them is tested there.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use mascate_integrations::MercadoLivre;
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{PlatformError, Timestamp};
use mascate_marketing::{ChannelAnswer, ChannelQuestion, ChannelQuestions, QuestionStatus};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};

const SEARCH: &str = "/questions/search?seller_id=1234567&status=UNANSWERED&api_version=4\
                      &sort_fields=date_created&sort_types=ASC&offset=0&limit=50";
const SEARCH_NEXT: &str = "/questions/search?seller_id=1234567&status=UNANSWERED\
                           &api_version=4&sort_fields=date_created&sort_types=ASC&offset=2\
                           &limit=50";
const ANSWERED: &str = "/questions/13001000001?api_version=4";
const BANNED: &str = "/questions/13001000004?api_version=4";
const ANSWERS: &str = "POST /answers";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/mercado-livre/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn at(text: &str) -> Timestamp {
    chrono::DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

fn unanswered(id: &str, listing: &str, text: &str, asked_at: &str) -> ChannelQuestion {
    ChannelQuestion {
        id: id.into(),
        listing: listing.into(),
        text: text.into(),
        status: QuestionStatus::Unanswered,
        asked_at: at(asked_at),
        answer: None,
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
            Utc.with_ymd_and_hms(2026, 10, 4, 15, 0, 0).unwrap(),
        ));
        let adapter = MercadoLivre::new(server.url(), "Mascate/test", store, clock);
        server.serve("/oauth/token", 200, fixture("token.json"));
        server.serve("/users/me", 200, fixture("users-me.json"));
        Self { server, adapter }
    }

    /// The bodies of the answers posted, as JSON.
    fn posted(&self) -> Vec<serde_json::Value> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.method == "POST" && request.path == "/answers")
            .map(|request| serde_json::from_str(&request.body).unwrap())
            .collect()
    }
}

#[test]
fn the_unanswered_questions_come_from_every_page_of_the_sellers_search() {
    let fake = Fake::connected();
    fake.server
        .serve(SEARCH, 200, fixture("questions-search-unanswered.json"));
    fake.server.serve(
        SEARCH_NEXT,
        200,
        fixture("questions-search-unanswered-end.json"),
    );

    let questions = fake.adapter.unanswered().unwrap();

    assert_eq!(
        questions,
        [
            unanswered(
                "13001000001",
                "MLB4100000001",
                "Boa tarde, o fone funciona com iPhone?",
                "2026-10-04T08:15:02-03:00",
            ),
            unanswered(
                "13001000002",
                "MLB4100000003",
                "Tem a camiseta no tamanho G na cor preta?",
                "2026-10-04T10:40:47-03:00",
            ),
            unanswered(
                "13001000003",
                "MLB4100000001",
                "Qual o prazo de envio para Curitiba?",
                "2026-10-04T11:58:10-03:00",
            ),
        ]
    );
    let requests = fake.server.requests();
    assert!(requests.contains(&SEARCH.to_owned()), "{requests:?}");
    assert!(requests.contains(&SEARCH_NEXT.to_owned()), "{requests:?}");
}

#[test]
fn no_unanswered_question_is_an_empty_inbox() {
    let fake = Fake::connected();
    fake.server
        .serve(SEARCH, 200, fixture("questions-search-empty.json"));

    assert_eq!(fake.adapter.unanswered().unwrap(), []);
}

#[test]
fn one_question_comes_with_its_answer_and_a_removed_one_with_no_text() {
    let fake = Fake::connected();
    fake.server
        .serve(ANSWERED, 200, fixture("question-13001000001.json"));
    fake.server
        .serve(BANNED, 200, fixture("question-13001000004-banned.json"));

    assert_eq!(
        fake.adapter.question("13001000001").unwrap(),
        Some(ChannelQuestion {
            status: QuestionStatus::Answered,
            answer: Some(ChannelAnswer {
                text: "Olá! Funciona sim, com qualquer celular com Bluetooth.".into(),
                at: at("2026-10-04T09:02:31.069-03:00"),
            }),
            ..unanswered(
                "13001000001",
                "MLB4100000001",
                "Boa tarde, o fone funciona com iPhone?",
                "2026-10-04T08:15:02-03:00",
            )
        })
    );
    let banned = fake.adapter.question("13001000004").unwrap().unwrap();
    assert_eq!(banned.status, QuestionStatus::Banned);
    assert_eq!(banned.text, "");
}

#[test]
fn a_question_mercado_livre_no_longer_has_is_none() {
    let fake = Fake::connected();
    fake.server.serve(
        "/questions/13001000009?api_version=4",
        404,
        fixture("error-404-question.json"),
    );

    assert_eq!(fake.adapter.question("13001000009").unwrap(), None);
}

#[test]
fn an_id_that_is_not_mercado_livres_never_reaches_a_path_or_a_body() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter.question("1/../../users/me"),
        Err(PlatformError::NotFound)
    );
    assert_eq!(
        fake.adapter.answer("MLB1", "Temos."),
        Err(PlatformError::NotFound)
    );
    assert!(fake.server.requests().iter().all(|path| path != "/answers"));
}

#[test]
fn the_answer_goes_with_the_questions_number_and_the_text_as_written() {
    let fake = Fake::connected();
    fake.server
        .serve_method("POST", "/answers", 200, fixture("answer-posted.json"));
    let text = "Olá! Temos sim, no tamanho G na cor \"preta\".\nAbraço!";

    fake.adapter.answer("13001000002", text).unwrap();

    assert_eq!(
        fake.posted(),
        [serde_json::json!({ "question_id": 13001000002u64, "text": text })]
    );
    let request = fake
        .server
        .received()
        .into_iter()
        .find(|request| request.path == "/answers")
        .unwrap();
    assert_eq!(
        request.authorization.as_deref(),
        Some("Bearer APP_USR-123456-090515-8cc4448aac10d5105474e1351-1234567")
    );
    assert!(
        request
            .headers
            .contains(&("content-type".into(), "application/json".into()))
    );
}

#[test]
fn an_answer_mercado_livre_refuses_says_why() {
    let fake = Fake::connected();
    fake.server
        .serve_method("POST", "/answers", 400, fixture("error-400-answer.json"));

    assert_eq!(
        fake.adapter.answer("13001000002", "Temos."),
        Err(PlatformError::Refused(
            "Error validating question_id and text".into()
        ))
    );
}

#[test]
fn a_forbidden_or_rate_limited_answer_is_not_sent_as_done() {
    let fake = Fake::connected();
    for (status, body, error) in [
        (
            403,
            fixture("error-403.json"),
            PlatformError::Refused("forbidden".into()),
        ),
        (429, fixture("error-429.json"), PlatformError::RateLimited),
    ] {
        fake.server.serve_method("POST", "/answers", status, body);
        assert_eq!(fake.adapter.answer("13001000002", "Temos."), Err(error));
    }
}

#[test]
fn an_answer_turned_down_for_an_old_token_goes_once_more_with_a_renewed_one() {
    let fake = Fake::connected();
    fake.server
        .serve_method("POST", "/answers", 401, r#"{"message":"invalid_token"}"#);
    fake.server
        .then_serve(ANSWERS, 200, fixture("answer-posted.json"));

    fake.adapter.answer("13001000002", "Temos.").unwrap();

    assert_eq!(fake.posted().len(), 2);
    let tokens = fake
        .server
        .requests()
        .iter()
        .filter(|path| *path == "/oauth/token")
        .count();
    assert_eq!(tokens, 2);
}
