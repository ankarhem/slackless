//! Front-end `Command` dispatch and domain-scoped command handlers.

mod channels;
mod collab;
pub(super) mod huddle;
mod messaging;
mod offline;

#[cfg(test)]
pub(super) use huddle::spawn_huddle_watcher;
pub(super) use offline::QueuedMutation;

use super::*;

impl<B: Backend> Core<B> {
    pub(super) async fn handle(&mut self, command: Command) {
        // Stealth mode MUST be enforced before `enqueue_or_run` so offline `MarkRead` commands
        // are never persisted into the offline queue and flushed on reconnect.
        if self.stealth_mode.load(Ordering::Relaxed)
            && matches!(
                command,
                Command::Messaging(
                    MessagingCommand::MarkRead { .. }
                        | MessagingCommand::MarkThreadRead { .. }
                        | MessagingCommand::MarkAllRead
                        | MessagingCommand::Typing { .. }
                )
            )
        {
            return;
        }
        if self.enqueue_or_run(&command).await {
            return;
        }
        match command {
            Command::Messaging(cmd) => self.handle_messaging(cmd).await,
            Command::Channel(cmd) => self.handle_channel(cmd),
            Command::Collab(cmd) => self.handle_collab(cmd),
            Command::Huddle(cmd) => self.handle_huddle(cmd),
            Command::Workspace(cmd) => self.handle_workspace(cmd),
            Command::Shutdown => {}
        }
    }

    async fn handle_messaging(&mut self, command: MessagingCommand) {
        match command {
            MessagingCommand::SendMessage {
                channel,
                content,
                thread_ts,
                broadcast,
            } => self.send_message(channel, content, thread_ts, broadcast),
            MessagingCommand::EditMessage {
                channel,
                ts,
                content,
            } => self.edit_message(channel, ts, content),
            MessagingCommand::DeleteMessage { channel, ts } => self.delete_message(channel, ts),
            MessagingCommand::AddReaction { channel, ts, name } => {
                self.add_reaction(channel, ts, name);
            }
            MessagingCommand::RemoveReaction { channel, ts, name } => {
                self.remove_reaction(channel, ts, name);
            }
            MessagingCommand::SetPinned {
                channel,
                ts,
                pinned,
            } => self.set_pinned(channel, ts, pinned),
            MessagingCommand::LoadPins(channel) => self.load_pins(channel),
            MessagingCommand::LoadBookmarks(channel) => self.load_bookmarks(channel),
            MessagingCommand::AddBookmark {
                channel,
                title,
                url,
            } => self.add_bookmark(channel, title, url),
            MessagingCommand::AddBookmarkInFolder {
                channel,
                title,
                url,
                parent_id,
            } => self.add_bookmark_in_folder(channel, title, url, parent_id),
            MessagingCommand::RemoveBookmark {
                channel,
                bookmark_id,
            } => self.remove_bookmark(channel, bookmark_id),
            MessagingCommand::ScheduleMessage {
                channel,
                post_at,
                content,
                thread_ts,
            } => self.schedule_message(channel, post_at, content, thread_ts),
            MessagingCommand::LoadScheduledMessages => self.load_scheduled_messages(),
            MessagingCommand::DeleteScheduledMessage {
                channel,
                scheduled_message_id,
            } => self.delete_scheduled_message(channel, scheduled_message_id),
            MessagingCommand::SyncDraft { channel, text } => self.sync_draft(channel, text),
            MessagingCommand::FetchCloudDrafts => self.fetch_cloud_drafts(),
            MessagingCommand::MarkRead { channel, ts } => self.mark_read(channel, ts),
            MessagingCommand::MarkThreadRead {
                channel,
                thread_ts,
                ts,
            } => self.mark_thread_read(channel, thread_ts, ts),
            MessagingCommand::MarkAllRead => self.mark_all_read(),
            MessagingCommand::Typing { channel, thread_ts } => {
                self.send_typing(channel, thread_ts);
            }
            MessagingCommand::OpenThread { channel, thread_ts } => {
                self.open_thread(channel, thread_ts);
            }
            MessagingCommand::SetThreadSubscribed {
                channel,
                thread_ts,
                last_read,
                subscribed,
            } => self.set_thread_subscribed(channel, thread_ts, last_read, subscribed),
            MessagingCommand::FetchThreads => self.fetch_threads(),
            MessagingCommand::FetchActivity { with_bodies } => self.fetch_activity(with_bodies),
            MessagingCommand::MarkActivity { entries, read } => self.mark_activity(entries, read),
            MessagingCommand::LoadSaved => self.fetch_saved(),
            MessagingCommand::SaveItem {
                channel,
                ts,
                date_due,
            } => self.save_item(channel, ts, date_due),
            MessagingCommand::SaveFile { file_id, date_due } => self.save_file(file_id, date_due),
            MessagingCommand::UpdateSavedItem {
                channel,
                ts,
                mark,
                date_due,
            } => self.update_saved_item(channel, ts, mark, date_due),
            MessagingCommand::DeleteSavedItem { channel, ts } => {
                self.delete_saved_item(channel, ts);
            }
            MessagingCommand::ClearCompletedSaved => self.clear_completed_saved(),
            MessagingCommand::RunCommand {
                channel,
                command,
                args,
                thread_ts,
            } => self.run_command(channel, command, args, thread_ts),
            MessagingCommand::RetryOfflineMutation(id) => self.retry_offline_mutation(id).await,
            MessagingCommand::DiscardOfflineMutation(id) => {
                self.discard_offline_mutation(id).await;
            }
        }
    }

