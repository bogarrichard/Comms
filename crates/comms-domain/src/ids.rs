//! Strongly typed identifiers.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::DomainError;

macro_rules! uuid_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Generate a new time-ordered (UUIDv7) identifier.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.as_hyphenated().fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = DomainError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s)
                    .map(Self)
                    .map_err(|_| DomainError::InvalidId(s.to_owned()))
            }
        }
    };
}

uuid_id!(
    /// Internal identifier of a configured account.
    AccountId
);
uuid_id!(
    /// Internal, stable identifier of a conversation (chat or channel).
    ConversationId
);
uuid_id!(
    /// Internal, stable identifier of a message. Exists before the provider
    /// has assigned a [`RemoteId`] (optimistic sends).
    MessageId
);

/// Identifier assigned by a provider. Opaque to the core: only the provider
/// adapter that produced it may interpret its structure.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RemoteId(String);

impl RemoteId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RemoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifies the backend implementation an entity came from (e.g. `"msgraph"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderKind(String);

impl ProviderKind {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Opaque pagination cursor owned by the provider that issued it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PageCursor(String);

impl PageCursor {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_through_string_and_serde() {
        let id = ConversationId::new();
        let parsed: ConversationId = id.to_string().parse().unwrap();
        assert_eq!(id, parsed);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        assert_eq!(serde_json::from_str::<ConversationId>(&json).unwrap(), id);
    }

    #[test]
    fn invalid_id_is_rejected() {
        assert!("nope".parse::<MessageId>().is_err());
    }

    #[test]
    fn new_ids_are_time_ordered() {
        let a = MessageId::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = MessageId::new();
        assert!(a < b);
    }

    #[test]
    fn different_id_types_do_not_mix() {
        // Compile-time property; this documents that the types are distinct.
        fn takes_conversation(_: ConversationId) {}
        takes_conversation(ConversationId::new());
    }
}
