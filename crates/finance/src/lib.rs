//! Finance & Reporting: revenue, Fees, margins and the dashboard.

mod margins;
mod releases;
mod tax;
mod volume;

pub use margins::{MarginOrder, MarginReport, MarginRow, SoldLine, SoldProduct};
pub use releases::{MoneyRelease, ReleaseSummary};
pub use tax::{TaxError, Taxes};
pub use volume::{SALES_VOLUME, SalesVolume, VOLUME_WINDOW_DAYS, VolumeCheck, VolumeError};

use mascate_platform::{ModuleMigrations, Reminder, ReminderShows, ReminderTopic};

/// This module's own tables.
pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "finance",
    migrations: &[tax::CREATE_TAX_SETTINGS, volume::CREATE_VOLUME_SETTINGS],
};

pub const SELLING_WITHOUT_CNPJ: Reminder = Reminder {
    key: "finance.selling_without_cnpj",
    topic: ReminderTopic::Fiscal,
    title: "Vendas sem CNPJ e sem NF-e",
    text: "Vender com frequência como pessoa física pode gerar questionamento fiscal, e o \
           Mercado Envios Full e Flex exigem NF-e. Enquanto não houver CNPJ, o app conta com \
           envio por coleta ou agência.",
    reappears_after_days: 30,
    shows: ReminderShows::Always,
};

/// This module's Reminders.
pub const REMINDERS: &[Reminder] = &[SELLING_WITHOUT_CNPJ, SALES_VOLUME];
