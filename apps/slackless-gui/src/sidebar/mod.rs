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
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::{ActiveTheme as _, Icon, WindowExt as _, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, AppContext as _, Context, DismissEvent, Div, Entity, EventEmitter, Focusable as _,
    FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, PathBuilder,
    Pixels, Point, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, canvas, div, point, px, relative,
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
use crate::sidebar_menu::{
    Hidden, HiddenConversations, MenuRow, SectionFilter, SectionFormEvent, SectionMenuInput,
    SectionNameForm, SectionSort, more_conversations_menu, workspace_menu,
};
use crate::status::SelfState;

/// Slack's workspace switcher column, left of the tab rail.
mod dms;
mod home;
mod rail;

pub(crate) const SWITCHER_WIDTH: f32 = 60.;
/// Slack's tab rail.
pub(super) const TAB_RAIL_WIDTH: f32 = 70.;

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

/// Which Home sidebar shortcut page currently owns the main column (`None` = a conversation).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HomePage {
    Unreads,
    Threads,
    Huddles,
    Drafts,
    Saved,
    Directories,
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
pub(super) struct Unread {
    pub(super) count: u32,
    pub(super) mentions: u32,
}

pub struct Sidebar {
    pub(super) fixtures: Arc<Fixtures>,
    pub(super) selected: ChannelId,
    /// When a Home shortcut page (Unreads, Threads, Huddles, Drafts, Saved, Directories) is
    /// open, that row carries the sidebar selection highlight instead of `selected`.
    pub(super) active_home_page: Option<HomePage>,
    pub(super) active_workspace: usize,
    pub(super) active_nav: NavItem,
    /// The Files list column's selected row: the page the Files view shows.
    pub(super) files_section: FilesSection,
    /// Each rail destination's list scroll, restored when the reader comes back to it.
    pub(super) nav_offsets: HashMap<NavItem, Point<Pixels>>,
    /// Keys of the collapsed sidebar sections.
    pub(super) collapsed: HashSet<SharedString>,
    /// Conversations read in this session since Slack's counts were taken.
    pub(super) read: HashSet<ChannelId>,
    /// The "Find a conversation…" field above the sections.
    pub(super) find: Entity<TextareaState>,
    /// The "Find a conversation…" query, trimmed and lowercased; empty lists everything.
    pub(super) find_query: String,
    /// The DM list's "Find a DM…" field.
    pub(super) dm_find: Entity<TextareaState>,
    /// The DM list's "Find a DM…" query, trimmed and lowercased; empty lists every DM.
    pub(super) dm_find_query: String,
    /// Live mode: DMs' newest messages read from the store since the last snapshot (a send,
    /// an arrival, an edit or a deletion), fresher than the snapshot's `latest`; `None`: the
    /// DM has no message left.
    pub(super) fresh_latest: HashMap<ChannelId, Option<Message>>,
    /// Slack's thread state (`client.counts`): whether followed threads have unread replies,
    /// and how many of those mention you.
    pub(super) threads_unread: (bool, u32),
    /// The DM list's "Unreads" toggle: only DMs with unread messages.
    pub(super) dm_unreads_only: bool,
    /// Channels joined from the channel browser this session, listed after the fixtures'.
    pub(super) joined: Vec<Conversation>,
    /// Conversations with a live huddle; their rows carry the headphones indicator.
    pub(super) huddles: HashSet<ChannelId>,
    /// Saved drafts, shown on the "Drafts & sent" row.
    pub(super) draft_count: usize,
    /// In-progress Later items, shown on the "Later" row.
    pub(super) later_count: usize,
    /// The gear menu is open (a scrim covers the sidebar list under it).
    pub(super) settings_menu_open: bool,
    /// Slack's "Active only" sidebar filter (Slack's default): hide dormant conversations.
    pub(super) active_only: bool,
    /// Per-section list filters and sorts (Slack keeps them per section too, in its
    /// `channel_sections` pref); a section without an entry follows [`Self::active_only`]
    /// and the built-in sort. Session-local, like Slack's until it syncs them.
    pub(super) section_filters: HashMap<SharedString, SectionFilter>,
    pub(super) section_sorts: HashMap<SharedString, SectionSort>,
    /// Fixture mode's section overlay: sections renamed, moved into, created or deleted
    /// locally this session, seeded from the fixtures on first change (`None`: pristine).
    /// Live sections arrive via the store and never touch this.
    pub(super) local_sections: Option<Vec<ChannelSection>>,
    /// Fixture mode's notification-level overrides (a live write lands through the store's
    /// prefs on the next snapshot).
    pub(super) notify_override: HashMap<ChannelId, NotifyLevel>,
    /// The Home list's conversation rows as last laid out, top to bottom, each with whether
    /// it is unread: what Opt+Up/Down and Opt+Shift+Up/Down step through ([`Self::step`]).
    pub(super) rows: RefCell<Vec<(ChannelId, bool)>>,
    /// The open "More conversations" popover.
    pub(super) section_search: Option<SectionSearch>,
    /// Whether the press on a "More conversations" button found its popover open: the press
    /// closes it, so the click that follows must not open it again.
    pub(super) search_was_open: bool,
    /// What the "Create a channel section" dialog builds when it confirms: a plain
    /// section, a rename of `id`, or a new section that `channel` moves into.
    pub(super) section_form_mode: SectionFormMode,
    /// The "Create a channel section" dialog body.
    pub(super) section_form: Entity<SectionNameForm>,
    /// Tracks the sidebar list's scroll state; the "More unreads" pills read which unread
    /// rows the scroll offset carries out of view.
    pub(super) scroll: ScrollHandle,
    /// The "More unreads" scroll in flight, eased one frame per render.
    pub(super) scroll_anim: Option<ScrollAnim>,
    /// The Slack Connect row whose organizations tooltip is showing, and the row whose
    /// tooltip waits out the hover delay (dropping the task cancels it).
    pub(super) org_tip: Option<ChannelId>,
    pub(super) org_tip_delay: Option<(ChannelId, Task<()>)>,
    /// Unread Activity rows (the root pushes the live feed's count; fixture mode derives
    /// it), for the rail tile and the dock badge.
    pub(super) activity_unread: u32,
    /// Per-workspace unread / mention badges shown on the workspace switcher rail tiles.
    pub(super) workspace_badges: Vec<WorkspaceBadge>,
    /// Navigation rail visibility and label style from Preferences -> Navigation.
    pub(super) nav_prefs: NavPrefs,
    /// Home sidebar filter, sort, photo, and row-density preferences from Preferences -> Home.
    pub(super) home_prefs: HomePrefs,
    /// The signed-in user's status and presence, for the rail avatar.
    pub(super) self_state: SelfState,
    /// The pointer is over the focus button / the focus card.
    pub(super) focus_hover: (bool, bool),
    pub(super) _subscriptions: Vec<Subscription>,
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
                active_home_page: None,
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
        self.active_home_page = None;
        cx.notify();
    }

    /// Highlight one of the Home shortcut rows (Unreads, Threads, …) instead of a channel.
    pub fn set_home_page(&mut self, page: Option<HomePage>, cx: &mut Context<Self>) {
        if self.active_home_page != page {
            self.active_home_page = page;
            cx.notify();
        }
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
    pub(super) fn section_set(&self) -> Vec<ChannelSection> {
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
    pub(super) fn notify_level(&self, channel: &ChannelId) -> Option<NotifyLevel> {
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
    pub(super) fn section_filter(&self, key: &SharedString) -> SectionFilter {
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
    pub(super) fn section_sort(&self, key: &SharedString, dm: bool) -> SectionSort {
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
    pub(super) fn section_menu_input(
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
    pub(super) fn hidden_conversations(
        &self,
        key: &SharedString,
        ids: &[ChannelId],
    ) -> HiddenConversations {
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
    pub(super) fn open_more_conversations(
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
    pub(super) fn hidden_reason(
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
    pub(super) fn active_recent(&self, conversation: &Conversation) -> bool {
        !self.active_only || !self.is_dormant(conversation)
    }

    /// Whether Slack marks the conversation dormant ("No messages in at least 30 days");
    /// the workspace's #general never counts as inactive.
    pub(super) fn is_dormant(&self, conversation: &Conversation) -> bool {
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
    pub(super) fn read_watermark(&self) -> Option<Ts> {
        self.fixtures
            .messages
            .get(&self.selected)
            .and_then(|history| history.iter().map(|message| message.ts.clone()).max())
    }

    /// Messages in `conversation` the signed-in user has not read (see [`Unread`]).
    pub(super) fn unread_messages(
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

    pub(super) fn unread_in(&self, conversation: &Conversation, watermark: Option<&Ts>) -> Unread {
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

    pub(super) fn total_unread(
        &self,
        conversations: &[Conversation],
        watermark: Option<&Ts>,
    ) -> Unread {
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

    /// Jump to the first conversation in the next or previous sidebar section (`F-SHELL-270`).
    pub fn step_section(&self, from: &ChannelId, forward: bool) -> Option<ChannelId> {
        let mut heads: Vec<ChannelId> = Vec::new();
        for section in &self.fixtures.sections {
            if let Some(first) = section
                .channel_ids
                .iter()
                .find(|id| self.fixtures.conversation(id).is_some())
            {
                if !heads.contains(first) {
                    heads.push(first.clone());
                }
            }
        }
        if let Some(first_ch) = self.fixtures.channels.first() {
            if !heads.contains(&first_ch.id) {
                heads.push(first_ch.id.clone());
            }
        }
        if let Some(first_dm) = self.fixtures.dms.first() {
            if !heads.contains(&first_dm.id) {
                heads.push(first_dm.id.clone());
            }
        }
        if heads.is_empty() {
            return None;
        }
        let rows = self.rows.borrow();
        let cur_row = rows.iter().position(|(id, _)| id == from);
        let head_positions: Vec<(usize, ChannelId)> = heads
            .into_iter()
            .filter_map(|id| rows.iter().position(|(r, _)| r == &id).map(|pos| (pos, id)))
            .collect();
        if head_positions.is_empty() {
            return None;
        }
        if forward {
            let cur = cur_row.unwrap_or(0);
            head_positions
                .iter()
                .find(|(pos, _)| *pos > cur)
                .or_else(|| head_positions.first())
                .map(|(_, id)| id.clone())
        } else {
            let cur = cur_row.unwrap_or(usize::MAX);
            head_positions
                .iter()
                .rev()
                .find(|(pos, _)| *pos < cur)
                .or_else(|| head_positions.last())
                .map(|(_, id)| id.clone())
        }
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
}

/// A "More unreads" scroll in flight: Slack eases the list to its target instead of jumping.
#[derive(Clone, Copy)]
pub(super) struct ScrollAnim {
    pub(super) from: Pixels,
    pub(super) to: Pixels,
    pub(super) start: Instant,
    /// The offset this animation set last; finding any other means the user scrolled.
    pub(super) last: Pixels,
}

/// The open "More conversations" popover: its section and the menu, until the menu dismisses.
pub(super) struct SectionSearch {
    pub(super) key: SharedString,
    pub(super) menu: Entity<PopupMenu>,
    pub(super) _dismiss: Subscription,
}

/// What the "Create a channel section" dialog builds when it confirms.
pub(super) enum SectionFormMode {
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
pub(super) fn drop_target(
    section: Option<&ChannelSection>,
    key: &SharedString,
) -> Option<Option<String>> {
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
pub(super) fn accepts_dms(section: Option<&ChannelSection>, key: &SharedString) -> bool {
    match section {
        Some(section) => section.kind == SectionKind::Standard,
        None => key.as_ref() != "channels",
    }
}

pub(super) const SCROLL_ANIM: Duration = Duration::from_millis(300);

/// Slack's tooltip over a Slack Connect row, centered above the name with its arrow pointing
/// down at it: the other organizations' icons in framed tiles (three, then a "+N" tile) over
/// "A, B, C, and N other external organizations are in this channel", in Slack's inverse
/// tooltip colors and caption type. `None` while no organization record is known.
pub(super) fn org_tooltip(teams: &[&Team], cx: &gpui_kit::gpui::App) -> Option<Div> {
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
pub(super) fn focus_dot(cx: &gpui_kit::gpui::App) -> Div {
    div()
        .flex_none()
        .size(px(6.))
        .rounded_full()
        .bg(cx.theme().sidebar_foreground.opacity(0.6))
}

/// Slack-style inline count pill: red for mentions (and DM messages), the sidebar's primary
/// emphasis colors otherwise.
pub(super) fn count_pill(count: u32, mentions: bool, cx: &gpui_kit::gpui::App) -> Div {
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
pub(super) enum ListChild {
    /// An unread conversation row; the flag says it carries mentions.
    Unread(bool),
    /// Headers, read rows and dividers.
    Plain,
}

/// Room kept between a revealed unread row and the list edge: the pill's height plus its inset.
pub(super) const PILL_CLEARANCE: f32 = 44.;

/// One edge of the list's hidden unreads: the unread row the pill scrolls to next, and
/// whether any unread row hidden past that edge carries mentions. `below` picks the pill's
/// arrow and the list edge it rides.
#[derive(Clone, Copy)]

pub(super) struct HiddenUnreads {
    pub(super) row: usize,
    /// The hidden unread row farthest past the edge.
    pub(super) far: usize,
    pub(super) mentions: bool,
    pub(super) below: bool,
}

/// The "More unreads" pills for `unread_rows` (child indices with mention flags, in list
/// order): the first unread row below the fold and the last one scrolled above it. Rows are
/// placed with the scroll handle's recorded child bounds; `None` on an edge whose unread
/// rows are in view (or before the list has laid out).
pub(super) fn hidden_unreads(
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
pub(super) fn section_title(
    section: &ChannelSection,
    fixtures: &Fixtures,
) -> (String, Option<AnyElement>) {
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
pub(super) fn team_icon_url(team: &Team) -> Option<String> {
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

pub(super) fn container_mentions_user(container: &RichTextContainer, user: &UserId) -> bool {
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

pub(super) fn element_mentions_user(element: &RichTextElement, user: &UserId) -> bool {
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
                        self.active_home_page == Some(HomePage::Unreads),
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
                        self.active_home_page == Some(HomePage::Threads),
                        None,
                        cx,
                    )
                    .when(self.threads_unread.1 > 0 && !focus, |row| {
                        row.child(count_pill(self.threads_unread.1, true, cx))
                    })
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenThreads))),
                    self.nav_row(
                        "huddles",
                        IconName::Headphones,
                        "Huddles",
                        false,
                        self.active_home_page == Some(HomePage::Huddles),
                        None,
                        cx,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenHuddles))),
                    self.nav_row(
                        "drafts",
                        IconName::Send,
                        "Drafts & sent",
                        false,
                        self.active_home_page == Some(HomePage::Drafts),
                        (self.draft_count > 0).then(|| self.draft_count.to_string()),
                        cx,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenDrafts))),
                    self.nav_row(
                        "saved",
                        IconName::Bookmark,
                        "Saved",
                        false,
                        self.active_home_page == Some(HomePage::Saved),
                        (self.later_count > 0).then(|| self.later_count.to_string()),
                        cx,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::OpenLater))),
                    self.nav_row(
                        "directories",
                        IconName::BookUser,
                        "Directories",
                        false,
                        self.active_home_page == Some(HomePage::Directories),
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
                                cx,
                            )),
                    )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui;

    #[test]
    fn nav_prefs_shows_respects_toggles() {
        let prefs = NavPrefs {
            show_home: false,
            show_dms: true,
            show_activity: false,
            show_files: true,
            show_later: false,
            icons_only: true,
        };
        assert!(!prefs.shows(NavItem::Home));
        assert!(prefs.shows(NavItem::Dms));
        assert!(!prefs.shows(NavItem::Activity));
        assert!(prefs.shows(NavItem::Files));
        assert!(!prefs.shows(NavItem::Todos));
        assert!(prefs.shows(NavItem::Admin));
    }

    #[test]
    fn drop_target_and_dm_acceptance_follow_section_kinds() {
        let dm_section = ChannelSection {
            id: "S_DM".into(),
            name: "Direct messages".into(),
            kind: SectionKind::DirectMessages,
            emoji: String::new(),
            next_id: None,
            channel_ids: Vec::new(),
        };
        let custom_section = ChannelSection {
            id: "S_CUSTOM".into(),
            name: "Priority".into(),
            kind: SectionKind::Standard,
            emoji: String::new(),
            next_id: None,
            channel_ids: Vec::new(),
        };
        let channels_key = SharedString::from("channels");
        let dms_key = SharedString::from("dms");
        let custom_key = SharedString::from("S_CUSTOM");

        assert_eq!(drop_target(None, &channels_key), Some(None));
        assert_eq!(drop_target(None, &dms_key), None);
        assert_eq!(drop_target(Some(&dm_section), &dms_key), None);
        assert_eq!(
            drop_target(Some(&custom_section), &custom_key),
            Some(Some("S_CUSTOM".into()))
        );
        assert!(accepts_dms(Some(&custom_section), &custom_key));
        assert!(!accepts_dms(None, &channels_key));
    }

    #[gpui::test]
    fn sidebar_unread_counts_stepping_and_section_jumps(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let mut fixtures = crate::fixtures::load();
        fixtures.messages.clear();
        let first = fixtures.channels[0].id.clone();
        let second = fixtures.channels[1].id.clone();
        let dm = fixtures.dms[0].id.clone();
        fixtures.counts.insert(
            second.clone(),
            slackless_core::api::ConversationCounts {
                id: second.clone(),
                has_unreads: true,
                mention_count: 2,
                ..Default::default()
            },
        );
        fixtures.counts.insert(
            dm.clone(),
            slackless_core::api::ConversationCounts {
                id: dm.clone(),
                has_unreads: true,
                mention_count: 3,
                ..Default::default()
            },
        );
        let fixtures = Arc::new(fixtures);

        let mounted = std::rc::Rc::new(std::cell::RefCell::new(None));
        let slot = mounted.clone();
        let init_fixtures = fixtures.clone();
        let init_first = first.clone();
        let (_root, cx) = cx.add_window_view(move |window, cx| {
            let entity = Sidebar::new(init_fixtures.clone(), init_first.clone(), window, cx);
            *slot.borrow_mut() = Some(entity.clone());
            gpui_kit::component::Root::new(entity, window, cx)
        });
        let sidebar = mounted.borrow_mut().take().unwrap();

        // Unread counts, badge_count, and mark_read.
        sidebar.read_with(cx, |s, _| {
            let second_conv = fixtures.conversation(&second).unwrap();
            let unread = s.unread_in(second_conv, None);
            assert_eq!(unread.count, 1);
            assert_eq!(unread.mentions, 2);
            assert!(s.unread_ids().contains(&second));
            assert!(s.unread_ids().contains(&dm));
            assert!(s.has_any_unreads());
            // Dock badge = unread activity (0) + unread DMs (1).
            assert_eq!(s.badge_count(), 1);
        });

        sidebar.update(cx, |s, cx| {
            s.set_activity_unread(4, cx);
            s.mark_read(second.clone(), cx);
        });
        sidebar.read_with(cx, |s, _| {
            assert!(!s.unread_ids().contains(&second));
            assert_eq!(s.badge_count(), 5);
        });

        // `step` and `step_section` walk the laid-out rows cleanly.
        sidebar.read_with(cx, |s, _| {
            *s.rows.borrow_mut() = vec![
                (first.clone(), false),
                (second.clone(), false),
                (dm.clone(), true),
            ];
            assert_eq!(s.step(&first, true, false), Some(second.clone()));
            assert_eq!(s.step(&first, true, true), Some(dm.clone()));
            assert_eq!(s.step(&dm, false, false), Some(second.clone()));
            assert_eq!(s.step(&first, false, false), None);

            // Section jump moves between Channels head (`first`) and DMs head (`dm`) and wraps.
            let next_head = s.step_section(&first, true);
            assert!(next_head.is_some());
            assert_ne!(next_head.as_ref(), Some(&first));
            let prev_head = s.step_section(&dm, false);
            assert!(prev_head.is_some());
        });
    }
}
