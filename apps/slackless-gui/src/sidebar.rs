//! Left sidebar: workspace switcher rail with the nav stack (Home / DMs / Activity / Files /
//! To-dos / Admin), and collapsible Channels / Direct messages sections with unread and
//! mention count pills (gpui-kit-guide section 3). Counts are derived from the fixture
//! history so fixture mode renders exactly what live mode will feed it.
//!
//! The rail's DMs and Activity destinations swap the list for the DM inbox (every DM with its
//! presence/group avatar, last message and time) and the [`ActivityFeed`] (mentions,
//! reactions and thread replies with read state); Home shows the Unreads, Threads, Huddles,
//! Drafts & sent, Saved and Directories rows above the sections and an "Add channels" row that asks the root to open the
//! channel browser. [`Sidebar::badge_count`] is the dock badge: unread activity plus unread
//! DMs.
//!
//! Everything visual comes from the theme's sidebar palette (`sidebar*` tokens); selected
//! rows use `sidebar_accent`, hover rows the theme's neutral hover wash (`accent`). The only
//! derived colors are opacity steps of `sidebar_foreground` for idle rows and icons.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, img, px,
};
use slackless_core::types::{
    ChannelId, ChannelSection, Conversation, ConversationKind, Message, SectionKind, Team, Ts,
    UserId,
    rich_text::{Block, RichTextContainer, RichTextElement},
};

use crate::activity::{ActivityEvent, ActivityFeed};
use crate::avatars::conversation_avatar;
use crate::emoji;
use crate::fixtures::Fixtures;
use crate::messages::list_stamp;
use crate::status::{SelfState, self_avatar};

pub enum SidebarEvent {
    SelectConversation(ChannelId),
    /// The "Add channels" row: open the channel browser.
    BrowseChannels,
    /// A workspace tile in the rail was clicked (index into `Fixtures::workspaces`).
    SwitchWorkspace(usize),
    /// The "Drafts & sent" row.
    OpenDrafts,
    /// The "Saved" row.
    OpenLater,
    /// The rail's moon button: switch between the dark and light palette.
    ToggleTheme,
    /// The rail's own avatar: open the user menu.
    OpenUserMenu,
    /// An Activity thread item: open the conversation with the thread panel on `thread_ts`.
    OpenThread {
        channel: ChannelId,
        thread_ts: Ts,
    },
}

/// The nav destinations stacked in the workspace rail.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavItem {
    Home,
    Dms,
    Activity,
    Files,
    Todos,
    Agents,
    Admin,
}

impl NavItem {
    fn label(self) -> &'static str {
        match self {
            NavItem::Home => "Home",
            NavItem::Dms => "DMs",
            NavItem::Activity => "Activity",
            NavItem::Files => "Files",
            NavItem::Todos => "To-dos",
            NavItem::Agents => "Agents & tools",
            NavItem::Admin => "Admin",
        }
    }

    fn icon(self) -> IconName {
        match self {
            NavItem::Home => IconName::House,
            NavItem::Dms => IconName::MessageCircle,
            NavItem::Activity => IconName::Bell,
            NavItem::Files => IconName::Files,
            NavItem::Todos => IconName::SquareCheck,
            NavItem::Agents => IconName::LayoutGrid,
            NavItem::Admin => IconName::Settings,
        }
    }

    /// Every rail destination, top to bottom.
    const ALL: [NavItem; 7] = [
        NavItem::Home,
        NavItem::Dms,
        NavItem::Activity,
        NavItem::Files,
        NavItem::Todos,
        NavItem::Agents,
        NavItem::Admin,
    ];
}

/// Unread state of one conversation row, derived from the fixture history: messages the
/// signed-in user did not author that are newer than the newest message in the open
/// conversation (the fixture-mode "read" watermark). `mentions` is the subset of `count`
/// that @-mentions the signed-in user.
#[derive(Default)]
struct Unread {
    count: u32,
    mentions: u32,
}

