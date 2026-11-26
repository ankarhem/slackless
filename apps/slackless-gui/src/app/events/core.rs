//! Active-workspace `CoreBridge` event dispatch.

use super::super::*;

impl App {
    /// Live mode: fold a core event into the views.
    pub(in crate::app) fn on_core_event(
        &mut self,
        event: &Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            Event::Conversations(_) | Event::UsersChanged(_) | Event::SidebarChanged => {
                self.reload_live(window, cx)
            }
            Event::Counts(counts) => self.on_counts_event(counts, window, cx),
            Event::ReadMarker { channel, ts } => {
                self.apply_read_marker(channel, ts, cx);
            }
            Event::StoreChanged { channel } => self.on_store_changed_event(channel, cx),
            Event::AuthExpired(error) => tracing::warn!(
                error = %error,
                "Slack rejected the saved session; sign in again with `slackless-gui --sign-in`"
            ),
            Event::CommandFailed { command, error } if error == "unknown_command" => notify(
                "Unknown command",
                format!("{command} is not a valid command."),
                window,
                cx,
            ),
            Event::CommandFailed { command, error }
                if command == "subscriptions.thread.getView" =>
            {
                // Offline / API error fallback: populate Threads from local SQLite store instead
                // of leaving the spinner stuck.
                self.load_threads_store_fallback(cx);
                tracing::warn!(command = %command, error = %error, "thread view command failed");
            }
            Event::CommandFailed { command, error } => {
                notify(command, format!("Slack refused it ({error})."), window, cx)
            }
            Event::Typing {
                channel,
                user,
                thread_ts,
            } => self.someone_typing(channel, user, thread_ts.as_ref(), cx),
            Event::MessagePosted { channel, message } => {
                self.message_posted(channel, message, window, cx)
            }
            Event::Error(error) => {
                tracing::warn!(error = %error, "slackless core error");
                if error.starts_with("chat.scheduleMessage:")
                    || error.starts_with("chat.deleteScheduledMessage:")
                {
                    let workspace = self.workspace;
                    self.drafts.update(cx, |drafts, cx| {
                        drafts.drop_pending_scheduled(workspace, cx)
                    });
                }
                if let Some(title) = user_facing_error_title(error) {
                    notify(title, error.clone(), window, cx);
                }
            }
            Event::BotProfileLoaded { bot_id, profile } => {
                self.ws
                    .bots
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(bot_id.clone(), profile.clone());
                window.refresh();
            }
            Event::Presence { users, presence } => {
                // Every snapshot shares this map: update it in place and repaint, with no
                // store reload however often presence changes.
                self.ws
                    .presence
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend(users.iter().map(|user| (user.clone(), presence.clone())));
                window.refresh();
            }
            Event::DndPaused { user, paused } => {
                self.ws.peer_dnd_paused.insert(user.clone(), *paused);
                cx.notify();
            }
            Event::TeamDndLoaded(paused_map) => {
                self.ws
                    .peer_dnd_paused
                    .extend(paused_map.iter().map(|(u, p)| (u.clone(), *p)));
                cx.notify();
            }
            Event::ThreadsView(view) => {
                let total_unread = view.total_unread_replies;
                self.threads
                    .update(cx, |threads, cx| threads.set_remote((**view).clone(), cx));
                if total_unread == 0 {
                    self.sidebar
                        .update(cx, |s, cx| s.set_thread_counts(false, 0, cx));
                    self.sync_badge(cx);
                } else if self.page == MainPage::Threads {
                    let settings = self.preferences.read(cx).settings().clone();
                    let allow_mark = settings.mark_read != 2
                        && (!settings.no_bg_mark_read || self.window_active);
                    if allow_mark {
                        self.auto_mark_threads_view_read(cx);
                    }
                }
                cx.notify();
            }
            Event::ThreadReadMarker {
                channel,
                thread_ts,
                last_read,
            } => {
                if let Some(cursor) = last_read {
                    self.ws
                        .marked_threads
                        .insert((channel.clone(), thread_ts.clone()), cursor.clone());
                }
                self.threads.update(cx, |threads, cx| {
                    threads.mark_thread_read(channel, thread_ts, last_read.as_ref(), false, cx);
                });
                self.sync_thread_sidebar_counts(cx);
            }
            Event::ActivityView { feed } => match feed.as_deref() {
                Ok(feed) => {
                    self.activity
                        .update(cx, |view, cx| view.show_remote(feed.clone(), cx));
                }
                Err(text) => {
                    self.activity
                        .update(cx, |view, cx| view.show_error(text, cx));
                }
            },
            Event::ConversationOpened(channel) => {
                let pending = self.pending_dm.take();
                self.select(channel.clone(), cx);
                if let Some(content) = pending {
                    self.deliver_send(content, cx);
                }
            }
            Event::ChannelManagers { channel, users } => {
                self.details.update(cx, |details, cx| {
                    details.set_managers(channel, users.clone(), cx)
                });
            }
            Event::ConversationInfo {
                channel,
                num_members,
                num_apps,
                members,
            } => self.on_conversation_info_event(channel, *num_members, *num_apps, members, cx),
            Event::PinsLoaded { channel, pins } => {
                // File pins are left to the Files & links tab; the panel lists messages.
                let messages = pins.as_ref().map_err(Clone::clone).map(|pins| {
                    pins.iter()
                        .filter_map(|pin| pin.message.clone())
                        .collect::<Vec<_>>()
                });
                if let Ok(messages) = &messages {
                    self.resolve_named_users(messages);
                }
                self.ws.pins.insert(channel.clone(), messages);
                cx.notify();
            }
            Event::BookmarksLoaded { channel, bookmarks } => {
                self.ws.bookmarks.insert(channel.clone(), bookmarks.clone());
                cx.notify();
            }
            Event::Connection(state) => {
                let became_connected = matches!(
                    state,
                    slackless_core::rtm::ConnectionState::Connected { .. }
                ) && !matches!(
                    self.ws.connection,
                    Some(slackless_core::rtm::ConnectionState::Connected { .. })
                );
                self.ws.connection = Some(state.clone());
                if became_connected && !self.fixtures.sample {
                    self.bridge.send(MessagingCommand::LoadScheduledMessages);
                }
                cx.notify();
            }
            Event::OfflineQueueChanged(count) => {
                self.ws.offline_queued = *count;
                cx.notify();
            }
            Event::OfflineMutationFailed(failed) => {
                if !self.ws.failed_mutations.iter().any(|m| m.id == failed.id) {
                    self.ws.failed_mutations.push(failed.clone());
                    cx.notify();
                }
            }
            Event::SelfDndUpdated(status) => {
                self.self_state.paused_until = status
                    .snooze_enabled
                    .then_some(status.snooze_endtime)
                    .flatten();
                self.push_self_state(cx);
                self.sync_badge(cx);
            }
            Event::MessageScheduled {
                channel,
                scheduled_message_id,
                post_at,
            } => {
                let workspace = self.workspace;
                self.drafts.update(cx, |drafts, cx| {
                    drafts.confirm_scheduled(
                        workspace,
                        channel,
                        *post_at,
                        scheduled_message_id.clone(),
                        cx,
                    );
                });
                notify(
                    "Message scheduled",
                    format!("Sends {}.", crate::drafts::when_label(*post_at, now_secs())),
                    window,
                    cx,
                );
                cx.notify();
            }
            Event::ScheduledMessagesLoaded(Ok(list)) => {
                let workspace = self.workspace;
                let entries: Vec<Entry> = list
                    .iter()
                    .map(|m| Entry {
                        id: Some(m.id.clone()),
                        workspace,
                        channel: m.channel_id.clone(),
                        label: self.label(&m.channel_id),
                        text: m.text.clone(),
                        at: m.post_at,
                    })
                    .collect();
                self.drafts.update(cx, |drafts, cx| {
                    drafts.set_remote_scheduled(workspace, entries, cx)
                });
                cx.notify();
            }
            Event::ScheduledMessagesLoaded(Err(error)) => {
                tracing::warn!(error = %error, "failed to load scheduled messages");
            }
            Event::Usergroups(groups) => {
                self.ws.usergroups = groups.clone();
                self.reload_live(window, cx);
            }
            Event::AppCommands(commands) => {
                self.ws.app_commands = commands.clone();
                self.reload_live(window, cx);
            }
            Event::QuickSwitchResults { query, results } => {
                self.switcher.update(cx, |switcher, cx| {
                    switcher.set_remote_results(query, (**results).clone(), cx);
                });
            }
            Event::GifResults { query, gifs } => {
                self.emoji_picker.update(cx, |picker, cx| {
                    picker.set_gif_results(query, gifs.clone(), cx);
                });
            }
            Event::SavedLoaded(Ok(items)) => {
                let ws = self.workspace;
                self.apply_saved_loaded(ws, items, cx);
            }
            Event::RemoteMessagesPage {
                query,
                sort,
                page,
                result,
            } => {
                let mapped = result.as_ref().map(|p| (**p).clone()).map_err(Clone::clone);
                self.results.update(cx, |view, cx| {
                    view.set_remote_messages(query, *sort, *page, mapped, cx);
                });
            }
            Event::RemoteFilesPage {
                query,
                sort,
                page,
                result,
            } => {
                let mapped = result.as_ref().map(|p| (**p).clone()).map_err(Clone::clone);
                self.results.update(cx, |view, cx| {
                    view.set_remote_files(query, *sort, *page, mapped, cx);
                });
            }
            Event::Huddle(huddle_ev) => self.on_huddle_core_event(huddle_ev, window, cx),
            Event::UserProfileLoaded {
                user,
                pronouns,
                fields,
            } => {
                self.profile.update(cx, |profile, cx| {
                    profile.set_user_profile(user.clone(), pronouns.clone(), fields.clone(), cx);
                });
            }
            Event::ModalOpened(view) => self.open_block_modal(view.clone(), window, cx),
            Event::ModalClosed(_) => window.close_dialog(cx),
            Event::CloudDrafts(cloud_drafts) => self.on_cloud_drafts_event(cloud_drafts, cx),
            Event::BlockSuggestions { key, options } => {
                self.messages.update(cx, |list, cx| {
                    list.set_block_suggestions(key.clone(), options.clone(), cx);
                });
            }
            Event::ChannelCanvasCreated { .. }
            | Event::CanvasLoaded { .. }
            | Event::CanvasMetaLoaded { .. }
            | Event::CanvasDeleted { .. }
            | Event::CanvasAccessUpdated { .. }
            | Event::CanvasCommentsLoaded { .. }
            | Event::CanvasCommentPosted { .. }
            | Event::CanvasCommentResolved { .. }
            | Event::CanvasCreated { .. } => self.on_canvas_event(event, window, cx),
            Event::ListLoaded { .. }
            | Event::ListMetaLoaded { .. }
            | Event::ListAccessUpdated { .. }
            | Event::ListCreated { .. }
            | Event::MessageAddedToList { .. }
            | Event::ListRecordComments { .. } => self.on_list_event(event, window, cx),
            Event::FilesLoaded { .. }
            | Event::FilePermissionRequested { .. }
            | Event::FilesExtrasLoaded { .. }
            | Event::FileRestored { .. }
            | Event::FileDeleted { .. } => self.on_files_event(event, window, cx),
            Event::FileTranscriptReady {
                file_id,
                text,
                cues,
            } => {
                let was_manual = crate::media::finish_transcription(file_id, cx);
                if !text.trim().is_empty() {
                    let updated = Arc::new(self.fixtures.with_file_transcript_cues(
                        file_id.clone(),
                        text.clone(),
                        cues.clone(),
                    ));
                    self.fixtures = updated.clone();
                    self.sync_all_views_fixtures(updated, cx);
                    if self.page == MainPage::Search {
                        self.results.update(cx, |view, cx| view.refresh_local(cx));
                    }
                    if was_manual {
                        notify(
                            "Transcript saved",
                            "Transcript saved \u{2014} video is now searchable in Messages",
                            window,
                            cx,
                        );
                    }
                } else if was_manual {
                    notify(
                        "No speech detected",
                        "No spoken audio was found in this media clip.",
                        window,
                        cx,
                    );
                }
                window.refresh();
                cx.notify();
            }
            Event::FileTranscriptFailed { file_id, error } => {
                crate::media::finish_transcription(file_id, cx);
                tracing::warn!(file_id = %file_id, error = %error, "file transcription failed");
                notify("Transcription failed", error.clone(), window, cx);
                window.refresh();
            }
            Event::AppHomeLoaded { .. } | Event::AppHomeUpdated(_) => {
                self.on_app_home_event(event, cx)
            }
            Event::BackfillProgress(progress) => {
                self.ws.backfill_progress = Some(progress.clone());
                let snap = progress.clone();
                self.preferences.update(cx, |prefs, cx| {
                    prefs.set_backfill_progress(Some(snap), cx);
                });
                window.refresh();
                cx.notify();
            }
            Event::WorkflowTripped { .. } => {
                notify(
                    "Workflow started",
                    "Workflow trigger dispatched.",
                    window,
                    cx,
                );
            }
            _ => {}
        }
    }

    fn on_counts_event(
        &mut self,
        counts: &slackless_core::api::ClientCounts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let threads = &counts.threads;
        let (has_unreads, mentions) = (threads.has_unreads, threads.mention_count);
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_thread_counts(has_unreads, mentions, cx)
        });
        let mut next_counts: HashMap<ChannelId, ConversationCounts> =
            [&counts.channels, &counts.ims, &counts.mpims]
                .into_iter()
                .flatten()
                .map(|entry| (entry.id.clone(), entry.clone()))
                .collect();
        let mut newly_unread: Vec<ChannelId> = Vec::new();
        for (id, entry) in &mut next_counts {
            if let Some(marked) = self.ws.marked_read.get(id).cloned() {
                let server_has_newer = entry.latest.as_ref().is_some_and(|latest| *latest > marked);
                if !server_has_newer {
                    // Keep local read state when an in-flight `client.counts` predates our
                    // `conversations.mark`.
                    entry.clear_unreads();
                    if entry.last_read.as_ref().is_none_or(|r| marked > *r) {
                        entry.last_read = Some(marked);
                    }
                } else {
                    // Genuinely newer messages landed since we last marked this read.
                    self.ws.marked_read.remove(id);
                    if entry.is_unread() {
                        newly_unread.push(id.clone());
                    }
                }
            } else if entry.is_unread() {
                newly_unread.push(id.clone());
            }
        }
        if !newly_unread.is_empty() {
            self.sidebar.update(cx, |sidebar, cx| {
                for id in &newly_unread {
                    sidebar.mark_unread(id, cx);
                }
            });
        }
        self.ws.counts = next_counts;
        // Fast path: when every conversation in `counts` is already known in memory,
        // update counts + `last_read` in-place without re-reading the whole SQLite store.
        let all_known = !self.fixtures.sample
            && !self.fixtures.channels.is_empty()
            && self
                .ws
                .counts
                .keys()
                .all(|id| self.fixtures.conversation(id).is_some());
        if all_known {
            let updated = Arc::new(self.fixtures.with_counts(self.ws.counts.clone()));
            self.apply_snapshot(updated, window, cx);
        } else {
            self.reload_live(window, cx);
        }
        // Cold launch can open a DM before `client.counts` arrives: once counts report it
        // unread, mark the visible open conversation read right away.
        if self
            .ws
            .counts
            .get(&self.channel)
            .is_some_and(ConversationCounts::is_unread)
        {
            self.maybe_mark_open_read(cx);
        }
    }

    fn on_store_changed_event(&mut self, channel: &ChannelId, cx: &mut Context<Self>) {
        if *channel == self.channel {
            self.load_history(channel, cx);
            // The channel's people are the history's recent authors: stale now.
            self.channel_people = None;
            self.load_join_people(cx);
        }
        self.refresh_dm_latest(channel, cx);
        // The open Unreads view follows a message landing in any channel.
        if self.page == MainPage::Unreads {
            self.refresh_unreads(cx);
        }
        // The open thread: a fetched replies page landed, or a live reply arrived.
        if self.right_pane.is_thread() {
            let open = self.thread.read(cx).open_thread();
            if let Some((thread_channel, thread_ts)) = open
                && thread_channel == *channel
            {
                self.load_thread(&thread_channel, &thread_ts, cx);
            }
        }
        // Any expanded card in the Threads view refreshes when its replies land.
        if self.page == MainPage::Threads {
            let include_deleted = self.preferences.read(cx).settings().show_deleted_messages;
            let expanded = self.threads.read(cx).expanded_in(channel);
            for thread_ts in expanded {
                if let Some(read) =
                    self.bridge
                        .thread(channel.clone(), thread_ts.clone(), include_deleted)
                {
                    let (ch, ts) = (channel.clone(), thread_ts);
                    cx.spawn(async move |this, cx| {
                        let msgs = cx.background_executor().spawn(read).await;
                        let _ = this.update(cx, |app, cx| {
                            app.resolve_named_users(&msgs);
                            let replies: Vec<_> = msgs.into_iter().filter(|m| m.ts != ts).collect();
                            app.threads.update(cx, |view, cx| {
                                view.set_card_replies(&ch, &ts, replies, cx);
                            });
                        });
                    })
                    .detach();
                }
            }
        }
    }

    fn on_conversation_info_event(
        &mut self,
        channel: &ChannelId,
        num_members: Option<u32>,
        num_apps: Option<u32>,
        members: &[UserId],
        cx: &mut Context<Self>,
    ) {
        if let Some(count) = num_members {
            self.ws.member_counts.insert(channel.clone(), count);
        }
        if let Some(apps) = num_apps {
            self.ws.app_counts.insert(channel.clone(), apps);
        }
        if (num_members.is_some() || num_apps.is_some())
            && let Some(write) = self
                .bridge
                .remember_channel_counts(&self.ws.member_counts, &self.ws.app_counts)
        {
            cx.background_executor().spawn(write).detach();
        }
        if !members.is_empty() {
            self.ws
                .members_cache
                .insert(channel.clone(), members.to_vec());
        }
        // A late answer for a channel left since is moot: the open one's follows.
        if *channel == self.channel {
            if !members.is_empty() {
                self.open_members = Some((channel.clone(), members.to_vec()));
                // Members `users.list` leaves out - another organization's people
                // (a connect channel's, or guests invited to this one) - resolve on
                // demand, the same way unknown DM peers do; the members an ordinary
                // channel lists all sit in the directory, so nothing is asked.
                self.resolve_users(members.iter().cloned());
            }
            self.sync_connect_orgs(cx);
            // An empty list (the fetch failed) turns the dialog's Members tab from
            // loading to its inline error.
            self.details.update(cx, |details, cx| {
                details.set_conversation_info(channel, members.to_vec(), cx)
            });
        }
        cx.notify();
    }

    fn on_cloud_drafts_event(
        &mut self,
        cloud_drafts: &[slackless_core::api::CloudDraft],
        cx: &mut Context<Self>,
    ) {
        let mut added = false;
        for draft in cloud_drafts {
            let has_local = self
                .drafts
                .read(cx)
                .draft(self.workspace, &draft.channel)
                .is_some();
            if !has_local {
                let entry = Entry {
                    id: Some(draft.id.clone()),
                    workspace: self.workspace,
                    channel: draft.channel.clone(),
                    label: self.label(&draft.channel),
                    text: draft.text.clone(),
                    at: draft.updated_secs.unwrap_or_else(now_secs),
                };
                self.drafts
                    .update(cx, |drafts, cx| drafts.set_draft(entry, cx));
                added = true;
            }
        }
        if added {
            let count = self.drafts.read(cx).draft_count();
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.set_draft_count(count, cx));
            self.persist_drafts(cx);
        }
    }

    fn on_canvas_event(&mut self, event: &Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::ChannelCanvasCreated { channel, canvas_id } if *channel == self.channel => {
                if !canvas_id.is_empty() {
                    self.open_live_canvas(canvas_id.clone(), cx);
                }
                notify("Canvas created", "Channel canvas is ready.", window, cx);
            }
            Event::CanvasLoaded {
                file_id,
                title,
                html,
            } => {
                let was_loaded = self.ws.canvases.get(file_id).is_some_and(Result::is_ok);
                let parsed = match html {
                    Ok(raw) => Ok(crate::canvas::CanvasDoc::from_html_with_emoji(
                        title.clone(),
                        raw,
                        &crate::emoji::display_glyph,
                    )),
                    Err(err) => Err(err.clone()),
                };
                if was_loaded && parsed.is_ok() {
                    self.ws
                        .canvas_updated_notes
                        .insert(file_id.clone(), format!("Canvas updated: “{title}”"));
                }
                self.ws.canvases.insert(file_id.clone(), parsed);
                cx.notify();
            }
            Event::CanvasMetaLoaded { file_id, meta } => {
                self.ws.canvas_metas.insert(file_id.clone(), meta.clone());
                cx.notify();
            }
            Event::CanvasDeleted { canvas_id, result } => match result {
                Ok(()) => {
                    self.ws.canvases.remove(canvas_id);
                    self.ws.canvas_metas.remove(canvas_id);
                    self.ws.canvas_comments.remove(canvas_id);
                    self.ws.canvas_status.remove(canvas_id);
                    self.ws.canvas_updated_notes.remove(canvas_id);
                    if self.ws.active_canvas.as_deref() == Some(canvas_id.as_str()) {
                        self.ws.active_canvas = None;
                        self.tab = ConversationTab::Messages;
                    }
                    notify("Canvas deleted", "Canvas permanently deleted.", window, cx);
                    cx.notify();
                }
                Err(err) => {
                    let msg = slackless_core::api::format_canvas_error(err);
                    self.ws
                        .canvas_status
                        .insert(canvas_id.clone(), (true, msg.clone()));
                    notify("Could not delete canvas", msg, window, cx);
                    cx.notify();
                }
            },
            Event::CanvasAccessUpdated {
                canvas_id,
                summary,
                result,
            } => match result {
                Ok(()) => {
                    self.ws
                        .canvas_status
                        .insert(canvas_id.clone(), (false, summary.clone()));
                    self.bridge.send(CollabCommand::LoadCanvas {
                        file_id: canvas_id.clone(),
                    });
                    notify("Canvas access updated", summary.clone(), window, cx);
                    cx.notify();
                }
                Err(err) => {
                    let msg = slackless_core::api::format_canvas_error(err);
                    self.ws
                        .canvas_status
                        .insert(canvas_id.clone(), (true, msg.clone()));
                    notify("Access update failed", msg, window, cx);
                    cx.notify();
                }
            },
            Event::CanvasCommentsLoaded { canvas_id, result } => match result {
                Ok(threads) => {
                    self.ws
                        .canvas_comments
                        .insert(canvas_id.clone(), threads.clone());
                    cx.notify();
                }
                Err(err) => {
                    let msg = slackless_core::api::format_canvas_error(err);
                    self.ws.canvas_status.insert(canvas_id.clone(), (true, msg));
                    cx.notify();
                }
            },
            Event::CanvasCommentPosted { canvas_id, result } => match result {
                Ok(()) => {
                    self.bridge.send(CollabCommand::LoadCanvasComments {
                        canvas_id: canvas_id.clone(),
                    });
                    cx.notify();
                }
                Err(err) => {
                    let msg = slackless_core::api::format_canvas_error(err);
                    self.ws
                        .canvas_status
                        .insert(canvas_id.clone(), (true, msg.clone()));
                    notify("Comment failed", msg, window, cx);
                    cx.notify();
                }
            },
            Event::CanvasCommentResolved {
                canvas_id,
                thread_ts,
                resolved,
                result,
            } => match result {
                Ok(()) => {
                    if let Some(threads) = self.ws.canvas_comments.get_mut(canvas_id) {
                        if thread_ts.as_str().is_empty() {
                            for t in threads.iter_mut() {
                                t.resolved = *resolved;
                            }
                        } else if let Some(t) = threads.iter_mut().find(|t| t.root_ts == *thread_ts)
                        {
                            t.resolved = *resolved;
                        }
                    }
                    self.bridge.send(CollabCommand::LoadCanvasComments {
                        canvas_id: canvas_id.clone(),
                    });
                    cx.notify();
                }
                Err(err) => {
                    let msg = slackless_core::api::format_canvas_error(err);
                    self.ws
                        .canvas_status
                        .insert(canvas_id.clone(), (true, msg.clone()));
                    notify("Could not update comment thread", msg, window, cx);
                    cx.notify();
                }
            },
            Event::CanvasCreated { file_id, title, .. } => {
                if !file_id.is_empty() {
                    self.open_live_canvas(file_id.clone(), cx);
                }
                notify("Canvas created", title.clone(), window, cx);
            }
            _ => {}
        }
    }

    fn on_list_event(&mut self, event: &Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::ListLoaded { file_id, list } => {
                let value = match list {
                    Ok(data) => Ok((**data).clone()),
                    Err(err) => Err(err.clone()),
                };
                self.ws.lists.insert(file_id.clone(), value);
                cx.notify();
            }
            Event::ListMetaLoaded { file_id, meta } => {
                crate::lists::set_list_meta(file_id, (**meta).clone(), cx);
                window.refresh();
                cx.notify();
            }
            Event::ListAccessUpdated {
                list_id,
                summary,
                result,
            } => {
                match result {
                    Ok(()) => {
                        crate::lists::set_list_status(list_id, false, summary.clone(), cx);
                        notify("List sharing updated", summary.clone(), window, cx);
                        self.bridge.send(CollabCommand::LoadList {
                            file_id: list_id.clone(),
                        });
                    }
                    Err(err) => {
                        let friendly = slackless_core::api::format_list_error(err);
                        crate::lists::set_list_status(list_id, true, friendly.clone(), cx);
                        notify("Could not update list sharing", friendly, window, cx);
                    }
                }
                window.refresh();
                cx.notify();
            }
            Event::ListCreated { file_id, name } => {
                if !file_id.is_empty() {
                    self.open_live_list(file_id.clone(), cx);
                }
                notify("List created", name.clone(), window, cx);
            }
            Event::MessageAddedToList { list_id, .. } => {
                self.bridge.send(CollabCommand::LoadList {
                    file_id: list_id.clone(),
                });
                notify(
                    "Added to list",
                    "Message added as a new list item.",
                    window,
                    cx,
                );
            }
            Event::ListRecordComments {
                list_id,
                record_id,
                messages,
            } => {
                let items: Vec<(String, String)> = messages
                    .iter()
                    .map(|m| {
                        let author = m
                            .user
                            .as_ref()
                            .and_then(|u| self.fixtures.user(u))
                            .map(|u| u.display_name().to_owned())
                            .or_else(|| m.username.clone())
                            .unwrap_or_else(|| "Teammate".to_owned());
                        (author, m.text.clone())
                    })
                    .collect();
                crate::lists::set_record_comments(list_id, record_id, items, cx);
                if !messages.is_empty() {
                    let (list_title, item_title) = self
                        .ws
                        .lists
                        .get(list_id)
                        .and_then(|res| res.as_ref().ok())
                        .map(|data| {
                            let list_title = if data.title.is_empty() {
                                "List".to_owned()
                            } else {
                                data.title.clone()
                            };
                            let item_title =
                                data.rows
                                    .iter()
                                    .find(|row| row.id == *record_id)
                                    .and_then(|row| {
                                        let primary = data
                                            .columns
                                            .iter()
                                            .find(|c| c.is_primary)
                                            .or_else(|| data.columns.first())?;
                                        match slackless_core::domain::lists::row_cell(row, primary)
                                        {
                                            Some(
                                                slackless_core::api::SlackListCellValue::Text(s),
                                            ) if !s.trim().is_empty() => Some(s.clone()),
                                            _ => None,
                                        }
                                    })
                                    .unwrap_or_else(|| format!("Item {record_id}"));
                            (list_title, item_title)
                        })
                        .unwrap_or_else(|| ("List".to_owned(), format!("Item {record_id}")));
                    let meta = crate::threads_cards::ListItemThreadMeta {
                        list_id: list_id.clone(),
                        list_title,
                        record_id: record_id.clone(),
                        item_title,
                    };
                    self.threads.update(cx, |view, cx| {
                        view.upsert_list_item_thread(meta, messages.clone(), cx);
                    });
                }
                window.refresh();
                cx.notify();
            }
            _ => {}
        }
    }

    fn on_files_event(&mut self, event: &Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::FilesLoaded { filter, files } => {
                if let Ok(files) = files {
                    self.resolve_users(files.iter().filter_map(|file| file.user.clone()));
                }
                match &filter.channel {
                    Some(channel) => {
                        self.ws.files.insert(channel.clone(), files.clone());
                    }
                    // A workspace-wide list answers the Files view.
                    None => self
                        .files_page
                        .update(cx, |page, cx| page.land(filter, files, cx)),
                }
                cx.notify();
            }
            Event::FilePermissionRequested { .. } => {
                notify("Access requested", "Edit access request sent.", window, cx);
            }
            Event::FilesExtrasLoaded {
                favorites,
                collections,
                deleted,
            } => {
                self.files_page.update(cx, |page, cx| {
                    page.set_extras(favorites.clone(), collections.clone(), deleted.clone(), cx);
                });
            }
            Event::FileRestored { file_id } => {
                self.files_page
                    .update(cx, |page, cx| page.mark_restored(file_id, cx));
                notify("Restored", "Restored item from trash.", window, cx);
                cx.notify();
            }
            Event::FileDeleted { file_id } => {
                self.files_page
                    .update(cx, |page, cx| page.remove_file(file_id, cx));
                for list in self.ws.files.values_mut().flatten() {
                    list.retain(|f| f.id.as_str() != file_id);
                }
                notify("File deleted", "Removed file from Slack.", window, cx);
                cx.notify();
            }
            _ => {}
        }
    }

    fn on_app_home_event(&mut self, event: &Event, cx: &mut Context<Self>) {
        match event {
            Event::AppHomeLoaded { channel, home } => {
                let value = match home {
                    Ok(data) => Ok((**data).clone()),
                    Err(err) => Err(err.clone()),
                };
                self.ws.app_homes.insert(channel.clone(), value);
                cx.notify();
            }
            Event::AppHomeUpdated(view) => {
                let app_id = view.get("app_id").and_then(|v| v.as_str());
                let mut updated = false;
                if let Some(target_app) = app_id {
                    for entry in self.ws.app_homes.values_mut().flatten() {
                        if entry
                            .home_view
                            .as_ref()
                            .and_then(|v| v.get("app_id"))
                            .and_then(|v| v.as_str())
                            == Some(target_app)
                        {
                            entry.home_tab_enabled = true;
                            entry.home_view = Some(view.clone());
                            updated = true;
                        }
                    }
                }
                if !updated && self.tab == ConversationTab::Home {
                    self.ws.app_homes.insert(
                        self.channel.clone(),
                        Ok(slackless_core::api::AppHomeResponse {
                            home_tab_enabled: true,
                            messages_tab_enabled: true,
                            home_view: Some(view.clone()),
                        }),
                    );
                }
                cx.notify();
            }
            _ => {}
        }
    }
}

