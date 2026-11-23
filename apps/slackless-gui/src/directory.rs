//! The workspace directory (the sidebar's "Directories" row): Slack's People and Channels
//! tabs with a search over each. A person opens their profile panel; a channel opens, or
//! offers the channel browser's local Join for one not joined yet. The filters and rows live
//! in [`crate::directory_rows`].
//!
//! `DirectoryView::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<DirectoryView>`
//! `DirectoryView::set_fixtures(&mut self, Arc<Fixtures>, &mut Context<Self>)` (live reloads)
//! emits `DirectoryEvent::OpenProfile(UserId)` / `Open(ChannelId)` / `Join(ChannelId)` /
//! `Close`

use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px,
};
use slackless_core::types::{ChannelId, UserId};

use crate::directory_rows::{channel_row, channels, people, person_row, usergroup_row, usergroups};
use crate::fixtures::Fixtures;

/// Rows listed per tab at once.
const MAX_ROWS: usize = 30;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DirectoryTab {
    People,
    Channels,
    UserGroups,
}

pub enum DirectoryEvent {
    /// A person row: open their profile panel.
    OpenProfile(UserId),
    /// A joined channel row: open it.
    Open(ChannelId),
    /// An unjoined channel's Join (already marked joined here).
    Join(ChannelId),
    /// Create a new `@handle` user group (`usergroups.create`, `F-COLLAB-1161`, `1162`).
    CreateUsergroup {
        name: String,
        handle: String,
        description: String,
    },
    Close,
}

pub struct DirectoryView {
    fixtures: Arc<Fixtures>,
    tab: DirectoryTab,
    filter: Entity<TextareaState>,
    /// Channels joined from this directory during the session.
    joined: HashSet<ChannelId>,
    /// Expanded user group id showing clickable member and default channel chips.
    expanded_group: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DirectoryEvent> for DirectoryView {}

impl DirectoryView {
    pub fn new(
        fixtures: Arc<Fixtures>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<DirectoryView> {
        cx.new(|cx| {
            let filter = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Search the directory")
                    .auto_grow(1, 1)
            });
            let subscriptions = vec![
                cx.subscribe(&filter, |_: &mut DirectoryView, _, _: &InputEvent, cx| {
                    cx.notify()
                }),
            ];
            DirectoryView {
                fixtures,
                tab: DirectoryTab::People,
                filter,
                joined: HashSet::new(),
                expanded_group: None,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Live refresh: search the directory in `fixtures`, keeping the typed filter.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    /// The snapshot the rows read.
    pub(super) fn fixtures(&self) -> &Arc<Fixtures> {
        &self.fixtures
    }

    fn needle(&self, cx: &Context<Self>) -> String {
        self.filter.read(cx).value().trim().to_lowercase()
    }

    /// Mark `channel` joined here and hand the join to the root.
    pub(super) fn join(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        self.joined.insert(channel.clone());
        cx.emit(DirectoryEvent::Join(channel));
        cx.notify();
    }

    pub(super) fn is_group_expanded(&self, id: &str) -> bool {
        self.expanded_group.as_deref() == Some(id)
    }

    pub(super) fn toggle_group(&mut self, id: String, cx: &mut Context<Self>) {
        if self.expanded_group.as_deref() == Some(id.as_str()) {
            self.expanded_group = None;
        } else {
            self.expanded_group = Some(id);
        }
        cx.notify();
    }
}

impl Render for DirectoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, foreground, link) = (
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.link,
        );
        let needle = self.needle(cx);
        let fixtures = self.fixtures.clone();
        let (tab, joined) = (self.tab, &self.joined);
        let (matching_people, matching_channels, matching_groups) = (
            people(&fixtures, &needle),
            channels(&fixtures, &needle),
            usergroups(&fixtures, &needle),
        );
        let count = match tab {
            DirectoryTab::People => matching_people.len(),
            DirectoryTab::Channels => matching_channels.len(),
            DirectoryTab::UserGroups => matching_groups.len(),
        };
        let rows: Vec<_> = match tab {
            DirectoryTab::People => matching_people
                .iter()
                .take(MAX_ROWS)
                .enumerate()
                .map(|(ix, user)| person_row(self, ix, user, cx))
                .collect(),
            DirectoryTab::Channels => matching_channels
                .iter()
                .take(MAX_ROWS)
                .enumerate()
                .map(|(ix, channel)| channel_row(ix, channel, joined, cx))
                .collect(),
            DirectoryTab::UserGroups => matching_groups
                .iter()
                .take(MAX_ROWS)
                .enumerate()
                .map(|(ix, group)| usergroup_row(self, ix, group, cx))
                .collect(),
        };
        let tab_row = [
            (DirectoryTab::People, "People", "directory-tab-People"),
            (DirectoryTab::Channels, "Channels", "directory-tab-Channels"),
            (
                DirectoryTab::UserGroups,
                "User groups",
                "directory-tab-UserGroups",
            ),
        ]
        .map(|(tab, label, id)| {
            let active = tab == self.tab;
            div()
                .id(SharedString::from(id))
                .h_full()
                .flex()
                .items_center()
                .px_1()
                .cursor_pointer()
                .text_sm()
                .text_color(if active { foreground } else { muted })
                .when(active, |tab| {
                    tab.font_weight(FontWeight::SEMIBOLD)
                        .border_b_2()
                        .border_color(link)
                })
                .hover(|el| el.text_color(foreground))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = tab;
                    cx.notify();
                }))
        });

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .h(px(46.))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .font_weight(FontWeight::BOLD)
                            .child(
                                Icon::new(IconName::BookUser)
                                    .text_color(foreground)
                                    .size(px(16.)),
                            )
                            .child("Directory"),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .when(tab == DirectoryTab::UserGroups, |row| {
                                let this_ug = cx.weak_entity();
                                row.child(
                                    Button::new("directory-new-usergroup")
                                        .primary()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Plus)
                                        .label("New user group")
                                        .on_click(move |_, window, cx| {
                                            open_new_usergroup_dialog(this_ug.clone(), window, cx);
                                        }),
                                )
                            })
                            .child(
                                Button::new("directory-close")
                                    .ghost()
                                    .with_size(Size::Small)
                                    .icon(IconName::X)
                                    .tooltip("Close")
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(DirectoryEvent::Close)),
                                    ),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .items_stretch()
                    .flex_none()
                    .px_4()
                    .gap_5()
                    .h(px(38.))
                    .border_b_1()
                    .border_color(border)
                    .children(tab_row),
            )
            .child(
                h_flex()
                    .flex_none()
                    .mx_4()
                    .my_2()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .child(Icon::new(IconName::Search).text_color(muted).size(px(14.)))
                    .child(
                        div().flex_1().min_w_0().child(
                            Textarea::new(&self.filter)
                                .appearance(false)
                                .bordered(false),
                        ),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .flex_none()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("{count} results")),
            )
            .child(
                v_flex()
                    .id("directory-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(count == 0, |list| {
                        list.child(
                            div()
                                .p_6()
                                .text_sm()
                                .text_color(muted)
                                .child("Nothing matches your search."),
                        )
                    }),
            )
    }
}

