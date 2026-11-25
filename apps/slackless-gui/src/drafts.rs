//! Drafts & sent (docs/research/api-messaging.md, drafts + `chat.scheduleMessage`): an
//! in-memory store of per-conversation composer drafts, scheduled messages and messages sent
//! this session, and the view that replaces the message column while it is open (tabs
//! Drafts / Scheduled / Sent, like Slack's "Drafts & sent").
//!
//! The root saves the composer's text here whenever the open conversation (or workspace)
//! changes and loads it back on return. Scheduled messages are not delivered by a timer in
//! fixture mode; "Send now" delivers one immediately and "Cancel" drops it.
//!
//! `DraftsView::new(&mut App) -> Entity<DraftsView>`
//! emits `DraftsEvent::Open { workspace, channel }` / `DraftsEvent::SendNow(Entry)` /
//! `DraftsEvent::Close`

use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};
use slackless_core::types::ChannelId;

const DAY: u64 = 86_400;
const HOUR: u64 = 3_600;

/// Unix seconds now.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// The composer's "Schedule for later" presets (times are UTC in fixture mode).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SendLater {
    InThirtyMinutes,
    InOneHour,
    TomorrowMorning,
    NextMonday,
}

impl SendLater {
    pub const ALL: [SendLater; 4] = [
        SendLater::InThirtyMinutes,
        SendLater::InOneHour,
        SendLater::TomorrowMorning,
        SendLater::NextMonday,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SendLater::InThirtyMinutes => "In 30 minutes",
            SendLater::InOneHour => "In 1 hour",
            SendLater::TomorrowMorning => "Tomorrow at 9:00",
            SendLater::NextMonday => "Monday at 9:00",
        }
    }

    /// Delivery time in unix seconds, counted from `now` using the user's local timezone.
    pub fn at(self, now: u64) -> u64 {
        self.at_with_offset(now, crate::messages::local_offset_now())
    }

    /// Pure local-timezone schedule calculation with an explicit UTC offset in seconds.
    pub fn at_with_offset(self, now: u64, offset_secs: i64) -> u64 {
        let local_now = now as i64 + offset_secs;
        let today = local_now.div_euclid(DAY as i64);
        let local_target = match self {
            SendLater::InThirtyMinutes => return now + HOUR / 2,
            SendLater::InOneHour => return now + HOUR,
            SendLater::TomorrowMorning => (today + 1) * DAY as i64 + 9 * HOUR as i64,
            SendLater::NextMonday => {
                // Day 0 (1970-01-01) was a Thursday; Monday = 0.
                let weekday = (today + 3).rem_euclid(7);
                (today + 7 - weekday) * DAY as i64 + 9 * HOUR as i64
            }
        };
        (local_target - offset_secs).max(0) as u64
    }
}

/// "Today 14:05" / "Tomorrow 09:00" / "Mon 09:00" / "Yesterday 08:12" in the user's local timezone.
pub fn when_label(at: u64, now: u64) -> String {
    when_label_with_offset(at, now, crate::messages::local_offset_now())
}

/// Pure local-timezone label formatter with an explicit UTC offset in seconds.
pub fn when_label_with_offset(at: u64, now: u64, offset_secs: i64) -> String {
    const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let local_at = at as i64 + offset_secs;
    let local_now = now as i64 + offset_secs;
    let day = local_at.div_euclid(DAY as i64);
    let today = local_now.div_euclid(DAY as i64);
    let secs_in_day = local_at.rem_euclid(DAY as i64) as u64;
    let clock = format!("{:02}:{:02}", secs_in_day / HOUR, (secs_in_day % HOUR) / 60);
    let date = match day - today {
        0 => "Today".to_owned(),
        1 => "Tomorrow".to_owned(),
        -1 => "Yesterday".to_owned(),
        _ => WEEKDAYS[(day + 3).rem_euclid(7) as usize].to_owned(),
    };
    format!("{date} {clock}")
}