    fn handle_channel(&mut self, command: ChannelCommand) {
        match command {
            ChannelCommand::OpenConversation(channel) => self.open_conversation(channel),
            ChannelCommand::FetchLatestHistory(channel) => self.fetch_latest_history(channel),
            ChannelCommand::CloseConversation(channel) => self.close_conversation(&channel),
            ChannelCommand::LeaveConversation(channel) => self.leave_conversation(channel),
            ChannelCommand::JoinConversation(channel) => self.join_conversation(channel),
            ChannelCommand::SetNotifyLevel { channel, level } => {
                self.set_notify_level(channel, level);
            }
            ChannelCommand::FetchChannelManagers { channel } => {
                self.fetch_channel_managers(channel);
            }
            ChannelCommand::OpenDm { users } => self.open_dm(users),
            ChannelCommand::CreateSection { name } => self.create_section(name),
            ChannelCommand::CreateSectionWithChannel { name, channel } => {
                self.create_section_with_channel(name, channel);
            }
            ChannelCommand::RenameSection { section, name } => self.rename_section(section, name),
            ChannelCommand::DeleteSection { section } => self.delete_section(section),
            ChannelCommand::MoveChannel { channel, from, to } => {
                self.move_channel(channel, from, to);
            }
            ChannelCommand::SetChannelTopic { channel, topic } => {
                self.set_channel_topic(channel, topic);
            }
            ChannelCommand::SetChannelPurpose { channel, purpose } => {
                self.set_channel_purpose(channel, purpose);
            }
            ChannelCommand::CreateChannel {
                name,
                is_private,
                purpose,
            } => self.create_channel(name, is_private, purpose),
            ChannelCommand::RenameChannel { channel, name } => self.rename_channel(channel, name),
            ChannelCommand::ArchiveChannel(channel) => self.archive_channel(channel),
            ChannelCommand::UnarchiveChannel(channel) => self.unarchive_channel(channel),
            ChannelCommand::DeleteChannel(channel) => self.delete_channel(channel),
            ChannelCommand::ConvertChannelToPrivate(channel) => {
                self.convert_channel_to_private(channel);
            }
            ChannelCommand::InviteToChannel { channel, users } => {
                self.invite_to_channel(channel, users);
            }
            ChannelCommand::AddConversationTab {
                channel,
                tab_type,
                label,
            } => self.add_conversation_tab(channel, tab_type, label),
            ChannelCommand::RemoveConversationTab { channel, tab_id } => {
                self.remove_conversation_tab(channel, tab_id);
            }
            ChannelCommand::MoveConversationTab {
                channel,
                tab_id,
                after,
            } => self.move_conversation_tab(channel, tab_id, after),
        }
    }

