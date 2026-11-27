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

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::gpui::{
    AppContext as _, Context, Entity, EventEmitter, Pixels, Point, ScrollHandle, SharedString,
    Subscription, Task, Window, point, px,
};
use slackless_core::NotifyLevel;
use slackless_core::types::{ChannelId, ChannelSection, Conversation, Message};

use crate::files_page::FilesSection;
use crate::fixtures::Fixtures;
use crate::sidebar_menu::{SectionFilter, SectionFormEvent, SectionNameForm, SectionSort};
use crate::status::SelfState;

mod dms;
mod helpers;
mod home;
mod rail;
mod render;
mod sections;
#[cfg(test)]
mod tests;
mod types;
mod unread;

use helpers::*;
pub(crate) use helpers::{mentions_user, team_initials, tooltip_arrow};
pub(crate) use types::NavItem;
use types::Unread;
pub use types::{HomePage, HomePrefs, NavPrefs, SidebarEvent, WorkspaceBadge};

pub(crate) const SWITCHER_WIDTH: f32 = 60.;
/// Slack's tab rail.
pub(super) const TAB_RAIL_WIDTH: f32 = 70.;

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
    /// Conversations with a live huddle and their active participants; their rows carry
    /// Slack's avatar + headphones count pill.
    pub(super) huddles: HashMap<ChannelId, Vec<slackless_core::types::UserId>>,
    /// Whether the signed-in user is currently in a live huddle (draws the rail avatar capsule).
    pub(super) self_in_huddle: bool,
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
    /// Locally starred conversations when the workspace has no server `SectionKind::Stars` (`F-CORE-019`).
    pub(super) local_starred: HashSet<ChannelId>,
    /// Fixture mode's notification-level overrides (a live write lands through the store's
    /// prefs on the next snapshot).
    pub(super) notify_override: HashMap<ChannelId, NotifyLevel>,
    /// The Home list's conversation rows as last laid out, top to bottom, each with whether
    /// it is unread: what Opt+Up/Down and Opt+Shift+Up/Down step through ([`Self::step`]).
    pub(super) rows: RefCell<Vec<(ChannelId, bool)>>,
    /// The open "More conversations" popover.
    pub(in crate::sidebar) section_search: Option<SectionSearch>,
    /// Whether the press on a "More conversations" button found its popover open: the press
    /// closes it, so the click that follows must not open it again.
    pub(super) search_was_open: bool,
    /// What the "Create a channel section" dialog builds when it confirms: a plain
    /// section, a rename of `id`, or a new section that `channel` moves into.
    pub(in crate::sidebar) section_form_mode: SectionFormMode,
    /// The "Create a channel section" dialog body.
    pub(super) section_form: Entity<SectionNameForm>,
    /// Tracks the sidebar list's scroll state; the "More unreads" pills read which unread
    /// rows the scroll offset carries out of view.
    pub(super) scroll: ScrollHandle,
    /// Last rendered unread row indices & hidden-unread pill pair, so scroll events only
    /// trigger a full sidebar re-render when a pill actually appears, changes, or disappears.
    pub(super) last_unread_rows: Vec<(usize, bool)>,
    pub(in crate::sidebar) last_hidden_pills: (Option<HiddenUnreads>, Option<HiddenUnreads>),
    /// The "More unreads" scroll in flight, eased one frame per render.
    pub(in crate::sidebar) scroll_anim: Option<ScrollAnim>,
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
    /// Comma-separated VIP handles/names/IDs from Preferences -> Notifications (`F-SHELL-058`).
    pub(super) vip_users: String,
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
                huddles: HashMap::new(),
                self_in_huddle: false,
                draft_count: 0,
                later_count: 0,
                settings_menu_open: false,
                active_only: true,
                section_filters: HashMap::new(),
                section_sorts: HashMap::new(),
                local_sections: None,
                local_starred: HashSet::new(),
                notify_override: HashMap::new(),
                rows: RefCell::default(),
                section_search: None,
                search_was_open: false,
                section_form_mode: SectionFormMode::Create,
                section_form,
                scroll: ScrollHandle::new(),
                last_unread_rows: Vec::new(),
                last_hidden_pills: (None, None),
                scroll_anim: None,
                org_tip: None,
                org_tip_delay: None,
                activity_unread: 0,
                workspace_badges: Vec::new(),
                nav_prefs: NavPrefs::default(),
                home_prefs: HomePrefs::default(),
                vip_users: String::new(),
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

    /// Update the configured VIP list driving the top-level VIPs sidebar section (`F-SHELL-058`).
    pub fn set_vip_users(&mut self, vip_users: String, cx: &mut Context<Self>) {
        if self.vip_users != vip_users {
            self.vip_users = vip_users;
            cx.notify();
        }
    }

    /// Live mode: Slack's thread counts (unread replies in followed threads, mentions among
    /// them) for the Threads row.
    pub fn set_thread_counts(&mut self, has_unreads: bool, mentions: u32, cx: &mut Context<Self>) {
        self.threads_unread = (has_unreads, mentions);
        cx.notify();
    }

    pub fn has_thread_unreads(&self) -> bool {
        self.threads_unread.0 || self.threads_unread.1 > 0
    }

    pub fn thread_mentions(&self) -> u32 {
        self.threads_unread.1
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
        if matches!(
            self.active_nav,
            NavItem::Activity | NavItem::Files | NavItem::Agents | NavItem::Todos
        ) {
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

    /// Allow `channel` to display as unread again (e.g. a new message arrived or it was marked unread).
    pub fn mark_unread(&mut self, channel: &ChannelId, cx: &mut Context<Self>) {
        if self.read.remove(channel) {
            cx.notify();
        }
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

    /// Cycle through visible navigation rail tabs (`Ctrl+Tab` / `Ctrl+Shift+Tab`), respecting the
    /// user's configured tab order (`NavPrefs::order`).
    pub fn cycle_nav(&mut self, forward: bool, cx: &mut Context<Self>) {
        let visible: Vec<NavItem> = self
            .nav_prefs
            .ordered_items()
            .into_iter()
            .filter(|&item| {
                !matches!(item, NavItem::Agents | NavItem::Admin) && self.nav_prefs.shows(item)
            })
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
            NavItem::Agents => cx.emit(SidebarEvent::OpenTools),
            NavItem::Admin => cx.emit(SidebarEvent::OpenAdmin),
            _ => {}
        }
    }

    /// Switch the sidebar back to the Home navigation tab (`Cmd+Shift+H`).
    pub fn show_home(&mut self, cx: &mut Context<Self>) {
        self.show_nav(NavItem::Home, cx);
    }

    /// Replace the map of conversations currently running a live huddle and their participants.
    pub fn set_huddles(
        &mut self,
        huddles: HashMap<ChannelId, Vec<slackless_core::types::UserId>>,
        cx: &mut Context<Self>,
    ) {
        if self.huddles != huddles {
            self.huddles = huddles;
            cx.notify();
        }
    }

    /// Whether the signed-in user is currently inside an active huddle (renders the rail
    /// headphones capsule above the self-avatar, matching official Slack).
    pub fn set_self_in_huddle(&mut self, in_huddle: bool, cx: &mut Context<Self>) {
        if self.self_in_huddle != in_huddle {
            self.self_in_huddle = in_huddle;
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

    pub(crate) fn select_row(&mut self, id: ChannelId, cx: &mut Context<Self>) {
        self.selected = id.clone();
        cx.emit(SidebarEvent::SelectConversation(id));
        cx.notify();
    }

    /// The rail destination the list column shows.
    pub(crate) fn active_nav(&self) -> NavItem {
        self.active_nav
    }
}
