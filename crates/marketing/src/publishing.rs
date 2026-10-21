//! Publishing an Offer Post (#103, ADR 0031 decisions 6 and 7): a
//! destination behind a port a Platform's adapter implements (ADR 0013),
//! reached only with a post the owner confirmed, the way answers to buyers
//! are (ADR 0022). The destination has no idempotency key, so a post whose
//! fate is unknown is never sent again on its own.

use mascate_kernel::{PlatformError, RecordId, Timestamp};
use mascate_platform::Migration;

use crate::channels::ChannelKind;
use crate::offer_posts::{PostImage, PostStatus};

pub(crate) const PUBLISH_OFFER_POSTS: Migration = Migration {
    version: 8,
    name: "keep how each Offer Post went out",
    risky: false,
    sql: "ALTER TABLE marketing_offer_posts ADD COLUMN sent_at TEXT;
    ALTER TABLE marketing_offer_posts ADD COLUMN message_id TEXT;
    ALTER TABLE marketing_offer_posts ADD COLUMN post_link TEXT;
    ALTER TABLE marketing_offer_posts ADD COLUMN failure TEXT;
    ALTER TABLE marketing_offer_posts ADD COLUMN failure_detail TEXT;",
};

/// Where the app stands in a destination's chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationStanding {
    /// An administrator allowed to post.
    CanPost,
    /// The chat is a group or a private chat, not a channel.
    NotAChannel,
    /// Not an administrator of the channel.
    NotAdministrator,
    /// An administrator without the right to post.
    CannotPost,
}

/// What a destination answered about a chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationCheck {
    /// The chat's title, as its members see it.
    pub title: String,
    pub standing: DestinationStanding,
}

/// A post the destination took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedPost {
    /// The destination's id of the message.
    pub message_id: String,
    /// Where the post opens; `None` when the chat has no public address.
    pub link: Option<String>,
}

/// Why a post did not go out, or may not have.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublishFailure {
    #[error("the destination's Connection is not set up")]
    NotConnected,
    /// The destination no longer accepts the login.
    #[error("the destination no longer accepts the login")]
    Expired,
    /// The destination refused the post and published nothing; the reason
    /// is its own words.
    #[error("the destination refused the post: {0}")]
    Refused(String),
    /// The destination kept asking to wait; nothing was published.
    #[error("the destination asked to wait and kept asking")]
    RateLimited,
    /// The request never went out: the destination could not be reached,
    /// or the photo could not be read.
    #[error("the post did not go out: {0}")]
    NotSent(String),
    /// The request went out and no answer says what happened: a timeout, a
    /// dropped connection or the destination failing on its side. The
    /// post may be in the channel.
    #[error("the post may have gone out: {0}")]
    Unconfirmed(String),
    /// The app closed while the post was on its way. Only the Marketing
    /// reports it, on the next start.
    #[error("the app closed while the post was on its way")]
    Interrupted,
}

impl PublishFailure {
    /// Whether the post may be in the channel, so it is never sent again
    /// without the owner looking first.
    pub fn may_have_gone_out(&self) -> bool {
        matches!(
            self,
            PublishFailure::Unconfirmed(_) | PublishFailure::Interrupted
        )
    }

    pub(crate) fn code(&self) -> &'static str {
        match self {
            PublishFailure::NotConnected => "not_connected",
            PublishFailure::Expired => "expired",
            PublishFailure::Refused(_) => "refused",
            PublishFailure::RateLimited => "rate_limited",
            PublishFailure::NotSent(_) => "not_sent",
            PublishFailure::Unconfirmed(_) => "unconfirmed",
            PublishFailure::Interrupted => "interrupted",
        }
    }

    pub(crate) fn detail(&self) -> Option<&str> {
        match self {
            PublishFailure::Refused(detail)
            | PublishFailure::NotSent(detail)
            | PublishFailure::Unconfirmed(detail) => Some(detail),
            PublishFailure::NotConnected
            | PublishFailure::Expired
            | PublishFailure::RateLimited
            | PublishFailure::Interrupted => None,
        }
    }

    pub(crate) fn from_code(code: &str, detail: Option<String>) -> Option<Self> {
        let detail = detail.unwrap_or_default();
        Some(match code {
            "not_connected" => PublishFailure::NotConnected,
            "expired" => PublishFailure::Expired,
            "refused" => PublishFailure::Refused(detail),
            "rate_limited" => PublishFailure::RateLimited,
            "not_sent" => PublishFailure::NotSent(detail),
            "unconfirmed" => PublishFailure::Unconfirmed(detail),
            "interrupted" => PublishFailure::Interrupted,
            _ => return None,
        })
    }
}

