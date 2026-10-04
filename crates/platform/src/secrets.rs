//! Where Connection secrets live: the system credential store (ADR 0003),
//! never the database and never a Backup. Secrets are named after the
//! variables in `docs/credentials.html`.

use std::fmt;
use std::sync::Arc;

/// A secret's value. Its `Debug` output hides the value, so a secret that
/// ends up in a log or an error message shows as `Secret(..)`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the system credential store failed: {0}")]
pub struct SecretStoreError(pub String);

/// Keeps secrets by name.
pub trait SecretStore: Send + Sync {
    fn read(&self, name: &str) -> Result<Option<Secret>, SecretStoreError>;
    fn write(&self, name: &str, secret: &Secret) -> Result<(), SecretStoreError>;
    /// Removing a secret that is not there succeeds.
    fn remove(&self, name: &str) -> Result<(), SecretStoreError>;
}

/// Which kind of build is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Build {
    Development,
    Release,
}

impl Build {
    pub const CURRENT: Build = if cfg!(debug_assertions) {
        Build::Development
    } else {
        Build::Release
    };
}

/// Reads an environment variable by name. Injected, so tests never touch the
/// process environment.
pub type Environment = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The environment of this process.
pub fn process_environment() -> Environment {
    Box::new(|name| std::env::var(name).ok())
}

/// The secret store the app uses: `system`, except that a development build
/// reads an environment variable named like a secret first. A release build
/// never looks at the environment.
pub fn secret_store_for(
    build: Build,
    system: Arc<dyn SecretStore>,
    environment: Environment,
) -> Arc<dyn SecretStore> {
    match build {
        Build::Development => Arc::new(EnvironmentFirst {
            system,
            environment,
        }),
        Build::Release => system,
    }
}

/// Reads from the environment before the store; writes and removals go to
/// the store, so a variable keeps winning until it is unset.
struct EnvironmentFirst {
    system: Arc<dyn SecretStore>,
    environment: Environment,
}

impl SecretStore for EnvironmentFirst {
    fn read(&self, name: &str) -> Result<Option<Secret>, SecretStoreError> {
        match (self.environment)(name) {
            Some(value) if !value.trim().is_empty() => Ok(Some(Secret::new(value.trim()))),
            _ => self.system.read(name),
        }
    }

    fn write(&self, name: &str, secret: &Secret) -> Result<(), SecretStoreError> {
        self.system.write(name, secret)
    }

    fn remove(&self, name: &str) -> Result<(), SecretStoreError> {
        self.system.remove(name)
    }
}
