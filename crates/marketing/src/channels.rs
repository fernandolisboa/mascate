//! Promotion Channels (#99, ADR 0031 decision 5): where the owner promotes
//! affiliate links and products, each with the marking its links carry.
//! The Affiliate keeps a channel only by its id; the app translates a
//! channel into the Affiliate's words when a link is created.

use std::sync::Arc;

use libsql::params;
use mascate_kernel::{Clock, IdGenerator, Record, RecordId, folded_words};
use mascate_platform::{Database, Migration, StoredRow, StoredValueError, stored};

pub(crate) const CREATE_PROMOTION_CHANNELS: Migration = Migration {
    version: 6,
    name: "create the Promotion Channels",
    risky: false,
    sql: "CREATE TABLE marketing_promotion_channels (
        id            TEXT PRIMARY KEY,
        name          TEXT NOT NULL,
        kind          TEXT NOT NULL,
        default_tag   TEXT NOT NULL,
        keywords      TEXT NOT NULL,
        telegram_chat TEXT,
        created_at    TEXT NOT NULL,
        updated_at    TEXT NOT NULL,
        deleted_at    TEXT
    );
    CREATE UNIQUE INDEX marketing_promotion_channels_by_tag
        ON marketing_promotion_channels (default_tag)
        WHERE deleted_at IS NULL;",
};

/// The longest name a channel can have, in characters.
pub const CHANNEL_NAME_MAX_CHARS: usize = 60;

/// The longest marking a channel can have.
pub const CHANNEL_TAG_MAX_CHARS: usize = 20;

/// What a Promotion Channel is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelKind {
    TelegramChannel,
    WhatsAppGroup,
    Instagram,
    TikTok,
    /// Ads on Google, Bing and other search engines.
    PaidSearch,
    Other,
}

impl ChannelKind {
    pub const ALL: [ChannelKind; 6] = [
        ChannelKind::TelegramChannel,
        ChannelKind::WhatsAppGroup,
        ChannelKind::Instagram,
        ChannelKind::TikTok,
        ChannelKind::PaidSearch,
        ChannelKind::Other,
    ];

    fn code(self) -> &'static str {
        match self {
            ChannelKind::TelegramChannel => "telegram_channel",
            ChannelKind::WhatsAppGroup => "whatsapp_group",
            ChannelKind::Instagram => "instagram",
            ChannelKind::TikTok => "tiktok",
            ChannelKind::PaidSearch => "paid_search",
            ChannelKind::Other => "other",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.code() == code)
    }

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            ChannelKind::TelegramChannel => "Canal do Telegram",
            ChannelKind::WhatsAppGroup => "Grupo de WhatsApp",
            ChannelKind::Instagram => "Instagram",
            ChannelKind::TikTok => "TikTok",
            ChannelKind::PaidSearch => "Busca paga",
            ChannelKind::Other => "Outro",
        }
    }
}

/// A place where the owner promotes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionChannel {
    pub id: RecordId,
    pub name: String,
    pub kind: ChannelKind,
    /// The marking its links carry: lowercase letters, digits and `_`.
    pub default_tag: String,
    /// The keywords paid search bids on; empty on any other kind.
    pub keywords: Vec<String>,
    /// The Telegram chat, `@name` or a `-100…` id; only on a Telegram
    /// channel.
    pub telegram_chat: Option<String>,
}

/// A channel as the owner writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPromotionChannel {
    pub name: String,
    pub kind: ChannelKind,
    pub default_tag: String,
    pub keywords: Vec<String>,
    pub telegram_chat: Option<String>,
}

