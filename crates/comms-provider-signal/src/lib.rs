//! Signal backend. Talks to a `signal-cli` child process over JSON-RPC, so no
//! Signal protocol code (and no AGPL dependency) is linked into Comms.
//!
//! Text only. Comms is a *linked device* of your Signal account (like Signal
//! Desktop). Signal keeps no history for linked devices, so conversations start
//! empty and fill with messages received while Comms is running.

mod inbox;
mod process;
mod rpc;
#[cfg(test)]
mod tests;
mod wire;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use comms_domain::{PageCursor, Participant, ProviderKind, RemoteId};
use comms_provider_api::{
    Capabilities, ConversationClass, MessagingProvider, OutgoingMessage, Page, PageRequest,
    ProviderError, RemoteConversation, RemoteMessage,
};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncWrite};

use inbox::{DM_PREFIX, GROUP_PREFIX, Inbox, Stored};
use rpc::{RpcClient, RpcError};
use wire::{ContactEntry, DataMessage, Envelope, GroupEntry, GroupInfo, SendResult};

pub use process::link;

/// Could not start or reach `signal-cli`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ConnectError(pub String);

#[derive(Debug, Clone)]
pub struct SignalConfig {
    /// The linked account's phone number in international format, e.g. `+36201234567`.
    pub account: String,
    /// Path or name of the `signal-cli` executable.
    pub cli_path: String,
    /// Where received messages are kept between runs (`None` = memory only).
    pub inbox_file: Option<PathBuf>,
}

const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const DIRECTORY_REFRESH: Duration = Duration::from_secs(300);

pub struct SignalProvider {
    client: Arc<RpcClient>,
    inbox: Arc<Mutex<Inbox>>,
    last_refresh: Mutex<Instant>,
    send_timeout: Duration,
    // Kept so the child is killed when the provider is dropped.
    _child: Option<tokio::process::Child>,
}

impl SignalProvider {
    /// Start `signal-cli` for the configured account.
    pub async fn connect(cfg: SignalConfig) -> Result<Self, ConnectError> {
        let spawned = process::spawn(&cfg.cli_path, &cfg.account)?;
        let mut provider =
            Self::with_transport(spawned.stdout, spawned.stdin, &cfg.account, cfg.inbox_file)
                .await?;
        provider._child = Some(spawned.child);
        Ok(provider)
    }

    /// Use an existing JSON-RPC transport (tests, or a daemon you started yourself).
    pub async fn with_transport<R, W>(
        reader: R,
        writer: W,
        account: &str,
        inbox_file: Option<PathBuf>,
    ) -> Result<Self, ConnectError>
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let (client, mut notifications) = RpcClient::new(reader, writer);
        let client = Arc::new(client);
        let inbox = Arc::new(Mutex::new(Inbox::new(account, inbox_file)));

        let sink = inbox.clone();
        tokio::spawn(async move {
            while let Some(n) = notifications.recv().await {
                if n.method == "receive"
                    && let Some(env) = Envelope::from_params(&n.params)
                {
                    handle_envelope(&mut sink.lock().expect("inbox lock"), env);
                }
            }
        });

        // Doubles as the "is the account usable" check: signal-cli exits at
        // once when the account is not linked.
        refresh_directory(&client, &inbox, true)
            .await
            .map_err(|e| {
                ConnectError(format!(
                    "signal-cli is not responding ({e}). Is {account} linked? \
                     Run `comms signal-link` first."
                ))
            })?;
        Ok(Self {
            client,
            inbox,
            last_refresh: Mutex::new(Instant::now()),
            send_timeout: Duration::from_secs(30),
            _child: None,
        })
    }

    pub fn with_send_timeout(mut self, timeout: Duration) -> Self {
        self.send_timeout = timeout;
        self
    }

    fn ensure_running(&self) -> Result<(), ProviderError> {
        if self.client.is_closed() {
            return Err(ProviderError::Network {
                may_have_reached_server: false,
                message: "signal-cli has stopped; restart Comms".into(),
            });
        }
        Ok(())
    }

    async fn refresh_if_stale(&self) {
        let due = {
            let mut last = self.last_refresh.lock().expect("refresh lock");
            let due = last.elapsed() >= DIRECTORY_REFRESH;
            if due {
                *last = Instant::now();
            }
            due
        };
        if due {
            let _ = refresh_directory(&self.client, &self.inbox, false).await;
        }
    }
}

