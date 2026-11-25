//! Preferences > Notifications section view.

use super::super::*;

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
}
