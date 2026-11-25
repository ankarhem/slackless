//! The signed-in user's status, presence and Do Not Disturb: the [`StatusPicker`] dialog
//! (Slack's two steps: pick a suggestion or type, then duration + pause), the [`UserMenu`] dropped from the
//! title-bar avatar (Slack's profile menu: status, away/active, a Notifications hover
//! submenu with the pause presets, profile, preferences, account settings, downloads and
//! sign out), the [`PausePicker`] custom pause dialog, and [`self_avatar`], the avatar with
//! its presence / DND badge.
//!
//! State lives in the root as a [`SelfState`] and is pushed down with `set_state`; the views
//! only emit what the user asked for. Fixture mode keeps everything in memory; live mode will
//! route these through `users.profile.set`, `users.setPresence` and `dnd.setSnooze`.
//!
//! `StatusPicker::new(&mut Window, &mut App) -> Entity<StatusPicker>`
//! `StatusPicker::edit(&mut self, Option<&UserStatus>, workspace: SharedString, &mut Window, &mut Context<Self>)`
//! emits `StatusEvent::Save(Option<UserStatus>)` / `StatusEvent::Cancel`
//! `UserMenu::new(Arc<Fixtures>, &mut App) -> Entity<UserMenu>`
//! `UserMenu::set_state(&mut self, SelfState, schedule: String, &mut Context<Self>)`
//! emits `UserMenuEvent`

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Div, ElementId, Entity, EventEmitter, Focusable as _,
    FontWeight, Hsla, InteractiveElement as _, InteractiveText, IntoElement, ParentElement as _,
    Render, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, StyledText,
    Subscription, TextRun, Window, div, px,
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
    pub emoji: String,
    pub text: String,
    /// Index into [`CLEAR_AFTER`].
    pub clear_after: usize,
    /// Slack's "Pause notifications until status is cleared".
    pub pause: bool,
    /// Concrete unix expiration timestamp (seconds), when set or synced from Slack.
    pub expiration: Option<i64>,
}

impl UserStatus {
    pub fn glyph(&self) -> &'static str {
        emoji_glyph(self.emoji.as_str()).unwrap_or("\u{1F4AC}")
    }

    /// Hydrate a [`UserStatus`] from a Slack [`Profile`], dropping expired or empty ones.
    pub fn from_profile(profile: &slackless_core::types::Profile) -> Option<Self> {
        let text = profile.status_text.as_deref().unwrap_or_default().trim();
        let emoji = profile
            .status_emoji
            .as_deref()
            .unwrap_or_default()
            .trim()
            .trim_matches(':');
        if text.is_empty() && emoji.is_empty() {
            return None;
        }
        let expiration = profile.status_expiration.filter(|&ts| ts > 0);
        if expiration.is_some_and(|ts| ts <= now_secs()) {
            return None;
        }
        Some(Self {
            emoji: if emoji.is_empty() {
                EMOJI_CHOICES[0].to_owned()
            } else {
                emoji.to_owned()
            },
            text: text.to_owned(),
            clear_after: 0,
            pause: false,
            expiration,
        })
    }

    /// Whether this status's concrete expiration has passed at `now` (Unix seconds).
    pub fn is_expired_at(&self, now: i64) -> bool {
        self.expiration.is_some_and(|exp| exp <= now)
    }

    /// Whether this status has expired right now.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_expired(&self) -> bool {
        self.is_expired_at(now_secs())
    }

    /// Human-readable time remaining until expiration (`"45s"`, `"12m"`, `"2h 15m"`, `"3d 4h"`),
    /// or `None` when the status has no expiration or has already expired.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn format_remaining(&self, now: i64) -> Option<String> {
        let rem = self.expiration? - now;
        if rem <= 0 {
            return None;
        }
        let days = rem / 86_400;
        let hours = (rem % 86_400) / 3_600;
        let mins = (rem % 3_600) / 60;
        let secs = rem % 60;
        Some(if days > 0 {
            if hours > 0 {
                format!("{days}d {hours}h")
            } else {
                format!("{days}d")
            }
        } else if hours > 0 {
            if mins > 0 {
                format!("{hours}h {mins}m")
            } else {
                format!("{hours}h")
            }
        } else if mins > 0 {
            format!("{mins}m")
        } else {
            format!("{secs}s")
        })
    }

    /// Pure version of [`Self::clears_at`] against explicit `now` and UTC `offset` (seconds).
    pub fn clears_at_from(&self, now: i64, offset: i64) -> Option<i64> {
        if let Some(exp) = self.expiration {
            return Some(exp);
        }
        let day = (now + offset).div_euclid(86_400);
        let midnight = day * 86_400 - offset;
        match self.clear_after {
            1 => Some(now + 1_800),
            2 => Some(now + 3_600),
            3 => Some(now + 14_400),
            4 => Some(midnight + 86_400),
            // 1970-01-01 was a Thursday: (day + 3) % 7 counts days since Monday.
            5 => Some(midnight + (7 - (day + 3).rem_euclid(7)) * 86_400),
            _ => None,
        }
    }

    /// When a status set now clears (Unix seconds); `None` for "Don't clear". "Today" ends
    /// at local midnight, "This week" at the local midnight starting next Monday.
    pub fn clears_at(&self) -> Option<i64> {
        self.clears_at_from(now_secs(), local_offset_now())
    }
}

/// A future status scheduled to activate automatically at `starts_at` (`F-SHELL-018`, `F-SHELL-019`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ScheduledStatus {
    pub emoji: String,
    pub text: String,
    pub starts_at: i64,
    pub clear_after: usize,
    pub pause: bool,
}

/// The signed-in user's status, presence and notification pause.
#[derive(Clone, Default)]
pub struct SelfState {
    pub status: Option<UserStatus>,
    /// Manually set away (`users.setPresence away`).
    pub away: bool,
    /// Notifications paused until this Unix time (`dnd.setSnooze`).
    pub paused_until: Option<i64>,
    /// Slack's focus mode is on until this Unix time (`i64::MAX`: "Don't turn off").
    pub focus_until: Option<i64>,
    /// The focus Duration in effect, an index into [`FOCUS_DURATIONS`].
    pub focus_preset: usize,
    /// Pending scheduled statuses that activate automatically when `starts_at <= now` (`F-SHELL-018`).
    pub scheduled_statuses: Vec<ScheduledStatus>,
}

