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
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, Size, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::api::SavedMark;
use slackless_core::types::{ChannelId, Ts};

use crate::drafts::{now_secs, when_label};
use crate::fixtures::Fixtures;

const DAY: u64 = 86_400;
const HOUR: u64 = 3_600;
const MINUTE: u64 = 60;

/// Open a local date/time dialog (`YYYY-MM-DD`, `HH:MM`) shared by custom reminders and
/// custom message scheduling.
pub fn open_custom_datetime_dialog(
    title: &'static str,
    subtitle: &'static str,
    confirm_label: &'static str,
    window: &mut Window,
    cx: &mut App,
    on_confirm: impl Fn(u64, String, String, &mut Window, &mut App) + 'static,
) {
    let on_confirm = std::rc::Rc::new(on_confirm);
    let (default_date, default_time) = crate::messages::default_custom_datetime();
    let date_state = cx.new(|cx| {
        let mut s = TextareaState::new(window, cx)
            .placeholder("YYYY-MM-DD")
            .auto_grow(1, 1);
        s.set_value(default_date, window, cx);
        s
    });
    let time_state = cx.new(|cx| {
        let mut s = TextareaState::new(window, cx)
            .placeholder("HH:MM")
            .auto_grow(1, 1);
        s.set_value(default_time, window, cx);
        s
    });
    time_state.read(cx).focus_handle(cx).focus(window, cx);
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = cx.theme();
        let (muted, bg) = (theme.muted_foreground, theme.background);
        let (date_state, time_state, on_confirm) =
            (date_state.clone(), time_state.clone(), on_confirm.clone());
        dialog.title(title).w(px(400.)).child(
            v_flex()
                .gap_3()
                .p_4()
                .child(div().text_xs().text_color(muted).child(subtitle))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            v_flex()
                                .flex_1()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("Date"),
                                )
                                .child(
                                    crate::preferences::text_input_box(&date_state, false, cx)
                                        .bg(bg),
                                ),
                        )
                        .child(
                            v_flex()
                                .w(px(120.))
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("Time"),
                                )
                                .child(
                                    crate::preferences::text_input_box(&time_state, false, cx)
                                        .bg(bg),
                                ),
                        ),
                )
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            crate::preferences::form_button(
                                Button::new("custom-datetime-cancel")
                                    .outline()
                                    .label("Cancel"),
                                cx,
                            )
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            crate::preferences::form_button(
                                Button::new("custom-datetime-save")
                                    .primary()
                                    .label(confirm_label),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                let d = date_state.read(cx).value().trim().to_owned();
                                let t = time_state.read(cx).value().trim().to_owned();
                                let Some(at) = crate::messages::parse_local_schedule(&d, &t) else {
                                    crate::actions::notify(
                                        "Invalid date or time",
                                        "Use YYYY-MM-DD and HH:MM in the future.",
                                        window,
                                        cx,
                                    );
                                    return;
                                };
                                window.close_dialog(cx);
                                on_confirm(at, d, t, window, cx);
                            }),
                        ),
                ),
        )
    });
}

