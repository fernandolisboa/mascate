//! Connections: the owner's credentials for each Platform, kept in the
//! system secret store, and the state each Connection is in.

use std::sync::{Arc, Mutex, PoisonError};

use mascate_platform::{Secret, SecretStore, SecretStoreError};

/// A Connection the app knows, one per Platform account it talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Connection {
    MercadoLivre,
    /// The Shopee Affiliate Open API, which needs Shopee's approval first.
    ShopeeAffiliates,
    /// Listing Copy drafts through the Claude API.
    Anthropic,
}

/// How a credential reaches the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialEntry {
    /// The owner pastes it in the settings screen.
    Pasted,
    /// The app gets it when the owner logs in to the Platform.
    Login,
}

/// One secret a Connection needs, named after its variable in
/// `docs/credentials.html`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Credential {
    pub name: &'static str,
    pub entry: CredentialEntry,
}

const fn pasted(name: &'static str) -> Credential {
    Credential {
        name,
        entry: CredentialEntry::Pasted,
    }
}

impl Connection {
    pub const ALL: [Connection; 3] = [
        Connection::MercadoLivre,
        Connection::ShopeeAffiliates,
        Connection::Anthropic,
    ];

    /// Everything the Connection needs before it counts as connected.
    pub fn credentials(self) -> &'static [Credential] {
        const MERCADO_LIVRE: &[Credential] = &[
            pasted("ML_CLIENT_ID"),
            pasted("ML_CLIENT_SECRET"),
            // Mercado Livre hands out a new single-use refresh token on every
            // renewal; the seller login stores the first one.
            Credential {
                name: "ML_REFRESH_TOKEN",
                entry: CredentialEntry::Login,
            },
        ];
        const SHOPEE_AFFILIATES: &[Credential] = &[
            pasted("SHOPEE_AFFILIATE_APP_ID"),
            pasted("SHOPEE_AFFILIATE_SECRET"),
        ];
        const ANTHROPIC: &[Credential] = &[pasted("ANTHROPIC_API_KEY")];
        match self {
            Connection::MercadoLivre => MERCADO_LIVRE,
            Connection::ShopeeAffiliates => SHOPEE_AFFILIATES,
            Connection::Anthropic => ANTHROPIC,
        }
    }

    /// The credentials the owner pastes.
    pub fn pasted_credentials(self) -> impl Iterator<Item = Credential> {
        self.credentials()
            .iter()
            .copied()
            .filter(|credential| credential.entry == CredentialEntry::Pasted)
    }

    /// Shopee approves Open API access by hand and it can take weeks, so
    /// having no credentials yet is the normal state, not a missing setup.
    fn awaits_approval_without_credentials(self) -> bool {
        self == Connection::ShopeeAffiliates
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// Some credential is missing.
    NotConfigured,
    /// No credentials yet, waiting on the Platform to grant access.
    AwaitingApproval,
    Connected,
    /// The Platform no longer accepts the login; reported by its adapter.
    Expired,
    /// Something went wrong; `reason` says what.
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectionError {
    #[error("{name} is not a credential pasted into {connection:?}")]
    NotPasted {
        connection: Connection,
        name: String,
    },
    #[error(transparent)]
    Store(#[from] SecretStoreError),
}

/// The Connections and their credentials.
pub struct Connections {
    store: Arc<dyn SecretStore>,
    /// Saves and removals run one at a time, so a removal never interleaves
    /// with a save and leaves half a Connection behind.
    writing: Mutex<()>,
}

impl Connections {
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self {
            store,
            writing: Mutex::new(()),
        }
    }

    pub fn state(&self, connection: Connection) -> ConnectionState {
        let mut present = 0;
        for credential in connection.credentials() {
            match self.store.read(credential.name) {
                Ok(Some(_)) => present += 1,
                Ok(None) => {}
                Err(error) => {
                    return ConnectionState::Failed {
                        reason: error.to_string(),
                    };
                }
            }
        }
        if present == connection.credentials().len() {
            ConnectionState::Connected
        } else if present == 0 && connection.awaits_approval_without_credentials() {
            ConnectionState::AwaitingApproval
        } else {
            ConnectionState::NotConfigured
        }
    }

    /// Whether `name`, one of the Connection's credentials, is stored.
    pub fn has_credential(&self, name: &str) -> Result<bool, ConnectionError> {
        Ok(self.store.read(name)?.is_some())
    }

    /// Stores the pasted `values`, by credential name. Values are trimmed and
    /// a blank one keeps what is stored, so the owner can change one key
    /// without pasting the others again. Nothing is written unless every
    /// name is one of the Connection's pasted credentials.
    pub fn save(
        &self,
        connection: Connection,
        values: &[(&str, &str)],
    ) -> Result<(), ConnectionError> {
        if let Some((name, _)) = values.iter().find(|(name, _)| {
            !connection
                .pasted_credentials()
                .any(|credential| credential.name == *name)
        }) {
            return Err(ConnectionError::NotPasted {
                connection,
                name: (*name).to_owned(),
            });
        }
        let _writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        for (name, value) in values {
            let value = value.trim();
            if !value.is_empty() {
                self.store.write(name, &Secret::new(value))?;
            }
        }
        Ok(())
    }

    /// Forgets every credential of the Connection, the login included.
    pub fn remove(&self, connection: Connection) -> Result<(), ConnectionError> {
        let _writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        for credential in connection.credentials() {
            self.store.remove(credential.name)?;
        }
        Ok(())
    }
}