pub struct Sidebar {
    fixtures: Arc<Fixtures>,
    selected: ChannelId,
    active_workspace: usize,
    active_nav: NavItem,
    /// Keys of the collapsed sidebar sections.
    collapsed: HashSet<SharedString>,
    /// The "Find a conversation…" field above the sections.
    find: Entity<TextareaState>,
    /// Channels joined from the channel browser this session, listed after the fixtures'.
    joined: Vec<Conversation>,
    /// The conversation with a live huddle; its row carries the headphones indicator.
    huddle: Option<ChannelId>,
    /// Saved drafts, shown on the "Drafts & sent" row.
    draft_count: usize,
    /// In-progress Later items, shown on the "Later" row.
    later_count: usize,
    /// The rail's Activity destination.
    activity: Entity<ActivityFeed>,
    /// The signed-in user's status and presence, for the rail avatar.
    self_state: SelfState,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

impl Sidebar {
    pub fn new(
        fixtures: Arc<Fixtures>,
        selected: ChannelId,
        window: &mut Window,
        cx: &mut gpui_kit::gpui::App,
    ) -> Entity<Sidebar> {
        cx.new(|cx| {
            let find = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Find a conversation…")
                    .auto_grow(1, 1)
            });
            let activity = ActivityFeed::new(fixtures.clone(), cx);
            let collapsed = fixtures
                .collapsed_sections
                .iter()
                .cloned()
                .map(SharedString::from)
                .collect();
            let subscriptions = vec![
                cx.observe(&activity, |_, _, cx| cx.notify()),
                cx.subscribe(
                    &activity,
                    |this: &mut Sidebar, _, event: &ActivityEvent, cx| {
                        let ActivityEvent::Open { channel, thread_ts } = event;
                        this.select_row(channel.clone(), cx);
                        if let Some(thread_ts) = thread_ts {
                            cx.emit(SidebarEvent::OpenThread {
                                channel: channel.clone(),
                                thread_ts: thread_ts.clone(),
                            });
                        }
                    },
                ),
            ];
            Sidebar {
                fixtures,
                selected,
                active_workspace: 0,
                active_nav: NavItem::Home,
                collapsed,
                find,
                joined: Vec::new(),
                huddle: None,
                draft_count: 0,
                later_count: 0,
                activity,
                self_state: SelfState::default(),
                _subscriptions: subscriptions,
            }
        })
    }

    /// Mark `channel` selected without emitting (the root opened it from elsewhere).
    pub fn set_selected(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        self.selected = channel;
        cx.notify();
    }

    /// Highlight workspace `index` in the rail (the root built this sidebar for it).
    pub fn set_workspace(&mut self, index: usize, cx: &mut Context<Self>) {
        self.active_workspace = index;
        cx.notify();
    }

    /// Show a rail destination (keyboard shortcuts).
    pub(crate) fn show_nav(&mut self, item: NavItem, cx: &mut Context<Self>) {
        self.active_nav = item;
        cx.notify();
    }

    pub fn set_draft_count(&mut self, count: usize, cx: &mut Context<Self>) {
        self.draft_count = count;
        cx.notify();
    }

    pub fn set_later_count(&mut self, count: usize, cx: &mut Context<Self>) {
        self.later_count = count;
        cx.notify();
    }

    /// The signed-in user's status and presence shown on the rail avatar.
    pub fn set_self_state(&mut self, state: SelfState, cx: &mut Context<Self>) {
        self.self_state = state;
        cx.notify();
    }

    /// The dock badge count: unread Activity items plus unread DM messages.
    pub fn badge_count(&self, cx: &gpui_kit::gpui::App) -> u32 {
        let watermark = self.read_watermark();
        self.activity.read(cx).unread_count()
            + self
                .total_unread(&self.fixtures.dms, watermark.as_ref())
                .count
    }

    /// Mark `channel` as having a live huddle (`None`: no huddle anywhere).
    pub fn set_huddle(&mut self, channel: Option<ChannelId>, cx: &mut Context<Self>) {
        self.huddle = channel;
        cx.notify();
    }

    /// List a channel joined from the browser under Channels.
    pub fn add_channel(&mut self, conversation: Conversation, cx: &mut Context<Self>) {
        if !self.joined.iter().any(|c| c.id == conversation.id) {
            self.joined.push(conversation);
        }
        cx.notify();
    }

    fn select_row(&mut self, id: ChannelId, cx: &mut Context<Self>) {
        self.selected = id.clone();
        cx.emit(SidebarEvent::SelectConversation(id));
        cx.notify();
    }

    /// Newest message in the open conversation: the fixture-mode "read" watermark. Live mode
    /// will replace this with the per-conversation `last_read` from `slackless-store`.
    fn read_watermark(&self) -> Option<Ts> {
        self.fixtures
            .messages
            .get(&self.selected)
            .and_then(|history| history.iter().map(|message| message.ts.clone()).max())
    }

    /// Messages in `conversation` the signed-in user has not read (see [`Unread`]).
    fn unread_messages(
        &self,
        conversation: &Conversation,
        watermark: Option<&Ts>,
    ) -> Vec<&Message> {
        if conversation.id == self.selected {
            return Vec::new();
        }
        let history = self.fixtures.messages.get(&conversation.id);
        history
            .into_iter()
            .flatten()
            .filter(|message| {
                let from_self = message
                    .user
                    .as_ref()
                    .is_some_and(|user| *user == self.fixtures.self_user);
                !from_self && watermark.is_none_or(|mark| message.ts > *mark)
            })
            .collect()
    }

    fn unread_in(&self, conversation: &Conversation, watermark: Option<&Ts>) -> Unread {
        // Live: Slack's own unread state. A DM's unread messages all count as mentions.
        if let Some(counts) = self.fixtures.counts.get(&conversation.id) {
            if conversation.id == self.selected {
                return Unread::default();
            }
            return Unread {
                count: u32::from(counts.has_unreads || counts.mention_count > 0),
                mentions: counts.mention_count,
            };
        }
        let messages = self.unread_messages(conversation, watermark);
        Unread {
            count: messages.len() as u32,
            mentions: messages
                .iter()
                .filter(|message| mentions_user(message, &self.fixtures.self_user))
                .count() as u32,
        }
    }

    fn total_unread(&self, conversations: &[Conversation], watermark: Option<&Ts>) -> Unread {
        let mut total = Unread::default();
        for conversation in conversations {
            let unread = self.unread_in(conversation, watermark);
            total.count += unread.count;
            total.mentions += unread.mentions;
        }
        total
    }

    /// The nav rail. Like Slack, Home and DMs get an unread dot, Activity its unread count.
    fn workspace_rail(
        &self,
        home_dot: bool,
        dm_dot: bool,
        activity_badge: u32,
        cx: &mut Context<Self>,
    ) -> Div {
        // Owned color copies: `cx.theme()` borrows `cx`, so nothing may outlive the
        // statement that reads it across the `&mut Context` calls below.
        let border = cx.theme().sidebar_border;
        let rail_bg = cx.theme().sidebar;
        let mut rail = v_flex()
            .flex_none()
            .h_full()
            .w(px(72.))
            .items_center()
            .pt_3()
            .pb_3()
            .gap_1()
            .children(
                self.fixtures
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(index, team)| self.workspace_tile(index, team, cx))
                    .collect::<Vec<_>>(),
            )
            .child(div().h(px(6.)));
        for item in NavItem::ALL {
            if item == NavItem::Admin {
                rail = rail.child(div().my_1().w(px(28.)).h(px(1.)).bg(border));
            }
            let (badge, dot) = match item {
                NavItem::Home => (0, home_dot),
                NavItem::Dms => (0, dm_dot),
                NavItem::Activity => (activity_badge, false),
                _ => (0, false),
            };
            rail = rail.child(self.nav_tile(item, badge, dot, cx));
        }
        let avatar = self_avatar(&self.fixtures, &self.self_state, 36., rail_bg, cx);
        rail.child(div().flex_1())
            .child(self.rail_button("rail-create", IconName::Plus, cx))
            .child(
                self.rail_button("rail-theme", IconName::Moon, cx)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::ToggleTheme))),
            )
            .child(
                div()
                    .id("rail-avatar")
                    .mt_1()
                    .cursor_pointer()
                    .child(avatar)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenUserMenu))),
            )
    }

    /// One 36px round button at the foot of the rail ("+" create, the theme moon).
    fn rail_button(&self, id: &'static str, icon: IconName, cx: &Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let fg = theme.sidebar_foreground.opacity(0.8);
        let bg = theme.sidebar_foreground.opacity(0.12);
        let hover = theme.sidebar_foreground.opacity(0.2);
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
            .hover(|button| button.bg(hover))
            .child(Icon::new(icon).text_color(fg).size(px(18.)))
    }

    fn workspace_tile(
        &self,
        index: usize,
        team: &Team,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let active = index == self.active_workspace;
        let face = match team_icon_url(team) {
            Some(url) => img(SharedString::from(url)).size_full().into_any_element(),
            None => div().child(team_initials(&team.name)).into_any_element(),
        };
        div()
            .id(SharedString::from(format!("workspace-{index}")))
            .size(px(36.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_lg()
            .overflow_hidden()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .when(active, |tile| {
                tile.bg(theme.sidebar_primary)
                    .text_color(theme.sidebar_primary_foreground)
            })
            .when(!active, |tile| {
                tile.bg(theme.sidebar_accent.opacity(0.25))
                    .text_color(theme.sidebar_accent_foreground)
                    .hover(|tile| tile.bg(theme.secondary_hover))
            })
            .child(face)
            .on_click(cx.listener(move |this, _, _, cx| {
                if index != this.active_workspace {
                    cx.emit(SidebarEvent::SwitchWorkspace(index));
                }
            }))
    }

    fn workspace_header(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let team = &self.fixtures.workspaces[self.active_workspace];
        let dim = theme.sidebar_foreground.opacity(0.6);
        let hover = theme.accent;
        h_flex()
            .px_4()
            .pt_3()
            .pb_1()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child(team.name.clone()),
            )
            .child(
                Icon::new(IconName::ChevronDown)
                    .text_color(dim)
                    .size(px(14.)),
            )
            .child(self.header_icon("ws-settings", IconName::Settings, dim, hover))
            .child(self.header_icon("ws-compose", IconName::SquarePen, dim, hover))
    }

    /// One 24px hover-square for the workspace header's gear / compose icons.
    fn header_icon(
        &self,
        id: &'static str,
        icon: IconName,
        dim: Hsla,
        hover: Hsla,
    ) -> Stateful<Div> {
        div()
            .id(SharedString::from(id))
            .flex_none()
            .size(px(24.))
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(dim)
            .hover(|el| el.bg(hover))
            .child(Icon::new(icon).text_color(dim).size(px(15.)))
    }

    /// The "Find a conversation…" field between the workspace header and the sections.
    fn find_field(&self, cx: &Context<Self>) -> Div {
        let border = cx.theme().sidebar_border;
        let dim = cx.theme().sidebar_foreground.opacity(0.6);
        h_flex()
            .mx_2()
            .mt_1()
            .flex_none()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_0p5()
            .rounded_md()
            .border_1()
            .border_color(border)
            .child(Icon::new(IconName::Search).text_color(dim).size(px(14.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(13.))
                    .child(Textarea::new(&self.find).appearance(false).bordered(false)),
            )
    }

    /// One rail destination: icon over its label, Slack-rail style, with an optional
    /// count badge riding the icon's top-right corner.
    fn nav_tile(
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
        let tile_bg = theme.sidebar_foreground.opacity(0.16);
        let hover = theme.sidebar_foreground.opacity(0.08);
        let (badge_bg, badge_fg) = (theme.danger, theme.danger_foreground);
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
            .child(
                div()
                    .w_full()
                    .px_1()
                    .text_center()
                    .text_size(px(11.))
                    .line_height(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(item.label()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.active_nav = item;
                cx.notify();
            }))
    }

    /// A sidebar section: a dim header row (section emoji or icon + name; click collapses)
    /// over its conversation rows. Like Slack, a collapsed section still lists its unread and
    /// open conversations. An expanded Channels section ends with the "Add channels" row.
    fn section(
        &self,
        key: SharedString,
        leading: Option<AnyElement>,
        title: &str,
        conversations: &[Conversation],
        watermark: Option<&Ts>,
        cx: &mut Context<Self>,
    ) -> Div {
        let add_channels =
            key.as_ref() == "channels"
                || self.fixtures.sections.iter().any(|section| {
                    section.kind == SectionKind::Channels && section.id == key.as_ref()
                });
        let theme = cx.theme();
        let dim = theme.sidebar_foreground.opacity(0.7);
        let hover = theme.sidebar_foreground.opacity(0.08);
        let open = !self.collapsed.contains(&key);
        let toggle = key.clone();
        v_flex()
            .px_2()
            .pt_2()
            .child(
                h_flex()
                    .id(SharedString::from(format!("section-{key}")))
                    .h(px(28.))
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(dim)
                    .hover(|row| row.bg(hover))
                    .child(div().w(px(18.)).flex().justify_center().children(leading))
                    .child(div().flex_1().truncate().child(title.to_owned()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed.remove(&toggle) {
                            this.collapsed.insert(toggle.clone());
                        }
                        cx.notify();
                    })),
            )
            .children(
                conversations
                    .iter()
                    .filter(|conversation| {
                        open || conversation.id == self.selected
                            || self.unread_in(conversation, watermark).count > 0
                    })
                    .map(|conversation| self.conversation_row(conversation, watermark, cx))
                    .collect::<Vec<_>>(),
            )
            .when(open && add_channels, |section| {
                section.child(self.add_channels_row(cx))
            })
    }

    /// The sections under the nav rows. Live: the user's own sidebar sections in their order,
    /// the built-in Channels and Direct messages buckets holding every conversation no other
    /// section claims, as in Slack. Sample workspace: plain Channels and Direct messages.
    fn section_list(
        &self,
        channels: &[Conversation],
        watermark: Option<&Ts>,
        cx: &mut Context<Self>,
    ) -> Vec<Div> {
        if self.fixtures.sections.is_empty() {
            return vec![
                self.section(
                    "channels".into(),
                    Some(caret()),
                    "Channels",
                    channels,
                    watermark,
                    cx,
                ),
                self.section(
                    "dms".into(),
                    Some(caret()),
                    "Direct messages",
                    &self.fixtures.dms,
                    watermark,
                    cx,
                ),
            ];
        }
        // Slack fills External connections itself, from the externally shared channels.
        let connect = self
            .fixtures
            .sections
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
        let mut ranked: Vec<&ChannelSection> = self
            .fixtures
            .sections
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
        let mut list = Vec::new();
        for section in &self.fixtures.sections {
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
            if rows.is_empty() && !builtin(&section.kind) {
                continue;
            }
            // Slack's default sort: DMs by latest activity, everything else by name.
            if section.kind == SectionKind::DirectMessages {
                let latest = |c: &Conversation| {
                    self.fixtures
                        .counts
                        .get(&c.id)
                        .and_then(|n| n.latest.clone())
                };
                rows.sort_by_key(|c| std::cmp::Reverse(latest(c)));
            } else {
                rows.sort_by_cached_key(|c| {
                    let label = self.fixtures.conversation_label(c);
                    label.trim_start_matches("# ").to_lowercase()
                });
            }
            let (title, leading) = section_title(section, &self.fixtures);
            list.push(self.section(
                section.id.clone().into(),
                leading,
                &title,
                &rows,
                watermark,
                cx,
            ));
        }
        list
    }

    /// "Add channels" under the channel list: opens the channel browser.
    fn add_channels_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dim = cx.theme().sidebar_foreground.opacity(0.6);
        let hover = cx.theme().accent;
        let tile = cx.theme().secondary;
        h_flex()
            .id("browse-channels")
            .h(px(28.))
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
    fn nav_row(
        &self,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        bold: bool,
        trailing: Option<String>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let color = if bold {
            theme.sidebar_foreground
        } else {
            theme.sidebar_foreground.opacity(0.75)
        };
        let hover = theme.sidebar_foreground.opacity(0.08);
        h_flex()
            .id(id)
            .mx_2()
            .px_2()
            .h(px(28.))
            .gap_2()
            .items_center()
            .rounded_md()
            .text_color(color)
            .cursor_pointer()
            .hover(|row| row.bg(hover))
            .child(Icon::new(icon).text_color(color).size(px(16.)))
            .child(
                div()
                    .flex_1()
                    .font_weight(if bold {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .child(label),
            )
            .children(trailing)
    }

    /// Bold title row heading the DM inbox and Activity lists.
    fn list_title(&self, title: &str, cx: &Context<Self>) -> Div {
        let border = cx.theme().sidebar_border;
        h_flex()
            .px_4()
            .pt_3()
            .pb_2()
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::BOLD)
                    .child(title.to_owned()),
            )
    }

    /// The rail's DMs destination: every DM, newest activity first, with presence or
    /// stacked group avatar, last message preview and its time.
    fn dm_list(&self, watermark: Option<&Ts>, cx: &mut Context<Self>) -> Div {
        let mut dms: Vec<(&Conversation, Option<&Message>)> = self
            .fixtures
            .dms
            .iter()
            .map(|dm| {
                (
                    dm,
                    self.fixtures.messages.get(&dm.id).and_then(|h| h.last()),
                )
            })
            .collect();
        dms.sort_by(|a, b| b.1.map(|m| &m.ts).cmp(&a.1.map(|m| &m.ts)));
        let rows: Vec<_> = dms
            .into_iter()
            .map(|(dm, last)| self.dm_row(dm, last, watermark, cx))
            .collect();
        v_flex()
            .child(self.list_title("Direct messages", cx))
            .child(v_flex().p_2().gap_0p5().children(rows))
    }

    fn dm_row(
        &self,
        dm: &Conversation,
        last: Option<&Message>,
        watermark: Option<&Ts>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let selected = dm.id == self.selected;
        let unread = self.unread_in(dm, watermark);
        let (fg, dim) = if selected {
            (
                theme.sidebar_accent_foreground,
                theme.sidebar_accent_foreground.opacity(0.8),
            )
        } else {
            (
                theme.sidebar_foreground,
                theme.sidebar_foreground.opacity(0.6),
            )
        };
        let ring = if selected {
            theme.sidebar_accent
        } else {
            theme.sidebar
        };
        let (selected_bg, hover) = (theme.sidebar_accent, theme.accent);
        let preview = last.map(|message| {
            let author = match &message.user {
                Some(user) if *user == self.fixtures.self_user => "You: ".to_owned(),
                _ if dm.is_mpim => message
                    .user
                    .as_ref()
                    .map(|user| format!("{}: ", self.fixtures.user_name(user)))
                    .unwrap_or_default(),
                _ => String::new(),
            };
            format!("{author}{}", self.fixtures.preview(message))
        });
        let stamp = last
            .map(|message| list_stamp(&message.ts))
            .unwrap_or_default();
        let id = dm.id.clone();
        h_flex()
            .id(SharedString::from(format!("dm-{id}")))
            .px_2()
            .py_1p5()
            .gap_2()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |row| row.bg(selected_bg))
            .hover(|row| row.bg(hover))
            .child(conversation_avatar(&self.fixtures, dm, 32., ring, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .text_color(fg)
                                    .font_weight(if unread.count > 0 {
                                        FontWeight::BOLD
                                    } else {
                                        FontWeight::SEMIBOLD
                                    })
                                    .child(self.fixtures.conversation_label(dm)),
                            )
                            .child(div().flex_none().text_xs().text_color(dim).child(stamp)),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(dim)
                            .child(preview.unwrap_or_default()),
                    ),
            )
            .when(unread.count > 0, |row| {
                row.child(count_pill(unread.count, unread.mentions > 0, cx))
            })
            .on_click(cx.listener(move |this, _, _, cx| this.select_row(id.clone(), cx)))
    }

    fn conversation_row(
        &self,
        conversation: &Conversation,
        watermark: Option<&Ts>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let id = conversation.id.clone();
        let selected = id == self.selected;
        let unread = self.unread_in(conversation, watermark);
        let label = self.fixtures.conversation_label(conversation);
        let emphasized = selected || unread.count > 0;
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
                conversation_avatar(
                    &self.fixtures,
                    conversation,
                    20.,
                    if selected {
                        theme.sidebar_accent
                    } else {
                        theme.sidebar
                    },
                    cx,
                )
                .into_any_element(),
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

        let mut row = h_flex()
            .id(SharedString::from(format!("conversation-{id}")))
            .h(px(28.))
            .pl(px(24.))
            .pr_2()
            .gap_2()
            .items_center()
            .rounded_sm()
            .text_color(color)
            .cursor_pointer()
            .when(selected, |row| row.bg(theme.sidebar_accent))
            .hover(|row| row.bg(theme.accent))
            .child(leading)
            .child(div().flex_1().truncate().font_weight(weight).child(label));
        if self.huddle.as_ref() == Some(&conversation.id) {
            let live = if selected { color } else { theme.success };
            row = row.child(
                Icon::new(IconName::Headphones)
                    .text_color(live)
                    .size(px(14.)),
            );
        }
        if unread.mentions > 0 {
            row = row.child(count_pill(unread.mentions, true, cx));
        }
        row.on_click(cx.listener(move |this, _, _, cx| this.select_row(id.clone(), cx)))
    }
}

