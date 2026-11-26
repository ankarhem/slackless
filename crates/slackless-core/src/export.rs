//! Shared Markdown & JSON export engine for channels, threads, and media/huddle transcripts.
//!
//! Backed by the local SQLite [`Store`] (and also exposing pure renderers so GUI fixture mode
//! produces identical Markdown/JSON output without duplicating formatting logic).

use std::collections::HashMap;

use serde::Serialize;

use crate::domain::mrkdwn::mrkdwn_to_plain;
use crate::store::{
    HuddleTranscriptSegment, Result as StoreResult, Store, StoredFileTranscript, StoredMessage,
    extract_inline_file_transcript,
};
use crate::types::time::civil_from_days;
use crate::types::{ChannelId, FileId, TeamId, Ts, UserId};

/// Supported archive export formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportFormat {
    #[default]
    Markdown,
    Json,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Json => "json",
        }
    }
}

/// Options controlling channel and thread exports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOptions {
    pub format: ExportFormat,
    /// Maximum top-level messages for a channel export (newest window, ordered oldest first).
    pub limit: usize,
    /// Include tombstoned locally preserved deleted messages (`deleted = 1`).
    pub include_deleted: bool,
    /// When exporting a channel, nest stored thread replies under their thread roots.
    pub include_threads: bool,
    /// Include stored or inline audio/video file transcripts under carrying messages.
    pub include_transcripts: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: ExportFormat::Markdown,
            limit: 10_000,
            include_deleted: true,
            include_threads: false,
            include_transcripts: true,
        }
    }
}

/// What to export from the local archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportTarget {
    Channel { channel: ChannelId },
    Thread { channel: ChannelId, thread_ts: Ts },
    FileTranscript { file_id: FileId },
    HuddleTranscript { call_id: String },
}

/// Rendered export output plus metadata for saving or printing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportArtifact {
    pub format: ExportFormat,
    pub title: String,
    pub suggested_filename: String,
    pub item_count: usize,
    pub content: String,
}

/// Serializable reaction entry in exported JSON/Markdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedReaction {
    pub name: String,
    pub count: u32,
}

/// Serializable file attachment entry (with optional stored/inline transcript).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedFile {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
}

/// One exported message (used for channel and thread exports).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedMessage {
    pub ts: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_utc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
    pub text: String,
    pub plain_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
    pub deleted: bool,
    pub edited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<ExportedReaction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<ExportedFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permalink: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<ExportedMessage>,
}

/// Structured channel or thread export document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedMessagesDoc {
    /// `"channel"` or `"thread"`.
    pub kind: String,
    pub workspace: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_name: Option<String>,
    pub channel: ExportedChannelMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
    pub count: usize,
    pub messages: Vec<ExportedMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedChannelMeta {
    pub id: String,
    pub label: String,
}

/// Structured audio/video file transcript export document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedFileTranscriptDoc {
    pub kind: &'static str,
    pub workspace: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_name: Option<String>,
    pub file_id: String,
    pub source: String,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_utc: Option<String>,
    pub text: String,
}

/// One utterance in an exported huddle transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedHuddleSegment {
    pub id: i64,
    pub at_ms: i64,
    pub timestamp_utc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    pub speaker: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    pub source: String,
    pub text: String,
}

/// Structured huddle transcript export document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedHuddleTranscriptDoc {
    pub kind: &'static str,
    pub workspace: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_name: Option<String>,
    pub call_id: String,
    pub channel: ExportedChannelMeta,
    pub count: usize,
    pub segments: Vec<ExportedHuddleSegment>,
}

