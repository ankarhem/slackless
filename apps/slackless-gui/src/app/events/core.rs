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
            Event::Counts(counts) => {
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
                        let server_has_newer =
                            entry.latest.as_ref().is_some_and(|latest| *latest > marked);
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
            Event::ReadMarker { channel, ts } => {
                self.apply_read_marker(channel, ts, cx);
            }
            Event::StoreChanged { channel } => {
                if *channel == self.channel {
                    self.load_history(channel, cx);
                    // The channel's people are the history's recent authors: stale now.
                    self.channel_people = None;
                    self.load_join_people(cx);
                }
                self.refresh_dm_latest(channel, cx);
                // The open Unreads view follows a message landing in any channel.
                if self.unreads_open {
                    self.refresh_unreads(cx);
                }
                // The open thread: a fetched replies page landed, or a live reply arrived.
                if self.thread_open {
                    let open = self.thread.read(cx).open_thread();
                    if let Some((thread_channel, thread_ts)) = open
                        && thread_channel == *channel
                    {
                        self.load_thread(&thread_channel, &thread_ts, cx);
                    }
                }
                // Any expanded card in the Threads view refreshes when its replies land.
                if self.threads_open {
                    let expanded = self.threads.read(cx).expanded_in(channel);
                    for thread_ts in expanded {
                        if let Some(read) = self.bridge.thread(channel.clone(), thread_ts.clone()) {
                            let (ch, ts) = (channel.clone(), thread_ts);
                            cx.spawn(async move |this, cx| {
                                let msgs = cx.background_executor().spawn(read).await;
                                let _ = this.update(cx, |app, cx| {
                                    app.resolve_named_users(&msgs);
                                    let replies: Vec<_> =
                                        msgs.into_iter().filter(|m| m.ts != ts).collect();
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
            Event::AuthExpired(error) => eprintln!(
                "slackless: Slack rejected the saved session ({error}); sign in again with `slackless-gui --sign-in`"
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
                eprintln!("slackless: {command} failed ({error})");
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
            Event::Error(error) => eprintln!("slackless: {error}"),
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
                } else if self.threads_open {
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
            } => {
                if let Some(count) = num_members {
                    self.ws.member_counts.insert(channel.clone(), *count);
                }
                if let Some(apps) = num_apps {
                    self.ws.app_counts.insert(channel.clone(), *apps);
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
                        .insert(channel.clone(), members.clone());
                }
                // A late answer for a channel left since is moot: the open one's follows.
                if *channel == self.channel {
                    if !members.is_empty() {
                        self.open_members = Some((channel.clone(), members.clone()));
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
                        details.set_conversation_info(channel, members.clone(), cx)
                    });
                }
                cx.notify();
            }
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
            Event::Connection(state) => {
                self.ws.connection = Some(state.clone());
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
            }
            Event::ChannelCanvasCreated { channel, canvas_id } if *channel == self.channel => {
                cx.open_url(&self.canvas_link(canvas_id));
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
            Event::HuddleUpdated { .. }
            | Event::HuddleSessionChanged(_)
            | Event::HuddleAudioLevels(_)
            | Event::HuddleVideoFrame { .. }
            | Event::HuddleVideoTileClosed(_)
            | Event::HuddleDataEvent(_)
            | Event::HuddleInvite { .. }
            | Event::HuddleInviteCancelled { .. }
            | Event::HuddleLinkLoaded { .. } => self.on_huddle_core_event(event, window, cx),
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
            Event::CloudDrafts(cloud_drafts) => {
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
            Event::BlockSuggestions { key, options } => {
                self.messages.update(cx, |list, cx| {
                    list.set_block_suggestions(key.clone(), options.clone(), cx);
                });
            }
            Event::CanvasLoaded {
                file_id,
                title,
                html,
            } => {
                let parsed = match html {
                    Ok(raw) => Ok(crate::canvas::CanvasDoc::from_html(title.clone(), raw)),
                    Err(err) => Err(err.clone()),
                };
                self.ws.canvases.insert(file_id.clone(), parsed);
                cx.notify();
            }
            Event::CanvasCreated {
                file_id,
                title,
                channel: _,
            } => {
                if !file_id.is_empty() {
                    self.open_live_canvas(file_id.clone(), cx);
                }
                notify("Canvas created", title.clone(), window, cx);
            }
            Event::FilePermissionRequested { file_id: _ } => {
                notify("Access requested", "Edit access request sent.", window, cx);
            }
            Event::ListLoaded { file_id, list } => {
                let value = match list {
                    Ok(data) => Ok((**data).clone()),
                    Err(err) => Err(err.clone()),
                };
                self.ws.lists.insert(file_id.clone(), value);
                cx.notify();
            }
            Event::ListCreated { file_id, name } => {
                if !file_id.is_empty() {
                    self.open_live_list(file_id.clone(), cx);
                }
                notify("List created", name.clone(), window, cx);
            }
            Event::MessageAddedToList { list_id, .. } => {
                self.bridge.send(Command::LoadList {
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
                window.refresh();
                cx.notify();
            }
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
            Event::WorkflowTripped { .. } => {
                notify(
                    "Workflow started",
                    "Workflow trigger dispatched.",
                    window,
                    cx,
                );
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
}
