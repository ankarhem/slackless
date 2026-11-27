//! The pinned-messages panel (docs/research/api-reactions-saved.md: `pins.list`): the right
//! column the Pins tab and the bookmarks bar's "N Pinned" chip open (exclusive with the
//! thread and profile panels), listing the pinned messages with the shared [`message_row`].
//!
//! Live mode lists the conversation's `pins.list` messages, fetched each time the panel
//! opens; file pins are left to the Files & links tab. Fixture mode derives a conversation's
//! pins from the fixtures: its messages with files or at least two reactions (oldest first,
//! at most four, else its first message).

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::{
    App, Div, FontWeight, InteractiveElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::types::{ChannelId, Message};

use crate::fixtures::Fixtures;
use crate::messages::message_row;
use crate::tab_views::{Loaded, state_line};

/// At most this many fixture pins per conversation.
const MAX_PINS: usize = 4;

/// The fixture pins of `channel` (see the module docs).
pub fn pinned<'a>(fixtures: &'a Fixtures, channel: &ChannelId) -> Vec<&'a Message> {
    let Some(history) = fixtures.messages.get(channel) else {
        return Vec::new();
    };
    let pins: Vec<&Message> = history
        .iter()
        .filter(|message| {
            let reactions: u32 = message
                .reactions
                .iter()
                .map(|reaction| reaction.count)
                .sum();
            !message.hidden && (!message.files.is_empty() || reactions >= 2)
        })
        .take(MAX_PINS)
        .collect();
    if pins.is_empty() {
        history
            .iter()
            .find(|message| !message.hidden)
            .into_iter()
            .collect()
    } else {
        pins
    }
}

/// The pinned-messages panel titled `label`: the pinned messages (oldest first) once
/// loaded, else a loading or error line; `on_close` closes it.
pub fn pins_panel(
    fixtures: &Arc<Fixtures>,
    pins: Option<&Loaded<Message>>,
    label: &str,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let (border, muted, background) = (theme.border, theme.muted_foreground, theme.background);
    let count = pins.and_then(|pins| pins.as_ref().ok()).map(Vec::len);
    let row = |message: &Message| {
        v_flex()
            .mx_3()
            .rounded_md()
            .border_1()
            .border_color(border)
            .py_1()
            .child(
                h_flex()
                    .px_3()
                    .gap_1()
                    .items_center()
                    .text_xs()
                    .text_color(muted)
                    .child(Icon::new(IconName::Pin).text_color(muted).size(px(12.)))
                    .child("Pinned"),
            )
            .child(message_row(fixtures, message, None, cx))
    };
    let body = match pins {
        None => state_line("Loading pinned messages…", muted),
        Some(Err(error)) => state_line(&format!("Could not load pins: {error}"), muted),
        Some(Ok(pins)) if pins.is_empty() => state_line("No pinned messages yet.", muted),
        Some(Ok(pins)) => v_flex()
            .id("pins-list")
            .flex_1()
            .min_h_0()
            .py_3()
            .gap_2()
            .overflow_y_scroll()
            .children(pins.iter().map(row)),
    };
    v_flex()
        .size_full()
        .bg(background)
        .border_l_1()
        .border_color(border)
        .child(
            h_flex()
                .flex_none()
                .items_center()
                .justify_between()
                .px_4()
                .h(px(46.))
                .border_b_1()
                .border_color(border)
                .child(
                    v_flex()
                        .child(div().font_weight(FontWeight::BOLD).child("Pinned messages"))
                        .child(div().text_xs().text_color(muted).child(match count {
                            Some(count) => format!("{label} \u{b7} {count}"),
                            None => label.to_owned(),
                        })),
                )
                .child(
                    Button::new("pins-close")
                        .ghost()
                        .with_size(Size::Small)
                        .icon(IconName::X)
                        .tooltip("Close")
                        .on_click(move |_, window, cx| on_close(window, cx)),
                ),
        )
        .child(body)
}
