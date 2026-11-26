//! Events emitted from `slackless-core` to front-ends.
//!
//! Critical control and store-sync notifications travel on the ordered [`Event`] stream, while
//! high-frequency native huddle media frames (60fps video, audio meters) travel on a separate
//! bounded [`HuddleMediaEvent`] channel using non-blocking `try_send` so slow consumers drop
//! stale frames instead of stalling or backing up control events.

use crate::api;
use crate::huddle;
use crate::rtm::ConnectionState;
use crate::store::StoredMessage;
use crate::types::{BotId, ChannelId, File, Message, Presence, Ts, UserId, UsergroupId};

/// Bounded capacity of the dedicated high-frequency huddle media lane.
pub const HUDDLE_MEDIA_CHANNEL_CAPACITY: usize = 64;

/// High-frequency huddle media events delivered on a dedicated bounded channel (`try_send`),
/// completely separated from the ordered [`Event`] stream.
#[derive(Debug, Clone, PartialEq)]
pub enum HuddleMediaEvent {
    /// Decoded BGRA8888 video frame for a local/remote camera or screen-share tile.
    VideoFrame {
        tile: huddle::VideoTileKey,
        frame: huddle::VideoFrame,
    },
    /// A camera or screen-share video tile stopped publishing and should be removed.
    VideoTileClosed(huddle::VideoTileKey),
    /// Live local RMS level and remote active-speaker / mute states from the active Chime session.
    AudioLevels(huddle::HuddleAudioLevels),
}

/// Low-frequency huddle control, lifecycle, invite, and data-channel events delivered on [`Event`].
#[derive(Debug, Clone, PartialEq)]
pub enum HuddleEvent {
    /// Live huddle state update for a conversation (`sh_room_join` / `sh_room_update` / `sh_room_leave`).
    Updated {
        channel: ChannelId,
        participants: Vec<UserId>,
        ended: bool,
        topic: Option<String>,
        background_id: Option<String>,
        thread_root_ts: Option<Ts>,
    },
    /// Active native huddle session lifecycle snapshot (`Connecting`, `Connected`, `Reconnecting`, `Failed`, `Idle`).
    SessionChanged(Box<huddle::HuddleSessionSnapshot>),
    /// Incoming direct-message huddle ring (`huddle_invite`, `F-COLLAB-031`).
    Invite {
        channel: ChannelId,
        inviter: UserId,
        call_id: String,
    },
    /// Incoming direct-message huddle invite was cancelled or timed out (`huddle_invite_cancel`).
    InviteCancelled {
        channel: Option<ChannelId>,
        call_id: Option<String>,
    },
    /// Shareable huddle join link fetched via `rooms.getLink` (`F-COLLAB-039`).
    LinkLoaded { channel: ChannelId, url: String },
    /// Ephemeral collaboration event over Chime `DATA_MESSAGE` (reaction, hand raise, topic, caption).
    Data(huddle::HuddleDataEvent),
}

impl From<HuddleEvent> for Event {
    fn from(event: HuddleEvent) -> Self {
        Self::Huddle(event)
    }
}

