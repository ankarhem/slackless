//! Preferences section views (Notifications, Navigation, Home, Appearance, Messages, Language, Audio/Video, Accessibility, Mark as read, Advanced).

use super::*;

impl Preferences {
    pub(super) fn nav(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (foreground, muted, hover, active, active_fg, border) = (
            theme.foreground,
            theme.muted_foreground,
            theme.accent,
            theme.sidebar_accent,
            theme.sidebar_accent_foreground,
            theme.border,
        );
        v_flex()
            .w(px(220.))
            .flex_none()
            .h_full()
            .gap_0p5()
            .p_2()
            .border_r_1()
            .border_color(border)
            .children(Section::ALL.iter().enumerate().map(|(ix, &section)| {
                let selected = section == self.section;
                let color = if selected { active_fg } else { foreground };
                h_flex()
                    .id(("prefs-section", ix))
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(color)
                    .when(selected, |row| {
                        row.bg(active).font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!selected, |row| row.hover(|el| el.bg(hover)))
                    .child(
                        Icon::new(section.icon())
                            .text_color(if selected { active_fg } else { muted })
                            .size(px(16.)),
                    )
                    .child(section.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.show(section, cx)))
            }))
    }

    /// A titled group of controls, with an optional description under the title.
    fn group(
        &self,
        title: &str,
        hint: Option<&str>,
        body: impl IntoElement,
        cx: &Context<Self>,
    ) -> Div {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        v_flex()
            .gap_2()
            .pb_5()
            .mb_5()
            .border_b_1()
            .border_color(border)
            .child(div().font_weight(FontWeight::BOLD).child(title.to_owned()))
            .children(hint.map(|hint| div().text_sm().text_color(muted).child(hint.to_owned())))
            .child(body)
    }

