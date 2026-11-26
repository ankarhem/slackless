//! Database statistics, storage breakdown, and compaction (`VACUUM`).

use super::*;

impl Store {
    /// Summary statistics for `workspace` and overall SQLite database footprint.
    pub fn stats(&self, workspace: &TeamId) -> Result<StoreStats> {
        let ws = workspace.as_str();
        self.with_read_tx(|tx| {
            let count = |sql: &str| -> Result<i64> {
                let mut stmt = tx.prepare_cached(sql)?;
                Ok(stmt.query_row(params![ws], |r| r.get(0))?)
            };
            let messages = count("SELECT count(*) FROM message_current WHERE workspace_id = ?1")?;
            let rev_rows = count("SELECT count(*) FROM message_revision WHERE workspace_id = ?1")?;
            let revised_msgs = count(
                "SELECT count(*) FROM (
                     SELECT DISTINCT channel_id, ts FROM message_revision WHERE workspace_id = ?1
                 )",
            )?;
            let revisions = (messages - revised_msgs).max(0) + rev_rows;
            let conversations = count("SELECT count(*) FROM conversation WHERE workspace_id = ?1")?;
            let users = count("SELECT count(*) FROM \"user\" WHERE workspace_id = ?1")?;
            let offline_queued =
                count("SELECT count(*) FROM offline_queue WHERE workspace_id = ?1")?
                    + count("SELECT count(*) FROM outbox WHERE workspace_id = ?1")?;
            let page_count: i64 = tx.pragma_query_value(None, "page_count", |r| r.get(0))?;
            let page_size: i64 = tx.pragma_query_value(None, "page_size", |r| r.get(0))?;
            Ok(StoreStats {
                messages: messages.max(0) as u64,
                revisions: revisions.max(0) as u64,
                conversations: conversations.max(0) as u64,
                users: users.max(0) as u64,
                offline_queued: offline_queued.max(0) as u64,
                page_count: page_count.max(0) as u64,
                page_size: page_size.max(0) as u64,
            })
        })
    }

    /// Detailed per-table counts and the top `top_n` channels by archived message count.
    pub fn storage_breakdown(&self, workspace: &TeamId, top_n: usize) -> Result<StorageBreakdown> {
        let ws = workspace.as_str();
        self.with_read_tx(|tx| {
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
                (
                    "outbox",
                    "SELECT count(*) FROM outbox WHERE workspace_id = ?1",
                ),
                (
                    "bot_profile",
                    "SELECT count(*) FROM bot_profile WHERE workspace_id = ?1",
                ),
                (
                    "huddle_transcript",
                    "SELECT count(*) FROM huddle_transcript WHERE workspace_id = ?1",
                ),
                (
                    "file_transcript",
                    "SELECT count(*) FROM file_transcript WHERE workspace_id = ?1",
                ),
                (
                    "message_file",
                    "SELECT count(*) FROM message_file WHERE workspace_id = ?1",
                ),
                (
                    "channel_section",
                    "SELECT count(*) FROM channel_section WHERE workspace_id = ?1",
                ),
                ("team", "SELECT count(*) FROM team WHERE workspace_id = ?1"),
                (
                    "prefs",
                    "SELECT count(*) FROM prefs WHERE workspace_id = ?1",
                ),
                (
                    "local_state",
                    "SELECT count(*) FROM local_state WHERE workspace_id = ?1",
                ),
            ];
            let mut tables = Vec::with_capacity(table_queries.len());
            for (name, sql) in table_queries {
                let mut stmt = tx.prepare_cached(sql)?;
                let count: i64 = stmt.query_row(params![ws], |r| r.get(0))?;
                tables.push((name.to_owned(), count.max(0) as u64));
            }
            let mut stmt = tx.prepare_cached(
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
        })
    }

    /// Compress any legacy uncompressed `message_current` JSON rows to zlib BLOBs, checkpoint
    /// WAL, compact unused pages (`VACUUM`), and refresh query planner statistics.
    pub fn vacuum(&mut self) -> Result<()> {
        self.ensure_writable()?;
        // Compress legacy plain-TEXT message payloads in batches (skipping ephemeral rows so
        // `drop_ephemeral` can keep using `json_extract`). Because `message_current_au` is scoped
        // to `text, file_names, file_transcripts`, updating `json` alone never churns FTS5.
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
                let tx = self
                    .conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                {
                    let mut upd =
                        tx.prepare_cached("UPDATE message_current SET json = ?1 WHERE id = ?2")?;
                    for (id, blob) in &batch {
                        upd.execute(params![blob, id])?;
                    }
                }
                tx.commit()?;
            }
        }
        self.conn.execute_batch(
            "PRAGMA wal_checkpoint(TRUNCATE);
             VACUUM;
             PRAGMA optimize;",
        )?;
        Ok(())
    }
}