/// One draft, scheduled or sent message.
#[derive(Clone)]
pub struct Entry {
    /// Slack's `scheduled_message_id` for a live scheduled message (`None` for local entries).
    pub id: Option<String>,
    /// Index into the workspace list the conversation belongs to.
    pub workspace: usize,
    pub channel: ChannelId,
    /// Conversation label at the time of writing (`# name` / peer names).
    pub label: String,
    pub text: String,
    /// Unix seconds: last edit, delivery time or send time.
    pub at: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DraftsTab {
    Drafts,
    Scheduled,
    Sent,
}

pub enum DraftsEvent {
    Open {
        workspace: usize,
        channel: ChannelId,
    },
    /// Deliver a scheduled message now (already removed from the schedule).
    SendNow(Entry),
    /// Cancel a pending scheduled message (already removed from the schedule).
    CancelScheduled(Entry),
    /// A saved draft was deleted from the Drafts tab.
    Deleted,
    Close,
}

pub struct DraftsView {
    tab: DraftsTab,
    drafts: Vec<Entry>,
    scheduled: Vec<Entry>,
    sent: Vec<Entry>,
}

impl EventEmitter<DraftsEvent> for DraftsView {}

impl DraftsView {
    pub fn new(cx: &mut App) -> Entity<DraftsView> {
        cx.new(|_| DraftsView {
            tab: DraftsTab::Drafts,
            drafts: Vec::new(),
            scheduled: Vec::new(),
            sent: Vec::new(),
        })
    }

    /// Save the draft of one conversation; blank text removes it.
    pub fn set_draft(&mut self, entry: Entry, cx: &mut Context<Self>) {
        self.drafts
            .retain(|d| !(d.workspace == entry.workspace && d.channel == entry.channel));
        if !entry.text.trim().is_empty() {
            self.drafts.insert(0, entry);
        }
        cx.notify();
    }

    /// The saved draft of one conversation.
    pub fn draft(&self, workspace: usize, channel: &ChannelId) -> Option<String> {
        self.drafts
            .iter()
            .find(|d| d.workspace == workspace && d.channel == *channel)
            .map(|d| d.text.clone())
    }

    pub fn draft_count(&self) -> usize {
        self.drafts.len()
    }

    /// All saved drafts for `workspace`, newest first.
    pub fn drafts_for(&self, workspace: usize) -> Vec<Entry> {
        self.drafts
            .iter()
            .filter(|d| d.workspace == workspace)
            .cloned()
            .collect()
    }

    /// Restore persisted drafts at launch (replacing any existing entries for their workspaces).
    pub fn load_drafts(&mut self, entries: Vec<Entry>, cx: &mut Context<Self>) {
        for entry in entries {
            if !entry.text.trim().is_empty() {
                self.drafts
                    .retain(|d| !(d.workspace == entry.workspace && d.channel == entry.channel));
                self.drafts.push(entry);
            }
        }
        cx.notify();
    }

    pub fn schedule(&mut self, entry: Entry, cx: &mut Context<Self>) {
        let at = entry.at;
        let index = self.scheduled.partition_point(|s| s.at <= at);
        self.scheduled.insert(index, entry);
        cx.notify();
    }

    /// Remove unconfirmed local optimistic scheduled entries (`id.is_none()`) for `workspace`
    /// when Slack rejects `chat.scheduleMessage` / `drafts.create`.
    pub fn drop_pending_scheduled(&mut self, workspace: usize, cx: &mut Context<Self>) {
        let before = self.scheduled.len();
        self.scheduled
            .retain(|e| !(e.workspace == workspace && e.id.is_none()));
        if self.scheduled.len() != before {
            cx.notify();
        }
    }

    /// Replace `workspace`'s scheduled list with Slack's authoritative
    /// `chat.scheduledMessages.list` entries, sorted earliest first.
    pub fn set_remote_scheduled(
        &mut self,
        workspace: usize,
        mut entries: Vec<Entry>,
        cx: &mut Context<Self>,
    ) {
        self.scheduled.retain(|e| e.workspace != workspace);
        entries.sort_by_key(|e| e.at);
        self.scheduled.extend(entries);
        self.scheduled.sort_by_key(|e| e.at);
        cx.notify();
    }

