//! stdio Model Context Protocol (MCP) server (`slackless mcp`), built on the official Rust SDK
//! (`rmcp`).
//!
//! Exposes offline SQLite archive queries (`list_workspaces`, `list_channels`,
//! `search_messages`, `read_channel`, `read_thread`, `get_transcript`, `list_transcripts`,
//! `export_conversation`, `get_catchup_summary`), live Slack write/read actions
//! (`send_message`, `reply_thread`, `add_reaction`, plus `live: true` on `read_channel`
//! and `read_thread`; `--read-only` withholds the write tools), MCP resources (`slackless://...`),
//! and reusable MCP prompts (`catch_up`, `summarize_thread`, `summarize_channel`, `find_decisions`,
//! `search_transcripts`).
//!
//! Stdout is reserved strictly for newline-delimited JSON-RPC 2.0 messages; no human logs or
//! prompts are ever written there.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use rmcp::handler::server::tool::{IntoCallToolResult, ToolCallContext, ToolRouter};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, CustomRequest,
    CustomResult, ErrorCode, GetPromptRequestParams, GetPromptResponse, GetPromptResult,
    Implementation, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, Prompt, PromptArgument, PromptMessage,
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ResourceTemplate, Role, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use slackless_core::api::{MessageContent, SlackApi};
use slackless_core::auth::Session;
use slackless_core::catchup::{CatchUpWindow, format_catchup_human};
use slackless_core::domain::mrkdwn::mrkdwn_to_plain;
use slackless_core::export::format_utc_timestamp;
use slackless_core::export::{ChannelWindow, ExportFormat, ExportOptions, ExportRequest};
use slackless_core::search::truncate_chars;
use slackless_core::store::{SearchQuery, SearchSort, StoredFileTranscript, StoredMessage};
use slackless_core::types::time::now_epoch_secs;
use slackless_core::types::{ChannelId, FileId, Message, TeamId, TranscriptCue, Ts, UserId};

use crate::archive::{self, Archive};
use crate::commands::browse::catchup_briefing;
use crate::commands::messages::{
    ChannelRead, ThreadRead, channel_payload, export_archive, export_not_found, live_channel_read,
    live_thread_read, local_channel, local_thread, search_all_document, search_all_in_archives,
    search_document, thread_payload,
};
use crate::commands::{CmdError, all_workspace_ids, parse_since_cutoff, raw_channel_id};
use crate::live::session::{classify_api_error, resolve_channel_with};
use crate::session::{
    is_valid_workspace_id, resolve_session_from, resolve_workspace_from, validate_workspace_id,
};
use crate::write::{detect_secret_hits, normalize_emoji_shortcode};
use crate::{CLI_VERSION, Ctx};

/// Protocol versions this server speaks. A client asking for anything else is answered with
/// [`DEFAULT_PROTOCOL_VERSION`], as the hand-rolled server did.
const SUPPORTED_PROTOCOL_VERSIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2024_11_05,
];
const DEFAULT_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V_2025_03_26;

/// Methods this server implements; a request for one of them whose params fail to parse is a
/// `-32602` (not the `-32601` the SDK reports for everything it cannot type).
const KNOWN_METHODS: &[&str] = &[
    "initialize",
    "ping",
    "tools/list",
    "tools/call",
    "resources/list",
    "resources/templates/list",
    "resources/read",
    "prompts/list",
    "prompts/get",
];

const SERVER_INSTRUCTIONS: &str = "Query local Slack SQLite archives offline (workspaces, channels, FTS5 search including audio/video file transcripts, channel history, threads, transcripts, Markdown/JSON exports, and catch-up digests), read MCP resources (`slackless://...`), invoke built-in prompts (`catch_up`, `summarize_thread`, `summarize_channel`, `find_decisions`, `search_transcripts`), and perform live Slack writes (send, reply, react) using stored slackless sessions.";

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

/// Entrypoint for `slackless mcp`: the validated `--workspace` is the default workspace, and
/// `read_only` (`--read-only`) withholds the tools that change Slack. Stored sessions are read
/// once, before serving. Serves MCP over stdin/stdout until the client closes the connection.
pub async fn run_mcp(ctx: &Ctx, read_only: bool) -> Result<ExitCode> {
    let server = McpServer::new(
        ctx.flag().map(str::to_owned),
        read_only,
        ctx.sessions().to_vec(),
    );
    let running = server.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(ExitCode::SUCCESS)
}

/// MCP server state shared by every tool, resource, and prompt handler.
pub(crate) struct McpServer {
    default_workspace: Option<String>,
    /// `--read-only`: tools that change Slack ([`is_write_tool`]) are neither listed nor run.
    read_only: bool,
    /// When `Some`, reads `<dir>/<TEAM_ID>.sqlite` instead of the default app data directory
    /// (used by tests to isolate from `$HOME`).
    archive_dir: Option<PathBuf>,
    /// Stored sessions, read once at startup: a read spawns `/usr/bin/security` per session,
    /// which must not block a request. Tests pass their own.
    sessions: Vec<Session>,
    tool_router: ToolRouter<Self>,
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
    pub(crate) fn new(
        default_workspace: Option<String>,
        read_only: bool,
        sessions: Vec<Session>,
    ) -> Self {
        Self::with_archives(default_workspace, read_only, None, sessions)
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        archive_dir: PathBuf,
        sessions: Vec<Session>,
        default_workspace: Option<String>,
        read_only: bool,
    ) -> Self {
        Self::with_archives(default_workspace, read_only, Some(archive_dir), sessions)
    }

    fn with_archives(
        default_workspace: Option<String>,
        read_only: bool,
        archive_dir: Option<PathBuf>,
        sessions: Vec<Session>,
    ) -> Self {
        Self {
            default_workspace,
            read_only,
            archive_dir,
            sessions,
            tool_router: Self::tool_router(),
        }
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
        all_workspace_ids(&self.sessions, self.stored_team_ids())
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
        let archive_ids = self.stored_team_ids();
        Ok(resolve_workspace_from(flag, &self.sessions, &archive_ids))
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
        Archive::open_rw_or_create_path(&path).map_err(|err| format!("{err:#}"))
    }

    fn resolve_live_api(
        &self,
        explicit_workspace: Option<&str>,
    ) -> std::result::Result<(SlackApi, TeamId), (&'static str, String)> {
        let flag = self
            .effective_workspace_flag(explicit_workspace)
            .map_err(|msg| ("invalid_args", msg))?;
        let session = resolve_session_from(flag, &self.sessions)
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
        let recorded = self.open_archive_rw(workspace).and_then(|mut archive| {
            archive
                .store_mut()
                .upsert_message(workspace, channel, message)
                .map_err(|err| format!("recording the sent message: {err}"))
        });
        // stderr: stdout carries the MCP protocol.
        if let Err(err) = recorded {
            eprintln!("slackless: sent, but could not record the echo locally ({err})");
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
                match self.sessions.iter().find(|s| s.team_id == team_id) {
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
        kind: ChannelKind,
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
        if matches!(kind, ChannelKind::All | ChannelKind::Channels) {
            rows.extend(
                archive
                    .channels(workspace)
                    .map_err(|err| format!("listing channels: {err}"))?,
            );
        }
        if matches!(kind, ChannelKind::All | ChannelKind::Dms) {
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

    fn collect_transcripts(
        &self,
        workspaces: &[TeamId],
        kind: TranscriptKind,
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

            if matches!(kind, TranscriptKind::All | TranscriptKind::File) {
                let allowed_files: Option<HashSet<FileId>> =
                    if let Some(ch_id) = resolved_channel_id.as_ref() {
                        Some(
                            archive
                                .store()
                                .file_transcript_ids_in_channel(ws, ch_id)
                                .map_err(|err| {
                                    format!("listing files in channel {ch_id} of {ws}: {err}")
                                })?,
                        )
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
                        excerpt: truncate_chars(&tr.text, MAX_EXCERPT_CHARS),
                    });
                }
            }

            if matches!(kind, TranscriptKind::All | TranscriptKind::Huddle) {
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
                        excerpt: truncate_chars(&session.preview, MAX_EXCERPT_CHARS),
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
    /// The one workspace a tool should read, or the `no_workspace`/`invalid_args` tool error.
    fn require_workspace(&self, explicit: Option<&str>) -> Result<TeamId, ToolError> {
        match self.resolve_workspace(explicit) {
            Ok(Some(ws)) => Ok(ws),
            Ok(None) => Err(tool_err(
                "no_workspace",
                "no workspace resolved; pass `workspace` (call `list_workspaces` to see available team IDs)",
            )),
            Err(msg) => Err(invalid_args(msg)),
        }
    }
}

// -----------------------------------------------------------------------------
// Typed tool arguments
// -----------------------------------------------------------------------------

/// A tool's outcome: its success payload, or a structured error (`isError: true` with
/// `{"error": <code>, "message": ...}`) that the caller can act on.
type ToolResult = Result<CallToolResult, ToolError>;

/// A failed tool run (`isError: true`), boxed so every `Result` carrying it stays small.
struct ToolError(Box<CallToolResult>);

impl IntoCallToolResult for ToolError {
    fn into_call_tool_result(self) -> Result<CallToolResponse, ErrorData> {
        (*self.0).into_call_tool_result()
    }
}

/// Typed tool arguments. Deserialization failures are captured here and surfaced by
/// [`Args::get`] as `invalid_args` tool results rather than JSON-RPC protocol errors.
struct Args<P>(Result<P, String>);

impl<'de, P: DeserializeOwned> Deserialize<'de> for Args<P> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Ok(Self(
            serde_json::from_value(value).map_err(|err| err.to_string()),
        ))
    }
}

impl<P: JsonSchema> JsonSchema for Args<P> {
    fn schema_name() -> Cow<'static, str> {
        P::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        P::json_schema(generator)
    }
}

impl<P> Args<P> {
    fn get(self) -> Result<P, ToolError> {
        self.0.map_err(invalid_args)
    }
}

// A required string argument that must contain non-whitespace text. Plain `//` docs keep the
// type's purpose out of the generated schema; the field docs describe each parameter.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(transparent)]
#[schemars(inline)]
struct NonEmpty(String);

impl fmt::Display for NonEmpty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::ops::Deref for NonEmpty {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for NonEmpty {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        if raw.trim().is_empty() {
            return Err(serde::de::Error::custom("must be a non-empty string"));
        }
        Ok(Self(raw))
    }
}

impl NonEmpty {
    fn as_str(&self) -> &str {
        &self.0
    }

