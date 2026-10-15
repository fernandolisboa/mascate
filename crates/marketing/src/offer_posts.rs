//! Offer Posts (#102, ADR 0031 decision 5): a post about one affiliate
//! offer for one Promotion Channel, born from the channel's template and
//! edited by the owner. The Marketing reads neither the Affiliate nor the
//! Catalog: the caller hands over the facts of the offer. The owner copies
//! the text, or publishes the post on a Telegram channel after confirming
//! it (#103, `publishing`), now or at a time of its queue (#104, `queue`).

use std::path::PathBuf;
use std::sync::Arc;

use chrono::TimeDelta;
use libsql::{Value, params};
use mascate_kernel::{
    Clock, IdGenerator, Money, Percentage, Record, RecordId, Timestamp, brasilia,
};
use mascate_platform::{Database, Migration, StoredRow, StoredValueError, stored};
use rust_decimal::{Decimal, RoundingStrategy};

use crate::AD_DISCLOSURE;
use crate::channels::{ChannelError, ChannelKind, PromotionChannel, PromotionChannels};
use crate::publishing::{
    ConfirmedPost, PostDestination, PublishFailure, PublishRequest, PublishedPost,
};
use crate::queue::price_fresh_at;
use crate::replies::{Contact, contacts_in, unknown_variable};

pub(crate) const CREATE_OFFER_POSTS: Migration = Migration {
    version: 7,
    name: "create the Offer Posts and the post templates",
    risky: false,
    sql: "CREATE TABLE marketing_post_templates (
        id         TEXT PRIMARY KEY,
        channel_id TEXT NOT NULL,
        text       TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE UNIQUE INDEX marketing_post_templates_by_channel
        ON marketing_post_templates (channel_id)
        WHERE deleted_at IS NULL;
    CREATE TABLE marketing_offer_posts (
        id                    TEXT PRIMARY KEY,
        channel_id            TEXT NOT NULL,
        affiliate_link_id     TEXT NOT NULL,
        link                  TEXT NOT NULL,
        product               TEXT NOT NULL,
        platform              TEXT NOT NULL,
        text                  TEXT NOT NULL,
        image_file            TEXT,
        image_url             TEXT,
        price                 TEXT,
        price_before          TEXT,
        currency              TEXT,
        price_seen_at         TEXT,
        price_needs_time      INTEGER NOT NULL,
        forbidden_contacts    TEXT NOT NULL,
        status                TEXT NOT NULL,
        copied_at             TEXT,
        undisclosed_copied_at TEXT,
        created_at            TEXT NOT NULL,
        updated_at            TEXT NOT NULL,
        deleted_at            TEXT
    );
    CREATE INDEX marketing_offer_posts_by_undisclosed_copy
        ON marketing_offer_posts (undisclosed_copied_at);",
};

/// Stands for the product's name in a post template.
pub const POST_PRODUCT: &str = "{produto}";
/// Stands for the price, as in `R$ 149,90`.
pub const POST_PRICE: &str = "{preco}";
/// Stands for the price before the discount.
pub const POST_BEFORE: &str = "{de}";
/// Stands for the discount, as in `25%`.
pub const POST_DISCOUNT: &str = "{desconto}";
/// Stands for the tagged affiliate link.
pub const POST_LINK: &str = "{link}";
/// Stands for when the price was seen, as in `em 13/10/2026 às 21:40,
/// horário de Brasília`.
pub const POST_PRICE_TIME: &str = "{hora_do_preco}";

/// Every variable a post template takes.
pub const POST_VARIABLES: [&str; 6] = [
    POST_PRODUCT,
    POST_PRICE,
    POST_BEFORE,
    POST_DISCOUNT,
    POST_LINK,
    POST_PRICE_TIME,
];

/// The advertising disclosure the CONAR asks for, at the end of every post
/// by default.
pub const DISCLOSURE: &str = "#publi · link de afiliado";

/// The notice Amazon's Associates policy asks for next to a price with its
/// date and time.
pub const PRICE_NOTICE: &str = "Os preços e a disponibilidade dos produtos estão corretos na \
                                data/horário indicados e poderão sofrer alterações";

/// Days a post copied without the disclosure keeps the Reminder up.
pub const DISCLOSURE_WINDOW_DAYS: i64 = 30;

/// The longest text and photo caption a destination takes, in UTF-16 code
/// units, the way Telegram counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PostLimits {
    pub text: usize,
    pub caption: usize,
}

/// Telegram's Bot API: `sendMessage` up to 4096, `sendPhoto`'s caption up
/// to 1024.
pub const TELEGRAM_LIMITS: PostLimits = PostLimits {
    text: 4096,
    caption: 1024,
};

