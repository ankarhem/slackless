//! Domain-grouped commands sent from front-ends into `slackless-core`.

use crate::api::{self, MessageContent};
use crate::huddle;
use crate::notify::NotifyLevel;
use crate::types::{BotId, ChannelId, TeamId, Ts, UserId};

/// Commands a front-end sends into the core, grouped by domain.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Messages, reactions, pins, bookmarks, scheduled messages, drafts, read cursors, threads,
    /// activity, saved/later items, slash commands, and offline mutation controls.
    Messaging(MessagingCommand),
    /// Conversations, memberships, sidebar sections, channel metadata, and conversation tabs.
    Channel(ChannelCommand),
    /// Search, files, canvases, Slack Lists, app home, workflows, Block Kit modals/actions.
    Collab(CollabCommand),
    /// Native audio/video huddle lifecycle, REST calls, and media session controls.
    Huddle(HuddleCommand),
    /// Workspace-wide user/bot directory, usergroups, DND, profile, presence, prefs, auth.
    Workspace(WorkspaceCommand),
    /// Close the websocket, cancel background work and stop the runtime thread.
    Shutdown,
}

impl From<MessagingCommand> for Command {
    fn from(cmd: MessagingCommand) -> Self {
        Self::Messaging(cmd)
    }
}

impl From<ChannelCommand> for Command {
    fn from(cmd: ChannelCommand) -> Self {
        Self::Channel(cmd)
    }
}

impl From<CollabCommand> for Command {
    fn from(cmd: CollabCommand) -> Self {
        Self::Collab(cmd)
    }
}

impl From<HuddleCommand> for Command {
    fn from(cmd: HuddleCommand) -> Self {
        Self::Huddle(cmd)
    }
}

impl From<HuddleControlCommand> for Command {
    fn from(cmd: HuddleControlCommand) -> Self {
        Self::Huddle(HuddleCommand::Control(cmd))
    }
}

impl From<WorkspaceCommand> for Command {
    fn from(cmd: WorkspaceCommand) -> Self {
        Self::Workspace(cmd)
    }
}

