//! Live write commands (`send`, `reply`, `react`).
//!
//! They go through `slackless-core`'s REST client with a stored session — one deterministic
//! request per invocation, no websocket session to keep alive. Without a stored session they
//! print a clear "not signed in" message instead of failing hard.

use std::io::Read as _;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::api::{ApiError, HistoryOpts, MessageContent, SlackApi, UploadFile};
use slackless_core::store::{Store, StoredMessage};
use slackless_core::types::{ChannelId, Message, TeamId, Ts};

use crate::archive::{self, Archive};
use crate::output::{self, MessageRow};
use crate::session::resolve_session;
use crate::{Ctx, fail};

/// Resolve a message text argument: when `text == "-"`, read the body from stdin.
pub fn resolve_text_arg(text: String) -> Result<String> {
    if text != "-" {
        return Ok(text);
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("reading message text from stdin")?;
    Ok(buf.trim_end_matches(&['\r', '\n'][..]).to_owned())
}

/// `send <channel> <text>`: posts and mirrors the echo into the local archive.
pub async fn run_send(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    text: String,
) -> Result<ExitCode> {
    let text = resolve_text_arg(text)?;
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel_arg(&workspace, channel_arg);
    let posted = match api
        .chat_post_message(&channel, &MessageContent::plain(text), None, false)
        .await
    {
        Ok(posted) => posted,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    record_echo(&workspace, &channel, posted.message.as_ref());
    output::emit(
        ctx.json,
        &serde_json::json!({"ok": true, "channel": channel.as_str(), "ts": posted.ts.as_str()}),
        &format!(
            "posted to {} as {}\n",
            label.unwrap_or_else(|| channel.to_string()),
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
) -> Result<ExitCode> {
    let text = resolve_text_arg(text)?;
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel_arg(&workspace, channel_arg);
    let thread_ts = Ts::from(thread_ts.to_owned());
    let posted = match api
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
    record_echo(&workspace, &channel, posted.message.as_ref());
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
            label.unwrap_or_else(|| channel.to_string()),
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
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel_arg(&workspace, channel_arg);
    // reactions.add takes the bare shortcode; accept ":tada:" too.
    let name = emoji.trim_matches(':').to_owned();
    let ts = Ts::from(ts.to_owned());
    match api.reactions_add(&channel, &ts, &name).await {
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
                    label.unwrap_or_else(|| channel.to_string()),
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
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel_arg(&workspace, channel_arg);
    let name = emoji.trim_matches(':').to_owned();
    let ts = Ts::from(ts.to_owned());
    match api.reactions_remove(&channel, &ts, &name).await {
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
                    label.unwrap_or_else(|| channel.to_string()),
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
) -> Result<ExitCode> {
    let text = resolve_text_arg(text)?;
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel_arg(&workspace, channel_arg);
    let ts = Ts::from(ts.to_owned());
    let updated = match api
        .chat_update(&channel, &ts, &MessageContent::plain(text))
        .await
    {
        Ok(msg) => msg,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    record_echo(&workspace, &channel, updated.message.as_ref());
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
            label.unwrap_or_else(|| channel.to_string()),
        ),
    );
    Ok(ExitCode::SUCCESS)
}

/// `delete <channel> <ts>`: deletes a message (`chat.delete`).
pub async fn run_delete(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    ts: &str,
) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel_arg(&workspace, channel_arg);
    let ts = Ts::from(ts.to_owned());
    match api.chat_delete(&channel, &ts).await {
        Ok(()) => {
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
                    label.unwrap_or_else(|| channel.to_string()),
                ),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `upload <channel> <files...> [--comment TEXT] [--thread-ts TS]`: uploads local files via v2.
pub async fn run_upload(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    paths: &[PathBuf],
    comment: Option<String>,
    thread_ts: Option<&str>,
) -> Result<ExitCode> {
    if paths.is_empty() {
        return Ok(fail(ctx, "invalid_args", "provide at least one file path"));
    }
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel_arg(&workspace, channel_arg);
    let mut uploads = Vec::with_capacity(paths.len());
    for path in paths {
        let bytes =
            std::fs::read(path).with_context(|| format!("reading file {}", path.display()))?;
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid file name: {}", path.display()))?
            .to_owned();
        uploads.push(UploadFile { filename, bytes });
    }
    let thread_ts = thread_ts.map(|t| Ts::from(t.to_owned()));
    let content = comment.map(MessageContent::plain);
    let files = match api
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
            label.unwrap_or_else(|| channel.to_string()),
            names
        ),
    );
    Ok(ExitCode::SUCCESS)
}

/// `thread <channel> <thread_ts> --live`: fetches live thread replies via `conversations.replies`,
/// mirrors them into the local SQLite archive, and prints them oldest first.
pub async fn run_thread_live(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    thread_ts_arg: &str,
    limit: u32,
) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel_arg(&workspace, channel_arg);
    let thread_ts = Ts::from(thread_ts_arg.to_owned());
    let opts = HistoryOpts {
        limit: Some(limit.clamp(1, 999)),
        ..HistoryOpts::default()
    };
    let page = match api
        .conversations_replies(&channel, &thread_ts, &opts, None)
        .await
    {
        Ok(page) => page,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };

    if let Some(path) = archive::store_path(&workspace) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut store) = Store::open(&path) {
            for msg in &page.items {
                let _ = store.upsert_message(&workspace, &channel, msg);
            }
        }
    }

    let archive = Archive::open(&workspace);
    let rows: Vec<MessageRow> = page
        .items
        .into_iter()
        .map(|message| {
            let stored = StoredMessage {
                channel: channel.clone(),
                message,
                deleted: false,
            };
            MessageRow::from_stored(&stored, archive.as_ref(), &workspace)
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

/// `mark-read <channel> [ts]`: marks `channel` read up to `ts` (or newest locally archived message).
pub async fn run_mark_read(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    ts_arg: Option<&str>,
) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel_arg(&workspace, channel_arg);
    let ts_str = match ts_arg {
        Some(ts) => ts.to_owned(),
        None => match Archive::open(&workspace).and_then(|a| a.latest_ts(&workspace, &channel)) {
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
    let ts = Ts::from(ts_str.clone());
    match api.conversations_mark(&channel, &ts).await {
        Ok(()) => {
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "channel": channel.as_str(),
                    "ts": ts.as_str(),
                }),
                &format!(
                    "marked {} read at {}\n",
                    label.unwrap_or_else(|| channel.to_string()),
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
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel_arg(&workspace, channel_arg);
    let pins = match api.pins_list(&channel).await {
        Ok(pins) => pins,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let archive = Archive::open(&workspace);
    let mut human = String::new();
    let json_items: Vec<serde_json::Value> = pins
        .items
        .iter()
        .map(|item| {
            let kind = item.kind.as_deref().unwrap_or("message");
            let created_by_name = item
                .created_by
                .as_ref()
                .and_then(|u| archive.as_ref().and_then(|a| a.user_display(&workspace, u)));
            if let Some(msg) = &item.message {
                let who = msg
                    .user
                    .as_ref()
                    .and_then(|u| archive.as_ref().and_then(|a| a.user_display(&workspace, u)))
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
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel_arg(&workspace, channel_arg);
    let ts = Ts::from(ts_arg.to_owned());
    let res = if remove {
        api.pins_remove(&channel, &ts).await
    } else {
        api.pins_add(&channel, &ts).await
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
                    label.unwrap_or_else(|| channel.to_string()),
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
    if s.len() < 2 {
        return Err(format!(
            "invalid duration '{raw}'; expected e.g. 30m, 24h, 7d"
        ));
    }
    let (num_part, unit) = s.split_at(s.len() - 1);
    let value: u64 = num_part
        .parse()
        .map_err(|_| format!("invalid duration '{raw}'; expected e.g. 30m, 24h, 7d"))?;
    let mult: u64 = match unit {
        "s" | "S" => 1,
        "m" | "M" => 60,
        "h" | "H" => 3600,
        "d" | "D" => 86_400,
        "w" | "W" => 604_800,
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
) -> Result<ExitCode> {
    let text = resolve_text_arg(text)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let post_at = match parse_schedule_at(at, now) {
        Ok(ts) => ts,
        Err(msg) => return Ok(fail(ctx, "invalid_args", &msg)),
    };
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel_arg(&workspace, channel_arg);
    let thread_ts = thread_ts.map(|t| Ts::from(t.to_owned()));
    match api
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
                    label.unwrap_or_else(|| created.channel.to_string()),
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
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let channel = channel_arg.map(|arg| resolve_channel_arg(&workspace, arg).0);
    let items = match api.chat_scheduled_messages_list(channel.as_ref()).await {
        Ok(items) => items,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let archive = Archive::open(&workspace);
    let names = archive
        .as_ref()
        .map_or_else(Default::default, |a| a.channel_names(&workspace));
    let mut human = String::new();
    let json_items: Vec<serde_json::Value> = items
        .iter()
        .map(|m| {
            let ch_label = names
                .get(m.channel_id.as_str())
                .map(|n| format!("#{n}"))
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

/// The API client + workspace for a write; `Err` is the already-printed failure.
fn api_for(ctx: &Ctx, flag: Option<&str>) -> Result<(SlackApi, TeamId), ExitCode> {
    let session = match resolve_session(flag) {
        Ok(session) => session,
        Err(not_signed_in) => return Err(fail(ctx, "not_signed_in", &not_signed_in.to_string())),
    };
    let api = SlackApi::new(
        &session.api_base(),
        session.xoxc_token.clone(),
        &session.d_cookie,
    )
    .map_err(|err| {
        fail(
            ctx,
            "client_error",
            &format!("building the Slack client: {err}"),
        )
    })?;
    Ok((api, session.team_id))
}

/// API failures become user-actionable messages; a rejected session asks for re-auth.
fn api_fail(ctx: &Ctx, err: &ApiError) -> ExitCode {
    if err.is_auth_expired() {
        return fail(
            ctx,
            "session_rejected",
            &format!(
                "slack rejected the stored session ({err}); sign in again with `slackless auth login`"
            ),
        );
    }
    fail(ctx, "request_failed", &err.to_string())
}

/// The channel id + human label for a user-supplied argument (`C…`, `#name`, `@handle`).
/// Falls back to the raw argument as an id when the archive cannot resolve it.
fn resolve_channel_arg(workspace: &TeamId, arg: &str) -> (ChannelId, Option<String>) {
    let archive = Archive::open(workspace);
    match archive
        .as_ref()
        .and_then(|archive| archive.resolve_channel(workspace, arg))
    {
        Some(row) => {
            let label = row.label();
            (row.id, Some(label))
        }
        None => (ChannelId::from(arg.strip_prefix('#').unwrap_or(arg)), None),
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
    let path = archive::store_path(workspace).context("resolving the archive path")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).context("creating the archive directory")?;
    }
    let mut store = Store::open(&path).context("opening the local archive")?;
    store
        .upsert_message(workspace, channel, message)
        .context("recording the sent message")
}
