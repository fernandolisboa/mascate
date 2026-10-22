//! Integrations: one adapter per Platform, publishing normalized data to the other modules.

mod affiliate_panel;
mod amazon;
mod amazon_crawler;
mod amazon_earnings;
mod answers;
mod anthropic;
mod base64;
mod browser;
mod connections;
mod crawler;
mod eduzz;
mod escaped;
mod hotmart;
mod hotmart_crawler;
mod instagram;
mod kiwify;
mod landing;
mod link_tags;
mod magalu;
mod magalu_affiliate_panel;
mod magalu_crawler;
mod mercado_livre;
mod mercado_livre_affiliate_panel;
mod mercado_livre_crawler;
mod monetizze;
mod pictures;
mod pinterest;
mod posting;
mod retry;
mod shopee_affiliate;
mod shopee_seller;
mod short_io;
mod single_use_tokens;
mod spreadsheet;
mod telegram;

pub use affiliate_panel::{AffiliatePanel, PanelState, TABLES_SCRIPT};
pub use amazon::{API_URL as AMAZON_API_URL, AUTH_URL as AMAZON_AUTH_URL, Amazon};
pub use amazon_crawler::{AMAZON_CRAWLER, AmazonCrawler, SEARCH_URL as AMAZON_SEARCH_URL};
pub use amazon_earnings::{
    EarningsImport, EarningsReport, LINKS as AMAZON_LINKS, PLATFORM as AMAZON_PLATFORM,
    ReportError, SkippedRow, read_earnings_report,
};
pub use anthropic::{API_URL as ANTHROPIC_API_URL, Anthropic};
pub use browser::{BrowserSession, installed_browser};
pub use connections::{
    Connection, ConnectionError, ConnectionState, Connections, Credential, CredentialEntry,
};
pub use crawler::Crawler;
pub use eduzz::{API_URL as EDUZZ_API_URL, Eduzz, LINKS as EDUZZ_LINKS};
pub use hotmart::{
    API_URL as HOTMART_API_URL, AUTH_URL as HOTMART_AUTH_URL, Hotmart, LINKS as HOTMART_LINKS,
};
pub use hotmart_crawler::{
    HOTMART_CATEGORIES, HOTMART_CRAWLER, HotmartCrawler,
    MARKETPLACE_URL as HOTMART_MARKETPLACE_URL, MarketplaceCategory,
};
pub use instagram::{
    API_URL as INSTAGRAM_API_URL, Instagram, InstagramAccount, TOKEN_DAYS as INSTAGRAM_TOKEN_DAYS,
};
pub use kiwify::{API_URL as KIWIFY_API_URL, Kiwify, LINKS as KIWIFY_LINKS};
pub use link_tags::UrlTagger;
pub use magalu::{
    API_URL as MAGALU_API_URL, ID_URL as MAGALU_ID_URL, Magalu, SANDBOX_URL as MAGALU_SANDBOX_URL,
    SCOPES as MAGALU_SCOPES, StoreAuthorization,
};
pub use magalu_affiliate_panel::{
    MAGALU_AFFILIATE_PANEL, MagaluAffiliatePanel, PANEL_URL as MAGALU_AFFILIATE_PANEL_URL,
};
pub use magalu_crawler::{MAGALU_CRAWLER, MagaluCrawler, SEARCH_URL as MAGALU_SEARCH_URL};
pub use mercado_livre::{
    API_URL as MERCADO_LIVRE_API_URL, MercadoLivre, PAYMENTS_API_URL as MERCADO_PAGO_API_URL,
};
pub use mercado_livre_affiliate_panel::{
    MERCADO_LIVRE_AFFILIATE_PANEL, MercadoLivreAffiliatePanel,
    PANEL_URL as MERCADO_LIVRE_AFFILIATE_PANEL_URL,
};
pub use mercado_livre_crawler::{
    LISTING_URL as MERCADO_LIVRE_LISTING_URL, MERCADO_LIVRE_CRAWLER, MercadoLivreCrawler,
};
pub use monetizze::{API_URL as MONETIZZE_API_URL, LINKS as MONETIZZE_LINKS, Monetizze};
pub use pinterest::{
    API_URL as PINTEREST_API_URL, LOGIN_URL as PINTEREST_LOGIN_URL,
    MAX_PHOTO_BYTES as PINTEREST_MAX_PHOTO_BYTES, Pinterest, PinterestAuthorization,
    REDIRECT as PINTEREST_REDIRECT, SANDBOX_URL as PINTEREST_SANDBOX_URL,
    SCOPES as PINTEREST_SCOPES,
};
pub use retry::{Pause, ThreadPause};
pub use shopee_affiliate::{API_URL as SHOPEE_AFFILIATE_API_URL, ShopeeAffiliates};
pub use shopee_seller::{
    API_URL as SHOPEE_SELLER_API_URL, SANDBOX_URL as SHOPEE_SELLER_SANDBOX_URL, ShopAuthorization,
    ShopeeSeller,
};
pub use short_io::{
    API_URL as SHORT_IO_API_URL, STATISTICS_URL as SHORT_IO_STATISTICS_URL, ShortIo,
};
pub use telegram::{
    API_URL as TELEGRAM_API_URL, MAX_PHOTO_BYTES as TELEGRAM_MAX_PHOTO_BYTES, Telegram,
};

use mascate_platform::{Flag, Reminder, ReminderShows, ReminderTopic};

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

/// Raised while the Magalu Connection holds any of its keys.
pub const MAGALU_COMPANY_STORE: Reminder = Reminder {
    key: "integrations.magalu_company_store",
    topic: ReminderTopic::Connection,
    title: "Loja PJ na Magalu",
    text: "A Magalu vende só por loja de pessoa jurídica: o Parceiro Magalu pede CNPJ e a NF-e \
           de cada venda. O app funciona com as chaves que você tiver; quando abrir o CNPJ, \
           confira o cadastro da loja na Magalu.",
    reappears_after_days: 60,
    shows: ReminderShows::WhenRaised,
};

/// The Reminders of the Platform accounts the adapters reach.
pub const REMINDERS: &[Reminder] = &[
    AMAZON_CLIENT_SECRET,
    SHOPEE_SELLER_CNPJ,
    MAGALU_COMPANY_STORE,
];

/// The Restricted Features of the adapters, which know each Platform's
/// terms (ADR 0035).
pub const FLAGS: &[Flag] = &[
    MERCADO_LIVRE_CRAWLER,
    AMAZON_CRAWLER,
    MAGALU_CRAWLER,
    HOTMART_CRAWLER,
    MERCADO_LIVRE_AFFILIATE_PANEL,
    MAGALU_AFFILIATE_PANEL,
];
