//! Custom Do Not Disturb pause-until dialog and time expression parser.

use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Window, div,
};

use crate::messages::{clock, local_offset_now, now_secs};

pub enum PauseEvent {
    /// Pause notifications until this Unix time.
    Save(i64),
    Cancel,
}

/// Slack's "Custom…" pause dialog: a wall-clock time field with a live "Until …" hint.
pub(super) struct PausePicker {
    pub(super) text: Entity<TextareaState>,
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
        let (muted, foreground) = (theme.muted_foreground, theme.foreground);
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
            .child(crate::preferences::text_input_box(&self.text, false, cx))
            .child(div().text_xs().text_color(hint_color).child(hint))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(
                        crate::preferences::form_button(
                            Button::new("pause-cancel").outline().label("Cancel"),
                            cx,
                        )
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(PauseEvent::Cancel))),
                    )
                    .child(
                        crate::preferences::form_button(
                            Button::new("pause-save")
                                .primary()
                                .label("Pause")
                                .disabled(self.until.is_none()),
                            cx,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
    }
}

/// The next occurrence of a wall-clock time (`4:30 pm`, `16:30`) as a Unix timestamp:
/// today if it is still ahead, otherwise tomorrow. `None` when `text` is not a time.
pub(super) fn parse_until_time(text: &str, now: i64, offset: i64) -> Option<i64> {
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
