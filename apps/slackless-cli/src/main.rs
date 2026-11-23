//! slackless CLI — headless search/read/write for agents (extra-features X-3).
//!
//! Local commands (`search`, `read`, `channels`, `dms`, `auth status`) run offline against the
//! durable store; `send`/`reply`/`react` go through `slackless-core`'s API client with a stored
//! session and print a clear "not signed in" message when none exists. `auth login` opens the
//! sign-in window; `auth logout` forgets stored sessions. Every command prints
//! `--json` for machines and a tab-separated human default. Nothing here needs a live Slack
//! connection to start.

mod archive;
mod live;
mod output;
mod session;
mod write;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
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
        /// Optional export format (`md` or `json`) for search results.
        #[arg(long, value_enum)]
        format: Option<ExportFormat>,
        /// Write search results to a file instead of stdout.
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
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
    /// Show local SQLite archive statistics (offline).
    Stats {
        /// Include per-table row counts and the top 10 largest channels.
        #[arg(long)]
        breakdown: bool,
    },
    /// Checkpoint WAL, compact unused pages (VACUUM), and optimize FTS5 indexes (offline).
    Vacuum,
    /// List archived thread roots with replies from the local archive (offline).
    Threads {
        /// Optional channel id, #name, or @handle to scope thread listing.
        #[arg(long, value_name = "CHANNEL")]
        channel: Option<String>,
        /// Maximum number of threads (newest first).
        #[arg(long, value_name = "N", default_value_t = 30)]
        limit: usize,
    },
    /// Show the append-only local revision history (edits & deletions) for one message (offline).
    Revisions {
        /// Channel id, #name, or @handle.
        channel: String,
        /// The ts of the message (first column of `slackless read`).
        ts: String,
    },
    /// Export a conversation's locally archived messages to JSON or Markdown (offline).
    Export {
        /// Channel id, #name, or @handle.
        channel: String,
        /// Output format (`md` or `json`).
        #[arg(long, value_enum, default_value_t = ExportFormat::Md)]
        format: ExportFormat,
        /// Maximum number of messages to export (newest window, ordered oldest first).
        #[arg(long, value_name = "N", default_value_t = 10_000)]
        limit: usize,
        /// Write export to a file instead of stdout.
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
    },
    /// Mark a followed thread read or unread (`subscriptions.thread.mark`, needs a stored session).
    MarkThread {
        /// Channel id, #name, or @handle.
        channel: String,
        /// Thread root ts.
        thread_ts: String,
        /// Read cursor ts (defaults to `thread_ts`).
        ts: Option<String>,
        /// Mark the thread unread instead of read.
        #[arg(long)]
        unread: bool,
    },
    /// Manage Later / Saved items (needs a stored session).
    Saved {
        #[command(subcommand)]
        command: SavedCommand,
    },
    /// List workspace or channel files (`files.list`, needs a stored session).
    Files {
        /// Optional channel id, #name, or @handle to filter by.
        #[arg(long, value_name = "CHANNEL")]
        channel: Option<String>,
        /// Optional Slack file types filter (e.g. `canvas`, `images`, `pdfs`).
        #[arg(long, value_name = "TYPES")]
        types: Option<String>,
        /// Maximum number of files (1..=200).
        #[arg(long, value_name = "N", default_value_t = 50)]
        limit: u32,
    },
    /// Read, create, or edit Slack Canvases (needs a stored session).
    Canvas {
        #[command(subcommand)]
        command: CanvasCommand,
    },
    /// Read, create, or mutate Slack Lists (needs a stored session).
    List {
        #[command(subcommand)]
        command: ListCommand,
    },
    /// Stream live Slack messages and events over the realtime websocket (needs a stored session).
    Watch {
        /// Optional channel id, #name, or @handle to filter events to.
        #[arg(long, value_name = "CHANNEL")]
        channel: Option<String>,
        /// Stop after emitting N matching events.
        #[arg(long, value_name = "N")]
        count: Option<usize>,
    },
    /// Native Slack Huddles (audio join/smoke-test, shareable link, invite, local audio devices).
    Huddle {
        #[command(subcommand)]
        command: HuddleCommand,
    },
    /// Account and session management.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
}

