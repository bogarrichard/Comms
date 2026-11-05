//! Application core: the only API frontends talk to.
//!
//! Frontends send [`Command`]s, receive [`AppEvent`]s and read data through
//! the query methods on [`AppHandle`] (which hit the local SQLite cache, never
//! the network). Nothing here depends on Ratatui, Slint or a specific provider.

mod engine;

use std::sync::Arc;
use std::time::Duration;

use comms_domain::{Account, Conversation, ConversationId, Message, MessageId, Participant};
use comms_provider_api::{Capabilities, MessagingProvider, ProviderError};
use comms_storage::{Store, StoreError};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc};

#[derive(Debug, Clone)]
pub struct CoreConfig {
    /// Messages fetched the first time a conversation is opened.
    pub initial_messages: u32,
    /// How often the chat list is refreshed.
    pub chat_poll: Duration,
    /// How often teams and channels are re-listed.
    pub channel_poll: Duration,
    /// How often the open conversation is refreshed.
    pub open_poll: Duration,
    /// Delay before re-checking a conversation after an uncertain send.
    pub reconcile_delay: Duration,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            initial_messages: 100,
            chat_poll: Duration::from_secs(60),
            channel_poll: Duration::from_secs(300),
            open_poll: Duration::from_secs(15),
            reconcile_delay: Duration::from_secs(4),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    /// Re-list chats and channels now.
    RefreshConversations,
    /// Select a conversation: clears local unread state and syncs it.
    OpenConversation(ConversationId),
    CloseConversation,
    /// Fetch the next page of older messages.
    LoadOlderMessages(ConversationId),
    SendMessage {
        conversation: ConversationId,
        text: String,
    },
    /// Send a `failed` or `uncertain` message again (explicit user action).
    RetryMessage(MessageId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connecting,
    Online,
    Offline(String),
    /// The session is no longer valid; the user has to sign in again.
    AuthRequired,
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    ConversationsUpdated,
    MessagesUpdated(ConversationId),
    MessageSendStarted(MessageId),
    MessageSendSucceeded(MessageId),
    MessageSendFailed {
        message: MessageId,
        reason: String,
        /// `true` when the message may nevertheless have been delivered.
        uncertain: bool,
    },
    /// `true` while at least one network request is in flight.
    SyncActivity(bool),
    /// A sync pass finished. `conversation` is `None` for the chat list.
    SyncCompleted {
        conversation: Option<ConversationId>,
    },
    ConnectionStatusChanged(ConnectionStatus),
    /// Channels could not be listed (typically missing admin consent).
    ChannelsUnavailable(String),
    /// Older messages were requested; `loaded` is how many were stored and
    /// `reached_start` whether there is nothing older.
    OlderMessagesLoaded {
        conversation: ConversationId,
        loaded: usize,
        reached_start: bool,
    },
    /// A non-fatal problem to show to the user.
    Notice(String),
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

/// Cheap to clone; all clones talk to the same engine.
#[derive(Clone)]
pub struct AppHandle {
    inner: Arc<engine::Inner>,
    commands: mpsc::UnboundedSender<Command>,
}

impl AppHandle {
    /// Start the engine. Uses the cached account when there is one, so a warm
    /// start never waits for the network; only the very first run asks the
    /// provider who the user is.
    pub async fn start(
        provider: Arc<dyn MessagingProvider>,
        store: Store,
        config: CoreConfig,
    ) -> Result<Self, AppError> {
        let kind = provider.kind();
        let (account, verify) = match store.first_account(&kind).await? {
            Some(a) => (a, true),
            None => {
                let me = provider.current_user().await?;
                (store.ensure_account(&kind, &me).await?, false)
            }
        };
        store.recover_interrupted_sends().await?;
        let (events, _) = broadcast::channel(512);
        let inner = Arc::new(engine::Inner::new(provider, store, account, config, events));
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(engine::run(inner.clone(), rx, verify));
        Ok(Self {
            inner,
            commands: tx,
        })
    }

    pub fn send(&self, command: Command) {
        // Only fails when the engine has stopped (shutdown).
        let _ = self.commands.send(command);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AppEvent> {
        self.inner.events.subscribe()
    }

    /// Which backend this handle talks to (e.g. `"msgraph"`, `"signal"`, `"fake"`).
    pub fn provider_kind(&self) -> comms_domain::ProviderKind {
        self.inner.provider.kind()
    }

    pub fn capabilities(&self) -> Capabilities {
        self.inner.provider.capabilities()
    }

    pub fn account(&self) -> &Account {
        &self.inner.account
    }

    pub fn me(&self) -> Participant {
        self.inner.me.clone()
    }

    pub fn connection_status(&self) -> ConnectionStatus {
        self.inner.status()
    }

    // ---- local-first queries (never touch the network) ----

    pub async fn conversations(&self) -> Result<Vec<Conversation>, AppError> {
        Ok(self
            .inner
            .store
            .list_conversations(self.inner.account.id)
            .await?)
    }

    pub async fn conversation(&self, id: ConversationId) -> Result<Option<Conversation>, AppError> {
        Ok(self.inner.store.get_conversation(id).await?)
    }

    /// The newest `limit` cached messages, oldest first.
    pub async fn messages(&self, id: ConversationId, limit: u32) -> Result<Vec<Message>, AppError> {
        Ok(self.inner.store.recent_messages(id, limit).await?)
    }

    /// Whether older messages can still be fetched from the provider.
    pub async fn has_older(&self, id: ConversationId) -> Result<bool, AppError> {
        let (cursor, done) = self.inner.store.backfill_state(id).await?;
        Ok(!done || cursor.is_some())
    }
}

#[cfg(test)]
mod tests;
