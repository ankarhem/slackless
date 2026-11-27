//! Root view: title bar over a resizable layout of sidebar | message column + composer |
//! optional thread panel.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::mem;
use std::rc::Rc;
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Escape as InputEscape, InputEvent, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, Size, TitleBar, WindowExt as _, h_flex,
    h_resizable, resizable_panel, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    self, Anchor, AnyElement, AppContext as _, BoxShadow, ClipboardItem, Context, DismissEvent,
    DispatchPhase, Div, Edges, Entity, FocusHandle, Focusable as _, FontWeight, Hsla,
    InputPreference, InteractiveElement as _, IntoElement, KeyBinding, MouseButton, MouseDownEvent,
    NavigationDirection, ParentElement as _, Pixels, Point, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Subscription, SystemNotification, Window,
    actions, anchored, canvas, deferred, div, hsla, point, px, relative,
};
use slackless_core::api::{ChannelBookmark, ConversationCounts, FilesFilter, MessageContent};
use slackless_core::types::rich_text::BroadcastRange;
use slackless_core::types::{
    Block, ChannelId, Conversation, ConversationKind, File, Message, RichTextContainer,
    RichTextElement, Team, Ts, UserId, Usergroup,
};
use slackless_core::{
    ChannelCommand, CollabCommand, Command, Event, HuddleCommand, HuddleControlCommand,
    HuddleEvent, HuddleMediaEvent, MessagingCommand, NotifyLevel, SharedStore, WorkspaceCommand,
};
use slackless_platform::{self as platform, DesktopPlatform, StatusBarState};
#[cfg(not(test))]
use slackless_platform::{MouseNavDirection, StatusBarAction};

use crate::actions::{
    self, FocusDeleteMessage, FocusEditMessage, FocusForwardMessage, FocusMarkMessageUnread,
    FocusReact, FocusRemindMe, FocusReplyInThread, FocusSaveMessage, FocusTogglePin, MessageAction,
    MessageChange, notify,
};
use crate::activity::{ActivityEvent, ActivityFeed};
use crate::avatars::conversation_avatar;
use crate::bookmarks;
use crate::bridge::{CoreBridge, SavedDraft};
use crate::browser::{ChannelBrowser, ChannelBrowserEvent};
use crate::canvas::canvas_view;
use crate::catchup::{CatchUpEvent, CatchUpView};
use crate::channel_details::{self, ChannelDetails, ChannelDetailsEvent};
use crate::composer::{self, Composer, ComposerEvent, ContextBar, Placeholder, TypingNames};
use crate::directory::{DirectoryEvent, DirectoryView};
use crate::drafts::{DraftsEvent, DraftsView, Entry, now_secs};
use crate::emoji::{EmojiChoice, EmojiPicker, EmojiPickerEvent, PickerTab};
use crate::files_page::{FilesPage, FilesPageEvent, FilesSection};
use crate::fixtures::{self, Fixtures};
use crate::hover_card;
use crate::huddle_dock::HuddleDock;
use crate::huddle_invite::HuddleInviteBanner;
use crate::huddle_state::{self, HuddleState, HuddleStateEvent};
use crate::huddles::{self, HuddlesEvent, HuddlesView};
use crate::image_viewer::{ImageViewer, ViewerEvent, ViewerImage};
use crate::later::{LaterEvent, LaterView, Remind, SaveRequest};
use crate::lists::list_view;
use crate::menu_bar::{self, MenuBar};
use crate::messages::{LinkTarget, MessageList, MessageListEvent};
use crate::new_message::{NewMessageEvent, NewMessageView};
use crate::pins;
use crate::preferences::{self, AppearanceMode, Preferences, PreferencesEvent, Section};
use crate::profile::{ProfileEvent, ProfilePanel};
use crate::search::{
    SearchEvent, SearchView, SearchWorkspaceSource, fixture_archive, fixture_archive_for,
};
use crate::search_menu::{SEARCH_PLACEHOLDER, SearchMenu, SearchMenuEvent, Snapshot};
use crate::shortcuts::shortcuts_view;
use crate::sidebar::{NavItem, Sidebar, SidebarEvent, WorkspaceBadge};
use crate::slash::{Invocation, SlashKind};
use crate::status::{
    FOCUS_UNTIL_OFF, SelfState, StatusEvent, StatusPicker, UserMenu, UserMenuEvent,
};
use crate::switcher::{QuickSwitcher, SwitcherEvent};
use crate::tab_views::{self, Loaded};
use crate::tabs::{self, ConversationTab, TabAction};
use crate::theme::{self, Palette, SlackTheme};
use crate::threads::{ThreadPanel, ThreadPanelEvent};
use crate::threads_cards::{local_cards, store_cards};
use crate::threads_view::{ThreadsEvent, ThreadsView};
use crate::unreads::{
    LiveRefresh, UnreadsEvent, UnreadsView, local_groups, store_groups, store_targets,
};

