use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Default page size of `read`.
pub(crate) const READ_LIMIT: usize = 50;
/// Default result cap of `search`.
pub(crate) const SEARCH_LIMIT: usize = 20;

pub(crate) const CLI_VERSION: &str = match option_env!("SLACKLESS_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(
    name = "slackless",
    version = CLI_VERSION,
    about = "Headless Slack client CLI over the local archive"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,
    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    pub(crate) json: bool,
    /// Workspace (team id) to act on; defaults to the first stored session, else the only
    /// local archive.
    #[arg(long, global = true, value_name = "TEAM_ID")]
    pub(crate) workspace: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum SearchSortArg {
    #[default]
    Recent,
    Relevant,
}

impl From<SearchSortArg> for slackless_core::store::SearchSort {
    fn from(arg: SearchSortArg) -> Self {
        match arg {
            SearchSortArg::Recent => Self::Recent,
            SearchSortArg::Relevant => Self::Relevant,
        }
    }
}

#[derive(Args)]
pub(crate) struct SearchArgs {
    /// Query text plus modifiers, e.g. "launch in:#eng from:@ana has:file".
    pub(crate) query: String,
    /// Maximum number of results.
    #[arg(long, value_name = "N", default_value_t = SEARCH_LIMIT)]
    pub(crate) limit: usize,
    /// Search across every locally archived workspace at once (messages, snippets, canvases, transcripts).
    #[arg(long)]
    pub(crate) all_workspaces: bool,
    /// Sort order (`recent` = newest first, `relevant` = FTS5 BM25 relevance).
    #[arg(long, value_enum, default_value_t = SearchSortArg::Recent)]
    pub(crate) sort: SearchSortArg,
    /// Optional export format (`md` or `json`) for search results.
    #[arg(long, value_enum)]
    pub(crate) format: Option<ExportFormat>,
    /// Write search results to a file instead of stdout.
    #[arg(long, value_name = "PATH")]
    pub(crate) out: Option<PathBuf>,
    /// Include locally archived deleted messages in results.
    #[arg(long)]
    pub(crate) include_deleted: bool,
}

#[derive(Args)]
pub(crate) struct ReadArgs {
    pub(crate) channel: String,
    /// Maximum number of messages (newest page, printed oldest first).
    #[arg(long, value_name = "N", default_value_t = READ_LIMIT)]
    pub(crate) limit: usize,
    /// Only include messages at or after this cutoff (`30m`, `24h`, `7d`, `YYYY-MM-DD`, or unix secs).
    #[arg(long, value_name = "WHEN")]
    pub(crate) since: Option<String>,
    /// Include locally archived deleted messages (marked as deleted).
    #[arg(long)]
    pub(crate) include_deleted: bool,
}

#[derive(Args)]
pub(crate) struct ThreadArgs {
    /// Channel id, #name, or @handle.
    pub(crate) channel: String,
    /// The ts of the thread root (first column of `slackless read` or `slackless threads`).
    pub(crate) thread_ts: String,
    /// Fetch fresh replies live via `conversations.replies` and mirror them locally.
    #[arg(long)]
    pub(crate) live: bool,
    /// Maximum number of messages when using `--live` (Slack max 999).
    #[arg(long, value_name = "N", default_value_t = 200)]
    pub(crate) limit: u32,
}

#[derive(Args)]
pub(crate) struct SendArgs {
    pub(crate) channel: String,
    pub(crate) text: String,
    /// Allow sending text that triggers the pre-send secret-leak detector.
    #[arg(long)]
    pub(crate) allow_secrets: bool,
}

#[derive(Args)]
pub(crate) struct ReplyArgs {
    pub(crate) channel: String,
    /// The ts of the thread root (first column of `slackless read`).
    pub(crate) thread_ts: String,
    pub(crate) text: String,
    /// Allow sending text that triggers the pre-send secret-leak detector.
    #[arg(long)]
    pub(crate) allow_secrets: bool,
}

#[derive(Args)]
pub(crate) struct EditArgs {
    pub(crate) channel: String,
    /// The ts of the message to edit.
    pub(crate) ts: String,
    /// New message text.
    pub(crate) text: String,
    /// Allow sending text that triggers the pre-send secret-leak detector.
    #[arg(long)]
    pub(crate) allow_secrets: bool,
}

#[derive(Args)]
pub(crate) struct ReactionArgs {
    pub(crate) channel: String,
    /// The ts of the message.
    pub(crate) ts: String,
    /// Emoji shortcode, with or without colons ("tada" or ":tada:").
    pub(crate) emoji: String,
}

#[derive(Args)]
pub(crate) struct UploadArgs {
    /// Channel id, #name, or @handle.
    pub(crate) channel: String,
    /// File path(s) to upload.
    #[arg(required = true)]
    pub(crate) files: Vec<PathBuf>,
    /// Optional initial message accompanying the uploaded file(s).
    #[arg(short, long)]
    pub(crate) comment: Option<String>,
    /// Optional thread root ts to upload into a thread.
    #[arg(long, value_name = "TS")]
    pub(crate) thread_ts: Option<String>,
    /// Allow uploading a `--comment` that triggers the pre-send secret-leak detector.
    #[arg(long)]
    pub(crate) allow_secrets: bool,
}

#[derive(Args)]
pub(crate) struct ScheduleArgs {
    /// Channel id, #name, or @handle.
    pub(crate) channel: String,
    /// Message text (pass `-` to read from stdin).
    pub(crate) text: String,
    /// When to deliver: relative (`+15m`, `+2h`, `+1d`) or unix epoch seconds.
    #[arg(long, value_name = "WHEN")]
    pub(crate) at: String,
    /// Optional thread root ts.
    #[arg(long, value_name = "TS")]
    pub(crate) thread_ts: Option<String>,
    /// Allow scheduling text that triggers the pre-send secret-leak detector.
    #[arg(long)]
    pub(crate) allow_secrets: bool,
}

#[derive(Args)]
pub(crate) struct ExportArgs {
    /// Channel id, #name, or @handle (required unless `--transcript` or `--huddle` is set).
    pub(crate) channel: Option<String>,
    /// Export a single thread (root + replies) by its root `ts` inside `channel`.
    #[arg(long, value_name = "TS", conflicts_with_all = ["transcript", "huddle"])]
    pub(crate) thread: Option<String>,
    /// Export a stored audio/video file transcript by Slack file ID (e.g. `F0123ABC`).
    #[arg(long, value_name = "FILE_ID", conflicts_with_all = ["thread", "huddle"])]
    pub(crate) transcript: Option<String>,
    /// Export a stored huddle transcript by call ID.
    #[arg(long, value_name = "CALL_ID", conflicts_with_all = ["thread", "transcript"])]
    pub(crate) huddle: Option<String>,
    /// Nest stored thread replies under thread roots when exporting a channel.
    #[arg(long)]
    pub(crate) include_threads: bool,
    /// Exclude locally preserved deleted messages from channel/thread export.
    #[arg(long)]
    pub(crate) exclude_deleted: bool,
    /// Output format (`md` or `json`).
    #[arg(long, value_enum, default_value_t = ExportFormat::Md)]
    pub(crate) format: ExportFormat,
    /// Maximum number of messages to export (newest window, ordered oldest first).
    #[arg(long, value_name = "N", default_value_t = 10_000)]
    pub(crate) limit: usize,
    /// Write export to a file instead of stdout.
    #[arg(long, value_name = "PATH")]
    pub(crate) out: Option<PathBuf>,
}

#[derive(Args)]
pub(crate) struct MarkThreadArgs {
    /// Channel id, #name, or @handle.
    pub(crate) channel: String,
    /// Thread root ts.
    pub(crate) thread_ts: String,
    /// Read cursor ts (defaults to `thread_ts`).
    pub(crate) ts: Option<String>,
    /// Mark the thread unread instead of read.
    #[arg(long)]
    pub(crate) unread: bool,
}

#[derive(Args)]
pub(crate) struct DigestArgs {
    /// Time window (`30m`, `24h`, `7d`, `YYYY-MM-DD`, or unix timestamp).
    #[arg(long, value_name = "WHEN", default_value = "24h")]
    pub(crate) since: String,
    /// Summarize across every locally archived workspace at once.
    #[arg(long)]
    pub(crate) all_workspaces: bool,
    /// Maximum conversations to show per workspace.
    #[arg(long, value_name = "N", default_value_t = 25)]
    pub(crate) limit: usize,
}

#[derive(Args)]
pub(crate) struct CatchupArgs {
    /// Time window (`24h`, `7d`, `since-last-visit`, or any duration/date cutoff; defaults to since last visit).
    #[arg(long, value_name = "WHEN")]
    pub(crate) since: Option<String>,
    /// Aggregate catch-up briefing across every locally archived workspace at once.
    #[arg(long)]
    pub(crate) all_workspaces: bool,
    /// Maximum items to include per briefing section.
    #[arg(long, value_name = "N", default_value_t = 25)]
    pub(crate) limit: usize,
}

#[derive(Args)]
pub(crate) struct FilesArgs {
    /// Optional channel id, #name, or @handle to filter by.
    #[arg(long, value_name = "CHANNEL")]
    pub(crate) channel: Option<String>,
    /// Optional Slack file types filter (e.g. `canvas`, `images`, `pdfs`).
    #[arg(long, value_name = "TYPES")]
    pub(crate) types: Option<String>,
    /// Maximum number of files (1..=200).
    #[arg(long, value_name = "N", default_value_t = 50)]
    pub(crate) limit: u32,
}

#[derive(Args)]
pub(crate) struct BackfillArgs {
    /// Channel id, #name, or @handle.
    pub(crate) channel: String,
    /// Maximum number of history pages to walk (0 = walk all history to the beginning).
    #[arg(long, value_name = "N", default_value_t = 10)]
    pub(crate) pages: usize,
    /// Messages per page (Slack max 200).
    #[arg(long, value_name = "N", default_value_t = 200)]
    pub(crate) page_size: u32,
}

#[derive(Args)]
pub(crate) struct ArchiveBackfillArgs {
    /// Channel id, #name, or @handle (omit or pass `--all` to backfill every archived channel).
    pub(crate) channel: Option<String>,
    /// Backfill every stored channel and DM in the workspace sequentially.
    #[arg(long)]
    pub(crate) all: bool,
    /// Also walk `conversations.replies` for every thread root encountered.
    #[arg(long)]
    pub(crate) threads: bool,
    /// Maximum number of history pages per channel (0 = walk all history to the beginning).
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub(crate) pages: usize,
    /// Messages per page (Slack max 200).
    #[arg(long, value_name = "N", default_value_t = 200)]
    pub(crate) page_size: u32,
}

#[derive(Subcommand)]
pub(crate) enum ArchiveCommand {
    /// Backfill one channel or the whole workspace into the local SQLite archive (needs a stored session).
    Backfill(Box<ArchiveBackfillArgs>),
    /// Show per-channel backfill sync status and local archive progress (offline).
    Status,
}

#[derive(Args)]
pub(crate) struct TranscribeArgs {
    /// Slack file ID (e.g. `F0123ABC`) to transcribe and index in the local archive.
    pub(crate) file_id: String,
    /// Local video or audio file path to transcribe directly (offline, skips Slack download).
    #[arg(long, value_name = "PATH")]
    pub(crate) file: Option<PathBuf>,
    /// Preferred local transcription provider (`parakeet` or `whisper`; defaults to auto-detect).
    #[arg(long, value_name = "PROVIDER")]
    pub(crate) provider: Option<String>,
}

#[derive(Args)]
pub(crate) struct UsersArgs {
    /// Optional case-insensitive filter matching user id, handle, display name, or real name.
    pub(crate) query: Option<String>,
    /// Include deactivated/deleted accounts.
    #[arg(long)]
    pub(crate) include_deleted: bool,
}

#[derive(Args)]
pub(crate) struct ThreadsArgs {
    /// Optional channel id, #name, or @handle to scope thread listing.
    #[arg(long, value_name = "CHANNEL")]
    pub(crate) channel: Option<String>,
    /// Maximum number of threads (newest first).
    #[arg(long, value_name = "N", default_value_t = 30)]
    pub(crate) limit: usize,
}

#[derive(Args)]
pub(crate) struct WatchArgs {
    /// Optional channel id, #name, or @handle to filter events to.
    #[arg(long, value_name = "CHANNEL")]
    pub(crate) channel: Option<String>,
    /// Stop after emitting N matching events.
    #[arg(long, value_name = "N")]
    pub(crate) count: Option<usize>,
}

#[derive(Args)]
pub(crate) struct DownloadArgs {
    /// Slack file ID (e.g. `F0123ABC`).
    pub(crate) file_id: String,
    /// Optional destination file path (defaults to `./<filename>`).
    #[arg(short, long, value_name = "PATH")]
    pub(crate) out: Option<std::path::PathBuf>,
}

#[derive(Args)]
pub(crate) struct UpdateArgs {
    /// Only check whether a newer release is available without installing.
    #[arg(long)]
    pub(crate) check: bool,
    /// Reinstall the latest release even if the current version matches.
    #[arg(long)]
    pub(crate) force: bool,
    /// Quit and relaunch Slackless.app automatically after installing an update.
    #[arg(long)]
    pub(crate) restart: bool,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Full-text search the local archive (modifiers: in: from: has: before: after:).
    Search(Box<SearchArgs>),
    /// Print recent messages from a conversation (channel id, #name or @handle).
    Read(Box<ReadArgs>),
    /// Read a thread's root message and replies (offline archive by default, or `--live` from Slack).
    Thread(Box<ThreadArgs>),
    /// Post a message to a conversation (needs a stored session).
    Send(Box<SendArgs>),
    /// Reply in a thread (needs a stored session).
    Reply(Box<ReplyArgs>),
    /// Edit an existing message (`chat.update`, needs a stored session).
    Edit(Box<EditArgs>),
    /// Delete a message (`chat.delete`, needs a stored session).
    Delete {
        channel: String,
        /// The ts of the message to delete.
        ts: String,
    },
    /// Add an emoji reaction to a message (needs a stored session).
    React(Box<ReactionArgs>),
    /// Remove an emoji reaction from a message (`reactions.remove`, needs a stored session).
    Unreact(Box<ReactionArgs>),
    /// Upload one or more local files to a conversation or thread (needs a stored session).
    Upload(Box<UploadArgs>),
    /// List channels and private groups known to the local archive.
    Channels,
    /// List direct messages and group DMs known to the local archive.
    Dms,
    /// List or search workspace users in the local archive (offline).
    Users(Box<UsersArgs>),
    /// Summarize recent archived activity per conversation (offline).
    Digest(Box<DigestArgs>),
    /// Local Catch-Up / Daily Briefing: mentions, unread channels, active threads, and new transcripts (offline).
    Catchup(Box<CatchupArgs>),
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
    Schedule(Box<ScheduleArgs>),
    /// List pending scheduled messages (`chat.scheduledMessages.list`, needs a stored session).
    Scheduled {
        /// Optional channel id, #name, or @handle to filter by.
        #[arg(long, value_name = "CHANNEL")]
        channel: Option<String>,
    },
    /// Cancel a pending scheduled message (`chat.deleteScheduledMessage` / `drafts.delete`, needs a stored session).
    Unschedule {
        /// Channel id, #name, or @handle.
        channel: String,
        /// Scheduled message or draft ID returned by `schedule` or `scheduled`.
        id: String,
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
    Threads(Box<ThreadsArgs>),
    /// Show the append-only local revision history (edits & deletions) for one message (offline).
    Revisions {
        /// Channel id, #name, or @handle.
        channel: String,
        /// The ts of the message (first column of `slackless read`).
        ts: String,
    },
    /// Export a conversation's locally archived messages to JSON or Markdown (offline).
    Export(Box<ExportArgs>),
    /// Mark a followed thread read or unread (`subscriptions.thread.mark`, needs a stored session).
    MarkThread(Box<MarkThreadArgs>),
    /// Manage Later / Saved items (needs a stored session).
    Saved {
        #[command(subcommand)]
        command: Box<SavedCommand>,
    },
    /// List workspace or channel files (`files.list`, needs a stored session).
    Files(Box<FilesArgs>),
    /// Read, create, edit, share, inspect, delete, or comment on Slack Canvases (needs a stored session).
    Canvas {
        #[command(subcommand)]
        command: Box<CanvasCommand>,
    },
    /// Read, create, or mutate Slack Lists (needs a stored session).
    List {
        #[command(subcommand)]
        command: Box<ListCommand>,
    },
    /// Stream live Slack messages and events over the realtime websocket (needs a stored session).
    Watch(Box<WatchArgs>),
    /// Native Slack Huddles (audio join/smoke-test, shareable link, invite, local audio devices).
    Huddle {
        #[command(subcommand)]
        command: Box<HuddleCommand>,
    },
    /// Show, set, or clear your Slack custom status (`users.profile.get` / `users.profile.set`).
    Status {
        #[command(subcommand)]
        command: Box<StatusCommand>,
    },
    /// Show live unread channels, DMs, mentions, and threads (`client.counts`, needs a stored session).
    Unread,
    /// Download a Slack file by ID (`files.info` + authenticated download, needs a stored session).
    Download(Box<DownloadArgs>),
    /// Transcribe a video/audio file locally (`parakeet` or `whisper`) and index it in local search.
    Transcribe(Box<TranscribeArgs>),
    /// Backfill a conversation's historical messages into the local SQLite archive (needs a stored session).
    Backfill(Box<BackfillArgs>),
    /// Manage the durable local SQLite archive (backfill channel/workspace history, inspect sync status).
    Archive {
        #[command(subcommand)]
        command: Box<ArchiveCommand>,
    },
    /// Check for and install Slackless updates from GitHub Releases (updates Slackless.app + CLI).
    Update(Box<UpdateArgs>),
    /// Run a JSON-RPC 2.0 stdio Model Context Protocol (MCP) server for AI agents.
    Mcp,
    /// Account and session management.
    Auth {
        #[command(subcommand)]
        command: Box<AuthCommand>,
    },
}

#[derive(Subcommand)]
pub(crate) enum SavedCommand {
    /// List saved items (`saved`, `completed`, or `archived`).
    List {
        #[arg(long, default_value = "saved")]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum CanvasAccessArg {
    Read,
    Write,
    Owner,
}

#[derive(Subcommand)]
pub(crate) enum CanvasCommand {
    /// Fetch a canvas's markdown (or HTML with `--html`).
    Read {
        file_id: String,
        #[arg(long)]
        html: bool,
    },
    /// Create a standalone or channel-tabbed canvas (`canvases.create`).
    Create {
        title: String,
        /// Optional markdown body (pass `-` to read from stdin).
        #[arg(long)]
        markdown: Option<String>,
        #[arg(long)]
        channel: Option<String>,
    },
    /// Edit a canvas via `canvases.edit` (replace whole canvas or one section, or `--append`).
    Edit {
        file_id: String,
        /// Markdown body (pass `-` to read from stdin).
        markdown: String,
        #[arg(long)]
        section: Option<String>,
        #[arg(long)]
        append: bool,
    },
    /// Fetch and display canvas metadata (`files.info`).
    Info {
        /// Canvas file ID (e.g. `F0123ABC`).
        canvas_id: String,
    },
    /// Permanently delete a canvas (`canvases.delete`).
    Delete {
        /// Canvas file ID (e.g. `F0123ABC`).
        canvas_id: String,
    },
    /// Grant read, write, or owner access on a canvas to channels or users (`canvases.access.set`).
    Share {
        /// Canvas file ID (e.g. `F0123ABC`).
        canvas_id: String,
        /// Access level to grant (`read`, `write`, or `owner`).
        #[arg(long, value_enum)]
        access: CanvasAccessArg,
        /// Channel id, #name, or @handle to grant access to (repeatable; mutually exclusive with `--user`).
        #[arg(long = "channel", value_name = "CHANNEL")]
        channels: Vec<String>,
        /// User id (e.g. `U0123ABC`) to grant access to (repeatable; mutually exclusive with `--channel`).
        #[arg(long = "user", value_name = "USER_ID")]
        users: Vec<String>,
        /// Also post the canvas permalink to each shared channel.
        #[arg(long)]
        post: bool,
    },
    /// Revoke canvas access from channels or users (`canvases.access.delete`).
    Unshare {
        /// Canvas file ID (e.g. `F0123ABC`).
        canvas_id: String,
        /// Channel id, #name, or @handle to revoke access from (repeatable; mutually exclusive with `--user`).
        #[arg(long = "channel", value_name = "CHANNEL")]
        channels: Vec<String>,
        /// User id (e.g. `U0123ABC`) to revoke access from (repeatable; mutually exclusive with `--channel`).
        #[arg(long = "user", value_name = "USER_ID")]
        users: Vec<String>,
    },
    /// List threaded comments on a canvas (open threads by default).
    Comments {
        /// Canvas file ID (e.g. `F0123ABC`).
        canvas_id: String,
        /// Show only resolved comment threads.
        #[arg(long, conflicts_with = "all")]
        resolved: bool,
        /// Show both open and resolved comment threads.
        #[arg(long, conflicts_with = "resolved")]
        all: bool,
    },
    /// Post a new canvas comment thread or reply to an existing one.
    Comment {
        /// Canvas file ID (e.g. `F0123ABC`).
        canvas_id: String,
        /// Comment text (pass `-` to read from stdin).
        text: String,
        /// Optional root comment thread ts to reply to.
        #[arg(long, value_name = "TS")]
        thread: Option<String>,
        /// Allow posting a comment that triggers the pre-send secret-leak detector.
        #[arg(long)]
        allow_secrets: bool,
    },
    /// Resolve or reopen canvas comment threads (`quip.thread.archive` / `unarchive` / `archiveAll`).
    Resolve {
        /// Canvas file ID (e.g. `F0123ABC`).
        canvas_id: String,
        /// Root comment thread ts to resolve (or reopen with `--reopen`).
        #[arg(long, value_name = "TS", conflicts_with = "all")]
        thread: Option<String>,
        /// Resolve every open comment thread on the canvas.
        #[arg(long, conflicts_with = "thread")]
        all: bool,
        /// Reopen a resolved comment thread instead of resolving it (requires `--thread`).
        #[arg(long)]
        reopen: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ListReadFormat {
    Table,
    Csv,
    Json,
}

#[derive(Subcommand)]
pub(crate) enum ListCommand {
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
pub(crate) enum ExportFormat {
    Md,
    Json,
}

#[derive(Subcommand)]
pub(crate) enum StatusCommand {
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
pub(crate) enum HuddleCommand {
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
        /// Enable the local webcam once connected (tests live AVFoundation/H.264 video publish).
        #[arg(long)]
        camera: bool,
        /// Enable portrait background blur when `--camera` is active.
        #[arg(long)]
        blur: bool,
        /// Share a monitor or window source ID once connected (e.g. `monitor:2` from `huddle screens`).
        #[arg(long, value_name = "SOURCE_ID")]
        screen: Option<String>,
        /// Transcription provider: `slack` (default), `parakeet` (local ONNX EN+SV), or `whisper` (local Whisper Turbo EN+SV).
        #[arg(long, value_name = "PROVIDER", default_value = "slack")]
        transcribe: String,
    },
    /// Search, list, or export locally archived text-only huddle transcripts (offline).
    Transcripts {
        /// Optional channel filter (id, #name, or @handle).
        channel: Option<String>,
        /// Full-text search across spoken huddle transcript lines (diacritic-insensitive EN/SV).
        #[arg(long, value_name = "QUERY")]
        search: Option<String>,
        /// Filter hits by detected language code (`en`, `sv`, etc.).
        #[arg(long, value_name = "LANG")]
        lang: Option<String>,
        /// Print the complete speaker-attributed transcript for a specific huddle call ID.
        #[arg(long, value_name = "CALL_ID")]
        call_id: Option<String>,
        /// Export format when printing a call transcript (`md` or `json`).
        #[arg(long, value_enum)]
        format: Option<ExportFormat>,
        /// Maximum sessions or search hits to return.
        #[arg(long, default_value_t = 50)]
        limit: usize,
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
pub(crate) enum AuthCommand {
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
