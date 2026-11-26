use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::embeddings::{
    Embedder, EmbeddingDownloadProgress, EmbeddingModel, OnnxEmbedder, delete_embedding_model,
    detect_embedding_model_setup, download_embedding_model_async,
};
use slackless_core::read_synced_spans;
use slackless_core::semantic_index::{
    SemanticIndexReport, default_semantic_index_stats, index_disk_archives,
    open_default_semantic_index,
};
use slackless_core::store::SemanticIndex;
use slackless_core::types::time::format_bytes;
use slackless_core::types::{ChannelId, TeamId};

use super::{all_workspace_ids, archive_hint, empty_local, fail_no_archive, fail_no_workspace};
use crate::archive::{self, Archive, format_channel_name};
use crate::cli::{ArchiveIndexSemanticArgs, ArchivePruneArgs, SemanticModelCommand};
use crate::{Ctx, fail, output};
use slackless_core::maintenance::{
    MediaCachePolicy, PruneReport, prune_default_media_caches, prune_default_semantic_index,
};
use slackless_core::store::IntegrityMode;

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
        return Ok(fail_no_workspace(ctx, "compacting the archive"));
    };
    let Some(mut archive) = Archive::open_rw(workspace)
        .with_context(|| format!("opening local archive for {workspace}"))?
    else {
        return Ok(fail_no_archive(ctx, workspace));
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

/// `archive check`: run `PRAGMA quick_check` (or `integrity_check` with `--full`) on the local
/// archive. Exits non-zero when SQLite reports problems.
pub(crate) fn cmd_archive_check(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    full: bool,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(fail_no_workspace(ctx, "checking archive integrity"));
    };
    let Some(path) = archive::store_path(workspace).filter(|p| p.is_file()) else {
        return Ok(fail_no_archive(ctx, workspace));
    };
    let mode = if full {
        IntegrityMode::Full
    } else {
        IntegrityMode::Quick
    };
    let (json, human, ok) = archive_check_report(workspace, &path, mode);
    output::emit(ctx.json, &json, &human);
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Integrity report for the archive at `path` as `(json, human, ok)`. An archive SQLite cannot
/// even open counts as a failed check rather than a command error.
pub(crate) fn archive_check_report(
    workspace: &TeamId,
    path: &Path,
    mode: IntegrityMode,
) -> (serde_json::Value, String, bool) {
    let outcome = Archive::open_path(path)
        .map_err(|e| e.to_string())
        .and_then(|a| a.ok_or_else(|| "archive file not found".to_owned()))
        .and_then(|archive| {
            let store = archive.store();
            let report = store
                .integrity_check_with(mode)
                .map_err(|e| e.to_string())?;
            let info = store.file_info().unwrap_or_default();
            Ok((report, info))
        });
    let (ok, problems, elapsed_ms, info) = match outcome {
        Ok((report, info)) => (report.ok, report.problems, report.elapsed_ms, info),
        Err(err) => (false, vec![err], 0, Default::default()),
    };
    let json = serde_json::json!({
        "ok": ok,
        "workspace": workspace.as_str(),
        "db_path": path.display().to_string(),
        "mode": mode,
        "check": mode.pragma(),
        "problems": problems,
        "elapsed_ms": elapsed_ms,
        "db_bytes": info.db_bytes,
        "wal_bytes": info.wal_bytes,
        "freelist_bytes": info.freelist_bytes,
    });
    let mut human = format!(
        "Workspace:\t{}\nDatabase:\t{} ({}, WAL {}, {} reclaimable)\nIntegrity:\t{} ({}, {} ms)\n",
        workspace.as_str(),
        path.display(),
        format_bytes(info.db_bytes),
        format_bytes(info.wal_bytes),
        format_bytes(info.freelist_bytes),
        if ok { "ok" } else { "FAILED" },
        mode.pragma(),
        elapsed_ms,
    );
    for problem in &problems {
        human.push_str(&format!("  - {problem}\n"));
    }
    if !ok {
        human.push_str(
            "Back up the archive (`slackless archive backup <path>`), then clear the local cache in the app to re-sync.\n",
        );
    }
    (json, human, ok)
}

/// `archive backup <output_path>`: consistent online snapshot of the local archive.
pub(crate) fn cmd_archive_backup(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    output: &Path,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(fail_no_workspace(ctx, "backing up the archive"));
    };
    let Some(path) = archive::store_path(workspace).filter(|p| p.is_file()) else {
        return Ok(fail_no_archive(ctx, workspace));
    };
    let (json, human) = archive_backup_report(workspace, &path, output)?;
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

/// Back up the archive at `source` to `dest`, returning `(json, human)`.
pub(crate) fn archive_backup_report(
    workspace: &TeamId,
    source: &Path,
    dest: &Path,
) -> Result<(serde_json::Value, String)> {
    let archive = Archive::open_path(source)
        .with_context(|| format!("opening local archive {}", source.display()))?
        .with_context(|| format!("local archive {} not found", source.display()))?;
    let started = std::time::Instant::now();
    let bytes = archive
        .store()
        .backup_to(dest)
        .with_context(|| format!("backing up {workspace} to {}", dest.display()))?;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let written = dest.canonicalize().unwrap_or_else(|_| dest.to_path_buf());
    let json = serde_json::json!({
        "ok": true,
        "workspace": workspace.as_str(),
        "source": source.display().to_string(),
        "output": written.display().to_string(),
        "bytes": bytes,
        "elapsed_ms": elapsed_ms,
    });
    let human = format!(
        "Backed up {workspace} to {} ({}, {elapsed_ms} ms)\n",
        written.display(),
        format_bytes(bytes),
    );
    Ok((json, human))
}

/// `archive prune`: apply size/age retention to every media cache directory.
pub(crate) fn cmd_archive_prune(ctx: &Ctx, args: &ArchivePruneArgs) -> Result<ExitCode> {
    let policy = MediaCachePolicy::from_mb_days(args.max_mb, args.max_days);
    let report = prune_default_media_caches(policy);
    let semantic = if args.semantic {
        Some(prune_default_semantic_index().map_err(|e| anyhow::anyhow!(e))?)
    } else {
        None
    };
    let (json, human) = prune_report_output(policy, &report, semantic);
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn prune_report_output(
    policy: MediaCachePolicy,
    report: &PruneReport,
    semantic_pruned: Option<usize>,
) -> (serde_json::Value, String) {
    let json = serde_json::json!({
        "ok": true,
        "policy": policy,
        "media": report,
        "semantic_vectors_pruned": semantic_pruned,
    });
    let mut human = format!(
        "Media cache:\tremoved {} file(s) ({}); {} file(s) ({}) remain\n",
        report.removed_files,
        format_bytes(report.removed_bytes),
        report.remaining_files,
        format_bytes(report.remaining_bytes),
    );
    if let Some(n) = semantic_pruned {
        human.push_str(&format!("Semantic index:\tpruned {n} orphaned vector(s)\n"));
    }
    (json, human)
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

/// Reports embedding-model download progress on stderr for humans; `--json` output stays clean.
fn download_progress(json_mode: bool) -> impl Fn(EmbeddingDownloadProgress) {
    move |p| {
        if json_mode {
            return;
        }
        let pct = p
            .total_bytes
            .filter(|&t| t > 0)
            .map(|t| format!(" ({}%)", (p.downloaded_bytes * 100 / t).min(100)))
            .unwrap_or_default();
        eprintln!(
            "[{}/{}] {} {}{}",
            p.file_index,
            p.file_count,
            p.file_name,
            format_bytes(p.downloaded_bytes),
            pct
        );
    }
}

/// `archive semantic-model status | download | delete`.
pub(crate) async fn cmd_archive_semantic_model(
    ctx: &Ctx,
    command: SemanticModelCommand,
) -> Result<ExitCode> {
    match command {
        SemanticModelCommand::Status { model } => emit_semantic_model_status(ctx, &model),
        SemanticModelCommand::Download { model } => {
            let Some(parsed) = EmbeddingModel::parse(&model) else {
                return Ok(fail(
                    ctx,
                    "invalid_model",
                    &format!("unknown embedding model '{model}'; expected 'minilm' or 'bge-small'"),
                ));
            };
            let dir = download_embedding_model_async(parsed, download_progress(ctx.json))
                .await
                .map_err(|e| anyhow::anyhow!("downloading embedding model: {e}"))?;
            let status = detect_embedding_model_setup(parsed);
            let human = format!(
                "Downloaded {} to {} ({})\n",
                parsed.slug(),
                dir.display(),
                format_bytes(status.disk_bytes)
            );
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "model": parsed.slug(),
                    "model_ready": status.model_ready,
                    "model_path": status.model_path.display().to_string(),
                    "disk_bytes": status.disk_bytes,
                    "dims": status.dims,
                }),
                &human,
            );
            Ok(ExitCode::SUCCESS)
        }
        SemanticModelCommand::Delete { model } => {
            let Some(parsed) = EmbeddingModel::parse(&model) else {
                return Ok(fail(
                    ctx,
                    "invalid_model",
                    &format!("unknown embedding model '{model}'; expected 'minilm' or 'bge-small'"),
                ));
            };
            let reclaimed = delete_embedding_model(parsed)
                .map_err(|e| anyhow::anyhow!("deleting embedding model: {e}"))?;
            let human = format!(
                "Removed {} (reclaimed {})\n",
                parsed.slug(),
                format_bytes(reclaimed)
            );
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "model": parsed.slug(),
                    "reclaimed_bytes": reclaimed,
                }),
                &human,
            );
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn emit_semantic_model_status(ctx: &Ctx, model_arg: &str) -> Result<ExitCode> {
    let Some(model) = EmbeddingModel::parse(model_arg) else {
        return Ok(fail(
            ctx,
            "invalid_model",
            &format!("unknown embedding model '{model_arg}'; expected 'minilm' or 'bge-small'"),
        ));
    };
    let setup = detect_embedding_model_setup(model);
    let idx_stats = default_semantic_index_stats();
    let idx_path = slackless_core::paths::semantic_index_path();
    let human = format!(
        "Model:\t\t{} ({})\nModel path:\t{} ({})\nIndex path:\t{} ({})\nIndexed docs:\t{} total ({} messages, {} snippets, {} canvases, {} file transcripts, {} huddle transcripts)\nWorkspaces:\t{}\n",
        model.slug(),
        if setup.model_ready {
            "ready"
        } else {
            "not downloaded"
        },
        setup.model_path.display(),
        if setup.model_ready {
            format_bytes(setup.disk_bytes)
        } else {
            setup.download_size_hint.clone()
        },
        idx_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(unknown)".to_owned()),
        format_bytes(idx_stats.db_bytes),
        idx_stats.total_docs,
        idx_stats.messages,
        idx_stats.snippets,
        idx_stats.canvases,
        idx_stats.file_transcripts,
        idx_stats.huddle_transcripts,
        idx_stats.workspaces,
    );
    output::emit(
        ctx.json,
        &serde_json::json!({
            "model": model.slug(),
            "model_ready": setup.model_ready,
            "model_path": setup.model_path.display().to_string(),
            "model_bytes": setup.disk_bytes,
            "download_size_hint": setup.download_size_hint,
            "dims": setup.dims,
            "index_path": idx_path.as_ref().map(|p| p.display().to_string()),
            "index": idx_stats,
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

/// `archive index-semantic`: incrementally embed messages, snippets, canvases, and transcripts
/// into `semantic_index.sqlite`.
pub(crate) async fn cmd_archive_index_semantic(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    args: ArchiveIndexSemanticArgs,
) -> Result<ExitCode> {
    if args.status {
        return emit_semantic_model_status(ctx, &args.model);
    }
    let Some(model) = EmbeddingModel::parse(&args.model) else {
        return Ok(fail(
            ctx,
            "invalid_model",
            &format!(
                "unknown embedding model '{}'; expected 'minilm' or 'bge-small'",
                args.model
            ),
        ));
    };

    let mut setup = detect_embedding_model_setup(model);
    if !setup.model_ready {
        if args.download {
            download_embedding_model_async(model, download_progress(ctx.json))
                .await
                .map_err(|e| anyhow::anyhow!("downloading embedding model: {e}"))?;
            setup = detect_embedding_model_setup(model);
        } else {
            return Ok(fail(
                ctx,
                "semantic_model_missing",
                &format!(
                    "local embedding model {} is not downloaded ({}). Re-run with `--download` or run `slackless archive semantic-model download --model {}`.",
                    model.slug(),
                    setup.download_size_hint,
                    model.slug()
                ),
            ));
        }
    }

    let archives: Vec<(TeamId, PathBuf)> = if args.single_workspace {
        let Some(ws) = workspace else {
            return Ok(fail_no_workspace(ctx, "indexing semantic vectors"));
        };
        let Some(path) = archive::store_path(ws).filter(|p| p.is_file()) else {
            return Ok(fail_no_archive(ctx, ws));
        };
        vec![(ws.clone(), path)]
    } else {
        all_workspace_ids(ctx.sessions())
            .into_iter()
            .filter_map(|ws| archive::store_path(&ws).map(|p| (ws, p)))
            .collect()
    };

    if archives.is_empty() {
        return Ok(empty_local(
            ctx,
            serde_json::json!({
                "ok": true,
                "workspaces_indexed": [],
                "total_seen": 0,
                "embedded": 0,
                "skipped_unchanged": 0,
                "pruned": 0,
            }),
            "indexing semantic vectors",
        ));
    }

    let mut index = open_default_semantic_index().map_err(|e| anyhow::anyhow!("{e}"))?;
    if args.rebuild {
        index
            .clear()
            .context("clearing semantic_index.sqlite before rebuild")?;
    }
    let embedder = OnnxEmbedder::load_from_dir(model, &setup.model_path)
        .map_err(|e| anyhow::anyhow!("loading ONNX embedding model: {e}"))?;
    let report = index_semantic_archives_with_embedder(ctx, &archives, &mut index, &embedder)?;
    emit_semantic_index_report(ctx, &report, &index)?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn index_semantic_archives_with_embedder(
    ctx: &Ctx,
    archives: &[(TeamId, PathBuf)],
    index: &mut SemanticIndex,
    embedder: &dyn Embedder,
) -> Result<SemanticIndexReport> {
    let json_mode = ctx.json;
    index_disk_archives(archives, index, embedder, |prog| {
        if !json_mode && prog.total > 0 {
            eprintln!(
                "Indexing {} ({}/{} docs; {} embedded, {} unchanged)",
                prog.workspace, prog.processed, prog.total, prog.embedded, prog.skipped
            );
        }
    })
    .map_err(|e| anyhow::anyhow!("{e}"))
}

fn emit_semantic_index_report(
    ctx: &Ctx,
    report: &SemanticIndexReport,
    index: &SemanticIndex,
) -> Result<()> {
    let stats = index.stats().context("reading semantic index stats")?;
    let human = format!(
        "Indexed {} workspace(s) with {} ({} dims)\nDocuments:\t{} seen ({} embedded, {} skipped unchanged, {} pruned)\nTotal indexed:\t{} ({})\n",
        report.workspaces_indexed.len(),
        report.model_id,
        report.dims,
        report.total_seen,
        report.embedded,
        report.skipped_unchanged,
        report.pruned,
        stats.total_docs,
        format_bytes(stats.db_bytes),
    );
    output::emit(
        ctx.json,
        &serde_json::json!({
            "ok": true,
            "report": report,
            "index": stats,
        }),
        &human,
    );
    Ok(())
}