    fn checkbox(
        &mut self,
        id: &'static str,
        label: &'static str,
        flag: Flag,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let on = *flag(&mut self.settings);
        let theme = cx.theme();
        let (fill, fill_fg, border) = (
            theme.sidebar_accent,
            theme.sidebar_accent_foreground,
            theme.input,
        );
        h_flex()
            .id(id)
            .items_center()
            .gap_2()
            .py_0p5()
            .cursor_pointer()
            .text_sm()
            .child(
                div()
                    .size(px(16.))
                    .flex_none()
                    .rounded(px(3.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .when(on, |b| b.bg(fill).border_color(fill))
                    .when(!on, |b| b.border_color(border))
                    .when(on, |b| {
                        b.child(Icon::new(IconName::Check).text_color(fill_fg).size(px(12.)))
                    }),
            )
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                let value = flag(&mut this.settings);
                *value = !*value;
                this.changed(cx);
            }))
    }

    fn radios(
        &mut self,
        id: &'static str,
        options: &'static [&'static str],
        choice: Choice,
        cx: &mut Context<Self>,
    ) -> Div {
        let selected = *choice(&mut self.settings);
        self.radio_rows(
            id,
            options,
            selected,
            move |settings, ix| *choice(settings) = ix,
            cx,
        )
    }

    /// Radio rows for `options` with `selected` checked; `pick` stores a click.
    fn radio_rows(
        &self,
        id: &'static str,
        options: &'static [&'static str],
        selected: usize,
        pick: impl Fn(&mut Settings, usize) + Copy + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (fill, border) = (theme.sidebar_accent, theme.input);
        v_flex()
            .gap_1()
            .children(options.iter().enumerate().map(|(ix, label)| {
                let on = ix == selected;
                h_flex()
                    .id(ElementId::NamedInteger(id.into(), ix as u64))
                    .items_center()
                    .gap_2()
                    .py_0p5()
                    .cursor_pointer()
                    .text_sm()
                    .child(
                        div()
                            .size(px(16.))
                            .flex_none()
                            .rounded_full()
                            .border_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .border_color(if on { fill } else { border })
                            .when(on, |dot| {
                                dot.child(div().size(px(8.)).rounded_full().bg(fill))
                            }),
                    )
                    .child(*label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        pick(&mut this.settings, ix);
                        this.changed(cx);
                    }))
            }))
    }

    /// A select: the current option in a bordered box; clicking expands the option list
    /// beneath it.
    fn select(
        &mut self,
        id: &'static str,
        options: &'static [&'static str],
        choice: Choice,
        cx: &mut Context<Self>,
    ) -> Div {
        let selected = *choice(&mut self.settings);
        let open = self.open_select == Some(id);
        let theme = cx.theme();
        let (border, muted, popover, hover, active) = (
            theme.input,
            theme.muted_foreground,
            theme.popover,
            theme.accent,
            theme.sidebar_accent,
        );
        let active_fg = theme.sidebar_accent_foreground;
        let trigger = h_flex()
            .id(id)
            .w(px(300.))
            .items_center()
            .justify_between()
            .px_3()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .text_sm()
            .child(options[selected])
            .child(
                Icon::new(IconName::ChevronDown)
                    .text_color(muted)
                    .size(px(14.)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_select = if this.open_select == Some(id) {
                    None
                } else {
                    Some(id)
                };
                cx.notify();
            }));
        let list = open.then(|| {
            v_flex()
                .w(px(300.))
                .p_1()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(popover)
                .children(options.iter().enumerate().map(|(ix, label)| {
                    let on = ix == selected;
                    div()
                        .id(ElementId::NamedInteger(
                            SharedString::from(format!("{id}-opt")),
                            ix as u64,
                        ))
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_sm()
                        .when(on, |row| row.bg(active).text_color(active_fg))
                        .when(!on, |row| row.hover(|el| el.bg(hover)))
                        .child(*label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            *choice(&mut this.settings) = ix;
                            this.open_select = None;
                            this.changed(cx);
                        }))
                }))
        });
        v_flex().gap_1().child(trigger).children(list)
    }

    fn labeled(&self, label: &'static str, control: impl IntoElement) -> Div {
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(label),
            )
            .child(control)
    }

    fn notifications(&mut self, cx: &mut Context<Self>) -> Div {
        let desktop = self.checkbox(
            "notify-desktop",
            "Enable desktop notifications",
            |s| &mut s.desktop_notifications,
            cx,
        );
        let level = self.radios("notify-level", &NOTIFY_LEVELS, |s| &mut s.notify_level, cx);
        let mobile = self.checkbox(
            "notify-mobile",
            "Use different settings for my mobile devices",
            |s| &mut s.mobile_different,
            cx,
        );
        let mobile_level = if self.settings.mobile_different {
            let sel = self.select(
                "notify-mobile-level",
                &NOTIFY_LEVELS,
                |s| &mut s.mobile_notify_level,
                cx,
            );
            Some(self.labeled("On mobile, notify me about:", sel))
        } else {
            None
        };
        let mobile_handoff_select = self.select(
            "notify-mobile-handoff",
            &MOBILE_HANDOFF,
            |s| &mut s.mobile_handoff,
            cx,
        );
        let mobile_handoff = self.labeled(
            "When I'm not active on desktop, send notifications to my mobile devices:",
            mobile_handoff_select,
        );
        let status_pause = self.checkbox(
            "notify-status-pause",
            "Pause notifications automatically while a custom status is set",
            |s| &mut s.status_pauses_notifications,
            cx,
        );
        let threads = self.checkbox(
            "notify-threads",
            "Notify me about replies to threads I'm following",
            |s| &mut s.thread_replies,
            cx,
        );
        let ch_mentions = self.checkbox(
            "notify-ch-mentions",
            "Notify me about @channel, @here, and @everyone mentions",
            |s| &mut s.notify_channel_mentions,
            cx,
        );
        let huddle_invites = self.checkbox(
            "notify-huddle-invites",
            "Notify me when someone invites me to a huddle",
            |s| &mut s.notify_huddle_invites,
            cx,
        );
        let reminders = self.checkbox(
            "notify-reminders",
            "Notify me about Slackbot reminders and scheduled alerts",
            |s| &mut s.notify_reminders,
            cx,
        );
        let border = cx.theme().input;
        let keywords = div()
            .w(px(420.))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(border)
            .text_sm()
            .child(
                Textarea::new(&self.keywords)
                    .appearance(false)
                    .bordered(false),
            );
        let schedule_on = self.checkbox(
            "schedule-on",
            "Only send notifications during these hours",
            |s| &mut s.schedule_enabled,
            cx,
        );
        let days = self.select(
            "schedule-days",
            &SCHEDULE_DAYS,
            |s| &mut s.schedule_days,
            cx,
        );
        let custom_days_row = (self.settings.schedule_days == 2).then(|| {
            const LABELS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
            let theme = cx.theme();
            let (accent, fg, muted, border) = (
                theme.primary,
                crate::theme::avatar_foreground(),
                theme.muted_foreground,
                theme.border,
            );
            h_flex()
                .flex_wrap()
                .gap_1p5()
                .children(LABELS.iter().enumerate().map(|(ix, label)| {
                    let active = self.settings.schedule_custom_days[ix];
                    div()
                        .id(("schedule-day-chip", ix))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .text_xs()
                        .cursor_pointer()
                        .when(active, |d| {
                            d.bg(accent)
                                .border_color(accent)
                                .text_color(fg)
                                .font_weight(FontWeight::SEMIBOLD)
                        })
                        .when(!active, |d| d.border_color(border).text_color(muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.settings.schedule_custom_days[ix] =
                                !this.settings.schedule_custom_days[ix];
                            this.changed(cx);
                        }))
                        .child(*label)
                }))
        });
        let start = self.select("schedule-start", &HOURS, |s| &mut s.schedule_start, cx);
        let end = self.select("schedule-end", &HOURS, |s| &mut s.schedule_end, cx);
        let sound = self.select("notify-sound", &SOUNDS, |s| &mut s.sound, cx);
        let dm_sound = self.select("notify-dm-sound", &SOUNDS, |s| &mut s.dm_sound, cx);
        let preview = self.checkbox(
            "notify-preview",
            "Include a preview of the message in each notification",
            |s| &mut s.show_preview,
            cx,
        );
        let mute = self.checkbox(
            "notify-mute",
            "Mute all messaging sounds from Slack",
            |s| &mut s.mute_sounds,
            cx,
        );
        let dock_badge = self.checkbox(
            "notify-dock-badge",
            "Show a badge on Slack's icon to indicate new activity",
            |s| &mut s.show_dock_badge,
            cx,
        );
        let badge_unreads = self.checkbox(
            "notify-badge-unreads",
            "Also show a dot badge for any unread message (not just DMs & mentions)",
            |s| &mut s.badge_on_unreads,
            cx,
        );
        let bounce_dock = self.checkbox(
            "notify-bounce-dock",
            "Bounce the dock icon for mentions & direct messages",
            |s| &mut s.bounce_dock,
            cx,
        );
        let vip_box = div()
            .w(px(420.))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(border)
            .text_sm()
            .child(
                Textarea::new(&self.vip_input)
                    .appearance(false)
                    .bordered(false),
            );
        let urgent_override = self.checkbox(
            "notify-urgent-dm-override",
            "Allow urgent direct messages to break through Do Not Disturb & Focus mode",
            |s| &mut s.urgent_dm_override,
            cx,
        );
        let vip_sound_sel = self.select("notify-vip-sound", &SOUNDS, |s| &mut s.vip_sound, cx);
        let focus_status = self.checkbox(
            "notify-focus-auto-status",
            "Automatically set 'In focus mode' status during Focus mode",
            |s| &mut s.focus_auto_status,
            cx,
        );
        let calendar_sync = self.checkbox(
            "notify-calendar-status-sync",
            "Sync macOS Calendar busy events to 'In a meeting' status",
            |s| &mut s.calendar_status_sync,
            cx,
        );
        v_flex()
            .child(self.group(
                "Notify me about\u{2026}",
                None,
                v_flex()
                    .gap_2()
                    .child(desktop)
                    .child(level)
                    .child(mobile)
                    .children(mobile_level)
                    .child(mobile_handoff)
                    .child(status_pause)
                    .child(threads)
                    .child(ch_mentions)
                    .child(huddle_invites)
                    .child(reminders),
                cx,
            ))
            .child(self.group(
                "VIPs & Focus overrides",
                Some("Messages from VIP colleagues always notify you even during Do Not Disturb or Focus mode."),
                v_flex()
                    .gap_2()
                    .child(vip_box)
                    .child(self.labeled("Notification sound (VIPs)", vip_sound_sel))
                    .child(urgent_override)
                    .child(focus_status)
                    .child(calendar_sync),
                cx,
            ))
            .child(self.group(
                "My keywords",
                Some("You'll be notified whenever someone says one of your keywords."),
                keywords,
                cx,
            ))
            .child(self.group(
                "Notification schedule",
                Some("Your personal schedule overrides workspace default Do Not Disturb hours. Outside of these times, notifications will be paused."),
                v_flex()
                    .gap_3()
                    .child(schedule_on)
                    .child(self.labeled("Allow notifications", days))
                    .children(custom_days_row)
                    .child(h_flex().gap_4().items_start().child(self.labeled("From", start)).child(self.labeled("To", end))),
                cx,
            ))
            .child(self.group(
                "Sound & appearance",
                None,
                v_flex()
                    .gap_3()
                    .child(self.labeled("Notification sound (channels)", sound))
                    .child(self.labeled("Notification sound (DMs & mentions)", dm_sound))
                    .child(preview)
                    .child(mute)
                    .child(dock_badge)
                    .child(badge_unreads)
                    .child(bounce_dock),
                cx,
            ))
    }

    fn navigation(&mut self, cx: &mut Context<Self>) -> Div {
        let order = self.settings.nav_order;
        let rows: Vec<Div> = order
            .iter()
            .enumerate()
            .map(|(pos, &slot)| {
                let (id, label, flag): (&'static str, &'static str, Flag) = match slot {
                    0 => ("nav-home", "Home", |s| &mut s.show_home),
                    1 => ("nav-dms", "DMs", |s| &mut s.show_dms),
                    2 => ("nav-activity", "Activity", |s| &mut s.show_activity),
                    3 => ("nav-files", "Files", |s| &mut s.show_files),
                    _ => ("nav-later", "Later / To-dos", |s| &mut s.show_later),
                };
                let cb = self.checkbox(id, label, flag, cx);
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(cb)
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new(("nav-up", pos))
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .icon(IconName::ChevronUp)
                                    .disabled(pos == 0)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if pos > 0 {
                                            this.settings.nav_order.swap(pos - 1, pos);
                                            this.changed(cx);
                                        }
                                    })),
                            )
                            .child(
                                Button::new(("nav-down", pos))
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .icon(IconName::ChevronDown)
                                    .disabled(pos + 1 >= order.len())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if pos + 1 < this.settings.nav_order.len() {
                                            this.settings.nav_order.swap(pos, pos + 1);
                                            this.changed(cx);
                                        }
                                    })),
                            ),
                    )
            })
            .collect();
        let style = self.radios("nav-style", &TAB_STYLES, |s| &mut s.tab_style, cx);
        v_flex()
            .child(self.group(
                "Show & order navigation tabs",
                Some("Choose which tabs appear in the left rail and reorder them."),
                v_flex().gap_1().children(rows),
                cx,
            ))
            .child(self.group("Tab appearance", None, style, cx))
    }

    fn home(&mut self, cx: &mut Context<Self>) -> Div {
        let show = self.select("home-show", &SHOW, |s| &mut s.show, cx);
        let sort = self.select("home-sort", &SORTS, |s| &mut s.sort, cx);
        let unreads = self.checkbox(
            "home-unreads",
            "Show all unreads in one section",
            |s| &mut s.unreads_section,
            cx,
        );
        let private = self.checkbox(
            "home-private",
            "Separate private channels from public ones",
            |s| &mut s.separate_private,
            cx,
        );
        let photos = self.checkbox(
            "home-photos",
            "Show profile photos next to DMs",
            |s| &mut s.profile_photos,
            cx,
        );
        let compact_rows = self.checkbox(
            "home-compact",
            "Use compact sidebar rows",
            |s| &mut s.sidebar_compact,
            cx,
        );
        let show_muted = self.checkbox(
            "home-show-muted",
            "Show muted conversations in sidebar",
            |s| &mut s.show_muted_channels,
            cx,
        );
        v_flex()
            .child(
                self.group(
                    "Show and sort",
                    Some("Change the default for how conversations show and sort in your sidebar."),
                    v_flex()
                        .gap_3()
                        .child(self.labeled("Show", show))
                        .child(self.labeled("Sort", sort)),
                    cx,
                ),
            )
            .child(
                self.group(
                    "Organization",
                    None,
                    v_flex()
                        .gap_1()
                        .child(unreads)
                        .child(private)
                        .child(photos)
                        .child(compact_rows)
                        .child(show_muted),
                    cx,
                ),
            )
    }

    fn appearance(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (border, ring, is_dark) = (theme.border, theme.ring, theme.mode.is_dark());
        // Slack's Color Mode: three equal buttons in a row, icon beside the label, the chosen
        // one ringed in the accent.
        let mode = self.settings.mode;
        let modes = h_flex().gap_2().children(
            [
                (AppearanceMode::Light, "Light", IconName::Sun),
                (AppearanceMode::Dark, "Dark", IconName::Moon),
                (AppearanceMode::System, "System", IconName::Monitor),
            ]
            .into_iter()
            .enumerate()
            .map(|(ix, (choice, label, icon))| {
                let on = choice == mode;
                h_flex()
                    .id(("appearance-mode", ix))
                    .flex_1()
                    .h(px(38.))
                    .justify_center()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if on { ring } else { border })
                    .when(on, |button| button.shadow_xs())
                    .child(Icon::new(icon).size(px(16.)))
                    .child(div().text_sm().child(label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.mode = choice;
                        this.changed(cx);
                    }))
            }),
        );

        let palette = self.settings.palette;
        let named =
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .children(Palette::NAMED.iter().enumerate().map(|(ix, &choice)| {
                    let (side, accent) = choice.swatch(is_dark);
                    self.swatch_card(
                        ("palette", ix),
                        choice.name(),
                        side,
                        accent,
                        choice == palette,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.palette = choice;
                        this.changed(cx);
                    }))
                }));

        let (custom_sidebar, custom_accent) = match palette {
            Palette::Custom { sidebar, accent } => (Some(sidebar), Some(accent)),
            _ => (None, None),
        };
        let swatch_row = |kind: &'static str,
                          colors: &'static [u32],
                          current: Option<usize>,
                          cx: &mut Context<Self>| {
            h_flex()
                .gap_2()
                .children(colors.iter().enumerate().map(move |(ix, &hex)| {
                    let on = current == Some(ix);
                    div()
                        .id(ElementId::NamedInteger(kind.into(), ix as u64))
                        .size(px(28.))
                        .rounded_md()
                        .cursor_pointer()
                        .bg(theme::swatch_color(hex))
                        .border_2()
                        .border_color(if on { ring } else { border })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let (sidebar, accent) = match this.settings.palette {
                                Palette::Custom { sidebar, accent } => (sidebar, accent),
                                _ => (0, 0),
                            };
                            this.settings.palette = if kind == "custom-sidebar" {
                                Palette::Custom {
                                    sidebar: ix,
                                    accent,
                                }
                            } else {
                                Palette::Custom {
                                    sidebar,
                                    accent: ix,
                                }
                            };
                            this.changed(cx);
                        }))
                }))
        };
        let custom_hex_row = {
            let input_border = cx.theme().input;
            let sb_preview = theme::parse_hex_rgb(&self.custom_sidebar_hex.read(cx).value())
                .map(theme::swatch_color);
            let ac_preview = theme::parse_hex_rgb(&self.custom_accent_hex.read(cx).value())
                .map(theme::swatch_color);
            let hex_box = |state: &Entity<TextareaState>, preview: Option<gpui_kit::gpui::Hsla>| {
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .w(px(120.))
                    .rounded_md()
                    .border_1()
                    .border_color(input_border)
                    .text_xs()
                    .when_some(preview, |row, col| {
                        row.child(
                            div()
                                .flex_none()
                                .size(px(14.))
                                .rounded_xs()
                                .bg(col)
                                .border_1()
                                .border_color(border),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Textarea::new(state).appearance(false).bordered(false)),
                    )
            };
            h_flex()
                .items_center()
                .gap_2()
                .child(hex_box(&self.custom_sidebar_hex, sb_preview))
                .child(hex_box(&self.custom_accent_hex, ac_preview))
                .child(
                    Button::new("custom-hex-apply")
                        .outline()
                        .with_size(Size::Small)
                        .label("Apply hex")
                        .on_click(cx.listener(|this, _, _, cx| {
                            let sb =
                                theme::parse_hex_rgb(&this.custom_sidebar_hex.read(cx).value());
                            let ac = theme::parse_hex_rgb(&this.custom_accent_hex.read(cx).value());
                            if let (Some(sidebar), Some(accent)) = (sb, ac) {
                                this.settings.palette = Palette::CustomHex { sidebar, accent };
                                this.changed(cx);
                            }
                        })),
                )
        };
        let slack_theme_row = {
            let input_border = cx.theme().input;
            v_flex().gap_2().child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .max_w(px(360.))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(input_border)
                            .text_xs()
                            .child(
                                Textarea::new(&self.slack_theme_import)
                                    .appearance(false)
                                    .bordered(false),
                            ),
                    )
                    .child(
                        Button::new("slack-theme-import")
                            .outline()
                            .with_size(Size::Small)
                            .label("Import")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let raw = this.slack_theme_import.read(cx).value().to_string();
                                if let Some((sidebar, accent)) =
                                    theme::parse_slack_theme_string(&raw)
                                {
                                    this.settings.palette = Palette::CustomHex { sidebar, accent };
                                    this.custom_sidebar_hex.update(cx, |s, cx| {
                                        s.set_value(theme::format_hex_rgb(sidebar), window, cx)
                                    });
                                    this.custom_accent_hex.update(cx, |s, cx| {
                                        s.set_value(theme::format_hex_rgb(accent), window, cx)
                                    });
                                    this.changed(cx);
                                    crate::actions::notify(
                                        "Theme imported",
                                        "Applied Slack theme colors.",
                                        window,
                                        cx,
                                    );
                                } else {
                                    crate::actions::notify(
                                        "Invalid theme string",
                                        "Paste comma-separated #RRGGBB hex values from Slack.",
                                        window,
                                        cx,
                                    );
                                }
                            })),
                    )
                    .child(
                        Button::new("slack-theme-copy")
                            .outline()
                            .with_size(Size::Small)
                            .icon(IconName::Copy)
                            .label("Copy theme")
                            .tooltip("Copy Slack-compatible comma-separated hex theme string")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let exported = theme::format_slack_theme_string(
                                    this.settings.palette,
                                    is_dark,
                                );
                                cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(
                                    exported.clone(),
                                ));
                                crate::actions::notify("Slack theme copied", exported, window, cx);
                            })),
                    ),
            )
        };
        let simplified = self.checkbox(
            "appearance-simplified",
            "Simplified layout (hide workspace rail and extra chrome)",
            |s| &mut s.simplified_layout,
            cx,
        );
        let custom = v_flex()
            .gap_3()
            .child(self.labeled(
                "Sidebar",
                swatch_row("custom-sidebar", &CUSTOM_SIDEBARS, custom_sidebar, cx),
            ))
            .child(self.labeled(
                "Accent",
                swatch_row("custom-accent", &CUSTOM_ACCENTS, custom_accent, cx),
            ))
            .child(self.labeled("Custom hex colors (Sidebar / Accent)", custom_hex_row))
            .child(self.labeled(
                "Share or import Slack theme (comma-separated hex)",
                slack_theme_row,
            ))
            .child(simplified);

        let gradient = self.checkbox(
            "appearance-gradient",
            "Window gradient",
            |s| &mut s.window_gradient,
            cx,
        );
        let darker = self.checkbox(
            "appearance-darker-sidebars",
            "Darker sidebars",
            |s| &mut s.darker_sidebars,
            cx,
        );
        let font = self.select("appearance-font", &theme::FONT_LABELS, |s| &mut s.font, cx);
        let skin = self.radios("appearance-skin", &SKIN_TONES, |s| &mut s.skin_tone, cx);
        let jumbo = self.checkbox(
            "appearance-jumbo",
            "Display jumbomoji",
            |s| &mut s.jumbomoji,
            cx,
        );
        let emoticons = self.checkbox(
            "appearance-emoticons",
            "Convert my typed emoticons to emoji, so :D becomes \u{1F603}",
            |s| &mut s.convert_emoticons,
            cx,
        );

        v_flex()
            .child(self.group("Font", None, font, cx))
            .child(self.group("Color Mode", Some("Choose if Slack's appearance should be light or dark, or follow your computer's settings."), modes, cx))
            .child(self.group(
                "Slack themes",
                Some("Single-color themes that tint your sidebar, chrome, and highlights."),
                v_flex().gap_3().child(named).child(h_flex().gap_5().child(gradient).child(darker)),
                cx,
            ))
            .child(self.group("Custom theme", Some("Build your own: pick a sidebar and an accent color."), custom, cx))
            .child(self.group(
                "Emoji",
                None,
                v_flex().gap_2().child(self.labeled("Default skin tone", h_flex().child(skin))).child(jumbo).child(emoticons),
                cx,
            ))
    }

    /// A color-theme card: a mini sidebar + accent preview over the theme's name.
    fn swatch_card(
        &self,
        id: (&'static str, usize),
        name: &'static str,
        sidebar: gpui_kit::gpui::Hsla,
        accent: gpui_kit::gpui::Hsla,
        selected: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let (border, ring, background, muted) =
            (theme.border, theme.ring, theme.background, theme.muted);
        v_flex()
            .id(id)
            .w(px(110.))
            .gap_1()
            .p_1p5()
            .rounded_md()
            .cursor_pointer()
            .border_2()
            .border_color(if selected { ring } else { border })
            .child(
                h_flex()
                    .h(px(44.))
                    .rounded_sm()
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .w(px(36.))
                            .h_full()
                            .bg(sidebar)
                            .p_1()
                            .gap_1()
                            .child(div().h(px(5.)).w_full().rounded_sm().bg(accent)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .bg(background)
                            .p_1()
                            .gap_1()
                            .child(div().h(px(4.)).w(px(40.)).rounded_sm().bg(muted))
                            .child(div().h(px(4.)).w(px(28.)).rounded_sm().bg(muted)),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .child(name),
            )
    }

    fn messages(&mut self, cx: &mut Context<Self>) -> Div {
        let density = self.radios("msg-density", &DENSITY, |s| &mut s.density, cx);
        let names = self.radios("msg-names", &NAMES, |s| &mut s.names, cx);
        let clock24 = self.checkbox(
            "msg-clock-24h",
            "Display times with a 24-hour clock",
            |s| &mut s.clock_24h,
            cx,
        );
        let emoji_text = self.checkbox(
            "msg-emoji-text",
            "Display emoji as plain text (:smile:)",
            |s| &mut s.emoji_text_mode,
            cx,
        );
        let enter = self.radio_rows(
            "msg-enter",
            &ENTER_OPTIONS,
            usize::from(!self.settings.enter_sends),
            |settings, ix| settings.enter_sends = ix == 0,
            cx,
        );
        let markup = self.checkbox(
            "msg-markup",
            "Format messages with markup",
            |s| &mut s.format_markup,
            cx,
        );
        let inline = self.checkbox(
            "msg-inline",
            "Show images and files uploaded to Slack",
            |s| &mut s.inline_media,
            cx,
        );
        let previews = self.checkbox(
            "msg-previews",
            "Show images and files from linked websites",
            |s| &mut s.link_previews,
            cx,
        );
        let large = self.checkbox(
            "msg-large",
            "Even if they're larger than 2 MB",
            |s| &mut s.large_files,
            cx,
        );
        let quick_toggle = self.checkbox(
            "msg-quick-reactions",
            "Show one-click reactions on messages",
            |s| &mut s.show_quick_reactions,
            cx,
        );
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let bg_card = cx.theme().secondary;
        let quick_editors = self.settings.show_quick_reactions.then(|| {
            let mut row = h_flex().flex_wrap().items_center().gap_2().pl_6().pt_1();
            for ix in 0..3 {
                let code = self.settings.quick_reaction_emojis[ix].trim();
                let glyph = crate::emoji::glyph(code)
                    .or_else(|| crate::emoji::glyph(crate::actions::DEFAULT_QUICK_REACTIONS[ix]))
                    .unwrap_or("🙂");
                row = row.child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(bg_card)
                        .border_1()
                        .border_color(border)
                        .child(div().text_sm().child(glyph))
                        .child(
                            div().w(px(124.)).child(
                                Textarea::new(&self.quick_reaction_inputs[ix])
                                    .appearance(false)
                                    .bordered(false),
                            ),
                        ),
                );
            }
            let reset = Button::new("msg-quick-reactions-reset")
                .ghost()
                .with_size(Size::XSmall)
                .label("Reset defaults")
                .on_click(cx.listener(|this, _, window, cx| {
                    let defaults = crate::actions::DEFAULT_QUICK_REACTIONS.map(String::from);
                    this.settings.quick_reaction_emojis = defaults.clone();
                    for (ix, val) in defaults.into_iter().enumerate() {
                        this.quick_reaction_inputs[ix]
                            .update(cx, |state, cx| state.set_value(val, window, cx));
                    }
                    this.changed(cx);
                }));
            v_flex().gap_1p5().child(row).child(
                h_flex()
                    .pl_6()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Enter emoji shortcodes (e.g. white_check_mark, eyes, rocket)."),
                    )
                    .child(reset),
            )
        });
        v_flex()
            .child(self.group("Message theme", None, density, cx))
            .child(
                self.group(
                    "One-click reactions",
                    Some("Quickly react to messages from the hover toolbar."),
                    v_flex()
                        .gap_1p5()
                        .child(quick_toggle)
                        .children(quick_editors),
                    cx,
                ),
            )
            .child(
                self.group(
                    "Names & timestamps",
                    None,
                    v_flex()
                        .gap_2()
                        .child(names)
                        .child(clock24)
                        .child(emoji_text),
                    cx,
                ),
            )
            .child(self.group(
                "When writing a message, press Enter to\u{2026}",
                None,
                v_flex().gap_2().child(enter).child(markup),
                cx,
            ))
            .child(
                self.group(
                    "Inline media & links",
                    None,
                    v_flex()
                        .gap_1()
                        .child(inline)
                        .child(previews)
                        .child(div().pl_6().child(large)),
                    cx,
                ),
            )
    }

    fn language(&mut self, cx: &mut Context<Self>) -> Div {
        let language = self.select("lang", &LANGUAGES, |s| &mut s.language, cx);
        let auto = self.checkbox(
            "tz-auto",
            "Set time zone automatically",
            |s| &mut s.timezone_auto,
            cx,
        );
        let zone = self.select("tz", &ZONES, |s| &mut s.timezone, cx);
        let spell = self.checkbox(
            "spellcheck",
            "Enable spellcheck on your messages",
            |s| &mut s.spellcheck,
            cx,
        );
        v_flex()
            .child(self.group("Language", Some("Choose the language you want to use in Slack."), language, cx))
            .child(self.group("Time zone", Some("Slack uses your time zone to send summary and notification emails, and for times in your activity feeds and reminders."), v_flex().gap_2().child(auto).child(zone), cx))
            .child(self.group("Spellcheck", None, spell, cx))
    }

    fn accessibility(&mut self, cx: &mut Context<Self>) -> Div {
        let zoom = self.select("zoom", &ZOOM, |s| &mut s.zoom, cx);
        let animate = self.checkbox(
            "a11y-animate",
            "Allow animated images and emoji",
            |s| &mut s.animate_emoji,
            cx,
        );
        let underline = self.checkbox(
            "a11y-underline",
            "Underline links to websites",
            |s| &mut s.underline_links,
            cx,
        );
        let motion = self.checkbox("a11y-motion", "Reduce motion", |s| &mut s.reduce_motion, cx);
        let transparency = self.checkbox(
            "a11y-transparency",
            "Translucent window background (macOS vibrancy)",
            |s| &mut s.window_transparency,
            cx,
        );
        let announce = self.checkbox(
            "a11y-announce",
            "Read incoming messages aloud with a screen reader",
            |s| &mut s.announce_incoming,
            cx,
        );
        v_flex()
            .child(self.group(
                "Zoom",
                Some("Use \u{2318}+ / \u{2318}- to zoom anywhere in Slack."),
                zoom,
                cx,
            ))
            .child(
                self.group(
                    "Animation & links",
                    None,
                    v_flex()
                        .gap_1()
                        .child(animate)
                        .child(underline)
                        .child(motion)
                        .child(transparency),
                    cx,
                ),
            )
            .child(self.group("Screen reader", None, announce, cx))
    }

    fn mark_as_read(&mut self, cx: &mut Context<Self>) -> Div {
        let mode = self.radios("mark-read", &MARK_READ, |s| &mut s.mark_read, cx);
        let confirm = self.checkbox(
            "mark-confirm",
            "Prompt to confirm when marking all messages as read (Shift+Esc)",
            |s| &mut s.confirm_mark_all,
            cx,
        );
        let no_bg = self.checkbox(
            "mark-no-bg",
            "Keep messages unread while window is in the background",
            |s| &mut s.no_bg_mark_read,
            cx,
        );
        v_flex()
            .child(self.group("When I view a channel", None, mode, cx))
            .child(self.group(
                "Mark everything read",
                None,
                v_flex().gap_1().child(confirm).child(no_bg),
                cx,
            ))
    }

    fn advanced(&mut self, cx: &mut Context<Self>) -> Div {
        let unsend = self.checkbox(
            "adv-unsend",
            "Ask before deleting a message",
            |s| &mut s.confirm_unsend,
            cx,
        );
        let external = self.checkbox(
            "adv-external",
            "Warn me about potentially malicious links",
            |s| &mut s.warn_external,
            cx,
        );
        let login = self.checkbox(
            "adv-login",
            "Launch slackless at login",
            |s| &mut s.launch_at_login,
            cx,
        );
        let menu_bar = self.checkbox(
            "adv-menu-bar",
            "Show slackless icon in the menu bar",
            |s| &mut s.show_menu_bar_icon,
            cx,
        );
        let close_tray = self.checkbox(
            "adv-close-tray",
            "Keep slackless running in the menu bar when closing the window",
            |s| &mut s.close_to_tray,
            cx,
        );
        let hardware = self.checkbox(
            "adv-hw",
            "Use hardware acceleration (Metal GPU — active on macOS)",
            |s| &mut s.hardware_acceleration,
            cx,
        );
        let debug = self.checkbox(
            "adv-debug",
            "Enable debug logging",
            |s| &mut s.debug_logging,
            cx,
        );
        let links = self.select("adv-links", &OPEN_LINKS, |s| &mut s.open_links, cx);
        let remote_search = self.checkbox(
            "adv-remote-search",
            "Also search Slack remotely by default (slower; local FTS5 stays instant otherwise)",
            |s| &mut s.remote_search_auto,
            cx,
        );
        let search_in_ch = self.checkbox(
            "adv-search-in-channel",
            "\u{2318}F searches the current conversation instead of all of Slack",
            |s| &mut s.search_in_channel,
            cx,
        );
        let excluded_border = cx.theme().input;
        let excluded_input = div()
            .w(px(420.))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(excluded_border)
            .text_sm()
            .child(
                Textarea::new(&self.excluded_channels)
                    .appearance(false)
                    .bordered(false),
            );
        let compact_db = Button::new("adv-compact-db")
            .outline()
            .with_size(Size::Small)
            .label("Compact database")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::CompactStorage)));
        let prune_media = Button::new("adv-prune-media")
            .outline()
            .with_size(Size::Small)
            .label("Prune media cache")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::PruneMediaCache)));
        let reset_cache = Button::new("adv-reset-cache")
            .danger()
            .with_size(Size::Small)
            .label("Clear local cache & reload")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::ResetCache)));
        let storage_summary = {
            let theme = cx.theme();
            let fg = theme.foreground;
            let muted = theme.muted_foreground;
            let border = theme.border;
            let card_bg = theme.secondary.opacity(0.35);
            let body =
                match self.storage_stats.clone() {
                    Some(st) => {
                        let mut metrics = h_flex()
                            .items_center()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .child("SQLite"),
                            )
                            .child(self.mono_chip(format_bytes(st.db_bytes), cx))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .child("Media cache"),
                            )
                            .child(self.mono_chip(format_bytes(st.media_cache_bytes), cx));
                        if st.offline_queued > 0 {
                            metrics = metrics.child(
                                self.mono_chip(format!("{} queued offline", st.offline_queued), cx),
                            );
                        }
                        v_flex().gap_1p5().child(metrics).child(
                            div().text_xs().text_color(muted).child(format!(
                                "{} messages ({} revisions) · {} conversations · {} users",
                                st.messages, st.revisions, st.conversations, st.users,
                            )),
                        )
                    }
                    None => v_flex().child(div().text_xs().text_color(muted).child(
                        "Local SQLite metrics are shown when connected to a live workspace.",
                    )),
                };
            v_flex()
                .gap_3()
                .px_3p5()
                .py_3()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(card_bg)
                .child(body)
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .flex_wrap()
                                .child(compact_db)
                                .child(prune_media),
                        )
                        .child(reset_cache),
                )
        };
        let dl_label = if self.settings.download_dir.is_empty() {
            "~/Downloads (default)".to_owned()
        } else {
            self.settings.download_dir.clone()
        };
        let has_custom_dl = !self.settings.download_dir.is_empty();
        let dl_muted = cx.theme().muted_foreground;
        let dl_fg = cx.theme().foreground;
        let dl_border = cx.theme().border;
        let dl_bg = cx.theme().secondary.opacity(0.35);
        let dl_mono = cx.theme().mono_font_family.clone();
        let download_row = h_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(dl_border)
                    .bg(dl_bg)
                    .font_family(dl_mono)
                    .text_xs()
                    .text_color(if has_custom_dl { dl_fg } else { dl_muted })
                    .child(dl_label),
            )
            .child(
                Button::new("adv-dl-choose")
                    .outline()
                    .with_size(Size::Small)
                    .label("Choose folder\u{2026}")
                    .on_click(cx.listener(|_, _, _, cx| {
                        let rx = cx.prompt_for_paths(PathPromptOptions {
                            files: false,
                            directories: true,
                            multiple: false,
                            prompt: Some("Choose download folder".into()),
                        });
                        cx.spawn(async move |this, cx| {
                            if let Ok(Ok(Some(paths))) = rx.await
                                && let Some(path) = paths.into_iter().next()
                            {
                                let _ = this.update(cx, |prefs, cx| {
                                    prefs.settings.download_dir =
                                        path.to_string_lossy().into_owned();
                                    prefs.changed(cx);
                                });
                            }
                        })
                        .detach();
                    })),
            )
            .when(has_custom_dl, |row| {
                row.child(
                    Button::new("adv-dl-reset")
                        .ghost()
                        .with_size(Size::Small)
                        .label("Reset")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.download_dir.clear();
                            this.changed(cx);
                        })),
                )
            });
        let ask_dl = self.checkbox(
            "adv-dl-ask",
            "Ask where to save each file before downloading",
            |s| &mut s.ask_download_location,
            cx,
        );
        let protocol_input = div()
            .w(px(420.))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(excluded_border)
            .text_sm()
            .child(
                Textarea::new(&self.protocol_allowlist_input)
                    .appearance(false)
                    .bordered(false),
            );
        let network_group = self.network_diagnostics_group(cx);
        v_flex()
            .child(self.group(
                "Search",
                Some("Local search is instant and works offline. Enable remote search when you want older history, unjoined channels, and server-indexed files automatically."),
                v_flex()
                    .gap_2()
                    .child(remote_search)
                    .child(search_in_ch)
                    .child(self.labeled("Exclude these channels from local search", excluded_input)),
                cx,
            ))
            .child(self.group(
                "Download location",
                Some("Where files downloaded from Slack are saved."),
                v_flex().gap_2().child(download_row).child(ask_dl),
                cx,
            ))
            .child(network_group)
            .child(self.group(
                "Other options",
                None,
                v_flex()
                    .gap_2()
                    .child(unsend)
                    .child(external)
                    .child(self.labeled("Allowed URL protocols (comma-separated)", protocol_input)),
                cx,
            ))
            .child(self.group("Open links in", None, links, cx))
            .child(self.group(
                "App",
                None,
                v_flex()
                    .gap_2()
                    .child(login)
                    .child(menu_bar)
                    .child(close_tray)
                    .child(hardware)
                    .child(debug),
                cx,
            ))
            .child(self.software_updates_group(cx))
            .child(self.cli_install_group(cx))
            .child(self.group(
                "Local storage & maintenance",
                Some("Durable local-first SQLite history & FTS5 search index for the active workspace."),
                storage_summary,
                cx,
            ))
    }

    fn mono_chip(&self, text: impl Into<SharedString>, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        div()
            .px_1p5()
            .py(px(1.))
            .rounded_sm()
            .bg(theme.secondary)
            .border_1()
            .border_color(theme.border)
            .font_family(theme.mono_font_family.clone())
            .text_xs()
            .text_color(theme.foreground)
            .child(text.into())
    }

    #[allow(clippy::too_many_arguments)]
    fn status_card(
        &self,
        dot_color: gpui_kit::gpui::Hsla,
        title: impl Into<SharedString>,
        chip: Option<SharedString>,
        detail: impl Into<SharedString>,
        detail_is_err: bool,
        actions: Div,
        cx: &Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let border = theme.border;
        let card_bg = theme.secondary.opacity(0.35);

        let mut header_row = h_flex()
            .items_center()
            .gap_2()
            .flex_wrap()
            .child(div().size(px(8.)).rounded_full().flex_none().bg(dot_color))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if detail_is_err { danger } else { fg })
                    .child(title.into()),
            );
        if let Some(chip_text) = chip {
            header_row = header_row.child(self.mono_chip(chip_text, cx));
        }

        h_flex()
            .items_center()
            .justify_between()
            .gap_3()
            .px_3p5()
            .py_2p5()
            .rounded_md()
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .child(
                v_flex().flex_1().min_w_0().gap_1().child(header_row).child(
                    div()
                        .text_xs()
                        .text_color(if detail_is_err { danger } else { muted })
                        .child(detail.into()),
                ),
            )
            .child(actions.flex_none())
    }

    fn software_updates_group(&mut self, cx: &mut Context<Self>) -> Div {
        use crate::updater::{UpdateState, installed_version, is_dev_build};
        let theme = cx.theme();
        let success = theme.success;
        let primary = theme.primary;
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let current = installed_version();
        let dev = is_dev_build();

        let (dot_color, title, chip, detail, is_err, actions) = match &self.update_state {
            UpdateState::Checking => (
                muted,
                "Checking for updates\u{2026}".to_owned(),
                Some(SharedString::from(current)),
                "Contacting GitHub Releases.".to_owned(),
                false,
                h_flex().gap_1p5().child(
                    Button::new("adv-upd-checking")
                        .outline()
                        .with_size(Size::Small)
                        .disabled(true)
                        .label("Checking\u{2026}"),
                ),
            ),
            UpdateState::Downloading(ver) => (
                primary,
                "Downloading update\u{2026}".to_owned(),
                Some(SharedString::from(ver.clone())),
                "Verifying package and preparing restart.".to_owned(),
                false,
                h_flex().gap_1p5().child(
                    Button::new("adv-upd-downloading")
                        .primary()
                        .with_size(Size::Small)
                        .disabled(true)
                        .label("Downloading\u{2026}"),
                ),
            ),
            UpdateState::Available(rel) => {
                let ver = rel.version.clone();
                let notes_url = rel.html_url.clone();
                (
                    primary,
                    "Update available".to_owned(),
                    Some(SharedString::from(ver)),
                    format!("Currently running {current}."),
                    false,
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            Button::new("adv-upd-notes")
                                .ghost()
                                .with_size(Size::Small)
                                .label("Release notes")
                                .on_click(move |_, _, cx| cx.open_url(&notes_url)),
                        )
                        .child(
                            Button::new("adv-upd-download")
                                .primary()
                                .with_size(Size::Small)
                                .label("Download & restart")
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(PreferencesEvent::InstallUpdate);
                                })),
                        ),
                )
            }
            UpdateState::UpToDate => (
                success,
                "Up to date".to_owned(),
                Some(SharedString::from(current)),
                "You have the latest release of Slackless.".to_owned(),
                false,
                h_flex().gap_1p5().child(
                    Button::new("adv-upd-check-again")
                        .outline()
                        .with_size(Size::Small)
                        .label("Check for updates")
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(PreferencesEvent::CheckForUpdates);
                        })),
                ),
            ),
            UpdateState::Error(err) => (
                danger,
                "Update check failed".to_owned(),
                Some(SharedString::from(current)),
                err.clone(),
                true,
                h_flex().gap_1p5().child(
                    Button::new("adv-upd-retry")
                        .outline()
                        .with_size(Size::Small)
                        .label("Try again")
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(PreferencesEvent::CheckForUpdates);
                        })),
                ),
            ),
            UpdateState::Idle => match &self.available_update {
                Some(rel) => {
                    let ver = rel.version.clone();
                    let notes_url = rel.html_url.clone();
                    (
                        primary,
                        "Update available".to_owned(),
                        Some(SharedString::from(ver)),
                        format!("Currently running {current}."),
                        false,
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Button::new("adv-upd-notes-cached")
                                    .ghost()
                                    .with_size(Size::Small)
                                    .label("Release notes")
                                    .on_click(move |_, _, cx| cx.open_url(&notes_url)),
                            )
                            .child(
                                Button::new("adv-upd-download-cached")
                                    .primary()
                                    .with_size(Size::Small)
                                    .label("Download & restart")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(PreferencesEvent::InstallUpdate);
                                    })),
                            ),
                    )
                }
                None => (
                    if dev { muted } else { success },
                    if dev {
                        "Development build"
                    } else {
                        "Current version"
                    }
                    .to_owned(),
                    Some(SharedString::from(current)),
                    if dev {
                        "Automatic startup checks are paused on dev builds."
                    } else {
                        "Checks automatically on startup."
                    }
                    .to_owned(),
                    false,
                    h_flex().gap_1p5().child(
                        Button::new("adv-upd-check")
                            .outline()
                            .with_size(Size::Small)
                            .label("Check for updates")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(PreferencesEvent::CheckForUpdates);
                            })),
                    ),
                ),
            },
        };

        let card = self.status_card(dot_color, title, chip, detail, is_err, actions, cx);
        let release_notes = match &self.update_state {
            UpdateState::Available(rel) => Some(rel),
            UpdateState::Idle => self.available_update.as_ref(),
            _ => None,
        }
        .filter(|rel| !rel.notes.is_empty())
        .map(|rel| (rel.version.clone(), rel.notes.clone()));

        let body = if let Some((ver, notes)) = release_notes {
            let theme = cx.theme();
            let (border, card_bg, fg, muted) = (
                theme.border,
                theme.secondary.opacity(0.25),
                theme.foreground,
                theme.muted_foreground,
            );
            v_flex().gap_2().child(card).child(
                v_flex()
                    .gap_1()
                    .px_3p5()
                    .py_2p5()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .bg(card_bg)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child(format!("What's new in {ver}")),
                    )
                    .children(notes.into_iter().map(move |item| {
                        h_flex()
                            .items_start()
                            .gap_1p5()
                            .text_xs()
                            .text_color(muted)
                            .child(div().flex_none().child("\u{2022}"))
                            .child(div().flex_1().min_w_0().child(item))
                    })),
            )
        } else {
            v_flex().child(card)
        };
        self.group(
            "Software updates",
            Some("Checks for new releases on startup or when requested; updates both Slackless.app and the CLI together."),
            body,
            cx,
        )
    }

    fn cli_install_group(&mut self, cx: &mut Context<Self>) -> Div {
        use crate::updater::{
            CliInstallStatus, cli_install_status, cli_link_path, dir_in_path, installed_version,
        };
        let status = cli_install_status();
        let target_link = cli_link_path();
        let ver = installed_version();
        let theme = cx.theme();
        let success = theme.success;
        let warning = theme.warning;
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let border = theme.border;
        let card_bg = theme.secondary.opacity(0.35);

        let effective_dir = match &status {
            CliInstallStatus::Installed(p) | CliInstallStatus::OutOfSync(p) => {
                p.parent().map(std::path::Path::to_path_buf)
            }
            _ => target_link.parent().map(std::path::Path::to_path_buf),
        };
        let in_path = effective_dir.as_deref().is_none_or(dir_in_path);

        let (dot_color, title, chip, detail, actions) = match status {
            CliInstallStatus::Installed(path) => (
                success,
                "Installed".to_owned(),
                Some(SharedString::from(path.display().to_string())),
                format!("Version {ver} · stays in sync with app updates"),
                h_flex().gap_1p5().child(
                    Button::new("adv-cli-uninstall")
                        .outline()
                        .with_size(Size::Small)
                        .label("Uninstall")
                        .on_click(cx.listener(|this, _, _, cx| {
                            match crate::updater::uninstall_cli_symlink() {
                                Ok(()) => {
                                    this.cli_status_note =
                                        Some("Removed slackless CLI link.".to_owned());
                                }
                                Err(err) => {
                                    this.cli_status_note = Some(format!("Error: {err}"));
                                }
                            }
                            cx.notify();
                        })),
                ),
            ),
            CliInstallStatus::OutOfSync(path) => (
                warning,
                "Out of sync".to_owned(),
                Some(SharedString::from(path.display().to_string())),
                format!("Points to a different build. Re-link to match Slackless {ver}."),
                h_flex().gap_1p5().child(
                    Button::new("adv-cli-relink")
                        .primary()
                        .with_size(Size::Small)
                        .label("Update link")
                        .on_click(cx.listener(|this, _, _, cx| {
                            match crate::updater::install_cli_symlink() {
                                Ok(p) => {
                                    this.cli_status_note =
                                        Some(format!("Linked {} to this app.", p.display()));
                                }
                                Err(err) => {
                                    this.cli_status_note = Some(format!("Error: {err}"));
                                }
                            }
                            cx.notify();
                        })),
                ),
            ),
            CliInstallStatus::NotInstalled => (
                muted,
                "Not installed".to_owned(),
                Some(SharedString::from(target_link.display().to_string())),
                format!(
                    "Links the bundled CLI (version {ver}) so it stays in sync with app updates."
                ),
                h_flex().gap_1p5().child(
                    Button::new("adv-cli-install")
                        .primary()
                        .with_size(Size::Small)
                        .label("Install CLI")
                        .on_click(cx.listener(|this, _, _, cx| {
                            match crate::updater::install_cli_symlink() {
                                Ok(p) => {
                                    this.cli_status_note =
                                        Some(format!("Installed CLI at {}.", p.display()));
                                }
                                Err(err) => {
                                    this.cli_status_note = Some(format!("Error: {err}"));
                                }
                            }
                            cx.notify();
                        })),
                ),
            ),
            CliInstallStatus::Unavailable => (
                muted,
                "Unavailable".to_owned(),
                None,
                "Launch from an installed Slackless.app bundle to install the matching CLI."
                    .to_owned(),
                h_flex(),
            ),
        };

        let card = self.status_card(dot_color, title, chip, detail, false, actions, cx);
        let mut body = v_flex().gap_2().child(card);

        if !in_path && let Some(dir) = effective_dir {
            let export_cmd = format!("export PATH=\"{}:$PATH\"", dir.display());
            let copy_cmd = export_cmd.clone();
            body = body.child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .bg(card_bg)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .flex_wrap()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("Add to your shell profile:"),
                            )
                            .child(self.mono_chip(export_cmd, cx)),
                    )
                    .child(
                        Button::new("adv-cli-copy-path")
                            .ghost()
                            .with_size(Size::Small)
                            .icon(IconName::Copy)
                            .label("Copy")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(
                                    copy_cmd.clone(),
                                ));
                                this.cli_status_note =
                                    Some("Copied PATH export command.".to_owned());
                                cx.notify();
                            })),
                    ),
            );
        }
        if let Some(note) = &self.cli_status_note {
            let note_color = if note.starts_with("Error:") {
                danger
            } else {
                muted
            };
            body = body.child(div().text_xs().text_color(note_color).child(note.clone()));
        }

        self.group(
            "Command-line interface",
            Some("Use the slackless CLI from your terminal with your local archive and session."),
            body,
            cx,
        )
    }

    fn network_diagnostics_group(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let dl_border = theme.input;
        let mono = theme.mono_font_family.clone();
        let dot_color = if self.settings.net_log_enabled {
            theme.success
        } else {
            theme.muted_foreground
        };
        let proxy_field = div()
            .w(px(420.))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(dl_border)
            .font_family(mono)
            .text_xs()
            .child(
                Textarea::new(&self.proxy_input)
                    .appearance(false)
                    .bordered(false),
            );
        let net_log_toggle = self.checkbox(
            "adv-net-log",
            "Capture HTTP net-log diagnostics (credentials redacted; last 500 requests)",
            |s| &mut s.net_log_enabled,
            cx,
        );
        let net_log_count = slackless_core::api::net_log_snapshot().len();
        let net_log_card = self.status_card(
            dot_color,
            if self.settings.net_log_enabled {
                "Capturing requests"
            } else {
                "Capture paused"
            },
            Some(SharedString::from(format!(
                "{net_log_count} {}",
                if net_log_count == 1 {
                    "request"
                } else {
                    "requests"
                }
            ))),
            "Credentials and tokens are redacted before recording.",
            false,
            h_flex()
                .items_center()
                .gap_1p5()
                .child(
                    Button::new("adv-net-log-clear")
                        .ghost()
                        .with_size(Size::Small)
                        .disabled(net_log_count == 0)
                        .label("Clear")
                        .on_click(cx.listener(|_, _, _, cx| {
                            slackless_core::api::clear_net_log();
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("adv-net-log-export")
                        .outline()
                        .with_size(Size::Small)
                        .disabled(net_log_count == 0)
                        .icon(IconName::Download)
                        .label("Export")
                        .on_click(
                            cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::ExportNetLog)),
                        ),
                ),
            cx,
        );
        self.group(
            "Network & diagnostics",
            Some("Optional HTTP/HTTPS/SOCKS proxy (leave empty to use system environment variables) and redacted HTTP net-log capture."),
            v_flex()
                .gap_2p5()
                .child(self.labeled("Proxy URL (applies to new connections)", proxy_field))
                .child(net_log_toggle)
                .child(net_log_card),
            cx,
        )
    }

    fn device_choice_button(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        selected: bool,
    ) -> Button {
        let btn = Button::new(id).with_size(Size::Small).label(label);
        if selected {
            btn.primary().icon(IconName::Check)
        } else {
            btn.outline()
        }
    }

    fn audio_video(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (border, muted, success, card_bg) = (
            theme.border,
            theme.muted_foreground,
            theme.success,
            theme.secondary.opacity(0.35),
        );
        let active_in = self.settings.huddle_input_device.clone();
        let active_out = self.settings.huddle_output_device.clone();
        let meter_pct = (self.mic_level * 3.5).clamp(0.0, 1.0);
        let mic_active = self.mic_probe.is_some();

        let mic_picker = v_flex()
            .gap_2p5()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child(
                        Self::device_choice_button(
                            "prefs-mic-default",
                            "System default",
                            active_in.is_none(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.huddle_input_device = None;
                            this.refresh_audio_devices_and_probe(cx);
                            this.changed(cx);
                        })),
                    )
                    .children(
                        self.audio_devices
                            .inputs
                            .iter()
                            .enumerate()
                            .map(|(ix, dev)| {
                                let selected = active_in.as_deref() == Some(dev.name.as_str());
                                let name = dev.name.clone();
                                Self::device_choice_button(
                                    ("prefs-mic", ix),
                                    dev.name.clone(),
                                    selected,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.settings.huddle_input_device = Some(name.clone());
                                        this.refresh_audio_devices_and_probe(cx);
                                        this.changed(cx);
                                    },
                                ))
                            }),
                    ),
            )
            .when(
                self.audio_loading && self.audio_devices.inputs.is_empty(),
                |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Detecting audio devices…"),
                    )
                },
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .bg(card_bg)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().size(px(7.)).rounded_full().bg(if mic_active {
                                success
                            } else {
                                muted
                            }))
                            .child(div().text_xs().text_color(muted).child("Input level")),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(6.))
                            .rounded_full()
                            .bg(border.opacity(0.6))
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui_kit::gpui::relative(meter_pct))
                                    .rounded_full()
                                    .bg(success),
                            ),
                    ),
            );

        let speaker_picker =
            v_flex()
                .gap_2p5()
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1p5()
                        .child(
                            Self::device_choice_button(
                                "prefs-spk-default",
                                "System default",
                                active_out.is_none(),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings.huddle_output_device = None;
                                this.changed(cx);
                            })),
                        )
                        .children(self.audio_devices.outputs.iter().enumerate().map(
                            |(ix, dev)| {
                                let selected = active_out.as_deref() == Some(dev.name.as_str());
                                let name = dev.name.clone();
                                Self::device_choice_button(
                                    ("prefs-spk", ix),
                                    dev.name.clone(),
                                    selected,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.settings.huddle_output_device = Some(name.clone());
                                        this.changed(cx);
                                    },
                                ))
                            },
                        )),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("prefs-test-speaker")
                                .outline()
                                .with_size(Size::Small)
                                .icon(IconName::Play)
                                .label("Test speaker")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.emit(PreferencesEvent::TestSpeaker(
                                        this.settings.huddle_output_device.clone(),
                                    ));
                                })),
                        )
                        .child(
                            Button::new("prefs-refresh-audio")
                                .ghost()
                                .with_size(Size::Small)
                                .icon(IconName::RefreshCw)
                                .label("Refresh devices")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.refresh_audio_devices_and_probe(cx);
                                    cx.notify();
                                })),
                        ),
                );

        let dsp = v_flex()
            .gap_1p5()
            .child(self.checkbox(
                "prefs-huddle-aec",
                "Echo cancellation (WebRTC AEC3)",
                |s| &mut s.huddle_aec,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-ns",
                "Background noise suppression",
                |s| &mut s.huddle_ns,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-agc",
                "Automatic microphone gain control",
                |s| &mut s.huddle_agc,
                cx,
            ));

        let behavior = v_flex()
            .gap_1p5()
            .child(self.checkbox(
                "prefs-huddle-join-muted",
                "Mute my microphone when joining a huddle",
                |s| &mut s.huddle_join_muted,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-leave-lock",
                "Automatically leave a huddle when my computer locks",
                |s| &mut s.huddle_leave_on_lock,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-auto-popout",
                "Automatically open the Huddle window when joining",
                |s| &mut s.huddle_auto_open_window,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-sounds",
                "Play join, leave, and incoming huddle ring chimes",
                |s| &mut s.huddle_sounds,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-reactions",
                "Show floating emoji reaction animations in huddles",
                |s| &mut s.huddle_reaction_animations,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-captions",
                "Automatically enable live captions when joining",
                |s| &mut s.huddle_auto_captions,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-music",
                "Play ambient music while waiting alone in a huddle",
                |s| &mut s.huddle_music_when_alone,
                cx,
            ));

        let camera_group = self.audio_video_camera(cx);

        v_flex()
            .child(camera_group)
            .child(self.group(
                "Microphone",
                Some("Choose which microphone Slackless uses for native huddles."),
                mic_picker,
                cx,
            ))
            .child(self.group(
                "Speaker",
                Some("Choose where huddle audio plays."),
                speaker_picker,
                cx,
            ))
            .child(self.group(
                "Audio processing",
                Some("Pure-Rust real-time DSP applied to captured microphone audio."),
                dsp,
                cx,
            ))
            .child(self.group("When joining a huddle", None, behavior, cx))
    }

    fn audio_video_camera(&mut self, cx: &mut Context<Self>) -> Div {
        use gpui_kit::gpui::{ObjectFit, StyledImage as _, img};

        let theme = cx.theme();
        let (border, muted, card_bg) = (theme.border, theme.muted_foreground, theme.muted);
        let active_cam = self.settings.huddle_camera_device;
        let preview_on = self.camera_preview_on;

        let picker = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child(
                        Self::device_choice_button(
                            "prefs-cam-default",
                            "System default",
                            active_cam.is_none(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.huddle_camera_device = None;
                            if this.camera_preview_on {
                                this.start_camera_preview(cx);
                            }
                            this.changed(cx);
                        })),
                    )
                    .children(self.cameras.iter().enumerate().map(|(ix, cam)| {
                        let selected = active_cam == Some(cam.index);
                        let cam_idx = cam.index;
                        Self::device_choice_button(("prefs-cam", ix), cam.name.clone(), selected)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.settings.huddle_camera_device = Some(cam_idx);
                                if this.camera_preview_on {
                                    this.start_camera_preview(cx);
                                }
                                this.changed(cx);
                            }))
                    })),
            )
            .child(self.checkbox(
                "prefs-huddle-blur",
                "Blur my background by default when turning on camera",
                |s| &mut s.huddle_blur_background,
                cx,
            ))
            .child(
                h_flex().gap_2().child(
                    Button::new("prefs-cam-preview-toggle")
                        .outline()
                        .with_size(Size::Small)
                        .icon(if preview_on {
                            IconName::VideoOff
                        } else {
                            IconName::Video
                        })
                        .label(if preview_on {
                            "Stop camera preview"
                        } else {
                            "Preview camera"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.camera_preview_on {
                                this.stop_camera_preview(cx);
                                cx.notify();
                            } else {
                                this.start_camera_preview(cx);
                            }
                        })),
                ),
            )
            .when(preview_on, |el| {
                let preview = div()
                    .w(px(280.))
                    .h(px(158.))
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(card_bg)
                    .border_1()
                    .border_color(border)
                    .flex()
                    .items_center()
                    .justify_center();
                el.child(if let Some(frame) = self.camera_frame.clone() {
                    preview.child(img(frame).w_full().h_full().object_fit(ObjectFit::Cover))
                } else {
                    preview.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Starting camera preview…"),
                    )
                })
            });

        self.group(
            "Camera",
            Some("Choose your webcam and optional portrait background blur for native huddles."),
            picker,
            cx,
        )
    }

    pub(super) fn render_body(&mut self, cx: &mut Context<Self>) -> Div {
        match self.section {
            Section::Notifications => self.notifications(cx),
            Section::Navigation => self.navigation(cx),
            Section::Home => self.home(cx),
            Section::Appearance => self.appearance(cx),
            Section::Messages => self.messages(cx),
            Section::Language => self.language(cx),
            Section::AudioVideo => self.audio_video(cx),
            Section::Accessibility => self.accessibility(cx),
            Section::MarkAsRead => self.mark_as_read(cx),
            Section::Advanced => self.advanced(cx),
        }
    }
}
