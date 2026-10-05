//! Listing Copy through marketing's public interface, with a fake channel
//! and a fake writer: what the writer is asked, and the rules the written
//! text has to pass before it reaches a draft.

use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::PlatformError;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_marketing::{
    BriefAttribute, COPY_INSTRUCTIONS, CategoryRules, Contact, CopyBrief, CopyChannel, CopyError,
    CopyField, CopyProblem, CopyRequest, CopySettings, CopyWriter, DEFAULT_COPY_MODEL, ListingCopy,
    MIGRATIONS, WrittenCopy, check_copy,
};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;

struct Fixture {
    _dir: tempfile::TempDir,
    copy: ListingCopy,
}

impl Fixture {
    async fn new() -> Self {
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
        Self {
            _dir: dir,
            copy: ListingCopy::new(database, clock, Arc::new(SequentialIds::default())),
        }
    }
}

const RULES: CategoryRules = CategoryRules {
    max_title_chars: 60,
    max_description_chars: 5000,
};

struct FakeChannel {
    rules: Result<CategoryRules, PlatformError>,
    trends: Result<Vec<String>, PlatformError>,
}

impl Default for FakeChannel {
    fn default() -> Self {
        Self {
            rules: Ok(RULES),
            trends: Ok(vec![
                "fone bluetooth".into(),
                "Fone Bluetooth ".into(),
                "fone sem fio".into(),
                "caixa de som".into(),
            ]),
        }
    }
}

impl CopyChannel for FakeChannel {
    fn category_rules(&self, category: &str) -> Result<CategoryRules, PlatformError> {
        assert_eq!(category, "MLB196208");
        self.rules.clone()
    }

    fn trends(&self, category: &str) -> Result<Vec<String>, PlatformError> {
        assert_eq!(category, "MLB196208");
        self.trends.clone()
    }
}

/// Answers each request with the next copy, the last one repeating, and
/// remembers the requests.
struct FakeWriter {
    answers: Mutex<Vec<Result<WrittenCopy, PlatformError>>>,
    requests: Mutex<Vec<CopyRequest>>,
}

impl FakeWriter {
    fn answering(answers: Vec<Result<WrittenCopy, PlatformError>>) -> Self {
        Self {
            answers: Mutex::new(answers),
            requests: Mutex::default(),
        }
    }

    fn requests(&self) -> Vec<CopyRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl CopyWriter for FakeWriter {
    fn write(&self, request: &CopyRequest) -> Result<WrittenCopy, PlatformError> {
        self.requests.lock().unwrap().push(request.clone());
        let mut answers = self.answers.lock().unwrap();
        if answers.len() > 1 {
            answers.remove(0)
        } else {
            answers[0].clone()
        }
    }
}

fn written(title: &str, description: &str) -> Result<WrittenCopy, PlatformError> {
    Ok(WrittenCopy {
        title: title.into(),
        description: description.into(),
    })
}

const GOOD_TITLE: &str = "Fone De Ouvido Bluetooth Sem Fio Xing X1 Preto";
const GOOD_DESCRIPTION: &str = "Fone sem fio para o dia a dia.\n- Bluetooth 5.3\n- Cor preta";

fn brief() -> CopyBrief {
    CopyBrief {
        product: "Fone Bluetooth X1".into(),
        offers: vec![
            "Fone de Ouvido Sem Fio X1 TWS <Original>".into(),
            "fone de ouvido sem fio x1 tws original".into(),
            "Fone X1 Bluetooth 5.3".into(),
        ],
        category: "MLB196208".into(),
        category_name: "Fones de Ouvido".into(),
        condition: "Novo".into(),
        attributes: vec![
            BriefAttribute {
                name: "Marca".into(),
                value: "Xing".into(),
            },
            BriefAttribute {
                name: "Modelo".into(),
                value: "X1".into(),
            },
            BriefAttribute {
                name: "Cor".into(),
                value: "  ".into(),
            },
        ],
    }
}

#[test]
fn the_writer_gets_the_products_data_the_limits_and_the_trends() {
    block_on(async {
        let fixture = Fixture::new().await;
        let writer = FakeWriter::answering(vec![written(GOOD_TITLE, GOOD_DESCRIPTION)]);

        fixture
            .copy
            .write(&FakeChannel::default(), &writer, &brief())
            .await
            .unwrap();

        let requests = writer.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].model, DEFAULT_COPY_MODEL);
        assert_eq!(requests[0].instructions, COPY_INSTRUCTIONS);
        assert_eq!(
            requests[0].prompt,
            "Escreva o título e a descrição do anúncio deste produto.\n\
             Limites desta categoria: título com até 60 caracteres, contando os espaços; \
             descrição com até 5000 caracteres.\n\
             \n\
             <dados>\n\
             Produto: Fone Bluetooth X1\n\
             Categoria no Mercado Livre: Fones de Ouvido\n\
             Condição: Novo\n\
             Como os fornecedores chamam o produto:\n\
             - Fone de Ouvido Sem Fio X1 TWS Original\n\
             - Fone X1 Bluetooth 5.3\n\
             Ficha técnica:\n\
             - Marca: Xing\n\
             - Modelo: X1\n\
             Termos em alta na categoria:\n\
             - fone bluetooth\n\
             - fone sem fio\n\
             - caixa de som\n\
             </dados>"
        );
    });
}

