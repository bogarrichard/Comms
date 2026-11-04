use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::{Duration as Chrono, Utc};
use comms_domain::{ConversationKind, SendState};
use comms_provider_api::ProviderError;
use comms_storage::Store;
use comms_testkit::{FakeProvider, SendScript, conversation, person, text_message};
use tokio::sync::broadcast::Receiver;

use super::*;

fn fast_config() -> CoreConfig {
    CoreConfig {
        chat_poll: Duration::from_secs(3600),
        channel_poll: Duration::from_secs(3600),
        open_poll: Duration::from_secs(3600),
        reconcile_delay: Duration::from_millis(30),
        ..CoreConfig::default()
    }
}

async fn start(provider: Arc<FakeProvider>) -> (AppHandle, Receiver<AppEvent>) {
    let store = Store::open_in_memory().await.unwrap();
    let handle = AppHandle::start(provider, store, fast_config())
        .await
        .unwrap();
    let rx = handle.subscribe();
    (handle, rx)
}

async fn wait_for(rx: &mut Receiver<AppEvent>, pred: impl Fn(&AppEvent) -> bool) -> AppEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Ok(e) if pred(&e) => return e,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(e) => panic!("event channel closed: {e}"),
            }
        }
    })
    .await
    .expect("timed out waiting for event")
}

async fn conv_by_title(h: &AppHandle, title: &str) -> comms_domain::Conversation {
    h.conversations()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.title == title)
        .expect("conversation")
}

async fn wait_for_conversations(h: &AppHandle, rx: &mut Receiver<AppEvent>, n: usize) {
    for _ in 0..50 {
        if h.conversations().await.unwrap().len() >= n {
            return;
        }
        wait_for(rx, |e| matches!(e, AppEvent::ConversationsUpdated)).await;
    }
    panic!("conversations never reached {n}");
}

#[tokio::test]
async fn startup_lists_chats_and_channels() {
    let (h, mut rx) = start(Arc::new(FakeProvider::demo())).await;
    wait_for_conversations(&h, &mut rx, 7).await;
    let convs = h.conversations().await.unwrap();
    assert_eq!(
        convs
            .iter()
            .filter(|c| c.kind == ConversationKind::Channel)
            .count(),
        4
    );
    assert_eq!(convs[0].title, "Anna Kovács", "most recent chat first");
    assert!(convs[0].unread, "server says unread");
    assert_eq!(h.connection_status(), ConnectionStatus::Online);
}

#[tokio::test]
async fn opening_a_conversation_loads_messages_and_clears_unread() {
    let (h, mut rx) = start(Arc::new(FakeProvider::demo())).await;
    wait_for_conversations(&h, &mut rx, 7).await;
    let c = conv_by_title(&h, "Anna Kovács").await;
    assert!(
        h.messages(c.id, 100).await.unwrap().is_empty(),
        "nothing cached before first open"
    );

    h.send(Command::OpenConversation(c.id));
    wait_for(&mut rx, |e| matches!(e, AppEvent::MessagesUpdated(_))).await;
    let msgs = h.messages(c.id, 100).await.unwrap();
    assert_eq!(msgs.len(), 4);
    assert!(
        msgs.windows(2).all(|w| w[0].timestamp <= w[1].timestamp),
        "oldest first"
    );
    assert!(!conv_by_title(&h, "Anna Kovács").await.unread);
}

#[tokio::test]
async fn cached_messages_are_available_without_waiting_for_the_network() {
    let store = Store::open_in_memory().await.unwrap();
    let provider = Arc::new(FakeProvider::demo());
    let h = AppHandle::start(provider.clone(), store.clone(), fast_config())
        .await
        .unwrap();
    let mut rx = h.subscribe();
    wait_for_conversations(&h, &mut rx, 7).await;
    let c = conv_by_title(&h, "Anna Kovács").await;
    h.send(Command::OpenConversation(c.id));
    wait_for(&mut rx, |e| {
        matches!(
            e,
            AppEvent::SyncCompleted {
                conversation: Some(_)
            }
        )
    })
    .await;
    drop(h);

    // Second launch: the provider now takes 10 s to answer anything.
    let slow = Arc::new(FakeProvider::demo().with_latency(Duration::from_secs(10)));
    let started = std::time::Instant::now();
    let h2 = AppHandle::start(slow, store, fast_config()).await.unwrap();
    let convs = h2.conversations().await.unwrap();
    let msgs = h2.messages(c.id, 100).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "startup must not wait for the network"
    );
    assert_eq!(convs.len(), 7);
    assert_eq!(msgs.len(), 4);
}

