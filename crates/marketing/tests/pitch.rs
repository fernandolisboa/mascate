//! The Pitch of an Offer Post through the Marketing module's public
//! interface with a real temporary database: written by a fake writer in
//! the Pitch shape, checked, asked for once more with what was wrong, and
//! put in the template's `{chamada}` without touching the price, the link,
//! the discount or the disclosure.

use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, PlatformError, RecordId};
use mascate_marketing::{
    ChannelKind, Contact, CopyRequest, CopySettings, CopyShape, CopyWriter, DEFAULT_COPY_MODEL,
    DISCLOSURE, ListingCopy, MIGRATIONS, NewPromotionChannel, OfferFacts, OfferPosts, OfferPrice,
    PITCH_INSTRUCTIONS, PITCH_MAX_CHARS, POST_VARIABLES, PitchError, PitchProblem,
    PromotionChannel, PromotionChannels, Written, WrittenCopy, check_pitch,
};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

struct Fixture {
    _dir: tempfile::TempDir,
    channels: PromotionChannels,
    posts: OfferPosts,
    copy: ListingCopy,
    made: Mutex<usize>,
}

impl Fixture {
    async fn new() -> Self {
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
        Self {
            _dir: dir,
            channels: PromotionChannels::new(database.clone(), clock.clone(), ids.clone()),
            posts: OfferPosts::new(database.clone(), clock.clone(), ids.clone()),
            copy: ListingCopy::new(database, clock, ids),
            made: Mutex::default(),
        }
    }

    async fn channel(&self, kind: ChannelKind) -> PromotionChannel {
        let made = {
            let mut made = self.made.lock().unwrap();
            *made += 1;
            *made
        };
        self.channels
            .create(&NewPromotionChannel {
                name: format!("Canal {made}"),
                kind,
                default_tag: format!("canal{made}"),
                keywords: Vec::new(),
                telegram_chat: None,
            })
            .await
            .unwrap()
    }
}

/// Answers each request with the next text, the last one repeating, and
/// remembers the requests.
struct FakeWriter {
    answers: Mutex<Vec<Result<Written, PlatformError>>>,
    requests: Mutex<Vec<CopyRequest>>,
}

impl FakeWriter {
    fn answering(answers: Vec<Result<Written, PlatformError>>) -> Self {
        Self {
            answers: Mutex::new(answers),
            requests: Mutex::default(),
        }
    }

    fn pitching(pitches: &[&str]) -> Self {
        Self::answering(
            pitches
                .iter()
                .map(|pitch| Ok(Written::Pitch((*pitch).to_owned())))
                .collect(),
        )
    }

    fn requests(&self) -> Vec<CopyRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl CopyWriter for FakeWriter {
    fn write(&self, request: &CopyRequest) -> Result<Written, PlatformError> {
        self.requests.lock().unwrap().push(request.clone());
        let mut answers = self.answers.lock().unwrap();
        if answers.len() > 1 {
            answers.remove(0)
        } else {
            answers[0].clone()
        }
    }
}

fn brl(cents: i64) -> Money {
    Money::new(Decimal::new(cents, 2), Currency::Brl)
}

const LINK: &str = "https://s.shopee.com.br/7AbCdEfGh";
const PITCH: &str = "Fotos de revista com o celular que você já tem no bolso";
const OTHER_PITCH: &str = "Aprenda a fotografar como profissional sem sair de casa";

fn facts() -> OfferFacts {
    OfferFacts {
        link_id: RecordId::from_u128(900),
        link: LINK.into(),
        product: "Curso de Fotografia <Completo>".into(),
        platform: "Hotmart".into(),
        price: Some(OfferPrice {
            price: brl(14990),
            before: Some(brl(19990)),
            seen_at: Utc.with_ymd_and_hms(2026, 10, 26, 11, 30, 0).unwrap(),
        }),
        image: None,
        price_needs_time: false,
        forbidden_contacts: Vec::new(),
    }
}

/// `text` without the line that is exactly `pitch`.
fn without_line(text: &str, pitch: &str) -> String {
    text.lines()
        .filter(|line| *line != pitch)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_written_pitch_fills_chamada_and_leaves_the_rest_byte_for_byte() {
    block_on(async {
        let f = Fixture::new().await;
        for kind in ChannelKind::ALL {
            let channel = f.channel(kind).await;
            let draft = f.posts.draft(&facts(), &channel).await.unwrap();
            let writer = FakeWriter::pitching(&[PITCH]);

            let pitched = f.posts.write_pitch(&writer, &draft, None).await.unwrap();

            assert_eq!(pitched.pitch, PITCH);
            assert!(!pitched.edits_replaced, "{kind:?}");
            assert_eq!(pitched.draft.facts, draft.facts);
            assert_eq!(pitched.draft.channel, draft.channel);
            assert!(pitched.draft.text.lines().any(|line| line == PITCH));
            assert_eq!(
                without_line(&pitched.draft.text, PITCH),
                draft.text,
                "{kind:?}"
            );
            assert_eq!(
                f.posts.check(&pitched.draft).await.unwrap(),
                f.posts.check(&draft).await.unwrap(),
                "{kind:?}"
            );
        }
        let telegram = f.channel(ChannelKind::TelegramChannel).await;
        let draft = f.posts.draft(&facts(), &telegram).await.unwrap();
        let pitched = f
            .posts
            .write_pitch(&FakeWriter::pitching(&[PITCH]), &draft, None)
            .await
            .unwrap();
        assert_eq!(
            pitched.draft.text,
            format!(
                "🔥 Curso de Fotografia <Completo>\n{PITCH}\n\nDe R$ 199,90\nPor R$ 149,90\n\
                 📉 25% de desconto\n\n👉 {LINK}\n\n{DISCLOSURE}"
            )
        );
    });
}

#[test]
fn the_writer_is_told_only_what_the_post_shows_in_the_pitch_shape() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::Instagram).await;
        let draft = f.posts.draft(&facts(), &channel).await.unwrap();
        let writer = FakeWriter::pitching(&[PITCH]);

