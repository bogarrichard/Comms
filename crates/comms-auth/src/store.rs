use std::fmt;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AuthError;

/// What is persisted between runs: the refresh token and the permission set
/// it was issued for. Access tokens are never persisted.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredSession {
    pub refresh_token: String,
    /// Normalised, sorted scope list requested at sign-in.
    pub scopes: String,
}

impl fmt::Debug for StoredSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredSession")
            .field("refresh_token", &"<redacted>")
            .field("scopes", &self.scopes)
            .finish()
    }
}

#[async_trait]
pub trait TokenStore: Send + Sync {
    async fn load(&self) -> Result<Option<StoredSession>, AuthError>;
    async fn save(&self, session: &StoredSession) -> Result<(), AuthError>;
    async fn delete(&self) -> Result<(), AuthError>;
}

/// Non-persistent store for tests and `--demo`-style runs.
#[derive(Default)]
pub struct MemoryStore(Mutex<Option<StoredSession>>);

#[async_trait]
impl TokenStore for MemoryStore {
    async fn load(&self) -> Result<Option<StoredSession>, AuthError> {
        Ok(self.0.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }
    async fn save(&self, session: &StoredSession) -> Result<(), AuthError> {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(session.clone());
        Ok(())
    }
    async fn delete(&self) -> Result<(), AuthError> {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
    }
}

const SERVICE: &str = "comms";

/// OS credential store: Secret Service (Linux), Credential Manager (Windows),
/// Keychain (macOS). Fails closed: if no secure store exists, sign-in fails
/// with an explanation instead of falling back to a file.
pub struct KeyringStore {
    user: String,
}

impl KeyringStore {
    pub fn new(account_key: &str) -> Result<Self, AuthError> {
        init_platform_store()?;
        Ok(Self {
            user: account_key.to_owned(),
        })
    }

    async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(keyring_core::Entry) -> Result<T, AuthError> + Send + 'static,
    ) -> Result<T, AuthError> {
        let user = self.user.clone();
        tokio::task::spawn_blocking(move || {
            let entry = keyring_core::Entry::new(SERVICE, &user).map_err(map_keyring_error)?;
            f(entry)
        })
        .await
        .map_err(|e| AuthError::Store(e.to_string()))?
    }
}

fn map_keyring_error(e: keyring_core::Error) -> AuthError {
    match e {
        keyring_core::Error::NoStorageAccess(inner) => AuthError::NoSecureStore(inner.to_string()),
        keyring_core::Error::PlatformFailure(inner) => AuthError::NoSecureStore(inner.to_string()),
        other => AuthError::Store(other.to_string()),
    }
}

#[async_trait]
impl TokenStore for KeyringStore {
    async fn load(&self) -> Result<Option<StoredSession>, AuthError> {
        self.blocking(|entry| match entry.get_password() {
            Ok(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|_| AuthError::Store("stored credential is corrupt".into())),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_keyring_error(e)),
        })
        .await
    }

    async fn save(&self, session: &StoredSession) -> Result<(), AuthError> {
        let json = serde_json::to_string(session).map_err(|e| AuthError::Store(e.to_string()))?;
        self.blocking(move |entry| entry.set_password(&json).map_err(map_keyring_error))
            .await
    }

    async fn delete(&self) -> Result<(), AuthError> {
        self.blocking(|entry| match entry.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_keyring_error(e)),
        })
        .await
    }
}

fn init_platform_store() -> Result<(), AuthError> {
    use std::sync::OnceLock;
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| {
        #[cfg(target_os = "linux")]
        {
            zbus_secret_service_keyring_store::Store::new()
                .map(|s| keyring_core::set_default_store(s))
                .map_err(|e| e.to_string())
        }
        #[cfg(target_os = "windows")]
        {
            windows_native_keyring_store::Store::new()
                .map(|s| keyring_core::set_default_store(s))
                .map_err(|e| e.to_string())
        }
        #[cfg(target_os = "macos")]
        {
            apple_native_keyring_store::keychain::Store::new()
                .map(|s| keyring_core::set_default_store(s))
                .map_err(|e| e.to_string())
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        {
            Err("unsupported platform".to_owned())
        }
    })
    .clone()
    .map_err(AuthError::NoSecureStore)
}
