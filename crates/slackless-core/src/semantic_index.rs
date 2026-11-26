//! Incremental sidecar vector indexing and Reciprocal Rank Fusion (RRF) hybrid search across
//! messages, snippets, canvases, file transcripts, and huddle transcripts across ALL workspaces.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use serde::{Deserialize, Serialize};
use slackless_store::{
    DEFAULT_RRF_K, Has, SearchFilters, SearchMode, SearchQuery, SearchSort, SemanticDocInput,
    SemanticDocKind, SemanticHit, SemanticIndex, SemanticIndexStats, Store,
    extract_inline_file_body, extract_inline_file_transcript, fuse_rrf,
};
use slackless_types::{ChannelId, File, FileId, Message, TeamId, Ts};

use crate::domain::mrkdwn::mrkdwn_to_plain;
use crate::embeddings::{Embedder, content_hash};
use crate::paths;
use crate::search::{
    SearchHitKind, UnifiedSearchHit, UnifiedSearchResults, UnifiedTranscriptHit,
    WorkspaceSearchTarget, is_canvas_file, is_snippet_file, match_excerpt, search_across_stores,
    search_disk_archives, search_huddle_transcripts_across_stores,
    search_huddle_transcripts_in_disk_archives, truncate_chars,
};

const EMBED_BATCH_SIZE: usize = 32;
/// Minimum cosine similarity for semantic candidates (filters orthogonal noise).
const DEFAULT_MIN_SIMILARITY: f32 = 0.08;

/// Incremental progress update emitted during [`index_store_documents`] / [`index_disk_archives`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticIndexProgress {
    pub workspace: TeamId,
    pub processed: usize,
    pub total: usize,
    pub embedded: usize,
    pub skipped: usize,
}

/// Summary report returned after indexing one or more workspace archives.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticIndexReport {
    pub workspaces_indexed: Vec<String>,
    pub total_seen: usize,
    pub embedded: usize,
    pub skipped_unchanged: usize,
    pub pruned: usize,
    pub model_changed: bool,
    pub model_id: String,
    pub dims: usize,
}

/// Open the default sidecar `semantic_index.sqlite` read-write (creating it if missing).
pub fn open_default_semantic_index() -> std::result::Result<SemanticIndex, String> {
    let path = paths::semantic_index_path().ok_or_else(|| {
        "could not resolve app data directory for semantic_index.sqlite".to_owned()
    })?;
    SemanticIndex::open(&path).map_err(|e| format!("failed to open {}: {e}", path.display()))
}

/// Open the default sidecar `semantic_index.sqlite` if it exists on disk (read-only preferred).
pub fn open_existing_semantic_index() -> std::result::Result<Option<SemanticIndex>, String> {
    let Some(path) = paths::semantic_index_path() else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }
    match SemanticIndex::open_read_only(&path) {
        Ok(idx) => Ok(Some(idx)),
        Err(_) => SemanticIndex::open(&path)
            .map(Some)
            .map_err(|e| format!("failed to open {}: {e}", path.display())),
    }
}

/// Read current statistics for the default sidecar `semantic_index.sqlite` (empty stats if missing).
pub fn default_semantic_index_stats() -> SemanticIndexStats {
    match open_existing_semantic_index() {
        Ok(Some(idx)) => idx.stats().unwrap_or_default(),
        _ => SemanticIndexStats::default(),
    }
}

/// Pending document ready for hash comparison and batched embedding.
struct CandidateDoc {
    workspace: TeamId,
    kind: SemanticDocKind,
    doc_id: String,
    channel_id: String,
    ts: String,
    text: String,
    content_hash: String,
}

fn classify_message_doc_kind(message: &Message) -> SemanticDocKind {
    for file in &message.files {
        if is_canvas_file(file) {
            return SemanticDocKind::Canvas;
        }
        if is_snippet_file(file) {
            return SemanticDocKind::Snippet;
        }
    }
    SemanticDocKind::Message
}

fn compose_message_index_text(
    message: &Message,
    file_names: &str,
    file_transcripts: &str,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    let plain = mrkdwn_to_plain(&message.text);
    let trimmed = plain.trim();
    if !trimmed.is_empty() {
        parts.push(trimmed.to_owned());
    }
    let fn_trimmed = file_names.trim();
    if !fn_trimmed.is_empty() {
        parts.push(fn_trimmed.to_owned());
    }
    let ft_trimmed = file_transcripts.trim();
    if !ft_trimmed.is_empty() {
        parts.push(ft_trimmed.to_owned());
    } else {
        for file in &message.files {
            if let Some(tr) = extract_inline_file_transcript(file) {
                parts.push(tr);
            }
            if let Some(body) = extract_inline_file_body(file) {
                parts.push(body);
            }
        }
    }
    // Truncate very long canvases/snippets cleanly to ~4000 chars so hashing & tokenization stay fast.
    let joined = parts.join("\n");
    if joined.chars().count() > 4000 {
        joined.chars().take(4000).collect()
    } else {
        joined
    }
}

