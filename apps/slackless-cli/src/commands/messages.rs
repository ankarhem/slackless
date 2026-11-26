use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::domain::mrkdwn::mrkdwn_to_plain;
use slackless_core::export::{
    ExportFormat as CoreExportFormat, ExportOptions, export_channel, export_file_transcript,
    export_huddle_transcript, export_thread,
};
use slackless_core::search::{SearchHitKind, UnifiedSearchHit, search_disk_archives};
use slackless_core::store::{SearchQuery, SearchSort, Store, StoredMessage};
use slackless_core::types::time::now_epoch_secs;
use slackless_core::types::{ChannelId, FileId, TeamId, Ts};

use super::{
    all_workspace_ids, empty_local, open_archive_with_hint, parse_since_cutoff, raw_channel_id,
    write_or_print_export,
};
use crate::archive::{self, Archive};
use crate::cli::ExportFormat;
use crate::output::{self, MessageRow, ReactionRow};
use crate::{Ctx, fail};

/// Read up to `limit` newest matching messages from `channel`, ordered oldest first.
///
/// Paginates backwards via `before` when filtering deleted rows or applying `--since` so
/// deleted tombstones or cutoff filters never starve `--limit`.
pub(crate) fn read_channel_messages(
    store: &Store,
    workspace: &TeamId,
    channel: &ChannelId,
    limit: usize,
    cutoff_secs: Option<i64>,
    include_deleted: bool,
) -> slackless_core::store::Result<Vec<StoredMessage>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    if include_deleted && cutoff_secs.is_none() {
        return store.channel_history(workspace, channel, None, limit);
    }

    let page_size = limit.clamp(64, 500);
    let mut collected: Vec<StoredMessage> = Vec::with_capacity(limit.min(512));
    let mut before: Option<Ts> = None;

    'pages: loop {
        let page = store.channel_history(workspace, channel, before.as_ref(), page_size)?;
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        let oldest_ts = page[0].message.ts.clone();

        for msg in page.into_iter().rev() {
            if let Some(cutoff) = cutoff_secs {
                match msg.message.ts.secs() {
                    Some(secs) if secs < cutoff => break 'pages,
                    Some(_) => {}
                    None => continue,
                }
            }
            if !include_deleted && msg.deleted {
                continue;
            }
            collected.push(msg);
            if collected.len() >= limit {
                break 'pages;
            }
        }

        if page_len < page_size || before.as_ref() == Some(&oldest_ts) {
            break;
        }
        before = Some(oldest_ts);
    }

    collected.reverse();
    Ok(collected)
}

pub(crate) struct SearchAllOpts<'a> {
    pub(crate) limit: usize,
    pub(crate) sort: SearchSort,
    pub(crate) format: Option<ExportFormat>,
    pub(crate) out: Option<&'a Path>,
    pub(crate) include_deleted: bool,
}

