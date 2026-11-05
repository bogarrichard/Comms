use std::sync::Arc;
use std::time::Duration;

use comms_domain::{ConversationKind, RemoteId, blocks_to_plain_text};
use comms_provider_api::{
    ConversationClass, MessagingProvider, OutgoingMessage, PageRequest, ProviderError,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::Mutex;

use crate::SignalProvider;
use crate::rpc::{RpcClient, RpcError};
use crate::wire::Envelope;

const ME: &str = "+36201111111";
const ANNA: &str = "+36202222222";
const GROUP: &str = "R3JvdXAxMg==";

/// A scripted stand-in for `signal-cli jsonRpc`.
struct FakeCli {
    out: Arc<Mutex<DuplexStream>>,
    requests: Arc<std::sync::Mutex<Vec<Value>>>,
}

#[derive(Clone, Default)]
struct Script {
    /// `result` to answer `send` with; `None` = never answer.
    send: Option<Value>,
    /// `error` object to answer `send` with instead.
    send_error: Option<Value>,
}

impl FakeCli {
    fn start(script: Script) -> (DuplexStream, DuplexStream, FakeCli) {
        let (client_read, server_write) = tokio::io::duplex(1 << 16);
        let (server_read, client_write) = tokio::io::duplex(1 << 16);
        let out = Arc::new(Mutex::new(server_write));
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (o, r) = (out.clone(), requests.clone());
        tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let req: Value = serde_json::from_str(&line).expect("valid request");
                r.lock().expect("lock").push(req.clone());
                let id = req["id"].clone();
                let result = match req["method"].as_str().unwrap_or("") {
                    "listGroups" => Some(json!([
                        {"id": GROUP, "name": "Study group", "isMember": true},
                        {"id": "b3RoZXI=", "name": "Left group", "isMember": false}
                    ])),
                    "listContacts" => Some(json!([
                        {"number": ANNA, "uuid": "uuid-anna", "name": "Anna Kovács"}
                    ])),
                    "send" => {
                        if let Some(err) = &script.send_error {
                            let reply = json!({"jsonrpc":"2.0","id":id,"error":err});
                            write_line(&o, &reply).await;
                            continue;
                        }
                        script.send.clone()
                    }
                    _ => Some(Value::Null),
                };
                if let Some(result) = result {
                    write_line(&o, &json!({"jsonrpc":"2.0","id":id,"result":result})).await;
                }
            }
        });
        (client_read, client_write, FakeCli { out, requests })
    }

    async fn push(&self, envelope: Value) {
        let n = json!({"jsonrpc":"2.0","method":"receive","params":{"envelope":envelope}});
        write_line(&self.out, &n).await;
    }

    fn sent(&self) -> Vec<Value> {
        self.requests
            .lock()
            .expect("lock")
            .iter()
            .filter(|r| r["method"] == "send")
            .cloned()
            .collect()
    }
}

async fn write_line(out: &Mutex<DuplexStream>, v: &Value) {
    let mut w = out.lock().await;
    w.write_all(format!("{v}\n").as_bytes())
        .await
        .expect("write");
    w.flush().await.expect("flush");
}

async fn provider_with(script: Script) -> (SignalProvider, FakeCli) {
    let (r, w, cli) = FakeCli::start(script);
    let p = SignalProvider::with_transport(r, w, ME, None)
        .await
        .expect("connect");
    (p, cli)
}

async fn provider() -> (SignalProvider, FakeCli) {
    provider_with(Script::default()).await
}

fn dm(from: &str, name: &str, ts: i64, text: &str) -> Value {
    json!({
        "sourceNumber": from, "sourceName": name, "timestamp": ts,
        "dataMessage": {"timestamp": ts, "message": text}
    })
}

async fn chats(p: &SignalProvider) -> Vec<comms_provider_api::RemoteConversation> {
    p.list_conversations(ConversationClass::Chats, PageRequest::first(50))
        .await
        .expect("list")
        .items
}

/// The ingest task runs concurrently; wait until `check` holds.
async fn eventually<F, Fut>(mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..200 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition never became true");
}

async fn texts(p: &SignalProvider, conv: &str, limit: u32) -> Vec<String> {
    p.get_messages(&RemoteId::new(conv), PageRequest::first(limit))
        .await
        .expect("messages")
        .items
        .iter()
        .map(|m| blocks_to_plain_text(&m.content))
        .collect()
}