/// Incrementally index all messages, snippets, canvases, file transcripts, and huddle transcripts
/// from `store` into `index` using `embedder`.
///
/// Unchanged documents (matching `(workspace, doc_kind, doc_id, content_hash, dims)`) are skipped.
pub fn index_store_documents(
    store: &Store,
    index: &mut SemanticIndex,
    embedder: &dyn Embedder,
    workspace_filter: Option<&TeamId>,
    mut on_progress: impl FnMut(SemanticIndexProgress),
) -> std::result::Result<SemanticIndexReport, String> {
    let model_id = embedder.model_id().to_owned();
    let dims = embedder.dims();
    let model_changed = index
        .ensure_model(&model_id, dims)
        .map_err(|e| format!("failed to update semantic_meta: {e}"))?;

    let workspaces: Vec<TeamId> = match workspace_filter {
        Some(ws) => vec![ws.clone()],
        None => store
            .list_workspaces()
            .map_err(|e| format!("failed to list workspaces: {e}"))?,
    };

    let mut report = SemanticIndexReport {
        model_changed,
        model_id,
        dims,
        ..Default::default()
    };

    for ws in workspaces {
        report.workspaces_indexed.push(ws.as_str().to_owned());
        let existing = index
            .existing_hashes(&ws)
            .map_err(|e| format!("failed to read existing hashes for {ws}: {e}"))?;
        let mut live_keys: HashSet<(SemanticDocKind, String)> = HashSet::new();
        let mut candidates: Vec<CandidateDoc> = Vec::new();

        // 1. Messages, snippets, and canvases from `message_current`.
        let messages = store
            .list_messages_for_indexing(&ws)
            .map_err(|e| format!("failed to list messages for {ws}: {e}"))?;
        for (channel, msg, file_names, file_transcripts) in messages {
            let text = compose_message_index_text(&msg, &file_names, &file_transcripts);
            if text.trim().is_empty() {
                continue;
            }
            let kind = classify_message_doc_kind(&msg);
            let doc_id = format!("{}:{}", channel.as_str(), msg.ts.as_str());
            let hash = content_hash(&text);
            live_keys.insert((kind, doc_id.clone()));
            candidates.push(CandidateDoc {
                workspace: ws.clone(),
                kind,
                doc_id,
                channel_id: channel.as_str().to_owned(),
                ts: msg.ts.as_str().to_owned(),
                text,
                content_hash: hash,
            });
        }

        // 2. Standalone / persisted file transcripts (`file_transcript`).
        let file_trs = store
            .list_file_transcripts_for_indexing(&ws)
            .map_err(|e| format!("failed to list file transcripts for {ws}: {e}"))?;
        for entry in file_trs {
            let text = entry.transcript.text.trim();
            if text.is_empty() {
                continue;
            }
            let truncated = if text.chars().count() > 4000 {
                text.chars().take(4000).collect::<String>()
            } else {
                text.to_owned()
            };
            let doc_id = entry.transcript.file_id.as_str().to_owned();
            let hash = content_hash(&truncated);
            live_keys.insert((SemanticDocKind::FileTranscript, doc_id.clone()));
            candidates.push(CandidateDoc {
                workspace: ws.clone(),
                kind: SemanticDocKind::FileTranscript,
                doc_id,
                channel_id: entry
                    .channel
                    .map(|c| c.as_str().to_owned())
                    .unwrap_or_default(),
                ts: entry
                    .message_ts
                    .map(|t| t.as_str().to_owned())
                    .unwrap_or_else(|| format!("{}.000000", entry.transcript.updated_at.max(0))),
                text: truncated,
                content_hash: hash,
            });
        }

        // 3. Spoken huddle transcript segments (`huddle_transcript`).
        let huddle_trs = store
            .list_huddle_transcripts_for_indexing(&ws)
            .map_err(|e| format!("failed to list huddle transcripts for {ws}: {e}"))?;
        for seg in huddle_trs {
            let text = seg.text.trim();
            if text.is_empty() {
                continue;
            }
            let doc_id = seg.id.to_string();
            let hash = content_hash(text);
            live_keys.insert((SemanticDocKind::HuddleTranscript, doc_id.clone()));
            candidates.push(CandidateDoc {
                workspace: ws.clone(),
                kind: SemanticDocKind::HuddleTranscript,
                doc_id,
                channel_id: seg.channel.as_str().to_owned(),
                ts: format!("{}.000000", seg.at_ms.div_euclid(1_000).max(0)),
                text: text.to_owned(),
                content_hash: hash,
            });
        }

        let total_ws = candidates.len();
        report.total_seen += total_ws;

        let mut to_embed: Vec<CandidateDoc> = Vec::new();
        let mut skipped_ws = 0usize;
        for doc in candidates {
            if let Some((prev_hash, prev_dims)) = existing.get(&(doc.kind, doc.doc_id.clone()))
                && *prev_hash == doc.content_hash
                && *prev_dims == dims
            {
                skipped_ws += 1;
                continue;
            }
            to_embed.push(doc);
        }
        report.skipped_unchanged += skipped_ws;

        let mut embedded_ws = 0usize;
        for chunk in to_embed.chunks(EMBED_BATCH_SIZE) {
            let texts: Vec<&str> = chunk.iter().map(|d| d.text.as_str()).collect();
            let vecs = embedder.embed_batch(&texts)?;
            if vecs.len() != chunk.len() {
                return Err(format!(
                    "embedder returned {} vectors for {} inputs",
                    vecs.len(),
                    chunk.len()
                ));
            }
            let batch: Vec<SemanticDocInput> = chunk
                .iter()
                .zip(vecs)
                .map(|(doc, embedding)| SemanticDocInput {
                    workspace: doc.workspace.clone(),
                    kind: doc.kind,
                    doc_id: doc.doc_id.clone(),
                    channel_id: doc.channel_id.clone(),
                    ts: doc.ts.clone(),
                    content_hash: doc.content_hash.clone(),
                    embedding,
                })
                .collect();
            index
                .upsert_batch(&batch)
                .map_err(|e| format!("failed to upsert semantic batch: {e}"))?;
            embedded_ws += batch.len();
            on_progress(SemanticIndexProgress {
                workspace: ws.clone(),
                processed: skipped_ws + embedded_ws,
                total: total_ws,
                embedded: embedded_ws,
                skipped: skipped_ws,
            });
        }
        if to_embed.is_empty() {
            on_progress(SemanticIndexProgress {
                workspace: ws.clone(),
                processed: skipped_ws,
                total: total_ws,
                embedded: 0,
                skipped: skipped_ws,
            });
        }
        report.embedded += embedded_ws;

        let pruned = index
            .prune_workspace(&ws, &live_keys)
            .map_err(|e| format!("failed to prune stale vectors for {ws}: {e}"))?;
        report.pruned += pruned;
    }

    Ok(report)
}

/// Incrementally index a set of on-disk `<team_id>.sqlite` archive files into `index`.
pub fn index_disk_archives(
    archives: &[(TeamId, PathBuf)],
    index: &mut SemanticIndex,
    embedder: &dyn Embedder,
    mut on_progress: impl FnMut(SemanticIndexProgress),
) -> std::result::Result<SemanticIndexReport, String> {
    let mut combined = SemanticIndexReport {
        model_id: embedder.model_id().to_owned(),
        dims: embedder.dims(),
        ..Default::default()
    };
    let mut seen_ws = HashSet::new();
    for (ws, path) in archives {
        if !path.is_file() {
            continue;
        }
        let store = open_archive_ro(path)
            .map_err(|e| format!("failed to open archive {}: {e}", path.display()))?;
        let mut inner_workspaces = store.list_workspaces().unwrap_or_default();
        if inner_workspaces.is_empty() {
            inner_workspaces.push(ws.clone());
        }
        for inner_ws in inner_workspaces {
            if !seen_ws.insert(inner_ws.clone()) {
                continue;
            }
            let part =
                index_store_documents(&store, index, embedder, Some(&inner_ws), &mut on_progress)?;
            combined.workspaces_indexed.extend(part.workspaces_indexed);
            combined.total_seen += part.total_seen;
            combined.embedded += part.embedded;
            combined.skipped_unchanged += part.skipped_unchanged;
            combined.pruned += part.pruned;
            combined.model_changed |= part.model_changed;
        }
    }
    Ok(combined)
}

