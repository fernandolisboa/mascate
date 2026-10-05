//! Integrations: one adapter per Platform, publishing normalized data to the other modules.

mod answers;
mod connections;
mod mercado_livre;
mod retry;
mod shopee_affiliate;

pub use connections::{
    Connection, ConnectionError, ConnectionState, Connections, Credential, CredentialEntry,
};
pub use mercado_livre::{
    API_URL as MERCADO_LIVRE_API_URL, MercadoLivre, PAYMENTS_API_URL as MERCADO_PAGO_API_URL,
};
pub use retry::{Pause, ThreadPause};
pub use shopee_affiliate::{API_URL as SHOPEE_AFFILIATE_API_URL, ShopeeAffiliates};