actions!(
    slackless,
    [
        QuickSwitch,
        NavigateBack,
        NavigateForward,
        OpenPreferences,
        ShowShortcuts,
        OpenDms,
        OpenActivity,
        OpenDrafts,
        OpenChannelBrowser,
        OpenDownloads,
        SetStatus,
        FocusSearch,
        NewMessage,
        OpenThreads,
        OpenUnreads,
        OpenCatchUp,
        OpenHome,
        OpenLater,
        OpenFiles,
        OpenPeople,
        OpenOwnProfile,
        CreateChannelAction,
        JumpToUnread,
        NextWorkspace,
        PreviousWorkspace,
        PreviousConversation,
        NextConversation,
        PreviousUnread,
        NextUnread,
        NextSection,
        PrevSection,
        MarkConversationRead,
        MarkAllRead,
        SearchConversation,
        ToggleWorkspaceRail,
        ToggleRightPane,
        PopOutConversation,
        UnsendLastMessage,
        NextNavTab,
        PrevNavTab,
        FocusNextPane,
        FocusPrevPane,
        BrowseUp,
        BrowseDown,
        BrowseOpen,
        ExitThread,
        JumpToOldest,
        JumpToNewest,
        ZoomIn,
        ZoomOut,
        ActualSize,
        ToggleHuddleMute,
        CrossWorkspaceSummary,
        ReplayFocusedMessage,
    ]
);

/// Switch to the workspace at this index (Cmd+1…9, File > Workspace).
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = slackless, no_json)]
pub struct SwitchWorkspace(pub usize);

/// Switch to the conversation header tab at this index (Ctrl+1…9, `F-SHELL-280`).
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = slackless, no_json)]
pub struct SelectConversationTab(pub usize);

/// Sign out of the workspace at this index (the Slackless menu's Sign Out).
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = slackless, no_json)]
pub struct SignOut(pub usize);

/// Open a recently visited conversation (the History menu).
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = slackless, no_json)]
pub struct OpenRecent(pub ChannelId);

/// How often due Later reminders are checked.
const REMINDER_POLL: Duration = Duration::from_secs(10);

/// How long a typing announcement shows: Slack's receivers drop it after 5 s.
const TYPING_TTL: Duration = Duration::from_secs(5);

/// Who is typing where (conversation, thread), each until their announcement runs out.
type Typing = HashMap<(ChannelId, Option<Ts>), Vec<(UserId, Instant)>>;

/// The channel sidebar, measured on Slack desktop (318px).
const SIDEBAR_WIDTH: Pixels = px(318.);
/// The DMs destination's wider DM list, measured on Slack desktop (524px).
const DM_LIST_WIDTH: Pixels = px(524.);
/// The corner radius of Slack's workspace panel (`.p-client_workspace_wrapper`).
const PANEL_RADIUS: f32 = 6.;
/// The thread / profile / pins panel, measured on Slack desktop.
const FLEXPANE_WIDTH: Pixels = px(540.);

mod bootstrap;
mod chrome;
mod demo;
mod dialogs;
mod events;
mod helpers;
mod messaging;
mod navigation;
mod render;
mod workspace;

use workspace::{build_workspace, subscribe_workspace};
#[cfg(test)]
mod tests;
#[cfg(test)]
mod wired_tests;

use self::chrome::{corner_cap, joinable, shadow};
use self::demo::demo;
#[cfg(test)]
pub(crate) use self::dialogs::profile::detect_locked_profile_fields;
use self::helpers::{
    broadcast_kind, first_conversation, forget_failure, is_catchup_notification_tag,
    is_suspicious_url, mentions_me, parse_notification_target, parse_permalink,
    sync_launch_at_login, workspace_badge_from_counts,
};
pub(crate) use self::messaging::{
    MessageSurface, MessageTarget, broadcast_autocomplete_hint, broadcast_guard, confirm_broadcast,
    mention_permission_summary,
};
#[cfg(test)]
pub(crate) use self::messaging::{evaluate_broadcast_guard, matching_slash_candidates};

/// The per-workspace views: rebuilt from that workspace's fixtures, parked while another
/// workspace is shown.
struct WorkspaceViews {
    sidebar: Entity<Sidebar>,
    messages: Entity<MessageList>,
    thread: Entity<ThreadPanel>,
    results: Entity<SearchView>,
    browser: Entity<ChannelBrowser>,
    profile: Entity<ProfilePanel>,
    switcher: Entity<QuickSwitcher>,
}