/// Export a channel's locally stored messages to Markdown or JSON.
pub fn export_channel(
    store: &Store,
    workspace: &TeamId,
    channel: &ChannelId,
    opts: &ExportOptions,
) -> StoreResult<ExportArtifact> {
    let ws_name = store
        .team(workspace)?
        .map(|t| t.name)
        .filter(|n| !n.trim().is_empty());
    let label = store
        .conversation_summary(workspace, channel)?
        .map(|c| c.label())
        .unwrap_or_else(|| channel.as_str().to_owned());
    let users = load_user_displays(store, workspace)?;
    let transcripts = if opts.include_transcripts {
        store.all_file_transcripts(workspace)?
    } else {
        HashMap::new()
    };

    let raw = store.channel_history(workspace, channel, None, opts.limit)?;
    let mut messages = Vec::with_capacity(raw.len());
    for stored in raw {
        if !opts.include_deleted && stored.deleted {
            continue;
        }
        let mut row =
            build_exported_message(&stored, &users, &transcripts, opts.include_transcripts);
        if opts.include_threads && stored.message.reply_count.unwrap_or(0) > 0 {
            let thread_rows = store.thread_replies(workspace, channel, &stored.message.ts)?;
            for reply in thread_rows {
                if reply.message.ts == stored.message.ts {
                    continue;
                }
                if !opts.include_deleted && reply.deleted {
                    continue;
                }
                row.replies.push(build_exported_message(
                    &reply,
                    &users,
                    &transcripts,
                    opts.include_transcripts,
                ));
            }
        }
        messages.push(row);
    }

    let doc = ExportedMessagesDoc {
        kind: "channel".to_owned(),
        workspace: workspace.as_str().to_owned(),
        workspace_name: ws_name,
        channel: ExportedChannelMeta {
            id: channel.as_str().to_owned(),
            label: label.clone(),
        },
        thread_ts: None,
        count: messages.len(),
        messages,
    };
    let content = match opts.format {
        ExportFormat::Markdown => render_messages_markdown(&doc),
        ExportFormat::Json => render_messages_json(&doc)?,
    };
    let stem = sanitize_export_filename(&label, channel.as_str());
    Ok(ExportArtifact {
        format: opts.format,
        title: label,
        suggested_filename: format!("{stem}-export.{}", opts.format.extension()),
        item_count: doc.count,
        content,
    })
}

/// Export one thread (root message + all replies, oldest first) from the local SQLite store.
pub fn export_thread(
    store: &Store,
    workspace: &TeamId,
    channel: &ChannelId,
    thread_ts: &Ts,
    opts: &ExportOptions,
) -> StoreResult<ExportArtifact> {
    let ws_name = store
        .team(workspace)?
        .map(|t| t.name)
        .filter(|n| !n.trim().is_empty());
    let label = store
        .conversation_summary(workspace, channel)?
        .map(|c| c.label())
        .unwrap_or_else(|| channel.as_str().to_owned());
    let users = load_user_displays(store, workspace)?;
    let transcripts = if opts.include_transcripts {
        store.all_file_transcripts(workspace)?
    } else {
        HashMap::new()
    };

    let raw = store.thread_replies(workspace, channel, thread_ts)?;
    let messages: Vec<ExportedMessage> = raw
        .into_iter()
        .filter(|m| opts.include_deleted || !m.deleted)
        .map(|m| build_exported_message(&m, &users, &transcripts, opts.include_transcripts))
        .collect();

    let doc = ExportedMessagesDoc {
        kind: "thread".to_owned(),
        workspace: workspace.as_str().to_owned(),
        workspace_name: ws_name,
        channel: ExportedChannelMeta {
            id: channel.as_str().to_owned(),
            label: label.clone(),
        },
        thread_ts: Some(thread_ts.as_str().to_owned()),
        count: messages.len(),
        messages,
    };
    let content = match opts.format {
        ExportFormat::Markdown => render_messages_markdown(&doc),
        ExportFormat::Json => render_messages_json(&doc)?,
    };
    let stem = sanitize_export_filename(&label, channel.as_str());
    let ts_slug = sanitize_export_filename(thread_ts.as_str(), "thread");
    Ok(ExportArtifact {
        format: opts.format,
        title: format!("Thread in {label} ({thread_ts})"),
        suggested_filename: format!("{stem}-thread-{ts_slug}.{}", opts.format.extension()),
        item_count: doc.count,
        content,
    })
}

