//! Root `App` initialization (`App::new` / `App::new_multi`), subscriptions, and startup hydration.

use super::*;

type ThemeKey = (SlackTheme, Palette, bool, bool, bool);

/// Shared feature views created once per window (shared across workspace switches).
struct FeatureViews {
    search: Entity<TextareaState>,
    search_menu: Entity<SearchMenu>,
    composer: Entity<Composer>,
    preferences: Entity<Preferences>,
    status_picker: Entity<StatusPicker>,
    user_menu: Entity<UserMenu>,
    emoji_picker: Entity<EmojiPicker>,
    drafts: Entity<DraftsView>,
    later: Entity<LaterView>,
    threads: Entity<ThreadsView>,
    unreads: Entity<UnreadsView>,
    catchup: Entity<CatchUpView>,
    activity: Entity<ActivityFeed>,
    files_page: Entity<FilesPage>,
    huddles: Entity<HuddlesView>,
    directory: Entity<DirectoryView>,
    tools_page: Entity<crate::tools_page::ToolsPage>,
    compose: Entity<NewMessageView>,
    details: Entity<ChannelDetails>,
    canvas_search: Entity<TextareaState>,
    list_search: Entity<TextareaState>,
}

fn bind_app_keybindings(cx: &mut Context<App>) {
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
        KeyBinding::new("cmd-shift-b", OpenCatchUp, None),
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
        KeyBinding::new("ctrl-shift-u", CrossWorkspaceSummary, None),
        KeyBinding::new("ctrl-shift-r", ReplayFocusedMessage, None),
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
        "ctrl-1", "ctrl-2", "ctrl-3", "ctrl-4", "ctrl-5", "ctrl-6", "ctrl-7", "ctrl-8", "ctrl-9",
    ];
    cx.bind_keys(
        (tab_keys.into_iter().enumerate())
            .map(|(ix, keys)| KeyBinding::new(keys, SelectConversationTab(ix), None)),
    );
}

fn restore_initial_preferences(
    fixtures: &Fixtures,
    bridge: &CoreBridge,
    local: &HashMap<String, String>,
    window: &mut Window,
    cx: &mut Context<App>,
) -> (
    Entity<Preferences>,
    ThemeKey,
    Option<AppearanceMode>,
    Option<crate::preferences::Settings>,
) {
    let preferences = Preferences::new(window, cx);
    // Restore persisted Preferences settings first, then overlay the zoom/font/color_mode
    // values `main` installed before the first frame.
    let mut restored_settings = SETTINGS.read(local);
    let has_local_quick_pref = SETTINGS
        .raw(local)
        .is_some_and(|raw| raw.contains("\"show_quick_reactions\""));
    if !has_local_quick_pref {
        let mut base = restored_settings.unwrap_or_default();
        base.show_quick_reactions = !fixtures.quick_reactions.is_empty();
        if !fixtures.quick_reactions.is_empty() {
            let mut emojis = crate::actions::DEFAULT_QUICK_REACTIONS.map(String::from);
            for (ix, name) in fixtures.quick_reactions.iter().take(3).enumerate() {
                emojis[ix] = name.clone();
            }
            base.quick_reaction_emojis = emojis;
        }
        restored_settings = Some(base);
    }
    if let Some(settings) = restored_settings.clone() {
        crate::preferences::set_display_prefs(&settings, cx);
        crate::composer::set_outgoing_guard_prefs(
            settings.strip_tracking_params,
            settings.warn_before_sending_secrets,
            cx,
        );
        preferences.update(cx, |prefs, cx| prefs.load_settings(settings, window, cx));
    } else {
        {
            let s = preferences.read(cx).settings();
            crate::composer::set_outgoing_guard_prefs(
                s.strip_tracking_params,
                s.warn_before_sending_secrets,
                cx,
            );
        }
    }
    // `open_url`'s link policy holds from the first click, not from the first Preferences change.
    let settings = preferences.read(cx).settings();
    crate::zoom::set_strip_tracking_params(settings.strip_tracking_params);
    crate::zoom::set_protocol_allowlist(&settings.protocol_allowlist);
    let factor = cx.global::<crate::zoom::Zoom>().factor();
    let zoom = (crate::zoom::FACTORS.iter())
        .position(|level| *level == factor)
        .unwrap_or(crate::zoom::ACTUAL_SIZE);
    preferences.update(cx, |prefs, cx| prefs.set_zoom(zoom, cx));
    // Live: the remembered Color Mode over Slack's color theme (or persisted palette),
    // as `main` installed it before the first frame.
    let color_mode = bridge
        .store()
        .map(|_| COLOR_MODE.read(local).unwrap_or(AppearanceMode::System));
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
    (preferences, applied_theme, color_mode, saved_settings)
}

