//! Marketing: Listing quality, questions, reputation, Promotions and Ads cost.

mod quality;

pub use quality::{
    ActionKind, ChannelQuality, IMPACT_DAYS, ListedItem, ListingQuality, QualityAction,
    QualityError, QualityLevel, QualityRow, QualitySource, QualitySync, Rating,
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
    migrations: &[quality::CREATE_LISTING_QUALITY],
};
