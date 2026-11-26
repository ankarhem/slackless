//! Lightweight JSON-RPC 2.0 stdio Model Context Protocol (MCP) server (`slackless mcp`).
//!
//! Exposes offline SQLite archive queries (`list_workspaces`, `list_channels`,
//! `search_messages`, `read_channel`, `read_thread`, `get_transcript`) and live Slack write/read
//! actions (`send_message`, `reply_thread`, `add_reaction`, plus `live: true` on `read_channel`
//! and `read_thread`) with zero extra protocol dependencies.
//!
//! Stdout is reserved strictly for newline-delimited JSON-RPC 2.0 responses; no human logs or
//! prompts are ever written there.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use serde::Serialize;
use serde_json::{Map, Value, json};
use slackless_core::api::{HistoryOpts, MessageContent, SlackApi};
use slackless_core::auth::{Session, SessionStore};
use slackless_core::store::{SearchQuery, SearchSort, Store, StoredMessage};
use slackless_core::types::time::now_epoch_secs;
use slackless_core::types::{ChannelId, FileId, Message, TeamId, Ts};

use crate::CLI_VERSION;
use crate::archive::{self, Archive};
use crate::commands::{parse_since_cutoff, raw_channel_id};
use crate::live::session::{classify_api_error, resolve_channel_with};
use crate::output::MessageRow;
use crate::session::{resolve_session_from, resolve_workspace_from};
use crate::write::normalize_emoji_shortcode;

/// Supported MCP protocol versions, newest first.
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const DEFAULT_PROTOCOL_VERSION: &str = "2025-03-26";

const ERR_PARSE: i64 = -32700;
const ERR_INVALID_REQUEST: i64 = -32600;
const ERR_METHOD_NOT_FOUND: i64 = -32601;
const ERR_INVALID_PARAMS: i64 = -32602;

const DEFAULT_CHANNEL_LIST_LIMIT: usize = 200;
const MAX_CHANNEL_LIST_LIMIT: usize = 1000;
const DEFAULT_SEARCH_LIMIT: usize = 20;
const MAX_SEARCH_LIMIT: usize = 200;
const DEFAULT_READ_LIMIT: usize = 50;
const MAX_READ_LIMIT: usize = 200;
const DEFAULT_THREAD_LIMIT: usize = 200;
const MAX_THREAD_LIMIT: usize = 999;

/// Entrypoint for `slackless mcp`.
pub async fn run_mcp(default_workspace: Option<&str>) -> Result<ExitCode> {
    let validated_default = match default_workspace.map(str::trim).filter(|s| !s.is_empty()) {
        Some(ws) => Some(validate_workspace_id(ws).map_err(|msg| anyhow::anyhow!("{msg}"))?),
        None => None,
    };
    let mut server = McpServer::new(validated_default);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    server.serve_io(stdin.lock(), stdout.lock()).await?;
    Ok(ExitCode::SUCCESS)
}

/// Self-contained stdio MCP server state.
pub(crate) struct McpServer {
    default_workspace: Option<String>,
    initialized: bool,
    /// When `Some`, reads `<dir>/<TEAM_ID>.sqlite` instead of the default app data directory
    /// (used by tests to isolate from `$HOME`).
    archive_dir: Option<PathBuf>,
    /// When `Some`, replaces `SessionStore` lookups (used by tests to avoid macOS Keychain).
    test_sessions: Option<Vec<Session>>,
}

struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: ERR_INVALID_PARAMS,
            message: message.into(),
        }
    }
}

#[derive(Serialize)]
struct FileRef {
    id: String,
    name: Option<String>,
    title: Option<String>,
    filetype: Option<String>,
}

#[derive(Serialize)]
struct McpMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel_name: Option<String>,
    #[serde(flatten)]
    row: MessageRow,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    file_refs: Vec<FileRef>,
}

impl McpMessage {
    fn from_stored(
        stored: &StoredMessage,
        user_names: &HashMap<slackless_core::types::UserId, String>,
        channel_name: Option<String>,
        workspace: Option<String>,
    ) -> Self {
        let file_refs = stored
            .message
            .files
            .iter()
            .filter(|f| !f.id.as_str().is_empty())
            .map(|f| FileRef {
                id: f.id.as_str().to_owned(),
                name: f.name.clone(),
                title: f.title.clone(),
                filetype: f.filetype.clone(),
            })
            .collect();
        Self {
            workspace,
            channel_name,
            row: MessageRow::from_stored(stored, user_names),
            file_refs,
        }
    }
}

#[derive(Serialize)]
struct WorkspaceEntry {
    team_id: String,
    workspace_url: Option<String>,
    user_id: Option<String>,
    signed_in: bool,
    has_secrets: bool,
    archive_path: String,
    archive_exists: bool,
    messages: u64,
}

#[derive(Serialize)]
struct ChannelEntry {
    id: String,
    name: Option<String>,
    label: String,
    kind: String,
    is_archived: bool,
    is_member: bool,
    topic: Option<String>,
    user: Option<String>,
    user_name: Option<String>,
}

impl McpServer {
    pub(crate) fn new(default_workspace: Option<String>) -> Self {
        Self {
            default_workspace,
            initialized: false,
            archive_dir: None,
            test_sessions: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        archive_dir: PathBuf,
        sessions: Vec<Session>,
        default_workspace: Option<String>,
    ) -> Self {
        Self {
            default_workspace,
            initialized: false,
            archive_dir: Some(archive_dir),
            test_sessions: Some(sessions),
        }
    }

    /// Run the newline-delimited JSON-RPC 2.0 request/response loop until EOF.
    pub(crate) async fn serve_io<R: BufRead, W: Write>(
        &mut self,
        mut reader: R,
        mut writer: W,
    ) -> Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = reader.read_line(&mut line)?;
            if bytes == 0 {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(response) = self.handle_line(trimmed).await {
                serde_json::to_writer(&mut writer, &response)?;
                writer.write_all(b"\n")?;
                writer.flush()?;
            }
        }
        Ok(())
    }

