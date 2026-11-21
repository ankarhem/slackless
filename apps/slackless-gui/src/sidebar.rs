//! Left sidebar: Slack's workspace switcher column, the tab rail with the nav stack (Home /
//! DMs / Activity / Files / To-dos / Admin) and control strip (create, focus mode, avatar),
//! which the root places itself ([`Sidebar::workspace_switcher`], [`Sidebar::tab_rail`]), and
//! this view's list column: collapsible Channels / Direct messages sections with unread and
//! mention count pills (gpui-kit-guide section 3). In focus mode the list drops its bolding
//! and counts for Slack's subtle dots. Counts are derived from the fixture
//! history so fixture mode renders exactly what live mode will feed it.
//!
//! The rail's DMs destination swaps the list for the DM inbox (a "Direct
//! messages" header with Slack's Unreads filter toggle and a "Find a DM…" field over every
//! DM with its presence/group avatar, bold name, relative stamp and latest-message preview);
//! the Activity destination is a full-width page the root renders beside the rail, so the
//! sidebar shows only the rail while it is active ([`crate::activity`]); the Files
//! destination swaps the list for Slack's Files list column (a "Files" header over the All
//! files, Canvases and Lists rows), whose pages the root renders in the main area
//! ([`crate::files_page`]); Home shows the
//! Unreads, Threads, Huddles, Drafts & sent, Saved and Directories rows above the sections
//! and an "Add channels" row that asks the root to open the channel browser.
//! [`Sidebar::badge_count`] is the dock badge: unread activity (pushed by the root from the
//! live feed) plus unread DMs.
//!
//! Slack's floating "More unreads" pill rides the list's bottom (or top) edge whenever
//! unread rows sit outside the scrolled view: the sidebar's primary colors while any hidden
//! row carries mentions, otherwise a neutral raised chip; a click scrolls the next hidden
//! unread row into view.
//!
//! Everything visual comes from the theme's sidebar palette (`sidebar*` tokens); selected
//! rows use `sidebar_accent`, hover rows the theme's neutral hover wash (`accent`). The only
//! derived colors are opacity steps of `sidebar_foreground` for idle rows and icons.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, Size, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, ScrollHandle,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    deferred, div, img, point, px, transparent_black,
};
use slackless_core::types::{
    ChannelId, ChannelSection, Conversation, ConversationKind, Message, SectionKind, Team, Ts,
    UserId,
    rich_text::{Block, RichTextContainer, RichTextElement},
};

use crate::avatars::conversation_avatar;
use crate::emoji;
use crate::files_page::FilesSection;
use crate::fixtures::Fixtures;
use crate::messages::list_stamp;
use crate::sidebar_menu::{SectionFormEvent, SectionNameForm, workspace_menu};
use crate::status::{FOCUS_DURATIONS, SelfState, self_avatar};

/// Slack's workspace switcher column, left of the tab rail.
pub(crate) const SWITCHER_WIDTH: f32 = 60.;
/// Slack's tab rail.
const TAB_RAIL_WIDTH: f32 = 70.;

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
    /// The "Threads" row: open the Threads view.
    OpenThreads,
    /// The "Huddles" row: open the Huddles page.
    OpenHuddles,
    /// The "Directories" row: open the directory.
    OpenDirectories,
    /// The workspace header's pen: open the "New message" view.
    OpenCompose,
    /// The gear menu's "Create a channel section" dialog confirmed `name`.
    CreateSection {
        name: String,
    },
    /// The rail's moon: turn focus mode on or off.
    ToggleFocus,
    /// The focus card's Duration choice (an index into `status::FOCUS_DURATIONS`).
    FocusDuration(usize),
    /// The rail's own avatar: open the user menu.
    OpenUserMenu,
    /// The rail's Activity tile (or the Home list's Unreads row): open the Activity page,
    /// which the root renders beside the rail instead of this sidebar's list column.
    OpenActivity,
    /// A rail destination other than Activity was shown while the Activity page was open;
    /// the root stops rendering the page.
    LeftActivity,
    /// The rail's Files tile: open the Files view, whose page the root renders in the main
    /// area beside this sidebar's Files list column.
    OpenFiles,
    /// A Files list-column row: show that page of the Files view.
    ShowFilesSection(FilesSection),
}

