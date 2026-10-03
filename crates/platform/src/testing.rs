//! A [`SecretStore`] in memory, for other crates' tests.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::{Secret, SecretStore, SecretStoreError};

/// Keeps secrets in a map. [`MemorySecretStore::unavailable`] fails every
/// call, like a system store that is locked or missing.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    secrets: Mutex<BTreeMap<String, Secret>>,
    unavailable: bool,
}

impl MemorySecretStore {
    pub fn unavailable() -> Self {
        Self {
            unavailable: true,
            ..Self::default()
        }
    }

    /// The names it holds, in order.
    pub fn names(&self) -> Vec<String> {
        self.secrets().keys().cloned().collect()
    }

    fn secrets(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Secret>> {
        self.secrets.lock().expect("secret store lock poisoned")
    }

    fn check(&self) -> Result<(), SecretStoreError> {
        if self.unavailable {
            Err(SecretStoreError("no store in this session".into()))
        } else {
            Ok(())
        }
    }
}

impl SecretStore for MemorySecretStore {
    fn read(&self, name: &str) -> Result<Option<Secret>, SecretStoreError> {
        self.check()?;
        Ok(self.secrets().get(name).cloned())
    }

    fn write(&self, name: &str, secret: &Secret) -> Result<(), SecretStoreError> {
        self.check()?;
        self.secrets().insert(name.to_owned(), secret.clone());
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<(), SecretStoreError> {
        self.check()?;
        self.secrets().remove(name);
        Ok(())
    }
}