fn wire_window_and_input_subscriptions(
    window: &mut Window,
    cx: &mut Context<App>,
) -> Vec<Subscription> {
    let this = cx.weak_entity();
    let viewer_keys = cx.weak_entity();
    let menu_keys = cx.weak_entity();
    let card_keys = cx.weak_entity();
    let unreads_keys = cx.weak_entity();
    vec![
        cx.observe_window_bounds(window, |this, window, cx| {
            this.save_window_state(window, cx);
        }),
        cx.observe_window_activation(window, |this, window, cx| {
            this.window_active = window.is_window_active();
            if this.window_active {
                let (no_bg_mark_read, mark_read) = {
                    let settings = this.preferences.read(cx).settings();
                    (settings.no_bg_mark_read, settings.mark_read)
                };
                if no_bg_mark_read && mark_read != 2 {
                    if this.page == MainPage::Threads {
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
                if !app.search_menu_open || !app.search.read(cx).focus_handle(cx).is_focused(window)
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
        cx.intercept_keystrokes(move |event, window, cx| {
            if event.input_preference == InputPreference::CharacterInput {
                return;
            }
            let _ = viewer_keys.update(cx, |app, cx| {
                let Some(viewer) = app.image_viewer.clone() else {
                    return;
                };
                match viewer.update(cx, |v, cx| v.handle_keystroke(&event.keystroke, window, cx)) {
                    crate::image_viewer::ViewerKeyOutcome::Close => {
                        app.close_image_viewer(window, cx);
                        cx.stop_propagation();
                    }
                    crate::image_viewer::ViewerKeyOutcome::Handled => {
                        cx.stop_propagation();
                    }
                    crate::image_viewer::ViewerKeyOutcome::Ignored => {}
                }
            });
        }),
        // While the profile menu is open, Escape closes it, submenu included —
        // Slack closes the whole menu, not just the open submenu.
        cx.intercept_keystrokes(move |event, _, cx| {
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
        }),
        // While the member hover card is open, Escape closes it, like Slack's hovercard.
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
                if app.page != MainPage::Unreads
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
        cx.observe_window_appearance(window, |this: &mut App, window, cx| {
            this.apply_preferences(window, cx);
        }),
    ]
}

fn wire_feature_view_subscriptions(
    views: &FeatureViews,
    window: &mut Window,
    cx: &mut Context<App>,
) -> Vec<Subscription> {
    vec![
        cx.subscribe_in(
            &views.search,
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
            &views.search_menu,
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
                    if let Some(write) = this.bridge.remember(&SEARCH_HISTORY_SHOWN, on) {
                        cx.background_executor().spawn(write).detach();
                    }
                }
                SearchMenuEvent::SetFiltersShown(on) => {
                    if let Some(write) = this.bridge.remember(&SEARCH_FILTERS_SHOWN, on) {
                        cx.background_executor().spawn(write).detach();
                    }
                }
                SearchMenuEvent::Dismiss => this.close_search_menu(window, cx),
            },
        ),
        cx.subscribe_in(
            &views.composer,
            window,
            |this: &mut App, _, event: &ComposerEvent, window, cx| match event {
                ComposerEvent::Submit(content) => this.send(content.clone(), window, cx),
                ComposerEvent::SubmitWithFiles { content, files } => {
                    let channel = this.channel.clone();
                    this.upload_files(channel, None, content.clone(), files.clone(), window, cx);
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
                ComposerEvent::EditLast => this
                    .messages
                    .update(cx, |list, cx| list.edit_last_own(window, cx)),
                ComposerEvent::SlashCommand(invocation) => {
                    this.run_slash(invocation, None, window, cx);
                }
                ComposerEvent::Schedule { text, when } => {
                    let channel = this.channel.clone();
                    this.schedule_at(
                        messaging::ScheduleSpec {
                            channel,
                            thread_ts: None,
                            text: text.clone(),
                            post_at: when.at(now_secs()),
                            when_label: when.label().to_lowercase(),
                        },
                        window,
                        cx,
                    );
                }
                ComposerEvent::ScheduleCustom(text) => {
                    let channel = this.channel.clone();
                    this.open_custom_schedule(channel, None, text.clone(), window, cx);
                }
                ComposerEvent::Typing => this.bridge.send(MessagingCommand::Typing {
                    channel: this.channel.clone(),
                    thread_ts: None,
                }),
                ComposerEvent::Cancel => {}
            },
        ),
        cx.subscribe_in(
            &views.emoji_picker,
            window,
            |this: &mut App, _, event: &EmojiPickerEvent, window, cx| match event {
                EmojiPickerEvent::Pick(choice) => this.pick_emoji(choice, window, cx),
                EmojiPickerEvent::PickGif(gif) => this.pick_gif(gif, window, cx),
                EmojiPickerEvent::RunGiphy(query) => this.run_giphy(query, window, cx),
                EmojiPickerEvent::SearchGifs(query) => {
                    if !this.fixtures.sample {
                        this.bridge.send(CollabCommand::SearchGifs {
                            query: query.clone(),
                        });
                    }
                }
                EmojiPickerEvent::ToneChanged(tone) => {
                    if let Some(write) = this.bridge.remember(&EMOJI_TONE, tone) {
                        cx.background_executor().spawn(write).detach();
                    }
                    if !this.fixtures.sample {
                        this.bridge.send(WorkspaceCommand::SetUserPref {
                            name: "preferred_skin_tone".to_owned(),
                            value: (*tone).to_string(),
                        });
                    }
                }
            },
        ),
        cx.subscribe_in(
            &views.preferences,
            window,
            |this: &mut App, _, event: &PreferencesEvent, window, cx| match event {
                PreferencesEvent::Changed => this.apply_preferences(window, cx),
                PreferencesEvent::CompactStorage => this.compact_storage(window, cx),
                PreferencesEvent::ClearMediaCache => {
                    this.clear_media_caches("Media cache cleared", window, cx);
                }
                PreferencesEvent::ExportNetLog => {
                    let json = slackless_core::api::net_log_export_json();
                    let configured = this.preferences.read(cx).settings().download_dir.clone();
                    let dir = if !configured.is_empty() {
                        std::path::PathBuf::from(configured)
                    } else {
                        std::env::var_os("HOME")
                            .map(|h| std::path::PathBuf::from(h).join("Downloads"))
                            .unwrap_or_else(std::env::temp_dir)
                    };
                    let path = dir.join("slackless-net-log.json");
                    window
                        .spawn(cx, async move |cx| {
                            let write_path = path.clone();
                            let result = cx
                                .background_executor()
                                .spawn(async move { std::fs::write(&write_path, json) })
                                .await;
                            let _ = cx.update(|window, cx| match result {
                                Ok(()) => notify(
                                    "Net log exported",
                                    format!("Saved to {}", path.display()),
                                    window,
                                    cx,
                                ),
                                Err(err) => {
                                    notify("Net log export failed", err.to_string(), window, cx)
                                }
                            });
                        })
                        .detach();
                }
                PreferencesEvent::TestSpeaker(output_device) => {
                    if this.fixtures.sample {
                        #[cfg(not(test))]
                        {
                            let dev = output_device.clone();
                            std::thread::spawn(move || {
                                let _ =
                                    slackless_core::huddle::play_speaker_test_tone(dev.as_deref());
                            });
                        }
                        #[cfg(test)]
                        {
                            let _ = output_device;
                        }
                    } else {
                        this.bridge.send(HuddleCommand::TestSpeaker {
                            output_device: output_device.clone(),
                        });
                    }
                }
                PreferencesEvent::CheckForUpdates => this.check_for_updates(true, cx),
                PreferencesEvent::InstallUpdate => this.install_available_update(cx),
                PreferencesEvent::OpenStorageDialog => this.open_storage_dialog(window, cx),
                PreferencesEvent::StartWorkspaceBackfill => {
                    this.start_workspace_backfill(window, cx);
                }
                PreferencesEvent::PauseBackfill => this.pause_backfill(window, cx),
                PreferencesEvent::ResumeBackfill => this.resume_backfill(window, cx),
            },
        ),
        cx.subscribe_in(
            &views.status_picker,
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
                                        ((until.saturating_sub(now) + 59) / 60).clamp(1, 1440)
                                            as u32
                                    })
                                    .unwrap_or(60);
                                this.self_state.paused_until = Some(now + i64::from(mins) * 60);
                                this.bridge
                                    .send(WorkspaceCommand::SetDndSnooze { minutes: mins });
                            } else if this.self_state.paused() {
                                this.self_state.paused_until = None;
                                this.bridge.send(WorkspaceCommand::EndDndSnooze);
                            }
                        }
                        this.push_self_state(cx);
                        this.sync_badge(cx);
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
                    StatusEvent::RemoveScheduled(target) => {
                        if let Some(pos) = this
                            .self_state
                            .scheduled_statuses
                            .iter()
                            .position(|s| s == target)
                        {
                            this.self_state.scheduled_statuses.remove(pos);
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
            &views.user_menu,
            window,
            |this: &mut App, _, event: &UserMenuEvent, window, cx| {
                this.user_menu_event(event, window, cx);
            },
        ),
        cx.subscribe_in(
            &views.drafts,
            window,
            |this: &mut App, _, event: &DraftsEvent, window, cx| match event {
                DraftsEvent::Close => {
                    if this.page == MainPage::Drafts {
                        this.page = MainPage::Channel;
                    }
                    cx.notify();
                }
                DraftsEvent::Open { workspace, channel } => {
                    this.switch_workspace(*workspace, window, cx);
                    this.select(channel.clone(), cx);
                }
                DraftsEvent::SendNow(entry) => this.deliver(entry.clone(), window, cx),
                DraftsEvent::CancelScheduled(entry) => {
                    if let Some(id) = entry.id.clone()
                        && let Some(bridge) = this.workspace_bridge(entry.workspace)
                    {
                        bridge.send(MessagingCommand::DeleteScheduledMessage {
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
            &views.later,
            window,
            |this: &mut App, _, event: &LaterEvent, window, cx| match event {
                LaterEvent::PopOut => this.pop_out_later(window, cx),
                LaterEvent::Close => {
                    if this.page == MainPage::Later {
                        this.page = MainPage::Channel;
                    }
                    cx.notify();
                }
                LaterEvent::Open {
                    workspace,
                    channel,
                    ts,
                } => {
                    if channel.as_str() == crate::later::PERSONAL_TODO_CHANNEL {
                        return;
                    }
                    this.switch_workspace(*workspace, window, cx);
                    if this.fixtures.conversation(channel).is_some() {
                        this.select(channel.clone(), cx);
                        this.messages.update(cx, |list, cx| list.reveal(ts, cx));
                    }
                }
                LaterEvent::Unsaved {
                    workspace,
                    channel,
                    ts,
                } => {
                    if channel.as_str() == crate::later::PERSONAL_TODO_CHANNEL {
                        return;
                    }
                    if *workspace == this.workspace {
                        this.messages
                            .update(cx, |list, cx| list.set_saved(ts, false, cx));
                        this.thread
                            .update(cx, |thread, cx| thread.set_saved(ts, false, cx));
                        this.threads
                            .update(cx, |threads, cx| threads.set_saved(ts, false, cx));
                        this.sync_saved(cx);
                        this.bridge.send(MessagingCommand::DeleteSavedItem {
                            channel: channel.clone(),
                            ts: ts.clone(),
                        });
                    } else if let Some(bridge) = this.workspace_bridge(*workspace) {
                        bridge.send(MessagingCommand::DeleteSavedItem {
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
                    if channel.as_str() == crate::later::PERSONAL_TODO_CHANNEL {
                        return;
                    }
                    let cmd = MessagingCommand::UpdateSavedItem {
                        channel: channel.clone(),
                        ts: ts.clone(),
                        mark: *mark,
                        date_due: *date_due,
                    };
                    if let Some(bridge) = this.workspace_bridge(*workspace) {
                        bridge.send(cmd);
                    }
                }
                LaterEvent::ClearCompleted { workspace } => {
                    if let Some(bridge) = this.workspace_bridge(*workspace) {
                        bridge.send(MessagingCommand::ClearCompletedSaved);
                    }
                }
                LaterEvent::Changed => this.sync_later_count(cx),
            },
        ),
        cx.subscribe_in(
            &views.threads,
            window,
            |this: &mut App, _, event: &ThreadsEvent, window, cx| match event {
                ThreadsEvent::Open { channel, thread_ts } => {
                    this.select(channel.clone(), cx);
                    this.open_thread(channel.clone(), thread_ts.clone(), cx);
                }
                ThreadsEvent::OpenListItem { list_id, record_id } => {
                    if this.fixtures.sample || list_id == "fixture" {
                        crate::lists::select_fixture_record(record_id, cx);
                        this.close_full_column_views(cx);
                        this.select_tab(ConversationTab::List, cx);
                    } else {
                        if let Some(Ok(data)) = this.ws.lists.get(list_id) {
                            crate::lists::select_list_record(list_id, record_id, data, cx);
                        }
                        this.open_live_list(list_id.clone(), cx);
                    }
                }
                ThreadsEvent::OpenLink(target) => {
                    this.open_link(target.clone(), window, cx);
                }
                ThreadsEvent::ExpandThread { channel, thread_ts } => {
                    this.bridge.send(MessagingCommand::OpenThread {
                        channel: channel.clone(),
                        thread_ts: thread_ts.clone(),
                    });
                    if let Some(read) = this.bridge.thread(channel.clone(), thread_ts.clone()) {
                        let epoch = this.workspace_epoch;
                        let (ch, ts) = (channel.clone(), thread_ts.clone());
                        cx.spawn(async move |this, cx| {
                            let msgs = cx.background_executor().spawn(read).await;
                            let _ = this.update(cx, |app, cx| {
                                if app.workspace_epoch != epoch {
                                    return;
                                }
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
                    this.submit(
                        messaging::OutgoingMessage {
                            channel: channel.clone(),
                            thread_ts: Some(thread_ts.clone()),
                            broadcast: *broadcast,
                            content: content.clone(),
                            files: None,
                            origin: messaging::OutgoingOrigin::ThreadsView,
                        },
                        window,
                        cx,
                    );
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
                    this.handle_message_action(
                        MessageTarget {
                            surface: MessageSurface::Threads,
                            channel: channel.clone(),
                            thread_ts: Some(thread_ts.clone()),
                        },
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
                    this.mark_thread_read(channel.clone(), thread_ts.clone(), ts.clone(), true, cx);
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
                    this.sync_badge(cx);
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
            &views.unreads,
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
                    let msg = messaging::OutgoingMessage {
                        channel: channel.clone(),
                        thread_ts: None,
                        broadcast: false,
                        content: composer::content_of(text, &this.fixtures),
                        files: None,
                        origin: messaging::OutgoingOrigin::External,
                    };
                    if this.submit(msg, window, cx) {
                        this.unreads
                            .update(cx, |view, cx| view.mark_read(channel, cx));
                        actions::notify("Reply sent", this.label(channel), window, cx);
                    }
                }
                UnreadsEvent::MessageAction {
                    channel,
                    message,
                    action,
                } => {
                    this.handle_message_action(
                        MessageTarget {
                            surface: MessageSurface::Unreads,
                            channel: channel.clone(),
                            thread_ts: None,
                        },
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
            &views.activity,
            window,
            |this: &mut App, _, event: &ActivityEvent, window, cx| match event {
                ActivityEvent::Open { channel, thread_ts } => {
                    this.select(channel.clone(), cx);
                    if let Some(thread_ts) = thread_ts {
                        this.open_thread(channel.clone(), thread_ts.clone(), cx);
                    }
                }
                ActivityEvent::Mark { entries, read } => {
                    this.bridge.send(MessagingCommand::MarkActivity {
                        entries: entries.clone(),
                        read: *read,
                    });
                }
                ActivityEvent::Retry => this.fetch_activity(cx),
                ActivityEvent::PopOut => this.pop_out_activity(window, cx),
            },
        ),
        cx.observe(&views.activity, |this: &mut App, _, cx| {
            // The rail tile and the dock badge follow the feed's unread rows.
            let unread = this.activity.read(cx).unread_count();
            this.sidebar
                .update(cx, |sidebar, cx| sidebar.set_activity_unread(unread, cx));
            this.sync_badge(cx);
            cx.notify();
        }),
        cx.subscribe_in(
            &views.files_page,
            window,
            |this: &mut App, _, event: &FilesPageEvent, window, cx| match event {
                FilesPageEvent::Load(filter) => {
                    this.bridge.send(CollabCommand::LoadFiles(filter.clone()));
                }
                FilesPageEvent::LoadExtras => {
                    this.bridge.send(CollabCommand::LoadFilesExtras);
                }
                FilesPageEvent::SaveFile { file_id, name } => {
                    this.bridge.send(MessagingCommand::SaveFile {
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
                    this.bridge.send(CollabCommand::SetFileFavorite {
                        file_id: file_id.clone(),
                        collection_id: collection_id.clone(),
                        starred: *starred,
                    });
                }
                FilesPageEvent::CreateCollection { name } => {
                    this.bridge.send(CollabCommand::CreateFileCollection {
                        name: name.clone(),
                        emoji: None,
                    });
                    notify("Collection created", name.clone(), window, cx);
                }
                FilesPageEvent::DeleteCollection { collection_id } => {
                    this.bridge.send(CollabCommand::DeleteFileCollection {
                        collection_id: collection_id.clone(),
                    });
                    notify("Collection deleted", "Removed file collection.", window, cx);
                }
                FilesPageEvent::RestoreFile { file_id, title } => {
                    this.bridge.send(CollabCommand::RestoreDeletedFile {
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
                    this.bridge.send(CollabCommand::CreateCanvas {
                        title: title.clone(),
                        markdown: format!("# {title}\n\n"),
                        channel: None,
                    });
                    notify("Creating canvas", title.clone(), window, cx);
                }
                FilesPageEvent::CreateList { name } => {
                    this.bridge.send(CollabCommand::CreateList {
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
            &views.canvas_search,
            |this: &mut App, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.canvas_query = input.read(cx).value().to_string();
                    cx.notify();
                }
            },
        ),
        cx.subscribe(
            &views.list_search,
            |this: &mut App, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.list_query = input.read(cx).value().to_string();
                    cx.notify();
                }
            },
        ),
        cx.subscribe_in(
            &views.huddles,
            window,
            |this: &mut App, _, event: &HuddlesEvent, window, cx| match event {
                HuddlesEvent::Open(channel) => {
                    this.select(channel.clone(), cx);
                    cx.notify();
                }
                HuddlesEvent::Join(channel) => {
                    this.select(channel.clone(), cx);
                    this.start_huddle(window, cx);
                    cx.notify();
                }
                HuddlesEvent::NewHuddle => {
                    if this.page == MainPage::Huddles {
                        this.page = MainPage::Channel;
                    }
                    this.start_huddle(window, cx);
                    cx.notify();
                }
                HuddlesEvent::OpenThread(channel, ts) => {
                    this.select(channel.clone(), cx);
                    this.open_thread(channel.clone(), ts.clone(), cx);
                    cx.notify();
                }
                HuddlesEvent::OpenTranscript(call_id, channel) => {
                    this.open_huddle_transcript(
                        call_id.clone(),
                        channel.clone(),
                        None,
                        None,
                        window,
                        cx,
                    );
                }
                HuddlesEvent::Close => {
                    if this.page == MainPage::Huddles {
                        this.page = MainPage::Channel;
                    }
                    cx.notify();
                }
            },
        ),
        cx.subscribe_in(
            &views.catchup,
            window,
            |this: &mut App, _, event: &CatchUpEvent, window, cx| match event {
                CatchUpEvent::Close => {
                    if this.page == MainPage::CatchUp {
                        this.page = MainPage::Channel;
                        this.sidebar.update(cx, |s, cx| s.set_home_page(None, cx));
                    }
                    cx.notify();
                }
                CatchUpEvent::OpenChannel {
                    workspace,
                    channel,
                    ts,
                } => {
                    this.open_catchup_channel(workspace, channel.clone(), ts.clone(), window, cx);
                }
                CatchUpEvent::OpenThread {
                    workspace,
                    channel,
                    thread_ts,
                } => {
                    this.open_catchup_thread(
                        workspace,
                        channel.clone(),
                        thread_ts.clone(),
                        window,
                        cx,
                    );
                }
                CatchUpEvent::OpenHuddleTranscript {
                    workspace,
                    call_id,
                    channel,
                } => {
                    this.open_catchup_huddle(
                        workspace,
                        call_id.clone(),
                        channel.clone(),
                        window,
                        cx,
                    );
                }
                CatchUpEvent::WindowChanged
                | CatchUpEvent::ScopeChanged
                | CatchUpEvent::Refresh => {
                    this.refresh_catchup(cx);
                }
                CatchUpEvent::MarkCaughtUp => {
                    this.mark_catchup_visited(window, cx);
                }
                CatchUpEvent::Summarize => {
                    this.open_summary_for_catchup(window, cx);
                }
            },
        ),
        cx.subscribe_in(
            &views.directory,
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
                        this.bridge.send(WorkspaceCommand::CreateUsergroup {
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
                    if this.page == MainPage::Directory {
                        this.page = MainPage::Channel;
                    }
                    cx.notify();
                }
            },
        ),
        cx.subscribe_in(
            &views.tools_page,
            window,
            |this: &mut App, _, event: &crate::tools_page::ToolsPageEvent, window, cx| match event {
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
                        this.bridge.send(ChannelCommand::OpenDm {
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
                    if this.page == MainPage::Tools {
                        this.page = MainPage::Channel;
                    }
                    this.sidebar.update(cx, |s, cx| s.leave_pages(cx));
                    this.composer.update(cx, |composer, cx| {
                        composer.load_draft(cmd, cx);
                        composer.focus_input(window, cx);
                    });
                    cx.notify();
                }
                crate::tools_page::ToolsPageEvent::Close => {
                    if this.page == MainPage::Tools {
                        this.page = MainPage::Channel;
                    }
                    this.sidebar.update(cx, |s, cx| s.leave_pages(cx));
                    cx.notify();
                }
            },
        ),
        cx.subscribe_in(
            &views.details,
            window,
            |this: &mut App, _, event: &ChannelDetailsEvent, window, cx| {
                this.details_event(event, window, cx);
            },
        ),
        cx.subscribe_in(
            &views.compose,
            window,
            |this: &mut App, _, event: &NewMessageEvent, window, cx| match event {
                NewMessageEvent::Send { channel, content } => {
                    this.select(channel.clone(), cx);
                    this.send(content.clone(), window, cx);
                }
                NewMessageEvent::SendWithFiles {
                    channel,
                    content,
                    files,
                } => {
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
                    if let [user] = users.as_slice()
                        && let Some(dm) = this
                            .fixtures
                            .dms
                            .iter()
                            .find(|dm| {
                                dm.kind() == ConversationKind::Im && dm.user.as_ref() == Some(user)
                            })
                            .map(|dm| dm.id.clone())
                    {
                        this.select(dm, cx);
                        this.send(content.clone(), window, cx);
                        return;
                    }
                    if this.bridge.store().is_some() {
                        this.pending_dm = Some(messaging::PendingDm {
                            users: users.clone(),
                            content: content.clone(),
                        });
                        this.bridge.send(ChannelCommand::OpenDm {
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
                    if this.page == MainPage::Compose {
                        this.page = MainPage::Channel;
                    }
                    cx.notify();
                }
            },
        ),
    ]
}

fn install_os_and_lifecycle_hooks(app: &Entity<App>, window: &mut Window, cx: &mut gpui::App) {
    // Route member-hover-card trigger reports (mention pills, author names) to this root.
    hover_card::init(app, cx);
    // Clicking a macOS notification banner activates the window, opens its conversation,
    // and scrolls to + highlights the triggering message.
    let notif_app = app.downgrade();
    let notif_window = window.window_handle();
    cx.on_system_notification_response(move |response, cx| {
        if is_catchup_notification_tag(&response.tag) {
            cx.activate(true);
            let _ = notif_window.update(cx, |_, window, cx| {
                window.activate_window();
                let _ = notif_app.update(cx, |app, cx| {
                    app.open_catchup(window, cx);
                });
            });
            return;
        }
        let Some((ws_opt, channel, ts)) = parse_notification_target(&response.tag) else {
            return;
        };
        cx.activate(true);
        let _ = notif_window.update(cx, |_, window, cx| {
            window.activate_window();
            let _ = notif_app.update(cx, |app, cx| {
                if let Some(ws_ix) = ws_opt
                    && ws_ix != app.workspace
                {
                    app.switch_workspace(ws_ix, window, cx);
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
    // Dock Quit, OS logout, or SIGTERM/SIGINT), immediately send `Command::Shutdown` to EVERY
    // live core bridge up-front, then join their threads in parallel (huddle owner prioritized)
    // inside GPUI's quit budget so later workspaces never miss Chime `LEAVE`.
    let quit_app = app.downgrade();
    let quit_sub = cx.on_app_quit(move |cx| {
        let mut join_tasks: Vec<Box<dyn FnOnce() + Send>> = Vec::new();
        if let Some(a) = quit_app.upgrade() {
            let app = a.read(cx);
            let owner = app.huddle_owner;
            // Phase 1: signal ALL bridges synchronously before joining any thread.
            app.bridge.send_shutdown();
            for parked in app.slots.iter().filter_map(|slot| slot.bridge.as_ref()) {
                parked.send_shutdown();
            }
            // Phase 2: collect join handles, prioritizing the huddle-owning workspace first.
            if owner == Some(app.workspace)
                && let Some(t) = app.bridge.join_shutdown_task()
            {
                join_tasks.push(t);
            }
            for (ix, slot) in app.slots.iter().enumerate() {
                if owner == Some(ix)
                    && let Some(t) = slot
                        .bridge
                        .as_ref()
                        .and_then(CoreBridge::join_shutdown_task)
                {
                    join_tasks.push(t);
                }
            }
            if owner != Some(app.workspace)
                && let Some(t) = app.bridge.join_shutdown_task()
            {
                join_tasks.push(t);
            }
            for (ix, slot) in app.slots.iter().enumerate() {
                if owner != Some(ix)
                    && let Some(t) = slot
                        .bridge
                        .as_ref()
                        .and_then(CoreBridge::join_shutdown_task)
                {
                    join_tasks.push(t);
                }
            }
        }
        let bg = cx.background_executor().clone();
        async move {
            if join_tasks.is_empty() {
                return;
            }
            let (done_tx, done_rx) = futures::channel::oneshot::channel::<()>();
            let _ = std::thread::Builder::new()
                .name("slackless-shutdown".into())
                .spawn(move || {
                    let handles: Vec<_> = join_tasks
                        .into_iter()
                        .filter_map(|task| std::thread::Builder::new().spawn(task).ok())
                        .collect();
                    for h in handles {
                        let _ = h.join();
                    }
                    let _ = done_tx.send(());
                });
            // Stay inside GPUI's 200ms `SHUTDOWN_TIMEOUT` so `app_will_quit` resolves cleanly
            // without timing out while `slackless-shutdown` flushes Chime `LEAVE`.
            let timeout = bg.timer(Duration::from_millis(160));
            futures::pin_mut!(done_rx);
            futures::pin_mut!(timeout);
            let _ = futures::future::select(done_rx, timeout).await;
        }
    });
    app.update(cx, |app, _| {
        app._subscriptions.push(quit_sub);
    });
    // Event-driven OS monitors (`F-SHELL-168`, mouse Back/Forward, screen lock). Skipped
    // under `#[cfg(test)]` because these install process-global callbacks whose senders would
    // be dropped across parallel `#[gpui::test]` threads, waking another test's scheduler.
    #[cfg(not(test))]
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
                                app.switch_workspace_at(ix, window, cx);
                            });
                        });
                    }
                    StatusBarAction::ToggleDnd => {
                        let _ = status_window.update(cx, |_, _, cx| {
                            let _ = status_app.update(cx, |app, cx| {
                                if app.self_state.paused() {
                                    app.self_state.paused_until = None;
                                    if !app.fixtures.sample {
                                        app.bridge.send(WorkspaceCommand::EndDndSnooze);
                                    }
                                } else {
                                    app.self_state.paused_until =
                                        Some(crate::messages::now_secs() + 3_600);
                                    if !app.fixtures.sample {
                                        app.bridge
                                            .send(WorkspaceCommand::SetDndSnooze { minutes: 60 });
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

        // Catch macOS swipe-based mouse Back/Forward events that GPUI drops (`phase == None`).
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

        // Auto-leave active native huddle when the macOS screen locks (`F-COLLAB-059`).
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
}

fn hydrate_initial_channel(app: &Entity<App>, window: &mut Window, cx: &mut gpui::App) {
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
            let (mark_read_mode, no_bg_mark_read) = {
                let settings = app.preferences.read(cx).settings();
                (settings.mark_read, settings.no_bg_mark_read)
            };
            app.messages.update(cx, |list, cx| {
                list.set_conversation(channel.clone(), cx);
                list.set_history(channel.clone(), initial_history, cx);
                if mark_read_mode == 0 {
                    list.scroll_to_unread_once(cx);
                }
            });
            let allow_mark = mark_read_mode != 2 && (!no_bg_mark_read || app.window_active);
            if allow_mark && let Some(newest) = newest {
                app.mark_read(&channel, newest, cx);
            }
        } else {
            app.load_history(&channel, cx);
        }
        app.load_draft(cx);
        app.sync_composer_placeholder(cx);
        app.update_menus(cx);
        app.sync_window_title_in(window);
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
}

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
            // Slot 0 is shown (its bridge is `bridge`); every other workspace keeps its own
            // bridge in its slot. A snapshot without teams still gets one slot.
            let mut background = bridges.into_iter();
            let teams = if fixtures.rail_workspaces().is_empty() {
                vec![Team::default()]
            } else {
                fixtures.rail_workspaces().to_vec()
            };
            let mut slots: Vec<WorkspaceSlot> = (teams.into_iter().enumerate())
                .map(|(ix, team)| WorkspaceSlot {
                    team,
                    bridge: if ix == 0 { None } else { background.next() },
                    parked: None,
                    state: WorkspaceState::default(),
                    activity_unread: 0,
                })
                .collect();
            let rail = (0..slots.len()).collect();
            // Read all workspace `local_state` keys once (1 SQLite query instead of ~10).
            let local = bridge.all_local();
            // Like Slack, reopen the conversation that was open last; a fresh store falls back
            // to the sidebar's first conversation until the user picks one.
            let restored = LAST_CONVERSATION
                .read(&local)
                .filter(|channel| fixtures.conversation(channel).is_some());
            let chosen = restored.is_some();
            let channel = restored.unwrap_or_else(|| first_conversation(&fixtures));
            let archive = bridge
                .read_store_clone()
                .or_else(|| fixture_archive(&fixtures));
            let views = build_workspace(&fixtures, &slots[0].team, archive, &channel, window, cx);
            let workspace_subscriptions = subscribe_workspace(&views, window, cx);
            let WorkspaceViews {
                sidebar,
                messages,
                thread,
                results,
                browser,
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
            if let Some(history) = SEARCH_HISTORY.read(&local) {
                search_menu.update(cx, |menu, cx| menu.set_history(history, cx));
            }
            if let Some(on) = SEARCH_HISTORY_SHOWN.read(&local) {
                search_menu.update(cx, |menu, cx| menu.set_history_shown(on, cx));
            }
            if let Some(on) = SEARCH_FILTERS_SHOWN.read(&local) {
                search_menu.update(cx, |menu, cx| menu.set_filters_shown(on, cx));
            }
            let restored_recent = RECENT_PLACES
                .read(&local)
                .unwrap_or_default()
                .into_iter()
                .filter(|channel| fixtures.conversation(channel).is_some())
                .collect();
            let (preferences, applied_theme, color_mode, saved_settings) =
                restore_initial_preferences(&fixtures, &bridge, &local, window, cx);
            let status_picker = StatusPicker::new(window, cx);
            let user_menu = UserMenu::new(fixtures.clone(), cx);
            let emoji_picker = EmojiPicker::new(fixtures.clone(), window, cx);
            let persisted_freq = FREQUENT_EMOJI.read(&local);
            let persisted_tone = EMOJI_TONE.read(&local);
            if persisted_freq.is_some() || persisted_tone.is_some() {
                emoji_picker.update(cx, |picker, cx| {
                    picker.restore(persisted_freq, persisted_tone, cx);
                });
            }
            let drafts = DraftsView::new(cx);
            if let Some(saved_drafts) = DRAFTS.read(&local) {
                let entries: Vec<Entry> = saved_drafts
                    .into_iter()
                    .map(|d| Entry {
                        id: None,
                        workspace: 0,
                        channel: d.channel,
                        thread_ts: None,
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
            let catchup = CatchUpView::new(cx);
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
            let modal_stack =
                dialogs::modal::AppModalStack::new(fixtures.clone(), cx.weak_entity(), cx);
            if fixtures.sample {
                later.update(cx, |later, cx| later.seed(0, &fixtures, cx));
            }

            bind_app_keybindings(cx);
            let focus = cx.focus_handle();
            focus.focus(window, cx);

            let feature_views = FeatureViews {
                search,
                search_menu,
                composer,
                preferences,
                status_picker,
                user_menu,
                emoji_picker,
                drafts,
                later,
                threads,
                unreads,
                catchup,
                activity,
                files_page,
                huddles,
                directory,
                tools_page,
                compose,
                details,
                canvas_search,
                list_search,
            };

            let mut subscriptions = wire_window_and_input_subscriptions(window, cx);
            subscriptions.push(bridge.subscribe_in(
                window,
                cx,
                |this: &mut App, event, window, cx| {
                    this.on_workspace_event(0, event, window, cx);
                },
            ));
            subscriptions.push(bridge.subscribe_huddle_media_in(
                window,
                cx,
                |this: &mut App, event, window, cx| {
                    this.on_huddle_media_event(0, event, window, cx);
                },
            ));
            subscriptions.extend(wire_feature_view_subscriptions(&feature_views, window, cx));
            // Subscribe to every background workspace's EventHub (and dedicated HuddleMediaHub)
            // so unread/mention badges, team metadata, and active huddle media stay live across
            // workspace switches. Each names its workspace by slot, which a reorder never moves.
            for (ws_ix, slot) in slots.iter_mut().enumerate() {
                if let Some(bg_bridge) = slot.bridge.as_mut() {
                    subscriptions.push(bg_bridge.subscribe_in(
                        window,
                        cx,
                        move |this: &mut App, event, window, cx| {
                            this.on_workspace_event(ws_ix, event, window, cx);
                        },
                    ));
                    subscriptions.push(bg_bridge.subscribe_huddle_media_in(
                        window,
                        cx,
                        move |this: &mut App, event, window, cx| {
                            this.on_huddle_media_event(ws_ix, event, window, cx);
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
                            app.check_daily_catchup_reminder(window, cx);
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
                bridge.send(ChannelCommand::OpenConversation(channel.clone()));
            }
            demo(&results, window, cx);
            // The fixture-derived feed lights the rail tile before any live fetch lands.
            let unread = feature_views.activity.read(cx).unread_count();
            let saved_section_prefs = bridge.section_prefs();
            sidebar.update(cx, |sidebar, cx| {
                sidebar.set_activity_unread(unread, cx);
                if let Some(prefs) = saved_section_prefs {
                    sidebar.restore_section_prefs(prefs, cx);
                }
            });

            let initial_outbox = bridge.outbox_sync();
            if !initial_outbox.is_empty() {
                thread.update(cx, |panel, cx| panel.set_outbox(initial_outbox.clone(), cx));
            }
            let (cached_member_counts, cached_app_counts) =
                CHANNEL_COUNTS_CACHE.read(&local).unwrap_or_default();
            let cached_bots = fixtures.bots.clone();
            let FeatureViews {
                search,
                search_menu,
                composer,
                preferences,
                status_picker,
                user_menu,
                emoji_picker,
                drafts,
                later,
                threads,
                unreads,
                catchup,
                activity,
                files_page,
                huddles,
                directory,
                tools_page,
                compose,
                details,
                canvas_search,
                list_search,
            } = feature_views;
            App {
                fixtures,
                bridge,
                slots,
                rail,
                workspace: 0,
                workspace_epoch: 0,
                ws: WorkspaceState {
                    chosen,
                    bots: cached_bots,
                    member_counts: cached_member_counts,
                    app_counts: cached_app_counts,
                    outbox: initial_outbox,
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
                browser,
                page: MainPage::Channel,
                right_pane: RightPane::None,
                tab: ConversationTab::Messages,
                huddle_session: None,
                huddle_owner: None,
                huddle_state: None,
                _huddle_state_sub: None,
                huddle_dock: None,
                huddle_popout: None,
                huddle_floating_panel: None,
                huddle_leaving_call: None,
                _huddle_dock_sub: None,
                huddle_invite: None,
                huddle_invite_owner: None,
                huddle_invite_ring_task: None,
                huddle_invite_expire_task: None,
                _huddle_invite_sub: None,
                huddle_alone_music_task: None,
                preferences,
                prefs_window: None,
                applied_theme,
                pane_focus: 2,
                color_mode,
                saved_settings,
                profile,
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
                _viewer_sub: None,
                drafts,
                later,
                threads,
                unreads,
                catchup,
                catchup_generation: 0,
                unreads_fetched: HashSet::new(),
                unreads_refresh: LiveRefresh::default(),
                activity,
                files_page,
                huddles,
                directory,
                tools_page,
                compose,
                details,
                pending_dm: None,
                last_right_pane: None,
                canvas_search,
                canvas_query: String::new(),
                list_search,
                list_query: String::new(),
                list_sort: None,
                app_home_kit,
                modal_stack,
                workflow_details: None,
                fixture_bookmarks: HashMap::new(),
                #[cfg(test)]
                os: Rc::new(platform::noop()),
                #[cfg(not(test))]
                os: Rc::new(platform::current()),
                badge: None,
                badge_label: None,
                last_status_bar: None,
                window_active: window.is_window_active(),
                recent_notified_tags: Vec::new(),
                window_handle: window.window_handle(),
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
        let dispatch_app = app.downgrade();
        let provider_app = app.downgrade();
        let has_tr_app = app.downgrade();
        crate::media::install_transcription_dispatcher(
            move |file_id, local_path, provider, cx| {
                let _ = dispatch_app.update(cx, |app, _| {
                    app.bridge.send(CollabCommand::TranscribeMediaFile {
                        file_id,
                        local_path,
                        provider,
                    });
                });
            },
            move |cx| {
                provider_app
                    .upgrade()
                    .map(|a| {
                        a.read(cx)
                            .preferences
                            .read(cx)
                            .settings()
                            .transcription_provider()
                    })
                    .unwrap_or(slackless_core::huddle::TranscriptionProvider::SlackBuiltIn)
            },
            move |file_id, cx| {
                has_tr_app
                    .upgrade()
                    .is_some_and(|a| a.read(cx).fixtures.file_transcript(file_id).is_some())
            },
            cx,
        );
        install_os_and_lifecycle_hooks(&app, window, cx);
        hydrate_initial_channel(&app, window, cx);
        app
    }
}
