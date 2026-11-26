//! Modal dialogs, pickers, preferences application, and App overlays.

mod channels;
pub(super) mod modal;
pub(super) mod profile;
pub(super) mod storage;
mod usergroups;
pub(super) mod workflow;

use super::*;
use slackless_core::types::{BotId, TeamId};

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
        self.ensure_usergroup_router(cx);
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
        self.profile
            .update(cx, |p, cx| p.set_show_pronouns(settings.show_pronouns, cx));
        let remote_auto = settings.remote_search_auto;
        let excluded_channels = settings.excluded_channels.clone();
        let show_deleted_messages = settings.show_deleted_messages;
        self.results.update(cx, |view, _| {
            view.set_remote_auto(remote_auto);
            view.set_excluded_channels(&excluded_channels);
            view.set_include_deleted(show_deleted_messages);
        });
        crate::zoom::set_strip_tracking_params(settings.strip_tracking_params);
        crate::composer::set_outgoing_guard_prefs(
            settings.strip_tracking_params,
            settings.warn_before_sending_secrets,
            cx,
        );
        if self
            .saved_settings
            .as_ref()
            .is_none_or(|prev| prev.stealth_mode != settings.stealth_mode)
        {
            self.bridge
                .send(WorkspaceCommand::SetStealthMode(settings.stealth_mode));
        }
        if self
            .saved_settings
            .as_ref()
            .is_none_or(|prev| prev.huddle_save_transcripts != settings.huddle_save_transcripts)
        {
            self.bridge.send(WorkspaceCommand::SetSaveHuddleTranscripts(
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
                    self.bridge.send(WorkspaceCommand::SetUserPref {
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
                    self.bridge
                        .send(WorkspaceCommand::SetGlobalNotificationPref {
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
                    self.bridge.send(HuddleControlCommand::SetAudioDevices {
                        input: settings.huddle_input_device.clone(),
                        output: settings.huddle_output_device.clone(),
                    });
                }
                if prev.huddle_aec != settings.huddle_aec
                    || prev.huddle_ns != settings.huddle_ns
                    || prev.huddle_agc != settings.huddle_agc
                {
                    self.bridge.send(HuddleControlCommand::SetAudioProcessing {
                        aec: settings.huddle_aec,
                        ns: settings.huddle_ns,
                        agc: settings.huddle_agc,
                    });
                }
                if prev.huddle_blur_background != settings.huddle_blur_background {
                    self.bridge.send(HuddleControlCommand::SetBlurBackground(
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
        self.sync_window_title_in(window);
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
                    self.bridge
                        .send(WorkspaceCommand::SetPresence { away: *away });
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
                            self.bridge.send(WorkspaceCommand::SetDndSnooze { minutes });
                        }
                        None => self.bridge.send(WorkspaceCommand::EndDndSnooze),
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
            self.bridge.send(WorkspaceCommand::SignOut);
        } else if let Some(Some(bridge)) = self.parked_bridges.get(index) {
            bridge.send(WorkspaceCommand::SignOut);
        }
        if let Some(team) = self.fixtures.workspaces.get(index)
            && let Err(e) =
                slackless_auth::SessionStore::new().and_then(|store| store.remove(&team.id))
        {
            tracing::warn!(error = %e, "could not remove the stored session");
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
        images: Rc<Vec<ViewerImage>>,
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
        let viewer = ImageViewer::new(self.fixtures.clone(), label, images, index, window, cx);
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
        let target_channel = match &target {
            PickerTarget::Message { channel, .. } => channel,
            _ => &self.channel,
        };
        let external_channel = self
            .fixtures
            .conversation(target_channel)
            .is_some_and(|c| c.is_ext_shared);
        self.picker = Some((target, position));
        self.emoji_picker.update(cx, |picker, cx| {
            picker.set_external_channel(external_channel, cx);
            picker.open_tab(tab, show_gifs, window, cx);
        });
        cx.notify();
    }

    /// Slack's M key: the reminder presets for the focused message, dropped at the mouse
    /// like the row's context menu. A pick routes through the transcript's handler and the
    /// menu dismisses itself (its [`DismissEvent`] closes the overlay).
    pub(super) fn open_remind_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.image_viewer.is_some()
            || self.remind_menu.is_some()
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

    /// Render an app-opened Block Kit modal (`view_opened` / `views.push`) in the multi-view
    /// modal stack (`F-COLLAB-600..616`).
    pub(super) fn open_block_modal(
        &mut self,
        view: crate::blockkit::Json,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.modal_stack
            .update(cx, |stack, cx| stack.open_view(view, window, cx));
    }

    /// Update an open Block Kit modal view in place (`view_updated` / `views.get`).
    pub(super) fn update_block_modal(
        &mut self,
        view: crate::blockkit::Json,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.modal_stack
            .update(cx, |stack, cx| stack.update_view(view, window, cx));
    }

    /// Close a Block Kit modal view (`view_closed`).
    pub(super) fn close_block_modal(
        &mut self,
        view_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.modal_stack.update(cx, |stack, cx| {
            stack.apply_modal_closed(view_id, window, cx)
        });
    }

    /// Fold a `views.submit` response into the active modal stack (`push`, `update`, `clear`,
    /// `close`, or inline per-`block_id` validation `errors`).
    pub(super) fn handle_modal_submit_result(
        &mut self,
        view_id: &str,
        outcome: Result<slackless_core::api::ViewSubmitResponse, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.modal_stack.update(cx, |stack, cx| {
            stack.apply_submit_result(view_id, outcome, window, cx)
        });
    }

    /// Dispatch `blocks.suggestions` for an `external_select` inside a modal (or return sample
    /// options in fixture mode).
    pub(super) fn request_modal_suggestions(
        &mut self,
        bot: BotId,
        app_id: String,
        team: Option<TeamId>,
        view_id: String,
        req: crate::blockkit::SuggestRequest,
        cx: &mut Context<Self>,
    ) {
        if self.fixtures.sample {
            let options = sample_external_suggestions(&req.value);
            self.modal_stack
                .update(cx, |stack, cx| stack.set_suggestions(&req.key, options, cx));
            return;
        }
        let team = team.unwrap_or_else(|| self.workspace_team_id());
        self.bridge.send(CollabCommand::BlockSuggestions {
            key: req.key,
            bot,
            app_id,
            team,
            action_id: req.action_id,
            block_id: req.block_id,
            value: req.value,
            container: slackless_core::api::ActionContainer {
                channel: self.channel.clone(),
                message_ts: Ts::default(),
                attachment_id: req.attachment,
                is_ephemeral: false,
                view_id: Some(view_id),
            },
        });
    }

    /// Active workspace team ID fallback for Block Kit actions/suggestions.
    pub(super) fn workspace_team_id(&self) -> TeamId {
        self.fixtures
            .workspaces
            .get(self.workspace)
            .or_else(|| self.fixtures.workspaces.first())
            .map(|t| t.id.clone())
            .unwrap_or_else(|| TeamId::from("T0"))
    }
}

/// Deterministic offline suggestions for `external_select` controls in sample/fixture mode.
pub(super) fn sample_external_suggestions(query: &str) -> Vec<(String, String)> {
    let catalog = [
        ("ENG-101 · Fix OAuth redirect", "ENG-101"),
        ("ENG-204 · Upgrade GPUI renderer", "ENG-204"),
        ("OPS-42 · Rotate staging certs", "OPS-42"),
        ("DES-19 · Modal validation polish", "DES-19"),
    ];
    let q = query.trim().to_ascii_lowercase();
    let mut out: Vec<(String, String)> = catalog
        .iter()
        .filter(|(label, val)| {
            q.is_empty()
                || label.to_ascii_lowercase().contains(&q)
                || val.to_ascii_lowercase().contains(&q)
        })
        .map(|(l, v)| ((*l).to_owned(), (*v).to_owned()))
        .collect();
    if out.is_empty() && !q.is_empty() {
        out.push((
            format!("Match for \u{201C}{}\u{201D}", query.trim()),
            query.trim().to_owned(),
        ));
    }
    out
}

impl App {
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
        let this = cx.entity().downgrade();
        crate::later::open_custom_datetime_dialog(
            "Schedule message",
            "Choose a local date and 24-hour time to send your message.",
            "Schedule message",
            window,
            cx,
            move |post_at, ds, ts, window, cx| {
                if let Some(app) = this.upgrade() {
                    app.update(cx, |app, cx| {
                        if thread_ts.is_some() {
                            app.thread.update(cx, |panel, cx| {
                                panel.clear_composer(window, cx);
                            });
                        } else {
                            app.composer.update(cx, |c, cx| {
                                c.load_draft("", cx);
                            });
                        }
                        app.schedule_at(
                            messaging::ScheduleSpec {
                                channel: channel.clone(),
                                thread_ts: thread_ts.clone(),
                                text: text.clone(),
                                post_at,
                                when_label: format!("on {ds} at {ts}"),
                            },
                            window,
                            cx,
                        );
                    });
                }
            },
        );
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
