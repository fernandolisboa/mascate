//! The quality and visits of the owner's listings on Mercado Livre, through
//! its adapter, against the fake server serving the answers in
//! `fixtures/mercado-livre`, written from the documentation (see its
//! README). What marketing does with them is tested there.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use mascate_integrations::MercadoLivre;
use mascate_kernel::PlatformError;
use mascate_kernel::testing::ManualClock;
use mascate_marketing::{ActionKind, ChannelQuality, QualityAction, QualityLevel, QualitySource};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};

const FONE: &str = "MLB4100000001";
const PERFORMANCE: &str = "/item/MLB4100000001/performance";
const VISITS: &str = "/items/MLB4100000001/visits/time_window?last=30&unit=day";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/mercado-livre/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn performance() -> serde_json::Value {
    serde_json::from_str(&fixture("item-MLB4100000001-performance.json")).unwrap()
}

fn fix_link(task: &str) -> Option<String> {
    Some(format!(
        "https://www.mercadolivre.com.br/syi/core/modify?taskId={task}&itemId={FONE}"
    ))
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
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
        ));
        let adapter = MercadoLivre::new(server.url(), "Mascate/test", store, clock);
        server.serve("/oauth/token", 200, fixture("token.json"));
        Self { server, adapter }
    }

    fn quality_with(&self, answer: serde_json::Value) -> ChannelQuality {
        self.server.serve(PERFORMANCE, 200, answer.to_string());
        self.adapter.quality(FONE).unwrap().unwrap()
    }
}

#[test]
fn the_quality_comes_with_its_level_score_and_only_the_pending_actions() {
    let fake = Fake::connected();
    fake.server.serve(
        PERFORMANCE,
        200,
        fixture("item-MLB4100000001-performance.json"),
    );

    let quality = fake.adapter.quality(FONE).unwrap();

    assert_eq!(
        quality,
        Some(ChannelQuality {
            score: 69,
            level: QualityLevel::Professional,
            pending: vec![
                QualityAction {
                    key: "PICTURES_QUANTITY_MIN".into(),
                    kind: ActionKind::Opportunity,
                    text: "Adicione mais fotos para mostrar seu produto de diferentes ângulos, \
                           envie no mínimo 3."
                        .into(),
                    label: "Adicionar fotos".into(),
                    link: fix_link("picture_uploader_task"),
                },
                QualityAction {
                    key: "TS_MAIN_QUANTITY".into(),
                    kind: ActionKind::Opportunity,
                    text: "Complete as características principais.".into(),
                    label: "Completar ficha técnica".into(),
                    link: fix_link("technical_specifications_task"),
                },
                // A warning lowers the score until it is fixed.
                QualityAction {
                    key: "TS_MAIN_QUALITY_INCOMPLETE_REQUIRED".into(),
                    kind: ActionKind::Problem,
                    text: "Complete os dados marcados como “obrigatórios”.".into(),
                    label: "Completar ficha técnica".into(),
                    link: fix_link("technical_specifications_task"),
                },
                QualityAction {
                    key: "BEST_FINANCING".into(),
                    kind: ActionKind::Opportunity,
                    text: "Ofereça parcelamento pelo mesmo preço que você anunciou para que seu \
                           anúncio seja mais competitivo"
                        .into(),
                    label: "Adicionar parcelamento".into(),
                    link: fix_link("listing_types_task"),
                },
            ],
        })
    );
}

#[test]
fn a_listing_mercado_livre_has_not_rated_yet_has_no_quality() {
    let fake = Fake::connected();
    fake.server
        .serve(PERFORMANCE, 404, fixture("error-404-performance.json"));

    assert_eq!(fake.adapter.quality(FONE).unwrap(), None);
}

#[test]
fn the_level_follows_the_score_when_mercado_livre_does_not_name_it() {
    let fake = Fake::connected();
    for (score, level) in [
        (49.6, QualityLevel::Standard),
        (49.4, QualityLevel::Basic),
        (65.0, QualityLevel::Standard),
        (66.0, QualityLevel::Professional),
    ] {
        let mut answer = performance();
        answer["score"] = score.into();
        answer["level"] = serde_json::Value::Null;
        answer["level_wording"] = "Excelente".into();

        let quality = fake.quality_with(answer);

        assert_eq!(quality.level, level, "{score}");
    }
    let mut basic = performance();
    basic["score"] = 30.into();
    basic["level"] = "Bad".into();
    basic["level_wording"] = "Básica".into();
    assert_eq!(fake.quality_with(basic).level, QualityLevel::Basic);
}

#[test]
fn an_action_without_its_own_words_takes_the_goals_and_a_link_off_mercado_livre_is_dropped() {
    let fake = Fake::connected();
    let mut answer = performance();
    let pictures = &mut answer["buckets"][0]["variables"][1];
    let rule = &mut pictures["rules"][0];
    rule["wordings"]["title"] = "  ".into();
    rule["wordings"]["label"] = serde_json::Value::Null;
    rule["wordings"]["link"] = "https://mercadolivre.com.br.example.com/syi".into();
    let specs = &mut answer["buckets"][0]["variables"][3]["rules"][0];
    specs["wordings"]["link"] = "http://www.mercadolivre.com.br/syi".into();
    let financing = &mut answer["buckets"][1]["variables"][1]["rules"][0];
    financing["wordings"]["link"] = "javascript:alert(1)".into();

    let quality = fake.quality_with(answer);

    let photos = &quality.pending[0];
    assert_eq!(photos.text, "Melhore as fotos para ter mais visitas");
    assert_eq!(photos.label, "Corrigir no Mercado Livre");
    assert_eq!(photos.link, None);
    assert_eq!(quality.pending[1].link, None);
    assert_eq!(quality.pending[3].link, None);
    assert_eq!(
        quality.pending[2].link,
        fix_link("technical_specifications_task")
    );
}

#[test]
fn the_visits_are_the_last_thirty_days_total() {
    let fake = Fake::connected();
    fake.server
        .serve(VISITS, 200, fixture("item-MLB4100000001-visits.json"));

    assert_eq!(fake.adapter.visits(FONE, 30).unwrap(), 412);
}

#[test]
fn refusals_and_rate_limits_say_so() {
    let fake = Fake::connected();
    fake.server
        .serve(PERFORMANCE, 429, fixture("error-429.json"));
    fake.server.serve(VISITS, 403, fixture("error-403.json"));

    assert_eq!(fake.adapter.quality(FONE), Err(PlatformError::RateLimited));
    assert_eq!(
        fake.adapter.visits(FONE, 30),
        Err(PlatformError::Refused("forbidden".into()))
    );
}

#[test]
fn an_id_that_is_not_mercado_livres_is_never_asked_about() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter.quality("../users/me"),
        Err(PlatformError::NotFound)
    );
    assert_eq!(
        fake.adapter.visits("MLB1?x=1", 30),
        Err(PlatformError::NotFound)
    );
    assert!(
        fake.server
            .requests()
            .iter()
            .all(|path| path.starts_with("/oauth/token"))
    );
}
