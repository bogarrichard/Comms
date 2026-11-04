use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::Utc;
use comms_domain::{
    Account, Conversation, ConversationId, ConversationKind, Message, MessageId, Participant,
    SendState, blocks_to_plain_text,
};
use comms_provider_api::{
    ConversationClass, MessagingProvider, OutgoingMessage, PageRequest, ProviderError,
    RemoteMessage,
};
use comms_storage::{IncomingConversation, IncomingMessage, Store, UpsertStats};
use tokio::sync::{broadcast, mpsc};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, warn};

use crate::{AppEvent, Command, ConnectionStatus, CoreConfig};

const PAGE_SIZE: u32 = 50;
const MAX_CONVERSATION_PAGES: usize = 4;
/// Upper bound of pages followed when catching up after an absence.
const MAX_GAP_PAGES: usize = 5;

#[derive(Default)]
struct State {
    open: Option<ConversationId>,
    syncing: HashSet<ConversationId>,
    chats_refreshing: bool,
    channels_refreshing: bool,
    channels_unavailable: bool,
    status: Option<ConnectionStatus>,
}

pub(crate) struct Inner {
    pub provider: Arc<dyn MessagingProvider>,
    pub store: Store,
    pub account: Account,
    pub me: Participant,
    config: CoreConfig,
    pub events: broadcast::Sender<AppEvent>,
    state: Mutex<State>,
    activity: AtomicUsize,
}

/// Counts in-flight network work and emits `SyncActivity` on transitions.
struct ActivityGuard(Arc<Inner>);

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        if self.0.activity.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.emit(AppEvent::SyncActivity(false));
        }
    }
}

impl Inner {
    pub fn new(
        provider: Arc<dyn MessagingProvider>,
        store: Store,
        account: Account,
        config: CoreConfig,
        events: broadcast::Sender<AppEvent>,
    ) -> Self {
        let me = Participant {
            remote_id: account.remote_id.clone(),
            display_name: account.display_name.clone(),
        };
        Self {
            provider,
            store,
            account,
            me,
            config,
            events,
            state: Mutex::new(State {
                status: Some(ConnectionStatus::Connecting),
                ..State::default()
            }),
            activity: AtomicUsize::new(0),
        }
    }

    fn emit(&self, event: AppEvent) {
        // No subscribers is fine.
        let _ = self.events.send(event);
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> ConnectionStatus {
        self.st()
            .status
            .clone()
            .unwrap_or(ConnectionStatus::Connecting)
    }

    fn set_status(&self, status: ConnectionStatus) {
        let changed = {
            let mut st = self.st();
            let changed = st.status.as_ref() != Some(&status);
            st.status = Some(status.clone());
            changed
        };
        if changed {
            self.emit(AppEvent::ConnectionStatusChanged(status));
        }
    }

    fn busy(self: &Arc<Self>) -> ActivityGuard {
        if self.activity.fetch_add(1, Ordering::SeqCst) == 0 {
            self.emit(AppEvent::SyncActivity(true));
        }
        ActivityGuard(self.clone())
    }

    /// Translate a provider failure into status changes and user notices.
    fn report(&self, what: &str, err: &ProviderError) {
        warn!(error = %err, "{what} failed");
        match err {
            ProviderError::Unauthorized => self.set_status(ConnectionStatus::AuthRequired),
            ProviderError::Network { message, .. } => {
                self.set_status(ConnectionStatus::Offline(message.clone()));
            }
            ProviderError::RateLimited { retry_after } => {
                self.emit(AppEvent::Notice(format!(
                    "Rate limited by the server; backing off ({}s)",
                    retry_after.as_secs()
                )));
            }
            other => self.emit(AppEvent::Notice(format!("{what}: {other}"))),
        }
    }

    fn store_error(&self, what: &str, err: impl std::fmt::Display) {
        warn!(error = %err, "{what}");
        self.emit(AppEvent::Notice(format!("{what}: {err}")));
    }
}

pub(crate) async fn run(
    inner: Arc<Inner>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    verify_identity: bool,
) {
    if verify_identity {
        let inner = inner.clone();
        tokio::spawn(async move {
            if let Err(e) = inner.provider.current_user().await {
                inner.report("Sign-in check", &e);
            }
        });
    }

    let cfg = inner.config.clone();
    let mut chats = interval(cfg.chat_poll);
    let mut channels = interval(cfg.channel_poll);
    let mut open = interval(cfg.open_poll);
    for t in [&mut chats, &mut channels, &mut open] {
        t.set_missed_tick_behavior(MissedTickBehavior::Delay);
    }

    loop {
        tokio::select! {
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { break };
                handle_command(&inner, cmd);
            }
            _ = chats.tick() => { tokio::spawn(refresh_chats(inner.clone())); }
            _ = channels.tick() => { tokio::spawn(refresh_channels(inner.clone())); }
            _ = open.tick() => {
                let current = inner.st().open;
                if let Some(id) = current {
                    tokio::spawn(sync_and_report(inner.clone(), id));
                }
            }
        }
    }
    debug!("engine stopped");
}

