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
mod reducer;

pub use backend::Backend;
pub use client::{Client, ClientHandle, SharedStore};
pub use fetcher::Fetcher;
pub use reducer::apply_event;

use rtm::ConnectionState;
use store::StoredMessage;
use types::{ChannelId, Presence, Ts, UserId};

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
    /// [`Event::UsersChanged`].
    ResolveUsers(Vec<UserId>),
    /// The user opened a thread: fetch its full history (root + all replies) from Slack into
    /// the store. Read-only; it never moves the user's read cursor.
    OpenThread {
        channel: ChannelId,
        thread_ts: Ts,
    },
    /// Post a message; `thread_ts` replies in a thread.
    SendMessage {
        channel: ChannelId,
        text: String,
        thread_ts: Option<Ts>,
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
    /// Send a typing indicator.
    Typing {
        channel: ChannelId,
        thread_ts: Option<Ts>,
    },
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
    ConversationInfo {
        channel: ChannelId,
        num_members: Option<u32>,
    },
    /// These users were added or updated in the store.
    UsersChanged(Vec<UserId>),
    /// Sidebar inputs (team, sections, custom emoji, prefs) changed in the store; re-read them.
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
    /// The session was rejected; the user has to sign in again. Carries Slack's error.
    AuthExpired(String),
    /// A non-fatal failure worth surfacing.
    Error(String),
}
