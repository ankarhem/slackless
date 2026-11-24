//! Navigation, workspace switching, deep links, tabs, pop-outs, search, and views.

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

    /// A link in message text was activated. Slack resolves its own targets in-app:
    /// profiles, channels and threads open their panes, and Slack permalinks open the
    /// conversation (or thread) instead of the browser; everything else is a web URL.
    pub(super) fn open_link(
        &mut self,
        target: LinkTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            LinkTarget::Profile(user) => self.open_profile(user, cx),
            LinkTarget::Channel(channel) if self.fixtures.conversation(&channel).is_some() => {
                self.select(channel, cx);
            }
            LinkTarget::Thread { channel, thread_ts }
                if self.fixtures.conversation(&channel).is_some() =>
            {
                self.open_thread(channel, thread_ts, cx);
            }
            LinkTarget::Url(url) => {
                if self.handle_deep_link(url.as_ref(), window, cx) {
                    return;
                }
                if is_workflow_trigger_url(url.as_ref()) {
                    if self.fixtures.sample {
                        notify(
                            "Workflow trigger",
                            format!("Triggered {}", url.as_ref()),
                            window,
                            cx,
                        );
                    } else {
                        self.bridge.send(Command::TripWorkflowTrigger {
                            url: url.to_string(),
                            context: crate::blockkit::serde_json::json!({
                                "location": "link",
                                "channel_id": self.channel.as_str(),
                            }),
                        });
                    }
                    return;
                }
                match parse_permalink(&url) {
                    Some(permalink) if self.fixtures.conversation(&permalink.channel).is_some() => {
                        self.select(permalink.channel.clone(), cx);
                        match permalink.thread_ts {
                            // A reply's link opens its thread scrolled to the reply, as in Slack.
                            Some(thread_ts) => {
                                self.open_thread(permalink.channel.clone(), thread_ts, cx);
                                self.thread
                                    .update(cx, |panel, cx| panel.reveal(permalink.ts, cx));
                            }
                            None => {
                                self.messages
                                    .update(cx, |list, cx| list.reveal(&permalink.ts, cx));
                            }
                        }
                    }
                    _ => self.open_external_url(url.as_ref(), window, cx),
                }
            }
            // An in-app target this workspace does not know: nothing to open.
            LinkTarget::Channel(_) | LinkTarget::Thread { .. } => {}
        }
    }

    /// Open an external browser URL, prompting first when the URL scheme is outside
    /// `Settings.protocol_allowlist` (`F-SHELL-193`) or when `Settings.warn_external` is enabled
    /// and the URL looks suspicious (non-HTTPS, punycode `xn--`, credential `@`, or IP host).
    pub(super) fn open_external_url(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = self.preferences.read(cx).settings();
        let browser_choice = settings.open_links;
        let blocked_scheme = !is_protocol_allowed(url, &settings.protocol_allowlist);
        let warn = blocked_scheme || (settings.warn_external && is_suspicious_url(url));
        if !warn {
            launch_external_url(url, browser_choice, cx);
            return;
        }
        let target = url.to_owned();
        let prompt_msg = if blocked_scheme {
            "This link uses a URL protocol outside your allowed protocol list (Preferences \u{2192} Advanced). Verify the destination before opening it:"
        } else {
            "This link uses an unencrypted or unusual address. Verify the destination before opening it:"
        };
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            let open_target = target.clone();
            dialog.title("Open external link?").w(px(460.)).child(
                v_flex()
                    .gap_3()
                    .child(div().text_sm().text_color(muted).child(prompt_msg))
                    .child(
                        div()
                            .p_2()
                            .rounded_md()
                            .bg(cx.theme().secondary)
                            .text_xs()
                            .child(target.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("ext-url-cancel")
                                    .label("Cancel")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("ext-url-open")
                                    .primary()
                                    .label("Open link")
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        launch_external_url(&open_target, browser_choice, cx);
                                    }),
                            ),
                    ),
            )
        });
    }

    /// Route a `slack://`, `https://app.slack.com/client/...`, or `https://*.slack.com/archives/...`
    /// deep link into the matching workspace, conversation, thread, or user profile (`F-SHELL-182..187`).
    /// Returns `true` when the URL was handled in-app.
    pub fn handle_deep_link(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use slackless_core::platform::{DeepLink, SlackLinkKind, parse_deep_link};
        let Ok(link) = parse_deep_link(url) else {
            return false;
        };
        let switch_team = |this: &mut Self,
                           team_id: Option<&str>,
                           window: &mut Window,
                           cx: &mut Context<Self>| {
            let Some(team_id) = team_id else {
                return;
            };
            let idx = this
                .live_teams
                .iter()
                .position(|t| t.id.as_str() == team_id)
                .or_else(|| {
                    this.fixtures
                        .workspaces
                        .iter()
                        .position(|t| t.id.as_str() == team_id)
                });
            if let Some(ix) = idx {
                if ix != this.workspace {
                    this.switch_workspace(ix, window, cx);
                }
            }
        };
        match link {
            DeepLink::Slack(slack) => match slack.kind {
                SlackLinkKind::Team { team_id } => {
                    switch_team(self, Some(&team_id), window, cx);
                    true
                }
                SlackLinkKind::Channel {
                    channel_id,
                    team_id,
                } => {
                    switch_team(self, team_id.as_deref(), window, cx);
                    let ch = ChannelId::from(channel_id.as_str());
                    if self.fixtures.conversation(&ch).is_some() {
                        self.select(ch, cx);
                        return true;
                    }
                    false
                }
                SlackLinkKind::User { user_id, team_id } => {
                    switch_team(self, team_id.as_deref(), window, cx);
                    let uid = UserId::from(user_id.as_str());
                    self.open_profile(uid, cx);
                    true
                }
                SlackLinkKind::Open { team_id, target_id } => {
                    switch_team(self, team_id.as_deref(), window, cx);
                    if let Some(target) = target_id {
                        if target.starts_with('U') || target.starts_with('W') {
                            self.open_profile(UserId::from(target.as_str()), cx);
                            return true;
                        }
                        let ch = ChannelId::from(target.as_str());
                        if self.fixtures.conversation(&ch).is_some() {
                            self.select(ch, cx);
                            return true;
                        }
                    }
                    team_id.is_some()
                }
            },
            DeepLink::WebClient(web) => {
                switch_team(self, web.team_id.as_deref(), window, cx);
                let Some(channel_id) = web.channel_id else {
                    return web.team_id.is_some();
                };
                let ch = ChannelId::from(channel_id.as_str());
                if self.fixtures.conversation(&ch).is_none() {
                    return false;
                }
                self.select(ch.clone(), cx);
                if let Some(ts_str) = web.thread_ts {
                    let ts = Ts::from(ts_str.as_str());
                    if url.contains("thread_ts=") {
                        self.open_thread(ch, ts.clone(), cx);
                        self.thread.update(cx, |panel, cx| panel.reveal(ts, cx));
                    } else {
                        self.messages.update(cx, |list, cx| list.reveal(&ts, cx));
                    }
                }
                true
            }
            DeepLink::Other { .. } => false,
        }
    }

    /// A tab row entry was clicked: switch the column, load the live channel canvas in the
    /// native reader, or create the missing canvas.
    pub(super) fn activate_tab(&mut self, action: TabAction, cx: &mut Context<Self>) {
        match action {
            TabAction::Switch(target) => self.select_tab(target, cx),
            TabAction::OpenCanvas(file) => self.open_live_canvas(file, cx),
            TabAction::OpenList(file) => self.open_live_list(file, cx),
            TabAction::AddCanvas => {
                self.bridge
                    .send(Command::CreateChannelCanvas(self.channel.clone()));
            }
        }
    }

    /// Open `file_id` natively inside the in-app Canvas reader (`F-COLLAB-125`), fetching its
    /// read-only HTML export (`files.info` -> `url_private`) if not cached yet.
    pub(super) fn open_live_canvas(&mut self, file_id: String, cx: &mut Context<Self>) {
        self.close_full_column_views(cx);
        self.ws.active_canvas = Some(file_id.clone());
        self.tab = ConversationTab::Canvas;
        let needs_load = match self.ws.canvases.get(&file_id) {
            None | Some(Err(_)) => true,
            Some(Ok(_)) => false,
        };
        if needs_load && !self.fixtures.sample {
            self.ws.canvases.remove(&file_id);
            self.bridge.send(Command::LoadCanvas { file_id });
        }
        cx.notify();
    }

    /// Open `file_id` natively inside the read-only Slack Lists viewer, fetching its schema and
    /// rows (`files.info` + `lists.records.list`) if not cached yet.
    pub(super) fn open_live_list(&mut self, file_id: String, cx: &mut Context<Self>) {
        self.close_full_column_views(cx);
        self.ws.active_list = Some(file_id.clone());
        self.list_sort = None;
        self.tab = ConversationTab::List;
        let needs_load = match self.ws.lists.get(&file_id) {
            None | Some(Err(_)) => true,
            Some(Ok(_)) => false,
        };
        if needs_load && !self.fixtures.sample {
            self.ws.lists.remove(&file_id);
            self.bridge.send(Command::LoadList { file_id });
        }
        cx.notify();
    }

    /// Show `target`'s body. The Pins tab toggles the right-hand panel over the messages
    /// column; live mode fetches the tab's data each time it opens.
    pub(super) fn select_tab(&mut self, target: ConversationTab, cx: &mut Context<Self>) {
        if target == ConversationTab::Pins && self.tab == ConversationTab::Pins {
            self.pins_open = false;
            self.tab = ConversationTab::Messages;
            cx.notify();
            return;
        }
        if target == ConversationTab::Pins {
            self.pins_open = true;
            self.close_thread(cx);
            self.profile_open = false;
        }
        self.load_tab_data(&target);
        self.tab = target;
        cx.notify();
    }

    /// Live mode: fetch the data of a tab that just opened - one request per open, never
    /// per channel. A failed earlier load is dropped so the tab shows loading again.
    pub(super) fn load_tab_data(&mut self, tab: &ConversationTab) {
        if self.fixtures.sample {
            return;
        }
        let channel = self.channel.clone();
        let command = match tab {
            ConversationTab::Home => {
                if self.ws.app_homes.get(&channel).is_some_and(Result::is_err) {
                    self.ws.app_homes.remove(&channel);
                }
                Command::LoadAppHome(channel)
            }
            ConversationTab::Pins => {
                forget_failure(&mut self.ws.pins, &channel);
                Command::LoadPins(channel)
            }
            ConversationTab::Files => {
                forget_failure(&mut self.ws.files, &channel);
                Command::LoadFiles(FilesFilter::channel(channel))
            }
            ConversationTab::Folder { .. } => {
                forget_failure(&mut self.ws.bookmarks, &channel);
                Command::LoadBookmarks(channel)
            }
            ConversationTab::Messages | ConversationTab::Canvas | ConversationTab::List => {
                return;
            }
        };
        self.bridge.send(command);
    }

    /// The live channel's top-level bookmarks for the bar: links open their URL, folders
    /// open their tab; entries without a target are left out. Empty until the channel's
    /// `bookmarks.list` has been fetched (lazy).
    pub(super) fn live_bookmarks(&self) -> Vec<bookmarks::Bookmark> {
        let Some(Ok(list)) = self.ws.bookmarks.get(&self.channel) else {
            return Vec::new();
        };
        list.iter()
            .filter(|bookmark| bookmark.parent_id.as_deref().unwrap_or_default().is_empty())
            .filter_map(|bookmark| {
                let title = bookmark.title.clone();
                if bookmark.kind.as_deref() == Some("folder") {
                    return Some(bookmarks::Bookmark::folder(title, bookmark.id.clone()));
                }
                let link = bookmark.link.as_deref().filter(|link| !link.is_empty())?;
                Some(bookmarks::Bookmark::link(title, link.to_owned()))
            })
            .collect()
    }

    /// A folder bookmark in the bar: show its bookmarks under the conversation's tab for the
    /// folder, or under the folder's own title when the row has no such tab.
    pub(super) fn open_folder_tab(&mut self, folder_id: &str, cx: &mut Context<Self>) {
        let row_label = self
            .fixtures
            .conversation(&self.channel)
            .and_then(|conversation| {
                tabs::derive(conversation, self.fixtures.sample)
                    .into_iter()
                    .find_map(|spec| match spec.action {
                        TabAction::Switch(ConversationTab::Folder { label, bookmark_id })
                            if bookmark_id == folder_id =>
                        {
                            Some(label)
                        }
                        _ => None,
                    })
            });
        let title = || {
            let list = self.ws.bookmarks.get(&self.channel)?.as_ref().ok()?;
            let folder = list.iter().find(|bookmark| bookmark.id == folder_id)?;
            Some(SharedString::from(folder.title.clone()))
        };
        let label = row_label
            .or_else(title)
            .unwrap_or_else(|| "Bookmarks".into());
        let bookmark_id = folder_id.to_owned();
        self.select_tab(ConversationTab::Folder { label, bookmark_id }, cx);
    }

    /// The canvas document's permalink in Slack's web client.
    pub(super) fn canvas_link(&self, canvas_id: &str) -> String {
        match self.fixtures.workspaces.first() {
            Some(team) if !team.domain.is_empty() => format!(
                "https://{}.slack.com/docs/{}/{}",
                team.domain, team.id, canvas_id
            ),
            Some(team) => {
                format!("https://app.slack.com/client/{}/{}", team.id, self.channel)
            }
            None => format!("https://app.slack.com/client/{}", self.channel),
        }
    }

    /// The "+" menu's Canvas item: open the conversation's canvas, creating it first when
    /// it has none (the created canvas arrives as `Event::ChannelCanvasCreated` and opens
    /// then).
    pub(super) fn open_or_add_canvas(&mut self, cx: &mut Context<Self>) {
        let canvas = self
            .fixtures
            .conversation(&self.channel)
            .and_then(|conversation| conversation.properties.as_ref())
            .and_then(|properties| properties.canvas.as_ref())
            .and_then(|canvas| canvas.file_id.as_ref())
            .map(|file| file.to_string());
        match canvas {
            Some(file) => cx.open_url(&self.canvas_link(&file)),
            None => {
                self.bridge
                    .send(Command::CreateChannelCanvas(self.channel.clone()));
            }
        }
    }

    /// The add-link dialog's Add: live mode sends `bookmarks.add`, fixture mode keeps the
    /// bookmark in memory with the fixture bar.
    pub(super) fn add_bookmark(&mut self, title: String, url: String, cx: &mut Context<Self>) {
        if self.fixtures.sample {
            self.fixture_bookmarks
                .entry(self.channel.clone())
                .or_default()
                .push(bookmarks::Bookmark::link(title, url));
            cx.notify();
        } else {
            self.bridge.send(Command::AddBookmark {
                channel: self.channel.clone(),
                title,
                url,
            });
        }
    }

    /// Slack's focus mode (the rail's moon): on with Slack's default "Don't turn off", or off.
    /// Syncs DND snooze & automatic focus status with Slack in live mode (`F-SHELL-036`).
    pub(super) fn toggle_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let auto_status = self.preferences.read(cx).settings().focus_auto_status;
        if self.self_state.focused() {
            self.self_state.end_focus();
            if !self.fixtures.sample {
                self.bridge.send(Command::EndDndSnooze);
                self.send_status_to_slack(self.self_state.status.as_ref());
            }
        } else {
            self.self_state
                .start_focus_with_status(FOCUS_UNTIL_OFF, auto_status);
            if !self.fixtures.sample {
                // Default 8-hour DND snooze for indefinite focus mode.
                self.bridge.send(Command::SetDndSnooze { minutes: 480 });
                if auto_status {
                    self.send_status_to_slack(self.self_state.status.as_ref());
                }
            }
        }
        self.apply_preferences(window, cx);
    }

    /// End a timed focus mode or expired custom status once its Duration ran out, and activate
    /// any scheduled statuses whose start time has arrived (checked on the reminder poll).
    pub(super) fn expire_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.self_state.focus_until.is_some() && !self.self_state.focused() {
            self.self_state.end_focus();
            self.apply_preferences(window, cx);
        }
        if self.self_state.expire_status() {
            self.push_self_state(cx);
        }
        if let Some(activated) = self.self_state.activate_due_scheduled() {
            if !self.fixtures.sample {
                self.send_status_to_slack(Some(&activated));
            }
            self.push_self_state(cx);
            notify(
                "Scheduled status active",
                format!("{} {}", activated.glyph(), activated.text),
                window,
                cx,
            );
        }
    }

    /// Opt-in native macOS Calendar busy-event status sync (`F-SHELL-024`): probes Calendar.app
    /// once per minute off the UI thread when `Settings.calendar_status_sync` is enabled, setting
    /// `:spiral_calendar_pad:` `"In a meeting"` while a non-all-day event is active and clearing
    /// it once the meeting ends (never overwriting a manual user status or active Focus mode).
    pub(super) fn poll_calendar_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = self.preferences.read(cx).settings().calendar_status_sync;
        if !enabled {
            if self.calendar_status_active {
                self.calendar_status_active = false;
                if self
                    .self_state
                    .status
                    .as_ref()
                    .is_some_and(|s| s.text == "In a meeting" && s.emoji == "spiral_calendar_pad")
                {
                    self.self_state.set_status(None);
                    if !self.fixtures.sample {
                        self.send_status_to_slack(None);
                    }
                    self.push_self_state(cx);
                }
            }
            return;
        }
        let tick = self.calendar_sync_ticks;
        self.calendar_sync_ticks = self.calendar_sync_ticks.wrapping_add(1);
        // Probe immediately on first enabled tick, then once every 60s (6 × 10s).
        if tick % 6 != 0 || self.self_state.focused() {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            cx.spawn_in(window, async move |this, cx| {
                let busy = cx
                    .background_executor()
                    .spawn(async move { probe_macos_calendar_busy() })
                    .await;
                let Some(is_busy) = busy else {
                    return;
                };
                let _ = this.update(cx, |app, cx| {
                    if app.self_state.focused()
                        || !app.preferences.read(cx).settings().calendar_status_sync
                    {
                        return;
                    }
                    if is_busy {
                        if app.self_state.status.is_none() {
                            let status = crate::status::UserStatus {
                                emoji: "spiral_calendar_pad".to_owned(),
                                text: "In a meeting".to_owned(),
                                clear_after: 0,
                                pause: false,
                                expiration: None,
                            };
                            app.self_state.set_status(Some(status.clone()));
                            app.calendar_status_active = true;
                            if !app.fixtures.sample {
                                app.send_status_to_slack(Some(&status));
                            }
                            app.push_self_state(cx);
                        }
                    } else if app.calendar_status_active {
                        app.calendar_status_active = false;
                        if app.self_state.status.as_ref().is_some_and(|s| {
                            s.text == "In a meeting" && s.emoji == "spiral_calendar_pad"
                        }) {
                            app.self_state.set_status(None);
                            if !app.fixtures.sample {
                                app.send_status_to_slack(None);
                            }
                            app.push_self_state(cx);
                        }
                    }
                });
            })
            .detach();
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (window, cx);
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

    /// Callback for popped-out Activity / Saved windows: brings the main window to the front
    /// and navigates to the clicked conversation, message, or thread.
    pub(super) fn popout_jump_handler(
        &self,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> crate::popout::JumpHandler {
        let app = cx.weak_entity();
        let main_win = window.window_handle();
        Rc::new(move |channel, ts, is_thread, cx| {
            cx.activate(true);
            let _ = main_win.update(cx, |_, window, cx| {
                window.activate_window();
                let _ = app.update(cx, |app, cx| {
                    app.select(channel.clone(), cx);
                    if let Some(ts) = ts.clone() {
                        if is_thread {
                            app.open_thread(channel.clone(), ts, cx);
                        } else {
                            app.messages.update(cx, |list, cx| list.reveal(&ts, cx));
                        }
                    }
                });
            });
        })
    }

    /// Pop out `channel` into a separate native GPUI window (`F-CORE-010`, `F-CORE-026`).
    pub(super) fn pop_out_conversation(
        &self,
        channel: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self.label(&channel);
        let on_jump = self.popout_jump_handler(window, cx);
        let display_id = window.display(cx).map(|d| d.id());
        crate::popout::open_popout(
            crate::popout::PopoutKind::Conversation {
                channel,
                source: self.messages.downgrade(),
                on_jump,
            },
            title,
            self.fixtures.clone(),
            self.bridge.store().cloned(),
            self.bridge.team_id().cloned(),
            self.bridge.forwarder(),
            Some((self.later.downgrade(), self.workspace)),
            display_id,
            cx,
        );
    }

    /// Pop out a thread into a separate native GPUI window (`F-CORE-109`).
    pub(super) fn pop_out_thread(
        &self,
        channel: ChannelId,
        thread_ts: Ts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = format!("Thread in {}", self.label(&channel));
        let on_jump = self.popout_jump_handler(window, cx);
        let display_id = window.display(cx).map(|d| d.id());
        crate::popout::open_popout(
            crate::popout::PopoutKind::Thread {
                channel,
                thread_ts,
                source: self.messages.downgrade(),
                on_jump,
            },
            title,
            self.fixtures.clone(),
            self.bridge.store().cloned(),
            self.bridge.team_id().cloned(),
            self.bridge.forwarder(),
            Some((self.later.downgrade(), self.workspace)),
            display_id,
            cx,
        );
    }

    /// Pop out the live Activity feed into a separate native GPUI window (`F-SHELL-149`).
    pub(super) fn pop_out_activity(&self, window: &mut Window, cx: &mut Context<Self>) {
        let on_jump = self.popout_jump_handler(window, cx);
        let display_id = window.display(cx).map(|d| d.id());
        crate::popout::open_popout(
            crate::popout::PopoutKind::Activity {
                source: self.activity.downgrade(),
                on_jump,
            },
            "Activity".to_owned(),
            self.fixtures.clone(),
            self.bridge.store().cloned(),
            self.bridge.team_id().cloned(),
            self.bridge.forwarder(),
            None,
            display_id,
            cx,
        );
    }

    /// Pop out the live Saved / Later view into a separate native GPUI window (`F-SHELL-149`).
    pub(super) fn pop_out_later(&self, window: &mut Window, cx: &mut Context<Self>) {
        let on_jump = self.popout_jump_handler(window, cx);
        let display_id = window.display(cx).map(|d| d.id());
        crate::popout::open_popout(
            crate::popout::PopoutKind::Later {
                source: self.later.downgrade(),
                on_jump,
            },
            "Saved".to_owned(),
            self.fixtures.clone(),
            self.bridge.store().cloned(),
            self.bridge.team_id().cloned(),
            self.bridge.forwarder(),
            None,
            display_id,
            cx,
        );
    }

    /// Pop out a Slack Canvas into its own native window (`F-COLLAB-128`, `F-SHELL-148`).
    pub(super) fn pop_out_canvas(
        &self,
        doc: crate::canvas::CanvasDoc,
        file_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = doc.title.clone();
        let open_browser_url = file_id.as_deref().map(|id| self.canvas_link(id));
        let on_jump = self.popout_jump_handler(window, cx);
        let display_id = window.display(cx).map(|d| d.id());
        crate::popout::open_popout(
            crate::popout::PopoutKind::Canvas {
                doc,
                file_id,
                open_browser_url,
                on_jump,
            },
            title,
            self.fixtures.clone(),
            self.bridge.store().cloned(),
            self.bridge.team_id().cloned(),
            self.bridge.forwarder(),
            None,
            display_id,
            cx,
        );
    }

    /// Pop out the live Files browser into a separate native GPUI window (`F-COLLAB-127`, `F-SHELL-150`).
    pub(super) fn pop_out_files(&self, window: &mut Window, cx: &mut Context<Self>) {
        let on_jump = self.popout_jump_handler(window, cx);
        let display_id = window.display(cx).map(|d| d.id());
        crate::popout::open_popout(
            crate::popout::PopoutKind::Files {
                source: self.files_page.downgrade(),
                on_jump,
            },
            "Files".to_owned(),
            self.fixtures.clone(),
            self.bridge.store().cloned(),
            self.bridge.team_id().cloned(),
            self.bridge.forwarder(),
            None,
            display_id,
            cx,
        );
    }

    /// Activate the `index`-th tab in the current conversation's header (`Ctrl+1..9`, `F-SHELL-280`).
    pub(super) fn select_conversation_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(conv) = self.fixtures.conversation(&self.channel) else {
            return;
        };
        let specs = self.conversation_tab_specs(conv);
        if let Some(spec) = specs.into_iter().nth(index) {
            self.activate_tab(spec.action, cx);
        }
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

    /// Slack's huddle link for the open conversation (its web client runs the call).
    pub(super) fn huddle_link(&self) -> String {
        let team = self
            .fixtures
            .workspaces
            .first()
            .map(|team| team.id.to_string())
            .unwrap_or_default();
        format!("https://app.slack.com/huddle/{team}/{}", self.channel)
    }

    /// The header's huddle button: toggles the sample huddle bar/dock in sample mode, or starts /
    /// joins a native Chime audio huddle in live workspaces (with a confirmation warning on
    /// channels with 150+ members, `F-COLLAB-055`).
    pub(super) fn start_huddle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let has_fixture_bar = self
            .huddle
            .as_ref()
            .is_some_and(|(channel, _)| *channel == self.channel);
        if has_fixture_bar && self.fixtures.sample {
            self.huddle_open = !self.huddle_open;
            if self.huddle_open {
                let settings = self.preferences.read(cx).settings().clone();
                let snap = slackless_core::huddle::HuddleSessionSnapshot {
                    phase: slackless_core::huddle::HuddlePhase::Connected,
                    channel: self.channel.clone(),
                    call_id: "R0SAMPLE".to_owned(),
                    muted: settings.huddle_join_muted,
                    started_at_secs: crate::drafts::now_secs(),
                    huddle_link: Some(self.huddle_link()),
                    input_device: settings.huddle_input_device,
                    output_device: settings.huddle_output_device,
                    aec: settings.huddle_aec,
                    ns: settings.huddle_ns,
                    agc: settings.huddle_agc,
                    ..Default::default()
                };
                self.mount_huddle_dock(snap, window, cx);
            } else {
                self.clear_huddle_dock(cx);
            }
            cx.notify();
            return;
        }
        // Already in a huddle in this channel: pop out the huddle window for quick access.
        if self
            .huddle_session
            .as_ref()
            .is_some_and(|s| s.channel == self.channel)
        {
            self.pop_out_huddle(window, cx);
            return;
        }
        let member_count = self
            .ws
            .member_counts
            .get(&self.channel)
            .copied()
            .or_else(|| {
                self.fixtures
                    .conversation(&self.channel)
                    .and_then(|c| c.num_members)
            })
            .unwrap_or(0);
        let is_channel = self.fixtures.conversation(&self.channel).is_some_and(|c| {
            matches!(
                c.kind(),
                ConversationKind::PublicChannel | ConversationKind::PrivateChannel
            )
        });
        let target_channel = self.channel.clone();
        if is_channel && member_count >= 150 {
            let label = self.label(&self.channel);
            let this = cx.weak_entity();
            window.open_dialog(cx, move |dialog, _, cx| {
                let muted = cx.theme().muted_foreground;
                let this = this.clone();
                let ch = target_channel.clone();
                dialog
                    .title(format!("Start a huddle in {label}?"))
                    .w(px(440.))
                    .child(
                        v_flex()
                            .gap_4()
                            .child(div().text_color(muted).child(format!(
                                "This channel has {member_count} members. Starting a huddle may notify everyone in {label}."
                            )))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_end()
                                    .child(Button::new("huddle-warn-cancel").label("Cancel").on_click(
                                        |_, window, cx| window.close_dialog(cx),
                                    ))
                                    .child(
                                        Button::new("huddle-warn-go")
                                            .primary()
                                            .label("Start huddle")
                                            .on_click(move |_, window, cx| {
                                                window.close_dialog(cx);
                                                let _ = this.update(cx, |app, cx| {
                                                    app.join_huddle_in(ch.clone(), window, cx);
                                                });
                                            }),
                                    ),
                            ),
                    )
            });
        } else {
            self.join_huddle_in(target_channel, window, cx);
        }
    }

    /// Join or start a native Chime audio huddle in `channel`.
    pub(super) fn join_huddle_in(
        &mut self,
        channel: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_huddle_invite(cx);
        let settings = self.preferences.read(cx).settings().clone();
        let audio = slackless_core::huddle::HuddleAudioConfig {
            muted: settings.huddle_join_muted,
            input_device: settings.huddle_input_device.clone(),
            output_device: settings.huddle_output_device.clone(),
            camera_device: settings.huddle_camera_device,
            blur_background: settings.huddle_blur_background,
            aec: settings.huddle_aec,
            ns: settings.huddle_ns,
            agc: settings.huddle_agc,
        };
        let initial = slackless_core::huddle::HuddleSessionSnapshot {
            phase: if self.fixtures.sample {
                slackless_core::huddle::HuddlePhase::Connected
            } else {
                slackless_core::huddle::HuddlePhase::Joining
            },
            channel: channel.clone(),
            muted: audio.muted,
            blur_background: audio.blur_background,
            captions_enabled: settings.huddle_auto_captions,
            started_at_secs: crate::drafts::now_secs(),
            input_device: audio.input_device.clone(),
            output_device: audio.output_device.clone(),
            aec: audio.aec,
            ns: audio.ns,
            agc: audio.agc,
            ..Default::default()
        };
        self.mount_huddle_dock(initial, window, cx);
        if self.fixtures.sample {
            self.play_huddle_chime(slackless_core::huddle::HuddleChime::Join, cx);
        } else {
            self.bridge.send(Command::JoinHuddle {
                channel: channel.clone(),
                audio,
            });
        }
        if settings.huddle_auto_open_window {
            self.pop_out_huddle(window, cx);
        }
        cx.notify();
    }

    pub(super) fn mount_huddle_dock(
        &mut self,
        snap: slackless_core::huddle::HuddleSessionSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let participants = self
            .ws
            .active_huddles
            .get(&snap.channel)
            .cloned()
            .or_else(|| {
                self.fixtures.sample.then(|| {
                    self.fixtures
                        .users
                        .iter()
                        .map(|u| u.id.clone())
                        .filter(|id| *id != self.fixtures.self_user)
                        .take(3)
                        .collect()
                })
            })
            .unwrap_or_default();
        self.huddle_session = Some(snap.clone());
        let dock = crate::huddle_dock::HuddleDock::new(
            self.fixtures.clone(),
            snap.clone(),
            participants,
            cx,
        );
        let sub = cx.subscribe_in(
            &dock,
            window,
            |this: &mut App, _, event: &crate::huddle_dock::HuddleDockEvent, window, cx| match event
            {
                crate::huddle_dock::HuddleDockEvent::ToggleMute => this.toggle_huddle_mute(cx),
                crate::huddle_dock::HuddleDockEvent::ToggleCamera => this.toggle_huddle_camera(cx),
                crate::huddle_dock::HuddleDockEvent::ToggleScreenShare => {
                    this.toggle_huddle_screen_share(window, cx);
                }
                crate::huddle_dock::HuddleDockEvent::Leave => this.leave_huddle(cx),
                crate::huddle_dock::HuddleDockEvent::PopOut => this.pop_out_huddle(window, cx),
                crate::huddle_dock::HuddleDockEvent::JumpToChannel(ch) => {
                    this.select(ch.clone(), cx);
                }
                crate::huddle_dock::HuddleDockEvent::OpenThread => this.toggle_huddle_thread(cx),
                crate::huddle_dock::HuddleDockEvent::OpenCanvas(fid) => {
                    this.open_huddle_canvas(fid.clone(), cx);
                }
                crate::huddle_dock::HuddleDockEvent::CopyLink(ch) => {
                    this.copy_huddle_link(ch.clone(), window, cx);
                }
                crate::huddle_dock::HuddleDockEvent::OpenBrowser(url) => cx.open_url(url),
                crate::huddle_dock::HuddleDockEvent::RetryJoin(ch) => {
                    this.join_huddle_in(ch.clone(), window, cx);
                }
                crate::huddle_dock::HuddleDockEvent::OpenAudioSettings => {
                    this.open_preferences(Some(Section::AudioVideo), window, cx);
                }
                crate::huddle_dock::HuddleDockEvent::SwitchInputDevice(input) => {
                    let output = this
                        .huddle_session
                        .as_ref()
                        .and_then(|s| s.output_device.clone());
                    this.bridge.send(Command::SetHuddleAudioDevices {
                        input: input.clone(),
                        output,
                    });
                }
                crate::huddle_dock::HuddleDockEvent::SwitchOutputDevice(output) => {
                    let input = this
                        .huddle_session
                        .as_ref()
                        .and_then(|s| s.input_device.clone());
                    this.bridge.send(Command::SetHuddleAudioDevices {
                        input,
                        output: output.clone(),
                    });
                }
            },
        );
        self.huddle_dock = Some(dock);
        self._huddle_dock_sub = Some(sub);
        self.sidebar
            .update(cx, |s, cx| s.set_self_in_huddle(true, cx));
        if let Some((ch, bar)) = &self.huddle {
            if *ch == snap.channel {
                bar.update(cx, |b, cx| b.update_session(Some(snap), cx));
            }
        }
    }

    pub(super) fn play_huddle_chime(
        &self,
        chime: slackless_core::huddle::HuddleChime,
        cx: &gpui_kit::gpui::App,
    ) {
        let settings = self.preferences.read(cx).settings();
        if settings.mute_sounds || !settings.huddle_sounds {
            return;
        }
        let output = settings.huddle_output_device.clone();
        std::thread::spawn(move || {
            let _ = slackless_core::huddle::play_huddle_chime(chime, output.as_deref());
        });
    }

    /// Arm Official Slack's 60-second solo-huddle lounge music timer (`huddles_play_music_timeout`).
    /// Starts Smooth Jazz (`waiting_muzak-9187b04.mp3`) if the user stays alone for 60 seconds,
    /// and cancels automatically if another participant joins or the user leaves the huddle.
    pub(super) fn arm_huddle_alone_music(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        if self.fixtures.sample || !self.preferences.read(cx).settings().huddle_music_when_alone {
            self.huddle_alone_music_task = None;
            return;
        }
        if self.huddle_alone_music_task.is_some() {
            return;
        }
        self.huddle_alone_music_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(60))
                .await;
            let _ = this.update(cx, |app, cx| {
                app.huddle_alone_music_task = None;
                let settings = app.preferences.read(cx).settings();
                if !settings.huddle_music_when_alone || settings.mute_sounds {
                    return;
                }
                let Some(snap) = app.huddle_session.as_ref() else {
                    return;
                };
                if snap.channel != channel
                    || snap.phase != slackless_core::huddle::HuddlePhase::Connected
                    || snap.jukebox_song.is_some()
                {
                    return;
                }
                let peer_count = app.ws.active_huddles.get(&channel).map_or(0, Vec::len);
                if peer_count <= 1 {
                    app.bridge
                        .send(Command::SetHuddleJukebox(Some("jazz".to_owned())));
                }
            });
        }));
    }

    pub(super) fn clear_huddle_dock(&mut self, cx: &mut Context<Self>) {
        self.huddle_alone_music_task = None;
        if let Some(dock) = &self.huddle_dock {
            dock.update(cx, |d, cx| d.clear_video_frames(cx));
        }
        self.huddle_session = None;
        self.huddle_dock = None;
        self._huddle_dock_sub = None;
        self.sidebar
            .update(cx, |s, cx| s.set_self_in_huddle(false, cx));
        if let Some((_, bar)) = &self.huddle {
            bar.update(cx, |b, cx| b.update_session(None, cx));
        }
    }

    pub(super) fn leave_huddle(&mut self, cx: &mut Context<Self>) {
        let was_active = self.huddle_session.is_some() || self.huddle_open;
        self.huddle_open = false;
        self.clear_huddle_dock(cx);
        if was_active {
            self.play_huddle_chime(slackless_core::huddle::HuddleChime::Leave, cx);
        }
        if !self.fixtures.sample {
            self.bridge.send(Command::LeaveHuddle);
        }
        cx.notify();
    }

    pub(super) fn toggle_huddle_mute(&mut self, cx: &mut Context<Self>) {
        let Some(snap) = self.huddle_session.as_mut() else {
            return;
        };
        let next = !snap.muted;
        snap.muted = next;
        if let Some(dock) = &self.huddle_dock {
            dock.update(cx, |d, cx| d.set_muted_local(next, cx));
        }
        if let Some((ch, bar)) = &self.huddle {
            if *ch == snap.channel {
                let s = snap.clone();
                bar.update(cx, |b, cx| b.update_session(Some(s), cx));
            }
        }
        if !self.fixtures.sample {
            self.bridge.send(Command::SetHuddleMuted(next));
        }
        cx.notify();
    }

    pub(super) fn toggle_huddle_camera(&mut self, cx: &mut Context<Self>) {
        let settings = self.preferences.read(cx).settings().clone();
        let Some(snap) = self.huddle_session.as_mut() else {
            return;
        };
        let next = !snap.video;
        snap.video = next;
        snap.blur_background = settings.huddle_blur_background;
        if let Some(dock) = &self.huddle_dock {
            dock.update(cx, |d, cx| d.set_camera_local(next, cx));
        }
        if let Some((ch, bar)) = &self.huddle {
            if *ch == snap.channel {
                let s = snap.clone();
                bar.update(cx, |b, cx| b.update_session(Some(s), cx));
            }
        }
        if !self.fixtures.sample {
            self.bridge.send(Command::SetHuddleCamera {
                enabled: next,
                device_index: settings.huddle_camera_device,
                blur: settings.huddle_blur_background,
            });
        }
        cx.notify();
    }

    pub(super) fn toggle_huddle_screen_share(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_sharing = self.huddle_session.as_ref().is_some_and(|s| s.sharing);
        if is_sharing {
            if let Some(snap) = self.huddle_session.as_mut() {
                snap.sharing = false;
                snap.share_source_id = None;
            }
            if let Some(dock) = &self.huddle_dock {
                dock.update(cx, |d, cx| d.set_sharing_local(None, cx));
            }
            if !self.fixtures.sample {
                self.bridge.send(Command::SetHuddleScreenShare {
                    source_id: None,
                    share_audio: false,
                });
            }
            cx.notify();
        } else {
            // Open the full Huddle Window so the user can pick a monitor or window to share.
            self.pop_out_huddle(window, cx);
        }
    }

    pub(super) fn copy_huddle_link(
        &mut self,
        channel: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.fixtures.sample {
            let url = self.huddle_link();
            cx.write_to_clipboard(ClipboardItem::new_string(url));
            notify("Huddle link copied", String::new(), window, cx);
        } else {
            self.bridge.send(Command::CopyHuddleLink { channel });
        }
    }

    pub(super) fn open_huddle_canvas(&mut self, file_id: String, cx: &mut Context<Self>) {
        self.ws.active_canvas = Some(file_id.clone());
        self.tab = ConversationTab::Canvas;
        if !self.fixtures.sample {
            self.bridge.send(Command::LoadCanvas { file_id });
        }
        cx.notify();
    }

    pub(super) fn pop_out_huddle(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock) = &self.huddle_dock else {
            return;
        };
        let ch = dock.read(cx).session().channel.clone();
        let title = format!("Huddle — {}", self.label(&ch));
        let on_jump = self.popout_jump_handler(window, cx);
        let this = cx.weak_entity();
        let on_open_canvas: crate::popout::CanvasOpenHandler = Rc::new(move |fid, cx| {
            let _ = this.update(cx, |app, cx| app.open_huddle_canvas(fid, cx));
        });
        let display_id = window.display(cx).map(|d| d.id());
        crate::popout::open_popout(
            crate::popout::PopoutKind::Huddle {
                source: dock.downgrade(),
                on_jump,
                on_open_canvas,
            },
            title,
            self.fixtures.clone(),
            self.bridge.store().cloned(),
            self.bridge.team_id().cloned(),
            self.bridge.forwarder(),
            None,
            display_id,
            cx,
        );
    }

    pub(super) fn show_huddle_invite(
        &mut self,
        invite: crate::huddle_invite::PendingHuddleInvite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let call_id = invite.call_id.clone();
        let channel = invite.channel.clone();
        let banner =
            crate::huddle_invite::HuddleInviteBanner::new(self.fixtures.clone(), invite, cx);
        let decl_ch = channel.clone();
        let decl_call = call_id.clone();
        let sub = cx.subscribe_in(
            &banner,
            window,
            move |this: &mut App,
                  _,
                  event: &crate::huddle_invite::HuddleInviteEvent,
                  window,
                  cx| {
                match event {
                    crate::huddle_invite::HuddleInviteEvent::Accept(ch) => {
                        this.dismiss_huddle_invite(cx);
                        this.select(ch.clone(), cx);
                        this.join_huddle_in(ch.clone(), window, cx);
                    }
                    crate::huddle_invite::HuddleInviteEvent::Decline => {
                        this.dismiss_huddle_invite(cx);
                        if !this.fixtures.sample {
                            this.bridge.send(Command::DeclineHuddleInvite {
                                channel: decl_ch.clone(),
                                call_id: decl_call.clone(),
                            });
                        }
                    }
                }
            },
        );
        self.huddle_invite = Some(banner);
        self._huddle_invite_sub = Some(sub);
        // Play initial ringtone + repeat every 3 seconds until accepted, declined, or expired.
        self.play_huddle_chime(slackless_core::huddle::HuddleChime::InviteRing, cx);
        let ring_ch = channel.clone();
        let ring_call = call_id.clone();
        cx.spawn(async move |this, cx| {
            for _ in 0..14 {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let still_ringing = this
                    .update(cx, |app, cx| {
                        let same = app.huddle_invite.as_ref().is_some_and(|b| {
                            let inv = b.read(cx).invite();
                            inv.channel == ring_ch && inv.call_id == ring_call
                        });
                        if same {
                            app.play_huddle_chime(
                                slackless_core::huddle::HuddleChime::InviteRing,
                                cx,
                            );
                        }
                        same
                    })
                    .unwrap_or(false);
                if !still_ringing {
                    break;
                }
            }
        })
        .detach();
        // Auto-dismiss unanswered invite after 45 seconds.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(45))
                .await;
            let _ = this.update(cx, |app, cx| {
                let still_same = app.huddle_invite.as_ref().is_some_and(|b| {
                    let inv = b.read(cx).invite();
                    inv.channel == channel && inv.call_id == call_id
                });
                if still_same {
                    app.dismiss_huddle_invite(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn dismiss_huddle_invite(&mut self, cx: &mut Context<Self>) {
        if self.huddle_invite.take().is_some() {
            self._huddle_invite_sub = None;
            cx.notify();
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
        self.recent.retain(|c| *c != channel);
        if let Some(write) = self.bridge.remember_recent_places(&self.recent) {
            cx.background_executor().spawn(write).detach();
        }
        self.update_menus(cx);
        self.reload_live(window, cx);
        if self.channel == channel {
            let next = self
                .back
                .pop()
                .unwrap_or_else(|| NavPlace::Channel(first_conversation(&self.fixtures)));
            self.go_to_place(next, window, cx);
        }
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

    /// The Huddles page: the bundled sample huddle, or the stored `huddle_thread` trails live.
    pub(super) fn open_huddles(&mut self, cx: &mut Context<Self>) {
        self.record_nav(&NavPlace::Huddles, cx);
        self.show_huddles(cx);
    }

    pub(super) fn show_huddles(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        let fixtures = self.fixtures.clone();
        if let Some(store) = self.bridge.store().cloned() {
            // Live mode: keep cached rows visible while refreshing from SQLite.
            self.huddles
                .update(cx, |view, cx| view.begin_live_load(fixtures, cx));
            let team = self.fixtures.workspaces[self.workspace].id.clone();
            cx.spawn(async move |this, cx| {
                let huddles = cx
                    .background_executor()
                    .spawn(async move {
                        let Ok(store) = store.lock() else {
                            return Vec::new();
                        };
                        huddles::store_huddles(&store, &team)
                    })
                    .await;
                this.update(cx, |app, cx| {
                    app.huddles
                        .update(cx, |view, cx| view.set_huddles(huddles, cx));
                })
            })
            .detach();
        } else {
            self.huddles
                .update(cx, |view, cx| view.show_local(&fixtures, cx));
        }
        self.close_full_column_views(cx);
        self.huddles_open = true;
        self.sidebar.update(cx, |s, cx| {
            s.set_home_page(Some(crate::sidebar::HomePage::Huddles), cx)
        });
        cx.notify();
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

    pub(super) fn huddle_here(&self) -> bool {
        self.huddle
            .as_ref()
            .is_some_and(|(channel, _)| *channel == self.channel)
            || self.ws.active_huddles.contains_key(&self.channel)
    }
}

/// Open `url` in the browser selected in Preferences -> Advanced (`0 = Default`, `1 = Safari`,
/// `2 = Firefox`). Falls back to the system default handler on non-macOS or launch errors.
fn launch_external_url(url: &str, browser_choice: usize, cx: &mut gpui::App) {
    #[cfg(target_os = "macos")]
    {
        let app_name = match browser_choice {
            1 => Some("Safari"),
            2 => Some("Firefox"),
            _ => None,
        };
        if let Some(app_name) = app_name {
            if std::process::Command::new("open")
                .args(["-a", app_name, url])
                .spawn()
                .is_ok()
            {
                return;
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = browser_choice;
    cx.open_url(url);
}

/// Whether `url`'s scheme is permitted by the comma-separated `allowlist` (`F-SHELL-193`).
pub(super) fn is_protocol_allowed(url: &str, allowlist: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    let scheme = scheme.trim().to_ascii_lowercase();
    if scheme.is_empty() {
        return false;
    }
    allowlist
        .split([',', ' ', ';'])
        .map(|s| s.trim().trim_end_matches(':').to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .any(|allowed| allowed == scheme)
}

/// Whether `url` is a Slack Workflow Builder link trigger (`https://slack.com/shortcuts/Ft...`,
/// `F-COLLAB-319`, `F-COLLAB-376`).
pub(super) fn is_workflow_trigger_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let Some((host, path)) = rest.split_once('/') else {
        return false;
    };
    (host == "slack.com" || host.ends_with(".slack.com")) && path.starts_with("shortcuts/Ft")
}

/// Read-only macOS Calendar.app probe: returns `Some(true)` if a non-all-day event is active
/// right now, `Some(false)` if free, or `None` if Calendar is unavailable or permission denied.
#[cfg(target_os = "macos")]
fn probe_macos_calendar_busy() -> Option<bool> {
    const SCRIPT: &str = r#"
tell application "System Events"
    if not (exists process "Calendar") then return "FREE"
end tell
tell application "Calendar"
    set nowDate to current date
    repeat with c in calendars
        try
            set evs to (every event of c whose start date <= nowDate and end date >= nowDate)
            repeat with e in evs
                if (allday event of e) is false then
                    return "BUSY"
                end if
            end repeat
        end try
    end repeat
end tell
return "FREE"
"#;
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(SCRIPT)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    match stdout.trim() {
        "BUSY" => Some(true),
        "FREE" => Some(false),
        _ => None,
    }
}
