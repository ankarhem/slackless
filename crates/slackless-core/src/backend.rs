//! The seam between the core and Slack's REST API.

use std::collections::HashMap;
use std::future::Future;

use crate::api::{
    self, ChannelBookmark, ClientCounts, HistoryOpts, ListOpts, MessageContent, Page, PinnedItem,
    PostedMessage, SearchMessages, SearchOpts, SectionCreated, SlackApi, ThreadsView, UserBoot,
};
use crate::types::{
    ChannelId, ChannelSection, Conversation, ConversationKind, DndStatus, File, Message, Ts, User,
    UserId, UserPresence,
};

/// `conversations.history` page size the core requests (Slack max 999).
pub(crate) const HISTORY_PAGE_LIMIT: u32 = 200;

/// `subscriptions.thread.getView` page size the core requests.
pub(crate) const THREADS_PAGE_LIMIT: u32 = 50;
/// `files.list` page size the core requests for a conversation's Files & links tab.
pub(crate) const FILES_PAGE: u32 = 100;

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
    /// A user's do-not-disturb state (`dnd.info`). Read-only.
    fn dnd_info(&self, user: UserId) -> impl Future<Output = api::Result<DndStatus>> + Send;
    /// One user's profile (`users.info`), reaching people `users.list` omits. Read-only.
    fn user_info(&self, user: UserId) -> impl Future<Output = api::Result<User>> + Send;
    /// The signed-in user leaves `channel` (`conversations.leave`).
    fn leave_conversation(
        &self,
        channel: ChannelId,
    ) -> impl Future<Output = api::Result<()>> + Send;
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
        text: String,
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
    /// The followed threads of the Threads view (`subscriptions.thread.getView`), page one.
    fn threads_view(&self) -> impl Future<Output = api::Result<ThreadsView>> + Send;
    /// Open (or find) the DM with `user` (`conversations.open`).
    fn open_dm(&self, user: UserId) -> impl Future<Output = api::Result<Conversation>> + Send;
    /// Create a sidebar channel section (`users.channelSections.create`).
    fn create_section(
        &self,
        name: String,
    ) -> impl Future<Output = api::Result<SectionCreated>> + Send;
    fn search(&self, query: String) -> impl Future<Output = api::Result<SearchMessages>> + Send;
    fn mark_read(&self, channel: ChannelId, ts: Ts)
    -> impl Future<Output = api::Result<()>> + Send;
    /// A conversation's pinned items (`pins.list`), oldest pin first. Read-only.
    fn pins(&self, channel: ChannelId)
    -> impl Future<Output = api::Result<Vec<PinnedItem>>> + Send;
    /// A conversation's bookmarks (`bookmarks.list`), folders included. Read-only.
    fn bookmarks(
        &self,
        channel: ChannelId,
    ) -> impl Future<Output = api::Result<Vec<ChannelBookmark>>> + Send;
    /// The files shared with a conversation (`files.list`), newest first. Read-only.
    fn files(&self, channel: ChannelId) -> impl Future<Output = api::Result<Vec<File>>> + Send;
    /// Add a link bookmark to a conversation's bar (`bookmarks.add`).
    fn add_bookmark(
        &self,
        channel: ChannelId,
        title: String,
        url: String,
    ) -> impl Future<Output = api::Result<()>> + Send;
    /// Create a conversation's empty channel canvas (`conversations.canvases.create`),
    /// returning the canvas file id.
    fn create_canvas(&self, channel: ChannelId)
    -> impl Future<Output = api::Result<String>> + Send;
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

    async fn dnd_info(&self, user: UserId) -> api::Result<DndStatus> {
        SlackApi::dnd_info(self, &user).await
    }

    async fn user_info(&self, user: UserId) -> api::Result<User> {
        self.users_info(&user).await
    }

    async fn leave_conversation(&self, channel: ChannelId) -> api::Result<()> {
        self.conversations_leave(&channel).await
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
        text: String,
    ) -> api::Result<PostedMessage> {
        self.chat_update(&channel, &ts, &MessageContent::plain(text))
            .await
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

    async fn search(&self, query: String) -> api::Result<SearchMessages> {
        self.search_messages(&query, &SearchOpts::default()).await
    }

    async fn threads_view(&self) -> api::Result<ThreadsView> {
        self.subscriptions_thread_get_view(THREADS_PAGE_LIMIT).await
    }

    async fn open_dm(&self, user: UserId) -> api::Result<Conversation> {
        self.conversations_open(user.as_str()).await
    }

    async fn create_section(&self, name: String) -> api::Result<SectionCreated> {
        self.users_channel_sections_create(&name, "").await
    }

    async fn mark_read(&self, channel: ChannelId, ts: Ts) -> api::Result<()> {
        self.conversations_mark(&channel, &ts).await
    }

    async fn pins(&self, channel: ChannelId) -> api::Result<Vec<PinnedItem>> {
        Ok(self.pins_list(&channel).await?.items)
    }

    async fn bookmarks(&self, channel: ChannelId) -> api::Result<Vec<ChannelBookmark>> {
        Ok(self.bookmarks_list(&channel).await?.bookmarks)
    }
    async fn files(&self, channel: ChannelId) -> api::Result<Vec<File>> {
        Ok(self.files_list(&channel, FILES_PAGE, 1).await?.items)
    }

    async fn add_bookmark(
        &self,
        channel: ChannelId,
        title: String,
        url: String,
    ) -> api::Result<()> {
        self.bookmarks_add_link(&channel, &title, &url).await
    }

    async fn create_canvas(&self, channel: ChannelId) -> api::Result<String> {
        Ok(self
            .conversations_canvases_create(&channel)
            .await?
            .canvas_id)
    }
}
