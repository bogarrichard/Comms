use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use comms_domain::{ConversationKind, RemoteId, blocks_to_plain_text};
use comms_provider_api::{
    ConversationClass, MessagingProvider, OutgoingMessage, PageRequest, ProviderError,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::model::{Target, channel_remote_id, parse_target};

struct Token {
    invalidated: AtomicUsize,
}

#[async_trait]
impl AccessTokenSource for Token {
    async fn access_token(&self) -> Result<String, ProviderError> {
        Ok("test-token".into())
    }
    async fn invalidate(&self) {
        self.invalidated.fetch_add(1, Ordering::SeqCst);
    }
}

fn token() -> Arc<Token> {
    Arc::new(Token {
        invalidated: AtomicUsize::new(0),
    })
}

fn provider_for(base: &str, tokens: Arc<Token>) -> MsGraphProvider {
    let client = GraphClient::new(base, tokens)
        .unwrap()
        .with_retry_policy(RetryPolicy {
            max_attempts: 3,
            max_wait: Duration::from_millis(50),
            backoff_base: Duration::from_millis(1),
        });
    MsGraphProvider::with_client(client)
}

async fn setup() -> (MockServer, MsGraphProvider, Arc<Token>) {
    let server = MockServer::start().await;
    let t = token();
    let p = provider_for(&server.uri(), t.clone());
    (server, p, t)
}

async fn mock_me(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "me-id", "displayName": "Alex Student", "userPrincipalName": "alex@edu.bme.hu"
        })))
        .mount(server)
        .await;
}

fn msg(id: &str, at: &str, from: Value, body_type: &str, body: &str) -> Value {
    json!({
        "id": id, "messageType": "message", "createdDateTime": at,
        "lastModifiedDateTime": at, "lastEditedDateTime": null, "deletedDateTime": null,
        "replyToId": null, "from": from,
        "body": {"contentType": body_type, "content": body},
        "attachments": [], "mentions": [], "reactions": []
    })
}

fn user(id: &str, name: &str) -> Value {
    json!({"application": null, "device": null, "user": {"id": id, "displayName": name, "userIdentityType": "aadUser"}})
}

#[tokio::test]
async fn current_user_is_fetched_with_bearer_token() {
    let (server, p, _) = setup().await;
    mock_me(&server).await;
    let me = p.current_user().await.unwrap();
    assert_eq!(me.remote_id.as_str(), "me-id");
    assert_eq!(me.display_name, "Alex Student");
}

#[tokio::test]
async fn current_user_without_id_is_rejected() {
    let (server, p, _) = setup().await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"displayName": "x"})))
        .mount(&server)
        .await;
    assert!(matches!(
        p.current_user().await,
        Err(ProviderError::InvalidResponse(_))
    ));
}