/// Focus mode's Duration choices: label and length in seconds (`None`: "Don't turn off").
pub const FOCUS_DURATIONS: [(&str, Option<i64>); 4] = [
    ("30 minutes", Some(1_800)),
    ("1 hour", Some(3_600)),
    ("2 hours", Some(7_200)),
    ("Don\u{2019}t turn off", None),
];

/// The Duration focus mode starts with, as Slack's does: "Don't turn off".
pub const FOCUS_UNTIL_OFF: usize = 3;

/// The status focus mode shows while no other is set: Slack's automatic "In focus mode"
/// with the telescope.
fn focus_status() -> UserStatus {
    UserStatus {
        emoji: "telescope".to_owned(),
        text: "In focus mode".to_owned(),
        clear_after: 0,
        pause: false,
        expiration: None,
    }
}

impl SelfState {
    pub fn paused(&self) -> bool {
        self.paused_until.is_some_and(|until| until > now_secs())
    }

    pub fn focused(&self) -> bool {
        self.focus_until.is_some_and(|until| until > now_secs())
    }

    /// Turn focus mode on with the Duration [`FOCUS_DURATIONS`]`[preset]`, or re-time it. Like
    /// Slack it sets the telescope's "In focus mode" status unless a status is already set.
    #[allow(dead_code)]
    pub fn start_focus(&mut self, preset: usize) {
        self.start_focus_with_status(preset, true);
    }

    /// Turn focus mode on with `preset`, optionally setting the automatic `"In focus mode"`
    /// status when `auto_status` is true (`F-SHELL-037`, `F-SHELL-039`).
    pub fn start_focus_with_status(&mut self, preset: usize, auto_status: bool) {
        let duration = FOCUS_DURATIONS[preset].1;
        self.focus_preset = preset;
        self.focus_until = Some(duration.map_or(i64::MAX, |secs| now_secs() + secs));
        if auto_status && self.status.is_none() {
            self.status = Some(focus_status());
        }
    }

    /// Pop and activate the earliest due scheduled status whose `starts_at <= now` using
    /// explicit `now` and UTC `offset` (`F-SHELL-018`).
    pub fn activate_due_scheduled_at(&mut self, now: i64, offset: i64) -> Option<UserStatus> {
        let ix = self
            .scheduled_statuses
            .iter()
            .enumerate()
            .filter(|(_, s)| s.starts_at <= now)
            .min_by_key(|(_, s)| s.starts_at)
            .map(|(ix, _)| ix)?;
        let sched = self.scheduled_statuses.remove(ix);
        let mut status = UserStatus {
            emoji: sched.emoji,
            text: sched.text,
            clear_after: sched.clear_after,
            pause: sched.pause,
            expiration: None,
        };
        status.expiration = status.clears_at_from(now, offset);
        self.set_status(Some(status.clone()));
        Some(status)
    }

    /// Pop and activate the earliest due scheduled status whose `starts_at <= now`, if any (`F-SHELL-018`).
    pub fn activate_due_scheduled(&mut self) -> Option<UserStatus> {
        self.activate_due_scheduled_at(now_secs(), local_offset_now())
    }

    /// Turn focus mode off, clearing its automatic status (a status the user set stays).
    pub fn end_focus(&mut self) {
        self.focus_until = None;
        if self.status == Some(focus_status()) {
            self.status = None;
        }
    }

    /// The focus card's subtitle: `Notifications paused` or `Notifications paused until 4:30 PM`.
    pub fn focus_label(&self) -> String {
        match self.focus_until.filter(|until| *until != i64::MAX) {
            Some(until) => format!("Notifications paused until {}", clock(until, None)),
            None => "Notifications paused".to_owned(),
        }
    }

    /// Set (or with `None` clear) the custom status. A status saved with "Pause notifications
    /// until status is cleared" pauses until it clears (indefinitely for "Don't clear"), and
    /// replacing or clearing such a status resumes notifications.
    pub fn set_status(&mut self, mut status: Option<UserStatus>) {
        if let Some(new) = status.as_mut()
            && new.expiration.is_none()
        {
            new.expiration = new.clears_at();
        }
        if let Some(new) = status.as_ref().filter(|status| status.pause) {
            self.paused_until = Some(new.clears_at().unwrap_or(i64::MAX));
        } else if self.status.as_ref().is_some_and(|old| old.pause) {
            self.paused_until = None;
        }
        self.status = status;
    }

    /// Clear the custom status if its expiration timestamp has passed at `now`. Returns `true` if cleared.
    pub fn expire_status_at(&mut self, now: i64) -> bool {
        let expired = self.status.as_ref().is_some_and(|s| s.is_expired_at(now));
        if expired {
            self.set_status(None);
            true
        } else {
            false
        }
    }

    /// Clear the custom status if its expiration timestamp has passed. Returns `true` if cleared.
    pub fn expire_status(&mut self) -> bool {
        self.expire_status_at(now_secs())
    }

    /// Pure version of [`Self::pause_label`] against explicit `now` and UTC `offset`.
    pub fn pause_label_at(&self, now: i64, offset: i64) -> Option<String> {
        let until = self.paused_until.filter(|&u| u > now)?;
        if until == i64::MAX {
            return Some("Notifications paused".to_owned());
        }
        let day = |secs: i64| (secs + offset).div_euclid(86_400);
        let when = match day(until) - day(now) {
            0 => clock(until, Some(offset)),
            1 => format!("tomorrow, {}", clock(until, Some(offset))),
            days => format!("{}, in {days} days", clock(until, Some(offset))),
        };
        Some(format!("Notifications paused until {when}"))
    }