/// The nav destinations stacked in the workspace rail.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
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
    /// The Files list column's selected row: the page the Files view shows.
    files_section: FilesSection,
    /// Each rail destination's list scroll, restored when the reader comes back to it.
    nav_offsets: HashMap<NavItem, Point<Pixels>>,
    /// Keys of the collapsed sidebar sections.
    collapsed: HashSet<SharedString>,
    /// Conversations read in this session since Slack's counts were taken.
    read: HashSet<ChannelId>,
    /// The "Find a conversation…" field above the sections.
    find: Entity<TextareaState>,
    /// The "Find a conversation…" query, trimmed and lowercased; empty lists everything.
    find_query: String,
    /// The DM list's "Find a DM…" field.
    dm_find: Entity<TextareaState>,
    /// The DM list's "Find a DM…" query, trimmed and lowercased; empty lists every DM.
    dm_find_query: String,
    /// Live mode: DMs' newest messages read from the store since the last snapshot (a send,
    /// an arrival, an edit or a deletion), fresher than the snapshot's `latest`; `None`: the
    /// DM has no message left.
    fresh_latest: HashMap<ChannelId, Option<Message>>,
    /// Slack's thread state (`client.counts`): whether followed threads have unread replies,
    /// and how many of those mention you.
    threads_unread: (bool, u32),
    /// The DM list's "Unreads" toggle: only DMs with unread messages.
    dm_unreads_only: bool,
    /// Channels joined from the channel browser this session, listed after the fixtures'.
    joined: Vec<Conversation>,
    /// The conversation with a live huddle; its row carries the headphones indicator.
    huddle: Option<ChannelId>,
    /// Saved drafts, shown on the "Drafts & sent" row.
    draft_count: usize,
    /// In-progress Later items, shown on the "Later" row.
    later_count: usize,
    /// The gear menu is open (a scrim covers the sidebar list under it).
    settings_menu_open: bool,
    /// Slack's "Active only" sidebar filter (Slack's default): hide conversations with no
    /// activity in 30 days.
    active_only: bool,
    /// Sections created locally this session (fixture mode; live ones arrive via the store).
    local_sections: Vec<ChannelSection>,
    /// The "Create a channel section" dialog body.
    section_form: Entity<SectionNameForm>,
    /// Tracks the sidebar list's scroll state; the "More unreads" pills read which unread
    /// rows the scroll offset carries out of view.
    scroll: ScrollHandle,
    /// Unread Activity rows (the root pushes the live feed's count; fixture mode derives
    /// it), for the rail tile and the dock badge.
    activity_unread: u32,
    /// The signed-in user's status and presence, for the rail avatar.
    self_state: SelfState,
    /// The pointer is over the focus button / the focus card.
    focus_hover: (bool, bool),
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
            let dm_find = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Find a DM…")
                    .auto_grow(1, 1)
            });
            let section_form = SectionNameForm::new(window, cx);
            let collapsed = fixtures
                .collapsed_sections
                .iter()
                .cloned()
                .map(SharedString::from)
                .collect();
            let subscriptions = vec![
                cx.subscribe(&find, |this: &mut Sidebar, find, event: &InputEvent, cx| {
                    if let InputEvent::Change = event {
                        this.find_query = find.read(cx).value().trim().to_lowercase();
                        cx.notify();
                    }
                }),
                cx.subscribe(
                    &dm_find,
                    |this: &mut Sidebar, find, event: &InputEvent, cx| {
                        if let InputEvent::Change = event {
                            this.dm_find_query = find.read(cx).value().trim().to_lowercase();
                            cx.notify();
                        }
                    },
                ),
                cx.subscribe_in(
                    &section_form,
                    window,
                    |_: &mut Sidebar, _, event: &SectionFormEvent, window, cx| {
                        let SectionFormEvent::Create(name) = event;
                        window.close_dialog(cx);
                        cx.emit(SidebarEvent::CreateSection { name: name.clone() });
                    },
                ),
            ];
            Sidebar {
                fixtures,
                selected,
                active_workspace: 0,
                active_nav: NavItem::Home,
                files_section: FilesSection::All,
                nav_offsets: HashMap::new(),
                collapsed,
                read: HashSet::new(),
                find_query: String::new(),
                find,
                dm_find,
                dm_find_query: String::new(),
                fresh_latest: HashMap::new(),
                threads_unread: (false, 0),
                dm_unreads_only: false,
                joined: Vec::new(),
                huddle: None,
                draft_count: 0,
                later_count: 0,
                settings_menu_open: false,
                active_only: true,
                local_sections: Vec::new(),
                section_form,
                scroll: ScrollHandle::new(),
                activity_unread: 0,
                self_state: SelfState::default(),
                focus_hover: (false, false),
                _subscriptions: subscriptions,
            }
        })
    }

    /// Live refresh: draw `fixtures` while keeping the reader's scroll, collapsed sections and
    /// read marks. A changed collapse pref from Slack still lands.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        if fixtures.collapsed_sections != self.fixtures.collapsed_sections {
            self.collapsed = fixtures
                .collapsed_sections
                .iter()
                .cloned()
                .map(SharedString::from)
                .collect();
        }
        // The snapshot read the store after every fresher read kept here.
        self.fresh_latest.clear();
        self.fixtures = fixtures;
        cx.notify();
    }

    /// Live mode: Slack's thread counts (unread replies in followed threads, mentions among
    /// them) for the Threads row.
    pub fn set_thread_counts(&mut self, has_unreads: bool, mentions: u32, cx: &mut Context<Self>) {
        self.threads_unread = (has_unreads, mentions);
        cx.notify();
    }

    /// Live mode: `channel`'s newest message as just re-read from the store, so the DM list's
    /// preview, stamp and order follow it before the next snapshot.
    pub fn set_latest(
        &mut self,
        channel: ChannelId,
        latest: Option<Message>,
        cx: &mut Context<Self>,
    ) {
        self.fresh_latest.insert(channel, latest);
        cx.notify();
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

    /// Show a rail destination (keyboard shortcuts; the rail tile and the Unreads row route
    /// Activity through the root, which owns the page).
    pub(crate) fn show_nav(&mut self, item: NavItem, cx: &mut Context<Self>) {
        let leaving_activity = self.active_nav == NavItem::Activity && item != NavItem::Activity;
        // One scroll container serves every destination: each keeps its own position, and a
        // first visit starts at the top.
        self.nav_offsets
            .insert(self.active_nav, self.scroll.offset());
        let offset = self.nav_offsets.get(&item).copied();
        self.scroll
            .set_offset(offset.unwrap_or_else(|| point(px(0.), px(0.))));
        self.active_nav = item;
        if leaving_activity {
            cx.emit(SidebarEvent::LeftActivity);
        }
        cx.notify();
    }

    /// Leave the Activity or Files destination, whose page owns the main area (a
    /// conversation or another full-column view takes over); nothing to do from any other
    /// destination.
    pub(crate) fn leave_pages(&mut self, cx: &mut Context<Self>) {
        if matches!(self.active_nav, NavItem::Activity | NavItem::Files) {
            self.show_nav(NavItem::Home, cx);
        }
    }

    /// Unread Activity rows (the root pushes the live feed's count; fixture mode derives
    /// it), for the rail tile and the dock badge.
    pub fn set_activity_unread(&mut self, count: u32, cx: &mut Context<Self>) {
        self.activity_unread = count;
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

    /// Show `channel` as read (the root just marked it read in Slack).
    pub fn mark_read(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        self.read.insert(channel);
        cx.notify();
    }

    /// The signed-in user's status and presence shown on the rail avatar.
    pub fn set_self_state(&mut self, state: SelfState, cx: &mut Context<Self>) {
        self.self_state = state;
        cx.notify();
    }

    /// Open the workspace gear menu (the demo capture hook; a click does the same).
    pub fn show_settings_menu(&mut self, cx: &mut Context<Self>) {
        self.settings_menu_open = true;
        cx.notify();
    }

    /// The dock badge count: unread Activity items plus unread DM messages.
    pub fn badge_count(&self) -> u32 {
        let watermark = self.read_watermark();
        self.activity_unread
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

    /// List a section created locally (fixture mode) after the stored ones.
    pub fn add_section(&mut self, name: String, cx: &mut Context<Self>) {
        let index = self.local_sections.len() + 1;
        self.local_sections.push(ChannelSection {
            id: format!("LLOCAL{index}"),
            name,
            kind: SectionKind::Standard,
            emoji: String::new(),
            next_id: None,
            channel_ids: Vec::new(),
        });
        cx.notify();
    }

    /// Slack's "Active only" rule (docs/research/slack-features-shell.md F-SHELL-068): a
    /// conversation stays listed when it saw activity in the last 30 days. Unknown activity
    /// keeps it, so a workspace without counts never blanks its sidebar.
    fn active_recent(&self, conversation: &Conversation) -> bool {
        if !self.active_only {
            return true;
        }
        let latest = self
            .fixtures
            .counts
            .get(&conversation.id)
            .and_then(|counts| counts.latest.clone())
            .or_else(|| {
                self.fixtures
                    .messages
                    .get(&conversation.id)
                    .and_then(|history| history.last())
                    .map(|message| message.ts.clone())
            });
        // A change to the channel itself (members, connection, settings; `updated`, millis)
        // counts too: Slack lists #partner_support_klarna, silent since April 2025 but updated
        // within the month.
        let updated = conversation
            .updated
            .map(|at| if at > 100_000_000_000 { at / 1000 } else { at });
        let cutoff = crate::drafts::now_secs().saturating_sub(30 * 86_400);
        latest
            .and_then(|ts| ts.secs())
            .max(updated)
            .and_then(|secs| u64::try_from(secs).ok())
            .is_none_or(|secs| secs >= cutoff)
    }

    /// The gear menu's "Create a channel section": the dialog over a fresh name field.
    fn open_section_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.section_form
            .update(cx, |form, cx| form.open(window, cx));
        let form = self.section_form.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Create a channel section")
                .w(px(440.))
                .child(form.clone())
        });
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
            if conversation.id == self.selected || self.read.contains(&conversation.id) {
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

    /// Conversations with unread messages, in sidebar row order: the search pill's
    /// dropdown "Unreads & drafts" section (the root adds drafted conversations).
    pub fn unread_ids(&self) -> Vec<ChannelId> {
        let watermark = self.read_watermark();
        self.fixtures
            .channels
            .iter()
            .chain(&self.fixtures.dms)
            .filter(|conversation| self.unread_in(conversation, watermark.as_ref()).count > 0)
            .map(|conversation| conversation.id.clone())
            .collect()
    }

    /// The search dropdown's unread state, alongside [`Self::unread_ids`]: per conversation
    /// unread-mention counts (conversations with any only), and the followed-threads state
    /// (unread replies present, mentions among them) for its Threads row and badge.
    pub fn search_unread_counts(&self) -> (Vec<(ChannelId, u32)>, (bool, u32)) {
        let watermark = self.read_watermark();
        let mentions = self
            .fixtures
            .channels
            .iter()
            .chain(&self.fixtures.dms)
            .filter_map(|conversation| {
                let mentions = self.unread_in(conversation, watermark.as_ref()).mentions;
                (mentions > 0).then(|| (conversation.id.clone(), mentions))
            })
            .collect();
        (mentions, self.threads_unread)
    }

    /// The rail destination the list column shows.
    pub(crate) fn active_nav(&self) -> NavItem {
        self.active_nav
    }

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
        let (outline, dark, avatar_ring) =
            (theme.sidebar_border, theme.mode.is_dark(), theme.sidebar);
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
        for item in NavItem::ALL {
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
        rail.child(div().flex_1()).child(
            v_flex()
                .items_center()
                .gap_4()
                .child(self.rail_button("rail-create", IconName::Plus, false, cx))
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
    fn rail_button(
        &self,
        id: &'static str,
        icon: IconName,
        active: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let (fg, bg) = if active {
            (theme.muted_foreground, theme.background)
        } else {
            (
                theme.sidebar_foreground.opacity(0.8),
                theme.sidebar_foreground.opacity(0.3),
            )
        };
        let hover = theme.sidebar_foreground.opacity(0.4);
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
    fn focus_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
    fn focus_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

    fn workspace_tile(
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
        // Slack rings the active workspace's icon; the others take a faint ring on hover.
        div()
            .id(SharedString::from(format!("workspace-{index}")))
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
    fn find_field(&self, field: &Entity<TextareaState>, cx: &Context<Self>) -> Div {
        let border = cx.theme().sidebar_border;
        let dim = cx.theme().sidebar_foreground.opacity(0.6);
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
            .child(
                Icon::new(IconName::TextSearch)
                    .text_color(dim)
                    .size(px(16.)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(13.))
                    .child(Textarea::new(field).appearance(false).bordered(false)),
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
        let tile_bg = theme.sidebar_foreground.opacity(0.32);
        let hover = theme.sidebar_foreground.opacity(0.08);
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
            .on_click(cx.listener(move |this, _, _, cx| match item {
                // Both destinations' pages live in the root, which opens them.
                NavItem::Activity => cx.emit(SidebarEvent::OpenActivity),
                NavItem::Files => cx.emit(SidebarEvent::OpenFiles),
                _ => this.show_nav(item, cx),
            }))
    }

    /// A sidebar section: a dim header row (section emoji or icon + name; click collapses)
    /// over its conversation rows. Like Slack, a collapsed section still lists its unread and
    /// open conversations. An expanded Channels section ends with the "Add channels" row.
    /// The section flattens into direct children of the scroll list (each carrying the
    /// section's own padding) so its scroll handle can track every row.
    fn section(
        &self,
        key: SharedString,
        leading: Option<AnyElement>,
        title: &str,
        conversations: &[Conversation],
        watermark: Option<&Ts>,
        cx: &mut Context<Self>,
    ) -> Vec<(AnyElement, ListChild)> {
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
        let header = h_flex()
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
            }));
        let mut children = vec![(
            v_flex().px_2().pt_2().child(header).into_any_element(),
            ListChild::Plain,
        )];
        for conversation in conversations.iter() {
            let unread = self.unread_in(conversation, watermark);
            // Like Slack, a collapsed section still lists its unread and open conversations;
            // "Active only" additionally hides stale ones from expanded sections.
            // A "Find a conversation…" query instead lists every match, collapsed or stale.
            let listed = if self.find_query.is_empty() {
                conversation.id == self.selected
                    || unread.count > 0
                    || (open && self.active_recent(conversation))
            } else {
                self.fixtures
                    .conversation_label(conversation)
                    .to_lowercase()
                    .contains(&self.find_query)
            };
            if !listed {
                continue;
            }
            let kind = if unread.count > 0 {
                ListChild::Unread(unread.mentions > 0)
            } else {
                ListChild::Plain
            };
            children.push((
                v_flex()
                    .px_2()
                    .child(self.conversation_row(conversation, watermark, cx))
                    .into_any_element(),
                kind,
            ));
        }
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

    /// The sections under the nav rows. Live: the user's own sidebar sections in their order,
    /// the built-in Channels and Direct messages buckets holding every conversation no other
    /// section claims, as in Slack. Sample workspace: plain Channels and Direct messages.
    fn section_list(
        &self,
        channels: &[Conversation],
        watermark: Option<&Ts>,
        cx: &mut Context<Self>,
    ) -> Vec<(AnyElement, ListChild)> {
        let sections: Vec<ChannelSection> = self
            .fixtures
            .sections
            .iter()
            .chain(&self.local_sections)
            .cloned()
            .collect();
        if sections.is_empty() {
            let mut children = self.section(
                "channels".into(),
                Some(caret()),
                "Channels",
                channels,
                watermark,
                cx,
            );
            children.extend(self.section(
                "dms".into(),
                Some(caret()),
                "Direct messages",
                &self.fixtures.dms,
                watermark,
                cx,
            ));
            return children;
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
        let mut list = Vec::new();
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
            list.extend(self.section(
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

    /// The rail's Files destination, Slack's Files list column: the "Files" header over one
    /// row per page of the Files view (All files, Canvases, Lists), the shown one selected.
    /// Slack's Deleted, Recently viewed and Starred rows are left out: their data sits behind
    /// private methods slackless does not call (`files.recentlyDeleted`,
    /// `files.favorites.list`) or no mapped method at all.
    fn files_list(&self, cx: &mut Context<Self>) -> Div {
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
    fn files_row(&self, section: FilesSection, cx: &mut Context<Self>) -> Stateful<Div> {
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

    /// The rail's DMs destination, Slack's DM list: the collapsible "Direct messages" header
    /// with the "Unreads" filter toggle and the compose pen, a "Find a DM…" field, then one
    /// row per DM, latest activity first — presence or stacked group avatar, bold name with
    /// its relative stamp, and a muted preview of the latest message ([`Fixtures::latest`] in
    /// live mode, the history in fixtures).
    fn dm_list(&self, watermark: Option<&Ts>, cx: &mut Context<Self>) -> Div {
        let query = self.dm_find_query.as_str();
        let mut dms: Vec<(&Conversation, Option<&Message>)> = self
            .fixtures
            .dms
            .iter()
            // Slack lists apps' and Slackbot's DMs under Apps, not here.
            .filter(|dm| {
                !dm.user.as_ref().is_some_and(|user| {
                    matches!(user.as_str(), "USLACKBOT" | "USLACK")
                        || self.fixtures.user(user).is_some_and(|u| u.is_bot)
                })
            })
            .map(|dm| (dm, self.dm_latest(dm)))
            .filter(|&(dm, _)| {
                query.is_empty()
                    || self
                        .fixtures
                        .conversation_label(dm)
                        .to_lowercase()
                        .contains(query)
            })
            .filter(|&(dm, _)| !self.dm_unreads_only || self.unread_in(dm, watermark).count > 0)
            .collect();
        dms.sort_by(|a, b| {
            b.1.map(|message| &message.ts)
                .cmp(&a.1.map(|message| &message.ts))
        });
        let border = cx.theme().sidebar_border;
        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, (dm, last)) in dms.into_iter().enumerate() {
            // Slack separates the rows with hairlines, not gaps.
            if index > 0 {
                rows.push(div().h(px(1.)).flex_none().bg(border).into_any_element());
            }
            rows.push(self.dm_row(dm, last, watermark, cx).into_any_element());
        }
        let collapsed = SharedString::from("dm-list");
        v_flex()
            .child(self.dm_header(cx))
            .when(!self.collapsed.contains(&collapsed), |list| {
                list.child(self.find_field(&self.dm_find, cx))
                    .child(v_flex().pt_1().pb_2().children(rows))
            })
    }

    /// The DM's latest top-level message: the freshest history the views hold (fixture mode,
    /// or live mode's open conversation), else the snapshot's per-DM latest message.
    fn dm_latest(&self, dm: &Conversation) -> Option<&Message> {
        if let Some(fresh) = self.fresh_latest.get(&dm.id) {
            return fresh.as_ref();
        }
        self.fixtures
            .messages
            .get(&dm.id)
            .and_then(|history| history.last())
            .or_else(|| self.fixtures.latest.get(&dm.id))
    }

    /// The DM list's header: the collapsible "Direct messages" title, Slack's "Unreads"
    /// filter toggle and the compose pen on the right.
    fn dm_header(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let dim = theme.sidebar_foreground.opacity(0.6);
        let hover = theme.accent;
        let border = theme.sidebar_border;
        let toggle = cx.weak_entity();
        let on_unreads = move |checked: &bool, _: &mut Window, cx: &mut gpui_kit::gpui::App| {
            toggle
                .update(cx, |sidebar: &mut Sidebar, cx| {
                    sidebar.dm_unreads_only = *checked;
                    cx.notify();
                })
                .ok();
        };
        h_flex()
            .px_4()
            .pt_3()
            .pb_2()
            .gap_2()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .id("dm-list-title")
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .items_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|title| title.bg(hover))
                    // Slack's title reads "Direct messages ⌄": the chevron follows it.
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .child("Direct messages"),
                    )
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .text_color(dim)
                            .size(px(14.)),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        let key = SharedString::from("dm-list");
                        if !this.collapsed.remove(&key) {
                            this.collapsed.insert(key);
                        }
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_color(dim)
                    .child("Unreads")
                    .child(
                        Switch::new("dm-unreads")
                            .accessibility_label("Unreads")
                            .with_size(Size::XSmall)
                            .checked(self.dm_unreads_only)
                            .on_click(on_unreads),
                    ),
            )
            .child(
                self.header_icon("dm-compose", IconName::SquarePen, dim, hover)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenCompose))),
            )
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
        let (fg, dim, preview_fg) = (
            theme.sidebar_foreground,
            theme.sidebar_foreground.opacity(0.6),
            theme.sidebar_foreground.opacity(0.9),
        );
        // Slack's DM list marks the open DM with a gray wash and keeps its text colors.
        let selected_bg = theme.sidebar.blend(theme.sidebar_foreground.opacity(0.1));
        let ring = if selected { selected_bg } else { theme.sidebar };
        let hover = theme.accent;
        // The self-DM reads "Name (you)" here, "(you)" in regular weight, as Slack's DM list
        // shows it; every other surface keeps the plain label.
        let label = self.fixtures.conversation_label(dm);
        let is_self = dm.is_im
            && dm
                .user
                .as_ref()
                .is_some_and(|user| *user == self.fixtures.self_user);
        let preview = last.map(|message| {
            // Slack prefixes only your own messages, in group DMs too.
            let author = match &message.user {
                Some(user) if *user == self.fixtures.self_user => "You: ",
                _ => "",
            };
            // Formatting, links and emoji render as in the transcript, as Slack's list does.
            crate::messages::mrkdwn_preview(
                &format!("{author}{}", self.fixtures.preview(message)),
                &self.fixtures,
                dim,
                format!("dm-preview-{}", dm.id),
                cx,
            )
        });
        let stamp = last
            .map(|message| list_stamp(&message.ts))
            .unwrap_or_default();
        let id = dm.id.clone();
        h_flex()
            .id(SharedString::from(format!("dm-{id}")))
            .px_2()
            .py_3()
            .gap_2()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            // Like Slack, the selected row keeps its colour under the pointer.
            .when(selected, |row| row.bg(selected_bg))
            .when(!selected, |row| row.hover(|row| row.bg(hover)))
            .child(conversation_avatar(&self.fixtures, dm, 36., ring, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .text_color(fg)
                                    .child(
                                        div().truncate().font_weight(FontWeight::BOLD).child(label),
                                    )
                                    .when(is_self, |name| {
                                        name.child(div().flex_none().child("(you)"))
                                    }),
                            )
                            .child(div().flex_none().text_xs().text_color(dim).child(stamp)),
                    )
                    // Slack's preview is as large as the name and only slightly dimmer.
                    .child(div().line_clamp(2).text_color(preview_fg).children(preview)),
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
            .when(!selected, |row| row.hover(|row| row.bg(theme.accent)))
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
        if focus && unread.count > 0 {
            row = row.child(focus_dot(cx));
        } else if unread.mentions > 0 {
            row = row.child(count_pill(unread.mentions, true, cx));
        }
        row.on_click(cx.listener(move |this, _, _, cx| this.select_row(id.clone(), cx)))
    }

    /// Slack's floating "More unreads" chip for one edge of the list: it points at hidden
    /// unread rows, turns into the red "Unread mentions" chip while any of them carries
    /// mentions, and otherwise reads as a neutral raised chip.
    fn unreads_pill(&self, target: HiddenUnreads, cx: &Context<Self>) -> Stateful<Div> {
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
        let (row, below) = (target.row, target.below);
        h_flex()
            .id(if target.below {
                "unreads-below"
            } else {
                "unreads-above"
            })
            .gap_1()
            .px_2()
            .py_0p5()
            .rounded_full()
            .border_1()
            .border_color(theme.sidebar_border)
            .bg(bg)
            .hover(|pill| pill.bg(hover))
            .shadow_sm()
            .text_sm()
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
                this.reveal_unread(row, below);
                cx.notify();
            }))
    }

    /// Scroll `row` into view a pill's height inside the edge its pill floats on, so the
    /// revealed unread row is not hidden under the pill that is still shown for later ones.
    fn reveal_unread(&self, row: usize, below: bool) {
        let Some(bounds) = self.scroll.bounds_for_item(row) else {
            self.scroll.scroll_to_item(row);
            return;
        };
        let viewport = self.scroll.bounds();
        let clearance = px(PILL_CLEARANCE);
        // Child bounds are unscrolled: on screen a row sits at its bounds plus the offset.
        let y = if below {
            viewport.bottom() - clearance - bounds.bottom()
        } else {
            viewport.top() + clearance - bounds.top()
        };
        let max = self.scroll.max_offset().y;
        let y = if y > px(0.) {
            px(0.)
        } else if y < -max {
            -max
        } else {
            y
        };
        self.scroll.set_offset(point(self.scroll.offset().x, y));
    }
}

