#![allow(unknown_lints, clippy::large_unopt_stack_frames)]

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

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

#[derive(Subcommand)]
pub(crate) enum Commands {
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
    /// Read, create, edit, share, inspect, delete, or comment on Slack Canvases (needs a stored session).
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
