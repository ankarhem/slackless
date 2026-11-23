//! Home sidebar sections, Files nav column, conversation rows, and More Unreads pill.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, AppContext as _, Context, Div, Edges, FontWeight, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, Point, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, anchored, deferred, div, point, px,
    relative,
};
use slackless_core::NotifyLevel;
use slackless_core::types::{
    ChannelId, ChannelSection, Conversation, ConversationKind, SectionKind, Ts,
};

use super::{
    HiddenUnreads, ListChild, PILL_CLEARANCE, SCROLL_ANIM, ScrollAnim, Sidebar, SidebarEvent,
    accepts_dms, count_pill, drop_target, focus_dot, org_tooltip, section_title,
};
use crate::avatars::conversation_avatar;
use crate::files_page::FilesSection;
use crate::sidebar_menu::{
    ChannelDrag, ChannelMenuInput, HiddenConversations, SectionSort, channel_menu, section_menu,
};

impl Sidebar {
    /// A sidebar section: a dim header row (section emoji or icon + name; click collapses)
    /// over its conversation rows, less the ones Slack's rules hide ([`Self::hidden_reason`]);
    /// a collapsed section keeps only its open conversation and the ones that mention you.
    /// An expanded Channels section ends with the "Add channels" row.
    /// The section flattens into direct children of the scroll list (each carrying the
    /// section's own padding) so its scroll handle can track every row.
    /// `(title, leading)` is the section's name and the icon or emoji before it.
    pub(super) fn section(
        &self,
        key: SharedString,
        (title, leading): (&str, Option<AnyElement>),
        conversations: &[Conversation],
        watermark: Option<&Ts>,
        section: Option<&ChannelSection>,
        cx: &mut Context<Self>,
    ) -> Vec<(AnyElement, ListChild)> {
        let add_channels = key.as_ref() == "channels"
            || self
                .section_set()
                .iter()
                .any(|section| section.kind == SectionKind::Channels && section.id == key.as_ref());
        let open = !self.collapsed.contains(&key);
        let filter = self.section_filter(&key);
        let mut hidden = HiddenConversations::default();
        let mut children = Vec::new();
        for conversation in conversations.iter() {
            let unread = self.unread_in(conversation, watermark);
            // A "Find a conversation…" query lists every match, collapsed or stale; otherwise
            // the rows leave out what Slack's rules hide, which the header's "More
            // conversations" popover lists.
            let listed = if self.find_query.is_empty() {
                let why = self.hidden_reason(conversation, unread, filter, open);
                if let Some(why) = why {
                    hidden.push(why, conversation.id.clone());
                }
                why.is_none()
            } else {
                self.fixtures
                    .conversation_label(conversation)
                    .to_lowercase()
                    .contains(&self.find_query)
            };
            if !listed {
                continue;
            }
            self.rows
                .borrow_mut()
                .push((conversation.id.clone(), unread.count > 0));
            let kind = if unread.count > 0 {
                ListChild::Unread(unread.mentions > 0)
            } else {
                ListChild::Plain
            };
            children.push((
                v_flex()
                    .px_2()
                    .child(self.conversation_row(conversation, watermark, section, &key, cx))
                    .into_any_element(),
                kind,
            ));
        }
        let header = self.section_header(
            key.clone(),
            (title, leading),
            section,
            conversations,
            &hidden,
            cx,
        );
        children.insert(
            0,
            (
                v_flex().px_2().pt_2().child(header).into_any_element(),
                ListChild::Plain,
            ),
        );
        if open && add_channels {
            children.push((
                v_flex()
                    .px_2()
                    .child(self.add_channels_row(cx))
                    .into_any_element(),
                ListChild::Plain,
            ));
        }
        children
    }