/// Messaging, reactions, pins, bookmarks, scheduled messages, drafts, read markers, threads,
/// activity feed, Later/saved items, slash commands, and offline mutation queue controls.
#[derive(Debug, Clone, PartialEq)]
pub enum MessagingCommand {
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
    /// The user opened a conversation's Pins tab: fetch its pinned items (`pins.list`).
    LoadPins(ChannelId),
    /// The user opened a bookmarks surface of a conversation: fetch its bookmarks (`bookmarks.list`).
    LoadBookmarks(ChannelId),
    /// Add a link bookmark to the conversation's bar (`bookmarks.add`), then re-fetch the list.
    AddBookmark {
        channel: ChannelId,
        title: String,
        url: String,
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
    /// Queue a message for delivery at unix timestamp `post_at` (`chat.scheduleMessage`).
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
    /// Sync a conversation draft to Slack (`drafts.create` / `drafts.update` / `drafts.delete`).
    SyncDraft {
        channel: ChannelId,
        text: String,
    },
    /// Fetch the signed-in user's cloud-synced drafts (`drafts.list`).
    FetchCloudDrafts,
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
    /// Mark every conversation and thread read (`users.markAllRead`, Slack's Shift+Esc).
    MarkAllRead,
    /// Send a typing indicator.
    Typing {
        channel: ChannelId,
        thread_ts: Option<Ts>,
    },
    /// The user opened a thread: fetch its full history (root + all replies) into the store.
    OpenThread {
        channel: ChannelId,
        thread_ts: Ts,
    },
    /// Follow or unfollow a thread (`subscriptions.thread.add` / `.remove`).
    SetThreadSubscribed {
        channel: ChannelId,
        thread_ts: Ts,
        last_read: Ts,
        subscribed: bool,
    },
    /// Fetch the followed threads from Slack (`subscriptions.thread.getView`).
    FetchThreads,
    /// Fetch Slack's Activity feed (`activity.feed`), optionally hydrating row bodies.
    FetchActivity {
        with_bodies: bool,
    },
    /// Mark feed entries read (`activity.markRead`) or unread (`activity.markUnread`).
    MarkActivity {
        entries: Vec<api::ActivityEntry>,
        read: bool,
    },
    /// Fetch all Later / Saved items (`saved.list`) and hydrate their message bodies.
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
    /// Run a slash command Slack handles (`chat.command`, e.g. `/giphy`).
    RunCommand {
        channel: ChannelId,
        /// With its slash: `/giphy`.
        command: String,
        args: String,
        thread_ts: Option<Ts>,
    },
    /// Retry a persisted offline mutation that previously failed to flush.
    RetryOfflineMutation(i64),
    /// Discard a persisted offline mutation from SQLite.
    DiscardOfflineMutation(i64),
}

/// Conversation lifecycle, channel administration, sidebar sections, and conversation tabs.
#[derive(Debug, Clone, PartialEq)]
pub enum ChannelCommand {
    /// Open a conversation: load newest history into the store and backfill older pages.
    OpenConversation(ChannelId),
    /// Fetch a single page of recent history (`conversations.history`, limit 50) without moving
    /// the read cursor or starting background backfill.
    FetchLatestHistory(ChannelId),
    /// Stop backfilling a conversation (it stays in the store).
    CloseConversation(ChannelId),
    /// Leave a channel on Slack (`conversations.leave`).
    LeaveConversation(ChannelId),
    /// Join a public channel on Slack (`conversations.join`).
    JoinConversation(ChannelId),
    /// Set `channel`'s notification level on Slack and mirror into stored prefs.
    SetNotifyLevel {
        channel: ChannelId,
        level: NotifyLevel,
    },
    /// Read a channel's managers (`admin.roles.entity.listAssignments`).
    FetchChannelManagers { channel: ChannelId },
    /// Open (or find) the DM or group DM with `users` (`conversations.open`).
    OpenDm { users: Vec<UserId> },
    /// Create a sidebar channel section (`users.channelSections.create`).
    CreateSection { name: String },
    /// Create a sidebar channel section and move `channel` into it in the same round.
    CreateSectionWithChannel { name: String, channel: ChannelId },
    /// Rename a sidebar section (`users.channelSections.set`).
    RenameSection { section: String, name: String },
    /// Delete a sidebar section (`users.channelSections.delete`).
    DeleteSection { section: String },
    /// Move `channel` between sidebar sections (`users.channelSections.channels.bulkUpdate`).
    MoveChannel {
        channel: ChannelId,
        from: Option<String>,
        to: Option<String>,
    },
    /// Replace `channel`'s topic (`conversations.setTopic`).
    SetChannelTopic { channel: ChannelId, topic: String },
    /// Replace `channel`'s description (`conversations.setPurpose`).
    SetChannelPurpose { channel: ChannelId, purpose: String },
    /// Create a public or private channel (`conversations.create`).
    CreateChannel {
        name: String,
        is_private: bool,
        purpose: Option<String>,
    },
    /// Rename `channel` (`conversations.rename`).
    RenameChannel { channel: ChannelId, name: String },
    /// Archive `channel` (`conversations.archive`).
    ArchiveChannel(ChannelId),
    /// Unarchive `channel` (`conversations.unarchive`).
    UnarchiveChannel(ChannelId),
    /// Permanently delete `channel` (`admin.conversations.delete`, `F-CORE-053`).
    DeleteChannel(ChannelId),
    /// Convert a public channel or group DM to private (`conversations.convertToPrivate`).
    ConvertChannelToPrivate(ChannelId),
    /// Invite `users` into `channel` (`conversations.invite`), then refresh channel members.
    InviteToChannel {
        channel: ChannelId,
        users: Vec<UserId>,
    },
    /// Add a conversation tab (`conversations.addTab`, `F-CORE-061`) and refresh channel info.
    AddConversationTab {
        channel: ChannelId,
        tab_type: String,
        label: Option<String>,
    },
    /// Remove a conversation tab (`conversations.removeTab`, `F-CORE-061`) and refresh channel info.
    RemoveConversationTab { channel: ChannelId, tab_id: String },
    /// Reorder a conversation tab (`conversations.moveTab`, `F-CORE-061`) and refresh channel info.
    MoveConversationTab {
        channel: ChannelId,
        tab_id: String,
        after: Option<String>,
    },
}

/// Search, files, canvases, Slack Lists, app home, workflows, and Block Kit interactions/modals.
#[derive(Debug, Clone, PartialEq)]
pub enum CollabCommand {
    /// Search with Slack's modifier syntax: local store results first, then Slack's.
    Search(String),
    /// Fast remote typeahead for Cmd+K Quick Switcher (`edgeapi`).
    QuickSwitchSearch { query: String },
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
    /// Fetch GIFs for the composer picker (`gif.featured` / `gif.search`).
    SearchGifs { query: String },
    /// Fetch files matching `filter` (`files.list`).
    LoadFiles(api::FilesFilter),
    /// Upload one or more files into `channel` via Slack's external upload v2 flow.
    UploadFiles {
        channel: ChannelId,
        thread_ts: Option<Ts>,
        content: Option<MessageContent>,
        files: Vec<api::UploadFile>,
    },
    /// Permanently delete a user-owned file/clip (`files.delete`, `F-CORE-130`).
    DeleteFile { file_id: String },
    /// Request edit (`upgrade`) or comment (`comment_upgrade`) access on a canvas/list.
    RequestFilePermission {
        file_id: String,
        request_type: String,
    },
    /// Load starred files, custom file collections, and recently deleted canvases/lists.
    LoadFilesExtras,
    /// Star/unstar a file or add/remove it from a collection (`files.favorites.add`/`.remove`).
    SetFileFavorite {
        file_id: String,
        collection_id: Option<String>,
        starred: bool,
    },
    /// Create a custom Files sidebar collection (`files.collections.create`).
    CreateFileCollection { name: String, emoji: Option<String> },
    /// Delete a custom Files sidebar collection (`files.collections.delete`).
    DeleteFileCollection { collection_id: String },
    /// Restore a recently deleted canvas or list (`files.undelete`).
    RestoreDeletedFile { file_id: String },
    /// Create the conversation's empty channel canvas (`conversations.canvases.create`).
    CreateChannelCanvas(ChannelId),
    /// Fetch a live Slack canvas's HTML export (`files.info` + `url_private`).
    LoadCanvas { file_id: String },
    /// Create a standalone (or channel-tabbed) canvas (`canvases.create`).
    CreateCanvas {
        title: String,
        markdown: String,
        channel: Option<ChannelId>,
    },
    /// Apply a section-level edit to a canvas (`canvases.edit`).
    EditCanvas {
        file_id: String,
        op: api::CanvasEditOp,
    },
    /// Permanently delete a canvas (`canvases.delete`).
    DeleteCanvas { canvas_id: String },
    /// Grant read/write/owner access on a canvas (`canvases.access.set`).
    SetCanvasAccess {
        canvas_id: String,
        access_level: api::CanvasAccessLevel,
        channel_ids: Vec<ChannelId>,
        user_ids: Vec<UserId>,
        post_permalink: Option<String>,
    },
    /// Revoke canvas access from channels or users (`canvases.access.delete`).
    DeleteCanvasAccess {
        canvas_id: String,
        channel_ids: Vec<ChannelId>,
        user_ids: Vec<UserId>,
    },
    /// Load threaded comments for a canvas from its derived file comment channel.
    LoadCanvasComments { canvas_id: String },
    /// Post a new top-level comment or reply into a canvas's comment channel.
    PostCanvasComment {
        canvas_id: String,
        text: String,
        thread_ts: Option<Ts>,
    },
    /// Resolve (`quip.thread.archive`) or reopen (`quip.thread.unarchive`) one comment thread.
    ResolveCanvasComment {
        canvas_id: String,
        thread_ts: Ts,
        resolved: bool,
    },
    /// Resolve all open comment threads on a canvas (`quip.thread.archiveAll`).
    ResolveAllCanvasComments { canvas_id: String },
    /// Fetch a live Slack List's schema and records (`files.info` + `lists.records.list`).
    LoadList { file_id: String },
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
    /// Update one or more cells of a Slack List item (`lists.cells.update`).
    UpdateListCell {
        list_id: String,
        row_id: String,
        cells: Vec<api::ListCellInput>,
    },
    /// Delete an item from a Slack List (`lists.records.delete`, `F-COLLAB-228`).
    DeleteListItem { list_id: String, row_id: String },
    /// Add a Slack message to an existing List (`lists.addToList`, `F-COLLAB-231`).
    AddMessageToList {
        list_id: String,
        channel: ChannelId,
        ts: Ts,
    },
    /// Archive an item in a Slack List (`lists.records.archive`, `F-COLLAB-255`).
    ArchiveListItem { list_id: String, row_id: String },
    /// Update sharing/access permissions on a Slack List (`slackLists.access.set`).
    SetListAccess {
        list_id: String,
        access_level: String,
        channel: Option<ChannelId>,
    },
    /// Fetch comment thread messages attached to a List item (`lists.getThreadsForRecord`).
    FetchListRecordComments { list_id: String, record_id: String },
    /// Fetch an app DM's Home tab view (`conversations.info` `return_app_home=true`).
    LoadAppHome(ChannelId),
    /// Resolve and open an app's DM/Home conversation (`apps.home.open`).
    OpenAppHome { app_id: String, team_id: String },
    /// Trigger a Workflow Builder link trigger natively (`workflows.triggers.trip`).
    TripWorkflowTrigger {
        url: String,
        context: serde_json::Value,
    },
    /// Dispatch a Block Kit interactive element action (`blocks.actions`).
    BlockAction {
        bot: BotId,
        app_id: String,
        /// The team that installed the app; the signed-in workspace when `None`.
        team: Option<TeamId>,
        /// The element as the message declared it, plus its `block_id`.
        action: serde_json::Value,
        container: api::ActionContainer,
    },
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
    /// Submit an interactive Block Kit modal (`views.submit`).
    SubmitModal {
        view_id: String,
        hash: Option<String>,
        state_values: serde_json::Value,
    },
    /// Dismiss an interactive Block Kit modal (`views.close`).
    CloseModal {
        view_id: String,
        hash: Option<String>,
    },
}

/// Native audio/video huddle lifecycle, REST calls, and low-level media controls.
#[derive(Debug, Clone, PartialEq)]
pub enum HuddleCommand {
    /// Start or join a native audio huddle in `channel` (`rooms.join` + Amazon Chime SFU).
    Join {
        channel: ChannelId,
        audio: huddle::HuddleAudioConfig,
    },
    /// Leave the currently active native huddle cleanly (sends Chime `LEAVE` and releases audio).
    Leave,
    /// Forward a low-level media/session control directly to the active native huddle session.
    Control(HuddleControlCommand),
    /// Ring/invite workspace members into the active huddle (`rooms.notifyMember`).
    Invite {
        channel: ChannelId,
        users: Vec<UserId>,
    },
    /// Fetch the shareable huddle link for `channel` (`rooms.getLink`).
    GetLink { channel: ChannelId },
    /// Play a short two-note speaker test chime on `output_device`.
    TestSpeaker { output_device: Option<String> },
    /// Set or clear the editable huddle topic (`screenhero.rooms.rename`, `F-COLLAB-016`).
    SetTopic(String),
    /// Update the active huddle's visual theme/background (`huddles.update` + `PeerMessage.Background`).
    SetBackground(String),
    /// Enable or disable live AWS Transcribe captions (`rooms.startTranscription` / `.stopTranscription`).
    SetCaptions(bool),
    /// Decline an incoming direct-message huddle invite (`rooms.inviteResponse`).
    DeclineInvite { channel: ChannelId, call_id: String },
}

impl From<HuddleControlCommand> for HuddleCommand {
    fn from(cmd: HuddleControlCommand) -> Self {
        Self::Control(cmd)
    }
}

/// Low-level media/session control forwarded directly to an active native huddle session.
#[derive(Debug, Clone, PartialEq)]
pub enum HuddleControlCommand {
    SetMuted(bool),
    SetAudioDevices {
        input: Option<String>,
        output: Option<String>,
    },
    SetAudioProcessing {
        aec: bool,
        ns: bool,
        agc: bool,
    },
    SetCamera {
        enabled: bool,
        device_index: Option<usize>,
        blur: bool,
    },
    SetBlurBackground(bool),
    SetScreenShare {
        source_id: Option<String>,
        share_audio: bool,
    },
    SendReaction {
        emoji: String,
        kind: huddle::HuddleReactionKind,
        emoji_url: Option<String>,
    },
    SetHandRaised(bool),
    SetStickyEmoji(Option<String>),
    SendDraw(huddle::HuddleDrawMessage),
    SetJukebox(Option<String>),
    MutePeer(String),
    SetAudioOnly(bool),
}

impl From<HuddleControlCommand> for huddle::HuddleControl {
    fn from(cmd: HuddleControlCommand) -> Self {
        match cmd {
            HuddleControlCommand::SetMuted(muted) => Self::SetMuted(muted),
            HuddleControlCommand::SetAudioDevices { input, output } => {
                Self::SetAudioDevices { input, output }
            }
            HuddleControlCommand::SetAudioProcessing { aec, ns, agc } => {
                Self::SetAudioProcessing { aec, ns, agc }
            }
            HuddleControlCommand::SetCamera {
                enabled,
                device_index,
                blur,
            } => Self::SetCamera {
                enabled,
                device_index,
                blur,
            },
            HuddleControlCommand::SetBlurBackground(blur) => Self::SetBlurBackground(blur),
            HuddleControlCommand::SetScreenShare {
                source_id,
                share_audio,
            } => Self::SetScreenShare {
                source_id,
                share_audio,
            },
            HuddleControlCommand::SendReaction {
                emoji,
                kind,
                emoji_url,
            } => Self::SendReaction {
                emoji,
                kind,
                emoji_url,
            },
            HuddleControlCommand::SetHandRaised(raised) => Self::SetHandRaised(raised),
            HuddleControlCommand::SetStickyEmoji(emoji) => Self::SetStickyEmoji(emoji),
            HuddleControlCommand::SendDraw(draw) => Self::SendDraw(draw),
            HuddleControlCommand::SetJukebox(song) => Self::SetJukebox(song),
            HuddleControlCommand::MutePeer(peer_id) => Self::MutePeer(peer_id),
            HuddleControlCommand::SetAudioOnly(enabled) => Self::SetAudioOnly(enabled),
        }
    }
}

/// Workspace-wide directory, usergroups, DND, profile, presence, preferences, and session auth.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceCommand {
    /// Look up people the workspace directory (`users.list`) leaves out (`users.info` + `team.info`).
    ResolveUsers(Vec<UserId>),
    /// Look up bot profiles (`bots.info`) for app/bot messages carrying only `bot_id`.
    ResolveBots(Vec<BotId>),
    /// Create a workspace `@handle` user group (`usergroups.create`).
    CreateUsergroup {
        name: String,
        handle: String,
        description: String,
    },
    /// Update an existing workspace `@handle` user group (`usergroups.update`).
    UpdateUsergroup {
        usergroup: String,
        name: String,
        handle: String,
        description: String,
    },
    /// Pause the signed-in user's notifications for `minutes` (`dnd.setSnooze`).
    SetDndSnooze { minutes: u32 },
    /// Resume the signed-in user's notifications immediately (`dnd.endSnooze`).
    EndDndSnooze,
    /// End the signed-in user's active DND session (`dnd.endDnd`, `F-SHELL-034`).
    EndDnd,
    /// Batch-fetch DND status for a list of users (`dnd.teamInfo`, `F-SHELL-035`).
    LoadTeamDnd(Vec<UserId>),
    /// Write a global notification preference (`users.prefs.setNotifications` with `global=true`).
    SetGlobalNotificationPref {
        name: String,
        value: String,
        sync: bool,
    },
    /// Update the signed-in user's profile or custom status (`users.profile.set`).
    UpdateProfile { update: api::ProfileUpdate },
    /// Upload a new profile photo for the signed-in user (`users.setPhoto`, `F-SHELL-004`).
    SetUserPhoto { filename: String, bytes: Vec<u8> },
    /// Fetch one user's full profile including custom fields & pronouns (`users.profile.get`).
    FetchUserProfile(UserId),
    /// Manually set presence active or away (`presence.set`).
    SetPresence { away: bool },
    /// Enable or disable Stealth Mode (suppresses outbound typing, read-cursor sync, active presence).
    SetStealthMode(bool),
    /// Enable or disable local SQLite FTS5 archiving of finalized huddle captions (text-only).
    SetSaveHuddleTranscripts(bool),
    /// Write a single user preference key/value back to Slack (`users.prefs.set`).
    SetUserPref { name: String, value: String },
    /// Revoke the workspace token server-side (`auth.revoke`, `F-CORE-009`).
    SignOut,
}
