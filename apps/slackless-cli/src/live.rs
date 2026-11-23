//! Live REST & realtime commands added for GUI parity:
//! - `mark-thread` (`subscriptions.thread.mark`)
//! - `saved` (`saved.list` / `saved.add` / `saved.update` / `saved.delete`)
//! - `files` (`files.list`)
//! - `canvas` (`canvases.getContent` / `fetch_canvas_html` / `canvases.create` / `canvases.edit`)
//! - `list` (`fetch_list` / `lists.create` / `lists.records.create` / `lists.records.delete` / `lists.addToList`)
//! - `watch` (live RTM event stream over `wss-primary`)

use std::process::ExitCode;

use anyhow::Result;
use slackless_core::api::{
    ApiError, CanvasEditOp, FilesFilter, ListCellInput, ListCellInputValue, SavedMark, SlackApi,
    SlackListCellValue, SlackListData,
};
use slackless_core::rtm::{ConnectParams, ConnectionState, RtmClient, RtmConfig, RtmEvent};
use slackless_core::types::{ChannelId, SlackEvent, TeamId, Ts};

use crate::archive::Archive;
use crate::output;
use crate::session::resolve_session;
use crate::{Ctx, ListReadFormat, fail};

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

fn resolve_channel(workspace: &TeamId, arg: &str) -> (ChannelId, Option<String>) {
    let archive = Archive::open(workspace);
    match archive
        .as_ref()
        .and_then(|a| a.resolve_channel(workspace, arg))
    {
        Some(row) => {
            let label = row.label();
            (row.id, Some(label))
        }
        None => (ChannelId::from(arg.strip_prefix('#').unwrap_or(arg)), None),
    }
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
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&workspace, channel_arg);
    let root = Ts::from(thread_ts.to_owned());
    // Moving the read cursor before the latest reply marks newer replies unread.
    let cursor = if unread && ts.is_none() {
        Ts::from("0".to_owned())
    } else {
        Ts::from(ts.unwrap_or(thread_ts).to_owned())
    };
    let read = !unread;
    match api
        .subscriptions_thread_mark(&channel, &root, &cursor)
        .await
    {
        Ok(()) => {
            let state = if read { "read" } else { "unread" };
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "channel": channel.as_str(),
                    "thread_ts": root.as_str(),
                    "ts": cursor.as_str(),
                    "read": read,
                }),
                &format!(
                    "marked thread {} in {} as {state}\n",
                    root,
                    label.unwrap_or_else(|| channel.to_string())
                ),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `saved list [--filter in_progress|completed|archived] [--limit N]`
