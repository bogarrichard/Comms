use std::time::Duration;

use thiserror::Error;

/// A named provider feature, used in [`ProviderError::Unsupported`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    SendMessages,
    GetConversation,
    EditMessages,
    DeleteMessages,
    Search,
    ListParticipants,
    Channels,
    Reactions,
    Presence,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ProviderError {
    #[error("operation not supported by this provider: {0:?}")]
    Unsupported(Capability),
    #[error("not signed in or session expired")]
    Unauthorized,
    /// Authenticated but not allowed — typically a missing permission or
    /// missing admin consent.
    #[error("access denied: {0}")]
    Forbidden(String),
    #[error("not found")]
    NotFound,
    #[error("rejected by provider: {0}")]
    BadRequest(String),
    #[error("rate limited; retry after {retry_after:?}")]
    RateLimited { retry_after: Duration },
    /// Transport failure. `may_have_reached_server` is `false` only when the
    /// request provably was not delivered (DNS, connect or TLS failure).
    #[error("network error: {message}")]
    Network {
        may_have_reached_server: bool,
        message: String,
    },
    #[error("provider returned an unexpected response: {0}")]
    InvalidResponse(String),
    #[error("provider error (HTTP {status}): {message}")]
    Server { status: u16, message: String },
}

impl ProviderError {
    /// `true` when the failed request is guaranteed not to have been applied,
    /// so repeating it cannot create a duplicate. Anything ambiguous is
    /// `false` (timeouts, resets, 5xx, malformed success responses).
    pub fn request_not_applied(&self) -> bool {
        match self {
            ProviderError::Unsupported(_)
            | ProviderError::Unauthorized
            | ProviderError::Forbidden(_)
            | ProviderError::NotFound
            | ProviderError::BadRequest(_)
            | ProviderError::RateLimited { .. } => true,
            ProviderError::Network {
                may_have_reached_server,
                ..
            } => !may_have_reached_server,
            ProviderError::Server { status, .. } => (400..500).contains(status),
            ProviderError::InvalidResponse(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_safety_classification() {
        use ProviderError::*;
        assert!(Unauthorized.request_not_applied());
        assert!(
            RateLimited {
                retry_after: Duration::from_secs(1)
            }
            .request_not_applied()
        );
        assert!(
            Network {
                may_have_reached_server: false,
                message: "dns".into()
            }
            .request_not_applied()
        );
        assert!(
            !Network {
                may_have_reached_server: true,
                message: "timeout".into()
            }
            .request_not_applied()
        );
        assert!(
            Server {
                status: 403,
                message: String::new()
            }
            .request_not_applied()
        );
        assert!(
            !Server {
                status: 502,
                message: String::new()
            }
            .request_not_applied()
        );
        assert!(!InvalidResponse("bad json".into()).request_not_applied());
    }
}
