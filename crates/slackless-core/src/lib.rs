//! slackless orchestrator: owns the tokio runtime thread and wires api + rtm + store + auth into
//! a front-end-agnostic command/event API (ARCHITECTURE: core).
//!
//! GPUI is smol-based, so the networking stack (reqwest, tokio-tungstenite) runs on a dedicated
//! OS thread that owns a multi-thread tokio runtime ([`Client::start`]). Front-ends talk to it
//! over runtime-agnostic `futures::channel::mpsc` channels ([`ClientHandle`]): [`Command`]s in,
//! [`Event`]s out.
//!
//! The [`store::Store`] is the source of truth. The core keeps it current (hydrate, websocket
//! events via [`apply_event`], history backfill) and emits events saying what changed; front-ends
//! re-read the store through [`ClientHandle::store`].
//!
//! Patchability: the core models no Slack JSON itself. Responses and websocket events are parsed
//! by `slackless_types` / `slackless_api` (lenient fields, catch-all `Unknown` / `Other`
//! variants, method names and URLs centralized in `types::endpoints`).

pub use slackless_api as api;
pub use slackless_auth as auth;
pub use slackless_platform as platform;
pub use slackless_rtm as rtm;
pub use slackless_store as store;
pub use slackless_types as types;

mod backend;
mod backfill;
mod client;
mod fetcher;
mod notify;
mod reducer;
mod unfurl;

pub use backend::Backend;
pub use client::{Client, ClientHandle, SharedStore};
pub use fetcher::Fetcher;
pub use notify::{NotifyLevel, notify_levels};
pub use reducer::apply_event;

use api::MessageContent;
use rtm::ConnectionState;
use store::StoredMessage;
use types::{ChannelId, File, Presence, Ts, UserId};

