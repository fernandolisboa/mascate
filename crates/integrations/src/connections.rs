//! Connections: the owner's credentials for each Platform, kept in the
//! system secret store, and the state each Connection is in.

use std::collections::HashSet;
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
    /// The owner's affiliate Commissions on Hotmart.
    Hotmart,
    /// The owner's affiliate Commissions on Monetizze.
    Monetizze,
    /// The owner's affiliate Commissions and withdrawals on Kiwify.
    Kiwify,
    /// The owner's affiliate Commissions and transfers on Eduzz.
    Eduzz,
    /// The bot that publishes Offer Posts on the owner's Telegram channels.
    Telegram,
    /// The owner's Amazon seller account, a Sales Channel.
    Amazon,
    /// The owner's Shopee shop, a Sales Channel; another API than Shopee
    /// Afiliados, with other keys.
    ShopeeSeller,
    /// The owner's Magalu store, a Sales Channel.
    Magalu,
    /// The owner's Pinterest account, where Offer Posts go as Pins.
    Pinterest,
    /// The owner's professional Instagram account, where Offer Posts go as
    /// photo posts.
    Instagram,
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
    pub const ALL: [Connection; 13] = [
        Connection::MercadoLivre,
        Connection::ShopeeAffiliates,
        Connection::Anthropic,
        Connection::Hotmart,
        Connection::Monetizze,
        Connection::Kiwify,
        Connection::Eduzz,
        Connection::Telegram,
        Connection::Amazon,
        Connection::ShopeeSeller,
        Connection::Magalu,
        Connection::Pinterest,
        Connection::Instagram,
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
        const HOTMART: &[Credential] = &[
            pasted("HOTMART_CLIENT_ID"),
            pasted("HOTMART_CLIENT_SECRET"),
            pasted("HOTMART_BASIC"),
        ];
        // The e-mail is not secret: it picks the owner's share among the
        // affiliates a Monetizze sale lists.
        const MONETIZZE: &[Credential] =
            &[pasted("MONETIZZE_CONSUMER_KEY"), pasted("MONETIZZE_EMAIL")];
        const KIWIFY: &[Credential] = &[
            pasted("KIWIFY_CLIENT_ID"),
            pasted("KIWIFY_CLIENT_SECRET"),
            pasted("KIWIFY_ACCOUNT_ID"),
        ];
        const EDUZZ: &[Credential] = &[pasted("EDUZZ_PERSONAL_TOKEN")];
        // The channel's chat is not secret: it lives on each Promotion
        // Channel.
        const TELEGRAM: &[Credential] = &[pasted("TELEGRAM_BOT_TOKEN")];
        // A private app authorizes itself: the Solution Provider Portal
        // shows the refresh token, so nothing comes by login. The seller id
        // is not secret: every listing path names it.
        const AMAZON: &[Credential] = &[
            pasted("AMAZON_LWA_CLIENT_ID"),
            pasted("AMAZON_LWA_CLIENT_SECRET"),
            pasted("AMAZON_REFRESH_TOKEN"),
            pasted("AMAZON_SELLER_ID"),
        ];
        // The partner's keys come from the Open Platform console; the shop's
        // id and its first single-use refresh token, from the owner's login
        // on Shopee's authorization page.
        const SHOPEE_SELLER: &[Credential] = &[
            pasted("SHOPEE_PARTNER_ID"),
            pasted("SHOPEE_PARTNER_KEY"),
            Credential {
                name: "SHOPEE_SHOP_ID",
                entry: CredentialEntry::Login,
            },
            Credential {
                name: "SHOPEE_REFRESH_TOKEN",
                entry: CredentialEntry::Login,
            },
        ];
        // The client's keys come from the `idm` CLI; the store's first
        // refresh token, from the owner's login on ID Magalu's page.
        const MAGALU: &[Credential] = &[
            pasted("MAGALU_CLIENT_ID"),
            pasted("MAGALU_CLIENT_SECRET"),
            Credential {
                name: "MAGALU_REFRESH_TOKEN",
                entry: CredentialEntry::Login,
            },
        ];
        // The app's keys come from Pinterest's developer page; the first
        // refresh token, from the owner's login on Pinterest's page.
        const PINTEREST: &[Credential] = &[
            pasted("PINTEREST_APP_ID"),
            pasted("PINTEREST_APP_SECRET"),
            Credential {
                name: "PINTEREST_REFRESH_TOKEN",
                entry: CredentialEntry::Login,
            },
        ];
        // The Meta app dashboard generates the token: no client secret and
        // no login page. The app renews it and keeps its term beside it.
        const INSTAGRAM: &[Credential] = &[pasted("INSTAGRAM_ACCESS_TOKEN")];
        match self {
            Connection::MercadoLivre => MERCADO_LIVRE,
            Connection::ShopeeAffiliates => SHOPEE_AFFILIATES,
            Connection::Anthropic => ANTHROPIC,
            Connection::Hotmart => HOTMART,
            Connection::Monetizze => MONETIZZE,
            Connection::Kiwify => KIWIFY,
            Connection::Eduzz => EDUZZ,
            Connection::Telegram => TELEGRAM,
            Connection::Amazon => AMAZON,
            Connection::ShopeeSeller => SHOPEE_SELLER,
            Connection::Magalu => MAGALU,
            Connection::Pinterest => PINTEREST,
            Connection::Instagram => INSTAGRAM,
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
    pub fn awaits_approval_without_credentials(self) -> bool {
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
    /// The Connections whose Platform refused their stored credentials,
    /// until new ones are saved.
    expired: Mutex<HashSet<Connection>>,
}

impl Connections {
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self {
            store,
            writing: Mutex::new(()),
            expired: Mutex::new(HashSet::new()),
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
        if present == connection.credentials().len() && self.is_expired(connection) {
            ConnectionState::Expired
        } else if present == connection.credentials().len() {
            ConnectionState::Connected
        } else if present == 0 && connection.awaits_approval_without_credentials() {
            ConnectionState::AwaitingApproval
        } else {
            ConnectionState::NotConfigured
        }
    }

    /// Notes that the Platform refused the Connection's stored credentials,
    /// as its adapter reported: it reads [`ConnectionState::Expired`] until
    /// the owner saves new ones or removes it.
    pub fn mark_expired(&self, connection: Connection) {
        self.expired_set().insert(connection);
    }

    /// Notes that the owner logged in to the Platform again: the
    /// Connection no longer reads [`ConnectionState::Expired`].
    pub fn logged_in(&self, connection: Connection) {
        self.expired_set().remove(&connection);
    }

    fn is_expired(&self, connection: Connection) -> bool {
        self.expired_set().contains(&connection)
    }

    fn expired_set(&self) -> std::sync::MutexGuard<'_, HashSet<Connection>> {
        self.expired.lock().unwrap_or_else(PoisonError::into_inner)
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
                self.expired_set().remove(&connection);
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
        self.expired_set().remove(&connection);
        Ok(())
    }
}
