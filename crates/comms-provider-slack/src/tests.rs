use std::time::Duration;

use comms_domain::{ContentBlock, ConversationKind, Inline, PageCursor, RemoteId};
use comms_provider_api::{
    Capability, ConversationClass, MessagingProvider, OutgoingMessage, PageRequest, ProviderError,
};
use serde_json::json;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::{SlackProvider, looks_like_token};

const TOKEN: &str = "xoxp-test-token-123";

async fn setup() -> (MockServer, SlackProvider) {
    let server = MockServer::start().await;
    let provider = SlackProvider::with_base_url(TOKEN, &format!("{}/api", server.uri())).unwrap();
    (server, provider)
}

async fn answer(server: &MockServer, api_method: &str, body: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path(format!("/api/{api_method}")))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn user(server: &MockServer, id: &str, display: &str) {
    Mock::given(method("POST"))
        .and(path("/api/users.info"))
        .and(body_string_contains(format!("user={id}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "user": {"name": id.to_lowercase(), "profile": {"display_name": display}}
        })))
        .mount(server)
        .await;
}

#[test]
fn token_shape_check() {
    assert!(looks_like_token("xoxp-1234-abcdef"));
    assert!(!looks_like_token("hello world"));
    assert!(!looks_like_token("xoxp"));
    assert!(!looks_like_token(""));
}

#[tokio::test]
async fn debug_output_never_shows_the_token() {
    let (_server, provider) = setup().await;
    assert!(!format!("{:?}", provider.client).contains(TOKEN));
}

#[tokio::test]
async fn rejects_plain_http_to_a_remote_host() {
    let err = SlackProvider::with_base_url(TOKEN, "http://slack.example/api")
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidResponse(_)));
}

#[tokio::test]
async fn current_user_comes_from_auth_test_and_users_info() {
    let (server, p) = setup().await;
    answer(
        &server,
        "auth.test",
        json!({"ok": true, "user_id": "U1", "user": "alice"}),
    )
    .await;
    user(&server, "U1", "Alice A.").await;
    let me = p.current_user().await.unwrap();
    assert_eq!(me.remote_id.as_str(), "U1");
    assert_eq!(me.display_name, "Alice A.");
}

#[tokio::test]
async fn lists_chats_with_resolved_names_and_a_cursor() {
    let (server, p) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/conversations.list"))
        .and(body_string_contains("types=im%2Cmpim"))
        .and(body_string_contains("cursor=c1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "channels": [
                {"id": "D1", "is_im": true, "user": "U2"},
                {"id": "G1", "is_mpim": true, "name": "mpdm-alice--bob--carol-1"}
            ],
            "response_metadata": {"next_cursor": "c2"}
        })))
        .mount(&server)
        .await;
    user(&server, "U2", "Bob").await;

    let page = p
        .list_conversations(
            ConversationClass::Chats,
            PageRequest {
                cursor: Some(PageCursor::new("c1")),
                limit: 50,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].kind, ConversationKind::Direct);
    assert_eq!(page.items[0].title, "Bob");
    assert_eq!(page.items[1].kind, ConversationKind::Group);
    assert_eq!(page.items[1].title, "alice, bob, carol");
    assert_eq!(page.next.unwrap().as_str(), "c2");
}

#[tokio::test]
async fn lists_channels_and_an_empty_cursor_ends_paging() {
    let (server, p) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/conversations.list"))
        .and(body_string_contains(
            "types=public_channel%2Cprivate_channel",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "channels": [{"id": "C1", "name": "general"}],
            "response_metadata": {"next_cursor": ""}
        })))
        .mount(&server)
        .await;
    let page = p
        .list_conversations(ConversationClass::Channels, PageRequest::first(0))
        .await
        .unwrap();
    assert_eq!(page.items[0].title, "#general");
    assert_eq!(page.items[0].kind, ConversationKind::Channel);
    assert!(page.next.is_none());
}

#[tokio::test]
async fn maps_messages_newest_first_with_mentions_edits_files_and_threads() {
    let (server, p) = setup().await;
    answer(
        &server,
        "conversations.history",
        json!({
            "ok": true,
            "messages": [
                {"ts": "1700000002.000000", "user": "U1", "text": "hey <@U2>, see <https://x.example|this>",
                 "edited": {"user": "U1", "ts": "1700000003.000000"},
                 "files": [{"name": "plan.pdf"}]},
                {"ts": "1700000001.000000", "user": "U2", "text": "reply",
                 "thread_ts": "1700000000.000000"},
                {"ts": "1700000000.000000", "user": "U1", "text": "joined", "subtype": "channel_join"},
                {"ts": "1700000000.5", "bot_id": "B1", "username": "ci", "text": "built",
                 "subtype": "bot_message"}
            ],
            "has_more": false
        }),
    )
    .await;
    user(&server, "U1", "Alice").await;
    user(&server, "U2", "Bob").await;

    let page = p
        .get_messages(&RemoteId::new("C1"), PageRequest::first(20))
        .await
        .unwrap();
    assert!(page.next.is_none());
    let m = &page.items;
    assert_eq!(m.len(), 4);

    assert_eq!(m[0].sender.display_name, "Alice");
    assert_eq!(m[0].remote_id.as_str(), "1700000002.000000");
    assert!(m[0].edited_at.is_some());
    assert!(matches!(
        &m[0].content[0],
        ContentBlock::Paragraph { inlines }
            if inlines.contains(&Inline::Mention { name: "Bob".into() })
    ));
    assert!(matches!(
        m[0].content.last(),
        Some(ContentBlock::Unsupported { fallback, .. }) if fallback == "[file: plan.pdf]"
    ));
    assert_eq!(
        m[1].reply_to.as_ref().unwrap().as_str(),
        "1700000000.000000"
    );
    assert!(m[2].is_system);
    assert_eq!(m[3].sender.display_name, "ci");
    assert!(!m[3].is_system);
    assert!(m[0].timestamp > m[1].timestamp);
}