    /// Parse and handle a single JSON-RPC 2.0 line. Returns `None` for notifications.
    pub(crate) async fn handle_line(&mut self, line: &str) -> Option<Value> {
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(err) => {
                return Some(jsonrpc_error(
                    Value::Null,
                    ERR_PARSE,
                    format!("parse error: {err}"),
                ));
            }
        };
        self.handle_value(value).await
    }

    async fn handle_value(&mut self, value: Value) -> Option<Value> {
        let Some(obj) = value.as_object() else {
            return Some(jsonrpc_error(
                Value::Null,
                ERR_INVALID_REQUEST,
                "invalid request: expected JSON-RPC 2.0 object",
            ));
        };

        let has_id = obj.contains_key("id");
        let id = obj.get("id").cloned();
        if let Some(ref id_val) = id
            && !matches!(id_val, Value::String(_) | Value::Number(_) | Value::Null)
        {
            return Some(jsonrpc_error(
                Value::Null,
                ERR_INVALID_REQUEST,
                "invalid request: id must be a string, number, or null",
            ));
        }

        if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(jsonrpc_error(
                id.unwrap_or(Value::Null),
                ERR_INVALID_REQUEST,
                "invalid request: jsonrpc must be \"2.0\"",
            ));
        }

        let Some(method) = obj
            .get("method")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
        else {
            return Some(jsonrpc_error(
                id.unwrap_or(Value::Null),
                ERR_INVALID_REQUEST,
                "invalid request: method must be a non-empty string",
            ));
        };

        let params = match obj.get("params") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(map)) => map.clone(),
            Some(_) => {
                if !has_id {
                    return None;
                }
                return Some(jsonrpc_error(
                    id.unwrap_or(Value::Null),
                    ERR_INVALID_PARAMS,
                    "invalid params: expected JSON object",
                ));
            }
        };

        // Notifications (no `id` key) must never receive a response.
        if !has_id {
            if method == "notifications/initialized" {
                self.initialized = true;
            }
            return None;
        }

        let req_id = id.unwrap_or(Value::Null);
        match method {
            "initialize" => Some(jsonrpc_ok(req_id, handle_initialize(&params))),
            "notifications/initialized" => {
                self.initialized = true;
                Some(jsonrpc_ok(req_id, json!({})))
            }
            "ping" => Some(jsonrpc_ok(req_id, json!({}))),
            "tools/list" => Some(jsonrpc_ok(req_id, json!({ "tools": tool_definitions() }))),
            "tools/call" => match self.handle_tools_call(&params).await {
                Ok(result) => Some(jsonrpc_ok(req_id, result)),
                Err(err) => Some(jsonrpc_error(req_id, err.code, err.message)),
            },
            _ => Some(jsonrpc_error(
                req_id,
                ERR_METHOD_NOT_FOUND,
                format!("method not found: {method}"),
            )),
        }
    }

    async fn handle_tools_call(
        &self,
        params: &Map<String, Value>,
    ) -> std::result::Result<Value, RpcError> {
        let Some(name) = params
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            return Err(RpcError::invalid_params(
                "tools/call requires a non-empty string `name`",
            ));
        };
        let args = match params.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(map)) => map.clone(),
            Some(_) => {
                return Err(RpcError::invalid_params(
                    "tools/call `arguments` must be a JSON object",
                ));
            }
        };

        match name {
            "list_workspaces" => Ok(self.tool_list_workspaces()),
            "list_channels" => Ok(self.tool_list_channels(&args)),
            "search_messages" => Ok(self.tool_search_messages(&args)),
            "read_channel" => Ok(self.tool_read_channel(&args).await),
            "read_thread" => Ok(self.tool_read_thread(&args).await),
            "get_transcript" => Ok(self.tool_get_transcript(&args)),
            "send_message" => Ok(self.tool_send_message(&args).await),
            "reply_thread" => Ok(self.tool_reply_thread(&args).await),
            "add_reaction" => Ok(self.tool_add_reaction(&args).await),
            _ => Err(RpcError::invalid_params(format!("unknown tool: {name}"))),
        }
    }

    // -------------------------------------------------------------------------
    // Storage & session helpers
    // -------------------------------------------------------------------------

    fn load_sessions(&self) -> Vec<Session> {
        if let Some(ref sessions) = self.test_sessions {
            return sessions.clone();
        }
        SessionStore::new()
            .and_then(|store| store.load())
            .unwrap_or_default()
    }

    fn archive_path_for(&self, team_id: &TeamId) -> Option<PathBuf> {
        if !is_valid_workspace_id(team_id.as_str()) {
            return None;
        }
        match &self.archive_dir {
            Some(dir) => Some(dir.join(format!("{}.sqlite", team_id.as_str()))),
            None => archive::store_path(team_id),
        }
    }

    fn stored_team_ids(&self) -> Vec<TeamId> {
        match &self.archive_dir {
            Some(dir) => archive::stored_team_ids_in(dir),
            None => archive::stored_team_ids(),
        }
    }

    fn all_workspace_ids(&self) -> Vec<TeamId> {
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        for s in self.load_sessions() {
            if is_valid_workspace_id(s.team_id.as_str()) && seen.insert(s.team_id.clone()) {
                ids.push(s.team_id);
            }
        }
        for tid in self.stored_team_ids() {
            if is_valid_workspace_id(tid.as_str()) && seen.insert(tid.clone()) {
                ids.push(tid);
            }
        }
        ids
    }

    fn effective_workspace_flag<'a>(
        &'a self,
        explicit: Option<&'a str>,
    ) -> std::result::Result<Option<&'a str>, String> {
        if let Some(ws) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
            validate_workspace_id(ws)?;
            return Ok(Some(ws));
        }
        if let Some(ws) = self
            .default_workspace
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            validate_workspace_id(ws)?;
            return Ok(Some(ws));
        }
        Ok(None)
    }

    fn resolve_workspace(
        &self,
        explicit: Option<&str>,
    ) -> std::result::Result<Option<TeamId>, String> {
        let flag = self.effective_workspace_flag(explicit)?;
        let sessions = self.load_sessions();
        let archive_ids = self.stored_team_ids();
        Ok(resolve_workspace_from(flag, &sessions, &archive_ids))
    }

    fn open_archive(&self, team_id: &TeamId) -> std::result::Result<Option<Archive>, String> {
        let Some(path) = self.archive_path_for(team_id) else {
            return Ok(None);
        };
        Archive::open_path(&path).map_err(|err| format!("opening archive for {team_id}: {err}"))
    }

    fn open_archive_rw(&self, team_id: &TeamId) -> std::result::Result<Archive, String> {
        let path = self
            .archive_path_for(team_id)
            .ok_or_else(|| format!("could not resolve local archive path for {team_id}"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("creating archive directory {}: {err}", parent.display()))?;
        }
        let store = Store::open(&path)
            .map_err(|err| format!("opening local archive {}: {err}", path.display()))?;
        drop(store);
        Archive::open_rw_path(&path)
            .map_err(|err| format!("opening archive rw for {team_id}: {err}"))?
            .ok_or_else(|| format!("archive file missing after creation: {}", path.display()))
    }

    fn resolve_live_api(
        &self,
        explicit_workspace: Option<&str>,
    ) -> std::result::Result<(SlackApi, TeamId), (&'static str, String)> {
        let flag = self
            .effective_workspace_flag(explicit_workspace)
            .map_err(|msg| ("invalid_args", msg))?;
        let sessions = self.load_sessions();
        let session = resolve_session_from(flag, &sessions)
            .map_err(|not_signed_in| ("not_signed_in", not_signed_in.to_string()))?;
        let api = SlackApi::new(
            &session.api_base(),
            session.xoxc_token.clone(),
            &session.d_cookie,
        )
        .map_err(|err| ("client_error", format!("building the Slack client: {err}")))?;
        Ok((api, session.team_id))
    }

    fn record_echo(&self, workspace: &TeamId, channel: &ChannelId, message: Option<&Message>) {
        let Some(message) = message else {
            return;
        };
        if let Ok(mut archive) = self.open_archive_rw(workspace) {
            let _ = archive
                .store_mut()
                .upsert_message(workspace, channel, message);
        }
    }

    // -------------------------------------------------------------------------
    // Tool implementations
    // -------------------------------------------------------------------------

    fn tool_list_workspaces(&self) -> Value {
        let sessions = self.load_sessions();
        let mut by_id: HashMap<TeamId, Session> = HashMap::new();
        for s in sessions {
            by_id.entry(s.team_id.clone()).or_insert(s);
        }

        let ids = self.all_workspace_ids();
        let default_workspace = self
            .resolve_workspace(None)
            .ok()
            .flatten()
            .map(|t| t.as_str().to_owned());

        let workspaces: Vec<WorkspaceEntry> = ids
            .into_iter()
            .map(|team_id| {
                let path = self.archive_path_for(&team_id);
                let archive_exists = path.as_deref().is_some_and(Path::is_file);
                let messages = if archive_exists {
                    self.open_archive(&team_id)
                        .ok()
                        .flatten()
                        .and_then(|a| a.message_count(&team_id).ok())
                        .unwrap_or(0)
                } else {
                    0
                };
                let archive_path = path.map_or_else(
                    || "(no home directory)".to_owned(),
                    |p| p.display().to_string(),
                );
                match by_id.get(&team_id) {
                    Some(session) => {
                        let has_secrets = session.has_secrets();
                        WorkspaceEntry {
                            team_id: team_id.as_str().to_owned(),
                            workspace_url: Some(session.workspace_url.clone()),
                            user_id: Some(session.user_id.as_str().to_owned()),
                            signed_in: has_secrets,
                            has_secrets,
                            archive_path,
                            archive_exists,
                            messages,
                        }
                    }
                    None => WorkspaceEntry {
                        team_id: team_id.as_str().to_owned(),
                        workspace_url: None,
                        user_id: None,
                        signed_in: false,
                        has_secrets: false,
                        archive_path,
                        archive_exists,
                        messages,
                    },
                }
            })
            .collect();

        tool_ok(&json!({
            "default_workspace": default_workspace,
            "count": workspaces.len(),
            "workspaces": workspaces,
        }))
    }

    fn tool_list_channels(&self, args: &Map<String, Value>) -> Value {
        let workspace_arg = match opt_workspace(args) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let kind = match opt_str(args, "kind") {
            Ok(Some(k)) => k.trim().to_ascii_lowercase(),
            Ok(None) => "all".to_owned(),
            Err(msg) => return tool_err("invalid_args", msg),
        };
        if !matches!(kind.as_str(), "all" | "channels" | "dms") {
            return tool_err(
                "invalid_args",
                format!("invalid `kind` '{kind}': expected \"all\", \"channels\", or \"dms\""),
            );
        }
        let filter = match opt_str(args, "filter") {
            Ok(v) => v.map(|s| s.trim().trim_start_matches(['#', '@']).to_ascii_lowercase()),
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let include_archived = match opt_bool(args, "include_archived", true) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let limit = match opt_usize(
            args,
            "limit",
            DEFAULT_CHANNEL_LIST_LIMIT,
            1,
            MAX_CHANNEL_LIST_LIMIT,
        ) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };

        let workspace = match self.resolve_workspace(workspace_arg.as_deref()) {
            Ok(Some(ws)) => ws,
            Ok(None) => {
                return tool_err(
                    "no_workspace",
                    "no workspace resolved; pass `workspace` (call `list_workspaces` to see available team IDs)",
                );
            }
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let archive = match self.open_archive(&workspace) {
            Ok(a) => a,
            Err(msg) => return tool_err("store_error", msg),
        };
        let Some(archive) = archive else {
            return tool_ok(&json!({
                "workspace": workspace.as_str(),
                "kind": kind,
                "archive_exists": false,
                "count": 0,
                "channels": [],
            }));
        };

        let user_names = match archive.user_display_map(&workspace) {
            Ok(map) => map,
            Err(err) => return tool_err("store_error", format!("loading users: {err}")),
        };

        let mut rows = Vec::new();
        if matches!(kind.as_str(), "all" | "channels") {
            match archive.channels(&workspace) {
                Ok(ch) => rows.extend(ch),
                Err(err) => return tool_err("store_error", format!("listing channels: {err}")),
            }
        }
        if matches!(kind.as_str(), "all" | "dms") {
            match archive.dms(&workspace) {
                Ok(dms) => rows.extend(dms),
                Err(err) => return tool_err("store_error", format!("listing DMs: {err}")),
            }
        }

        let needle = filter.as_deref().filter(|s| !s.is_empty());
        let mut entries: Vec<ChannelEntry> = Vec::new();
        for row in &rows {
            if !include_archived && row.is_archived {
                continue;
            }
            let user_name = row.user.as_ref().and_then(|u| user_names.get(u).cloned());
            let label = if row.kind == "im" {
                user_name
                    .as_deref()
                    .map(|u| format!("@{u}"))
                    .unwrap_or_else(|| row.label())
            } else {
                row.label()
            };
            if let Some(q) = needle {
                let matches_field = |s: &str| s.to_ascii_lowercase().contains(q);
                let hit = matches_field(row.id.as_str())
                    || matches_field(&label)
                    || row.name.as_deref().is_some_and(matches_field)
                    || row.topic.as_deref().is_some_and(matches_field)
                    || row.user.as_ref().is_some_and(|u| matches_field(u.as_str()))
                    || user_name.as_deref().is_some_and(matches_field);
                if !hit {
                    continue;
                }
            }
            entries.push(ChannelEntry {
                id: row.id.as_str().to_owned(),
                name: row.name.clone(),
                label,
                kind: row.kind.clone(),
                is_archived: row.is_archived,
                is_member: row.is_member,
                topic: row.topic.clone(),
                user: row.user.as_ref().map(|u| u.as_str().to_owned()),
                user_name,
            });
        }
        let total_matched = entries.len();
        entries.truncate(limit);

        tool_ok(&json!({
            "workspace": workspace.as_str(),
            "kind": kind,
            "count": entries.len(),
            "total_matched": total_matched,
            "channels": entries,
        }))
    }

    fn tool_search_messages(&self, args: &Map<String, Value>) -> Value {
        let query = match req_str(args, "query") {
            Ok(q) => q,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let workspace_arg = match opt_workspace(args) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let all_workspaces_arg = match opt_bool_optional(args, "all_workspaces") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        if workspace_arg.is_some() && all_workspaces_arg == Some(true) {
            return tool_err(
                "invalid_args",
                "cannot specify both `workspace` and `all_workspaces: true`",
            );
        }
        let sort_str = match opt_str(args, "sort") {
            Ok(Some(s)) => s.trim().to_ascii_lowercase(),
            Ok(None) => "recent".to_owned(),
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let sort = match sort_str.as_str() {
            "recent" => SearchSort::Recent,
            "relevant" => SearchSort::Relevant,
            other => {
                return tool_err(
                    "invalid_args",
                    format!("invalid `sort` '{other}': expected \"recent\" or \"relevant\""),
                );
            }
        };
        let limit = match opt_usize(args, "limit", DEFAULT_SEARCH_LIMIT, 1, MAX_SEARCH_LIMIT) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let include_deleted = match opt_bool(args, "include_deleted", false) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };

        let mut parsed = SearchQuery::parse(&query);
        parsed.filters.include_deleted = include_deleted;

        let search_all = match (workspace_arg.as_deref(), all_workspaces_arg) {
            (Some(_), _) => false,
            (None, Some(true)) => true,
            (None, Some(false)) => false,
            (None, None) => match self.resolve_workspace(None) {
                Ok(ws) => ws.is_none(),
                Err(msg) => return tool_err("invalid_args", msg),
            },
        };

        if search_all {
            let team_ids = self.all_workspace_ids();
            let mut per_workspace: Vec<Vec<(i64, String, McpMessage)>> = Vec::new();
            for ws in &team_ids {
                let archive = match self.open_archive(ws) {
                    Ok(Some(a)) => a,
                    Ok(None) => continue,
                    Err(msg) => return tool_err("store_error", msg),
                };
                let messages = match archive.store().search_sorted(ws, &parsed, sort, limit) {
                    Ok(m) => m,
                    Err(err) => {
                        return tool_err(
                            "store_error",
                            format!("searching archive of {ws}: {err}"),
                        );
                    }
                };
                if messages.is_empty() {
                    continue;
                }
                let names = archive.channel_names(ws).unwrap_or_default();
                let user_names = archive.user_display_map(ws).unwrap_or_default();
                let mut ws_hits = Vec::with_capacity(messages.len());
                for msg in &messages {
                    let ch_name = names.get(msg.channel.as_str()).cloned();
                    let ts_secs = msg.message.ts.secs().unwrap_or(0);
                    let ts_raw = msg.message.ts.as_str().to_owned();
                    let mcp_msg = McpMessage::from_stored(
                        msg,
                        &user_names,
                        ch_name,
                        Some(ws.as_str().to_owned()),
                    );
                    ws_hits.push((ts_secs, ts_raw, mcp_msg));
                }
                per_workspace.push(ws_hits);
            }
            let messages: Vec<McpMessage> = match sort {
                SearchSort::Recent => {
                    let mut all_hits: Vec<(i64, String, McpMessage)> =
                        per_workspace.into_iter().flatten().collect();
                    all_hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
                    all_hits.truncate(limit);
                    all_hits.into_iter().map(|(_, _, m)| m).collect()
                }
                SearchSort::Relevant => {
                    let lists: Vec<Vec<McpMessage>> = per_workspace
                        .into_iter()
                        .map(|hits| hits.into_iter().map(|(_, _, m)| m).collect())
                        .collect();
                    interleave_round_robin(lists, limit)
                }
            };
            let searched: Vec<&str> = team_ids.iter().map(TeamId::as_str).collect();
            return tool_ok(&json!({
                "query": query,
                "all_workspaces": true,
                "workspaces_searched": searched,
                "sort": sort_str,
                "count": messages.len(),
                "messages": messages,
            }));
        }

        let workspace = match self.resolve_workspace(workspace_arg.as_deref()) {
            Ok(Some(ws)) => ws,
            Ok(None) => {
                return tool_err(
                    "no_workspace",
                    "no workspace resolved; pass `workspace` or set `all_workspaces: true`",
                );
            }
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let archive = match self.open_archive(&workspace) {
            Ok(a) => a,
            Err(msg) => return tool_err("store_error", msg),
        };
        let messages = match archive.as_ref() {
            Some(archive) => {
                let raw = match archive
                    .store()
                    .search_sorted(&workspace, &parsed, sort, limit)
                {
                    Ok(m) => m,
                    Err(err) => {
                        return tool_err(
                            "store_error",
                            format!("searching archive of {workspace}: {err}"),
                        );
                    }
                };
                let names = archive.channel_names(&workspace).unwrap_or_default();
                let user_names = archive.user_display_map(&workspace).unwrap_or_default();
                raw.iter()
                    .map(|msg| {
                        let ch_name = names.get(msg.channel.as_str()).cloned();
                        McpMessage::from_stored(msg, &user_names, ch_name, None)
                    })
                    .collect::<Vec<_>>()
            }
            None => Vec::new(),
        };

        tool_ok(&json!({
            "query": query,
            "workspace": workspace.as_str(),
            "all_workspaces": false,
            "sort": sort_str,
            "count": messages.len(),
            "messages": messages,
        }))
    }

    async fn tool_read_channel(&self, args: &Map<String, Value>) -> Value {
        let channel_arg = match req_str(args, "channel") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let workspace_arg = match opt_workspace(args) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let limit = match opt_usize(args, "limit", DEFAULT_READ_LIMIT, 1, MAX_READ_LIMIT) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let since = match opt_str(args, "since") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let cutoff_secs = match since.as_deref() {
            Some(spec) => match parse_since_cutoff(spec, now_epoch_secs()) {
                Ok(secs) => Some(secs),
                Err(msg) => return tool_err("invalid_args", msg),
            },
            None => None,
        };
        let before = match opt_str(args, "before") {
            Ok(v) => v.filter(|s| !s.trim().is_empty()).map(Ts::from),
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let include_deleted = match opt_bool(args, "include_deleted", false) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let live = match opt_bool(args, "live", false) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };

        if live {
            let (api, workspace) = match self.resolve_live_api(workspace_arg.as_deref()) {
                Ok(pair) => pair,
                Err((code, msg)) => return tool_err(code, msg),
            };
            let mut archive = self.open_archive_rw(&workspace).ok();
            let (channel_id, label) =
                resolve_channel_with(archive.as_ref(), &workspace, &channel_arg);
            let opts = HistoryOpts {
                oldest: cutoff_secs.map(|s| Ts::from(format!("{s}.000000"))),
                latest: before,
                inclusive: false,
                limit: Some(u32::try_from(limit).unwrap_or(200)),
            };
            let page = match api.conversations_history(&channel_id, &opts, None).await {
                Ok(page) => page,
                Err(err) => {
                    let (code, msg) = classify_api_error(&err);
                    return tool_err(code, msg);
                }
            };
            if let Some(archive) = archive.as_mut() {
                let _ = archive
                    .store_mut()
                    .upsert_messages(&workspace, &channel_id, &page.items);
            }
            let user_names = archive
                .as_ref()
                .and_then(|a| a.user_display_map(&workspace).ok())
                .unwrap_or_default();
            // Slack returns newest-first; reverse so output is oldest-first like local read.
            let messages: Vec<McpMessage> = page
                .items
                .into_iter()
                .rev()
                .map(|message| {
                    let stored = StoredMessage {
                        channel: channel_id.clone(),
                        message,
                        deleted: false,
                    };
                    McpMessage::from_stored(&stored, &user_names, label.clone(), None)
                })
                .collect();
            return tool_ok(&json!({
                "workspace": workspace.as_str(),
                "channel": { "id": channel_id.as_str(), "label": label },
                "live": true,
                "count": messages.len(),
                "messages": messages,
            }));
        }

        let workspace = match self.resolve_workspace(workspace_arg.as_deref()) {
            Ok(Some(ws)) => ws,
            Ok(None) => {
                return tool_err(
                    "no_workspace",
                    "no workspace resolved; pass `workspace` (call `list_workspaces` to see available team IDs)",
                );
            }
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let archive = match self.open_archive(&workspace) {
            Ok(a) => a,
            Err(msg) => return tool_err("store_error", msg),
        };
        let (resolved, channel_id, stored_messages, user_names) = match archive.as_ref() {
            Some(archive) => {
                let resolved = match archive.resolve_channel(&workspace, &channel_arg) {
                    Ok(r) => r,
                    Err(err) => {
                        return tool_err("store_error", format!("resolving channel: {err}"));
                    }
                };
                let channel_id = resolved
                    .as_ref()
                    .map_or_else(|| raw_channel_id(&channel_arg), |row| row.id.clone());
                let msgs = match read_channel_window(
                    archive.store(),
                    &workspace,
                    &channel_id,
                    before.as_ref(),
                    limit,
                    cutoff_secs,
                    include_deleted,
                ) {
                    Ok(m) => m,
                    Err(err) => return tool_err("store_error", format!("reading channel: {err}")),
                };
                let user_names = archive.user_display_map(&workspace).unwrap_or_default();
                (resolved, channel_id, msgs, user_names)
            }
            None => (
                None,
                raw_channel_id(&channel_arg),
                Vec::new(),
                HashMap::new(),
            ),
        };

        let channel_row = match resolved {
            Some(row) => Some(row),
            None => archive
                .as_ref()
                .and_then(|a| a.conversation_by_id(&workspace, &channel_id).ok().flatten()),
        };
        let ch_label = channel_row.as_ref().map(|r| r.label());
        let messages: Vec<McpMessage> = stored_messages
            .iter()
            .map(|msg| McpMessage::from_stored(msg, &user_names, ch_label.clone(), None))
            .collect();
        let channel_json = match channel_row {
            Some(row) => json!({
                "id": channel_id.as_str(),
                "name": row.name,
                "kind": row.kind,
                "user": row.user.as_ref().map(|u| u.as_str()),
            }),
            None => json!({ "id": channel_id.as_str() }),
        };

        tool_ok(&json!({
            "workspace": workspace.as_str(),
            "channel": channel_json,
            "live": false,
            "count": messages.len(),
            "messages": messages,
        }))
    }

    async fn tool_read_thread(&self, args: &Map<String, Value>) -> Value {
        let channel_arg = match req_str(args, "channel") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let thread_ts_arg = match req_str(args, "thread_ts") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let workspace_arg = match opt_workspace(args) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let live = match opt_bool(args, "live", false) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let limit = match opt_usize(args, "limit", DEFAULT_THREAD_LIMIT, 1, MAX_THREAD_LIMIT) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };

        let thread_ts = Ts::from(thread_ts_arg);

        if live {
            let (api, workspace) = match self.resolve_live_api(workspace_arg.as_deref()) {
                Ok(pair) => pair,
                Err((code, msg)) => return tool_err(code, msg),
            };
            let mut archive = self.open_archive_rw(&workspace).ok();
            let (channel_id, label) =
                resolve_channel_with(archive.as_ref(), &workspace, &channel_arg);
            let opts = HistoryOpts {
                limit: Some(u32::try_from(limit).unwrap_or(200)),
                ..HistoryOpts::default()
            };
            let page = match api
                .conversations_replies(&channel_id, &thread_ts, &opts, None)
                .await
            {
                Ok(page) => page,
                Err(err) => {
                    let (code, msg) = classify_api_error(&err);
                    return tool_err(code, msg);
                }
            };
            if let Some(archive) = archive.as_mut() {
                let _ = archive
                    .store_mut()
                    .upsert_messages(&workspace, &channel_id, &page.items);
            }
            let user_names = archive
                .as_ref()
                .and_then(|a| a.user_display_map(&workspace).ok())
                .unwrap_or_default();
            let messages: Vec<McpMessage> = page
                .items
                .into_iter()
                .map(|message| {
                    let stored = StoredMessage {
                        channel: channel_id.clone(),
                        message,
                        deleted: false,
                    };
                    McpMessage::from_stored(&stored, &user_names, label.clone(), None)
                })
                .collect();
            return tool_ok(&json!({
                "workspace": workspace.as_str(),
                "channel": channel_id.as_str(),
                "thread_ts": thread_ts.as_str(),
                "live": true,
                "count": messages.len(),
                "messages": messages,
            }));
        }

        let workspace = match self.resolve_workspace(workspace_arg.as_deref()) {
            Ok(Some(ws)) => ws,
            Ok(None) => {
                return tool_err(
                    "no_workspace",
                    "no workspace resolved; pass `workspace` (call `list_workspaces` to see available team IDs)",
                );
            }
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let archive = match self.open_archive(&workspace) {
            Ok(a) => a,
            Err(msg) => return tool_err("store_error", msg),
        };
        let (label, channel_id, mut stored_messages, user_names) = match archive.as_ref() {
            Some(archive) => {
                let resolved = match archive.resolve_channel(&workspace, &channel_arg) {
                    Ok(r) => r,
                    Err(err) => {
                        return tool_err("store_error", format!("resolving channel: {err}"));
                    }
                };
                let channel_id = resolved
                    .as_ref()
                    .map_or_else(|| raw_channel_id(&channel_arg), |row| row.id.clone());
                let msgs = match archive
                    .store()
                    .thread_replies(&workspace, &channel_id, &thread_ts)
                {
                    Ok(m) => m,
                    Err(err) => return tool_err("store_error", format!("reading thread: {err}")),
                };
                let user_names = archive.user_display_map(&workspace).unwrap_or_default();
                (resolved.map(|r| r.label()), channel_id, msgs, user_names)
            }
            None => (
                None,
                raw_channel_id(&channel_arg),
                Vec::new(),
                HashMap::new(),
            ),
        };
        stored_messages.truncate(limit);
        let messages: Vec<McpMessage> = stored_messages
            .iter()
            .map(|msg| McpMessage::from_stored(msg, &user_names, label.clone(), None))
            .collect();

        tool_ok(&json!({
            "workspace": workspace.as_str(),
            "channel": channel_id.as_str(),
            "thread_ts": thread_ts.as_str(),
            "live": false,
            "count": messages.len(),
            "messages": messages,
        }))
    }

    fn tool_get_transcript(&self, args: &Map<String, Value>) -> Value {
        let file_id = match req_str(args, "file_id") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let workspace_arg = match opt_workspace(args) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };

        let candidates: Vec<TeamId> = if let Some(ws) = workspace_arg {
            vec![TeamId::from(ws)]
        } else {
            let mut list = Vec::new();
            let mut seen = HashSet::new();
            let default_ws = match self.resolve_workspace(None) {
                Ok(ws) => ws,
                Err(msg) => return tool_err("invalid_args", msg),
            };
            if let Some(default_ws) = default_ws
                && seen.insert(default_ws.clone())
            {
                list.push(default_ws);
            }
            for ws in self.all_workspace_ids() {
                if seen.insert(ws.clone()) {
                    list.push(ws);
                }
            }
            list
        };

        if candidates.is_empty() {
            return tool_err(
                "no_workspace",
                "no local archive workspaces found; pass `workspace` or sync the desktop app first",
            );
        }

        let fid = FileId::from(file_id.as_str());
        for ws in &candidates {
            let archive = match self.open_archive(ws) {
                Ok(Some(a)) => a,
                Ok(None) => continue,
                Err(msg) => return tool_err("store_error", msg),
            };
            match archive.store().file_transcript(ws, &fid) {
                Ok(Some(transcript)) => {
                    return tool_ok(&json!({
                        "workspace": ws.as_str(),
                        "file_id": transcript.file_id.as_str(),
                        "source": transcript.source,
                        "text": transcript.text,
                        "updated_at": transcript.updated_at,
                    }));
                }
                Ok(None) => continue,
                Err(err) => {
                    return tool_err("store_error", format!("reading file transcript: {err}"));
                }
            }
        }

        tool_err(
            "transcript_not_found",
            format!("no stored transcript found for file_id '{file_id}'"),
        )
    }

    async fn tool_send_message(&self, args: &Map<String, Value>) -> Value {
        let channel_arg = match req_str(args, "channel") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let text = match req_str(args, "text") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let workspace_arg = match opt_workspace(args) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let allow_secrets = match opt_bool(args, "allow_secrets", false) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };

        if let Some(err) = check_secrets_mcp(&text, allow_secrets) {
            return err;
        }
        let (api, workspace) = match self.resolve_live_api(workspace_arg.as_deref()) {
            Ok(pair) => pair,
            Err((code, msg)) => return tool_err(code, msg),
        };
        let archive = self.open_archive(&workspace).ok().flatten();
        let (channel_id, _label) = resolve_channel_with(archive.as_ref(), &workspace, &channel_arg);

        let posted = match api
            .chat_post_message(&channel_id, &MessageContent::plain(text), None, false)
            .await
        {
            Ok(p) => p,
            Err(err) => {
                let (code, msg) = classify_api_error(&err);
                return tool_err(code, msg);
            }
        };
        self.record_echo(&workspace, &channel_id, posted.message.as_ref());

        tool_ok(&json!({
            "ok": true,
            "workspace": workspace.as_str(),
            "channel": channel_id.as_str(),
            "ts": posted.ts.as_str(),
        }))
    }

    async fn tool_reply_thread(&self, args: &Map<String, Value>) -> Value {
        let channel_arg = match req_str(args, "channel") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let thread_ts_arg = match req_str(args, "thread_ts") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let text = match req_str(args, "text") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let reply_broadcast = match opt_bool(args, "reply_broadcast", false) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let workspace_arg = match opt_workspace(args) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let allow_secrets = match opt_bool(args, "allow_secrets", false) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };

        if let Some(err) = check_secrets_mcp(&text, allow_secrets) {
            return err;
        }
        let (api, workspace) = match self.resolve_live_api(workspace_arg.as_deref()) {
            Ok(pair) => pair,
            Err((code, msg)) => return tool_err(code, msg),
        };
        let archive = self.open_archive(&workspace).ok().flatten();
        let (channel_id, _label) = resolve_channel_with(archive.as_ref(), &workspace, &channel_arg);
        let thread_ts = Ts::from(thread_ts_arg);

        let posted = match api
            .chat_post_message(
                &channel_id,
                &MessageContent::plain(text),
                Some(&thread_ts),
                reply_broadcast,
            )
            .await
        {
            Ok(p) => p,
            Err(err) => {
                let (code, msg) = classify_api_error(&err);
                return tool_err(code, msg);
            }
        };
        self.record_echo(&workspace, &channel_id, posted.message.as_ref());

        tool_ok(&json!({
            "ok": true,
            "workspace": workspace.as_str(),
            "channel": channel_id.as_str(),
            "thread_ts": thread_ts.as_str(),
            "ts": posted.ts.as_str(),
        }))
    }

    async fn tool_add_reaction(&self, args: &Map<String, Value>) -> Value {
        let channel_arg = match req_str(args, "channel") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let ts_arg = match req_str(args, "ts") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let emoji_arg = match req_str(args, "emoji") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let workspace_arg = match opt_workspace(args) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };

        let name = normalize_emoji_shortcode(&emoji_arg);
        if name.is_empty() {
            return tool_err("invalid_args", "`emoji` cannot be empty");
        }
        let (api, workspace) = match self.resolve_live_api(workspace_arg.as_deref()) {
            Ok(pair) => pair,
            Err((code, msg)) => return tool_err(code, msg),
        };
        let archive = self.open_archive(&workspace).ok().flatten();
        let (channel_id, _label) = resolve_channel_with(archive.as_ref(), &workspace, &channel_arg);
        let ts = Ts::from(ts_arg);

        match api.reactions_add(&channel_id, &ts, &name).await {
            Ok(()) => tool_ok(&json!({
                "ok": true,
                "workspace": workspace.as_str(),
                "channel": channel_id.as_str(),
                "ts": ts.as_str(),
                "name": name,
            })),
            Err(err) => {
                let (code, msg) = classify_api_error(&err);
                tool_err(code, msg)
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Protocol helpers & schemas
// -----------------------------------------------------------------------------

fn negotiate_protocol_version(requested: Option<&str>) -> &'static str {
    if let Some(req) = requested {
        for &supported in SUPPORTED_PROTOCOL_VERSIONS {
            if supported == req {
                return supported;
            }
        }
    }
    DEFAULT_PROTOCOL_VERSION
}

fn handle_initialize(params: &Map<String, Value>) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let protocol_version = negotiate_protocol_version(requested);
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": {
                "listChanged": false
            }
        },
        "serverInfo": {
            "name": "slackless",
            "version": CLI_VERSION
        },
        "instructions": "Query local Slack SQLite archives offline (workspaces, channels, FTS5 search including audio/video file transcripts, channel history, threads, transcripts) and perform live Slack writes (send, reply, react) using stored slackless sessions."
    })
}

