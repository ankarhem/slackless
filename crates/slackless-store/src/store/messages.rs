//! Message upserts, deletions, history/thread queries, revisions, and zlib JSON storage.

use std::cmp::Ordering;

use super::*;

/// Parameters shared by [`insert_revision`] and [`write_current`].
struct MessageWrite<'a> {
    workspace: &'a TeamId,
    channel: &'a ChannelId,
    message: &'a Message,
    edit_ts: &'a str,
    deleted: bool,
    json: &'a SqlValue,
}

impl Store {
    /// Records a message observed from a live event: the websocket, a `chat.*` echo, or a
    /// local rewrite of a stored message (reaction toggle, bot profile).
    ///
    /// For a `message_changed` event pass its inner `message`: the edit is keyed by `edited.ts`
    /// and appended as a new revision. The current state follows the newest revision, and the
    /// same revision key replaces the row (that is how a reaction or reply count changes);
    /// once a message is deleted, later (stale) observations are logged but never resurrect it.
    /// Pages fetched from `conversations.*` go through [`Self::upsert_history_page`] instead.
    pub fn upsert_message(
        &mut self,
        workspace: &TeamId,
        channel: &ChannelId,
        message: &Message,
    ) -> Result<()> {
        self.upsert_messages(workspace, channel, std::slice::from_ref(message))
    }

    /// Records a batch of live observations for `channel` in a single SQLite transaction (one
    /// WAL commit and one revision bump for the whole batch), with [`Self::upsert_message`]
    /// semantics per message.
    pub fn upsert_messages(
        &mut self,
        workspace: &TeamId,
        channel: &ChannelId,
        messages: &[Message],
    ) -> Result<()> {
        self.ensure_writable()?;
        if messages.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        for message in messages {
            upsert_message_in_tx(&tx, workspace, channel, message, SameRevision::Replace)?;
        }
        tx.commit()?;
        self.bump_message_revision();
        Ok(())
    }

    /// Records a `conversations.history` page of `channel`, requested at `fetched_at` (unix
    /// seconds, read before the request was sent), in a single SQLite transaction.
    ///
    /// The page is a snapshot that can predate live events already applied: when the stored
    /// copy carries the same `edited.ts` but was updated at or after `fetched_at`, the page's
    /// copy is skipped, so a reaction, reply count or pin change that arrived over the websocket
    /// while the page was in flight survives. A newer `edited.ts` always wins, as for live writes.
    ///
    /// The page is also a complete view of the channel-level messages between its oldest and
    /// newest `ts`, and `conversations.history` simply omits deleted messages: a stored, undeleted
    /// top-level message in that span that the page omits was deleted while no `message_deleted`
    /// reached this client, and is tombstoned with [`Self::record_deletion`] semantics (content
    /// kept, nothing removed). Thread replies (never in such a page), ephemeral and hidden rows,
    /// rows updated at or after `fetched_at`, and empty pages are never reconciled. Returns the
    /// number of messages tombstoned.
    pub fn upsert_history_page(
        &mut self,
        workspace: &TeamId,
        channel: &ChannelId,
        messages: &[Message],
        fetched_at: i64,
    ) -> Result<usize> {
        self.upsert_page(workspace, channel, messages, fetched_at, true)
    }

    /// Records a `conversations.replies` page requested at `fetched_at` (unix seconds) with the
    /// live-event protection of [`Self::upsert_history_page`] but without deletion
    /// reconciliation: the page covers one thread, so absence from it says nothing about the rest
    /// of the channel.
    pub fn upsert_replies_page(
        &mut self,
        workspace: &TeamId,
        channel: &ChannelId,
        messages: &[Message],
        fetched_at: i64,
    ) -> Result<()> {
        self.upsert_page(workspace, channel, messages, fetched_at, false)
            .map(|_| ())
    }

    fn upsert_page(
        &mut self,
        workspace: &TeamId,
        channel: &ChannelId,
        messages: &[Message],
        fetched_at: i64,
        reconcile_deletions: bool,
    ) -> Result<usize> {
        self.ensure_writable()?;
        if messages.is_empty() {
            return Ok(0);
        }
        // IMMEDIATE: the page reads `message_current` before writing it while the GUI and the
        // CLI share the file; holding the write lock from the start turns a concurrent live write
        // into a short wait instead of a `SQLITE_BUSY_SNAPSHOT` failure that would drop the page.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let same = SameRevision::SkipIfUpdatedSince(fetched_at);
        for message in messages {
            upsert_message_in_tx(&tx, workspace, channel, message, same)?;
        }
        let tombstoned = if reconcile_deletions {
            tombstone_absent_from_page(&tx, workspace, channel, messages, fetched_at)?
        } else {
            0
        };
        tx.commit()?;
        self.bump_message_revision();
        Ok(tombstoned)
    }