/// Maps user-triggered mutation error prefixes to a toast title so failures surface in the UI,
/// while keeping background hydration/sync/presence/websocket errors log-only.
pub(crate) fn user_facing_error_title(error: &str) -> Option<&'static str> {
    const PREFIXES: &[(&str, &str)] = &[
        ("chat.scheduleMessage:", "Could not schedule message"),
        (
            "chat.deleteScheduledMessage:",
            "Could not cancel scheduled message",
        ),
        ("chat.postMessage:", "Could not send message"),
        ("chat.update:", "Could not edit message"),
        ("chat.delete:", "Could not delete message"),
        ("reactions.add:", "Could not add reaction"),
        ("reactions.remove:", "Could not remove reaction"),
        ("files.upload:", "Could not upload file"),
        ("files.delete:", "Could not delete file"),
        ("files.undelete:", "Could not restore file"),
        ("canvases.create:", "Could not create canvas"),
        ("conversations.canvases.create:", "Could not create canvas"),
        ("canvases.edit:", "Could not save canvas"),
        ("canvases.delete:", "Could not delete canvas"),
        ("canvases.access.set:", "Could not share canvas"),
        ("canvases.access.delete:", "Could not update canvas access"),
        ("canvas.comment.post:", "Could not post comment"),
        ("quip.thread.", "Could not update comment thread"),
        ("lists.create:", "Could not create list"),
        ("lists.records.", "Could not update list"),
        ("lists.cells.update:", "Could not update list cell"),
        ("lists.addToList:", "Could not add to list"),
        ("pins.add:", "Could not pin message"),
        ("pins.remove:", "Could not unpin message"),
        ("bookmarks.add:", "Could not add bookmark"),
        ("bookmarks.remove:", "Could not remove bookmark"),
        ("saved.add", "Could not save for later"),
        ("saved.update:", "Could not update saved item"),
        ("saved.delete:", "Could not remove saved item"),
        ("conversations.create:", "Could not create channel"),
        ("conversations.rename:", "Could not rename channel"),
        ("conversations.archive:", "Could not archive channel"),
        ("conversations.unarchive:", "Could not unarchive channel"),
        ("conversations.invite:", "Could not invite member"),
        ("conversations.setTopic:", "Could not update topic"),
        ("conversations.setPurpose:", "Could not update description"),
        ("users.profile.set:", "Could not update profile"),
        ("users.setPhoto:", "Could not update profile photo"),
        ("dnd.setSnooze:", "Could not pause notifications"),
        ("rooms.join:", "Could not join huddle"),
    ];
    PREFIXES
        .iter()
        .find_map(|(prefix, title)| error.starts_with(prefix).then_some(*title))
}

#[cfg(test)]
mod tests {
    use super::user_facing_error_title;

    #[test]
    fn user_facing_error_title_surfaces_mutations_and_hides_background_noise() {
        assert_eq!(
            user_facing_error_title("chat.scheduleMessage: slack api error: invalid_arguments"),
            Some("Could not schedule message")
        );
        assert_eq!(
            user_facing_error_title("chat.postMessage: network error"),
            Some("Could not send message")
        );
        assert_eq!(
            user_facing_error_title("canvases.access.set: slack api error: not_allowed"),
            Some("Could not share canvas")
        );
        assert_eq!(
            user_facing_error_title("rooms.join: slack api error: cannot_huddle_here"),
            Some("Could not join huddle")
        );
        // Background sync/hydration/presence noise must stay log-only.
        for bg in [
            "websocket: connection reset",
            "bots.info: slack api error: bot_not_found",
            "users.info: timeout",
            "typing: socket closed",
            "presence_sub: timeout",
            "backfill C1: timeout",
            "client.counts: timeout",
        ] {
            assert_eq!(
                user_facing_error_title(bg),
                None,
                "unexpected toast for {bg}"
            );
        }
    }
}
