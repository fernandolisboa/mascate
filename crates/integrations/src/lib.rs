//! Integrations: one adapter per Platform, publishing normalized data to the other modules.

mod connections;

pub use connections::{
    Connection, ConnectionError, ConnectionState, Connections, Credential, CredentialEntry,
};
