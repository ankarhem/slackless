use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::auth::SessionStore;
use slackless_core::store::SearchQuery;
use slackless_core::types::{TeamId, Ts};

use super::{
    archive_hint, empty_local, now_epoch_secs, open_archive_store, parse_since_cutoff,
    raw_channel_id,
};
use crate::archive::{self, Archive};
use crate::cli::ExportFormat;
use crate::output::{self, MessageRow};
use crate::{Ctx, fail};

pub(crate) fn cmd_search_all_workspaces(
    ctx: &Ctx,
    query: &str,
    limit: usize,
    include_deleted: bool,
) -> Result<ExitCode> {
    let mut team_ids: Vec<TeamId> = Vec::new();
    if let Ok(sessions) = SessionStore::new().and_then(|s| s.load()) {
        for s in sessions {
            if !team_ids.contains(&s.team_id) {
                team_ids.push(s.team_id);
            }
        }
    }
    for tid in archive::stored_team_ids() {
        if !team_ids.contains(&tid) {
            team_ids.push(tid);
        }
    }
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
        let Some(store) = open_archive_store(ws) else {
            continue;
        };
        let messages = store
            .search(ws, &parsed, limit)
            .with_context(|| format!("searching the local archive of {ws}"))?;
        if messages.is_empty() {
            continue;
        }
        let archive = Archive::open(ws);
        let names = archive
            .as_ref()
            .map_or_else(Default::default, |a| a.channel_names(ws));
        for message in &messages {
            let row = MessageRow::from_stored(message, archive.as_ref(), ws);
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
    let mut parsed = SearchQuery::parse(query);
    parsed.filters.include_deleted = include_deleted;
    let messages = match open_archive_store(workspace) {
        Some(store) => store
            .search(workspace, &parsed, limit)
            .with_context(|| format!("searching the local archive of {workspace}"))?,
        None => Vec::new(),
    };
    let archive = Archive::open(workspace);
    let names = archive
        .as_ref()
        .map_or_else(Default::default, |archive| archive.channel_names(workspace));
    let rows: Vec<MessageRow> = messages
        .iter()
        .map(|message| MessageRow::from_stored(message, archive.as_ref(), workspace))
        .collect();
    if format.is_some() || out.is_some() {
        let effective = if ctx.json {
            ExportFormat::Json
        } else {
            format.unwrap_or(ExportFormat::Md)
        };
        let rendered = match effective {
            ExportFormat::Json => serde_json::to_string_pretty(&serde_json::json!({
                "query": query,
                "count": rows.len(),
                "messages": rows,
            }))?,
            ExportFormat::Md => format_markdown_export(&format!("Search: {query}"), &rows),
        };
        if let Some(path) = out {
            std::fs::write(path, &rendered)
                .with_context(|| format!("writing search results to {}", path.display()))?;
            if ctx.json {
                output::emit_json(&serde_json::json!({
                    "ok": true,
                    "path": path.display().to_string(),
                    "count": rows.len(),
                }));
            } else {
                eprintln!(
                    "Exported {} search matches to {}",
                    rows.len(),
                    path.display()
                );
            }
        } else {
            print!("{rendered}");
            if !rendered.ends_with('\n') {
                println!();
            }
        }
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
    let archive = Archive::open(workspace);
    if archive.is_none() {
        archive_hint(workspace);
    }
    let resolved = archive
        .as_ref()
        .and_then(|a| a.resolve_channel(workspace, channel_arg));
    let channel_id = resolved
        .as_ref()
        .map_or_else(|| raw_channel_id(channel_arg), |row| row.id.clone());
    let label = resolved.as_ref().map(|row| row.label());
    let mut messages = match open_archive_store(workspace) {
        Some(store) => store
            .channel_history(workspace, &channel_id, None, limit)
            .with_context(|| format!("reading the history of {channel_id}"))?,
        None => Vec::new(),
    };
    if !include_deleted {
        messages.retain(|m| !m.deleted);
    }
    if let Some(cutoff) = cutoff_secs {
        messages.retain(|m| m.message.ts.secs().is_some_and(|s| s >= cutoff));
    }
    let rows: Vec<MessageRow> = messages
        .iter()
        .map(|message| MessageRow::from_stored(message, archive.as_ref(), workspace))
        .collect();
    let mut human = String::new();
    for row in &rows {
        human.push_str(&row.human_line(label.as_deref()));
    }
    let channel = match archive
        .as_ref()
        .and_then(|a| a.conversation_by_id(workspace, &channel_id))
    {
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
    let archive = Archive::open(workspace);
    if archive.is_none() {
        archive_hint(workspace);
    }
    let resolved = archive
        .as_ref()
        .and_then(|a| a.resolve_channel(workspace, channel_arg));
    let channel_id = resolved
        .as_ref()
        .map_or_else(|| raw_channel_id(channel_arg), |row| row.id.clone());
    let label = resolved.as_ref().map(|row| row.label());
    let thread_ts = Ts::from(thread_ts_arg.to_owned());
    let messages = match open_archive_store(workspace) {
        Some(store) => store
            .thread_replies(workspace, &channel_id, &thread_ts)
            .with_context(|| format!("reading thread {thread_ts} in {channel_id}"))?,
        None => Vec::new(),
    };
    let rows: Vec<MessageRow> = messages
        .iter()
        .map(|message| MessageRow::from_stored(message, archive.as_ref(), workspace))
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
    let archive = Archive::open(workspace);
    if archive.is_none() {
        archive_hint(workspace);
    }
    let resolved = archive
        .as_ref()
        .and_then(|a| a.resolve_channel(workspace, channel_arg));
    let channel_id = resolved
        .as_ref()
        .map_or_else(|| raw_channel_id(channel_arg), |row| row.id.clone());
    let label = resolved
        .as_ref()
        .map(|row| row.label())
        .unwrap_or_else(|| channel_id.as_str().to_owned());
    let messages = match open_archive_store(workspace) {
        Some(store) => store
            .channel_history(workspace, &channel_id, None, limit)
            .with_context(|| format!("exporting the history of {channel_id}"))?,
        None => Vec::new(),
    };
    let rows: Vec<MessageRow> = messages
        .iter()
        .map(|message| MessageRow::from_stored(message, archive.as_ref(), workspace))
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
    if let Some(path) = out {
        std::fs::write(path, &rendered)
            .with_context(|| format!("writing export to {}", path.display()))?;
        if ctx.json {
            output::emit_json(&serde_json::json!({
                "ok": true,
                "path": path.display().to_string(),
                "count": rows.len(),
            }));
        } else {
            eprintln!("Exported {} messages to {}", rows.len(), path.display());
        }
    } else {
        print!("{rendered}");
        if !rendered.ends_with('\n') {
            println!();
        }
    }
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
