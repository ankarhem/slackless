//! Local archive reads: `search`, `read`, `thread`, and `export`. Every command builds a
//! [`Payload`] here; the CLI prints it and the MCP server returns the JSON.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use slackless_core::api::{HistoryOpts, SlackApi};
use slackless_core::auth::Session;
use slackless_core::domain::mrkdwn::mrkdwn_to_plain;
use slackless_core::embeddings::{
    Embedder, EmbeddingModel, OnnxEmbedder, detect_embedding_model_setup,
};
use slackless_core::export::{
    ChannelWindow, ExportArtifact, ExportFormat as CoreExportFormat, ExportOptions, ExportRequest,
    export_request, read_channel_window,
};
use slackless_core::search::{
    SearchHitKind, UnifiedSearchHit, UnifiedSearchResults, search_disk_archives,
};
use slackless_core::semantic_index::{hybrid_search_disk_archives, open_existing_semantic_index};
use slackless_core::store::{
    SearchMode, SearchQuery, SearchSort, SemanticIndex, Store, StoredMessage,
};
use slackless_core::types::time::now_epoch_secs;
use slackless_core::types::{ChannelId, FileId, TeamId, Ts, UserId};

use super::{CmdError, Payload, all_workspace_ids, parse_since_cutoff, raw_channel_id};
use crate::archive::{self, Archive, ConversationRow};
use crate::cli::ExportFormat;
use crate::live::session::resolve_channel_with;
use crate::output::{self, MessageView};
use crate::{Ctx, emit_payload, fail, run_local};

/// Every `(workspace, archive file)` a multi-workspace command reads: each workspace known
/// from `sessions` or from an archive on disk.
pub(crate) fn all_archives(sessions: &[Session]) -> Vec<(TeamId, PathBuf)> {
    all_workspace_ids(sessions, archive::stored_team_ids())
        .into_iter()
        .filter_map(|ws| archive::store_path(&ws).map(|path| (ws, path)))
        .collect()
}

fn sort_name(sort: SearchSort) -> &'static str {
    match sort {
        SearchSort::Recent => "recent",
        SearchSort::Relevant => "relevant",
    }
}

// -----------------------------------------------------------------------------
// Search
// -----------------------------------------------------------------------------

pub(crate) struct SearchAllOpts<'a> {
    pub(crate) limit: usize,
    pub(crate) sort: SearchSort,
    pub(crate) format: Option<ExportFormat>,
    pub(crate) out: Option<&'a Path>,
    pub(crate) include_deleted: bool,
}

/// One unified hit as the all-workspace search rows: the message row plus its workspace and
/// what matched (snippet, canvas, or transcript).
pub(crate) fn unified_hit_to_json(hit: &UnifiedSearchHit) -> serde_json::Value {
    let inner = &hit.message;
    let reactions: Vec<output::ReactionRow> = inner
        .reactions
        .iter()
        .map(|r| output::ReactionRow {
            name: r.name.clone(),
            count: r.count,
        })
        .collect();
    let files: Vec<String> = inner
        .files
        .iter()
        .filter_map(|f| f.name.clone().or_else(|| f.title.clone()))
        .collect();
    let mut obj = serde_json::json!({
        "workspace": hit.workspace.as_str(),
        "workspace_name": hit.workspace_name,
        "channel": hit.channel.as_str(),
        "channel_name": hit.channel_name,
        "ts": inner.ts.as_str(),
        "user": inner.user.as_ref().map(|u| u.as_str()),
        "user_name": hit.user_name,
        "bot_id": inner.bot_id.as_ref().map(|b| b.as_str()),
        "text": inner.text,
        "thread_ts": inner.thread_ts.as_ref().map(|ts| ts.as_str()),
        "subtype": inner.subtype.as_ref().map(|s| s.as_str()),
        "deleted": hit.deleted,
        "edited": inner.edited.is_some(),
        "reply_count": inner.reply_count,
        "reactions": reactions,
        "files": files,
        "permalink": inner.permalink,
        "kind": hit.kind,
    });
    if let Some(map) = obj.as_object_mut() {
        let file_refs: Vec<output::FileRef> = inner
            .files
            .iter()
            .filter(|f| !f.id.as_str().is_empty())
            .map(|f| output::FileRef {
                id: f.id.as_str().to_owned(),
                name: f.name.clone(),
                title: f.title.clone(),
                filetype: f.filetype.clone(),
            })
            .collect();
        if !file_refs.is_empty() {
            map.insert("file_refs".to_owned(), serde_json::json!(file_refs));
        }
        if let Some(fid) = &hit.transcript_file_id {
            map.insert(
                "transcript_file_id".to_owned(),
                serde_json::json!(fid.as_str()),
            );
        }
        if let Some(excerpt) = &hit.transcript_excerpt {
            map.insert("transcript_excerpt".to_owned(), serde_json::json!(excerpt));
        }
        if let Some(rank) = hit.rank {
            map.insert("rank".to_owned(), serde_json::json!(rank));
        }
        if let Some(score) = hit.semantic_score {
            map.insert("semantic_score".to_owned(), serde_json::json!(score));
        }
    }
    obj
}

