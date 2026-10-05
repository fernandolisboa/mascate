//! Reply templates (#24): the owner's ready answers to common questions,
//! with the product's name and the dispatch time filled in, and the rule
//! that keeps contact details and links off Mercado Livre out of any answer
//! before it is sent. An answer always leaves on the owner's confirmation
//! (see `questions`).

use std::sync::Arc;

use libsql::{Value, params};
use mascate_kernel::{Clock, IdGenerator, Record, RecordId, mercado_livre_link};
use mascate_platform::{Database, StoredRow, load_single_row, save_single_row, stored};

use crate::QuestionError;

/// The longest answer Mercado Livre takes, in characters.
pub const ANSWER_MAX_CHARS: usize = 2000;

/// The longest name a template can have, in characters.
pub const TEMPLATE_NAME_MAX_CHARS: usize = 60;

/// Stands for the product's name in a template.
pub const PRODUCT_VARIABLE: &str = "{produto}";

/// Stands for the dispatch time in a template, as in "1 dia útil".
pub const DEADLINE_VARIABLE: &str = "{prazo}";

const VARIABLES: [&str; 2] = [PRODUCT_VARIABLE, DEADLINE_VARIABLE];

/// Business days the owner takes to dispatch, at most.
pub const MAX_DISPATCH_DAYS: u8 = 30;

const SETTINGS_TABLE: &str = "marketing_question_settings";
const SETTINGS_COLUMNS: &[&str] = &["dispatch_days"];

/// A kind of contact detail an answer cannot carry: Mercado Livre forbids
/// taking the buyer off the channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Contact {
    /// A link to a page off Mercado Livre.
    Link,
    Email,
    Phone,
    /// A messenger or social network, or an `@` handle on one.
    SocialNetwork,
}

impl Contact {
    /// As the owner reads it, after "A resposta tem".
    pub fn name(self) -> &'static str {
        match self {
            Contact::Link => "um link para fora do Mercado Livre",
            Contact::Email => "um e-mail",
            Contact::Phone => "um telefone",
            Contact::SocialNetwork => "uma rede social ou aplicativo de mensagens",
        }
    }
}

/// Why an answer or a template cannot be used as written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplyProblem {
    #[error("the text is empty")]
    Empty,
    #[error("the text has {chars} characters, above {ANSWER_MAX_CHARS}")]
    TooLong { chars: usize },
    #[error("the text carries contact details: {}", .0.name())]
    Contact(Contact),
    #[error("the template uses {0}, which the app does not fill")]
    UnknownVariable(String),
    #[error("the template has no name or a name above {TEMPLATE_NAME_MAX_CHARS} characters")]
    BadName,
    #[error("another template is already named {0}")]
    NameTaken(String),
}

/// The first contact detail in `text`, if any. Links that open Mercado
/// Livre over HTTPS are allowed; anything that looks like a link, an
/// e-mail, a phone number or a messenger is not.
pub fn contact_in(text: &str) -> Option<Contact> {
    if phone_in(text) {
        return Some(Contact::Phone);
    }
    text.split_whitespace().find_map(|token| {
        let token = token.trim_matches(|c: char| {
            matches!(
                c,
                '.' | ',' | ';' | ':' | '!' | '?' | '(' | ')' | '"' | '\'' | '“' | '”'
            )
        });
        contact_token(token)
    })
}

/// Networks and messengers by the words people write them with, folded.
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

/// Top-level domains that make a word such as `loja.com` a link.
const DOMAINS: [&str; 18] = [
    "com", "br", "net", "org", "io", "me", "ly", "app", "shop", "store", "site", "online", "info",
    "biz", "co", "link", "page", "xyz",
];

fn contact_token(token: &str) -> Option<Contact> {
    if token.is_empty() {
        return None;
    }
    let lower = token.to_lowercase();
    if lower.contains("://") || lower.starts_with("www.") {
        return (!mercado_livre_link(token)).then_some(Contact::Link);
    }
    if let Some((user, host)) = token.split_once('@') {
        if !user.is_empty() && host.contains('.') {
            return Some(Contact::Email);
        }
        if user.is_empty() && host.chars().filter(|c| c.is_alphanumeric()).count() >= 2 {
            return Some(Contact::SocialNetwork);
        }
    }
    if looks_like_domain(&lower) {
        return Some(Contact::Link);
    }
    let folded: String = token.chars().filter_map(mascate_kernel::fold).collect();
    NETWORKS
        .contains(&folded.as_str())
        .then_some(Contact::SocialNetwork)
}

