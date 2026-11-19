//! Schema migrations. The applied version lives in `PRAGMA user_version`; each entry of
//! [`MIGRATIONS`] moves the schema one version forward and is applied in its own transaction.
//! Never edit a shipped migration: append a new one.

use rusqlite::Connection;

use crate::{Result, StoreError};

/// Version 1: the append-only revision log, current-state tables and the FTS5 index.
const V1: &str = r#"
-- Every message revision ever observed. Immutable: rows are only ever inserted.
-- `edit_ts` is '' for the original post, `edited.ts` for an edit, and the deletion event ts for
-- a tombstone (which carries the prior content with `deleted = 1`).
CREATE TABLE message_revision (
    workspace_id TEXT NOT NULL,
    channel_id   TEXT NOT NULL,
    ts           TEXT NOT NULL,
    edit_ts      TEXT NOT NULL,
    user_id      TEXT,
    thread_ts    TEXT,
    subtype      TEXT,
    text         TEXT NOT NULL,
    deleted      INTEGER NOT NULL DEFAULT 0,
    json         TEXT NOT NULL,
    created_at   INTEGER,
    recorded_at  INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, channel_id, ts, edit_ts)
) WITHOUT ROWID;

CREATE TRIGGER message_revision_no_update BEFORE UPDATE ON message_revision
BEGIN SELECT RAISE(ABORT, 'message_revision is append-only'); END;
CREATE TRIGGER message_revision_no_delete BEFORE DELETE ON message_revision
BEGIN SELECT RAISE(ABORT, 'message_revision is append-only'); END;

-- Latest revision per message: what the UI renders. Deleted messages stay (deleted = 1).
CREATE TABLE message_current (
    id           INTEGER PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    channel_id   TEXT NOT NULL,
    ts           TEXT NOT NULL,
    ts_key       INTEGER NOT NULL,
    edit_ts      TEXT NOT NULL,
    user_id      TEXT,
    thread_ts    TEXT,
    subtype      TEXT,
    text         TEXT NOT NULL,
    file_names   TEXT NOT NULL,
    deleted      INTEGER NOT NULL,
    has_link     INTEGER NOT NULL,
    has_file     INTEGER NOT NULL,
    is_pinned    INTEGER NOT NULL,
    reply_count  INTEGER,
    json         TEXT NOT NULL,
    created_at   INTEGER,
    updated_at   INTEGER NOT NULL,
    UNIQUE (workspace_id, channel_id, ts)
);
CREATE INDEX message_current_history ON message_current (workspace_id, channel_id, ts_key);
CREATE INDEX message_current_thread ON message_current (workspace_id, channel_id, thread_ts);
CREATE INDEX message_current_user ON message_current (workspace_id, user_id);

-- Full-text index over current message text and file names, kept in sync by triggers.
CREATE VIRTUAL TABLE message_fts USING fts5(
    text, file_names,
    content = 'message_current', content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER message_current_ai AFTER INSERT ON message_current BEGIN
    INSERT INTO message_fts (rowid, text, file_names) VALUES (new.id, new.text, new.file_names);
END;
CREATE TRIGGER message_current_ad AFTER DELETE ON message_current BEGIN
    INSERT INTO message_fts (message_fts, rowid, text, file_names)
    VALUES ('delete', old.id, old.text, old.file_names);
END;
CREATE TRIGGER message_current_au AFTER UPDATE ON message_current BEGIN
    INSERT INTO message_fts (message_fts, rowid, text, file_names)
    VALUES ('delete', old.id, old.text, old.file_names);
    INSERT INTO message_fts (rowid, text, file_names) VALUES (new.id, new.text, new.file_names);
END;

-- Current reaction aggregates per message.
CREATE TABLE reaction (
    workspace_id TEXT NOT NULL,
    channel_id   TEXT NOT NULL,
    ts           TEXT NOT NULL,
    name         TEXT NOT NULL,
    count        INTEGER NOT NULL,
    users        TEXT NOT NULL,
    PRIMARY KEY (workspace_id, channel_id, ts, name)
) WITHOUT ROWID;

CREATE TABLE conversation (
    workspace_id TEXT NOT NULL,
    id           TEXT NOT NULL,
    name         TEXT,
    kind         TEXT NOT NULL,
    is_archived  INTEGER NOT NULL,
    is_member    INTEGER NOT NULL,
    json         TEXT NOT NULL,
    updated_at   INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, id)
) WITHOUT ROWID;
CREATE INDEX conversation_name ON conversation (workspace_id, name COLLATE NOCASE);

CREATE TABLE "user" (
    workspace_id TEXT NOT NULL,
    id           TEXT NOT NULL,
    name         TEXT NOT NULL,
    real_name    TEXT,
    display_name TEXT,
    is_bot       INTEGER NOT NULL,
    deleted      INTEGER NOT NULL,
    json         TEXT NOT NULL,
    updated_at   INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, id)
) WITHOUT ROWID;
"#;

/// Version 2: sidebar data — the team record, channel sections, custom emoji and user prefs.
const V2: &str = r#"
CREATE TABLE team (
    workspace_id TEXT NOT NULL,
    id           TEXT NOT NULL,
    json         TEXT NOT NULL,
    updated_at   INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, id)
) WITHOUT ROWID;

-- One row per section; `seq` keeps the stored (API) order that `sections()` falls back to.
CREATE TABLE channel_section (
    workspace_id TEXT NOT NULL,
    id           TEXT NOT NULL,
    seq          INTEGER NOT NULL,
    json         TEXT NOT NULL,
    updated_at   INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, id)
) WITHOUT ROWID;

CREATE TABLE custom_emoji (
    workspace_id TEXT NOT NULL,
    name         TEXT NOT NULL,
    url          TEXT NOT NULL,
    PRIMARY KEY (workspace_id, name)
) WITHOUT ROWID;

CREATE TABLE prefs (
    workspace_id TEXT NOT NULL PRIMARY KEY,
    json         TEXT NOT NULL,
    updated_at   INTEGER NOT NULL
) WITHOUT ROWID;
"#;

const MIGRATIONS: &[&str] = &[V1, V2];

/// The schema version this build migrates to.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

pub(crate) fn schema_version(conn: &Connection) -> Result<u32> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
}

/// Applies every migration past the stored version. Idempotent.
pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    let found = schema_version(conn)?;
    if found > SCHEMA_VERSION {
        return Err(StoreError::FutureSchema {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(found as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", index as u32 + 1)?;
        tx.commit()?;
    }
    Ok(())
}