fn unified_hit_human_line(hit: &UnifiedSearchHit) -> String {
    let ws = hit.workspace.as_str();
    let ch = hit
        .channel_name
        .as_deref()
        .unwrap_or_else(|| hit.channel.as_str());
    let who = hit
        .user_name
        .as_deref()
        .or_else(|| hit.message.user.as_ref().map(|u| u.as_str()))
        .or_else(|| hit.message.bot_id.as_ref().map(|b| b.as_str()))
        .unwrap_or("unknown");
    let deleted = if hit.deleted { " (deleted)" } else { "" };
    let kind_tag = match hit.kind {
        SearchHitKind::Message => "",
        SearchHitKind::Snippet => " [snippet]",
        SearchHitKind::Canvas => " [canvas]",
        SearchHitKind::FileTranscript => " [transcript]",
    };
    let plain = mrkdwn_to_plain(&hit.message.text);
    let body = match (&hit.transcript_excerpt, plain.trim().is_empty()) {
        (Some(excerpt), true) => excerpt.clone(),
        (Some(excerpt), false) if hit.kind == SearchHitKind::FileTranscript => {
            format!("{plain} — {excerpt}")
        }
        _ => plain,
    };
    format!(
        "{ws}\t{ch}\t{}\t{who}:{deleted}{kind_tag} {body}\n",
        hit.message.ts.as_str()
    )
}

fn format_unified_markdown_export(query: &str, hits: &[UnifiedSearchHit]) -> String {
    let mut out = format!("# Search (all workspaces): {query}\n\n");
    for hit in hits {
        let ws = hit
            .workspace_name
            .as_deref()
            .map(|n| format!("{n} (`{}`)", hit.workspace.as_str()))
            .unwrap_or_else(|| format!("`{}`", hit.workspace.as_str()));
        let ch = hit
            .channel_name
            .as_deref()
            .unwrap_or_else(|| hit.channel.as_str());
        let author = hit
            .user_name
            .as_deref()
            .or_else(|| hit.message.user.as_ref().map(|u| u.as_str()))
            .or_else(|| hit.message.bot_id.as_ref().map(|b| b.as_str()))
            .unwrap_or("unknown");
        let deleted = if hit.deleted { " _(deleted)_" } else { "" };
        let kind = match hit.kind {
            SearchHitKind::Message => String::new(),
            other => format!(" _[{}]_ ", other.label()),
        };
        out.push_str(&format!(
            "**{author}** in {ch} · {ws} (`{}`){deleted}{kind}\n\n",
            hit.message.ts.as_str()
        ));
        if !hit.message.text.is_empty() {
            out.push_str(&hit.message.text);
            out.push_str("\n\n");
        }
        if let Some(excerpt) = &hit.transcript_excerpt {
            out.push_str(&format!("> _Transcript:_ {excerpt}\n\n"));
        }
        out.push_str("---\n\n");
    }
    out
}

/// The all-workspace search document both surfaces return (plus `mode` and `model` for a
/// semantic search), with the tab-separated lines.
pub(crate) fn search_all_document(
    query: &str,
    semantic: Option<(&str, &str)>,
    sort: SearchSort,
    results: &UnifiedSearchResults,
) -> Payload {
    let searched: Vec<&str> = results
        .workspaces_searched
        .iter()
        .map(|(ws, _)| ws.as_str())
        .collect();
    let workspaces_meta: Vec<serde_json::Value> = results
        .workspaces_searched
        .iter()
        .map(|(ws, name)| serde_json::json!({"id": ws.as_str(), "name": name}))
        .collect();
    let json_rows: Vec<serde_json::Value> = results.hits.iter().map(unified_hit_to_json).collect();
    let mut json = serde_json::json!({
        "query": query,
        "all_workspaces": true,
        "workspaces_searched": searched,
        "workspaces": workspaces_meta,
        "sort": sort_name(sort),
        "count": json_rows.len(),
        "messages": json_rows,
    });
    if let Some((mode, model)) = semantic {
        json["mode"] = serde_json::json!(mode);
        json["model"] = serde_json::json!(model);
    }
    let human: String = results.hits.iter().map(unified_hit_human_line).collect();
    (json, human)
}

