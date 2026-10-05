//! Buyers' questions for marketing's port (ADR 0022): the unanswered ones
//! from `/questions/search` by seller, one question from `/questions/{id}`,
//! both with `api_version=4` as the documentation requires, and the owner's
//! confirmed answer through `POST /answers`.

use mascate_kernel::PlatformError;
use mascate_marketing::{ChannelAnswer, ChannelQuestion, ChannelQuestions, QuestionStatus};
use serde_json::json;

use super::MercadoLivre;
use super::answers::{QuestionAnswer, QuestionSearch, UserAnswer, time, whole_id};
use super::sales_channel::path_id;

/// Questions per page of `/questions/search`.
const SEARCH_PAGE: u32 = 50;

impl ChannelQuestions for MercadoLivre {
    /// Pages through the seller's unanswered questions, oldest first.
    fn unanswered(&self) -> Result<Vec<ChannelQuestion>, PlatformError> {
        let user: UserAnswer = self.get("/users/me", &[])?;
        let seller = user.id.to_string();
        let mut questions = Vec::new();
        let mut offset: u32 = 0;
        loop {
            let page: QuestionSearch = self.get(
                "/questions/search",
                &[
                    ("seller_id", &seller),
                    ("status", "UNANSWERED"),
                    ("api_version", "4"),
                    ("sort_fields", "date_created"),
                    ("sort_types", "ASC"),
                    ("offset", &offset.to_string()),
                    ("limit", &SEARCH_PAGE.to_string()),
                ],
            )?;
            if page.questions.is_empty() {
                break;
            }
            offset += u32::try_from(page.questions.len()).unwrap_or(SEARCH_PAGE);
            for answer in page.questions {
                questions.push(channel_question(answer)?);
            }
            if offset >= page.total {
                break;
            }
        }
        Ok(questions)
    }

    /// Mercado Livre answers 404 for a question it no longer has.
    fn question(&self, id: &str) -> Result<Option<ChannelQuestion>, PlatformError> {
        let path = format!("/questions/{}", path_id(id)?);
        match self.get::<QuestionAnswer>(&path, &[("api_version", "4")]) {
            Ok(answer) => channel_question(answer).map(Some),
            Err(PlatformError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The answer Mercado Livre sends back is not read: the next Sync reads
    /// the question as it stands.
    fn answer(&self, id: &str, text: &str) -> Result<(), PlatformError> {
        let question: u64 = path_id(id)?.parse().map_err(|_| PlatformError::NotFound)?;
        let body = json!({ "question_id": question, "text": text }).to_string();
        self.signed("/answers", |bearer| self.posting("/answers", bearer, &body))?;
        Ok(())
    }
}

fn channel_question(answer: QuestionAnswer) -> Result<ChannelQuestion, PlatformError> {
    let id = whole_id(&answer.id);
    let asked_at = time(&answer.date_created).ok_or_else(|| {
        PlatformError::Failed(format!("Mercado Livre sent the question {id} undated"))
    })?;
    let reply = answer.answer.and_then(|reply| {
        Some(ChannelAnswer {
            at: time(reply.date_created.as_deref()?)?,
            text: reply.text.unwrap_or_default(),
        })
    });
    Ok(ChannelQuestion {
        id,
        listing: answer.item_id,
        text: answer.text.unwrap_or_default(),
        status: status(&answer.status),
        asked_at,
        answer: reply,
    })
}

fn status(code: &str) -> QuestionStatus {
    match code.to_ascii_uppercase().as_str() {
        "UNANSWERED" => QuestionStatus::Unanswered,
        "ANSWERED" => QuestionStatus::Answered,
        "CLOSED_UNANSWERED" => QuestionStatus::ClosedUnanswered,
        "BANNED" => QuestionStatus::Banned,
        "DELETED" | "DISABLED" => QuestionStatus::Deleted,
        // `UNDER_REVIEW` and any new one: not answerable, asked about again
        // in each Sync until it settles.
        _ => QuestionStatus::UnderReview,
    }
}
