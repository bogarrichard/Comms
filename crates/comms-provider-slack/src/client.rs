//! Thin Slack Web API client: bearer auth, form encoding, and mapping of
//! Slack's `{"ok": false, "error": "..."}` answers onto [`ProviderError`].
//!
//! Slack answers almost everything with HTTP 200 and reports failures in the
//! body, so the error string is what decides whether a request provably was
//! not applied (see [`map_error`]). Nothing here retries: the engine owns that
//! decision for sends, and Slack's rate limits for apps outside the
//! Marketplace are strict enough that blind retries would only make them worse.

use std::fmt;
use std::time::Duration;

use comms_provider_api::ProviderError;
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde_json::Value;
use url::Url;

pub const SLACK_BASE: &str = "https://slack.com/api";

/// Wait suggested when Slack rate limits us without a usable `Retry-After`.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(30);

pub struct SlackClient {
    http: reqwest::Client,
    base: Url,
    token: String,
}

impl fmt::Debug for SlackClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SlackClient")
            .field("base", &self.base.as_str())
            .field("token", &"<redacted>")
            .finish()
    }
}

impl SlackClient {
    /// `base` must be HTTPS (plain HTTP is accepted for loopback test servers only).
    pub fn new(base: &str, token: String) -> Result<Self, ProviderError> {
        let base = Url::parse(base.trim_end_matches('/'))
            .map_err(|e| ProviderError::InvalidResponse(format!("bad base url: {e}")))?;
        let loopback = matches!(base.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if base.scheme() != "https" && !loopback {
            return Err(ProviderError::InvalidResponse(
                "Slack base URL must use HTTPS".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(concat!("comms/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            // The token must never follow a redirect to another host.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ProviderError::Network {
                may_have_reached_server: false,
                message: e.to_string(),
            })?;
        Ok(Self { http, base, token })
    }

    /// Call one Web API method (`conversations.list`, `chat.postMessage`, ...).
    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: &[(&str, &str)],
    ) -> Result<T, ProviderError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| ProviderError::InvalidResponse("base url cannot hold a path".into()))?
            .push(method);

        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .form(params)
            .send()
            .await
            .map_err(network_error)?;

        let status = resp.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map_or(DEFAULT_RETRY_AFTER, Duration::from_secs);
            return Err(ProviderError::RateLimited { retry_after });
        }
        if !status.is_success() {
            return Err(match status {
                StatusCode::UNAUTHORIZED => ProviderError::Unauthorized,
                StatusCode::FORBIDDEN => ProviderError::Forbidden(format!("HTTP {status}")),
                _ => ProviderError::Server {
                    status: status.as_u16(),
                    message: status.canonical_reason().unwrap_or("error").to_owned(),
                },
            });
        }

        let value: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::InvalidResponse(format!("{method}: {e}")))?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(map_error(&value));
        }
        serde_json::from_value(value)
            .map_err(|e| ProviderError::InvalidResponse(format!("unexpected {method} answer: {e}")))
    }
}

/// Largest workspace icon we are willing to download.
const MAX_ICON_BYTES: usize = 2 * 1024 * 1024;

impl SlackClient {
    /// Download a public image (the workspace icon). The token is **not**
    /// sent: icons live on Slack's CDN and need no authentication.
    pub async fn fetch_image(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let url = Url::parse(url)
            .map_err(|e| ProviderError::InvalidResponse(format!("bad image url: {e}")))?;
        let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if url.scheme() != "https" && !loopback {
            return Err(ProviderError::InvalidResponse(
                "image URL must use HTTPS".into(),
            ));
        }
        let resp = self.http.get(url).send().await.map_err(network_error)?;
        if !resp.status().is_success() {
            return Err(ProviderError::Server {
                status: resp.status().as_u16(),
                message: "image download failed".into(),
            });
        }
        if resp
            .content_length()
            .is_some_and(|n| n > MAX_ICON_BYTES as u64)
        {
            return Err(ProviderError::InvalidResponse("image too large".into()));
        }
        let bytes = resp.bytes().await.map_err(network_error)?;
        if bytes.len() > MAX_ICON_BYTES {
            return Err(ProviderError::InvalidResponse("image too large".into()));
        }
        Ok(bytes.to_vec())
    }
}

fn network_error(e: reqwest::Error) -> ProviderError {
    ProviderError::Network {
        // Only a failure to connect proves the request never left.
        may_have_reached_server: !(e.is_connect() || e.is_builder()),
        message: e.without_url().to_string(),
    }
}

/// Map a Slack `{"ok": false, "error": code}` body to a [`ProviderError`].
///
/// Unknown codes become [`ProviderError::InvalidResponse`], which counts as
/// "might have been applied": the engine then re-syncs instead of resending.
pub(crate) fn map_error(body: &Value) -> ProviderError {
    let code = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("unknown_error");
    match code {
        "invalid_auth" | "not_authed" | "token_revoked" | "token_expired" | "account_inactive"
        | "invalid_token" => ProviderError::Unauthorized,
        "missing_scope" => {
            let needed = body.get("needed").and_then(Value::as_str).unwrap_or("?");
            ProviderError::Forbidden(format!(
                "the token lacks a permission (missing_scope, needs `{needed}`); \
                 add it to the Slack app and reinstall it"
            ))
        }
        "access_denied"
        | "not_allowed_token_type"
        | "restricted_action"
        | "not_in_channel"
        | "is_archived"
        | "cant_update_message"
        | "cant_delete_message"
        | "edit_window_closed"
        | "team_access_not_granted"
        | "ekm_access_denied" => ProviderError::Forbidden(code.to_owned()),
        "channel_not_found" | "user_not_found" | "message_not_found" | "thread_not_found" => {
            ProviderError::NotFound
        }
        "ratelimited" | "rate_limited" => ProviderError::RateLimited {
            retry_after: DEFAULT_RETRY_AFTER,
        },
        "msg_too_long"
        | "no_text"
        | "invalid_arguments"
        | "invalid_arg_name"
        | "invalid_cursor"
        | "invalid_limit"
        | "invalid_form_data"
        | "invalid_charset"
        | "invalid_ts_latest"
        | "invalid_ts_oldest"
        | "missing_post_type"
        | "too_many_attachments" => ProviderError::BadRequest(code.to_owned()),
        "internal_error" | "fatal_error" | "service_unavailable" | "request_timeout" => {
            ProviderError::Server {
                status: 500,
                message: code.to_owned(),
            }
        }
        other => ProviderError::InvalidResponse(format!("unrecognised Slack error `{other}`")),
    }
}
