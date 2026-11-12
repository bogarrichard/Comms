//! Slack backend: the Web API with a **user token** (`xoxp-...`) that the
//! user created for their own workspace's Slack app (see `docs/slack.md`).
//!
//! Everything Slack-specific lives here: wire types, mrkdwn, error codes.
//! Polling only for now. Real-time (Socket Mode) is future work, so
//! `realtime_events` is `false`.
//!
//! Known gaps of this first version: no per-conversation last-activity or
//! read marker (Slack's list call has neither), thread replies are not
//! fetched, files show as `[file: name]`, and bold/italic markers stay in the
//! text. Apps distributed outside the workspace are throttled hard by Slack
//! (1 request per minute for history); see the docs for why a workspace-internal
//! app avoids that.

mod client;
mod model;
mod mrkdwn;
pub mod oauth;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use comms_domain::{
    ContentBlock, ConversationKind, PageCursor, Participant, ProviderKind, RemoteId,
};
use comms_provider_api::{
    Capabilities, ConversationClass, MessagingProvider, OutgoingMessage, Page, PageRequest,
    ProviderError, RemoteConversation, RemoteMessage,
};
use futures::StreamExt;
use tokio::sync::OnceCell;
use tracing::warn;

pub use client::{SLACK_BASE, SlackClient};
use model::{
    AuthTest, ConversationInfo, ConversationsList, History, PostResponse, SChannel, SMessage,
    TeamInfo, UserInfo, next_cursor, parse_ts, pretty_mpim_name,
};

pub const PROVIDER_KIND: &str = "slack";

/// A chat's last-message time is re-read from Slack at most this often
/// (new messages in the open chat are seen through its own polling).
const ACTIVITY_TTL: Duration = Duration::from_secs(300);
/// Chat id to (when it was looked up, time of its latest message).
type ActivityCache = Mutex<HashMap<String, (Instant, Option<DateTime<Utc>>)>>;
/// Parallel "latest message" lookups when ordering chats.
const ACTIVITY_LOOKUPS: usize = 4;

/// Slack tokens start with `xox` (`xoxp-` user, `xoxb-` bot).
pub fn looks_like_token(s: &str) -> bool {
    let s = s.trim();
    s.starts_with("xox") && s.len() > 10 && s.bytes().all(|b| b.is_ascii_graphic())
}

/// User-token scopes the Slack app needs, for the setup docs and error hints.
pub mod scopes {
    pub const USER: &[&str] = &[
        "channels:read",
        "groups:read",
        "im:read",
        "mpim:read",
        "channels:history",
        "groups:history",
        "im:history",
        "mpim:history",
        "users:read",
        "chat:write",
        "team:read",
    ];
}

/// The workspace's name and, when it has set one, its icon (PNG/JPEG bytes).
#[derive(Debug, Clone)]
pub struct WorkspaceInfo {
    pub name: String,
    pub icon: Option<Vec<u8>>,
}

pub struct SlackProvider {
    client: SlackClient,
    me: OnceCell<Participant>,
    /// user id to display name; only successful lookups are stored.
    users: Mutex<HashMap<String, String>>,
    /// Only list channels you are a member of (`users.conversations`).
    member_only: bool,
    activity: ActivityCache,
}

impl SlackProvider {
    pub fn new(token: impl Into<String>) -> Result<Self, ProviderError> {
        Self::with_base_url(token, SLACK_BASE)
    }

    /// Workspace name and icon (needs the `team:read` scope). A failed icon
    /// download is not an error: the name alone is still returned.
    pub async fn workspace(&self) -> Result<WorkspaceInfo, ProviderError> {
        let info: TeamInfo = self.client.call("team.info", &[]).await?;
        let icon = match info.team.icon.as_ref().and_then(|i| i.best_url()) {
            Some(url) => match self.client.fetch_image(url).await {
                Ok(bytes) => Some(bytes),
                Err(e) => {
                    warn!("workspace icon not downloaded: {e}");
                    None
                }
            },
            None => None,
        };
        Ok(WorkspaceInfo {
            name: info.team.name,
            icon,
        })
    }

    pub fn with_base_url(token: impl Into<String>, base: &str) -> Result<Self, ProviderError> {
        Ok(Self {
            client: SlackClient::new(base, token.into().trim().to_owned())?,
            me: OnceCell::new(),
            users: Mutex::new(HashMap::new()),
            member_only: true,
            activity: Mutex::new(HashMap::new()),
        })
    }

    /// `true` (the default): list only the channels you have joined.
    /// `false`: every public channel in the workspace plus your private ones.
    #[must_use]
    pub fn member_only(mut self, member_only: bool) -> Self {
        self.member_only = member_only;
        self
    }

