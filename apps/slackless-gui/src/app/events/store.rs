//! Async SQLite snapshot reloads, count/read-marker folding, history/thread paging, and user resolution.

use super::super::*;

impl App {
    /// Live mode: rebuild the workspace snapshot off the UI thread on the read-only WAL
    /// connection, debouncing rapid event bursts (40ms) so boot hydration builds `Fixtures` once
    /// instead of spawning concurrent duplicate SQLite reads and peaking heap memory.
    pub(in crate::app) fn reload_live(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bridge.store().is_none() {
            return;
        }
        self.reload_generation += 1;
        let generation = self.reload_generation;
        let workspace = self.workspace;
        self.reload_task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(40))
                .await;
            let task = this
                .update(cx, |app, _| {
                    if app.reload_generation != generation || app.workspace != workspace {
                        return None;
                    }
                    app.bridge.snapshot_async(app.ws.counts.clone())
                })
                .ok()
                .flatten();
            let Some(task) = task else {
                return;
            };
            let snapshot = cx.background_executor().spawn(task).await;
            let Some(mut snapshot) = snapshot else {
                return;
            };
            let _ = this.update_in(cx, |app, window, cx| {
                if app.reload_generation != generation || app.workspace != workspace {
                    return;
                }
                // Keep the full multi-workspace rail intact while updating this workspace's team info.
                if let Some(own) = snapshot.workspaces.first().cloned()
                    && let Some(slot) = app.live_teams.get_mut(app.workspace)
                {
                    *slot = own;
                }
                if !app.live_teams.is_empty() {
                    snapshot.workspaces = app.live_teams.clone();
                }
                // Presence, usergroups, and app_commands are transient (never stored), so they ride along.
                snapshot.presence = app.ws.presence.clone();
                snapshot.bots = app.ws.bots.clone();
                snapshot.usergroups = app.ws.usergroups.clone();
                snapshot.app_commands = app.ws.app_commands.clone();
                app.apply_snapshot(Arc::new(snapshot), window, cx);
            });
        });
    }

    /// Fast in-memory update pushing `self.ws.counts` (and updated per-conversation `last_read`
    /// cursors) into `self.fixtures` and the views that read unread state, without touching SQLite
    /// or requiring `&mut Window`.
    pub(in crate::app) fn sync_counts_to_views(&mut self, cx: &mut Context<Self>) {
        if self.fixtures.sample {
            return;
        }
        let fixtures = Arc::new(self.fixtures.with_counts(self.ws.counts.clone()));
        self.sidebar
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.messages
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.unreads
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.switcher
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.search_menu
            .update(cx, |menu, cx| menu.set_fixtures(fixtures.clone(), cx));
        self.thread
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.threads
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.details
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.fixtures = fixtures;
        if self.unreads_open {
            self.refresh_unreads(cx);
        }
        cx.notify();
    }

    /// Fold an RTM `*_marked` (`im_marked`, `mpim_marked`, `channel_marked`, `group_marked`) event
    /// into local counts, `fixtures`, sidebar, and badge state.
    pub(in crate::app) fn apply_read_marker(
        &mut self,
        channel: &ChannelId,
        ts: &Ts,
        cx: &mut Context<Self>,
    ) {
        let entry = self
            .ws
            .counts
            .entry(channel.clone())
            .or_insert_with(|| ConversationCounts {
                id: channel.clone(),
                ..Default::default()
            });
        entry.last_read = Some(ts.clone());
        let caught_up = entry.latest.as_ref().is_none_or(|latest| *ts >= *latest);
        if caught_up {
            entry.clear_unreads();
            let advance = self
                .ws
                .marked_read
                .get(channel)
                .is_none_or(|prev| *ts > *prev);
            if advance {
                self.ws.marked_read.insert(channel.clone(), ts.clone());
            }
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.mark_read(channel.clone(), cx));
        } else {
            entry.has_unreads = true;
            if self
                .fixtures
                .conversation(channel)
                .is_some_and(|c| matches!(c.kind(), ConversationKind::Im | ConversationKind::Mpim))
                && entry.effective_mentions() == 0
            {
                entry.mention_count = 1;
            }
            self.ws.marked_read.remove(channel);
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.mark_unread(channel, cx));
        }
        self.sync_counts_to_views(cx);
    }

    /// Push a freshly built (or in-memory counts-updated) `Arc<Fixtures>` snapshot to all views.
    pub(in crate::app) fn apply_snapshot(
        &mut self,
        fixtures: Arc<Fixtures>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Refresh the views in place, as Slack does: a status change or a new channel keeps
        // every scroll position, collapsed section and open panel.
        self.sidebar
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.messages
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.thread
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.results
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.browser
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.profile
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.switcher
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.search_menu
            .update(cx, |menu, cx| menu.set_fixtures(fixtures.clone(), cx));
        self.composer
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.threads
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.unreads
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.huddles
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.directory
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.compose
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.details
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.files_page
            .update(cx, |page, cx| page.set_fixtures(fixtures.clone(), cx));
        self.tools_page
            .update(cx, |page, cx| page.set_fixtures(fixtures.clone(), cx));
        let keep = self.ws.chosen && fixtures.conversation(&self.channel).is_some();
        self.fixtures = fixtures;
        // Hydrate custom status from Slack's live self profile (preserving active focus mode).
        if !self.self_state.focused()
            && let Some(me) = self.fixtures.user(&self.fixtures.self_user)
        {
            let remote = crate::status::UserStatus::from_profile(&me.profile);
            if self.self_state.status != remote {
                self.self_state.set_status(remote);
            }
        }
        self.sync_connect_orgs(cx);
        self.resolve_users(self.fixtures.dms.iter().filter_map(|dm| dm.user.clone()));
        if !keep {
            self.show_conversation(first_conversation(&self.fixtures), cx);
        }
        self.sync_composer_placeholder(cx);
        self.apply_preferences(window, cx);
        if self.unreads_open {
            self.refresh_unreads(cx);
        }
        // Conversation names may have arrived: the History menu shows them.
        self.update_menus(cx);
        cx.notify();
    }

    /// Live mode: show `channel`'s stored history, read off the UI thread. A read that a newer
    /// one overtook, or whose conversation is no longer open, is dropped.
    /// Live mode: a DM's messages changed (a send, an arrival, an edit or a deletion): re-read
    /// its newest message off the UI thread so the DM list's preview and order follow, as in
    /// Slack, without rebuilding the whole snapshot.
    pub(in crate::app) fn refresh_dm_latest(
        &mut self,
        channel: &ChannelId,
        cx: &mut Context<Self>,
    ) {
        let is_dm = self
            .fixtures
            .conversation(channel)
            .is_some_and(|c| matches!(c.kind(), ConversationKind::Im | ConversationKind::Mpim));
        let Some(read) = is_dm.then(|| self.bridge.latest(channel.clone())).flatten() else {
            return;
        };
        let channel = channel.clone();
        cx.spawn(async move |this, cx| {
            let latest = cx.background_executor().spawn(read).await;
            this.update(cx, |app, cx| {
                app.sidebar
                    .update(cx, |sidebar, cx| sidebar.set_latest(channel, latest, cx));
            })
        })
        .detach();
    }

    pub(in crate::app) fn load_history(&mut self, channel: &ChannelId, cx: &mut Context<Self>) {
        let Some(read) = self.bridge.history(channel.clone(), None) else {
            return;
        };
        self.history_generation += 1;
        let generation = self.history_generation;
        let channel = channel.clone();
        cx.spawn(async move |this, cx| {
            let history = cx.background_executor().spawn(read).await;
            this.update(cx, |app, cx| {
                if app.history_generation == generation && app.channel == channel {
                    app.resolve_named_users(&history);
                    let newest = history
                        .last()
                        .map(|message| message.ts.clone())
                        .or_else(|| {
                            app.ws
                                .counts
                                .get(&channel)
                                .filter(|c| c.is_unread())
                                .and_then(|c| c.latest.clone())
                        });
                    let settings = app.preferences.read(cx).settings().clone();
                    let mark_read_mode = settings.mark_read;
                    app.messages.update(cx, |list, cx| {
                        list.set_history(channel.clone(), history, cx);
                        if mark_read_mode == 0 {
                            list.scroll_to_unread_once(cx);
                        }
                    });
                    let allow_mark = app.transcript_visible(cx)
                        && mark_read_mode != 2
                        && (!settings.no_bg_mark_read || app.window_active);
                    if allow_mark && let Some(newest) = newest {
                        app.mark_read(&channel, newest, cx);
                    }
                }
            })
        })
        .detach();
    }

    /// Live mode: the transcript reached the top of `channel`; read the next older page from
    /// the store, off the UI thread, and prepend it. A page a newer reload overtook is
    /// dropped (the reload's `set_history` forgets the request, so the top re-asks); an empty
    /// page marks the channel fully loaded.
    pub(in crate::app) fn load_older(
        &mut self,
        channel: &ChannelId,
        before: Ts,
        cx: &mut Context<Self>,
    ) {
        if self.ws.history_complete.contains(channel) {
            return;
        }
        let Some(read) = self.bridge.history(channel.clone(), Some(before)) else {
            return;
        };
        let generation = self.history_generation;
        let channel = channel.clone();
        cx.spawn(async move |this, cx| {
            let older = cx.background_executor().spawn(read).await;
            this.update(cx, |app, cx| {
                if app.history_generation != generation || app.channel != channel {
                    return;
                }
                if older.is_empty() {
                    app.ws.history_complete.insert(channel);
                    return;
                }
                app.resolve_named_users(&older);
                app.messages.update(cx, |list, cx| {
                    list.prepend_history(channel.clone(), older, cx)
                });
            })
        })
        .detach();
    }

    /// Live mode: page older stored history into `channel` until it reaches `since` (unix
    /// seconds), the beginning, or a page cap that bounds memory, for the date divider's
    /// "Jump to…"; then the list scrolls to its target.
    pub(in crate::app) fn load_since(
        &mut self,
        channel: &ChannelId,
        since: i64,
        cx: &mut Context<Self>,
    ) {
        /// Pages one jump may load; past it the list stops at the oldest page read.
        const JUMP_PAGES: usize = 20;
        let generation = self.history_generation;
        let channel = channel.clone();
        cx.spawn(async move |this, cx| {
            for _ in 0..JUMP_PAGES {
                let read = this.update(cx, |app, cx| {
                    if app.history_generation != generation
                        || app.channel != channel
                        || app.ws.history_complete.contains(&channel)
                    {
                        return None;
                    }
                    let before = app.messages.read(cx).oldest_loaded(cx)?;
                    app.bridge.history(channel.clone(), Some(before))
                });
                let Ok(Some(read)) = read else {
                    break;
                };
                let older = cx.background_executor().spawn(read).await;
                let reached = older
                    .first()
                    .and_then(|message| message.ts.secs())
                    .is_none_or(|secs| secs <= since);
                let landed = this.update(cx, |app, cx| {
                    if app.history_generation != generation || app.channel != channel {
                        return false;
                    }
                    if older.is_empty() {
                        app.ws.history_complete.insert(channel.clone());
                    } else {
                        app.messages.update(cx, |list, cx| {
                            list.prepend_history(channel.clone(), older, cx)
                        });
                    }
                    true
                });
                if reached || !matches!(landed, Ok(true)) {
                    break;
                }
            }
            let _ = this.update(cx, |app, cx| {
                app.messages.update(cx, |list, cx| list.finish_jump(cx))
            });
        })
        .detach();
    }

    /// Live mode: show the open thread's stored history (root + replies, oldest first), read
    /// off the UI thread. A read that a newer one overtook is dropped; one whose thread is no
    /// longer open is ignored by the panel.
    pub(in crate::app) fn load_thread(
        &mut self,
        channel: &ChannelId,
        thread_ts: &Ts,
        cx: &mut Context<Self>,
    ) {
        let Some(read) = self.bridge.thread(channel.clone(), thread_ts.clone()) else {
            return;
        };
        self.thread_generation += 1;
        let generation = self.thread_generation;
        let (channel, thread_ts) = (channel.clone(), thread_ts.clone());
        cx.spawn(async move |this, cx| {
            let messages = cx.background_executor().spawn(read).await;
            this.update(cx, |app, cx| {
                if app.thread_generation == generation {
                    app.resolve_named_users(&messages);
                    let newest = (messages.len() > 1)
                        .then(|| messages.last().map(|m| m.ts.clone()))
                        .flatten();
                    let root_has_unread =
                        messages
                            .first()
                            .zip(newest.as_ref())
                            .is_some_and(|(root, last)| {
                                root.last_read.as_ref().is_some_and(|lr| last > lr)
                            });
                    app.thread.update(cx, |panel, cx| {
                        panel.set_thread(channel.clone(), thread_ts.clone(), messages, cx)
                    });
                    if app.thread_open {
                        let settings = app.preferences.read(cx).settings().clone();
                        let allow_mark = settings.mark_read != 2
                            && (!settings.no_bg_mark_read || app.window_active);
                        if allow_mark && let Some(newest) = newest {
                            let should_mark = root_has_unread
                                || app.threads.read(cx).is_thread_unread(&channel, &thread_ts)
                                || app.sidebar.read(cx).has_thread_unreads();
                            if should_mark {
                                app.mark_thread_read(channel, thread_ts, newest, false, cx);
                            }
                        }
                    }
                }
            })
        })
        .detach();
    }

    /// Ask the core to look up `users` the snapshot lacks: members of other organizations,
    /// whom `users.list` leaves out, so they show by name instead of id.
    pub(in crate::app) fn resolve_users(&self, users: impl IntoIterator<Item = UserId>) {
        // Dedupe before sending: a page names the same few people many times. Which ids
        // really need `users.info` (the store holds no full record for them yet) the core
        // decides, so a boot-only record - a connect channel's external people, or guests -
        // still gets its full profile asked for.
        let named: Vec<UserId> = users
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if !named.is_empty() {
            self.bridge.send(Command::ResolveUsers(named));
        }
    }

    /// [`App::resolve_users`] for the people `messages` name: authors, repliers and
    /// `<@U…>` / `<@U…|name>` mentions.
    pub(in crate::app) fn resolve_named_users(&self, messages: &[Message]) {
        self.resolve_users(messages.iter().flat_map(|message| {
            let mentions = message.text.split("<@").skip(1).filter_map(|rest| {
                let end = rest.find(['>', '|'])?;
                Some(UserId::from(&rest[..end]))
            });
            message
                .user
                .iter()
                .chain(&message.reply_users)
                .cloned()
                .chain(mentions)
        }));
        let bots: Vec<slackless_core::types::BotId> = messages
            .iter()
            .filter(|m| m.bot_profile.is_none())
            .filter_map(|m| m.bot_id.clone())
            .filter(|id| self.fixtures.bot_profile(id).is_none())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if !bots.is_empty() {
            self.bridge.send(Command::ResolveBots(bots));
        }
    }
}
