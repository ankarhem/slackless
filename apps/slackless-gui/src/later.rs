//! Later: saved messages and their reminders (docs/research/api-reactions-saved.md, the
//! `saved.*` family behind Slack's "Later" tab). The view replaces the message column while
//! open, with tabs In progress / Archived / Completed; each item can be completed, archived,
//! moved back to in progress, given or cleared a reminder, or removed. Due reminders are
//! handed to the root by [`LaterView::take_due`], which posts the system notification.
//!
//! Fixture mode keeps everything in memory. Each workspace is seeded once from its fixtures
//! ([`LaterView::seed`]) with a few saved messages: one due later today, one overdue, one
//! completed and one archived, so every tab and due state renders.
//!
//! `LaterView::new(&mut App) -> Entity<LaterView>`
//! `LaterView::save(&mut self, SaveRequest, workspace, label, cx)` adds or updates an item
//! emits `LaterEvent::Open { workspace, channel }` / `LaterEvent::Unsaved { workspace, ts }`
//! / `LaterEvent::Changed` / `LaterEvent::Close`

use std::collections::HashSet;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};
use slackless_core::types::{ChannelId, Ts};

use crate::drafts::{now_secs, when_label};
use crate::fixtures::Fixtures;

const DAY: u64 = 86_400;
const HOUR: u64 = 3_600;
const MINUTE: u64 = 60;

/// The "Remind me about this" presets offered on messages and Later items.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Remind {
    InTwentyMinutes,
    InOneHour,
    InThreeHours,
    Tomorrow,
    NextWeek,
}

impl Remind {
    pub const ALL: [Remind; 5] = [
        Remind::InTwentyMinutes,
        Remind::InOneHour,
        Remind::InThreeHours,
        Remind::Tomorrow,
        Remind::NextWeek,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Remind::InTwentyMinutes => "In 20 minutes",
            Remind::InOneHour => "In 1 hour",
            Remind::InThreeHours => "In 3 hours",
            Remind::Tomorrow => "Tomorrow at 9:00",
            Remind::NextWeek => "Next week",
        }
    }

    /// Due time in unix seconds, counted from `now` (UTC in fixture mode).
    pub fn at(self, now: u64) -> u64 {
        match self {
            Remind::InTwentyMinutes => now + 20 * MINUTE,
            Remind::InOneHour => now + HOUR,
            Remind::InThreeHours => now + 3 * HOUR,
            Remind::Tomorrow => (now / DAY + 1) * DAY + 9 * HOUR,
            Remind::NextWeek => (now / DAY + 7) * DAY + 9 * HOUR,
        }
    }
}

/// A message view's request to save (or unsave) a message, optionally with a reminder.
#[derive(Clone)]
pub struct SaveRequest {
    pub channel: ChannelId,
    pub ts: Ts,
    pub author: String,
    pub text: String,
    /// `false` removes the item.
    pub saved: bool,
    /// Unix seconds of the reminder, if one was asked for.
    pub due: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LaterState {
    InProgress,
    Archived,
    Completed,
}

/// One saved message.
#[derive(Clone)]
pub struct LaterItem {
    pub workspace: usize,
    pub channel: ChannelId,
    pub ts: Ts,
    /// Conversation label (`# name` / peer names).
    pub label: String,
    pub author: String,
    pub text: String,
    pub state: LaterState,
    /// Reminder time (unix seconds).
    pub due: Option<u64>,
    /// The reminder already fired (overdue items stay listed, flagged red).
    pub fired: bool,
}

impl LaterItem {
    fn is(&self, workspace: usize, channel: &ChannelId, ts: &Ts) -> bool {
        self.workspace == workspace && self.channel == *channel && self.ts == *ts
    }
}

pub enum LaterEvent {
    Open {
        workspace: usize,
        channel: ChannelId,
    },
    /// An item was removed from Later; message views drop their saved marker.
    Unsaved {
        workspace: usize,
        ts: Ts,
    },
    /// Counts changed (the sidebar's Later row).
    Changed,
    Close,
}

pub struct LaterView {
    tab: LaterState,
    items: Vec<LaterItem>,
    /// Workspaces already seeded from fixtures.
    seeded: HashSet<usize>,
}

impl EventEmitter<LaterEvent> for LaterView {}

impl LaterView {
    pub fn new(cx: &mut App) -> Entity<LaterView> {
        cx.new(|_| LaterView {
            tab: LaterState::InProgress,
            items: Vec::new(),
            seeded: HashSet::new(),
        })
    }

