//! Front-end `Command` dispatch and sequential offline mutation queue flushing.

use super::*;

impl<B: Backend> Core<B> {
    pub fn handle(&mut self, command: Command) {
        if self.is_offline {
            if let Some(mutation) = QueuedMutation::from_command(&command) {
                let payload = serde_json::to_string(&mutation).unwrap_or_default();
                let id = self
                    .store
                    .lock()
                    .ok()
                    .and_then(|s| s.enqueue_offline(&self.workspace, &payload).ok())
                    .unwrap_or(0);
                self.offline_queue.push_back((id, mutation));
                self.events
                    .emit(Event::OfflineQueueChanged(self.offline_queue.len()));
                return;
            }
        }
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        match command {
            Command::OpenConversation(channel) => {
                self.open(channel.clone());
                self.merge_channel_info(channel.clone());
                self.fetch_peer_dnd(channel.clone());
                self.watch_peer_presence(channel);
            }
            Command::FetchLatestHistory(channel) => {
                let unfurls = self.unfurls.clone();
                self.spawn("conversations.history", async move {
                    let items = wait_out_rate_limits(|| {
                        backend
                            .recent_history(channel.clone(), crate::backend::UNREADS_HISTORY_LIMIT)
                    })
                    .await?;
                    unfurls.resolve(items.iter());
                    let target = channel.clone();
                    with_store(&store, move |s| s.upsert_messages(&ws, &target, &items)).await?;
                    events.emit(Event::StoreChanged { channel });
                    Ok(())
                });
            }
            Command::CloseConversation(channel) => {
                if let Some(task) = self.backfills.remove(&channel) {
                    task.abort();
                }
            }
            Command::ResolveUsers(ids) => self.resolve_users(ids),
            Command::ResolveBots(ids) => {
                for bot in ids {
                    if let Some(profile) = self
                        .bots
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .get(&bot)
                        .cloned()
                    {
                        self.events.emit(Event::BotProfileLoaded {
                            bot_id: bot,
                            profile,
                        });
                        continue;
                    }
                    if !self.bots_asked.insert(bot.clone()) {
                        continue;
                    }
                    let backend = self.backend.clone();
                    let store = Arc::clone(&self.store);
                    let bots = Arc::clone(&self.bots);
                    let ws = self.workspace.clone();
                    let events = self.events.clone();
                    self.spawn("bots.info", async move {
                        let profile = backend.bot_info(bot.clone()).await?;
                        bots.lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert(bot.clone(), profile.clone());
                        let bot_store = bot.clone();
                        let profile_store = profile.clone();
                        let _ = with_store(&store, move |s| {
                            s.upsert_bot_profile(&ws, &bot_store, &profile_store)
                        })
                        .await;
                        events.emit(Event::BotProfileLoaded {
                            bot_id: bot,
                            profile,
                        });
                        Ok(())
                    });
                }
            }
            Command::SetNotifyLevel { channel, level } => {
                self.spawn("users.prefs.setNotifications", async move {
                    for &(name, value) in level.writes() {
                        backend
                            .set_notification_pref(channel.clone(), name, value)
                            .await?;
                    }
                    with_store(&store, move |s| {
                        let mut prefs = s.prefs(&ws)?.unwrap_or_default();
                        crate::notify::set_notify_level(&mut prefs, &channel, level);
                        s.replace_prefs(&ws, &prefs)
                    })
                    .await?;
                    events.emit(Event::SidebarChanged);
                    Ok(())
                });
            }
            Command::LeaveConversation(channel) => {
                self.spawn("conversations.leave", async move {
                    backend.leave_conversation(channel.clone()).await?;
                    let target = channel.clone();
                    with_store(&store, move |s| match s.conversation(&ws, &target)? {
                        Some(mut left) => {
                            left.is_member = false;
                            s.upsert_conversation(&ws, &left)
                        }
                        None => Ok(()),
                    })
                    .await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::JoinConversation(channel) => {
                self.spawn("conversations.join", async move {
                    let mut joined = backend.join_conversation(channel.clone()).await?;
                    joined.is_member = true;
                    with_store(&store, move |s| s.upsert_conversation(&ws, &joined)).await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::SetChannelTopic { channel, topic } => {
                self.spawn("conversations.setTopic", async move {
                    let updated = backend.set_topic(channel.clone(), topic).await?;
                    with_store(&store, move |s| s.merge_conversation(&ws, &updated)).await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::SetChannelPurpose { channel, purpose } => {
                self.spawn("conversations.setPurpose", async move {
                    let updated = backend.set_purpose(channel.clone(), purpose).await?;
                    with_store(&store, move |s| s.merge_conversation(&ws, &updated)).await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::OpenThread { channel, thread_ts } => {
                let backfill = self.backfill.clone();
                self.spawn("conversations.replies", async move {
                    backfill.fetch_thread(channel, thread_ts).await
                });
            }
            Command::SendMessage {
                channel,
                content,
                thread_ts,
                broadcast,
            } => {
                let unfurls = self.unfurls.clone();
                self.spawn("chat.postMessage", async move {
                    let posted = backend
                        .post_message(channel.clone(), content, thread_ts, broadcast)
                        .await?;
                    // Store the echo now; the websocket copy of it is an idempotent re-upsert.
                    if let Some(message) = posted.message {
                        unfurls.resolve(std::slice::from_ref(&message));
                        let target = channel.clone();
                        with_store(&store, move |s| s.upsert_message(&ws, &target, &message))
                            .await?;
                        events.emit(Event::StoreChanged { channel });
                    }
                    Ok(())
                });
            }
            // Edits, deletions and reactions reach the store through their websocket echo
            // (`message_changed`, `message_deleted`, `reaction_*`), which carries the edit ts.
            Command::EditMessage {
                channel,
                ts,
                content,
            } => self.spawn("chat.update", async move {
                backend.update_message(channel, ts, content).await?;
                Ok(())
            }),
            Command::DeleteMessage { channel, ts } => self.spawn("chat.delete", async move {
                Ok(backend.delete_message(channel, ts).await?)
            }),
            Command::AddReaction { channel, ts, name } => self.spawn("reactions.add", async move {
                Ok(backend.add_reaction(channel, ts, name).await?)
            }),
            Command::RemoveReaction { channel, ts, name } => {
                self.spawn("reactions.remove", async move {
                    Ok(backend.remove_reaction(channel, ts, name).await?)
                });
            }
            Command::RunCommand {
                channel,
                command,
                args,
                thread_ts,
            } => self.spawn("chat.command", async move {
                let ran = backend
                    .run_command(channel, command.clone(), args, thread_ts, ws)
                    .await;
                match ran {
                    Err(ApiError::Slack(error)) => {
                        events.emit(Event::CommandFailed { command, error });
                        Ok(())
                    }
                    ran => Ok(ran?),
                }
            }),
            Command::BlockAction {
                bot,
                app_id,
                team,
                action,
                container,
            } => self.spawn("blocks.actions", async move {
                let team = team.unwrap_or(ws);
                Ok(backend
                    .block_action(bot, app_id, team, action, container)
                    .await?)
            }),
            Command::Search(query) => self.spawn("search", async move {
                let parsed = SearchQuery::parse(&query);
                let messages =
                    with_store(&store, move |s| s.search(&ws, &parsed, SEARCH_LIMIT)).await?;
                events.emit(Event::SearchResults {
                    query: query.clone(),
                    messages,
                });
                let results = Box::new(backend.search(query.clone()).await?);
                events.emit(Event::RemoteSearchResults { query, results });
                Ok(())
            }),
            Command::MarkRead { channel, ts } => self.spawn("conversations.mark", async move {
                backend.mark_read(channel.clone(), ts.clone()).await?;
                let _ = with_store(&store, move |s| {
                    if let Some(mut conv) = s.conversation(&ws, &channel)? {
                        conv.last_read = Some(ts);
                        s.upsert_conversation(&ws, &conv)?;
                    }
                    Ok::<_, StoreError>(())
                })
                .await;
                Ok(())
            }),
            Command::MarkThreadRead {
                channel,
                thread_ts,
                ts,
            } => self.spawn("subscriptions.thread.mark", async move {
                Ok(backend.mark_thread_read(channel, thread_ts, ts).await?)
            }),
            Command::MarkAllRead => self.spawn("users.markAllRead", async move {
                backend.mark_all_read().await?;
                events.emit(Event::Counts(Box::new(backend.counts().await?)));
                Ok(())
            }),
            Command::SetThreadSubscribed {
                channel,
                thread_ts,
                last_read,
                subscribed,
            } => {
                let backfill = self.backfill.clone();
                let method = if subscribed {
                    "subscriptions.thread.add"
                } else {
                    "subscriptions.thread.remove"
                };
                self.spawn(method, async move {
                    backend
                        .set_thread_subscribed(
                            channel.clone(),
                            thread_ts.clone(),
                            last_read,
                            subscribed,
                        )
                        .await?;
                    // The stored root still carries the old follow flag.
                    backfill.fetch_thread(channel, thread_ts).await
                });
            }
            Command::FetchChannelManagers { channel } => {
                self.spawn("admin.roles.entity.listAssignments", async move {
                    let users = backend.channel_managers(channel.clone()).await?;
                    events.emit(Event::ChannelManagers { channel, users });
                    Ok(())
                });
            }
            Command::FetchThreads => {
                self.spawn("subscriptions.thread.getView", async move {
                    let view = Box::new(backend.threads_view().await?);
                    events.emit(Event::ThreadsView(view));
                    Ok(())
                });
            }
            Command::FetchActivity { with_bodies } => self.fetch_activity(with_bodies),
            Command::MarkActivity { entries, read } => {
                self.spawn("activity.mark", async move {
                    if read {
                        backend.activity_mark_read(entries).await?
                    } else {
                        backend.activity_mark_unread(entries).await?
                    }
                    Ok(())
                });
            }
            Command::OpenDm { users } => self.spawn("conversations.open", async move {
                let dm = backend.open_dm(users).await?;
                let channel = dm.id.clone();
                with_store(&store, move |s| s.upsert_conversation(&ws, &dm)).await?;
                events.emit(Event::Conversations(vec![channel.clone()]));
                events.emit(Event::ConversationOpened(channel));
                Ok(())
            }),
            Command::CreateSection { name } => {
                self.spawn("users.channelSections.create", async move {
                    backend.create_section(name).await?;
                    let sections = backend.channel_sections().await?;
                    with_store(&store, move |s| s.replace_sections(&ws, &sections)).await?;
                    events.emit(Event::SidebarChanged);
                    Ok(())
                });
            }
            Command::CreateSectionWithChannel { name, channel } => {
                self.spawn("users.channelSections.create", async move {
                    let created = backend.create_section(name).await?;
                    let section = created.channel_section_id;
                    // The channel may sit in another section: move, not just add.
                    let (probe, read_ws) = (channel.clone(), ws.clone());
                    let source = with_store(&store, move |s| {
                        s.sections(&read_ws).map(|sections| {
                            sections
                                .iter()
                                .find(|known| known.channel_ids.contains(&probe))
                                .map(|known| known.id.clone())
                        })
                    })
                    .await?;
                    backend
                        .move_channel(
                            Some((section.clone(), channel.clone())),
                            source.map(|from| (from, channel.clone())),
                        )
                        .await?;
                    let sections = backend.channel_sections().await?;
                    with_store(&store, move |s| s.replace_sections(&ws, &sections)).await?;
                    events.emit(Event::SidebarChanged);
                    Ok(())
                });
            }
            Command::RenameSection { section, name } => {
                self.spawn("users.channelSections.set", async move {
                    // `.set` clears every omitted key, so the emoji is re-read and resent.
                    let (read_ws, target) = (ws.clone(), section.clone());
                    let emoji = with_store(&store, move |s| {
                        s.sections(&read_ws).map(|sections| {
                            sections
                                .iter()
                                .find(|known| known.id == target)
                                .map(|known| known.emoji.clone())
                                .unwrap_or_default()
                        })
                    })
                    .await?;
                    backend.rename_section(section, name, emoji).await?;
                    let sections = backend.channel_sections().await?;
                    with_store(&store, move |s| s.replace_sections(&ws, &sections)).await?;
                    events.emit(Event::SidebarChanged);
                    Ok(())
                });
            }
            Command::DeleteSection { section } => {
                self.spawn("users.channelSections.delete", async move {
                    backend.delete_section(section).await?;
                    let sections = backend.channel_sections().await?;
                    with_store(&store, move |s| s.replace_sections(&ws, &sections)).await?;
                    events.emit(Event::SidebarChanged);
                    Ok(())
                });
            }
            Command::MoveChannel { channel, from, to } => {
                self.spawn("users.channelSections.channels.bulkUpdate", async move {
                    if from != to {
                        backend
                            .move_channel(
                                to.map(|to| (to, channel.clone())),
                                from.map(|from| (from, channel.clone())),
                            )
                            .await?;
                    }
                    let sections = backend.channel_sections().await?;
                    with_store(&store, move |s| s.replace_sections(&ws, &sections)).await?;
                    events.emit(Event::SidebarChanged);
                    Ok(())
                });
            }
            Command::Typing { channel, thread_ts } => {
                if let Some(Err(e)) = self.rtm.as_ref().map(|r| r.user_typing(channel, thread_ts)) {
                    events.emit(Event::Error(format!("typing: {e}")));
                }
            }
            Command::LoadPins(channel) => self.spawn("pins.list", async move {
                let pins = backend.pins(channel.clone()).await;
                answer_load(&events, pins, |pins| Event::PinsLoaded { channel, pins })
            }),
            Command::LoadBookmarks(channel) => self.spawn("bookmarks.list", async move {
                let bookmarks = backend.bookmarks(channel.clone()).await;
                answer_load(&events, bookmarks, |bookmarks| Event::BookmarksLoaded {
                    channel,
                    bookmarks,
                })
            }),
            Command::LoadFiles(filter) => self.spawn("files.list", async move {
                let files = backend.files(filter.clone()).await;
                answer_load(&events, files, |files| Event::FilesLoaded { filter, files })
            }),
            Command::AddBookmark {
                channel,
                title,
                url,
            } => {
                self.spawn("bookmarks.add", async move {
                    backend.add_bookmark(channel.clone(), title, url).await?;
                    let bookmarks = backend.bookmarks(channel.clone()).await;
                    answer_load(&events, bookmarks, |bookmarks| Event::BookmarksLoaded {
                        channel,
                        bookmarks,
                    })
                });
            }
            Command::SetPinned {
                channel,
                ts,
                pinned,
            } => {
                let method = if pinned { "pins.add" } else { "pins.remove" };
                self.spawn(method, async move {
                    backend
                        .set_pinned(channel.clone(), ts.clone(), pinned)
                        .await?;
                    let pins = backend.pins(channel.clone()).await;
                    answer_load(&events, pins, |pins| Event::PinsLoaded { channel, pins })
                });
            }
            Command::CreateChannelCanvas(channel) => {
                self.spawn("conversations.canvases.create", async move {
                    let canvas_id = backend.create_canvas(channel.clone()).await?;
                    // Refresh the stored conversation so `properties.canvas` lands; the tab
                    // row re-derives from the next snapshot.
                    let fresh = backend.conversation_info(channel.clone()).await?;
                    with_store(&store, move |s| s.merge_conversation(&ws, &fresh)).await?;
                    events.emit(Event::Conversations(vec![channel.clone()]));
                    events.emit(Event::ChannelCanvasCreated { channel, canvas_id });
                    Ok(())
                });
            }
            Command::SetDndSnooze { minutes } => {
                self.spawn("dnd.setSnooze", async move {
                    let status = backend.set_dnd_snooze(minutes).await?;
                    events.emit(Event::SelfDndUpdated(status));
                    Ok(())
                });
            }
            Command::EndDndSnooze => {
                self.spawn("dnd.endSnooze", async move {
                    let status = backend.end_dnd_snooze().await?;
                    events.emit(Event::SelfDndUpdated(status));
                    Ok(())
                });
            }
            Command::EndDnd => {
                self.spawn("dnd.endDnd", async move {
                    backend.dnd_end_dnd().await?;
                    events.emit(Event::SelfDndUpdated(slackless_types::DndStatus::default()));
                    Ok(())
                });
            }
            Command::LoadTeamDnd(users) => {
                self.spawn("dnd.teamInfo", async move {
                    let map = backend.dnd_team_info(users).await?;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs() as i64);
                    let paused = map
                        .into_iter()
                        .map(|(uid, status)| (uid, status.paused(now)))
                        .collect();
                    events.emit(Event::TeamDndLoaded(paused));
                    Ok(())
                });
            }
            Command::ScheduleMessage {
                channel,
                post_at,
                content,
                thread_ts,
            } => {
                self.spawn("chat.scheduleMessage", async move {
                    let created = backend
                        .schedule_message(channel.clone(), post_at, content, thread_ts)
                        .await?;
                    events.emit(Event::MessageScheduled {
                        channel: created.channel,
                        scheduled_message_id: created.scheduled_message_id,
                        post_at: created.post_at,
                    });
                    let list = backend.scheduled_messages().await;
                    answer_load(&events, list, Event::ScheduledMessagesLoaded)
                });
            }
            Command::LoadScheduledMessages => {
                self.spawn("chat.scheduledMessages.list", async move {
                    let list = backend.scheduled_messages().await;
                    answer_load(&events, list, Event::ScheduledMessagesLoaded)
                });
            }
            Command::DeleteScheduledMessage {
                channel,
                scheduled_message_id,
            } => {
                self.spawn("chat.deleteScheduledMessage", async move {
                    backend
                        .delete_scheduled_message(channel, scheduled_message_id)
                        .await?;
                    let list = backend.scheduled_messages().await;
                    answer_load(&events, list, Event::ScheduledMessagesLoaded)
                });
            }
            Command::UploadFiles {
                channel,
                thread_ts,
                content,
                files,
            } => {
                self.spawn("files.upload", async move {
                    let uploaded = backend
                        .upload_files(channel.clone(), thread_ts, content, files)
                        .await?;
                    events.emit(Event::FilesUploaded {
                        channel,
                        count: uploaded.len(),
                    });
                    Ok(())
                });
            }
            Command::SearchGifs { query } => {
                let method = if query.trim().is_empty() {
                    "gif.featured"
                } else {
                    "gif.search"
                };
                self.spawn(method, async move {
                    let trimmed = query.trim().to_owned();
                    let gifs = if trimmed.is_empty() {
                        backend.gif_featured(24).await?
                    } else {
                        backend.gif_search(trimmed.clone(), 24).await?
                    };
                    events.emit(Event::GifResults { query, gifs });
                    Ok(())
                });
            }
            Command::LoadSaved => self.fetch_saved(),
            Command::SaveItem {
                channel,
                ts,
                date_due,
            } => {
                self.spawn("saved.add", async move {
                    match backend
                        .saved_add(channel.clone(), ts.clone(), date_due)
                        .await
                    {
                        Ok(()) => {}
                        Err(ApiError::Slack(ref error))
                            if error == "already_saved" || error == "already_exists" =>
                        {
                            if let Some(due) = date_due {
                                backend
                                    .saved_update(channel, ts, None, Some(Some(due)))
                                    .await?;
                            }
                        }
                        Err(e) => return Err(e.into()),
                    }
                    let loaded = load_saved_items(&backend, &events).await;
                    answer_load(&events, loaded, Event::SavedLoaded)
                });
            }
            Command::UpdateSavedItem {
                channel,
                ts,
                mark,
                date_due,
            } => {
                self.spawn("saved.update", async move {
                    backend.saved_update(channel, ts, mark, date_due).await?;
                    let loaded = load_saved_items(&backend, &events).await;
                    answer_load(&events, loaded, Event::SavedLoaded)
                });
            }
            Command::DeleteSavedItem { channel, ts } => {
                self.spawn("saved.delete", async move {
                    backend.saved_delete(channel, ts).await?;
                    let loaded = load_saved_items(&backend, &events).await;
                    answer_load(&events, loaded, Event::SavedLoaded)
                });
            }
            Command::ClearCompletedSaved => {
                self.spawn("saved.clearCompleted", async move {
                    backend.saved_clear_completed().await?;
                    let loaded = load_saved_items(&backend, &events).await;
                    answer_load(&events, loaded, Event::SavedLoaded)
                });
            }
            Command::QuickSwitchSearch { query } => {
                self.spawn("edgeapi.search", async move {
                    let trimmed = query.trim().to_owned();
                    if trimmed.is_empty() {
                        return Ok(());
                    }
                    let results = backend.edge_search(ws, trimmed).await?;
                    events.emit(Event::QuickSwitchResults {
                        query,
                        results: Box::new(results),
                    });
                    Ok(())
                });
            }
            Command::SearchRemoteMessages { query, sort, page } => {
                self.spawn("search.modules.messages", async move {
                    let res = backend
                        .search_remote_messages(query.clone(), sort, page)
                        .await;
                    let emit = match res {
                        Ok(data) => Ok(Box::new(data)),
                        Err(e) => Err(e.to_string()),
                    };
                    events.emit(Event::RemoteMessagesPage {
                        query,
                        sort,
                        page,
                        result: emit,
                    });
                    Ok(())
                });
            }
            Command::SearchRemoteFiles { query, sort, page } => {
                self.spawn("search.modules.files", async move {
                    let res = backend.search_remote_files(query.clone(), sort, page).await;
                    let emit = match res {
                        Ok(data) => Ok(Box::new(data)),
                        Err(e) => Err(e.to_string()),
                    };
                    events.emit(Event::RemoteFilesPage {
                        query,
                        sort,
                        page,
                        result: emit,
                    });
                    Ok(())
                });
            }
            Command::UpdateProfile { update } => {
                let self_user_id = Arc::clone(&self.self_user_id);
                self.spawn("users.profile.set", async move {
                    let profile = backend.update_profile(update).await?;
                    let me = self_user_id.lock().ok().and_then(|g| g.clone());
                    if let Some(me) = me {
                        let me_for_store = me.clone();
                        let _ = with_store(&store, move |s| {
                            if let Some(mut user) =
                                s.users(&ws)?.into_iter().find(|u| u.id == me_for_store)
                            {
                                user.profile = profile;
                                s.upsert_user(&ws, &user)?;
                            }
                            Ok::<_, StoreError>(())
                        })
                        .await;
                        events.emit(Event::UsersChanged(vec![me]));
                    }
                    Ok(())
                });
            }
            Command::SetPresence { away } => {
                let self_user_id = Arc::clone(&self.self_user_id);
                self.spawn("presence.set", async move {
                    backend.set_presence(away).await?;
                    let me = self_user_id.lock().ok().and_then(|g| g.clone());
                    if let Some(me) = me {
                        events.emit(Event::Presence {
                            users: vec![me],
                            presence: if away {
                                crate::Presence::Away
                            } else {
                                crate::Presence::Active
                            },
                        });
                    }
                    Ok(())
                });
            }
            Command::SetUserPref { name, value } => {
                self.spawn("users.prefs.set", async move {
                    backend.set_user_pref(name, value).await?;
                    Ok(())
                });
            }
            Command::FetchUserProfile(user) => {
                self.spawn("users.profile.get", async move {
                    let (profile, labels) = tokio::join!(
                        backend.user_profile(user.clone()),
                        backend.team_profile_fields(),
                    );
                    let profile = profile?;
                    let labels = labels.unwrap_or_default();
                    let pronouns = profile.pronouns.clone().filter(|p| !p.trim().is_empty());
                    let mut fields = Vec::new();
                    if let Some(map) = profile.fields.as_ref().and_then(|v| v.as_object()) {
                        for (id, entry) in map {
                            let val = entry
                                .get("value")
                                .and_then(|v| v.as_str())
                                .map(str::trim)
                                .unwrap_or_default();
                            if val.is_empty() {
                                continue;
                            }
                            let label = entry
                                .get("label")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                                .map(str::to_owned)
                                .or_else(|| labels.get(id).cloned())
                                .unwrap_or_else(|| id.clone());
                            fields.push((label, val.to_owned()));
                        }
                    }
                    events.emit(Event::UserProfileLoaded {
                        user,
                        pronouns,
                        fields,
                    });
                    Ok(())
                });
            }
            Command::SubmitModal {
                view_id,
                hash,
                state_values,
            } => {
                self.spawn("views.submit", async move {
                    backend.views_submit(view_id, hash, state_values).await?;
                    Ok(())
                });
            }
            Command::CloseModal { view_id, hash } => {
                self.spawn("views.close", async move {
                    backend.views_close(view_id, hash).await?;
                    Ok(())
                });
            }
            Command::AddBookmarkInFolder {
                channel,
                title,
                url,
                parent_id,
            } => {
                self.spawn("bookmarks.add", async move {
                    backend
                        .add_bookmark_in_folder(channel.clone(), title, url, parent_id)
                        .await?;
                    let bookmarks = backend.bookmarks(channel.clone()).await;
                    answer_load(&events, bookmarks, |bookmarks| Event::BookmarksLoaded {
                        channel,
                        bookmarks,
                    })
                });
            }
            Command::RemoveBookmark {
                channel,
                bookmark_id,
            } => {
                self.spawn("bookmarks.remove", async move {
                    backend
                        .remove_bookmark(channel.clone(), bookmark_id)
                        .await?;
                    let bookmarks = backend.bookmarks(channel.clone()).await;
                    answer_load(&events, bookmarks, |bookmarks| Event::BookmarksLoaded {
                        channel,
                        bookmarks,
                    })
                });
            }
            Command::FetchCloudDrafts => {
                let draft_ids = Arc::clone(&self.cloud_draft_ids);
                self.spawn("drafts.list", async move {
                    let drafts = backend.drafts_list().await?;
                    if let Ok(mut guard) = draft_ids.lock() {
                        guard.clear();
                        for d in &drafts {
                            guard.insert(d.channel.clone(), d.id.clone());
                        }
                    }
                    events.emit(Event::CloudDrafts(drafts));
                    Ok(())
                });
            }
            Command::SyncDraft { channel, text } => {
                let draft_ids = Arc::clone(&self.cloud_draft_ids);
                let channel_lock = {
                    let mut locks = self
                        .draft_locks
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    Arc::clone(
                        locks
                            .entry(channel.clone())
                            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
                    )
                };
                self.spawn("drafts.sync", async move {
                    let _permit = channel_lock.lock().await;
                    let existing = draft_ids.lock().ok().and_then(|g| g.get(&channel).cloned());
                    if text.trim().is_empty() {
                        if let Some(id) = existing {
                            if backend.drafts_delete(id).await.is_ok() {
                                if let Ok(mut guard) = draft_ids.lock() {
                                    guard.remove(&channel);
                                }
                            }
                        }
                    } else if let Some(id) = existing {
                        let _ = backend.drafts_update(id, channel, text).await;
                    } else if let Ok(Some(new_id)) =
                        backend.drafts_create(channel.clone(), text).await
                    {
                        if let Ok(mut guard) = draft_ids.lock() {
                            guard.insert(channel, new_id);
                        }
                    }
                    Ok(())
                });
            }
            Command::BlockSuggestions {
                key,
                bot,
                app_id,
                team,
                action_id,
                block_id,
                value,
                container,
            } => {
                self.spawn("blocks.suggestions", async move {
                    let options = backend
                        .block_suggestions(bot, app_id, team, action_id, block_id, value, container)
                        .await?;
                    events.emit(Event::BlockSuggestions { key, options });
                    Ok(())
                });
            }
            Command::CreateChannel {
                name,
                is_private,
                purpose,
            } => {
                self.spawn("conversations.create", async move {
                    let mut created = backend.create_channel(name, is_private).await?;
                    let id = created.id.clone();
                    if let Some(purpose) = purpose.filter(|p| !p.trim().is_empty()) {
                        if let Ok(updated) = backend.set_purpose(id.clone(), purpose).await {
                            created = updated;
                        }
                    }
                    created.is_member = true;
                    let for_store = created.clone();
                    with_store(&store, move |s| s.upsert_conversation(&ws, &for_store)).await?;
                    events.emit(Event::Conversations(vec![id.clone()]));
                    events.emit(Event::ConversationOpened(id));
                    Ok(())
                });
            }
            Command::RenameChannel { channel, name } => {
                self.spawn("conversations.rename", async move {
                    let updated = backend.rename_channel(channel.clone(), name).await?;
                    with_store(&store, move |s| s.merge_conversation(&ws, &updated)).await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::ArchiveChannel(channel) => {
                self.spawn("conversations.archive", async move {
                    backend.archive_channel(channel.clone()).await?;
                    let id = channel.clone();
                    with_store(&store, move |s| {
                        if let Some(mut conv) = s.conversation(&ws, &id)? {
                            conv.is_archived = true;
                            s.upsert_conversation(&ws, &conv)?;
                        }
                        Ok::<_, StoreError>(())
                    })
                    .await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::UnarchiveChannel(channel) => {
                self.spawn("conversations.unarchive", async move {
                    backend.unarchive_channel(channel.clone()).await?;
                    let id = channel.clone();
                    with_store(&store, move |s| {
                        if let Some(mut conv) = s.conversation(&ws, &id)? {
                            conv.is_archived = false;
                            s.upsert_conversation(&ws, &conv)?;
                        }
                        Ok::<_, StoreError>(())
                    })
                    .await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::DeleteChannel(channel) => {
                self.spawn("admin.conversations.delete", async move {
                    backend.delete_channel(channel.clone()).await?;
                    let id = channel.clone();
                    with_store(&store, move |s| {
                        if let Some(mut conv) = s.conversation(&ws, &id)? {
                            conv.is_archived = true;
                            conv.is_member = false;
                            s.upsert_conversation(&ws, &conv)?;
                        }
                        Ok::<_, StoreError>(())
                    })
                    .await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::ConvertChannelToPrivate(channel) => {
                self.spawn("conversations.convertToPrivate", async move {
                    let updated = backend.convert_channel_to_private(channel.clone()).await?;
                    with_store(&store, move |s| s.merge_conversation(&ws, &updated)).await?;
                    events.emit(Event::Conversations(vec![channel]));
                    events.emit(Event::SidebarChanged);
                    Ok(())
                });
            }
            Command::CreateUsergroup {
                name,
                handle,
                description,
            } => {
                self.spawn("usergroups.create", async move {
                    let _ = backend.create_usergroup(name, handle, description).await?;
                    if let Ok(groups) = backend.usergroups().await {
                        events.emit(Event::Usergroups(groups));
                    }
                    Ok(())
                });
            }
            Command::UpdateUsergroup {
                usergroup,
                name,
                handle,
                description,
            } => {
                self.spawn("usergroups.update", async move {
                    let _ = backend
                        .update_usergroup(usergroup, name, handle, description)
                        .await?;
                    if let Ok(groups) = backend.usergroups().await {
                        events.emit(Event::Usergroups(groups));
                    }
                    Ok(())
                });
            }
            Command::InviteToChannel { channel, users } => {
                if !users.is_empty() {
                    self.spawn("conversations.invite", async move {
                        let updated = backend.invite_to_channel(channel.clone(), users).await?;
                        let ws_merge = ws.clone();
                        with_store(&store, move |s| s.merge_conversation(&ws_merge, &updated))
                            .await?;
                        if let Ok(members) = backend.conversation_members(channel.clone()).await {
                            let for_counts = members.clone();
                            let counts =
                                with_store(&store, move |s| s.member_counts(&ws, &for_counts))
                                    .await
                                    .ok();
                            events.emit(Event::ConversationInfo {
                                channel: channel.clone(),
                                num_members: counts.map(|c| c.humans),
                                num_apps: counts.map(|c| c.apps),
                                members,
                            });
                        }
                        events.emit(Event::Conversations(vec![channel]));
                        Ok(())
                    });
                }
            }
            Command::RetryOfflineMutation(id) => {
                let found = self
                    .store
                    .lock()
                    .ok()
                    .and_then(|s| s.list_offline(&self.workspace).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .find(|(row_id, _)| *row_id == id)
                    .and_then(|(_, payload)| serde_json::from_str::<QueuedMutation>(&payload).ok());
                if let Some(mutation) = found {
                    if self.is_offline {
                        if !self.offline_queue.iter().any(|(row_id, _)| *row_id == id) {
                            self.offline_queue.push_back((id, mutation));
                            self.events
                                .emit(Event::OfflineQueueChanged(self.offline_queue.len()));
                        }
                    } else {
                        self.flush_queued_mutation(id, mutation);
                    }
                }
            }
            Command::DiscardOfflineMutation(id) => {
                self.offline_queue.retain(|(row_id, _)| *row_id != id);
                if let Ok(s) = self.store.lock() {
                    let _ = s.delete_offline(id);
                }
                self.events
                    .emit(Event::OfflineQueueChanged(self.offline_queue.len()));
            }
            Command::SignOut => {
                self.spawn("auth.revoke", async move {
                    let _ = backend.auth_revoke().await;
                    Ok(())
                });
            }
            Command::AddConversationTab {
                channel,
                tab_type,
                label,
            } => {
                self.spawn("conversations.addTab", async move {
                    backend
                        .add_conversation_tab(channel.clone(), tab_type, label)
                        .await?;
                    if let Ok(fresh) = backend.conversation_info(channel.clone()).await {
                        with_store(&store, move |s| s.merge_conversation(&ws, &fresh)).await?;
                    }
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::RemoveConversationTab { channel, tab_id } => {
                self.spawn("conversations.removeTab", async move {
                    backend
                        .remove_conversation_tab(channel.clone(), tab_id)
                        .await?;
                    if let Ok(fresh) = backend.conversation_info(channel.clone()).await {
                        with_store(&store, move |s| s.merge_conversation(&ws, &fresh)).await?;
                    }
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::MoveConversationTab {
                channel,
                tab_id,
                after,
            } => {
                self.spawn("conversations.moveTab", async move {
                    backend
                        .move_conversation_tab(channel.clone(), tab_id, after)
                        .await?;
                    if let Ok(fresh) = backend.conversation_info(channel.clone()).await {
                        with_store(&store, move |s| s.merge_conversation(&ws, &fresh)).await?;
                    }
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::SaveFile { file_id, date_due } => {
                self.spawn("saved.add(file)", async move {
                    match backend.save_file(file_id, date_due).await {
                        Ok(()) => {}
                        Err(ApiError::Slack(ref error))
                            if error == "already_saved" || error == "already_exists" => {}
                        Err(e) => return Err(e.into()),
                    }
                    let loaded = load_saved_items(&backend, &events).await;
                    answer_load(&events, loaded, Event::SavedLoaded)
                });
            }
            Command::DeleteFile { file_id } => {
                self.spawn("files.delete", async move {
                    backend.delete_file(file_id.clone()).await?;
                    events.emit(Event::FileDeleted { file_id });
                    Ok(())
                });
            }
            Command::LoadCanvas { file_id } => {
                self.spawn("files.info(canvas)", async move {
                    match backend.fetch_canvas_html(file_id.clone()).await {
                        Ok((title, html)) => {
                            events.emit(Event::CanvasLoaded {
                                file_id,
                                title,
                                html: Ok(html),
                            });
                        }
                        Err(err) => {
                            events.emit(Event::CanvasLoaded {
                                file_id,
                                title: "Canvas".to_owned(),
                                html: Err(err.to_string()),
                            });
                        }
                    }
                    Ok(())
                });
            }
            Command::CreateCanvas {
                title,
                markdown,
                channel,
            } => {
                self.spawn("canvases.create", async move {
                    let created = backend
                        .canvases_create(
                            Some(title.clone()),
                            (!markdown.is_empty()).then_some(markdown),
                            channel.clone(),
                        )
                        .await?;
                    events.emit(Event::CanvasCreated {
                        file_id: created.canvas_id,
                        title,
                        channel,
                    });
                    Ok(())
                });
            }
            Command::EditCanvas { file_id, op } => {
                self.spawn("canvases.edit", async move {
                    backend.canvases_edit(file_id.clone(), op).await?;
                    match backend.fetch_canvas_html(file_id.clone()).await {
                        Ok((title, html)) => {
                            events.emit(Event::CanvasLoaded {
                                file_id,
                                title,
                                html: Ok(html),
                            });
                        }
                        Err(err) => {
                            events.emit(Event::CanvasLoaded {
                                file_id,
                                title: "Canvas".to_owned(),
                                html: Err(err.to_string()),
                            });
                        }
                    }
                    Ok(())
                });
            }
            Command::RequestFilePermission {
                file_id,
                request_type,
            } => {
                self.spawn("files.requestPermission", async move {
                    backend
                        .files_request_permission(file_id.clone(), request_type)
                        .await?;
                    events.emit(Event::FilePermissionRequested { file_id });
                    Ok(())
                });
            }
            Command::LoadList { file_id } => {
                self.spawn("lists.records.list", async move {
                    let list = backend
                        .fetch_list(file_id.clone())
                        .await
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                    events.emit(Event::ListLoaded { file_id, list });
                    Ok(())
                });
            }
            Command::CreateList {
                name,
                description,
                channel,
            } => {
                self.spawn("lists.create", async move {
                    let file_id = backend
                        .lists_create(name.clone(), description, channel)
                        .await?;
                    events.emit(Event::ListCreated { file_id, name });
                    Ok(())
                });
            }
            Command::CreateListItem {
                list_id,
                initial_fields,
            } => {
                self.spawn("lists.records.create", async move {
                    let _ = backend
                        .lists_records_create(list_id.clone(), initial_fields)
                        .await?;
                    let list = backend
                        .fetch_list(list_id.clone())
                        .await
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                    events.emit(Event::ListLoaded {
                        file_id: list_id,
                        list,
                    });
                    Ok(())
                });
            }
            Command::UpdateListCell {
                list_id,
                row_id,
                cells,
            } => {
                self.spawn("lists.cells.update", async move {
                    backend
                        .lists_cells_update(list_id.clone(), row_id, cells)
                        .await?;
                    let list = backend
                        .fetch_list(list_id.clone())
                        .await
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                    events.emit(Event::ListLoaded {
                        file_id: list_id,
                        list,
                    });
                    Ok(())
                });
            }
            Command::DeleteListItem { list_id, row_id } => {
                self.spawn("lists.records.delete", async move {
                    backend
                        .lists_records_delete(list_id.clone(), row_id)
                        .await?;
                    let list = backend
                        .fetch_list(list_id.clone())
                        .await
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                    events.emit(Event::ListLoaded {
                        file_id: list_id,
                        list,
                    });
                    Ok(())
                });
            }
            Command::AddMessageToList {
                list_id,
                channel,
                ts,
            } => {
                self.spawn("lists.addToList", async move {
                    let record_id = backend
                        .lists_add_to_list(list_id.clone(), channel, ts)
                        .await?;
                    events.emit(Event::MessageAddedToList { list_id, record_id });
                    Ok(())
                });
            }
            Command::ArchiveListItem { list_id, row_id } => {
                self.spawn("lists.records.archive", async move {
                    backend
                        .lists_records_archive(list_id.clone(), row_id)
                        .await?;
                    let list = backend
                        .fetch_list(list_id.clone())
                        .await
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                    events.emit(Event::ListLoaded {
                        file_id: list_id,
                        list,
                    });
                    Ok(())
                });
            }
            Command::SetListAccess {
                list_id,
                access_level,
                channel,
            } => {
                self.spawn("slackLists.access.set", async move {
                    backend
                        .lists_access_set(list_id, access_level, channel)
                        .await?;
                    Ok(())
                });
            }
            Command::FetchListRecordComments { list_id, record_id } => {
                self.spawn("lists.getThreadsForRecord", async move {
                    let messages = backend
                        .lists_get_threads_for_record(list_id.clone(), record_id.clone())
                        .await
                        .unwrap_or_default();
                    events.emit(Event::ListRecordComments {
                        list_id,
                        record_id,
                        messages,
                    });
                    Ok(())
                });
            }
            Command::SetUserPhoto { filename, bytes } => {
                self.spawn("users.setPhoto", async move {
                    backend.users_set_photo(filename, bytes).await?;
                    Ok(())
                });
            }
            Command::LoadAppHome(channel) => {
                self.spawn("conversations.info(app_home)", async move {
                    let home = backend
                        .load_app_home(channel.clone())
                        .await
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                    events.emit(Event::AppHomeLoaded { channel, home });
                    Ok(())
                });
            }
            Command::OpenAppHome { app_id, team_id } => {
                self.spawn("apps.home.open", async move {
                    if let Some(channel) = backend.open_app_home(app_id, team_id).await? {
                        events.emit(Event::ConversationOpened(channel));
                    }
                    Ok(())
                });
            }
            Command::TripWorkflowTrigger { url, context } => {
                self.spawn("workflows.triggers.trip", async move {
                    backend.trip_workflow_trigger(url.clone(), context).await?;
                    events.emit(Event::WorkflowTripped { url });
                    Ok(())
                });
            }
            Command::SetGlobalNotificationPref { name, value, sync } => {
                self.spawn("users.prefs.setNotifications(global)", async move {
                    backend
                        .set_global_notification_pref(name, value, sync)
                        .await?;
                    Ok(())
                });
            }
            Command::LoadFilesExtras => {
                self.spawn("files.extras", async move {
                    let (favorites, collections, deleted) = tokio::join!(
                        backend.files_favorites_list(),
                        backend.files_collections_list(),
                        backend.files_recently_deleted(),
                    );
                    events.emit(Event::FilesExtrasLoaded {
                        favorites: favorites.unwrap_or_default(),
                        collections: collections.unwrap_or_default(),
                        deleted: deleted.unwrap_or_default(),
                    });
                    Ok(())
                });
            }
            Command::SetFileFavorite {
                file_id,
                collection_id,
                starred,
            } => {
                self.spawn("files.favorites", async move {
                    backend
                        .files_favorites_set(file_id, collection_id, starred)
                        .await?;
                    let (favorites, collections, deleted) = tokio::join!(
                        backend.files_favorites_list(),
                        backend.files_collections_list(),
                        backend.files_recently_deleted(),
                    );
                    events.emit(Event::FilesExtrasLoaded {
                        favorites: favorites.unwrap_or_default(),
                        collections: collections.unwrap_or_default(),
                        deleted: deleted.unwrap_or_default(),
                    });
                    Ok(())
                });
            }
            Command::CreateFileCollection { name, emoji } => {
                self.spawn("files.collections.create", async move {
                    let _ = backend.files_collections_create(name, emoji).await?;
                    let (favorites, collections, deleted) = tokio::join!(
                        backend.files_favorites_list(),
                        backend.files_collections_list(),
                        backend.files_recently_deleted(),
                    );
                    events.emit(Event::FilesExtrasLoaded {
                        favorites: favorites.unwrap_or_default(),
                        collections: collections.unwrap_or_default(),
                        deleted: deleted.unwrap_or_default(),
                    });
                    Ok(())
                });
            }
            Command::DeleteFileCollection { collection_id } => {
                self.spawn("files.collections.delete", async move {
                    backend.files_collections_delete(collection_id).await?;
                    let (favorites, collections, deleted) = tokio::join!(
                        backend.files_favorites_list(),
                        backend.files_collections_list(),
                        backend.files_recently_deleted(),
                    );
                    events.emit(Event::FilesExtrasLoaded {
                        favorites: favorites.unwrap_or_default(),
                        collections: collections.unwrap_or_default(),
                        deleted: deleted.unwrap_or_default(),
                    });
                    Ok(())
                });
            }
            Command::RestoreDeletedFile { file_id } => {
                self.spawn("files.undelete", async move {
                    backend.files_undelete(file_id.clone()).await?;
                    events.emit(Event::FileRestored { file_id });
                    let (favorites, collections, deleted) = tokio::join!(
                        backend.files_favorites_list(),
                        backend.files_collections_list(),
                        backend.files_recently_deleted(),
                    );
                    events.emit(Event::FilesExtrasLoaded {
                        favorites: favorites.unwrap_or_default(),
                        collections: collections.unwrap_or_default(),
                        deleted: deleted.unwrap_or_default(),
                    });
                    Ok(())
                });
            }
            Command::JoinHuddle { channel, audio } => {
                // Reject duplicate concurrent join while `rooms.join` is already in-flight;
                // otherwise leave any active huddle and claim a fresh generation id.
                let my_gen = {
                    let mut slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
                    if slot.joining {
                        return;
                    }
                    slot.generation = slot.generation.wrapping_add(1);
                    slot.joining = true;
                    slot.call_id = None;
                    if let Some(prev) = slot.handle.take() {
                        prev.leave();
                    }
                    slot.generation
                };
                let huddle = Arc::clone(&self.huddle);
                let team_id = self.workspace.clone();
                self.spawn("rooms.join", async move {
                    struct JoinGuard {
                        huddle: Arc<Mutex<HuddleSlot>>,
                        generation: u64,
                    }
                    impl Drop for JoinGuard {
                        fn drop(&mut self) {
                            let mut slot =
                                self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
                            if slot.generation == self.generation {
                                slot.joining = false;
                            }
                        }
                    }
                    let _join_guard = JoinGuard {
                        huddle: Arc::clone(&huddle),
                        generation: my_gen,
                    };
                    let joined = backend.rooms_join(channel.clone()).await?;
                    {
                        let slot = huddle.lock().unwrap_or_else(PoisonError::into_inner);
                        if slot.generation != my_gen {
                            return Ok(());
                        }
                    }
                    let call_id = joined.call_id.clone();
                    let creds = crate::huddle::HuddleCredentials {
                        channel: channel.clone(),
                        call_id: joined.call_id,
                        meeting_id: joined.chime.meeting.meeting_id,
                        attendee_id: joined.chime.attendee.attendee_id,
                        external_user_id: joined.chime.attendee.external_user_id,
                        join_token: joined.chime.attendee.join_token,
                        signaling_url: joined.chime.meeting.media_placement.signaling_url,
                        turn_control_url: joined.chime.meeting.media_placement.turn_control_url,
                        audio_host_url: joined.chime.meeting.media_placement.audio_host_url,
                        thread_root_ts: joined
                            .huddle
                            .thread_root_ts
                            .filter(|ts| !ts.as_str().trim().is_empty())
                            .or_else(|| {
                                joined
                                    .canvas
                                    .as_ref()
                                    .and_then(|c| c.root_thread_ts.clone())
                                    .filter(|ts| !ts.as_str().trim().is_empty())
                            }),
                        canvas_file_id: joined.canvas.as_ref().and_then(|c| c.file_id.clone()),
                        huddle_link: joined.huddle.huddle_link.or_else(|| {
                            Some(format!("https://app.slack.com/huddle/{team_id}/{channel}"))
                        }),
                    };
                    let (upd_tx, upd_rx) = tokio::sync::mpsc::unbounded_channel();
                    let handle = crate::huddle::spawn_huddle_session(creds, audio, upd_tx);
                    let rejected = {
                        let mut slot = huddle.lock().unwrap_or_else(PoisonError::into_inner);
                        if slot.generation == my_gen {
                            slot.joining = false;
                            slot.call_id = Some(call_id);
                            slot.handle = Some(handle);
                            None
                        } else {
                            Some(handle)
                        }
                    };
                    if let Some(handle) = rejected {
                        handle.leave();
                        return Ok(());
                    }
                    spawn_huddle_watcher(Arc::clone(&huddle), my_gen, upd_rx, events);
                    Ok(())
                });
            }
            Command::LeaveHuddle => {
                let prev = {
                    let mut slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
                    slot.generation = slot.generation.wrapping_add(1);
                    slot.joining = false;
                    slot.call_id = None;
                    slot.handle.take()
                };
                if let Some(handle) = prev {
                    handle.leave();
                }
            }
            Command::SetHuddleMuted(muted) => {
                if let Some(handle) = self
                    .huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .handle
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetMuted(muted));
                }
            }
            Command::SetHuddleAudioDevices { input, output } => {
                if let Some(handle) = self
                    .huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .handle
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetAudioDevices { input, output });
                }
            }
            Command::SetHuddleAudioProcessing { aec, ns, agc } => {
                if let Some(handle) = self
                    .huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .handle
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetAudioProcessing { aec, ns, agc });
                }
            }
            Command::InviteToHuddle { channel, users } => {
                self.spawn("rooms.notifyMember", async move {
                    for user in users {
                        backend.rooms_notify_member(channel.clone(), user).await?;
                    }
                    Ok(())
                });
            }
            Command::CopyHuddleLink { channel } => {
                self.spawn("rooms.getLink", async move {
                    let url = backend.rooms_get_link(channel.clone()).await?;
                    events.emit(Event::HuddleLinkLoaded { channel, url });
                    Ok(())
                });
            }
            Command::TestHuddleSpeaker { output_device } => {
                let events = self.events.clone();
                tokio::task::spawn_blocking(move || {
                    if let Err(err) =
                        crate::huddle::play_speaker_test_tone(output_device.as_deref())
                    {
                        events.emit(Event::Error(format!("speaker test: {err}")));
                    }
                });
            }
            Command::SetHuddleCamera {
                enabled,
                device_index,
                blur,
            } => {
                if let Some(handle) = self
                    .huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .handle
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetCamera {
                        enabled,
                        device_index,
                        blur,
                    });
                }
            }
            Command::SetHuddleBlurBackground(blur) => {
                if let Some(handle) = self
                    .huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .handle
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetBlurBackground(blur));
                }
            }
            Command::SetHuddleScreenShare {
                source_id,
                share_audio,
            } => {
                if let Some(handle) = self
                    .huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .handle
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetScreenShare {
                        source_id,
                        share_audio,
                    });
                }
            }
            Command::SendHuddleReaction {
                emoji,
                kind,
                emoji_url,
            } => {
                if let Some(handle) = self
                    .huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .handle
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SendReaction {
                        emoji,
                        kind,
                        emoji_url,
                    });
                }
            }
            Command::SetHuddleHandRaised(raised) => {
                if let Some(handle) = self
                    .huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .handle
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetHandRaised(raised));
                }
            }
            Command::SetHuddleStickyEmoji(emoji) => {
                if let Some(handle) = self
                    .huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .handle
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetStickyEmoji(emoji));
                }
            }
            Command::SendHuddleDraw(draw) => {
                if let Some(handle) = self
                    .huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .handle
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SendDraw(draw));
                }
            }
            Command::SetHuddleJukebox(song) => {
                if let Some(handle) = self
                    .huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .handle
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetJukebox(song));
                }
            }
            Command::MuteHuddlePeer(peer_id) => {
                if let Some(handle) = self
                    .huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .handle
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::MutePeer(peer_id));
                }
            }
            Command::SetHuddleTopic(topic) => {
                let call_id = {
                    let slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
                    if let Some(handle) = slot.handle.as_ref() {
                        handle.send(crate::huddle::HuddleControl::SetTopic(topic.clone()));
                    }
                    slot.call_id.clone()
                };
                if let Some(call_id) = call_id {
                    self.spawn("screenhero.rooms.rename", async move {
                        backend.rooms_rename(call_id, topic).await?;
                        Ok(())
                    });
                }
            }
            Command::SetHuddleBackground(background_id) => {
                let call_id = {
                    let slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
                    if let Some(handle) = slot.handle.as_ref() {
                        handle.send(crate::huddle::HuddleControl::SetBackground(
                            (!background_id.is_empty() && background_id != "default")
                                .then(|| background_id.clone()),
                        ));
                    }
                    slot.call_id.clone()
                };
                if let Some(call_id) = call_id {
                    self.spawn("huddles.update", async move {
                        backend
                            .huddles_update_background(call_id, background_id)
                            .await?;
                        Ok(())
                    });
                }
            }
            Command::SetHuddleCaptions(enabled) => {
                let call_id = {
                    let slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
                    if let Some(handle) = slot.handle.as_ref() {
                        handle.send(crate::huddle::HuddleControl::SetCaptionsEnabled(enabled));
                    }
                    slot.call_id.clone()
                };
                if let Some(call_id) = call_id {
                    self.spawn(
                        if enabled {
                            "rooms.startTranscription"
                        } else {
                            "rooms.stopTranscription"
                        },
                        async move {
                            if enabled {
                                backend.rooms_start_transcription(call_id).await?;
                            } else {
                                backend.rooms_stop_transcription(call_id).await?;
                            }
                            Ok(())
                        },
                    );
                }
            }
            Command::SetHuddleAudioOnly(enabled) => {
                if let Some(handle) = self
                    .huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .handle
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetAudioOnly(enabled));
                }
            }
            Command::DeclineHuddleInvite { channel, call_id } => {
                self.spawn("rooms.inviteResponse", async move {
                    backend
                        .rooms_invite_response(channel, call_id, "decline".to_owned())
                        .await?;
                    Ok(())
                });
            }
            // Handled by the loop.
            Command::Shutdown => {}
        }
    }

    /// Execute a batch of persisted offline mutations strictly sequentially (FIFO), deleting each
    /// SQLite row on success or emitting [`Event::OfflineMutationFailed`] on failure.
    pub(super) fn flush_queued_mutations_sequentially(&self, queued: Vec<(i64, QueuedMutation)>) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        let unfurls = self.unfurls.clone();
        tokio::spawn(async move {
            for (id, mutation) in queued {
                let summary = mutation.summary();
                let outcome = Self::execute_queued_mutation(
                    &backend, &store, &ws, &events, &unfurls, mutation,
                )
                .await;
                match outcome {
                    Ok(()) => {
                        if id > 0 {
                            let _ = with_store(&store, move |s| s.delete_offline(id)).await;
                        }
                    }
                    Err(CoreError::Api(ApiError::AuthExpired(code))) => {
                        events.emit(Event::AuthExpired(code));
                        break;
                    }
                    Err(err) => {
                        events.emit(Event::OfflineMutationFailed(FailedMutation {
                            id,
                            summary,
                            error: err.to_string(),
                        }));
                    }
                }
            }
        });
    }

    /// Execute one persisted offline mutation; deletes the SQLite row on success or emits
    /// [`Event::OfflineMutationFailed`] on failure so the user can retry or discard it.
    pub(super) fn flush_queued_mutation(&self, id: i64, mutation: QueuedMutation) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        let unfurls = self.unfurls.clone();
        let summary = mutation.summary();
        tokio::spawn(async move {
            let outcome =
                Self::execute_queued_mutation(&backend, &store, &ws, &events, &unfurls, mutation)
                    .await;
            match outcome {
                Ok(()) => {
                    if id > 0 {
                        let _ = with_store(&store, move |s| s.delete_offline(id)).await;
                    }
                }
                Err(CoreError::Api(ApiError::AuthExpired(code))) => {
                    events.emit(Event::AuthExpired(code));
                }
                Err(err) => {
                    events.emit(Event::OfflineMutationFailed(FailedMutation {
                        id,
                        summary,
                        error: err.to_string(),
                    }));
                }
            }
        });
    }

    pub(super) async fn execute_queued_mutation(
        backend: &B,
        store: &SharedStore,
        ws: &TeamId,
        events: &Emitter,
        unfurls: &Unfurls<B>,
        mutation: QueuedMutation,
    ) -> Result<(), CoreError> {
        match mutation {
            QueuedMutation::SendMessage {
                channel,
                content,
                thread_ts,
                broadcast,
            } => {
                let posted = backend
                    .post_message(channel.clone(), content, thread_ts, broadcast)
                    .await?;
                if let Some(message) = posted.message {
                    unfurls.resolve(std::slice::from_ref(&message));
                    let target = channel.clone();
                    let ws = ws.clone();
                    with_store(store, move |s| s.upsert_message(&ws, &target, &message)).await?;
                    events.emit(Event::StoreChanged { channel });
                }
            }
            QueuedMutation::AddReaction { channel, ts, name } => {
                backend.add_reaction(channel, ts, name).await?;
            }
            QueuedMutation::RemoveReaction { channel, ts, name } => {
                backend.remove_reaction(channel, ts, name).await?;
            }
            QueuedMutation::MarkRead { channel, ts } => {
                backend.mark_read(channel.clone(), ts.clone()).await?;
                let ws = ws.clone();
                let _ = with_store(store, move |s| {
                    if let Some(mut conv) = s.conversation(&ws, &channel)? {
                        conv.last_read = Some(ts);
                        s.upsert_conversation(&ws, &conv)?;
                    }
                    Ok::<_, StoreError>(())
                })
                .await;
            }
        }
        Ok(())
    }
}

/// Forward huddle session updates from `upd_rx` to `events`, tagged with `my_gen` so an older
/// session transitioning to `Idle`/`Failed` can never clobber a newer active session's state.
pub(super) fn spawn_huddle_watcher(
    huddle: Arc<Mutex<HuddleSlot>>,
    my_gen: u64,
    mut upd_rx: tokio::sync::mpsc::UnboundedReceiver<crate::huddle::HuddleUpdate>,
    events: Emitter,
) {
    tokio::spawn(async move {
        while let Some(upd) = upd_rx.recv().await {
            match upd {
                crate::huddle::HuddleUpdate::State(snap) => {
                    let done = matches!(
                        snap.phase,
                        crate::huddle::HuddlePhase::Idle | crate::huddle::HuddlePhase::Failed
                    );
                    let should_emit = {
                        let mut slot = huddle.lock().unwrap_or_else(PoisonError::into_inner);
                        let is_current = slot.generation == my_gen;
                        if is_current && done {
                            slot.handle = None;
                            slot.call_id = None;
                        }
                        is_current || (done && slot.handle.is_none() && !slot.joining)
                    };
                    if should_emit {
                        events.emit(Event::HuddleSessionChanged(snap));
                    }
                }
                crate::huddle::HuddleUpdate::AudioLevels(levels) => {
                    if huddle
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .generation
                        == my_gen
                    {
                        events.emit(Event::HuddleAudioLevels(levels));
                    }
                }
                crate::huddle::HuddleUpdate::VideoFrame { tile, frame } => {
                    if huddle
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .generation
                        == my_gen
                    {
                        events.emit(Event::HuddleVideoFrame { tile, frame });
                    }
                }
                crate::huddle::HuddleUpdate::VideoTileClosed(tile) => {
                    if huddle
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .generation
                        == my_gen
                    {
                        events.emit(Event::HuddleVideoTileClosed(tile));
                    }
                }
                crate::huddle::HuddleUpdate::DataEvent(ev) => {
                    if huddle
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .generation
                        == my_gen
                    {
                        events.emit(Event::HuddleDataEvent(ev));
                    }
                }
            }
        }
    });
}