fn chats_fixture(next: Option<String>) -> Value {
    let mut v = json!({
        "value": [
            {
                "id": "19:one@unq.gbl.spaces", "topic": null, "chatType": "oneOnOne",
                "viewpoint": {"isHidden": false, "lastMessageReadDateTime": "2026-01-01T10:00:00Z"},
                "members": [
                    {"displayName": "Alex Student", "userId": "me-id"},
                    {"displayName": "Anna Kovács", "userId": "anna-id"}
                ],
                "lastMessagePreview": {
                    "id": "1", "createdDateTime": "2026-01-02T10:00:00Z", "messageType": "message",
                    "isDeleted": false,
                    "body": {"contentType": "html", "content": "<p>Are you <b>coming</b>?</p>"},
                    "from": user("anna-id", "Anna Kovács")
                }
            },
            {
                "id": "19:grp@thread.v2", "topic": null, "chatType": "group",
                "viewpoint": {"isHidden": false, "lastMessageReadDateTime": "0001-01-01T00:00:00Z"},
                "members": [
                    {"displayName": "Alex Student", "userId": "me-id"},
                    {"displayName": "A", "userId": "a"}, {"displayName": "B", "userId": "b"},
                    {"displayName": "C", "userId": "c"}, {"displayName": "D", "userId": "d"}
                ],
                "lastMessagePreview": {
                    "id": "2", "createdDateTime": "2026-01-03T10:00:00Z", "messageType": "message",
                    "body": {"contentType": "text", "content": "mine"}, "from": user("me-id", "Alex Student")
                }
            },
            {
                "id": "19:hidden@thread.v2", "chatType": "group", "members": [],
                "viewpoint": {"isHidden": true, "lastMessageReadDateTime": null}
            },
            {
                "id": "19:meeting_x@thread.v2", "topic": "Weekly sync", "chatType": "meeting",
                "viewpoint": {"isHidden": false, "lastMessageReadDateTime": "2026-01-01T00:00:00Z"},
                "members": [],
                "lastMessagePreview": {
                    "id": "3", "createdDateTime": "2026-01-04T10:00:00Z",
                    "messageType": "systemEventMessage", "body": {"contentType": "html", "content": "<systemEventMessage/>"},
                    "from": null
                }
            },
            {"topic": "no id, must be skipped"}
        ]
    });
    if let Some(n) = next {
        v["@odata.nextLink"] = json!(n);
    }
    v
}

#[tokio::test]
async fn chats_are_mapped_filtered_and_titled() {
    let (server, p, _) = setup().await;
    mock_me(&server).await;
    Mock::given(method("GET"))
        .and(path("/me/chats"))
        .and(query_param("$top", "50"))
        .and(query_param("$expand", "members,lastMessagePreview"))
        .and(query_param(
            "$orderby",
            "lastMessagePreview/createdDateTime desc",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(chats_fixture(None)))
        .mount(&server)
        .await;

    let page = p
        .list_conversations(ConversationClass::Chats, PageRequest::first(50))
        .await
        .unwrap();
    assert!(page.next.is_none());
    let items = page.items;
    assert_eq!(items.len(), 3, "hidden and id-less chats are dropped");

    let one = &items[0];
    assert_eq!(one.kind, ConversationKind::Direct);
    assert_eq!(
        one.title, "Anna Kovács",
        "1:1 chats are titled after the other person"
    );
    assert_eq!(one.preview.as_deref(), Some("Are you coming?"));
    assert!(!one.last_message_from_me);
    assert!(one.last_activity.is_some() && one.last_read.is_some());

    let grp = &items[1];
    assert_eq!(grp.kind, ConversationKind::Group);
    assert_eq!(grp.title, "A, B, C +1");
    assert!(grp.last_message_from_me);

    let meeting = &items[2];
    assert_eq!(meeting.kind, ConversationKind::Meeting);
    assert_eq!(meeting.title, "Weekly sync");
    assert!(
        meeting.last_message_from_me,
        "system events never look like unread messages"
    );
    assert_eq!(meeting.preview.as_deref(), Some("(chat event)"));
}

#[tokio::test]
async fn chat_pagination_follows_next_link_and_rejects_foreign_hosts() {
    let (server, p, _) = setup().await;
    mock_me(&server).await;
    let next = format!("{}/me/chats?$skiptoken=abc", server.uri());
    Mock::given(method("GET"))
        .and(path("/me/chats"))
        .and(query_param("$skiptoken", "abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/me/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chats_fixture(Some(next))))
        .mount(&server)
        .await;

    let first = p
        .list_conversations(ConversationClass::Chats, PageRequest::first(50))
        .await
        .unwrap();
    let cursor = first.next.expect("next page");
    let second = p
        .list_conversations(
            ConversationClass::Chats,
            PageRequest {
                cursor: Some(cursor),
                limit: 50,
            },
        )
        .await
        .unwrap();
    assert!(second.items.is_empty() && second.next.is_none());

    // A cursor aimed at another host must never receive our bearer token.
    let evil = PageRequest {
        cursor: Some(comms_domain::PageCursor::new(
            "https://evil.example/me/chats?x=1",
        )),
        limit: 50,
    };
    let err = p
        .list_conversations(ConversationClass::Chats, evil)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidResponse(_)), "{err:?}");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        3,
        "/me, first page, second page - nothing sent for the foreign link"
    );
}

