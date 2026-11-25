//! Persisted audio/video file transcripts (`file_transcript`) and message-file FTS5 indexing.

use super::*;
use crate::types::{File, FileId};

/// A persisted transcript for an audio or video file attached to one or more messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFileTranscript {
    pub file_id: FileId,
    pub source: String,
    pub text: String,
    pub updated_at: i64,
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

/// Combine transcripts for every file attached to a message: prefer the persisted
/// `file_transcript.text` for `(workspace, file.id)` when present, falling back to
/// [`extract_inline_file_transcript`].
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
        if let Some(text) = stored.or_else(|| extract_inline_file_transcript(file)) {
            parts.push(text);
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
        self.ensure_writable()?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO file_transcript (workspace_id, file_id, source, text, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (workspace_id, file_id) DO UPDATE SET
                     source = excluded.source,
                     text = excluded.text,
                     updated_at = excluded.updated_at",
            )?;
            stmt.execute(params![
                workspace.as_str(),
                file_id.as_str(),
                source,
                trimmed,
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
            "SELECT file_id, source, text, updated_at FROM file_transcript
             WHERE workspace_id = ?1 AND file_id = ?2",
        )?;
        Ok(stmt
            .query_row(params![workspace.as_str(), file_id.as_str()], |r| {
                let id: String = r.get(0)?;
                Ok(StoredFileTranscript {
                    file_id: FileId::from(id),
                    source: r.get(1)?,
                    text: r.get(2)?,
                    updated_at: r.get(3)?,
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
