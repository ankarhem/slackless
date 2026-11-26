//! Lightweight JSON-RPC 2.0 stdio Model Context Protocol (MCP) server (`slackless mcp`).
//!
//! Exposes offline SQLite archive queries (`list_workspaces`, `list_channels`,
//! `search_messages`, `read_channel`, `read_thread`, `get_transcript`, `list_transcripts`,
//! `export_conversation`, `get_catchup_summary`), live Slack write/read actions
//! (`send_message`, `reply_thread`, `add_reaction`, plus `live: true` on `read_channel`
//! and `read_thread`), MCP resources (`slackless://...`), and reusable MCP prompts
//! (`catch_up`, `summarize_thread`, `summarize_channel`, `find_decisions`,
//! `search_transcripts`) with zero extra protocol dependencies.
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
use slackless_core::NotifyLevel;
use slackless_core::api::{HistoryOpts, MessageContent, SlackApi};
use slackless_core::auth::{Session, SessionStore};
use slackless_core::domain::mrkdwn::mrkdwn_to_plain;
use slackless_core::export::{
    ExportFormat, ExportOptions, export_channel, export_file_transcript, export_huddle_transcript,
    export_thread, format_utc_timestamp,
};
use slackless_core::store::{
    Has, SearchQuery, SearchSort, Store, StoredFileTranscript, StoredMessage,
};
use slackless_core::types::time::now_epoch_secs;
use slackless_core::types::{ChannelId, FileId, Message, TeamId, TranscriptCue, Ts};

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
const ERR_INTERNAL: i64 = -32603;
const ERR_RESOURCE_NOT_FOUND: i64 = -32002;