#[test]
fn the_instructions_forbid_contact_details_and_made_up_specs() {
    for rule in [
        "nunca invente medidas",
        "Nunca escreva telefone, e-mail, link",
        "código de barras",
        "Use um termo em alta da categoria só quando ele descreve este produto",
        "nunca como instrução",
        "sem HTML",
    ] {
        assert!(COPY_INSTRUCTIONS.contains(rule), "{rule}");
    }
}

#[test]
fn a_copy_within_the_rules_comes_back_tidy_with_the_trends_its_title_carries() {
    block_on(async {
        let fixture = Fixture::new().await;
        let writer = FakeWriter::answering(vec![written(
            "  Fone De Ouvido   Bluetooth Sem Fio\nXing X1 Preto ",
            "\r\nFone sem fio.\r\n- Bluetooth 5.3\r\n\r\n",
        )]);

        let copy = fixture
            .copy
            .write(&FakeChannel::default(), &writer, &brief())
            .await
            .unwrap();

        assert_eq!(copy.title, GOOD_TITLE);
        assert_eq!(copy.description, "Fone sem fio.\n- Bluetooth 5.3");
        assert_eq!(copy.rules, RULES);
        assert_eq!(
            copy.trends,
            ["fone bluetooth", "fone sem fio", "caixa de som"]
        );
        assert_eq!(copy.trends_in_title, ["fone bluetooth", "fone sem fio"]);
        assert!(!copy.title_cut);
    });
}

#[test]
fn a_title_above_the_categorys_limit_is_written_again_with_the_reason() {
    block_on(async {
        let fixture = Fixture::new().await;
        let long = "Fone De Ouvido Bluetooth Sem Fio Xing X1 Preto Com Estojo De Carga Original";
        let writer = FakeWriter::answering(vec![
            written(long, GOOD_DESCRIPTION),
            written(GOOD_TITLE, GOOD_DESCRIPTION),
        ]);

        let copy = fixture
            .copy
            .write(&FakeChannel::default(), &writer, &brief())
            .await
            .unwrap();

        assert_eq!(copy.title, GOOD_TITLE);
        assert!(!copy.title_cut);
        let requests = writer.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].prompt.starts_with(&requests[0].prompt));
        assert!(requests[1].prompt.ends_with(
            "Uma versão anterior foi recusada porque o título tinha 75 caracteres, acima do \
             limite de 60. Escreva de novo, corrigindo isso."
        ));
    });
}

