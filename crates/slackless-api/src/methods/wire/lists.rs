//! Live Slack Lists schema, typed records/cells, and cell mutation wire payloads.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::Value;

use crate::de::value_as_lenient_bool;
use crate::types::{ChannelId, Message, Ts, UserId};

/// Access level for `slackLists.access.set` (`F-COLLAB-243..245`, `F-COLLAB-274..278`).
pub use super::{AccessLevel as ListAccessLevel, comment_channel_id as list_comment_channel_id};

/// Partial-failure report returned by `slackLists.access.delete` (`api-canvases-lists.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ListAccessDeleteResult {
    #[serde(default)]
    pub failed_to_update_channel_ids: Vec<ChannelId>,
    #[serde(default)]
    pub failed_to_update_user_ids: Vec<UserId>,
}

/// Parsed access, sharing, and limit metadata from `files.info` for a Slack List.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListMeta {
    pub file_id: String,
    pub title: String,
    pub permalink: Option<String>,
    pub editable: bool,
    pub access: Option<String>,
    pub org_or_workspace_access: Option<String>,
    pub owner_id: Option<UserId>,
    pub editors: Vec<UserId>,
    pub shared_channels: Vec<ChannelId>,
    pub row_count: Option<u32>,
    pub row_count_limit: Option<u32>,
    pub archived_row_count: Option<u32>,
    pub over_row_maximum: bool,
    pub comment_channel_id: ChannelId,
}

/// Parse [`ListMeta`] from a `files.info` `file` object (or `slackLists.items.list` `list` object).
pub fn parse_list_meta(file_obj: &Value) -> ListMeta {
    let raw = file_obj.get("file").unwrap_or(file_obj);
    let str_field = |key: &str| {
        raw.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let file_id = str_field("id").unwrap_or_default();
    let title = str_field("title")
        .or_else(|| str_field("name"))
        .unwrap_or_else(|| "List".to_owned());
    let permalink = str_field("permalink");
    let editable = raw
        .get("editable")
        .and_then(value_as_lenient_bool)
        .unwrap_or(false);
    let access = str_field("access");
    let org_or_workspace_access = str_field("org_or_workspace_access");
    let owner_id = str_field("user").map(UserId::from);
    let editors = raw
        .get("editors")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(UserId::from)
                .collect()
        })
        .unwrap_or_default();
    let mut shared_channels = Vec::new();
    for key in ["channels", "groups"] {
        if let Some(arr) = raw.get(key).and_then(Value::as_array) {
            for cid in arr
                .iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(ChannelId::from)
            {
                if !shared_channels.contains(&cid) {
                    shared_channels.push(cid);
                }
            }
        }
    }
    let limits = raw.get("list_limits");
    let u32_limit = |key: &str| {
        limits
            .and_then(|l| l.get(key))
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
    };
    let row_count = u32_limit("row_count");
    let row_count_limit = u32_limit("row_count_limit");
    let archived_row_count = u32_limit("archived_row_count");
    let over_row_maximum = limits
        .and_then(|l| l.get("over_row_maximum"))
        .and_then(value_as_lenient_bool)
        .unwrap_or(false);
    let comment_channel_id = list_comment_channel_id(&file_id);
    ListMeta {
        file_id,
        title,
        permalink,
        editable,
        access,
        org_or_workspace_access,
        owner_id,
        editors,
        shared_channels,
        row_count,
        row_count_limit,
        archived_row_count,
        over_row_maximum,
        comment_channel_id,
    }
}

/// Translate Slack server-enforced List errors into clear user-facing guidance
/// (`F-COLLAB-246`, `F-COLLAB-254`, `F-COLLAB-274..279`).
pub fn format_list_error(err: &str) -> String {
    match err.trim() {
        "restricted_action" | "permission_denied" | "access_denied" => {
            "You do not have permission to perform this action on this list (owner or edit access may be required).".to_owned()
        }
        "list_not_found" => "List not found or you no longer have access to it.".to_owned(),
        "over_row_maximum" => {
            "This list has reached Slack's item limit. Archive or delete old items before adding more."
                .to_owned()
        }
        "paid_only" | "plan_upgrade_required" => {
            "Slack Lists require a paid Slack workspace plan.".to_owned()
        }
        "invalid_parameters" => "Invalid list sharing or request parameters.".to_owned(),
        "not_allowed_token_type" => {
            "This list action is not allowed with the current Slack session token.".to_owned()
        }
        "missing_scope" => {
            "Your Slack session is missing the required scope for this list action.".to_owned()
        }
        _ => err.to_owned(),
    }
}

