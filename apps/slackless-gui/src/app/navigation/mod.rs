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
        self.page = MainPage::Later;
        self.sidebar.update(cx, |s, cx| {
            s.set_home_page(Some(crate::sidebar::HomePage::Saved), cx)
        });
        self.bridge.send(MessagingCommand::LoadSaved);
        cx.notify();
    }

    /// Transition the right-hand flexpane, always clearing open-thread tint when leaving
    /// `RightPane::Thread` and resetting `ConversationTab::Pins` when leaving `RightPane::Pins`.
    pub(super) fn set_right_pane(&mut self, next: RightPane, cx: &mut Context<Self>) {
        let prev = self.right_pane;
        if prev.is_thread() && !next.is_thread() {
            self.messages
                .update(cx, |list, cx| list.set_open_thread(None, cx));
            self.unreads
                .update(cx, |unreads, cx| unreads.set_open_thread(None, cx));
        }
        if prev.is_pins() && !next.is_pins() && self.tab == ConversationTab::Pins {
            self.tab = ConversationTab::Messages;
        }
        self.right_pane = next;
        if next.is_thread()
            && let Some((channel, thread_ts)) = self.thread.read(cx).open_thread()
        {
            self.messages.update(cx, |list, cx| {
                list.set_open_thread(Some(thread_ts.clone()), cx)
            });
            self.unreads.update(cx, |unreads, cx| {
                unreads.set_open_thread(Some((channel, thread_ts)), cx)
            });
        }
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
        self.bridge
            .send(MessagingCommand::OpenThread { channel, thread_ts });
        self.set_right_pane(RightPane::Thread, cx);
        if self.tab == ConversationTab::Pins {
            self.tab = ConversationTab::Messages;
        }
        self.show_typing(cx);
        cx.notify();
    }

    /// Hide the thread panel and drop the open-thread tint from the transcript.
    pub(super) fn close_thread(&mut self, cx: &mut Context<Self>) {
        if self.right_pane.is_thread() {
            self.set_right_pane(RightPane::None, cx);
        } else {
            self.messages
                .update(cx, |list, cx| list.set_open_thread(None, cx));
            self.unreads
                .update(cx, |unreads, cx| unreads.set_open_thread(None, cx));
        }
    }

    pub(super) fn toggle_pins(&mut self, cx: &mut Context<Self>) {
        if self.right_pane.is_pins() {
            self.set_right_pane(RightPane::None, cx);
        } else {
            self.set_right_pane(RightPane::Pins, cx);
            self.load_tab_data(&ConversationTab::Pins);
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
        if self.right_pane.is_open() {
            self.last_right_pane = Some(self.right_pane);
            self.set_right_pane(RightPane::None, cx);
        } else {
            match self.last_right_pane {
                Some(RightPane::Thread) if self.thread.read(cx).open_thread().is_some() => {
                    self.set_right_pane(RightPane::Thread, cx);
                }
                Some(pane @ RightPane::Profile { .. }) => {
                    self.set_right_pane(pane, cx);
                }
                Some(RightPane::Pins) => {
                    self.set_right_pane(RightPane::Pins, cx);
                }
                _ => {
                    self.set_right_pane(RightPane::Pins, cx);
                    if !self.fixtures.sample && !self.ws.pins.contains_key(&self.channel) {
                        self.bridge
                            .send(MessagingCommand::LoadPins(self.channel.clone()));
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
        self.close_full_column_views(cx);
        self.sidebar.update(cx, |sidebar, cx| sidebar.show_home(cx));
        cx.notify();
    }

    /// Swap rail tiles `from` and `to` and persist the order (`F-CORE-003`). Only the rail's
    /// order changes: every workspace keeps its slot, so its events, drafts and Later items
    /// stay its own.
    pub(super) fn reorder_workspace(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let len = self.rail.len();
        if from == to || from >= len || to >= len {
            return;
        }
        self.rail.swap(from, to);
        let order: Vec<String> = (self.rail.iter())
            .map(|&ix| self.slots[ix].team.id.as_str().to_owned())
            .collect();
        if let Some(write) = self.bridge.remember(&WORKSPACE_ORDER, order.as_slice()) {
            cx.background_executor().spawn(write).detach();
        }
        self.sync_rail(cx);
        self.sync_badge(cx);
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
            self.bridge.send(ChannelCommand::OpenDm {
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
        match self.page {
            MainPage::CatchUp => NavPlace::CatchUp,
            MainPage::Threads => NavPlace::Threads,
            MainPage::Unreads => NavPlace::Unreads,
            MainPage::Huddles => NavPlace::Huddles,
            MainPage::Drafts => NavPlace::Drafts,
            MainPage::Later => NavPlace::Saved,
            MainPage::Directory => NavPlace::Directories,
            MainPage::Tools => NavPlace::Tools,
            MainPage::Activity => NavPlace::Activity,
            MainPage::Files => NavPlace::Files,
            MainPage::Channel | MainPage::Search | MainPage::Compose => {
                if self.sidebar.read(cx).active_nav() == NavItem::Files {
                    NavPlace::Files
                } else {
                    NavPlace::Channel(self.channel.clone())
                }
            }
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
        self.page == MainPage::Channel
            && self.sidebar.read(cx).active_nav() != crate::sidebar::NavItem::Files
    }

    /// Show `channel` as the user's choice, leaving any full-column view (search, drafts,
    /// later). History is the caller's: [`App::select`] records it, back/forward replay it.
    pub(super) fn navigate(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        let was_covered = !self.transcript_visible(cx);
        self.ws.chosen = true;
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
            NavPlace::CatchUp => self.show_catchup(window, cx),
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
        let team = self.team();
        if team.domain.is_empty() {
            format!("https://app.slack.com/client/{}/{channel}", team.id)
        } else {
            format!("https://{}.slack.com/archives/{channel}", team.domain)
        }
    }

    /// The header's search icon: the top search field scoped to this channel, like Slack's
    /// "Search in #channel".
    /// Whether the main center column is currently showing the conversation transcript.
    pub(super) fn showing_transcript(&self) -> bool {
        self.page == MainPage::Channel
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

    /// Cycle keyboard focus between app panes (`F6` / `Shift+F6`), or between canvas regions
    /// when a Canvas tab is active (`F-COLLAB-194`).
    pub(super) fn cycle_pane_focus(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let showing_canvas = self.tab == ConversationTab::Canvas && self.transcript_visible(cx);
        if showing_canvas {
            let key = if self.fixtures.sample {
                "fixture".to_owned()
            } else {
                self.ws
                    .active_canvas
                    .clone()
                    .unwrap_or_else(|| "live-canvas".to_owned())
            };
            let has_outline = if self.fixtures.sample {
                true
            } else {
                self.ws
                    .canvases
                    .get(&key)
                    .and_then(|r| r.as_ref().ok())
                    .is_some_and(|doc| doc.outline_headings().len() >= 2)
            };
            let _ =
                crate::canvas::cycle_canvas_focus_region(&key, forward, has_outline, window, cx);
            return;
        }
        let has_right = self.right_pane.is_open();
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

    /// The slot of the workspace `team_id` names.
    pub(in crate::app) fn workspace_index_for(
        &self,
        team_id: &slackless_core::types::TeamId,
    ) -> Option<usize> {
        self.slots.iter().position(|slot| slot.team.id == *team_id)
    }

    /// Build the list of all mounted workspace archives (active + parked live workspaces, or all
    /// fixture workspaces in sample mode) and hand them to `SearchView` for "All workspaces" FTS5.
    pub(in crate::app) fn sync_search_workspace_sources(&self, cx: &mut Context<Self>) {
        self.sync_search_workspace_sources_inner(true, cx);
    }

    fn sync_search_workspace_sources_inner(&self, refresh: bool, cx: &mut Context<Self>) {
        let mut sources: Vec<SearchWorkspaceSource> = Vec::new();
        if !self.fixtures.sample {
            for (idx, slot) in self.slots.iter().enumerate() {
                if let Some(bridge) = self.workspace_bridge(idx)
                    && let (Some(store), Some(tid)) =
                        (bridge.read_store_clone(), bridge.team_id().cloned())
                {
                    let name = Some(slot.team.name.clone())
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or_else(|| tid.to_string());
                    sources.push(SearchWorkspaceSource {
                        team_id: tid,
                        name,
                        store,
                    });
                }
            }
        } else {
            for (idx, slot) in self.slots.iter().enumerate() {
                let team = &slot.team;
                let fx = self
                    .workspace_fixtures(idx)
                    .unwrap_or_else(|| Arc::new(fixtures::workspace(idx)));
                if let Some(store) = fixture_archive_for(&fx, &team.id) {
                    sources.push(SearchWorkspaceSource {
                        team_id: team.id.clone(),
                        name: team.name.clone(),
                        store,
                    });
                }
            }
        }
        self.results.update(cx, |results, cx| {
            if refresh {
                results.set_workspace_sources(sources, cx);
            } else {
                results.set_workspace_sources_quiet(sources);
            }
        });
    }

    /// Jump to a search hit located in `workspace`: switches workspace if needed, selects the
    /// channel, and reveals `ts` in the transcript.
    pub(in crate::app) fn open_message_in_workspace(
        &mut self,
        workspace: &slackless_core::types::TeamId,
        channel: ChannelId,
        ts: Ts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(idx) = self.workspace_index_for(workspace) else {
            notify(
                "Workspace not loaded",
                format!(
                    "Workspace {} is archived on disk but not currently signed in.",
                    workspace
                ),
                window,
                cx,
            );
            return;
        };
        if idx != self.workspace {
            self.switch_workspace(idx, window, cx);
        }
        self.select(channel, cx);
        self.messages.update(cx, |list, cx| list.reveal(&ts, cx));
    }

    /// Search messages for `query`, showing results in place of the message column. The
    /// pill's Enter and the dropdown's rows both land here; the dropdown's filter chips
    /// (and only its non-empty query) narrow the run and join its history.
    pub(super) fn run_message_search(&mut self, query: String, cx: &mut Context<Self>) {
        self.sync_search_workspace_sources_inner(false, cx);
        let filters = self
            .search_menu
            .read(cx)
            .search_filters(&self.channel, &self.fixtures.self_user);
        self.results.update(cx, |results, cx| {
            results.run_with_filters(query.clone(), filters, cx)
        });
        self.close_full_column_views(cx);
        self.page = MainPage::Search;
        if !query.trim().is_empty() {
            self.search_menu
                .update(cx, |menu, cx| menu.record_search(&query, cx));
            let history = self.search_menu.read(cx).history().to_vec();
            if let Some(write) = self.bridge.remember(&SEARCH_HISTORY, history.as_slice()) {
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
            .send(ChannelCommand::LeaveConversation(channel.clone()));
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
        if let Some(write) = self.bridge.remember(&RECENT_PLACES, self.recent.as_slice()) {
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
        if let Some(write) = self.bridge.remember(&LAST_CONVERSATION, &channel) {
            cx.background_executor().spawn(write).detach();
        }
        /// Recent conversations kept for the history menu.
        const RECENT_MAX: usize = 50;
        let left = self.channel.clone();
        self.recent.retain(|c| *c != left && *c != channel);
        self.recent.insert(0, left);
        self.recent.truncate(RECENT_MAX);
        *self.ws.visits.entry(channel.clone()).or_default() += 1;
        if let Some(write) = self.bridge.remember(&RECENT_PLACES, self.recent.as_slice()) {
            cx.background_executor().spawn(write).detach();
        }
        self.save_draft(cx);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_selected(channel.clone(), cx));
        self.bridge
            .send(ChannelCommand::CloseConversation(self.channel.clone()));
        self.bridge
            .send(ChannelCommand::OpenConversation(channel.clone()));
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
        if self.right_pane.is_pins() {
            self.load_tab_data(&ConversationTab::Pins);
        }
        let ws_outbox = self.ws.outbox.clone();
        let ws_offline = self.is_connection_offline();
        self.thread.update(cx, |panel, cx| {
            panel.set_outbox(ws_outbox, cx);
            panel.set_offline(ws_offline, cx);
        });
        self.load_draft(cx);
        self.sync_composer_placeholder(cx);
        self.sync_window_title(cx);
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
            profile: mem::replace(&mut self.profile, views.profile),
            switcher: mem::replace(&mut self.switcher, views.switcher),
        }
    }

    /// The shown workspace's team.
    pub(super) fn team(&self) -> &Team {
        &self.slots[self.workspace].team
    }

    /// Every workspace's team in rail order: what the rail, the menus and the menu bar item
    /// list.
    pub(super) fn rail_teams(&self) -> Vec<Team> {
        (self.rail.iter())
            .map(|&ix| self.slots[ix].team.clone())
            .collect()
    }

    /// Where slot `ix` sits on the rail.
    pub(super) fn rail_position(&self, ix: usize) -> usize {
        self.rail
            .iter()
            .position(|&slot| slot == ix)
            .unwrap_or_default()
    }

    /// The live bridge of the workspace in slot `ix` (the shown one's is `self.bridge`).
    pub(super) fn workspace_bridge(&self, ix: usize) -> Option<&CoreBridge> {
        if ix == self.workspace {
            Some(&self.bridge)
        } else {
            self.slots.get(ix)?.bridge.as_ref()
        }
    }

    /// The snapshot of the workspace in slot `ix`: the shown one's, or a visited one's parked
    /// snapshot (`None` before a first visit).
    pub(super) fn workspace_fixtures(&self, ix: usize) -> Option<Arc<Fixtures>> {
        if ix == self.workspace {
            return Some(self.fixtures.clone());
        }
        let parked = self.slots.get(ix)?.parked.as_ref()?;
        Some(parked.fixtures.clone())
    }

    /// Show the workspace at rail position `position` (a tile, Cmd+1…9, the menus).
    pub(super) fn switch_workspace_at(
        &mut self,
        position: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(&ix) = self.rail.get(position) {
            self.switch_workspace(ix, window, cx);
        }
    }

    /// Show the next (or previous) workspace down the rail, wrapping (`Cmd+}` / `Cmd+{`).
    pub(super) fn step_workspace(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let len = self.rail.len();
        let position = self.rail_position(self.workspace);
        let next = if forward {
            (position + 1) % len
        } else {
            (position + len - 1) % len
        };
        self.switch_workspace_at(next, window, cx);
    }

    /// Show the rail in its current order with the shown workspace's tile active. The rail
    /// lists the shown snapshot's `workspaces`, so a snapshot taken before the last reorder or
    /// team refresh (or a sample workspace's own list) takes the current order first.
    pub(super) fn sync_rail(&mut self, cx: &mut Context<Self>) {
        let teams = self.rail_teams();
        if self.fixtures.workspaces != teams {
            let mut fixtures = self.fixtures.with_counts(self.fixtures.counts.clone());
            fixtures.workspaces = teams;
            let fixtures = Arc::new(fixtures);
            self.sync_all_views_fixtures(fixtures.clone(), cx);
            self.fixtures = fixtures;
        }
        let position = self.rail_position(self.workspace);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_workspace(position, cx));
    }

    /// Show the workspace in slot `index`: park the current workspace's views (and live
    /// bridge/state) and bring back the target's (built from its live bridge or sample
    /// fixtures on the first visit). Drafts carry over through the store.
    pub(super) fn switch_workspace(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index == self.workspace || index >= self.slots.len() {
            return;
        }
        // Stop any active inline video / PiP dock, full-screen viewer, PDF render cache, or
        // open app modal / workflow details dialog from the outgoing workspace before swapping
        // bridges, auth cookies, and fixtures.
        crate::media::stop_active_inline_video(window, cx);
        self.close_image_viewer(window, cx);
        crate::media::clear_pdf_cache(window, cx);
        self.modal_stack
            .update(cx, |stack, cx| stack.dismiss_all(window, cx));
        self.workflow_details = None;
        self.save_draft(cx);
        self.bridge
            .send(ChannelCommand::CloseConversation(self.channel.clone()));
        // Swap live bridges when target workspace has its own `CoreBridge`.
        if let Some(incoming_bridge) = self.slots[index].bridge.take() {
            let outgoing_bridge = mem::replace(&mut self.bridge, incoming_bridge);
            self.slots[self.workspace].bridge = Some(outgoing_bridge);
            self.bridge.activate_media(cx);
        }
        // The state kept while in the background (counts, presence, connection, ...) carries on.
        let mut state = mem::take(&mut self.slots[index].state);
        let incoming = match self.slots[index].parked.take() {
            Some(parked) => parked,
            None => {
                // Live: the workspace paints at once from a placeholder while its snapshot is
                // read off the UI thread, as the incoming state's pending reload.
                let local = self.bridge.all_local();
                let (fixtures, channel, archive, chosen, reload) =
                    if let Some(mut snap) = self.bridge.placeholder(&state.counts) {
                        snap.workspaces = self.rail_teams();
                        // The conversation last open here opens at once; the snapshot confirms
                        // it when it lands, else the first conversation opens instead.
                        let restored = LAST_CONVERSATION.read(&local);
                        let chosen = restored.is_some();
                        let channel = restored.unwrap_or_default();
                        let archive = self.bridge.read_store_clone();
                        // Restore persisted drafts for this workspace on first visit.
                        if let Some(saved_drafts) = DRAFTS.read(&local) {
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
                        (Arc::new(snap), channel, archive, chosen, Some(Reload::Full))
                    } else {
                        let fixtures = Arc::new(fixtures::workspace(index));
                        let channel = first_conversation(&fixtures);
                        let archive = fixture_archive_for(&fixtures, &self.slots[index].team.id);
                        (fixtures, channel, archive, false, None)
                    };
                let views = build_workspace(
                    &fixtures,
                    &self.slots[index].team,
                    archive,
                    &channel,
                    window,
                    cx,
                );
                if let Some(prefs) = SECTION_PREFS.read(&local) {
                    views
                        .sidebar
                        .update(cx, |sidebar, cx| sidebar.restore_section_prefs(prefs, cx));
                }
                let initial_outbox = self.bridge.outbox_sync();
                if !initial_outbox.is_empty() {
                    views
                        .thread
                        .update(cx, |panel, cx| panel.set_outbox(initial_outbox.clone(), cx));
                }
                let subscriptions = subscribe_workspace(&views, window, cx);
                state.chosen = chosen;
                state.outbox = initial_outbox;
                state.reload = reload;
                Parked {
                    fixtures,
                    channel,
                    views,
                    subscriptions,
                }
            }
        };
        let outgoing = Parked {
            fixtures: mem::replace(&mut self.fixtures, incoming.fixtures),
            channel: mem::replace(&mut self.channel, incoming.channel),
            views: self.swap_views(incoming.views),
            subscriptions: mem::replace(&mut self.workspace_subscriptions, incoming.subscriptions),
        };
        let outgoing_state = mem::replace(&mut self.ws, state);
        let left = &mut self.slots[self.workspace];
        left.parked = Some(outgoing);
        left.state = outgoing_state;
        self.workspace = index;
        // The outgoing workspace's read in flight is dropped (its reload stays pending in its
        // parked state); the incoming one's pending reload resumes.
        self.start_reload(window, cx);
        self.open_members = None;
        self.members_hover = (false, false);
        self.channel_people = None;
        self.sync_connect_orgs(cx);
        self.load_join_people(cx);
        self.page = MainPage::Channel;
        self.unreads_fetched.clear();
        self.back.clear();
        self.forward.clear();
        self.pending_dm = None;
        self.set_right_pane(RightPane::None, cx);
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
        // Keep every shared view (Activity, Unreads, Huddles, Directory, Compose, Details,
        // SearchMenu, Files, Tools, Composer, HuddleDock) synced with the active workspace.
        self.sync_all_views_fixtures(fixtures, cx);
        self.sync_rail(cx);
        self.sync_saved(cx);
        self.sync_later_count(cx);
        self.tab = ConversationTab::Messages;
        let channel = self.channel.clone();
        if !channel.as_str().is_empty() {
            self.bridge
                .send(ChannelCommand::OpenConversation(channel.clone()));
            self.load_history(&channel, cx);
        }
        let ws_outbox = self.ws.outbox.clone();
        let ws_offline = self.is_connection_offline();
        self.thread.update(cx, |panel, cx| {
            panel.set_outbox(ws_outbox, cx);
            panel.set_offline(ws_offline, cx);
        });
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
        let active_team =
            (self.bridge.team_id().cloned()).unwrap_or_else(|| self.team().id.clone());
        self.results
            .update(cx, |view, cx| view.set_workspace(active_team, cx));
        let bf_progress = self.ws.backfill_progress.clone();
        self.preferences.update(cx, |prefs, cx| {
            prefs.set_backfill_progress(bf_progress, cx);
        });
        self.sync_search_workspace_sources(cx);
        self.apply_preferences(window, cx);
        self.update_menus(cx);
        self.sync_window_title_in(window);
        cx.notify();
    }

    pub(super) fn open_drafts(&mut self, cx: &mut Context<Self>) {
        self.record_nav(&NavPlace::Drafts, cx);
        self.show_drafts(cx);
    }

    pub(super) fn show_drafts(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.close_full_column_views(cx);
        self.page = MainPage::Drafts;
        self.sidebar.update(cx, |s, cx| {
            s.set_home_page(Some(crate::sidebar::HomePage::Drafts), cx)
        });
        if !self.fixtures.sample {
            self.bridge.send(MessagingCommand::LoadScheduledMessages);
            self.bridge.send(MessagingCommand::FetchCloudDrafts);
            if let Some(read) = self.bridge.recent_sent(self.fixtures.self_user.clone(), 50) {
                let workspace = self.workspace;
                let fixtures = self.fixtures.clone();
                let drafts = self.drafts.downgrade();
                cx.spawn(async move |_, cx| {
                    let stored = cx.background_executor().spawn(read).await;
                    let entries: Vec<Entry> = stored
                        .into_iter()
                        .filter_map(|row| {
                            let text = crate::messages::plain_text(&row.message.text, &*fixtures);
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
        self.page = MainPage::Channel;
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
            self.bridge.send(MessagingCommand::FetchThreads);
        }
        self.close_full_column_views(cx);
        self.page = MainPage::Threads;
        self.sidebar.update(cx, |s, cx| {
            s.set_home_page(Some(crate::sidebar::HomePage::Threads), cx)
        });
        cx.notify();
    }

    /// Offline / error fallback for Threads: derive cards from the local SQLite store only when
    /// `subscriptions.thread.getView` fails.
    pub(super) fn load_threads_store_fallback(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.bridge.read_store_clone() else {
            return;
        };
        let team = self.team().id.clone();
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

    /// Open the Local Catch-Up / Daily Briefing view (`Cmd+Shift+B` / sidebar / Go menu).
    pub(super) fn open_catchup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.record_nav(&NavPlace::CatchUp, cx);
        self.show_catchup(window, cx);
    }

    pub(super) fn show_catchup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.close_full_column_views(cx);
        self.page = MainPage::CatchUp;
        self.sidebar.update(cx, |s, cx| {
            s.set_home_page(Some(crate::sidebar::HomePage::CatchUp), cx)
        });
        self.focus.focus(window, cx);
        self.refresh_catchup(cx);
        cx.notify();
    }

    /// Build the list of workspace store targets for Catch-Up (`This workspace` or `All workspaces`).
    fn catchup_workspace_targets(
        &self,
        all_workspaces: bool,
    ) -> (Vec<slackless_core::catchup::WorkspaceCatchUpTarget>, bool) {
        use slackless_core::catchup::WorkspaceCatchUpTarget;

        let mut targets = Vec::new();
        let total_workspaces = self.slots.len();
        let indices: Vec<usize> = if all_workspaces {
            (0..total_workspaces).collect()
        } else {
            vec![self.workspace]
        };

        if !self.fixtures.sample {
            for idx in indices {
                if let Some(bridge) = self.workspace_bridge(idx)
                    && let (Some(store), Some(tid)) =
                        (bridge.read_store_clone(), bridge.team_id().cloned())
                {
                    let name =
                        Some(self.slots[idx].team.name.clone()).filter(|n| !n.trim().is_empty());
                    targets.push(WorkspaceCatchUpTarget {
                        workspace: tid,
                        workspace_name: name,
                        self_user: bridge.self_user_id().cloned(),
                        store,
                    });
                }
            }
        } else {
            for idx in indices {
                let team = &self.slots[idx].team;
                let fx = self
                    .workspace_fixtures(idx)
                    .unwrap_or_else(|| Arc::new(fixtures::workspace(idx)));
                if let Some(store) = fixture_archive_for(&fx, &team.id) {
                    targets.push(WorkspaceCatchUpTarget {
                        workspace: team.id.clone(),
                        workspace_name: Some(team.name.clone()),
                        self_user: Some(fx.self_user.clone()),
                        store,
                    });
                }
            }
        }
        (targets, total_workspaces > 1)
    }

    /// Recompute the Local Catch-Up briefing off the UI thread against SQLite read-only stores.
    pub(super) fn refresh_catchup(&mut self, cx: &mut Context<Self>) {
        let window = self.catchup.read(cx).window();
        let all_workspaces = self.catchup.read(cx).all_workspaces();
        let (targets, multi_available) = self.catchup_workspace_targets(all_workspaces);
        self.catchup.update(cx, |view, cx| {
            view.set_multi_workspace_available(multi_available, cx);
            view.set_loading(true, cx);
        });
        self.catchup_generation = self.catchup_generation.wrapping_add(1);
        let generation = self.catchup_generation;
        let now_secs = slackless_core::types::time::now_epoch_secs();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    slackless_core::catchup::build_catchup_across_stores(
                        &targets,
                        window,
                        now_secs,
                        slackless_core::catchup::DEFAULT_CATCHUP_LIMIT,
                    )
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                if app.catchup_generation != generation {
                    return;
                }
                match result {
                    Ok(briefing) => {
                        app.catchup
                            .update(cx, |view, cx| view.set_briefing(briefing, cx));
                    }
                    Err(err) => {
                        app.catchup
                            .update(cx, |view, cx| view.set_error(err.to_string(), cx));
                    }
                }
            });
        })
        .detach();
    }

    /// Persist the current timestamp as the Catch-Up visit watermark for the scoped workspace(s)
    /// and refresh the briefing once the SQLite write(s) complete.
    pub(super) fn mark_catchup_visited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let all_workspaces = self.catchup.read(cx).all_workspaces();
        let now_secs = slackless_core::types::time::now_epoch_secs();
        let mut writes = Vec::new();
        if !self.fixtures.sample {
            if let Some(write) = self.bridge.record_catchup_visit(now_secs) {
                writes.push(write);
            }
            if all_workspaces {
                for bg in self.slots.iter().filter_map(|slot| slot.bridge.as_ref()) {
                    if let Some(write) = bg.record_catchup_visit(now_secs) {
                        writes.push(write);
                    }
                }
            }
        }
        notify(
            "Marked caught up",
            "Updated Catch-Up visit timestamp.",
            window,
            cx,
        );
        if writes.is_empty() {
            self.refresh_catchup(cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move {
                    for write in writes {
                        write.await;
                    }
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.refresh_catchup(cx);
            });
        })
        .detach();
    }

    /// Jump from Catch-Up into a channel (switching workspaces if needed) and optionally reveal `ts`.
    pub(super) fn open_catchup_channel(
        &mut self,
        workspace: &slackless_core::types::TeamId,
        channel: ChannelId,
        ts: Option<Ts>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = self.workspace_index_for(workspace)
            && idx != self.workspace
        {
            self.switch_workspace(idx, window, cx);
        }
        self.select(channel, cx);
        if let Some(ts) = ts {
            self.messages.update(cx, |list, cx| list.reveal(&ts, cx));
        }
    }

    /// Jump from Catch-Up into a thread (switching workspaces if needed) and opening the thread panel.
    pub(super) fn open_catchup_thread(
        &mut self,
        workspace: &slackless_core::types::TeamId,
        channel: ChannelId,
        thread_ts: Ts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = self.workspace_index_for(workspace)
            && idx != self.workspace
        {
            self.switch_workspace(idx, window, cx);
        }
        self.select(channel.clone(), cx);
        self.open_thread(channel, thread_ts, cx);
    }

    /// Jump from Catch-Up into a huddle transcript modal (switching workspaces if needed).
    pub(super) fn open_catchup_huddle(
        &mut self,
        workspace: &slackless_core::types::TeamId,
        call_id: String,
        channel: Option<ChannelId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = self.workspace_index_for(workspace)
            && idx != self.workspace
        {
            self.switch_workspace(idx, window, cx);
        }
        let target_channel = channel.unwrap_or_else(|| self.channel.clone());
        self.open_huddle_transcript(call_id, target_channel, None, None, window, cx);
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
        self.page = MainPage::Unreads;
        self.sidebar.update(cx, |s, cx| {
            s.set_home_page(Some(crate::sidebar::HomePage::Unreads), cx)
        });
        self.focus.focus(window, cx);
        self.refresh_unreads(cx);
        cx.notify();
    }

    /// Re-derive the open Unreads view's cards: the view opened, the workspace reloaded, a
    /// message landed, or everything was marked read. Live mode reads the store off the UI
    /// thread one read at a time ([`LiveRefresh`]): a burst of refreshes costs the read in
    /// flight plus one more, and a read a newer refresh overtook never lands.
    pub(super) fn refresh_unreads(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.bridge.read_store_clone() else {
            let order = self.sidebar.read(cx).unread_ids();
            let groups = local_groups(&self.fixtures, &order);
            self.unreads_refresh.overtake();
            let fixtures = self.fixtures.clone();
            self.unreads
                .update(cx, |view, cx| view.show_local(fixtures, groups, cx));
            return;
        };
        let team = self.team().id.clone();
        let Some(generation) = self.unreads_refresh.request(&team) else {
            return;
        };
        let order = self.sidebar.read(cx).unread_ids();
        let targets = store_targets(&self.fixtures, &local_groups(&self.fixtures, &order));
        // Keep already-loaded store groups on screen while refreshing off-thread (never flash
        // empty `local_groups` placeholders).
        let fixtures = self.fixtures.clone();
        self.unreads
            .update(cx, |view, cx| view.begin_live_refresh(fixtures, cx));
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
                let Some(again) = app.unreads_refresh.land(generation) else {
                    return;
                };
                // The view closed while the store was read: its next opening reads again.
                if app.page != MainPage::Unreads {
                    return;
                }
                // Only conversations Slack still counts unread take part: a mark may have
                // raced the store read.
                let keep = app.sidebar.read(cx).unread_ids();
                for group in &groups {
                    if group.messages.is_empty()
                        && keep.contains(&group.channel)
                        && app.unreads_fetched.insert(group.channel.clone())
                    {
                        app.bridge
                            .send(ChannelCommand::FetchLatestHistory(group.channel.clone()));
                    }
                }
                app.unreads
                    .update(cx, |view, cx| view.show_store(groups, &keep, cx));
                if again {
                    app.refresh_unreads(cx);
                }
            })
        })
        .detach();
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
        self.page = MainPage::Directory;
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
        self.page = MainPage::Compose;
        cx.notify();
    }

    /// The gear menu's "Create a channel section": Slack-side in live mode (the section
    /// arrives with the next sidebar reload), local in the bundled sample workspace.
    pub(super) fn create_section(&mut self, name: String, cx: &mut Context<Self>) {
        if self.fixtures.sample {
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.add_section(name, cx));
        } else {
            self.bridge.send(ChannelCommand::CreateSection { name });
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
        self.page = MainPage::Tools;
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.show_nav(NavItem::Agents, cx));
        cx.notify();
    }

    /// Open Slack workspace administration in the default browser.
    pub(super) fn open_admin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let domain = Some(self.team().domain.as_str()).filter(|d| !d.is_empty());
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
        self.page = MainPage::Activity;
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
                .send(MessagingCommand::FetchActivity { with_bodies: true });
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
        self.page = MainPage::Files;
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.show_nav(NavItem::Files, cx));
        let fixtures = self.fixtures.clone();
        self.files_page
            .update(cx, |page, cx| page.open(fixtures, cx));
        cx.notify();
    }
}
