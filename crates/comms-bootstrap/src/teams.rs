//! The Microsoft Teams backend (Graph) and its sign-in. Compiled with the `teams` feature.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use comms_app::AppHandle;
use comms_auth::{AuthConfig, AuthError, DeviceCodePrompt, KeyringStore, Session};
use comms_provider_api::ProviderError;
use comms_provider_msgraph::{AccessTokenSource, MsGraphProvider};
use comms_storage::Store;

use crate::{ChannelAccess, Paths, Settings, core_config};

/// Adapts the auth session to what the Graph client needs.
struct SessionTokens(Arc<Session>);

#[async_trait]
impl AccessTokenSource for SessionTokens {
    async fn access_token(&self) -> Result<String, ProviderError> {
        self.0.access_token().await.map_err(|e| match e {
            AuthError::Network(message) => ProviderError::Network {
                may_have_reached_server: false,
                message,
            },
            _ => ProviderError::Unauthorized,
        })
    }

    async fn invalidate(&self) {
        self.0.invalidate().await;
    }
}

/// Build an authentication session from the settings (does not sign in).
pub fn make_session(settings: &Settings) -> Result<Arc<Session>> {
    let Some(client_id) = settings.client_id.clone() else {
        bail!(
            "no client_id configured.\n\
             Register an Entra app (see docs/setup-entra.md) and set `client_id` in {} \
             or the COMMS_CLIENT_ID environment variable. \
             Use `comms --demo` to try the interface without signing in.",
            Paths::discover()?.config_file.display()
        );
    };
    let cfg = AuthConfig::new(client_id, settings.tenant.clone(), settings.scopes());
    let store = KeyringStore::new(&cfg.account_key())?;
    Ok(Arc::new(Session::new(cfg, Arc::new(store))?))
}

/// Sign in interactively. Prints instructions through `say`.
pub async fn login(session: &Session, device_code: bool, say: impl Fn(&str)) -> Result<()> {
    let res = if device_code {
        session
            .login_device_code(|p: &DeviceCodePrompt| {
                say(&format!(
                    "To sign in, open {} and enter the code {}",
                    p.verification_uri, p.user_code
                ));
            })
            .await
    } else {
        session
            .login_browser(|url| {
                if open_in_browser(url) {
                    say("Opening your browser to sign in...");
                } else {
                    say("Open this address in a browser to sign in:");
                }
                say(url);
                say("Waiting for sign-in to complete (Ctrl+C to cancel)...");
            })
            .await
    };
    res.map_err(explain_auth_error)
}

/// Turn an auth failure into an error that includes advice when we have any.
pub fn explain_auth_error(e: AuthError) -> anyhow::Error {
    match e.hint() {
        Some(h) => anyhow::anyhow!("{e}\n\n{h}"),
        None => e.into(),
    }
}

/// Best effort launch of the default browser. Returns whether it was started.
pub fn open_in_browser(url: &str) -> bool {
    use std::process::{Command, Stdio};
    // Only ever hand http(s) URLs to the OS.
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return false;
    }
    #[cfg(target_os = "linux")]
    let cmd = Command::new("xdg-open")
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    #[cfg(target_os = "macos")]
    let cmd = Command::new("open")
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    #[cfg(target_os = "windows")]
    let cmd = Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let cmd: std::io::Result<std::process::Child> = Err(std::io::Error::other("unsupported"));
    cmd.is_ok()
}

/// The real backend. Requires a stored sign-in (see [`login`]).
pub async fn start_graph(settings: &Settings, session: Arc<Session>) -> Result<AppHandle> {
    if !session.is_signed_in().await.map_err(explain_auth_error)? {
        bail!("not signed in. Run `comms login` first.");
    }
    let paths = Paths::discover()?;
    let store = Store::open(&paths.database)
        .await
        .context("opening the local database")?;
    let provider = MsGraphProvider::new(Arc::new(SessionTokens(session)))?
        .with_channels(settings.channels != ChannelAccess::Off);
    Ok(AppHandle::start(Arc::new(provider), store, core_config(settings)).await?)
}

/// `comms login`: build the session from the settings and sign in.
pub async fn teams_login(settings: &Settings, device_code: bool, say: impl Fn(&str)) -> Result<()> {
    let session = make_session(settings)?;
    login(&session, device_code, say).await
}

/// Start the Teams backend, signing in first (interactively) if needed.
pub async fn start_teams(settings: &Settings, say: impl Fn(&str)) -> Result<AppHandle> {
    let session = make_session(settings)?;
    if !session.is_signed_in().await.map_err(explain_auth_error)? {
        say("You are not signed in yet.");
        login(&session, false, &say).await?;
    }
    start_graph(settings, session).await
}

/// Forget the stored sign-in. Returns what was done.
pub(crate) async fn teams_logout(settings: &Settings) -> Result<Option<String>> {
    if settings.client_id.is_none() {
        return Ok(None);
    }
    let session = make_session(settings)?;
    session.logout().await.map_err(explain_auth_error)?;
    Ok(Some(
        "Removed the stored sign-in from the system keyring.".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_client_id_gives_actionable_guidance() {
        let err = make_session(&Settings::default())
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(err.contains("client_id") && err.contains("--demo"), "{err}");
    }
}
