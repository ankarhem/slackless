//! Schema migrations. The applied version lives in `PRAGMA user_version`; each entry of
//! [`MIGRATIONS`] moves the schema one version forward and is applied in its own transaction.
//! Never edit a shipped migration: append a new one.

use rusqlite::{Connection, Transaction, params};

use crate::store::{decode_json_value, extract_inline_file_transcript};
use crate::types::Message;
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

/// Version 9: text-only searchable huddle transcripts (zero audio/video on disk) with FTS5
/// unicode61 diacritic-folding index supporting English + Swedish code-switching.
const V9: &str = r#"
CREATE TABLE huddle_transcript (
    id             INTEGER PRIMARY KEY,
    workspace_id   TEXT NOT NULL,
    channel_id     TEXT NOT NULL,
    call_id        TEXT NOT NULL,
    thread_root_ts TEXT,
    speaker_id     TEXT,
    lang           TEXT,
    source         TEXT NOT NULL DEFAULT 'chime',
    text           TEXT NOT NULL,
    at_ms          INTEGER NOT NULL
);
CREATE INDEX huddle_transcript_call
    ON huddle_transcript (workspace_id, call_id, at_ms);
CREATE INDEX huddle_transcript_channel
    ON huddle_transcript (workspace_id, channel_id, at_ms DESC);
CREATE INDEX huddle_transcript_thread
    ON huddle_transcript (workspace_id, channel_id, thread_root_ts, at_ms);

CREATE VIRTUAL TABLE huddle_transcript_fts USING fts5(
    text,
    content = 'huddle_transcript', content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER huddle_transcript_ai AFTER INSERT ON huddle_transcript BEGIN
    INSERT INTO huddle_transcript_fts (rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER huddle_transcript_ad AFTER DELETE ON huddle_transcript BEGIN
    INSERT INTO huddle_transcript_fts (huddle_transcript_fts, rowid, text)
    VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER huddle_transcript_au AFTER UPDATE OF text ON huddle_transcript BEGIN
    INSERT INTO huddle_transcript_fts (huddle_transcript_fts, rowid, text)
    VALUES ('delete', old.id, old.text);
    INSERT INTO huddle_transcript_fts (rowid, text) VALUES (new.id, new.text);
END;
"#;

/// Version 10: replace the write-amplifying `reaction` table with a `has_reaction` flag on
/// `message_current` (the only query ever run against `reaction` was `EXISTS(...)` for
/// `has:reaction`), and drop redundant prefix indexes (`message_revision_lookup` is a strict
/// prefix of `UNIQUE (workspace_id, channel_id, ts, edit_ts)`; `message_current_user` is a
/// strict prefix of V8's `message_current_user_ts`).
const V10: &str = r#"
ALTER TABLE message_current ADD COLUMN has_reaction INTEGER NOT NULL DEFAULT 0;
UPDATE message_current
SET has_reaction = 1
WHERE EXISTS (
    SELECT 1 FROM reaction r
    WHERE r.workspace_id = message_current.workspace_id
      AND r.channel_id = message_current.channel_id
      AND r.ts = message_current.ts
);
DROP TABLE IF EXISTS reaction;
DROP INDEX IF EXISTS message_revision_lookup;
DROP INDEX IF EXISTS message_current_user;
"#;

/// Version 11: persist searchable video/audio file transcripts (`file_transcript`), map
/// `message_current` rows to attached files (`message_file`), and extend `message_fts` with
/// `file_transcripts` so spoken clip words are searchable alongside message text and file names.
const V11: &str = r#"
CREATE TABLE file_transcript (
    workspace_id TEXT NOT NULL,
    file_id      TEXT NOT NULL,
    source       TEXT NOT NULL,
    text         TEXT NOT NULL,
    updated_at   INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, file_id)
) WITHOUT ROWID;

CREATE TABLE message_file (
    workspace_id TEXT NOT NULL,
    file_id      TEXT NOT NULL,
    message_id   INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, file_id, message_id)
) WITHOUT ROWID;
CREATE INDEX message_file_by_message ON message_file (message_id);

ALTER TABLE message_current ADD COLUMN file_transcripts TEXT NOT NULL DEFAULT '';

