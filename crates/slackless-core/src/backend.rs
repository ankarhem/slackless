//! The seam between the core and Slack's REST API.

use std::collections::HashMap;
use std::future::Future;

use crate::api::{
    self, ActionContainer, ActivityEntry, ActivityFeed, ChannelBookmark, ClientCounts, FilesFilter,
    HistoryOpts, ListOpts, MessageContent, Page, PinnedItem, PostedMessage, ScheduledMessage,
    ScheduledMessageCreated, SearchMessages, SearchOpts, SectionCreated, SlackApi, ThreadsView,
    UploadFile, UserBoot,
};
use crate::types::{
    BotId, BotProfile, ChannelId, ChannelSection, Conversation, ConversationKind, DndStatus, File,
    Message, Team, TeamId, Ts, User, UserId, UserPresence, Usergroup,
};

/// `conversations.history` page size the core requests (Slack max 999).
pub(crate) const HISTORY_PAGE_LIMIT: u32 = 200;
/// Single-page `conversations.history` size for [`crate::Command::FetchLatestHistory`].
pub(crate) const UNREADS_HISTORY_LIMIT: u32 = 50;

/// `subscriptions.thread.getView` page size the core requests.
pub(crate) const THREADS_PAGE_LIMIT: u32 = 50;
/// `activity.feed` page size the core requests for the Activity view.
pub(crate) const ACTIVITY_PAGE_LIMIT: u32 = 50;
/// `files.list` page size the core requests for a conversation's Files & links tab.
pub(crate) const FILES_PAGE: u32 = 100;
/// `files.list` page size the core requests for a workspace-wide list (the Files view's
/// row cap: one page, newest first).
pub(crate) const FILES_WORKSPACE_PAGE: u32 = 200;

