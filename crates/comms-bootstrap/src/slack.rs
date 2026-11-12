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
    let provider = SlackProvider::new(token)?
        .member_only(settings.slack.channels == crate::SlackChannels::Member);
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

/// The port of the redirect URL registered in the Slack app
/// (`http://localhost:8765/callback`).
const REDIRECT_PORT: u16 = 8765;
const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// A Slack Client ID looks like `1234567890.1234567890123`.
fn valid_client_id(id: &str) -> bool {
    let mut parts = id.split('.');
    let digits =
        |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    digits(parts.next()) && digits(parts.next()) && parts.next().is_none()
}

/// Sign in through the browser (authorization code + PKCE, no client secret),
/// then verify and store the token like [`slack_login`]. Returns your name.
pub async fn slack_login_browser(settings: &Settings, say: impl Fn(&str)) -> Result<String> {
    let client_id = settings.slack.client_id.clone().context(
        "no Slack client ID configured (set [slack] client_id or COMMS_SLACK_CLIENT_ID)",
    )?;
    let token = browser_token(
        comms_provider_slack::oauth::DEFAULT_API_BASE,
        &client_id,
        REDIRECT_PORT,
        |url| {
            if crate::open_in_browser(url) {
                say("Opening your browser to sign in to Slack...");
            } else {
                say("Open this address in a browser to sign in to Slack:");
            }
            say(url);
            say("Waiting for you to click Allow (Ctrl+C to cancel)...");
        },
        LOGIN_TIMEOUT,
    )
    .await?;
    slack_login(&token).await
}

/// The browser round trip: listen on loopback, hand the authorize URL to
/// `on_url`, wait for the redirect and swap the code for a token.
async fn browser_token(
    api_base: &str,
    client_id: &str,
    port: u16,
    on_url: impl FnOnce(&str),
    timeout: std::time::Duration,
) -> Result<String> {
    use comms_auth::loopback;
    if !valid_client_id(client_id) {
        bail!(
            "the Slack client ID must look like 1234567890.1234567890123 (Basic Information in your app)"
        );
    }
    let pkce = loopback::Pkce::new()?;
    // Bound to 127.0.0.1 only; closed as soon as one valid answer arrives.
    let listener = loopback::bind(port).await?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://localhost:{port}/callback");
    on_url(&comms_provider_slack::oauth::authorize_url(
        client_id,
        &redirect_uri,
        &pkce.state,
        &pkce.challenge,
    ));
    let code = loopback::wait_for_code(&listener, &pkce.state, timeout).await?;
    drop(listener);
    Ok(comms_provider_slack::oauth::exchange_code(
        api_base,
        client_id,
        &code,
        &redirect_uri,
        &pkce.verifier,
    )
    .await?)
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

    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A fake browser: follows the authorize URL's redirect with `code=abc`
    /// and the `state` chosen by `state_for`.
    fn fake_browser(state_for: impl FnOnce(&str) -> String + Send + 'static) -> impl FnOnce(&str) {
        move |auth_url| {
            let url = url::Url::parse(auth_url).unwrap();
            let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
            let target = format!(
                "{}?code=abc&state={}",
                q["redirect_uri"],
                state_for(&q["state"])
            );
            tokio::spawn(async move {
                let _ = reqwest::get(target).await;
            });
        }
    }

    async fn token_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/oauth.v2.access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "authed_user": {"id": "U1", "access_token": "xoxp-user-token-123"}
            })))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn browser_login_exchanges_the_code_with_a_matching_pkce_verifier() {
        use base64::Engine;
        use sha2::Digest;
        let server = token_server().await;
        let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let seen2 = seen.clone();
        let browser = fake_browser(|s| s.to_owned());
        let token = browser_token(
            &format!("{}/api", server.uri()),
            "111.222",
            0,
            move |u| {
                *seen2.lock().unwrap() = u.to_owned();
                browser(u)
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(token, "xoxp-user-token-123");

        let auth = url::Url::parse(&seen.lock().unwrap()).unwrap();
        let challenge = auth
            .query_pairs()
            .find(|(k, _)| k == "code_challenge")
            .unwrap()
            .1
            .into_owned();
        let body =
            String::from_utf8(server.received_requests().await.unwrap()[0].body.clone()).unwrap();
        let verifier = body
            .split('&')
            .find_map(|p| p.strip_prefix("code_verifier="))
            .expect("the exchange carries the verifier");
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, expected, "the challenge must be S256(verifier)");
        assert!(verifier.len() >= 43, "RFC 7636 minimum length");
        assert!(!body.contains("client_secret"));
    }

    #[tokio::test]
    async fn a_redirect_with_the_wrong_state_is_refused_and_nothing_is_exchanged() {
        let server = token_server().await;
        let err = browser_token(
            &format!("{}/api", server.uri()),
            "111.222",
            0,
            fake_browser(|_| "forged".to_owned()),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("did not match"), "{err}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn login_times_out_when_nobody_answers_and_checks_the_client_id() {
        let server = token_server().await;
        let base = format!("{}/api", server.uri());
        let err = browser_token(&base, "111.222", 0, |_| {}, Duration::from_millis(100))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.to_lowercase().contains("time"), "{err}");
        for bad in ["", "abc", "1.2.3", "12 34.5", "1.2&x=y"] {
            let err = browser_token(&base, bad, 0, |_| {}, Duration::from_secs(1))
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains("client ID"), "{bad}: {err}");
        }
    }
}