/// `loja.com.br`, `bit.ly/x`: labels of letters, digits and hyphens joined
/// by dots, ending in a known domain. Prices (`1.299`) and sizes (`3.5mm`)
/// do not end in one.
fn looks_like_domain(lower: &str) -> bool {
    let host = lower.split(['/', '?', '#']).next().unwrap_or_default();
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
        && labels.last().is_some_and(|last| DOMAINS.contains(last))
}

/// Digits that read as a phone number: at least 8, in groups joined by
/// single spaces, dots, dashes, parentheses or a plus sign, with a group of
/// 4 or more, as in `(11) 98765-4321` or `+55 11 987654321`. Sizes such as
/// `38 39 40 41` have no long group.
fn phone_in(text: &str) -> bool {
    let mut digits = 0;
    let mut group = 0;
    let mut longest = 0;
    let mut separators = 0;
    for c in text.chars() {
        if c.is_ascii_digit() {
            digits += 1;
            group += 1;
            longest = longest.max(group);
            separators = 0;
            if digits >= 8 && longest >= 4 {
                return true;
            }
            continue;
        }
        group = 0;
        let joins = matches!(c, ' ' | '.' | '-' | '(' | ')' | '+');
        if joins && separators < 2 && (digits > 0 || c == '(' || c == '+') {
            separators += 1;
        } else {
            digits = 0;
            longest = 0;
            separators = 0;
        }
    }
    false
}

/// Whether `text` can go to the buyer as an answer: something written,
/// within Mercado Livre's limit and without contact details.
pub fn check_answer(text: &str) -> Result<(), ReplyProblem> {
    if text.trim().is_empty() {
        return Err(ReplyProblem::Empty);
    }
    let chars = text.chars().count();
    if chars > ANSWER_MAX_CHARS {
        return Err(ReplyProblem::TooLong { chars });
    }
    match contact_in(text) {
        Some(contact) => Err(ReplyProblem::Contact(contact)),
        None => Ok(()),
    }
}

/// How the owner answers questions about dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuestionSettings {
    /// Business days, 1 to [`MAX_DISPATCH_DAYS`], to dispatch after the
    /// payment: what [`DEADLINE_VARIABLE`] says.
    pub dispatch_days: u8,
}

impl Default for QuestionSettings {
    fn default() -> Self {
        Self { dispatch_days: 1 }
    }
}

impl QuestionSettings {
    fn is_valid(&self) -> bool {
        (1..=MAX_DISPATCH_DAYS).contains(&self.dispatch_days)
    }

    /// "1 dia útil", "2 dias úteis".
    pub fn deadline(&self) -> String {
        match self.dispatch_days {
            1 => "1 dia útil".into(),
            days => format!("{days} dias úteis"),
        }
    }
}

/// A ready answer the owner keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyTemplate {
    pub id: RecordId,
    pub name: String,
    /// The answer, with [`PRODUCT_VARIABLE`] and [`DEADLINE_VARIABLE`]
    /// where the product's name and the dispatch time go.
    pub text: String,
}

impl ReplyTemplate {
    /// The answer for a question about `product`, every variable filled.
    pub fn fill(&self, product: &str, settings: &QuestionSettings) -> String {
        self.text
            .replace(PRODUCT_VARIABLE, product.trim())
            .replace(DEADLINE_VARIABLE, &settings.deadline())
    }
}

/// A template as the owner writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewReplyTemplate {
    pub name: String,
    pub text: String,
}

impl NewReplyTemplate {
    /// The template trimmed, if it names itself, uses only the variables
    /// the app fills and would make an answer the channel takes.
    fn checked(&self) -> Result<NewReplyTemplate, ReplyProblem> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > TEMPLATE_NAME_MAX_CHARS {
            return Err(ReplyProblem::BadName);
        }
        let text = self.text.trim();
        if let Some(unknown) = unknown_variable(text) {
            return Err(ReplyProblem::UnknownVariable(unknown));
        }
        let mut bare = text.to_owned();
        for variable in VARIABLES {
            bare = bare.replace(variable, " ");
        }
        check_answer(&bare)?;
        if text.chars().count() > ANSWER_MAX_CHARS {
            return Err(ReplyProblem::TooLong {
                chars: text.chars().count(),
            });
        }
        Ok(NewReplyTemplate {
            name: name.to_owned(),
            text: text.to_owned(),
        })
    }
}

