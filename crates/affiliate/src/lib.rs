//! Affiliate: Programs, Affiliate Links, Conversions, Commissions, Payouts
//! and Prospects.

mod affiliate_links;
mod clicks;
mod commissions;
mod links;
mod merge;
mod payouts;
mod prospect_score;
mod prospect_signals;
mod prospect_source;
mod prospects;
mod provisional_programs;
mod report;
mod rules;
mod short_links;
mod source;
mod spending;
mod tagging;
mod totals;

pub use affiliate_links::{AffiliateLink, CreatedLink, NewAffiliateLink, TAG_MAX_CHARS};
pub use clicks::{CLICKS_SETTLE_DAYS, ClickGroup, ClickReport, ClicksRead, DayClicks, LinkClicks};
pub use commissions::{
    Affiliate, AffiliateError, AffiliateSync, Commission, CommissionFilter, INCOME_WINDOW_DAYS,
    LastSync, ManualCommission, Reading, Recorded, StatusChange,
};
pub use links::PromotedProduct;
pub use merge::MERGE;
pub use payouts::{ManualPayout, Payout, PayoutProposal};
pub use prospect_score::{
    BET_MAX_AGE_DAYS, BET_MAX_TEMPERATURE, BET_MIN_BLUEPRINT, BET_MIN_RATING, BET_MIN_REVIEWS,
    Score, ScoreArea, ScorePart, is_bet, score,
};
pub use prospect_signals::{
    Affiliation, MAX_TEMPERATURE, ProductFormat, SignalKind, SignalOrigin, SignalReading, Signals,
    Support,
};
pub use prospect_source::{FoundProspect, ProspectCategory, ProspectSource};
pub use prospects::{Prospect, ProspectsRead, prospect_key_of};
pub use provisional_programs::{NewProgram, ProgramJoin};
pub use report::{CommissionGroup, CommissionReport, GroupOrder, ProgramRef};
pub use rules::{
    AMAZON_TYPED_PRICE, BRAND_KEYWORDS, Practice, Program, Promotion, PromotionRules, Reach,
    RuleWarning, SHOPEE_PAID_SEARCH, ShownPrice, Standing,
};
pub use short_links::{
    AMAZON_SHORT_LINK, LinkShortener, MERCADO_LIVRE_SHORT_LINK, SHOPEE_SHORT_LINK, Shortened,
    ShortenedLink, short_link_flag,
};
pub use source::{
    AffiliatePlatform, CommissionSource, CommissionStatus, ReportedConversion, ReportedPayout,
    ReportedProgram,
};
pub use spending::spends_on;
pub use tagging::{LinkTagger, TagRefusal};
pub use totals::{CurrencyTotals, totals_by_currency};

use mascate_platform::{Flag, ModuleMigrations, Reminder, ReminderShows, ReminderTopic};

/// This module's own tables.
pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "affiliate",
    migrations: &[
        commissions::CREATE_COMMISSIONS,
        payouts::CREATE_PAYOUTS,
        links::CREATE_PRODUCT_LINKS,
        rules::ADD_PROMOTION_RULES,
        affiliate_links::CREATE_AFFILIATE_LINKS,
        short_links::CREATE_SHORT_LINKS,
        clicks::CREATE_LINK_CLICKS,
        provisional_programs::ADD_PROVISIONAL_PROGRAMS,
        prospects::CREATE_PROSPECTS,
    ],
};

/// This module's flags: the phase-2 Restricted Features (ADR 0031) and the
/// Short Links the terms forbid (ADR 0035).
pub const FLAGS: &[Flag] = &[
    SHOPEE_PAID_SEARCH,
    BRAND_KEYWORDS,
    AMAZON_TYPED_PRICE,
    AMAZON_SHORT_LINK,
    SHOPEE_SHORT_LINK,
    MERCADO_LIVRE_SHORT_LINK,
];

pub const COMMISSION_INCOME: Reminder = Reminder {
    key: "affiliate.commission_income",
    topic: ReminderTopic::Fiscal,
    title: "Comissões de afiliado são renda tributável",
    text: "Há comissão aprovada nos últimos 12 meses. Comissão de afiliado entra na declaração \
           de imposto de renda e, com frequência, no carnê-leão; vale conferir com um contador. \
           O app continua funcionando do mesmo jeito.",
    reappears_after_days: 30,
    shows: ReminderShows::WhenRaised,
};

/// This module's Reminders.
pub const REMINDERS: &[Reminder] = &[COMMISSION_INCOME];
