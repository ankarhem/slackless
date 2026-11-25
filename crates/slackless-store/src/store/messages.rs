//! Message upserts, deletions, history/thread queries, revisions, and zlib JSON storage.

use super::*;

impl Store {
    /// Records a message observed from history, replies, `chat.*` or the websocket.
    ///
    /// For a `message_changed` event pass its inner `message`: the edit is keyed by `edited.ts`
    /// and appended as a new revision. The current state follows the newest revision; once a
    /// message is deleted, later (stale) observations are logged but never resurrect it.
    pub fn upsert_message(
        &mut self,
        workspace: &TeamId,
        channel: &ChannelId,
        message: &Message,
    ) -> Result<()> {
        self.upsert_messages(workspace, channel, std::slice::from_ref(message))
    }

    /// Records a batch of messages for `channel` in a single SQLite transaction (one WAL commit and
    /// one revision bump for the whole page).
    pub fn upsert_messages(
        &mut self,
        workspace: &TeamId,
        channel: &ChannelId,
        messages: &[Message],
    ) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        for message in messages {
            upsert_message_in_tx(&tx, workspace, channel, message)?;
        }
        tx.commit()?;
        Self::bump_message_revision();
        Ok(())
    }

    /// Forgets the current state of ephemeral messages ("Only visible to you", like Giphy's
    /// preview): Slack keeps no copy, so one left from a past session would otherwise show
    /// forever. Their revisions stay in the log. Returns how many were dropped.
    pub fn drop_ephemeral(&mut self, workspace: &TeamId) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM message_current
             WHERE workspace_id = ?1 AND typeof(json) = 'text'
               AND json_extract(json, '$.is_ephemeral') = 1",
            params![workspace.as_str()],
        )?;
        if n > 0 {
            Self::bump_message_revision();
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
        let tx = self.conn.transaction()?;
        archive_current_as_revision(&tx, workspace, channel, deleted_ts)?;
        let mut prior = match previous {
            Some(m) => m.clone(),
            None => {
                let stored: Option<String> = tx
                    .query_row(
                        "SELECT json FROM message_current
                         WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
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
        insert_revision(
            &tx,
            workspace,
            channel,
            &prior,
            &edit_ts,
            true,
            &stored_json,
        )?;
        write_current(
            &tx,
            workspace,
            channel,
            &prior,
            &edit_ts,
            true,
            &stored_json,
        )?;
        tx.commit()?;
        Self::bump_message_revision();
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
    /// directly (`ORDER BY ts_key DESC LIMIT 1` per channel) so large DM histories are never
    /// scanned by a window function.
    pub fn latest_messages(
        &self,
        workspace: &TeamId,
        channels: &[ChannelId],
    ) -> Result<HashMap<ChannelId, StoredMessage>> {
        if channels.is_empty() {
            return Ok(HashMap::new());
        }
        let mut stmt = self.conn.prepare_cached(
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
            if let Some(raw) = row {
                let (ch, json, deleted) = raw;
                let ch_id = ChannelId::from(ch);
                let mut message: Message = serde_json::from_str(&json)?;
                message.channel.get_or_insert_with(|| ch_id.clone());
                out.insert(
                    channel.clone(),
                    StoredMessage {
                        channel: ch_id,
                        message,
                        deleted,
                    },
                );
            }
        }
        Ok(out)
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
        for chunk in channels.chunks(SQL_IN_CHUNK) {
            let sql = format!(
                "SELECT DISTINCT channel_id FROM message_current
                 WHERE workspace_id = ?1 AND channel_id IN ({})",
                vec!["?"; chunk.len()].join(", ")
            );
            let mut args = Vec::with_capacity(chunk.len() + 1);
            args.push(SqlValue::Text(workspace.as_str().to_owned()));
            args.extend(
                chunk
                    .iter()
                    .map(|channel| SqlValue::Text(channel.as_str().to_owned())),
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(args), |r| r.get::<_, String>(0))?;
            for row in rows {
                out.insert(ChannelId::from(row?));
            }
        }
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
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, bool>(1)?,
                    decode_json_value(r.get_ref(2)?)?,
                ))
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (edit_ts, deleted, json) = row?;
            out.push(Revision {
                edit_ts,
                deleted,
                message: serde_json::from_str(&json)?,
            });
        }
        if out.is_empty() {
            let current = self
                .conn
                .query_row(
                    "SELECT edit_ts, deleted, json FROM message_current
                     WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
                    params![workspace.as_str(), channel.as_str(), ts.as_str()],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, bool>(1)?,
                            decode_json_value(r.get_ref(2)?)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((edit_ts, deleted, json)) = current {
                out.push(Revision {
                    edit_ts,
                    deleted,
                    message: serde_json::from_str(&json)?,
                });
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
        match channel {
            Some(ch) => {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT channel_id, json, deleted FROM message_current
                     WHERE workspace_id = ?1 AND channel_id = ?2 AND deleted = 0
                       AND COALESCE(reply_count, 0) > 0
                     ORDER BY ts_key DESC LIMIT ?3",
                )?;
                let rows = stmt.query_map(
                    params![workspace.as_str(), ch.as_str(), limit as i64],
                    raw_row,
                )?;
                collect(rows)
            }
            None => {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT channel_id, json, deleted FROM message_current
                     WHERE workspace_id = ?1 AND deleted = 0
                       AND COALESCE(reply_count, 0) > 0
                     ORDER BY ts_key DESC LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![workspace.as_str(), limit as i64], raw_row)?;
                collect(rows)
            }
        }
    }
}

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
pub(super) fn decode_json_value(val: ValueRef<'_>) -> rusqlite::Result<String> {
    match val {
        ValueRef::Text(bytes) => std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(rusqlite::Error::Utf8Error),
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
pub(super) fn archive_current_as_revision(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    channel: &ChannelId,
    ts: &Ts,
) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO message_revision
             (workspace_id, channel_id, ts, edit_ts, user_id, thread_ts, subtype, text, deleted,
              json, created_at, recorded_at)
         SELECT workspace_id, channel_id, ts, edit_ts, user_id, thread_ts, subtype, text, deleted,
                json, created_at, updated_at
         FROM message_current
         WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
        params![workspace.as_str(), channel.as_str(), ts.as_str()],
    )?;
    Ok(())
}

pub(super) fn insert_revision(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    channel: &ChannelId,
    message: &Message,
    edit_ts: &str,
    deleted: bool,
    json: &SqlValue,
) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO message_revision
             (workspace_id, channel_id, ts, edit_ts, user_id, thread_ts, subtype, text, deleted,
              json, created_at, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            workspace.as_str(),
            channel.as_str(),
            message.ts.as_str(),
            edit_ts,
            message.user.as_ref().map(|u| u.as_str()),
            message.thread_ts.as_ref().map(|t| t.as_str()),
            message.subtype.as_ref().map(|s| s.as_str()),
            message.text,
            deleted,
            json,
            message.ts.secs(),
            now_secs(),
        ],
    )?;
    Ok(())
}