const DEFAULT_CHANNEL_LIST_LIMIT: usize = 200;
const MAX_CHANNEL_LIST_LIMIT: usize = 1000;
const DEFAULT_SEARCH_LIMIT: usize = 20;
const MAX_SEARCH_LIMIT: usize = 200;
const DEFAULT_READ_LIMIT: usize = 50;
const MAX_READ_LIMIT: usize = 200;
const DEFAULT_THREAD_LIMIT: usize = 200;
const MAX_THREAD_LIMIT: usize = 999;
const DEFAULT_EXPORT_LIMIT: usize = 500;
const MAX_EXPORT_LIMIT: usize = 5_000;
const DEFAULT_TRANSCRIPT_LIST_LIMIT: usize = 50;
const MAX_TRANSCRIPT_LIST_LIMIT: usize = 200;
const DEFAULT_CATCHUP_HOURS: usize = 24;
const MAX_CATCHUP_HOURS: usize = 720;
const DEFAULT_CATCHUP_LIMIT: usize = 20;
const MAX_CATCHUP_LIMIT: usize = 100;
const MAX_PROMPT_CONTEXT_CHARS: usize = 16_000;
const MAX_EXCERPT_CHARS: usize = 200;

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

    fn resource_not_found(message: impl Into<String>) -> Self {
        Self {
            code: ERR_RESOURCE_NOT_FOUND,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: ERR_INTERNAL,
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

#[derive(Serialize)]
struct TranscriptListItem {
    kind: &'static str,
    workspace: String,
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_root_ts: Option<String>,
    updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ended_at_ms: Option<i64>,
    duration_secs: Option<f64>,
    cue_count: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    languages: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uri: Option<String>,
    excerpt: String,
}

#[derive(Serialize)]
struct CatchupChannelEntry {
    workspace: String,
    channel: String,
    label: String,
    message_count: u64,
    author_count: u64,
    unread_count: u32,
    is_unread: bool,
    muted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_ts: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_author: Option<String>,
    latest_text: String,
}

#[derive(Serialize)]
struct CatchupMentionEntry {
    workspace: String,
    channel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel_name: Option<String>,
    ts: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_ts: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_name: Option<String>,
    excerpt: String,
}

#[derive(Serialize)]
struct CatchupThreadEntry {
    workspace: String,
    channel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel_name: Option<String>,
    thread_ts: String,
    reply_count: u32,
    latest_activity_ts: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    root_excerpt: String,
}

struct CatchupSummaryData {
    since_hours: usize,
    cutoff_secs: i64,
    all_workspaces: bool,
    workspaces: Vec<String>,
    total_messages: u64,
    channels: Vec<CatchupChannelEntry>,
    mentions: Vec<CatchupMentionEntry>,
    active_threads: Vec<CatchupThreadEntry>,
    new_transcripts: Vec<TranscriptListItem>,
}

enum ResourceUri {
    Workspaces,
    WorkspaceChannels {
        workspace: TeamId,
    },
    Channel {
        workspace: TeamId,
        channel: String,
    },
    Thread {
        workspace: TeamId,
        channel: String,
        thread_ts: Ts,
    },
    Transcript {
        workspace: TeamId,
        file_id: FileId,
    },
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
            "resources/list" => Some(jsonrpc_ok(
                req_id,
                json!({ "resources": resource_definitions() }),
            )),
            "resources/templates/list" => Some(jsonrpc_ok(
                req_id,
                json!({ "resourceTemplates": resource_template_definitions() }),
            )),
            "resources/read" => match self.handle_resources_read(&params) {
                Ok(result) => Some(jsonrpc_ok(req_id, result)),
                Err(err) => Some(jsonrpc_error(req_id, err.code, err.message)),
            },
            "prompts/list" => Some(jsonrpc_ok(
                req_id,
                json!({ "prompts": prompt_definitions() }),
            )),
            "prompts/get" => match self.handle_prompts_get(&params) {
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
            "list_transcripts" => Ok(self.tool_list_transcripts(&args)),
            "export_conversation" => Ok(self.tool_export_conversation(&args)),
            "get_catchup_summary" => Ok(self.tool_get_catchup_summary(&args)),
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

    fn workspace_candidates(
        &self,
        explicit: Option<&str>,
    ) -> std::result::Result<Vec<TeamId>, String> {
        if let Some(ws) = explicit {
            return Ok(vec![TeamId::from(ws)]);
        }
        let mut list = Vec::new();
        let mut seen = HashSet::new();
        if let Some(default_ws) = self.resolve_workspace(None)?
            && seen.insert(default_ws.clone())
        {
            list.push(default_ws);
        }
        for ws in self.all_workspace_ids() {
            if seen.insert(ws.clone()) {
                list.push(ws);
            }
        }
        Ok(list)
    }

    fn resolve_scope_workspaces(
        &self,
        workspace_arg: Option<&str>,
        all_workspaces_arg: Option<bool>,
    ) -> std::result::Result<(bool, Vec<TeamId>), (&'static str, String)> {
        if workspace_arg.is_some() && all_workspaces_arg == Some(true) {
            return Err((
                "invalid_args",
                "cannot specify both `workspace` and `all_workspaces: true`".to_owned(),
            ));
        }
        if let Some(ws) = workspace_arg {
            validate_workspace_id(ws).map_err(|m| ("invalid_args", m))?;
            return Ok((false, vec![TeamId::from(ws)]));
        }
        match all_workspaces_arg {
            Some(true) => Ok((true, self.all_workspace_ids())),
            Some(false) => match self
                .resolve_workspace(None)
                .map_err(|m| ("invalid_args", m))?
            {
                Some(ws) => Ok((false, vec![ws])),
                None => Err((
                    "no_workspace",
                    "no workspace resolved; pass `workspace` or set `all_workspaces: true`"
                        .to_owned(),
                )),
            },
            None => match self
                .resolve_workspace(None)
                .map_err(|m| ("invalid_args", m))?
            {
                Some(ws) => Ok((false, vec![ws])),
                None => Ok((true, self.all_workspace_ids())),
            },
        }
    }

    fn list_workspaces_payload(&self) -> Value {
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

        json!({
            "default_workspace": default_workspace,
            "count": workspaces.len(),
            "workspaces": workspaces,
        })
    }

    fn list_channels_payload(
        &self,
        workspace: &TeamId,
        kind: &str,
        filter: Option<&str>,
        include_archived: bool,
        limit: usize,
    ) -> std::result::Result<Value, String> {
        let Some(archive) = self.open_archive(workspace)? else {
            return Ok(json!({
                "workspace": workspace.as_str(),
                "kind": kind,
                "archive_exists": false,
                "count": 0,
                "channels": [],
            }));
        };

        let user_names = archive
            .user_display_map(workspace)
            .map_err(|err| format!("loading users: {err}"))?;

        let mut rows = Vec::new();
        if matches!(kind, "all" | "channels") {
            rows.extend(
                archive
                    .channels(workspace)
                    .map_err(|err| format!("listing channels: {err}"))?,
            );
        }
        if matches!(kind, "all" | "dms") {
            rows.extend(
                archive
                    .dms(workspace)
                    .map_err(|err| format!("listing DMs: {err}"))?,
            );
        }

        let needle = filter.filter(|s| !s.is_empty());
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

        Ok(json!({
            "workspace": workspace.as_str(),
            "kind": kind,
            "count": entries.len(),
            "total_matched": total_matched,
            "channels": entries,
        }))
    }

    // -------------------------------------------------------------------------
    // Tool implementations
    // -------------------------------------------------------------------------

    fn tool_list_workspaces(&self) -> Value {
        tool_ok(&self.list_workspaces_payload())
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
        match self.list_channels_payload(
            &workspace,
            &kind,
            filter.as_deref(),
            include_archived,
            limit,
        ) {
            Ok(payload) => tool_ok(&payload),
            Err(msg) => tool_err("store_error", msg),
        }
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

        let candidates = match self.workspace_candidates(workspace_arg.as_deref()) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
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
                    return tool_ok(&file_transcript_payload(ws, &transcript));
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

    fn collect_transcripts(
        &self,
        workspaces: &[TeamId],
        kind: &str,
        channel_filter: Option<&str>,
        since_secs: Option<i64>,
        limit: usize,
    ) -> std::result::Result<Vec<TranscriptListItem>, String> {
        let mut items: Vec<TranscriptListItem> = Vec::new();
        for ws in workspaces {
            let Some(archive) = self.open_archive(ws)? else {
                continue;
            };
            let resolved_channel_id = match channel_filter {
                Some(ch_arg) => {
                    let resolved = archive
                        .resolve_channel(ws, ch_arg)
                        .map_err(|err| format!("resolving channel '{ch_arg}' in {ws}: {err}"))?;
                    Some(
                        resolved
                            .as_ref()
                            .map_or_else(|| raw_channel_id(ch_arg), |row| row.id.clone()),
                    )
                }
                None => None,
            };

            if matches!(kind, "all" | "file") {
                let allowed_files: Option<HashSet<FileId>> =
                    if let Some(ch_id) = resolved_channel_id.as_ref() {
                        let mut q = SearchQuery::default();
                        q.filters.in_channels.push(ch_id.as_str().to_owned());
                        q.filters.has.push(Has::File);
                        let msgs = archive
                            .store()
                            .search_sorted(ws, &q, SearchSort::Recent, 500)
                            .map_err(|err| {
                                format!("listing files in channel {ch_id} of {ws}: {err}")
                            })?;
                        let mut set = HashSet::new();
                        for msg in msgs {
                            for f in msg.message.files {
                                if !f.id.as_str().is_empty() {
                                    set.insert(f.id);
                                }
                            }
                        }
                        Some(set)
                    } else {
                        None
                    };

                let all_files = archive
                    .store()
                    .all_file_transcripts(ws)
                    .map_err(|err| format!("listing file transcripts for {ws}: {err}"))?;
                for file_id in all_files.keys() {
                    if let Some(allowed) = allowed_files.as_ref()
                        && !allowed.contains(file_id)
                    {
                        continue;
                    }
                    let Some(tr) = archive
                        .store()
                        .file_transcript(ws, file_id)
                        .map_err(|err| format!("reading file transcript {file_id}: {err}"))?
                    else {
                        continue;
                    };
                    if let Some(cutoff) = since_secs
                        && tr.updated_at < cutoff
                    {
                        continue;
                    }
                    let duration_secs = transcript_duration_secs(&tr.cues);
                    let cue_count = tr.cues.len() as u64;
                    let uri = format!(
                        "slackless://workspace/{}/transcript/{}",
                        ws.as_str(),
                        tr.file_id.as_str()
                    );
                    items.push(TranscriptListItem {
                        kind: "file",
                        workspace: ws.as_str().to_owned(),
                        id: tr.file_id.as_str().to_owned(),
                        source: Some(tr.source),
                        channel: None,
                        channel_name: None,
                        thread_root_ts: None,
                        updated_at: tr.updated_at,
                        started_at_ms: None,
                        ended_at_ms: None,
                        duration_secs,
                        cue_count,
                        languages: Vec::new(),
                        uri: Some(uri),
                        excerpt: collapse_excerpt(&tr.text, MAX_EXCERPT_CHARS),
                    });
                }
            }

            if matches!(kind, "all" | "huddle") {
                let channel_names = archive.channel_names(ws).unwrap_or_default();
                let sessions = archive
                    .store()
                    .list_huddle_transcript_sessions(
                        ws,
                        resolved_channel_id.as_ref(),
                        limit.max(50),
                    )
                    .map_err(|err| format!("listing huddle transcripts for {ws}: {err}"))?;
                for session in sessions {
                    let ended_secs = session.ended_at_ms.div_euclid(1_000);
                    if let Some(cutoff) = since_secs
                        && ended_secs < cutoff
                    {
                        continue;
                    }
                    let duration_secs = Some(
                        ((session.ended_at_ms - session.started_at_ms).max(0) as f64) / 1000.0,
                    );
                    let ch_name = channel_names.get(session.channel.as_str()).cloned();
                    items.push(TranscriptListItem {
                        kind: "huddle",
                        workspace: ws.as_str().to_owned(),
                        id: session.call_id,
                        source: None,
                        channel: Some(session.channel.as_str().to_owned()),
                        channel_name: ch_name,
                        thread_root_ts: session.thread_root_ts.map(|t| t.as_str().to_owned()),
                        updated_at: ended_secs,
                        started_at_ms: Some(session.started_at_ms),
                        ended_at_ms: Some(session.ended_at_ms),
                        duration_secs,
                        cue_count: session.segment_count,
                        languages: session.languages,
                        uri: None,
                        excerpt: collapse_excerpt(&session.preview, MAX_EXCERPT_CHARS),
                    });
                }
            }
        }

        items.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.workspace.cmp(&b.workspace))
                .then_with(|| a.id.cmp(&b.id))
        });
        items.truncate(limit);
        Ok(items)
    }

    fn tool_list_transcripts(&self, args: &Map<String, Value>) -> Value {
        let workspace_arg = match opt_workspace(args) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let all_workspaces_arg = match opt_bool_optional(args, "all_workspaces") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let kind = match opt_str(args, "kind") {
            Ok(Some(k)) => k.trim().to_ascii_lowercase(),
            Ok(None) => "all".to_owned(),
            Err(msg) => return tool_err("invalid_args", msg),
        };
        if !matches!(kind.as_str(), "all" | "file" | "huddle") {
            return tool_err(
                "invalid_args",
                format!("invalid `kind` '{kind}': expected \"all\", \"file\", or \"huddle\""),
            );
        }
        let channel_arg = match opt_non_empty_str(args, "channel") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let limit = match opt_usize(
            args,
            "limit",
            DEFAULT_TRANSCRIPT_LIST_LIMIT,
            1,
            MAX_TRANSCRIPT_LIST_LIMIT,
        ) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };

        let (all_workspaces, workspaces) =
            match self.resolve_scope_workspaces(workspace_arg.as_deref(), all_workspaces_arg) {
                Ok(v) => v,
                Err((code, msg)) => return tool_err(code, msg),
            };

        let transcripts =
            match self.collect_transcripts(&workspaces, &kind, channel_arg.as_deref(), None, limit)
            {
                Ok(items) => items,
                Err(msg) => return tool_err("store_error", msg),
            };

        let file_count = transcripts.iter().filter(|t| t.kind == "file").count();
        let huddle_count = transcripts.iter().filter(|t| t.kind == "huddle").count();
        let searched: Vec<&str> = workspaces.iter().map(TeamId::as_str).collect();

        tool_ok(&json!({
            "all_workspaces": all_workspaces,
            "workspaces_searched": searched,
            "kind": kind,
            "count": transcripts.len(),
            "file_count": file_count,
            "huddle_count": huddle_count,
            "transcripts": transcripts,
        }))
    }

    fn tool_export_conversation(&self, args: &Map<String, Value>) -> Value {
        let workspace_arg = match opt_workspace(args) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let channel_arg = match opt_non_empty_str(args, "channel") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let thread_ts_arg = match opt_non_empty_str(args, "thread_ts") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let file_id_arg = match opt_non_empty_str(args, "file_id") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let huddle_id_arg = match opt_non_empty_str(args, "huddle_id") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let format_raw = match opt_str(args, "format") {
            Ok(Some(s)) => s.trim().to_ascii_lowercase(),
            Ok(None) => "md".to_owned(),
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let (export_fmt, fmt_label) = match format_raw.as_str() {
            "md" | "markdown" => (ExportFormat::Markdown, "md"),
            "json" => (ExportFormat::Json, "json"),
            other => {
                return tool_err(
                    "invalid_args",
                    format!("invalid `format` '{other}': expected \"md\" or \"json\""),
                );
            }
        };
        let include_threads = match opt_bool(args, "include_threads", false) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let include_deleted = match opt_bool(args, "include_deleted", false) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let include_transcripts = match opt_bool(args, "include_transcripts", true) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let limit = match opt_usize(args, "limit", DEFAULT_EXPORT_LIMIT, 1, MAX_EXPORT_LIMIT) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };

        if thread_ts_arg.is_some() && channel_arg.is_none() {
            return tool_err("invalid_args", "`thread_ts` requires `channel`");
        }
        let target_count = usize::from(channel_arg.is_some())
            + usize::from(file_id_arg.is_some())
            + usize::from(huddle_id_arg.is_some());
        if target_count == 0 {
            return tool_err(
                "invalid_args",
                "provide exactly one export target: `channel` (optionally with `thread_ts`), `file_id`, or `huddle_id`",
            );
        }
        if target_count > 1 {
            return tool_err(
                "invalid_args",
                "`channel`, `file_id`, and `huddle_id` are mutually exclusive",
            );
        }

        if let Some(file_id) = file_id_arg {
            let candidates = match self.workspace_candidates(workspace_arg.as_deref()) {
                Ok(v) => v,
                Err(msg) => return tool_err("invalid_args", msg),
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
                match export_file_transcript(archive.store(), ws, &fid, export_fmt) {
                    Ok(Some(artifact)) => {
                        return tool_ok(&json!({
                            "workspace": ws.as_str(),
                            "target": "file_transcript",
                            "file_id": file_id,
                            "format": fmt_label,
                            "title": artifact.title,
                            "suggested_filename": artifact.suggested_filename,
                            "item_count": artifact.item_count,
                            "content": artifact.content,
                        }));
                    }
                    Ok(None) => continue,
                    Err(err) => {
                        return tool_err(
                            "store_error",
                            format!("exporting file transcript '{file_id}': {err}"),
                        );
                    }
                }
            }
            return tool_err(
                "not_found",
                format!("no stored file transcript found for file_id '{file_id}'"),
            );
        }

        if let Some(huddle_id) = huddle_id_arg {
            let candidates = match self.workspace_candidates(workspace_arg.as_deref()) {
                Ok(v) => v,
                Err(msg) => return tool_err("invalid_args", msg),
            };
            if candidates.is_empty() {
                return tool_err(
                    "no_workspace",
                    "no local archive workspaces found; pass `workspace` or sync the desktop app first",
                );
            }
            for ws in &candidates {
                let archive = match self.open_archive(ws) {
                    Ok(Some(a)) => a,
                    Ok(None) => continue,
                    Err(msg) => return tool_err("store_error", msg),
                };
                match export_huddle_transcript(archive.store(), ws, &huddle_id, export_fmt) {
                    Ok(Some(artifact)) => {
                        return tool_ok(&json!({
                            "workspace": ws.as_str(),
                            "target": "huddle_transcript",
                            "huddle_id": huddle_id,
                            "format": fmt_label,
                            "title": artifact.title,
                            "suggested_filename": artifact.suggested_filename,
                            "item_count": artifact.item_count,
                            "content": artifact.content,
                        }));
                    }
                    Ok(None) => continue,
                    Err(err) => {
                        return tool_err(
                            "store_error",
                            format!("exporting huddle transcript '{huddle_id}': {err}"),
                        );
                    }
                }
            }
            return tool_err(
                "not_found",
                format!("no stored huddle transcript found for huddle_id '{huddle_id}'"),
            );
        }

        let channel_arg = channel_arg.expect("checked channel_arg.is_some()");
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
        let export_opts = ExportOptions {
            format: export_fmt,
            limit,
            include_deleted,
            include_threads,
            include_transcripts,
        };

        let (channel_id, artifact) = match archive.as_ref() {
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
                let artifact = match thread_ts_arg.as_deref() {
                    Some(ts) => export_thread(
                        archive.store(),
                        &workspace,
                        &channel_id,
                        &Ts::from(ts),
                        &export_opts,
                    ),
                    None => export_channel(archive.store(), &workspace, &channel_id, &export_opts),
                };
                match artifact {
                    Ok(a) => (channel_id, a),
                    Err(err) => {
                        return tool_err("store_error", format!("exporting archive: {err}"));
                    }
                }
            }
            None => {
                let channel_id = raw_channel_id(&channel_arg);
                let fallback = match Store::open_in_memory() {
                    Ok(s) => s,
                    Err(err) => {
                        return tool_err("store_error", format!("opening fallback store: {err}"));
                    }
                };
                let artifact = match thread_ts_arg.as_deref() {
                    Some(ts) => export_thread(
                        &fallback,
                        &workspace,
                        &channel_id,
                        &Ts::from(ts),
                        &export_opts,
                    ),
                    None => export_channel(&fallback, &workspace, &channel_id, &export_opts),
                };
                match artifact {
                    Ok(a) => (channel_id, a),
                    Err(err) => {
                        return tool_err("store_error", format!("exporting archive: {err}"));
                    }
                }
            }
        };

        let target_kind = if thread_ts_arg.is_some() {
            "thread"
        } else {
            "channel"
        };

        tool_ok(&json!({
            "workspace": workspace.as_str(),
            "target": target_kind,
            "channel": channel_id.as_str(),
            "thread_ts": thread_ts_arg,
            "format": fmt_label,
            "title": artifact.title,
            "suggested_filename": artifact.suggested_filename,
            "item_count": artifact.item_count,
            "content": artifact.content,
        }))
    }

    fn collect_catchup_summary(
        &self,
        all_workspaces: bool,
        workspaces: &[TeamId],
        since_hours: usize,
        limit: usize,
        explicit_user_id: Option<&str>,
    ) -> std::result::Result<CatchupSummaryData, String> {
        let now = now_epoch_secs();
        let cutoff_secs = now.saturating_sub((since_hours as i64).saturating_mul(3_600));

        let session_users: HashMap<TeamId, String> = self
            .load_sessions()
            .into_iter()
            .filter(|s| !s.user_id.as_str().is_empty())
            .map(|s| (s.team_id, s.user_id.as_str().to_owned()))
            .collect();

        let mut total_messages: u64 = 0;
        let mut channels: Vec<CatchupChannelEntry> = Vec::new();
        let mut mentions: Vec<CatchupMentionEntry> = Vec::new();
        let mut active_threads: Vec<CatchupThreadEntry> = Vec::new();

        for ws in workspaces {
            let Some(archive) = self.open_archive(ws)? else {
                continue;
            };

            let user_names = archive.user_display_map(ws).unwrap_or_default();
            let channel_names = archive.channel_names(ws).unwrap_or_default();
            let notify = archive.notify_levels(ws).unwrap_or_default();

            // 1. Active & unread channels in the window.
            let conv_map: HashMap<ChannelId, slackless_core::types::Conversation> = archive
                .store()
                .conversations(ws)
                .unwrap_or_default()
                .into_iter()
                .map(|c| (c.id.clone(), c))
                .collect();
            let digest_rows = archive
                .digest_since(ws, cutoff_secs, limit)
                .map_err(|err| format!("building digest for {ws}: {err}"))?;
            let mut seen_channels: HashSet<ChannelId> = HashSet::new();
            for row in digest_rows {
                total_messages = total_messages.saturating_add(row.message_count);
                let ch_id = ChannelId::from(row.channel.as_str());
                seen_channels.insert(ch_id.clone());
                let conv = conv_map.get(&ch_id);
                let unread_count = conv
                    .and_then(|c| c.unread_count_display.or(c.unread_count))
                    .unwrap_or(0);
                let newer_than_read = conv
                    .and_then(|c| c.last_read.as_ref())
                    .is_some_and(|lr| Ts::from(row.latest_ts.as_str()) > *lr);
                let is_unread = unread_count > 0 || newer_than_read;
                let muted = notify.get(&ch_id) == Some(&NotifyLevel::Muted);
                channels.push(CatchupChannelEntry {
                    workspace: ws.as_str().to_owned(),
                    channel: row.channel,
                    label: row.label,
                    message_count: row.message_count,
                    author_count: row.author_count,
                    unread_count,
                    is_unread,
                    muted,
                    latest_ts: Some(row.latest_ts),
                    latest_author: row.latest_author,
                    latest_text: row.latest_text,
                });
            }
            // Include unread conversations that had unread_count > 0 locally even without messages
            // in the time-window digest.
            for (ch_id, conv) in &conv_map {
                if seen_channels.contains(ch_id) || conv.is_archived {
                    continue;
                }
                let unread_count = conv.unread_count_display.or(conv.unread_count).unwrap_or(0);
                if unread_count == 0 {
                    continue;
                }
                let label = if let Some(name) = conv.name.as_deref() {
                    format!("#{name}")
                } else if conv.is_im {
                    conv.user
                        .as_ref()
                        .and_then(|u| user_names.get(u))
                        .map(|n| format!("@{n}"))
                        .unwrap_or_else(|| ch_id.as_str().to_owned())
                } else {
                    ch_id.as_str().to_owned()
                };
                let muted = notify.get(ch_id) == Some(&NotifyLevel::Muted);
                let latest_ts = conv.latest.as_ref().map(|l| l.ts().as_str().to_owned());
                channels.push(CatchupChannelEntry {
                    workspace: ws.as_str().to_owned(),
                    channel: ch_id.as_str().to_owned(),
                    label,
                    message_count: 0,
                    author_count: 0,
                    unread_count,
                    is_unread: true,
                    muted,
                    latest_ts,
                    latest_author: None,
                    latest_text: String::new(),
                });
            }

            // 2. Direct mentions of the workspace user within the window.
            let mention_uid = explicit_user_id
                .map(str::to_owned)
                .or_else(|| session_users.get(ws).cloned());
            if let Some(uid) = mention_uid {
                let mut q = SearchQuery::default();
                q.filters.includes_user = Some(uid);
                q.filters.include_deleted = false;
                let hits = archive
                    .store()
                    .search_sorted(ws, &q, SearchSort::Recent, limit.saturating_mul(2))
                    .map_err(|err| format!("searching mentions for {ws}: {err}"))?;
                for stored in hits {
                    if stored.message.ts.secs().is_none_or(|s| s < cutoff_secs) {
                        continue;
                    }
                    let ch_name = channel_names.get(stored.channel.as_str()).cloned();
                    let u_name = stored
                        .message
                        .user
                        .as_ref()
                        .and_then(|u| user_names.get(u).cloned());
                    let plain = mrkdwn_to_plain(&stored.message.text);
                    let body = if plain.trim().is_empty() {
                        &stored.message.text
                    } else {
                        &plain
                    };
                    mentions.push(CatchupMentionEntry {
                        workspace: ws.as_str().to_owned(),
                        channel: stored.channel.as_str().to_owned(),
                        channel_name: ch_name,
                        ts: stored.message.ts.as_str().to_owned(),
                        thread_ts: stored
                            .message
                            .thread_ts
                            .as_ref()
                            .map(|t| t.as_str().to_owned()),
                        user: stored.message.user.as_ref().map(|u| u.as_str().to_owned()),
                        user_name: u_name,
                        excerpt: collapse_excerpt(body, MAX_EXCERPT_CHARS),
                    });
                }
            }

            // 3. Active threads in the window.
            let thread_roots = archive
                .store()
                .recent_threads(ws, None, limit.saturating_mul(3).max(50))
                .map_err(|err| format!("loading active threads for {ws}: {err}"))?;
            for root in thread_roots {
                let latest_activity = if let Some(lr) = root.message.latest_reply.clone() {
                    lr
                } else {
                    archive
                        .store()
                        .thread_replies(ws, &root.channel, &root.message.ts)
                        .ok()
                        .and_then(|replies| {
                            replies
                                .into_iter()
                                .rev()
                                .find(|m| !m.deleted)
                                .map(|m| m.message.ts)
                        })
                        .unwrap_or_else(|| root.message.ts.clone())
                };
                let active_in_window = latest_activity.secs().is_some_and(|s| s >= cutoff_secs)
                    || root.message.ts.secs().is_some_and(|s| s >= cutoff_secs);
                if !active_in_window {
                    continue;
                }
                let ch_name = channel_names.get(root.channel.as_str()).cloned();
                let author = root
                    .message
                    .user
                    .as_ref()
                    .and_then(|u| user_names.get(u).cloned())
                    .or_else(|| root.message.user.as_ref().map(|u| u.as_str().to_owned()));
                let plain = mrkdwn_to_plain(&root.message.text);
                let body = if plain.trim().is_empty() {
                    &root.message.text
                } else {
                    &plain
                };
                active_threads.push(CatchupThreadEntry {
                    workspace: ws.as_str().to_owned(),
                    channel: root.channel.as_str().to_owned(),
                    channel_name: ch_name,
                    thread_ts: root.message.ts.as_str().to_owned(),
                    reply_count: root.message.reply_count.unwrap_or(0),
                    latest_activity_ts: latest_activity.as_str().to_owned(),
                    author,
                    root_excerpt: collapse_excerpt(body, MAX_EXCERPT_CHARS),
                });
            }
        }

        // 4. New file and huddle transcripts in the window.
        let new_transcripts =
            self.collect_transcripts(workspaces, "all", None, Some(cutoff_secs), limit)?;

        channels.sort_by(|a, b| {
            b.is_unread
                .cmp(&a.is_unread)
                .then_with(|| b.message_count.cmp(&a.message_count))
                .then_with(|| b.latest_ts.cmp(&a.latest_ts))
        });
        channels.truncate(limit);

        mentions.sort_by(|a, b| b.ts.cmp(&a.ts));
        mentions.truncate(limit);

        active_threads.sort_by(|a, b| b.latest_activity_ts.cmp(&a.latest_activity_ts));
        active_threads.truncate(limit);

        Ok(CatchupSummaryData {
            since_hours,
            cutoff_secs,
            all_workspaces,
            workspaces: workspaces.iter().map(|w| w.as_str().to_owned()).collect(),
            total_messages,
            channels,
            mentions,
            active_threads,
            new_transcripts,
        })
    }

    fn tool_get_catchup_summary(&self, args: &Map<String, Value>) -> Value {
        let workspace_arg = match opt_workspace(args) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let all_workspaces_arg = match opt_bool_optional(args, "all_workspaces") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let since_hours = match opt_usize(
            args,
            "since_hours",
            DEFAULT_CATCHUP_HOURS,
            1,
            MAX_CATCHUP_HOURS,
        ) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let limit = match opt_usize(args, "limit", DEFAULT_CATCHUP_LIMIT, 1, MAX_CATCHUP_LIMIT) {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };
        let user_id_arg = match opt_non_empty_str(args, "user_id") {
            Ok(v) => v,
            Err(msg) => return tool_err("invalid_args", msg),
        };

        let (all_workspaces, workspaces) =
            match self.resolve_scope_workspaces(workspace_arg.as_deref(), all_workspaces_arg) {
                Ok(v) => v,
                Err((code, msg)) => return tool_err(code, msg),
            };

        let data = match self.collect_catchup_summary(
            all_workspaces,
            &workspaces,
            since_hours,
            limit,
            user_id_arg.as_deref(),
        ) {
            Ok(d) => d,
            Err(msg) => return tool_err("store_error", msg),
        };

        let unread_channels = data.channels.iter().filter(|c| c.is_unread).count();

        tool_ok(&json!({
            "since_hours": data.since_hours,
            "cutoff_secs": data.cutoff_secs,
            "all_workspaces": data.all_workspaces,
            "workspaces": data.workspaces,
            "totals": {
                "messages": data.total_messages,
                "active_channels": data.channels.len(),
                "unread_channels": unread_channels,
                "mentions": data.mentions.len(),
                "active_threads": data.active_threads.len(),
                "new_transcripts": data.new_transcripts.len(),
            },
            "channels": data.channels,
            "mentions": data.mentions,
            "active_threads": data.active_threads,
            "new_transcripts": data.new_transcripts,
        }))
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

    // -------------------------------------------------------------------------
    // MCP Resources (`resources/read`)
    // -------------------------------------------------------------------------

    fn handle_resources_read(
        &self,
        params: &Map<String, Value>,
    ) -> std::result::Result<Value, RpcError> {
        let Some(uri) = params
            .get("uri")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Err(RpcError::invalid_params(
                "resources/read requires a non-empty string `uri`",
            ));
        };

        let parsed = parse_resource_uri(uri)?;
        let payload = match parsed {
            ResourceUri::Workspaces => self.list_workspaces_payload(),
            ResourceUri::WorkspaceChannels { workspace } => self
                .list_channels_payload(&workspace, "all", None, true, DEFAULT_CHANNEL_LIST_LIMIT)
                .map_err(RpcError::internal)?,
            ResourceUri::Channel { workspace, channel } => {
                self.read_channel_resource(&workspace, &channel)?
            }
            ResourceUri::Thread {
                workspace,
                channel,
                thread_ts,
            } => self.read_thread_resource(&workspace, &channel, &thread_ts)?,
            ResourceUri::Transcript { workspace, file_id } => {
                let archive = self.open_archive(&workspace).map_err(RpcError::internal)?;
                let Some(archive) = archive else {
                    return Err(RpcError::resource_not_found(format!(
                        "no local archive for workspace '{workspace}'; transcript '{file_id}' not found"
                    )));
                };
                let transcript = archive
                    .store()
                    .file_transcript(&workspace, &file_id)
                    .map_err(|err| RpcError::internal(format!("reading transcript: {err}")))?;
                let Some(transcript) = transcript else {
                    return Err(RpcError::resource_not_found(format!(
                        "no stored transcript found for file_id '{file_id}' in workspace '{workspace}'"
                    )));
                };
                file_transcript_payload(&workspace, &transcript)
            }
        };

        let text = serde_json::to_string_pretty(&payload)
            .map_err(|err| RpcError::internal(format!("serializing resource payload: {err}")))?;
        Ok(json!({
            "contents": [
                {
                    "uri": uri,
                    "mimeType": "application/json",
                    "text": text,
                }
            ]
        }))
    }

    fn read_channel_resource(
        &self,
        workspace: &TeamId,
        channel_arg: &str,
    ) -> std::result::Result<Value, RpcError> {
        let archive = self.open_archive(workspace).map_err(RpcError::internal)?;
        let (resolved, channel_id, stored_messages, user_names) = match archive.as_ref() {
            Some(archive) => {
                let resolved = archive
                    .resolve_channel(workspace, channel_arg)
                    .map_err(|err| RpcError::internal(format!("resolving channel: {err}")))?;
                let channel_id = resolved
                    .as_ref()
                    .map_or_else(|| raw_channel_id(channel_arg), |row| row.id.clone());
                let msgs = read_channel_window(
                    archive.store(),
                    workspace,
                    &channel_id,
                    None,
                    DEFAULT_READ_LIMIT,
                    None,
                    false,
                )
                .map_err(|err| RpcError::internal(format!("reading channel: {err}")))?;
                let user_names = archive.user_display_map(workspace).unwrap_or_default();
                (resolved, channel_id, msgs, user_names)
            }
            None => (
                None,
                raw_channel_id(channel_arg),
                Vec::new(),
                HashMap::new(),
            ),
        };
        let channel_row = match resolved {
            Some(row) => Some(row),
            None => archive
                .as_ref()
                .and_then(|a| a.conversation_by_id(workspace, &channel_id).ok().flatten()),
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
        Ok(json!({
            "workspace": workspace.as_str(),
            "channel": channel_json,
            "count": messages.len(),
            "messages": messages,
        }))
    }

    fn read_thread_resource(
        &self,
        workspace: &TeamId,
        channel_arg: &str,
        thread_ts: &Ts,
    ) -> std::result::Result<Value, RpcError> {
        let archive = self.open_archive(workspace).map_err(RpcError::internal)?;
        let (label, channel_id, mut stored_messages, user_names) = match archive.as_ref() {
            Some(archive) => {
                let resolved = archive
                    .resolve_channel(workspace, channel_arg)
                    .map_err(|err| RpcError::internal(format!("resolving channel: {err}")))?;
                let channel_id = resolved
                    .as_ref()
                    .map_or_else(|| raw_channel_id(channel_arg), |row| row.id.clone());
                let msgs = archive
                    .store()
                    .thread_replies(workspace, &channel_id, thread_ts)
                    .map_err(|err| RpcError::internal(format!("reading thread: {err}")))?;
                let user_names = archive.user_display_map(workspace).unwrap_or_default();
                (resolved.map(|r| r.label()), channel_id, msgs, user_names)
            }
            None => (
                None,
                raw_channel_id(channel_arg),
                Vec::new(),
                HashMap::new(),
            ),
        };
        stored_messages.truncate(DEFAULT_THREAD_LIMIT);
        let messages: Vec<McpMessage> = stored_messages
            .iter()
            .map(|msg| McpMessage::from_stored(msg, &user_names, label.clone(), None))
            .collect();
        Ok(json!({
            "workspace": workspace.as_str(),
            "channel": channel_id.as_str(),
            "thread_ts": thread_ts.as_str(),
            "count": messages.len(),
            "messages": messages,
        }))
    }

    // -------------------------------------------------------------------------
    // MCP Prompts (`prompts/get`)
    // -------------------------------------------------------------------------

    fn handle_prompts_get(
        &self,
        params: &Map<String, Value>,
    ) -> std::result::Result<Value, RpcError> {
        let Some(name) = params
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            return Err(RpcError::invalid_params(
                "prompts/get requires a non-empty string `name`",
            ));
        };
        let args = match params.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(map)) => map.clone(),
            Some(_) => {
                return Err(RpcError::invalid_params(
                    "prompts/get `arguments` must be a JSON object",
                ));
            }
        };

        match name {
            "catch_up" => self.prompt_catch_up(&args),
            "summarize_thread" => self.prompt_summarize_thread(&args),
            "summarize_channel" => self.prompt_summarize_channel(&args),
            "find_decisions" => self.prompt_find_decisions(&args),
            "search_transcripts" => self.prompt_search_transcripts(&args),
            _ => Err(RpcError::invalid_params(format!("unknown prompt: {name}"))),
        }
    }

    fn prompt_catch_up(&self, args: &Map<String, Value>) -> std::result::Result<Value, RpcError> {
        let workspace_arg = prompt_opt_workspace(args)?;
        let since_hours = prompt_opt_usize(
            args,
            "since_hours",
            DEFAULT_CATCHUP_HOURS,
            1,
            MAX_CATCHUP_HOURS,
        )?;
        let (all_workspaces, workspaces) = self
            .resolve_scope_workspaces(workspace_arg.as_deref(), None)
            .map_err(|(_, msg)| RpcError::invalid_params(msg))?;

        let data = self
            .collect_catchup_summary(all_workspaces, &workspaces, since_hours, 15, None)
            .map_err(RpcError::internal)?;

        let mut context = format!(
            "## Local Slack Activity (last {}h)\nWorkspaces: {}\nTotal messages: {}\n\n",
            data.since_hours,
            if data.workspaces.is_empty() {
                "(none)".to_owned()
            } else {
                data.workspaces.join(", ")
            },
            data.total_messages
        );

        context.push_str("### Active & Unread Channels\n");
        if data.channels.is_empty() {
            context.push_str("_No recent channel activity in this window._\n");
        } else {
            for ch in &data.channels {
                let unread_tag = if ch.is_unread {
                    if ch.unread_count > 0 {
                        format!(" [UNREAD: {}]", ch.unread_count)
                    } else {
                        " [UNREAD]".to_owned()
                    }
                } else {
                    String::new()
                };
                let muted_tag = if ch.muted { " [muted]" } else { "" };
                let who = ch.latest_author.as_deref().unwrap_or("unknown");
                context.push_str(&format!(
                    "- [{}] {} ({} msgs, {} people){unread_tag}{muted_tag}: {who}: {}\n",
                    ch.workspace, ch.label, ch.message_count, ch.author_count, ch.latest_text
                ));
            }
        }

        context.push_str("\n### Direct Mentions\n");
        if data.mentions.is_empty() {
            context.push_str("_No direct mentions found in this window._\n");
        } else {
            for m in &data.mentions {
                let ch = m.channel_name.as_deref().unwrap_or(&m.channel);
                let who = m
                    .user_name
                    .as_deref()
                    .or(m.user.as_deref())
                    .unwrap_or("unknown");
                context.push_str(&format!(
                    "- [{}] {} `{}` **{who}**: {}\n",
                    m.workspace, ch, m.ts, m.excerpt
                ));
            }
        }

        context.push_str("\n### Active Threads\n");
        if data.active_threads.is_empty() {
            context.push_str("_No active threads in this window._\n");
        } else {
            for t in &data.active_threads {
                let ch = t.channel_name.as_deref().unwrap_or(&t.channel);
                let who = t.author.as_deref().unwrap_or("unknown");
                context.push_str(&format!(
                    "- [{}] {} `{}` ({} replies) **{who}**: {}\n",
                    t.workspace, ch, t.thread_ts, t.reply_count, t.root_excerpt
                ));
            }
        }

        context.push_str("\n### New Audio/Video & Huddle Transcripts\n");
        if data.new_transcripts.is_empty() {
            context.push_str("_No new transcripts in this window._\n");
        } else {
            for tr in &data.new_transcripts {
                let dur = tr
                    .duration_secs
                    .map(|d| format!(", {d:.0}s"))
                    .unwrap_or_default();
                context.push_str(&format!(
                    "- [{}] ({}) `{}` ({} cues{dur}): {}\n",
                    tr.workspace, tr.kind, tr.id, tr.cue_count, tr.excerpt
                ));
            }
        }

        let bounded = bound_prompt_context(&context, MAX_PROMPT_CONTEXT_CHARS);
        let text = format!(
            "Prepare a concise executive Slack catch-up briefing from the local archive snapshot below.\n\
             Group your response into:\n\
             1. Urgent mentions & items needing my reply\n\
             2. Key channel highlights\n\
             3. Active threads worth following up on\n\
             4. Notable takeaways from new audio/video or huddle transcripts\n\n\
             {bounded}"
        );

        Ok(prompt_result(
            "Summarize recent Slack activity, mentions, active threads, and new transcripts.",
            text,
        ))
    }

    fn prompt_summarize_thread(
        &self,
        args: &Map<String, Value>,
    ) -> std::result::Result<Value, RpcError> {
        let channel_arg = prompt_req_str(args, "channel")?;
        let thread_ts_arg = prompt_req_str(args, "thread_ts")?;
        let workspace_arg = prompt_opt_workspace(args)?;

        let workspace = self
            .resolve_workspace(workspace_arg.as_deref())
            .map_err(RpcError::invalid_params)?
            .ok_or_else(|| {
                RpcError::invalid_params(
                    "no workspace resolved; pass `workspace` in prompt arguments",
                )
            })?;

        let archive = self.open_archive(&workspace).map_err(RpcError::internal)?;
        let (label, channel_id, messages, user_names) = match archive.as_ref() {
            Some(archive) => {
                let resolved = archive
                    .resolve_channel(&workspace, &channel_arg)
                    .map_err(|err| RpcError::internal(format!("resolving channel: {err}")))?;
                let channel_id = resolved
                    .as_ref()
                    .map_or_else(|| raw_channel_id(&channel_arg), |r| r.id.clone());
                let label = resolved
                    .as_ref()
                    .map(|r| r.label())
                    .unwrap_or_else(|| channel_id.as_str().to_owned());
                let mut msgs = archive
                    .store()
                    .thread_replies(&workspace, &channel_id, &Ts::from(thread_ts_arg.as_str()))
                    .map_err(|err| RpcError::internal(format!("reading thread: {err}")))?;
                msgs.retain(|m| !m.deleted);
                msgs.truncate(100);
                let users = archive.user_display_map(&workspace).unwrap_or_default();
                (label, channel_id, msgs, users)
            }
            None => {
                let channel_id = raw_channel_id(&channel_arg);
                (
                    channel_id.as_str().to_owned(),
                    channel_id,
                    Vec::new(),
                    HashMap::new(),
                )
            }
        };

        let mut context = format!(
            "## Thread Context\nWorkspace: `{}` · Channel: {} (`{}`) · Thread: `{}` · Messages: {}\n\n",
            workspace.as_str(),
            label,
            channel_id.as_str(),
            thread_ts_arg,
            messages.len()
        );
        if messages.is_empty() {
            context.push_str("_No archived messages found for this thread._\n");
        } else {
            for msg in &messages {
                context.push_str(&format_prompt_message(msg, &user_names, None));
            }
        }

        let bounded = bound_prompt_context(&context, MAX_PROMPT_CONTEXT_CHARS);
        let text = format!(
            "Summarize the Slack thread below. Provide:\n\
             1. One-sentence TL;DR\n\
             2. Key points raised (with speaker attribution)\n\
             3. Decisions reached\n\
             4. Action items with owners\n\
             5. Open questions or blockers\n\n\
             {bounded}"
        );

        Ok(prompt_result(
            "Summarize a Slack thread with decisions, action items, and open questions.",
            text,
        ))
    }

    fn prompt_summarize_channel(
        &self,
        args: &Map<String, Value>,
    ) -> std::result::Result<Value, RpcError> {
        let channel_arg = prompt_req_str(args, "channel")?;
        let workspace_arg = prompt_opt_workspace(args)?;
        let since = prompt_opt_str(args, "since")?;
        let cutoff_secs = match since.as_deref() {
            Some(spec) => {
                Some(parse_since_cutoff(spec, now_epoch_secs()).map_err(RpcError::invalid_params)?)
            }
            None => None,
        };
        let limit = prompt_opt_usize(args, "limit", DEFAULT_READ_LIMIT, 1, 100)?;

        let workspace = self
            .resolve_workspace(workspace_arg.as_deref())
            .map_err(RpcError::invalid_params)?
            .ok_or_else(|| {
                RpcError::invalid_params(
                    "no workspace resolved; pass `workspace` in prompt arguments",
                )
            })?;

        let archive = self.open_archive(&workspace).map_err(RpcError::internal)?;
        let (label, channel_id, messages, user_names) = match archive.as_ref() {
            Some(archive) => {
                let resolved = archive
                    .resolve_channel(&workspace, &channel_arg)
                    .map_err(|err| RpcError::internal(format!("resolving channel: {err}")))?;
                let channel_id = resolved
                    .as_ref()
                    .map_or_else(|| raw_channel_id(&channel_arg), |r| r.id.clone());
                let label = resolved
                    .as_ref()
                    .map(|r| r.label())
                    .unwrap_or_else(|| channel_id.as_str().to_owned());
                let msgs = read_channel_window(
                    archive.store(),
                    &workspace,
                    &channel_id,
                    None,
                    limit,
                    cutoff_secs,
                    false,
                )
                .map_err(|err| RpcError::internal(format!("reading channel: {err}")))?;
                let users = archive.user_display_map(&workspace).unwrap_or_default();
                (label, channel_id, msgs, users)
            }
            None => {
                let channel_id = raw_channel_id(&channel_arg);
                (
                    channel_id.as_str().to_owned(),
                    channel_id,
                    Vec::new(),
                    HashMap::new(),
                )
            }
        };

        let window_desc = since.as_deref().unwrap_or("recent");
        let mut context = format!(
            "## Channel Context\nWorkspace: `{}` · Channel: {} (`{}`) · Window: {} · Messages: {}\n\n",
            workspace.as_str(),
            label,
            channel_id.as_str(),
            window_desc,
            messages.len()
        );
        if messages.is_empty() {
            context.push_str("_No archived messages found for this channel window._\n");
        } else {
            for msg in &messages {
                context.push_str(&format_prompt_message(msg, &user_names, None));
            }
        }

        let bounded = bound_prompt_context(&context, MAX_PROMPT_CONTEXT_CHARS);
        let text = format!(
            "Summarize the recent discussions in {} below. Highlight:\n\
             1. Main topics and updates\n\
             2. Decisions or consensus reached\n\
             3. Active threads or blockers that need attention\n\
             4. Follow-up actions and owners\n\n\
             {bounded}",
            label
        );

        Ok(prompt_result(
            "Summarize recent discussions, themes, and blockers in a Slack channel.",
            text,
        ))
    }

    fn prompt_find_decisions(
        &self,
        args: &Map<String, Value>,
    ) -> std::result::Result<Value, RpcError> {
        let channel_arg = prompt_opt_str(args, "channel")?;
        let query_arg = prompt_opt_str(args, "query")?;
        let since_arg = prompt_opt_str(args, "since")?;
        let workspace_arg = prompt_opt_workspace(args)?;

        let cutoff_secs = match since_arg.as_deref() {
            Some(spec) => {
                Some(parse_since_cutoff(spec, now_epoch_secs()).map_err(RpcError::invalid_params)?)
            }
            None => None,
        };

        let mut context = String::from("## Candidate Messages for Decision Extraction\n");
        let mut collected = 0usize;

        if let Some(ch_arg) = channel_arg.as_deref()
            && query_arg.is_none()
        {
            let workspace = self
                .resolve_workspace(workspace_arg.as_deref())
                .map_err(RpcError::invalid_params)?
                .ok_or_else(|| {
                    RpcError::invalid_params(
                        "no workspace resolved; pass `workspace` in prompt arguments",
                    )
                })?;
            if let Some(archive) = self.open_archive(&workspace).map_err(RpcError::internal)? {
                let resolved = archive
                    .resolve_channel(&workspace, ch_arg)
                    .map_err(|err| RpcError::internal(format!("resolving channel: {err}")))?;
                let ch_id = resolved
                    .as_ref()
                    .map_or_else(|| raw_channel_id(ch_arg), |r| r.id.clone());
                let label = resolved
                    .as_ref()
                    .map(|r| r.label())
                    .unwrap_or_else(|| ch_id.as_str().to_owned());
                let msgs = read_channel_window(
                    archive.store(),
                    &workspace,
                    &ch_id,
                    None,
                    80,
                    cutoff_secs,
                    false,
                )
                .map_err(|err| RpcError::internal(format!("reading channel: {err}")))?;
                let users = archive.user_display_map(&workspace).unwrap_or_default();
                context.push_str(&format!(
                    "Scope: workspace `{}` · channel {} (`{}`)\n\n",
                    workspace.as_str(),
                    label,
                    ch_id.as_str()
                ));
                for msg in &msgs {
                    context.push_str(&format_prompt_message(msg, &users, Some(&label)));
                    collected += 1;
                }
            }
        } else {
            let effective_query = query_arg.as_deref().unwrap_or("decided");
            let base_parsed = SearchQuery::parse(effective_query);
            let (_, workspaces) = self
                .resolve_scope_workspaces(workspace_arg.as_deref(), None)
                .map_err(|(_, msg)| RpcError::invalid_params(msg))?;
            if let Some(ch_arg) = channel_arg.as_deref() {
                context.push_str(&format!(
                    "Query: `{effective_query}` · Channel: `{ch_arg}`\n\n"
                ));
            } else {
                context.push_str(&format!("Query: `{effective_query}`\n\n"));
            }
            for ws in &workspaces {
                let Some(archive) = self.open_archive(ws).map_err(RpcError::internal)? else {
                    continue;
                };
                let mut ws_query = base_parsed.clone();
                if let Some(ch_arg) = channel_arg.as_deref() {
                    let resolved = archive
                        .resolve_channel(ws, ch_arg)
                        .map_err(|err| RpcError::internal(format!("resolving channel: {err}")))?;
                    let ch_id = resolved
                        .as_ref()
                        .map_or_else(|| raw_channel_id(ch_arg), |r| r.id.clone());
                    ws_query.filters.in_channels.push(ch_id.as_str().to_owned());
                }
                let hits = archive
                    .store()
                    .search_sorted(ws, &ws_query, SearchSort::Recent, 40)
                    .map_err(|err| RpcError::internal(format!("searching {ws}: {err}")))?;
                if hits.is_empty() {
                    continue;
                }
                let users = archive.user_display_map(ws).unwrap_or_default();
                let ch_names = archive.channel_names(ws).unwrap_or_default();
                for msg in &hits {
                    if let Some(cutoff) = cutoff_secs
                        && msg.message.ts.secs().is_none_or(|s| s < cutoff)
                    {
                        continue;
                    }
                    let ch_label = ch_names
                        .get(msg.channel.as_str())
                        .map(String::as_str)
                        .unwrap_or(msg.channel.as_str());
                    context.push_str(&format!(
                        "[{}] {}",
                        ws.as_str(),
                        format_prompt_message(msg, &users, Some(ch_label))
                    ));
                    collected += 1;
                    if collected >= 50 {
                        break;
                    }
                }
                if collected >= 50 {
                    break;
                }
            }
        }

        if collected == 0 {
            context.push_str("_No matching archived messages found._\n");
        }

        let bounded = bound_prompt_context(&context, MAX_PROMPT_CONTEXT_CHARS);
        let text = format!(
            "Analyze the Slack messages below and extract every concrete or tentative decision.\n\
             For each decision, provide:\n\
             - **Decision:** concise statement\n\
             - **Status:** Decided / Tentative / Deferred\n\
             - **Owner / Approver:** who decided or owns execution\n\
             - **Rationale & Citation:** brief reason plus channel and `ts`\n\
             - **Open Questions:** any unresolved caveats\n\n\
             {bounded}"
        );

        Ok(prompt_result(
            "Extract decisions, owners, rationale, and open questions from archived Slack context.",
            text,
        ))
    }

    fn prompt_search_transcripts(
        &self,
        args: &Map<String, Value>,
    ) -> std::result::Result<Value, RpcError> {
        let query = prompt_req_str(args, "query")?;
        let workspace_arg = prompt_opt_workspace(args)?;

        let (_, workspaces) = self
            .resolve_scope_workspaces(workspace_arg.as_deref(), None)
            .map_err(|(_, msg)| RpcError::invalid_params(msg))?;

        let parsed = SearchQuery::parse(&query);
        let mut context = format!("## Transcript Matches for `{query}`\n\n");
        let mut file_hits = 0usize;
        let mut huddle_hits = 0usize;

        context.push_str("### Audio/Video File Transcripts\n");
        for ws in &workspaces {
            let Some(archive) = self.open_archive(ws).map_err(RpcError::internal)? else {
                continue;
            };
            let matches = archive
                .store()
                .search_file_transcripts(Some(ws), &parsed, 15)
                .map_err(|err| RpcError::internal(format!("searching file transcripts: {err}")))?;
            for entry in matches {
                let dur = transcript_duration_secs(&entry.transcript.cues)
                    .map(|d| format!(" · {d:.0}s"))
                    .unwrap_or_default();
                let ch_part = entry
                    .channel
                    .as_ref()
                    .map(|c| format!(" · channel `{}`", c.as_str()))
                    .unwrap_or_default();
                let body = collapse_excerpt(&entry.transcript.text, 800);
                context.push_str(&format!(
                    "- [{}] file `{}` (`{}`{dur}{ch_part}): {body}\n",
                    ws.as_str(),
                    entry.transcript.file_id.as_str(),
                    entry.transcript.source
                ));
                file_hits += 1;
                if file_hits >= 15 {
                    break;
                }
            }
            if file_hits >= 15 {
                break;
            }
        }
        if file_hits == 0 {
            context.push_str("_No matching audio/video file transcripts._\n");
        }

        context.push_str("\n### Huddle Transcripts\n");
        for ws in &workspaces {
            let Some(archive) = self.open_archive(ws).map_err(RpcError::internal)? else {
                continue;
            };
            let users = archive.user_display_map(ws).unwrap_or_default();
            let ch_names = archive.channel_names(ws).unwrap_or_default();
            let segments = archive
                .store()
                .search_huddle_transcripts(ws, &query, None, None, 25)
                .map_err(|err| {
                    RpcError::internal(format!("searching huddle transcripts: {err}"))
                })?;
            for seg in segments {
                let ch = ch_names
                    .get(seg.channel.as_str())
                    .map(String::as_str)
                    .unwrap_or(seg.channel.as_str());
                let speaker = seg
                    .speaker_id
                    .as_ref()
                    .and_then(|u| users.get(u).map(String::as_str))
                    .or_else(|| seg.speaker_id.as_ref().map(|u| u.as_str()))
                    .unwrap_or("Speaker");
                let stamp = format_utc_timestamp(seg.at_ms.div_euclid(1_000));
                context.push_str(&format!(
                    "- [{}] huddle `{}` in {} ({stamp}) **{speaker}**: {}\n",
                    ws.as_str(),
                    seg.call_id,
                    ch,
                    seg.text
                ));
                huddle_hits += 1;
                if huddle_hits >= 25 {
                    break;
                }
            }
            if huddle_hits >= 25 {
                break;
            }
        }
        if huddle_hits == 0 {
            context.push_str("_No matching huddle transcript segments._\n");
        }

        let bounded = bound_prompt_context(&context, MAX_PROMPT_CONTEXT_CHARS);
        let text = format!(
            "Synthesize what was said about `{query}` across the stored audio/video clip and huddle transcripts below.\n\
             Cite specific `file_id`s or huddle `call_id`s, speakers, and timestamps, and note any action items or open questions.\n\n\
             {bounded}"
        );

        Ok(prompt_result(
            "Search and synthesize audio/video and huddle transcripts for a topic.",
            text,
        ))
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
            },
            "resources": {
                "subscribe": false,
                "listChanged": false
            },
            "prompts": {
                "listChanged": false
            }
        },
        "serverInfo": {
            "name": "slackless",
            "version": CLI_VERSION
        },
        "instructions": "Query local Slack SQLite archives offline (workspaces, channels, FTS5 search including audio/video file transcripts, channel history, threads, transcripts, Markdown/JSON exports, and catch-up digests), read MCP resources (`slackless://...`), invoke built-in prompts (`catch_up`, `summarize_thread`, `summarize_channel`, `find_decisions`, `search_transcripts`), and perform live Slack writes (send, reply, react) using stored slackless sessions."
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

