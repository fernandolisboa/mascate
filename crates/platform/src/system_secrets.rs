//! The operating system's credential store: Credential Manager on Windows,
//! the Secret Service (GNOME Keyring, KWallet) on Linux.

use std::sync::{Arc, Mutex, PoisonError};

use keyring_core::{CredentialStore, Entry, Error};

use crate::{Secret, SecretStore, SecretStoreError};

/// Every secret is filed under this service, named by its variable.
const SERVICE: &str = "Mascate";

/// Opens the system store on first use and again after a failure, so a
/// keyring daemon that starts after the app, or a locked one, is picked up
/// without restarting.
#[derive(Default)]
pub struct SystemSecretStore {
    store: Mutex<Option<Arc<CredentialStore>>>,
}

impl SystemSecretStore {
    fn entry(&self, name: &str) -> Result<Entry, SecretStoreError> {
        let mut store = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        let store = match store.as_ref() {
            Some(open) => open.clone(),
            None => store.insert(open_system_store().map_err(failure)?).clone(),
        };
        store.build(SERVICE, name, None).map_err(failure)
    }
}

#[cfg(windows)]
fn open_system_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(target_os = "linux")]
fn open_system_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn open_system_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Err(Error::NotSupportedByStore(
        "no system credential store on this platform yet".into(),
    ))
}

fn failure(error: Error) -> SecretStoreError {
    SecretStoreError(error.to_string())
}

impl SecretStore for SystemSecretStore {
    fn read(&self, name: &str) -> Result<Option<Secret>, SecretStoreError> {
        match self.entry(name)?.get_password() {
            Ok(value) => Ok(Some(Secret::new(value))),
            Err(Error::NoEntry) => Ok(None),
            Err(error) => Err(failure(error)),
        }
    }

    fn write(&self, name: &str, secret: &Secret) -> Result<(), SecretStoreError> {
        self.entry(name)?
            .set_password(secret.expose())
            .map_err(failure)
    }

    fn remove(&self, name: &str) -> Result<(), SecretStoreError> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(error) => Err(failure(error)),
        }
    }
}
