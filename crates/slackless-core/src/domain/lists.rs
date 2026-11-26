//! Unified Slack Lists RFC-4180 CSV parser, CSV/TSV/JSON serializers, and import mapping.

use serde_json::{Map, Number, Value, json};

use crate::api::{
    ApiError, ListAccessLevel, ListCellInput, ListCellInputValue, ListMeta, SlackListCellValue,
    SlackListColumn, SlackListData, SlackListRow, format_list_error,
};
use crate::domain::canvas::parse_share_targets;
use crate::types::{ChannelId, UserId};

/// Active tab inside the List Share dialog (`F-COLLAB-275`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListShareTab {
    #[default]
    Share,
    Access,
    Settings,
}

/// Validated share/access-grant request for a Slack List (`F-COLLAB-243..245`, `F-COLLAB-274..278`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListShareAccessRequest {
    pub access_level: ListAccessLevel,
    pub channel_ids: Vec<ChannelId>,
    pub user_ids: Vec<UserId>,
    /// `Some(permalink)` posts the list link to shared channels; `None` grants silent access
    /// (`F-COLLAB-274`).
    pub post_permalink: Option<String>,
    /// Optional record ID when sharing from an item (`F-COLLAB-276`: sharing an item grants list
    /// access).
    pub item_record_id: Option<String>,
}

/// Validate and build a [`ListShareAccessRequest`] from raw target IDs and share options.
pub fn build_list_share_request(
    raw_targets: &str,
    access_level: ListAccessLevel,
    share_without_posting: bool,
    permalink: Option<&str>,
    item_record_id: Option<&str>,
) -> Result<ListShareAccessRequest, String> {
    let (channel_ids, user_ids) = parse_share_targets(raw_targets);
    if channel_ids.is_empty() && user_ids.is_empty() {
        return Err(
            "Enter at least one Slack channel ID (C…) or user ID (U…) to share this list with."
                .to_owned(),
        );
    }
    if access_level == ListAccessLevel::Owner && !channel_ids.is_empty() && user_ids.is_empty() {
        return Err(
            "Owner access can only be granted to individual users (U…), not channels.".to_owned(),
        );
    }
    let post_permalink = if share_without_posting || channel_ids.is_empty() {
        None
    } else {
        permalink
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let item_record_id = item_record_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Ok(ListShareAccessRequest {
        access_level,
        channel_ids,
        user_ids,
        post_permalink,
        item_record_id,
    })
}

/// Summarize a list's general workspace access badge and description (`F-COLLAB-244`,
/// `F-COLLAB-247`, `F-COLLAB-277`, `F-COLLAB-278`).
pub fn list_general_access_summary(
    meta: Option<&ListMeta>,
    local_override: Option<&str>,
    limit_sharing: bool,
) -> (&'static str, String) {
    let raw = local_override
        .filter(|s| !s.is_empty())
        .or_else(|| meta.and_then(|m| m.org_or_workspace_access.as_deref()))
        .or_else(|| meta.and_then(|m| m.access.as_deref()))
        .unwrap_or("private")
        .to_ascii_lowercase();

    let (badge, mut desc) = match raw.as_str() {
        "write" | "edit" | "workspace_edit" => (
            "Workspace can edit",
            "Anyone in this workspace with the link can view, comment, and edit items.".to_owned(),
        ),
        "read" | "view" | "workspace_view" => (
            "Workspace can view",
            "Anyone in this workspace with the link can view items and comment on item threads."
                .to_owned(),
        ),
        _ => (
            "Invite only",
            "Only people and channels explicitly added can open this list.".to_owned(),
        ),
    };
    if limit_sharing {
        desc.push_str(" Re-sharing is limited to list owners.");
    }
    (badge, desc)
}

/// Translate a Slack [`ApiError`] from a list endpoint into clear user-facing guidance.
pub fn format_list_api_error(err: &ApiError) -> String {
    match err {
        ApiError::Slack(code) | ApiError::AuthExpired(code) => format_list_error(code),
        other => format_list_error(&other.to_string()),
    }
}

/// Look up a cell in `row` by column ID first, falling back to column key.
pub fn row_cell<'a>(
    row: &'a SlackListRow,
    col: &SlackListColumn,
) -> Option<&'a SlackListCellValue> {
    row.cells.get(&col.id).or_else(|| row.cells.get(&col.key))
}

/// Resolve a select option ID to its human-readable label when declared on `col.choices`.
pub fn resolve_choice_label<'a>(col: &'a SlackListColumn, opt: &'a str) -> &'a str {
    col.choices
        .iter()
        .find(|c| c.value == opt)
        .map(|c| c.label.as_str())
        .unwrap_or(opt)
}

