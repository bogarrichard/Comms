use chrono::{Duration, Utc};
use comms_domain::{
    ContentBlock, ConversationKind, Participant, ProviderKind, RemoteId, SendState,
};

use super::*;

fn me() -> Participant {
    Participant {
        remote_id: RemoteId::new("me"),
        display_name: "Me".into(),
    }
}
fn other() -> Participant {
    Participant {
        remote_id: RemoteId::new("o"),
        display_name: "Other".into(),
    }
}

async fn setup() -> (Store, AccountId, ConversationId) {
    let store = Store::open_in_memory().await.unwrap();
    let acct = store
        .ensure_account(&ProviderKind::new("t"), &me())
        .await
        .unwrap();
    let rid = RemoteId::new("c1");
    store
        .upsert_conversations(
            acct.id,
            &[IncomingConversation {
                remote_id: &rid,
                kind: ConversationKind::Direct,
                title: "Other",
                group: None,
                last_activity: None,
                last_read: None,
                preview: None,
                last_message_from_me: false,
            }],
            None,
        )
        .await
        .unwrap();
    let conv = store.list_conversations(acct.id).await.unwrap().remove(0);
    (store, acct.id, conv.id)
}

fn incoming<'a>(
    id: &'a RemoteId,
    sender: &'a Participant,
    at: DateTime<Utc>,
    content: &'a [ContentBlock],
) -> IncomingMessage<'a> {
    IncomingMessage {
        remote_id: id,
        sender,
        timestamp: at,
        content,
        attachments: &[],
        edited_at: None,
        deleted_at: None,
        reply_to: None,
    }
}

#[tokio::test]
async fn migrations_apply_and_account_is_stable() {
    let store = Store::open_in_memory().await.unwrap();
    let a = store
        .ensure_account(&ProviderKind::new("t"), &me())
        .await
        .unwrap();
    let b = store
        .ensure_account(&ProviderKind::new("t"), &me())
        .await
        .unwrap();
    assert_eq!(a.id, b.id);
}

#[tokio::test]
async fn duplicate_messages_are_not_inserted_twice() {
    let (store, _, conv) = setup().await;
    let id = RemoteId::new("m1");
    let content = vec![ContentBlock::paragraph("hello")];
    let o = other();
    let now = Utc::now();
    let s1 = store
        .upsert_remote_messages(conv, &me().remote_id, &[incoming(&id, &o, now, &content)])
        .await
        .unwrap();
    let s2 = store
        .upsert_remote_messages(conv, &me().remote_id, &[incoming(&id, &o, now, &content)])
        .await
        .unwrap();
    assert_eq!(s1.inserted, 1);
    assert!(!s2.changed());
    assert_eq!(store.message_count(conv).await.unwrap(), 1);
}

#[tokio::test]
async fn edits_and_deletions_update_cached_rows() {
    let (store, _, conv) = setup().await;
    let id = RemoteId::new("m1");
    let o = other();
    let now = Utc::now();
    let v1 = vec![ContentBlock::paragraph("hello")];
    store
        .upsert_remote_messages(conv, &me().remote_id, &[incoming(&id, &o, now, &v1)])
        .await
        .unwrap();

    let v2 = vec![ContentBlock::paragraph("hello world")];
    let mut edited = incoming(&id, &o, now, &v2);
    edited.edited_at = Some(now + Duration::minutes(1));
    let s = store
        .upsert_remote_messages(conv, &me().remote_id, &[edited])
        .await
        .unwrap();
    assert_eq!(s.updated, 1);
    let msgs = store.recent_messages(conv, 10).await.unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        comms_domain::blocks_to_plain_text(&msgs[0].content),
        "hello world"
    );
    assert!(msgs[0].edited_at.is_some());

    let mut deleted = incoming(&id, &o, now, &v2);
    deleted.deleted_at = Some(now + Duration::minutes(2));
    store
        .upsert_remote_messages(conv, &me().remote_id, &[deleted])
        .await
        .unwrap();
    assert!(
        store.recent_messages(conv, 10).await.unwrap()[0]
            .deleted_at
            .is_some()
    );
}