DROP TRIGGER IF EXISTS message_current_ai;
DROP TRIGGER IF EXISTS message_current_ad;
DROP TRIGGER IF EXISTS message_current_au;
DROP TABLE IF EXISTS message_fts;

CREATE VIRTUAL TABLE message_fts USING fts5(
    text, file_names, file_transcripts,
    content = 'message_current', content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER message_current_ai AFTER INSERT ON message_current BEGIN
    INSERT INTO message_fts (rowid, text, file_names, file_transcripts)
    VALUES (new.id, new.text, new.file_names, new.file_transcripts);
END;
CREATE TRIGGER message_current_ad AFTER DELETE ON message_current BEGIN
    INSERT INTO message_fts (message_fts, rowid, text, file_names, file_transcripts)
    VALUES ('delete', old.id, old.text, old.file_names, old.file_transcripts);
    DELETE FROM message_file WHERE message_id = old.id;
END;
CREATE TRIGGER message_current_au AFTER UPDATE OF text, file_names, file_transcripts ON message_current BEGIN
    INSERT INTO message_fts (message_fts, rowid, text, file_names, file_transcripts)
    VALUES ('delete', old.id, old.text, old.file_names, old.file_transcripts);
    INSERT INTO message_fts (rowid, text, file_names, file_transcripts)
    VALUES (new.id, new.text, new.file_names, new.file_transcripts);
END;

INSERT INTO message_fts(message_fts) VALUES('rebuild');
"#;

/// Version 12: preserve timed transcript cues JSON (`cues_json`) alongside plain searchable text
/// in `file_transcript` so video/audio transcripts support clickable seek and live cue highlight
/// without polluting the `message_fts` full-text index.
const V12: &str = r#"
ALTER TABLE file_transcript ADD COLUMN cues_json TEXT NOT NULL DEFAULT '';
"#;

/// Version 13: durable offline send outbox (`outbox`) for resilient messages, thread replies, and
/// reactions that fail due to offline/transient transport errors and flush idempotently in FIFO
/// order on reconnect.
const V13: &str = r#"
CREATE TABLE outbox (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    channel_id   TEXT NOT NULL,
    thread_ts    TEXT,
    kind         TEXT NOT NULL CHECK (kind IN ('message', 'reply', 'reaction_add', 'reaction_remove')),
    payload_json TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'sending', 'failed')),
    attempts     INTEGER NOT NULL DEFAULT 0,
    last_error   TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);
CREATE INDEX outbox_ws_status ON outbox (workspace_id, status, id);
CREATE INDEX outbox_ws_channel ON outbox (workspace_id, channel_id, thread_ts, id);
"#;

/// Version 14: index `recent_messages_by_subtype` (huddle threads and other sparse subtypes) so it
/// reads the matching rows newest first instead of walking the workspace's whole timeline.
const V14: &str = r#"
CREATE INDEX IF NOT EXISTS message_current_subtype
    ON message_current (workspace_id, subtype, ts_key DESC);
"#;

const MIGRATIONS: &[&str] = &[V1, V2, V3, V4, V5, V6, V7, V8, V9, V10, V11, V12, V13, V14];

/// The schema version this build migrates to.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

pub(crate) fn schema_version(conn: &Connection) -> Result<u32> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
}

