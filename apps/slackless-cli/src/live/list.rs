//! `list` live commands (`read`, `create`, `add-item`, `delete-item`, `add-message`),
//! delegating CSV, TSV, and typed JSON serialization to `slackless_core::domain::lists`.

use std::process::ExitCode;

use anyhow::Result;
use slackless_core::api::{ListCellInput, ListCellInputValue, SlackListData};
use slackless_core::domain::lists;
use slackless_core::types::{Ts, UserId};

use crate::archive::Archive;
use crate::output;
use crate::{Ctx, ListReadFormat};

use super::session::{api_fail, api_for, resolve_channel};

pub(crate) fn effective_list_format(ctx_json: bool, format: ListReadFormat) -> ListReadFormat {
    if ctx_json {
        ListReadFormat::Json
    } else {
        format
    }
}

/// `list read <list_id> [--format table|csv|json]`
pub async fn run_list_read(
    ctx: &Ctx,
    flag: Option<&str>,
    list_id: &str,
    format: ListReadFormat,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let data = match session.api.fetch_list(list_id).await {
        Ok(data) => data,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let user_names = Archive::open(&session.workspace)
        .ok()
        .flatten()
        .and_then(|a| a.user_display_map(&session.workspace).ok())
        .unwrap_or_default();
    let resolve_user =
        |u: &UserId| -> String { user_names.get(u).cloned().unwrap_or_else(|| u.to_string()) };
    match effective_list_format(ctx.json, format) {
        ListReadFormat::Json => {
            let val = lists::list_to_json_value(&data, resolve_user);
            output::emit_json(&val);
        }
        ListReadFormat::Csv => print!("{}", lists::list_to_csv(&data, resolve_user)),
        ListReadFormat::Table => print!("{}", lists::list_to_tsv(&data, resolve_user)),
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
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let channel = channel_arg.map(|ch| resolve_channel(&session.workspace, ch).0);
    match session
        .api
        .lists_create(name, description, channel.as_ref())
        .await
    {
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

pub(crate) fn select_list_primary_column(data: &SlackListData) -> String {
    data.columns
        .iter()
        .find(|c| c.is_primary)
        .or_else(|| data.columns.first())
        .map(|c| c.id.clone())
        .unwrap_or_else(|| "name".to_owned())
}

/// `list add-item <list_id> <title> [--column <col_id>]`
pub async fn run_list_add_item(
    ctx: &Ctx,
    flag: Option<&str>,
    list_id: &str,
    title: String,
    column: Option<String>,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let col_id = match column {
        Some(c) => c,
        None => match session.api.fetch_list(list_id).await {
            Ok(data) => select_list_primary_column(&data),
            Err(err) => return Ok(api_fail(ctx, &err)),
        },
    };
    let fields = vec![ListCellInput {
        column_id: col_id,
        value: ListCellInputValue::Text(title.clone()),
    }];
    match session
        .api
        .lists_records_create(list_id, &fields, None)
        .await
    {
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
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    match session.api.lists_records_delete(list_id, row_id).await {
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
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, _) = resolve_channel(&session.workspace, channel_arg);
    let ts = Ts::from(ts.to_owned());
    match session.api.lists_add_to_list(list_id, &channel, &ts).await {
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