    fn into_string(self) -> String {
        self.0
    }
}

// A workspace team ID: ASCII letters, digits, `_`, or `-` only, so it is safe in archive paths.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(transparent)]
#[schemars(inline)]
struct Workspace(String);

impl std::ops::Deref for Workspace {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

/// Deserializes an optional `workspace` argument: blank input means "none", anything else must
/// be a valid team ID.
fn workspace_arg<'de, D>(deserializer: D) -> Result<Option<Workspace>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    match raw.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(ws) => validate_workspace_id(&ws)
            .map(Workspace)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

/// Deserializes an optional enum argument case-insensitively (`"ALL"` and `"all"` are `All`).
fn ci_enum<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    match raw {
        None => Ok(None),
        Some(raw) => {
            let normalized = Value::String(raw.trim().to_ascii_lowercase());
            T::deserialize(normalized)
                .map(Some)
                .map_err(serde::de::Error::custom)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(inline)]
enum ChannelKind {
    #[default]
    All,
    Channels,
    Dms,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(inline)]
enum TranscriptKind {
    #[default]
    All,
    File,
    Huddle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(inline)]
enum Sort {
    Recent,
    Relevant,
}

impl From<Sort> for SearchSort {
    fn from(sort: Sort) -> Self {
        match sort {
            Sort::Recent => Self::Recent,
            Sort::Relevant => Self::Relevant,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(inline)]
enum ExportKind {
    #[serde(alias = "markdown")]
    Md,
    Json,
}

impl From<ExportKind> for ExportFormat {
    fn from(kind: ExportKind) -> Self {
        match kind {
            ExportKind::Md => Self::Markdown,
            ExportKind::Json => Self::Json,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
struct ListChannelsArgs {
    /// Workspace team ID (e.g. T0123ABC). Defaults to the server's default workspace.
    #[serde(default, deserialize_with = "workspace_arg")]
    workspace: Option<Workspace>,
    /// Conversation kinds to return (default: "all").
    #[serde(default, deserialize_with = "ci_enum")]
    kind: Option<ChannelKind>,
    /// Optional case-insensitive substring filter across channel id, #name, topic, or DM peer name.
    filter: Option<String>,
    /// Include archived channels (default: true).
    include_archived: Option<bool>,
    /// Maximum conversations to return (default: 200).
    #[schemars(range(min = 1, max = 1000))]
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
struct SearchMessagesArgs {
    /// Search query text and optional Slack-style modifiers.
    query: NonEmpty,
    /// Optional workspace team ID to restrict search to one archive.
    #[serde(default, deserialize_with = "workspace_arg")]
    workspace: Option<Workspace>,
    /// Search across every locally archived workspace and merge hits newest first.
    all_workspaces: Option<bool>,
    /// Result ordering: "recent" (newest first, default) or "relevant" (FTS5 BM25 relevance).
    #[serde(default, deserialize_with = "ci_enum")]
    sort: Option<Sort>,
    /// Maximum messages to return (default: 20).
    #[schemars(range(min = 1, max = 200))]
    limit: Option<usize>,
    /// Include locally preserved deleted message tombstones (default: false).
    include_deleted: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct ReadChannelArgs {
    /// Channel ID (C…/D…/G…), #name, bare name, or @handle for a DM.
    channel: NonEmpty,
    /// Optional workspace team ID.
    #[serde(default, deserialize_with = "workspace_arg")]
    workspace: Option<Workspace>,
    /// Maximum messages to return (default: 50).
    #[schemars(range(min = 1, max = 200))]
    limit: Option<usize>,
    /// Optional lower time bound: relative duration (30m, 24h, 7d, 2w), UTC date (YYYY-MM-DD), or unix epoch seconds.
    since: Option<String>,
    /// Optional Slack message ts cursor; returns only messages strictly older than `before`.
    before: Option<String>,
    /// Include locally preserved deleted messages (default: false).
    include_deleted: Option<bool>,
    /// Fetch live via conversations.history and mirror locally (requires a signed-in session; default: false).
    live: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct ReadThreadArgs {
    /// Channel ID, #name, or @handle.
    channel: NonEmpty,
    /// Timestamp (`ts`) of the thread root message.
    thread_ts: NonEmpty,
    /// Optional workspace team ID.
    #[serde(default, deserialize_with = "workspace_arg")]
    workspace: Option<Workspace>,
    /// Fetch fresh replies live via conversations.replies and mirror locally (default: false).
    live: Option<bool>,
    /// Maximum messages to return (default: 200).
    #[schemars(range(min = 1, max = 999))]
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
struct GetTranscriptArgs {
    /// Slack file ID (e.g. F0123ABC).
    file_id: NonEmpty,
    /// Optional workspace team ID. When omitted, checks the default workspace then all archived workspaces.
    #[serde(default, deserialize_with = "workspace_arg")]
    workspace: Option<Workspace>,
}

#[derive(Deserialize, JsonSchema)]
struct ListTranscriptsArgs {
    /// Optional workspace team ID.
    #[serde(default, deserialize_with = "workspace_arg")]
    workspace: Option<Workspace>,
    /// List transcripts across every locally archived workspace.
    all_workspaces: Option<bool>,
    /// Transcript kinds to return (default: "all").
    #[serde(default, deserialize_with = "ci_enum")]
    kind: Option<TranscriptKind>,
    /// Optional channel ID, #name, or @handle to filter transcripts.
    channel: Option<NonEmpty>,
    /// Maximum transcripts to return (default: 50).
    #[schemars(range(min = 1, max = 200))]
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
struct ExportConversationArgs {
    /// Optional workspace team ID.
    #[serde(default, deserialize_with = "workspace_arg")]
    workspace: Option<Workspace>,
    /// Channel ID, #name, or @handle (for channel or thread export).
    channel: Option<NonEmpty>,
    /// Optional thread root `ts` (requires `channel`) to export a single thread.
    thread_ts: Option<NonEmpty>,
    /// Slack file ID to export a stored audio/video transcript (mutually exclusive with `channel` and `huddle_id`).
    file_id: Option<NonEmpty>,
    /// Huddle call ID to export a stored huddle transcript (mutually exclusive with `channel` and `file_id`).
    huddle_id: Option<NonEmpty>,
    /// Output format: "md" (Markdown, default) or "json".
    #[serde(default, deserialize_with = "ci_enum")]
    format: Option<ExportKind>,
    /// When exporting a channel, nest stored thread replies under thread roots (default: false).
    include_threads: Option<bool>,
    /// Include locally preserved deleted message tombstones (default: false).
    include_deleted: Option<bool>,
    /// Include stored/inline audio/video transcripts attached to exported messages (default: true).
    include_transcripts: Option<bool>,
    /// Maximum top-level channel messages to export (default: 500).
    #[schemars(range(min = 1, max = 5000))]
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
struct CatchupSummaryArgs {
    /// Optional workspace team ID.
    #[serde(default, deserialize_with = "workspace_arg")]
    workspace: Option<Workspace>,
    /// Aggregate catch-up activity across every locally archived workspace.
    all_workspaces: Option<bool>,
    /// Lookback window in hours (default: 24, max: 720).
    #[schemars(range(min = 1, max = 720))]
    since_hours: Option<usize>,
    /// Maximum items per section (channels, mentions, active_threads, new_transcripts; default: 20).
    #[schemars(range(min = 1, max = 100))]
    limit: Option<usize>,
    /// Optional Slack user ID (e.g. U0123ABC) to detect direct `<@USER>` mentions for. Defaults to the signed-in session's user ID per workspace.
    user_id: Option<NonEmpty>,
}

#[derive(Deserialize, JsonSchema)]
struct SendMessageArgs {
    /// Channel ID, #name, or @handle.
    channel: NonEmpty,
    /// Message body text.
    text: NonEmpty,
    /// Optional workspace team ID.
    #[serde(default, deserialize_with = "workspace_arg")]
    workspace: Option<Workspace>,
    /// Allow sending text that triggers the pre-send secret-leak detector (default: false).
    allow_secrets: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct ReplyThreadArgs {
    /// Channel ID, #name, or @handle.
    channel: NonEmpty,
    /// Timestamp (`ts`) of the thread root message.
    thread_ts: NonEmpty,
    /// Reply body text.
    text: NonEmpty,
    /// Also broadcast the reply to the main channel timeline (default: false).
    reply_broadcast: Option<bool>,
    /// Optional workspace team ID.
    #[serde(default, deserialize_with = "workspace_arg")]
    workspace: Option<Workspace>,
    /// Allow sending text that triggers the pre-send secret-leak detector (default: false).
    allow_secrets: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct AddReactionArgs {
    /// Channel ID, #name, or @handle.
    channel: NonEmpty,
    /// Timestamp (`ts`) of the message to react to.
    ts: NonEmpty,
    /// Emoji shortcode with or without colons (e.g. "tada" or ":tada:").
    emoji: NonEmpty,
    /// Optional workspace team ID.
    #[serde(default, deserialize_with = "workspace_arg")]
    workspace: Option<Workspace>,
}

// -----------------------------------------------------------------------------
// Tools
// -----------------------------------------------------------------------------

#[tool_router]
impl McpServer {
    #[tool(
        name = "list_workspaces",
        description = "List stored Slack workspaces (signed-in sessions and local SQLite archives) with message counts and default workspace.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn list_workspaces(&self) -> ToolResult {
        Ok(tool_ok(&self.list_workspaces_payload()))
    }

    #[tool(
        name = "list_channels",
        description = "List channels and/or DMs known to the local archive for a workspace, with optional kind and substring filters.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn list_channels(&self, Parameters(args): Parameters<Args<ListChannelsArgs>>) -> ToolResult {
        let ListChannelsArgs {
            workspace,
            kind,
            filter,
            include_archived,
            limit,
        } = args.get()?;
        let filter = filter.map(|f| f.trim().trim_start_matches(['#', '@']).to_ascii_lowercase());
        let limit = limit
            .unwrap_or(DEFAULT_CHANNEL_LIST_LIMIT)
            .clamp(1, MAX_CHANNEL_LIST_LIMIT);
        let workspace = self.require_workspace(workspace.as_deref())?;
        self.list_channels_payload(
            &workspace,
            kind.unwrap_or_default(),
            filter.as_deref(),
            include_archived.unwrap_or(true),
            limit,
        )
        .map(|payload| tool_ok(&payload))
        .map_err(|msg| tool_err("store_error", msg))
    }

    #[tool(
        name = "search_messages",
        description = "Full-text FTS5 search over the local Slack SQLite archive, including message text, file names, and audio/video file transcripts. Supports Slack modifiers: in:#ch, from:@user, has:file|link|reaction|pin, before:YYYY-MM-DD, after:YYYY-MM-DD, -exclude, \"phrase\", prefix*.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn search_messages(
        &self,
        Parameters(args): Parameters<Args<SearchMessagesArgs>>,
    ) -> ToolResult {
        let SearchMessagesArgs {
            query,
            workspace,
            all_workspaces,
            sort,
            limit,
            include_deleted,
        } = args.get()?;
        if workspace.is_some() && all_workspaces == Some(true) {
            return Err(invalid_args(
                "cannot specify both `workspace` and `all_workspaces: true`",
            ));
        }
        let sort = sort.unwrap_or(Sort::Recent);
        let ranking = SearchSort::from(sort);
        let limit = limit
            .unwrap_or(DEFAULT_SEARCH_LIMIT)
            .clamp(1, MAX_SEARCH_LIMIT);
        let mut parsed = SearchQuery::parse(query.as_str());
        parsed.filters.include_deleted = include_deleted.unwrap_or(false);

        let search_all = match (workspace.as_deref(), all_workspaces) {
            (Some(_), _) => false,
            (None, Some(all)) => all,
            (None, None) => match self.resolve_workspace(None) {
                Ok(ws) => ws.is_none(),
                Err(msg) => return Err(invalid_args(msg)),
            },
        };

        if search_all {
            let archives: Vec<(TeamId, PathBuf)> = self
                .all_workspace_ids()
                .into_iter()
                .filter_map(|ws| self.archive_path_for(&ws).map(|path| (ws, path)))
                .collect();
            let results = search_all_in_archives(&archives, &parsed, ranking, limit)?;
            let (json, _) = search_all_document(query.as_str(), None, ranking, &results);
            return Ok(tool_ok(&json));
        }

        let workspace = match self.resolve_workspace(workspace.as_deref()) {
            Ok(Some(ws)) => ws,
            Ok(None) => {
                return Err(tool_err(
                    "no_workspace",
                    "no workspace resolved; pass `workspace` or set `all_workspaces: true`",
                ));
            }
            Err(msg) => return Err(tool_err("invalid_args", msg)),
        };
        let archive = match self.open_archive(&workspace) {
            Ok(a) => a,
            Err(msg) => return Err(tool_err("store_error", msg)),
        };
        let ((json, _), _) = search_document(
            &workspace,
            archive.as_ref(),
            query.as_str(),
            &parsed,
            ranking,
            limit,
        )?;
        Ok(tool_ok(&json))
    }

    #[tool(
        name = "read_channel",
        description = "Read recent or windowed messages from a channel or DM (oldest first), from the local SQLite archive by default or live from Slack when `live: true`.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn read_channel(
        &self,
        Parameters(args): Parameters<Args<ReadChannelArgs>>,
    ) -> ToolResult {
        let ReadChannelArgs {
            channel,
            workspace,
            limit,
            since,
            before,
            include_deleted,
            live,
        } = args.get()?;
        let limit = limit.unwrap_or(DEFAULT_READ_LIMIT).clamp(1, MAX_READ_LIMIT);
        let cutoff_secs = match since.as_deref() {
            Some(spec) => Some(parse_since_cutoff(spec, now_epoch_secs()).map_err(invalid_args)?),
            None => None,
        };
        let window = ChannelWindow {
            before: before.filter(|s| !s.trim().is_empty()).map(Ts::from),
            since_secs: cutoff_secs,
            limit,
            include_deleted: include_deleted.unwrap_or(false),
        };
        let live = live.unwrap_or(false);

        if live {
            let (api, workspace) = match self.resolve_live_api(workspace.as_deref()) {
                Ok(pair) => pair,
                Err((code, msg)) => return Err(tool_err(code, msg)),
            };
            let mut archive = self.open_archive_rw(&workspace);
            let read = live_channel_read(&api, &workspace, &mut archive, channel.as_str(), &window)
                .await
                .map_err(|err| {
                    let (code, msg) = classify_api_error(&err);
                    tool_err(code, msg)
                })?;
            return Ok(tool_ok(&channel_json(&read, &workspace, true)));
        }

        let workspace = self.require_workspace(workspace.as_deref())?;
        let archive = match self.open_archive(&workspace) {
            Ok(a) => a,
            Err(msg) => return Err(tool_err("store_error", msg)),
        };
        let read = local_channel(&workspace, archive.as_ref(), channel.as_str(), &window)?;
        Ok(tool_ok(&channel_json(&read, &workspace, false)))
    }

    #[tool(
        name = "read_thread",
        description = "Read a thread's root message and all replies (oldest first), from the local archive by default or live via conversations.replies when `live: true`.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn read_thread(&self, Parameters(args): Parameters<Args<ReadThreadArgs>>) -> ToolResult {
        let ReadThreadArgs {
            channel,
            thread_ts,
            workspace,
            live,
            limit,
        } = args.get()?;
        let live = live.unwrap_or(false);
        let limit = limit
            .unwrap_or(DEFAULT_THREAD_LIMIT)
            .clamp(1, MAX_THREAD_LIMIT);
        let thread_ts = Ts::from(thread_ts.as_str());

        if live {
            let (api, workspace) = match self.resolve_live_api(workspace.as_deref()) {
                Ok(pair) => pair,
                Err((code, msg)) => return Err(tool_err(code, msg)),
            };
            let mut archive = self.open_archive_rw(&workspace);
            let mut read = live_thread_read(
                &api,
                &workspace,
                &mut archive,
                channel.as_str(),
                &thread_ts,
                u32::try_from(limit).unwrap_or(u32::MAX),
            )
            .await
            .map_err(|err| {
                let (code, msg) = classify_api_error(&err);
                tool_err(code, msg)
            })?;
            read.messages.truncate(limit);
            return Ok(tool_ok(&thread_json(
                &read,
                &thread_ts,
                true,
                Some(&workspace),
            )));
        }

        let workspace = self.require_workspace(workspace.as_deref())?;
        let archive = match self.open_archive(&workspace) {
            Ok(a) => a,
            Err(msg) => return Err(tool_err("store_error", msg)),
        };
        let mut read = local_thread(&workspace, archive.as_ref(), channel.as_str(), &thread_ts)?;
        read.messages.truncate(limit);
        Ok(tool_ok(&thread_json(
            &read,
            &thread_ts,
            false,
            Some(&workspace),
        )))
    }

    #[tool(
        name = "get_transcript",
        description = "Fetch a stored audio/video file transcript (including timed cues and duration when available) from the local SQLite archive by Slack `file_id`.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn get_transcript(&self, Parameters(args): Parameters<Args<GetTranscriptArgs>>) -> ToolResult {
        let GetTranscriptArgs { file_id, workspace } = args.get()?;
        let candidates = self
            .workspace_candidates(workspace.as_deref())
            .map_err(invalid_args)?;
        if candidates.is_empty() {
            return Err(tool_err(
                "no_workspace",
                "no local archive workspaces found; pass `workspace` or sync the desktop app first",
            ));
        }
        let fid = FileId::from(file_id.as_str());
        for ws in &candidates {
            let archive = match self.open_archive(ws) {
                Ok(Some(a)) => a,
                Ok(None) => continue,
                Err(msg) => return Err(tool_err("store_error", msg)),
            };
            match archive.store().file_transcript(ws, &fid) {
                Ok(Some(transcript)) => {
                    return Ok(tool_ok(&file_transcript_payload(ws, &transcript)));
                }
                Ok(None) => continue,
                Err(err) => {
                    return Err(tool_err(
                        "store_error",
                        format!("reading file transcript: {err}"),
                    ));
                }
            }
        }
        Err(tool_err(
            "transcript_not_found",
            format!("no stored transcript found for file_id '{file_id}'"),
        ))
    }

    #[tool(
        name = "list_transcripts",
        description = "List stored audio/video file transcripts and huddle transcript sessions for one or all workspaces with excerpts, durations, and timed cue counts.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn list_transcripts(
        &self,
        Parameters(args): Parameters<Args<ListTranscriptsArgs>>,
    ) -> ToolResult {
        let ListTranscriptsArgs {
            workspace,
            all_workspaces,
            kind,
            channel,
            limit,
        } = args.get()?;
        let limit = limit
            .unwrap_or(DEFAULT_TRANSCRIPT_LIST_LIMIT)
            .clamp(1, MAX_TRANSCRIPT_LIST_LIMIT);
        let kind = kind.unwrap_or_default();
        let (all_workspaces, workspaces) = self
            .resolve_scope_workspaces(workspace.as_deref(), all_workspaces)
            .map_err(|(code, msg)| tool_err(code, msg))?;
        let transcripts = self
            .collect_transcripts(&workspaces, kind, channel.as_deref(), None, limit)
            .map_err(|msg| tool_err("store_error", msg))?;
        let file_count = transcripts.iter().filter(|t| t.kind == "file").count();
        let huddle_count = transcripts.iter().filter(|t| t.kind == "huddle").count();
        let searched: Vec<&str> = workspaces.iter().map(TeamId::as_str).collect();

        Ok(tool_ok(&json!({
            "all_workspaces": all_workspaces,
            "workspaces_searched": searched,
            "kind": kind,
            "count": transcripts.len(),
            "file_count": file_count,
            "huddle_count": huddle_count,
            "transcripts": transcripts,
        })))
    }

    #[tool(
        name = "export_conversation",
        description = "Export a locally archived channel, thread, audio/video file transcript, or huddle transcript as Markdown or JSON returned directly as tool content (no filesystem writes).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn export_conversation(
        &self,
        Parameters(args): Parameters<Args<ExportConversationArgs>>,
    ) -> ToolResult {
        let ExportConversationArgs {
            workspace,
            channel: channel_arg,
            thread_ts,
            file_id,
            huddle_id,
            format,
            include_threads,
            include_deleted,
            include_transcripts,
            limit,
        } = args.get()?;
        let format = format.unwrap_or(ExportKind::Md);
        let export_opts = ExportOptions {
            format: ExportFormat::from(format),
            limit: limit
                .unwrap_or(DEFAULT_EXPORT_LIMIT)
                .clamp(1, MAX_EXPORT_LIMIT),
            include_deleted: include_deleted.unwrap_or(false),
            include_threads: include_threads.unwrap_or(false),
            include_transcripts: include_transcripts.unwrap_or(true),
        };

        if thread_ts.is_some() && channel_arg.is_none() {
            return Err(invalid_args("`thread_ts` requires `channel`"));
        }
        let target_count = usize::from(channel_arg.is_some())
            + usize::from(file_id.is_some())
            + usize::from(huddle_id.is_some());
        if target_count == 0 {
            return Err(invalid_args(
                "provide exactly one export target: `channel` (optionally with `thread_ts`), `file_id`, or `huddle_id`",
            ));
        }
        if target_count > 1 {
            return Err(invalid_args(
                "`channel`, `file_id`, and `huddle_id` are mutually exclusive",
            ));
        }
        let thread_ts = thread_ts.as_deref().map(Ts::from);
        let file_id = file_id.as_deref().map(FileId::from);
        let req = if let Some(file_id) = file_id.as_ref() {
            ExportRequest::FileTranscript { file_id }
        } else if let Some(call_id) = huddle_id.as_deref() {
            ExportRequest::HuddleTranscript { call_id }
        } else {
            ExportRequest::Channel {
                channel: channel_arg
                    .as_deref()
                    .expect("checked channel_arg.is_some()"),
                thread_ts: thread_ts.as_ref(),
            }
        };

        // A transcript or huddle may live in any archived workspace; a channel is read from the
        // resolved one, from an empty archive at worst.
        let stored = |ws: &TeamId, archive: Option<&Archive>| -> Result<_, ToolError> {
            Ok(export_archive(ws, archive, &req, &export_opts)?
                .map(|(artifact, channel)| export_json(ws, &req, artifact, channel, format)))
        };
        if matches!(req, ExportRequest::Channel { .. }) {
            let workspace = self.require_workspace(workspace.as_deref())?;
            let archive = match self.open_archive(&workspace) {
                Ok(a) => a,
                Err(msg) => return Err(tool_err("store_error", msg)),
            };
            let json = stored(&workspace, archive.as_ref())?
                .ok_or_else(|| invalid_args("nothing to export"))?;
            return Ok(tool_ok(&json));
        }
        let candidates = match self.workspace_candidates(workspace.as_deref()) {
            Ok(v) => v,
            Err(msg) => return Err(tool_err("invalid_args", msg)),
        };
        if candidates.is_empty() {
            return Err(tool_err(
                "no_workspace",
                "no local archive workspaces found; pass `workspace` or sync the desktop app first",
            ));
        }
        for ws in &candidates {
            let archive = match self.open_archive(ws) {
                Ok(Some(a)) => a,
                Ok(None) => continue,
                Err(msg) => return Err(tool_err("store_error", msg)),
            };
            if let Some(json) = stored(ws, Some(&archive))? {
                return Ok(tool_ok(&json));
            }
        }
        Err(tool_err("not_found", export_not_found(&req)))
    }

    #[tool(
        name = "get_catchup_summary",
        description = "Build a local SQLite catch-up digest of recent activity (unread/recent channels, direct mentions, active threads, and new transcripts) over a `since_hours` window across one or all workspaces.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn get_catchup_summary(
        &self,
        Parameters(args): Parameters<Args<CatchupSummaryArgs>>,
    ) -> ToolResult {
        let CatchupSummaryArgs {
            workspace,
            all_workspaces,
            since_hours,
            limit,
            user_id,
        } = args.get()?;
        let since_hours = since_hours
            .unwrap_or(DEFAULT_CATCHUP_HOURS)
            .clamp(1, MAX_CATCHUP_HOURS);
        let limit = limit
            .unwrap_or(DEFAULT_CATCHUP_LIMIT)
            .clamp(1, MAX_CATCHUP_LIMIT);
        let (_, workspaces) = self
            .resolve_scope_workspaces(workspace.as_deref(), all_workspaces)
            .map_err(|(code, msg)| tool_err(code, msg))?;
        let briefing = self.catchup_over(&workspaces, since_hours, limit, user_id.as_deref())?;
        Ok(tool_ok(&briefing))
    }

    #[tool(
        name = "send_message",
        description = "Post a live message to a Slack channel or DM via chat.postMessage and mirror it into the local archive (requires a signed-in session).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn send_message(
        &self,
        Parameters(args): Parameters<Args<SendMessageArgs>>,
    ) -> ToolResult {
        let SendMessageArgs {
            channel,
            text,
            workspace,
            allow_secrets,
        } = args.get()?;
        if let Some(err) = check_secrets_mcp(&text, allow_secrets.unwrap_or(false)) {
            return Err(err);
        }
        let (api, workspace) = match self.resolve_live_api(workspace.as_deref()) {
            Ok(pair) => pair,
            Err((code, msg)) => return Err(tool_err(code, msg)),
        };
        let archive = self.open_archive(&workspace).ok().flatten();
        let (channel_id, _label) = resolve_channel_with(archive.as_ref(), &workspace, &channel);

        let posted = match api
            .chat_post_message(
                &channel_id,
                &MessageContent::plain(text.into_string()),
                None,
                false,
            )
            .await
        {
            Ok(p) => p,
            Err(err) => {
                let (code, msg) = classify_api_error(&err);
                return Err(tool_err(code, msg));
            }
        };
        self.record_echo(&workspace, &channel_id, posted.message.as_ref());

        Ok(tool_ok(&json!({
            "ok": true,
            "workspace": workspace.as_str(),
            "channel": channel_id.as_str(),
            "ts": posted.ts.as_str(),
        })))
    }

    #[tool(
        name = "reply_thread",
        description = "Post a live reply into a Slack thread via chat.postMessage and mirror it into the local archive (requires a signed-in session).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn reply_thread(
        &self,
        Parameters(args): Parameters<Args<ReplyThreadArgs>>,
    ) -> ToolResult {
        let ReplyThreadArgs {
            channel,
            thread_ts,
            text,
            reply_broadcast,
            workspace,
            allow_secrets,
        } = args.get()?;
        if let Some(err) = check_secrets_mcp(&text, allow_secrets.unwrap_or(false)) {
            return Err(err);
        }
        let (api, workspace) = match self.resolve_live_api(workspace.as_deref()) {
            Ok(pair) => pair,
            Err((code, msg)) => return Err(tool_err(code, msg)),
        };
        let archive = self.open_archive(&workspace).ok().flatten();
        let (channel_id, _label) = resolve_channel_with(archive.as_ref(), &workspace, &channel);
        let thread_ts = Ts::from(thread_ts.as_str());

        let posted = match api
            .chat_post_message(
                &channel_id,
                &MessageContent::plain(text.into_string()),
                Some(&thread_ts),
                reply_broadcast.unwrap_or(false),
            )
            .await
        {
            Ok(p) => p,
            Err(err) => {
                let (code, msg) = classify_api_error(&err);
                return Err(tool_err(code, msg));
            }
        };
        self.record_echo(&workspace, &channel_id, posted.message.as_ref());

        Ok(tool_ok(&json!({
            "ok": true,
            "workspace": workspace.as_str(),
            "channel": channel_id.as_str(),
            "thread_ts": thread_ts.as_str(),
            "ts": posted.ts.as_str(),
        })))
    }

    #[tool(
        name = "add_reaction",
        description = "Add a live emoji reaction to a Slack message via reactions.add (requires a signed-in session).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn add_reaction(
        &self,
        Parameters(args): Parameters<Args<AddReactionArgs>>,
    ) -> ToolResult {
        let AddReactionArgs {
            channel,
            ts,
            emoji,
            workspace,
        } = args.get()?;
        let name = normalize_emoji_shortcode(emoji.as_str());
        if name.is_empty() {
            return Err(invalid_args("`emoji` cannot be empty"));
        }
        let (api, workspace) = match self.resolve_live_api(workspace.as_deref()) {
            Ok(pair) => pair,
            Err((code, msg)) => return Err(tool_err(code, msg)),
        };
        let archive = self.open_archive(&workspace).ok().flatten();
        let (channel_id, _label) = resolve_channel_with(archive.as_ref(), &workspace, &channel);
        let ts = Ts::from(ts.as_str());

        match api.reactions_add(&channel_id, &ts, &name).await {
            Ok(()) => Ok(tool_ok(&json!({
                "ok": true,
                "workspace": workspace.as_str(),
                "channel": channel_id.as_str(),
                "ts": ts.as_str(),
                "name": name,
            }))),
            Err(err) => {
                let (code, msg) = classify_api_error(&err);
                Err(tool_err(code, msg))
            }
        }
    }
}

// -----------------------------------------------------------------------------
// MCP protocol surface: capabilities, tool routing, resources, prompts
// -----------------------------------------------------------------------------

/// Whether `tool` may change Slack, i.e. is not annotated `readOnlyHint: true`; `--read-only`
/// neither lists nor runs these.
fn is_write_tool(tool: &Tool) -> bool {
    tool.annotations
        .as_ref()
        .is_none_or(|a| a.read_only_hint != Some(true))
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("slackless", CLI_VERSION))
        .with_protocol_version(DEFAULT_PROTOCOL_VERSION)
        .with_instructions(SERVER_INSTRUCTIONS)
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(SUPPORTED_PROTOCOL_VERSIONS)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if self.read_only
            && self
                .tool_router
                .get(&request.name)
                .is_some_and(is_write_tool)
        {
            return Err(invalid_params(format!(
                "tool `{}` changes Slack and is disabled: the server runs with --read-only",
                request.name
            )));
        }
        self.tool_router
            .call(ToolCallContext::new(self, request, context))
            .await
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut tools = self.tool_router.list_all();
        if self.read_only {
            tools.retain(|tool| !is_write_tool(tool));
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new("slackless://workspaces", "workspaces")
                .with_title("Slack Workspaces")
                .with_description(
                    "All stored Slack workspaces (signed-in sessions and local SQLite archives) with message counts.",
                )
                .with_mime_type("application/json"),
        ]))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            template(
                "slackless://workspace/{workspace}/channels",
                "workspace_channels",
                "Workspace Channels & DMs",
                "Channels and direct messages archived locally for a workspace.",
            ),
            template(
                "slackless://workspace/{workspace}/channel/{channel}",
                "channel_messages",
                "Channel Recent Messages",
                "Recent locally archived messages from a channel or DM (accepts channel ID, name, or @handle).",
            ),
            template(
                "slackless://workspace/{workspace}/thread/{channel}/{ts}",
                "thread_messages",
                "Thread Conversation",
                "Root message and replies for a thread from the local SQLite archive.",
            ),
            template(
                "slackless://workspace/{workspace}/transcript/{file_id}",
                "file_transcript",
                "Audio/Video File Transcript",
                "Stored audio/video file transcript and timed cues from the local SQLite archive.",
            ),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri.trim();
        if uri.is_empty() {
            return Err(invalid_params(
                "resources/read requires a non-empty string `uri`",
            ));
        }
        let text = serde_json::to_string_pretty(&self.read_resource_payload(uri)?)
            .map_err(|err| internal_error(format!("serializing resource payload: {err}")))?;
        Ok(
            ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
                uri: uri.to_owned(),
                mime_type: Some("application/json".to_owned()),
                text,
                meta: None,
            }])
            .into(),
        )
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult::with_all_items(vec![
            prompt(
                "catch_up",
                "Summarize recent Slack activity, direct mentions, active threads, and new transcripts over a lookback window.",
                &[
                    argument(
                        "workspace",
                        "Optional workspace team ID (defaults to default workspace or all workspaces).",
                        false,
                    ),
                    argument(
                        "since_hours",
                        "Lookback window in hours (default: 24, max: 720).",
                        false,
                    ),
                ],
            ),
            prompt(
                "summarize_thread",
                "Summarize a Slack thread with key takeaways, decisions, action items, and open questions.",
                &[
                    argument("channel", "Channel ID, #name, or @handle.", true),
                    argument(
                        "thread_ts",
                        "Timestamp (`ts`) of the thread root message.",
                        true,
                    ),
                    argument("workspace", "Optional workspace team ID.", false),
                ],
            ),
            prompt(
                "summarize_channel",
                "Summarize recent discussions, themes, and blockers in a Slack channel or DM.",
                &[
                    argument("channel", "Channel ID, #name, or @handle.", true),
                    argument(
                        "since",
                        "Optional lookback cutoff (e.g. \"24h\", \"7d\", \"YYYY-MM-DD\", or unix epoch seconds).",
                        false,
                    ),
                    argument(
                        "limit",
                        "Maximum messages to include (default: 50, max: 100).",
                        false,
                    ),
                    argument("workspace", "Optional workspace team ID.", false),
                ],
            ),
            prompt(
                "find_decisions",
                "Extract concrete or tentative decisions, owners, rationale, and open questions from a channel or query.",
                &[
                    argument(
                        "channel",
                        "Optional channel ID, #name, or @handle to focus on.",
                        false,
                    ),
                    argument(
                        "query",
                        "Optional topic keywords or Slack search modifiers.",
                        false,
                    ),
                    argument(
                        "since",
                        "Optional lower time bound (e.g. \"7d\", \"2026-01-01\").",
                        false,
                    ),
                    argument("workspace", "Optional workspace team ID.", false),
                ],
            ),
            prompt(
                "search_transcripts",
                "Search and synthesize stored audio/video file transcripts and huddle transcripts for a topic.",
                &[
                    argument(
                        "query",
                        "Keywords or topic to search across stored audio/video and huddle transcripts.",
                        true,
                    ),
                    argument("workspace", "Optional workspace team ID.", false),
                ],
            ),
        ]))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        let GetPromptRequestParams {
            name, arguments, ..
        } = request;
        let name = name.trim();
        if name.is_empty() {
            return Err(invalid_params(
                "prompts/get requires a non-empty string `name`",
            ));
        }
        let args = arguments.unwrap_or_default();
        let result = match name {
            "catch_up" => self.prompt_catch_up(&args),
            "summarize_thread" => self.prompt_summarize_thread(&args),
            "summarize_channel" => self.prompt_summarize_channel(&args),
            "find_decisions" => self.prompt_find_decisions(&args),
            "search_transcripts" => self.prompt_search_transcripts(&args),
            other => return Err(invalid_params(format!("unknown prompt: {other}"))),
        }?;
        Ok(result.into())
    }

    async fn on_custom_request(
        &self,
        request: CustomRequest,
        _context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        let method = request.method;
        if KNOWN_METHODS.contains(&method.as_str()) {
            // A supported method whose params failed to deserialize; the SDK routes it here.
            return Err(invalid_params(format!(
                "invalid params: `{method}` received malformed params"
            )));
        }
        Err(ErrorData::new(
            ErrorCode::METHOD_NOT_FOUND,
            format!("method not found: {method}"),
            None,
        ))
    }
}

