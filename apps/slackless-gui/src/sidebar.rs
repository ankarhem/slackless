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

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, Size, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, AnyElement, AppContext as _, Context, DismissEvent, Div, Edges, Entity, EventEmitter,
    Focusable as _, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, PathBuilder, Pixels, Point, Render, ScrollHandle, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, anchored, canvas,
    deferred, div, img, point, px, relative, transparent_black,
};
use slackless_core::NotifyLevel;
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
use crate::sidebar_menu::{
    ChannelDrag, ChannelMenuInput, Hidden, HiddenConversations, MenuRow, SectionFilter,
    SectionFormEvent, SectionMenuInput, SectionNameForm, SectionSort, channel_menu,
    more_conversations_menu, section_menu, workspace_menu,
};
use crate::status::{FOCUS_DURATIONS, SelfState, self_avatar};

/// Slack's workspace switcher column, left of the tab rail.
pub(crate) const SWITCHER_WIDTH: f32 = 60.;
/// Slack's tab rail.
const TAB_RAIL_WIDTH: f32 = 70.;

pub enum SidebarEvent {
    SelectConversation(ChannelId),
    /// The "Add channels" row: open the channel browser.
    BrowseChannels,
    /// Create a new public or private channel (`conversations.create`).
    CreateChannel,
    /// A workspace tile in the rail was clicked (index into `Fixtures::workspaces`).
    SwitchWorkspace(usize),
    /// Reorder a workspace tile in the rail (`F-CORE-003`).
    ReorderWorkspace {
        from: usize,
        to: usize,
    },
    /// Open `channel` in a separate native GPUI window (`F-CORE-026`).
    OpenInNewWindow(ChannelId),
    /// Per-section sort or filter changed; persist to SQLite `local_state`.
    SectionPrefsChanged,
    /// The "Drafts & sent" row.
    OpenDrafts,
    /// The "Saved" row.
    OpenLater,
    /// The "Threads" row: open the Threads view.
    OpenThreads,
    /// The Home list's "Unreads" row: open the All Unreads view (Slack's `p-unreads_view`).
    OpenUnreads,
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
    /// The row menu's "+ Create new section": the dialog confirmed `name`, and `channel`
    /// should move into the fresh section.
    CreateSectionWithChannel {
        name: String,
        channel: ChannelId,
    },
    /// Rename a custom section (`users.channelSections.set`).
    RenameSection {
        section: String,
        name: String,
    },
    /// Delete a custom section (`users.channelSections.delete`), after its confirm dialog.
    DeleteSection {
        section: String,
    },
    /// Move a conversation between sections (`users.channelSections.channels.bulkUpdate`);
    /// `from` / `to` name custom sections, `None` being the default bucket on either side.
    MoveChannel {
        channel: ChannelId,
        from: Option<String>,
        to: Option<String>,
    },
    /// A section menu's "Mark all as read": move `channel`'s cursor to `ts` (the root's
    /// existing `Command::MarkRead` path).
    MarkRead {
        channel: ChannelId,
        ts: Ts,
    },
    /// A row menu or "Mute all conversations" choice: the existing
    /// `Command::SetNotifyLevel` path.
    SetNotifyLevel {
        channel: ChannelId,
        level: NotifyLevel,
    },
    /// A row menu's "Channel details": open the existing details dialog.
    OpenChannelDetails(ChannelId),
    /// A row menu's "Leave channel": the existing `Command::LeaveConversation` path.
    LeaveChannel(ChannelId),
    /// The rail's moon: turn focus mode on or off.
    ToggleFocus,
    /// The focus card's Duration choice (an index into `status::FOCUS_DURATIONS`).
    FocusDuration(usize),
    /// The rail's own avatar: open the user menu.
    OpenUserMenu,
    /// The rail's Activity tile: open the Activity page, which the root renders beside the
    /// rail instead of this sidebar's list column.
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
            NavItem::Todos => IconName::CircleCheck,
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

/// Navigation rail visibility and label style driven by Preferences -> Navigation (`F-SHELL-062`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NavPrefs {
    pub show_home: bool,
    pub show_dms: bool,
    pub show_activity: bool,
    pub show_files: bool,
    pub show_later: bool,
    pub icons_only: bool,
}

impl Default for NavPrefs {
    fn default() -> Self {
        Self {
            show_home: true,
            show_dms: true,
            show_activity: true,
            show_files: true,
            show_later: true,
            icons_only: false,
        }
    }
}

impl NavPrefs {
    fn shows(self, item: NavItem) -> bool {
        match item {
            NavItem::Home => self.show_home,
            NavItem::Dms => self.show_dms,
            NavItem::Activity => self.show_activity,
            NavItem::Files => self.show_files,
            NavItem::Todos => self.show_later,
            NavItem::Agents | NavItem::Admin => true,
        }
    }
}

/// Home sidebar display preferences driven by Preferences -> Home (`F-CORE-024`, `F-SHELL-066`, `F-SHELL-072`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HomePrefs {
    pub show: usize,
    pub sort: usize,
    pub unreads_section: bool,
    pub separate_private: bool,
    pub profile_photos: bool,
    pub compact: bool,
}