impl NewPromotionChannel {
    /// The channel trimmed, keeping only what its kind uses, if every
    /// field has a form the app and the Platforms take.
    fn checked(&self) -> Result<NewPromotionChannel, ChannelError> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > CHANNEL_NAME_MAX_CHARS {
            return Err(ChannelError::BadName);
        }
        let default_tag = self.default_tag.trim();
        if !is_channel_tag(default_tag) {
            return Err(ChannelError::BadTag);
        }
        let keywords = if self.kind == ChannelKind::PaidSearch {
            tidy_keywords(&self.keywords)
        } else {
            Vec::new()
        };
        let telegram_chat = match (self.kind, self.telegram_chat.as_deref().map(str::trim)) {
            (ChannelKind::TelegramChannel, Some(chat)) if !chat.is_empty() => {
                if !is_telegram_chat(chat) {
                    return Err(ChannelError::BadChat);
                }
                Some(chat.to_owned())
            }
            _ => None,
        };
        Ok(NewPromotionChannel {
            name: name.to_owned(),
            kind: self.kind,
            default_tag: default_tag.to_owned(),
            keywords,
            telegram_chat,
        })
    }
}

/// A marking made from a channel's name: lowercase, accents off, every
/// run of other characters as one `_`, cut at the longest marking.
pub fn suggested_tag(name: &str) -> String {
    let words = folded_words(name).join("_").to_lowercase();
    words
        .chars()
        .take(CHANNEL_TAG_MAX_CHARS)
        .collect::<String>()
        .trim_end_matches('_')
        .to_owned()
}

/// Lowercase letters, digits and `_`, up to 20: every Platform carries it
/// in a link as it is.
fn is_channel_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.chars().count() <= CHANNEL_TAG_MAX_CHARS
        && tag
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A public chat's `@username` (5 to 32 letters, digits or `_`, starting
/// with a letter) or a private channel's `-100…` id, as the Bot API takes
/// them.
fn is_telegram_chat(chat: &str) -> bool {
    if let Some(username) = chat.strip_prefix('@') {
        let length = username.chars().count();
        return (5..=32).contains(&length)
            && username.starts_with(|c: char| c.is_ascii_alphabetic())
            && username
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
    }
    chat.strip_prefix("-100").is_some_and(|rest| {
        !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) && chat.parse::<i64>().is_ok()
    })
}

/// Keywords trimmed, inner spaces collapsed, blank and repeated ones (case
/// and accents aside) dropped.
fn tidy_keywords(keywords: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for keyword in keywords {
        let keyword = keyword.split_whitespace().collect::<Vec<_>>().join(" ");
        let folded = folded_words(&keyword);
        if !folded.is_empty() && !kept.iter().any(|seen| folded_words(seen) == folded) {
            kept.push(keyword);
        }
    }
    kept
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("a channel needs a name of up to {CHANNEL_NAME_MAX_CHARS} characters")]
    BadName,
    #[error(
        "a marking is lowercase letters, digits and _, up to {CHANNEL_TAG_MAX_CHARS} characters"
    )]
    BadTag,
    #[error("another channel already uses the marking {0}")]
    TagTaken(String),
    #[error("a Telegram chat is @name or a -100… id")]
    BadChat,
    #[error("no such Promotion Channel")]
    NotFound,
    #[error("the channels hold a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for ChannelError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => ChannelError::Unreadable(text),
            StoredValueError::Sql(error) => ChannelError::Sql(error),
        }
    }
}

const CHANNEL_QUERY: &str = "SELECT id, name, kind, default_tag, keywords, telegram_chat
     FROM marketing_promotion_channels WHERE deleted_at IS NULL";