#[tokio::test]
async fn send_posts_the_text_as_the_user_and_returns_the_message() {
    let (server, p) = setup().await;
    answer(
        &server,
        "auth.test",
        json!({"ok": true, "user_id": "U1", "user": "alice"}),
    )
    .await;
    user(&server, "U1", "Alice").await;
    Mock::given(method("POST"))
        .and(path("/api/chat.postMessage"))
        .and(body_string_contains("channel=C1"))
        .and(body_string_contains("text=hello+world"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "channel": "C1", "ts": "1700000005.000200",
            "message": {"ts": "1700000005.000200", "text": "hello world"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let sent = p
        .send_message(
            &RemoteId::new("C1"),
            OutgoingMessage {
                text: "hello world".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(sent.remote_id.as_str(), "1700000005.000200");
    assert_eq!(sent.sender.display_name, "Alice");
}

#[tokio::test]
async fn edit_and_delete_use_the_message_timestamp() {
    let (server, p) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/chat.update"))
        .and(body_string_contains("ts=1.5"))
        .and(body_string_contains("text=fixed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/chat.delete"))
        .and(body_string_contains("ts=1.5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;
    let (c, m) = (RemoteId::new("C1"), RemoteId::new("1.5"));
    p.edit_message(&c, &m, "fixed".into()).await.unwrap();
    p.delete_message(&c, &m).await.unwrap();
}

#[tokio::test]
async fn slack_error_codes_map_to_retry_safety() {
    let (server, p) = setup().await;
    let c = RemoteId::new("C1");
    let cases = [
        ("invalid_auth", ProviderError::Unauthorized, true),
        ("channel_not_found", ProviderError::NotFound, true),
        (
            "msg_too_long",
            ProviderError::BadRequest("msg_too_long".into()),
            true,
        ),
        (
            "ratelimited",
            ProviderError::RateLimited {
                retry_after: Duration::from_secs(30),
            },
            true,
        ),
        (
            "internal_error",
            ProviderError::Server {
                status: 500,
                message: "internal_error".into(),
            },
            false,
        ),
        (
            "brand_new_error",
            ProviderError::InvalidResponse("unrecognised Slack error `brand_new_error`".into()),
            false,
        ),
    ];
    for (code, expected, safe) in cases {
        server.reset().await;
        answer(&server, "chat.delete", json!({"ok": false, "error": code})).await;
        let err = p
            .delete_message(&c, &RemoteId::new("1.0"))
            .await
            .unwrap_err();
        assert_eq!(err, expected, "{code}");
        assert_eq!(err.request_not_applied(), safe, "{code}");
    }
}

#[tokio::test]
async fn missing_scope_names_the_permission_to_add() {
    let (server, p) = setup().await;
    answer(
        &server,
        "conversations.history",
        json!({"ok": false, "error": "missing_scope", "needed": "im:history", "provided": "x"}),
    )
    .await;
    let err = p
        .get_messages(&RemoteId::new("D1"), PageRequest::first(5))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ProviderError::Forbidden(m) if m.contains("im:history")),
        "{err:?}"
    );
}

#[tokio::test]
async fn http_429_is_rate_limited_with_the_retry_after_header() {
    let (server, p) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/conversations.history"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "60"))
        .expect(1) // not retried behind the engine's back
        .mount(&server)
        .await;
    let err = p
        .get_messages(&RemoteId::new("C1"), PageRequest::first(5))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        ProviderError::RateLimited {
            retry_after: Duration::from_secs(60)
        }
    );
}

#[tokio::test]
async fn a_5xx_on_send_is_ambiguous_and_never_retried() {
    let (server, p) = setup().await;
    answer(
        &server,
        "auth.test",
        json!({"ok": true, "user_id": "U1", "user": "alice"}),
    )
    .await;
    user(&server, "U1", "Alice").await;
    Mock::given(method("POST"))
        .and(path("/api/chat.postMessage"))
        .respond_with(ResponseTemplate::new(502))
        .expect(1)
        .mount(&server)
        .await;
    let err = p
        .send_message(&RemoteId::new("C1"), OutgoingMessage { text: "x".into() })
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Server { status: 502, .. }));
    assert!(!err.request_not_applied());
}

#[tokio::test]
async fn unsupported_operations_say_so() {
    let (_server, p) = setup().await;
    assert!(matches!(
        p.search_messages("q", PageRequest::first(5)).await,
        Err(ProviderError::Unsupported(Capability::Search))
    ));
    let caps = p.capabilities();
    assert!(caps.channels && caps.direct_messages && caps.edit_messages);
    assert!(!caps.realtime_events && !caps.search);
}
