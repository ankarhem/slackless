//! Unified cross-workspace local FTS5 search across all stored SQLite archives.
//!
//! Queries one or more workspaces (whether colocated in a single SQLite database or stored as
//! separate `<team_id>.sqlite` archive files on disk), covering messages, snippets, canvases,
//! `file_transcript` entries (both message-attached and standalone), and huddle transcripts,
//! returning full workspace attribution per hit.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use crate::SharedStore;
use crate::paths;
use crate::store::{
    HuddleTranscriptSegment, Result as StoreResult, SearchQuery, SearchSort, Store,
    WorkspaceStoredMessage, extract_inline_file_body, extract_inline_file_transcript,
};
use crate::types::{ChannelId, File, FileId, FileMode, Message, TeamId, Ts};

/// Content classification for a unified search hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchHitKind {
    Message,
    Snippet,
    Canvas,
    FileTranscript,
}

impl SearchHitKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Message => "Message",
            Self::Snippet => "Snippet",
            Self::Canvas => "Canvas",
            Self::FileTranscript => "Transcript",
        }
    }
}

/// One unified search hit with full workspace, channel, author, and transcript attribution.
#[derive(Debug, Clone, PartialEq)]
pub struct UnifiedSearchHit {
    pub workspace: TeamId,
    pub workspace_name: Option<String>,
    pub channel: ChannelId,
    pub channel_name: Option<String>,
    pub user_name: Option<String>,
    pub message: Message,
    pub deleted: bool,
    pub kind: SearchHitKind,
    /// When the hit came from a file transcript (attached or standalone), its file ID and excerpt.
    pub transcript_file_id: Option<FileId>,
    pub transcript_excerpt: Option<String>,
    /// BM25 relevance rank (lower/more negative = better) when `SearchSort::Relevant`.
    pub rank: Option<f64>,
    /// Semantic cosine similarity or RRF fused score when searched via `SearchMode::Semantic` / `Hybrid`.
    pub semantic_score: Option<f32>,
}

/// One cross-workspace huddle transcript hit with workspace, channel, and speaker attribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnifiedTranscriptHit {
    pub workspace: TeamId,
    pub workspace_name: Option<String>,
    pub channel_name: Option<String>,
    pub speaker_name: Option<String>,
    pub segment: HuddleTranscriptSegment,
}

/// Aggregated result of a unified cross-workspace local search.
#[derive(Debug, Clone, PartialEq)]
pub struct UnifiedSearchResults {
    pub workspaces_searched: Vec<(TeamId, Option<String>)>,
    pub hits: Vec<UnifiedSearchHit>,
}

/// A mounted workspace store to search in-memory / via an active `SharedStore`.
#[derive(Clone)]
pub struct WorkspaceSearchTarget {
    pub workspace: TeamId,
    pub workspace_name: Option<String>,
    pub store: SharedStore,
}

/// Discover every `<team_id>.sqlite` archive file in `dir`, sorted by team ID.
pub fn discover_disk_archives(dir: &Path) -> Vec<(TeamId, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(TeamId, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.to_string_lossy() == "sqlite")
        })
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?.to_owned();
            if stem.is_empty() || stem == paths::SEMANTIC_INDEX_STEM {
                return None;
            }
            Some((TeamId::from(stem), path))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// Discover every local `<team_id>.sqlite` archive in [`paths::app_data_dir`].
pub fn discover_local_archives() -> Vec<(TeamId, PathBuf)> {
    let Some(dir) = paths::app_data_dir() else {
        return Vec::new();
    };
    discover_disk_archives(&dir)
}

/// Open an existing SQLite archive read-only (falling back once to RW if WAL lacks its shm file).
fn open_archive_ro(path: &Path) -> StoreResult<Store> {
    match Store::open_read_only(path) {
        Ok(store) => Ok(store),
        Err(_) => Store::open(path),
    }
}