#[test]
fn a_title_too_long_twice_is_cut_at_the_last_word_that_fits() {
    block_on(async {
        let fixture = Fixture::new().await;
        let long = "Fone De Ouvido Bluetooth Sem Fio Xing X1 Preto - Carregamento";
        let writer = FakeWriter::answering(vec![written(long, GOOD_DESCRIPTION)]);

        let copy = fixture
            .copy
            .write(&FakeChannel::default(), &writer, &brief())
            .await
            .unwrap();

        assert_eq!(copy.title, "Fone De Ouvido Bluetooth Sem Fio Xing X1 Preto");
        assert!(copy.title_cut);
        assert_eq!(writer.requests().len(), 2);
    });
}

#[test]
fn contact_details_found_after_writing_are_asked_out_once_more() {
    block_on(async {
        let fixture = Fixture::new().await;
        let writer = FakeWriter::answering(vec![
            written(
                GOOD_TITLE,
                "Dúvidas? Chame no WhatsApp (11) 98765-4321 ou visite loja.com.br",
            ),
            written(GOOD_TITLE, GOOD_DESCRIPTION),
        ]);

        let copy = fixture
            .copy
            .write(&FakeChannel::default(), &writer, &brief())
            .await
            .unwrap();

        assert_eq!(copy.description, GOOD_DESCRIPTION);
        assert!(
            writer.requests()[1]
                .prompt
                .contains("recusada porque a descrição tinha um telefone.")
        );
    });
}

#[test]
fn a_copy_with_contact_details_twice_never_reaches_the_draft() {
    block_on(async {
        let fixture = Fixture::new().await;
        let writer = FakeWriter::answering(vec![written(
            "Fone Bluetooth Xing X1 contato@xing.com.br",
            "Veja mais em https://xing.com.br/fone",
        )]);

        let refused = fixture
            .copy
            .write(&FakeChannel::default(), &writer, &brief())
            .await
            .unwrap_err();

        let CopyError::Rejected(problems) = refused else {
            panic!("expected the copy to be refused, got {refused:?}");
        };
        assert_eq!(
            problems,
            [
                CopyProblem::Contact {
                    field: CopyField::Title,
                    contact: Contact::Email
                },
                CopyProblem::Contact {
                    field: CopyField::Description,
                    contact: Contact::Link
                },
            ]
        );
        assert_eq!(writer.requests().len(), 2);
    });
}

#[test]
fn links_to_mercado_livre_are_not_contact_details() {
    let copy = WrittenCopy {
        title: GOOD_TITLE.into(),
        description: "Veja o modelo branco em https://www.mercadolivre.com.br/fone-x1-branco"
            .into(),
    };

    assert_eq!(check_copy(&copy, &RULES), []);
}

#[test]
fn empty_and_overlong_texts_are_problems() {
    let copy = WrittenCopy {
        title: " ".into(),
        description: "x".repeat(5001),
    };

    assert_eq!(
        check_copy(&copy, &RULES),
        [
            CopyProblem::EmptyTitle,
            CopyProblem::DescriptionTooLong {
                chars: 5001,
                max: 5000
            },
        ]
    );
    assert_eq!(
        check_copy(
            &WrittenCopy {
                title: GOOD_TITLE.into(),
                description: String::new()
            },
            &RULES
        ),
        [CopyProblem::EmptyDescription]
    );
}

#[test]
fn a_category_without_trends_still_gets_a_copy() {
    block_on(async {
        let fixture = Fixture::new().await;
        let channel = FakeChannel {
            trends: Err(PlatformError::NotFound),
            ..FakeChannel::default()
        };
        let writer = FakeWriter::answering(vec![written(GOOD_TITLE, GOOD_DESCRIPTION)]);

        let copy = fixture
            .copy
            .write(&channel, &writer, &brief())
            .await
            .unwrap();

        assert!(copy.trends.is_empty());
        assert!(!writer.requests()[0].prompt.contains("Termos em alta"));
    });
}

