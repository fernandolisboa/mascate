//! Questions through marketing's public interface, with a real temporary
//! database and an in-memory channel: each question is kept once by the
//! channel's id with its status and how long it waits, the unanswered ones
//! on top, and an answer reaches the channel only once the owner confirms
//! it, once.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, PlatformError, Timestamp};
use mascate_marketing::{
    ChannelAnswer, ChannelQuestion, ChannelQuestions, Contact, MIGRATIONS, Question, QuestionError,
    QuestionStatus, QuestionSync, Questions, ReplyProblem, SentAnswer,
};
use mascate_platform::{Database, migrate};

/// A channel answering from memory, and what it was asked to post.
#[derive(Default)]
struct Channel {
    questions: Mutex<BTreeMap<String, ChannelQuestion>>,
    posted: Mutex<Vec<(String, String)>>,
    /// Refuses every answer while set.
    refusing: Mutex<Option<PlatformError>>,
}

impl Channel {
    fn asks(&self, question: ChannelQuestion) {
        self.questions
            .lock()
            .unwrap()
            .insert(question.id.clone(), question);
    }

    fn answered_on_the_site(&self, id: &str, text: &str, at: Timestamp) {
        let mut questions = self.questions.lock().unwrap();
        let question = questions.get_mut(id).unwrap();
        question.status = QuestionStatus::Answered;
        question.answer = Some(ChannelAnswer {
            text: text.into(),
            at,
        });
    }

    fn removes(&self, id: &str) {
        self.questions.lock().unwrap().remove(id);
    }

    fn posted(&self) -> Vec<(String, String)> {
        self.posted.lock().unwrap().clone()
    }
}

impl ChannelQuestions for Channel {
    fn unanswered(&self) -> Result<Vec<ChannelQuestion>, PlatformError> {
        Ok(self
            .questions
            .lock()
            .unwrap()
            .values()
            .filter(|question| question.status == QuestionStatus::Unanswered)
            .cloned()
            .collect())
    }

    fn question(&self, id: &str) -> Result<Option<ChannelQuestion>, PlatformError> {
        Ok(self.questions.lock().unwrap().get(id).cloned())
    }

    fn answer(&self, id: &str, text: &str) -> Result<(), PlatformError> {
        if let Some(error) = self.refusing.lock().unwrap().clone() {
            return Err(error);
        }
        self.posted.lock().unwrap().push((id.into(), text.into()));
        Ok(())
    }
}

fn noon() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap()
}

fn asked(id: &str, listing: &str, text: &str, minutes_ago: i64) -> ChannelQuestion {
    ChannelQuestion {
        id: id.into(),
        listing: listing.into(),
        text: text.into(),
        status: QuestionStatus::Unanswered,
        asked_at: noon() - TimeDelta::minutes(minutes_ago),
        answer: None,
    }
}

fn ids_of(questions: &[Question]) -> Vec<&str> {
    questions
        .iter()
        .map(|question| question.asked.id.as_str())
        .collect()
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    questions: Questions,
    channel: Channel,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(noon()));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        let questions = Questions::new(database, clock.clone(), Arc::new(SequentialIds::default()));
        Self {
            _dir: dir,
            clock,
            questions,
            channel: Channel::default(),
        }
    }

    async fn sync(&self) -> QuestionSync {
        self.questions.sync(&self.channel).await.unwrap()
    }

    async fn inbox(&self) -> Vec<Question> {
        self.questions.inbox().await.unwrap()
    }

    async fn find(&self, id: &str) -> Question {
        self.inbox()
            .await
            .into_iter()
            .find(|question| question.asked.id == id)
            .unwrap()
    }

    async fn answer(&self, id: &str, text: &str) -> Result<Question, QuestionError> {
        let question = self.find(id).await;
        let request = self.questions.prepare(&question, text)?;
        self.questions.send(&self.channel, request.confirm()).await
    }
}

#[test]
fn the_sync_keeps_each_question_with_its_status_and_how_long_it_waits() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .asks(asked("1001", "MLB1", "Tem na cor preta?", 90));

        let report = fixture.sync().await;

        assert_eq!(report.unanswered, 1);
        assert_eq!(
            report.arrived,
            [asked("1001", "MLB1", "Tem na cor preta?", 90)]
        );
        let inbox = fixture.inbox().await;
        assert_eq!(inbox.len(), 1);
        assert_eq!(
            inbox[0].asked,
            asked("1001", "MLB1", "Tem na cor preta?", 90)
        );
        assert_eq!(inbox[0].waiting(noon()), Some(TimeDelta::minutes(90)));
        assert_eq!(inbox[0].sent, None);
        assert_eq!(
            fixture.questions.last_sync().await.unwrap(),
            Some(fixture.clock.now())
        );
    });
}

