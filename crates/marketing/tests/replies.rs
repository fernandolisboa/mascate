//! Reply templates and the contact rule through marketing's public
//! interface: templates are kept with their variables, fill in the
//! product's name and the dispatch time, and no template or answer carries
//! contact details or links off Mercado Livre.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{RecordId, fold};
use mascate_marketing::{
    Contact, MIGRATIONS, NewReplyTemplate, QuestionError, QuestionSettings, ReplyProblem,
    ReplyTemplates, check_answer, contact_in,
};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;

struct Fixture {
    _dir: tempfile::TempDir,
    templates: ReplyTemplates,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
        ));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        Self {
            _dir: dir,
            templates: ReplyTemplates::new(database, clock, Arc::new(SequentialIds::default())),
        }
    }
}

fn template(name: &str, text: &str) -> NewReplyTemplate {
    NewReplyTemplate {
        name: name.into(),
        text: text.into(),
    }
}

const SHIPPING: &str = "Olá! O {produto} sai em até {prazo} depois do pagamento.";

#[test]
fn a_template_fills_the_product_name_and_the_dispatch_time() {
    block_on(async {
        let fixture = Fixture::new().await;
        let saved = fixture
            .templates
            .create(template("  Prazo de envio ", SHIPPING))
            .await
            .unwrap();

        assert_eq!(saved.name, "Prazo de envio");
        assert_eq!(
            saved.fill("Fone Bluetooth", &QuestionSettings::default()),
            "Olá! O Fone Bluetooth sai em até 1 dia útil depois do pagamento."
        );
        assert_eq!(
            saved.fill(" Fone Bluetooth ", &QuestionSettings { dispatch_days: 3 }),
            "Olá! O Fone Bluetooth sai em até 3 dias úteis depois do pagamento."
        );
        assert_eq!(fixture.templates.templates().await.unwrap(), [saved]);
    });
}

#[test]
fn templates_are_created_listed_by_name_changed_and_deleted() {
    block_on(async {
        let fixture = Fixture::new().await;
        let shipping = fixture
            .templates
            .create(template("Prazo de envio", SHIPPING))
            .await
            .unwrap();
        let warranty = fixture
            .templates
            .create(template("Garantia", "O {produto} tem 3 meses de garantia."))
            .await
            .unwrap();
        assert_eq!(
            fixture.templates.templates().await.unwrap(),
            [warranty.clone(), shipping.clone()]
        );

        let changed = fixture
            .templates
            .update(
                warranty.id,
                template("Garantia", "O {produto} tem 6 meses de garantia."),
            )
            .await
            .unwrap();
        fixture.templates.delete(shipping.id).await.unwrap();

        assert_eq!(fixture.templates.templates().await.unwrap(), [changed]);
        assert!(matches!(
            fixture.templates.delete(shipping.id).await,
            Err(QuestionError::NotFound)
        ));
        assert!(matches!(
            fixture
                .templates
                .update(RecordId::from_u128(77), template("Outro", "Texto"))
                .await,
            Err(QuestionError::NotFound)
        ));
    });
}

#[test]
fn two_templates_never_share_a_name_and_a_deleted_one_frees_it() {
    block_on(async {
        let fixture = Fixture::new().await;
        let first = fixture
            .templates
            .create(template("Garantia", "Tem garantia."))
            .await
            .unwrap();

        let taken = fixture
            .templates
            .create(template("garantia", "Outra."))
            .await;
        assert!(
            matches!(&taken, Err(QuestionError::Reply(ReplyProblem::NameTaken(name))) if name == "garantia"),
            "{taken:?}"
        );
        // Saving a template under its own name is not taking it.
        fixture
            .templates
            .update(first.id, template("Garantia", "Tem 3 meses."))
            .await
            .unwrap();

        fixture.templates.delete(first.id).await.unwrap();
        assert!(
            fixture
                .templates
                .create(template("Garantia", "Outra."))
                .await
                .is_ok()
        );
    });
}

#[test]
fn a_template_is_refused_with_an_unknown_variable_contact_details_or_no_name() {
    block_on(async {
        let fixture = Fixture::new().await;
        for (new, problem) in [
            (
                template("Cor", "O {produto} vem na cor {cor}."),
                ReplyProblem::UnknownVariable("{cor}".into()),
            ),
            (
                template("Sem fim", "O {produto"),
                ReplyProblem::UnknownVariable("{produto".into()),
            ),
            (
                template("Zap", "Chama no zap para combinar."),
                ReplyProblem::Contact(Contact::SocialNetwork),
            ),
            (
                template("Site", "Veja mais em minhaloja.com.br/{produto}"),
                ReplyProblem::Contact(Contact::Link),
            ),
            (template("   ", "Texto"), ReplyProblem::BadName),
            (template(&"n".repeat(61), "Texto"), ReplyProblem::BadName),
            (template("Vazio", "  "), ReplyProblem::Empty),
            (template("Só variável", "{produto}"), ReplyProblem::Empty),
        ] {
            let created = fixture.templates.create(new.clone()).await;
            assert!(
                matches!(&created, Err(QuestionError::Reply(found)) if *found == problem),
                "{new:?}: {created:?}"
            );
        }
        assert!(fixture.templates.templates().await.unwrap().is_empty());
    });
}

