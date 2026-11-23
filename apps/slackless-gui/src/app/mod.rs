//! Root view: title bar over a resizable layout of sidebar | message column + composer |
//! optional thread panel (gpui-kit-guide sections 1-3).
//!
//! MODULE INTERFACES - do not change
//! ==================================
//! Every view is an `Entity` created by its own constructor; `cx` is `gpui::App` (a
//! `&mut Context<_>` coerces to it). Views talk upward only through their event enum, which the
//! root subscribes to; the root talks downward through the listed `&mut self` methods.
//! New methods may be added; these signatures and events stay as they are.
//!
//! fixtures.rs
//!   pub struct Fixtures { pub workspaces: Vec<Team>, pub self_user: UserId, pub users: Vec<User>,
//!       pub channels: Vec<Conversation>, pub dms: Vec<Conversation>,
//!       pub messages: HashMap<ChannelId, Vec<Message>>,
//!       pub threads: HashMap<(ChannelId, Ts), Vec<Message>> }
//!   pub fn load() -> Fixtures                       (sample workspace, `sample: true`)
//!   pub fn live(&Session, &Store) -> Fixtures         (signed-in workspace from the store)
//!   Fixtures::user_name(&self, &UserId) -> &str
//!   Fixtures::conversation(&self, &ChannelId) -> Option<&Conversation>
//!   Fixtures::conversation_label(&self, &Conversation) -> String
//!   Fixtures::local_message(&self, &ChannelId, text: String, thread_ts: Option<Ts>) -> Message
//!
//! bridge.rs
//!   CoreBridge::offline() -> CoreBridge
//!   CoreBridge::start(Session, &mut App) -> Option<CoreBridge>
//!   CoreBridge::send(&self, slackless_core::Command)
//!   CoreBridge::snapshot(&self) -> Option<Fixtures>   (live: fresh store snapshot)
//!   CoreBridge::history(&self, &ChannelId, Option<Ts>) -> Option<Vec<Message>>
//!       (live: stored history; `Some(ts)` pages older than `ts`)
//!   CoreBridge::thread(&self, &ChannelId, &Ts) -> Option<Vec<Message>>   (live: stored thread)
//!   CoreBridge::subscribe_in(&mut self, &mut Window, &mut Context<T>, handler) -> Subscription
//!
//! sign_in.rs
//!   pub const FLAG; pub fn run_helper() -> i32; pub fn session() -> Option<Session>
//!
//! sidebar.rs
//!   Sidebar::new(Arc<Fixtures>, selected: ChannelId, &mut Window, &mut App) -> Entity<Sidebar>
//!   Sidebar::set_selected(&mut self, ChannelId, &mut Context<Self>)
//!   Sidebar::add_channel(&mut self, Conversation, &mut Context<Self>)
//!   emits SidebarEvent::SelectConversation(ChannelId)
//!   emits SidebarEvent::BrowseChannels
//!
//! browser.rs (mounted in a gpui-kit dialog)
//!   ChannelBrowser::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<ChannelBrowser>
//!   emits ChannelBrowserEvent::Join(ChannelId) / ChannelBrowserEvent::Open(ChannelId)
//!
//! search.rs (replaces the message column while a search is shown)
//!   SearchView::new(Arc<Fixtures>, Option<SharedStore>, &mut App) -> Entity<SearchView>
//!   SearchView::run(&mut self, query: String, &mut Context<Self>)
//!   emits SearchEvent::Close / SearchEvent::Open(ChannelId)
//!   pub fn fixture_archive(&Fixtures) -> Option<SharedStore>
//!
//! avatars.rs / files.rs: shared render helpers (presence + group avatars, attachments).
//!
//! canvas.rs / lists.rs (read-only; the Canvas / List tab bodies)
//!   pub fn canvas_view(&App) -> Stateful<Div>
//!   pub fn list_view(&App) -> Stateful<Div>
//!
//! huddle.rs (UI only; media deferred)
//!   pub fn fixture(&Fixtures) -> Option<Huddle>
//!   HuddleBar::new(Arc<Fixtures>, Huddle, &mut App) -> Entity<HuddleBar>
//!   emits HuddleEvent::Leave
//!   Sidebar::set_huddle(&mut self, Option<ChannelId>, &mut Context<Self>)
//!
//! messages.rs
//!   MessageList::new(Arc<Fixtures>, channel: ChannelId, &mut Window, &mut App)
//!       -> Entity<MessageList>
//!   MessageList::set_conversation(&mut self, ChannelId, &mut Context<Self>)
//!   MessageList::append(&mut self, ChannelId, Message, &mut Context<Self>)
//!   MessageList::set_history(&mut self, ChannelId, Vec<Message>, &mut Context<Self>)
//!   MessageList::prepend_history(&mut self, ChannelId, Vec<Message>, &mut Context<Self>)
//!       (an older page; the visible row keeps its screen position)
//!   emits MessageListEvent::OpenThread { channel: ChannelId, thread_ts: Ts }
//!   emits MessageListEvent::PickReaction { channel: ChannelId, ts: Ts, position: Point<Pixels> }
//!   emits MessageListEvent::NeedOlder { channel: ChannelId, before: Ts }   (top of list)
//!   MessageList::add_reaction(&mut self, &ChannelId, &Ts, name: &str, &mut Context<Self>)
//!   pub fn message_row(&Fixtures, &Message, &App) -> Div   (shared with threads.rs)
//!
//! composer.rs (WYSIWYG draft; the editing model is in editor/)
//!   Composer::new(placeholder: SharedString, Arc<Fixtures>, &mut Window, &mut App)
//!   emits ComposerEvent::Submit(MessageContent)   (rich_text block + mrkdwn fallback; never
//!     blank; composer already cleared)
//!   emits ComposerEvent::OpenEmojiPicker(Point<Pixels>)
//!   emits ComposerEvent::EditLast   (Up in an empty draft)
//!   Composer::insert_emoji(&mut self, &EmojiChoice, &mut Window, &mut Context<Self>)
//!   Composer::set_fixtures(&mut self, Arc<Fixtures>, &mut Context<Self>)
//!   pub fn content_of(draft: &str, &Fixtures) -> MessageContent   (a mrkdwn draft as posted)
//!
//! emoji.rs (overlay mounted by the root, anchored at the requesting click)
//!   EmojiPicker::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<EmojiPicker>
//!   EmojiPicker::open(&mut self, &mut Window, &mut Context<Self>)
//!   emits EmojiPickerEvent::Pick(EmojiChoice { name, glyph: Option<String> })
//!
//! actions.rs / blockkit.rs: shared message actions (hover bar, menu, chips, edit box) and
//!   Block Kit + legacy attachment rendering, used by messages.rs and threads.rs.
//!
//! threads.rs
//!   ThreadPanel::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<ThreadPanel>
//!   ThreadPanel::open(&mut self, ChannelId, thread_ts: Ts, &mut Context<Self>)
//!   ThreadPanel::open_thread(&self) -> Option<(ChannelId, Ts)>
//!   ThreadPanel::set_thread(&mut self, ChannelId, Ts, Vec<Message>, &mut Context<Self>)   (live)
//!   emits ThreadPanelEvent::Close
//!   emits ThreadPanelEvent::Reply { channel: ChannelId, thread_ts: Ts, content: MessageContent,
//!       broadcast: bool }   (reply already shown in the panel; `broadcast`: "Also send to")
//!   emits ThreadPanelEvent::PickReaction { ts: Ts, position } / ThreadPanelEvent::PickEmoji(position)
//!   emits ThreadPanelEvent::SlashCommand(Invocation)   (commands other than /shrug)
//!   ThreadPanel::add_reaction(&mut self, &Ts, name: &str, &mut Context<Self>)
//!   ThreadPanel::insert_emoji(&mut self, &EmojiChoice, &mut Window, &mut Context<Self>)
//!
//! composer.rs / threads.rs (Preferences > Messages & media)
//!   Composer::set_enter_sends(&mut self, bool, &mut Context<Self>)
//!   ThreadPanel::set_enter_sends(&mut self, bool, &mut Context<Self>)
//!
//! preferences.rs (mounted in a gpui-kit dialog; settings in memory only)
//!   Preferences::new(&mut Window, &mut App) -> Entity<Preferences>
//!   Preferences::settings(&self) -> &Settings
//!   Preferences::show(&mut self, Section, &mut Context<Self>)
//!   Preferences::set_appearance_mode(&mut self, AppearanceMode, &mut Context<Self>)
//!   emits PreferencesEvent::Changed
//!
//! profile.rs (right-hand panel, exclusive with the thread panel)
//!   ProfilePanel::new(Arc<Fixtures>, &mut App) -> Entity<ProfilePanel>
//!   ProfilePanel::open(&mut self, UserId, back: bool, &mut Context<Self>)
//!   ProfilePanel::set_self_state(&mut self, SelfState, &mut Context<Self>)
//!   emits ProfileEvent::Close / ProfileEvent::Message(UserId) / ProfileEvent::EditStatus
//!
//! status.rs (status, presence, DND; the root owns the SelfState)
//!   StatusPicker::new(&mut Window, &mut App) -> Entity<StatusPicker>   (in a dialog)
//!   StatusPicker::edit(&mut self, Option<&UserStatus>, &mut Window, &mut Context<Self>)
//!   emits StatusEvent::Save(Option<UserStatus>) / StatusEvent::Cancel
//!   UserMenu::new(Arc<Fixtures>, &mut App) -> Entity<UserMenu>   (title-bar avatar menu)
//!   UserMenu::set_state(&mut self, SelfState, schedule: String, &mut Context<Self>)
//!   emits UserMenuEvent
//!   pub fn self_avatar(&Fixtures, &SelfState, size, ring: Hsla, &App) -> Div
//!
//! switcher.rs (Cmd+K, in a dialog)
//!   QuickSwitcher::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<QuickSwitcher>
//!   QuickSwitcher::open(&mut self, &mut Window, &mut Context<Self>)
//!   emits SwitcherEvent::Open(ChannelId)
//!
//! slash.rs (composer slash commands; dispatched by the root)
//!   pub const BUILTINS; pub fn parse(&str) -> Option<Invocation>
//!   Composer emits ComposerEvent::SlashCommand(Invocation)
//!
//! drafts.rs (Drafts & sent; replaces the message column while open)
//!   DraftsView::new(&mut App) -> Entity<DraftsView>
//!   DraftsView::set_draft / draft / schedule / record_sent
//!   emits DraftsEvent::Open { workspace, channel } / SendNow(Entry) / Close
//!   Composer::text(&self) -> String; Composer::load_draft(&mut self, &str, cx)   (mrkdwn)
//!   Composer emits ComposerEvent::Schedule { text, when: SendLater }
//!
//! Workspaces: fixtures::workspace(index) -> Fixtures; the sidebar emits
//!   SidebarEvent::SwitchWorkspace(index) / SidebarEvent::OpenDrafts and takes
//!   Sidebar::set_workspace / set_draft_count / show_nav. The root parks the views of the
//!   workspace it leaves, so returning restores its state.
//!
//! shortcuts.rs (Cmd+/, in a dialog)
//!   pub fn shortcuts_view(&App) -> Stateful<Div>
//!
//! later.rs (Later: saved messages + reminders; replaces the message column while open)
//!   LaterView::new(&mut App) -> Entity<LaterView>
//!   LaterView::seed / save(SaveRequest, workspace, label) / saved_in / take_due
//!   emits LaterEvent::Open { workspace, channel } / Unsaved { .. } / Changed / Close
//!   MessageList emits MessageListEvent::Later(SaveRequest), ThreadPanel
//!   ThreadPanelEvent::Later(SaveRequest); both take set_saved(&Ts, bool, cx)
//!   SidebarEvent::OpenLater; Sidebar::set_later_count
//!
//! tabs.rs (the conversation tab row, derived from `properties.tabs` in live mode)
//!   pub fn derive(&Conversation, sample: bool) -> Vec<TabSpec> / tab(..) -> Stateful<Div>
//!   TabAction::Switch(ConversationTab) / OpenCanvas(file id) / AddCanvas
//! tab_views.rs (Files & links and bookmark-folder tab bodies over lazily `Loaded` data)
//!   pub fn files_view(..) -> Div / folder_view(..) -> Div / state_line(..)
//!
//! bookmarks.rs (bookmarks bar under the tabs, and its add-link dialog)
//!   pub fn fixture(&Fixtures, &ChannelId) -> Vec<Bookmark> / bar(.., BarClicks, ..) -> Div
//!   pub fn open_add_dialog(&mut Window, &mut App, on_add(title, url, cx))
//!
//! pins.rs (pins panel in the right column)
//!   pub fn pinned(&Fixtures, &ChannelId) / pins_panel(.., Option<&Loaded<Message>>, ..)
//!
//! activity.rs (the Activity page, rendered full-width beside the rail while the rail's
//! Activity destination is active)
//!   ActivityFeed::new(Arc<Fixtures>, &mut App) -> Entity<ActivityFeed>
//!   emits ActivityEvent::Open { channel, thread_ts } (this root opens the conversation,
//!   thread items with the thread panel), Mark { entries, read } (forwarded to the core as
//!   Command::MarkActivity) and Retry (Command::FetchActivity)
//!   Sidebar::badge_count() -> u32 drives the dock badge (slackless_core::platform); the
//!   sidebar's activity part is pushed from the feed's unread rows
//!
//! unreads.rs (the All Unreads view, Slack's `p-unreads_view`; replaces the message column
//! while `unreads_open`)
//!   UnreadsView::new(Arc<Fixtures>, &mut App) -> Entity<UnreadsView>
//!   local_groups / store_groups / store_targets drive it (pure, unit-tested there)
//!   emits UnreadsEvent::OpenConversation(ChannelId) / OpenLink(LinkTarget)
//!   / MarkRead { channel, ts } (the root's mark path) / MarkAllRead (the root's confirm
//!   dialog); SidebarEvent::OpenUnreads opens it
//!
//! files_page.rs (the Files view: the main-area page of the rail's Files destination, beside
//! the sidebar's Files list column)
//!   FilesPage::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<FilesPage>
//!   open(Arc<Fixtures>, cx) / show_section(FilesSection, ..) / land(&FilesFilter, ..)
//!   emits FilesPageEvent::Load(FilesFilter) (forwarded to the core as Command::LoadFiles);
//!   the sidebar emits SidebarEvent::OpenFiles and ShowFilesSection(FilesSection)
//!
//! app.rs
//!   App::new(Arc<Fixtures>, CoreBridge, &mut Window, &mut gpui::App) -> Entity<App>
//!   actions: QuickSwitch (cmd-k), OpenPreferences (cmd-,), ShowShortcuts (cmd-/),
//!   OpenDms (cmd-shift-k), OpenActivity (cmd-shift-m), OpenUnreads (cmd-shift-a),
//!   OpenDrafts (cmd-shift-d),
//!   OpenChannelBrowser (cmd-shift-l), OpenDownloads (cmd-shift-j), SetStatus (cmd-shift-y),
//!   FocusSearch (cmd-g), NavigateBack / NavigateForward (cmd-[ / cmd-]), NewMessage (cmd-n),
//!   OpenThreads (cmd-shift-t), OpenPeople (cmd-shift-e), OpenLater, OpenFiles,
//!   OpenOwnProfile, SwitchWorkspace(index) (cmd-1..9), NextWorkspace / PreviousWorkspace
//!   (cmd-} / cmd-{), SignOut(index), OpenRecent(ChannelId): the menu bar's items
//!   (menu_bar.rs), rebuilt by App::update_menus

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::mem;
use std::rc::Rc;
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Escape as InputEscape, InputEvent, Textarea, TextareaState};
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
use slackless_core::platform::{
    self, AttentionRequester, BadgeSetter, MouseNavDirection, Notifier, SoundPlayer,
    StatusBarAction, StatusBarItem, StatusBarState,
};
use slackless_core::types::rich_text::BroadcastRange;
use slackless_core::types::{
    Block, ChannelId, Conversation, ConversationKind, File, Message, RichTextContainer,
    RichTextElement, Team, Ts, UserId, Usergroup,
};
use slackless_core::{Command, Event, NotifyLevel, SharedStore};

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
use crate::channel_details::{self, ChannelDetails, ChannelDetailsEvent};
use crate::composer::{self, Composer, ComposerEvent, ContextBar, Placeholder, TypingNames};
use crate::directory::{DirectoryEvent, DirectoryView};
use crate::drafts::{DraftsEvent, DraftsView, Entry, now_secs};
use crate::emoji::{EmojiChoice, EmojiPicker, EmojiPickerEvent, PickerTab};
use crate::files_page::{FilesPage, FilesPageEvent, FilesSection};
use crate::fixtures::{self, Fixtures};
use crate::hover_card;
use crate::huddle::{self, HuddleBar, HuddleEvent};
use crate::huddle_dock::HuddleDock;
use crate::huddle_invite::HuddleInviteBanner;
use crate::huddles::{self, HuddlesEvent, HuddlesView};
use crate::image_viewer::{ImageViewer, ViewerEvent, ViewerImage};
use crate::later::{LaterEvent, LaterView, Remind, SaveRequest};
use crate::lists::list_view;
use crate::menu_bar::{self, MenuBar};
use crate::messages::{LinkTarget, MessageList, MessageListEvent};
use crate::new_message::{NewMessageEvent, NewMessageView};
use crate::pins;
use crate::preferences::{AppearanceMode, Preferences, PreferencesEvent, Section};
use crate::profile::{ProfileEvent, ProfilePanel};
use crate::search::{SearchEvent, SearchView, fixture_archive};
use crate::search_menu::{SEARCH_PLACEHOLDER, SearchMenu, SearchMenuEvent, Snapshot};
use crate::shortcuts::shortcuts_view;
use crate::sidebar::{NavItem, Sidebar, SidebarEvent, WorkspaceBadge};
use crate::slash::{Invocation, SHRUG, SlashKind};
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
use crate::unreads::{UnreadsEvent, UnreadsView, local_groups, store_groups, store_targets};

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
        OpenHome,
        OpenLater,
        OpenFiles,
        OpenMore,
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

