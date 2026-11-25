//! `mark-thread` and `saved` (Later) live commands.

use std::process::ExitCode;

use anyhow::Result;
use slackless_core::api::{SavedItem, SavedMark};
use slackless_core::types::{ChannelId, Ts};

use crate::Ctx;
use crate::output;

use super::session::{api_fail, api_for, display_channel, resolve_channel};

/// Selects the read cursor timestamp for `mark-thread`.
/// Moving the read cursor to `"0"` when marking unread without an explicit `ts` marks all replies unread.
pub(crate) fn mark_thread_cursor(thread_ts: &str, ts: Option<&str>, unread: bool) -> Ts {
    if unread && ts.is_none() {
        Ts::from("0".to_owned())
    } else {
        Ts::from(ts.unwrap_or(thread_ts).to_owned())
    }
}

pub(crate) fn format_mark_thread_output(
    channel: &ChannelId,
    label: Option<&str>,
    root: &Ts,
    cursor: &Ts,
    read: bool,
) -> (serde_json::Value, String) {
    let state = if read { "read" } else { "unread" };
    let json = serde_json::json!({
        "ok": true,
        "channel": channel.as_str(),
        "thread_ts": root.as_str(),
        "ts": cursor.as_str(),
        "read": read,
    });
    let human = format!(
        "marked thread {} in {} as {state}\n",
        root,
        display_channel(channel, label)
    );
    (json, human)
}

/// `mark-thread <channel> <thread_ts> [ts] [--unread]`
pub async fn run_mark_thread(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    thread_ts: &str,
    ts: Option<&str>,
    unread: bool,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&session.workspace, channel_arg);
    let root = Ts::from(thread_ts.to_owned());
    let cursor = mark_thread_cursor(thread_ts, ts, unread);
    let read = !unread;
    match session
        .api
        .subscriptions_thread_mark(&channel, &root, &cursor)
        .await
    {
        Ok(()) => {
            let (json, human) =
                format_mark_thread_output(&channel, label.as_deref(), &root, &cursor, read);
            output::emit(ctx.json, &json, &human);
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

pub(crate) fn format_saved_list(filter: &str, items: &[SavedItem]) -> (serde_json::Value, String) {
    let json_items: Vec<serde_json::Value> = items
        .iter()
        .map(|it| {
            serde_json::json!({
                "channel": it.channel.as_str(),
                "ts": it.ts.as_str(),
                "state": format!("{:?}", it.state).to_lowercase(),
                "date_due": it.date_due,
                "text": it.message.as_ref().map(|m| m.text.as_str()),
            })
        })
        .collect();
    let mut human = String::new();
    for it in items {
        let preview = it
            .message
            .as_ref()
            .map(|m| m.text.as_str())
            .unwrap_or("(no preview)");
        human.push_str(&format!(
            "{}\t{}\t{:?}\t{}\n",
            it.channel, it.ts, it.state, preview
        ));
    }
    let json = serde_json::json!({ "filter": filter, "count": items.len(), "items": json_items });
    (json, human)
}

/// Normalize CLI filter aliases (`in_progress` -> `saved`) to Slack's `saved.list` values.
pub(crate) fn normalize_saved_filter(filter: &str) -> &str {
    match filter.trim() {
        "in_progress" | "" => "saved",
        other => other,
    }
}

/// `saved list [--filter saved|completed|archived] [--limit N]`
pub async fn run_saved_list(
    ctx: &Ctx,
    flag: Option<&str>,
    filter: &str,
    limit: u32,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let filter = normalize_saved_filter(filter);
    let items = match session.api.saved_list(filter, limit).await {
        Ok(items) => items,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let (json, human) = format_saved_list(filter, &items);
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

/// `saved add <channel> <ts> [--due <unix_ts>]`
pub async fn run_saved_add(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    ts: &str,
    due: Option<u64>,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&session.workspace, channel_arg);
    let ts = Ts::from(ts.to_owned());
    match session.api.saved_add(&channel, &ts, due).await {
        Ok(()) => {
            output::emit(
                ctx.json,
                &serde_json::json!({ "ok": true, "channel": channel.as_str(), "ts": ts.as_str(), "date_due": due }),
                &format!(
                    "saved {} in {}\n",
                    ts,
                    display_channel(&channel, label.as_deref())
                ),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `saved done <channel> <ts>`
pub async fn run_saved_done(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    ts: &str,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, _) = resolve_channel(&session.workspace, channel_arg);
    let ts = Ts::from(ts.to_owned());
    match session
        .api
        .saved_update(&channel, &ts, Some(SavedMark::Completed), None)
        .await
    {
        Ok(()) => {
            output::emit(
                ctx.json,
                &serde_json::json!({ "ok": true, "channel": channel.as_str(), "ts": ts.as_str(), "state": "completed" }),
                &format!("marked saved item {} in {} completed\n", ts, channel),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `saved delete <channel> <ts>`
pub async fn run_saved_delete(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    ts: &str,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, _) = resolve_channel(&session.workspace, channel_arg);
    let ts = Ts::from(ts.to_owned());
    match session.api.saved_delete(&channel, &ts).await {
        Ok(()) => {
            output::emit(
                ctx.json,
                &serde_json::json!({ "ok": true, "channel": channel.as_str(), "ts": ts.as_str() }),
                &format!("removed saved item {} in {}\n", ts, channel),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}