/// Slack-style inline count pill: red for mentions (and DM messages), the sidebar's primary
/// emphasis colors otherwise.
fn count_pill(count: u32, mentions: bool, cx: &gpui_kit::gpui::App) -> Div {
    let theme = cx.theme();
    let (bg, fg) = if mentions {
        (theme.danger, theme.danger_foreground)
    } else {
        (theme.sidebar_primary, theme.sidebar_primary_foreground)
    };
    h_flex()
        .flex_none()
        .justify_center()
        .gap_0p5()
        .min_w(px(18.))
        .px_1p5()
        .py_0p5()
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .child(count.to_string())
}

/// The chevron leading a section header that has no emoji or icon of its own.
fn caret() -> AnyElement {
    Icon::new(IconName::ChevronDown)
        .size(px(14.))
        .into_any_element()
}

/// A section's header: Slack's current label for a built-in bucket (the stored names lag it:
/// "Slack Connect", "Direct Messages"), else the user's name, led by the section emoji (when
/// it resolves) or the bucket's icon.
fn section_title(section: &ChannelSection, fixtures: &Fixtures) -> (String, Option<AnyElement>) {
    let icon = |name: IconName| Some(Icon::new(name).size(px(15.)).into_any_element());
    let emoji = &section.emoji;
    let resolves = emoji::display_glyph(emoji).is_some() || fixtures.custom_emoji(emoji).is_some();
    let (builtin_name, leading) = match section.kind {
        SectionKind::Channels => (Some("Channels"), Some(caret())),
        SectionKind::DirectMessages => (Some("Direct messages"), Some(caret())),
        SectionKind::Stars => (Some("Starred"), icon(IconName::Star)),
        SectionKind::SlackConnect => (Some("External connections"), icon(IconName::Building2)),
        SectionKind::RecentApps => (Some("Apps"), icon(IconName::LayoutGrid)),
        SectionKind::Agents => (Some("Agents"), icon(IconName::Bot)),
        _ if !emoji.is_empty() && resolves => {
            (None, Some(emoji::emoji_element(emoji, fixtures, px(15.))))
        }
        _ => (None, Some(caret())),
    };
    let name = builtin_name.map_or_else(|| section.name.clone(), str::to_owned);
    (name, leading)
}

