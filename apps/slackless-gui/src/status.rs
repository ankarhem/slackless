//! The signed-in user's status, presence and Do Not Disturb: the [`StatusPicker`] dialog
//! (emoji + text + "remove after" + suggested statuses), the [`UserMenu`] dropped from the
//! title-bar avatar (status, away/active, pause notifications, schedule, profile,
//! preferences), and [`self_avatar`], the avatar with its presence / DND badge.
//!
//! State lives in the root as a [`SelfState`] and is pushed down with `set_state`; the views
//! only emit what the user asked for. Fixture mode keeps everything in memory; live mode will
//! route these through `users.profile.set`, `users.setPresence` and `dnd.setSnooze`.
//!
//! `StatusPicker::new(&mut Window, &mut App) -> Entity<StatusPicker>`
//! `StatusPicker::edit(&mut self, Option<&UserStatus>, &mut Window, &mut Context<Self>)`
//! emits `StatusEvent::Save(Option<UserStatus>)` / `StatusEvent::Cancel`
//! `UserMenu::new(Arc<Fixtures>, &mut App) -> Entity<UserMenu>`
//! `UserMenu::set_state(&mut self, SelfState, schedule: String, &mut Context<Self>)`
//! emits `UserMenuEvent`

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Div, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Stateful,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};

use crate::avatars::user_chip;
use crate::emoji::glyph as emoji_glyph;
use crate::fixtures::Fixtures;
use crate::messages::{clock, local_offset_now, now_secs};

/// "Remove status after" choices.
pub const CLEAR_AFTER: [&str; 6] = [
    "Don't clear",
    "30 minutes",
    "1 hour",
    "4 hours",
    "Today",
    "This week",
];

/// Slack's default suggested statuses: (emoji shortcode, text, clear-after index).
const SUGGESTED: [(&str, &str, usize); 5] = [
    ("spiral_calendar_pad", "In a meeting", 2),
    ("bus", "Commuting", 1),
    ("face_with_thermometer", "Out sick", 4),
    ("palm_tree", "Vacationing", 0),
    ("house_with_garden", "Working remotely", 4),
];

/// The emoji offered by the picker's emoji button.
const EMOJI_CHOICES: [&str; 12] = [
    "speech_balloon",
    "spiral_calendar_pad",
    "bus",
    "face_with_thermometer",
    "palm_tree",
    "house_with_garden",
    "headphones",
    "computer",
    "sandwich",
    "coffee",
    "rocket",
    "zzz",
];

/// "Pause notifications" choices: (label, what the pause lasts).
pub const PAUSE_OPTIONS: [(&str, PauseLength); 5] = [
    ("30 minutes", PauseLength::Seconds(1_800)),
    ("1 hour", PauseLength::Seconds(3_600)),
    ("2 hours", PauseLength::Seconds(7_200)),
    ("Until tomorrow", PauseLength::Days(1)),
    ("Until next week", PauseLength::Days(7)),
];

#[derive(Clone, Copy)]
pub enum PauseLength {
    Seconds(i64),
    /// Until 9:00 AM local, this many days from today.
    Days(i64),
}

impl PauseLength {
    /// Unix seconds the pause ends at, starting now.
    pub fn until(self) -> i64 {
        let now = now_secs();
        match self {
            PauseLength::Seconds(secs) => now + secs,
            PauseLength::Days(days) => {
                let offset = local_offset_now();
                let local_midnight = (now + offset).div_euclid(86_400) * 86_400;
                local_midnight + days * 86_400 + 9 * 3_600 - offset
            }
        }
    }
}

/// A custom status: emoji shortcode (no colons), text, and when it clears.
#[derive(Clone, PartialEq)]
pub struct UserStatus {
    pub emoji: &'static str,
    pub text: String,
    /// Index into [`CLEAR_AFTER`].
    pub clear_after: usize,
}

impl UserStatus {
    pub fn glyph(&self) -> &'static str {
        emoji_glyph(self.emoji).unwrap_or("\u{1F4AC}")
    }
}

/// The signed-in user's status, presence and notification pause.
#[derive(Clone, Default)]
pub struct SelfState {
    pub status: Option<UserStatus>,
    /// Manually set away (`users.setPresence away`).
    pub away: bool,
    /// Notifications paused until this Unix time (`dnd.setSnooze`).
    pub paused_until: Option<i64>,
}

impl SelfState {
    pub fn paused(&self) -> bool {
        self.paused_until.is_some_and(|until| until > now_secs())
    }

