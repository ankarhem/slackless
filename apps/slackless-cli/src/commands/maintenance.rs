use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::types::TeamId;

use super::{archive_hint, empty_local, open_archive_store, open_archive_store_rw};
use crate::{Ctx, archive, output};

pub(crate) fn cmd_stats(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    breakdown: bool,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"workspace": null, "messages": 0}),
            "reading archive stats",
        ));
    };
    let db_path = archive::store_path(workspace);
    let Some(store) = open_archive_store(workspace) else {
        archive_hint(workspace);
        return Ok(ExitCode::SUCCESS);
    };
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
        "db_bytes": stats.db_bytes(),
        "messages": stats.messages,
        "revisions": stats.revisions,
        "conversations": stats.conversations,
        "users": stats.users,
        "offline_queued": stats.offline_queued,
    });
    let mut human = format_stats_human(
        workspace.as_str(),
        db_path.as_deref(),
        stats.db_bytes(),
        stats.messages,
        stats.revisions,
        stats.conversations,
        stats.users,
        stats.offline_queued,
    );
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
                let label = name.as_deref().unwrap_or(id.as_str());
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
    let Some(mut store) = open_archive_store_rw(workspace) else {
        archive_hint(workspace);
        return Ok(ExitCode::SUCCESS);
    };
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn format_stats_human(
    workspace: &str,
    db_path: Option<&std::path::Path>,
    db_bytes: u64,
    messages: u64,
    revisions: u64,
    conversations: u64,
    users: u64,
    offline_queued: u64,
) -> String {
    let path = db_path
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(unknown)".to_owned());
    let size = if db_bytes >= 1024 * 1024 {
        format!("{:.1} MiB", db_bytes as f64 / (1024.0 * 1024.0))
    } else if db_bytes >= 1024 {
        format!("{:.1} KiB", db_bytes as f64 / 1024.0)
    } else {
        format!("{db_bytes} B")
    };
    format!(
        "Workspace:\t{workspace}\nDatabase:\t{path} ({size})\nMessages:\t{messages} ({revisions} revisions)\nConversations:\t{conversations}\nUsers:\t\t{users}\nOffline queue:\t{offline_queued}\n"
    )
}