fn template(uri: &str, name: &str, title: &str, description: &str) -> ResourceTemplate {
    ResourceTemplate::new(uri, name)
        .with_title(title)
        .with_description(description)
        .with_mime_type("application/json")
}

fn prompt(name: &str, description: &str, arguments: &[PromptArgument]) -> Prompt {
    Prompt::new(name, Some(description), Some(arguments.to_vec()))
}

fn argument(name: &str, description: &str, required: bool) -> PromptArgument {
    PromptArgument::new(name)
        .with_description(description)
        .with_required(required)
}

impl McpServer {
    fn read_resource_payload(&self, uri: &str) -> Result<Value, ErrorData> {
        let parsed = parse_resource_uri(uri)?;
        let payload = match parsed {
            ResourceUri::Workspaces => self.list_workspaces_payload(),
            ResourceUri::WorkspaceChannels { workspace } => self
                .list_channels_payload(
                    &workspace,
                    ChannelKind::All,
                    None,
                    true,
                    DEFAULT_CHANNEL_LIST_LIMIT,
                )
                .map_err(internal_error)?,
            ResourceUri::Channel { workspace, channel } => {
                self.read_channel_resource(&workspace, &channel)?
            }
            ResourceUri::Thread {
                workspace,
                channel,
                thread_ts,
            } => self.read_thread_resource(&workspace, &channel, &thread_ts)?,
            ResourceUri::Transcript { workspace, file_id } => {
                let archive = self.open_archive(&workspace).map_err(internal_error)?;
                let Some(archive) = archive else {
                    return Err(resource_not_found(format!(
                        "no local archive for workspace '{workspace}'; transcript '{file_id}' not found"
                    )));
                };
                let transcript = archive
                    .store()
                    .file_transcript(&workspace, &file_id)
                    .map_err(|err| internal_error(format!("reading transcript: {err}")))?;
                let Some(transcript) = transcript else {
                    return Err(resource_not_found(format!(
                        "no stored transcript found for file_id '{file_id}' in workspace '{workspace}'"
                    )));
                };
                file_transcript_payload(&workspace, &transcript)
            }
        };
        Ok(payload)
    }

