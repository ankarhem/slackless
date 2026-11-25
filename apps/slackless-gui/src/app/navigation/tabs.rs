//! Conversation header tabs, canvases, lists, bookmarks, and pop-out windows.

use super::super::*;

impl App {
    /// A tab row entry was clicked: switch the column, load the live channel canvas in the
    /// native reader, or create the missing canvas.
    pub(in crate::app) fn activate_tab(&mut self, action: TabAction, cx: &mut Context<Self>) {
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
    pub(in crate::app) fn open_live_canvas(&mut self, file_id: String, cx: &mut Context<Self>) {
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
    pub(in crate::app) fn open_live_list(&mut self, file_id: String, cx: &mut Context<Self>) {
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
    pub(in crate::app) fn select_tab(&mut self, target: ConversationTab, cx: &mut Context<Self>) {
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
    pub(in crate::app) fn load_tab_data(&mut self, tab: &ConversationTab) {
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
    pub(in crate::app) fn live_bookmarks(&self) -> Vec<bookmarks::Bookmark> {
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
    pub(in crate::app) fn open_folder_tab(&mut self, folder_id: &str, cx: &mut Context<Self>) {
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
    pub(in crate::app) fn canvas_link(&self, canvas_id: &str) -> String {
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

    /// The "+" menu's Canvas item: open the conversation's canvas natively, creating it first
    /// when it has none (the created canvas arrives as `Event::ChannelCanvasCreated` and opens
    /// natively then; `F-COLLAB-123`, `F-COLLAB-124`).
    pub(in crate::app) fn open_or_add_canvas(&mut self, cx: &mut Context<Self>) {
        let canvas = self
            .fixtures
            .conversation(&self.channel)
            .and_then(|conversation| conversation.properties.as_ref())
            .and_then(|properties| properties.canvas.as_ref())
            .and_then(|canvas| canvas.file_id.as_ref())
            .map(|file| file.to_string());
        match canvas {
            Some(file) => self.open_live_canvas(file, cx),
            None => {
                self.bridge
                    .send(Command::CreateChannelCanvas(self.channel.clone()));
            }
        }
    }

    /// The add-link dialog's Add: live mode sends `bookmarks.add`, fixture mode keeps the
    /// bookmark in memory with the fixture bar.
    pub(in crate::app) fn add_bookmark(
        &mut self,
        title: String,
        url: String,
        cx: &mut Context<Self>,
    ) {
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
    /// Callback for popped-out Activity / Saved windows: brings the main window to the front
    /// and navigates to the clicked conversation, message, or thread.
    pub(in crate::app) fn popout_jump_handler(
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
    pub(in crate::app) fn pop_out_conversation(
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
    pub(in crate::app) fn pop_out_thread(
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
    pub(in crate::app) fn pop_out_activity(&self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(in crate::app) fn pop_out_later(&self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(in crate::app) fn pop_out_canvas(
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
    pub(in crate::app) fn pop_out_files(&self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(in crate::app) fn select_conversation_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(conv) = self.fixtures.conversation(&self.channel) else {
            return;
        };
        let specs = self.conversation_tab_specs(conv);
        if let Some(spec) = specs.into_iter().nth(index) {
            self.activate_tab(spec.action, cx);
        }
    }

    /// Format the live cross-workspace unread & mention summary (`F-SHELL-278`).
    pub(in crate::app) fn cross_workspace_summary_text(&self, cx: &gpui::App) -> String {
        let ws_count = self
            .live_teams
            .len()
            .max(self.fixtures.workspaces.len())
            .max(1);
        let mut entries = Vec::with_capacity(ws_count);
        for ix in 0..ws_count {
            let badge = if ix == self.workspace {
                let sidebar = self.sidebar.read(cx);
                WorkspaceBadge {
                    mentions: sidebar.badge_count(),
                    has_unreads: sidebar.has_any_unreads(),
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
            let name = self
                .live_teams
                .get(ix)
                .or_else(|| self.fixtures.workspaces.get(ix))
                .map(|t| t.name.clone())
                .unwrap_or_else(|| format!("Workspace {}", ix + 1));
            entries.push((name, badge.mentions, badge.has_unreads));
        }
        crate::preferences::format_cross_workspace_summary(&entries)
    }

    /// Announce the live cross-workspace unread/mention summary (`F-SHELL-278`).
    pub(in crate::app) fn announce_cross_workspace_summary(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> String {
        let summary = self.cross_workspace_summary_text(cx);
        notify("Workspace summary", summary.clone(), window, cx);
        summary
    }

    /// Format and announce the currently focused transcript message according to the user's
    /// screen-reader verbosity setting (`F-SHELL-105`, `F-SHELL-106`, `F-SHELL-279`).
    pub(in crate::app) fn replay_focused_message(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let ts = self.messages.read(cx).focused(cx)?;
        let msg = self
            .messages
            .read(cx)
            .message_by_ts(&self.channel, &ts, cx)?;
        let settings = self.preferences.read(cx).settings();
        let author = msg
            .user
            .as_ref()
            .map(|u| self.fixtures.user_name(u))
            .or(msg.username.as_deref())
            .unwrap_or("Someone");
        let stamp = crate::messages::day_and_time_with(&msg.ts, settings.clock_24h);
        let reactions: usize = msg.reactions.iter().map(|r| r.count as usize).sum();
        let replies = msg.reply_count.unwrap_or(0) as usize;
        let announcement = crate::preferences::format_accessible_message(
            author,
            &stamp,
            &msg.text,
            reactions,
            replies,
            msg.edited.is_some(),
            settings.message_verbosity,
        );
        notify("Message replay", announcement.clone(), window, cx);
        Some(announcement)
    }
}