pub(super) fn write_current(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    channel: &ChannelId,
    message: &Message,
    edit_ts: &str,
    deleted: bool,
    json: &SqlValue,
) -> Result<()> {
    let file_names = message
        .files
        .iter()
        .flat_map(|f| [f.name.as_deref(), f.title.as_deref()])
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let has_link = message.text.contains("http://")
        || message.text.contains("https://")
        || message
            .attachments
            .iter()
            .any(|a| a.from_url.is_some() || a.title_link.is_some());
    tx.execute(
        "INSERT INTO message_current
             (workspace_id, channel_id, ts, ts_key, edit_ts, user_id, thread_ts, subtype, text,
              file_names, deleted, has_link, has_file, is_pinned, reply_count, json, created_at,
              updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
         ON CONFLICT (workspace_id, channel_id, ts) DO UPDATE SET
             ts_key = excluded.ts_key, edit_ts = excluded.edit_ts, user_id = excluded.user_id,
             thread_ts = excluded.thread_ts, subtype = excluded.subtype, text = excluded.text,
             file_names = excluded.file_names, deleted = excluded.deleted,
             has_link = excluded.has_link, has_file = excluded.has_file,
             is_pinned = excluded.is_pinned, reply_count = excluded.reply_count,
             json = excluded.json, created_at = excluded.created_at,
             updated_at = excluded.updated_at",
        params![
            workspace.as_str(),
            channel.as_str(),
            message.ts.as_str(),
            ts_key(&message.ts),
            edit_ts,
            message.user.as_ref().map(|u| u.as_str()),
            message.thread_ts.as_ref().map(|t| t.as_str()),
            message.subtype.as_ref().map(|s| s.as_str()),
            message.text,
            file_names,
            deleted,
            has_link,
            !message.files.is_empty(),
            !message.pinned_to.is_empty(),
            message.reply_count,
            json,
            message.ts.secs(),
            now_secs(),
        ],
    )?;
    tx.execute(
        "DELETE FROM reaction WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
        params![workspace.as_str(), channel.as_str(), message.ts.as_str()],
    )?;
    for reaction in &message.reactions {
        tx.execute(
            "INSERT OR REPLACE INTO reaction (workspace_id, channel_id, ts, name, count, users)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                workspace.as_str(),
                channel.as_str(),
                message.ts.as_str(),
                reaction.name,
                reaction.count,
                serde_json::to_string(&reaction.users)?,
            ],
        )?;
    }
    Ok(())
}