/// Commands a front-end sends into the core.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// The user opened a conversation: load its newest history into the store and keep
    /// backfilling older pages in the background until the beginning is reached.
    OpenConversation(ChannelId),
    /// Stop backfilling a conversation (it stays in the store).
    CloseConversation(ChannelId),
    /// Look up people the workspace directory (`users.list`) leaves out, such as members of
    /// other organizations in shared channels and DMs, so they show by name. Each id is asked
    /// once per session; found profiles are stored and announced with
    /// [`Event::UsersChanged`], then the other organizations they belong to are looked up
    /// (`team.info`, each once per session), stored and announced with
    /// [`Event::SidebarChanged`].
    ResolveUsers(Vec<UserId>),
    /// The user chose "Leave channel": leave it on Slack, then store it as no longer joined
    /// and announce it with [`Event::Conversations`].
    LeaveConversation(ChannelId),
    /// The header bell's choice: set `channel`'s notification level on Slack, mirror it into
    /// the stored prefs, then announce [`Event::SidebarChanged`].
    SetNotifyLevel {
        channel: ChannelId,
        level: NotifyLevel,
    },
    /// The user opened a thread: fetch its full history (root + all replies) from Slack into
    /// the store. Read-only; it never moves the user's read cursor.
    OpenThread {
        channel: ChannelId,
        thread_ts: Ts,
    },
    /// The user opened the Threads view: fetch the followed threads from Slack
    /// (`subscriptions.thread.getView`) and announce them as [`Event::ThreadsView`]. No
    /// store fallback lives here; the front-end serves its local data until the event lands
    /// (and keeps it when the fetch fails).
    FetchThreads,
    /// The user opened the Activity view (or the badge needs its count): fetch Slack's
    /// Activity feed (`activity.feed`, page one) with its row bodies (`messages.list`) and
    /// announce it as [`Event::ActivityView`]. The boot hydrate sends the same command
    /// without the bodies, so the badges light up before the view is ever opened.
    FetchActivity {
        /// Hydrate the rows' bodies through `messages.list` (the view needs them; the
        /// badge does not).
        with_bodies: bool,
    },
    /// Mark feed entries read (`activity.markRead`) or unread (`activity.markUnread`);
    /// `entries` carries each item's handles (see `slackless_api::ActivityItem::entry`).
    MarkActivity {
        entries: Vec<api::ActivityEntry>,
        read: bool,
    },
    /// Post a message (its blocks, with the text as fallback); `thread_ts` replies in a
    /// thread, and `broadcast` also posts that reply to the channel (Slack's "Also send to").
    SendMessage {
        channel: ChannelId,
        content: MessageContent,
        thread_ts: Option<Ts>,
        broadcast: bool,
    },
    /// Replace a message's text.
    EditMessage {
        channel: ChannelId,
        ts: Ts,
        text: String,
    },
    DeleteMessage {
        channel: ChannelId,
        ts: Ts,
    },
    /// `name` is the emoji shortcode, with or without colons.
    AddReaction {
        channel: ChannelId,
        ts: Ts,
        name: String,
    },
    RemoveReaction {
        channel: ChannelId,
        ts: Ts,
        name: String,
    },
    /// Search with Slack's modifier syntax: local store results first, then Slack's.
    Search(String),
    /// Move the read cursor of a conversation to `ts`.
    MarkRead {
        channel: ChannelId,
        ts: Ts,
    },
    /// Open (or find) the DM with `user` (`conversations.open`), store it and announce it
    /// with [`Event::ConversationOpened`].
    OpenDm {
        user: UserId,
    },
    /// Create a sidebar channel section (`users.channelSections.create`), then re-read the
    /// sections and announce [`Event::SidebarChanged`].
    CreateSection {
        name: String,
    },
    /// Replace `channel`'s topic (`conversations.setTopic`), merge the returned channel
    /// into the store and announce it with [`Event::Conversations`].
    SetChannelTopic {
        channel: ChannelId,
        topic: String,
    },
    /// Replace `channel`'s description (`conversations.setPurpose`), merge the returned
    /// channel into the store and announce it with [`Event::Conversations`].
    SetChannelPurpose {
        channel: ChannelId,
        purpose: String,
    },
    /// Send a typing indicator.
    Typing {
        channel: ChannelId,
        thread_ts: Option<Ts>,
    },
    /// The user opened a conversation's Pins tab: fetch its pinned items (`pins.list`).
    /// Lazy by design: one call per conversation, when the tab is first opened.
    LoadPins(ChannelId),
    /// The user opened a bookmarks surface of a conversation (folder tab, bookmark bar):
    /// fetch its bookmarks (`bookmarks.list`), folders included.
    LoadBookmarks(ChannelId),
    /// The user opened a files surface: fetch its files (`files.list`). A `channel` in the
    /// filter asks for that conversation's shared files (the Files & links tab); without
    /// one Slack answers every file the signed-in user can see workspace-wide (the Files
    /// view).
    LoadFiles(api::FilesFilter),
    /// Add a link bookmark to the conversation's bar (`bookmarks.add`), then re-fetch the
    /// list so the bar shows the new entry.
    AddBookmark {
        channel: ChannelId,
        title: String,
        url: String,
    },
    /// Create the conversation's empty channel canvas (`conversations.canvases.create`),
    /// then refresh the stored conversation (its `properties.canvas` lands in the store).
    CreateChannelCanvas(ChannelId),
    /// Close the websocket, cancel background work and stop the runtime thread.
    Shutdown,
}