/// Focus mode's subtle unread marker, in place of bolding and count pills.
fn focus_dot(cx: &gpui_kit::gpui::App) -> Div {
    div()
        .flex_none()
        .size(px(6.))
        .rounded_full()
        .bg(cx.theme().sidebar_foreground.opacity(0.6))
}

/// Slack-style inline count pill: red for mentions (and DM messages), the sidebar's primary
/// emphasis colors otherwise.
fn count_pill(count: u32, mentions: bool, cx: &gpui_kit::gpui::App) -> Div {
    let theme = cx.theme();
    let (bg, fg) = if mentions {
        crate::theme::mention_badge(cx)
    } else {
        (theme.sidebar_primary, theme.sidebar_primary_foreground)
    };
    // Slack's pill is wider than tall: 18px high, at least 25px wide.
    h_flex()
        .flex_none()
        .justify_center()
        .items_center()
        .gap_0p5()
        .h(px(18.))
        .min_w(px(25.))
        .px_2()
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .child(count.to_string())
}

/// One flattened child of the sidebar's scroll list: unread conversation rows are the ones
/// the "More unreads" pills track.
enum ListChild {
    /// An unread conversation row; the flag says it carries mentions.
    Unread(bool),
    /// Headers, read rows and dividers.
    Plain,
}