/// Renders the search to `--out` (or stdout) when `--format`/`--out` ask for an export:
/// Markdown from `markdown`, otherwise pretty JSON. `false` when it should be emitted instead.
fn export_search_if_asked(
    ctx: &Ctx,
    format: Option<ExportFormat>,
    out: Option<&Path>,
    count: usize,
    json: &serde_json::Value,
    markdown: impl FnOnce() -> String,
) -> Result<bool, CmdError> {
    if format.is_none() && out.is_none() {
        return Ok(false);
    }
    let effective = if ctx.json {
        ExportFormat::Json
    } else {
        format.unwrap_or(ExportFormat::Md)
    };
    let rendered = match effective {
        ExportFormat::Json => serde_json::to_string_pretty(json)?,
        ExportFormat::Md => markdown(),
    };
    output::write_or_print_export(ctx, &rendered, out, count, "search matches")?;
    Ok(true)
}

/// Keyword search across every archive in `archives`, merged rank- or recency-ordered.
pub(crate) fn search_all_in_archives(
    archives: &[(TeamId, PathBuf)],
    parsed: &SearchQuery,
    sort: SearchSort,
    limit: usize,
) -> Result<UnifiedSearchResults, CmdError> {
    search_disk_archives(archives, parsed, sort, limit)
        .map_err(|err| CmdError::store(err, "searching across local workspace archives"))
}

/// Semantic or hybrid search across every archive in `archives`, merged the same way.
pub(crate) fn semantic_search_in_archives(
    archives: &[(TeamId, PathBuf)],
    index: &SemanticIndex,
    embedder: &dyn Embedder,
    parsed: &SearchQuery,
    mode: SearchMode,
    sort: SearchSort,
    limit: usize,
) -> Result<UnifiedSearchResults, CmdError> {
    hybrid_search_disk_archives(archives, index, embedder, parsed, mode, sort, limit)
        .map_err(|err| CmdError::new("command_failed", err))
}

pub(crate) fn cmd_search_all_workspaces(
    ctx: &Ctx,
    query: &str,
    opts: SearchAllOpts<'_>,
) -> Result<ExitCode> {
    let archives = all_archives(ctx.sessions());
    if archives.is_empty() {
        return Ok(output::empty_local(
            ctx,
            serde_json::json!({
                "query": query,
                "all_workspaces": true,
                "workspaces_searched": [],
                "count": 0,
                "messages": [],
            }),
            "searching across workspaces",
        ));
    }
    cmd_search_all_in_archives(ctx, &archives, query, opts)
}

pub(crate) fn cmd_search_all_in_archives(
    ctx: &Ctx,
    archives: &[(TeamId, PathBuf)],
    query: &str,
    opts: SearchAllOpts<'_>,
) -> Result<ExitCode> {
    let mut parsed = SearchQuery::parse(query);
    parsed.filters.include_deleted = opts.include_deleted;
    let results = search_all_in_archives(archives, &parsed, opts.sort, opts.limit)?;
    let payload = search_all_document(query, None, opts.sort, &results);
    let hits = &results.hits;
    if export_search_if_asked(ctx, opts.format, opts.out, hits.len(), &payload.0, || {
        format_unified_markdown_export(query, hits)
    })? {
        return Ok(ExitCode::SUCCESS);
    }
    Ok(emit_payload(ctx, Ok(payload)))
}

pub(crate) struct SearchCmdOpts<'a> {
    pub(crate) limit: usize,
    pub(crate) sort: SearchSort,
    pub(crate) format: Option<ExportFormat>,
    pub(crate) out: Option<&'a Path>,
    pub(crate) include_deleted: bool,
}