/// Export a stored audio/video `file_transcript` by `file_id`, returning `Ok(None)` if not stored.
pub fn export_file_transcript(
    store: &Store,
    workspace: &TeamId,
    file_id: &FileId,
    format: ExportFormat,
) -> StoreResult<Option<ExportArtifact>> {
    let Some(stored) = store.file_transcript(workspace, file_id)? else {
        return Ok(None);
    };
    let ws_name = store
        .team(workspace)?
        .map(|t| t.name)
        .filter(|n| !n.trim().is_empty());
    let doc = build_file_transcript_doc(workspace, ws_name, &stored);
    let content = match format {
        ExportFormat::Markdown => render_file_transcript_markdown(&doc),
        ExportFormat::Json => render_file_transcript_json(&doc)?,
    };
    let stem = sanitize_export_filename(file_id.as_str(), "transcript");
    Ok(Some(ExportArtifact {
        format,
        title: format!("Transcript: {file_id}"),
        suggested_filename: format!("{stem}-transcript.{}", format.extension()),
        item_count: 1,
        content,
    }))
}

/// Export a stored huddle transcript session by `call_id`, returning `Ok(None)` if no lines exist.
pub fn export_huddle_transcript(
    store: &Store,
    workspace: &TeamId,
    call_id: &str,
    format: ExportFormat,
) -> StoreResult<Option<ExportArtifact>> {
    let segments = store.huddle_transcript_for_call(workspace, call_id)?;
    if segments.is_empty() {
        return Ok(None);
    }
    let ws_name = store
        .team(workspace)?
        .map(|t| t.name)
        .filter(|n| !n.trim().is_empty());
    let channel = segments[0].channel.clone();
    let label = store
        .conversation_summary(workspace, &channel)?
        .map(|c| c.label())
        .unwrap_or_else(|| channel.as_str().to_owned());
    let users = load_user_displays(store, workspace)?;
    let doc = build_huddle_transcript_doc(
        workspace, ws_name, call_id, &channel, &label, &segments, &users,
    );
    let content = match format {
        ExportFormat::Markdown => render_huddle_transcript_markdown(&doc),
        ExportFormat::Json => render_huddle_transcript_json(&doc)?,
    };
    let stem = sanitize_export_filename(&label, channel.as_str());
    let call_slug = sanitize_export_filename(call_id, "huddle");
    Ok(Some(ExportArtifact {
        format,
        title: format!("Huddle Transcript — {label} ({call_id})"),
        suggested_filename: format!("{stem}-huddle-{call_slug}.{}", format.extension()),
        item_count: doc.count,
        content,
    }))
}

/// Dispatch any [`ExportTarget`] through the shared export engine.
pub fn export_target(
    store: &Store,
    workspace: &TeamId,
    target: &ExportTarget,
    opts: &ExportOptions,
) -> StoreResult<Option<ExportArtifact>> {
    match target {
        ExportTarget::Channel { channel } => {
            export_channel(store, workspace, channel, opts).map(Some)
        }
        ExportTarget::Thread { channel, thread_ts } => {
            export_thread(store, workspace, channel, thread_ts, opts).map(Some)
        }
        ExportTarget::FileTranscript { file_id } => {
            export_file_transcript(store, workspace, file_id, opts.format)
        }
        ExportTarget::HuddleTranscript { call_id } => {
            export_huddle_transcript(store, workspace, call_id, opts.format)
        }
    }
}