#[test]
fn a_question_arrives_once_however_many_syncs_see_it() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .asks(asked("1001", "MLB1", "Tem na cor preta?", 90));
        fixture.sync().await;

        fixture.clock.advance(TimeDelta::minutes(5));
        let again = fixture.sync().await;

        assert_eq!(
            again,
            QuestionSync {
                arrived: Vec::new(),
                unanswered: 1,
            }
        );
        assert_eq!(fixture.inbox().await.len(), 1);
    });
}

#[test]
fn the_last_sync_counts_even_without_questions() {
    block_on(async {
        let fixture = Fixture::new().await;
        assert_eq!(fixture.questions.last_sync().await.unwrap(), None);

        assert_eq!(fixture.sync().await, QuestionSync::default());

        assert_eq!(fixture.questions.last_sync().await.unwrap(), Some(noon()));
    });
}

#[test]
fn the_unanswered_come_first_the_longest_waiting_on_top_then_the_rest_newest_first() {
    block_on(async {
        let fixture = Fixture::new().await;
        for question in [
            asked("1", "MLB1", "Pergunta antiga respondida", 600),
            asked("2", "MLB1", "Espera há pouco", 10),
            asked("3", "MLB2", "Espera há muito", 300),
            asked("4", "MLB2", "Respondida há pouco", 100),
        ] {
            fixture.channel.asks(question);
        }
        fixture.sync().await;
        fixture
            .channel
            .answered_on_the_site("1", "Sim", noon() - TimeDelta::minutes(500));
        fixture
            .channel
            .answered_on_the_site("4", "Não", noon() - TimeDelta::minutes(50));

        fixture.sync().await;

        let inbox = fixture.inbox().await;
        assert_eq!(ids_of(&inbox), ["3", "2", "4", "1"]);
        assert_eq!(inbox[2].waiting(noon()), None);
        assert_eq!(inbox[2].waited(), Some(TimeDelta::minutes(50)));
    });
}

#[test]
fn a_question_answered_on_the_channel_is_read_again_and_one_removed_leaves() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .asks(asked("1", "MLB1", "Aceita troca?", 60));
        fixture.channel.asks(asked("2", "MLB1", "Spam", 30));
        fixture.sync().await;
        fixture
            .channel
            .answered_on_the_site("1", "Aceitamos em 7 dias.", noon());
        fixture.channel.removes("2");

        let report = fixture.sync().await;

        assert_eq!(report.unanswered, 0);
        let inbox = fixture.inbox().await;
        assert_eq!(ids_of(&inbox), ["1"]);
        assert_eq!(inbox[0].asked.status, QuestionStatus::Answered);
        assert_eq!(
            inbox[0].asked.answer,
            Some(ChannelAnswer {
                text: "Aceitamos em 7 dias.".into(),
                at: noon(),
            })
        );
    });
}

#[test]
fn an_answer_goes_only_after_the_owner_confirms_and_is_kept_with_the_question() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .asks(asked("1001", "MLB1", "Tem na cor preta?", 90));
        fixture.sync().await;
        let question = fixture.find("1001").await;

        let request = fixture
            .questions
            .prepare(&question, "  Olá! Temos sim, na cor preta.  ")
            .unwrap();
        assert_eq!(request.text(), "Olá! Temos sim, na cor preta.");
        drop(request);
        assert!(fixture.channel.posted().is_empty());

        fixture.clock.advance(TimeDelta::minutes(3));
        let request = fixture
            .questions
            .prepare(&question, "Olá! Temos sim, na cor preta.")
            .unwrap();
        let answered = fixture
            .questions
            .send(&fixture.channel, request.confirm())
            .await
            .unwrap();

        assert_eq!(
            fixture.channel.posted(),
            [(
                "1001".to_owned(),
                "Olá! Temos sim, na cor preta.".to_owned()
            )]
        );
        let sent_at = noon() + TimeDelta::minutes(3);
        assert_eq!(answered.asked.status, QuestionStatus::Answered);
        assert_eq!(
            answered.sent,
            Some(SentAnswer {
                text: "Olá! Temos sim, na cor preta.".into(),
                at: sent_at,
            })
        );
        assert_eq!(answered.waited(), Some(TimeDelta::minutes(93)));
        assert_eq!(fixture.find("1001").await, answered);
    });
}

