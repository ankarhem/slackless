use std::collections::HashMap;
use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::store::{SearchQuery, Store, StoredMessage};
use slackless_core::types::time::now_epoch_secs;
use slackless_core::types::{ChannelId, TeamId, Ts};

use super::{
    all_workspace_ids, empty_local, open_archive_with_hint, parse_since_cutoff, raw_channel_id,
    write_or_print_export,
};
use crate::archive::Archive;
use crate::cli::ExportFormat;
use crate::output::{self, MessageRow};
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

pub(crate) fn cmd_search_all_workspaces(
    ctx: &Ctx,
    query: &str,
    limit: usize,
    include_deleted: bool,
) -> Result<ExitCode> {
    let team_ids = all_workspace_ids();
    if team_ids.is_empty() {
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

    let mut parsed = SearchQuery::parse(query);
    parsed.filters.include_deleted = include_deleted;
    let mut hits: Vec<(String, Option<String>, MessageRow)> = Vec::new();
    for ws in &team_ids {
        let Some(archive) =
            Archive::open(ws).with_context(|| format!("opening local archive for {ws}"))?
        else {
            continue;
        };
        let messages = archive
            .store()
            .search(ws, &parsed, limit)
            .with_context(|| format!("searching the local archive of {ws}"))?;
        if messages.is_empty() {
            continue;
        }
        let names = archive
            .channel_names(ws)
            .with_context(|| format!("reading channel names for {ws}"))?;
        let user_names = archive
            .user_display_map(ws)
            .with_context(|| format!("reading user names for {ws}"))?;
        for message in &messages {
            let row = MessageRow::from_stored(message, &user_names);
            let label = names.get(row.channel.as_str()).cloned();
            hits.push((ws.as_str().to_owned(), label, row));
        }
    }
    hits.sort_by(|a, b| b.2.ts.cmp(&a.2.ts));
    hits.truncate(limit);

    let mut human = String::new();
    let json_rows: Vec<serde_json::Value> = hits
        .iter()
        .map(|(ws_id, label, row)| {
            human.push_str(&format!("{ws_id}\t{}", row.human_line(label.as_deref())));
            let mut val = serde_json::to_value(row).unwrap_or_default();
            if let Some(obj) = val.as_object_mut() {
                obj.insert("workspace".to_owned(), serde_json::json!(ws_id));
            }
            val
        })
        .collect();
    let searched: Vec<&str> = team_ids.iter().map(TeamId::as_str).collect();
    output::emit(
        ctx.json,
        &serde_json::json!({
            "query": query,
            "all_workspaces": true,
            "workspaces_searched": searched,
            "count": json_rows.len(),
            "messages": json_rows,
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

pub(crate) struct SearchCmdOpts<'a> {
    pub(crate) limit: usize,
    pub(crate) format: Option<ExportFormat>,
    pub(crate) out: Option<&'a std::path::Path>,
    pub(crate) include_deleted: bool,
}

pub(crate) fn cmd_search(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    query: &str,
    limit: usize,
    format: Option<ExportFormat>,
    out: Option<&std::path::Path>,
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
                .search(workspace, &parsed, opts.limit)
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

pub(crate) fn cmd_export(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    channel_arg: &str,
    format: ExportFormat,
    limit: usize,
    out: Option<&std::path::Path>,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"channel": channel_arg, "count": 0, "messages": []}),
            "exporting",
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
            let messages = archive
                .store()
                .channel_history(workspace, &channel_id, None, limit)
                .with_context(|| format!("exporting the history of {channel_id}"))?;
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
    let label = resolved
        .as_ref()
        .map(|row| row.label())
        .unwrap_or_else(|| channel_id.as_str().to_owned());
    let rows: Vec<MessageRow> = messages
        .iter()
        .map(|message| MessageRow::from_stored(message, &user_names))
        .collect();
    let effective = if ctx.json { ExportFormat::Json } else { format };
    let rendered = match effective {
        ExportFormat::Json => serde_json::to_string_pretty(&serde_json::json!({
            "workspace": workspace.as_str(),
            "channel": {
                "id": channel_id.as_str(),
                "label": label,
            },
            "count": rows.len(),
            "messages": rows,
        }))?,
        ExportFormat::Md => format_markdown_export(&label, &rows),
    };
    write_or_print_export(ctx, &rendered, out, rows.len(), "messages")?;
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