#[tokio::test]
async fn recent_messages_are_ordered_oldest_first_and_limited() {
    let (store, _, conv) = setup().await;
    let o = other();
    let now = Utc::now();
    let ids: Vec<_> = (0..5).map(|i| RemoteId::new(format!("m{i}"))).collect();
    let content = vec![ContentBlock::paragraph("x")];
    let batch: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| incoming(id, &o, now + Duration::seconds(i as i64), &content))
        .collect();
    store
        .upsert_remote_messages(conv, &me().remote_id, &batch)
        .await
        .unwrap();
    let msgs = store.recent_messages(conv, 3).await.unwrap();
    let got: Vec<_> = msgs
        .iter()
        .map(|m| m.remote_id.clone().unwrap().to_string())
        .collect();
    assert_eq!(got, ["m2", "m3", "m4"]);
}

#[tokio::test]
async fn uncertain_local_message_is_adopted_not_duplicated() {
    let (store, _, conv) = setup().await;
    let now = Utc::now();
    let local = store
        .insert_pending(conv, &me(), "see you  at 5", now)
        .await
        .unwrap();
    store
        .set_send_state(local.id, SendState::Uncertain)
        .await
        .unwrap();

    // The server copy comes back (HTML conversion may change whitespace).
    let rid = RemoteId::new("srv1");
    let content = vec![ContentBlock::paragraph("see you at 5")];
    let m = me();
    let stats = store
        .upsert_remote_messages(
            conv,
            &m.remote_id,
            &[incoming(&rid, &m, now + Duration::seconds(2), &content)],
        )
        .await
        .unwrap();
    assert_eq!(stats.adopted, 1);
    assert_eq!(stats.inserted, 0);
    let msgs = store.recent_messages(conv, 10).await.unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].send_state, SendState::Sent);
    assert_eq!(msgs[0].remote_id.as_ref(), Some(&rid));
}

#[tokio::test]
async fn failed_message_is_never_adopted() {
    let (store, _, conv) = setup().await;
    let now = Utc::now();
    let local = store
        .insert_pending(conv, &me(), "retry me", now)
        .await
        .unwrap();
    store
        .set_send_state(local.id, SendState::Failed)
        .await
        .unwrap();
    let rid = RemoteId::new("srv1");
    let content = vec![ContentBlock::paragraph("retry me")];
    let m = me();
    let stats = store
        .upsert_remote_messages(conv, &m.remote_id, &[incoming(&rid, &m, now, &content)])
        .await
        .unwrap();
    assert_eq!(stats.inserted, 1);
    assert_eq!(store.message_count(conv).await.unwrap(), 2);
}

#[tokio::test]
async fn mark_sent_after_sync_adopted_it_drops_the_duplicate() {
    let (store, _, conv) = setup().await;
    let now = Utc::now();
    let local = store.insert_pending(conv, &me(), "hi", now).await.unwrap();
    let rid = RemoteId::new("srv1");
    let content = vec![ContentBlock::paragraph("other text")];
    let o = other();
    // Same remote id already stored by a sync (different content, so no adoption).
    store
        .upsert_remote_messages(conv, &me().remote_id, &[incoming(&rid, &o, now, &content)])
        .await
        .unwrap();
    store.mark_sent(local.id, &rid, now).await.unwrap();
    assert_eq!(store.message_count(conv).await.unwrap(), 1);
}

#[tokio::test]
async fn interrupted_sends_become_uncertain() {
    let (store, _, conv) = setup().await;
    store
        .insert_pending(conv, &me(), "x", Utc::now())
        .await
        .unwrap();
    assert_eq!(store.recover_interrupted_sends().await.unwrap(), 1);
    let msgs = store.recent_messages(conv, 10).await.unwrap();
    assert_eq!(msgs[0].send_state, SendState::Uncertain);
}

#[tokio::test]
async fn unread_logic() {
    let store = Store::open_in_memory().await.unwrap();
    let acct = store
        .ensure_account(&ProviderKind::new("t"), &me())
        .await
        .unwrap();
    let rid = RemoteId::new("c1");
    let now = Utc::now();
    let mk = |from_me: bool, read: Option<DateTime<Utc>>| IncomingConversation {
        remote_id: &rid,
        kind: ConversationKind::Direct,
        title: "T",
        group: None,
        last_activity: Some(now),
        last_read: read,
        preview: Some("p"),
        last_message_from_me: from_me,
    };
    store
        .upsert_conversations(acct.id, &[mk(false, Some(now - Duration::hours(1)))], None)
        .await
        .unwrap();
    let c = store.list_conversations(acct.id).await.unwrap().remove(0);
    assert!(c.unread);

    store
        .mark_read_locally(c.id, now + Duration::seconds(1))
        .await
        .unwrap();
    assert!(!store.list_conversations(acct.id).await.unwrap()[0].unread);

    store
        .upsert_conversations(acct.id, &[mk(true, None)], None)
        .await
        .unwrap();
    assert!(!store.list_conversations(acct.id).await.unwrap()[0].unread);
}