#[tokio::test]
async fn repeated_syncs_do_not_duplicate_messages() {
    let provider = Arc::new(FakeProvider::demo());
    let (h, mut rx) = start(provider.clone()).await;
    wait_for_conversations(&h, &mut rx, 7).await;
    let c = conv_by_title(&h, "Anna Kovács").await;
    for _ in 0..3 {
        h.send(Command::OpenConversation(c.id));
        wait_for(&mut rx, |e| {
            matches!(
                e,
                AppEvent::SyncCompleted {
                    conversation: Some(_)
                }
            )
        })
        .await;
    }
    assert_eq!(h.messages(c.id, 100).await.unwrap().len(), 4);
}

#[tokio::test]
async fn incoming_messages_arrive_on_refresh() {
    let provider = Arc::new(FakeProvider::demo());
    let (h, mut rx) = start(provider.clone()).await;
    wait_for_conversations(&h, &mut rx, 7).await;
    let c = conv_by_title(&h, "Anna Kovács").await;
    h.send(Command::OpenConversation(c.id));
    wait_for(&mut rx, |e| {
        matches!(
            e,
            AppEvent::SyncCompleted {
                conversation: Some(_)
            }
        )
    })
    .await;

    provider.inject_incoming("chat-anna", &person("u-anna", "Anna Kovács"), "ping!");
    h.send(Command::OpenConversation(c.id));
    wait_for(&mut rx, |e| matches!(e, AppEvent::MessagesUpdated(_))).await;
    let msgs = h.messages(c.id, 100).await.unwrap();
    assert_eq!(msgs.len(), 5);
    assert_eq!(
        comms_domain::blocks_to_plain_text(&msgs[4].content),
        "ping!"
    );
}

