//! Message toasts, clipboard helpers, row status banners, and delete confirmation dialog.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Icon, WindowExt as _, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, ClipboardItem, Div, FontWeight, InteractiveElement as _, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::types::{ChannelId, Message, Ts};

use super::{ActionHandler, MessageAction};
use crate::fixtures::Fixtures;

/// Slack's "Saved for later" marker above a saved message.
pub fn saved_banner(cx: &App) -> Div {
    let warning = cx.theme().warning;
    h_flex()
        .items_center()
        .gap_1()
        .pb_0p5()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(warning)
        .child(
            Icon::new(IconName::Bookmark)
                .text_color(warning)
                .size(px(12.)),
        )
        .child("Saved for later")
}

/// Slack's "Only visible to you" marker above an ephemeral message, with an optional
/// "Dismiss" link that removes it from the transcript (`F-COLLAB-683`).
pub fn ephemeral_banner(key: &str, handler: Option<&ActionHandler>, cx: &App) -> Div {
    let theme = cx.theme();
    let (muted, link, tint) = (
        theme.muted_foreground,
        theme.link,
        theme.secondary.opacity(0.55),
    );
    h_flex()
        .items_center()
        .gap_1p5()
        .mb_1()
        .px_2()
        .py_0p5()
        .rounded_sm()
        .bg(tint)
        .text_xs()
        .text_color(muted)
        .child(Icon::new(IconName::Eye).text_color(muted).size(px(12.)))
        .child("Only visible to you")
        .when_some(handler.cloned(), |row, handler| {
            row.child("·").child(
                div()
                    .id(SharedString::from(format!("{key}-dismiss-eph")))
                    .cursor_pointer()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(link)
                    .hover(|el| el.underline())
                    .child("Dismiss")
                    .on_click(move |_, window, cx| {
                        handler(MessageAction::DismissEphemeral, window, cx)
                    }),
            )
        })
}

/// A toast for feedback on message actions.
pub fn notify(title: &str, message: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
    window.push_notification(
        Notification::new()
            .title(title.to_owned())
            .message(message)
            .autohide(true),
        cx,
    );
}

/// Copy the message's permalink (Slack's `archives/{channel}/p{ts}` shape) with a toast.
pub fn copy_link(
    fixtures: &Fixtures,
    channel: &ChannelId,
    ts: &Ts,
    window: &mut Window,
    cx: &mut App,
) {
    let link = permalink(fixtures, channel, ts);
    cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
    notify("Link copied", link, window, cx);
}

/// Copy `message`'s plain text to the clipboard with a consistent confirmation toast.
pub fn copy_message_text(
    fixtures: &Fixtures,
    message: &Message,
    window: &mut Window,
    cx: &mut App,
) {
    let text = crate::messages::plain_text(&message.text, fixtures);
    cx.write_to_clipboard(ClipboardItem::new_string(text));
    notify("Copied", "Message text copied.", window, cx);
}

/// Toast after toggling a message's pin state.
pub fn notify_pin_toggled(
    pinned: bool,
    detail: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) {
    notify(
        if pinned {
            "Pinned to conversation"
        } else {
            "Unpinned from conversation"
        },
        detail,
        window,
        cx,
    );
}

/// Toast after toggling a message's saved-for-later state.
pub fn notify_saved_toggled(
    saved: bool,
    detail: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) {
    notify(
        if saved {
            "Saved for later"
        } else {
            "Removed from Later"
        },
        detail,
        window,
        cx,
    );
}

/// Slack's permalink to the message `ts` in `channel`.
pub fn permalink(fixtures: &Fixtures, channel: &ChannelId, ts: &Ts) -> String {
    let domain = fixtures
        .own_team()
        .map(|team| team.domain.as_str())
        .filter(|d| !d.is_empty())
        .unwrap_or("app");
    format!(
        "https://{domain}.slack.com/archives/{channel}/p{}",
        ts.as_str().replace('.', "")
    )
}

/// Slack's "Delete message" confirmation, previewing `message`; `delete` runs only when the
/// user picks Delete.
pub fn confirm_delete(
    message: Message,
    fixtures: Arc<Fixtures>,
    window: &mut Window,
    cx: &mut App,
    delete: impl Fn(&mut App) + 'static,
) {
    let delete = Rc::new(delete);
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let delete = delete.clone();
        let preview = div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .child(crate::messages::message_row(&fixtures, &message, None, cx));
        alert
            // Slack seats it in the middle of the window, the message shown under the question.
            .centered()
            .width(px(520.))
            .title("Delete message")
            .description(
                v_flex()
                    .gap_3()
                    .child("Are you sure you want to delete this message? This cannot be undone.")
                    .child(preview),
            )
            .show_cancel(true)
            .ok_text("Delete")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, _, cx| {
                delete(cx);
                true
            })
    });
}