#[cfg(test)]
pub(crate) fn apply_up_to_for_test(conn: &Connection, target_version: usize) {
    for (i, sql) in MIGRATIONS[..target_version].iter().enumerate() {
        conn.execute_batch(sql).unwrap();
        conn.pragma_update(None, "user_version", i as u32 + 1)
            .unwrap();
    }
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
        let version = index as u32 + 1;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        if version == 11 {
            backfill_v11_message_files_and_transcripts(&tx)?;
        }
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

/// Backfill `message_file` and Slack inline clip transcripts on `message_current.file_transcripts`
/// for existing rows with attached files during the V11 migration transaction.
fn backfill_v11_message_files_and_transcripts(tx: &Transaction<'_>) -> Result<()> {
    let mut rows_to_apply: Vec<(i64, String, Vec<String>, Option<String>)> = Vec::new();
    {
        let mut stmt =
            tx.prepare("SELECT id, workspace_id, json FROM message_current WHERE has_file = 1")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                decode_json_value(r.get_ref(2)?)?,
            ))
        })?;
        for row in rows {
            let (id, workspace_id, json) = row?;
            let Ok(message) = serde_json::from_str::<Message>(&json) else {
                continue;
            };
            if message.files.is_empty() {
                continue;
            }
            let mut file_ids = Vec::with_capacity(message.files.len());
            let mut transcripts = Vec::new();
            for file in &message.files {
                if !file.id.as_str().is_empty() {
                    file_ids.push(file.id.as_str().to_owned());
                }
                if let Some(t) = extract_inline_file_transcript(file) {
                    transcripts.push(t);
                }
            }
            let combined = if transcripts.is_empty() {
                None
            } else {
                Some(transcripts.join(" "))
            };
            if !file_ids.is_empty() || combined.is_some() {
                rows_to_apply.push((id, workspace_id, file_ids, combined));
            }
        }
    }

    if rows_to_apply.is_empty() {
        return Ok(());
    }

    let mut ins_file = tx.prepare_cached(
        "INSERT OR IGNORE INTO message_file (workspace_id, file_id, message_id)
         VALUES (?1, ?2, ?3)",
    )?;
    let mut upd_msg =
        tx.prepare_cached("UPDATE message_current SET file_transcripts = ?1 WHERE id = ?2")?;
    for (id, workspace_id, file_ids, combined) in rows_to_apply {
        for file_id in file_ids {
            ins_file.execute(params![workspace_id, file_id, id])?;
        }
        if let Some(transcript_text) = combined {
            upd_msg.execute(params![transcript_text, id])?;
        }
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

    #[test]
    fn future_schema_version_returns_error() {
        let mut conn = Connection::open_in_memory().unwrap();
        let future = SCHEMA_VERSION + 1;
        conn.pragma_update(None, "user_version", future).unwrap();
        let err = migrate(&mut conn).unwrap_err();
        assert!(
            matches!(
                err,
                StoreError::FutureSchema {
                    found,
                    supported,
                } if found == future && supported == SCHEMA_VERSION
            ),
            "expected FutureSchema, got {err:?}"
        );
    }

    #[test]
    fn v7_prunes_single_edited_revision_and_preserves_deleted_original() {
        let mut conn = Connection::open_in_memory().unwrap();
        // Bootstrap up through V6 so we exercise V7 specifically.
        for (i, sql) in MIGRATIONS[..6].iter().enumerate() {
            conn.execute_batch(sql).unwrap();
            conn.pragma_update(None, "user_version", i as u32 + 1)
                .unwrap();
        }
        assert_eq!(schema_version(&conn).unwrap(), 6);

        let already_edited_json = r#"{"ts":"1700000010.000100","text":"arrived edited","edited":{"ts":"1700000010.000200"}}"#;
        let del_orig_json = r#"{"ts":"1700000020.000100","text":"before delete"}"#;
        let del_tomb_json = r#"{"ts":"1700000020.000100","text":"before delete"}"#;
        let del_solo_json = r#"{"ts":"1700000030.000100","text":"solo deleted"}"#;
        let multi_v1_json = r#"{"ts":"1700000040.000100","text":"v1"}"#;
        let multi_v2_json = r#"{"ts":"1700000040.000100","text":"v2"}"#;

        conn.execute(
            "INSERT INTO message_revision
             (workspace_id, channel_id, ts, edit_ts, user_id, text, deleted, json, recorded_at)
             VALUES
             ('T1','C1','1700000010.000100','1700000010.000200','U1','arrived edited',0,?1,100),
             ('T1','C1','1700000020.000100','','U1','before delete',0,?2,100),
             ('T1','C1','1700000020.000100','1700000020.000200','U1','before delete',1,?3,101),
             ('T1','C1','1700000030.000100','1700000030.000200','U1','solo deleted',1,?4,102),
             ('T1','C1','1700000040.000100','','U1','v1',0,?5,103),
             ('T1','C1','1700000040.000100','1700000040.000200','U1','v2',0,?6,104)",
            rusqlite::params![
                already_edited_json,
                del_orig_json,
                del_tomb_json,
                del_solo_json,
                multi_v1_json,
                multi_v2_json
            ],
        )
        .unwrap();

        migrate(&mut conn).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);

        let mut stmt = conn
            .prepare("SELECT ts, edit_ts, deleted FROM message_revision ORDER BY ts, edit_ts")
            .unwrap();
        let remaining: Vec<(String, String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        assert_eq!(
            remaining,
            vec![
                // 1700000010.000100 (single non-deleted edited rev) was pruned.
                // 1700000020.000100 keeps BOTH its non-deleted original AND tombstone.
                ("1700000020.000100".to_owned(), String::new(), 0),
                (
                    "1700000020.000100".to_owned(),
                    "1700000020.000200".to_owned(),
                    1
                ),
                // 1700000030.000100 solo tombstone survives.
                (
                    "1700000030.000100".to_owned(),
                    "1700000030.000200".to_owned(),
                    1
                ),
                // 1700000040.000100 multi-revision non-deleted pair survives.
                ("1700000040.000100".to_owned(), String::new(), 0),
                (
                    "1700000040.000100".to_owned(),
                    "1700000040.000200".to_owned(),
                    0
                ),
            ]
        );
        // Append-only DELETE trigger was restored after V7.
        assert!(conn.execute("DELETE FROM message_revision", []).is_err());
    }

    #[test]
    fn v6_fts_update_trigger_preserves_index_on_json_only_and_syncs_text_or_file_changes() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();

        let trigger_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'message_current_au'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            trigger_sql.contains("AFTER UPDATE OF text, file_names"),
            "unexpected trigger SQL: {trigger_sql}"
        );

        conn.execute(
            "INSERT INTO message_current
             (workspace_id, channel_id, ts, ts_key, edit_ts, user_id, text, file_names, deleted,
              has_link, has_file, is_pinned, reply_count, json, updated_at)
             VALUES
             ('T1','C1','1700000001.000100',1700000001000100,'','U1','alpha payload','spec.pdf',0,0,1,0,NULL,'{\"ts\":\"1700000001.000100\"}',100)",
            [],
        )
        .unwrap();

        let fts_count = |conn: &Connection, q: &str| -> i64 {
            conn.query_row(
                "SELECT count(*) FROM message_fts WHERE message_fts MATCH ?1",
                [q],
                |r| r.get(0),
            )
            .unwrap()
        };
        let check_integrity = |conn: &Connection| {
            conn.execute(
                "INSERT INTO message_fts(message_fts) VALUES('integrity-check')",
                [],
            )
            .unwrap();
        };

        assert_eq!(fts_count(&conn, "alpha"), 1);
        assert_eq!(fts_count(&conn, "spec"), 1);

        // Updating json alone (as vacuum() does) leaves FTS5 untouched and consistent.
        conn.execute(
            "UPDATE message_current SET json = X'789c' WHERE ts = '1700000001.000100'",
            [],
        )
        .unwrap();
        assert_eq!(fts_count(&conn, "alpha"), 1);
        assert_eq!(fts_count(&conn, "spec"), 1);
        check_integrity(&conn);

        // Updating text fires the trigger and re-indexes text.
        conn.execute(
            "UPDATE message_current SET text = 'beta payload' WHERE ts = '1700000001.000100'",
            [],
        )
        .unwrap();
        assert_eq!(fts_count(&conn, "alpha"), 0);
        assert_eq!(fts_count(&conn, "beta"), 1);
        assert_eq!(fts_count(&conn, "spec"), 1);
        check_integrity(&conn);

        // Updating file_names fires the trigger and re-indexes file_names.
        conn.execute(
            "UPDATE message_current SET file_names = 'design.png' WHERE ts = '1700000001.000100'",
            [],
        )
        .unwrap();
        assert_eq!(fts_count(&conn, "spec"), 0);
        assert_eq!(fts_count(&conn, "design"), 1);
        assert_eq!(fts_count(&conn, "beta"), 1);
        check_integrity(&conn);

        // Updating file_transcripts fires the trigger and re-indexes file_transcripts.
        conn.execute(
            "UPDATE message_current SET file_transcripts = 'spoken quarterly roadmap' WHERE ts = '1700000001.000100'",
            [],
        )
        .unwrap();
        assert_eq!(fts_count(&conn, "roadmap"), 1);
        assert_eq!(fts_count(&conn, "design"), 1);
        assert_eq!(fts_count(&conn, "beta"), 1);
        check_integrity(&conn);
    }

    #[test]
    fn v10_backfills_has_reaction_and_drops_reaction_table_and_redundant_indexes() {
        let mut conn = Connection::open_in_memory().unwrap();
        // Bootstrap strictly through V9 so we exercise V10 migration cleanly.
        for (i, sql) in MIGRATIONS[..9].iter().enumerate() {
            conn.execute_batch(sql).unwrap();
            conn.pragma_update(None, "user_version", i as u32 + 1)
                .unwrap();
        }
        assert_eq!(schema_version(&conn).unwrap(), 9);

        conn.execute(
            "INSERT INTO message_current
             (workspace_id, channel_id, ts, ts_key, edit_ts, user_id, text, file_names, deleted,
              has_link, has_file, is_pinned, reply_count, json, updated_at)
             VALUES
             ('T1','C1','1700000001.000100',1700000001000100,'','U1','reacted msg','',0,0,0,0,NULL,'{\"ts\":\"1700000001.000100\"}',100),
             ('T1','C1','1700000002.000100',1700000002000100,'','U1','plain msg','',0,0,0,0,NULL,'{\"ts\":\"1700000002.000100\"}',101)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO reaction (workspace_id, channel_id, ts, name, count, users)
             VALUES ('T1', 'C1', '1700000001.000100', 'rocket', 2, '[\"U1\",\"U2\"]')",
            [],
        )
        .unwrap();

        migrate(&mut conn).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);

        let reacted_flag: i64 = conn
            .query_row(
                "SELECT has_reaction FROM message_current WHERE ts = '1700000001.000100'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let plain_flag: i64 = conn
            .query_row(
                "SELECT has_reaction FROM message_current WHERE ts = '1700000002.000100'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reacted_flag, 1);
        assert_eq!(plain_flag, 0);

        let reaction_table: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'reaction'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reaction_table, 0);

        for dropped_idx in ["message_revision_lookup", "message_current_user"] {
            let exists: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    [dropped_idx],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(exists, 0, "redundant index {dropped_idx} should be dropped");
        }

        // FTS5 stays intact after V10 backfill UPDATE (V6 trigger scopes to text, file_names).
        conn.execute(
            "INSERT INTO message_fts(message_fts) VALUES('integrity-check')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn v12_migrates_cleanly_to_v13_outbox_with_constraints_and_indexes() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_up_to_for_test(&conn, 12);
        assert_eq!(schema_version(&conn).unwrap(), 12);

        // Seed existing V12 data to verify zero data loss on migration.
        conn.execute(
            "INSERT INTO file_transcript (workspace_id, file_id, source, text, cues_json, updated_at)
             VALUES ('T1', 'F1', 'parakeet', 'hello clip', '[]', 1700000000)",
            [],
        )
        .unwrap();

        migrate(&mut conn).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), SCHEMA_VERSION);

        // Existing V12 row survives untouched.
        let ft_text: String = conn
            .query_row(
                "SELECT text FROM file_transcript WHERE workspace_id = 'T1' AND file_id = 'F1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ft_text, "hello clip");

        // Outbox table and indexes exist.
        for idx in ["outbox_ws_status", "outbox_ws_channel"] {
            let exists: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    [idx],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "missing V13 index {idx}");
        }

        // CHECK constraints reject invalid kind or status.
        assert!(
            conn.execute(
                "INSERT INTO outbox
                 (workspace_id, channel_id, kind, payload_json, status, attempts, created_at, updated_at)
                 VALUES ('T1', 'C1', 'invalid_kind', '{}', 'pending', 0, 100, 100)",
                [],
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "INSERT INTO outbox
                 (workspace_id, channel_id, kind, payload_json, status, attempts, created_at, updated_at)
                 VALUES ('T1', 'C1', 'message', '{}', 'bogus_status', 0, 100, 100)",
                [],
            )
            .is_err()
        );
        // Valid row succeeds.
        conn.execute(
            "INSERT INTO outbox
             (workspace_id, channel_id, thread_ts, kind, payload_json, status, attempts, last_error, created_at, updated_at)
             VALUES ('T1', 'C1', NULL, 'message', '{\"text\":\"hi\"}', 'pending', 0, NULL, 100, 100)",
            [],
        )
        .unwrap();
    }
}