fn opt_non_empty_str(
    args: &Map<String, Value>,
    key: &str,
) -> std::result::Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Err(format!("`{key}` must be a non-empty string when provided"))
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

fn prompt_req_str(args: &Map<String, Value>, key: &str) -> std::result::Result<String, RpcError> {
    req_str(args, key).map_err(RpcError::invalid_params)
}

fn prompt_opt_str(
    args: &Map<String, Value>,
    key: &str,
) -> std::result::Result<Option<String>, RpcError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        Some(_) => Err(RpcError::invalid_params(format!(
            "`{key}` must be a string"
        ))),
    }
}

fn prompt_opt_workspace(
    args: &Map<String, Value>,
) -> std::result::Result<Option<String>, RpcError> {
    opt_workspace(args).map_err(RpcError::invalid_params)
}

fn prompt_opt_usize(
    args: &Map<String, Value>,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> std::result::Result<usize, RpcError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Number(n)) => {
            let Some(val) = n.as_u64() else {
                return Err(RpcError::invalid_params(format!(
                    "`{key}` must be a positive integer"
                )));
            };
            Ok(usize::try_from(val).unwrap_or(max).clamp(min, max))
        }
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(default);
            }
            let val: u64 = trimmed.parse().map_err(|_| {
                RpcError::invalid_params(format!("`{key}` must be a positive integer"))
            })?;
            Ok(usize::try_from(val).unwrap_or(max).clamp(min, max))
        }
        Some(_) => Err(RpcError::invalid_params(format!(
            "`{key}` must be a positive integer"
        ))),
    }
}

