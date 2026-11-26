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
//!   MessagingCommand::MarkActivity) and Retry (MessagingCommand::FetchActivity)
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
//!   emits FilesPageEvent::Load(FilesFilter) (forwarded to the core as CollabCommand::LoadFiles);
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
use crate::search::{SearchEvent, SearchView, SearchWorkspaceSource, fixture_archive};
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

use self::chrome::{corner_cap, joinable, shadow};
use self::demo::demo;
use self::helpers::{
    broadcast_kind, convert_emoticons, first_conversation, forget_failure, is_suspicious_url,
    mentions_me, parse_notification_target, parse_permalink, sync_launch_at_login,
    workspace_badge_from_counts,
};

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
    /// Live mode: parsed `CanvasMeta` keyed by file id (`Event::CanvasMetaLoaded`).
    pub(super) canvas_metas: HashMap<String, slackless_core::api::CanvasMeta>,
    /// Live mode: threaded comments keyed by canvas file id (`Event::CanvasCommentsLoaded`).
    pub(super) canvas_comments: HashMap<String, Vec<slackless_core::api::CanvasCommentThread>>,
    /// Live mode: inline status or error banner per canvas id (`(is_error, message)`).
    pub(super) canvas_status: HashMap<String, (bool, String)>,
    /// Live mode: "Canvas updated" notice per canvas id (`F-COLLAB-185`).
    pub(super) canvas_updated_notes: HashMap<String, String>,
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
    /// Live archive backfill progress snapshot for this workspace (`Event::BackfillProgress`).
    pub(super) backfill_progress: Option<slackless_core::WorkspaceBackfillProgress>,
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

/// Which top-level page currently owns the main column (mutually exclusive).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum MainPage {
    #[default]
    Channel,
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
    /// Channels already asked for a single-page history fetch from `refresh_unreads`, so an
    /// empty unread channel is fetched at most once per session.
    unreads_fetched: HashSet<ChannelId>,
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
    /// A compose send waiting for its DM to open ([`ChannelCommand::OpenDm`]).
    pending_dm: Option<MessageContent>,
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
    /// Live mode: the newest async `reload_live` snapshot started; coalesces event bursts.
    reload_generation: u64,
    /// Debounced snapshot reload handle; replacing cancels pending reads during hydration bursts.
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