fn jsonrpc_ok(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn jsonrpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into(),
        }
    })
}

fn tool_ok(payload: &impl Serialize) -> Value {
    let text = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_owned());
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false,
    })
}

fn tool_err(code: &str, message: impl Into<String>) -> Value {
    let text = serde_json::to_string(&json!({
        "error": code,
        "message": message.into(),
    }))
    .unwrap_or_else(|_| format!("{{\"error\":\"{code}\"}}"));
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": true,
    })
}

fn check_secrets_mcp(text: &str, allow_secrets: bool) -> Option<Value> {
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
    Some(tool_err(
        "secret_detected",
        format!(
            "refused to send possible secret(s): {}; pass `allow_secrets: true` to override",
            summary.join(", ")
        ),
    ))
}

/// Paginated local channel read supporting both `before` (ts cursor) and `since` (epoch cutoff)
/// without letting deleted tombstones starve `limit`.
fn read_channel_window(
    store: &Store,
    workspace: &TeamId,
    channel: &ChannelId,
    initial_before: Option<&Ts>,
    limit: usize,
    cutoff_secs: Option<i64>,
    include_deleted: bool,
) -> slackless_core::store::Result<Vec<StoredMessage>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    if include_deleted && cutoff_secs.is_none() {
        return store.channel_history(workspace, channel, initial_before, limit);
    }

    let page_size = limit.clamp(64, 500);
    let mut collected: Vec<StoredMessage> = Vec::with_capacity(limit.min(512));
    let mut before: Option<Ts> = initial_before.cloned();

    'pages: loop {
        let page = store.channel_history(workspace, channel, before.as_ref(), page_size)?;
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        let oldest_ts = page[0].message.ts.clone();

        for msg in page.into_iter().rev() {
            if let Some(cutoff) = cutoff_secs {
                match msg.message.ts.secs() {
                    Some(secs) if secs < cutoff => break 'pages,
                    Some(_) => {}
                    None => continue,
                }
            }
            if !include_deleted && msg.deleted {
                continue;
            }
            collected.push(msg);
            if collected.len() >= limit {
                break 'pages;
            }
        }

        if page_len < page_size || before.as_ref() == Some(&oldest_ts) {
            break;
        }
        before = Some(oldest_ts);
    }

    collected.reverse();
    Ok(collected)
}