/// The Slack calls the core makes. [`SlackApi`] is the real implementation; tests drive the core
/// with an in-process fake.
pub trait Backend: Clone + Send + Sync + 'static {
    fn user_boot(&self) -> impl Future<Output = api::Result<UserBoot>> + Send;
    fn counts(&self) -> impl Future<Output = api::Result<ClientCounts>> + Send;
    /// The workspace's user directory (`client.userBoot` carries only the signed-in user).
    fn users(&self) -> impl Future<Output = api::Result<Vec<User>>> + Send;
    /// The user's sidebar sections (`users.channelSections.list`).
    fn channel_sections(&self) -> impl Future<Output = api::Result<Vec<ChannelSection>>> + Send;
    /// The workspace's custom emoji (`emoji.list`).
    fn custom_emoji(&self) -> impl Future<Output = api::Result<HashMap<String, String>>> + Send;
    /// The DMs-tab list (`client.dms`): IMs and MPIMs with per-DM `is_open`, closed DMs
    /// included. Read-only.
    fn dm_list(&self) -> impl Future<Output = api::Result<Vec<Conversation>>> + Send;
    /// The signed-in user's channel memberships (`users.conversations`, public/private,
    /// unarchived, all pages). Read-only.
    fn channel_memberships(&self) -> impl Future<Output = api::Result<Vec<Conversation>>> + Send;
    /// One page of channel history, newest first, strictly older than `latest` when set.
    fn history_page(
        &self,
        channel: ChannelId,
        latest: Option<Ts>,
        cursor: Option<String>,
    ) -> impl Future<Output = api::Result<Page<Message>>> + Send;
    /// One page of a thread (`conversations.replies`): the root followed by its replies.
    fn replies_page(
        &self,
        channel: ChannelId,
        thread_ts: Ts,
        cursor: Option<String>,
    ) -> impl Future<Output = api::Result<Page<Message>>> + Send;
    fn conversation_info(
        &self,
        channel: ChannelId,
    ) -> impl Future<Output = api::Result<Conversation>> + Send;
    /// Every member id of a conversation (`conversations.members`, all pages). Read-only.
    fn conversation_members(
        &self,
        channel: ChannelId,
    ) -> impl Future<Output = api::Result<Vec<UserId>>> + Send;
    /// The newest message of a conversation (`conversations.history`, limit 1). Read-only;
    /// the DM-list preview backfill uses it for conversations with nothing stored.
    fn latest_message(
        &self,
        channel: ChannelId,
    ) -> impl Future<Output = api::Result<Option<Message>>> + Send;
    /// One page of recent history (`conversations.history`, up to `limit` messages, newest
    /// first). Read-only; never moves the read cursor.
    fn recent_history(
        &self,
        channel: ChannelId,
        limit: u32,
    ) -> impl Future<Output = api::Result<Vec<Message>>> + Send;
    /// A user's do-not-disturb state (`dnd.info`). Read-only.
    fn dnd_info(&self, user: UserId) -> impl Future<Output = api::Result<DndStatus>> + Send;
    /// One user's profile (`users.info`), reaching people `users.list` omits. Read-only.
    fn user_info(&self, user: UserId) -> impl Future<Output = api::Result<User>> + Send;
    /// The signed-in user leaves `channel` (`conversations.leave`).
    fn leave_conversation(
        &self,
        channel: ChannelId,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// The signed-in user joins public `channel` (`conversations.join`); the answer carries
    /// the joined channel.
    fn join_conversation(
        &self,
        channel: ChannelId,
    ) -> impl Future<Output = api::Result<Conversation>> + Send;
    /// Replace `channel`'s topic (`conversations.setTopic`); the answer carries the updated
    /// conversation.
    fn set_topic(
        &self,
        channel: ChannelId,
        topic: String,
    ) -> impl Future<Output = api::Result<Conversation>> + Send;
    /// Replace `channel`'s description (`conversations.setPurpose`); the answer carries the
    /// updated conversation.
    fn set_purpose(
        &self,
        channel: ChannelId,
        purpose: String,
    ) -> impl Future<Output = api::Result<Conversation>> + Send;
    /// One workspace's record (`team.info`), by team id: the connected organizations of a
    /// Slack Connect channel, which `client.userBoot` never carries.
    fn team_info(&self, team: TeamId) -> impl Future<Output = api::Result<Team>> + Send;
    /// One per-channel notification pref (`users.prefs.setNotifications`, web client).
    fn set_notification_pref(
        &self,
        channel: ChannelId,
        name: &'static str,
        value: &'static str,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// A user's current presence (`users.getPresence`). Read-only.
    fn user_presence(&self, user: UserId)
    -> impl Future<Output = api::Result<UserPresence>> + Send;
    /// `broadcast` also posts a thread reply to the channel (`reply_broadcast`).
    fn post_message(
        &self,
        channel: ChannelId,
        content: MessageContent,
        thread_ts: Option<Ts>,
        broadcast: bool,
    ) -> impl Future<Output = api::Result<PostedMessage>> + Send;
    fn update_message(
        &self,
        _channel: ChannelId,
        _ts: Ts,
        _content: MessageContent,
    ) -> impl Future<Output = api::Result<PostedMessage>> + Send {
        async { Ok(PostedMessage::default()) }
    }
    fn delete_message(
        &self,
        _channel: ChannelId,
        _ts: Ts,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    fn add_reaction(
        &self,
        _channel: ChannelId,
        _ts: Ts,
        _name: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    fn remove_reaction(
        &self,
        _channel: ChannelId,
        _ts: Ts,
        _name: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Run a slash command Slack handles (`chat.command`) in `channel`, or in the thread at
    /// `thread_ts`; `team` is the signed-in workspace.
    fn run_command(
        &self,
        _channel: ChannelId,
        _command: String,
        _args: String,
        _thread_ts: Option<Ts>,
        _team: TeamId,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Dispatch a Block Kit interaction to the app that posted the message (`blocks.actions`).
    fn block_action(
        &self,
        _bot: BotId,
        _app_id: String,
        _team: TeamId,
        _action: serde_json::Value,
        _container: ActionContainer,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// A bot's profile (`bots.info`), for messages that carry only its id.
    fn bot_info(&self, bot: BotId) -> impl Future<Output = api::Result<BotProfile>> + Send {
        async move {
            Ok(BotProfile {
                id: bot,
                ..Default::default()
            })
        }
    }
    /// The followed threads of the Threads view (`subscriptions.thread.getView`), page one.
    fn threads_view(&self) -> impl Future<Output = api::Result<ThreadsView>> + Send;
    /// The Activity view's feed (`activity.feed`), page one; row bodies hydrate separately
    /// through [`Backend::messages_list`].
    fn activity_feed(&self) -> impl Future<Output = api::Result<ActivityFeed>> + Send;
    /// `messages.list`: the bodies of the given (channel, ts) pairs, batched per call.
    fn messages_list(
        &self,
        ids: Vec<(ChannelId, Vec<Ts>)>,
    ) -> impl Future<Output = api::Result<HashMap<ChannelId, Vec<Message>>>> + Send;
    /// `activity.markRead` for feed entries.
    fn activity_mark_read(
        &self,
        entries: Vec<ActivityEntry>,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// `activity.markUnread` for feed entries.
    fn activity_mark_unread(
        &self,
        entries: Vec<ActivityEntry>,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// Open (or find) the DM or group DM with `users` (`conversations.open`).
    fn open_dm(&self, users: Vec<UserId>)
    -> impl Future<Output = api::Result<Conversation>> + Send;
    /// Create a sidebar channel section (`users.channelSections.create`).
    fn create_section(
        &self,
        name: String,
    ) -> impl Future<Output = api::Result<SectionCreated>> + Send;
    /// Rename a sidebar section (`users.channelSections.set`; `name` and `emoji` must both
    /// be resent because every omitted key is cleared).
    fn rename_section(
        &self,
        section: String,
        name: String,
        emoji: String,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// Delete a sidebar section (`users.channelSections.delete`).
    fn delete_section(&self, section: String) -> impl Future<Output = api::Result<()>> + Send;
    /// Move one channel between sections (`users.channelSections.channels.bulkUpdate`):
    /// `insert` into a target and `remove` from a source, in one call.
    fn move_channel(
        &self,
        insert: Option<(String, ChannelId)>,
        remove: Option<(String, ChannelId)>,
    ) -> impl Future<Output = api::Result<()>> + Send;
    fn search(&self, query: String) -> impl Future<Output = api::Result<SearchMessages>> + Send;
    fn mark_read(&self, channel: ChannelId, ts: Ts)
    -> impl Future<Output = api::Result<()>> + Send;
    /// Move the read cursor of a thread (`subscriptions.thread.mark`).
    fn mark_thread_read(
        &self,
        channel: ChannelId,
        thread_ts: Ts,
        ts: Ts,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// Mark every conversation and thread read (`users.markAllRead`).
    fn mark_all_read(&self) -> impl Future<Output = api::Result<()>> + Send;
    /// Follow or unfollow a thread (`subscriptions.thread.add` / `.remove`); `last_read` is
    /// the thread's read cursor.
    fn set_thread_subscribed(
        &self,
        channel: ChannelId,
        thread_ts: Ts,
        last_read: Ts,
        subscribed: bool,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// A channel's managers (`admin.roles.entity.listAssignments`). Read-only.
    fn channel_managers(
        &self,
        channel: ChannelId,
    ) -> impl Future<Output = api::Result<Vec<UserId>>> + Send;
    /// A conversation's pinned items (`pins.list`), oldest pin first. Read-only.
    fn pins(&self, channel: ChannelId)
    -> impl Future<Output = api::Result<Vec<PinnedItem>>> + Send;
    /// A conversation's bookmarks (`bookmarks.list`), folders included. Read-only.
    fn bookmarks(
        &self,
        channel: ChannelId,
    ) -> impl Future<Output = api::Result<Vec<ChannelBookmark>>> + Send;
    /// The files matching `filter` (`files.list`), newest first: one conversation's shared
    /// files, or every file the signed-in user can see when no channel is set. Read-only.
    fn files(&self, filter: FilesFilter) -> impl Future<Output = api::Result<Vec<File>>> + Send;
    /// Add a link bookmark to a conversation's bar (`bookmarks.add`).
    fn add_bookmark(
        &self,
        channel: ChannelId,
        title: String,
        url: String,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// Pin (`pins.add`) or unpin (`pins.remove`) the message `ts` in `channel`.
    fn set_pinned(
        &self,
        channel: ChannelId,
        ts: Ts,
        pinned: bool,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// Create a conversation's empty channel canvas (`conversations.canvases.create`),
    /// returning the canvas file id.
    fn create_canvas(&self, channel: ChannelId)
    -> impl Future<Output = api::Result<String>> + Send;
    /// Pause the signed-in user's notifications for `minutes` (`dnd.setSnooze`).
    fn set_dnd_snooze(&self, minutes: u32) -> impl Future<Output = api::Result<DndStatus>> + Send;
    /// End the signed-in user's active notification snooze (`dnd.endSnooze`).
    fn end_dnd_snooze(&self) -> impl Future<Output = api::Result<DndStatus>> + Send;
    /// Queue a message for delivery at unix timestamp `post_at` (`chat.scheduleMessage`).
    fn schedule_message(
        &self,
        channel: ChannelId,
        post_at: u64,
        content: MessageContent,
        thread_ts: Option<Ts>,
    ) -> impl Future<Output = api::Result<ScheduledMessageCreated>> + Send;
    /// List pending scheduled messages (`chat.scheduledMessages.list`).
    fn scheduled_messages(&self)
    -> impl Future<Output = api::Result<Vec<ScheduledMessage>>> + Send;
    /// Delete a pending scheduled message (`chat.deleteScheduledMessage`).
    fn delete_scheduled_message(
        &self,
        channel: ChannelId,
        scheduled_message_id: String,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// Active `@handle` user groups (`usergroups.list`).
    fn usergroups(&self) -> impl Future<Output = api::Result<Vec<Usergroup>>> + Send {
        async { Ok(Vec::new()) }
    }
    /// Upload files into `channel` via external upload v2 (`files.getUploadURLExternal` +
    /// raw upload POST + `files.completeUploadExternal`).
    fn upload_files(
        &self,
        channel: ChannelId,
        thread_ts: Option<Ts>,
        content: Option<MessageContent>,
        files: Vec<UploadFile>,
    ) -> impl Future<Output = api::Result<Vec<File>>> + Send;
    /// Featured/trending GIFs for the composer picker (`gif.featured`).
    fn gif_featured(
        &self,
        _limit: u32,
    ) -> impl Future<Output = api::Result<Vec<api::GifItem>>> + Send {
        async { Ok(Vec::new()) }
    }
    /// Search GIFs for the composer picker (`gif.search`).
    fn gif_search(
        &self,
        _query: String,
        _limit: u32,
    ) -> impl Future<Output = api::Result<Vec<api::GifItem>>> + Send {
        async { Ok(Vec::new()) }
    }
    /// Saved-for-Later items (`saved.list`) under `filter` (`"saved"`, `"completed"`, `"archived"`).
    fn saved_list(
        &self,
        _filter: &'static str,
    ) -> impl Future<Output = api::Result<Vec<api::SavedItem>>> + Send {
        async { Ok(Vec::new()) }
    }
    /// Save a message for Later (`saved.add`), optionally with a reminder (`date_due`).
    fn saved_add(
        &self,
        _channel: ChannelId,
        _ts: Ts,
        _date_due: Option<u64>,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Update a saved item's state (`mark`) and/or reminder (`date_due`).
    fn saved_update(
        &self,
        _channel: ChannelId,
        _ts: Ts,
        _mark: Option<api::SavedMark>,
        _date_due: Option<Option<u64>>,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Remove a message from Later (`saved.delete`).
    fn saved_delete(
        &self,
        _channel: ChannelId,
        _ts: Ts,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Clear all completed Later items (`saved.clearCompleted`).
    fn saved_clear_completed(&self) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Workspace slash command catalog (`client.appCommands`).
    fn app_commands(&self) -> impl Future<Output = api::Result<Vec<api::AppCommand>>> + Send {
        async { Ok(Vec::new()) }
    }
    /// Fast remote typeahead (`edgeapi` `users/search` + `channels/search`) for Cmd+K.
    fn edge_search(
        &self,
        _team_id: TeamId,
        _query: String,
    ) -> impl Future<Output = api::Result<api::EdgeSearchResults>> + Send {
        async { Ok(api::EdgeSearchResults::default()) }
    }
    /// Opt-in remote Slack message search (`search.modules.messages`).
    fn search_remote_messages(
        &self,
        _query: String,
        _sort: api::SearchSort,
        page: usize,
    ) -> impl Future<Output = api::Result<api::RemoteMessagePage>> + Send {
        async move {
            Ok(api::RemoteMessagePage {
                page,
                ..Default::default()
            })
        }
    }
    /// Opt-in remote Slack file search (`search.modules.files`).
    fn search_remote_files(
        &self,
        _query: String,
        _sort: api::SearchSort,
        page: usize,
    ) -> impl Future<Output = api::Result<api::RemoteFilePage>> + Send {
        async move {
            Ok(api::RemoteFilePage {
                page,
                ..Default::default()
            })
        }
    }
    /// Update the signed-in user's profile or custom status (`users.profile.set`).
    fn update_profile(
        &self,
        _update: api::ProfileUpdate,
    ) -> impl Future<Output = api::Result<slackless_types::Profile>> + Send {
        async { Ok(slackless_types::Profile::default()) }
    }
    /// Manually set presence active or away (`presence.set`).
    fn set_presence(&self, _away: bool) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Create a new public or private channel (`conversations.create`).
    fn create_channel(
        &self,
        name: String,
        is_private: bool,
    ) -> impl Future<Output = api::Result<Conversation>> + Send;
    /// Rename a channel (`conversations.rename`).
    fn rename_channel(
        &self,
        channel: ChannelId,
        name: String,
    ) -> impl Future<Output = api::Result<Conversation>> + Send {
        async move {
            Ok(Conversation {
                id: channel,
                name: Some(name),
                ..Default::default()
            })
        }
    }
    /// Archive a channel (`conversations.archive`).
    fn archive_channel(&self, _channel: ChannelId) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Unarchive a channel (`conversations.unarchive`).
    fn unarchive_channel(
        &self,
        _channel: ChannelId,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Invite users to a channel (`conversations.invite`).
    fn invite_to_channel(
        &self,
        channel: ChannelId,
        _users: Vec<UserId>,
    ) -> impl Future<Output = api::Result<Conversation>> + Send {
        async move {
            Ok(Conversation {
                id: channel,
                ..Default::default()
            })
        }
    }
    /// End active DND session (`dnd.endDnd`, `F-SHELL-034`).
    fn dnd_end_dnd(&self) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Batch DND status lookup (`dnd.teamInfo`, `F-SHELL-035`).
    fn dnd_team_info(
        &self,
        _users: Vec<UserId>,
    ) -> impl Future<Output = api::Result<HashMap<UserId, slackless_types::DndStatus>>> + Send {
        async { Ok(HashMap::new()) }
    }
    /// Write a single user preference back to Slack (`users.prefs.set`).
    fn set_user_pref(
        &self,
        _name: String,
        _value: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Fetch a user's full profile including custom `fields` and `pronouns` (`users.profile.get`).
    fn user_profile(
        &self,
        _user: UserId,
    ) -> impl Future<Output = api::Result<slackless_types::Profile>> + Send {
        async { Ok(slackless_types::Profile::default()) }
    }
    /// Fetch workspace custom profile field definitions (`team.profile.get`).
    fn team_profile_fields(
        &self,
    ) -> impl Future<Output = api::Result<HashMap<String, String>>> + Send {
        async { Ok(HashMap::new()) }
    }
    /// Submit a Block Kit modal view (`views.submit`).
    fn views_submit(
        &self,
        _view_id: String,
        _hash: Option<String>,
        _state_values: serde_json::Value,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Close a Block Kit modal view (`views.close`).
    fn views_close(
        &self,
        _view_id: String,
        _hash: Option<String>,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Add a link bookmark inside a folder (`bookmarks.add` with `parent_id`).
    fn add_bookmark_in_folder(
        &self,
        _channel: ChannelId,
        _title: String,
        _url: String,
        _parent_id: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Remove a bookmark (`bookmarks.remove`).
    fn remove_bookmark(
        &self,
        _channel: ChannelId,
        _bookmark_id: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Fetch cloud-synced drafts (`drafts.list`).
    fn drafts_list(&self) -> impl Future<Output = api::Result<Vec<api::CloudDraft>>> + Send {
        async { Ok(Vec::new()) }
    }
    /// Create a cloud draft (`drafts.create`).
    fn drafts_create(
        &self,
        _channel: ChannelId,
        _text: String,
    ) -> impl Future<Output = api::Result<Option<String>>> + Send {
        async { Ok(None) }
    }
    /// Update an existing cloud draft (`drafts.update`).
    fn drafts_update(
        &self,
        _draft_id: String,
        _channel: ChannelId,
        _text: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Delete a cloud draft (`drafts.delete`).
    fn drafts_delete(&self, _draft_id: String) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Fetch dynamic `external_select` options (`blocks.suggestions`).
    #[allow(clippy::too_many_arguments)]
    fn block_suggestions(
        &self,
        _bot: BotId,
        _app_id: String,
        _team: TeamId,
        _action_id: String,
        _block_id: String,
        _value: String,
        _container: ActionContainer,
    ) -> impl Future<Output = api::Result<Vec<(String, String)>>> + Send {
        async { Ok(Vec::new()) }
    }
    /// Revoke the workspace token server-side (`auth.revoke`, `F-CORE-009`).
    fn auth_revoke(&self) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Add a conversation tab (`conversations.addTab`, `F-CORE-061`).
    fn add_conversation_tab(
        &self,
        _channel: ChannelId,
        _tab_type: String,
        _label: Option<String>,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Remove a conversation tab (`conversations.removeTab`, `F-CORE-061`).
    fn remove_conversation_tab(
        &self,
        _channel: ChannelId,
        _tab_id: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Move a conversation tab (`conversations.moveTab`, `F-CORE-061`).
    fn move_conversation_tab(
        &self,
        _channel: ChannelId,
        _tab_id: String,
        _after: Option<String>,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Save a file for Later (`saved.add` with `item_type=file`, `F-CORE-157`).
    fn save_file(
        &self,
        _file_id: String,
        _date_due: Option<u64>,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Permanently delete a user-owned file/clip (`files.delete`, `F-CORE-130`).
    fn delete_file(&self, _file_id: String) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Fetch a live Slack canvas's read-only HTML export (`files.info` + `url_private`, `F-COLLAB-125`).
    fn fetch_canvas_html(
        &self,
        _file_id: String,
    ) -> impl Future<Output = api::Result<(String, String)>> + Send {
        async { Ok((String::new(), String::new())) }
    }
    /// Create a standalone or channel-tabbed canvas (`canvases.create`, `F-COLLAB-119`).
    fn canvases_create(
        &self,
        _title: Option<String>,
        _markdown: Option<String>,
        _channel: Option<ChannelId>,
    ) -> impl Future<Output = api::Result<api::CanvasCreated>> + Send {
        async { Ok(api::CanvasCreated::default()) }
    }
    /// Apply a section-level edit to a canvas (`canvases.edit`, `F-COLLAB-132`, `F-SHELL-264`).
    fn canvases_edit(
        &self,
        _file_id: String,
        _op: api::CanvasEditOp,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Request edit or comment access on a file/canvas (`files.requestPermission`, `F-COLLAB-129`).
    fn files_request_permission(
        &self,
        _file_id: String,
        _request_type: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Fetch a live Slack List's schema and rows (`files.info` + `lists.records.list`).
    fn fetch_list(
        &self,
        _file_id: String,
    ) -> impl Future<Output = api::Result<api::SlackListData>> + Send {
        async { Ok(api::SlackListData::default()) }
    }
    /// Create a new Slack List (`lists.create`, `F-COLLAB-213`).
    fn lists_create(
        &self,
        _name: String,
        _description: Option<String>,
        _channel: Option<ChannelId>,
    ) -> impl Future<Output = api::Result<String>> + Send {
        async { Ok(String::new()) }
    }
    /// Add an item to a Slack List (`lists.records.create`, `F-COLLAB-226`).
    fn lists_records_create(
        &self,
        _list_id: String,
        _initial_fields: Vec<api::ListCellInput>,
    ) -> impl Future<Output = api::Result<api::SlackListRow>> + Send {
        async { Ok(api::SlackListRow::default()) }
    }
    /// Update one or more cells of a Slack List item (`lists.cells.update`, `F-COLLAB-227`).
    fn lists_cells_update(
        &self,
        _list_id: String,
        _row_id: String,
        _cells: Vec<api::ListCellInput>,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Delete an item from a Slack List (`lists.records.delete`, `F-COLLAB-228`).
    fn lists_records_delete(
        &self,
        _list_id: String,
        _row_id: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Add a message to a Slack List (`lists.addToList`, `F-COLLAB-231`).
    fn lists_add_to_list(
        &self,
        _list_id: String,
        _channel: ChannelId,
        _ts: Ts,
    ) -> impl Future<Output = api::Result<String>> + Send {
        async { Ok(String::new()) }
    }
    /// Fetch an app DM's Home tab view (`conversations.info` `return_app_home=true`) and
    /// dispatch `apps.home.dispatchOpenEvent` (`F-COLLAB-480`).
    fn load_app_home(
        &self,
        _channel: ChannelId,
    ) -> impl Future<Output = api::Result<api::AppHomeResponse>> + Send {
        async { Ok(api::AppHomeResponse::default()) }
    }
    /// Open an app's Home DM conversation (`apps.home.open`, `F-COLLAB-481`).
    fn open_app_home(
        &self,
        _app_id: String,
        _team_id: String,
    ) -> impl Future<Output = api::Result<Option<ChannelId>>> + Send {
        async { Ok(None) }
    }
    /// Trigger a Workflow Builder link trigger (`workflows.triggers.trip`, `F-COLLAB-319`, `F-COLLAB-376`).
    fn trip_workflow_trigger(
        &self,
        _url: String,
        _context: serde_json::Value,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Write a global notification preference (`users.prefs.setNotifications` with `global=true`).
    fn set_global_notification_pref(
        &self,
        _name: String,
        _value: String,
        _sync: bool,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Starred file IDs (`files.favorites.list`, `F-CORE-133`).
    fn files_favorites_list(&self) -> impl Future<Output = api::Result<Vec<String>>> + Send {
        async { Ok(Vec::new()) }
    }
    /// Star/unstar a file or add/remove it from a collection (`files.favorites.add`/`.remove`).
    fn files_favorites_set(
        &self,
        _file_id: String,
        _collection_id: Option<String>,
        _starred: bool,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Custom file collections (`files.collections.list`, `F-CORE-133`).
    fn files_collections_list(
        &self,
    ) -> impl Future<Output = api::Result<Vec<api::FileCollection>>> + Send {
        async { Ok(Vec::new()) }
    }
    /// Create a custom file collection (`files.collections.create`, `F-CORE-133`).
    fn files_collections_create(
        &self,
        _name: String,
        _emoji: Option<String>,
    ) -> impl Future<Output = api::Result<Vec<api::FileCollection>>> + Send {
        async { Ok(Vec::new()) }
    }
    /// Delete a custom file collection (`files.collections.delete`, `F-CORE-133`).
    fn files_collections_delete(
        &self,
        _collection_id: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Recently deleted canvases & lists (`files.recentlyDeleted`, `F-CORE-133`).
    fn files_recently_deleted(
        &self,
    ) -> impl Future<Output = api::Result<Vec<api::DeletedFile>>> + Send {
        async { Ok(Vec::new()) }
    }
    /// Restore a recently deleted canvas or list (`files.undelete`, `F-CORE-133`).
    fn files_undelete(&self, _file_id: String) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Join or start a Slack huddle in `channel` (`rooms.join`, returns Chime `free_willy` credentials).
    fn rooms_join(
        &self,
        _channel: ChannelId,
    ) -> impl Future<Output = api::Result<api::RoomJoinResponse>> + Send {
        async {
            Err(api::ApiError::UnsupportedHuddleBackend(
                "huddles are unavailable in this backend".to_owned(),
            ))
        }
    }
    /// Invite/ring a workspace member into the active huddle (`rooms.notifyMember`).
    fn rooms_notify_member(
        &self,
        _channel: ChannelId,
        _user: UserId,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Fetch a shareable huddle URL for `channel` (`rooms.getLink`).
    fn rooms_get_link(
        &self,
        channel: ChannelId,
    ) -> impl Future<Output = api::Result<String>> + Send {
        async move { Ok(format!("https://app.slack.com/huddle/{channel}")) }
    }
    /// Set or clear the live huddle topic (`screenhero.rooms.rename`).
    fn rooms_rename(
        &self,
        _room_id: String,
        _name: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Start AWS Transcribe live captions for an active huddle (`rooms.startTranscription`).
    fn rooms_start_transcription(
        &self,
        _call_id: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Stop AWS Transcribe live captions for an active huddle (`rooms.stopTranscription`).
    fn rooms_stop_transcription(
        &self,
        _call_id: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Respond to an incoming huddle invite (`rooms.inviteResponse`, e.g. `"decline"`).
    fn rooms_invite_response(
        &self,
        _channel: ChannelId,
        _room_id: String,
        _response: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Update the active huddle's theme/background (`huddles.update`).
    fn huddles_update_background(
        &self,
        _huddle_id: String,
        _background_id: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
}

impl Backend for SlackApi {
    async fn user_boot(&self) -> api::Result<UserBoot> {
        self.client_user_boot().await
    }

    async fn counts(&self) -> api::Result<ClientCounts> {
        self.client_counts().await
    }

    async fn users(&self) -> api::Result<Vec<User>> {
        self.users_list_all().await
    }

    async fn channel_sections(&self) -> api::Result<Vec<ChannelSection>> {
        self.users_channel_sections_list().await
    }

    async fn custom_emoji(&self) -> api::Result<HashMap<String, String>> {
        self.emoji_list().await
    }

    async fn dm_list(&self) -> api::Result<Vec<Conversation>> {
        self.client_dms_all().await
    }

    async fn channel_memberships(&self) -> api::Result<Vec<Conversation>> {
        let opts = ListOpts {
            kinds: vec![
                ConversationKind::PublicChannel,
                ConversationKind::PrivateChannel,
            ],
            exclude_archived: true,
            limit: Some(1000),
        };
        self.users_conversations_all(&opts).await
    }

    async fn history_page(
        &self,
        channel: ChannelId,
        latest: Option<Ts>,
        cursor: Option<String>,
    ) -> api::Result<Page<Message>> {
        let opts = HistoryOpts {
            latest,
            limit: Some(HISTORY_PAGE_LIMIT),
            ..Default::default()
        };
        self.conversations_history(&channel, &opts, cursor).await
    }

    async fn replies_page(
        &self,
        channel: ChannelId,
        thread_ts: Ts,
        cursor: Option<String>,
    ) -> api::Result<Page<Message>> {
        let opts = HistoryOpts {
            limit: Some(HISTORY_PAGE_LIMIT),
            ..Default::default()
        };
        self.conversations_replies(&channel, &thread_ts, &opts, cursor)
            .await
    }

    async fn conversation_info(&self, channel: ChannelId) -> api::Result<Conversation> {
        self.conversations_info(&channel).await
    }

    async fn conversation_members(&self, channel: ChannelId) -> api::Result<Vec<UserId>> {
        self.conversations_members_all(&channel).await
    }

    async fn latest_message(&self, channel: ChannelId) -> api::Result<Option<Message>> {
        let opts = HistoryOpts {
            limit: Some(1),
            ..Default::default()
        };
        Ok(self
            .conversations_history(&channel, &opts, None)
            .await?
            .items
            .into_iter()
            .next())
    }

    async fn recent_history(&self, channel: ChannelId, limit: u32) -> api::Result<Vec<Message>> {
        let opts = HistoryOpts {
            limit: Some(limit),
            ..Default::default()
        };
        Ok(self
            .conversations_history(&channel, &opts, None)
            .await?
            .items)
    }

    async fn dnd_info(&self, user: UserId) -> api::Result<DndStatus> {
        SlackApi::dnd_info(self, &user).await
    }

    async fn user_info(&self, user: UserId) -> api::Result<User> {
        self.users_info(&user).await
    }

    async fn leave_conversation(&self, channel: ChannelId) -> api::Result<()> {
        self.conversations_leave(&channel).await
    }

    async fn join_conversation(&self, channel: ChannelId) -> api::Result<Conversation> {
        self.conversations_join(&channel).await
    }

    async fn set_topic(&self, channel: ChannelId, topic: String) -> api::Result<Conversation> {
        self.conversations_set_topic(&channel, &topic).await
    }

    async fn set_purpose(&self, channel: ChannelId, purpose: String) -> api::Result<Conversation> {
        self.conversations_set_purpose(&channel, &purpose).await
    }

    async fn team_info(&self, team: TeamId) -> api::Result<Team> {
        SlackApi::team_info(self, &team).await
    }

    async fn set_notification_pref(
        &self,
        channel: ChannelId,
        name: &'static str,
        value: &'static str,
    ) -> api::Result<()> {
        self.users_prefs_set_notifications(&channel, name, value)
            .await
    }

    async fn user_presence(&self, user: UserId) -> api::Result<UserPresence> {
        self.users_get_presence(&user).await
    }

    async fn post_message(
        &self,
        channel: ChannelId,
        content: MessageContent,
        thread_ts: Option<Ts>,
        broadcast: bool,
    ) -> api::Result<PostedMessage> {
        self.chat_post_message(&channel, &content, thread_ts.as_ref(), broadcast)
            .await
    }

    async fn update_message(
        &self,
        channel: ChannelId,
        ts: Ts,
        content: MessageContent,
    ) -> api::Result<PostedMessage> {
        self.chat_update(&channel, &ts, &content).await
    }

    async fn delete_message(&self, channel: ChannelId, ts: Ts) -> api::Result<()> {
        self.chat_delete(&channel, &ts).await
    }

    async fn add_reaction(&self, channel: ChannelId, ts: Ts, name: String) -> api::Result<()> {
        self.reactions_add(&channel, &ts, &name).await
    }

    async fn remove_reaction(&self, channel: ChannelId, ts: Ts, name: String) -> api::Result<()> {
        self.reactions_remove(&channel, &ts, &name).await
    }

    async fn run_command(
        &self,
        channel: ChannelId,
        command: String,
        args: String,
        thread_ts: Option<Ts>,
        team: TeamId,
    ) -> api::Result<()> {
        self.chat_command(&channel, &command, &args, thread_ts.as_ref(), &team)
            .await
    }

    async fn block_action(
        &self,
        bot: BotId,
        app_id: String,
        team: TeamId,
        action: serde_json::Value,
        container: ActionContainer,
    ) -> api::Result<()> {
        self.blocks_actions(&bot, &app_id, &team, &action, &container)
            .await
    }

    async fn bot_info(&self, bot: BotId) -> api::Result<BotProfile> {
        self.bots_info(&bot).await
    }

    async fn search(&self, query: String) -> api::Result<SearchMessages> {
        self.search_messages(&query, &SearchOpts::default()).await
    }

    async fn threads_view(&self) -> api::Result<ThreadsView> {
        self.subscriptions_thread_get_view(THREADS_PAGE_LIMIT).await
    }

    async fn activity_feed(&self) -> api::Result<ActivityFeed> {
        SlackApi::activity_feed(self, ACTIVITY_PAGE_LIMIT).await
    }

    async fn messages_list(
        &self,
        ids: Vec<(ChannelId, Vec<Ts>)>,
    ) -> api::Result<HashMap<ChannelId, Vec<Message>>> {
        SlackApi::messages_list(self, &ids).await
    }

    async fn activity_mark_read(&self, entries: Vec<ActivityEntry>) -> api::Result<()> {
        SlackApi::activity_mark_read(self, &entries).await
    }

    async fn activity_mark_unread(&self, entries: Vec<ActivityEntry>) -> api::Result<()> {
        SlackApi::activity_mark_unread(self, &entries).await
    }

    async fn open_dm(&self, users: Vec<UserId>) -> api::Result<Conversation> {
        let csv: Vec<&str> = users.iter().map(UserId::as_str).collect();
        self.conversations_open(&csv.join(",")).await
    }

    async fn create_section(&self, name: String) -> api::Result<SectionCreated> {
        self.users_channel_sections_create(&name, "").await
    }

    async fn rename_section(
        &self,
        section: String,
        name: String,
        emoji: String,
    ) -> api::Result<()> {
        self.users_channel_sections_set(&section, &name, &emoji)
            .await
    }

    async fn delete_section(&self, section: String) -> api::Result<()> {
        self.users_channel_sections_delete(&section).await
    }

    async fn move_channel(
        &self,
        insert: Option<(String, ChannelId)>,
        remove: Option<(String, ChannelId)>,
    ) -> api::Result<()> {
        self.users_channel_sections_channels_bulk_update(
            insert
                .as_ref()
                .map(|(section, channel)| (section.as_str(), channel.clone())),
            remove
                .as_ref()
                .map(|(section, channel)| (section.as_str(), channel.clone())),
        )
        .await
    }

    async fn mark_read(&self, channel: ChannelId, ts: Ts) -> api::Result<()> {
        self.conversations_mark(&channel, &ts).await
    }

    async fn mark_thread_read(&self, channel: ChannelId, thread_ts: Ts, ts: Ts) -> api::Result<()> {
        self.subscriptions_thread_mark(&channel, &thread_ts, &ts)
            .await
    }

    async fn mark_all_read(&self) -> api::Result<()> {
        self.users_mark_all_read().await
    }

    async fn set_thread_subscribed(
        &self,
        channel: ChannelId,
        thread_ts: Ts,
        last_read: Ts,
        subscribed: bool,
    ) -> api::Result<()> {
        self.subscriptions_thread_set(&channel, &thread_ts, &last_read, subscribed)
            .await
    }

    async fn channel_managers(&self, channel: ChannelId) -> api::Result<Vec<UserId>> {
        SlackApi::channel_managers(self, &channel).await
    }

    async fn pins(&self, channel: ChannelId) -> api::Result<Vec<PinnedItem>> {
        Ok(self.pins_list(&channel).await?.items)
    }

    async fn bookmarks(&self, channel: ChannelId) -> api::Result<Vec<ChannelBookmark>> {
        Ok(self.bookmarks_list(&channel).await?.bookmarks)
    }
    async fn files(&self, filter: FilesFilter) -> api::Result<Vec<File>> {
        let count = if filter.channel.is_some() {
            FILES_PAGE
        } else {
            FILES_WORKSPACE_PAGE
        };
        Ok(self.files_list(&filter, count, 1).await?.items)
    }

    async fn add_bookmark(
        &self,
        channel: ChannelId,
        title: String,
        url: String,
    ) -> api::Result<()> {
        self.bookmarks_add_link(&channel, &title, &url).await
    }

    async fn set_pinned(&self, channel: ChannelId, ts: Ts, pinned: bool) -> api::Result<()> {
        if pinned {
            self.pins_add(&channel, &ts).await
        } else {
            self.pins_remove(&channel, &ts).await
        }
    }

    async fn create_canvas(&self, channel: ChannelId) -> api::Result<String> {
        Ok(self
            .conversations_canvases_create(&channel)
            .await?
            .canvas_id)
    }

    async fn set_dnd_snooze(&self, minutes: u32) -> api::Result<DndStatus> {
        self.dnd_set_snooze(minutes).await
    }

    async fn end_dnd_snooze(&self) -> api::Result<DndStatus> {
        self.dnd_end_snooze().await
    }

    async fn schedule_message(
        &self,
        channel: ChannelId,
        post_at: u64,
        content: MessageContent,
        thread_ts: Option<Ts>,
    ) -> api::Result<ScheduledMessageCreated> {
        self.chat_schedule_message(&channel, post_at, &content, thread_ts.as_ref())
            .await
    }

    async fn scheduled_messages(&self) -> api::Result<Vec<ScheduledMessage>> {
        self.chat_scheduled_messages_list(None).await
    }

    async fn delete_scheduled_message(
        &self,
        channel: ChannelId,
        scheduled_message_id: String,
    ) -> api::Result<()> {
        self.chat_delete_scheduled_message(&channel, &scheduled_message_id)
            .await
    }

    async fn usergroups(&self) -> api::Result<Vec<Usergroup>> {
        self.usergroups_list().await
    }

    async fn upload_files(
        &self,
        channel: ChannelId,
        thread_ts: Option<Ts>,
        content: Option<MessageContent>,
        files: Vec<UploadFile>,
    ) -> api::Result<Vec<File>> {
        SlackApi::upload_files(self, &channel, thread_ts.as_ref(), content.as_ref(), &files).await
    }

    async fn gif_featured(&self, limit: u32) -> api::Result<Vec<api::GifItem>> {
        SlackApi::gif_featured(self, limit).await
    }

    async fn gif_search(&self, query: String, limit: u32) -> api::Result<Vec<api::GifItem>> {
        SlackApi::gif_search(self, &query, limit).await
    }

    async fn dnd_end_dnd(&self) -> api::Result<()> {
        SlackApi::dnd_end_dnd(self).await
    }

    async fn dnd_team_info(
        &self,
        users: Vec<UserId>,
    ) -> api::Result<HashMap<UserId, slackless_types::DndStatus>> {
        SlackApi::dnd_team_info(self, &users).await
    }

    async fn saved_list(&self, filter: &'static str) -> api::Result<Vec<api::SavedItem>> {
        SlackApi::saved_list(self, filter, 50).await
    }

    async fn saved_add(
        &self,
        channel: ChannelId,
        ts: Ts,
        date_due: Option<u64>,
    ) -> api::Result<()> {
        SlackApi::saved_add(self, &channel, &ts, date_due).await
    }

    async fn saved_update(
        &self,
        channel: ChannelId,
        ts: Ts,
        mark: Option<api::SavedMark>,
        date_due: Option<Option<u64>>,
    ) -> api::Result<()> {
        SlackApi::saved_update(self, &channel, &ts, mark, date_due).await
    }

    async fn saved_delete(&self, channel: ChannelId, ts: Ts) -> api::Result<()> {
        SlackApi::saved_delete(self, &channel, &ts).await
    }

    async fn saved_clear_completed(&self) -> api::Result<()> {
        SlackApi::saved_clear_completed(self).await
    }

    async fn app_commands(&self) -> api::Result<Vec<api::AppCommand>> {
        SlackApi::client_app_commands(self).await
    }

    async fn edge_search(
        &self,
        team_id: TeamId,
        query: String,
    ) -> api::Result<api::EdgeSearchResults> {
        let (users, channels) = tokio::join!(
            SlackApi::edge_users_search(self, &team_id, &query, 15),
            SlackApi::edge_channels_search(self, &team_id, &query, 15),
        );
        Ok(api::EdgeSearchResults {
            users: users.unwrap_or_default(),
            channels: channels.unwrap_or_default(),
        })
    }

    async fn search_remote_messages(
        &self,
        query: String,
        sort: api::SearchSort,
        page: usize,
    ) -> api::Result<api::RemoteMessagePage> {
        SlackApi::search_modules_messages(self, &query, sort, page).await
    }

    async fn search_remote_files(
        &self,
        query: String,
        sort: api::SearchSort,
        page: usize,
    ) -> api::Result<api::RemoteFilePage> {
        SlackApi::search_modules_files(self, &query, sort, page).await
    }

    async fn update_profile(
        &self,
        update: api::ProfileUpdate,
    ) -> api::Result<slackless_types::Profile> {
        SlackApi::users_profile_set(self, &update).await
    }

    async fn set_presence(&self, away: bool) -> api::Result<()> {
        SlackApi::presence_set(self, away).await
    }

    async fn create_channel(&self, name: String, is_private: bool) -> api::Result<Conversation> {
        SlackApi::conversations_create(self, &name, is_private).await
    }

    async fn rename_channel(&self, channel: ChannelId, name: String) -> api::Result<Conversation> {
        SlackApi::conversations_rename(self, &channel, &name).await
    }

    async fn archive_channel(&self, channel: ChannelId) -> api::Result<()> {
        SlackApi::conversations_archive(self, &channel).await
    }

    async fn unarchive_channel(&self, channel: ChannelId) -> api::Result<()> {
        SlackApi::conversations_unarchive(self, &channel).await
    }

    async fn invite_to_channel(
        &self,
        channel: ChannelId,
        users: Vec<UserId>,
    ) -> api::Result<Conversation> {
        SlackApi::conversations_invite(self, &channel, &users).await
    }

    async fn set_user_pref(&self, name: String, value: String) -> api::Result<()> {
        SlackApi::users_prefs_set(self, &name, &value).await
    }

    async fn user_profile(&self, user: UserId) -> api::Result<slackless_types::Profile> {
        SlackApi::users_profile_get(self, &user).await
    }

    async fn team_profile_fields(&self) -> api::Result<HashMap<String, String>> {
        SlackApi::team_profile_get(self).await
    }

    async fn views_submit(
        &self,
        view_id: String,
        hash: Option<String>,
        state_values: serde_json::Value,
    ) -> api::Result<()> {
        SlackApi::views_submit(self, &view_id, hash.as_deref(), &state_values).await
    }

    async fn views_close(&self, view_id: String, hash: Option<String>) -> api::Result<()> {
        SlackApi::views_close(self, &view_id, hash.as_deref()).await
    }

    async fn add_bookmark_in_folder(
        &self,
        channel: ChannelId,
        title: String,
        url: String,
        parent_id: String,
    ) -> api::Result<()> {
        self.bookmarks_add_link_in_folder(&channel, &title, &url, Some(&parent_id))
            .await
    }

    async fn remove_bookmark(&self, channel: ChannelId, bookmark_id: String) -> api::Result<()> {
        self.bookmarks_remove(&channel, &bookmark_id).await
    }

    async fn drafts_list(&self) -> api::Result<Vec<api::CloudDraft>> {
        SlackApi::drafts_list(self).await
    }

    async fn drafts_create(&self, channel: ChannelId, text: String) -> api::Result<Option<String>> {
        SlackApi::drafts_create(self, &channel, &text).await
    }

    async fn drafts_update(
        &self,
        draft_id: String,
        channel: ChannelId,
        text: String,
    ) -> api::Result<()> {
        SlackApi::drafts_update(self, &draft_id, &channel, &text).await
    }

    async fn drafts_delete(&self, draft_id: String) -> api::Result<()> {
        SlackApi::drafts_delete(self, &draft_id).await
    }

    async fn block_suggestions(
        &self,
        bot: BotId,
        app_id: String,
        team: TeamId,
        action_id: String,
        block_id: String,
        value: String,
        container: ActionContainer,
    ) -> api::Result<Vec<(String, String)>> {
        self.blocks_suggestions(
            &bot, &app_id, &team, &action_id, &block_id, &value, &container,
        )
        .await
    }

    async fn auth_revoke(&self) -> api::Result<()> {
        SlackApi::auth_revoke(self).await
    }

    async fn add_conversation_tab(
        &self,
        channel: ChannelId,
        tab_type: String,
        label: Option<String>,
    ) -> api::Result<()> {
        self.conversations_add_tab(&channel, &tab_type, label.as_deref())
            .await
    }

    async fn remove_conversation_tab(&self, channel: ChannelId, tab_id: String) -> api::Result<()> {
        self.conversations_remove_tab(&channel, &tab_id).await
    }

    async fn move_conversation_tab(
        &self,
        channel: ChannelId,
        tab_id: String,
        after: Option<String>,
    ) -> api::Result<()> {
        self.conversations_move_tab(&channel, &tab_id, after.as_deref())
            .await
    }

    async fn save_file(&self, file_id: String, date_due: Option<u64>) -> api::Result<()> {
        self.saved_add_file(&file_id, date_due).await
    }

    async fn delete_file(&self, file_id: String) -> api::Result<()> {
        self.files_delete(&file_id).await
    }

    async fn fetch_canvas_html(&self, file_id: String) -> api::Result<(String, String)> {
        SlackApi::fetch_canvas_html(self, &file_id).await
    }

    async fn canvases_create(
        &self,
        title: Option<String>,
        markdown: Option<String>,
        channel: Option<ChannelId>,
    ) -> api::Result<api::CanvasCreated> {
        SlackApi::canvases_create(
            self,
            title.as_deref(),
            markdown.as_deref(),
            channel.as_ref(),
        )
        .await
    }

    async fn canvases_edit(&self, file_id: String, op: api::CanvasEditOp) -> api::Result<()> {
        SlackApi::canvases_edit(self, &file_id, &op).await
    }

    async fn files_request_permission(
        &self,
        file_id: String,
        request_type: String,
    ) -> api::Result<()> {
        SlackApi::files_request_permission(self, &file_id, &request_type).await
    }

    async fn fetch_list(&self, file_id: String) -> api::Result<api::SlackListData> {
        SlackApi::fetch_list(self, &file_id).await
    }

    async fn lists_create(
        &self,
        name: String,
        description: Option<String>,
        channel: Option<ChannelId>,
    ) -> api::Result<String> {
        SlackApi::lists_create(self, &name, description.as_deref(), channel.as_ref()).await
    }

    async fn lists_records_create(
        &self,
        list_id: String,
        initial_fields: Vec<api::ListCellInput>,
    ) -> api::Result<api::SlackListRow> {
        SlackApi::lists_records_create(self, &list_id, &initial_fields, None).await
    }

    async fn lists_cells_update(
        &self,
        list_id: String,
        row_id: String,
        cells: Vec<api::ListCellInput>,
    ) -> api::Result<()> {
        SlackApi::lists_cells_update(self, &list_id, &row_id, &cells).await
    }

    async fn lists_records_delete(&self, list_id: String, row_id: String) -> api::Result<()> {
        SlackApi::lists_records_delete(self, &list_id, &row_id).await
    }

    async fn lists_add_to_list(
        &self,
        list_id: String,
        channel: ChannelId,
        ts: Ts,
    ) -> api::Result<String> {
        SlackApi::lists_add_to_list(self, &list_id, &channel, &ts).await
    }

    async fn load_app_home(&self, channel: ChannelId) -> api::Result<api::AppHomeResponse> {
        let home = self.conversations_app_home(&channel).await?;
        let _ = self.apps_home_dispatch_open_event(&channel).await;
        Ok(home)
    }

    async fn open_app_home(
        &self,
        app_id: String,
        team_id: String,
    ) -> api::Result<Option<ChannelId>> {
        self.apps_home_open(&app_id, &team_id).await
    }

    async fn trip_workflow_trigger(
        &self,
        url: String,
        context: serde_json::Value,
    ) -> api::Result<()> {
        self.workflows_triggers_trip(&url, &context).await
    }

    async fn set_global_notification_pref(
        &self,
        name: String,
        value: String,
        sync: bool,
    ) -> api::Result<()> {
        self.users_prefs_set_global_notification(&name, &value, sync)
            .await
    }

    async fn files_favorites_list(&self) -> api::Result<Vec<String>> {
        SlackApi::files_favorites_list(self).await
    }

    async fn files_favorites_set(
        &self,
        file_id: String,
        collection_id: Option<String>,
        starred: bool,
    ) -> api::Result<()> {
        SlackApi::files_favorites_set(self, &file_id, collection_id.as_deref(), starred).await
    }

    async fn files_collections_list(&self) -> api::Result<Vec<api::FileCollection>> {
        SlackApi::files_collections_list(self).await
    }

    async fn files_collections_create(
        &self,
        name: String,
        emoji: Option<String>,
    ) -> api::Result<Vec<api::FileCollection>> {
        SlackApi::files_collections_create(self, &name, emoji.as_deref()).await
    }

    async fn files_collections_delete(&self, collection_id: String) -> api::Result<()> {
        SlackApi::files_collections_delete(self, &collection_id).await
    }

    async fn files_recently_deleted(&self) -> api::Result<Vec<api::DeletedFile>> {
        SlackApi::files_recently_deleted(self).await
    }

    async fn files_undelete(&self, file_id: String) -> api::Result<()> {
        SlackApi::files_undelete(self, &file_id).await
    }

    async fn rooms_join(&self, channel: ChannelId) -> api::Result<api::RoomJoinResponse> {
        SlackApi::rooms_join(self, &channel, None, None).await
    }

    async fn rooms_notify_member(&self, channel: ChannelId, user: UserId) -> api::Result<()> {
        SlackApi::rooms_notify_member(self, &channel, &user).await
    }

    async fn rooms_get_link(&self, channel: ChannelId) -> api::Result<String> {
        Ok(SlackApi::rooms_get_link(self, &channel)
            .await?
            .unwrap_or_else(|| format!("https://app.slack.com/huddle/{channel}")))
    }

    async fn rooms_rename(&self, room_id: String, name: String) -> api::Result<()> {
        SlackApi::rooms_rename(self, &room_id, &name).await
    }

    async fn rooms_start_transcription(&self, call_id: String) -> api::Result<()> {
        SlackApi::rooms_start_transcription(self, &call_id).await
    }

    async fn rooms_stop_transcription(&self, call_id: String) -> api::Result<()> {
        SlackApi::rooms_stop_transcription(self, &call_id).await
    }

    async fn rooms_invite_response(
        &self,
        channel: ChannelId,
        room_id: String,
        response: String,
    ) -> api::Result<()> {
        SlackApi::rooms_invite_response(self, &channel, &room_id, &response).await
    }

    async fn huddles_update_background(
        &self,
        huddle_id: String,
        background_id: String,
    ) -> api::Result<()> {
        SlackApi::huddles_update_background(self, &huddle_id, &background_id).await
    }
}