/// What a channel of `kind` takes; `None` where the app knows no limit.
pub fn post_limits(kind: ChannelKind) -> Option<PostLimits> {
    match kind {
        ChannelKind::TelegramChannel => Some(TELEGRAM_LIMITS),
        ChannelKind::WhatsAppGroup
        | ChannelKind::Instagram
        | ChannelKind::TikTok
        | ChannelKind::PaidSearch
        | ChannelKind::Other => None,
    }
}

/// The template a channel of `kind` starts with. A line with a variable
/// that has no value is left out of the post, so each optional value
/// stands on its own line.
pub fn default_template(kind: ChannelKind) -> &'static str {
    match kind {
        ChannelKind::TelegramChannel | ChannelKind::WhatsAppGroup | ChannelKind::Other => {
            "🔥 {produto}\n\nDe {de}\nPor {preco}\nPreço {hora_do_preco}\n📉 {desconto} de \
             desconto\n\n👉 {link}"
        }
        ChannelKind::Instagram | ChannelKind::TikTok => {
            "{produto}\n\nDe {de}\nPor {preco}\nPreço {hora_do_preco}\n{desconto} de \
             desconto\n\nLink: {link}"
        }
        ChannelKind::PaidSearch => "{produto}\nPor {preco}\nPreço {hora_do_preco}\n{link}",
    }
}

/// How much cheaper `price` is than `before`, when both share a currency
/// and `before` is higher: above 0 and at most 100%. Kept at full
/// precision; round it only to show it.
pub fn discount(price: Money, before: Money) -> Option<Percentage> {
    if price.currency() != before.currency()
        || price.is_negative()
        || before.amount() <= price.amount()
    {
        return None;
    }
    let cut = (before.amount() - price.amount()) * Decimal::ONE_HUNDRED / before.amount();
    Percentage::new(cut).ok()
}

/// A discount as a post shows it, in whole percent: `25%`. `None` when it
/// rounds to nothing.
fn discount_text(discount: Percentage) -> Option<String> {
    let whole = discount
        .percent()
        .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero);
    (!whole.is_zero()).then(|| format!("{whole}%"))
}

/// When a price was seen, on Brasília's clock, as Amazon's policy asks:
/// `em 13/10/2026 às 21:40, horário de Brasília`.
pub fn price_time(at: Timestamp) -> String {
    format!(
        "em {}, horário de Brasília",
        at.with_timezone(&brasilia()).format("%d/%m/%Y às %H:%M")
    )
}

/// Whether `text` identifies itself as advertising: `#publi`, or
/// `#publicidade`, anywhere, case aside.
pub fn has_disclosure(text: &str) -> bool {
    text.to_lowercase().contains("#publi")
}

/// A price a post shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfferPrice {
    pub price: Money,
    /// The price before the discount, if the offer shows one.
    pub before: Option<Money>,
    /// When the owner typed or confirmed it, or the Platform gave it.
    pub seen_at: Timestamp,
}

/// The picture of a post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostImage {
    /// A photo in the Product's folder.
    File(PathBuf),
    /// The Platform's picture of the product.
    Url(String),
}

/// What the caller knows of the offer a post promotes (ADR 0031, decision
/// 5): the Affiliate Link and its Program's rules, the Product's name and
/// photo, the price.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferFacts {
    /// The Affiliate Link's id in the Affiliate.
    pub link_id: RecordId,
    /// The tagged link the post carries.
    pub link: String,
    pub product: String,
    /// The Platform's name, as the owner reads it.
    pub platform: String,
    pub price: Option<OfferPrice>,
    pub image: Option<PostImage>,
    /// The Program asks for the date and time next to a price.
    pub price_needs_time: bool,
    /// The contacts the Program forbids or has unread terms on, which the
    /// post must not carry.
    pub forbidden_contacts: Vec<Contact>,
}

/// A post's content, saved or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostDraft {
    /// The Promotion Channel it is for.
    pub channel: RecordId,
    pub facts: OfferFacts,
    /// What goes out, as the owner left it.
    pub text: String,
}

/// Where a post is on its way out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PostStatus {
    Draft,
    /// Confirmed for a time: the queue publishes it then.
    Scheduled,
    /// Its time passed by more than [`crate::LATE_AFTER_MINUTES`] with the
    /// app closed: it waits for the owner to publish it now or reschedule.
    Late,
    /// Its price got older than its Program keeps one before it went out:
    /// it waits for the owner to see the price again.
    PriceExpired,
    /// Handed to the destination, its answer not back yet.
    Sending,
    Sent,
    /// The request went out and nothing says whether the post did: the
    /// owner looks at the channel, then marks it published or publishes it
    /// again.
    Unconfirmed,
    /// The destination published nothing; the failure says why.
    Failed,
}

