//! `canvas` live commands (`read`, `create`, `edit`, `info`, `delete`, `share`, `unshare`,
//! `comments`, `comment`, `resolve`), delegating comment filtering, error formatting, and share
//! target helpers to `slackless_core::domain::canvas`.

use std::collections::HashSet;
use std::process::ExitCode;

use anyhow::Result;
use slackless_core::api::{
    ApiError, CanvasAccessLevel, CanvasCommentThread, CanvasEditOp, CanvasMeta, MessageContent,
    canvas_comment_channel_id, parse_canvas_meta_from_file,
};
use slackless_core::domain::canvas::{
    self as domain_canvas, CanvasCommentFilter, filter_comment_threads, format_canvas_api_error,
};
use slackless_core::types::{ChannelId, Ts, UserId};

use crate::archive::Archive;
use crate::output;
use crate::write::{check_secrets, resolve_text_arg};
use crate::{CanvasAccessArg, Ctx, fail};

use super::session::{api_for, classify_api_error, resolve_channel, resolve_channel_with};

impl From<CanvasAccessArg> for CanvasAccessLevel {
    fn from(arg: CanvasAccessArg) -> Self {
        match arg {
            CanvasAccessArg::Read => Self::Read,
            CanvasAccessArg::Write => Self::Write,
            CanvasAccessArg::Owner => Self::Owner,
        }
    }
}

/// Map a canvas [`ApiError`] to its CLI error code and human-friendly message,
/// translating known Slack Canvas error codes via `slackless_core::domain::canvas`.
pub(crate) fn classify_canvas_api_error(err: &ApiError) -> (&'static str, String) {
    if err.is_auth_expired() {
        classify_api_error(err)
    } else {
        ("request_failed", format_canvas_api_error(err))
    }
}

fn canvas_api_fail(ctx: &Ctx, err: &ApiError) -> ExitCode {
    let (code, message) = classify_canvas_api_error(err);
    fail(ctx, code, &message)
}

/// Returns whether a failed `canvases.getContent` call may fall back to `fetch_canvas_html`.
/// Never swallows authentication expiry, network/HTTP errors, rate limits, or untrusted URLs.
pub(crate) fn should_fallback_canvas_html(err: &ApiError) -> bool {
    matches!(err, ApiError::Slack(_)) && !err.is_auth_expired()
}

pub(crate) fn format_canvas_read(
    file_id: &str,
    format: &str,
    title: &str,
    content: &str,
) -> (serde_json::Value, String) {
    let human = if content.ends_with('\n') {
        content.to_owned()
    } else {
        format!("{content}\n")
    };
    let json = serde_json::json!({
        "file_id": file_id,
        "title": title,
        "format": format,
        "content": content,
    });
    (json, human)
}

/// `canvas read <file_id> [--html]`
pub async fn run_canvas_read(
    ctx: &Ctx,
    flag: Option<&str>,
    file_id: &str,
    html: bool,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (format, title, content) = if html {
        match session.api.fetch_canvas_html(file_id).await {
            Ok((t, h)) => ("html", t, h),
            Err(err) => return Ok(canvas_api_fail(ctx, &err)),
        }
    } else {
        match session
            .api
            .canvases_get_content(file_id, Some("markdown"))
            .await
        {
            Ok(md) => {
                let title = match session.api.files_info(file_id).await {
                    Ok(file) => {
                        let meta = parse_canvas_meta_from_file(&file, None);
                        if meta.title.trim().is_empty() {
                            file_id.to_owned()
                        } else {
                            meta.title
                        }
                    }
                    Err(err) if err.is_auth_expired() => return Ok(canvas_api_fail(ctx, &err)),
                    Err(_) => file_id.to_owned(),
                };
                ("markdown", title, md)
            }
            Err(err) if should_fallback_canvas_html(&err) => {
                match session.api.fetch_canvas_html(file_id).await {
                    Ok((t, h)) => ("html", t, h),
                    Err(fallback_err) => return Ok(canvas_api_fail(ctx, &fallback_err)),
                }
            }
            Err(err) => return Ok(canvas_api_fail(ctx, &err)),
        }
    };
    let (json, human) = format_canvas_read(file_id, format, &title, &content);
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

/// `canvas create <title> [--markdown <md|->] [--channel <ch>]`
pub async fn run_canvas_create(
    ctx: &Ctx,
    flag: Option<&str>,
    title: &str,
    markdown: Option<String>,
    channel_arg: Option<&str>,
) -> Result<ExitCode> {
    let markdown = markdown.map(resolve_text_arg).transpose()?;
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let channel = channel_arg.map(|ch| resolve_channel(&session.workspace, ch).0);
    match session
        .api
        .canvases_create(Some(title), markdown.as_deref(), channel.as_ref())
        .await
    {
        Ok(created) => {
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "canvas_id": created.canvas_id,
                    "title": title,
                }),
                &format!("created canvas {} ({title})\n", created.canvas_id),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(canvas_api_fail(ctx, &err)),
    }
}