/// Searches one workspace's archive (nothing without one): the single-workspace `search` document.
pub(crate) fn search_document(
    workspace: &TeamId,
    archive: Option<&Archive>,
    query: &str,
    parsed: &SearchQuery,
    sort: SearchSort,
    limit: usize,
) -> Result<(Payload, Vec<MessageView>), CmdError> {
    let (messages, names, user_names) = match archive {
        Some(archive) => (
            archive
                .store()
                .search_sorted(workspace, parsed, sort, limit)
                .map_err(|err| {
                    CmdError::store(err, format!("searching the local archive of {workspace}"))
                })?,
            archive.channel_names(workspace).map_err(|err| {
                CmdError::store(err, format!("reading channel names for {workspace}"))
            })?,
            archive.user_display_map(workspace)?,
        ),
        None => (Vec::new(), HashMap::new(), HashMap::new()),
    };
    let rows: Vec<MessageView> = messages
        .iter()
        .map(|message| {
            MessageView::from_stored(
                message,
                &user_names,
                names.get(message.channel.as_str()).map(String::as_str),
            )
        })
        .collect();
    let human: String = rows.iter().map(MessageView::human_line).collect();
    let json = serde_json::json!({"query": query, "count": rows.len(), "messages": rows});
    Ok(((json, human), rows))
}

/// Prints one workspace's search, or renders it to `--out`/stdout in the export format.
pub(crate) fn cmd_search_in_archive(
    ctx: &Ctx,
    workspace: &TeamId,
    archive: Option<&Archive>,
    query: &str,
    opts: SearchCmdOpts<'_>,
) -> Result<ExitCode> {
    let mut parsed = SearchQuery::parse(query);
    parsed.filters.include_deleted = opts.include_deleted;
    let (payload, rows) =
        search_document(workspace, archive, query, &parsed, opts.sort, opts.limit)?;
    if export_search_if_asked(ctx, opts.format, opts.out, rows.len(), &payload.0, || {
        format_markdown_export(&format!("Search: {query}"), &rows)
    })? {
        return Ok(ExitCode::SUCCESS);
    }
    Ok(emit_payload(ctx, Ok(payload)))
}

pub(crate) fn cmd_search(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    query: &str,
    opts: SearchCmdOpts<'_>,
) -> Result<ExitCode> {
    run_local(
        ctx,
        workspace,
        serde_json::json!({"query": query, "count": 0, "messages": []}),
        "searching",
        |workspace, archive| {
            cmd_search_in_archive(ctx, workspace, archive, query, opts).map_err(CmdError::from)
        },
    )
}

pub(crate) struct SemanticSearchCmdOpts<'a> {
    pub(crate) mode: SearchMode,
    pub(crate) model: EmbeddingModel,
    pub(crate) all_workspaces: bool,
    pub(crate) limit: usize,
    pub(crate) sort: SearchSort,
    pub(crate) format: Option<ExportFormat>,
    pub(crate) out: Option<&'a Path>,
    pub(crate) include_deleted: bool,
}

pub(crate) fn cmd_search_semantic(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    query: &str,
    opts: SemanticSearchCmdOpts<'_>,
) -> Result<ExitCode> {
    let model = opts.model;
    let setup = detect_embedding_model_setup(model);
    if !setup.model_ready {
        return Ok(fail(
            ctx,
            "semantic_model_missing",
            &format!(
                "local embedding model {} is not downloaded ({}). Run: slackless archive semantic-model download --model {}",
                model.slug(),
                setup.download_size_hint,
                model.slug()
            ),
        ));
    }
    let Some(index) = open_existing_semantic_index().map_err(|e| anyhow::anyhow!("{e}"))? else {
        return Ok(fail(
            ctx,
            "semantic_index_missing",
            "semantic_index.sqlite has not been built yet. Run: slackless archive index-semantic",
        ));
    };

    let archives: Vec<(TeamId, PathBuf)> = if opts.all_workspaces {
        all_archives(ctx.sessions())
    } else {
        match workspace {
            Some(ws) => archive::store_path(ws)
                .filter(|p| p.is_file())
                .map(|p| vec![(ws.clone(), p)])
                .unwrap_or_default(),
            None => {
                return Ok(output::empty_local(
                    ctx,
                    serde_json::json!({
                        "query": query,
                        "mode": opts.mode.as_str(),
                        "all_workspaces": false,
                        "count": 0,
                        "messages": [],
                    }),
                    "running semantic search",
                ));
            }
        }
    };

    if archives.is_empty() {
        return Ok(output::empty_local(
            ctx,
            serde_json::json!({
                "query": query,
                "mode": opts.mode.as_str(),
                "all_workspaces": opts.all_workspaces,
                "count": 0,
                "messages": [],
            }),
            "running semantic search",
        ));
    }

    let embedder = OnnxEmbedder::load_default(model)
        .map_err(|e| anyhow::anyhow!("loading local ONNX embedding model: {e}"))?;
    cmd_search_semantic_in_archives(ctx, &archives, &index, &embedder, query, opts)
}

