//! slackless CLI — headless search/read/write for agents (extra-features X-3).
//!
//! Local commands (`search`, `read`, `channels`, `dms`, `auth status`) run offline against the
//! durable store; `send`/`reply`/`react` go through `slackless-core`'s API client with a stored
//! session and print a clear "not signed in" message when none exists. `auth login` opens the
//! sign-in window; `auth logout` forgets stored sessions. Every command prints
//! `--json` for machines and a tab-separated human default. Nothing here needs a live Slack
//! connection to start.

mod archive;
mod output;
mod session;
mod write;

use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use slackless_core::auth::SessionStore;
use slackless_core::store::{SearchQuery, Store};
use slackless_core::types::{ChannelId, TeamId};

use crate::archive::Archive;
use crate::output::{ChannelView, DmView, MessageRow};
use crate::session::resolve_workspace;
use crate::write::{run_react, run_reply, run_send};

/// Default page size of `read`.
const READ_LIMIT: usize = 50;
/// Default result cap of `search`.
const SEARCH_LIMIT: usize = 20;

#[derive(Parser)]
#[command(
    name = "slackless",
    about = "Headless Slack client CLI over the local archive"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,
    /// Workspace (team id) to act on; defaults to the first stored session, else the only
    /// local archive.
    #[arg(long, global = true, value_name = "TEAM_ID")]
    workspace: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Full-text search the local archive (modifiers: in: from: has: before: after:).
    Search {
        /// Query text plus modifiers, e.g. "launch in:#eng from:@ana has:file".
        query: String,
        /// Maximum number of results.
        #[arg(long, value_name = "N", default_value_t = SEARCH_LIMIT)]
        limit: usize,
    },
    /// Print recent messages from a conversation (channel id, #name or @handle).
    Read {
        channel: String,
        /// Maximum number of messages (newest page, printed oldest first).
        #[arg(long, value_name = "N", default_value_t = READ_LIMIT)]
        limit: usize,
    },
    /// Post a message to a conversation (needs a stored session).
    Send { channel: String, text: String },
    /// Reply in a thread (needs a stored session).
    Reply {
        channel: String,
        /// The ts of the thread root (first column of `slackless read`).
        thread_ts: String,
        text: String,
    },
    /// Add an emoji reaction to a message (needs a stored session).
    React {
        channel: String,
        /// The ts of the message (first column of `slackless read`).
        ts: String,
        /// Emoji shortcode, with or without colons ("tada" or ":tada:").
        emoji: String,
    },
    /// List channels and private groups known to the local archive.
    Channels,
    /// List direct messages and group DMs known to the local archive.
    Dms,
    /// Account and session management.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
}

#[derive(Subcommand)]
enum AuthCommand {
    /// Show stored sessions and their local archives (offline).
    Status,
    /// Sign in through Slack's own sign-in page in a window slackless owns, then store the
    /// session (secrets in the macOS Keychain).
    Login {
        /// Workspace subdomain to open (`acme` for acme.slack.com); default: Slack's picker.
        domain: Option<String>,
    },
    /// Forget the stored session of `--workspace`, or every stored session without it.
    Logout,
}

/// Shared output mode for one invocation.
struct Ctx {
    json: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ctx = Ctx { json: cli.json };
    let flag = cli.workspace.clone();
    let workspace = resolve_workspace(flag.as_deref());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let result = match runtime {
        Ok(rt) => rt.block_on(dispatch(cli, &ctx, flag, workspace)),
        Err(err) => Err(anyhow::anyhow!("starting the async runtime: {err}")),
    };
    result.unwrap_or_else(|err| fail(&ctx, "command_failed", &format!("{err:#}")))
}

async fn dispatch(
    cli: Cli,
    ctx: &Ctx,
    flag: Option<String>,
    workspace: Option<TeamId>,
) -> Result<ExitCode> {
    let flag = flag.as_deref();
    match cli.command {
        Commands::Search { query, limit } => cmd_search(ctx, workspace.as_ref(), &query, limit),
        Commands::Read { channel, limit } => cmd_read(ctx, workspace.as_ref(), &channel, limit),
        Commands::Send { channel, text } => run_send(ctx, flag, &channel, text).await,
        Commands::Reply {
            channel,
            thread_ts,
            text,
        } => run_reply(ctx, flag, &channel, &thread_ts, text).await,
        Commands::React { channel, ts, emoji } => run_react(ctx, flag, &channel, &ts, &emoji).await,
        Commands::Channels => cmd_channels(ctx, workspace.as_ref()),
        Commands::Dms => cmd_dms(ctx, workspace.as_ref()),
        Commands::Auth { command } => match command {
            AuthCommand::Status => cmd_auth_status(ctx),
            AuthCommand::Login { domain } => cmd_auth_login(ctx, domain),
            AuthCommand::Logout => cmd_auth_logout(ctx, flag),
        },
    }
}

