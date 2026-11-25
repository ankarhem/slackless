use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::auth::SessionStore;
use slackless_core::types::TeamId;

use super::{
    archive_hint, empty_local, now_epoch_secs, open_archive_store, parse_since_cutoff,
    raw_channel_id,
};
use crate::archive::{self, Archive};
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
    let archive = Archive::open(workspace);
    if archive.is_none() {
        archive_hint(workspace);
    }
    let rows: Vec<ChannelView> = match archive.as_ref() {
        Some(archive) => archive
            .channels(workspace)
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
    let archive = Archive::open(workspace);
    if archive.is_none() {
        archive_hint(workspace);
    }
    let rows: Vec<DmView> = match archive.as_ref() {
        Some(archive) => archive
            .dms(workspace)
            .iter()
            .map(|row| DmView::from_row(row, archive, workspace))
            .collect(),
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
    let archive = Archive::open(workspace);
    if archive.is_none() {
        archive_hint(workspace);
    }
    let users = archive
        .as_ref()
        .map_or_else(Vec::new, |a| a.users(workspace, query, include_deleted));
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
    let archive = Archive::open(workspace);
    if archive.is_none() {
        archive_hint(workspace);
    }
    let resolved_ch = channel_arg.map(|arg| {
        archive
            .as_ref()
            .and_then(|a| a.resolve_channel(workspace, arg))
            .map_or_else(|| raw_channel_id(arg), |row| row.id)
    });
    let messages = match open_archive_store(workspace) {
        Some(store) => store
            .recent_threads(workspace, resolved_ch.as_ref(), limit)
            .with_context(|| format!("listing archived threads for {workspace}"))?,
        None => Vec::new(),
    };
    let names = archive
        .as_ref()
        .map_or_else(Default::default, |a| a.channel_names(workspace));
    let rows: Vec<MessageRow> = messages
        .iter()
        .map(|m| MessageRow::from_stored(m, archive.as_ref(), workspace))
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
    let archive = Archive::open(workspace);
    let channel_id = archive
        .as_ref()
        .and_then(|a| a.resolve_channel(workspace, channel_arg))
        .map_or_else(|| raw_channel_id(channel_arg), |row| row.id);
    let revisions = match open_archive_store(workspace) {
        Some(store) => store
            .revisions(workspace, &channel_id, &slackless_core::types::Ts::from(ts))
            .with_context(|| format!("reading revisions of {channel_id} {ts}"))?,
        None => Vec::new(),
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
        let mut ids = Vec::new();
        if let Ok(sessions) = SessionStore::new().and_then(|s| s.load()) {
            for s in sessions {
                if !ids.contains(&s.team_id) {
                    ids.push(s.team_id);
                }
            }
        }
        for tid in archive::stored_team_ids() {
            if !ids.contains(&tid) {
                ids.push(tid);
            }
        }
        ids
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
        let Some(archive) = Archive::open(ws) else {
            continue;
        };
        let rows = archive.digest_since(ws, cutoff_secs, limit);
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
