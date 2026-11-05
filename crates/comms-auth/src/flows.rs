//! Protocol steps against the Entra v2.0 endpoints.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use crate::config::AuthConfig;
use crate::error::AuthError;

pub(crate) struct Tokens {
    pub access_token: String,
    pub expires_at: DateTime<Utc>,
    pub refresh_token: Option<String>,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: Option<String>,
    error_description: Option<String>,
}

/// What to show the user during device-code sign-in.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCodePrompt {
    pub verification_uri: String,
    pub user_code: String,
    pub message: Option<String>,
    #[serde(default)]
    pub expires_in: u64,
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    #[serde(flatten)]
    prompt: DeviceCodePrompt,
    interval: Option<u64>,
}

pub(crate) fn http_client() -> Result<Client, AuthError> {
    Client::builder()
        .user_agent(concat!("comms/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| AuthError::Config(e.to_string()))
}

fn net(e: reqwest::Error) -> AuthError {
    AuthError::Network(e.without_url().to_string())
}

async fn post_form(
    http: &Client,
    url: Url,
    form: &[(&str, &str)],
) -> Result<serde_json::Value, AuthError> {
    let resp = http.post(url).form(form).send().await.map_err(net)?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.map_err(|_| {
        AuthError::Network(format!(
            "unexpected response from login server (HTTP {status})"
        ))
    })?;
    if status.is_success() {
        return Ok(body);
    }
    let err: ErrorResponse = serde_json::from_value(body).unwrap_or(ErrorResponse {
        error: None,
        error_description: None,
    });
    Err(AuthError::Server {
        code: err
            .error
            .unwrap_or_else(|| format!("http_{}", status.as_u16())),
        description: err
            .error_description
            .map(|d| d.lines().next().unwrap_or_default().to_owned())
            .unwrap_or_else(|| format!("login server returned HTTP {status}")),
    })
}

fn into_tokens(body: serde_json::Value) -> Result<Tokens, AuthError> {
    let r: TokenResponse = serde_json::from_value(body).map_err(|_| {
        AuthError::Network("login server returned an unexpected token response".into())
    })?;
    // `expires_in` is a number, but some servers send it as a string.
    let secs = match r.expires_in {
        Some(serde_json::Value::Number(n)) => n.as_i64(),
        Some(serde_json::Value::String(s)) => s.parse().ok(),
        _ => None,
    }
    .unwrap_or(3600);
    Ok(Tokens {
        access_token: r.access_token,
        expires_at: Utc::now() + chrono::Duration::seconds(secs),
        refresh_token: r.refresh_token,
    })
}

fn random_urlsafe(bytes: usize) -> Result<String, AuthError> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf)
        .map_err(|e| AuthError::Config(format!("no secure randomness: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

pub(crate) fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

// ---------------------------------------------------------------- refresh

pub(crate) async fn refresh(
    http: &Client,
    cfg: &AuthConfig,
    refresh_token: &str,
) -> Result<Tokens, AuthError> {
    let scope = cfg.scope_string();
    let body = post_form(
        http,
        cfg.endpoint("token")?,
        &[
            ("client_id", &cfg.client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", &scope),
        ],
    )
    .await
    .map_err(|e| match e {
        AuthError::Server { code, .. } if code == "invalid_grant" => AuthError::ReauthRequired,
        other => other,
    })?;
    into_tokens(body)
}

// ---------------------------------------------------------------- browser (PKCE)

const SUCCESS_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Comms</title>\
<body style=\"font-family:sans-serif;margin:3em\"><h2>Signed in</h2>\
<p>You can close this tab and return to Comms.</p></body>";

const FAILURE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Comms</title>\
<body style=\"font-family:sans-serif;margin:3em\"><h2>Sign-in failed</h2>\
<p>Return to Comms for details.</p></body>";

/// Authorization code flow with PKCE and a loopback redirect (RFC 8252).
pub(crate) async fn browser_login(
    http: &Client,
    cfg: &AuthConfig,
    on_url: impl FnOnce(&str),
    timeout: Duration,
) -> Result<Tokens, AuthError> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| AuthError::Config(format!("cannot listen on loopback: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| AuthError::Config(e.to_string()))?
        .port();
    // Entra matches loopback redirect URIs ignoring the port, but requires the
    // registered host name (`localhost`).
    let redirect_uri = format!("http://localhost:{port}");

    let verifier = random_urlsafe(48)?;
    let state = random_urlsafe(24)?;
    let scope = cfg.scope_string();

    let mut auth_url = cfg.endpoint("authorize")?;
    auth_url
        .query_pairs_mut()
        .append_pair("client_id", &cfg.client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("response_mode", "query")
        .append_pair("scope", &scope)
        .append_pair("state", &state)
        .append_pair("code_challenge", &pkce_challenge(&verifier))
        .append_pair("code_challenge_method", "S256")
        .append_pair("prompt", "select_account");
    on_url(auth_url.as_str());

    let code = tokio::time::timeout(timeout, wait_for_redirect(&listener, &state))
        .await
        .map_err(|_| AuthError::Timeout)??;

    let body = post_form(
        http,
        cfg.endpoint("token")?,
        &[
            ("client_id", &cfg.client_id),
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", &verifier),
            ("scope", &scope),
        ],
    )
    .await?;
    into_tokens(body)
}

async fn wait_for_redirect(listener: &TcpListener, state: &str) -> Result<String, AuthError> {
    loop {
        let (mut sock, _) = listener
            .accept()
            .await
            .map_err(|e| AuthError::Network(e.to_string()))?;
        let mut buf = vec![0u8; 8192];
        let mut len = 0;
        // Read the request head; bounded so a misbehaving client cannot hang us.
        let head = loop {
            let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf[len..]))
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or(0);
            if n == 0 {
                break None;
            }
            len += n;
            if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") || len == buf.len() {
                break Some(String::from_utf8_lossy(&buf[..len]).into_owned());
            }
        };
        let Some(head) = head else { continue };
        let target = head.lines().next().and_then(|l| {
            let mut parts = l.split_whitespace();
            (parts.next() == Some("GET"))
                .then(|| parts.next())
                .flatten()
        });
        let Some(target) = target else {
            respond(&mut sock, "400 Bad Request", FAILURE_PAGE).await;
            continue;
        };
        let Ok(url) = Url::parse(&format!("http://localhost{target}")) else {
            respond(&mut sock, "400 Bad Request", FAILURE_PAGE).await;
            continue;
        };
        let q = |k: &str| {
            url.query_pairs()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.into_owned())
        };
        if q("code").is_none() && q("error").is_none() {
            // favicon, prefetch, port scanners...
            respond(&mut sock, "404 Not Found", "").await;
            continue;
        }
        if q("state").as_deref() != Some(state) {
            respond(&mut sock, "400 Bad Request", FAILURE_PAGE).await;
            return Err(AuthError::Denied(
                "the sign-in response did not match this request".into(),
            ));
        }
        if let Some(err) = q("error") {
            respond(&mut sock, "200 OK", FAILURE_PAGE).await;
            let desc = q("error_description").unwrap_or_default();
            let first = desc.lines().next().unwrap_or_default().to_owned();
            return Err(if err == "access_denied" {
                AuthError::Denied(first)
            } else {
                AuthError::Server {
                    code: err,
                    description: first,
                }
            });
        }
        respond(&mut sock, "200 OK", SUCCESS_PAGE).await;
        return q("code").ok_or_else(|| AuthError::Denied("no authorization code returned".into()));
    }
}

async fn respond(sock: &mut tokio::net::TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(resp.as_bytes()).await;
    let _ = sock.shutdown().await;
}

// ---------------------------------------------------------------- device code

pub(crate) async fn device_code_login(
    http: &Client,
    cfg: &AuthConfig,
    on_prompt: impl FnOnce(&DeviceCodePrompt),
) -> Result<Tokens, AuthError> {
    let scope = cfg.scope_string();
    let body = post_form(
        http,
        cfg.endpoint("devicecode")?,
        &[("client_id", &cfg.client_id), ("scope", &scope)],
    )
    .await?;
    let dc: DeviceCodeResponse = serde_json::from_value(body)
        .map_err(|_| AuthError::Network("unexpected device code response".into()))?;
    on_prompt(&dc.prompt);

    let mut interval = Duration::from_secs(dc.interval.unwrap_or(5));
    let deadline = tokio::time::Instant::now()
        + Duration::from_secs(if dc.prompt.expires_in == 0 {
            900
        } else {
            dc.prompt.expires_in
        });
    loop {
        tokio::time::sleep(interval).await;
        if tokio::time::Instant::now() > deadline {
            return Err(AuthError::Timeout);
        }
        let res = post_form(
            http,
            cfg.endpoint("token")?,
            &[
                ("client_id", &cfg.client_id),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", &dc.device_code),
            ],
        )
        .await;
        match res {
            Ok(body) => return into_tokens(body),
            Err(AuthError::Server { code, description }) => match code.as_str() {
                "authorization_pending" => {}
                "slow_down" => interval += Duration::from_secs(5),
                "authorization_declined" => return Err(AuthError::Denied(description)),
                "expired_token" | "bad_verification_code" => return Err(AuthError::Timeout),
                _ => return Err(AuthError::Server { code, description }),
            },
            Err(e) => return Err(e),
        }
    }
}