    /// `Notifications paused until 4:30 PM`, or just `Notifications paused` while paused
    /// until a status is cleared.
    pub fn pause_label(&self) -> Option<String> {
        self.pause_label_at(now_secs(), local_offset_now())
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
    /// Schedule a status to activate automatically at a future time (`F-SHELL-018`).
    Schedule(ScheduledStatus),
    /// Remove a pending scheduled status by index (`F-SHELL-019`).
    RemoveScheduled(usize),
    Cancel,
}

/// The "Set a status" dialog body. Like Slack's it has two steps: an empty field over the
/// workspace's suggestions, then (once a suggestion is picked or text typed) the status with
/// its Duration select and the pause checkbox.
pub struct StatusPicker {
    emoji: String,
    /// An emoji was chosen (or loaded): until then the field shows Slack's smiley outline.
    emoji_set: bool,
    text: Entity<TextareaState>,
    has_text: bool,
    clear_after: usize,
    pause: bool,
    /// The workspace the suggestions are for ("For Norce").
    workspace: SharedString,
    /// Workspace-configured status presets (`F-SHELL-015`); falls back to [`SUGGESTED`] when empty.
    presets: Vec<(String, String, usize)>,
    /// Recently saved custom statuses in this session (newest first, max 3).
    recent: Vec<(String, String, usize)>,
    /// Upcoming scheduled statuses shown for management (`F-SHELL-019`).
    scheduled: Vec<ScheduledStatus>,
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
                emoji: EMOJI_CHOICES[0].to_owned(),
                emoji_set: false,
                text,
                has_text: false,
                clear_after: 0,
                pause: false,
                workspace: SharedString::default(),
                presets: Vec::new(),
                recent: Vec::new(),
                scheduled: Vec::new(),
                emoji_open: false,
                clear_open: false,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Install workspace-configured suggested statuses (`team.prefs.custom_status_default_suggestions`).
    pub fn set_presets(&mut self, presets: Vec<(String, String, usize)>, cx: &mut Context<Self>) {
        self.presets = presets;
        cx.notify();
    }

    /// Sync upcoming scheduled statuses so the picker can display and cancel them (`F-SHELL-019`).
    pub fn set_scheduled(&mut self, scheduled: Vec<ScheduledStatus>, cx: &mut Context<Self>) {
        self.scheduled = scheduled;
        cx.notify();
    }

    fn effective_suggestions(&self) -> Vec<(String, String, usize)> {
        if !self.presets.is_empty() {
            self.presets.clone()
        } else {
            SUGGESTED
                .iter()
                .map(|(code, text, clear)| ((*code).to_owned(), (*text).to_owned(), *clear))
                .collect()
        }
    }

    /// Load `status` (or a blank draft) for `workspace` before the dialog opens.
    pub fn edit(
        &mut self,
        status: Option<&UserStatus>,
        workspace: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace = workspace;
        self.load(status, window, cx);
    }

    /// Put the caret in the status field (once the dialog is up).
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.text.read(cx).focus_handle(cx).focus(window, cx);
    }

    fn load(&mut self, status: Option<&UserStatus>, window: &mut Window, cx: &mut Context<Self>) {
        let (emoji, text, clear_after, pause) = match status {
            Some(status) => (
                status.emoji.clone(),
                status.text.clone(),
                status.clear_after,
                status.pause,
            ),
            None => (EMOJI_CHOICES[0].to_owned(), String::new(), 0, false),
        };
        self.emoji = emoji;
        self.emoji_set = status.is_some();
        self.clear_after = clear_after;
        self.pause = pause;
        self.has_text = !text.is_empty();
        self.emoji_open = false;
        self.clear_open = false;
        self.text
            .update(cx, |state, cx| state.set_value(text, window, cx));
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let text = self.text.read(cx).value().trim().to_owned();
        let status = (!text.is_empty()).then(|| {
            let mut s = UserStatus {
                emoji: self.emoji.clone(),
                text: text.clone(),
                clear_after: self.clear_after,
                pause: self.pause,
                expiration: None,
            };
            s.expiration = s.clears_at();
            self.recent.retain(|(_, t, _)| t != &text);
            self.recent
                .insert(0, (self.emoji.clone(), text, self.clear_after));
            self.recent.truncate(3);
            s
        });
        cx.emit(StatusEvent::Save(status));
    }

    fn pick_suggestion(
        &mut self,
        emoji: String,
        text: String,
        clear_after: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load(
            Some(&UserStatus {
                emoji,
                text,
                clear_after,
                pause: false,
                expiration: None,
            }),
            window,
            cx,
        );
    }
}

