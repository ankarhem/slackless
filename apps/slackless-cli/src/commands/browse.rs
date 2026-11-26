use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::auth::Session;
use slackless_core::catchup::{
    CatchUpBriefing, CatchUpWindow, build_catchup_disk_archives, format_catchup_human,
};
use slackless_core::types::time::now_epoch_secs;
use slackless_core::types::{TeamId, Ts, UserId};

use super::{CmdError, Payload, all_workspace_ids, parse_since_cutoff, raw_channel_id};
use crate::archive::{self, Archive};
use crate::output::{self, ChannelView, DmView, MessageRow};
use crate::{Ctx, emit_payload, fail, run_local};

pub(crate) fn cmd_channels(ctx: &Ctx, workspace: Option<&TeamId>) -> Result<ExitCode> {
    run_local(
        ctx,
        workspace,
        serde_json::json!({"count": 0, "channels": []}),
        "listing channels",
        |workspace, archive| Ok(emit_payload(ctx, channels_payload(workspace, archive))),
    )
}

fn channels_payload(workspace: &TeamId, archive: Option<&Archive>) -> Result<Payload, CmdError> {
    let rows: Vec<ChannelView> = match archive {
        Some(archive) => archive
            .channels(workspace)
            .map_err(|err| CmdError::store(err, format!("listing channels for {workspace}")))?
            .iter()
            .map(ChannelView::from_row)
            .collect(),
        None => Vec::new(),
    };
    Ok((
        serde_json::json!({"count": rows.len(), "channels": rows}),
        output::channels_human(&rows),
    ))
}

pub(crate) fn cmd_dms(ctx: &Ctx, workspace: Option<&TeamId>) -> Result<ExitCode> {
    run_local(
        ctx,
        workspace,
        serde_json::json!({"count": 0, "dms": []}),
        "listing DMs",
        |workspace, archive| Ok(emit_payload(ctx, dms_payload(workspace, archive))),
    )
}

fn dms_payload(workspace: &TeamId, archive: Option<&Archive>) -> Result<Payload, CmdError> {
    let rows: Vec<DmView> = match archive {
        Some(archive) => {
            let user_names = archive
                .user_display_map(workspace)
                .map_err(|err| CmdError::store(err, format!("loading users for {workspace}")))?;
            archive
                .dms(workspace)
                .map_err(|err| CmdError::store(err, format!("listing DMs for {workspace}")))?
                .iter()
                .map(|row| DmView::from_row(row, &user_names))
                .collect()
        }
        None => Vec::new(),
    };
    Ok((
        serde_json::json!({"count": rows.len(), "dms": rows}),
        output::dms_human(&rows),
    ))
}

