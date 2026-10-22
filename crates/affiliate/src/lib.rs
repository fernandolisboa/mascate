//! Affiliate: Programs, Affiliate Links, Conversions, Commissions and Payouts.

mod affiliate_links;
mod commissions;
mod links;
mod merge;
mod payouts;
mod report;
mod rules;
mod short_links;
mod source;
mod tagging;
mod totals;

pub use affiliate_links::{AffiliateLink, CreatedLink, NewAffiliateLink, TAG_MAX_CHARS};
pub use commissions::{
    Affiliate, AffiliateError, AffiliateSync, Commission, CommissionFilter, INCOME_WINDOW_DAYS,
    LastSync, ManualCommission, Reading, Recorded, StatusChange,
};
pub use links::PromotedProduct;
pub use merge::MERGE;
pub use payouts::{ManualPayout, Payout, PayoutProposal};
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