impl Render for StatusPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.text.read(cx).focus_handle(cx).is_focused(window);
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
        let foreground = theme.foreground;
        let glyph = emoji_glyph(self.emoji.as_str()).unwrap_or("\u{1F4AC}");
        let show_glyph = self.emoji_set || self.has_text;

        let field = h_flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(if focused { ring } else { border })
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
                    .when(show_glyph, |button| button.child(glyph))
                    .when(!show_glyph, |button| {
                        button.child(
                            Icon::new(IconName::FaceSlightlySmiling)
                                .text_color(muted)
                                .size(px(18.)),
                        )
                    })
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
                        .child(
                            Icon::new(IconName::CircleX)
                                .text_color(foreground)
                                .size(px(16.)),
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.load(None, window, cx))),
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
                            this.emoji = code.to_owned();
                            this.emoji_set = true;
                            this.emoji_open = false;
                            cx.notify();
                        }))
                }))
        });

        let suggestions = (!self.has_text).then(|| {
            let workspace_list = self.effective_suggestions();
            let recent_list = self.recent.clone();
            v_flex()
                .gap_0p5()
                .when(!recent_list.is_empty(), |col| {
                    col.child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .pb_1()
                            .child("Recent"),
                    )
                    .children(recent_list.into_iter().enumerate().map(
                        |(ix, (code, text, clear))| {
                            let clear_label =
                                CLEAR_AFTER.get(clear).copied().unwrap_or(CLEAR_AFTER[0]);
                            let (c_click, t_click) = (code.clone(), text.clone());
                            h_flex()
                                .id(("status-recent", ix))
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .text_sm()
                                .hover(|el| el.bg(hover))
                                .child(emoji_glyph(&code).unwrap_or("\u{1F4AC}"))
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(text))
                                .child(
                                    div()
                                        .text_color(muted)
                                        .child(format!("\u{2014} {clear_label}")),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.pick_suggestion(
                                        c_click.clone(),
                                        t_click.clone(),
                                        clear,
                                        window,
                                        cx,
                                    )
                                }))
                        },
                    ))
                    .child(div().h(px(6.)))
                })
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted)
                        .pb_1()
                        .child(format!("For {}", self.workspace)),
                )
                .children(workspace_list.into_iter().enumerate().map(
                    |(ix, (code, text, clear))| {
                        let clear_label = CLEAR_AFTER.get(clear).copied().unwrap_or(CLEAR_AFTER[0]);
                        let (c_click, t_click) = (code.clone(), text.clone());
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
                            .child(emoji_glyph(&code).unwrap_or("\u{1F4AC}"))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(text))
                            .child(
                                div()
                                    .text_color(muted)
                                    .child(format!("\u{2014} {clear_label}")),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.pick_suggestion(
                                    c_click.clone(),
                                    t_click.clone(),
                                    clear,
                                    window,
                                    cx,
                                )
                            }))
                    },
                ))
        });

        // Step two: once there is a status, its Duration and the pause checkbox.
        let this = cx.entity().downgrade();
        let pause = Checkbox::new("status-pause")
            .label("Pause notifications until status is cleared")
            .checked(self.pause)
            .on_click(move |checked, _, cx| {
                this.update(cx, |picker, cx| {
                    picker.pause = *checked;
                    cx.notify();
                })
                .ok();
            });
        let clear_select = v_flex()
            .gap_1p5()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child("Duration"),
            )
            .child(
                h_flex()
                    .id("status-clear-after")
                    .w_full()
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
                        .w_full()
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
                    .outline()
                    .label("Cancel")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(StatusEvent::Cancel))),
            )
            .child(
                Button::new("status-save")
                    .primary()
                    .label("Save")
                    .disabled(!self.has_text)
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            );
        let schedule_row = v_flex()
            .gap_1p5()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child("Or schedule for later"),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child(
                        Button::new("status-sched-1h")
                            .outline()
                            .with_size(gpui_kit::component::Size::XSmall)
                            .label("In 1 hour")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let text = this.text.read(cx).value().trim().to_owned();
                                if !text.is_empty() {
                                    cx.emit(StatusEvent::Schedule(ScheduledStatus {
                                        emoji: this.emoji.clone(),
                                        text,
                                        starts_at: PauseLength::Seconds(3_600).until(),
                                        clear_after: this.clear_after,
                                        pause: this.pause,
                                    }));
                                }
                            })),
                    )
                    .child(
                        Button::new("status-sched-tomorrow")
                            .outline()
                            .with_size(gpui_kit::component::Size::XSmall)
                            .label("Tomorrow 9:00 AM")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let text = this.text.read(cx).value().trim().to_owned();
                                if !text.is_empty() {
                                    cx.emit(StatusEvent::Schedule(ScheduledStatus {
                                        emoji: this.emoji.clone(),
                                        text,
                                        starts_at: PauseLength::Days(1).until(),
                                        clear_after: this.clear_after,
                                        pause: this.pause,
                                    }));
                                }
                            })),
                    )
                    .child(
                        Button::new("status-sched-next-week")
                            .outline()
                            .with_size(gpui_kit::component::Size::XSmall)
                            .label("Next week 9:00 AM")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let text = this.text.read(cx).value().trim().to_owned();
                                if !text.is_empty() {
                                    cx.emit(StatusEvent::Schedule(ScheduledStatus {
                                        emoji: this.emoji.clone(),
                                        text,
                                        starts_at: PauseLength::Days(7).until(),
                                        clear_after: this.clear_after,
                                        pause: this.pause,
                                    }));
                                }
                            })),
                    ),
            );
        let details = self.has_text.then(|| {
            v_flex()
                .gap_3()
                .child(clear_select)
                .child(pause)
                .child(schedule_row)
        });

        let upcoming = (!self.scheduled.is_empty()).then(|| {
            v_flex()
                .gap_1()
                .pt_1()
                .border_t_1()
                .border_color(border)
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted)
                        .child("Scheduled statuses"),
                )
                .children(self.scheduled.iter().enumerate().map(|(ix, sched)| {
                    let glyph = emoji_glyph(&sched.emoji).unwrap_or("\u{1F4AC}");
                    let when = clock(sched.starts_at, None);
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(hover.opacity(0.45))
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1p5()
                                .min_w_0()
                                .child(glyph)
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .truncate()
                                        .child(sched.text.clone()),
                                )
                                .child(
                                    div().text_xs().text_color(muted).child(format!("· {when}")),
                                ),
                        )
                        .child(
                            Button::new(("status-sched-remove", ix))
                                .ghost()
                                .with_size(gpui_kit::component::Size::XSmall)
                                .icon(IconName::X)
                                .tooltip("Cancel scheduled status")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if ix < this.scheduled.len() {
                                        this.scheduled.remove(ix);
                                    }
                                    cx.emit(StatusEvent::RemoveScheduled(ix));
                                    cx.notify();
                                })),
                        )
                }))
        });

        v_flex()
            .w_full()
            .gap_3()
            .child(field)
            .children(emoji_grid)
            .children(suggestions)
            .children(details)
            .children(upcoming)
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
    /// Slack's "Custom…" pause dialog ([`PausePicker`]).
    OpenPauseCustom,
    /// The workspace's account settings at `<domain>.slack.com`, in the browser.
    OpenAccount,
    /// The user's Downloads folder in Finder (slackless has no download manager).
    OpenDownloads,
    /// Forget this workspace's stored session and quit; the next launch signs in.
    SignOut,
}

/// The menu under the title-bar avatar.
pub struct UserMenu {
    fixtures: Arc<Fixtures>,
    state: SelfState,
    schedule: String,
    /// The Notifications row is under the pointer: its submenu is open (Slack opens it on
    /// hover). The submenu keeps it open while the pointer is over it, so the two hover
    /// flags stay independent and order-independent.
    notif_hover: bool,
    sub_hover: bool,
    /// The hovered row id: Slack turns the whole row, hints included, white on its blue
    /// highlight, which plain hover styles cannot express for the right-aligned hints.
    hovered: Option<&'static str>,
    _pause_subscription: Option<Subscription>,
}

impl EventEmitter<UserMenuEvent> for UserMenu {}

impl UserMenu {
    pub fn new(fixtures: Arc<Fixtures>, cx: &mut App) -> Entity<UserMenu> {
        cx.new(|_| UserMenu {
            fixtures,
            state: SelfState::default(),
            schedule: String::new(),
            notif_hover: false,
            sub_hover: false,
            hovered: None,
            _pause_subscription: None,
        })
    }