/// Build an [`ExportedMessage`] from a [`StoredMessage`], resolving user display names and
/// attached file transcripts.
pub fn build_exported_message(
    stored: &StoredMessage,
    users: &HashMap<UserId, String>,
    transcripts: &HashMap<FileId, String>,
    include_transcripts: bool,
) -> ExportedMessage {
    let msg = &stored.message;
    let author = msg
        .user
        .as_ref()
        .and_then(|u| users.get(u).cloned())
        .or_else(|| msg.bot_profile.as_ref().map(|b| b.name.clone()))
        .or_else(|| msg.username.clone());
    let files = msg
        .files
        .iter()
        .map(|f| {
            let name = f
                .name
                .clone()
                .or_else(|| f.title.clone())
                .unwrap_or_else(|| f.id.as_str().to_owned());
            let transcript = if include_transcripts {
                transcripts
                    .get(&f.id)
                    .cloned()
                    .or_else(|| extract_inline_file_transcript(f))
            } else {
                None
            };
            ExportedFile {
                id: f.id.as_str().to_owned(),
                name,
                transcript,
            }
        })
        .collect();
    let reactions = msg
        .reactions
        .iter()
        .map(|r| ExportedReaction {
            name: r.name.clone(),
            count: r.count,
        })
        .collect();

    ExportedMessage {
        ts: msg.ts.as_str().to_owned(),
        timestamp_utc: msg.ts.secs().map(format_utc_timestamp),
        user: msg.user.as_ref().map(|u| u.as_str().to_owned()),
        author,
        bot_id: msg.bot_id.as_ref().map(|b| b.as_str().to_owned()),
        text: msg.text.clone(),
        plain_text: mrkdwn_to_plain(&msg.text),
        thread_ts: msg.thread_ts.as_ref().map(|t| t.as_str().to_owned()),
        deleted: stored.deleted,
        edited: msg.edited.is_some(),
        reply_count: msg.reply_count,
        reactions,
        files,
        permalink: msg.permalink.clone(),
        replies: Vec::new(),
    }
}

pub fn build_file_transcript_doc(
    workspace: &TeamId,
    workspace_name: Option<String>,
    stored: &StoredFileTranscript,
) -> ExportedFileTranscriptDoc {
    ExportedFileTranscriptDoc {
        kind: "file_transcript",
        workspace: workspace.as_str().to_owned(),
        workspace_name,
        file_id: stored.file_id.as_str().to_owned(),
        source: stored.source.clone(),
        updated_at: stored.updated_at,
        updated_utc: (stored.updated_at > 0).then(|| format_utc_timestamp(stored.updated_at)),
        text: stored.text.clone(),
    }
}

pub fn build_huddle_transcript_doc(
    workspace: &TeamId,
    workspace_name: Option<String>,
    call_id: &str,
    channel: &ChannelId,
    channel_label: &str,
    segments: &[HuddleTranscriptSegment],
    users: &HashMap<UserId, String>,
) -> ExportedHuddleTranscriptDoc {
    let rows: Vec<ExportedHuddleSegment> = segments
        .iter()
        .map(|seg| {
            let speaker = seg
                .speaker_id
                .as_ref()
                .and_then(|u| users.get(u).cloned())
                .or_else(|| seg.speaker_id.as_ref().map(|u| u.as_str().to_owned()))
                .unwrap_or_else(|| "Speaker".to_owned());
            ExportedHuddleSegment {
                id: seg.id,
                at_ms: seg.at_ms,
                timestamp_utc: format_utc_timestamp(seg.at_ms.div_euclid(1_000)),
                speaker_id: seg.speaker_id.as_ref().map(|u| u.as_str().to_owned()),
                speaker,
                lang: seg.lang.clone(),
                source: seg.source.clone(),
                text: seg.text.clone(),
            }
        })
        .collect();
    ExportedHuddleTranscriptDoc {
        kind: "huddle_transcript",
        workspace: workspace.as_str().to_owned(),
        workspace_name,
        call_id: call_id.to_owned(),
        channel: ExportedChannelMeta {
            id: channel.as_str().to_owned(),
            label: channel_label.to_owned(),
        },
        count: rows.len(),
        segments: rows,
    }
}

/// Render an [`ExportedMessagesDoc`] (channel or thread) as clean Markdown.
pub fn render_messages_markdown(doc: &ExportedMessagesDoc) -> String {
    let mut out = String::new();
    if let Some(thread_ts) = &doc.thread_ts {
        out.push_str(&format!(
            "# Thread in {} (`{thread_ts}`)\n\n",
            doc.channel.label
        ));
    } else {
        out.push_str(&format!("# {}\n\n", doc.channel.label));
    }
    if let Some(ws) = &doc.workspace_name {
        out.push_str(&format!(
            "_Workspace:_ {ws} (`{}`) · _Messages:_ {}\n\n---\n\n",
            doc.workspace, doc.count
        ));
    } else {
        out.push_str(&format!(
            "_Workspace:_ `{}` · _Messages:_ {}\n\n---\n\n",
            doc.workspace, doc.count
        ));
    }

    for msg in &doc.messages {
        write_markdown_message(&mut out, msg, "");
        if !msg.replies.is_empty() {
            out.push_str(&format!(
                "> **Thread replies ({})**\n>\n",
                msg.replies.len()
            ));
            for reply in &msg.replies {
                write_markdown_message(&mut out, reply, "> ");
            }
        }
        out.push_str("---\n\n");
    }
    out
}