        f.posts.write_pitch(&writer, &draft, None).await.unwrap();

        let requests = writer.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].shape, CopyShape::Pitch);
        assert_eq!(requests[0].model, DEFAULT_COPY_MODEL);
        assert_eq!(requests[0].instructions, PITCH_INSTRUCTIONS);
        assert_eq!(
            requests[0].prompt,
            "Escreva a chamada deste post: uma frase, numa linha, com até 120 caracteres.\n\n\
             <dados>\nProduto: Curso de Fotografia Completo\nPrograma de afiliados: Hotmart\n\
             Onde o post sai: Instagram\nCom desconto: sim\n</dados>"
        );
        for absent in ["149", "199", "25%", LINK, DISCLOSURE] {
            assert!(!requests[0].prompt.contains(absent), "{absent}");
        }

        let full_price = OfferFacts {
            price: Some(OfferPrice {
                before: None,
                ..facts().price.unwrap()
            }),
            ..facts()
        };
        let draft = f.posts.draft(&full_price, &channel).await.unwrap();
        let writer = FakeWriter::pitching(&[PITCH]);
        f.posts.write_pitch(&writer, &draft, None).await.unwrap();
        assert!(writer.requests()[0].prompt.contains("Com desconto: não"));
    });
}

#[test]
fn the_pitch_is_written_by_the_model_the_owner_chose_for_listing_copy() {
    block_on(async {
        let f = Fixture::new().await;
        f.copy
            .save_settings(CopySettings {
                model: "claude-opus-5-5".into(),
            })
            .await
            .unwrap();
        let channel = f.channel(ChannelKind::TelegramChannel).await;
        let draft = f.posts.draft(&facts(), &channel).await.unwrap();
        let writer = FakeWriter::pitching(&[PITCH]);

        f.posts.write_pitch(&writer, &draft, None).await.unwrap();

        assert_eq!(writer.requests()[0].model, "claude-opus-5-5");
    });
}