#[tokio::test]
async fn send_succeeds_and_message_is_confirmed() {
    let provider = Arc::new(FakeProvider::demo());
    let (h, mut rx) = start(provider.clone()).await;
    wait_for_conversations(&h, &mut rx, 7).await;
    let c = conv_by_title(&h, "Anna Kovács").await;
    h.send(Command::OpenConversation(c.id));
    wait_for(&mut rx, |e| {
        matches!(
            e,
            AppEvent::SyncCompleted {
                conversation: Some(_)
            }
        )
    })
    .await;

    h.send(Command::SendMessage {
        conversation: c.id,
        text: "  hello there  ".into(),
    });
    wait_for(&mut rx, |e| matches!(e, AppEvent::MessageSendStarted(_))).await;
    wait_for(&mut rx, |e| matches!(e, AppEvent::MessageSendSucceeded(_))).await;
    let msgs = h.messages(c.id, 100).await.unwrap();
    let last = msgs.last().unwrap();
    assert_eq!(
        comms_domain::blocks_to_plain_text(&last.content),
        "hello there"
    );
    assert_eq!(last.send_state, SendState::Sent);
    assert!(last.remote_id.is_some());
    assert!(last.is_mine);
    assert_eq!(
        provider.server_messages("chat-anna")[0].remote_id.as_str(),
        "srv-1"
    );
    assert_eq!(provider.send_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn empty_messages_are_not_sent() {
    let provider = Arc::new(FakeProvider::demo());
    let (h, mut rx) = start(provider.clone()).await;
    wait_for_conversations(&h, &mut rx, 7).await;
    let c = conv_by_title(&h, "Anna Kovács").await;
    h.send(Command::SendMessage {
        conversation: c.id,
        text: "   \n ".into(),
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(provider.send_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn definite_failure_marks_failed_and_manual_retry_works() {
    let provider = Arc::new(FakeProvider::demo());
    provider.script_send(SendScript::Fail(ProviderError::Forbidden(
        "no permission".into(),
    )));
    let (h, mut rx) = start(provider.clone()).await;
    wait_for_conversations(&h, &mut rx, 7).await;
    let c = conv_by_title(&h, "Anna Kovács").await;

    h.send(Command::SendMessage {
        conversation: c.id,
        text: "try me".into(),
    });
    let ev = wait_for(&mut rx, |e| matches!(e, AppEvent::MessageSendFailed { .. })).await;
    let AppEvent::MessageSendFailed {
        message, uncertain, ..
    } = ev
    else {
        unreachable!()
    };
    assert!(!uncertain);
    assert_eq!(
        h.messages(c.id, 10)
            .await
            .unwrap()
            .last()
            .unwrap()
            .send_state,
        SendState::Failed
    );
    assert_eq!(
        provider.send_calls.load(Ordering::SeqCst),
        1,
        "no automatic resend"
    );

    h.send(Command::RetryMessage(message));
    wait_for(&mut rx, |e| matches!(e, AppEvent::MessageSendSucceeded(_))).await;
    let msgs = h.messages(c.id, 10).await.unwrap();
    assert_eq!(
        msgs.iter()
            .filter(|m| m.is_mine && m.send_state == SendState::Sent)
            .count(),
        1
    );
    assert_eq!(provider.send_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn uncertain_send_is_reconciled_and_never_resent() {
    let provider = Arc::new(FakeProvider::demo());
    provider.script_send(SendScript::AppliedThenFail(ProviderError::Network {
        may_have_reached_server: true,
        message: "timed out".into(),
    }));
    let (h, mut rx) = start(provider.clone()).await;
    wait_for_conversations(&h, &mut rx, 7).await;
    let c = conv_by_title(&h, "Anna Kovács").await;
    h.send(Command::OpenConversation(c.id));
    wait_for(&mut rx, |e| {
        matches!(
            e,
            AppEvent::SyncCompleted {
                conversation: Some(_)
            }
        )
    })
    .await;

    h.send(Command::SendMessage {
        conversation: c.id,
        text: "did this arrive?".into(),
    });
    let ev = wait_for(&mut rx, |e| matches!(e, AppEvent::MessageSendFailed { .. })).await;
    assert!(matches!(
        ev,
        AppEvent::MessageSendFailed {
            uncertain: true,
            ..
        }
    ));

    // The reconcile sync finds the server copy and adopts it.
    for _ in 0..50 {
        let msgs = h.messages(c.id, 100).await.unwrap();
        if msgs
            .iter()
            .any(|m| m.is_mine && m.send_state == SendState::Sent)
        {
            assert_eq!(msgs.len(), 5, "exactly one copy of the message");
            assert_eq!(
                provider.send_calls.load(Ordering::SeqCst),
                1,
                "never resent"
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("uncertain message was never reconciled");
}

#[tokio::test]
async fn uncertain_send_that_never_arrived_stays_uncertain() {
    let provider = Arc::new(FakeProvider::demo());
    provider.script_send(SendScript::Fail(ProviderError::Server {
        status: 502,
        message: "bad gateway".into(),
    }));
    let (h, mut rx) = start(provider.clone()).await;
    wait_for_conversations(&h, &mut rx, 7).await;
    let c = conv_by_title(&h, "Anna Kovács").await;
    h.send(Command::SendMessage {
        conversation: c.id,
        text: "maybe".into(),
    });
    wait_for(&mut rx, |e| {
        matches!(
            e,
            AppEvent::MessageSendFailed {
                uncertain: true,
                ..
            }
        )
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let msgs = h.messages(c.id, 100).await.unwrap();
    assert_eq!(msgs.last().unwrap().send_state, SendState::Uncertain);
    assert_eq!(provider.send_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn load_older_pages_until_the_start() {
    let provider = Arc::new(FakeProvider::empty());
    let me = provider.me();
    let now = Utc::now();
    let msgs: Vec<_> = (0..130)
        .map(|i| {
            text_message(
                &format!("m{i}"),
                &me,
                now - Chrono::minutes(200 - i),
                &format!("msg {i}"),
            )
        })
        .collect();
    provider.add_conversation(
        conversation("c", ConversationKind::Group, "Big", None),
        msgs,
    );
    let (h, mut rx) = start(provider.clone()).await;
    wait_for_conversations(&h, &mut rx, 1).await;
    let c = h.conversations().await.unwrap().remove(0);

    h.send(Command::OpenConversation(c.id));
    wait_for(&mut rx, |e| {
        matches!(
            e,
            AppEvent::SyncCompleted {
                conversation: Some(_)
            }
        )
    })
    .await;
    assert_eq!(
        h.messages(c.id, 500).await.unwrap().len(),
        100,
        "initial cache is 100"
    );
    assert!(h.has_older(c.id).await.unwrap());

    h.send(Command::LoadOlderMessages(c.id));
    let ev = wait_for(&mut rx, |e| {
        matches!(e, AppEvent::OlderMessagesLoaded { .. })
    })
    .await;
    assert!(matches!(
        ev,
        AppEvent::OlderMessagesLoaded {
            loaded: 30,
            reached_start: true,
            ..
        }
    ));
    assert_eq!(h.messages(c.id, 500).await.unwrap().len(), 130);
    assert!(!h.has_older(c.id).await.unwrap());

    h.send(Command::LoadOlderMessages(c.id));
    let ev = wait_for(&mut rx, |e| {
        matches!(e, AppEvent::OlderMessagesLoaded { .. })
    })
    .await;
    assert!(matches!(
        ev,
        AppEvent::OlderMessagesLoaded {
            loaded: 0,
            reached_start: true,
            ..
        }
    ));
}

#[tokio::test]
async fn gap_since_last_sync_is_filled() {
    let provider = Arc::new(FakeProvider::empty());
    let me = provider.me();
    let other = person("o", "Other");
    let now = Utc::now();
    provider.add_conversation(
        conversation("c", ConversationKind::Direct, "Other", None),
        vec![text_message("m0", &other, now - Chrono::hours(5), "old")],
    );
    let (h, mut rx) = start(provider.clone()).await;
    wait_for_conversations(&h, &mut rx, 1).await;
    let c = h.conversations().await.unwrap().remove(0);
    h.send(Command::OpenConversation(c.id));
    wait_for(&mut rx, |e| {
        matches!(
            e,
            AppEvent::SyncCompleted {
                conversation: Some(_)
            }
        )
    })
    .await;

    // 120 messages arrive while we are away (more than one page).
    for i in 0..120 {
        provider.inject_incoming("c", &other, &format!("new {i}"));
    }
    let _ = me;
    h.send(Command::OpenConversation(c.id));
    wait_for(&mut rx, |e| {
        matches!(
            e,
            AppEvent::SyncCompleted {
                conversation: Some(_)
            }
        )
    })
    .await;
    assert_eq!(h.messages(c.id, 500).await.unwrap().len(), 121);
}

#[tokio::test]
async fn forbidden_channels_do_not_break_chats() {
    struct NoChannels(FakeProvider);
    #[async_trait::async_trait]
    impl comms_provider_api::MessagingProvider for NoChannels {
        fn kind(&self) -> comms_domain::ProviderKind {
            self.0.kind()
        }
        fn capabilities(&self) -> comms_provider_api::Capabilities {
            self.0.capabilities()
        }
        async fn current_user(&self) -> Result<comms_domain::Participant, ProviderError> {
            self.0.current_user().await
        }
        async fn list_conversations(
            &self,
            class: comms_provider_api::ConversationClass,
            page: comms_provider_api::PageRequest,
        ) -> Result<comms_provider_api::Page<comms_provider_api::RemoteConversation>, ProviderError>
        {
            if class == comms_provider_api::ConversationClass::Channels {
                return Err(ProviderError::Forbidden("admin consent required".into()));
            }
            self.0.list_conversations(class, page).await
        }
        async fn get_messages(
            &self,
            c: &comms_domain::RemoteId,
            p: comms_provider_api::PageRequest,
        ) -> Result<comms_provider_api::Page<comms_provider_api::RemoteMessage>, ProviderError>
        {
            self.0.get_messages(c, p).await
        }
        async fn send_message(
            &self,
            c: &comms_domain::RemoteId,
            m: comms_provider_api::OutgoingMessage,
        ) -> Result<comms_provider_api::RemoteMessage, ProviderError> {
            self.0.send_message(c, m).await
        }
    }

    let store = Store::open_in_memory().await.unwrap();
    let h = AppHandle::start(
        Arc::new(NoChannels(FakeProvider::demo())),
        store,
        fast_config(),
    )
    .await
    .unwrap();
    let mut rx = h.subscribe();
    let ev = wait_for(&mut rx, |e| matches!(e, AppEvent::ChannelsUnavailable(_))).await;
    assert!(matches!(ev, AppEvent::ChannelsUnavailable(m) if m.contains("admin consent")));
    wait_for_conversations(&h, &mut rx, 3).await;
    assert_eq!(h.conversations().await.unwrap().len(), 3);
}

#[tokio::test]
async fn unauthorized_sets_auth_required() {
    struct Expired;
    #[async_trait::async_trait]
    impl comms_provider_api::MessagingProvider for Expired {
        fn kind(&self) -> comms_domain::ProviderKind {
            comms_domain::ProviderKind::new("expired")
        }
        fn capabilities(&self) -> comms_provider_api::Capabilities {
            Default::default()
        }
        async fn current_user(&self) -> Result<comms_domain::Participant, ProviderError> {
            Ok(person("me", "Me"))
        }
        async fn list_conversations(
            &self,
            _: comms_provider_api::ConversationClass,
            _: comms_provider_api::PageRequest,
        ) -> Result<comms_provider_api::Page<comms_provider_api::RemoteConversation>, ProviderError>
        {
            Err(ProviderError::Unauthorized)
        }
        async fn get_messages(
            &self,
            _: &comms_domain::RemoteId,
            _: comms_provider_api::PageRequest,
        ) -> Result<comms_provider_api::Page<comms_provider_api::RemoteMessage>, ProviderError>
        {
            Err(ProviderError::Unauthorized)
        }
        async fn send_message(
            &self,
            _: &comms_domain::RemoteId,
            _: comms_provider_api::OutgoingMessage,
        ) -> Result<comms_provider_api::RemoteMessage, ProviderError> {
            Err(ProviderError::Unauthorized)
        }
    }
    let store = Store::open_in_memory().await.unwrap();
    let h = AppHandle::start(Arc::new(Expired), store, fast_config())
        .await
        .unwrap();
    let mut rx = h.subscribe();
    wait_for(&mut rx, |e| {
        matches!(
            e,
            AppEvent::ConnectionStatusChanged(ConnectionStatus::AuthRequired)
        )
    })
    .await;
}