    /// Forgets the current state of ephemeral messages ("Only visible to you", like Giphy's
    /// preview): Slack keeps no copy, so one left from a past session would otherwise show
    /// forever. Their revisions stay in the log. Returns how many were dropped.
    pub fn drop_ephemeral(&mut self, workspace: &TeamId) -> Result<usize> {
        self.ensure_writable()?;
        let n = self.conn.execute(
            "DELETE FROM message_current
             WHERE workspace_id = ?1 AND typeof(json) = 'text'
               AND json_extract(json, '$.is_ephemeral') = 1",
            params![workspace.as_str()],
        )?;
        if n > 0 {
            self.bump_message_revision();
        }
        Ok(n)
    }

    /// Records a `message_deleted` as a tombstone revision carrying the prior content
    /// (`previous`, else the current stored state). Nothing is removed.
    ///
    /// `event_ts` keys the tombstone; when absent the current time is used.
    pub fn record_deletion(
        &mut self,
        workspace: &TeamId,
        channel: &ChannelId,
        deleted_ts: &Ts,
        event_ts: Option<&Ts>,
        previous: Option<&Message>,
    ) -> Result<()> {
        self.ensure_writable()?;
        let tx = self.conn.transaction()?;
        record_deletion_in_tx(&tx, workspace, channel, deleted_ts, event_ts, previous)?;
        tx.commit()?;
        self.bump_message_revision();
        Ok(())
    }

