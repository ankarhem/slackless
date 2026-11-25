use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::types::TeamId;
use slackless_core::types::time::format_bytes;

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
