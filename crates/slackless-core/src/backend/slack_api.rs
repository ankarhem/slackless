//! [`Backend`] implementation backed by the real [`SlackApi`] HTTP client.

use std::collections::HashMap;

use crate::api::{
    self, ActionContainer, ActivityEntry, ActivityFeed, ChannelBookmark, ClientCounts, FilesFilter,
    MessageContent, Page, PinnedItem, PostedMessage, ScheduledMessage, ScheduledMessageCreated,
    SearchMessages, SearchOpts, SectionCreated, SlackApi, ThreadsView, UploadFile, UserBoot,
};
use crate::types::{
    BotId, BotProfile, ChannelId, ChannelSection, Conversation, DndStatus, File, Message, Team,
    TeamId, Ts, User, UserId, UserPresence, Usergroup,
};

use super::slack_api_helpers as helpers;
use super::{ACTIVITY_PAGE_LIMIT, Backend, THREADS_PAGE_LIMIT};

impl Backend for SlackApi {
    // --- Boot, workspace & sidebar ---
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
        helpers::channel_memberships(self).await
    }
    async fn team_info(&self, team: TeamId) -> api::Result<Team> {
        SlackApi::team_info(self, &team).await
    }
    async fn app_commands(&self) -> api::Result<Vec<api::AppCommand>> {
        SlackApi::client_app_commands(self).await
    }
    async fn auth_revoke(&self) -> api::Result<()> {
        SlackApi::auth_revoke(self).await
    }

    // --- Conversations, history, sections & tabs ---
    async fn history_page(
        &self,
        channel: ChannelId,
        latest: Option<Ts>,
        cursor: Option<String>,
    ) -> api::Result<Page<Message>> {
        helpers::history_page(self, channel, latest, cursor).await
    }
    async fn replies_page(
        &self,
        channel: ChannelId,
        thread_ts: Ts,
        cursor: Option<String>,
    ) -> api::Result<Page<Message>> {
        helpers::replies_page(self, channel, thread_ts, cursor).await
    }
    async fn conversation_info(&self, channel: ChannelId) -> api::Result<Conversation> {
        self.conversations_info(&channel).await
    }
    async fn conversation_members(&self, channel: ChannelId) -> api::Result<Vec<UserId>> {
        self.conversations_members_all(&channel).await
    }
    async fn latest_message(&self, channel: ChannelId) -> api::Result<Option<Message>> {
        helpers::latest_message(self, channel).await
    }
    async fn recent_history(&self, channel: ChannelId, limit: u32) -> api::Result<Vec<Message>> {
        helpers::recent_history(self, channel, limit).await
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
    async fn channel_managers(&self, channel: ChannelId) -> api::Result<Vec<UserId>> {
        SlackApi::channel_managers(self, &channel).await
    }
    async fn open_dm(&self, users: Vec<UserId>) -> api::Result<Conversation> {
        helpers::open_dm(self, users).await
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
    async fn delete_channel(&self, channel: ChannelId) -> api::Result<()> {
        SlackApi::admin_conversations_delete(self, &channel).await
    }
    async fn convert_channel_to_private(&self, channel: ChannelId) -> api::Result<Conversation> {
        self.conversations_convert_to_private(&channel).await
    }
    async fn invite_to_channel(
        &self,
        channel: ChannelId,
        users: Vec<UserId>,
    ) -> api::Result<Conversation> {
        SlackApi::conversations_invite(self, &channel, &users).await
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
        helpers::move_channel(self, insert, remove).await
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

    // --- Messaging, reactions, threads, activity, drafts, scheduled, pins, bookmarks, saved ---
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
    async fn pins(&self, channel: ChannelId) -> api::Result<Vec<PinnedItem>> {
        Ok(self.pins_list(&channel).await?.items)
    }
    async fn set_pinned(&self, channel: ChannelId, ts: Ts, pinned: bool) -> api::Result<()> {
        helpers::set_pinned(self, channel, ts, pinned).await
    }
    async fn bookmarks(&self, channel: ChannelId) -> api::Result<Vec<ChannelBookmark>> {
        Ok(self.bookmarks_list(&channel).await?.bookmarks)
    }
    async fn add_bookmark(
        &self,
        channel: ChannelId,
        title: String,
        url: String,
    ) -> api::Result<()> {
        self.bookmarks_add_link(&channel, &title, &url).await
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
    async fn save_file(&self, file_id: String, date_due: Option<u64>) -> api::Result<()> {
        self.saved_add_file(&file_id, date_due).await
    }

    // --- Users, bots, profiles, presence, prefs, DND & usergroups ---
    async fn user_info(&self, user: UserId) -> api::Result<User> {
        self.users_info(&user).await
    }
    async fn bot_info(&self, bot: BotId) -> api::Result<BotProfile> {
        self.bots_info(&bot).await
    }
    async fn user_presence(&self, user: UserId) -> api::Result<UserPresence> {
        self.users_get_presence(&user).await
    }
    async fn set_presence(&self, away: bool) -> api::Result<()> {
        SlackApi::presence_set(self, away).await
    }
    async fn update_profile(
        &self,
        update: api::ProfileUpdate,
    ) -> api::Result<slackless_types::Profile> {
        SlackApi::users_profile_set(self, &update).await
    }
    async fn user_profile(&self, user: UserId) -> api::Result<slackless_types::Profile> {
        SlackApi::users_profile_get(self, &user).await
    }
    async fn team_profile_fields(&self) -> api::Result<HashMap<String, String>> {
        SlackApi::team_profile_get(self).await
    }
    async fn users_set_photo(&self, filename: String, bytes: Vec<u8>) -> api::Result<()> {
        SlackApi::users_set_photo(self, &filename, &bytes).await
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
    async fn set_global_notification_pref(
        &self,
        name: String,
        value: String,
        sync: bool,
    ) -> api::Result<()> {
        self.users_prefs_set_global_notification(&name, &value, sync)
            .await
    }
    async fn set_user_pref(&self, name: String, value: String) -> api::Result<()> {
        SlackApi::users_prefs_set(self, &name, &value).await
    }
    async fn dnd_info(&self, user: UserId) -> api::Result<DndStatus> {
        SlackApi::dnd_info(self, &user).await
    }
    async fn set_dnd_snooze(&self, minutes: u32) -> api::Result<DndStatus> {
        self.dnd_set_snooze(minutes).await
    }
    async fn end_dnd_snooze(&self) -> api::Result<DndStatus> {
        self.dnd_end_snooze().await
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
    async fn create_usergroup(
        &self,
        name: String,
        handle: String,
        description: String,
    ) -> api::Result<Usergroup> {
        self.usergroups_create(&name, &handle, &description).await
    }
    async fn update_usergroup(
        &self,
        usergroup: String,
        name: String,
        handle: String,
        description: String,
    ) -> api::Result<Usergroup> {
        self.usergroups_update(&usergroup, &name, &handle, &description)
            .await
    }
    async fn usergroups(&self) -> api::Result<Vec<Usergroup>> {
        self.usergroups_list().await
    }

    // --- Search, GIFs, files, canvases, lists, app home, workflows & modals ---
    async fn search(&self, query: String) -> api::Result<SearchMessages> {
        self.search_messages(&query, &SearchOpts::default()).await
    }
    async fn edge_search(
        &self,
        team_id: TeamId,
        query: String,
    ) -> api::Result<api::EdgeSearchResults> {
        helpers::edge_search(self, team_id, query).await
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
    async fn gif_featured(&self, limit: u32) -> api::Result<Vec<api::GifItem>> {
        SlackApi::gif_featured(self, limit).await
    }
    async fn gif_search(&self, query: String, limit: u32) -> api::Result<Vec<api::GifItem>> {
        SlackApi::gif_search(self, &query, limit).await
    }
    async fn files(&self, filter: FilesFilter) -> api::Result<Vec<File>> {
        helpers::files(self, filter).await
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
    async fn delete_file(&self, file_id: String) -> api::Result<()> {
        self.files_delete(&file_id).await
    }
    async fn files_request_permission(
        &self,
        file_id: String,
        request_type: String,
    ) -> api::Result<()> {
        SlackApi::files_request_permission(self, &file_id, &request_type).await
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
    async fn create_canvas(&self, channel: ChannelId) -> api::Result<String> {
        helpers::create_canvas(self, channel).await
    }
    async fn fetch_canvas_html(&self, file_id: String) -> api::Result<(String, String)> {
        SlackApi::fetch_canvas_html(self, &file_id).await
    }
    async fn fetch_canvas_with_meta(
        &self,
        file_id: String,
    ) -> api::Result<(api::CanvasMeta, String)> {
        SlackApi::fetch_canvas_with_meta(self, &file_id).await
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
    async fn canvases_delete(&self, canvas_id: String) -> api::Result<()> {
        SlackApi::canvases_delete(self, &canvas_id).await
    }
    async fn canvases_access_set(
        &self,
        canvas_id: String,
        access_level: api::CanvasAccessLevel,
        channel_ids: Vec<ChannelId>,
        user_ids: Vec<UserId>,
    ) -> api::Result<()> {
        SlackApi::canvases_access_set(self, &canvas_id, access_level, &channel_ids, &user_ids).await
    }
    async fn canvases_access_delete(
        &self,
        canvas_id: String,
        channel_ids: Vec<ChannelId>,
        user_ids: Vec<UserId>,
    ) -> api::Result<()> {
        SlackApi::canvases_access_delete(self, &canvas_id, &channel_ids, &user_ids).await
    }
    async fn canvas_comments_list(
        &self,
        canvas_id: String,
    ) -> api::Result<Vec<api::CanvasCommentThread>> {
        SlackApi::canvas_comments_list(self, &canvas_id).await
    }
    async fn canvas_comment_post(
        &self,
        canvas_id: String,
        text: String,
        thread_ts: Option<Ts>,
    ) -> api::Result<PostedMessage> {
        SlackApi::canvas_comment_post(self, &canvas_id, &text, thread_ts.as_ref()).await
    }
    async fn quip_thread_archive(&self, channel_id: ChannelId, thread_ts: Ts) -> api::Result<()> {
        SlackApi::quip_thread_archive(self, &channel_id, &thread_ts).await
    }
    async fn quip_thread_unarchive(&self, channel_id: ChannelId, thread_ts: Ts) -> api::Result<()> {
        SlackApi::quip_thread_unarchive(self, &channel_id, &thread_ts).await
    }
    async fn quip_thread_archive_all(&self, channel_id: ChannelId) -> api::Result<()> {
        SlackApi::quip_thread_archive_all(self, &channel_id).await
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
    async fn lists_records_archive(&self, list_id: String, row_id: String) -> api::Result<()> {
        SlackApi::lists_records_archive(self, &list_id, &row_id).await
    }
    async fn lists_access_set(
        &self,
        list_id: String,
        access_level: String,
        channel: Option<ChannelId>,
    ) -> api::Result<()> {
        SlackApi::lists_access_set(self, &list_id, &access_level, channel.as_ref()).await
    }
    async fn lists_get_threads_for_record(
        &self,
        list_id: String,
        record_id: String,
    ) -> api::Result<Vec<Message>> {
        SlackApi::lists_get_threads_for_record(self, &list_id, &record_id).await
    }
    async fn load_app_home(&self, channel: ChannelId) -> api::Result<api::AppHomeResponse> {
        helpers::load_app_home(self, channel).await
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

    // --- Huddles ---
    async fn rooms_join(&self, channel: ChannelId) -> api::Result<api::RoomJoinResponse> {
        SlackApi::rooms_join(self, &channel, None, None).await
    }
    async fn rooms_notify_member(&self, channel: ChannelId, user: UserId) -> api::Result<()> {
        SlackApi::rooms_notify_member(self, &channel, &user).await
    }
    async fn rooms_get_link(&self, channel: ChannelId) -> api::Result<String> {
        helpers::rooms_get_link(self, channel).await
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