/// The OS integration the root uses: system notifications, dock badge, attention bounce, sounds and menu-bar icon.
trait Os: Notifier + BadgeSetter + SoundPlayer + StatusBarItem + AttentionRequester {}
impl<T: Notifier + BadgeSetter + SoundPlayer + StatusBarItem + AttentionRequester> Os for T {}

mod chrome;
mod dialogs;
mod events;
mod helpers;
mod messaging;
mod navigation;
#[cfg(test)]
mod tests;

use self::chrome::{corner_cap, joinable, shadow};
use self::helpers::{
    broadcast_kind, civil_from_days, convert_emoticons, demo, first_conversation, forget_failure,
    is_suspicious_url, mentions_me, parse_notification_target, parse_permalink,
    sync_launch_at_login, workspace_badge_from_counts,
};
pub(crate) use self::helpers::{default_custom_datetime, parse_local_schedule};

/// The per-workspace views: rebuilt from that workspace's fixtures, parked while another
/// workspace is shown.
struct WorkspaceViews {
    sidebar: Entity<Sidebar>,
    messages: Entity<MessageList>,
    thread: Entity<ThreadPanel>,
    results: Entity<SearchView>,
    browser: Entity<ChannelBrowser>,
    huddle: Option<(ChannelId, Entity<HuddleBar>)>,
    profile: Entity<ProfilePanel>,
    switcher: Entity<QuickSwitcher>,
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
    /// Member counts from open-time `conversations.info`, newer than the snapshot's.
    pub(super) member_counts: HashMap<ChannelId, u32>,
    /// Apps among a channel's members, counted apart from people as Slack's header does.
    pub(super) app_counts: HashMap<ChannelId, u32>,
    /// Cached per-channel member lists from `Event::ConversationInfo` so returning to a Slack
    /// Connect channel renders its amber banner on frame 0 without waiting on a refetch.
    pub(super) members_cache: HashMap<ChannelId, Vec<UserId>>,
    /// Live mode: channels whose stored history has no older page left; the transcript
    /// stops asking for them.
    pub(super) history_complete: HashSet<ChannelId>,
    /// The user opened a conversation themselves; until then live reloads open the one most
    /// recently read in Slack.
    pub(super) chosen: bool,
    /// Live mode: per channel, its pinned messages (`pins.list`) or why they could not be
    /// loaded; fetched each time a pins surface opens, never per channel.
    pub(super) pins: HashMap<ChannelId, Loaded<Message>>,
    /// Live mode: per channel, its bookmarks (`bookmarks.list`), fetched each time a
    /// bookmarks surface opens.
    pub(super) bookmarks: HashMap<ChannelId, Loaded<ChannelBookmark>>,
    /// Live mode: per channel, its shared files (`files.list`), fetched each time Files &
    /// links opens.
    pub(super) files: HashMap<ChannelId, Loaded<File>>,
    /// Live mode: parsed read-only canvases keyed by file id (`files.info` -> `url_private`, `F-COLLAB-125`).
    pub(super) canvases: HashMap<String, Result<crate::canvas::CanvasDoc, String>>,
    /// File id of the live canvas currently displayed in `ConversationTab::Canvas`.
    pub(super) active_canvas: Option<String>,
    /// Live mode: parsed read-only Slack Lists keyed by file id (`files.info` + `lists.records.list`).
    pub(super) lists: HashMap<String, Result<slackless_core::api::SlackListData, String>>,
    /// File id of the live Slack List currently displayed in `ConversationTab::List`.
    pub(super) active_list: Option<String>,
    /// Live mode: per app DM channel, its App Home metadata and Block Kit view (`F-COLLAB-480`).
    pub(super) app_homes: HashMap<ChannelId, Result<slackless_core::api::AppHomeResponse, String>>,
    /// Deduplicates `subscriptions.thread.mark` calls: last cursor sent per `(channel, thread_ts)`.
    pub(super) marked_threads: HashMap<(ChannelId, Ts), Ts>,
}

/// A workspace that is not shown: its fixtures, open conversation, views, subscriptions,
/// and per-workspace live state.
struct Parked {
    fixtures: Arc<Fixtures>,
    channel: ChannelId,
    views: WorkspaceViews,
    subscriptions: Vec<Subscription>,
    state: WorkspaceState,
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

/// Which right-hand panel was last closed by `ToggleRightPane` (`Cmd+.`) so pressing it
/// again restores the same panel.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RightPane {
    Thread,
    Profile,
    Pins,
}

pub struct App {
    /// The shown workspace's fixtures.
    fixtures: Arc<Fixtures>,
    bridge: CoreBridge,
    /// Per workspace index: the live bridge for every other signed-in workspace (`None` for the
    /// shown workspace or fixture workspaces).
    parked_bridges: Vec<Option<CoreBridge>>,
    /// Every signed-in workspace's Team record (name, domain, icon) kept in sync across live
    /// refreshes so the workspace rail stays complete when `reload_live` rebuilds a single
    /// workspace's snapshot.
    live_teams: Vec<Team>,
    /// Per workspace index: latest `client.counts` for background workspaces so their rail
    /// tiles show live unread/mention badges even before they are visited.
    bg_counts: Vec<HashMap<ChannelId, ConversationCounts>>,
    /// Per workspace index: latest Activity feed unread count for background workspaces.
    bg_activity_unread: Vec<u32>,
    /// Index of the shown workspace in `fixtures.workspaces`.
    workspace: usize,
    /// Per workspace index: the parked state of a workspace visited before (`None` for the
    /// shown one and never-visited ones).
    parked: Vec<Option<Parked>>,
    /// Active workspace's per-workspace live state (swapped atomically with `Parked.state`).
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
    /// Results of the last search; shown in place of the message column while `searching`.
    results: Entity<SearchView>,
    searching: bool,
    /// The channel browser, mounted in a dialog on demand.
    browser: Entity<ChannelBrowser>,
    thread_open: bool,
    tab: ConversationTab,
    /// The fixture huddle's bar and the channel it runs in; `None` without a huddle.
    huddle: Option<(ChannelId, Entity<HuddleBar>)>,
    huddle_open: bool,
    /// Active native audio huddle session snapshot (Chime SFU).
    huddle_session: Option<slackless_core::huddle::HuddleSessionSnapshot>,
    /// Persistent sidebar mini-dock mounted at the bottom of the sidebar while in a huddle.
    huddle_dock: Option<Entity<HuddleDock>>,
    _huddle_dock_sub: Option<Subscription>,
    /// Floating incoming DM huddle ring banner (`huddle_invite`).
    huddle_invite: Option<Entity<HuddleInviteBanner>>,
    _huddle_invite_sub: Option<Subscription>,
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
    /// The profile pane; shown in the right panel while `profile_open` (exclusive with the
    /// thread panel).
    profile: Entity<ProfilePanel>,
    profile_open: bool,
    /// Last right-hand panel hidden by `ToggleRightPane` (`Cmd+.`), restored on the next toggle.
    last_right_pane: Option<RightPane>,
    /// The signed-in user's status, presence and notification pause.
    self_state: SelfState,
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
    /// Drafts, scheduled and sent messages; shown in place of the message column while
    /// `drafts_open`.
    drafts: Entity<DraftsView>,
    drafts_open: bool,
    /// Saved messages and reminders; shown in place of the message column while `later_open`.
    later: Entity<LaterView>,
    later_open: bool,
    /// The Threads view; replaces the message column while `threads_open`.
    threads: Entity<ThreadsView>,
    threads_open: bool,
    /// The All Unreads view (Slack's `p-unreads_view`); replaces the message column while
    /// `unreads_open`.
    unreads: Entity<UnreadsView>,
    unreads_open: bool,
    /// Channels already asked for a single-page history fetch from `refresh_unreads`, so an
    /// empty unread channel is fetched at most once per session.
    unreads_fetched: HashSet<ChannelId>,
    /// The Activity page (the rail's Activity destination): a full-width page right of the
    /// rail - the sidebar's list column hides while the destination is active.
    activity: Entity<ActivityFeed>,
    /// The rail's Activity destination is active (the page owns the main area).
    activity_open: bool,
    /// The Files view (the rail's Files destination): the main-area page beside the
    /// sidebar's Files list column, shown while that destination is active.
    files_page: Entity<FilesPage>,
    /// The Huddles page; replaces the message column while `huddles_open`.
    huddles: Entity<HuddlesView>,
    huddles_open: bool,
    /// The directory; replaces the message column while `directory_open`.
    directory: Entity<DirectoryView>,
    directory_open: bool,
    /// Native `Agents & tools` view (Apps, Slash commands, Slack Connect).
    tools_page: Entity<crate::tools_page::ToolsPage>,
    tools_open: bool,
    /// The "New message" view; replaces the message column while `compose_open`.
    compose: Entity<NewMessageView>,
    compose_open: bool,
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
    /// A compose send waiting for its DM to open ([`Command::OpenDm`]).
    pending_dm: Option<MessageContent>,
    /// The pinned-messages panel of the open conversation (right column, exclusive with the
    /// thread and profile panels).
    pins_open: bool,
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
    /// Fixture mode: link bookmarks added through the add-link dialog, per channel.
    fixture_bookmarks: HashMap<ChannelId, Vec<bookmarks::Bookmark>>,
    /// Dock badge + native notifications + menu-bar status item (`slackless-platform`).
    os: Rc<dyn Os>,
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
    /// The window title last set, so it is only set on change.
    window_title: String,
    /// The pending, debounced save of the window frame (replacing it cancels the last one).
    window_state_save: gpui::Task<()>,
    /// Live mode: the newest history read started; an overtaken read is dropped.
    history_generation: u64,
    /// Live mode: the newest thread read started; an overtaken read is dropped.
    thread_generation: u64,
    /// Live mode: the newest async `reload_live` snapshot started; coalesces event bursts.
    reload_generation: u64,
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
    /// Subscriptions to the shown workspace's views (parked with them on a switch).
    workspace_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl App {
    #[cfg(test)]
    pub fn new(
        fixtures: Arc<Fixtures>,
        bridge: CoreBridge,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Entity<App> {
        Self::new_multi(fixtures, vec![bridge], window, cx)
    }

    pub fn new_multi(
        fixtures: Arc<Fixtures>,
        mut bridges: Vec<CoreBridge>,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Entity<App> {
        if bridges.is_empty() {
            bridges.push(CoreBridge::offline());
        }
        let app = cx.new(|cx| {
            let mut bridge = bridges.remove(0);
            let live_teams = fixtures.workspaces.clone();
            let ws_count = live_teams.len().max(1);
            let mut parked_bridges: Vec<Option<CoreBridge>> = Vec::with_capacity(ws_count);
            parked_bridges.push(None);
            for extra in bridges {
                parked_bridges.push(Some(extra));
            }
            while parked_bridges.len() < ws_count {
                parked_bridges.push(None);
            }
            let bg_counts = vec![HashMap::new(); ws_count];
            let bg_activity_unread = vec![0; ws_count];
            // Read all workspace `local_state` keys once (1 SQLite query instead of ~10).
            let local = bridge.all_local();
            // Like Slack, reopen the conversation that was open last; a fresh store falls back
            // to the sidebar's first conversation until the user picks one.
            let restored = CoreBridge::parse_last_conversation(&local)
                .filter(|channel| fixtures.conversation(channel).is_some());
            let chosen = restored.is_some();
            let channel = restored.unwrap_or_else(|| first_conversation(&fixtures));
            let archive = bridge
                .store()
                .cloned()
                .or_else(|| fixture_archive(&fixtures));
            let views = build_workspace(&fixtures, 0, archive, &channel, window, cx);
            let workspace_subscriptions = subscribe_workspace(&views, window, cx);
            let WorkspaceViews {
                sidebar,
                messages,
                thread,
                results,
                browser,
                huddle,
                profile,
                switcher,
            } = views;
            let composer = Composer::new("Message".into(), fixtures.clone(), window, cx);
            let search = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder(SEARCH_PLACEHOLDER)
                    .auto_grow(1, 1)
            });
            let search_menu = SearchMenu::new(fixtures.clone(), search.clone(), cx);
            // The search panel's remembered state, restored at launch like Slack's: the
            // recent searches, both toggles and the recently visited conversations.
            if let Some(history) = CoreBridge::parse_search_history(&local) {
                search_menu.update(cx, |menu, cx| menu.set_history(history, cx));
            }
            if let Some(on) = CoreBridge::parse_search_history_shown(&local) {
                search_menu.update(cx, |menu, cx| menu.set_history_shown(on, cx));
            }
            if let Some(on) = CoreBridge::parse_search_filters_shown(&local) {
                search_menu.update(cx, |menu, cx| menu.set_filters_shown(on, cx));
            }
            let restored_recent = CoreBridge::parse_recent_places(&local)
                .unwrap_or_default()
                .into_iter()
                .filter(|channel| fixtures.conversation(channel).is_some())
                .collect();
            let preferences = Preferences::new(window, cx);
            // Restore persisted Preferences settings first, then overlay the zoom/font/color_mode
            // values `main` installed before the first frame.
            let mut restored_settings = CoreBridge::parse_settings(&local);
            if let Some(settings) = restored_settings.clone() {
                preferences.update(cx, |prefs, cx| prefs.load_settings(settings, window, cx));
            }
            let factor = cx.global::<crate::zoom::Zoom>().factor();
            let zoom = (crate::zoom::FACTORS.iter())
                .position(|level| *level == factor)
                .unwrap_or(crate::zoom::ACTUAL_SIZE);
            preferences.update(cx, |prefs, cx| prefs.set_zoom(zoom, cx));
            // Live: the remembered Color Mode over Slack's color theme (or persisted palette),
            // as `main` installed it before the first frame.
            let color_mode = bridge.parse_color_mode(&local);
            let applied_theme = match color_mode {
                Some(mode) => {
                    let palette = restored_settings
                        .as_ref()
                        .map(|s| s.palette)
                        .or(fixtures.palette)
                        .unwrap_or(Palette::Default);
                    let font = theme::font(cx);
                    preferences.update(cx, |prefs, cx| {
                        prefs.set_appearance(mode, palette, font, cx)
                    });
                    if let Some(ref mut s) = restored_settings {
                        s.mode = mode;
                        s.palette = palette;
                        s.font = font;
                        s.zoom = zoom;
                    }
                    let darker = restored_settings
                        .as_ref()
                        .is_some_and(|s| s.darker_sidebars);
                    let gradient = restored_settings.as_ref().is_none_or(|s| s.window_gradient);
                    (
                        mode.resolve(window.appearance()),
                        palette,
                        darker,
                        gradient,
                        false,
                    )
                }
                None => (SlackTheme::Dark, Palette::Default, false, true, false),
            };
            let saved_settings = if bridge.store().is_some() {
                Some(restored_settings.unwrap_or_else(|| preferences.read(cx).settings().clone()))
            } else {
                None
            };
            let status_picker = StatusPicker::new(window, cx);
            let user_menu = UserMenu::new(fixtures.clone(), cx);
            let emoji_picker = EmojiPicker::new(fixtures.clone(), window, cx);
            let persisted_freq = CoreBridge::parse_frequent_emoji(&local);
            let persisted_tone = CoreBridge::parse_emoji_tone(&local);
            if persisted_freq.is_some() || persisted_tone.is_some() {
                emoji_picker.update(cx, |picker, cx| {
                    picker.restore(persisted_freq, persisted_tone, cx);
                });
            }
            let drafts = DraftsView::new(cx);
            if let Some(saved_drafts) = CoreBridge::parse_drafts(&local) {
                let entries: Vec<Entry> = saved_drafts
                    .into_iter()
                    .map(|d| Entry {
                        id: None,
                        workspace: 0,
                        channel: d.channel,
                        label: d.label,
                        text: d.text,
                        at: d.at,
                    })
                    .collect();
                drafts.update(cx, |view, cx| view.load_drafts(entries, cx));
            }
            let later = LaterView::new(cx);
            let threads = ThreadsView::new(fixtures.clone(), window, cx);
            let unreads = UnreadsView::new(fixtures.clone(), cx);
            let activity = ActivityFeed::new(fixtures.clone(), cx);
            let files_page = FilesPage::new(fixtures.clone(), window, cx);
            let huddles = HuddlesView::new(fixtures.clone(), cx);
            let directory = DirectoryView::new(fixtures.clone(), window, cx);
            let tools_page = crate::tools_page::ToolsPage::new(fixtures.clone(), window, cx);
            let compose = NewMessageView::new(fixtures.clone(), window, cx);
            let details = ChannelDetails::new(fixtures.clone(), window, cx);
            let canvas_search = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Find in canvas…")
                    .auto_grow(1, 1)
            });
            let list_search = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Filter list items…")
                    .auto_grow(1, 1)
            });
            let app_home_kit = cx.new(|_| crate::blockkit::BlockKitState::default());
            if fixtures.sample {
                later.update(cx, |later, cx| later.seed(0, &fixtures, cx));
            }
            crate::actions::bind_menu_keys(cx);
            cx.bind_keys([
                KeyBinding::new("cmd-k", QuickSwitch, None),
                KeyBinding::new("cmd-j", JumpToUnread, None),
                KeyBinding::new("cmd-shift-h", OpenHome, None),
                KeyBinding::new("cmd-[", NavigateBack, None),
                KeyBinding::new("cmd-]", NavigateForward, None),
                KeyBinding::new("cmd-,", OpenPreferences, None),
                KeyBinding::new("cmd-/", ShowShortcuts, None),
                KeyBinding::new("cmd-n", NewMessage, None),
                KeyBinding::new("cmd-shift-k", OpenDms, None),
                // Slack's Activity shortcut: Cmd+Shift+A is Slack's All Unreads, which
                // its own menu labels "All Unreads"; Activity keeps Cmd+Shift+M.
                KeyBinding::new("cmd-shift-m", OpenActivity, None),
                KeyBinding::new("cmd-shift-a", OpenUnreads, None),
                KeyBinding::new("cmd-shift-t", OpenThreads, None),
                KeyBinding::new("cmd-shift-d", OpenDrafts, None),
                KeyBinding::new("cmd-shift-e", OpenPeople, None),
                KeyBinding::new("cmd-shift-j", OpenDownloads, None),
                KeyBinding::new("cmd-shift-l", OpenChannelBrowser, None),
                KeyBinding::new("cmd-shift-y", SetStatus, None),
                KeyBinding::new("cmd-g", FocusSearch, None),
                KeyBinding::new("cmd-f", SearchConversation, None),
                KeyBinding::new("cmd-shift-s", ToggleWorkspaceRail, None),
                KeyBinding::new("cmd-.", ToggleRightPane, None),
                KeyBinding::new("cmd-shift-backspace", UnsendLastMessage, None),
                KeyBinding::new("cmd-shift-m", OpenMore, None),
                KeyBinding::new("ctrl-tab", NextNavTab, None),
                KeyBinding::new("ctrl-shift-tab", PrevNavTab, None),
                KeyBinding::new("ctrl-down", NextSection, None),
                KeyBinding::new("ctrl-up", PrevSection, None),
                KeyBinding::new("f6", FocusNextPane, None),
                KeyBinding::new("shift-f6", FocusPrevPane, None),
                // Slack's View menu shows Cmd++; Cmd+= (no Shift) zooms in too.
                KeyBinding::new("cmd-+", ZoomIn, None),
                KeyBinding::new("cmd-=", ZoomIn, None),
                KeyBinding::new("cmd--", ZoomOut, None),
                KeyBinding::new("cmd-0", ActualSize, None),
                KeyBinding::new("cmd-}", NextWorkspace, None),
                KeyBinding::new("cmd-{", PreviousWorkspace, None),
                KeyBinding::new("alt-up", PreviousConversation, None),
                KeyBinding::new("alt-down", NextConversation, None),
                KeyBinding::new("alt-shift-up", PreviousUnread, None),
                KeyBinding::new("alt-shift-down", NextUnread, None),
                // Only fire while focus is on the app shell itself, never inside an input,
                // popup menu, dialog or popover: GPUI resolves keybindings before text
                // insertion, so an unscoped single-key binding would swallow those letters
                // (and Backspace / Right / Escape) in the composer and every text field.
                KeyBinding::new("shift-escape", MarkAllRead, None),
                KeyBinding::new("cmd-shift-space", ToggleHuddleMute, None),
            ]);
            const OUTSIDE_INPUT: Option<&str> =
                Some("App && !Input && !PopupMenu && !Dialog && !Popover && !Select");
            cx.bind_keys([
                KeyBinding::new("escape", MarkConversationRead, OUTSIDE_INPUT),
                // Slack's one-key actions on the focused message (the transcript's last
                // hovered row). Scoped to `OUTSIDE_INPUT` so focused fields still receive
                // their own letters, arrows, backspace and escape; [`App::focus_message`]
                // re-checks the rest (which message, whose message, no overlay open).
                KeyBinding::new("e", FocusEditMessage, OUTSIDE_INPUT),
                KeyBinding::new("backspace", FocusDeleteMessage, OUTSIDE_INPUT),
                KeyBinding::new("t", FocusReplyInThread, OUTSIDE_INPUT),
                KeyBinding::new("right", FocusReplyInThread, OUTSIDE_INPUT),
                KeyBinding::new("f", FocusForwardMessage, OUTSIDE_INPUT),
                KeyBinding::new("p", FocusTogglePin, OUTSIDE_INPUT),
                KeyBinding::new("a", FocusSaveMessage, OUTSIDE_INPUT),
                KeyBinding::new("u", FocusMarkMessageUnread, OUTSIDE_INPUT),
                KeyBinding::new("m", FocusRemindMe, OUTSIDE_INPUT),
                KeyBinding::new("r", FocusReact, OUTSIDE_INPUT),
                KeyBinding::new("up", BrowseUp, OUTSIDE_INPUT),
                KeyBinding::new("down", BrowseDown, OUTSIDE_INPUT),
                KeyBinding::new("enter", BrowseOpen, OUTSIDE_INPUT),
                KeyBinding::new("left", ExitThread, OUTSIDE_INPUT),
                KeyBinding::new("home", JumpToOldest, OUTSIDE_INPUT),
                KeyBinding::new("end", JumpToNewest, OUTSIDE_INPUT),
            ]);
            let workspace_keys = [
                "cmd-1", "cmd-2", "cmd-3", "cmd-4", "cmd-5", "cmd-6", "cmd-7", "cmd-8", "cmd-9",
            ];
            cx.bind_keys(
                (workspace_keys.into_iter().enumerate())
                    .map(|(ix, keys)| KeyBinding::new(keys, SwitchWorkspace(ix), None)),
            );
            let tab_keys = [
                "ctrl-1", "ctrl-2", "ctrl-3", "ctrl-4", "ctrl-5", "ctrl-6", "ctrl-7", "ctrl-8",
                "ctrl-9",
            ];
            cx.bind_keys(
                (tab_keys.into_iter().enumerate())
                    .map(|(ix, keys)| KeyBinding::new(keys, SelectConversationTab(ix), None)),
            );
            let focus = cx.focus_handle();
            focus.focus(window, cx);

