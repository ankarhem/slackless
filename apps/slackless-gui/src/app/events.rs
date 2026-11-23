//! Live `CoreBridge` event folding, store reads, notifications, typing, and badges.

use super::*;

impl App {
    /// Rebuild the menu bar when its moving parts change: the workspaces (File > Workspace,
    /// Sign Out) and the History menu's places, labelled as Slack's.
    pub(super) fn update_menus(&mut self, cx: &mut Context<Self>) {
        /// Slack's History menu lists this many places.
        const PLACES: usize = 15;
        let bar = MenuBar {
            workspaces: (self.fixtures.workspaces.iter())
                .map(|team| team.name.clone().into())
                .collect(),
            recent: (self.recent_entries().into_iter().take(PLACES))
                .map(|(id, label, icon)| {
                    let label = match icon {
                        IconName::Hash => format!("#{label}").into(),
                        IconName::Lock => format!("#{label} (Private)").into(),
                        _ => label,
                    };
                    (label, id)
                })
                .collect(),
        };
        if self.menu_bar.as_ref() != Some(&bar) {
            menu_bar::set(&bar, cx);
            self.menu_bar = Some(bar);
        }
    }

    /// Put the shown workspace's Later items on its message views' saved markers.
    pub(super) fn sync_saved(&mut self, cx: &mut Context<Self>) {
        let saved: HashSet<Ts> = self
            .later
            .read(cx)
            .saved_in(self.workspace)
            .into_iter()
            .map(|(_, ts)| ts)
            .collect();
        self.messages
            .update(cx, |list, cx| list.replace_saved(saved.clone(), cx));
        self.thread
            .update(cx, |thread, cx| thread.replace_saved(saved, cx));
    }

