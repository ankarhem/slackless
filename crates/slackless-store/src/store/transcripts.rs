//! Text-only searchable huddle transcripts stored in SQLite + FTS5 (`huddle_transcript`).

use super::*;

/// One finalized spoken utterance persisted from a huddle (Chime live captions or opt-in
/// local multilingual Whisper transcription).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HuddleTranscriptSegment {
    pub id: i64,
    pub channel: ChannelId,
    pub call_id: String,
    pub thread_root_ts: Option<Ts>,
    pub speaker_id: Option<UserId>,
    /// BCP-47 or ISO-639-1 language tag (e.g. `"en"`, `"sv"`, `"en-US"`, `"sv-SE"`).
    pub lang: Option<String>,
    /// Origin: `"chime"` (Slack/AWS Transcribe) or `"local-whisper"`.
    pub source: String,
    pub text: String,
    pub at_ms: i64,
}

/// A huddle transcript segment attributed to its workspace (for cross-workspace search).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceHuddleTranscriptSegment {
    pub workspace: TeamId,
    pub segment: HuddleTranscriptSegment,
}

/// Summary of one recorded huddle transcript session (grouped by `call_id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HuddleTranscriptSession {
    pub call_id: String,
    pub channel: ChannelId,
    pub thread_root_ts: Option<Ts>,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub segment_count: u64,
    /// Distinct normalized language codes observed in this huddle (e.g. `["en", "sv"]`).
    pub languages: Vec<String>,
    /// First non-empty utterance snippet for list previews.
    pub preview: String,
}

/// Input parameters for inserting a single spoken transcript segment.
#[derive(Debug, Clone)]
pub struct NewHuddleTranscriptSegment<'a> {
    pub channel: &'a ChannelId,
    pub call_id: &'a str,
    pub thread_root_ts: Option<&'a Ts>,
    pub speaker_id: Option<&'a UserId>,
    pub lang: Option<&'a str>,
    pub source: &'a str,
    pub text: &'a str,
    pub at_ms: i64,
}

const SESSION_SELECT_PREFIX: &str = "SELECT call_id,
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
 WHERE workspace_id = ?1";

const SESSION_GROUP_SUFFIX: &str = " GROUP BY call_id, channel_id
 ORDER BY MAX(at_ms) DESC";

fn row_to_segment(row: &rusqlite::Row<'_>) -> rusqlite::Result<HuddleTranscriptSegment> {
    let channel_s: String = row.get(1)?;
    let thread_s: Option<String> = row.get(3)?;
    let speaker_s: Option<String> = row.get(4)?;
    Ok(HuddleTranscriptSegment {
        id: row.get(0)?,
        channel: ChannelId::from(channel_s),
        call_id: row.get(2)?,
        thread_root_ts: thread_s.filter(|s| !s.is_empty()).map(Ts::from),
        speaker_id: speaker_s.filter(|s| !s.is_empty()).map(UserId::from),
        lang: row.get(5)?,
        source: row.get(6)?,
        text: row.get(7)?,
        at_ms: row.get(8)?,
    })
}

pub(super) fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<HuddleTranscriptSession> {
    let call_id: String = row.get(0)?;
    let ch: String = row.get(1)?;
    let thread_s: Option<String> = row.get(2)?;
    let started_at_ms: i64 = row.get(3)?;
    let ended_at_ms: i64 = row.get(4)?;
    let segment_count: i64 = row.get(5)?;
    let langs_csv: String = row.get(6)?;
    let preview: Option<String> = row.get(7)?;
    let mut languages: Vec<String> = langs_csv
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    languages.sort();
    languages.dedup();
    Ok(HuddleTranscriptSession {
        call_id,
        channel: ChannelId::from(ch),
        thread_root_ts: thread_s.filter(|s| !s.is_empty()).map(Ts::from),
        started_at_ms,
        ended_at_ms,
        segment_count: segment_count.max(0) as u64,
        languages,
        preview: preview.unwrap_or_default(),
    })
}