fn is_valid_workspace_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn validate_workspace_id(raw: &str) -> std::result::Result<String, String> {
    let trimmed = raw.trim();
    if is_valid_workspace_id(trimmed) {
        Ok(trimmed.to_owned())
    } else {
        Err(format!(
            "invalid `workspace` '{trimmed}': must contain only ASCII letters, digits, '_', or '-'"
        ))
    }
}

fn opt_workspace(args: &Map<String, Value>) -> std::result::Result<Option<String>, String> {
    match opt_str(args, "workspace")? {
        Some(s) if !s.trim().is_empty() => validate_workspace_id(&s).map(Some),
        _ => Ok(None),
    }
}

/// Round-robin interleave per-workspace ranked hit lists up to `limit` so an early workspace
/// with `>= limit` hits cannot starve later workspaces when sorting by local BM25 relevance.
fn interleave_round_robin<T>(lists: Vec<Vec<T>>, limit: usize) -> Vec<T> {
    if limit == 0 {
        return Vec::new();
    }
    let mut queues: Vec<VecDeque<T>> = lists
        .into_iter()
        .filter(|list| !list.is_empty())
        .map(VecDeque::from)
        .collect();
    let mut out = Vec::with_capacity(limit);
    while out.len() < limit {
        let mut progressed = false;
        for queue in &mut queues {
            if let Some(item) = queue.pop_front() {
                out.push(item);
                progressed = true;
                if out.len() >= limit {
                    break;
                }
            }
        }
        if !progressed {
            break;
        }
    }
    out
}

