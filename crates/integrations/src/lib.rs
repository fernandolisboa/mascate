//! Integrations: one adapter per Platform, publishing normalized data to the other modules.

mod amazon_earnings;
mod answers;
mod anthropic;
mod connections;
mod hotmart;
mod kiwify;
mod mercado_livre;
mod monetizze;
mod retry;
mod shopee_affiliate;
mod spreadsheet;

pub use amazon_earnings::{
    EarningsImport, EarningsReport, PLATFORM as AMAZON_PLATFORM, ReportError, SkippedRow,
    read_earnings_report,
};
pub use anthropic::{API_URL as ANTHROPIC_API_URL, Anthropic};
pub use connections::{
    Connection, ConnectionError, ConnectionState, Connections, Credential, CredentialEntry,
};
pub use hotmart::{API_URL as HOTMART_API_URL, AUTH_URL as HOTMART_AUTH_URL, Hotmart};
pub use kiwify::{API_URL as KIWIFY_API_URL, Kiwify};
pub use mercado_livre::{
    API_URL as MERCADO_LIVRE_API_URL, MercadoLivre, PAYMENTS_API_URL as MERCADO_PAGO_API_URL,
};
pub use monetizze::{API_URL as MONETIZZE_API_URL, Monetizze};
pub use retry::{Pause, ThreadPause};
pub use shopee_affiliate::{API_URL as SHOPEE_AFFILIATE_API_URL, ShopeeAffiliates};