/// Format a cell value as human-readable text, resolving choice labels and user display names
/// through `resolve_user`.
pub fn cell_to_text<F>(
    cell: Option<&SlackListCellValue>,
    col: &SlackListColumn,
    resolve_user: &F,
) -> String
where
    F: Fn(&UserId) -> String,
{
    match cell {
        None | Some(SlackListCellValue::Empty) => String::new(),
        Some(SlackListCellValue::Text(s))
        | Some(SlackListCellValue::Date(s))
        | Some(SlackListCellValue::Number(s)) => s.clone(),
        Some(SlackListCellValue::Link { url, label }) => {
            if label.is_empty() || label == url {
                url.clone()
            } else {
                format!("{label} ({url})")
            }
        }
        Some(SlackListCellValue::Checkbox(checked)) => {
            if *checked { "true" } else { "false" }.to_owned()
        }
        Some(SlackListCellValue::Users(users)) => users
            .iter()
            .map(resolve_user)
            .collect::<Vec<_>>()
            .join("; "),
        Some(SlackListCellValue::Select(opts)) => opts
            .iter()
            .map(|opt| resolve_choice_label(col, opt))
            .collect::<Vec<_>>()
            .join("; "),
    }
}

/// Append one RFC-4180 CSV field to `out`, properly quoting commas, quotes, `\n`, and `\r`.
pub fn push_csv_field(out: &mut String, field: &str) {
    if field.contains([',', '"', '\n', '\r']) {
        out.push('"');
        for ch in field.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
    } else {
        out.push_str(field);
    }
}

/// Serialize a live Slack List to RFC-4180 CSV, resolving choice labels and user names via
/// `resolve_user`.
pub fn list_to_csv<F>(data: &SlackListData, resolve_user: F) -> String
where
    F: Fn(&UserId) -> String,
{
    let mut out = String::new();
    for (ix, col) in data.columns.iter().enumerate() {
        if ix > 0 {
            out.push(',');
        }
        push_csv_field(&mut out, &col.name);
    }
    out.push('\n');
    for row in &data.rows {
        for (ix, col) in data.columns.iter().enumerate() {
            if ix > 0 {
                out.push(',');
            }
            let val = cell_to_text(row_cell(row, col), col, &resolve_user);
            push_csv_field(&mut out, &val);
        }
        out.push('\n');
    }
    out
}

/// Serialize a live Slack List to TSV (tab-separated values with an `id` leading column).
pub fn list_to_tsv<F>(data: &SlackListData, resolve_user: F) -> String
where
    F: Fn(&UserId) -> String,
{
    let mut out = String::new();
    let header: Vec<&str> = std::iter::once("id")
        .chain(data.columns.iter().map(|c| c.name.as_str()))
        .collect();
    out.push_str(&header.join("\t"));
    out.push('\n');
    for row in &data.rows {
        let mut cols = vec![row.id.clone()];
        for col in &data.columns {
            let text = cell_to_text(row_cell(row, col), col, &resolve_user);
            cols.push(text.replace(['\t', '\n', '\r'], " "));
        }
        out.push_str(&cols.join("\t"));
        out.push('\n');
    }
    out
}

/// Serialize a live Slack List to a structured, typed JSON [`Value`], resolving choice labels and
/// user display names through `resolve_user`.
pub fn list_to_json_value<F>(data: &SlackListData, resolve_user: F) -> Value
where
    F: Fn(&UserId) -> String,
{
    let items: Vec<Value> = data
        .rows
        .iter()
        .map(|row| {
            let mut obj = Map::new();
            obj.insert("id".to_owned(), Value::String(row.id.clone()));
            for col in &data.columns {
                let val = match row_cell(row, col) {
                    None | Some(SlackListCellValue::Empty) => Value::Null,
                    Some(SlackListCellValue::Text(s)) | Some(SlackListCellValue::Date(s)) => {
                        Value::String(s.clone())
                    }
                    Some(SlackListCellValue::Number(n)) => n
                        .parse::<f64>()
                        .ok()
                        .and_then(Number::from_f64)
                        .map(Value::Number)
                        .unwrap_or_else(|| Value::String(n.clone())),
                    Some(SlackListCellValue::Checkbox(b)) => Value::Bool(*b),
                    Some(SlackListCellValue::Link { label, url }) => json!({
                        "label": label,
                        "url": url,
                    }),
                    Some(SlackListCellValue::Users(users)) => Value::Array(
                        users
                            .iter()
                            .map(|u| Value::String(resolve_user(u)))
                            .collect(),
                    ),
                    Some(SlackListCellValue::Select(opts)) => Value::Array(
                        opts.iter()
                            .map(|opt| Value::String(resolve_choice_label(col, opt).to_owned()))
                            .collect(),
                    ),
                };
                obj.insert(col.name.clone(), val);
            }
            Value::Object(obj)
        })
        .collect();
    json!({
        "file_id": data.file_id,
        "title": data.title,
        "description": data.description,
        "columns": data.columns.iter().map(|c| json!({
            "id": c.id,
            "name": c.name,
            "type": c.kind,
            "is_primary": c.is_primary,
        })).collect::<Vec<_>>(),
        "count": items.len(),
        "items": items,
    })
}