/// Prints a semantic or hybrid search across `archives`, or renders it in the export format.
/// Semantic/hybrid ranks by relevance unless the caller explicitly asked for recent.
pub(crate) fn cmd_search_semantic_in_archives(
    ctx: &Ctx,
    archives: &[(TeamId, PathBuf)],
    index: &SemanticIndex,
    embedder: &dyn Embedder,
    query: &str,
    opts: SemanticSearchCmdOpts<'_>,
) -> Result<ExitCode> {
    let mut parsed = SearchQuery::parse(query);
    parsed.filters.include_deleted = opts.include_deleted;
    let results = semantic_search_in_archives(
        archives, index, embedder, &parsed, opts.mode, opts.sort, opts.limit,
    )?;
    let payload = search_all_document(
        query,
        Some((opts.mode.as_str(), embedder.model_id())),
        opts.sort,
        &results,
    );
    let hits = &results.hits;
    if export_search_if_asked(ctx, opts.format, opts.out, hits.len(), &payload.0, || {
        format_unified_markdown_export(query, hits)
    })? {
        return Ok(ExitCode::SUCCESS);
    }
    Ok(emit_payload(ctx, Ok(payload)))
}

// -----------------------------------------------------------------------------
// Read
// -----------------------------------------------------------------------------

/// One conversation read from the archive: what `read`, MCP `read_channel`, and the MCP
/// prompts render.
pub(crate) struct ChannelRead {
    pub(crate) channel: ChannelId,
    pub(crate) row: Option<ConversationRow>,
    pub(crate) label: Option<String>,
    pub(crate) messages: Vec<StoredMessage>,
    pub(crate) user_names: HashMap<UserId, String>,
}

/// Resolves `channel_arg` against `archive` and reads one window of its history (nothing
/// without an archive).
pub(crate) fn local_channel(
    workspace: &TeamId,
    archive: Option<&Archive>,
    channel_arg: &str,
    window: &ChannelWindow,
) -> Result<ChannelRead, CmdError> {
    let raw = raw_channel_id(channel_arg);
    let Some(archive) = archive else {
        return Ok(ChannelRead {
            channel: raw,
            row: None,
            label: None,
            messages: Vec::new(),
            user_names: HashMap::new(),
        });
    };
    let row = match archive
        .resolve_channel(workspace, channel_arg)
        .map_err(|err| CmdError::store(err, format!("resolving channel {channel_arg}")))?
    {
        Some(row) => Some(row),
        None => archive
            .conversation_by_id(workspace, &raw)
            .map_err(|err| CmdError::store(err, format!("looking up conversation {raw}")))?,
    };
    let channel = row.as_ref().map_or(raw, |row| row.id.clone());
    let messages = read_channel_window(archive.store(), workspace, &channel, window)
        .map_err(|err| CmdError::store(err, format!("reading the history of {channel}")))?;
    let label = row.as_ref().map(|row| row.label());
    let user_names = archive.user_display_map(workspace)?;
    Ok(ChannelRead {
        channel,
        row,
        label,
        messages,
        user_names,
    })
}

/// One thread read: its channel, the root and every reply oldest first, and the user names.
pub(crate) struct ThreadRead {
    pub(crate) channel: ChannelId,
    pub(crate) label: Option<String>,
    pub(crate) messages: Vec<StoredMessage>,
    pub(crate) user_names: HashMap<UserId, String>,
}

/// Reads the thread `thread_ts` of `channel_arg` from `archive` (nothing without one).
pub(crate) fn local_thread(
    workspace: &TeamId,
    archive: Option<&Archive>,
    channel_arg: &str,
    thread_ts: &Ts,
) -> Result<ThreadRead, CmdError> {
    let raw = raw_channel_id(channel_arg);
    let Some(archive) = archive else {
        return Ok(ThreadRead {
            channel: raw,
            label: None,
            messages: Vec::new(),
            user_names: HashMap::new(),
        });
    };
    let row = archive
        .resolve_channel(workspace, channel_arg)
        .map_err(|err| CmdError::store(err, format!("resolving channel {channel_arg}")))?;
    let channel = row.as_ref().map_or(raw, |row| row.id.clone());
    let messages = archive
        .store()
        .thread_replies(workspace, &channel, thread_ts)
        .map_err(|err| CmdError::store(err, format!("reading thread {thread_ts} in {channel}")))?;
    let user_names = archive.user_display_map(workspace)?;
    Ok(ThreadRead {
        channel,
        label: row.map(|row| row.label()),
        messages,
        user_names,
    })
}