    /// Up to `limit` channel-level messages (roots, plain messages, thread broadcasts) older
    /// than `before` (newest when `None`), oldest first. Deleted messages are included.
    pub fn channel_history(
        &self,
        workspace: &TeamId,
        channel: &ChannelId,
        before: Option<&Ts>,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let before_key = before.map_or(i64::MAX, ts_key);
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, json, deleted FROM (
                 SELECT channel_id, json, deleted, ts_key FROM message_current
                 WHERE workspace_id = ?1 AND channel_id = ?2 AND ts_key < ?3
                   AND (thread_ts IS NULL OR thread_ts = ts OR subtype = ?4)
                 ORDER BY ts_key DESC LIMIT ?5)
             ORDER BY ts_key ASC",
        )?;
        let rows = stmt.query_map(
            params![
                workspace.as_str(),
                channel.as_str(),
                before_key,
                MessageSubtype::ThreadBroadcast.as_str(),
                limit as i64,
            ],
            raw_row,
        )?;
        collect(rows)
    }

    /// Each of `channels`' newest undeleted channel-level message (roots, plain messages and
    /// thread broadcasts; replies excluded), using the `(workspace_id, channel_id, ts_key)` index
    /// directly (`ORDER BY ts_key DESC LIMIT 1` per channel) inside a single read snapshot so
    /// large DM histories are never scanned by a window function.
    pub fn latest_messages(
        &self,
        workspace: &TeamId,
        channels: &[ChannelId],
    ) -> Result<HashMap<ChannelId, StoredMessage>> {
        if channels.is_empty() {
            return Ok(HashMap::new());
        }
        self.with_read_tx(|tx| {
            let mut stmt = tx.prepare_cached(
                "SELECT channel_id, json, deleted FROM message_current
                 WHERE workspace_id = ?1 AND channel_id = ?2
                   AND deleted = 0
                   AND (thread_ts IS NULL OR thread_ts = ts OR subtype = ?3)
                 ORDER BY ts_key DESC LIMIT 1",
            )?;
            let broadcast = MessageSubtype::ThreadBroadcast.as_str();
            let ws = workspace.as_str();
            let mut out = HashMap::with_capacity(channels.len());
            for channel in channels {
                let row = stmt
                    .query_row(params![ws, channel.as_str(), broadcast], raw_row)
                    .optional()?;
                if let Some(message) = row.and_then(decode_or_skip) {
                    out.insert(channel.clone(), message);
                }
            }
            Ok(out)
        })
    }

    /// Up to `limit` newest undeleted messages sent by `user` across all conversations in
    /// `workspace`, newest first (used to populate Drafts & sent -> Sent across restarts).
    pub fn recent_messages_by_user(
        &self,
        workspace: &TeamId,
        user: &UserId,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, json, deleted FROM message_current
             WHERE workspace_id = ?1 AND user_id = ?2 AND deleted = 0
             ORDER BY ts_key DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![workspace.as_str(), user.as_str(), limit as i64],
            raw_row,
        )?;
        collect(rows)
    }

    /// Which of `channels` have at least one stored message, deleted ones included: the
    /// DM-list preview backfill fetches only the conversations this leaves out.
    pub fn channels_with_messages(
        &self,
        workspace: &TeamId,
        channels: &[ChannelId],
    ) -> Result<HashSet<ChannelId>> {
        if channels.is_empty() {
            return Ok(HashSet::new());
        }
        let mut out = HashSet::new();
        query_in_chunks(
            &self.conn,
            workspace,
            channels,
            ChannelId::as_str,
            "SELECT DISTINCT channel_id FROM message_current
             WHERE workspace_id = ?1 AND channel_id",
            |r| r.get::<_, String>(0),
            |ch| {
                out.insert(ChannelId::from(ch));
            },
        )?;
        Ok(out)
    }

    /// A thread's root and replies, oldest first.
    pub fn thread_replies(
        &self,
        workspace: &TeamId,
        channel: &ChannelId,
        thread_ts: &Ts,
    ) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, json, deleted FROM message_current
             WHERE workspace_id = ?1 AND channel_id = ?2 AND (ts = ?3 OR thread_ts = ?3)
             ORDER BY ts_key ASC",
        )?;
        let rows = stmt.query_map(
            params![workspace.as_str(), channel.as_str(), thread_ts.as_str()],
            raw_row,
        )?;
        collect(rows)
    }

    /// Every revision of a message, in the order observed.
    pub fn revisions(
        &self,
        workspace: &TeamId,
        channel: &ChannelId,
        ts: &Ts,
    ) -> Result<Vec<Revision>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT edit_ts, deleted, json FROM message_revision
             WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3
             ORDER BY recorded_at, edit_ts",
        )?;
        let rows = stmt.query_map(
            params![workspace.as_str(), channel.as_str(), ts.as_str()],
            raw_revision_row,
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(decode_revision(row?)?);
        }
        if out.is_empty() {
            let mut cur_stmt = self.conn.prepare_cached(
                "SELECT edit_ts, deleted, json FROM message_current
                 WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
            )?;
            let current = cur_stmt
                .query_row(
                    params![workspace.as_str(), channel.as_str(), ts.as_str()],
                    raw_revision_row,
                )
                .optional()?;
            if let Some(raw) = current {
                out.push(decode_revision(raw)?);
            }
        }
        Ok(out)
    }

    /// Archived thread roots (`reply_count > 0`, undeleted), newest first, optionally scoped
    /// to a single `channel`.
    pub fn recent_threads(
        &self,
        workspace: &TeamId,
        channel: Option<&ChannelId>,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        // Use bare `reply_count > 0` (not `COALESCE(reply_count, 0) > 0`) so SQLite uses
        // V8's partial index `message_current_threads (workspace_id, ts_key DESC) WHERE
        // reply_count > 0 AND deleted = 0`.
        match channel {
            Some(ch) => {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT channel_id, json, deleted FROM message_current
                     WHERE workspace_id = ?1 AND channel_id = ?2 AND deleted = 0
                       AND reply_count > 0
                     ORDER BY ts_key DESC LIMIT ?3",
                )?;
                let rows = stmt.query_map(
                    params![workspace.as_str(), ch.as_str(), limit as i64],
                    raw_row,
                )?;
                collect(rows)
            }
            None => {
                let mut stmt = self.conn.prepare_cached(RECENT_THREADS_WORKSPACE_SQL)?;
                let rows = stmt.query_map(params![workspace.as_str(), limit as i64], raw_row)?;
                collect(rows)
            }
        }
    }
}