    /// Seed workspace `workspace` with saved fixture messages (once per workspace): the
    /// newest messages of its first conversations, in every tab and due state.
    pub fn seed(&mut self, workspace: usize, fixtures: &Fixtures, cx: &mut Context<Self>) {
        if !self.seeded.insert(workspace) {
            return;
        }
        let now = now_secs();
        let plans = [
            (LaterState::InProgress, Some(now + 2 * HOUR)),
            (LaterState::InProgress, Some(now.saturating_sub(HOUR))),
            (LaterState::InProgress, None),
            (LaterState::Completed, None),
            (LaterState::Archived, None),
        ];
        let conversations = fixtures.channels.iter().chain(&fixtures.dms);
        let picks = conversations.filter_map(|conversation| {
            let message = fixtures
                .messages
                .get(&conversation.id)?
                .iter()
                .rev()
                .find(|message| message.user.is_some() && !message.text.is_empty())?;
            Some((conversation, message))
        });
        for ((conversation, message), (state, due)) in picks.zip(plans) {
            let author = message
                .user
                .as_ref()
                .map(|user| fixtures.user_name(user).to_owned())
                .unwrap_or_default();
            self.items.push(LaterItem {
                workspace,
                channel: conversation.id.clone(),
                ts: message.ts.clone(),
                label: fixtures.conversation_label(conversation),
                author,
                text: fixtures.preview(message),
                state,
                due,
                // A seeded overdue reminder shows as overdue without notifying at startup.
                fired: due.is_some_and(|due| due <= now),
            });
        }
        cx.emit(LaterEvent::Changed);
        cx.notify();
    }

    /// Apply a message view's save request in `workspace` (conversation `label`).
    pub fn save(
        &mut self,
        request: SaveRequest,
        workspace: usize,
        label: String,
        cx: &mut Context<Self>,
    ) {
        let existing = self
            .items
            .iter()
            .position(|item| item.is(workspace, &request.channel, &request.ts));
        match (request.saved, existing) {
            (false, Some(ix)) => {
                self.items.remove(ix);
            }
            (false, None) => {}
            (true, Some(ix)) => {
                let item = &mut self.items[ix];
                item.state = LaterState::InProgress;
                if request.due.is_some() {
                    item.due = request.due;
                    item.fired = false;
                }
            }
            (true, None) => self.items.insert(
                0,
                LaterItem {
                    workspace,
                    channel: request.channel,
                    ts: request.ts,
                    label,
                    author: request.author,
                    text: request.text,
                    state: LaterState::InProgress,
                    due: request.due,
                    fired: false,
                },
            ),
        }
        cx.emit(LaterEvent::Changed);
        cx.notify();
    }

    /// Saved message ids of `workspace`, for the message views' saved markers.
    pub fn saved_in(&self, workspace: usize) -> Vec<(ChannelId, Ts)> {
        self.items
            .iter()
            .filter(|item| item.workspace == workspace)
            .map(|item| (item.channel.clone(), item.ts.clone()))
            .collect()
    }

    /// Items still in progress (the sidebar's Later count).
    pub fn in_progress_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.state == LaterState::InProgress)
            .count()
    }

    /// In-progress items whose reminder is due at `now` and has not fired yet; marks them
    /// fired.
    pub fn take_due(&mut self, now: u64, cx: &mut Context<Self>) -> Vec<LaterItem> {
        let due: Vec<LaterItem> = self
            .items
            .iter_mut()
            .filter(|item| {
                item.state == LaterState::InProgress
                    && !item.fired
                    && item.due.is_some_and(|due| due <= now)
            })
            .map(|item| {
                item.fired = true;
                item.clone()
            })
            .collect();
        if !due.is_empty() {
            cx.notify();
        }
        due
    }

    pub fn show(&mut self, tab: LaterState, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    fn set_state(&mut self, ix: usize, state: LaterState, cx: &mut Context<Self>) {
        self.items[ix].state = state;
        cx.emit(LaterEvent::Changed);
        cx.notify();
    }

    fn set_due(&mut self, ix: usize, due: Option<u64>, cx: &mut Context<Self>) {
        let item = &mut self.items[ix];
        item.due = due;
        item.fired = false;
        cx.notify();
    }

    fn remove(&mut self, ix: usize, cx: &mut Context<Self>) {
        let item = self.items.remove(ix);
        cx.emit(LaterEvent::Unsaved {
            workspace: item.workspace,
            ts: item.ts,
        });
        cx.emit(LaterEvent::Changed);
        cx.notify();
    }

    fn count(&self, state: LaterState) -> usize {
        self.items.iter().filter(|item| item.state == state).count()
    }

    /// The reminder dropdown of item `ix`: the presets plus "Remove reminder" when one is set.
    fn reminder_button(
        &self,
        ix: usize,
        has_due: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let this = cx.weak_entity();
        Button::new(("later-remind", ix))
            .ghost()
            .with_size(Size::Small)
            .icon(IconName::AlarmClock)
            .tooltip("Remind me")
            .dropdown_menu(move |mut menu, _, _| {
                for remind in Remind::ALL {
                    let this = this.clone();
                    menu = menu.item(PopupMenuItem::new(remind.label()).on_click(
                        move |_, _, cx| {
                            this.update(cx, |view, cx| {
                                view.set_due(ix, Some(remind.at(now_secs())), cx)
                            })
                            .ok();
                        },
                    ));
                }
                if has_due {
                    let this = this.clone();
                    menu = menu.separator().item(
                        PopupMenuItem::new("Remove reminder")
                            .icon(IconName::BellOff)
                            .on_click(move |_, _, cx| {
                                this.update(cx, |view, cx| view.set_due(ix, None, cx)).ok();
                            }),
                    );
                }
                menu
            })
    }
}

