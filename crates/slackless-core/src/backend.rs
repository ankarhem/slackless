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
        channel: ChannelId,
        ts: Ts,
        content: MessageContent,
    ) -> impl Future<Output = api::Result<PostedMessage>> + Send;
    fn delete_message(
        &self,
        channel: ChannelId,
        ts: Ts,
    ) -> impl Future<Output = api::Result<()>> + Send;
    fn add_reaction(
        &self,
        channel: ChannelId,
        ts: Ts,
        name: String,
    ) -> impl Future<Output = api::Result<()>> + Send;
    fn remove_reaction(
        &self,
        channel: ChannelId,
        ts: Ts,
        name: String,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// Run a slash command Slack handles (`chat.command`) in `channel`, or in the thread at
    /// `thread_ts`; `team` is the signed-in workspace.
    fn run_command(
        &self,
        channel: ChannelId,
        command: String,
        args: String,
        thread_ts: Option<Ts>,
        team: TeamId,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// Dispatch a Block Kit interaction to the app that posted the message (`blocks.actions`).
    fn block_action(
        &self,
        bot: BotId,
        app_id: String,
        team: TeamId,
        action: serde_json::Value,
        container: ActionContainer,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// A bot's profile (`bots.info`), for messages that carry only its id.
    fn bot_info(&self, bot: BotId) -> impl Future<Output = api::Result<BotProfile>> + Send;
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
    fn usergroups(&self) -> impl Future<Output = api::Result<Vec<Usergroup>>> + Send;
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
        limit: u32,
    ) -> impl Future<Output = api::Result<Vec<api::GifItem>>> + Send;
    /// Search GIFs for the composer picker (`gif.search`).
    fn gif_search(
        &self,
        query: String,
        limit: u32,
    ) -> impl Future<Output = api::Result<Vec<api::GifItem>>> + Send;
    /// Saved-for-Later items (`saved.list`) under `filter` (`"saved"`, `"completed"`, `"archived"`).
    fn saved_list(
        &self,
        filter: &'static str,
    ) -> impl Future<Output = api::Result<Vec<api::SavedItem>>> + Send;
    /// Save a message for Later (`saved.add`), optionally with a reminder (`date_due`).
    fn saved_add(
        &self,
        channel: ChannelId,
        ts: Ts,
        date_due: Option<u64>,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// Update a saved item's state (`mark`) and/or reminder (`date_due`).
    fn saved_update(
        &self,
        channel: ChannelId,
        ts: Ts,
        mark: Option<api::SavedMark>,
        date_due: Option<Option<u64>>,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// Remove a message from Later (`saved.delete`).
    fn saved_delete(
        &self,
        channel: ChannelId,
        ts: Ts,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// Clear all completed Later items (`saved.clearCompleted`).
    fn saved_clear_completed(&self) -> impl Future<Output = api::Result<()>> + Send;
    /// Workspace slash command catalog (`client.appCommands`).
    fn app_commands(&self) -> impl Future<Output = api::Result<Vec<api::AppCommand>>> + Send;
    /// Fast remote typeahead (`edgeapi` `users/search` + `channels/search`) for Cmd+K.
    fn edge_search(
        &self,
        team_id: TeamId,
        query: String,
    ) -> impl Future<Output = api::Result<api::EdgeSearchResults>> + Send;
    /// Opt-in remote Slack message search (`search.modules.messages`).
    fn search_remote_messages(
        &self,
        query: String,
        sort: api::SearchSort,
        page: usize,
    ) -> impl Future<Output = api::Result<api::RemoteMessagePage>> + Send;
    /// Opt-in remote Slack file search (`search.modules.files`).
    fn search_remote_files(
        &self,
        query: String,
        sort: api::SearchSort,
        page: usize,
    ) -> impl Future<Output = api::Result<api::RemoteFilePage>> + Send;
    /// Update the signed-in user's profile or custom status (`users.profile.set`).
    fn update_profile(
        &self,
        update: api::ProfileUpdate,
    ) -> impl Future<Output = api::Result<slackless_types::Profile>> + Send;
    /// Manually set presence active or away (`presence.set`).
    fn set_presence(&self, away: bool) -> impl Future<Output = api::Result<()>> + Send;
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
    ) -> impl Future<Output = api::Result<Conversation>> + Send;
    /// Archive a channel (`conversations.archive`).
    fn archive_channel(&self, channel: ChannelId) -> impl Future<Output = api::Result<()>> + Send;
    /// Unarchive a channel (`conversations.unarchive`).
    fn unarchive_channel(&self, channel: ChannelId)
    -> impl Future<Output = api::Result<()>> + Send;
    /// Invite users to a channel (`conversations.invite`).
    fn invite_to_channel(
        &self,
        channel: ChannelId,
        users: Vec<UserId>,
    ) -> impl Future<Output = api::Result<Conversation>> + Send;
    /// Write a single user preference back to Slack (`users.prefs.set`).
    fn set_user_pref(
        &self,
        name: String,
        value: String,
    ) -> impl Future<Output = api::Result<()>> + Send;
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
}
