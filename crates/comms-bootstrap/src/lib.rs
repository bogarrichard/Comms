//! Composition root: reads settings, opens storage and authentication, and
//! builds the application core. The only crate (besides the adapter itself)
//! that names `comms-provider-msgraph`, so frontends stay provider-agnostic.

mod settings;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use comms_app::{AppHandle, CoreConfig};
use comms_auth::{AuthConfig, AuthError, DeviceCodePrompt, KeyringStore, Session};
use comms_provider_api::ProviderError;
use comms_provider_msgraph::{AccessTokenSource, MsGraphProvider};
use comms_storage::Store;
use comms_testkit::FakeProvider;

pub use settings::{ChannelAccess, Paths, Settings};

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

fn core_config(settings: &Settings) -> CoreConfig {
    CoreConfig {
        initial_messages: settings.initial_messages,
        chat_poll: Duration::from_secs(settings.poll.chats_secs.max(10)),
        channel_poll: Duration::from_secs(settings.poll.channels_secs.max(30)),
        open_poll: Duration::from_secs(settings.poll.open_secs.max(5)),
        ..CoreConfig::default()
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

/// A self-contained demo: fake data, in-memory storage, no network or credentials.
pub async fn start_demo() -> Result<AppHandle> {
    let store = Store::open_in_memory().await?;
    let provider = FakeProvider::demo().with_latency(Duration::from_millis(250));
    let cfg = CoreConfig {
        chat_poll: Duration::from_secs(3600),
        channel_poll: Duration::from_secs(3600),
        open_poll: Duration::from_secs(3600),
        ..CoreConfig::default()
    };
    Ok(AppHandle::start(Arc::new(provider), store, cfg).await?)
}

/// Sign out and optionally delete the local message cache.
pub async fn logout(settings: &Settings, wipe_cache: bool) -> Result<Vec<String>> {
    let mut done = Vec::new();
    if settings.client_id.is_some() {
        let session = make_session(settings)?;
        session.logout().await.map_err(explain_auth_error)?;
        done.push("Removed the stored sign-in from the system keyring.".to_owned());
    }
    if wipe_cache {
        let paths = Paths::discover()?;
        for suffix in ["", "-wal", "-shm"] {
            let mut p: PathBuf = paths.database.clone().into_os_string().into();
            p.as_mut_os_string().push(suffix);
            if p.exists() {
                std::fs::remove_file(&p).with_context(|| format!("deleting {}", p.display()))?;
            }
        }
        done.push(format!(
            "Deleted the local message cache ({}).",
            paths.database.display()
        ));
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use comms_app::ConnectionStatus;

    use super::*;

    #[tokio::test]
    async fn demo_backend_starts_without_credentials_and_lists_conversations() {
        let h = start_demo().await.unwrap();
        let mut rx = h.subscribe();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if h.conversations().await.unwrap().len() >= 7 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "demo data never arrived"
            );
            let _ = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
        }
        assert!(h.capabilities().channels);
        assert_ne!(h.connection_status(), ConnectionStatus::AuthRequired);
    }

    #[test]
    fn missing_client_id_gives_actionable_guidance() {
        let err = make_session(&Settings::default())
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(err.contains("client_id") && err.contains("--demo"), "{err}");
    }
}