impl Store {
    /// Persist one finalized spoken huddle transcript segment (text-only; zero audio on disk).
    pub fn insert_huddle_transcript_segment(
        &mut self,
        workspace: &TeamId,
        seg: &NewHuddleTranscriptSegment<'_>,
    ) -> Result<i64> {
        self.ensure_writable()?;
        let trimmed = seg.text.trim();
        if trimmed.is_empty() {
            return Ok(0);
        }
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO huddle_transcript
                (workspace_id, channel_id, call_id, thread_root_ts, speaker_id, lang, source, text, at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        stmt.execute(params![
            workspace.as_str(),
            seg.channel.as_str(),
            seg.call_id,
            seg.thread_root_ts.map(Ts::as_str),
            seg.speaker_id.map(UserId::as_str),
            seg.lang,
            seg.source,
            trimmed,
            seg.at_ms,
        ])?;
        let id = self.conn.last_insert_rowid();
        Self::bump_message_revision();
        Ok(id)
    }

    /// Every persisted transcript segment for `call_id`, oldest first.
    pub fn huddle_transcript_for_call(
        &self,
        workspace: &TeamId,
        call_id: &str,
    ) -> Result<Vec<HuddleTranscriptSegment>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, channel_id, call_id, thread_root_ts, speaker_id, lang, source, text, at_ms
             FROM huddle_transcript
             WHERE workspace_id = ?1 AND call_id = ?2
             ORDER BY at_ms ASC, id ASC",
        )?;
        let rows = stmt
            .query_map(params![workspace.as_str(), call_id], row_to_segment)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Every persisted transcript segment linked to a `huddle_thread` message (`(channel, ts)`),
    /// oldest first.
    pub fn huddle_transcript_for_thread(
        &self,
        workspace: &TeamId,
        channel: &ChannelId,
        thread_root_ts: &Ts,
    ) -> Result<Vec<HuddleTranscriptSegment>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, channel_id, call_id, thread_root_ts, speaker_id, lang, source, text, at_ms
             FROM huddle_transcript
             WHERE workspace_id = ?1 AND channel_id = ?2 AND thread_root_ts = ?3
             ORDER BY at_ms ASC, id ASC",
        )?;
        let rows = stmt
            .query_map(
                params![
                    workspace.as_str(),
                    channel.as_str(),
                    thread_root_ts.as_str()
                ],
                row_to_segment,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Map `(channel_id, thread_root_ts) -> (call_id, segment_count)` for quick badges in Huddles UI.
    pub fn huddle_transcript_counts_by_thread(
        &self,
        workspace: &TeamId,
    ) -> Result<HashMap<(ChannelId, Ts), (String, u64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, thread_root_ts, MIN(call_id), COUNT(*)
             FROM huddle_transcript
             WHERE workspace_id = ?1 AND thread_root_ts IS NOT NULL AND thread_root_ts != ''
             GROUP BY channel_id, thread_root_ts",
        )?;
        let mut out = HashMap::new();
        let rows = stmt.query_map(params![workspace.as_str()], |row| {
            let ch: String = row.get(0)?;
            let ts: String = row.get(1)?;
            let call_id: String = row.get(2)?;
            let count: i64 = row.get(3)?;
            Ok((
                ChannelId::from(ch),
                Ts::from(ts),
                call_id,
                count.max(0) as u64,
            ))
        })?;
        for row in rows {
            let (ch, ts, call_id, count) = row?;
            out.insert((ch, ts), (call_id, count));
        }
        Ok(out)
    }

    /// Recent recorded huddle transcript sessions, newest first.
    pub fn list_huddle_transcript_sessions(
        &self,
        workspace: &TeamId,
        channel: Option<&ChannelId>,
        limit: usize,
    ) -> Result<Vec<HuddleTranscriptSession>> {
        let mut sql = String::from(SESSION_SELECT_PREFIX);
        let mut bind: Vec<SqlValue> = vec![SqlValue::Text(workspace.as_str().to_owned())];
        if let Some(ch) = channel {
            bind.push(SqlValue::Text(ch.as_str().to_owned()));
            sql.push_str(&format!(" AND channel_id = ?{}", bind.len()));
        }
        sql.push_str(SESSION_GROUP_SUFFIX);
        bind.push(SqlValue::Integer(limit as i64));
        sql.push_str(&format!(" LIMIT ?{}", bind.len()));

        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(bind), row_to_session)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Full-text search across locally persisted huddle transcripts (diacritic-insensitive
    /// `unicode61` so Swedish `åäö` and ASCII both match cleanly).
    pub fn search_huddle_transcripts(
        &self,
        workspace: &TeamId,
        query: &str,
        channel: Option<&ChannelId>,
        lang: Option<&str>,
        limit: usize,
    ) -> Result<Vec<HuddleTranscriptSegment>> {
        let fts = crate::query::fts_prefix_match(query);
        let lang_prefix = lang
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("{}%", s.to_ascii_lowercase()));

        let (mut sql, mut bind) = if let Some(fts) = fts {
            (
                String::from(
                    "SELECT t.id, t.channel_id, t.call_id, t.thread_root_ts, t.speaker_id, t.lang, t.source, t.text, t.at_ms
                     FROM huddle_transcript_fts f
                     JOIN huddle_transcript t ON t.id = f.rowid
                     WHERE huddle_transcript_fts MATCH ?1 AND t.workspace_id = ?2",
                ),
                vec![
                    SqlValue::Text(fts),
                    SqlValue::Text(workspace.as_str().to_owned()),
                ],
            )
        } else {
            (
                String::from(
                    "SELECT t.id, t.channel_id, t.call_id, t.thread_root_ts, t.speaker_id, t.lang, t.source, t.text, t.at_ms
                     FROM huddle_transcript t
                     WHERE t.workspace_id = ?1",
                ),
                vec![SqlValue::Text(workspace.as_str().to_owned())],
            )
        };

        if let Some(ch) = channel {
            bind.push(SqlValue::Text(ch.as_str().to_owned()));
            sql.push_str(&format!(" AND t.channel_id = ?{}", bind.len()));
        }
        if let Some(lp) = lang_prefix {
            bind.push(SqlValue::Text(lp));
            sql.push_str(&format!(
                " AND LOWER(COALESCE(t.lang, '')) LIKE ?{}",
                bind.len()
            ));
        }
        bind.push(SqlValue::Integer(limit as i64));
        sql.push_str(&format!(
            " ORDER BY t.at_ms DESC, t.id DESC LIMIT ?{}",
            bind.len()
        ));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(bind), row_to_segment)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Full-text search across locally persisted huddle transcripts in EVERY workspace stored
    /// in this database, returning workspace attribution per segment (newest first).
    pub fn search_huddle_transcripts_all(
        &self,
        query: &str,
        lang: Option<&str>,
        limit: usize,
    ) -> Result<Vec<WorkspaceHuddleTranscriptSegment>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let fts = crate::query::fts_prefix_match(query);
        let lang_prefix = lang
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("{}%", s.to_ascii_lowercase()));

        let (mut sql, mut bind) = if let Some(fts) = fts {
            (
                String::from(
                    "SELECT t.id, t.channel_id, t.call_id, t.thread_root_ts, t.speaker_id,
                            t.lang, t.source, t.text, t.at_ms, t.workspace_id
                     FROM huddle_transcript_fts f
                     JOIN huddle_transcript t ON t.id = f.rowid
                     WHERE huddle_transcript_fts MATCH ?1",
                ),
                vec![SqlValue::Text(fts)],
            )
        } else {
            (
                String::from(
                    "SELECT t.id, t.channel_id, t.call_id, t.thread_root_ts, t.speaker_id,
                            t.lang, t.source, t.text, t.at_ms, t.workspace_id
                     FROM huddle_transcript t
                     WHERE 1 = 1",
                ),
                Vec::new(),
            )
        };
        if let Some(lp) = lang_prefix {
            bind.push(SqlValue::Text(lp));
            sql.push_str(&format!(
                " AND LOWER(COALESCE(t.lang, '')) LIKE ?{}",
                bind.len()
            ));
        }
        bind.push(SqlValue::Integer(limit as i64));
        sql.push_str(&format!(
            " ORDER BY t.at_ms DESC, t.id DESC LIMIT ?{}",
            bind.len()
        ));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(bind), |row| {
                let seg = row_to_segment(row)?;
                let ws: String = row.get(9)?;
                Ok(WorkspaceHuddleTranscriptSegment {
                    workspace: TeamId::from(ws),
                    segment: seg,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Delete every stored transcript segment for `call_id` (returns number of deleted rows).
    pub fn delete_huddle_transcript(&mut self, workspace: &TeamId, call_id: &str) -> Result<usize> {
        self.ensure_writable()?;
        let mut stmt = self.conn.prepare_cached(
            "DELETE FROM huddle_transcript WHERE workspace_id = ?1 AND call_id = ?2",
        )?;
        let deleted = stmt.execute(params![workspace.as_str(), call_id])?;
        if deleted > 0 {
            Self::bump_message_revision();
        }
        Ok(deleted)
    }
}
