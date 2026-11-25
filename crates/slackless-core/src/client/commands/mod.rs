//! Front-end `Command` dispatch and domain-scoped command handlers.

mod channels;
mod collab;
mod huddle;
mod messaging;
mod offline;

#[cfg(test)]
pub(super) use huddle::spawn_huddle_watcher;
pub(super) use offline::QueuedMutation;

use super::*;
use crate::huddle::HuddleControl;

impl<B: Backend> Core<B> {
    pub(super) fn handle(&mut self, command: Command) {
        if self.enqueue_or_run(&command) {
            return;
        }
        match command {
            // --- Conversations, channels, sections, tabs, users, profile, DND, usergroups ---
            Command::OpenConversation(channel) => self.open_conversation(channel),
            Command::FetchLatestHistory(channel) => self.fetch_latest_history(channel),
            Command::CloseConversation(channel) => self.close_conversation(&channel),
            Command::ResolveUsers(ids) => self.resolve_users(ids),
            Command::ResolveBots(ids) => self.resolve_bots(ids),
            Command::SetNotifyLevel { channel, level } => self.set_notify_level(channel, level),
            Command::SetGlobalNotificationPref { name, value, sync } => {
                self.set_global_notification_pref(name, value, sync);
            }
            Command::LeaveConversation(channel) => self.leave_conversation(channel),
            Command::JoinConversation(channel) => self.join_conversation(channel),
            Command::SetChannelTopic { channel, topic } => self.set_channel_topic(channel, topic),
            Command::SetChannelPurpose { channel, purpose } => {
                self.set_channel_purpose(channel, purpose);
            }
            Command::FetchChannelManagers { channel } => self.fetch_channel_managers(channel),
            Command::OpenDm { users } => self.open_dm(users),
            Command::CreateSection { name } => self.create_section(name),
            Command::CreateSectionWithChannel { name, channel } => {
                self.create_section_with_channel(name, channel);
            }
            Command::RenameSection { section, name } => self.rename_section(section, name),
            Command::DeleteSection { section } => self.delete_section(section),
            Command::MoveChannel { channel, from, to } => self.move_channel(channel, from, to),
            Command::CreateChannel {
                name,
                is_private,
                purpose,
            } => self.create_channel(name, is_private, purpose),
            Command::RenameChannel { channel, name } => self.rename_channel(channel, name),
            Command::ArchiveChannel(channel) => self.archive_channel(channel),
            Command::UnarchiveChannel(channel) => self.unarchive_channel(channel),
            Command::DeleteChannel(channel) => self.delete_channel(channel),
            Command::ConvertChannelToPrivate(channel) => self.convert_channel_to_private(channel),
            Command::InviteToChannel { channel, users } => self.invite_to_channel(channel, users),
            Command::AddConversationTab {
                channel,
                tab_type,
                label,
            } => self.add_conversation_tab(channel, tab_type, label),
            Command::RemoveConversationTab { channel, tab_id } => {
                self.remove_conversation_tab(channel, tab_id);
            }
            Command::MoveConversationTab {
                channel,
                tab_id,
                after,
            } => self.move_conversation_tab(channel, tab_id, after),
            Command::SetDndSnooze { minutes } => self.set_dnd_snooze(minutes),
            Command::EndDndSnooze => self.end_dnd_snooze(),
            Command::EndDnd => self.end_dnd(),
            Command::LoadTeamDnd(users) => self.load_team_dnd(users),
            Command::UpdateProfile { update } => self.update_profile(update),
            Command::SetPresence { away } => {
                if away || !self.stealth_mode {
                    self.set_presence(away);
                }
            }
            Command::SetStealthMode(enabled) => {
                self.stealth_mode = enabled;
            }
            Command::SetUserPref { name, value } => self.set_user_pref(name, value),
            Command::FetchUserProfile(user) => self.fetch_user_profile(user),
            Command::SetUserPhoto { filename, bytes } => self.set_user_photo(filename, bytes),
            Command::CreateUsergroup {
                name,
                handle,
                description,
            } => self.create_usergroup(name, handle, description),
            Command::UpdateUsergroup {
                usergroup,
                name,
                handle,
                description,
            } => self.update_usergroup(usergroup, name, handle, description),
            Command::SignOut => self.sign_out(),

            // --- Messaging, reactions, threads, drafts, scheduled, pins, bookmarks, saved ---
            Command::OpenThread { channel, thread_ts } => self.open_thread(channel, thread_ts),
            Command::SendMessage {
                channel,
                content,
                thread_ts,
                broadcast,
            } => self.send_message(channel, content, thread_ts, broadcast),
            Command::EditMessage {
                channel,
                ts,
                content,
            } => self.edit_message(channel, ts, content),
            Command::DeleteMessage { channel, ts } => self.delete_message(channel, ts),
            Command::AddReaction { channel, ts, name } => self.add_reaction(channel, ts, name),
            Command::RemoveReaction { channel, ts, name } => {
                self.remove_reaction(channel, ts, name);
            }
            Command::RunCommand {
                channel,
                command,
                args,
                thread_ts,
            } => self.run_command(channel, command, args, thread_ts),
            Command::BlockAction {
                bot,
                app_id,
                team,
                action,
                container,
            } => self.block_action(bot, app_id, team, action, container),
            Command::MarkRead { channel, ts } => self.mark_read(channel, ts),
            Command::MarkThreadRead {
                channel,
                thread_ts,
                ts,
            } => self.mark_thread_read(channel, thread_ts, ts),
            Command::MarkAllRead => self.mark_all_read(),
            Command::SetThreadSubscribed {
                channel,
                thread_ts,
                last_read,
                subscribed,
            } => self.set_thread_subscribed(channel, thread_ts, last_read, subscribed),
            Command::FetchThreads => self.fetch_threads(),
            Command::FetchActivity { with_bodies } => self.fetch_activity(with_bodies),
            Command::MarkActivity { entries, read } => self.mark_activity(entries, read),
            Command::Typing { channel, thread_ts } => self.send_typing(channel, thread_ts),
            Command::LoadPins(channel) => self.load_pins(channel),
            Command::SetPinned {
                channel,
                ts,
                pinned,
            } => self.set_pinned(channel, ts, pinned),
            Command::LoadBookmarks(channel) => self.load_bookmarks(channel),
            Command::AddBookmark {
                channel,
                title,
                url,
            } => self.add_bookmark(channel, title, url),
            Command::AddBookmarkInFolder {
                channel,
                title,
                url,
                parent_id,
            } => self.add_bookmark_in_folder(channel, title, url, parent_id),
            Command::RemoveBookmark {
                channel,
                bookmark_id,
            } => self.remove_bookmark(channel, bookmark_id),
            Command::ScheduleMessage {
                channel,
                post_at,
                content,
                thread_ts,
            } => self.schedule_message(channel, post_at, content, thread_ts),
            Command::LoadScheduledMessages => self.load_scheduled_messages(),
            Command::DeleteScheduledMessage {
                channel,
                scheduled_message_id,
            } => self.delete_scheduled_message(channel, scheduled_message_id),
            Command::FetchCloudDrafts => self.fetch_cloud_drafts(),
            Command::SyncDraft { channel, text } => self.sync_draft(channel, text),
            Command::LoadSaved => self.fetch_saved(),
            Command::SaveItem {
                channel,
                ts,
                date_due,
            } => self.save_item(channel, ts, date_due),
            Command::SaveFile { file_id, date_due } => self.save_file(file_id, date_due),
            Command::UpdateSavedItem {
                channel,
                ts,
                mark,
                date_due,
            } => self.update_saved_item(channel, ts, mark, date_due),
            Command::DeleteSavedItem { channel, ts } => self.delete_saved_item(channel, ts),
            Command::ClearCompletedSaved => self.clear_completed_saved(),

            // --- Search, GIFs, files, canvases, lists, app home, workflows, modals ---
            Command::Search(query) => self.search(query),
            Command::QuickSwitchSearch { query } => self.quick_switch_search(query),
            Command::SearchRemoteMessages { query, sort, page } => {
                self.search_remote_messages(query, sort, page);
            }
            Command::SearchRemoteFiles { query, sort, page } => {
                self.search_remote_files(query, sort, page);
            }
            Command::SearchGifs { query } => self.search_gifs(query),
            Command::LoadFiles(filter) => self.load_files(filter),
            Command::UploadFiles {
                channel,
                thread_ts,
                content,
                files,
            } => self.upload_files(channel, thread_ts, content, files),
            Command::DeleteFile { file_id } => self.delete_file(file_id),
            Command::RequestFilePermission {
                file_id,
                request_type,
            } => self.request_file_permission(file_id, request_type),
            Command::LoadFilesExtras => self.load_files_extras(),
            Command::SetFileFavorite {
                file_id,
                collection_id,
                starred,
            } => self.set_file_favorite(file_id, collection_id, starred),
            Command::CreateFileCollection { name, emoji } => {
                self.create_file_collection(name, emoji);
            }
            Command::DeleteFileCollection { collection_id } => {
                self.delete_file_collection(collection_id);
            }
            Command::RestoreDeletedFile { file_id } => self.restore_deleted_file(file_id),
            Command::CreateChannelCanvas(channel) => self.create_channel_canvas(channel),
            Command::LoadCanvas { file_id } => self.load_canvas(file_id),
            Command::CreateCanvas {
                title,
                markdown,
                channel,
            } => self.create_canvas(title, markdown, channel),
            Command::EditCanvas { file_id, op } => self.edit_canvas(file_id, op),
            Command::DeleteCanvas { canvas_id } => self.delete_canvas(canvas_id),
            Command::SetCanvasAccess {
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
            Command::DeleteCanvasAccess {
                canvas_id,
                channel_ids,
                user_ids,
            } => self.delete_canvas_access(canvas_id, channel_ids, user_ids),
            Command::LoadCanvasComments { canvas_id } => self.load_canvas_comments(canvas_id),
            Command::PostCanvasComment {
                canvas_id,
                text,
                thread_ts,
            } => self.post_canvas_comment(canvas_id, text, thread_ts),
            Command::ResolveCanvasComment {
                canvas_id,
                thread_ts,
                resolved,
            } => self.resolve_canvas_comment(canvas_id, thread_ts, resolved),
            Command::ResolveAllCanvasComments { canvas_id } => {
                self.resolve_all_canvas_comments(canvas_id);
            }
            Command::LoadList { file_id } => self.load_list(file_id),
            Command::CreateList {
                name,
                description,
                channel,
            } => self.create_list(name, description, channel),
            Command::CreateListItem {
                list_id,
                initial_fields,
            } => self.create_list_item(list_id, initial_fields),
            Command::UpdateListCell {
                list_id,
                row_id,
                cells,
            } => self.update_list_cell(list_id, row_id, cells),
            Command::DeleteListItem { list_id, row_id } => self.delete_list_item(list_id, row_id),
            Command::AddMessageToList {
                list_id,
                channel,
                ts,
            } => self.add_message_to_list(list_id, channel, ts),
            Command::ArchiveListItem { list_id, row_id } => self.archive_list_item(list_id, row_id),
            Command::SetListAccess {
                list_id,
                access_level,
                channel,
            } => self.set_list_access(list_id, access_level, channel),
            Command::FetchListRecordComments { list_id, record_id } => {
                self.fetch_list_record_comments(list_id, record_id);
            }
            Command::LoadAppHome(channel) => self.load_app_home(channel),
            Command::OpenAppHome { app_id, team_id } => self.open_app_home(app_id, team_id),
            Command::TripWorkflowTrigger { url, context } => {
                self.trip_workflow_trigger(url, context);
            }
            Command::SubmitModal {
                view_id,
                hash,
                state_values,
            } => self.submit_modal(view_id, hash, state_values),
            Command::CloseModal { view_id, hash } => self.close_modal(view_id, hash),
            Command::BlockSuggestions {
                key,
                bot,
                app_id,
                team,
                action_id,
                block_id,
                value,
                container,
            } => self.block_suggestions(
                key, bot, app_id, team, action_id, block_id, value, container,
            ),

            // --- Offline mutation queue ---
            Command::RetryOfflineMutation(id) => self.retry_offline_mutation(id),
            Command::DiscardOfflineMutation(id) => self.discard_offline_mutation(id),

            // --- Huddles ---
            Command::JoinHuddle { channel, audio } => self.join_huddle(channel, audio),
            Command::LeaveHuddle => self.leave_huddle(),
            Command::SetHuddleMuted(muted) => {
                self.send_huddle_control(HuddleControl::SetMuted(muted));
            }
            Command::SetHuddleAudioDevices { input, output } => {
                self.send_huddle_control(HuddleControl::SetAudioDevices { input, output });
            }
            Command::SetHuddleAudioProcessing { aec, ns, agc } => {
                self.send_huddle_control(HuddleControl::SetAudioProcessing { aec, ns, agc });
            }
            Command::InviteToHuddle { channel, users } => self.invite_to_huddle(channel, users),
            Command::CopyHuddleLink { channel } => self.copy_huddle_link(channel),
            Command::TestHuddleSpeaker { output_device } => {
                self.test_huddle_speaker(output_device);
            }
            Command::SetHuddleCamera {
                enabled,
                device_index,
                blur,
            } => self.send_huddle_control(HuddleControl::SetCamera {
                enabled,
                device_index,
                blur,
            }),
            Command::SetHuddleBlurBackground(blur) => {
                self.send_huddle_control(HuddleControl::SetBlurBackground(blur));
            }
            Command::SetHuddleScreenShare {
                source_id,
                share_audio,
            } => self.send_huddle_control(HuddleControl::SetScreenShare {
                source_id,
                share_audio,
            }),
            Command::SendHuddleReaction {
                emoji,
                kind,
                emoji_url,
            } => self.send_huddle_control(HuddleControl::SendReaction {
                emoji,
                kind,
                emoji_url,
            }),
            Command::SetHuddleHandRaised(raised) => {
                self.send_huddle_control(HuddleControl::SetHandRaised(raised));
            }
            Command::SetHuddleStickyEmoji(emoji) => {
                self.send_huddle_control(HuddleControl::SetStickyEmoji(emoji));
            }
            Command::SendHuddleDraw(draw) => {
                self.send_huddle_control(HuddleControl::SendDraw(draw));
            }
            Command::SetHuddleJukebox(song) => {
                self.send_huddle_control(HuddleControl::SetJukebox(song));
            }
            Command::MuteHuddlePeer(peer_id) => {
                self.send_huddle_control(HuddleControl::MutePeer(peer_id));
            }
            Command::SetHuddleTopic(topic) => self.set_huddle_topic(topic),
            Command::SetHuddleBackground(background_id) => {
                self.set_huddle_background(background_id);
            }
            Command::SetHuddleCaptions(enabled) => self.set_huddle_captions(enabled),
            Command::SetHuddleAudioOnly(enabled) => {
                self.send_huddle_control(HuddleControl::SetAudioOnly(enabled));
            }
            Command::DeclineHuddleInvite { channel, call_id } => {
                self.decline_huddle_invite(channel, call_id);
            }

            // Handled by the loop.
            Command::Shutdown => {}
        }
    }
}