/// [`Store::record_deletion`] inside `tx`; the caller's transaction bumps the revision once.
fn record_deletion_in_tx(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    channel: &ChannelId,
    deleted_ts: &Ts,
    event_ts: Option<&Ts>,
    previous: Option<&Message>,
) -> Result<()> {
    archive_current_as_revision(tx, workspace, channel, deleted_ts)?;
    let mut prior = match previous {
        Some(m) => m.clone(),
        None => {
            let mut stmt = tx.prepare_cached(
                "SELECT json FROM message_current
                     WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
            )?;
            let stored: Option<String> = stmt
                .query_row(
                    params![workspace.as_str(), channel.as_str(), deleted_ts.as_str()],
                    |r| decode_json_value(r.get_ref(0)?),
                )
                .optional()?;
            match stored {
                Some(json) => serde_json::from_str(&json)?,
                None => Message::default(),
            }
        }
    };
    prior.ts = deleted_ts.clone();
    let edit_ts = event_ts.map_or_else(now_ts, |t| t.as_str().to_owned());
    let json = serde_json::to_string(&prior)?;
    let stored_json = encode_json_value(&json, is_ephemeral_message(&prior));
    let write = MessageWrite {
        workspace,
        channel,
        message: &prior,
        edit_ts: &edit_ts,
        deleted: true,
        json: &stored_json,
    };
    insert_revision(tx, &write)?;
    write_current(tx, &write, true)?;
    Ok(())
}

/// What a write does with the current row when the incoming message carries the same revision
/// key (`edited.ts`) as the stored copy.
#[derive(Clone, Copy)]
enum SameRevision {
    /// A live observation: the incoming copy replaces the row.
    Replace,
    /// A page requested at `fetched_at` (unix seconds): a row updated at or after that moment
    /// was written by a live event the page predates, so the page's copy is skipped. Rows updated
    /// within the request's own second are skipped too: `updated_at` has second resolution, and
    /// keeping an equally fresh copy costs nothing while the next fetch still corrects it.
    SkipIfUpdatedSince(i64),
}

impl SameRevision {
    /// Whether the incoming copy (revision key `edit_ts`) replaces the stored, undeleted row
    /// (revision key `current_edit`, last written at `updated_at`).
    fn replaces(&self, edit_ts: &str, current_edit: &str, updated_at: i64) -> bool {
        match Ts::from(edit_ts).cmp(&Ts::from(current_edit)) {
            Ordering::Less => false,
            Ordering::Greater => true,
            Ordering::Equal => match self {
                Self::Replace => true,
                Self::SkipIfUpdatedSince(fetched_at) => updated_at < *fetched_at,
            },
        }
    }
}

/// Tombstones the stored top-level messages inside the page's `[oldest, newest]` span that the
/// page omits (see [`Store::upsert_history_page`]). Rows written at or after `fetched_at` are
/// left alone, as are rows whose JSON no longer parses (nothing is tombstoned on a guess).
/// Returns how many were tombstoned.
fn tombstone_absent_from_page(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    channel: &ChannelId,
    page: &[Message],
    fetched_at: i64,
) -> Result<usize> {
    let mut keys = page.iter().map(|m| ts_key(&m.ts));
    let Some(first) = keys.next() else {
        return Ok(0);
    };
    let (oldest, newest) = keys.fold((first, first), |(lo, hi), key| (lo.min(key), hi.max(key)));
    let in_page: HashSet<&str> = page.iter().map(|m| m.ts.as_str()).collect();
    let absent: Vec<String> = {
        let mut stmt = tx.prepare_cached(
            "SELECT ts FROM message_current
             WHERE workspace_id = ?1 AND channel_id = ?2 AND deleted = 0
               AND ts_key BETWEEN ?3 AND ?4
               AND (thread_ts IS NULL OR thread_ts = ts)
               AND updated_at < ?5",
        )?;
        let rows = stmt.query_map(
            params![
                workspace.as_str(),
                channel.as_str(),
                oldest,
                newest,
                fetched_at
            ],
            |r| r.get::<_, String>(0),
        )?;
        let mut out = Vec::new();
        for ts in rows {
            let ts = ts?;
            if !in_page.contains(ts.as_str()) {
                out.push(ts);
            }
        }
        out
    };
    let mut tombstoned = 0;
    for ts in absent {
        let stored = stored_message_json(tx, workspace, channel, &ts)?;
        let Ok(message) = serde_json::from_str::<Message>(&stored) else {
            continue;
        };
        if message.hidden || is_ephemeral_message(&message) {
            continue;
        }
        record_deletion_in_tx(tx, workspace, channel, &Ts::from(ts), None, Some(&message))?;
        tombstoned += 1;
    }
    Ok(tombstoned)
}