/// The workspace icon Slack serves (`team.icon.image_88`, ...), unless it is the generated
/// default.
fn team_icon_url(team: &Team) -> Option<String> {
    let icon = team.icon.as_ref()?;
    if icon.get("image_default").and_then(|value| value.as_bool()) == Some(true) {
        return None;
    }
    ["image_88", "image_68", "image_132", "image_44"]
        .iter()
        .find_map(|key| icon.get(*key)?.as_str())
        .map(str::to_owned)
}

/// Up to the first two initials of a workspace name, e.g. "Acme Corp" -> "AC".
pub(crate) fn team_initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

/// Whether `message` @-mentions `user`, in the mrkdwn fallback text or the rich-text blocks.
pub(crate) fn mentions_user(message: &Message, user: &UserId) -> bool {
    let needle = format!("<@{}>", user.as_str());
    message.text.contains(&needle)
        || message.blocks.iter().any(|block| match block {
            Block::RichText(rich) => rich
                .elements
                .iter()
                .any(|container| container_mentions_user(container, user)),
            Block::Unknown { .. } => false,
        })
}

fn container_mentions_user(container: &RichTextContainer, user: &UserId) -> bool {
    match container {
        RichTextContainer::Section(section) => section
            .elements
            .iter()
            .any(|element| element_mentions_user(element, user)),
        RichTextContainer::List(list) => list
            .elements
            .iter()
            .any(|nested| container_mentions_user(nested, user)),
        RichTextContainer::Quote(quote) => quote
            .elements
            .iter()
            .any(|element| element_mentions_user(element, user)),
        RichTextContainer::Preformatted(pre) => pre
            .elements
            .iter()
            .any(|element| element_mentions_user(element, user)),
        RichTextContainer::Unknown { .. } => false,
    }
}