/// Serialize a live Slack List to pretty-printed JSON.
pub fn list_to_json<F>(data: &SlackListData, resolve_user: F) -> String
where
    F: Fn(&UserId) -> String,
{
    serde_json::to_string_pretty(&list_to_json_value(data, resolve_user)).unwrap_or_default()
}

/// Parse RFC-4180 CSV text into rows of unescaped fields, skipping completely blank lines.
pub fn parse_csv_rows(input: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = input.chars().peekable();
    let mut in_quotes = false;
    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(ch);
            }
        } else {
            match ch {
                '"' if field.is_empty() => in_quotes = true,
                ',' => {
                    row.push(std::mem::take(&mut field));
                }
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    row.push(std::mem::take(&mut field));
                    if row.iter().any(|c| !c.trim().is_empty()) {
                        rows.push(std::mem::take(&mut row));
                    } else {
                        row.clear();
                    }
                }
                '\n' => {
                    row.push(std::mem::take(&mut field));
                    if row.iter().any(|c| !c.trim().is_empty()) {
                        rows.push(std::mem::take(&mut row));
                    } else {
                        row.clear();
                    }
                }
                other => field.push(other),
            }
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        if row.iter().any(|c| !c.trim().is_empty()) {
            rows.push(row);
        }
    }
    rows
}

