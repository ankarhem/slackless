//! The signed-in user's status, presence and Do Not Disturb: the [`StatusPicker`] dialog,
//! the [`UserMenu`] dropped from the title-bar avatar, the [`PausePicker`] custom pause dialog,
//! and [`self_avatar`], the avatar with its presence / DND badge.

mod menu;
mod pause;
mod picker;

#[cfg(test)]
mod tests;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::gpui::{App, Div, FontWeight, Hsla, ParentElement as _, Styled as _, div, px};

use crate::avatars::user_chip;
use crate::emoji::glyph as emoji_glyph;
use crate::fixtures::Fixtures;
use crate::messages::{clock, local_offset_now, now_secs};

pub use menu::{UserMenu, UserMenuEvent};
pub use picker::{StatusEvent, StatusPicker};

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
pub(super) const SUGGESTED: [(&str, &str, usize); 5] = [
    ("spiral_calendar_pad", "In a meeting", 2),
    ("bus", "Commuting", 1),
    ("face_with_thermometer", "Out sick", 4),
    ("palm_tree", "Vacationing", 0),
    ("house_with_garden", "Working remotely", 4),
];

/// The emoji offered by the picker's emoji button.
pub(super) const EMOJI_CHOICES: [&str; 12] = [
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

/// How an active custom status was set (`F-SHELL-020`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusSource {
    #[default]
    Manual,
    FocusMode,
    Calendar,
    Huddle,
    Scheduled,
    App,
}

impl StatusSource {
    /// Descriptive label for automatic status sources shown in Status Picker & Profile (`F-SHELL-020`).
    pub const fn badge_label(self) -> Option<&'static str> {
        match self {
            Self::Manual => None,
            Self::FocusMode => Some("Automatic \u{b7} Focus mode"),
            Self::Calendar => Some("Automatic \u{b7} Calendar"),
            Self::Huddle => Some("Automatic \u{b7} In a huddle"),
            Self::Scheduled => Some("Automatic \u{b7} Scheduled"),
            Self::App => Some("Automatic \u{b7} App"),
        }
    }

    /// Compact badge shown inside narrow menus.
    pub const fn short_label(self) -> Option<&'static str> {
        match self {
            Self::Manual => None,
            Self::FocusMode => Some("Focus"),
            Self::Calendar => Some("Calendar"),
            Self::Huddle => Some("Huddle"),
            Self::Scheduled => Some("Scheduled"),
            Self::App => Some("Auto"),
        }
    }

    pub const fn is_automatic(self) -> bool {
        !matches!(self, Self::Manual)
    }
}

/// Infer an automatic status source from a Slack [`Profile`] (`F-SHELL-020`).
pub fn infer_profile_status_source(
    profile: &slackless_core::types::Profile,
    emoji: &str,
    text: &str,
) -> StatusSource {
    if profile.huddle_state.as_deref() == Some("in_a_huddle") {
        return StatusSource::Huddle;
    }
    if text.eq_ignore_ascii_case("In focus mode") && emoji == "telescope" {
        return StatusSource::FocusMode;
    }
    if text.eq_ignore_ascii_case("In a meeting")
        && matches!(emoji, "spiral_calendar_pad" | "calendar" | "date")
    {
        return StatusSource::Calendar;
    }
    if text.eq_ignore_ascii_case("In a huddle") && emoji == "headphones" {
        return StatusSource::Huddle;
    }
    if matches!(emoji, "slack_call" | "video_camera" | "phone")
        && (text.eq_ignore_ascii_case("On a call") || text.to_ascii_lowercase().contains("zoom"))
    {
        return StatusSource::App;
    }
    StatusSource::Manual
}

/// A custom status: emoji shortcode (no colons), text, and when it clears.
#[derive(Clone, PartialEq, Debug)]
pub struct UserStatus {
    pub emoji: String,
    pub text: String,
    /// Index into [`CLEAR_AFTER`].
    pub clear_after: usize,
    /// Slack's "Pause notifications until status is cleared".
    pub pause: bool,
    /// Concrete unix expiration timestamp (seconds), when set or synced from Slack.
    pub expiration: Option<i64>,
    /// Origin of this status (manual vs automatic Calendar / Focus / Huddle / Scheduled; `F-SHELL-020`).
    pub source: StatusSource,
}

impl UserStatus {
    pub fn glyph(&self) -> &'static str {
        emoji_glyph(self.emoji.as_str()).unwrap_or("\u{1F4AC}")
    }

    /// Hydrate a [`UserStatus`] from a Slack [`Profile`], dropping expired or empty ones and
    /// surfacing automatic huddle status when `huddle_state == "in_a_huddle"` (`F-SHELL-020`).
    pub fn from_profile(profile: &slackless_core::types::Profile) -> Option<Self> {
        let text = profile.status_text.as_deref().unwrap_or_default().trim();
        let emoji = profile
            .status_emoji
            .as_deref()
            .unwrap_or_default()
            .trim()
            .trim_matches(':');
        if text.is_empty() && emoji.is_empty() {
            if profile.huddle_state.as_deref() == Some("in_a_huddle") {
                let exp = profile.huddle_state_expiration_ts.filter(|&ts| ts > 0);
                if exp.is_none_or(|ts| ts > now_secs()) {
                    return Some(Self {
                        emoji: "headphones".to_owned(),
                        text: "In a huddle".to_owned(),
                        clear_after: 0,
                        pause: false,
                        expiration: exp,
                        source: StatusSource::Huddle,
                    });
                }
            }
            return None;
        }
        let expiration = profile.status_expiration.filter(|&ts| ts > 0);
        if expiration.is_some_and(|ts| ts <= now_secs()) {
            return None;
        }
        let resolved_emoji = if emoji.is_empty() {
            EMOJI_CHOICES[0].to_owned()
        } else {
            emoji.to_owned()
        };
        let source = infer_profile_status_source(profile, &resolved_emoji, text);
        Some(Self {
            emoji: resolved_emoji,
            text: text.to_owned(),
            clear_after: 0,
            pause: false,
            expiration,
            source,
        })
    }

    /// Whether this status's concrete expiration has passed at `now` (Unix seconds).
    pub fn is_expired_at(&self, now: i64) -> bool {
        self.expiration.is_some_and(|exp| exp <= now)
    }

    /// Whether this status has expired right now.
    #[cfg(test)]
    pub fn is_expired(&self) -> bool {
        self.is_expired_at(now_secs())
    }

    /// Human-readable time remaining until expiration (`"45s"`, `"12m"`, `"2h 15m"`, `"3d 4h"`),
    /// or `None` when the status has no expiration or has already expired.
    #[cfg(test)]
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
        source: StatusSource::FocusMode,
    }
}

impl SelfState {
    pub fn paused(&self) -> bool {
        self.paused_until.is_some_and(|until| until > now_secs())
    }

    pub fn focused(&self) -> bool {
        self.focus_until.is_some_and(|until| until > now_secs())
    }

    /// Turn focus mode on with `preset` (out-of-range falls back to [`FOCUS_UNTIL_OFF`]),
    /// optionally setting the automatic `"In focus mode"` status when `auto_status` is true
    /// (`F-SHELL-037`, `F-SHELL-039`).
    pub fn start_focus_with_status(&mut self, preset: usize, auto_status: bool) {
        let clamped = if preset < FOCUS_DURATIONS.len() {
            preset
        } else {
            FOCUS_UNTIL_OFF
        };
        let duration = FOCUS_DURATIONS.get(clamped).and_then(|(_, secs)| *secs);
        self.focus_preset = clamped;
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
            source: StatusSource::Scheduled,
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
