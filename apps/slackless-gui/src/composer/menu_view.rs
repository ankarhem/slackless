//! How the completion menu looks: a panel floating above the composer's frame, one row per
//! entry.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, Context, Div, FontWeight, Hsla, InteractiveElement as _, MouseButton, ParentElement as _,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, div, px, relative,
};
use slackless_core::types::rich_text::BroadcastRange;

use super::Composer;
use super::menu::{Entry, Menu};
use crate::avatars;
use crate::editor::Directory as _;
use crate::emoji;
use crate::fixtures::Fixtures;

impl Composer {
    /// The menu floating above the frame.
    pub(super) fn menu_panel(&self, menu: &Menu, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let colors = RowColors {
            muted: theme.muted_foreground,
            active: theme.sidebar_accent,
            active_foreground: theme.sidebar_accent_foreground,
            hover: theme.accent,
            surface: theme.popover,
        };
        let (popover, border) = (theme.popover, theme.border);
        let hint = match menu.entries.first() {
            Some(Entry::User(_) | Entry::Broadcast(_)) => "People",
            Some(Entry::Channel(_)) => "Channels",
            Some(Entry::Emoji(_)) => "Emoji",
            Some(Entry::Slash(_)) | None => "Built-in commands",
        };
        let rows: Vec<_> = menu
            .entries
            .iter()
            .enumerate()
            .map(|(ix, entry)| {
                let is_active = ix == menu.selected;
                self.row(entry, colors.tones(is_active), cx)
                    .id(("composer-menu-row", ix))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(is_active, |row| {
                        row.bg(colors.active).text_color(colors.active_foreground)
                    })
                    .when(!is_active, |row| row.hover(|el| el.bg(colors.hover)))
                    // Keep the draft focused: the click must not move focus to the window.
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(cx.listener(move |this, _, window, cx| this.pick(ix, window, cx)))
            })
            .collect();
        v_flex()
            .id("composer-menu")
            .occlude()
            .absolute()
            .left(px(16.))
            .right(px(16.))
            .bottom(relative(1.))
            .max_h(px(320.))
            .overflow_y_scroll()
            .p_1()
            .gap_0p5()
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(popover)
            .shadow_lg()
            .text_sm()
            .child(div().px_3().py_1().text_xs().text_color(colors.muted).child(format!(
                "{hint} \u{b7} \u{2191}\u{2193} navigate \u{b7} tab or enter to pick \u{b7} esc to dismiss"
            )))
            .children(rows)
    }
}

struct RowColors {
    muted: Hsla,
    active: Hsla,
    active_foreground: Hsla,
    hover: Hsla,
    surface: Hsla,
}

/// The colors one row's content takes.
struct RowTones {
    /// Secondary text and icons.
    secondary: Hsla,
    /// The surface behind the row, cut out around an avatar's presence dot.
    ring: Hsla,
}

impl RowColors {
    fn tones(&self, is_active: bool) -> RowTones {
        if is_active {
            RowTones {
                secondary: self.active_foreground,
                ring: self.active,
            }
        } else {
            RowTones {
                secondary: self.muted,
                ring: self.surface,
            }
        }
    }
}

impl Composer {
    /// One menu row's content.
    fn row(&self, entry: &Entry, tones: RowTones, cx: &App) -> Div {
        let fixtures: &Fixtures = &self.fixtures;
        let RowTones { secondary, ring } = tones;
        let title = |text: SharedString| div().font_weight(FontWeight::SEMIBOLD).child(text);
        let note = |text: SharedString| div().text_color(secondary).child(text);
        let line = h_flex().items_center().gap_2();
        match entry {
            Entry::User(id) => {
                let chip = avatars::user_chip(fixtures, id, 20.);
                let handle = fixtures.user(id).map(|user| user.name.clone());
                line.child(avatars::with_presence(
                    chip,
                    fixtures.presence(id),
                    20.,
                    ring,
                    cx,
                ))
                .child(title(fixtures.user_name(id).to_owned().into()))
                .children(handle.map(|handle| note(handle.into())))
            }
            Entry::Broadcast(range) => {
                let reach = match range {
                    BroadcastRange::Here => "Notify every online member in this channel",
                    BroadcastRange::Channel => "Notify everyone in this channel",
                    BroadcastRange::Everyone | BroadcastRange::Other(_) => {
                        "Notify everyone in this workspace"
                    }
                };
                line.child(
                    Icon::new(IconName::Megaphone)
                        .size(px(16.))
                        .text_color(secondary),
                )
                .child(title(format!("@{range}").into()))
                .child(note(reach.into()))
            }
            Entry::Channel(id) => {
                let is_private = fixtures.conversation(id).is_some_and(|c| c.is_private);
                let icon = if is_private {
                    IconName::Lock
                } else {
                    IconName::Hash
                };
                line.child(Icon::new(icon).size(px(14.)).text_color(secondary))
                    .child(title(fixtures.channel_name(id).into()))
            }
            Entry::Emoji(name) => line
                .child(emoji::emoji_element(name, fixtures, px(18.)))
                .child(div().child(format!(":{name}:"))),
            Entry::Slash(command) => v_flex()
                .child(
                    h_flex()
                        .gap_2()
                        .child(title(format!("/{}", command.name).into()))
                        .child(note(command.usage.into())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(secondary)
                        .child(command.description),
                ),
        }
    }
}