    fn read_channel_resource(
        &self,
        workspace: &TeamId,
        channel_arg: &str,
    ) -> Result<Value, ErrorData> {
        let archive = self.open_archive(workspace).map_err(internal_error)?;
        let read = local_channel(
            workspace,
            archive.as_ref(),
            channel_arg,
            &ChannelWindow {
                limit: DEFAULT_READ_LIMIT,
                ..ChannelWindow::default()
            },
        )
        .map_err(|err| internal_error(err.message))?;
        Ok(channel_json(&read, workspace, false))
    }

    fn read_thread_resource(
        &self,
        workspace: &TeamId,
        channel_arg: &str,
        thread_ts: &Ts,
    ) -> Result<Value, ErrorData> {
        let archive = self.open_archive(workspace).map_err(internal_error)?;
        let mut read = local_thread(workspace, archive.as_ref(), channel_arg, thread_ts)
            .map_err(|err| internal_error(err.message))?;
        read.messages.truncate(DEFAULT_THREAD_LIMIT);
        Ok(thread_json(&read, thread_ts, false, Some(workspace)))
    }

    // -------------------------------------------------------------------------
    // MCP Prompts (`prompts/get`)
    // -------------------------------------------------------------------------

    /// The catch-up briefing over the last `since_hours` of `workspaces`, looking for
    /// mentions of `user_id` or, by default, of each workspace's signed-in user.
    fn catchup_over(
        &self,
        workspaces: &[TeamId],
        since_hours: usize,
        limit: usize,
        user_id: Option<&str>,
    ) -> Result<slackless_core::catchup::CatchUpBriefing, CmdError> {
        let now = now_epoch_secs();
        let window = CatchUpWindow::Custom {
            since_secs: now.saturating_sub((since_hours as i64).saturating_mul(3_600)),
        };
        let self_users: HashMap<TeamId, UserId> = workspaces
            .iter()
            .filter_map(|ws| {
                let user = user_id.map(UserId::from).or_else(|| {
                    self.sessions
                        .iter()
                        .find(|session| &session.team_id == ws)
                        .map(|session| session.user_id.clone())
                })?;
                Some((ws.clone(), user))
            })
            .collect();
        let archives: Vec<(TeamId, PathBuf)> = workspaces
            .iter()
            .filter_map(|ws| self.archive_path_for(ws).map(|path| (ws.clone(), path)))
            .collect();
        catchup_briefing(&archives, &self_users, window, now, limit)
    }

