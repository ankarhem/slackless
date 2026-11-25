//! `canvas` and `list` live commands and table/CSV/JSON formatters.

use std::process::ExitCode;

use anyhow::Result;
use slackless_core::api::{
    CanvasEditOp, ListCellInput, ListCellInputValue, SlackListCellValue, SlackListData,
};
use slackless_core::types::Ts;

use crate::output;
use crate::{Ctx, ListReadFormat};

use super::session::{api_fail, api_for, resolve_channel};

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
    let (json, human) = format_canvas_read(file_id, format, &title, &content);
    output::emit(ctx.json, &json, &human);
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
    let op = build_canvas_edit_op(markdown, section, append);
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
    let (api, _) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let data = match api.fetch_list(list_id).await {
        Ok(data) => data,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    match effective_list_format(ctx.json, format) {
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
    let (api, _) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let col_id = match column {
        Some(c) => c,
        None => match api.fetch_list(list_id).await {
            Ok(data) => select_list_primary_column(&data),
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