/// Where Offer Posts go out, as a Platform's adapter reaches it. Calls
/// block on the network, so they run off the UI thread.
pub trait PostDestination: Send + Sync {
    /// Whether the app can post in `chat`.
    fn check(&self, chat: &str) -> Result<DestinationCheck, PlatformError>;

    /// Publishes the post the owner confirmed, once: only a wait the
    /// destination guarantees published nothing is tried again.
    fn publish(&self, post: &ConfirmedPost) -> Result<PublishedPost, PublishFailure>;
}

/// A post checked and ready for its destination, waiting for the owner to
/// read the final text, the picture and where it goes, and confirm. Only
/// [`PublishRequest::confirm`] makes it something
/// [`crate::OfferPosts::publish`] takes; dropping it publishes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "a post goes nowhere until confirmed and passed to OfferPosts::publish"]
pub struct PublishRequest {
    pub(crate) post: RecordId,
    /// The kind of the Promotion Channel, which names its destination.
    pub(crate) kind: ChannelKind,
    pub(crate) channel: String,
    /// The destination's address: the Telegram chat or the board's id.
    pub(crate) chat: String,
    /// Where it goes, as the owner reads it: the chat or the board's name.
    pub(crate) place: String,
    /// The product's name, cut to the destination's title limit.
    pub(crate) title: String,
    /// The Affiliate Link, whole, for a destination that takes a link
    /// beside the text.
    pub(crate) link: String,
    pub(crate) text: String,
    pub(crate) image: Option<PostImage>,
    pub(crate) prepared_status: PostStatus,
    /// When the post last changed: a post changed since is not the one the
    /// owner confirmed.
    pub(crate) prepared_at: Timestamp,
}

impl PublishRequest {
    /// The text that will reach the channel, as it is.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn image(&self) -> Option<&PostImage> {
        self.image.as_ref()
    }

    /// The Promotion Channel's name.
    pub fn channel(&self) -> &str {
        &self.channel
    }

    /// The Promotion Channel's kind, which picks the destination.
    pub fn kind(&self) -> ChannelKind {
        self.kind
    }

    /// The destination's address: a Telegram chat, `@name` or a `-100…`
    /// id, or a Pinterest board's id.
    pub fn chat(&self) -> &str {
        &self.chat
    }

    /// Where the post goes, as the owner reads it: the chat, or the board's
    /// name.
    pub fn place(&self) -> &str {
        &self.place
    }

    /// The title of a post that has one: the product's name, cut to fit.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The Affiliate Link the post promotes, whole.
    pub fn link(&self) -> &str {
        &self.link
    }

    /// The owner read the post and where it goes, and confirmed it.
    pub fn confirm(self) -> ConfirmedPost {
        ConfirmedPost { request: self }
    }
}

/// A post the owner confirmed. Only the Marketing makes one, and
/// publishing takes it whole, so it goes out at most once.
#[derive(Debug, PartialEq, Eq)]
pub struct ConfirmedPost {
    request: PublishRequest,
}

impl ConfirmedPost {
    pub fn text(&self) -> &str {
        &self.request.text
    }

    pub fn image(&self) -> Option<&PostImage> {
        self.request.image.as_ref()
    }

    pub fn chat(&self) -> &str {
        &self.request.chat
    }

    pub fn title(&self) -> &str {
        &self.request.title
    }

    pub fn link(&self) -> &str {
        &self.request.link
    }

    pub(crate) fn request(&self) -> &PublishRequest {
        &self.request
    }
}