impl PostStatus {
    const ALL: [PostStatus; 8] = [
        PostStatus::Draft,
        PostStatus::Scheduled,
        PostStatus::Late,
        PostStatus::PriceExpired,
        PostStatus::Sending,
        PostStatus::Sent,
        PostStatus::Unconfirmed,
        PostStatus::Failed,
    ];

    pub(crate) fn code(self) -> &'static str {
        match self {
            PostStatus::Draft => "draft",
            PostStatus::Scheduled => "scheduled",
            PostStatus::Late => "late",
            PostStatus::PriceExpired => "price_expired",
            PostStatus::Sending => "sending",
            PostStatus::Sent => "sent",
            PostStatus::Unconfirmed => "unconfirmed",
            PostStatus::Failed => "failed",
        }
    }

    /// Whether the owner may still change the post. Changing one waiting
    /// in the queue takes it out, until the owner confirms it again.
    pub fn editable(self) -> bool {
        matches!(
            self,
            PostStatus::Draft
                | PostStatus::Scheduled
                | PostStatus::Late
                | PostStatus::PriceExpired
                | PostStatus::Failed
        )
    }

    /// Whether the owner may publish or schedule the post: a draft, one in
    /// the queue or late, a failed one, or again, one that may or may not
    /// have gone out. One whose price expired needs the price first.
    pub fn publishable(self) -> bool {
        matches!(
            self,
            PostStatus::Draft
                | PostStatus::Scheduled
                | PostStatus::Late
                | PostStatus::Failed
                | PostStatus::Unconfirmed
        )
    }

    /// Whether the post waits in the queue or for the owner to pick its
    /// time again.
    pub fn queued(self) -> bool {
        matches!(
            self,
            PostStatus::Scheduled | PostStatus::Late | PostStatus::PriceExpired
        )
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.code() == code)
    }
}

/// A post in its channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentPost {
    pub at: Timestamp,
    /// The destination's id of the message; `None` when the owner marked
    /// it published.
    pub message_id: Option<String>,
    /// Where the post opens, when the destination gave it.
    pub link: Option<String>,
}

/// A saved Offer Post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferPost {
    pub id: RecordId,
    pub draft: PostDraft,
    pub status: PostStatus,
    /// The last time the owner copied its text.
    pub copied_at: Option<Timestamp>,
    /// When the queue publishes it, or was to.
    pub scheduled_at: Option<Timestamp>,
    /// Once [`PostStatus::Sent`].
    pub sent: Option<SentPost>,
    /// Why the last publishing failed or is unconfirmed.
    pub failure: Option<PublishFailure>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// Something wrong with a post. Only the link and the length block it;
/// the rest the owner reads and decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostProblem {
    /// The text lost the affiliate link.
    NoLink,
    /// The text is longer than the destination takes.
    TooLong {
        units: usize,
        limit: usize,
        /// The limit is the photo caption's.
        caption: bool,
    },
    /// The advertising disclosure was taken out.
    NoDisclosure,
    /// The price lost the date and time its Program asks for.
    NoPriceTime,
    /// The price lost the notice its Program asks for.
    NoPriceNotice,
    /// The text carries a contact the Program forbids or has unread terms
    /// on.
    Contact(Contact),
}

impl PostProblem {
    /// Whether the post cannot be saved with it.
    pub fn blocks(&self) -> bool {
        matches!(self, PostProblem::NoLink | PostProblem::TooLong { .. })
    }
}

/// Everything wrong with `draft` for a destination taking `limits`, the
/// blocking problems first.
pub fn check_post(draft: &PostDraft, limits: Option<PostLimits>) -> Vec<PostProblem> {
    let text = draft.text.trim();
    let facts = &draft.facts;
    let mut problems = Vec::new();
    let link = facts.link.trim();
    if link.is_empty() || !text.contains(link) {
        problems.push(PostProblem::NoLink);
    }
    if let Some(limits) = limits {
        let caption = facts.image.is_some();
        let limit = if caption { limits.caption } else { limits.text };
        let units = text.encode_utf16().count();
        if units > limit {
            problems.push(PostProblem::TooLong {
                units,
                limit,
                caption,
            });
        }
    }
    if !has_disclosure(text) {
        problems.push(PostProblem::NoDisclosure);
    }
    let timed_price = facts
        .price
        .filter(|price| facts.price_needs_time && shows_price(text, price));
    if let Some(price) = timed_price {
        if !text.contains(&price_time(price.seen_at)) {
            problems.push(PostProblem::NoPriceTime);
        }
        if !text.contains(PRICE_NOTICE) {
            problems.push(PostProblem::NoPriceNotice);
        }
    }
    // The link itself is not a contact, and its digits are no phone.
    let unlinked = if link.is_empty() {
        text.to_owned()
    } else {
        text.replace(link, " ")
    };
    problems.extend(
        contacts_in(&unlinked)
            .into_iter()
            .filter(|contact| facts.forbidden_contacts.contains(contact))
            .map(PostProblem::Contact),
    );
    problems
}