/// Unified hybrid search across active in-memory/mounted [`WorkspaceSearchTarget`]s.
///
/// - `SearchMode::Keyword`: pure FTS5 (`search_across_stores`).
/// - `SearchMode::Semantic`: pure vector similarity from `index`, hydrated & filtered against `targets`.
/// - `SearchMode::Hybrid`: runs FTS5 + Semantic in tandem and blends rankings via Reciprocal Rank
///   Fusion (`fuse_rrf`).
pub fn hybrid_search_stores(
    targets: &[WorkspaceSearchTarget],
    index: &SemanticIndex,
    embedder: &dyn Embedder,
    query: &SearchQuery,
    mode: SearchMode,
    sort: SearchSort,
    limit: usize,
) -> std::result::Result<UnifiedSearchResults, String> {
    if limit == 0 || targets.is_empty() {
        return Ok(UnifiedSearchResults {
            workspaces_searched: Vec::new(),
            hits: Vec::new(),
        });
    }
    let free_text = query.free_text();
    if mode == SearchMode::Keyword || free_text.trim().is_empty() {
        return search_across_stores(targets, query, sort, limit).map_err(|e| e.to_string());
    }

    let candidate_limit = (limit.saturating_mul(3)).max(40);
    let workspaces: Vec<TeamId> = targets.iter().map(|t| t.workspace.clone()).collect();
    let query_vec = embedder.embed_query(&free_text)?;
    let sem_raw = index
        .search(
            &query_vec,
            Some(&workspaces),
            Some(&[
                SemanticDocKind::Message,
                SemanticDocKind::Snippet,
                SemanticDocKind::Canvas,
                SemanticDocKind::FileTranscript,
            ]),
            candidate_limit,
            DEFAULT_MIN_SIMILARITY,
        )
        .map_err(|e| format!("semantic vector search failed: {e}"))?;

    let semantic_hits = hydrate_semantic_hits_from_targets(targets, &sem_raw, query)?;

    match mode {
        SearchMode::Keyword => unreachable!(),
        SearchMode::Semantic => {
            let mut hits = semantic_hits;
            if sort == SearchSort::Recent {
                hits.sort_by(|a, b| {
                    b.message
                        .ts
                        .cmp(&a.message.ts)
                        .then_with(|| a.workspace.cmp(&b.workspace))
                });
            }
            hits.truncate(limit);
            let searched = targets
                .iter()
                .map(|t| (t.workspace.clone(), t.workspace_name.clone()))
                .collect();
            Ok(UnifiedSearchResults {
                workspaces_searched: searched,
                hits,
            })
        }
        SearchMode::Hybrid => {
            let fts_res =
                search_across_stores(targets, query, SearchSort::Relevant, candidate_limit)
                    .map_err(|e| e.to_string())?;
            let mut fused = fuse_unified_hits(fts_res.hits, semantic_hits, sort, limit);
            fused.truncate(limit);
            Ok(UnifiedSearchResults {
                workspaces_searched: fts_res.workspaces_searched,
                hits: fused,
            })
        }
    }
}

/// Unified hybrid search across on-disk `<team_id>.sqlite` archive files.
pub fn hybrid_search_disk_archives(
    archives: &[(TeamId, PathBuf)],
    index: &SemanticIndex,
    embedder: &dyn Embedder,
    query: &SearchQuery,
    mode: SearchMode,
    sort: SearchSort,
    limit: usize,
) -> std::result::Result<UnifiedSearchResults, String> {
    if limit == 0 || archives.is_empty() {
        return Ok(UnifiedSearchResults {
            workspaces_searched: Vec::new(),
            hits: Vec::new(),
        });
    }
    let free_text = query.free_text();
    if mode == SearchMode::Keyword || free_text.trim().is_empty() {
        return search_disk_archives(archives, query, sort, limit).map_err(|e| e.to_string());
    }

    let candidate_limit = (limit.saturating_mul(3)).max(40);
    let mut opened: Vec<(TeamId, Option<String>, Store)> = Vec::new();
    let mut workspace_ids: Vec<TeamId> = Vec::new();
    let mut seen_ws: HashSet<TeamId> = HashSet::new();

    for (ws, path) in archives {
        if !path.is_file() {
            continue;
        }
        let store = open_archive_ro(path)
            .map_err(|e| format!("failed to open archive {}: {e}", path.display()))?;
        let mut inner_ws = store.list_workspaces().unwrap_or_default();
        if inner_ws.is_empty() {
            inner_ws.push(ws.clone());
        }
        let first_ws = inner_ws[0].clone();
        for w in &inner_ws {
            if seen_ws.insert(w.clone()) {
                workspace_ids.push(w.clone());
            }
        }
        let name = store
            .team(&first_ws)
            .ok()
            .flatten()
            .map(|t| t.name)
            .filter(|n| !n.trim().is_empty());
        opened.push((first_ws, name, store));
    }

    let query_vec = embedder.embed_query(&free_text)?;
    let sem_raw = index
        .search(
            &query_vec,
            Some(&workspace_ids),
            Some(&[
                SemanticDocKind::Message,
                SemanticDocKind::Snippet,
                SemanticDocKind::Canvas,
                SemanticDocKind::FileTranscript,
            ]),
            candidate_limit,
            DEFAULT_MIN_SIMILARITY,
        )
        .map_err(|e| format!("semantic vector search failed: {e}"))?;

    let semantic_hits = hydrate_semantic_hits_from_stores(&opened, &sem_raw, query)?;

    match mode {
        SearchMode::Keyword => unreachable!(),
        SearchMode::Semantic => {
            let mut hits = semantic_hits;
            if sort == SearchSort::Recent {
                hits.sort_by(|a, b| {
                    b.message
                        .ts
                        .cmp(&a.message.ts)
                        .then_with(|| a.workspace.cmp(&b.workspace))
                });
            }
            hits.truncate(limit);
            let searched = opened.into_iter().map(|(ws, name, _)| (ws, name)).collect();
            Ok(UnifiedSearchResults {
                workspaces_searched: searched,
                hits,
            })
        }
        SearchMode::Hybrid => {
            let fts_res =
                search_disk_archives(archives, query, SearchSort::Relevant, candidate_limit)
                    .map_err(|e| e.to_string())?;
            let fused = fuse_unified_hits(fts_res.hits, semantic_hits, sort, limit);
            Ok(UnifiedSearchResults {
                workspaces_searched: fts_res.workspaces_searched,
                hits: fused,
            })
        }
    }
}

