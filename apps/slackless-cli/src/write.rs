//! Live write & message commands (`send`, `reply`, `edit`, `delete`, `react`, `upload`,
//! `thread --live`, `mark-read`, `pins`, `pin`, `schedule`, `scheduled`, `unschedule`).
//!
//! They go through `slackless-core`'s REST client with a stored session — one deterministic
//! request per invocation, no websocket session to keep alive. Without a stored session they
//! print a clear "not signed in" message instead of failing hard.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::api::{HistoryOpts, MessageContent, UploadFile};
use slackless_core::store::StoredMessage;
use slackless_core::types::time::now_epoch_secs;
use slackless_core::types::{ChannelId, Message, TeamId, Ts};

use crate::archive::Archive;
use crate::live::session::{
    api_fail, api_for, display_channel, resolve_channel, resolve_channel_with,
};
use crate::output::{self, MessageRow};
use crate::{Ctx, fail};

/// Resolve a message text argument: when `text == "-"`, read the body from stdin.
pub fn resolve_text_arg(text: String) -> Result<String> {
    if text != "-" {
        return Ok(text);
    }
    resolve_text_from(&text, std::io::stdin())
}

/// Pure helper backing [`resolve_text_arg`]: returns `text` verbatim unless `text == "-"`,
/// in which case it reads all of `reader` and strips trailing `\r`/`\n`.
pub(crate) fn resolve_text_from(text: &str, mut reader: impl std::io::Read) -> Result<String> {
    if text != "-" {
        return Ok(text.to_owned());
    }
    let mut buf = String::new();
    reader
        .read_to_string(&mut buf)
        .context("reading message text from stdin")?;
    Ok(buf.trim_end_matches(&['\r', '\n'][..]).to_owned())
}

/// Normalize an emoji shortcode for `reactions.add`/`reactions.remove` by stripping surrounding `:`.
pub(crate) fn normalize_emoji_shortcode(emoji: &str) -> String {
    emoji.trim_matches(':').to_owned()
}

/// Pre-send guard shared across `send`, `reply`, `edit`, `schedule`, `upload`, and
/// `canvas comment`: refuses a blank body locally (Slack answers `no_text` only after a round
/// trip), then possible secrets unless `allow_secrets`.
pub(crate) fn check_outgoing_text(ctx: &Ctx, text: &str, allow_secrets: bool) -> Option<ExitCode> {
    if text.trim().is_empty() {
        return Some(fail(ctx, "invalid_args", "the message text is empty"));
    }
    if allow_secrets {
        return None;
    }
    let hits = slackless_core::detect_secrets(text);
    if hits.is_empty() {
        return None;
    }
    let summary: Vec<String> = hits
        .iter()
        .map(|h| format!("{} ({})", h.kind, h.preview))
        .collect();
    Some(fail(
        ctx,
        "secret_detected",
        &format!(
            "refused to send possible secret(s): {}; pass --allow-secrets to override",
            summary.join(", ")
        ),
    ))
}

/// `send <channel> <text>`: posts and mirrors the echo into the local archive.
pub async fn run_send(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    text: String,
    allow_secrets: bool,
) -> Result<ExitCode> {
    let text = resolve_text_arg(text)?;
    if let Some(exit) = check_outgoing_text(ctx, &text, allow_secrets) {
        return Ok(exit);
    }
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&session.workspace, channel_arg);
    let posted = match session
        .api
        .chat_post_message(&channel, &MessageContent::plain(text), None, false)
        .await
    {
        Ok(posted) => posted,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    record_echo(&session.workspace, &channel, posted.message.as_ref());
    output::emit(
        ctx.json,
        &serde_json::json!({"ok": true, "channel": channel.as_str(), "ts": posted.ts.as_str()}),
        &format!(
            "posted to {} as {}\n",
            display_channel(&channel, label.as_deref()),
            posted.ts
        ),
    );
    Ok(ExitCode::SUCCESS)
}