pub(crate) fn build_canvas_edit_op(
    markdown: String,
    section: Option<String>,
    append: bool,
) -> CanvasEditOp {
    if append {
        CanvasEditOp::InsertAtEnd { markdown }
    } else {
        CanvasEditOp::Replace {
            section_id: section,
            markdown,
        }
    }
}

/// `canvas edit <file_id> <markdown|-> [--section <section_id>] [--append]`
pub async fn run_canvas_edit(
    ctx: &Ctx,
    flag: Option<&str>,
    file_id: &str,
    markdown: String,
    section: Option<String>,
    append: bool,
) -> Result<ExitCode> {
    let markdown = resolve_text_arg(markdown)?;
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let op = build_canvas_edit_op(markdown, section, append);
    match session.api.canvases_edit(file_id, &op).await {
        Ok(()) => {
            output::emit(
                ctx.json,
                &serde_json::json!({ "ok": true, "canvas_id": file_id }),
                &format!("updated canvas {file_id}\n"),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(canvas_api_fail(ctx, &err)),
    }
}

/// Validated target for `canvas share` / `canvas unshare` (strictly XOR: channels or users).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CanvasShareTarget {
    Channels(Vec<ChannelId>),
    Users(Vec<UserId>),
}

/// Lightweight validation of share/unshare CLI arguments before touching credentials or SQLite.
fn validate_share_inputs(
    channels: &[String],
    users: &[String],
    post: bool,
) -> std::result::Result<(), String> {
    if !channels.is_empty() && !users.is_empty() {
        return Err("specify either --channel or --user, not both".to_owned());
    }
    if channels.is_empty() && users.is_empty() {
        return Err("specify at least one --channel or --user".to_owned());
    }
    if post && !users.is_empty() {
        return Err("--post can only be used with --channel".to_owned());
    }
    if channels.iter().any(|c| c.trim().is_empty()) {
        return Err("--channel value cannot be empty".to_owned());
    }
    if users.iter().any(|u| u.trim().is_empty()) {
        return Err("--user value cannot be empty".to_owned());
    }
    Ok(())
}

/// Validate XOR between `--channel` and `--user`, reject blank targets, resolve channels,
/// and deduplicate preserving input order. Delegates comma/multi-ID parsing to
/// `slackless_core::domain::canvas::parse_share_targets` while still supporting `#name` / `@handle`
/// channel resolution.
pub(crate) fn parse_canvas_share_target(
    channels: &[String],
    users: &[String],
    mut resolve_ch: impl FnMut(&str) -> ChannelId,
) -> std::result::Result<CanvasShareTarget, String> {
    validate_share_inputs(channels, users, false)?;
    if !channels.is_empty() {
        let mut seen = HashSet::new();
        let mut resolved = Vec::with_capacity(channels.len());
        for raw in channels {
            let trimmed = raw.trim();
            if trimmed.contains([',', ';']) {
                let (parsed_chs, _) = domain_canvas::parse_share_targets(trimmed);
                if !parsed_chs.is_empty() {
                    for id in parsed_chs {
                        if seen.insert(id.clone()) {
                            resolved.push(id);
                        }
                    }
                    continue;
                }
            }
            let id = resolve_ch(trimmed);
            if seen.insert(id.clone()) {
                resolved.push(id);
            }
        }
        Ok(CanvasShareTarget::Channels(resolved))
    } else {
        let mut seen = HashSet::new();
        let mut resolved = Vec::with_capacity(users.len());
        for raw in users {
            let trimmed = raw.trim();
            let (_, parsed_users) = domain_canvas::parse_share_targets(trimmed);
            if !parsed_users.is_empty() {
                for id in parsed_users {
                    if seen.insert(id.clone()) {
                        resolved.push(id);
                    }
                }
            } else {
                let id = UserId::from(trimmed.trim_start_matches('@').to_owned());
                if seen.insert(id.clone()) {
                    resolved.push(id);
                }
            }
        }
        Ok(CanvasShareTarget::Users(resolved))
    }
}

/// Validate that `--post` is only requested when sharing with channels.
#[cfg(test)]
pub(crate) fn validate_canvas_share_post(
    target: &CanvasShareTarget,
    post: bool,
) -> std::result::Result<(), String> {
    if post && matches!(target, CanvasShareTarget::Users(_)) {
        return Err("--post can only be used with --channel".to_owned());
    }
    Ok(())
}

pub(crate) fn format_canvas_delete_result(canvas_id: &str) -> (serde_json::Value, String) {
    let json = serde_json::json!({
        "ok": true,
        "canvas_id": canvas_id,
        "deleted": true,
    });
    let human = format!("deleted canvas {canvas_id}\n");
    (json, human)
}

/// `canvas delete <canvas_id>`
pub async fn run_canvas_delete(ctx: &Ctx, flag: Option<&str>, canvas_id: &str) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    match session.api.canvases_delete(canvas_id).await {
        Ok(()) => {
            let (json, human) = format_canvas_delete_result(canvas_id);
            output::emit(ctx.json, &json, &human);
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(canvas_api_fail(ctx, &err)),
    }
}

pub(crate) fn format_canvas_share_result(
    canvas_id: &str,
    access: CanvasAccessLevel,
    target: &CanvasShareTarget,
    posted_to: &[ChannelId],
    permalink: Option<&str>,
) -> (serde_json::Value, String) {
    let (channels, users, target_summary): (Vec<&str>, Vec<&str>, String) = match target {
        CanvasShareTarget::Channels(chs) => {
            let ids: Vec<&str> = chs.iter().map(ChannelId::as_str).collect();
            let joined = ids.join(", ");
            (ids, Vec::new(), format!("channels: {joined}"))
        }
        CanvasShareTarget::Users(us) => {
            let ids: Vec<&str> = us.iter().map(UserId::as_str).collect();
            let joined = ids.join(", ");
            (Vec::new(), ids, format!("users: {joined}"))
        }
    };
    let posted_ids: Vec<&str> = posted_to.iter().map(ChannelId::as_str).collect();
    let posted = !posted_ids.is_empty();
    let json = serde_json::json!({
        "ok": true,
        "canvas_id": canvas_id,
        "access": access.as_str(),
        "channels": channels,
        "users": users,
        "posted": posted,
        "posted_channels": posted_ids,
        "permalink": permalink,
    });
    let post_suffix = if posted {
        format!(" (posted permalink to {} channel(s))", posted_to.len())
    } else {
        String::new()
    };
    let human = format!(
        "shared canvas {canvas_id} ({}) with {target_summary}{post_suffix}\n",
        access.as_str()
    );
    (json, human)
}

/// `canvas share <canvas_id> --access <read|write|owner> [--channel <ch>...] [--user <u_id>...] [--post]`
pub async fn run_canvas_share(
    ctx: &Ctx,
    flag: Option<&str>,
    canvas_id: &str,
    access: CanvasAccessArg,
    channels: &[String],
    users: &[String],
    post: bool,
) -> Result<ExitCode> {
    if let Err(msg) = validate_share_inputs(channels, users, post) {
        return Ok(fail(ctx, "invalid_args", &msg));
    }

    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let archive = if channels.is_empty() {
        None
    } else {
        Archive::open(&session.workspace).ok().flatten()
    };
    let target = match parse_canvas_share_target(channels, users, |ch| {
        resolve_channel_with(archive.as_ref(), &session.workspace, ch).0
    }) {
        Ok(target) => target,
        Err(msg) => return Ok(fail(ctx, "invalid_args", &msg)),
    };

    let access_level = CanvasAccessLevel::from(access);
    let (channel_ids, user_ids): (&[ChannelId], &[UserId]) = match &target {
        CanvasShareTarget::Channels(chs) => (chs.as_slice(), &[]),
        CanvasShareTarget::Users(us) => (&[], us.as_slice()),
    };

    if let Err(err) = session
        .api
        .canvases_access_set(canvas_id, access_level, channel_ids, user_ids)
        .await
    {
        return Ok(canvas_api_fail(ctx, &err));
    }

    let mut posted_to = Vec::new();
    let mut permalink: Option<String> = None;
    if post && !channel_ids.is_empty() {
        let file = match session.api.files_info(canvas_id).await {
            Ok(f) => f,
            Err(err) => return Ok(canvas_api_fail(ctx, &err)),
        };
        let meta = parse_canvas_meta_from_file(&file, None);
        let Some(url) = meta.permalink.filter(|u| !u.trim().is_empty()) else {
            return Ok(fail(
                ctx,
                "request_failed",
                "canvas access granted, but no canvas permalink is available to post",
            ));
        };
        for ch in channel_ids {
            if let Err(err) = session
                .api
                .chat_post_message(ch, &MessageContent::plain(url.clone()), None, false)
                .await
            {
                return Ok(canvas_api_fail(ctx, &err));
            }
            posted_to.push(ch.clone());
        }
        permalink = Some(url);
    }

    let (json, human) = format_canvas_share_result(
        canvas_id,
        access_level,
        &target,
        &posted_to,
        permalink.as_deref(),
    );
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn format_canvas_unshare_result(
    canvas_id: &str,
    target: &CanvasShareTarget,
) -> (serde_json::Value, String) {
    let (channels, users, target_summary): (Vec<&str>, Vec<&str>, String) = match target {
        CanvasShareTarget::Channels(chs) => {
            let ids: Vec<&str> = chs.iter().map(ChannelId::as_str).collect();
            let joined = ids.join(", ");
            (ids, Vec::new(), format!("channels: {joined}"))
        }
        CanvasShareTarget::Users(us) => {
            let ids: Vec<&str> = us.iter().map(UserId::as_str).collect();
            let joined = ids.join(", ");
            (Vec::new(), ids, format!("users: {joined}"))
        }
    };
    let json = serde_json::json!({
        "ok": true,
        "canvas_id": canvas_id,
        "channels": channels,
        "users": users,
    });
    let human = format!("unshared canvas {canvas_id} from {target_summary}\n");
    (json, human)
}

/// `canvas unshare <canvas_id> [--channel <ch>...] [--user <u_id>...]`
pub async fn run_canvas_unshare(
    ctx: &Ctx,
    flag: Option<&str>,
    canvas_id: &str,
    channels: &[String],
    users: &[String],
) -> Result<ExitCode> {
    if let Err(msg) = validate_share_inputs(channels, users, false) {
        return Ok(fail(ctx, "invalid_args", &msg));
    }

    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let archive = if channels.is_empty() {
        None
    } else {
        Archive::open(&session.workspace).ok().flatten()
    };
    let target = match parse_canvas_share_target(channels, users, |ch| {
        resolve_channel_with(archive.as_ref(), &session.workspace, ch).0
    }) {
        Ok(target) => target,
        Err(msg) => return Ok(fail(ctx, "invalid_args", &msg)),
    };

    let (channel_ids, user_ids): (&[ChannelId], &[UserId]) = match &target {
        CanvasShareTarget::Channels(chs) => (chs.as_slice(), &[]),
        CanvasShareTarget::Users(us) => (&[], us.as_slice()),
    };

    match session
        .api
        .canvases_access_delete(canvas_id, channel_ids, user_ids)
        .await
    {
        Ok(()) => {
            let (json, human) = format_canvas_unshare_result(canvas_id, &target);
            output::emit(ctx.json, &json, &human);
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(canvas_api_fail(ctx, &err)),
    }
}

pub(crate) fn format_canvas_meta(meta: &CanvasMeta) -> (serde_json::Value, String) {
    let editors: Vec<&str> = meta.editors.iter().map(UserId::as_str).collect();
    let json = serde_json::json!({
        "canvas_id": meta.file_id,
        "title": meta.title,
        "permalink": meta.permalink,
        "access": meta.access,
        "editable": meta.editable,
        "org_or_workspace_access": meta.org_or_workspace_access,
        "canvas_printing_enabled": meta.canvas_printing_enabled,
        "creator": meta.canvas_creator_id.as_ref().map(UserId::as_str),
        "editors": editors,
        "quip_thread_id": meta.quip_thread_id,
        "comment_channel_id": meta.comment_channel_id.as_str(),
    });

    let editors_display = if editors.is_empty() {
        "-".to_owned()
    } else {
        editors.join(", ")
    };
    let mut human = String::new();
    human.push_str(&format!("Canvas:\t{}\n", meta.file_id));
    human.push_str(&format!("Title:\t{}\n", meta.title));
    human.push_str(&format!(
        "Permalink:\t{}\n",
        meta.permalink.as_deref().unwrap_or("-")
    ));
    human.push_str(&format!(
        "Access:\t{}\n",
        meta.access.as_deref().unwrap_or("-")
    ));
    human.push_str(&format!("Editable:\t{}\n", meta.editable));
    human.push_str(&format!(
        "Workspace access:\t{}\n",
        meta.org_or_workspace_access.as_deref().unwrap_or("-")
    ));
    human.push_str(&format!(
        "Printing enabled:\t{}\n",
        meta.canvas_printing_enabled
    ));
    human.push_str(&format!(
        "Creator:\t{}\n",
        meta.canvas_creator_id
            .as_ref()
            .map(UserId::as_str)
            .unwrap_or("-")
    ));
    human.push_str(&format!("Editors:\t{editors_display}\n"));
    human.push_str(&format!("Comment channel:\t{}\n", meta.comment_channel_id));
    if let Some(quip_id) = meta.quip_thread_id.as_deref() {
        human.push_str(&format!("Quip thread:\t{quip_id}\n"));
    }

    (json, human)
}

/// `canvas info <canvas_id>`
pub async fn run_canvas_info(ctx: &Ctx, flag: Option<&str>, canvas_id: &str) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let file = match session.api.files_info(canvas_id).await {
        Ok(f) => f,
        Err(err) => return Ok(canvas_api_fail(ctx, &err)),
    };
    let mut meta = parse_canvas_meta_from_file(&file, None);
    if meta.file_id.is_empty() {
        meta.file_id = canvas_id.to_owned();
        meta.comment_channel_id = canvas_comment_channel_id(canvas_id);
    }
    let (json, human) = format_canvas_meta(&meta);
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn comment_filter_from_flags(resolved: bool, all: bool) -> CanvasCommentFilter {
    if all {
        CanvasCommentFilter::All
    } else if resolved {
        CanvasCommentFilter::Resolved
    } else {
        CanvasCommentFilter::Open
    }
}

pub(crate) const fn comment_filter_as_str(filter: CanvasCommentFilter) -> &'static str {
    match filter {
        CanvasCommentFilter::Open => "open",
        CanvasCommentFilter::Resolved => "resolved",
        CanvasCommentFilter::All => "all",
    }
}

pub(crate) fn filter_canvas_comments(
    threads: &[CanvasCommentThread],
    filter: CanvasCommentFilter,
) -> Vec<&CanvasCommentThread> {
    filter_comment_threads(threads, filter)
}

pub(crate) fn format_canvas_comments(
    canvas_id: &str,
    filter: CanvasCommentFilter,
    threads: &[&CanvasCommentThread],
) -> (serde_json::Value, String) {
    let comment_channel = canvas_comment_channel_id(canvas_id);
    let json_threads: Vec<serde_json::Value> = threads
        .iter()
        .map(|t| {
            let replies: Vec<serde_json::Value> = t
                .replies
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "ts": r.ts.as_str(),
                        "author": r.author.as_ref().map(UserId::as_str),
                        "text": r.text,
                    })
                })
                .collect();
            serde_json::json!({
                "root_ts": t.root_ts.as_str(),
                "author": t.author.as_ref().map(UserId::as_str),
                "text": t.text,
                "resolved": t.resolved,
                "reply_count": t.reply_count,
                "replies": replies,
            })
        })
        .collect();

    let mut human = String::new();
    for t in threads {
        let status = if t.resolved { "resolved" } else { "open" };
        let author = t.author.as_ref().map(UserId::as_str).unwrap_or("unknown");
        let text = t.text.replace(['\t', '\r', '\n'], " ");
        human.push_str(&format!(
            "{}\t{status}\t{author}\treplies={}\t{text}\n",
            t.root_ts, t.reply_count
        ));
        for r in &t.replies {
            let reply_author = r.author.as_ref().map(UserId::as_str).unwrap_or("unknown");
            let reply_text = r.text.replace(['\t', '\r', '\n'], " ");
            human.push_str(&format!(
                "  {}\treply\t{reply_author}\t{reply_text}\n",
                r.ts
            ));
        }
    }

    let json = serde_json::json!({
        "canvas_id": canvas_id,
        "comment_channel_id": comment_channel.as_str(),
        "filter": comment_filter_as_str(filter),
        "count": threads.len(),
        "threads": json_threads,
    });
    (json, human)
}

