//! Preferences > Notifications section view.

use super::*;

impl Preferences {
    pub(super) fn notifications(&mut self, cx: &mut Context<Self>) -> Div {
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
        let m = self.metrics();
        let keywords = text_input_box_for(&self.keywords, true, false, m, cx)
            .w_full()
            .max_w(px(420.));
        let schedule_on = self.checkbox(
            "schedule-on",
            "Only send notifications during these hours",
            |s| &mut s.schedule_enabled,
            cx,
        );
        let days = self.select_w(
            "schedule-days",
            220.,
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
        let start = self.select_w(
            "schedule-start",
            148.,
            &HOURS,
            |s| &mut s.schedule_start,
            cx,
        );
        let end = self.select_w("schedule-end", 148., &HOURS, |s| &mut s.schedule_end, cx);
        let sound = self.select_w("notify-sound", 220., &SOUNDS, |s| &mut s.sound, cx);
        let dm_sound = self.select_w("notify-dm-sound", 220., &SOUNDS, |s| &mut s.dm_sound, cx);
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
        let vip_box = text_input_box_for(&self.vip_input, true, false, m, cx)
            .w_full()
            .max_w(px(420.));
        let urgent_override = self.checkbox(
            "notify-urgent-dm-override",
            "Allow urgent direct messages to break through Do Not Disturb & Focus mode",
            |s| &mut s.urgent_dm_override,
            cx,
        );
        let vip_sound_sel =
            self.select_w("notify-vip-sound", 220., &SOUNDS, |s| &mut s.vip_sound, cx);
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
        let catchup_reminder_group = self.catchup_reminder_group(m, cx);
        let smart_rules_group = self.smart_rules_group(m, cx);
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
            .child(smart_rules_group)
            .child(catchup_reminder_group)
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

    fn catchup_reminder_group(&mut self, m: ControlMetrics, cx: &mut Context<Self>) -> Div {
        let enabled_cb = self.checkbox(
            "catchup-reminder-enabled",
            "Send a daily Catch-Up briefing reminder",
            |s| &mut s.catchup_reminder_enabled,
            cx,
        );
        let time_box = text_input_box_for(&self.catchup_time_input, true, false, m, cx).w(px(120.));
        let time_raw = self.settings.catchup_reminder_time.clone();
        let valid_time = slackless_core::rules::parse_daily_time_hhmm(&time_raw);
        let theme = cx.theme();
        let (muted, danger, border, accent, fg) = (
            theme.muted_foreground,
            theme.danger,
            theme.border,
            theme.primary,
            crate::theme::avatar_foreground(),
        );
        let status_note = if let Some((h, min)) = valid_time {
            let formatted = format!("{h:02}:{min:02}");
            let detail = if self.settings.catchup_reminder_enabled {
                format!(
                    "Fires at most once per day at {formatted} local time. Clicking the notification opens Catch-Up."
                )
            } else {
                format!(
                    "Scheduled time ready ({formatted}). Enable above to receive daily reminders."
                )
            };
            div().text_xs().text_color(muted).child(detail)
        } else {
            div()
                .text_xs()
                .text_color(danger)
                .child("Enter a valid 24-hour time in HH:MM format (for example, 09:00).")
        };
        const PRESETS: [&str; 4] = ["08:30", "09:00", "10:00", "17:00"];
        let presets = h_flex()
            .flex_wrap()
            .gap_1p5()
            .children(PRESETS.iter().enumerate().map(|(ix, &preset)| {
                let active = time_raw.trim() == preset;
                div()
                    .id(("catchup-time-preset", ix))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .text_xs()
                    .cursor_pointer()
                    .flex_none()
                    .when(active, |d| {
                        d.bg(accent)
                            .border_color(accent)
                            .text_color(fg)
                            .font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!active, |d| d.border_color(border).text_color(muted))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.settings.catchup_reminder_time = preset.to_owned();
                        this.catchup_time_input
                            .update(cx, |s, cx| s.set_value(preset.to_owned(), window, cx));
                        this.changed(cx);
                    }))
                    .child(preset)
            }));
        self.group(
            "Daily Catch-Up reminder",
            Some("Get a gentle daily nudge to review unread channels, mentions, and threads across your workspaces."),
            v_flex()
                .gap_2()
                .child(enabled_cb)
                .child(
                    h_flex()
                        .items_center()
                        .flex_wrap()
                        .gap_3()
                        .child(self.labeled("Reminder time (24h HH:MM)", time_box))
                        .child(v_flex().gap_1().child(div().text_xs().text_color(muted).child("Presets")).child(presets)),
                )
                .child(status_note),
            cx,
        )
    }

