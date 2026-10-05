//! Questions (#24): what buyers ask on the owner's listings, read in each
//! Sync through a port a Platform's adapter implements (ADR 0013) and kept
//! by the channel's id of each question, the unanswered ones on top with
//! how long they wait. An answer reaches the channel only after the owner
//! confirms its final text, and the app keeps what it sent (ADR 0022).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use chrono::TimeDelta;
use libsql::{Connection, TransactionBehavior, Value, params};
use mascate_kernel::{Clock, IdGenerator, PlatformError, RecordId, Timestamp};
use mascate_platform::{
    Database, Migration, StoredRow, StoredValueError, load_single_row, save_single_row, stored,
};

use crate::replies::{ReplyProblem, check_answer};

pub(crate) const CREATE_QUESTIONS: Migration = Migration {
    version: 2,
    name: "create the questions and the reply templates",
    risky: false,
    sql: "CREATE TABLE marketing_questions (
        id             TEXT PRIMARY KEY,
        ml_question_id TEXT NOT NULL,
        ml_item_id     TEXT NOT NULL,
        text           TEXT NOT NULL,
        status         TEXT NOT NULL,
        asked_at       TEXT NOT NULL,
        answer_text    TEXT,
        answered_at    TEXT,
        sent_text      TEXT,
        sent_at        TEXT,
        created_at     TEXT NOT NULL,
        updated_at     TEXT NOT NULL,
        deleted_at     TEXT
    );
    CREATE UNIQUE INDEX marketing_questions_by_question
        ON marketing_questions (ml_question_id)
        WHERE deleted_at IS NULL;
    CREATE TABLE marketing_question_syncs (
        id         TEXT PRIMARY KEY,
        synced_at  TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE TABLE marketing_reply_templates (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        text       TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE TABLE marketing_question_settings (
        id            TEXT    PRIMARY KEY,
        dispatch_days INTEGER NOT NULL,
        created_at    TEXT    NOT NULL,
        updated_at    TEXT    NOT NULL,
        deleted_at    TEXT
    );",
};

const SYNC_TABLE: &str = "marketing_question_syncs";
const SYNC_COLUMNS: &[&str] = &["synced_at"];

/// Where a question stands on the channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuestionStatus {
    /// Waiting for the owner's answer: the only one that takes an answer.
    Unanswered,
    Answered,
    /// The listing closed before anyone answered.
    ClosedUnanswered,
    /// The channel is checking the question.
    UnderReview,
    /// Removed by the channel, its text with it.
    Banned,
    /// Deleted or disabled on the channel.
    Deleted,
}

impl QuestionStatus {
    const ALL: [QuestionStatus; 6] = [
        QuestionStatus::Unanswered,
        QuestionStatus::Answered,
        QuestionStatus::ClosedUnanswered,
        QuestionStatus::UnderReview,
        QuestionStatus::Banned,
        QuestionStatus::Deleted,
    ];

    fn code(self) -> &'static str {
        match self {
            QuestionStatus::Unanswered => "unanswered",
            QuestionStatus::Answered => "answered",
            QuestionStatus::ClosedUnanswered => "closed_unanswered",
            QuestionStatus::UnderReview => "under_review",
            QuestionStatus::Banned => "banned",
            QuestionStatus::Deleted => "deleted",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.code() == code)
    }

    /// Whether the channel may still change it on its own, so each Sync
    /// asks about it again.
    fn may_change(self) -> bool {
        matches!(
            self,
            QuestionStatus::Unanswered | QuestionStatus::UnderReview
        )
    }
}

/// An answer as the channel shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelAnswer {
    pub text: String,
    pub at: Timestamp,
}

/// A buyer's question as the channel reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelQuestion {
    /// The channel's id of the question: what keeps it from showing twice.
    pub id: String,
    /// The channel's id of the listing asked about.
    pub listing: String,
    /// Empty when the channel removed it.
    pub text: String,
    pub status: QuestionStatus,
    pub asked_at: Timestamp,
    pub answer: Option<ChannelAnswer>,
}