    /// `Notifications paused until 4:30 PM`.
    pub fn pause_label(&self) -> Option<String> {
        let until = self.paused_until.filter(|_| self.paused())?;
        let day = |secs: i64| (secs + local_offset_now()).div_euclid(86_400);
        let when = match day(until) - day(now_secs()) {
            0 => clock(until, None),
            1 => format!("tomorrow, {}", clock(until, None)),
            days => format!("{}, in {days} days", clock(until, None)),
        };
        Some(format!("Notifications paused until {when}"))
    }
}

/// The user's chip with Slack's presence badge: a filled green dot when active, a hollow
/// ring when away, and a `z` badge while notifications are paused.
pub fn self_avatar(fixtures: &Fixtures, state: &SelfState, size: f32, ring: Hsla, cx: &App) -> Div {
    let theme = cx.theme();
    let dot = (size * 0.45).clamp(9., 14.);
    let inner = if state.paused() {
        div()
            .size_full()
            .rounded_full()
            .bg(theme.muted_foreground)
            .flex()
            .items_center()
            .justify_center()
            .text_color(ring)
            .text_size(px(dot - 4.))
            .font_weight(FontWeight::BOLD)
            .child("z")
    } else if state.away {
        div()
            .size_full()
            .rounded_full()
            .border_1()
            .border_color(theme.muted_foreground)
    } else {
        div().size_full().rounded_full().bg(theme.success)
    };
    div()
        .relative()
        .flex_none()
        .child(user_chip(fixtures, &fixtures.self_user, size))
        .child(
            div()
                .absolute()
                .bottom(px(-2.))
                .right(px(-2.))
                .size(px(dot))
                .p(px(if state.paused() { 1. } else { 2. }))
                .rounded_full()
                .bg(ring)
                .child(inner),
        )
}

pub enum StatusEvent {
    /// The status to set; `None` clears it.
    Save(Option<UserStatus>),
    Cancel,
}

/// The "Set a status" dialog body.
pub struct StatusPicker {
    emoji: &'static str,
    text: Entity<TextareaState>,
    has_text: bool,
    clear_after: usize,
    emoji_open: bool,
    clear_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<StatusEvent> for StatusPicker {}

impl StatusPicker {
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<StatusPicker> {
        cx.new(|cx| {
            let text = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("What's your status?")
                    .auto_grow(1, 1)
            });
            let subscriptions = vec![cx.subscribe_in(
                &text,
                window,
                |this: &mut StatusPicker, text, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => {
                        this.has_text = !text.read(cx).value().trim().is_empty();
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.save(cx),
                    _ => {}
                },
            )];
            StatusPicker {
                emoji: EMOJI_CHOICES[0],
                text,
                has_text: false,
                clear_after: 0,
                emoji_open: false,
                clear_open: false,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Load `status` (or a blank draft) before the dialog opens.
    pub fn edit(
        &mut self,
        status: Option<&UserStatus>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (emoji, text, clear_after) = match status {
            Some(status) => (status.emoji, status.text.clone(), status.clear_after),
            None => (EMOJI_CHOICES[0], String::new(), 0),
        };
        self.emoji = emoji;
        self.clear_after = clear_after;
        self.has_text = !text.is_empty();
        self.emoji_open = false;
        self.clear_open = false;
        self.text
            .update(cx, |state, cx| state.set_value(text, window, cx));
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let text = self.text.read(cx).value().trim().to_owned();
        let status = (!text.is_empty()).then_some(UserStatus {
            emoji: self.emoji,
            text,
            clear_after: self.clear_after,
        });
        cx.emit(StatusEvent::Save(status));
    }

    fn pick_suggestion(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (emoji, text, clear_after) = SUGGESTED[ix];
        self.edit(
            Some(&UserStatus {
                emoji,
                text: text.to_owned(),
                clear_after,
            }),
            window,
            cx,
        );
    }
}

impl Render for StatusPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, hover, popover, ring, accent_fg, accent) = (
            theme.input,
            theme.muted_foreground,
            theme.accent,
            theme.popover,
            theme.ring,
            theme.sidebar_accent_foreground,
            theme.sidebar_accent,
        );
        let glyph = emoji_glyph(self.emoji).unwrap_or("\u{1F4AC}");

