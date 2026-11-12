//! Composition root: reads settings, opens storage and builds the application
//! core for the chosen backend. The only crate (besides the adapters
//! themselves) that names a provider crate, so frontends stay
//! provider-agnostic.
//!
//! Each backend is a Cargo feature (`teams`, `signal`, `slack`; all on by
//! default). A build without one still understands its settings but refuses to
//! start it with a message saying so.

mod settings;
#[cfg(feature = "signal")]
mod signal;
#[cfg(feature = "slack")]
mod slack;
#[cfg(feature = "teams")]
mod teams;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use comms_app::{AppHandle, CoreConfig};
use comms_storage::Store;
use comms_testkit::FakeProvider;
use std::sync::Arc;

pub use settings::{
    Backend, ChannelAccess, Paths, Settings, SignalSettings, SlackChannels, SlackSettings,
};
#[cfg(feature = "signal")]
pub use signal::{signal_link, start_signal, valid_signal_account};
#[cfg(feature = "slack")]
pub use slack::{save_slack_token, slack_login, slack_workspace, start_slack};
#[cfg(feature = "teams")]
pub use teams::{
    explain_auth_error, login, make_session, open_in_browser, start_graph, start_teams, teams_login,
};

/// What the frontend shows for the workspace: its name and an optional icon
/// (PNG or JPEG bytes).
#[derive(Debug, Clone)]
pub struct WorkspaceInfo {
    pub name: String,
    pub icon: Option<Vec<u8>>,
}

/// Best-effort workspace name and icon for the backend in `settings`. `None`
/// for backends without the notion, and on any failure (a missing `team:read`
/// scope, no network): the UI then keeps its letter tile.
pub async fn workspace_info(settings: &Settings) -> Option<WorkspaceInfo> {
    match settings.provider {
        #[cfg(feature = "slack")]
        Backend::Slack => match slack_workspace(settings).await {
            Ok(ws) => Some(ws),
            Err(e) => {
                tracing::warn!("workspace info unavailable: {e}");
                None
            }
        },
        _ => None,
    }
}

/// Which backends this build contains.
pub fn compiled_backends() -> Vec<Backend> {
    let mut v = Vec::new();
    if cfg!(feature = "teams") {
        v.push(Backend::Teams);
    }
    if cfg!(feature = "signal") {
        v.push(Backend::Signal);
    }
    if cfg!(feature = "slack") {
        v.push(Backend::Slack);
    }
    v
}

// Unused when every backend is compiled in.
#[allow(dead_code)]
fn not_compiled(name: &str, feature: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "this build of comms does not include {name} support \
         (it was built without the `{feature}` feature). Install a build that does."
    )
}

#[cfg_attr(
    not(any(feature = "teams", feature = "signal", feature = "slack")),
    allow(dead_code)
)]
fn core_config(settings: &Settings) -> CoreConfig {
    CoreConfig {
        initial_messages: settings.initial_messages,
        chat_poll: Duration::from_secs(settings.poll.chats_secs.max(10)),
        channel_poll: Duration::from_secs(settings.poll.channels_secs.max(30)),
        open_poll: Duration::from_secs(settings.poll.open_secs.max(5)),
        ..CoreConfig::default()
    }
}

/// Start the backend selected in the settings. `say` receives sign-in prompts
/// for backends that need one (Teams).
pub async fn start(settings: &Settings, say: impl Fn(&str)) -> Result<AppHandle> {
    match settings.provider {
        Backend::Teams => {
            #[cfg(feature = "teams")]
            return start_teams(settings, say).await;
            #[cfg(not(feature = "teams"))]
            return {
                let _ = say;
                Err(not_compiled("Microsoft Teams", "teams"))
            };
        }
        Backend::Signal => {
            #[cfg(feature = "signal")]
            return start_signal(settings).await;
            #[cfg(not(feature = "signal"))]
            return Err(not_compiled("Signal", "signal"));
        }
        Backend::Slack => {
            #[cfg(feature = "slack")]
            return start_slack(settings).await;
            #[cfg(not(feature = "slack"))]
            return Err(not_compiled("Slack", "slack"));
        }
    }
}

/// `comms login` (Teams only).
pub async fn login_command(
    settings: &Settings,
    device_code: bool,
    say: impl Fn(&str),
) -> Result<()> {
    match settings.provider {
        Backend::Teams => {
            #[cfg(feature = "teams")]
            return teams_login(settings, device_code, say).await;
            #[cfg(not(feature = "teams"))]
            return {
                let _ = (device_code, say);
                Err(not_compiled("Microsoft Teams", "teams"))
            };
        }
        Backend::Signal => bail!("Signal does not use `login`; run `comms signal-link` instead."),
        Backend::Slack => bail!(
            "Slack does not use `login`; put your user token in COMMS_SLACK_TOKEN \
             or the [slack] section (see docs/slack.md)."
        ),
    }
}

/// `comms signal-link`.
#[cfg(not(feature = "signal"))]
pub async fn signal_link(_settings: &Settings, _say: impl Fn(&str)) -> Result<()> {
    Err(not_compiled("Signal", "signal"))
}

/// `comms slack-login`: verify a pasted token and store it.
#[cfg(not(feature = "slack"))]
pub async fn slack_login(_token: &str) -> Result<String> {
    Err(not_compiled("Slack", "slack"))
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

/// Like [`start_demo`] but Slack-flavoured: the workspace layout with threads.
pub async fn start_demo_slack() -> Result<AppHandle> {
    let store = Store::open_in_memory().await?;
    let provider = FakeProvider::demo_slack().with_latency(Duration::from_millis(250));
    let cfg = CoreConfig {
        chat_poll: Duration::from_secs(3600),
        channel_poll: Duration::from_secs(3600),
        open_poll: Duration::from_secs(3600),
        ..CoreConfig::default()
    };
    Ok(AppHandle::start(Arc::new(provider), store, cfg).await?)
}

/// Sign out and optionally delete the local message cache.
#[cfg_attr(not(feature = "teams"), allow(unused_variables))]
pub async fn logout(settings: &Settings, wipe_cache: bool) -> Result<Vec<String>> {
    let mut done = Vec::new();
    #[cfg(feature = "teams")]
    if settings.provider == Backend::Teams
        && let Some(line) = teams::teams_logout(settings).await?
    {
        done.push(line);
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
        if paths.signal_inbox.exists() {
            std::fs::remove_file(&paths.signal_inbox)
                .with_context(|| format!("deleting {}", paths.signal_inbox.display()))?;
            done.push("Deleted the stored Signal messages.".to_owned());
        }
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
    fn the_compiled_backends_follow_the_features() {
        let b = compiled_backends();
        assert_eq!(b.contains(&Backend::Teams), cfg!(feature = "teams"));
        assert_eq!(b.contains(&Backend::Signal), cfg!(feature = "signal"));
        assert_eq!(b.contains(&Backend::Slack), cfg!(feature = "slack"));
    }

    #[cfg(not(feature = "slack"))]
    #[tokio::test]
    async fn a_build_without_slack_says_so() {
        let s = Settings {
            provider: Backend::Slack,
            ..Settings::default()
        };
        let err = start(&s, |_| {}).await.map(|_| ()).unwrap_err().to_string();
        assert!(err.contains("`slack` feature"), "{err}");
    }

    #[tokio::test]
    async fn login_is_explained_for_backends_without_one() {
        let s = Settings {
            provider: Backend::Slack,
            ..Settings::default()
        };
        let err = login_command(&s, false, |_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("COMMS_SLACK_TOKEN"), "{err}");
    }
}