/// Channels a lazily loaded per-channel cache keeps (pins, bookmarks, files, member lists):
/// past it the least recently loaded channel is dropped, fetched again when one of its
/// surfaces next opens.
const CHANNEL_CACHE_MAX: usize = 32;
/// Documents a workspace keeps parsed (canvases with their comments and details, Lists, App
/// Homes): heavier than a channel's lists, so fewer; one dropped loads again when it reopens.
const DOC_CACHE_MAX: usize = 8;

/// A cache bounded to its `CAP` most recently loaded keys.
pub(super) struct RecentCache<K, V, const CAP: usize> {
    entries: HashMap<K, V>,
    /// The cached keys, least recently loaded first.
    lru: Vec<K>,
}

/// A per-channel cache bounded to the [`CHANNEL_CACHE_MAX`] most recently loaded channels.
pub(super) type ChannelCache<V> = RecentCache<ChannelId, V, CHANNEL_CACHE_MAX>;

/// A per-document cache bounded to the [`DOC_CACHE_MAX`] most recently loaded documents.
pub(super) type DocCache<K, V> = RecentCache<K, V, DOC_CACHE_MAX>;

impl<K, V, const CAP: usize> Default for RecentCache<K, V, CAP> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            lru: Vec::new(),
        }
    }
}

impl<K: Clone + Eq + std::hash::Hash, V, const CAP: usize> RecentCache<K, V, CAP> {
    fn get<Q: std::hash::Hash + Eq + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: std::borrow::Borrow<Q>,
    {
        self.entries.get(key)
    }

    fn get_mut<Q: std::hash::Hash + Eq + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: std::borrow::Borrow<Q>,
    {
        self.entries.get_mut(key)
    }

    fn contains_key<Q: std::hash::Hash + Eq + ?Sized>(&self, key: &Q) -> bool
    where
        K: std::borrow::Borrow<Q>,
    {
        self.entries.contains_key(key)
    }

    /// Keep `key`'s freshly loaded `value` as the most recent entry, dropping the least
    /// recently loaded key past `CAP`.
    fn insert(&mut self, key: K, value: V) {
        self.lru.retain(|known| *known != key);
        self.lru.push(key.clone());
        self.entries.insert(key, value);
        while self.lru.len() > CAP {
            let oldest = self.lru.remove(0);
            self.entries.remove(&oldest);
        }
    }

    fn remove<Q: std::hash::Hash + Eq + ?Sized>(&mut self, key: &Q)
    where
        K: std::borrow::Borrow<Q>,
    {
        self.lru
            .retain(|known| std::borrow::Borrow::<Q>::borrow(known) != key);
        self.entries.remove(key);
    }

    fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter()
    }

    fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.entries.values_mut()
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Per-workspace live state swapped atomically when switching workspaces.
#[derive(Default)]
pub(super) struct WorkspaceState {
    /// Live mode: Slack's latest read and unread state (`client.counts`).
    pub(super) counts: HashMap<ChannelId, ConversationCounts>,
    /// Highest message ts marked read locally per conversation in this session, so a stale
    /// in-flight `client.counts` snapshot cannot resurrect unreads on a conversation already viewed.
    pub(super) marked_read: HashMap<ChannelId, Ts>,
    /// Live mode: last known presence per DM peer (`users.getPresence`, `presence_change`);
    /// injected into every live fixtures snapshot so sidebar rows and headers render dots.
    pub(super) presence: fixtures::PresenceMap,
    /// Live mode: resolved bot profiles (`bots.info`) shared across every snapshot.
    pub(super) bots: fixtures::BotProfileMap,
    /// Live mode: DM peers whose notifications are paused right now (`dnd.info`).
    pub(super) peer_dnd_paused: HashMap<UserId, bool>,
    /// Live mode: active `@handle` user groups from `usergroups.list`, injected into snapshots.
    pub(super) usergroups: Vec<Usergroup>,
    /// Live mode: workspace slash commands from `client.appCommands`, injected into snapshots.
    pub(super) app_commands: Vec<slackless_core::api::AppCommand>,
    /// Per-conversation visit counts for Cmd+K Quick Switcher frecency ranking.
    pub(super) visits: HashMap<ChannelId, u32>,
    /// Live huddles currently running in this workspace (`sh_room_*` socket updates): channel -> participant user ids.
    pub(super) active_huddles: HashMap<ChannelId, Vec<UserId>>,
    /// Live mode: latest RTM websocket connection state (drives the offline/reconnecting banner).
    pub(super) connection: Option<slackless_core::rtm::ConnectionState>,
    /// Live mode: outgoing mutations buffered while offline, waiting to flush on reconnect.
    pub(super) offline_queued: usize,
    /// Live mode: offline mutations that failed to flush and await user Retry or Discard.
    pub(super) failed_mutations: Vec<slackless_core::FailedMutation>,
    /// Live mode: durable V13 send outbox items (queued, sending, or failed) for this workspace.
    pub(super) outbox: Vec<slackless_core::store::OutboxItem>,
    /// Member counts from open-time `conversations.info`, newer than the snapshot's.
    pub(super) member_counts: HashMap<ChannelId, u32>,
    /// Apps among a channel's members, counted apart from people as Slack's header does.
    pub(super) app_counts: HashMap<ChannelId, u32>,
    /// Cached member lists of the most recently opened channels from `Event::ConversationInfo`
    /// so returning to a Slack Connect channel renders its amber banner on frame 0 without
    /// waiting on a refetch.
    pub(super) members_cache: ChannelCache<Vec<UserId>>,
    /// Live mode: channels whose stored history has no older page left; the transcript
    /// stops asking for them.
    pub(super) history_complete: HashSet<ChannelId>,
    /// The user opened a conversation themselves; until then live reloads open the one most
    /// recently read in Slack.
    pub(super) chosen: bool,
    /// Live mode: what the next debounced reload re-reads from the store, cleared once its
    /// read lands. It is parked with the workspace, so a reload a switch interrupted (a first
    /// visit's snapshot among them) resumes when the workspace is shown again.
    pub(super) reload: Option<Reload>,
    /// Live mode: per recently loaded channel, its pinned messages (`pins.list`) or why they
    /// could not be loaded; fetched each time a pins surface opens, never per channel.
    pub(super) pins: ChannelCache<Loaded<Message>>,
    /// Live mode: per recently loaded channel, its bookmarks (`bookmarks.list`), fetched each
    /// time a bookmarks surface opens.
    pub(super) bookmarks: ChannelCache<Loaded<ChannelBookmark>>,
    /// Live mode: per recently loaded channel, its shared files (`files.list`), fetched each
    /// time Files & links opens.
    pub(super) files: ChannelCache<Loaded<File>>,
    /// Live mode: parsed read-only canvases keyed by file id (`files.info` -> `url_private`, `F-COLLAB-125`).
    pub(super) canvases: DocCache<String, Result<crate::canvas::CanvasDoc, String>>,
    /// Live mode: parsed `CanvasMeta` keyed by file id (`Event::CanvasMetaLoaded`).
    pub(super) canvas_metas: DocCache<String, slackless_core::api::CanvasMeta>,
    /// Live mode: threaded comments keyed by canvas file id (`Event::CanvasCommentsLoaded`).
    pub(super) canvas_comments: DocCache<String, Vec<slackless_core::api::CanvasCommentThread>>,
    /// Live mode: inline status or error banner per canvas id (`(is_error, message)`).
    pub(super) canvas_status: HashMap<String, (bool, String)>,
    /// Live mode: "Canvas updated" notice per canvas id (`F-COLLAB-185`).
    pub(super) canvas_updated_notes: HashMap<String, String>,
    /// File id of the live canvas currently displayed in `ConversationTab::Canvas`.
    pub(super) active_canvas: Option<String>,
    /// Live mode: parsed read-only Slack Lists keyed by file id (`files.info` + `lists.records.list`).
    pub(super) lists: DocCache<String, Result<slackless_core::api::SlackListData, String>>,
    /// File id of the live Slack List currently displayed in `ConversationTab::List`.
    pub(super) active_list: Option<String>,
    /// Live mode: per app DM channel, its App Home metadata and Block Kit view (`F-COLLAB-480`).
    pub(super) app_homes: DocCache<ChannelId, Result<slackless_core::api::AppHomeResponse, String>>,
    /// Deduplicates `subscriptions.thread.mark` calls: last cursor sent per `(channel, thread_ts)`.
    pub(super) marked_threads: HashMap<(ChannelId, Ts), Ts>,
    /// Live archive backfill progress snapshot for this workspace (`Event::BackfillProgress`).
    pub(super) backfill_progress: Option<slackless_core::WorkspaceBackfillProgress>,
    /// The notification levels its prefs set, read once for the messages that arrive while
    /// another workspace is shown (a shown snapshot carries its own); dropped on a sidebar or
    /// conversations update, which a changed level brings.
    pub(super) notify_levels: Option<HashMap<ChannelId, NotifyLevel>>,
}

/// How a posted message moves its conversation's unread state.
pub(super) enum Arrival {
    /// The user's own message: the conversation is read up to it.
    Own,
    /// Read as it arrived (the open conversation, marked read as shown).
    Seen,
    /// Unread: `mention` bumps the mention badge, `im` the DM count.
    Unread { mention: bool, im: bool },
}