/// Room kept between a revealed unread row and the list edge: the pill's height plus its inset.
const PILL_CLEARANCE: f32 = 44.;

/// One edge of the list's hidden unreads: the unread row the pill scrolls to next, and
/// whether any unread row hidden past that edge carries mentions. `below` picks the pill's
/// arrow and the list edge it rides.
#[derive(Clone, Copy)]

struct HiddenUnreads {
    row: usize,
    mentions: bool,
    below: bool,
}

/// The "More unreads" pills for `unread_rows` (child indices with mention flags, in list
/// order): the first unread row below the fold and the last one scrolled above it. Rows are
/// placed with the scroll handle's recorded child bounds; `None` on an edge whose unread
/// rows are in view (or before the list has laid out).
fn hidden_unreads(
    scroll: &ScrollHandle,
    unread_rows: &[(usize, bool)],
) -> (Option<HiddenUnreads>, Option<HiddenUnreads>) {
    let viewport = scroll.bounds();
    // Child bounds live in the layout's unscrolled space; scrolling down drives the offset
    // negative (`ScrollHandle::set_offset`), so subtracting it maps the viewport's edges
    // into that space.
    let top = viewport.top() - scroll.offset().y;
    let bottom = viewport.bottom() - scroll.offset().y;
    let mut below = Option::<HiddenUnreads>::None;
    let mut above = Option::<HiddenUnreads>::None;
    for &(row, mentions) in unread_rows {
        if let Some(bounds) = scroll.bounds_for_item(row) {
            if bounds.bottom() > bottom {
                let pill = below.get_or_insert(HiddenUnreads {
                    row,
                    mentions: false,
                    below: true,
                });
                pill.mentions |= mentions;
            } else if bounds.top() < top {
                above = Some(HiddenUnreads {
                    row,
                    mentions: above.is_some_and(|pill| pill.mentions) || mentions,
                    below: false,
                });
            }
        }
    }
    (below, above)
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
        let dark = cx.theme().mode.is_dark();
        let focus = self.self_state.focused();
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
            .track_scroll(&self.scroll)
            .on_scroll(cx.listener(|_, _, _, cx| cx.notify()))
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
                        "unreads",
                        IconName::Inbox,
                        "Unreads",
                        total.count > 0 && !focus,
                        None,
                        cx,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenActivity))),
                    // Slack bolds Threads while followed threads have unread replies and
                    // counts the ones mentioning you in a red pill.
                    self.nav_row(
                        "threads",
                        IconName::MessageSquareText,
                        "Threads",
                        self.threads_unread.0 && !focus,
                        None,
                        cx,
                    )
                    .when(self.threads_unread.1 > 0 && !focus, |row| {
                        row.child(count_pill(self.threads_unread.1, true, cx))
                    })
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenThreads))),
                    self.nav_row("huddles", IconName::Headphones, "Huddles", false, None, cx)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenHuddles))),
                    self.nav_row(
                        "drafts",
                        IconName::Send,
                        "Drafts & sent",
                        false,
                        (self.draft_count > 0).then(|| self.draft_count.to_string()),
                        cx,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenDrafts))),
                    self.nav_row(
                        "saved",
                        IconName::Bookmark,
                        "Saved",
                        false,
                        (self.later_count > 0).then(|| self.later_count.to_string()),
                        cx,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenLater))),
                    self.nav_row(
                        "directories",
                        IconName::BookUser,
                        "Directories",
                        false,
                        None,
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
                for (child, kind) in self.section_list(&channels, watermark.as_ref(), cx) {
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
                list.children(children)
            }
        };

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
                    // Slack divides it from the conversation with a hairline; light Slack lets
                    // the conversation's shadow do it.
                    div()
                        .relative()
                        .flex_1()
                        .min_w(px(0.))
                        .h_full()
                        .bg(sidebar_bg)
                        .when(dark, |list| list.border_r_1().border_color(sidebar_border))
                        .child(list)
                        // Light Slack's conversation casts its shadow over the sidebar's edge.
                        .when(!dark, |list| {
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
                sidebar
                    .child(
                        div()
                            .id("workspace-menu-scrim")
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
                                cx,
                            )),
                    )
            })
    }
}