            let this = cx.weak_entity();
            // A second handle: the search intercept below moves `this` into its closure.
            let viewer_keys = cx.weak_entity();
            // A third: the profile menu's Escape intercept moves `this` into its closure.
            let menu_keys = cx.weak_entity();
            // A fourth: the member hover card's Escape intercept.
            let card_keys = cx.weak_entity();
            // A fifth: the All Unreads view's arrow-key intercept.
            let unreads_keys = cx.weak_entity();
            let mut subscriptions = vec![
                cx.observe_window_bounds(window, |this, window, cx| {
                    this.save_window_state(window, cx);
                }),
                cx.observe_window_activation(window, |this, window, cx| {
                    this.window_active = window.is_window_active();
                    if this.window_active {
                        let settings = this.preferences.read(cx).settings().clone();
                        if settings.no_bg_mark_read && settings.mark_read != 2 {
                            if this.threads_open {
                                this.auto_mark_threads_view_read(cx);
                            } else {
                                this.maybe_mark_open_read(cx);
                            }
                        }
                    }
                }),
                // While the search panel is open, Up/Down/Enter/Escape steer its rows. GPUI
                // resolves the focused field's own key bindings (caret moves, a newline)
                // before any listener around it, so the keys are taken here, ahead of them.
                cx.intercept_keystrokes(move |event, window, cx| {
                    if event.input_preference == InputPreference::CharacterInput {
                        return;
                    }
                    let _ = this.update(cx, |app, cx| {
                        if !app.search_menu_open
                            || !app.search.read(cx).focus_handle(cx).is_focused(window)
                        {
                            return;
                        }
                        if app
                            .search_menu
                            .update(cx, |menu, cx| menu.handle_key(&event.keystroke, cx))
                        {
                            cx.stop_propagation();
                        }
                    });
                }),
                // While the image viewer is open, Escape closes it and the arrows step
                // through the conversation's images (ahead of any focused field, like the
                // search panel's keys above).
                cx.intercept_keystrokes({
                    move |event, window, cx| {
                        if event.input_preference == InputPreference::CharacterInput {
                            return;
                        }
                        let _ = viewer_keys.update(cx, |app, cx| {
                            let Some(viewer) = app.image_viewer.clone() else {
                                return;
                            };
                            match event.keystroke.key.as_str() {
                                "escape" => {
                                    app.close_image_viewer(cx);
                                    cx.stop_propagation();
                                }
                                "left" => {
                                    viewer.update(cx, |viewer, cx| viewer.step(-1, window, cx));
                                    cx.stop_propagation();
                                }
                                "right" => {
                                    viewer.update(cx, |viewer, cx| viewer.step(1, window, cx));
                                    cx.stop_propagation();
                                }
                                _ => {}
                            }
                        });
                    }
                }),
                // While the profile menu is open, Escape closes it, submenu included —
                // Slack closes the whole menu, not just the open submenu.
                cx.intercept_keystrokes({
                    move |event, _, cx| {
                        if event.input_preference == InputPreference::CharacterInput
                            || event.keystroke.key.as_str() != "escape"
                        {
                            return;
                        }
                        let _ = menu_keys.update(cx, |app, cx| {
                            if !app.user_menu_open {
                                return;
                            }
                            app.user_menu_open = false;
                            app.user_menu.update(cx, |menu, cx| menu.closed(cx));
                            cx.notify();
                            cx.stop_propagation();
                        });
                    }
                }),
                // While the member hover card is open, Escape closes it, like Slack's
                // hovercard.
                cx.intercept_keystrokes(move |event, _, cx| {
                    if event.input_preference == InputPreference::CharacterInput
                        || event.keystroke.key.as_str() != "escape"
                    {
                        return;
                    }
                    let _ = card_keys.update(cx, |app, cx| {
                        if app.profile_hover.card.is_some() {
                            app.close_hover_card(cx);
                            cx.stop_propagation();
                        }
                    });
                }),
                // While the All Unreads view is open and no field, dialog or menu holds the
                // focus, its keys steer the cards (Slack's "Navigate unread messages"):
                // Up/Down walk, Left/Right fold, Esc marks the focused group read - ahead of
                // the transcript's own arrow and Esc actions.
                cx.intercept_keystrokes(move |event, window, cx| {
                    if event.input_preference == InputPreference::CharacterInput {
                        return;
                    }
                    let _ = unreads_keys.update(cx, |app, cx| {
                        if !app.unreads_open
                            || !window
                                .focused(cx)
                                .is_none_or(|focused| focused == app.focus)
                        {
                            return;
                        }
                        let handled = app
                            .unreads
                            .update(cx, |view, cx| view.handle_key(&event.keystroke.key, cx));
                        if handled {
                            cx.stop_propagation();
                        }
                    });
                }),
                bridge.subscribe_in(window, cx, |this: &mut App, event, window, cx| {
                    this.on_workspace_event(0, event, window, cx);
                }),
                cx.subscribe_in(
                    &search,
                    window,
                    |this: &mut App, search, event: &InputEvent, window, cx| match event {
                        InputEvent::PressEnter { .. } => {
                            let query = search.read(cx).value().to_string();
                            this.run_message_search(query, cx);
                        }
                        // The pill's text changed while the dropdown is open: refilter it.
                        InputEvent::Change if this.search_menu_open => {
                            let value = search.read(cx).value().to_string();
                            this.search_menu
                                .update(cx, |menu, cx| menu.set_query(&value, cx));
                        }
                        // Focus moved away from the pill (a dialog, another pane): fold
                        // it, leaving whatever took the focus alone.
                        InputEvent::Blur if this.search_menu_open => {
                            this.fold_search_menu(window, cx);
                        }
                        _ => {}
                    },
                ),
                cx.subscribe_in(
                    &search_menu,
                    window,
                    |this: &mut App, _, event: &SearchMenuEvent, window, cx| match event {
                        SearchMenuEvent::Open(channel) => {
                            this.close_search_menu(window, cx);
                            this.select(channel.clone(), cx);
                            this.focus.focus(window, cx);
                        }
                        // The Threads row of "Unreads & drafts": the sidebar's own path.
                        SearchMenuEvent::OpenThreads => {
                            this.close_search_menu(window, cx);
                            this.open_threads(cx);
                            this.focus.focus(window, cx);
                        }
                        // The field stays focused: the query can be refined and re-run.
                        SearchMenuEvent::Search(query) => {
                            this.fold_search_menu(window, cx);
                            this.run_message_search(query.clone(), cx);
                        }
                        SearchMenuEvent::SetHistoryShown(on) => {
                            if let Some(write) = this.bridge.remember_search_history_shown(*on) {
                                cx.background_executor().spawn(write).detach();
                            }
                        }
                        SearchMenuEvent::SetFiltersShown(on) => {
                            if let Some(write) = this.bridge.remember_search_filters_shown(*on) {
                                cx.background_executor().spawn(write).detach();
                            }
                        }
                        SearchMenuEvent::Dismiss => this.close_search_menu(window, cx),
                    },
                ),
                cx.subscribe_in(
                    &composer,
                    window,
                    |this: &mut App, _, event: &ComposerEvent, window, cx| match event {
                        ComposerEvent::Submit(content) => this.send(content.clone(), window, cx),
                        ComposerEvent::SubmitWithFiles { content, files } => {
                            let channel = this.channel.clone();
                            this.upload_files(
                                channel,
                                None,
                                content.clone(),
                                files.clone(),
                                window,
                                cx,
                            );
                        }
                        ComposerEvent::OpenEmojiPicker(position) => {
                            this.open_picker(PickerTarget::Composer, *position, window, cx);
                        }
                        ComposerEvent::OpenGifPicker(position) => {
                            this.open_picker_tab(
                                PickerTarget::Composer,
                                *position,
                                PickerTab::Gifs,
                                window,
                                cx,
                            );
                        }
                        // The message list has no way in to start an edit from outside yet;
                        // the thread panel handles this for its own composer.
                        ComposerEvent::EditLast => this
                            .messages
                            .update(cx, |list, cx| list.edit_last_own(window, cx)),
                        ComposerEvent::SlashCommand(invocation) => {
                            this.run_slash(invocation, None, window, cx);
                        }
                        ComposerEvent::Schedule { text, when } => {
                            let channel = this.channel.clone();
                            this.schedule_at(
                                channel,
                                None,
                                text.clone(),
                                when.at(now_secs()),
                                when.label().to_lowercase(),
                                window,
                                cx,
                            );
                        }
                        ComposerEvent::ScheduleCustom(text) => {
                            let channel = this.channel.clone();
                            this.open_custom_schedule(channel, None, text.clone(), window, cx);
                        }
                        ComposerEvent::Typing => this.bridge.send(Command::Typing {
                            channel: this.channel.clone(),
                            thread_ts: None,
                        }),
                        ComposerEvent::Cancel => {}
                    },
                ),
                cx.subscribe_in(
                    &emoji_picker,
                    window,
                    |this: &mut App, _, event: &EmojiPickerEvent, window, cx| match event {
                        EmojiPickerEvent::Pick(choice) => this.pick_emoji(choice, window, cx),
                        EmojiPickerEvent::PickGif(gif) => this.pick_gif(gif, window, cx),
                        EmojiPickerEvent::RunGiphy(query) => this.run_giphy(query, window, cx),
                        EmojiPickerEvent::SearchGifs(query) => {
                            if !this.fixtures.sample {
                                this.bridge.send(Command::SearchGifs {
                                    query: query.clone(),
                                });
                            }
                        }
                        EmojiPickerEvent::ToneChanged(tone) => {
                            if let Some(write) = this.bridge.remember_emoji_tone(*tone) {
                                cx.background_executor().spawn(write).detach();
                            }
                            if !this.fixtures.sample {
                                this.bridge.send(Command::SetUserPref {
                                    name: "preferred_skin_tone".to_owned(),
                                    value: (*tone).to_string(),
                                });
                            }
                        }
                    },
                ),
                cx.subscribe_in(
                    &preferences,
                    window,
                    |this: &mut App, _, event: &PreferencesEvent, window, cx| match event {
                        PreferencesEvent::Changed => this.apply_preferences(window, cx),
                        PreferencesEvent::ResetCache => this.confirm_reset_cache(window, cx),
                        PreferencesEvent::CompactStorage => this.compact_storage(window, cx),
                        PreferencesEvent::PruneMediaCache => this.prune_media_cache(window, cx),
                        PreferencesEvent::ExportNetLog => {
                            let json = slackless_core::api::net_log_export_json();
                            let configured =
                                this.preferences.read(cx).settings().download_dir.clone();
                            let dir = if !configured.is_empty() {
                                std::path::PathBuf::from(configured)
                            } else {
                                std::env::var_os("HOME")
                                    .map(|h| std::path::PathBuf::from(h).join("Downloads"))
                                    .unwrap_or_else(std::env::temp_dir)
                            };
                            let path = dir.join("slackless-net-log.json");
                            match std::fs::write(&path, json) {
                                Ok(()) => notify(
                                    "Net log exported",
                                    format!("Saved to {}", path.display()),
                                    window,
                                    cx,
                                ),
                                Err(err) => {
                                    notify("Net log export failed", err.to_string(), window, cx)
                                }
                            }
                        }
                        PreferencesEvent::TestSpeaker(output_device) => {
                            if this.fixtures.sample {
                                let dev = output_device.clone();
                                std::thread::spawn(move || {
                                    let _ = slackless_core::huddle::play_speaker_test_tone(
                                        dev.as_deref(),
                                    );
                                });
                            } else {
                                this.bridge.send(Command::TestHuddleSpeaker {
                                    output_device: output_device.clone(),
                                });
                            }
                        }
                    },
                ),
                cx.observe_window_appearance(window, |this: &mut App, window, cx| {
                    this.apply_preferences(window, cx);
                }),
                cx.subscribe_in(
                    &status_picker,
                    window,
                    |this: &mut App, _, event: &StatusEvent, window, cx| {
                        match event {
                            StatusEvent::Save(status) => {
                                this.self_state.set_status(status.clone());
                                this.send_status_to_slack(this.self_state.status.as_ref());
                                if this
                                    .preferences
                                    .read(cx)
                                    .settings()
                                    .status_pauses_notifications
                                {
                                    if let Some(st) = status {
                                        let now = crate::messages::now_secs();
                                        let mins = st
                                            .expiration
                                            .map(|until| {
                                                ((until.saturating_sub(now) + 59) / 60)
                                                    .clamp(1, 1440)
                                                    as u32
                                            })
                                            .unwrap_or(60);
                                        this.self_state.paused_until =
                                            Some(now + i64::from(mins) * 60);
                                        this.bridge.send(Command::SetDndSnooze { minutes: mins });
                                    } else if this.self_state.paused() {
                                        this.self_state.paused_until = None;
                                        this.bridge.send(Command::EndDndSnooze);
                                    }
                                }
                                this.push_self_state(cx);
                            }
                            StatusEvent::Schedule(sched) => {
                                this.self_state.scheduled_statuses.push(sched.clone());
                                let when = crate::messages::clock(sched.starts_at, None);
                                notify(
                                    "Status scheduled",
                                    format!("Will set \"{}\" at {when}.", sched.text),
                                    window,
                                    cx,
                                );
                            }
                            StatusEvent::RemoveScheduled(ix) => {
                                if *ix < this.self_state.scheduled_statuses.len() {
                                    this.self_state.scheduled_statuses.remove(*ix);
                                    notify(
                                        "Scheduled status removed",
                                        "Cancelled upcoming scheduled status.",
                                        window,
                                        cx,
                                    );
                                }
                                return;
                            }
                            StatusEvent::Cancel => {}
                        }
                        window.close_dialog(cx);
                    },
                ),
                cx.subscribe_in(
                    &user_menu,
                    window,
                    |this: &mut App, _, event: &UserMenuEvent, window, cx| {
                        this.user_menu_event(event, window, cx);
                    },
                ),
                cx.subscribe_in(
                    &drafts,
                    window,
                    |this: &mut App, _, event: &DraftsEvent, window, cx| match event {
                        DraftsEvent::Close => {
                            this.drafts_open = false;
                            cx.notify();
                        }
                        DraftsEvent::Open { workspace, channel } => {
                            this.switch_workspace(*workspace, window, cx);
                            this.select(channel.clone(), cx);
                        }
                        DraftsEvent::SendNow(entry) => this.deliver(entry.clone(), window, cx),
                        DraftsEvent::CancelScheduled(entry) => {
                            if let Some(id) = entry.id.clone() {
                                this.bridge.send(Command::DeleteScheduledMessage {
                                    channel: entry.channel.clone(),
                                    scheduled_message_id: id,
                                });
                            }
                        }
                        DraftsEvent::Deleted => {
                            let count = this.drafts.read(cx).draft_count();
                            this.sidebar
                                .update(cx, |sidebar, cx| sidebar.set_draft_count(count, cx));
                            this.persist_drafts(cx);
                        }
                    },
                ),
                cx.subscribe_in(
                    &later,
                    window,
                    |this: &mut App, _, event: &LaterEvent, window, cx| match event {
                        LaterEvent::PopOut => this.pop_out_later(window, cx),
                        LaterEvent::Close => {
                            this.later_open = false;
                            cx.notify();
                        }
                        LaterEvent::Open {
                            workspace,
                            channel,
                            ts,
                        } => {
                            this.switch_workspace(*workspace, window, cx);
                            this.select(channel.clone(), cx);
                            this.messages.update(cx, |list, cx| list.reveal(ts, cx));
                        }
                        LaterEvent::Unsaved {
                            workspace,
                            channel,
                            ts,
                        } => {
                            if *workspace == this.workspace {
                                this.messages
                                    .update(cx, |list, cx| list.set_saved(ts, false, cx));
                                this.thread
                                    .update(cx, |thread, cx| thread.set_saved(ts, false, cx));
                                this.threads
                                    .update(cx, |threads, cx| threads.set_saved(ts, false, cx));
                                this.sync_saved(cx);
                                this.bridge.send(Command::DeleteSavedItem {
                                    channel: channel.clone(),
                                    ts: ts.clone(),
                                });
                            } else if let Some(bridge) =
                                this.parked_bridges.get(*workspace).and_then(Option::as_ref)
                            {
                                bridge.send(Command::DeleteSavedItem {
                                    channel: channel.clone(),
                                    ts: ts.clone(),
                                });
                            }
                        }
                        LaterEvent::Update {
                            workspace,
                            channel,
                            ts,
                            mark,
                            date_due,
                        } => {
                            let cmd = Command::UpdateSavedItem {
                                channel: channel.clone(),
                                ts: ts.clone(),
                                mark: *mark,
                                date_due: *date_due,
                            };
                            if *workspace == this.workspace {
                                this.bridge.send(cmd);
                            } else if let Some(bridge) =
                                this.parked_bridges.get(*workspace).and_then(Option::as_ref)
                            {
                                bridge.send(cmd);
                            }
                        }
                        LaterEvent::ClearCompleted { workspace } => {
                            if *workspace == this.workspace {
                                this.bridge.send(Command::ClearCompletedSaved);
                            } else if let Some(bridge) =
                                this.parked_bridges.get(*workspace).and_then(Option::as_ref)
                            {
                                bridge.send(Command::ClearCompletedSaved);
                            }
                        }
                        LaterEvent::Changed => this.sync_later_count(cx),
                    },
                ),
                cx.subscribe_in(
                    &threads,
                    window,
                    |this: &mut App, _, event: &ThreadsEvent, window, cx| match event {
                        ThreadsEvent::Open { channel, thread_ts } => {
                            this.select(channel.clone(), cx);
                            this.open_thread(channel.clone(), thread_ts.clone(), cx);
                        }
                        ThreadsEvent::OpenLink(target) => {
                            this.open_link(target.clone(), window, cx);
                        }
                        ThreadsEvent::ExpandThread { channel, thread_ts } => {
                            this.bridge.send(Command::OpenThread {
                                channel: channel.clone(),
                                thread_ts: thread_ts.clone(),
                            });
                            if let Some(read) =
                                this.bridge.thread(channel.clone(), thread_ts.clone())
                            {
                                let (ch, ts) = (channel.clone(), thread_ts.clone());
                                cx.spawn(async move |this, cx| {
                                    let msgs = cx.background_executor().spawn(read).await;
                                    let _ = this.update(cx, |app, cx| {
                                        app.resolve_named_users(&msgs);
                                        let replies: Vec<_> =
                                            msgs.into_iter().filter(|m| m.ts != ts).collect();
                                        app.threads.update(cx, |view, cx| {
                                            view.set_card_replies(&ch, &ts, replies, cx);
                                        });
                                    });
                                })
                                .detach();
                            }
                        }
                        ThreadsEvent::Reply {
                            channel,
                            thread_ts,
                            content,
                            broadcast,
                        } => {
                            this.bridge.send(Command::SendMessage {
                                channel: channel.clone(),
                                content: content.clone(),
                                thread_ts: Some(thread_ts.clone()),
                                broadcast: *broadcast,
                            });
                            this.sync_thread_sidebar_counts(cx);
                        }
                        ThreadsEvent::ReplyWithFiles {
                            channel,
                            thread_ts,
                            content,
                            files,
                        } => {
                            this.upload_files(
                                channel.clone(),
                                Some(thread_ts.clone()),
                                content.clone(),
                                files.clone(),
                                window,
                                cx,
                            );
                        }
                        ThreadsEvent::MessageAction {
                            channel,
                            thread_ts,
                            message,
                            action,
                        } => {
                            this.handle_threads_message_action(
                                channel.clone(),
                                thread_ts.clone(),
                                message.as_ref().clone(),
                                action.clone(),
                                window,
                                cx,
                            );
                        }
                        ThreadsEvent::EditMessage {
                            channel,
                            ts,
                            content,
                        } => {
                            this.post_change(
                                channel,
                                &MessageChange::Edit {
                                    ts: ts.clone(),
                                    content: content.clone(),
                                },
                            );
                        }
                        ThreadsEvent::MarkRead {
                            channel,
                            thread_ts,
                            ts,
                        } => {
                            this.mark_thread_read(
                                channel.clone(),
                                thread_ts.clone(),
                                ts.clone(),
                                true,
                                cx,
                            );
                        }
                        ThreadsEvent::MarkAllRead(targets) => {
                            for (channel, thread_ts, ts) in targets {
                                this.mark_thread_read(
                                    channel.clone(),
                                    thread_ts.clone(),
                                    ts.clone(),
                                    true,
                                    cx,
                                );
                            }
                            this.sidebar
                                .update(cx, |s, cx| s.set_thread_counts(false, 0, cx));
                        }
                        ThreadsEvent::PickEmoji(position) => {
                            this.open_picker(PickerTarget::ThreadsComposer, *position, window, cx);
                        }
                        ThreadsEvent::PickGif(position) => {
                            this.open_picker_tab(
                                PickerTarget::ThreadsComposer,
                                *position,
                                PickerTab::Gifs,
                                window,
                                cx,
                            );
                        }
                        ThreadsEvent::SlashCommand(invocation) => {
                            let thread = this.threads.read(cx).active_thread();
                            this.run_slash(invocation, thread, window, cx)
                        }
                    },
                ),
                cx.subscribe_in(
                    &unreads,
                    window,
                    |this: &mut App, _, event: &UnreadsEvent, window, cx| match event {
                        UnreadsEvent::OpenConversation(channel) => {
                            this.select(channel.clone(), cx);
                        }
                        UnreadsEvent::OpenThread { channel, thread_ts } => {
                            this.open_thread(channel.clone(), thread_ts.clone(), cx);
                        }
                        UnreadsEvent::OpenLink(target) => {
                            this.open_link(target.clone(), window, cx);
                        }
                        UnreadsEvent::MarkRead { channel, ts } => {
                            this.mark_read(channel, ts.clone(), cx);
                        }
                        UnreadsEvent::Reply { channel, text } => {
                            let content = slackless_core::api::MessageContent::plain(text.clone());
                            this.bridge.send(Command::SendMessage {
                                channel: channel.clone(),
                                content,
                                thread_ts: None,
                                broadcast: false,
                            });
                            actions::notify("Reply sent", this.label(channel), window, cx);
                        }
                        UnreadsEvent::MessageAction {
                            channel,
                            message,
                            action,
                        } => {
                            this.handle_unreads_message_action(
                                channel.clone(),
                                message.as_ref().clone(),
                                action.clone(),
                                window,
                                cx,
                            );
                        }
                        UnreadsEvent::MarkAllRead => this.request_mark_all_read(window, cx),
                    },
                ),
                cx.subscribe_in(
                    &activity,
                    window,
                    |this: &mut App, _, event: &ActivityEvent, window, cx| match event {
                        ActivityEvent::Open { channel, thread_ts } => {
                            this.select(channel.clone(), cx);
                            if let Some(thread_ts) = thread_ts {
                                this.open_thread(channel.clone(), thread_ts.clone(), cx);
                            }
                        }
                        ActivityEvent::Mark { entries, read } => {
                            this.bridge.send(Command::MarkActivity {
                                entries: entries.clone(),
                                read: *read,
                            });
                        }
                        ActivityEvent::Retry => this.fetch_activity(cx),
                        ActivityEvent::PopOut => this.pop_out_activity(window, cx),
                    },
                ),
                cx.observe(&activity, |this: &mut App, _, cx| {
                    // The rail tile and the dock badge follow the feed's unread rows.
                    let unread = this.activity.read(cx).unread_count();
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.set_activity_unread(unread, cx));
                    cx.notify();
                }),
                cx.subscribe_in(
                    &files_page,
                    window,
                    |this: &mut App, _, event: &FilesPageEvent, window, cx| match event {
                        FilesPageEvent::Load(filter) => {
                            this.bridge.send(Command::LoadFiles(filter.clone()));
                        }
                        FilesPageEvent::LoadExtras => {
                            this.bridge.send(Command::LoadFilesExtras);
                        }
                        FilesPageEvent::SaveFile { file_id, name } => {
                            this.bridge.send(Command::SaveFile {
                                file_id: file_id.clone(),
                                date_due: None,
                            });
                            notify("Saved for later", name.clone(), window, cx);
                        }
                        FilesPageEvent::DeleteFile { file_id, name } => {
                            this.confirm_delete_file(file_id.clone(), name.clone(), window, cx);
                        }
                        FilesPageEvent::ToggleFavorite {
                            file_id,
                            collection_id,
                            starred,
                        } => {
                            this.bridge.send(Command::SetFileFavorite {
                                file_id: file_id.clone(),
                                collection_id: collection_id.clone(),
                                starred: *starred,
                            });
                        }
                        FilesPageEvent::CreateCollection { name } => {
                            this.bridge.send(Command::CreateFileCollection {
                                name: name.clone(),
                                emoji: None,
                            });
                            notify("Collection created", name.clone(), window, cx);
                        }
                        FilesPageEvent::DeleteCollection { collection_id } => {
                            this.bridge.send(Command::DeleteFileCollection {
                                collection_id: collection_id.clone(),
                            });
                            notify("Collection deleted", "Removed file collection.", window, cx);
                        }
                        FilesPageEvent::RestoreFile { file_id, title } => {
                            this.bridge.send(Command::RestoreDeletedFile {
                                file_id: file_id.clone(),
                            });
                            notify("Restoring", title.clone(), window, cx);
                        }
                        FilesPageEvent::OpenCanvas { file_id } => {
                            this.open_live_canvas(file_id.clone(), cx);
                        }
                        FilesPageEvent::OpenList { file_id } => {
                            this.open_live_list(file_id.clone(), cx);
                        }
                        FilesPageEvent::CreateCanvas { title } => {
                            this.bridge.send(Command::CreateCanvas {
                                title: title.clone(),
                                markdown: format!("# {title}\n\n"),
                                channel: None,
                            });
                            notify("Creating canvas", title.clone(), window, cx);
                        }
                        FilesPageEvent::CreateList { name } => {
                            this.bridge.send(Command::CreateList {
                                name: name.clone(),
                                description: None,
                                channel: None,
                            });
                            notify("Creating list", name.clone(), window, cx);
                        }
                        FilesPageEvent::PopOut => {
                            this.pop_out_files(window, cx);
                        }
                    },
                ),
                cx.subscribe(
                    &canvas_search,
                    |this: &mut App, input, event: &InputEvent, cx| {
                        if let InputEvent::Change = event {
                            this.canvas_query = input.read(cx).value().to_string();
                            cx.notify();
                        }
                    },
                ),
                cx.subscribe(
                    &list_search,
                    |this: &mut App, input, event: &InputEvent, cx| {
                        if let InputEvent::Change = event {
                            this.list_query = input.read(cx).value().to_string();
                            cx.notify();
                        }
                    },
                ),
                cx.subscribe_in(
                    &huddles,
                    window,
                    |this: &mut App, _, event: &HuddlesEvent, _, cx| match event {
                        HuddlesEvent::Open(channel) => {
                            this.select(channel.clone(), cx);
                            this.huddle_open = this.huddle_here();
                            cx.notify();
                        }
                        HuddlesEvent::OpenThread(channel, ts) => {
                            this.select(channel.clone(), cx);
                            this.open_thread(channel.clone(), ts.clone(), cx);
                            cx.notify();
                        }
                        HuddlesEvent::Close => {
                            this.huddles_open = false;
                            cx.notify();
                        }
                    },
                ),
                cx.subscribe_in(
                    &directory,
                    window,
                    |this: &mut App, _, event: &DirectoryEvent, window, cx| match event {
                        DirectoryEvent::OpenProfile(user) => this.open_profile(user.clone(), cx),
                        DirectoryEvent::Join(channel) => {
                            if let Some(joined) = this.fixtures.conversation(channel).cloned() {
                                this.sidebar
                                    .update(cx, |sidebar, cx| sidebar.add_channel(joined, cx));
                            }
                        }
                        DirectoryEvent::Open(channel) => this.select(channel.clone(), cx),
                        DirectoryEvent::CreateUsergroup {
                            name,
                            handle,
                            description,
                        } => {
                            if !this.fixtures.sample {
                                this.bridge.send(Command::CreateUsergroup {
                                    name: name.clone(),
                                    handle: handle.clone(),
                                    description: description.clone(),
                                });
                            }
                            notify(
                                "User group created",
                                if handle.is_empty() {
                                    name.clone()
                                } else {
                                    format!("{name} (@{handle})")
                                },
                                window,
                                cx,
                            );
                        }
                        DirectoryEvent::Close => {
                            this.directory_open = false;
                            cx.notify();
                        }
                    },
                ),
                cx.subscribe_in(
                    &tools_page,
                    window,
                    |this: &mut App, _, event: &crate::tools_page::ToolsPageEvent, window, cx| {
                        match event {
                            crate::tools_page::ToolsPageEvent::OpenAppDm(user) => {
                                let dm = this
                                    .fixtures
                                    .dms
                                    .iter()
                                    .find(|c| c.user.as_ref() == Some(user))
                                    .map(|c| c.id.clone());
                                if let Some(dm) = dm {
                                    this.select(dm, cx);
                                } else if this.bridge.store().is_some() {
                                    this.bridge.send(Command::OpenDm {
                                        users: vec![user.clone()],
                                    });
                                } else {
                                    this.open_profile(user.clone(), cx);
                                }
                            }
                            crate::tools_page::ToolsPageEvent::OpenChannel(channel) => {
                                this.select(channel.clone(), cx);
                            }
                            crate::tools_page::ToolsPageEvent::InsertCommand(cmd) => {
                                this.tools_open = false;
                                this.sidebar.update(cx, |s, cx| s.leave_pages(cx));
                                this.composer.update(cx, |composer, cx| {
                                    composer.load_draft(cmd, cx);
                                    composer.focus_input(window, cx);
                                });
                                cx.notify();
                            }
                            crate::tools_page::ToolsPageEvent::Close => {
                                this.tools_open = false;
                                this.sidebar.update(cx, |s, cx| s.leave_pages(cx));
                                cx.notify();
                            }
                        }
                    },
                ),
                cx.subscribe_in(
                    &details,
                    window,
                    |this: &mut App, _, event: &ChannelDetailsEvent, window, cx| {
                        this.details_event(event, window, cx);
                    },
                ),
                cx.subscribe_in(
                    &compose,
                    window,
                    |this: &mut App, _, event: &NewMessageEvent, window, cx| match event {
                        NewMessageEvent::Send { channel, content } => {
                            this.compose_open = false;
                            this.select(channel.clone(), cx);
                            this.send(content.clone(), window, cx);
                        }
                        NewMessageEvent::SendWithFiles {
                            channel,
                            content,
                            files,
                        } => {
                            this.compose_open = false;
                            this.select(channel.clone(), cx);
                            this.upload_files(
                                channel.clone(),
                                None,
                                content.clone(),
                                files.clone(),
                                window,
                                cx,
                            );
                        }
                        NewMessageEvent::OpenDm { users, content } => {
                            // If one person was picked and an IM already exists, deliver directly.
                            if let [user] = users.as_slice() {
                                if let Some(dm) = this
                                    .fixtures
                                    .dms
                                    .iter()
                                    .find(|dm| {
                                        dm.kind() == ConversationKind::Im
                                            && dm.user.as_ref() == Some(user)
                                    })
                                    .map(|dm| dm.id.clone())
                                {
                                    this.compose_open = false;
                                    this.select(dm, cx);
                                    this.send(content.clone(), window, cx);
                                    return;
                                }
                            }
                            if this.bridge.store().is_some() {
                                this.pending_dm = Some(content.clone());
                                this.bridge.send(Command::OpenDm {
                                    users: users.clone(),
                                });
                            } else {
                                notify(
                                    "Needs a live workspace",
                                    "Sign in to start a new DM or group conversation.",
                                    window,
                                    cx,
                                );
                            }
                        }
                        NewMessageEvent::PickEmoji(position) => {
                            this.open_picker(PickerTarget::ComposeView, *position, window, cx);
                        }
                        NewMessageEvent::PickGif(position) => {
                            this.open_picker_tab(
                                PickerTarget::ComposeView,
                                *position,
                                PickerTab::Gifs,
                                window,
                                cx,
                            );
                        }
                        NewMessageEvent::SlashCommand(invocation) => {
                            this.run_slash(invocation, None, window, cx)
                        }
                        NewMessageEvent::Close => {
                            this.compose_open = false;
                            cx.notify();
                        }
                    },
                ),
            ];
            // Subscribe to every background workspace's EventHub so their unread/mention
            // badges and team metadata stay current while parked.
            for (ws_ix, slot) in parked_bridges.iter_mut().enumerate().skip(1) {
                if let Some(bg_bridge) = slot.as_mut() {
                    subscriptions.push(bg_bridge.subscribe_in(
                        window,
                        cx,
                        move |this: &mut App, event, window, cx| {
                            this.on_workspace_event(ws_ix, event, window, cx);
                        },
                    ));
                }
            }
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(REMINDER_POLL).await;
                    if this
                        .update_in(cx, |app, window, cx| {
                            app.fire_reminders(window, cx);
                            app.expire_focus(window, cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();

            // A first live launch has no conversations yet; hydrate brings them (reload_live).
            if !channel.as_str().is_empty() {
                bridge.send(Command::OpenConversation(channel.clone()));
            }
            demo(&results, window, cx);
            let parked = fixtures.workspaces.iter().map(|_| None).collect();
            // The fixture-derived feed lights the rail tile before any live fetch lands.
            let unread = activity.read(cx).unread_count();
            let saved_section_prefs = bridge.section_prefs();
            sidebar.update(cx, |sidebar, cx| {
                sidebar.set_activity_unread(unread, cx);
                if let Some(prefs) = saved_section_prefs {
                    sidebar.restore_section_prefs(prefs, cx);
                }
            });

            let (cached_member_counts, cached_app_counts) =
                CoreBridge::parse_channel_counts(&local);
            let cached_bots = fixtures.bots.clone();
            App {
                fixtures,
                bridge,
                parked_bridges,
                live_teams,
                bg_counts,
                bg_activity_unread,
                workspace: 0,
                parked,
                ws: WorkspaceState {
                    chosen,
                    bots: cached_bots,
                    member_counts: cached_member_counts,
                    app_counts: cached_app_counts,
                    ..Default::default()
                },
                channel,
                sidebar,
                messages,
                composer,
                thread,
                search,
                search_menu,
                search_menu_open: false,
                results,
                searching: false,
                browser,
                thread_open: false,
                tab: ConversationTab::Messages,
                huddle,
                huddle_open: false,
                huddle_session: None,
                huddle_dock: None,
                _huddle_dock_sub: None,
                huddle_invite: None,
                _huddle_invite_sub: None,
                preferences,
                prefs_window: None,
                applied_theme,
                pane_focus: 2,
                color_mode,
                saved_settings,
                profile,
                profile_open: false,
                self_state: SelfState::default(),
                status_picker,
                user_menu,
                user_menu_open: false,
                switcher,
                emoji_picker,
                picker: None,
                profile_hover: hover_card::Lifecycle::default(),
                remind_menu: None,
                image_viewer: None,
                drafts,
                drafts_open: false,
                later,
                later_open: false,
                threads,
                threads_open: false,
                unreads,
                unreads_open: false,
                unreads_fetched: HashSet::new(),
                activity,
                activity_open: false,
                files_page,
                huddles,
                huddles_open: false,
                directory,
                directory_open: false,
                tools_page,
                tools_open: false,
                compose,
                compose_open: false,
                details,
                pending_dm: None,
                pins_open: false,
                last_right_pane: None,
                canvas_search,
                canvas_query: String::new(),
                list_search,
                list_query: String::new(),
                list_sort: None,
                app_home_kit,
                fixture_bookmarks: HashMap::new(),
                os: Rc::new(platform::current()),
                badge: None,
                badge_label: None,
                last_status_bar: None,
                window_active: window.is_window_active(),
                recent_notified_tags: Vec::new(),
                window_title: String::new(),
                window_state_save: gpui::Task::ready(()),
                history_generation: 0,
                thread_generation: 0,
                reload_generation: 0,
                open_members: None,
                members_hover: (false, false),
                channel_people: None,
                connect_orgs: Vec::new(),
                back: Vec::new(),
                forward: Vec::new(),
                recent: restored_recent,
                menu_bar: None,
                typing: HashMap::new(),
                typing_expiry: gpui::Task::ready(()),
                sound: None,
                sound_fetch: gpui::Task::ready(()),
                sound_choice: None,
                focus,
                workspace_subscriptions,
                _subscriptions: subscriptions,
            }
        });
        // Route member-hover-card trigger reports (mention pills, author names) to this root.
        hover_card::init(&app, cx);
        // Clicking a macOS notification banner activates the window, opens its conversation,
        // and scrolls to + highlights the triggering message.
        let notif_app = app.downgrade();
        let notif_window = window.window_handle();
        cx.on_system_notification_response(move |response, cx| {
            let Some((ws_opt, channel, ts)) = parse_notification_target(&response.tag) else {
                return;
            };
            cx.activate(true);
            let _ = notif_window.update(cx, |_, window, cx| {
                window.activate_window();
                let _ = notif_app.update(cx, |app, cx| {
                    if let Some(ws_ix) = ws_opt {
                        if ws_ix != app.workspace {
                            app.switch_workspace(ws_ix, window, cx);
                        }
                    }
                    app.select(channel, cx);
                    app.messages.update(cx, |list, cx| list.reveal(&ts, cx));
                });
            });
        });
        // Close-to-tray (`F-SHELL-141`): when enabled, closing the main window hides the app
        // to the menu bar instead of terminating.
        let close_app = app.downgrade();
        window.on_window_should_close(cx, move |_window, cx| {
            let keep_in_tray = close_app
                .upgrade()
                .is_some_and(|a| a.read(cx).preferences.read(cx).settings().close_to_tray);
            if keep_in_tray {
                cx.hide();
                false
            } else {
                true
            }
        });
        // Poll actions clicked in the macOS menu-bar status item (`F-SHELL-168`).
        let status_app = app.downgrade();
        let status_window = window.window_handle();
        // Catch macOS swipe-based mouse Back/Forward events that GPUI drops (`phase == None`).
        {
            use futures::StreamExt as _;
            let (nav_tx, mut nav_rx) = futures::channel::mpsc::unbounded::<MouseNavDirection>();
            platform::install_mouse_nav_monitor(move |dir| {
                let _ = nav_tx.unbounded_send(dir);
            });
            let nav_app = app.downgrade();
            let nav_window = window.window_handle();
            cx.spawn(async move |cx| {
                while let Some(dir) = nav_rx.next().await {
                    let _ = nav_window.update(cx, |_, window, cx| {
                        let _ = nav_app.update(cx, |app, cx| match dir {
                            MouseNavDirection::Back => app.go_back(window, cx),
                            MouseNavDirection::Forward => app.go_forward(window, cx),
                        });
                    });
                }
            })
            .detach();
        }
        // Auto-leave active native huddle when the macOS screen locks (`F-COLLAB-059`).
        {
            use futures::StreamExt as _;
            let (lock_tx, mut lock_rx) = futures::channel::mpsc::unbounded::<()>();
            platform::install_screen_lock_monitor(move || {
                let _ = lock_tx.unbounded_send(());
            });
            let lock_app = app.downgrade();
            cx.spawn(async move |cx| {
                while lock_rx.next().await.is_some() {
                    let _ = lock_app.update(cx, |app, cx| {
                        if app.huddle_session.is_some()
                            && app.preferences.read(cx).settings().huddle_leave_on_lock
                        {
                            app.leave_huddle(cx);
                        }
                    });
                }
            })
            .detach();
        }
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let action = status_app
                    .upgrade()
                    .and_then(|a| a.read_with(cx, |app, _| app.os.take_status_bar_action()));
                let Some(action) = action else {
                    continue;
                };
                match action {
                    StatusBarAction::ShowWindow => {
                        let _ = status_window.update(cx, |_, window, cx| {
                            cx.activate(true);
                            window.activate_window();
                        });
                    }
                    StatusBarAction::SwitchWorkspace(ix) => {
                        let _ = status_window.update(cx, |_, window, cx| {
                            cx.activate(true);
                            window.activate_window();
                            let _ = status_app.update(cx, |app, cx| {
                                app.switch_workspace(ix, window, cx);
                            });
                        });
                    }
                    StatusBarAction::ToggleDnd => {
                        let _ = status_window.update(cx, |_, _, cx| {
                            let _ = status_app.update(cx, |app, cx| {
                                if app.self_state.paused() {
                                    app.self_state.paused_until = None;
                                    if !app.fixtures.sample {
                                        app.bridge.send(Command::EndDndSnooze);
                                    }
                                } else {
                                    app.self_state.paused_until =
                                        Some(crate::messages::now_secs() + 3_600);
                                    if !app.fixtures.sample {
                                        app.bridge.send(Command::SetDndSnooze { minutes: 60 });
                                    }
                                }
                                app.push_self_state(cx);
                                app.sync_badge(cx);
                            });
                        });
                    }
                    StatusBarAction::Quit => {
                        cx.update(|cx| cx.quit());
                        break;
                    }
                }
            }
        })
        .detach();
        app.update(cx, |app, cx| {
            app.push_self_state(cx);
            app.sync_saved(cx);
            app.sync_later_count(cx);
            app.sync_badge(cx);
            let channel = app.channel.clone();
            // Cold-launch frame-0 preload: read the initial channel's newest viewport page
            // synchronously (80 msgs on the read-only WAL connection, <1ms) so frame 0 renders
            // immediately without a redundant second SQLite read right after.
            let initial_history = app.bridge.history_sync(&channel);
            if !initial_history.is_empty() {
                app.resolve_named_users(&initial_history);
                let newest = initial_history.last().map(|m| m.ts.clone());
                let settings = app.preferences.read(cx).settings().clone();
                let mark_read_mode = settings.mark_read;
                app.messages.update(cx, |list, cx| {
                    list.set_conversation(channel.clone(), cx);
                    list.set_history(channel.clone(), initial_history, cx);
                    if mark_read_mode == 0 {
                        list.scroll_to_unread_once(cx);
                    }
                });
                let allow_mark =
                    mark_read_mode != 2 && (!settings.no_bg_mark_read || app.window_active);
                if allow_mark {
                    if let Some(newest) = newest {
                        app.mark_read(&channel, newest, cx);
                    }
                }
            } else {
                app.load_history(&channel, cx);
            }
            app.load_draft(cx);
            app.sync_composer_placeholder(cx);
            app.update_menus(cx);
        });
        app
    }
}