// ------------------------------------------------------------------ receiving

#[tokio::test]
async fn direct_message_creates_a_conversation_named_from_contacts() {
    let (p, cli) = provider().await;
    cli.push(dm(ANNA, "Anna (profile)", 1_700_000_000_000, "hello"))
        .await;
    eventually(|| async {
        !chats(&p)
            .await
            .iter()
            .all(|c| c.kind != ConversationKind::Direct)
    })
    .await;

    let c = chats(&p).await;
    let direct = c
        .iter()
        .find(|c| c.kind == ConversationKind::Direct)
        .expect("dm");
    assert_eq!(direct.remote_id.as_str(), format!("dm:{ANNA}"));
    // The address-book name wins over the profile name in the envelope.
    assert_eq!(direct.title, "Anna Kovács");
    assert_eq!(direct.preview.as_deref(), Some("hello"));
    assert!(!direct.last_message_from_me);
    assert_eq!(texts(&p, &format!("dm:{ANNA}"), 10).await, ["hello"]);
}

#[tokio::test]
async fn unknown_sender_falls_back_to_profile_name_then_number() {
    let (p, cli) = provider().await;
    cli.push(dm("+36203333333", "Zoli", 1_000, "a")).await;
    cli.push(dm("+36204444444", "", 2_000, "b")).await;
    eventually(|| async { chats(&p).await.len() >= 3 }).await;
    let titles: Vec<String> = chats(&p).await.into_iter().map(|c| c.title).collect();
    assert!(titles.contains(&"Zoli".to_owned()), "{titles:?}");
    assert!(titles.contains(&"+36204444444".to_owned()), "{titles:?}");
}

#[tokio::test]
async fn groups_are_listed_even_when_empty_and_messages_land_in_them() {
    let (p, cli) = provider().await;
    let before = chats(&p).await;
    assert_eq!(before.len(), 1, "member group only, not the one we left");
    assert_eq!(before[0].title, "Study group");
    assert_eq!(before[0].kind, ConversationKind::Group);
    assert!(before[0].last_activity.is_none());

    cli.push(json!({
        "sourceNumber": ANNA, "timestamp": 5_000,
        "dataMessage": {"timestamp": 5_000, "message": "exam on the 14th",
                        "groupInfo": {"groupId": GROUP, "type": "DELIVER"}}
    }))
    .await;
    let key = format!("grp:{GROUP}");
    eventually(|| async { !texts(&p, &key, 5).await.is_empty() }).await;

    let after = chats(&p).await;
    assert_eq!(after.len(), 1, "no extra conversation for the group");
    assert_eq!(after[0].preview.as_deref(), Some("exam on the 14th"));
    let m = &p
        .get_messages(&RemoteId::new(&key), PageRequest::first(5))
        .await
        .expect("messages")
        .items[0];
    assert_eq!(m.sender.display_name, "Anna Kovács");
}

#[tokio::test]
async fn messages_sent_from_the_phone_appear_as_mine_and_mark_read() {
    let (p, cli) = provider().await;
    cli.push(dm(ANNA, "", 1_000, "are you there?")).await;
    cli.push(json!({
        "sourceNumber": ME, "timestamp": 2_000,
        "syncMessage": {"sentMessage": {
            "destinationNumber": ANNA, "timestamp": 2_000, "message": "yes!"}}
    }))
    .await;
    let key = format!("dm:{ANNA}");
    eventually(|| async { texts(&p, &key, 5).await.len() == 2 }).await;

    assert_eq!(texts(&p, &key, 5).await, ["yes!", "are you there?"]);
    let c = &chats(&p).await[0];
    assert!(c.last_message_from_me);
    assert_eq!(c.last_read, c.last_activity, "your own reply means read");
}

#[tokio::test]
async fn read_receipts_from_other_devices_update_last_read() {
    let (p, cli) = provider().await;
    cli.push(dm(ANNA, "", 1_000, "hi")).await;
    cli.push(json!({
        "sourceNumber": ME, "timestamp": 3_000,
        "syncMessage": {"readMessages": [{"senderNumber": ANNA, "timestamp": 1_000}]}
    }))
    .await;
    eventually(|| async { chats(&p).await.iter().any(|c| c.last_read.is_some()) }).await;
    let c = &chats(&p).await[0];
    assert_eq!(c.last_read, c.last_activity);
}