/// Open a local date/time dialog for setting a custom reminder due time (`YYYY-MM-DD`, `HH:MM`).
pub fn open_custom_remind_dialog(
    window: &mut Window,
    cx: &mut App,
    on_confirm: impl Fn(u64, &mut Window, &mut App) + 'static,
) {
    open_custom_datetime_dialog(
        "Set a custom reminder",
        "Enter a local date (YYYY-MM-DD) and 24-hour time (HH:MM).",
        "Set reminder",
        window,
        cx,
        move |at, _, _, window, cx| on_confirm(at, window, cx),
    );
}

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

    /// Due time in unix seconds, counted from `now` at 09:00 local time for multi-day presets.
    pub fn at(self, now: u64) -> u64 {
        let local_9am_days_ahead = |days_ahead: i64| {
            let now_i = now as i64;
            let off_now = crate::messages::local_utc_offset(now_i);
            let local_day = (now_i + off_now).div_euclid(DAY as i64);
            let target_local = (local_day + days_ahead) * (DAY as i64) + 9 * (HOUR as i64);
            let off_target = crate::messages::local_utc_offset(target_local - off_now);
            (target_local - off_target).max(0) as u64
        };
        match self {
            Remind::InTwentyMinutes => now + 20 * MINUTE,
            Remind::InOneHour => now + HOUR,
            Remind::InThreeHours => now + 3 * HOUR,
            Remind::Tomorrow => local_9am_days_ahead(1),
            Remind::NextWeek => local_9am_days_ahead(7),
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

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum LaterFilter {
    #[default]
    All,
    Saved,
    Todos,
}

/// Sentinel channel id used for standalone personal to-dos created directly in Later (`F-CORE-162`).
pub const PERSONAL_TODO_CHANNEL: &str = "TODO";

/// One saved message or standalone personal to-do (`F-CORE-161`..`166`).
#[derive(Clone)]
pub struct LaterItem {
    pub workspace: usize,
    pub channel: ChannelId,
    pub ts: Ts,
    /// Conversation label (`# name` / peer names / `"To-dos"`).
    pub label: String,
    pub author: String,
    pub text: String,
    pub state: LaterState,
    /// Reminder time (unix seconds).
    pub due: Option<u64>,
    /// The reminder already fired (overdue items stay listed, flagged red).
    pub fired: bool,
    /// `true` when this item is a personal To-do or moved from Saved to To-dos (`F-CORE-162`, `165`).
    pub is_todo: bool,
}

impl LaterItem {
    fn is(&self, workspace: usize, channel: &ChannelId, ts: &Ts) -> bool {
        self.workspace == workspace && self.channel == *channel && self.ts == *ts
    }

    fn key(&self) -> LaterKey {
        LaterKey {
            workspace: self.workspace,
            channel: self.channel.clone(),
            ts: self.ts.clone(),
        }
    }

    pub fn is_personal_todo(&self) -> bool {
        self.channel.as_str() == PERSONAL_TODO_CHANNEL
    }
}

/// A Later item's identity, what the row actions capture instead of a position that a save
/// or a refresh landing meanwhile can shift onto another item.
#[derive(Clone)]
struct LaterKey {
    workspace: usize,
    channel: ChannelId,
    ts: Ts,
}

pub enum LaterEvent {
    Open {
        workspace: usize,
        channel: ChannelId,
        ts: Ts,
    },
    /// An item was removed from Later; message views drop their saved marker and live mode
    /// calls `saved.delete`.
    Unsaved {
        workspace: usize,
        channel: ChannelId,
        ts: Ts,
    },
    /// State or reminder changed on a Later item; live mode calls `saved.update`.
    Update {
        workspace: usize,
        channel: ChannelId,
        ts: Ts,
        mark: Option<SavedMark>,
        date_due: Option<Option<u64>>,
    },
    /// Clear all completed items; live mode calls `saved.clearCompleted`.
    ClearCompleted {
        workspace: usize,
    },
    /// Counts changed (the sidebar's Later row).
    Changed,
    /// Open Saved / Later in a standalone pop-out window (`F-SHELL-149`).
    PopOut,
    Close,
}

pub struct LaterView {
    workspace: usize,
    tab: LaterState,
    filter: LaterFilter,
    creating_todo: bool,
    todo_input: Option<Entity<TextareaState>>,
    items: Vec<LaterItem>,
    /// Workspaces already seeded from fixtures.
    seeded: HashSet<usize>,
}

impl EventEmitter<LaterEvent> for LaterView {}

impl LaterView {
    pub fn new(cx: &mut App) -> Entity<LaterView> {
        cx.new(|_| LaterView {
            workspace: 0,
            tab: LaterState::InProgress,
            filter: LaterFilter::All,
            creating_todo: false,
            todo_input: None,
            items: Vec::new(),
            seeded: HashSet::new(),
        })
    }

    /// Create a standalone personal to-do item in the current workspace (`F-CORE-162`).
    pub fn add_todo(&mut self, text: String, cx: &mut Context<Self>) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let ts = Ts::from(format!("{}.{:06}", now_secs(), self.items.len()));
        self.items.insert(
            0,
            LaterItem {
                workspace: self.workspace,
                channel: ChannelId::from(PERSONAL_TODO_CHANNEL),
                ts,
                label: "To-dos".to_owned(),
                author: "Personal to-do".to_owned(),
                text: trimmed.to_owned(),
                state: LaterState::InProgress,
                due: None,
                fired: false,
                is_todo: true,
            },
        );
        self.tab = LaterState::InProgress;
        cx.emit(LaterEvent::Changed);
        cx.notify();
    }

    /// Toggle whether item `key` lives in To-dos or Saved (`F-CORE-165`).
    fn toggle_todo(&mut self, key: &LaterKey, cx: &mut Context<Self>) {
        if let Some(item) = self.item_mut(key) {
            item.is_todo = !item.is_todo;
            cx.emit(LaterEvent::Changed);
            cx.notify();
        }
    }

    pub fn set_workspace(&mut self, workspace: usize, cx: &mut Context<Self>) {
        if self.workspace != workspace {
            self.workspace = workspace;
            cx.emit(LaterEvent::Changed);
            cx.notify();
        }
    }

    /// Replace `workspace`'s saved list with live items fetched from Slack (`saved.list`),
    /// keeping `fired` flags for reminders that already triggered in this session.
    pub fn replace_live(
        &mut self,
        workspace: usize,
        mut incoming: Vec<LaterItem>,
        cx: &mut Context<Self>,
    ) {
        let now = now_secs();
        let already_fired: HashSet<(ChannelId, Ts)> = self
            .items
            .iter()
            .filter(|item| item.workspace == workspace && item.fired)
            .map(|item| (item.channel.clone(), item.ts.clone()))
            .collect();
        for item in &mut incoming {
            item.workspace = workspace;
            if already_fired.contains(&(item.channel.clone(), item.ts.clone()))
                || item.due.is_some_and(|due| due <= now)
            {
                item.fired = true;
            }
        }
        self.items.retain(|item| item.workspace != workspace);
        self.items.extend(incoming);
        cx.emit(LaterEvent::Changed);
        cx.notify();
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
                is_todo: false,
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
                    is_todo: false,
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

    /// Items still in progress in the active workspace (the sidebar's Later count).
    pub fn in_progress_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.workspace == self.workspace && item.state == LaterState::InProgress)
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

    /// The item `key` names, if it is still listed.
    fn item_mut(&mut self, key: &LaterKey) -> Option<&mut LaterItem> {
        self.items
            .iter_mut()
            .find(|item| item.is(key.workspace, &key.channel, &key.ts))
    }

    fn set_state(&mut self, key: &LaterKey, state: LaterState, cx: &mut Context<Self>) {
        let Some(item) = self.item_mut(key) else {
            return;
        };
        let prev = item.state;
        item.state = state;
        let mark = match (prev, state) {
            (_, LaterState::Completed) => SavedMark::Completed,
            (_, LaterState::Archived) => SavedMark::Archived,
            (LaterState::Archived, LaterState::InProgress) => SavedMark::Unarchived,
            _ => SavedMark::Uncompleted,
        };
        cx.emit(LaterEvent::Update {
            workspace: item.workspace,
            channel: item.channel.clone(),
            ts: item.ts.clone(),
            mark: Some(mark),
            date_due: None,
        });
        cx.emit(LaterEvent::Changed);
        cx.notify();
    }

    fn set_due(&mut self, key: &LaterKey, due: Option<u64>, cx: &mut Context<Self>) {
        let Some(item) = self.item_mut(key) else {
            return;
        };
        item.due = due;
        item.fired = false;
        cx.emit(LaterEvent::Update {
            workspace: item.workspace,
            channel: item.channel.clone(),
            ts: item.ts.clone(),
            mark: None,
            date_due: Some(due),
        });
        cx.notify();
    }

    fn remove(&mut self, key: &LaterKey, cx: &mut Context<Self>) {
        let Some(ix) = self
            .items
            .iter()
            .position(|item| item.is(key.workspace, &key.channel, &key.ts))
        else {
            return;
        };
        let item = self.items.remove(ix);
        cx.emit(LaterEvent::Unsaved {
            workspace: item.workspace,
            channel: item.channel,
            ts: item.ts,
        });
        cx.emit(LaterEvent::Changed);
        cx.notify();
    }

    fn clear_completed(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace;
        let removed: Vec<(ChannelId, Ts)> = self
            .items
            .iter()
            .filter(|item| item.workspace == ws && item.state == LaterState::Completed)
            .map(|item| (item.channel.clone(), item.ts.clone()))
            .collect();
        if removed.is_empty() {
            return;
        }
        self.items
            .retain(|item| !(item.workspace == ws && item.state == LaterState::Completed));
        cx.emit(LaterEvent::ClearCompleted { workspace: ws });
        cx.emit(LaterEvent::Changed);
        cx.notify();
    }

    /// Snapshot of currently shown Later items for a popped-out window (`F-SHELL-149`).
    pub fn items_for_popout(&self) -> Vec<LaterItem> {
        let ws = self.workspace;
        let tab = self.tab;
        self.items
            .iter()
            .filter(|item| item.workspace == ws && item.state == tab)
            .cloned()
            .collect()
    }

    fn count(&self, state: LaterState) -> usize {
        self.items
            .iter()
            .filter(|item| item.workspace == self.workspace && item.state == state)
            .count()
    }

    /// The reminder dropdown of item `key`: the presets plus "Remove reminder" when one is set.
    fn reminder_button(
        &self,
        key: LaterKey,
        has_due: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let this = cx.weak_entity();
        let id = SharedString::from(format!("later-remind-{}", key.ts.as_str()));
        Button::new(id)
            .ghost()
            .with_size(Size::Small)
            .icon(IconName::AlarmClock)
            .tooltip("Remind me")
            .dropdown_menu(move |mut menu, _, _| {
                for remind in Remind::ALL {
                    let this = this.clone();
                    let key = key.clone();
                    menu = menu.item(PopupMenuItem::new(remind.label()).on_click(
                        move |_, _, cx| {
                            this.update(cx, |view, cx| {
                                view.set_due(&key, Some(remind.at(now_secs())), cx)
                            })
                            .ok();
                        },
                    ));
                }
                let custom_this = this.clone();
                let custom_key = key.clone();
                menu = menu.separator().item(
                    PopupMenuItem::new("Custom\u{2026}")
                        .icon(IconName::Calendar)
                        .on_click(move |_, window, cx| {
                            let this = custom_this.clone();
                            let key = custom_key.clone();
                            open_custom_remind_dialog(window, cx, move |due, _, cx| {
                                this.update(cx, |view, cx| view.set_due(&key, Some(due), cx))
                                    .ok();
                            });
                        }),
                );
                if has_due {
                    let this = this.clone();
                    let key = key.clone();
                    menu = menu.separator().item(
                        PopupMenuItem::new("Remove reminder")
                            .icon(IconName::BellOff)
                            .on_click(move |_, _, cx| {
                                this.update(cx, |view, cx| view.set_due(&key, None, cx))
                                    .ok();
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

        let filter = self.filter;
        let filter_pills = h_flex().ml_auto().items_center().gap_1().children(
            [
                (LaterFilter::All, "All"),
                (LaterFilter::Saved, "Saved"),
                (LaterFilter::Todos, "To-dos"),
            ]
            .into_iter()
            .map(|(f, label)| {
                let on = f == filter;
                Button::new(SharedString::from(format!("later-filter-{label}")))
                    .when(on, |b| b.primary())
                    .when(!on, |b| b.ghost())
                    .with_size(Size::XSmall)
                    .label(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = f;
                        cx.notify();
                    }))
            }),
        );
        let tab_row = tab_row.child(filter_pills);

        let tab = self.tab;
        let ws = self.workspace;
        let shown: Vec<(usize, LaterItem)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                item.workspace == ws
                    && item.state == tab
                    && match filter {
                        LaterFilter::All => true,
                        LaterFilter::Saved => !item.is_todo,
                        LaterFilter::Todos => item.is_todo,
                    }
            })
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
                let key = item.key();
                match item.state {
                    LaterState::InProgress => {
                        let complete_key = key.clone();
                        let archive_key = key.clone();
                        actions = actions
                            .child(
                                Button::new(("later-complete", ix))
                                    .ghost()
                                    .with_size(Size::Small)
                                    .icon(IconName::CircleCheck)
                                    .tooltip("Mark complete")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_state(&complete_key, LaterState::Completed, cx);
                                    })),
                            )
                            .child(self.reminder_button(key.clone(), item.due.is_some(), cx))
                            .child(
                                Button::new(("later-archive", ix))
                                    .ghost()
                                    .with_size(Size::Small)
                                    .icon(IconName::Archive)
                                    .tooltip("Archive")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_state(&archive_key, LaterState::Archived, cx);
                                    })),
                            );
                    }
                    LaterState::Archived | LaterState::Completed => {
                        let restore_key = key.clone();
                        actions = actions.child(
                            Button::new(("later-restore", ix))
                                .ghost()
                                .with_size(Size::Small)
                                .icon(IconName::RotateCcw)
                                .tooltip("Move to in progress")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_state(&restore_key, LaterState::InProgress, cx);
                                })),
                        );
                    }
                }
                let is_todo_item = item.is_todo;
                let todo_key = key.clone();
                let remove_key = key.clone();
                actions = actions
                    .child(
                        Button::new(("later-toggle-todo", ix))
                            .ghost()
                            .with_size(Size::Small)
                            .icon(if is_todo_item {
                                IconName::Bookmark
                            } else {
                                IconName::ListChecks
                            })
                            .tooltip(if is_todo_item {
                                "Move to Saved"
                            } else {
                                "Move to To-dos"
                            })
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.toggle_todo(&todo_key, cx)),
                            ),
                    )
                    .child(
                        Button::new(("later-remove", ix))
                            .ghost()
                            .with_size(Size::Small)
                            .icon(IconName::Trash)
                            .tooltip("Remove from Later")
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.remove(&remove_key, cx)),
                            ),
                    );
                let (workspace, channel) = (item.workspace, item.channel.clone());
                let is_personal_todo = item.is_personal_todo();
                let completed = item.state == LaterState::Completed;
                let row_group = SharedString::from(format!("later-row-grp-{ix}"));
                h_flex()
                    .id(("later-row", ix))
                    .group(row_group.clone())
                    .items_start()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(border)
                    .hover(|el| el.bg(hover))
                    .child(
                        h_flex()
                            .id(("later-row-open", ix))
                            .flex_1()
                            .min_w_0()
                            .items_start()
                            .gap_3()
                            .cursor_pointer()
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
                                            .when(completed, |text| {
                                                text.text_color(muted).line_through()
                                            })
                                            .child(item.text.replace('\n', " ")),
                                    ),
                            )
                            .on_click(cx.listener({
                                let open_ts = item.ts.clone();
                                let key = key.clone();
                                move |this, _, _, cx| {
                                    if is_personal_todo {
                                        let next = if completed {
                                            LaterState::InProgress
                                        } else {
                                            LaterState::Completed
                                        };
                                        this.set_state(&key, next, cx);
                                    } else {
                                        cx.emit(LaterEvent::Open {
                                            workspace,
                                            channel: channel.clone(),
                                            ts: open_ts.clone(),
                                        });
                                    }
                                }
                            })),
                    )
                    .child(
                        actions
                            .opacity(0.6)
                            .group_hover(row_group, |el| el.opacity(1.0)),
                    )
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

        let todo_composer = (self.creating_todo)
            .then(|| self.todo_input.clone())
            .flatten()
            .map(|input| {
                let input_add = input.clone();
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        crate::preferences::text_input_box(&input, true, cx)
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        crate::preferences::form_button(
                            Button::new("later-todo-add").primary().label("Add"),
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let text = input_add.read(cx).value().to_string();
                                if !text.trim().is_empty() {
                                    this.add_todo(text, cx);
                                    input_add.update(cx, |st, cx| {
                                        st.set_value(String::new(), window, cx);
                                    });
                                    this.creating_todo = false;
                                }
                            },
                        )),
                    )
                    .child(
                        crate::preferences::form_button(
                            Button::new("later-todo-cancel").ghost().label("Cancel"),
                            cx,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.creating_todo = false;
                            cx.notify();
                        })),
                    )
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
                            // Slack's sidebar row and page cover Saved & To-dos.
                            .child("Saved & To-dos"),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("later-new-todo")
                                    .primary()
                                    .with_size(Size::Small)
                                    .icon(IconName::Plus)
                                    .label("New to-do")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.creating_todo = !this.creating_todo;
                                        if this.creating_todo && this.todo_input.is_none() {
                                            let input = cx.new(|cx| {
                                                TextareaState::new(window, cx)
                                                    .placeholder("What do you need to get done?")
                                                    .auto_grow(1, 2)
                                            });
                                            input.read(cx).focus_handle(cx).focus(window, cx);
                                            this.todo_input = Some(input);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .when(
                                tab == LaterState::Completed
                                    && self.count(LaterState::Completed) > 0,
                                |row| {
                                    row.child(
                                        Button::new("later-clear-completed")
                                            .ghost()
                                            .with_size(Size::Small)
                                            .label("Clear completed")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.clear_completed(cx);
                                            })),
                                    )
                                },
                            )
                            .child(
                                Button::new("later-popout")
                                    .ghost()
                                    .with_size(Size::Small)
                                    .icon(IconName::ExternalLink)
                                    .tooltip("Open Saved in new window")
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(LaterEvent::PopOut)),
                                    ),
                            )
                            .child(
                                Button::new("later-close")
                                    .ghost()
                                    .with_size(Size::Small)
                                    .icon(IconName::X)
                                    .tooltip("Close")
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(LaterEvent::Close)),
                                    ),
                            ),
                    ),
            )
            .child(tab_row)
            .children(todo_composer)
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui;

    #[test]
    fn remind_presets_compute_expected_due_timestamps() {
        let base = 10 * DAY + 14 * HOUR;
        assert_eq!(Remind::InTwentyMinutes.at(base), base + 20 * MINUTE);
        assert_eq!(Remind::InOneHour.at(base), base + HOUR);
        assert_eq!(Remind::InThreeHours.at(base), base + 3 * HOUR);

        let tomorrow = Remind::Tomorrow.at(base);
        let next_week = Remind::NextWeek.at(base);
        let t_off = crate::messages::local_utc_offset(tomorrow as i64);
        let w_off = crate::messages::local_utc_offset(next_week as i64);
        assert_eq!(
            (tomorrow as i64 + t_off).rem_euclid(DAY as i64),
            9 * HOUR as i64
        );
        assert_eq!(
            (next_week as i64 + w_off).rem_euclid(DAY as i64),
            9 * HOUR as i64
        );
        let base_day =
            (base as i64 + crate::messages::local_utc_offset(base as i64)).div_euclid(DAY as i64);
        assert_eq!(
            (tomorrow as i64 + t_off).div_euclid(DAY as i64),
            base_day + 1
        );
        assert_eq!(
            (next_week as i64 + w_off).div_euclid(DAY as i64),
            base_day + 7
        );
    }

    #[gpui::test]
    fn later_view_save_due_state_transitions_and_live_refresh(cx: &mut gpui::TestAppContext) {
        let view = cx.update(LaterView::new);
        let ch = ChannelId::from("C1");
        let ts1 = Ts::from("100.0");
        let ts2 = Ts::from("200.0");

        // Save two items in workspace 0 and one in workspace 1.
        view.update(cx, |v, cx| {
            v.save(
                SaveRequest {
                    channel: ch.clone(),
                    ts: ts1.clone(),
                    author: "Ada".into(),
                    text: "Ship it".into(),
                    saved: true,
                    due: Some(1_000),
                },
                0,
                "# general".into(),
                cx,
            );
            v.save(
                SaveRequest {
                    channel: ch.clone(),
                    ts: ts2.clone(),
                    author: "Lin".into(),
                    text: "Review PR".into(),
                    saved: true,
                    due: Some(2_000),
                },
                0,
                "# general".into(),
                cx,
            );
            v.save(
                SaveRequest {
                    channel: ch.clone(),
                    ts: ts1.clone(),
                    author: "Other".into(),
                    text: "WS 1 item".into(),
                    saved: true,
                    due: None,
                },
                1,
                "# ops".into(),
                cx,
            );
        });

        view.read_with(cx, |v, _| {
            assert_eq!(v.in_progress_count(), 2);
            assert_eq!(v.saved_in(0).len(), 2);
            assert_eq!(v.saved_in(1).len(), 1);
        });

        // `take_due` fires ts1 at t=1500 once and never double-fires it.
        let due = view.update(cx, |v, cx| v.take_due(1_500, cx));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].ts, ts1);
        let due_again = view.update(cx, |v, cx| v.take_due(1_500, cx));
        assert!(due_again.is_empty());

        // `replace_live` preserves `fired` for ts1 while replacing workspace 0 items.
        view.update(cx, |v, cx| {
            v.replace_live(
                0,
                vec![LaterItem {
                    workspace: 0,
                    channel: ch.clone(),
                    ts: ts1.clone(),
                    label: "# general".into(),
                    author: "Ada".into(),
                    text: "Ship it".into(),
                    state: LaterState::InProgress,
                    due: Some(u64::MAX),
                    fired: false,
                    is_todo: false,
                }],
                cx,
            );
        });
        view.read_with(cx, |v, _| {
            assert_eq!(v.in_progress_count(), 1);
            assert_eq!(v.saved_in(1).len(), 1);
            let item = v
                .items
                .iter()
                .find(|i| i.workspace == 0 && i.ts == ts1)
                .unwrap();
            assert!(item.fired);
        });

        // Unsaving removes the item from workspace 0 only.
        view.update(cx, |v, cx| {
            v.save(
                SaveRequest {
                    channel: ch.clone(),
                    ts: ts1.clone(),
                    author: String::new(),
                    text: String::new(),
                    saved: false,
                    due: None,
                },
                0,
                String::new(),
                cx,
            );
        });
        view.read_with(cx, |v, _| {
            assert_eq!(v.in_progress_count(), 0);
            assert_eq!(v.saved_in(1).len(), 1);
        });
    }

    /// Row actions address their item by identity: while a reminder menu is open another save
    /// lands in front of it, so the position it captured is another item's.
    #[gpui::test]
    fn row_actions_edit_the_item_they_were_opened_on(cx: &mut gpui::TestAppContext) {
        let view = cx.update(LaterView::new);
        let (c1, c2) = (ChannelId::from("C1"), ChannelId::from("C2"));
        let (ts1, ts2) = (Ts::from("100.0"), Ts::from("200.0"));
        let save = |channel: &ChannelId, ts: &Ts, text: &str| SaveRequest {
            channel: channel.clone(),
            ts: ts.clone(),
            author: "Ada".into(),
            text: text.into(),
            saved: true,
            due: None,
        };
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = events.clone();
        let recorder = cx.new(|cx| {
            struct Recorder {
                _sub: gpui::Subscription,
            }
            let subscription =
                cx.subscribe(&view, move |_, _, event: &LaterEvent, _| match event {
                    LaterEvent::Update {
                        workspace,
                        channel,
                        ts,
                        mark,
                        date_due,
                    } => seen.borrow_mut().push(format!(
                        "update {workspace} {channel} {} {mark:?} {date_due:?}",
                        ts.as_str()
                    )),
                    LaterEvent::Unsaved {
                        workspace,
                        channel,
                        ts,
                    } => seen
                        .borrow_mut()
                        .push(format!("unsaved {workspace} {channel} {}", ts.as_str())),
                    _ => {}
                });
            Recorder { _sub: subscription }
        });
        view.update(cx, |v, cx| {
            v.save(save(&c1, &ts1, "A"), 0, "# general".into(), cx);
            // A's row captured its identity; a newer save then lands in front of it.
            let a = v.items[0].key();
            v.save(save(&c2, &ts2, "B"), 0, "# ops".into(), cx);
            v.set_due(&a, Some(1_000), cx);
            v.set_state(&a, LaterState::Archived, cx);
            v.remove(&a, cx);
            // An item that is gone is a no-op, not an edit of whatever took its place.
            v.set_due(&a, None, cx);
        });
        assert_eq!(
            *events.borrow(),
            [
                "update 0 C1 100.0 None Some(Some(1000))",
                "update 0 C1 100.0 Some(Archived) None",
                "unsaved 0 C1 100.0",
            ],
            "every action lands on A, the item it was opened on"
        );
        view.read_with(cx, |v, _| {
            assert_eq!(
                v.items
                    .iter()
                    .map(|item| item.ts.clone())
                    .collect::<Vec<_>>(),
                vec![ts2],
                "B stays, untouched"
            );
            assert!(v.items[0].state == LaterState::InProgress);
            assert_eq!(v.items[0].due, None);
        });
        drop(recorder);
    }

    #[test]
    fn remind_tomorrow_and_next_week_schedule_at_local_9am() {
        struct ResetTz;
        impl Drop for ResetTz {
            fn drop(&mut self) {
                crate::messages::set_timezone_override(None);
            }
        }
        let _reset = ResetTz;

        // 2026-06-15 02:00:00 UTC
        let base_utc =
            (slackless_core::types::time::days_from_civil(2026, 6, 15) * 86_400 + 2 * 3_600) as u64;

        // In Stockholm (CEST, UTC+02:00), 02:00 UTC is 04:00 local on June 15.
        // Tomorrow 09:00 local = June 16 07:00 UTC; NextWeek = June 22 07:00 UTC.
        crate::messages::set_timezone_override(Some("Europe/Stockholm"));
        let expected_tomorrow_cest =
            (slackless_core::types::time::days_from_civil(2026, 6, 16) * 86_400 + 7 * 3_600) as u64;
        let expected_next_week_cest =
            (slackless_core::types::time::days_from_civil(2026, 6, 22) * 86_400 + 7 * 3_600) as u64;
        assert_eq!(Remind::Tomorrow.at(base_utc), expected_tomorrow_cest);
        assert_eq!(Remind::NextWeek.at(base_utc), expected_next_week_cest);

        // In Los Angeles (PDT, UTC-07:00), 02:00 UTC on June 15 is 19:00 local on June 14.
        // Tomorrow 09:00 local = June 15 09:00 PDT = June 15 16:00 UTC.
        crate::messages::set_timezone_override(Some("America/Los_Angeles"));
        let expected_tomorrow_pdt = (slackless_core::types::time::days_from_civil(2026, 6, 15)
            * 86_400
            + 16 * 3_600) as u64;
        assert_eq!(Remind::Tomorrow.at(base_utc), expected_tomorrow_pdt);
    }
}