impl Render for App {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Owned color copies: `cx.theme()` borrows `cx`, so nothing may outlive the
        // statement that reads it across the `cx.listener` calls in the helpers below.
        let background = cx.theme().background;
        let foreground = cx.theme().foreground;
        // The chrome (and its corner glow) is painted once here; the title bar and rail are
        // transparent over it.
        let chrome = cx.theme().title_bar;
        let (glow_side, glow) = crate::theme::chrome_glow(cx);
        self.sync_badge(cx);

        let conversation = self.fixtures.conversation(&self.channel).cloned();
        self.sync_window_title(conversation.as_ref(), window);
        let title = conversation
            .as_ref()
            .map(|c| self.fixtures.conversation_label(c))
            .unwrap_or_else(|| self.channel.to_string());
        let topic = conversation
            .as_ref()
            .and_then(|c| c.topic.as_ref())
            .map(|t| t.value.clone())
            .unwrap_or_default();

        let title_bar = self.global_bar(cx);
        let main_column = if self.searching {
            v_flex().size_full().child(self.results.clone())
        } else if self.drafts_open {
            v_flex().size_full().child(self.drafts.clone())
        } else if self.later_open {
            v_flex().size_full().child(self.later.clone())
        } else if self.threads_open {
            v_flex().size_full().child(self.threads.clone())
        } else if self.unreads_open {
            v_flex().size_full().child(self.unreads.clone())
        } else if self.huddles_open {
            v_flex().size_full().child(self.huddles.clone())
        } else if self.directory_open {
            v_flex().size_full().child(self.directory.clone())
        } else if self.tools_open {
            v_flex().size_full().child(self.tools_page.clone())
        } else if self.compose_open {
            v_flex().size_full().child(self.compose.clone())
        } else if self.activity_open {
            // The Activity page spans the whole main area; the sidebar renders only its
            // rail while this destination is active.
            v_flex().size_full().child(self.activity.clone())
        } else if self.sidebar.read(cx).active_nav() == NavItem::Files {
            // The Files view owns the main area beside the sidebar's Files list column.
            v_flex().size_full().child(self.files_page.clone())
        } else {
            let header = self.conversation_header(&title, &topic, conversation.as_ref(), cx);
            let tabs = self.conversation_tabs(cx);
            let this = cx.weak_entity();
            // The bookmarks bar: the sample workspace's fixture set (plus links added in the
            // dialog), or the live channel's `bookmarks.list` once fetched. Like Slack, no
            // bar for a conversation with neither bookmarks nor pins.
            let (mut bookmark_list, pin_count) = if self.fixtures.sample {
                (
                    bookmarks::fixture(&self.fixtures, &self.channel),
                    Some(pins::pinned(&self.fixtures, &self.channel).len()),
                )
            } else {
                // The pin count is unknown until the channel's pins have been fetched.
                let pinned = self
                    .ws
                    .pins
                    .get(&self.channel)
                    .and_then(|loaded| loaded.as_ref().ok());
                (self.live_bookmarks(), pinned.map(Vec::len))
            };
            if let Some(added) = self.fixture_bookmarks.get(&self.channel) {
                bookmark_list.extend(added.iter().cloned());
            }
            let clicks = bookmarks::BarClicks {
                pins: Rc::new({
                    let this = this.clone();
                    move |_window, cx| {
                        let _ = this.update(cx, |app, cx| app.toggle_pins(cx));
                    }
                }),
                folder: Rc::new({
                    let this = this.clone();
                    move |folder, _window, cx| {
                        let _ = this.update(cx, |app, cx| app.open_folder_tab(folder, cx));
                    }
                }),
                add: Rc::new(move |window, cx| {
                    let _ = this.update(cx, |app, cx| app.open_link_dialog(window, cx));
                }),
            };
            let bookmark_bar = (!bookmark_list.is_empty() || pin_count.is_some_and(|n| n > 0))
                .then(|| bookmarks::bar(bookmark_list, pin_count, self.pins_open, clicks, cx));
            let column = v_flex()
                .size_full()
                .children(self.connection_banner(cx))
                .children(self.failed_mutations_banner(cx))
                .child(header);
            let column = match (&self.huddle, self.huddle_open) {
                (Some((_, bar)), true) => column.child(bar.clone()),
                _ => column,
            };
            let column = column.child(tabs).children(bookmark_bar);
            match &self.tab {
                ConversationTab::Home => {
                    let view = self.app_home_view(window, cx);
                    column.child(div().flex_1().min_h_0().child(view))
                }
                // The Pins tab keeps the transcript under it; its panel is the right column.
                ConversationTab::Messages | ConversationTab::Pins => {
                    let column = column
                        .children(self.app_dm_banner(conversation.as_ref(), cx))
                        .child(div().flex_1().min_h_0().child(self.messages.clone()));
                    // In a public channel the user is not in, Slack's Join card takes the
                    // composer's place.
                    match conversation.as_ref().filter(|c| joinable(c)) {
                        Some(joinable) => column.child(self.join_card(joinable, cx)),
                        None => column
                            .children(self.dnd_banner(conversation.as_ref(), cx))
                            .child(self.composer.clone()),
                    }
                }
                ConversationTab::Canvas => {
                    let view = if self.fixtures.sample {
                        let this_pop = cx.weak_entity();
                        let on_pop_out: crate::canvas::CanvasPopOutFn =
                            Rc::new(move |doc, window, cx| {
                                let _ = this_pop.update(cx, |app, cx| {
                                    app.pop_out_canvas(doc, None, window, cx);
                                });
                            });
                        canvas_view(
                            &self.canvas_query,
                            Some(&self.canvas_search),
                            crate::canvas::CanvasActions {
                                on_pop_out: Some(on_pop_out),
                                ..Default::default()
                            },
                            cx,
                        )
                    } else {
                        let file_id = self.ws.active_canvas.clone().or_else(|| {
                            conversation
                                .as_ref()
                                .and_then(|c| c.properties.as_ref())
                                .and_then(|p| p.canvas.as_ref())
                                .and_then(|cv| cv.file_id.as_ref())
                                .map(|f| f.to_string())
                        });
                        let loaded = file_id.as_deref().and_then(|id| self.ws.canvases.get(id));
                        let open_browser = file_id.as_ref().map(|id| {
                            let url = self.canvas_link(id);
                            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                                cx.open_url(&url);
                            }) as crate::canvas::OpenBrowserFn
                        });
                        let on_edit = file_id.as_ref().map(|id| {
                            let fid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |op, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::EditCanvas {
                                        file_id: fid.clone(),
                                        op,
                                    });
                                });
                            }) as crate::canvas::CanvasEditFn
                        });
                        let on_request_access = file_id.as_ref().map(|id| {
                            let fid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::RequestFilePermission {
                                        file_id: fid.clone(),
                                        request_type: "upgrade".to_owned(),
                                    });
                                });
                            }) as crate::canvas::OpenBrowserFn
                        });
                        let on_pop_out = file_id.as_ref().map(|id| {
                            let fid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |doc, window: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, cx| {
                                    app.pop_out_canvas(doc, Some(fid.clone()), window, cx);
                                });
                            }) as crate::canvas::CanvasPopOutFn
                        });
                        let canvas_url = file_id.as_ref().map(|id| self.canvas_link(id));
                        let on_pin_tab = file_id.as_ref().map(|_| {
                            let ch = self.channel.clone();
                            let label = loaded
                                .and_then(|res| res.as_ref().ok())
                                .map(|doc| doc.title.clone());
                            let this = cx.weak_entity();
                            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::AddConversationTab {
                                        channel: ch.clone(),
                                        tab_type: "canvas".to_owned(),
                                        label: label.clone(),
                                    });
                                });
                            }) as crate::canvas::OpenBrowserFn
                        });
                        crate::canvas::live_canvas_view(
                            file_id.as_deref().unwrap_or("live-canvas"),
                            loaded,
                            &self.canvas_query,
                            Some(&self.canvas_search),
                            crate::canvas::CanvasActions {
                                open_browser,
                                on_edit,
                                on_request_access,
                                on_pop_out,
                                on_pin_tab,
                                canvas_url,
                            },
                            cx,
                        )
                    };
                    column.child(div().flex_1().min_h_0().child(view))
                }
                ConversationTab::List => {
                    let view = if self.fixtures.sample {
                        list_view(cx)
                    } else {
                        let file_id = self.ws.active_list.clone();
                        let loaded = file_id.as_deref().and_then(|id| self.ws.lists.get(id));
                        let open_browser = file_id.as_ref().map(|id| {
                            let url = self.canvas_link(id);
                            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                                cx.open_url(&url);
                            }) as crate::canvas::OpenBrowserFn
                        });
                        let this = cx.weak_entity();
                        let on_sort: crate::lists::SortColumnFn =
                            Rc::new(move |col_ix, _window, cx| {
                                let _ = this.update(cx, |app, cx| {
                                    app.list_sort = match app.list_sort {
                                        Some((c, true)) if c == col_ix => Some((col_ix, false)),
                                        Some((c, false)) if c == col_ix => None,
                                        _ => Some((col_ix, true)),
                                    };
                                    cx.notify();
                                });
                            });
                        let on_create_item = file_id.as_ref().map(|id| {
                            let lid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |initial_fields, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::CreateListItem {
                                        list_id: lid.clone(),
                                        initial_fields,
                                    });
                                });
                            }) as crate::lists::CreateListItemFn
                        });
                        let on_update_cell = file_id.as_ref().map(|id| {
                            let lid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |row_id, cell, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::UpdateListCell {
                                        list_id: lid.clone(),
                                        row_id,
                                        cells: vec![cell],
                                    });
                                });
                            }) as crate::lists::UpdateListCellFn
                        });
                        let on_delete_item = file_id.as_ref().map(|id| {
                            let lid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |row_id, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::DeleteListItem {
                                        list_id: lid.clone(),
                                        row_id,
                                    });
                                });
                            }) as crate::lists::DeleteListItemFn
                        });
                        let on_archive_item = file_id.as_ref().map(|id| {
                            let lid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |row_id, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::ArchiveListItem {
                                        list_id: lid.clone(),
                                        row_id,
                                    });
                                });
                            }) as crate::lists::DeleteListItemFn
                        });
                        let on_set_access = file_id.as_ref().map(|id| {
                            let lid = id.clone();
                            let ch = self.channel.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |access_level, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::SetListAccess {
                                        list_id: lid.clone(),
                                        channel: Some(ch.clone()),
                                        access_level,
                                    });
                                });
                            }) as crate::lists::SetListAccessFn
                        });
                        let on_load_comments = file_id.as_ref().map(|id| {
                            let lid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |record_id, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::FetchListRecordComments {
                                        list_id: lid.clone(),
                                        record_id,
                                    });
                                });
                            }) as crate::lists::DeleteListItemFn
                        });
                        crate::lists::live_list_view(
                            loaded,
                            &self.fixtures,
                            &self.list_query,
                            Some(&self.list_search),
                            self.list_sort,
                            crate::lists::ListActions {
                                on_sort: Some(on_sort),
                                on_create_item,
                                on_update_cell,
                                on_delete_item,
                                on_archive_item,
                                on_set_access,
                                on_load_comments,
                                open_browser,
                            },
                            cx,
                        )
                    };
                    column.child(div().flex_1().min_h_0().child(view))
                }
                ConversationTab::Files => {
                    // The sample workspace lists its messages' files, newest first.
                    let sample_files: Loaded<File>;
                    let files = if self.fixtures.sample {
                        let history = self.fixtures.messages.get(&self.channel);
                        sample_files = Ok(history
                            .into_iter()
                            .flat_map(|history| history.iter().rev())
                            .flat_map(|message| message.files.iter().cloned())
                            .collect());
                        Some(&sample_files)
                    } else {
                        self.ws.files.get(&self.channel)
                    };
                    let view = tab_views::files_view(&self.fixtures, files, cx);
                    column.child(div().flex_1().min_h_0().child(view))
                }
                ConversationTab::Folder { bookmark_id, .. } => {
                    let bookmarks = self.ws.bookmarks.get(&self.channel);
                    let (on_add, on_remove): (
                        Option<tab_views::BookmarkAddFn>,
                        Option<tab_views::BookmarkRemoveFn>,
                    ) = if !self.fixtures.sample {
                        let this_add = cx.weak_entity();
                        let folder_for_add = bookmark_id.clone();
                        let ch_for_add = self.channel.clone();
                        let add_fn: tab_views::BookmarkAddFn =
                            std::rc::Rc::new(move |window, cx| {
                                let this_cb = this_add.clone();
                                let folder_cb = folder_for_add.clone();
                                let ch_cb = ch_for_add.clone();
                                bookmarks::open_add_dialog(window, cx, move |title, url, cx| {
                                    let _ = this_cb.update(cx, |app, _| {
                                        app.bridge.send(Command::AddBookmarkInFolder {
                                            channel: ch_cb.clone(),
                                            title,
                                            url,
                                            parent_id: folder_cb.clone(),
                                        });
                                    });
                                });
                            });
                        let this_rm = cx.weak_entity();
                        let ch_for_rm = self.channel.clone();
                        let rm_fn: tab_views::BookmarkRemoveFn =
                            std::rc::Rc::new(move |bm_id, cx| {
                                let _ = this_rm.update(cx, |app, _| {
                                    app.bridge.send(Command::RemoveBookmark {
                                        channel: ch_for_rm.clone(),
                                        bookmark_id: bm_id,
                                    });
                                });
                            });
                        (Some(add_fn), Some(rm_fn))
                    } else {
                        (None, None)
                    };
                    let view =
                        tab_views::folder_view(bookmark_id, bookmarks, on_add, on_remove, cx);
                    column.child(div().flex_1().min_h_0().child(view))
                }
            }
        };

        // Slack's DM list is wider than its channel sidebar, and each keeps the width it is
        // dragged to: one resizable layout (and so one remembered width) per destination.
        // The Activity page replaces the sidebar's list column.
        let nav = self.sidebar.read(cx).active_nav();
        let (layout, sidebar_width) = match nav {
            NavItem::Dms => ("workspace-dms", DM_LIST_WIDTH),
            NavItem::Activity => ("workspace-activity", SIDEBAR_WIDTH),
            _ => ("workspace", SIDEBAR_WIDTH),
        };
        let dark = cx.theme().mode.is_dark();
        let outline = cx.theme().sidebar_border;
        let side_panel = self.thread_open || self.profile_open || self.pins_open;
        // Like Slack, opening the thread / profile / pins panel narrows the conversation,
        // never the sidebar, and resizing the window only resizes the conversation: the
        // sidebar and the side panel are fixed at the widths Slack gives them.
        // Slack draws its own dividers (hairlines in dark mode, shadows in light), so the kit's
        // handle lines stay unpainted; the handles still resize.
        let mut panels = h_resizable(layout).with_handle_appearance(Rc::new(
            |_: &gpui_kit::base::ResizeHandleContext, _: &mut Window, _: &mut gpui::App| {
                Some(div().into_any_element())
            },
        ));
        if nav != NavItem::Activity {
            let sidebar_col = v_flex()
                .size_full()
                .child(div().flex_1().min_h_0().child(self.sidebar.clone()))
                .when_some(self.huddle_dock.clone(), |col, dock| col.child(dock));
            panels = panels.child(
                resizable_panel()
                    .size(sidebar_width)
                    .size_range(px(288.)..px(728.))
                    .fixed()
                    .child(sidebar_col),
            );
        }
        let mut panels = panels.child(
            resizable_panel().child(
                div()
                    .relative()
                    .size_full()
                    .bg(background)
                    .when(!side_panel, |column| {
                        column
                            .rounded_tr(px(PANEL_RADIUS))
                            .rounded_br(px(PANEL_RADIUS))
                    })
                    .child(main_column)
                    // Light Slack's side panel casts its shadow over the conversation's edge.
                    .when(!dark && side_panel, |column| {
                        column.child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right_0()
                                .w(px(16.))
                                .bg(crate::theme::panel_shadow_edge()),
                        )
                    }),
            ),
        );
        // Dark Slack's side panel starts with a hairline; light Slack's is lifted by its shadow.
        let side_pane = |pane: AnyElement| {
            div()
                .size_full()
                .when(dark, |pane| pane.border_l_1().border_color(outline))
                .child(pane)
        };
        // A profile opened from the thread stacks over it.
        if self.profile_open {
            panels = panels.child(
                resizable_panel()
                    .size(FLEXPANE_WIDTH)
                    .size_range(px(340.)..px(720.))
                    .fixed()
                    .child(side_pane(self.profile.clone().into_any_element())),
            );
        } else if self.thread_open {
            panels = panels.child(
                resizable_panel()
                    .size(FLEXPANE_WIDTH)
                    .size_range(px(340.)..px(720.))
                    .fixed()
                    .child(side_pane(self.thread.clone().into_any_element())),
            );
        } else if self.pins_open {
            let this = cx.weak_entity();
            let close = move |_: &mut Window, cx: &mut gpui::App| {
                this.update(cx, |app, cx| {
                    app.pins_open = false;
                    if app.tab == ConversationTab::Pins {
                        app.tab = ConversationTab::Messages;
                    }
                    cx.notify();
                })
                .ok();
            };
            let sample_pins: Loaded<Message>;
            let pinned = if self.fixtures.sample {
                sample_pins = Ok(pins::pinned(&self.fixtures, &self.channel)
                    .into_iter()
                    .cloned()
                    .collect());
                Some(&sample_pins)
            } else {
                self.ws.pins.get(&self.channel)
            };
            let panel = pins::pins_panel(&self.fixtures, pinned, &title, close, cx);
            panels = panels.child(
                resizable_panel()
                    .size(FLEXPANE_WIDTH)
                    .size_range(px(340.)..px(720.))
                    .fixed()
                    .child(side_pane(panel.into_any_element())),
            );
        }

        let (switcher, rail) = self.sidebar.update(cx, |sidebar, cx| {
            (sidebar.workspace_switcher(cx), sidebar.tab_rail(cx))
        });

        v_flex()
            .id("root")
            .key_context("App")
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|this, _: &QuickSwitch, window, cx| this.open_switcher(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenPreferences, window, cx| {
                this.open_preferences(None, window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &ShowShortcuts, window, cx| this.open_shortcuts(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenDms, _, cx| this.show_nav(NavItem::Dms, cx)))
            .on_action(
                cx.listener(|this, _: &OpenActivity, _, cx| this.show_nav(NavItem::Activity, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenDrafts, _, cx| this.open_drafts(cx)))
            .on_action(
                cx.listener(|this, _: &OpenChannelBrowser, window, cx| {
                    this.open_browser(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &OpenDownloads, window, cx| this.open_downloads(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SetStatus, window, cx| this.open_status_picker(window, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.search.read(cx).focus_handle(cx).focus(window, cx);
                this.open_search_menu(cx);
            }))
            // Only with somewhere to go, so the menu bar greys out Back / Forward as Slack's does.
            .when(!self.back.is_empty(), |root| {
                root.on_action(
                    cx.listener(|this, _: &NavigateBack, window, cx| this.go_back(window, cx)),
                )
            })
            .when(!self.forward.is_empty(), |root| {
                root.on_action(
                    cx.listener(|this, _: &NavigateForward, window, cx| {
                        this.go_forward(window, cx)
                    }),
                )
            })
            .on_action(cx.listener(|this, switch: &SwitchWorkspace, window, cx| {
                this.switch_workspace(switch.0, window, cx)
            }))
            .on_action(cx.listener(|this, tab: &SelectConversationTab, _, cx| {
                this.select_conversation_tab(tab.0, cx)
            }))
            .on_action(cx.listener(|this, _: &NextWorkspace, window, cx| {
                let count = this.parked.len().max(1);
                this.switch_workspace((this.workspace + 1) % count, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousWorkspace, window, cx| {
                let count = this.parked.len().max(1);
                this.switch_workspace((this.workspace + count - 1) % count, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &NewMessage, window, cx| this.open_compose("", window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenThreads, _, cx| this.open_threads(cx)))
            .on_action(
                cx.listener(|this, _: &OpenUnreads, window, cx| this.open_unreads(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenLater, _, cx| this.open_later(cx)))
            .on_action(cx.listener(|this, _: &OpenFiles, _, cx| this.open_files(cx)))
            .on_action(cx.listener(|this, _: &OpenPeople, _, cx| this.open_directory(cx)))
            .on_action(cx.listener(|this, _: &OpenOwnProfile, _, cx| {
                this.open_profile(this.fixtures.self_user.clone(), cx)
            }))
            .on_action(
                cx.listener(|this, open: &OpenRecent, _, cx| this.select(open.0.clone(), cx)),
            )
            .on_action(cx.listener(|this, sign_out: &SignOut, _, cx| this.sign_out(sign_out.0, cx)))
            .on_action(cx.listener(|this, _: &PreviousConversation, _, cx| {
                this.step_conversation(false, false, cx)
            }))
            .on_action(cx.listener(|this, _: &NextConversation, _, cx| {
                this.step_conversation(true, false, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousUnread, _, cx| {
                this.step_conversation(false, true, cx)
            }))
            .on_action(
                cx.listener(|this, _: &NextUnread, _, cx| this.step_conversation(true, true, cx)),
            )
            .on_action(cx.listener(|this, _: &MarkConversationRead, _, cx| this.mark_open_read(cx)))
            // A focused field passes Esc on when it has nothing to close (the composer's menu,
            // extra cursors): that marks the conversation read too, as in Slack, and Esc still
            // reaches the listeners outside the root (an open dialog's).
            .on_action(cx.listener(|this, _: &InputEscape, _, cx| {
                this.mark_open_read(cx);
                cx.propagate();
            }))
            // Slack's one-key message shortcuts (E, Delete, T/Right, F, P, A, U, M, R), all
            // through the transcript's shared [`MessageAction`] handlers.
            .on_action(cx.listener(|this, _: &FocusEditMessage, window, cx| {
                this.focus_message(MessageAction::Edit, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusDeleteMessage, window, cx| {
                this.focus_message(MessageAction::Delete, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusReplyInThread, window, cx| {
                this.focus_message(MessageAction::ReplyInThread, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusForwardMessage, window, cx| {
                this.focus_message(MessageAction::Forward, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusTogglePin, window, cx| {
                this.focus_message(MessageAction::TogglePin, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusSaveMessage, window, cx| {
                this.focus_message(MessageAction::SaveForLater, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusMarkMessageUnread, window, cx| {
                this.focus_message(MessageAction::MarkUnread, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &FocusRemindMe, window, cx| {
                    this.open_remind_menu(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &FocusReact, window, cx| {
                this.focus_message(
                    MessageAction::PickReaction(window.mouse_position()),
                    window,
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &MarkAllRead, window, cx| {
                this.request_mark_all_read(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.step_zoom(Some(1), cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.step_zoom(Some(-1), cx)))
            .on_action(cx.listener(|this, _: &ActualSize, _, cx| this.step_zoom(None, cx)))
            .on_action(cx.listener(|this, _: &SearchConversation, window, cx| {
                this.search_in_conversation(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleWorkspaceRail, window, cx| {
                this.toggle_workspace_rail(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleRightPane, _, cx| this.toggle_right_pane(cx)))
            .on_action(cx.listener(|this, _: &JumpToUnread, _, cx| this.jump_to_unread(cx)))
            .on_action(cx.listener(|this, _: &OpenHome, _, cx| this.open_home(cx)))
            .on_action(cx.listener(|this, _: &CreateChannelAction, window, cx| {
                this.open_create_channel(window, cx)
            }))
            .on_action(cx.listener(|this, _: &PopOutConversation, window, cx| {
                let ch = this.channel.clone();
                this.pop_out_conversation(ch, window, cx);
            }))
            .on_action(cx.listener(|this, _: &UnsendLastMessage, window, cx| {
                this.unsend_last_message(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextNavTab, _, cx| {
                this.sidebar.update(cx, |s, cx| s.cycle_nav(true, cx));
            }))
            .on_action(cx.listener(|this, _: &PrevNavTab, _, cx| {
                this.sidebar.update(cx, |s, cx| s.cycle_nav(false, cx));
            }))
            .on_action(cx.listener(|this, _: &FocusNextPane, window, cx| {
                this.cycle_pane_focus(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPrevPane, window, cx| {
                this.cycle_pane_focus(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &BrowseUp, _, cx| {
                if this.activity_open {
                    this.activity.update(cx, |a, cx| a.step(false, cx));
                } else if this.showing_transcript() {
                    this.messages.update(cx, |m, cx| m.step_focus(false, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &BrowseDown, _, cx| {
                if this.activity_open {
                    this.activity.update(cx, |a, cx| a.step(true, cx));
                } else if this.showing_transcript() {
                    this.messages.update(cx, |m, cx| m.step_focus(true, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &BrowseOpen, window, cx| {
                if this.activity_open {
                    this.activity.update(cx, |a, cx| a.open_selected(cx));
                } else if this.showing_transcript() {
                    this.focus_message(MessageAction::ReplyInThread, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ExitThread, window, cx| {
                if this.thread_open {
                    this.thread_open = false;
                    this.messages
                        .update(cx, |list, cx| list.set_open_thread(None, cx));
                    this.focus.focus(window, cx);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &JumpToOldest, _, cx| {
                if this.showing_transcript() {
                    this.messages
                        .update(cx, |m, cx| m.jump(crate::messages::Jump::Beginning, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &JumpToNewest, _, cx| {
                if this.showing_transcript() {
                    this.messages
                        .update(cx, |m, cx| m.jump(crate::messages::Jump::Recent, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &OpenMore, _, cx| {
                this.open_directory(cx);
            }))
            .on_action(cx.listener(|this, _: &NextSection, _, cx| {
                if let Some(next) = this.sidebar.read(cx).step_section(&this.channel, true) {
                    this.select(next, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PrevSection, _, cx| {
                if let Some(prev) = this.sidebar.read(cx).step_section(&this.channel, false) {
                    this.select(prev, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleHuddleMute, _, cx| {
                this.toggle_huddle_mute(cx);
            }))
            .relative()
            .size_full()
            .bg(chrome)
            .text_color(foreground)
            // Window-level capture for hardware mouse Back/Forward buttons (buttons 3 & 4 and
            // swipe-ended events). Registered via `window.on_mouse_event` rather than an element
            // hitbox so it works unconditionally even over `.occlude()` overlays or dialogs.
            .child({
                let nav_app = cx.weak_entity();
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let nav_app = nav_app.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            match event.button {
                                MouseButton::Navigate(NavigationDirection::Back) => {
                                    let _ = nav_app.update(cx, |app, cx| app.go_back(window, cx));
                                    cx.stop_propagation();
                                }
                                MouseButton::Navigate(NavigationDirection::Forward) => {
                                    let _ =
                                        nav_app.update(cx, |app, cx| app.go_forward(window, cx));
                                    cx.stop_propagation();
                                }
                                _ => {}
                            }
                        });
                    },
                )
                .absolute()
                .size_0()
            })
            .child(div().absolute().top_0().left_0().size(glow_side).bg(glow))
            .child(title_bar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(
                        {
                            let s = self.preferences.read(cx).settings();
                            s.show_workspace_rail && !s.simplified_layout
                        },
                        |row| row.child(switcher),
                    )
                    // Slack's workspace panel: the tab rail and the tab panel (sidebar,
                    // conversation, side panel) in one rounded card inset from the window's
                    // right and bottom edges, lifted by a shadow in light mode and outlined in
                    // dark mode.
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .mr_1()
                            .mb_1()
                            .rounded(px(PANEL_RADIUS))
                            .when(!dark, |panel| panel.shadow(vec![shadow(-4., 0.09, 16.)]))
                            .child(rail)
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .rounded_tr(px(PANEL_RADIUS))
                                    .rounded_br(px(PANEL_RADIUS))
                                    .overflow_hidden()
                                    .when(dark, |panel| panel.border_1().border_color(outline))
                                    .child(panels)
                                    .when_some(self.huddle_invite.clone(), |panel, banner| {
                                        panel.child(
                                            div()
                                                .id("huddle-invite-float")
                                                .occlude()
                                                .absolute()
                                                .top_4()
                                                .right_4()
                                                .child(banner),
                                        )
                                    })
                                    .when(
                                        nav == NavItem::Activity && self.huddle_dock.is_some(),
                                        |panel| {
                                            panel.child(
                                                div()
                                                    .id("huddle-dock-activity-float")
                                                    .occlude()
                                                    .absolute()
                                                    .left_4()
                                                    .bottom_4()
                                                    .w(px(280.))
                                                    .rounded_lg()
                                                    .overflow_hidden()
                                                    .shadow_lg()
                                                    .children(self.huddle_dock.clone()),
                                            )
                                        },
                                    )
                                    .when(side_panel, |panel| {
                                        let outline = dark.then_some(outline);
                                        panel
                                            .children(corner_cap(true, chrome, outline))
                                            .children(corner_cap(false, chrome, outline))
                                    }),
                            ),
                    ),
            )
            .when(self.user_menu_open, |root| {
                root.child(
                    div()
                        .id("user-menu-scrim")
                        .occlude()
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.user_menu_open = false;
                            this.user_menu.update(cx, |menu, cx| menu.closed(cx));
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("user-menu")
                        .occlude()
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .absolute()
                        // Where Slack drops it: beside the tab rail's avatar.
                        .bottom(px(24.))
                        .left(px(124.))
                        .child(self.user_menu.clone()),
                )
            })
            .when_some(
                self.picker.as_ref().map(|(_, position)| *position),
                |root, position| {
                    // Grow away from the nearest window edges: up from low clicks (composer),
                    // leftwards from clicks on the right (hover bars).
                    let viewport = window.viewport_size();
                    let anchor = match (
                        position.y > viewport.height / 2.,
                        position.x > viewport.width / 2.,
                    ) {
                        (true, true) => Anchor::BottomRight,
                        (true, false) => Anchor::BottomLeft,
                        (false, true) => Anchor::TopRight,
                        (false, false) => Anchor::TopLeft,
                    };
                    root.child(
                        div()
                            .id("emoji-picker-scrim")
                            .occlude()
                            .on_scroll(|_, _, cx| cx.stop_propagation())
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_picker(window, cx)),
                            ),
                    )
                    .child(deferred(
                        anchored()
                            .position(position)
                            .anchor(anchor)
                            .snap_to_window_with_margin(Edges::all(px(8.)))
                            .child(
                                div()
                                    .id("emoji-picker-overlay")
                                    .occlude()
                                    .on_scroll(|_, _, cx| cx.stop_propagation())
                                    .child(self.emoji_picker.clone()),
                            ),
                    ))
                },
            )
            .when_some(
                self.remind_menu
                    .as_ref()
                    .map(|(menu, position, _)| (menu.clone(), *position)),
                |root, (menu, position)| {
                    // Slack drops the reminder menu beside the message; the mouse is the
                    // nearest fixed point the focused message has.
                    root.child(deferred(
                        anchored()
                            .position(position)
                            .anchor(Anchor::TopLeft)
                            .snap_to_window_with_margin(Edges::all(px(8.)))
                            .child(
                                div()
                                    .id("remind-menu-overlay")
                                    .occlude()
                                    .on_scroll(|_, _, cx| cx.stop_propagation())
                                    .child(menu),
                            ),
                    ))
                },
            )
            .when_some(self.profile_hover.card.clone(), |root, trigger| {
                // Slack's member hover card, anchored at its trigger: above one in the
                // lower half of the viewport, below one in the upper half.
                let this = cx.weak_entity();
                let user = trigger.user.clone();
                let actions = hover_card::CardActions {
                    open_profile: {
                        let this = this.clone();
                        Rc::new(move |_window, cx| {
                            let _ = this.update(cx, |app, cx| {
                                app.close_hover_card(cx);
                                app.open_profile(user.clone(), cx);
                            });
                        })
                    },
                    message: {
                        let (this, user) = (this.clone(), trigger.user.clone());
                        Rc::new(move |window, cx| {
                            let _ = this.update(cx, |app, cx| app.message_user(&user, window, cx));
                        })
                    },
                    set_status: {
                        let this = this.clone();
                        Rc::new(move |window, cx| {
                            let _ = this.update(cx, |app, cx| {
                                app.close_hover_card(cx);
                                app.open_status_picker(window, cx);
                            });
                        })
                    },
                    hovered: {
                        let this = this.clone();
                        Rc::new(move |inside, _window, cx| {
                            let _ = this.update(cx, |app, cx| app.hover_card_hovered(inside, cx));
                        })
                    },
                    dismiss: {
                        let this = this.clone();
                        Rc::new(move |_window, cx| {
                            let _ = this.update(cx, |app, cx| app.close_hover_card(cx));
                        })
                    },
                };
                let (anchor, position) =
                    hover_card::anchor_for(&trigger.anchor, window.viewport_size());
                root.child(deferred(
                    anchored()
                        .position(position)
                        .anchor(anchor)
                        .snap_to_window_with_margin(Edges::all(px(8.)))
                        .child(hover_card::card(
                            &self.fixtures,
                            &self.self_state,
                            &trigger.user,
                            &actions,
                            cx,
                        )),
                ))
            })
            .when(self.search_menu_open, |root| {
                let bounds = self.search_menu.read(cx).pill_bounds();
                root.when_some(
                    (bounds.size.width > px(0.)).then_some(bounds),
                    |root, bounds| {
                        // Slack's search panel: hangs over the pill from its top-left
                        // corner, just as wide, above every pane. A press outside it (and
                        // outside the pill) folds it.
                        root.child(deferred(
                            anchored()
                                .position(bounds.origin)
                                .anchor(Anchor::TopLeft)
                                .snap_to_window_with_margin(Edges::all(px(8.)))
                                .child(
                                    div()
                                        .id("search-menu-overlay")
                                        .occlude()
                                        .on_scroll(|_, _, cx| cx.stop_propagation())
                                        .w(bounds.size.width)
                                        .on_mouse_down_out(cx.listener(
                                            |this, event: &MouseDownEvent, window, cx| {
                                                if !this
                                                    .search_menu
                                                    .read(cx)
                                                    .pill_bounds()
                                                    .contains(&event.position)
                                                {
                                                    this.close_search_menu(window, cx);
                                                }
                                            },
                                        ))
                                        .child(self.search_menu.clone()),
                                ),
                        ))
                    },
                )
            })
            .when_some(self.image_viewer.clone(), |root, viewer| {
                // Slack's image viewer: an opaque gray surface covering the window below
                // the title bar; the panel itself renders the viewer.
                root.child(
                    div()
                        .id("image-viewer-overlay")
                        .occlude()
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .absolute()
                        .top(px(40.))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .child(viewer),
                )
            })
    }
}

/// Build the views of workspace `index` from its fixtures, opened on `channel`.
fn build_workspace(
    fixtures: &Arc<Fixtures>,
    index: usize,
    archive: Option<SharedStore>,
    channel: &ChannelId,
    window: &mut Window,
    cx: &mut gpui::App,
) -> WorkspaceViews {
    let sidebar = Sidebar::new(fixtures.clone(), channel.clone(), window, cx);
    let messages = MessageList::new(fixtures.clone(), channel.clone(), window, cx);
    let thread = ThreadPanel::new(fixtures.clone(), window, cx);
    let results = SearchView::new(fixtures.clone(), archive, cx);
    let browser = ChannelBrowser::new(fixtures.clone(), window, cx);
    let huddle = huddle::fixture(fixtures).map(|huddle| {
        let channel = huddle.channel.clone();
        (channel, HuddleBar::new(fixtures.clone(), huddle, cx))
    });
    let huddle_channel = huddle.as_ref().map(|(channel, _)| channel.clone());
    sidebar.update(cx, |sidebar, cx| {
        sidebar.set_huddle(huddle_channel, cx);
        sidebar.set_workspace(index, cx);
    });
    let profile = ProfilePanel::new(fixtures.clone(), cx);
    let switcher = QuickSwitcher::new(fixtures.clone(), window, cx);
    WorkspaceViews {
        sidebar,
        messages,
        thread,
        results,
        browser,
        huddle,
        profile,
        switcher,
    }
}

/// The root's subscriptions to one workspace's views. Handlers always act on the shown
/// workspace's fields; parked views are not rendered, so they emit nothing.
fn subscribe_workspace(
    views: &WorkspaceViews,
    window: &mut Window,
    cx: &mut Context<App>,
) -> Vec<Subscription> {
    let mut subscriptions = vec![
        cx.subscribe_in(
            &views.sidebar,
            window,
            |this: &mut App, _, event: &SidebarEvent, window, cx| match event {
                SidebarEvent::SelectConversation(channel) => this.select(channel.clone(), cx),
                SidebarEvent::BrowseChannels => this.open_browser(window, cx),
                SidebarEvent::CreateChannel => this.open_create_channel(window, cx),
                SidebarEvent::SwitchWorkspace(index) => this.switch_workspace(*index, window, cx),
                SidebarEvent::ReorderWorkspace { from, to } => {
                    this.reorder_workspace(*from, *to, window, cx)
                }
                SidebarEvent::OpenInNewWindow(channel) => {
                    this.pop_out_conversation(channel.clone(), window, cx)
                }
                SidebarEvent::SectionPrefsChanged => {
                    let prefs = this.sidebar.read(cx).export_section_prefs();
                    if let Some(write) = this.bridge.remember_section_prefs(&prefs) {
                        cx.background_executor().spawn(write).detach();
                    }
                }
                SidebarEvent::OpenDrafts => this.open_drafts(cx),
                SidebarEvent::OpenLater => this.open_later(cx),
                SidebarEvent::OpenThreads => this.open_threads(cx),
                SidebarEvent::OpenUnreads => this.open_unreads(window, cx),
                SidebarEvent::OpenActivity => this.open_activity(cx),
                SidebarEvent::LeftActivity => {
                    this.activity_open = false;
                    cx.notify();
                }
                SidebarEvent::OpenFiles => this.open_files(cx),
                SidebarEvent::OpenTools => this.open_tools(cx),
                SidebarEvent::OpenAdmin => this.open_admin(window, cx),
                SidebarEvent::ShowFilesSection(section) => this
                    .files_page
                    .update(cx, |page, cx| page.show_section(*section, window, cx)),
                SidebarEvent::OpenHuddles => this.open_huddles(cx),
                SidebarEvent::OpenDirectories => this.open_directory(cx),
                SidebarEvent::OpenCompose => this.open_compose("", window, cx),
                SidebarEvent::CreateSection { name } => this.create_section(name.clone(), cx),
                SidebarEvent::CreateSectionWithChannel { name, channel } => {
                    if this.fixtures.sample {
                        this.sidebar.update(cx, |sidebar, cx| {
                            sidebar.add_section_with_channel(name.clone(), channel.clone(), cx)
                        });
                    } else {
                        this.bridge.send(Command::CreateSectionWithChannel {
                            name: name.clone(),
                            channel: channel.clone(),
                        });
                    }
                }
                SidebarEvent::RenameSection { section, name } => {
                    if this.fixtures.sample {
                        this.sidebar.update(cx, |sidebar, cx| {
                            sidebar.rename_section_local(section, name, cx)
                        });
                    } else {
                        this.bridge.send(Command::RenameSection {
                            section: section.clone(),
                            name: name.clone(),
                        });
                    }
                }
                SidebarEvent::DeleteSection { section } => {
                    if this.fixtures.sample {
                        this.sidebar
                            .update(cx, |sidebar, cx| sidebar.delete_section_local(section, cx));
                    } else {
                        this.bridge.send(Command::DeleteSection {
                            section: section.clone(),
                        });
                    }
                }
                SidebarEvent::MoveChannel { channel, from, to } => {
                    this.bridge.send(Command::MoveChannel {
                        channel: channel.clone(),
                        from: from.clone(),
                        to: to.clone(),
                    });
                }
                SidebarEvent::MarkRead { channel, ts } => {
                    this.mark_read(channel, ts.clone(), cx);
                }
                SidebarEvent::SetNotifyLevel { channel, level } => {
                    this.bridge.send(Command::SetNotifyLevel {
                        channel: channel.clone(),
                        level: *level,
                    });
                }
                SidebarEvent::OpenChannelDetails(channel) => {
                    this.open_channel_details(
                        channel,
                        channel_details::DetailsTab::About,
                        window,
                        cx,
                    );
                }
                SidebarEvent::LeaveChannel(channel) => {
                    this.leave(channel.clone(), window, cx);
                }
                SidebarEvent::ToggleFocus => this.toggle_focus(window, cx),
                SidebarEvent::FocusDuration(preset) => {
                    let auto_status = this.preferences.read(cx).settings().focus_auto_status;
                    this.self_state
                        .start_focus_with_status(*preset, auto_status);
                    if !this.fixtures.sample {
                        let minutes = crate::status::FOCUS_DURATIONS
                            .get(*preset)
                            .and_then(|(_, secs)| *secs)
                            .map(|secs| ((secs + 59) / 60).clamp(1, 1440) as u32)
                            .unwrap_or(480);
                        this.bridge.send(Command::SetDndSnooze { minutes });
                        if auto_status {
                            this.send_status_to_slack(this.self_state.status.as_ref());
                        }
                    }
                    this.apply_preferences(window, cx);
                }
                SidebarEvent::OpenUserMenu => {
                    this.user_menu_open = !this.user_menu_open;
                    cx.notify();
                }
            },
        ),
        cx.subscribe(
            &views.results,
            |this: &mut App, _, event: &SearchEvent, cx| match event {
                SearchEvent::Close => {
                    this.searching = false;
                    cx.notify();
                }
                SearchEvent::Open { channel, ts } => {
                    this.select(channel.clone(), cx);
                    this.messages.update(cx, |list, cx| list.reveal(ts, cx));
                }
                SearchEvent::OpenChannel(channel) => {
                    this.select(channel.clone(), cx);
                }
                SearchEvent::OpenProfile(user) => {
                    this.open_profile(user.clone(), cx);
                }
                SearchEvent::RemoteMessages { query, sort, page } => {
                    this.bridge.send(Command::SearchRemoteMessages {
                        query: query.clone(),
                        sort: *sort,
                        page: *page,
                    });
                }
                SearchEvent::RemoteFiles { query, sort, page } => {
                    this.bridge.send(Command::SearchRemoteFiles {
                        query: query.clone(),
                        sort: *sort,
                        page: *page,
                    });
                }
            },
        ),
        cx.subscribe_in(
            &views.browser,
            window,
            |this: &mut App, _, event: &ChannelBrowserEvent, window, cx| {
                let channel = match event {
                    ChannelBrowserEvent::Join(channel) => {
                        if let Some(joined) = this.fixtures.conversation(channel).cloned() {
                            this.sidebar
                                .update(cx, |sidebar, cx| sidebar.add_channel(joined, cx));
                        }
                        channel
                    }
                    ChannelBrowserEvent::Open(channel) => channel,
                };
                window.close_dialog(cx);
                this.select(channel.clone(), cx);
            },
        ),
        cx.subscribe_in(
            &views.messages,
            window,
            |this: &mut App, _, event: &MessageListEvent, window, cx| match event {
                MessageListEvent::OpenThread { channel, thread_ts } => {
                    this.open_thread(channel.clone(), thread_ts.clone(), cx);
                }
                MessageListEvent::Later(request) => this.save_for_later(request, cx),
                MessageListEvent::Changed { channel, change } => {
                    this.post_change(channel, change);
                    if let MessageChange::MarkUnread { ts } = change {
                        this.apply_local_mark_unread(channel, ts.clone(), cx);
                    }
                }
                MessageListEvent::OpenProfile(user) => this.open_profile(user.clone(), cx),
                MessageListEvent::OpenLink(target) => this.open_link(target.clone(), window, cx),
                MessageListEvent::OpenImage {
                    channel,
                    images,
                    index,
                } => this.open_image_viewer(channel.clone(), images.clone(), *index, window, cx),
                MessageListEvent::Forward(link) => this.open_compose(link, window, cx),
                MessageListEvent::JoinHuddle(channel) => {
                    if this.channel != *channel {
                        this.select(channel.clone(), cx);
                    }
                    this.start_huddle(window, cx);
                }
                MessageListEvent::ViewHistory { channel, ts } => {
                    this.open_revision_history(channel.clone(), ts.clone(), window, cx)
                }
                MessageListEvent::AddToList { channel, ts } => {
                    this.open_add_to_list_dialog(channel.clone(), ts.clone(), window, cx)
                }
                MessageListEvent::NeedHistorySince { channel, since } => {
                    this.load_since(channel, *since, cx);
                }
                MessageListEvent::NeedOlder { channel, before } => {
                    this.load_older(channel, before.clone(), cx);
                }
                MessageListEvent::PickReaction {
                    channel,
                    ts,
                    position,
                } => {
                    let target = PickerTarget::Message {
                        channel: channel.clone(),
                        ts: ts.clone(),
                    };
                    this.open_picker(target, *position, window, cx);
                }
                MessageListEvent::PickEditEmoji(position) => {
                    this.open_picker(PickerTarget::MessageEdit, *position, window, cx);
                }
            },
        ),
        cx.subscribe_in(
            &views.thread,
            window,
            |this: &mut App, _, event: &ThreadPanelEvent, window, cx| match event {
                ThreadPanelEvent::PickReaction { ts, position } => {
                    let target = PickerTarget::ThreadMessage(ts.clone());
                    this.open_picker(target, *position, window, cx);
                }
                ThreadPanelEvent::PickEmoji(position) => {
                    this.open_picker(PickerTarget::ThreadComposer, *position, window, cx);
                }
                ThreadPanelEvent::PickGif(position) => {
                    this.open_picker_tab(
                        PickerTarget::ThreadComposer,
                        *position,
                        PickerTab::Gifs,
                        window,
                        cx,
                    );
                }
                ThreadPanelEvent::PickEditEmoji(position) => {
                    this.open_picker(PickerTarget::ThreadEdit, *position, window, cx);
                }
                ThreadPanelEvent::Close => {
                    this.close_thread(cx);
                    cx.notify();
                }
                ThreadPanelEvent::OpenProfile(user) => this.open_profile(user.clone(), cx),
                ThreadPanelEvent::Typing => {
                    if let Some((channel, ts)) = this.thread.read(cx).open_thread() {
                        this.bridge.send(Command::Typing {
                            channel,
                            thread_ts: Some(ts),
                        });
                    }
                }
                ThreadPanelEvent::OpenLink(target) => this.open_link(target.clone(), window, cx),
                ThreadPanelEvent::OpenImage {
                    channel,
                    images,
                    index,
                } => this.open_image_viewer(channel.clone(), images.clone(), *index, window, cx),
                ThreadPanelEvent::Forward(link) => this.open_compose(link, window, cx),
                ThreadPanelEvent::ViewHistory { channel, ts } => {
                    this.open_revision_history(channel.clone(), ts.clone(), window, cx)
                }
                ThreadPanelEvent::AddToList { channel, ts } => {
                    this.open_add_to_list_dialog(channel.clone(), ts.clone(), window, cx)
                }
                ThreadPanelEvent::SendToChannel {
                    channel,
                    thread_ts,
                    content,
                } => {
                    this.bridge.send(Command::SendMessage {
                        channel: channel.clone(),
                        content: content.clone(),
                        thread_ts: Some(thread_ts.clone()),
                        broadcast: true,
                    });
                    window.push_notification("Sent reply to channel", cx);
                }
                ThreadPanelEvent::Reply {
                    channel,
                    thread_ts,
                    content,
                    broadcast,
                } => {
                    this.bridge.send(Command::SendMessage {
                        channel: channel.clone(),
                        content: content.clone(),
                        thread_ts: Some(thread_ts.clone()),
                        broadcast: *broadcast,
                    });
                }
                ThreadPanelEvent::ReplyWithFiles {
                    channel,
                    thread_ts,
                    content,
                    files,
                } => {
                    this.upload_files(
                        channel.clone(),
                        Some(thread_ts.clone()),
                        content.clone(),
                        files.clone(),
                        window,
                        cx,
                    );
                }
                ThreadPanelEvent::SlashCommand(invocation) => {
                    let thread = this.thread.read(cx).open_thread();
                    this.run_slash(invocation, thread, window, cx)
                }
                ThreadPanelEvent::Schedule {
                    channel,
                    thread_ts,
                    text,
                    when,
                } => {
                    this.schedule_at(
                        channel.clone(),
                        Some(thread_ts.clone()),
                        text.clone(),
                        when.at(now_secs()),
                        when.label().to_lowercase(),
                        window,
                        cx,
                    );
                }
                ThreadPanelEvent::ScheduleCustom {
                    channel,
                    thread_ts,
                    text,
                } => {
                    this.open_custom_schedule(
                        channel.clone(),
                        Some(thread_ts.clone()),
                        text.clone(),
                        window,
                        cx,
                    );
                }
                ThreadPanelEvent::PopOut { channel, thread_ts } => {
                    this.pop_out_thread(channel.clone(), thread_ts.clone(), window, cx);
                }
                ThreadPanelEvent::Later(request) => this.save_for_later(request, cx),
                ThreadPanelEvent::Changed { channel, change } => this.post_change(channel, change),
                ThreadPanelEvent::SetSubscribed {
                    channel,
                    thread_ts,
                    last_read,
                    subscribed,
                } => this.bridge.send(Command::SetThreadSubscribed {
                    channel: channel.clone(),
                    thread_ts: thread_ts.clone(),
                    last_read: last_read.clone(),
                    subscribed: *subscribed,
                }),
                ThreadPanelEvent::MarkThreadRead {
                    channel,
                    thread_ts,
                    ts,
                } => {
                    this.mark_thread_unread(channel.clone(), thread_ts.clone(), ts.clone(), cx);
                }
            },
        ),
        cx.subscribe_in(
            &views.profile,
            window,
            |this: &mut App, _, event: &ProfileEvent, window, cx| match event {
                // Slack's close shuts the whole pane, the thread under the profile too.
                ProfileEvent::Close => {
                    this.profile_open = false;
                    this.close_thread(cx);
                    cx.notify();
                }
                ProfileEvent::Back => {
                    this.profile_open = false;
                    let root = this.thread.read(cx).open_thread().map(|(_, ts)| ts);
                    this.messages
                        .update(cx, |list, cx| list.set_open_thread(root, cx));
                    cx.notify();
                }
                ProfileEvent::Message(user) => {
                    let dm = this
                        .fixtures
                        .dms
                        .iter()
                        .find(|c| c.user.as_ref() == Some(user))
                        .map(|c| c.id.clone());
                    if let Some(dm) = dm {
                        this.select(dm, cx);
                    }
                }
                ProfileEvent::EditStatus => this.open_status_picker(window, cx),
                ProfileEvent::EditProfile => this.open_profile_editor(window, cx),
            },
        ),
        cx.subscribe_in(
            &views.switcher,
            window,
            |this: &mut App, _, event: &SwitcherEvent, window, cx| match event {
                SwitcherEvent::Open(channel) => {
                    window.close_dialog(cx);
                    this.select(channel.clone(), cx);
                    this.focus.focus(window, cx);
                }
                SwitcherEvent::OpenRemoteChannel(conv) => {
                    window.close_dialog(cx);
                    if this.fixtures.conversation(&conv.id).is_none() {
                        if let Some((store, ws)) = this.bridge.store().zip(this.bridge.team_id()) {
                            if let Ok(mut guard) = store.lock() {
                                let _ = guard.upsert_conversation(ws, conv);
                            }
                            this.reload_live(window, cx);
                        }
                    }
                    this.select(conv.id.clone(), cx);
                    this.focus.focus(window, cx);
                }
                SwitcherEvent::OpenUser(user) => {
                    window.close_dialog(cx);
                    this.message_user(user, window, cx);
                    this.focus.focus(window, cx);
                }
                SwitcherEvent::SearchRemote(query) => {
                    this.bridge.send(Command::QuickSwitchSearch {
                        query: query.clone(),
                    });
                }
            },
        ),
    ];
    if let Some((_, bar)) = &views.huddle {
        subscriptions.push(cx.subscribe_in(
            bar,
            window,
            |this: &mut App, _, event: &HuddleEvent, window, cx| match event {
                HuddleEvent::Leave => {
                    this.leave_huddle(cx);
                }
                HuddleEvent::OpenThread => {
                    this.toggle_huddle_thread(cx);
                }
                HuddleEvent::ToggleMute => {
                    this.toggle_huddle_mute(cx);
                }
                HuddleEvent::ToggleVideo => {
                    this.toggle_huddle_camera(cx);
                }
                HuddleEvent::ToggleShare => {
                    this.toggle_huddle_screen_share(window, cx);
                }
                HuddleEvent::PopOut => {
                    this.pop_out_huddle(window, cx);
                }
                HuddleEvent::CopyLink => {
                    let ch = this
                        .huddle_session
                        .as_ref()
                        .map(|s| s.channel.clone())
                        .or_else(|| this.huddle.as_ref().map(|(c, _)| c.clone()))
                        .unwrap_or_else(|| this.channel.clone());
                    this.copy_huddle_link(ch, window, cx);
                }
                HuddleEvent::OpenCanvas(fid) => {
                    this.open_huddle_canvas(fid.clone(), cx);
                }
            },
        ));
    }
    subscriptions
}
