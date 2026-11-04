-- Local cache and source of truth for the UI.

CREATE TABLE accounts (
    id           TEXT PRIMARY KEY,
    provider     TEXT NOT NULL,
    remote_id    TEXT NOT NULL,
    display_name TEXT NOT NULL,
    UNIQUE (provider, remote_id)
);

CREATE TABLE conversations (
    id               TEXT PRIMARY KEY,
    account_id       TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    remote_id        TEXT NOT NULL,
    kind             TEXT NOT NULL,
    title            TEXT NOT NULL,
    group_title      TEXT,
    last_activity    INTEGER,           -- unix millis
    last_read_remote INTEGER,           -- unix millis, as reported by the provider
    local_read_at    INTEGER,           -- unix millis, set when opened locally
    last_from_me     INTEGER NOT NULL DEFAULT 0,
    preview          TEXT,
    older_cursor     TEXT,              -- provider cursor for loading older messages
    older_done       INTEGER NOT NULL DEFAULT 0,
    UNIQUE (account_id, remote_id)
);

CREATE TABLE messages (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    remote_id       TEXT,               -- NULL until the provider has accepted it
    sender_id       TEXT NOT NULL,
    sender_name     TEXT NOT NULL,
    is_mine         INTEGER NOT NULL,
    created_at      INTEGER NOT NULL,   -- unix millis
    edited_at       INTEGER,
    deleted_at      INTEGER,
    reply_to        TEXT,
    send_state      TEXT NOT NULL DEFAULT 'sent',
    content_json    TEXT NOT NULL,
    attachments_json TEXT NOT NULL DEFAULT '[]',
    plain_text      TEXT NOT NULL
);

-- Duplicate prevention: one row per provider message id per conversation.
CREATE UNIQUE INDEX messages_remote_uq ON messages (conversation_id, remote_id)
    WHERE remote_id IS NOT NULL;
CREATE INDEX messages_by_time ON messages (conversation_id, created_at);
CREATE INDEX conversations_by_activity ON conversations (account_id, last_activity);