/// Load groups and contact names. Groups are required; contacts are best effort.
async fn refresh_directory(
    client: &RpcClient,
    inbox: &Mutex<Inbox>,
    required: bool,
) -> Result<(), RpcError> {
    let groups = client.call("listGroups", json!({}), CALL_TIMEOUT).await;
    let contacts = client.call("listContacts", json!({}), CALL_TIMEOUT).await;
    let mut inbox = inbox.lock().expect("inbox lock");
    match groups {
        Ok(v) => {
            for g in serde_json::from_value::<Vec<GroupEntry>>(v).unwrap_or_default() {
                if g.is_member == Some(false) {
                    continue;
                }
                if let Some(id) = g.id {
                    inbox.set_group(&id, g.name.as_deref().unwrap_or(""));
                }
            }
        }
        Err(e) if required => return Err(e),
        Err(_) => {}
    }
    if let Ok(v) = contacts {
        for c in serde_json::from_value::<Vec<ContactEntry>>(v).unwrap_or_default() {
            if let Some(name) = c.display_name() {
                for address in [&c.number, &c.uuid].into_iter().flatten() {
                    inbox.set_contact(address, &name);
                }
            }
        }
    }
    Ok(())
}

fn conversation_key(group: Option<&GroupInfo>, peer: Option<String>) -> Option<String> {
    match group.and_then(|g| g.group_id.as_deref()) {
        Some(id) if !id.is_empty() => Some(format!("{GROUP_PREFIX}{id}")),
        _ => peer.map(|p| format!("{DM_PREFIX}{p}")),
    }
}

/// The text to show for a message: its text plus a marker per attachment
/// (attachments themselves are not downloaded yet).
fn message_text(text: Option<&str>, attachments: &[wire::AttachmentInfo]) -> Option<String> {
    let mut out = text
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("")
        .to_owned();
    for a in attachments {
        let label = a
            .filename
            .as_deref()
            .or(a.content_type.as_deref())
            .unwrap_or("file");
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("[attachment: {label}]"));
    }
    (!out.is_empty()).then_some(out)
}

fn handle_envelope(inbox: &mut Inbox, mut env: Envelope) {
    let me = inbox.me().to_owned();
    if let Some(DataMessage {
        timestamp,
        message,
        group_info,
        attachments,
    }) = env.data_message.take()
    {
        let Some(sender) = env.sender() else { return };
        if let Some(name) = env.source_name.as_deref() {
            inbox.learn_name(&sender, name);
        }
        learn_group(inbox, group_info.as_ref());
        let peer = Some(sender.clone());
        if let (Some(key), Some(text)) = (
            conversation_key(group_info.as_ref(), peer),
            message_text(message.as_deref(), &attachments),
        ) {
            let ts = timestamp
                .or(env.timestamp)
                .unwrap_or_else(|| Utc::now().timestamp_millis());
            inbox.add(Stored {
                conversation: key,
                id: format!("{ts}:{sender}"),
                mine: sender == me,
                sender,
                ts_ms: ts,
                text,
            });
        }
        return;
    }
    let Some(sync) = env.sync_message.take() else {
        return;
    };
    if let Some(sent) = sync.sent_message {
        learn_group(inbox, sent.group_info.as_ref());
        if let (Some(key), Some(text)) = (
            conversation_key(sent.group_info.as_ref(), sent.destination()),
            message_text(sent.message.as_deref(), &sent.attachments),
        ) {
            let ts = sent
                .timestamp
                .or(env.timestamp)
                .unwrap_or_else(|| Utc::now().timestamp_millis());
            inbox.add(Stored {
                conversation: key,
                id: format!("{ts}:{me}"),
                sender: me.clone(),
                ts_ms: ts,
                text,
                mine: true,
            });
        }
    }
    for read in sync.read_messages {
        if let (Some(peer), Some(ts)) = (read.sender(), read.timestamp) {
            inbox.mark_read(&format!("{DM_PREFIX}{peer}"), ts);
        }
    }
}

fn learn_group(inbox: &mut Inbox, group: Option<&GroupInfo>) {
    if let Some(g) = group
        && let (Some(id), Some(name)) = (g.group_id.as_deref(), g.group_name.as_deref())
    {
        inbox.learn_group(id, name);
    }
}