fn handle_command(inner: &Arc<Inner>, cmd: Command) {
    match cmd {
        Command::RefreshConversations => {
            tokio::spawn(refresh_chats(inner.clone()));
            tokio::spawn(refresh_channels(inner.clone()));
        }
        Command::OpenConversation(id) => {
            inner.st().open = Some(id);
            tokio::spawn(open_conversation(inner.clone(), id));
        }
        Command::CloseConversation => inner.st().open = None,
        Command::LoadOlderMessages(id) => {
            tokio::spawn(load_older(inner.clone(), id));
        }
        Command::SendMessage { conversation, text } => {
            tokio::spawn(send_new(inner.clone(), conversation, text));
        }
        Command::RetryMessage(id) => {
            tokio::spawn(retry(inner.clone(), id));
        }
    }
}

// ---------------------------------------------------------------- conversations

fn to_incoming(c: &comms_provider_api::RemoteConversation) -> IncomingConversation<'_> {
    IncomingConversation {
        remote_id: &c.remote_id,
        kind: c.kind,
        title: &c.title,
        group: c.group.as_deref(),
        last_activity: c.last_activity,
        last_read: c.last_read,
        preview: c.preview.as_deref(),
        last_message_from_me: c.last_message_from_me,
    }
}

async fn refresh_chats(inner: Arc<Inner>) {
    if std::mem::replace(&mut inner.st().chats_refreshing, true) {
        return;
    }
    let _busy = inner.busy();
    let result = list_all(&inner, ConversationClass::Chats).await;
    inner.st().chats_refreshing = false;
    match result {
        Ok((items, _complete)) => {
            let incoming: Vec<_> = items.iter().map(to_incoming).collect();
            match inner
                .store
                .upsert_conversations(inner.account.id, &incoming, None)
                .await
            {
                Ok(()) => {
                    inner.set_status(ConnectionStatus::Online);
                    inner.emit(AppEvent::ConversationsUpdated);
                    inner.emit(AppEvent::SyncCompleted { conversation: None });
                }
                Err(e) => inner.store_error("Could not save chats", e),
            }
        }
        Err(e) => inner.report("Refreshing chats", &e),
    }
}

async fn refresh_channels(inner: Arc<Inner>) {
    if !inner.provider.capabilities().channels {
        return;
    }
    if std::mem::replace(&mut inner.st().channels_refreshing, true) {
        return;
    }
    let _busy = inner.busy();
    let result = list_all(&inner, ConversationClass::Channels).await;
    inner.st().channels_refreshing = false;
    match result {
        Ok((items, complete)) => {
            inner.st().channels_unavailable = false;
            let incoming: Vec<_> = items.iter().map(to_incoming).collect();
            let prune = (complete && !incoming.is_empty()).then_some(ConversationKind::Channel);
            match inner
                .store
                .upsert_conversations(inner.account.id, &incoming, prune)
                .await
            {
                Ok(()) => inner.emit(AppEvent::ConversationsUpdated),
                Err(e) => inner.store_error("Could not save channels", e),
            }
        }
        Err(ProviderError::Forbidden(msg)) => {
            let first = !std::mem::replace(&mut inner.st().channels_unavailable, true);
            if first {
                inner.emit(AppEvent::ChannelsUnavailable(msg));
            }
        }
        Err(e) => inner.report("Refreshing channels", &e),
    }
}

/// Follow pagination up to a sane bound. Returns `(items, complete)`.
async fn list_all(
    inner: &Inner,
    class: ConversationClass,
) -> Result<(Vec<comms_provider_api::RemoteConversation>, bool), ProviderError> {
    let mut items = Vec::new();
    let mut req = PageRequest::first(PAGE_SIZE);
    for _ in 0..MAX_CONVERSATION_PAGES {
        let page = inner
            .provider
            .list_conversations(class, req.clone())
            .await?;
        items.extend(page.items);
        match page.next {
            Some(c) => req.cursor = Some(c),
            None => return Ok((items, true)),
        }
    }
    Ok((items, false))
}

// ---------------------------------------------------------------------- messages

