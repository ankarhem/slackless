//! Keyboard shortcuts help (Cmd+/), mounted in a dialog by the root: Slack desktop's
//! shortcut sheet grouped by area. Shortcuts the app does not handle yet are listed dimmed
//! with a "soon" tag so the sheet never promises more than the app does.
//!
//! `pub fn shortcuts_view(&App) -> Stateful<Div>`

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, Div, FontWeight, InteractiveElement as _, ParentElement as _, Stateful,
    StatefulInteractiveElement as _, Styled as _, div, px,
};

/// `(keys, what it does, handled by this app)`.
type Shortcut = (&'static [&'static str], &'static str, bool);

const GROUPS: [(&str, &[Shortcut]); 5] = [
    (
        "Navigation",
        &[
            (&["\u{2318}", "K"], "Jump to a conversation", true),
            (&["\u{2318}", "J"], "Jump to unread messages", true),
            (&["\u{2318}", "\u{21e7}", "H"], "Open Home", true),
            (&["\u{2318}", "\u{21e7}", "K"], "Open direct messages", true),
            (&["\u{2318}", "\u{21e7}", "M"], "Open activity", true),
            (&["\u{2318}", "\u{21e7}", "A"], "Open All unreads", true),
            (&["\u{2318}", "\u{21e7}", "T"], "Open threads", true),
            (&["\u{2318}", "\u{21e7}", "D"], "Open drafts & sent", true),
            (&["\u{2318}", "\u{21e7}", "L"], "Browse channels", true),
            (
                &["\u{2318}", "\u{21e7}", "E"],
                "Open people & user groups",
                true,
            ),
            (&["\u{2318}", "\u{21e7}", "J"], "View all downloads", true),
            (&["\u{2318}", "G"], "Search", true),
            (&["\u{2318}", "F"], "Search current conversation", true),
            (&["\u{2318}", "["], "Go back in history", true),
            (&["\u{2318}", "]"], "Go forward in history", true),
            (
                &["\u{2325}", "\u{2191}/\u{2193}"],
                "Previous / next conversation",
                true,
            ),
            (
                &["\u{2325}", "\u{21e7}", "\u{2191}/\u{2193}"],
                "Previous / next unread conversation",
                true,
            ),
            (&["\u{2318}", "1\u{2026}9"], "Switch workspace", true),
            (&["\u{2318}", "{ / }"], "Previous / next workspace", true),
        ],
    ),
    (
        "Messages",
        &[
            (&["\u{2318}", "N"], "New message", true),
            (
                &["\u{2318}", "\u{21e7}", "\u{21b5}"],
                "Create a text or code snippet",
                true,
            ),
            (&["\u{21b5}"], "Send message", true),
            (&["\u{21e7}", "\u{21b5}"], "New line in message", true),
            (&["/"], "Slash commands (at the start of a message)", true),
            (&["\u{2191}"], "Edit your last message", true),
            (
                &["\u{2318}", "\u{21e7}", "\u{232b}"],
                "Unsend your last message",
                true,
            ),
            (
                &["\u{2318}", "\u{21e7}", "\\"],
                "Add a reaction to the last message",
                false,
            ),
            (&["Esc"], "Mark conversation as read", true),
            (&["\u{21e7}", "Esc"], "Mark all messages as read", true),
        ],
    ),
    (
        "Take actions on messages",
        // Slack's message focus: the last message the mouse hovered, acted on only while
        // no field holds the focus (messages.rs focus_action).
        &[
            (&["E"], "Edit message", true),
            (&["\u{232b}"], "Delete message", true),
            (&["T", "\u{2192}"], "Open or reply in thread", true),
            (&["F"], "Forward message", true),
            (&["P"], "Pin or unpin a message", true),
            (&["A"], "Save or unsave a message", true),
            (&["U"], "Mark all messages above as unread", true),
            (&["M"], "Set a reminder", true),
            (&["R"], "Add a reaction", true),
        ],
    ),
    (
        "Formatting",
        &[
            (&["\u{2318}", "B"], "Bold", true),
            (&["\u{2318}", "I"], "Italic", true),
            (&["\u{2318}", "U"], "Underline", true),
            (&["\u{2318}", "\u{21e7}", "X"], "Strikethrough", true),
            (&["\u{2318}", "\u{21e7}", "C"], "Code", true),
            (
                &["\u{2318}", "\u{2325}", "\u{21e7}", "C"],
                "Code block",
                true,
            ),
            (&["\u{2318}", "\\"], "Clear formatting", true),
            (&["\u{2318}", "\u{21e7}", "U"], "Link", true),
            (&["\u{2318}", "\u{21e7}", "7"], "Numbered list", true),
            (&["\u{2318}", "\u{21e7}", "8"], "Bulleted list", true),
            (&["\u{2318}", "\u{21e7}", "9"], "Quote", true),
        ],
    ),
    (
        "General",
        &[
            (&["\u{2318}", ","], "Preferences", true),
            (&["\u{2318}", "\u{21e7}", "Y"], "Set a status", true),
            (
                &["\u{2318}", "\u{21e7}", "S"],
                "Toggle workspace switcher",
                true,
            ),
            (&["\u{2318}", "."], "Toggle right sidebar", true),
            (&["\u{2318}", "/"], "Keyboard shortcuts", true),
            (&["\u{2303}", "\u{2318}", "F"], "Toggle full screen", true),
            (&["\u{2318}", "+ / -"], "Zoom in / out", true),
            (&["\u{2318}", "0"], "Actual size", true),
        ],
    ),
];

fn key_cap(key: &'static str, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .min_w(px(22.))
        .h(px(22.))
        .px_1p5()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .child(key)
}

pub fn shortcuts_view(cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (muted, border, foreground) = (theme.muted_foreground, theme.border, theme.foreground);
    let groups = GROUPS.map(|(title, shortcuts)| {
        v_flex()
            .gap_1()
            .child(
                div()
                    .pb_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child(title.to_uppercase()),
            )
            .children(shortcuts.iter().map(|(keys, label, wired)| {
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .py_1()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .text_color(if *wired { foreground } else { muted })
                            .child(*label)
                            .when(!wired, |row| {
                                row.child(div().text_xs().text_color(muted).child("soon"))
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .children(keys.iter().map(|key| key_cap(key, cx))),
                    )
            }))
    });
    let [navigation, messages, take_actions, formatting, general] = groups;
    div()
        .id("shortcuts")
        .max_h(px(560.))
        .overflow_y_scroll()
        .child(
            h_flex()
                .items_start()
                .gap_6()
                .child(v_flex().flex_1().gap_5().child(navigation).child(general))
                .child(
                    v_flex()
                        .flex_1()
                        .gap_5()
                        .child(messages)
                        .child(take_actions)
                        .child(formatting),
                ),
        )
}