/// The messages as rows and as tab-separated lines under `channel_name`.
fn message_lines(
    messages: &[StoredMessage],
    user_names: &HashMap<UserId, String>,
    channel_name: Option<&str>,
) -> (Vec<MessageView>, String) {
    let rows: Vec<MessageView> = messages
        .iter()
        .map(|message| MessageView::from_stored(message, user_names, channel_name))
        .collect();
    let human = rows.iter().map(MessageView::human_line).collect();
    (rows, human)
}

/// The `read` document: the channel as the archive knows it and the window's messages.
pub(crate) fn channel_payload(read: &ChannelRead) -> Payload {
    let channel = match (&read.row, &read.label) {
        (Some(row), _) => serde_json::json!({
            "id": read.channel.as_str(),
            "name": row.name,
            "label": row.label(),
            "kind": row.kind,
            "user": row.user.as_ref().map(|user| user.as_str()),
        }),
        (None, Some(label)) => serde_json::json!({"id": read.channel.as_str(), "label": label}),
        (None, None) => serde_json::json!({"id": read.channel.as_str()}),
    };
    let (rows, human) = message_lines(&read.messages, &read.user_names, read.label.as_deref());
    (
        serde_json::json!({"channel": channel, "count": rows.len(), "messages": rows}),
        human,
    )
}

/// The `thread` document for `read`, from the archive or live.
pub(crate) fn thread_payload(read: &ThreadRead, thread_ts: &Ts, live: bool) -> Payload {
    let (rows, human) = message_lines(&read.messages, &read.user_names, read.label.as_deref());
    (
        serde_json::json!({
            "channel": read.channel.as_str(),
            "thread_ts": thread_ts.as_str(),
            "live": live,
            "count": rows.len(),
            "messages": rows,
        }),
        human,
    )
}

/// Mirrors a live page into the archive. A failed mirror, including an archive that would not
/// open, is reported on stderr: stdout carries the `--json` output or the MCP protocol.
fn mirror_live(
    archive: &mut Result<Archive, String>,
    write: impl FnOnce(&mut Archive) -> slackless_core::store::Result<()>,
    what: &str,
) {
    let mirrored = archive
        .as_mut()
        .map_err(|err| err.clone())
        .and_then(|archive| write(archive).map_err(|err| err.to_string()));
    if let Err(err) = mirrored {
        eprintln!("slackless: could not mirror the live {what} into the local archive ({err})");
    }
}

/// Fetches one live page of a thread from Slack, mirrors it into the local archive, and returns
/// it like an archive read (to render with `live: true`). `archive` may have failed to open.
pub(crate) async fn live_thread_read(
    api: &SlackApi,
    workspace: &TeamId,
    archive: &mut Result<Archive, String>,
    channel_arg: &str,
    thread_ts: &Ts,
    limit: u32,
) -> Result<ThreadRead, slackless_core::api::ApiError> {
    let (channel, label) = resolve_channel_with(archive.as_ref().ok(), workspace, channel_arg);
    let opts = HistoryOpts {
        limit: Some(limit.clamp(1, 999)),
        ..HistoryOpts::default()
    };
    let fetched_at = now_epoch_secs();
    let page = api
        .conversations_replies(&channel, thread_ts, &opts, None)
        .await?;
    let items = &page.items;
    mirror_live(
        archive,
        |archive| {
            archive
                .store_mut()
                .upsert_replies_page(workspace, &channel, items, fetched_at)
        },
        "replies",
    );
    let user_names = archive
        .as_ref()
        .ok()
        .and_then(|archive| archive.user_display_map(workspace).ok())
        .unwrap_or_default();
    Ok(ThreadRead {
        channel: channel.clone(),
        label,
        messages: page
            .items
            .into_iter()
            .map(|message| StoredMessage {
                channel: channel.clone(),
                message,
                deleted: false,
            })
            .collect(),
        user_names,
    })
}