#[test]
fn a_pitch_with_a_price_a_percentage_a_link_or_a_contact_is_retried_then_fails() {
    let too_long = "a".repeat(PITCH_MAX_CHARS + 1);
    let cases: Vec<(&str, PitchProblem)> = vec![
        ("Só R$ 149,90 hoje", PitchProblem::Price),
        ("Leve por 149,90 e aprenda", PitchProblem::Price),
        ("Sai por 149 no Pix", PitchProblem::Price),
        ("Leve por 59 e economize", PitchProblem::Price),
        ("Hoje só 39!", PitchProblem::Price),
        ("Menos de 200 reais", PitchProblem::Price),
        ("Desconto de 25 no curso", PitchProblem::Price),
        ("Um quarto mais barato: 25% off", PitchProblem::Percentage),
        (
            "Metade do preço, cinquenta por cento",
            PitchProblem::Percentage,
        ),
        (
            "Veja em https://exemplo.com.br agora",
            PitchProblem::Contact(Contact::Link),
        ),
        (
            "Chama no zap 11 99876-5432",
            PitchProblem::Contact(Contact::Phone),
        ),
        ("Primeira linha\nSegunda linha", PitchProblem::SeveralLines),
        ("   ", PitchProblem::Empty),
        (
            too_long.as_str(),
            PitchProblem::TooLong {
                chars: PITCH_MAX_CHARS + 1,
                max: PITCH_MAX_CHARS,
            },
        ),
    ];
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::TelegramChannel).await;
        let draft = f.posts.draft(&facts(), &channel).await.unwrap();
        let saved = f.posts.create(&draft).await.unwrap();
        for (bad, problem) in cases {
            let writer = FakeWriter::pitching(&[bad]);

            let failed = f.posts.write_pitch(&writer, &saved.draft, None).await;

            assert!(
                matches!(&failed, Err(PitchError::Rejected(problems)) if problems.contains(&problem)),
                "{bad:?}: {failed:?}"
            );
            let requests = writer.requests();
            assert_eq!(requests.len(), 2, "{bad:?}");
            assert!(requests[1].prompt.starts_with(&requests[0].prompt));
            assert!(
                requests[1].prompt.ends_with(&format!(
                    "Uma versão anterior foi recusada porque {}. Escreva de novo, corrigindo \
                     isso.",
                    check_pitch(bad.trim(), &facts())
                        .iter()
                        .map(PitchProblem::describe)
                        .collect::<Vec<_>>()
                        .join("; ")
                )),
                "{}",
                requests[1].prompt
            );
            assert_eq!(f.posts.post(saved.id).await.unwrap().draft, saved.draft);
        }

        let writer = FakeWriter::pitching(&["Só R$ 149,90 hoje", PITCH]);
        let pitched = f
            .posts
            .write_pitch(&writer, &saved.draft, None)
            .await
            .unwrap();
        assert_eq!(pitched.pitch, PITCH);
        assert!(
            writer.requests()[1].prompt.ends_with(
                "porque a chamada tinha preço ou moeda. Escreva de novo, corrigindo isso."
            )
        );
    });
}

#[test]
fn a_new_pitch_takes_the_old_ones_place_and_keeps_the_owners_edits() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::TelegramChannel).await;
        let draft = f.posts.draft(&facts(), &channel).await.unwrap();
        let first = f
            .posts
            .write_pitch(&FakeWriter::pitching(&[PITCH]), &draft, None)
            .await
            .unwrap();
        let mut edited = first.draft.clone();
        edited.text = edited.text.replace("🔥", "📸");

        let second = f
            .posts
            .write_pitch(&FakeWriter::pitching(&[OTHER_PITCH]), &edited, Some(PITCH))
            .await
            .unwrap();

        assert_eq!(second.draft.text, edited.text.replace(PITCH, OTHER_PITCH));
        assert!(!second.edits_replaced);

        // The owner took the Pitch out by hand: the text is drafted again
        // from the template, and the edits go.
        let mut without = edited.clone();
        without.text = without_line(&edited.text, PITCH);
        let third = f
            .posts
            .write_pitch(&FakeWriter::pitching(&[OTHER_PITCH]), &without, Some(PITCH))
            .await
            .unwrap();
        assert!(third.edits_replaced);
        assert_eq!(
            third.draft.text,
            f.posts
                .pitched_draft(&facts(), &channel, Some(OTHER_PITCH))
                .await
                .unwrap()
                .text
        );
    });
}

#[test]
fn a_template_without_chamada_writes_nothing() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::TelegramChannel).await;
        f.posts
            .set_template(channel.id, "{produto} por {preco}\n{link}")
            .await
            .unwrap();
        let draft = f.posts.draft(&facts(), &channel).await.unwrap();
        let writer = FakeWriter::pitching(&[PITCH]);

        assert!(matches!(
            f.posts.write_pitch(&writer, &draft, None).await,
            Err(PitchError::NoPitchLine)
        ));
        assert!(writer.requests().is_empty());

        f.posts
            .set_template(channel.id, "{produto}: {chamada}\n{preco}\n{link}")
            .await
            .unwrap();
        let draft = f.posts.draft(&facts(), &channel).await.unwrap();
        assert_eq!(
            draft.text,
            format!("R$ 149,90\n{LINK}\n\n{DISCLOSURE}"),
            "a line whose Pitch is empty is left out"
        );
        let pitched = f.posts.write_pitch(&writer, &draft, None).await.unwrap();
        assert_eq!(
            pitched.draft.text,
            format!("Curso de Fotografia <Completo>: {PITCH}\nR$ 149,90\n{LINK}\n\n{DISCLOSURE}")
        );
    });
}