impl WorkspaceState {
    /// Fold Slack's read marker (`*_marked`, from here or another device) for `channel` at `ts`
    /// into the counts. A DM left unread (`dm`) keeps a mention, as Slack badges it. Returns
    /// whether the conversation is read now.
    pub(super) fn fold_read_marker(&mut self, channel: &ChannelId, ts: &Ts, dm: bool) -> bool {
        let entry = counts_entry(&mut self.counts, channel);
        entry.last_read = Some(ts.clone());
        let read = entry.latest.as_ref().is_none_or(|latest| ts >= latest);
        if read {
            entry.clear_unreads();
            if self.marked_read.get(channel).is_none_or(|prev| ts > prev) {
                self.marked_read.insert(channel.clone(), ts.clone());
            }
        } else {
            entry.has_unreads = true;
            if dm && entry.effective_mentions() == 0 {
                entry.mention_count = 1;
            }
            self.marked_read.remove(channel);
        }
        read
    }

    /// Fold a message posted in `channel` at `ts` into the counts.
    pub(super) fn fold_message(&mut self, channel: &ChannelId, ts: &Ts, arrival: Arrival) {
        let entry = counts_entry(&mut self.counts, channel);
        if entry.latest.as_ref().is_none_or(|latest| ts > latest) {
            entry.latest = Some(ts.clone());
        }
        match arrival {
            Arrival::Own => {
                if entry.last_read.as_ref().is_none_or(|read| ts > read) {
                    entry.last_read = Some(ts.clone());
                }
            }
            Arrival::Seen => {}
            Arrival::Unread { mention, im } => {
                entry.has_unreads = true;
                if mention {
                    entry.mention_count = entry.mention_count.saturating_add(1);
                }
                if im {
                    entry.dm_count = entry.dm_count.saturating_add(1);
                }
                if self
                    .marked_read
                    .get(channel)
                    .is_some_and(|marked| ts > marked)
                {
                    self.marked_read.remove(channel);
                }
            }
        }
    }
}

/// `channel`'s entry in `counts`, empty until Slack counts it.
fn counts_entry<'a>(
    counts: &'a mut HashMap<ChannelId, ConversationCounts>,
    channel: &ChannelId,
) -> &'a mut ConversationCounts {
    counts
        .entry(channel.clone())
        .or_insert_with(|| ConversationCounts {
            id: channel.clone(),
            ..Default::default()
        })
}

/// A visited workspace that is not shown: its fixtures, open conversation, views and
/// subscriptions.
struct Parked {
    fixtures: Arc<Fixtures>,
    channel: ChannelId,
    views: WorkspaceViews,
    subscriptions: Vec<Subscription>,
}

/// One workspace at the slot it keeps for the whole run: drafts, Later items, notification
/// tags and core subscriptions name a workspace by its slot, never by its place on the rail.
/// While the workspace is shown its bridge and state live in [`App::bridge`] and [`App::ws`].
struct WorkspaceSlot {
    /// Name, domain and icon, refreshed from the store's `team.info` record.
    team: Team,
    /// The live core while another workspace is shown (`None` for sample workspaces).
    bridge: Option<CoreBridge>,
    /// The views, once visited, while another workspace is shown.
    parked: Option<Parked>,
    /// The live state (counts, presence, connection, ...) while another workspace is shown.
    state: WorkspaceState,
    /// The Activity feed's unread count while another workspace is shown.
    activity_unread: u32,
}

/// How long a live reload waits for more events, so a burst (boot hydration) reads the store
/// once.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(40);

/// What a live reload re-reads from the store.
pub(super) enum Reload {
    /// The whole workspace snapshot.
    Full,
    /// Only these people (a status or profile change), patched into the shown snapshot.
    Users(HashSet<UserId>),
}

impl Reload {
    /// This pending reload with `next` folded in; a full one already re-reads everyone.
    fn merge(self, next: Reload) -> Reload {
        match (self, next) {
            (Reload::Users(mut users), Reload::Users(more)) => {
                users.extend(more);
                Reload::Users(users)
            }
            _ => Reload::Full,
        }
    }
}

/// Where an emoji pick goes.
#[derive(Clone)]
enum PickerTarget {
    Composer,
    ThreadComposer,
    ThreadsComposer,
    ComposeView,
    MessageEdit,
    ThreadEdit,
    Message { channel: ChannelId, ts: Ts },
    ThreadMessage(Ts),
}

/// A destination in the Back / Forward navigation history (title-bar arrows, `Cmd+[`/`Cmd+]`,
/// and mouse Back/Forward buttons).
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum NavPlace {
    Channel(ChannelId),
    CatchUp,
    Threads,
    Unreads,
    Huddles,
    Drafts,
    Saved,
    Directories,
    Activity,
    Files,
    Tools,
}

