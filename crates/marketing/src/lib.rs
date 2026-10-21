//! Marketing: Listing quality and shop performance, questions, reputation, Promotions, Ads cost, Listing Copy,
//! Promotion Channels and Offer Posts, published through a destination's port
//! now or from the queue at a time the owner confirmed, and the Sales Events
//! calendar they are planned around.

mod ads;
mod channels;
mod copy;
mod merge;
mod offer_posts;
mod performance;
mod publishing;
mod quality;
mod questions;
mod queue;
mod replies;
mod reputation;
mod sales_channels;
mod sales_events;

pub use ads::{
    ADS_HISTORY_DAYS, ADS_REFRESH_MINUTES, ADS_SETTLE_DAYS, AdDay, AdMetrics, AdsError, AdsReport,
    AdsSync, AdsSyncState, AdvertisedListing, CampaignAds, CampaignStatus, ChannelAd, ChannelAds,
    ChannelCampaign, ProductAds, ProductRoas,
};
pub use channels::{
    CHANNEL_NAME_MAX_CHARS, CHANNEL_TAG_MAX_CHARS, ChannelError, ChannelKind, NewPromotionChannel,
    PinterestBoard, PromotionChannel, PromotionChannels, suggested_tag,
};
pub use copy::{
    BriefAttribute, COPY_INSTRUCTIONS, CategoryRules, CopyBrief, CopyChannel, CopyError, CopyField,
    CopyProblem, CopyRequest, CopySettings, CopyWriter, DEFAULT_COPY_MODEL, GeneratedCopy,
    ListingCopy, MODEL_MAX_CHARS, WrittenCopy, check_copy,
};
pub use merge::MERGE;
pub use offer_posts::{
    DISCLOSURE, DISCLOSURE_WINDOW_DAYS, INSTAGRAM_LIMITS, OfferFacts, OfferPost, OfferPosts,
    OfferPrice, PINTEREST_LIMITS, POST_BEFORE, POST_DISCOUNT, POST_LINK, POST_PRICE,
    POST_PRICE_TIME, POST_PRODUCT, POST_VARIABLES, PRICE_NOTICE, PostDraft, PostError, PostImage,
    PostLimits, PostProblem, PostStatus, SentPost, TELEGRAM_LIMITS, check_post, default_template,
    discount, has_disclosure, post_limits, price_time,
};
pub use performance::{
    ChannelPerformance, Comparator, MetricGroup, MetricTarget, MetricUnit, PerformanceMetric,
    PerformanceRating, ShopPerformance,
};
pub use publishing::{
    ConfirmedPost, DestinationCheck, DestinationStanding, PostDestination, PostFate,
    PublishFailure, PublishRequest, PublishedPost,
};
pub use quality::{
    ActionKind, ChannelQuality, IMPACT_DAYS, ListedItem, ListingQuality, QualityAction,
    QualityError, QualityLevel, QualityRow, QualitySource, QualitySync, Rating, Score,
};
pub use questions::{
    AnswerRequest, ChannelAnswer, ChannelQuestion, ChannelQuestions, ConfirmedAnswer, Question,
    QuestionError, QuestionStatus, QuestionSync, Questions, SentAnswer,
};
pub use queue::{
    CHAT_SPACING, DestinationLimit, INSTAGRAM_POSTS_PER_DAY, LATE_AFTER_MINUTES,
    PINTEREST_POSTS_PER_DAY, PRICE_KEEPS_HOURS, PostDestinations, PostQueue, PostTime,
    QUOTA_RECHECK_MINUTES, QueueRound, RATE_LIMIT_WAIT_MINUTES, default_posting_times,
    next_free_time,
};
pub use replies::{
    ANSWER_MAX_CHARS, Contact, DEADLINE_VARIABLE, MAX_DISPATCH_DAYS, NewReplyTemplate,
    PRODUCT_VARIABLE, QuestionSettings, ReplyProblem, ReplyTemplate, ReplyTemplates,
    TEMPLATE_NAME_MAX_CHARS, check_answer, contact_in, contacts_in,
};
pub use reputation::{
    ArrivedReview, ChannelReputation, ChannelReview, LOW_RATING, ListingReviews, LowReview,
    MetricReading, MetricStanding, PRODUCT_ADS_MIN_SALES, ProductReviews, REFRESH_MINUTES,
    Reputation, ReputationColor, ReputationError, ReputationMetric, ReputationSource,
    ReputationStanding, ReputationSync, ReviewedItem, SellerTool,
};
pub use sales_events::{
    DateRule, EVENT_NAME_MAX_CHARS, EVENT_NOTES_MAX_CHARS, EventError, MAX_DAYS_AFTER_THANKSGIVING,
    MAX_LEAD_DAYS, NewSalesEvent, SalesEvent, SalesEvents, UpcomingEvent, default_sales_events,
};

use mascate_platform::{
    Flag, FlagKind, ModuleMigrations, Phase, Reminder, ReminderShows, ReminderTopic,
};

/// Answering a buyer's question on Mercado Livre without the owner
/// confirming the answer first.
pub const AUTO_ANSWER_QUESTIONS: Flag = Flag {
    key: "marketing.auto_answer_questions",
    name: "Resposta automática de perguntas no Mercado Livre",
    kind: FlagKind::ProductChoice,
    phase: Phase::One,
    reason: "Os termos do Mercado Livre permitem, mas você decidiu que toda resposta a \
             comprador só sai depois da sua confirmação.",
    risk: "Uma resposta errada ou fora de contexto chega ao comprador sem revisão e fica \
           pública no anúncio.",
};

/// This module's flags.
pub const FLAGS: &[Flag] = &[AUTO_ANSWER_QUESTIONS];

/// This module's own tables.
pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "marketing",
    migrations: &[
        quality::CREATE_LISTING_QUALITY,
        questions::CREATE_QUESTIONS,
        reputation::CREATE_REPUTATION,
        ads::CREATE_PRODUCT_ADS,
        copy::CREATE_COPY_SETTINGS,
        channels::CREATE_PROMOTION_CHANNELS,
        offer_posts::CREATE_OFFER_POSTS,
        publishing::PUBLISH_OFFER_POSTS,
        queue::SCHEDULE_OFFER_POSTS,
        sales_events::CREATE_SALES_EVENTS,
        sales_channels::ADD_SALES_CHANNELS,
        channels::ADD_PINTEREST_BOARDS,
    ],
};

pub const AD_DISCLOSURE: Reminder = Reminder {
    key: "marketing.ad_disclosure",
    topic: ReminderTopic::Legal,
    title: "Identifique a publicidade nos posts de afiliado",
    text: "Um post sem #publi saiu nos últimos 30 dias. Post com link de afiliado é \
           publicidade, e o CONAR pede que ela seja identificada: deixe \"#publi · link de \
           afiliado\" no fim, como o app sugere. O app continua funcionando do mesmo jeito.",
    reappears_after_days: 30,
    shows: ReminderShows::WhenRaised,
};

/// This module's Reminders.
pub const REMINDERS: &[Reminder] = &[AD_DISCLOSURE];