/// Search across all workspaces present inside a single [`Store`], returning unified attributed hits.
pub fn search_single_store_all_workspaces(
    store: &Store,
    query: &SearchQuery,
    sort: SearchSort,
    limit: usize,
) -> StoreResult<UnifiedSearchResults> {
    if limit == 0 {
        return Ok(UnifiedSearchResults {
            workspaces_searched: Vec::new(),
            hits: Vec::new(),
        });
    }
    let workspaces = store.list_workspaces()?;
    let mut meta: HashMap<TeamId, WorkspaceMeta> = HashMap::with_capacity(workspaces.len());
    let mut searched = Vec::with_capacity(workspaces.len());
    for ws in &workspaces {
        let m = load_workspace_meta(store, ws, None)?;
        searched.push((ws.clone(), m.name.clone()));
        meta.insert(ws.clone(), m);
    }

    let mut seen_keys: HashSet<(TeamId, ChannelId, Ts)> = HashSet::new();
    let mut seen_files: HashSet<(TeamId, FileId)> = HashSet::new();
    let mut hits = Vec::new();

    let raw_msgs = store.search_all_workspaces(query, sort, limit)?;
    for hit in raw_msgs {
        record_message_hit(
            store,
            hit,
            &meta,
            query,
            &mut seen_keys,
            &mut seen_files,
            &mut hits,
        );
    }

    append_standalone_file_transcripts(
        store,
        None,
        &meta,
        query,
        limit,
        &mut seen_files,
        &mut hits,
    )?;

    sort_and_truncate_hits(&mut hits, sort, limit);
    Ok(UnifiedSearchResults {
        workspaces_searched: searched,
        hits,
    })
}

/// Search across a list of open [`WorkspaceSearchTarget`]s (used by the GUI's "All workspaces"
/// search across active + parked live bridges and multi-workspace fixture archives).
pub fn search_across_stores(
    targets: &[WorkspaceSearchTarget],
    query: &SearchQuery,
    sort: SearchSort,
    limit: usize,
) -> StoreResult<UnifiedSearchResults> {
    if limit == 0 || targets.is_empty() {
        return Ok(UnifiedSearchResults {
            workspaces_searched: Vec::new(),
            hits: Vec::new(),
        });
    }
    let mut searched: Vec<(TeamId, Option<String>)> = Vec::with_capacity(targets.len());
    let mut seen_ws: HashSet<TeamId> = HashSet::new();
    let mut seen_keys: HashSet<(TeamId, ChannelId, Ts)> = HashSet::new();
    let mut seen_files: HashSet<(TeamId, FileId)> = HashSet::new();
    let mut hits: Vec<UnifiedSearchHit> = Vec::new();

    for target in targets {
        let store = target.store.lock().unwrap_or_else(PoisonError::into_inner);
        let ws = &target.workspace;
        let ws_meta = load_workspace_meta(&store, ws, target.workspace_name.as_deref())?;
        if seen_ws.insert(ws.clone()) {
            searched.push((ws.clone(), ws_meta.name.clone()));
        }
        let mut meta_map = HashMap::with_capacity(1);
        meta_map.insert(ws.clone(), ws_meta);

        let raw_msgs = store.search_workspace_ranked(ws, query, sort, limit)?;
        for hit in raw_msgs {
            record_message_hit(
                &store,
                hit,
                &meta_map,
                query,
                &mut seen_keys,
                &mut seen_files,
                &mut hits,
            );
        }

        append_standalone_file_transcripts(
            &store,
            Some(ws),
            &meta_map,
            query,
            limit,
            &mut seen_files,
            &mut hits,
        )?;
    }

    sort_and_truncate_hits(&mut hits, sort, limit);
    Ok(UnifiedSearchResults {
        workspaces_searched: searched,
        hits,
    })
}

