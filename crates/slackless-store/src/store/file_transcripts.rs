//! Persisted audio/video file transcripts (`file_transcript`) and message-file FTS5 indexing.

use super::*;
use crate::types::{File, FileId, TranscriptCue};

/// A persisted transcript for an audio or video file attached to one or more messages.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredFileTranscript {
    pub file_id: FileId,
    pub source: String,
    pub text: String,
    pub cues: Vec<TranscriptCue>,
    pub updated_at: i64,
}

fn decode_cues_json(raw: &str) -> Vec<TranscriptCue> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    serde_json::from_str::<Vec<TranscriptCue>>(trimmed).unwrap_or_default()
}

/// A matched `file_transcript` row attributed to its workspace (plus optional carrying message).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceFileTranscript {
    pub workspace: TeamId,
    pub transcript: StoredFileTranscript,
    /// First carrying message's channel & ts when linked via `message_file`.
    pub channel: Option<ChannelId>,
    pub message_ts: Option<Ts>,
}

/// Extract Slack's inline clip transcript text from a [`File`]'s metadata
/// (`extra["transcription"]["preview"]["content"]`, `extra["transcription"]["text"]`, or
/// `extra["transcript"]`), trimmed and non-empty.
pub fn extract_inline_file_transcript(file: &File) -> Option<String> {
    if let Some(t) = file.extra.get("transcription") {
        if let Some(s) = t
            .get("preview")
            .and_then(|p| p.get("content"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(s.to_owned());
        }
        if let Some(s) = t
            .get("text")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(s.to_owned());
        }
    }
    file.extra
        .get("transcript")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

/// Extract searchable inline body text for snippets and canvases attached to a message:
/// `file.preview`, `extra["plain_text"]`, `extra["preview_plain_text"]`, `extra["canvas_markdown"]`,
/// `extra["canvas_text"]`, `extra["markdown"]`, and `extra["document_content"]`.
pub fn extract_inline_file_body(file: &File) -> Option<String> {
    fn push_unique<'a>(parts: &mut Vec<&'a str>, s: &'a str) {
        let trimmed = s.trim();
        if !trimmed.is_empty() && !parts.contains(&trimmed) {
            parts.push(trimmed);
        }
    }

    let mut parts: Vec<&str> = Vec::new();
    if let Some(preview) = file.preview.as_deref() {
        push_unique(&mut parts, preview);
    }
    for key in [
        "plain_text",
        "preview_plain_text",
        "canvas_markdown",
        "canvas_text",
        "markdown",
        "document_content",
    ] {
        if let Some(val) = file.extra.get(key).and_then(|v| v.as_str()) {
            push_unique(&mut parts, val);
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// Combine transcripts and inline snippet/canvas body text for every file attached to a message:
/// prefer the persisted `file_transcript.text` for `(workspace, file.id)` when present, falling
/// back to [`extract_inline_file_transcript`], and append [`extract_inline_file_body`] when
/// distinct so snippets, canvases, and spoken clips are all searchable in `message_fts`.
pub(super) fn compute_message_file_transcripts(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    files: &[File],
) -> Result<String> {
    if files.is_empty() {
        return Ok(String::new());
    }
    let mut stmt = tx.prepare_cached(
        "SELECT text FROM file_transcript WHERE workspace_id = ?1 AND file_id = ?2",
    )?;
    let mut parts: Vec<String> = Vec::new();
    for file in files {
        let stored = if file.id.as_str().is_empty() {
            None
        } else {
            stmt.query_row(params![workspace.as_str(), file.id.as_str()], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
        };
        let transcript = stored.or_else(|| extract_inline_file_transcript(file));
        if let Some(text) = &transcript {
            parts.push(text.clone());
        }
        if let Some(body) = extract_inline_file_body(file)
            && transcript.as_deref() != Some(body.as_str())
        {
            parts.push(body);
        }
    }
    Ok(parts.join(" "))
}

/// Recompute `message_current.file_transcripts` (firing `message_current_au` into `message_fts`)
/// for every message linked to `(workspace, file_id)` in `message_file`.
fn refresh_messages_for_file(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    file_id: &FileId,
) -> Result<usize> {
    let message_ids: Vec<i64> = {
        let mut stmt = tx.prepare_cached(
            "SELECT message_id FROM message_file
             WHERE workspace_id = ?1 AND file_id = ?2",
        )?;
        let rows = stmt.query_map(params![workspace.as_str(), file_id.as_str()], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    if message_ids.is_empty() {
        return Ok(0);
    }

    let mut updated = 0usize;
    for message_id in message_ids {
        let json: Option<String> = {
            let mut stmt = tx.prepare_cached("SELECT json FROM message_current WHERE id = ?1")?;
            stmt.query_row(params![message_id], |r| decode_json_value(r.get_ref(0)?))
                .optional()?
        };
        let Some(json) = json else {
            continue;
        };
        let Ok(message) = serde_json::from_str::<Message>(&json) else {
            continue;
        };
        let combined = compute_message_file_transcripts(tx, workspace, &message.files)?;
        let mut upd =
            tx.prepare_cached("UPDATE message_current SET file_transcripts = ?1 WHERE id = ?2")?;
        updated += upd.execute(params![combined, message_id])?;
    }
    Ok(updated)
}

impl Store {
    /// Persist a video/audio file transcript and update `message_current.file_transcripts` (and
    /// `message_fts`) for every message carrying `(workspace, file_id)`. Returns the number of
    /// messages updated.
    pub fn upsert_file_transcript(
        &mut self,
        workspace: &TeamId,
        file_id: &FileId,
        source: &str,
        text: &str,
    ) -> Result<usize> {
        self.upsert_file_transcript_with_cues(workspace, file_id, source, text, &[])
    }

    /// Persist a video/audio file transcript together with timed [`TranscriptCue`] entries. Plain
    /// `text` stays indexed in `message_current.file_transcripts` / `message_fts` while `cues` are
    /// preserved in `file_transcript.cues_json` for clickable seek & live highlighting.
    pub fn upsert_file_transcript_with_cues(
        &mut self,
        workspace: &TeamId,
        file_id: &FileId,
        source: &str,
        text: &str,
        cues: &[TranscriptCue],
    ) -> Result<usize> {
        self.ensure_writable()?;
        let trimmed = text.trim();
        let fallback_from_cues = if trimmed.is_empty() && !cues.is_empty() {
            TranscriptCue::plain_text(cues)
        } else {
            String::new()
        };
        let effective_text = if trimmed.is_empty() {
            fallback_from_cues.as_str()
        } else {
            trimmed
        };
        if effective_text.is_empty() {
            return Ok(0);
        }
        let cues_json = if cues.is_empty() {
            String::new()
        } else {
            serde_json::to_string(cues).unwrap_or_default()
        };
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO file_transcript (workspace_id, file_id, source, text, cues_json, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (workspace_id, file_id) DO UPDATE SET
                     source = excluded.source,
                     text = excluded.text,
                     cues_json = CASE
                         WHEN excluded.cues_json != '' THEN excluded.cues_json
                         WHEN excluded.text = file_transcript.text THEN file_transcript.cues_json
                         ELSE ''
                     END,
                     updated_at = excluded.updated_at",
            )?;
            stmt.execute(params![
                workspace.as_str(),
                file_id.as_str(),
                source,
                effective_text,
                cues_json,
                now_secs(),
            ])?;
        }
        let updated = refresh_messages_for_file(&tx, workspace, file_id)?;
        tx.commit()?;
        Self::bump_message_revision();
        Ok(updated)
    }

    /// Look up one stored file transcript by `(workspace, file_id)`.
    pub fn file_transcript(
        &self,
        workspace: &TeamId,
        file_id: &FileId,
    ) -> Result<Option<StoredFileTranscript>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT file_id, source, text, cues_json, updated_at FROM file_transcript
             WHERE workspace_id = ?1 AND file_id = ?2",
        )?;
        Ok(stmt
            .query_row(params![workspace.as_str(), file_id.as_str()], |r| {
                let id: String = r.get(0)?;
                let cues_raw: String = r.get(3)?;
                Ok(StoredFileTranscript {
                    file_id: FileId::from(id),
                    source: r.get(1)?,
                    text: r.get(2)?,
                    cues: decode_cues_json(&cues_raw),
                    updated_at: r.get(4)?,
                })
            })
            .optional()?)
    }

    /// Load every stored file transcript in `workspace` as `file_id -> text` in one query.
    pub fn all_file_transcripts(&self, workspace: &TeamId) -> Result<HashMap<FileId, String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT file_id, text FROM file_transcript WHERE workspace_id = ?1")?;
        let rows = stmt.query_map(params![workspace.as_str()], |r| {
            Ok((FileId::from(r.get::<_, String>(0)?), r.get::<_, String>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, text) = row?;
            out.insert(id, text);
        }
        Ok(out)
    }

    /// Load timed transcript cues for every file in `workspace` that has stored cue timestamps.
    pub fn all_file_transcript_cues(
        &self,
        workspace: &TeamId,
    ) -> Result<HashMap<FileId, Vec<TranscriptCue>>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT file_id, cues_json FROM file_transcript
             WHERE workspace_id = ?1 AND cues_json != ''",
        )?;
        let rows = stmt.query_map(params![workspace.as_str()], |r| {
            Ok((FileId::from(r.get::<_, String>(0)?), r.get::<_, String>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, raw) = row?;
            let cues = decode_cues_json(&raw);
            if !cues.is_empty() {
                out.insert(id, cues);
            }
        }
        Ok(out)
    }

    /// Search persisted `file_transcript` entries in one `workspace` (or across ALL stored
    /// workspaces when `workspace` is `None`). Matches every positive term in `query`
    /// case-insensitively (with trailing `*` treated as a substring stem) and excludes any
    /// `excluded_terms`, returning newest-updated transcripts first along with optional
    /// carrying-message coordinates from `message_file`.
    pub fn search_file_transcripts(
        &self,
        workspace: Option<&TeamId>,
        query: &SearchQuery,
        limit: usize,
    ) -> Result<Vec<WorkspaceFileTranscript>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let terms: Vec<String> = query
            .terms
            .iter()
            .map(|t| t.trim_end_matches('*').to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        let excluded: Vec<String> = query
            .excluded_terms
            .iter()
            .map(|t| t.trim_end_matches('*').to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        if terms.is_empty() && excluded.is_empty() {
            return Ok(Vec::new());
        }

        let mut sql = String::from(
            "SELECT ft.workspace_id, ft.file_id, ft.source, ft.text, ft.cues_json, ft.updated_at,
                    (SELECT mc.channel_id
                     FROM message_file mf
                     JOIN message_current mc ON mc.id = mf.message_id
                     WHERE mf.workspace_id = ft.workspace_id AND mf.file_id = ft.file_id
                     ORDER BY mc.ts_key DESC LIMIT 1),
                    (SELECT mc.ts
                     FROM message_file mf
                     JOIN message_current mc ON mc.id = mf.message_id
                     WHERE mf.workspace_id = ft.workspace_id AND mf.file_id = ft.file_id
                     ORDER BY mc.ts_key DESC LIMIT 1)
             FROM file_transcript ft",
        );
        let mut args: Vec<SqlValue> = Vec::new();
        if let Some(ws) = workspace {
            sql.push_str(" WHERE ft.workspace_id = ?1");
            args.push(SqlValue::Text(ws.as_str().to_owned()));
        }
        sql.push_str(" ORDER BY ft.updated_at DESC, ft.file_id ASC");

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), |r| {
            let ws: String = r.get(0)?;
            let fid: String = r.get(1)?;
            let source: String = r.get(2)?;
            let text: String = r.get(3)?;
            let cues_raw: String = r.get(4)?;
            let updated_at: i64 = r.get(5)?;
            let channel: Option<String> = r.get(6)?;
            let message_ts: Option<String> = r.get(7)?;
            Ok(WorkspaceFileTranscript {
                workspace: TeamId::from(ws),
                transcript: StoredFileTranscript {
                    file_id: FileId::from(fid),
                    source,
                    text,
                    cues: decode_cues_json(&cues_raw),
                    updated_at,
                },
                channel: channel.map(ChannelId::from),
                message_ts: message_ts.map(Ts::from),
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            let entry = row?;
            let lower = entry.transcript.text.to_lowercase();
            let id_lower = entry.transcript.file_id.as_str().to_lowercase();
            let matches_pos = terms
                .iter()
                .all(|t| lower.contains(t.as_str()) || id_lower.contains(t.as_str()));
            if !matches_pos {
                continue;
            }
            if excluded.iter().any(|t| lower.contains(t.as_str())) {
                continue;
            }
            out.push(entry);
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Delete a persisted file transcript and refresh affected messages' `file_transcripts` (and
    /// `message_fts`). Returns `true` if a stored transcript existed and was deleted.
    pub fn delete_file_transcript(&mut self, workspace: &TeamId, file_id: &FileId) -> Result<bool> {
        self.ensure_writable()?;
        let tx = self.conn.transaction()?;
        let deleted = {
            let mut stmt = tx.prepare_cached(
                "DELETE FROM file_transcript WHERE workspace_id = ?1 AND file_id = ?2",
            )?;
            stmt.execute(params![workspace.as_str(), file_id.as_str()])?
        };
        if deleted > 0 {
            refresh_messages_for_file(&tx, workspace, file_id)?;
        }
        tx.commit()?;
        if deleted > 0 {
            Self::bump_message_revision();
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