fn map_rpc_error(e: RpcError) -> ProviderError {
    match e {
        RpcError::NotSent(message) => ProviderError::Network {
            may_have_reached_server: false,
            message,
        },
        RpcError::NoAnswer(message) => ProviderError::Network {
            may_have_reached_server: true,
            message,
        },
        // signal-cli answers with an error only when it did not deliver anything.
        RpcError::Remote { message, .. } => ProviderError::BadRequest(message),
    }
}

#[async_trait]
impl MessagingProvider for SignalProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::new("signal")
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            direct_messages: true,
            group_conversations: true,
            ..Capabilities::default()
        }
    }

    async fn current_user(&self) -> Result<Participant, ProviderError> {
        let inbox = self.inbox.lock().expect("inbox lock");
        Ok(inbox.participant(inbox.me()))
    }

    async fn list_conversations(
        &self,
        class: ConversationClass,
        _page: PageRequest,
    ) -> Result<Page<RemoteConversation>, ProviderError> {
        if class == ConversationClass::Channels {
            return Ok(Page::single(vec![]));
        }
        self.ensure_running()?;
        self.refresh_if_stale().await;
        Ok(Page::single(
            self.inbox.lock().expect("inbox lock").conversations(),
        ))
    }

    async fn get_messages(
        &self,
        conversation: &RemoteId,
        page: PageRequest,
    ) -> Result<Page<RemoteMessage>, ProviderError> {
        self.ensure_running()?;
        let (items, next) = self.inbox.lock().expect("inbox lock").page(
            conversation.as_str(),
            page.cursor.as_ref().map(PageCursor::as_str),
            page.limit as usize,
        );
        Ok(Page {
            items,
            next: next.map(PageCursor::new),
        })
    }

    async fn get_conversation(
        &self,
        conversation: &RemoteId,
    ) -> Result<RemoteConversation, ProviderError> {
        self.inbox
            .lock()
            .expect("inbox lock")
            .conversation(conversation.as_str())
            .ok_or(ProviderError::NotFound)
    }

    async fn send_message(
        &self,
        conversation: &RemoteId,
        message: OutgoingMessage,
    ) -> Result<RemoteMessage, ProviderError> {
        let key = conversation.as_str();
        let params = if let Some(group) = key.strip_prefix(GROUP_PREFIX) {
            json!({"groupId": group, "message": message.text})
        } else if let Some(peer) = key.strip_prefix(DM_PREFIX) {
            json!({"recipient": [peer], "message": message.text})
        } else {
            return Err(ProviderError::BadRequest(format!(
                "not a Signal conversation: {key}"
            )));
        };
        let answer = self
            .client
            .call("send", params, self.send_timeout)
            .await
            .map_err(map_rpc_error)?;
        let result: SendResult = serde_json::from_value(answer)
            .map_err(|_| ProviderError::InvalidResponse("unreadable send result".into()))?;
        check_delivery(&result)?;
        let ts = result
            .timestamp
            .ok_or_else(|| ProviderError::InvalidResponse("send result has no timestamp".into()))?;

        let mut inbox = self.inbox.lock().expect("inbox lock");
        let me = inbox.me().to_owned();
        let stored = Stored {
            conversation: key.to_owned(),
            id: format!("{ts}:{me}"),
            sender: me,
            ts_ms: ts,
            text: message.text,
            mine: true,
        };
        inbox.add(stored.clone());
        Ok(inbox.remote_message_for(&stored))
    }
}

/// Look at the per-recipient outcomes of a `send`. Success for at least one
/// recipient counts as sent; total failure is classified by its kind.
fn check_delivery(result: &SendResult) -> Result<(), ProviderError> {
    if result.results.is_empty()
        || result
            .results
            .iter()
            .any(|r| r.kind.as_deref() == Some("SUCCESS"))
    {
        return Ok(());
    }
    let kind = result.results[0].kind.as_deref().unwrap_or("UNKNOWN");
    Err(match kind {
        "RATE_LIMIT_FAILURE" => ProviderError::RateLimited {
            retry_after: Duration::from_secs(60),
        },
        // The network failed somewhere: the message may or may not have gone out.
        "NETWORK_FAILURE" => ProviderError::Network {
            may_have_reached_server: true,
            message: "Signal could not confirm delivery".into(),
        },
        "UNREGISTERED_FAILURE" => {
            ProviderError::BadRequest("that number is not registered with Signal".into())
        }
        other => ProviderError::BadRequest(format!("Signal refused the message ({other})")),
    })
}