/// Map one parsed CSV data row onto Slack List [`ListCellInput`]s using `(column_id, name, kind)`
/// schema descriptors.
pub fn map_csv_row_to_cells(
    headers: &[String],
    values: &[String],
    cols: &[(String, String, String)],
    primary_ix: usize,
) -> Vec<ListCellInput> {
    let mut cells = Vec::new();
    for (idx, val) in values.iter().enumerate() {
        let trimmed = val.trim();
        if trimmed.is_empty() {
            continue;
        }
        let header = headers.get(idx).map(|s| s.trim()).unwrap_or("");
        let matched = cols
            .iter()
            .find(|(_, name, _)| !header.is_empty() && name.eq_ignore_ascii_case(header))
            .or_else(|| (idx == 0).then(|| cols.get(primary_ix)).flatten());
        let Some((col_id, _, kind)) = matched else {
            continue;
        };
        let cell_val = match kind.as_str() {
            "checkbox" | "todo_completed" | "completed" => ListCellInputValue::Checkbox(matches!(
                trimmed.to_ascii_lowercase().as_str(),
                "true" | "1" | "yes" | "x"
            )),
            "date" | "todo_due_date" | "due_date" => {
                ListCellInputValue::Date(vec![trimmed.to_owned()])
            }
            "number" | "rating" | "currency" => trimmed
                .parse::<f64>()
                .map(|n| ListCellInputValue::Number(vec![n]))
                .unwrap_or_else(|_| ListCellInputValue::Text(trimmed.to_owned())),
            "link" => ListCellInputValue::Link {
                url: trimmed.to_owned(),
                display_name: None,
            },
            _ => ListCellInputValue::Text(trimmed.to_owned()),
        };
        cells.push(ListCellInput {
            column_id: col_id.clone(),
            value: cell_val,
        });
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::SlackListChoice;
    use std::collections::HashMap;

    fn sample_list() -> SlackListData {
        let mut cells = HashMap::new();
        cells.insert(
            "col_title".into(),
            SlackListCellValue::Text("Ship \"v2\"\r\nnow".into()),
        );
        cells.insert(
            "col_status".into(),
            SlackListCellValue::Select(vec!["opt_done".into()]),
        );
        cells.insert(
            "col_owner".into(),
            SlackListCellValue::Users(vec![UserId::from("U1")]),
        );
        cells.insert("col_pts".into(), SlackListCellValue::Number("3.5".into()));
        cells.insert("col_done".into(), SlackListCellValue::Checkbox(true));
        SlackListData {
            file_id: "FL1".into(),
            title: "Sprint".into(),
            description: "Q4".into(),
            columns: vec![
                SlackListColumn {
                    id: "col_title".into(),
                    key: "title".into(),
                    name: "Title".into(),
                    kind: "text".into(),
                    is_primary: true,
                    choices: vec![],
                },
                SlackListColumn {
                    id: "col_status".into(),
                    key: "status".into(),
                    name: "Status".into(),
                    kind: "select".into(),
                    is_primary: false,
                    choices: vec![SlackListChoice {
                        value: "opt_done".into(),
                        label: "Done".into(),
                        color: String::new(),
                    }],
                },
                SlackListColumn {
                    id: "col_owner".into(),
                    key: "owner".into(),
                    name: "Owner".into(),
                    kind: "user".into(),
                    is_primary: false,
                    choices: vec![],
                },
                SlackListColumn {
                    id: "col_pts".into(),
                    key: "pts".into(),
                    name: "Points".into(),
                    kind: "number".into(),
                    is_primary: false,
                    choices: vec![],
                },
                SlackListColumn {
                    id: "col_done".into(),
                    key: "done".into(),
                    name: "Done?".into(),
                    kind: "checkbox".into(),
                    is_primary: false,
                    choices: vec![],
                },
            ],
            rows: vec![SlackListRow {
                id: "Rec1".into(),
                cells,
            }],
        }
    }

    #[test]
    fn csv_serializer_escapes_quotes_and_cr_and_resolves_choices_and_users() {
        let data = sample_list();
        let csv = list_to_csv(&data, |u| {
            if u.as_str() == "U1" {
                "Alice".into()
            } else {
                u.to_string()
            }
        });
        assert!(csv.starts_with("Title,Status,Owner,Points,Done?\n"));
        assert!(csv.contains("\"Ship \"\"v2\"\"\r\nnow\",Done,Alice,3.5,true\n"));

        let parsed = parse_csv_rows(&csv);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1][0], "Ship \"v2\"\r\nnow");
        assert_eq!(parsed[1][1], "Done");
        assert_eq!(parsed[1][2], "Alice");
    }

    #[test]
    fn json_serializer_produces_typed_fields_and_resolved_labels() {
        let data = sample_list();
        let val = list_to_json_value(&data, |u| format!("@{u}"));
        assert_eq!(val["count"], json!(1));
        assert_eq!(val["items"][0]["Status"], json!(["Done"]));
        assert_eq!(val["items"][0]["Owner"], json!(["@U1"]));
        assert_eq!(val["items"][0]["Points"], json!(3.5));
        assert_eq!(val["items"][0]["Done?"], json!(true));
    }

    #[test]
    fn map_csv_row_maps_typed_columns_and_primary_fallback() {
        let cols = vec![
            ("C_NAME".into(), "Task".into(), "text".into()),
            ("C_DONE".into(), "Completed".into(), "checkbox".into()),
            ("C_PTS".into(), "Points".into(), "number".into()),
        ];
        let headers = vec!["Unknown".into(), "completed".into(), "Points".into()];
        let row = vec!["Fix bug".into(), "yes".into(), "5".into()];
        let cells = map_csv_row_to_cells(&headers, &row, &cols, 0);
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[0].column_id, "C_NAME");
        assert_eq!(cells[1].column_id, "C_DONE");
        assert!(matches!(cells[1].value, ListCellInputValue::Checkbox(true)));
        assert_eq!(cells[2].column_id, "C_PTS");
    }

    #[test]
    fn list_share_request_parses_targets_enforces_owner_rule_and_silent_mode() {
        let req = build_list_share_request(
            "#C01ENG, @U01ALICE",
            ListAccessLevel::Write,
            false,
            Some("https://acme.slack.com/lists/T1/F1"),
            None,
        )
        .unwrap();
        assert_eq!(req.channel_ids, vec![ChannelId::from("C01ENG")]);
        assert_eq!(req.user_ids, vec![UserId::from("U01ALICE")]);
        assert_eq!(
            req.post_permalink.as_deref(),
            Some("https://acme.slack.com/lists/T1/F1")
        );

        // Silent share omits post_permalink (`F-COLLAB-274`).
        let silent = build_list_share_request(
            "C01ENG",
            ListAccessLevel::Read,
            true,
            Some("https://acme.slack.com/lists/T1/F1"),
            Some("Rec1"),
        )
        .unwrap();
        assert!(silent.post_permalink.is_none());

        // Owner access cannot be assigned to channels.
        let err = build_list_share_request("C01ENG", ListAccessLevel::Owner, true, None, None)
            .unwrap_err();
        assert!(err.contains("Owner"));

        // Empty targets rejected.
        assert!(build_list_share_request("   ", ListAccessLevel::Read, true, None, None).is_err());

        let (read_label, read_desc) = list_general_access_summary(None, Some("read"), false);
        assert!(read_label.contains("view") || read_desc.contains("view"));
        let (write_label, write_desc) = list_general_access_summary(None, Some("write"), true);
        assert!(write_label.contains("edit") || write_desc.contains("edit"));
        assert!(write_desc.to_lowercase().contains("owner"));
        let (priv_label, _) = list_general_access_summary(None, None, false);
        assert!(!priv_label.is_empty());
        assert!(
            format_list_api_error(&slackless_api::ApiError::Slack("access_denied".into()))
                .to_lowercase()
                .contains("permission")
        );
    }
}