async fn open_conversation(inner: Arc<Inner>, id: ConversationId) {
    if let Err(e) = inner.store.mark_read_locally(id, Utc::now()).await {
        inner.store_error("Could not update read state", e);
    }
    inner.emit(AppEvent::ConversationsUpdated);
    sync_and_report(inner, id).await;
}

async fn sync_and_report(inner: Arc<Inner>, id: ConversationId) {
    match sync_conversation(&inner, id).await {
        Ok(()) => inner.set_status(ConnectionStatus::Online),
        Err(e) => inner.report("Loading messages", &e),
    }
}

fn newest_first(msgs: &[RemoteMessage]) -> Option<&RemoteMessage> {
    msgs.iter()
        .filter(|m| !m.is_system)
        .max_by_key(|m| m.timestamp)
}

async fn store_page(
    inner: &Inner,
    conv: &Conversation,
    msgs: &[RemoteMessage],
) -> Result<(UpsertStats, usize), comms_storage::StoreError> {
    let visible: Vec<&RemoteMessage> = msgs.iter().filter(|m| !m.is_system).collect();
    let incoming: Vec<IncomingMessage<'_>> = visible
        .iter()
        .map(|m| IncomingMessage {
            remote_id: &m.remote_id,
            sender: &m.sender,
            timestamp: m.timestamp,
            content: &m.content,
            attachments: &m.attachments,
            edited_at: m.edited_at,
            deleted_at: m.deleted_at,
            reply_to: m.reply_to.as_ref(),
        })
        .collect();
    let stats = inner
        .store
        .upsert_remote_messages(conv.id, &inner.me.remote_id, &incoming)
        .await?;
    if let Some(newest) = newest_first(msgs) {
        let preview = blocks_to_plain_text(&newest.content);
        inner
            .store
            .touch_conversation(
                conv.id,
                newest.timestamp,
                &preview,
                newest.sender.remote_id == inner.me.remote_id,
            )
            .await?;
    }
    Ok((stats, incoming.len()))
}

/// Fetch the newest messages of a conversation into the cache. On the first
/// ever sync this fills up to `initial_messages`; later syncs fetch one page,
/// continuing only while every message on it was new (a gap since last time).
async fn sync_conversation(inner: &Arc<Inner>, id: ConversationId) -> Result<(), ProviderError> {
    if !inner.st().syncing.insert(id) {
        return Ok(());
    }
    struct Release<'a>(&'a Inner, ConversationId);
    impl Drop for Release<'_> {
        fn drop(&mut self) {
            self.0.st().syncing.remove(&self.1);
        }
    }
    let _release = Release(inner, id);
    let _busy = inner.busy();

    let conv = match inner.store.get_conversation(id).await {
        Ok(Some(c)) => c,
        Ok(None) => return Ok(()),
        Err(e) => {
            inner.store_error("Could not read conversation", e);
            return Ok(());
        }
    };
    let first_ever = inner.store.message_count(id).await.unwrap_or(0) == 0;

    let mut req = PageRequest::first(PAGE_SIZE);
    let mut fetched = 0usize;
    let mut pages = 0usize;
    loop {
        let page = inner
            .provider
            .get_messages(&conv.remote_id, req.clone())
            .await?;
        pages += 1;
        let (stats, visible) = match store_page(inner, &conv, &page.items).await {
            Ok(r) => r,
            Err(e) => {
                inner.store_error("Could not save messages", e);
                return Ok(());
            }
        };
        fetched += visible;
        if first_ever {
            // Remember where "older" starts; later pages move it forward.
            let _ = inner
                .store
                .set_backfill_state(id, page.next.as_ref(), page.next.is_none())
                .await;
        }
        if stats.changed() {
            inner.emit(AppEvent::MessagesUpdated(id));
            inner.emit(AppEvent::ConversationsUpdated);
        }
        // Reading while the conversation is open keeps it marked read.
        if stats.inserted > 0 && inner.st().open == Some(id) {
            let _ = inner.store.mark_read_locally(id, Utc::now()).await;
        }
        let wants_more = if first_ever {
            fetched < inner.config.initial_messages as usize
        } else {
            visible > 0 && stats.inserted == visible && pages < MAX_GAP_PAGES
        };
        match page.next {
            Some(c) if wants_more => req.cursor = Some(c),
            _ => break,
        }
    }
    inner.emit(AppEvent::SyncCompleted {
        conversation: Some(id),
    });
    Ok(())
}

