//! The seam between the core and Slack's REST API.

mod slack_api;
mod slack_api_helpers;

use std::collections::HashMap;
use std::future::Future;

#[cfg(doc)]
use crate::api::SlackApi;
use crate::api::{
    self, ActionContainer, ActivityEntry, ActivityFeed, ChannelBookmark, ClientCounts, FilesFilter,
    MessageContent, Page, PinnedItem, PostedMessage, ScheduledMessage, ScheduledMessageCreated,
    SearchMessages, SectionCreated, ThreadsView, UploadFile, UserBoot,
};
use crate::types::{
    BotId, BotProfile, ChannelId, ChannelSection, Conversation, DndStatus, File, Message, Team,
    TeamId, Ts, User, UserId, UserPresence, Usergroup,
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
    /// Convert a public channel or group DM into a private channel (`conversations.convertToPrivate`).
    fn convert_channel_to_private(
        &self,
        _channel: ChannelId,
    ) -> impl Future<Output = api::Result<Conversation>> + Send {
        async { Err(api::ApiError::Slack("unsupported".to_owned())) }
    }
    /// Create a `@handle` user group (`usergroups.create`).
    fn create_usergroup(
        &self,
        _name: String,
        _handle: String,
        _description: String,
    ) -> impl Future<Output = api::Result<Usergroup>> + Send {
        async { Err(api::ApiError::Slack("unsupported".to_owned())) }
    }
    /// Update a `@handle` user group (`usergroups.update`).
    fn update_usergroup(
        &self,
        _usergroup: String,
        _name: String,
        _handle: String,
        _description: String,
    ) -> impl Future<Output = api::Result<Usergroup>> + Send {
        async { Err(api::ApiError::Slack("unsupported".to_owned())) }
    }
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
    /// Permanently delete a channel (`admin.conversations.delete`, `F-CORE-053`).
    fn delete_channel(&self, _channel: ChannelId) -> impl Future<Output = api::Result<()>> + Send {
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
    /// Fetch a live Slack canvas's parsed metadata and HTML export (`files.info` -> `url_private`
    /// with `canvases.getContent` fallback). Defaults to delegating to [`Self::fetch_canvas_html`].
    fn fetch_canvas_with_meta(
        &self,
        file_id: String,
    ) -> impl Future<Output = api::Result<(api::CanvasMeta, String)>> + Send {
        async move {
            let (title, html) = self.fetch_canvas_html(file_id.clone()).await?;
            let meta = api::CanvasMeta {
                comment_channel_id: api::canvas_comment_channel_id(&file_id),
                file_id,
                title,
                ..Default::default()
            };
            Ok((meta, html))
        }
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
    /// Permanently delete a canvas (`canvases.delete`).
    fn canvases_delete(&self, _canvas_id: String) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Grant read/write/owner access on a canvas (`canvases.access.set`).
    fn canvases_access_set(
        &self,
        _canvas_id: String,
        _access_level: api::CanvasAccessLevel,
        _channel_ids: Vec<ChannelId>,
        _user_ids: Vec<UserId>,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Revoke canvas access from channels or users (`canvases.access.delete`).
    fn canvases_access_delete(
        &self,
        _canvas_id: String,
        _channel_ids: Vec<ChannelId>,
        _user_ids: Vec<UserId>,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Load comment threads on a canvas (`conversations.history` + `conversations.replies`).
    fn canvas_comments_list(
        &self,
        _canvas_id: String,
    ) -> impl Future<Output = api::Result<Vec<api::CanvasCommentThread>>> + Send {
        async { Ok(Vec::new()) }
    }
    /// Post a comment or reply into a canvas's comment channel (`chat.postMessage`).
    fn canvas_comment_post(
        &self,
        _canvas_id: String,
        _text: String,
        _thread_ts: Option<Ts>,
    ) -> impl Future<Output = api::Result<PostedMessage>> + Send {
        async { Ok(PostedMessage::default()) }
    }
    /// Resolve one canvas comment thread (`quip.thread.archive`).
    fn quip_thread_archive(
        &self,
        _channel_id: ChannelId,
        _thread_ts: Ts,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Reopen one resolved canvas comment thread (`quip.thread.unarchive`).
    fn quip_thread_unarchive(
        &self,
        _channel_id: ChannelId,
        _thread_ts: Ts,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Resolve every open canvas comment thread (`quip.thread.archiveAll`).
    fn quip_thread_archive_all(
        &self,
        _channel_id: ChannelId,
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
    /// Archive an item in a Slack List (`lists.records.archive`, `F-COLLAB-255`).
    fn lists_records_archive(
        &self,
        _list_id: String,
        _row_id: String,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Update sharing/access permissions on a Slack List (`slackLists.access.set`, `F-COLLAB-243`).
    fn lists_access_set(
        &self,
        _list_id: String,
        _access_level: String,
        _channel: Option<ChannelId>,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Load comment thread messages for a List item (`lists.getThreadsForRecord`, `F-COLLAB-250`).
    fn lists_get_threads_for_record(
        &self,
        _list_id: String,
        _record_id: String,
    ) -> impl Future<Output = api::Result<Vec<Message>>> + Send {
        async { Ok(Vec::new()) }
    }
    /// Upload a new profile photo (`users.setPhoto`, `F-SHELL-004`).
    fn users_set_photo(
        &self,
        _filename: String,
        _bytes: Vec<u8>,
    ) -> impl Future<Output = api::Result<()>> + Send {
        async { Ok(()) }
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