#[derive(Subcommand)]
enum SavedCommand {
    /// List saved items (`in_progress`, `completed`, or `archived`).
    List {
        #[arg(long, default_value = "in_progress")]
        filter: String,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Save a message for Later (`saved.add`).
    Add {
        channel: String,
        ts: String,
        /// Optional reminder due time (unix epoch seconds).
        #[arg(long)]
        due: Option<u64>,
    },
    /// Mark a saved item completed (`saved.update`).
    Done { channel: String, ts: String },
    /// Remove a saved item from Later (`saved.delete`).
    Delete { channel: String, ts: String },
}

#[derive(Subcommand)]
enum CanvasCommand {
    /// Fetch a canvas's markdown (or HTML with `--html`).
    Read {
        file_id: String,
        #[arg(long)]
        html: bool,
    },
    /// Create a standalone or channel-tabbed canvas (`canvases.create`).
    Create {
        title: String,
        #[arg(long)]
        markdown: Option<String>,
        #[arg(long)]
        channel: Option<String>,
    },
    /// Edit a canvas via `canvases.edit` (replace whole canvas or one section, or `--append`).
    Edit {
        file_id: String,
        markdown: String,
        #[arg(long)]
        section: Option<String>,
        #[arg(long)]
        append: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ListReadFormat {
    Table,
    Csv,
    Json,
}

#[derive(Subcommand)]
enum ListCommand {
    /// Fetch a Slack List and print it as table, CSV, or JSON.
    Read {
        list_id: String,
        #[arg(long, value_enum, default_value_t = ListReadFormat::Table)]
        format: ListReadFormat,
    },
    /// Create a new Slack List (`lists.create`).
    Create {
        name: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        channel: Option<String>,
    },
    /// Add an item to a Slack List (`lists.records.create`).
    AddItem {
        list_id: String,
        title: String,
        /// Optional column ID (defaults to the list's primary column).
        #[arg(long)]
        column: Option<String>,
    },
    /// Delete an item from a Slack List (`lists.records.delete`).
    DeleteItem { list_id: String, row_id: String },
    /// Add a Slack message to an existing List (`lists.addToList`).
    AddMessage {
        list_id: String,
        channel: String,
        ts: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ExportFormat {
    Md,
    Json,
}

#[derive(Subcommand)]
enum HuddleCommand {
    /// List available local microphones and speakers (offline).
    Devices,
    /// Join or start a live native audio huddle in a channel (Ctrl+C or `--duration` leaves cleanly).
    Join {
        /// Channel id, #name, or @handle.
        channel: String,
        /// Start with microphone muted.
        #[arg(long)]
        muted: bool,
        /// Automatically leave after N seconds (useful for headless smoke tests).
        #[arg(long, value_name = "SECS")]
        duration: Option<u64>,
        /// Preferred input microphone name (defaults to system default).
        #[arg(long, value_name = "NAME")]
        input: Option<String>,
        /// Preferred output speaker name (defaults to system default).
        #[arg(long, value_name = "NAME")]
        output: Option<String>,
    },
    /// Fetch the shareable huddle link for a channel (`rooms.getLink`).
    Link {
        /// Channel id, #name, or @handle.
        channel: String,
    },
    /// Invite/ring a workspace member into a channel's active huddle (`rooms.notifyMember`).
    Invite {
        /// Channel id, #name, or @handle.
        channel: String,
        /// Member user id (e.g. `U0123ABC`).
        user: String,
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
        Commands::Search {
            query,
            limit,
            format,
            out,
        } => cmd_search(
            ctx,
            workspace.as_ref(),
            &query,
            limit,
            format,
            out.as_deref(),
        ),
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
        Commands::Stats { breakdown } => cmd_stats(ctx, workspace.as_ref(), breakdown),
        Commands::Vacuum => cmd_vacuum(ctx, workspace.as_ref()),
        Commands::Threads { channel, limit } => {
            cmd_threads(ctx, workspace.as_ref(), channel.as_deref(), limit)
        }
        Commands::Revisions { channel, ts } => {
            cmd_revisions(ctx, workspace.as_ref(), &channel, &ts)
        }
        Commands::Export {
            channel,
            format,
            limit,
            out,
        } => cmd_export(
            ctx,
            workspace.as_ref(),
            &channel,
            format,
            limit,
            out.as_deref(),
        ),
        Commands::MarkThread {
            channel,
            thread_ts,
            ts,
            unread,
        } => live::run_mark_thread(ctx, flag, &channel, &thread_ts, ts.as_deref(), unread).await,
        Commands::Saved { command } => match command {
            SavedCommand::List { filter, limit } => {
                live::run_saved_list(ctx, flag, &filter, limit).await
            }
            SavedCommand::Add { channel, ts, due } => {
                live::run_saved_add(ctx, flag, &channel, &ts, due).await
            }
            SavedCommand::Done { channel, ts } => {
                live::run_saved_done(ctx, flag, &channel, &ts).await
            }
            SavedCommand::Delete { channel, ts } => {
                live::run_saved_delete(ctx, flag, &channel, &ts).await
            }
        },
        Commands::Files {
            channel,
            types,
            limit,
        } => live::run_files(ctx, flag, channel.as_deref(), types, limit).await,
        Commands::Canvas { command } => match command {
            CanvasCommand::Read { file_id, html } => {
                live::run_canvas_read(ctx, flag, &file_id, html).await
            }
            CanvasCommand::Create {
                title,
                markdown,
                channel,
            } => {
                live::run_canvas_create(ctx, flag, &title, markdown.as_deref(), channel.as_deref())
                    .await
            }
            CanvasCommand::Edit {
                file_id,
                markdown,
                section,
                append,
            } => live::run_canvas_edit(ctx, flag, &file_id, markdown, section, append).await,
        },
        Commands::List { command } => match command {
            ListCommand::Read { list_id, format } => {
                live::run_list_read(ctx, flag, &list_id, format).await
            }
            ListCommand::Create {
                name,
                description,
                channel,
            } => {
                live::run_list_create(ctx, flag, &name, description.as_deref(), channel.as_deref())
                    .await
            }
            ListCommand::AddItem {
                list_id,
                title,
                column,
            } => live::run_list_add_item(ctx, flag, &list_id, title, column).await,
            ListCommand::DeleteItem { list_id, row_id } => {
                live::run_list_delete_item(ctx, flag, &list_id, &row_id).await
            }
            ListCommand::AddMessage {
                list_id,
                channel,
                ts,
            } => live::run_list_add_message(ctx, flag, &list_id, &channel, &ts).await,
        },
        Commands::Watch { channel, count } => {
            live::run_watch(ctx, flag, channel.as_deref(), count).await
        }
        Commands::Huddle { command } => match command {
            HuddleCommand::Devices => live::run_huddle_devices(ctx),
            HuddleCommand::Join {
                channel,
                muted,
                duration,
                input,
                output,
            } => live::run_huddle_join(ctx, flag, &channel, muted, duration, input, output).await,
            HuddleCommand::Link { channel } => live::run_huddle_link(ctx, flag, &channel).await,
            HuddleCommand::Invite { channel, user } => {
                live::run_huddle_invite(ctx, flag, &channel, &user).await
            }
        },
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
    format: Option<ExportFormat>,
    out: Option<&std::path::Path>,
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
    if format.is_some() || out.is_some() {
        let effective = if ctx.json {
            ExportFormat::Json
        } else {
            format.unwrap_or(ExportFormat::Md)
        };
        let rendered = match effective {
            ExportFormat::Json => serde_json::to_string_pretty(&serde_json::json!({
                "query": query,
                "count": rows.len(),
                "messages": rows,
            }))?,
            ExportFormat::Md => format_markdown_export(&format!("Search: {query}"), &rows),
        };
        if let Some(path) = out {
            std::fs::write(path, &rendered)
                .with_context(|| format!("writing search results to {}", path.display()))?;
            if ctx.json {
                output::emit_json(&serde_json::json!({
                    "ok": true,
                    "path": path.display().to_string(),
                    "count": rows.len(),
                }));
            } else {
                eprintln!(
                    "Exported {} search matches to {}",
                    rows.len(),
                    path.display()
                );
            }
        } else {
            print!("{rendered}");
            if !rendered.ends_with('\n') {
                println!();
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
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

fn cmd_stats(ctx: &Ctx, workspace: Option<&TeamId>, breakdown: bool) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"workspace": null, "messages": 0}),
            "reading archive stats",
        ));
    };
    let db_path = archive::store_path(workspace);
    let Some(store) = open_archive_store(workspace) else {
        archive_hint(workspace);
        return Ok(ExitCode::SUCCESS);
    };
    let stats = store
        .stats(workspace)
        .with_context(|| format!("reading local archive stats for {workspace}"))?;
    let detail = if breakdown {
        Some(
            store
                .storage_breakdown(workspace, 10)
                .with_context(|| format!("reading archive breakdown for {workspace}"))?,
        )
    } else {
        None
    };
    let mut json = serde_json::json!({
        "workspace": workspace.as_str(),
        "db_path": db_path.as_ref().map(|p| p.display().to_string()),
        "db_bytes": stats.db_bytes(),
        "messages": stats.messages,
        "revisions": stats.revisions,
        "conversations": stats.conversations,
        "users": stats.users,
        "offline_queued": stats.offline_queued,
    });
    let mut human = format_stats_human(
        workspace.as_str(),
        db_path.as_deref(),
        stats.db_bytes(),
        stats.messages,
        stats.revisions,
        stats.conversations,
        stats.users,
        stats.offline_queued,
    );
    if let Some(detail) = detail {
        if let Some(obj) = json.as_object_mut() {
            obj.insert(
                "tables".to_owned(),
                serde_json::json!(
                    detail
                        .tables
                        .iter()
                        .map(|(name, rows)| serde_json::json!({"table": name, "rows": rows}))
                        .collect::<Vec<_>>()
                ),
            );
            obj.insert(
                "top_channels".to_owned(),
                serde_json::json!(detail
                    .top_channels
                    .iter()
                    .map(|(id, name, rows)| serde_json::json!({"id": id, "name": name, "messages": rows}))
                    .collect::<Vec<_>>()),
            );
        }
        human.push_str("\nTables:\n");
        for (name, rows) in &detail.tables {
            human.push_str(&format!("  {name:<18} {rows}\n"));
        }
        if !detail.top_channels.is_empty() {
            human.push_str("\nTop channels by message count:\n");
            for (id, name, rows) in &detail.top_channels {
                let label = name.as_deref().unwrap_or(id.as_str());
                human.push_str(&format!("  {label:<28} {rows} ({id})\n"));
            }
        }
    }
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

fn cmd_vacuum(ctx: &Ctx, workspace: Option<&TeamId>) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"ok": false, "reason": "no_workspace"}),
            "compacting archive",
        ));
    };
    let Some(store) = open_archive_store(workspace) else {
        archive_hint(workspace);
        return Ok(ExitCode::SUCCESS);
    };
    let before = store.stats(workspace).map(|s| s.db_bytes()).unwrap_or(0);
    store
        .vacuum()
        .with_context(|| format!("compacting local archive for {workspace}"))?;
    let after = store.stats(workspace).map(|s| s.db_bytes()).unwrap_or(0);
    let reclaimed = before.saturating_sub(after);
    let json = serde_json::json!({
        "ok": true,
        "workspace": workspace.as_str(),
        "before_bytes": before,
        "after_bytes": after,
        "reclaimed_bytes": reclaimed,
    });
    let human = format!(
        "Compacted {workspace}: {:.1} MiB -> {:.1} MiB (reclaimed {:.1} MiB)\n",
        before as f64 / (1024.0 * 1024.0),
        after as f64 / (1024.0 * 1024.0),
        reclaimed as f64 / (1024.0 * 1024.0),
    );
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