    fn handle_collab(&mut self, command: CollabCommand) {
        match command {
            CollabCommand::Search(query) => self.search(query),
            CollabCommand::QuickSwitchSearch { query } => self.quick_switch_search(query),
            CollabCommand::SearchRemoteMessages { query, sort, page } => {
                self.search_remote_messages(query, sort, page);
            }
            CollabCommand::SearchRemoteFiles { query, sort, page } => {
                self.search_remote_files(query, sort, page);
            }
            CollabCommand::SearchGifs { query } => self.search_gifs(query),
            CollabCommand::LoadFiles(filter) => self.load_files(filter),
            CollabCommand::UploadFiles {
                channel,
                thread_ts,
                content,
                files,
            } => self.upload_files(channel, thread_ts, content, files),
            CollabCommand::DeleteFile { file_id } => self.delete_file(file_id),
            CollabCommand::RequestFilePermission {
                file_id,
                request_type,
            } => self.request_file_permission(file_id, request_type),
            CollabCommand::LoadFilesExtras => self.load_files_extras(),
            CollabCommand::SetFileFavorite {
                file_id,
                collection_id,
                starred,
            } => self.set_file_favorite(file_id, collection_id, starred),
            CollabCommand::CreateFileCollection { name, emoji } => {
                self.create_file_collection(name, emoji);
            }
            CollabCommand::DeleteFileCollection { collection_id } => {
                self.delete_file_collection(collection_id);
            }
            CollabCommand::RestoreDeletedFile { file_id } => self.restore_deleted_file(file_id),
            CollabCommand::CreateChannelCanvas(channel) => self.create_channel_canvas(channel),
            CollabCommand::LoadCanvas { file_id } => self.load_canvas(file_id),
            CollabCommand::CreateCanvas {
                title,
                markdown,
                channel,
            } => self.create_canvas(title, markdown, channel),
            CollabCommand::EditCanvas { file_id, op } => self.edit_canvas(file_id, op),
            CollabCommand::DeleteCanvas { canvas_id } => self.delete_canvas(canvas_id),
            CollabCommand::SetCanvasAccess {
                canvas_id,
                access_level,
                channel_ids,
                user_ids,
                post_permalink,
            } => self.set_canvas_access(
                canvas_id,
                access_level,
                channel_ids,
                user_ids,
                post_permalink,
            ),
            CollabCommand::DeleteCanvasAccess {
                canvas_id,
                channel_ids,
                user_ids,
            } => self.delete_canvas_access(canvas_id, channel_ids, user_ids),
            CollabCommand::LoadCanvasComments { canvas_id } => self.load_canvas_comments(canvas_id),
            CollabCommand::PostCanvasComment {
                canvas_id,
                text,
                thread_ts,
            } => self.post_canvas_comment(canvas_id, text, thread_ts),
            CollabCommand::ResolveCanvasComment {
                canvas_id,
                thread_ts,
                resolved,
            } => self.resolve_canvas_comment(canvas_id, thread_ts, resolved),
            CollabCommand::ResolveAllCanvasComments { canvas_id } => {
                self.resolve_all_canvas_comments(canvas_id);
            }
            CollabCommand::LoadList { file_id } => self.load_list(file_id),
            CollabCommand::CreateList {
                name,
                description,
                channel,
            } => self.create_list(name, description, channel),
            CollabCommand::CreateListItem {
                list_id,
                initial_fields,
            } => self.create_list_item(list_id, initial_fields),
            CollabCommand::UpdateListCell {
                list_id,
                row_id,
                cells,
            } => self.update_list_cell(list_id, row_id, cells),
            CollabCommand::DeleteListItem { list_id, row_id } => {
                self.delete_list_item(list_id, row_id);
            }
            CollabCommand::AddMessageToList {
                list_id,
                channel,
                ts,
            } => self.add_message_to_list(list_id, channel, ts),
            CollabCommand::ArchiveListItem { list_id, row_id } => {
                self.archive_list_item(list_id, row_id);
            }
            CollabCommand::SetListAccess {
                list_id,
                access_level,
                channel,
            } => self.set_list_access(list_id, access_level, channel),
            CollabCommand::FetchListRecordComments { list_id, record_id } => {
                self.fetch_list_record_comments(list_id, record_id);
            }
            CollabCommand::LoadAppHome(channel) => self.load_app_home(channel),
            CollabCommand::OpenAppHome { app_id, team_id } => self.open_app_home(app_id, team_id),
            CollabCommand::TripWorkflowTrigger { url, context } => {
                self.trip_workflow_trigger(url, context);
            }
            CollabCommand::BlockAction {
                bot,
                app_id,
                team,
                action,
                container,
            } => self.block_action(bot, app_id, team, action, container),
            CollabCommand::BlockSuggestions {
                key,
                bot,
                app_id,
                team,
                action_id,
                block_id,
                value,
                container,
            } => self.block_suggestions(collab::BlockSuggestionsParams {
                key,
                bot,
                app_id,
                team,
                action_id,
                block_id,
                value,
                container,
            }),
            CollabCommand::SubmitModal {
                view_id,
                hash,
                state_values,
            } => self.submit_modal(view_id, hash, state_values),
            CollabCommand::CloseModal { view_id, hash } => self.close_modal(view_id, hash),
        }
    }

