//! Channel bookmarks bar and pinned-messages panel (docs/research/api-reactions-saved.md:
//! `pins.list`, `bookmarks.list`).
//!
//! The bookmarks bar sits under the conversation tabs: a "N Pinned" chip that toggles the
//! pins panel, then the conversation's bookmarks (links, docs, folders), each opening its
//! URL. The pins panel takes the right-hand column (exclusive with the thread and profile
//! panels) and lists the pinned messages with the shared [`message_row`].
//!
//! Fixture mode derives both from the fixtures: a conversation's pins are its messages with
//! files or at least two reactions (oldest first, at most four, else its first message), and
//! its bookmarks are a fixed set per conversation kind with names drawn from the channel.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, Div, FontWeight, InteractiveElement as _, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::types::{ChannelId, ConversationKind, Message};

use crate::fixtures::Fixtures;
use crate::messages::message_row;

/// At most this many fixture pins per conversation.
const MAX_PINS: usize = 4;

#[derive(Clone, Copy)]
enum BookmarkKind {
    Link,
    Doc,
    Folder,
}

impl BookmarkKind {
    fn icon(self) -> IconName {
        match self {
            BookmarkKind::Link => IconName::Globe,
            BookmarkKind::Doc => IconName::FileText,
            BookmarkKind::Folder => IconName::Folder,
        }
    }
}

pub struct Bookmark {
    title: String,
    url: String,
    kind: BookmarkKind,
}

/// The fixture bookmarks of `channel`: channels get a runbook, a roadmap doc and a shared
/// folder named after the channel; DMs get a single shared doc. Sample workspace only.
pub fn bookmarks(fixtures: &Fixtures, channel: &ChannelId) -> Vec<Bookmark> {
    let Some(conversation) = fixtures.conversation(channel).filter(|_| fixtures.sample) else {
        return Vec::new();
    };
    let name = conversation
        .name
        .clone()
        .unwrap_or_else(|| channel.to_string());
    let bookmark = |title: String, path: &str, kind| Bookmark {
        title,
        url: format!("https://example.com/{name}/{path}"),
        kind,
    };
    match conversation.kind() {
        ConversationKind::PublicChannel | ConversationKind::PrivateChannel => vec![
            bookmark(format!("{name} runbook"), "runbook", BookmarkKind::Link),
            bookmark("Roadmap".to_owned(), "roadmap", BookmarkKind::Doc),
            bookmark("Shared files".to_owned(), "files", BookmarkKind::Folder),
        ],
        ConversationKind::Im | ConversationKind::Mpim => {
            vec![bookmark("1:1 notes".to_owned(), "notes", BookmarkKind::Doc)]
        }
    }
}

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

/// The bar under the conversation tabs: pinned-count chip (`on_pins` toggles the panel,
/// highlighted while `pins_open`), then the bookmarks.
pub fn bookmarks_bar(
    bookmarks: Vec<Bookmark>,
    pin_count: usize,
    pins_open: bool,
    on_pins: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let (border, muted, foreground, hover, link) = (
        theme.border,
        theme.muted_foreground,
        theme.foreground,
        theme.accent,
        theme.link,
    );
    let chip = |id: SharedString| {
        h_flex()
            .id(id)
            .flex_none()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_0p5()
            .rounded_md()
            .text_xs()
            .cursor_pointer()
            .hover(move |el| el.bg(hover))
    };
    let pin_color = if pins_open { link } else { muted };
    let pins = chip("bookmark-pins".into())
        .when(pins_open, |chip| chip.bg(hover))
        .text_color(if pins_open { foreground } else { muted })
        .child(Icon::new(IconName::Pin).text_color(pin_color).size(px(14.)))
        .child(format!("{pin_count} Pinned"))
        .on_click(move |_, window, cx| on_pins(window, cx));
    let marks = bookmarks.into_iter().enumerate().map(|(ix, bookmark)| {
        let url = bookmark.url;
        chip(SharedString::from(format!("bookmark-{ix}")))
            .text_color(foreground)
            .child(
                Icon::new(bookmark.kind.icon())
                    .text_color(muted)
                    .size(px(14.)),
            )
            .child(bookmark.title)
            .on_click(move |_, _, cx| cx.open_url(&url))
    });
    h_flex()
        .flex_none()
        .items_center()
        .gap_1()
        .px_3()
        .h(px(32.))
        .border_b_1()
        .border_color(border)
        .overflow_hidden()
        .child(pins)
        .child(div().w(px(1.)).h(px(16.)).bg(border))
        .children(marks)
        .child(
            chip("bookmark-add".into())
                .text_color(muted)
                .child(Icon::new(IconName::Plus).text_color(muted).size(px(14.)))
                .child("Add a bookmark"),
        )
}

/// The pinned-messages panel for `channel` titled `label`; `on_close` closes it.
pub fn pins_panel(
    fixtures: &Fixtures,
    channel: &ChannelId,
    label: &str,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let (border, muted, background) = (theme.border, theme.muted_foreground, theme.background);
    let pins = pinned(fixtures, channel);
    let count = pins.len();
    let rows = pins.into_iter().map(|message| {
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
    });
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
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(format!("{label} \u{b7} {count}")),
                        ),
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
        .child(
            v_flex()
                .id("pins-list")
                .flex_1()
                .min_h_0()
                .py_3()
                .gap_2()
                .overflow_y_scroll()
                .children(rows),
        )
}
