//! Modal dialogs, popovers, hover cards, storage maintenance, and transcript export.

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
        if crate::zoom::set_level(settings.zoom, cx) {
            if let Some(write) = self.bridge.remember_zoom(settings.zoom) {
                cx.background_executor().spawn(write).detach();
            }
        }
        if theme::font(cx) != settings.font {
            theme::set_font(settings.font, cx);
            if let Some(write) = self.bridge.remember_font(settings.font) {
                cx.background_executor().spawn(write).detach();
            }
        }
        let enter_sends = settings.enter_sends;
        self.composer.update(cx, |composer, cx| {
            composer.set_enter_sends(enter_sends, cx);
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
        self.results.update(cx, |view, _| {
            view.set_remote_auto(remote_auto);
            view.set_excluded_channels(&excluded_channels);
        });
        crate::preferences::set_display_prefs(&settings, cx);
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
        };
        let home_prefs = crate::sidebar::HomePrefs {
            show: settings.show,
            sort: settings.sort,
            unreads_section: settings.unreads_section,
            separate_private: settings.separate_private,
            profile_photos: settings.profile_photos,
            compact: settings.sidebar_compact,
        };
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_nav_prefs(nav_prefs, cx);
            sidebar.set_home_prefs(home_prefs, cx);
        });
        if let Some(prev) = self.saved_settings.as_ref() {
            if *prev != settings {
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
                    if prev.thread_replies != settings.thread_replies {
                        sync_global_notif(
                            "threads_everything",
                            settings.thread_replies.to_string(),
                            false,
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
                    if prev.palette != settings.palette {
                        if let Some(theme_name) = settings.palette.slack_pref_name() {
                            sync_pref("sidebar_theme", theme_name.to_owned());
                        }
                    }
                }
                if prev.launch_at_login != settings.launch_at_login {
                    sync_launch_at_login(settings.launch_at_login);
                }
                self.saved_settings = Some(settings.clone());
                if let Some(write) = self.bridge.remember_settings(&settings) {
                    cx.background_executor().spawn(write).detach();
                }
            }
        }
        window.refresh();
        self.push_self_state(cx);
        self.sync_badge(cx);
    }

    /// Open Slack's "Create a channel" dialog (`conversations.create` + optional purpose).
    pub(super) fn open_create_channel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        let name_state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("e.g. plan-budget")
                .auto_grow(1, 1)
        });
        let purpose_state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("What is this channel about? (optional)")
                .auto_grow(1, 3)
        });
        let is_private = cx.new(|_| false);
        name_state.read(cx).focus_handle(cx).focus(window, cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (border, muted, bg, fg, accent) = (
                theme.border,
                theme.muted_foreground,
                theme.background,
                theme.foreground,
                theme.sidebar_accent,
            );
            let private_now = *is_private.read(cx);
            let raw_name = name_state.read(cx).value().to_string();
            let slug = channel_details::normalize_channel_slug(&raw_name);
            let preview = if slug.is_empty() {
                String::new()
            } else if private_now {
                format!("Will create private channel #{slug}")
            } else {
                format!("Will create public channel #{slug}")
            };
            let (name_s, purpose_s, priv_s, this_create) = (
                name_state.clone(),
                purpose_state.clone(),
                is_private.clone(),
                this.clone(),
            );
            let priv_pub = is_private.clone();
            let priv_prv = is_private.clone();
            dialog.title("Create a channel").w(px(460.)).child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(muted)
                                    .child("Name"),
                            )
                            .child(
                                div()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(border)
                                    .bg(bg)
                                    .child(
                                        Textarea::new(&name_state)
                                            .appearance(false)
                                            .bordered(false),
                                    ),
                            )
                            .when(!preview.is_empty(), |col| {
                                col.child(div().text_xs().text_color(muted).child(preview))
                            }),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(muted)
                                    .child("Visibility"),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        h_flex()
                                            .id("create-ch-public")
                                            .flex_1()
                                            .items_center()
                                            .gap_2()
                                            .px_3()
                                            .py_2()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(if !private_now { accent } else { border })
                                            .cursor_pointer()
                                            .text_sm()
                                            .text_color(fg)
                                            .child(Icon::new(IconName::Hash).size(px(14.)))
                                            .child("Public")
                                            .on_click(move |_, _, cx| {
                                                priv_pub.update(cx, |v, cx| {
                                                    *v = false;
                                                    cx.notify();
                                                });
                                            }),
                                    )
                                    .child(
                                        h_flex()
                                            .id("create-ch-private")
                                            .flex_1()
                                            .items_center()
                                            .gap_2()
                                            .px_3()
                                            .py_2()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(if private_now { accent } else { border })
                                            .cursor_pointer()
                                            .text_sm()
                                            .text_color(fg)
                                            .child(Icon::new(IconName::Lock).size(px(14.)))
                                            .child("Private")
                                            .on_click(move |_, _, cx| {
                                                priv_prv.update(cx, |v, cx| {
                                                    *v = true;
                                                    cx.notify();
                                                });
                                            }),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(muted)
                                    .child("Description"),
                            )
                            .child(
                                div()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(border)
                                    .bg(bg)
                                    .child(
                                        Textarea::new(&purpose_state)
                                            .appearance(false)
                                            .bordered(false),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("create-ch-cancel")
                                    .outline()
                                    .with_size(Size::Small)
                                    .label("Cancel")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("create-ch-submit")
                                    .primary()
                                    .with_size(Size::Small)
                                    .label("Create")
                                    .on_click(move |_, window, cx| {
                                        let slug = channel_details::normalize_channel_slug(
                                            name_s.read(cx).value().as_ref(),
                                        );
                                        if slug.is_empty() {
                                            actions::notify(
                                                "Enter a channel name",
                                                "Channel names use lowercase letters, numbers, and hyphens.",
                                                window,
                                                cx,
                                            );
                                            return;
                                        }
                                        let purpose =
                                            purpose_s.read(cx).value().trim().to_owned();
                                        let is_private = *priv_s.read(cx);
                                        window.close_dialog(cx);
                                        let _ = this_create.update(cx, |app, cx| {
                                            app.bridge.send(Command::CreateChannel {
                                                name: slug.clone(),
                                                is_private,
                                                purpose: (!purpose.is_empty()).then_some(purpose),
                                            });
                                            actions::notify(
                                                "Creating channel",
                                                format!("#{slug}"),
                                                window,
                                                cx,
                                            );
                                        });
                                    }),
                            ),
                    ),
            )
        });
    }

    /// Open a colleague multi-picker dialog to invite people into a channel (`conversations.invite`)
    /// or add people to a DM / start a group DM (`conversations.open`).
    pub(super) fn open_add_people(
        &mut self,
        channel: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.weak_entity();
        let fixtures = self.fixtures.clone();
        let conv = fixtures.conversation(&channel).cloned();
        let is_dm = conv.as_ref().is_some_and(|c| c.is_im || c.is_mpim);
        let existing: HashSet<UserId> = if is_dm {
            let mut set = HashSet::new();
            set.insert(fixtures.self_user.clone());
            if let Some(c) = &conv {
                if let Some(u) = &c.user {
                    set.insert(u.clone());
                }
                set.extend(c.members.iter().cloned());
            }
            if let Some((open_ch, members)) = &self.open_members {
                if *open_ch == channel {
                    set.extend(members.iter().cloned());
                }
            }
            set
        } else {
            self.open_members
                .as_ref()
                .filter(|(c, _)| *c == channel)
                .map(|(_, m)| m.iter().cloned().collect())
                .unwrap_or_default()
        };
        let candidates: Vec<(UserId, String, String)> = fixtures
            .users
            .iter()
            .filter(|u| !u.deleted && !u.is_bot && u.id.as_str() != "USLACKBOT")
            .filter(|u| !existing.contains(&u.id))
            .map(|u| {
                let name = fixtures.name_of(u).to_owned();
                let key = format!("{} {}", name.to_lowercase(), u.name.to_lowercase());
                (u.id.clone(), name, key)
            })
            .collect();
        let filter_state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Search people by name…")
                .auto_grow(1, 1)
        });
        let selected = cx.new(|_| Vec::<UserId>::new());
        filter_state.read(cx).focus_handle(cx).focus(window, cx);
        let title = if is_dm {
            "Add people to this DM".to_owned()
        } else {
            format!("Add people to {}", self.label(&channel))
        };
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (border, muted, bg, fg, hover, accent) = (
                theme.border,
                theme.muted_foreground,
                theme.background,
                theme.foreground,
                theme.accent,
                theme.sidebar_accent,
            );
            let needle = filter_state.read(cx).value().trim().to_lowercase();
            let picked = selected.read(cx).clone();
            let rows: Vec<_> = candidates
                .iter()
                .filter(|(_, _, key)| needle.is_empty() || key.contains(&needle))
                .take(25)
                .enumerate()
                .map(|(ix, (uid, name, _))| {
                    let on = picked.contains(uid);
                    let uid_click = uid.clone();
                    let sel_click = selected.clone();
                    h_flex()
                        .id(("add-people-row", ix))
                        .items_center()
                        .justify_between()
                        .px_3()
                        .py_1p5()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(on, |r| r.bg(accent.opacity(0.18)))
                        .hover(|r| r.bg(hover))
                        .child(div().text_sm().text_color(fg).child(name.clone()))
                        .when(on, |r| {
                            r.child(Icon::new(IconName::Check).size(px(14.)).text_color(accent))
                        })
                        .on_click(move |_, _, cx| {
                            sel_click.update(cx, |list, cx| {
                                if let Some(pos) = list.iter().position(|id| id == &uid_click) {
                                    list.remove(pos);
                                } else {
                                    list.push(uid_click.clone());
                                }
                                cx.notify();
                            });
                        })
                })
                .collect();
            let count = picked.len();
            let (sel_submit, ch_submit, this_submit, existing_submit) = (
                selected.clone(),
                channel.clone(),
                this.clone(),
                existing.clone(),
            );
            dialog.title(title.clone()).w(px(440.)).child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .child(
                        div()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .bg(bg)
                            .child(
                                Textarea::new(&filter_state)
                                    .appearance(false)
                                    .bordered(false),
                            ),
                    )
                    .child(
                        v_flex()
                            .id("add-people-list")
                            .max_h(px(280.))
                            .overflow_y_scroll()
                            .gap_0p5()
                            .when(rows.is_empty(), |col| {
                                col.child(
                                    div()
                                        .py_4()
                                        .text_center()
                                        .text_xs()
                                        .text_color(muted)
                                        .child("No matching people to add."),
                                )
                            })
                            .children(rows),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(div().text_xs().text_color(muted).child(if count == 0 {
                                "Select one or more people".to_owned()
                            } else {
                                format!("{count} selected")
                            }))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("add-people-cancel")
                                            .outline()
                                            .with_size(Size::Small)
                                            .label("Cancel")
                                            .on_click(|_, window, cx| window.close_dialog(cx)),
                                    )
                                    .child(
                                        Button::new("add-people-confirm")
                                            .primary()
                                            .with_size(Size::Small)
                                            .label(if is_dm { "Open group DM" } else { "Add" })
                                            .on_click(move |_, window, cx| {
                                                let chosen = sel_submit.read(cx).clone();
                                                if chosen.is_empty() {
                                                    return;
                                                }
                                                window.close_dialog(cx);
                                                let _ = this_submit.update(cx, |app, cx| {
                                                    if is_dm {
                                                        let mut users: Vec<UserId> =
                                                            existing_submit
                                                                .iter()
                                                                .filter(|id| {
                                                                    **id != app.fixtures.self_user
                                                                })
                                                                .cloned()
                                                                .collect();
                                                        for u in chosen {
                                                            if !users.contains(&u) {
                                                                users.push(u);
                                                            }
                                                        }
                                                        app.bridge.send(Command::OpenDm { users });
                                                    } else {
                                                        let n = chosen.len();
                                                        app.bridge.send(Command::InviteToChannel {
                                                            channel: ch_submit.clone(),
                                                            users: chosen,
                                                        });
                                                        actions::notify(
                                                            "Added people",
                                                            format!(
                                                                "Invited {n} {} to channel",
                                                                if n == 1 {
                                                                    "person"
                                                                } else {
                                                                    "people"
                                                                }
                                                            ),
                                                            window,
                                                            cx,
                                                        );
                                                    }
                                                });
                                            }),
                                    ),
                            ),
                    ),
            )
        });
    }

    /// Sync the signed-in user's custom status (or clear it with `None`) to Slack via
    /// `users.profile.set`.
    pub(super) fn send_status_to_slack(&self, status: Option<&crate::status::UserStatus>) {
        if self.fixtures.sample {
            return;
        }
        let update = match status {
            Some(s) => slackless_core::api::ProfileUpdate {
                status_text: Some(s.text.clone()),
                status_emoji: Some(format!(":{}:", s.emoji.trim_matches(':'))),
                status_expiration: Some(s.clears_at().unwrap_or(0)),
                ..Default::default()
            },
            None => slackless_core::api::ProfileUpdate {
                status_text: Some(String::new()),
                status_emoji: Some(String::new()),
                status_expiration: Some(0),
                ..Default::default()
            },
        };
        self.bridge.send(Command::UpdateProfile { update });
    }

    /// Open Slack's "Edit your profile" dialog for the signed-in user (display name, title,
    /// phone, pronouns), persisting via `users.profile.set`.
    pub(super) fn open_profile_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let me = self.fixtures.user(&self.fixtures.self_user).cloned();
        let initial_display = me
            .as_ref()
            .and_then(|u| u.profile.display_name.clone())
            .unwrap_or_default();
        let initial_title = me
            .as_ref()
            .and_then(|u| u.profile.title.clone())
            .unwrap_or_default();
        let initial_phone = me
            .as_ref()
            .and_then(|u| u.profile.phone.clone())
            .unwrap_or_default();
        let (loaded_pronouns, loaded_fields) = self
            .profile
            .read(cx)
            .loaded_profile(&self.fixtures.self_user);
        let initial_pronouns = loaded_pronouns
            .or_else(|| me.as_ref().and_then(|u| u.profile.pronouns.clone()))
            .or_else(|| {
                self.fixtures
                    .profile_extras
                    .get(&self.fixtures.self_user)
                    .and_then(|e| e.pronouns.clone())
            })
            .unwrap_or_default();
        let initial_tz = me.as_ref().and_then(|u| u.tz.clone()).unwrap_or_default();

        let display_in = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .placeholder("Display name")
                .auto_grow(1, 1);
            s.set_value(initial_display, window, cx);
            s
        });
        let title_in = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .placeholder("What I do (e.g. Developer)")
                .auto_grow(1, 1);
            s.set_value(initial_title, window, cx);
            s
        });
        let phone_in = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .placeholder("Phone number")
                .auto_grow(1, 1);
            s.set_value(initial_phone, window, cx);
            s
        });
        let pronouns_in = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .placeholder("Pronouns (optional)")
                .auto_grow(1, 1);
            s.set_value(initial_pronouns, window, cx);
            s
        });
        let tz_in = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .placeholder("Time zone (e.g. Europe/Stockholm)")
                .auto_grow(1, 1);
            s.set_value(initial_tz, window, cx);
            s
        });

        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (muted, border) = (theme.muted_foreground, theme.border);
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
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .child(Textarea::new(state).appearance(false).bordered(false)),
                    )
            };
            let this_save = this.clone();
            let (d_save, t_save, p_save, pr_save, tz_save, fields_save) = (
                display_in.clone(),
                title_in.clone(),
                phone_in.clone(),
                pronouns_in.clone(),
                tz_in.clone(),
                loaded_fields.clone(),
            );
            dialog.title("Edit your profile").w(px(460.)).child(
                v_flex()
                    .gap_3()
                    .child(field("Display name", &display_in))
                    .child(field("Title", &title_in))
                    .child(field("Phone", &phone_in))
                    .child(field("Pronouns", &pronouns_in))
                    .child(field("Time zone", &tz_in))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .pt_2()
                            .child(
                                Button::new("profile-edit-cancel")
                                    .outline()
                                    .label("Cancel")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("profile-edit-save")
                                    .primary()
                                    .label("Save Changes")
                                    .on_click(move |_, window, cx| {
                                        let pronouns_val =
                                            pr_save.read(cx).value().trim().to_owned();
                                        let tz_val = tz_save.read(cx).value().trim().to_owned();
                                        let update = slackless_core::api::ProfileUpdate {
                                            display_name: Some(
                                                d_save.read(cx).value().trim().to_owned(),
                                            ),
                                            title: Some(t_save.read(cx).value().trim().to_owned()),
                                            phone: Some(p_save.read(cx).value().trim().to_owned()),
                                            pronouns: Some(pronouns_val.clone()),
                                            tz: (!tz_val.is_empty()).then_some(tz_val),
                                            ..Default::default()
                                        };
                                        if let Some(app) = this_save.upgrade() {
                                            app.update(cx, |app, cx| {
                                                let me_id = app.fixtures.self_user.clone();
                                                app.profile.update(cx, |profile, cx| {
                                                    profile.set_user_profile(
                                                        me_id,
                                                        (!pronouns_val.is_empty())
                                                            .then_some(pronouns_val),
                                                        fields_save.clone(),
                                                        cx,
                                                    );
                                                });
                                                if !app.fixtures.sample {
                                                    app.bridge
                                                        .send(Command::UpdateProfile { update });
                                                }
                                                notify(
                                                    "Profile updated",
                                                    "Your profile changes were saved.",
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

    /// A `file://` URL for `path`, percent-encoded (Finder opens folder URLs).
    pub(super) fn file_url(path: &std::path::Path) -> String {
        let mut url = String::from("file://");
        for byte in path.as_os_str().to_string_lossy().bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                    url.push(byte as char)
                }
                _ => url.push_str(&format!("%{byte:02X}")),
            }
        }
        url
    }

    /// Open Slack's in-app Downloads list (`Cmd+Shift+J`): shows every file downloaded in this
    /// session with Open / Show in Finder / Retry controls plus a shortcut to `~/Downloads`.
    pub(super) fn open_downloads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let downloads_dir = std::env::var_os("HOME").map(|home| {
            let mut p = std::path::PathBuf::from(home);
            p.push("Downloads");
            p
        });
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (border, muted, foreground, danger) = (
                theme.border,
                theme.muted_foreground,
                theme.foreground,
                theme.danger,
            );
            let items = crate::media::downloads(cx);
            let has_finished = items
                .iter()
                .any(|d| !matches!(d.status, crate::media::DownloadStatus::InProgress));
            let folder = downloads_dir.clone();
            let header_actions = h_flex()
                .items_center()
                .gap_2()
                .when_some(folder, |row, path| {
                    row.child(
                        Button::new("downloads-open-folder")
                            .outline()
                            .with_size(Size::Small)
                            .icon(IconName::Folder)
                            .label("Open folder")
                            .on_click(move |_, _, cx| {
                                cx.open_url(&Self::file_url(&path));
                            }),
                    )
                })
                .when(has_finished, |row| {
                    row.child(
                        Button::new("downloads-clear")
                            .ghost()
                            .with_size(Size::Small)
                            .label("Clear list")
                            .on_click(|_, window, cx| {
                                crate::media::clear_finished_downloads(cx);
                                window.refresh();
                            }),
                    )
                });
            let rows: Vec<_> = items
                .into_iter()
                .map(|item| {
                    let (status_line, is_err) = match &item.status {
                        crate::media::DownloadStatus::InProgress => {
                            ("Downloading\u{2026}".to_owned(), false)
                        }
                        crate::media::DownloadStatus::Completed(_) => {
                            let size = item
                                .bytes
                                .map(|b| crate::files::human_size(b as i64))
                                .unwrap_or_default();
                            if size.is_empty() {
                                ("Completed".to_owned(), false)
                            } else {
                                (format!("{size} \u{b7} Completed"), false)
                            }
                        }
                        crate::media::DownloadStatus::Failed(err) => {
                            (format!("Failed: {err}"), true)
                        }
                    };
                    let actions = match &item.status {
                        crate::media::DownloadStatus::InProgress => h_flex().into_any_element(),
                        crate::media::DownloadStatus::Completed(path) => {
                            let open_path = path.clone();
                            let reveal_path = path.clone();
                            h_flex()
                                .flex_none()
                                .gap_1()
                                .child(
                                    Button::new(("dl-open", item.id))
                                        .outline()
                                        .with_size(Size::XSmall)
                                        .label("Open")
                                        .on_click(move |_, _, _| {
                                            crate::media::open_local_file(&open_path);
                                        }),
                                )
                                .child(
                                    Button::new(("dl-reveal", item.id))
                                        .ghost()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Folder)
                                        .tooltip("Show in Finder")
                                        .on_click(move |_, _, _| {
                                            crate::media::reveal_in_finder(&reveal_path);
                                        }),
                                )
                                .into_any_element()
                        }
                        crate::media::DownloadStatus::Failed(_) => {
                            let url = item.url.clone();
                            let name = item.name.clone();
                            Button::new(("dl-retry", item.id))
                                .outline()
                                .with_size(Size::XSmall)
                                .icon(IconName::RotateCcw)
                                .label("Retry")
                                .on_click(move |_, window, cx| {
                                    crate::media::download(url.clone(), name.clone(), window, cx);
                                })
                                .into_any_element()
                        }
                    };
                    h_flex()
                        .id(("dl-row", item.id))
                        .items_center()
                        .gap_3()
                        .py_2()
                        .border_b_1()
                        .border_color(border)
                        .child(
                            Icon::new(IconName::Download)
                                .size(px(16.))
                                .text_color(if is_err { danger } else { muted }),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(foreground)
                                        .child(item.name),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(if is_err { danger } else { muted })
                                        .child(status_line),
                                ),
                        )
                        .child(actions)
                })
                .collect();
            let empty = rows.is_empty().then(|| {
                v_flex()
                    .items_center()
                    .gap_2()
                    .py_8()
                    .text_sm()
                    .text_color(muted)
                    .child(
                        Icon::new(IconName::Download)
                            .size(px(28.))
                            .text_color(muted),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(foreground)
                            .child("No downloads yet"),
                    )
                    .child("Files you download from messages appear here.")
            });
            dialog.title("Downloads").w(px(560.)).child(
                v_flex().gap_3().child(header_actions).child(
                    v_flex()
                        .id("downloads-list")
                        .max_h(px(380.))
                        .overflow_y_scroll()
                        .children(rows)
                        .children(empty),
                ),
            )
        });
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
        if let Some(team) = self.fixtures.workspaces.get(index) {
            if let Err(e) =
                slackless_auth::SessionStore::new().and_then(|store| store.remove(&team.id))
            {
                eprintln!("slackless: could not remove the stored session ({e})");
            }
        }
        cx.quit();
    }

    /// Open Preferences in its own standalone native OS window (`F-CORE-011`), focusing the
    /// existing window if already open.
    pub(super) fn open_preferences(
        &mut self,
        section: Option<Section>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh_storage_stats(cx);
        if let Some(section) = section {
            self.preferences
                .update(cx, |prefs, cx| prefs.show(section, cx));
        }
        if let Some(handle) = self.prefs_window {
            if handle
                .update(cx, |_, win, _| {
                    win.activate_window();
                })
                .is_ok()
            {
                return;
            }
            self.prefs_window = None;
        }
        let display_id = window.display(cx).map(|d| d.id());
        self.prefs_window =
            crate::preferences::open_preferences_window(self.preferences.clone(), display_id, cx);
    }

    pub(super) fn open_status_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_hover_card(cx);
        let status = self.self_state.status.clone();
        let workspace = SharedString::from(self.fixtures.workspaces[self.workspace].name.clone());
        let presets = self.fixtures.status_presets.clone();
        self.status_picker.update(cx, |picker, cx| {
            picker.set_presets(presets, cx);
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
            |this: &mut App, _, event: &ViewerEvent, _, cx| match event {
                ViewerEvent::Close => this.close_image_viewer(cx),
            },
        );
        // A replaced viewer drops its subscription with it.
        self._subscriptions.push(subscription);
        self.image_viewer = Some(viewer);
        cx.notify();
    }

    /// Closes the image viewer, dropping the large rendition it holds.
    pub(super) fn close_image_viewer(&mut self, cx: &mut Context<Self>) {
        if self.image_viewer.take().is_some() {
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

    /// Slack's profile pane. Opened from an open thread it stacks over the thread, which
    /// stays loaded under it: the transcript drops the thread's tint while the profile
    /// shows, and the profile's back chevron returns to the thread.
    pub(super) fn open_profile(&mut self, user: UserId, cx: &mut Context<Self>) {
        self.close_hover_card(cx);
        let over_thread = self.thread_open;
        if !self.fixtures.sample {
            self.bridge.send(Command::FetchUserProfile(user.clone()));
        }
        self.profile
            .update(cx, |profile, cx| profile.open(user, over_thread, cx));
        self.profile_open = true;
        if over_thread {
            self.messages
                .update(cx, |list, cx| list.set_open_thread(None, cx));
        }
        self.pins_open = false;
        cx.notify();
    }

    // ---------------------------------------------------------------------------
    // Slack's member profile hover card (`hover_card.rs`): dwell open, grace close.
    // ---------------------------------------------------------------------------

    /// The pointer entered a profile hover trigger (an @mention pill or an author name):
    /// dwell before showing, so a sweeping pointer never flashes the card. A user the
    /// snapshot lacks (an external member) is resolved right away, so the card has a name
    /// by the time the dwell ends.
    pub(crate) fn profile_hover_entered(
        &mut self,
        trigger: hover_card::Trigger,
        cx: &mut Context<Self>,
    ) {
        let unchanged = self.profile_hover.trigger.as_ref() == Some(&trigger);
        let same_open_card = self
            .profile_hover
            .card
            .as_ref()
            .is_some_and(|card| card.key == trigger.key && card.user == trigger.user);
        self.profile_hover.grace = None;
        self.profile_hover.trigger = Some(trigger.clone());
        if self.fixtures.user(&trigger.user).is_none() {
            self.resolve_users(std::iter::once(trigger.user.clone()));
        }
        if unchanged || same_open_card {
            return;
        }
        let (key, user) = (trigger.key.clone(), trigger.user.clone());
        self.profile_hover.dwell = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(hover_card::DWELL_MS))
                .await;
            let _ = this.update(cx, |app, cx| app.hover_dwell_elapsed(&key, &user, cx));
        }));
    }

    /// The pointer left the trigger registered under `key`. A leave from any other element
    /// (the pointer crossed straight onto a newer trigger before this frame's hover diff)
    /// is stale and drops.
    pub(crate) fn profile_hover_left(&mut self, key: &SharedString, cx: &mut Context<Self>) {
        if !self
            .profile_hover
            .trigger
            .as_ref()
            .is_some_and(|trigger| trigger.key == *key)
        {
            return;
        }
        self.profile_hover.trigger = None;
        self.profile_hover.dwell = None; // left before the dwell: no card
        self.hover_card_maybe_close(cx);
    }

    /// The dwell elapsed with the pointer still on `key`'s trigger: show the card.
    pub(super) fn hover_dwell_elapsed(
        &mut self,
        key: &SharedString,
        user: &UserId,
        cx: &mut Context<Self>,
    ) {
        if !self
            .profile_hover
            .trigger
            .as_ref()
            .is_some_and(|trigger| trigger.key == *key && trigger.user == *user)
        {
            return; // the pointer moved on (or left) before the dwell ended
        }
        let trigger = self
            .profile_hover
            .trigger
            .clone()
            .expect("the guard above matched the trigger");
        self.profile_hover.card = Some(trigger);
        self.profile_hover.grace = None;
        cx.notify();
    }

    /// The grace elapsed without the pointer returning to the trigger or into the card.
    pub(super) fn hover_grace_elapsed(&mut self, cx: &mut Context<Self>) {
        if self.profile_hover.trigger.is_some() || self.profile_hover.inside {
            return; // the pointer came back before the grace closed the card
        }
        self.profile_hover.grace = None;
        if self.profile_hover.card.take().is_some() {
            cx.notify();
        }
    }

    /// The pointer entered / left the open card: entering cancels a pending close (the
    /// pointer crossed the gap from its trigger); leaving starts the grace.
    pub(super) fn hover_card_hovered(&mut self, inside: bool, cx: &mut Context<Self>) {
        if self.profile_hover.inside == inside {
            return;
        }
        self.profile_hover.inside = inside;
        if inside {
            self.profile_hover.grace = None;
        } else {
            self.hover_card_maybe_close(cx);
        }
    }

    /// Start the card's close grace, unless the pointer is still on its trigger or inside the
    /// card, or a grace is already running.
    pub(super) fn hover_card_maybe_close(&mut self, cx: &mut Context<Self>) {
        if self.profile_hover.card.is_none()
            || self.profile_hover.trigger.is_some()
            || self.profile_hover.inside
            || self.profile_hover.grace.is_some()
        {
            return;
        }
        self.profile_hover.grace = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(hover_card::GRACE_MS))
                .await;
            let _ = this.update(cx, |app, cx| app.hover_grace_elapsed(cx));
        }));
    }

    /// Close the card outright: a click outside, Escape, a conversation switch, or an
    /// action opened from it. The trigger stays, so the card does not come back until the
    /// pointer re-enters it.
    pub(super) fn close_hover_card(&mut self, cx: &mut Context<Self>) {
        let closed = self.profile_hover.card.take().is_some();
        self.profile_hover.inside = false;
        self.profile_hover.grace = None;
        self.profile_hover.dwell = None;
        if closed {
            cx.notify();
        }
    }

    /// Slack's channel details dialog, opened by clicking the channel name in the header or
    /// its members chip: the About and Members tabs over the channel's record and member
    /// list. A creator missing from the snapshot resolves through the core, like any other
    /// unknown person.
    pub(super) fn open_channel_details(
        &mut self,
        channel: &ChannelId,
        tab: channel_details::DetailsTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(conversation) = self.fixtures.conversation(channel).cloned() else {
            return;
        };
        if let Some(creator) = &conversation.creator {
            self.resolve_users(std::iter::once(creator.clone()));
        }
        // Live mode without a member list yet keeps the Members tab on its placeholder until
        // `Event::ConversationInfo` lands.
        let stored = self.member_ids(channel, &conversation).to_vec();
        let loading = stored.is_empty() && self.bridge.store().is_some();
        self.details.update(cx, |details, cx| {
            details.open(
                self.fixtures.clone(),
                channel.clone(),
                stored,
                loading,
                tab,
                cx,
            )
        });
        self.bridge.send(Command::FetchChannelManagers {
            channel: channel.clone(),
        });
        // Slack's title row: a private channel's lock, else the hash, beside the name in the
        // dialog's bold display size (the close X is the dialog's own).
        let title_name: SharedString = if conversation.is_private {
            conversation
                .name
                .as_deref()
                .unwrap_or_else(|| channel.as_str())
                .to_owned()
                .into()
        } else {
            format!(
                "# {}",
                conversation
                    .name
                    .as_deref()
                    .unwrap_or_else(|| channel.as_str())
            )
            .into()
        };
        let title_private = conversation.is_private;
        let details = self.details.clone();
        let focus_find_people = Rc::new(Cell::new(true));
        window.open_dialog(cx, move |dialog, window, cx| {
            // Slack puts the caret in "Find people" as the dialog opens.
            if focus_find_people.replace(false) {
                details.read(cx).find_people_focus(cx).focus(window, cx);
            }
            let mut title = h_flex()
                .items_center()
                .gap_1p5()
                .text_size(px(22.))
                .font_weight(FontWeight::BOLD);
            title = if title_private {
                title
                    .child(Icon::new(IconName::Lock))
                    .child(title_name.clone())
            } else {
                title.child(title_name.clone())
            };
            // Slack's 580pt dialog: its rules run edge to edge, so the content pads itself and
            // the title row carries the close X.
            let title_row = h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .px(px(28.))
                .child(title)
                .child(
                    Button::new("details-close")
                        .ghost()
                        .icon(IconName::Close)
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                );
            dialog
                .title(title_row)
                .w(px(580.))
                .px_0()
                .pb_0()
                .close_button(false)
                .child(details.clone())
        });
    }

    /// The channel details dialog's actions.
    pub(super) fn details_event(
        &mut self,
        event: &ChannelDetailsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ChannelDetailsEvent::Leave(channel) => {
                window.close_dialog(cx);
                self.leave(channel.clone(), window, cx);
            }
            ChannelDetailsEvent::Rename { channel, name } => {
                self.bridge.send(Command::RenameChannel {
                    channel: channel.clone(),
                    name: name.clone(),
                });
                actions::notify("Renamed channel", format!("#{name}"), window, cx);
            }
            ChannelDetailsEvent::Archive(channel) => {
                window.close_dialog(cx);
                let this = cx.weak_entity();
                let ch = channel.clone();
                let label = self.label(&ch);
                window.open_dialog(cx, move |dialog, _, cx| {
                    let muted = cx.theme().muted_foreground;
                    let (this, ch) = (this.clone(), ch.clone());
                    dialog.title(format!("Archive {label}?")).w(px(420.)).child(
                        v_flex()
                            .gap_3()
                            .p_4()
                            .child(div().text_sm().text_color(muted).child(
                                "Archiving stops new messages from being posted. Message history remains searchable and the channel can be unarchived later.",
                            ))
                            .child(
                                h_flex()
                                    .justify_end()
                                    .gap_2()
                                    .child(
                                        Button::new("archive-cancel")
                                            .outline()
                                            .with_size(Size::Small)
                                            .label("Cancel")
                                            .on_click(|_, window, cx| window.close_dialog(cx)),
                                    )
                                    .child(
                                        Button::new("archive-confirm")
                                            .danger()
                                            .with_size(Size::Small)
                                            .label("Archive channel")
                                            .on_click(move |_, window, cx| {
                                                window.close_dialog(cx);
                                                let _ = this.update(cx, |app, cx| {
                                                    app.bridge
                                                        .send(Command::ArchiveChannel(ch.clone()));
                                                    actions::notify(
                                                        "Channel archived",
                                                        app.label(&ch),
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }),
                                    ),
                            ),
                    )
                });
            }
            ChannelDetailsEvent::Unarchive(channel) => {
                window.close_dialog(cx);
                self.bridge.send(Command::UnarchiveChannel(channel.clone()));
                actions::notify("Channel unarchived", self.label(channel), window, cx);
            }
            ChannelDetailsEvent::AddPeople(channel) => {
                window.close_dialog(cx);
                self.open_add_people(channel.clone(), window, cx);
            }
            ChannelDetailsEvent::SetTopic { channel, topic } => {
                self.bridge.send(Command::SetChannelTopic {
                    channel: channel.clone(),
                    topic: topic.clone(),
                });
            }
            ChannelDetailsEvent::SetPurpose { channel, purpose } => {
                self.bridge.send(Command::SetChannelPurpose {
                    channel: channel.clone(),
                    purpose: purpose.clone(),
                });
            }
            ChannelDetailsEvent::StartHuddle => self.start_huddle(window, cx),
            ChannelDetailsEvent::SetNotifyLevel { channel, level } => {
                self.bridge.send(Command::SetNotifyLevel {
                    channel: channel.clone(),
                    level: *level,
                });
            }
            ChannelDetailsEvent::OpenProfile(user) => {
                window.close_dialog(cx);
                self.open_profile(user.clone(), cx);
            }
        }
    }

    /// Confirm permanent deletion of a user-owned file/clip (`files.delete`, `F-CORE-130`).
    pub(super) fn confirm_delete_file(
        &self,
        file_id: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            let this = this.clone();
            let file_id = file_id.clone();
            dialog.title("Delete file?").w(px(420.)).child(
                v_flex()
                    .gap_4()
                    .child(div().text_color(muted).child(format!(
                        "Permanently delete \"{name}\"? This cannot be undone."
                    )))
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("delete-file-cancel")
                                    .label("Cancel")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("delete-file-confirm")
                                    .danger()
                                    .label("Delete")
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let _ = this.update(cx, |app, _| {
                                            app.bridge.send(Command::DeleteFile {
                                                file_id: file_id.clone(),
                                            });
                                        });
                                    }),
                            ),
                    ),
            )
        });
    }

    /// Confirm and clear local SQLite message cache (`Preferences -> Advanced -> Clear local cache`).
    pub(super) fn confirm_reset_cache(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            dialog
                .title("Clear local cache?")
                .w(px(420.))
                .child(
                    v_flex()
                        .gap_4()
                        .child(div().text_color(muted).child(
                            "Removes cached search & message data from disk (your signed-in sessions stay intact). Relaunch slackless to re-sync fresh history.",
                        ))
                        .child(
                            h_flex()
                                .gap_2()
                                .justify_end()
                                .child(Button::new("reset-cache-cancel").label("Cancel").on_click(
                                    |_, window, cx| window.close_dialog(cx),
                                ))
                                .child(
                                    Button::new("reset-cache-confirm")
                                        .danger()
                                        .label("Clear cache")
                                        .on_click(|_, window, cx| {
                                            window.close_dialog(cx);
                                            if let Some(dir) = crate::bridge::store_dir() {
                                                if let Ok(entries) = std::fs::read_dir(&dir) {
                                                    for entry in entries.flatten() {
                                                        let path = entry.path();
                                                        let name = path
                                                            .file_name()
                                                            .and_then(|n| n.to_str())
                                                            .unwrap_or_default();
                                                        if name.ends_with(".sqlite")
                                                            || name.ends_with(".sqlite-wal")
                                                            || name.ends_with(".sqlite-shm")
                                                        {
                                                            let _ = std::fs::remove_file(path);
                                                        }
                                                    }
                                                }
                                            }
                                            notify(
                                                "Cache cleared",
                                                "Restart slackless to re-sync from Slack.".to_owned(),
                                                window,
                                                cx,
                                            );
                                        }),
                                ),
                        ),
                )
        });
    }

    /// Gather SQLite & media cache metrics for the active workspace and push them into
    /// `Preferences -> Advanced -> Local storage & maintenance`.
    pub(super) fn refresh_storage_stats(&self, cx: &mut Context<Self>) {
        fn dir_bytes(dir: &std::path::Path) -> u64 {
            std::fs::read_dir(dir)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        }
        let info = self
            .bridge
            .store()
            .zip(self.bridge.team_id())
            .and_then(|(store, team)| {
                let stats = store.lock().ok()?.stats(team).ok()?;
                let media_cache_bytes = dir_bytes(&std::env::temp_dir().join("slackless-media"))
                    .saturating_add(dir_bytes(&std::env::temp_dir().join("slackless-audio")));
                Some(crate::preferences::StorageInfo {
                    messages: stats.messages,
                    revisions: stats.revisions,
                    conversations: stats.conversations,
                    users: stats.users,
                    offline_queued: stats.offline_queued,
                    db_bytes: stats.db_bytes(),
                    media_cache_bytes,
                })
            });
        self.preferences
            .update(cx, |prefs, cx| prefs.set_storage_stats(info, cx));
    }

    /// Run `PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA optimize;` on the active workspace store.
    pub(super) fn compact_storage(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((store, team)) = self.bridge.store().zip(self.bridge.team_id()) else {
            notify(
                "No local database",
                "Compact is available in live workspaces.",
                window,
                cx,
            );
            return;
        };
        let before = store
            .lock()
            .ok()
            .and_then(|s| s.stats(team).ok())
            .map(|s| s.db_bytes())
            .unwrap_or(0);
        let res = store.lock().ok().map(|s| s.vacuum());
        match res {
            Some(Ok(())) => {
                let after = store
                    .lock()
                    .ok()
                    .and_then(|s| s.stats(team).ok())
                    .map(|s| s.db_bytes())
                    .unwrap_or(0);
                self.refresh_storage_stats(cx);
                let saved = before.saturating_sub(after);
                let msg = if saved > 0 {
                    format!("Reclaimed {} KiB and optimized FTS5 indexes.", saved / 1024)
                } else {
                    "Database is already compact; FTS5 statistics refreshed.".to_owned()
                };
                notify("Database compacted", msg, window, cx);
            }
            Some(Err(err)) => {
                notify("Compact failed", err.to_string(), window, cx);
            }
            None => {
                notify(
                    "Compact failed",
                    "Could not acquire database lock.",
                    window,
                    cx,
                );
            }
        }
    }

    /// Prune temporary downloaded media cache files (`slackless-media` + `slackless-audio`).
    pub(super) fn prune_media_cache(&self, window: &mut Window, cx: &mut Context<Self>) {
        let mut removed = 0u64;
        let mut dirs: Vec<std::path::PathBuf> = ["slackless-media", "slackless-audio"]
            .into_iter()
            .map(|folder| std::env::temp_dir().join(folder))
            .collect();
        if let Some(cache) = slackless_core::Fetcher::disk_cache_dir() {
            dirs.push(cache);
        }
        for dir in dirs {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if let Ok(meta) = entry.metadata() {
                        if meta.is_file() && std::fs::remove_file(entry.path()).is_ok() {
                            removed = removed.saturating_add(meta.len());
                        }
                    }
                }
            }
        }
        self.refresh_storage_stats(cx);
        let msg = if removed > 0 {
            format!(
                "Removed {} KiB of cached media files.",
                (removed / 1024).max(1)
            )
        } else {
            "Media cache was already clean.".to_owned()
        };
        notify("Media cache pruned", msg, window, cx);
    }

    /// Open a modal dialog showing every locally recorded revision of `(channel, ts)` from
    /// SQLite's immutable append-only `message_revision` log.
    pub(super) fn open_revision_history(
        &self,
        channel: ChannelId,
        ts: Ts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fixtures = self.fixtures.clone();
        let revisions: Vec<(String, bool, String)> =
            if let (Some(store), Some(team)) = (self.bridge.store(), self.bridge.team_id()) {
                store
                    .lock()
                    .ok()
                    .and_then(|s| s.revisions(team, &channel, &ts).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|rev| {
                        let stamp = if rev.edit_ts.is_empty() {
                            format!("Original · {}", rev.message.ts)
                        } else if rev.deleted {
                            format!("Deleted · {}", rev.edit_ts)
                        } else {
                            format!("Edited · {}", rev.edit_ts)
                        };
                        let text = fixtures.plain_text(&rev.message);
                        (stamp, rev.deleted, text)
                    })
                    .collect()
            } else {
                fixtures
                    .messages
                    .get(&channel)
                    .and_then(|list| list.iter().find(|m| m.ts == ts))
                    .map(|m| {
                        vec![(
                            format!("Original · {}", m.ts),
                            false,
                            fixtures.plain_text(m),
                        )]
                    })
                    .unwrap_or_default()
            };
        let subtitle = format!("{} · {}", self.label(&channel), ts);
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (border, muted, danger, card_bg) = (
                theme.border,
                theme.muted_foreground,
                theme.danger,
                theme.secondary.opacity(0.35),
            );
            let mut body = v_flex()
                .id("revision-history-list")
                .max_h(px(440.))
                .overflow_y_scroll()
                .gap_2()
                .child(div().text_xs().text_color(muted).child(subtitle.clone()));
            if revisions.is_empty() {
                body = body.child(
                    div()
                        .py_6()
                        .text_sm()
                        .text_color(muted)
                        .child("No stored revisions found for this message."),
                );
            } else {
                for (ix, (stamp, deleted, text)) in revisions.iter().enumerate() {
                    let copy_text = text.clone();
                    body = body.child(
                        v_flex()
                            .gap_1()
                            .p_3()
                            .rounded_md()
                            .border_1()
                            .border_color(if *deleted {
                                danger.opacity(0.45)
                            } else {
                                border
                            })
                            .bg(if *deleted {
                                danger.opacity(0.08)
                            } else {
                                card_bg
                            })
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(if *deleted { danger } else { muted })
                                            .child(format!("#{} · {stamp}", ix + 1)),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!("rev-copy-{ix}")))
                                            .ghost()
                                            .with_size(Size::XSmall)
                                            .label("Copy")
                                            .on_click(move |_, window, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    copy_text.clone(),
                                                ));
                                                notify(
                                                    "Revision copied",
                                                    "Copied message text to clipboard.",
                                                    window,
                                                    cx,
                                                );
                                            }),
                                    ),
                            )
                            .child(div().text_sm().child(if text.is_empty() {
                                "(empty)".to_owned()
                            } else {
                                text.clone()
                            })),
                    );
                }
            }
            dialog
                .title(format!("Edit history ({} revisions)", revisions.len()))
                .w(px(560.))
                .child(body)
        });
    }

    /// Export the active conversation's locally archived messages as Markdown or pretty JSON
    /// using a native save dialog.
    pub(super) fn export_conversation_transcript(
        &self,
        markdown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let channel = self.channel.clone();
        let label = self.label(&channel);
        let safe_name: String = label
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let safe_name = safe_name.trim_matches('-');
        let stem = if safe_name.is_empty() {
            channel.as_str()
        } else {
            safe_name
        };
        let ext = if markdown { "md" } else { "json" };
        let suggested = format!("{stem}-transcript.{ext}");
        let start_dir = {
            let custom = self.preferences.read(cx).settings().download_dir.clone();
            if !custom.is_empty() {
                std::path::PathBuf::from(custom)
            } else {
                std::env::var_os("HOME")
                    .map(|h| std::path::PathBuf::from(h).join("Downloads"))
                    .unwrap_or_else(std::env::temp_dir)
            }
        };
        let fixtures = self.fixtures.clone();
        let store = self.bridge.store().cloned();
        let team = self.bridge.team_id().cloned();
        let rx = cx.prompt_for_new_path(&start_dir, Some(&suggested));
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move {
                    let messages: Vec<slackless_core::types::Message> =
                        if let (Some(store), Some(team)) = (store, team) {
                            store
                                .lock()
                                .ok()
                                .and_then(|s| s.channel_history(&team, &channel, None, 10_000).ok())
                                .map(|rows| rows.into_iter().map(|r| r.message).collect())
                                .unwrap_or_default()
                        } else {
                            fixtures.messages.get(&channel).cloned().unwrap_or_default()
                        };
                    let count = messages.len();
                    let body = if markdown {
                        let mut md = format!("# {label}\n\n");
                        for msg in &messages {
                            let author = msg
                                .user
                                .as_ref()
                                .map(|u| fixtures.user_name(u).to_owned())
                                .or_else(|| msg.bot_profile.as_ref().map(|b| b.name.clone()))
                                .unwrap_or_else(|| "unknown".to_owned());
                            md.push_str(&format!("**{author}** (`{}`)\n\n", msg.ts));
                            let text = fixtures.plain_text(msg);
                            if !text.is_empty() {
                                md.push_str(&text);
                                md.push_str("\n\n");
                            }
                            if !msg.files.is_empty() {
                                let names: Vec<&str> = msg
                                    .files
                                    .iter()
                                    .filter_map(|f| f.name.as_deref().or(f.title.as_deref()))
                                    .collect();
                                if !names.is_empty() {
                                    md.push_str(&format!("_Files:_ {}\n\n", names.join(", ")));
                                }
                            }
                            md.push_str("---\n\n");
                        }
                        md
                    } else {
                        let rows: Vec<_> = messages
                            .iter()
                            .map(|msg| {
                                let author = msg
                                    .user
                                    .as_ref()
                                    .map(|u| fixtures.user_name(u).to_owned())
                                    .or_else(|| msg.bot_profile.as_ref().map(|b| b.name.clone()));
                                crate::blockkit::serde_json::json!({
                                    "ts": msg.ts.as_str(),
                                    "user": msg.user.as_ref().map(|u| u.as_str()),
                                    "author": author,
                                    "text": fixtures.plain_text(msg),
                                    "thread_ts": msg.thread_ts.as_ref().map(|t| t.as_str()),
                                })
                            })
                            .collect();
                        crate::blockkit::serde_json::to_string_pretty(
                            &crate::blockkit::serde_json::json!({
                                "channel": channel.as_str(),
                                "label": label,
                                "count": count,
                                "messages": rows,
                            }),
                        )
                        .unwrap_or_default()
                    };
                    std::fs::write(&path, body).map(|()| (count, path))
                })
                .await;
            let _ = cx.update(|window, cx| match result {
                Ok((count, path)) => notify(
                    "Transcript exported",
                    format!("Saved {count} messages to {}", path.display()),
                    window,
                    cx,
                ),
                Err(err) => notify("Export failed", err.to_string(), window, cx),
            });
        })
        .detach();
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
                Button::new("modal-close")
                    .outline()
                    .label(close_label.clone())
                    .on_click(move |_, window, cx| {
                        if notify_on_close {
                            if let Some(app) = this_close.upgrade() {
                                app.update(cx, |app, _| {
                                    app.bridge.send(Command::CloseModal {
                                        view_id: close_id.clone(),
                                        hash: close_hash.clone(),
                                    });
                                });
                            }
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
                    Button::new("modal-submit")
                        .primary()
                        .label(submit_text.clone())
                        .on_click(
                            move |_, window, cx| match crate::blockkit::collect_modal_state(
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
                            },
                        ),
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
                                    Button::new("mark-all-cancel")
                                        .label("Cancel")
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    Button::new("mark-all-confirm")
                                        .primary()
                                        .label("Mark all as read")
                                        .on_click(move |_, window, cx| {
                                            window.close_dialog(cx);
                                            let _ =
                                                this.update(cx, |app, cx| app.mark_all_read(cx));
                                        }),
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
        let now = now_secs() as i64;
        let offset = crate::messages::local_offset_now();
        let tomorrow_day = (now + offset).div_euclid(86_400) + 1;
        let (y, m, d) = civil_from_days(tomorrow_day);
        let default_date = format!("{y:04}-{m:02}-{d:02}");

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
            s.set_value("09:00", window, cx);
            s
        });
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (muted, border) = (theme.muted_foreground, theme.border);
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
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .child(Textarea::new(state).appearance(false).bordered(false)),
                    )
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
                                Button::new("sched-custom-cancel")
                                    .outline()
                                    .label("Cancel")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("sched-custom-ok")
                                    .primary()
                                    .label("Schedule message")
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
