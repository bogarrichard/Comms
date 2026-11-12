//! Slack's OAuth v2 user sign-in with PKCE (no client secret). The caller
//! owns the browser and the loopback listener; this module only builds the
//! authorize URL and turns the returned code into a user token.

use std::time::Duration;

use comms_provider_api::ProviderError;
use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::{SLACK_BASE, scopes};

/// Where the user is sent to approve the app.
pub const AUTHORIZE_URL: &str = "https://slack.com/oauth/v2/authorize";

/// The authorize link for one sign-in attempt. `redirect_uri` must be one of
/// the app's registered redirect URLs, and the app needs `pkce_enabled`.
pub fn authorize_url(client_id: &str, redirect_uri: &str, state: &str, challenge: &str) -> String {
    let mut url = Url::parse(AUTHORIZE_URL).expect("constant URL is valid");
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("user_scope", &scopes::USER.join(","))
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256");
    url.into()
}

#[derive(Debug, Deserialize)]
struct Access {
    authed_user: Option<AuthedUser>,
}

#[derive(Debug, Deserialize)]
struct AuthedUser {
    access_token: Option<String>,
}

/// Exchange the one-time `code` for the signed-in user's token. `api_base` is
/// [`SLACK_BASE`] except in tests. No client secret is sent: PKCE proves the
/// exchange comes from whoever started the sign-in.
pub async fn exchange_code(
    api_base: &str,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<String, ProviderError> {
    let url = format!("{}/oauth.v2.access", api_base.trim_end_matches('/'));
    let http = reqwest::Client::builder()
        .user_agent(concat!("comms/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| ProviderError::Network {
            may_have_reached_server: false,
            message: e.to_string(),
        })?;
    let resp = http
        .post(url)
        .form(&[
            ("client_id", client_id),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .map_err(|e| ProviderError::Network {
            may_have_reached_server: true,
            message: e.without_url().to_string(),
        })?;
    let value: Value = resp
        .json()
        .await
        .map_err(|e| ProviderError::InvalidResponse(format!("oauth.v2.access: {e}")))?;
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        let code = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown_error");
        return Err(ProviderError::BadRequest(format!(
            "Slack refused the sign-in ({code})"
        )));
    }
    let access: Access = serde_json::from_value(value)
        .map_err(|e| ProviderError::InvalidResponse(format!("oauth.v2.access: {e}")))?;
    access
        .authed_user
        .and_then(|u| u.access_token)
        .filter(|t| crate::looks_like_token(t))
        .ok_or_else(|| {
            ProviderError::InvalidResponse(
                "Slack's answer holds no user token (does the app request user scopes?)".into(),
            )
        })
}

/// The default API base for [`exchange_code`].
pub const DEFAULT_API_BASE: &str = SLACK_BASE;
