//! Navigation history, workspace switching, search menu, and top-level views.

mod huddle;
mod links;
mod tabs;

use super::*;

impl App {
    pub(super) fn open_later(&mut self, cx: &mut Context<Self>) {
        self.record_nav(&NavPlace::Saved, cx);
        self.show_later(cx);
    }

    pub(super) fn show_later(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.close_full_column_views(cx);
        self.later_open = true;
        self.sidebar.update(cx, |s, cx| {
            s.set_home_page(Some(crate::sidebar::HomePage::Saved), cx)
        });
        self.bridge.send(Command::LoadSaved);
        cx.notify();
    }

    pub(super) fn open_thread(
        &mut self,
        channel: ChannelId,
        thread_ts: Ts,
        cx: &mut Context<Self>,
    ) {
        let seed_root = self
            .messages
            .read(cx)
            .message_by_ts(&channel, &thread_ts, cx)
            .or_else(|| self.unreads.read(cx).message_by_ts(&channel, &thread_ts));
        self.thread.update(cx, |panel, cx| {
            panel.open(channel.clone(), thread_ts.clone(), seed_root, cx)
        });
        self.sync_thread_bar(cx);
        // Live mode: show the stored thread now, then fetch its full history from Slack
        // (fixture mode: both are no-ops and the panel keeps its fixture content).
        self.load_thread(&channel, &thread_ts, cx);
        self.messages.update(cx, |list, cx| {
            list.set_open_thread(Some(thread_ts.clone()), cx)
        });
        self.unreads.update(cx, |unreads, cx| {
            unreads.set_open_thread(Some((channel.clone(), thread_ts.clone())), cx)
        });
        self.bridge.send(Command::OpenThread { channel, thread_ts });
        self.thread_open = true;
        self.profile_open = false;
        self.pins_open = false;
        if self.tab == ConversationTab::Pins {
            self.tab = ConversationTab::Messages;
        }
        self.show_typing(cx);
        cx.notify();
    }

    /// Hide the thread panel and drop the open-thread tint from the transcript.
    pub(super) fn close_thread(&mut self, cx: &mut Context<Self>) {
        self.thread_open = false;
        self.messages
            .update(cx, |list, cx| list.set_open_thread(None, cx));
        self.unreads
            .update(cx, |unreads, cx| unreads.set_open_thread(None, cx));
    }

    pub(super) fn toggle_pins(&mut self, cx: &mut Context<Self>) {
        self.pins_open = !self.pins_open;
        if self.pins_open {
            self.close_thread(cx);
            self.profile_open = false;
            self.load_tab_data(&ConversationTab::Pins);
        } else if self.tab == ConversationTab::Pins {
            self.tab = ConversationTab::Messages;
        }
        cx.notify();
    }