/// Parse thread messages from `lists.getThreadsForRecord` across Slack's known shapes:
/// flat `messages: [...]`, flat `threads: [Message...]`, or nested
/// `threads: [{ root, latest_replies, unread_replies, messages, channel, thread_ts }]`.
pub fn parse_list_record_threads(data: &Value) -> Vec<Message> {
    let mut out = Vec::new();
    let mut seen: HashSet<Ts> = HashSet::new();
    let mut push_msg = |mut msg: Message, fallback_ch: Option<&ChannelId>| {
        if msg.channel.is_none() {
            msg.channel = fallback_ch.cloned();
        }
        if msg.ts.as_str().is_empty() {
            return;
        }
        if seen.insert(msg.ts.clone()) {
            out.push(msg);
        }
    };

    if let Some(msgs) = data.get("messages").and_then(Value::as_array) {
        for item in msgs {
            if let Ok(m) = serde_json::from_value::<Message>(item.clone()) {
                push_msg(m, None);
            }
        }
    }
    if let Some(threads) = data.get("threads").and_then(Value::as_array) {
        for entry in threads {
            let entry_ch = entry
                .get("channel")
                .or_else(|| entry.get("channel_id"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(ChannelId::from);
            let mut extracted_nested = false;
            if let Some(root) = entry.get("root")
                && let Ok(m) = serde_json::from_value::<Message>(root.clone())
            {
                push_msg(m, entry_ch.as_ref());
                extracted_nested = true;
            }
            for arr_key in ["messages", "latest_replies", "unread_replies", "replies"] {
                if let Some(arr) = entry.get(arr_key).and_then(Value::as_array) {
                    for sub in arr {
                        if let Ok(m) = serde_json::from_value::<Message>(sub.clone()) {
                            push_msg(m, entry_ch.as_ref());
                            extracted_nested = true;
                        }
                    }
                }
            }
            if !extracted_nested && let Ok(m) = serde_json::from_value::<Message>(entry.clone()) {
                push_msg(m, entry_ch.as_ref());
            }
        }
    }
    out.sort_by(|a, b| a.ts.cmp(&b.ts));
    out
}

/// One select choice option on a Slack List select column.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlackListChoice {
    pub value: String,
    pub label: String,
    pub color: String,
}

/// One column in a Slack List schema (`list_metadata.schema`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlackListColumn {
    pub id: String,
    pub key: String,
    pub name: String,
    pub kind: String,
    pub is_primary: bool,
    pub choices: Vec<SlackListChoice>,
}

/// Typed cell value inside a Slack List row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SlackListCellValue {
    #[default]
    Empty,
    Text(String),
    Users(Vec<UserId>),
    Select(Vec<String>),
    Date(String),
    Checkbox(bool),
    Number(String),
    Link {
        label: String,
        url: String,
    },
}

/// One record/row of a Slack List.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlackListRow {
    pub id: String,
    pub cells: HashMap<String, SlackListCellValue>,
}

impl SlackListRow {
    /// Looks up the cell for `column` by its `id`, falling back to its `key`.
    pub fn cell(&self, column: &SlackListColumn) -> Option<&SlackListCellValue> {
        self.cells
            .get(&column.id)
            .or_else(|| self.cells.get(&column.key))
    }
}

/// A parsed Slack List ready for native read-only table rendering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlackListData {
    pub file_id: String,
    pub title: String,
    pub description: String,
    pub columns: Vec<SlackListColumn>,
    pub rows: Vec<SlackListRow>,
}

/// Parse `list_metadata.schema` from `files.info` (or `slackLists.items.list`'s `list` object).
pub(in crate::methods) fn parse_list_schema(
    list_obj: &Value,
) -> (String, String, Vec<SlackListColumn>) {
    let title = list_obj
        .get("title")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| list_obj.get("name").and_then(Value::as_str))
        .unwrap_or("List")
        .to_owned();
    let meta = list_obj.get("list_metadata");
    let description = meta
        .and_then(|m| m.get("description"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut columns = Vec::new();
    if let Some(schema) = meta.and_then(|m| m.get("schema")).and_then(Value::as_array) {
        for col in schema {
            let id = col
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let key = col
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if id.is_empty() && key.is_empty() {
                continue;
            }
            let name = col
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| if !key.is_empty() { &key } else { &id })
                .to_owned();
            let kind = col
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("text")
                .to_owned();
            let is_primary = col
                .get("is_primary_column")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mut choices = Vec::new();
            if let Some(arr) = col
                .get("options")
                .and_then(|o| o.get("choices"))
                .and_then(Value::as_array)
            {
                for ch in arr {
                    let value = ch
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let label = ch
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or(&value)
                        .to_owned();
                    let color = ch
                        .get("color")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    if !value.is_empty() || !label.is_empty() {
                        choices.push(SlackListChoice {
                            value,
                            label,
                            color,
                        });
                    }
                }
            }
            columns.push(SlackListColumn {
                id: if id.is_empty() { key.clone() } else { id },
                key,
                name,
                kind,
                is_primary,
                choices,
            });
        }
    }
    (title, description, columns)
}