/// Hybrid / semantic search across huddle transcripts in mounted [`WorkspaceSearchTarget`]s.
pub fn hybrid_search_huddle_transcripts_across_stores(
    targets: &[WorkspaceSearchTarget],
    index: &SemanticIndex,
    embedder: &dyn Embedder,
    query: &str,
    lang: Option<&str>,
    mode: SearchMode,
    limit: usize,
) -> std::result::Result<Vec<UnifiedTranscriptHit>, String> {
    if limit == 0 || targets.is_empty() {
        return Ok(Vec::new());
    }
    if mode == SearchMode::Keyword || query.trim().is_empty() {
        return Ok(search_huddle_transcripts_across_stores(
            targets, query, lang, limit,
        ));
    }

    let candidate_limit = (limit.saturating_mul(3)).max(40);
    let workspaces: Vec<TeamId> = targets.iter().map(|t| t.workspace.clone()).collect();
    let query_vec = embedder.embed_query(query)?;
    let sem_raw = index
        .search(
            &query_vec,
            Some(&workspaces),
            Some(&[SemanticDocKind::HuddleTranscript]),
            candidate_limit,
            DEFAULT_MIN_SIMILARITY,
        )
        .map_err(|e| format!("semantic huddle transcript search failed: {e}"))?;

    let mut by_ws: HashMap<&str, &WorkspaceSearchTarget> = HashMap::new();
    for t in targets {
        by_ws.insert(t.workspace.as_str(), t);
    }

    let mut semantic_hits = Vec::new();
    let lang_prefix = lang
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase);
    for hit in sem_raw {
        let Ok(seg_id) = hit.doc_id.parse::<i64>() else {
            continue;
        };
        let Some(target) = by_ws.get(hit.workspace.as_str()) else {
            continue;
        };
        let store = target.store.lock().unwrap_or_else(PoisonError::into_inner);
        let Ok(Some(segment)) = store.huddle_transcript_segment(&hit.workspace, seg_id) else {
            continue;
        };
        if let Some(ref lp) = lang_prefix
            && !segment
                .lang
                .as_deref()
                .is_some_and(|l| l.to_ascii_lowercase().starts_with(lp))
        {
            continue;
        }
        let channels = store.channel_names(&hit.workspace).unwrap_or_default();
        let channel_name = channels.get(segment.channel.as_str()).cloned();
        let speaker_name = segment
            .speaker_id
            .as_ref()
            .and_then(|u| store.user_display(&hit.workspace, u).ok().flatten());
        semantic_hits.push(UnifiedTranscriptHit {
            workspace: hit.workspace,
            workspace_name: target.workspace_name.clone(),
            channel_name,
            speaker_name,
            segment,
        });
    }

    if mode == SearchMode::Semantic {
        semantic_hits.truncate(limit);
        return Ok(semantic_hits);
    }

    let fts_hits = search_huddle_transcripts_across_stores(targets, query, lang, candidate_limit);
    Ok(fuse_transcript_hits(fts_hits, semantic_hits, limit))
}

/// Hybrid / semantic search across huddle transcripts in on-disk archive files.
pub fn hybrid_search_huddle_transcripts_in_disk_archives(
    archives: &[(TeamId, PathBuf)],
    index: &SemanticIndex,
    embedder: &dyn Embedder,
    query: &str,
    lang: Option<&str>,
    mode: SearchMode,
    limit: usize,
) -> std::result::Result<Vec<UnifiedTranscriptHit>, String> {
    if limit == 0 || archives.is_empty() {
        return Ok(Vec::new());
    }
    if mode == SearchMode::Keyword || query.trim().is_empty() {
        return Ok(search_huddle_transcripts_in_disk_archives(
            archives, query, lang, limit,
        ));
    }

    let candidate_limit = (limit.saturating_mul(3)).max(40);
    let workspaces: Vec<TeamId> = archives.iter().map(|(w, _)| w.clone()).collect();
    let query_vec = embedder.embed_query(query)?;
    let sem_raw = index
        .search(
            &query_vec,
            Some(&workspaces),
            Some(&[SemanticDocKind::HuddleTranscript]),
            candidate_limit,
            DEFAULT_MIN_SIMILARITY,
        )
        .map_err(|e| format!("semantic huddle transcript search failed: {e}"))?;

    let mut path_by_ws: HashMap<&str, &Path> = HashMap::new();
    for (ws, p) in archives {
        path_by_ws.insert(ws.as_str(), p.as_path());
    }
    let mut store_cache: HashMap<String, Store> = HashMap::new();
    let lang_prefix = lang
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase);

    let mut semantic_hits = Vec::new();
    for hit in sem_raw {
        let Ok(seg_id) = hit.doc_id.parse::<i64>() else {
            continue;
        };
        if !store_cache.contains_key(hit.workspace.as_str())
            && let Some(path) = path_by_ws.get(hit.workspace.as_str())
            && let Ok(s) = open_archive_ro(path)
        {
            store_cache.insert(hit.workspace.as_str().to_owned(), s);
        }
        let Some(store) = store_cache.get(hit.workspace.as_str()) else {
            continue;
        };
        let Ok(Some(segment)) = store.huddle_transcript_segment(&hit.workspace, seg_id) else {
            continue;
        };
        if let Some(ref lp) = lang_prefix
            && !segment
                .lang
                .as_deref()
                .is_some_and(|l| l.to_ascii_lowercase().starts_with(lp))
        {
            continue;
        }
        let ws_name = store
            .team(&hit.workspace)
            .ok()
            .flatten()
            .map(|t| t.name)
            .filter(|n| !n.trim().is_empty());
        let channels = store.channel_names(&hit.workspace).unwrap_or_default();
        let channel_name = channels.get(segment.channel.as_str()).cloned();
        let speaker_name = segment
            .speaker_id
            .as_ref()
            .and_then(|u| store.user_display(&hit.workspace, u).ok().flatten());
        semantic_hits.push(UnifiedTranscriptHit {
            workspace: hit.workspace,
            workspace_name: ws_name,
            channel_name,
            speaker_name,
            segment,
        });
    }

    if mode == SearchMode::Semantic {
        semantic_hits.truncate(limit);
        return Ok(semantic_hits);
    }

    let fts_hits =
        search_huddle_transcripts_in_disk_archives(archives, query, lang, candidate_limit);
    Ok(fuse_transcript_hits(fts_hits, semantic_hits, limit))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum HitKey {
    Message(TeamId, ChannelId, Ts),
    FileTranscript(TeamId, FileId),
}