/// The owner's Promotion Channels.
pub struct PromotionChannels {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl PromotionChannels {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            database,
            clock,
            ids,
        }
    }

    /// Every channel, by name.
    pub async fn channels(&self) -> Result<Vec<PromotionChannel>, ChannelError> {
        let mut rows = self
            .database
            .connection()
            .query(
                &format!("{CHANNEL_QUERY} ORDER BY name COLLATE NOCASE, created_at"),
                (),
            )
            .await?;
        let mut channels = Vec::new();
        while let Some(row) = rows.next().await? {
            channels.push(channel_from(&row)?);
        }
        Ok(channels)
    }

    pub async fn channel(&self, id: RecordId) -> Result<PromotionChannel, ChannelError> {
        let mut rows = self
            .database
            .connection()
            .query(
                &format!("{CHANNEL_QUERY} AND id = ?1"),
                params![id.to_string()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => channel_from(&row),
            None => Err(ChannelError::NotFound),
        }
    }

    pub async fn create(
        &self,
        channel: &NewPromotionChannel,
    ) -> Result<PromotionChannel, ChannelError> {
        let channel = channel.checked()?;
        self.ensure_free(&channel.default_tag, None).await?;
        let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
        self.database
            .connection()
            .execute(
                "INSERT INTO marketing_promotion_channels
                     (id, name, kind, default_tag, keywords, telegram_chat, created_at,
                      updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![
                    record.id.to_string(),
                    channel.name.clone(),
                    channel.kind.code(),
                    channel.default_tag.clone(),
                    channel.keywords.join("\n"),
                    channel.telegram_chat.clone(),
                    stored(record.created_at)
                ],
            )
            .await?;
        Ok(kept(record.id, channel))
    }

    /// Changes the channel; the links already made keep the marking they
    /// were made with.
    pub async fn update(
        &self,
        id: RecordId,
        channel: &NewPromotionChannel,
    ) -> Result<PromotionChannel, ChannelError> {
        let channel = channel.checked()?;
        self.ensure_free(&channel.default_tag, Some(id)).await?;
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE marketing_promotion_channels
                 SET name = ?1, kind = ?2, default_tag = ?3, keywords = ?4, telegram_chat = ?5,
                     updated_at = ?6
                 WHERE id = ?7 AND deleted_at IS NULL",
                params![
                    channel.name.clone(),
                    channel.kind.code(),
                    channel.default_tag.clone(),
                    channel.keywords.join("\n"),
                    channel.telegram_chat.clone(),
                    stored(self.clock.now()),
                    id.to_string()
                ],
            )
            .await?;
        if changed == 0 {
            return Err(ChannelError::NotFound);
        }
        Ok(kept(id, channel))
    }

    pub async fn delete(&self, id: RecordId) -> Result<(), ChannelError> {
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE marketing_promotion_channels SET deleted_at = ?1, updated_at = ?1
                 WHERE id = ?2 AND deleted_at IS NULL",
                params![stored(self.clock.now()), id.to_string()],
            )
            .await?;
        if changed == 0 {
            return Err(ChannelError::NotFound);
        }
        Ok(())
    }

    /// Fails when another live channel than `except` uses `tag`.
    async fn ensure_free(&self, tag: &str, except: Option<RecordId>) -> Result<(), ChannelError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT id FROM marketing_promotion_channels
                 WHERE deleted_at IS NULL AND default_tag = ?1",
                params![tag],
            )
            .await?;
        while let Some(row) = rows.next().await? {
            if Some(row.id_at(0)?) != except {
                return Err(ChannelError::TagTaken(tag.to_owned()));
            }
        }
        Ok(())
    }
}

fn kept(id: RecordId, channel: NewPromotionChannel) -> PromotionChannel {
    PromotionChannel {
        id,
        name: channel.name,
        kind: channel.kind,
        default_tag: channel.default_tag,
        keywords: channel.keywords,
        telegram_chat: channel.telegram_chat,
    }
}

fn channel_from(row: &libsql::Row) -> Result<PromotionChannel, ChannelError> {
    let kind: String = row.get(2)?;
    let keywords: String = row.get(4)?;
    Ok(PromotionChannel {
        id: row.id_at(0)?,
        name: row.get(1)?,
        kind: ChannelKind::from_code(&kind).ok_or(ChannelError::Unreadable(kind))?,
        default_tag: row.get(3)?,
        keywords: keywords
            .lines()
            .filter(|keyword| !keyword.is_empty())
            .map(str::to_owned)
            .collect(),
        telegram_chat: row.get(5)?,
    })
}