fn shows_price(text: &str, price: &OfferPrice) -> bool {
    text.contains(&price.price.to_pt_br())
}

/// The post `template` makes of `facts`: every variable filled, each line
/// with a variable that has no value left out, blank runs made one, then
/// the price notice when the Program asks for the price time and the
/// disclosure when the template has none.
fn fill(template: &str, facts: &OfferFacts) -> String {
    let price = facts.price.as_ref();
    let discounted = price.and_then(|price| {
        let before = price.before?;
        Some((before, discount(price.price, before)?))
    });
    let product = facts.product.trim();
    let link = facts.link.trim();
    let values: [(&str, Option<String>); 6] = [
        (
            POST_PRODUCT,
            (!product.is_empty()).then(|| product.to_owned()),
        ),
        (POST_PRICE, price.map(|price| price.price.to_pt_br())),
        (POST_BEFORE, discounted.map(|(before, _)| before.to_pt_br())),
        (
            POST_DISCOUNT,
            discounted.and_then(|(_, discount)| discount_text(discount)),
        ),
        (POST_LINK, (!link.is_empty()).then(|| link.to_owned())),
        (
            POST_PRICE_TIME,
            price
                .filter(|_| facts.price_needs_time)
                .map(|price| price_time(price.seen_at)),
        ),
    ];
    let mut lines: Vec<String> = Vec::new();
    for line in template.lines() {
        let mut filled = line.to_owned();
        let mut missing = false;
        for (variable, value) in &values {
            if filled.contains(variable) {
                match value {
                    Some(value) => filled = filled.replace(variable, value),
                    None => missing = true,
                }
            }
        }
        let filled = filled.trim_end().to_owned();
        let blank_after_blank =
            filled.is_empty() && lines.last().is_none_or(|last| last.is_empty());
        if !missing && !blank_after_blank {
            lines.push(filled);
        }
    }
    let mut text = lines.join("\n").trim().to_owned();
    let mut endings = Vec::new();
    if let Some(price) = price.filter(|price| facts.price_needs_time && shows_price(&text, price)) {
        let time = price_time(price.seen_at);
        endings.push(if text.contains(&time) {
            format!("{PRICE_NOTICE}.")
        } else {
            format!("Preço {time}. {PRICE_NOTICE}.")
        });
    }
    if !has_disclosure(&text) {
        endings.push(DISCLOSURE.to_owned());
    }
    for ending in endings {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&ending);
    }
    text
}

#[derive(Debug, thiserror::Error)]
pub enum PostError {
    #[error("the post cannot be saved as it is: {0:?}")]
    Blocked(Vec<PostProblem>),
    #[error("the template uses {0}, which the app does not fill")]
    UnknownVariable(String),
    #[error("the template needs {POST_LINK}")]
    TemplateWithoutLink,
    #[error("no such Promotion Channel")]
    NoSuchChannel,
    #[error("no such Offer Post")]
    NotFound,
    #[error("the post's channel is not one the app publishes to")]
    NoDestination,
    #[error("the Telegram channel has no chat yet")]
    NoChat,
    #[error("the post is {0:?}, which does not allow it")]
    WrongStatus(PostStatus),
    #[error("the post changed since it was confirmed")]
    Changed,
    #[error("the time has passed")]
    PastTime,
    #[error("another post of the channel is scheduled at that time")]
    TimeTaken,
    #[error("the channel has no posting times")]
    NoPostingTimes,
    #[error("the price would be older than its Program keeps one")]
    OldPrice,
    #[error("the posts hold a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for PostError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => PostError::Unreadable(text),
            StoredValueError::Sql(error) => PostError::Sql(error),
        }
    }
}

impl From<ChannelError> for PostError {
    fn from(error: ChannelError) -> Self {
        match error {
            ChannelError::NotFound => PostError::NoSuchChannel,
            ChannelError::Unreadable(text) => PostError::Unreadable(text),
            ChannelError::Sql(error) => PostError::Sql(error),
            other => PostError::Unreadable(other.to_string()),
        }
    }
}