/// Extract plain text from a Slack List cell's `rich_text` JSON array when `text` is absent.
fn list_rich_text_plain(val: &Value) -> String {
    fn walk(v: &Value, out: &mut String) {
        match v {
            Value::Array(arr) => {
                for item in arr {
                    walk(item, out);
                }
            }
            Value::Object(map) => {
                if let Some(t) = map.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                } else if let Some(u) = map.get("url").and_then(Value::as_str) {
                    out.push_str(u);
                }
                if let Some(elems) = map.get("elements") {
                    walk(elems, out);
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    walk(val, &mut out);
    out.trim().to_owned()
}

/// Parse `records` (`lists.records.list`) or `items` (`slackLists.items.list`) into typed rows.
pub(in crate::methods) fn parse_list_records(records: &[Value]) -> Vec<SlackListRow> {
    let mut rows = Vec::new();
    for rec in records {
        if rec
            .get("archived")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        let id = rec
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut cells = HashMap::new();
        if let Some(fields) = rec.get("fields").and_then(Value::as_array) {
            for field in fields {
                let col_key = field
                    .get("column_id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .or_else(|| field.get("key").and_then(Value::as_str))
                    .unwrap_or_default();
                if col_key.is_empty() {
                    continue;
                }
                let cell = parse_list_cell(field);
                if cell != SlackListCellValue::Empty {
                    cells.insert(col_key.to_owned(), cell);
                }
            }
        }
        rows.push(SlackListRow { id, cells });
    }
    rows
}

pub(in crate::methods) fn parse_list_cell(field: &Value) -> SlackListCellValue {
    if let Some(b) = field.get("checkbox").and_then(Value::as_bool) {
        return SlackListCellValue::Checkbox(b);
    }
    if let Some(arr) = field.get("checkbox").and_then(Value::as_array)
        && let Some(b) = arr.first().and_then(Value::as_bool)
    {
        return SlackListCellValue::Checkbox(b);
    }
    if let Some(arr) = field.get("user").and_then(Value::as_array) {
        let users: Vec<UserId> = arr
            .iter()
            .filter_map(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(UserId::from)
            .collect();
        if !users.is_empty() {
            return SlackListCellValue::Users(users);
        }
    }
    if let Some(arr) = field.get("select").and_then(Value::as_array) {
        let opts: Vec<String> = arr
            .iter()
            .filter_map(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if !opts.is_empty() {
            return SlackListCellValue::Select(opts);
        }
    }
    if let Some(arr) = field.get("date").and_then(Value::as_array)
        && let Some(d) = arr
            .first()
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    {
        return SlackListCellValue::Date(d.to_owned());
    }
    if let Some(arr) = field.get("link").and_then(Value::as_array)
        && let Some(first) = arr.first()
    {
        let url = first
            .get("originalUrl")
            .or_else(|| first.get("original_url"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !url.is_empty() {
            let label = first
                .get("displayName")
                .or_else(|| first.get("display_name"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(url);
            return SlackListCellValue::Link {
                label: label.to_owned(),
                url: url.to_owned(),
            };
        }
    }
    if let Some(arr) = field.get("number").and_then(Value::as_array)
        && let Some(n) = arr.first()
    {
        let s = n
            .as_i64()
            .map(|i| i.to_string())
            .or_else(|| n.as_u64().map(|u| u.to_string()))
            .or_else(|| {
                n.as_f64().filter(|v| v.is_finite()).map(|v| {
                    if v.fract() == 0.0 && v >= (i64::MIN as f64) && v < (i64::MAX as f64) {
                        format!("{}", v as i64)
                    } else {
                        v.to_string()
                    }
                })
            })
            .or_else(|| n.as_str().map(str::to_owned));
        if let Some(s) = s {
            return SlackListCellValue::Number(s);
        }
    }
    if let Some(arr) = field.get("rating").and_then(Value::as_array)
        && let Some(n) = arr.first().and_then(Value::as_u64)
    {
        return SlackListCellValue::Text("\u{2605}".repeat((n as usize).min(10)));
    }
    if let Some(t) = field.get("text").and_then(Value::as_str) {
        let trimmed = t.trim();
        if !trimmed.is_empty() {
            return SlackListCellValue::Text(trimmed.to_owned());
        }
    }
    if let Some(rt) = field.get("rich_text") {
        let plain = list_rich_text_plain(rt);
        if !plain.is_empty() {
            return SlackListCellValue::Text(plain);
        }
    }
    for arr_key in ["email", "phone", "channel"] {
        if let Some(arr) = field.get(arr_key).and_then(Value::as_array) {
            let joined: Vec<&str> = arr.iter().filter_map(Value::as_str).collect();
            if !joined.is_empty() {
                return SlackListCellValue::Text(joined.join(", "));
            }
        }
    }
    if let Some(v) = field.get("value").and_then(Value::as_str) {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            return SlackListCellValue::Text(trimmed.to_owned());
        }
    }
    SlackListCellValue::Empty
}

/// Typed cell update payload for `lists.cells.update` / `lists.records.create` (`F-COLLAB-226`, `F-COLLAB-227`, `F-COLLAB-248`).
#[derive(Debug, Clone, PartialEq)]
pub struct ListCellInput {
    pub column_id: String,
    pub value: ListCellInputValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ListCellInputValue {
    Text(String),
    Checkbox(bool),
    Select(Vec<String>),
    Users(Vec<UserId>),
    Date(Vec<String>),
    Number(Vec<f64>),
    Link {
        url: String,
        display_name: Option<String>,
    },
    Clear,
}

impl ListCellInput {
    pub(in crate::methods) fn to_json(&self, row_id: Option<&str>) -> Value {
        let mut obj = serde_json::Map::new();
        if let Some(row_id) = row_id {
            obj.insert("row_id".to_owned(), Value::String(row_id.to_owned()));
        }
        obj.insert(
            "column_id".to_owned(),
            Value::String(self.column_id.clone()),
        );
        match &self.value {
            ListCellInputValue::Text(text) => {
                let elements = if text.starts_with("https://") || text.starts_with("http://") {
                    serde_json::json!([{ "type": "link", "url": text }])
                } else {
                    serde_json::json!([{ "type": "text", "text": text }])
                };
                obj.insert(
                    "rich_text".to_owned(),
                    serde_json::json!([{
                        "type": "rich_text",
                        "elements": [{
                            "type": "rich_text_section",
                            "elements": elements
                        }]
                    }]),
                );
            }
            ListCellInputValue::Checkbox(checked) => {
                obj.insert("checkbox".to_owned(), Value::Bool(*checked));
            }
            ListCellInputValue::Select(opts) => {
                obj.insert("select".to_owned(), serde_json::json!(opts));
            }
            ListCellInputValue::Users(users) => {
                let ids: Vec<String> = users.iter().map(ToString::to_string).collect();
                obj.insert("user".to_owned(), serde_json::json!(ids));
            }
            ListCellInputValue::Date(dates) => {
                obj.insert("date".to_owned(), serde_json::json!(dates));
            }
            ListCellInputValue::Number(nums) => {
                obj.insert("number".to_owned(), serde_json::json!(nums));
            }
            ListCellInputValue::Link { url, display_name } => {
                let mut item = serde_json::Map::new();
                item.insert("original_url".to_owned(), Value::String(url.clone()));
                item.insert(
                    "display_as_url".to_owned(),
                    Value::Bool(display_name.as_deref().is_none_or(str::is_empty)),
                );
                if let Some(name) = display_name.as_deref().filter(|s| !s.is_empty()) {
                    item.insert("display_name".to_owned(), Value::String(name.to_owned()));
                }
                obj.insert("link".to_owned(), Value::Array(vec![Value::Object(item)]));
            }
            ListCellInputValue::Clear => {
                obj.insert("rich_text".to_owned(), Value::Array(Vec::new()));
            }
        }
        Value::Object(obj)
    }
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct ListCreateWire {
    #[serde(default)]
    pub(in crate::methods) list: Option<Value>,
    #[serde(default)]
    pub(in crate::methods) list_id: Option<String>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct ListRecordCreateWire {
    #[serde(default)]
    pub(in crate::methods) record: Option<Value>,
    #[serde(default)]
    pub(in crate::methods) item: Option<Value>,
}