fn write_markdown_message(out: &mut String, msg: &ExportedMessage, prefix: &str) {
    let author = msg
        .author
        .as_deref()
        .or(msg.user.as_deref())
        .or(msg.bot_id.as_deref())
        .unwrap_or("unknown");
    let stamp = msg
        .timestamp_utc
        .as_deref()
        .map(|utc| format!("{utc} · `{}`", msg.ts))
        .unwrap_or_else(|| format!("`{}`", msg.ts));
    let mut flags = String::new();
    if msg.edited {
        flags.push_str(" _(edited)_");
    }
    if msg.deleted {
        flags.push_str(" _(deleted)_");
    }
    out.push_str(&format!(
        "{prefix}**{author}** ({stamp}){flags}\n{prefix}\n"
    ));
    let body = if !msg.plain_text.is_empty() {
        &msg.plain_text
    } else {
        &msg.text
    };
    if !body.is_empty() {
        for line in body.lines() {
            out.push_str(&format!("{prefix}{line}\n"));
        }
        out.push_str(&format!("{prefix}\n"));
    }
    if !msg.files.is_empty() {
        let names: Vec<&str> = msg.files.iter().map(|f| f.name.as_str()).collect();
        out.push_str(&format!(
            "{prefix}_Files:_ {}\n{prefix}\n",
            names.join(", ")
        ));
        for f in &msg.files {
            if let Some(tr) = &f.transcript {
                out.push_str(&format!("{prefix}_Transcript ({}):_\n", f.name));
                for line in tr.lines() {
                    out.push_str(&format!("{prefix}> {line}\n"));
                }
                out.push_str(&format!("{prefix}\n"));
            }
        }
    }
    if !msg.reactions.is_empty() {
        let rx: Vec<String> = msg
            .reactions
            .iter()
            .map(|r| format!(":{}: {}", r.name, r.count))
            .collect();
        out.push_str(&format!(
            "{prefix}_Reactions:_ {}\n{prefix}\n",
            rx.join(" · ")
        ));
    }
}

/// Render an [`ExportedMessagesDoc`] as pretty-printed JSON.
pub fn render_messages_json(
    doc: &ExportedMessagesDoc,
) -> std::result::Result<String, serde_json::Error> {
    serde_json::to_string_pretty(doc)
}

/// Render a file transcript document as clean Markdown.
pub fn render_file_transcript_markdown(doc: &ExportedFileTranscriptDoc) -> String {
    let mut out = format!("# Media Transcript — `{}`\n\n", doc.file_id);
    let ws = doc
        .workspace_name
        .as_deref()
        .map(|n| format!("{n} (`{}`)", doc.workspace))
        .unwrap_or_else(|| format!("`{}`", doc.workspace));
    let updated = doc
        .updated_utc
        .as_deref()
        .map(|u| format!(" · _Updated:_ {u}"))
        .unwrap_or_default();
    out.push_str(&format!(
        "_Workspace:_ {ws} · _Source:_ `{}`{updated}\n\n---\n\n{}\n",
        doc.source,
        doc.text.trim()
    ));
    out
}

/// Render a file transcript document as pretty-printed JSON.
pub fn render_file_transcript_json(
    doc: &ExportedFileTranscriptDoc,
) -> std::result::Result<String, serde_json::Error> {
    serde_json::to_string_pretty(doc)
}

