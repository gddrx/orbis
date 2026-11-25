//! Secret storage for the session cookies.
//!
//! On the desktop the secrets go to the Secret Service (on KDE Plasma it is
//! served by KWallet, on GNOME by GNOME Keyring); inside a Flatpak sandbox
//! `oo7` switches to its encrypted file backend behind the secret portal.
//! Nothing secret is written to plain files.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

/// The application attribute every orbis item carries.
pub const APPLICATION: &str = "orbis";

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// No secret storage could be reached (no Secret Service on the bus).
    #[error("secret storage is unavailable: {0}")]
    Unavailable(String),
    /// The storage refused or failed an operation (locked, dismissed prompt…).
    #[error("secret storage failed: {0}")]
    Backend(String),
}

/// Stores small secrets under a key.
pub trait SecretStore {
    /// The secret under `key`, or `None` when there is none.
    fn load(&self, key: &str) -> impl Future<Output = Result<Option<Vec<u8>>, SecretError>> + Send;

    /// Saves `secret` under `key`, replacing an earlier one. `label` is what
    /// the user sees in the wallet manager.
    fn save(
        &self,
        key: &str,
        label: &str,
        secret: &[u8],
    ) -> impl Future<Output = Result<(), SecretError>> + Send;

    /// Removes the secret under `key`; removing a missing one is not an error.
    fn delete(&self, key: &str) -> impl Future<Output = Result<(), SecretError>> + Send;
}

/// A store in memory, for tests and for running without a wallet.
#[derive(Debug, Default)]
pub struct MemoryStore {
    items: Mutex<HashMap<String, Vec<u8>>>,
}

impl SecretStore for MemoryStore {
    fn load(&self, key: &str) -> impl Future<Output = Result<Option<Vec<u8>>, SecretError>> + Send {
        let value = self.items.lock().expect("store lock").get(key).cloned();
        async move { Ok(value) }
    }

    fn save(
        &self,
        key: &str,
        _label: &str,
        secret: &[u8],
    ) -> impl Future<Output = Result<(), SecretError>> + Send {
        self.items
            .lock()
            .expect("store lock")
            .insert(key.to_owned(), secret.to_vec());
        async { Ok(()) }
    }

    fn delete(&self, key: &str) -> impl Future<Output = Result<(), SecretError>> + Send {
        self.items.lock().expect("store lock").remove(key);
        async { Ok(()) }
    }
}

/// The desktop's Secret Service, through `oo7`.
#[derive(Debug)]
pub struct SecretServiceStore {
    keyring: oo7::Keyring,
}

impl SecretServiceStore {
    /// Connects to the Secret Service (or the secret portal in a sandbox).
    pub async fn connect() -> Result<Self, SecretError> {
        let keyring = oo7::Keyring::new()
            .await
            .map_err(|e| SecretError::Unavailable(e.to_string()))?;
        Ok(Self { keyring })
    }

    fn attributes(key: &str) -> [(&'static str, String); 2] {
        [
            ("application", APPLICATION.to_owned()),
            ("key", key.to_owned()),
        ]
    }
}

fn backend(e: oo7::Error) -> SecretError {
    SecretError::Backend(e.to_string())
}

impl SecretStore for SecretServiceStore {
    fn load(&self, key: &str) -> impl Future<Output = Result<Option<Vec<u8>>, SecretError>> + Send {
        let attributes = Self::attributes(key);
        async move {
            let items = self
                .keyring
                .search_items(&attributes)
                .await
                .map_err(backend)?;
            let Some(item) = items.first() else {
                return Ok(None);
            };
            if item.is_locked().await.map_err(backend)? {
                item.unlock().await.map_err(backend)?;
            }
            let secret = item.secret().await.map_err(backend)?;
            Ok(Some(secret.as_bytes().to_vec()))
        }
    }

    fn save(
        &self,
        key: &str,
        label: &str,
        secret: &[u8],
    ) -> impl Future<Output = Result<(), SecretError>> + Send {
        let attributes = Self::attributes(key);
        let label = label.to_owned();
        let secret = secret.to_vec();
        async move {
            if self.keyring.is_locked().await.map_err(backend)? {
                self.keyring.unlock().await.map_err(backend)?;
            }
            self.keyring
                .create_item(&label, &attributes, secret, true)
                .await
                .map_err(backend)
        }
    }

    fn delete(&self, key: &str) -> impl Future<Output = Result<(), SecretError>> + Send {
        let attributes = Self::attributes(key);
        async move { self.keyring.delete(&attributes).await.map_err(backend) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn roundtrip(store: &impl SecretStore, key: &str) {
        store.delete(key).await.unwrap();
        assert_eq!(store.load(key).await.unwrap(), None);
        store.save(key, "orbis test item", b"first").await.unwrap();
        store.save(key, "orbis test item", b"second").await.unwrap();
        assert_eq!(
            store.load(key).await.unwrap().as_deref(),
            Some(&b"second"[..])
        );
        store.delete(key).await.unwrap();
        assert_eq!(store.load(key).await.unwrap(), None);
    }

    #[tokio::test]
    async fn memory_store_roundtrip() {
        roundtrip(&MemoryStore::default(), "session").await;
    }

    /// Writes, reads and deletes a test item in the real wallet. Run by hand:
    /// `cargo test -p desktop -- --ignored secret_service`.
    #[tokio::test]
    #[ignore = "touches the user's wallet"]
    async fn secret_service_roundtrip() {
        let store = SecretServiceStore::connect().await.unwrap();
        roundtrip(&store, "selftest").await;
    }
}