fn file_transcript_payload(workspace: &TeamId, transcript: &StoredFileTranscript) -> Value {
    let duration_secs = transcript_duration_secs(&transcript.cues);
    json!({
        "workspace": workspace.as_str(),
        "file_id": transcript.file_id.as_str(),
        "source": transcript.source,
        "text": transcript.text,
        "updated_at": transcript.updated_at,
        "cue_count": transcript.cues.len(),
        "duration_secs": duration_secs,
        "cues": transcript.cues,
    })
}

fn transcript_duration_secs(cues: &[TranscriptCue]) -> Option<f64> {
    let mut max_secs: Option<f64> = None;
    for cue in cues {
        let candidate = if cue.end_secs.is_finite() && cue.end_secs >= 0.0 {
            Some(cue.end_secs)
        } else if cue.start_secs.is_finite() && cue.start_secs >= 0.0 {
            Some(cue.start_secs)
        } else {
            None
        };
        if let Some(secs) = candidate {
            max_secs = Some(max_secs.map_or(secs, |prev| prev.max(secs)));
        }
    }
    max_secs
}

fn collapse_excerpt(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if max_chars == 0 {
        return String::new();
    }
    if collapsed.chars().count() > max_chars {
        let mut out: String = collapsed
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect();
        out.push('…');
        out
    } else {
        collapsed
    }
}

