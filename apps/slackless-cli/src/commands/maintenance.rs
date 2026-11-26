use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::read_synced_spans;
use slackless_core::types::time::format_bytes;
use slackless_core::types::{ChannelId, TeamId};

use super::{archive_hint, empty_local};
use crate::archive::{self, Archive, format_channel_name};
use crate::{Ctx, output};

/// Parameters for formatting archive stats in human-readable form.
pub(crate) struct StatsSummary<'a> {
    pub workspace: &'a str,
    pub db_path: Option<&'a Path>,
    pub db_bytes: u64,
    pub messages: u64,
    pub revisions: u64,
    pub conversations: u64,
    pub users: u64,
    pub offline_queued: u64,
}

pub(crate) fn cmd_stats(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    breakdown: bool,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"workspace": null, "archive_exists": false, "messages": 0}),
            "reading archive stats",
        ));
    };
    let db_path = archive::store_path(workspace);
    let Some(archive) = Archive::open(workspace)
        .with_context(|| format!("opening local archive for {workspace}"))?
    else {
        if ctx.json {
            output::emit_json(&serde_json::json!({
                "workspace": workspace.as_str(),
                "db_path": db_path.as_ref().map(|p| p.display().to_string()),
                "archive_exists": false,
                "db_bytes": 0,
                "messages": 0,
                "revisions": 0,
                "conversations": 0,
                "users": 0,
                "offline_queued": 0,
            }));
        } else {
            archive_hint(workspace);
        }
        return Ok(ExitCode::SUCCESS);
    };
    let store = archive.store();
    let stats = store
        .stats(workspace)
        .with_context(|| format!("reading local archive stats for {workspace}"))?;
    let detail = if breakdown {
        Some(
            store
                .storage_breakdown(workspace, 10)
                .with_context(|| format!("reading archive breakdown for {workspace}"))?,
        )
    } else {
        None
    };
    let mut json = serde_json::json!({
        "workspace": workspace.as_str(),
        "db_path": db_path.as_ref().map(|p| p.display().to_string()),
        "archive_exists": true,
        "db_bytes": stats.db_bytes(),
        "messages": stats.messages,
        "revisions": stats.revisions,
        "conversations": stats.conversations,
        "users": stats.users,
        "offline_queued": stats.offline_queued,
    });
    let mut human = format_stats_human(&StatsSummary {
        workspace: workspace.as_str(),
        db_path: db_path.as_deref(),
        db_bytes: stats.db_bytes(),
        messages: stats.messages,
        revisions: stats.revisions,
        conversations: stats.conversations,
        users: stats.users,
        offline_queued: stats.offline_queued,
    });
    if let Some(detail) = detail {
        if let Some(obj) = json.as_object_mut() {
            obj.insert(
                "tables".to_owned(),
                serde_json::json!(
                    detail
                        .tables
                        .iter()
                        .map(|(name, rows)| serde_json::json!({"table": name, "rows": rows}))
                        .collect::<Vec<_>>()
                ),
            );
            obj.insert(
                "top_channels".to_owned(),
                serde_json::json!(detail
                    .top_channels
                    .iter()
                    .map(|(id, name, rows)| serde_json::json!({"id": id, "name": name, "messages": rows}))
                    .collect::<Vec<_>>()),
            );
        }
        human.push_str("\nTables:\n");
        for (name, rows) in &detail.tables {
            human.push_str(&format!("  {name:<18} {rows}\n"));
        }
        if !detail.top_channels.is_empty() {
            human.push_str("\nTop channels by message count:\n");
            for (id, name, rows) in &detail.top_channels {
                let label = name
                    .as_deref()
                    .map(format_channel_name)
                    .unwrap_or_else(|| id.clone());
                human.push_str(&format!("  {label:<28} {rows} ({id})\n"));
            }
        }
    }
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn cmd_vacuum(ctx: &Ctx, workspace: Option<&TeamId>) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"ok": false, "reason": "no_workspace"}),
            "compacting archive",
        ));
    };
    let Some(mut archive) = Archive::open_rw(workspace)
        .with_context(|| format!("opening local archive for {workspace}"))?
    else {
        if ctx.json {
            output::emit_json(&serde_json::json!({
                "ok": false,
                "reason": "no_archive",
                "workspace": workspace.as_str(),
            }));
        } else {
            archive_hint(workspace);
        }
        return Ok(ExitCode::SUCCESS);
    };
    let store = archive.store_mut();
    let before = store.stats(workspace).map(|s| s.db_bytes()).unwrap_or(0);
    store
        .vacuum()
        .with_context(|| format!("compacting local archive for {workspace}"))?;
    let after = store.stats(workspace).map(|s| s.db_bytes()).unwrap_or(0);
    let reclaimed = before.saturating_sub(after);
    let json = serde_json::json!({
        "ok": true,
        "workspace": workspace.as_str(),
        "before_bytes": before,
        "after_bytes": after,
        "reclaimed_bytes": reclaimed,
    });
    let human = format!(
        "Compacted {workspace}: {:.1} MiB -> {:.1} MiB (reclaimed {:.1} MiB)\n",
        before as f64 / (1024.0 * 1024.0),
        after as f64 / (1024.0 * 1024.0),
        reclaimed as f64 / (1024.0 * 1024.0),
    );
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn format_stats_human(summary: &StatsSummary<'_>) -> String {
    let path = summary
        .db_path
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(unknown)".to_owned());
    let size = format_bytes(summary.db_bytes);
    format!(
        "Workspace:\t{}\nDatabase:\t{path} ({size})\nMessages:\t{} ({} revisions)\nConversations:\t{}\nUsers:\t\t{}\nOffline queue:\t{}\n",
        summary.workspace,
        summary.messages,
        summary.revisions,
        summary.conversations,
        summary.users,
        summary.offline_queued,
    )
}

