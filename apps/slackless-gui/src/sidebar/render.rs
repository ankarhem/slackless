//! `Render` implementation for [`Sidebar`]: Home list, shortcut rows, floating unread pills, and gear menu.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::types::Conversation;

use super::helpers::{ListChild, count_pill, hidden_unreads};
use super::{HomePage, NavItem, Sidebar, SidebarEvent, Unread};
use crate::sidebar_menu::workspace_menu;

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.step_scroll(window);
        // Owned color copies: `cx.theme()` borrows `cx`, so nothing may outlive the
        // statement that reads it across the `&mut Context` calls below.
        let sidebar_bg = cx.theme().sidebar;
        let sidebar_border = cx.theme().sidebar_border;
        let sidebar_fg = cx.theme().sidebar_foreground;
        let focus = self.self_state.focused();
        let watermark = self.read_watermark();
        let channels_cow: std::borrow::Cow<'_, [Conversation]> = if self.joined.is_empty() {
            std::borrow::Cow::Borrowed(&self.fixtures.channels)
        } else {
            let mut v = Vec::with_capacity(self.fixtures.channels.len() + self.joined.len());
            v.extend(self.fixtures.channels.iter().cloned());
            v.extend(self.joined.iter().cloned());
            std::borrow::Cow::Owned(v)
        };
        let channels: &[Conversation] = &channels_cow;
        let channel_unread = self.total_unread(channels, watermark.as_ref());
        let dm_unread = self.total_unread(&self.fixtures.dms, watermark.as_ref());
        let mentions = channel_unread.mentions + dm_unread.mentions;
        let total = Unread {
            count: channel_unread.count + dm_unread.count,
            mentions,
        };

        let list = v_flex()
            .id("sidebar")
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll(cx.listener(|this, _, _, cx| {
                if !this.last_unread_rows.is_empty() {
                    let next = hidden_unreads(&this.scroll, &this.last_unread_rows);
                    if next != this.last_hidden_pills {
                        cx.notify();
                    }
                }
            }))
            .pb_2();
        let (mut below, mut above) = (None, None);
        let list = match self.active_nav {
            NavItem::Dms => list.child(self.dm_list(watermark.as_ref(), cx)),
            // The Activity page replaces the whole sidebar column; the empty list keeps
            // this match exhaustive (the rail-only render below never reaches it).
            NavItem::Activity => list,
            NavItem::Files => list.child(self.files_list(cx)),
            NavItem::Home | NavItem::Todos | NavItem::Agents | NavItem::Admin => {
                let mut children = vec![
                    self.workspace_header(cx).into_any_element(),
                    self.find_field(&self.find, cx).into_any_element(),
                    div()
                        .mx_2()
                        .my_2()
                        .h(px(1.))
                        .bg(sidebar_border)
                        .into_any_element(),
                ];
                for row in [
                    self.nav_row(
                        super::home::NavRowArgs {
                            id: "unreads",
                            icon: IconName::Inbox,
                            label: "Unreads",
                            bold: total.count > 0 && !focus,
                            active: self.active_home_page == Some(HomePage::Unreads),
                            trailing: None,
                        },
                        cx,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenUnreads))),
                    // Slack bolds Threads while followed threads have unread replies and
                    // counts the ones mentioning you in a red pill.
                    self.nav_row(
                        super::home::NavRowArgs {
                            id: "threads",
                            icon: IconName::MessageSquareText,
                            label: "Threads",
                            bold: self.threads_unread.0 && !focus,
                            active: self.active_home_page == Some(HomePage::Threads),
                            trailing: None,
                        },
                        cx,
                    )
                    .when(self.threads_unread.1 > 0 && !focus, |row| {
                        row.child(count_pill(self.threads_unread.1, true, cx))
                    })
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenThreads))),
                    {
                        let mut active_people: Vec<slackless_core::types::UserId> = Vec::new();
                        for users in self.huddles.values() {
                            for uid in users {
                                if !active_people.contains(uid) && active_people.len() < 2 {
                                    active_people.push(uid.clone());
                                }
                            }
                        }
                        self.nav_row(
                            super::home::NavRowArgs {
                                id: "huddles",
                                icon: IconName::Headphones,
                                label: "Huddles",
                                bold: false,
                                active: self.active_home_page == Some(HomePage::Huddles),
                                trailing: None,
                            },
                            cx,
                        )
                        .when(!active_people.is_empty(), |row| {
                            row.child(h_flex().items_center().children(
                                active_people.iter().enumerate().map(|(ix, uid)| {
                                    div()
                                        .when(ix > 0, |el| el.ml(px(-4.)))
                                        .rounded(px(4.))
                                        .overflow_hidden()
                                        .border_1()
                                        .border_color(sidebar_bg)
                                        .child(crate::avatars::user_chip(&self.fixtures, uid, 18.))
                                }),
                            ))
                        })
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenHuddles)))
                    },
                    self.nav_row(
                        super::home::NavRowArgs {
                            id: "drafts",
                            icon: IconName::Send,
                            label: "Drafts & sent",
                            bold: false,
                            active: self.active_home_page == Some(HomePage::Drafts),
                            trailing: (self.draft_count > 0).then(|| self.draft_count.to_string()),
                        },
                        cx,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenDrafts))),
                    self.nav_row(
                        super::home::NavRowArgs {
                            id: "saved",
                            icon: IconName::Bookmark,
                            label: "Saved",
                            bold: false,
                            active: self.active_home_page == Some(HomePage::Saved),
                            trailing: (self.later_count > 0).then(|| self.later_count.to_string()),
                        },
                        cx,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenLater))),
                    self.nav_row(
                        super::home::NavRowArgs {
                            id: "directories",
                            icon: IconName::BookUser,
                            label: "Directories",
                            bold: false,
                            active: self.active_home_page == Some(HomePage::Directories),
                            trailing: None,
                        },
                        cx,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenDirectories))),
                ] {
                    children.push(row.into_any_element());
                }
                children.push(
                    div()
                        .mx_4()
                        .my_2()
                        .h(px(1.))
                        .bg(sidebar_border)
                        .into_any_element(),
                );
                let mut unread_rows = Vec::new();
                for (child, kind) in self.section_list(channels, watermark.as_ref(), cx) {
                    if let ListChild::Unread(mentions) = kind {
                        unread_rows.push((children.len(), mentions));
                    }
                    children.push(child);
                }
                // The handle records the list's child bounds during prepaint, one frame
                // behind this render; ask for that frame before the pills can trust it.
                if self.scroll.bounds_for_item(children.len() - 1).is_none() {
                    cx.notify();
                }
                (below, above) = hidden_unreads(&self.scroll, &unread_rows);
                self.last_unread_rows = unread_rows;
                self.last_hidden_pills = (below, above);
                list.children(children)
            }
        };

        let dark_sidebar = crate::theme::sidebar_is_dark(cx);
        h_flex()
            .relative()
            .size_full()
            .text_color(sidebar_fg)
            .text_sm()
            // The Activity page spans the whole main area right of the rail, so its own
            // card replaces this sidebar's list column while it is the active destination.
            .when(self.active_nav != NavItem::Activity, |sidebar| {
                sidebar.child(
                    // A relative wrapper, so the pills can float over the list's edges. Dark
                    // or tinted sidebars divide from the conversation with a hairline; light
                    // default Slack lets the conversation's shadow do it.
                    div()
                        .relative()
                        .flex_1()
                        .min_w(px(0.))
                        .h_full()
                        .bg(sidebar_bg)
                        .when(dark_sidebar, |list| {
                            list.border_r_1().border_color(sidebar_border)
                        })
                        .child(list)
                        // Light Slack's conversation casts its shadow over the light sidebar's edge.
                        .when(!dark_sidebar, |list| {
                            list.child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .right_0()
                                    .w(px(16.))
                                    .bg(crate::theme::panel_shadow_edge()),
                            )
                        })
                        .when_some(below, |list, pill| {
                            list.child(
                                div()
                                    .absolute()
                                    .bottom_2()
                                    .left_0()
                                    .right_0()
                                    .flex()
                                    .justify_center()
                                    .child(self.unreads_pill(pill, cx)),
                            )
                        })
                        .when_some(above, |list, pill| {
                            list.child(
                                div()
                                    .absolute()
                                    .top_2()
                                    .left_0()
                                    .right_0()
                                    .flex()
                                    .justify_center()
                                    .child(self.unreads_pill(pill, cx)),
                            )
                        }),
                )
            })
            .when(self.settings_menu_open, |sidebar| {
                let toggle = cx.weak_entity();
                let on_active_only = move |_: &mut Window, cx: &mut gpui_kit::gpui::App| {
                    toggle
                        .update(cx, |sidebar: &mut Sidebar, cx| {
                            sidebar.active_only = !sidebar.active_only;
                            sidebar.settings_menu_open = false;
                            cx.notify();
                        })
                        .ok();
                };
                let create = cx.weak_entity();
                let on_create_section = move |window: &mut Window, cx: &mut gpui_kit::gpui::App| {
                    create
                        .update(cx, |sidebar: &mut Sidebar, cx| {
                            sidebar.settings_menu_open = false;
                            sidebar.open_section_dialog(window, cx);
                        })
                        .ok();
                };
                let cleanup = cx.weak_entity();
                let on_cleanup_channels = move |_: &mut Window, cx: &mut gpui_kit::gpui::App| {
                    cleanup
                        .update(cx, |sidebar: &mut Sidebar, cx| {
                            sidebar.settings_menu_open = false;
                            cx.emit(SidebarEvent::OpenCleanupChannels);
                        })
                        .ok();
                };
                sidebar
                    .child(
                        div()
                            .id("workspace-menu-scrim")
                            .occlude()
                            .on_scroll(|_, _, cx| cx.stop_propagation())
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings_menu_open = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(44.))
                            .right(px(8.))
                            .child(workspace_menu(
                                self.active_only,
                                on_active_only,
                                on_create_section,
                                on_cleanup_channels,
                                cx,
                            )),
                    )
            })
    }
}