fn bound_prompt_context(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let suffix = "\n\n... [context truncated to stay within prompt budget]";
    let keep = max_chars.saturating_sub(suffix.chars().count());
    let mut out: String = text.chars().take(keep).collect();
    out.push_str(suffix);
    out
}

fn format_prompt_message(
    stored: &StoredMessage,
    user_names: &HashMap<slackless_core::types::UserId, String>,
    channel_label: Option<&str>,
) -> String {
    let msg = &stored.message;
    let author = msg
        .user
        .as_ref()
        .and_then(|u| user_names.get(u).map(String::as_str))
        .or_else(|| msg.bot_profile.as_ref().map(|b| b.name.as_str()))
        .or(msg.username.as_deref())
        .or_else(|| msg.user.as_ref().map(|u| u.as_str()))
        .or_else(|| msg.bot_id.as_ref().map(|b| b.as_str()))
        .unwrap_or("unknown");
    let stamp = msg
        .ts
        .secs()
        .map(format_utc_timestamp)
        .unwrap_or_else(|| msg.ts.as_str().to_owned());
    let ch_prefix = channel_label
        .map(|label| format!("{label} · "))
        .unwrap_or_default();
    let plain = mrkdwn_to_plain(&msg.text);
    let body = if plain.trim().is_empty() {
        collapse_excerpt(&msg.text, 600)
    } else {
        collapse_excerpt(&plain, 600)
    };
    let replies = msg
        .reply_count
        .filter(|&n| n > 0)
        .map(|n| format!(" ({n} replies)"))
        .unwrap_or_default();
    let files = if msg.files.is_empty() {
        String::new()
    } else {
        let names: Vec<&str> = msg
            .files
            .iter()
            .map(|f| {
                f.name
                    .as_deref()
                    .or(f.title.as_deref())
                    .unwrap_or(f.id.as_str())
            })
            .collect();
        format!(" [files: {}]", names.join(", "))
    };
    format!(
        "- {ch_prefix}**{author}** ({stamp} · `{}`){replies}: {body}{files}\n",
        msg.ts.as_str()
    )
}

fn prompt_result(description: &str, text: String) -> Value {
    json!({
        "description": description,
        "messages": [
            {
                "role": "user",
                "content": {
                    "type": "text",
                    "text": text,
                }
            }
        ]
    })
}

fn percent_decode_segment(raw: &str) -> std::result::Result<String, String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(format!("invalid percent-encoding in URI segment '{raw}'"));
            }
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            match (hi, lo) {
                (Some(h), Some(l)) => {
                    out.push(((h << 4) | l) as u8);
                    i += 3;
                }
                _ => {
                    return Err(format!("invalid percent-encoding in URI segment '{raw}'"));
                }
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let decoded =
        String::from_utf8(out).map_err(|_| format!("invalid UTF-8 in URI segment '{raw}'"))?;
    if decoded.is_empty()
        || decoded == "."
        || decoded == ".."
        || decoded.contains('/')
        || decoded.contains('\\')
        || decoded.bytes().any(|b| b < 0x20 || b == 0x7f)
    {
        return Err(format!("unsafe or empty URI segment '{raw}'"));
    }
    Ok(decoded)
}

