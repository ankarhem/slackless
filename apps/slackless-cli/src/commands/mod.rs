pub(crate) mod auth;
pub(crate) mod browse;
pub(crate) mod maintenance;
pub(crate) mod messages;

use std::process::ExitCode;

use slackless_core::store::Store;
use slackless_core::types::{ChannelId, TeamId};

use crate::write::parse_duration_secs;
use crate::{Ctx, archive, output};

/// The workspace's durable store opened read-only; `None` when no archive exists yet (creating
/// one as a side effect of a read would be surprising).
pub(crate) fn open_archive_store(workspace: &TeamId) -> Option<Store> {
    let path = archive::store_path(workspace)?;
    if !path.is_file() {
        return None;
    }
    Store::open_read_only(&path)
        .or_else(|_| Store::open(&path))
        .ok()
}

/// The workspace's durable store opened read-write (for `vacuum`); `None` when no archive exists.
pub(crate) fn open_archive_store_rw(workspace: &TeamId) -> Option<Store> {
    let path = archive::store_path(workspace)?;
    if !path.is_file() {
        return None;
    }
    Store::open(&path).ok()
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

pub(crate) fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
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

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn days_in_month(year: i64, month: u32) -> Option<u32> {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 => Some(if is_leap_year(year) { 29 } else { 28 }),
        _ => None,
    }
}

/// Howard Hinnant's civil-from-days inverse: UTC `YYYY-MM-DD` -> days since `1970-01-01`.
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