/// `canvas comments <canvas_id> [--resolved|--all]`
pub async fn run_canvas_comments(
    ctx: &Ctx,
    flag: Option<&str>,
    canvas_id: &str,
    resolved: bool,
    all: bool,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let all_threads = match session.api.canvas_comments_list(canvas_id).await {
        Ok(threads) => threads,
        Err(err) => return Ok(canvas_api_fail(ctx, &err)),
    };
    let filter = comment_filter_from_flags(resolved, all);
    let filtered = filter_canvas_comments(&all_threads, filter);
    let (json, human) = format_canvas_comments(canvas_id, filter, &filtered);
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn validate_canvas_comment_args(
    text: &str,
    thread: Option<&str>,
) -> std::result::Result<Option<Ts>, String> {
    if text.trim().is_empty() {
        return Err("comment text cannot be empty".to_owned());
    }
    match thread {
        None => Ok(None),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Err("--thread value cannot be empty".to_owned())
            } else {
                Ok(Some(Ts::from(trimmed.to_owned())))
            }
        }
    }
}

pub(crate) fn format_canvas_comment_posted(
    canvas_id: &str,
    comment_channel: &ChannelId,
    thread_ts: Option<&Ts>,
    posted_ts: &Ts,
) -> (serde_json::Value, String) {
    let json = serde_json::json!({
        "ok": true,
        "canvas_id": canvas_id,
        "comment_channel_id": comment_channel.as_str(),
        "thread_ts": thread_ts.map(Ts::as_str),
        "ts": posted_ts.as_str(),
    });
    let human = match thread_ts {
        Some(root) => format!("replied to comment {root} on canvas {canvas_id} as {posted_ts}\n"),
        None => format!("posted comment on canvas {canvas_id} as {posted_ts}\n"),
    };
    (json, human)
}