#[tokio::test]
async fn channels_sort_after_chats_and_stale_channels_are_pruned() {
    let store = Store::open_in_memory().await.unwrap();
    let acct = store
        .ensure_account(&ProviderKind::new("t"), &me())
        .await
        .unwrap();
    let ids = ["a", "b", "c"].map(RemoteId::new);
    let mk = |i: usize, kind, title, group| IncomingConversation {
        remote_id: &ids[i],
        kind,
        title,
        group,
        last_activity: None,
        last_read: None,
        preview: None,
        last_message_from_me: false,
    };
    store
        .upsert_conversations(
            acct.id,
            &[
                mk(0, ConversationKind::Channel, "Z", Some("Team")),
                mk(1, ConversationKind::Channel, "A", Some("Team")),
                mk(2, ConversationKind::Direct, "Chat", None),
            ],
            None,
        )
        .await
        .unwrap();
    let titles: Vec<_> = store
        .list_conversations(acct.id)
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.title)
        .collect();
    assert_eq!(titles, ["Chat", "A", "Z"]);

    store
        .upsert_conversations(
            acct.id,
            &[mk(1, ConversationKind::Channel, "A", Some("Team"))],
            Some(ConversationKind::Channel),
        )
        .await
        .unwrap();
    let titles: Vec<_> = store
        .list_conversations(acct.id)
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.title)
        .collect();
    assert_eq!(titles, ["Chat", "A"]);
}

#[tokio::test]
async fn backfill_state_round_trip() {
    let (store, _, conv) = setup().await;
    assert_eq!(store.backfill_state(conv).await.unwrap(), (None, false));
    let cur = PageCursor::new("abc");
    store
        .set_backfill_state(conv, Some(&cur), false)
        .await
        .unwrap();
    assert_eq!(
        store.backfill_state(conv).await.unwrap(),
        (Some(cur), false)
    );
    store.set_backfill_state(conv, None, true).await.unwrap();
    assert_eq!(store.backfill_state(conv).await.unwrap(), (None, true));
}

#[cfg(unix)]
#[tokio::test]
async fn database_file_is_private_and_persists() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("comms.db");
    let store = Store::open(&path).await.unwrap();
    store
        .ensure_account(&ProviderKind::new("t"), &me())
        .await
        .unwrap();
    drop(store);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let dir_mode = std::fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);
    // Reopen: migrations are idempotent and data is still there.
    let store = Store::open(&path).await.unwrap();
    let a = store
        .ensure_account(&ProviderKind::new("t"), &me())
        .await
        .unwrap();
    assert_eq!(a.display_name, "Me");
}

#[tokio::test]
async fn wipe_removes_everything() {
    let (store, acct, conv) = setup().await;
    store
        .insert_pending(conv, &me(), "x", Utc::now())
        .await
        .unwrap();
    store.wipe().await.unwrap();
    assert!(store.list_conversations(acct).await.unwrap().is_empty());
    assert_eq!(store.message_count(conv).await.unwrap(), 0);
}

#[tokio::test]
async fn first_account_and_get_message() {
    let store = Store::open_in_memory().await.unwrap();
    let kind = ProviderKind::new("t");
    assert!(store.first_account(&kind).await.unwrap().is_none());
    let acct = store.ensure_account(&kind, &me()).await.unwrap();
    assert_eq!(
        store.first_account(&kind).await.unwrap().unwrap().id,
        acct.id
    );
    assert!(
        store
            .first_account(&ProviderKind::new("other"))
            .await
            .unwrap()
            .is_none()
    );

    let rid = RemoteId::new("c1");
    store
        .upsert_conversations(
            acct.id,
            &[IncomingConversation {
                remote_id: &rid,
                kind: ConversationKind::Direct,
                title: "T",
                group: None,
                last_activity: None,
                last_read: None,
                preview: None,
                last_message_from_me: false,
            }],
            None,
        )
        .await
        .unwrap();
    let conv = store.list_conversations(acct.id).await.unwrap().remove(0);
    let m = store
        .insert_pending(conv.id, &me(), "yo", Utc::now())
        .await
        .unwrap();
    assert_eq!(store.get_message(m.id).await.unwrap().unwrap().id, m.id);
}