#[tokio::test]
async fn chat_list_falls_back_when_orderby_is_rejected() {
    let (server, p, _) = setup().await;
    mock_me(&server).await;
    Mock::given(method("GET"))
        .and(path("/me/chats"))
        .and(query_param(
            "$orderby",
            "lastMessagePreview/createdDateTime desc",
        ))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {"code": "BadRequest", "message": "orderby not supported with expand"}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/me/chats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chats_fixture(None)))
        .mount(&server)
        .await;
    let page = p
        .list_conversations(ConversationClass::Chats, PageRequest::first(50))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 3);
}

#[tokio::test]
async fn chat_messages_are_mapped_newest_first() {
    let (server, p, _) = setup().await;
    let mut system = msg(
        "s1",
        "2026-01-01T09:00:00Z",
        Value::Null,
        "html",
        "<systemEventMessage/>",
    );
    system["messageType"] = json!("systemEventMessage");
    let mut deleted = msg(
        "d1",
        "2026-01-01T09:30:00Z",
        user("anna-id", "Anna"),
        "html",
        "",
    );
    deleted["deletedDateTime"] = json!("2026-01-01T09:40:00Z");
    let mut edited = msg(
        "e1",
        "2026-01-01T10:00:00Z",
        user("me-id", "Alex"),
        "html",
        "<p>fixed &amp; final</p>",
    );
    edited["lastEditedDateTime"] = json!("2026-01-01T10:05:00Z");
    let mut attached = msg(
        "a1",
        "2026-01-01T11:00:00Z",
        user("anna-id", "Anna"),
        "html",
        "<attachment id=\"9\"></attachment>",
    );
    attached["attachments"] = json!([{"contentType": "reference", "name": "notes.pdf", "contentUrl": "https://x/notes.pdf"}]);
    let bot = msg(
        "b1",
        "2026-01-01T12:00:00Z",
        json!({"application": {"id": "app1", "displayName": "Build Bot"}, "user": null}),
        "text",
        "deployed",
    );
    let bad_time = msg(
        "x1",
        "not-a-date",
        user("anna-id", "Anna"),
        "text",
        "skipped",
    );
    let next = format!(
        "{}/chats/19:c@thread.v2/messages?$skiptoken=zzz",
        server.uri()
    );

    Mock::given(method("GET"))
        .and(path("/chats/19:c@thread.v2/messages"))
        .and(query_param("$top", "50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [system, deleted, edited, attached, bot, bad_time],
            "@odata.nextLink": next
        })))
        .mount(&server)
        .await;

    let page = p
        .get_messages(&RemoteId::new("19:c@thread.v2"), PageRequest::first(50))
        .await
        .unwrap();
    assert!(page.next.is_some());
    let ids: Vec<_> = page.items.iter().map(|m| m.remote_id.as_str()).collect();
    assert_eq!(
        ids,
        ["b1", "a1", "e1", "d1", "s1"],
        "newest first; invalid timestamp skipped"
    );
    assert_eq!(page.items[0].sender.display_name, "Build Bot");
    assert_eq!(page.items[1].attachments[0].name, "notes.pdf");
    assert_eq!(
        blocks_to_plain_text(&page.items[2].content),
        "fixed & final"
    );
    assert!(page.items[2].edited_at.is_some());
    assert!(page.items[3].deleted_at.is_some());
    assert!(page.items[4].is_system);
    assert!(!page.items[0].is_system);
}

