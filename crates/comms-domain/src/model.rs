use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::content::ContentBlock;
use crate::error::DomainError;
use crate::ids::{AccountId, ConversationId, MessageId, ProviderKind, RemoteId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    pub provider: ProviderKind,
    pub remote_id: RemoteId,
    pub display_name: String,
}

/// A person as seen by the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    pub remote_id: RemoteId,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    Direct,
    Group,
    Meeting,
    Channel,
}

impl ConversationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConversationKind::Direct => "direct",
            ConversationKind::Group => "group",
            ConversationKind::Meeting => "meeting",
            ConversationKind::Channel => "channel",
        }
    }

    pub fn parse(s: &str) -> Result<Self, DomainError> {
        Ok(match s {
            "direct" => ConversationKind::Direct,
            "group" => ConversationKind::Group,
            "meeting" => ConversationKind::Meeting,
            "channel" => ConversationKind::Channel,
            other => {
                return Err(DomainError::UnknownValue {
                    what: "ConversationKind",
                    value: other.to_owned(),
                });
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: ConversationId,
    pub account_id: AccountId,
    pub remote_id: RemoteId,
    pub kind: ConversationKind,
    pub title: String,
    /// Containing group, e.g. the team name for a channel.
    pub group: Option<String>,
    pub last_activity: Option<DateTime<Utc>>,
    /// Plain-text preview of the most recent message.
    pub preview: Option<String>,
    pub unread: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendState {
    /// Confirmed by the provider (or received from it).
    Sent,
    /// Queued locally; the request has not been issued yet or is in flight.
    Pending,
    /// The provider definitely did not accept it. Safe to retry manually.
    Failed,
    /// The request may or may not have reached the provider. Never resent
    /// automatically; resolved when the message shows up in a sync.
    Uncertain,
}

impl SendState {
    pub fn as_str(&self) -> &'static str {
        match self {
            SendState::Sent => "sent",
            SendState::Pending => "pending",
            SendState::Failed => "failed",
            SendState::Uncertain => "uncertain",
        }
    }

    pub fn parse(s: &str) -> Result<Self, DomainError> {
        Ok(match s {
            "sent" => SendState::Sent,
            "pending" => SendState::Pending,
            "failed" => SendState::Failed,
            "uncertain" => SendState::Uncertain,
            other => {
                return Err(DomainError::UnknownValue {
                    what: "SendState",
                    value: other.to_owned(),
                });
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub remote_id: Option<RemoteId>,
    pub sender: Participant,
    pub is_mine: bool,
    pub timestamp: DateTime<Utc>,
    pub content: Vec<ContentBlock>,
    pub attachments: Vec<Attachment>,
    pub edited_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    /// Remote id of the message this one replies to (channel threads).
    pub reply_to: Option<RemoteId>,
    pub send_state: SendState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip_through_strings() {
        for k in [
            ConversationKind::Direct,
            ConversationKind::Group,
            ConversationKind::Meeting,
            ConversationKind::Channel,
        ] {
            assert_eq!(ConversationKind::parse(k.as_str()).unwrap(), k);
        }
        for s in [
            SendState::Sent,
            SendState::Pending,
            SendState::Failed,
            SendState::Uncertain,
        ] {
            assert_eq!(SendState::parse(s.as_str()).unwrap(), s);
        }
    }

    #[test]
    fn unknown_enum_values_are_errors() {
        assert!(ConversationKind::parse("bogus").is_err());
        assert!(SendState::parse("bogus").is_err());
    }
}