pub(crate) const POST_QUERY: &str =
    "SELECT id, channel_id, affiliate_link_id, link, product, platform,
         text, image_file, image_url, price, price_before, currency, price_seen_at,
         price_needs_time, forbidden_contacts, status, copied_at, created_at, updated_at,
         sent_at, message_id, post_link, failure, failure_detail, scheduled_at
     FROM marketing_offer_posts WHERE deleted_at IS NULL";

/// The owner's Offer Posts and the template of each Promotion Channel.
pub struct OfferPosts {
    pub(crate) database: Arc<Database>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) ids: Arc<dyn IdGenerator>,
    pub(crate) channels: PromotionChannels,
}

impl OfferPosts {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            channels: PromotionChannels::new(database.clone(), clock.clone(), ids.clone()),
            database,
            clock,
            ids,
        }
    }

    /// The channel's template: the owner's, or its kind's default.
    pub async fn template(&self, channel: &PromotionChannel) -> Result<String, PostError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT text FROM marketing_post_templates
                 WHERE channel_id = ?1 AND deleted_at IS NULL",
                params![channel.id.to_string()],
            )
            .await?;
        Ok(match rows.next().await? {
            Some(row) => row.get(0)?,
            None => default_template(channel.kind).to_owned(),
        })
    }

    /// Keeps the owner's template for `channel`, trimmed: it takes only
    /// [`POST_VARIABLES`] and must carry the link.
    pub async fn set_template(&self, channel: RecordId, text: &str) -> Result<String, PostError> {
        let text = text.trim();
        if let Some(unknown) = unknown_variable(text, &POST_VARIABLES) {
            return Err(PostError::UnknownVariable(unknown));
        }
        if !text.contains(POST_LINK) {
            return Err(PostError::TemplateWithoutLink);
        }
        self.channels.channel(channel).await?;
        let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
        self.database
            .connection()
            .execute(
                "INSERT INTO marketing_post_templates
                     (id, channel_id, text, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT (channel_id) WHERE deleted_at IS NULL
                 DO UPDATE SET text = excluded.text, updated_at = excluded.updated_at",
                params![
                    record.id.to_string(),
                    channel.to_string(),
                    text,
                    stored(record.created_at)
                ],
            )
            .await?;
        Ok(text.to_owned())
    }

    /// Puts `channel` back on its kind's default template.
    pub async fn reset_template(&self, channel: RecordId) -> Result<(), PostError> {
        self.database
            .connection()
            .execute(
                "UPDATE marketing_post_templates SET deleted_at = ?1, updated_at = ?1
                 WHERE channel_id = ?2 AND deleted_at IS NULL",
                params![stored(self.clock.now()), channel.to_string()],
            )
            .await?;
        Ok(())
    }

    /// A post for `channel` made of `facts` with the channel's template,
    /// not saved yet.
    pub async fn draft(
        &self,
        facts: &OfferFacts,
        channel: &PromotionChannel,
    ) -> Result<PostDraft, PostError> {
        let template = self.template(channel).await?;
        Ok(PostDraft {
            channel: channel.id,
            text: fill(&template, facts),
            facts: facts.clone(),
        })
    }

    /// What is wrong with `draft` for its channel.
    pub async fn check(&self, draft: &PostDraft) -> Result<Vec<PostProblem>, PostError> {
        let channel = self.channels.channel(draft.channel).await?;
        Ok(check_post(draft, post_limits(channel.kind)))
    }

    /// Saves `draft` as a new post, unless a problem blocks it.
    pub async fn create(&self, draft: &PostDraft) -> Result<OfferPost, PostError> {
        let draft = self.ensure_savable(draft).await?;
        let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
        let mut values = row_values(&draft);
        values.extend([
            Value::from(PostStatus::Draft.code()),
            Value::from(record.id.to_string()),
            Value::from(stored(record.created_at)),
        ]);
        self.database
            .connection()
            .execute(
                "INSERT INTO marketing_offer_posts
                     (channel_id, affiliate_link_id, link, product, platform, text, image_file,
                      image_url, price, price_before, currency, price_seen_at, price_needs_time,
                      forbidden_contacts, status, id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                     ?17, ?17)",
                values,
            )
            .await?;
        self.post(record.id).await
    }

    /// Saves `draft` over the post `id`, unless a problem blocks it or the
    /// post is no longer [`PostStatus::editable`]. A post in the queue
    /// leaves it as a draft: the owner confirms the new text again.
    pub async fn update(&self, id: RecordId, draft: &PostDraft) -> Result<OfferPost, PostError> {
        let status = self.post(id).await?.status;
        if !status.editable() {
            return Err(PostError::WrongStatus(status));
        }
        let draft = self.ensure_savable(draft).await?;
        let mut values = row_values(&draft);
        values.extend([
            Value::from(stored(self.clock.now())),
            Value::from(id.to_string()),
        ]);
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE marketing_offer_posts
                 SET channel_id = ?1, affiliate_link_id = ?2, link = ?3, product = ?4,
                     platform = ?5, text = ?6, image_file = ?7, image_url = ?8, price = ?9,
                     price_before = ?10, currency = ?11, price_seen_at = ?12,
                     price_needs_time = ?13, forbidden_contacts = ?14, updated_at = ?15,
                     status = CASE status WHEN 'failed' THEN 'failed' ELSE 'draft' END,
                     scheduled_at = NULL
                 WHERE id = ?16 AND deleted_at IS NULL
                     AND status IN ('draft', 'scheduled', 'late', 'price_expired', 'failed')",
                values,
            )
            .await?;
        if changed == 0 {
            return Err(PostError::Changed);
        }
        self.post(id).await
    }

    /// `draft` with its text trimmed, if nothing blocks it on its channel.
    async fn ensure_savable(&self, draft: &PostDraft) -> Result<PostDraft, PostError> {
        let draft = PostDraft {
            text: draft.text.trim().to_owned(),
            ..draft.clone()
        };
        let blocking: Vec<PostProblem> = self
            .check(&draft)
            .await?
            .into_iter()
            .filter(PostProblem::blocks)
            .collect();
        if blocking.is_empty() {
            Ok(draft)
        } else {
            Err(PostError::Blocked(blocking))
        }
    }

    /// Every post, the latest changed first.
    pub async fn posts(&self) -> Result<Vec<OfferPost>, PostError> {
        let mut rows = self
            .database
            .connection()
            .query(&format!("{POST_QUERY} ORDER BY updated_at DESC, id"), ())
            .await?;
        let mut posts = Vec::new();
        while let Some(row) = rows.next().await? {
            posts.push(post_from(&row)?);
        }
        Ok(posts)
    }

    pub async fn post(&self, id: RecordId) -> Result<OfferPost, PostError> {
        let mut rows = self
            .database
            .connection()
            .query(
                &format!("{POST_QUERY} AND id = ?1"),
                params![id.to_string()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => post_from(&row),
            None => Err(PostError::NotFound),
        }
    }

    pub async fn delete(&self, id: RecordId) -> Result<(), PostError> {
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE marketing_offer_posts SET deleted_at = ?1, updated_at = ?1
                 WHERE id = ?2 AND deleted_at IS NULL",
                params![stored(self.clock.now()), id.to_string()],
            )
            .await?;
        if changed == 0 {
            return Err(PostError::NotFound);
        }
        Ok(())
    }

    /// Notes that the owner copied the post's text to share it; a text
    /// without the disclosure raises [`AD_DISCLOSURE`] for
    /// [`DISCLOSURE_WINDOW_DAYS`].
    pub async fn copied(&self, id: RecordId) -> Result<OfferPost, PostError> {
        let post = self.post(id).await?;
        let now = stored(self.clock.now());
        let undisclosed = (!has_disclosure(&post.draft.text)).then(|| now.clone());
        self.database
            .connection()
            .execute(
                "UPDATE marketing_offer_posts
                 SET copied_at = ?1, updated_at = ?1,
                     undisclosed_copied_at = coalesce(?2, undisclosed_copied_at)
                 WHERE id = ?3",
                params![now, undisclosed, id.to_string()],
            )
            .await?;
        self.post(id).await
    }

    /// Checks the saved post `id` for its destination: what comes back
    /// carries the final text, the picture and the chat for the owner to
    /// read and confirm. Only a post on a Telegram channel with its chat,
    /// [`PostStatus::publishable`], with nothing blocking it and with a
    /// price its Program still keeps gets one, so a text over the
    /// destination's limits never reaches it.
    pub async fn prepare(&self, id: RecordId) -> Result<PublishRequest, PostError> {
        let post = self.post(id).await?;
        if !post.status.publishable() {
            return Err(PostError::WrongStatus(post.status));
        }
        if !price_fresh_at(&post.draft.facts, self.clock.now()) {
            return Err(PostError::OldPrice);
        }
        let channel = self.channels.channel(post.draft.channel).await?;
        if channel.kind != ChannelKind::TelegramChannel {
            return Err(PostError::NoDestination);
        }
        let chat = channel.telegram_chat.clone().ok_or(PostError::NoChat)?;
        let blocking: Vec<PostProblem> = check_post(&post.draft, post_limits(channel.kind))
            .into_iter()
            .filter(PostProblem::blocks)
            .collect();
        if !blocking.is_empty() {
            return Err(PostError::Blocked(blocking));
        }
        Ok(PublishRequest {
            post: post.id,
            channel: channel.name,
            chat,
            text: post.draft.text,
            image: post.draft.facts.image,
            prepared_status: post.status,
            prepared_at: post.updated_at,
        })
    }

    /// Publishes the post the owner confirmed, once. It is marked
    /// [`PostStatus::Sending`] before the destination is called, and only
    /// if it is still as the owner read it; then [`PostStatus::Sent`],
    /// [`PostStatus::Failed`] when nothing went out, or
    /// [`PostStatus::Unconfirmed`] when it may have, which nothing sends
    /// again on its own. A post that may have gone out without the
    /// disclosure raises [`AD_DISCLOSURE`], as a copied one does.
    pub async fn publish(
        &self,
        destination: &dyn PostDestination,
        post: ConfirmedPost,
    ) -> Result<OfferPost, PostError> {
        let request = post.request();
        let claimed = self
            .database
            .connection()
            .execute(
                "UPDATE marketing_offer_posts
                 SET status = ?1, failure = NULL, failure_detail = NULL, updated_at = ?2
                 WHERE id = ?3 AND deleted_at IS NULL AND status = ?4 AND updated_at = ?5",
                params![
                    PostStatus::Sending.code(),
                    stored(self.clock.now()),
                    request.post.to_string(),
                    request.prepared_status.code(),
                    stored(request.prepared_at)
                ],
            )
            .await?;
        if claimed == 0 {
            return Err(PostError::Changed);
        }
        let published = destination.publish(&post);
        let request = post.request();
        let now = stored(self.clock.now());
        let undisclosed = (!has_disclosure(&request.text)).then(|| now.clone());
        match published {
            Ok(PublishedPost { message_id, link }) => {
                self.database
                    .connection()
                    .execute(
                        "UPDATE marketing_offer_posts
                         SET status = ?1, sent_at = ?2, message_id = ?3, post_link = ?4,
                             updated_at = ?2,
                             undisclosed_copied_at = coalesce(?5, undisclosed_copied_at)
                         WHERE id = ?6",
                        params![
                            PostStatus::Sent.code(),
                            now,
                            message_id,
                            link,
                            undisclosed,
                            request.post.to_string()
                        ],
                    )
                    .await?;
            }
            Err(failure) => {
                let (status, undisclosed) = if failure.may_have_gone_out() {
                    (PostStatus::Unconfirmed, undisclosed)
                } else {
                    (PostStatus::Failed, None)
                };
                self.database
                    .connection()
                    .execute(
                        "UPDATE marketing_offer_posts
                         SET status = ?1, failure = ?2, failure_detail = ?3, updated_at = ?4,
                             undisclosed_copied_at = coalesce(?5, undisclosed_copied_at)
                         WHERE id = ?6",
                        params![
                            status.code(),
                            failure.code(),
                            failure.detail().map(str::to_owned),
                            now,
                            undisclosed,
                            request.post.to_string()
                        ],
                    )
                    .await?;
            }
        }
        self.post(request.post).await
    }

    /// The owner found the [`PostStatus::Unconfirmed`] post in the channel.
    pub async fn mark_published(&self, id: RecordId) -> Result<OfferPost, PostError> {
        let post = self.post(id).await?;
        if post.status != PostStatus::Unconfirmed {
            return Err(PostError::WrongStatus(post.status));
        }
        let now = stored(self.clock.now());
        let undisclosed = (!has_disclosure(&post.draft.text)).then(|| now.clone());
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE marketing_offer_posts
                 SET status = ?1, sent_at = ?2, message_id = NULL, post_link = NULL,
                     failure = NULL, failure_detail = NULL, updated_at = ?2,
                     undisclosed_copied_at = coalesce(?3, undisclosed_copied_at)
                 WHERE id = ?4 AND status = ?5",
                params![
                    PostStatus::Sent.code(),
                    now,
                    undisclosed,
                    id.to_string(),
                    PostStatus::Unconfirmed.code()
                ],
            )
            .await?;
        if changed == 0 {
            return Err(PostError::Changed);
        }
        self.post(id).await
    }

    /// Marks [`PostStatus::Unconfirmed`] every post left
    /// [`PostStatus::Sending`] when the app closed mid-way, on the next
    /// start; how many there were.
    pub async fn settle_interrupted(&self) -> Result<u64, PostError> {
        Ok(self
            .database
            .connection()
            .execute(
                "UPDATE marketing_offer_posts
                 SET status = ?1, failure = ?2, failure_detail = NULL, updated_at = ?3
                 WHERE status = ?4 AND deleted_at IS NULL",
                params![
                    PostStatus::Unconfirmed.code(),
                    PublishFailure::Interrupted.code(),
                    stored(self.clock.now()),
                    PostStatus::Sending.code()
                ],
            )
            .await?)
    }

    /// The Reminders the posts raise now: [`AD_DISCLOSURE`] while a post
    /// went out without the disclosure in the last
    /// [`DISCLOSURE_WINDOW_DAYS`] days.
    pub async fn raised_reminders(&self) -> Result<Vec<&'static str>, PostError> {
        let since = self.clock.now() - TimeDelta::days(DISCLOSURE_WINDOW_DAYS);
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT 1 FROM marketing_offer_posts WHERE undisclosed_copied_at >= ?1 LIMIT 1",
                params![stored(since)],
            )
            .await?;
        Ok(match rows.next().await? {
            Some(_) => vec![AD_DISCLOSURE.key],
            None => Vec::new(),
        })
    }
}