fn element_mentions_user(element: &RichTextElement, user: &UserId) -> bool {
    matches!(element, RichTextElement::User(mention) if mention.user_id == *user)
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Owned color copies: `cx.theme()` borrows `cx`, so nothing may outlive the
        // statement that reads it across the `&mut Context` calls below.
        let sidebar_bg = cx.theme().sidebar;
        let sidebar_border = cx.theme().sidebar_border;
        let sidebar_fg = cx.theme().sidebar_foreground;
        let watermark = self.read_watermark();
        let channels: Vec<Conversation> = self
            .fixtures
            .channels
            .iter()
            .chain(&self.joined)
            .cloned()
            .collect();
        let channel_unread = self.total_unread(&channels, watermark.as_ref());
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
            .border_l_1()
            .border_color(sidebar_border)
            .pb_2();
        let list = match self.active_nav {
            NavItem::Dms => list.child(self.dm_list(watermark.as_ref(), cx)),
            NavItem::Activity => list.child(self.activity.clone()),
            NavItem::Home | NavItem::Files | NavItem::Todos | NavItem::Agents | NavItem::Admin => {
                list.child(self.workspace_header(cx))
                    .child(self.find_field(cx))
                    .child(div().mx_2().my_2().h(px(1.)).bg(sidebar_border))
                    .child(
                        self.nav_row(
                            "unreads",
                            IconName::Inbox,
                            "Unreads",
                            total.count > 0,
                            None,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.active_nav = NavItem::Activity;
                            cx.notify();
                        })),
                    )
                    .child(self.nav_row(
                        "threads",
                        IconName::MessageSquareText,
                        "Threads",
                        false,
                        None,
                        cx,
                    ))
                    .child(self.nav_row(
                        "huddles",
                        IconName::Headphones,
                        "Huddles",
                        false,
                        None,
                        cx,
                    ))
                    .child(
                        self.nav_row(
                            "drafts",
                            IconName::Send,
                            "Drafts & sent",
                            false,
                            (self.draft_count > 0).then(|| self.draft_count.to_string()),
                            cx,
                        )
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenDrafts))),
                    )
                    .child(
                        self.nav_row(
                            "saved",
                            IconName::Bookmark,
                            "Saved",
                            false,
                            (self.later_count > 0).then(|| self.later_count.to_string()),
                            cx,
                        )
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenLater))),
                    )
                    .child(self.nav_row(
                        "directories",
                        IconName::BookUser,
                        "Directories",
                        false,
                        None,
                        cx,
                    ))
                    .child(div().mx_4().my_2().h(px(1.)).bg(sidebar_border))
                    .children(self.section_list(&channels, watermark.as_ref(), cx))
            }
        };

        h_flex()
            .size_full()
            .bg(sidebar_bg)
            .text_color(sidebar_fg)
            .text_sm()
            .child(self.workspace_rail(
                channel_unread.count > 0,
                dm_unread.count > 0,
                self.activity.read(cx).unread_count(),
                cx,
            ))
            .child(list)
    }
}