#[tokio::test]
async fn non_message_envelopes_are_ignored() {
    let (p, cli) = provider().await;
    cli.push(json!({"sourceNumber": ANNA, "typingMessage": {"action": "STARTED"}}))
        .await;
    cli.push(json!({"sourceNumber": ANNA, "receiptMessage": {"type": "DELIVERY"}}))
        .await;
    cli.push(json!({"sourceNumber": ANNA, "timestamp": 9,
        "dataMessage": {"timestamp": 9, "reaction": {"emoji": "👍"}}}))
        .await;
    // A real message afterwards proves the earlier ones were processed (in order).
    cli.push(dm(ANNA, "", 10, "real")).await;
    let key = format!("dm:{ANNA}");
    eventually(|| async { !texts(&p, &key, 5).await.is_empty() }).await;
    assert_eq!(texts(&p, &key, 5).await, ["real"]);
}

#[tokio::test]
async fn attachment_only_messages_show_a_marker() {
    let (p, cli) = provider().await;
    cli.push(json!({"sourceNumber": ANNA, "timestamp": 7,
        "dataMessage": {"timestamp": 7, "attachments": [
            {"filename": "photo.jpg", "contentType": "image/jpeg"}]}}))
        .await;
    cli.push(json!({"sourceNumber": ANNA, "timestamp": 8,
        "dataMessage": {"timestamp": 8, "message": "look", "attachments": [
            {"contentType": "audio/aac"}]}}))
        .await;
    let key = format!("dm:{ANNA}");
    eventually(|| async { texts(&p, &key, 5).await.len() == 2 }).await;
    assert_eq!(
        texts(&p, &key, 5).await,
        ["look\n[attachment: audio/aac]", "[attachment: photo.jpg]"]
    );
}

#[tokio::test]
async fn duplicate_deliveries_are_stored_once() {
    let (p, cli) = provider().await;
    for _ in 0..3 {
        cli.push(dm(ANNA, "", 42, "once")).await;
    }
    cli.push(dm(ANNA, "", 43, "marker")).await;
    let key = format!("dm:{ANNA}");
    eventually(|| async { texts(&p, &key, 5).await.len() >= 2 }).await;
    assert_eq!(texts(&p, &key, 5).await, ["marker", "once"]);
}

#[tokio::test]
async fn multiline_text_keeps_its_line_breaks() {
    let (p, cli) = provider().await;
    cli.push(dm(ANNA, "", 1, "line one\nline two")).await;
    let key = format!("dm:{ANNA}");
    eventually(|| async { !texts(&p, &key, 5).await.is_empty() }).await;
    assert_eq!(texts(&p, &key, 5).await, ["line one\nline two"]);
}

// --------------------------------------------------------------------- paging

