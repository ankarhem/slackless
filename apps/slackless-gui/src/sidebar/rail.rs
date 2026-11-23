//! Workspace switcher rail, tab rail, focus card, and sidebar header.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, Context, Div, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    deferred, div, img, px, transparent_black,
};
use slackless_core::types::{Conversation, Team};

use super::{
    NavItem, SWITCHER_WIDTH, Sidebar, SidebarEvent, TAB_RAIL_WIDTH, team_icon_url, team_initials,
};
use crate::status::{FOCUS_DURATIONS, self_avatar};

impl Sidebar {
    /// Slack's workspace switcher: the column left of the tab rail with one tile per signed-in
    /// workspace, the active one ringed.
    pub(crate) fn workspace_switcher(&self, cx: &mut Context<Self>) -> Div {
        v_flex()
            .flex_none()
            .h_full()
            .w(px(SWITCHER_WIDTH))
            .items_center()
            .pt(px(6.))
            .gap_2()
            .children(
                self.fixtures
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(index, team)| self.workspace_tile(index, team, cx))
                    .collect::<Vec<_>>(),
            )
    }

    /// Slack's tab rail, the left edge of the workspace panel the root rounds: the nav
    /// destinations (Home and DMs carry an unread dot, Activity its unread count) over the
    /// control strip (create, focus mode, the user's avatar).
    pub(crate) fn tab_rail(&self, cx: &mut Context<Self>) -> Div {
        let watermark = self.read_watermark();
        let unread = |conversations: &[Conversation]| {
            self.total_unread(conversations, watermark.as_ref()).count > 0
        };
        let home_dot = unread(&self.fixtures.channels) || unread(&self.joined);
        let focus = self.self_state.focused();
        let dm_dot = unread(&self.fixtures.dms);
        // Owned color copies: `cx.theme()` borrows `cx`, so nothing may outlive the
        // statement that reads it across the `&mut Context` calls below.
        let theme = cx.theme();
        let (outline, dark, avatar_ring) = (
            theme.sidebar_border,
            crate::theme::sidebar_is_dark(cx),
            theme.sidebar,
        );
        let rail_bg = crate::theme::tab_rail_wash(cx);
        let mut rail = v_flex()
            .flex_none()
            .h_full()
            .w(px(TAB_RAIL_WIDTH))
            .items_center()
            .pt_2()
            .pb(px(20.))
            .gap_1()
            .bg(rail_bg)
            .rounded_tl(px(6.))
            .rounded_bl(px(6.))
            // Dark Slack outlines the panel; light Slack ends the rail with a hairline.
            .when(dark, |rail| {
                rail.border_t_1()
                    .border_l_1()
                    .border_b_1()
                    .border_color(outline)
            })
            .when(!dark, |rail| rail.border_r_1().border_color(outline));
        for item in self.nav_prefs.ordered_items() {
            if !self.nav_prefs.shows(item) {
                continue;
            }
            if item == NavItem::Admin {
                rail = rail.child(div().my_1().w(px(28.)).h(px(1.)).bg(outline));
            }
            let (badge, dot) = match item {
                NavItem::Home => (0, home_dot),
                NavItem::Dms => (0, dm_dot),
                NavItem::Activity => (self.activity_unread, false),
                _ => (0, false),
            };
            // Focus mode trades the count for the subtle dot.
            let (badge, dot) = if focus {
                (0, badge > 0 || dot)
            } else {
                (badge, dot)
            };
            rail = rail.child(self.nav_tile(item, badge, dot, cx));
        }
        let avatar = self_avatar(&self.fixtures, &self.self_state, 36., avatar_ring, cx);
        let create_this = cx.entity().downgrade();
        let create_fill = crate::theme::rail_fill(cx);
        let sidebar_fg = cx.theme().sidebar_foreground;
        let create_fg = sidebar_fg.opacity(0.8);
        let create_bg = sidebar_fg.opacity(create_fill);
        let create_btn = Button::new("rail-create")
            .ghost()
            .with_size(Size::Small)
            .size(px(36.))
            .p_0()
            .rounded_full()
            .bg(create_bg)
            .child(
                Icon::new(IconName::Plus)
                    .text_color(create_fg)
                    .size(px(18.)),
            )
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let t1 = create_this.clone();
                let t2 = create_this.clone();
                let t3 = create_this.clone();
                let t4 = create_this.clone();
                menu.label("Create")
                    .item(
                        PopupMenuItem::new("New message")
                            .icon(IconName::SquarePen)
                            .on_click(move |_, _, cx| {
                                if let Some(this) = t1.upgrade() {
                                    this.update(cx, |_, cx| cx.emit(SidebarEvent::OpenCompose));
                                }
                            }),
                    )
                    .item(
                        PopupMenuItem::new("Create a channel")
                            .icon(IconName::Hash)
                            .on_click(move |_, _, cx| {
                                if let Some(this) = t2.upgrade() {
                                    this.update(cx, |_, cx| cx.emit(SidebarEvent::CreateChannel));
                                }
                            }),
                    )
                    .item(
                        PopupMenuItem::new("Browse channels")
                            .icon(IconName::Compass)
                            .on_click(move |_, _, cx| {
                                if let Some(this) = t3.upgrade() {
                                    this.update(cx, |_, cx| cx.emit(SidebarEvent::BrowseChannels));
                                }
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("Create a channel section")
                            .icon(IconName::FolderPlus)
                            .on_click(move |_, window, cx| {
                                if let Some(this) = t4.upgrade() {
                                    this.update(cx, |sidebar, cx| {
                                        sidebar.open_section_dialog(window, cx);
                                    });
                                }
                            }),
                    )
            });
        rail.child(div().flex_1()).child(
            v_flex()
                .items_center()
                .gap_4()
                .child(create_btn)
                .child(self.focus_button(cx))
                .child(
                    div()
                        .id("rail-avatar")
                        .cursor_pointer()
                        .child(avatar)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenUserMenu))),
                ),
        )
    }

    /// One 36px round control-strip button ("+" create, focus mode's moon); `active` gives
    /// it Slack's pressed look.
    pub(super) fn rail_button(
        &self,
        id: &'static str,
        icon: IconName,
        active: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let fill = crate::theme::rail_fill(cx);
        let (fg, bg) = if active {
            (theme.muted_foreground, theme.background)
        } else {
            (
                theme.sidebar_foreground.opacity(0.8),
                theme.sidebar_foreground.opacity(fill),
            )
        };
        let hover = theme.sidebar_foreground.opacity(fill + 0.1);
        div()
            .id(id)
            .size(px(36.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(bg)
            .cursor_pointer()
            .when(!active, |button| button.hover(|button| button.bg(hover)))
            .child(Icon::new(icon).text_color(fg).size(px(18.)))
    }

    /// Slack's focus mode control: the moon turns focus mode on and off. Off, its tooltip
    /// explains it; on, hovering it (or the card) shows the focus card.
    pub(super) fn focus_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.self_state.focused();
        let card_open = focused && (self.focus_hover.0 || self.focus_hover.1);
        let muted = cx.theme().muted_foreground;
        let button = self
            .rail_button("rail-focus", IconName::Moon, focused, cx)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.focus_hover.0 = *hovered;
                cx.notify();
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                this.focus_hover = (false, false);
                cx.emit(SidebarEvent::ToggleFocus);
            }))
            .when(!focused, |button| {
                button.tooltip(move |window, cx| {
                    Tooltip::element(move |_, _| {
                        v_flex().child("Turn on focus mode").child(
                            div()
                                .text_color(muted)
                                .child("Pause notifications and reduce badging and bolding."),
                        )
                    })
                    .build(window, cx)
                })
            });
        div().relative().child(button).when(card_open, |anchor| {
            anchor.child(deferred(self.focus_card(cx)).with_priority(1))
        })
    }

    /// The card focus mode shows beside the moon: what is paused and Slack's Duration choices.
    /// Its transparent left padding bridges the gap to the button, so the pointer can cross it.
    pub(super) fn focus_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (popover, border, muted, hover, fg) = (
            theme.popover,
            theme.border,
            theme.muted_foreground,
            theme.accent,
            theme.popover_foreground,
        );
        let preset = self.self_state.focus_preset;
        let rows = FOCUS_DURATIONS
            .iter()
            .enumerate()
            .map(|(ix, (label, _))| {
                h_flex()
                    .id(("focus-duration", ix))
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_1()
                    .cursor_pointer()
                    .hover(|row| row.bg(hover))
                    .child(
                        div().w(px(16.)).children(
                            (ix == preset)
                                .then(|| Icon::new(IconName::Check).text_color(fg).size(px(14.))),
                        ),
                    )
                    .child(*label)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SidebarEvent::FocusDuration(ix));
                    }))
            })
            .collect::<Vec<_>>();
        div()
            .id("focus-card")
            .absolute()
            .left(px(36.))
            .bottom(px(-8.))
            .pl(px(12.))
            .occlude()
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.focus_hover.1 = *hovered;
                cx.notify();
            }))
            .child(
                v_flex()
                    .w(px(260.))
                    .py_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .shadow_lg()
                    .text_sm()
                    .text_color(fg)
                    .child(
                        v_flex()
                            .px_4()
                            .pb_2()
                            .child(div().font_weight(FontWeight::BOLD).child("Focus mode"))
                            .child(div().text_color(muted).child(self.self_state.focus_label())),
                    )
                    .child(div().my_1().h(px(1.)).bg(border))
                    .child(
                        div()
                            .px_4()
                            .py_1()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .child("Duration"),
                    )
                    .children(rows),
            )
    }

    pub(super) fn workspace_tile(
        &self,
        index: usize,
        team: &Team,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let active = index == self.active_workspace;
        let face = match team_icon_url(team) {
            Some(url) => img(SharedString::from(url))
                .size_full()
                .rounded(px(8.))
                .into_any_element(),
            None => div().child(team_initials(&team.name)).into_any_element(),
        };
        let (tile_bg, tile_fg) = if active {
            (theme.sidebar_primary, theme.sidebar_primary_foreground)
        } else {
            (
                theme.sidebar_accent.opacity(0.25),
                theme.sidebar_accent_foreground,
            )
        };
        let ring = theme.sidebar_foreground.opacity(0.7);
        let hover_ring = theme.sidebar_foreground.opacity(0.3);
        let badge = self
            .workspace_badges
            .get(index)
            .copied()
            .unwrap_or_default();
        let (danger, danger_fg, badge_ring) =
            (theme.danger, theme.danger_foreground, theme.title_bar);
        let pill = (!active && badge.mentions > 0).then(|| {
            let label = if badge.mentions > 99 {
                "99+".to_owned()
            } else {
                badge.mentions.to_string()
            };
            div()
                .absolute()
                .top(px(-2.))
                .right(px(-2.))
                .min_w(px(16.))
                .h(px(16.))
                .px_1()
                .rounded_full()
                .border_2()
                .border_color(badge_ring)
                .bg(danger)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.))
                .font_weight(FontWeight::BOLD)
                .text_color(danger_fg)
                .child(label)
        });
        let dot = (!active && badge.mentions == 0 && badge.has_unreads).then(|| {
            div()
                .absolute()
                .top(px(2.))
                .right(px(2.))
                .size(px(8.))
                .rounded_full()
                .border_1()
                .border_color(badge_ring)
                .bg(theme.sidebar_foreground)
        });
        let ws_len = self.fixtures.workspaces.len();
        let this_ws = cx.entity().downgrade();
        // Slack rings the active workspace's icon; the others take a faint ring on hover.
        div()
            .id(SharedString::from(format!("workspace-{index}")))
            .relative()
            .size(px(44.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(11.))
            .border_2()
            .border_color(if active { ring } else { transparent_black() })
            .when(!active, |tile| {
                tile.hover(|tile| tile.border_color(hover_ring))
            })
            .cursor_pointer()
            .child(
                div()
                    .size(px(34.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .overflow_hidden()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .bg(tile_bg)
                    .text_color(tile_fg)
                    .child(face),
            )
            .children(pill)
            .children(dot)
            .on_click(cx.listener(move |this, _, _, cx| {
                if index != this.active_workspace {
                    cx.emit(SidebarEvent::SwitchWorkspace(index));
                }
            }))
            .context_menu(move |mut menu, _, _| {
                if ws_len > 1 {
                    if index > 0 {
                        let this = this_ws.clone();
                        menu = menu.item(
                            PopupMenuItem::new("Move up")
                                .icon(IconName::ArrowUp)
                                .on_click(move |_, _, cx| {
                                    let _ = this.update(cx, |_, cx| {
                                        cx.emit(SidebarEvent::ReorderWorkspace {
                                            from: index,
                                            to: index - 1,
                                        });
                                    });
                                }),
                        );
                    }
                    if index + 1 < ws_len {
                        let this = this_ws.clone();
                        menu = menu.item(
                            PopupMenuItem::new("Move down")
                                .icon(IconName::ArrowDown)
                                .on_click(move |_, _, cx| {
                                    let _ = this.update(cx, |_, cx| {
                                        cx.emit(SidebarEvent::ReorderWorkspace {
                                            from: index,
                                            to: index + 1,
                                        });
                                    });
                                }),
                        );
                    }
                }
                menu
            })
    }

    pub(super) fn workspace_header(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let team = &self.fixtures.workspaces[self.active_workspace];
        let dim = theme.sidebar_foreground.opacity(0.6);
        let hover = theme.sidebar_foreground.opacity(0.08);
        // Slack's "Norce ⌄": the chevron rides right after the name, the icons sit right.
        h_flex()
            .flex_none()
            .px_4()
            .pt_3()
            .pb_1()
            .gap_1()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .truncate()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .child(team.name.clone()),
                    )
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .flex_none()
                            .text_color(dim)
                            .size(px(14.)),
                    ),
            )
            .child(
                self.header_icon("ws-settings", IconName::Settings, dim, hover)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings_menu_open = !this.settings_menu_open;
                        cx.notify();
                    })),
            )
            .child(
                self.header_icon("ws-compose", IconName::SquarePen, dim, hover)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenCompose))),
            )
    }

    /// One 28px hover-square for the workspace header's gear / compose icons.
    pub(super) fn header_icon(
        &self,
        id: &'static str,
        icon: IconName,
        dim: Hsla,
        hover: Hsla,
    ) -> Stateful<Div> {
        div()
            .id(SharedString::from(id))
            .flex_none()
            .size(px(28.))
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(dim)
            .hover(|el| el.bg(hover))
            .child(Icon::new(icon).text_color(dim).size(px(20.)))
    }

    /// One of the sidebar's slim find fields: the Home list's "Find a conversation…" or the
    /// DM list's "Find a DM…". Slack's field is a slim 28px box led by its text-search glyph.
    pub(super) fn find_field(&self, field: &Entity<TextareaState>, cx: &Context<Self>) -> Div {
        let fg = cx.theme().sidebar_foreground;
        let border = cx.theme().sidebar_border;
        let dim = fg.opacity(0.65);
        let empty = field.read(cx).value().is_empty();
        let placeholder = if *field == self.dm_find {
            "Find a DM"
        } else {
            "Find a conversation\u{2026}"
        };
        // Slack's field is a slim 28px box led by its text-search glyph.
        h_flex()
            .mx_2()
            .mt_1()
            .flex_none()
            .h(px(28.))
            .overflow_hidden()
            .items_center()
            .gap_1p5()
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(border)
            .text_color(fg)
            .child(
                Icon::new(IconName::TextSearch)
                    .text_color(dim)
                    .size(px(16.)),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(13.))
                    .text_color(fg)
                    .when(empty, |wrap| {
                        wrap.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .text_color(dim)
                                .child(placeholder),
                        )
                    })
                    .child(Textarea::new(field).appearance(false).bordered(false)),
            )
    }

    /// One rail destination: icon over its label, Slack-rail style, with an optional
    /// count badge riding the icon's top-right corner.
    pub(super) fn nav_tile(
        &self,
        item: NavItem,
        badge_count: u32,
        dot: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let active = self.active_nav == item;
        let ring = theme.sidebar;
        let fg = theme.sidebar_foreground;
        let tile_bg = fg.opacity(crate::theme::rail_fill(cx));
        let hover = fg.opacity(0.08);
        let (badge_bg, badge_fg) = crate::theme::mention_badge(cx);
        v_flex()
            .id(SharedString::from(format!("nav-{}", item.label())))
            .w_full()
            .items_center()
            .gap_1()
            .py_1()
            .text_color(fg)
            .cursor_pointer()
            .child(
                div()
                    .relative()
                    .size(px(36.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .when(active, |tile| tile.bg(tile_bg))
                    .when(!active, |tile| tile.hover(|tile| tile.bg(hover)))
                    .child(Icon::new(item.icon()).text_color(fg).size(px(20.)))
                    .when(dot, |tile| {
                        tile.child(
                            div()
                                .absolute()
                                .top(px(1.))
                                .right(px(1.))
                                .size(px(10.))
                                .rounded_full()
                                .border_2()
                                .border_color(ring)
                                .bg(badge_bg),
                        )
                    })
                    .when(badge_count > 0, |tile| {
                        tile.child(
                            div()
                                .absolute()
                                .top(px(-4.))
                                .right(px(-6.))
                                .min_w(px(18.))
                                .h(px(18.))
                                .px_1()
                                .rounded_full()
                                .bg(badge_bg)
                                .text_color(badge_fg)
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(badge_count.to_string()),
                        )
                    }),
            )
            .when(!self.nav_prefs.icons_only, |tile| {
                tile.child(
                    div()
                        .w_full()
                        .px_1()
                        .text_center()
                        .text_size(px(11.))
                        .line_height(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(item.label()),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| match item {
                // Both destinations' pages live in the root, which opens them.
                NavItem::Activity => cx.emit(SidebarEvent::OpenActivity),
                NavItem::Files => cx.emit(SidebarEvent::OpenFiles),
                _ => this.show_nav(item, cx),
            }))
    }
}