async fn load_older(inner: Arc<Inner>, id: ConversationId) {
    let (cursor, done) = match inner.store.backfill_state(id).await {
        Ok(s) => s,
        Err(e) => return inner.store_error("Could not read sync state", e),
    };
    let reached = |loaded| AppEvent::OlderMessagesLoaded {
        conversation: id,
        loaded,
        reached_start: true,
    };
    if done && cursor.is_none() {
        return inner.emit(reached(0));
    }
    let Some(cursor) = cursor else {
        // Never synced: the initial sync establishes the cursor.
        return sync_and_report(inner, id).await;
    };
    let conv = match inner.store.get_conversation(id).await {
        Ok(Some(c)) => c,
        _ => return,
    };
    let _busy = inner.busy();
    let req = PageRequest {
        cursor: Some(cursor),
        limit: PAGE_SIZE,
    };
    match inner.provider.get_messages(&conv.remote_id, req).await {
        Ok(page) => match store_page(&inner, &conv, &page.items).await {
            Ok((stats, _)) => {
                let _ = inner
                    .store
                    .set_backfill_state(id, page.next.as_ref(), page.next.is_none())
                    .await;
                inner.set_status(ConnectionStatus::Online);
                inner.emit(AppEvent::MessagesUpdated(id));
                inner.emit(AppEvent::OlderMessagesLoaded {
                    conversation: id,
                    loaded: stats.inserted,
                    reached_start: page.next.is_none(),
                });
            }
            Err(e) => inner.store_error("Could not save messages", e),
        },
        Err(e) => inner.report("Loading older messages", &e),
    }
}

// ---------------------------------------------------------------------- sending

async fn send_new(inner: Arc<Inner>, conversation: ConversationId, text: String) {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return;
    }
    let local = match inner
        .store
        .insert_pending(conversation, &inner.me, &text, Utc::now())
        .await
    {
        Ok(m) => m,
        Err(e) => return inner.store_error("Could not queue message", e),
    };
    deliver(inner, local).await;
}

async fn retry(inner: Arc<Inner>, id: MessageId) {
    let msg = match inner.store.get_message(id).await {
        Ok(Some(m)) => m,
        _ => return,
    };
    if !matches!(msg.send_state, SendState::Failed | SendState::Uncertain)
        || msg.remote_id.is_some()
    {
        return;
    }
    if let Err(e) = inner.store.set_send_state(id, SendState::Pending).await {
        return inner.store_error("Could not update message", e);
    }
    deliver(
        inner,
        Message {
            send_state: SendState::Pending,
            ..msg
        },
    )
    .await;
}

/// Send one locally-stored message and record the outcome. Never retries on
/// its own: after an ambiguous failure the message is marked `uncertain` and
/// reconciled against the server's copy instead of being resent.
async fn deliver(inner: Arc<Inner>, local: Message) {
    let conv_id = local.conversation_id;
    inner.emit(AppEvent::MessageSendStarted(local.id));
    inner.emit(AppEvent::MessagesUpdated(conv_id));

    let conv = match inner.store.get_conversation(conv_id).await {
        Ok(Some(c)) => c,
        _ => return,
    };
    let text = blocks_to_plain_text(&local.content);
    let _busy = inner.busy();
    match inner
        .provider
        .send_message(&conv.remote_id, OutgoingMessage { text })
        .await
    {
        Ok(sent) => {
            if let Err(e) = inner
                .store
                .mark_sent(local.id, &sent.remote_id, sent.timestamp)
                .await
            {
                return inner.store_error("Could not record sent message", e);
            }
            let preview = blocks_to_plain_text(&sent.content);
            let _ = inner
                .store
                .touch_conversation(conv_id, sent.timestamp, &preview, true)
                .await;
            inner.set_status(ConnectionStatus::Online);
            inner.emit(AppEvent::MessageSendSucceeded(local.id));
            inner.emit(AppEvent::MessagesUpdated(conv_id));
            inner.emit(AppEvent::ConversationsUpdated);
        }
        Err(e) => {
            let not_applied = e.request_not_applied();
            let state = if not_applied {
                SendState::Failed
            } else {
                SendState::Uncertain
            };
            if let Err(se) = inner.store.set_send_state(local.id, state).await {
                inner.store_error("Could not update message", se);
            }
            if matches!(
                e,
                ProviderError::Unauthorized | ProviderError::Network { .. }
            ) {
                inner.report("Sending", &e);
            }
            inner.emit(AppEvent::MessageSendFailed {
                message: local.id,
                reason: e.to_string(),
                uncertain: !not_applied,
            });
            inner.emit(AppEvent::MessagesUpdated(conv_id));
            if !not_applied {
                // Look for the message on the server instead of resending.
                tokio::time::sleep(inner.config.reconcile_delay).await;
                drop(_busy);
                sync_and_report(inner, conv_id).await;
            }
        }
    }
}