#[test]
fn a_question_is_answered_once() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .asks(asked("1001", "MLB1", "Tem na cor preta?", 90));
        fixture.sync().await;
        let waiting = fixture.find("1001").await;
        let first = fixture.questions.prepare(&waiting, "Temos.").unwrap();
        let second = fixture.questions.prepare(&waiting, "Temos sim.").unwrap();

        fixture
            .questions
            .send(&fixture.channel, first.confirm())
            .await
            .unwrap();
        let again = fixture
            .questions
            .send(&fixture.channel, second.confirm())
            .await;

        assert!(matches!(again, Err(QuestionError::NotWaiting)), "{again:?}");
        assert_eq!(fixture.channel.posted().len(), 1);
        let answered = fixture.find("1001").await;
        assert!(matches!(
            fixture.questions.prepare(&answered, "De novo"),
            Err(QuestionError::NotWaiting)
        ));
    });
}

#[test]
fn a_question_answered_on_the_channel_meanwhile_is_not_answered_again() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .asks(asked("1001", "MLB1", "Tem na cor preta?", 90));
        fixture.sync().await;
        let request = fixture
            .questions
            .prepare(&fixture.find("1001").await, "Temos.")
            .unwrap();
        fixture
            .channel
            .answered_on_the_site("1001", "Temos, sim!", noon());

        let sent = fixture
            .questions
            .send(&fixture.channel, request.confirm())
            .await;

        assert!(matches!(sent, Err(QuestionError::NotWaiting)), "{sent:?}");
        assert!(fixture.channel.posted().is_empty());
        let kept = fixture.find("1001").await;
        assert_eq!(kept.asked.status, QuestionStatus::Answered);
        assert_eq!(kept.asked.answer.unwrap().text, "Temos, sim!");
        assert_eq!(kept.sent, None);
    });
}

#[test]
fn a_question_the_channel_removed_meanwhile_leaves_without_an_answer() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .asks(asked("1001", "MLB1", "Tem na cor preta?", 90));
        fixture.sync().await;
        let request = fixture
            .questions
            .prepare(&fixture.find("1001").await, "Temos.")
            .unwrap();
        fixture.channel.removes("1001");

        let sent = fixture
            .questions
            .send(&fixture.channel, request.confirm())
            .await;

        assert!(matches!(sent, Err(QuestionError::Gone)), "{sent:?}");
        assert!(fixture.inbox().await.is_empty());
    });
}

#[test]
fn a_refused_answer_keeps_the_question_waiting() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .asks(asked("1001", "MLB1", "Tem na cor preta?", 90));
        fixture.sync().await;
        *fixture.channel.refusing.lock().unwrap() = Some(PlatformError::RateLimited);
        let question = fixture.find("1001").await;
        let request = fixture.questions.prepare(&question, "Temos.").unwrap();

        let sent = fixture
            .questions
            .send(&fixture.channel, request.confirm())
            .await;

        assert!(
            matches!(
                sent,
                Err(QuestionError::Platform(PlatformError::RateLimited))
            ),
            "{sent:?}"
        );
        assert_eq!(fixture.find("1001").await, question);
        *fixture.channel.refusing.lock().unwrap() = None;
        assert!(fixture.answer("1001", "Temos.").await.is_ok());
    });
}

#[test]
fn an_answer_with_contact_details_or_too_long_never_reaches_the_channel() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .asks(asked("1001", "MLB1", "Tem loja física?", 90));
        fixture.sync().await;

        for (text, problem) in [
            (
                "Chama no (11) 98765-4321",
                ReplyProblem::Contact(Contact::Phone),
            ),
            (
                "Veja em www.minhaloja.com.br",
                ReplyProblem::Contact(Contact::Link),
            ),
            (
                "Manda para loja@exemplo.com",
                ReplyProblem::Contact(Contact::Email),
            ),
            ("   ", ReplyProblem::Empty),
        ] {
            let answered = fixture.answer("1001", text).await;
            assert!(
                matches!(&answered, Err(QuestionError::Reply(found)) if *found == problem),
                "{text}: {answered:?}"
            );
        }
        let long = "a".repeat(2001);
        assert!(matches!(
            fixture.answer("1001", &long).await,
            Err(QuestionError::Reply(ReplyProblem::TooLong { chars: 2001 }))
        ));
        assert!(fixture.channel.posted().is_empty());
        assert!(fixture.find("1001").await.is_unanswered());
    });
}

#[test]
fn a_link_to_mercado_livre_may_go_in_an_answer() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .asks(asked("1001", "MLB1", "Tem a versão azul?", 90));
        fixture.sync().await;

        let answered = fixture
            .answer(
                "1001",
                "Temos neste anúncio: https://produto.mercadolivre.com.br/MLB-2",
            )
            .await;

        assert!(answered.is_ok(), "{answered:?}");
    });
}