fn hit_key(hit: &UnifiedSearchHit) -> HitKey {
    if hit.kind == SearchHitKind::FileTranscript
        && hit.channel.as_str().starts_with("file:")
        && let Some(fid) = &hit.transcript_file_id
    {
        return HitKey::FileTranscript(hit.workspace.clone(), fid.clone());
    }
    HitKey::Message(
        hit.workspace.clone(),
        hit.channel.clone(),
        hit.message.ts.clone(),
    )
}

fn fuse_unified_hits(
    fts_hits: Vec<UnifiedSearchHit>,
    semantic_hits: Vec<UnifiedSearchHit>,
    sort: SearchSort,
    limit: usize,
) -> Vec<UnifiedSearchHit> {
    let mut by_key: HashMap<HitKey, UnifiedSearchHit> = HashMap::new();
    let mut fts_keys = Vec::with_capacity(fts_hits.len());
    for hit in fts_hits {
        let key = hit_key(&hit);
        fts_keys.push(key.clone());
        by_key.entry(key).or_insert(hit);
    }

    let mut sem_keys = Vec::with_capacity(semantic_hits.len());
    for hit in semantic_hits {
        let key = hit_key(&hit);
        sem_keys.push(key.clone());
        let sem_score = hit.semantic_score;
        by_key
            .entry(key)
            .and_modify(|existing| {
                if existing.semantic_score.is_none() {
                    existing.semantic_score = sem_score;
                }
            })
            .or_insert(hit);
    }

    let fused = fuse_rrf(&[(1.0, fts_keys), (1.0, sem_keys)], DEFAULT_RRF_K);
    let mut out: Vec<UnifiedSearchHit> = Vec::with_capacity(fused.len().min(limit));
    for (key, rrf) in fused {
        if let Some(mut hit) = by_key.remove(&key) {
            // Preserve the blended RRF score on `semantic_score` if no raw cosine was set,
            // and store negative RRF on `rank` so downstream BM25-style ascending sort works too.
            hit.rank = Some(-rrf);
            if hit.semantic_score.is_none() {
                hit.semantic_score = Some(rrf as f32);
            }
            out.push(hit);
        }
    }

    if sort == SearchSort::Recent {
        out.sort_by(|a, b| {
            b.message
                .ts
                .cmp(&a.message.ts)
                .then_with(|| a.workspace.cmp(&b.workspace))
        });
    }
    out.truncate(limit);
    out
}

fn fuse_transcript_hits(
    fts_hits: Vec<UnifiedTranscriptHit>,
    semantic_hits: Vec<UnifiedTranscriptHit>,
    limit: usize,
) -> Vec<UnifiedTranscriptHit> {
    let mut by_key: HashMap<(TeamId, i64), UnifiedTranscriptHit> = HashMap::new();
    let mut fts_keys = Vec::with_capacity(fts_hits.len());
    for hit in fts_hits {
        let key = (hit.workspace.clone(), hit.segment.id);
        fts_keys.push(key.clone());
        by_key.entry(key).or_insert(hit);
    }
    let mut sem_keys = Vec::with_capacity(semantic_hits.len());
    for hit in semantic_hits {
        let key = (hit.workspace.clone(), hit.segment.id);
        sem_keys.push(key.clone());
        by_key.entry(key).or_insert(hit);
    }
    let fused = fuse_rrf(&[(1.0, fts_keys), (1.0, sem_keys)], DEFAULT_RRF_K);
    fused
        .into_iter()
        .filter_map(|(k, _)| by_key.remove(&k))
        .take(limit)
        .collect()
}

struct HydrateMeta {
    workspace_name: Option<String>,
    channels: HashMap<String, String>,
    users: HashMap<String, (String, Option<String>, Option<String>)>,
}

fn load_hydrate_meta(store: &Store, ws: &TeamId, fallback_name: Option<&str>) -> HydrateMeta {
    let workspace_name = store
        .team(ws)
        .ok()
        .flatten()
        .map(|t| t.name)
        .filter(|n| !n.trim().is_empty())
        .or_else(|| {
            fallback_name
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(ToOwned::to_owned)
        });
    let channels = store.channel_names(ws).unwrap_or_default();
    let mut users = HashMap::new();
    if let Ok(list) = store.list_users(ws, None, true) {
        for u in list {
            users.insert(
                u.id.as_str().to_owned(),
                (u.name, u.display_name, u.real_name),
            );
        }
    }
    HydrateMeta {
        workspace_name,
        channels,
        users,
    }
}

fn hydrate_semantic_hits_from_targets(
    targets: &[WorkspaceSearchTarget],
    raw_hits: &[SemanticHit],
    query: &SearchQuery,
) -> std::result::Result<Vec<UnifiedSearchHit>, String> {
    let mut by_ws: HashMap<&str, &WorkspaceSearchTarget> = HashMap::new();
    for t in targets {
        by_ws.insert(t.workspace.as_str(), t);
    }
    let mut meta_cache: HashMap<TeamId, HydrateMeta> = HashMap::new();
    let mut out = Vec::new();
    let mut seen_keys: HashSet<HitKey> = HashSet::new();

    for sem in raw_hits {
        let Some(target) = by_ws.get(sem.workspace.as_str()) else {
            continue;
        };
        let store = target.store.lock().unwrap_or_else(PoisonError::into_inner);
        let meta = meta_cache.entry(sem.workspace.clone()).or_insert_with(|| {
            load_hydrate_meta(&store, &sem.workspace, target.workspace_name.as_deref())
        });
        if let Some(hit) = hydrate_one_hit(&store, sem, meta, query) {
            let key = hit_key(&hit);
            if seen_keys.insert(key) {
                out.push(hit);
            }
        }
    }
    Ok(out)
}