    /// Toggle the leftmost workspace switcher rail (`Cmd+Shift+S` / View menu) and persist.
    pub(super) fn toggle_workspace_rail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences.update(cx, |prefs, cx| {
            let mut next = prefs.settings().clone();
            next.show_workspace_rail = !next.show_workspace_rail;
            prefs.load_settings(next, window, cx);
        });
        self.apply_preferences(window, cx);
        cx.notify();
    }

    /// Hide the open right sidebar (thread, profile, or pins) or restore the last hidden one
    /// (`Cmd+.` / View > Toggle Right Sidebar).
    pub(super) fn toggle_right_pane(&mut self, cx: &mut Context<Self>) {
        if self.thread_open {
            self.last_right_pane = Some(RightPane::Thread);
            self.thread_open = false;
        } else if self.profile_open {
            self.last_right_pane = Some(RightPane::Profile);
            self.profile_open = false;
        } else if self.pins_open {
            self.last_right_pane = Some(RightPane::Pins);
            self.pins_open = false;
        } else {
            match self.last_right_pane {
                Some(RightPane::Thread) if self.thread.read(cx).open_thread().is_some() => {
                    self.thread_open = true;
                }
                Some(RightPane::Profile) => {
                    self.profile_open = true;
                }
                Some(RightPane::Pins) => {
                    self.pins_open = true;
                }
                _ => {
                    self.pins_open = true;
                    if !self.fixtures.sample && !self.ws.pins.contains_key(&self.channel) {
                        self.bridge.send(Command::LoadPins(self.channel.clone()));
                    }
                }
            }
        }
        cx.notify();
    }

    /// Jump to the oldest unread message in the current conversation, or step to the next
    /// unread conversation if there are none here (`Cmd+J`).
    pub(super) fn jump_to_unread(&mut self, cx: &mut Context<Self>) {
        let jumped = self.messages.update(cx, |list, cx| list.jump_to_unread(cx));
        if !jumped {
            self.step_conversation(true, true, cx);
        }
    }

    /// Return to the Home navigation tab and current conversation (`Cmd+Shift+H`).
    pub(super) fn open_home(&mut self, cx: &mut Context<Self>) {
        self.activity_open = false;
        self.close_full_column_views(cx);
        self.sidebar.update(cx, |sidebar, cx| sidebar.show_home(cx));
        cx.notify();
    }

    /// Reorder workspace tile `from` to `to` in the switcher rail and persist (`F-CORE-003`).
    pub(super) fn reorder_workspace(
        &mut self,
        from: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let len = self.live_teams.len();
        if from == to || from >= len || to >= len {
            return;
        }
        self.live_teams.swap(from, to);
        if from < self.parked.len() && to < self.parked.len() {
            self.parked.swap(from, to);
        }
        if from < self.parked_bridges.len() && to < self.parked_bridges.len() {
            self.parked_bridges.swap(from, to);
        }
        if from < self.bg_counts.len() && to < self.bg_counts.len() {
            self.bg_counts.swap(from, to);
        }
        if from < self.bg_activity_unread.len() && to < self.bg_activity_unread.len() {
            self.bg_activity_unread.swap(from, to);
        }
        if self.workspace == from {
            self.workspace = to;
        } else if self.workspace == to {
            self.workspace = from;
        }
        self.sidebar
            .update(cx, |s, cx| s.set_workspace(self.workspace, cx));
        let order: Vec<String> = self
            .live_teams
            .iter()
            .map(|t| t.id.as_str().to_owned())
            .collect();
        if let Some(write) = self.bridge.remember_workspace_order(&order) {
            cx.background_executor().spawn(write).detach();
        }
        self.reload_live(window, cx);
        self.update_menus(cx);
    }

    /// The hover card's "Message": open the DM with `user` — the existing IM, else (live)
    /// have Slack open (or create) it, like the New message view's people picks.
    pub(super) fn message_user(
        &mut self,
        user: &UserId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_hover_card(cx);
        if let Some(dm) = self
            .fixtures
            .dms
            .iter()
            .find(|dm| dm.user.as_ref() == Some(user))
            .map(|dm| dm.id.clone())
        {
            self.select(dm, cx);
        } else if self.bridge.store().is_some() {
            self.bridge.send(Command::OpenDm {
                users: vec![user.clone()],
            });
        } else {
            notify(
                "Needs a live workspace",
                "Sign in to start a new DM with this person.",
                window,
                cx,
            );
        }
    }

    /// The currently visible place for Back/Forward navigation history.
    pub(super) fn current_place(&self, cx: &gpui::App) -> NavPlace {
        if self.threads_open {
            NavPlace::Threads
        } else if self.unreads_open {
            NavPlace::Unreads
        } else if self.huddles_open {
            NavPlace::Huddles
        } else if self.drafts_open {
            NavPlace::Drafts
        } else if self.later_open {
            NavPlace::Saved
        } else if self.directory_open {
            NavPlace::Directories
        } else if self.tools_open {
            NavPlace::Tools
        } else if self.activity_open {
            NavPlace::Activity
        } else if self.sidebar.read(cx).active_nav() == NavItem::Files {
            NavPlace::Files
        } else {
            NavPlace::Channel(self.channel.clone())
        }
    }

    /// Record the current place before moving to `dest`.
    pub(super) fn record_nav(&mut self, dest: &NavPlace, cx: &gpui::App) {
        let cur = self.current_place(cx);
        if cur != *dest {
            self.back.push(cur);
            self.forward.clear();
        }
    }

    pub(super) fn select(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        self.record_nav(&NavPlace::Channel(channel.clone()), cx);
        self.navigate(channel, cx);
    }

    /// Whether the main column is currently showing `self.channel`'s transcript (as opposed to
    /// a full-column overlay such as Unreads, Threads, Activity, Search, Drafts, Later, or Files).
    pub(super) fn transcript_visible(&self, cx: &gpui::App) -> bool {
        !self.searching
            && !self.drafts_open
            && !self.later_open
            && !self.threads_open
            && !self.unreads_open
            && !self.huddles_open
            && !self.directory_open
            && !self.tools_open
            && !self.compose_open
            && !self.activity_open
            && self.sidebar.read(cx).active_nav() != crate::sidebar::NavItem::Files
    }

    /// Show `channel` as the user's choice, leaving any full-column view (search, drafts,
    /// later). History is the caller's: [`App::select`] records it, back/forward replay it.
    pub(super) fn navigate(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        let was_covered = !self.transcript_visible(cx);
        self.ws.chosen = true;
        self.activity_open = false;
        self.close_full_column_views(cx);
        self.close_hover_card(cx);
        cx.notify();
        if channel == self.channel && was_covered {
            self.maybe_mark_open_read(cx);
        }
        self.show_conversation(channel, cx);
    }

    fn go_to_place(&mut self, place: NavPlace, window: &mut Window, cx: &mut Context<Self>) {
        match place {
            NavPlace::Channel(ch) => self.navigate(ch, cx),
            NavPlace::Threads => self.show_threads(cx),
            NavPlace::Unreads => self.show_unreads(window, cx),
            NavPlace::Huddles => self.show_huddles(cx),
            NavPlace::Drafts => self.show_drafts(cx),
            NavPlace::Saved => self.show_later(cx),
            NavPlace::Directories => self.show_directory(cx),
            NavPlace::Activity => self.show_activity(cx),
            NavPlace::Files => self.show_files(cx),
            NavPlace::Tools => self.show_tools(cx),
        }
    }

    /// Slack's back arrow (Cmd+[, mouse Back): the place before this one.
    pub(super) fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(previous) = self.back.pop() {
            let cur = self.current_place(cx);
            self.forward.push(cur);
            self.go_to_place(previous, window, cx);
        }
    }

    /// Slack's forward arrow (Cmd+], mouse Forward): undo the last back.
    pub(super) fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(next) = self.forward.pop() {
            let cur = self.current_place(cx);
            self.back.push(cur);
            self.go_to_place(next, window, cx);
        }
    }

    /// The history clock's "Recent" list: the open conversation, then the recently left
    /// ones still in the snapshot, each with Slack's kind icon.
    pub(super) fn recent_entries(&self) -> Vec<(ChannelId, SharedString, IconName)> {
        /// Slack lists about this many before its "Show more".
        const SHOWN: usize = 25;
        std::iter::once(&self.channel)
            .chain(&self.recent)
            .filter_map(|id| {
                let conversation = self.fixtures.conversation(id)?;
                let (label, icon) = match conversation.kind() {
                    ConversationKind::PublicChannel => (
                        conversation.name.clone().unwrap_or_else(|| id.to_string()),
                        IconName::Hash,
                    ),
                    ConversationKind::PrivateChannel => (
                        conversation.name.clone().unwrap_or_else(|| id.to_string()),
                        IconName::Lock,
                    ),
                    ConversationKind::Im => (
                        self.fixtures.conversation_label(conversation),
                        IconName::User,
                    ),
                    ConversationKind::Mpim => (
                        self.fixtures.conversation_label(conversation),
                        IconName::Users,
                    ),
                };
                Some((id.clone(), SharedString::from(label), icon))
            })
            .take(SHOWN)
            .collect()
    }

    /// A link to `channel` like Slack's "Copy link": the workspace's archive URL.
    pub(super) fn conversation_link(&self, channel: &ChannelId) -> String {
        match self.fixtures.workspaces.first() {
            Some(team) if !team.domain.is_empty() => {
                format!("https://{}.slack.com/archives/{channel}", team.domain)
            }
            Some(team) => format!("https://app.slack.com/client/{}/{channel}", team.id),
            None => format!("https://app.slack.com/client/{channel}"),
        }
    }

    /// The header's search icon: the top search field scoped to this channel, like Slack's
    /// "Search in #channel".
    /// Whether the main center column is currently showing the conversation transcript.
    pub(super) fn showing_transcript(&self) -> bool {
        !self.searching
            && !self.drafts_open
            && !self.later_open
            && !self.threads_open
            && !self.unreads_open
            && !self.huddles_open
            && !self.directory_open
            && !self.tools_open
            && !self.compose_open
            && !self.activity_open
    }

    pub(super) fn search_in_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scope = if self.preferences.read(cx).settings().search_in_channel {
            self.fixtures
                .conversation(&self.channel)
                .filter(|c| {
                    matches!(
                        c.kind(),
                        ConversationKind::PublicChannel | ConversationKind::PrivateChannel
                    )
                })
                .and_then(|c| c.name.as_deref())
                .map(|name| format!("in:#{name} "))
                .unwrap_or_default()
        } else {
            String::new()
        };
        self.search
            .update(cx, |search, cx| search.set_value(scope, window, cx));
        self.search.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// Cycle keyboard focus between app panes (`F6` / `Shift+F6`).
    pub(super) fn cycle_pane_focus(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let has_right = self.thread_open || self.profile_open || self.pins_open;
        let count = if has_right { 4 } else { 3 };
        self.pane_focus = if forward {
            (self.pane_focus + 1) % count
        } else {
            (self.pane_focus + count - 1) % count
        };
        match self.pane_focus {
            0 => self.sidebar.update(cx, |s, cx| s.focus_find(window, cx)),
            1 => self.focus.focus(window, cx),
            2 => self.composer.update(cx, |c, cx| c.focus_input(window, cx)),
            _ => self.focus.focus(window, cx),
        }
    }

    /// Search messages for `query`, showing results in place of the message column. The
    /// pill's Enter and the dropdown's rows both land here; the dropdown's filter chips
    /// (and only its non-empty query) narrow the run and join its history.
    pub(super) fn run_message_search(&mut self, query: String, cx: &mut Context<Self>) {
        let filters = self
            .search_menu
            .read(cx)
            .search_filters(&self.channel, &self.fixtures.self_user);
        self.results.update(cx, |results, cx| {
            results.run_with_filters(query.clone(), filters, cx)
        });
        self.searching = true;
        self.drafts_open = false;
        self.later_open = false;
        if !query.trim().is_empty() {
            self.search_menu
                .update(cx, |menu, cx| menu.record_search(&query, cx));
            let history = self.search_menu.read(cx).history().to_vec();
            if let Some(write) = self.bridge.remember_search_history(&history) {
                cx.background_executor().spawn(write).detach();
            }
        }
        cx.notify();
    }

    /// Open the search pill's dropdown under the pill: the empty-query sections (unreads
    /// & drafts, recent places) are computed here, once per open.
    pub(super) fn open_search_menu(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.search_menu_snapshot(cx);
        let query = self.search.read(cx).value().to_string();
        self.search_menu
            .update(cx, |menu, cx| menu.open(snapshot, &query, cx));
        self.search_menu_open = true;
        cx.notify();
    }

    /// Fold the dropdown and put the field's placeholder back to its resting text,
    /// leaving focus alone: a blur, a search run or a workspace switch folds this way, so
    /// whatever took the focus (a dialog, the field itself) keeps it.
    pub(super) fn fold_search_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_menu_open {
            return;
        }
        self.search_menu_open = false;
        self.search.update(cx, |state, cx| {
            state.set_placeholder(SharedString::from(SEARCH_PLACEHOLDER), window, cx)
        });
        cx.notify();
    }

    /// Fold the dropdown and blur the pill (Escape or a click outside it): the open menu
    /// ghosts the row under the pointer or the arrows.
    pub(super) fn close_search_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_menu_open {
            return;
        }
        self.fold_search_menu(window, cx);
        self.focus.focus(window, cx);
    }

    /// The dropdown's empty-query rows: conversations with unread messages (the sidebar's
    /// own read state) or a saved draft, then the recently left ones, current excluded —
    /// it is the place the user is jumping from.
    pub(super) fn search_menu_snapshot(&self, cx: &Context<Self>) -> Snapshot {
        let mut unreads = self.sidebar.read(cx).unread_ids();
        let drafts = self.drafts.read(cx);
        for conversation in self.fixtures.channels.iter().chain(&self.fixtures.dms) {
            if conversation.id != self.channel
                && !unreads.contains(&conversation.id)
                && drafts.draft(self.workspace, &conversation.id).is_some()
            {
                unreads.push(conversation.id.clone());
            }
        }
        // Slack lists them by latest activity, newest first.
        unreads.sort_by_cached_key(|id| {
            std::cmp::Reverse(self.fixtures.counts.get(id).and_then(|n| n.latest.clone()))
        });
        let recents = self
            .recent
            .iter()
            .filter(|id| self.fixtures.conversation(id).is_some())
            .cloned()
            .collect();
        let (mentions, threads) = self.sidebar.read(cx).search_unread_counts();
        Snapshot {
            unreads,
            recents,
            mentions,
            threads,
        }
    }

    /// "Leave channel": it drops out of the sidebar at once and the view steps back, as in
    /// Slack; the core leaves on Slack and stores the channel as left.
    pub(super) fn leave(
        &mut self,
        channel: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.bridge
            .send(Command::LeaveConversation(channel.clone()));
        self.ws.counts.remove(&channel);
        self.back
            .retain(|p| *p != NavPlace::Channel(channel.clone()));
        self.forward
            .retain(|p| *p != NavPlace::Channel(channel.clone()));
        self.reload_live(window, cx);
        if self.channel == channel {
            let next = self
                .back
                .pop()
                .unwrap_or_else(|| NavPlace::Channel(first_conversation(&self.fixtures)));
            self.go_to_place(next, window, cx);
        }
        // Scrub `recent` after navigating away so `show_conversation` does not re-record the
        // channel we just left at the top of the history list.
        self.recent.retain(|c| *c != channel);
        if let Some(write) = self.bridge.remember_recent_places(&self.recent) {
            cx.background_executor().spawn(write).detach();
        }
        self.update_menus(cx);
    }

    /// Open `channel` in the transcript (a no-op when it already is): the sidebar selection,
    /// the core's open conversation, its history and its draft follow.
    pub(super) fn show_conversation(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        if channel == self.channel {
            return;
        }
        self.close_hover_card(cx);
        if let Some(write) = self.bridge.remember_conversation(channel.clone()) {
            cx.background_executor().spawn(write).detach();
        }
        /// Recent conversations kept for the history menu.
        const RECENT_MAX: usize = 50;
        let left = self.channel.clone();
        self.recent.retain(|c| *c != left && *c != channel);
        self.recent.insert(0, left);
        self.recent.truncate(RECENT_MAX);
        *self.ws.visits.entry(channel.clone()).or_default() += 1;
        if let Some(write) = self.bridge.remember_recent_places(&self.recent) {
            cx.background_executor().spawn(write).detach();
        }
        self.save_draft(cx);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_selected(channel.clone(), cx));
        self.bridge
            .send(Command::CloseConversation(self.channel.clone()));
        self.bridge.send(Command::OpenConversation(channel.clone()));
        self.messages
            .update(cx, |list, cx| list.set_conversation(channel.clone(), cx));
        // Seed frame 0 immediately from the read-only WAL SQLite cache (<1ms) so switching to a
        // prefetched unread channel shows the newest messages instantly without waiting for an
        // async executor hop or HTTP round-trip; `load_history` then fills the full 500-msg page
        // and updates read state.
        let cached_page = self.bridge.history_sync(&channel);
        if !cached_page.is_empty() {
            self.resolve_named_users(&cached_page);
            let mark_read_mode = self.preferences.read(cx).settings().mark_read;
            self.messages.update(cx, |list, cx| {
                list.set_history(channel.clone(), cached_page, cx);
                if mark_read_mode == 0 {
                    list.scroll_to_unread_once(cx);
                }
            });
        }
        self.load_history(&channel, cx);
        self.channel = channel;
        self.open_members = None;
        self.members_hover = (false, false);
        self.channel_people = None;
        self.sync_connect_orgs(cx);
        self.load_join_people(cx);
        self.close_thread(cx);
        self.tab = ConversationTab::Messages;
        // An open pins panel follows the conversation: fetch the new one's pins.
        if self.pins_open {
            self.load_tab_data(&ConversationTab::Pins);
        }
        self.load_draft(cx);
        self.sync_composer_placeholder(cx);
        cx.notify();
        self.update_menus(cx);
        self.show_typing(cx);
    }

    /// Slack's Opt+Up/Down (any row) and Opt+Shift+Up/Down (unread rows): open the sidebar
    /// conversation above or below the open one.
    pub(super) fn step_conversation(
        &mut self,
        forward: bool,
        unread_only: bool,
        cx: &mut Context<Self>,
    ) {
        let next = self
            .sidebar
            .read(cx)
            .step(&self.channel, forward, unread_only);
        if let Some(next) = next {
            self.select(next, cx);
        }
    }

    /// Cmd+= / Cmd+- step Preferences' zoom a level up or down (`None`, Cmd+0: 100%), which
    /// applies it.
    pub(super) fn step_zoom(&mut self, step: Option<isize>, cx: &mut Context<Self>) {
        let level = self.preferences.read(cx).settings().zoom;
        let next = match step {
            Some(step) => level
                .saturating_add_signed(step)
                .min(crate::zoom::FACTORS.len() - 1),
            None => crate::zoom::ACTUAL_SIZE,
        };
        self.preferences
            .update(cx, |prefs, cx| prefs.set_zoom(next, cx));
    }

    /// Put `views` on screen as the workspace views; returns the ones they replace.
    pub(super) fn swap_views(&mut self, views: WorkspaceViews) -> WorkspaceViews {
        WorkspaceViews {
            sidebar: mem::replace(&mut self.sidebar, views.sidebar),
            messages: mem::replace(&mut self.messages, views.messages),
            thread: mem::replace(&mut self.thread, views.thread),
            results: mem::replace(&mut self.results, views.results),
            browser: mem::replace(&mut self.browser, views.browser),
            huddle: mem::replace(&mut self.huddle, views.huddle),
            profile: mem::replace(&mut self.profile, views.profile),
            switcher: mem::replace(&mut self.switcher, views.switcher),
        }
    }

    /// Show workspace `index`: park the current workspace's views (and live bridge/state) and
    /// bring back the target's (built from its live bridge or sample fixtures on the first
    /// visit). Drafts carry over through the store.
    pub(super) fn switch_workspace(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index == self.workspace || index >= self.parked.len() {
            return;
        }
        self.save_draft(cx);
        self.bridge
            .send(Command::CloseConversation(self.channel.clone()));
        // Swap live bridges when target workspace has its own `CoreBridge`.
        let swapped_bridge = self.parked_bridges.get_mut(index).and_then(Option::take);
        if let Some(incoming_bridge) = swapped_bridge {
            let outgoing_bridge = mem::replace(&mut self.bridge, incoming_bridge);
            if let Some(slot) = self.parked_bridges.get_mut(self.workspace) {
                *slot = Some(outgoing_bridge);
            }
            self.bridge.activate_media(cx);
        }
        let incoming = match self.parked[index].take() {
            Some(parked) => parked,
            None => {
                let bg_counts = self.bg_counts.get(index).cloned().unwrap_or_default();
                let (fixtures, channel, archive, chosen) =
                    if let Some(mut snap) = self.bridge.snapshot(&bg_counts) {
                        if !self.live_teams.is_empty() {
                            snap.workspaces = self.live_teams.clone();
                        }
                        let restored = self
                            .bridge
                            .last_conversation()
                            .filter(|c| snap.conversation(c).is_some());
                        let chosen = restored.is_some();
                        let channel = restored.unwrap_or_else(|| first_conversation(&snap));
                        let archive = self.bridge.store().cloned();
                        // Restore persisted drafts for this workspace on first visit.
                        if let Some(saved_drafts) = self.bridge.drafts() {
                            let entries: Vec<Entry> = saved_drafts
                                .into_iter()
                                .map(|d| Entry {
                                    id: None,
                                    workspace: index,
                                    channel: d.channel,
                                    label: d.label,
                                    text: d.text,
                                    at: d.at,
                                })
                                .collect();
                            self.drafts
                                .update(cx, |view, cx| view.load_drafts(entries, cx));
                        }
                        (Arc::new(snap), channel, archive, chosen)
                    } else {
                        let fixtures = Arc::new(fixtures::workspace(index));
                        let channel = first_conversation(&fixtures);
                        let archive = fixture_archive(&fixtures);
                        (fixtures, channel, archive, false)
                    };
                let views = build_workspace(&fixtures, index, archive, &channel, window, cx);
                if let Some(prefs) = self.bridge.section_prefs() {
                    views
                        .sidebar
                        .update(cx, |sidebar, cx| sidebar.restore_section_prefs(prefs, cx));
                }
                let subscriptions = subscribe_workspace(&views, window, cx);
                Parked {
                    fixtures,
                    channel,
                    views,
                    subscriptions,
                    state: WorkspaceState {
                        counts: bg_counts,
                        chosen,
                        ..Default::default()
                    },
                }
            }
        };
        let outgoing = Parked {
            fixtures: mem::replace(&mut self.fixtures, incoming.fixtures),
            channel: mem::replace(&mut self.channel, incoming.channel),
            views: self.swap_views(incoming.views),
            subscriptions: mem::replace(&mut self.workspace_subscriptions, incoming.subscriptions),
            state: mem::replace(&mut self.ws, incoming.state),
        };
        self.parked[self.workspace] = Some(outgoing);
        self.workspace = index;
        self.open_members = None;
        self.members_hover = (false, false);
        self.channel_people = None;
        self.sync_connect_orgs(cx);
        self.load_join_people(cx);
        self.searching = false;
        self.drafts_open = false;
        self.later_open = false;
        self.threads_open = false;
        self.unreads_open = false;
        self.unreads_fetched.clear();
        self.back.clear();
        self.forward.clear();
        self.huddles_open = false;
        self.directory_open = false;
        self.tools_open = false;
        self.compose_open = false;
        self.pending_dm = None;
        self.close_thread(cx);
        self.profile_open = false;
        self.pins_open = false;
        let fixtures = self.fixtures.clone();
        self.threads.update(cx, |view, cx| {
            view.clear_for_workspace(fixtures.clone(), cx)
        });
        self.later.update(cx, |later, cx| {
            later.set_workspace(index, cx);
            if fixtures.sample {
                later.seed(index, &fixtures, cx);
            }
        });
        self.fold_search_menu(window, cx);
        self.search_menu
            .update(cx, |menu, cx| menu.set_fixtures(fixtures.clone(), cx));
        // The Files view serves every workspace: its rows follow the shown one.
        self.files_page
            .update(cx, |page, cx| page.set_fixtures(fixtures.clone(), cx));
        self.tools_page
            .update(cx, |page, cx| page.set_fixtures(fixtures.clone(), cx));
        // Before the draft is loaded: its mentions are named from this workspace.
        self.composer
            .update(cx, |composer, cx| composer.set_fixtures(fixtures, cx));
        self.sync_saved(cx);
        self.sync_later_count(cx);
        self.huddle_open = false;
        self.tab = ConversationTab::Messages;
        let channel = self.channel.clone();
        if !channel.as_str().is_empty() {
            self.bridge.send(Command::OpenConversation(channel.clone()));
            self.load_history(&channel, cx);
        }
        self.load_draft(cx);
        self.sync_composer_placeholder(cx);
        // Apply per-workspace color theme palette (`F-SHELL-076`).
        let ws_palette = self
            .bridge
            .settings()
            .map(|s| s.palette)
            .or(self.fixtures.palette);
        if let Some(palette) = ws_palette {
            self.preferences.update(cx, |prefs, cx| {
                let mut next = prefs.settings().clone();
                if next.palette != palette {
                    next.palette = palette;
                    prefs.load_settings(next, window, cx);
                }
            });
        }
        // New thread panels / profile panes pick up Enter-to-send and the self state.
        self.apply_preferences(window, cx);
        self.update_menus(cx);
        cx.notify();
    }

    pub(super) fn open_drafts(&mut self, cx: &mut Context<Self>) {
        self.record_nav(&NavPlace::Drafts, cx);
        self.show_drafts(cx);
    }

    pub(super) fn show_drafts(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.close_full_column_views(cx);
        self.drafts_open = true;
        self.sidebar.update(cx, |s, cx| {
            s.set_home_page(Some(crate::sidebar::HomePage::Drafts), cx)
        });
        if !self.fixtures.sample {
            self.bridge.send(Command::LoadScheduledMessages);
            self.bridge.send(Command::FetchCloudDrafts);
            if let Some(read) = self.bridge.recent_sent(self.fixtures.self_user.clone(), 50) {
                let workspace = self.workspace;
                let fixtures = self.fixtures.clone();
                let drafts = self.drafts.downgrade();
                cx.spawn(async move |_, cx| {
                    let stored = cx.background_executor().spawn(read).await;
                    let entries: Vec<Entry> = stored
                        .into_iter()
                        .filter_map(|row| {
                            let text = fixtures.plain_text(&row.message);
                            if text.trim().is_empty() {
                                return None;
                            }
                            let label = fixtures
                                .conversation(&row.channel)
                                .map(|c| fixtures.conversation_label(c))
                                .unwrap_or_else(|| row.channel.to_string());
                            let at = row.message.ts.secs().unwrap_or(0).max(0) as u64;
                            Some(Entry {
                                id: None,
                                workspace,
                                channel: row.channel,
                                label,
                                text,
                                at,
                            })
                        })
                        .collect();
                    let _ = drafts.update(cx, |view, cx| {
                        view.set_stored_sent(workspace, entries, cx);
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// Close every full-column view (a conversation or another view is taking the column).
    pub(super) fn close_full_column_views(&mut self, cx: &mut Context<Self>) {
        self.searching = false;
        self.drafts_open = false;
        self.later_open = false;
        self.threads_open = false;
        self.unreads_open = false;
        self.huddles_open = false;
        self.directory_open = false;
        self.tools_open = false;
        self.compose_open = false;
        self.activity_open = false;
        // The Activity and Files pages are sidebar destinations, so leaving the full-column
        // views returns the rail to Home.
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.leave_pages(cx);
            sidebar.set_home_page(None, cx);
        });
    }

    /// The Threads view: fixture cards in sample mode, or Slack's own list
    /// (`subscriptions.thread.getView`) in live mode — keeping any already-fetched remote cards
    /// on screen while refreshing silently so there is never a flash of stale/empty content.
    pub(super) fn open_threads(&mut self, cx: &mut Context<Self>) {
        self.record_nav(&NavPlace::Threads, cx);
        self.show_threads(cx);
    }

    pub(super) fn show_threads(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        let fixtures = self.fixtures.clone();
        if self.fixtures.sample {
            let cards = local_cards(&self.fixtures);
            self.threads.update(cx, |view, cx| {
                view.set_fixtures(fixtures, cx);
                view.show_local(cards, cx);
            });
        } else {
            self.threads
                .update(cx, |view, cx| view.begin_remote_load(fixtures, cx));
            self.bridge.send(Command::FetchThreads);
        }
        self.close_full_column_views(cx);
        self.threads_open = true;
        self.sidebar.update(cx, |s, cx| {
            s.set_home_page(Some(crate::sidebar::HomePage::Threads), cx)
        });
        cx.notify();
    }

    /// Offline / error fallback for Threads: derive cards from the local SQLite store only when
    /// `subscriptions.thread.getView` fails.
    pub(super) fn load_threads_store_fallback(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.bridge.store().cloned() else {
            return;
        };
        let team = self.fixtures.workspaces[self.workspace].id.clone();
        let me = self.fixtures.self_user.clone();
        cx.spawn(async move |this, cx| {
            let cards = cx
                .background_executor()
                .spawn(async move {
                    let Ok(store) = store.lock() else {
                        return Vec::new();
                    };
                    store_cards(&store, &team, &me)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.threads
                    .update(cx, |view, cx| view.show_store(cards, cx));
            });
        })
        .detach();
    }

    /// The All Unreads view (Slack's `p-unreads_view`, Cmd+Shift+A): the sidebar's unread
    /// order as one card per conversation - its rows from the bundled workspace at once
    /// and, in live mode, from the local store off the UI thread.
    pub(super) fn open_unreads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.record_nav(&NavPlace::Unreads, cx);
        self.show_unreads(window, cx);
    }

    pub(super) fn show_unreads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.close_full_column_views(cx);
        self.unreads_open = true;
        self.sidebar.update(cx, |s, cx| {
            s.set_home_page(Some(crate::sidebar::HomePage::Unreads), cx)
        });
        self.focus.focus(window, cx);
        self.refresh_unreads(cx);
        cx.notify();
    }

    /// Re-derive the open Unreads view's cards: the view opened, the workspace reloaded, a
    /// message landed, or everything was marked read.
    pub(super) fn refresh_unreads(&mut self, cx: &mut Context<Self>) {
        let order = self.sidebar.read(cx).unread_ids();
        let groups = local_groups(&self.fixtures, &order);
        let fixtures = self.fixtures.clone();
        if let Some(store) = self.bridge.store().cloned() {
            // Live mode: keep already-loaded store groups on screen while refreshing off-thread
            // (never flash empty `local_groups` placeholders).
            self.unreads
                .update(cx, |view, cx| view.begin_live_refresh(fixtures, cx));
            let team = self.fixtures.workspaces[self.workspace].id.clone();
            let targets = store_targets(&self.fixtures, &groups);
            cx.spawn(async move |this, cx| {
                let groups = cx
                    .background_executor()
                    .spawn(async move {
                        let Ok(store) = store.lock() else {
                            return Vec::new();
                        };
                        store_groups(&store, &team, &targets)
                    })
                    .await;
                this.update(cx, |app, cx| {
                    // Only conversations Slack still counts unread take part: a mark may
                    // have raced the store read.
                    let keep = app.sidebar.read(cx).unread_ids();
                    for group in &groups {
                        if group.messages.is_empty()
                            && keep.contains(&group.channel)
                            && app.unreads_fetched.insert(group.channel.clone())
                        {
                            app.bridge
                                .send(Command::FetchLatestHistory(group.channel.clone()));
                        }
                    }
                    app.unreads
                        .update(cx, |view, cx| view.show_store(groups, &keep, cx));
                })
            })
            .detach();
        } else {
            self.unreads
                .update(cx, |view, cx| view.show_local(fixtures, groups, cx));
        }
    }

    /// The directory: the workspace's people and channels.
    pub(super) fn open_directory(&mut self, cx: &mut Context<Self>) {
        self.record_nav(&NavPlace::Directories, cx);
        self.show_directory(cx);
    }

    pub(super) fn show_directory(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        let fixtures = self.fixtures.clone();
        self.directory
            .update(cx, |view, cx| view.set_fixtures(fixtures, cx));
        self.close_full_column_views(cx);
        self.directory_open = true;
        self.sidebar.update(cx, |s, cx| {
            s.set_home_page(Some(crate::sidebar::HomePage::Directories), cx)
        });
        cx.notify();
    }

    /// The "New message" view, focused on its To field.
    /// Slack's "New message" view, empty or with `draft` (Forward drafts the permalink).
    pub(super) fn open_compose(
        &mut self,
        draft: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_draft(cx);
        let fixtures = self.fixtures.clone();
        self.compose
            .update(cx, |view, cx| view.set_fixtures(fixtures, cx));
        self.compose
            .update(cx, |view, cx| view.open(draft, window, cx));
        self.close_full_column_views(cx);
        self.compose_open = true;
        cx.notify();
    }

    /// The gear menu's "Create a channel section": Slack-side in live mode (the section
    /// arrives with the next sidebar reload), local in the bundled sample workspace.
    pub(super) fn create_section(&mut self, name: String, cx: &mut Context<Self>) {
        if self.fixtures.sample {
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.add_section(name, cx));
        } else {
            self.bridge.send(Command::CreateSection { name });
        }
    }

    pub(super) fn show_nav(&mut self, item: NavItem, cx: &mut Context<Self>) {
        match item {
            NavItem::Activity => self.open_activity(cx),
            NavItem::Files => self.open_files(cx),
            NavItem::Todos => self.open_later(cx),
            NavItem::Agents => self.open_tools(cx),
            _ => {
                self.close_full_column_views(cx);
                self.sidebar
                    .update(cx, |sidebar, cx| sidebar.show_nav(item, cx));
                cx.notify();
            }
        }
    }

    /// Native `Agents & tools` view (Apps, Slash commands, Slack Connect).
    pub(super) fn open_tools(&mut self, cx: &mut Context<Self>) {
        self.record_nav(&NavPlace::Tools, cx);
        self.show_tools(cx);
    }

    pub(super) fn show_tools(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        let fixtures = self.fixtures.clone();
        self.tools_page
            .update(cx, |page, cx| page.set_fixtures(fixtures, cx));
        self.close_full_column_views(cx);
        self.tools_open = true;
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.show_nav(NavItem::Agents, cx));
        cx.notify();
    }

    /// Open Slack workspace administration in the default browser.
    pub(super) fn open_admin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let domain = self
            .fixtures
            .workspaces
            .get(self.workspace)
            .or_else(|| self.fixtures.workspaces.first())
            .map(|t| t.domain.as_str())
            .filter(|d| !d.is_empty());
        if let Some(domain) = domain {
            cx.open_url(&format!("https://{domain}.slack.com/admin"));
        } else {
            notify(
                "Workspace Admin",
                "Sign in to a workspace to open Slack Admin settings.",
                window,
                cx,
            );
        }
    }

    /// The Activity page (the rail's Activity destination): a full-width page right of the
    /// rail - the sidebar hides its list column while it is active. Slack's own feed
    /// (`activity.feed`) lands with the next [`Event::ActivityView`]; the fixture workspace
    /// serves its derived rows.
    pub(super) fn open_activity(&mut self, cx: &mut Context<Self>) {
        self.record_nav(&NavPlace::Activity, cx);
        self.show_activity(cx);
    }

    pub(super) fn show_activity(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.close_full_column_views(cx);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.show_nav(NavItem::Activity, cx));
        self.activity_open = true;
        let fixtures = self.fixtures.clone();
        self.activity
            .update(cx, |view, cx| view.set_fixtures(fixtures, cx));
        self.fetch_activity(cx);
        cx.notify();
    }

    /// Ask the core for the Activity feed (live mode; fixture mode keeps its derived rows).
    pub(super) fn fetch_activity(&mut self, cx: &mut Context<Self>) {
        if self.bridge.store().is_some() {
            self.activity.update(cx, |view, cx| view.start_loading(cx));
            self.bridge
                .send(Command::FetchActivity { with_bodies: true });
        }
    }

    /// The Files view (the rail's Files destination): the sidebar shows its Files list
    /// column and the page takes the main area. Live mode refetches the shown page's rows
    /// (`files.list`, answered by [`Event::FilesLoaded`]); the sample workspace derives them
    /// from its messages.
    pub(super) fn open_files(&mut self, cx: &mut Context<Self>) {
        self.record_nav(&NavPlace::Files, cx);
        self.show_files(cx);
    }

    pub(super) fn show_files(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.close_full_column_views(cx);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.show_nav(NavItem::Files, cx));
        let fixtures = self.fixtures.clone();
        self.files_page
            .update(cx, |page, cx| page.open(fixtures, cx));
        cx.notify();
    }
}