#[tokio::test]
async fn channel_messages_include_expanded_replies() {
    let (server, p, _) = setup().await;
    let mut root = msg(
        "r1",
        "2026-02-01T10:00:00Z",
        user("u1", "Prof"),
        "html",
        "Deadline moved <at id=\"0\">Alex</at>",
    );
    let mut reply = msg(
        "r2",
        "2026-02-01T10:30:00Z",
        user("u2", "Anna"),
        "text",
        "Thanks!",
    );
    reply["replyToId"] = json!("r1");
    root["replies"] = json!([reply]);
    Mock::given(method("GET"))
        .and(path("/teams/team-1/channels/19:chan@thread.tacv2/messages"))
        .and(query_param("$expand", "replies"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [root]})))
        .mount(&server)
        .await;
    let id = channel_remote_id("team-1", "19:chan@thread.tacv2");
    let page = p.get_messages(&id, PageRequest::first(50)).await.unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].remote_id.as_str(), "r2");
    assert_eq!(page.items[0].reply_to.as_ref().unwrap().as_str(), "r1");
    assert_eq!(
        blocks_to_plain_text(&page.items[1].content),
        "Deadline moved @Alex"
    );
}

#[tokio::test]
async fn channel_messages_retry_without_expand_when_rejected() {
    let (server, p, _) = setup().await;
    Mock::given(method("GET"))
        .and(path("/teams/t/channels/c/messages"))
        .and(query_param("$expand", "replies"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"error": {"code": "BadRequest", "message": "no expand"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/teams/t/channels/c/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [msg("m1", "2026-02-01T10:00:00Z", user("u", "U"), "text", "hi")]
        })))
        .mount(&server)
        .await;
    let page = p
        .get_messages(&channel_remote_id("t", "c"), PageRequest::first(50))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
}

#[tokio::test]
async fn channel_reading_without_consent_is_forbidden_with_a_useful_message() {
    let (server, p, _) = setup().await;
    Mock::given(method("GET"))
        .and(path("/teams/t/channels/c/messages"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": {"code": "Forbidden", "message": "Missing scope permissions on the request. API requires one of 'ChannelMessage.Read.All'."}
        })))
        .mount(&server)
        .await;
    let err = p
        .get_messages(&channel_remote_id("t", "c"), PageRequest::first(50))
        .await
        .unwrap_err();
    match err {
        ProviderError::Forbidden(m) => assert!(m.contains("ChannelMessage.Read.All"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn channels_are_listed_per_team_skipping_archived_and_tolerating_partial_failure() {
    let (server, p, _) = setup().await;
    Mock::given(method("GET"))
        .and(path("/me/joinedTeams"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [
            {"id": "t1", "displayName": "Software Engineering", "isArchived": false},
            {"id": "t2", "displayName": "Old Course", "isArchived": true},
            {"id": "t3", "displayName": "Broken Team", "isArchived": false}
        ]})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/teams/t1/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [
            {"id": "19:gen@thread.tacv2", "displayName": "General"},
            {"id": "19:lab@thread.tacv2", "displayName": "Lab"}
        ]})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/teams/t3/channels"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(json!({"error": {"code": "Forbidden", "message": "nope"}})),
        )
        .mount(&server)
        .await;

    let page = p
        .list_conversations(ConversationClass::Channels, PageRequest::first(50))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 2);
    assert!(
        page.items
            .iter()
            .all(|c| c.kind == ConversationKind::Channel)
    );
    assert!(
        page.items
            .iter()
            .all(|c| c.group.as_deref() == Some("Software Engineering"))
    );
    assert_eq!(
        parse_target(&page.items[0].remote_id),
        Some(Target::Channel {
            team: "t1".into(),
            channel: "19:gen@thread.tacv2".into()
        })
    );
    let paths: Vec<_> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect();
    assert!(
        !paths.iter().any(|p| p.contains("/teams/t2/")),
        "archived teams are not queried"
    );
}

#[tokio::test]
async fn channel_listing_without_team_permission_is_forbidden() {
    let (server, p, _) = setup().await;
    Mock::given(method("GET"))
        .and(path("/me/joinedTeams"))
        .respond_with(ResponseTemplate::new(403).set_body_json(
            json!({"error": {"code": "Forbidden", "message": "Insufficient privileges"}}),
        ))
        .mount(&server)
        .await;
    let err = p
        .list_conversations(ConversationClass::Channels, PageRequest::first(50))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Forbidden(_)));
}