/// Render a huddle transcript document as clean Markdown.
pub fn render_huddle_transcript_markdown(doc: &ExportedHuddleTranscriptDoc) -> String {
    let mut out = format!(
        "# Huddle Transcript — {} (`{}`)\n\n",
        doc.channel.label, doc.call_id
    );
    let ws = doc
        .workspace_name
        .as_deref()
        .map(|n| format!("{n} (`{}`)", doc.workspace))
        .unwrap_or_else(|| format!("`{}`", doc.workspace));
    out.push_str(&format!(
        "_Workspace:_ {ws} · _Lines:_ {}\n\n---\n\n",
        doc.count
    ));
    for seg in &doc.segments {
        let lang_tag = seg
            .lang
            .as_deref()
            .filter(|l| !l.trim().is_empty())
            .map(|l| format!(" `[{l}]`"))
            .unwrap_or_default();
        out.push_str(&format!(
            "- **{}** ({}){lang_tag}: {}\n",
            seg.speaker, seg.timestamp_utc, seg.text
        ));
    }
    out
}

/// Render a huddle transcript document as pretty-printed JSON.
pub fn render_huddle_transcript_json(
    doc: &ExportedHuddleTranscriptDoc,
) -> std::result::Result<String, serde_json::Error> {
    serde_json::to_string_pretty(doc)
}

/// Format unix epoch seconds as `YYYY-MM-DD HH:MM:SS UTC`.
pub fn format_utc_timestamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

