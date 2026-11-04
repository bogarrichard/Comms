//! Provider-independent domain model for Comms.
//!
//! Nothing in this crate knows about Microsoft Graph or any other backend.
//! Every provider entity has a stable *internal* identifier (a UUIDv7 newtype)
//! and, separately, an opaque provider identifier ([`RemoteId`]).

pub mod content;
pub mod error;
pub mod ids;
pub mod model;

pub use content::{ContentBlock, Inline, TextStyle, blocks_to_plain_text};
pub use error::DomainError;
pub use ids::{AccountId, ConversationId, MessageId, PageCursor, ProviderKind, RemoteId};
pub use model::{
    Account, Attachment, Conversation, ConversationKind, Message, Participant, SendState,
};
