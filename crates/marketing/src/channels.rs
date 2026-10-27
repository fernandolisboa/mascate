//! Promotion Channels (#99, ADR 0031 decision 5): where the owner promotes
//! affiliate links and products, each with the marking its links carry.
//! The Affiliate keeps a channel only by its id; the app translates a
//! channel into the Affiliate's words when a link is created. A channel the
//! app posts to also has the times its scheduled posts go out at (#104,
//! #157); a Pinterest channel, the board its Pins go to, picked from the
//! boards Pinterest lists (#155, ADR 0035 decision 7).

use std::sync::Arc;

use chrono::NaiveTime;
use libsql::params;
use mascate_kernel::{Clock, IdGenerator, Record, RecordId, folded_words};
use mascate_platform::{Database, Migration, StoredRow, StoredValueError, stored};

use crate::queue::{default_posting_times, posting_times_from, posting_times_text, unschedule};

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

pub(crate) const ADD_PINTEREST_BOARDS: Migration = Migration {
    version: 12,
    name: "keep the board each Pinterest channel posts to",
    risky: false,
    sql: "ALTER TABLE marketing_promotion_channels ADD COLUMN pinterest_board TEXT;
    ALTER TABLE marketing_promotion_channels ADD COLUMN pinterest_board_name TEXT;",
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
    /// Paid ads on Meta (Facebook, Instagram) or TikTok: a boosted post in
    /// a Program's rules.
    PaidSocial,
    /// The owner's Pinterest account: a board of Pins.
    Pinterest,
    Other,
}

impl ChannelKind {
    pub const ALL: [ChannelKind; 8] = [
        ChannelKind::TelegramChannel,
        ChannelKind::WhatsAppGroup,
        ChannelKind::Instagram,
        ChannelKind::TikTok,
        ChannelKind::Pinterest,
        ChannelKind::PaidSearch,
        ChannelKind::PaidSocial,
        ChannelKind::Other,
    ];

    pub(crate) fn code(self) -> &'static str {
        match self {
            ChannelKind::TelegramChannel => "telegram_channel",
            ChannelKind::WhatsAppGroup => "whatsapp_group",
            ChannelKind::Instagram => "instagram",
            ChannelKind::TikTok => "tiktok",
            ChannelKind::PaidSearch => "paid_search",
            ChannelKind::PaidSocial => "paid_social",
            ChannelKind::Pinterest => "pinterest",
            ChannelKind::Other => "other",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.code() == code)
    }

    /// Whether the app posts to a channel of this kind, now or at its
    /// posting times: a Telegram channel, an Instagram account or a
    /// Pinterest board.
    pub fn has_destination(self) -> bool {
        matches!(
            self,
            ChannelKind::TelegramChannel | ChannelKind::Instagram | ChannelKind::Pinterest
        )
    }

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            ChannelKind::TelegramChannel => "Canal do Telegram",
            ChannelKind::WhatsAppGroup => "Grupo de WhatsApp",
            ChannelKind::Instagram => "Instagram",
            ChannelKind::TikTok => "TikTok",
            ChannelKind::PaidSearch => "Busca paga",
            ChannelKind::PaidSocial => "Anúncio pago na Meta ou no TikTok",
            ChannelKind::Pinterest => "Pinterest",
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
    /// When its scheduled posts go out, on Brasília's clock, earliest
    /// first; only on a channel the app posts to.
    pub posting_times: Vec<NaiveTime>,
    /// Where its Pins go; only on a Pinterest channel, once picked.
    pub pinterest_board: Option<PinterestBoard>,
}

/// A board of the owner's Pinterest account, as Pinterest lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinterestBoard {
    /// Pinterest's id of the board.
    pub id: String,
    pub name: String,
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
    #[error("only a channel the app posts to has posting times")]
    NoDestination,
    #[error("only a Pinterest channel has a board")]
    NotPinterest,
    #[error("a board needs Pinterest's id and its name")]
    BadBoard,
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

