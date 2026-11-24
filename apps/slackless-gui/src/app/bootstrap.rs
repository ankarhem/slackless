//! Root `App` initialization (`App::new` / `App::new_multi`), subscriptions, and startup hydration.

use super::*;

impl App {
    #[cfg(test)]
    pub fn new(
        fixtures: Arc<Fixtures>,
        bridge: CoreBridge,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Entity<App> {
        Self::new_multi(fixtures, vec![bridge], window, cx)
    }

    pub fn new_multi(
        fixtures: Arc<Fixtures>,
        mut bridges: Vec<CoreBridge>,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Entity<App> {
        if bridges.is_empty() {
            bridges.push(CoreBridge::offline());
        }
        let app = cx.new(|cx| {
            let mut bridge = bridges.remove(0);
            let live_teams = fixtures.workspaces.clone();
            let ws_count = live_teams.len().max(1);
            let mut parked_bridges: Vec<Option<CoreBridge>> = Vec::with_capacity(ws_count);
            parked_bridges.push(None);
            for extra in bridges {
                parked_bridges.push(Some(extra));
            }
            while parked_bridges.len() < ws_count {
                parked_bridges.push(None);
            }
            let bg_counts = vec![HashMap::new(); ws_count];
            let bg_activity_unread = vec![0; ws_count];
            // Read all workspace `local_state` keys once (1 SQLite query instead of ~10).
            let local = bridge.all_local();
            // Like Slack, reopen the conversation that was open last; a fresh store falls back
            // to the sidebar's first conversation until the user picks one.
            let restored = CoreBridge::parse_last_conversation(&local)
                .filter(|channel| fixtures.conversation(channel).is_some());
            let chosen = restored.is_some();
            let channel = restored.unwrap_or_else(|| first_conversation(&fixtures));
            let archive = bridge
                .store()
                .cloned()
                .or_else(|| fixture_archive(&fixtures));
            let views = build_workspace(&fixtures, 0, archive, &channel, window, cx);
            let workspace_subscriptions = subscribe_workspace(&views, window, cx);
            let WorkspaceViews {
                sidebar,
                messages,
                thread,
                results,
                browser,
                huddle,
                profile,
                switcher,
            } = views;
            let composer = Composer::new("Message".into(), fixtures.clone(), window, cx);
            let search = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder(SEARCH_PLACEHOLDER)
                    .auto_grow(1, 1)
            });
            let search_menu = SearchMenu::new(fixtures.clone(), search.clone(), cx);
            // The search panel's remembered state, restored at launch like Slack's: the
            // recent searches, both toggles and the recently visited conversations.
            if let Some(history) = CoreBridge::parse_search_history(&local) {
                search_menu.update(cx, |menu, cx| menu.set_history(history, cx));
            }
            if let Some(on) = CoreBridge::parse_search_history_shown(&local) {
                search_menu.update(cx, |menu, cx| menu.set_history_shown(on, cx));
            }
            if let Some(on) = CoreBridge::parse_search_filters_shown(&local) {
                search_menu.update(cx, |menu, cx| menu.set_filters_shown(on, cx));
            }
            let restored_recent = CoreBridge::parse_recent_places(&local)
                .unwrap_or_default()
                .into_iter()
                .filter(|channel| fixtures.conversation(channel).is_some())
                .collect();
            let preferences = Preferences::new(window, cx);
            // Restore persisted Preferences settings first, then overlay the zoom/font/color_mode
            // values `main` installed before the first frame.
            let mut restored_settings = CoreBridge::parse_settings(&local);
            if let Some(settings) = restored_settings.clone() {
                preferences.update(cx, |prefs, cx| prefs.load_settings(settings, window, cx));
            }
            let factor = cx.global::<crate::zoom::Zoom>().factor();
            let zoom = (crate::zoom::FACTORS.iter())
                .position(|level| *level == factor)
                .unwrap_or(crate::zoom::ACTUAL_SIZE);
            preferences.update(cx, |prefs, cx| prefs.set_zoom(zoom, cx));
            // Live: the remembered Color Mode over Slack's color theme (or persisted palette),
            // as `main` installed it before the first frame.
            let color_mode = bridge.parse_color_mode(&local);
            let applied_theme = match color_mode {
                Some(mode) => {
                    let palette = restored_settings
                        .as_ref()
                        .map(|s| s.palette)
                        .or(fixtures.palette)
                        .unwrap_or(Palette::Default);
                    let font = theme::font(cx);
                    preferences.update(cx, |prefs, cx| {
                        prefs.set_appearance(mode, palette, font, cx)
                    });
                    if let Some(ref mut s) = restored_settings {
                        s.mode = mode;
                        s.palette = palette;
                        s.font = font;
                        s.zoom = zoom;
                    }
                    let darker = restored_settings
                        .as_ref()
                        .is_some_and(|s| s.darker_sidebars);
                    let gradient = restored_settings.as_ref().is_none_or(|s| s.window_gradient);
                    (
                        mode.resolve(window.appearance()),
                        palette,
                        darker,
                        gradient,
                        false,
                    )
                }
                None => (SlackTheme::Dark, Palette::Default, false, true, false),
            };
            let saved_settings = if bridge.store().is_some() {
                Some(restored_settings.unwrap_or_else(|| preferences.read(cx).settings().clone()))
            } else {
                None
            };
            let status_picker = StatusPicker::new(window, cx);
            let user_menu = UserMenu::new(fixtures.clone(), cx);
            let emoji_picker = EmojiPicker::new(fixtures.clone(), window, cx);
            let persisted_freq = CoreBridge::parse_frequent_emoji(&local);
            let persisted_tone = CoreBridge::parse_emoji_tone(&local);
            if persisted_freq.is_some() || persisted_tone.is_some() {
                emoji_picker.update(cx, |picker, cx| {
                    picker.restore(persisted_freq, persisted_tone, cx);
                });
            }
            let drafts = DraftsView::new(cx);
            if let Some(saved_drafts) = CoreBridge::parse_drafts(&local) {
                let entries: Vec<Entry> = saved_drafts
                    .into_iter()
                    .map(|d| Entry {
                        id: None,
                        workspace: 0,
                        channel: d.channel,
                        label: d.label,
                        text: d.text,
                        at: d.at,
                    })
                    .collect();
                drafts.update(cx, |view, cx| view.load_drafts(entries, cx));
            }
            let later = LaterView::new(cx);
            let threads = ThreadsView::new(fixtures.clone(), window, cx);
            let unreads = UnreadsView::new(fixtures.clone(), cx);
            let activity = ActivityFeed::new(fixtures.clone(), cx);
            let files_page = FilesPage::new(fixtures.clone(), window, cx);
            let huddles = HuddlesView::new(fixtures.clone(), cx);
            let directory = DirectoryView::new(fixtures.clone(), window, cx);
            let tools_page = crate::tools_page::ToolsPage::new(fixtures.clone(), window, cx);
            let compose = NewMessageView::new(fixtures.clone(), window, cx);
            let details = ChannelDetails::new(fixtures.clone(), window, cx);
            let canvas_search = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Find in canvas…")
                    .auto_grow(1, 1)
            });
            let list_search = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Filter list items…")
                    .auto_grow(1, 1)
            });
            let app_home_kit = cx.new(|_| crate::blockkit::BlockKitState::default());
            if fixtures.sample {
                later.update(cx, |later, cx| later.seed(0, &fixtures, cx));
            }
            crate::actions::bind_menu_keys(cx);
            cx.bind_keys([
                KeyBinding::new("cmd-k", QuickSwitch, None),
                KeyBinding::new("cmd-j", JumpToUnread, None),
                KeyBinding::new("cmd-shift-h", OpenHome, None),
                KeyBinding::new("cmd-[", NavigateBack, None),
                KeyBinding::new("cmd-]", NavigateForward, None),
                KeyBinding::new("cmd-,", OpenPreferences, None),
                KeyBinding::new("cmd-/", ShowShortcuts, None),
                KeyBinding::new("cmd-n", NewMessage, None),
                KeyBinding::new("cmd-shift-k", OpenDms, None),
                // Slack's Activity shortcut: Cmd+Shift+A is Slack's All Unreads, which
                // its own menu labels "All Unreads"; Activity keeps Cmd+Shift+M.
                KeyBinding::new("cmd-shift-m", OpenActivity, None),
                KeyBinding::new("cmd-shift-a", OpenUnreads, None),
                KeyBinding::new("cmd-shift-t", OpenThreads, None),
                KeyBinding::new("cmd-shift-d", OpenDrafts, None),
                KeyBinding::new("cmd-shift-e", OpenPeople, None),
                KeyBinding::new("cmd-shift-j", OpenDownloads, None),
                KeyBinding::new("cmd-shift-l", OpenChannelBrowser, None),
                KeyBinding::new("cmd-shift-y", SetStatus, None),
                KeyBinding::new("cmd-g", FocusSearch, None),
                KeyBinding::new("cmd-f", SearchConversation, None),
                KeyBinding::new("cmd-shift-s", ToggleWorkspaceRail, None),
                KeyBinding::new("cmd-.", ToggleRightPane, None),
                KeyBinding::new("cmd-shift-backspace", UnsendLastMessage, None),
                KeyBinding::new("ctrl-tab", NextNavTab, None),
                KeyBinding::new("ctrl-shift-tab", PrevNavTab, None),
                KeyBinding::new("ctrl-down", NextSection, None),
                KeyBinding::new("ctrl-up", PrevSection, None),
                KeyBinding::new("f6", FocusNextPane, None),
                KeyBinding::new("shift-f6", FocusPrevPane, None),
                // Slack's View menu shows Cmd++; Cmd+= (no Shift) zooms in too.
                KeyBinding::new("cmd-+", ZoomIn, None),
                KeyBinding::new("cmd-=", ZoomIn, None),
                KeyBinding::new("cmd--", ZoomOut, None),
                KeyBinding::new("cmd-0", ActualSize, None),
                KeyBinding::new("cmd-}", NextWorkspace, None),
                KeyBinding::new("cmd-{", PreviousWorkspace, None),
                KeyBinding::new("alt-up", PreviousConversation, None),
                KeyBinding::new("alt-down", NextConversation, None),
                KeyBinding::new("alt-shift-up", PreviousUnread, None),
                KeyBinding::new("alt-shift-down", NextUnread, None),
                // Only fire while focus is on the app shell itself, never inside an input,
                // popup menu, dialog or popover: GPUI resolves keybindings before text
                // insertion, so an unscoped single-key binding would swallow those letters
                // (and Backspace / Right / Escape) in the composer and every text field.
                KeyBinding::new("shift-escape", MarkAllRead, None),
                KeyBinding::new("cmd-shift-space", ToggleHuddleMute, None),
            ]);
            const OUTSIDE_INPUT: Option<&str> =
                Some("App && !Input && !PopupMenu && !Dialog && !Popover && !Select");
            cx.bind_keys([
                KeyBinding::new("escape", MarkConversationRead, OUTSIDE_INPUT),
                // Slack's one-key actions on the focused message (the transcript's last
                // hovered row). Scoped to `OUTSIDE_INPUT` so focused fields still receive
                // their own letters, arrows, backspace and escape; [`App::focus_message`]
                // re-checks the rest (which message, whose message, no overlay open).
                KeyBinding::new("e", FocusEditMessage, OUTSIDE_INPUT),
                KeyBinding::new("backspace", FocusDeleteMessage, OUTSIDE_INPUT),
                KeyBinding::new("t", FocusReplyInThread, OUTSIDE_INPUT),
                KeyBinding::new("right", FocusReplyInThread, OUTSIDE_INPUT),
                KeyBinding::new("f", FocusForwardMessage, OUTSIDE_INPUT),
                KeyBinding::new("p", FocusTogglePin, OUTSIDE_INPUT),
                KeyBinding::new("a", FocusSaveMessage, OUTSIDE_INPUT),
                KeyBinding::new("u", FocusMarkMessageUnread, OUTSIDE_INPUT),
                KeyBinding::new("m", FocusRemindMe, OUTSIDE_INPUT),
                KeyBinding::new("r", FocusReact, OUTSIDE_INPUT),
                KeyBinding::new("up", BrowseUp, OUTSIDE_INPUT),
                KeyBinding::new("down", BrowseDown, OUTSIDE_INPUT),
                KeyBinding::new("enter", BrowseOpen, OUTSIDE_INPUT),
                KeyBinding::new("left", ExitThread, OUTSIDE_INPUT),
                KeyBinding::new("home", JumpToOldest, OUTSIDE_INPUT),
                KeyBinding::new("end", JumpToNewest, OUTSIDE_INPUT),
            ]);
            let workspace_keys = [
                "cmd-1", "cmd-2", "cmd-3", "cmd-4", "cmd-5", "cmd-6", "cmd-7", "cmd-8", "cmd-9",
            ];
            cx.bind_keys(
                (workspace_keys.into_iter().enumerate())
                    .map(|(ix, keys)| KeyBinding::new(keys, SwitchWorkspace(ix), None)),
            );
            let tab_keys = [
                "ctrl-1", "ctrl-2", "ctrl-3", "ctrl-4", "ctrl-5", "ctrl-6", "ctrl-7", "ctrl-8",
                "ctrl-9",
            ];
            cx.bind_keys(
                (tab_keys.into_iter().enumerate())
                    .map(|(ix, keys)| KeyBinding::new(keys, SelectConversationTab(ix), None)),
            );
            let focus = cx.focus_handle();
            focus.focus(window, cx);

            let this = cx.weak_entity();
            // A second handle: the search intercept below moves `this` into its closure.
            let viewer_keys = cx.weak_entity();
            // A third: the profile menu's Escape intercept moves `this` into its closure.
            let menu_keys = cx.weak_entity();
            // A fourth: the member hover card's Escape intercept.
            let card_keys = cx.weak_entity();
            // A fifth: the All Unreads view's arrow-key intercept.
            let unreads_keys = cx.weak_entity();
            let mut subscriptions = vec![
                cx.observe_window_bounds(window, |this, window, cx| {
                    this.save_window_state(window, cx);
                }),
                cx.observe_window_activation(window, |this, window, cx| {
                    this.window_active = window.is_window_active();
                    if this.window_active {
                        let settings = this.preferences.read(cx).settings().clone();
                        if settings.no_bg_mark_read && settings.mark_read != 2 {
                            if this.threads_open {
                                this.auto_mark_threads_view_read(cx);
                            } else {
                                this.maybe_mark_open_read(cx);
                            }
                        }
                    }
                }),
                // While the search panel is open, Up/Down/Enter/Escape steer its rows. GPUI
                // resolves the focused field's own key bindings (caret moves, a newline)
                // before any listener around it, so the keys are taken here, ahead of them.
                cx.intercept_keystrokes(move |event, window, cx| {
                    if event.input_preference == InputPreference::CharacterInput {
                        return;
                    }
                    let _ = this.update(cx, |app, cx| {
                        if !app.search_menu_open
                            || !app.search.read(cx).focus_handle(cx).is_focused(window)
                        {
                            return;
                        }
                        if app
                            .search_menu
                            .update(cx, |menu, cx| menu.handle_key(&event.keystroke, cx))
                        {
                            cx.stop_propagation();
                        }
                    });
                }),
                // While the image viewer is open, Escape closes it and the arrows step
                // through the conversation's images (ahead of any focused field, like the
                // search panel's keys above).
                cx.intercept_keystrokes({
                    move |event, window, cx| {
                        if event.input_preference == InputPreference::CharacterInput {
                            return;
                        }
                        let _ = viewer_keys.update(cx, |app, cx| {
                            let Some(viewer) = app.image_viewer.clone() else {
                                return;
                            };
                            match event.keystroke.key.as_str() {
                                "escape" => {
                                    app.close_image_viewer(cx);
                                    cx.stop_propagation();
                                }
                                "left" => {
                                    viewer.update(cx, |viewer, cx| viewer.step(-1, window, cx));
                                    cx.stop_propagation();
                                }
                                "right" => {
                                    viewer.update(cx, |viewer, cx| viewer.step(1, window, cx));
                                    cx.stop_propagation();
                                }
                                _ => {}
                            }
                        });
                    }
                }),
                // While the profile menu is open, Escape closes it, submenu included —
                // Slack closes the whole menu, not just the open submenu.
                cx.intercept_keystrokes({
                    move |event, _, cx| {
                        if event.input_preference == InputPreference::CharacterInput
                            || event.keystroke.key.as_str() != "escape"
                        {
                            return;
                        }
                        let _ = menu_keys.update(cx, |app, cx| {
                            if !app.user_menu_open {
                                return;
                            }
                            app.user_menu_open = false;
                            app.user_menu.update(cx, |menu, cx| menu.closed(cx));
                            cx.notify();
                            cx.stop_propagation();
                        });
                    }
                }),
                // While the member hover card is open, Escape closes it, like Slack's
                // hovercard.
                cx.intercept_keystrokes(move |event, _, cx| {
                    if event.input_preference == InputPreference::CharacterInput
                        || event.keystroke.key.as_str() != "escape"
                    {
                        return;
                    }
                    let _ = card_keys.update(cx, |app, cx| {
                        if app.profile_hover.card.is_some() {
                            app.close_hover_card(cx);
                            cx.stop_propagation();
                        }
                    });
                }),
                // While the All Unreads view is open and no field, dialog or menu holds the
                // focus, its keys steer the cards (Slack's "Navigate unread messages"):
                // Up/Down walk, Left/Right fold, Esc marks the focused group read - ahead of
                // the transcript's own arrow and Esc actions.
                cx.intercept_keystrokes(move |event, window, cx| {
                    if event.input_preference == InputPreference::CharacterInput {
                        return;
                    }
                    let _ = unreads_keys.update(cx, |app, cx| {
                        if !app.unreads_open
                            || !window
                                .focused(cx)
                                .is_none_or(|focused| focused == app.focus)
                        {
                            return;
                        }
                        let handled = app
                            .unreads
                            .update(cx, |view, cx| view.handle_key(&event.keystroke.key, cx));
                        if handled {
                            cx.stop_propagation();
                        }
                    });
                }),
                bridge.subscribe_in(window, cx, |this: &mut App, event, window, cx| {
                    this.on_workspace_event(0, event, window, cx);
                }),
                cx.subscribe_in(
                    &search,
                    window,
                    |this: &mut App, search, event: &InputEvent, window, cx| match event {
                        InputEvent::PressEnter { .. } => {
                            let query = search.read(cx).value().to_string();
                            this.run_message_search(query, cx);
                        }
                        // The pill's text changed while the dropdown is open: refilter it.
                        InputEvent::Change if this.search_menu_open => {
                            let value = search.read(cx).value().to_string();
                            this.search_menu
                                .update(cx, |menu, cx| menu.set_query(&value, cx));
                        }
                        // Focus moved away from the pill (a dialog, another pane): fold
                        // it, leaving whatever took the focus alone.
                        InputEvent::Blur if this.search_menu_open => {
                            this.fold_search_menu(window, cx);
                        }
                        _ => {}
                    },
                ),
                cx.subscribe_in(
                    &search_menu,
                    window,
                    |this: &mut App, _, event: &SearchMenuEvent, window, cx| match event {
                        SearchMenuEvent::Open(channel) => {
                            this.close_search_menu(window, cx);
                            this.select(channel.clone(), cx);
                            this.focus.focus(window, cx);
                        }
                        // The Threads row of "Unreads & drafts": the sidebar's own path.
                        SearchMenuEvent::OpenThreads => {
                            this.close_search_menu(window, cx);
                            this.open_threads(cx);
                            this.focus.focus(window, cx);
                        }
                        // The field stays focused: the query can be refined and re-run.
                        SearchMenuEvent::Search(query) => {
                            this.fold_search_menu(window, cx);
                            this.run_message_search(query.clone(), cx);
                        }
                        SearchMenuEvent::SetHistoryShown(on) => {
                            if let Some(write) = this.bridge.remember_search_history_shown(*on) {
                                cx.background_executor().spawn(write).detach();
                            }
                        }
                        SearchMenuEvent::SetFiltersShown(on) => {
                            if let Some(write) = this.bridge.remember_search_filters_shown(*on) {
                                cx.background_executor().spawn(write).detach();
                            }
                        }
                        SearchMenuEvent::Dismiss => this.close_search_menu(window, cx),
                    },
                ),
                cx.subscribe_in(
                    &composer,
                    window,
                    |this: &mut App, _, event: &ComposerEvent, window, cx| match event {
                        ComposerEvent::Submit(content) => this.send(content.clone(), window, cx),
                        ComposerEvent::SubmitWithFiles { content, files } => {
                            let channel = this.channel.clone();
                            this.upload_files(
                                channel,
                                None,
                                content.clone(),
                                files.clone(),
                                window,
                                cx,
                            );
                        }
                        ComposerEvent::OpenEmojiPicker(position) => {
                            this.open_picker(PickerTarget::Composer, *position, window, cx);
                        }
                        ComposerEvent::OpenGifPicker(position) => {
                            this.open_picker_tab(
                                PickerTarget::Composer,
                                *position,
                                PickerTab::Gifs,
                                window,
                                cx,
                            );
                        }
                        // The message list has no way in to start an edit from outside yet;
                        // the thread panel handles this for its own composer.
                        ComposerEvent::EditLast => this
                            .messages
                            .update(cx, |list, cx| list.edit_last_own(window, cx)),
                        ComposerEvent::SlashCommand(invocation) => {
                            this.run_slash(invocation, None, window, cx);
                        }
                        ComposerEvent::Schedule { text, when } => {
                            let channel = this.channel.clone();
                            this.schedule_at(
                                channel,
                                None,
                                text.clone(),
                                when.at(now_secs()),
                                when.label().to_lowercase(),
                                window,
                                cx,
                            );
                        }
                        ComposerEvent::ScheduleCustom(text) => {
                            let channel = this.channel.clone();
                            this.open_custom_schedule(channel, None, text.clone(), window, cx);
                        }
                        ComposerEvent::Typing => this.bridge.send(Command::Typing {
                            channel: this.channel.clone(),
                            thread_ts: None,
                        }),
                        ComposerEvent::Cancel => {}
                    },
                ),
                cx.subscribe_in(
                    &emoji_picker,
                    window,
                    |this: &mut App, _, event: &EmojiPickerEvent, window, cx| match event {
                        EmojiPickerEvent::Pick(choice) => this.pick_emoji(choice, window, cx),
                        EmojiPickerEvent::PickGif(gif) => this.pick_gif(gif, window, cx),
                        EmojiPickerEvent::RunGiphy(query) => this.run_giphy(query, window, cx),
                        EmojiPickerEvent::SearchGifs(query) => {
                            if !this.fixtures.sample {
                                this.bridge.send(Command::SearchGifs {
                                    query: query.clone(),
                                });
                            }
                        }
                        EmojiPickerEvent::ToneChanged(tone) => {
                            if let Some(write) = this.bridge.remember_emoji_tone(*tone) {
                                cx.background_executor().spawn(write).detach();
                            }
                            if !this.fixtures.sample {
                                this.bridge.send(Command::SetUserPref {
                                    name: "preferred_skin_tone".to_owned(),
                                    value: (*tone).to_string(),
                                });
                            }
                        }
                    },
                ),
                cx.subscribe_in(
                    &preferences,
                    window,
                    |this: &mut App, _, event: &PreferencesEvent, window, cx| match event {
                        PreferencesEvent::Changed => this.apply_preferences(window, cx),
                        PreferencesEvent::ResetCache => this.confirm_reset_cache(window, cx),
                        PreferencesEvent::CompactStorage => this.compact_storage(window, cx),
                        PreferencesEvent::PruneMediaCache => this.prune_media_cache(window, cx),
                        PreferencesEvent::ExportNetLog => {
                            let json = slackless_core::api::net_log_export_json();
                            let configured =
                                this.preferences.read(cx).settings().download_dir.clone();
                            let dir = if !configured.is_empty() {
                                std::path::PathBuf::from(configured)
                            } else {
                                std::env::var_os("HOME")
                                    .map(|h| std::path::PathBuf::from(h).join("Downloads"))
                                    .unwrap_or_else(std::env::temp_dir)
                            };
                            let path = dir.join("slackless-net-log.json");
                            match std::fs::write(&path, json) {
                                Ok(()) => notify(
                                    "Net log exported",
                                    format!("Saved to {}", path.display()),
                                    window,
                                    cx,
                                ),
                                Err(err) => {
                                    notify("Net log export failed", err.to_string(), window, cx)
                                }
                            }
                        }
                        PreferencesEvent::TestSpeaker(output_device) => {
                            if this.fixtures.sample {
                                let dev = output_device.clone();
                                std::thread::spawn(move || {
                                    let _ = slackless_core::huddle::play_speaker_test_tone(
                                        dev.as_deref(),
                                    );
                                });
                            } else {
                                this.bridge.send(Command::TestHuddleSpeaker {
                                    output_device: output_device.clone(),
                                });
                            }
                        }
                        PreferencesEvent::CheckForUpdates => this.check_for_updates(true, cx),
                        PreferencesEvent::InstallUpdate => this.install_available_update(cx),
                    },
                ),
                cx.observe_window_appearance(window, |this: &mut App, window, cx| {
                    this.apply_preferences(window, cx);
                }),
                cx.subscribe_in(
                    &status_picker,
                    window,
                    |this: &mut App, _, event: &StatusEvent, window, cx| {
                        match event {
                            StatusEvent::Save(status) => {
                                this.self_state.set_status(status.clone());
                                this.send_status_to_slack(this.self_state.status.as_ref());
                                if this
                                    .preferences
                                    .read(cx)
                                    .settings()
                                    .status_pauses_notifications
                                {
                                    if let Some(st) = status {
                                        let now = crate::messages::now_secs();
                                        let mins = st
                                            .expiration
                                            .map(|until| {
                                                ((until.saturating_sub(now) + 59) / 60)
                                                    .clamp(1, 1440)
                                                    as u32
                                            })
                                            .unwrap_or(60);
                                        this.self_state.paused_until =
                                            Some(now + i64::from(mins) * 60);
                                        this.bridge.send(Command::SetDndSnooze { minutes: mins });
                                    } else if this.self_state.paused() {
                                        this.self_state.paused_until = None;
                                        this.bridge.send(Command::EndDndSnooze);
                                    }
                                }
                                this.push_self_state(cx);
                            }
                            StatusEvent::Schedule(sched) => {
                                this.self_state.scheduled_statuses.push(sched.clone());
                                let when = crate::messages::clock(sched.starts_at, None);
                                notify(
                                    "Status scheduled",
                                    format!("Will set \"{}\" at {when}.", sched.text),
                                    window,
                                    cx,
                                );
                            }
                            StatusEvent::RemoveScheduled(ix) => {
                                if *ix < this.self_state.scheduled_statuses.len() {
                                    this.self_state.scheduled_statuses.remove(*ix);
                                    notify(
                                        "Scheduled status removed",
                                        "Cancelled upcoming scheduled status.",
                                        window,
                                        cx,
                                    );
                                }
                                return;
                            }
                            StatusEvent::Cancel => {}
                        }
                        window.close_dialog(cx);
                    },
                ),
                cx.subscribe_in(
                    &user_menu,
                    window,
                    |this: &mut App, _, event: &UserMenuEvent, window, cx| {
                        this.user_menu_event(event, window, cx);
                    },
                ),
                cx.subscribe_in(
                    &drafts,
                    window,
                    |this: &mut App, _, event: &DraftsEvent, window, cx| match event {
                        DraftsEvent::Close => {
                            this.drafts_open = false;
                            cx.notify();
                        }
                        DraftsEvent::Open { workspace, channel } => {
                            this.switch_workspace(*workspace, window, cx);
                            this.select(channel.clone(), cx);
                        }
                        DraftsEvent::SendNow(entry) => this.deliver(entry.clone(), window, cx),
                        DraftsEvent::CancelScheduled(entry) => {
                            if let Some(id) = entry.id.clone() {
                                this.bridge.send(Command::DeleteScheduledMessage {
                                    channel: entry.channel.clone(),
                                    scheduled_message_id: id,
                                });
                            }
                        }
                        DraftsEvent::Deleted => {
                            let count = this.drafts.read(cx).draft_count();
                            this.sidebar
                                .update(cx, |sidebar, cx| sidebar.set_draft_count(count, cx));
                            this.persist_drafts(cx);
                        }
                    },
                ),
                cx.subscribe_in(
                    &later,
                    window,
                    |this: &mut App, _, event: &LaterEvent, window, cx| match event {
                        LaterEvent::PopOut => this.pop_out_later(window, cx),
                        LaterEvent::Close => {
                            this.later_open = false;
                            cx.notify();
                        }
                        LaterEvent::Open {
                            workspace,
                            channel,
                            ts,
                        } => {
                            this.switch_workspace(*workspace, window, cx);
                            this.select(channel.clone(), cx);
                            this.messages.update(cx, |list, cx| list.reveal(ts, cx));
                        }
                        LaterEvent::Unsaved {
                            workspace,
                            channel,
                            ts,
                        } => {
                            if *workspace == this.workspace {
                                this.messages
                                    .update(cx, |list, cx| list.set_saved(ts, false, cx));
                                this.thread
                                    .update(cx, |thread, cx| thread.set_saved(ts, false, cx));
                                this.threads
                                    .update(cx, |threads, cx| threads.set_saved(ts, false, cx));
                                this.sync_saved(cx);
                                this.bridge.send(Command::DeleteSavedItem {
                                    channel: channel.clone(),
                                    ts: ts.clone(),
                                });
                            } else if let Some(bridge) =
                                this.parked_bridges.get(*workspace).and_then(Option::as_ref)
                            {
                                bridge.send(Command::DeleteSavedItem {
                                    channel: channel.clone(),
                                    ts: ts.clone(),
                                });
                            }
                        }
                        LaterEvent::Update {
                            workspace,
                            channel,
                            ts,
                            mark,
                            date_due,
                        } => {
                            let cmd = Command::UpdateSavedItem {
                                channel: channel.clone(),
                                ts: ts.clone(),
                                mark: *mark,
                                date_due: *date_due,
                            };
                            if *workspace == this.workspace {
                                this.bridge.send(cmd);
                            } else if let Some(bridge) =
                                this.parked_bridges.get(*workspace).and_then(Option::as_ref)
                            {
                                bridge.send(cmd);
                            }
                        }
                        LaterEvent::ClearCompleted { workspace } => {
                            if *workspace == this.workspace {
                                this.bridge.send(Command::ClearCompletedSaved);
                            } else if let Some(bridge) =
                                this.parked_bridges.get(*workspace).and_then(Option::as_ref)
                            {
                                bridge.send(Command::ClearCompletedSaved);
                            }
                        }
                        LaterEvent::Changed => this.sync_later_count(cx),
                    },
                ),
                cx.subscribe_in(
                    &threads,
                    window,
                    |this: &mut App, _, event: &ThreadsEvent, window, cx| match event {
                        ThreadsEvent::Open { channel, thread_ts } => {
                            this.select(channel.clone(), cx);
                            this.open_thread(channel.clone(), thread_ts.clone(), cx);
                        }
                        ThreadsEvent::OpenLink(target) => {
                            this.open_link(target.clone(), window, cx);
                        }
                        ThreadsEvent::ExpandThread { channel, thread_ts } => {
                            this.bridge.send(Command::OpenThread {
                                channel: channel.clone(),
                                thread_ts: thread_ts.clone(),
                            });
                            if let Some(read) =
                                this.bridge.thread(channel.clone(), thread_ts.clone())
                            {
                                let (ch, ts) = (channel.clone(), thread_ts.clone());
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
                        ThreadsEvent::Reply {
                            channel,
                            thread_ts,
                            content,
                            broadcast,
                        } => {
                            this.bridge.send(Command::SendMessage {
                                channel: channel.clone(),
                                content: content.clone(),
                                thread_ts: Some(thread_ts.clone()),
                                broadcast: *broadcast,
                            });
                            this.sync_thread_sidebar_counts(cx);
                        }
                        ThreadsEvent::ReplyWithFiles {
                            channel,
                            thread_ts,
                            content,
                            files,
                        } => {
                            this.upload_files(
                                channel.clone(),
                                Some(thread_ts.clone()),
                                content.clone(),
                                files.clone(),
                                window,
                                cx,
                            );
                        }
                        ThreadsEvent::MessageAction {
                            channel,
                            thread_ts,
                            message,
                            action,
                        } => {
                            this.handle_threads_message_action(
                                channel.clone(),
                                thread_ts.clone(),
                                message.as_ref().clone(),
                                action.clone(),
                                window,
                                cx,
                            );
                        }
                        ThreadsEvent::EditMessage {
                            channel,
                            ts,
                            content,
                        } => {
                            this.post_change(
                                channel,
                                &MessageChange::Edit {
                                    ts: ts.clone(),
                                    content: content.clone(),
                                },
                            );
                        }
                        ThreadsEvent::MarkRead {
                            channel,
                            thread_ts,
                            ts,
                        } => {
                            this.mark_thread_read(
                                channel.clone(),
                                thread_ts.clone(),
                                ts.clone(),
                                true,
                                cx,
                            );
                        }
                        ThreadsEvent::MarkAllRead(targets) => {
                            for (channel, thread_ts, ts) in targets {
                                this.mark_thread_read(
                                    channel.clone(),
                                    thread_ts.clone(),
                                    ts.clone(),
                                    true,
                                    cx,
                                );
                            }
                            this.sidebar
                                .update(cx, |s, cx| s.set_thread_counts(false, 0, cx));
                        }
                        ThreadsEvent::PickEmoji(position) => {
                            this.open_picker(PickerTarget::ThreadsComposer, *position, window, cx);
                        }
                        ThreadsEvent::PickGif(position) => {
                            this.open_picker_tab(
                                PickerTarget::ThreadsComposer,
                                *position,
                                PickerTab::Gifs,
                                window,
                                cx,
                            );
                        }
                        ThreadsEvent::SlashCommand(invocation) => {
                            let thread = this.threads.read(cx).active_thread();
                            this.run_slash(invocation, thread, window, cx)
                        }
                    },
                ),
                cx.subscribe_in(
                    &unreads,
                    window,
                    |this: &mut App, _, event: &UnreadsEvent, window, cx| match event {
                        UnreadsEvent::OpenConversation(channel) => {
                            this.select(channel.clone(), cx);
                        }
                        UnreadsEvent::OpenThread { channel, thread_ts } => {
                            this.open_thread(channel.clone(), thread_ts.clone(), cx);
                        }
                        UnreadsEvent::OpenLink(target) => {
                            this.open_link(target.clone(), window, cx);
                        }
                        UnreadsEvent::MarkRead { channel, ts } => {
                            this.mark_read(channel, ts.clone(), cx);
                        }
                        UnreadsEvent::Reply { channel, text } => {
                            let content = slackless_core::api::MessageContent::plain(text.clone());
                            this.bridge.send(Command::SendMessage {
                                channel: channel.clone(),
                                content,
                                thread_ts: None,
                                broadcast: false,
                            });
                            actions::notify("Reply sent", this.label(channel), window, cx);
                        }
                        UnreadsEvent::MessageAction {
                            channel,
                            message,
                            action,
                        } => {
                            this.handle_unreads_message_action(
                                channel.clone(),
                                message.as_ref().clone(),
                                action.clone(),
                                window,
                                cx,
                            );
                        }
                        UnreadsEvent::MarkAllRead => this.request_mark_all_read(window, cx),
                    },
                ),
                cx.subscribe_in(
                    &activity,
                    window,
                    |this: &mut App, _, event: &ActivityEvent, window, cx| match event {
                        ActivityEvent::Open { channel, thread_ts } => {
                            this.select(channel.clone(), cx);
                            if let Some(thread_ts) = thread_ts {
                                this.open_thread(channel.clone(), thread_ts.clone(), cx);
                            }
                        }
                        ActivityEvent::Mark { entries, read } => {
                            this.bridge.send(Command::MarkActivity {
                                entries: entries.clone(),
                                read: *read,
                            });
                        }
                        ActivityEvent::Retry => this.fetch_activity(cx),
                        ActivityEvent::PopOut => this.pop_out_activity(window, cx),
                    },
                ),
                cx.observe(&activity, |this: &mut App, _, cx| {
                    // The rail tile and the dock badge follow the feed's unread rows.
                    let unread = this.activity.read(cx).unread_count();
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.set_activity_unread(unread, cx));
                    cx.notify();
                }),
                cx.subscribe_in(
                    &files_page,
                    window,
                    |this: &mut App, _, event: &FilesPageEvent, window, cx| match event {
                        FilesPageEvent::Load(filter) => {
                            this.bridge.send(Command::LoadFiles(filter.clone()));
                        }
                        FilesPageEvent::LoadExtras => {
                            this.bridge.send(Command::LoadFilesExtras);
                        }
                        FilesPageEvent::SaveFile { file_id, name } => {
                            this.bridge.send(Command::SaveFile {
                                file_id: file_id.clone(),
                                date_due: None,
                            });
                            notify("Saved for later", name.clone(), window, cx);
                        }
                        FilesPageEvent::DeleteFile { file_id, name } => {
                            this.confirm_delete_file(file_id.clone(), name.clone(), window, cx);
                        }
                        FilesPageEvent::ToggleFavorite {
                            file_id,
                            collection_id,
                            starred,
                        } => {
                            this.bridge.send(Command::SetFileFavorite {
                                file_id: file_id.clone(),
                                collection_id: collection_id.clone(),
                                starred: *starred,
                            });
                        }
                        FilesPageEvent::CreateCollection { name } => {
                            this.bridge.send(Command::CreateFileCollection {
                                name: name.clone(),
                                emoji: None,
                            });
                            notify("Collection created", name.clone(), window, cx);
                        }
                        FilesPageEvent::DeleteCollection { collection_id } => {
                            this.bridge.send(Command::DeleteFileCollection {
                                collection_id: collection_id.clone(),
                            });
                            notify("Collection deleted", "Removed file collection.", window, cx);
                        }
                        FilesPageEvent::RestoreFile { file_id, title } => {
                            this.bridge.send(Command::RestoreDeletedFile {
                                file_id: file_id.clone(),
                            });
                            notify("Restoring", title.clone(), window, cx);
                        }
                        FilesPageEvent::OpenCanvas { file_id } => {
                            this.open_live_canvas(file_id.clone(), cx);
                        }
                        FilesPageEvent::OpenList { file_id } => {
                            this.open_live_list(file_id.clone(), cx);
                        }
                        FilesPageEvent::CreateCanvas { title } => {
                            this.bridge.send(Command::CreateCanvas {
                                title: title.clone(),
                                markdown: format!("# {title}\n\n"),
                                channel: None,
                            });
                            notify("Creating canvas", title.clone(), window, cx);
                        }
                        FilesPageEvent::CreateList { name } => {
                            this.bridge.send(Command::CreateList {
                                name: name.clone(),
                                description: None,
                                channel: None,
                            });
                            notify("Creating list", name.clone(), window, cx);
                        }
                        FilesPageEvent::PopOut => {
                            this.pop_out_files(window, cx);
                        }
                    },
                ),
                cx.subscribe(
                    &canvas_search,
                    |this: &mut App, input, event: &InputEvent, cx| {
                        if let InputEvent::Change = event {
                            this.canvas_query = input.read(cx).value().to_string();
                            cx.notify();
                        }
                    },
                ),
                cx.subscribe(
                    &list_search,
                    |this: &mut App, input, event: &InputEvent, cx| {
                        if let InputEvent::Change = event {
                            this.list_query = input.read(cx).value().to_string();
                            cx.notify();
                        }
                    },
                ),
                cx.subscribe_in(
                    &huddles,
                    window,
                    |this: &mut App, _, event: &HuddlesEvent, window, cx| match event {
                        HuddlesEvent::Open(channel) => {
                            this.select(channel.clone(), cx);
                            this.huddle_open = this.huddle_here();
                            cx.notify();
                        }
                        HuddlesEvent::Join(channel) => {
                            this.huddles_open = false;
                            this.select(channel.clone(), cx);
                            this.start_huddle(window, cx);
                            cx.notify();
                        }
                        HuddlesEvent::NewHuddle => {
                            this.huddles_open = false;
                            this.start_huddle(window, cx);
                            cx.notify();
                        }
                        HuddlesEvent::OpenThread(channel, ts) => {
                            this.select(channel.clone(), cx);
                            this.open_thread(channel.clone(), ts.clone(), cx);
                            cx.notify();
                        }
                        HuddlesEvent::Close => {
                            this.huddles_open = false;
                            cx.notify();
                        }
                    },
                ),
                cx.subscribe_in(
                    &directory,
                    window,
                    |this: &mut App, _, event: &DirectoryEvent, window, cx| match event {
                        DirectoryEvent::OpenProfile(user) => this.open_profile(user.clone(), cx),
                        DirectoryEvent::Join(channel) => {
                            if let Some(joined) = this.fixtures.conversation(channel).cloned() {
                                this.sidebar
                                    .update(cx, |sidebar, cx| sidebar.add_channel(joined, cx));
                            }
                        }
                        DirectoryEvent::Open(channel) => this.select(channel.clone(), cx),
                        DirectoryEvent::CreateUsergroup {
                            name,
                            handle,
                            description,
                        } => {
                            if !this.fixtures.sample {
                                this.bridge.send(Command::CreateUsergroup {
                                    name: name.clone(),
                                    handle: handle.clone(),
                                    description: description.clone(),
                                });
                            }
                            notify(
                                "User group created",
                                if handle.is_empty() {
                                    name.clone()
                                } else {
                                    format!("{name} (@{handle})")
                                },
                                window,
                                cx,
                            );
                        }
                        DirectoryEvent::Close => {
                            this.directory_open = false;
                            cx.notify();
                        }
                    },
                ),
                cx.subscribe_in(
                    &tools_page,
                    window,
                    |this: &mut App, _, event: &crate::tools_page::ToolsPageEvent, window, cx| {
                        match event {
                            crate::tools_page::ToolsPageEvent::OpenAppDm(user) => {
                                let dm = this
                                    .fixtures
                                    .dms
                                    .iter()
                                    .find(|c| c.user.as_ref() == Some(user))
                                    .map(|c| c.id.clone());
                                if let Some(dm) = dm {
                                    this.select(dm, cx);
                                } else if this.bridge.store().is_some() {
                                    this.bridge.send(Command::OpenDm {
                                        users: vec![user.clone()],
                                    });
                                } else {
                                    this.open_profile(user.clone(), cx);
                                }
                            }
                            crate::tools_page::ToolsPageEvent::OpenChannel(channel) => {
                                this.select(channel.clone(), cx);
                            }
                            crate::tools_page::ToolsPageEvent::InsertCommand(cmd) => {
                                this.tools_open = false;
                                this.sidebar.update(cx, |s, cx| s.leave_pages(cx));
                                this.composer.update(cx, |composer, cx| {
                                    composer.load_draft(cmd, cx);
                                    composer.focus_input(window, cx);
                                });
                                cx.notify();
                            }
                            crate::tools_page::ToolsPageEvent::Close => {
                                this.tools_open = false;
                                this.sidebar.update(cx, |s, cx| s.leave_pages(cx));
                                cx.notify();
                            }
                        }
                    },
                ),
                cx.subscribe_in(
                    &details,
                    window,
                    |this: &mut App, _, event: &ChannelDetailsEvent, window, cx| {
                        this.details_event(event, window, cx);
                    },
                ),
                cx.subscribe_in(
                    &compose,
                    window,
                    |this: &mut App, _, event: &NewMessageEvent, window, cx| match event {
                        NewMessageEvent::Send { channel, content } => {
                            this.compose_open = false;
                            this.select(channel.clone(), cx);
                            this.send(content.clone(), window, cx);
                        }
                        NewMessageEvent::SendWithFiles {
                            channel,
                            content,
                            files,
                        } => {
                            this.compose_open = false;
                            this.select(channel.clone(), cx);
                            this.upload_files(
                                channel.clone(),
                                None,
                                content.clone(),
                                files.clone(),
                                window,
                                cx,
                            );
                        }
                        NewMessageEvent::OpenDm { users, content } => {
                            // If one person was picked and an IM already exists, deliver directly.
                            if let [user] = users.as_slice() {
                                if let Some(dm) = this
                                    .fixtures
                                    .dms
                                    .iter()
                                    .find(|dm| {
                                        dm.kind() == ConversationKind::Im
                                            && dm.user.as_ref() == Some(user)
                                    })
                                    .map(|dm| dm.id.clone())
                                {
                                    this.compose_open = false;
                                    this.select(dm, cx);
                                    this.send(content.clone(), window, cx);
                                    return;
                                }
                            }
                            if this.bridge.store().is_some() {
                                this.pending_dm = Some(content.clone());
                                this.bridge.send(Command::OpenDm {
                                    users: users.clone(),
                                });
                            } else {
                                notify(
                                    "Needs a live workspace",
                                    "Sign in to start a new DM or group conversation.",
                                    window,
                                    cx,
                                );
                            }
                        }
                        NewMessageEvent::PickEmoji(position) => {
                            this.open_picker(PickerTarget::ComposeView, *position, window, cx);
                        }
                        NewMessageEvent::PickGif(position) => {
                            this.open_picker_tab(
                                PickerTarget::ComposeView,
                                *position,
                                PickerTab::Gifs,
                                window,
                                cx,
                            );
                        }
                        NewMessageEvent::SlashCommand(invocation) => {
                            this.run_slash(invocation, None, window, cx)
                        }
                        NewMessageEvent::Close => {
                            this.compose_open = false;
                            cx.notify();
                        }
                    },
                ),
            ];
            // Subscribe to every background workspace's EventHub so their unread/mention
            // badges and team metadata stay current while parked.
            for (ws_ix, slot) in parked_bridges.iter_mut().enumerate().skip(1) {
                if let Some(bg_bridge) = slot.as_mut() {
                    subscriptions.push(bg_bridge.subscribe_in(
                        window,
                        cx,
                        move |this: &mut App, event, window, cx| {
                            this.on_workspace_event(ws_ix, event, window, cx);
                        },
                    ));
                }
            }
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(REMINDER_POLL).await;
                    if this
                        .update_in(cx, |app, window, cx| {
                            app.fire_reminders(window, cx);
                            app.expire_focus(window, cx);
                            app.poll_calendar_status(window, cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();

            // A first live launch has no conversations yet; hydrate brings them (reload_live).
            if !channel.as_str().is_empty() {
                bridge.send(Command::OpenConversation(channel.clone()));
            }
            demo(&results, window, cx);
            let parked = fixtures.workspaces.iter().map(|_| None).collect();
            // The fixture-derived feed lights the rail tile before any live fetch lands.
            let unread = activity.read(cx).unread_count();
            let saved_section_prefs = bridge.section_prefs();
            sidebar.update(cx, |sidebar, cx| {
                sidebar.set_activity_unread(unread, cx);
                if let Some(prefs) = saved_section_prefs {
                    sidebar.restore_section_prefs(prefs, cx);
                }
            });

            let (cached_member_counts, cached_app_counts) =
                CoreBridge::parse_channel_counts(&local);
            let cached_bots = fixtures.bots.clone();
            App {
                fixtures,
                bridge,
                parked_bridges,
                live_teams,
                bg_counts,
                bg_activity_unread,
                workspace: 0,
                parked,
                ws: WorkspaceState {
                    chosen,
                    bots: cached_bots,
                    member_counts: cached_member_counts,
                    app_counts: cached_app_counts,
                    ..Default::default()
                },
                channel,
                sidebar,
                messages,
                composer,
                thread,
                search,
                search_menu,
                search_menu_open: false,
                results,
                searching: false,
                browser,
                thread_open: false,
                tab: ConversationTab::Messages,
                huddle,
                huddle_open: false,
                huddle_session: None,
                huddle_dock: None,
                huddle_floating_panel: None,
                _huddle_dock_sub: None,
                huddle_invite: None,
                _huddle_invite_sub: None,
                huddle_alone_music_task: None,
                preferences,
                prefs_window: None,
                applied_theme,
                pane_focus: 2,
                color_mode,
                saved_settings,
                profile,
                profile_open: false,
                self_state: SelfState::default(),
                calendar_status_active: false,
                calendar_sync_ticks: 0,
                status_picker,
                user_menu,
                user_menu_open: false,
                switcher,
                emoji_picker,
                picker: None,
                profile_hover: hover_card::Lifecycle::default(),
                remind_menu: None,
                image_viewer: None,
                drafts,
                drafts_open: false,
                later,
                later_open: false,
                threads,
                threads_open: false,
                unreads,
                unreads_open: false,
                unreads_fetched: HashSet::new(),
                activity,
                activity_open: false,
                files_page,
                huddles,
                huddles_open: false,
                directory,
                directory_open: false,
                tools_page,
                tools_open: false,
                compose,
                compose_open: false,
                details,
                pending_dm: None,
                pins_open: false,
                last_right_pane: None,
                canvas_search,
                canvas_query: String::new(),
                list_search,
                list_query: String::new(),
                list_sort: None,
                app_home_kit,
                fixture_bookmarks: HashMap::new(),
                os: Rc::new(platform::current()),
                badge: None,
                badge_label: None,
                last_status_bar: None,
                window_active: window.is_window_active(),
                recent_notified_tags: Vec::new(),
                window_title: String::new(),
                window_state_save: gpui::Task::ready(()),
                history_generation: 0,
                thread_generation: 0,
                reload_generation: 0,
                reload_task: gpui::Task::ready(()),
                open_members: None,
                members_hover: (false, false),
                channel_people: None,
                connect_orgs: Vec::new(),
                back: Vec::new(),
                forward: Vec::new(),
                recent: restored_recent,
                menu_bar: None,
                typing: HashMap::new(),
                typing_expiry: gpui::Task::ready(()),
                sound: None,
                sound_fetch: gpui::Task::ready(()),
                sound_choice: None,
                focus,
                update_state: crate::updater::UpdateState::Idle,
                available_update: None,
                update_dismissed: None,
                workspace_subscriptions,
                _subscriptions: subscriptions,
            }
        });
        // Route member-hover-card trigger reports (mention pills, author names) to this root.
        hover_card::init(&app, cx);
        // Clicking a macOS notification banner activates the window, opens its conversation,
        // and scrolls to + highlights the triggering message.
        let notif_app = app.downgrade();
        let notif_window = window.window_handle();
        cx.on_system_notification_response(move |response, cx| {
            let Some((ws_opt, channel, ts)) = parse_notification_target(&response.tag) else {
                return;
            };
            cx.activate(true);
            let _ = notif_window.update(cx, |_, window, cx| {
                window.activate_window();
                let _ = notif_app.update(cx, |app, cx| {
                    if let Some(ws_ix) = ws_opt {
                        if ws_ix != app.workspace {
                            app.switch_workspace(ws_ix, window, cx);
                        }
                    }
                    app.select(channel, cx);
                    app.messages.update(cx, |list, cx| list.reveal(&ts, cx));
                });
            });
        });
        // Close-to-tray (`F-SHELL-141`): when enabled, closing the main window hides the app
        // to the menu bar instead of terminating. Otherwise start leaving any active huddle
        // immediately so teardown is already underway before exit.
        let close_app = app.downgrade();
        window.on_window_should_close(cx, move |_window, cx| {
            let keep_in_tray = close_app
                .upgrade()
                .is_some_and(|a| a.read(cx).preferences.read(cx).settings().close_to_tray);
            if keep_in_tray {
                cx.hide();
                false
            } else {
                if let Some(a) = close_app.upgrade() {
                    a.update(cx, |app, cx| {
                        if app.huddle_session.is_some() {
                            app.leave_huddle(cx);
                        }
                    });
                }
                true
            }
        });
        // Graceful app quit cleanup: whenever Slackless quits (Cmd+Q, Menu Quit, Status Bar Quit,
        // Dock Quit, OS logout, or SIGTERM/SIGINT), send `Command::Shutdown` to every live core
        // bridge and wait briefly for active huddles to flush Chime `LEAVE` before exit.
        let quit_app = app.downgrade();
        let quit_sub = cx.on_app_quit(move |cx| {
            let mut tasks: Vec<Box<dyn FnOnce() + Send>> = Vec::new();
            if let Some(a) = quit_app.upgrade() {
                let app = a.read(cx);
                if let Some(t) = app.bridge.shutdown_task() {
                    tasks.push(t);
                }
                for parked in app.parked_bridges.iter().flatten() {
                    if let Some(t) = parked.shutdown_task() {
                        tasks.push(t);
                    }
                }
            }
            let bg = cx.background_executor().clone();
            async move {
                if tasks.is_empty() {
                    return;
                }
                let (done_tx, done_rx) = futures::channel::oneshot::channel::<()>();
                let _ = std::thread::Builder::new()
                    .name("slackless-shutdown".into())
                    .spawn(move || {
                        for task in tasks {
                            task();
                        }
                        let _ = done_tx.send(());
                    });
                let timeout = bg.timer(Duration::from_millis(1500));
                futures::pin_mut!(done_rx);
                futures::pin_mut!(timeout);
                let _ = futures::future::select(done_rx, timeout).await;
            }
        });
        app.update(cx, |app, _| {
            app._subscriptions.push(quit_sub);
        });
        // Event-driven actions clicked in the macOS menu-bar status item (`F-SHELL-168`, zero polling).
        {
            use futures::StreamExt as _;
            let (status_tx, mut status_rx) = futures::channel::mpsc::unbounded::<StatusBarAction>();
            platform::install_status_bar_action_monitor(move |action| {
                let _ = status_tx.unbounded_send(action);
            });
            let status_app = app.downgrade();
            let status_window = window.window_handle();
            cx.spawn(async move |cx| {
                while let Some(action) = status_rx.next().await {
                    match action {
                        StatusBarAction::ShowWindow => {
                            let _ = status_window.update(cx, |_, window, cx| {
                                cx.activate(true);
                                window.activate_window();
                            });
                        }
                        StatusBarAction::SwitchWorkspace(ix) => {
                            let _ = status_window.update(cx, |_, window, cx| {
                                cx.activate(true);
                                window.activate_window();
                                let _ = status_app.update(cx, |app, cx| {
                                    app.switch_workspace(ix, window, cx);
                                });
                            });
                        }
                        StatusBarAction::ToggleDnd => {
                            let _ = status_window.update(cx, |_, _, cx| {
                                let _ = status_app.update(cx, |app, cx| {
                                    if app.self_state.paused() {
                                        app.self_state.paused_until = None;
                                        if !app.fixtures.sample {
                                            app.bridge.send(Command::EndDndSnooze);
                                        }
                                    } else {
                                        app.self_state.paused_until =
                                            Some(crate::messages::now_secs() + 3_600);
                                        if !app.fixtures.sample {
                                            app.bridge.send(Command::SetDndSnooze { minutes: 60 });
                                        }
                                    }
                                    app.push_self_state(cx);
                                    app.sync_badge(cx);
                                });
                            });
                        }
                        StatusBarAction::Quit => {
                            cx.update(|cx| cx.quit());
                            break;
                        }
                    }
                }
            })
            .detach();
        }
        // Catch macOS swipe-based mouse Back/Forward events that GPUI drops (`phase == None`).
        {
            use futures::StreamExt as _;
            let (nav_tx, mut nav_rx) = futures::channel::mpsc::unbounded::<MouseNavDirection>();
            platform::install_mouse_nav_monitor(move |dir| {
                let _ = nav_tx.unbounded_send(dir);
            });
            let nav_app = app.downgrade();
            let nav_window = window.window_handle();
            cx.spawn(async move |cx| {
                while let Some(dir) = nav_rx.next().await {
                    let _ = nav_window.update(cx, |_, window, cx| {
                        let _ = nav_app.update(cx, |app, cx| match dir {
                            MouseNavDirection::Back => app.go_back(window, cx),
                            MouseNavDirection::Forward => app.go_forward(window, cx),
                        });
                    });
                }
            })
            .detach();
        }
        // Auto-leave active native huddle when the macOS screen locks (`F-COLLAB-059`).
        {
            use futures::StreamExt as _;
            let (lock_tx, mut lock_rx) = futures::channel::mpsc::unbounded::<()>();
            platform::install_screen_lock_monitor(move || {
                let _ = lock_tx.unbounded_send(());
            });
            let lock_app = app.downgrade();
            cx.spawn(async move |cx| {
                while lock_rx.next().await.is_some() {
                    let _ = lock_app.update(cx, |app, cx| {
                        if app.huddle_session.is_some()
                            && app.preferences.read(cx).settings().huddle_leave_on_lock
                        {
                            app.leave_huddle(cx);
                        }
                    });
                }
            })
            .detach();
        }
        app.update(cx, |app, cx| {
            app.push_self_state(cx);
            app.sync_saved(cx);
            app.sync_later_count(cx);
            app.sync_badge(cx);
            let channel = app.channel.clone();
            // Cold-launch frame-0 preload: read the initial channel's newest viewport page
            // synchronously (80 msgs on the read-only WAL connection, <1ms) so frame 0 renders
            // immediately without a redundant second SQLite read right after.
            let initial_history = app.bridge.history_sync(&channel);
            if !initial_history.is_empty() {
                app.resolve_named_users(&initial_history);
                let newest = initial_history.last().map(|m| m.ts.clone());
                let settings = app.preferences.read(cx).settings().clone();
                let mark_read_mode = settings.mark_read;
                app.messages.update(cx, |list, cx| {
                    list.set_conversation(channel.clone(), cx);
                    list.set_history(channel.clone(), initial_history, cx);
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
            } else {
                app.load_history(&channel, cx);
            }
            app.load_draft(cx);
            app.sync_composer_placeholder(cx);
            app.update_menus(cx);
        });
        // One-shot startup auto-update check (`F-SHELL-163`) on stamped CalVer `.app` releases
        // only — skipped on local dev builds (`0.1.0` / `-dev`) and unbundled runs.
        if !app.read(cx).fixtures.sample
            && crate::updater::current_app_bundle().is_some()
            && !crate::updater::is_dev_build()
        {
            let upd_app = app.downgrade();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(4)).await;
                let _ = upd_app.update(cx, |app, cx| {
                    app.check_for_updates(false, cx);
                });
            })
            .detach();
        }
        app
    }
}