fn parse_resource_uri(uri: &str) -> std::result::Result<ResourceUri, RpcError> {
    if uri.bytes().any(|b| b < 0x20 || b == 0x7f) || uri.contains('\\') {
        return Err(RpcError::invalid_params(format!(
            "invalid resource URI '{uri}'"
        )));
    }
    let Some(rest) = uri.strip_prefix("slackless://") else {
        return Err(RpcError::invalid_params(format!(
            "unsupported URI scheme in '{uri}': expected slackless://"
        )));
    };
    if rest.contains('?') || rest.contains('#') {
        return Err(RpcError::invalid_params(format!(
            "unexpected query or fragment in resource URI '{uri}'"
        )));
    }
    if rest == "workspaces" {
        return Ok(ResourceUri::Workspaces);
    }

    let segments: Vec<&str> = rest.split('/').collect();
    if segments
        .iter()
        .any(|s| s.is_empty() || *s == "." || *s == "..")
    {
        return Err(RpcError::invalid_params(format!(
            "invalid path segment in resource URI '{uri}'"
        )));
    }

    match segments.as_slice() {
        ["workspace", ws_raw, "channels"] => {
            let ws = validate_workspace_id(ws_raw).map_err(RpcError::invalid_params)?;
            Ok(ResourceUri::WorkspaceChannels {
                workspace: TeamId::from(ws),
            })
        }
        ["workspace", ws_raw, "channel", ch_raw] => {
            let ws = validate_workspace_id(ws_raw).map_err(RpcError::invalid_params)?;
            let channel = percent_decode_segment(ch_raw).map_err(RpcError::invalid_params)?;
            Ok(ResourceUri::Channel {
                workspace: TeamId::from(ws),
                channel,
            })
        }
        ["workspace", ws_raw, "thread", ch_raw, ts_raw] => {
            let ws = validate_workspace_id(ws_raw).map_err(RpcError::invalid_params)?;
            let channel = percent_decode_segment(ch_raw).map_err(RpcError::invalid_params)?;
            let ts = percent_decode_segment(ts_raw).map_err(RpcError::invalid_params)?;
            Ok(ResourceUri::Thread {
                workspace: TeamId::from(ws),
                channel,
                thread_ts: Ts::from(ts),
            })
        }
        ["workspace", ws_raw, "transcript", file_raw] => {
            let ws = validate_workspace_id(ws_raw).map_err(RpcError::invalid_params)?;
            let file_id = percent_decode_segment(file_raw).map_err(RpcError::invalid_params)?;
            Ok(ResourceUri::Transcript {
                workspace: TeamId::from(ws),
                file_id: FileId::from(file_id),
            })
        }
        _ => Err(RpcError::invalid_params(format!(
            "unknown or malformed resource URI '{uri}'"
        ))),
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
            "description": "Fetch a stored audio/video file transcript (including timed cues and duration when available) from the local SQLite archive by Slack `file_id`.",
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
            "name": "list_transcripts",
            "description": "List stored audio/video file transcripts and huddle transcript sessions for one or all workspaces with excerpts, durations, and timed cue counts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace": {
                        "type": "string",
                        "description": "Optional workspace team ID."
                    },
                    "all_workspaces": {
                        "type": "boolean",
                        "description": "List transcripts across every locally archived workspace."
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["all", "file", "huddle"],
                        "description": "Transcript kinds to return (default: \"all\")."
                    },
                    "channel": {
                        "type": "string",
                        "description": "Optional channel ID, #name, or @handle to filter transcripts."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description": "Maximum transcripts to return (default: 50)."
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
            "name": "export_conversation",
            "description": "Export a locally archived channel, thread, audio/video file transcript, or huddle transcript as Markdown or JSON returned directly as tool content (no filesystem writes).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace": {
                        "type": "string",
                        "description": "Optional workspace team ID."
                    },
                    "channel": {
                        "type": "string",
                        "description": "Channel ID, #name, or @handle (for channel or thread export)."
                    },
                    "thread_ts": {
                        "type": "string",
                        "description": "Optional thread root `ts` (requires `channel`) to export a single thread."
                    },
                    "file_id": {
                        "type": "string",
                        "description": "Slack file ID to export a stored audio/video transcript (mutually exclusive with `channel` and `huddle_id`)."
                    },
                    "huddle_id": {
                        "type": "string",
                        "description": "Huddle call ID to export a stored huddle transcript (mutually exclusive with `channel` and `file_id`)."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["md", "json"],
                        "description": "Output format: \"md\" (Markdown, default) or \"json\"."
                    },
                    "include_threads": {
                        "type": "boolean",
                        "description": "When exporting a channel, nest stored thread replies under thread roots (default: false)."
                    },
                    "include_deleted": {
                        "type": "boolean",
                        "description": "Include locally preserved deleted message tombstones (default: false)."
                    },
                    "include_transcripts": {
                        "type": "boolean",
                        "description": "Include stored/inline audio/video transcripts attached to exported messages (default: true)."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 5000,
                        "description": "Maximum top-level channel messages to export (default: 500)."
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
            "name": "get_catchup_summary",
            "description": "Build a local SQLite catch-up digest of recent activity (unread/recent channels, direct mentions, active threads, and new transcripts) over a `since_hours` window across one or all workspaces.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace": {
                        "type": "string",
                        "description": "Optional workspace team ID."
                    },
                    "all_workspaces": {
                        "type": "boolean",
                        "description": "Aggregate catch-up activity across every locally archived workspace."
                    },
                    "since_hours": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 720,
                        "description": "Lookback window in hours (default: 24, max: 720)."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100,
                        "description": "Maximum items per section (channels, mentions, active_threads, new_transcripts; default: 20)."
                    },
                    "user_id": {
                        "type": "string",
                        "description": "Optional Slack user ID (e.g. U0123ABC) to detect direct `<@USER>` mentions for. Defaults to the signed-in session's user ID per workspace."
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

fn resource_definitions() -> Value {
    json!([
        {
            "uri": "slackless://workspaces",
            "name": "workspaces",
            "title": "Slack Workspaces",
            "description": "All stored Slack workspaces (signed-in sessions and local SQLite archives) with message counts.",
            "mimeType": "application/json"
        }
    ])
}

fn resource_template_definitions() -> Value {
    json!([
        {
            "uriTemplate": "slackless://workspace/{workspace}/channels",
            "name": "workspace_channels",
            "title": "Workspace Channels & DMs",
            "description": "Channels and direct messages archived locally for a workspace.",
            "mimeType": "application/json"
        },
        {
            "uriTemplate": "slackless://workspace/{workspace}/channel/{channel}",
            "name": "channel_messages",
            "title": "Channel Recent Messages",
            "description": "Recent locally archived messages from a channel or DM (accepts channel ID, name, or @handle).",
            "mimeType": "application/json"
        },
        {
            "uriTemplate": "slackless://workspace/{workspace}/thread/{channel}/{ts}",
            "name": "thread_messages",
            "title": "Thread Conversation",
            "description": "Root message and replies for a thread from the local SQLite archive.",
            "mimeType": "application/json"
        },
        {
            "uriTemplate": "slackless://workspace/{workspace}/transcript/{file_id}",
            "name": "file_transcript",
            "title": "Audio/Video File Transcript",
            "description": "Stored audio/video file transcript and timed cues from the local SQLite archive.",
            "mimeType": "application/json"
        }
    ])
}

fn prompt_definitions() -> Value {
    json!([
        {
            "name": "catch_up",
            "description": "Summarize recent Slack activity, direct mentions, active threads, and new transcripts over a lookback window.",
            "arguments": [
                {
                    "name": "workspace",
                    "description": "Optional workspace team ID (defaults to default workspace or all workspaces).",
                    "required": false
                },
                {
                    "name": "since_hours",
                    "description": "Lookback window in hours (default: 24, max: 720).",
                    "required": false
                }
            ]
        },
        {
            "name": "summarize_thread",
            "description": "Summarize a Slack thread with key takeaways, decisions, action items, and open questions.",
            "arguments": [
                {
                    "name": "channel",
                    "description": "Channel ID, #name, or @handle.",
                    "required": true
                },
                {
                    "name": "thread_ts",
                    "description": "Timestamp (`ts`) of the thread root message.",
                    "required": true
                },
                {
                    "name": "workspace",
                    "description": "Optional workspace team ID.",
                    "required": false
                }
            ]
        },
        {
            "name": "summarize_channel",
            "description": "Summarize recent discussions, themes, and blockers in a Slack channel or DM.",
            "arguments": [
                {
                    "name": "channel",
                    "description": "Channel ID, #name, or @handle.",
                    "required": true
                },
                {
                    "name": "since",
                    "description": "Optional lookback cutoff (e.g. \"24h\", \"7d\", \"YYYY-MM-DD\", or unix epoch seconds).",
                    "required": false
                },
                {
                    "name": "limit",
                    "description": "Maximum messages to include (default: 50, max: 100).",
                    "required": false
                },
                {
                    "name": "workspace",
                    "description": "Optional workspace team ID.",
                    "required": false
                }
            ]
        },
        {
            "name": "find_decisions",
            "description": "Extract concrete or tentative decisions, owners, rationale, and open questions from a channel or query.",
            "arguments": [
                {
                    "name": "channel",
                    "description": "Optional channel ID, #name, or @handle to focus on.",
                    "required": false
                },
                {
                    "name": "query",
                    "description": "Optional topic keywords or Slack search modifiers.",
                    "required": false
                },
                {
                    "name": "since",
                    "description": "Optional lower time bound (e.g. \"7d\", \"2026-01-01\").",
                    "required": false
                },
                {
                    "name": "workspace",
                    "description": "Optional workspace team ID.",
                    "required": false
                }
            ]
        },
        {
            "name": "search_transcripts",
            "description": "Search and synthesize stored audio/video file transcripts and huddle transcripts for a topic.",
            "arguments": [
                {
                    "name": "query",
                    "description": "Keywords or topic to search across stored audio/video and huddle transcripts.",
                    "required": true
                },
                {
                    "name": "workspace",
                    "description": "Optional workspace team ID.",
                    "required": false
                }
            ]
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
        assert!(lines[0]["result"]["capabilities"]["resources"].is_object());
        assert!(lines[0]["result"]["capabilities"]["prompts"].is_object());

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
                "list_transcripts",
                "export_conversation",
                "get_catchup_summary",
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
            .handle_line(r#"{"jsonrpc":"2.0","id":"abc","method":"unknown/method"}"#)
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
            json!({"name": "list_transcripts", "arguments": {"workspace": "../secret_outside"}}),
            json!({"name": "export_conversation", "arguments": {"channel": "#eng", "workspace": "../secret_outside"}}),
            json!({"name": "get_catchup_summary", "arguments": {"workspace": "../secret_outside"}}),
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

    #[tokio::test]
    async fn v2_export_conversation_and_list_transcripts_over_stdio() {
        use slackless_core::store::NewHuddleTranscriptSegment;

        let dir = TempDir::new("v2-export-transcripts");
        seed_multi_workspace_stores(&dir);

        // Enrich T1 with timed file cues and a huddle transcript session; add a file transcript in T2.
        {
            let t1 = TeamId::from("T1");
            let mut s1 = Store::open(dir.path().join("T1.sqlite")).expect("open T1");
            let cues = vec![
                TranscriptCue {
                    start_secs: 0.0,
                    end_secs: 4.5,
                    text: "Zero downtime canary migration".into(),
                },
                TranscriptCue {
                    start_secs: 4.5,
                    end_secs: 12.25,
                    text: "succeeded across eu-north.".into(),
                },
            ];
            s1.upsert_file_transcript_with_cues(
                &t1,
                &FileId::from("F_CLIP1"),
                "parakeet-v3",
                "Zero downtime canary migration succeeded across eu-north.",
                &cues,
            )
            .expect("upsert timed file transcript");

            s1.insert_huddle_transcript_segment(
                &t1,
                &NewHuddleTranscriptSegment {
                    channel: &ChannelId::from("C_ENG"),
                    call_id: "R_HUDDLE1",
                    thread_root_ts: Some(&Ts::from("1700000200.000100")),
                    speaker_id: Some(&UserId::from("U1")),
                    lang: Some("en-US"),
                    source: "chime",
                    text: "Let us finalize the rollout checklist.",
                    at_ms: 1_700_000_600_000,
                },
            )
            .expect("insert huddle seg 1");
            s1.insert_huddle_transcript_segment(
                &t1,
                &NewHuddleTranscriptSegment {
                    channel: &ChannelId::from("C_ENG"),
                    call_id: "R_HUDDLE1",
                    thread_root_ts: Some(&Ts::from("1700000200.000100")),
                    speaker_id: Some(&UserId::from("U2")),
                    lang: Some("sv-SE"),
                    source: "chime",
                    text: "Vi skeppar i eftermiddag.",
                    at_ms: 1_700_000_645_500,
                },
            )
            .expect("insert huddle seg 2");
        }
        {
            let t2 = TeamId::from("T2");
            let mut s2 = Store::open(dir.path().join("T2.sqlite")).expect("open T2");
            s2.upsert_file_transcript(
                &t2,
                &FileId::from("F_T2"),
                "whisper-local",
                "Secondary workspace ops walkthrough.",
            )
            .expect("upsert T2 transcript");
        }

        let sessions = vec![Session::new(
            "https://acme.slack.com/",
            "T1",
            "xoxc-test",
            "xoxd-test",
            "U1",
        )];
        let mut server = McpServer::for_test(dir.path().to_path_buf(), sessions, None);

        let requests = [
            // 1. export channel Markdown with nested thread replies, excluding deleted
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "export_conversation",
                    "arguments": {
                        "channel": "#eng",
                        "format": "md",
                        "include_threads": true,
                        "include_deleted": false,
                        "limit": 10
                    }
                }
            }),
            // 2. export channel JSON with deleted included
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "export_conversation",
                    "arguments": {
                        "channel": "C_ENG",
                        "format": "json",
                        "include_deleted": true
                    }
                }
            }),
            // 3. export thread Markdown
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "export_conversation",
                    "arguments": {
                        "channel": "#eng",
                        "thread_ts": "1700000200.000100",
                        "format": "md"
                    }
                }
            }),
            // 4. export file transcript JSON (cross-workspace lookup for T2's F_T2)
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "export_conversation",
                    "arguments": {
                        "file_id": "F_T2",
                        "format": "json"
                    }
                }
            }),
            // 5. export huddle transcript Markdown
            json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "export_conversation",
                    "arguments": {
                        "huddle_id": "R_HUDDLE1",
                        "format": "md"
                    }
                }
            }),
            // 6. list_transcripts across all workspaces
            json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": {
                    "name": "list_transcripts",
                    "arguments": { "all_workspaces": true }
                }
            }),
            // 7. list_transcripts filtered to kind="huddle" and channel="#eng"
            json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {
                    "name": "list_transcripts",
                    "arguments": { "workspace": "T1", "kind": "huddle", "channel": "#eng" }
                }
            }),
            // 8. export error: thread_ts without channel
            json!({
                "jsonrpc": "2.0",
                "id": 8,
                "method": "tools/call",
                "params": {
                    "name": "export_conversation",
                    "arguments": { "thread_ts": "1700000200.000100" }
                }
            }),
            // 9. export error: mutually exclusive targets
            json!({
                "jsonrpc": "2.0",
                "id": 9,
                "method": "tools/call",
                "params": {
                    "name": "export_conversation",
                    "arguments": { "channel": "#eng", "file_id": "F_CLIP1" }
                }
            }),
            // 10. export error: missing huddle_id
            json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "tools/call",
                "params": {
                    "name": "export_conversation",
                    "arguments": { "huddle_id": "R_MISSING" }
                }
            }),
        ];

        let mut input = String::new();
        for req in &requests {
            input.push_str(&serde_json::to_string(req).expect("serialize req"));
            input.push('\n');
        }
        let mut output = Vec::new();
        server
            .serve_io(Cursor::new(input), &mut output)
            .await
            .expect("serve_io");
        let lines: Vec<Value> = String::from_utf8(output)
            .expect("utf8")
            .lines()
            .map(|l| serde_json::from_str(l).expect("json line"))
            .collect();
        assert_eq!(lines.len(), 10);

        // 1. channel Markdown export includes thread replies and file transcript, excludes deleted
        let (err, p) = parse_tool_payload(&lines[0]);
        assert!(!err, "unexpected error: {p}");
        assert_eq!(p["target"], "channel");
        assert_eq!(p["format"], "md");
        assert_eq!(p["item_count"], 2);
        let md = p["content"].as_str().expect("md content");
        assert!(md.contains("# #eng"));
        assert!(md.contains("Thread replies (1)"));
        assert!(md.contains("Looks solid, shipping today"));
        assert!(md.contains("eu-north"));
        assert!(!md.contains("Temporary scratch note"));

        // 2. channel JSON export with include_deleted=true includes tombstone
        let (err, p) = parse_tool_payload(&lines[1]);
        assert!(!err);
        assert_eq!(p["format"], "json");
        assert_eq!(p["item_count"], 3);
        let exported_doc: Value =
            serde_json::from_str(p["content"].as_str().expect("json content"))
                .expect("valid exported json");
        assert_eq!(exported_doc["kind"], "channel");
        assert_eq!(exported_doc["count"], 3);

        // 3. thread Markdown export
        let (err, p) = parse_tool_payload(&lines[2]);
        assert!(!err);
        assert_eq!(p["target"], "thread");
        assert_eq!(p["item_count"], 2);
        let md = p["content"].as_str().expect("thread md");
        assert!(md.contains("Thread in #eng"));
        assert!(md.contains("Looks solid, shipping today"));

        // 4. file transcript JSON export across workspaces
        let (err, p) = parse_tool_payload(&lines[3]);
        assert!(!err);
        assert_eq!(p["workspace"], "T2");
        assert_eq!(p["target"], "file_transcript");
        assert_eq!(p["file_id"], "F_T2");
        let exported_doc: Value =
            serde_json::from_str(p["content"].as_str().expect("file json")).expect("valid json");
        assert_eq!(exported_doc["kind"], "file_transcript");
        assert!(
            exported_doc["text"]
                .as_str()
                .expect("text")
                .contains("walkthrough")
        );

        // 5. huddle transcript Markdown export
        let (err, p) = parse_tool_payload(&lines[4]);
        assert!(!err);
        assert_eq!(p["target"], "huddle_transcript");
        assert_eq!(p["item_count"], 2);
        let md = p["content"].as_str().expect("huddle md");
        assert!(md.contains("Huddle Transcript"));
        assert!(md.contains("Ana"));
        assert!(md.contains("Bob"));
        assert!(md.contains("Vi skeppar i eftermiddag."));

        // 6. list_transcripts across all workspaces returns 2 files + 1 huddle
        let (err, p) = parse_tool_payload(&lines[5]);
        assert!(!err);
        assert_eq!(p["all_workspaces"], true);
        assert_eq!(p["count"], 3);
        assert_eq!(p["file_count"], 2);
        assert_eq!(p["huddle_count"], 1);
        let items = p["transcripts"].as_array().expect("transcripts array");
        let f_clip = items
            .iter()
            .find(|t| t["id"] == "F_CLIP1")
            .expect("F_CLIP1 entry");
        assert_eq!(f_clip["cue_count"], 2);
        assert_eq!(f_clip["duration_secs"], 12.25);
        assert_eq!(f_clip["uri"], "slackless://workspace/T1/transcript/F_CLIP1");
        let huddle = items
            .iter()
            .find(|t| t["id"] == "R_HUDDLE1")
            .expect("R_HUDDLE1 entry");
        assert_eq!(huddle["kind"], "huddle");
        assert_eq!(huddle["cue_count"], 2);
        assert_eq!(huddle["duration_secs"], 45.5);
        assert_eq!(huddle["channel_name"], "#eng");
        assert_eq!(huddle["languages"], json!(["en", "sv"]));

        // 7. filtered huddle transcripts
        let (err, p) = parse_tool_payload(&lines[6]);
        assert!(!err);
        assert_eq!(p["count"], 1);
        assert_eq!(p["transcripts"][0]["id"], "R_HUDDLE1");

        // 8-10. error handling
        let (err, p) = parse_tool_payload(&lines[7]);
        assert!(err);
        assert_eq!(p["error"], "invalid_args");

        let (err, p) = parse_tool_payload(&lines[8]);
        assert!(err);
        assert_eq!(p["error"], "invalid_args");

        let (err, p) = parse_tool_payload(&lines[9]);
        assert!(err);
        assert_eq!(p["error"], "not_found");
    }

    #[tokio::test]
    async fn v2_get_catchup_summary_aggregates_windowed_activity() {
        use slackless_core::store::NewHuddleTranscriptSegment;

        let dir = TempDir::new("v2-catchup");
        let now = now_epoch_secs();
        let recent_1 = now - 600;
        let recent_2 = now - 300;
        let old_secs = now - (100 * 3_600);

        let t1 = TeamId::from("T1");
        let mut s1 = Store::open(dir.path().join("T1.sqlite")).expect("open T1");
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
        .expect("users");
        s1.upsert_conversations(
            &t1,
            &[
                Conversation {
                    id: ChannelId::from("C_ENG"),
                    name: Some("eng".into()),
                    is_channel: true,
                    is_member: true,
                    unread_count_display: Some(2),
                    last_read: Some(Ts::from(format!("{}.000000", now - 3_600))),
                    ..Conversation::default()
                },
                Conversation {
                    id: ChannelId::from("C_NOISE"),
                    name: Some("noise".into()),
                    is_channel: true,
                    is_member: true,
                    ..Conversation::default()
                },
            ],
        )
        .expect("convs");
        s1.replace_prefs(
            &t1,
            &json!({
                "all_notifications_prefs": json!({
                    "channels": { "C_NOISE": { "muted": true } }
                }).to_string()
            }),
        )
        .expect("prefs");

        // Recent mention + active thread root + reply, plus an old message outside window.
        let thread_root_ts = format!("{recent_1}.000100");
        let thread_reply_ts = format!("{recent_2}.000200");
        s1.upsert_messages(
            &t1,
            &ChannelId::from("C_ENG"),
            &[
                Message {
                    ts: Ts::from(format!("{old_secs}.000000")),
                    user: Some(UserId::from("U2")),
                    text: "Ancient message mentioning <@U1>".into(),
                    ..Message::default()
                },
                Message {
                    ts: Ts::from(thread_root_ts.clone()),
                    user: Some(UserId::from("U2")),
                    text: "Hey <@U1> can you review the release plan?".into(),
                    reply_count: Some(1),
                    latest_reply: Some(Ts::from(thread_reply_ts.clone())),
                    ..Message::default()
                },
                Message {
                    ts: Ts::from(thread_reply_ts),
                    thread_ts: Some(Ts::from(thread_root_ts.clone())),
                    user: Some(UserId::from("U1")),
                    text: "Looking now!".into(),
                    ..Message::default()
                },
            ],
        )
        .expect("upsert C_ENG");
        s1.upsert_message(
            &t1,
            &ChannelId::from("C_NOISE"),
            &Message {
                ts: Ts::from(format!("{recent_1}.000000")),
                user: Some(UserId::from("U2")),
                text: "Muted alert ping".into(),
                ..Message::default()
            },
        )
        .expect("upsert C_NOISE");
        s1.upsert_file_transcript(
            &t1,
            &FileId::from("F_NEW"),
            "parakeet-v3",
            "Fresh architecture review recording.",
        )
        .expect("file transcript");
        s1.insert_huddle_transcript_segment(
            &t1,
            &NewHuddleTranscriptSegment {
                channel: &ChannelId::from("C_ENG"),
                call_id: "R_OLD",
                thread_root_ts: None,
                speaker_id: Some(&UserId::from("U2")),
                lang: Some("en"),
                source: "chime",
                text: "Old huddle outside window",
                at_ms: old_secs * 1_000,
            },
        )
        .expect("old huddle");
        s1.insert_huddle_transcript_segment(
            &t1,
            &NewHuddleTranscriptSegment {
                channel: &ChannelId::from("C_ENG"),
                call_id: "R_NEW",
                thread_root_ts: None,
                speaker_id: Some(&UserId::from("U2")),
                lang: Some("en"),
                source: "chime",
                text: "New huddle inside window",
                at_ms: recent_2 * 1_000,
            },
        )
        .expect("new huddle");
        drop(s1);

        let sessions = vec![Session::new(
            "https://acme.slack.com/",
            "T1",
            "xoxc-test",
            "xoxd-test",
            "U1",
        )];
        let mut server = McpServer::for_test(dir.path().to_path_buf(), sessions, None);

        let resp = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_catchup_summary","arguments":{"since_hours":24}}}"#,
            )
            .await
            .expect("catchup response");
        let (is_err, p) = parse_tool_payload(&resp);
        assert!(!is_err, "unexpected error: {p}");
        assert_eq!(p["since_hours"], 24);
        assert_eq!(p["totals"]["active_channels"], 2);
        assert_eq!(p["totals"]["unread_channels"], 1);
        assert_eq!(p["totals"]["mentions"], 1);
        assert_eq!(p["totals"]["active_threads"], 1);
        assert_eq!(p["totals"]["new_transcripts"], 2);

        // Unread channel #eng sorts first; muted #noise is marked muted.
        let channels = p["channels"].as_array().expect("channels");
        assert_eq!(channels[0]["channel"], "C_ENG");
        assert_eq!(channels[0]["is_unread"], true);
        assert_eq!(channels[0]["unread_count"], 2);
        assert_eq!(channels[1]["channel"], "C_NOISE");
        assert_eq!(channels[1]["muted"], true);

        // Mention excludes the 100h-old message.
        let mentions = p["mentions"].as_array().expect("mentions");
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0]["ts"], thread_root_ts);
        assert_eq!(mentions[0]["user_name"], "Bob");

        // Active threads & new transcripts exclude old huddle R_OLD.
        assert_eq!(p["active_threads"][0]["thread_ts"], thread_root_ts);
        let tr_ids: Vec<&str> = p["new_transcripts"]
            .as_array()
            .expect("new_transcripts")
            .iter()
            .filter_map(|t| t["id"].as_str())
            .collect();
        assert!(tr_ids.contains(&"F_NEW"));
        assert!(tr_ids.contains(&"R_NEW"));
        assert!(!tr_ids.contains(&"R_OLD"));
    }

    #[tokio::test]
    async fn v2_resources_list_templates_read_and_traversal_rejection() {
        let parent = TempDir::new("v2-resources");
        let archive_dir = parent.path().join("archives");
        std::fs::create_dir_all(&archive_dir).expect("create archives dir");

        // Seed T1 and T2 inside archive_dir.
        let t1 = TeamId::from("T1");
        let mut s1 = Store::open(archive_dir.join("T1.sqlite")).expect("open T1");
        s1.upsert_users(
            &t1,
            &[User {
                id: UserId::from("U1"),
                name: "ana".into(),
                profile: Profile {
                    display_name: Some("Ana".into()),
                    ..Profile::default()
                },
                ..User::default()
            }],
        )
        .expect("users");
        s1.upsert_conversations(
            &t1,
            &[Conversation {
                id: ChannelId::from("C_ENG"),
                name: Some("eng".into()),
                is_channel: true,
                is_member: true,
                ..Conversation::default()
            }],
        )
        .expect("convs");
        s1.upsert_messages(
            &t1,
            &ChannelId::from("C_ENG"),
            &[
                Message {
                    ts: Ts::from("1700000200.000100"),
                    user: Some(UserId::from("U1")),
                    text: "Root message".into(),
                    reply_count: Some(1),
                    ..Message::default()
                },
                Message {
                    ts: Ts::from("1700000250.000200"),
                    thread_ts: Some(Ts::from("1700000200.000100")),
                    user: Some(UserId::from("U1")),
                    text: "Thread reply".into(),
                    ..Message::default()
                },
            ],
        )
        .expect("messages");
        s1.upsert_file_transcript(
            &t1,
            &FileId::from("F_CLIP1"),
            "parakeet-v3",
            "Resource transcript body.",
        )
        .expect("transcript");
        drop(s1);

        // Place a secret archive OUTSIDE archive_dir to prove traversal URIs cannot reach it.
        let outside_ws = TeamId::from("secret_outside");
        let mut outside =
            Store::open(parent.path().join("secret_outside.sqlite")).expect("outside");
        outside
            .upsert_file_transcript(
                &outside_ws,
                &FileId::from("F_SECRET"),
                "local",
                "do not leak",
            )
            .expect("outside transcript");
        drop(outside);

        let mut server = McpServer::for_test(archive_dir, Vec::new(), Some("T1".into()));

        // resources/list
        let r = server
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#)
            .await
            .expect("resources/list");
        let resources = r["result"]["resources"]
            .as_array()
            .expect("resources array");
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0]["uri"], "slackless://workspaces");

        // resources/templates/list
        let r = server
            .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"resources/templates/list"}"#)
            .await
            .expect("templates/list");
        let templates: Vec<&str> = r["result"]["resourceTemplates"]
            .as_array()
            .expect("templates array")
            .iter()
            .filter_map(|t| t["uriTemplate"].as_str())
            .collect();
        assert_eq!(
            templates,
            vec![
                "slackless://workspace/{workspace}/channels",
                "slackless://workspace/{workspace}/channel/{channel}",
                "slackless://workspace/{workspace}/thread/{channel}/{ts}",
                "slackless://workspace/{workspace}/transcript/{file_id}",
            ]
        );

        // resources/read: workspaces
        let r = server
            .handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"slackless://workspaces"}}"#)
            .await
            .expect("read workspaces");
        let body: Value =
            serde_json::from_str(r["result"]["contents"][0]["text"].as_str().expect("text"))
                .expect("json");
        assert_eq!(body["count"], 1);
        assert_eq!(body["workspaces"][0]["team_id"], "T1");

        // resources/read: channels
        let r = server
            .handle_line(r#"{"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"slackless://workspace/T1/channels"}}"#)
            .await
            .expect("read channels");
        let body: Value =
            serde_json::from_str(r["result"]["contents"][0]["text"].as_str().expect("text"))
                .expect("json");
        assert_eq!(body["count"], 1);
        assert_eq!(body["channels"][0]["id"], "C_ENG");

        // resources/read: channel (both raw ID and percent-encoded #eng)
        for uri in [
            "slackless://workspace/T1/channel/C_ENG",
            "slackless://workspace/T1/channel/%23eng",
        ] {
            let req = json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "resources/read",
                "params": { "uri": uri }
            });
            let r = server
                .handle_line(&serde_json::to_string(&req).expect("ser"))
                .await
                .expect("read channel");
            let body: Value =
                serde_json::from_str(r["result"]["contents"][0]["text"].as_str().expect("text"))
                    .expect("json");
            assert_eq!(body["channel"]["id"], "C_ENG");
            assert_eq!(body["count"], 1);
        }

        // resources/read: thread
        let r = server
            .handle_line(r#"{"jsonrpc":"2.0","id":6,"method":"resources/read","params":{"uri":"slackless://workspace/T1/thread/C_ENG/1700000200.000100"}}"#)
            .await
            .expect("read thread");
        let body: Value =
            serde_json::from_str(r["result"]["contents"][0]["text"].as_str().expect("text"))
                .expect("json");
        assert_eq!(body["count"], 2);
        assert_eq!(body["messages"][1]["text"], "Thread reply");

        // resources/read: transcript
        let r = server
            .handle_line(r#"{"jsonrpc":"2.0","id":7,"method":"resources/read","params":{"uri":"slackless://workspace/T1/transcript/F_CLIP1"}}"#)
            .await
            .expect("read transcript");
        let body: Value =
            serde_json::from_str(r["result"]["contents"][0]["text"].as_str().expect("text"))
                .expect("json");
        assert_eq!(body["file_id"], "F_CLIP1");
        assert_eq!(body["text"], "Resource transcript body.");

        // Missing transcript -> ERR_RESOURCE_NOT_FOUND (-32002)
        let r = server
            .handle_line(r#"{"jsonrpc":"2.0","id":8,"method":"resources/read","params":{"uri":"slackless://workspace/T1/transcript/F_NOPE"}}"#)
            .await
            .expect("missing transcript");
        assert_eq!(r["error"]["code"], ERR_RESOURCE_NOT_FOUND);

        // Traversal and malformed URIs -> strictly ERR_INVALID_PARAMS (-32602)
        for bad_uri in [
            "slackless://workspace/../secret_outside/transcript/F_SECRET",
            "slackless://workspace/..%2Fsecret_outside/transcript/F_SECRET",
            "slackless://workspace/%2e%2e/channels",
            "slackless://workspace/secret_outside.sqlite/channels",
            r"slackless://workspace/T1\..\secret_outside/channels",
            "slackless://workspace/T1/channel/foo%2Fbar",
            "slackless://workspace/T1/channel/%2e%2e",
            "slackless://workspace/T1/transcript/%2e",
            "slackless://workspace/T1/channel/%ZZ",
            "slackless://workspace/T1/channel/",
            "slackless://workspace/T1/channels?foo=bar",
            "slackless://workspace/T1/channels#frag",
            "file:///etc/passwd",
            "https://example.com/slackless",
            "slackless://unknown/path",
        ] {
            let req = json!({
                "jsonrpc": "2.0",
                "id": 99,
                "method": "resources/read",
                "params": { "uri": bad_uri }
            });
            let r = server
                .handle_line(&serde_json::to_string(&req).expect("ser"))
                .await
                .expect("bad uri response");
            assert_eq!(
                r["error"]["code"], ERR_INVALID_PARAMS,
                "expected ERR_INVALID_PARAMS for URI {bad_uri:?}, got {r}"
            );
        }
    }

    #[tokio::test]
    async fn v2_prompts_list_get_and_context_bounding() {
        use slackless_core::store::NewHuddleTranscriptSegment;

        let dir = TempDir::new("v2-prompts");
        seed_multi_workspace_stores(&dir);

        // Add a decision message, a huddle transcript, and a huge channel to test prompt bounding.
        {
            let t1 = TeamId::from("T1");
            let mut s1 = Store::open(dir.path().join("T1.sqlite")).expect("open T1");
            s1.upsert_conversations(
                &t1,
                &[Conversation {
                    id: ChannelId::from("C_HUGE"),
                    name: Some("huge".into()),
                    is_channel: true,
                    is_member: true,
                    ..Conversation::default()
                }],
            )
            .expect("huge conv");
            s1.upsert_message(
                &t1,
                &ChannelId::from("C_ENG"),
                &Message {
                    ts: Ts::from(format!("{}.000000", now_epoch_secs() - 120)),
                    user: Some(UserId::from("U1")),
                    text: "We decided to ship the eu-north canary on Friday.".into(),
                    ..Message::default()
                },
            )
            .expect("decision msg");
            s1.upsert_message(
                &t1,
                &ChannelId::from("D_ANA"),
                &Message {
                    ts: Ts::from(format!("{}.000000", now_epoch_secs() - 90)),
                    user: Some(UserId::from("U1")),
                    text: "DM decision: approved the staging cutover with Ana.".into(),
                    ..Message::default()
                },
            )
            .expect("dm decision msg");
            s1.insert_huddle_transcript_segment(
                &t1,
                &NewHuddleTranscriptSegment {
                    channel: &ChannelId::from("C_ENG"),
                    call_id: "R_PROMPT1",
                    thread_root_ts: None,
                    speaker_id: Some(&UserId::from("U2")),
                    lang: Some("en"),
                    source: "chime",
                    text: "The eu-north failover drill passed in 14 seconds.",
                    at_ms: (now_epoch_secs() - 60) * 1_000,
                },
            )
            .expect("huddle seg");

            let big_body = "x".repeat(550);
            let huge_msgs: Vec<Message> = (0..80)
                .map(|i| Message {
                    ts: Ts::from(format!("{}.000000", 1_700_001_000 + i)),
                    user: Some(UserId::from("U1")),
                    text: format!("entry-{i:02} {big_body}"),
                    ..Message::default()
                })
                .collect();
            s1.upsert_messages(&t1, &ChannelId::from("C_HUGE"), &huge_msgs)
                .expect("huge msgs");
        }

        let sessions = vec![Session::new(
            "https://acme.slack.com/",
            "T1",
            "xoxc-test",
            "xoxd-test",
            "U1",
        )];
        let mut server = McpServer::for_test(dir.path().to_path_buf(), sessions, None);

        // prompts/list
        let r = server
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"prompts/list"}"#)
            .await
            .expect("prompts/list");
        let prompt_names: Vec<&str> = r["result"]["prompts"]
            .as_array()
            .expect("prompts array")
            .iter()
            .filter_map(|p| p["name"].as_str())
            .collect();
        assert_eq!(
            prompt_names,
            vec![
                "catch_up",
                "summarize_thread",
                "summarize_channel",
                "find_decisions",
                "search_transcripts",
            ]
        );

        // 1. catch_up
        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":2,"method":"prompts/get","params":{"name":"catch_up","arguments":{"since_hours":"24"}}}"#,
            )
            .await
            .expect("catch_up");
        let text = r["result"]["messages"][0]["content"]["text"]
            .as_str()
            .expect("catch_up text");
        assert_eq!(r["result"]["messages"][0]["role"], "user");
        assert!(text.contains("#eng"));
        assert!(text.contains("R_PROMPT1"));

        // 2. summarize_thread
        let r = server
            .handle_line(
                r##"{"jsonrpc":"2.0","id":3,"method":"prompts/get","params":{"name":"summarize_thread","arguments":{"channel":"#eng","thread_ts":"1700000200.000100"}}}"##,
            )
            .await
            .expect("summarize_thread");
        let text = r["result"]["messages"][0]["content"]["text"]
            .as_str()
            .expect("thread prompt text");
        assert!(text.contains("Watch the rollout demo clip"));
        assert!(text.contains("Looks solid, shipping today"));

        // 3. summarize_channel (including context truncation on huge channel)
        let r = server
            .handle_line(
                r##"{"jsonrpc":"2.0","id":4,"method":"prompts/get","params":{"name":"summarize_channel","arguments":{"channel":"#huge","limit":"80"}}}"##,
            )
            .await
            .expect("summarize_channel huge");
        let text = r["result"]["messages"][0]["content"]["text"]
            .as_str()
            .expect("huge prompt text");
        assert!(text.contains("context truncated to stay within prompt budget"));
        assert!(text.chars().count() < MAX_PROMPT_CONTEXT_CHARS + 512);

        // 4. find_decisions (channel + query, plus @handle DM + query)
        let r = server
            .handle_line(
                r##"{"jsonrpc":"2.0","id":5,"method":"prompts/get","params":{"name":"find_decisions","arguments":{"query":"decided","channel":"#eng"}}}"##,
            )
            .await
            .expect("find_decisions");
        let text = r["result"]["messages"][0]["content"]["text"]
            .as_str()
            .expect("decisions prompt text");
        assert!(text.contains("ship the eu-north canary on Friday"));

        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":51,"method":"prompts/get","params":{"name":"find_decisions","arguments":{"query":"cutover","channel":"@ana"}}}"#,
            )
            .await
            .expect("find_decisions dm");
        let text = r["result"]["messages"][0]["content"]["text"]
            .as_str()
            .expect("dm decisions prompt text");
        assert!(text.contains("approved the staging cutover with Ana"));
        assert!(!text.contains("ship the eu-north canary on Friday"));

        // 5. search_transcripts (hits both file transcript F_CLIP1 and huddle R_PROMPT1)
        let r = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":6,"method":"prompts/get","params":{"name":"search_transcripts","arguments":{"query":"eu-north"}}}"#,
            )
            .await
            .expect("search_transcripts");
        let text = r["result"]["messages"][0]["content"]["text"]
            .as_str()
            .expect("search_transcripts prompt text");
        assert!(text.contains("F_CLIP1"));
        assert!(text.contains("R_PROMPT1"));
        assert!(text.contains("failover drill passed in 14 seconds"));

        // Error cases -> ERR_INVALID_PARAMS (-32602)
        for bad_req in [
            json!({"jsonrpc":"2.0","id":10,"method":"prompts/get","params":{"name":"unknown_prompt"}}),
            json!({"jsonrpc":"2.0","id":11,"method":"prompts/get","params":{"name":"summarize_thread","arguments":{"channel":"#eng"}}}),
            json!({"jsonrpc":"2.0","id":12,"method":"prompts/get","params":{"name":"search_transcripts","arguments":{}}}),
            json!({"jsonrpc":"2.0","id":13,"method":"prompts/get","params":{"name":"catch_up","arguments":{"workspace":"../secret"}}}),
            json!({"jsonrpc":"2.0","id":14,"method":"prompts/get","params":{"name":"catch_up","arguments":"bad"}}),
        ] {
            let r = server
                .handle_line(&serde_json::to_string(&bad_req).expect("ser"))
                .await
                .expect("bad prompt response");
            assert_eq!(
                r["error"]["code"], ERR_INVALID_PARAMS,
                "expected ERR_INVALID_PARAMS for {bad_req}, got {r}"
            );
        }
    }
}
