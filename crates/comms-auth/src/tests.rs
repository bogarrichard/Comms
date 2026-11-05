use std::sync::Arc;

use serde_json::json;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;
use crate::flows::pkce_challenge;

fn cfg(server: &MockServer) -> AuthConfig {
    let mut c = AuthConfig::new(
        "client-123",
        "organizations",
        vec![
            "openid".into(),
            "offline_access".into(),
            "User.Read".into(),
            "Chat.Read".into(),
        ],
    );
    c.authority = server.uri();
    c
}

fn token_json(access: &str, refresh: Option<&str>, expires_in: i64) -> serde_json::Value {
    let mut v = json!({"access_token": access, "token_type": "Bearer", "expires_in": expires_in});
    if let Some(r) = refresh {
        v["refresh_token"] = json!(r);
    }
    v
}

fn form(req: &Request) -> Vec<(String, String)> {
    url::form_urlencoded::parse(&req.body)
        .into_owned()
        .collect()
}

fn field(req: &Request, k: &str) -> Option<String> {
    form(req).into_iter().find(|(n, _)| n == k).map(|(_, v)| v)
}

fn session(server: &MockServer) -> (Session, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    (Session::new(cfg(server), store.clone()).unwrap(), store)
}

const TOKEN_PATH: &str = "/organizations/oauth2/v2.0/token";

#[test]
fn https_is_required_for_remote_authorities() {
    let mut c = AuthConfig::new("c", "organizations", vec![]);
    c.authority = "http://login.example.com".into();
    assert!(matches!(c.endpoint("token"), Err(AuthError::Config(_))));
    c.authority = DEFAULT_AUTHORITY_FOR_TEST.into();
    assert_eq!(
        c.endpoint("token").unwrap().as_str(),
        "https://login.microsoftonline.com/organizations/oauth2/v2.0/token"
    );
}
const DEFAULT_AUTHORITY_FOR_TEST: &str = "https://login.microsoftonline.com";

#[test]
fn pkce_challenge_matches_rfc7636_example() {
    // RFC 7636 appendix B.
    assert_eq!(
        pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[tokio::test]
async fn browser_login_completes_the_pkce_exchange_and_stores_only_the_refresh_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(TOKEN_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_json(
            "AT1",
            Some("RT1"),
            3600,
        )))
        .mount(&server)
        .await;
    let (s, store) = session(&server);

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let login = s.login_browser(move |url| {
        let _ = tx.send(url.to_owned());
    });
    let browser = async {
        let url = Url::parse(&rx.await.unwrap()).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "client-123");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["response_type"], "code");
        assert!(q["scope"].contains("Chat.Read"));
        assert!(q["redirect_uri"].starts_with("http://localhost:"));
        // Impersonate the browser being redirected back.
        let cb = format!("{}/?code=the-code&state={}", q["redirect_uri"], q["state"]);
        let cb = cb.replace("localhost", "127.0.0.1");
        reqwest::get(cb).await.unwrap().text().await.unwrap();
        q
    };
    let (res, q) = tokio::join!(login, browser);
    res.unwrap();

    let reqs = server.received_requests().await.unwrap();
    let exchange = reqs.iter().find(|r| r.url.path() == TOKEN_PATH).unwrap();
    assert_eq!(
        field(exchange, "grant_type").as_deref(),
        Some("authorization_code")
    );
    assert_eq!(field(exchange, "code").as_deref(), Some("the-code"));
    let verifier = field(exchange, "code_verifier").unwrap();
    assert!(verifier.len() >= 43, "RFC 7636 minimum verifier length");
    assert_eq!(
        pkce_challenge(&verifier),
        q["code_challenge"],
        "challenge binds the verifier"
    );
    assert_eq!(field(exchange, "redirect_uri").unwrap(), q["redirect_uri"]);
    assert!(
        field(exchange, "client_secret").is_none(),
        "public client: no secret"
    );

    let saved = store.load().await.unwrap().unwrap();
    assert_eq!(saved.refresh_token, "RT1");
    assert!(
        !format!("{saved:?}").contains("RT1"),
        "Debug output redacts the token"
    );
    // The cached access token is served without another request.
    assert_eq!(s.access_token().await.unwrap(), "AT1");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn browser_login_rejects_a_mismatched_state() {
    let server = MockServer::start().await;
    let (s, store) = session(&server);
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let login = s.login_browser(move |url| {
        let _ = tx.send(url.to_owned());
    });
    let browser = async {
        let url = Url::parse(&rx.await.unwrap()).unwrap();
        let ru = url
            .query_pairs()
            .find(|(k, _)| k == "redirect_uri")
            .unwrap()
            .1
            .replace("localhost", "127.0.0.1");
        let _ = reqwest::get(format!("{ru}/?code=evil&state=forged")).await;
    };
    let (res, ()) = tokio::join!(login, browser);
    assert!(matches!(res, Err(AuthError::Denied(_))), "{res:?}");
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "no code exchange happened"
    );
    assert!(store.load().await.unwrap().is_none());
}

