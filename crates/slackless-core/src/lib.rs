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
pub use slackless_huddle as huddle;
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
use types::{BotId, ChannelId, File, Message, Presence, TeamId, Ts, UserId};

/// Commands a front-end sends into the core.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// The user opened a conversation: load its newest history into the store and keep
    /// backfilling older pages in the background until the beginning is reached.
    OpenConversation(ChannelId),
    /// Fetch a single page of recent history (`conversations.history`, limit 50) into the
    /// store and announce [`Event::StoreChanged`], without moving the read cursor or walking
    /// older pages. Used by the All Unreads view for unread channels with no stored rows yet.
    FetchLatestHistory(ChannelId),
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
    /// The user pressed "Join Channel" on a public channel they are not in: join it on Slack,
    /// store the joined channel and announce it with [`Event::Conversations`].
    JoinConversation(ChannelId),
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
    /// Replace a message's content (its blocks, with mrkdwn text as fallback).
    EditMessage {
        channel: ChannelId,
        ts: Ts,
        content: MessageContent,
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
    /// Pin or unpin a message in its conversation (`pins.add` / `pins.remove`), then
    /// re-read the pins so an open Pins tab shows the change.
    SetPinned {
        channel: ChannelId,
        ts: Ts,
        pinned: bool,
    },
    /// Run a slash command Slack handles (`chat.command`, e.g. `/giphy`) in `channel`, or in
    /// the thread at `thread_ts`. Its result arrives on the websocket; a refusal (a command
    /// the workspace does not have) is announced as [`Event::CommandFailed`].
    RunCommand {
        channel: ChannelId,
        /// With its slash: `/giphy`.
        command: String,
        args: String,
        thread_ts: Option<Ts>,
    },
    /// A Block Kit button pressed in a message: dispatch it to the app that posted the
    /// message (`blocks.actions`). The outcome arrives on the websocket (Giphy's preview
    /// replaced or deleted, its GIF posted).
    BlockAction {
        bot: BotId,
        app_id: String,
        /// The team that installed the app; the signed-in workspace when `None`.
        team: Option<TeamId>,
        /// The element as the message declared it, plus its `block_id`.
        action: serde_json::Value,
        container: api::ActionContainer,
    },
    /// Search with Slack's modifier syntax: local store results first, then Slack's.
    Search(String),
    /// Move the read cursor of a conversation to `ts`.
    MarkRead {
        channel: ChannelId,
        ts: Ts,
    },
    /// Move the read cursor of a thread to `ts` (`subscriptions.thread.mark`).
    MarkThreadRead {
        channel: ChannelId,
        thread_ts: Ts,
        ts: Ts,
    },
    /// Mark every conversation and thread read (`users.markAllRead`, Slack's Shift+Esc),
    /// then re-read the unread counts.
    MarkAllRead,
    /// Follow or unfollow a thread (`subscriptions.thread.add` / `.remove`), then read the
    /// thread again so the stored root carries the new state. `last_read` is the thread's
    /// read cursor.
    SetThreadSubscribed {
        channel: ChannelId,
        thread_ts: Ts,
        last_read: Ts,
        subscribed: bool,
    },
    /// Read a channel's managers (`admin.roles.entity.listAssignments`) for its details
    /// dialog; answered with [`Event::ChannelManagers`].
    FetchChannelManagers {
        channel: ChannelId,
    },
    /// Open (or find) the DM or group DM with `users` (`conversations.open`), store it and
    /// announce it with [`Event::ConversationOpened`].
    OpenDm {
        users: Vec<UserId>,
    },
    /// Create a sidebar channel section (`users.channelSections.create`), then re-read the
    /// sections and announce [`Event::SidebarChanged`].
    CreateSection {
        name: String,
    },
    /// Create a sidebar channel section and move `channel` into it in the same round
    /// (Slack's row-menu "+ Create new section"): `users.channelSections.create`, then a
    /// `users.channelSections.channels.bulkUpdate` insert into the fresh id, then the
    /// section re-read and [`Event::SidebarChanged`].
    CreateSectionWithChannel {
        name: String,
        channel: ChannelId,
    },
    /// Rename a sidebar section (`users.channelSections.set`). The section's emoji is
    /// re-read from the store and resent, because the call clears every omitted key; then
    /// the sections are re-read and [`Event::SidebarChanged`] announced.
    RenameSection {
        section: String,
        name: String,
    },
    /// Delete a sidebar section (`users.channelSections.delete`), then re-read the sections
    /// and announce [`Event::SidebarChanged`]. Its conversations fall back to their default
    /// buckets.
    DeleteSection {
        section: String,
    },
    /// Move `channel` between sidebar sections (`users.channelSections.channels.bulkUpdate`,
    /// one call carrying the insert into `to` and the remove from `from`); `None` on either
    /// side is the unsectioned default bucket. Then the sections are re-read and
    /// [`Event::SidebarChanged`] announced.
    MoveChannel {
        channel: ChannelId,
        from: Option<String>,
        to: Option<String>,
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
    /// Create a public or private channel (`conversations.create`), optionally set its
    /// description (`conversations.setPurpose`), store it, and open it via
    /// [`Event::ConversationOpened`].
    CreateChannel {
        name: String,
        is_private: bool,
        purpose: Option<String>,
    },
    /// Rename `channel` (`conversations.rename`), store the updated record and announce
    /// [`Event::Conversations`].
    RenameChannel {
        channel: ChannelId,
        name: String,
    },
    /// Archive `channel` (`conversations.archive`), update the stored record and announce
    /// [`Event::Conversations`].
    ArchiveChannel(ChannelId),
    /// Unarchive `channel` (`conversations.unarchive`), update the stored record and announce
    /// [`Event::Conversations`].
    UnarchiveChannel(ChannelId),
    /// Invite `users` into `channel` (`conversations.invite`), then refresh channel members.
    InviteToChannel {
        channel: ChannelId,
        users: Vec<UserId>,
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
    /// Pause the signed-in user's notifications for `minutes` (`dnd.setSnooze`).
    SetDndSnooze {
        minutes: u32,
    },
    /// Resume the signed-in user's notifications immediately (`dnd.endSnooze`).
    EndDndSnooze,
    /// End the signed-in user's active DND session (`dnd.endDnd`, `F-SHELL-034`).
    EndDnd,
    /// Batch-fetch DND status for a list of users (`dnd.teamInfo`, `F-SHELL-035`).
    LoadTeamDnd(Vec<UserId>),
    /// Queue a message for delivery at unix timestamp `post_at` (`chat.scheduleMessage`),
    /// optionally inside a thread (`thread_ts`), then reload scheduled messages.
    ScheduleMessage {
        channel: ChannelId,
        post_at: u64,
        content: MessageContent,
        thread_ts: Option<Ts>,
    },
    /// Fetch pending scheduled messages (`chat.scheduledMessages.list`).
    LoadScheduledMessages,
    /// Cancel a pending scheduled message (`chat.deleteScheduledMessage`), then reload.
    DeleteScheduledMessage {
        channel: ChannelId,
        scheduled_message_id: String,
    },
    /// Upload one or more files into `channel` (optionally in `thread_ts` with `content`)
    /// through Slack's external upload v2 flow (`files.getUploadURLExternal` -> raw POST ->
    /// `files.completeUploadExternal`).
    UploadFiles {
        channel: ChannelId,
        thread_ts: Option<Ts>,
        content: Option<MessageContent>,
        files: Vec<api::UploadFile>,
    },
    /// Fetch GIFs for the composer picker: featured/trending (`gif.featured`) when `query`
    /// is empty, or search (`gif.search`) when non-empty.
    SearchGifs {
        query: String,
    },
    /// Fetch all Later / Saved items (`saved.list` across in-progress, completed, archived)
    /// and hydrate their message bodies via `messages.list`.
    LoadSaved,
    /// Save a message for Later (`saved.add`), optionally with a reminder due time (`date_due`).
    SaveItem {
        channel: ChannelId,
        ts: Ts,
        date_due: Option<u64>,
    },
    /// Save a file for Later (`saved.add` with `item_type=file`, `F-CORE-157`).
    SaveFile {
        file_id: String,
        date_due: Option<u64>,
    },
    /// Permanently delete a user-owned file/clip (`files.delete`, `F-CORE-130`).
    DeleteFile {
        file_id: String,
    },
    /// Fetch a live Slack canvas's read-only HTML export (`files.info` + `url_private`, `F-COLLAB-125`).
    LoadCanvas {
        file_id: String,
    },
    /// Create a standalone (or channel-tabbed) canvas (`canvases.create`, `F-COLLAB-119`).
    CreateCanvas {
        title: String,
        markdown: String,
        channel: Option<ChannelId>,
    },
    /// Apply a section-level edit to a canvas (`canvases.edit`, `F-COLLAB-132`, `F-SHELL-264`, `F-SHELL-277`).
    EditCanvas {
        file_id: String,
        op: api::CanvasEditOp,
    },
    /// Request edit (`upgrade`) or comment (`comment_upgrade`) access on a canvas/list (`F-COLLAB-129`).
    RequestFilePermission {
        file_id: String,
        request_type: String,
    },
    /// Fetch a live Slack List's schema and records (`files.info` + `lists.records.list`).
    LoadList {
        file_id: String,
    },
    /// Create a new Slack List (`lists.create`, `F-COLLAB-213`).
    CreateList {
        name: String,
        description: Option<String>,
        channel: Option<ChannelId>,
    },
    /// Add an item to a Slack List (`lists.records.create`, `F-COLLAB-226`).
    CreateListItem {
        list_id: String,
        initial_fields: Vec<api::ListCellInput>,
    },
    /// Update one or more cells of a Slack List item (`lists.cells.update`, `F-COLLAB-227`, `F-COLLAB-248`).
    UpdateListCell {
        list_id: String,
        row_id: String,
        cells: Vec<api::ListCellInput>,
    },
    /// Delete an item from a Slack List (`lists.records.delete`, `F-COLLAB-228`).
    DeleteListItem {
        list_id: String,
        row_id: String,
    },
    /// Add a Slack message to an existing List (`lists.addToList`, `F-COLLAB-231`).
    AddMessageToList {
        list_id: String,
        channel: ChannelId,
        ts: Ts,
    },
    /// Fetch an app DM's Home tab view (`conversations.info` `return_app_home=true` +
    /// `apps.home.dispatchOpenEvent`, `F-COLLAB-480`).
    LoadAppHome(ChannelId),
    /// Resolve and open an app's DM/Home conversation (`apps.home.open`, `F-COLLAB-481`, `F-COLLAB-523`).
    OpenAppHome {
        app_id: String,
        team_id: String,
    },
    /// Trigger a Workflow Builder link trigger natively (`workflows.triggers.trip`, `F-COLLAB-319`, `F-COLLAB-376`).
    TripWorkflowTrigger {
        url: String,
        context: serde_json::Value,
    },
    /// Write a global notification preference (`users.prefs.setNotifications` with `global=true`; `F-CORE-147`, `F-CORE-155`).
    SetGlobalNotificationPref {
        name: String,
        value: String,
        sync: bool,
    },
    /// Load starred files, custom file collections, and recently deleted canvases/lists (`F-CORE-133`).
    LoadFilesExtras,
    /// Star/unstar a file or add/remove it from a collection (`files.favorites.add`/`.remove`, `F-CORE-133`).
    SetFileFavorite {
        file_id: String,
        collection_id: Option<String>,
        starred: bool,
    },
    /// Create a custom Files sidebar collection (`files.collections.create`, `F-CORE-133`).
    CreateFileCollection {
        name: String,
        emoji: Option<String>,
    },
    /// Delete a custom Files sidebar collection (`files.collections.delete`, `F-CORE-133`).
    DeleteFileCollection {
        collection_id: String,
    },
    /// Restore a recently deleted canvas or list (`files.undelete`, `F-CORE-133`).
    RestoreDeletedFile {
        file_id: String,
    },
    /// Update a Later item's state (`mark`) and/or reminder due time (`date_due`, `Some(None)` clears).
    UpdateSavedItem {
        channel: ChannelId,
        ts: Ts,
        mark: Option<api::SavedMark>,
        date_due: Option<Option<u64>>,
    },
    /// Remove a message from Later (`saved.delete`).
    DeleteSavedItem {
        channel: ChannelId,
        ts: Ts,
    },
    /// Clear all completed Later items (`saved.clearCompleted`).
    ClearCompletedSaved,
    /// Fast remote typeahead for Cmd+K Quick Switcher (`edgeapi` `users/search` + `channels/search`).
    QuickSwitchSearch {
        query: String,
    },
    /// Opt-in remote Slack message search (`search.modules.messages`).
    SearchRemoteMessages {
        query: String,
        sort: api::SearchSort,
        page: usize,
    },
    /// Opt-in remote Slack file search (`search.modules.files`).
    SearchRemoteFiles {
        query: String,
        sort: api::SearchSort,
        page: usize,
    },
    /// Update the signed-in user's profile or custom status (`users.profile.set`).
    UpdateProfile {
        update: api::ProfileUpdate,
    },
    /// Manually set presence active or away (`presence.set`).
    SetPresence {
        away: bool,
    },
    /// Write a single user preference key/value back to Slack (`users.prefs.set`).
    SetUserPref {
        name: String,
        value: String,
    },
    /// Fetch one user's full profile including custom fields & pronouns (`users.profile.get` + `team.profile.get`).
    FetchUserProfile(UserId),
    /// Submit an interactive Block Kit modal (`views.submit`).
    SubmitModal {
        view_id: String,
        hash: Option<String>,
        state_values: serde_json::Value,
    },
    /// Add a link bookmark to a folder inside `channel` (`bookmarks.add` with `parent_id`).
    AddBookmarkInFolder {
        channel: ChannelId,
        title: String,
        url: String,
        parent_id: String,
    },
    /// Remove a bookmark from `channel` (`bookmarks.remove`, `F-CORE-062`).
    RemoveBookmark {
        channel: ChannelId,
        bookmark_id: String,
    },
    /// Sync a conversation draft to Slack (`drafts.create` / `drafts.update` / `drafts.delete`, `F-CORE-089`).
    SyncDraft {
        channel: ChannelId,
        text: String,
    },
    /// Fetch the signed-in user's cloud-synced drafts (`drafts.list`).
    FetchCloudDrafts,
    /// Fetch dynamic option suggestions for an `external_select` element (`blocks.suggestions`).
    BlockSuggestions {
        key: String,
        bot: BotId,
        app_id: String,
        team: TeamId,
        action_id: String,
        block_id: String,
        value: String,
        container: api::ActionContainer,
    },
    /// Dismiss an interactive Block Kit modal (`views.close`).
    CloseModal {
        view_id: String,
        hash: Option<String>,
    },
    /// Retry a persisted offline mutation that previously failed to flush.
    RetryOfflineMutation(i64),
    /// Discard a persisted offline mutation from SQLite.
    DiscardOfflineMutation(i64),
    /// Revoke the workspace token server-side (`auth.revoke`, `F-CORE-009`).
    SignOut,
    /// Add a conversation tab (`conversations.addTab`, `F-CORE-061`) and refresh channel info.
    AddConversationTab {
        channel: ChannelId,
        tab_type: String,
        label: Option<String>,
    },
    /// Remove a conversation tab (`conversations.removeTab`, `F-CORE-061`) and refresh channel info.
    RemoveConversationTab {
        channel: ChannelId,
        tab_id: String,
    },
    /// Reorder a conversation tab (`conversations.moveTab`, `F-CORE-061`) and refresh channel info.
    MoveConversationTab {
        channel: ChannelId,
        tab_id: String,
        after: Option<String>,
    },
    /// Start or join a native audio huddle in `channel` (`rooms.join` + Amazon Chime SFU).
    JoinHuddle {
        channel: ChannelId,
        audio: huddle::HuddleAudioConfig,
    },
    /// Leave the currently active native huddle cleanly (sends Chime `LEAVE` and releases audio).
    LeaveHuddle,
    /// Toggle local microphone mute in the active huddle.
    SetHuddleMuted(bool),
    /// Switch microphone and/or speaker in the active huddle without dropping the call.
    SetHuddleAudioDevices {
        input: Option<String>,
        output: Option<String>,
    },
    /// Toggle pure-Rust Sonora AEC3 / noise suppression / AGC2 live in the active huddle.
    SetHuddleAudioProcessing {
        aec: bool,
        ns: bool,
        agc: bool,
    },
    /// Ring/invite workspace members into the active huddle (`rooms.notifyMember`).
    InviteToHuddle {
        channel: ChannelId,
        users: Vec<UserId>,
    },
    /// Fetch the shareable huddle link for `channel` (`rooms.getLink`).
    CopyHuddleLink {
        channel: ChannelId,
    },
    /// Play a short two-note speaker test chime on `output_device` (Preferences -> Audio & video).
    TestHuddleSpeaker {
        output_device: Option<String>,
    },
    /// Enable or disable local webcam H.264 video in the active huddle (Phase 2).
    SetHuddleCamera {
        enabled: bool,
        device_index: Option<usize>,
        blur: bool,
    },
    /// Toggle software portrait background blur on the active local camera stream.
    SetHuddleBlurBackground(bool),
    /// Share a local monitor/window (`Some(source_id)`) or stop screen sharing (`None`) (Phase 3).
    SetHuddleScreenShare {
        source_id: Option<String>,
    },
    /// Broadcast a floating emoji reaction, burst effect, or GIF sticker over official Slack
    /// `PeerMessage.Reacji` (`data-channel`).
    SendHuddleReaction {
        emoji: String,
        kind: huddle::HuddleReactionKind,
        emoji_url: Option<String>,
    },
    /// Raise or lower the local participant's hand (`PeerMessage.AwarenessBroadcast` `raised_hand`).
    SetHuddleHandRaised(bool),
    /// Set or clear a persistent sticky status emoji (`PeerMessage.AwarenessBroadcast`).
    SetHuddleStickyEmoji(Option<String>),
    /// Broadcast a screen-share drawing stroke or live cursor position (`PeerMessage.Draw`).
    SendHuddleDraw(huddle::HuddleDrawMessage),
    /// Start (`Some(song_key)`) or stop (`None`) ambient/jukebox huddle music (`PeerMessage.JukeboxRequest`).
    SetHuddleJukebox(Option<String>),
    /// Request a remote huddle peer to mute their microphone (`PeerMessage.MuteRequest`).
    MuteHuddlePeer(String),
    /// Set or clear the editable huddle topic (`screenhero.rooms.rename`, `F-COLLAB-016`).
    SetHuddleTopic(String),
    /// Update the active huddle's visual theme/background (`huddles.update` + `PeerMessage.Background`).
    SetHuddleBackground(String),
    /// Enable or disable live AWS Transcribe captions (`rooms.startTranscription` / `rooms.stopTranscription`).
    SetHuddleCaptions(bool),
    /// Decline an incoming direct-message huddle invite (`rooms.inviteResponse`).
    DeclineHuddleInvite {
        channel: ChannelId,
        call_id: String,
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
    /// The managers of `channel` ([`Command::FetchChannelManagers`]).
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
    /// A user's notifications are paused (snooze or their do-not-disturb window); fetched
    /// when their DM opens. Ephemeral: front-ends keep it in memory, it is never stored.
    DndPaused { user: UserId, paused: bool },
    /// Batch DND status results from `dnd.teamInfo` (`F-SHELL-035`).
    TeamDndLoaded(std::collections::HashMap<UserId, bool>),
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
    /// The signed-in user's DND/snooze status after `dnd.setSnooze` / `dnd.endSnooze`.
    SelfDndUpdated(types::DndStatus),
    /// Pending scheduled messages from `chat.scheduledMessages.list`.
    ScheduledMessagesLoaded(Result<Vec<api::ScheduledMessage>, String>),
    /// A message was queued via `chat.scheduleMessage`.
    MessageScheduled {
        channel: ChannelId,
        scheduled_message_id: String,
        post_at: u64,
    },
    /// Slack refused a [`Command::RunCommand`]: the command and Slack's error code
    /// (`unknown_command` for one the workspace does not have).
    CommandFailed { command: String, error: String },
    /// One or more files finished uploading and sharing into `channel`.
    FilesUploaded { channel: ChannelId, count: usize },
    /// Active `@handle` user groups from `usergroups.list`.
    Usergroups(Vec<types::Usergroup>),
    /// Featured or searched GIFs for the composer picker (`gif.featured` / `gif.search`).
    GifResults {
        query: String,
        gifs: Vec<api::GifItem>,
    },
    /// Hydrated Later / Saved items from `saved.list` + `messages.list`.
    SavedLoaded(Result<Vec<api::SavedItem>, String>),
    /// Workspace slash commands from `client.appCommands` (merged with built-ins in `/` menu).
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
    /// Live huddle state update for a conversation (`sh_room_join` / `sh_room_update` / `sh_room_leave`).
    HuddleUpdated {
        channel: ChannelId,
        participants: Vec<UserId>,
        ended: bool,
        topic: Option<String>,
        background_id: Option<String>,
    },
    /// Active native huddle session lifecycle snapshot (`Connecting`, `Connected`, `Reconnecting`, `Failed`, `Idle`).
    HuddleSessionChanged(Box<huddle::HuddleSessionSnapshot>),
    /// Live local RMS level and remote active-speaker / mute states from the active Chime session.
    HuddleAudioLevels(huddle::HuddleAudioLevels),
    /// Incoming direct-message huddle ring (`huddle_invite`, `F-COLLAB-031`).
    HuddleInvite {
        channel: ChannelId,
        inviter: UserId,
        call_id: String,
    },
    /// Incoming direct-message huddle invite was cancelled or timed out (`huddle_invite_cancel`).
    HuddleInviteCancelled {
        channel: Option<ChannelId>,
        call_id: Option<String>,
    },
    /// Shareable huddle join link fetched via `rooms.getLink` (`F-COLLAB-039`).
    HuddleLinkLoaded { channel: ChannelId, url: String },
    /// Decoded BGRA8888 video frame for a local/remote camera or screen-share tile (Phases 2–3).
    HuddleVideoFrame {
        tile: huddle::VideoTileKey,
        frame: huddle::VideoFrame,
    },
    /// A camera or screen-share video tile stopped publishing and should be removed.
    HuddleVideoTileClosed(huddle::VideoTileKey),
    /// Ephemeral collaboration event over Chime `DATA_MESSAGE` (reaction, hand raise, topic, caption).
    HuddleDataEvent(huddle::HuddleDataEvent),
    /// Hydrated custom profile fields and pronouns for `user` (`users.profile.get` + `team.profile.get`).
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
    /// A queued offline mutation failed while flushing on reconnect; stays persisted until
    /// retried via [`Command::RetryOfflineMutation`] or discarded via [`Command::DiscardOfflineMutation`].
    OfflineMutationFailed(FailedMutation),
    /// A live Slack canvas HTML export finished loading (`F-COLLAB-125`).
    CanvasLoaded {
        file_id: String,
        title: String,
        html: Result<String, String>,
    },
    /// A standalone canvas was created via `canvases.create` (`F-COLLAB-119`).
    CanvasCreated {
        file_id: String,
        title: String,
        channel: Option<ChannelId>,
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
    /// An app DM's Home tab finished loading (`conversations.info` `return_app_home=true`, `F-COLLAB-480`).
    AppHomeLoaded {
        channel: ChannelId,
        home: Result<Box<api::AppHomeResponse>, String>,
    },
    /// An app published a fresh Home tab view (`view_updated` with `type: "home"`, `F-COLLAB-480`).
    AppHomeUpdated(serde_json::Value),
    /// A Workflow Builder link trigger was tripped via `workflows.triggers.trip` (`F-COLLAB-319`, `F-COLLAB-376`).
    WorkflowTripped { url: String },
    /// Starred files, custom collections, and recently deleted items for the Files browser (`F-CORE-133`).
    FilesExtrasLoaded {
        favorites: Vec<String>,
        collections: Vec<api::FileCollection>,
        deleted: Vec<api::DeletedFile>,
    },
    /// A recently deleted canvas/list was restored (`files.undelete`, `F-CORE-133`).
    FileRestored { file_id: String },
    /// A user-owned file was deleted via `files.delete` (`F-CORE-130`).
    FileDeleted { file_id: String },
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