/// The questions on the owner's listings in a Sales Channel, as a
/// Platform's adapter reports them. Calls block on the network, so they run
/// off the UI thread.
pub trait ChannelQuestions: Send + Sync {
    /// Every question still waiting for the owner's answer.
    fn unanswered(&self) -> Result<Vec<ChannelQuestion>, PlatformError>;

    /// One question as it stands now; `None` once the channel no longer
    /// has it.
    fn question(&self, id: &str) -> Result<Option<ChannelQuestion>, PlatformError>;

    /// Posts `text` as the answer to the question `id`.
    fn answer(&self, id: &str, text: &str) -> Result<(), PlatformError>;
}

/// What the owner sent from the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentAnswer {
    pub text: String,
    pub at: Timestamp,
}

/// A question as the app keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub id: RecordId,
    /// As the channel last reported it.
    pub asked: ChannelQuestion,
    /// The answer the owner sent from the app, if it went from here.
    pub sent: Option<SentAnswer>,
}

impl Question {
    pub fn is_unanswered(&self) -> bool {
        self.asked.status == QuestionStatus::Unanswered
    }

    /// How long the buyer has been waiting by `now`; `None` once it no
    /// longer waits for the owner.
    pub fn waiting(&self, now: Timestamp) -> Option<TimeDelta> {
        self.is_unanswered()
            .then(|| (now - self.asked.asked_at).max(TimeDelta::zero()))
    }

    /// How long the buyer waited for the answer.
    pub fn waited(&self) -> Option<TimeDelta> {
        let answered_at = self.asked.answer.as_ref().map(|answer| answer.at)?;
        Some((answered_at - self.asked.asked_at).max(TimeDelta::zero()))
    }
}

/// What a Sync of questions did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuestionSync {
    /// Unanswered questions the app had not seen before, oldest first.
    pub arrived: Vec<ChannelQuestion>,
    /// Questions waiting for the owner after the Sync.
    pub unanswered: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum QuestionError {
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error(transparent)]
    Reply(#[from] ReplyProblem),
    #[error("the question is no longer waiting for an answer")]
    NotWaiting,
    #[error("the answer to this question is already being sent")]
    Sending,
    #[error("the channel no longer has the question")]
    Gone,
    #[error("no such question or template")]
    NotFound,
    #[error("the dispatch time goes from 1 to 30 business days")]
    InvalidSettings,
    #[error("the questions hold a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for QuestionError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => QuestionError::Unreadable(text),
            StoredValueError::Sql(error) => QuestionError::Sql(error),
        }
    }
}

/// An answer checked and ready, waiting for the owner to read its final
/// text and confirm. Only [`AnswerRequest::confirm`] makes it something
/// [`Questions::send`] takes; dropping it sends nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "an answer does nothing until confirmed and passed to Questions::send"]
pub struct AnswerRequest {
    question: RecordId,
    text: String,
}

impl AnswerRequest {
    /// The text that will reach the buyer, as it is.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The owner read the final text and confirmed it.
    pub fn confirm(self) -> ConfirmedAnswer {
        ConfirmedAnswer {
            question: self.question,
            text: self.text,
        }
    }
}

/// An answer the owner confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedAnswer {
    question: RecordId,
    text: String,
}

/// The questions on the owner's listings and the answers sent from the app.
pub struct Questions {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    /// The questions whose answer is on its way, so a second click never
    /// sends another.
    sending: Mutex<HashSet<RecordId>>,
}

