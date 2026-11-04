//! SQLite-backed local store. The UI reads exclusively from here.

use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use comms_domain::{
    Account, AccountId, Attachment, ContentBlock, Conversation, ConversationId, ConversationKind,
    Message, MessageId, PageCursor, Participant, ProviderKind, RemoteId, SendState,
    blocks_to_plain_text,
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("corrupt data: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Minimal input needed to upsert a message received from a provider.
pub struct IncomingMessage<'a> {
    pub remote_id: &'a RemoteId,
    pub sender: &'a Participant,
    pub timestamp: DateTime<Utc>,
    pub content: &'a [ContentBlock],
    pub attachments: &'a [Attachment],
    pub edited_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub reply_to: Option<&'a RemoteId>,
}

/// Conversation data reported by a provider, ready to be stored.
pub struct IncomingConversation<'a> {
    pub remote_id: &'a RemoteId,
    pub kind: ConversationKind,
    pub title: &'a str,
    pub group: Option<&'a str>,
    pub last_activity: Option<DateTime<Utc>>,
    pub last_read: Option<DateTime<Utc>>,
    pub preview: Option<&'a str>,
    pub last_message_from_me: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct UpsertStats {
    pub inserted: usize,
    pub updated: usize,
    /// Local pending/uncertain messages matched to their server copy.
    pub adopted: usize,
}

impl UpsertStats {
    pub fn changed(&self) -> bool {
        self.inserted + self.updated + self.adopted > 0
    }
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

fn ms(t: DateTime<Utc>) -> i64 {
    t.timestamp_millis()
}

fn from_ms(v: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(v).single().unwrap_or_default()
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Store {
    /// Open (creating if needed) the database at `path`. On Unix the file and
    /// its directory are restricted to the current user.
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(path)?;
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    /// Single-connection in-memory database (tests, `--demo`).
    pub async fn open_in_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    /// Delete everything cached for every account (logout / cache wipe).
    pub async fn wipe(&self) -> Result<()> {
        sqlx::query("DELETE FROM accounts")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---- accounts ----

    pub async fn ensure_account(
        &self,
        provider: &ProviderKind,
        me: &Participant,
    ) -> Result<Account> {
        let existing =
            sqlx::query("SELECT id FROM accounts WHERE provider = ?1 AND remote_id = ?2")
                .bind(provider.as_str())
                .bind(me.remote_id.as_str())
                .fetch_optional(&self.pool)
                .await?;
        let id = match existing {
            Some(row) => {
                let id: String = row.try_get("id")?;
                sqlx::query("UPDATE accounts SET display_name = ?1 WHERE id = ?2")
                    .bind(&me.display_name)
                    .bind(&id)
                    .execute(&self.pool)
                    .await?;
                id.parse()
                    .map_err(|_| StoreError::Corrupt(format!("account id {id}")))?
            }
            None => {
                let id = AccountId::new();
                sqlx::query(
                    "INSERT INTO accounts (id, provider, remote_id, display_name) VALUES (?1,?2,?3,?4)",
                )
                .bind(id.to_string())
                .bind(provider.as_str())
                .bind(me.remote_id.as_str())
                .bind(&me.display_name)
                .execute(&self.pool)
                .await?;
                id
            }
        };
        Ok(Account {
            id,
            provider: provider.clone(),
            remote_id: me.remote_id.clone(),
            display_name: me.display_name.clone(),
        })
    }

    /// The account previously stored for `provider`, if any. Lets the app
    /// start from the cache without contacting the provider.
    pub async fn first_account(&self, provider: &ProviderKind) -> Result<Option<Account>> {
        let row = sqlx::query(
            "SELECT id, remote_id, display_name FROM accounts WHERE provider = ?1 ORDER BY rowid LIMIT 1",
        )
        .bind(provider.as_str())
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        let id: String = row.try_get("id")?;
        Ok(Some(Account {
            id: id
                .parse()
                .map_err(|_| StoreError::Corrupt(format!("account id {id}")))?,
            provider: provider.clone(),
            remote_id: RemoteId::new(row.try_get::<String, _>("remote_id")?),
            display_name: row.try_get("display_name")?,
        }))
    }

    pub async fn get_message(&self, id: MessageId) -> Result<Option<Message>> {
        let row = sqlx::query("SELECT * FROM messages WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(Self::message_from_row).transpose()
    }

    // ---- conversations ----

    /// Insert or update conversations. When `prune_missing_of_kind` is given,
    /// stored conversations of that kind that are absent from `items` are
    /// removed (only pass it after a complete, successful listing).
    pub async fn upsert_conversations(
        &self,
        account: AccountId,
        items: &[IncomingConversation<'_>],
        prune_missing_of_kind: Option<ConversationKind>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for c in items {
            sqlx::query(
                "INSERT INTO conversations
                    (id, account_id, remote_id, kind, title, group_title, last_activity,
                     last_read_remote, last_from_me, preview)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                 ON CONFLICT (account_id, remote_id) DO UPDATE SET
                    kind = excluded.kind,
                    title = excluded.title,
                    group_title = excluded.group_title,
                    last_activity = COALESCE(excluded.last_activity, last_activity),
                    last_read_remote = COALESCE(excluded.last_read_remote, last_read_remote),
                    last_from_me = CASE WHEN excluded.last_activity IS NULL
                                        THEN last_from_me ELSE excluded.last_from_me END,
                    preview = COALESCE(excluded.preview, preview)",
            )
            .bind(ConversationId::new().to_string())
            .bind(account.to_string())
            .bind(c.remote_id.as_str())
            .bind(c.kind.as_str())
            .bind(c.title)
            .bind(c.group)
            .bind(c.last_activity.map(ms))
            .bind(c.last_read.map(ms))
            .bind(c.last_message_from_me)
            .bind(c.preview)
            .execute(&mut *tx)
            .await?;
        }
        if let Some(kind) = prune_missing_of_kind {
            let keep: Vec<&str> = items.iter().map(|c| c.remote_id.as_str()).collect();
            let rows = sqlx::query(
                "SELECT id, remote_id FROM conversations WHERE account_id = ?1 AND kind = ?2",
            )
            .bind(account.to_string())
            .bind(kind.as_str())
            .fetch_all(&mut *tx)
            .await?;
            for row in rows {
                let remote: String = row.try_get("remote_id")?;
                if !keep.contains(&remote.as_str()) {
                    let id: String = row.try_get("id")?;
                    sqlx::query("DELETE FROM conversations WHERE id = ?1")
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                }
            }
        }
        tx.commit().await?;
        Ok(())
    }

    fn conversation_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Conversation> {
        let corrupt = |what: &str| StoreError::Corrupt(what.to_owned());
        let kind = ConversationKind::parse(&row.try_get::<String, _>("kind")?)
            .map_err(|e| StoreError::Corrupt(e.to_string()))?;
        let last_activity: Option<i64> = row.try_get("last_activity")?;
        let last_read_remote: Option<i64> = row.try_get("last_read_remote")?;
        let local_read_at: Option<i64> = row.try_get("local_read_at")?;
        let last_from_me: bool = row.try_get("last_from_me")?;
        let read_up_to = last_read_remote.into_iter().chain(local_read_at).max();
        let unread = kind != ConversationKind::Channel
            && !last_from_me
            && match (last_activity, read_up_to) {
                (Some(a), Some(r)) => a > r,
                (Some(_), None) => true,
                _ => false,
            };
        Ok(Conversation {
            id: row
                .try_get::<String, _>("id")?
                .parse()
                .map_err(|_| corrupt("conversation id"))?,
            account_id: row
                .try_get::<String, _>("account_id")?
                .parse()
                .map_err(|_| corrupt("account id"))?,
            remote_id: RemoteId::new(row.try_get::<String, _>("remote_id")?),
            kind,
            title: row.try_get("title")?,
            group: row.try_get("group_title")?,
            last_activity: last_activity.map(from_ms),
            preview: row.try_get("preview")?,
            unread,
        })
    }

    /// Chats by most recent activity, then channels grouped by team.
    pub async fn list_conversations(&self, account: AccountId) -> Result<Vec<Conversation>> {
        let rows = sqlx::query("SELECT * FROM conversations WHERE account_id = ?1")
            .bind(account.to_string())
            .fetch_all(&self.pool)
            .await?;
        let mut convs = rows
            .iter()
            .map(Self::conversation_from_row)
            .collect::<Result<Vec<_>>>()?;
        convs.sort_by(|a, b| {
            let a_chan = a.kind == ConversationKind::Channel;
            let b_chan = b.kind == ConversationKind::Channel;
            a_chan.cmp(&b_chan).then_with(|| {
                if a_chan {
                    a.group
                        .as_deref()
                        .map(str::to_lowercase)
                        .cmp(&b.group.as_deref().map(str::to_lowercase))
                        .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
                } else {
                    b.last_activity.cmp(&a.last_activity)
                }
            })
        });
        Ok(convs)
    }

    pub async fn get_conversation(&self, id: ConversationId) -> Result<Option<Conversation>> {
        let row = sqlx::query("SELECT * FROM conversations WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(Self::conversation_from_row).transpose()
    }

    pub async fn mark_read_locally(&self, id: ConversationId, at: DateTime<Utc>) -> Result<()> {
        sqlx::query("UPDATE conversations SET local_read_at = ?1 WHERE id = ?2")
            .bind(ms(at))
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Refresh activity/preview from the newest message of a conversation
    /// (used for channels, whose listing carries no activity information).
    pub async fn touch_conversation(
        &self,
        id: ConversationId,
        last_activity: DateTime<Utc>,
        preview: &str,
        from_me: bool,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE conversations SET last_activity = ?1, preview = ?2, last_from_me = ?3
             WHERE id = ?4 AND (last_activity IS NULL OR last_activity <= ?1)",
        )
        .bind(ms(last_activity))
        .bind(preview)
        .bind(from_me)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// `(cursor for older messages, whether the beginning has been reached)`.
    pub async fn backfill_state(&self, id: ConversationId) -> Result<(Option<PageCursor>, bool)> {
        let row = sqlx::query("SELECT older_cursor, older_done FROM conversations WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => {
                let cursor: Option<String> = r.try_get("older_cursor")?;
                let done: bool = r.try_get("older_done")?;
                Ok((cursor.map(PageCursor::new), done))
            }
            None => Ok((None, false)),
        }
    }

    pub async fn set_backfill_state(
        &self,
        id: ConversationId,
        cursor: Option<&PageCursor>,
        done: bool,
    ) -> Result<()> {
        sqlx::query("UPDATE conversations SET older_cursor = ?1, older_done = ?2 WHERE id = ?3")
            .bind(cursor.map(PageCursor::as_str))
            .bind(done)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---- messages ----

    fn message_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Message> {
        let corrupt = |what: &str| StoreError::Corrupt(what.to_owned());
        let content: Vec<ContentBlock> =
            serde_json::from_str(&row.try_get::<String, _>("content_json")?)
                .map_err(|e| StoreError::Corrupt(format!("content: {e}")))?;
        let attachments: Vec<Attachment> =
            serde_json::from_str(&row.try_get::<String, _>("attachments_json")?)
                .map_err(|e| StoreError::Corrupt(format!("attachments: {e}")))?;
        Ok(Message {
            id: row
                .try_get::<String, _>("id")?
                .parse()
                .map_err(|_| corrupt("message id"))?,
            conversation_id: row
                .try_get::<String, _>("conversation_id")?
                .parse()
                .map_err(|_| corrupt("conversation id"))?,
            remote_id: row
                .try_get::<Option<String>, _>("remote_id")?
                .map(RemoteId::new),
            sender: Participant {
                remote_id: RemoteId::new(row.try_get::<String, _>("sender_id")?),
                display_name: row.try_get("sender_name")?,
            },
            is_mine: row.try_get("is_mine")?,
            timestamp: from_ms(row.try_get("created_at")?),
            content,
            attachments,
            edited_at: row.try_get::<Option<i64>, _>("edited_at")?.map(from_ms),
            deleted_at: row.try_get::<Option<i64>, _>("deleted_at")?.map(from_ms),
            reply_to: row
                .try_get::<Option<String>, _>("reply_to")?
                .map(RemoteId::new),
            send_state: SendState::parse(&row.try_get::<String, _>("send_state")?)
                .map_err(|e| StoreError::Corrupt(e.to_string()))?,
        })
    }

    /// The newest `limit` messages, returned oldest → newest.
    pub async fn recent_messages(&self, id: ConversationId, limit: u32) -> Result<Vec<Message>> {
        let rows = sqlx::query(
            "SELECT * FROM messages WHERE conversation_id = ?1
             ORDER BY created_at DESC, id DESC LIMIT ?2",
        )
        .bind(id.to_string())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        let mut msgs = rows
            .iter()
            .map(Self::message_from_row)
            .collect::<Result<Vec<_>>>()?;
        msgs.reverse();
        Ok(msgs)
    }

    pub async fn message_count(&self, id: ConversationId) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS n FROM messages WHERE conversation_id = ?1")
            .bind(id.to_string())
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("n")?)
    }

    /// Store messages received from the provider. Idempotent: replaying the
    /// same page never creates duplicates, and edits/deletions update the
    /// cached row. A local message that was left `pending`/`uncertain` is
    /// matched to its server copy instead of being duplicated.
    pub async fn upsert_remote_messages(
        &self,
        conversation: ConversationId,
        me: &RemoteId,
        msgs: &[IncomingMessage<'_>],
    ) -> Result<UpsertStats> {
        let mut stats = UpsertStats::default();
        let mut tx = self.pool.begin().await?;
        for m in msgs {
            let plain = blocks_to_plain_text(m.content);
            let content_json =
                serde_json::to_string(m.content).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            let attachments_json = serde_json::to_string(m.attachments)
                .map_err(|e| StoreError::Corrupt(e.to_string()))?;

            let existing = sqlx::query(
                "SELECT id, content_json, edited_at, deleted_at FROM messages
                 WHERE conversation_id = ?1 AND remote_id = ?2",
            )
            .bind(conversation.to_string())
            .bind(m.remote_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;

            if let Some(row) = existing {
                let id: String = row.try_get("id")?;
                let old_content: String = row.try_get("content_json")?;
                let old_edit: Option<i64> = row.try_get("edited_at")?;
                let old_del: Option<i64> = row.try_get("deleted_at")?;
                if old_content == content_json
                    && old_edit == m.edited_at.map(ms)
                    && old_del == m.deleted_at.map(ms)
                {
                    continue;
                }
                sqlx::query(
                    "UPDATE messages SET content_json = ?1, attachments_json = ?2, plain_text = ?3,
                        edited_at = ?4, deleted_at = ?5, sender_name = ?6 WHERE id = ?7",
                )
                .bind(&content_json)
                .bind(&attachments_json)
                .bind(&plain)
                .bind(m.edited_at.map(ms))
                .bind(m.deleted_at.map(ms))
                .bind(&m.sender.display_name)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                stats.updated += 1;
                continue;
            }

            let is_mine = &m.sender.remote_id == me;

            if is_mine {
                let candidates = sqlx::query(
                    "SELECT id, plain_text, created_at FROM messages
                     WHERE conversation_id = ?1 AND remote_id IS NULL AND is_mine = 1
                       AND send_state IN ('pending','uncertain')",
                )
                .bind(conversation.to_string())
                .fetch_all(&mut *tx)
                .await?;
                let wanted = norm(&plain);
                let adopt = candidates.iter().find_map(|r| {
                    let text: String = r.try_get("plain_text").ok()?;
                    let at: i64 = r.try_get("created_at").ok()?;
                    let close = (at - ms(m.timestamp)).abs() <= 10 * 60 * 1000;
                    (close && norm(&text) == wanted)
                        .then(|| r.try_get::<String, _>("id").ok())
                        .flatten()
                });
                if let Some(local_id) = adopt {
                    sqlx::query(
                        "UPDATE messages SET remote_id = ?1, send_state = 'sent', created_at = ?2,
                            content_json = ?3, plain_text = ?4, edited_at = ?5, deleted_at = ?6
                         WHERE id = ?7",
                    )
                    .bind(m.remote_id.as_str())
                    .bind(ms(m.timestamp))
                    .bind(&content_json)
                    .bind(&plain)
                    .bind(m.edited_at.map(ms))
                    .bind(m.deleted_at.map(ms))
                    .bind(local_id)
                    .execute(&mut *tx)
                    .await?;
                    stats.adopted += 1;
                    continue;
                }
            }

            sqlx::query(
                "INSERT INTO messages
                    (id, conversation_id, remote_id, sender_id, sender_name, is_mine, created_at,
                     edited_at, deleted_at, reply_to, send_state, content_json, attachments_json,
                     plain_text)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'sent',?11,?12,?13)",
            )
            .bind(MessageId::new().to_string())
            .bind(conversation.to_string())
            .bind(m.remote_id.as_str())
            .bind(m.sender.remote_id.as_str())
            .bind(&m.sender.display_name)
            .bind(is_mine)
            .bind(ms(m.timestamp))
            .bind(m.edited_at.map(ms))
            .bind(m.deleted_at.map(ms))
            .bind(m.reply_to.map(RemoteId::as_str))
            .bind(&content_json)
            .bind(&attachments_json)
            .bind(&plain)
            .execute(&mut *tx)
            .await?;
            stats.inserted += 1;
        }
        tx.commit().await?;
        Ok(stats)
    }

    /// Record an outgoing message locally before contacting the provider.
    pub async fn insert_pending(
        &self,
        conversation: ConversationId,
        me: &Participant,
        text: &str,
        at: DateTime<Utc>,
    ) -> Result<Message> {
        let content = vec![ContentBlock::paragraph(text)];
        let id = MessageId::new();
        sqlx::query(
            "INSERT INTO messages
                (id, conversation_id, remote_id, sender_id, sender_name, is_mine, created_at,
                 send_state, content_json, attachments_json, plain_text)
             VALUES (?1,?2,NULL,?3,?4,1,?5,'pending',?6,'[]',?7)",
        )
        .bind(id.to_string())
        .bind(conversation.to_string())
        .bind(me.remote_id.as_str())
        .bind(&me.display_name)
        .bind(ms(at))
        .bind(serde_json::to_string(&content).map_err(|e| StoreError::Corrupt(e.to_string()))?)
        .bind(text)
        .execute(&self.pool)
        .await?;
        Ok(Message {
            id,
            conversation_id: conversation,
            remote_id: None,
            sender: me.clone(),
            is_mine: true,
            timestamp: at,
            content,
            attachments: vec![],
            edited_at: None,
            deleted_at: None,
            reply_to: None,
            send_state: SendState::Pending,
        })
    }

    /// The provider confirmed the message. If a sync already adopted it
    /// (so `remote_id` exists on another row) the local duplicate is dropped.
    pub async fn mark_sent(
        &self,
        local: MessageId,
        remote_id: &RemoteId,
        server_time: DateTime<Utc>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT conversation_id FROM messages WHERE id = ?1")
            .bind(local.to_string())
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else { return Ok(()) };
        let conv: String = row.try_get("conversation_id")?;
        let taken = sqlx::query(
            "SELECT id FROM messages WHERE conversation_id = ?1 AND remote_id = ?2 AND id != ?3",
        )
        .bind(&conv)
        .bind(remote_id.as_str())
        .bind(local.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        if taken.is_some() {
            sqlx::query("DELETE FROM messages WHERE id = ?1")
                .bind(local.to_string())
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query(
                "UPDATE messages SET remote_id = ?1, send_state = 'sent', created_at = ?2 WHERE id = ?3",
            )
            .bind(remote_id.as_str())
            .bind(ms(server_time))
            .bind(local.to_string())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn set_send_state(&self, id: MessageId, state: SendState) -> Result<()> {
        sqlx::query("UPDATE messages SET send_state = ?1 WHERE id = ?2 AND remote_id IS NULL")
            .bind(state.as_str())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Messages left `pending` by a crash or quit may or may not have been
    /// delivered, so they become `uncertain` and are never resent blindly.
    pub async fn recover_interrupted_sends(&self) -> Result<u64> {
        let r = sqlx::query(
            "UPDATE messages SET send_state = 'uncertain'
             WHERE send_state = 'pending' AND remote_id IS NULL",
        )
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }
}

#[cfg(test)]
mod tests;