impl Render for LaterView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, foreground, link, hover, danger) = (
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.link,
            theme.accent,
            theme.danger,
        );
        let now = now_secs();

        let tabs = [
            (LaterState::InProgress, "In progress"),
            (LaterState::Archived, "Archived"),
            (LaterState::Completed, "Completed"),
        ];
        let tab_row = h_flex()
            .flex_none()
            .px_4()
            .gap_5()
            .h(px(38.))
            .border_b_1()
            .border_color(border)
            .children(tabs.map(|(tab, label)| {
                let active = tab == self.tab;
                let color = if active { foreground } else { muted };
                let count = self.count(tab);
                div()
                    .id(SharedString::from(format!("later-tab-{label}")))
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
        let shown: Vec<(usize, LaterItem)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.state == tab)
            .map(|(ix, item)| (ix, item.clone()))
            .collect();
        let rows: Vec<_> = shown
            .into_iter()
            .map(|(ix, item)| {
                let due_chip = item.due.map(|due| {
                    let overdue = due <= now && item.state == LaterState::InProgress;
                    let color = if overdue { danger } else { muted };
                    let text = if overdue {
                        format!("Overdue: {}", when_label(due, now))
                    } else {
                        format!("Due {}", when_label(due, now))
                    };
                    h_flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .px_1p5()
                        .rounded_md()
                        .text_xs()
                        .text_color(color)
                        .bg(color.opacity(0.12))
                        .child(
                            Icon::new(IconName::AlarmClock)
                                .text_color(color)
                                .size(px(12.)),
                        )
                        .child(text)
                });
                let mut actions = h_flex().flex_none().gap_0p5();
                match item.state {
                    LaterState::InProgress => {
                        actions = actions
                            .child(
                                Button::new(("later-complete", ix))
                                    .ghost()
                                    .with_size(Size::Small)
                                    .icon(IconName::CircleCheck)
                                    .tooltip("Mark complete")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_state(ix, LaterState::Completed, cx);
                                    })),
                            )
                            .child(self.reminder_button(ix, item.due.is_some(), cx))
                            .child(
                                Button::new(("later-archive", ix))
                                    .ghost()
                                    .with_size(Size::Small)
                                    .icon(IconName::Archive)
                                    .tooltip("Archive")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_state(ix, LaterState::Archived, cx);
                                    })),
                            );
                    }
                    LaterState::Archived | LaterState::Completed => {
                        actions = actions.child(
                            Button::new(("later-restore", ix))
                                .ghost()
                                .with_size(Size::Small)
                                .icon(IconName::RotateCcw)
                                .tooltip("Move to in progress")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_state(ix, LaterState::InProgress, cx);
                                })),
                        );
                    }
                }
                actions = actions.child(
                    Button::new(("later-remove", ix))
                        .ghost()
                        .with_size(Size::Small)
                        .icon(IconName::Trash)
                        .tooltip("Remove from Later")
                        .on_click(cx.listener(move |this, _, _, cx| this.remove(ix, cx))),
                );
                let (workspace, channel) = (item.workspace, item.channel.clone());
                let completed = item.state == LaterState::Completed;
                h_flex()
                    .id(("later-row", ix))
                    .items_start()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(border)
                    .cursor_pointer()
                    .hover(|el| el.bg(hover))
                    .child(
                        Icon::new(if completed {
                            IconName::BookmarkCheck
                        } else {
                            IconName::Bookmark
                        })
                        .text_color(if completed { muted } else { link })
                        .size(px(16.)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(item.author.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(format!("in {}", item.label)),
                                    )
                                    .children(due_chip),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .truncate()
                                    .when(completed, |text| text.text_color(muted).line_through())
                                    .child(item.text.replace('\n', " ")),
                            ),
                    )
                    .child(actions)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(LaterEvent::Open {
                            workspace,
                            channel: channel.clone(),
                        });
                    }))
            })
            .collect();

        let empty = rows.is_empty().then(|| {
            let (icon, text) = match tab {
                // Slack's empty Saved page.
                LaterState::InProgress => (IconName::Bookmark, "Nothing saved yet"),
                LaterState::Archived => (IconName::Archive, "Archived items show up here."),
                LaterState::Completed => (IconName::CircleCheck, "Completed items show up here."),
            };
            v_flex()
                .items_center()
                .gap_2()
                .py_10()
                .text_sm()
                .text_color(muted)
                .child(Icon::new(icon).text_color(muted).size(px(28.)))
                .child(
                    div()
                        .text_color(foreground)
                        .font_weight(FontWeight::BOLD)
                        .child(text),
                )
                .when(tab == LaterState::InProgress, |empty| {
                    empty.child("Save a message to keep it here.")
                })
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
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            // Slack's sidebar row and page are both "Saved".
                            .child("Saved"),
                    )
                    .child(
                        Button::new("later-close")
                            .ghost()
                            .with_size(Size::Small)
                            .icon(IconName::X)
                            .tooltip("Close")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(LaterEvent::Close))),
                    ),
            )
            .child(tab_row)
            .child(
                v_flex()
                    .id("later-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .children(empty),
            )
    }
}