fn hydrate_semantic_hits_from_stores(
    stores: &[(TeamId, Option<String>, Store)],
    raw_hits: &[SemanticHit],
    query: &SearchQuery,
) -> std::result::Result<Vec<UnifiedSearchHit>, String> {
    let mut by_ws: HashMap<&str, &(TeamId, Option<String>, Store)> = HashMap::new();
    for entry in stores {
        by_ws.insert(entry.0.as_str(), entry);
    }
    let mut meta_cache: HashMap<TeamId, HydrateMeta> = HashMap::new();
    let mut out = Vec::new();
    let mut seen_keys: HashSet<HitKey> = HashSet::new();

    for sem in raw_hits {
        let Some((_, fallback, store)) = by_ws.get(sem.workspace.as_str()) else {
            continue;
        };
        let meta = meta_cache
            .entry(sem.workspace.clone())
            .or_insert_with(|| load_hydrate_meta(store, &sem.workspace, fallback.as_deref()));
        if let Some(hit) = hydrate_one_hit(store, sem, meta, query) {
            let key = hit_key(&hit);
            if seen_keys.insert(key) {
                out.push(hit);
            }
        }
    }
    Ok(out)
}

fn hydrate_one_hit(
    store: &Store,
    sem: &SemanticHit,
    meta: &HydrateMeta,
    query: &SearchQuery,
) -> Option<UnifiedSearchHit> {
    match sem.kind {
        SemanticDocKind::Message | SemanticDocKind::Snippet | SemanticDocKind::Canvas => {
            let (ch_str, ts_str) = if !sem.channel_id.is_empty() && !sem.ts.is_empty() {
                (sem.channel_id.as_str(), sem.ts.as_str())
            } else {
                sem.doc_id.split_once(':')?
            };
            let channel = ChannelId::from(ch_str);
            let ts = Ts::from(ts_str);
            let stored = store.message(&sem.workspace, &channel, &ts).ok()??;
            if !message_matches_filters(&stored.message, &channel, stored.deleted, meta, query) {
                return None;
            }
            let channel_name = meta.channels.get(channel.as_str()).cloned();
            let user_name = stored
                .message
                .user
                .as_ref()
                .and_then(|u| meta.users.get(u.as_str()))
                .and_then(|(n, d, r)| {
                    d.clone()
                        .or_else(|| r.clone())
                        .or_else(|| (!n.is_empty()).then(|| n.clone()))
                })
                .or_else(|| stored.message.bot_profile.as_ref().map(|b| b.name.clone()));
            let kind = match sem.kind {
                SemanticDocKind::Canvas => SearchHitKind::Canvas,
                SemanticDocKind::Snippet => SearchHitKind::Snippet,
                _ => SearchHitKind::Message,
            };
            let (transcript_file_id, transcript_excerpt) = match kind {
                SearchHitKind::Canvas | SearchHitKind::Snippet => stored
                    .message
                    .files
                    .first()
                    .map(|f| {
                        let body = extract_inline_file_body(f).unwrap_or_default();
                        let ex = match_excerpt(&body, &query.terms)
                            .or_else(|| (!body.is_empty()).then(|| truncate_chars(&body, 140)));
                        (Some(f.id.clone()), ex)
                    })
                    .unwrap_or((None, None)),
                _ => (None, None),
            };
            Some(UnifiedSearchHit {
                workspace: sem.workspace.clone(),
                workspace_name: meta.workspace_name.clone(),
                channel,
                channel_name,
                user_name,
                message: stored.message,
                deleted: stored.deleted,
                kind,
                transcript_file_id,
                transcript_excerpt,
                rank: Some(-f64::from(sem.score)),
                semantic_score: Some(sem.score),
            })
        }
        SemanticDocKind::FileTranscript => {
            let f = &query.filters;
            let has_incompatible_filter = !f.in_channels.is_empty()
                || !f.from_users.is_empty()
                || f.includes_user.is_some()
                || f.threads_only
                || f.has.iter().any(|h| *h != Has::File);
            if has_incompatible_filter {
                return None;
            }
            let fid = FileId::from(sem.doc_id.as_str());
            let tr = store.file_transcript(&sem.workspace, &fid).ok()??;
            if let Some(before) = f.before
                && tr.updated_at >= before.unix_start()
            {
                return None;
            }
            if let Some(after) = f.after
                && tr.updated_at < after.unix_start() + 86_400
            {
                return None;
            }
            let lower = tr.text.to_lowercase();
            if query
                .excluded_terms
                .iter()
                .any(|ex| lower.contains(&ex.trim_end_matches('*').to_lowercase()))
            {
                return None;
            }
            let channel = if !sem.channel_id.is_empty() {
                ChannelId::from(sem.channel_id.as_str())
            } else {
                ChannelId::from(format!("file:{}", tr.file_id))
            };
            let channel_name = meta.channels.get(channel.as_str()).cloned();
            let ts = if !sem.ts.is_empty() {
                Ts::from(sem.ts.as_str())
            } else {
                Ts::from(format!("{}.000000", tr.updated_at.max(0)))
            };
            let excerpt = match_excerpt(&tr.text, &query.terms)
                .or_else(|| Some(truncate_chars(&tr.text, 140)));
            let synthetic_msg = Message {
                ts,
                channel: Some(channel.clone()),
                text: tr.text.clone(),
                files: vec![File {
                    id: tr.file_id.clone(),
                    title: Some(format!("Transcript ({})", tr.file_id)),
                    ..Default::default()
                }],
                ..Default::default()
            };
            Some(UnifiedSearchHit {
                workspace: sem.workspace.clone(),
                workspace_name: meta.workspace_name.clone(),
                channel,
                channel_name,
                user_name: Some(tr.source),
                message: synthetic_msg,
                deleted: false,
                kind: SearchHitKind::FileTranscript,
                transcript_file_id: Some(tr.file_id),
                transcript_excerpt: excerpt,
                rank: Some(-f64::from(sem.score)),
                semantic_score: Some(sem.score),
            })
        }
        SemanticDocKind::HuddleTranscript => None,
    }
}