pub async fn run_saved_list(
    ctx: &Ctx,
    flag: Option<&str>,
    filter: &str,
    limit: u32,
) -> Result<ExitCode> {
    let (api, _) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let items = match api.saved_list(filter, limit).await {
        Ok(items) => items,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
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
    for it in &items {
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
    output::emit(
        ctx.json,
        &serde_json::json!({ "filter": filter, "count": items.len(), "items": json_items }),
        &human,
    );
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
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&workspace, channel_arg);
    let ts = Ts::from(ts.to_owned());
    match api.saved_add(&channel, &ts, due).await {
        Ok(()) => {
            output::emit(
                ctx.json,
                &serde_json::json!({ "ok": true, "channel": channel.as_str(), "ts": ts.as_str(), "date_due": due }),
                &format!(
                    "saved {} in {}\n",
                    ts,
                    label.unwrap_or_else(|| channel.to_string())
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
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, _) = resolve_channel(&workspace, channel_arg);
    let ts = Ts::from(ts.to_owned());
    match api
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
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, _) = resolve_channel(&workspace, channel_arg);
    let ts = Ts::from(ts.to_owned());
    match api.saved_delete(&channel, &ts).await {
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

/// `files [--channel <ch>] [--types <csv>] [--limit N]`
pub async fn run_files(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: Option<&str>,
    types: Option<String>,
    limit: u32,
) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let channel = channel_arg.map(|ch| resolve_channel(&workspace, ch).0);
    let filter = FilesFilter {
        channel,
        types,
        ..FilesFilter::workspace()
    };
    let page = match api.files_list(&filter, limit.clamp(1, 200), 1).await {
        Ok(page) => page,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let files = page.items;
    let json_files: Vec<serde_json::Value> = files
        .iter()
        .map(|f| {
            serde_json::json!({
                "id": f.id.as_str(),
                "name": f.name,
                "title": f.title,
                "filetype": f.filetype,
                "size": f.size,
                "user": f.user.as_ref().map(|u| u.as_str()),
                "permalink": f.permalink,
            })
        })
        .collect();
    let mut human = String::new();
    for f in &files {
        let name = f
            .title
            .as_deref()
            .or(f.name.as_deref())
            .unwrap_or(f.id.as_str());
        let kind = f.filetype.as_deref().unwrap_or("file");
        let size = f.size.map_or_else(|| "-".to_owned(), |s| s.to_string());
        human.push_str(&format!("{}\t{kind}\t{size}\t{name}\n", f.id));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({ "count": files.len(), "files": json_files }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

/// `canvas read <file_id> [--html]`
pub async fn run_canvas_read(
    ctx: &Ctx,
    flag: Option<&str>,
    file_id: &str,
    html: bool,
) -> Result<ExitCode> {
    let (api, _) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (format, title, content) = if html {
        match api.fetch_canvas_html(file_id).await {
            Ok((t, h)) => ("html", t, h),
            Err(err) => return Ok(api_fail(ctx, &err)),
        }
    } else {
        match api.canvases_get_content(file_id, Some("markdown")).await {
            Ok(md) => ("markdown", file_id.to_owned(), md),
            Err(_) => match api.fetch_canvas_html(file_id).await {
                Ok((t, h)) => ("html", t, h),
                Err(err) => return Ok(api_fail(ctx, &err)),
            },
        }
    };
    let human = if content.ends_with('\n') {
        content.clone()
    } else {
        format!("{content}\n")
    };
    output::emit(
        ctx.json,
        &serde_json::json!({
            "file_id": file_id,
            "title": title,
            "format": format,
            "content": content,
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

/// `canvas create <title> [--markdown <md>] [--channel <ch>]`
pub async fn run_canvas_create(
    ctx: &Ctx,
    flag: Option<&str>,
    title: &str,
    markdown: Option<&str>,
    channel_arg: Option<&str>,
) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let channel = channel_arg.map(|ch| resolve_channel(&workspace, ch).0);
    match api
        .canvases_create(Some(title), markdown, channel.as_ref())
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
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `canvas edit <file_id> <markdown> [--section <section_id>] [--append]`
pub async fn run_canvas_edit(
    ctx: &Ctx,
    flag: Option<&str>,
    file_id: &str,
    markdown: String,
    section: Option<String>,
    append: bool,
) -> Result<ExitCode> {
    let (api, _) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let op = if append {
        CanvasEditOp::InsertAtEnd { markdown }
    } else {
        CanvasEditOp::Replace {
            section_id: section,
            markdown,
        }
    };
    match api.canvases_edit(file_id, &op).await {
        Ok(()) => {
            output::emit(
                ctx.json,
                &serde_json::json!({ "ok": true, "canvas_id": file_id }),
                &format!("updated canvas {file_id}\n"),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `list read <list_id> [--format table|csv|json]`
pub async fn run_list_read(
    ctx: &Ctx,
    flag: Option<&str>,
    list_id: &str,
    format: ListReadFormat,
) -> Result<ExitCode> {
    let (api, _) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let data = match api.fetch_list(list_id).await {
        Ok(data) => data,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let effective = if ctx.json {
        ListReadFormat::Json
    } else {
        format
    };
    match effective {
        ListReadFormat::Json => {
            let val = list_to_json_value(&data);
            output::emit_json(&val);
        }
        ListReadFormat::Csv => print!("{}", list_to_csv(&data)),
        ListReadFormat::Table => print!("{}", list_to_tsv(&data)),
    }
    Ok(ExitCode::SUCCESS)
}

/// `list create <name> [--description <desc>] [--channel <ch>]`
pub async fn run_list_create(
    ctx: &Ctx,
    flag: Option<&str>,
    name: &str,
    description: Option<&str>,
    channel_arg: Option<&str>,
) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let channel = channel_arg.map(|ch| resolve_channel(&workspace, ch).0);
    match api.lists_create(name, description, channel.as_ref()).await {
        Ok(file_id) => {
            output::emit(
                ctx.json,
                &serde_json::json!({ "ok": true, "list_id": file_id, "name": name }),
                &format!("created list {file_id} ({name})\n"),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `list add-item <list_id> <title> [--column <col_id>]`
pub async fn run_list_add_item(
    ctx: &Ctx,
    flag: Option<&str>,
    list_id: &str,
    title: String,
    column: Option<String>,
) -> Result<ExitCode> {
    let (api, _) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let col_id = match column {
        Some(c) => c,
        None => match api.fetch_list(list_id).await {
            Ok(data) => data
                .columns
                .iter()
                .find(|c| c.is_primary)
                .or_else(|| data.columns.first())
                .map(|c| c.id.clone())
                .unwrap_or_else(|| "name".to_owned()),
            Err(err) => return Ok(api_fail(ctx, &err)),
        },
    };
    let fields = vec![ListCellInput {
        column_id: col_id,
        value: ListCellInputValue::Text(title.clone()),
    }];
    match api.lists_records_create(list_id, &fields, None).await {
        Ok(row) => {
            output::emit(
                ctx.json,
                &serde_json::json!({ "ok": true, "list_id": list_id, "row_id": row.id, "title": title }),
                &format!("added item {} to {list_id}\n", row.id),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `list delete-item <list_id> <row_id>`
pub async fn run_list_delete_item(
    ctx: &Ctx,
    flag: Option<&str>,
    list_id: &str,
    row_id: &str,
) -> Result<ExitCode> {
    let (api, _) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    match api.lists_records_delete(list_id, row_id).await {
        Ok(()) => {
            output::emit(
                ctx.json,
                &serde_json::json!({ "ok": true, "list_id": list_id, "row_id": row_id }),
                &format!("deleted item {row_id} from {list_id}\n"),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `list add-message <list_id> <channel> <ts>`
pub async fn run_list_add_message(
    ctx: &Ctx,
    flag: Option<&str>,
    list_id: &str,
    channel_arg: &str,
    ts: &str,
) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, _) = resolve_channel(&workspace, channel_arg);
    let ts = Ts::from(ts.to_owned());
    match api.lists_add_to_list(list_id, &channel, &ts).await {
        Ok(record_id) => {
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "list_id": list_id,
                    "channel": channel.as_str(),
                    "ts": ts.as_str(),
                    "record_id": record_id,
                }),
                &format!("added message {ts} to list {list_id} (record {record_id})\n"),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `watch [--channel <ch>] [--count N]`: stream live Slack events over the RTM websocket.
pub async fn run_watch(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: Option<&str>,
    count: Option<usize>,
) -> Result<ExitCode> {
    let session = match resolve_session(flag) {
        Ok(session) => session,
        Err(not_signed_in) => return Ok(fail(ctx, "not_signed_in", &not_signed_in.to_string())),
    };
    let filter_ch = channel_arg.map(|ch| resolve_channel(&session.team_id, ch).0);
    let mut cfg = RtmConfig::new(ConnectParams::new(session.xoxc_token.clone()));
    cfg.d_cookie = Some(session.d_cookie.clone());
    let (_rtm, mut rx, driver) = match RtmClient::new(cfg) {
        Ok(parts) => parts,
        Err(err) => return Ok(fail(ctx, "client_error", &format!("starting RTM: {err}"))),
    };
    let driver_task = tokio::spawn(driver);
    let mut emitted = 0usize;

    while let Some(ev) = rx.recv().await {
        match ev {
            RtmEvent::State(ConnectionState::Disconnected { reason }) => {
                driver_task.abort();
                return Ok(fail(ctx, "disconnected", &reason));
            }
            RtmEvent::Event(slack_ev) => {
                let maybe_out = format_watch_event(&slack_ev, filter_ch.as_ref());
                if let Some((json_val, human_line)) = maybe_out {
                    output::emit(ctx.json, &json_val, &human_line);
                    emitted += 1;
                    if count.is_some_and(|max| emitted >= max) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    driver_task.abort();
    Ok(ExitCode::SUCCESS)
}

fn format_watch_event(
    ev: &SlackEvent,
    filter_ch: Option<&ChannelId>,
) -> Option<(serde_json::Value, String)> {
    match ev {
        SlackEvent::Message(slackless_core::types::MessageEvent::Message(msg)) => {
            if let Some(target) = filter_ch {
                if msg.channel.as_ref() != Some(target) {
                    return None;
                }
            }
            let ch = msg.channel.as_ref().map(|c| c.as_str()).unwrap_or("");
            let who = msg
                .user
                .as_ref()
                .map(|u| u.as_str())
                .or(msg.bot_id.as_ref().map(|b| b.as_str()))
                .unwrap_or("unknown");
            let json = serde_json::json!({
                "type": "message",
                "channel": ch,
                "ts": msg.ts.as_str(),
                "thread_ts": msg.thread_ts.as_ref().map(|t| t.as_str()),
                "user": msg.user.as_ref().map(|u| u.as_str()),
                "text": msg.text,
            });
            let human = format!("message\t{ch}\t{}\t{who}: {}\n", msg.ts, msg.text);
            Some((json, human))
        }
        SlackEvent::ReactionAdded(rx) => {
            if let Some(target) = filter_ch {
                if rx.item.channel.as_ref() != Some(target) {
                    return None;
                }
            }
            let ch = rx.item.channel.as_ref().map(|c| c.as_str()).unwrap_or("");
            let ts = rx.item.ts.as_ref().map(|t| t.as_str()).unwrap_or("");
            let json = serde_json::json!({
                "type": "reaction_added",
                "channel": ch,
                "ts": ts,
                "user": rx.user.as_str(),
                "reaction": rx.reaction,
            });
            let human = format!(
                "reaction_added\t{ch}\t{ts}\t{} :{}: \n",
                rx.user, rx.reaction
            );
            Some((json, human))
        }
        SlackEvent::ThreadMarked(tm) => {
            let sub = &tm.subscription;
            if let Some(target) = filter_ch {
                if &sub.channel != target {
                    return None;
                }
            }
            let last_read = sub.last_read.as_ref().map(|t| t.as_str());
            let json = serde_json::json!({
                "type": "thread_marked",
                "channel": sub.channel.as_str(),
                "thread_ts": sub.thread_ts.as_str(),
                "last_read": last_read,
                "active": sub.active,
            });
            let human = format!(
                "thread_marked\t{}\t{}\tlast_read={}\n",
                sub.channel,
                sub.thread_ts,
                last_read.unwrap_or("-")
            );
            Some((json, human))
        }
        _ => None,
    }
}

pub(crate) fn cell_to_text(cell: Option<&SlackListCellValue>) -> String {
    match cell {
        None | Some(SlackListCellValue::Empty) => String::new(),
        Some(SlackListCellValue::Text(s))
        | Some(SlackListCellValue::Date(s))
        | Some(SlackListCellValue::Number(s)) => s.clone(),
        Some(SlackListCellValue::Users(u)) => u
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        Some(SlackListCellValue::Select(opts)) => opts.join(", "),
        Some(SlackListCellValue::Checkbox(b)) => b.to_string(),
        Some(SlackListCellValue::Link { label, url }) => {
            if label.is_empty() {
                url.clone()
            } else {
                label.clone()
            }
        }
    }
}

pub(crate) fn list_to_json_value(data: &SlackListData) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = data
        .rows
        .iter()
        .map(|row| {
            let mut fields = serde_json::Map::new();
            for col in &data.columns {
                let cell = row.cells.get(&col.id).or_else(|| row.cells.get(&col.key));
                fields.insert(
                    col.name.clone(),
                    serde_json::Value::String(cell_to_text(cell)),
                );
            }
            serde_json::json!({ "id": row.id, "fields": fields })
        })
        .collect();
    serde_json::json!({
        "file_id": data.file_id,
        "title": data.title,
        "description": data.description,
        "columns": data.columns.iter().map(|c| serde_json::json!({
            "id": c.id,
            "name": c.name,
            "kind": c.kind,
            "is_primary": c.is_primary,
        })).collect::<Vec<_>>(),
        "count": rows.len(),
        "items": rows,
    })
}

pub(crate) fn list_to_csv(data: &SlackListData) -> String {
    let escape = |s: &str| {
        if s.contains(',') || s.contains('"') || s.contains('\n') {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_owned()
        }
    };
    let mut out = String::new();
    let header: Vec<String> = data.columns.iter().map(|c| escape(&c.name)).collect();
    out.push_str(&header.join(","));
    out.push('\n');
    for row in &data.rows {
        let cells: Vec<String> = data
            .columns
            .iter()
            .map(|c| {
                let cell = row.cells.get(&c.id).or_else(|| row.cells.get(&c.key));
                escape(&cell_to_text(cell))
            })
            .collect();
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    out
}

pub(crate) fn list_to_tsv(data: &SlackListData) -> String {
    let mut out = String::new();
    let header: Vec<&str> = std::iter::once("id")
        .chain(data.columns.iter().map(|c| c.name.as_str()))
        .collect();
    out.push_str(&header.join("\t"));
    out.push('\n');
    for row in &data.rows {
        let mut cols = vec![row.id.clone()];
        for c in &data.columns {
            let cell = row.cells.get(&c.id).or_else(|| row.cells.get(&c.key));
            cols.push(cell_to_text(cell).replace(['\t', '\n'], " "));
        }
        out.push_str(&cols.join("\t"));
        out.push('\n');
    }
    out
}

/// `huddle devices` (offline): list host input microphones and output speakers.
pub fn run_huddle_devices(ctx: &Ctx) -> Result<ExitCode> {
    let devices = slackless_core::huddle::list_audio_devices();
    let mut human = String::new();
    human.push_str("INPUTS\n");
    for d in &devices.inputs {
        let mark = if d.is_default { "*" } else { " " };
        human.push_str(&format!("{mark}\t{}\n", d.name));
    }
    human.push_str("OUTPUTS\n");
    for d in &devices.outputs {
        let mark = if d.is_default { "*" } else { " " };
        human.push_str(&format!("{mark}\t{}\n", d.name));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({
            "inputs": devices.inputs.iter().map(|d| serde_json::json!({"name": d.name, "default": d.is_default})).collect::<Vec<_>>(),
            "outputs": devices.outputs.iter().map(|d| serde_json::json!({"name": d.name, "default": d.is_default})).collect::<Vec<_>>(),
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

/// `huddle cameras` (offline): list available local cameras.
pub fn run_huddle_cameras(ctx: &Ctx) -> Result<ExitCode> {
    let cameras = slackless_core::huddle::list_cameras();
    let mut human = String::new();
    for c in &cameras {
        human.push_str(&format!("{}\t{}\n", c.index, c.name));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({
            "cameras": cameras.iter().map(|c| serde_json::json!({"index": c.index, "name": c.name})).collect::<Vec<_>>(),
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

/// `huddle screens` (offline): list monitors and windows available for screen sharing.
pub fn run_huddle_screens(ctx: &Ctx) -> Result<ExitCode> {
    let sources = slackless_core::huddle::list_screen_sources();
    let mut human = String::new();
    for s in &sources {
        let kind = if s.is_monitor { "monitor" } else { "window" };
        human.push_str(&format!(
            "{}\t{kind}\t{}x{}\t{}\n",
            s.id, s.width, s.height, s.name
        ));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({
            "sources": sources.iter().map(|s| serde_json::json!({
                "id": s.id,
                "name": s.name,
                "monitor": s.is_monitor,
                "width": s.width,
                "height": s.height,
            })).collect::<Vec<_>>(),
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

/// `huddle link <channel>`: fetch the shareable huddle join URL (`rooms.getLink`).
pub async fn run_huddle_link(ctx: &Ctx, flag: Option<&str>, channel_arg: &str) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, _label) = resolve_channel(&workspace, channel_arg);
    match api.rooms_get_link(&channel).await {
        Ok(url_opt) => {
            let url = url_opt
                .unwrap_or_else(|| format!("https://app.slack.com/huddle/{workspace}/{channel}"));
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "channel": channel.as_str(),
                    "url": url,
                }),
                &format!("{url}\n"),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `huddle invite <channel> <user>`: ring/notify a workspace member to join (`rooms.notifyMember`).
pub async fn run_huddle_invite(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    user_arg: &str,
) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&workspace, channel_arg);
    let user = slackless_core::types::UserId::from(user_arg.to_owned());
    match api.rooms_notify_member(&channel, &user).await {
        Ok(()) => {
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "channel": channel.as_str(),
                    "user": user.as_str(),
                }),
                &format!(
                    "invited {} to huddle in {}\n",
                    user,
                    label.unwrap_or_else(|| channel.to_string())
                ),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `huddle join <channel> [--muted] [--duration N] [--input NAME] [--output NAME] [--react EMOJI]`:
/// join a live native Chime audio huddle from the terminal and leave cleanly on Ctrl+C or timeout.
#[allow(clippy::too_many_arguments)]
pub async fn run_huddle_join(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    muted: bool,
    duration: Option<u64>,
    input: Option<String>,
    output_dev: Option<String>,
    react: Option<String>,
) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&workspace, channel_arg);
    let joined = match api.rooms_join(&channel, None, None).await {
        Ok(resp) => resp,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };

    let creds = slackless_core::huddle::HuddleCredentials {
        channel: channel.clone(),
        call_id: joined.call_id.clone(),
        meeting_id: joined.chime.meeting.meeting_id,
        attendee_id: joined.chime.attendee.attendee_id,
        external_user_id: joined.chime.attendee.external_user_id,
        join_token: joined.chime.attendee.join_token,
        signaling_url: joined.chime.meeting.media_placement.signaling_url,
        turn_control_url: joined.chime.meeting.media_placement.turn_control_url,
        audio_host_url: joined.chime.meeting.media_placement.audio_host_url,
        thread_root_ts: joined.huddle.thread_root_ts.or_else(|| {
            joined
                .canvas
                .as_ref()
                .and_then(|c| c.root_thread_ts.clone())
        }),
        canvas_file_id: joined.canvas.as_ref().and_then(|c| c.file_id.clone()),
        huddle_link: joined.huddle.huddle_link.or_else(|| {
            Some(format!(
                "https://app.slack.com/huddle/{workspace}/{channel}"
            ))
        }),
    };
    let audio = slackless_core::huddle::HuddleAudioConfig {
        muted,
        input_device: input,
        output_device: output_dev,
        ..Default::default()
    };

    let (upd_tx, mut upd_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = slackless_core::huddle::spawn_huddle_session(creds, audio, upd_tx);
    let display = label.unwrap_or_else(|| channel.to_string());

    let deadline =
        duration.map(|secs| tokio::time::Instant::now() + std::time::Duration::from_secs(secs));
    let mut exit_code = ExitCode::SUCCESS;
    let mut sent_react = false;

    loop {
        let timer_fut = async {
            if let Some(at) = deadline {
                tokio::time::sleep_until(at).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                handle.leave();
                break;
            }
            _ = timer_fut => {
                handle.leave();
                break;
            }
            upd = upd_rx.recv() => {
                match upd {
                    Some(slackless_core::huddle::HuddleUpdate::State(snap)) => {
                        let phase = format!("{:?}", snap.phase);
                        output::emit(
                            ctx.json,
                            &serde_json::json!({
                                "event": "state",
                                "phase": phase,
                                "channel": snap.channel.as_str(),
                                "call_id": snap.call_id,
                                "muted": snap.muted,
                                "packets_sent": snap.packets_sent,
                                "packets_received": snap.packets_received,
                                "pongs_sent": snap.pongs_sent,
                                "error": snap.error,
                                "udp_blocked": snap.udp_blocked,
                            }),
                            &format!(
                                "huddle {display}\t{phase}\ttx={} rx={} pongs={}{}\n",
                                snap.packets_sent,
                                snap.packets_received,
                                snap.pongs_sent,
                                snap.error
                                    .as_deref()
                                    .map(|e| format!("\t{e}"))
                                    .unwrap_or_default()
                            ),
                        );
                        if snap.phase == slackless_core::huddle::HuddlePhase::Connected
                            && !sent_react
                        {
                            if let Some(emoji) = &react {
                                sent_react = true;
                                handle.send(slackless_core::huddle::HuddleControl::SendReaction {
                                    emoji: emoji.clone(),
                                    kind: slackless_core::huddle::HuddleReactionKind::Standard,
                                    emoji_url: None,
                                });
                            }
                        }
                        if snap.phase == slackless_core::huddle::HuddlePhase::Failed {
                            exit_code = ExitCode::FAILURE;
                            break;
                        }
                        if snap.phase == slackless_core::huddle::HuddlePhase::Idle {
                            break;
                        }
                    }
                    Some(slackless_core::huddle::HuddleUpdate::DataEvent(ev)) => {
                        output::emit(
                            ctx.json,
                            &serde_json::json!({
                                "event": "data",
                                "detail": format!("{ev:?}"),
                            }),
                            &format!("huddle {display}\tdata\t{ev:?}\n"),
                        );
                    }
                    Some(
                        slackless_core::huddle::HuddleUpdate::AudioLevels(_)
                        | slackless_core::huddle::HuddleUpdate::VideoFrame { .. }
                        | slackless_core::huddle::HuddleUpdate::VideoTileClosed(_),
                    ) => {}
                    None => break,
                }
            }
        }
    }

    // Drain briefly so Chime receives the LEAVE frame before process exit, printing final stats.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while let Some(upd) = upd_rx.recv().await {
            if let slackless_core::huddle::HuddleUpdate::State(snap) = upd {
                let phase = format!("{:?}", snap.phase);
                output::emit(
                    ctx.json,
                    &serde_json::json!({
                        "event": "state",
                        "phase": phase,
                        "channel": snap.channel.as_str(),
                        "call_id": snap.call_id,
                        "muted": snap.muted,
                        "packets_sent": snap.packets_sent,
                        "packets_received": snap.packets_received,
                        "pongs_sent": snap.pongs_sent,
                        "error": snap.error,
                        "udp_blocked": snap.udp_blocked,
                    }),
                    &format!(
                        "huddle {display}\t{phase}\ttx={} rx={} pongs={}\n",
                        snap.packets_sent, snap.packets_received, snap.pongs_sent
                    ),
                );
                if snap.phase == slackless_core::huddle::HuddlePhase::Idle {
                    break;
                }
            }
        }
    })
    .await;

    Ok(exit_code)
}

/// `status show`
pub async fn run_status_show(ctx: &Ctx, flag: Option<&str>) -> Result<ExitCode> {
    let session = match resolve_session(flag) {
        Ok(session) => session,
        Err(not_signed_in) => return Ok(fail(ctx, "not_signed_in", &not_signed_in.to_string())),
    };
    let api = match SlackApi::new(
        &session.api_base(),
        session.xoxc_token.clone(),
        &session.d_cookie,
    ) {
        Ok(api) => api,
        Err(err) => {
            return Ok(fail(
                ctx,
                "client_error",
                &format!("building the Slack client: {err}"),
            ));
        }
    };
    match api.users_profile_get(&session.user_id).await {
        Ok(profile) => {
            let text = profile.status_text.unwrap_or_default();
            let emoji = profile.status_emoji.unwrap_or_default();
            let expiration = profile.status_expiration.unwrap_or(0);
            let human = if text.is_empty() && emoji.is_empty() {
                "(no status set)\n".to_owned()
            } else {
                format!("{emoji} {text}\n").trim_start().to_owned()
            };
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "user": session.user_id.as_str(),
                    "status_text": text,
                    "status_emoji": emoji,
                    "status_expiration": expiration,
                }),
                &human,
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `status set <TEXT> [--emoji EMOJI] [--expires-in SECS]`
pub async fn run_status_set(
    ctx: &Ctx,
    flag: Option<&str>,
    text: String,
    emoji: Option<String>,
    expires_in: Option<i64>,
) -> Result<ExitCode> {
    let (api, _) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let status_emoji = emoji.map(|e| {
        let trimmed = e.trim().trim_matches(':');
        if trimmed.is_empty() {
            String::new()
        } else {
            format!(":{trimmed}:")
        }
    });
    let status_expiration = expires_in.map(|secs| {
        if secs <= 0 {
            0
        } else {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            now.saturating_add(secs)
        }
    });
    let update = slackless_core::api::ProfileUpdate {
        status_text: Some(text.clone()),
        status_emoji: status_emoji.clone(),
        status_expiration,
        ..Default::default()
    };
    match api.users_profile_set(&update).await {
        Ok(profile) => {
            let em = profile.status_emoji.unwrap_or_default();
            let txt = profile.status_text.unwrap_or(text);
            let exp = profile.status_expiration.unwrap_or(0);
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "status_text": txt,
                    "status_emoji": em,
                    "status_expiration": exp,
                }),
                &format!("status set: {em} {txt}\n"),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `status clear`
pub async fn run_status_clear(ctx: &Ctx, flag: Option<&str>) -> Result<ExitCode> {
    let (api, _) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let update = slackless_core::api::ProfileUpdate {
        status_text: Some(String::new()),
        status_emoji: Some(String::new()),
        status_expiration: Some(0),
        ..Default::default()
    };
    match api.users_profile_set(&update).await {
        Ok(_) => {
            output::emit(
                ctx.json,
                &serde_json::json!({ "ok": true, "cleared": true }),
                "status cleared\n",
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `unread`: live unread channels, DMs, mentions, and threads (`client.counts`).
pub async fn run_unread(ctx: &Ctx, flag: Option<&str>) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let counts = match api.client_counts().await {
        Ok(c) => c,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let archive = Archive::open(&workspace);
    let label_for = |id: &ChannelId| -> String {
        archive
            .as_ref()
            .and_then(|a| a.resolve_channel(&workspace, id.as_str()))
            .map(|r| r.label())
            .unwrap_or_else(|| id.to_string())
    };

    let mut items = Vec::new();
    let mut lines = String::new();
    let mut total_mentions: u32 = 0;

    let groups: [(&str, &[slackless_core::api::ConversationCounts]); 3] = [
        ("dm", &counts.ims),
        ("mpim", &counts.mpims),
        ("channel", &counts.channels),
    ];
    for (kind, list) in groups {
        for c in list.iter().filter(|c| c.is_unread()) {
            let mentions = c.effective_mentions();
            total_mentions = total_mentions.saturating_add(mentions);
            let label = label_for(&c.id);
            lines.push_str(&format!(
                "{kind}\t{}\t{label}\tmentions={mentions}\n",
                c.id.as_str()
            ));
            items.push(serde_json::json!({
                "id": c.id.as_str(),
                "label": label,
                "kind": kind,
                "mentions": mentions,
                "has_unreads": c.has_unreads,
                "latest": c.latest.as_ref().map(|t| t.as_str()),
            }));
        }
    }
    let threads_unread = counts.threads.has_unreads || counts.threads.mention_count > 0;
    if items.is_empty() && !threads_unread {
        lines.push_str("all caught up\n");
    } else if threads_unread {
        lines.push_str(&format!(
            "threads\t-\tThreads\tmentions={}\n",
            counts.threads.mention_count
        ));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({
            "ok": true,
            "workspace": workspace.as_str(),
            "unread_conversations": items.len(),
            "total_mentions": total_mentions,
            "threads_unread": threads_unread,
            "threads_mentions": counts.threads.mention_count,
            "items": items,
        }),
        &lines,
    );
    Ok(ExitCode::SUCCESS)
}

/// `download <FILE_ID> [--out PATH]`
pub async fn run_download(
    ctx: &Ctx,
    flag: Option<&str>,
    file_id: &str,
    out: Option<&std::path::Path>,
) -> Result<ExitCode> {
    let (api, _) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let file = match api.files_info(file_id).await {
        Ok(f) => f,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let Some(url) = file
        .url_private_download
        .as_deref()
        .or(file.url_private.as_deref())
    else {
        return Ok(fail(
            ctx,
            "no_download_url",
            "file has no downloadable private URL",
        ));
    };
    let bytes = match api.download_file(url).await {
        Ok(b) => b,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let fallback_name = file
        .name
        .as_deref()
        .or(file.title.as_deref())
        .unwrap_or(file_id);
    let safe_name = std::path::Path::new(fallback_name)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("{file_id}.bin"));
    let dest = out
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(&safe_name));
    if let Err(err) = std::fs::write(&dest, &bytes) {
        return Ok(fail(
            ctx,
            "write_failed",
            &format!("writing {}: {err}", dest.display()),
        ));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({
            "ok": true,
            "file_id": file_id,
            "name": safe_name,
            "bytes": bytes.len(),
            "path": dest.display().to_string(),
        }),
        &format!("downloaded {} ({} bytes)\n", dest.display(), bytes.len()),
    );
    Ok(ExitCode::SUCCESS)
}

/// `backfill <CHANNEL> [--pages N] [--page-size N]`
/// Walks `conversations.history` pages politely (Tier-3 pacing + 429 retry) and upserts every
/// fetched message into the local SQLite archive (`X-1`).
pub async fn run_backfill(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    max_pages: usize,
    page_size: u32,
) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&workspace, channel_arg);
    let Some(path) = crate::archive::store_path(&workspace) else {
        return Ok(fail(
            ctx,
            "no_store_path",
            "could not resolve local archive path",
        ));
    };
    let mut store = slackless_store::Store::open(&path)?;
    let opts = slackless_core::api::HistoryOpts {
        limit: Some(page_size.clamp(1, 200)),
        ..Default::default()
    };
    let mut cursor: Option<String> = None;
    let mut pages_fetched: usize = 0;
    let mut messages_stored: usize = 0;
    let mut reached_end = false;

    loop {
        if max_pages > 0 && pages_fetched >= max_pages {
            break;
        }
        let page = loop {
            match api
                .conversations_history(&channel, &opts, cursor.clone())
                .await
            {
                Ok(page) => break page,
                Err(ApiError::RateLimited { retry_after }) => {
                    tokio::time::sleep(retry_after.max(std::time::Duration::from_secs(1))).await;
                }
                Err(err) => return Ok(api_fail(ctx, &err)),
            }
        };
        pages_fetched += 1;
        for msg in &page.items {
            store.upsert_message(&workspace, &channel, msg)?;
            messages_stored += 1;
        }
        match page.next_cursor {
            Some(next) if !next.is_empty() => {
                cursor = Some(next);
                tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
            }
            _ => {
                reached_end = true;
                break;
            }
        }
    }

    let display = label.unwrap_or_else(|| channel.to_string());
    output::emit(
        ctx.json,
        &serde_json::json!({
            "ok": true,
            "workspace": workspace.as_str(),
            "channel": channel.as_str(),
            "label": display,
            "pages": pages_fetched,
            "messages": messages_stored,
            "reached_end": reached_end,
        }),
        &format!(
            "backfilled {display}: {messages_stored} messages across {pages_fetched} page(s) (complete={reached_end})\n"
        ),
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use slackless_core::api::{SlackListColumn, SlackListRow};
    use std::collections::HashMap;

    #[test]
    fn formats_list_as_json_csv_and_tsv() {
        let mut cells = HashMap::new();
        cells.insert("Col1".into(), SlackListCellValue::Text("Ship, v1".into()));
        cells.insert("Col2".into(), SlackListCellValue::Checkbox(true));
        let data = SlackListData {
            file_id: "F0LIST1".into(),
            title: "Launch".into(),
            description: String::new(),
            columns: vec![
                SlackListColumn {
                    id: "Col1".into(),
                    key: "name".into(),
                    name: "Task".into(),
                    kind: "text".into(),
                    is_primary: true,
                    choices: Vec::new(),
                },
                SlackListColumn {
                    id: "Col2".into(),
                    key: "done".into(),
                    name: "Done".into(),
                    kind: "checkbox".into(),
                    is_primary: false,
                    choices: Vec::new(),
                },
            ],
            rows: vec![SlackListRow {
                id: "Rec1".into(),
                cells,
            }],
        };

        let json = list_to_json_value(&data);
        assert_eq!(json["count"], 1);
        assert_eq!(json["items"][0]["fields"]["Task"], "Ship, v1");
        assert_eq!(json["items"][0]["fields"]["Done"], "true");

        let csv = list_to_csv(&data);
        assert!(csv.contains("Task,Done\n"));
        assert!(csv.contains("\"Ship, v1\",true\n"));

        let tsv = list_to_tsv(&data);
        assert!(tsv.contains("id\tTask\tDone\n"));
        assert!(tsv.contains("Rec1\tShip, v1\ttrue\n"));
    }
}