fn unified_hit_to_json(hit: &UnifiedSearchHit) -> serde_json::Value {
    let inner = &hit.message;
    let reactions: Vec<ReactionRow> = inner
        .reactions
        .iter()
        .map(|r| ReactionRow {
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

pub(crate) fn cmd_search_all_workspaces(
    ctx: &Ctx,
    query: &str,
    opts: SearchAllOpts<'_>,
) -> Result<ExitCode> {
    let team_ids = all_workspace_ids();
    let archives: Vec<(TeamId, PathBuf)> = team_ids
        .into_iter()
        .filter_map(|ws| archive::store_path(&ws).map(|path| (ws, path)))
        .collect();
    if archives.is_empty() {
        return Ok(empty_local(
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
    let results = search_disk_archives(archives, &parsed, opts.sort, opts.limit)
        .context("searching across local workspace archives")?;

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

    if opts.format.is_some() || opts.out.is_some() {
        let effective = if ctx.json {
            ExportFormat::Json
        } else {
            opts.format.unwrap_or(ExportFormat::Md)
        };
        let rendered = match effective {
            ExportFormat::Json => serde_json::to_string_pretty(&serde_json::json!({
                "query": query,
                "all_workspaces": true,
                "workspaces_searched": searched,
                "workspaces": workspaces_meta,
                "count": json_rows.len(),
                "messages": json_rows,
            }))?,
            ExportFormat::Md => format_unified_markdown_export(query, &results.hits),
        };
        write_or_print_export(ctx, &rendered, opts.out, json_rows.len(), "search matches")?;
        return Ok(ExitCode::SUCCESS);
    }

    let mut human = String::new();
    for hit in &results.hits {
        human.push_str(&unified_hit_human_line(hit));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({
            "query": query,
            "all_workspaces": true,
            "workspaces_searched": searched,
            "workspaces": workspaces_meta,
            "count": json_rows.len(),
            "messages": json_rows,
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

pub(crate) struct SearchCmdOpts<'a> {
    pub(crate) limit: usize,
    pub(crate) sort: SearchSort,
    pub(crate) format: Option<ExportFormat>,
    pub(crate) out: Option<&'a Path>,
    pub(crate) include_deleted: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_search(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    query: &str,
    limit: usize,
    sort: SearchSort,
    format: Option<ExportFormat>,
    out: Option<&Path>,
    include_deleted: bool,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"query": query, "count": 0, "messages": []}),
            "searching",
        ));
    };
    let archive = open_archive_with_hint(ctx, workspace)?;
    cmd_search_in_archive(
        ctx,
        workspace,
        archive.as_ref(),
        query,
        SearchCmdOpts {
            limit,
            sort,
            format,
            out,
            include_deleted,
        },
    )
}

pub(crate) fn cmd_search_in_archive(
    ctx: &Ctx,
    workspace: &TeamId,
    archive: Option<&Archive>,
    query: &str,
    opts: SearchCmdOpts<'_>,
) -> Result<ExitCode> {
    let mut parsed = SearchQuery::parse(query);
    parsed.filters.include_deleted = opts.include_deleted;
    let (messages, names, user_names) = match archive {
        Some(archive) => {
            let messages = archive
                .store()
                .search_sorted(workspace, &parsed, opts.sort, opts.limit)
                .with_context(|| format!("searching the local archive of {workspace}"))?;
            let names = archive
                .channel_names(workspace)
                .with_context(|| format!("reading channel names for {workspace}"))?;
            let user_names = archive
                .user_display_map(workspace)
                .with_context(|| format!("reading user names for {workspace}"))?;
            (messages, names, user_names)
        }
        None => (Vec::new(), HashMap::new(), HashMap::new()),
    };
    let rows: Vec<MessageRow> = messages
        .iter()
        .map(|message| MessageRow::from_stored(message, &user_names))
        .collect();
    if opts.format.is_some() || opts.out.is_some() {
        let effective = if ctx.json {
            ExportFormat::Json
        } else {
            opts.format.unwrap_or(ExportFormat::Md)
        };
        let rendered = match effective {
            ExportFormat::Json => serde_json::to_string_pretty(&serde_json::json!({
                "query": query,
                "count": rows.len(),
                "messages": rows,
            }))?,
            ExportFormat::Md => format_markdown_export(&format!("Search: {query}"), &rows),
        };
        write_or_print_export(ctx, &rendered, opts.out, rows.len(), "search matches")?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut human = String::new();
    for row in &rows {
        let label = names.get(row.channel.as_str()).map(String::as_str);
        human.push_str(&row.human_line(label));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({"query": query, "count": rows.len(), "messages": rows}),
        &human,
    );
    Ok(ExitCode::SUCCESS)
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
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"channel": {"id": channel_arg}, "count": 0, "messages": []}),
            "reading",
        ));
    };
    let archive = open_archive_with_hint(ctx, workspace)?;
    let (resolved, channel_id, messages, user_names) = match archive.as_ref() {
        Some(archive) => {
            let resolved = archive
                .resolve_channel(workspace, channel_arg)
                .with_context(|| format!("resolving channel {channel_arg}"))?;
            let channel_id = resolved
                .as_ref()
                .map_or_else(|| raw_channel_id(channel_arg), |row| row.id.clone());
            let messages = read_channel_messages(
                archive.store(),
                workspace,
                &channel_id,
                limit,
                cutoff_secs,
                include_deleted,
            )
            .with_context(|| format!("reading the history of {channel_id}"))?;
            let user_names = archive
                .user_display_map(workspace)
                .with_context(|| format!("reading user names for {workspace}"))?;
            (resolved, channel_id, messages, user_names)
        }
        None => (
            None,
            raw_channel_id(channel_arg),
            Vec::new(),
            HashMap::new(),
        ),
    };
    let label = resolved.as_ref().map(|row| row.label());
    let rows: Vec<MessageRow> = messages
        .iter()
        .map(|message| MessageRow::from_stored(message, &user_names))
        .collect();
    let mut human = String::new();
    for row in &rows {
        human.push_str(&row.human_line(label.as_deref()));
    }
    let channel_row = match resolved {
        Some(row) => Some(row),
        None => match archive.as_ref() {
            Some(archive) => archive
                .conversation_by_id(workspace, &channel_id)
                .with_context(|| format!("looking up conversation {channel_id}"))?,
            None => None,
        },
    };
    let channel = match channel_row {
        Some(row) => serde_json::json!({
            "id": channel_id.as_str(),
            "name": row.name,
            "kind": row.kind,
            "user": row.user.as_ref().map(|user| user.as_str()),
        }),
        None => serde_json::json!({"id": channel_id.as_str()}),
    };
    output::emit(
        ctx.json,
        &serde_json::json!({"channel": channel, "count": rows.len(), "messages": rows}),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn cmd_thread(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    channel_arg: &str,
    thread_ts_arg: &str,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({
                "channel": channel_arg,
                "thread_ts": thread_ts_arg,
                "live": false,
                "count": 0,
                "messages": [],
            }),
            "reading thread",
        ));
    };
    let archive = open_archive_with_hint(ctx, workspace)?;
    let thread_ts = Ts::from(thread_ts_arg.to_owned());
    let (resolved, channel_id, messages, user_names) = match archive.as_ref() {
        Some(archive) => {
            let resolved = archive
                .resolve_channel(workspace, channel_arg)
                .with_context(|| format!("resolving channel {channel_arg}"))?;
            let channel_id = resolved
                .as_ref()
                .map_or_else(|| raw_channel_id(channel_arg), |row| row.id.clone());
            let messages = archive
                .store()
                .thread_replies(workspace, &channel_id, &thread_ts)
                .with_context(|| format!("reading thread {thread_ts} in {channel_id}"))?;
            let user_names = archive
                .user_display_map(workspace)
                .with_context(|| format!("reading user names for {workspace}"))?;
            (resolved, channel_id, messages, user_names)
        }
        None => (
            None,
            raw_channel_id(channel_arg),
            Vec::new(),
            HashMap::new(),
        ),
    };
    let label = resolved.as_ref().map(|row| row.label());
    let rows: Vec<MessageRow> = messages
        .iter()
        .map(|message| MessageRow::from_stored(message, &user_names))
        .collect();
    let mut human = String::new();
    for row in &rows {
        human.push_str(&row.human_line(label.as_deref()));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({
            "channel": channel_id.as_str(),
            "thread_ts": thread_ts.as_str(),
            "live": false,
            "count": rows.len(),
            "messages": rows,
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

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

pub(crate) fn cmd_export(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    opts: ExportCmdOpts<'_>,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({
                "channel": opts.channel,
                "thread_ts": opts.thread_ts,
                "transcript": opts.transcript_file_id,
                "huddle": opts.huddle_call_id,
                "count": 0,
                "messages": [],
            }),
            "exporting",
        ));
    };
    let archive = open_archive_with_hint(ctx, workspace)?;
    cmd_export_in_archive(ctx, workspace, archive.as_ref(), opts)
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

    if let Some(file_id_arg) = opts.transcript_file_id {
        let Some(archive) = archive else {
            return Ok(fail(
                ctx,
                "not_found",
                &format!(
                    "no local archive for workspace {workspace}; transcript {file_id_arg} not found"
                ),
            ));
        };
        let fid = FileId::from(file_id_arg);
        let Some(artifact) = export_file_transcript(archive.store(), workspace, &fid, effective)
            .with_context(|| format!("exporting transcript for file {file_id_arg}"))?
        else {
            return Ok(fail(
                ctx,
                "not_found",
                &format!("no local transcript stored for file {file_id_arg}"),
            ));
        };
        write_or_print_export(
            ctx,
            &artifact.content,
            opts.out,
            artifact.item_count,
            "transcript",
        )?;
        return Ok(ExitCode::SUCCESS);
    }

    if let Some(call_id) = opts.huddle_call_id {
        let Some(archive) = archive else {
            return Ok(fail(
                ctx,
                "not_found",
                &format!(
                    "no local archive for workspace {workspace}; huddle transcript {call_id} not found"
                ),
            ));
        };
        let Some(artifact) =
            export_huddle_transcript(archive.store(), workspace, call_id, effective)
                .with_context(|| format!("exporting huddle transcript {call_id}"))?
        else {
            return Ok(fail(
                ctx,
                "not_found",
                &format!("no local huddle transcript stored for call {call_id}"),
            ));
        };
        write_or_print_export(
            ctx,
            &artifact.content,
            opts.out,
            artifact.item_count,
            "huddle transcript lines",
        )?;
        return Ok(ExitCode::SUCCESS);
    }

    let Some(channel_arg) = opts.channel else {
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

    let Some(archive) = archive else {
        let channel_id = raw_channel_id(channel_arg);
        let fallback_store = Store::open_in_memory()?;
        let artifact = match opts.thread_ts {
            Some(ts) => export_thread(
                &fallback_store,
                workspace,
                &channel_id,
                &Ts::from(ts),
                &export_opts,
            )?,
            None => export_channel(&fallback_store, workspace, &channel_id, &export_opts)?,
        };
        write_or_print_export(
            ctx,
            &artifact.content,
            opts.out,
            artifact.item_count,
            "messages",
        )?;
        return Ok(ExitCode::SUCCESS);
    };

    let resolved = archive
        .resolve_channel(workspace, channel_arg)
        .with_context(|| format!("resolving channel {channel_arg}"))?;
    let channel_id = resolved
        .as_ref()
        .map_or_else(|| raw_channel_id(channel_arg), |row| row.id.clone());

    let artifact = match opts.thread_ts {
        Some(ts) => export_thread(
            archive.store(),
            workspace,
            &channel_id,
            &Ts::from(ts),
            &export_opts,
        )
        .with_context(|| format!("exporting thread {ts} in {channel_id}"))?,
        None => export_channel(archive.store(), workspace, &channel_id, &export_opts)
            .with_context(|| format!("exporting the history of {channel_id}"))?,
    };
    write_or_print_export(
        ctx,
        &artifact.content,
        opts.out,
        artifact.item_count,
        "messages",
    )?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn format_markdown_export(label: &str, rows: &[MessageRow]) -> String {
    let mut out = format!("# {label}\n\n");
    for row in rows {
        let author = row
            .user_name
            .as_deref()
            .or(row.user.as_deref())
            .or(row.bot_id.as_deref())
            .unwrap_or("unknown");
        let deleted = if row.deleted { " _(deleted)_" } else { "" };
        out.push_str(&format!("**{author}** (`{}`){deleted}\n\n", row.ts));
        if !row.text.is_empty() {
            out.push_str(&row.text);
            out.push_str("\n\n");
        }
        if !row.files.is_empty() {
            out.push_str(&format!("_Files:_ {}\n\n", row.files.join(", ")));
        }
        if !row.reactions.is_empty() {
            let rx: Vec<String> = row
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
