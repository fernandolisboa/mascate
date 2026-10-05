//! Integrations: one adapter per Platform, publishing normalized data to the other modules.

mod connections;
mod mercado_livre;

pub use connections::{
    Connection, ConnectionError, ConnectionState, Connections, Credential, CredentialEntry,
};
pub use mercado_livre::{
    API_URL as MERCADO_LIVRE_API_URL, MercadoLivre, PAYMENTS_API_URL as MERCADO_PAGO_API_URL,
};