        let field = h_flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(if self.has_text { ring } else { border })
            .child(
                div()
                    .id("status-emoji")
                    .size(px(28.))
                    .flex_none()
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|el| el.bg(hover))
                    .child(glyph)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.emoji_open = !this.emoji_open;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Textarea::new(&self.text).appearance(false).bordered(false)),
            )
            .when(self.has_text, |row| {
                row.child(
                    div()
                        .id("status-clear")
                        .size(px(24.))
                        .flex_none()
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|el| el.bg(hover))
                        .child(Icon::new(IconName::X).text_color(muted).size(px(14.)))
                        .on_click(cx.listener(|this, _, window, cx| this.edit(None, window, cx))),
                )
            });

        let emoji_grid = self.emoji_open.then(|| {
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(popover)
                .children(EMOJI_CHOICES.iter().enumerate().map(|(ix, &code)| {
                    div()
                        .id(("status-emoji-choice", ix))
                        .size(px(32.))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_lg()
                        .hover(|el| el.bg(hover))
                        .child(emoji_glyph(code).unwrap_or("?"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.emoji = code;
                            this.emoji_open = false;
                            cx.notify();
                        }))
                }))
        });

        let suggestions = (!self.has_text).then(|| {
            v_flex()
                .gap_0p5()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted)
                        .pb_1()
                        .child("For Acme Corp"),
                )
                .children(
                    SUGGESTED
                        .iter()
                        .enumerate()
                        .map(|(ix, (code, text, clear))| {
                            h_flex()
                                .id(("status-suggestion", ix))
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .text_sm()
                                .hover(|el| el.bg(hover))
                                .child(emoji_glyph(code).unwrap_or("?"))
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(*text))
                                .child(
                                    div()
                                        .text_color(muted)
                                        .child(format!("\u{2013} {}", CLEAR_AFTER[*clear])),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.pick_suggestion(ix, window, cx)
                                }))
                        }),
                )
        });

        let clear_select = v_flex()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Remove status after\u{2026}"),
            )
            .child(
                h_flex()
                    .id("status-clear-after")
                    .w(px(240.))
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .cursor_pointer()
                    .text_sm()
                    .child(CLEAR_AFTER[self.clear_after])
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .text_color(muted)
                            .size(px(14.)),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.clear_open = !this.clear_open;
                        cx.notify();
                    })),
            )
            .when(self.clear_open, |col| {
                col.child(
                    v_flex()
                        .w(px(240.))
                        .p_1()
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .bg(popover)
                        .children(CLEAR_AFTER.iter().enumerate().map(|(ix, label)| {
                            let on = ix == self.clear_after;
                            div()
                                .id(("status-clear-option", ix))
                                .px_2()
                                .py_1()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_sm()
                                .when(on, |row| row.bg(accent).text_color(accent_fg))
                                .when(!on, |row| row.hover(|el| el.bg(hover)))
                                .child(*label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.clear_after = ix;
                                    this.clear_open = false;
                                    cx.notify();
                                }))
                        })),
                )
            });

        let footer = h_flex()
            .justify_end()
            .gap_2()
            .pt_2()
            .child(
                Button::new("status-cancel")
                    .ghost()
                    .label("Cancel")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(StatusEvent::Cancel))),
            )
            .child(
                Button::new("status-save")
                    .primary()
                    .label("Save")
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            );

        v_flex()
            .w_full()
            .gap_3()
            .child(field)
            .children(emoji_grid)
            .children(suggestions)
            .child(clear_select)
            .child(footer)
    }
}

pub enum UserMenuEvent {
    EditStatus,
    ClearStatus,
    SetAway(bool),
    /// Pause notifications until this Unix time; `None` resumes them.
    Pause(Option<i64>),
    OpenProfile,
    OpenPreferences,
    /// Preferences > Notifications, where the schedule lives.
    OpenSchedule,
}

/// The menu under the title-bar avatar.
pub struct UserMenu {
    fixtures: Arc<Fixtures>,
    state: SelfState,
    schedule: String,
    pause_open: bool,
}

impl EventEmitter<UserMenuEvent> for UserMenu {}

impl UserMenu {
    pub fn new(fixtures: Arc<Fixtures>, cx: &mut App) -> Entity<UserMenu> {
        cx.new(|_| UserMenu {
            fixtures,
            state: SelfState::default(),
            schedule: String::new(),
            pause_open: false,
        })
    }

    pub fn set_state(&mut self, state: SelfState, schedule: String, cx: &mut Context<Self>) {
        self.state = state;
        self.schedule = schedule;
        cx.notify();
    }
}

/// A plain menu row.
fn menu_row(id: &'static str, label: impl Into<String>, hover: Hsla) -> Stateful<Div> {
    h_flex()
        .id(id)
        .items_center()
        .gap_2()
        .px_4()
        .py_1()
        .cursor_pointer()
        .text_sm()
        .hover(|el| el.bg(hover))
        .child(label.into())
}