    fn smart_rules_group(&mut self, m: ControlMetrics, cx: &mut Context<Self>) -> Div {
        use slackless_core::rules::{RuleAction, RuleContext, test_rule, validate_regex};

        let theme = cx.theme();
        let (fg, muted, border, card_bg, accent, danger, fill, fill_fg, input_border) = (
            theme.foreground,
            theme.muted_foreground,
            theme.border,
            theme.secondary.opacity(0.35),
            theme.primary,
            theme.danger,
            theme.sidebar_accent,
            theme.sidebar_accent_foreground,
            theme.input,
        );
        let badge_fg = crate::theme::avatar_foreground();
        let rules = self.settings.smart_notification_rules.clone();

        let mut list = v_flex().gap_2();
        if rules.is_empty() {
            list = list.child(
                div()
                    .px_3()
                    .py_2p5()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .bg(card_bg)
                    .text_xs()
                    .text_color(muted)
                    .child("No smart rules yet. Create a rule to break through muted channels, highlight priority messages, or suppress noisy bots."),
            );
        } else {
            for (ix, rule) in rules.iter().enumerate() {
                let rule_id_toggle = rule.id.clone();
                let rule_id_edit = rule.id.clone();
                let rule_id_delete = rule.id.clone();
                let enabled = rule.enabled;
                let summary = rule.condition_summary();
                let action_label = rule.action.label();
                let (pill_bg, pill_fg) = match rule.action {
                    RuleAction::BreakThroughMute => (accent, badge_fg),
                    RuleAction::PriorityHighlight => (fill, fill_fg),
                    RuleAction::Suppress => (theme.secondary, muted),
                };
                let row = h_flex()
                    .id(("smart-rule-row", ix))
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
                        div()
                            .id(("smart-rule-toggle", ix))
                            .size(px(16.))
                            .flex_none()
                            .rounded(px(3.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .border_1()
                            .cursor_pointer()
                            .when(enabled, |b| b.bg(fill).border_color(fill))
                            .when(!enabled, |b| b.border_color(input_border))
                            .when(enabled, |b| {
                                b.child(
                                    Icon::new(IconName::Check).text_color(fill_fg).size(px(12.)),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_rule_enabled(&rule_id_toggle, cx);
                            })),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(if enabled { fg } else { muted })
                                            .child(rule.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .px_1p5()
                                            .py(px(1.))
                                            .rounded_sm()
                                            .bg(pill_bg)
                                            .text_xs()
                                            .text_color(pill_fg)
                                            .child(action_label),
                                    ),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(summary),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .child(
                                self.btn(
                                    Button::new(("smart-rule-edit", ix))
                                        .ghost()
                                        .small()
                                        .label("Edit"),
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        let existing = this
                                            .settings
                                            .smart_notification_rules
                                            .iter()
                                            .find(|r| r.id == rule_id_edit)
                                            .cloned();
                                        this.rule_editor.load_rule(existing.as_ref(), window, cx);
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                self.btn(
                                    Button::new(("smart-rule-delete", ix))
                                        .ghost()
                                        .small()
                                        .label("Delete"),
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.delete_rule(&rule_id_delete, cx);
                                    },
                                )),
                            ),
                    );
                list = list.child(row);
            }
        }

        let add_btn = h_flex().child(
            self.btn(
                Button::new("smart-rule-add")
                    .outline()
                    .label(if self.rule_editor.open {
                        "New rule"
                    } else {
                        "Add rule"
                    }),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.rule_editor.load_rule(None, window, cx);
                cx.notify();
            })),
        );

        let editor_form =
            self.rule_editor.open.then(|| {
                let is_editing = self.rule_editor.editing_id.is_some();
                let draft = self.rule_editor.draft_rule("preview".to_owned(), true, cx);
                let regex_err = draft
                    .regex_pattern
                    .as_deref()
                    .and_then(|p| validate_regex(p).err());
                let can_save = draft.has_any_condition() && regex_err.is_none();

                let sample_sender_raw = self.rule_editor.test_sender.read(cx).value().to_string();
                let sample_sender = sample_sender_raw.trim();
                let sample_handle = sample_sender.trim_start_matches('@');
                let sample_channel_raw = self.rule_editor.test_channel.read(cx).value().to_string();
                let sample_channel = sample_channel_raw.trim();
                let sample_channel_clean = sample_channel.trim_start_matches('#');
                let sample_text = self.rule_editor.test_message.read(cx).value().to_string();
                let sample_ctx = RuleContext {
                    sender_id: (!sample_handle.is_empty()).then_some(sample_handle),
                    sender_handle: (!sample_handle.is_empty()).then_some(sample_handle),
                    sender_display_name: (!sample_handle.is_empty()).then_some(sample_handle),
                    channel_id: sample_channel_clean,
                    channel_name: (!sample_channel_clean.is_empty())
                        .then_some(sample_channel_clean),
                    text: &sample_text,
                    is_thread_reply: self.rule_editor.test_is_thread,
                    mentions_me: self.rule_editor.test_mentions_me,
                };
                let test_preview = test_rule(&draft, &sample_ctx);

                let action_chips = h_flex().flex_wrap().gap_1p5().children(
                    RuleAction::ALL.iter().enumerate().map(|(ix, &act)| {
                        let active = self.rule_editor.action == act;
                        div()
                            .id(("rule-action-chip", ix))
                            .px_2p5()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .text_xs()
                            .cursor_pointer()
                            .flex_none()
                            .when(active, |d| {
                                d.bg(accent)
                                    .border_color(accent)
                                    .text_color(badge_fg)
                                    .font_weight(FontWeight::SEMIBOLD)
                            })
                            .when(!active, |d| d.border_color(border).text_color(muted))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.rule_editor.action = act;
                                cx.notify();
                            }))
                            .child(act.label())
                    }),
                );

                let threshold_chips = h_flex().flex_wrap().gap_1p5().children(
                    RULE_SEMANTIC_THRESHOLDS
                        .iter()
                        .enumerate()
                        .map(|(ix, &(_, label))| {
                            let active = self.rule_editor.semantic_threshold_idx == ix;
                            div()
                                .id(("rule-sem-thresh", ix))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .text_xs()
                                .cursor_pointer()
                                .flex_none()
                                .when(active, |d| {
                                    d.bg(accent)
                                        .border_color(accent)
                                        .text_color(badge_fg)
                                        .font_weight(FontWeight::SEMIBOLD)
                                })
                                .when(!active, |d| d.border_color(border).text_color(muted))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.rule_editor.semantic_threshold_idx = ix;
                                    cx.notify();
                                }))
                                .child(label)
                        }),
                );

                let only_threads = self.rule_editor.only_threads;
                let mentions_me = self.rule_editor.mentions_me;
                let test_is_thread = self.rule_editor.test_is_thread;
                let test_mentions_me = self.rule_editor.test_mentions_me;

                let flag_toggle = |id: &'static str, label: &'static str, active: bool| {
                    h_flex()
                        .id(id)
                        .items_center()
                        .gap_2()
                        .py_0p5()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(fg)
                        .child(
                            div()
                                .size(px(14.))
                                .flex_none()
                                .rounded(px(3.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .border_1()
                                .when(active, |b| b.bg(fill).border_color(fill))
                                .when(!active, |b| b.border_color(input_border))
                                .when(active, |b| {
                                    b.child(
                                        Icon::new(IconName::Check)
                                            .text_color(fill_fg)
                                            .size(px(10.)),
                                    )
                                }),
                        )
                        .child(label)
                };

                let preview_color = if test_preview.regex_error.is_some() {
                    danger
                } else if test_preview.matched {
                    accent
                } else {
                    muted
                };

                v_flex()
                .gap_3()
                .p_3p5()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(card_bg)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .child(if is_editing {
                            "Edit smart rule"
                        } else {
                            "New smart rule"
                        }),
                )
                .child(self.labeled(
                    "Rule name",
                    text_input_box_for(&self.rule_editor.name, true, false, m, cx)
                        .w_full()
                        .max_w(px(420.)),
                ))
                .child(self.labeled("Action", action_chips))
                .child(self.labeled(
                    "From users (IDs, @handles, or names)",
                    text_input_box_for(&self.rule_editor.from_users, true, false, m, cx)
                        .w_full()
                        .max_w(px(420.)),
                ))
                .child(self.labeled(
                    "In channels (IDs or #names)",
                    text_input_box_for(&self.rule_editor.in_channels, true, false, m, cx)
                        .w_full()
                        .max_w(px(420.)),
                ))
                .child(self.labeled(
                    "Keywords (any matches, case-insensitive)",
                    text_input_box_for(&self.rule_editor.keywords, true, false, m, cx)
                        .w_full()
                        .max_w(px(420.)),
                ))
                .child(
                    v_flex()
                        .gap_1()
                        .child(self.labeled(
                            "Regex pattern (optional, case-insensitive)",
                            text_input_box_for(
                                &self.rule_editor.regex_pattern,
                                true,
                                true,
                                m,
                                cx,
                            )
                            .w_full()
                            .max_w(px(420.)),
                        ))
                        .children(regex_err.map(|err| {
                            div()
                                .text_xs()
                                .text_color(danger)
                                .child(format!("Invalid regex: {err}"))
                        })),
                )
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_4()
                        .child(
                            flag_toggle(
                                "rule-flag-threads",
                                "Only match thread replies",
                                only_threads,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.rule_editor.only_threads = !this.rule_editor.only_threads;
                                cx.notify();
                            })),
                        )
                        .child(
                            flag_toggle(
                                "rule-flag-mentions",
                                "Only when I'm mentioned",
                                mentions_me,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.rule_editor.mentions_me = !this.rule_editor.mentions_me;
                                cx.notify();
                            })),
                        ),
                )
                .child(
                    v_flex()
                        .gap_1p5()
                        .child(self.labeled(
                            "Semantic topic (optional, uses local embeddings when installed)",
                            text_input_box_for(&self.rule_editor.semantic_topic, true, false, m, cx)
                                .w_full()
                                .max_w(px(420.)),
                        ))
                        .child(threshold_chips),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .pt_2()
                        .border_t_1()
                        .border_color(border)
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg)
                                .child("Test rule preview"),
                        )
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    text_input_box_for(
                                        &self.rule_editor.test_sender,
                                        true,
                                        false,
                                        m,
                                        cx,
                                    )
                                    .w(px(160.)),
                                )
                                .child(
                                    text_input_box_for(
                                        &self.rule_editor.test_channel,
                                        true,
                                        false,
                                        m,
                                        cx,
                                    )
                                    .w(px(160.)),
                                ),
                        )
                        .child(
                            text_input_box_for(&self.rule_editor.test_message, true, false, m, cx)
                                .w_full()
                                .max_w(px(420.)),
                        )
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_4()
                                .child(
                                    flag_toggle(
                                        "rule-test-thread",
                                        "Sample is thread reply",
                                        test_is_thread,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.rule_editor.test_is_thread =
                                            !this.rule_editor.test_is_thread;
                                        cx.notify();
                                    })),
                                )
                                .child(
                                    flag_toggle(
                                        "rule-test-mention",
                                        "Sample mentions me",
                                        test_mentions_me,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.rule_editor.test_mentions_me =
                                            !this.rule_editor.test_mentions_me;
                                        cx.notify();
                                    })),
                                ),
                        )
                        .child(
                            div()
                                .px_2p5()
                                .py_1p5()
                                .rounded_sm()
                                .border_1()
                                .border_color(preview_color)
                                .text_xs()
                                .text_color(preview_color)
                                .child(test_preview.summary),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            self.btn(Button::new("smart-rule-save").primary().label("Save rule"))
                                .disabled(!can_save)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.save_rule_draft(cx);
                                })),
                        )
                        .child(
                            self.btn(Button::new("smart-rule-cancel").outline().label("Cancel"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.rule_editor.open = false;
                                    this.rule_editor.editing_id = None;
                                    cx.notify();
                                })),
                        ),
                )
            });

        self.group(
            "Smart notification rules",
            Some("Local rules evaluated in real time. Precedence: Break through mute > Suppress > Priority highlight."),
            v_flex()
                .gap_3()
                .child(list)
                .child(add_btn)
                .children(editor_form),
            cx,
        )
    }
}
