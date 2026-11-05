//! Microsoft Graph (v1.0) adapter implementing `MessagingProvider` for Teams
//! chats and team channels, using documented delegated APIs only.
//!
//! Everything Microsoft-specific lives in this crate: wire types, HTML bodies,
//! URL layout and throttling behaviour.

mod client;
mod html;
mod model;

use std::sync::Arc;

use async_trait::async_trait;
use comms_domain::{PageCursor, Participant, ProviderKind, RemoteId};
use comms_provider_api::{
    Capabilities, ConversationClass, MessagingProvider, OutgoingMessage, Page, PageRequest,
    ProviderError, RemoteConversation, RemoteMessage,
};
use futures::StreamExt;
use serde::Serialize;
use tokio::sync::OnceCell;
use tracing::{debug, warn};

pub use client::{AccessTokenSource, GRAPH_BASE, GraphClient, RetryPolicy};
pub use html::{body_to_blocks, html_to_blocks};
use model::{
    Collection, GChannel, GChat, GMessage, GTeam, GUser, Target, channel_remote_id, map_chat,
    map_message, map_messages, parse_target,
};

pub const PROVIDER_KIND: &str = "msgraph";

/// Delegated scopes by feature. Channel *reading* needs admin consent in most
/// tenants, so the tiers are separate and chosen by the caller.
pub mod scopes {
    pub const BASE: &[&str] = &[
        "openid",
        "profile",
        "offline_access",
        "User.Read",
        "Chat.ReadBasic",
        "Chat.Read",
        "ChatMessage.Send",
    ];
    /// List teams/channels and post to channels (no admin consent required).
    pub const CHANNELS_BASIC: &[&str] = &[
        "Team.ReadBasic.All",
        "Channel.ReadBasic.All",
        "ChannelMessage.Send",
    ];
    /// Read channel messages (**admin consent required**).
    pub const CHANNELS_READ: &[&str] = &["ChannelMessage.Read.All"];
}

pub struct MsGraphProvider {
    client: GraphClient,
    me: OnceCell<Participant>,
    channels: bool,
}

#[derive(Serialize)]
struct SendBody<'a> {
    body: SendContent<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SendContent<'a> {
    content_type: &'a str,
    content: &'a str,
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\r' => {}
            '\n' => out.push_str("<br>"),
            c => out.push(c),
        }
    }
    out
}

impl MsGraphProvider {
    pub fn new(tokens: Arc<dyn AccessTokenSource>) -> Result<Self, ProviderError> {
        Self::with_base_url(GRAPH_BASE, tokens)
    }

    pub fn with_base_url(
        base: &str,
        tokens: Arc<dyn AccessTokenSource>,
    ) -> Result<Self, ProviderError> {
        Ok(Self::with_client(GraphClient::new(base, tokens)?))
    }

    pub fn with_client(client: GraphClient) -> Self {
        Self {
            client,
            me: OnceCell::new(),
            channels: true,
        }
    }

    /// Enable or disable teams/channels (off when the permissions were not requested).
    pub fn with_channels(mut self, enabled: bool) -> Self {
        self.channels = enabled;
        self
    }

    async fn me(&self) -> Result<&Participant, ProviderError> {
        self.me
            .get_or_try_init(|| async {
                let mut url = self.client.url(&["me"]);
                url.query_pairs_mut()
                    .append_pair("$select", "id,displayName,userPrincipalName");
                let u: GUser = self.client.get(url).await?;
                let id =
                    u.id.clone().filter(|i| !i.is_empty()).ok_or_else(|| {
                        ProviderError::InvalidResponse("/me returned no id".into())
                    })?;
                let name = u
                    .display_name
                    .or(u.user_principal_name)
                    .unwrap_or_else(|| "Me".to_owned());
                Ok(Participant {
                    remote_id: RemoteId::new(id),
                    display_name: name,
                })
            })
            .await
    }

    fn chat_list_url(&self) -> url::Url {
        let mut url = self.client.url(&["me", "chats"]);
        url.query_pairs_mut()
            .append_pair("$top", "50")
            .append_pair("$expand", "members,lastMessagePreview")
            .append_pair("$orderby", "lastMessagePreview/createdDateTime desc");
        url
    }

    async fn list_chats(
        &self,
        page: PageRequest,
    ) -> Result<Page<RemoteConversation>, ProviderError> {
        let me = self.me().await?.remote_id.as_str().to_owned();
        let first = page.cursor.is_none();
        let url = match &page.cursor {
            Some(c) => self.client.validate_link(c.as_str())?,
            None => self.chat_list_url(),
        };
        let result: Result<Collection<GChat>, _> = self.client.get(url).await;
        let list = match result {
            Ok(l) => l,
            // Some tenants/API versions reject the combined orderby+expand query.
            Err(ProviderError::BadRequest(msg)) if first => {
                warn!("chat list query rejected ({msg}); retrying without $orderby");
                let mut url = self.client.url(&["me", "chats"]);
                url.query_pairs_mut()
                    .append_pair("$top", "50")
                    .append_pair("$expand", "members,lastMessagePreview");
                self.client.get(url).await?
            }
            Err(e) => return Err(e),
        };
        let items = list.value.iter().filter_map(|c| map_chat(c, &me)).collect();
        let next = list.next_link.map(PageCursor::new);
        Ok(Page { items, next })
    }