impl Render for UserMenu {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (popover, border, muted, hover, success, foreground) = (
            theme.popover,
            theme.border,
            theme.muted_foreground,
            theme.accent,
            theme.success,
            theme.foreground,
        );
        let state = self.state.clone();
        let name = self.fixtures.user_name(&self.fixtures.self_user).to_owned();
        let avatar = self_avatar(&self.fixtures, &state, 36., popover, cx);
        let presence = if state.away { "Away" } else { "Active" };

        let header = h_flex()
            .items_center()
            .gap_3()
            .px_4()
            .pt_3()
            .pb_2()
            .child(avatar)
            .child(
                v_flex()
                    .child(div().font_weight(FontWeight::BOLD).child(name))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(muted)
                            .child(
                                div()
                                    .size(px(8.))
                                    .rounded_full()
                                    .when(!state.away, |dot| dot.bg(success))
                                    .when(state.away, |dot| dot.border_1().border_color(muted)),
                            )
                            .child(presence),
                    ),
            );

        let status_box = h_flex()
            .id("menu-status")
            .mx_4()
            .my_1()
            .items_center()
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .text_sm()
            .hover(|el| el.bg(hover))
            .map(|row| match &state.status {
                Some(status) => row
                    .child(status.glyph())
                    .child(div().flex_1().truncate().child(status.text.clone()))
                    .child(
                        div()
                            .id("menu-status-clear")
                            .size(px(20.))
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(|el| el.bg(border))
                            .child(Icon::new(IconName::X).text_color(muted).size(px(12.)))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.stop_propagation();
                                cx.emit(UserMenuEvent::ClearStatus);
                            })),
                    ),
                None => row
                    .text_color(muted)
                    .child("\u{1F4AC}")
                    .child("Update your status"),
            })
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::EditStatus)));

        let away = state.away;
        let away_row = menu_row(
            "menu-away",
            if away {
                "Set yourself as active"
            } else {
                "Set yourself as away"
            },
            hover,
        )
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(UserMenuEvent::SetAway(!away))));

        let pause_label = state.pause_label();
        let pause_row = menu_row("menu-pause", "Pause notifications", hover)
            .justify_between()
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(muted)
                    .child(if state.paused() { "On" } else { "Off" })
                    .child(
                        Icon::new(if self.pause_open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .text_color(muted)
                        .size(px(14.)),
                    ),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.pause_open = !this.pause_open;
                cx.notify();
            }));
        let pause_status = pause_label.map(|label| {
            h_flex()
                .items_center()
                .gap_2()
                .px_4()
                .pb_1()
                .text_xs()
                .text_color(muted)
                .child(
                    Icon::new(IconName::MoonStar)
                        .text_color(muted)
                        .size(px(12.)),
                )
                .child(label)
        });
        let pause_options = self.pause_open.then(|| {
            v_flex()
                .pl_4()
                .when(state.paused(), |col| {
                    col.child(
                        menu_row("menu-resume", "Resume notifications", hover)
                            .text_color(foreground)
                            .font_weight(FontWeight::SEMIBOLD)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pause_open = false;
                                cx.emit(UserMenuEvent::Pause(None));
                            })),
                    )
                })
                .children(
                    PAUSE_OPTIONS
                        .iter()
                        .enumerate()
                        .map(|(ix, (label, length))| {
                            let length = *length;
                            h_flex()
                                .id(("menu-pause-option", ix))
                                .px_4()
                                .py_1()
                                .cursor_pointer()
                                .text_sm()
                                .hover(|el| el.bg(hover))
                                .child(*label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.pause_open = false;
                                    cx.emit(UserMenuEvent::Pause(Some(length.until())));
                                }))
                        }),
                )
                .child(
                    v_flex()
                        .id("menu-schedule")
                        .px_4()
                        .py_1()
                        .cursor_pointer()
                        .hover(|el| el.bg(hover))
                        .child(div().text_sm().child("Set a notification schedule"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(self.schedule.clone()),
                        )
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenSchedule))),
                )
        });

        let separator = || div().my_1().h(px(1.)).w_full().bg(border);

        v_flex()
            .w(px(300.))
            .py_1()
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(popover)
            .shadow_lg()
            .child(header)
            .child(status_box)
            .child(separator())
            .child(away_row)
            .child(pause_row)
            .children(pause_status)
            .children(pause_options)
            .child(separator())
            .child(
                menu_row("menu-profile", "Profile", hover)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenProfile))),
            )
            .child(
                menu_row("menu-prefs", "Preferences", hover)
                    .justify_between()
                    .child(div().text_xs().text_color(muted).child("\u{2318},"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenPreferences))),
            )
    }
}
