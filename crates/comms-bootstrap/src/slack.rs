//! The Slack backend (Web API, user token). Compiled with the `slack` feature.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use comms_app::AppHandle;
use comms_provider_api::MessagingProvider;
use comms_provider_slack::{SlackProvider, looks_like_token};
use comms_storage::Store;

use crate::{Paths, Settings, WorkspaceInfo, core_config};

/// The Slack backend. The token comes from `COMMS_SLACK_TOKEN` or `[slack] token`.
pub async fn start_slack(settings: &Settings) -> Result<AppHandle> {
    let Some(token) = settings.slack.token.clone() else {
        bail!(
            "no Slack token configured.\n\
             Run `comms slack-login` to paste your user token (xoxp-...), or set \
             COMMS_SLACK_TOKEN or `token = \"xoxp-...\"` in the [slack] section of {}. \
             See docs/slack.md.",
            Paths::discover()?.config_file.display()
        );
    };
    if !looks_like_token(&token) {
        bail!("the Slack token must look like xoxp-... (a user token); check slack.token");
    }
    let paths = Paths::discover()?;
    let store = Store::open(&paths.database)
        .await
        .context("opening the local database")?;
    let provider = SlackProvider::new(token)?;
    Ok(AppHandle::start(Arc::new(provider), store, core_config(settings)).await?)
}

/// The workspace's name and icon (needs the `team:read` scope).
pub async fn slack_workspace(settings: &Settings) -> Result<WorkspaceInfo> {
    let token = settings
        .slack
        .token
        .clone()
        .context("no Slack token configured")?;
    let ws = SlackProvider::new(token)?.workspace().await?;
    Ok(WorkspaceInfo {
        name: ws.name,
        icon: ws.icon,
    })
}

/// Check a pasted user token with Slack, then store it (and `provider =
/// "slack"`) in the config file. Returns the name Slack knows you by.
/// The token is never echoed in an error.
pub async fn slack_login(token: &str) -> Result<String> {
    let token = token.trim();
    if !looks_like_token(token) {
        bail!(
            "that does not look like a Slack user token (it starts with xoxp-). \
             An xoxb- bot token or an xapp- app token will not work."
        );
    }
    if token.starts_with("xoxb-") || token.starts_with("xapp-") {
        bail!("that is a bot or app token; Comms needs the User OAuth Token (xoxp-...)");
    }
    let me = SlackProvider::new(token.to_owned())?
        .current_user()
        .await
        .context("Slack did not accept the token")?;
    save_slack_token(&Paths::discover()?.config_file, token)?;
    Ok(me.display_name)
}

/// Set `provider = "slack"` and `[slack] token` in `config_file`, keeping
/// everything else (comments included). The file is created with owner-only
/// permissions and replaced atomically.
pub fn save_slack_token(config_file: &Path, token: &str) -> Result<()> {
    let text = match std::fs::read_to_string(config_file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", config_file.display())),
    };
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("{} is not valid TOML", config_file.display()))?;
    doc["provider"] = toml_edit::value("slack");
    if !doc.contains_table("slack") {
        doc["slack"] = toml_edit::table();
    }
    doc["slack"]["token"] = toml_edit::value(token);

    if let Some(dir) = config_file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = config_file.with_extension("toml.tmp");
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        f.write_all(doc.to_string().as_bytes())?;
    }
    std::fs::rename(&tmp, config_file)
        .with_context(|| format!("replacing {}", config_file.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_keeps_comments_and_other_settings_and_locks_the_file_down() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        std::fs::write(
            &file,
            "# my notes\ntenant = \"bme.hu\"\n[poll]\nopen_secs = 20\n",
        )
        .unwrap();
        save_slack_token(&file, "xoxp-test-token-123").unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("# my notes") && text.contains("open_secs = 20"));
        let s = Settings::parse(&text).unwrap();
        assert_eq!(s.provider, crate::Backend::Slack);
        assert_eq!(s.slack.token.as_deref(), Some("xoxp-test-token-123"));
        assert_eq!(s.tenant, "bme.hu");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "config must not be group/world readable");
        }
    }

    #[test]
    fn saving_creates_the_file_and_replaces_an_old_token() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("new/config.toml");
        save_slack_token(&file, "xoxp-first-token-1").unwrap();
        save_slack_token(&file, "xoxp-second-token-2").unwrap();
        let s = Settings::from_file(&file).unwrap();
        assert_eq!(s.slack.token.as_deref(), Some("xoxp-second-token-2"));
    }

    #[tokio::test]
    async fn login_refuses_bot_and_app_tokens_without_echoing_them() {
        for t in ["xoxb-1234567890-secret", "xapp-1-A0123456789-secret"] {
            let err = slack_login(t).await.unwrap_err().to_string();
            assert!(!err.contains("secret"), "{err}");
        }
    }

    #[tokio::test]
    async fn starting_slack_without_a_token_explains_what_to_do() {
        let err = start_slack(&Settings::default())
            .await
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("COMMS_SLACK_TOKEN") && err.contains("xoxp"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn starting_slack_with_a_malformed_token_is_rejected() {
        let mut s = Settings::default();
        s.slack.token = Some("hunter2".into());
        let err = start_slack(&s).await.map(|_| ()).unwrap_err().to_string();
        assert!(err.contains("xoxp"), "{err}");
        assert!(
            !err.contains("hunter2"),
            "the token must not be echoed: {err}"
        );
    }
}
