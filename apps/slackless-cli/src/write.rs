//! Live write commands (`send`, `reply`, `react`).
//!
//! They go through `slackless-core`'s REST client with a stored session — one deterministic
//! request per invocation, no websocket session to keep alive. Without a stored session they
//! print a clear "not signed in" message instead of failing hard.

use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::api::{ApiError, MessageContent, SlackApi};
use slackless_core::store::Store;
use slackless_core::types::{ChannelId, Message, TeamId, Ts};

use crate::archive::{self, Archive};
use crate::output;
use crate::session::resolve_session;
use crate::{Ctx, fail};

/// `send <channel> <text>`: posts and mirrors the echo into the local archive.
pub async fn run_send(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    text: String,
) -> Result<ExitCode> {
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