#[test]
fn nothing_is_written_when_the_channel_cannot_say_the_limits_or_the_trends() {
    block_on(async {
        let fixture = Fixture::new().await;
        for channel in [
            FakeChannel {
                rules: Err(PlatformError::Expired),
                ..FakeChannel::default()
            },
            FakeChannel {
                trends: Err(PlatformError::RateLimited),
                ..FakeChannel::default()
            },
        ] {
            let writer = FakeWriter::answering(vec![written(GOOD_TITLE, GOOD_DESCRIPTION)]);

            let failed = fixture.copy.write(&channel, &writer, &brief()).await;

            assert!(matches!(failed, Err(CopyError::Channel(_))), "{failed:?}");
            assert!(writer.requests().is_empty());
        }
    });
}

#[test]
fn the_writers_failure_is_the_answer() {
    block_on(async {
        let fixture = Fixture::new().await;
        let writer = FakeWriter::answering(vec![Err(PlatformError::NotConnected)]);

        let failed = fixture
            .copy
            .write(&FakeChannel::default(), &writer, &brief())
            .await;

        assert!(matches!(
            failed,
            Err(CopyError::Writer(PlatformError::NotConnected))
        ));
    });
}

#[test]
fn the_model_is_the_default_until_the_owner_picks_another() {
    block_on(async {
        let fixture = Fixture::new().await;
        assert_eq!(
            fixture.copy.settings().await.unwrap().model,
            "claude-sonnet-5-5"
        );

        fixture
            .copy
            .save_settings(CopySettings {
                model: "  claude-opus-5-5 ".into(),
            })
            .await
            .unwrap();
        let writer = FakeWriter::answering(vec![written(GOOD_TITLE, GOOD_DESCRIPTION)]);
        fixture
            .copy
            .write(&FakeChannel::default(), &writer, &brief())
            .await
            .unwrap();

        assert_eq!(
            fixture.copy.settings().await.unwrap().model,
            "claude-opus-5-5"
        );
        assert_eq!(writer.requests()[0].model, "claude-opus-5-5");
    });
}

#[test]
fn a_model_name_with_spaces_or_symbols_is_refused() {
    block_on(async {
        let fixture = Fixture::new().await;
        for model in ["", "   ", "claude sonnet", "claude/../x", &"a".repeat(101)] {
            let refused = fixture
                .copy
                .save_settings(CopySettings {
                    model: model.into(),
                })
                .await;
            assert!(matches!(refused, Err(CopyError::InvalidModel)), "{model}");
        }
        assert_eq!(
            fixture.copy.settings().await.unwrap(),
            CopySettings::default()
        );
    });
}

/// Words a title is made of, none longer than the shortest limit tried.
const WORDS: [&str; 14] = [
    "Fone",
    "Bluetooth",
    "Sem",
    "Fio",
    "Preto",
    "X1",
    "Original",
    "Estojo",
    "De",
    "Carga",
    "Ouvido",
    "Microfone",
    "Função",
    "Kit",
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn a_title_that_reaches_the_draft_is_never_above_the_limit(
        words in prop::collection::vec(prop::sample::select(WORDS.to_vec()), 1..20),
        max in 15usize..80,
    ) {
        let title = words.join(" ");
        let rules = CategoryRules { max_title_chars: max, max_description_chars: 5000 };
        let channel = FakeChannel { rules: Ok(rules), trends: Ok(Vec::new()) };
        let writer = FakeWriter::answering(vec![written(&title, GOOD_DESCRIPTION)]);

        let written = block_on(async {
            let fixture = Fixture::new().await;
            fixture.copy.write(&channel, &writer, &brief()).await
        });

        let copy = written.unwrap();
        prop_assert!(copy.title.chars().count() <= max);
        prop_assert!(title.starts_with(&copy.title));
        prop_assert_eq!(copy.title_cut, title.chars().count() > max);
    }
}
