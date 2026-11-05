use url::Url;

use crate::error::AuthError;

pub const DEFAULT_AUTHORITY: &str = "https://login.microsoftonline.com";

#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Application (client) ID of an Entra public-client registration.
    pub client_id: String,
    /// `organizations` (any work/school account), `common`, or a tenant id/domain.
    pub tenant: String,
    pub scopes: Vec<String>,
    /// Login host. Only changed in tests.
    pub authority: String,
}

impl AuthConfig {
    pub fn new(
        client_id: impl Into<String>,
        tenant: impl Into<String>,
        scopes: Vec<String>,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            tenant: tenant.into(),
            scopes,
            authority: DEFAULT_AUTHORITY.to_owned(),
        }
    }

    pub fn scope_string(&self) -> String {
        self.scopes.join(" ")
    }

    /// Endpoint URL, e.g. `endpoint("token")`. HTTPS only (loopback excepted for tests).
    pub(crate) fn endpoint(&self, name: &str) -> Result<Url, AuthError> {
        let mut url = Url::parse(self.authority.trim_end_matches('/'))
            .map_err(|e| AuthError::Config(format!("invalid authority: {e}")))?;
        let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if url.scheme() != "https" && !loopback {
            return Err(AuthError::Config("authority must use HTTPS".into()));
        }
        url.path_segments_mut()
            .map_err(|_| AuthError::Config("invalid authority".into()))?
            .extend([self.tenant.as_str(), "oauth2", "v2.0", name]);
        Ok(url)
    }

    /// Stable key for keyring entries: one per app registration and tenant.
    pub fn account_key(&self) -> String {
        format!("msgraph:{}:{}", self.client_id, self.tenant)
    }
}