    fn handle_huddle(&mut self, command: HuddleCommand) {
        match command {
            HuddleCommand::Join { channel, audio } => self.join_huddle(channel, audio),
            HuddleCommand::Leave => self.leave_huddle(),
            HuddleCommand::Control(ctrl) => self.send_huddle_control(ctrl.into()),
            HuddleCommand::Invite { channel, users } => self.invite_to_huddle(channel, users),
            HuddleCommand::GetLink { channel } => self.copy_huddle_link(channel),
            HuddleCommand::TestSpeaker { output_device } => {
                self.test_huddle_speaker(output_device);
            }
            HuddleCommand::SetTopic(topic) => self.set_huddle_topic(topic),
            HuddleCommand::SetBackground(background_id) => {
                self.set_huddle_background(background_id);
            }
            HuddleCommand::SetCaptions(enabled) => self.set_huddle_captions(enabled),
            HuddleCommand::DeclineInvite { channel, call_id } => {
                self.decline_huddle_invite(channel, call_id);
            }
        }
    }

    fn handle_workspace(&mut self, command: WorkspaceCommand) {
        match command {
            WorkspaceCommand::ResolveUsers(ids) => self.resolve_users(ids),
            WorkspaceCommand::ResolveBots(ids) => self.resolve_bots(ids),
            WorkspaceCommand::CreateUsergroup {
                name,
                handle,
                description,
            } => self.create_usergroup(name, handle, description),
            WorkspaceCommand::UpdateUsergroup {
                usergroup,
                name,
                handle,
                description,
            } => self.update_usergroup(usergroup, name, handle, description),
            WorkspaceCommand::DisableUsergroup(usergroup) => self.disable_usergroup(usergroup),
            WorkspaceCommand::EnableUsergroup(usergroup) => self.enable_usergroup(usergroup),
            WorkspaceCommand::LoadUsergroupUsers(usergroup) => self.load_usergroup_users(usergroup),
            WorkspaceCommand::UpdateUsergroupUsers { usergroup, users } => {
                self.update_usergroup_users(usergroup, users);
            }
            WorkspaceCommand::SetDndSnooze { minutes } => self.set_dnd_snooze(minutes),
            WorkspaceCommand::EndDndSnooze => self.end_dnd_snooze(),
            WorkspaceCommand::EndDnd => self.end_dnd(),
            WorkspaceCommand::LoadTeamDnd(users) => self.load_team_dnd(users),
            WorkspaceCommand::SetGlobalNotificationPref { name, value, sync } => {
                self.set_global_notification_pref(name, value, sync);
            }
            WorkspaceCommand::UpdateProfile { update } => self.update_profile(update),
            WorkspaceCommand::SetUserPhoto { filename, bytes } => {
                self.set_user_photo(filename, bytes);
            }
            WorkspaceCommand::FetchUserProfile(user) => self.fetch_user_profile(user),
            WorkspaceCommand::SetPresence { away } => {
                if away || !self.stealth_mode.load(Ordering::Relaxed) {
                    self.set_presence(away);
                }
            }
            WorkspaceCommand::SetStealthMode(enabled) => {
                self.stealth_mode.store(enabled, Ordering::Relaxed);
            }
            WorkspaceCommand::SetSaveHuddleTranscripts(enabled) => {
                self.save_huddle_transcripts
                    .store(enabled, Ordering::Relaxed);
            }
            WorkspaceCommand::SetUserPref { name, value } => self.set_user_pref(name, value),
            WorkspaceCommand::SignOut => self.sign_out(),
        }
    }
}