/// Turn a channel label or ID into a filesystem-safe filename stem.
pub fn sanitize_export_filename(label: &str, fallback: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn load_user_displays(store: &Store, workspace: &TeamId) -> StoreResult<HashMap<UserId, String>> {
    let users = store.list_users(workspace, None, true)?;
    let mut map = HashMap::with_capacity(users.len());
    for user in users {
        let best = user
            .display_name
            .or(user.real_name)
            .or_else(|| (!user.name.is_empty()).then_some(user.name));
        if let Some(name) = best {
            map.insert(user.id, name);
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewHuddleTranscriptSegment;
    use crate::types::{Conversation, File, Message, Profile, Reaction, Team, User};

    fn seeded_store() -> (Store, TeamId, ChannelId) {
        let mut store = Store::open_in_memory().unwrap();
        let ws = TeamId::from("T1");
        let ch = ChannelId::from("C1");
        store
            .upsert_team(
                &ws,
                &Team {
                    id: ws.clone(),
                    name: "Acme".into(),
                    domain: "acme".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: ch.clone(),
                    name: Some("engineering".into()),
                    is_channel: true,
                    is_member: true,
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .upsert_user(
                &ws,
                &User {
                    id: UserId::from("U1"),
                    name: "alice".into(),
                    profile: Profile {
                        display_name: Some("Alice".into()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap();

        let root_ts = Ts::from("1700000100.000100");
        store
            .upsert_messages(
                &ws,
                &ch,
                &[
                    Message {
                        ts: root_ts.clone(),
                        user: Some(UserId::from("U1")),
                        text: "Ship *release* v2".into(),
                        reply_count: Some(1),
                        reactions: vec![Reaction {
                            name: "rocket".into(),
                            count: 2,
                            users: vec![UserId::from("U1")],
                        }],
                        files: vec![File {
                            id: FileId::from("F_CLIP"),
                            name: Some("demo.mp4".into()),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    Message {
                        ts: Ts::from("1700000150.000100"),
                        thread_ts: Some(root_ts),
                        user: Some(UserId::from("U1")),
                        text: "Verified metrics look great".into(),
                        ..Default::default()
                    },
                ],
            )
            .unwrap();
        store
            .upsert_file_transcript(
                &ws,
                &FileId::from("F_CLIP"),
                "parakeet-v3",
                "Here is the live release walkthrough",
            )
            .unwrap();
        store
            .insert_huddle_transcript_segment(
                &ws,
                &NewHuddleTranscriptSegment {
                    channel: &ch,
                    call_id: "R_CALL1",
                    thread_root_ts: None,
                    speaker_id: Some(&UserId::from("U1")),
                    lang: Some("en"),
                    source: "parakeet-v3",
                    text: "Let us cut the release branch now",
                    at_ms: 1_700_000_200_000,
                },
            )
            .unwrap();
        (store, ws, ch)
    }

    #[test]
    fn export_channel_markdown_and_json_include_threads_and_transcripts() {
        let (store, ws, ch) = seeded_store();
        let md = export_channel(
            &store,
            &ws,
            &ch,
            &ExportOptions {
                format: ExportFormat::Markdown,
                include_threads: true,
                include_transcripts: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(md.item_count, 1);
        assert_eq!(md.suggested_filename, "engineering-export.md");
        assert!(md.content.contains("# #engineering"));
        assert!(md.content.contains("**Alice**"));
        assert!(md.content.contains("Ship release v2"));
        assert!(md.content.contains(":rocket: 2"));
        assert!(md.content.contains("Here is the live release walkthrough"));
        assert!(md.content.contains("Thread replies (1)"));
        assert!(md.content.contains("Verified metrics look great"));

        let json_art = export_channel(
            &store,
            &ws,
            &ch,
            &ExportOptions {
                format: ExportFormat::Json,
                include_threads: true,
                include_transcripts: true,
                ..Default::default()
            },
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_art.content).unwrap();
        assert_eq!(parsed["kind"], "channel");
        assert_eq!(parsed["workspace"], "T1");
        assert_eq!(parsed["workspace_name"], "Acme");
        assert_eq!(parsed["count"], 1);
        assert_eq!(
            parsed["messages"][0]["replies"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            parsed["messages"][0]["files"][0]["transcript"],
            "Here is the live release walkthrough"
        );
    }

    #[test]
    fn export_thread_file_transcript_and_huddle_transcript() {
        let (store, ws, ch) = seeded_store();

        // Thread export
        let thread_md = export_thread(
            &store,
            &ws,
            &ch,
            &Ts::from("1700000100.000100"),
            &ExportOptions::default(),
        )
        .unwrap();
        assert_eq!(thread_md.item_count, 2);
        assert!(thread_md.content.contains("# Thread in #engineering"));
        assert!(thread_md.content.contains("Verified metrics look great"));

        // File transcript export
        let ft_json =
            export_file_transcript(&store, &ws, &FileId::from("F_CLIP"), ExportFormat::Json)
                .unwrap()
                .expect("file transcript exists");
        let parsed_ft: serde_json::Value = serde_json::from_str(&ft_json.content).unwrap();
        assert_eq!(parsed_ft["kind"], "file_transcript");
        assert_eq!(parsed_ft["file_id"], "F_CLIP");
        assert_eq!(parsed_ft["text"], "Here is the live release walkthrough");

        let ft_md =
            export_file_transcript(&store, &ws, &FileId::from("F_CLIP"), ExportFormat::Markdown)
                .unwrap()
                .unwrap();
        assert!(ft_md.content.contains("# Media Transcript — `F_CLIP`"));
        assert!(
            ft_md
                .content
                .contains("Here is the live release walkthrough")
        );

        assert!(
            export_file_transcript(
                &store,
                &ws,
                &FileId::from("F_MISSING"),
                ExportFormat::Markdown
            )
            .unwrap()
            .is_none()
        );

        // Huddle transcript export
        let ht_md = export_huddle_transcript(&store, &ws, "R_CALL1", ExportFormat::Markdown)
            .unwrap()
            .expect("huddle exists");
        assert_eq!(ht_md.item_count, 1);
        assert!(
            ht_md
                .content
                .contains("# Huddle Transcript — #engineering (`R_CALL1`)")
        );
        assert!(ht_md.content.contains("**Alice**"));
        assert!(ht_md.content.contains("Let us cut the release branch now"));

        let ht_json = export_huddle_transcript(&store, &ws, "R_CALL1", ExportFormat::Json)
            .unwrap()
            .unwrap();
        let parsed_ht: serde_json::Value = serde_json::from_str(&ht_json.content).unwrap();
        assert_eq!(parsed_ht["kind"], "huddle_transcript");
        assert_eq!(parsed_ht["call_id"], "R_CALL1");
        assert_eq!(parsed_ht["count"], 1);
    }
}