/// `canvas comment <canvas_id> <text> [--thread <ts>] [--allow-secrets]`
pub async fn run_canvas_comment(
    ctx: &Ctx,
    flag: Option<&str>,
    canvas_id: &str,
    text: String,
    thread: Option<String>,
    allow_secrets: bool,
) -> Result<ExitCode> {
    let text = resolve_text_arg(text)?;
    let thread_ts = match validate_canvas_comment_args(&text, thread.as_deref()) {
        Ok(ts) => ts,
        Err(msg) => return Ok(fail(ctx, "invalid_args", &msg)),
    };
    if let Some(exit) = check_secrets(ctx, &text, allow_secrets) {
        return Ok(exit);
    }

    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let comment_channel = canvas_comment_channel_id(canvas_id);
    match session
        .api
        .canvas_comment_post(canvas_id, &text, thread_ts.as_ref())
        .await
    {
        Ok(posted) => {
            let (json, human) = format_canvas_comment_posted(
                canvas_id,
                &comment_channel,
                thread_ts.as_ref(),
                &posted.ts,
            );
            output::emit(ctx.json, &json, &human);
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(canvas_api_fail(ctx, &err)),
    }
}

/// Validated action for `canvas resolve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CanvasResolveAction {
    ResolveOne(Ts),
    ReopenOne(Ts),
    ResolveAll,
}