    /// Time of the newest message in `id`, cached for [`ACTIVITY_TTL`]. Slack's
    /// list calls carry no activity time, and chats must be ordered by it.
    async fn latest_activity(&self, id: &str) -> Option<DateTime<Utc>> {
        let cached = self.activity.lock().ok().and_then(|m| m.get(id).cloned());
        if let Some((at, value)) = cached
            && at.elapsed() < ACTIVITY_TTL
        {
            return value;
        }
        let found = match self
            .client
            .call::<History>("conversations.history", &[("channel", id), ("limit", "1")])
            .await
        {
            Ok(h) => h.messages.first().and_then(|m| parse_ts(&m.ts)),
            Err(e) => {
                warn!("latest message of a chat not read: {e}");
                // Keep what we knew; try again next time.
                return cached_value(&self.activity, id);
            }
        };
        if let Ok(mut m) = self.activity.lock() {
            m.insert(id.to_owned(), (Instant::now(), found));
        }
        found
    }

    fn cached_name(&self, id: &str) -> Option<String> {
        self.users.lock().expect("users lock").get(id).cloned()
    }

    /// Display name for a user id. Falls back to the id itself when the lookup
    /// fails (for example because the token lacks `users:read`).
    async fn display_name(&self, id: &str) -> String {
        if let Some(name) = self.cached_name(id) {
            return name;
        }
        match self
            .client
            .call::<UserInfo>("users.info", &[("user", id)])
            .await
        {
            Ok(info) => {
                if let Some(name) = info.user.best_name() {
                    self.users
                        .lock()
                        .expect("users lock")
                        .insert(id.to_owned(), name.clone());
                    return name;
                }
            }
            Err(e) => warn!("users.info failed: {e}"),
        }
        id.to_owned()
    }

    async fn conversation(&self, ch: SChannel) -> RemoteConversation {
        let (kind, title) = if ch.is_im {
            let title = match ch.user.as_deref() {
                Some(u) => self.display_name(u).await,
                None => "Direct message".to_owned(),
            };
            (ConversationKind::Direct, title)
        } else if ch.is_mpim {
            let name = ch.name.as_deref().unwrap_or(&ch.id);
            (ConversationKind::Group, pretty_mpim_name(name))
        } else {
            let name = ch.name.as_deref().unwrap_or(&ch.id);
            (ConversationKind::Channel, format!("#{name}"))
        };
        RemoteConversation {
            remote_id: RemoteId::new(ch.id),
            kind,
            title,
            group: None,
            last_activity: None,
            last_read: None,
            preview: None,
            last_message_from_me: false,
        }
    }

    async fn message(&self, m: SMessage) -> Option<RemoteMessage> {
        let Some(timestamp) = parse_ts(&m.ts) else {
            warn!("skipping a Slack message with an unreadable timestamp");
            return None;
        };

        let mut names = HashMap::new();
        for id in mrkdwn::mention_ids(&m.text) {
            let name = self.display_name(&id).await;
            names.insert(id, name);
        }
        let mut content = mrkdwn::mrkdwn_to_blocks(&m.text, &names);
        for file in &m.files {
            content.push(ContentBlock::Unsupported {
                kind: "file".to_owned(),
                fallback: format!("[file: {}]", file.label()),
            });
        }

        let sender = if let Some(user) = m.user.as_deref() {
            Participant {
                remote_id: RemoteId::new(user),
                display_name: self.display_name(user).await,
            }
        } else {
            let bot = m.bot_id.as_deref().unwrap_or("bot");
            Participant {
                remote_id: RemoteId::new(bot),
                display_name: m.username.clone().unwrap_or_else(|| "bot".to_owned()),
            }
        };

        Some(RemoteMessage {
            remote_id: RemoteId::new(m.ts.clone()),
            sender,
            timestamp,
            content,
            attachments: Vec::new(),
            edited_at: m.edited.as_ref().and_then(|e| parse_ts(&e.ts)),
            deleted_at: None,
            // Thread replies point at their parent; the parent points at itself.
            reply_to: m
                .thread_ts
                .as_ref()
                .filter(|t| **t != m.ts)
                .map(|t| RemoteId::new(t.clone())),
            is_system: m.is_system(),
        })
    }
}

/// Slack accepts at most 200 for list-style calls we make; 0 means default.
fn cached_value(cache: &ActivityCache, id: &str) -> Option<DateTime<Utc>> {
    cache
        .lock()
        .ok()
        .and_then(|m| m.get(id).and_then(|(_, v)| *v))
}

fn page_limit(page: &PageRequest) -> String {
    let limit = if page.limit == 0 { 100 } else { page.limit };
    limit.clamp(1, 200).to_string()
}

