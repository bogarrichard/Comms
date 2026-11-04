//! The contract between the application core and a messaging backend.
//!
//! The core only ever talks to `dyn MessagingProvider`. Anything a backend
//! cannot do is reported through [`Capabilities`] and, if called anyway,
//! [`ProviderError::Unsupported`].

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use comms_domain::{
    Attachment, ContentBlock, ConversationKind, PageCursor, Participant, ProviderKind, RemoteId,
};

mod error;

pub use error::{Capability, ProviderError};

/// What a provider can do. Frontends use this to hide unavailable actions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub direct_messages: bool,
    pub group_conversations: bool,
    pub channels: bool,
    pub edit_messages: bool,
    pub delete_messages: bool,
    pub reactions: bool,
    pub attachments: bool,
    pub presence: bool,
    pub typing_indicators: bool,
    pub search: bool,
    pub threads: bool,
    /// The provider can push changes. Polling is used when this is false.
    pub realtime_events: bool,
}

/// Which family of conversations to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationClass {
    /// Direct, group and meeting chats.
    Chats,
    /// Channels inside teams/groups.
    Channels,
}

#[derive(Debug, Clone, Default)]
pub struct PageRequest {
    pub cursor: Option<PageCursor>,
    /// Maximum items wanted; providers may return fewer.
    pub limit: u32,
}

impl PageRequest {
    pub fn first(limit: u32) -> Self {
        Self {
            cursor: None,
            limit,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// `Some` when more items can be fetched with this cursor.
    pub next: Option<PageCursor>,
}

impl<T> Page<T> {
    pub fn single(items: Vec<T>) -> Self {
        Self { items, next: None }
    }
}

/// A conversation as reported by a provider (no internal id yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteConversation {
    pub remote_id: RemoteId,
    pub kind: ConversationKind,
    pub title: String,
    pub group: Option<String>,
    pub last_activity: Option<DateTime<Utc>>,
    /// When the signed-in user last read the conversation, if the provider says.
    pub last_read: Option<DateTime<Utc>>,
    pub preview: Option<String>,
    pub last_message_from_me: bool,
}

/// A message as reported by a provider (no internal id yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMessage {
    pub remote_id: RemoteId,
    pub sender: Participant,
    pub timestamp: DateTime<Utc>,
    pub content: Vec<ContentBlock>,
    pub attachments: Vec<Attachment>,
    pub edited_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub reply_to: Option<RemoteId>,
    /// Join/leave/rename events and similar. Shown differently by frontends.
    pub is_system: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingMessage {
    pub text: String,
}

#[async_trait]
pub trait MessagingProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn capabilities(&self) -> Capabilities;

    /// The signed-in user.
    async fn current_user(&self) -> Result<Participant, ProviderError>;

    async fn list_conversations(
        &self,
        class: ConversationClass,
        page: PageRequest,
    ) -> Result<Page<RemoteConversation>, ProviderError>;

    /// Messages of one conversation, **newest first**.
    async fn get_messages(
        &self,
        conversation: &RemoteId,
        page: PageRequest,
    ) -> Result<Page<RemoteMessage>, ProviderError>;

    /// Send a message. Must never retry internally after the request may have
    /// reached the server: the caller owns that decision.
    async fn send_message(
        &self,
        conversation: &RemoteId,
        message: OutgoingMessage,
    ) -> Result<RemoteMessage, ProviderError>;

    async fn get_conversation(
        &self,
        _conversation: &RemoteId,
    ) -> Result<RemoteConversation, ProviderError> {
        Err(ProviderError::Unsupported(Capability::GetConversation))
    }

    async fn edit_message(
        &self,
        _conversation: &RemoteId,
        _message: &RemoteId,
        _new_text: String,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported(Capability::EditMessages))
    }

    async fn delete_message(
        &self,
        _conversation: &RemoteId,
        _message: &RemoteId,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported(Capability::DeleteMessages))
    }

    async fn search_messages(
        &self,
        _query: &str,
        _page: PageRequest,
    ) -> Result<Page<RemoteMessage>, ProviderError> {
        Err(ProviderError::Unsupported(Capability::Search))
    }

    async fn list_participants(
        &self,
        _conversation: &RemoteId,
    ) -> Result<Vec<Participant>, ProviderError> {
        Err(ProviderError::Unsupported(Capability::ListParticipants))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Minimal;

    #[async_trait]
    impl MessagingProvider for Minimal {
        fn kind(&self) -> ProviderKind {
            ProviderKind::new("minimal")
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        async fn current_user(&self) -> Result<Participant, ProviderError> {
            Err(ProviderError::Unauthorized)
        }
        async fn list_conversations(
            &self,
            _: ConversationClass,
            _: PageRequest,
        ) -> Result<Page<RemoteConversation>, ProviderError> {
            Ok(Page::single(vec![]))
        }
        async fn get_messages(
            &self,
            _: &RemoteId,
            _: PageRequest,
        ) -> Result<Page<RemoteMessage>, ProviderError> {
            Ok(Page::single(vec![]))
        }
        async fn send_message(
            &self,
            _: &RemoteId,
            _: OutgoingMessage,
        ) -> Result<RemoteMessage, ProviderError> {
            Err(ProviderError::Unsupported(Capability::SendMessages))
        }
    }

    #[tokio::test]
    async fn optional_operations_report_unsupported() {
        let p = Minimal;
        let id = RemoteId::new("x");
        assert_eq!(
            p.edit_message(&id, &id, "t".into()).await,
            Err(ProviderError::Unsupported(Capability::EditMessages))
        );
        assert_eq!(
            p.delete_message(&id, &id).await,
            Err(ProviderError::Unsupported(Capability::DeleteMessages))
        );
        assert!(matches!(
            p.search_messages("q", PageRequest::first(5)).await,
            Err(ProviderError::Unsupported(Capability::Search))
        ));
        assert!(matches!(
            p.list_participants(&id).await,
            Err(ProviderError::Unsupported(Capability::ListParticipants))
        ));
    }

    #[test]
    fn page_helpers() {
        let p = Page::single(vec![1, 2]);
        assert!(p.next.is_none());
        assert_eq!(PageRequest::first(10).limit, 10);
    }
}