pub(crate) fn parse_canvas_resolve_action(
    thread: Option<&str>,
    all: bool,
    reopen: bool,
) -> std::result::Result<CanvasResolveAction, String> {
    match (thread, all, reopen) {
        (Some(_), true, _) => Err("specify either --thread or --all, not both".to_owned()),
        (None, false, _) => Err("specify either --thread <TS> or --all".to_owned()),
        (None, true, true) => {
            Err("--reopen cannot be used with --all; specify --thread <TS>".to_owned())
        }
        (None, true, false) => Ok(CanvasResolveAction::ResolveAll),
        (Some(raw), false, is_reopen) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err("--thread value cannot be empty".to_owned());
            }
            let ts = Ts::from(trimmed.to_owned());
            if is_reopen {
                Ok(CanvasResolveAction::ReopenOne(ts))
            } else {
                Ok(CanvasResolveAction::ResolveOne(ts))
            }
        }
    }
}

pub(crate) fn format_canvas_resolve_result(
    canvas_id: &str,
    comment_channel: &ChannelId,
    action: &CanvasResolveAction,
) -> (serde_json::Value, String) {
    match action {
        CanvasResolveAction::ResolveOne(ts) => (
            serde_json::json!({
                "ok": true,
                "canvas_id": canvas_id,
                "comment_channel_id": comment_channel.as_str(),
                "thread_ts": ts.as_str(),
                "all": false,
                "resolved": true,
            }),
            format!("resolved comment thread {ts} on canvas {canvas_id}\n"),
        ),
        CanvasResolveAction::ReopenOne(ts) => (
            serde_json::json!({
                "ok": true,
                "canvas_id": canvas_id,
                "comment_channel_id": comment_channel.as_str(),
                "thread_ts": ts.as_str(),
                "all": false,
                "resolved": false,
            }),
            format!("reopened comment thread {ts} on canvas {canvas_id}\n"),
        ),
        CanvasResolveAction::ResolveAll => (
            serde_json::json!({
                "ok": true,
                "canvas_id": canvas_id,
                "comment_channel_id": comment_channel.as_str(),
                "thread_ts": serde_json::Value::Null,
                "all": true,
                "resolved": true,
            }),
            format!("resolved all comment threads on canvas {canvas_id}\n"),
        ),
    }
}

/// `canvas resolve <canvas_id> [--thread <ts> | --all] [--reopen]`
pub async fn run_canvas_resolve(
    ctx: &Ctx,
    flag: Option<&str>,
    canvas_id: &str,
    thread: Option<String>,
    all: bool,
    reopen: bool,
) -> Result<ExitCode> {
    let action = match parse_canvas_resolve_action(thread.as_deref(), all, reopen) {
        Ok(action) => action,
        Err(msg) => return Ok(fail(ctx, "invalid_args", &msg)),
    };

    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let comment_channel = canvas_comment_channel_id(canvas_id);
    let res = match &action {
        CanvasResolveAction::ResolveOne(ts) => {
            session.api.quip_thread_archive(&comment_channel, ts).await
        }
        CanvasResolveAction::ReopenOne(ts) => {
            session
                .api
                .quip_thread_unarchive(&comment_channel, ts)
                .await
        }
        CanvasResolveAction::ResolveAll => {
            session.api.quip_thread_archive_all(&comment_channel).await
        }
    };
    match res {
        Ok(()) => {
            let (json, human) = format_canvas_resolve_result(canvas_id, &comment_channel, &action);
            output::emit(ctx.json, &json, &human);
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(canvas_api_fail(ctx, &err)),
    }
}