#[tokio::test]
async fn browser_login_reports_denied_consent() {
    let server = MockServer::start().await;
    let (s, _) = session(&server);
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let login = s.login_browser(move |url| {
        let _ = tx.send(url.to_owned());
    });
    let browser = async {
        let url = Url::parse(&rx.await.unwrap()).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        let ru = q["redirect_uri"].replace("localhost", "127.0.0.1");
        // A stray request (favicon) must not end the flow.
        let _ = reqwest::get(format!("{ru}/favicon.ico")).await;
        let _ = reqwest::get(format!(
            "{ru}/?error=access_denied&error_description=AADSTS65004%3A+User+declined&state={}",
            q["state"]
        ))
        .await;
    };
    let (res, ()) = tokio::join!(login, browser);
    assert!(matches!(res, Err(AuthError::Denied(m)) if m.contains("declined")));
}

#[tokio::test]
async fn device_code_flow_polls_until_the_user_finishes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/organizations/oauth2/v2.0/devicecode"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": "DC", "user_code": "ABCD-1234",
            "verification_uri": "https://microsoft.com/devicelogin",
            "expires_in": 900, "interval": 0, "message": "go sign in"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(TOKEN_PATH))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(
                json!({"error": "authorization_pending", "error_description": "wait"}),
            ),
        )
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(TOKEN_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_json("AT", Some("RT"), 3600)))
        .mount(&server)
        .await;
    let (s, store) = session(&server);
    let mut shown = None;
    s.login_device_code(|p| shown = Some(p.user_code.clone()))
        .await
        .unwrap();
    assert_eq!(shown.as_deref(), Some("ABCD-1234"));
    assert_eq!(store.load().await.unwrap().unwrap().refresh_token, "RT");
    let polls = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == TOKEN_PATH)
        .count();
    assert_eq!(polls, 3);
}

#[tokio::test]
async fn device_code_flow_reports_decline_and_expiry() {
    for (code, expect_timeout) in [("authorization_declined", false), ("expired_token", true)] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/organizations/oauth2/v2.0/devicecode"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "device_code": "DC", "user_code": "X", "verification_uri": "https://v", "expires_in": 900, "interval": 0
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(json!({"error": code, "error_description": "d"})),
            )
            .mount(&server)
            .await;
        let (s, _) = session(&server);
        let res = s.login_device_code(|_| {}).await;
        if expect_timeout {
            assert!(matches!(res, Err(AuthError::Timeout)), "{res:?}");
        } else {
            assert!(matches!(res, Err(AuthError::Denied(_))), "{res:?}");
        }
    }
}

async fn signed_in_session(server: &MockServer, expires_in: i64) -> (Session, Arc<MemoryStore>) {
    let (s, store) = session(server);
    store
        .save(&StoredSession {
            refresh_token: "RT0".into(),
            scopes: "chat.read user.read".into(),
        })
        .await
        .unwrap();
    // First token fetch triggers a refresh with the configured lifetime.
    Mock::given(method("POST"))
        .and(path(TOKEN_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_json(
            "AT-new",
            Some("RT-rotated"),
            expires_in,
        )))
        .mount(server)
        .await;
    (s, store)
}

#[tokio::test]
async fn access_token_is_refreshed_once_cached_and_the_refresh_token_rotates() {
    let server = MockServer::start().await;
    let (s, store) = signed_in_session(&server, 3600).await;
    assert!(s.is_signed_in().await.unwrap());
    assert_eq!(s.access_token().await.unwrap(), "AT-new");
    assert_eq!(s.access_token().await.unwrap(), "AT-new");
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1, "second call served from cache");
    assert_eq!(
        field(&reqs[0], "grant_type").as_deref(),
        Some("refresh_token")
    );
    assert_eq!(field(&reqs[0], "refresh_token").as_deref(), Some("RT0"));
    assert_eq!(
        store.load().await.unwrap().unwrap().refresh_token,
        "RT-rotated"
    );
}