#[async_trait]
impl MessagingProvider for SlackProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::new(PROVIDER_KIND)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            direct_messages: true,
            group_conversations: true,
            channels: true,
            edit_messages: true,
            delete_messages: true,
            ..Capabilities::default()
        }
    }

    async fn current_user(&self) -> Result<Participant, ProviderError> {
        self.me
            .get_or_try_init(|| async {
                let me: AuthTest = self.client.call("auth.test", &[]).await?;
                let display_name = match self.display_name(&me.user_id).await {
                    n if n == me.user_id && !me.user.is_empty() => me.user.clone(),
                    n => n,
                };
                Ok(Participant {
                    remote_id: RemoteId::new(me.user_id),
                    display_name,
                })
            })
            .await
            .cloned()
    }

    async fn list_conversations(
        &self,
        class: ConversationClass,
        page: PageRequest,
    ) -> Result<Page<RemoteConversation>, ProviderError> {
        let types = match class {
            ConversationClass::Chats => "im,mpim",
            ConversationClass::Channels => "public_channel,private_channel",
        };
        let limit = page_limit(&page);
        let mut params = vec![
            ("types", types),
            ("exclude_archived", "true"),
            ("limit", limit.as_str()),
        ];
        if let Some(c) = page.cursor.as_ref() {
            params.push(("cursor", c.as_str()));
        }
        // `users.conversations` lists only what you are in; chats always are.
        let api = if class == ConversationClass::Channels && self.member_only {
            "users.conversations"
        } else {
            "conversations.list"
        };
        let list: ConversationsList = self.client.call(api, &params).await?;
        let mut items = Vec::with_capacity(list.channels.len());
        for ch in list.channels {
            items.push(self.conversation(ch).await);
        }
        if class == ConversationClass::Chats {
            // Newest chats first needs each chat's latest message time.
            let ids: Vec<String> = items.iter().map(|c| c.remote_id.to_string()).collect();
            let times: Vec<Option<DateTime<Utc>>> = futures::stream::iter(ids)
                .map(|id| async move { self.latest_activity(&id).await })
                .buffered(ACTIVITY_LOOKUPS)
                .collect()
                .await;
            for (c, t) in items.iter_mut().zip(times) {
                c.last_activity = t;
            }
        }
        Ok(Page {
            items,
            next: next_cursor(list.response_metadata.as_ref()).map(PageCursor::new),
        })
    }

    async fn get_conversation(
        &self,
        conversation: &RemoteId,
    ) -> Result<RemoteConversation, ProviderError> {
        let info: ConversationInfo = self
            .client
            .call("conversations.info", &[("channel", conversation.as_str())])
            .await?;
        Ok(self.conversation(info.channel).await)
    }

    async fn get_messages(
        &self,
        conversation: &RemoteId,
        page: PageRequest,
    ) -> Result<Page<RemoteMessage>, ProviderError> {
        let limit = page_limit(&page);
        let mut params = vec![
            ("channel", conversation.as_str()),
            ("limit", limit.as_str()),
        ];
        if let Some(c) = page.cursor.as_ref() {
            params.push(("cursor", c.as_str()));
        }
        // Slack returns newest first, as the trait requires.
        let history: History = self.client.call("conversations.history", &params).await?;
        let mut items = Vec::with_capacity(history.messages.len());
        for m in history.messages {
            if let Some(m) = self.message(m).await {
                items.push(m);
            }
        }
        Ok(Page {
            items,
            next: next_cursor(history.response_metadata.as_ref()).map(PageCursor::new),
        })
    }

    async fn send_message(
        &self,
        conversation: &RemoteId,
        message: OutgoingMessage,
    ) -> Result<RemoteMessage, ProviderError> {
        // No retry here, whatever the error: the caller owns that decision.
        let posted: PostResponse = self
            .client
            .call(
                "chat.postMessage",
                &[
                    ("channel", conversation.as_str()),
                    ("text", &message.text),
                    // Without this Slack attributes the post to the *app*
                    // ("Comms", with an APP tag) instead of to you.
                    ("as_user", "true"),
                ],
            )
            .await?;
        let mut msg = posted.message.unwrap_or_else(|| SMessage {
            ts: posted.ts.clone(),
            user: None,
            bot_id: None,
            username: None,
            text: message.text.clone(),
            subtype: None,
            thread_ts: None,
            edited: None,
            files: Vec::new(),
        });
        msg.ts = posted.ts;
        // Posting with a user token always speaks as that user.
        let me = self.current_user().await?;
        msg.user = Some(me.remote_id.as_str().to_owned());
        self.message(msg).await.ok_or_else(|| {
            ProviderError::InvalidResponse("chat.postMessage returned a bad timestamp".into())
        })
    }

    async fn edit_message(
        &self,
        conversation: &RemoteId,
        message: &RemoteId,
        new_text: String,
    ) -> Result<(), ProviderError> {
        let _: serde_json::Value = self
            .client
            .call(
                "chat.update",
                &[
                    ("channel", conversation.as_str()),
                    ("ts", message.as_str()),
                    ("text", &new_text),
                ],
            )
            .await?;
        Ok(())
    }

    async fn delete_message(
        &self,
        conversation: &RemoteId,
        message: &RemoteId,
    ) -> Result<(), ProviderError> {
        let _: serde_json::Value = self
            .client
            .call(
                "chat.delete",
                &[("channel", conversation.as_str()), ("ts", message.as_str())],
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