/// Search across a list of `(TeamId, PathBuf)` disk archive files, merging and ranking hits
/// across every stored workspace.
pub fn search_disk_archives(
    archives: &[(TeamId, PathBuf)],
    query: &SearchQuery,
    sort: SearchSort,
    limit: usize,
) -> StoreResult<UnifiedSearchResults> {
    if limit == 0 || archives.is_empty() {
        return Ok(UnifiedSearchResults {
            workspaces_searched: Vec::new(),
            hits: Vec::new(),
        });
    }
    let mut searched: Vec<(TeamId, Option<String>)> = Vec::with_capacity(archives.len());
    let mut seen_ws: HashSet<TeamId> = HashSet::new();
    let mut seen_keys: HashSet<(TeamId, ChannelId, Ts)> = HashSet::new();
    let mut seen_files: HashSet<(TeamId, FileId)> = HashSet::new();
    let mut hits: Vec<UnifiedSearchHit> = Vec::new();

    for (ws, path) in archives {
        if !path.is_file() {
            continue;
        }
        let store = open_archive_ro(path)?;
        // A single SQLite file may hold one workspace (`ws`) or multiple workspaces.
        let mut workspaces = store.list_workspaces()?;
        if workspaces.is_empty() {
            workspaces.push(ws.clone());
        }
        let mut meta_map = HashMap::with_capacity(workspaces.len());
        for inner_ws in &workspaces {
            let m = load_workspace_meta(&store, inner_ws, None)?;
            if seen_ws.insert(inner_ws.clone()) {
                searched.push((inner_ws.clone(), m.name.clone()));
            }
            meta_map.insert(inner_ws.clone(), m);
        }

        let raw_msgs = store.search_all_workspaces(query, sort, limit)?;
        for hit in raw_msgs {
            record_message_hit(
                &store,
                hit,
                &meta_map,
                query,
                &mut seen_keys,
                &mut seen_files,
                &mut hits,
            );
        }

        append_standalone_file_transcripts(
            &store,
            None,
            &meta_map,
            query,
            limit,
            &mut seen_files,
            &mut hits,
        )?;
    }

    sort_and_truncate_hits(&mut hits, sort, limit);
    Ok(UnifiedSearchResults {
        workspaces_searched: searched,
        hits,
    })
}

/// Search across EVERY locally stored SQLite archive in [`paths::app_data_dir`].
pub fn search_all_local_workspaces(
    query: &SearchQuery,
    sort: SearchSort,
    limit: usize,
) -> StoreResult<UnifiedSearchResults> {
    let archives = discover_local_archives();
    search_disk_archives(&archives, query, sort, limit)
}

/// Search huddle transcripts across multiple [`WorkspaceSearchTarget`]s, newest first.
pub fn search_huddle_transcripts_across_stores(
    targets: &[WorkspaceSearchTarget],
    query: &str,
    lang: Option<&str>,
    limit: usize,
) -> Vec<UnifiedTranscriptHit> {
    if limit == 0 || targets.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut seen: HashSet<(TeamId, i64)> = HashSet::new();
    for target in targets {
        let store = target.store.lock().unwrap_or_else(PoisonError::into_inner);
        let ws = &target.workspace;
        let Ok(meta) = load_workspace_meta(&store, ws, target.workspace_name.as_deref()) else {
            continue;
        };
        let Ok(segments) = store.search_huddle_transcripts(ws, query, None, lang, limit) else {
            continue;
        };
        for segment in segments {
            if !seen.insert((ws.clone(), segment.id)) {
                continue;
            }
            let channel_name = meta.channels.get(segment.channel.as_str()).cloned();
            let speaker_name = segment
                .speaker_id
                .as_ref()
                .and_then(|u| meta.users.get(u.as_str()).cloned());
            out.push(UnifiedTranscriptHit {
                workspace: ws.clone(),
                workspace_name: meta.name.clone(),
                channel_name,
                speaker_name,
                segment,
            });
        }
    }
    out.sort_by(|a, b| {
        b.segment
            .at_ms
            .cmp(&a.segment.at_ms)
            .then_with(|| b.segment.id.cmp(&a.segment.id))
    });
    out.truncate(limit);
    out
}