    /// Slack's section header: the 26px caret square with its own hover wash (the caret
    /// points down while open, right when collapsed), the name (bold while the collapsed
    /// section hides unread conversations), and - only while the pointer is over the
    /// header - the "More conversations" button (while the section hides any of `hidden`)
    /// and the ⋮ "More actions", each with its dark tooltip. Right-click opens the section menu; the
    /// ⋮ opens the same items with "Search in this section" first and the manage items
    /// inline. Header and rows take channel drops (a drop moves the channel here).
    pub(super) fn section_header(
        &self,
        key: SharedString,
        (title, leading): (&str, Option<AnyElement>),
        section: Option<&ChannelSection>,
        conversations: &[Conversation],
        hidden: &HiddenConversations,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let bright = theme.sidebar_foreground;
        let dim = theme.sidebar_foreground.opacity(0.7);
        let wash = theme.sidebar_foreground.opacity(0.08);
        let icon_wash = theme.sidebar_foreground.opacity(0.12);
        let open = !self.collapsed.contains(&key);
        let toggle = key.clone();
        let header_group = SharedString::from(format!("section-header-{key}"));
        let ids: Rc<[ChannelId]> = conversations.iter().map(|c| c.id.clone()).collect();
        let search_ids = ids.clone();

        // The ⋮ and right-click menus read the sidebar once, as they open.
        let menu_input = {
            let key = key.clone();
            let section = section.cloned();
            let title = title.to_owned();
            move |this: &gpui_kit::gpui::WeakEntity<Self>, cx: &gpui_kit::gpui::App| {
                this.upgrade().map(|sidebar| {
                    sidebar.read(cx).section_menu_input(
                        key.clone(),
                        section.clone(),
                        title.clone(),
                        &ids,
                    )
                })
            }
        };
        let more_menu = {
            let input = menu_input.clone();
            let this = cx.entity().downgrade();
            move |menu: PopupMenu,
                  window: &mut Window,
                  cx: &mut gpui_kit::gpui::Context<PopupMenu>| {
                let Some(input) = input(&this, cx) else {
                    return menu;
                };
                section_menu(menu, input, true, &this, window, cx)
            }
        };
        let context_menu = {
            let input = menu_input.clone();
            let this = cx.entity().downgrade();
            move |menu: PopupMenu,
                  window: &mut Window,
                  cx: &mut gpui_kit::gpui::Context<PopupMenu>| {
                let Some(input) = input(&this, cx) else {
                    return menu;
                };
                section_menu(menu, input, false, &this, window, cx)
            }
        };

        let caret = div()
            .id(SharedString::from(format!("section-caret-{key}")))
            .size(px(26.))
            .rounded(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .hover(|el| el.bg(icon_wash))
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(px(16.)),
            );
        let (search_key, press_key) = (key.clone(), key.clone());
        let more = Button::new(SharedString::from(format!("section-more-{key}")))
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::EllipsisVertical)
            .tooltip("More actions")
            .dropdown_menu_with_anchor(gpui_kit::gpui::Anchor::TopRight, more_menu);
        let popover = self
            .section_search
            .as_ref()
            .filter(|search| search.key == key)
            .map(|search| search.menu.clone());
        let search = (!hidden.is_empty()).then(|| {
            let button = Button::new(SharedString::from(format!("section-search-{key}")))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::TextSearch)
                .tooltip("More conversations")
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !std::mem::take(&mut this.search_was_open) {
                        this.open_more_conversations(&search_key, &search_ids, window, cx);
                    }
                }));
            div()
                .relative()
                // The press finds the popover open or not before the popover's own
                // outside-press closes it, so a second click closes it rather than reopening.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, _| {
                        this.search_was_open = this
                            .section_search
                            .as_ref()
                            .is_some_and(|search| search.key == press_key);
                    }),
                )
                .child(button)
                // Slack hangs the popover off the button's right edge, level with its top.
                .children(popover.map(|menu| {
                    div()
                        .absolute()
                        .top_0()
                        .left(relative(1.))
                        .ml(px(6.))
                        .child(
                            deferred(
                                anchored()
                                    .snap_to_window_with_margin(Edges::all(px(8.)))
                                    .child(menu),
                            )
                            .with_priority(2),
                        )
                }))
        });
        // A collapsed section that hides unread conversations bolds its name.
        let alert = !open && !hidden.unread.is_empty();

        let header = h_flex()
            .id(SharedString::from(format!("section-{key}")))
            .relative()
            .h(px(28.))
            .px_2()
            .gap_1()
            .items_center()
            .rounded(px(4.))
            .cursor_pointer()
            .text_color(dim)
            .hover(|row| row.bg(wash))
            .group(header_group.clone())
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.collapsed.remove(&toggle) {
                    this.collapsed.insert(toggle.clone());
                }
                cx.notify();
            }))
            // A custom section (or one with an emoji) leads with its own glyph in the caret's
            // place; every other header shows the caret square itself.
            .child(if let Some(leading) = leading {
                // Slack swaps a section's own glyph for the caret while the pointer is on it.
                let glyph = SharedString::from(format!("section-glyph-{key}"));
                div()
                    .relative()
                    .w(px(26.))
                    .flex()
                    .justify_center()
                    .group(glyph.clone())
                    .child(
                        div()
                            .group_hover(glyph.clone(), |style| style.invisible())
                            .child(leading),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .invisible()
                            .group_hover(glyph, |style| style.visible())
                            .child(caret),
                    )
                    .into_any_element()
            } else {
                caret.into_any_element()
            })
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .when(alert, |name| {
                        name.font_weight(FontWeight::BOLD).text_color(bright)
                    })
                    .child(title.to_owned()),
            )
            // The search and ⋮ buttons stay mounted and show only while the header is hovered:
            // an open ⋮ menu has to outlive the pointer leaving the header.
            .child(
                h_flex()
                    .absolute()
                    .right(px(4.))
                    .top_0()
                    .h_full()
                    .items_center()
                    .invisible()
                    .group_hover(header_group, |style| style.visible())
                    // The buttons' presses are theirs: a click reaches every element its
                    // press reached, and the header's collapses the section.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(search)
                    .child(more),
            );
        let header = match drop_target(section, &key) {
            Some(target) => {
                let (this, accepts_dms) = (cx.entity().downgrade(), accepts_dms(section, &key));
                header
                    .drag_over::<ChannelDrag>(move |style, drag, _, cx| {
                        if accepts_dms || !drag.is_dm {
                            style.bg(cx.theme().sidebar_foreground.opacity(0.12))
                        } else {
                            style
                        }
                    })
                    .on_drop(move |drag: &ChannelDrag, _, cx| {
                        if !accepts_dms && drag.is_dm {
                            return;
                        }
                        let _ = this.update(cx, |sidebar, cx| {
                            sidebar.move_channel(
                                drag.id.clone(),
                                drag.from.clone(),
                                target.clone(),
                                cx,
                            )
                        });
                    })
            }
            None => header,
        };
        div().relative().child(header.context_menu(context_menu))
    }

    /// The sections under the nav rows. Live: the user's own sidebar sections in their order,
    /// the built-in Channels and Direct messages buckets holding every conversation no other
    /// section claims, as in Slack. Sample workspace: plain Channels and Direct messages.
    pub(super) fn section_list(
        &self,
        channels: &[Conversation],
        watermark: Option<&Ts>,
        cx: &mut Context<Self>,
    ) -> Vec<(AnyElement, ListChild)> {
        self.rows.borrow_mut().clear();
        let sections: Vec<ChannelSection> = self.section_set();
        let home_keep = |c: &Conversation| -> bool {
            if c.id == self.selected {
                return true;
            }
            match self.home_prefs.show {
                1 => self.unread_in(c, watermark).count > 0,
                2 => self.unread_in(c, watermark).mentions > 0,
                _ => true,
            }
        };
        let mut list = Vec::new();
        if self.home_prefs.unreads_section {
            let unread_all: Vec<Conversation> = channels
                .iter()
                .chain(&self.fixtures.dms)
                .filter(|c| self.unread_in(c, watermark).count > 0)
                .cloned()
                .collect();
            if !unread_all.is_empty() {
                list.extend(self.section(
                    "home-unreads-section".into(),
                    ("Unreads", None),
                    &unread_all,
                    watermark,
                    None,
                    cx,
                ));
            }
        }
        if sections.is_empty() {
            let filtered_ch: Vec<Conversation> =
                channels.iter().filter(|c| home_keep(c)).cloned().collect();
            let filtered_dm: Vec<Conversation> = self
                .fixtures
                .dms
                .iter()
                .filter(|c| home_keep(c))
                .cloned()
                .collect();
            if self.home_prefs.separate_private {
                let public: Vec<Conversation> = filtered_ch
                    .iter()
                    .filter(|c| !c.is_private)
                    .cloned()
                    .collect();
                let private: Vec<Conversation> = filtered_ch
                    .iter()
                    .filter(|c| c.is_private)
                    .cloned()
                    .collect();
                list.extend(self.section(
                    "channels".into(),
                    ("Channels", None),
                    &public,
                    watermark,
                    None,
                    cx,
                ));
                if !private.is_empty() {
                    list.extend(self.section(
                        "private-channels".into(),
                        ("Private channels", None),
                        &private,
                        watermark,
                        None,
                        cx,
                    ));
                }
            } else {
                list.extend(self.section(
                    "channels".into(),
                    ("Channels", None),
                    &filtered_ch,
                    watermark,
                    None,
                    cx,
                ));
            }
            list.extend(self.section(
                "dms".into(),
                ("Direct messages", None),
                &filtered_dm,
                watermark,
                None,
                cx,
            ));
            return list;
        }
        // Slack fills External connections itself, from the externally shared channels.
        let connect = sections
            .iter()
            .any(|section| section.kind == SectionKind::SlackConnect);
        let builtin = |kind: &SectionKind| {
            matches!(kind, SectionKind::Channels | SectionKind::DirectMessages)
        };
        // A conversation shows in one section only: the highest ranked one listing it
        // (Starred, then the user's own sections, then user-group sections, then the rest).
        let rank = |kind: &SectionKind| match kind {
            SectionKind::Stars => 0,
            SectionKind::Standard => 1,
            SectionKind::UserGroup => 2,
            _ => 3,
        };
        let mut ranked: Vec<&ChannelSection> = sections
            .iter()
            .filter(|section| !builtin(&section.kind))
            .collect();
        ranked.sort_by_key(|section| rank(&section.kind));
        let mut claimed: HashMap<&ChannelId, &str> = HashMap::new();
        for section in ranked {
            for id in &section.channel_ids {
                claimed.entry(id).or_insert(section.id.as_str());
            }
        }
        let member = |id: &ChannelId| {
            channels
                .iter()
                .chain(&self.fixtures.dms)
                .find(|conversation| conversation.id == *id)
        };
        for section in &sections {
            let mut rows: Vec<Conversation> = match section.kind {
                SectionKind::Channels => channels
                    .iter()
                    .filter(|c| !claimed.contains_key(&c.id) && !(connect && c.is_ext_shared))
                    .cloned()
                    .collect(),
                SectionKind::SlackConnect => channels
                    .iter()
                    .filter(|c| c.is_ext_shared && !claimed.contains_key(&c.id))
                    .cloned()
                    .collect(),
                SectionKind::DirectMessages => self
                    .fixtures
                    .dms
                    .iter()
                    .filter(|c| !claimed.contains_key(&c.id))
                    .cloned()
                    .collect(),
                _ => section
                    .channel_ids
                    .iter()
                    .filter(|id| claimed.get(id) == Some(&section.id.as_str()))
                    .filter_map(member)
                    .cloned()
                    .collect(),
            };
            rows.retain(&home_keep);
            if rows.is_empty() && !builtin(&section.kind) {
                continue;
            }
            // The section's sort: its own choice, else Home preference (Recent / Priority / Alpha).
            let section_key: SharedString = section.id.clone().into();
            let default_recent =
                section.kind == SectionKind::DirectMessages || self.home_prefs.sort == 1;
            let sort =
                if self.home_prefs.sort == 2 && !self.section_sorts.contains_key(&section_key) {
                    // Priority: mentions first, then unreads, then recency.
                    rows.sort_by_key(|c| {
                        let u = self.unread_in(c, watermark);
                        let latest = self
                            .fixtures
                            .counts
                            .get(&c.id)
                            .and_then(|n| n.latest.clone());
                        (
                            std::cmp::Reverse(u.mentions),
                            std::cmp::Reverse(u.count > 0),
                            std::cmp::Reverse(latest),
                        )
                    });
                    None
                } else {
                    Some(self.section_sort(&section_key, default_recent))
                };
            if let Some(sort) = sort {
                match sort {
                    SectionSort::Recency => {
                        let latest = |c: &Conversation| {
                            self.fixtures
                                .counts
                                .get(&c.id)
                                .and_then(|n| n.latest.clone())
                        };
                        rows.sort_by_key(|c| std::cmp::Reverse(latest(c)));
                    }
                    SectionSort::Alpha => {
                        rows.sort_by_cached_key(|c| {
                            let label = self.fixtures.conversation_label(c);
                            label.trim_start_matches("# ").to_lowercase()
                        });
                    }
                    SectionSort::Priority => {
                        rows.sort_by_key(|c| {
                            let u = self.unread_in(c, watermark);
                            let latest = self
                                .fixtures
                                .counts
                                .get(&c.id)
                                .and_then(|n| n.latest.clone());
                            (
                                std::cmp::Reverse(u.mentions),
                                std::cmp::Reverse(u.count > 0),
                                std::cmp::Reverse(latest),
                            )
                        });
                    }
                }
            }
            let (title, leading) = section_title(section, &self.fixtures);
            list.extend(self.section(
                section.id.clone().into(),
                (&title, leading),
                &rows,
                watermark,
                Some(section),
                cx,
            ));
        }
        list
    }

    /// "Add channels" under the channel list: opens the channel browser.
    pub(super) fn add_channels_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let fg = cx.theme().sidebar_foreground;
        let dim = fg.opacity(0.6);
        let hover = fg.opacity(0.08);
        let tile = fg.opacity(0.14);
        let row_h = if self.home_prefs.compact { 24. } else { 28. };
        h_flex()
            .id("browse-channels")
            .h(px(row_h))
            .pl(px(24.))
            .pr_2()
            .gap_2()
            .items_center()
            .rounded_sm()
            .text_color(dim)
            .cursor_pointer()
            .hover(|row| row.bg(hover))
            .child(
                div()
                    .size(px(20.))
                    .flex_none()
                    .rounded_sm()
                    .bg(tile)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::Plus).text_color(dim).size(px(14.))),
            )
            .child("Add channels")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::BrowseChannels)))
    }

    /// One of Home's shortcut rows above the sections (Unreads, Threads, …): outline icon and
    /// label, bold while it has something new, with an optional trailing count.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn nav_row(
        &self,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        bold: bool,
        active: bool,
        trailing: Option<String>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let color = if active {
            theme.sidebar_accent_foreground
        } else if bold {
            theme.sidebar_foreground
        } else {
            theme.sidebar_foreground.opacity(0.75)
        };
        let hover = theme.sidebar_foreground.opacity(0.08);
        // `flex_none`: in the overflowing scroll column these rows would otherwise shrink.
        h_flex()
            .id(id)
            .flex_none()
            .mx_2()
            .px_2()
            .h(px(28.))
            .gap_2()
            .items_center()
            .rounded_md()
            .text_color(color)
            .cursor_pointer()
            .when(active, |row| row.bg(theme.sidebar_accent))
            .when(!active, |row| row.hover(|row| row.bg(hover)))
            .child(Icon::new(icon).text_color(color).size(px(16.)))
            .child(
                div()
                    .flex_1()
                    .font_weight(if bold {
                        FontWeight::BOLD
                    } else if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .child(label),
            )
            .children(trailing)
    }

    /// The rail's Files destination, Slack's Files list column: the "Files" header over one
    /// row per page of the Files view (All files, Canvases, Lists), the shown one selected.
    /// Slack's Deleted, Recently viewed and Starred rows are left out: their data sits behind
    /// private methods slackless does not call (`files.recentlyDeleted`,
    /// `files.favorites.list`) or no mapped method at all.
    pub(super) fn files_list(&self, cx: &mut Context<Self>) -> Div {
        v_flex()
            .child(
                div()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child("Files"),
            )
            .children(FilesSection::ALL.map(|section| self.files_row(section, cx)))
    }

    /// One Files list-column row; a click shows its page.
    pub(super) fn files_row(&self, section: FilesSection, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let selected = section == self.files_section;
        let color = if selected {
            theme.sidebar_accent_foreground
        } else {
            theme.sidebar_foreground.opacity(0.75)
        };
        let (fill, hover) = (theme.sidebar_accent, theme.sidebar_foreground.opacity(0.08));
        h_flex()
            .id(SharedString::from(format!("files-{section:?}")))
            .flex_none()
            .mx_2()
            .px_2()
            .h(px(28.))
            .gap_2()
            .items_center()
            .rounded_md()
            .text_color(color)
            .cursor_pointer()
            .when(selected, |row| row.bg(fill))
            .when(!selected, |row| row.hover(|row| row.bg(hover)))
            .child(Icon::new(section.icon()).text_color(color).size(px(16.)))
            .child(section.title())
            .on_click(cx.listener(move |this, _, _, cx| this.show_files_section(section, cx)))
    }

    /// Select `section`'s Files list-column row and ask the root to show its page (a row
    /// click, or the demo hook).
    pub(crate) fn show_files_section(&mut self, section: FilesSection, cx: &mut Context<Self>) {
        if self.files_section != section {
            self.files_section = section;
            cx.emit(SidebarEvent::ShowFilesSection(section));
            cx.notify();
        }
    }

    pub(super) fn conversation_row(
        &self,
        conversation: &Conversation,
        watermark: Option<&Ts>,
        section: Option<&ChannelSection>,
        key: &SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let id = conversation.id.clone();
        let selected = self.active_home_page.is_none() && id == self.selected;
        let unread = self.unread_in(conversation, watermark);
        let label = self.fixtures.conversation_label(conversation);
        // Focus mode marks new activity with a subtle dot instead of bolding and counts.
        let focus = self.self_state.focused();
        let emphasized = selected || (unread.count > 0 && !focus);
        let color = if selected {
            theme.sidebar_accent_foreground
        } else if emphasized {
            theme.sidebar_foreground
        } else {
            theme.sidebar_foreground.opacity(0.6)
        };
        let weight = if emphasized {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        };

        let kind = conversation.kind();
        // A Slack Connect channel names its other organizations in a tooltip over its name.
        let shared = conversation.is_ext_shared
            && matches!(
                kind,
                ConversationKind::PublicChannel | ConversationKind::PrivateChannel
            );
        let tip = (shared && self.org_tip.as_ref() == Some(&id))
            .then(|| org_tooltip(&self.fixtures.shared_teams(conversation), cx))
            .flatten();
        // Public rows carry `# name` from the label itself, like the reference list.
        let (leading, label) = match kind {
            ConversationKind::PrivateChannel => (
                Icon::new(IconName::Lock)
                    .text_color(color)
                    .size(px(16.))
                    .into_any_element(),
                label.strip_prefix("# ").map(str::to_owned).unwrap_or(label),
            ),
            ConversationKind::Im | ConversationKind::Mpim => (
                if self.home_prefs.profile_photos {
                    conversation_avatar(
                        &self.fixtures,
                        conversation,
                        if self.home_prefs.compact { 18. } else { 20. },
                        if selected {
                            theme.sidebar_accent
                        } else {
                            theme.sidebar
                        },
                        cx,
                    )
                    .into_any_element()
                } else {
                    Icon::new(IconName::User)
                        .text_color(color)
                        .size(px(15.))
                        .into_any_element()
                },
                label.clone(),
            ),
            ConversationKind::PublicChannel => (
                div()
                    .w(px(16.))
                    .flex()
                    .justify_center()
                    .text_base()
                    .child("#")
                    .into_any_element(),
                label.strip_prefix("# ").map(str::to_owned).unwrap_or(label),
            ),
        };

        let row_h = if self.home_prefs.compact { 24. } else { 28. };
        let row_hover = theme.sidebar_foreground.opacity(0.08);
        let mut row = h_flex()
            .id(SharedString::from(format!("conversation-{id}")))
            .h(px(row_h))
            .ml(px(2.))
            .mr(px(2.))
            .pl(px(22.))
            .pr_2()
            .gap_2()
            .items_center()
            .rounded_md()
            .text_color(color)
            .cursor_pointer()
            .when(selected, |row| row.bg(theme.sidebar_accent))
            .when(!selected, |row| row.hover(|row| row.bg(row_hover)))
            .child(leading);
        let name = div().truncate().font_weight(weight).child(label);
        row = if shared {
            // The tooltip centers on the name itself, so the name shrinks to its text.
            let hovered_id = id.clone();
            row.child(
                h_flex().flex_1().min_w_0().child(
                    div()
                        .relative()
                        .min_w_0()
                        .child(name)
                        .children(tip.map(|tip| deferred(tip).with_priority(2))),
                ),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hover_shared_row(&hovered_id, *hovered, cx);
            }))
        } else {
            row.child(name.flex_1())
        };
        if self.huddles.contains(&conversation.id) {
            let live = if selected { color } else { theme.success };
            row = row.child(
                Icon::new(IconName::Headphones)
                    .text_color(live)
                    .size(px(14.)),
            );
        }
        if focus && unread.count > 0 {
            row = row.child(focus_dot(cx));
        } else if unread.mentions > 0 {
            row = row.child(count_pill(unread.mentions, true, cx));
        }
        // Drag the row onto another section's header or rows to move it there, with
        // Slack's drag-layer preview; a row also takes drops for its own section.
        let drag = ChannelDrag {
            id: id.clone(),
            label: self.fixtures.conversation_label(conversation),
            private: matches!(kind, ConversationKind::PrivateChannel),
            is_dm: matches!(kind, ConversationKind::Im | ConversationKind::Mpim),
            from: section
                .filter(|section| section.kind == SectionKind::Standard)
                .map(|section| section.id.clone()),
            position: Point::default(),
        };
        let row = row.on_drag(drag, |drag: &ChannelDrag, position, _, cx| {
            cx.new(|_| ChannelDrag {
                position,
                ..drag.clone()
            })
        });
        let row = match drop_target(section, key) {
            Some(target) => {
                let (this, accepts_dms) = (cx.entity().downgrade(), accepts_dms(section, key));
                row.drag_over::<ChannelDrag>(move |style, drag, _, cx| {
                    if accepts_dms || !drag.is_dm {
                        style.bg(cx.theme().sidebar_foreground.opacity(0.12))
                    } else {
                        style
                    }
                })
                .on_drop(move |drag: &ChannelDrag, _, cx| {
                    if !accepts_dms && drag.is_dm {
                        return;
                    }
                    let _ = this.update(cx, |sidebar, cx| {
                        sidebar.move_channel(drag.id.clone(), drag.from.clone(), target.clone(), cx)
                    });
                })
            }
            None => row,
        };
        // Right-click: the row's own menu (details, copy, move, notify, leave).
        let menu_conversation = conversation.clone();
        let menu_section = section.cloned();
        let this = cx.entity().downgrade();
        row.on_click(cx.listener(move |this, _, _, cx| this.select_row(id.clone(), cx)))
            .context_menu(move |menu, window, cx| {
                let Some(input) = this.upgrade().map(|sidebar| {
                    sidebar
                        .read(cx)
                        .channel_menu_input(&menu_conversation, menu_section.as_ref())
                }) else {
                    return menu;
                };
                channel_menu(menu, input, &this, window, cx)
            })
    }

    /// One row menu's whole input, gathered as the menu opens: the label, the workspace's
    /// archive link, the custom sections to move into and the current notify level.
    pub(super) fn channel_menu_input(
        &self,
        conversation: &Conversation,
        section: Option<&ChannelSection>,
    ) -> ChannelMenuInput {
        let link = match self.fixtures.workspaces.first() {
            Some(team) if !team.domain.is_empty() => {
                format!(
                    "https://{}.slack.com/archives/{}",
                    team.domain, conversation.id
                )
            }
            Some(team) => format!(
                "https://app.slack.com/client/{}/{}",
                team.id, conversation.id
            ),
            None => format!("https://app.slack.com/client/{}", conversation.id),
        };
        let sections: Vec<(String, String, String)> = self
            .section_set()
            .iter()
            .filter(|section| section.kind == SectionKind::Standard)
            .map(|section| {
                (
                    section.id.clone(),
                    section.name.clone(),
                    section.emoji.clone(),
                )
            })
            .collect();
        ChannelMenuInput {
            channel: conversation.id.clone(),
            label: self.fixtures.conversation_label(conversation),
            link,
            is_channel: matches!(
                conversation.kind(),
                ConversationKind::PublicChannel | ConversationKind::PrivateChannel
            ),
            from: section
                .filter(|section| section.kind == SectionKind::Standard)
                .map(|section| (section.id.clone(), section.name.clone())),
            sections,
            level: self
                .notify_level(&conversation.id)
                .unwrap_or(NotifyLevel::Everything),
        }
    }

    /// Slack's floating "More unreads" chip for one edge of the list: it points at hidden
    /// unread rows, turns into the red "Unread mentions" chip while any of them carries
    /// mentions, and otherwise reads as a neutral raised chip.
    pub(super) fn unreads_pill(&self, target: HiddenUnreads, cx: &Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (bg, hover, fg) = if target.mentions {
            let (bg, fg) = crate::theme::mention_badge(cx);
            (bg, bg.blend(fg.opacity(0.1)), fg)
        } else {
            // Opaque like Slack's chip: the rows it floats over must not show through.
            (
                theme.secondary,
                theme.secondary_hover,
                theme.secondary_foreground,
            )
        };
        // Slack's chip: 28px high, the arrow close to its left edge, more room after the label.
        h_flex()
            .id(if target.below {
                "unreads-below"
            } else {
                "unreads-above"
            })
            .h(px(28.))
            .gap(px(6.))
            .pl(px(10.))
            .pr(px(16.))
            .rounded_full()
            .border_1()
            .border_color(theme.sidebar_border)
            .bg(bg)
            .hover(|pill| pill.bg(hover))
            .shadow_sm()
            .text_size(px(13.))
            .text_color(fg)
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .child(
                Icon::new(if target.below {
                    IconName::ArrowDown
                } else {
                    IconName::ArrowUp
                })
                .text_color(fg)
                .size(px(14.)),
            )
            .child(if target.mentions {
                "Unread mentions"
            } else {
                "More unreads"
            })
            // The pill floats over sidebar rows: a click must not also open the row beneath.
            .occlude()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.reveal_unread(target);
                cx.notify();
            }))
    }

    /// Ease the list toward the unread rows past `target`'s edge, as Slack does: as many of
    /// them as fit come into view, the nearest stays in view, and each edge keeps a pill's
    /// height clear for the pill still floating there for later ones.
    pub(super) fn reveal_unread(&mut self, target: HiddenUnreads) {
        let (Some(near), Some(far)) = (
            self.scroll.bounds_for_item(target.row),
            self.scroll.bounds_for_item(target.far),
        ) else {
            self.scroll.scroll_to_item(target.row);
            return;
        };
        let viewport = self.scroll.bounds();
        let clearance = px(PILL_CLEARANCE);
        // Child bounds are unscrolled: on screen a row sits at its bounds plus the offset,
        // which turns negative as the list scrolls down.
        let y = if target.below {
            (viewport.bottom() - clearance - far.bottom())
                .max(viewport.top() + clearance - near.top())
        } else {
            (viewport.top() + clearance - far.top())
                .min(viewport.bottom() - clearance - near.bottom())
        };
        let from = self.scroll.offset().y;
        self.scroll_anim = Some(ScrollAnim {
            from,
            to: y.clamp(-self.scroll.max_offset().y, px(0.)),
            start: Instant::now(),
            last: from,
        });
    }

    /// Track the pointer over a Slack Connect row: its organizations tooltip shows after the
    /// kit's tooltip delay, as Slack's does, and hides as soon as the pointer leaves.
    pub(super) fn hover_shared_row(
        &mut self,
        id: &ChannelId,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        if hovered {
            let delay = gpui_kit::base::TooltipDefaults::global(cx).show_delay();
            let show = id.clone();
            let task = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                let _ = this.update(cx, |this, cx| {
                    this.org_tip = Some(show);
                    this.org_tip_delay = None;
                    cx.notify();
                });
            });
            self.org_tip_delay = Some((id.clone(), task));
            return;
        }
        if self
            .org_tip_delay
            .as_ref()
            .is_some_and(|(pending, _)| pending == id)
        {
            self.org_tip_delay = None;
        }
        if self.org_tip.as_ref() == Some(id) {
            self.org_tip = None;
            cx.notify();
        }
    }

    /// Advance the "More unreads" scroll one frame (ease-out cubic), dropping it once it
    /// lands or as soon as the user scrolls the list themselves.
    pub(super) fn step_scroll(&mut self, window: &mut Window) {
        let Some(anim) = self.scroll_anim.as_mut() else {
            return;
        };
        let offset = self.scroll.offset();
        if offset.y != anim.last {
            self.scroll_anim = None;
            return;
        }
        let t = (anim.start.elapsed().as_secs_f32() / SCROLL_ANIM.as_secs_f32()).min(1.);
        let y = anim.from + (anim.to - anim.from) * (1. - (1. - t).powi(3));
        self.scroll.set_offset(point(offset.x, y));
        anim.last = y;
        if t < 1. {
            window.request_animation_frame();
        } else {
            self.scroll_anim = None;
        }
    }
}