const CHANNEL_QUERY: &str = "SELECT id, name, kind, default_tag, keywords, telegram_chat,
         posting_times, pinterest_board, pinterest_board_name
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
                     (id, name, kind, default_tag, keywords, telegram_chat, posting_times,
                      created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    record.id.to_string(),
                    channel.name.clone(),
                    channel.kind.code(),
                    channel.default_tag.clone(),
                    channel.keywords.join("\n"),
                    channel.telegram_chat.clone(),
                    posting_times_text(&default_posting_times()),
                    stored(record.created_at)
                ],
            )
            .await?;
        self.channel(record.id).await
    }

    /// Changes the channel; the links already made keep the marking they
    /// were made with. A post scheduled for its old kind or chat leaves the
    /// queue, since the owner confirmed where it goes. A channel that stops
    /// being a Pinterest one forgets its board.
    pub async fn update(
        &self,
        id: RecordId,
        channel: &NewPromotionChannel,
    ) -> Result<PromotionChannel, ChannelError> {
        let channel = channel.checked()?;
        self.ensure_free(&channel.default_tag, Some(id)).await?;
        let before = self.channel(id).await?;
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE marketing_promotion_channels
                 SET name = ?1, kind = ?2, default_tag = ?3, keywords = ?4, telegram_chat = ?5,
                     updated_at = ?6,
                     pinterest_board = CASE ?2 WHEN 'pinterest' THEN pinterest_board END,
                     pinterest_board_name = CASE ?2 WHEN 'pinterest' THEN pinterest_board_name END
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
        let after = self.channel(id).await?;
        if before.kind != after.kind
            || before.telegram_chat != after.telegram_chat
            || before.pinterest_board != after.pinterest_board
        {
            unschedule(&self.database, id, self.clock.now()).await?;
        }
        Ok(after)
    }

    /// Keeps `board`, picked from the ones Pinterest listed, as where the
    /// Pinterest channel's Pins go. A post scheduled for the old board
    /// leaves the queue.
    pub async fn set_pinterest_board(
        &self,
        id: RecordId,
        board: &PinterestBoard,
    ) -> Result<PromotionChannel, ChannelError> {
        let board = PinterestBoard {
            id: board.id.trim().to_owned(),
            name: board.name.trim().to_owned(),
        };
        if board.id.is_empty() || board.name.is_empty() {
            return Err(ChannelError::BadBoard);
        }
        let before = self.channel(id).await?;
        if before.kind != ChannelKind::Pinterest {
            return Err(ChannelError::NotPinterest);
        }
        self.database
            .connection()
            .execute(
                "UPDATE marketing_promotion_channels
                 SET pinterest_board = ?1, pinterest_board_name = ?2, updated_at = ?3
                 WHERE id = ?4 AND deleted_at IS NULL",
                params![
                    board.id.clone(),
                    board.name.clone(),
                    stored(self.clock.now()),
                    id.to_string()
                ],
            )
            .await?;
        if before.pinterest_board.as_ref().map(|old| &old.id) != Some(&board.id) {
            unschedule(&self.database, id, self.clock.now()).await?;
        }
        self.channel(id).await
    }

    /// Keeps the times `channel`'s scheduled posts go out at, to the
    /// minute, earliest first, each once. The posts already scheduled keep
    /// their time.
    pub async fn set_posting_times(
        &self,
        id: RecordId,
        times: &[NaiveTime],
    ) -> Result<PromotionChannel, ChannelError> {
        if !self.channel(id).await?.kind.has_destination() {
            return Err(ChannelError::NoDestination);
        }
        self.database
            .connection()
            .execute(
                "UPDATE marketing_promotion_channels SET posting_times = ?1, updated_at = ?2
                 WHERE id = ?3 AND deleted_at IS NULL",
                params![
                    posting_times_text(times),
                    stored(self.clock.now()),
                    id.to_string()
                ],
            )
            .await?;
        self.channel(id).await
    }

    /// Deletes the channel; its scheduled posts leave the queue.
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
        unschedule(&self.database, id, self.clock.now()).await?;
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

fn channel_from(row: &libsql::Row) -> Result<PromotionChannel, ChannelError> {
    let kind: String = row.get(2)?;
    let kind = ChannelKind::from_code(&kind).ok_or(ChannelError::Unreadable(kind))?;
    let keywords: String = row.get(4)?;
    let times: String = row.get(6)?;
    let posting_times = if kind.has_destination() {
        posting_times_from(&times).ok_or(ChannelError::Unreadable(times))?
    } else {
        Vec::new()
    };
    let pinterest_board = match (row.get::<Option<String>>(7)?, row.get::<Option<String>>(8)?) {
        (Some(id), Some(name)) if kind == ChannelKind::Pinterest => {
            Some(PinterestBoard { id, name })
        }
        _ => None,
    };
    Ok(PromotionChannel {
        id: row.id_at(0)?,
        name: row.get(1)?,
        kind,
        default_tag: row.get(3)?,
        keywords: keywords
            .lines()
            .filter(|keyword| !keyword.is_empty())
            .map(str::to_owned)
            .collect(),
        telegram_chat: row.get(5)?,
        posting_times,
        pinterest_board,
    })
}
