//! Indexed read-only queries powering Local Catch-Up / Daily Briefing.

use super::*;
use crate::types::FileId;

/// One recently transcribed audio/video file clip with optional carrying-message coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentFileTranscript {
    pub file_id: FileId,
    pub source: String,
    pub text: String,
    pub updated_at: i64,
    pub channel: Option<ChannelId>,
    pub message_ts: Option<Ts>,
    pub title: Option<String>,
}

impl Store {
    /// Up to `limit` newest non-deleted messages in `workspace` with `ts_key >= cutoff_secs * 1_000_000`,
    /// ordered newest first (backed by V8 index `message_current_ws_ts`).
    pub fn messages_since(
        &self,
        workspace: &TeamId,
        cutoff_secs: i64,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let cutoff_key = cutoff_secs.saturating_mul(1_000_000);
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, json, deleted FROM message_current
             WHERE workspace_id = ?1 AND ts_key >= ?2 AND deleted = 0
             ORDER BY ts_key DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![workspace.as_str(), cutoff_key, limit as i64],
            raw_row,
        )?;
        collect(rows)
    }

    /// Batch-fetch stored non-deleted messages by `(channel_id, ts)` inside a single read snapshot.
    pub fn messages_by_keys(
        &self,
        workspace: &TeamId,
        keys: &[(ChannelId, Ts)],
    ) -> Result<HashMap<(ChannelId, Ts), StoredMessage>> {
        if keys.is_empty() {
            return Ok(HashMap::new());
        }
        self.with_read_tx(|tx| {
            let mut stmt = tx.prepare_cached(
                "SELECT channel_id, json, deleted FROM message_current
                 WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3 AND deleted = 0",
            )?;
            let ws = workspace.as_str();
            let mut out = HashMap::with_capacity(keys.len());
            for (ch, ts) in keys {
                if let Some(message) = stmt
                    .query_row(params![ws, ch.as_str(), ts.as_str()], raw_row)
                    .optional()?
                    .and_then(decode_or_skip)
                {
                    out.insert((ch.clone(), ts.clone()), message);
                }
            }
            Ok(out)
        })
    }

    /// Recently updated audio/video file transcripts (`updated_at >= cutoff_secs`), newest first,
    /// joined with the newest carrying message when linked via `message_file`.
    pub fn recent_file_transcripts_since(
        &self,
        workspace: &TeamId,
        cutoff_secs: i64,
        limit: usize,
    ) -> Result<Vec<RecentFileTranscript>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT ft.file_id, ft.source, ft.text, ft.updated_at,
                    (SELECT mc.channel_id
                     FROM message_file mf
                     JOIN message_current mc ON mc.id = mf.message_id
                     WHERE mf.workspace_id = ft.workspace_id AND mf.file_id = ft.file_id
                       AND mc.deleted = 0
                     ORDER BY mc.ts_key DESC LIMIT 1),
                    (SELECT mc.ts
                     FROM message_file mf
                     JOIN message_current mc ON mc.id = mf.message_id
                     WHERE mf.workspace_id = ft.workspace_id AND mf.file_id = ft.file_id
                       AND mc.deleted = 0
                     ORDER BY mc.ts_key DESC LIMIT 1),
                    (SELECT mc.file_names
                     FROM message_file mf
                     JOIN message_current mc ON mc.id = mf.message_id
                     WHERE mf.workspace_id = ft.workspace_id AND mf.file_id = ft.file_id
                       AND mc.deleted = 0
                     ORDER BY mc.ts_key DESC LIMIT 1)
             FROM file_transcript ft
             WHERE ft.workspace_id = ?1 AND ft.updated_at >= ?2
             ORDER BY ft.updated_at DESC, ft.file_id ASC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![workspace.as_str(), cutoff_secs, limit as i64],
            |r| {
                let fid: String = r.get(0)?;
                let source: String = r.get(1)?;
                let text: String = r.get(2)?;
                let updated_at: i64 = r.get(3)?;
                let channel: Option<String> = r.get(4)?;
                let message_ts: Option<String> = r.get(5)?;
                let file_names: Option<String> = r.get(6)?;
                let title = file_names
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty());
                Ok(RecentFileTranscript {
                    file_id: FileId::from(fid),
                    source,
                    text,
                    updated_at,
                    channel: channel.map(ChannelId::from),
                    message_ts: message_ts.map(Ts::from),
                    title,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Recently recorded huddle transcript sessions whose latest spoken segment is at or after
    /// `cutoff_secs` (unix seconds), newest first.
    pub fn recent_huddle_sessions_since(
        &self,
        workspace: &TeamId,
        cutoff_secs: i64,
        limit: usize,
    ) -> Result<Vec<HuddleTranscriptSession>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let cutoff_ms = cutoff_secs.saturating_mul(1_000);
        let mut stmt = self.conn.prepare_cached(
            "SELECT call_id,
                    channel_id,
                    MAX(thread_root_ts),
                    MIN(at_ms),
                    MAX(at_ms),
                    COUNT(*),
                    COALESCE(GROUP_CONCAT(DISTINCT LOWER(SUBSTR(lang, 1, 2))), ''),
                    (SELECT t2.text FROM huddle_transcript t2
                     WHERE t2.workspace_id = t.workspace_id AND t2.call_id = t.call_id
                     ORDER BY t2.at_ms ASC, t2.id ASC LIMIT 1)
             FROM huddle_transcript t
             WHERE workspace_id = ?1
             GROUP BY call_id, channel_id
             HAVING MAX(at_ms) >= ?2
             ORDER BY MAX(at_ms) DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![workspace.as_str(), cutoff_ms, limit as i64],
            transcripts::row_to_session,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}
