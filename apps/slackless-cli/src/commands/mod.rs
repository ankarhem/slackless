pub(crate) mod auth;
pub(crate) mod browse;
pub(crate) mod maintenance;
pub(crate) mod messages;
pub(crate) mod summarize;

use std::collections::HashSet;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::auth::Session;
use slackless_core::types::time::{days_from_civil, days_in_month};
use slackless_core::types::{ChannelId, TeamId};

use crate::archive::{self, Archive};
use crate::write::parse_duration_secs;
use crate::{Ctx, output};

/// Opens a workspace archive read-only, propagating genuine SQLite errors while printing a
/// stderr hint in human mode when no archive file exists yet.
pub(crate) fn open_archive_with_hint(ctx: &Ctx, workspace: &TeamId) -> Result<Option<Archive>> {
    let archive = Archive::open(workspace)
        .with_context(|| format!("opening local archive for {workspace}"))?;
    if archive.is_none() && !ctx.json {
        archive_hint(workspace);
    }
    Ok(archive)
}

/// Every workspace known from stored sessions or local archive files, deduplicated in O(n)
/// while preserving session-first order.
pub(crate) fn all_workspace_ids(sessions: &[Session]) -> Vec<TeamId> {
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for s in sessions {
        if seen.insert(s.team_id.clone()) {
            ids.push(s.team_id.clone());
        }
    }
    for tid in archive::stored_team_ids() {
        if seen.insert(tid.clone()) {
            ids.push(tid);
        }
    }
    ids
}

/// Write rendered export output to `out` (emitting a JSON or human confirmation) or print to stdout.
pub(crate) fn write_or_print_export(
    ctx: &Ctx,
    rendered: &str,
    out: Option<&Path>,
    count: usize,
    item_noun: &str,
) -> Result<()> {
    if let Some(path) = out {
        std::fs::write(path, rendered)
            .with_context(|| format!("writing export to {}", path.display()))?;
        if ctx.json {
            output::emit_json(&serde_json::json!({
                "ok": true,
                "path": path.display().to_string(),
                "count": count,
            }));
        } else {
            eprintln!("Exported {count} {item_noun} to {}", path.display());
        }
    } else {
        print!("{rendered}");
        if !rendered.ends_with('\n') {
            println!();
        }
    }
    Ok(())
}

/// The channel id when the archive cannot resolve the argument.
pub(crate) fn raw_channel_id(arg: &str) -> ChannelId {
    ChannelId::from(arg.strip_prefix('#').unwrap_or(arg))
}

/// Prints empty `--json` results (or a human hint) when no workspace is known: local commands
/// succeed with empty output rather than failing hard offline.
pub(crate) fn empty_local(ctx: &Ctx, empty: serde_json::Value, action: &str) -> ExitCode {
    if ctx.json {
        output::emit_json(&empty);
    } else {
        eprintln!(
            "slackless: no workspace known for {action}; pass --workspace TEAM_ID (run `slackless auth login` first)"
        );
    }
    ExitCode::SUCCESS
}

/// A note on stderr when a workspace has no local archive to read.
pub(crate) fn archive_hint(workspace: &TeamId) {
    eprintln!(
        "slackless: no local archive for workspace {workspace} yet; it fills up once the desktop app syncs"
    );
}

/// Parse a `--since` cutoff (`30m`, `24h`, `7d`, `2w`, `YYYY-MM-DD`, or unix seconds) into
/// unix epoch seconds.
pub(crate) fn parse_since_cutoff(spec: &str, now_secs: i64) -> Result<i64, String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err("empty --since value".to_owned());
    }
    if let Ok(epoch) = trimmed.parse::<i64>() {
        return Ok(epoch);
    }
    if let Some((y, rest)) = trimmed.split_once('-')
        && let Some((m, d)) = rest.split_once('-')
        && y.len() == 4
        && (1..=2).contains(&m.len())
        && (1..=2).contains(&d.len())
        && y.bytes().all(|b| b.is_ascii_digit())
        && m.bytes().all(|b| b.is_ascii_digit())
        && d.bytes().all(|b| b.is_ascii_digit())
        && let (Ok(year), Ok(month), Ok(day)) =
            (y.parse::<i64>(), m.parse::<u32>(), d.parse::<u32>())
    {
        let Some(max_day) = days_in_month(year, month) else {
            return Err(format!("invalid date '{trimmed}': month must be 1..=12"));
        };
        if day == 0 || day > max_day {
            return Err(format!(
                "invalid date '{trimmed}': day must be 1..={max_day} for {year:04}-{month:02}"
            ));
        }
        return Ok(days_from_civil(year, month, day).saturating_mul(86_400));
    }
    let delta = parse_duration_secs(trimmed)?;
    let delta_i64 = i64::try_from(delta).unwrap_or(i64::MAX);
    Ok(now_secs.saturating_sub(delta_i64))
}
