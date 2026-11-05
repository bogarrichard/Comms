//! What the provider knows: conversations and messages received so far.
//!
//! Signal pushes messages and keeps no history for linked devices, while the
//! Comms core polls. So received messages are buffered here, served through
//! `get_messages`, and appended to a private file so they survive a restart
//! (the core only copies them into its own cache when it syncs).

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use comms_domain::{ContentBlock, ConversationKind, Inline, Participant, RemoteId};
use comms_provider_api::{RemoteConversation, RemoteMessage};
use serde::{Deserialize, Serialize};

pub const DM_PREFIX: &str = "dm:";
pub const GROUP_PREFIX: &str = "grp:";
/// Messages kept in memory per conversation.
const MAX_PER_CONVERSATION: usize = 5000;

/// One line of the persisted inbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub conversation: String,
    pub id: String,
    pub sender: String,
    pub ts_ms: i64,
    pub text: String,
    pub mine: bool,
}

#[derive(Default)]
struct Conv {
    messages: Vec<Stored>,
    /// Newest timestamp the user has read on any device (ms).
    read_up_to_ms: Option<i64>,
}

pub struct Inbox {
    me: String,
    convs: HashMap<String, Conv>,
    /// address (number or uuid) → display name
    names: HashMap<String, String>,
    groups: HashMap<String, String>,
    persist: Option<PathBuf>,
}

impl Inbox {
    pub fn new(me: impl Into<String>, persist: Option<PathBuf>) -> Self {
        let mut inbox = Self {
            me: me.into(),
            convs: HashMap::new(),
            names: HashMap::new(),
            groups: HashMap::new(),
            persist,
        };
        inbox.load();
        inbox
    }

    fn load(&mut self) {
        let Some(path) = self.persist.clone() else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        for line in text.lines() {
            // A torn last line (crash mid-write) is simply skipped.
            if let Ok(m) = serde_json::from_str::<Stored>(line) {
                self.insert(m, false);
            }
        }
    }

    pub fn me(&self) -> &str {
        &self.me
    }

    pub fn set_contact(&mut self, address: &str, name: &str) {
        if !address.is_empty() && !name.trim().is_empty() {
            self.names
                .insert(address.to_owned(), name.trim().to_owned());
        }
    }

    /// Remember a profile name only if no better (address book) name is known.
    pub fn learn_name(&mut self, address: &str, name: &str) {
        if !address.is_empty() && !name.trim().is_empty() {
            self.names
                .entry(address.to_owned())
                .or_insert_with(|| name.trim().to_owned());
        }
    }

    pub fn learn_group(&mut self, group_id: &str, name: &str) {
        if !name.trim().is_empty() {
            self.groups
                .entry(group_id.to_owned())
                .or_insert_with(|| name.trim().to_owned());
        }
    }

    pub fn set_group(&mut self, group_id: &str, name: &str) {
        self.groups
            .insert(group_id.to_owned(), name.trim().to_owned());
    }

    /// Record a received/sent message. Returns whether it was new.
    pub fn add(&mut self, m: Stored) -> bool {
        self.insert(m, true)
    }

    fn insert(&mut self, m: Stored, persist: bool) -> bool {
        let conv = self.convs.entry(m.conversation.clone()).or_default();
        if conv.messages.iter().any(|x| x.id == m.id) {
            return false;
        }
        let at = conv
            .messages
            .partition_point(|x| (x.ts_ms, &x.id) < (m.ts_ms, &m.id));
        if persist {
            self.append(&m);
        }
        let conv = self.convs.entry(m.conversation.clone()).or_default();
        conv.messages.insert(at, m);
        if conv.messages.len() > MAX_PER_CONVERSATION {
            conv.messages.remove(0);
        }
        true
    }