/// Fetches one live page of a conversation's history (bounded by `window`) from Slack, mirrors
/// it into the local archive, and returns it oldest first like an archive read.
pub(crate) async fn live_channel_read(
    api: &SlackApi,
    workspace: &TeamId,
    archive: &mut Result<Archive, String>,
    channel_arg: &str,
    window: &ChannelWindow,
) -> Result<ChannelRead, slackless_core::api::ApiError> {
    let (channel, label) = resolve_channel_with(archive.as_ref().ok(), workspace, channel_arg);
    let opts = HistoryOpts {
        oldest: window.since_secs.map(|s| Ts::from(format!("{s}.000000"))),
        latest: window.before.clone(),
        inclusive: false,
        limit: Some(u32::try_from(window.limit).unwrap_or(200)),
    };
    let fetched_at = now_epoch_secs();
    let page = api.conversations_history(&channel, &opts, None).await?;
    let items = &page.items;
    mirror_live(
        archive,
        |archive| {
            archive
                .store_mut()
                .upsert_history_page(workspace, &channel, items, fetched_at)
                .map(|_| ())
        },
        "messages",
    );
    let user_names = archive
        .as_ref()
        .ok()
        .and_then(|archive| archive.user_display_map(workspace).ok())
        .unwrap_or_default();
    Ok(ChannelRead {
        channel: channel.clone(),
        row: None,
        label,
        // Slack returns newest first; reverse so output is oldest-first like local read.
        messages: page
            .items
            .into_iter()
            .rev()
            .map(|message| StoredMessage {
                channel: channel.clone(),
                message,
                deleted: false,
            })
            .collect(),
        user_names,
    })
}

pub(crate) fn cmd_read(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    channel_arg: &str,
    limit: usize,
    since: Option<&str>,
    include_deleted: bool,
) -> Result<ExitCode> {
    let cutoff_secs = match since {
        Some(spec) => match parse_since_cutoff(spec, now_epoch_secs()) {
            Ok(secs) => Some(secs),
            Err(msg) => return Ok(fail(ctx, "invalid_args", &msg)),
        },
        None => None,
    };
    run_local(
        ctx,
        workspace,
        serde_json::json!({"channel": {"id": raw_channel_id(channel_arg).as_str()}, "count": 0, "messages": []}),
        "reading",
        |workspace, archive| {
            let read = local_channel(
                workspace,
                archive,
                channel_arg,
                &ChannelWindow {
                    since_secs: cutoff_secs,
                    limit,
                    include_deleted,
                    ..ChannelWindow::default()
                },
            )?;
            Ok(emit_payload(ctx, Ok(channel_payload(&read))))
        },
    )
}

pub(crate) fn cmd_thread(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    channel_arg: &str,
    thread_ts_arg: &str,
) -> Result<ExitCode> {
    let thread_ts = Ts::from(thread_ts_arg.to_owned());
    run_local(
        ctx,
        workspace,
        serde_json::json!({
            "channel": raw_channel_id(channel_arg).as_str(),
            "thread_ts": thread_ts.as_str(),
            "live": false,
            "count": 0,
            "messages": [],
        }),
        "reading thread",
        |workspace, archive| {
            let read = local_thread(workspace, archive, channel_arg, &thread_ts)?;
            Ok(emit_payload(
                ctx,
                Ok(thread_payload(&read, &thread_ts, false)),
            ))
        },
    )
}

// -----------------------------------------------------------------------------
// Export
// -----------------------------------------------------------------------------

pub(crate) struct ExportCmdOpts<'a> {
    pub(crate) channel: Option<&'a str>,
    pub(crate) thread_ts: Option<&'a str>,
    pub(crate) transcript_file_id: Option<&'a str>,
    pub(crate) huddle_call_id: Option<&'a str>,
    pub(crate) include_threads: bool,
    pub(crate) include_deleted: bool,
    pub(crate) format: ExportFormat,
    pub(crate) limit: usize,
    pub(crate) out: Option<&'a Path>,
}

/// Exports `req` from `archive`. Without an archive a channel or thread still exports (from an
/// empty archive, so the headers render); a transcript or huddle that is not stored is `Ok(None)`.
pub(crate) fn export_archive(
    workspace: &TeamId,
    archive: Option<&Archive>,
    req: &ExportRequest<'_>,
    opts: &ExportOptions,
) -> Result<Option<(ExportArtifact, Option<ChannelId>)>, CmdError> {
    let fallback;
    let store = match archive {
        Some(archive) => archive.store(),
        None => {
            fallback = Store::open_in_memory()?;
            &fallback
        }
    };
    Ok(export_request(store, workspace, req, opts)?)
}