#[tokio::test]
async fn nearly_expired_tokens_are_refreshed_and_invalidate_forces_a_refresh() {
    let server = MockServer::start().await;
    let (s, _) = signed_in_session(&server, 30).await; // inside the 2-minute safety margin
    s.access_token().await.unwrap();
    s.access_token().await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);

    let server2 = MockServer::start().await;
    let (s2, _) = signed_in_session(&server2, 3600).await;
    s2.access_token().await.unwrap();
    s2.invalidate().await;
    s2.access_token().await.unwrap();
    assert_eq!(server2.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn concurrent_callers_share_a_single_refresh() {
    let server = MockServer::start().await;
    let (s, _) = signed_in_session(&server, 3600).await;
    let s = Arc::new(s);
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let s = s.clone();
            tokio::spawn(async move { s.access_token().await.unwrap() })
        })
        .collect();
    for h in handles {
        assert_eq!(h.await.unwrap(), "AT-new");
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn revoked_refresh_token_requires_reauth_and_clears_the_store() {
    let server = MockServer::start().await;
    let (s, store) = session(&server);
    store
        .save(&StoredSession {
            refresh_token: "RT0".into(),
            scopes: "chat.read user.read".into(),
        })
        .await
        .unwrap();
    Mock::given(method("POST"))
        .and(path(TOKEN_PATH))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant", "error_description": "AADSTS70008: The refresh token has expired."
        })))
        .mount(&server)
        .await;
    assert!(matches!(
        s.access_token().await,
        Err(AuthError::ReauthRequired)
    ));
    assert!(store.load().await.unwrap().is_none());
    assert!(matches!(
        s.access_token().await,
        Err(AuthError::NotSignedIn)
    ));
}

#[tokio::test]
async fn changed_permissions_require_a_new_sign_in_without_contacting_the_server() {
    let server = MockServer::start().await;
    let (s, store) = session(&server);
    store
        .save(&StoredSession {
            refresh_token: "RT0".into(),
            scopes: "user.read".into(),
        })
        .await
        .unwrap();
    assert!(!s.is_signed_in().await.unwrap());
    assert!(matches!(
        s.access_token().await,
        Err(AuthError::ScopesChanged)
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn network_failure_during_refresh_keeps_the_session() {
    let mut c = AuthConfig::new("c", "organizations", vec!["User.Read".into()]);
    c.authority = "http://127.0.0.1:1".into();
    let store = Arc::new(MemoryStore::default());
    store
        .save(&StoredSession {
            refresh_token: "RT".into(),
            scopes: "user.read".into(),
        })
        .await
        .unwrap();
    let s = Session::new(c, store.clone()).unwrap();
    assert!(matches!(s.access_token().await, Err(AuthError::Network(_))));
    assert!(
        store.load().await.unwrap().is_some(),
        "offline must not sign the user out"
    );
}

#[tokio::test]
async fn logout_forgets_everything() {
    let server = MockServer::start().await;
    let (s, store) = signed_in_session(&server, 3600).await;
    s.access_token().await.unwrap();
    s.logout().await.unwrap();
    assert!(store.load().await.unwrap().is_none());
    assert!(matches!(
        s.access_token().await,
        Err(AuthError::NotSignedIn)
    ));
}

#[test]
fn well_known_entra_errors_get_actionable_hints() {
    let e = |d: &str| AuthError::Server {
        code: "x".into(),
        description: d.into(),
    };
    assert!(
        e("AADSTS65001: The user or administrator has not consented")
            .hint()
            .unwrap()
            .contains("administrator")
    );
    assert!(
        e("AADSTS90094: The grant requires admin permission")
            .hint()
            .unwrap()
            .contains("channels")
    );
    assert!(
        e("AADSTS7000218: client_assertion required")
            .hint()
            .unwrap()
            .contains("public client")
    );
    assert!(
        e("AADSTS700016: Application not found")
            .hint()
            .unwrap()
            .contains("client")
    );
    assert!(
        e("AADSTS53003: blocked by Conditional Access")
            .hint()
            .unwrap()
            .contains("Conditional Access")
    );
    assert!(
        e("AADSTS700021: single tenant")
            .hint()
            .unwrap()
            .contains("single-tenant")
    );
    assert!(e("something else").hint().is_none());
}