#[tokio::test]
async fn paging_walks_newest_to_oldest_without_gaps_or_repeats() {
    let (p, cli) = provider().await;
    for i in 1..=5 {
        cli.push(dm(ANNA, "", i * 1000, &format!("m{i}"))).await;
    }
    let key = format!("dm:{ANNA}");
    eventually(|| async { texts(&p, &key, 10).await.len() == 5 }).await;

    let id = RemoteId::new(&key);
    let mut req = PageRequest::first(2);
    let mut seen = Vec::new();
    loop {
        let page = p.get_messages(&id, req.clone()).await.expect("page");
        seen.extend(page.items.iter().map(|m| blocks_to_plain_text(&m.content)));
        match page.next {
            Some(c) => req.cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen, ["m5", "m4", "m3", "m2", "m1"]);
}

#[tokio::test]
async fn paging_is_stable_when_new_messages_arrive_between_pages() {
    let (p, cli) = provider().await;
    for i in 1..=4 {
        cli.push(dm(ANNA, "", i * 1000, &format!("m{i}"))).await;
    }
    let key = format!("dm:{ANNA}");
    eventually(|| async { texts(&p, &key, 10).await.len() == 4 }).await;
    let id = RemoteId::new(&key);
    let first = p
        .get_messages(&id, PageRequest::first(2))
        .await
        .expect("page");
    cli.push(dm(ANNA, "", 9_000, "newer")).await;
    eventually(|| async { texts(&p, &key, 10).await.len() == 5 }).await;
    let second = p
        .get_messages(
            &id,
            PageRequest {
                cursor: first.next,
                limit: 2,
            },
        )
        .await
        .expect("page");
    let got: Vec<String> = second
        .items
        .iter()
        .map(|m| blocks_to_plain_text(&m.content))
        .collect();
    assert_eq!(got, ["m2", "m1"]);
}

#[tokio::test]
async fn unknown_conversation_has_no_messages_and_channels_are_empty() {
    let (p, _cli) = provider().await;
    assert!(texts(&p, "dm:+000", 5).await.is_empty());
    let channels = p
        .list_conversations(ConversationClass::Channels, PageRequest::first(5))
        .await
        .expect("channels");
    assert!(channels.items.is_empty());
}

// -------------------------------------------------------------------- sending

#[tokio::test]
async fn send_direct_uses_recipient_and_returns_the_stored_message() {
    let (p, cli) = provider_with(Script {
        send: Some(json!({"timestamp": 123456, "results": [{"type": "SUCCESS"}]})),
        ..Script::default()
    })
    .await;
    let sent = p
        .send_message(
            &RemoteId::new(format!("dm:{ANNA}")),
            OutgoingMessage {
                text: "hi Anna".into(),
            },
        )
        .await
        .expect("sent");
    assert_eq!(sent.remote_id.as_str(), format!("123456:{ME}"));
    assert_eq!(sent.sender.remote_id.as_str(), ME);
    assert_eq!(blocks_to_plain_text(&sent.content), "hi Anna");

    let req = &cli.sent()[0];
    assert_eq!(req["params"]["recipient"], json!([ANNA]));
    assert_eq!(req["params"]["message"], "hi Anna");
    assert!(req["params"].get("groupId").is_none());
    // It is now part of the history too.
    assert_eq!(texts(&p, &format!("dm:{ANNA}"), 5).await, ["hi Anna"]);
}

#[tokio::test]
async fn send_to_a_group_uses_group_id() {
    let (p, cli) = provider_with(Script {
        send: Some(json!({"timestamp": 5, "results": []})),
        ..Script::default()
    })
    .await;
    p.send_message(
        &RemoteId::new(format!("grp:{GROUP}")),
        OutgoingMessage {
            text: "hello all".into(),
        },
    )
    .await
    .expect("sent");
    let req = &cli.sent()[0];
    assert_eq!(req["params"]["groupId"], GROUP);
    assert!(req["params"].get("recipient").is_none());
}

#[tokio::test]
async fn sending_to_a_non_signal_conversation_is_rejected_without_a_request() {
    let (p, cli) = provider().await;
    let err = p
        .send_message(
            &RemoteId::new("19:abc@thread.v2"),
            OutgoingMessage { text: "x".into() },
        )
        .await
        .expect_err("rejected");
    assert!(matches!(err, ProviderError::BadRequest(_)));
    assert!(cli.sent().is_empty());
}

#[tokio::test]
async fn an_error_answer_means_not_delivered_and_safe_to_retry() {
    let (p, _cli) = provider_with(Script {
        send_error: Some(json!({"code": -1, "message": "Unregistered user"})),
        ..Script::default()
    })
    .await;
    let err = p
        .send_message(
            &RemoteId::new(format!("dm:{ANNA}")),
            OutgoingMessage { text: "x".into() },
        )
        .await
        .expect_err("error");
    assert_eq!(err, ProviderError::BadRequest("Unregistered user".into()));
    assert!(err.request_not_applied());
    assert!(
        texts(&p, &format!("dm:{ANNA}"), 5).await.is_empty(),
        "nothing stored"
    );
}

#[tokio::test]
async fn no_answer_is_uncertain_and_never_retry_safe() {
    let (p, cli) = provider_with(Script::default()).await; // never answers `send`
    let p = p.with_send_timeout(Duration::from_millis(100));
    let err = p
        .send_message(
            &RemoteId::new(format!("dm:{ANNA}")),
            OutgoingMessage { text: "x".into() },
        )
        .await
        .expect_err("timeout");
    assert!(
        matches!(
            err,
            ProviderError::Network {
                may_have_reached_server: true,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(!err.request_not_applied());
    assert_eq!(cli.sent().len(), 1, "the request did go out exactly once");
}

#[tokio::test]
async fn delivery_failures_are_classified() {
    for (kind, applied_safe) in [
        ("RATE_LIMIT_FAILURE", true),
        ("UNREGISTERED_FAILURE", true),
        ("IDENTITY_FAILURE", true),
        ("NETWORK_FAILURE", false),
    ] {
        let (p, _cli) = provider_with(Script {
            send: Some(json!({"timestamp": 1, "results": [{"type": kind}]})),
            ..Script::default()
        })
        .await;
        let err = p
            .send_message(
                &RemoteId::new(format!("dm:{ANNA}")),
                OutgoingMessage { text: "x".into() },
            )
            .await
            .expect_err(kind);
        assert_eq!(err.request_not_applied(), applied_safe, "{kind}: {err:?}");
        if kind == "RATE_LIMIT_FAILURE" {
            assert!(matches!(err, ProviderError::RateLimited { .. }));
        }
    }
}

#[tokio::test]
async fn partial_group_success_counts_as_sent() {
    let (p, _cli) = provider_with(Script {
        send: Some(json!({"timestamp": 9, "results": [
            {"type": "UNREGISTERED_FAILURE"}, {"type": "SUCCESS"}]})),
        ..Script::default()
    })
    .await;
    p.send_message(
        &RemoteId::new(format!("grp:{GROUP}")),
        OutgoingMessage { text: "x".into() },
    )
    .await
    .expect("sent");
}

#[tokio::test]
async fn a_success_without_timestamp_is_ambiguous() {
    let (p, _cli) = provider_with(Script {
        send: Some(json!({"results": [{"type": "SUCCESS"}]})),
        ..Script::default()
    })
    .await;
    let err = p
        .send_message(
            &RemoteId::new(format!("dm:{ANNA}")),
            OutgoingMessage { text: "x".into() },
        )
        .await
        .expect_err("ambiguous");
    assert!(matches!(err, ProviderError::InvalidResponse(_)));
    assert!(!err.request_not_applied());
}

// ----------------------------------------------------------------- connection

#[tokio::test]
async fn a_dead_signal_cli_reports_offline_and_sends_nothing() {
    let (client_read, server_write) = tokio::io::duplex(4096);
    let (server_read, client_write) = tokio::io::duplex(4096);
    // Answer the two startup calls, then hang up.
    tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        let mut out = server_write;
        for _ in 0..2 {
            let line = lines.next_line().await.expect("io").expect("line");
            let id = serde_json::from_str::<Value>(&line).expect("json")["id"].clone();
            out.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":id,"result":[]})).as_bytes())
                .await
                .expect("write");
        }
        drop(out);
    });
    let p = SignalProvider::with_transport(client_read, client_write, ME, None)
        .await
        .expect("connect");
    eventually(|| async { p.client.is_closed() }).await;

    let err = p
        .list_conversations(ConversationClass::Chats, PageRequest::first(5))
        .await
        .expect_err("offline");
    assert!(matches!(
        err,
        ProviderError::Network {
            may_have_reached_server: false,
            ..
        }
    ));
    let err = p
        .send_message(
            &RemoteId::new(format!("dm:{ANNA}")),
            OutgoingMessage { text: "x".into() },
        )
        .await
        .expect_err("offline");
    assert!(err.request_not_applied(), "{err:?}");
}

#[tokio::test]
async fn connecting_fails_with_guidance_when_signal_cli_exits_at_once() {
    let (client_read, server_write) = tokio::io::duplex(4096);
    let (_server_read, client_write) = tokio::io::duplex(4096);
    drop(server_write); // EOF immediately: the account is not linked
    let err = SignalProvider::with_transport(client_read, client_write, ME, None)
        .await
        .err()
        .expect("must fail");
    assert!(err.0.contains("comms signal-link"), "{err}");
    assert!(err.0.contains(ME), "{err}");
}

#[tokio::test]
async fn missing_signal_cli_binary_gives_install_guidance() {
    let err = SignalProvider::connect(crate::SignalConfig {
        account: ME.into(),
        cli_path: "/definitely/not/here/signal-cli".into(),
        inbox_file: None,
    })
    .await
    .err()
    .expect("must fail");
    assert!(err.0.contains("Install signal-cli"), "{err}");
}

#[tokio::test]
async fn current_user_is_the_account() {
    let (p, _cli) = provider().await;
    let me = p.current_user().await.expect("me");
    assert_eq!(me.remote_id.as_str(), ME);
    assert_eq!(p.kind().as_str(), "signal");
    let caps = p.capabilities();
    assert!(caps.direct_messages && caps.group_conversations);
    assert!(!caps.channels && !caps.edit_messages && !caps.attachments);
}

// ---------------------------------------------------------------- persistence

#[tokio::test]
async fn received_messages_survive_a_restart_with_a_private_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("sub").join("signal-inbox.jsonl");
    {
        let (r, w, cli) = FakeCli::start(Script::default());
        let p = SignalProvider::with_transport(r, w, ME, Some(file.clone()))
            .await
            .expect("connect");
        cli.push(dm(ANNA, "", 1_000, "keep me")).await;
        let key = format!("dm:{ANNA}");
        eventually(|| async { !texts(&p, &key, 5).await.is_empty() }).await;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).expect("file").permissions().mode();
        assert_eq!(
            mode & 0o077,
            0,
            "inbox must not be group/world readable: {mode:o}"
        );
    }
    // Simulate a crash mid-write: a torn last line must not break loading.
    let mut text = std::fs::read_to_string(&file).expect("read");
    text.push_str("{\"conversation\":\"dm:+3");
    std::fs::write(&file, text).expect("write");

    let (r, w, _cli) = FakeCli::start(Script::default());
    let p = SignalProvider::with_transport(r, w, ME, Some(file))
        .await
        .expect("reconnect");
    assert_eq!(texts(&p, &format!("dm:{ANNA}"), 5).await, ["keep me"]);
}