impl Default for HomePrefs {
    fn default() -> Self {
        Self {
            show: 0,
            sort: 0,
            unreads_section: false,
            separate_private: false,
            profile_photos: true,
            compact: false,
        }
    }
}

/// Unread/mention badge summary for one tile in the workspace switcher rail (`F-CORE-006`).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct WorkspaceBadge {
    pub mentions: u32,
    pub has_unreads: bool,
}

/// Unread state of one conversation row, derived from the fixture history: messages the
/// signed-in user did not author that are newer than the newest message in the open
/// conversation (the fixture-mode "read" watermark). `mentions` is the subset of `count`
/// that @-mentions the signed-in user.
#[derive(Clone, Copy, Default)]
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
    /// Conversations with a live huddle; their rows carry the headphones indicator.
    huddles: HashSet<ChannelId>,
    /// Saved drafts, shown on the "Drafts & sent" row.
    draft_count: usize,
    /// In-progress Later items, shown on the "Later" row.
    later_count: usize,
    /// The gear menu is open (a scrim covers the sidebar list under it).
    settings_menu_open: bool,
    /// Slack's "Active only" sidebar filter (Slack's default): hide dormant conversations.
    active_only: bool,
    /// Per-section list filters and sorts (Slack keeps them per section too, in its
    /// `channel_sections` pref); a section without an entry follows [`Self::active_only`]
    /// and the built-in sort. Session-local, like Slack's until it syncs them.
    section_filters: HashMap<SharedString, SectionFilter>,
    section_sorts: HashMap<SharedString, SectionSort>,
    /// Fixture mode's section overlay: sections renamed, moved into, created or deleted
    /// locally this session, seeded from the fixtures on first change (`None`: pristine).
    /// Live sections arrive via the store and never touch this.
    local_sections: Option<Vec<ChannelSection>>,
    /// Fixture mode's notification-level overrides (a live write lands through the store's
    /// prefs on the next snapshot).
    notify_override: HashMap<ChannelId, NotifyLevel>,
    /// The Home list's conversation rows as last laid out, top to bottom, each with whether
    /// it is unread: what Opt+Up/Down and Opt+Shift+Up/Down step through ([`Self::step`]).
    rows: RefCell<Vec<(ChannelId, bool)>>,
    /// The open "More conversations" popover.
    section_search: Option<SectionSearch>,
    /// Whether the press on a "More conversations" button found its popover open: the press
    /// closes it, so the click that follows must not open it again.
    search_was_open: bool,
    /// What the "Create a channel section" dialog builds when it confirms: a plain
    /// section, a rename of `id`, or a new section that `channel` moves into.
    section_form_mode: SectionFormMode,
    /// The "Create a channel section" dialog body.
    section_form: Entity<SectionNameForm>,
    /// Tracks the sidebar list's scroll state; the "More unreads" pills read which unread
    /// rows the scroll offset carries out of view.
    scroll: ScrollHandle,
    /// The "More unreads" scroll in flight, eased one frame per render.
    scroll_anim: Option<ScrollAnim>,
    /// The Slack Connect row whose organizations tooltip is showing, and the row whose
    /// tooltip waits out the hover delay (dropping the task cancels it).
    org_tip: Option<ChannelId>,
    org_tip_delay: Option<(ChannelId, Task<()>)>,
    /// Unread Activity rows (the root pushes the live feed's count; fixture mode derives
    /// it), for the rail tile and the dock badge.
    activity_unread: u32,
    /// Per-workspace unread / mention badges shown on the workspace switcher rail tiles.
    workspace_badges: Vec<WorkspaceBadge>,
    /// Navigation rail visibility and label style from Preferences -> Navigation.
    nav_prefs: NavPrefs,
    /// Home sidebar filter, sort, photo, and row-density preferences from Preferences -> Home.
    home_prefs: HomePrefs,
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
            let find = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 1));
            let dm_find = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 1));
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
                    |sidebar: &mut Sidebar, _, event: &SectionFormEvent, window, cx| {
                        let SectionFormEvent::Create(name) = event;
                        window.close_dialog(cx);
                        match std::mem::replace(
                            &mut sidebar.section_form_mode,
                            SectionFormMode::Create,
                        ) {
                            SectionFormMode::Create => {
                                cx.emit(SidebarEvent::CreateSection { name: name.clone() });
                            }
                            SectionFormMode::Rename(section) => {
                                cx.emit(SidebarEvent::RenameSection {
                                    section,
                                    name: name.clone(),
                                });
                            }
                            SectionFormMode::CreateWithChannel(channel) => {
                                cx.emit(SidebarEvent::CreateSectionWithChannel {
                                    name: name.clone(),
                                    channel,
                                });
                            }
                        }
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
                huddles: HashSet::new(),
                draft_count: 0,
                later_count: 0,
                settings_menu_open: false,
                active_only: true,
                section_filters: HashMap::new(),
                section_sorts: HashMap::new(),
                local_sections: None,
                notify_override: HashMap::new(),
                rows: RefCell::default(),

                section_search: None,
                search_was_open: false,
                section_form_mode: SectionFormMode::Create,
                section_form,
                scroll: ScrollHandle::new(),
                scroll_anim: None,
                org_tip: None,
                org_tip_delay: None,
                activity_unread: 0,
                workspace_badges: Vec::new(),
                nav_prefs: NavPrefs::default(),
                home_prefs: HomePrefs::default(),
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

    /// Whether any channel or DM in this workspace currently has unread messages.
    pub fn has_any_unreads(&self) -> bool {
        let watermark = self.read_watermark();
        self.activity_unread > 0
            || self
                .total_unread(&self.fixtures.channels, watermark.as_ref())
                .count
                > 0
            || self
                .total_unread(&self.fixtures.dms, watermark.as_ref())
                .count
                > 0
            || self.total_unread(&self.joined, watermark.as_ref()).count > 0
    }

    /// Update per-workspace unread/mention badges on the workspace switcher rail.
    pub fn set_workspace_badges(&mut self, badges: Vec<WorkspaceBadge>, cx: &mut Context<Self>) {
        if self.workspace_badges != badges {
            self.workspace_badges = badges;
            cx.notify();
        }
    }

    /// Apply Preferences -> Navigation visibility and label style to the left nav rail.
    pub fn set_nav_prefs(&mut self, prefs: NavPrefs, cx: &mut Context<Self>) {
        if self.nav_prefs != prefs {
            self.nav_prefs = prefs;
            cx.notify();
        }
    }

    /// Apply Preferences -> Home display, density, and sorting preferences.
    pub fn set_home_prefs(&mut self, prefs: HomePrefs, cx: &mut Context<Self>) {
        if self.home_prefs != prefs {
            self.home_prefs = prefs;
            cx.notify();
        }
    }

    /// Cycle through visible navigation rail tabs (`Ctrl+Tab` / `Ctrl+Shift+Tab`).
    pub fn cycle_nav(&mut self, forward: bool, cx: &mut Context<Self>) {
        let visible: Vec<NavItem> = [
            NavItem::Home,
            NavItem::Dms,
            NavItem::Activity,
            NavItem::Files,
            NavItem::Todos,
        ]
        .into_iter()
        .filter(|&item| self.nav_prefs.shows(item))
        .collect();
        if visible.len() < 2 {
            return;
        }
        let pos = visible
            .iter()
            .position(|&item| item == self.active_nav)
            .unwrap_or(0);
        let next = if forward {
            (pos + 1) % visible.len()
        } else {
            (pos + visible.len() - 1) % visible.len()
        };
        let target = visible[next];
        self.show_nav(target, cx);
        match target {
            NavItem::Activity => cx.emit(SidebarEvent::OpenActivity),
            NavItem::Files => cx.emit(SidebarEvent::OpenFiles),
            NavItem::Todos => cx.emit(SidebarEvent::OpenLater),
            _ => {}
        }
    }

    /// Switch the sidebar back to the Home navigation tab (`Cmd+Shift+H`).
    pub fn show_home(&mut self, cx: &mut Context<Self>) {
        self.show_nav(NavItem::Home, cx);
    }

    /// Mark `channel` as having a live huddle (`None`: no huddle anywhere).
    pub fn set_huddle(&mut self, channel: Option<ChannelId>, cx: &mut Context<Self>) {
        self.huddles.clear();
        if let Some(channel) = channel {
            self.huddles.insert(channel);
        }
        cx.notify();
    }

    /// Replace the set of conversations currently running a live huddle.
    pub fn set_huddles(&mut self, huddles: HashSet<ChannelId>, cx: &mut Context<Self>) {
        if self.huddles != huddles {
            self.huddles = huddles;
            cx.notify();
        }
    }

    /// List a channel joined from the browser under Channels.
    pub fn add_channel(&mut self, conversation: Conversation, cx: &mut Context<Self>) {
        if !self.joined.iter().any(|c| c.id == conversation.id) {
            self.joined.push(conversation);
        }
        cx.notify();
    }

    /// The sidebar's sections: the fixture-mode overlay once anything changed locally,
    /// else the snapshot's.
    fn section_set(&self) -> Vec<ChannelSection> {
        self.local_sections
            .clone()
            .unwrap_or_else(|| self.fixtures.sections.clone())
    }

    /// List a section created locally (fixture mode) after the stored ones.
    pub fn add_section(&mut self, name: String, cx: &mut Context<Self>) {
        let index = self.section_set().len() + 1;
        let mut sections = self.section_set();
        sections.push(ChannelSection {
            id: format!("LLOCAL{index}"),
            name,
            kind: SectionKind::Standard,
            emoji: String::new(),
            next_id: None,
            channel_ids: Vec::new(),
        });
        self.local_sections = Some(sections);
        cx.notify();
    }

    /// Fixture mode's row-menu "+ Create new section": create the section and move the
    /// channel into it in one step, as Slack does.
    pub fn add_section_with_channel(
        &mut self,
        name: String,
        channel: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let index = self.section_set().len() + 1;
        let id = format!("LLOCAL{index}");
        let mut sections = self.section_set();
        sections.push(ChannelSection {
            id: id.clone(),
            name,
            kind: SectionKind::Standard,
            emoji: String::new(),
            next_id: None,
            channel_ids: vec![channel.clone()],
        });
        for section in sections.iter_mut() {
            if section.kind == SectionKind::Standard && section.id != id {
                section.channel_ids.retain(|other| *other != channel);
            }
        }
        self.local_sections = Some(sections);
        cx.notify();
    }

    /// Fixture mode's rename of a section (live mode's goes through
    /// [`SidebarEvent::RenameSection`]).
    pub fn rename_section_local(&mut self, section: &str, name: &str, cx: &mut Context<Self>) {
        let mut sections = self.section_set();
        if let Some(target) = sections.iter_mut().find(|known| known.id == section) {
            target.name = name.to_owned();
            self.local_sections = Some(sections);
            cx.notify();
        }
    }

    /// Fixture mode's section delete; its conversations fall back to their default buckets.
    pub fn delete_section_local(&mut self, section: &str, cx: &mut Context<Self>) {
        let mut sections = self.section_set();
        let before = sections.len();
        sections.retain(|known| known.id != section);
        if sections.len() != before {
            self.local_sections = Some(sections);
            cx.notify();
        }
    }

    /// A conversation moved between sections: fixture mode edits the overlay; live mode
    /// asks the root for the `users.channelSections.channels.bulkUpdate` round.
    pub(crate) fn move_channel(
        &mut self,
        channel: ChannelId,
        from: Option<String>,
        to: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if from == to {
            return;
        }
        if self.fixtures.sample {
            let mut sections = self.section_set();
            for section in sections.iter_mut() {
                if section.kind == SectionKind::Standard {
                    section.channel_ids.retain(|id| *id != channel);
                }
            }
            if let Some(target) = to.as_ref().and_then(|to| {
                sections
                    .iter_mut()
                    .find(|section| section.id == *to && section.kind == SectionKind::Standard)
            }) {
                target.channel_ids.push(channel);
            }
            self.local_sections = Some(sections);
            cx.notify();
            return;
        }
        cx.emit(SidebarEvent::MoveChannel { channel, from, to });
    }

    /// One notification-level choice: fixture mode keeps it locally (the snapshot carries
    /// live ones); live mode asks the root for the `Command::SetNotifyLevel` path.
    pub(crate) fn set_notify(
        &mut self,
        channel: ChannelId,
        level: NotifyLevel,
        cx: &mut Context<Self>,
    ) {
        if self.fixtures.sample {
            self.notify_override.insert(channel, level);
            cx.notify();
            return;
        }
        cx.emit(SidebarEvent::SetNotifyLevel { channel, level });
    }

    /// A conversation's effective notification level: this session's fixture override, or
    /// the snapshot's.
    fn notify_level(&self, channel: &ChannelId) -> Option<NotifyLevel> {
        self.notify_override
            .get(channel)
            .or_else(|| self.fixtures.notify_levels.get(channel))
            .copied()
    }

    /// "Mute all conversations": one choice per unmuted conversation of the section.
    pub(crate) fn mute_section(&mut self, channels: &[ChannelId], cx: &mut Context<Self>) {
        for channel in channels {
            if self.notify_level(channel) != Some(NotifyLevel::Muted) {
                self.set_notify(channel.clone(), NotifyLevel::Muted, cx);
            }
        }
    }

    /// "Mark all as read": every conversation with unread messages in the section, through
    /// the root's existing read path (and, in fixture mode, the local read marks the
    /// snapshot cannot carry).
    pub(crate) fn mark_section_read(&mut self, channels: &[ChannelId], cx: &mut Context<Self>) {
        let watermark = self.read_watermark();
        for channel in channels {
            let Some(conversation) = self
                .fixtures
                .channels
                .iter()
                .chain(&self.fixtures.dms)
                .find(|conversation| conversation.id == *channel)
            else {
                continue;
            };
            if self.unread_in(conversation, watermark.as_ref()).count == 0 {
                continue;
            }
            let latest =
                self.fixtures
                    .counts
                    .get(channel)
                    .and_then(|counts| counts.latest.clone())
                    .or_else(|| {
                        self.fixtures.messages.get(channel).and_then(|history| {
                            history.iter().map(|message| message.ts.clone()).max()
                        })
                    })
                    .or_else(|| {
                        self.fixtures
                            .latest
                            .get(channel)
                            .map(|message| message.ts.clone())
                    });
            if let Some(latest) = latest {
                self.read.insert(channel.clone());
                cx.emit(SidebarEvent::MarkRead {
                    channel: channel.clone(),
                    ts: latest,
                });
            }
        }
        cx.notify();
    }

    /// The section's effective filter: its own choice, or the workspace's "Active only"
    /// default (`All` while the gear menu's toggle is off).
    fn section_filter(&self, key: &SharedString) -> SectionFilter {
        self.section_filters.get(key).copied().unwrap_or({
            if self.active_only {
                SectionFilter::ActiveOnly
            } else {
                SectionFilter::All
            }
        })
    }

    pub(crate) fn set_section_filter(
        &mut self,
        key: SharedString,
        filter: SectionFilter,
        cx: &mut Context<Self>,
    ) {
        self.section_filters.insert(key, filter);
        cx.emit(SidebarEvent::SectionPrefsChanged);
        cx.notify();
    }

    /// The section's effective sort: its own choice, or the built-in default (the DMs
    /// bucket by activity, everything else by name).
    fn section_sort(&self, key: &SharedString, dm: bool) -> SectionSort {
        self.section_sorts.get(key).copied().unwrap_or(if dm {
            SectionSort::Recency
        } else {
            SectionSort::Alpha
        })
    }

    pub(crate) fn set_section_sort(
        &mut self,
        key: SharedString,
        sort: SectionSort,
        cx: &mut Context<Self>,
    ) {
        self.section_sorts.insert(key, sort);
        cx.emit(SidebarEvent::SectionPrefsChanged);
        cx.notify();
    }

    /// Snapshot per-section filter & sort choices for SQLite `local_state` persistence.
    pub(crate) fn export_section_prefs(&self) -> crate::bridge::SectionPrefs {
        crate::bridge::SectionPrefs {
            filters: self
                .section_filters
                .iter()
                .map(|(k, &v)| (k.to_string(), v))
                .collect(),
            sorts: self
                .section_sorts
                .iter()
                .map(|(k, &v)| (k.to_string(), v))
                .collect(),
        }
    }

    /// Restore per-section filter & sort choices loaded from SQLite `local_state`.
    pub(crate) fn restore_section_prefs(
        &mut self,
        prefs: crate::bridge::SectionPrefs,
        cx: &mut Context<Self>,
    ) {
        self.section_filters = prefs
            .filters
            .into_iter()
            .map(|(k, v)| (SharedString::from(k), v))
            .collect();
        self.section_sorts = prefs
            .sorts
            .into_iter()
            .map(|(k, v)| (SharedString::from(k), v))
            .collect();
        cx.notify();
    }

    /// "Search in this section": the find field filters the list, so it takes the focus.
    pub(crate) fn focus_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find
            .update(cx, |state, cx| state.focus_handle(cx).focus(window, cx));
    }

    /// The row menu's "Channel details": the root owns the dialog.
    pub(crate) fn open_details(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        cx.emit(SidebarEvent::OpenChannelDetails(channel));
    }

    /// The row menu's "Leave channel": the root's existing leave path.
    pub(crate) fn leave(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        cx.emit(SidebarEvent::LeaveChannel(channel));
    }

    pub(crate) fn open_in_new_window(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        cx.emit(SidebarEvent::OpenInNewWindow(channel));
    }

    /// The rows a section's menus and popover list: this menu's labels and glyphs.
    pub(crate) fn menu_rows(&self, ids: &[ChannelId]) -> Vec<MenuRow> {
        let watermark = self.read_watermark();
        ids.iter()
            .filter_map(|id| {
                let conversation = self.fixtures.conversation(id)?;
                let label = self.fixtures.conversation_label(conversation).to_owned();
                Some(MenuRow {
                    id: id.clone(),
                    label,
                    private: matches!(conversation.kind(), ConversationKind::PrivateChannel),
                    is_dm: matches!(
                        conversation.kind(),
                        ConversationKind::Im | ConversationKind::Mpim
                    ),
                    unread: self.unread_in(conversation, watermark.as_ref()).count > 0,
                })
            })
            .collect()
    }

    /// A DM menu row's avatar, cut out of the menu's surface.
    pub(crate) fn menu_avatar(&self, id: &ChannelId, cx: &gpui_kit::gpui::App) -> Option<Div> {
        let conversation = self.fixtures.conversation(id)?;
        Some(conversation_avatar(
            &self.fixtures,
            conversation,
            20.,
            cx.theme().popover,
            cx,
        ))
    }

    /// One section menu's whole input, gathered as the menu opens (never per frame): the
    /// section's channels, the ones its rows leave out, and the mark-read / mute targets.
    fn section_menu_input(
        &self,
        key: SharedString,
        section: Option<ChannelSection>,
        title: String,
        ids: &[ChannelId],
    ) -> SectionMenuInput {
        let watermark = self.read_watermark();
        let (mut unread, mut unmuted) = (Vec::new(), Vec::new());
        for id in ids {
            let Some(conversation) = self.fixtures.conversation(id) else {
                continue;
            };
            if self.unread_in(conversation, watermark.as_ref()).count > 0 {
                unread.push(id.clone());
            }
            if self.notify_level(id) != Some(NotifyLevel::Muted) {
                unmuted.push(id.clone());
            }
        }
        let dm = section
            .as_ref()
            .map(|section| section.kind == SectionKind::DirectMessages)
            .unwrap_or_else(|| key.as_ref() == "dms");
        let (filter, sort) = (self.section_filter(&key), self.section_sort(&key, dm));
        let hidden = self.hidden_conversations(&key, ids);
        SectionMenuInput {
            key,
            title,
            section,
            hidden,
            unread,
            unmuted,
            filter,
            sort,
        }
    }

    /// The conversations of `ids` (the section keyed `key`) its rows leave out, grouped by
    /// why ([`Self::hidden_reason`]).
    fn hidden_conversations(&self, key: &SharedString, ids: &[ChannelId]) -> HiddenConversations {
        let (open, filter) = (!self.collapsed.contains(key), self.section_filter(key));
        let watermark = self.read_watermark();
        let mut hidden = HiddenConversations::default();
        for conversation in ids.iter().filter_map(|id| self.fixtures.conversation(id)) {
            let unread = self.unread_in(conversation, watermark.as_ref());
            if let Some(why) = self.hidden_reason(conversation, unread, filter, open) {
                hidden.push(why, conversation.id.clone());
            }
        }
        hidden
    }

    /// Open `key`'s "More conversations" popover over the section as it is now: "Mark all
    /// as read" while anything in it is unread, then what its rows leave out.
    fn open_more_conversations(
        &mut self,
        key: &SharedString,
        ids: &[ChannelId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hidden = self.hidden_conversations(key, ids);
        let watermark = self.read_watermark();
        let mark: Vec<ChannelId> = ids
            .iter()
            .filter(|id| {
                self.fixtures
                    .conversation(id)
                    .is_some_and(|c| self.unread_in(c, watermark.as_ref()).count > 0)
            })
            .cloned()
            .collect();
        let this = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, |menu, _, _| {
            more_conversations_menu(
                menu.min_w(px(300.)).max_w(px(300.)),
                &hidden,
                mark,
                |ids| self.menu_rows(ids),
                &this,
            )
        });
        let dismiss = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.section_search = None;
            cx.notify();
        });
        menu.focus_handle(cx).focus(window, cx);
        self.section_search = Some(SectionSearch {
            key: key.clone(),
            menu,
            _dismiss: dismiss,
        });
        cx.notify();
    }

    /// Why the section leaves `conversation` out of its rows, by Slack's rules in Slack's
    /// order; `None` keeps it. The open conversation always stays. A muted one leaves unless
    /// something mentions you there (Slack's "Mute and hide"); a collapsed section keeps
    /// only the conversations that mention you; an expanded one applies its filter. What
    /// leaves is grouped as the "More conversations" popover lists it: dormant (under
    /// "Active only"), then muted, then unread, then read.
    fn hidden_reason(
        &self,
        conversation: &Conversation,
        unread: Unread,
        filter: SectionFilter,
        open: bool,
    ) -> Option<Hidden> {
        if conversation.id == self.selected {
            return None;
        }
        let muted = self.notify_level(&conversation.id) == Some(NotifyLevel::Muted);
        let listed = if muted || !open {
            unread.mentions > 0
        } else {
            match filter {
                SectionFilter::ActiveOnly => unread.count > 0 || self.active_recent(conversation),
                SectionFilter::All => true,
                SectionFilter::Unreads => unread.count > 0,
                SectionFilter::Mentions => unread.mentions > 0,
            }
        };
        if listed {
            return None;
        }
        Some(
            if filter == SectionFilter::ActiveOnly && !self.active_recent(conversation) {
                Hidden::Dormant
            } else if muted {
                Hidden::Muted
            } else if unread.count > 0 {
                Hidden::Unread
            } else {
                Hidden::Read
            },
        )
    }

    /// Slack's "Active only" rule (`sidebar_behavior: hide_inactive_channels`): Slack's
    /// client hides exactly the conversations the server marks dormant
    /// (`properties.is_dormant`, labelled "No messages in at least 30 days") and lists every
    /// other one however quiet, so a silent but undormant channel such as
    /// #partner_support_klarna stays.
    fn active_recent(&self, conversation: &Conversation) -> bool {
        !self.active_only || !self.is_dormant(conversation)
    }

    /// Whether Slack marks the conversation dormant ("No messages in at least 30 days");
    /// the workspace's #general never counts as inactive.
    fn is_dormant(&self, conversation: &Conversation) -> bool {
        !conversation.is_general
            && conversation
                .properties
                .as_ref()
                .and_then(|properties| properties.extra.get("is_dormant"))
                .and_then(|dormant| dormant.as_bool())
                .unwrap_or(false)
    }

    /// The gear menu's "Create a channel section": the dialog over a fresh name field.
    pub(crate) fn open_section_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.section_form_mode = SectionFormMode::Create;
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

    /// A section's "Rename": the same dialog seeded with the current name.
    pub(crate) fn open_rename_section(
        &mut self,
        section: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.section_form_mode = SectionFormMode::Rename(section);
        self.section_form
            .update(cx, |form, cx| form.open_editing(&name, window, cx));
        let form = self.section_form.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Rename section")
                .w(px(440.))
                .child(form.clone())
        });
    }

    /// The row menu's "+ Create new section": the dialog, and the channel moves into the
    /// fresh section once it confirms.
    pub(crate) fn open_create_with_channel(
        &mut self,
        channel: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.section_form_mode = SectionFormMode::CreateWithChannel(channel);
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

    /// "Delete section", behind Slack's confirmation: the section's channels fall back to
    /// their default buckets.
    pub(crate) fn ask_delete_section(
        &mut self,
        section: &ChannelSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        let id = section.id.clone();
        let name = section.name.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            let (this, section_id) = (this.clone(), id.clone());
            dialog
                .title(format!("Delete {name}?"))
                .w(px(440.))
                .child(
                    v_flex()
                        .gap_4()
                        .child(div().text_color(muted).child(format!(
                            "Any channels in {name} will move back to their default sections."
                        )))
                        .child(
                            h_flex().gap_2().justify_end().child(
                                Button::new("section-delete-cancel")
                                    .label("Cancel")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            ),
                        ),
                )
                // The confirm rides as the dialog's footer action so it reads as one.
                .child(
                    h_flex().justify_end().child(
                        Button::new("section-delete-confirm")
                            .danger()
                            .label("Delete section")
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = this.update(cx, |_, cx| {
                                    cx.emit(SidebarEvent::DeleteSection {
                                        section: section_id.clone(),
                                    })
                                });
                            }),
                    ),
                )
        });
    }

    pub(crate) fn select_row(&mut self, id: ChannelId, cx: &mut Context<Self>) {
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
        if conversation.id == self.selected || self.read.contains(&conversation.id) {
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

    /// The conversation row above (`forward` false) or below `from` in the Home list as laid
    /// out, or the nearest unread one with `unread_only` (Slack's Opt+Up/Down and
    /// Opt+Shift+Up/Down); from a conversation not in the list, the first or last match.
    pub fn step(&self, from: &ChannelId, forward: bool, unread_only: bool) -> Option<ChannelId> {
        let rows = self.rows.borrow();
        let at = rows.iter().position(|(id, _)| id == from);
        let pick = |row: &&(ChannelId, bool)| !unread_only || row.1;
        let found = if forward {
            rows.iter().skip(at.map_or(0, |at| at + 1)).find(pick)
        } else {
            rows[..at.unwrap_or(rows.len())].iter().rev().find(pick)
        };
        found.map(|(id, _)| id.clone())
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
        for item in NavItem::ALL {
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
    fn rail_button(
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

    fn workspace_header(&self, cx: &mut Context<Self>) -> Div {
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

    /// A sidebar section: a dim header row (section emoji or icon + name; click collapses)
    /// over its conversation rows, less the ones Slack's rules hide ([`Self::hidden_reason`]);
    /// a collapsed section keeps only its open conversation and the ones that mention you.
    /// An expanded Channels section ends with the "Add channels" row.
    /// The section flattens into direct children of the scroll list (each carrying the
    /// section's own padding) so its scroll handle can track every row.
    /// `(title, leading)` is the section's name and the icon or emoji before it.
    fn section(
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
    fn section_header(
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
    fn section_list(
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
    fn add_channels_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
        let hover = theme.sidebar_foreground.opacity(0.08);
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
        let hover = theme.sidebar_foreground.opacity(0.08);
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
        section: Option<&ChannelSection>,
        key: &SharedString,
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
    fn channel_menu_input(
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
    fn reveal_unread(&mut self, target: HiddenUnreads) {
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
    fn hover_shared_row(&mut self, id: &ChannelId, hovered: bool, cx: &mut Context<Self>) {
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
    fn step_scroll(&mut self, window: &mut Window) {
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

/// A "More unreads" scroll in flight: Slack eases the list to its target instead of jumping.
#[derive(Clone, Copy)]
struct ScrollAnim {
    from: Pixels,
    to: Pixels,
    start: Instant,
    /// The offset this animation set last; finding any other means the user scrolled.
    last: Pixels,
}

/// The open "More conversations" popover: its section and the menu, until the menu dismisses.
struct SectionSearch {
    key: SharedString,
    menu: Entity<PopupMenu>,
    _dismiss: Subscription,
}

/// What the "Create a channel section" dialog builds when it confirms.
enum SectionFormMode {
    /// A plain new section.
    Create,
    /// `Rename(section)` of an existing one.
    Rename(String),
    /// A new section that `CreateWithChannel(channel)` moves into.
    CreateWithChannel(ChannelId),
}

/// Where a channel dropped on a section lands: `Some(target)` accepts the drop, with
/// `target` the custom section's id or `None` for the default Channels bucket. The DMs
/// bucket and the server-managed buckets take no drops.
fn drop_target(section: Option<&ChannelSection>, key: &SharedString) -> Option<Option<String>> {
    match section {
        Some(section) => match section.kind {
            SectionKind::Standard => Some(Some(section.id.clone())),
            SectionKind::Channels => Some(None),
            _ => None,
        },
        None => (key.as_ref() == "channels").then_some(None),
    }
}

/// Whether a section takes DM drops (custom sections do; the Channels bucket does not).
fn accepts_dms(section: Option<&ChannelSection>, key: &SharedString) -> bool {
    match section {
        Some(section) => section.kind == SectionKind::Standard,
        None => key.as_ref() != "channels",
    }
}

const SCROLL_ANIM: Duration = Duration::from_millis(300);

/// Slack's tooltip over a Slack Connect row, centered above the name with its arrow pointing
/// down at it: the other organizations' icons in framed tiles (three, then a "+N" tile) over
/// "A, B, C, and N other external organizations are in this channel", in Slack's inverse
/// tooltip colors and caption type. `None` while no organization record is known.
fn org_tooltip(teams: &[&Team], cx: &gpui_kit::gpui::App) -> Option<Div> {
    let text = match teams {
        [] => return None,
        [a] => format!("{} is in this channel", a.name),
        [a, b] => format!("{} and {} are in this channel", a.name, b.name),
        [a, b, c] => format!("{}, {}, and {} are in this channel", a.name, b.name, c.name),
        [a, b, c, rest @ ..] => format!(
            "{}, {}, {}, and {} other external organization{} are in this channel",
            a.name,
            b.name,
            c.name,
            rest.len(),
            if rest.len() == 1 { "" } else { "s" }
        ),
    };
    let theme = cx.theme();
    let (surface, ink, plus) = (theme.foreground, theme.background, theme.muted_foreground);
    let frame = |tile: Div| div().p(px(2.)).rounded(px(10.)).bg(ink).child(tile);
    let more = teams.len().saturating_sub(3);
    let tiles = h_flex()
        .justify_center()
        .gap_1()
        .my_1()
        .children(
            teams
                .iter()
                .take(3)
                .map(|team| frame(crate::channel_details::org_chip(team, 36.))),
        )
        .when(more > 0, |tiles| {
            tiles.child(frame(
                div()
                    .size(px(36.))
                    .rounded(px(8.))
                    .bg(plus)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(20.))
                    .child(if more < 9 {
                        format!("+{more}")
                    } else {
                        "9+".to_owned()
                    }),
            ))
        });
    Some(
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(relative(1.))
            .pb(px(2.))
            .flex()
            .justify_center()
            .child(
                v_flex()
                    .items_center()
                    .child(
                        v_flex()
                            .max_w(px(200.))
                            .px_3()
                            .py_2()
                            .rounded(px(8.))
                            .bg(surface)
                            .text_color(ink)
                            .text_size(px(13.))
                            .line_height(px(16.25))
                            .font_weight(FontWeight::BOLD)
                            .text_center()
                            .child(tiles)
                            .child(text),
                    )
                    .child(tooltip_arrow(surface, false)),
            ),
    )
}

/// A Slack tooltip's arrow in the tooltip's color: a small triangle pointing down at the
/// trigger below the tooltip, or up at one above it.
pub(crate) fn tooltip_arrow(color: Hsla, up: bool) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let (left, top) = (bounds.origin.x, bounds.origin.y);
            let (width, height) = (bounds.size.width, bounds.size.height);
            let (base, tip) = if up {
                (top + height, top)
            } else {
                (top, top + height)
            };
            let mut path = PathBuilder::fill();
            path.move_to(point(left, base));
            path.line_to(point(left + width, base));
            path.line_to(point(left + width / 2., tip));
            path.close();
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .w(px(14.))
    .h(px(7.))
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
    /// The hidden unread row farthest past the edge.
    far: usize,
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
                    far: row,
                    mentions: false,
                    below: true,
                });
                pill.far = row;
                pill.mentions |= mentions;
            } else if bounds.top() < top {
                above = Some(HiddenUnreads {
                    row,
                    far: above.map_or(row, |pill| pill.far),
                    mentions: above.is_some_and(|pill| pill.mentions) || mentions,
                    below: false,
                });
            }
        }
    }
    (below, above)
}

/// A section's header: Slack's current label for a built-in bucket (the stored names lag it:
/// "Slack Connect", "Direct Messages"), else the user's name, led by the section emoji (when
/// it resolves) or the bucket's icon; `None` leaves the header's own caret square.
fn section_title(section: &ChannelSection, fixtures: &Fixtures) -> (String, Option<AnyElement>) {
    let icon = |name: IconName| Some(Icon::new(name).size(px(15.)).into_any_element());
    let emoji = &section.emoji;
    let resolves = emoji::display_glyph(emoji).is_some() || fixtures.custom_emoji(emoji).is_some();
    let (builtin_name, leading) = match section.kind {
        SectionKind::Channels => (Some("Channels"), None),
        SectionKind::DirectMessages => (Some("Direct messages"), None),
        SectionKind::Stars => (Some("Starred"), icon(IconName::Star)),
        SectionKind::SlackConnect => (Some("External connections"), icon(IconName::Building2)),
        SectionKind::RecentApps => (Some("Apps"), icon(IconName::LayoutGrid)),
        SectionKind::Agents => (Some("Agents"), icon(IconName::Bot)),
        _ if !emoji.is_empty() && resolves => {
            (None, Some(emoji::emoji_element(emoji, fixtures, px(15.))))
        }
        _ => (None, None),
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
        RichTextContainer::Header(header) => header
            .elements
            .iter()
            .any(|element| element_mentions_user(element, user)),
        RichTextContainer::Divider(_) => false,
        RichTextContainer::Unknown { .. } => false,
    }
}

fn element_mentions_user(element: &RichTextElement, user: &UserId) -> bool {
    matches!(element, RichTextElement::User(mention) if mention.user_id == *user)
}

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
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenUnreads))),
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