pub(crate) fn export_not_found(req: &ExportRequest<'_>) -> String {
    match req {
        ExportRequest::FileTranscript { file_id } => {
            format!("no local transcript stored for file {file_id}")
        }
        ExportRequest::HuddleTranscript { call_id } => {
            format!("no local huddle transcript stored for call {call_id}")
        }
        ExportRequest::Channel { .. } => "nothing to export".to_owned(),
    }
}

fn export_item_noun(req: &ExportRequest<'_>) -> &'static str {
    match req {
        ExportRequest::FileTranscript { .. } => "transcript",
        ExportRequest::HuddleTranscript { .. } => "huddle transcript lines",
        ExportRequest::Channel { .. } => "messages",
    }
}

pub(crate) fn cmd_export(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    opts: ExportCmdOpts<'_>,
) -> Result<ExitCode> {
    run_local(
        ctx,
        workspace,
        serde_json::json!({
            "channel": opts.channel,
            "thread_ts": opts.thread_ts,
            "transcript": opts.transcript_file_id,
            "huddle": opts.huddle_call_id,
            "count": 0,
            "messages": [],
        }),
        "exporting",
        |workspace, archive| {
            cmd_export_in_archive(ctx, workspace, archive, opts).map_err(CmdError::from)
        },
    )
}

pub(crate) fn cmd_export_in_archive(
    ctx: &Ctx,
    workspace: &TeamId,
    archive: Option<&Archive>,
    opts: ExportCmdOpts<'_>,
) -> Result<ExitCode> {
    let effective = if ctx.json {
        CoreExportFormat::Json
    } else {
        match opts.format {
            ExportFormat::Md => CoreExportFormat::Markdown,
            ExportFormat::Json => CoreExportFormat::Json,
        }
    };
    let thread_ts = opts.thread_ts.map(Ts::from);
    let file_id = opts.transcript_file_id.map(FileId::from);
    let req = if let Some(file_id) = file_id.as_ref() {
        ExportRequest::FileTranscript { file_id }
    } else if let Some(call_id) = opts.huddle_call_id {
        ExportRequest::HuddleTranscript { call_id }
    } else if let Some(channel) = opts.channel {
        ExportRequest::Channel {
            channel,
            thread_ts: thread_ts.as_ref(),
        }
    } else {
        return Ok(fail(
            ctx,
            "invalid_args",
            "provide a <CHANNEL> (optionally with --thread <TS>), or pass --transcript <FILE_ID> or --huddle <CALL_ID>",
        ));
    };
    let export_opts = ExportOptions {
        format: effective,
        limit: opts.limit,
        include_deleted: opts.include_deleted,
        include_threads: opts.include_threads,
        include_transcripts: true,
    };
    let Some((artifact, _)) = export_archive(workspace, archive, &req, &export_opts)? else {
        return Ok(fail(ctx, "not_found", &export_not_found(&req)));
    };
    output::write_or_print_export(
        ctx,
        &artifact.content,
        opts.out,
        artifact.item_count,
        export_item_noun(&req),
    )?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn format_markdown_export(label: &str, rows: &[MessageView]) -> String {
    let mut out = format!("# {label}\n\n");
    for row in rows {
        let author = row
            .row
            .user_name
            .as_deref()
            .or(row.row.user.as_deref())
            .or(row.row.bot_id.as_deref())
            .unwrap_or("unknown");
        let deleted = if row.row.deleted { " _(deleted)_" } else { "" };
        out.push_str(&format!("**{author}** (`{}`){deleted}\n\n", row.row.ts));
        if !row.row.text.is_empty() {
            out.push_str(&row.row.text);
            out.push_str("\n\n");
        }
        if !row.row.files.is_empty() {
            out.push_str(&format!("_Files:_ {}\n\n", row.row.files.join(", ")));
        }
        if !row.row.reactions.is_empty() {
            let rx: Vec<String> = row
                .row
                .reactions
                .iter()
                .map(|r| format!(":{}: {}", r.name, r.count))
                .collect();
            out.push_str(&format!("_Reactions:_ {}\n\n", rx.join(" · ")));
        }
        out.push_str("---\n\n");
    }
    out
}