// ------------------------------------------------------------------- rpc/wire

#[tokio::test]
async fn rpc_ignores_garbage_and_delivers_notifications() {
    let (client_read, mut server_write) = tokio::io::duplex(4096);
    let (_server_read, client_write) = tokio::io::duplex(4096);
    let (_client, mut notes) = RpcClient::new(client_read, client_write);
    server_write
        .write_all(
            b"not json\n\n{\"jsonrpc\":\"2.0\",\"method\":\"receive\",\"params\":{\"a\":1}}\n",
        )
        .await
        .expect("write");
    let n = tokio::time::timeout(Duration::from_secs(2), notes.recv())
        .await
        .expect("in time")
        .expect("notification");
    assert_eq!(n.method, "receive");
    assert_eq!(n.params["a"], 1);
}

#[tokio::test]
async fn rpc_fails_waiting_calls_when_the_connection_ends() {
    let (client_read, server_write) = tokio::io::duplex(4096);
    let (_server_read, client_write) = tokio::io::duplex(4096);
    let (client, _notes) = RpcClient::new(client_read, client_write);
    let call = client.call("x", json!({}), Duration::from_secs(5));
    let hangup = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(server_write);
    };
    let (res, ()) = tokio::join!(call, hangup);
    assert!(matches!(res, Err(RpcError::NoAnswer(_))), "{res:?}");
    assert!(matches!(
        client.call("y", json!({}), Duration::from_secs(1)).await,
        Err(RpcError::NotSent(_))
    ));
}

#[test]
fn envelope_is_found_in_both_notification_shapes() {
    let env = json!({"sourceNumber": ANNA, "dataMessage": {"message": "x"}});
    for params in [
        json!({"envelope": env}),
        json!({"result": {"envelope": env}, "subscription": 0}),
    ] {
        let e = Envelope::from_params(&params).expect("envelope");
        assert_eq!(e.sender().as_deref(), Some(ANNA));
    }
    assert!(Envelope::from_params(&json!({"nothing": 1})).is_none());
}

#[test]
fn sender_prefers_number_then_uuid() {
    let e: Envelope =
        serde_json::from_value(json!({"sourceUuid": "u-1", "source": "legacy"})).expect("parse");
    assert_eq!(e.sender().as_deref(), Some("u-1"));
    let e: Envelope =
        serde_json::from_value(json!({"sourceNumber": "", "sourceUuid": "u-2"})).expect("parse");
    assert_eq!(e.sender().as_deref(), Some("u-2"));
}