#[test]
fn the_dispatch_time_is_kept_and_stays_within_its_range() {
    block_on(async {
        let fixture = Fixture::new().await;
        assert_eq!(
            fixture.templates.settings().await.unwrap(),
            QuestionSettings { dispatch_days: 1 }
        );

        fixture
            .templates
            .save_settings(QuestionSettings { dispatch_days: 2 })
            .await
            .unwrap();

        for wrong in [0, 31] {
            assert!(matches!(
                fixture
                    .templates
                    .save_settings(QuestionSettings {
                        dispatch_days: wrong
                    })
                    .await,
                Err(QuestionError::InvalidSettings)
            ));
        }
        assert_eq!(
            fixture.templates.settings().await.unwrap(),
            QuestionSettings { dispatch_days: 2 }
        );
    });
}

#[test]
fn contact_details_are_found_in_their_usual_shapes() {
    for (text, contact) in [
        ("Ligue (11) 98765-4321", Contact::Phone),
        ("Whats 11987654321", Contact::Phone),
        ("+55 11 98765 4321", Contact::Phone),
        ("Fixo 3333-4444", Contact::Phone),
        ("Acesse https://minhaloja.com.br/fone", Contact::Link),
        (
            "Acesse http://produto.mercadolivre.com.br/MLB-1",
            Contact::Link,
        ),
        ("Veja em www.loja.net", Contact::Link),
        ("Veja em bit.ly/oferta.", Contact::Link),
        ("Escreva para vendas@loja.com.br", Contact::Email),
        ("Siga @minhaloja no perfil", Contact::SocialNetwork),
        ("Chama no WhatsApp!", Contact::SocialNetwork),
        ("Me chama no zap", Contact::SocialNetwork),
        ("Estamos no Instagram", Contact::SocialNetwork),
        ("Fale pelo Telegram", Contact::SocialNetwork),
    ] {
        assert_eq!(contact_in(text), Some(contact), "{text}");
    }
}

#[test]
fn everyday_answers_carry_no_contact_details() {
    for text in [
        "Olá! Temos sim, envio em até 1 dia útil.",
        "Custa R$ 1.299,90 em até 12x sem juros.",
        "O conector é de 3.5mm e o cabo tem 1.2m.",
        "Temos os tamanhos 38 39 40 41 42.",
        "Medidas: 20 x 30 x 40 cm, 1,5 kg.",
        "Modelo SM-A546E, versão 2.0.",
        "Veja o outro anúncio: https://produto.mercadolivre.com.br/MLB-2",
        "Sim, Sr. Cliente. Obrigado!",
        "E-mail de confirmação vem do Mercado Livre.",
    ] {
        assert_eq!(contact_in(text), None, "{text}");
        assert_eq!(check_answer(text), Ok(()), "{text}");
    }
}

/// Words with no digits, `@`, dots or slashes, none of them a network,
/// accents aside.
fn plain_text() -> impl Strategy<Value = String> {
    const NETWORKS: [&str; 12] = [
        "WHATSAPP",
        "WHATS",
        "WPP",
        "ZAP",
        "ZAPZAP",
        "TELEGRAM",
        "INSTAGRAM",
        "INSTA",
        "FACEBOOK",
        "TIKTOK",
        "MESSENGER",
        "SIGNAL",
    ];
    prop::collection::vec("[a-záéíóúãõç]{1,12}", 1..30).prop_map(|words| {
        words
            .into_iter()
            .filter(|word| {
                let folded: String = word.chars().filter_map(fold).collect();
                !NETWORKS.contains(&folded.as_str())
            })
            .collect::<Vec<_>>()
            .join(" ")
    })
}

fn contact() -> impl Strategy<Value = (String, Contact)> {
    prop_oneof![
        ("[a-z]{2,10}", "[a-z]{2,10}")
            .prop_map(|(user, host)| (format!("{user}@{host}.com.br"), Contact::Email)),
        "[a-z]{2,12}".prop_map(|site| (format!("https://{site}.com/oferta"), Contact::Link)),
        "[a-z]{2,12}".prop_map(|site| (format!("{site}.com.br"), Contact::Link)),
        ("[1-9]{2}", "9[0-9]{4}", "[0-9]{4}")
            .prop_map(|(area, first, last)| (format!("({area}) {first}-{last}"), Contact::Phone)),
        ("[1-9]{2}", "9[0-9]{8}")
            .prop_map(|(area, number)| (format!("{area}{number}"), Contact::Phone)),
    ]
}

proptest! {
    #[test]
    fn plain_words_are_never_taken_for_contact_details(text in plain_text()) {
        prop_assert_eq!(contact_in(&text), None);
    }

    #[test]
    fn contact_details_are_found_wherever_they_go(
        before in plain_text(),
        (detail, kind) in contact(),
        after in plain_text(),
    ) {
        let text = format!("{before} {detail} {after}");
        prop_assert_eq!(contact_in(&text), Some(kind));
        prop_assert_eq!(check_answer(&text), Err(ReplyProblem::Contact(kind)));
    }

    #[test]
    fn a_filled_template_has_no_variables_left_and_names_the_product(
        product in "[A-Za-z][A-Za-z0-9 ]{0,30}",
        days in 1u8..=30,
    ) {
        let template = mascate_marketing::ReplyTemplate {
            id: RecordId::from_u128(1),
            name: "Prazo".into(),
            text: SHIPPING.into(),
        };
        let filled = template.fill(&product, &QuestionSettings { dispatch_days: days });
        prop_assert!(!filled.contains(['{', '}']), "{filled}");
        prop_assert!(filled.contains(product.trim()));
        prop_assert!(filled.contains(&days.to_string()));
    }
}
