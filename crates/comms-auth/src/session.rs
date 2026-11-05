use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::config::AuthConfig;
use crate::error::AuthError;
use crate::flows::{self, DeviceCodePrompt, Tokens};
use crate::store::{StoredSession, TokenStore};

struct Cached {
    token: String,
    expires_at: DateTime<Utc>,
}

/// An authenticated session: hands out access tokens, refreshing and
/// persisting rotated refresh tokens as needed.
pub struct Session {
    cfg: AuthConfig,
    http: reqwest::Client,
    store: Arc<dyn TokenStore>,
    cache: Mutex<Option<Cached>>,
}

/// Permission sets are compared ignoring order, case and the OIDC basics.
fn normalise_scopes(scopes: &[String]) -> String {
    let mut v: Vec<String> = scopes
        .iter()
        .map(|s| s.to_lowercase())
        .filter(|s| !matches!(s.as_str(), "openid" | "profile" | "offline_access"))
        .collect();
    v.sort();
    v.dedup();
    v.join(" ")
}

impl Session {
    pub fn new(cfg: AuthConfig, store: Arc<dyn TokenStore>) -> Result<Self, AuthError> {
        Ok(Self {
            http: flows::http_client()?,
            cfg,
            store,
            cache: Mutex::new(None),
        })
    }

    pub fn config(&self) -> &AuthConfig {
        &self.cfg
    }

    /// Whether a usable stored sign-in exists for the configured permissions.
    pub async fn is_signed_in(&self) -> Result<bool, AuthError> {
        Ok(matches!(
            self.store.load().await?,
            Some(s) if s.scopes == normalise_scopes(&self.cfg.scopes)
        ))
    }

    /// Interactive sign-in via the system browser. `on_url` receives the URL
    /// the user must open (the caller launches the browser or prints it).
    pub async fn login_browser(&self, on_url: impl FnOnce(&str)) -> Result<(), AuthError> {
        let tokens =
            flows::browser_login(&self.http, &self.cfg, on_url, Duration::from_secs(300)).await?;
        self.adopt(tokens).await
    }

    /// Device-code sign-in for terminals without a local browser.
    pub async fn login_device_code(
        &self,
        on_prompt: impl FnOnce(&DeviceCodePrompt),
    ) -> Result<(), AuthError> {
        let tokens = flows::device_code_login(&self.http, &self.cfg, on_prompt).await?;
        self.adopt(tokens).await
    }

    async fn adopt(&self, tokens: Tokens) -> Result<(), AuthError> {
        let refresh = tokens.refresh_token.clone().ok_or_else(|| {
            AuthError::Config(
                "the server issued no refresh token (is offline_access granted?)".into(),
            )
        })?;
        self.store
            .save(&StoredSession {
                refresh_token: refresh,
                scopes: normalise_scopes(&self.cfg.scopes),
            })
            .await?;
        *self.cache.lock().await = Some(Cached {
            token: tokens.access_token,
            expires_at: tokens.expires_at,
        });
        Ok(())
    }

    /// A valid access token. Refreshes (and rotates the stored refresh token)
    /// when the cached one is close to expiry. Concurrent callers share one refresh.
    pub async fn access_token(&self) -> Result<String, AuthError> {
        let mut cache = self.cache.lock().await;
        if let Some(c) = cache.as_ref()
            && c.expires_at - chrono::Duration::minutes(2) > Utc::now()
        {
            return Ok(c.token.clone());
        }
        let stored = self.store.load().await?.ok_or(AuthError::NotSignedIn)?;
        if stored.scopes != normalise_scopes(&self.cfg.scopes) {
            return Err(AuthError::ScopesChanged);
        }
        debug!("refreshing access token");
        match flows::refresh(&self.http, &self.cfg, &stored.refresh_token).await {
            Ok(tokens) => {
                if let Some(new_refresh) = &tokens.refresh_token {
                    self.store
                        .save(&StoredSession {
                            refresh_token: new_refresh.clone(),
                            scopes: stored.scopes.clone(),
                        })
                        .await?;
                }
                let token = tokens.access_token.clone();
                *cache = Some(Cached {
                    token: tokens.access_token,
                    expires_at: tokens.expires_at,
                });
                Ok(token)
            }
            Err(AuthError::ReauthRequired) => {
                warn!("refresh token rejected; clearing stored session");
                *cache = None;
                self.store.delete().await?;
                Err(AuthError::ReauthRequired)
            }
            Err(e) => Err(e),
        }
    }

    /// The server rejected the cached access token; force a refresh next time.
    pub async fn invalidate(&self) {
        *self.cache.lock().await = None;
    }

    /// Forget the account on this machine.
    pub async fn logout(&self) -> Result<(), AuthError> {
        *self.cache.lock().await = None;
        self.store.delete().await
    }
}