/// Search huddle transcripts across on-disk `<team_id>.sqlite` archive files, newest first.
pub fn search_huddle_transcripts_in_disk_archives(
    archives: &[(TeamId, PathBuf)],
    query: &str,
    lang: Option<&str>,
    limit: usize,
) -> Vec<UnifiedTranscriptHit> {
    if limit == 0 || archives.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut seen: HashSet<(TeamId, i64)> = HashSet::new();
    for (ws, path) in archives {
        if !path.is_file() {
            continue;
        }
        let Ok(store) = open_archive_ro(path) else {
            continue;
        };
        let mut workspaces = store.list_workspaces().unwrap_or_default();
        if workspaces.is_empty() {
            workspaces.push(ws.clone());
        }
        for inner_ws in &workspaces {
            let Ok(meta) = load_workspace_meta(&store, inner_ws, None) else {
                continue;
            };
            let Ok(segments) = store.search_huddle_transcripts(inner_ws, query, None, lang, limit)
            else {
                continue;
            };
            for segment in segments {
                if !seen.insert((inner_ws.clone(), segment.id)) {
                    continue;
                }
                let channel_name = meta.channels.get(segment.channel.as_str()).cloned();
                let speaker_name = segment
                    .speaker_id
                    .as_ref()
                    .and_then(|u| meta.users.get(u.as_str()).cloned());
                out.push(UnifiedTranscriptHit {
                    workspace: inner_ws.clone(),
                    workspace_name: meta.name.clone(),
                    channel_name,
                    speaker_name,
                    segment,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        b.segment
            .at_ms
            .cmp(&a.segment.at_ms)
            .then_with(|| b.segment.id.cmp(&a.segment.id))
    });
    out.truncate(limit);
    out
}

struct WorkspaceMeta {
    name: Option<String>,
    channels: HashMap<String, String>,
    users: HashMap<String, String>,
}

fn load_workspace_meta(
    store: &Store,
    workspace: &TeamId,
    fallback_name: Option<&str>,
) -> StoreResult<WorkspaceMeta> {
    let team_name = store
        .team(workspace)?
        .map(|t| t.name)
        .filter(|n| !n.trim().is_empty())
        .or_else(|| {
            fallback_name
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(ToOwned::to_owned)
        });
    let channels = store.channel_names(workspace)?;
    let users_list = store.list_users(workspace, None, true)?;
    let mut users = HashMap::with_capacity(users_list.len());
    for u in users_list {
        let display = u
            .display_name
            .or(u.real_name)
            .or_else(|| (!u.name.is_empty()).then_some(u.name));
        if let Some(label) = display {
            users.insert(u.id.as_str().to_owned(), label);
        }
    }
    Ok(WorkspaceMeta {
        name: team_name,
        channels,
        users,
    })
}

fn record_message_hit(
    store: &Store,
    hit: WorkspaceStoredMessage,
    meta_map: &HashMap<TeamId, WorkspaceMeta>,
    query: &SearchQuery,
    seen_keys: &mut HashSet<(TeamId, ChannelId, Ts)>,
    seen_files: &mut HashSet<(TeamId, FileId)>,
    hits: &mut Vec<UnifiedSearchHit>,
) {
    let key = (
        hit.workspace.clone(),
        hit.channel.clone(),
        hit.message.ts.clone(),
    );
    if !seen_keys.insert(key) {
        return;
    }
    for f in &hit.message.files {
        if !f.id.as_str().is_empty() {
            seen_files.insert((hit.workspace.clone(), f.id.clone()));
        }
    }

    let meta = meta_map.get(&hit.workspace);
    let workspace_name = meta.and_then(|m| m.name.clone());
    let channel_name = meta
        .and_then(|m| m.channels.get(hit.channel.as_str()).cloned())
        .or_else(|| {
            store
                .conversation_summary(&hit.workspace, &hit.channel)
                .ok()
                .flatten()
                .and_then(|c| c.name)
        });
    let user_name = hit
        .message
        .user
        .as_ref()
        .and_then(|u| meta.and_then(|m| m.users.get(u.as_str()).cloned()))
        .or_else(|| hit.message.bot_profile.as_ref().map(|b| b.name.clone()));

    let (kind, transcript_file_id, transcript_excerpt) =
        classify_hit(store, &hit.workspace, &hit.message, query);

    hits.push(UnifiedSearchHit {
        workspace: hit.workspace,
        workspace_name,
        channel: hit.channel,
        channel_name,
        user_name,
        message: hit.message,
        deleted: hit.deleted,
        kind,
        transcript_file_id,
        transcript_excerpt,
        rank: hit.rank,
        semantic_score: None,
    });
}

/// Append standalone `file_transcript` rows that are not already represented by a matched message.
fn append_standalone_file_transcripts(
    store: &Store,
    workspace: Option<&TeamId>,
    meta_map: &HashMap<TeamId, WorkspaceMeta>,
    query: &SearchQuery,
    limit: usize,
    seen_files: &mut HashSet<(TeamId, FileId)>,
    hits: &mut Vec<UnifiedSearchHit>,
) -> StoreResult<()> {
    // Only synthesize standalone file_transcript hits when there are no channel/user/pin/reaction
    // filters that exclude unattached files (a `has:file` filter is compatible!).
    let f = &query.filters;
    let has_incompatible_filter = !f.in_channels.is_empty()
        || !f.from_users.is_empty()
        || f.includes_user.is_some()
        || f.threads_only
        || f.has.iter().any(|h| *h != crate::store::Has::File);
    if has_incompatible_filter {
        return Ok(());
    }

    let transcripts = store.search_file_transcripts(workspace, query, limit)?;
    for entry in transcripts {
        if !seen_files.insert((entry.workspace.clone(), entry.transcript.file_id.clone())) {
            continue;
        }
        // Respect date bounds if provided.
        if let Some(before) = f.before
            && entry.transcript.updated_at >= before.unix_start()
        {
            continue;
        }
        if let Some(after) = f.after
            && entry.transcript.updated_at < after.unix_start() + 86_400
        {
            continue;
        }
        let meta = meta_map.get(&entry.workspace);
        let channel = entry
            .channel
            .clone()
            .unwrap_or_else(|| ChannelId::from(format!("file:{}", entry.transcript.file_id)));
        let channel_name = entry
            .channel
            .as_ref()
            .and_then(|c| meta.and_then(|m| m.channels.get(c.as_str()).cloned()));
        let ts = entry
            .message_ts
            .clone()
            .unwrap_or_else(|| Ts::from(format!("{}.000000", entry.transcript.updated_at.max(0))));
        let excerpt = match_excerpt(&entry.transcript.text, &query.terms);
        let synthetic_msg = Message {
            ts,
            channel: Some(channel.clone()),
            text: entry.transcript.text.clone(),
            files: vec![File {
                id: entry.transcript.file_id.clone(),
                title: Some(format!("Transcript ({})", entry.transcript.file_id)),
                ..Default::default()
            }],
            ..Default::default()
        };
        hits.push(UnifiedSearchHit {
            workspace: entry.workspace,
            workspace_name: meta.and_then(|m| m.name.clone()),
            channel,
            channel_name,
            user_name: Some(entry.transcript.source),
            message: synthetic_msg,
            deleted: false,
            kind: SearchHitKind::FileTranscript,
            transcript_file_id: Some(entry.transcript.file_id),
            transcript_excerpt: excerpt,
            rank: None,
            semantic_score: None,
        });
    }
    Ok(())
}

/// Classify whether a message matched primarily as a transcript, canvas, snippet, or plain message.
/// Stored transcripts are read per attached file of the matched hits (at most `limit`), never
/// whole-workspace per search.
fn classify_hit(
    store: &Store,
    workspace: &TeamId,
    message: &Message,
    query: &SearchQuery,
) -> (SearchHitKind, Option<FileId>, Option<String>) {
    if message.files.is_empty() {
        return (SearchHitKind::Message, None, None);
    }
    let text_lower = message.text.to_lowercase();
    let terms_lower: Vec<String> = query
        .terms
        .iter()
        .map(|t| t.trim_end_matches('*').to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    let text_matches =
        !terms_lower.is_empty() && terms_lower.iter().all(|t| text_lower.contains(t.as_str()));

    for file in &message.files {
        let transcript = store
            .file_transcript(workspace, &file.id)
            .ok()
            .flatten()
            .map(|t| t.text)
            .or_else(|| extract_inline_file_transcript(file));
        if let Some(tr) = transcript {
            let excerpt = match_excerpt(&tr, &query.terms);
            if excerpt.is_some() || (!text_matches && !tr.is_empty()) {
                return (
                    SearchHitKind::FileTranscript,
                    Some(file.id.clone()),
                    excerpt.or_else(|| Some(truncate_chars(&tr, 140))),
                );
            }
        }
    }

    if !text_matches {
        for file in &message.files {
            if is_canvas_file(file) {
                let body = extract_inline_file_body(file).unwrap_or_default();
                let excerpt = match_excerpt(&body, &query.terms);
                return (SearchHitKind::Canvas, Some(file.id.clone()), excerpt);
            }
            if is_snippet_file(file) {
                let body = extract_inline_file_body(file).unwrap_or_default();
                let excerpt = match_excerpt(&body, &query.terms);
                return (SearchHitKind::Snippet, Some(file.id.clone()), excerpt);
            }
        }
    }

    (SearchHitKind::Message, None, None)
}

pub(crate) fn is_canvas_file(file: &File) -> bool {
    matches!(
        file.filetype.as_deref(),
        Some("quip" | "canvas" | "slack_canvas")
    ) || file.extra.contains_key("canvas_markdown")
        || file.extra.contains_key("canvas_text")
        || file.extra.contains_key("document_content")
}

pub(crate) fn is_snippet_file(file: &File) -> bool {
    file.mode == Some(FileMode::Snippet)
        || file
            .preview
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty())
        || file.extra.contains_key("plain_text")
}

pub(crate) fn match_excerpt(body: &str, terms: &[String]) -> Option<String> {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    let chars: Vec<char> = collapsed.chars().collect();
    // Map each UTF-8 byte offset of `lowered` back to its source char index in `chars` so
    // Unicode case-folding that changes byte or char counts (e.g. `İ` -> `i\u{307}`, `ẞ` -> `ß`)
    // never slices on an invalid UTF-8 byte boundary or misaligns the excerpt window.
    let mut lowered = String::with_capacity(collapsed.len());
    let mut byte_to_char_ix: Vec<usize> = Vec::with_capacity(collapsed.len());
    for (ci, ch) in chars.iter().enumerate() {
        for lc in ch.to_lowercase() {
            lowered.push(lc);
            byte_to_char_ix.resize(lowered.len(), ci);
        }
    }
    let matched_pos = terms
        .iter()
        .map(|t| t.trim_end_matches('*').to_lowercase())
        .filter(|t| !t.is_empty())
        .find_map(|t| lowered.find(t.as_str()))?;
    if chars.len() <= 120 {
        return Some(collapsed);
    }
    let char_pos = byte_to_char_ix
        .get(matched_pos)
        .copied()
        .unwrap_or(0)
        .min(chars.len());
    let start = char_pos.saturating_sub(36);
    let end = (start + 120).min(chars.len());
    let slice: String = chars[start..end].iter().collect();
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(slice.trim());
    if end < chars.len() {
        out.push('…');
    }
    Some(out)
}

pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        collapsed
    } else {
        let mut s: String = collapsed
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect();
        s.push('…');
        s
    }
}

fn sort_and_truncate_hits(hits: &mut Vec<UnifiedSearchHit>, sort: SearchSort, limit: usize) {
    match sort {
        SearchSort::Recent => {
            hits.sort_by(|a, b| {
                b.message
                    .ts
                    .cmp(&a.message.ts)
                    .then_with(|| a.workspace.cmp(&b.workspace))
            });
        }
        SearchSort::Relevant => {
            hits.sort_by(|a, b| match (a.rank, b.rank) {
                (Some(ra), Some(rb)) => ra
                    .total_cmp(&rb)
                    .then_with(|| b.message.ts.cmp(&a.message.ts)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => b.message.ts.cmp(&a.message.ts),
            });
        }
    }
    hits.truncate(limit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::types::{Conversation, Profile, Team, User, UserId};

    fn seed_workspace(
        store: &mut Store,
        team_id: &str,
        team_name: &str,
        channel_id: &str,
        channel_name: &str,
        user_id: &str,
        user_name: &str,
    ) {
        let ws = TeamId::from(team_id);
        store
            .upsert_team(
                &ws,
                &Team {
                    id: ws.clone(),
                    name: team_name.into(),
                    domain: team_name.to_lowercase(),
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: ChannelId::from(channel_id),
                    name: Some(channel_name.into()),
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
                    id: UserId::from(user_id),
                    name: user_name.to_lowercase(),
                    profile: Profile {
                        display_name: Some(user_name.into()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap();
    }

    #[test]
    fn search_across_stores_merges_workspaces_and_classifies_content_kinds() {
        let mut s1 = Store::open_in_memory().unwrap();
        seed_workspace(&mut s1, "T1", "Acme", "C1", "general", "U1", "Alice");
        s1.upsert_message(
            &TeamId::from("T1"),
            &ChannelId::from("C1"),
            &Message {
                ts: Ts::from("1700000100.000100"),
                user: Some(UserId::from("U1")),
                text: "Launching nebula v2 today".into(),
                ..Default::default()
            },
        )
        .unwrap();
        // Standalone file_transcript in T1.
        s1.upsert_file_transcript(
            &TeamId::from("T1"),
            &FileId::from("F_SOLO"),
            "parakeet-v3",
            "Nebula standalone clip note",
        )
        .unwrap();

        let mut s2 = Store::open_in_memory().unwrap();
        seed_workspace(&mut s2, "T2", "Beta Corp", "C2", "infra", "U2", "Bob");
        let mut snip_msg = Message {
            ts: Ts::from("1700000200.000100"),
            user: Some(UserId::from("U2")),
            text: "Config snippet".into(),
            files: vec![File {
                id: FileId::from("F_SNIP"),
                name: Some("nebula.toml".into()),
                mode: Some(FileMode::Snippet),
                preview: Some("cluster = \"nebula-prod\"".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        s2.upsert_message(&TeamId::from("T2"), &ChannelId::from("C2"), &snip_msg)
            .unwrap();

        let mut canvas_file = File {
            id: FileId::from("F_CANVAS"),
            title: Some("Runbook".into()),
            filetype: Some("quip".into()),
            ..Default::default()
        };
        canvas_file.extra.insert(
            "canvas_markdown".into(),
            serde_json::json!("Nebula incident runbook"),
        );
        snip_msg.ts = Ts::from("1700000300.000100");
        snip_msg.text = "Shared runbook".into();
        snip_msg.files = vec![canvas_file];
        s2.upsert_message(&TeamId::from("T2"), &ChannelId::from("C2"), &snip_msg)
            .unwrap();
        // A message that only matches through the stored transcript of its attached file.
        s2.upsert_message(
            &TeamId::from("T2"),
            &ChannelId::from("C2"),
            &Message {
                ts: Ts::from("1700000250.000100"),
                user: Some(UserId::from("U2")),
                text: "Recording of the launch call".into(),
                files: vec![File {
                    id: FileId::from("F_CLIP"),
                    name: Some("launch.mp4".into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .unwrap();
        s2.upsert_file_transcript(
            &TeamId::from("T2"),
            &FileId::from("F_CLIP"),
            "parakeet-v3",
            "Nebula launch walkthrough",
        )
        .unwrap();

        let targets = vec![
            WorkspaceSearchTarget {
                workspace: TeamId::from("T1"),
                workspace_name: Some("Acme".into()),
                store: Arc::new(Mutex::new(s1)),
            },
            WorkspaceSearchTarget {
                workspace: TeamId::from("T2"),
                workspace_name: Some("Beta Corp".into()),
                store: Arc::new(Mutex::new(s2)),
            },
        ];

        let res = search_across_stores(
            &targets,
            &SearchQuery::parse("nebula"),
            SearchSort::Recent,
            20,
        )
        .unwrap();
        assert_eq!(res.workspaces_searched.len(), 2);
        assert_eq!(res.hits.len(), 5);

        let attached_tx = res
            .hits
            .iter()
            .find(|h| h.transcript_file_id == Some(FileId::from("F_CLIP")))
            .expect("attached transcript hit");
        assert_eq!(
            (attached_tx.kind, attached_tx.transcript_excerpt.as_deref()),
            (
                SearchHitKind::FileTranscript,
                Some("Nebula launch walkthrough")
            )
        );

        let canvas_hit = res
            .hits
            .iter()
            .find(|h| h.kind == SearchHitKind::Canvas)
            .expect("canvas hit");
        assert_eq!(canvas_hit.workspace, TeamId::from("T2"));
        assert_eq!(canvas_hit.workspace_name.as_deref(), Some("Beta Corp"));
        assert_eq!(canvas_hit.channel_name.as_deref(), Some("infra"));
        assert_eq!(canvas_hit.user_name.as_deref(), Some("Bob"));

        let snippet_hit = res
            .hits
            .iter()
            .find(|h| h.kind == SearchHitKind::Snippet)
            .expect("snippet hit");
        assert_eq!(snippet_hit.workspace, TeamId::from("T2"));

        let msg_hit = res
            .hits
            .iter()
            .find(|h| h.kind == SearchHitKind::Message)
            .expect("message hit");
        assert_eq!(msg_hit.workspace, TeamId::from("T1"));
        assert_eq!(msg_hit.workspace_name.as_deref(), Some("Acme"));
        assert_eq!(msg_hit.user_name.as_deref(), Some("Alice"));

        let tx_hit = res
            .hits
            .iter()
            .find(|h| h.kind == SearchHitKind::FileTranscript)
            .expect("standalone transcript hit");
        assert_eq!(tx_hit.workspace, TeamId::from("T1"));
        assert_eq!(tx_hit.transcript_file_id, Some(FileId::from("F_SOLO")));
    }

    #[test]
    fn search_disk_archives_discovers_and_merges_multiple_sqlite_files() {
        let dir = std::env::temp_dir().join(format!(
            "slackless-core-unified-search-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        {
            let mut s1 = Store::open(dir.join("T_ALPHA.sqlite")).unwrap();
            seed_workspace(&mut s1, "T_ALPHA", "Alpha", "C1", "eng", "U1", "Ada");
            s1.upsert_message(
                &TeamId::from("T_ALPHA"),
                &ChannelId::from("C1"),
                &Message {
                    ts: Ts::from("1700001000.000100"),
                    user: Some(UserId::from("U1")),
                    text: "hyperdrive telemetry online".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        {
            let mut s2 = Store::open(dir.join("T_OMEGA.sqlite")).unwrap();
            seed_workspace(&mut s2, "T_OMEGA", "Omega", "C9", "launch", "U9", "Linus");
            s2.upsert_message(
                &TeamId::from("T_OMEGA"),
                &ChannelId::from("C9"),
                &Message {
                    ts: Ts::from("1700002000.000100"),
                    user: Some(UserId::from("U9")),
                    text: "hyperdrive ignition verified".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }

        let archives = discover_disk_archives(&dir);
        assert_eq!(archives.len(), 2);
        let res = search_disk_archives(
            &archives,
            &SearchQuery::parse("hyperdrive"),
            SearchSort::Recent,
            10,
        )
        .unwrap();
        assert_eq!(res.hits.len(), 2);
        assert_eq!(res.hits[0].workspace, TeamId::from("T_OMEGA"));
        assert_eq!(res.hits[0].workspace_name.as_deref(), Some("Omega"));
        assert_eq!(res.hits[1].workspace, TeamId::from("T_ALPHA"));
        assert_eq!(res.hits[1].workspace_name.as_deref(), Some("Alpha"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn match_excerpt_handles_expanding_unicode_case_folding_without_panicking() {
        // `İ` (U+0130, 2 UTF-8 bytes -> `i\u{307}`, 3 UTF-8 bytes) and `ẞ` (3 bytes -> `ß`, 2 bytes)
        // shift byte offsets between `collapsed` and `lowered` across a >120-char body.
        let prefix = "İstanbul Straße ".repeat(12);
        let suffix = " åäö monitoring tail".repeat(8);
        let body = format!("{prefix}zephyrprobe rollout verified{suffix}");
        assert!(body.chars().count() > 120);

        let excerpt = match_excerpt(&body, &["zephyrprobe".to_owned()])
            .expect("must extract excerpt around match");
        assert!(excerpt.contains("zephyrprobe"));
        assert!(excerpt.starts_with('…'));
        assert!(excerpt.ends_with('…'));

        // Also match a term containing `İ` directly.
        let dotted = format!("{}İSTANBUL_CANARY{}", "ÅÄÖ ".repeat(40), " end".repeat(20));
        let ex2 = match_excerpt(&dotted, &["i\u{307}stanbul_canary".to_owned()])
            .expect("must match folded Turkish dotted I");
        assert!(ex2.contains("İSTANBUL_CANARY"));
    }
}