fn cmd_threads(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    channel_arg: Option<&str>,
    limit: usize,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"count": 0, "threads": []}),
            "listing threads",
        ));
    };
    let archive = Archive::open(workspace);
    if archive.is_none() {
        archive_hint(workspace);
    }
    let resolved_ch = channel_arg.map(|arg| {
        archive
            .as_ref()
            .and_then(|a| a.resolve_channel(workspace, arg))
            .map_or_else(|| raw_channel_id(arg), |row| row.id)
    });
    let messages = match open_archive_store(workspace) {
        Some(store) => store
            .recent_threads(workspace, resolved_ch.as_ref(), limit)
            .with_context(|| format!("listing archived threads for {workspace}"))?,
        None => Vec::new(),
    };
    let names = archive
        .as_ref()
        .map_or_else(Default::default, |a| a.channel_names(workspace));
    let rows: Vec<MessageRow> = messages
        .iter()
        .map(|m| MessageRow::from_stored(m, archive.as_ref(), workspace))
        .collect();
    let mut human = String::new();
    for row in &rows {
        let label = names
            .get(row.channel.as_str())
            .map(String::as_str)
            .unwrap_or(&row.channel);
        let replies = row.reply_count.unwrap_or(0);
        let author = row
            .user_name
            .as_deref()
            .or(row.user.as_deref())
            .unwrap_or("unknown");
        let snippet: String = row.text.chars().take(80).collect();
        human.push_str(&format!(
            "{label}\t{}\t{replies} replies\t{author}: {snippet}\n",
            row.ts
        ));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({"count": rows.len(), "threads": rows}),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_revisions(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    channel_arg: &str,
    ts: &str,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"count": 0, "revisions": []}),
            "reading revisions",
        ));
    };
    let archive = Archive::open(workspace);
    let channel_id = archive
        .as_ref()
        .and_then(|a| a.resolve_channel(workspace, channel_arg))
        .map_or_else(|| raw_channel_id(channel_arg), |row| row.id);
    let revisions = match open_archive_store(workspace) {
        Some(store) => store
            .revisions(workspace, &channel_id, &slackless_core::types::Ts::from(ts))
            .with_context(|| format!("reading revisions of {channel_id} {ts}"))?,
        None => Vec::new(),
    };
    let json_rows: Vec<_> = revisions
        .iter()
        .map(|rev| {
            serde_json::json!({
                "edit_ts": rev.edit_ts,
                "deleted": rev.deleted,
                "text": rev.message.text,
            })
        })
        .collect();
    let mut human = String::new();
    for (ix, rev) in revisions.iter().enumerate() {
        let kind = if rev.edit_ts.is_empty() {
            "original"
        } else if rev.deleted {
            "deleted"
        } else {
            "edited"
        };
        let stamp = if rev.edit_ts.is_empty() {
            ts
        } else {
            rev.edit_ts.as_str()
        };
        human.push_str(&format!(
            "#{}\t{kind}\t{stamp}\t{}\n",
            ix + 1,
            rev.message.text
        ));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({
            "channel": channel_id.as_str(),
            "ts": ts,
            "count": json_rows.len(),
            "revisions": json_rows,
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_export(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    channel_arg: &str,
    format: ExportFormat,
    limit: usize,
    out: Option<&std::path::Path>,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"channel": channel_arg, "count": 0, "messages": []}),
            "exporting",
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
    let label = resolved
        .as_ref()
        .map(|row| row.label())
        .unwrap_or_else(|| channel_id.as_str().to_owned());
    let messages = match open_archive_store(workspace) {
        Some(store) => store
            .channel_history(workspace, &channel_id, None, limit)
            .with_context(|| format!("exporting the history of {channel_id}"))?,
        None => Vec::new(),
    };
    let rows: Vec<MessageRow> = messages
        .iter()
        .map(|message| MessageRow::from_stored(message, archive.as_ref(), workspace))
        .collect();
    let effective = if ctx.json { ExportFormat::Json } else { format };
    let rendered = match effective {
        ExportFormat::Json => serde_json::to_string_pretty(&serde_json::json!({
            "workspace": workspace.as_str(),
            "channel": {
                "id": channel_id.as_str(),
                "label": label,
            },
            "count": rows.len(),
            "messages": rows,
        }))?,
        ExportFormat::Md => format_markdown_export(&label, &rows),
    };
    if let Some(path) = out {
        std::fs::write(path, &rendered)
            .with_context(|| format!("writing export to {}", path.display()))?;
        if ctx.json {
            output::emit_json(&serde_json::json!({
                "ok": true,
                "path": path.display().to_string(),
                "count": rows.len(),
            }));
        } else {
            eprintln!("Exported {} messages to {}", rows.len(), path.display());
        }
    } else {
        print!("{rendered}");
        if !rendered.ends_with('\n') {
            println!();
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[allow(clippy::too_many_arguments)]
fn format_stats_human(
    workspace: &str,
    db_path: Option<&std::path::Path>,
    db_bytes: u64,
    messages: u64,
    revisions: u64,
    conversations: u64,
    users: u64,
    offline_queued: u64,
) -> String {
    let path = db_path
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(unknown)".to_owned());
    let size = if db_bytes >= 1024 * 1024 {
        format!("{:.1} MiB", db_bytes as f64 / (1024.0 * 1024.0))
    } else if db_bytes >= 1024 {
        format!("{:.1} KiB", db_bytes as f64 / 1024.0)
    } else {
        format!("{db_bytes} B")
    };
    format!(
        "Workspace:\t{workspace}\nDatabase:\t{path} ({size})\nMessages:\t{messages} ({revisions} revisions)\nConversations:\t{conversations}\nUsers:\t\t{users}\nOffline queue:\t{offline_queued}\n"
    )
}

fn format_markdown_export(label: &str, rows: &[MessageRow]) -> String {
    let mut out = format!("# {label}\n\n");
    for row in rows {
        let author = row
            .user_name
            .as_deref()
            .or(row.user.as_deref())
            .or(row.bot_id.as_deref())
            .unwrap_or("unknown");
        let deleted = if row.deleted { " _(deleted)_" } else { "" };
        out.push_str(&format!("**{author}** (`{}`){deleted}\n\n", row.ts));
        if !row.text.is_empty() {
            out.push_str(&row.text);
            out.push_str("\n\n");
        }
        if !row.files.is_empty() {
            out.push_str(&format!("_Files:_ {}\n\n", row.files.join(", ")));
        }
        if !row.reactions.is_empty() {
            let rx: Vec<String> = row
                .reactions
                .iter()
                .map(|r| format!(":{}: {}", r.name, r.count))
                .collect();
            out.push_str(&format!("_Reactions:_ {}\n\n", rx.join(" · ")));
        }
        out.push_str("---\n\n");
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::ReactionRow;

    #[test]
    fn formats_markdown_export_and_stats_human() {
        let rows = vec![MessageRow {
            channel: "C1".into(),
            ts: "1700000001.000100".into(),
            user: Some("U1".into()),
            user_name: Some("Ana".into()),
            bot_id: None,
            text: "Shipped release".into(),
            thread_ts: None,
            subtype: None,
            deleted: false,
            edited: false,
            reply_count: None,
            reactions: vec![ReactionRow {
                name: "tada".into(),
                count: 2,
            }],
            files: vec!["notes.md".into()],
            permalink: None,
        }];
        let md = format_markdown_export("#eng", &rows);
        assert!(md.contains("# #eng"));
        assert!(md.contains("**Ana** (`1700000001.000100`)"));
        assert!(md.contains("Shipped release"));
        assert!(md.contains("_Files:_ notes.md"));
        assert!(md.contains(":tada: 2"));

        let stats = format_stats_human(
            "T1",
            Some(std::path::Path::new("/tmp/T1.sqlite")),
            2 * 1024 * 1024,
            42,
            50,
            7,
            12,
            1,
        );
        assert!(stats.contains("2.0 MiB"));
        assert!(stats.contains("42 (50 revisions)"));
        assert!(stats.contains("Offline queue:\t1"));
    }
}