    fn append(&self, m: &Stored) {
        let Some(path) = &self.persist else {
            return;
        };
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut opts = std::fs::OpenOptions::new();
            opts.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(path)?;
            let mut line = serde_json::to_string(m).map_err(std::io::Error::other)?;
            line.push('\n');
            f.write_all(line.as_bytes())
        })();
        if let Err(e) = result {
            tracing::warn!("could not save a Signal message to disk: {e}");
        }
    }

    pub fn mark_read(&mut self, conversation: &str, up_to_ms: i64) {
        let conv = self.convs.entry(conversation.to_owned()).or_default();
        conv.read_up_to_ms = conv.read_up_to_ms.max(Some(up_to_ms));
    }

    fn name_of(&self, address: &str) -> String {
        self.names
            .get(address)
            .cloned()
            .unwrap_or_else(|| address.to_owned())
    }

    pub fn participant(&self, address: &str) -> Participant {
        if address == self.me {
            return Participant {
                remote_id: RemoteId::new(address),
                display_name: "You".to_owned(),
            };
        }
        Participant {
            remote_id: RemoteId::new(address),
            display_name: self.name_of(address),
        }
    }

    pub fn title(&self, conversation: &str) -> String {
        if let Some(id) = conversation.strip_prefix(GROUP_PREFIX) {
            return self
                .groups
                .get(id)
                .filter(|n| !n.is_empty())
                .cloned()
                .unwrap_or_else(|| "Unnamed group".to_owned());
        }
        self.name_of(conversation.strip_prefix(DM_PREFIX).unwrap_or(conversation))
    }

    pub fn remote_message_for(&self, m: &Stored) -> RemoteMessage {
        RemoteMessage {
            remote_id: RemoteId::new(m.id.clone()),
            sender: self.participant(&m.sender),
            timestamp: to_utc(m.ts_ms),
            content: text_to_blocks(&m.text),
            attachments: vec![],
            edited_at: None,
            deleted_at: None,
            reply_to: None,
            is_system: false,
        }
    }

    fn summarize(&self, key: &str, conv: &Conv) -> RemoteConversation {
        let last = conv.messages.last();
        let last_activity = last.map(|m| to_utc(m.ts_ms));
        let from_me = last.is_some_and(|m| m.mine);
        // Your own newest message means you have read everything before it.
        let read_ms = conv
            .read_up_to_ms
            .max(last.filter(|m| m.mine).map(|m| m.ts_ms));
        RemoteConversation {
            remote_id: RemoteId::new(key),
            kind: if key.starts_with(GROUP_PREFIX) {
                ConversationKind::Group
            } else {
                ConversationKind::Direct
            },
            title: self.title(key),
            group: None,
            last_activity,
            last_read: read_ms.map(to_utc),
            preview: last.map(|m| m.text.lines().next().unwrap_or("").to_owned()),
            last_message_from_me: from_me,
        }
    }

    /// Every conversation with messages, plus every known group.
    pub fn conversations(&self) -> Vec<RemoteConversation> {
        let mut out: Vec<RemoteConversation> = self
            .convs
            .iter()
            .filter(|(_, c)| !c.messages.is_empty())
            .map(|(k, c)| self.summarize(k, c))
            .collect();
        for id in self.groups.keys() {
            let key = format!("{GROUP_PREFIX}{id}");
            if !self.convs.get(&key).is_some_and(|c| !c.messages.is_empty()) {
                out.push(self.summarize(&key, &Conv::default()));
            }
        }
        out.sort_by(|a, b| {
            b.last_activity
                .cmp(&a.last_activity)
                .then_with(|| a.title.cmp(&b.title))
        });
        out
    }

    pub fn conversation(&self, key: &str) -> Option<RemoteConversation> {
        if let Some(c) = self.convs.get(key) {
            return Some(self.summarize(key, c));
        }
        let id = key.strip_prefix(GROUP_PREFIX)?;
        self.groups
            .contains_key(id)
            .then(|| self.summarize(key, &Conv::default()))
    }

    /// Up to `limit` messages older than `before` (`"<ts>|<id>"`), newest first,
    /// and a cursor for the next page when more remain.
    pub fn page(
        &self,
        key: &str,
        before: Option<&str>,
        limit: usize,
    ) -> (Vec<RemoteMessage>, Option<String>) {
        let Some(conv) = self.convs.get(key) else {
            return (vec![], None);
        };
        let end = match before.and_then(parse_cursor) {
            Some((ts, id)) => conv
                .messages
                .partition_point(|x| (x.ts_ms, x.id.as_str()) < (ts, id.as_str())),
            None => conv.messages.len(),
        };
        let start = end.saturating_sub(limit.max(1));
        let items: Vec<RemoteMessage> = conv.messages[start..end]
            .iter()
            .rev()
            .map(|m| self.remote_message_for(m))
            .collect();
        let next = (start > 0).then(|| {
            let oldest = &conv.messages[start];
            format!("{}|{}", oldest.ts_ms, oldest.id)
        });
        (items, next)
    }
}

fn parse_cursor(s: &str) -> Option<(i64, String)> {
    let (ts, id) = s.split_once('|')?;
    Some((ts.parse().ok()?, id.to_owned()))
}

pub fn to_utc(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or_default()
}

/// Plain text → one paragraph with hard breaks. Signal text carries no markup.
pub fn text_to_blocks(text: &str) -> Vec<ContentBlock> {
    let mut inlines = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            inlines.push(Inline::Break);
        }
        inlines.push(Inline::text(line));
    }
    vec![ContentBlock::Paragraph { inlines }]
}