    async fn list_channels(&self) -> Result<Page<RemoteConversation>, ProviderError> {
        let mut url = self.client.url(&["me", "joinedTeams"]);
        url.query_pairs_mut()
            .append_pair("$select", "id,displayName,isArchived");
        let teams: Collection<GTeam> = self.client.get(url).await?;
        let teams: Vec<(String, String)> = teams
            .value
            .into_iter()
            .filter(|t| t.is_archived != Some(true))
            .filter_map(|t| {
                Some((
                    t.id.filter(|i| !i.is_empty())?,
                    t.display_name.unwrap_or_default(),
                ))
            })
            .collect();
        if teams.is_empty() {
            return Ok(Page::single(vec![]));
        }

        let results: Vec<_> = futures::stream::iter(teams.iter().cloned())
            .map(|(team_id, team_name)| async move {
                let mut url = self.client.url(&["teams", &team_id, "channels"]);
                url.query_pairs_mut()
                    .append_pair("$select", "id,displayName");
                let res: Result<Collection<GChannel>, _> = self.client.get(url).await;
                (team_id, team_name, res)
            })
            .buffer_unordered(4)
            .collect()
            .await;

        let mut items = Vec::new();
        let mut last_err = None;
        let mut ok = 0;
        for (team_id, team_name, res) in results {
            match res {
                Ok(channels) => {
                    ok += 1;
                    for ch in channels.value {
                        let Some(id) = ch.id.filter(|i| !i.is_empty()) else {
                            continue;
                        };
                        items.push(RemoteConversation {
                            remote_id: channel_remote_id(&team_id, &id),
                            kind: comms_domain::ConversationKind::Channel,
                            title: ch.display_name.unwrap_or_else(|| "Channel".into()),
                            group: Some(team_name.clone()),
                            last_activity: None,
                            last_read: None,
                            preview: None,
                            last_message_from_me: false,
                        });
                    }
                }
                Err(e) => {
                    debug!(error = %e, "could not list channels of one team");
                    last_err = Some(e);
                }
            }
        }
        match (ok, last_err) {
            (0, Some(e)) => Err(e),
            _ => Ok(Page::single(items)),
        }
    }

    fn messages_url(&self, target: &Target, with_replies: bool) -> url::Url {
        let mut url = match target {
            Target::Chat(id) => self.client.url(&["chats", id, "messages"]),
            Target::Channel { team, channel } => self
                .client
                .url(&["teams", team, "channels", channel, "messages"]),
        };
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("$top", "50");
            if with_replies && matches!(target, Target::Channel { .. }) {
                q.append_pair("$expand", "replies");
            }
        }
        url
    }
}

#[async_trait]
impl MessagingProvider for MsGraphProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::new(PROVIDER_KIND)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            direct_messages: true,
            group_conversations: true,
            channels: self.channels,
            attachments: false,
            threads: false,
            // Delegated clients cannot receive change notifications without a
            // public webhook, so the core polls.
            realtime_events: false,
            ..Capabilities::default()
        }
    }

    async fn current_user(&self) -> Result<Participant, ProviderError> {
        self.me().await.cloned()
    }

    async fn list_conversations(
        &self,
        class: ConversationClass,
        page: PageRequest,
    ) -> Result<Page<RemoteConversation>, ProviderError> {
        match class {
            ConversationClass::Chats => self.list_chats(page).await,
            ConversationClass::Channels => self.list_channels().await,
        }
    }

    async fn get_messages(
        &self,
        conversation: &RemoteId,
        page: PageRequest,
    ) -> Result<Page<RemoteMessage>, ProviderError> {
        let target = parse_target(conversation)
            .ok_or_else(|| ProviderError::BadRequest("malformed conversation id".into()))?;
        let first = page.cursor.is_none();
        let url = match &page.cursor {
            Some(c) => self.client.validate_link(c.as_str())?,
            None => self.messages_url(&target, true),
        };
        let result: Result<Collection<GMessage>, _> = self.client.get(url).await;
        let list = match result {
            Ok(l) => l,
            Err(ProviderError::BadRequest(msg))
                if first && matches!(target, Target::Channel { .. }) =>
            {
                warn!("channel query with $expand=replies rejected ({msg}); retrying without");
                self.client.get(self.messages_url(&target, false)).await?
            }
            Err(e) => return Err(e),
        };
        Ok(Page {
            items: map_messages(&list.value),
            next: list.next_link.map(PageCursor::new),
        })
    }

    async fn send_message(
        &self,
        conversation: &RemoteId,
        message: OutgoingMessage,
    ) -> Result<RemoteMessage, ProviderError> {
        let target = parse_target(conversation)
            .ok_or_else(|| ProviderError::BadRequest("malformed conversation id".into()))?;
        let url = match &target {
            Target::Chat(id) => self.client.url(&["chats", id, "messages"]),
            Target::Channel { team, channel } => self
                .client
                .url(&["teams", team, "channels", channel, "messages"]),
        };
        let html = escape_html(&message.text);
        let body = SendBody {
            body: SendContent {
                content_type: "html",
                content: &html,
            },
        };
        let created: GMessage = self.client.post(url, &body).await?;
        map_message(&created).ok_or_else(|| {
            ProviderError::InvalidResponse("send succeeded but the response was unusable".into())
        })
    }
}

#[cfg(test)]
mod tests;