impl Questions {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            database,
            clock,
            ids,
            sending: Mutex::new(HashSet::new()),
        }
    }

    pub fn now(&self) -> Timestamp {
        self.clock.now()
    }

    /// Reads the unanswered questions, and asks again about each one the
    /// app still had waiting or under review, which may have been answered
    /// on the channel, closed or removed. Each is kept by the channel's id:
    /// the same answers again change nothing. Nothing is kept unless every
    /// question was read.
    pub async fn sync(
        &self,
        channel: &dyn ChannelQuestions,
    ) -> Result<QuestionSync, QuestionError> {
        let mut read = Vec::new();
        let mut seen = HashSet::new();
        for question in channel.unanswered()? {
            if seen.insert(question.id.clone()) {
                read.push(question);
            }
        }
        let mut gone = Vec::new();
        for kept in read_questions(self.database.connection()).await? {
            if !kept.asked.status.may_change() || seen.contains(&kept.asked.id) {
                continue;
            }
            match channel.question(&kept.asked.id)? {
                Some(question) => read.push(question),
                None => gone.push(kept.id),
            }
        }

        let now = self.clock.now();
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let mut report = QuestionSync::default();
        for question in read {
            let new = write_question(&transaction, self.ids.as_ref(), &question, now).await?;
            if question.status == QuestionStatus::Unanswered {
                report.unanswered += 1;
                if new {
                    report.arrived.push(question);
                }
            }
        }
        for id in gone {
            transaction
                .execute(
                    "UPDATE marketing_questions SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                    params![stored(now), id.to_string()],
                )
                .await?;
        }
        save_single_row(
            &transaction,
            self.clock.as_ref(),
            self.ids.as_ref(),
            SYNC_TABLE,
            SYNC_COLUMNS,
            vec![Value::Text(stored(now))],
        )
        .await?;
        transaction.commit().await?;
        report.arrived.sort_by_key(|question| question.asked_at);
        Ok(report)
    }

    /// When the questions were last read; `None` before the first Sync.
    pub async fn last_sync(&self) -> Result<Option<Timestamp>, QuestionError> {
        match load_single_row(self.database.connection(), SYNC_TABLE, SYNC_COLUMNS).await? {
            Some(row) => Ok(Some(row.time_at(0)?)),
            None => Ok(None),
        }
    }

    /// Every question kept: the unanswered ones first, the longest waiting
    /// on top, then those under review, then the rest, newest first.
    pub async fn inbox(&self) -> Result<Vec<Question>, QuestionError> {
        let mut questions = read_questions(self.database.connection()).await?;
        questions.sort_by(|a, b| {
            let group = |question: &Question| match question.asked.status {
                QuestionStatus::Unanswered => 0,
                QuestionStatus::UnderReview => 1,
                _ => 2,
            };
            group(a).cmp(&group(b)).then_with(|| {
                if group(a) == 0 {
                    a.asked.asked_at.cmp(&b.asked.asked_at)
                } else {
                    b.asked.asked_at.cmp(&a.asked.asked_at)
                }
            })
        });
        Ok(questions)
    }

    /// Checks `text` as the answer to `question`, which must still wait for
    /// one: what comes back carries the final text for the owner to read
    /// and confirm.
    pub fn prepare(&self, question: &Question, text: &str) -> Result<AnswerRequest, QuestionError> {
        if !question.is_unanswered() || question.sent.is_some() {
            return Err(QuestionError::NotWaiting);
        }
        let text = text.trim();
        check_answer(text)?;
        Ok(AnswerRequest {
            question: question.id,
            text: text.to_owned(),
        })
    }

    /// Sends the answer the owner confirmed, once. The channel is asked
    /// first whether the question still waits: one answered on the channel
    /// meanwhile, or by an earlier send whose reply never arrived, is kept
    /// as the channel has it and nothing is sent. What the app sent is kept
    /// with the question.
    pub async fn send(
        &self,
        channel: &dyn ChannelQuestions,
        answer: ConfirmedAnswer,
    ) -> Result<Question, QuestionError> {
        let _sending = Sending::start(&self.sending, answer.question)?;
        let kept = self.question(answer.question).await?;
        if !kept.is_unanswered() || kept.sent.is_some() {
            return Err(QuestionError::NotWaiting);
        }
        let now = self.clock.now();
        match channel.question(&kept.asked.id)? {
            None => {
                self.database
                    .connection()
                    .execute(
                        "UPDATE marketing_questions SET deleted_at = ?1, updated_at = ?1
                         WHERE id = ?2",
                        params![stored(now), kept.id.to_string()],
                    )
                    .await?;
                return Err(QuestionError::Gone);
            }
            Some(current) if current.status != QuestionStatus::Unanswered => {
                write_question(self.database.connection(), self.ids.as_ref(), &current, now)
                    .await?;
                return Err(QuestionError::NotWaiting);
            }
            Some(_) => {}
        }
        channel.answer(&kept.asked.id, &answer.text)?;
        let now = self.clock.now();
        self.database
            .connection()
            .execute(
                "UPDATE marketing_questions
                 SET status = ?1, answer_text = ?2, answered_at = ?3, sent_text = ?2,
                     sent_at = ?3, updated_at = ?3
                 WHERE id = ?4",
                params![
                    QuestionStatus::Answered.code(),
                    answer.text.clone(),
                    stored(now),
                    kept.id.to_string()
                ],
            )
            .await?;
        self.question(kept.id).await
    }

    async fn question(&self, id: RecordId) -> Result<Question, QuestionError> {
        read_questions(self.database.connection())
            .await?
            .into_iter()
            .find(|question| question.id == id)
            .ok_or(QuestionError::NotFound)
    }
}

