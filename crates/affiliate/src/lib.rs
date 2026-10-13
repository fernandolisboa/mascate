//! Affiliate: Programs, Affiliate Links, Conversions, Commissions and Payouts.

mod commissions;
mod source;
mod totals;

pub use commissions::{
    Affiliate, AffiliateError, AffiliateSync, Commission, CommissionFilter, INCOME_WINDOW_DAYS,
    LastSync, ManualCommission, Recorded, StatusChange,
};
pub use source::{
    AffiliatePlatform, CommissionSource, CommissionStatus, ReportedConversion, ReportedPayout,
    ReportedProgram,
};
pub use totals::{CurrencyTotals, totals_by_currency};

use mascate_platform::{ModuleMigrations, Reminder, ReminderShows, ReminderTopic};

/// This module's own tables.
pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "affiliate",
    migrations: &[commissions::CREATE_COMMISSIONS],
};

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