pub(crate) fn cmd_users(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    query: Option<&str>,
    include_deleted: bool,
) -> Result<ExitCode> {
    run_local(
        ctx,
        workspace,
        serde_json::json!({"count": 0, "users": []}),
        "listing users",
        |workspace, archive| {
            let users = match archive {
                Some(archive) => {
                    archive
                        .users(workspace, query, include_deleted)
                        .map_err(|err| {
                            CmdError::store(err, format!("listing users for {workspace}"))
                        })?
                }
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
            Ok(emit_payload(
                ctx,
                Ok((
                    serde_json::json!({"count": users.len(), "users": users}),
                    human,
                )),
            ))
        },
    )
}

pub(crate) fn cmd_threads(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    channel_arg: Option<&str>,
    limit: usize,
) -> Result<ExitCode> {
    run_local(
        ctx,
        workspace,
        serde_json::json!({"count": 0, "threads": []}),
        "listing threads",
        |workspace, archive| {
            let (messages, names, user_names) = match archive {
                Some(archive) => {
                    let resolved_ch = match channel_arg {
                        Some(arg) => Some(
                            archive
                                .resolve_channel(workspace, arg)
                                .map_err(|err| {
                                    CmdError::store(err, format!("resolving channel {arg}"))
                                })?
                                .map_or_else(|| raw_channel_id(arg), |row| row.id),
                        ),
                        None => None,
                    };
                    (
                        archive
                            .store()
                            .recent_threads(workspace, resolved_ch.as_ref(), limit)
                            .map_err(|err| {
                                CmdError::store(
                                    err,
                                    format!("listing archived threads for {workspace}"),
                                )
                            })?,
                        archive.channel_names(workspace)?,
                        archive.user_display_map(workspace)?,
                    )
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
            Ok(emit_payload(
                ctx,
                Ok((
                    serde_json::json!({"count": rows.len(), "threads": rows}),
                    human,
                )),
            ))
        },
    )
}

pub(crate) fn cmd_revisions(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    channel_arg: &str,
    ts: &str,
) -> Result<ExitCode> {
    run_local(
        ctx,
        workspace,
        serde_json::json!({"count": 0, "revisions": []}),
        "reading revisions",
        |workspace, archive| {
            let (channel_id, revisions) = match archive {
                Some(archive) => {
                    let channel_id = archive
                        .resolve_channel(workspace, channel_arg)
                        .map_err(|err| {
                            CmdError::store(err, format!("resolving channel {channel_arg}"))
                        })?
                        .map_or_else(|| raw_channel_id(channel_arg), |row| row.id);
                    let revisions = archive
                        .store()
                        .revisions(workspace, &channel_id, &Ts::from(ts))
                        .map_err(|err| {
                            CmdError::store(err, format!("reading revisions of {channel_id} {ts}"))
                        })?;
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
            Ok(emit_payload(
                ctx,
                Ok((
                    serde_json::json!({
                        "channel": channel_id.as_str(),
                        "ts": ts,
                        "count": json_rows.len(),
                        "revisions": json_rows,
                    }),
                    human,
                )),
            ))
        },
    )
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
        all_workspace_ids(ctx.sessions(), archive::stored_team_ids())
    } else {
        workspace.into_iter().cloned().collect()
    };
    if team_ids.is_empty() {
        return Ok(output::empty_local(
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

// -----------------------------------------------------------------------------
// Catch-Up
// -----------------------------------------------------------------------------

/// Parse CLI `--since` into a [`slackless_core::catchup::CatchUpWindow`].
pub(crate) fn parse_catchup_window(
    since: Option<&str>,
    now_secs: i64,
) -> Result<slackless_core::catchup::CatchUpWindow, String> {
    let Some(raw) = since.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(CatchUpWindow::SinceLastVisit);
    };
    match raw.to_ascii_lowercase().as_str() {
        "since-last-visit" | "last-visit" | "last_visit" => Ok(CatchUpWindow::SinceLastVisit),
        "24h" | "1d" => Ok(CatchUpWindow::Last24h),
        "7d" | "1w" => Ok(CatchUpWindow::Last7d),
        _ => {
            parse_since_cutoff(raw, now_secs).map(|since_secs| CatchUpWindow::Custom { since_secs })
        }
    }
}

/// The archives a catch-up covers (`workspace`, or every known workspace) and, for each, the
/// signed-in user whose mentions and DMs it looks for.
pub(crate) fn catchup_scope(
    sessions: &[Session],
    workspace: Option<&TeamId>,
    all_workspaces: bool,
) -> (Vec<(TeamId, PathBuf)>, HashMap<TeamId, UserId>) {
    let team_ids: Vec<TeamId> = if all_workspaces {
        all_workspace_ids(sessions, archive::stored_team_ids())
    } else {
        workspace.into_iter().cloned().collect()
    };
    let archives = team_ids
        .iter()
        .filter_map(|ws| archive::store_path(ws).map(|path| (ws.clone(), path)))
        .collect();
    let self_users = team_ids
        .into_iter()
        .filter_map(|ws| {
            sessions
                .iter()
                .find(|session| session.team_id == ws)
                .map(|session| (ws, session.user_id.clone()))
        })
        .collect();
    (archives, self_users)
}

/// The catch-up briefing across every existing archive in `archives`.
pub(crate) fn catchup_briefing(
    archives: &[(TeamId, PathBuf)],
    self_users: &HashMap<TeamId, UserId>,
    window: CatchUpWindow,
    now_secs: i64,
    limit: usize,
) -> Result<CatchUpBriefing, CmdError> {
    build_catchup_disk_archives(archives, self_users, window, now_secs, limit)
        .map_err(|err| CmdError::store(err, "building the catch-up briefing"))
}

/// A catch-up briefing as both surfaces return it: the briefing document and its human text.
pub(crate) fn catchup_payload(
    briefing: &CatchUpBriefing,
    multi_workspace: bool,
) -> Result<Payload, CmdError> {
    Ok((
        serde_json::to_value(briefing)?,
        format_catchup_human(briefing, multi_workspace),
    ))
}

pub(crate) fn cmd_catchup(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    since: Option<&str>,
    all_workspaces: bool,
    limit: usize,
) -> Result<ExitCode> {
    let now_secs = now_epoch_secs();
    let window = match parse_catchup_window(since, now_secs) {
        Ok(w) => w,
        Err(msg) => return Ok(fail(ctx, "invalid_args", &msg)),
    };
    let (archives, self_users) = catchup_scope(ctx.sessions(), workspace, all_workspaces);
    if archives.is_empty() {
        let empty = CatchUpBriefing::empty(window, now_secs);
        return Ok(output::empty_local(
            ctx,
            serde_json::to_value(&empty)?,
            "building catch-up briefing",
        ));
    }
    let briefing = catchup_briefing(&archives, &self_users, window, now_secs, limit)?;
    Ok(emit_payload(
        ctx,
        catchup_payload(&briefing, all_workspaces || briefing.workspaces.len() > 1),
    ))
}
