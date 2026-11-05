//! Thin Microsoft Graph HTTP client: auth header, throttling, retries,
//! error mapping and pagination-link validation.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use comms_provider_api::ProviderError;
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::Semaphore;
use tracing::{debug, warn};
use url::Url;

pub const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";

/// Supplies bearer tokens. Implemented by the auth layer (see `comms-bootstrap`).
#[async_trait]
pub trait AccessTokenSource: Send + Sync {
    /// A currently valid access token, refreshing it if necessary.
    async fn access_token(&self) -> Result<String, ProviderError>;
    /// The server rejected the last token; drop any cached copy.
    async fn invalidate(&self);
}

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Total attempts for requests that are safe to repeat.
    pub max_attempts: u32,
    /// Longest single wait honoured from `Retry-After`.
    pub max_wait: Duration,
    /// Base for exponential backoff when the server gives no `Retry-After`.
    pub backoff_base: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            max_wait: Duration::from_secs(30),
            backoff_base: Duration::from_secs(1),
        }
    }
}

pub struct GraphClient {
    http: reqwest::Client,
    base: Url,
    tokens: Arc<dyn AccessTokenSource>,
    permits: Semaphore,
    retry: RetryPolicy,
}

impl GraphClient {
    /// `base` must be HTTPS (plain HTTP is accepted for loopback test servers only).
    pub fn new(base: &str, tokens: Arc<dyn AccessTokenSource>) -> Result<Self, ProviderError> {
        let base = Url::parse(base.trim_end_matches('/'))
            .map_err(|e| ProviderError::InvalidResponse(format!("bad base url: {e}")))?;
        let loopback = matches!(base.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if base.scheme() != "https" && !loopback {
            return Err(ProviderError::InvalidResponse(
                "Graph base URL must use HTTPS".into(),
            ));
        }
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
        Ok(Self {
            http,
            base,
            tokens,
            permits: Semaphore::new(4),
            retry: RetryPolicy::default(),
        })
    }

    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Build `base + segments`, percent-encoding each segment.
    pub fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        {
            let mut path = url
                .path_segments_mut()
                .expect("https base has path segments");
            for s in segments {
                path.push(s);
            }
        }
        url
    }

    /// Accept a server-provided `@odata.nextLink` only if it points back at
    /// the Graph host we authenticate to, so tokens can never be sent elsewhere.
    pub fn validate_link(&self, link: &str) -> Result<Url, ProviderError> {
        let url = Url::parse(link)
            .map_err(|_| ProviderError::InvalidResponse("malformed pagination link".into()))?;
        if url.scheme() != self.base.scheme()
            || url.host_str() != self.base.host_str()
            || url.port_or_known_default() != self.base.port_or_known_default()
        {
            return Err(ProviderError::InvalidResponse(
                "pagination link points at a different host".into(),
            ));
        }
        Ok(url)
    }

    pub async fn get<T: DeserializeOwned>(&self, url: Url) -> Result<T, ProviderError> {
        let resp = self.send(Method::GET, url, None::<&()>).await?;
        parse_json(resp).await
    }

    /// POST a JSON body. Never retried once the request may have been accepted.
    pub async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        url: Url,
        body: &B,
    ) -> Result<T, ProviderError> {
        let resp = self.send(Method::POST, url, Some(body)).await?;
        parse_json(resp).await
    }

    async fn send<B: Serialize>(
        &self,
        method: Method,
        url: Url,
        body: Option<&B>,
    ) -> Result<reqwest::Response, ProviderError> {
        let idempotent = method == Method::GET;
        let _permit = self.permits.acquire().await.expect("semaphore open");
        let mut refreshed = false;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let token = self.tokens.access_token().await?;
            let mut req = self
                .http
                .request(method.clone(), url.clone())
                .bearer_auth(token);
            if let Some(b) = body {
                req = req.json(b);
            }
            let resp = req.send().await.map_err(map_transport_error)?;
            let status = resp.status();
            if status.is_success() {
                return Ok(resp);
            }
            let retry_after = parse_retry_after(&resp);
            let err_body = resp.text().await.unwrap_or_default();
            let message = graph_error_message(&err_body);
            debug!(%status, %url, "graph request failed: {message}");

            match status {
                StatusCode::UNAUTHORIZED => {
                    // The request was rejected before processing, so repeating is safe.
                    if refreshed {
                        return Err(ProviderError::Unauthorized);
                    }
                    refreshed = true;
                    self.tokens.invalidate().await;
                }
                StatusCode::TOO_MANY_REQUESTS => {
                    let wait = retry_after
                        .unwrap_or_else(|| self.backoff(attempt))
                        .min(self.retry.max_wait);
                    if attempt >= self.retry.max_attempts {
                        return Err(ProviderError::RateLimited { retry_after: wait });
                    }
                    warn!(?wait, "throttled by Graph; waiting");
                    tokio::time::sleep(wait).await;
                }
                StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT if idempotent => {
                    if attempt >= self.retry.max_attempts {
                        return Err(ProviderError::Server {
                            status: status.as_u16(),
                            message,
                        });
                    }
                    let wait = retry_after
                        .unwrap_or_else(|| self.backoff(attempt))
                        .min(self.retry.max_wait);
                    tokio::time::sleep(wait).await;
                }
                StatusCode::FORBIDDEN => return Err(ProviderError::Forbidden(message)),
                StatusCode::NOT_FOUND => return Err(ProviderError::NotFound),
                StatusCode::BAD_REQUEST => return Err(ProviderError::BadRequest(message)),
                s => {
                    return Err(ProviderError::Server {
                        status: s.as_u16(),
                        message,
                    });
                }
            }
        }
    }

    fn backoff(&self, attempt: u32) -> Duration {
        self.retry.backoff_base * 2u32.saturating_pow(attempt.saturating_sub(1))
    }
}

fn map_transport_error(e: reqwest::Error) -> ProviderError {
    // Only connection-establishment failures prove the request never arrived.
    let may_have_reached_server = !e.is_connect();
    ProviderError::Network {
        may_have_reached_server,
        message: describe(e),
    }
}

fn describe(e: reqwest::Error) -> String {
    if e.is_timeout() {
        "request timed out".into()
    } else if e.is_connect() {
        "could not connect to Microsoft Graph".into()
    } else {
        // Do not include the URL: it may contain identifiers.
        e.without_url().to_string()
    }
}

fn parse_retry_after(resp: &reqwest::Response) -> Option<Duration> {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

async fn parse_json<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T, ProviderError> {
    let bytes = resp.bytes().await.map_err(map_transport_error)?;
    serde_json::from_slice(&bytes)
        .map_err(|e| ProviderError::InvalidResponse(format!("unexpected response body: {e}")))
}

/// Extract `error.code: error.message` without ever echoing message content.
fn graph_error_message(body: &str) -> String {
    #[derive(serde::Deserialize)]
    struct Envelope {
        error: Option<Inner>,
    }
    #[derive(serde::Deserialize)]
    struct Inner {
        code: Option<String>,
        message: Option<String>,
    }
    match serde_json::from_str::<Envelope>(body)
        .ok()
        .and_then(|e| e.error)
    {
        Some(Inner { code, message }) => match (code, message) {
            (Some(c), Some(m)) => format!("{c}: {m}"),
            (Some(c), None) => c,
            (None, Some(m)) => m,
            _ => "unknown error".into(),
        },
        None => "unknown error".into(),
    }
}
