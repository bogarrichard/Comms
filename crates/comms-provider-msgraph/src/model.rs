//! Graph wire types (private to this crate) and their mapping to the
//! provider-independent API types. Responses are validated here: items that
//! lack mandatory fields are skipped rather than trusted.

use chrono::{DateTime, Utc};
use comms_domain::{Attachment, ConversationKind, Participant, RemoteId};
use comms_provider_api::{RemoteConversation, RemoteMessage};
use serde::Deserialize;
use tracing::warn;

use crate::html::body_to_blocks;

#[derive(Debug, Deserialize)]
pub struct Collection<T> {
    #[serde(default = "Vec::new")]
    pub value: Vec<T>,
    #[serde(rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GUser {
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub user_principal_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GIdentitySet {
    pub user: Option<GUser>,
    pub application: Option<GUser>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GBody {
    pub content_type: Option<String>,
    pub content: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GAttachment {
    pub content_type: Option<String>,
    pub content_url: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GMessage {
    pub id: Option<String>,
    pub reply_to_id: Option<String>,
    pub message_type: Option<String>,
    pub created_date_time: Option<String>,
    pub last_edited_date_time: Option<String>,
    pub deleted_date_time: Option<String>,
    pub from: Option<GIdentitySet>,
    pub body: Option<GBody>,
    #[serde(default)]
    pub attachments: Vec<GAttachment>,
    #[serde(default)]
    pub replies: Vec<GMessage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GViewpoint {
    pub is_hidden: Option<bool>,
    pub last_message_read_date_time: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GMember {
    pub display_name: Option<String>,
    pub user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GChat {
    pub id: Option<String>,
    pub topic: Option<String>,
    pub chat_type: Option<String>,
    pub viewpoint: Option<GViewpoint>,
    #[serde(default)]
    pub members: Vec<GMember>,
    pub last_message_preview: Option<GMessage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GTeam {
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub is_archived: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GChannel {
    pub id: Option<String>,
    pub display_name: Option<String>,
}

pub fn parse_time(s: &Option<String>) -> Option<DateTime<Utc>> {
    s.as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
}

/// How a conversation is addressed in Graph URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Chat(String),
    Channel { team: String, channel: String },
}

const CHANNEL_PREFIX: &str = "ch/";

pub fn channel_remote_id(team: &str, channel: &str) -> RemoteId {
    RemoteId::new(format!("{CHANNEL_PREFIX}{team}/{channel}"))
}

pub fn chat_remote_id(chat: &str) -> RemoteId {
    RemoteId::new(chat)
}

pub fn parse_target(id: &RemoteId) -> Option<Target> {
    let s = id.as_str();
    match s.strip_prefix(CHANNEL_PREFIX) {
        Some(rest) => {
            let (team, channel) = rest.split_once('/')?;
            (!team.is_empty() && !channel.is_empty() && !channel.contains('/')).then(|| {
                Target::Channel {
                    team: team.to_owned(),
                    channel: channel.to_owned(),
                }
            })
        }
        None => (!s.is_empty()).then(|| Target::Chat(s.to_owned())),
    }
}

fn name_of(u: &GUser) -> String {
    u.display_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or_else(|| u.user_principal_name.clone())
        .unwrap_or_else(|| "Unknown".to_owned())
}

pub fn sender_of(from: &Option<GIdentitySet>) -> Participant {
    match from {
        Some(GIdentitySet { user: Some(u), .. }) => Participant {
            remote_id: RemoteId::new(u.id.clone().unwrap_or_default()),
            display_name: name_of(u),
        },
        Some(GIdentitySet {
            application: Some(a),
            ..
        }) => Participant {
            remote_id: RemoteId::new(format!("app:{}", a.id.clone().unwrap_or_default())),
            display_name: name_of(a),
        },
        _ => Participant {
            remote_id: RemoteId::new(""),
            display_name: "System".to_owned(),
        },
    }
}

fn attachment_label(a: &GAttachment) -> Option<Attachment> {
    // Only file references carry a meaningful name; cards/forwards are labelled.
    let kind = a.content_type.as_deref().unwrap_or("");
    let name = match (kind, a.name.as_deref().filter(|n| !n.is_empty())) {
        ("reference", Some(n)) => n.to_owned(),
        ("forwardedMessageReference", _) => "forwarded message".to_owned(),
        (_, Some(n)) => n.to_owned(),
        _ => return None,
    };
    Some(Attachment {
        name,
        url: a.content_url.clone(),
    })
}

/// Map one Graph message. Returns `None` (with a warning) if it is unusable.
pub fn map_message(m: &GMessage) -> Option<RemoteMessage> {
    let id = m.id.as_deref().filter(|i| !i.is_empty());
    let created = parse_time(&m.created_date_time);
    let (Some(id), Some(created)) = (id, created) else {
        warn!("skipping message with missing id or timestamp");
        return None;
    };
    let body = m.body.as_ref();
    let content = body
        .map(|b| {
            body_to_blocks(
                b.content_type.as_deref().unwrap_or("html"),
                b.content.as_deref().unwrap_or(""),
            )
        })
        .unwrap_or_default();
    Some(RemoteMessage {
        remote_id: RemoteId::new(id),
        sender: sender_of(&m.from),
        timestamp: created,
        content,
        attachments: m.attachments.iter().filter_map(attachment_label).collect(),
        edited_at: parse_time(&m.last_edited_date_time),
        deleted_at: parse_time(&m.deleted_date_time),
        reply_to: m
            .reply_to_id
            .as_deref()
            .filter(|r| !r.is_empty())
            .map(RemoteId::new),
        is_system: m.message_type.as_deref().is_some_and(|t| t != "message"),
    })
}

/// Chat message list → newest first. Channel roots are flattened with their
/// expanded replies.
pub fn map_messages(items: &[GMessage]) -> Vec<RemoteMessage> {
    let mut out: Vec<RemoteMessage> = items
        .iter()
        .flat_map(|m| std::iter::once(m).chain(m.replies.iter()))
        .filter_map(map_message)
        .collect();
    out.sort_by_key(|m| std::cmp::Reverse(m.timestamp));
    out
}

fn chat_kind(t: Option<&str>) -> ConversationKind {
    match t {
        Some("oneOnOne") => ConversationKind::Direct,
        Some("meeting") => ConversationKind::Meeting,
        _ => ConversationKind::Group,
    }
}

fn chat_title(chat: &GChat, kind: ConversationKind, me: &str) -> String {
    if let Some(t) = chat
        .topic
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        return t.to_owned();
    }
    let others: Vec<&str> = chat
        .members
        .iter()
        .filter(|m| m.user_id.as_deref() != Some(me))
        .filter_map(|m| m.display_name.as_deref())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .collect();
    match kind {
        ConversationKind::Direct => others.first().map_or("Chat", |n| n).to_owned(),
        _ if others.is_empty() => match kind {
            ConversationKind::Meeting => "Meeting chat".to_owned(),
            _ => "Group chat".to_owned(),
        },
        _ => {
            let shown = others
                .iter()
                .take(3)
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            match others.len().saturating_sub(3) {
                0 => shown,
                n => format!("{shown} +{n}"),
            }
        }
    }
}

/// Map a chat; hidden chats and chats without an id are dropped.
pub fn map_chat(chat: &GChat, me: &str) -> Option<RemoteConversation> {
    let id = chat.id.as_deref().filter(|i| !i.is_empty())?;
    if chat.viewpoint.as_ref().and_then(|v| v.is_hidden) == Some(true) {
        return None;
    }
    let kind = chat_kind(chat.chat_type.as_deref());
    let preview = chat.last_message_preview.as_ref();
    let (preview_text, from_me, activity) = match preview {
        Some(p) => {
            let system = p.message_type.as_deref().is_some_and(|t| t != "message");
            let text = if system {
                "(chat event)".to_owned()
            } else if p.deleted_date_time.is_some() {
                "(message deleted)".to_owned()
            } else {
                let b = p.body.as_ref();
                let blocks = body_to_blocks(
                    b.and_then(|b| b.content_type.as_deref()).unwrap_or("html"),
                    b.and_then(|b| b.content.as_deref()).unwrap_or(""),
                );
                comms_domain::blocks_to_plain_text(&blocks)
            };
            // System events never count as unread activity from others.
            let from_me = system
                || p.from
                    .as_ref()
                    .and_then(|f| f.user.as_ref())
                    .and_then(|u| u.id.as_deref())
                    == Some(me);
            (Some(text), from_me, parse_time(&p.created_date_time))
        }
        None => (None, false, None),
    };
    Some(RemoteConversation {
        remote_id: chat_remote_id(id),
        kind,
        title: chat_title(chat, kind, me),
        group: None,
        last_activity: activity,
        last_read: chat
            .viewpoint
            .as_ref()
            .and_then(|v| parse_time(&v.last_message_read_date_time)),
        preview: preview_text,
        last_message_from_me: from_me,
    })
}