/// Maximum number of bind parameters per SQL `IN (?, ...)` chunk (well below SQLite's limit).
pub(super) const SQL_IN_CHUNK: usize = 500;

pub(super) fn upsert_message_in_tx(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    channel: &ChannelId,
    message: &Message,
) -> Result<()> {
    let edit_ts = message.edited.as_ref().map_or("", |e| e.ts.as_str());
    let json = serde_json::to_string(message)?;
    let ephemeral = is_ephemeral_message(message);
    let stored_json = encode_json_value(&json, ephemeral);
    let current: Option<(String, bool)> = tx
        .query_row(
            "SELECT edit_ts, deleted FROM message_current
             WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
            params![workspace.as_str(), channel.as_str(), message.ts.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let newest = match &current {
        None => true,
        Some((_, true)) => false,
        Some((current_edit, false)) => Ts::from(edit_ts) >= Ts::from(current_edit.as_str()),
    };
    // Avoid duplicating untouched originals in `message_revision` (cuts archive size ~65%):
    // whenever a message is edited, tombstoned, or re-observed with a different revision key,
    // archive the existing `message_current` row first and append the incoming revision.
    let needs_revision = match &current {
        None => false,
        Some((_, true)) => true,
        Some((current_edit, false)) => current_edit.as_str() != edit_ts,
    };
    if needs_revision {
        if current.is_some() {
            archive_current_as_revision(tx, workspace, channel, &message.ts)?;
        }
        insert_revision(
            tx,
            workspace,
            channel,
            message,
            edit_ts,
            false,
            &stored_json,
        )?;
    }
    if newest {
        write_current(
            tx,
            workspace,
            channel,
            message,
            edit_ts,
            false,
            &stored_json,
        )?;
    }
    Ok(())
}

pub(super) type RawRow = (String, String, bool);

pub(super) fn raw_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
    Ok((r.get(0)?, decode_json_value(r.get_ref(1)?)?, r.get(2)?))
}

pub(super) fn collect(
    rows: impl Iterator<Item = rusqlite::Result<RawRow>>,
) -> Result<Vec<StoredMessage>> {
    let mut out = Vec::new();
    for row in rows {
        let (channel, json, deleted) = row?;
        let channel = ChannelId::from(channel);
        let mut message: Message = serde_json::from_str(&json)?;
        message.channel.get_or_insert_with(|| channel.clone());
        out.push(StoredMessage {
            channel,
            message,
            deleted,
        });
    }
    Ok(out)
}

/// Numeric sort key: microseconds since the epoch (`"1700000000.000100"` -> 1700000000000100).
pub(super) fn ts_key(ts: &Ts) -> i64 {
    let (secs, frac) = ts.as_str().split_once('.').unwrap_or((ts.as_str(), ""));
    let secs: i64 = secs.parse().unwrap_or(0);
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
    secs.saturating_mul(1_000_000).saturating_add(micros)
}

pub(super) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

pub(super) fn now_ts() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:06}", d.as_secs(), d.subsec_micros())
}
