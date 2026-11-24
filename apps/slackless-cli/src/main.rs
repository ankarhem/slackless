//! slackless CLI — headless search/read/write for agents (extra-features X-3).
//!
//! Local commands (`search`, `read`, `channels`, `dms`, `auth status`) run offline against the
//! durable store; `send`/`reply`/`react` go through `slackless-core`'s API client with a stored
//! session and print a clear "not signed in" message when none exists. `auth login` opens the
//! sign-in window; `auth logout` forgets stored sessions. Every command prints
//! `--json` for machines and a tab-separated human default. Nothing here needs a live Slack
//! connection to start.

#![allow(clippy::large_unopt_stack_frames)]

mod archive;
mod live;
mod output;
mod session;
mod update;
mod write;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use slackless_core::auth::SessionStore;
use slackless_core::store::{SearchQuery, Store};
use slackless_core::types::{ChannelId, TeamId, Ts};

use crate::archive::Archive;
use crate::output::{ChannelView, DmView, MessageRow};
use crate::session::resolve_workspace;
use crate::write::{
    parse_duration_secs, run_delete, run_edit, run_mark_read, run_pin, run_pins, run_react,
    run_reply, run_schedule, run_scheduled, run_send, run_thread_live, run_unreact, run_upload,
};

/// Default page size of `read`.
const READ_LIMIT: usize = 50;
/// Default result cap of `search`.
const SEARCH_LIMIT: usize = 20;