fn open_new_usergroup_dialog(
    view: gpui_kit::gpui::WeakEntity<DirectoryView>,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::gpui::Focusable as _;
    let name_in = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder("Group name (e.g. Checkout Engineers)")
            .auto_grow(1, 1)
    });
    let handle_in = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder("Handle without @ (e.g. checkout-eng)")
            .auto_grow(1, 1)
    });
    let desc_in = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder("Purpose / description (optional)")
            .auto_grow(1, 2)
    });
    name_in.read(cx).focus_handle(cx).focus(window, cx);
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = cx.theme();
        let (border, muted, bg) = (theme.border, theme.muted_foreground, theme.background);
        let (n_c, h_c, d_c, v_c) = (
            name_in.clone(),
            handle_in.clone(),
            desc_in.clone(),
            view.clone(),
        );
        let field = |label: &'static str, st: &Entity<TextareaState>| {
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted)
                        .child(label),
                )
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .bg(bg)
                        .child(Textarea::new(st).appearance(false).bordered(false)),
                )
        };
        dialog.title("Create user group").w(px(420.)).child(
            v_flex()
                .gap_3()
                .p_4()
                .child(field("Name", &name_in))
                .child(field("Handle (@handle)", &handle_in))
                .child(field("Description", &desc_in))
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("ug-create-cancel")
                                .outline()
                                .with_size(Size::Small)
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("ug-create-submit")
                                .primary()
                                .with_size(Size::Small)
                                .label("Create group")
                                .on_click(move |_, window, cx| {
                                    let name = n_c.read(cx).value().trim().to_owned();
                                    let handle = h_c
                                        .read(cx)
                                        .value()
                                        .trim()
                                        .trim_start_matches('@')
                                        .to_owned();
                                    let description = d_c.read(cx).value().trim().to_owned();
                                    if name.is_empty() {
                                        return;
                                    }
                                    window.close_dialog(cx);
                                    let _ = v_c.update(cx, |_, cx| {
                                        cx.emit(DirectoryEvent::CreateUsergroup {
                                            name,
                                            handle,
                                            description,
                                        });
                                    });
                                }),
                        ),
                ),
        )
    });
}
