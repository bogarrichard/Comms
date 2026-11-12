//! The pieces of an OAuth authorization-code + PKCE sign-in that do not depend
//! on the identity provider: a loopback listener, PKCE values and the state
//! check. Used by the Teams login and by the Slack browser login.

use std::time::Duration;

use tokio::net::TcpListener;

use crate::AuthError;
use crate::flows::{pkce_challenge, random_urlsafe, wait_for_redirect};

/// Fresh PKCE and `state` values for one sign-in attempt.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl Pkce {
    pub fn new() -> Result<Self, AuthError> {
        let verifier = random_urlsafe(48)?;
        Ok(Self {
            challenge: pkce_challenge(&verifier),
            verifier,
            state: random_urlsafe(24)?,
        })
    }
}

/// Listen on `127.0.0.1:port` (the port is part of the registered redirect URL).
pub async fn bind(port: u16) -> Result<TcpListener, AuthError> {
    TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| {
        AuthError::Config(format!(
            "cannot listen on 127.0.0.1:{port} (is another sign-in running?): {e}"
        ))
    })
}

/// Wait for the browser to be redirected back with `?code=...&state=...` and
/// return the code. Requests with a wrong `state` are rejected.
pub async fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    timeout: Duration,
) -> Result<String, AuthError> {
    tokio::time::timeout(timeout, wait_for_redirect(listener, state))
        .await
        .map_err(|_| AuthError::Timeout)?
}