/// Which top-level page currently owns the main column (mutually exclusive).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum MainPage {
    #[default]
    Channel,
    CatchUp,
    Search,
    Drafts,
    Later,
    Threads,
    Unreads,
    Huddles,
    Directory,
    Tools,
    Compose,
    Activity,
    Files,
}

/// Which right-hand panel is open (mutually exclusive). `Profile { over_thread: true }`
/// remembers an open thread underneath so clicking Back restores the thread.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum RightPane {
    #[default]
    None,
    Thread,
    Profile {
        over_thread: bool,
    },
    Pins,
}

impl RightPane {
    pub(super) fn is_open(self) -> bool {
        !matches!(self, Self::None)
    }

    pub(super) fn is_thread(self) -> bool {
        matches!(self, Self::Thread)
    }

    #[cfg(test)]
    pub(super) fn is_profile(self) -> bool {
        matches!(self, Self::Profile { .. })
    }

    pub(super) fn is_pins(self) -> bool {
        matches!(self, Self::Pins)
    }
}

pub struct App {
    /// The shown workspace's fixtures.
    fixtures: Arc<Fixtures>,
    bridge: CoreBridge,
    /// Every workspace, by slot (at least one).
    slots: Vec<WorkspaceSlot>,
    /// The rail's order, top to bottom, as slots: the tiles, Cmd+1…9, the menus and the menu
    /// bar item list workspaces in this order, and the snapshots' `workspaces` follow it.
    rail: Vec<usize>,
    /// Slot of the shown workspace.
    workspace: usize,
    /// Active workspace's per-workspace live state (swapped with its slot's on a switch).
    ws: WorkspaceState,
    channel: ChannelId,
    sidebar: Entity<Sidebar>,
    messages: Entity<MessageList>,
    composer: Entity<Composer>,
    thread: Entity<ThreadPanel>,
    /// The global search field in the title bar.
    search: Entity<TextareaState>,
    /// The search pill's dropdown; open while `search_menu_open`.
    search_menu: Entity<SearchMenu>,
    search_menu_open: bool,
    /// Results of the last search; shown in place of the message column when `page == MainPage::Search`.
    results: Entity<SearchView>,
    /// The channel browser, mounted in a dialog on demand.
    browser: Entity<ChannelBrowser>,
    /// Which top-level page currently owns the main column.
    page: MainPage,
    /// Which right-hand flexpane is currently open (thread, profile, or pins).
    right_pane: RightPane,
    tab: ConversationTab,
    /// Active native audio huddle session snapshot (Chime SFU).
    huddle_session: Option<slackless_core::huddle::HuddleSessionSnapshot>,
    /// Slot of the workspace running the huddle, set with the dock: its core gets every huddle
    /// command and its session events drive the dock, whichever workspace is shown.
    huddle_owner: Option<usize>,
    /// Shared huddle UI state observed by `HuddleDock`, `PopoutKind::Huddle`, and `HuddleFloatingPanel`.
    huddle_state: Option<Entity<HuddleState>>,
    _huddle_state_sub: Option<Subscription>,
    /// Persistent sidebar mini-dock mounted at the bottom of the sidebar while in a huddle.
    huddle_dock: Option<Entity<HuddleDock>>,
    huddle_floating_panel: Option<gpui::AnyWindowHandle>,
    _huddle_dock_sub: Option<Subscription>,
    /// Floating incoming DM huddle ring banner (`huddle_invite`).
    huddle_invite: Option<Entity<HuddleInviteBanner>>,
    _huddle_invite_sub: Option<Subscription>,
    /// Official Slack 60-second solo-huddle idle music timer (`huddles_play_music_timeout` = 60s).
    huddle_alone_music_task: Option<gpui::Task<()>>,
    /// The Preferences window body, mounted in a dialog on demand; owns the settings.
    preferences: Entity<Preferences>,
    /// Standalone native Preferences OS window handle (`F-CORE-011`) so Cmd+, focuses it if open.
    prefs_window: Option<gpui::AnyWindowHandle>,
    /// The theme last installed (mode, color theme, darker sidebars, window gradient, focus
    /// mode's grayscale), so unrelated preference edits do not repaint the theme.
    applied_theme: (SlackTheme, Palette, bool, bool, bool),
    /// Current pane index for F6 / Shift+F6 focus cycling (0=sidebar, 1=transcript, 2=composer, 3=right).
    pane_focus: usize,
    /// The Color Mode last stored (`None` offline), so only a changed mode is written.
    color_mode: Option<AppearanceMode>,
    /// The full Preferences snapshot last persisted (`None` offline), avoiding redundant writes.
    saved_settings: Option<crate::preferences::Settings>,
    /// The profile pane; shown in the right panel when `right_pane` is `RightPane::Profile`.
    profile: Entity<ProfilePanel>,
    /// Last right-hand panel hidden by `ToggleRightPane` (`Cmd+.`), restored on the next toggle.
    last_right_pane: Option<RightPane>,
    /// The signed-in user's status, presence and notification pause.
    self_state: SelfState,
    /// True while macOS Calendar sync set an automatic `"In a meeting"` status (`F-SHELL-024`).
    calendar_status_active: bool,
    /// Poll counter (10s ticks) so Calendar.app is probed at most once per minute.
    calendar_sync_ticks: u32,
    status_picker: Entity<StatusPicker>,
    user_menu: Entity<UserMenu>,
    user_menu_open: bool,
    switcher: Entity<QuickSwitcher>,
    /// The emoji picker overlay and, while it is open, its target and anchor point.
    emoji_picker: Entity<EmojiPicker>,
    picker: Option<(PickerTarget, Point<Pixels>)>,
    /// Slack's member profile hover card (hover on an @mention pill or an author name): the
    /// trigger under the pointer, the open card and the pending dwell / grace timers
    /// (trigger reports arrive through `hover_card`'s router; a dropped timer cancels).
    profile_hover: hover_card::Lifecycle,
    /// The focused message's reminder menu (Slack's M key): the open menu, its anchor
    /// point and its dismiss subscription; `None` keeps it closed.
    remind_menu: Option<(Entity<PopupMenu>, Point<Pixels>, Subscription)>,
    /// The image viewer overlay (a message image was clicked); `None` keeps it closed.
    image_viewer: Option<Entity<ImageViewer>>,
    /// Subscription for the active `image_viewer`; dropped on close/replace to avoid leaking.
    _viewer_sub: Option<Subscription>,
    /// Drafts, scheduled and sent messages; shown when `page == MainPage::Drafts`.
    drafts: Entity<DraftsView>,
    /// Saved messages and reminders; shown when `page == MainPage::Later`.
    later: Entity<LaterView>,
    /// The Threads view; shown when `page == MainPage::Threads`.
    threads: Entity<ThreadsView>,
    /// The All Unreads view (Slack's `p-unreads_view`); shown when `page == MainPage::Unreads`.
    unreads: Entity<UnreadsView>,
    /// Local Catch-Up / Daily Briefing view; shown when `page == MainPage::CatchUp`.
    catchup: Entity<CatchUpView>,
    /// Monotonic generation guarding background Catch-Up briefing builds against stale writes.
    catchup_generation: u64,
    /// Channels already asked for a single-page history fetch from `refresh_unreads`, so an
    /// empty unread channel is fetched at most once per session.
    unreads_fetched: HashSet<ChannelId>,
    /// Live mode: the Unreads view's store reads - one in flight, the refreshes asked for
    /// meanwhile folded into one more, an overtaken read dropped.
    unreads_refresh: LiveRefresh,
    /// The Activity page (the rail's Activity destination): a full-width page right of the
    /// rail - the sidebar's list column hides while `page == MainPage::Activity`.
    activity: Entity<ActivityFeed>,
    /// The Files view (the rail's Files destination): the main-area page beside the
    /// sidebar's Files list column, shown while `page == MainPage::Files`.
    files_page: Entity<FilesPage>,
    /// The Huddles page; shown when `page == MainPage::Huddles`.
    huddles: Entity<HuddlesView>,
    /// The directory; shown when `page == MainPage::Directory`.
    directory: Entity<DirectoryView>,
    /// Native `Agents & tools` view (Apps, Slash commands, Slack Connect); shown when `page == MainPage::Tools`.
    tools_page: Entity<crate::tools_page::ToolsPage>,
    /// The "New message" view; shown when `page == MainPage::Compose`.
    compose: Entity<NewMessageView>,
    /// The channel details dialog's body, mounted in a dialog on demand.
    details: Entity<ChannelDetails>,
    /// Live: the open channel's member ids as the core last announced them
    /// (`Event::ConversationInfo`), for the details dialog and the Slack Connect banner. Only
    /// the open channel's list is kept: every open fetches it again.
    open_members: Option<(ChannelId, Vec<UserId>)>,
    /// The header members chip's hover: (chip, popover) - Slack keeps the tooltip while the
    /// pointer is over either.
    members_hover: (bool, bool),
    /// The open channel's people as Slack names them: its most recent human authors, topped
    /// up from its members (the members popover's "Includes …" line, the Join card's faces
    /// and names). Computed when one of those needs them and after the channel or its history
    /// changes - never per frame. `None` until then.
    channel_people: Option<Vec<UserId>>,
    /// The connect banner's lines for the open conversation, recomputed when its members or
    /// the snapshot change - never per frame.
    connect_orgs: Vec<channel_details::OrgLine>,
    /// A New-message send waiting for its DM to open ([`ChannelCommand::OpenDm`]); delivered
    /// only to the DM with the picked users.
    pending_dm: Option<messaging::PendingDm>,
    /// In-canvas Find field (`F-SHELL-266`).
    canvas_search: Entity<TextareaState>,
    /// Current in-canvas Find query.
    canvas_query: String,
    /// In-list Filter field for the native Slack Lists viewer.
    list_search: Entity<TextareaState>,
    /// Current in-list filter query.
    list_query: String,
    /// Active `(column_index, ascending)` sort for the native Slack Lists viewer.
    list_sort: Option<(usize, bool)>,
    /// Interactive Block Kit widget state (selects, checkboxes, inputs) for App Home tabs.
    app_home_kit: Entity<crate::blockkit::BlockKitState>,
    /// Multi-view app modal stack (`view_opened` / `view_updated` / `view_closed` / `views.submit`).
    modal_stack: Entity<dialogs::modal::AppModalStack>,
    /// Currently open Workflow Details dialog (`workflows.triggers.info`), if any.
    /// Held weakly so the dialog layer owns its lifetime and closing the dialog by
    /// any path (Close button, Run button, Escape, backdrop, or `close_dialog`)
    /// releases the entity automatically.
    workflow_details: Option<gpui_kit::gpui::WeakEntity<dialogs::workflow::WorkflowDetailsView>>,
    /// Fixture mode: link bookmarks added through the add-link dialog, per channel.
    fixture_bookmarks: HashMap<ChannelId, Vec<bookmarks::Bookmark>>,
    /// Dock badge + native notifications + menu-bar status item (`slackless-platform`).
    os: Rc<dyn DesktopPlatform>,
    /// Numeric badge count for window title ("N new items").
    badge: Option<u32>,
    /// The badge label last handed to the dock, so it is only set on change.
    badge_label: Option<Option<String>>,
    /// The last status-bar state handed to the OS menu bar, to avoid redundant updates.
    last_status_bar: Option<StatusBarState>,
    /// Whether the main window is currently active/focused (guards background mark-as-read).
    window_active: bool,
    /// Recently dispatched desktop notification tags (`F-SHELL-181` duplicate-notification guard).
    recent_notified_tags: Vec<(String, Instant)>,
    /// Main window handle, used to update the OS window title on state changes outside `render`.
    window_handle: gpui::AnyWindowHandle,
    /// The window title last set, so it is only set on change.
    window_title: String,
    /// The pending, debounced save of the window frame (replacing it cancels the last one).
    window_state_save: gpui::Task<()>,
    /// Live mode: the newest history read started; an overtaken read is dropped.
    history_generation: u64,
    /// Live mode: the newest thread read started; an overtaken read is dropped.
    thread_generation: u64,
    /// Live mode: the newest debounced store reload started (a snapshot or a people patch);
    /// coalesces event bursts.
    reload_generation: u64,
    /// Debounced store reload handle; replacing cancels pending reads during hydration bursts.
    reload_task: gpui::Task<()>,
    /// Places navigated away from, newest last (Slack's back arrow, Cmd+[, mouse Back).
    back: Vec<NavPlace>,
    /// Places stepped back from, newest last (the forward arrow, Cmd+], mouse Forward).
    forward: Vec<NavPlace>,
    /// Recently left conversations, newest first (the history clock's "Recent" menu).
    recent: Vec<ChannelId>,
    /// What the menu bar shows, so it is rebuilt only when that changes.
    menu_bar: Option<MenuBar>,
    typing: Typing,
    /// Wakes when the next typing announcement runs out.
    typing_expiry: gpui::Task<()>,
    /// The notification sound last fetched: its Preferences choice and bytes.
    sound: Option<(usize, Vec<u8>)>,
    /// A notification sound being fetched.
    sound_fetch: gpui::Task<()>,
    /// The sound Preferences last applied: picking another plays it, as in Slack.
    sound_choice: Option<usize>,
    /// Root focus target, so Cmd+K / Cmd+, reach the root with nothing else focused.
    focus: FocusHandle,
    /// In-app GitHub Releases auto-updater state (`F-SHELL-163`).
    update_state: crate::updater::UpdateState,
    /// Latest detected update kept available for the menu bar (`Download Update (<ver>)…`) even
    /// if the user dismisses the top banner.
    available_update: Option<crate::updater::ReleaseInfo>,
    /// Version tag dismissed by the user for the current session.
    update_dismissed: Option<String>,
    /// Subscriptions to the shown workspace's views (parked with them on a switch).
    workspace_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}