/// Per-channel archive backfill coverage row for `slackless archive status`.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ArchiveChannelStatusRow {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub messages: u64,
    pub complete: bool,
    pub oldest_ts: Option<String>,
    pub newest_ts: Option<String>,
}

/// `archive status`: show per-channel local backfill progress and archive coverage.
pub(crate) fn cmd_archive_status(ctx: &Ctx, workspace: Option<&TeamId>) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({
                "workspace": null,
                "archive_exists": false,
                "total_channels": 0,
                "completed_channels": 0,
                "synced_channels": 0,
                "total_messages": 0,
                "db_bytes": 0,
                "channels": [],
            }),
            "reading archive backfill status",
        ));
    };
    let archive = Archive::open(workspace)
        .with_context(|| format!("opening local archive for {workspace}"))?;
    if archive.is_none() && !ctx.json {
        archive_hint(workspace);
        return Ok(ExitCode::SUCCESS);
    }
    cmd_archive_status_in_archive(ctx, workspace, archive.as_ref())
}

pub(crate) fn cmd_archive_status_in_archive(
    ctx: &Ctx,
    workspace: &TeamId,
    archive: Option<&Archive>,
) -> Result<ExitCode> {
    let Some(archive) = archive else {
        output::emit(
            ctx.json,
            &serde_json::json!({
                "workspace": workspace.as_str(),
                "archive_exists": false,
                "total_channels": 0,
                "completed_channels": 0,
                "synced_channels": 0,
                "total_messages": 0,
                "db_bytes": 0,
                "channels": [],
            }),
            "",
        );
        return Ok(ExitCode::SUCCESS);
    };

    let (json, human) = build_archive_status_report(workspace, archive)?;
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn build_archive_status_report(
    workspace: &TeamId,
    archive: &Archive,
) -> Result<(serde_json::Value, String)> {
    let store = archive.store();
    let stats = store
        .stats(workspace)
        .with_context(|| format!("reading local archive stats for {workspace}"))?;
    let spans = read_synced_spans(store, workspace);
    let counts = store
        .channel_message_counts(workspace)
        .with_context(|| format!("reading channel message counts for {workspace}"))?;
    let channels = archive
        .channels(workspace)
        .with_context(|| format!("listing channels for {workspace}"))?;
    let dms = archive
        .dms(workspace)
        .with_context(|| format!("listing dms for {workspace}"))?;

    let mut by_id: HashMap<ChannelId, (String, String)> = HashMap::new();
    for ch in channels.into_iter().chain(dms) {
        by_id.insert(ch.id.clone(), (ch.label(), ch.kind));
    }

    let mut all_ids: BTreeMap<String, ChannelId> = BTreeMap::new();
    for id in by_id.keys() {
        all_ids.insert(id.as_str().to_owned(), id.clone());
    }
    for id in spans.keys() {
        all_ids.insert(id.as_str().to_owned(), id.clone());
    }
    for id in counts.keys() {
        all_ids.insert(id.as_str().to_owned(), id.clone());
    }

    let mut rows: Vec<ArchiveChannelStatusRow> = all_ids
        .into_values()
        .map(|id| {
            let (label, kind) = by_id
                .get(&id)
                .cloned()
                .unwrap_or_else(|| (id.as_str().to_owned(), "unknown".to_owned()));
            let span = spans.get(&id);
            let messages = counts.get(&id).copied().unwrap_or(0);
            ArchiveChannelStatusRow {
                id: id.as_str().to_owned(),
                label,
                kind,
                messages,
                complete: span.is_some_and(|s| s.complete),
                oldest_ts: span.map(|s| s.oldest.as_str().to_owned()),
                newest_ts: span.map(|s| s.newest.as_str().to_owned()),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.complete
            .cmp(&a.complete)
            .then_with(|| b.messages.cmp(&a.messages))
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });

    let total_channels = rows.len();
    let completed_channels = rows.iter().filter(|r| r.complete).count();
    let synced_channels = rows
        .iter()
        .filter(|r| r.complete || r.messages > 0 || r.oldest_ts.is_some())
        .count();

    let mut human = format!(
        "Workspace:\t{}\nArchive size:\t{}\nMessages:\t{}\nBackfilled:\t{completed_channels}/{total_channels} complete ({synced_channels} with local history)\n",
        workspace.as_str(),
        format_bytes(stats.db_bytes()),
        stats.messages,
    );
    if !rows.is_empty() {
        human.push('\n');
        for row in &rows {
            let status = if row.complete {
                "complete"
            } else if row.messages > 0 || row.oldest_ts.is_some() {
                "partial"
            } else {
                "idle"
            };
            human.push_str(&format!(
                "{}\t{}\t{status}\t{} msgs\toldest={}\n",
                row.id,
                row.label,
                row.messages,
                row.oldest_ts.as_deref().unwrap_or("-"),
            ));
        }
    }

    let json = serde_json::json!({
        "workspace": workspace.as_str(),
        "archive_exists": true,
        "total_channels": total_channels,
        "completed_channels": completed_channels,
        "synced_channels": synced_channels,
        "total_messages": stats.messages,
        "db_bytes": stats.db_bytes(),
        "channels": rows,
    });
    Ok((json, human))
}