#[tokio::test]
async fn sending_to_a_chat_posts_escaped_html() {
    let (server, p, _) = setup().await;
    Mock::given(method("POST"))
        .and(path("/chats/19:c@thread.v2/messages"))
        .and(body_json(json!({"body": {"contentType": "html", "content": "a &lt;b&gt; &amp; &quot;c&quot;<br>line2"}})))
        .respond_with(ResponseTemplate::new(201).set_body_json(msg(
            "new1", "2026-03-01T10:00:00Z", user("me-id", "Alex"), "html", "<p>a &lt;b&gt; &amp; \"c\"<br>line2</p>",
        )))
        .expect(1)
        .mount(&server)
        .await;
    let sent = p
        .send_message(
            &RemoteId::new("19:c@thread.v2"),
            OutgoingMessage {
                text: "a <b> & \"c\"\nline2".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(sent.remote_id.as_str(), "new1");
    assert_eq!(blocks_to_plain_text(&sent.content), "a <b> & \"c\"\nline2");
}

#[tokio::test]
async fn sending_to_a_channel_uses_the_channel_endpoint() {
    let (server, p, _) = setup().await;
    Mock::given(method("POST"))
        .and(path("/teams/t1/channels/19:gen@thread.tacv2/messages"))
        .respond_with(ResponseTemplate::new(201).set_body_json(msg(
            "c-new",
            "2026-03-01T10:00:00Z",
            user("me-id", "Alex"),
            "html",
            "hello",
        )))
        .expect(1)
        .mount(&server)
        .await;
    let id = channel_remote_id("t1", "19:gen@thread.tacv2");
    p.send_message(
        &id,
        OutgoingMessage {
            text: "hello".into(),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_failed_send_is_never_retried_on_server_errors() {
    let (server, p, _) = setup().await;
    Mock::given(method("POST"))
        .and(path("/chats/c/messages"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let err = p
        .send_message(&RemoteId::new("c"), OutgoingMessage { text: "x".into() })
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Server { status: 503, .. }));
    assert!(!err.request_not_applied(), "must be treated as uncertain");
}

#[tokio::test]
async fn send_with_garbled_success_response_is_uncertain() {
    let (server, p, _) = setup().await;
    Mock::given(method("POST"))
        .and(path("/chats/c/messages"))
        .respond_with(ResponseTemplate::new(201).set_body_string("<html>proxy page</html>"))
        .expect(1)
        .mount(&server)
        .await;
    let err = p
        .send_message(&RemoteId::new("c"), OutgoingMessage { text: "x".into() })
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidResponse(_)));
    assert!(!err.request_not_applied());
}

#[tokio::test]
async fn throttled_send_is_retried_after_the_advertised_delay() {
    let (server, p, _) = setup().await;
    Mock::given(method("POST"))
        .and(path("/chats/c/messages"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chats/c/messages"))
        .respond_with(ResponseTemplate::new(201).set_body_json(msg(
            "ok",
            "2026-03-01T10:00:00Z",
            user("me", "Me"),
            "text",
            "x",
        )))
        .mount(&server)
        .await;
    let sent = p
        .send_message(&RemoteId::new("c"), OutgoingMessage { text: "x".into() })
        .await
        .unwrap();
    assert_eq!(sent.remote_id.as_str(), "ok");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn persistent_throttling_surfaces_as_rate_limited() {
    let (server, p, _) = setup().await;
    Mock::given(method("GET"))
        .and(path("/chats/c/messages"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .mount(&server)
        .await;
    let err = p
        .get_messages(&RemoteId::new("c"), PageRequest::first(50))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::RateLimited { .. }));
    assert!(err.request_not_applied());
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        3,
        "bounded attempts"
    );
}

#[tokio::test]
async fn retry_after_is_capped_to_the_configured_maximum() {
    let (server, p, _) = setup().await;
    Mock::given(method("GET"))
        .and(path("/chats/c/messages"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
        .mount(&server)
        .await;
    let started = std::time::Instant::now();
    let err = p
        .get_messages(&RemoteId::new("c"), PageRequest::first(50))
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5));
    match err {
        ProviderError::RateLimited { retry_after } => {
            assert!(retry_after <= Duration::from_millis(50))
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn unavailable_service_is_retried_for_reads() {
    let (server, p, _) = setup().await;
    Mock::given(method("GET"))
        .and(path("/chats/c/messages"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/chats/c/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
        .mount(&server)
        .await;
    let page = p
        .get_messages(&RemoteId::new("c"), PageRequest::first(50))
        .await
        .unwrap();
    assert!(page.items.is_empty());
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn expired_token_is_invalidated_once_then_retried() {
    let (server, p, tokens) = setup().await;
    Mock::given(method("GET"))
        .and(path("/chats/c/messages"))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/chats/c/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
        .mount(&server)
        .await;
    p.get_messages(&RemoteId::new("c"), PageRequest::first(50))
        .await
        .unwrap();
    assert_eq!(tokens.invalidated.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn persistent_401_is_unauthorized() {
    let (server, p, tokens) = setup().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let err = p
        .get_messages(&RemoteId::new("c"), PageRequest::first(50))
        .await
        .unwrap_err();
    assert_eq!(err, ProviderError::Unauthorized);
    assert_eq!(tokens.invalidated.load(Ordering::SeqCst), 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn not_found_maps_to_not_found() {
    let (server, p, _) = setup().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    assert_eq!(
        p.get_messages(&RemoteId::new("c"), PageRequest::first(50))
            .await
            .unwrap_err(),
        ProviderError::NotFound
    );
}

#[tokio::test]
async fn connection_failure_proves_the_request_was_not_sent() {
    let p = provider_for("http://127.0.0.1:1", token());
    let err = p
        .send_message(&RemoteId::new("c"), OutgoingMessage { text: "x".into() })
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ProviderError::Network {
                may_have_reached_server: false,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(err.request_not_applied());
}

#[test]
fn plain_http_is_refused_for_remote_hosts() {
    assert!(GraphClient::new("http://graph.example.com/v1.0", token()).is_err());
    assert!(GraphClient::new("https://graph.microsoft.com/v1.0", token()).is_ok());
}

#[test]
fn conversation_ids_round_trip_and_garbage_is_rejected() {
    let id = channel_remote_id("team", "19:abc@thread.tacv2");
    assert_eq!(
        parse_target(&id),
        Some(Target::Channel {
            team: "team".into(),
            channel: "19:abc@thread.tacv2".into()
        })
    );
    assert_eq!(
        parse_target(&RemoteId::new("19:x@thread.v2")),
        Some(Target::Chat("19:x@thread.v2".into()))
    );
    assert_eq!(parse_target(&RemoteId::new("")), None);
    assert_eq!(parse_target(&RemoteId::new("ch/only-team")), None);
    assert_eq!(parse_target(&RemoteId::new("ch//chan")), None);
    assert_eq!(parse_target(&RemoteId::new("ch/a/b/c")), None);
}

#[test]
fn capabilities_are_honest() {
    let p = provider_for("http://127.0.0.1:9", token());
    let c = p.capabilities();
    assert!(c.channels && c.direct_messages && c.group_conversations);
    assert!(!c.realtime_events && !c.edit_messages && !c.delete_messages);
}

#[tokio::test]
async fn unsupported_operations_report_unsupported() {
    let p = provider_for("http://127.0.0.1:9", token());
    let id = RemoteId::new("c");
    assert!(matches!(
        p.edit_message(&id, &id, "x".into()).await,
        Err(ProviderError::Unsupported(_))
    ));
    assert!(matches!(
        p.delete_message(&id, &id).await,
        Err(ProviderError::Unsupported(_))
    ));
}