/// Ordered control and store-sync events the core emits to a front-end.
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
    ConversationInfo {
        channel: ChannelId,
        num_members: Option<u32>,
        num_apps: Option<u32>,
        members: Vec<UserId>,
    },
    /// The managers of `channel` (`ChannelCommand::FetchChannelManagers`).
    ChannelManagers {
        channel: ChannelId,
        users: Vec<UserId>,
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
    /// The read cursor of a thread moved (`thread_marked`, possibly from another client).
    ThreadReadMarker {
        channel: ChannelId,
        thread_ts: Ts,
        last_read: Option<Ts>,
    },
    /// A message was just posted (live, over the websocket; not history or edits), after
    /// [`Event::StoreChanged`] for its channel: what notifications are decided from.
    MessagePosted {
        channel: ChannelId,
        message: Box<Message>,
    },
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
    /// A user's notifications are paused (snooze or their do-not-disturb window).
    DndPaused { user: UserId, paused: bool },
    /// Batch DND status results from `dnd.teamInfo` (`F-SHELL-035`).
    TeamDndLoaded(std::collections::HashMap<UserId, bool>),
    /// Local store matches for a `CollabCommand::Search`, newest first.
    SearchResults {
        query: String,
        messages: Vec<StoredMessage>,
    },
    /// Slack's `search.messages` results for a `CollabCommand::Search`.
    RemoteSearchResults {
        query: String,
        results: Box<api::SearchMessages>,
    },
    /// The followed threads of `MessagingCommand::FetchThreads`.
    ThreadsView(Box<api::ThreadsView>),
    /// The answer to `MessagingCommand::FetchActivity`.
    ActivityView {
        feed: Result<Box<api::ActivityFeed>, String>,
    },
    /// The DM of `ChannelCommand::OpenDm` opened (or was already open): its conversation id.
    ConversationOpened(ChannelId),
    /// The session was rejected; the user has to sign in again. Carries Slack's error.
    AuthExpired(String),
    /// The answer to `MessagingCommand::LoadPins`.
    PinsLoaded {
        channel: ChannelId,
        pins: Result<Vec<api::PinnedItem>, String>,
    },
    /// The answer to `MessagingCommand::LoadBookmarks` or `AddBookmark`.
    BookmarksLoaded {
        channel: ChannelId,
        bookmarks: Result<Vec<api::ChannelBookmark>, String>,
    },
    /// The answer to `CollabCommand::LoadFiles`.
    FilesLoaded {
        filter: api::FilesFilter,
        files: Result<Vec<File>, String>,
    },
    /// The conversation's channel canvas was created and the stored conversation refreshed.
    ChannelCanvasCreated {
        channel: ChannelId,
        canvas_id: String,
    },
    /// The signed-in user's DND/snooze status after `dnd.setSnooze` / `dnd.endSnooze`.
    SelfDndUpdated(crate::types::DndStatus),
    /// Pending scheduled messages from `chat.scheduledMessages.list`.
    ScheduledMessagesLoaded(Result<Vec<api::ScheduledMessage>, String>),
    /// A message was queued via `chat.scheduleMessage`.
    MessageScheduled {
        channel: ChannelId,
        scheduled_message_id: String,
        post_at: u64,
    },
    /// Slack refused a `MessagingCommand::RunCommand`.
    CommandFailed { command: String, error: String },
    /// One or more files finished uploading and sharing into `channel`.
    FilesUploaded { channel: ChannelId, count: usize },
    /// Active `@handle` user groups from `usergroups.list`.
    Usergroups(Vec<crate::types::Usergroup>),
    /// Member user IDs of a `@handle` user group (`usergroups.users.list` / `usergroups.users.update`).
    UsergroupUsersLoaded {
        usergroup: UsergroupId,
        users: Vec<UserId>,
    },
    /// Featured or searched GIFs for the composer picker.
    GifResults {
        query: String,
        gifs: Vec<api::GifItem>,
    },
    /// Hydrated Later / Saved items from `saved.list` + `messages.list`.
    SavedLoaded(Result<Vec<api::SavedItem>, String>),
    /// Workspace slash commands from `client.appCommands`.
    AppCommands(Vec<api::AppCommand>),
    /// Remote people and channels matching a Cmd+K Quick Switcher query (`edgeapi`).
    QuickSwitchResults {
        query: String,
        results: Box<api::EdgeSearchResults>,
    },
    /// Opt-in remote message search page (`search.modules.messages`).
    RemoteMessagesPage {
        query: String,
        sort: api::SearchSort,
        page: usize,
        result: Result<Box<api::RemoteMessagePage>, String>,
    },
    /// Opt-in remote file search page (`search.modules.files`).
    RemoteFilesPage {
        query: String,
        sort: api::SearchSort,
        page: usize,
        result: Result<Box<api::RemoteFilePage>, String>,
    },
    /// Low-frequency huddle control, lifecycle, invite, and data-channel events.
    Huddle(HuddleEvent),
    /// Hydrated custom profile fields and pronouns for `user`.
    UserProfileLoaded {
        user: UserId,
        pronouns: Option<String>,
        fields: Vec<(String, String)>,
    },
    /// An app opened or updated an interactive Block Kit modal (`view_opened` / `view_updated`).
    ModalOpened(serde_json::Value),
    /// An app closed an interactive Block Kit modal (`view_closed`).
    ModalClosed(String),
    /// Cloud-synced drafts fetched via `drafts.list` (`F-CORE-089`).
    CloudDrafts(Vec<api::CloudDraft>),
    /// Dynamic options returned by `blocks.suggestions` (`F-COLLAB-786`).
    BlockSuggestions {
        key: String,
        options: Vec<(String, String)>,
    },
    /// Number of outgoing commands buffered while offline, waiting to flush on reconnect.
    OfflineQueueChanged(usize),
    /// A queued offline mutation failed while flushing on reconnect.
    OfflineMutationFailed(FailedMutation),
    /// A live Slack canvas HTML export finished loading (`F-COLLAB-125`).
    CanvasLoaded {
        file_id: String,
        title: String,
        html: Result<String, String>,
    },
    /// Parsed Canvas metadata loaded alongside [`Event::CanvasLoaded`].
    CanvasMetaLoaded {
        file_id: String,
        meta: api::CanvasMeta,
    },
    /// A standalone canvas was created via `canvases.create` (`F-COLLAB-119`).
    CanvasCreated {
        file_id: String,
        title: String,
        channel: Option<ChannelId>,
    },
    /// Result of `CollabCommand::DeleteCanvas` (`canvases.delete`).
    CanvasDeleted {
        canvas_id: String,
        result: Result<(), String>,
    },
    /// Result of `CollabCommand::SetCanvasAccess` or `DeleteCanvasAccess`.
    CanvasAccessUpdated {
        canvas_id: String,
        summary: String,
        result: Result<(), String>,
    },
    /// Result of `CollabCommand::LoadCanvasComments`.
    CanvasCommentsLoaded {
        canvas_id: String,
        result: Result<Vec<api::CanvasCommentThread>, String>,
    },
    /// Result of `CollabCommand::PostCanvasComment`.
    CanvasCommentPosted {
        canvas_id: String,
        result: Result<(), String>,
    },
    /// Result of `CollabCommand::ResolveCanvasComment` or `ResolveAllCanvasComments`.
    CanvasCommentResolved {
        canvas_id: String,
        thread_ts: Ts,
        resolved: bool,
        result: Result<(), String>,
    },
    /// Access was requested on a canvas/list (`files.requestPermission`, `F-COLLAB-129`).
    FilePermissionRequested { file_id: String },
    /// A live Slack List finished loading (`files.info` + `lists.records.list`).
    ListLoaded {
        file_id: String,
        list: Result<Box<api::SlackListData>, String>,
    },
    /// A new Slack List was created via `lists.create` (`F-COLLAB-213`).
    ListCreated { file_id: String, name: String },
    /// A message was added to a Slack List (`lists.addToList`, `F-COLLAB-231`).
    MessageAddedToList { list_id: String, record_id: String },
    /// Comment thread messages loaded for a Slack List item (`lists.getThreadsForRecord`).
    ListRecordComments {
        list_id: String,
        record_id: String,
        messages: Vec<Message>,
    },
    /// A bot profile loaded via `bots.info` for messages that only carried `bot_id`.
    BotProfileLoaded {
        bot_id: BotId,
        profile: crate::types::BotProfile,
    },
    /// An app DM's Home tab finished loading (`conversations.info` `return_app_home=true`).
    AppHomeLoaded {
        channel: ChannelId,
        home: Result<Box<api::AppHomeResponse>, String>,
    },
    /// An app published a fresh Home tab view (`view_updated` with `type: "home"`).
    AppHomeUpdated(serde_json::Value),
    /// A Workflow Builder link trigger was tripped via `workflows.triggers.trip`.
    WorkflowTripped { url: String },
    /// Starred files, custom collections, and recently deleted items for the Files browser.
    FilesExtrasLoaded {
        favorites: Vec<String>,
        collections: Vec<api::FileCollection>,
        deleted: Vec<api::DeletedFile>,
    },
    /// A recently deleted canvas/list was restored (`files.undelete`, `F-CORE-133`).
    FileRestored { file_id: String },
    /// A user-owned file was deleted via `files.delete` (`F-CORE-130`).
    FileDeleted { file_id: String },
    /// A video or audio file finished local transcription and was indexed in `message_fts`.
    FileTranscriptReady {
        file_id: crate::types::FileId,
        text: String,
    },
    /// Local transcription of a video or audio file failed.
    FileTranscriptFailed {
        file_id: crate::types::FileId,
        error: String,
    },
    /// Live per-channel & per-workspace archive backfill progress update.
    BackfillProgress(crate::backfill::WorkspaceBackfillProgress),
    /// A non-fatal failure worth surfacing.
    Error(String),
}

/// An offline-queued mutation that failed to flush on reconnect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedMutation {
    pub id: i64,
    pub summary: String,
    pub error: String,
}