fn message_matches_filters(
    message: &Message,
    channel: &ChannelId,
    deleted: bool,
    meta: &HydrateMeta,
    query: &SearchQuery,
) -> bool {
    let f: &SearchFilters = &query.filters;
    if deleted && !f.include_deleted {
        return false;
    }
    let ch_name = meta.channels.get(channel.as_str()).map(String::as_str);
    let matches_channel = |cand: &str| {
        channel.as_str().eq_ignore_ascii_case(cand)
            || ch_name.is_some_and(|n| n.eq_ignore_ascii_case(cand))
    };
    if !f.in_channels.is_empty() && !f.in_channels.iter().any(|c| matches_channel(c)) {
        return false;
    }
    if f.exclude_channels.iter().any(|c| matches_channel(c)) {
        return false;
    }

    let user_id = message.user.as_ref().map(|u| u.as_str());
    let user_tuple = user_id.and_then(|id| meta.users.get(id));
    let matches_user = |cand: &str| {
        user_id.is_some_and(|id| id.eq_ignore_ascii_case(cand))
            || user_tuple.is_some_and(|(name, disp, real)| {
                name.eq_ignore_ascii_case(cand)
                    || disp
                        .as_deref()
                        .is_some_and(|d| d.eq_ignore_ascii_case(cand))
                    || real
                        .as_deref()
                        .is_some_and(|r| r.eq_ignore_ascii_case(cand))
            })
    };
    if !f.from_users.is_empty() && !f.from_users.iter().any(|u| matches_user(u)) {
        return false;
    }
    if f.exclude_users.iter().any(|u| matches_user(u)) {
        return false;
    }

    if let Some(secs) = message.ts.secs() {
        if let Some(before) = f.before
            && secs >= before.unix_start()
        {
            return false;
        }
        if let Some(after) = f.after
            && secs < after.unix_start() + 86_400
        {
            return false;
        }
    }

    for has in &f.has {
        let ok = match has {
            Has::Link => {
                message.text.contains("http://")
                    || message.text.contains("https://")
                    || message
                        .attachments
                        .iter()
                        .any(|a| a.from_url.is_some() || a.title_link.is_some())
            }
            Has::File => !message.files.is_empty(),
            Has::Pin => !message.pinned_to.is_empty(),
            Has::Reaction => !message.reactions.is_empty(),
        };
        if !ok {
            return false;
        }
    }

    if let Some(uid) = &f.includes_user
        && !message.text.contains(&format!("<@{uid}>"))
    {
        return false;
    }
    if f.threads_only && message.thread_ts.is_none() {
        return false;
    }
    if !query.excluded_terms.is_empty() {
        let lower = message.text.to_lowercase();
        if query
            .excluded_terms
            .iter()
            .any(|ex| lower.contains(&ex.trim_end_matches('*').to_lowercase()))
        {
            return false;
        }
    }
    true
}

