use std::collections::HashMap;
use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::types::time::now_epoch_secs;
use slackless_core::types::{TeamId, Ts};

use super::{
    all_workspace_ids, empty_local, open_archive_with_hint, parse_since_cutoff, raw_channel_id,
};
use crate::archive::Archive;
use crate::output::{self, ChannelView, DmView, MessageRow};
use crate::{Ctx, fail};

pub(crate) fn cmd_channels(ctx: &Ctx, workspace: Option<&TeamId>) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"count": 0, "channels": []}),
            "listing channels",
        ));
    };
    let archive = open_archive_with_hint(ctx, workspace)?;
    let rows: Vec<ChannelView> = match archive.as_ref() {
        Some(archive) => archive
            .channels(workspace)
            .with_context(|| format!("listing channels for {workspace}"))?
            .iter()
            .map(ChannelView::from_row)
            .collect(),
        None => Vec::new(),
    };
    output::emit(
        ctx.json,
        &serde_json::json!({"count": rows.len(), "channels": rows}),
        &output::channels_human(&rows),
    );
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn cmd_dms(ctx: &Ctx, workspace: Option<&TeamId>) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"count": 0, "dms": []}),
            "listing DMs",
        ));
    };
    let archive = open_archive_with_hint(ctx, workspace)?;
    let rows: Vec<DmView> = match archive.as_ref() {
        Some(archive) => {
            let user_names = archive
                .user_display_map(workspace)
                .with_context(|| format!("loading users for {workspace}"))?;
            archive
                .dms(workspace)
                .with_context(|| format!("listing DMs for {workspace}"))?
                .iter()
                .map(|row| DmView::from_row(row, &user_names))
                .collect()
        }
        None => Vec::new(),
    };
    output::emit(
        ctx.json,
        &serde_json::json!({"count": rows.len(), "dms": rows}),
        &output::dms_human(&rows),
    );
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn cmd_users(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    query: Option<&str>,
    include_deleted: bool,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"count": 0, "users": []}),
            "listing users",
        ));
    };
    let archive = open_archive_with_hint(ctx, workspace)?;
    let users = match archive.as_ref() {
        Some(archive) => archive
            .users(workspace, query, include_deleted)
            .with_context(|| format!("listing users for {workspace}"))?,
        None => Vec::new(),
    };
    let mut human = String::new();
    for u in &users {
        let display = u
            .display_name
            .as_deref()
            .or(u.real_name.as_deref())
            .unwrap_or(&u.name);
        let mut flags = Vec::new();
        if u.is_bot {
            flags.push("bot");
        }
        if u.deleted {
            flags.push("deleted");
        }
        let flag_str = if flags.is_empty() {
            String::new()
        } else {
            format!(" ({})", flags.join(", "))
        };
        human.push_str(&format!("{}\t@{}\t{}{flag_str}\n", u.id, u.name, display));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({"count": users.len(), "users": users}),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn cmd_threads(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    channel_arg: Option<&str>,
    limit: usize,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"count": 0, "threads": []}),
            "listing threads",
        ));
    };
    let archive = open_archive_with_hint(ctx, workspace)?;
    let (messages, names, user_names) = match archive.as_ref() {
        Some(archive) => {
            let resolved_ch = match channel_arg {
                Some(arg) => Some(
                    archive
                        .resolve_channel(workspace, arg)
                        .with_context(|| format!("resolving channel {arg}"))?
                        .map_or_else(|| raw_channel_id(arg), |row| row.id),
                ),
                None => None,
            };
            let messages = archive
                .store()
                .recent_threads(workspace, resolved_ch.as_ref(), limit)
                .with_context(|| format!("listing archived threads for {workspace}"))?;
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
        .map(|m| MessageRow::from_stored(m, &user_names))
        .collect();
    let mut human = String::new();
    for row in &rows {
        let label = names
            .get(row.channel.as_str())
            .map(String::as_str)
            .unwrap_or(&row.channel);
        let replies = row.reply_count.unwrap_or(0);
        let author = row
            .user_name
            .as_deref()
            .or(row.user.as_deref())
            .unwrap_or("unknown");
        let snippet: String = row.text.chars().take(80).collect();
        human.push_str(&format!(
            "{label}\t{}\t{replies} replies\t{author}: {snippet}\n",
            row.ts
        ));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({"count": rows.len(), "threads": rows}),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn cmd_revisions(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    channel_arg: &str,
    ts: &str,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"count": 0, "revisions": []}),
            "reading revisions",
        ));
    };
    let archive = open_archive_with_hint(ctx, workspace)?;
    let (channel_id, revisions) = match archive.as_ref() {
        Some(archive) => {
            let channel_id = archive
                .resolve_channel(workspace, channel_arg)
                .with_context(|| format!("resolving channel {channel_arg}"))?
                .map_or_else(|| raw_channel_id(channel_arg), |row| row.id);
            let revisions = archive
                .store()
                .revisions(workspace, &channel_id, &Ts::from(ts))
                .with_context(|| format!("reading revisions of {channel_id} {ts}"))?;
            (channel_id, revisions)
        }
        None => (raw_channel_id(channel_arg), Vec::new()),
    };
    let json_rows: Vec<_> = revisions
        .iter()
        .map(|rev| {
            serde_json::json!({
                "edit_ts": rev.edit_ts,
                "deleted": rev.deleted,
                "text": rev.message.text,
            })
        })
        .collect();
    let mut human = String::new();
    for (ix, rev) in revisions.iter().enumerate() {
        let kind = if rev.edit_ts.is_empty() {
            "original"
        } else if rev.deleted {
            "deleted"
        } else {
            "edited"
        };
        let stamp = if rev.edit_ts.is_empty() {
            ts
        } else {
            rev.edit_ts.as_str()
        };
        human.push_str(&format!(
            "#{}\t{kind}\t{stamp}\t{}\n",
            ix + 1,
            rev.message.text
        ));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({
            "channel": channel_id.as_str(),
            "ts": ts,
            "count": json_rows.len(),
            "revisions": json_rows,
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn cmd_digest(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    since: &str,
    all_workspaces: bool,
    limit: usize,
) -> Result<ExitCode> {
    let cutoff_secs = match parse_since_cutoff(since, now_epoch_secs()) {
        Ok(secs) => secs,
        Err(msg) => return Ok(fail(ctx, "invalid_args", &msg)),
    };
    let team_ids: Vec<TeamId> = if all_workspaces {
        all_workspace_ids()
    } else {
        workspace.into_iter().cloned().collect()
    };
    if team_ids.is_empty() {
        return Ok(empty_local(
            ctx,
            serde_json::json!({
                "since": since,
                "cutoff_secs": cutoff_secs,
                "total_messages": 0,
                "conversations": [],
            }),
            "building digest",
        ));
    }
    let mut total_messages: u64 = 0;
    let mut rows_json = Vec::new();
    let mut human = String::new();
    for ws in &team_ids {
        let Some(archive) =
            Archive::open(ws).with_context(|| format!("opening local archive for {ws}"))?
        else {
            continue;
        };
        let rows = archive
            .digest_since(ws, cutoff_secs, limit)
            .with_context(|| format!("building digest for {ws}"))?;
        for row in rows {
            total_messages = total_messages.saturating_add(row.message_count);
            let who = row.latest_author.as_deref().unwrap_or("unknown");
            if all_workspaces {
                human.push_str(&format!(
                    "{}\t{}\t{} msg ({} people)\t{who}: {}\n",
                    ws.as_str(),
                    row.label,
                    row.message_count,
                    row.author_count,
                    row.latest_text
                ));
            } else {
                human.push_str(&format!(
                    "{}\t{} msg ({} people)\t{who}: {}\n",
                    row.label, row.message_count, row.author_count, row.latest_text
                ));
            }
            let mut val = serde_json::to_value(&row).unwrap_or_default();
            if let Some(obj) = val.as_object_mut() {
                obj.insert("workspace".to_owned(), serde_json::json!(ws.as_str()));
            }
            rows_json.push(val);
        }
    }
    output::emit(
        ctx.json,
        &serde_json::json!({
            "since": since,
            "cutoff_secs": cutoff_secs,
            "all_workspaces": all_workspaces,
            "total_messages": total_messages,
            "count": rows_json.len(),
            "conversations": rows_json,
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}
