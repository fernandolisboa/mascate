//! Marketing: Listing quality, questions, reputation, Promotions, Ads cost and Listing Copy.

mod ads;
mod copy;
mod quality;
mod questions;
mod replies;
mod reputation;

pub use ads::{
    ADS_HISTORY_DAYS, ADS_REFRESH_MINUTES, ADS_SETTLE_DAYS, AdDay, AdMetrics, AdsError, AdsReport,
    AdsSync, AdsSyncState, AdvertisedListing, CampaignAds, CampaignStatus, ChannelAd, ChannelAds,
    ChannelCampaign, ProductAds, ProductRoas,
};
pub use copy::{
    BriefAttribute, COPY_INSTRUCTIONS, CategoryRules, CopyBrief, CopyChannel, CopyError, CopyField,
    CopyProblem, CopyRequest, CopySettings, CopyWriter, DEFAULT_COPY_MODEL, GeneratedCopy,
    ListingCopy, MODEL_MAX_CHARS, WrittenCopy, check_copy,
};
pub use quality::{
    ActionKind, ChannelQuality, IMPACT_DAYS, ListedItem, ListingQuality, QualityAction,
    QualityError, QualityLevel, QualityRow, QualitySource, QualitySync, Rating,
};
pub use questions::{
    AnswerRequest, ChannelAnswer, ChannelQuestion, ChannelQuestions, ConfirmedAnswer, Question,
    QuestionError, QuestionStatus, QuestionSync, Questions, SentAnswer,
};
pub use replies::{
    ANSWER_MAX_CHARS, Contact, DEADLINE_VARIABLE, MAX_DISPATCH_DAYS, NewReplyTemplate,
    PRODUCT_VARIABLE, QuestionSettings, ReplyProblem, ReplyTemplate, ReplyTemplates,
    TEMPLATE_NAME_MAX_CHARS, check_answer, contact_in,
};
pub use reputation::{
    ArrivedReview, ChannelReputation, ChannelReview, LOW_RATING, ListingReviews, LowReview,
    MetricReading, MetricStanding, PRODUCT_ADS_MIN_SALES, ProductReviews, REFRESH_MINUTES,
    Reputation, ReputationColor, ReputationError, ReputationMetric, ReputationSource,
    ReputationStanding, ReputationSync, ReviewedItem, SellerTool,
};

use mascate_platform::{Flag, FlagKind, ModuleMigrations, Phase};

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
    ],
};