    pub fn set_state(&mut self, state: SelfState, schedule: String, cx: &mut Context<Self>) {
        self.state = state;
        self.schedule = schedule;
        cx.notify();
    }

    /// The menu was dismissed: drop hover state so the submenu cannot reappear stale.
    pub fn closed(&mut self, cx: &mut Context<Self>) {
        self.notif_hover = false;
        self.sub_hover = false;
        self.hovered = None;
        cx.notify();
    }

    /// Slack's "Custom…" pause dialog: pick a wall-clock time to pause until.
    pub fn open_pause_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picker = PausePicker::new(window, cx);
        self._pause_subscription = Some(cx.subscribe_in(
            &picker,
            window,
            |this, _, event: &PauseEvent, window, cx| {
                window.close_dialog(cx);
                this.notif_hover = false;
                this.sub_hover = false;
                if let PauseEvent::Save(until) = event {
                    cx.emit(UserMenuEvent::Pause(Some(*until)));
                }
            },
        ));
        let picker = picker.clone();
        let first_focus = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            // The dialog shell takes focus when it opens; put the caret in the time field
            // on the builder's first run only (it re-runs on rebuilds).
            if first_focus.replace(false) {
                // Split the reads from the focus call: the shared cx borrows must end first.
                let handle = picker.read(cx).text.read(cx).focus_handle(cx);
                handle.focus(window, cx);
            }
            dialog
                .title("Pause notifications")
                .w(px(360.))
                .child(picker.clone())
        });
    }
}

/// A plain menu row sized like Slack's: 28pt tall, 24pt text inset. `hot` is Slack's
/// state-driven hover: the workspace's selected-row blue with every child turned white.
fn row_base(id: &'static str, hot: bool, blue: Hsla, on_blue: Hsla) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(28.))
        .px_6()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .text_size(px(15.))
        .when(hot, |row| row.bg(blue).text_color(on_blue))
}

/// A row whose children are plain text: hover styles alone flip them white.
fn simple_row(id: impl Into<ElementId>) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(28.))
        .px_6()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .text_size(px(15.))
}

/// Slack's menu metrics (pt): the panel is ~300pt wide, the hover submenu is a ~260pt
/// panel flush to the right edge with its top level with the status box under the header
/// (20pt header top pad + 36pt avatar + 8pt + 8pt margins). Measured off Slack's menu.
const MENU_WIDTH: f32 = 300.;
const SUBMENU_TOP: f32 = 74.;
const SUBMENU_WIDTH: f32 = 260.;

/// Slack's submenu footer; the "in preferences" tail opens Preferences.
const VIP_NOTE: &str = "You can choose to always allow notifications from VIPs ";
const VIP_LINK: &str = "in preferences";