/// The first `{…}` in `text` that is not one of the variables.
fn unknown_variable(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let after = &rest[start..];
        let end = after.find('}').map_or(after.len(), |end| end + 1);
        let candidate = &after[..end];
        if !VARIABLES.contains(&candidate) {
            return Some(candidate.to_owned());
        }
        rest = &after[end..];
    }
    None
}

/// The owner's reply templates and how they fill the dispatch time.
pub struct ReplyTemplates {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl ReplyTemplates {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            database,
            clock,
            ids,
        }
    }

    /// Every template, by name.
    pub async fn templates(&self) -> Result<Vec<ReplyTemplate>, QuestionError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT id, name, text FROM marketing_reply_templates
                 WHERE deleted_at IS NULL ORDER BY name COLLATE NOCASE, created_at",
                (),
            )
            .await?;
        let mut templates = Vec::new();
        while let Some(row) = rows.next().await? {
            templates.push(ReplyTemplate {
                id: row.id_at(0)?,
                name: row.get(1)?,
                text: row.get(2)?,
            });
        }
        Ok(templates)
    }

    pub async fn create(&self, template: NewReplyTemplate) -> Result<ReplyTemplate, QuestionError> {
        let template = template.checked()?;
        self.ensure_free(&template.name, None).await?;
        let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
        self.database
            .connection()
            .execute(
                "INSERT INTO marketing_reply_templates (id, name, text, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![
                    record.id.to_string(),
                    template.name.clone(),
                    template.text.clone(),
                    stored(record.created_at)
                ],
            )
            .await?;
        Ok(ReplyTemplate {
            id: record.id,
            name: template.name,
            text: template.text,
        })
    }

    pub async fn update(
        &self,
        id: RecordId,
        template: NewReplyTemplate,
    ) -> Result<ReplyTemplate, QuestionError> {
        let template = template.checked()?;
        self.ensure_free(&template.name, Some(id)).await?;
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE marketing_reply_templates SET name = ?1, text = ?2, updated_at = ?3
                 WHERE id = ?4 AND deleted_at IS NULL",
                params![
                    template.name.clone(),
                    template.text.clone(),
                    stored(self.clock.now()),
                    id.to_string()
                ],
            )
            .await?;
        if changed == 0 {
            return Err(QuestionError::NotFound);
        }
        Ok(ReplyTemplate {
            id,
            name: template.name,
            text: template.text,
        })
    }

    pub async fn delete(&self, id: RecordId) -> Result<(), QuestionError> {
        let now = stored(self.clock.now());
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE marketing_reply_templates SET deleted_at = ?1, updated_at = ?1
                 WHERE id = ?2 AND deleted_at IS NULL",
                params![now, id.to_string()],
            )
            .await?;
        if changed == 0 {
            return Err(QuestionError::NotFound);
        }
        Ok(())
    }

    pub async fn settings(&self) -> Result<QuestionSettings, QuestionError> {
        let row =
            load_single_row(self.database.connection(), SETTINGS_TABLE, SETTINGS_COLUMNS).await?;
        let settings = match row {
            Some(row) => QuestionSettings {
                dispatch_days: u8::try_from(row.get::<u32>(0)?).unwrap_or(0),
            },
            None => QuestionSettings::default(),
        };
        Ok(if settings.is_valid() {
            settings
        } else {
            QuestionSettings::default()
        })
    }

    pub async fn save_settings(&self, settings: QuestionSettings) -> Result<(), QuestionError> {
        if !settings.is_valid() {
            return Err(QuestionError::InvalidSettings);
        }
        save_single_row(
            self.database.connection(),
            self.clock.as_ref(),
            self.ids.as_ref(),
            SETTINGS_TABLE,
            SETTINGS_COLUMNS,
            vec![Value::Integer(i64::from(settings.dispatch_days))],
        )
        .await?;
        Ok(())
    }

    /// Fails when another live template than `except` is named `name`,
    /// case aside.
    async fn ensure_free(&self, name: &str, except: Option<RecordId>) -> Result<(), QuestionError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT id FROM marketing_reply_templates
                 WHERE deleted_at IS NULL AND name = ?1 COLLATE NOCASE",
                params![name],
            )
            .await?;
        while let Some(row) = rows.next().await? {
            if Some(row.id_at(0)?) != except {
                return Err(ReplyProblem::NameTaken(name.to_owned()).into());
            }
        }
        Ok(())
    }
}
