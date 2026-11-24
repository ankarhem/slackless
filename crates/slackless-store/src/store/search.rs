//! FTS5 search, channel memberships, storage stats/vacuum, bot profiles, and offline queue.

use super::*;

impl Store {
    /// Splits `members` into Slack's header counts (see [`MemberCounts`]). The workspace's
    /// built-in `USLACKBOT` is neither a human nor an app; a deactivated member is neither.
    pub fn member_counts(&self, workspace: &TeamId, members: &[UserId]) -> Result<MemberCounts> {
        /// Slack's built-in workspace bot.
        const USLACKBOT: &str = "USLACKBOT";
        let ask: Vec<&UserId> = members
            .iter()
            .filter(|id| id.as_str() != USLACKBOT)
            .collect();
        let mut counts = MemberCounts {
            humans: ask.len() as u32,
            apps: 0,
        };
        if ask.is_empty() {
            return Ok(counts);
        }
        // Everyone starts as a human (unknown identities included); the stored profiles then
        // re-classify the bots and drop the deactivated.
        for chunk in ask.chunks(SQL_IN_CHUNK) {
            let sql = format!(
                r#"SELECT id, is_bot, deleted FROM "user" WHERE workspace_id = ?1 AND id IN ({})"#,
                vec!["?"; chunk.len()].join(", ")
            );
            let mut args = Vec::with_capacity(chunk.len() + 1);
            args.push(SqlValue::Text(workspace.as_str().to_owned()));
            args.extend(
                chunk
                    .iter()
                    .map(|id| SqlValue::Text(id.as_str().to_owned())),
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(args), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, bool>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            })?;
            for row in rows {
                let (_, is_bot, deleted) = row?;
                if deleted {
                    counts.humans -= 1;
                } else if is_bot {
                    counts.humans -= 1;
                    counts.apps += 1;
                }
            }
        }
        Ok(counts)
    }

    /// Which of `channels` are stored with `is_member = 1` (one query per chunk): the boot hydrate
    /// preserves these flags, because `client.userBoot` objects omit `is_member` and the
    /// membership sync (`users.conversations`) only runs after them.
    pub fn member_flags(
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
                "SELECT id FROM conversation
                 WHERE workspace_id = ?1 AND is_member = 1 AND id IN ({})",
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

    /// Records the signed-in user's channel memberships (`users.conversations`): the ids in
    /// `members` become `is_member = 1`, every other stored public/private channel `0`, the
    /// `json` copy of the flag included. One transaction; rows that already agree are not
    /// rewritten. Returns the ids whose flag flipped, in both directions, so callers announce
    /// exactly the changed rows. IM/MPIM rows are never touched: their flag is not channel
    /// membership.
    pub fn set_channel_memberships(
        &mut self,
        workspace: &TeamId,
        members: &[ChannelId],
    ) -> Result<Vec<ChannelId>> {
        /// Only channel rows carry a membership flag worth syncing.
        const CHANNEL_KINDS: &str = "('public_channel', 'private_channel')";
        let mut flipped = Vec::new();
        let tx = self.conn.transaction()?;
        let now = now_secs();
        if members.len() <= SQL_IN_CHUNK {
            if !members.is_empty() {
                let sql = format!(
                    "UPDATE conversation
                     SET is_member = 1, json = json_set(json, '$.is_member', json('true')),
                         updated_at = ?1
                     WHERE workspace_id = ?2 AND kind IN {CHANNEL_KINDS} AND is_member = 0
                       AND id IN ({}) RETURNING id",
                    vec!["?"; members.len()].join(", ")
                );
                let mut args = vec![
                    SqlValue::Integer(now),
                    SqlValue::Text(workspace.as_str().to_owned()),
                ];
                args.extend(
                    members
                        .iter()
                        .map(|id| SqlValue::Text(id.as_str().to_owned())),
                );
                flipped.extend(flipping(&tx, &sql, args)?);
            }
            let left_out = if !members.is_empty() {
                format!(" AND id NOT IN ({})", vec!["?"; members.len()].join(", "))
            } else {
                String::new()
            };
            let sql = format!(
                "UPDATE conversation
                 SET is_member = 0, json = json_set(json, '$.is_member', json('false')),
                     updated_at = ?1
                 WHERE workspace_id = ?2 AND kind IN {CHANNEL_KINDS} AND is_member = 1{left_out}
                 RETURNING id"
            );
            let mut args = vec![
                SqlValue::Integer(now),
                SqlValue::Text(workspace.as_str().to_owned()),
            ];
            args.extend(
                members
                    .iter()
                    .map(|id| SqlValue::Text(id.as_str().to_owned())),
            );
            flipped.extend(flipping(&tx, &sql, args)?);
        } else {
            tx.execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS _slackless_member_ids (id TEXT PRIMARY KEY) WITHOUT ROWID;
                 DELETE FROM _slackless_member_ids;",
            )?;
            {
                let mut ins =
                    tx.prepare("INSERT OR IGNORE INTO _slackless_member_ids (id) VALUES (?1)")?;
                for id in members {
                    ins.execute(params![id.as_str()])?;
                }
            }
            let sql_join = format!(
                "UPDATE conversation
                 SET is_member = 1, json = json_set(json, '$.is_member', json('true')),
                     updated_at = ?1
                 WHERE workspace_id = ?2 AND kind IN {CHANNEL_KINDS} AND is_member = 0
                   AND id IN (SELECT id FROM _slackless_member_ids)
                 RETURNING id"
            );
            flipped.extend(flipping(
                &tx,
                &sql_join,
                vec![
                    SqlValue::Integer(now),
                    SqlValue::Text(workspace.as_str().to_owned()),
                ],
            )?);
            let sql_leave = format!(
                "UPDATE conversation
                 SET is_member = 0, json = json_set(json, '$.is_member', json('false')),
                     updated_at = ?1
                 WHERE workspace_id = ?2 AND kind IN {CHANNEL_KINDS} AND is_member = 1
                   AND id NOT IN (SELECT id FROM _slackless_member_ids)
                 RETURNING id"
            );
            flipped.extend(flipping(
                &tx,
                &sql_leave,
                vec![
                    SqlValue::Integer(now),
                    SqlValue::Text(workspace.as_str().to_owned()),
                ],
            )?);
            tx.execute("DELETE FROM _slackless_member_ids", [])?;
        }
        tx.commit()?;
        Ok(flipped)
    }

    /// Local search: free text via FTS5 MATCH, modifiers as WHERE clauses. Newest first;
    /// deleted messages are excluded. `in:`/`from:` match an id or a stored name.
    pub fn search(
        &self,
        workspace: &TeamId,
        query: &SearchQuery,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        self.search_sorted(workspace, query, crate::query::SearchSort::Recent, limit)
    }

    /// Local search with explicit sort order (`Recent` = newest first; `Relevant` = FTS5 BM25
    /// relevance when free text is present, breaking ties by newest first).
    pub fn search_sorted(
        &self,
        workspace: &TeamId,
        query: &SearchQuery,
        sort: crate::query::SearchSort,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let f = &query.filters;
        let fts = query.fts_match();
        let rank_relevant = sort == crate::query::SearchSort::Relevant && fts.is_some();
        let mut sql = if rank_relevant {
            String::from(
                "SELECT m.channel_id, m.json, m.deleted FROM message_current m
                 JOIN message_fts ON message_fts.rowid = m.id
                 WHERE m.workspace_id = ? AND m.deleted = 0 AND message_fts MATCH ?",
            )
        } else {
            String::from(
                "SELECT m.channel_id, m.json, m.deleted FROM message_current m
                 WHERE m.workspace_id = ? AND m.deleted = 0",
            )
        };
        let mut args = vec![SqlValue::Text(workspace.as_str().to_owned())];
        if let Some(fts) = fts {
            if rank_relevant {
                args.push(SqlValue::Text(fts));
            } else {
                sql.push_str(
                    " AND m.id IN (SELECT rowid FROM message_fts WHERE message_fts MATCH ?)",
                );
                args.push(SqlValue::Text(fts));
            }
        } else if let Some(excluded_fts) = query.fts_excluded_match() {
            sql.push_str(
                " AND m.id NOT IN (SELECT rowid FROM message_fts WHERE message_fts MATCH ?)",
            );
            args.push(SqlValue::Text(excluded_fts));
        }
        any_of(
            &mut sql,
            &mut args,
            &f.in_channels,
            "m.channel_id = ?1 OR m.channel_id IN (SELECT id FROM conversation
                 WHERE workspace_id = m.workspace_id AND name = ?1 COLLATE NOCASE)",
        );
        none_of(
            &mut sql,
            &mut args,
            &f.exclude_channels,
            "m.channel_id = ?1 OR m.channel_id IN (SELECT id FROM conversation
                 WHERE workspace_id = m.workspace_id AND name = ?1 COLLATE NOCASE)",
        );
        any_of(
            &mut sql,
            &mut args,
            &f.from_users,
            r#"m.user_id = ?1 OR m.user_id IN (SELECT id FROM "user"
                 WHERE workspace_id = m.workspace_id AND (name = ?1 COLLATE NOCASE
                   OR display_name = ?1 COLLATE NOCASE OR real_name = ?1 COLLATE NOCASE))"#,
        );
        none_of(
            &mut sql,
            &mut args,
            &f.exclude_users,
            r#"m.user_id = ?1 OR m.user_id IN (SELECT id FROM "user"
                 WHERE workspace_id = m.workspace_id AND (name = ?1 COLLATE NOCASE
                   OR display_name = ?1 COLLATE NOCASE OR real_name = ?1 COLLATE NOCASE))"#,
        );
        if let Some(before) = f.before {
            sql.push_str(" AND m.created_at < ?");
            args.push(SqlValue::Integer(before.unix_start()));
        }
        if let Some(after) = f.after {
            sql.push_str(" AND m.created_at >= ?");
            args.push(SqlValue::Integer(after.unix_start() + 86_400));
        }
        for has in &f.has {
            sql.push_str(match has {
                Has::Link => " AND m.has_link = 1",
                Has::File => " AND m.has_file = 1",
                Has::Pin => " AND m.is_pinned = 1",
                Has::Reaction => {
                    " AND EXISTS (SELECT 1 FROM reaction r WHERE r.workspace_id = m.workspace_id
                       AND r.channel_id = m.channel_id AND r.ts = m.ts)"
                }
            });
        }
        if let Some(user) = &f.includes_user {
            sql.push_str(" AND instr(m.text, ?) > 0");
            args.push(SqlValue::Text(format!("<@{user}>")));
        }
        if f.threads_only {
            sql.push_str(" AND m.thread_ts IS NOT NULL");
        }
        if rank_relevant {
            sql.push_str(" ORDER BY bm25(message_fts), m.ts_key DESC LIMIT ?");
        } else {
            sql.push_str(" ORDER BY m.ts_key DESC LIMIT ?");
        }
        args.push(SqlValue::Integer(limit as i64));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), raw_row)?;
        collect(rows)
    }

    /// Summary statistics for `workspace` and overall SQLite database footprint.
    pub fn stats(&self, workspace: &TeamId) -> Result<StoreStats> {
        let ws = workspace.as_str();
        let messages: i64 = self.conn.query_row(
            "SELECT count(*) FROM message_current WHERE workspace_id = ?1",
            params![ws],
            |r| r.get(0),
        )?;
        let rev_rows: i64 = self.conn.query_row(
            "SELECT count(*) FROM message_revision WHERE workspace_id = ?1",
            params![ws],
            |r| r.get(0),
        )?;
        let revised_msgs: i64 = self.conn.query_row(
            "SELECT count(*) FROM (
                 SELECT DISTINCT channel_id, ts FROM message_revision WHERE workspace_id = ?1
             )",
            params![ws],
            |r| r.get(0),
        )?;
        let revisions = (messages - revised_msgs).max(0) + rev_rows;
        let conversations: i64 = self.conn.query_row(
            "SELECT count(*) FROM conversation WHERE workspace_id = ?1",
            params![ws],
            |r| r.get(0),
        )?;
        let users: i64 = self.conn.query_row(
            "SELECT count(*) FROM \"user\" WHERE workspace_id = ?1",
            params![ws],
            |r| r.get(0),
        )?;
        let offline_queued: i64 = self.conn.query_row(
            "SELECT count(*) FROM offline_queue WHERE workspace_id = ?1",
            params![ws],
            |r| r.get(0),
        )?;
        let page_count: i64 = self
            .conn
            .pragma_query_value(None, "page_count", |r| r.get(0))?;
        let page_size: i64 = self
            .conn
            .pragma_query_value(None, "page_size", |r| r.get(0))?;
        Ok(StoreStats {
            messages: messages.max(0) as u64,
            revisions: revisions.max(0) as u64,
            conversations: conversations.max(0) as u64,
            users: users.max(0) as u64,
            offline_queued: offline_queued.max(0) as u64,
            page_count: page_count.max(0) as u64,
            page_size: page_size.max(0) as u64,
        })
    }

    /// Detailed per-table counts and the top `top_n` channels by archived message count.
    pub fn storage_breakdown(&self, workspace: &TeamId, top_n: usize) -> Result<StorageBreakdown> {
        let ws = workspace.as_str();
        let table_queries = [
            (
                "message_current",
                "SELECT count(*) FROM message_current WHERE workspace_id = ?1",
            ),
            (
                "message_revision",
                "SELECT count(*) FROM message_revision WHERE workspace_id = ?1",
            ),
            (
                "reaction",
                "SELECT count(*) FROM reaction WHERE workspace_id = ?1",
            ),
            (
                "conversation",
                "SELECT count(*) FROM conversation WHERE workspace_id = ?1",
            ),
            (
                "user",
                "SELECT count(*) FROM \"user\" WHERE workspace_id = ?1",
            ),
            (
                "custom_emoji",
                "SELECT count(*) FROM custom_emoji WHERE workspace_id = ?1",
            ),
            (
                "offline_queue",
                "SELECT count(*) FROM offline_queue WHERE workspace_id = ?1",
            ),
        ];
        let mut tables = Vec::with_capacity(table_queries.len());
        for (name, sql) in table_queries {
            let count: i64 = self.conn.query_row(sql, params![ws], |r| r.get(0))?;
            tables.push((name.to_owned(), count.max(0) as u64));
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.channel_id, c.name, count(*) AS cnt
             FROM message_current m
             LEFT JOIN conversation c
               ON c.workspace_id = m.workspace_id AND c.id = m.channel_id
             WHERE m.workspace_id = ?1
             GROUP BY m.channel_id
             ORDER BY cnt DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![ws, top_n as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?.max(0) as u64,
            ))
        })?;
        let mut top_channels = Vec::new();
        for row in rows {
            top_channels.push(row?);
        }
        Ok(StorageBreakdown {
            tables,
            top_channels,
        })
    }

    /// Persist a resolved `bots.info` profile so cold launches render bot names and icons
    /// immediately on frame 0 without waiting on network lookups.
    pub fn upsert_bot_profile(
        &mut self,
        workspace: &TeamId,
        bot_id: &BotId,
        profile: &BotProfile,
    ) -> Result<()> {
        let json = serde_json::to_string(profile)?;
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO bot_profile (workspace_id, bot_id, json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (workspace_id, bot_id) DO UPDATE SET
                json = excluded.json,
                updated_at = excluded.updated_at",
        )?;
        stmt.execute(params![
            workspace.as_str(),
            bot_id.as_str(),
            json,
            now_secs()
        ])?;
        Ok(())
    }

    /// All persisted `bots.info` profiles for `workspace`.
    pub fn bot_profiles(&self, workspace: &TeamId) -> Result<HashMap<BotId, BotProfile>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT bot_id, json FROM bot_profile WHERE workspace_id = ?1")?;
        let rows = stmt.query_map(params![workspace.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, raw) = row?;
            if let Ok(profile) = serde_json::from_str::<BotProfile>(&raw) {
                out.insert(BotId::from(id), profile);
            }
        }
        Ok(out)
    }

    /// Compress any legacy uncompressed `message_current` JSON rows to zlib BLOBs, checkpoint
    /// WAL, compact unused pages (`VACUUM`), and refresh query planner statistics.
    pub fn vacuum(&self) -> Result<()> {
        // Compress legacy plain-TEXT message payloads in batches (skipping ephemeral rows so
        // `drop_ephemeral` can keep using `json_extract`). Because `V6` scopes `message_current_au`
        // to `text, file_names`, updating `json` alone never churns the FTS5 index.
        const BATCH: usize = 5_000;
        let mut last_id: i64 = 0;
        loop {
            let mut batch: Vec<(i64, Vec<u8>)> = Vec::with_capacity(BATCH);
            {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT id, json FROM message_current
                     WHERE id > ?1 AND typeof(json) = 'text' AND length(json) >= 160
                       AND COALESCE(json_extract(json, '$.is_ephemeral'), 0) = 0
                     ORDER BY id ASC LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![last_id, BATCH as i64], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })?;
                let mut seen = 0usize;
                for row in rows {
                    let (id, text) = row?;
                    last_id = id;
                    seen += 1;
                    if let SqlValue::Blob(blob) = encode_json_value(&text, false) {
                        batch.push((id, blob));
                    }
                }
                if seen == 0 {
                    break;
                }
            }
            if !batch.is_empty() {
                self.conn.execute_batch("BEGIN IMMEDIATE")?;
                let update_res = (|| -> Result<()> {
                    let mut upd = self
                        .conn
                        .prepare_cached("UPDATE message_current SET json = ?1 WHERE id = ?2")?;
                    for (id, blob) in &batch {
                        upd.execute(params![blob, id])?;
                    }
                    Ok(())
                })();
                match update_res {
                    Ok(()) => self.conn.execute_batch("COMMIT")?,
                    Err(e) => {
                        let _ = self.conn.execute_batch("ROLLBACK");
                        return Err(e);
                    }
                }
            }
        }
        self.conn.execute_batch(
            "PRAGMA wal_checkpoint(TRUNCATE);
             VACUUM;
             PRAGMA optimize;",
        )?;
        Ok(())
    }

    /// Append a serialized offline mutation payload for `workspace`, returning its row id.
    pub fn enqueue_offline(&self, workspace: &TeamId, payload: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO offline_queue (workspace_id, payload, created_at) VALUES (?1, ?2, ?3)",
            params![workspace.as_str(), payload, now_secs()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// List persisted offline mutations for `workspace` in FIFO order `(id, payload)`.
    pub fn list_offline(&self, workspace: &TeamId) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, payload FROM offline_queue WHERE workspace_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![workspace.as_str()], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Remove one persisted offline mutation by row id (after flush or user discard).
    pub fn delete_offline(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM offline_queue WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Remove all persisted offline mutations for `workspace`.
    pub fn clear_offline(&self, workspace: &TeamId) -> Result<()> {
        self.conn.execute(
            "DELETE FROM offline_queue WHERE workspace_id = ?1",
            params![workspace.as_str()],
        )?;
        Ok(())
    }
}

/// Appends ` AND (clause(v1) OR clause(v2) ...)`; `?1` in `clause` is the value placeholder.
pub(super) fn any_of(sql: &mut String, args: &mut Vec<SqlValue>, values: &[String], clause: &str) {
    if values.is_empty() {
        return;
    }
    let parts: Vec<String> = values
        .iter()
        .map(|v| {
            let occurrences = clause.matches("?1").count();
            args.extend((0..occurrences).map(|_| SqlValue::Text(v.clone())));
            format!("({})", clause.replace("?1", "?"))
        })
        .collect();
    sql.push_str(&format!(" AND ({})", parts.join(" OR ")));
}

/// Appends ` AND NOT (clause(v1) OR clause(v2) ...)`; `?1` in `clause` is the value placeholder.
pub(super) fn none_of(sql: &mut String, args: &mut Vec<SqlValue>, values: &[String], clause: &str) {
    if values.is_empty() {
        return;
    }
    let parts: Vec<String> = values
        .iter()
        .map(|v| {
            let occurrences = clause.matches("?1").count();
            args.extend((0..occurrences).map(|_| SqlValue::Text(v.clone())));
            format!("({})", clause.replace("?1", "?"))
        })
        .collect();
    sql.push_str(&format!(" AND NOT ({})", parts.join(" OR ")));
}
/// Runs one membership `UPDATE ... RETURNING id`, collecting the flipped ids.
pub(super) fn flipping(
    tx: &Transaction<'_>,
    sql: &str,
    args: Vec<SqlValue>,
) -> Result<Vec<ChannelId>> {
    let mut stmt = tx.prepare(sql)?;
    let rows = stmt.query_map(params_from_iter(args), |r| r.get::<_, String>(0))?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(ChannelId::from)
        .collect())
}
