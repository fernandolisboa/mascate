//! Integrations: one adapter per Platform, publishing normalized data to the other modules.

mod amazon;
mod amazon_earnings;
mod answers;
mod anthropic;
mod connections;
mod eduzz;
mod hotmart;
mod kiwify;
mod link_tags;
mod mercado_livre;
mod monetizze;
mod retry;
mod shopee_affiliate;
mod shopee_seller;
mod spreadsheet;
mod telegram;

pub use amazon::{API_URL as AMAZON_API_URL, AUTH_URL as AMAZON_AUTH_URL, Amazon};
pub use amazon_earnings::{
    EarningsImport, EarningsReport, LINKS as AMAZON_LINKS, PLATFORM as AMAZON_PLATFORM,
    ReportError, SkippedRow, read_earnings_report,
};
pub use anthropic::{API_URL as ANTHROPIC_API_URL, Anthropic};
pub use connections::{
    Connection, ConnectionError, ConnectionState, Connections, Credential, CredentialEntry,
};
pub use eduzz::{API_URL as EDUZZ_API_URL, Eduzz, LINKS as EDUZZ_LINKS};
pub use hotmart::{
    API_URL as HOTMART_API_URL, AUTH_URL as HOTMART_AUTH_URL, Hotmart, LINKS as HOTMART_LINKS,
};
pub use kiwify::{API_URL as KIWIFY_API_URL, Kiwify, LINKS as KIWIFY_LINKS};
pub use link_tags::UrlTagger;
pub use mercado_livre::{
    API_URL as MERCADO_LIVRE_API_URL, MercadoLivre, PAYMENTS_API_URL as MERCADO_PAGO_API_URL,
};
pub use monetizze::{API_URL as MONETIZZE_API_URL, LINKS as MONETIZZE_LINKS, Monetizze};
pub use retry::{Pause, ThreadPause};
pub use shopee_affiliate::{API_URL as SHOPEE_AFFILIATE_API_URL, ShopeeAffiliates};
pub use shopee_seller::{
    API_URL as SHOPEE_SELLER_API_URL, SANDBOX_URL as SHOPEE_SELLER_SANDBOX_URL, ShopAuthorization,
    ShopeeSeller,
};
pub use telegram::{
    API_URL as TELEGRAM_API_URL, MAX_PHOTO_BYTES as TELEGRAM_MAX_PHOTO_BYTES, Telegram,
};

use mascate_platform::{Reminder, ReminderShows, ReminderTopic};

/// Raised while the Amazon Connection holds its keys.
pub const AMAZON_CLIENT_SECRET: Reminder = Reminder {
    key: "integrations.amazon_client_secret",
    topic: ReminderTopic::Connection,
    title: "Client secret da Amazon",
    text: "O client secret do app na Amazon vence a cada 180 dias. Quando a Amazon avisar, gere \
           um novo no Solution Provider Portal e cole em Configurações › Conexões › Amazon; \
           vencido, a Amazon para de responder ao app até a troca.",
    reappears_after_days: 60,
    shows: ReminderShows::WhenRaised,
};

/// Raised while the Shopee seller Connection holds any of its keys.
pub const SHOPEE_SELLER_CNPJ: Reminder = Reminder {
    key: "integrations.shopee_seller_cnpj",
    topic: ReminderTopic::Connection,
    title: "CNPJ na Shopee",
    text: "A Shopee pede CNPJ para registrar o app de vendedor na Open Platform e para a loja \
           usar a API. O app funciona com as chaves que você tiver; quando abrir o CNPJ, \
           confira o cadastro da loja e do app na Shopee.",
    reappears_after_days: 60,
    shows: ReminderShows::WhenRaised,
};

/// The Reminders of the Platform accounts the adapters reach.
pub const REMINDERS: &[Reminder] = &[AMAZON_CLIENT_SECRET, SHOPEE_SELLER_CNPJ];