const CLI_VERSION: &str = match option_env!("SLACKLESS_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(
    name = "slackless",
    version = CLI_VERSION,
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
        /// Search across every locally archived workspace at once (merges hits newest first).
        #[arg(long)]
        all_workspaces: bool,
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
        /// Only include messages at or after this cutoff (`30m`, `24h`, `7d`, `YYYY-MM-DD`, or unix secs).
        #[arg(long, value_name = "WHEN")]
        since: Option<String>,
    },
    /// Read a thread's root message and replies (offline archive by default, or `--live` from Slack).
    Thread {
        /// Channel id, #name, or @handle.
        channel: String,
        /// The ts of the thread root (first column of `slackless read` or `slackless threads`).
        thread_ts: String,
        /// Fetch fresh replies live via `conversations.replies` and mirror them locally.
        #[arg(long)]
        live: bool,
        /// Maximum number of messages when using `--live` (Slack max 999).
        #[arg(long, value_name = "N", default_value_t = 200)]
        limit: u32,
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
    /// Edit an existing message (`chat.update`, needs a stored session).
    Edit {
        channel: String,
        /// The ts of the message to edit.
        ts: String,
        /// New message text.
        text: String,
    },
    /// Delete a message (`chat.delete`, needs a stored session).
    Delete {
        channel: String,
        /// The ts of the message to delete.
        ts: String,
    },
    /// Add an emoji reaction to a message (needs a stored session).
    React {
        channel: String,
        /// The ts of the message (first column of `slackless read`).
        ts: String,
        /// Emoji shortcode, with or without colons ("tada" or ":tada:").
        emoji: String,
    },
    /// Remove an emoji reaction from a message (`reactions.remove`, needs a stored session).
    Unreact {
        channel: String,
        /// The ts of the message.
        ts: String,
        /// Emoji shortcode, with or without colons ("tada" or ":tada:").
        emoji: String,
    },
    /// Upload one or more local files to a conversation or thread (needs a stored session).
    Upload {
        /// Channel id, #name, or @handle.
        channel: String,
        /// File path(s) to upload.
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Optional initial message accompanying the uploaded file(s).
        #[arg(short, long)]
        comment: Option<String>,
        /// Optional thread root ts to upload into a thread.
        #[arg(long, value_name = "TS")]
        thread_ts: Option<String>,
    },
    /// List channels and private groups known to the local archive.
    Channels,
    /// List direct messages and group DMs known to the local archive.
    Dms,
    /// List or search workspace users in the local archive (offline).
    Users {
        /// Optional case-insensitive filter matching user id, handle, display name, or real name.
        query: Option<String>,
        /// Include deactivated/deleted accounts.
        #[arg(long)]
        include_deleted: bool,
    },
    /// Summarize recent archived activity per conversation (offline).
    Digest {
        /// Time window (`30m`, `24h`, `7d`, `YYYY-MM-DD`, or unix timestamp).
        #[arg(long, value_name = "WHEN", default_value = "24h")]
        since: String,
        /// Summarize across every locally archived workspace at once.
        #[arg(long)]
        all_workspaces: bool,
        /// Maximum conversations to show per workspace.
        #[arg(long, value_name = "N", default_value_t = 25)]
        limit: usize,
    },
    /// Mark a conversation read (`conversations.mark`; defaults to newest archived message ts).
    MarkRead {
        /// Channel id, #name, or @handle.
        channel: String,
        /// Message ts to mark as read cursor (defaults to newest archived message in channel).
        ts: Option<String>,
    },
    /// List pinned messages and files in a conversation (`pins.list`, needs a stored session).
    Pins {
        /// Channel id, #name, or @handle.
        channel: String,
    },
    /// Pin a message in a conversation (`pins.add`, needs a stored session).
    Pin {
        /// Channel id, #name, or @handle.
        channel: String,
        /// Message ts to pin.
        ts: String,
    },
    /// Unpin a message in a conversation (`pins.remove`, needs a stored session).
    Unpin {
        /// Channel id, #name, or @handle.
        channel: String,
        /// Message ts to unpin.
        ts: String,
    },
    /// Schedule a message for future delivery (`chat.scheduleMessage`, needs a stored session).
    Schedule {
        /// Channel id, #name, or @handle.
        channel: String,
        /// Message text (pass `-` to read from stdin).
        text: String,
        /// When to deliver: relative (`+15m`, `+2h`, `+1d`) or unix epoch seconds.
        #[arg(long, value_name = "WHEN")]
        at: String,
        /// Optional thread root ts.
        #[arg(long, value_name = "TS")]
        thread_ts: Option<String>,
    },
    /// List pending scheduled messages (`chat.scheduledMessages.list`, needs a stored session).
    Scheduled {
        /// Optional channel id, #name, or @handle to filter by.
        #[arg(long, value_name = "CHANNEL")]
        channel: Option<String>,
    },
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
    /// Show, set, or clear your Slack custom status (`users.profile.get` / `users.profile.set`).
    Status {
        #[command(subcommand)]
        command: StatusCommand,
    },
    /// Show live unread channels, DMs, mentions, and threads (`client.counts`, needs a stored session).
    Unread,
    /// Download a Slack file by ID (`files.info` + authenticated download, needs a stored session).
    Download {
        /// Slack file ID (e.g. `F0123ABC`).
        file_id: String,
        /// Optional destination file path (defaults to `./<filename>`).
        #[arg(short, long, value_name = "PATH")]
        out: Option<std::path::PathBuf>,
    },
    /// Backfill a conversation's historical messages into the local SQLite archive (needs a stored session).
    Backfill {
        /// Channel id, #name, or @handle.
        channel: String,
        /// Maximum number of history pages to walk (0 = walk all history to the beginning).
        #[arg(long, value_name = "N", default_value_t = 10)]
        pages: usize,
        /// Messages per page (Slack max 200).
        #[arg(long, value_name = "N", default_value_t = 200)]
        page_size: u32,
    },
    /// Check for and install Slackless updates from GitHub Releases (updates Slackless.app + CLI).
    Update {
        /// Only check whether a newer release is available without installing.
        #[arg(long)]
        check: bool,
        /// Reinstall the latest release even if the current version matches.
        #[arg(long)]
        force: bool,
        /// Quit and relaunch Slackless.app automatically after installing an update.
        #[arg(long)]
        restart: bool,
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
enum StatusCommand {
    /// Print your current Slack status.
    Show,
    /// Set your Slack custom status text, emoji, and optional expiry.
    Set {
        /// Status message text.
        text: String,
        /// Emoji shortcode (with or without colons, e.g. `calendar` or `:spiral_calendar_pad:`).
        #[arg(long)]
        emoji: Option<String>,
        /// Automatically clear after N seconds from now (0 = don't clear).
        #[arg(long, value_name = "SECS")]
        expires_in: Option<i64>,
    },
    /// Clear your Slack custom status.
    Clear,
}

#[derive(Subcommand)]
enum HuddleCommand {
    /// List available local microphones and speakers (offline).
    Devices,
    /// List available local webcam cameras (offline).
    Cameras,
    /// List available monitors and application windows for screen sharing (offline).
    Screens,
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
        /// Optional emoji reaction to broadcast once connected (tests official `PeerMessage` `data-channel`).
        #[arg(long, value_name = "EMOJI")]
        react: Option<String>,
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
            all_workspaces,
            format,
            out,
        } => {
            if all_workspaces {
                cmd_search_all_workspaces(ctx, &query, limit)
            } else {
                cmd_search(
                    ctx,
                    workspace.as_ref(),
                    &query,
                    limit,
                    format,
                    out.as_deref(),
                )
            }
        }
        Commands::Read {
            channel,
            limit,
            since,
        } => cmd_read(ctx, workspace.as_ref(), &channel, limit, since.as_deref()),
        Commands::Thread {
            channel,
            thread_ts,
            live,
            limit,
        } => {
            if live {
                run_thread_live(ctx, flag, &channel, &thread_ts, limit).await
            } else {
                cmd_thread(ctx, workspace.as_ref(), &channel, &thread_ts)
            }
        }
        Commands::Send { channel, text } => run_send(ctx, flag, &channel, text).await,
        Commands::Reply {
            channel,
            thread_ts,
            text,
        } => run_reply(ctx, flag, &channel, &thread_ts, text).await,
        Commands::Edit { channel, ts, text } => run_edit(ctx, flag, &channel, &ts, text).await,
        Commands::Delete { channel, ts } => run_delete(ctx, flag, &channel, &ts).await,
        Commands::React { channel, ts, emoji } => run_react(ctx, flag, &channel, &ts, &emoji).await,
        Commands::Unreact { channel, ts, emoji } => {
            run_unreact(ctx, flag, &channel, &ts, &emoji).await
        }
        Commands::Upload {
            channel,
            files,
            comment,
            thread_ts,
        } => run_upload(ctx, flag, &channel, &files, comment, thread_ts.as_deref()).await,
        Commands::Channels => cmd_channels(ctx, workspace.as_ref()),
        Commands::Dms => cmd_dms(ctx, workspace.as_ref()),
        Commands::Users {
            query,
            include_deleted,
        } => cmd_users(ctx, workspace.as_ref(), query.as_deref(), include_deleted),
        Commands::Digest {
            since,
            all_workspaces,
            limit,
        } => cmd_digest(ctx, workspace.as_ref(), &since, all_workspaces, limit),
        Commands::MarkRead { channel, ts } => {
            run_mark_read(ctx, flag, &channel, ts.as_deref()).await
        }
        Commands::Pins { channel } => run_pins(ctx, flag, &channel).await,
        Commands::Pin { channel, ts } => run_pin(ctx, flag, &channel, &ts, false).await,
        Commands::Unpin { channel, ts } => run_pin(ctx, flag, &channel, &ts, true).await,
        Commands::Schedule {
            channel,
            text,
            at,
            thread_ts,
        } => run_schedule(ctx, flag, &channel, text, &at, thread_ts.as_deref()).await,
        Commands::Scheduled { channel } => run_scheduled(ctx, flag, channel.as_deref()).await,
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
            HuddleCommand::Cameras => live::run_huddle_cameras(ctx),
            HuddleCommand::Screens => live::run_huddle_screens(ctx),
            HuddleCommand::Join {
                channel,
                muted,
                duration,
                input,
                output,
                react,
            } => {
                live::run_huddle_join(ctx, flag, &channel, muted, duration, input, output, react)
                    .await
            }
            HuddleCommand::Link { channel } => live::run_huddle_link(ctx, flag, &channel).await,
            HuddleCommand::Invite { channel, user } => {
                live::run_huddle_invite(ctx, flag, &channel, &user).await
            }
        },
        Commands::Status { command } => match command {
            StatusCommand::Show => live::run_status_show(ctx, flag).await,
            StatusCommand::Set {
                text,
                emoji,
                expires_in,
            } => live::run_status_set(ctx, flag, text, emoji, expires_in).await,
            StatusCommand::Clear => live::run_status_clear(ctx, flag).await,
        },
        Commands::Unread => live::run_unread(ctx, flag).await,
        Commands::Download { file_id, out } => {
            live::run_download(ctx, flag, &file_id, out.as_deref()).await
        }
        Commands::Backfill {
            channel,
            pages,
            page_size,
        } => live::run_backfill(ctx, flag, &channel, pages, page_size).await,
        Commands::Update {
            check,
            force,
            restart,
        } => update::run_update(ctx, check, force, restart),
        Commands::Auth { command } => match command {
            AuthCommand::Status => cmd_auth_status(ctx),
            AuthCommand::Login { domain } => cmd_auth_login(ctx, domain),
            AuthCommand::Logout => cmd_auth_logout(ctx, flag),
        },
    }
}

