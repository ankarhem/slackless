//! Home sidebar sections, Files nav column, conversation rows, and More Unreads pill.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
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
use crate::fixtures::Fixtures;
use crate::sidebar_menu::{
    ChannelDrag, ChannelMenuInput, HiddenConversations, SectionSort, channel_menu, section_menu,
};

thread_local! {
    static ALPHA_KEYS: RefCell<AlphaKeys> = RefCell::default();
}

/// The Alpha sort's keys by conversation, computed once for the workspace data they came
/// from: a frame sorts by them without building and lowercasing every label again.
#[derive(Default)]
struct AlphaKeys {
    fixtures: Option<Arc<Fixtures>>,
    keys: HashMap<ChannelId, Rc<str>>,
}

/// The Alpha sort's key for `conversation`: its label lowercased, without a channel's `# `.
fn alpha_key(fixtures: &Arc<Fixtures>, conversation: &Conversation) -> Rc<str> {
    ALPHA_KEYS.with(|memo| {
        let mut memo = memo.borrow_mut();
        if !memo
            .fixtures
            .as_ref()
            .is_some_and(|known| known.shares_data(fixtures))
        {
            memo.fixtures = Some(fixtures.clone());
            memo.keys.clear();
        }
        if let Some(key) = memo.keys.get(&conversation.id) {
            return key.clone();
        }
        let label = fixtures.conversation_label(conversation);
        let key: Rc<str> = label.trim_start_matches("# ").to_lowercase().into();
        memo.keys.insert(conversation.id.clone(), key.clone());
        key
    })
}

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
        conversations: &[&Conversation],
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
        for &conversation in conversations {
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
        conversations: &[&Conversation],
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
            let section = section.map(|section| section.id.clone());
            let title = title.to_owned();
            move |this: &gpui_kit::gpui::WeakEntity<Self>, cx: &gpui_kit::gpui::App| {
                this.upgrade().map(|sidebar| {
                    sidebar.read(cx).section_menu_input(
                        key.clone(),
                        section.as_deref(),
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

    /// The joined channels the Home list shows: the snapshot's, then the ones joined from the
    /// channel browser this session that the snapshot does not list yet (once a snapshot lists
    /// a session join, its copy is the one shown).
    pub(super) fn home_channels(&self) -> impl Iterator<Item = &Conversation> {
        let listed = &self.fixtures.channels;
        listed.iter().chain(
            self.joined
                .iter()
                .filter(|joined| !listed.iter().any(|channel| channel.id == joined.id)),
        )
    }

    /// The conversation `id` names among the ones the Home list shows (the channels of
    /// [`Self::home_channels`] and the DMs). [`Fixtures::conversation`] also finds directory
    /// and archived entries, so only a hit inside the snapshot's own channel or DM list counts;
    /// a channel joined this session is looked up among the few joined ones.
    pub(super) fn listed_conversation(&self, id: &ChannelId) -> Option<&Conversation> {
        let holds = |list: &[Conversation], conversation: &Conversation| {
            list.as_ptr_range()
                .contains(&std::ptr::from_ref(conversation))
        };
        self.fixtures
            .conversation(id)
            .filter(|conversation| {
                holds(&self.fixtures.channels, conversation)
                    || holds(&self.fixtures.dms, conversation)
            })
            .or_else(|| {
                self.joined
                    .iter()
                    .find(|conversation| conversation.id == *id)
            })
    }

    /// The sections under the nav rows. Live: the user's own sidebar sections in their order,
    /// the built-in Channels and Direct messages buckets holding every conversation no other
    /// section claims, as in Slack. Sample workspace: plain Channels and Direct messages.
    pub(super) fn section_list(
        &self,
        watermark: Option<&Ts>,
        cx: &mut Context<Self>,
    ) -> Vec<(AnyElement, ListChild)> {
        self.rows.borrow_mut().clear();
        let sections = self.section_set();
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
        // Dedicated VIPs section at top of Home when VIP colleagues are configured (`F-SHELL-058`).
        if !self.vip_users.trim().is_empty() {
            let vip_settings = crate::preferences::Settings {
                vip_users: self.vip_users.clone(),
                ..Default::default()
            };
            let vip_dms: Vec<&Conversation> = self
                .fixtures
                .dms
                .iter()
                .filter(|conv| {
                    conv.user.as_ref().is_some_and(|uid| {
                        let u = self.fixtures.user(uid);
                        let handle = u.map(|x| x.name.as_str()).unwrap_or("");
                        let display = u.map(|x| x.display_name()).unwrap_or("");
                        vip_settings.is_vip(uid.as_str(), handle, display)
                    })
                })
                .collect();
            if !vip_dms.is_empty() {
                list.extend(
                    self.section(
                        "home-vip-section".into(),
                        (
                            "VIPs",
                            Some(
                                Icon::new(IconName::Star)
                                    .size(px(14.))
                                    .text_color(cx.theme().warning)
                                    .into_any_element(),
                            ),
                        ),
                        &vip_dms,
                        watermark,
                        None,
                        cx,
                    ),
                );
            }
        }
        if self.home_prefs.unreads_section {
            let unread_all: Vec<&Conversation> = self
                .home_channels()
                .chain(&self.fixtures.dms)
                .filter(|c| self.unread_in(c, watermark).count > 0)
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
        let has_server_stars = sections.iter().any(|s| s.kind == SectionKind::Stars);
        if !has_server_stars && !self.local_starred.is_empty() {
            let starred_rows: Vec<&Conversation> = self
                .home_channels()
                .chain(&self.fixtures.dms)
                .filter(|c| self.local_starred.contains(&c.id) && home_keep(c))
                .collect();
            if !starred_rows.is_empty() {
                list.extend(
                    self.section(
                        "local-starred".into(),
                        (
                            "Starred",
                            Some(
                                Icon::new(IconName::Star)
                                    .size(px(14.))
                                    .text_color(cx.theme().sidebar_foreground.opacity(0.75))
                                    .into_any_element(),
                            ),
                        ),
                        &starred_rows,
                        watermark,
                        None,
                        cx,
                    ),
                );
            }
        }
        if sections.is_empty() {
            let filtered_ch: Vec<&Conversation> = self
                .home_channels()
                .filter(|c| !self.local_starred.contains(&c.id) && home_keep(c))
                .collect();
            let filtered_dm: Vec<&Conversation> = self
                .fixtures
                .dms
                .iter()
                .filter(|c| !self.local_starred.contains(&c.id) && home_keep(c))
                .collect();
            if self.home_prefs.separate_private {
                let (private, public): (Vec<&Conversation>, Vec<&Conversation>) =
                    filtered_ch.into_iter().partition(|c| c.is_private);
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
        if !has_server_stars {
            for id in &self.local_starred {
                claimed.entry(id).or_insert("local-starred");
            }
        }
        for section in ranked {
            for id in &section.channel_ids {
                claimed.entry(id).or_insert(section.id.as_str());
            }
        }
        for section in sections {
            let mut rows: Vec<&Conversation> = match section.kind {
                SectionKind::Channels => self
                    .home_channels()
                    .filter(|c| !claimed.contains_key(&c.id) && !(connect && c.is_ext_shared))
                    .collect(),
                SectionKind::SlackConnect => self
                    .home_channels()
                    .filter(|c| c.is_ext_shared && !claimed.contains_key(&c.id))
                    .collect(),
                SectionKind::DirectMessages => self
                    .fixtures
                    .dms
                    .iter()
                    .filter(|c| !claimed.contains_key(&c.id))
                    .collect(),
                _ => section
                    .channel_ids
                    .iter()
                    .filter(|id| claimed.get(id) == Some(&section.id.as_str()))
                    .filter_map(|id| self.listed_conversation(id))
                    .collect(),
            };
            rows.retain(|c| home_keep(c));
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
                        rows.sort_by_cached_key(|c| alpha_key(&self.fixtures, c));
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
}

pub(super) struct NavRowArgs {
    pub id: &'static str,
    pub icon: IconName,
    pub label: &'static str,
    pub bold: bool,
    pub active: bool,
    pub trailing: Option<String>,
}

impl Sidebar {
    /// One of Home's shortcut rows above the sections (Unreads, Threads, …): outline icon and
    /// label, bold while it has something new, with an optional trailing count.
    pub(super) fn nav_row(&self, args: NavRowArgs, cx: &Context<Self>) -> Stateful<Div> {
        let NavRowArgs {
            id,
            icon,
            label,
            bold,
            active,
            trailing,
        } = args;
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
        // A Slack Connect channel names its other organizations in a tooltip over its name
        // and carries a compact external-org indicator (`F-COLLAB-798`, `800`, `828`).
        let is_channel_kind = matches!(
            kind,
            ConversationKind::PublicChannel | ConversationKind::PrivateChannel
        );
        let shared = conversation.is_external_connect() && is_channel_kind;
        let org_shared_internal =
            conversation.is_org_shared && !conversation.is_ext_shared && is_channel_kind;
        let external_dm = matches!(kind, ConversationKind::Im)
            && conversation
                .user
                .as_ref()
                .is_some_and(|u| self.fixtures.is_external_user(u));
        let shared_teams = if shared {
            self.fixtures.shared_teams(conversation)
        } else {
            Vec::new()
        };
        let owner_team = if shared {
            self.fixtures.channel_owner_team(conversation)
        } else {
            None
        };
        let tip = (shared && self.org_tip.as_ref() == Some(&id))
            .then(|| org_tooltip(&shared_teams, owner_team, cx))
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
            .id(SharedString::from(format!("conversation-{key}-{id}")))
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
        let name = div().min_w_0().truncate().font_weight(weight).child(label);
        let external_badge_icon = Icon::new(IconName::Building2)
            .flex_none()
            .size(px(12.))
            .text_color(color.opacity(0.8));
        row = if shared {
            // The tooltip centers on the name itself, so the name shrinks to its text.
            let hovered_id = id.clone();
            let connect_indicator = if let Some(first_org) = shared_teams.first() {
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap_0p5()
                    .child(crate::channel_details::org_chip(first_org, 13.))
                    .when(shared_teams.len() > 1, |el| {
                        el.child(
                            div()
                                .text_size(px(10.))
                                .text_color(color.opacity(0.8))
                                .child(format!("+{}", shared_teams.len() - 1)),
                        )
                    })
                    .into_any_element()
            } else {
                external_badge_icon.into_any_element()
            };
            row.child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .relative()
                            .min_w_0()
                            .child(name)
                            .children(tip.map(|tip| deferred(tip).with_priority(2))),
                    )
                    .child(connect_indicator),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hover_shared_row(&hovered_id, *hovered, cx);
            }))
        } else if org_shared_internal {
            row.child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_1p5()
                    .child(name)
                    .child(
                        div()
                            .flex_none()
                            .px_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(color.opacity(0.35))
                            .text_size(px(9.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color.opacity(0.85))
                            .child("Org"),
                    ),
            )
        } else if external_dm {
            row.child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_1()
                    .child(name)
                    .child(external_badge_icon),
            )
        } else {
            row.child(name.flex_1())
        };
        if let Some(participants) = self.huddles.get(&conversation.id) {
            let count = participants.len();
            let ring_bg = if selected {
                theme.sidebar_accent
            } else {
                theme.sidebar
            };
            let (pill_bg, pill_fg) = if selected {
                (theme.sidebar_accent_foreground, theme.sidebar_accent)
            } else {
                (theme.sidebar_accent, theme.sidebar_accent_foreground)
            };
            let tip_text: SharedString = if participants.is_empty() {
                "Huddle in progress".into()
            } else {
                let names: Vec<_> = participants
                    .iter()
                    .take(4)
                    .map(|u| self.fixtures.user_name(u))
                    .collect();
                let word = if count == 1 { "person" } else { "people" };
                format!("{count} {word} in the huddle — {}", names.join(", ")).into()
            };
            let pill_id = SharedString::from(format!("sidebar-huddle-pill-{id}"));
            row = row.child(
                h_flex()
                    .id(pill_id)
                    .flex_none()
                    .items_center()
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip_text.clone())
                            .build(window, cx)
                    })
                    .children(participants.iter().take(2).enumerate().map(|(ix, uid)| {
                        div()
                            .when(ix > 0, |el| el.ml(px(-3.)))
                            .rounded(px(4.))
                            .overflow_hidden()
                            .border_1()
                            .border_color(ring_bg)
                            .child(crate::avatars::user_chip(&self.fixtures, uid, 18.))
                    }))
                    .child(
                        h_flex()
                            .when(!participants.is_empty(), |el| el.ml(px(-2.)))
                            .h(px(18.))
                            .px_1p5()
                            .gap_1()
                            .items_center()
                            .rounded(px(4.))
                            .bg(pill_bg)
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(pill_fg)
                            .child(
                                Icon::new(IconName::Headphones)
                                    .text_color(pill_fg)
                                    .size(px(11.)),
                            )
                            .when(count > 0, |el| el.child(count.to_string())),
                    ),
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
        // Right-click: the row's own menu (details, copy, move, notify, leave), gathered from
        // the sidebar as it opens.
        let (menu_id, menu_section) = (id.clone(), section.map(|section| section.id.clone()));
        let this = cx.entity().downgrade();
        row.on_click(cx.listener(move |this, _, _, cx| this.select_row(id.clone(), cx)))
            .context_menu(move |menu, window, cx| {
                let Some(input) = this.upgrade().and_then(|sidebar| {
                    sidebar
                        .read(cx)
                        .channel_menu_input(&menu_id, menu_section.as_deref())
                }) else {
                    return menu;
                };
                channel_menu(menu, input, &this, window, cx)
            })
    }

    /// One row menu's whole input, gathered as the menu opens: the label, the workspace's
    /// archive link, the custom sections to move into and the current notify level. `None`
    /// once the row's conversation left the list.
    pub(super) fn channel_menu_input(
        &self,
        channel: &ChannelId,
        section: Option<&str>,
    ) -> Option<ChannelMenuInput> {
        let conversation = self.listed_conversation(channel)?;
        let section = section.and_then(|id| self.section_set().iter().find(|known| known.id == id));
        let link = match self.fixtures.own_team() {
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
        Some(ChannelMenuInput {
            channel: conversation.id.clone(),
            label: self.fixtures.conversation_label(conversation),
            link,
            is_channel: matches!(
                conversation.kind(),
                ConversationKind::PublicChannel | ConversationKind::PrivateChannel
            ),
            starred: self.is_starred(&conversation.id),
            from: section
                .filter(|section| section.kind == SectionKind::Standard)
                .map(|section| (section.id.clone(), section.name.clone())),
            sections,
            level: self
                .notify_level(&conversation.id)
                .unwrap_or(NotifyLevel::Everything),
        })
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
            // The pill floats over sidebar rows: block clicks/hover from reaching the row
            // beneath while letting scroll events pass through to the sidebar list.
            .block_mouse_except_scroll()
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
