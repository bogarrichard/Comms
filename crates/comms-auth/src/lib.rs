//! OAuth 2.0 / OIDC for Microsoft Entra ID as a public client (no secret):
//! authorization code + PKCE through the system browser, device code as the
//! headless alternative, refresh-token rotation, and OS credential storage.
//!
//! Tokens are never logged and never written to disk in plaintext: the only
//! persisted secret is the refresh token, kept in the OS keyring.

mod config;
mod error;
mod flows;
mod session;
mod store;

pub use config::AuthConfig;
pub use error::AuthError;
pub use flows::DeviceCodePrompt;
pub use session::Session;
pub use store::{KeyringStore, MemoryStore, StoredSession, TokenStore};

#[cfg(test)]
mod tests;
