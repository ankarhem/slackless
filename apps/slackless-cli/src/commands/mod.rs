pub(crate) mod auth;
pub(crate) mod browse;
pub(crate) mod maintenance;
pub(crate) mod messages;
pub(crate) mod summarize;

use std::collections::HashSet;

use anyhow::Result;
use slackless_core::auth::Session;
use slackless_core::store::StoreError;
use slackless_core::types::time::{days_from_civil, days_in_month};
use slackless_core::types::{ChannelId, TeamId};

use crate::session::is_valid_workspace_id;
use crate::write::parse_duration_secs;

/// A command's output: the `--json` document and the tab-separated text. Commands build it and
/// never print; the CLI emits it and the MCP server returns the JSON.
pub(crate) type Payload = (serde_json::Value, String);

/// A command failure with the code agents branch on. Store failures map to `store_error` and
/// anything else to `command_failed`, as the CLI's top-level handler does.
#[derive(Debug)]
pub(crate) struct CmdError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl CmdError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// A failed archive read or write, naming the operation that failed.
    pub(crate) fn store(err: impl std::fmt::Display, doing: impl std::fmt::Display) -> Self {
        Self::new("store_error", format!("{doing}: {err}"))
    }
}

impl From<StoreError> for CmdError {
    fn from(err: StoreError) -> Self {
        Self::new("store_error", err.to_string())
    }
}

impl From<anyhow::Error> for CmdError {
    fn from(err: anyhow::Error) -> Self {
        Self::new("command_failed", format!("{err:#}"))
    }
}

impl From<serde_json::Error> for CmdError {
    fn from(err: serde_json::Error) -> Self {
        Self::new("command_failed", err.to_string())
    }
}

impl std::fmt::Display for CmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CmdError {}

/// Every workspace from `sessions` then `archive_ids`, deduplicated while preserving that
/// order and skipping ids that are not safe archive file names.
pub(crate) fn all_workspace_ids(
    sessions: &[Session],
    archive_ids: impl IntoIterator<Item = TeamId>,
) -> Vec<TeamId> {
    let mut seen = HashSet::new();
    sessions
        .iter()
        .map(|s| s.team_id.clone())
        .chain(archive_ids)
        .filter(|id| is_valid_workspace_id(id.as_str()) && seen.insert(id.clone()))
        .collect()
}

/// The channel id when the archive cannot resolve the argument.
pub(crate) fn raw_channel_id(arg: &str) -> ChannelId {
    ChannelId::from(arg.strip_prefix('#').unwrap_or(arg))
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
