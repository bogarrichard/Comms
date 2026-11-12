//! The Slack backend (Web API, user token). Compiled with the `slack` feature.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use comms_app::AppHandle;
use comms_provider_slack::{SlackProvider, looks_like_token};
use comms_storage::Store;

use crate::{Paths, Settings, core_config};

/// The Slack backend. The token comes from `COMMS_SLACK_TOKEN` or `[slack] token`.
pub async fn start_slack(settings: &Settings) -> Result<AppHandle> {
    let Some(token) = settings.slack.token.clone() else {
        bail!(
            "no Slack token configured.\n\
             Create a Slack app and copy its user token (xoxp-...), then set \
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

#[cfg(test)]
mod tests {
    use super::*;

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
