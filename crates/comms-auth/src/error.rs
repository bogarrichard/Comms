use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("not signed in")]
    NotSignedIn,
    /// The refresh token is no longer accepted; the user must sign in again.
    #[error("your session has expired; please sign in again")]
    ReauthRequired,
    /// The configured permissions changed since the last sign-in.
    #[error("the requested permissions changed; please sign in again")]
    ScopesChanged,
    #[error("sign-in was cancelled or denied: {0}")]
    Denied(String),
    #[error("sign-in timed out")]
    Timeout,
    #[error("network error: {0}")]
    Network(String),
    #[error("{description}")]
    Server { code: String, description: String },
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error(
        "no secure credential store is available ({0}). Comms never stores tokens in plaintext; \
         install and unlock a Secret Service provider (e.g. GNOME Keyring or KeePassXC) and try again"
    )]
    NoSecureStore(String),
    #[error("credential store error: {0}")]
    Store(String),
}

impl AuthError {
    /// Plain-language advice for well-known Entra failures, if any.
    pub fn hint(&self) -> Option<&'static str> {
        let AuthError::Server { description, .. } = self else {
            return None;
        };
        let d = description.as_str();
        let has = |c: &str| has_code(d, c);
        if has("AADSTS65001") || has("AADSTS90094") || has("AADSTS65002") || has("AADSTS90008") {
            Some(
                "Your organisation requires administrator approval for one or more requested \
                 permissions. Ask your IT administrator to grant consent, or set `channels = \"off\"` \
                 (or \"basic\") in the Comms config to request fewer permissions.",
            )
        } else if has("AADSTS700016") {
            Some("The application (client) ID was not found. Check `client_id` in the config.")
        } else if has("AADSTS50194") || has("AADSTS700021") {
            Some(
                "The app registration is single-tenant. Set `tenant` in the config to your tenant ID, \
                 or make the registration multi-tenant.",
            )
        } else if has("AADSTS7000218") || has("AADSTS7000215") {
            Some(
                "The app registration does not allow public client flows. In Entra: Authentication \
                 → Advanced settings → Allow public client flows = Yes.",
            )
        } else if has("AADSTS53003")
            || has("AADSTS50076")
            || has("AADSTS50079")
            || has("AADSTS530003")
        {
            Some(
                "A Conditional Access policy blocked this sign-in. If you used --device-code, try the \
                 browser sign-in instead (or the other way round).",
            )
        } else if has("AADSTS50011") {
            Some(
                "The redirect URI is not registered. Add `http://localhost` as a 'Mobile and desktop \
                 applications' redirect URI.",
            )
        } else {
            None
        }
    }
}

/// `true` if `code` occurs in `text` as a whole code (`AADSTS7000218` must not
/// match `AADSTS700021`).
fn has_code(text: &str, code: &str) -> bool {
    text.match_indices(code).any(|(i, _)| {
        !text[i + code.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    })
}