    pub fn record_sent(&mut self, entry: Entry, cx: &mut Context<Self>) {
        self.sent.insert(0, entry);
        cx.notify();
    }

    /// Merge persisted recent sent messages from the local SQLite store for `workspace`,
    /// deduplicating against in-session entries and sorting newest first.
    pub fn set_stored_sent(
        &mut self,
        workspace: usize,
        stored: Vec<Entry>,
        cx: &mut Context<Self>,
    ) {
        for entry in stored {
            let duplicate = self.sent.iter().any(|existing| {
                existing.workspace == workspace
                    && existing.channel == entry.channel
                    && existing.text == entry.text
                    && existing.at.abs_diff(entry.at) <= 5
            });
            if !duplicate {
                self.sent.push(entry);
            }
        }
        self.sent.sort_by_key(|a| std::cmp::Reverse(a.at));
        self.sent.truncate(100);
        cx.notify();
    }

    pub fn show(&mut self, tab: DraftsTab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    fn entries(&self) -> &[Entry] {
        match self.tab {
            DraftsTab::Drafts => &self.drafts,
            DraftsTab::Scheduled => &self.scheduled,
            DraftsTab::Sent => &self.sent,
        }
    }
}

impl Render for DraftsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, foreground, link, hover) = (
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.link,
            theme.accent,
        );
        let now = now_secs();

        let tabs = [
            (DraftsTab::Drafts, "Drafts", self.drafts.len()),
            (DraftsTab::Scheduled, "Scheduled", self.scheduled.len()),
            (DraftsTab::Sent, "Sent", self.sent.len()),
        ];
        let tab_row = h_flex()
            .flex_none()
            .px_4()
            .gap_5()
            .h(px(38.))
            .border_b_1()
            .border_color(border)
            .children(tabs.map(|(tab, label, count)| {
                let active = tab == self.tab;
                let color = if active { foreground } else { muted };
                div()
                    .id(SharedString::from(format!("drafts-tab-{label}")))
                    .h_full()
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(color)
                    .when(active, |tab| {
                        tab.font_weight(FontWeight::SEMIBOLD)
                            .border_b_2()
                            .border_color(link)
                    })
                    .hover(|el| el.text_color(foreground))
                    .child(label)
                    .when(count > 0, |tab| {
                        tab.child(div().text_xs().text_color(muted).child(count.to_string()))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.show(tab, cx)))
            }));