#[test]
fn a_writer_that_fails_or_answers_another_shape_leaves_the_post_alone() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::TelegramChannel).await;
        let draft = f.posts.draft(&facts(), &channel).await.unwrap();

        let writer = FakeWriter::answering(vec![Err(PlatformError::Expired)]);
        assert!(matches!(
            f.posts.write_pitch(&writer, &draft, None).await,
            Err(PitchError::Writer(PlatformError::Expired))
        ));
        assert_eq!(writer.requests().len(), 1);

        let writer = FakeWriter::answering(vec![Ok(Written::Listing(WrittenCopy {
            title: "Curso".into(),
            description: "Curso".into(),
        }))]);
        assert!(matches!(
            f.posts.write_pitch(&writer, &draft, None).await,
            Err(PitchError::Writer(PlatformError::Failed(_)))
        ));
    });
}

#[test]
fn an_honest_pitch_has_no_problem() {
    for pitch in [
        PITCH,
        "Fone com 30 horas de bateria e som que surpreende",
        "Bluetooth 5.3, leve e confortável para o dia todo",
        "Em oferta: o kit que faltava na sua cozinha 🍳",
    ] {
        assert_eq!(check_pitch(pitch, &facts()), [], "{pitch}");
    }
}

/// A template of any of the other variables and a few words, with
/// `{chamada}` on a line of its own and the link on the last line.
fn template() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        prop::sample::select(
            POST_VARIABLES
                .into_iter()
                .filter(|variable| *variable != "{chamada}")
                .collect::<Vec<_>>()
        )
        .prop_map(str::to_owned),
        Just("De {de}".to_owned()),
        Just("por ".to_owned()),
        Just("\n".to_owned()),
    ];
    (
        prop::collection::vec(piece.clone(), 0..8),
        prop::collection::vec(piece, 0..8),
    )
        .prop_map(|(before, after)| {
            format!(
                "{}\n{{chamada}}\n{}\n{{link}}",
                before.concat(),
                after.concat()
            )
        })
}

proptest! {
    #[test]
    fn a_pitch_never_changes_the_rest_of_the_post(
        template in template(),
        pitch in "[A-Za-zÀ-ú][A-Za-zÀ-ú !]{0,80}",
        price in prop::option::of(1i64..100_000),
        before in prop::option::of(1i64..100_000),
    ) {
        block_on(async {
            let f = Fixture::new().await;
            let channel = f.channel(ChannelKind::Other).await;
            f.posts.set_template(channel.id, &template).await.unwrap();
            let facts = OfferFacts {
                price: price.map(|price| OfferPrice {
                    price: brl(price),
                    before: before.map(brl),
                    seen_at: Utc.with_ymd_and_hms(2026, 10, 26, 11, 30, 0).unwrap(),
                }),
                ..facts()
            };
            let pitch = pitch.trim();
            prop_assume!(check_pitch(pitch, &facts).is_empty());
            let plain = f.posts.draft(&facts, &channel).await.unwrap();
            let pitched = f
                .posts
                .write_pitch(&FakeWriter::pitching(&[pitch]), &plain, None)
                .await
                .unwrap();
            let blank_runs_as_one = |text: &str| {
                let mut lines: Vec<&str> = Vec::new();
                for line in text.lines() {
                    if !(line.is_empty() && lines.last().is_none_or(|last| last.is_empty())) {
                        lines.push(line);
                    }
                }
                lines.join("\n").trim().to_owned()
            };
            prop_assert_eq!(
                blank_runs_as_one(&without_line(&pitched.draft.text, pitch)),
                blank_runs_as_one(&plain.text)
            );
            prop_assert_eq!(
                f.posts.check(&pitched.draft).await.unwrap(),
                f.posts.check(&plain).await.unwrap()
            );
            Ok(())
        })?;
    }

    #[test]
    fn any_price_or_discount_the_post_shows_is_refused_in_a_pitch(
        cents in 1i64..10_000_000,
        percent in 1u32..100,
    ) {
        let price = brl(cents).to_pt_br();
        let bare = price.trim_start_matches("R$").trim().to_owned();
        for pitch in [format!("Agora por {price}"), format!("Leve por {bare}")] {
            let problems = check_pitch(&pitch, &facts());
            prop_assert!(problems.contains(&PitchProblem::Price), "{}", pitch);
        }
        let pitch = format!("{percent}% mais barato");
        let problems = check_pitch(&pitch, &facts());
        prop_assert!(problems.contains(&PitchProblem::Percentage), "{}", pitch);
    }
}