/// Events the core emits to a front-end.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Websocket lifecycle.
    Connection(ConnectionState),
    /// Messages of `channel` changed in the store; re-read it.
    StoreChanged { channel: ChannelId },
    /// Backfill reached the beginning of `channel`'s history; every message is in the store.
    HistoryComplete { channel: ChannelId },
    /// These conversations were added or updated in the store.
    Conversations(Vec<ChannelId>),
    /// An opened channel's `conversations.info` refresh (already merged into the store).
    /// Details only, so front-ends update the header without rebuilding conversation lists.
    /// `num_members` counts human members (`conversations.members` cross-checked against the
    /// stored profiles: apps, deactivated people and `USLACKBOT` excluded) and `num_apps`
    /// counts the app/bot members for the header's separate apps chip; when the member list
    /// cannot be read, `num_apps` is `None`, `num_members` falls back to
    /// `conversations.info`'s raw count and `members` is empty. Otherwise `members` carries
    /// the full member id list (member panels, Slack Connect org counts). When
    /// `conversations.info` itself fails, the event still lands carrying nothing (`None`
    /// counts, no members) so front-ends stop waiting; the failure is reported as usual.
    ConversationInfo {
        channel: ChannelId,
        num_members: Option<u32>,
        num_apps: Option<u32>,
        members: Vec<UserId>,
    },
    /// These users were added or updated in the store.
    UsersChanged(Vec<UserId>),
    /// Sidebar inputs (the team and its connected organizations, sections, custom emoji,
    /// prefs) changed in the store; re-read them.
    SidebarChanged,
    /// Unread and mention counts from `client.counts`.
    Counts(Box<api::ClientCounts>),
    /// The read cursor of `channel` moved to `ts` (possibly from another client).
    ReadMarker { channel: ChannelId, ts: Ts },
    /// Someone is typing (receivers expire it after ~5 s).
    Typing {
        channel: ChannelId,
        user: UserId,
        thread_ts: Option<Ts>,
    },
    Presence {
        users: Vec<UserId>,
        presence: Presence,
    },
    /// A user's notifications are paused (snooze or their do-not-disturb window); fetched
    /// when their DM opens. Ephemeral: front-ends keep it in memory, it is never stored.
    DndPaused { user: UserId, paused: bool },
    /// Local store matches for a [`Command::Search`], newest first.
    SearchResults {
        query: String,
        messages: Vec<StoredMessage>,
    },
    /// Slack's `search.messages` results for a [`Command::Search`].
    RemoteSearchResults {
        query: String,
        results: Box<api::SearchMessages>,
    },
    /// The followed threads of [`Command::FetchThreads`]
    /// (`subscriptions.thread.getView`).
    ThreadsView(Box<api::ThreadsView>),
    /// The answer to a [`Command::FetchActivity`] (`activity.feed`, hydrated through
    /// `messages.list` when asked): the Activity view's rows, or the error text the view
    /// shows instead of loading forever (the failure is also reported).
    ActivityView {
        feed: Result<Box<api::ActivityFeed>, String>,
    },
    /// The DM of a [`Command::OpenDm`] opened (or was already open): its conversation id.
    ConversationOpened(ChannelId),
    /// The session was rejected; the user has to sign in again. Carries Slack's error.
    AuthExpired(String),
    /// The answer to a [`Command::LoadPins`]: the conversation's pinned items, or the error
    /// text the Pins tab shows instead of loading forever (the failure is also reported).
    PinsLoaded {
        channel: ChannelId,
        pins: Result<Vec<api::PinnedItem>, String>,
    },
    /// The answer to a [`Command::LoadBookmarks`] or [`Command::AddBookmark`]: the
    /// conversation's bookmarks, or the error text.
    BookmarksLoaded {
        channel: ChannelId,
        bookmarks: Result<Vec<api::ChannelBookmark>, String>,
    },
    /// The answer to a [`Command::LoadFiles`]: the files matching its filter, or the error
    /// text. The filter rides along so a workspace-wide load (the Files view) and a
    /// per-conversation one (the Files & links tab) land in the right place.
    FilesLoaded {
        filter: api::FilesFilter,
        files: Result<Vec<File>, String>,
    },
    /// The conversation's channel canvas was created and the stored conversation refreshed;
    /// `canvas_id` opens it in Slack's web client.
    ChannelCanvasCreated {
        channel: ChannelId,
        canvas_id: String,
    },
    /// A non-fatal failure worth surfacing.
    Error(String),
}