fn cmd_search(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    query: &str,
    limit: usize,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"query": query, "count": 0, "messages": []}),
            "searching",
        ));
    };
    let parsed = SearchQuery::parse(query);
    let messages = match open_archive_store(workspace) {
        Some(store) => store
            .search(workspace, &parsed, limit)
            .with_context(|| format!("searching the local archive of {workspace}"))?,
        None => Vec::new(),
    };
    let archive = Archive::open(workspace);
    let names = archive
        .as_ref()
        .map_or_else(Default::default, |archive| archive.channel_names(workspace));
    let rows: Vec<MessageRow> = messages
        .iter()
        .map(|message| MessageRow::from_stored(message, archive.as_ref(), workspace))
        .collect();
    let mut human = String::new();
    for row in &rows {
        let label = names.get(row.channel.as_str()).map(String::as_str);
        human.push_str(&row.human_line(label));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({"query": query, "count": rows.len(), "messages": rows}),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_read(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    channel_arg: &str,
    limit: usize,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"channel": {"id": channel_arg}, "count": 0, "messages": []}),
            "reading",
        ));
    };
    let archive = Archive::open(workspace);
    if archive.is_none() {
        archive_hint(workspace);
    }
    let resolved = archive
        .as_ref()
        .and_then(|a| a.resolve_channel(workspace, channel_arg));
    let channel_id = resolved
        .as_ref()
        .map_or_else(|| raw_channel_id(channel_arg), |row| row.id.clone());
    let label = resolved.as_ref().map(|row| row.label());
    let messages = match open_archive_store(workspace) {
        Some(store) => store
            .channel_history(workspace, &channel_id, None, limit)
            .with_context(|| format!("reading the history of {channel_id}"))?,
        None => Vec::new(),
    };
    let rows: Vec<MessageRow> = messages
        .iter()
        .map(|message| MessageRow::from_stored(message, archive.as_ref(), workspace))
        .collect();
    let mut human = String::new();
    for row in &rows {
        human.push_str(&row.human_line(label.as_deref()));
    }
    let channel = match archive
        .as_ref()
        .and_then(|a| a.conversation_by_id(workspace, &channel_id))
    {
        Some(row) => serde_json::json!({
            "id": channel_id.as_str(),
            "name": row.name,
            "kind": row.kind,
            "user": row.user.as_ref().map(|user| user.as_str()),
        }),
        None => serde_json::json!({"id": channel_id.as_str()}),
    };
    output::emit(
        ctx.json,
        &serde_json::json!({"channel": channel, "count": rows.len(), "messages": rows}),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_channels(ctx: &Ctx, workspace: Option<&TeamId>) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"count": 0, "channels": []}),
            "listing channels",
        ));
    };
    let archive = Archive::open(workspace);
    if archive.is_none() {
        archive_hint(workspace);
    }
    let rows: Vec<ChannelView> = match archive.as_ref() {
        Some(archive) => archive
            .channels(workspace)
            .iter()
            .map(ChannelView::from_row)
            .collect(),
        None => Vec::new(),
    };
    output::emit(
        ctx.json,
        &serde_json::json!({"count": rows.len(), "channels": rows}),
        &output::channels_human(&rows),
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_dms(ctx: &Ctx, workspace: Option<&TeamId>) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"count": 0, "dms": []}),
            "listing DMs",
        ));
    };
    let archive = Archive::open(workspace);
    if archive.is_none() {
        archive_hint(workspace);
    }
    let rows: Vec<DmView> = match archive.as_ref() {
        Some(archive) => archive
            .dms(workspace)
            .iter()
            .map(|row| DmView::from_row(row, archive, workspace))
            .collect(),
        None => Vec::new(),
    };
    output::emit(
        ctx.json,
        &serde_json::json!({"count": rows.len(), "dms": rows}),
        &output::dms_human(&rows),
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_auth_status(ctx: &Ctx) -> Result<ExitCode> {
    let entries = session::status_entries();
    let view = output::AuthStatusView {
        signed_in: entries.iter().any(|entry| entry.has_secrets),
        workspaces: entries
            .iter()
            .map(output::WorkspaceView::from_entry)
            .collect(),
    };
    output::emit(ctx.json, &view, &output::auth_human(&view));
    Ok(ExitCode::SUCCESS)
}

fn cmd_auth_login(ctx: &Ctx, workspace_domain: Option<String>) -> Result<ExitCode> {
    let sessions =
        slackless_auth::embedded_sign_in(slackless_auth::SignInRequest { workspace_domain })
            .context("signing in")?;
    let store = SessionStore::new()?;
    for session in sessions {
        store.upsert(session)?;
    }
    cmd_auth_status(ctx)
}

fn cmd_auth_logout(ctx: &Ctx, workspace: Option<&str>) -> Result<ExitCode> {
    let store = SessionStore::new()?;
    for session in store.load()? {
        if workspace.is_none_or(|team| team == session.team_id.as_str()) {
            store.remove(&session.team_id)?;
        }
    }
    cmd_auth_status(ctx)
}

/// The workspace's durable store for reading; `None` when no archive exists yet (creating one
/// as a side effect of a read would be surprising).
fn open_archive_store(workspace: &TeamId) -> Option<Store> {
    let path = archive::store_path(workspace)?;
    if !path.is_file() {
        return None;
    }
    Store::open(&path).ok()
}

/// The channel id when the archive cannot resolve the argument.
fn raw_channel_id(arg: &str) -> ChannelId {
    ChannelId::from(arg.strip_prefix('#').unwrap_or(arg))
}

/// Prints empty `--json` results (or a human hint) when no workspace is known: local commands
/// succeed with empty output rather than failing hard offline.
fn empty_local(ctx: &Ctx, empty: serde_json::Value, action: &str) -> ExitCode {
    if ctx.json {
        output::emit_json(&empty);
    } else {
        eprintln!(
            "slackless: no workspace known for {action}; pass --workspace TEAM_ID (run `slackless auth login` first)"
        );
    }
    ExitCode::SUCCESS
}

/// A note on stderr when a workspace has no local archive to read.
fn archive_hint(workspace: &TeamId) {
    eprintln!(
        "slackless: no local archive for workspace {workspace} yet; it fills up once the desktop app syncs"
    );
}

/// A user-actionable failure: message + JSON envelope, not an error dump.
fn fail(ctx: &Ctx, code: &str, message: &str) -> ExitCode {
    if ctx.json {
        output::emit_json(&serde_json::json!({"ok": false, "error": code, "message": message}));
    } else {
        eprintln!("slackless: {message}");
    }
    ExitCode::FAILURE
}