/// `reply <channel> <thread_ts> <text>`: posts into a thread.
pub async fn run_reply(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    thread_ts: &str,
    text: String,
    allow_secrets: bool,
) -> Result<ExitCode> {
    let text = resolve_text_arg(text)?;
    if let Some(exit) = check_outgoing_text(ctx, &text, allow_secrets) {
        return Ok(exit);
    }
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&session.workspace, channel_arg);
    let thread_ts = Ts::from(thread_ts.to_owned());
    let posted = match session
        .api
        .chat_post_message(
            &channel,
            &MessageContent::plain(text),
            Some(&thread_ts),
            false,
        )
        .await
    {
        Ok(posted) => posted,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    record_echo(&session.workspace, &channel, posted.message.as_ref());
    output::emit(
        ctx.json,
        &serde_json::json!({
            "ok": true,
            "channel": channel.as_str(),
            "thread_ts": thread_ts.as_str(),
            "ts": posted.ts.as_str(),
        }),
        &format!(
            "replied to {} in {} as {}\n",
            thread_ts,
            display_channel(&channel, label.as_deref()),
            posted.ts
        ),
    );
    Ok(ExitCode::SUCCESS)
}

/// `react <channel> <ts> <emoji>`: adds a reaction.
pub async fn run_react(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    ts: &str,
    emoji: &str,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&session.workspace, channel_arg);
    let name = normalize_emoji_shortcode(emoji);
    let ts = Ts::from(ts.to_owned());
    match session.api.reactions_add(&channel, &ts, &name).await {
        Ok(()) => {
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "channel": channel.as_str(),
                    "ts": ts.as_str(),
                    "name": name,
                }),
                &format!(
                    "reacted :{name}: to {} at {}\n",
                    display_channel(&channel, label.as_deref()),
                    ts
                ),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `unreact <channel> <ts> <emoji>`: removes an emoji reaction from a message.
pub async fn run_unreact(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    ts: &str,
    emoji: &str,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&session.workspace, channel_arg);
    let name = normalize_emoji_shortcode(emoji);
    let ts = Ts::from(ts.to_owned());
    match session.api.reactions_remove(&channel, &ts, &name).await {
        Ok(()) => {
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "channel": channel.as_str(),
                    "ts": ts.as_str(),
                    "name": name,
                    "removed": true,
                }),
                &format!(
                    "removed :{name}: from {} at {}\n",
                    display_channel(&channel, label.as_deref()),
                    ts
                ),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `edit <channel> <ts> <text>`: updates an existing message (`chat.update`).
pub async fn run_edit(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    ts: &str,
    text: String,
    allow_secrets: bool,
) -> Result<ExitCode> {
    let text = resolve_text_arg(text)?;
    if let Some(exit) = check_outgoing_text(ctx, &text, allow_secrets) {
        return Ok(exit);
    }
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&session.workspace, channel_arg);
    let ts = Ts::from(ts.to_owned());
    let updated = match session
        .api
        .chat_update(&channel, &ts, &MessageContent::plain(text))
        .await
    {
        Ok(msg) => msg,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    record_echo(&session.workspace, &channel, updated.message.as_ref());
    output::emit(
        ctx.json,
        &serde_json::json!({
            "ok": true,
            "channel": channel.as_str(),
            "ts": updated.ts.as_str(),
        }),
        &format!(
            "updated {} in {}\n",
            updated.ts,
            display_channel(&channel, label.as_deref()),
        ),
    );
    Ok(ExitCode::SUCCESS)
}