    fn prompt_catch_up(&self, args: &Map<String, Value>) -> Result<GetPromptResult, ErrorData> {
        let workspace_arg = prompt_opt_workspace(args)?;
        let since_hours = prompt_opt_usize(
            args,
            "since_hours",
            DEFAULT_CATCHUP_HOURS,
            1,
            MAX_CATCHUP_HOURS,
        )?;
        let (_, workspaces) = self
            .resolve_scope_workspaces(workspace_arg.as_deref(), None)
            .map_err(|(_, msg)| invalid_params(msg))?;
        let briefing = self
            .catchup_over(&workspaces, since_hours, 15, None)
            .map_err(|err| internal_error(err.message))?;
        let context = format_catchup_human(&briefing, workspaces.len() > 1);
        let bounded = bound_prompt_context(&context, MAX_PROMPT_CONTEXT_CHARS);
        let text = format!(
            "Prepare a concise executive Slack catch-up briefing from the local archive snapshot below.
             Group your response into:
             1. Urgent mentions & items needing my reply
             2. Key channel highlights
             3. Active threads worth following up on
             4. Notable takeaways from new audio/video or huddle transcripts

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
    ) -> Result<GetPromptResult, ErrorData> {
        let channel_arg = prompt_req_str(args, "channel")?;
        let thread_ts_arg = prompt_req_str(args, "thread_ts")?;
        let workspace_arg = prompt_opt_workspace(args)?;

        let workspace = self
            .resolve_workspace(workspace_arg.as_deref())
            .map_err(invalid_params)?
            .ok_or_else(|| {
                invalid_params("no workspace resolved; pass `workspace` in prompt arguments")
            })?;

        let archive = self.open_archive(&workspace).map_err(internal_error)?;
        let read = local_thread(
            &workspace,
            archive.as_ref(),
            &channel_arg,
            &Ts::from(thread_ts_arg.as_str()),
        )
        .map_err(|err| internal_error(err.message))?;
        let channel_id = read.channel;
        let label = read.label.unwrap_or_else(|| channel_id.as_str().to_owned());
        let user_names = read.user_names;
        let mut messages = read.messages;
        messages.retain(|m| !m.deleted);
        messages.truncate(100);

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
    ) -> Result<GetPromptResult, ErrorData> {
        let channel_arg = prompt_req_str(args, "channel")?;
        let workspace_arg = prompt_opt_workspace(args)?;
        let since = prompt_opt_str(args, "since")?;
        let cutoff_secs = match since.as_deref() {
            Some(spec) => Some(parse_since_cutoff(spec, now_epoch_secs()).map_err(invalid_params)?),
            None => None,
        };
        let limit = prompt_opt_usize(args, "limit", DEFAULT_READ_LIMIT, 1, 100)?;

        let workspace = self
            .resolve_workspace(workspace_arg.as_deref())
            .map_err(invalid_params)?
            .ok_or_else(|| {
                invalid_params("no workspace resolved; pass `workspace` in prompt arguments")
            })?;

        let archive = self.open_archive(&workspace).map_err(internal_error)?;
        let read = local_channel(
            &workspace,
            archive.as_ref(),
            &channel_arg,
            &ChannelWindow {
                since_secs: cutoff_secs,
                limit,
                include_deleted: false,
                ..ChannelWindow::default()
            },
        )
        .map_err(|err| internal_error(err.message))?;
        let channel_id = read.channel;
        let label = read.label.unwrap_or_else(|| channel_id.as_str().to_owned());
        let user_names = read.user_names;
        let messages = read.messages;

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
    ) -> Result<GetPromptResult, ErrorData> {
        let channel_arg = prompt_opt_str(args, "channel")?;
        let query_arg = prompt_opt_str(args, "query")?;
        let since_arg = prompt_opt_str(args, "since")?;
        let workspace_arg = prompt_opt_workspace(args)?;

        let cutoff_secs = match since_arg.as_deref() {
            Some(spec) => Some(parse_since_cutoff(spec, now_epoch_secs()).map_err(invalid_params)?),
            None => None,
        };

        let mut context = String::from("## Candidate Messages for Decision Extraction\n");
        let mut collected = 0usize;

        if let Some(ch_arg) = channel_arg.as_deref()
            && query_arg.is_none()
        {
            let workspace = self
                .resolve_workspace(workspace_arg.as_deref())
                .map_err(invalid_params)?
                .ok_or_else(|| {
                    invalid_params("no workspace resolved; pass `workspace` in prompt arguments")
                })?;
            if let Some(archive) = self.open_archive(&workspace).map_err(internal_error)? {
                let read = local_channel(
                    &workspace,
                    Some(&archive),
                    ch_arg,
                    &ChannelWindow {
                        since_secs: cutoff_secs,
                        limit: 80,
                        include_deleted: false,
                        ..ChannelWindow::default()
                    },
                )
                .map_err(|err| internal_error(err.message))?;
                let ch_id = read.channel;
                let label = read.label.unwrap_or_else(|| ch_id.as_str().to_owned());
                let msgs = read.messages;
                let users = read.user_names;
                context.push_str(&format!(
                    "Scope: workspace `{}` · channel {} (`{}`)

",
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
                .map_err(|(_, msg)| invalid_params(msg))?;
            if let Some(ch_arg) = channel_arg.as_deref() {
                context.push_str(&format!(
                    "Query: `{effective_query}` · Channel: `{ch_arg}`\n\n"
                ));
            } else {
                context.push_str(&format!("Query: `{effective_query}`\n\n"));
            }
            for ws in &workspaces {
                let Some(archive) = self.open_archive(ws).map_err(internal_error)? else {
                    continue;
                };
                let mut ws_query = base_parsed.clone();
                if let Some(ch_arg) = channel_arg.as_deref() {
                    let resolved = archive
                        .resolve_channel(ws, ch_arg)
                        .map_err(|err| internal_error(format!("resolving channel: {err}")))?;
                    let ch_id = resolved
                        .as_ref()
                        .map_or_else(|| raw_channel_id(ch_arg), |r| r.id.clone());
                    ws_query.filters.in_channels.push(ch_id.as_str().to_owned());
                }
                let hits = archive
                    .store()
                    .search_sorted(ws, &ws_query, SearchSort::Recent, 40)
                    .map_err(|err| internal_error(format!("searching {ws}: {err}")))?;
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
    ) -> Result<GetPromptResult, ErrorData> {
        let query = prompt_req_str(args, "query")?;
        let workspace_arg = prompt_opt_workspace(args)?;

        let (_, workspaces) = self
            .resolve_scope_workspaces(workspace_arg.as_deref(), None)
            .map_err(|(_, msg)| invalid_params(msg))?;

        let parsed = SearchQuery::parse(&query);
        let mut context = format!("## Transcript Matches for `{query}`\n\n");
        let mut file_hits = 0usize;
        let mut huddle_hits = 0usize;

        context.push_str("### Audio/Video File Transcripts\n");
        for ws in &workspaces {
            let Some(archive) = self.open_archive(ws).map_err(internal_error)? else {
                continue;
            };
            let matches = archive
                .store()
                .search_file_transcripts(Some(ws), &parsed, 15)
                .map_err(|err| internal_error(format!("searching file transcripts: {err}")))?;
            for entry in matches {
                let dur = transcript_duration_secs(&entry.transcript.cues)
                    .map(|d| format!(" · {d:.0}s"))
                    .unwrap_or_default();
                let ch_part = entry
                    .channel
                    .as_ref()
                    .map(|c| format!(" · channel `{}`", c.as_str()))
                    .unwrap_or_default();
                let body = truncate_chars(&entry.transcript.text, 800);
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
            let Some(archive) = self.open_archive(ws).map_err(internal_error)? else {
                continue;
            };
            let users = archive.user_display_map(ws).unwrap_or_default();
            let ch_names = archive.channel_names(ws).unwrap_or_default();
            let segments = archive
                .store()
                .search_huddle_transcripts(ws, &query, None, None, 25)
                .map_err(|err| internal_error(format!("searching huddle transcripts: {err}")))?;
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
// Tool results, argument helpers, URI parsing
// -----------------------------------------------------------------------------

fn invalid_params(message: impl Into<String>) -> ErrorData {
    ErrorData::invalid_params(message.into(), None)
}

fn internal_error(message: impl Into<String>) -> ErrorData {
    ErrorData::internal_error(message.into(), None)
}

fn resource_not_found(message: impl Into<String>) -> ErrorData {
    ErrorData::resource_not_found(message.into(), None)
}

fn invalid_args(message: impl Into<String>) -> ToolError {
    tool_err("invalid_args", message)
}

impl From<CmdError> for ToolError {
    fn from(err: CmdError) -> Self {
        tool_err(err.code, err.message)
    }
}

/// `read_channel`'s document for a live or archived channel read.
fn channel_json(read: &ChannelRead, workspace: &TeamId, live: bool) -> Value {
    let (mut json, _) = channel_payload(read);
    json["workspace"] = json!(workspace.as_str());
    json["live"] = json!(live);
    json
}

/// `read_thread`'s (and the thread resource's) document; the resource names no workspace.
fn thread_json(read: &ThreadRead, thread_ts: &Ts, live: bool, workspace: Option<&TeamId>) -> Value {
    let (mut json, _) = thread_payload(read, thread_ts, live);
    if let Some(workspace) = workspace {
        json["workspace"] = json!(workspace.as_str());
    }
    json
}

/// `export_conversation`'s document: the rendered artifact and what it was rendered from.
fn export_json(
    workspace: &TeamId,
    req: &ExportRequest<'_>,
    artifact: slackless_core::export::ExportArtifact,
    channel: Option<ChannelId>,
    format: ExportKind,
) -> Value {
    let mut json = json!({
        "workspace": workspace.as_str(),
        "target": match req {
            ExportRequest::FileTranscript { .. } => "file_transcript",
            ExportRequest::HuddleTranscript { .. } => "huddle_transcript",
            ExportRequest::Channel { thread_ts: Some(_), .. } => "thread",
            ExportRequest::Channel { .. } => "channel",
        },
        "format": format,
        "title": artifact.title,
        "suggested_filename": artifact.suggested_filename,
        "item_count": artifact.item_count,
        "content": artifact.content,
    });
    match req {
        ExportRequest::FileTranscript { file_id } => json["file_id"] = json!(file_id.as_str()),
        ExportRequest::HuddleTranscript { call_id } => json["huddle_id"] = json!(call_id),
        ExportRequest::Channel { thread_ts, .. } => {
            if let Some(channel) = channel {
                json["channel"] = json!(channel.as_str());
            }
            if let Some(ts) = thread_ts {
                json["thread_ts"] = json!(ts.as_str());
            }
        }
    }
    json
}

/// A successful tool result: the payload as compact JSON in a single text block.
fn tool_ok(payload: &impl Serialize) -> CallToolResult {
    let text = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_owned());
    CallToolResult::success(vec![ContentBlock::text(text)])
}

/// A failed tool run (`isError: true`) carrying `{"error": <code>, "message": ...}` for the caller.
fn tool_err(code: &str, message: impl Into<String>) -> ToolError {
    let text = serde_json::to_string(&json!({
        "error": code,
        "message": message.into(),
    }))
    .unwrap_or_else(|_| format!("{{\"error\":\"{code}\"}}"));
    ToolError(Box::new(CallToolResult::error(vec![ContentBlock::text(
        text,
    )])))
}

fn check_secrets_mcp(text: &str, allow_secrets: bool) -> Option<ToolError> {
    if allow_secrets {
        return None;
    }
    detect_secret_hits(text).map(|summary| {
        tool_err(
            "secret_detected",
            format!(
                "refused to send possible secret(s): {summary}; pass `allow_secrets: true` to override"
            ),
        )
    })
}

fn prompt_req_str(args: &Map<String, Value>, key: &str) -> Result<String, ErrorData> {
    match args.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        Some(Value::String(_)) => Err(invalid_params(format!(
            "`{key}` must be a non-empty string"
        ))),
        Some(_) => Err(invalid_params(format!("`{key}` must be a string"))),
        None => Err(invalid_params(format!("missing required argument `{key}`"))),
    }
}

fn prompt_opt_str(args: &Map<String, Value>, key: &str) -> Result<Option<String>, ErrorData> {
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
        Some(_) => Err(invalid_params(format!("`{key}` must be a string"))),
    }
}

fn prompt_opt_workspace(args: &Map<String, Value>) -> Result<Option<String>, ErrorData> {
    match args.get("workspace") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => validate_workspace_id(s).map(Some).map_err(invalid_params),
        Some(_) => Err(invalid_params("`workspace` must be a string")),
    }
}

fn prompt_opt_usize(
    args: &Map<String, Value>,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, ErrorData> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Number(n)) => {
            let Some(val) = n.as_u64() else {
                return Err(invalid_params(format!(
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
            let val: u64 = trimmed
                .parse()
                .map_err(|_| invalid_params(format!("`{key}` must be a positive integer")))?;
            Ok(usize::try_from(val).unwrap_or(max).clamp(min, max))
        }
        Some(_) => Err(invalid_params(format!(
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
        truncate_chars(&msg.text, 600)
    } else {
        truncate_chars(&plain, 600)
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

fn prompt_result(description: &str, text: String) -> GetPromptResult {
    GetPromptResult::new(vec![PromptMessage::new_text(Role::User, text)])
        .with_description(description)
}

fn percent_decode_segment(raw: &str) -> Result<String, String> {
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

fn parse_resource_uri(uri: &str) -> Result<ResourceUri, ErrorData> {
    if uri.bytes().any(|b| b < 0x20 || b == 0x7f) || uri.contains('\\') {
        return Err(invalid_params(format!("invalid resource URI '{uri}'")));
    }
    let Some(rest) = uri.strip_prefix("slackless://") else {
        return Err(invalid_params(format!(
            "unsupported URI scheme in '{uri}': expected slackless://"
        )));
    };
    if rest.contains('?') || rest.contains('#') {
        return Err(invalid_params(format!(
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
        return Err(invalid_params(format!(
            "invalid path segment in resource URI '{uri}'"
        )));
    }

    match segments.as_slice() {
        ["workspace", ws_raw, "channels"] => {
            let ws = validate_workspace_id(ws_raw).map_err(invalid_params)?;
            Ok(ResourceUri::WorkspaceChannels {
                workspace: TeamId::from(ws),
            })
        }
        ["workspace", ws_raw, "channel", ch_raw] => {
            let ws = validate_workspace_id(ws_raw).map_err(invalid_params)?;
            let channel = percent_decode_segment(ch_raw).map_err(invalid_params)?;
            Ok(ResourceUri::Channel {
                workspace: TeamId::from(ws),
                channel,
            })
        }
        ["workspace", ws_raw, "thread", ch_raw, ts_raw] => {
            let ws = validate_workspace_id(ws_raw).map_err(invalid_params)?;
            let channel = percent_decode_segment(ch_raw).map_err(invalid_params)?;
            let ts = percent_decode_segment(ts_raw).map_err(invalid_params)?;
            Ok(ResourceUri::Thread {
                workspace: TeamId::from(ws),
                channel,
                thread_ts: Ts::from(ts),
            })
        }
        ["workspace", ws_raw, "transcript", file_raw] => {
            let ws = validate_workspace_id(ws_raw).map_err(invalid_params)?;
            let file_id = percent_decode_segment(file_raw).map_err(invalid_params)?;
            Ok(ResourceUri::Transcript {
                workspace: TeamId::from(ws),
                file_id: FileId::from(file_id),
            })
        }
        _ => Err(invalid_params(format!(
            "unknown or malformed resource URI '{uri}'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slackless_core::store::{NewHuddleTranscriptSegment, Store};
    use slackless_core::types::{
        Conversation, File, Latest, Message, Profile, TopicPurpose, User, UserId,
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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

    fn user(id: &str, name: &str, display: &str) -> User {
        User {
            id: UserId::from(id),
            name: name.into(),
            profile: Profile {
                display_name: Some(display.into()),
                ..Profile::default()
            },
            ..User::default()
        }
    }

    fn conv(id: &str, name: &str) -> Conversation {
        Conversation {
            id: ChannelId::from(id),
            name: Some(name.into()),
            is_channel: true,
            is_member: true,
            ..Conversation::default()
        }
    }

    fn msg(ts: impl Into<String>, user_id: Option<&str>, text: &str) -> Message {
        Message {
            ts: Ts::from(ts.into()),
            user: user_id.map(UserId::from),
            text: text.into(),
            ..Message::default()
        }
    }

    fn huddle_segment(
        store: &mut Store,
        t1: &TeamId,
        call_id: &str,
        user_id: &str,
        lang: &str,
        text: &str,
        at_ms: i64,
    ) {
        store
            .insert_huddle_transcript_segment(
                t1,
                &NewHuddleTranscriptSegment {
                    channel: &ChannelId::from("C_ENG"),
                    call_id,
                    thread_root_ts: None,
                    speaker_id: Some(&UserId::from(user_id)),
                    lang: Some(lang),
                    source: "chime",
                    text,
                    at_ms,
                },
            )
            .expect("insert huddle segment");
    }

    /// Seeds the archive fixture every protocol row runs against. Workspace T1: users Ana and Bob,
    /// channels #eng (a thread, a deleted message, a recent decision), archived #legacy, DM with
    /// Ana (a recent decision), and a huge #huge; a clipped video transcript with timed cues; two
    /// huddle transcripts. Workspace T2: one message and one file transcript.
    fn seed_stores(dir: &TempDir) {
        let now = now_epoch_secs();
        let t1 = TeamId::from("T1");
        let mut s1 = Store::open(dir.path().join("T1.sqlite")).expect("open T1 store");
        s1.upsert_users(&t1, &[user("U1", "ana", "Ana"), user("U2", "bob", "Bob")])
            .expect("upsert T1 users");
        s1.upsert_conversations(
            &t1,
            &[
                Conversation {
                    topic: Some(TopicPurpose {
                        value: "Deployments and releases".into(),
                        ..TopicPurpose::default()
                    }),
                    ..conv("C_ENG", "eng")
                },
                Conversation {
                    is_archived: true,
                    is_member: false,
                    ..conv("C_OLD", "legacy")
                },
                Conversation {
                    is_channel: false,
                    is_member: false,
                    is_im: true,
                    user: Some(UserId::from("U1")),
                    latest: Some(Latest::Ts(Ts::from("1700000400.000000"))),
                    ..conv("D_ANA", "")
                },
                conv("C_HUGE", "huge"),
            ],
        )
        .expect("upsert T1 conversations");

        let clip = Message {
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
        let reply = Message {
            ts: Ts::from("1700000250.000200"),
            thread_ts: Some(Ts::from("1700000200.000100")),
            user: Some(UserId::from("U2")),
            text: "Looks solid, shipping today".into(),
            ..Message::default()
        };
        let deleted_ts = Ts::from("1700000300.000000");
        s1.upsert_messages(
            &t1,
            &ChannelId::from("C_ENG"),
            &[
                msg("1700000100.000000", Some("U2"), "Earlier status check"),
                clip,
                reply,
                msg(deleted_ts.as_str(), Some("U1"), "Temporary scratch note"),
                msg(
                    format!("{}.000000", now - 120),
                    Some("U1"),
                    "We decided to ship the eu-north rollout on Friday.",
                ),
            ],
        )
        .expect("upsert #eng messages");
        s1.record_deletion(
            &t1,
            &ChannelId::from("C_ENG"),
            &deleted_ts,
            Some(&Ts::from("1700000301.000000")),
            None,
        )
        .expect("delete message");
        s1.upsert_message(
            &t1,
            &ChannelId::from("D_ANA"),
            &msg(
                format!("{}.000000", now - 90),
                Some("U1"),
                "DM decision: approved the staging cutover with Ana.",
            ),
        )
        .expect("upsert DM message");
        let big_body = "x".repeat(550);
        let huge: Vec<Message> = (0..80)
            .map(|i| {
                msg(
                    format!("{}.000000", 1_700_001_000 + i),
                    Some("U1"),
                    &format!("entry-{i:02} {big_body}"),
                )
            })
            .collect();
        s1.upsert_messages(&t1, &ChannelId::from("C_HUGE"), &huge)
            .expect("upsert #huge messages");
        s1.upsert_file_transcript_with_cues(
            &t1,
            &FileId::from("F_CLIP1"),
            "parakeet-v3",
            "Zero downtime canary migration succeeded across eu-north.",
            &[
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
            ],
        )
        .expect("upsert timed file transcript");
        huddle_segment(
            &mut s1,
            &t1,
            "R_HUDDLE1",
            "U1",
            "en-US",
            "Let us finalize the rollout checklist.",
            1_700_000_600_000,
        );
        huddle_segment(
            &mut s1,
            &t1,
            "R_HUDDLE1",
            "U2",
            "sv-SE",
            "Vi skeppar i eftermiddag.",
            1_700_000_645_500,
        );
        huddle_segment(
            &mut s1,
            &t1,
            "R_PROMPT1",
            "U2",
            "en",
            "The eu-north failover drill passed in 14 seconds.",
            (now - 60) * 1_000,
        );
        drop(s1);

        let t2 = TeamId::from("T2");
        let mut s2 = Store::open(dir.path().join("T2.sqlite")).expect("open T2 store");
        s2.upsert_conversations(&t2, &[conv("C_OPS", "ops")])
            .expect("upsert T2 conversations");
        s2.upsert_message(
            &t2,
            &ChannelId::from("C_OPS"),
            &msg(
                "1700000500.000000",
                None,
                "Canary rollout finished in workspace two",
            ),
        )
        .expect("upsert T2 message");
        s2.upsert_file_transcript(
            &t2,
            &FileId::from("F_T2"),
            "whisper-local",
            "Secondary workspace ops walkthrough.",
        )
        .expect("upsert T2 transcript");
    }

    fn session() -> Session {
        Session::new(
            "https://acme.slack.com/",
            "T1",
            "xoxc-test",
            "xoxd-test",
            "U1",
        )
    }

    /// What the server must answer for one request line.
    #[derive(Clone)]
    enum Expect {
        /// A notification: no response at all.
        Silent,
        /// A JSON-RPC error with this code.
        RpcError(i64),
        /// `isError: true` with this `error` code in the tool payload.
        ToolError(&'static str),
        /// `isError: false`; each `(path, expected)` pair matches the parsed tool payload.
        Tool(Vec<(&'static str, Value)>),
        /// Each `(path, expected)` pair matches the raw `result`.
        Result(Vec<(&'static str, Value)>),
        /// Like [`Expect::Result`], but against the JSON document inside `contents[0].text`.
        Resource(Vec<(&'static str, Value)>),
    }

    fn request(id: u64, method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    }

    fn call(id: u64, name: &str, arguments: Value) -> Value {
        request(
            id,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        )
    }

    fn tool(
        id: u64,
        name: &str,
        arguments: Value,
        checks: Vec<(&'static str, Value)>,
    ) -> (Value, Expect) {
        (call(id, name, arguments), Expect::Tool(checks))
    }

    fn tool_err(id: u64, name: &str, arguments: Value, code: &'static str) -> (Value, Expect) {
        (call(id, name, arguments), Expect::ToolError(code))
    }

    fn rpc_err(id: u64, method: &str, params: Value) -> (Value, Expect) {
        (request(id, method, params), Expect::RpcError(-32602))
    }

    /// The handshake every table starts with; the server echoes the requested version.
    fn initialize(id: u64) -> (Value, Expect) {
        let req = request(
            id,
            "initialize",
            json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "test-client", "version": "1.0"}}),
        );
        (
            req,
            Expect::Result(vec![("protocolVersion", json!("2025-03-26"))]),
        )
    }

    /// Resolves a dotted path into `value`: `[N]` indexes an array and `[*]` maps the remaining
    /// path over every element (yielding an array).
    fn lookup(value: &Value, path: &str) -> Option<Value> {
        let mut current = vec![value];
        let mut expanded = false;
        for seg in path.split('.') {
            let (field, selector) = match seg.split_once('[') {
                Some((field, rest)) => (field, Some(rest.trim_end_matches(']'))),
                None => (seg, None),
            };
            let mut next = Vec::new();
            for item in current {
                let item = if field.is_empty() {
                    item
                } else {
                    item.get(field)?
                };
                match selector {
                    Some("*") => next.extend(item.as_array()?),
                    Some(idx) => next.push(item.get(idx.parse::<usize>().ok()?)?),
                    None => next.push(item),
                }
            }
            current = next;
            expanded |= selector == Some("*");
        }
        if expanded {
            Some(Value::Array(
                current.into_iter().map(|v| (*v).clone()).collect(),
            ))
        } else {
            current.pop().map(|v| (*v).clone())
        }
    }

    fn check(label: &str, value: &Value, checks: &[(&str, Value)]) {
        for (path, want) in checks {
            let found = lookup(value, path.trim_end_matches('~'));
            if path.ends_with('~') {
                let hit = found
                    .as_ref()
                    .and_then(Value::as_str)
                    .is_some_and(|s| want.as_str().is_some_and(|w| s.contains(w)));
                assert!(hit, "{label}: {path} should contain {want}, got {found:?}");
            } else {
                assert_eq!(found.as_ref(), Some(want), "{label}: {path}");
            }
        }
    }

    fn tool_payload(response: &Value, label: &str) -> (bool, Value) {
        let is_error = response["result"]["isError"].as_bool().unwrap_or(false);
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{label}: no tool text in {response}"));
        let payload = serde_json::from_str(text)
            .unwrap_or_else(|_| panic!("{label}: bad tool payload {text}"));
        (is_error, payload)
    }

    /// Drives `server` over an in-memory stdio transport: writes every request line, collects the
    /// JSON-RPC responses by id, and checks each row against its [`Expect`].
    async fn run_table(server: McpServer, rows: &[(Value, Expect)]) {
        let (io, server_io) = tokio::io::duplex(256 * 1024);
        let (reader, mut writer) = tokio::io::split(io);
        let requests: Vec<String> = rows.iter().map(|(req, _)| format!("{req}\n")).collect();
        let written = tokio::spawn(async move {
            for line in &requests {
                writer.write_all(line.as_bytes()).await?;
            }
            writer.shutdown().await
        });
        let running = server.serve(server_io).await.expect("server initializes");
        let mut responses: HashMap<String, Value> = HashMap::new();
        let mut lines = BufReader::new(reader).lines();
        while let Some(line) = lines.next_line().await.expect("read response") {
            let response: Value = serde_json::from_str(&line).expect("response is JSON");
            let id = response
                .get("id")
                .cloned()
                .unwrap_or(Value::Null)
                .to_string();
            responses.insert(id, response);
        }
        written.await.expect("writer task").expect("write requests");
        running
            .waiting()
            .await
            .expect("server task finishes at EOF");

        let id_of = |req: &Value| req["id"].to_string();
        let expected: HashSet<String> = rows
            .iter()
            .filter(|(_, expect)| !matches!(expect, Expect::Silent))
            .map(|(req, _)| id_of(req))
            .collect();
        let got: HashSet<String> = responses.keys().cloned().collect();
        assert_eq!(got, expected, "response ids");

        for (idx, (req, expect)) in rows.iter().enumerate() {
            let label = format!("row {idx}: {req}");
            let response = responses.get(&id_of(req));
            match expect {
                Expect::Silent => {}
                Expect::RpcError(code) => {
                    let response = response.expect("response");
                    assert_eq!(response["error"]["code"], *code, "{label}: {response}");
                    assert!(response.get("result").is_none(), "{label}: {response}");
                }
                Expect::ToolError(code) => {
                    let (is_error, payload) = tool_payload(response.expect("response"), &label);
                    assert!(is_error, "{label}: {payload}");
                    assert_eq!(payload["error"], *code, "{label}: {payload}");
                }
                Expect::Tool(checks) => {
                    let (is_error, payload) = tool_payload(response.expect("response"), &label);
                    assert!(!is_error, "{label}: {payload}");
                    check(&label, &payload, checks);
                }
                Expect::Result(checks) => {
                    let response = response.expect("response");
                    assert!(response.get("error").is_none(), "{label}: {response}");
                    check(&label, &response["result"], checks);
                }
                Expect::Resource(checks) => {
                    let response = response.expect("response");
                    assert!(response.get("error").is_none(), "{label}: {response}");
                    let body: Value = serde_json::from_str(
                        response["result"]["contents"][0]["text"]
                            .as_str()
                            .unwrap_or_else(|| panic!("{label}: {response}")),
                    )
                    .expect("resource body is JSON");
                    check(&label, &body, checks);
                }
            }
        }
    }

    /// Resource URIs that must be refused with `-32602` before any archive path is built.
    const BAD_URIS: &[&str] = &[
        "slackless://workspace/../secret_outside/transcript/F_SECRET",
        "slackless://workspace/..%2Fsecret_outside/transcript/F_SECRET",
        "slackless://workspace/%2e%2e/channels",
        "slackless://workspace/secret_outside.sqlite/channels",
        r"slackless://workspace/T1\..\secret_outside/channels",
        "slackless://workspace/T1/channel/foo%2Fbar",
        "slackless://workspace/T1/channel/%ZZ",
        "slackless://workspace/T1/channel/",
        "slackless://workspace/T1/channels?foo=bar",
        "slackless://workspace/T1/channels#frag",
        "file:///etc/passwd",
        "slackless://unknown/path",
    ];

    /// `tools/list` under `--read-only`, in the SDK's alphabetical order.
    const READ_ONLY_TOOLS: [&str; 9] = [
        "export_conversation",
        "get_catchup_summary",
        "get_transcript",
        "list_channels",
        "list_transcripts",
        "list_workspaces",
        "read_channel",
        "read_thread",
        "search_messages",
    ];

    /// One end-to-end pass over the wire protocol: version negotiation, tool listing and every
    /// tool against the fixture archive, resources, prompts, and the error paths. Three servers:
    /// signed in to T1, no session, and `--read-only`.
    #[tokio::test]
    async fn mcp_protocol_over_stdio() {
        let dir = TempDir::new("protocol");
        seed_stores(&dir);
        let path = dir.path().to_path_buf();
        let fake_secret = format!("xoxb-{}-{}", "123456789012", "abcdefghijklmnopqrstuvwx");

        let tools_list = (
            json!({"jsonrpc":"2.0","id":7,"method":"tools/list"}),
            Expect::Result(vec![
                (
                    "tools[*].name",
                    json!([
                        "add_reaction",
                        "export_conversation",
                        "get_catchup_summary",
                        "get_transcript",
                        "list_channels",
                        "list_transcripts",
                        "list_workspaces",
                        "read_channel",
                        "read_thread",
                        "reply_thread",
                        "search_messages",
                        "send_message"
                    ]),
                ),
                ("tools[*].inputSchema.type", json!(vec!["object"; 12])),
                ("tools[7].inputSchema.required", json!(["channel"])),
                ("tools[7].inputSchema.properties.limit.maximum", json!(200)),
                (
                    "tools[4].inputSchema.properties.kind.enum",
                    json!(["all", "channels", "dms", null]),
                ),
                (
                    "tools[7].annotations",
                    json!({"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true}),
                ),
                ("tools[11].annotations.readOnlyHint", json!(false)),
                (
                    "tools[1].inputSchema.properties.limit.description~",
                    json!("default: 500"),
                ),
            ]),
        );

        run_table(
            McpServer::for_test(path.clone(), vec![session()], None, false),
            &[
                (json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test-client","version":"1.0"}}}), Expect::Result(vec![
                    ("protocolVersion", json!("2024-11-05")),
                    ("serverInfo.name", json!("slackless")),
                    ("serverInfo.version", json!(CLI_VERSION)),
                    ("capabilities.tools", json!({})),
                    ("capabilities.resources", json!({})),
                    ("capabilities.prompts", json!({})),
                    ("instructions~", json!("FTS5 search")),
                ])),
                (json!({"jsonrpc":"2.0","method":"notifications/initialized"}), Expect::Silent),
                (json!({"jsonrpc":"2.0","id":2,"method":"ping"}), Expect::Result(vec![])),
                (json!({"jsonrpc":"2.0","id":3,"method":"initialize","params":{"protocolVersion":"2099-01-01","capabilities":{},"clientInfo":{"name":"test-client","version":"1.0"}}}), Expect::Result(vec![("protocolVersion", json!("2025-03-26"))])),
                (json!({"jsonrpc":"2.0","id":4,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-client","version":"1.0"}}}), Expect::Result(vec![("protocolVersion", json!("2025-06-18"))])),
                (json!({"jsonrpc":"2.0","id":5,"method":"unknown/method"}), Expect::RpcError(-32601)),
                tools_list.clone(),
                tool(8, "list_workspaces", json!({}), vec![
                    ("default_workspace", json!("T1")),
                    ("count", json!(2)),
                    ("workspaces[*].team_id", json!(["T1", "T2"])),
                    ("workspaces[*].signed_in", json!([true, false])),
                ]),
                tool(9, "list_channels", json!({"workspace":"T1","include_archived":false}), vec![
                    ("count", json!(3)),
                    ("channels[*].id", json!(["C_ENG", "C_HUGE", "D_ANA"])),
                    ("channels[2].label", json!("@Ana")),
                    ("kind", json!("all")),
                ]),
                tool(10, "list_channels", json!({"workspace":"T1","kind":"dms","filter":"@ANA"}), vec![
                    ("count", json!(1)),
                    ("channels[0].id", json!("D_ANA")),
                ]),
                // Case-insensitive enum, out-of-range limit clamped, archived included by default.
                tool(11, "list_channels", json!({"kind":"ALL","limit":5000}), vec![
                    ("count", json!(4)),
                    ("total_matched", json!(4)),
                ]),
                tool_err(12, "list_channels", json!({"kind":"bogus"}), "invalid_args"),
                tool_err(13, "list_channels", json!({"limit":"x"}), "invalid_args"),
                tool(14, "search_messages", json!({"query":"canary","all_workspaces":true}), vec![
                    ("all_workspaces", json!(true)),
                    ("sort", json!("recent")),
                    ("count", json!(2)),
                    ("messages[*].workspace", json!(["T2", "T1"])),
                    ("messages[0].channel", json!("C_OPS")),
                    ("messages[1].file_refs[0].id", json!("F_CLIP1")),
                    ("workspaces_searched", json!(["T1", "T2"])),
                ]),
                tool(15, "search_messages", json!({"workspace":"T1","query":"eu-north in:#eng from:@ana has:file"}), vec![
                    ("count", json!(1)),
                    ("messages[0].ts", json!("1700000200.000100")),
                ]),
                tool_err(16, "search_messages", json!({"query":"x","workspace":"T1","all_workspaces":true}), "invalid_args"),
                tool_err(17, "search_messages", json!({}), "invalid_args"),
                tool_err(18, "search_messages", json!({"query":"x","sort":"bogus"}), "invalid_args"),
                tool(19, "read_channel", json!({"channel":"#eng","before":"1700000250.000000","limit":10}), vec![
                    ("live", json!(false)),
                    ("channel.id", json!("C_ENG")),
                    ("count", json!(2)),
                    ("messages[*].ts", json!(["1700000100.000000", "1700000200.000100"])),
                    ("messages[1].user_name", json!("Ana")),
                ]),
                tool(20, "read_channel", json!({"workspace":"T1","channel":"#eng","include_deleted":true}), vec![
                    ("count", json!(4)),
                    ("messages[2].deleted", json!(true)),
                ]),
                tool(21, "read_channel", json!({"workspace":"T1","channel":"#eng","since":"1700000150"}), vec![
                    ("count", json!(2)),
                    ("messages[0].ts", json!("1700000200.000100")),
                ]),
                tool_err(22, "read_channel", json!({"channel":"#eng","since":"not-a-date"}), "invalid_args"),
                tool(23, "read_thread", json!({"channel":"#eng","thread_ts":"1700000200.000100"}), vec![
                    ("live", json!(false)),
                    ("channel", json!("C_ENG")),
                    ("thread_ts", json!("1700000200.000100")),
                    ("count", json!(2)),
                    ("messages[0].ts", json!("1700000200.000100")),
                    ("messages[1].user_name", json!("Bob")),
                ]),
                tool_err(24, "read_thread", json!({"channel":"#eng"}), "invalid_args"),
                tool(25, "get_transcript", json!({"file_id":"F_CLIP1"}), vec![
                    ("workspace", json!("T1")),
                    ("file_id", json!("F_CLIP1")),
                    ("source", json!("parakeet-v3")),
                    ("cue_count", json!(2)),
                    ("duration_secs", json!(12.25)),
                    ("text~", json!("eu-north")),
                ]),
                tool_err(26, "get_transcript", json!({"file_id":"F_MISSING"}), "transcript_not_found"),
                tool(27, "list_transcripts", json!({"all_workspaces":true}), vec![
                    ("all_workspaces", json!(true)),
                    ("count", json!(4)),
                    ("file_count", json!(2)),
                    ("huddle_count", json!(2)),
                ]),
                tool(28, "list_transcripts", json!({"workspace":"T1","kind":"huddle","channel":"#eng"}), vec![
                    ("count", json!(2)),
                    ("transcripts[*].id", json!(["R_PROMPT1", "R_HUDDLE1"])),
                    ("transcripts[1].cue_count", json!(2)),
                    ("transcripts[1].duration_secs", json!(45.5)),
                    ("transcripts[1].channel_name", json!("#eng")),
                    ("transcripts[1].languages", json!(["en", "sv"])),
                ]),
                tool(29, "export_conversation", json!({"channel":"#eng","format":"md","include_threads":true,"limit":10}), vec![
                    ("target", json!("channel")),
                    ("format", json!("md")),
                    ("item_count", json!(3)),
                    ("content~", json!("Thread replies (1)")),
                    ("content~", json!("Looks solid, shipping today")),
                    ("content~", json!("eu-north")),
                ]),
                tool(30, "export_conversation", json!({"channel":"C_ENG","format":"json","include_deleted":true}), vec![
                    ("format", json!("json")),
                    ("item_count", json!(4)),
                ]),
                tool(31, "export_conversation", json!({"channel":"#eng","thread_ts":"1700000200.000100"}), vec![
                    ("target", json!("thread")),
                    ("thread_ts", json!("1700000200.000100")),
                    ("format", json!("md")),
                    ("item_count", json!(2)),
                ]),
                tool(32, "export_conversation", json!({"file_id":"F_T2","format":"json"}), vec![
                    ("workspace", json!("T2")),
                    ("target", json!("file_transcript")),
                    ("content~", json!("walkthrough")),
                ]),
                tool(33, "export_conversation", json!({"huddle_id":"R_HUDDLE1","format":"MARKDOWN"}), vec![
                    ("target", json!("huddle_transcript")),
                    ("format", json!("md")),
                    ("item_count", json!(2)),
                    ("content~", json!("Vi skeppar i eftermiddag.")),
                ]),
                tool_err(34, "export_conversation", json!({"thread_ts":"1"}), "invalid_args"),
                tool_err(35, "export_conversation", json!({"channel":"#eng","file_id":"F_CLIP1"}), "invalid_args"),
                tool_err(36, "export_conversation", json!({"huddle_id":"R_MISSING"}), "not_found"),
                tool(37, "get_catchup_summary", json!({"since_hours":24}), vec![
                    ("workspaces[0].team_id", json!("T1")),
                    ("window.kind", json!("custom")),
                    // Newest first; the 2023 huddle R_HUDDLE1 is outside the 24-hour window.
                    ("transcripts[*].call_id", json!([null, "R_PROMPT1"])),
                ]),
                // The secret guard runs before any session or network use.
                tool_err(38, "send_message", json!({"channel":"#eng","text":fake_secret}), "secret_detected"),
                tool_err(39, "reply_thread", json!({"channel":"#eng","thread_ts":"1700000200.000100","text":fake_secret}), "secret_detected"),
                tool_err(40, "add_reaction", json!({"channel":"#eng","ts":"1700000100.000000","emoji":"::"}), "invalid_args"),
                (json!({"jsonrpc":"2.0","id":41,"method":"resources/list"}), Expect::Result(vec![
                    ("resources[*].uri", json!(["slackless://workspaces"])),
                    ("resources[0].mimeType", json!("application/json")),
                ])),
                (json!({"jsonrpc":"2.0","id":42,"method":"resources/templates/list"}), Expect::Result(vec![
                    ("resourceTemplates[*].uriTemplate", json!([
                        "slackless://workspace/{workspace}/channels",
                        "slackless://workspace/{workspace}/channel/{channel}",
                        "slackless://workspace/{workspace}/thread/{channel}/{ts}",
                        "slackless://workspace/{workspace}/transcript/{file_id}",
                    ])),
                ])),
                (json!({"jsonrpc":"2.0","id":43,"method":"resources/read","params":{"uri":"slackless://workspaces"}}), Expect::Resource(vec![
                    ("count", json!(2)),
                    ("workspaces[0].team_id", json!("T1")),
                ])),
                (json!({"jsonrpc":"2.0","id":44,"method":"resources/read","params":{"uri":"slackless://workspace/T1/channels"}}), Expect::Resource(vec![
                    ("count", json!(4)),
                    ("channels[0].id", json!("C_ENG")),
                ])),
                // Channel resources accept a raw ID and a percent-encoded #name alike.
                (json!({"jsonrpc":"2.0","id":45,"method":"resources/read","params":{"uri":"slackless://workspace/T1/channel/C_ENG"}}), Expect::Resource(vec![
                    ("channel.id", json!("C_ENG")),
                    ("count", json!(3)),
                ])),
                (json!({"jsonrpc":"2.0","id":46,"method":"resources/read","params":{"uri":"slackless://workspace/T1/channel/%23eng"}}), Expect::Resource(vec![
                    ("channel.id", json!("C_ENG")),
                ])),
                (json!({"jsonrpc":"2.0","id":47,"method":"resources/read","params":{"uri":"slackless://workspace/T1/thread/C_ENG/1700000200.000100"}}), Expect::Resource(vec![
                    ("count", json!(2)),
                    ("messages[1].text", json!("Looks solid, shipping today")),
                ])),
                (json!({"jsonrpc":"2.0","id":48,"method":"resources/read","params":{"uri":"slackless://workspace/T1/transcript/F_CLIP1"}}), Expect::Resource(vec![
                    ("file_id", json!("F_CLIP1")),
                    ("text", json!("Zero downtime canary migration succeeded across eu-north.")),
                ])),
                (json!({"jsonrpc":"2.0","id":49,"method":"resources/read","params":{"uri":"slackless://workspace/T1/transcript/F_NOPE"}}), Expect::RpcError(-32002)),
                rpc_err(50, "resources/read", json!({})),
                rpc_err(51, "tools/call", json!({})),
                rpc_err(52, "tools/call", json!({"name":"list_workspaces","arguments":"bad"})),
                rpc_err(53, "tools/call", json!({"name":"does_not_exist","arguments":{}})),
                (json!({"jsonrpc":"2.0","id":54,"method":"prompts/list"}), Expect::Result(vec![
                    ("prompts[*].name", json!(["catch_up", "summarize_thread", "summarize_channel", "find_decisions", "search_transcripts"])),
                    ("prompts[0].arguments[*].name", json!(["workspace", "since_hours"])),
                    ("prompts[1].arguments[*].required", json!([true, true, false])),
                ])),
                (json!({"jsonrpc":"2.0","id":55,"method":"prompts/get","params":{"name":"catch_up","arguments":{"since_hours":"24"}}}), Expect::Result(vec![
                    ("messages[0].role", json!("user")),
                    ("messages[0].content.type", json!("text")),
                    ("messages[0].content.text~", json!("TRANSCRIPTS (2)")),
                    ("messages[0].content.text~", json!("failover drill passed in 14 seconds")),
                    ("description~", json!("Summarize recent Slack activity")),
                ])),
                (json!({"jsonrpc":"2.0","id":56,"method":"prompts/get","params":{"name":"summarize_thread","arguments":{"channel":"#eng","thread_ts":"1700000200.000100"}}}), Expect::Result(vec![
                    ("messages[0].content.text~", json!("Watch the rollout demo clip")),
                    ("messages[0].content.text~", json!("Looks solid, shipping today")),
                ])),
                (json!({"jsonrpc":"2.0","id":57,"method":"prompts/get","params":{"name":"summarize_channel","arguments":{"channel":"#huge","limit":"80"}}}), Expect::Result(vec![
                    ("messages[0].content.text~", json!("context truncated to stay within prompt budget")),
                ])),
                (json!({"jsonrpc":"2.0","id":58,"method":"prompts/get","params":{"name":"find_decisions","arguments":{"query":"decided","channel":"#eng"}}}), Expect::Result(vec![
                    ("messages[0].content.text~", json!("ship the eu-north rollout on Friday")),
                ])),
                (json!({"jsonrpc":"2.0","id":59,"method":"prompts/get","params":{"name":"find_decisions","arguments":{"query":"cutover","channel":"@ana"}}}), Expect::Result(vec![
                    ("messages[0].content.text~", json!("approved the staging cutover with Ana")),
                ])),
                (json!({"jsonrpc":"2.0","id":60,"method":"prompts/get","params":{"name":"search_transcripts","arguments":{"query":"eu-north"}}}), Expect::Result(vec![
                    ("messages[0].content.text~", json!("F_CLIP1")),
                    ("messages[0].content.text~", json!("R_PROMPT1")),
                    ("messages[0].content.text~", json!("failover drill passed in 14 seconds")),
                ])),
                rpc_err(61, "prompts/get", json!({"name":"unknown_prompt"})),
                rpc_err(62, "prompts/get", json!({"name":"summarize_thread","arguments":{"channel":"#eng"}})),
                rpc_err(63, "prompts/get", json!({"name":"search_transcripts","arguments":{}})),
                rpc_err(64, "prompts/get", json!({"name":"catch_up","arguments":{"workspace":"../secret"}})),
                rpc_err(65, "prompts/get", json!({"name":"catch_up","arguments":"bad"})),
            ])
        .await;

        // Malformed or traversing resource URIs are rejected before any archive is opened.
        let mut rows = vec![initialize(1)];
        rows.extend(
            BAD_URIS
                .iter()
                .enumerate()
                .map(|(i, uri)| rpc_err(i as u64 + 2, "resources/read", json!({"uri": uri}))),
        );
        rows.push((
            json!({"jsonrpc":"2.0","id":99,"method":"resources/read","params":{}}),
            Expect::RpcError(-32602),
        ));
        run_table(
            McpServer::for_test(path.clone(), vec![session()], None, false),
            &rows,
        )
        .await;

        // No stored session: live actions fail before the network, and every tool that accepts
        // `workspace` rejects traversal payloads before opening anything.
        let mut rows = vec![
            initialize(1),
            tool_err(
                2,
                "send_message",
                json!({"channel":"#eng","text":"hello"}),
                "not_signed_in",
            ),
            tool_err(
                3,
                "reply_thread",
                json!({"channel":"#eng","thread_ts":"1700000200.000100","text":"safe reply"}),
                "not_signed_in",
            ),
            tool_err(
                4,
                "read_channel",
                json!({"channel":"#eng","live":true}),
                "not_signed_in",
            ),
            tool_err(
                5,
                "read_thread",
                json!({"channel":"#eng","thread_ts":"1700000200.000100","live":true}),
                "not_signed_in",
            ),
        ];
        for (i, (name, mut arguments)) in [
            ("list_channels", json!({})),
            ("search_messages", json!({"query":"secret"})),
            ("read_channel", json!({"channel":"#eng"})),
            ("read_thread", json!({"channel":"#eng","thread_ts":"1.0"})),
            ("get_transcript", json!({"file_id":"F_SECRET"})),
            ("list_transcripts", json!({})),
            ("export_conversation", json!({"channel":"#eng"})),
            ("get_catchup_summary", json!({})),
            ("send_message", json!({"channel":"#eng","text":"hi"})),
            (
                "reply_thread",
                json!({"channel":"#eng","thread_ts":"1.0","text":"hi"}),
            ),
            (
                "add_reaction",
                json!({"channel":"#eng","ts":"1.0","emoji":"tada"}),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            arguments["workspace"] =
                json!(["../secret_outside", "/tmp/foo", "../../etc", r"T1\..\T2"][i % 4]);
            rows.push(tool_err(i as u64 + 6, name, arguments, "invalid_args"));
        }
        run_table(
            McpServer::for_test(path.clone(), Vec::new(), Some("T1".into()), false),
            &rows,
        )
        .await;

        // `--read-only` withholds the write tools from tools/list and refuses them with -32602.
        run_table(
            McpServer::for_test(path, Vec::new(), None, true),
            &[
                initialize(1),
                (json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}), Expect::Result(vec![
                    ("tools[*].name", json!(READ_ONLY_TOOLS)),
                ])),
                rpc_err(3, "tools/call", json!({"name":"send_message","arguments":{"channel":"#eng","text":"hi"}})),
                rpc_err(4, "tools/call", json!({"name":"reply_thread","arguments":{"channel":"#eng","thread_ts":"1.0","text":"hi"}})),
                rpc_err(5, "tools/call", json!({"name":"add_reaction","arguments":{"channel":"#eng","ts":"1.0","emoji":"tada"}})),
                tool(6, "list_workspaces", json!({}), vec![("default_workspace", json!(null)), ("count", json!(2))]),
                // Two archives and no default: single-workspace tools refuse, search spans all.
                tool_err(7, "list_channels", json!({}), "no_workspace"),
                tool(8, "search_messages", json!({"query":"canary","sort":"relevant"}), vec![
                    ("all_workspaces", json!(true)),
                    ("sort", json!("relevant")),
                    ("count", json!(2)),
                ]),
                tool(9, "get_transcript", json!({"file_id":"F_CLIP1"}), vec![("workspace", json!("T1"))]),
            ],
        )
        .await;
    }

    /// `get_catchup_summary` is the core catch-up briefing: only activity inside the window,
    /// with self-posts and muted chatter left out.
    #[tokio::test]
    async fn catchup_summary_windows_activity_and_excludes_old_items() {
        let dir = TempDir::new("catchup");
        let now = now_epoch_secs();
        let (recent_1, recent_2, old) = (now - 600, now - 300, now - 100 * 3_600);
        let t1 = TeamId::from("T1");
        let mut s1 = Store::open(dir.path().join("T1.sqlite")).expect("open T1 store");
        s1.upsert_users(&t1, &[user("U1", "ana", "Ana"), user("U2", "bob", "Bob")])
            .expect("users");
        s1.upsert_conversations(
            &t1,
            &[
                Conversation {
                    unread_count_display: Some(2),
                    last_read: Some(Ts::from(format!("{}.000000", now - 3_600))),
                    ..conv("C_ENG", "eng")
                },
                conv("C_NOISE", "noise"),
            ],
        )
        .expect("conversations");
        s1.replace_prefs(
            &t1,
            &json!({"all_notifications_prefs": json!({"channels": {"C_NOISE": {"muted": true}}}).to_string()}),
        )
        .expect("prefs");
        let thread_root = format!("{recent_1}.000100");
        let thread_reply = format!("{recent_2}.000200");
        s1.upsert_messages(
            &t1,
            &ChannelId::from("C_ENG"),
            &[
                msg(
                    format!("{old}.000000"),
                    Some("U2"),
                    "Ancient message mentioning <@U1>",
                ),
                Message {
                    latest_reply: Some(Ts::from(thread_reply.clone())),
                    reply_count: Some(1),
                    ..msg(
                        thread_root.clone(),
                        Some("U2"),
                        "Hey <@U1> can you review the release plan?",
                    )
                },
                Message {
                    thread_ts: Some(Ts::from(thread_root.clone())),
                    ..msg(thread_reply, Some("U1"), "Looking now!")
                },
            ],
        )
        .expect("upsert #eng");
        s1.upsert_message(
            &t1,
            &ChannelId::from("C_NOISE"),
            &msg(format!("{recent_1}.000000"), Some("U2"), "Muted alert ping"),
        )
        .expect("upsert #noise");
        s1.upsert_file_transcript(
            &t1,
            &FileId::from("F_NEW"),
            "parakeet-v3",
            "Fresh architecture review recording.",
        )
        .expect("file transcript");
        huddle_segment(
            &mut s1,
            &t1,
            "R_OLD",
            "U2",
            "en",
            "Old huddle outside window",
            old * 1_000,
        );
        huddle_segment(
            &mut s1,
            &t1,
            "R_NEW",
            "U2",
            "en",
            "New huddle inside window",
            recent_2 * 1_000,
        );
        drop(s1);

        run_table(
            McpServer::for_test(dir.path().to_path_buf(), vec![session()], None, false),
            &[
                initialize(1),
                tool(
                    2,
                    "get_catchup_summary",
                    json!({"since_hours":24}),
                    vec![
                        ("workspaces[*].team_id", json!(["T1"])),
                        ("window.kind", json!("custom")),
                        // Bob's <@U1> mention; Ana's own reply and the 100h-old message are out.
                        ("mentions[*].ts", json!([thread_root])),
                        ("mentions[0].kind", json!("direct_mention")),
                        ("mentions[0].author_name", json!("Bob")),
                        ("unread_channels[*].channel", json!(["C_ENG"])),
                        ("unread_channels[0].mention_count", json!(1)),
                        ("active_threads[*].thread_ts", json!([thread_root])),
                        ("active_threads[0].new_reply_count", json!(1)),
                        ("transcripts[*].file_id", json!(["F_NEW", null])),
                        ("transcripts[1].call_id", json!("R_NEW")),
                    ],
                ),
                // An explicit user id replaces the signed-in one for every workspace.
                tool(
                    3,
                    "get_catchup_summary",
                    json!({"since_hours":24,"user_id":"U2"}),
                    vec![("mentions", json!([]))],
                ),
            ],
        )
        .await;
    }

    /// With `sort: "relevant"`, hits are merged by BM25 rank across workspaces instead of
    /// interleaved per workspace, so the strongest hit wins whatever archive it lives in.
    #[tokio::test]
    async fn relevant_search_ranks_hits_across_workspaces() {
        let dir = TempDir::new("rank");
        for (team, body) in [("T1", "aurora notes"), ("T2", "aurora aurora aurora notes")] {
            let team_id = TeamId::from(team);
            let mut store =
                Store::open(dir.path().join(format!("{team}.sqlite"))).expect("open store");
            store
                .upsert_message(
                    &team_id,
                    &ChannelId::from("C1"),
                    &msg("1700000100.000000", None, body),
                )
                .expect("upsert message");
        }
        run_table(
            McpServer::for_test(dir.path().to_path_buf(), Vec::new(), None, false),
            &[
                initialize(1),
                tool(
                    2,
                    "search_messages",
                    json!({"query":"aurora","all_workspaces":true,"sort":"relevant","limit":1}),
                    vec![("count", json!(1)), ("messages[0].workspace", json!("T2"))],
                ),
                tool(
                    3,
                    "search_messages",
                    json!({"query":"aurora","all_workspaces":true,"sort":"relevant","limit":4}),
                    vec![("messages[*].workspace", json!(["T2", "T1"]))],
                ),
            ],
        )
        .await;
    }

    #[test]
    fn workspace_ids_are_path_safe() {
        let dir = TempDir::new("traversal");
        let server = McpServer::for_test(dir.path().to_path_buf(), Vec::new(), None, false);
        for bad in [
            "../secret_outside",
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
        assert!(
            McpServer::for_test(
                dir.path().to_path_buf(),
                Vec::new(),
                Some("../secret_outside".into()),
                false
            )
            .resolve_workspace(None)
            .is_err()
        );
    }

    #[test]
    fn cli_parses_mcp_subcommand_with_optional_workspace() {
        use clap::Parser as _;
        let cli = crate::cli::Cli::try_parse_from(["slackless", "mcp"]).expect("parse mcp");
        assert!(matches!(
            cli.command,
            crate::cli::Commands::Mcp { read_only: false }
        ));
        assert_eq!(cli.workspace, None);

        let cli = crate::cli::Cli::try_parse_from(["slackless", "--workspace", "T1", "mcp"])
            .expect("parse mcp with workspace");
        assert!(matches!(
            cli.command,
            crate::cli::Commands::Mcp { read_only: false }
        ));
        assert_eq!(cli.workspace.as_deref(), Some("T1"));

        let cli = crate::cli::Cli::try_parse_from(["slackless", "mcp", "--read-only"])
            .expect("parse mcp --read-only");
        assert!(matches!(
            cli.command,
            crate::cli::Commands::Mcp { read_only: true }
        ));
    }
}