    pub(super) fn sync_later_count(&mut self, cx: &mut Context<Self>) {
        let count = self.later.read(cx).in_progress_count();
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_later_count(count, cx));
    }

    /// A message view saved, unsaved or set a reminder on a message of the shown workspace.
    pub(super) fn save_for_later(&mut self, request: &SaveRequest, cx: &mut Context<Self>) {
        let label = self.label(&request.channel);
        let workspace = self.workspace;
        self.later.update(cx, |later, cx| {
            later.save(request.clone(), workspace, label, cx)
        });
        if request.saved {
            self.bridge.send(Command::SaveItem {
                channel: request.channel.clone(),
                ts: request.ts.clone(),
                date_due: request.due,
            });
        } else {
            self.bridge.send(Command::DeleteSavedItem {
                channel: request.channel.clone(),
                ts: request.ts.clone(),
            });
        }
    }

    /// Map live `saved.list` rows into [`LaterItem`]s using `fixtures` (falling back to the
    /// workspace's channel/user names when a message body was not returned by `messages.list`).
    pub(super) fn apply_saved_loaded(
        &mut self,
        workspace: usize,
        items: &[slackless_core::api::SavedItem],
        cx: &mut Context<Self>,
    ) {
        let fixtures = if workspace == self.workspace {
            self.fixtures.clone()
        } else if let Some(parked) = self.parked.get(workspace).and_then(Option::as_ref) {
            parked.fixtures.clone()
        } else {
            return;
        };
        let mapped: Vec<crate::later::LaterItem> = items
            .iter()
            .map(|item| {
                let label = fixtures
                    .conversation(&item.channel)
                    .map(|c| fixtures.conversation_label(c))
                    .unwrap_or_else(|| item.channel.to_string());
                let (author, text) = match &item.message {
                    Some(msg) => {
                        let author = msg
                            .user
                            .as_ref()
                            .map(|u| fixtures.user_name(u).to_owned())
                            .or_else(|| msg.username.clone())
                            .or_else(|| msg.bot_profile.as_ref().map(|b| b.name.clone()))
                            .unwrap_or_else(|| "Message".to_owned());
                        (author, fixtures.preview(msg))
                    }
                    None => ("Saved message".to_owned(), String::new()),
                };
                let state = match item.state {
                    slackless_core::api::SavedItemState::InProgress => {
                        crate::later::LaterState::InProgress
                    }
                    slackless_core::api::SavedItemState::Archived => {
                        crate::later::LaterState::Archived
                    }
                    slackless_core::api::SavedItemState::Completed => {
                        crate::later::LaterState::Completed
                    }
                };
                crate::later::LaterItem {
                    workspace,
                    channel: item.channel.clone(),
                    ts: item.ts.clone(),
                    label,
                    author,
                    text,
                    state,
                    due: item.date_due,
                    fired: false,
                }
            })
            .collect();
        self.later
            .update(cx, |later, cx| later.replace_live(workspace, mapped, cx));
        if workspace == self.workspace {
            self.sync_saved(cx);
        }
    }

    /// Post every Later reminder that came due: system notification plus in-app toast.
    pub(super) fn fire_reminders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let due = self
            .later
            .update(cx, |later, cx| later.take_due(now_secs(), cx));
        for item in due {
            let title = format!("Reminder: {} in {}", item.author, item.label);
            let tag = format!("later:{}:{}", item.channel, item.ts);
            self.post_notification(&tag, &title, &item.text, window, cx);
        }
    }

    /// A system notification (a macOS banner) with the notification sound, as Slack's desktop
    /// notifications come.
    pub(super) fn system_notification(
        &mut self,
        tag: &str,
        title: &str,
        body: &str,
        cx: &mut Context<Self>,
    ) {
        self.system_notification_kind(tag, title, body, false, cx);
    }

    pub(super) fn system_notification_kind(
        &mut self,
        tag: &str,
        title: &str,
        body: &str,
        is_direct_or_mention: bool,
        cx: &mut Context<Self>,
    ) {
        cx.show_system_notification(SystemNotification {
            tag: tag.to_owned().into(),
            title: title.to_owned().into(),
            body: body.to_owned().into(),
            actions: Vec::new(),
        });
        self.os.notify(title, body);
        let bounce = self.preferences.read(cx).settings().bounce_dock;
        if bounce && is_direct_or_mention && !self.window_active {
            self.os.request_attention(false);
        }
        self.play_notification_sound_kind(is_direct_or_mention, cx);
    }

    /// A notification mirrored as an in-app toast (Later reminders).
    pub(super) fn post_notification(
        &mut self,
        tag: &str,
        title: &str,
        body: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.system_notification(tag, title, body, cx);
        notify(title, body.to_owned(), window, cx);
    }

    /// Remember the window's frame once it has settled after a move or resize, so the next
    /// launch reopens it there, like Slack (live mode; a no-op offline or in full screen).
    pub(super) fn save_window_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        const SETTLE: Duration = Duration::from_millis(500);
        self.window_state_save = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let _ = this.update_in(cx, |app, window, cx| {
                let write = window
                    .native_window_state()
                    .and_then(|state| app.bridge.remember_window_state(state));
                if let Some(write) = write {
                    cx.background_executor().spawn(write).detach();
                }
            });
        });
    }

    /// Slack's window title (Window menu, Mission Control):
    /// "<conversation> (Channel|DM) - <workspace> - N new items - <app>".
    pub(super) fn sync_window_title(
        &mut self,
        conversation: Option<&Conversation>,
        window: &mut Window,
    ) {
        let mut parts = Vec::with_capacity(4);
        if let Some(conversation) = conversation {
            let kind = match conversation.kind() {
                ConversationKind::Im | ConversationKind::Mpim => "DM",
                ConversationKind::PublicChannel | ConversationKind::PrivateChannel => "Channel",
            };
            let label = self.fixtures.conversation_label(conversation);
            let name = label.strip_prefix("# ").unwrap_or(&label);
            parts.push(format!("{name} ({kind})"));
        }
        if let Some(team) = self
            .fixtures
            .workspaces
            .get(self.workspace)
            .filter(|team| !team.name.is_empty())
        {
            parts.push(team.name.clone());
        }
        match self.badge {
            Some(1) => parts.push("1 new item".to_owned()),
            Some(n) if n > 1 => parts.push(format!("{n} new items")),
            _ => {}
        }
        parts.push("slackless".to_owned());
        let title = parts.join(" - ");
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }
    }

    /// Hand the total unread count (across every workspace) to the dock badge, and push
    /// per-workspace badges to the workspace switcher rail (`F-CORE-006`).
    pub(super) fn sync_badge(&mut self, cx: &mut Context<Self>) {
        let ws_count = self
            .live_teams
            .len()
            .max(self.fixtures.workspaces.len())
            .max(1);
        let mut badges = Vec::with_capacity(ws_count);
        let mut total_dock: u32 = 0;
        for ix in 0..ws_count {
            let badge = if ix == self.workspace {
                let sidebar = self.sidebar.read(cx);
                let mentions = sidebar.badge_count();
                let has_unreads = sidebar.has_any_unreads();
                WorkspaceBadge {
                    mentions,
                    has_unreads,
                }
            } else if let Some(parked) = self.parked.get(ix).and_then(Option::as_ref) {
                if !parked.state.counts.is_empty() {
                    let activity = self.bg_activity_unread.get(ix).copied().unwrap_or(0);
                    workspace_badge_from_counts(&parked.state.counts, activity)
                } else {
                    let sidebar = parked.views.sidebar.read(cx);
                    WorkspaceBadge {
                        mentions: sidebar.badge_count(),
                        has_unreads: sidebar.has_any_unreads(),
                    }
                }
            } else if let Some(counts) = self.bg_counts.get(ix).filter(|c| !c.is_empty()) {
                let activity = self.bg_activity_unread.get(ix).copied().unwrap_or(0);
                workspace_badge_from_counts(counts, activity)
            } else {
                WorkspaceBadge::default()
            };
            total_dock = total_dock.saturating_add(badge.mentions);
            badges.push(badge);
        }
        let mut any_unreads = false;
        let mut status_workspaces = Vec::with_capacity(ws_count);
        for (ix, badge) in badges.iter().enumerate() {
            any_unreads |= badge.has_unreads;
            let name = self
                .fixtures
                .workspaces
                .get(ix)
                .map(|t| t.name.clone())
                .unwrap_or_else(|| format!("Workspace {}", ix + 1));
            status_workspaces.push((name, badge.mentions, badge.has_unreads));
        }
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_workspace_badges(badges, cx));
        let settings = self.preferences.read(cx).settings().clone();
        self.badge = Some(if settings.show_dock_badge {
            total_dock
        } else {
            0
        });
        let next_label = if !settings.show_dock_badge {
            None
        } else if total_dock > 0 {
            Some(total_dock.to_string())
        } else if settings.badge_on_unreads && any_unreads {
            Some("\u{2022}".to_owned())
        } else {
            None
        };
        if self.badge_label.as_ref() != Some(&next_label) {
            self.badge_label = Some(next_label.clone());
            self.os.set_badge(next_label.as_deref());
        }
        let status_bar_state = StatusBarState {
            visible: settings.show_menu_bar_icon,
            unread_mentions: total_dock,
            has_unreads: any_unreads,
            dnd_active: self.self_state.paused(),
            workspaces: status_workspaces,
        };
        if self.last_status_bar.as_ref() != Some(&status_bar_state) {
            self.last_status_bar = Some(status_bar_state.clone());
            self.os
                .update_status_bar(Some(include_bytes!("../../icon.png")), &status_bar_state);
        }
    }

    /// Route an event from workspace `ws_index`: active workspace events update the live UI,
    /// while background workspace events keep their counts, activity badges, and team icons
    /// fresh on the workspace switcher rail.
    pub(super) fn on_workspace_event(
        &mut self,
        ws_index: usize,
        event: &Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ws_index == self.workspace {
            self.on_core_event(event, window, cx);
            return;
        }
        match event {
            Event::Counts(counts) => {
                let map: HashMap<ChannelId, ConversationCounts> =
                    [&counts.channels, &counts.ims, &counts.mpims]
                        .into_iter()
                        .flatten()
                        .map(|entry| (entry.id.clone(), entry.clone()))
                        .collect();
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked.state.counts = map.clone();
                }
                if let Some(slot) = self.bg_counts.get_mut(ws_index) {
                    *slot = map;
                }
                cx.notify();
            }
            Event::ActivityView { feed } => {
                if let Ok(feed) = feed.as_deref() {
                    let unread = feed.items.iter().filter(|item| item.unread).count() as u32;
                    if let Some(slot) = self.bg_activity_unread.get_mut(ws_index) {
                        *slot = unread;
                    }
                    cx.notify();
                }
            }
            Event::SavedLoaded(Ok(items)) => {
                self.apply_saved_loaded(ws_index, items, cx);
            }
            Event::Conversations(_) | Event::SidebarChanged => {
                // Refresh this background workspace's team name/icon in `live_teams` if hydrate
                // just stored a richer `team.info` record.
                let fresh_team = self
                    .parked_bridges
                    .get(ws_index)
                    .and_then(Option::as_ref)
                    .and_then(|b| b.snapshot(&HashMap::new()))
                    .and_then(|snap| snap.workspaces.into_iter().next());
                if let (Some(team), Some(slot)) = (fresh_team, self.live_teams.get_mut(ws_index)) {
                    if *slot != team {
                        *slot = team;
                        self.reload_live(window, cx);
                    }
                }
            }
            Event::Usergroups(groups) => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked.state.usergroups = groups.clone();
                }
            }
            Event::AppCommands(commands) => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked.state.app_commands = commands.clone();
                }
            }
            Event::Connection(state) => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked.state.connection = Some(state.clone());
                }
            }
            Event::OfflineQueueChanged(count) => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked.state.offline_queued = *count;
                }
            }
            Event::OfflineMutationFailed(failed) => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    if !parked
                        .state
                        .failed_mutations
                        .iter()
                        .any(|m| m.id == failed.id)
                    {
                        parked.state.failed_mutations.push(failed.clone());
                    }
                }
            }
            Event::Presence { users, presence } => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked
                        .state
                        .presence
                        .write()
                        .unwrap_or_else(PoisonError::into_inner)
                        .extend(users.iter().map(|u| (u.clone(), presence.clone())));
                }
            }
            Event::MessagePosted { channel, message } => {
                let settings = self.preferences.read(cx).settings().clone();
                if !settings.desktop_notifications
                    || self.self_state.paused()
                    || !settings.notifications_allowed_now()
                {
                    return;
                }
                let snap = self
                    .parked_bridges
                    .get(ws_index)
                    .and_then(Option::as_ref)
                    .and_then(|b| b.snapshot(&HashMap::new()));
                let Some(snap) = snap else {
                    return;
                };
                if message.user.as_ref() == Some(&snap.self_user) {
                    return;
                }
                let level = snap.notify_levels.get(channel).copied();
                if level == Some(NotifyLevel::Muted) {
                    return;
                }
                let Some(conv) = snap.conversation(channel) else {
                    return;
                };
                let dm = matches!(conv.kind(), ConversationKind::Im | ConversationKind::Mpim);
                let mentioned = mentions_me(&message.text, &snap.self_user, &settings.keywords);
                let wanted = match (level, settings.notify_level) {
                    (Some(NotifyLevel::Mentions), _) | (_, 1) => dm || mentioned,
                    (_, 0) => true,
                    _ => false,
                };
                if !wanted {
                    return;
                }
                let ws_name = self
                    .live_teams
                    .get(ws_index)
                    .map(|t| t.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "Slack".to_owned());
                let sender = message
                    .user
                    .as_ref()
                    .map(|u| snap.user_name(u).to_owned())
                    .unwrap_or_default();
                let text = if settings.show_preview {
                    snap.plain_text(message)
                } else {
                    "New message".to_owned()
                };
                let (title, body) = match conv.kind() {
                    ConversationKind::Im => (format!("{ws_name} — {sender}"), text),
                    ConversationKind::Mpim => (
                        format!("{ws_name} — {}", snap.conversation_label(conv)),
                        format!("{sender}: {text}"),
                    ),
                    _ => (
                        format!("{ws_name} — #{}", conv.name.as_deref().unwrap_or_default()),
                        format!("{sender}: {text}"),
                    ),
                };
                let tag = format!("bg:{ws_index}:{channel}:{}", message.ts);
                self.system_notification_kind(&tag, &title, &body, dm || mentioned, cx);
            }
            Event::HuddleUpdated {
                channel,
                participants,
                ended,
            } => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    if *ended {
                        parked.state.active_huddles.remove(channel);
                    } else {
                        parked
                            .state
                            .active_huddles
                            .insert(channel.clone(), participants.clone());
                    }
                    let keys: HashSet<ChannelId> =
                        parked.state.active_huddles.keys().cloned().collect();
                    parked
                        .views
                        .sidebar
                        .update(cx, |sidebar, cx| sidebar.set_huddles(keys, cx));
                }
            }
            _ => {}
        }
    }

    /// Live mode: fold a core event into the views.
    pub(super) fn on_core_event(
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
                self.ws.counts = [&counts.channels, &counts.ims, &counts.mpims]
                    .into_iter()
                    .flatten()
                    .map(|entry| (entry.id.clone(), entry.clone()))
                    .collect();
                self.reload_live(window, cx);
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
                    if let Some((thread_channel, thread_ts)) = open {
                        if thread_channel == *channel {
                            self.load_thread(&thread_channel, &thread_ts, cx);
                        }
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
            Event::ThreadsView(view) => {
                self.threads
                    .update(cx, |threads, cx| threads.set_remote((**view).clone(), cx));
                cx.notify();
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
            Event::HuddleUpdated {
                channel,
                participants,
                ended,
            } => {
                if *ended {
                    self.ws.active_huddles.remove(channel);
                } else {
                    self.ws
                        .active_huddles
                        .insert(channel.clone(), participants.clone());
                }
                let keys: HashSet<ChannelId> = self.ws.active_huddles.keys().cloned().collect();
                self.sidebar
                    .update(cx, |sidebar, cx| sidebar.set_huddles(keys, cx));
                cx.notify();
            }
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
            Event::ListLoaded { file_id, list } => {
                let value = match list {
                    Ok(data) => Ok((**data).clone()),
                    Err(err) => Err(err.clone()),
                };
                self.ws.lists.insert(file_id.clone(), value);
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

    /// Live mode: rebuild the workspace views from a fresh store snapshot after its
    /// conversations or people changed, staying on the open conversation while it exists.
    pub(super) fn reload_live(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut snapshot) = self.bridge.snapshot(&self.ws.counts) else {
            return;
        };
        // Keep the full multi-workspace rail intact while updating this workspace's team info.
        if let Some(own) = snapshot.workspaces.first().cloned() {
            if let Some(slot) = self.live_teams.get_mut(self.workspace) {
                *slot = own;
            }
        }
        if !self.live_teams.is_empty() {
            snapshot.workspaces = self.live_teams.clone();
        }
        // Presence, usergroups, and app_commands are transient (never stored), so they ride along.
        snapshot.presence = self.ws.presence.clone();
        snapshot.usergroups = self.ws.usergroups.clone();
        snapshot.app_commands = self.ws.app_commands.clone();
        let fixtures = Arc::new(snapshot);
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
        let keep = self.ws.chosen && fixtures.conversation(&self.channel).is_some();
        self.fixtures = fixtures;
        // Hydrate custom status from Slack's live self profile (preserving active focus mode).
        if !self.self_state.focused() {
            if let Some(me) = self.fixtures.user(&self.fixtures.self_user) {
                let remote = crate::status::UserStatus::from_profile(&me.profile);
                if self.self_state.status != remote {
                    self.self_state.set_status(remote);
                }
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
    pub(super) fn refresh_dm_latest(&mut self, channel: &ChannelId, cx: &mut Context<Self>) {
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

    pub(super) fn load_history(&mut self, channel: &ChannelId, cx: &mut Context<Self>) {
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
                    let newest = history.last().map(|message| message.ts.clone());
                    let settings = app.preferences.read(cx).settings().clone();
                    let mark_read_mode = settings.mark_read;
                    app.messages.update(cx, |list, cx| {
                        list.set_history(channel.clone(), history, cx);
                        if mark_read_mode == 0 {
                            list.scroll_to_unread_once(cx);
                        }
                    });
                    let allow_mark =
                        mark_read_mode != 2 && (!settings.no_bg_mark_read || app.window_active);
                    if allow_mark {
                        if let Some(newest) = newest {
                            app.mark_read(&channel, newest, cx);
                        }
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
    pub(super) fn load_older(&mut self, channel: &ChannelId, before: Ts, cx: &mut Context<Self>) {
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
    pub(super) fn load_since(&mut self, channel: &ChannelId, since: i64, cx: &mut Context<Self>) {
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
    pub(super) fn load_thread(
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
                    app.thread.update(cx, |panel, cx| {
                        panel.set_thread(channel, thread_ts, messages, cx)
                    });
                }
            })
        })
        .detach();
    }

    /// Ask the core to look up `users` the snapshot lacks: members of other organizations,
    /// whom `users.list` leaves out, so they show by name instead of id.
    pub(super) fn resolve_users(&self, users: impl IntoIterator<Item = UserId>) {
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
    pub(super) fn resolve_named_users(&self, messages: &[Message]) {
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
    }

    /// Someone started or kept typing: show them until [`TYPING_TTL`] after their last
    /// announcement, as Slack does. The user's own typing (from another client) is not shown.
    pub(super) fn someone_typing(
        &mut self,
        channel: &ChannelId,
        user: &UserId,
        thread_ts: Option<&Ts>,
        cx: &mut Context<Self>,
    ) {
        if *user == self.fixtures.self_user {
            return;
        }
        let typists = self
            .typing
            .entry((channel.clone(), thread_ts.cloned()))
            .or_default();
        typists.retain(|(typist, _)| typist != user);
        typists.push((user.clone(), Instant::now() + TYPING_TTL));
        self.expire_typing(cx);
    }

    /// Drop typists whose announcement ran out, show who is left, and wake again when the
    /// next announcement runs out.
    pub(super) fn expire_typing(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        for typists in self.typing.values_mut() {
            typists.retain(|(_, until)| *until > now);
        }
        self.typing.retain(|_, typists| !typists.is_empty());
        self.show_typing(cx);
        let next = self
            .typing
            .values()
            .flatten()
            .map(|(_, until)| *until)
            .min();
        self.typing_expiry = match next {
            Some(next) => cx.spawn(async move |this, cx| {
                let wait = next.saturating_duration_since(Instant::now());
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |app, cx| app.expire_typing(cx));
            }),
            None => gpui::Task::ready(()),
        };
    }

    /// Put who is typing under the open conversation's box and the open thread's reply box.
    pub(super) fn show_typing(&self, cx: &mut Context<Self>) {
        let names = |key: (ChannelId, Option<Ts>)| {
            let names: Vec<SharedString> = (self.typing.get(&key).into_iter().flatten())
                .map(|(user, _)| self.fixtures.user_name(user).to_owned().into())
                .collect();
            TypingNames::of(&names)
        };
        let here = names((self.channel.clone(), None));
        self.composer
            .update(cx, |composer, cx| composer.set_typing(here, cx));
        let open = self.thread.read(cx).open_thread();
        let in_thread = open.and_then(|(channel, ts)| names((channel, Some(ts))));
        self.thread
            .update(cx, |thread, cx| thread.set_typing(in_thread, cx));
    }

    /// A message was just posted: its author stops typing there, and Slack's desktop
    /// notification rules decide whether it gets a banner and the notification sound.
    pub(super) fn message_posted(
        &mut self,
        channel: &ChannelId,
        message: &Message,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(author) = &message.user {
            let thread = message.thread_ts.clone().filter(|ts| *ts != message.ts);
            if let Some(typists) = self.typing.get_mut(&(channel.clone(), thread)) {
                typists.retain(|(typist, _)| typist != author);
            }
            self.expire_typing(cx);
        }
        if !self.should_notify(channel, message, window, cx) {
            return;
        }
        let Some(conversation) = self.fixtures.conversation(channel) else {
            return;
        };
        let sender = match &message.user {
            Some(user) => self.fixtures.user_name(user).to_owned(),
            None => (message.bot_profile.as_ref())
                .map(|bot| bot.name.clone())
                .unwrap_or_default(),
        };
        let text = if self.preferences.read(cx).settings().show_preview {
            self.fixtures.plain_text(message)
        } else {
            "New message".to_owned()
        };
        let (title, body) = match conversation.kind() {
            ConversationKind::Im => (sender, text),
            ConversationKind::Mpim => (
                self.fixtures.conversation_label(conversation),
                format!("{sender}: {text}"),
            ),
            _ => (
                format!("#{}", conversation.name.clone().unwrap_or_default()),
                format!("{sender}: {text}"),
            ),
        };
        let is_dm_or_mention = matches!(
            conversation.kind(),
            ConversationKind::Im | ConversationKind::Mpim
        ) || mentions_me(
            &message.text,
            &self.fixtures.self_user,
            &self.preferences.read(cx).settings().keywords,
        );
        let tag = format!("msg:{channel}:{}", message.ts);
        self.system_notification_kind(&tag, &title, &body, is_dm_or_mention, cx);
    }

    /// Slack's desktop notification rules for a message just posted: never your own, while
    /// notifications are paused or outside the notification schedule, or for the
    /// conversation (or thread) on screen in the focused window; never in a muted
    /// conversation; otherwise by the conversation's level (its bell menu choice, else
    /// Preferences > Notifications): every message, or direct messages, mentions and
    /// keywords. A thread reply that is neither a DM nor mentions you notifies only under
    /// "replies to threads I'm following", for a thread you started.
    pub(super) fn should_notify(
        &self,
        channel: &ChannelId,
        message: &Message,
        window: &Window,
        cx: &gpui::App,
    ) -> bool {
        let me = &self.fixtures.self_user;
        let settings = self.preferences.read(cx).settings();
        if !settings.desktop_notifications
            || message.user.as_ref() == Some(me)
            || self.self_state.paused()
            || !settings.notifications_allowed_now()
        {
            return false;
        }
        let reply = message
            .thread_ts
            .as_ref()
            .is_some_and(|ts| *ts != message.ts);
        let on_screen = if reply {
            self.thread_open
                && self
                    .thread
                    .read(cx)
                    .open_thread()
                    .is_some_and(|(open, ts)| {
                        open == *channel && message.thread_ts.as_ref() == Some(&ts)
                    })
        } else {
            self.channel == *channel
        };
        if on_screen && window.is_window_active() {
            return false;
        }
        let level = self.fixtures.notify_levels.get(channel).copied();
        if level == Some(NotifyLevel::Muted) {
            return false;
        }
        let dm = self
            .fixtures
            .conversation(channel)
            .is_some_and(|conversation| {
                matches!(
                    conversation.kind(),
                    ConversationKind::Im | ConversationKind::Mpim
                )
            });
        let mentioned = mentions_me(&message.text, me, &settings.keywords);
        let wanted = match (level, settings.notify_level) {
            (Some(NotifyLevel::Mentions), _) | (_, 1) => dm || mentioned,
            (_, 0) => true,
            _ => false,
        };
        if reply && !dm && !mentioned {
            return wanted
                && settings.thread_replies
                && message.parent_user_id.as_ref() == Some(me);
        }
        wanted
    }

    /// Play the notification sound chosen in Preferences, unless muted or in quiet hours / DND (`F-SHELL-180`).
    pub(super) fn play_notification_sound_kind(
        &mut self,
        is_direct_or_mention: bool,
        cx: &mut Context<Self>,
    ) {
        let settings = self.preferences.read(cx).settings();
        if settings.mute_sounds
            || self.self_state.paused()
            || self.self_state.focused()
            || !settings.notifications_allowed_now()
        {
            return;
        }
        let choice = if is_direct_or_mention {
            settings.dm_sound
        } else {
            settings.sound
        };
        self.play_sound(choice, cx);
    }

    /// Play Preferences' sound `choice` ("None" plays nothing): fetched from Slack's CDN
    /// through the core the first time, then kept for the next notifications.
    pub(super) fn play_sound(&mut self, choice: usize, cx: &mut Context<Self>) {
        let Some(url) = crate::preferences::SOUND_FILES
            .get(choice)
            .copied()
            .flatten()
        else {
            return;
        };
        if let Some((cached, data)) = &self.sound {
            if *cached == choice {
                self.os.play_sound(data);
                return;
            }
        }
        let Some(fetch) = crate::media::fetch(url.to_owned(), cx) else {
            return;
        };
        self.sound_fetch = cx.spawn(async move |this, cx| match fetch.await {
            Ok(data) => {
                let _ = this.update(cx, |app, _| {
                    app.os.play_sound(&data);
                    app.sound = Some((choice, data));
                });
            }
            Err(e) => eprintln!("slackless: notification sound not loaded ({e})"),
        });
    }
}
