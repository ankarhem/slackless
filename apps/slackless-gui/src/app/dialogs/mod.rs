//! Modal dialogs, pickers, preferences application, and App overlays.

mod channels;
mod profile;
mod storage;

use super::*;

impl App {
    /// The "+" menu's Link item and the bar's "Add a bookmark": the add-link dialog for the
    /// open conversation.
    pub(super) fn open_link_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        bookmarks::open_add_dialog(window, cx, move |title, url, cx| {
            let _ = this.update(cx, |app, cx| app.add_bookmark(title, url, cx));
        });
    }

    /// Apply what the app honors from Preferences: the theme (Light / Dark / Sync with OS,
    /// tinted by the color theme), Enter-to-send, and the schedule shown in the user menu.
    pub(super) fn apply_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.preferences.read(cx).settings().clone();
        // Slack plays a sound when it is picked in Preferences.
        if self
            .sound_choice
            .replace(settings.sound)
            .is_some_and(|before| before != settings.sound)
        {
            self.play_sound(settings.sound, cx);
        }
        let theme = settings.mode.resolve(window.appearance());
        if self
            .color_mode
            .is_some_and(|stored| stored != settings.mode)
        {
            self.color_mode = Some(settings.mode);
            if let Some(write) = self.bridge.remember_color_mode(settings.mode) {
                cx.background_executor().spawn(write).detach();
            }
        }
        let focus = self.self_state.focused();
        let theme_key = (
            theme,
            settings.palette,
            settings.darker_sidebars,
            settings.window_gradient,
            focus,
        );
        if self.applied_theme != theme_key {
            self.applied_theme = theme_key;
            theme::activate_with(
                theme,
                settings.palette,
                settings.darker_sidebars,
                settings.window_gradient,
                focus,
                cx,
            );
        }
        if crate::zoom::set_level(settings.zoom, cx)
            && let Some(write) = self.bridge.remember_zoom(settings.zoom)
        {
            cx.background_executor().spawn(write).detach();
        }
        if theme::font(cx) != settings.font {
            theme::set_font(settings.font, cx);
            if let Some(write) = self.bridge.remember_font(settings.font) {
                cx.background_executor().spawn(write).detach();
            }
        }
        let enter_sends = settings.enter_sends;
        let format_markup = settings.format_markup;
        self.composer.update(cx, |composer, cx| {
            composer.set_enter_sends(enter_sends, cx);
            composer.set_format_markup(format_markup, cx);
            composer.set_spellcheck(settings.spellcheck, settings.spellcheck_language(), cx);
        });
        self.messages
            .update(cx, |list, cx| list.set_enter_sends(enter_sends, cx));
        self.thread
            .update(cx, |thread, cx| thread.set_enter_sends(enter_sends, cx));
        self.threads
            .update(cx, |view, cx| view.set_enter_sends(enter_sends, cx));
        self.compose
            .update(cx, |view, cx| view.set_enter_sends(enter_sends, cx));
        let remote_auto = settings.remote_search_auto;
        let excluded_channels = settings.excluded_channels.clone();
        let show_deleted_messages = settings.show_deleted_messages;
        self.results.update(cx, |view, _| {
            view.set_remote_auto(remote_auto);
            view.set_excluded_channels(&excluded_channels);
            view.set_include_deleted(show_deleted_messages);
        });
        crate::zoom::set_strip_tracking_params(settings.strip_tracking_params);
        if self
            .saved_settings
            .as_ref()
            .is_none_or(|prev| prev.stealth_mode != settings.stealth_mode)
        {
            self.bridge
                .send(Command::SetStealthMode(settings.stealth_mode));
        }
        if self
            .saved_settings
            .as_ref()
            .is_none_or(|prev| prev.huddle_save_transcripts != settings.huddle_save_transcripts)
        {
            self.bridge.send(Command::SetSaveHuddleTranscripts(
                settings.huddle_save_transcripts,
            ));
        }
        crate::preferences::set_display_prefs(&settings, cx);
        window.set_background_appearance(if settings.window_transparency {
            gpui::WindowBackgroundAppearance::Blurred
        } else {
            gpui::WindowBackgroundAppearance::Opaque
        });
        slackless_core::api::set_net_log_enabled(
            settings.net_log_enabled || settings.debug_logging,
        );
        crate::messages::set_timezone_override(settings.timezone_offset_secs());
        crate::media::set_download_dir(
            (!settings.download_dir.is_empty())
                .then(|| std::path::PathBuf::from(&settings.download_dir)),
            cx,
        );
        crate::media::set_ask_download_location(settings.ask_download_location, cx);
        let nav_prefs = crate::sidebar::NavPrefs {
            show_home: settings.show_home,
            show_dms: settings.show_dms,
            show_activity: settings.show_activity,
            show_files: settings.show_files,
            show_later: settings.show_later,
            icons_only: settings.tab_style == 1,
            order: settings.nav_order,
        };
        let home_prefs = crate::sidebar::HomePrefs {
            show: settings.show,
            sort: settings.sort,
            unreads_section: settings.unreads_section,
            separate_private: settings.separate_private,
            profile_photos: settings.profile_photos,
            compact: settings.sidebar_compact,
            show_muted: settings.show_muted_channels,
        };
        let vip_users = settings.vip_users.clone();
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_nav_prefs(nav_prefs, cx);
            sidebar.set_home_prefs(home_prefs, cx);
            sidebar.set_vip_users(vip_users.clone(), cx);
        });
        self.activity.update(cx, |activity, cx| {
            activity.set_vip_users(vip_users, cx);
        });
        if let Some(prev) = self.saved_settings.as_ref()
            && *prev != settings
        {
            // Cloud pref write-back (`users.prefs.set`) for display, emoji, and notification prefs.
            if !self.fixtures.sample {
                let sync_pref = |name: &str, value: String| {
                    self.bridge.send(Command::SetUserPref {
                        name: name.to_owned(),
                        value,
                    });
                };
                if prev.clock_24h != settings.clock_24h {
                    sync_pref("time24", settings.clock_24h.to_string());
                }
                if prev.names != settings.names {
                    sync_pref("display_real_names", (settings.names == 0).to_string());
                }
                if prev.jumbomoji != settings.jumbomoji {
                    sync_pref("jumbomoji", settings.jumbomoji.to_string());
                }
                if prev.convert_emoticons != settings.convert_emoticons {
                    sync_pref("convert_emoticons", settings.convert_emoticons.to_string());
                }
                if prev.emoji_text_mode != settings.emoji_text_mode {
                    sync_pref("emoji_only_text", settings.emoji_text_mode.to_string());
                }
                if prev.show_quick_reactions != settings.show_quick_reactions {
                    sync_pref(
                        "show_quick_reactions",
                        settings.show_quick_reactions.to_string(),
                    );
                }
                if prev.quick_reaction_emojis != settings.quick_reaction_emojis {
                    let resolved = settings.resolved_quick_reactions();
                    let defaults: Vec<&str> = crate::actions::DEFAULT_QUICK_REACTIONS.to_vec();
                    let is_default = resolved.len() == defaults.len()
                        && resolved.iter().zip(defaults.iter()).all(|(a, b)| a == b);
                    sync_pref(
                        "user_customized_quick_reactions_has_customized",
                        (!is_default).to_string(),
                    );
                    for (ix, key) in [
                        "user_customized_quick_reactions_emoji_1",
                        "user_customized_quick_reactions_emoji_2",
                        "user_customized_quick_reactions_emoji_3",
                    ]
                    .iter()
                    .enumerate()
                    {
                        let val = settings.quick_reaction_emojis[ix]
                            .trim()
                            .trim_matches(':')
                            .to_owned();
                        if !val.is_empty() {
                            sync_pref(key, val);
                        }
                    }
                }
                if prev.underline_links != settings.underline_links {
                    sync_pref("underline_links", settings.underline_links.to_string());
                }
                if prev.density != settings.density {
                    sync_pref(
                        "messages_theme",
                        if settings.density == 1 {
                            "compact"
                        } else {
                            "default"
                        }
                        .to_owned(),
                    );
                }
                if prev.skin_tone != settings.skin_tone {
                    sync_pref(
                        "preferred_skin_tone",
                        format!("skin-tone-{}", settings.skin_tone + 1),
                    );
                }
                let sync_global_notif = |name: &str, value: String, sync: bool| {
                    self.bridge.send(Command::SetGlobalNotificationPref {
                        name: name.to_owned(),
                        value,
                        sync,
                    });
                };
                if prev.notify_level != settings.notify_level
                    || prev.mobile_different != settings.mobile_different
                {
                    sync_global_notif(
                        "global_desktop",
                        settings.notify_level_wire().to_owned(),
                        !settings.mobile_different,
                    );
                }
                if settings.mobile_different
                    && (prev.mobile_notify_level != settings.mobile_notify_level
                        || !prev.mobile_different)
                {
                    sync_global_notif(
                        "global_mobile",
                        settings.mobile_notify_level_wire().to_owned(),
                        false,
                    );
                }
                if prev.thread_replies != settings.thread_replies {
                    sync_global_notif(
                        "threads_everything",
                        settings.thread_replies.to_string(),
                        false,
                    );
                }
                if prev.notify_channel_mentions != settings.notify_channel_mentions {
                    sync_pref(
                        "suppress_at_channel_global",
                        (!settings.notify_channel_mentions).to_string(),
                    );
                }
                if prev.notify_huddle_invites != settings.notify_huddle_invites {
                    sync_pref(
                        "huddle_invite_notifications",
                        settings.notify_huddle_invites.to_string(),
                    );
                }
                if prev.notify_reminders != settings.notify_reminders {
                    sync_pref(
                        "reminder_notifications",
                        settings.notify_reminders.to_string(),
                    );
                }
                if prev.show_preview != settings.show_preview {
                    sync_global_notif(
                        "no_text_in_notifications",
                        (!settings.show_preview).to_string(),
                        false,
                    );
                }
                if prev.mobile_handoff != settings.mobile_handoff {
                    sync_global_notif(
                        "push_idle_wait",
                        settings.mobile_handoff_secs().to_string(),
                        false,
                    );
                }
                if prev.keywords != settings.keywords {
                    sync_pref("highlight_words", settings.keywords.clone());
                    sync_global_notif("global_keywords", settings.keywords.clone(), false);
                }
                if prev.mute_sounds != settings.mute_sounds {
                    sync_pref("mute_sounds", settings.mute_sounds.to_string());
                }
                if prev.badge_on_unreads != settings.badge_on_unreads {
                    sync_pref("mac_ssb_bullet", settings.badge_on_unreads.to_string());
                }
                if prev.bounce_dock != settings.bounce_dock {
                    sync_pref("mac_ssb_bounce", settings.bounce_dock.to_string());
                }
                if prev.schedule_enabled != settings.schedule_enabled {
                    sync_pref("dnd_enabled", settings.schedule_enabled.to_string());
                }
                if prev.schedule_days != settings.schedule_days {
                    sync_pref("dnd_days", settings.schedule_days_wire().to_owned());
                }
                if prev.schedule_custom_days != settings.schedule_custom_days
                    && settings.schedule_days == 2
                {
                    const DAY_KEYS: [&str; 7] = [
                        "dnd_enabled_sunday",
                        "dnd_enabled_monday",
                        "dnd_enabled_tuesday",
                        "dnd_enabled_wednesday",
                        "dnd_enabled_thursday",
                        "dnd_enabled_friday",
                        "dnd_enabled_saturday",
                    ];
                    for (ix, key) in DAY_KEYS.iter().enumerate() {
                        if prev.schedule_custom_days[ix] != settings.schedule_custom_days[ix] {
                            sync_pref(key, settings.schedule_custom_days[ix].to_string());
                        }
                    }
                }
                if prev.schedule_start != settings.schedule_start {
                    sync_pref(
                        "dnd_end_hour",
                        format!("{:02}:00", settings.schedule_start % 24),
                    );
                }
                if prev.schedule_end != settings.schedule_end {
                    sync_pref(
                        "dnd_start_hour",
                        format!("{:02}:00", settings.schedule_end % 24),
                    );
                }
                if prev.palette != settings.palette
                    && let Some(theme_name) = settings.palette.slack_pref_name()
                {
                    sync_pref("sidebar_theme", theme_name.to_owned());
                }
            }
            if prev.launch_at_login != settings.launch_at_login {
                sync_launch_at_login(settings.launch_at_login);
            }
            if prev.proxy_url != settings.proxy_url {
                slackless_core::api::set_proxy_url(Some(settings.proxy_url.clone()));
            }
            if prev.net_log_enabled != settings.net_log_enabled {
                slackless_core::api::set_net_log_enabled(settings.net_log_enabled);
            }
            let reload_deleted = prev.show_deleted_messages != settings.show_deleted_messages;
            if self.huddle_session.is_some() {
                if prev.huddle_input_device != settings.huddle_input_device
                    || prev.huddle_output_device != settings.huddle_output_device
                {
                    self.bridge.send(Command::SetHuddleAudioDevices {
                        input: settings.huddle_input_device.clone(),
                        output: settings.huddle_output_device.clone(),
                    });
                }
                if prev.huddle_aec != settings.huddle_aec
                    || prev.huddle_ns != settings.huddle_ns
                    || prev.huddle_agc != settings.huddle_agc
                {
                    self.bridge.send(Command::SetHuddleAudioProcessing {
                        aec: settings.huddle_aec,
                        ns: settings.huddle_ns,
                        agc: settings.huddle_agc,
                    });
                }
                if prev.huddle_blur_background != settings.huddle_blur_background {
                    self.bridge.send(Command::SetHuddleBlurBackground(
                        settings.huddle_blur_background,
                    ));
                }
            }
            self.saved_settings = Some(settings.clone());
            if let Some(write) = self.bridge.remember_settings(&settings) {
                cx.background_executor().spawn(write).detach();
            }
            if reload_deleted {
                self.messages
                    .update(cx, |list, cx| list.reset_deleted_overrides(cx));
                self.thread
                    .update(cx, |panel, cx| panel.reset_deleted_overrides(cx));
            }
        }
        window.refresh();
        self.push_self_state(cx);
        self.sync_badge(cx);
    }

    pub(super) fn user_menu_event(
        &mut self,
        event: &UserMenuEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            UserMenuEvent::EditStatus => {
                self.user_menu_open = false;
                self.open_status_picker(window, cx);
            }
            UserMenuEvent::ClearStatus => {
                self.self_state.set_status(None);
                self.send_status_to_slack(None);
            }
            UserMenuEvent::SetAway(away) => {
                self.self_state.away = *away;
                if !self.fixtures.sample {
                    self.bridge.send(Command::SetPresence { away: *away });
                }
            }
            UserMenuEvent::Pause(until) => {
                self.self_state.paused_until = *until;
                self.user_menu_open = false;
                if !self.fixtures.sample {
                    match *until {
                        Some(ts) => {
                            let secs = ts.saturating_sub(now_secs() as i64).max(0) as u64;
                            let minutes = secs.div_ceil(60).max(1) as u32;
                            self.bridge.send(Command::SetDndSnooze { minutes });
                        }
                        None => self.bridge.send(Command::EndDndSnooze),
                    }
                }
            }
            UserMenuEvent::OpenProfile => {
                self.user_menu_open = false;
                self.open_profile(self.fixtures.self_user.clone(), cx);
            }
            UserMenuEvent::OpenPreferences => {
                self.user_menu_open = false;
                self.open_preferences(None, window, cx);
            }
            UserMenuEvent::OpenSchedule => {
                self.user_menu_open = false;
                self.open_preferences(Some(Section::Notifications), window, cx);
            }
            UserMenuEvent::OpenPauseCustom => {
                self.user_menu_open = false;
                self.user_menu
                    .update(cx, |menu, cx| menu.open_pause_dialog(window, cx));
            }
            UserMenuEvent::OpenAccount => {
                self.user_menu_open = false;
                // Slack opens the workspace's account settings in the browser.
                let domain = self
                    .fixtures
                    .workspaces
                    .get(self.workspace)
                    .map(|team| team.domain.as_str())
                    .unwrap_or_default();
                let url = if domain.is_empty() {
                    "https://slack.com/account/settings".to_owned()
                } else {
                    format!("https://{domain}.slack.com/account/settings")
                };
                cx.open_url(&url);
            }
            UserMenuEvent::OpenDownloads => {
                self.user_menu_open = false;
                self.open_downloads(window, cx);
            }
            UserMenuEvent::SignOut => {
                self.user_menu_open = false;
                self.sign_out(self.workspace, cx);
            }
        }
        if !self.user_menu_open {
            // Forget hover state so a closed menu cannot come back with its submenu open.
            self.user_menu.update(cx, |menu, cx| menu.closed(cx));
        }
        self.push_self_state(cx);
    }

    /// Sign out of the workspace at `index`: revokes the token server-side (`auth.revoke`,
    /// `F-CORE-009`), removes the stored session (Keychain included), and quits so the next
    /// launch runs the sign-in helper.
    pub(super) fn sign_out(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == self.workspace {
            self.bridge.send(Command::SignOut);
        } else if let Some(Some(bridge)) = self.parked_bridges.get(index) {
            bridge.send(Command::SignOut);
        }
        if let Some(team) = self.fixtures.workspaces.get(index)
            && let Err(e) =
                slackless_auth::SessionStore::new().and_then(|store| store.remove(&team.id))
        {
            eprintln!("slackless: could not remove the stored session ({e})");
        }
        cx.quit();
    }

    /// Open Preferences in its own standalone native OS window (`F-CORE-011`), focusing the
    /// existing window if already open. All slow diagnostics (SQLite counts, media cache walk,
    /// CoreAudio enumeration) run asynchronously off the UI thread so the window appears instantly.
    pub(super) fn open_preferences(
        &mut self,
        section: Option<Section>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(section) = section {
            self.preferences
                .update(cx, |prefs, cx| prefs.show(section, cx));
        } else {
            self.preferences.update(cx, |prefs, cx| prefs.on_reopen(cx));
        }
        if let Some(handle) = self.prefs_window {
            if handle
                .update(cx, |_, win, _| {
                    win.activate_window();
                })
                .is_ok()
            {
                self.refresh_storage_stats(cx);
                return;
            }
            self.prefs_window = None;
        }
        let display_id = window.display(cx).map(|d| d.id());
        self.prefs_window =
            crate::preferences::open_preferences_window(self.preferences.clone(), display_id, cx);
        self.refresh_storage_stats(cx);
    }

    pub(super) fn open_status_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_hover_card(cx);
        let status = self.self_state.status.clone();
        let workspace = SharedString::from(self.fixtures.workspaces[self.workspace].name.clone());
        let presets = self.fixtures.status_presets.clone();
        let scheduled = self.self_state.scheduled_statuses.clone();
        self.status_picker.update(cx, |picker, cx| {
            picker.set_presets(presets, cx);
            picker.set_scheduled(scheduled, cx);
            picker.edit(status.as_ref(), workspace, window, cx)
        });
        let picker = self.status_picker.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            // Slack centers its modal and titles it large and bold.
            dialog
                .title(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::BOLD)
                        .child("Set a status"),
                )
                .centered()
                .w(px(520.))
                .child(picker.clone())
        });
        // Slack opens the dialog with the caret in the status field.
        let picker = self.status_picker.clone();
        window.defer(cx, move |window, cx| {
            picker.update(cx, |picker, cx| picker.focus(window, cx))
        });
    }

    /// The Cmd+K switcher in a dialog; the filter is focused once the dialog is up.
    pub(super) fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut recent = Vec::with_capacity(self.recent.len() + 1);
        if !self.channel.as_str().is_empty() {
            recent.push(self.channel.clone());
        }
        recent.extend(self.recent.iter().cloned());
        let visits = self.ws.visits.clone();
        self.switcher
            .update(cx, |switcher, cx| switcher.set_frecency(recent, visits, cx));
        let switcher = self.switcher.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.w(px(560.)).child(switcher.clone())
        });
        let switcher = self.switcher.clone();
        window.defer(cx, move |window, cx| {
            switcher.update(cx, |switcher, cx| switcher.open(window, cx));
        });
    }

    /// Show the emoji picker anchored at `position`, routing the pick to `target`.
    /// Opens the image viewer over the conversation's images, the clicked one showing.
    pub(super) fn open_image_viewer(
        &mut self,
        channel: ChannelId,
        images: Arc<Vec<ViewerImage>>,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if images.is_empty() {
            return;
        }
        let label = self
            .fixtures
            .conversation(&channel)
            .map(|conversation| self.fixtures.conversation_label(conversation))
            .unwrap_or_default();
        let viewer = ImageViewer::new(self.fixtures.clone(), label, images, index, cx);
        let subscription = cx.subscribe_in(
            &viewer,
            window,
            |this: &mut App, _, event: &ViewerEvent, window, cx| match event {
                ViewerEvent::Close => this.close_image_viewer(window, cx),
            },
        );
        // Replacing or closing the viewer drops its subscription immediately (no Vec leak).
        self._viewer_sub = Some(subscription);
        self.image_viewer = Some(viewer);
        cx.notify();
    }

    /// Closes the image viewer, evicting decoded full-res textures from GPUI's cache.
    pub(super) fn close_image_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(viewer) = self.image_viewer.take() {
            viewer.update(cx, |v, cx| v.evict_assets(window, cx));
            self._viewer_sub = None;
            cx.notify();
        }
    }

    pub(super) fn open_picker(
        &mut self,
        target: PickerTarget,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_picker_tab(target, position, PickerTab::Emoji, window, cx);
    }

    pub(super) fn open_picker_tab(
        &mut self,
        target: PickerTarget,
        position: Point<Pixels>,
        tab: PickerTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let show_gifs = !matches!(
            target,
            PickerTarget::Message { .. } | PickerTarget::ThreadMessage(_)
        );
        self.picker = Some((target, position));
        self.emoji_picker
            .update(cx, |picker, cx| picker.open_tab(tab, show_gifs, window, cx));
        cx.notify();
    }

    /// Slack's M key: the reminder presets for the focused message, dropped at the mouse
    /// like the row's context menu. A pick routes through the transcript's handler and the
    /// menu dismisses itself (its [`DismissEvent`] closes the overlay).
    pub(super) fn open_remind_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.remind_menu.is_some()
            || self.picker.is_some()
            || !window
                .focused(cx)
                .is_none_or(|focused| focused == self.focus)
            || self.messages.read(cx).focused(cx).is_none()
        {
            return;
        }
        let position = window.mouse_position();
        let menu = PopupMenu::build(window, cx, |menu, _, _| {
            let mut menu = menu;
            for preset in Remind::ALL {
                let messages = self.messages.downgrade();
                menu = menu.item(
                    PopupMenuItem::new(preset.label().to_owned())
                        .icon(IconName::Clock)
                        .on_click(move |_, window, cx| {
                            let _ = messages.update(cx, |list, cx| {
                                list.focus_action(MessageAction::RemindMe(preset), window, cx)
                            });
                        }),
                );
            }
            let messages = self.messages.downgrade();
            menu.separator().item(
                PopupMenuItem::new("Custom\u{2026}")
                    .icon(IconName::Calendar)
                    .on_click(move |_, window, cx| {
                        let _ = messages.update(cx, |list, cx| {
                            list.focus_action(MessageAction::RemindCustom, window, cx)
                        });
                    }),
            )
        });
        menu.focus_handle(cx).focus(window, cx);
        let subscription = cx.subscribe_in(
            &menu,
            window,
            |this: &mut Self, _, _: &DismissEvent, window, cx| {
                this.close_remind_menu(window, cx);
            },
        );
        self.remind_menu = Some((menu, position, subscription));
        cx.notify();
    }

    pub(super) fn close_remind_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.remind_menu.take().is_some() {
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    pub(super) fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = None;
        self.emoji_picker
            .update(cx, |picker, cx| picker.release_gifs(window, cx));
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Render an app-opened Block Kit modal (`views.open`) and submit via `views.submit` (`F-COLLAB-725`).
    pub(super) fn open_block_modal(
        &mut self,
        view: crate::blockkit::Json,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view_id = view["id"].as_str().unwrap_or_default().to_owned();
        if view_id.is_empty() {
            return;
        }
        let hash = view["hash"].as_str().map(str::to_owned);
        let title = view["title"]["text"]
            .as_str()
            .unwrap_or("App modal")
            .to_owned();
        let submit_label = view["submit"]["text"].as_str().map(str::to_owned);
        let close_label = view["close"]["text"]
            .as_str()
            .unwrap_or("Cancel")
            .to_owned();
        let notify_on_close = view["notify_on_close"].as_bool().unwrap_or(false);
        let raw_blocks: Vec<crate::blockkit::Json> =
            view["blocks"].as_array().cloned().unwrap_or_default();
        let ts = Ts::from(format!("modal-{view_id}"));
        let kit = cx.new(|_| crate::blockkit::BlockKitState::default());
        crate::blockkit::prepare_raw_inputs(&kit, &ts, &raw_blocks, window, cx);
        let fixtures = self.fixtures.clone();
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let kcx = crate::blockkit::KitCx::new(&fixtures, Some(&kit), &ts);
            let body = crate::blockkit::render_raw_blocks(&raw_blocks, &kcx, cx);
            let this_close = this.clone();
            let (close_id, close_hash) = (view_id.clone(), hash.clone());
            let mut footer = h_flex().justify_end().gap_2().pt_3().child(
                preferences::form_button(
                    Button::new("modal-close")
                        .outline()
                        .label(close_label.clone()),
                    cx,
                )
                .on_click(move |_, window, cx| {
                    if notify_on_close && let Some(app) = this_close.upgrade() {
                        app.update(cx, |app, _| {
                            app.bridge.send(Command::CloseModal {
                                view_id: close_id.clone(),
                                hash: close_hash.clone(),
                            });
                        });
                    }
                    window.close_dialog(cx);
                }),
            );
            if let Some(submit_text) = &submit_label {
                let (this_submit, kit_submit, ts_submit, blocks_submit, sub_id, sub_hash) = (
                    this.clone(),
                    kit.clone(),
                    ts.clone(),
                    raw_blocks.clone(),
                    view_id.clone(),
                    hash.clone(),
                );
                footer = footer.child(
                    preferences::form_button(
                        Button::new("modal-submit")
                            .primary()
                            .label(submit_text.clone()),
                        cx,
                    )
                    .on_click(move |_, window, cx| {
                        match crate::blockkit::collect_modal_state(
                            &ts_submit,
                            &blocks_submit,
                            &kit_submit,
                            cx,
                        ) {
                            Ok(state_values) => {
                                if let Some(app) = this_submit.upgrade() {
                                    app.update(cx, |app, _| {
                                        app.bridge.send(Command::SubmitModal {
                                            view_id: sub_id.clone(),
                                            hash: sub_hash.clone(),
                                            state_values,
                                        });
                                    });
                                }
                                window.close_dialog(cx);
                            }
                            Err(err) => {
                                notify("Missing or invalid field", err, window, cx);
                            }
                        }
                    }),
                );
            }
            dialog.title(title.clone()).w(px(540.)).child(
                v_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .id("modal-blocks-body")
                            .max_h(px(460.))
                            .overflow_y_scroll()
                            .child(body),
                    )
                    .child(footer),
            )
        });
    }

    /// Shift+Esc: ask first while Preferences > Mark as read prompts to confirm (on by
    /// default), then mark everything read.
    pub(super) fn request_mark_all_read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.preferences.read(cx).settings().confirm_mark_all {
            return self.mark_all_read(cx);
        }
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            let this = this.clone();
            dialog
                .title("Mark all messages as read?")
                .w(px(440.))
                .child(
                    v_flex()
                        .gap_4()
                        .child(div().text_color(muted).child(
                            "Every channel, direct message and thread will be marked as read.",
                        ))
                        .child(
                            h_flex()
                                .gap_2()
                                .justify_end()
                                .child(
                                    preferences::form_button(
                                        Button::new("mark-all-cancel").outline().label("Cancel"),
                                        cx,
                                    )
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    preferences::form_button(
                                        Button::new("mark-all-confirm")
                                            .primary()
                                            .label("Mark all as read"),
                                        cx,
                                    )
                                    .on_click(
                                        move |_, window, cx| {
                                            window.close_dialog(cx);
                                            let _ =
                                                this.update(cx, |app, cx| app.mark_all_read(cx));
                                        },
                                    ),
                                ),
                        ),
                )
        });
    }

    pub(super) fn open_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.open_dialog(cx, |dialog, _, cx| {
            dialog
                .title("Keyboard shortcuts")
                .w(px(860.))
                .child(shortcuts_view(cx))
        });
    }

    /// Open a native dialog for picking a custom local date (`YYYY-MM-DD`) and time (`HH:MM`)
    /// to schedule `text` in `channel` (optionally in `thread_ts`).
    pub(super) fn open_custom_schedule(
        &mut self,
        channel: ChannelId,
        thread_ts: Option<Ts>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (default_date, default_time) = default_custom_datetime();

        let date_in = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .placeholder("YYYY-MM-DD")
                .auto_grow(1, 1);
            s.set_value(default_date, window, cx);
            s
        });
        let time_in = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .placeholder("HH:MM (24-hour)")
                .auto_grow(1, 1);
            s.set_value(default_time, window, cx);
            s
        });
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            let field = |label: &'static str, state: &Entity<TextareaState>| {
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .child(label),
                    )
                    .child(preferences::text_input_box(state, false, cx))
            };
            let this_save = this.clone();
            let (d_in, t_in) = (date_in.clone(), time_in.clone());
            let (ch, th, body) = (channel.clone(), thread_ts.clone(), text.clone());
            dialog.title("Schedule message").w(px(400.)).child(
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Choose a local date and 24-hour time to send your message."),
                    )
                    .child(field("Date (YYYY-MM-DD)", &date_in))
                    .child(field("Time (HH:MM)", &time_in))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .pt_2()
                            .child(
                                preferences::form_button(
                                    Button::new("sched-custom-cancel").outline().label("Cancel"),
                                    cx,
                                )
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                preferences::form_button(
                                    Button::new("sched-custom-ok")
                                        .primary()
                                        .label("Schedule message"),
                                    cx,
                                )
                                .on_click(move |_, window, cx| {
                                    let ds = d_in.read(cx).value().trim().to_owned();
                                    let ts = t_in.read(cx).value().trim().to_owned();
                                    let Some(post_at) = parse_local_schedule(&ds, &ts) else {
                                        notify(
                                            "Invalid date or time",
                                            "Use YYYY-MM-DD and HH:MM in the future.",
                                            window,
                                            cx,
                                        );
                                        return;
                                    };
                                    if let Some(app) = this_save.upgrade() {
                                        app.update(cx, |app, cx| {
                                            if th.is_some() {
                                                app.thread.update(cx, |panel, cx| {
                                                    panel.clear_composer(window, cx);
                                                });
                                            } else {
                                                app.composer.update(cx, |c, cx| {
                                                    c.load_draft("", cx);
                                                });
                                            }
                                            app.schedule_at(
                                                ch.clone(),
                                                th.clone(),
                                                body.clone(),
                                                post_at,
                                                format!("on {ds} at {ts}"),
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                    window.close_dialog(cx);
                                }),
                            ),
                    ),
            )
        });
    }

    /// The channel browser in a gpui-kit dialog (gpui-kit-guide section 7).
    pub(super) fn open_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let browser = self.browser.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Browse channels")
                .w(px(640.))
                .child(browser.clone())
        });
    }
}