/// The decoded `json` of one stored message (empty when the row is gone).
fn stored_message_json(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    channel: &ChannelId,
    ts: &str,
) -> Result<String> {
    let mut stmt = tx.prepare_cached(
        "SELECT json FROM message_current
         WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
    )?;
    Ok(stmt
        .query_row(params![workspace.as_str(), channel.as_str(), ts], |r| {
            decode_json_value(r.get_ref(0)?)
        })
        .optional()?
        .unwrap_or_default())
}

/// Workspace-wide `recent_threads` query, shared with tests to assert `EXPLAIN QUERY PLAN`
/// hits the V8 partial index `message_current_threads`.
pub(super) const RECENT_THREADS_WORKSPACE_SQL: &str =
    "SELECT channel_id, json, deleted FROM message_current
 WHERE workspace_id = ?1 AND deleted = 0
   AND reply_count > 0
 ORDER BY ts_key DESC LIMIT ?2";

pub(super) fn is_ephemeral_message(message: &Message) -> bool {
    message.is_ephemeral
        || message
            .extra
            .get("is_ephemeral")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

/// Compress JSON payloads >= 160 bytes with fast zlib when it saves space (unless `force_text`,
/// used for ephemeral messages so `json_extract(json, '$.is_ephemeral')` stays index-free).
pub(super) fn encode_json_value(json: &str, force_text: bool) -> SqlValue {
    if !force_text && json.len() >= 160 {
        let mut enc = ZlibEncoder::new(Vec::with_capacity(json.len() / 2), Compression::fast());
        if enc.write_all(json.as_bytes()).is_ok()
            && let Ok(compressed) = enc.finish()
            && compressed.len() + 16 < json.len()
        {
            return SqlValue::Blob(compressed);
        }
    }
    SqlValue::Text(json.to_owned())
}

/// Decode a `json` column stored as either legacy UTF-8 `TEXT` or zlib-compressed `BLOB`.
pub(crate) fn decode_json_value(val: ValueRef<'_>) -> rusqlite::Result<String> {
    match val {
        ValueRef::Text(bytes) => std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|e| rusqlite::Error::Utf8Error(0, e)),
        ValueRef::Blob(bytes) => {
            let mut dec = ZlibDecoder::new(bytes);
            let mut out = String::new();
            dec.read_to_string(&mut out).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Blob,
                    Box::new(e),
                )
            })?;
            Ok(out)
        }
        other => Err(rusqlite::Error::InvalidColumnType(
            0,
            "json".to_owned(),
            other.data_type(),
        )),
    }
}

/// Snapshot the existing `message_current` row into `message_revision` before an edit or deletion
/// replaces it, preserving the original content verbatim in the append-only log.
fn archive_current_as_revision(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    channel: &ChannelId,
    ts: &Ts,
) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "INSERT OR IGNORE INTO message_revision
             (workspace_id, channel_id, ts, edit_ts, user_id, thread_ts, subtype, text, deleted,
              json, created_at, recorded_at)
         SELECT workspace_id, channel_id, ts, edit_ts, user_id, thread_ts, subtype, text, deleted,
                json, created_at, updated_at
         FROM message_current
         WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
    )?;
    stmt.execute(params![workspace.as_str(), channel.as_str(), ts.as_str()])?;
    Ok(())
}

fn insert_revision(tx: &Transaction<'_>, w: &MessageWrite<'_>) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "INSERT OR IGNORE INTO message_revision
             (workspace_id, channel_id, ts, edit_ts, user_id, thread_ts, subtype, text, deleted,
              json, created_at, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?;
    stmt.execute(params![
        w.workspace.as_str(),
        w.channel.as_str(),
        w.message.ts.as_str(),
        w.edit_ts,
        w.message.user.as_ref().map(|u| u.as_str()),
        w.message.thread_ts.as_ref().map(|t| t.as_str()),
        w.message.subtype.as_ref().map(|s| s.as_str()),
        w.message.text,
        w.deleted,
        w.json,
        w.message.ts.secs(),
        now_epoch_secs(),
    ])?;
    Ok(())
}