fn req_str(args: &Map<String, Value>, key: &str) -> std::result::Result<String, String> {
    match args.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        Some(Value::String(_)) => Err(format!("`{key}` must be a non-empty string")),
        Some(_) => Err(format!("`{key}` must be a string")),
        None => Err(format!("missing required argument `{key}`")),
    }
}

fn opt_str(args: &Map<String, Value>, key: &str) -> std::result::Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

fn opt_bool(
    args: &Map<String, Value>,
    key: &str,
    default: bool,
) -> std::result::Result<bool, String> {
    Ok(opt_bool_optional(args, key)?.unwrap_or(default))
}

fn opt_bool_optional(
    args: &Map<String, Value>,
    key: &str,
) -> std::result::Result<Option<bool>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("`{key}` must be a boolean")),
    }
}

fn opt_usize(
    args: &Map<String, Value>,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> std::result::Result<usize, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Number(n)) => {
            let Some(val) = n.as_u64() else {
                return Err(format!("`{key}` must be a positive integer"));
            };
            let clamped = usize::try_from(val).unwrap_or(max).clamp(min, max);
            Ok(clamped)
        }
        Some(_) => Err(format!("`{key}` must be a positive integer")),
    }
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "list_workspaces",
            "description": "List stored Slack workspaces (signed-in sessions and local SQLite archives) with message counts and default workspace.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        },
        {
            "name": "list_channels",
            "description": "List channels and/or DMs known to the local archive for a workspace, with optional kind and substring filters.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace": {
                        "type": "string",
                        "description": "Workspace team ID (e.g. T0123ABC). Defaults to the server's default workspace."
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["all", "channels", "dms"],
                        "description": "Conversation kinds to return (default: \"all\")."
                    },
                    "filter": {
                        "type": "string",
                        "description": "Optional case-insensitive substring filter across channel id, #name, topic, or DM peer name."
                    },
                    "include_archived": {
                        "type": "boolean",
                        "description": "Include archived channels (default: true)."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 1000,
                        "description": "Maximum conversations to return (default: 200)."
                    }
                }
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        },
        {
            "name": "search_messages",
            "description": "Full-text FTS5 search over the local Slack SQLite archive, including message text, file names, and audio/video file transcripts. Supports Slack modifiers: in:#ch, from:@user, has:file|link|reaction|pin, before:YYYY-MM-DD, after:YYYY-MM-DD, -exclude, \"phrase\", prefix*.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query text and optional Slack-style modifiers."
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Optional workspace team ID to restrict search to one archive."
                    },
                    "all_workspaces": {
                        "type": "boolean",
                        "description": "Search across every locally archived workspace and merge hits newest first."
                    },
                    "sort": {
                        "type": "string",
                        "enum": ["recent", "relevant"],
                        "description": "Result ordering: \"recent\" (newest first, default) or \"relevant\" (FTS5 BM25 relevance)."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description": "Maximum messages to return (default: 20)."
                    },
                    "include_deleted": {
                        "type": "boolean",
                        "description": "Include locally preserved deleted message tombstones (default: false)."
                    }
                },
                "required": ["query"]
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        },
        {
            "name": "read_channel",
            "description": "Read recent or windowed messages from a channel or DM (oldest first), from the local SQLite archive by default or live from Slack when `live: true`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "channel": {
                        "type": "string",
                        "description": "Channel ID (C…/D…/G…), #name, bare name, or @handle for a DM."
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Optional workspace team ID."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description": "Maximum messages to return (default: 50)."
                    },
                    "since": {
                        "type": "string",
                        "description": "Optional lower time bound: relative duration (30m, 24h, 7d, 2w), UTC date (YYYY-MM-DD), or unix epoch seconds."
                    },
                    "before": {
                        "type": "string",
                        "description": "Optional Slack message ts cursor; returns only messages strictly older than `before`."
                    },
                    "include_deleted": {
                        "type": "boolean",
                        "description": "Include locally preserved deleted messages (default: false)."
                    },
                    "live": {
                        "type": "boolean",
                        "description": "Fetch live via conversations.history and mirror locally (requires a signed-in session; default: false)."
                    }
                },
                "required": ["channel"]
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": true
            }
        },
        {
            "name": "read_thread",
            "description": "Read a thread's root message and all replies (oldest first), from the local archive by default or live via conversations.replies when `live: true`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "channel": {
                        "type": "string",
                        "description": "Channel ID, #name, or @handle."
                    },
                    "thread_ts": {
                        "type": "string",
                        "description": "Timestamp (`ts`) of the thread root message."
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Optional workspace team ID."
                    },
                    "live": {
                        "type": "boolean",
                        "description": "Fetch fresh replies live via conversations.replies and mirror locally (default: false)."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 999,
                        "description": "Maximum messages to return (default: 200)."
                    }
                },
                "required": ["channel", "thread_ts"]
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": true
            }
        },
        {
            "name": "get_transcript",
            "description": "Fetch a stored audio/video file transcript from the local SQLite archive by Slack `file_id`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file_id": {
                        "type": "string",
                        "description": "Slack file ID (e.g. F0123ABC)."
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Optional workspace team ID. When omitted, checks the default workspace then all archived workspaces."
                    }
                },
                "required": ["file_id"]
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            }
        },
        {
            "name": "send_message",
            "description": "Post a live message to a Slack channel or DM via chat.postMessage and mirror it into the local archive (requires a signed-in session).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "channel": {
                        "type": "string",
                        "description": "Channel ID, #name, or @handle."
                    },
                    "text": {
                        "type": "string",
                        "description": "Message body text."
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Optional workspace team ID."
                    },
                    "allow_secrets": {
                        "type": "boolean",
                        "description": "Allow sending text that triggers the pre-send secret-leak detector (default: false)."
                    }
                },
                "required": ["channel", "text"]
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": true
            }
        },
        {
            "name": "reply_thread",
            "description": "Post a live reply into a Slack thread via chat.postMessage and mirror it into the local archive (requires a signed-in session).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "channel": {
                        "type": "string",
                        "description": "Channel ID, #name, or @handle."
                    },
                    "thread_ts": {
                        "type": "string",
                        "description": "Timestamp (`ts`) of the thread root message."
                    },
                    "text": {
                        "type": "string",
                        "description": "Reply body text."
                    },
                    "reply_broadcast": {
                        "type": "boolean",
                        "description": "Also broadcast the reply to the main channel timeline (default: false)."
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Optional workspace team ID."
                    },
                    "allow_secrets": {
                        "type": "boolean",
                        "description": "Allow sending text that triggers the pre-send secret-leak detector (default: false)."
                    }
                },
                "required": ["channel", "thread_ts", "text"]
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": true
            }
        },
        {
            "name": "add_reaction",
            "description": "Add a live emoji reaction to a Slack message via reactions.add (requires a signed-in session).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "channel": {
                        "type": "string",
                        "description": "Channel ID, #name, or @handle."
                    },
                    "ts": {
                        "type": "string",
                        "description": "Timestamp (`ts`) of the message to react to."
                    },
                    "emoji": {
                        "type": "string",
                        "description": "Emoji shortcode with or without colons (e.g. \"tada\" or \":tada:\")."
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Optional workspace team ID."
                    }
                },
                "required": ["channel", "ts", "emoji"]
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": true
            }
        }
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU64, Ordering};

    use slackless_core::types::{
        Conversation, File, Latest, Message, Profile, TopicPurpose, User, UserId,
    };

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "slackless-cli-mcp-test-{}-{}-{n}",
                label,
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn seed_multi_workspace_stores(dir: &TempDir) {
        // Workspace T1: channels, DMs, thread replies, deleted message, and a video clip with transcript.
        let t1 = TeamId::from("T1");
        let mut s1 = Store::open(dir.path().join("T1.sqlite")).expect("open T1 store");
        s1.upsert_users(
            &t1,
            &[
                User {
                    id: UserId::from("U1"),
                    name: "ana".into(),
                    profile: Profile {
                        display_name: Some("Ana".into()),
                        ..Profile::default()
                    },
                    ..User::default()
                },
                User {
                    id: UserId::from("U2"),
                    name: "bob".into(),
                    profile: Profile {
                        display_name: Some("Bob".into()),
                        ..Profile::default()
                    },
                    ..User::default()
                },
            ],
        )
        .expect("upsert T1 users");

        s1.upsert_conversations(
            &t1,
            &[
                Conversation {
                    id: ChannelId::from("C_ENG"),
                    name: Some("eng".into()),
                    is_channel: true,
                    is_member: true,
                    topic: Some(TopicPurpose {
                        value: "Deployments and releases".into(),
                        ..TopicPurpose::default()
                    }),
                    ..Conversation::default()
                },
                Conversation {
                    id: ChannelId::from("C_OLD"),
                    name: Some("legacy".into()),
                    is_channel: true,
                    is_archived: true,
                    is_member: false,
                    ..Conversation::default()
                },
                Conversation {
                    id: ChannelId::from("D_ANA"),
                    is_im: true,
                    user: Some(UserId::from("U1")),
                    latest: Some(Latest::Ts(Ts::from("1700000400.000000"))),
                    ..Conversation::default()
                },
            ],
        )
        .expect("upsert T1 convs");

        let clip_msg = Message {
            ts: Ts::from("1700000200.000100"),
            user: Some(UserId::from("U1")),
            text: "Watch the rollout demo clip".into(),
            reply_count: Some(1),
            files: vec![File {
                id: FileId::from("F_CLIP1"),
                name: Some("demo.mp4".into()),
                title: Some("Rollout Demo".into()),
                filetype: Some("mp4".into()),
                ..File::default()
            }],
            ..Message::default()
        };
        let reply_msg = Message {
            ts: Ts::from("1700000250.000200"),
            thread_ts: Some(Ts::from("1700000200.000100")),
            user: Some(UserId::from("U2")),
            text: "Looks solid, shipping today".into(),
            ..Message::default()
        };
        let older_msg = Message {
            ts: Ts::from("1700000100.000000"),
            user: Some(UserId::from("U2")),
            text: "Earlier status check".into(),
            ..Message::default()
        };
        let deleted_msg = Message {
            ts: Ts::from("1700000300.000000"),
            user: Some(UserId::from("U1")),
            text: "Temporary scratch note".into(),
            ..Message::default()
        };
        s1.upsert_messages(
            &t1,
            &ChannelId::from("C_ENG"),
            &[older_msg, clip_msg, reply_msg, deleted_msg],
        )
        .expect("upsert T1 messages");
        s1.record_deletion(
            &t1,
            &ChannelId::from("C_ENG"),
            &Ts::from("1700000300.000000"),
            Some(&Ts::from("1700000301.000000")),
            None,
        )
        .expect("delete message");
        s1.upsert_file_transcript(
            &t1,
            &FileId::from("F_CLIP1"),
            "parakeet-v3",
            "Zero downtime canary migration succeeded across eu-north.",
        )
        .expect("upsert transcript");

        // Workspace T2: second workspace for cross-workspace search.
        let t2 = TeamId::from("T2");
        let mut s2 = Store::open(dir.path().join("T2.sqlite")).expect("open T2 store");
        s2.upsert_conversations(
            &t2,
            &[Conversation {
                id: ChannelId::from("C_OPS"),
                name: Some("ops".into()),
                is_channel: true,
                is_member: true,
                ..Conversation::default()
            }],
        )
        .expect("upsert T2 convs");
        s2.upsert_message(
            &t2,
            &ChannelId::from("C_OPS"),
            &Message {
                ts: Ts::from("1700000500.000000"),
                text: "Canary rollout finished in workspace two".into(),
                ..Message::default()
            },
        )
        .expect("upsert T2 message");
    }

    fn parse_tool_payload(resp: &Value) -> (bool, Value) {
        let is_error = resp["result"]["isError"].as_bool().unwrap_or(false);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("tool content[0].text must be string");
        let parsed: Value = serde_json::from_str(text).expect("tool text must be valid JSON");
        (is_error, parsed)
    }

    #[tokio::test]
    async fn stdio_protocol_lifecycle_and_core_tools() {
        let dir = TempDir::new("lifecycle");
        seed_multi_workspace_stores(&dir);

        let sessions = vec![Session::new(
            "https://acme.slack.com/",
            "T1",
            "xoxc-test",
            "xoxd-test",
            "U1",
        )];
        let mut server = McpServer::for_test(dir.path().to_path_buf(), sessions, None);

        let requests = [
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "test-client", "version": "1.0" }
                }
            }),
            // Notification: must NOT produce any response line.
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "ping"
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/list"
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": { "name": "list_workspaces", "arguments": {} }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "list_channels",
                    "arguments": { "workspace": "T1", "include_archived": false }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": {
                    "name": "search_messages",
                    "arguments": { "query": "canary", "all_workspaces": true }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {
                    "name": "read_channel",
                    "arguments": { "channel": "#eng", "before": "1700000250.000000", "limit": 10 }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 8,
                "method": "tools/call",
                "params": {
                    "name": "read_thread",
                    "arguments": { "channel": "#eng", "thread_ts": "1700000200.000100" }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 9,
                "method": "tools/call",
                "params": {
                    "name": "get_transcript",
                    "arguments": { "file_id": "F_CLIP1" }
                }
            }),
        ];

        let mut input = String::new();
        for req in &requests {
            input.push_str(&serde_json::to_string(req).expect("serialize req"));
            input.push('\n');
        }
        // Extra blank line should be skipped cleanly.
        input.push_str("\n   \n");

        let mut output = Vec::new();
        server
            .serve_io(Cursor::new(input), &mut output)
            .await
            .expect("serve_io");
        assert!(server.initialized);

        let out_str = String::from_utf8(output).expect("valid utf8");
        let lines: Vec<Value> = out_str
            .lines()
            .map(|l| serde_json::from_str(l).expect("valid response JSON"))
            .collect();
        // 10 inputs minus 1 notification = 9 responses.
        assert_eq!(lines.len(), 9);

        // 1. initialize
        assert_eq!(lines[0]["id"], 1);
        assert_eq!(lines[0]["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(lines[0]["result"]["serverInfo"]["name"], "slackless");
        assert!(lines[0]["result"]["capabilities"]["tools"].is_object());

        // 2. ping
        assert_eq!(lines[1]["id"], 2);
        assert_eq!(lines[1]["result"], json!({}));

        // 3. tools/list
        assert_eq!(lines[2]["id"], 3);
        let tools = lines[2]["result"]["tools"].as_array().expect("tools array");
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(
            names,
            vec![
                "list_workspaces",
                "list_channels",
                "search_messages",
                "read_channel",
                "read_thread",
                "get_transcript",
                "send_message",
                "reply_thread",
                "add_reaction",
            ]
        );
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object");
        }

        // 4. list_workspaces (both signed-in T1 and archive-only T2)
        let (err, ws_data) = parse_tool_payload(&lines[3]);
        assert!(!err);
        assert_eq!(ws_data["default_workspace"], "T1");
        assert_eq!(ws_data["count"], 2);
        assert_eq!(ws_data["workspaces"][0]["team_id"], "T1");
        assert_eq!(ws_data["workspaces"][0]["signed_in"], true);
        assert_eq!(ws_data["workspaces"][0]["messages"], 4);
        assert_eq!(ws_data["workspaces"][1]["team_id"], "T2");
        assert_eq!(ws_data["workspaces"][1]["signed_in"], false);
        assert_eq!(ws_data["workspaces"][1]["messages"], 1);

        // 5. list_channels (archived excluded -> C_ENG + D_ANA)
        let (err, ch_data) = parse_tool_payload(&lines[4]);
        assert!(!err);
        assert_eq!(ch_data["count"], 2);
        let ch_ids: Vec<&str> = ch_data["channels"]
            .as_array()
            .expect("channels array")
            .iter()
            .filter_map(|c| c["id"].as_str())
            .collect();
        assert_eq!(ch_ids, vec!["C_ENG", "D_ANA"]);
        assert_eq!(ch_data["channels"][1]["label"], "@Ana");

        // 6. search_messages across all workspaces matches T2 text AND T1 file transcript!
        let (err, search_data) = parse_tool_payload(&lines[5]);
        assert!(!err);
        assert_eq!(search_data["all_workspaces"], true);
        assert_eq!(search_data["count"], 2);
        let hits = search_data["messages"].as_array().expect("hits array");
        // Newest first: T2 (1700000500) then T1 transcript hit (1700000200).
        assert_eq!(hits[0]["workspace"], "T2");
        assert_eq!(hits[0]["channel"], "C_OPS");
        assert_eq!(hits[1]["workspace"], "T1");
        assert_eq!(hits[1]["channel"], "C_ENG");
        assert_eq!(hits[1]["file_refs"][0]["id"], "F_CLIP1");

        // 7. read_channel windowed with `before` (oldest first, excludes deleted & newer)
        let (err, read_data) = parse_tool_payload(&lines[6]);
        assert!(!err);
        assert_eq!(read_data["live"], false);
        assert_eq!(read_data["channel"]["id"], "C_ENG");
        assert_eq!(read_data["count"], 2);
        assert_eq!(read_data["messages"][0]["ts"], "1700000100.000000");
        assert_eq!(read_data["messages"][1]["ts"], "1700000200.000100");
        assert_eq!(read_data["messages"][1]["user_name"], "Ana");

        // 8. read_thread returns root + reply oldest first
        let (err, thread_data) = parse_tool_payload(&lines[7]);
        assert!(!err);
        assert_eq!(thread_data["count"], 2);
        assert_eq!(thread_data["messages"][0]["ts"], "1700000200.000100");
        assert_eq!(thread_data["messages"][1]["ts"], "1700000250.000200");
        assert_eq!(thread_data["messages"][1]["user_name"], "Bob");

        // 9. get_transcript returns persisted transcript
        let (err, tr_data) = parse_tool_payload(&lines[8]);
        assert!(!err);
        assert_eq!(tr_data["workspace"], "T1");
        assert_eq!(tr_data["file_id"], "F_CLIP1");
        assert_eq!(tr_data["source"], "parakeet-v3");
        assert!(
            tr_data["text"]
                .as_str()
                .expect("transcript text")
                .contains("eu-north")
        );
    }

    #[tokio::test]
    async fn error_handling_protocol_and_tool_errors() {
        let dir = TempDir::new("errors");
        seed_multi_workspace_stores(&dir);
        let mut server =
            McpServer::for_test(dir.path().to_path_buf(), Vec::new(), Some("T1".into()));

        // Parse error (-32700).
        let r = server
            .handle_line("{not valid json")
            .await
            .expect("parse error response");
        assert_eq!(r["error"]["code"], ERR_PARSE);
        assert_eq!(r["id"], Value::Null);

        // Invalid request: non-object (-32600).
        let r = server
            .handle_line("[1, 2, 3]")
            .await
            .expect("invalid request response");
        assert_eq!(r["error"]["code"], ERR_INVALID_REQUEST);

        // Invalid request: wrong jsonrpc version (-32600).
        let r = server
            .handle_line(r#"{"jsonrpc":"1.0","id":10,"method":"ping"}"#)
            .await
            .expect("wrong version response");
        assert_eq!(r["error"]["code"], ERR_INVALID_REQUEST);
        assert_eq!(r["id"], 10);

        // Method not found (-32601).
        let r = server
            .handle_line(r#"{"jsonrpc":"2.0","id":"abc","method":"resources/list"}"#)
            .await
            .expect("method not found response");
        assert_eq!(r["error"]["code"], ERR_METHOD_NOT_FOUND);
        assert_eq!(r["id"], "abc");

        // Unknown tool (-32602).
        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"does_not_exist","arguments":{}}}"#,
            )
            .await
            .expect("unknown tool response");
        assert_eq!(r["error"]["code"], ERR_INVALID_PARAMS);

        // Missing transcript -> isError: true with structured error code.
        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"get_transcript","arguments":{"file_id":"F_MISSING"}}}"#,
            )
            .await
            .expect("missing transcript response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(is_err);
        assert_eq!(payload["error"], "transcript_not_found");

        // Missing required argument -> isError: true, invalid_args.
        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":13,"method":"tools/call","params":{"name":"search_messages","arguments":{}}}"#,
            )
            .await
            .expect("missing query response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(is_err);
        assert_eq!(payload["error"], "invalid_args");

        // Conflicting workspace + all_workspaces -> invalid_args.
        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":14,"method":"tools/call","params":{"name":"search_messages","arguments":{"query":"x","workspace":"T1","all_workspaces":true}}}"#,
            )
            .await
            .expect("conflict response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(is_err);
        assert_eq!(payload["error"], "invalid_args");

        // Live write without stored session -> not_signed_in.
        let r = server
            .handle_line(
                r##"{"jsonrpc":"2.0","id":15,"method":"tools/call","params":{"name":"send_message","arguments":{"channel":"#eng","text":"hello"}}}"##,
            )
            .await
            .expect("not signed in response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(is_err);
        assert_eq!(payload["error"], "not_signed_in");

        // Secret-leak detector fires before session/network on send_message and reply_thread.
        let fake_secret = format!("xoxb-{}-{}", "123456789012", "abcdefghijklmnopqrstuvwx");
        let req = json!({
            "jsonrpc": "2.0",
            "id": 16,
            "method": "tools/call",
            "params": {
                "name": "send_message",
                "arguments": { "channel": "#eng", "text": fake_secret }
            }
        });
        let r = server
            .handle_line(&serde_json::to_string(&req).expect("serialize"))
            .await
            .expect("secret detected response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(is_err);
        assert_eq!(payload["error"], "secret_detected");

        // Empty emoji after colon stripping -> invalid_args.
        let r = server
            .handle_line(
                r##"{"jsonrpc":"2.0","id":17,"method":"tools/call","params":{"name":"add_reaction","arguments":{"channel":"#eng","ts":"1700000100.000000","emoji":"::"}}}"##,
            )
            .await
            .expect("empty emoji response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(is_err);
        assert_eq!(payload["error"], "invalid_args");

        // Invalid `since` on read_channel -> invalid_args.
        let r = server
            .handle_line(
                r##"{"jsonrpc":"2.0","id":18,"method":"tools/call","params":{"name":"read_channel","arguments":{"channel":"#eng","since":"not-a-date"}}}"##,
            )
            .await
            .expect("invalid since response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(is_err);
        assert_eq!(payload["error"], "invalid_args");

        // tools/call missing `name` and non-object `arguments` -> JSON-RPC -32602.
        let r = server
            .handle_line(r#"{"jsonrpc":"2.0","id":19,"method":"tools/call","params":{}}"#)
            .await
            .expect("missing tool name");
        assert_eq!(r["error"]["code"], ERR_INVALID_PARAMS);

        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":20,"method":"tools/call","params":{"name":"list_workspaces","arguments":"bad"}}"#,
            )
            .await
            .expect("bad arguments type");
        assert_eq!(r["error"]["code"], ERR_INVALID_PARAMS);

        // reply_thread secret detection & not_signed_in, plus live read_channel / read_thread not_signed_in.
        let req = json!({
            "jsonrpc": "2.0",
            "id": 21,
            "method": "tools/call",
            "params": {
                "name": "reply_thread",
                "arguments": {
                    "channel": "#eng",
                    "thread_ts": "1700000200.000100",
                    "text": fake_secret
                }
            }
        });
        let r = server
            .handle_line(&serde_json::to_string(&req).expect("serialize"))
            .await
            .expect("reply secret response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(is_err);
        assert_eq!(payload["error"], "secret_detected");

        let r = server
            .handle_line(
                r##"{"jsonrpc":"2.0","id":22,"method":"tools/call","params":{"name":"reply_thread","arguments":{"channel":"#eng","thread_ts":"1700000200.000100","text":"safe reply"}}}"##,
            )
            .await
            .expect("reply not signed in");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(is_err);
        assert_eq!(payload["error"], "not_signed_in");

        let r = server
            .handle_line(
                r##"{"jsonrpc":"2.0","id":23,"method":"tools/call","params":{"name":"read_channel","arguments":{"channel":"#eng","live":true}}}"##,
            )
            .await
            .expect("live read_channel not signed in");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(is_err);
        assert_eq!(payload["error"], "not_signed_in");

        let r = server
            .handle_line(
                r##"{"jsonrpc":"2.0","id":24,"method":"tools/call","params":{"name":"read_thread","arguments":{"channel":"#eng","thread_ts":"1700000200.000100","live":true}}}"##,
            )
            .await
            .expect("live read_thread not signed in");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(is_err);
        assert_eq!(payload["error"], "not_signed_in");

        // Invalid `kind` on list_channels and invalid `sort` on search_messages.
        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":25,"method":"tools/call","params":{"name":"list_channels","arguments":{"kind":"bogus"}}}"#,
            )
            .await
            .expect("invalid kind");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(is_err);
        assert_eq!(payload["error"], "invalid_args");

        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":26,"method":"tools/call","params":{"name":"search_messages","arguments":{"query":"x","sort":"bogus"}}}"#,
            )
            .await
            .expect("invalid sort");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(is_err);
        assert_eq!(payload["error"], "invalid_args");
    }

    #[tokio::test]
    async fn search_filter_and_channel_filter_edge_cases() {
        let dir = TempDir::new("filters");
        seed_multi_workspace_stores(&dir);
        // No sessions + two archives -> default workspace is ambiguous (None).
        let mut server = McpServer::for_test(dir.path().to_path_buf(), Vec::new(), None);

        // search_messages with neither workspace nor all_workspaces falls back to all archives.
        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_messages","arguments":{"query":"canary","sort":"relevant"}}}"#,
            )
            .await
            .expect("search response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(!is_err);
        assert_eq!(payload["all_workspaces"], true);
        assert_eq!(payload["sort"], "relevant");
        assert_eq!(payload["count"], 2);

        // get_transcript without workspace still locates F_CLIP1 in T1.
        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_transcript","arguments":{"file_id":"F_CLIP1"}}}"#,
            )
            .await
            .expect("transcript response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(!is_err);
        assert_eq!(payload["workspace"], "T1");
        assert_eq!(payload["file_id"], "F_CLIP1");

        // list_channels with kind="dms" and filter="@ana" on explicit workspace.
        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_channels","arguments":{"workspace":"T1","kind":"dms","filter":"@ANA"}}}"#,
            )
            .await
            .expect("channels response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(!is_err);
        assert_eq!(payload["count"], 1);
        assert_eq!(payload["channels"][0]["id"], "D_ANA");

        // read_channel with include_deleted=true includes the tombstone.
        let r = server
            .handle_line(
                r##"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"read_channel","arguments":{"workspace":"T1","channel":"#eng","include_deleted":true}}}"##,
            )
            .await
            .expect("read deleted response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(!is_err);
        assert_eq!(payload["count"], 3);
        assert_eq!(payload["messages"][2]["deleted"], true);

        // read_channel with `since` (unix epoch seconds) filters out older messages.
        let r = server
            .handle_line(
                r##"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"read_channel","arguments":{"workspace":"T1","channel":"#eng","since":"1700000150"}}}"##,
            )
            .await
            .expect("read since response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(!is_err);
        assert_eq!(payload["count"], 1);
        assert_eq!(payload["messages"][0]["ts"], "1700000200.000100");

        // search_messages with Slack modifiers (in:#eng from:@ana has:file).
        let r = server
            .handle_line(
                r##"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"search_messages","arguments":{"workspace":"T1","query":"eu-north in:#eng from:@ana has:file"}}}"##,
            )
            .await
            .expect("modifier search response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(!is_err);
        assert_eq!(payload["count"], 1);
        assert_eq!(payload["messages"][0]["ts"], "1700000200.000100");
        assert_eq!(payload["messages"][0]["file_refs"][0]["id"], "F_CLIP1");

        // list_channels without workspace when multiple archives exist returns no_workspace.
        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"list_channels","arguments":{}}}"#,
            )
            .await
            .expect("ambiguous workspace response");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(is_err);
        assert_eq!(payload["error"], "no_workspace");

        // Protocol version negotiation falls back to default for unknown future version.
        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":5,"method":"initialize","params":{"protocolVersion":"2099-01-01"}}"#,
            )
            .await
            .expect("init response");
        assert_eq!(r["result"]["protocolVersion"], DEFAULT_PROTOCOL_VERSION);
    }

    #[test]
    fn cli_parses_mcp_subcommand_with_optional_workspace() {
        use clap::Parser as _;
        let cli = crate::cli::Cli::try_parse_from(["slackless", "mcp"]).expect("parse mcp");
        assert!(matches!(cli.command, crate::cli::Commands::Mcp));
        assert_eq!(cli.workspace, None);

        let cli = crate::cli::Cli::try_parse_from(["slackless", "--workspace", "T1", "mcp"])
            .expect("parse mcp with workspace");
        assert!(matches!(cli.command, crate::cli::Commands::Mcp));
        assert_eq!(cli.workspace.as_deref(), Some("T1"));
    }

    #[tokio::test]
    async fn workspace_validation_blocks_path_traversal() {
        let parent = TempDir::new("traversal-parent");
        let archive_dir = parent.path().join("archives");
        std::fs::create_dir_all(&archive_dir).expect("create archives dir");

        // Place a real SQLite archive OUTSIDE `archive_dir` that a traversal payload targets.
        let outside_ws = TeamId::from("secret_outside");
        let outside_db = parent.path().join("secret_outside.sqlite");
        let mut outside_store = Store::open(&outside_db).expect("open outside store");
        outside_store
            .upsert_file_transcript(
                &outside_ws,
                &FileId::from("F_SECRET"),
                "local",
                "top secret",
            )
            .expect("upsert outside transcript");
        drop(outside_store);

        let mut server = McpServer::for_test(archive_dir.clone(), Vec::new(), None);

        // Defense-in-depth: `archive_path_for` refuses unsafe IDs directly.
        for bad in [
            "../secret_outside",
            "../../etc/passwd",
            "/tmp/secret_outside",
            "T1/../T2",
            r"T1\..\T2",
            "..",
            ".",
            "T1.sqlite",
            "T1 space",
        ] {
            assert!(
                server.archive_path_for(&TeamId::from(bad)).is_none(),
                "archive_path_for must reject unsafe team id {bad:?}"
            );
        }
        assert!(
            server
                .archive_path_for(&TeamId::from("T0123_ABC-1"))
                .is_some()
        );

        // Every tool that accepts `workspace` rejects traversal inputs with `invalid_args`.
        let calls = [
            json!({"name": "list_channels", "arguments": {"workspace": "../secret_outside"}}),
            json!({"name": "search_messages", "arguments": {"query": "secret", "workspace": "/tmp/foo"}}),
            json!({"name": "read_channel", "arguments": {"channel": "#eng", "workspace": "../../etc"}}),
            json!({"name": "read_thread", "arguments": {"channel": "#eng", "thread_ts": "1.0", "workspace": "../secret_outside"}}),
            json!({"name": "get_transcript", "arguments": {"file_id": "F_SECRET", "workspace": "../secret_outside"}}),
            json!({"name": "send_message", "arguments": {"channel": "#eng", "text": "hi", "workspace": "../secret_outside"}}),
            json!({"name": "reply_thread", "arguments": {"channel": "#eng", "thread_ts": "1.0", "text": "hi", "workspace": r"T1\..\T2"}}),
            json!({"name": "add_reaction", "arguments": {"channel": "#eng", "ts": "1.0", "emoji": "tada", "workspace": "/tmp/foo"}}),
        ];
        for (idx, params) in calls.into_iter().enumerate() {
            let req = json!({
                "jsonrpc": "2.0",
                "id": idx + 1,
                "method": "tools/call",
                "params": params,
            });
            let resp = server
                .handle_line(&serde_json::to_string(&req).expect("serialize req"))
                .await
                .expect("tool response");
            let (is_err, payload) = parse_tool_payload(&resp);
            assert!(is_err, "expected error for call #{idx}");
            assert_eq!(
                payload["error"], "invalid_args",
                "expected invalid_args for call #{idx}: {payload}"
            );
        }

        // Invalid `default_workspace` on the server itself is also rejected.
        let mut bad_default =
            McpServer::for_test(archive_dir, Vec::new(), Some("../secret_outside".into()));
        let resp = bad_default
            .handle_line(
                r#"{"jsonrpc":"2.0","id":99,"method":"tools/call","params":{"name":"get_transcript","arguments":{"file_id":"F_SECRET"}}}"#,
            )
            .await
            .expect("bad default response");
        let (is_err, payload) = parse_tool_payload(&resp);
        assert!(is_err);
        assert_eq!(payload["error"], "invalid_args");
    }

    #[tokio::test]
    async fn cross_workspace_relevant_search_interleaves_without_starving_later_workspaces() {
        let dir = TempDir::new("relevant-interleave");

        // Workspace T1 has 3 matching messages (>= limit).
        let t1 = TeamId::from("T1");
        let mut s1 = Store::open(dir.path().join("T1.sqlite")).expect("open T1");
        s1.upsert_messages(
            &t1,
            &ChannelId::from("C1"),
            &[
                Message {
                    ts: Ts::from("1700000100.000000"),
                    text: "aurora deploy aurora aurora".into(),
                    ..Message::default()
                },
                Message {
                    ts: Ts::from("1700000200.000000"),
                    text: "aurora staging check".into(),
                    ..Message::default()
                },
                Message {
                    ts: Ts::from("1700000300.000000"),
                    text: "aurora notes".into(),
                    ..Message::default()
                },
            ],
        )
        .expect("upsert T1");

        // Workspace T2 has 2 matching messages.
        let t2 = TeamId::from("T2");
        let mut s2 = Store::open(dir.path().join("T2.sqlite")).expect("open T2");
        s2.upsert_messages(
            &t2,
            &ChannelId::from("C2"),
            &[
                Message {
                    ts: Ts::from("1700000400.000000"),
                    text: "aurora production aurora".into(),
                    ..Message::default()
                },
                Message {
                    ts: Ts::from("1700000500.000000"),
                    text: "aurora follow-up".into(),
                    ..Message::default()
                },
            ],
        )
        .expect("upsert T2");

        let mut server = McpServer::for_test(dir.path().to_path_buf(), Vec::new(), None);

        // With `limit: 2`, round-robin interleaving ensures T1 and T2 each get their top hit.
        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_messages","arguments":{"query":"aurora","all_workspaces":true,"sort":"relevant","limit":2}}}"#,
            )
            .await
            .expect("relevant search limit=2");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(!is_err);
        assert_eq!(payload["count"], 2);
        let ws_order: Vec<&str> = payload["messages"]
            .as_array()
            .expect("messages array")
            .iter()
            .filter_map(|m| m["workspace"].as_str())
            .collect();
        assert_eq!(ws_order, vec!["T1", "T2"]);

        // With `limit: 4`, round-robin yields T1, T2, T1, T2.
        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_messages","arguments":{"query":"aurora","all_workspaces":true,"sort":"relevant","limit":4}}}"#,
            )
            .await
            .expect("relevant search limit=4");
        let (is_err, payload) = parse_tool_payload(&r);
        assert!(!is_err);
        assert_eq!(payload["count"], 4);
        let ws_order: Vec<&str> = payload["messages"]
            .as_array()
            .expect("messages array")
            .iter()
            .filter_map(|m| m["workspace"].as_str())
            .collect();
        assert_eq!(ws_order, vec!["T1", "T2", "T1", "T2"]);
    }
}