/// Marks a question as being answered for as long as it lives.
struct Sending<'a> {
    set: &'a Mutex<HashSet<RecordId>>,
    question: RecordId,
}

impl<'a> Sending<'a> {
    fn start(set: &'a Mutex<HashSet<RecordId>>, question: RecordId) -> Result<Self, QuestionError> {
        let mut sending = set.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !sending.insert(question) {
            return Err(QuestionError::Sending);
        }
        Ok(Self { set, question })
    }
}

impl Drop for Sending<'_> {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.question);
    }
}

/// Writes a question by the channel's id, keeping what the app sent; `true`
/// when it was not kept before.
async fn write_question(
    on: &Connection,
    ids: &dyn IdGenerator,
    question: &ChannelQuestion,
    now: Timestamp,
) -> Result<bool, QuestionError> {
    let mut rows = on
        .query(
            "SELECT 1 FROM marketing_questions WHERE ml_question_id = ?1 AND deleted_at IS NULL",
            params![question.id.clone()],
        )
        .await?;
    let new = rows.next().await?.is_none();
    drop(rows);
    let answer = question.answer.as_ref();
    on.execute(
        "INSERT INTO marketing_questions
             (id, ml_question_id, ml_item_id, text, status, asked_at, answer_text, answered_at,
              created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
         ON CONFLICT (ml_question_id) WHERE deleted_at IS NULL
         DO UPDATE SET ml_item_id = excluded.ml_item_id, text = excluded.text,
             status = excluded.status, asked_at = excluded.asked_at,
             answer_text = COALESCE(excluded.answer_text, marketing_questions.answer_text),
             answered_at = COALESCE(excluded.answered_at, marketing_questions.answered_at),
             updated_at = excluded.updated_at",
        params![
            ids.next_id().to_string(),
            question.id.clone(),
            question.listing.clone(),
            question.text.clone(),
            question.status.code(),
            stored(question.asked_at),
            answer.map(|answer| answer.text.clone()),
            answer.map(|answer| stored(answer.at)),
            stored(now)
        ],
    )
    .await?;
    Ok(new)
}

/// Every question kept, in no particular order.
async fn read_questions(on: &Connection) -> Result<Vec<Question>, QuestionError> {
    let mut rows = on
        .query(
            "SELECT id, ml_question_id, ml_item_id, text, status, asked_at, answer_text,
                    answered_at, sent_text, sent_at
             FROM marketing_questions WHERE deleted_at IS NULL",
            (),
        )
        .await?;
    let mut questions = Vec::new();
    while let Some(row) = rows.next().await? {
        let code = row.get::<String>(4)?;
        let status = QuestionStatus::from_code(&code).ok_or(QuestionError::Unreadable(code))?;
        let answer = match (row.get::<Option<String>>(6)?, row.optional_time_at(7)?) {
            (Some(text), Some(at)) => Some(ChannelAnswer { text, at }),
            _ => None,
        };
        let sent = match (row.get::<Option<String>>(8)?, row.optional_time_at(9)?) {
            (Some(text), Some(at)) => Some(SentAnswer { text, at }),
            _ => None,
        };
        questions.push(Question {
            id: row.id_at(0)?,
            asked: ChannelQuestion {
                id: row.get(1)?,
                listing: row.get(2)?,
                text: row.get(3)?,
                status,
                asked_at: row.time_at(5)?,
                answer,
            },
            sent,
        });
    }
    Ok(questions)
}
