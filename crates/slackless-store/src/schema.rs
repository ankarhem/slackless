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

/// Version 3: slackless-local state Slack does not keep for this client (the last open
/// conversation, restored at launch).
const V3: &str = r#"
CREATE TABLE local_state (
    workspace_id TEXT NOT NULL,
    key          TEXT NOT NULL,
    value        TEXT NOT NULL,
    PRIMARY KEY (workspace_id, key)
) WITHOUT ROWID;
"#;

/// Version 4: durable offline mutation queue surviving restarts until flushed on reconnect.
const V4: &str = r#"
CREATE TABLE offline_queue (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    payload      TEXT NOT NULL,
    created_at   INTEGER NOT NULL
);
CREATE INDEX offline_queue_ws ON offline_queue (workspace_id, id);
"#;

/// Version 5: persisted `bots.info` profiles for zero-flash cold boot rendering.
const V5: &str = r#"
CREATE TABLE bot_profile (
    workspace_id TEXT NOT NULL,
    bot_id       TEXT NOT NULL,
    json         TEXT NOT NULL,
    updated_at   INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, bot_id)
) WITHOUT ROWID;
"#;

/// Version 6: rebuild `message_revision` as a ROWID table (eliminating WITHOUT ROWID B-tree bloat
/// on large JSON rows) retaining only edited/deleted/multi-revision history (untouched originals
/// live in `message_current`), and scope the FTS5 update trigger to `text, file_names` so payload
/// compaction never churns the full-text index.
const V6: &str = r#"
DROP TRIGGER IF EXISTS message_revision_no_update;
DROP TRIGGER IF EXISTS message_revision_no_delete;
CREATE TABLE message_revision_v6 (
    id           INTEGER PRIMARY KEY,
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
    UNIQUE (workspace_id, channel_id, ts, edit_ts)
);
CREATE INDEX message_revision_lookup ON message_revision_v6 (workspace_id, channel_id, ts);
INSERT INTO message_revision_v6
    (workspace_id, channel_id, ts, edit_ts, user_id, thread_ts, subtype, text, deleted,
     json, created_at, recorded_at)
SELECT r.workspace_id, r.channel_id, r.ts, r.edit_ts, r.user_id, r.thread_ts, r.subtype,
       r.text, r.deleted, r.json, r.created_at, r.recorded_at
FROM message_revision r
WHERE r.deleted = 1
   OR r.edit_ts != ''
   OR (r.workspace_id, r.channel_id, r.ts) IN (
       SELECT workspace_id, channel_id, ts
       FROM message_revision
       GROUP BY workspace_id, channel_id, ts
       HAVING COUNT(*) > 1
   );
DROP TABLE message_revision;
ALTER TABLE message_revision_v6 RENAME TO message_revision;
CREATE TRIGGER message_revision_no_update BEFORE UPDATE ON message_revision
BEGIN SELECT RAISE(ABORT, 'message_revision is append-only'); END;
CREATE TRIGGER message_revision_no_delete BEFORE DELETE ON message_revision
BEGIN SELECT RAISE(ABORT, 'message_revision is append-only'); END;

DROP TRIGGER IF EXISTS message_current_au;
CREATE TRIGGER message_current_au AFTER UPDATE OF text, file_names ON message_current BEGIN
    INSERT INTO message_fts (message_fts, rowid, text, file_names)
    VALUES ('delete', old.id, old.text, old.file_names);
    INSERT INTO message_fts (rowid, text, file_names) VALUES (new.id, new.text, new.file_names);
END;
"#;

/// Version 7: prune single-revision non-deleted rows from `message_revision` (messages that had
/// already been edited once on Slack prior to sync and have no separate prior revision locally;
/// their sole revision lives compressed in `message_current`).
const V7: &str = r#"
DROP TRIGGER IF EXISTS message_revision_no_delete;
DELETE FROM message_revision
WHERE deleted = 0
  AND (workspace_id, channel_id, ts) NOT IN (
      SELECT workspace_id, channel_id, ts
      FROM message_revision
      GROUP BY workspace_id, channel_id, ts
      HAVING COUNT(*) > 1 OR MAX(deleted) = 1
  );