/// `delete <channel> <ts>`: deletes a message (`chat.delete`) and mirrors the tombstone locally.
pub async fn run_delete(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    ts: &str,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let mut archive = Archive::open_rw(&session.workspace).ok().flatten();
    let (channel, label) = resolve_channel_with(archive.as_ref(), &session.workspace, channel_arg);
    let ts = Ts::from(ts.to_owned());
    match session.api.chat_delete(&channel, &ts).await {
        Ok(()) => {
            if let Some(archive) = archive.as_mut()
                && let Err(err) = archive.store_mut().record_deletion(
                    &session.workspace,
                    &channel,
                    &ts,
                    None,
                    None,
                )
            {
                eprintln!(
                    "slackless: deleted, but could not record deletion in local archive ({err})"
                );
            }
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "channel": channel.as_str(),
                    "ts": ts.as_str(),
                    "deleted": true,
                }),
                &format!(
                    "deleted {} in {}\n",
                    ts,
                    display_channel(&channel, label.as_deref()),
                ),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `upload <channel> <files...> [--comment TEXT] [--thread-ts TS] [--allow-secrets]`: uploads local files via v2.
pub async fn run_upload(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    paths: &[PathBuf],
    comment: Option<String>,
    thread_ts: Option<&str>,
    allow_secrets: bool,
) -> Result<ExitCode> {
    if paths.is_empty() {
        return Ok(fail(ctx, "invalid_args", "provide at least one file path"));
    }
    let comment = comment.map(resolve_text_arg).transpose()?;
    if let Some(ref text) = comment
        && let Some(exit) = check_outgoing_text(ctx, text, allow_secrets)
    {
        return Ok(exit);
    }
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&session.workspace, channel_arg);
    let mut uploads = Vec::with_capacity(paths.len());
    for path in paths {
        let bytes =
            std::fs::read(path).with_context(|| format!("reading file {}", path.display()))?;
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid file name: {}", path.display()))?
            .to_owned();
        uploads.push(UploadFile {
            filename,
            bytes: bytes.into(),
        });
    }
    let thread_ts = thread_ts.map(|t| Ts::from(t.to_owned()));
    let content = comment.map(MessageContent::plain);
    let files = match session
        .api
        .upload_files(&channel, thread_ts.as_ref(), content.as_ref(), &uploads)
        .await
    {
        Ok(files) => files,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let file_views: Vec<serde_json::Value> = files
        .iter()
        .map(|f| {
            serde_json::json!({
                "id": f.id.as_str(),
                "name": f.name,
                "title": f.title,
                "mimetype": f.mimetype,
                "size": f.size,
                "permalink": f.permalink,
            })
        })
        .collect();
    let names = files
        .iter()
        .map(|f| {
            f.name
                .as_deref()
                .or(f.title.as_deref())
                .unwrap_or(f.id.as_str())
        })
        .collect::<Vec<_>>()
        .join(", ");
    output::emit(
        ctx.json,
        &serde_json::json!({
            "ok": true,
            "channel": channel.as_str(),
            "thread_ts": thread_ts.as_ref().map(Ts::as_str),
            "count": files.len(),
            "files": file_views,
        }),
        &format!(
            "uploaded {} file(s) to {}: {}\n",
            files.len(),
            display_channel(&channel, label.as_deref()),
            names
        ),
    );
    Ok(ExitCode::SUCCESS)
}

/// `thread <channel> <thread_ts> --live`: fetches live thread replies via `conversations.replies`,
/// mirrors them into the local SQLite archive in one batch transaction, and prints them oldest first.
pub async fn run_thread_live(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    thread_ts_arg: &str,
    limit: u32,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let mut archive = Archive::open_or_create_rw(&session.workspace);
    let (channel, label) =
        resolve_channel_with(archive.as_ref().ok(), &session.workspace, channel_arg);
    let thread_ts = Ts::from(thread_ts_arg.to_owned());
    let opts = HistoryOpts {
        limit: Some(limit.clamp(1, 999)),
        ..HistoryOpts::default()
    };
    let fetched_at = now_epoch_secs();
    let page = match session
        .api
        .conversations_replies(&channel, &thread_ts, &opts, None)
        .await
    {
        Ok(page) => page,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };

    let mirrored = archive
        .as_mut()
        .map_err(|err| format!("{err:#}"))
        .and_then(|a| {
            a.store_mut()
                .upsert_replies_page(&session.workspace, &channel, &page.items, fetched_at)
                .map_err(|err| err.to_string())
        });
    // stderr: `--json` consumers read only the replies on stdout.
    if let Err(err) = mirrored {
        eprintln!("slackless: could not mirror the live replies into the local archive ({err})");
    }

    let user_names = archive
        .as_ref()
        .ok()
        .and_then(|a| a.user_display_map(&session.workspace).ok())
        .unwrap_or_default();
    let rows: Vec<MessageRow> = page
        .items
        .into_iter()
        .map(|message| {
            let stored = StoredMessage {
                channel: channel.clone(),
                message,
                deleted: false,
            };
            MessageRow::from_stored(&stored, &user_names)
        })
        .collect();
    let mut human = String::new();
    for row in &rows {
        human.push_str(&row.human_line(label.as_deref()));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({
            "channel": channel.as_str(),
            "thread_ts": thread_ts.as_str(),
            "live": true,
            "count": rows.len(),
            "messages": rows,
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

/// `mark-read <channel> [ts]`: marks `channel` read up to `ts` (or newest locally archived message)
/// and updates `conv.last_read` in the local archive when present.
pub async fn run_mark_read(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    ts_arg: Option<&str>,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let mut archive = Archive::open_rw(&session.workspace).ok().flatten();
    let (channel, label) = resolve_channel_with(archive.as_ref(), &session.workspace, channel_arg);
    let ts_str = match ts_arg {
        Some(ts) => ts.to_owned(),
        None => match archive
            .as_ref()
            .and_then(|a| a.latest_ts(&session.workspace, &channel).ok().flatten())
        {
            Some(ts) => ts,
            None => {
                return Ok(fail(
                    ctx,
                    "missing_ts",
                    "no archived messages for this channel; pass an explicit <TS> to mark read",
                ));
            }
        },
    };
    let ts = Ts::from(ts_str);
    match session.api.conversations_mark(&channel, &ts).await {
        Ok(()) => {
            if let Some(archive) = archive.as_mut()
                && let Ok(Some(mut conv)) =
                    archive.store().conversation(&session.workspace, &channel)
            {
                conv.last_read = Some(ts.clone());
                if let Err(err) = archive
                    .store_mut()
                    .upsert_conversation(&session.workspace, &conv)
                {
                    eprintln!("slackless: marked read, but could not update local archive ({err})");
                }
            }
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "channel": channel.as_str(),
                    "ts": ts.as_str(),
                }),
                &format!(
                    "marked {} read at {}\n",
                    display_channel(&channel, label.as_deref()),
                    ts
                ),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `pins <channel>`: lists pinned messages and files in a conversation (`pins.list`).
pub async fn run_pins(ctx: &Ctx, flag: Option<&str>, channel_arg: &str) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let archive = Archive::open(&session.workspace).ok().flatten();
    let (channel, label) = resolve_channel_with(archive.as_ref(), &session.workspace, channel_arg);
    let pins = match session.api.pins_list(&channel).await {
        Ok(pins) => pins,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let user_names = archive
        .as_ref()
        .and_then(|a| a.user_display_map(&session.workspace).ok())
        .unwrap_or_default();
    let mut human = String::new();
    let json_items: Vec<serde_json::Value> = pins
        .items
        .iter()
        .map(|item| {
            let kind = item.kind.as_deref().unwrap_or("message");
            let created_by_name = item
                .created_by
                .as_ref()
                .and_then(|u| user_names.get(u).cloned());
            if let Some(msg) = &item.message {
                let who = msg
                    .user
                    .as_ref()
                    .and_then(|u| user_names.get(u).cloned())
                    .or_else(|| msg.user.as_ref().map(|u| u.as_str().to_owned()))
                    .unwrap_or_else(|| "unknown".to_owned());
                human.push_str(&format!(
                    "{}\t{}\t{who}: {}\n",
                    label.as_deref().unwrap_or(channel.as_str()),
                    msg.ts,
                    msg.text.replace('\n', " ")
                ));
                serde_json::json!({
                    "type": kind,
                    "created": item.created,
                    "created_by": item.created_by.as_ref().map(|u| u.as_str()),
                    "created_by_name": created_by_name,
                    "ts": msg.ts.as_str(),
                    "user": msg.user.as_ref().map(|u| u.as_str()),
                    "text": msg.text,
                    "permalink": msg.permalink,
                })
            } else {
                let file_name = item
                    .file
                    .as_ref()
                    .and_then(|f| f.name.as_deref().or(f.title.as_deref()))
                    .unwrap_or("(file)");
                human.push_str(&format!(
                    "{}\t{kind}\t{file_name}\n",
                    label.as_deref().unwrap_or(channel.as_str()),
                ));
                serde_json::json!({
                    "type": kind,
                    "created": item.created,
                    "created_by": item.created_by.as_ref().map(|u| u.as_str()),
                    "created_by_name": created_by_name,
                    "file": item.file.as_ref().map(|f| serde_json::json!({
                        "id": f.id.as_str(),
                        "name": f.name,
                        "title": f.title,
                    })),
                })
            }
        })
        .collect();
    output::emit(
        ctx.json,
        &serde_json::json!({
            "channel": channel.as_str(),
            "count": json_items.len(),
            "pins": json_items,
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

/// `pin <channel> <ts>` / `unpin <channel> <ts>`: pins or unpins one message.
pub async fn run_pin(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    ts_arg: &str,
    remove: bool,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&session.workspace, channel_arg);
    let ts = Ts::from(ts_arg.to_owned());
    let res = if remove {
        session.api.pins_remove(&channel, &ts).await
    } else {
        session.api.pins_add(&channel, &ts).await
    };
    match res {
        Ok(()) => {
            let verb = if remove { "unpinned" } else { "pinned" };
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "channel": channel.as_str(),
                    "ts": ts.as_str(),
                    "pinned": !remove,
                }),
                &format!(
                    "{verb} {} in {}\n",
                    ts,
                    display_channel(&channel, label.as_deref()),
                ),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// Parse a schedule `--at` specification into unix epoch seconds:
/// supports relative durations (`+15m`, `+2h`, `+1d`, `+90s`) or an absolute unix epoch timestamp.
pub fn parse_schedule_at(spec: &str, now_secs: u64) -> Result<u64, String> {
    let trimmed = spec.trim();
    if let Some(rel) = trimmed.strip_prefix('+') {
        let delta = parse_duration_secs(rel)?;
        if delta == 0 {
            return Err("schedule offset must be > 0".to_owned());
        }
        return Ok(now_secs.saturating_add(delta));
    }
    let epoch = trimmed.parse::<u64>().map_err(|_| {
        format!("invalid --at value '{trimmed}'; use +15m, +2h, +1d, or a unix timestamp")
    })?;
    if epoch <= now_secs {
        return Err("scheduled time must be in the future".to_owned());
    }
    Ok(epoch)
}

/// Parse a compact duration string like `30s`, `15m`, `24h`, `7d`, `2w` into seconds.
pub fn parse_duration_secs(raw: &str) -> Result<u64, String> {
    let s = raw.trim();
    let mut chars = s.chars();
    let Some(unit) = chars.next_back() else {
        return Err(format!(
            "invalid duration '{raw}'; expected e.g. 30m, 24h, 7d"
        ));
    };
    let num_part = chars.as_str();
    if num_part.is_empty() || !num_part.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "invalid duration '{raw}'; expected e.g. 30m, 24h, 7d"
        ));
    }
    let value: u64 = num_part
        .parse()
        .map_err(|_| format!("invalid duration '{raw}'; expected e.g. 30m, 24h, 7d"))?;
    let mult: u64 = match unit {
        's' | 'S' => 1,
        'm' | 'M' => 60,
        'h' | 'H' => 3600,
        'd' | 'D' => 86_400,
        'w' | 'W' => 604_800,
        _ => {
            return Err(format!(
                "unknown duration unit '{unit}' in '{raw}'; use s, m, h, d, or w"
            ));
        }
    };
    Ok(value.saturating_mul(mult))
}

/// `schedule <channel> <text> --at <WHEN> [--thread-ts TS]`: schedules a message via `chat.scheduleMessage`.
pub async fn run_schedule(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    text: String,
    at: &str,
    thread_ts: Option<&str>,
    allow_secrets: bool,
) -> Result<ExitCode> {
    let text = resolve_text_arg(text)?;
    if let Some(exit) = check_outgoing_text(ctx, &text, allow_secrets) {
        return Ok(exit);
    }
    let now = u64::try_from(now_epoch_secs()).unwrap_or(0);
    let post_at = match parse_schedule_at(at, now) {
        Ok(ts) => ts,
        Err(msg) => return Ok(fail(ctx, "invalid_args", &msg)),
    };
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&session.workspace, channel_arg);
    let thread_ts = thread_ts.map(|t| Ts::from(t.to_owned()));
    match session
        .api
        .chat_schedule_message(
            &channel,
            post_at,
            &MessageContent::plain(text),
            thread_ts.as_ref(),
        )
        .await
    {
        Ok(created) => {
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "scheduled_message_id": created.scheduled_message_id,
                    "channel": created.channel.as_str(),
                    "post_at": created.post_at,
                }),
                &format!(
                    "scheduled {} in {} for {}\n",
                    created.scheduled_message_id,
                    display_channel(&created.channel, label.as_deref()),
                    created.post_at
                ),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `scheduled [--channel CHANNEL]`: lists pending scheduled messages (`chat.scheduledMessages.list`).
pub async fn run_scheduled(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: Option<&str>,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let archive = Archive::open(&session.workspace).ok().flatten();
    let channel =
        channel_arg.map(|arg| resolve_channel_with(archive.as_ref(), &session.workspace, arg).0);
    let items = match session
        .api
        .chat_scheduled_messages_list(channel.as_ref())
        .await
    {
        Ok(items) => items,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let names: HashMap<String, String> = archive
        .as_ref()
        .and_then(|a| a.channel_names(&session.workspace).ok())
        .unwrap_or_default();
    let mut human = String::new();
    let json_items: Vec<serde_json::Value> = items
        .iter()
        .map(|m| {
            let ch_label = names
                .get(m.channel_id.as_str())
                .cloned()
                .unwrap_or_else(|| m.channel_id.to_string());
            human.push_str(&format!(
                "{}\t{ch_label}\t{}\t{}\n",
                m.id,
                m.post_at,
                m.text.replace('\n', " ")
            ));
            serde_json::json!({
                "id": m.id,
                "channel": m.channel_id.as_str(),
                "post_at": m.post_at,
                "date_created": m.date_created,
                "text": m.text,
            })
        })
        .collect();
    output::emit(
        ctx.json,
        &serde_json::json!({
            "count": json_items.len(),
            "scheduled_messages": json_items,
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

/// `unschedule <channel> <id>`: cancels a pending scheduled message (`chat.deleteScheduledMessage`
/// or `drafts.delete`).
pub async fn run_unschedule(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    id: &str,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&session.workspace, channel_arg);
    match session
        .api
        .chat_delete_scheduled_message(&channel, id)
        .await
    {
        Ok(()) => {
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "channel": channel.as_str(),
                    "scheduled_message_id": id,
                }),
                &format!(
                    "cancelled scheduled message {} in {}\n",
                    id,
                    display_channel(&channel, label.as_deref()),
                ),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// Local-first: record the just-sent message so `read` shows it offline too.
fn record_echo(workspace: &TeamId, channel: &ChannelId, message: Option<&Message>) {
    let Some(message) = message else {
        return;
    };
    if let Err(err) = store_echo(workspace, channel, message) {
        eprintln!("slackless: sent, but could not record the echo locally ({err:#})");
    }
}

fn store_echo(workspace: &TeamId, channel: &ChannelId, message: &Message) -> Result<()> {
    let mut archive = Archive::open_or_create_rw(workspace)?;
    archive
        .store_mut()
        .upsert_message(workspace, channel, message)
        .context("recording the sent message")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_secs_handles_units_bounds_and_multibyte_utf8() {
        assert_eq!(parse_duration_secs("30s"), Ok(30));
        assert_eq!(parse_duration_secs("30S"), Ok(30));
        assert_eq!(parse_duration_secs("15m"), Ok(900));
        assert_eq!(parse_duration_secs("15M"), Ok(900));
        assert_eq!(parse_duration_secs("2h"), Ok(7_200));
        assert_eq!(parse_duration_secs("2H"), Ok(7_200));
        assert_eq!(parse_duration_secs("3d"), Ok(259_200));
        assert_eq!(parse_duration_secs("3D"), Ok(259_200));
        assert_eq!(parse_duration_secs("2w"), Ok(1_209_600));
        assert_eq!(parse_duration_secs("2W"), Ok(1_209_600));
        assert_eq!(parse_duration_secs("  10m  "), Ok(600));
        assert_eq!(parse_duration_secs("0s"), Ok(0));

        // Saturating multiplication prevents overflow panic.
        assert_eq!(parse_duration_secs(&format!("{}w", u64::MAX)), Ok(u64::MAX));

        // Short / malformed inputs.
        assert!(parse_duration_secs("").is_err());
        assert!(parse_duration_secs("   ").is_err());
        assert!(parse_duration_secs("5").is_err());
        assert!(parse_duration_secs("m").is_err());
        assert!(parse_duration_secs("abcm").is_err());
        assert!(parse_duration_secs("-5m").is_err());
        assert!(parse_duration_secs("+5m").is_err());
        assert!(parse_duration_secs("1.5h").is_err());
        assert!(parse_duration_secs("10 m").is_err());

        // Unknown ASCII units.
        let err = parse_duration_secs("10x").unwrap_err();
        assert!(err.contains("unknown duration unit 'x'"));

        // Multi-byte UTF-8 trailing characters must return Err cleanly, never panic.
        for bad in ["10µ", "5€", "µ", "€", "10🎉"] {
            assert!(
                parse_duration_secs(bad).is_err(),
                "expected Err for multi-byte input {bad:?}"
            );
        }
    }

    #[test]
    fn parse_schedule_at_edge_cases() {
        let now = 1_700_000_000u64;
        assert_eq!(parse_schedule_at("+90s", now), Ok(now + 90));
        assert_eq!(parse_schedule_at("  +15m  ", now), Ok(now + 900));
        assert_eq!(parse_schedule_at("+1d", now), Ok(now + 86_400));
        assert_eq!(parse_schedule_at("  1700000001  ", now), Ok(now + 1));

        // Relative zero offset and double signs are rejected.
        assert!(parse_schedule_at("+0s", now).is_err());
        assert!(parse_schedule_at("+0m", now).is_err());
        assert!(parse_schedule_at("+0w", now).is_err());
        assert!(parse_schedule_at("++15m", now).is_err());
        assert!(parse_schedule_at("+-15m", now).is_err());
        assert!(parse_schedule_at("+", now).is_err());
        assert_eq!(parse_schedule_at("+2w", now), Ok(now + 1_209_600));
        assert_eq!(parse_schedule_at("+1s", 0), Ok(1));
        assert_eq!(parse_schedule_at("1", 0), Ok(1));
        assert!(parse_schedule_at("0", 0).is_err());

        // Epoch equal to or before `now` is rejected.
        assert!(parse_schedule_at("1700000000", now).is_err());
        assert!(parse_schedule_at("1699999999", now).is_err());
        assert!(parse_schedule_at("-1700005000", now).is_err());
        assert!(parse_schedule_at("2026-01-01", now).is_err());

        // Non-numeric / malformed inputs.
        assert!(parse_schedule_at("tomorrow", now).is_err());
        assert!(parse_schedule_at("", now).is_err());
        assert!(parse_schedule_at("+10€", now).is_err());

        // Relative addition saturates at u64::MAX.
        assert_eq!(parse_schedule_at("+10s", u64::MAX - 5), Ok(u64::MAX));
    }

    #[test]
    fn resolve_text_from_passthrough_and_stdin_trimming() {
        // Literal argument passes through without reading.
        let mut failing_reader = std::io::empty();
        assert_eq!(
            resolve_text_from("hello world\n", &mut failing_reader).unwrap(),
            "hello world\n"
        );

        // "-" reads from reader and trims trailing CR/LF only.
        assert_eq!(
            resolve_text_from("-", "line 1\nline 2\r\n".as_bytes()).unwrap(),
            "line 1\nline 2"
        );
        assert_eq!(
            resolve_text_from("-", "hello\n\r\n".as_bytes()).unwrap(),
            "hello"
        );
        assert_eq!(resolve_text_from("-", "".as_bytes()).unwrap(), "");

        // I/O errors propagate cleanly.
        struct BrokenReader;
        impl std::io::Read for BrokenReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "stdin closed",
                ))
            }
        }
        assert!(resolve_text_from("-", BrokenReader).is_err());
    }

    #[test]
    fn normalize_emoji_shortcode_strips_colons() {
        assert_eq!(normalize_emoji_shortcode(":tada:"), "tada");
        assert_eq!(normalize_emoji_shortcode("tada"), "tada");
        assert_eq!(normalize_emoji_shortcode("::fire::"), "fire");
        assert_eq!(normalize_emoji_shortcode(":+1:"), "+1");
        assert_eq!(normalize_emoji_shortcode("::"), "");
    }
}