fn write_current(tx: &Transaction<'_>, w: &MessageWrite<'_>, existed: bool) -> Result<()> {
    let file_names = w
        .message
        .files
        .iter()
        .flat_map(|f| [f.name.as_deref(), f.title.as_deref()])
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let file_transcripts =
        file_transcripts::compute_message_file_transcripts(tx, w.workspace, &w.message.files)?;
    let has_link = w.message.text.contains("http://")
        || w.message.text.contains("https://")
        || w.message
            .attachments
            .iter()
            .any(|a| a.from_url.is_some() || a.title_link.is_some());
    let has_reaction = !w.message.reactions.is_empty();
    let message_id: i64 = {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO message_current
                 (workspace_id, channel_id, ts, ts_key, edit_ts, user_id, thread_ts, subtype, text,
                  file_names, file_transcripts, deleted, has_link, has_file, has_reaction,
                  is_pinned, reply_count, json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                     ?18, ?19, ?20)
             ON CONFLICT (workspace_id, channel_id, ts) DO UPDATE SET
                 ts_key = excluded.ts_key, edit_ts = excluded.edit_ts, user_id = excluded.user_id,
                 thread_ts = excluded.thread_ts, subtype = excluded.subtype, text = excluded.text,
                 file_names = excluded.file_names, file_transcripts = excluded.file_transcripts,
                 deleted = excluded.deleted, has_link = excluded.has_link,
                 has_file = excluded.has_file, has_reaction = excluded.has_reaction,
                 is_pinned = excluded.is_pinned, reply_count = excluded.reply_count,
                 json = excluded.json, created_at = excluded.created_at,
                 updated_at = excluded.updated_at
             RETURNING id",
        )?;
        stmt.query_row(
            params![
                w.workspace.as_str(),
                w.channel.as_str(),
                w.message.ts.as_str(),
                ts_key(&w.message.ts),
                w.edit_ts,
                w.message.user.as_ref().map(|u| u.as_str()),
                w.message.thread_ts.as_ref().map(|t| t.as_str()),
                w.message.subtype.as_ref().map(|s| s.as_str()),
                w.message.text,
                file_names,
                file_transcripts,
                w.deleted,
                has_link,
                !w.message.files.is_empty(),
                has_reaction,
                !w.message.pinned_to.is_empty(),
                w.message.reply_count,
                w.json,
                w.message.ts.secs(),
                now_epoch_secs(),
            ],
            |r| r.get(0),
        )?
    };
    if existed || !w.message.files.is_empty() {
        let mut del = tx.prepare_cached("DELETE FROM message_file WHERE message_id = ?1")?;
        del.execute(params![message_id])?;
        if !w.message.files.is_empty() {
            let mut ins = tx.prepare_cached(
                "INSERT OR IGNORE INTO message_file (workspace_id, file_id, message_id)
                 VALUES (?1, ?2, ?3)",
            )?;
            for file in &w.message.files {
                if !file.id.as_str().is_empty() {
                    ins.execute(params![w.workspace.as_str(), file.id.as_str(), message_id])?;
                }
            }
        }
    }
    Ok(())
}

/// Maximum number of bind parameters per SQL `IN (?, ...)` chunk (well below SQLite's limit).
pub(super) const SQL_IN_CHUNK: usize = 500;