CREATE TRIGGER message_revision_no_delete BEFORE DELETE ON message_revision
BEGIN SELECT RAISE(ABORT, 'message_revision is append-only'); END;
"#;

/// Version 8: covering/partial indexes eliminating full-table scans & filesorts in
/// `recent_messages_by_user`, `recent_threads`, and non-FTS `search_sorted`.
const V8: &str = r#"
CREATE INDEX IF NOT EXISTS message_current_user_ts
    ON message_current (workspace_id, user_id, ts_key DESC);
CREATE INDEX IF NOT EXISTS message_current_ws_ts
    ON message_current (workspace_id, ts_key DESC);
CREATE INDEX IF NOT EXISTS message_current_threads
    ON message_current (workspace_id, ts_key DESC)
    WHERE reply_count > 0 AND deleted = 0;
"#;

const MIGRATIONS: &[&str] = &[V1, V2, V3, V4, V5, V6, V7, V8];

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn populated_v1_database_migrates_cleanly_to_latest() {
        let mut conn = Connection::open_in_memory().unwrap();
        // Bootstrap strictly at schema V1 with populated untouched, edited, and deleted rows.
        conn.execute_batch(V1).unwrap();
        conn.pragma_update(None, "user_version", 1u32).unwrap();

        let untouched_json = r#"{"ts":"1700000001.000100","text":"untouched original"}"#;
        let edited_v1_json = r#"{"ts":"1700000002.000100","text":"before edit"}"#;
        let edited_v2_json = r#"{"ts":"1700000002.000100","text":"after edit","edited":{"user":"U1","ts":"1700000002.000200"}}"#;
        let deleted_json = r#"{"ts":"1700000003.000100","text":"gone message"}"#;

        conn.execute(
            "INSERT INTO message_revision
             (workspace_id, channel_id, ts, edit_ts, user_id, text, deleted, json, recorded_at)
             VALUES
             ('T1','C1','1700000001.000100','','U1','untouched original',0,?1,100),
             ('T1','C1','1700000002.000100','','U1','before edit',0,?2,100),
             ('T1','C1','1700000002.000100','1700000002.000200','U1','after edit',0,?3,101),
             ('T1','C1','1700000003.000100','1700000003.000200','U1','gone message',1,?4,102)",
            rusqlite::params![untouched_json, edited_v1_json, edited_v2_json, deleted_json],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message_current
             (workspace_id, channel_id, ts, ts_key, edit_ts, user_id, text, file_names, deleted,
              has_link, has_file, is_pinned, reply_count, json, updated_at)
             VALUES
             ('T1','C1','1700000001.000100',1700000001000100,'','U1','untouched original','',0,0,0,0,NULL,?1,100),
             ('T1','C1','1700000002.000100',1700000002000100,'1700000002.000200','U1','after edit','',0,0,0,0,2,?2,101),
             ('T1','C1','1700000003.000100',1700000003000100,'1700000003.000200','U1','gone message','',1,0,0,0,NULL,?3,102)",
            rusqlite::params![untouched_json, edited_v2_json, deleted_json],
        )
        .unwrap();

        migrate(&mut conn).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);

        // Untouched single-revision row was pruned from message_revision while edited + deleted survive.
        let rev_count: i64 = conn
            .query_row("SELECT count(*) FROM message_revision", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rev_count, 3);

        // All 3 messages survive in message_current and FTS5 remains searchable.
        let cur_count: i64 = conn
            .query_row("SELECT count(*) FROM message_current", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cur_count, 3);
        let fts_hits: i64 = conn
            .query_row(
                "SELECT count(*) FROM message_fts WHERE message_fts MATCH 'untouched'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fts_hits, 1);

        // Append-only triggers survived the V6/V7 table rebuilds.
        assert!(conn.execute("DELETE FROM message_revision", []).is_err());

        // V8 covering/partial indexes are present.
        for idx in [
            "message_current_user_ts",
            "message_current_ws_ts",
            "message_current_threads",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    [idx],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "missing V8 index {idx}");
        }
    }
}
