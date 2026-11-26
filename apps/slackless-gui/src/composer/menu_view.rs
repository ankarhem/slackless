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
        let has_dup_slash = menu
            .entries
            .iter()
            .any(|e| matches!(e, Entry::Slash(cmd) if cmd.is_duplicate));
        let hint = match menu.entries.first() {
            Some(Entry::User(_) | Entry::Usergroup(_) | Entry::Broadcast(_)) => "People",
            Some(Entry::Channel(_)) => "Channels",
            Some(Entry::Emoji(_)) => "Emoji",
            Some(Entry::Slash(_)) if has_dup_slash => {
                "Commands \u{b7} Multiple apps share a name \u{2014} pick an app"
            }
            Some(Entry::Slash(_)) | None => "Commands",
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
            .on_scroll(|_, _, cx| cx.stop_propagation())
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
        let title = |text: SharedString| {
            div()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .child(text)
        };
        let note =
            |text: SharedString| div().min_w_0().truncate().text_color(secondary).child(text);
        let line = h_flex().items_center().gap_2();
        match entry {
            Entry::User(id) => {
                let chip = avatars::user_chip(fixtures, id, 20.);
                let user = fixtures.user(id);
                let handle = user.map(|user| user.name.clone());
                let is_app = user.is_some_and(|u| u.is_bot || u.is_app_user);
                line.child(avatars::with_presence(
                    chip,
                    fixtures.presence(id),
                    20.,
                    ring,
                    cx,
                ))
                .child(title(fixtures.user_name(id).to_owned().into()))
                .when(is_app, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .px_1()
                            .rounded(px(3.))
                            .bg(cx.theme().secondary)
                            .text_size(px(10.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(secondary)
                            .child("APP"),
                    )
                })
                .children(handle.map(|handle| note(handle.into())))
                .when(is_app, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .ml_auto()
                            .text_xs()
                            .text_color(secondary)
                            .child("Mention app"),
                    )
                })
            }
            Entry::Usergroup(id) => {
                let handle = fixtures.usergroup_handle(id).unwrap_or(id.as_str());
                let detail = fixtures.usergroup(id).map(|g| match g.user_count {
                    Some(n) if n > 0 => format!("{} ({n})", g.name),
                    _ => g.name.clone(),
                });
                line.child(
                    Icon::new(IconName::Users)
                        .flex_none()
                        .size(px(16.))
                        .text_color(secondary),
                )
                .child(title(format!("@{handle}").into()))
                .children(detail.filter(|s| !s.is_empty()).map(|s| note(s.into())))
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
                        .flex_none()
                        .size(px(16.))
                        .text_color(secondary),
                )
                .child(title(format!("@{range}").into()))
                .child(note(reach.into()))
                .when_some(self.broadcast_hint.clone(), |row, badge| {
                    row.child(
                        div()
                            .flex_none()
                            .ml_auto()
                            .px_1p5()
                            .py(px(1.))
                            .rounded_sm()
                            .border_1()
                            .border_color(secondary.opacity(0.35))
                            .text_xs()
                            .text_color(secondary)
                            .child(badge),
                    )
                })
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
            Entry::Slash(command) => {
                let attribution = command.app_name.clone().or_else(|| {
                    command.is_duplicate.then(|| {
                        if command.is_builtin {
                            "Built-in".to_owned()
                        } else {
                            command
                                .app_id
                                .as_deref()
                                .map(|id| format!("App {id}"))
                                .unwrap_or_else(|| "App".to_owned())
                        }
                    })
                });
                v_flex()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(title(format!("/{}", command.name).into()))
                            .when(!command.usage.is_empty(), |row| {
                                row.child(note(command.usage.clone().into()))
                            })
                            .when_some(attribution, |row, app| {
                                row.child(
                                    div()
                                        .ml_auto()
                                        .when(command.is_duplicate, |pill| {
                                            pill.px_1p5()
                                                .py(px(1.))
                                                .rounded_sm()
                                                .border_1()
                                                .border_color(secondary.opacity(0.35))
                                                .font_weight(FontWeight::MEDIUM)
                                        })
                                        .text_xs()
                                        .text_color(secondary)
                                        .child(app),
                                )
                            }),
                    )
                    .when(!command.description.is_empty(), |col| {
                        col.child(
                            div()
                                .text_xs()
                                .text_color(secondary)
                                .child(command.description.clone()),
                        )
                    })
            }
        }
    }
}