impl Render for UserMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (popover, border, muted, accent, success) = (
            theme.popover,
            theme.border,
            theme.muted_foreground,
            theme.accent,
            theme.success,
        );
        // Slack highlights the hovered row with the workspace's selected-row blue, every
        // hint on the row turned white with it.
        let (blue, on_blue) = (theme.sidebar_accent, theme.sidebar_accent_foreground);
        let link = theme.link;
        let state = self.state.clone();
        let name = self.fixtures.user_name(&self.fixtures.self_user).to_owned();
        // Slack's menu header avatar carries no presence badge; the "● Active" line below
        // the name is the only presence here (the rail avatar keeps its badge).
        let avatar = user_chip(&self.fixtures, &self.fixtures.self_user, 36.);
        let pause_open = self.notif_hover || self.sub_hover;

        let presence = if state.away { "Away" } else { "Active" };
        let header = h_flex()
            .items_center()
            .gap_3()
            .px_6()
            .pt_5()
            .pb_2()
            .child(avatar)
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::BOLD)
                            .line_height(px(20.))
                            .child(name),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                div()
                                    .size(px(8.))
                                    .rounded_full()
                                    .when(!state.away, |dot| dot.bg(success))
                                    .when(state.away, |dot| dot.border_1().border_color(muted)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .line_height(px(16.))
                                    .text_color(muted)
                                    .child(presence),
                            ),
                    ),
            );

        let status_box = h_flex()
            .id("menu-status")
            .mx_6()
            .my_2()
            .h(px(40.))
            .items_center()
            .gap_2()
            .px_2p5()
            .rounded(px(4.))
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .text_size(px(15.))
            .hover(|boxel| boxel.bg(accent))
            .map(|boxel| match &state.status {
                Some(status) => boxel
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
                            .hover(|x| x.bg(border))
                            .child(Icon::new(IconName::X).text_color(muted).size(px(12.)))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.stop_propagation();
                                cx.emit(UserMenuEvent::ClearStatus);
                            })),
                    ),
                None => boxel
                    .text_color(muted)
                    .child(
                        Icon::new(IconName::FaceSlightlySmiling)
                            .text_color(muted)
                            .size(px(16.)),
                    )
                    .child("Update your status"),
            })
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::EditStatus)));

        let away = state.away;
        let away_row = simple_row("menu-away")
            .hover(|row| row.bg(blue).text_color(on_blue))
            .child("Set yourself as")
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .child(if away { "active" } else { "away" }),
            )
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(UserMenuEvent::SetAway(!away))));

        let notif_hot = self.hovered == Some("menu-notifications");
        let hint_color = if notif_hot { on_blue } else { muted };
        let paused = state.paused();
        let pause_label = if paused {
            state
                .paused_until
                .filter(|until| *until != i64::MAX)
                .map(|until| format!("Until {}", clock(until, None)))
                .unwrap_or_else(|| "Paused".to_owned())
        } else {
            "On".to_owned()
        };
        let notifications_row = row_base("menu-notifications", notif_hot, blue, on_blue)
            .justify_between()
            .child("Notifications")
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(hint_color)
                            .child(pause_label),
                    )
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .size(px(14.))
                            .text_color(hint_color),
                    ),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.notif_hover = *hovered;
                this.hovered = hovered.then_some("menu-notifications");
                cx.notify();
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                // Hover opens the submenu; a click pins it for touch input too.
                this.notif_hover = true;
                this.hovered = Some("menu-notifications");
                cx.notify();
            }));

        let prefs_hot = self.hovered == Some("menu-prefs");
        let prefs_row = row_base("menu-prefs", prefs_hot, blue, on_blue)
            .justify_between()
            .child("Preferences")
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(if prefs_hot { on_blue } else { muted })
                    .child("\u{2318},"),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = hovered.then_some("menu-prefs");
                cx.notify();
            }))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenPreferences)));

        let account_hot = self.hovered == Some("menu-account");
        let account_row = row_base("menu-account", account_hot, blue, on_blue)
            .justify_between()
            .child("Account settings")
            .child(
                Icon::new(IconName::ExternalLink)
                    .size(px(14.))
                    .text_color(if account_hot { on_blue } else { muted }),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = hovered.then_some("menu-account");
                cx.notify();
            }))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenAccount)));

        let downloads_hot = self.hovered == Some("menu-downloads");
        let downloads_row = row_base("menu-downloads", downloads_hot, blue, on_blue)
            .justify_between()
            .child("Downloads")
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(if downloads_hot { on_blue } else { muted })
                    .child("\u{2318}\u{21e7}J"),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = hovered.then_some("menu-downloads");
                cx.notify();
            }))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenDownloads)));

        let profile_row = simple_row("menu-profile")
            .hover(|row| row.bg(blue).text_color(on_blue))
            .child("Profile")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenProfile)));

        let signout_row = simple_row("menu-signout")
            .hover(|row| row.bg(blue).text_color(on_blue))
            .child("Sign out")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::SignOut)));

        let separator = || div().my_2().h(px(1.)).w_full().bg(border);

        let font = window.text_style().font();
        let run = |len: usize, color: Hsla| TextRun {
            len,
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let note_text = format!("{VIP_NOTE}{VIP_LINK}.");
        let runs = vec![
            run(VIP_NOTE.len(), muted),
            run(VIP_LINK.len(), link),
            run(1, muted),
        ];
        let this = cx.entity();
        let link_range =
            std::iter::once(VIP_NOTE.len()..VIP_NOTE.len() + VIP_LINK.len()).collect::<Vec<_>>();
        let footer = div().px_6().pt_1().pb_2().text_size(px(12.)).child(
            InteractiveText::new(
                "menu-vip-preferences",
                StyledText::new(note_text).with_runs(runs),
            )
            .on_click(link_range, move |_, _, cx| {
                this.update(cx, |_, cx| cx.emit(UserMenuEvent::OpenPreferences));
            }),
        );

        let resume = paused.then(|| {
            simple_row("menu-resume")
                .hover(|row| row.bg(blue).text_color(on_blue))
                .child("Resume notifications")
                .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::Pause(None))))
        });
        let options = PAUSE_OPTIONS
            .iter()
            .enumerate()
            .map(|(ix, (label, length))| {
                let length = *length;
                simple_row(("menu-pause-option", ix))
                    .hover(|row| row.bg(blue).text_color(on_blue))
                    .child(*label)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(UserMenuEvent::Pause(Some(length.until())));
                    }))
            });
        let custom_row = simple_row("menu-custom")
            .hover(|row| row.bg(blue).text_color(on_blue))
            .child("Custom\u{2026}")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenPauseCustom)));
        let working_hot = self.hovered == Some("menu-working-hours");
        let schedule_row = row_base("menu-working-hours", working_hot, blue, on_blue)
            .child("Edit your working hours")
            // Slack's submenu shows no schedule text; the current schedule stays in the
            // row's accessibility label.
            .aria_label(if self.schedule.is_empty() {
                "Edit your working hours".to_owned()
            } else {
                format!(
                    "Edit your working hours. Notification schedule: {}",
                    self.schedule
                )
            })
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = hovered.then_some("menu-working-hours");
                cx.notify();
            }))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenSchedule)));

        let submenu = pause_open.then(|| {
            v_flex()
                .id("menu-submenu")
                .occlude()
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .absolute()
                // Flush to the menu's right edge (2pt overlap keeps the hover chain intact
                // while the pointer crosses), top level with the status box, as Slack drops
                // it.
                .left(px(MENU_WIDTH - 2.))
                .top(px(SUBMENU_TOP))
                .w(px(SUBMENU_WIDTH))
                .rounded(px(6.))
                .border_1()
                .border_color(border)
                .bg(popover)
                .shadow_lg()
                .pt_3()
                .pb_3()
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    this.sub_hover = *hovered;
                    cx.notify();
                }))
                .child(
                    h_flex()
                        .h(px(28.))
                        .px_6()
                        .items_center()
                        .text_size(px(13.))
                        .text_color(muted)
                        .child("Pause notifications for\u{2026}"),
                )
                .children(resume)
                .children(options)
                .child(custom_row)
                .child(separator())
                .child(schedule_row)
                .child(separator())
                .child(footer)
        });

        v_flex()
            .relative()
            .w(px(MENU_WIDTH))
            .rounded(px(6.))
            .border_1()
            .border_color(border)
            .bg(popover)
            .shadow_lg()
            .pb_4()
            .child(header)
            .child(status_box)
            .child(away_row)
            .child(notifications_row)
            .child(separator())
            .child(profile_row)
            .child(prefs_row)
            .child(account_row)
            .child(separator())
            .child(downloads_row)
            .child(separator())
            .child(signout_row)
            .children(submenu)
    }
}

pub enum PauseEvent {
    /// Pause notifications until this Unix time.
    Save(i64),
    Cancel,
}

/// Slack's "Custom…" pause dialog: a wall-clock time field with a live "Until …" hint.
struct PausePicker {
    text: Entity<TextareaState>,
    /// The parsed pause end (Unix seconds), if the field holds a time.
    until: Option<i64>,
    /// The field holds text that is not a time.
    invalid: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PauseEvent> for PausePicker {}

impl PausePicker {
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<PausePicker> {
        cx.new(|cx| {
            let text = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Until 4:30 PM")
                    .auto_grow(1, 1)
            });
            let subscriptions = vec![cx.subscribe_in(
                &text,
                window,
                |this: &mut PausePicker, text, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => this.parse(&text.read(cx).value(), cx),
                    InputEvent::PressEnter { .. } => this.save(cx),
                    _ => {}
                },
            )];
            PausePicker {
                text,
                until: None,
                invalid: false,
                _subscriptions: subscriptions,
            }
        })
    }

    fn parse(&mut self, value: &str, cx: &mut Context<Self>) {
        self.until = parse_until_time(value, now_secs(), local_offset_now());
        self.invalid = self.until.is_none() && !value.trim().is_empty();
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if let Some(until) = self.until {
            cx.emit(PauseEvent::Save(until));
        }
    }
}