        let tab = self.tab;
        let rows: Vec<_> = self
            .entries()
            .iter()
            .enumerate()
            .map(|(ix, entry)| {
                let stamp = match tab {
                    DraftsTab::Scheduled => format!("Sends {}", when_label(entry.at, now)),
                    _ => when_label(entry.at, now),
                };
                let (workspace, channel) = (entry.workspace, entry.channel.clone());
                let actions = match tab {
                    DraftsTab::Drafts => h_flex().child(
                        Button::new(("draft-delete", ix))
                            .ghost()
                            .with_size(Size::Small)
                            .icon(IconName::Trash)
                            .tooltip("Delete draft")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.drafts.remove(ix);
                                cx.emit(DraftsEvent::Deleted);
                                cx.notify();
                            })),
                    ),
                    DraftsTab::Scheduled => h_flex()
                        .gap_1()
                        .child(
                            Button::new(("scheduled-send", ix))
                                .ghost()
                                .with_size(Size::Small)
                                .label("Send now")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let entry = this.scheduled.remove(ix);
                                    cx.emit(DraftsEvent::SendNow(entry));
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(("scheduled-cancel", ix))
                                .ghost()
                                .with_size(Size::Small)
                                .label("Cancel")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let entry = this.scheduled.remove(ix);
                                    cx.emit(DraftsEvent::CancelScheduled(entry));
                                    cx.notify();
                                })),
                        ),
                    DraftsTab::Sent => h_flex(),
                };
                h_flex()
                    .id(("drafts-row", ix))
                    .items_start()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(border)
                    .cursor_pointer()
                    .hover(|el| el.bg(hover))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(entry.label.clone()),
                                    )
                                    .child(div().text_xs().text_color(muted).child(stamp)),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .truncate()
                                    .child(entry.text.replace('\n', " ")),
                            ),
                    )
                    .child(actions)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(DraftsEvent::Open {
                            workspace,
                            channel: channel.clone(),
                        });
                    }))
            })
            .collect();

        let empty = rows.is_empty().then(|| {
            let (icon, text) = match tab {
                DraftsTab::Drafts => (
                    IconName::Pencil,
                    "No drafts. Unsent text in a conversation shows up here.",
                ),
                DraftsTab::Scheduled => (
                    IconName::Clock,
                    "Nothing scheduled. Use the arrow beside Send to schedule a message.",
                ),
                DraftsTab::Sent => (
                    IconName::Send,
                    "Messages you send this session show up here.",
                ),
            };
            v_flex()
                .items_center()
                .gap_2()
                .py_10()
                .text_sm()
                .text_color(muted)
                .child(Icon::new(icon).text_color(muted).size(px(28.)))
                .child(text)
        });

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .h(px(46.))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .font_weight(FontWeight::BOLD)
                            .child(
                                Icon::new(IconName::Send)
                                    .text_color(foreground)
                                    .size(px(16.)),
                            )
                            .child("Drafts & sent"),
                    )
                    .child(
                        Button::new("drafts-close")
                            .ghost()
                            .with_size(Size::Small)
                            .icon(IconName::X)
                            .tooltip("Close")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DraftsEvent::Close))),
                    ),
            )
            .child(tab_row)
            .child(
                v_flex()
                    .id("drafts-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .children(empty),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui;

    #[test]
    fn send_later_and_when_label_compute_schedule_times() {
        // Day 0 = Thursday 1970-01-01; day 4 = Monday 1970-01-05.
        let thursday = 10 * HOUR + 15 * 60;
        assert_eq!(
            SendLater::InThirtyMinutes.at_with_offset(thursday, 0),
            thursday + 1_800
        );
        assert_eq!(
            SendLater::InOneHour.at_with_offset(thursday, 0),
            thursday + 3_600
        );
        assert_eq!(
            SendLater::TomorrowMorning.at_with_offset(thursday, 0),
            DAY + 9 * HOUR
        );
        assert_eq!(
            SendLater::NextMonday.at_with_offset(thursday, 0),
            4 * DAY + 9 * HOUR
        );

        // CET (+01:00): Tomorrow at 09:00 local = 08:00 UTC.
        assert_eq!(
            SendLater::TomorrowMorning.at_with_offset(thursday, 3_600),
            DAY + 8 * HOUR
        );
        assert_eq!(
            when_label_with_offset(DAY + 8 * HOUR, thursday, 3_600),
            "Tomorrow 09:00"
        );
        // 10:00 UTC is 11:00 CET local.
        assert_eq!(
            when_label_with_offset(10 * HOUR, thursday, 3_600),
            "Today 11:00"
        );
        // EST (-05:00): 02:00 UTC on day 1 is still day 0 at 21:00 local.
        assert_eq!(
            when_label_with_offset(DAY + 2 * HOUR, thursday, -5 * 3_600),
            "Today 21:00"
        );

        // On Monday itself, NextMonday schedules for the following Monday (+7 days).
        let monday = 4 * DAY + 8 * HOUR;
        assert_eq!(
            SendLater::NextMonday.at_with_offset(monday, 0),
            11 * DAY + 9 * HOUR
        );

        assert_eq!(when_label_with_offset(thursday, thursday, 0), "Today 10:15");
        assert_eq!(
            when_label_with_offset(DAY + 9 * HOUR, thursday, 0),
            "Tomorrow 09:00"
        );
        assert_eq!(
            when_label_with_offset(3 * DAY + 8 * HOUR + 5 * 60, 4 * DAY + 12 * HOUR, 0),
            "Yesterday 08:05"
        );
        assert_eq!(
            when_label_with_offset(4 * DAY + 9 * HOUR, thursday, 0),
            "Mon 09:00"
        );
    }

    #[gpui::test]
    fn drafts_view_upserts_dedups_and_sorts_scheduled_and_sent(cx: &mut gpui::TestAppContext) {
        let view = cx.update(DraftsView::new);
        let c1 = ChannelId::from("C1");
        let c2 = ChannelId::from("C2");
        let entry = |ws, ch: &ChannelId, text: &str, at| Entry {
            id: None,
            workspace: ws,
            channel: ch.clone(),
            label: format!("# {}", ch.as_str()),
            text: text.into(),
            at,
        };

        // Upsert draft, replace existing draft for same (ws, channel), and delete on blank text.
        view.update(cx, |v, cx| {
            v.set_draft(entry(0, &c1, "first", 10), cx);
            v.set_draft(entry(0, &c1, "updated", 20), cx);
            v.set_draft(entry(1, &c1, "other ws", 30), cx);
            v.set_draft(entry(0, &c2, "   ", 40), cx);
        });
        view.read_with(cx, |v, _| {
            assert_eq!(v.draft_count(), 2);
            assert_eq!(v.draft(0, &c1).as_deref(), Some("updated"));
            assert_eq!(v.draft(0, &c2), None);
            assert_eq!(v.drafts_for(0).len(), 1);
            assert_eq!(v.drafts_for(1).len(), 1);
        });
        view.update(cx, |v, cx| v.set_draft(entry(0, &c1, "", 50), cx));
        view.read_with(cx, |v, _| {
            assert_eq!(v.draft(0, &c1), None);
            assert_eq!(v.draft_count(), 1);
        });

        // Scheduled entries stay ordered earliest-first across `schedule` and `set_remote_scheduled`.
        view.update(cx, |v, cx| {
            v.schedule(entry(0, &c1, "later", 500), cx);
            v.schedule(entry(0, &c2, "sooner", 200), cx);
            v.schedule(entry(1, &c1, "ws1", 300), cx);
        });
        view.read_with(cx, |v, _| {
            let ats: Vec<u64> = v.scheduled.iter().map(|e| e.at).collect();
            assert_eq!(ats, vec![200, 300, 500]);
        });
        view.update(cx, |v, cx| {
            let mut confirmed = entry(0, &c1, "remote", 100);
            confirmed.id = Some("Dr1".to_owned());
            v.set_remote_scheduled(0, vec![confirmed], cx);
            v.schedule(entry(0, &c2, "optimistic", 150), cx);
        });
        view.update(cx, |v, cx| {
            v.drop_pending_scheduled(0, cx);
        });
        view.read_with(cx, |v, _| {
            // Only the unconfirmed workspace-0 entry was dropped; confirmed remote + ws-1 stay.
            let ats: Vec<u64> = v.scheduled.iter().map(|e| e.at).collect();
            assert_eq!(ats, vec![100, 300]);
            assert!(v.scheduled.iter().all(|e| e.text != "optimistic"));
        });

        // `set_stored_sent` deduplicates within 5 seconds and sorts newest-first.
        view.update(cx, |v, cx| {
            v.record_sent(entry(0, &c1, "hello", 1000), cx);
            v.set_stored_sent(
                0,
                vec![
                    entry(0, &c1, "hello", 1003),
                    entry(0, &c1, "older", 900),
                    entry(0, &c2, "newer", 1200),
                ],
                cx,
            );
        });
        view.read_with(cx, |v, _| {
            let texts: Vec<&str> = v.sent.iter().map(|e| e.text.as_str()).collect();
            assert_eq!(texts, vec!["newer", "hello", "older"]);
        });
    }
}