fn cmd_search_all_workspaces(ctx: &Ctx, query: &str, limit: usize) -> Result<ExitCode> {
    let mut team_ids: Vec<TeamId> = Vec::new();
    if let Ok(sessions) = SessionStore::new().and_then(|s| s.load()) {
        for s in sessions {
            if !team_ids.contains(&s.team_id) {
                team_ids.push(s.team_id);
            }
        }
    }
    for tid in archive::stored_team_ids() {
        if !team_ids.contains(&tid) {
            team_ids.push(tid);
        }
    }
    if team_ids.is_empty() {
        return Ok(empty_local(
            ctx,
            serde_json::json!({
                "query": query,
                "all_workspaces": true,
                "workspaces_searched": [],
                "count": 0,
                "messages": [],
            }),
            "searching across workspaces",
        ));
    }

    let parsed = SearchQuery::parse(query);
    let mut hits: Vec<(String, Option<String>, MessageRow)> = Vec::new();
    for ws in &team_ids {
        let Some(store) = open_archive_store(ws) else {
            continue;
        };
        let messages = store
            .search(ws, &parsed, limit)
            .with_context(|| format!("searching the local archive of {ws}"))?;
        if messages.is_empty() {
            continue;
        }
        let archive = Archive::open(ws);
        let names = archive
            .as_ref()
            .map_or_else(Default::default, |a| a.channel_names(ws));
        for message in &messages {
            let row = MessageRow::from_stored(message, archive.as_ref(), ws);
            let label = names.get(row.channel.as_str()).cloned();
            hits.push((ws.as_str().to_owned(), label, row));
        }
    }
    hits.sort_by(|a, b| b.2.ts.cmp(&a.2.ts));
    hits.truncate(limit);

    let mut human = String::new();
    let json_rows: Vec<serde_json::Value> = hits
        .iter()
        .map(|(ws_id, label, row)| {
            human.push_str(&format!("{ws_id}\t{}", row.human_line(label.as_deref())));
            let mut val = serde_json::to_value(row).unwrap_or_default();
            if let Some(obj) = val.as_object_mut() {
                obj.insert("workspace".to_owned(), serde_json::json!(ws_id));
            }
            val
        })
        .collect();
    let searched: Vec<&str> = team_ids.iter().map(TeamId::as_str).collect();
    output::emit(
        ctx.json,
        &serde_json::json!({
            "query": query,
            "all_workspaces": true,
            "workspaces_searched": searched,
            "count": json_rows.len(),
            "messages": json_rows,
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
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
    since: Option<&str>,
) -> Result<ExitCode> {
    let cutoff_secs = match since {
        Some(spec) => match parse_since_cutoff(spec, now_epoch_secs()) {
            Ok(secs) => Some(secs),
            Err(msg) => return Ok(fail(ctx, "invalid_args", &msg)),
        },
        None => None,
    };
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
    let mut messages = match open_archive_store(workspace) {
        Some(store) => store
            .channel_history(workspace, &channel_id, None, limit)
            .with_context(|| format!("reading the history of {channel_id}"))?,
        None => Vec::new(),
    };
    if let Some(cutoff) = cutoff_secs {
        messages.retain(|m| m.message.ts.secs().is_some_and(|s| s >= cutoff));
    }
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

fn cmd_thread(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    channel_arg: &str,
    thread_ts_arg: &str,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({
                "channel": channel_arg,
                "thread_ts": thread_ts_arg,
                "live": false,
                "count": 0,
                "messages": [],
            }),
            "reading thread",
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
    let thread_ts = Ts::from(thread_ts_arg.to_owned());
    let messages = match open_archive_store(workspace) {
        Some(store) => store
            .thread_replies(workspace, &channel_id, &thread_ts)
            .with_context(|| format!("reading thread {thread_ts} in {channel_id}"))?,
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
    output::emit(
        ctx.json,
        &serde_json::json!({
            "channel": channel_id.as_str(),
            "thread_ts": thread_ts.as_str(),
            "live": false,
            "count": rows.len(),
            "messages": rows,
        }),
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

fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Parse a `--since` cutoff (`30m`, `24h`, `7d`, `2w`, `YYYY-MM-DD`, or unix seconds) into
/// unix epoch seconds.
fn parse_since_cutoff(spec: &str, now_secs: i64) -> Result<i64, String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err("empty --since value".to_owned());
    }
    if let Ok(epoch) = trimmed.parse::<i64>() {
        return Ok(epoch);
    }
    if let Some((y, rest)) = trimmed.split_once('-') {
        if let Some((m, d)) = rest.split_once('-') {
            if let (Ok(year), Ok(month), Ok(day)) =
                (y.parse::<i64>(), m.parse::<u32>(), d.parse::<u32>())
            {
                if (1..=12).contains(&month) && (1..=31).contains(&day) {
                    return Ok(days_from_civil(year, month, day).saturating_mul(86_400));
                }
            }
        }
    }
    let delta = parse_duration_secs(trimmed)?;
    let delta_i64 = i64::try_from(delta).unwrap_or(i64::MAX);
    Ok(now_secs.saturating_sub(delta_i64))
}

/// Howard Hinnant's civil-from-days inverse: UTC `YYYY-MM-DD` -> days since `1970-01-01`.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn cmd_users(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    query: Option<&str>,
    include_deleted: bool,
) -> Result<ExitCode> {
    let Some(workspace) = workspace else {
        return Ok(empty_local(
            ctx,
            serde_json::json!({"count": 0, "users": []}),
            "listing users",
        ));
    };
    let archive = Archive::open(workspace);
    if archive.is_none() {
        archive_hint(workspace);
    }
    let users = archive
        .as_ref()
        .map_or_else(Vec::new, |a| a.users(workspace, query, include_deleted));
    let mut human = String::new();
    for u in &users {
        let display = u
            .display_name
            .as_deref()
            .or(u.real_name.as_deref())
            .unwrap_or(&u.name);
        let mut flags = Vec::new();
        if u.is_bot {
            flags.push("bot");
        }
        if u.deleted {
            flags.push("deleted");
        }
        let flag_str = if flags.is_empty() {
            String::new()
        } else {
            format!(" ({})", flags.join(", "))
        };
        human.push_str(&format!("{}\t@{}\t{}{flag_str}\n", u.id, u.name, display));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({"count": users.len(), "users": users}),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_digest(
    ctx: &Ctx,
    workspace: Option<&TeamId>,
    since: &str,
    all_workspaces: bool,
    limit: usize,
) -> Result<ExitCode> {
    let cutoff_secs = match parse_since_cutoff(since, now_epoch_secs()) {
        Ok(secs) => secs,
        Err(msg) => return Ok(fail(ctx, "invalid_args", &msg)),
    };
    let team_ids: Vec<TeamId> = if all_workspaces {
        let mut ids = Vec::new();
        if let Ok(sessions) = SessionStore::new().and_then(|s| s.load()) {
            for s in sessions {
                if !ids.contains(&s.team_id) {
                    ids.push(s.team_id);
                }
            }
        }
        for tid in archive::stored_team_ids() {
            if !ids.contains(&tid) {
                ids.push(tid);
            }
        }
        ids
    } else {
        workspace.into_iter().cloned().collect()
    };
    if team_ids.is_empty() {
        return Ok(empty_local(
            ctx,
            serde_json::json!({
                "since": since,
                "cutoff_secs": cutoff_secs,
                "total_messages": 0,
                "conversations": [],
            }),
            "building digest",
        ));
    }
    let mut total_messages: u64 = 0;
    let mut rows_json = Vec::new();
    let mut human = String::new();
    for ws in &team_ids {
        let Some(archive) = Archive::open(ws) else {
            continue;
        };
        let rows = archive.digest_since(ws, cutoff_secs, limit);
        for row in rows {
            total_messages = total_messages.saturating_add(row.message_count);
            let who = row.latest_author.as_deref().unwrap_or("unknown");
            if all_workspaces {
                human.push_str(&format!(
                    "{}\t{}\t{} msg ({} people)\t{who}: {}\n",
                    ws.as_str(),
                    row.label,
                    row.message_count,
                    row.author_count,
                    row.latest_text
                ));
            } else {
                human.push_str(&format!(
                    "{}\t{} msg ({} people)\t{who}: {}\n",
                    row.label, row.message_count, row.author_count, row.latest_text
                ));
            }
            let mut val = serde_json::to_value(&row).unwrap_or_default();
            if let Some(obj) = val.as_object_mut() {
                obj.insert("workspace".to_owned(), serde_json::json!(ws.as_str()));
            }
            rows_json.push(val);
        }
    }
    output::emit(
        ctx.json,
        &serde_json::json!({
            "since": since,
            "cutoff_secs": cutoff_secs,
            "all_workspaces": all_workspaces,
            "total_messages": total_messages,
            "count": rows_json.len(),
            "conversations": rows_json,
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
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

    #[test]
    fn parses_since_cutoffs_and_schedule_offsets() {
        let now = 1_764_000_000;
        assert_eq!(parse_since_cutoff("30m", now), Ok(now - 1800));
        assert_eq!(parse_since_cutoff("24h", now), Ok(now - 86_400));
        assert_eq!(parse_since_cutoff("7d", now), Ok(now - 7 * 86_400));
        assert_eq!(parse_since_cutoff("2w", now), Ok(now - 14 * 86_400));
        assert_eq!(parse_since_cutoff("1700000000", now), Ok(1_700_000_000));
        assert_eq!(parse_since_cutoff("1970-01-02", now), Ok(86_400));
        assert_eq!(parse_since_cutoff("2026-01-01", now), Ok(1_767_225_600));
        assert!(parse_since_cutoff("bogus", now).is_err());

        assert_eq!(
            crate::write::parse_schedule_at("+15m", 1_700_000_000),
            Ok(1_700_000_900)
        );
        assert_eq!(
            crate::write::parse_schedule_at("+2h", 1_700_000_000),
            Ok(1_700_007_200)
        );
        assert_eq!(
            crate::write::parse_schedule_at("1700005000", 1_700_000_000),
            Ok(1_700_005_000)
        );
        assert!(crate::write::parse_schedule_at("1699999999", 1_700_000_000).is_err());
        assert!(crate::write::parse_schedule_at("+0m", 1_700_000_000).is_err());
    }
}