fn upsert_message_in_tx(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    channel: &ChannelId,
    message: &Message,
    same: SameRevision,
) -> Result<()> {
    let edit_ts = message.edited.as_ref().map_or("", |e| e.ts.as_str());
    let json = serde_json::to_string(message)?;
    let ephemeral = is_ephemeral_message(message);
    let stored_json = encode_json_value(&json, ephemeral);
    let current: Option<(String, bool, i64)> = {
        let mut stmt = tx.prepare_cached(
            "SELECT edit_ts, deleted, updated_at FROM message_current
             WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
        )?;
        stmt.query_row(
            params![workspace.as_str(), channel.as_str(), message.ts.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
    };
    let newest = match &current {
        None => true,
        Some((_, true, _)) => false,
        Some((current_edit, false, updated_at)) => {
            same.replaces(edit_ts, current_edit, *updated_at)
        }
    };
    // Avoid duplicating untouched originals in `message_revision` (cuts archive size ~65%):
    // whenever a message is edited, tombstoned, or re-observed with a different revision key,
    // archive the existing `message_current` row first and append the incoming revision.
    let needs_revision = match &current {
        None => false,
        Some((_, true, _)) => true,
        Some((current_edit, false, _)) => current_edit.as_str() != edit_ts,
    };
    let write = MessageWrite {
        workspace,
        channel,
        message,
        edit_ts,
        deleted: false,
        json: &stored_json,
    };
    if needs_revision {
        if current.is_some() {
            archive_current_as_revision(tx, workspace, channel, &message.ts)?;
        }
        insert_revision(tx, &write)?;
    }
    if newest {
        write_current(tx, &write, current.is_some())?;
    }
    Ok(())
}

pub(super) type RawRow = (String, String, bool);
type RawRevisionRow = (String, bool, String);

pub(super) fn raw_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
    Ok((r.get(0)?, decode_json_value(r.get_ref(1)?)?, r.get(2)?))
}

pub(super) fn decode_stored_message(raw: RawRow) -> Result<StoredMessage> {
    let (channel, json, deleted) = raw;
    Ok(stored_message(
        channel,
        serde_json::from_str(&json)?,
        deleted,
    ))
}

/// [`decode_stored_message`] for reads that return many messages: a row whose JSON no longer
/// matches [`Message`] (a field type tightened by an upgrade, say) is logged and skipped, so one
/// stale row cannot fail a whole channel, digest or search. [`Store::message`] stays strict.
pub(super) fn decode_or_skip(raw: RawRow) -> Option<StoredMessage> {
    let (channel, json, deleted) = raw;
    match serde_json::from_str(&json) {
        Ok(message) => Some(stored_message(channel, message, deleted)),
        Err(error) => {
            tracing::warn!(%error, %channel, "skipping stored message that no longer decodes");
            None
        }
    }
}

fn stored_message(channel: String, mut message: Message, deleted: bool) -> StoredMessage {
    let channel = ChannelId::from(channel);
    message.channel.get_or_insert_with(|| channel.clone());
    StoredMessage {
        channel,
        message,
        deleted,
    }
}

pub(super) fn collect(
    rows: impl Iterator<Item = rusqlite::Result<RawRow>>,
) -> Result<Vec<StoredMessage>> {
    let mut out = Vec::new();
    for row in rows {
        if let Some(message) = decode_or_skip(row?) {
            out.push(message);
        }
    }
    Ok(out)
}

fn raw_revision_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawRevisionRow> {
    Ok((
        r.get::<_, String>(0)?,
        r.get::<_, bool>(1)?,
        decode_json_value(r.get_ref(2)?)?,
    ))
}

fn decode_revision(raw: RawRevisionRow) -> Result<Revision> {
    let (edit_ts, deleted, json) = raw;
    Ok(Revision {
        edit_ts,
        deleted,
        message: serde_json::from_str(&json)?,
    })
}

/// Strictly parses a Slack timestamp (`"<secs>[.<frac>]"`) into a microsecond sort key, returning
/// `None` when the seconds component is empty or non-numeric (avoiding combining an invalid
/// seconds prefix with a numeric fraction). A malformed fractional part falls back to `0` micros.
pub(super) fn parse_ts_key(ts: &Ts) -> Option<i64> {
    let raw = ts.as_str();
    let (secs_str, frac) = raw.split_once('.').unwrap_or((raw, ""));
    if secs_str.is_empty() || !secs_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let secs: i64 = secs_str.parse().ok()?;
    let mut micros: i64 = 0;
    let mut digits = 0u32;
    for ch in frac.chars().take(6) {
        if let Some(d) = ch.to_digit(10) {
            micros = micros * 10 + i64::from(d);
            digits += 1;
        } else {
            micros = 0;
            digits = 6;
            break;
        }
    }
    while digits < 6 {
        micros *= 10;
        digits += 1;
    }
    Some(secs.saturating_mul(1_000_000).saturating_add(micros))
}

/// Numeric sort key: microseconds since the epoch (`"1700000000.000100"` -> 1700000000000100).
/// Unparseable timestamps sort at epoch `0`.
pub(super) fn ts_key(ts: &Ts) -> i64 {
    parse_ts_key(ts).unwrap_or(0)
}

pub(super) fn now_ts() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:06}", d.as_secs(), d.subsec_micros())
}