impl Render for PausePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, foreground) = (theme.input, theme.muted_foreground, theme.foreground);
        let hint = match (self.until, self.invalid) {
            (Some(until), _) => {
                format!("Notifications will stay off until {}.", clock(until, None))
            }
            (None, true) => "Enter a time like 4:30 PM or 16:30.".to_owned(),
            (None, false) => "Notifications will stay paused until the time you pick.".to_owned(),
        };
        let hint_color = if self.invalid {
            crate::theme::danger_text(cx)
        } else {
            muted
        };

        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(foreground)
                    .child("Pause notifications until a time you pick."),
            )
            .child(
                h_flex()
                    .items_center()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .child(Textarea::new(&self.text).appearance(false).bordered(false)),
            )
            .child(div().text_xs().text_color(hint_color).child(hint))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(
                        Button::new("pause-cancel")
                            .outline()
                            .label("Cancel")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PauseEvent::Cancel))),
                    )
                    .child(
                        Button::new("pause-save")
                            .primary()
                            .label("Pause")
                            .disabled(self.until.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
    }
}

/// The next occurrence of a wall-clock time (`4:30 pm`, `16:30`) as a Unix timestamp:
/// today if it is still ahead, otherwise tomorrow. `None` when `text` is not a time.
fn parse_until_time(text: &str, now: i64, offset: i64) -> Option<i64> {
    #[derive(Clone, Copy)]
    enum Meridiem {
        Am,
        Pm,
        Clock24,
    }
    let rest = text.trim().to_lowercase();
    let (body, meridiem) = match rest.strip_suffix("pm") {
        Some(body) => (body.trim_end(), Meridiem::Pm),
        None => match rest.strip_suffix("am") {
            Some(body) => (body.trim_end(), Meridiem::Am),
            None => (rest.as_str(), Meridiem::Clock24),
        },
    };
    let (hour_text, minute_text) = match body.split_once(':') {
        Some((hour, minute)) => (hour.trim(), minute.trim()),
        None => (body.trim(), "0"),
    };
    if hour_text.is_empty()
        || !hour_text.chars().all(|c| c.is_ascii_digit())
        || minute_text.is_empty()
        || minute_text.len() > 2
        || !minute_text.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let hour: i64 = hour_text.parse().ok()?;
    let minute: i64 = minute_text.parse().ok()?;
    let (low, high) = match meridiem {
        Meridiem::Am | Meridiem::Pm => (1, 12),
        Meridiem::Clock24 => (0, 23),
    };
    if !(low..=high).contains(&hour) || minute > 59 {
        return None;
    }
    let hour = match (meridiem, hour) {
        (Meridiem::Pm, 12) => 12,
        (Meridiem::Pm, hour) => hour + 12,
        (Meridiem::Am, 12) => 0,
        (Meridiem::Am, hour) | (Meridiem::Clock24, hour) => hour,
    };
    let midnight = (now + offset).div_euclid(86_400) * 86_400 - offset;
    let mut until = midnight + hour * 3_600 + minute * 60;
    if until <= now {
        until += 86_400;
    }
    Some(until)
}

#[cfg(test)]
mod tests {
    use super::{ScheduledStatus, SelfState, UserStatus, parse_until_time};

    #[test]
    fn activates_due_scheduled_statuses_in_earliest_timestamp_order() {
        let mut state = SelfState::default();
        // Push out of chronological order; earliest due timestamp must activate first.
        state.scheduled_statuses.push(ScheduledStatus {
            emoji: "palm_tree".to_owned(),
            text: "Future PTO".to_owned(),
            starts_at: i64::MAX - 10,
            clear_after: 0,
            pause: true,
        });
        state.scheduled_statuses.push(ScheduledStatus {
            emoji: "coffee".to_owned(),
            text: "Lunch".to_owned(),
            starts_at: 2_000,
            clear_after: 1,
            pause: false,
        });
        state.scheduled_statuses.push(ScheduledStatus {
            emoji: "spiral_calendar_pad".to_owned(),
            text: "Standup".to_owned(),
            starts_at: 1_000,
            clear_after: 1,
            pause: true,
        });

        let first = state
            .activate_due_scheduled_at(5_000, 0)
            .expect("earliest due status activates");
        assert_eq!(first.text, "Standup");
        assert_eq!(first.expiration, Some(6_800));
        assert!(state.paused_until == Some(6_800));

        let second = state
            .activate_due_scheduled_at(5_000, 0)
            .expect("second due status activates");
        assert_eq!(second.text, "Lunch");
        assert_eq!(state.scheduled_statuses.len(), 1);
        assert!(state.activate_due_scheduled_at(5_000, 0).is_none());
    }

    #[test]
    fn scheduled_status_clears_at_respects_timezone_boundaries() {
        // 2024-03-07 23:00:00 UTC (Thursday):
        // - In UTC (offset 0): Today ends at 2024-03-08 00:00:00 UTC (+3600s).
        // - In UTC+02:00 (+7200): local time is already 2024-03-08 01:00 (Friday),
        //   so Today ends at 2024-03-09 00:00 local = 2024-03-08 22:00 UTC (+82800s).
        let now_utc = 1_709_852_400; // 2024-03-07 23:00:00 UTC
        let status_today = UserStatus {
            emoji: "bus".into(),
            text: "Commuting".into(),
            clear_after: 4, // Today
            pause: false,
            expiration: None,
        };
        assert_eq!(
            status_today.clears_at_from(now_utc, 0),
            Some(now_utc + 3_600)
        );
        assert_eq!(
            status_today.clears_at_from(now_utc, 7_200),
            Some(now_utc + 23 * 3_600)
        );
        // "This week" on Thursday (day 3 since Monday) -> +4 days from local midnight
        let status_week = UserStatus {
            clear_after: 5,
            ..status_today.clone()
        };
        assert_eq!(
            status_week.clears_at_from(now_utc, 0),
            Some(now_utc + 3_600 + 3 * 86_400)
        );
        // In UTC+02:00 it is already Friday (day 4 since Monday) -> +3 days from Friday local midnight
        assert_eq!(
            status_week.clears_at_from(now_utc, 7_200),
            Some(now_utc + 23 * 3_600 + 2 * 86_400)
        );
    }