/// `draft` as the 14 columns from `channel_id` to `forbidden_contacts`.
fn row_values(draft: &PostDraft) -> Vec<Value> {
    let facts = &draft.facts;
    let (image_file, image_url) = match &facts.image {
        Some(PostImage::File(path)) => (Some(path.to_string_lossy().into_owned()), None),
        Some(PostImage::Url(url)) => (None, Some(url.clone())),
        None => (None, None),
    };
    let price = facts.price.as_ref();
    let optional = |text: Option<String>| text.map_or(Value::Null, Value::from);
    vec![
        Value::from(draft.channel.to_string()),
        Value::from(facts.link_id.to_string()),
        Value::from(facts.link.clone()),
        Value::from(facts.product.clone()),
        Value::from(facts.platform.clone()),
        Value::from(draft.text.clone()),
        optional(image_file),
        optional(image_url),
        optional(price.map(|price| price.price.amount().to_string())),
        optional(price.and_then(|price| price.before.map(|before| before.amount().to_string()))),
        optional(price.map(|price| price.price.currency().code().to_owned())),
        optional(price.map(|price| stored(price.seen_at))),
        Value::from(i64::from(facts.price_needs_time)),
        Value::from(
            facts
                .forbidden_contacts
                .iter()
                .map(|contact| contact.code())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    ]
}

pub(crate) fn post_from(row: &libsql::Row) -> Result<OfferPost, PostError> {
    let image = match (row.get::<Option<String>>(7)?, row.get::<Option<String>>(8)?) {
        (Some(file), _) => Some(PostImage::File(PathBuf::from(file))),
        (None, Some(url)) => Some(PostImage::Url(url)),
        (None, None) => None,
    };
    let price = match row.get::<Option<String>>(9)? {
        Some(_) => {
            let currency = row.currency_at(11)?;
            let before = row
                .get::<Option<String>>(10)?
                .map(|_| row.decimal_at(10))
                .transpose()?;
            Some(OfferPrice {
                price: Money::new(row.decimal_at(9)?, currency),
                before: before.map(|before| Money::new(before, currency)),
                seen_at: row.time_at(12)?,
            })
        }
        None => None,
    };
    let contacts: String = row.get(14)?;
    let forbidden_contacts = contacts
        .lines()
        .filter(|code| !code.is_empty())
        .map(|code| Contact::from_code(code).ok_or_else(|| PostError::Unreadable(code.into())))
        .collect::<Result<_, _>>()?;
    let status: String = row.get(15)?;
    let status = PostStatus::from_code(&status).ok_or(PostError::Unreadable(status))?;
    let sent = match row.optional_time_at(19)? {
        Some(at) => Some(SentPost {
            at,
            message_id: row.get(20)?,
            link: row.get(21)?,
        }),
        None => None,
    };
    let failure = match row.get::<Option<String>>(22)? {
        Some(code) => Some(
            PublishFailure::from_code(&code, row.get(23)?).ok_or(PostError::Unreadable(code))?,
        ),
        None => None,
    };
    Ok(OfferPost {
        id: row.id_at(0)?,
        draft: PostDraft {
            channel: row.id_at(1)?,
            facts: OfferFacts {
                link_id: row.id_at(2)?,
                link: row.get(3)?,
                product: row.get(4)?,
                platform: row.get(5)?,
                price,
                image,
                price_needs_time: row.get::<i64>(13)? != 0,
                forbidden_contacts,
            },
            text: row.get(6)?,
        },
        status,
        copied_at: row.optional_time_at(16)?,
        scheduled_at: row.optional_time_at(24)?,
        sent,
        failure,
        created_at: row.time_at(17)?,
        updated_at: row.time_at(18)?,
    })
}