fn open_archive_ro(path: &Path) -> slackless_store::Result<Store> {
    match Store::open_read_only(path) {
        Ok(store) => Ok(store),
        Err(_) => Store::open(path),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use slackless_store::NewHuddleTranscriptSegment;
    use slackless_types::{Conversation, FileMode, Profile, Team, User, UserId};

    use super::*;
    use crate::embeddings::HashProjectionEmbedder;
    use crate::search::discover_disk_archives;

    fn seed_ws(
        store: &mut Store,
        ws_id: &str,
        ws_name: &str,
        ch_id: &str,
        ch_name: &str,
        user_id: &str,
        user_name: &str,
    ) {
        let ws = TeamId::from(ws_id);
        store
            .upsert_team(
                &ws,
                &Team {
                    id: ws.clone(),
                    name: ws_name.into(),
                    domain: ws_name.to_lowercase(),
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: ChannelId::from(ch_id),
                    name: Some(ch_name.into()),
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
                    name: user_name.into(),
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
    fn incremental_indexing_skips_unchanged_and_prunes_deleted() {
        let mut store = Store::open_in_memory().unwrap();
        let ws = TeamId::from("T_INC");
        seed_ws(&mut store, "T_INC", "Acme", "C_ENG", "eng", "U1", "alice");

        store
            .upsert_messages(
                &ws,
                &ChannelId::from("C_ENG"),
                &[
                    Message {
                        ts: Ts::from("1700000100.000000"),
                        user: Some(UserId::from("U1")),
                        text: "Kubernetes pod autoscaler rolled out cleanly".into(),
                        ..Default::default()
                    },
                    Message {
                        ts: Ts::from("1700000200.000000"),
                        user: Some(UserId::from("U1")),
                        text: "Attached architecture spec".into(),
                        files: vec![File {
                            id: FileId::from("F_CANVAS"),
                            name: Some("Architecture Canvas".into()),
                            filetype: Some("quip".into()),
                            preview: Some("Database sharding strategy and read replicas".into()),
                            ..Default::default()
                        }],
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
                "Spoken walkthrough of quarterly latency targets",
            )
            .unwrap();
        store
            .insert_huddle_transcript_segment(
                &ws,
                &NewHuddleTranscriptSegment {
                    channel: &ChannelId::from("C_ENG"),
                    call_id: "R_CALL1",
                    thread_root_ts: None,
                    speaker_id: Some(&UserId::from("U1")),
                    lang: Some("en"),
                    source: "parakeet-v3",
                    text: "Let us double check the database failover runbook",
                    at_ms: 1_700_000_300_000,
                },
            )
            .unwrap();

        let embedder = HashProjectionEmbedder::new(64);
        let mut index = SemanticIndex::open_in_memory().unwrap();

        // First pass embeds all 4 docs (1 message, 1 canvas, 1 file transcript, 1 huddle transcript).
        let r1 = index_store_documents(&store, &mut index, &embedder, None, |_| {}).unwrap();
        assert_eq!(r1.total_seen, 4);
        assert_eq!(r1.embedded, 4);
        assert_eq!(r1.skipped_unchanged, 0);
        assert_eq!(r1.pruned, 0);
        let st = index.stats().unwrap();
        assert_eq!(st.messages, 1);
        assert_eq!(st.canvases, 1);
        assert_eq!(st.file_transcripts, 1);
        assert_eq!(st.huddle_transcripts, 1);

        // Second pass skips all 4 unchanged docs (0 re-embedded!).
        let r2 = index_store_documents(&store, &mut index, &embedder, None, |_| {}).unwrap();
        assert_eq!(r2.total_seen, 4);
        assert_eq!(r2.embedded, 0);
        assert_eq!(r2.skipped_unchanged, 4);
        assert_eq!(r2.pruned, 0);

        // Deleting a message causes incremental re-index to prune its vector.
        store
            .record_deletion(
                &ws,
                &ChannelId::from("C_ENG"),
                &Ts::from("1700000100.000000"),
                Some(&Ts::from("1700000101.000000")),
                None,
            )
            .unwrap();
        let r3 = index_store_documents(&store, &mut index, &embedder, None, |_| {}).unwrap();
        assert_eq!(r3.total_seen, 3);
        assert_eq!(r3.embedded, 0);
        assert_eq!(r3.skipped_unchanged, 3);
        assert_eq!(r3.pruned, 1);
        assert_eq!(index.stats().unwrap().total_docs, 3);
    }

    #[test]
    fn hybrid_and_semantic_search_across_multiple_workspaces_and_filters() {
        let mut store_a = Store::open_in_memory().unwrap();
        let ws_a = TeamId::from("T_ALPHA");
        seed_ws(
            &mut store_a,
            "T_ALPHA",
            "Alpha",
            "C_A1",
            "infra",
            "U_A1",
            "alice",
        );
        store_a
            .upsert_messages(
                &ws_a,
                &ChannelId::from("C_A1"),
                &[
                    Message {
                        ts: Ts::from("1700001000.000000"),
                        user: Some(UserId::from("U_A1")),
                        text: "Deploying kubernetes cluster rollback pipeline".into(),
                        ..Default::default()
                    },
                    Message {
                        ts: Ts::from("1700001005.000000"),
                        user: Some(UserId::from("U_A1")),
                        text: "Office lunch pizza order arrives at noon".into(),
                        ..Default::default()
                    },
                ],
            )
            .unwrap();

        let mut store_b = Store::open_in_memory().unwrap();
        let ws_b = TeamId::from("T_BETA");
        seed_ws(
            &mut store_b,
            "T_BETA",
            "Beta",
            "C_B1",
            "platform",
            "U_B1",
            "bob",
        );
        store_b
            .upsert_messages(
                &ws_b,
                &ChannelId::from("C_B1"),
                &[Message {
                    ts: Ts::from("1700002000.000000"),
                    user: Some(UserId::from("U_B1")),
                    text: "Snippet attached".into(),
                    files: vec![File {
                        id: FileId::from("F_SNIP"),
                        name: Some("rollback.sh".into()),
                        mode: Some(FileMode::Snippet),
                        preview: Some("kubectl rollout undo deployment/api".into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
            )
            .unwrap();
        store_b
            .insert_huddle_transcript_segment(
                &ws_b,
                &NewHuddleTranscriptSegment {
                    channel: &ChannelId::from("C_B1"),
                    call_id: "R_BETA1",
                    thread_root_ts: None,
                    speaker_id: Some(&UserId::from("U_B1")),
                    lang: Some("en"),
                    source: "parakeet-v3",
                    text: "We rolled back the kubernetes deployment cleanly",
                    at_ms: 1_700_002_500_000,
                },
            )
            .unwrap();

        let embedder = HashProjectionEmbedder::new(128);
        let mut index = SemanticIndex::open_in_memory().unwrap();
        index_store_documents(&store_a, &mut index, &embedder, None, |_| {}).unwrap();
        index_store_documents(&store_b, &mut index, &embedder, None, |_| {}).unwrap();

        let targets = vec![
            WorkspaceSearchTarget {
                workspace: ws_a.clone(),
                workspace_name: Some("Alpha".into()),
                store: Arc::new(Mutex::new(store_a)),
            },
            WorkspaceSearchTarget {
                workspace: ws_b.clone(),
                workspace_name: Some("Beta".into()),
                store: Arc::new(Mutex::new(store_b)),
            },
        ];

        // Semantic search with a query that has partial morphological overlap ("deployment rollout")
        // finds both Alpha's message and Beta's snippet across workspaces.
        let q = SearchQuery::parse("deployment rollout kubernetes");
        let sem = hybrid_search_stores(
            &targets,
            &index,
            &embedder,
            &q,
            SearchMode::Semantic,
            SearchSort::Relevant,
            10,
        )
        .unwrap();
        assert!(
            sem.hits.len() >= 2,
            "expected cross-workspace semantic hits, got {}",
            sem.hits.len()
        );
        assert!(sem.hits.iter().any(|h| h.workspace == ws_a));
        assert!(
            sem.hits
                .iter()
                .any(|h| h.workspace == ws_b && h.kind == SearchHitKind::Snippet)
        );
        assert!(sem.hits.iter().all(|h| h.semantic_score.is_some()));

        // Hybrid RRF blends FTS5 + semantic cleanly and respects Slack modifiers (`in:#platform`).
        let q_filtered = SearchQuery::parse("in:#platform rollback");
        let hybrid = hybrid_search_stores(
            &targets,
            &index,
            &embedder,
            &q_filtered,
            SearchMode::Hybrid,
            SearchSort::Relevant,
            10,
        )
        .unwrap();
        assert_eq!(hybrid.hits.len(), 1);
        assert_eq!(hybrid.hits[0].workspace, ws_b);
        assert_eq!(hybrid.hits[0].kind, SearchHitKind::Snippet);

        // Hybrid huddle transcript search across stores finds Beta's spoken segment.
        let trs = hybrid_search_huddle_transcripts_across_stores(
            &targets,
            &index,
            &embedder,
            "kubernetes rollback",
            None,
            SearchMode::Hybrid,
            10,
        )
        .unwrap();
        assert_eq!(trs.len(), 1);
        assert_eq!(trs[0].workspace, ws_b);
        assert_eq!(trs[0].segment.call_id, "R_BETA1");
    }

    #[test]
    fn discover_disk_archives_excludes_semantic_index_sidecar() {
        let dir = std::env::temp_dir().join(format!(
            "slackless-semantic-discover-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("T100.sqlite"), b"").unwrap();
        std::fs::write(dir.join("semantic_index.sqlite"), b"").unwrap();
        let found = discover_disk_archives(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, TeamId::from("T100"));
    }
}