    #[test]
    fn user_status_expiry_countdown_and_self_state_expiration() {
        let mut s = UserStatus {
            emoji: "coffee".into(),
            text: "Break".into(),
            clear_after: 0,
            pause: true,
            expiration: None,
        };
        assert!(!s.is_expired_at(1_000));
        assert!(!s.is_expired());
        assert_eq!(s.format_remaining(1_000), None);

        s.expiration = Some(1_045);
        assert!(!s.is_expired_at(1_000));
        assert_eq!(s.format_remaining(1_000).as_deref(), Some("45s"));
        assert!(s.is_expired_at(1_045));
        assert!(s.is_expired_at(1_046));
        assert_eq!(s.format_remaining(1_045), None);

        s.expiration = Some(1_000 + 12 * 60 + 30);
        assert_eq!(s.format_remaining(1_000).as_deref(), Some("12m"));
        s.expiration = Some(1_000 + 2 * 3_600);
        assert_eq!(s.format_remaining(1_000).as_deref(), Some("2h"));
        s.expiration = Some(1_000 + 2 * 3_600 + 15 * 60);
        assert_eq!(s.format_remaining(1_000).as_deref(), Some("2h 15m"));
        s.expiration = Some(1_000 + 3 * 86_400);
        assert_eq!(s.format_remaining(1_000).as_deref(), Some("3d"));
        s.expiration = Some(1_000 + 3 * 86_400 + 4 * 3_600);
        assert_eq!(s.format_remaining(1_000).as_deref(), Some("3d 4h"));

        // SelfState expiration clears both status and pause
        let mut state = SelfState::default();
        s.expiration = Some(2_000);
        state.set_status(Some(s));
        assert_eq!(state.paused_until, Some(2_000));
        assert!(!state.expire_status_at(1_999));
        assert!(state.status.is_some());
        assert!(state.expire_status_at(2_000));
        assert!(state.status.is_none());
        assert!(state.paused_until.is_none());
    }

    #[test]
    fn pause_label_formats_today_tomorrow_multi_day_indefinite_and_expired() {
        // 2024-03-07 10:00:00 UTC
        let mut state = SelfState {
            paused_until: Some(NOW - 1),
            ..SelfState::default()
        };
        assert_eq!(state.pause_label_at(NOW, 0), None);

        state.paused_until = Some(i64::MAX);
        assert_eq!(
            state.pause_label_at(NOW, 0).as_deref(),
            Some("Notifications paused")
        );

        state.paused_until = Some(NOW + 23_400); // 4:30 PM today
        assert_eq!(
            state.pause_label_at(NOW, 0).as_deref(),
            Some("Notifications paused until 4:30 PM")
        );

        state.paused_until = Some(NOW + 86_400); // 10:00 AM tomorrow
        assert_eq!(
            state.pause_label_at(NOW, 0).as_deref(),
            Some("Notifications paused until tomorrow, 10:00 AM")
        );

        state.paused_until = Some(NOW + 3 * 86_400);
        assert_eq!(
            state.pause_label_at(NOW, 0).as_deref(),
            Some("Notifications paused until 10:00 AM, in 3 days")
        );
    }

    /// 2024-03-07 10:00:00 UTC (`offset` 0 in the calls below).
    const NOW: i64 = 1_709_805_600;

    fn until(text: &str) -> Option<i64> {
        parse_until_time(text, NOW, 0)
    }

    #[test]
    fn parses_clock_times_to_the_next_occurrence() {
        // 4:30 PM today is ahead of 10:00 AM: today at 16:30.
        assert_eq!(until("4:30 pm"), Some(NOW + 23_400));
        assert_eq!(until("4:30pm"), Some(NOW + 23_400));
        assert_eq!(until("4:30 PM"), Some(NOW + 23_400));
        assert_eq!(until("  16:30 "), Some(NOW + 23_400));
        // Noon: 12 PM and 24-hour 12:00 are both noon today; 12 AM and 0:00 are midnight tomorrow.
        assert_eq!(until("12 pm"), Some(NOW + 7_200));
        assert_eq!(until("12:00 PM"), Some(NOW + 7_200));
        assert_eq!(until("12:00"), Some(NOW + 7_200));
        assert_eq!(until("12 am"), Some(NOW + 86_400 - 36_000));
        assert_eq!(until("12:00 am"), Some(NOW + 86_400 - 36_000));
        assert_eq!(until("0:00"), Some(NOW + 86_400 - 36_000));
        assert_eq!(until("00:00"), Some(NOW + 86_400 - 36_000));
        // Single-digit hours and exact current time (rolls to tomorrow).
        assert_eq!(until("9:05 am"), Some(NOW + 83_100));
        assert_eq!(until("9:05 AM"), Some(NOW + 83_100));
        assert_eq!(until("10:00"), Some(NOW + 86_400));
        assert_eq!(until("11"), Some(NOW + 3_600));
        // Bare hours count as 24-hour clock times.
        assert_eq!(until("23"), Some(NOW + 46_800));
        // Timezone offset boundary: at 10:00 UTC in UTC-08:00 it is 02:00 local, so 09:00 local is +7h.
        assert_eq!(
            parse_until_time("9:00 am", NOW, -8 * 3_600),
            Some(NOW + 7 * 3_600)
        );
    }

    #[test]
    fn rejects_non_times() {
        assert_eq!(until(""), None);
        assert_eq!(until("soon"), None);
        assert_eq!(until("13 pm"), None);
        assert_eq!(until("13:00 pm"), None);
        assert_eq!(until("13:00 am"), None);
        assert_eq!(until("0 pm"), None);
        assert_eq!(until("0:00 am"), None);
        assert_eq!(until("4:65 pm"), None);
        assert_eq!(until("12:60"), None);
        assert_eq!(until("24:00"), None);
        assert_eq!(until("25:00"), None);
        assert_eq!(until("4:3:00 pm"), None);
        assert_eq!(until("-4 pm"), None);
        assert_eq!(until("4:"), None);
    }
}
