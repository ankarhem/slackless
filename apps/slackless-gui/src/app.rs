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
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, Size, TitleBar, WindowExt as _, h_flex,
    h_resizable, resizable_panel, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    self, Anchor, AnyElement, AppContext as _, BoxShadow, ClipboardItem, Context, DismissEvent,
    Div, Edges, Entity, FocusHandle, Focusable as _, FontWeight, Hsla, InputPreference,
    InteractiveElement as _, IntoElement, KeyBinding, MouseButton, MouseDownEvent,
    ParentElement as _, Pixels, Point, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Subscription, SystemNotification, Window,
    actions, anchored, deferred, div, hsla, point, px, relative,
};
use slackless_core::api::{ChannelBookmark, ConversationCounts, FilesFilter, MessageContent};
use slackless_core::platform::{self, BadgeSetter, Notifier, SoundPlayer};
use slackless_core::types::rich_text::BroadcastRange;
use slackless_core::types::{
    Block, ChannelId, Conversation, ConversationKind, File, Message, RichTextContainer,
    RichTextElement, Team, Ts, UserId, Usergroup,
};
use slackless_core::{Command, Event, NotifyLevel, SharedStore};

use crate::actions::{
    FocusDeleteMessage, FocusEditMessage, FocusForwardMessage, FocusMarkMessageUnread, FocusReact,
    FocusRemindMe, FocusReplyInThread, FocusSaveMessage, FocusTogglePin, MessageAction,
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
use crate::drafts::{DraftsEvent, DraftsView, Entry, SendLater, now_secs};
use crate::emoji::{EmojiChoice, EmojiPicker, EmojiPickerEvent};
use crate::files_page::{FilesPage, FilesPageEvent, FilesSection};
use crate::fixtures::{self, Fixtures};
use crate::hover_card;
use crate::huddle::{self, HuddleBar, HuddleEvent};
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
use crate::sidebar::{NavItem, Sidebar, SidebarEvent};
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
        OpenLater,
        OpenFiles,
        OpenPeople,
        OpenOwnProfile,
        NextWorkspace,
        PreviousWorkspace,
        PreviousConversation,
        NextConversation,
        PreviousUnread,
        NextUnread,
        MarkConversationRead,
        MarkAllRead,
        SearchConversation,
        ZoomIn,
        ZoomOut,
        ActualSize,
    ]
);

/// Switch to the workspace at this index (Cmd+1…9, File > Workspace).
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = slackless, no_json)]
pub struct SwitchWorkspace(pub usize);

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

/// A black box shadow (Slack's `box-shadow: <offset> <offset> <blur> rgba(0,0,0,<alpha>)`).
fn shadow(offset: f32, alpha: f32, blur: f32) -> BoxShadow {
    BoxShadow {
        color: hsla(0., 0., 0., alpha),
        offset: point(px(offset), px(offset)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }
}

/// One right corner (`top` or bottom) of the workspace panel over a side panel that paints a
/// square background: GPUI clips rectangularly, so a band whose inner edge is the corner's arc
/// repaints the window chrome outside it, and dark mode's `outline` is redrawn along it.
fn corner_cap(top: bool, chrome: Hsla, outline: Option<Hsla>) -> Vec<Div> {
    let band = PANEL_RADIUS;
    // Absolute insets start inside the panel's outline.
    let edge = if outline.is_some() { 1. } else { 0. };
    let cap = div()
        .absolute()
        .right(px(-band - edge))
        .size(px(2. * (PANEL_RADIUS + band)))
        .border_r(px(band))
        .border_color(chrome);
    let mut caps = vec![if top {
        cap.top(px(-band - edge))
            .border_t(px(band))
            .rounded_tr(px(PANEL_RADIUS + band))
    } else {
        cap.bottom(px(-band - edge))
            .border_b(px(band))
            .rounded_br(px(PANEL_RADIUS + band))
    }];
    if let Some(outline) = outline {
        let arc = div()
            .absolute()
            .right(px(-edge))
            .size(px(2. * PANEL_RADIUS))
            .border_r_1()
            .border_color(outline);
        caps.push(if top {
            arc.top(px(-edge)).border_t_1().rounded_tr(px(PANEL_RADIUS))
        } else {
            arc.bottom(px(-edge))
                .border_b_1()
                .rounded_br(px(PANEL_RADIUS))
        });
    }
    caps
}
/// The thread / profile / pins panel, measured on Slack desktop.
const FLEXPANE_WIDTH: Pixels = px(540.);

/// The OS integration the root uses: system notifications and the dock badge.
trait Os: Notifier + BadgeSetter + SoundPlayer {}
impl<T: Notifier + BadgeSetter + SoundPlayer> Os for T {}

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

/// A workspace that is not shown: its fixtures, open conversation, views and their
/// subscriptions.
struct Parked {
    fixtures: Arc<Fixtures>,
    channel: ChannelId,
    views: WorkspaceViews,
    subscriptions: Vec<Subscription>,
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

pub struct App {
    /// The shown workspace's fixtures.
    fixtures: Arc<Fixtures>,
    bridge: CoreBridge,
    /// Index of the shown workspace in `fixtures.workspaces`.
    workspace: usize,
    /// Per workspace index: the parked state of a workspace visited before (`None` for the
    /// shown one and never-visited ones).
    parked: Vec<Option<Parked>>,
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
    /// The Preferences window body, mounted in a dialog on demand; owns the settings.
    preferences: Entity<Preferences>,
    /// The theme last installed (mode, color theme, focus mode's grayscale), so unrelated
    /// preference edits do not repaint the theme.
    applied_theme: (SlackTheme, Palette, bool),
    /// The Color Mode last stored (`None` offline), so only a changed mode is written.
    color_mode: Option<AppearanceMode>,
    /// The full Preferences snapshot last persisted (`None` offline), avoiding redundant writes.
    saved_settings: Option<crate::preferences::Settings>,
    /// The profile pane; shown in the right panel while `profile_open` (exclusive with the
    /// thread panel).
    profile: Entity<ProfilePanel>,
    profile_open: bool,
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
    /// Live mode: per channel, its pinned messages (`pins.list`) or why they could not be
    /// loaded; fetched each time a pins surface opens, never per channel.
    pins: HashMap<ChannelId, Loaded<Message>>,
    /// Live mode: per channel, its bookmarks (`bookmarks.list`), fetched each time a
    /// bookmarks surface opens.
    bookmarks: HashMap<ChannelId, Loaded<ChannelBookmark>>,
    /// Live mode: per channel, its shared files (`files.list`), fetched each time Files &
    /// links opens.
    files: HashMap<ChannelId, Loaded<File>>,
    /// Fixture mode: link bookmarks added through the add-link dialog, per channel.
    fixture_bookmarks: HashMap<ChannelId, Vec<bookmarks::Bookmark>>,
    /// Dock badge + native notifications (`slackless-platform`).
    os: Rc<dyn Os>,
    /// The badge count last handed to the dock, so it is only set on change.
    badge: Option<u32>,
    /// The window title last set, so it is only set on change.
    window_title: String,
    /// The pending, debounced save of the window frame (replacing it cancels the last one).
    window_state_save: gpui::Task<()>,
    /// Live mode: the newest history read started; an overtaken read is dropped.
    history_generation: u64,
    /// Live mode: channels whose stored history has no older page left; the transcript
    /// stops asking for them.
    history_complete: HashSet<ChannelId>,
    /// Live mode: the newest thread read started; an overtaken read is dropped.
    thread_generation: u64,
    /// Live mode: Slack's latest read and unread state (`client.counts`).
    counts: HashMap<ChannelId, ConversationCounts>,
    /// Live mode: last known presence per DM peer (`users.getPresence`, `presence_change`);
    /// injected into every live fixtures snapshot so sidebar rows and headers render dots.
    presence: fixtures::PresenceMap,
    /// Live mode: DM peers whose notifications are paused right now (`dnd.info`).
    peer_dnd_paused: HashMap<UserId, bool>,
    /// Live mode: latest RTM websocket connection state (drives the offline/reconnecting banner).
    connection: Option<slackless_core::rtm::ConnectionState>,
    /// Live mode: active `@handle` user groups from `usergroups.list`, injected into snapshots.
    usergroups: Vec<Usergroup>,
    /// Member counts from open-time `conversations.info`, newer than the snapshot's.
    member_counts: HashMap<ChannelId, u32>,
    /// Apps among a channel's members, counted apart from people as Slack's header does.
    app_counts: HashMap<ChannelId, u32>,
    /// Conversations navigated away from, newest last (Slack's back arrow, Cmd+[).
    back: Vec<ChannelId>,
    /// Conversations stepped back from, newest last (the forward arrow, Cmd+]).
    forward: Vec<ChannelId>,
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
    /// The user opened a conversation themselves; until then live reloads open the one most
    /// recently read in Slack.
    chosen: bool,
    /// Root focus target, so Cmd+K / Cmd+, reach the root with nothing else focused.
    focus: FocusHandle,
    /// Subscriptions to the shown workspace's views (parked with them on a switch).
    workspace_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl App {
    pub fn new(
        fixtures: Arc<Fixtures>,
        mut bridge: CoreBridge,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Entity<App> {
        let app = cx.new(|cx| {
            // Like Slack, reopen the conversation that was open last; a fresh store falls back
            // to the sidebar's first conversation until the user picks one.
            let restored = bridge
                .last_conversation()
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
            if let Some(history) = bridge.search_history() {
                search_menu.update(cx, |menu, cx| menu.set_history(history, cx));
            }
            if let Some(on) = bridge.search_history_shown() {
                search_menu.update(cx, |menu, cx| menu.set_history_shown(on, cx));
            }
            if let Some(on) = bridge.search_filters_shown() {
                search_menu.update(cx, |menu, cx| menu.set_filters_shown(on, cx));
            }
            let restored_recent = bridge
                .recent_places()
                .unwrap_or_default()
                .into_iter()
                .filter(|channel| fixtures.conversation(channel).is_some())
                .collect();
            let preferences = Preferences::new(window, cx);
            // Restore persisted Preferences settings first, then overlay the zoom/font/color_mode
            // values `main` installed before the first frame.
            let mut restored_settings = bridge.settings();
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
            let color_mode = bridge.color_mode();
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
                    (mode.resolve(window.appearance()), palette, false)
                }
                None => (SlackTheme::Dark, Palette::Default, false),
            };
            let saved_settings = if bridge.store().is_some() {
                Some(restored_settings.unwrap_or_else(|| preferences.read(cx).settings().clone()))
            } else {
                None
            };
            let status_picker = StatusPicker::new(window, cx);
            let user_menu = UserMenu::new(fixtures.clone(), cx);
            let emoji_picker = EmojiPicker::new(fixtures.clone(), window, cx);
            let persisted_freq = bridge.frequent_emoji();
            let persisted_tone = bridge.emoji_tone();
            if persisted_freq.is_some() || persisted_tone.is_some() {
                emoji_picker.update(cx, |picker, cx| {
                    picker.restore(persisted_freq, persisted_tone, cx);
                });
            }
            let drafts = DraftsView::new(cx);
            if let Some(saved_drafts) = bridge.drafts() {
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
            let compose = NewMessageView::new(fixtures.clone(), window, cx);
            let details = ChannelDetails::new(fixtures.clone(), window, cx);
            later.update(cx, |later, cx| later.seed(0, &fixtures, cx));
            crate::actions::bind_menu_keys(cx);
            cx.bind_keys([
                KeyBinding::new("cmd-k", QuickSwitch, None),
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
            ]);
            let workspace_keys = [
                "cmd-1", "cmd-2", "cmd-3", "cmd-4", "cmd-5", "cmd-6", "cmd-7", "cmd-8", "cmd-9",
            ];
            cx.bind_keys(
                (workspace_keys.into_iter().enumerate())
                    .map(|(ix, keys)| KeyBinding::new(keys, SwitchWorkspace(ix), None)),
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
            let subscriptions = vec![
                cx.observe_window_bounds(window, |this, window, cx| {
                    this.save_window_state(window, cx);
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
                    this.on_core_event(event, window, cx);
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
                        ComposerEvent::OpenEmojiPicker(position) => {
                            this.open_picker(PickerTarget::Composer, *position, window, cx);
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
                            this.schedule(text.clone(), *when, window, cx);
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
                        EmojiPickerEvent::ToneChanged(tone) => {
                            if let Some(write) = this.bridge.remember_emoji_tone(*tone) {
                                cx.background_executor().spawn(write).detach();
                            }
                        }
                    },
                ),
                cx.subscribe_in(
                    &preferences,
                    window,
                    |this: &mut App, _, _: &PreferencesEvent, window, cx| {
                        this.apply_preferences(window, cx);
                    },
                ),
                cx.observe_window_appearance(window, |this: &mut App, window, cx| {
                    this.apply_preferences(window, cx);
                }),
                cx.subscribe_in(
                    &status_picker,
                    window,
                    |this: &mut App, _, event: &StatusEvent, window, cx| {
                        if let StatusEvent::Save(status) = event {
                            this.self_state.set_status(status.clone());
                            this.push_self_state(cx);
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
                        LaterEvent::Unsaved { workspace, ts } => {
                            if *workspace == this.workspace {
                                this.messages
                                    .update(cx, |list, cx| list.set_saved(ts, false, cx));
                                this.thread
                                    .update(cx, |thread, cx| thread.set_saved(ts, false, cx));
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
                            this.threads_open = false;
                            this.select(channel.clone(), cx);
                            this.open_thread(channel.clone(), thread_ts.clone(), cx);
                        }
                        ThreadsEvent::Reply {
                            channel,
                            thread_ts,
                            content,
                        } => {
                            this.bridge.send(Command::SendMessage {
                                channel: channel.clone(),
                                content: content.clone(),
                                thread_ts: Some(thread_ts.clone()),
                                broadcast: false,
                            });
                        }
                        ThreadsEvent::PickEmoji(position) => {
                            this.open_picker(PickerTarget::ThreadsComposer, *position, window, cx);
                        }
                        ThreadsEvent::SlashCommand(invocation) => {
                            let thread = this.threads.read(cx).active_thread();
                            this.run_slash(invocation, thread, window, cx)
                        }
                        ThreadsEvent::Close => {
                            this.threads_open = false;
                            cx.notify();
                        }
                    },
                ),
                cx.subscribe_in(
                    &unreads,
                    window,
                    |this: &mut App, _, event: &UnreadsEvent, window, cx| match event {
                        UnreadsEvent::OpenConversation(channel) => {
                            this.unreads_open = false;
                            this.select(channel.clone(), cx);
                        }
                        UnreadsEvent::OpenLink(target) => {
                            this.open_link(target.clone(), window, cx);
                        }
                        UnreadsEvent::MarkRead { channel, ts } => {
                            this.mark_read(channel, ts.clone(), cx);
                        }
                        UnreadsEvent::MarkAllRead => this.request_mark_all_read(window, cx),
                    },
                ),
                cx.subscribe(
                    &activity,
                    |this: &mut App, _, event: &ActivityEvent, cx| match event {
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
                    },
                ),
                cx.observe(&activity, |this: &mut App, _, cx| {
                    // The rail tile and the dock badge follow the feed's unread rows.
                    let unread = this.activity.read(cx).unread_count();
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.set_activity_unread(unread, cx));
                    cx.notify();
                }),
                cx.subscribe(
                    &files_page,
                    |this: &mut App, _, event: &FilesPageEvent, _| {
                        let FilesPageEvent::Load(filter) = event;
                        this.bridge.send(Command::LoadFiles(filter.clone()));
                    },
                ),
                cx.subscribe_in(
                    &huddles,
                    window,
                    |this: &mut App, _, event: &HuddlesEvent, _, cx| match event {
                        HuddlesEvent::Open(channel) => {
                            this.huddles_open = false;
                            this.select(channel.clone(), cx);
                            this.huddle_open = this.huddle_here();
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
                    |this: &mut App, _, event: &DirectoryEvent, _, cx| match event {
                        DirectoryEvent::OpenProfile(user) => this.open_profile(user.clone(), cx),
                        DirectoryEvent::Join(channel) => {
                            if let Some(joined) = this.fixtures.conversation(channel).cloned() {
                                this.sidebar
                                    .update(cx, |sidebar, cx| sidebar.add_channel(joined, cx));
                            }
                        }
                        DirectoryEvent::Open(channel) => this.select(channel.clone(), cx),
                        DirectoryEvent::Close => {
                            this.directory_open = false;
                            cx.notify();
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
            sidebar.update(cx, |sidebar, cx| sidebar.set_activity_unread(unread, cx));

            App {
                fixtures,
                bridge,
                workspace: 0,
                parked,
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
                preferences,
                applied_theme,
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
                compose,
                compose_open: false,
                details,
                pending_dm: None,
                pins_open: false,
                pins: HashMap::new(),
                bookmarks: HashMap::new(),
                files: HashMap::new(),
                fixture_bookmarks: HashMap::new(),
                os: Rc::new(platform::current()),
                badge: None,
                window_title: String::new(),
                window_state_save: gpui::Task::ready(()),
                history_generation: 0,
                history_complete: HashSet::new(),
                thread_generation: 0,
                counts: HashMap::new(),
                presence: fixtures::PresenceMap::default(),
                peer_dnd_paused: HashMap::new(),
                connection: None,
                usergroups: Vec::new(),
                member_counts: HashMap::new(),
                app_counts: HashMap::new(),
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
                chosen,
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
            let Some((channel, ts)) = parse_notification_tag(&response.tag) else {
                return;
            };
            cx.activate(true);
            let _ = notif_window.update(cx, |_, window, cx| {
                window.activate_window();
                let _ = notif_app.update(cx, |app, cx| {
                    app.select(channel, cx);
                    app.messages.update(cx, |list, cx| list.reveal(&ts, cx));
                });
            });
        });
        app.update(cx, |app, cx| {
            app.push_self_state(cx);
            app.sync_saved(cx);
            app.sync_later_count(cx);
            let channel = app.channel.clone();
            app.load_history(&channel, cx);
            app.load_draft(cx);
            app.sync_composer_placeholder(cx);
            app.update_menus(cx);
        });
        app
    }

    /// Rebuild the menu bar when its moving parts change: the workspaces (File > Workspace,
    /// Sign Out) and the History menu's places, labelled as Slack's.
    fn update_menus(&mut self, cx: &mut Context<Self>) {
        /// Slack's History menu lists this many places.
        const PLACES: usize = 15;
        let bar = MenuBar {
            workspaces: (self.fixtures.workspaces.iter())
                .map(|team| team.name.clone().into())
                .collect(),
            recent: (self.recent_entries().into_iter().take(PLACES))
                .map(|(id, label, icon)| {
                    let label = match icon {
                        IconName::Hash => format!("#{label}").into(),
                        IconName::Lock => format!("#{label} (Private)").into(),
                        _ => label,
                    };
                    (label, id)
                })
                .collect(),
        };
        if self.menu_bar.as_ref() != Some(&bar) {
            menu_bar::set(&bar, cx);
            self.menu_bar = Some(bar);
        }
    }

    /// Put the shown workspace's Later items on its message views' saved markers.
    fn sync_saved(&mut self, cx: &mut Context<Self>) {
        let saved = self.later.read(cx).saved_in(self.workspace);
        for (_, ts) in saved {
            self.messages
                .update(cx, |list, cx| list.set_saved(&ts, true, cx));
            self.thread
                .update(cx, |thread, cx| thread.set_saved(&ts, true, cx));
        }
    }

    fn sync_later_count(&mut self, cx: &mut Context<Self>) {
        let count = self.later.read(cx).in_progress_count();
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_later_count(count, cx));
    }

    /// A message view saved, unsaved or set a reminder on a message of the shown workspace.
    fn save_for_later(&mut self, request: &SaveRequest, cx: &mut Context<Self>) {
        let label = self.label(&request.channel);
        let workspace = self.workspace;
        self.later.update(cx, |later, cx| {
            later.save(request.clone(), workspace, label, cx)
        });
    }

    fn open_later(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.later_open = true;
        self.drafts_open = false;
        self.searching = false;
        cx.notify();
    }

    /// Post every Later reminder that came due: system notification plus in-app toast.
    fn fire_reminders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let due = self
            .later
            .update(cx, |later, cx| later.take_due(now_secs(), cx));
        for item in due {
            let title = format!("Reminder: {} in {}", item.author, item.label);
            let tag = format!("later:{}:{}", item.channel, item.ts);
            self.post_notification(&tag, &title, &item.text, window, cx);
        }
    }

    /// A system notification (a macOS banner) with the notification sound, as Slack's desktop
    /// notifications come.
    fn system_notification(&mut self, tag: &str, title: &str, body: &str, cx: &mut Context<Self>) {
        cx.show_system_notification(SystemNotification {
            tag: tag.to_owned().into(),
            title: title.to_owned().into(),
            body: body.to_owned().into(),
            actions: Vec::new(),
        });
        self.os.notify(title, body);
        self.play_notification_sound(cx);
    }

    /// A notification mirrored as an in-app toast (Later reminders).
    fn post_notification(
        &mut self,
        tag: &str,
        title: &str,
        body: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.system_notification(tag, title, body, cx);
        notify(title, body.to_owned(), window, cx);
    }

    /// Remember the window's frame once it has settled after a move or resize, so the next
    /// launch reopens it there, like Slack (live mode; a no-op offline or in full screen).
    fn save_window_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        const SETTLE: Duration = Duration::from_millis(500);
        self.window_state_save = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let _ = this.update_in(cx, |app, window, cx| {
                let write = window
                    .native_window_state()
                    .and_then(|state| app.bridge.remember_window_state(state));
                if let Some(write) = write {
                    cx.background_executor().spawn(write).detach();
                }
            });
        });
    }

    /// Slack's window title (Window menu, Mission Control):
    /// "<conversation> (Channel|DM) - <workspace> - N new items - <app>".
    fn sync_window_title(&mut self, conversation: Option<&Conversation>, window: &mut Window) {
        let mut parts = Vec::with_capacity(4);
        if let Some(conversation) = conversation {
            let kind = match conversation.kind() {
                ConversationKind::Im | ConversationKind::Mpim => "DM",
                ConversationKind::PublicChannel | ConversationKind::PrivateChannel => "Channel",
            };
            let label = self.fixtures.conversation_label(conversation);
            let name = label.strip_prefix("# ").unwrap_or(&label);
            parts.push(format!("{name} ({kind})"));
        }
        if let Some(team) = self
            .fixtures
            .workspaces
            .get(self.workspace)
            .filter(|team| !team.name.is_empty())
        {
            parts.push(team.name.clone());
        }
        match self.badge {
            Some(1) => parts.push("1 new item".to_owned()),
            Some(n) if n > 1 => parts.push(format!("{n} new items")),
            _ => {}
        }
        parts.push("slackless".to_owned());
        let title = parts.join(" - ");
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }
    }

    /// Hand the unread count to the dock badge when it changed (empty badge at zero).
    fn sync_badge(&mut self, cx: &mut Context<Self>) {
        let count = self.sidebar.read(cx).badge_count();
        if self.badge != Some(count) {
            self.badge = Some(count);
            let label = (count > 0).then(|| count.to_string());
            self.os.set_badge(label.as_deref());
        }
    }

    fn open_thread(&mut self, channel: ChannelId, thread_ts: Ts, cx: &mut Context<Self>) {
        self.thread.update(cx, |panel, cx| {
            panel.open(channel.clone(), thread_ts.clone(), cx)
        });
        self.sync_thread_bar(cx);
        // Live mode: show the stored thread now, then fetch its full history from Slack
        // (fixture mode: both are no-ops and the panel keeps its fixture content).
        self.load_thread(&channel, &thread_ts, cx);
        self.messages.update(cx, |list, cx| {
            list.set_open_thread(Some(thread_ts.clone()), cx)
        });
        self.bridge.send(Command::OpenThread { channel, thread_ts });
        self.thread_open = true;
        self.profile_open = false;
        self.pins_open = false;
        if self.tab == ConversationTab::Pins {
            self.tab = ConversationTab::Messages;
        }
        self.show_typing(cx);
        cx.notify();
    }

    /// Hide the thread panel and drop the open-thread tint from the transcript.
    fn close_thread(&mut self, cx: &mut Context<Self>) {
        self.thread_open = false;
        self.messages
            .update(cx, |list, cx| list.set_open_thread(None, cx));
    }

    fn toggle_pins(&mut self, cx: &mut Context<Self>) {
        self.pins_open = !self.pins_open;
        if self.pins_open {
            self.close_thread(cx);
            self.profile_open = false;
            self.load_tab_data(&ConversationTab::Pins);
        } else if self.tab == ConversationTab::Pins {
            self.tab = ConversationTab::Messages;
        }
        cx.notify();
    }

    /// A link in message text was activated. Slack resolves its own targets in-app:
    /// profiles, channels and threads open their panes, and Slack permalinks open the
    /// conversation (or thread) instead of the browser; everything else is a web URL.
    fn open_link(&mut self, target: LinkTarget, _window: &mut Window, cx: &mut Context<Self>) {
        match target {
            LinkTarget::Profile(user) => self.open_profile(user, cx),
            LinkTarget::Channel(channel) if self.fixtures.conversation(&channel).is_some() => {
                self.select(channel, cx);
            }
            LinkTarget::Thread { channel, thread_ts }
                if self.fixtures.conversation(&channel).is_some() =>
            {
                self.open_thread(channel, thread_ts, cx);
            }
            LinkTarget::Url(url) => match parse_permalink(&url) {
                Some(permalink) if self.fixtures.conversation(&permalink.channel).is_some() => {
                    self.select(permalink.channel.clone(), cx);
                    match permalink.thread_ts {
                        // A reply's link opens its thread scrolled to the reply, as in Slack.
                        Some(thread_ts) => {
                            self.open_thread(permalink.channel.clone(), thread_ts, cx);
                            self.thread
                                .update(cx, |panel, cx| panel.reveal(permalink.ts, cx));
                        }
                        None => {
                            self.messages
                                .update(cx, |list, cx| list.reveal(&permalink.ts, cx));
                        }
                    }
                }
                _ => cx.open_url(url.as_ref()),
            },
            // An in-app target this workspace does not know: nothing to open.
            LinkTarget::Channel(_) | LinkTarget::Thread { .. } => {}
        }
    }

    /// A tab row entry was clicked: switch the column, open the channel canvas in Slack's
    /// web client, or create the missing canvas.
    fn activate_tab(&mut self, action: TabAction, cx: &mut Context<Self>) {
        match action {
            TabAction::Switch(target) => self.select_tab(target, cx),
            TabAction::OpenCanvas(file) => cx.open_url(&self.canvas_link(&file)),
            TabAction::AddCanvas => {
                self.bridge
                    .send(Command::CreateChannelCanvas(self.channel.clone()));
            }
        }
    }

    /// Show `target`'s body. The Pins tab toggles the right-hand panel over the messages
    /// column; live mode fetches the tab's data each time it opens.
    fn select_tab(&mut self, target: ConversationTab, cx: &mut Context<Self>) {
        if target == ConversationTab::Pins && self.tab == ConversationTab::Pins {
            self.pins_open = false;
            self.tab = ConversationTab::Messages;
            cx.notify();
            return;
        }
        if target == ConversationTab::Pins {
            self.pins_open = true;
            self.close_thread(cx);
            self.profile_open = false;
        }
        self.load_tab_data(&target);
        self.tab = target;
        cx.notify();
    }

    /// Live mode: fetch the data of a tab that just opened - one request per open, never
    /// per channel. A failed earlier load is dropped so the tab shows loading again.
    fn load_tab_data(&mut self, tab: &ConversationTab) {
        if self.fixtures.sample {
            return;
        }
        let channel = self.channel.clone();
        let command = match tab {
            ConversationTab::Pins => {
                forget_failure(&mut self.pins, &channel);
                Command::LoadPins(channel)
            }
            ConversationTab::Files => {
                forget_failure(&mut self.files, &channel);
                Command::LoadFiles(FilesFilter::channel(channel))
            }
            ConversationTab::Folder { .. } => {
                forget_failure(&mut self.bookmarks, &channel);
                Command::LoadBookmarks(channel)
            }
            ConversationTab::Messages | ConversationTab::Canvas | ConversationTab::List => {
                return;
            }
        };
        self.bridge.send(command);
    }

    /// The live channel's top-level bookmarks for the bar: links open their URL, folders
    /// open their tab; entries without a target are left out. Empty until the channel's
    /// `bookmarks.list` has been fetched (lazy).
    fn live_bookmarks(&self) -> Vec<bookmarks::Bookmark> {
        let Some(Ok(list)) = self.bookmarks.get(&self.channel) else {
            return Vec::new();
        };
        list.iter()
            .filter(|bookmark| bookmark.parent_id.as_deref().unwrap_or_default().is_empty())
            .filter_map(|bookmark| {
                let title = bookmark.title.clone();
                if bookmark.kind.as_deref() == Some("folder") {
                    return Some(bookmarks::Bookmark::folder(title, bookmark.id.clone()));
                }
                let link = bookmark.link.as_deref().filter(|link| !link.is_empty())?;
                Some(bookmarks::Bookmark::link(title, link.to_owned()))
            })
            .collect()
    }

    /// A folder bookmark in the bar: show its bookmarks under the conversation's tab for the
    /// folder, or under the folder's own title when the row has no such tab.
    fn open_folder_tab(&mut self, folder_id: &str, cx: &mut Context<Self>) {
        let row_label = self
            .fixtures
            .conversation(&self.channel)
            .and_then(|conversation| {
                tabs::derive(conversation, self.fixtures.sample)
                    .into_iter()
                    .find_map(|spec| match spec.action {
                        TabAction::Switch(ConversationTab::Folder { label, bookmark_id })
                            if bookmark_id == folder_id =>
                        {
                            Some(label)
                        }
                        _ => None,
                    })
            });
        let title = || {
            let list = self.bookmarks.get(&self.channel)?.as_ref().ok()?;
            let folder = list.iter().find(|bookmark| bookmark.id == folder_id)?;
            Some(SharedString::from(folder.title.clone()))
        };
        let label = row_label
            .or_else(title)
            .unwrap_or_else(|| "Bookmarks".into());
        let bookmark_id = folder_id.to_owned();
        self.select_tab(ConversationTab::Folder { label, bookmark_id }, cx);
    }

    /// The canvas document's permalink in Slack's web client.
    fn canvas_link(&self, canvas_id: &str) -> String {
        match self.fixtures.workspaces.first() {
            Some(team) if !team.domain.is_empty() => format!(
                "https://{}.slack.com/docs/{}/{}",
                team.domain, team.id, canvas_id
            ),
            Some(team) => {
                format!("https://app.slack.com/client/{}/{}", team.id, self.channel)
            }
            None => format!("https://app.slack.com/client/{}", self.channel),
        }
    }

    /// The "+" menu's Canvas item: open the conversation's canvas, creating it first when
    /// it has none (the created canvas arrives as `Event::ChannelCanvasCreated` and opens
    /// then).
    fn open_or_add_canvas(&mut self, cx: &mut Context<Self>) {
        let canvas = self
            .fixtures
            .conversation(&self.channel)
            .and_then(|conversation| conversation.properties.as_ref())
            .and_then(|properties| properties.canvas.as_ref())
            .and_then(|canvas| canvas.file_id.as_ref())
            .map(|file| file.to_string());
        match canvas {
            Some(file) => cx.open_url(&self.canvas_link(&file)),
            None => {
                self.bridge
                    .send(Command::CreateChannelCanvas(self.channel.clone()));
            }
        }
    }

    /// The add-link dialog's Add: live mode sends `bookmarks.add`, fixture mode keeps the
    /// bookmark in memory with the fixture bar.
    fn add_bookmark(&mut self, title: String, url: String, cx: &mut Context<Self>) {
        if self.fixtures.sample {
            self.fixture_bookmarks
                .entry(self.channel.clone())
                .or_default()
                .push(bookmarks::Bookmark::link(title, url));
            cx.notify();
        } else {
            self.bridge.send(Command::AddBookmark {
                channel: self.channel.clone(),
                title,
                url,
            });
        }
    }

    /// The "+" menu's Link item and the bar's "Add a bookmark": the add-link dialog for the
    /// open conversation.
    fn open_link_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        bookmarks::open_add_dialog(window, cx, move |title, url, cx| {
            let _ = this.update(cx, |app, cx| app.add_bookmark(title, url, cx));
        });
    }

    /// Apply what the app honors from Preferences: the theme (Light / Dark / Sync with OS,
    /// tinted by the color theme), Enter-to-send, and the schedule shown in the user menu.
    fn apply_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.preferences.read(cx).settings().clone();
        // Slack plays a sound when it is picked in Preferences.
        if self
            .sound_choice
            .replace(settings.sound)
            .is_some_and(|before| before != settings.sound)
        {
            self.play_sound(settings.sound, cx);
        }
        let theme = settings.mode.resolve(window.appearance());
        if self
            .color_mode
            .is_some_and(|stored| stored != settings.mode)
        {
            self.color_mode = Some(settings.mode);
            if let Some(write) = self.bridge.remember_color_mode(settings.mode) {
                cx.background_executor().spawn(write).detach();
            }
        }
        let focus = self.self_state.focused();
        if self.applied_theme != (theme, settings.palette, focus) {
            self.applied_theme = (theme, settings.palette, focus);
            theme::activate_with(theme, settings.palette, focus, cx);
        }
        if crate::zoom::set_level(settings.zoom, cx) {
            if let Some(write) = self.bridge.remember_zoom(settings.zoom) {
                cx.background_executor().spawn(write).detach();
            }
        }
        if theme::font(cx) != settings.font {
            theme::set_font(settings.font, cx);
            if let Some(write) = self.bridge.remember_font(settings.font) {
                cx.background_executor().spawn(write).detach();
            }
        }
        let enter_sends = settings.enter_sends;
        self.composer
            .update(cx, |composer, cx| composer.set_enter_sends(enter_sends, cx));
        self.messages
            .update(cx, |list, cx| list.set_enter_sends(enter_sends, cx));
        self.thread
            .update(cx, |thread, cx| thread.set_enter_sends(enter_sends, cx));
        self.threads
            .update(cx, |view, cx| view.set_enter_sends(enter_sends, cx));
        self.compose
            .update(cx, |view, cx| view.set_enter_sends(enter_sends, cx));
        if self
            .saved_settings
            .as_ref()
            .is_some_and(|saved| *saved != settings)
        {
            self.saved_settings = Some(settings.clone());
            if let Some(write) = self.bridge.remember_settings(&settings) {
                cx.background_executor().spawn(write).detach();
            }
        }
        self.push_self_state(cx);
    }

    /// Slack's focus mode (the rail's moon): on with Slack's default "Don't turn off", or off.
    fn toggle_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.self_state.focused() {
            self.self_state.end_focus();
        } else {
            self.self_state.start_focus(FOCUS_UNTIL_OFF);
        }
        self.apply_preferences(window, cx);
    }

    /// End a timed focus mode once its Duration ran out (checked on the reminder poll).
    fn expire_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.self_state.focus_until.is_some() && !self.self_state.focused() {
            self.self_state.end_focus();
            self.apply_preferences(window, cx);
        }
    }

    /// The composer prompts with the open conversation's name, as Slack does.
    fn sync_composer_placeholder(&mut self, cx: &mut Context<Self>) {
        // Slack's note-to-self DM prompts "Jot something down" instead of "Message <you>";
        // a private channel's prompt carries a lock instead of "#".
        let placeholder = match self.fixtures.conversation(&self.channel) {
            Some(c)
                if c.kind() == ConversationKind::Im
                    && c.user.as_ref() == Some(&self.fixtures.self_user) =>
            {
                Placeholder::Text("Jot something down".into())
            }
            Some(c) if c.kind() == ConversationKind::PrivateChannel => {
                Placeholder::Private(c.name.as_deref().unwrap_or(c.id.as_str()).to_owned().into())
            }
            _ => Placeholder::Text(
                format!(
                    "Message {}",
                    self.label(&self.channel).replacen("# ", "#", 1)
                )
                .into(),
            ),
        };
        self.composer
            .update(cx, |composer, cx| composer.set_placeholder(placeholder, cx));
    }

    /// Hand the current status / presence / pause / focus mode to the views that show it.
    fn push_self_state(&mut self, cx: &mut Context<Self>) {
        let state = self.self_state.clone();
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_self_state(state.clone(), cx));
        let schedule = self.preferences.read(cx).settings().schedule_summary();
        self.user_menu
            .update(cx, |menu, cx| menu.set_state(state.clone(), schedule, cx));
        self.profile
            .update(cx, |profile, cx| profile.set_self_state(state, cx));
        cx.notify();
    }

    /// A `file://` URL for `path`, percent-encoded (Finder opens folder URLs).
    fn file_url(path: &std::path::Path) -> String {
        let mut url = String::from("file://");
        for byte in path.as_os_str().to_string_lossy().bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                    url.push(byte as char)
                }
                _ => url.push_str(&format!("%{byte:02X}")),
            }
        }
        url
    }

    /// slackless has no download manager, so like the browser this reveals the user's
    /// Downloads folder.
    fn open_downloads(&mut self, cx: &mut Context<Self>) {
        if let Some(home) = std::env::var_os("HOME") {
            let mut downloads = std::path::PathBuf::from(home);
            downloads.push("Downloads");
            cx.open_url(&Self::file_url(&downloads));
        }
    }

    fn user_menu_event(
        &mut self,
        event: &UserMenuEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            UserMenuEvent::EditStatus => {
                self.user_menu_open = false;
                self.open_status_picker(window, cx);
            }
            UserMenuEvent::ClearStatus => self.self_state.set_status(None),
            UserMenuEvent::SetAway(away) => self.self_state.away = *away,
            UserMenuEvent::Pause(until) => {
                self.self_state.paused_until = *until;
                self.user_menu_open = false;
                if !self.fixtures.sample {
                    match *until {
                        Some(ts) => {
                            let secs = ts.saturating_sub(now_secs() as i64).max(0) as u64;
                            let minutes = secs.div_ceil(60).max(1) as u32;
                            self.bridge.send(Command::SetDndSnooze { minutes });
                        }
                        None => self.bridge.send(Command::EndDndSnooze),
                    }
                }
            }
            UserMenuEvent::OpenProfile => {
                self.user_menu_open = false;
                self.open_profile(self.fixtures.self_user.clone(), cx);
            }
            UserMenuEvent::OpenPreferences => {
                self.user_menu_open = false;
                self.open_preferences(None, window, cx);
            }
            UserMenuEvent::OpenSchedule => {
                self.user_menu_open = false;
                self.open_preferences(Some(Section::Notifications), window, cx);
            }
            UserMenuEvent::OpenPauseCustom => {
                self.user_menu_open = false;
                self.user_menu
                    .update(cx, |menu, cx| menu.open_pause_dialog(window, cx));
            }
            UserMenuEvent::OpenAccount => {
                self.user_menu_open = false;
                // Slack opens the workspace's account settings in the browser.
                let domain = self
                    .fixtures
                    .workspaces
                    .get(self.workspace)
                    .map(|team| team.domain.as_str())
                    .unwrap_or_default();
                let url = if domain.is_empty() {
                    "https://slack.com/account/settings".to_owned()
                } else {
                    format!("https://{domain}.slack.com/account/settings")
                };
                cx.open_url(&url);
            }
            UserMenuEvent::OpenDownloads => {
                self.user_menu_open = false;
                self.open_downloads(cx);
            }
            UserMenuEvent::SignOut => {
                self.user_menu_open = false;
                self.sign_out(self.workspace, cx);
            }
        }
        if !self.user_menu_open {
            // Forget hover state so a closed menu cannot come back with its submenu open.
            self.user_menu.update(cx, |menu, cx| menu.closed(cx));
        }
        self.push_self_state(cx);
    }

    /// Sign out of the workspace at `index`: Slack forgets its session and returns to sign-in;
    /// slackless removes the stored session (Keychain included) and quits, and the next launch
    /// runs the sign-in helper.
    fn sign_out(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(team) = self.fixtures.workspaces.get(index) {
            if let Err(e) =
                slackless_auth::SessionStore::new().and_then(|store| store.remove(&team.id))
            {
                eprintln!("slackless: could not remove the stored session ({e})");
            }
        }
        cx.quit();
    }

    /// The Preferences window in a gpui-kit dialog, optionally at `section`.
    fn open_preferences(
        &mut self,
        section: Option<Section>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(section) = section {
            self.preferences
                .update(cx, |prefs, cx| prefs.show(section, cx));
        }
        let preferences = self.preferences.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Preferences")
                .w(px(920.))
                .child(preferences.clone())
        });
    }

    fn open_status_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_hover_card(cx);
        let status = self.self_state.status.clone();
        let workspace = SharedString::from(self.fixtures.workspaces[self.workspace].name.clone());
        self.status_picker.update(cx, |picker, cx| {
            picker.edit(status.as_ref(), workspace, window, cx)
        });
        let picker = self.status_picker.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            // Slack centers its modal and titles it large and bold.
            dialog
                .title(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::BOLD)
                        .child("Set a status"),
                )
                .centered()
                .w(px(520.))
                .child(picker.clone())
        });
        // Slack opens the dialog with the caret in the status field.
        let picker = self.status_picker.clone();
        window.defer(cx, move |window, cx| {
            picker.update(cx, |picker, cx| picker.focus(window, cx))
        });
    }

    /// The Cmd+K switcher in a dialog; the filter is focused once the dialog is up.
    fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let switcher = self.switcher.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.w(px(560.)).child(switcher.clone())
        });
        let switcher = self.switcher.clone();
        window.defer(cx, move |window, cx| {
            switcher.update(cx, |switcher, cx| switcher.open(window, cx));
        });
    }

    /// Show the emoji picker anchored at `position`, routing the pick to `target`.
    /// Opens the image viewer over the conversation's images, the clicked one showing.
    fn open_image_viewer(
        &mut self,
        channel: ChannelId,
        images: Arc<Vec<ViewerImage>>,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if images.is_empty() {
            return;
        }
        let label = self
            .fixtures
            .conversation(&channel)
            .map(|conversation| self.fixtures.conversation_label(conversation))
            .unwrap_or_default();
        let viewer = ImageViewer::new(self.fixtures.clone(), label, images, index, cx);
        let subscription = cx.subscribe_in(
            &viewer,
            window,
            |this: &mut App, _, event: &ViewerEvent, _, cx| match event {
                ViewerEvent::Close => this.close_image_viewer(cx),
            },
        );
        // A replaced viewer drops its subscription with it.
        self._subscriptions.push(subscription);
        self.image_viewer = Some(viewer);
        cx.notify();
    }

    /// Closes the image viewer, dropping the large rendition it holds.
    fn close_image_viewer(&mut self, cx: &mut Context<Self>) {
        if self.image_viewer.take().is_some() {
            cx.notify();
        }
    }

    fn open_picker(
        &mut self,
        target: PickerTarget,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker = Some((target, position));
        self.emoji_picker
            .update(cx, |picker, cx| picker.open(window, cx));
        cx.notify();
    }

    /// A one-key shortcut on the focused message, routed through the transcript's shared
    /// [`MessageAction`] handler. Inert while an overlay owns the screen (a picker, the
    /// reminder menu) or while the focus has left the root (a field, a dialog, an open
    /// menu) — on top of GPUI already withholding character keys from focused fields.
    fn focus_message(
        &mut self,
        action: MessageAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.picker.is_some()
            || self.remind_menu.is_some()
            || !window
                .focused(cx)
                .is_none_or(|focused| focused == self.focus)
        {
            return;
        }
        self.messages
            .update(cx, |list, cx| list.focus_action(action, window, cx));
    }

    /// Slack's M key: the reminder presets for the focused message, dropped at the mouse
    /// like the row's context menu. A pick routes through the transcript's handler and the
    /// menu dismisses itself (its [`DismissEvent`] closes the overlay).
    fn open_remind_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.remind_menu.is_some()
            || self.picker.is_some()
            || !window
                .focused(cx)
                .is_none_or(|focused| focused == self.focus)
            || self.messages.read(cx).focused(cx).is_none()
        {
            return;
        }
        let position = window.mouse_position();
        let menu = PopupMenu::build(window, cx, |menu, _, _| {
            let mut menu = menu;
            for preset in Remind::ALL {
                let messages = self.messages.downgrade();
                menu = menu.item(
                    PopupMenuItem::new(preset.label().to_owned())
                        .icon(IconName::Clock)
                        .on_click(move |_, window, cx| {
                            let _ = messages.update(cx, |list, cx| {
                                list.focus_action(MessageAction::RemindMe(preset), window, cx)
                            });
                        }),
                );
            }
            menu
        });
        menu.focus_handle(cx).focus(window, cx);
        let subscription = cx.subscribe_in(
            &menu,
            window,
            |this: &mut Self, _, _: &DismissEvent, window, cx| {
                this.close_remind_menu(window, cx);
            },
        );
        self.remind_menu = Some((menu, position, subscription));
        cx.notify();
    }

    fn close_remind_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.remind_menu.take().is_some() {
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Post a reaction, edit or deletion a view already shows to Slack.
    fn post_change(&self, channel: &ChannelId, change: &MessageChange) {
        let channel = channel.clone();
        self.bridge.send(match change {
            MessageChange::React { ts, name, add } => {
                let (ts, name) = (ts.clone(), name.clone());
                if *add {
                    Command::AddReaction { channel, ts, name }
                } else {
                    Command::RemoveReaction { channel, ts, name }
                }
            }
            MessageChange::Edit { ts, content } => Command::EditMessage {
                channel,
                ts: ts.clone(),
                content: content.clone(),
            },
            MessageChange::Delete { ts } => Command::DeleteMessage {
                channel,
                ts: ts.clone(),
            },
            MessageChange::Pin { ts, pinned } => Command::SetPinned {
                channel,
                ts: ts.clone(),
                pinned: *pinned,
            },
            MessageChange::MarkUnread { ts } => Command::MarkRead {
                channel,
                ts: ts.clone(),
            },
            MessageChange::BlockAction(dispatch) => Command::BlockAction {
                bot: dispatch.bot.clone(),
                app_id: dispatch.app_id.clone(),
                team: dispatch.team.clone(),
                action: dispatch.action.clone(),
                container: dispatch.container.clone(),
            },
        });
    }

    /// Route a pick: composers get the emoji, messages a reaction.
    fn pick_emoji(&mut self, choice: &EmojiChoice, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, _)) = self.picker.take() else {
            return;
        };
        match target {
            PickerTarget::Composer => {
                self.composer
                    .update(cx, |composer, cx| composer.insert_emoji(choice, window, cx));
            }
            PickerTarget::ThreadComposer => {
                self.thread
                    .update(cx, |thread, cx| thread.insert_emoji(choice, window, cx));
            }
            PickerTarget::ThreadsComposer => {
                self.threads
                    .update(cx, |view, cx| view.insert_emoji(choice, window, cx));
            }
            PickerTarget::ComposeView => {
                self.compose
                    .update(cx, |view, cx| view.insert_emoji(choice, window, cx));
            }
            PickerTarget::MessageEdit => {
                self.messages
                    .update(cx, |list, cx| list.insert_edit_emoji(choice, window, cx));
            }
            PickerTarget::ThreadEdit => {
                self.thread.update(cx, |thread, cx| {
                    thread.insert_edit_emoji(choice, window, cx)
                });
            }
            PickerTarget::Message { channel, ts } => {
                self.messages.update(cx, |list, cx| {
                    list.add_reaction(&channel, &ts, &choice.name, cx)
                });
                self.focus.focus(window, cx);
            }
            PickerTarget::ThreadMessage(ts) => {
                self.thread
                    .update(cx, |thread, cx| thread.add_reaction(&ts, &choice.name, cx));
                self.focus.focus(window, cx);
            }
        }
        let frequent = self.emoji_picker.read(cx).frequent().to_vec();
        if let Some(write) = self.bridge.remember_frequent_emoji(&frequent) {
            cx.background_executor().spawn(write).detach();
        }
        cx.notify();
    }

    /// Slack's profile pane. Opened from an open thread it stacks over the thread, which
    /// stays loaded under it: the transcript drops the thread's tint while the profile
    /// shows, and the profile's back chevron returns to the thread.
    fn open_profile(&mut self, user: UserId, cx: &mut Context<Self>) {
        self.close_hover_card(cx);
        let over_thread = self.thread_open;
        self.profile
            .update(cx, |profile, cx| profile.open(user, over_thread, cx));
        self.profile_open = true;
        if over_thread {
            self.messages
                .update(cx, |list, cx| list.set_open_thread(None, cx));
        }
        self.pins_open = false;
        cx.notify();
    }

    // ---------------------------------------------------------------------------
    // Slack's member profile hover card (`hover_card.rs`): dwell open, grace close.
    // ---------------------------------------------------------------------------

    /// The pointer entered a profile hover trigger (an @mention pill or an author name):
    /// dwell before showing, so a sweeping pointer never flashes the card. A user the
    /// snapshot lacks (an external member) is resolved right away, so the card has a name
    /// by the time the dwell ends.
    pub(crate) fn profile_hover_entered(
        &mut self,
        trigger: hover_card::Trigger,
        cx: &mut Context<Self>,
    ) {
        let unchanged = self.profile_hover.trigger.as_ref() == Some(&trigger);
        self.profile_hover.grace = None;
        self.profile_hover.trigger = Some(trigger.clone());
        if self.fixtures.user(&trigger.user).is_none() {
            self.resolve_users(std::iter::once(trigger.user.clone()));
        }
        if unchanged {
            return;
        }
        let (key, user) = (trigger.key.clone(), trigger.user.clone());
        self.profile_hover.dwell = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(hover_card::DWELL_MS))
                .await;
            let _ = this.update(cx, |app, cx| app.hover_dwell_elapsed(&key, &user, cx));
        }));
    }

    /// The pointer left the trigger registered under `key`. A leave from any other element
    /// (the pointer crossed straight onto a newer trigger before this frame's hover diff)
    /// is stale and drops.
    pub(crate) fn profile_hover_left(&mut self, key: &SharedString, cx: &mut Context<Self>) {
        if !self
            .profile_hover
            .trigger
            .as_ref()
            .is_some_and(|trigger| trigger.key == *key)
        {
            return;
        }
        self.profile_hover.trigger = None;
        self.profile_hover.dwell = None; // left before the dwell: no card
        self.hover_card_maybe_close(cx);
    }

    /// The dwell elapsed with the pointer still on `key`'s trigger: show the card.
    fn hover_dwell_elapsed(&mut self, key: &SharedString, user: &UserId, cx: &mut Context<Self>) {
        if !self
            .profile_hover
            .trigger
            .as_ref()
            .is_some_and(|trigger| trigger.key == *key && trigger.user == *user)
        {
            return; // the pointer moved on (or left) before the dwell ended
        }
        let trigger = self
            .profile_hover
            .trigger
            .clone()
            .expect("the guard above matched the trigger");
        self.profile_hover.card = Some(trigger);
        self.profile_hover.grace = None;
        cx.notify();
    }

    /// The grace elapsed without the pointer returning to the trigger or into the card.
    fn hover_grace_elapsed(&mut self, cx: &mut Context<Self>) {
        if self.profile_hover.trigger.is_some() || self.profile_hover.inside {
            return; // the pointer came back before the grace closed the card
        }
        self.profile_hover.grace = None;
        if self.profile_hover.card.take().is_some() {
            cx.notify();
        }
    }

    /// The pointer entered / left the open card: entering cancels a pending close (the
    /// pointer crossed the gap from its trigger); leaving starts the grace.
    fn hover_card_hovered(&mut self, inside: bool, cx: &mut Context<Self>) {
        if self.profile_hover.inside == inside {
            return;
        }
        self.profile_hover.inside = inside;
        if inside {
            self.profile_hover.grace = None;
        } else {
            self.hover_card_maybe_close(cx);
        }
    }

    /// Start the card's close grace, unless the pointer is inside the card or a grace is
    /// already running.
    fn hover_card_maybe_close(&mut self, cx: &mut Context<Self>) {
        if self.profile_hover.card.is_none()
            || self.profile_hover.inside
            || self.profile_hover.grace.is_some()
        {
            return;
        }
        self.profile_hover.grace = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(hover_card::GRACE_MS))
                .await;
            let _ = this.update(cx, |app, cx| app.hover_grace_elapsed(cx));
        }));
    }

    /// Close the card outright: a click outside, Escape, a conversation switch, or an
    /// action opened from it. The trigger stays, so the card does not come back until the
    /// pointer re-enters it.
    fn close_hover_card(&mut self, cx: &mut Context<Self>) {
        let closed = self.profile_hover.card.take().is_some();
        self.profile_hover.grace = None;
        self.profile_hover.dwell = None;
        if closed {
            cx.notify();
        }
    }

    /// The hover card's "Message": open the DM with `user` — the existing IM, else (live)
    /// have Slack open (or create) it, like the New message view's people picks.
    fn message_user(&mut self, user: &UserId, window: &mut Window, cx: &mut Context<Self>) {
        self.close_hover_card(cx);
        if let Some(dm) = self
            .fixtures
            .dms
            .iter()
            .find(|dm| dm.user.as_ref() == Some(user))
            .map(|dm| dm.id.clone())
        {
            self.select(dm, cx);
        } else if self.bridge.store().is_some() {
            self.bridge.send(Command::OpenDm {
                users: vec![user.clone()],
            });
        } else {
            notify(
                "Needs a live workspace",
                "Sign in to start a new DM with this person.",
                window,
                cx,
            );
        }
    }

    /// Slack's channel details dialog, opened by clicking the channel name in the header or
    /// its members chip: the About and Members tabs over the channel's record and member
    /// list. A creator missing from the snapshot resolves through the core, like any other
    /// unknown person.
    fn open_channel_details(
        &mut self,
        channel: &ChannelId,
        tab: channel_details::DetailsTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(conversation) = self.fixtures.conversation(channel).cloned() else {
            return;
        };
        if let Some(creator) = &conversation.creator {
            self.resolve_users(std::iter::once(creator.clone()));
        }
        // Live mode without a member list yet keeps the Members tab on its placeholder until
        // `Event::ConversationInfo` lands.
        let stored = self.member_ids(channel, &conversation).to_vec();
        let loading = stored.is_empty() && self.bridge.store().is_some();
        self.details.update(cx, |details, cx| {
            details.open(
                self.fixtures.clone(),
                channel.clone(),
                stored,
                loading,
                tab,
                cx,
            )
        });
        self.bridge.send(Command::FetchChannelManagers {
            channel: channel.clone(),
        });
        // Slack's title row: a private channel's lock, else the hash, beside the name in the
        // dialog's bold display size (the close X is the dialog's own).
        let title_name: SharedString = if conversation.is_private {
            conversation
                .name
                .as_deref()
                .unwrap_or_else(|| channel.as_str())
                .to_owned()
                .into()
        } else {
            format!(
                "# {}",
                conversation
                    .name
                    .as_deref()
                    .unwrap_or_else(|| channel.as_str())
            )
            .into()
        };
        let title_private = conversation.is_private;
        let details = self.details.clone();
        let focus_find_people = Rc::new(Cell::new(true));
        window.open_dialog(cx, move |dialog, window, cx| {
            // Slack puts the caret in "Find people" as the dialog opens.
            if focus_find_people.replace(false) {
                details.read(cx).find_people_focus(cx).focus(window, cx);
            }
            let mut title = h_flex()
                .items_center()
                .gap_1p5()
                .text_size(px(22.))
                .font_weight(FontWeight::BOLD);
            title = if title_private {
                title
                    .child(Icon::new(IconName::Lock))
                    .child(title_name.clone())
            } else {
                title.child(title_name.clone())
            };
            // Slack's 580pt dialog: its rules run edge to edge, so the content pads itself and
            // the title row carries the close X.
            let title_row = h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .px(px(28.))
                .child(title)
                .child(
                    Button::new("details-close")
                        .ghost()
                        .icon(IconName::Close)
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                );
            dialog
                .title(title_row)
                .w(px(580.))
                .px_0()
                .pb_0()
                .close_button(false)
                .child(details.clone())
        });
    }

    /// The channel details dialog's actions.
    fn details_event(
        &mut self,
        event: &ChannelDetailsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ChannelDetailsEvent::Leave(channel) => {
                window.close_dialog(cx);
                self.leave(channel.clone(), window, cx);
            }
            ChannelDetailsEvent::SetTopic { channel, topic } => {
                self.bridge.send(Command::SetChannelTopic {
                    channel: channel.clone(),
                    topic: topic.clone(),
                });
            }
            ChannelDetailsEvent::SetPurpose { channel, purpose } => {
                self.bridge.send(Command::SetChannelPurpose {
                    channel: channel.clone(),
                    purpose: purpose.clone(),
                });
            }
            ChannelDetailsEvent::StartHuddle => self.start_huddle(cx),
            ChannelDetailsEvent::SetNotifyLevel { channel, level } => {
                self.bridge.send(Command::SetNotifyLevel {
                    channel: channel.clone(),
                    level: *level,
                });
            }
            ChannelDetailsEvent::OpenProfile(user) => {
                window.close_dialog(cx);
                self.open_profile(user.clone(), cx);
            }
        }
    }

    fn select(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        if channel != self.channel {
            self.back.push(self.channel.clone());
            self.forward.clear();
        }
        self.navigate(channel, cx);
    }

    /// Show `channel` as the user's choice, leaving any full-column view (search, drafts,
    /// later). History is the caller's: [`App::select`] records it, back/forward replay it.
    fn navigate(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        self.chosen = true;
        self.close_full_column_views(cx);
        self.close_hover_card(cx);
        cx.notify();
        self.show_conversation(channel, cx);
    }

    /// Slack's back arrow (Cmd+[): the conversation before this one.
    fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(previous) = self.back.pop() {
            self.forward.push(self.channel.clone());
            self.navigate(previous, cx);
        }
    }

    /// Slack's forward arrow (Cmd+]): undo the last back.
    fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = self.forward.pop() {
            self.back.push(self.channel.clone());
            self.navigate(next, cx);
        }
    }

    /// The history clock's "Recent" list: the open conversation, then the recently left
    /// ones still in the snapshot, each with Slack's kind icon.
    fn recent_entries(&self) -> Vec<(ChannelId, SharedString, IconName)> {
        /// Slack lists about this many before its "Show more".
        const SHOWN: usize = 25;
        std::iter::once(&self.channel)
            .chain(&self.recent)
            .filter_map(|id| {
                let conversation = self.fixtures.conversation(id)?;
                let (label, icon) = match conversation.kind() {
                    ConversationKind::PublicChannel => (
                        conversation.name.clone().unwrap_or_else(|| id.to_string()),
                        IconName::Hash,
                    ),
                    ConversationKind::PrivateChannel => (
                        conversation.name.clone().unwrap_or_else(|| id.to_string()),
                        IconName::Lock,
                    ),
                    ConversationKind::Im => (
                        self.fixtures.conversation_label(conversation),
                        IconName::User,
                    ),
                    ConversationKind::Mpim => (
                        self.fixtures.conversation_label(conversation),
                        IconName::Users,
                    ),
                };
                Some((id.clone(), SharedString::from(label), icon))
            })
            .take(SHOWN)
            .collect()
    }

    /// A link to `channel` like Slack's "Copy link": the workspace's archive URL.
    fn conversation_link(&self, channel: &ChannelId) -> String {
        match self.fixtures.workspaces.first() {
            Some(team) if !team.domain.is_empty() => {
                format!("https://{}.slack.com/archives/{channel}", team.domain)
            }
            Some(team) => format!("https://app.slack.com/client/{}/{channel}", team.id),
            None => format!("https://app.slack.com/client/{channel}"),
        }
    }

    /// Slack's huddle link for the open conversation (its web client runs the call).
    fn huddle_link(&self) -> String {
        let team = self
            .fixtures
            .workspaces
            .first()
            .map(|team| team.id.to_string())
            .unwrap_or_default();
        format!("https://app.slack.com/huddle/{team}/{}", self.channel)
    }

    /// The header's huddle button: the sample workspace toggles its huddle bar; a live
    /// conversation starts or joins its huddle in Slack's web client (no media stack here).
    fn start_huddle(&mut self, cx: &mut Context<Self>) {
        if self.huddle_here() {
            self.huddle_open = !self.huddle_open;
            cx.notify();
        } else {
            cx.open_url(&self.huddle_link());
        }
    }

    /// The header's search icon: the top search field scoped to this channel, like Slack's
    /// "Search in #channel".
    fn search_in_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scope = self
            .fixtures
            .conversation(&self.channel)
            .filter(|c| {
                matches!(
                    c.kind(),
                    ConversationKind::PublicChannel | ConversationKind::PrivateChannel
                )
            })
            .and_then(|c| c.name.as_deref())
            .map(|name| format!("in:#{name} "))
            .unwrap_or_default();
        self.search
            .update(cx, |search, cx| search.set_value(scope, window, cx));
        self.search.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// Search messages for `query`, showing results in place of the message column. The
    /// pill's Enter and the dropdown's rows both land here; the dropdown's filter chips
    /// (and only its non-empty query) narrow the run and join its history.
    fn run_message_search(&mut self, query: String, cx: &mut Context<Self>) {
        let filters = self
            .search_menu
            .read(cx)
            .search_filters(&self.channel, &self.fixtures.self_user);
        self.results.update(cx, |results, cx| {
            results.run_with_filters(query.clone(), filters, cx)
        });
        self.searching = true;
        self.drafts_open = false;
        self.later_open = false;
        if !query.trim().is_empty() {
            self.search_menu
                .update(cx, |menu, cx| menu.record_search(&query, cx));
            let history = self.search_menu.read(cx).history().to_vec();
            if let Some(write) = self.bridge.remember_search_history(&history) {
                cx.background_executor().spawn(write).detach();
            }
        }
        cx.notify();
    }

    /// Open the search pill's dropdown under the pill: the empty-query sections (unreads
    /// & drafts, recent places) are computed here, once per open.
    fn open_search_menu(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.search_menu_snapshot(cx);
        let query = self.search.read(cx).value().to_string();
        self.search_menu
            .update(cx, |menu, cx| menu.open(snapshot, &query, cx));
        self.search_menu_open = true;
        cx.notify();
    }

    /// Fold the dropdown and put the field's placeholder back to its resting text,
    /// leaving focus alone: a blur, a search run or a workspace switch folds this way, so
    /// whatever took the focus (a dialog, the field itself) keeps it.
    fn fold_search_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_menu_open {
            return;
        }
        self.search_menu_open = false;
        self.search.update(cx, |state, cx| {
            state.set_placeholder(SharedString::from(SEARCH_PLACEHOLDER), window, cx)
        });
        cx.notify();
    }

    /// Fold the dropdown and blur the pill (Escape or a click outside it): the open menu
    /// ghosts the row under the pointer or the arrows.
    fn close_search_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_menu_open {
            return;
        }
        self.fold_search_menu(window, cx);
        self.focus.focus(window, cx);
    }

    /// The dropdown's empty-query rows: conversations with unread messages (the sidebar's
    /// own read state) or a saved draft, then the recently left ones, current excluded —
    /// it is the place the user is jumping from.
    fn search_menu_snapshot(&self, cx: &Context<Self>) -> Snapshot {
        let mut unreads = self.sidebar.read(cx).unread_ids();
        let drafts = self.drafts.read(cx);
        for conversation in self.fixtures.channels.iter().chain(&self.fixtures.dms) {
            if conversation.id != self.channel
                && !unreads.contains(&conversation.id)
                && drafts.draft(self.workspace, &conversation.id).is_some()
            {
                unreads.push(conversation.id.clone());
            }
        }
        // Slack lists them by latest activity, newest first.
        unreads.sort_by_cached_key(|id| {
            std::cmp::Reverse(self.fixtures.counts.get(id).and_then(|n| n.latest.clone()))
        });
        let recents = self
            .recent
            .iter()
            .filter(|id| self.fixtures.conversation(id).is_some())
            .cloned()
            .collect();
        let (mentions, threads) = self.sidebar.read(cx).search_unread_counts();
        Snapshot {
            unreads,
            recents,
            mentions,
            threads,
        }
    }

    /// "Leave channel": it drops out of the sidebar at once and the view steps back, as in
    /// Slack; the core leaves on Slack and stores the channel as left.
    fn leave(&mut self, channel: ChannelId, window: &mut Window, cx: &mut Context<Self>) {
        self.bridge
            .send(Command::LeaveConversation(channel.clone()));
        self.counts.remove(&channel);
        self.back.retain(|c| *c != channel);
        self.forward.retain(|c| *c != channel);
        self.recent.retain(|c| *c != channel);
        if let Some(write) = self.bridge.remember_recent_places(&self.recent) {
            cx.background_executor().spawn(write).detach();
        }
        self.update_menus(cx);
        self.reload_live(window, cx);
        if self.channel == channel {
            let next = self
                .back
                .pop()
                .unwrap_or_else(|| first_conversation(&self.fixtures));
            self.navigate(next, cx);
        }
    }

    /// The huddle button's chevron: Slack's huddle menu.
    fn huddle_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let link = self.huddle_link();
        let start = if self.huddle_here() {
            "Show huddle"
        } else {
            "Start huddle"
        };
        Button::new("hdr-huddle-menu")
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::ChevronDown)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let this = this.clone();
                let link = link.clone();
                menu.item(
                    PopupMenuItem::new(start)
                        .icon(IconName::Headphones)
                        .on_click(move |_, _, cx| {
                            let _ = this.update(cx, |app, cx| app.start_huddle(cx));
                        }),
                )
                .item(
                    PopupMenuItem::new("Copy huddle link")
                        .icon(IconName::Link)
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(link.clone()))
                        }),
                )
            })
    }

    /// The header bell: Slack's "Notify you about…" menu for this conversation, checked at
    /// its current level; the icon is a crossed-out bell while muted.
    fn notify_menu(
        &self,
        conversation: Option<&Conversation>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let channel = conversation.map(|c| c.id.clone());
        let level = channel
            .as_ref()
            .and_then(|c| self.fixtures.notify_levels.get(c).copied())
            .unwrap_or(NotifyLevel::Everything);
        let icon = if level == NotifyLevel::Muted {
            IconName::BellOff
        } else {
            IconName::Bell
        };
        Button::new("hdr-bell")
            .ghost()
            .with_size(Size::Small)
            .icon(icon)
            .disabled(channel.is_none())
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                menu = menu.label("Notify you about…");
                for (choice, label) in [
                    (NotifyLevel::Everything, "All new posts"),
                    (NotifyLevel::Mentions, "Just mentions"),
                    (NotifyLevel::Muted, "Mute and hide"),
                ] {
                    let (this, channel) = (this.clone(), channel.clone());
                    menu = menu.item(PopupMenuItem::new(label).checked(choice == level).on_click(
                        move |_, _, cx| {
                            let Some(channel) = channel.clone() else {
                                return;
                            };
                            let _ = this.update(cx, |app, _| {
                                app.bridge.send(Command::SetNotifyLevel {
                                    channel,
                                    level: choice,
                                })
                            });
                        },
                    ));
                }
                menu
            })
    }

    /// The header's ⋮ menu: what slackless can do for this conversation (Slack's menu minus
    /// the workflow, template, split-view and window items it has no backend for).
    fn conversation_menu(
        &self,
        conversation: Option<&Conversation>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let channel = conversation.map(|c| c.id.clone());
        let channel_name = conversation
            .filter(|c| {
                matches!(
                    c.kind(),
                    ConversationKind::PublicChannel | ConversationKind::PrivateChannel
                )
            })
            .and_then(|c| c.name.as_deref())
            .map(|name| format!("#{name}"));
        let peer = conversation
            .filter(|c| c.kind() == ConversationKind::Im)
            .and_then(|c| c.user.clone());
        let link = channel.as_ref().map(|id| self.conversation_link(id));
        Button::new("hdr-more")
            .ghost()
            .with_size(Size::Small)
            .icon(IconName::EllipsisVertical)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                if let Some(user) = peer.clone() {
                    let this = this.clone();
                    menu = menu.item(
                        PopupMenuItem::new("View full profile")
                            .icon(IconName::CircleUser)
                            .on_click(move |_, _, cx| {
                                let _ =
                                    this.update(cx, |app, cx| app.open_profile(user.clone(), cx));
                            }),
                    );
                }
                if let Some(link) = link.clone() {
                    let label = if channel_name.is_some() {
                        "Copy link to channel"
                    } else {
                        "Copy link to conversation"
                    };
                    menu = menu.item(PopupMenuItem::new(label).icon(IconName::Link).on_click(
                        move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(link.clone()))
                        },
                    ));
                }
                if let Some(name) = channel_name.clone() {
                    menu = menu.item(
                        PopupMenuItem::new("Copy channel name")
                            .icon(IconName::Copy)
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(name.clone()))
                            }),
                    );
                }
                if let (Some(_), Some(channel)) = (&channel_name, channel.clone()) {
                    let this = this.clone();
                    menu = menu.separator().item(
                        PopupMenuItem::new("Leave channel")
                            .icon(IconName::LogOut)
                            .on_click(move |_, window, cx| {
                                let _ = this
                                    .update(cx, |app, cx| app.leave(channel.clone(), window, cx));
                            }),
                    );
                }
                menu
            })
    }

    /// Open `channel` in the transcript (a no-op when it already is): the sidebar selection,
    /// the core's open conversation, its history and its draft follow.
    fn show_conversation(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        if channel == self.channel {
            return;
        }
        self.close_hover_card(cx);
        if let Some(write) = self.bridge.remember_conversation(channel.clone()) {
            cx.background_executor().spawn(write).detach();
        }
        /// Recent conversations kept for the history menu.
        const RECENT_MAX: usize = 50;
        let left = self.channel.clone();
        self.recent.retain(|c| *c != left && *c != channel);
        self.recent.insert(0, left);
        self.recent.truncate(RECENT_MAX);
        if let Some(write) = self.bridge.remember_recent_places(&self.recent) {
            cx.background_executor().spawn(write).detach();
        }
        self.save_draft(cx);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_selected(channel.clone(), cx));
        self.bridge
            .send(Command::CloseConversation(self.channel.clone()));
        self.bridge.send(Command::OpenConversation(channel.clone()));
        self.messages
            .update(cx, |list, cx| list.set_conversation(channel.clone(), cx));
        self.load_history(&channel, cx);
        self.channel = channel;
        self.open_members = None;
        self.members_hover = (false, false);
        self.channel_people = None;
        self.sync_connect_orgs(cx);
        self.load_join_people(cx);
        self.close_thread(cx);
        self.tab = ConversationTab::Messages;
        // An open pins panel follows the conversation: fetch the new one's pins.
        if self.pins_open {
            self.load_tab_data(&ConversationTab::Pins);
        }
        self.load_draft(cx);
        self.sync_composer_placeholder(cx);
        cx.notify();
        self.update_menus(cx);
        self.show_typing(cx);
    }

    /// Live mode: fold a core event into the views.
    fn on_core_event(&mut self, event: &Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Conversations(_) | Event::UsersChanged(_) | Event::SidebarChanged => {
                self.reload_live(window, cx)
            }
            Event::Counts(counts) => {
                let threads = &counts.threads;
                let (has_unreads, mentions) = (threads.has_unreads, threads.mention_count);
                self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.set_thread_counts(has_unreads, mentions, cx)
                });
                self.counts = [&counts.channels, &counts.ims, &counts.mpims]
                    .into_iter()
                    .flatten()
                    .map(|entry| (entry.id.clone(), entry.clone()))
                    .collect();
                self.reload_live(window, cx);
            }
            Event::StoreChanged { channel } => {
                if *channel == self.channel {
                    self.load_history(channel, cx);
                    // The channel's people are the history's recent authors: stale now.
                    self.channel_people = None;
                    self.load_join_people(cx);
                }
                self.refresh_dm_latest(channel, cx);
                // The open Unreads view follows a message landing in any channel.
                if self.unreads_open {
                    self.refresh_unreads(cx);
                }
                // The open thread: a fetched replies page landed, or a live reply arrived.
                if self.thread_open {
                    let open = self.thread.read(cx).open_thread();
                    if let Some((thread_channel, thread_ts)) = open {
                        if thread_channel == *channel {
                            self.load_thread(&thread_channel, &thread_ts, cx);
                        }
                    }
                }
            }
            Event::AuthExpired(error) => eprintln!(
                "slackless: Slack rejected the saved session ({error}); sign in again with `slackless-gui --sign-in`"
            ),
            Event::CommandFailed { command, error } if error == "unknown_command" => notify(
                "Unknown command",
                format!("{command} is not a valid command."),
                window,
                cx,
            ),
            Event::CommandFailed { command, error } => {
                notify(command, format!("Slack refused it ({error})."), window, cx)
            }
            Event::Typing {
                channel,
                user,
                thread_ts,
            } => self.someone_typing(channel, user, thread_ts.as_ref(), cx),
            Event::MessagePosted { channel, message } => {
                self.message_posted(channel, message, window, cx)
            }
            Event::Error(error) => eprintln!("slackless: {error}"),
            Event::Presence { users, presence } => {
                // Every snapshot shares this map: update it in place and repaint, with no
                // store reload however often presence changes.
                self.presence
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend(users.iter().map(|user| (user.clone(), presence.clone())));
                window.refresh();
            }
            Event::DndPaused { user, paused } => {
                self.peer_dnd_paused.insert(user.clone(), *paused);
                cx.notify();
            }
            Event::ThreadsView(view) => {
                self.threads
                    .update(cx, |threads, cx| threads.set_remote((**view).clone(), cx));
                cx.notify();
            }
            Event::ActivityView { feed } => match feed.as_deref() {
                Ok(feed) => {
                    self.activity
                        .update(cx, |view, cx| view.show_remote(feed.clone(), cx));
                }
                Err(text) => {
                    self.activity
                        .update(cx, |view, cx| view.show_error(text, cx));
                }
            },
            Event::ConversationOpened(channel) => {
                let pending = self.pending_dm.take();
                self.select(channel.clone(), cx);
                if let Some(content) = pending {
                    self.deliver_send(content, cx);
                }
            }
            Event::ChannelManagers { channel, users } => {
                self.details.update(cx, |details, cx| {
                    details.set_managers(channel, users.clone(), cx)
                });
            }
            Event::ConversationInfo {
                channel,
                num_members,
                num_apps,
                members,
            } => {
                if let Some(count) = num_members {
                    self.member_counts.insert(channel.clone(), *count);
                }
                if let Some(apps) = num_apps {
                    self.app_counts.insert(channel.clone(), *apps);
                }
                // A late answer for a channel left since is moot: the open one's follows.
                if *channel == self.channel {
                    if !members.is_empty() {
                        self.open_members = Some((channel.clone(), members.clone()));
                        // Members `users.list` leaves out - another organization's people
                        // (a connect channel's, or guests invited to this one) - resolve on
                        // demand, the same way unknown DM peers do; the members an ordinary
                        // channel lists all sit in the directory, so nothing is asked.
                        self.resolve_users(members.iter().cloned());
                    }
                    self.sync_connect_orgs(cx);
                    // An empty list (the fetch failed) turns the dialog's Members tab from
                    // loading to its inline error.
                    self.details.update(cx, |details, cx| {
                        details.set_conversation_info(channel, members.clone(), cx)
                    });
                }
                cx.notify();
            }
            Event::PinsLoaded { channel, pins } => {
                // File pins are left to the Files & links tab; the panel lists messages.
                let messages = pins.as_ref().map_err(Clone::clone).map(|pins| {
                    pins.iter()
                        .filter_map(|pin| pin.message.clone())
                        .collect::<Vec<_>>()
                });
                if let Ok(messages) = &messages {
                    self.resolve_named_users(messages);
                }
                self.pins.insert(channel.clone(), messages);
                cx.notify();
            }
            Event::BookmarksLoaded { channel, bookmarks } => {
                self.bookmarks.insert(channel.clone(), bookmarks.clone());
                cx.notify();
            }
            Event::FilesLoaded { filter, files } => {
                if let Ok(files) = files {
                    self.resolve_users(files.iter().filter_map(|file| file.user.clone()));
                }
                match &filter.channel {
                    Some(channel) => {
                        self.files.insert(channel.clone(), files.clone());
                    }
                    // A workspace-wide list answers the Files view.
                    None => self
                        .files_page
                        .update(cx, |page, cx| page.land(filter, files, cx)),
                }
                cx.notify();
            }
            Event::Connection(state) => {
                self.connection = Some(state.clone());
                cx.notify();
            }
            Event::SelfDndUpdated(status) => {
                self.self_state.paused_until = status
                    .snooze_enabled
                    .then_some(status.snooze_endtime)
                    .flatten();
                self.push_self_state(cx);
            }
            Event::ScheduledMessagesLoaded(Ok(list)) => {
                let workspace = self.workspace;
                let entries: Vec<Entry> = list
                    .iter()
                    .map(|m| Entry {
                        id: Some(m.id.clone()),
                        workspace,
                        channel: m.channel_id.clone(),
                        label: self.label(&m.channel_id),
                        text: m.text.clone(),
                        at: m.post_at,
                    })
                    .collect();
                self.drafts.update(cx, |drafts, cx| {
                    drafts.set_remote_scheduled(workspace, entries, cx)
                });
            }
            Event::ChannelCanvasCreated { channel, canvas_id } if *channel == self.channel => {
                cx.open_url(&self.canvas_link(canvas_id));
            }
            Event::Usergroups(groups) => {
                self.usergroups = groups.clone();
                self.reload_live(window, cx);
            }
            _ => {}
        }
    }

    /// Live mode: rebuild the workspace views from a fresh store snapshot after its
    /// conversations or people changed, staying on the open conversation while it exists.
    fn reload_live(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut snapshot) = self.bridge.snapshot(&self.counts) else {
            return;
        };
        // Presence and usergroups are transient (never stored), so they ride along every snapshot.
        snapshot.presence = self.presence.clone();
        snapshot.usergroups = self.usergroups.clone();
        let fixtures = Arc::new(snapshot);
        // Refresh the views in place, as Slack does: a status change or a new channel keeps
        // every scroll position, collapsed section and open panel.
        self.sidebar
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.messages
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.thread
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.results
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.browser
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.profile
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.switcher
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.search_menu
            .update(cx, |menu, cx| menu.set_fixtures(fixtures.clone(), cx));
        self.composer
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.threads
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.unreads
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.huddles
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.directory
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.compose
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.details
            .update(cx, |view, cx| view.set_fixtures(fixtures.clone(), cx));
        self.files_page
            .update(cx, |page, cx| page.set_fixtures(fixtures.clone(), cx));
        let keep = self.chosen && fixtures.conversation(&self.channel).is_some();
        self.fixtures = fixtures;
        self.sync_connect_orgs(cx);
        self.resolve_users(self.fixtures.dms.iter().filter_map(|dm| dm.user.clone()));
        if !keep {
            self.show_conversation(first_conversation(&self.fixtures), cx);
        }
        self.sync_composer_placeholder(cx);
        self.apply_preferences(window, cx);
        if self.unreads_open {
            self.refresh_unreads(cx);
        }
        // Conversation names may have arrived: the History menu shows them.
        self.update_menus(cx);
        cx.notify();
    }

    /// Live mode: show `channel`'s stored history, read off the UI thread. A read that a newer
    /// one overtook, or whose conversation is no longer open, is dropped.
    /// Live mode: a DM's messages changed (a send, an arrival, an edit or a deletion): re-read
    /// its newest message off the UI thread so the DM list's preview and order follow, as in
    /// Slack, without rebuilding the whole snapshot.
    fn refresh_dm_latest(&mut self, channel: &ChannelId, cx: &mut Context<Self>) {
        let is_dm = self
            .fixtures
            .conversation(channel)
            .is_some_and(|c| matches!(c.kind(), ConversationKind::Im | ConversationKind::Mpim));
        let Some(read) = is_dm.then(|| self.bridge.latest(channel.clone())).flatten() else {
            return;
        };
        let channel = channel.clone();
        cx.spawn(async move |this, cx| {
            let latest = cx.background_executor().spawn(read).await;
            this.update(cx, |app, cx| {
                app.sidebar
                    .update(cx, |sidebar, cx| sidebar.set_latest(channel, latest, cx));
            })
        })
        .detach();
    }

    fn load_history(&mut self, channel: &ChannelId, cx: &mut Context<Self>) {
        let Some(read) = self.bridge.history(channel.clone(), None) else {
            return;
        };
        self.history_generation += 1;
        let generation = self.history_generation;
        let channel = channel.clone();
        cx.spawn(async move |this, cx| {
            let history = cx.background_executor().spawn(read).await;
            this.update(cx, |app, cx| {
                if app.history_generation == generation && app.channel == channel {
                    app.resolve_named_users(&history);
                    let newest = history.last().map(|message| message.ts.clone());
                    app.messages.update(cx, |list, cx| {
                        list.set_history(channel.clone(), history, cx)
                    });
                    if let Some(newest) = newest {
                        app.mark_read(&channel, newest, cx);
                    }
                }
            })
        })
        .detach();
    }

    /// Live mode: the transcript reached the top of `channel`; read the next older page from
    /// the store, off the UI thread, and prepend it. A page a newer reload overtook is
    /// dropped (the reload's `set_history` forgets the request, so the top re-asks); an empty
    /// page marks the channel fully loaded.
    fn load_older(&mut self, channel: &ChannelId, before: Ts, cx: &mut Context<Self>) {
        if self.history_complete.contains(channel) {
            return;
        }
        let Some(read) = self.bridge.history(channel.clone(), Some(before)) else {
            return;
        };
        let generation = self.history_generation;
        let channel = channel.clone();
        cx.spawn(async move |this, cx| {
            let older = cx.background_executor().spawn(read).await;
            this.update(cx, |app, cx| {
                if app.history_generation != generation || app.channel != channel {
                    return;
                }
                if older.is_empty() {
                    app.history_complete.insert(channel);
                    return;
                }
                app.resolve_named_users(&older);
                app.messages.update(cx, |list, cx| {
                    list.prepend_history(channel.clone(), older, cx)
                });
            })
        })
        .detach();
    }

    /// Live mode: page older stored history into `channel` until it reaches `since` (unix
    /// seconds), the beginning, or a page cap that bounds memory, for the date divider's
    /// "Jump to…"; then the list scrolls to its target.
    fn load_since(&mut self, channel: &ChannelId, since: i64, cx: &mut Context<Self>) {
        /// Pages one jump may load; past it the list stops at the oldest page read.
        const JUMP_PAGES: usize = 20;
        let generation = self.history_generation;
        let channel = channel.clone();
        cx.spawn(async move |this, cx| {
            for _ in 0..JUMP_PAGES {
                let read = this.update(cx, |app, cx| {
                    if app.history_generation != generation
                        || app.channel != channel
                        || app.history_complete.contains(&channel)
                    {
                        return None;
                    }
                    let before = app.messages.read(cx).oldest_loaded(cx)?;
                    app.bridge.history(channel.clone(), Some(before))
                });
                let Ok(Some(read)) = read else {
                    break;
                };
                let older = cx.background_executor().spawn(read).await;
                let reached = older
                    .first()
                    .and_then(|message| message.ts.secs())
                    .is_none_or(|secs| secs <= since);
                let landed = this.update(cx, |app, cx| {
                    if app.history_generation != generation || app.channel != channel {
                        return false;
                    }
                    if older.is_empty() {
                        app.history_complete.insert(channel.clone());
                    } else {
                        app.messages.update(cx, |list, cx| {
                            list.prepend_history(channel.clone(), older, cx)
                        });
                    }
                    true
                });
                if reached || !matches!(landed, Ok(true)) {
                    break;
                }
            }
            let _ = this.update(cx, |app, cx| {
                app.messages.update(cx, |list, cx| list.finish_jump(cx))
            });
        })
        .detach();
    }

    /// Live mode: show the open thread's stored history (root + replies, oldest first), read
    /// off the UI thread. A read that a newer one overtook is dropped; one whose thread is no
    /// longer open is ignored by the panel.
    fn load_thread(&mut self, channel: &ChannelId, thread_ts: &Ts, cx: &mut Context<Self>) {
        let Some(read) = self.bridge.thread(channel.clone(), thread_ts.clone()) else {
            return;
        };
        self.thread_generation += 1;
        let generation = self.thread_generation;
        let (channel, thread_ts) = (channel.clone(), thread_ts.clone());
        cx.spawn(async move |this, cx| {
            let messages = cx.background_executor().spawn(read).await;
            this.update(cx, |app, cx| {
                if app.thread_generation == generation {
                    app.resolve_named_users(&messages);
                    app.thread.update(cx, |panel, cx| {
                        panel.set_thread(channel, thread_ts, messages, cx)
                    });
                }
            })
        })
        .detach();
    }

    /// Ask the core to look up `users` the snapshot lacks: members of other organizations,
    /// whom `users.list` leaves out, so they show by name instead of id.
    fn resolve_users(&self, users: impl IntoIterator<Item = UserId>) {
        // Dedupe before sending: a page names the same few people many times. Which ids
        // really need `users.info` (the store holds no full record for them yet) the core
        // decides, so a boot-only record - a connect channel's external people, or guests -
        // still gets its full profile asked for.
        let named: Vec<UserId> = users
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if !named.is_empty() {
            self.bridge.send(Command::ResolveUsers(named));
        }
    }

    /// [`App::resolve_users`] for the people `messages` name: authors, repliers and
    /// `<@U…>` / `<@U…|name>` mentions.
    fn resolve_named_users(&self, messages: &[Message]) {
        self.resolve_users(messages.iter().flat_map(|message| {
            let mentions = message.text.split("<@").skip(1).filter_map(|rest| {
                let end = rest.find(['>', '|'])?;
                Some(UserId::from(&rest[..end]))
            });
            message
                .user
                .iter()
                .chain(&message.reply_users)
                .cloned()
                .chain(mentions)
        }));
    }

    /// Like Slack, viewing a conversation with unreads marks it read up to its newest message:
    /// Slack is told (`conversations.mark`) and the sidebar un-bolds it at once. Without a
    /// counts entry (the bundled sample workspace) the sidebar's own read state moves.
    fn mark_read(&mut self, channel: &ChannelId, newest: Ts, cx: &mut Context<Self>) {
        if let Some(counts) = self.counts.get_mut(channel) {
            if !counts.has_unreads && counts.mention_count == 0 {
                return;
            }
            counts.has_unreads = false;
            counts.mention_count = 0;
            counts.last_read = Some(newest.clone());
        }
        self.bridge.send(Command::MarkRead {
            channel: channel.clone(),
            ts: newest,
        });
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.mark_read(channel.clone(), cx));
    }

    /// Slack's Opt+Up/Down (any row) and Opt+Shift+Up/Down (unread rows): open the sidebar
    /// conversation above or below the open one.
    fn step_conversation(&mut self, forward: bool, unread_only: bool, cx: &mut Context<Self>) {
        let next = self
            .sidebar
            .read(cx)
            .step(&self.channel, forward, unread_only);
        if let Some(next) = next {
            self.select(next, cx);
        }
    }

    /// Esc: mark the open conversation read up to its newest message - or, while the All
    /// Unreads view is open, the group the keyboard focused.
    fn mark_open_read(&mut self, cx: &mut Context<Self>) {
        if self.unreads_open {
            self.unreads.update(cx, |view, cx| {
                view.handle_key("escape", cx);
            });
            return;
        }
        let channel = self.channel.clone();
        if let Some(newest) = self.messages.read(cx).newest_ts() {
            self.mark_read(&channel, newest, cx);
        }
    }

    /// Shift+Esc: ask first while Preferences > Mark as read prompts to confirm (on by
    /// default), then mark everything read.
    fn request_mark_all_read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.preferences.read(cx).settings().confirm_mark_all {
            return self.mark_all_read(cx);
        }
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            let this = this.clone();
            dialog
                .title("Mark all messages as read?")
                .w(px(440.))
                .child(
                    v_flex()
                        .gap_4()
                        .child(div().text_color(muted).child(
                            "Every channel, direct message and thread will be marked as read.",
                        ))
                        .child(
                            h_flex()
                                .gap_2()
                                .justify_end()
                                .child(
                                    Button::new("mark-all-cancel")
                                        .label("Cancel")
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    Button::new("mark-all-confirm")
                                        .primary()
                                        .label("Mark all as read")
                                        .on_click(move |_, window, cx| {
                                            window.close_dialog(cx);
                                            let _ =
                                                this.update(cx, |app, cx| app.mark_all_read(cx));
                                        }),
                                ),
                        ),
                )
        });
    }

    /// Cmd+= / Cmd+- step Preferences' zoom a level up or down (`None`, Cmd+0: 100%), which
    /// applies it.
    fn step_zoom(&mut self, step: Option<isize>, cx: &mut Context<Self>) {
        let level = self.preferences.read(cx).settings().zoom;
        let next = match step {
            Some(step) => level
                .saturating_add_signed(step)
                .min(crate::zoom::FACTORS.len() - 1),
            None => crate::zoom::ACTUAL_SIZE,
        };
        self.preferences
            .update(cx, |prefs, cx| prefs.set_zoom(next, cx));
    }

    /// Shift+Esc: Slack marks every conversation and thread read (`users.markAllRead`); the
    /// sidebar un-bolds at once and the core's fresh counts follow.
    fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        self.bridge.send(Command::MarkAllRead);
        for counts in self.counts.values_mut() {
            counts.has_unreads = false;
            counts.mention_count = 0;
        }
        let unread = self.sidebar.read(cx).unread_ids();
        self.sidebar.update(cx, |sidebar, cx| {
            for channel in unread {
                sidebar.mark_read(channel, cx);
            }
        });
        if self.unreads_open {
            self.refresh_unreads(cx);
        }
    }

    /// Someone started or kept typing: show them until [`TYPING_TTL`] after their last
    /// announcement, as Slack does. The user's own typing (from another client) is not shown.
    fn someone_typing(
        &mut self,
        channel: &ChannelId,
        user: &UserId,
        thread_ts: Option<&Ts>,
        cx: &mut Context<Self>,
    ) {
        if *user == self.fixtures.self_user {
            return;
        }
        let typists = self
            .typing
            .entry((channel.clone(), thread_ts.cloned()))
            .or_default();
        typists.retain(|(typist, _)| typist != user);
        typists.push((user.clone(), Instant::now() + TYPING_TTL));
        self.expire_typing(cx);
    }

    /// Drop typists whose announcement ran out, show who is left, and wake again when the
    /// next announcement runs out.
    fn expire_typing(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        for typists in self.typing.values_mut() {
            typists.retain(|(_, until)| *until > now);
        }
        self.typing.retain(|_, typists| !typists.is_empty());
        self.show_typing(cx);
        let next = self
            .typing
            .values()
            .flatten()
            .map(|(_, until)| *until)
            .min();
        self.typing_expiry = match next {
            Some(next) => cx.spawn(async move |this, cx| {
                let wait = next.saturating_duration_since(Instant::now());
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |app, cx| app.expire_typing(cx));
            }),
            None => gpui::Task::ready(()),
        };
    }

    /// Put who is typing under the open conversation's box and the open thread's reply box.
    fn show_typing(&self, cx: &mut Context<Self>) {
        let names = |key: (ChannelId, Option<Ts>)| {
            let names: Vec<SharedString> = (self.typing.get(&key).into_iter().flatten())
                .map(|(user, _)| self.fixtures.user_name(user).to_owned().into())
                .collect();
            TypingNames::of(&names)
        };
        let here = names((self.channel.clone(), None));
        self.composer
            .update(cx, |composer, cx| composer.set_typing(here, cx));
        let open = self.thread.read(cx).open_thread();
        let in_thread = open.and_then(|(channel, ts)| names((channel, Some(ts))));
        self.thread
            .update(cx, |thread, cx| thread.set_typing(in_thread, cx));
    }

    /// A message was just posted: its author stops typing there, and Slack's desktop
    /// notification rules decide whether it gets a banner and the notification sound.
    fn message_posted(
        &mut self,
        channel: &ChannelId,
        message: &Message,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(author) = &message.user {
            let thread = message.thread_ts.clone().filter(|ts| *ts != message.ts);
            if let Some(typists) = self.typing.get_mut(&(channel.clone(), thread)) {
                typists.retain(|(typist, _)| typist != author);
            }
            self.expire_typing(cx);
        }
        if !self.should_notify(channel, message, window, cx) {
            return;
        }
        let Some(conversation) = self.fixtures.conversation(channel) else {
            return;
        };
        let sender = match &message.user {
            Some(user) => self.fixtures.user_name(user).to_owned(),
            None => (message.bot_profile.as_ref())
                .map(|bot| bot.name.clone())
                .unwrap_or_default(),
        };
        let text = if self.preferences.read(cx).settings().show_preview {
            self.fixtures.plain_text(message)
        } else {
            "New message".to_owned()
        };
        let (title, body) = match conversation.kind() {
            ConversationKind::Im => (sender, text),
            ConversationKind::Mpim => (
                self.fixtures.conversation_label(conversation),
                format!("{sender}: {text}"),
            ),
            _ => (
                format!("#{}", conversation.name.clone().unwrap_or_default()),
                format!("{sender}: {text}"),
            ),
        };
        let tag = format!("msg:{channel}:{}", message.ts);
        self.system_notification(&tag, &title, &body, cx);
    }

    /// Slack's desktop notification rules for a message just posted: never your own, while
    /// notifications are paused or outside the notification schedule, or for the
    /// conversation (or thread) on screen in the focused window; never in a muted
    /// conversation; otherwise by the conversation's level (its bell menu choice, else
    /// Preferences > Notifications): every message, or direct messages, mentions and
    /// keywords. A thread reply that is neither a DM nor mentions you notifies only under
    /// "replies to threads I'm following", for a thread you started.
    fn should_notify(
        &self,
        channel: &ChannelId,
        message: &Message,
        window: &Window,
        cx: &gpui::App,
    ) -> bool {
        let me = &self.fixtures.self_user;
        let settings = self.preferences.read(cx).settings();
        if message.user.as_ref() == Some(me)
            || self.self_state.paused()
            || !settings.notifications_allowed_now()
        {
            return false;
        }
        let reply = message
            .thread_ts
            .as_ref()
            .is_some_and(|ts| *ts != message.ts);
        let on_screen = if reply {
            self.thread_open
                && self
                    .thread
                    .read(cx)
                    .open_thread()
                    .is_some_and(|(open, ts)| {
                        open == *channel && message.thread_ts.as_ref() == Some(&ts)
                    })
        } else {
            self.channel == *channel
        };
        if on_screen && window.is_window_active() {
            return false;
        }
        let level = self.fixtures.notify_levels.get(channel).copied();
        if level == Some(NotifyLevel::Muted) {
            return false;
        }
        let dm = self
            .fixtures
            .conversation(channel)
            .is_some_and(|conversation| {
                matches!(
                    conversation.kind(),
                    ConversationKind::Im | ConversationKind::Mpim
                )
            });
        let mentioned = mentions_me(&message.text, me, &settings.keywords);
        let wanted = match (level, settings.notify_level) {
            (Some(NotifyLevel::Mentions), _) | (_, 1) => dm || mentioned,
            (_, 0) => true,
            _ => false,
        };
        if reply && !dm && !mentioned {
            return wanted
                && settings.thread_replies
                && message.parent_user_id.as_ref() == Some(me);
        }
        wanted
    }

    /// Play the notification sound chosen in Preferences, unless sounds are muted.
    fn play_notification_sound(&mut self, cx: &mut Context<Self>) {
        let settings = self.preferences.read(cx).settings();
        let (muted, choice) = (settings.mute_sounds, settings.sound);
        if !muted {
            self.play_sound(choice, cx);
        }
    }

    /// Play Preferences' sound `choice` ("None" plays nothing): fetched from Slack's CDN
    /// through the core the first time, then kept for the next notifications.
    fn play_sound(&mut self, choice: usize, cx: &mut Context<Self>) {
        let Some(url) = crate::preferences::SOUND_FILES
            .get(choice)
            .copied()
            .flatten()
        else {
            return;
        };
        if let Some((cached, data)) = &self.sound {
            if *cached == choice {
                self.os.play_sound(data);
                return;
            }
        }
        let Some(fetch) = crate::media::fetch(url.to_owned(), cx) else {
            return;
        };
        self.sound_fetch = cx.spawn(async move |this, cx| match fetch.await {
            Ok(data) => {
                let _ = this.update(cx, |app, _| {
                    app.os.play_sound(&data);
                    app.sound = Some((choice, data));
                });
            }
            Err(e) => eprintln!("slackless: notification sound not loaded ({e})"),
        });
    }

    /// Put `views` on screen as the workspace views; returns the ones they replace.
    fn swap_views(&mut self, views: WorkspaceViews) -> WorkspaceViews {
        WorkspaceViews {
            sidebar: mem::replace(&mut self.sidebar, views.sidebar),
            messages: mem::replace(&mut self.messages, views.messages),
            thread: mem::replace(&mut self.thread, views.thread),
            results: mem::replace(&mut self.results, views.results),
            browser: mem::replace(&mut self.browser, views.browser),
            huddle: mem::replace(&mut self.huddle, views.huddle),
            profile: mem::replace(&mut self.profile, views.profile),
            switcher: mem::replace(&mut self.switcher, views.switcher),
        }
    }

    /// Show workspace `index`: park the current workspace's views and bring back the target's
    /// (built from its fixtures on the first visit). Drafts carry over through the store.
    fn switch_workspace(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == self.workspace || index >= self.parked.len() {
            return;
        }
        self.save_draft(cx);
        self.bridge
            .send(Command::CloseConversation(self.channel.clone()));
        let incoming = match self.parked[index].take() {
            Some(parked) => parked,
            None => {
                let fixtures = Arc::new(fixtures::workspace(index));
                let channel = first_conversation(&fixtures);
                let archive = fixture_archive(&fixtures);
                let views = build_workspace(&fixtures, index, archive, &channel, window, cx);
                let subscriptions = subscribe_workspace(&views, window, cx);
                Parked {
                    fixtures,
                    channel,
                    views,
                    subscriptions,
                }
            }
        };
        let outgoing = Parked {
            fixtures: mem::replace(&mut self.fixtures, incoming.fixtures),
            channel: mem::replace(&mut self.channel, incoming.channel),
            views: self.swap_views(incoming.views),
            subscriptions: mem::replace(&mut self.workspace_subscriptions, incoming.subscriptions),
        };
        self.parked[self.workspace] = Some(outgoing);
        self.workspace = index;
        self.open_members = None;
        self.members_hover = (false, false);
        self.channel_people = None;
        self.sync_connect_orgs(cx);
        self.load_join_people(cx);
        self.searching = false;
        self.drafts_open = false;
        self.later_open = false;
        self.threads_open = false;
        self.unreads_open = false;
        self.unreads_fetched.clear();
        self.huddles_open = false;
        self.directory_open = false;
        self.compose_open = false;
        self.pending_dm = None;
        self.close_thread(cx);
        self.profile_open = false;
        self.pins_open = false;
        let fixtures = self.fixtures.clone();
        self.later
            .update(cx, |later, cx| later.seed(index, &fixtures, cx));
        self.fold_search_menu(window, cx);
        self.search_menu
            .update(cx, |menu, cx| menu.set_fixtures(fixtures.clone(), cx));
        // The Files view serves every workspace: its rows follow the shown one.
        self.files_page
            .update(cx, |page, cx| page.set_fixtures(fixtures.clone(), cx));
        // Before the draft is loaded: its mentions are named from this workspace.
        self.composer
            .update(cx, |composer, cx| composer.set_fixtures(fixtures, cx));
        self.sync_saved(cx);
        self.sync_later_count(cx);
        self.huddle_open = false;
        self.tab = ConversationTab::Messages;
        self.bridge
            .send(Command::OpenConversation(self.channel.clone()));
        self.load_draft(cx);
        self.sync_composer_placeholder(cx);
        // New thread panels / profile panes pick up Enter-to-send and the self state.
        self.apply_preferences(window, cx);
        self.update_menus(cx);
    }

    /// Header label of a conversation in the shown workspace.
    fn label(&self, channel: &ChannelId) -> String {
        self.fixtures
            .conversation(channel)
            .map(|c| self.fixtures.conversation_label(c))
            .unwrap_or_else(|| channel.to_string())
    }

    /// Store the composer's text as the open conversation's draft (blank removes it).
    fn save_draft(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text();
        let saved = self
            .drafts
            .read(cx)
            .draft(self.workspace, &self.channel)
            .unwrap_or_default();
        if text == saved {
            return;
        }
        let entry = Entry {
            id: None,
            workspace: self.workspace,
            channel: self.channel.clone(),
            label: self.label(&self.channel),
            text,
            at: now_secs(),
        };
        self.drafts
            .update(cx, |drafts, cx| drafts.set_draft(entry, cx));
        let count = self.drafts.read(cx).draft_count();
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_draft_count(count, cx));
        self.persist_drafts(cx);
    }

    /// Persist the active workspace's unsent drafts into the local store off the UI thread.
    fn persist_drafts(&self, cx: &Context<Self>) {
        let saved: Vec<SavedDraft> = self
            .drafts
            .read(cx)
            .drafts_for(self.workspace)
            .into_iter()
            .map(|entry| SavedDraft {
                channel: entry.channel,
                label: entry.label,
                text: entry.text,
                at: entry.at,
            })
            .collect();
        if let Some(write) = self.bridge.remember_drafts(&saved) {
            cx.background_executor().spawn(write).detach();
        }
    }

    /// Put the open conversation's saved draft (or nothing) in the composer.
    fn load_draft(&mut self, cx: &mut Context<Self>) {
        let drafts = self.drafts.read(cx);
        let text = drafts
            .draft(self.workspace, &self.channel)
            .unwrap_or_default();
        let count = drafts.draft_count();
        self.composer
            .update(cx, |composer, cx| composer.load_draft(&text, cx));
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_draft_count(count, cx));
    }

    fn open_drafts(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.drafts_open = true;
        self.later_open = false;
        self.searching = false;
        if !self.fixtures.sample {
            self.bridge.send(Command::LoadScheduledMessages);
        }
        cx.notify();
    }

    /// Close every full-column view (a conversation or another view is taking the column).
    fn close_full_column_views(&mut self, cx: &mut Context<Self>) {
        self.searching = false;
        self.drafts_open = false;
        self.later_open = false;
        self.threads_open = false;
        self.unreads_open = false;
        self.huddles_open = false;
        self.directory_open = false;
        self.compose_open = false;
        // The Activity and Files pages are sidebar destinations, so leaving the full-column
        // views returns the rail to Home.
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.leave_pages(cx));
    }

    /// The Threads view: local cards at once, Slack's own list (`subscriptions.thread.getView`)
    /// when the fetch answers - and the local ones stay if it never does.
    fn open_threads(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        let cards = local_cards(&self.fixtures);
        let fixtures = self.fixtures.clone();
        self.threads.update(cx, |view, cx| {
            view.set_fixtures(fixtures, cx);
            view.show_local(cards, cx);
        });
        if let Some(store) = self.bridge.store().cloned() {
            let team = self.fixtures.workspaces[self.workspace].id.clone();
            let me = self.fixtures.self_user.clone();
            cx.spawn(async move |this, cx| {
                let cards = cx
                    .background_executor()
                    .spawn(async move {
                        let Ok(store) = store.lock() else {
                            return Vec::new();
                        };
                        store_cards(&store, &team, &me)
                    })
                    .await;
                this.update(cx, |app, cx| {
                    app.threads
                        .update(cx, |view, cx| view.show_store(cards, cx));
                })
            })
            .detach();
            self.bridge.send(Command::FetchThreads);
        }
        self.close_full_column_views(cx);
        self.threads_open = true;
        cx.notify();
    }

    /// The All Unreads view (Slack's `p-unreads_view`, Cmd+Shift+A): the sidebar's unread
    /// order as one card per conversation - its rows from the bundled workspace at once
    /// and, in live mode, from the local store off the UI thread.
    fn open_unreads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.close_full_column_views(cx);
        self.unreads_open = true;
        self.focus.focus(window, cx);
        self.refresh_unreads(cx);
        cx.notify();
    }

    /// Re-derive the open Unreads view's cards: the view opened, the workspace reloaded, a
    /// message landed, or everything was marked read.
    fn refresh_unreads(&mut self, cx: &mut Context<Self>) {
        let order = self.sidebar.read(cx).unread_ids();
        let groups = local_groups(&self.fixtures, &order);
        let fixtures = self.fixtures.clone();
        self.unreads
            .update(cx, |view, cx| view.show_local(fixtures, groups.clone(), cx));
        if let Some(store) = self.bridge.store().cloned() {
            let team = self.fixtures.workspaces[self.workspace].id.clone();
            let targets = store_targets(&self.fixtures, &groups);
            cx.spawn(async move |this, cx| {
                let groups = cx
                    .background_executor()
                    .spawn(async move {
                        let Ok(store) = store.lock() else {
                            return Vec::new();
                        };
                        store_groups(&store, &team, &targets)
                    })
                    .await;
                this.update(cx, |app, cx| {
                    // Only conversations Slack still counts unread take part: a mark may
                    // have raced the store read.
                    let keep = app.sidebar.read(cx).unread_ids();
                    for group in &groups {
                        if group.messages.is_empty()
                            && keep.contains(&group.channel)
                            && app.unreads_fetched.insert(group.channel.clone())
                        {
                            app.bridge
                                .send(Command::FetchLatestHistory(group.channel.clone()));
                        }
                    }
                    app.unreads
                        .update(cx, |view, cx| view.show_store(groups, &keep, cx));
                })
            })
            .detach();
        }
    }

    /// The Huddles page: the bundled sample huddle, or the stored `huddle_thread` trails live.
    fn open_huddles(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        let fixtures = self.fixtures.clone();
        self.huddles
            .update(cx, |view, cx| view.show_local(&fixtures, cx));
        if let Some(store) = self.bridge.store().cloned() {
            let team = self.fixtures.workspaces[self.workspace].id.clone();
            cx.spawn(async move |this, cx| {
                let huddles = cx
                    .background_executor()
                    .spawn(async move {
                        let Ok(store) = store.lock() else {
                            return Vec::new();
                        };
                        huddles::store_huddles(&store, &team)
                    })
                    .await;
                this.update(cx, |app, cx| {
                    app.huddles
                        .update(cx, |view, cx| view.set_huddles(huddles, cx));
                })
            })
            .detach();
        }
        self.close_full_column_views(cx);
        self.huddles_open = true;
        cx.notify();
    }

    /// The directory: the workspace's people and channels.
    fn open_directory(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        let fixtures = self.fixtures.clone();
        self.directory
            .update(cx, |view, cx| view.set_fixtures(fixtures, cx));
        self.close_full_column_views(cx);
        self.directory_open = true;
        cx.notify();
    }

    /// The "New message" view, focused on its To field.
    /// Slack's "New message" view, empty or with `draft` (Forward drafts the permalink).
    fn open_compose(&mut self, draft: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.save_draft(cx);
        let fixtures = self.fixtures.clone();
        self.compose
            .update(cx, |view, cx| view.set_fixtures(fixtures, cx));
        self.compose
            .update(cx, |view, cx| view.open(draft, window, cx));
        self.close_full_column_views(cx);
        self.compose_open = true;
        cx.notify();
    }

    /// The gear menu's "Create a channel section": Slack-side in live mode (the section
    /// arrives with the next sidebar reload), local in the bundled sample workspace.
    fn create_section(&mut self, name: String, cx: &mut Context<Self>) {
        if self.fixtures.sample {
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.add_section(name, cx));
        } else {
            self.bridge.send(Command::CreateSection { name });
        }
    }

    fn open_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.open_dialog(cx, |dialog, _, cx| {
            dialog
                .title("Keyboard shortcuts")
                .w(px(860.))
                .child(shortcuts_view(cx))
        });
    }

    fn show_nav(&mut self, item: NavItem, cx: &mut Context<Self>) {
        match item {
            NavItem::Activity => self.open_activity(cx),
            NavItem::Files => self.open_files(cx),
            _ => {
                self.close_full_column_views(cx);
                self.sidebar
                    .update(cx, |sidebar, cx| sidebar.show_nav(item, cx));
                cx.notify();
            }
        }
    }

    /// The Activity page (the rail's Activity destination): a full-width page right of the
    /// rail - the sidebar hides its list column while it is active. Slack's own feed
    /// (`activity.feed`) lands with the next [`Event::ActivityView`]; the fixture workspace
    /// serves its derived rows.
    fn open_activity(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.close_full_column_views(cx);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.show_nav(NavItem::Activity, cx));
        self.activity_open = true;
        let fixtures = self.fixtures.clone();
        self.activity
            .update(cx, |view, cx| view.set_fixtures(fixtures, cx));
        self.fetch_activity(cx);
        cx.notify();
    }

    /// Ask the core for the Activity feed (live mode; fixture mode keeps its derived rows).
    fn fetch_activity(&mut self, cx: &mut Context<Self>) {
        if self.bridge.store().is_some() {
            self.activity.update(cx, |view, cx| view.start_loading(cx));
            self.bridge
                .send(Command::FetchActivity { with_bodies: true });
        }
    }

    /// The Files view (the rail's Files destination): the sidebar shows its Files list
    /// column and the page takes the main area. Live mode refetches the shown page's rows
    /// (`files.list`, answered by [`Event::FilesLoaded`]); the sample workspace derives them
    /// from its messages.
    fn open_files(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.close_full_column_views(cx);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.show_nav(NavItem::Files, cx));
        let fixtures = self.fixtures.clone();
        self.files_page
            .update(cx, |page, cx| page.open(fixtures, cx));
        cx.notify();
    }

    /// Queue the composer's draft for later delivery in the open conversation.
    fn schedule(
        &mut self,
        text: String,
        when: SendLater,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let post_at = when.at(now_secs());
        if !self.fixtures.sample {
            let content = composer::content_of(&text, &self.fixtures);
            self.bridge.send(Command::ScheduleMessage {
                channel: self.channel.clone(),
                post_at,
                content,
            });
        }
        let entry = Entry {
            id: None,
            workspace: self.workspace,
            channel: self.channel.clone(),
            label: self.label(&self.channel),
            text,
            at: post_at,
        };
        self.drafts
            .update(cx, |drafts, cx| drafts.schedule(entry, cx));
        self.save_draft(cx);
        notify(
            "Message scheduled",
            format!("Sends {}. See Drafts & sent.", when.label().to_lowercase()),
            window,
            cx,
        );
    }

    /// Deliver a scheduled message now; it shows in the transcript when its workspace is the
    /// shown one (a parked workspace's transcript is not touched).
    fn deliver(&mut self, mut entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = entry.id.take() {
            self.bridge.send(Command::DeleteScheduledMessage {
                channel: entry.channel.clone(),
                scheduled_message_id: id,
            });
        }
        // A scheduled message is kept as its draft text; it posts as the draft would have.
        let content = composer::content_of(&entry.text, &self.fixtures);
        if entry.workspace == self.workspace {
            let mut message =
                self.fixtures
                    .local_message(&entry.channel, content.text.clone(), None);
            message.blocks = content.blocks.clone();
            let channel = entry.channel.clone();
            self.messages
                .update(cx, |list, cx| list.append(channel, message, cx));
        }
        self.bridge.send(Command::SendMessage {
            channel: entry.channel.clone(),
            content,
            thread_ts: None,
            broadcast: false,
        });
        notify(
            "Message sent",
            format!("Delivered to {}", entry.label),
            window,
            cx,
        );
        entry.at = now_secs();
        self.drafts
            .update(cx, |drafts, cx| drafts.record_sent(entry, cx));
    }

    /// Dispatch a slash command typed in the main composer, or in the reply composer of
    /// `thread`. Live mode hands `/giphy` and commands slackless does not know to Slack
    /// (`chat.command`; the result arrives on the websocket). Fixture mode runs the ones that
    /// only touch local state (/shrug, /away, /status) and logs the rest with a toast.
    fn run_slash(
        &mut self,
        invocation: &Invocation,
        thread: Option<(ChannelId, Ts)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let remote = invocation
            .command
            .is_none_or(|command| command.kind == SlashKind::Giphy);
        if remote && !self.fixtures.sample {
            let (channel, thread_ts) = match thread {
                Some((channel, thread_ts)) => (channel, Some(thread_ts)),
                None => (self.channel.clone(), None),
            };
            self.bridge.send(Command::RunCommand {
                channel,
                command: format!("/{}", invocation.name),
                args: invocation.args.clone(),
                thread_ts,
            });
            return;
        }
        let Some(command) = invocation.command else {
            notify(
                "Unknown command",
                format!("/{} is not a valid command.", invocation.name),
                window,
                cx,
            );
            return;
        };
        match command.kind {
            SlashKind::Shrug => {
                let text = if invocation.args.is_empty() {
                    SHRUG.to_owned()
                } else {
                    format!("{} {SHRUG}", invocation.args)
                };
                self.send(MessageContent::plain(text), window, cx);
            }
            SlashKind::Away => {
                self.self_state.away = !self.self_state.away;
                self.push_self_state(cx);
                let state = if self.self_state.away {
                    "away"
                } else {
                    "active"
                };
                notify("/away", format!("You are now {state}."), window, cx);
            }
            SlashKind::Status if invocation.args.eq_ignore_ascii_case("clear") => {
                self.self_state.status = None;
                self.push_self_state(cx);
                notify("/status", "Your status was cleared.", window, cx);
            }
            SlashKind::Status => self.open_status_picker(window, cx),
            kind => {
                eprintln!(
                    "slackless: slash command {kind:?} /{} args={:?} in {} (fixture mode: not run)",
                    command.name, invocation.args, self.channel
                );
                notify(
                    &format!("/{}", command.name),
                    "Logged. This command runs against a live workspace.",
                    window,
                    cx,
                );
            }
        }
    }

    fn huddle_here(&self) -> bool {
        self.huddle
            .as_ref()
            .is_some_and(|(channel, _)| *channel == self.channel)
    }

    /// The channel browser in a gpui-kit dialog (gpui-kit-guide section 7).
    fn open_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let browser = self.browser.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Browse channels")
                .w(px(640.))
                .child(browser.clone())
        });
    }

    /// Slack's broadcast confirmation (`F-CORE-083`): warn before posting `@channel`, `@here`,
    /// or `@everyone` to a channel with 6 or more members.
    fn send(&mut self, content: MessageContent, window: &mut Window, cx: &mut Context<Self>) {
        const BROADCAST_WARN_THRESHOLD: usize = 6;
        if let Some(range) = broadcast_kind(&content) {
            let conv = self.fixtures.conversation(&self.channel);
            let is_channel = conv.is_some_and(|c| {
                matches!(
                    c.kind(),
                    ConversationKind::PublicChannel | ConversationKind::PrivateChannel
                )
            });
            let members = self
                .member_counts
                .get(&self.channel)
                .map(|&n| n as usize)
                .or_else(|| conv.and_then(|c| c.num_members.map(|n| n as usize)))
                .or_else(|| conv.map(|c| c.members.len()))
                .unwrap_or(0);
            if is_channel && members >= BROADCAST_WARN_THRESHOLD {
                let channel_name = conv
                    .and_then(|c| c.name.clone())
                    .unwrap_or_else(|| self.channel.to_string());
                let title = match range {
                    BroadcastRange::Here => format!("Notify active members in #{channel_name}?"),
                    _ => format!("Notify everyone in #{channel_name}?"),
                };
                let body = format!(
                    "By using @{range}, you\u{2019}re about to notify {members} people in #{channel_name}. Are you sure you want to continue?"
                );
                let this = cx.weak_entity();
                let send_content = content.clone();
                let cancel_text = content.text.clone();
                window.open_dialog(cx, move |dialog, _, cx| {
                    let muted = cx.theme().muted_foreground;
                    let this_cancel = this.clone();
                    let this_send = this.clone();
                    let send_content = send_content.clone();
                    let cancel_text = cancel_text.clone();
                    dialog.title(title.clone()).w(px(460.)).child(
                        v_flex()
                            .gap_4()
                            .child(div().text_color(muted).child(body.clone()))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_end()
                                    .child(
                                        Button::new("broadcast-cancel").label("Cancel").on_click(
                                            move |_, window, cx| {
                                                window.close_dialog(cx);
                                                let _ = this_cancel.update(cx, |app, cx| {
                                                    app.composer.update(cx, |composer, cx| {
                                                        composer.load_draft(&cancel_text, cx);
                                                    });
                                                });
                                            },
                                        ),
                                    )
                                    .child(
                                        Button::new("broadcast-send")
                                            .primary()
                                            .label("Send")
                                            .on_click(move |_, window, cx| {
                                                window.close_dialog(cx);
                                                let _ = this_send.update(cx, |app, cx| {
                                                    app.deliver_send(send_content.clone(), cx);
                                                });
                                            }),
                                    ),
                            ),
                    )
                });
                return;
            }
        }
        self.deliver_send(content, cx);
    }

    fn deliver_send(&mut self, content: MessageContent, cx: &mut Context<Self>) {
        let channel = self.channel.clone();
        // The local echo shows the same blocks the message posts with.
        let mut message = self
            .fixtures
            .local_message(&channel, content.text.clone(), None);
        message.blocks = content.blocks.clone();
        self.messages
            .update(cx, |list, cx| list.append(channel.clone(), message, cx));
        let entry = Entry {
            id: None,
            workspace: self.workspace,
            channel: channel.clone(),
            label: self.label(&channel),
            text: content.text.clone(),
            at: now_secs(),
        };
        self.drafts
            .update(cx, |drafts, cx| drafts.record_sent(entry, cx));
        self.bridge.send(Command::SendMessage {
            channel,
            content,
            thread_ts: None,
            broadcast: false,
        });
        // The composer is empty now: drop any saved draft for this conversation.
        self.save_draft(cx);
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
                .child(header);
            let column = match (&self.huddle, self.huddle_open) {
                (Some((_, bar)), true) => column.child(bar.clone()),
                _ => column,
            };
            let column = column.child(tabs).children(bookmark_bar);
            match &self.tab {
                // The Pins tab keeps the transcript under it; its panel is the right column.
                ConversationTab::Messages | ConversationTab::Pins => {
                    let column =
                        column.child(div().flex_1().min_h_0().child(self.messages.clone()));
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
                    column.child(div().flex_1().min_h_0().child(canvas_view(cx)))
                }
                ConversationTab::List => {
                    column.child(div().flex_1().min_h_0().child(list_view(cx)))
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
                        self.files.get(&self.channel)
                    };
                    let view = tab_views::files_view(&self.fixtures, files, cx);
                    column.child(div().flex_1().min_h_0().child(view))
                }
                ConversationTab::Folder { bookmark_id, .. } => {
                    let bookmarks = self.bookmarks.get(&self.channel);
                    let view = tab_views::folder_view(bookmark_id, bookmarks, cx);
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
            panels = panels.child(
                resizable_panel()
                    .size(sidebar_width)
                    .size_range(px(288.)..px(728.))
                    .fixed()
                    .child(self.sidebar.clone()),
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
                self.pins.get(&self.channel)
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
            .on_action(cx.listener(|this, _: &OpenDownloads, _, cx| this.open_downloads(cx)))
            .on_action(
                cx.listener(|this, _: &SetStatus, window, cx| this.open_status_picker(window, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.search.read(cx).focus_handle(cx).focus(window, cx);
                this.open_search_menu(cx);
            }))
            // Only with somewhere to go, so the menu bar greys out Back / Forward as Slack's does.
            .when(!self.back.is_empty(), |root| {
                root.on_action(cx.listener(|this, _: &NavigateBack, _, cx| this.go_back(cx)))
            })
            .when(!self.forward.is_empty(), |root| {
                root.on_action(cx.listener(|this, _: &NavigateForward, _, cx| this.go_forward(cx)))
            })
            .on_action(cx.listener(|this, switch: &SwitchWorkspace, window, cx| {
                this.switch_workspace(switch.0, window, cx)
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
            .relative()
            .size_full()
            .bg(chrome)
            .text_color(foreground)
            .child(div().absolute().top_0().left_0().size(glow_side).bg(glow))
            .child(title_bar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(switcher)
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
                            .child(div().id("remind-menu-overlay").occlude().child(menu)),
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

impl App {
    /// The global bar (gpui-kit `TitleBar`): history navigation on the left, the wide
    /// centered search pill, and the workspace chip, theme toggle and help on the right —
    /// the reference screenshot's top row.
    fn global_bar(&mut self, cx: &mut Context<Self>) -> TitleBar {
        let muted_foreground = cx.theme().muted_foreground;
        // Slack's pill is a translucent shade of the chrome, not a bordered field.
        let pill = cx.theme().foreground.opacity(0.08);
        let accent = cx.theme().accent;

        let this = cx.entity().downgrade();
        let recent = self.recent_entries();
        let nav = h_flex()
            .items_center()
            .gap_1()
            .child(
                Button::new("nav-back")
                    .ghost()
                    .with_size(Size::Small)
                    .icon(IconName::ArrowLeft)
                    .tooltip("Back")
                    .disabled(self.back.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.go_back(cx))),
            )
            .child(
                Button::new("nav-forward")
                    .ghost()
                    .with_size(Size::Small)
                    .icon(IconName::ArrowRight)
                    .tooltip("Forward")
                    .disabled(self.forward.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.go_forward(cx))),
            )
            .child(
                Button::new("nav-history")
                    .ghost()
                    .with_size(Size::Small)
                    .icon(IconName::Clock)
                    .tooltip("History")
                    .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                        menu = menu.label("Recent");
                        for (id, label, icon) in &recent {
                            let (this, id) = (this.clone(), id.clone());
                            menu =
                                menu.item(PopupMenuItem::new(label.clone()).icon(*icon).on_click(
                                    move |_, _, cx| {
                                        let _ =
                                            this.update(cx, |app, cx| app.select(id.clone(), cx));
                                    },
                                ));
                        }
                        menu
                    }),
            );

        // The dropdown hangs under the pill: capture its window bounds as it paints
        // (stored only, never notified — the panel's rows are computed on open and query
        // change, never per frame).
        let menu = self.search_menu.downgrade();
        let search_pill = div()
            .w(px(820.))
            .min_w(px(200.))
            .on_children_prepainted(move |children, _, cx| {
                if let Some(bounds) = children.first() {
                    let _ = menu.update(cx, |menu, _| menu.set_pill_bounds(*bounds));
                }
            })
            .child(
                h_flex()
                    .w_full()
                    .h(px(26.))
                    .items_center()
                    .gap_2()
                    .px_2()
                    .rounded(px(6.))
                    .bg(pill)
                    // Like Slack, a click anywhere on the pill focuses search (and never
                    // drags the window, as the rest of the title bar does) and opens the
                    // dropdown under it.
                    .cursor_text()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            window.prevent_default();
                            this.search.read(cx).focus_handle(cx).focus(window, cx);
                            this.open_search_menu(cx);
                        }),
                    )
                    .child(
                        Icon::new(IconName::Search)
                            .text_color(muted_foreground)
                            .size(px(14.)),
                    )
                    .child(
                        // While the dropdown is open the field renders inside the panel;
                        // the pill keeps its box so the bar does not shift.
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .when(!self.search_menu_open, |field| {
                                field.child(
                                    Textarea::new(&self.search)
                                        .appearance(false)
                                        .bordered(false),
                                )
                            }),
                    ),
            );

        // Slack's top-right status button: the status emoji (a plain face while none is set)
        // and a chevron, opening the status picker. The avatar lives at the rail's foot.
        let status_glyph = self
            .self_state
            .status
            .as_ref()
            .map_or("\u{1F642}", |status| status.glyph());
        let status_button = h_flex()
            .id("status-button")
            .flex_none()
            .items_center()
            .gap_0p5()
            .px_1()
            .h(px(28.))
            .rounded_sm()
            .cursor_pointer()
            .hover(|el| el.bg(accent))
            .child(div().text_base().child(status_glyph))
            .child(
                Icon::new(IconName::ChevronDown)
                    .text_color(muted_foreground)
                    .size(px(12.)),
            )
            .on_click(cx.listener(|this, _, window, cx| this.open_status_picker(window, cx)));

        // Slack's 40px top bar: the history arrows hug the left of a search pill centred in
        // the window (the right padding mirrors the traffic lights' inset on the left), with
        // status and help pinned to the right edge.
        // Transparent over the root's chrome (and its corner glow), like the rail, and without
        // the kit's bottom rule: Slack's chrome runs unbroken into the workspace column.
        TitleBar::new()
            .h(px(40.))
            .bg(gpui_kit::gpui::transparent_black())
            .border_b_0()
            .child(
                h_flex()
                    .relative()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .pr(px(80.))
                    .child(h_flex().flex_1().min_w_0().justify_end().pr_2().child(nav))
                    .child(search_pill)
                    .child(div().flex_1().min_w_0())
                    .child(
                        h_flex()
                            .absolute()
                            .right_2()
                            .top_0()
                            .h_full()
                            .items_center()
                            .gap_1()
                            .child(status_button)
                            .child(
                                chrome_icon(
                                    "help",
                                    IconName::CircleQuestionMark,
                                    muted_foreground,
                                    accent,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| this.open_shortcuts(window, cx),
                                )),
                            ),
                    ),
            )
    }

    /// Offline / reconnecting notice at the top of the conversation column while Slack's RTM
    /// socket is dropped (`F-SHELL-169`). Hidden once connected or in fixture mode.
    fn connection_banner(&self, cx: &Context<Self>) -> Option<Div> {
        use slackless_core::rtm::ConnectionState;
        let text = match self.connection.as_ref()? {
            ConnectionState::Connected { .. } | ConnectionState::Connecting => return None,
            ConnectionState::Reconnecting { attempt, delay, .. } => {
                let secs = delay.as_secs();
                if secs > 0 {
                    format!("Offline — reconnecting to Slack in {secs}s (attempt {attempt})…")
                } else {
                    format!("Offline — reconnecting to Slack (attempt {attempt})…")
                }
            }
            ConnectionState::Disconnected { reason } => {
                format!("Disconnected from Slack ({reason})")
            }
        };
        let theme = cx.theme();
        Some(
            h_flex()
                .flex_none()
                .items_center()
                .justify_center()
                .gap_2()
                .px_4()
                .py_1p5()
                .bg(theme.warning.opacity(0.18))
                .border_b_1()
                .border_color(theme.warning.opacity(0.35))
                .child(
                    Icon::new(IconName::WifiOff)
                        .size(px(14.))
                        .text_color(theme.warning),
                )
                .child(div().text_xs().text_color(theme.foreground).child(text)),
        )
    }

    /// Slack's paused-notifications strip for a DM whose peer has do-not-disturb on: a
    /// muted row between the message list and the composer.
    fn dnd_banner(&self, conversation: Option<&Conversation>, cx: &Context<Self>) -> Option<Div> {
        let peer = conversation
            .filter(|c| c.kind() == ConversationKind::Im)
            .and_then(|c| c.user.as_ref())?;
        if !self.peer_dnd_paused.get(peer).copied().unwrap_or(false) {
            return None;
        }
        let muted = cx.theme().muted;
        let muted_foreground = cx.theme().muted_foreground;
        // Slack's notice: a rounded box aligned with the composer, worded for the reader.
        Some(
            h_flex()
                .items_center()
                .gap_2()
                .mx_4()
                .mb_1()
                .px_3()
                .py_1p5()
                .flex_none()
                .rounded_md()
                .bg(muted)
                .child(
                    Icon::new(IconName::BellOff)
                        .size(px(14.))
                        .text_color(muted_foreground),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child("They will see your messages when they're back."),
                ),
        )
    }

    /// Slack's context bar for an externally shared channel, drawn inside the composer's
    /// frame: the first organizations' icons stacked, then "<N> external people are from
    /// <Org>" (two organizations by name, more as a count), the count linking to the channel's
    /// details. Built when the members or the snapshot change, never per frame.
    fn connect_bar(&self, cx: &Context<Self>) -> Option<ContextBar> {
        let (teams, count, rest) = self.connect_line()?;
        let (app, channel) = (cx.entity().downgrade(), self.channel.clone());
        Some(Rc::new(move |_, cx| {
            let (bg, fg, link) = (
                crate::theme::connect_bar(cx),
                cx.theme().foreground,
                cx.theme().link,
            );
            let (app, channel) = (app.clone(), channel.clone());
            h_flex()
                .items_center()
                .gap_2()
                .px_3()
                // Slack's bar shows about 38pt of amber; the composer box covers the rest.
                .pt(px(10.))
                .pb(px(10. + composer::BAR_TUCK))
                .rounded_tl(px(8.))
                .rounded_tr(px(8.))
                .bg(bg)
                .text_size(px(13.))
                .text_color(fg)
                .child(org_stack(&teams, bg))
                .child(
                    h_flex()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .id("connect-count")
                                .flex_none()
                                .font_weight(FontWeight::BOLD)
                                .text_color(link)
                                .cursor_pointer()
                                .hover(|count| count.underline())
                                .child(count.clone())
                                .on_click(move |_, window, cx| {
                                    let _ = app.update(cx, |app, cx| {
                                        app.open_channel_details(
                                            &channel,
                                            channel_details::DetailsTab::About,
                                            window,
                                            cx,
                                        )
                                    });
                                }),
                        )
                        .child(div().min_w_0().truncate().child(rest.clone())),
                )
                .into_any_element()
        }))
    }

    /// Slack's Slack Connect line for the open conversation: up to three of the other
    /// organizations, then "<N> external people" and "are from <Org>" (two organizations by
    /// name, more as a count). `None` for a channel only this workspace is in.
    fn connect_line(&self) -> Option<(Vec<Team>, String, String)> {
        let conversation = self.fixtures.conversation(&self.channel)?;
        if !conversation.is_ext_shared {
            return None;
        }
        let mut teams: Vec<Team> = self
            .fixtures
            .shared_teams(conversation)
            .into_iter()
            .cloned()
            .collect();
        if teams.is_empty() {
            teams = self
                .connect_orgs
                .iter()
                .map(|org| org.team.clone())
                .collect();
        }
        if teams.is_empty() {
            return None;
        }
        let people: u32 = self.connect_orgs.iter().map(|org| org.members).sum();
        let (count, rest) = if people == 0 {
            (
                format!("{} external organizations", teams.len()),
                "can view this channel".to_owned(),
            )
        } else {
            let from = match teams.as_slice() {
                [team] => team.name.to_string(),
                [a, b] => format!("{} and {}", a.name, b.name),
                _ => format!("{} organizations", teams.len()),
            };
            (
                format!(
                    "{people} external {}",
                    if people == 1 { "person" } else { "people" }
                ),
                format!("{} from {from}", if people == 1 { "is" } else { "are" }),
            )
        };
        teams.truncate(3);
        Some((teams, count, rest))
    }

    /// Recompute [`Self::connect_orgs`] for the open conversation: only a Slack Connect
    /// channel has other organizations to count.
    fn sync_connect_orgs(&mut self, cx: &mut Context<Self>) {
        self.connect_orgs = match self.fixtures.conversation(&self.channel) {
            Some(conversation) if conversation.is_ext_shared => channel_details::external_orgs(
                &channel_details::own_team(&self.fixtures),
                self.member_ids(&self.channel, conversation),
                &self.fixtures.users,
                &self.fixtures.teams,
            ),
            _ => Vec::new(),
        };
        let bar = self.connect_bar(cx);
        self.composer
            .update(cx, |composer, cx| composer.set_context_bar(bar, cx));
        self.sync_thread_bar(cx);
    }

    /// Slack shows the channel's Slack Connect bar over the thread's reply box too.
    fn sync_thread_bar(&mut self, cx: &mut Context<Self>) {
        let here = self
            .thread
            .read(cx)
            .open_thread()
            .is_some_and(|(channel, _)| channel == self.channel);
        let bar = here.then(|| self.connect_bar(cx)).flatten();
        self.thread
            .update(cx, |panel, cx| panel.set_context_bar(bar, cx));
    }

    /// The freshest member ids of `channel`: the core's announcement while it is the open
    /// channel, else the conversation row (the sample workspace's lists).
    fn member_ids<'a>(
        &'a self,
        channel: &ChannelId,
        conversation: &'a Conversation,
    ) -> &'a [UserId] {
        match &self.open_members {
            Some((open, members)) if open == channel => members,
            _ => &conversation.members,
        }
    }

    /// Slack's dark tooltip hanging under the header members chip (`.c-tooltip__tip`: the
    /// inverse surface, 200px wide, 13px text on a 1.25 line, 8x12 padding, radius 8, the
    /// arrow filling the 8px gap up to the chip).
    fn members_popover(&self, cx: &Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (surface, ink) = (theme.foreground, theme.background);
        div()
            .id("hdr-members-popover")
            .absolute()
            .top(relative(1.))
            .left_1_2()
            .ml(px(-100.))
            .pt(px(8.))
            .occlude()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.members_hover.1 = *hovered;
                cx.notify();
            }))
            .child(
                v_flex()
                    .items_center()
                    .child(crate::sidebar::tooltip_arrow(surface, true))
                    .child(
                        v_flex()
                            .w(px(200.))
                            .px_3()
                            .py_2()
                            .rounded(px(8.))
                            .bg(surface)
                            .text_size(px(13.))
                            .line_height(relative(1.25))
                            .text_center()
                            .text_color(ink)
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .child("View all members of this channel"),
                            )
                            .children(
                                includes_line(&self.people_names())
                                    .map(|line| div().text_color(ink.opacity(0.65)).child(line)),
                            ),
                    ),
            )
    }

    /// The open channel's people ([`Self::channel_people`]): fixture mode reads the snapshot's
    /// loaded history in hand; live mode reads the same local-store history the transcript
    /// opened with, off the UI thread. Cached until the channel or its history changes -
    /// never per frame.
    fn load_channel_people(&mut self, cx: &mut Context<Self>) {
        if self.channel_people.is_some() {
            return;
        }
        let channel = self.channel.clone();
        if self.bridge.store().is_some() {
            let Some(read) = self.bridge.history(channel.clone(), None) else {
                return;
            };
            cx.spawn(async move |this, cx| {
                let history = cx.background_executor().spawn(read).await;
                this.update(cx, |app, cx| {
                    if app.channel == channel {
                        app.channel_people = Some(app.people(&channel, &history));
                        if app.members_hover.0 || app.members_hover.1 {
                            cx.notify();
                        }
                    }
                })
            })
            .detach();
        } else {
            let history = self
                .fixtures
                .messages
                .get(&channel)
                .cloned()
                .unwrap_or_default();
            self.channel_people = Some(self.people(&channel, &history));
            cx.notify();
        }
    }

    /// `channel`'s people from its loaded history: the most recent distinct human authors,
    /// newest first, topped up from the member list.
    fn people(&self, channel: &ChannelId, history: &[Message]) -> Vec<UserId> {
        let conversation = self.fixtures.conversation(channel);
        channel_people(
            &self.fixtures,
            self.member_ids(channel, conversation.unwrap_or(&Conversation::default())),
            history,
        )
    }

    /// The names of [`Self::channel_people`], as the members popover lists them.
    fn people_names(&self) -> Vec<String> {
        self.channel_people
            .iter()
            .flatten()
            .map(|user| self.fixtures.user_name(user).to_owned())
            .collect()
    }

    /// Load the open channel's people when it shows the Join card, which names them.
    fn load_join_people(&mut self, cx: &mut Context<Self>) {
        if self
            .fixtures
            .conversation(&self.channel)
            .is_some_and(joinable)
        {
            self.load_channel_people(cx);
        }
    }

    /// Slack's card in place of the composer in a public channel the user is not in: the
    /// faces of up to three of its people, "Join #name", who is here (two names and how many
    /// more people, then its apps), a shared channel's Slack Connect line, and the green Join
    /// Channel button.
    fn join_card(&self, conversation: &Conversation, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (surface, _) = crate::theme::composer_surfaces(cx);
        let (foreground, muted) = (theme.foreground, theme.muted_foreground);
        let people = self.channel_people.as_deref().unwrap_or(&[]);
        // Each face is ringed in the card's color, overlapping the one before by 8px.
        let faces = h_flex().children(people.iter().take(3).enumerate().map(|(ix, user)| {
            let face = crate::avatars::with_external_badge(
                &self.fixtures,
                Some(user),
                crate::avatars::user_chip(&self.fixtures, user, 36.),
                36.,
                surface,
            );
            div()
                .when(ix > 0, |face| face.ml(px(-12.)))
                .p(px(2.))
                .rounded(px(10.))
                .bg(surface)
                .child(face)
        }));
        let names: Vec<&str> = people
            .iter()
            .take(2)
            .map(|user| self.fixtures.user_name(user))
            .collect();
        let humans = self
            .member_counts
            .get(&conversation.id)
            .copied()
            .or(conversation.num_members)
            .map_or(names.len(), |count| count as usize);
        let apps: Vec<&str> = self
            .member_ids(&conversation.id, conversation)
            .iter()
            .filter_map(|member| self.fixtures.user(member))
            // Slack names the channel's apps, not its workflows' bots, alphabetically.
            .filter(|user| {
                let workflow = user.extra.get("is_workflow_bot").and_then(|v| v.as_bool());
                user.is_bot && !user.deleted && workflow != Some(true)
            })
            .map(|user| self.fixtures.name_of(user))
            .collect();
        let mut apps = apps;
        apps.sort_by_key(|name| name.to_lowercase());
        let who = [
            who_is_here(&names, humans.saturating_sub(names.len())),
            apps_also_here(&apps),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
        let external = self.connect_line().map(|(teams, count, rest)| {
            h_flex()
                .items_center()
                .gap_2()
                .child(org_stack(&teams, surface))
                .child(
                    h_flex()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::BOLD)
                                .text_color(foreground)
                                .child(count),
                        )
                        .child(div().min_w_0().truncate().child(rest)),
                )
        });
        let name = conversation.name.clone().unwrap_or_default();
        div().w_full().px_4().pb_3().child(
            h_flex()
                .w_full()
                .items_start()
                .justify_between()
                .gap_4()
                .p_4()
                .rounded_lg()
                .border_1()
                .border_color(foreground.opacity(0.15))
                .bg(surface)
                .child(
                    v_flex()
                        .min_w_0()
                        .gap_1()
                        .text_sm()
                        .text_color(muted)
                        // The faces' 2px rings sit in the card's padding, as Slack's do.
                        .child(faces.ml(px(-2.)).mt(px(-2.)))
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(foreground)
                                .child(format!("Join # {name}")),
                        )
                        .when(!who.is_empty(), |column| column.child(who))
                        .children(external),
                )
                .child(
                    Button::new("join-channel")
                        .primary()
                        .with_size(gpui_kit::component::Size::Large)
                        .label("Join Channel")
                        .on_click(cx.listener(|app, _, window, cx| app.join_channel(window, cx))),
                ),
        )
    }

    /// The Join card's button: join the open channel on Slack; the card gives way to the
    /// composer once the core stores it as joined. Fixture mode logs it.
    fn join_channel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fixtures.sample {
            notify(
                "Join Channel",
                "Logged. Joining runs against a live workspace.",
                window,
                cx,
            );
            return;
        }
        self.bridge
            .send(Command::JoinConversation(self.channel.clone()));
    }

    /// The conversation header row: star, peer avatar for DMs (presence dot for IMs, stacked
    /// chips for group DMs), the conversation name, the member count for channels, and the
    /// huddle / notifications / search / details icons on the right.
    fn conversation_header(
        &self,
        title: &str,
        topic: &str,
        conversation: Option<&Conversation>,
        cx: &mut Context<Self>,
    ) -> Div {
        let muted_foreground = cx.theme().muted_foreground;
        let accent = cx.theme().accent;
        let background = cx.theme().background;
        let success = cx.theme().success;
        let huddle_here = self.huddle_here();

        // Channels: the label already carries `# name`, which Slack keeps beside the channel
        // icon tile; a private channel's lock replaces the hash.
        let channel = conversation.filter(|c| {
            matches!(
                c.kind(),
                ConversationKind::PublicChannel | ConversationKind::PrivateChannel
            )
        });
        let section_emoji = channel.and_then(|c| {
            self.fixtures
                .sections
                .iter()
                .find(|section| !section.emoji.is_empty() && section.channel_ids.contains(&c.id))
                .map(|section| section.emoji.as_str())
                .filter(|emoji| {
                    crate::emoji::display_glyph(emoji).is_some()
                        || self.fixtures.custom_emoji(emoji).is_some()
                })
        });
        // Slack's tile beside the name shows the emoji of the channel's sidebar section.
        let channel_tile = section_emoji.map(|emoji| {
            let glyph = crate::emoji::emoji_element(emoji, &self.fixtures, px(18.));
            h_flex()
                .id("hdr-channel-tile")
                .size(px(28.))
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .bg(cx.theme().secondary)
                .text_color(muted_foreground)
                .text_size(px(16.))
                .child(glyph)
        });
        let channel_lock = channel.filter(|c| c.is_private).map(|_| {
            Icon::new(IconName::Lock)
                .text_color(muted_foreground)
                .size(px(16.))
        });
        let header_title = if channel.is_some_and(|c| c.is_private) {
            title.strip_prefix("# ").unwrap_or(title).to_owned()
        } else {
            title.to_owned()
        };
        let peer = conversation
            .map(|c| conversation_avatar(&self.fixtures, c, 24., background, cx))
            .unwrap_or_else(div);
        // Slack's members chip: hover shows the "View all members" popover, a click opens
        // the details dialog straight onto its Members tab.
        let members_channel = channel.map(|c| c.id.clone());
        let members = conversation
            .filter(|c| {
                matches!(
                    c.kind(),
                    ConversationKind::PublicChannel | ConversationKind::PrivateChannel
                )
            })
            .and_then(|c| self.member_counts.get(&c.id).copied().or(c.num_members))
            .map(|count| {
                let popover_open = self.members_hover.0 || self.members_hover.1;
                div()
                    .id("hdr-members")
                    .relative()
                    .flex_none()
                    .cursor_pointer()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .px_1()
                            .h(px(28.))
                            .rounded_sm()
                            .text_sm()
                            .text_color(muted_foreground)
                            .child(
                                Icon::new(IconName::User)
                                    .text_color(muted_foreground)
                                    .size(px(16.)),
                            )
                            .child(count.to_string()),
                    )
                    // Slack keeps the pill lit while its tooltip is open, not only under
                    // the pointer.
                    .when(popover_open, |chip| chip.bg(accent))
                    .hover(|el| el.bg(accent))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.members_hover.0 = *hovered;
                        if *hovered {
                            this.load_channel_people(cx);
                        }
                        cx.notify();
                    }))
                    .when_some(members_channel.clone(), |chip, channel| {
                        chip.on_click(cx.listener(move |this, _, window, cx| {
                            this.members_hover = (false, false);
                            this.open_channel_details(
                                &channel,
                                channel_details::DetailsTab::Members,
                                window,
                                cx,
                            );
                        }))
                    })
                    .when(popover_open, |chip| {
                        chip.child(deferred(self.members_popover(cx)).with_priority(1))
                    })
            });
        // Slack counts the channel's apps apart: "👤 13  ⊞ 4".
        let apps = conversation
            .and_then(|c| self.app_counts.get(&c.id).copied())
            .filter(|&apps| apps > 0)
            .map(|apps| {
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child(
                        Icon::new(IconName::LayoutGrid)
                            .text_color(muted_foreground)
                            .size(px(16.)),
                    )
                    .child(apps.to_string())
            });

        // IMs: the peer avatar + name open the peer's profile, as in Slack.
        let peer_user = conversation
            .filter(|c| c.kind() == ConversationKind::Im)
            .and_then(|c| c.user.clone());
        // Channels: the name tile opens the details dialog, as in Slack.
        let details_channel = channel.map(|c| c.id.clone());
        // Slack offers no huddle with yourself.
        let can_huddle = peer_user.as_ref() != Some(&self.fixtures.self_user);
        // Slack keeps the whole name; the topic after it truncates instead.
        let name = h_flex()
            .id("hdr-peer")
            .flex_none()
            .items_center()
            .gap_2()
            .px_1()
            .rounded_sm()
            .child(peer)
            .children(channel_tile)
            .children(channel_lock)
            .child(
                div()
                    .truncate()
                    .text_size(px(17.))
                    .font_weight(FontWeight::BOLD)
                    .child(header_title),
            )
            .when_some(peer_user, |name, user| {
                name.cursor_pointer().hover(|el| el.bg(accent)).on_click(
                    cx.listener(move |this, _, _, cx| this.open_profile(user.clone(), cx)),
                )
            })
            .when_some(details_channel, |name, channel| {
                name.cursor_pointer()
                    .hover(|el| el.bg(accent))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_channel_details(
                            &channel,
                            channel_details::DetailsTab::About,
                            window,
                            cx,
                        )
                    }))
            });

        h_flex()
            .items_center()
            .gap_2()
            .px_4()
            .h(px(50.))
            .flex_none()
            .child(name)
            .when(!topic.is_empty(), |header| {
                // Slack renders the topic's mrkdwn: mentions by name, links, emoji.
                header.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child(crate::messages::mrkdwn_preview(
                            topic,
                            &self.fixtures,
                            muted_foreground,
                            format!("topic-{title}"),
                            cx,
                        )),
                )
            })
            .child(div().flex_1())
            .children(members)
            .children(apps)
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .when(can_huddle, |actions| {
                        actions
                            .child(
                                chrome_icon(
                                    "hdr-huddle",
                                    IconName::Headphones,
                                    if huddle_here {
                                        success
                                    } else {
                                        muted_foreground
                                    },
                                    accent,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.start_huddle(cx))),
                            )
                            .child(self.huddle_menu(cx))
                    })
                    .child(self.notify_menu(conversation, cx))
                    .child(
                        chrome_icon("hdr-search", IconName::Search, muted_foreground, accent)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.search_in_conversation(window, cx)
                            })),
                    )
                    .child(self.conversation_menu(conversation, cx)),
            )
    }

    /// The tab row under the header, derived from the conversation's `properties.tabs` in
    /// live mode ([`tabs`] for the rules) or the sample workspace's fixed set, with the
    /// active tab underlined. The trailing "+" opens Slack's new-tab menu, restricted to
    /// the actions slackless can run: Canvas (live only) and Link.
    fn conversation_tabs(&self, cx: &mut Context<Self>) -> Div {
        let border = cx.theme().border;
        let foreground = cx.theme().foreground;
        let muted_foreground = cx.theme().muted_foreground;

        let fallback = Conversation::default();
        let specs = tabs::derive(
            self.fixtures
                .conversation(&self.channel)
                .unwrap_or(&fallback),
            self.fixtures.sample,
        );
        let this = cx.entity().downgrade();
        let sample = self.fixtures.sample;
        let add_menu = Button::new("tab-add")
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::Plus)
            .tooltip("New tab")
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                if !sample {
                    let this = this.clone();
                    menu = menu.item(
                        PopupMenuItem::new("Canvas")
                            .icon(IconName::LayoutTemplate)
                            .on_click(move |_, _, cx| {
                                let _ = this.update(cx, |app, cx| app.open_or_add_canvas(cx));
                            }),
                    );
                }
                let this = this.clone();
                menu.item(PopupMenuItem::new("Link").icon(IconName::Link).on_click(
                    move |_, window, cx| {
                        let _ = this.update(cx, |app, cx| app.open_link_dialog(window, cx));
                    },
                ))
            });
        h_flex()
            .items_stretch()
            .flex_none()
            .px_4()
            .gap_5()
            .h(px(40.))
            .border_b_1()
            .border_color(border)
            .children(specs.into_iter().map(|spec| {
                let active = match &spec.action {
                    TabAction::Switch(target) => {
                        *target == self.tab
                            && !(*target == ConversationTab::Pins && !self.pins_open)
                    }
                    _ => false,
                };
                let entry = tab_views::tab(
                    spec.id,
                    spec.label,
                    spec.icon,
                    active,
                    foreground,
                    muted_foreground,
                    foreground,
                );
                match spec.action {
                    TabAction::Switch(target) => {
                        entry.on_click(cx.listener(move |this, _, _, cx| {
                            this.select_tab(target.clone(), cx);
                        }))
                    }
                    action => entry.on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_tab(action.clone(), cx);
                    })),
                }
            }))
            // The stretched row would top-align the small button: center it, as the tab
            // labels are.
            .child(h_flex().items_center().child(add_menu))
    }
}

/// The conversation a workspace opens on: its first DM, else its first channel.
/// The conversation to open: the one most recently read in Slack (live), else the first DM or
/// channel.
/// The in-app target of a Slack permalink (`https://<team>.slack.com/archives/...`).
struct Permalink {
    channel: ChannelId,
    /// The message the link points at (the `p<ts>` path segment).
    ts: Ts,
    /// The thread the link opens into, when it is a thread link (`?thread_ts=`).
    thread_ts: Option<Ts>,
}

/// Parse a Slack permalink; `None` for anything else (or a malformed one).
fn parse_permalink(url: &str) -> Option<Permalink> {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let mut path = base.strip_prefix("https://")?.split('/');
    let host = path.next()?;
    if !host.ends_with(".slack.com") {
        return None;
    }
    if path.next()? != "archives" {
        return None;
    }
    let channel = path.next()?.trim();
    if channel.is_empty() {
        return None;
    }
    let ts = ts_from_permalink(path.next()?.trim())?;
    let mut permalink = Permalink {
        channel: ChannelId::from(channel),
        ts,
        thread_ts: None,
    };
    for (key, value) in query.split('&').filter_map(|pair| pair.split_once('=')) {
        match key {
            "thread_ts" => permalink.thread_ts = Some(Ts::new(value)),
            // A thread link's channel rides the `cid` query parameter (DMs).
            "cid" => permalink.channel = ChannelId::from(value),
            _ => {}
        }
    }
    Some(permalink)
}

/// `p1234567890123456` back to Slack's `ts` (`1234567890.123456`): the permalink form drops
/// the dot before the six-digit fraction.
fn ts_from_permalink(digits: &str) -> Option<Ts> {
    let digits = digits.strip_prefix('p')?;
    if digits.len() < 7 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (secs, fraction) = digits.split_at(digits.len() - 6);
    Some(Ts::new(format!("{secs}.{fraction}")))
}

fn first_conversation(fixtures: &Fixtures) -> ChannelId {
    let last_read = fixtures
        .counts
        .values()
        .filter(|counts| counts.last_read.is_some() && fixtures.conversation(&counts.id).is_some())
        .max_by(|a, b| a.last_read.cmp(&b.last_read));
    last_read
        .map(|counts| counts.id.clone())
        .or_else(|| {
            fixtures
                .dms
                .first()
                .or(fixtures.channels.first())
                .map(|c| c.id.clone())
        })
        .unwrap_or_default()
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

/// Decode a system notification tag (`msg:<channel>:<ts>` or `later:<channel>:<ts>`).
fn parse_notification_tag(tag: &str) -> Option<(ChannelId, Ts)> {
    let rest = tag
        .strip_prefix("msg:")
        .or_else(|| tag.strip_prefix("later:"))?;
    let (channel, ts) = rest.split_once(':')?;
    (!channel.is_empty() && !ts.is_empty()).then(|| (ChannelId::from(channel), Ts::from(ts)))
}

/// First `@here` / `@channel` / `@everyone` broadcast inside `content`, if any.
fn broadcast_kind(content: &MessageContent) -> Option<BroadcastRange> {
    fn scan_elements(elements: &[RichTextElement]) -> Option<BroadcastRange> {
        elements.iter().find_map(|el| match el {
            RichTextElement::Broadcast(b) => Some(b.range.clone()),
            _ => None,
        })
    }
    fn scan_containers(containers: &[RichTextContainer]) -> Option<BroadcastRange> {
        containers.iter().find_map(|c| match c {
            RichTextContainer::Section(s) => scan_elements(&s.elements),
            RichTextContainer::Header(h) => scan_elements(&h.elements),
            RichTextContainer::Quote(q) => scan_elements(&q.elements),
            RichTextContainer::List(l) => scan_containers(&l.elements),
            RichTextContainer::Preformatted(_)
            | RichTextContainer::Divider(_)
            | RichTextContainer::Unknown { .. } => None,
        })
    }
    content.blocks.iter().find_map(|block| match block {
        Block::RichText(rt) => scan_containers(&rt.elements),
        Block::Unknown { .. } => None,
    })
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
                SidebarEvent::SwitchWorkspace(index) => this.switch_workspace(*index, window, cx),
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
                    this.self_state.start_focus(*preset);
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
                MessageListEvent::Changed { channel, change } => this.post_change(channel, change),
                MessageListEvent::OpenProfile(user) => this.open_profile(user.clone(), cx),
                MessageListEvent::OpenLink(target) => this.open_link(target.clone(), window, cx),
                MessageListEvent::OpenImage {
                    channel,
                    images,
                    index,
                } => this.open_image_viewer(channel.clone(), images.clone(), *index, window, cx),
                MessageListEvent::Forward(link) => this.open_compose(link, window, cx),
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
                ThreadPanelEvent::SlashCommand(invocation) => {
                    let thread = this.thread.read(cx).open_thread();
                    this.run_slash(invocation, thread, window, cx)
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
                SwitcherEvent::OpenUser(user) => {
                    window.close_dialog(cx);
                    this.message_user(user, window, cx);
                    this.focus.focus(window, cx);
                }
            },
        ),
    ];
    if let Some((_, bar)) = &views.huddle {
        subscriptions.push(
            cx.subscribe(bar, |this: &mut App, _, event: &HuddleEvent, cx| {
                let HuddleEvent::Leave = event;
                this.huddle_open = false;
                cx.notify();
            }),
        );
    }
    subscriptions
}

/// Screenshot/demo hook: `SLACKLESS_GUI_DEMO=browser` opens the channel browser at startup,
/// `SLACKLESS_GUI_DEMO=search:<query>` starts on the results of `<query>`,
/// `SLACKLESS_GUI_DEMO=canvas` / `=list` start in the huddle channel with the huddle bar open
/// on the Canvas / List tab. `=prefs` opens Preferences on Appearance & Themes, `=profile` the
/// first DM peer's profile, `=status` the status picker, `=switcher` the Cmd+K switcher and
/// `=menu` the title-bar user menu. `=blockkit` starts in #engineering on the deploy bot's
/// Block Kit message; `=emoji` additionally opens the composer's emoji picker. `=shortcuts`
/// opens the shortcuts sheet, `=drafts` Drafts & sent, `=side` the second workspace and
/// `=slash` the composer's slash menu. `=later` opens Later, `=pins` the pins panel,
/// `=activity` the Activity page, `=unreads` the All Unreads view, `=files` the Files view
/// (`=files:canvases` / `=files:lists` on that page), `=notify` posts a test system
/// notification, `=typing` shows two people typing in the open conversation and `=message`
/// delivers a demo direct message.
fn demo(results: &Entity<SearchView>, window: &mut Window, cx: &mut Context<App>) {
    let Ok(demo) = std::env::var("SLACKLESS_GUI_DEMO") else {
        return;
    };
    if matches!(demo.as_str(), "blockkit" | "emoji") {
        let this = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| {
                    app.select("C0ENG".into(), cx);
                    if demo == "emoji" {
                        // Where the composer's emoji button sits in the default 1200x800 window.
                        let anchor = point(px(340.), px(730.));
                        app.open_picker(PickerTarget::Composer, anchor, window, cx);
                    }
                });
            }
        });
        return;
    }
    if matches!(
        demo.as_str(),
        "prefs"
            | "profile"
            | "status"
            | "switcher"
            | "menu"
            | "shortcuts"
            | "drafts"
            | "side"
            | "slash"
            | "later"
            | "pins"
            | "activity"
            | "unreads"
            | "files"
            | "files:canvases"
            | "files:lists"
            | "threads"
            | "huddles"
            | "directory"
            | "compose"
            | "details"
            | "wsmenu"
            | "notify"
            | "typing"
            | "message"
    ) {
        let this = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| match demo.as_str() {
                    "prefs" => app.open_preferences(Some(Section::Appearance), window, cx),
                    "profile" => {
                        if let Some(peer) = app.fixtures.dms.first().and_then(|c| c.user.clone()) {
                            app.open_profile(peer, cx);
                        }
                    }
                    "status" => app.open_status_picker(window, cx),
                    "switcher" => app.open_switcher(window, cx),
                    "shortcuts" => app.open_shortcuts(window, cx),
                    "drafts" => app.open_drafts(cx),
                    "side" => app.switch_workspace(1, window, cx),
                    "slash" => app
                        .composer
                        .update(cx, |composer, cx| composer.load_draft("/", cx)),
                    "later" => app.open_later(cx),
                    "pins" => app.toggle_pins(cx),
                    "activity" => app.show_nav(NavItem::Activity, cx),
                    "unreads" => app.open_unreads(window, cx),
                    "files" => app.show_nav(NavItem::Files, cx),
                    "files:canvases" | "files:lists" => {
                        app.show_nav(NavItem::Files, cx);
                        let section = if demo == "files:lists" {
                            FilesSection::Lists
                        } else {
                            FilesSection::Canvases
                        };
                        app.sidebar
                            .update(cx, |sidebar, cx| sidebar.show_files_section(section, cx));
                    }
                    "threads" => app.open_threads(cx),
                    "huddles" => app.open_huddles(cx),
                    "directory" => app.open_directory(cx),
                    "compose" => app.open_compose("", window, cx),
                    "details" => {
                        // The Slack Connect channel: the dialog and banner together.
                        app.select("C0PARTNER".into(), cx);
                        app.open_channel_details(
                            &"C0PARTNER".into(),
                            channel_details::DetailsTab::About,
                            window,
                            cx,
                        );
                    }
                    "wsmenu" => app
                        .sidebar
                        .update(cx, |sidebar, cx| sidebar.show_settings_menu(cx)),
                    "notify" => app.post_notification(
                        "demo",
                        "slackless",
                        "Notifications are working.",
                        window,
                        cx,
                    ),
                    "typing" => {
                        // Two DM partners typing in the open conversation, re-announced as
                        // Slack's clients do while someone keeps typing.
                        let typists: Vec<UserId> = (app.fixtures.dms.iter())
                            .filter_map(|dm| dm.user.clone())
                            .take(2)
                            .collect();
                        cx.spawn(async move |this, cx| {
                            loop {
                                let typing = this.update(cx, |app, cx| {
                                    let channel = app.channel.clone();
                                    for user in &typists {
                                        app.someone_typing(&channel, user, None, cx);
                                    }
                                });
                                if typing.is_err() {
                                    break;
                                }
                                cx.background_executor().timer(Duration::from_secs(3)).await;
                            }
                        })
                        .detach();
                    }
                    "message" => {
                        // A direct message arriving live: Slack's notification rules, banner
                        // and sound.
                        let dm = (app.fixtures.dms.iter())
                            .find(|dm| {
                                dm.id != app.channel
                                    && dm.user.as_ref() != Some(&app.fixtures.self_user)
                            })
                            .cloned();
                        if let Some(dm) = dm {
                            let message = Message {
                                ts: Ts::from("9999999999.000100"),
                                channel: Some(dm.id.clone()),
                                user: dm.user.clone(),
                                text: "This is a demo direct message.".to_owned(),
                                ..Default::default()
                            };
                            app.message_posted(&dm.id, &message, window, cx);
                        }
                    }
                    _ => {
                        app.user_menu_open = true;
                        cx.notify();
                    }
                });
            }
        });
        return;
    }
    let surface = match demo.as_str() {
        "canvas" => Some(ConversationTab::Canvas),
        "list" => Some(ConversationTab::List),
        _ => None,
    };
    if let Some(tab) = surface {
        let this = cx.weak_entity();
        window.defer(cx, move |_, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| {
                    if let Some(channel) = app.huddle.as_ref().map(|(channel, _)| channel.clone()) {
                        app.select(channel, cx);
                        app.huddle_open = true;
                    }
                    app.tab = tab;
                    cx.notify();
                });
            }
        });
    } else if demo == "browser" {
        let this = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| app.open_browser(window, cx));
            }
        });
    } else if let Some(query) = demo.strip_prefix("search:") {
        let query = query.to_owned();
        results.update(cx, |results, cx| results.run(query, cx));
        let this = cx.weak_entity();
        window.defer(cx, move |_, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| {
                    app.searching = true;
                    cx.notify();
                });
            }
        });
    }
}

/// Drop a failed lazy tab load so its tab shows loading again while it is re-requested.
fn forget_failure<T>(loaded: &mut HashMap<ChannelId, Loaded<T>>, channel: &ChannelId) {
    if loaded.get(channel).is_some_and(Result::is_err) {
        loaded.remove(channel);
    }
}

/// A 28px square chrome icon button.
/// The members popover's "Includes …" line for one to three names, Slack's list
/// punctuation ("Includes A", "Includes A and B", "Includes A, B, and C").
fn includes_line(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [a] => Some(format!("Includes {a}")),
        [a, b] => Some(format!("Includes {a} and {b}")),
        [a, b, c, ..] => Some(format!("Includes {a}, {b}, and {c}")),
    }
}

/// Slack's stacked organization icons (Slack Connect bar and Join card), left to right, each
/// ringed in `ring`, the surface behind them.
fn org_stack(teams: &[Team], ring: Hsla) -> Div {
    h_flex()
        .flex_none()
        .children(teams.iter().enumerate().map(|(ix, team)| {
            div()
                .when(ix > 0, |chip| chip.ml(px(-7.)))
                .p(px(1.))
                .rounded(px(5.))
                .bg(ring)
                .child(channel_details::org_chip(team, 16.))
        }))
}

/// Whether `conversation` is a public channel the user is not in, where Slack shows the
/// Join card instead of the composer.
fn joinable(conversation: &Conversation) -> bool {
    conversation.kind() == ConversationKind::PublicChannel
        && !conversation.is_member
        && !conversation.is_archived
}

/// The Join card's "who is here": up to two names, then how many more people.
fn who_is_here(names: &[&str], more: usize) -> Option<String> {
    let more_people = |n: usize| match n {
        1 => "1 more person is".to_owned(),
        n => format!("{n} more people are"),
    };
    Some(match (names, more) {
        ([], 0) => return None,
        ([], 1) => "1 person is here.".to_owned(),
        ([], n) => format!("{n} people are here."),
        ([a], 0) => format!("{a} is here."),
        ([a], n) => format!("{a} and {} here.", more_people(n)),
        ([a, b, ..], 0) => format!("{a} and {b} are here."),
        ([a, b, ..], n) => format!("{a}, {b}, and {} here.", more_people(n)),
    })
}

/// The Join card's apps: "mastermind and norcebot are also here."
fn apps_also_here(apps: &[&str]) -> Option<String> {
    match apps {
        [] => None,
        [a] => Some(format!("{a} is also here.")),
        [a, b] => Some(format!("{a} and {b} are also here.")),
        [a, b, rest @ ..] => Some(format!(
            "{a}, {b}, and {} more apps are also here.",
            rest.len()
        )),
    }
}

/// A channel's people as Slack names them: the most recent distinct human authors of
/// `history` (newest first, never the signed-in user, never bots), topped up from `members`.
/// Free-standing for the test.
fn channel_people(fixtures: &Fixtures, members: &[UserId], history: &[Message]) -> Vec<UserId> {
    const LIMIT: usize = 3;
    let mut names: Vec<UserId> = Vec::new();
    let mut seen = HashSet::new();
    let self_user = &fixtures.self_user;
    for message in history.iter().rev() {
        // Thread replies and hidden metadata events are not channel authors; bots are not
        // people Slack would name.
        if message.hidden || message.bot_id.is_some() || message.bot_profile.is_some() {
            continue;
        }
        if let Some(user) = message.user.as_ref() {
            push_person(fixtures, self_user, &mut names, &mut seen, user);
            if names.len() == LIMIT {
                return names;
            }
        }
    }
    for member in members {
        push_person(fixtures, self_user, &mut names, &mut seen, member);
        if names.len() == LIMIT {
            break;
        }
    }
    names
}

/// Count `user` toward the channel's people once: only resolved people who are not the
/// signed-in user.
fn push_person(
    fixtures: &Fixtures,
    self_user: &UserId,
    names: &mut Vec<UserId>,
    seen: &mut HashSet<UserId>,
    user: &UserId,
) {
    if user == self_user || !seen.insert(user.clone()) {
        return;
    }
    let Some(record) = fixtures.user(user) else {
        return;
    };
    if record.is_bot || record.deleted {
        return;
    }
    names.push(user.clone());
}

fn chrome_icon(id: &'static str, icon: IconName, color: Hsla, hover_bg: Hsla) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(28.))
        .rounded_sm()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(color)
        .hover(|el| el.bg(hover_bg))
        .child(Icon::new(icon).text_color(color).size(px(16.)))
}

/// Whether `text` (Slack's mrkdwn) is for `me`: it mentions them (`<@U…>`), broadcasts
/// (`@here`, `@channel`, `@everyone`), or holds one of the comma-separated notification
/// `keywords` (case-insensitive).
fn mentions_me(text: &str, me: &UserId, keywords: &str) -> bool {
    let mention = format!("<@{me}");
    let tagged = text
        .match_indices(&mention)
        .any(|(at, _)| matches!(text[at + mention.len()..].chars().next(), Some('>' | '|')));
    if tagged
        || ["<!here", "<!channel", "<!everyone"]
            .iter()
            .any(|b| text.contains(b))
    {
        return true;
    }
    let lower = text.to_lowercase();
    (keywords.split(',').map(str::trim))
        .filter(|keyword| !keyword.is_empty())
        .any(|keyword| lower.contains(&keyword.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui;
    use slackless_core::types::User;

    #[test]
    fn permalinks_parse_into_their_in_app_targets() {
        let message =
            parse_permalink("https://acme.slack.com/archives/C0ENG/p17600000000123456").unwrap();
        assert_eq!(message.channel, ChannelId::from("C0ENG"));
        assert_eq!(message.ts.as_str(), "17600000000.123456");
        assert_eq!(message.thread_ts, None);

        // A thread link names its root in `thread_ts`; DM links carry the channel in `cid`.
        let thread = parse_permalink(
            "https://acme.slack.com/archives/D0ALICE/p17600000000654321?thread_ts=17600000000.123456&cid=D0ALICE",
        )
        .unwrap();
        assert_eq!(thread.channel, ChannelId::from("D0ALICE"));
        assert_eq!(
            thread.thread_ts.as_ref().map(Ts::as_str),
            Some("17600000000.123456")
        );

        // Anything else stays a web URL.
        assert!(parse_permalink("https://github.com/gpui").is_none());
        assert!(parse_permalink("https://acme.slack.com/help").is_none());
        assert!(parse_permalink("https://acme.slack.com/archives/C0ENG").is_none());
        assert!(parse_permalink("https://acme.slack.com/archives/C0ENG/plive").is_none());
    }

    #[test]
    fn includes_line_joins_slacks_member_list() {
        assert_eq!(includes_line(&[]), None);
        assert_eq!(includes_line(&["A".into()]), Some("Includes A".into()));
        assert_eq!(
            includes_line(&["A".into(), "B".into()]),
            Some("Includes A and B".into())
        );
        assert_eq!(
            includes_line(&["A".into(), "B".into(), "C".into()]),
            Some("Includes A, B, and C".into())
        );
        // Slack names three; a longer list never renders but still joins three.
        assert_eq!(
            includes_line(&["A".into(), "B".into(), "C".into(), "D".into()]),
            Some("Includes A, B, and C".into())
        );
    }

    #[test]
    fn channel_people_recent_authors_then_members() {
        let fixtures = fixtures::load();
        let person = |id: &str| User {
            id: UserId::from(id),
            real_name: Some(id.to_owned()),
            ..Default::default()
        };
        let bot = User {
            id: UserId::from("UBOT"),
            is_bot: true,
            ..Default::default()
        };
        let message = |user: Option<&str>, bot_profile: bool| Message {
            user: user.map(UserId::from),
            bot_profile: bot_profile.then(Default::default),
            ..Default::default()
        };
        // Newest last, so the popover reads authors newest first, skipping bots, the
        // signed-in user and repeats, then tops up from the member list.
        let history = vec![
            message(Some("U0ONE"), false),
            message(Some("UBOT"), false),
            message(Some("U0TWO"), false),
            message(None, true),
            message(Some("U0SELF"), false),
            message(Some("U0TWO"), false),
        ];
        let members = ["U0ONE", "U0THREE"].map(UserId::from).to_vec();
        let mut fixtures = fixtures;
        fixtures.self_user = UserId::from("U0SELF");
        fixtures.users = vec![person("U0ONE"), person("U0TWO"), person("U0THREE"), bot];
        let people = channel_people(&fixtures, &members, &history);
        assert_eq!(
            people.iter().map(UserId::as_str).collect::<Vec<_>>(),
            ["U0TWO", "U0ONE", "U0THREE"],
            "newest author first, then member top-up"
        );
    }

    #[test]
    fn notifications_follow_mentions_broadcasts_and_keywords() {
        let me = UserId::from("U1");
        assert!(mentions_me("hi <@U1>", &me, ""));
        assert!(mentions_me("hi <@U1|jakob>", &me, ""));
        assert!(!mentions_me("hi <@U12>", &me, ""));
        assert!(mentions_me("<!here> deploying", &me, ""));
        assert!(mentions_me("The Checkout is down", &me, "deploy, checkout"));
        assert!(!mentions_me("nothing here", &me, "deploy, , checkout"));
    }

    #[gpui::test]
    fn composer_typing_and_editing_are_not_swallowed_by_app_message_shortcuts(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui_kit::base::Root;
        use std::cell::RefCell;

        cx.update(|cx| {
            cx.set_global(crate::zoom::Zoom::default());
            gpui_kit::init(cx);
            theme::init(cx);
        });
        let mounted = Rc::new(RefCell::new(None));
        let slot = mounted.clone();
        let (root, cx) = cx.add_window_view(move |window, cx| {
            let app = App::new(
                Arc::new(fixtures::load()),
                CoreBridge::offline(),
                window,
                cx,
            );
            *slot.borrow_mut() = Some(app.clone());
            Root::new(app, window, cx)
        });
        let app = mounted.borrow_mut().take().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let _ = window.draw(cx);
        });

        // Focus the main composer and type every single-key shortcut letter (e, t, f, p, a, u, m, r)
        // plus Backspace and Right arrow through the full GPUI key dispatch tree.
        cx.update(|window, cx| {
            let composer = app.read(cx).composer.clone();
            composer.update(cx, |composer, cx| composer.focus_input(window, cx));
        });
        cx.run_until_parked();

        cx.simulate_keystrokes("e t f p a u m r");
        cx.run_until_parked();
        let typed = app.read_with(cx, |app, cx| app.composer.read(cx).text());
        assert_eq!(
            typed, "etfpaumr",
            "one-key message shortcuts must not swallow letters typed into the composer"
        );

        // Left, Backspace, Right, and typing at the end all reach the composer's textarea.
        cx.simulate_keystrokes("left backspace right !");
        cx.run_until_parked();
        let edited = app.read_with(cx, |app, cx| app.composer.read(cx).text());
        assert_eq!(
            edited, "etfpaur!",
            "backspace and right arrow must reach the focused composer"
        );

        // Select all + Cmd+B bolds the draft in place.
        cx.simulate_keystrokes("cmd-a cmd-b");
        cx.run_until_parked();
        let bolded = app.read_with(cx, |app, cx| app.composer.read(cx).text());
        assert_eq!(bolded, "*etfpaur!*");

        // Blurring back to the App root restores one-key message shortcuts (`e` opens WYSIWYG edit).
        let own_ts = app.read_with(cx, |app, cx| {
            app.messages
                .read(cx)
                .own_message_ts_for_test()
                .expect("fixture channel has an own message")
        });
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.messages
                    .update(cx, |list, cx| list.set_focused_for_test(&own_ts, cx));
                app.focus.focus(window, cx);
            });
        });
        cx.run_until_parked();

        cx.simulate_keystrokes("e");
        cx.run_until_parked();
        let editing = app.read_with(cx, |app, cx| app.messages.read(cx).is_editing_for_test(cx));
        assert!(
            editing,
            "pressing `e` outside an input opens inline WYSIWYG edit"
        );

        // Inside inline edit mode, typing `e, t, f, p, a, u, m, r` + backspace works and Esc cancels.
        cx.simulate_keystrokes("space e t f p a u m r backspace");
        cx.run_until_parked();
        let edit_text = app.read_with(cx, |app, cx| app.messages.read(cx).edit_text_for_test(cx));
        assert!(
            edit_text.ends_with(" etfpaum"),
            "typing in inline edit composer should work, got: {edit_text:?}"
        );

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        let still_editing =
            app.read_with(cx, |app, cx| app.messages.read(cx).is_editing_for_test(cx));
        assert!(!still_editing, "Esc cancels inline WYSIWYG edit mode");
        drop(root);
    }

    #[gpui::test]
    fn composer_live_preview_collapses_and_reveals_markdown_on_cursor_movement(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui_kit::base::Root;
        use std::cell::RefCell;

        cx.update(|cx| {
            cx.set_global(crate::zoom::Zoom::default());
            gpui_kit::init(cx);
            theme::init(cx);
        });
        let mounted = Rc::new(RefCell::new(None));
        let slot = mounted.clone();
        let (root, cx) = cx.add_window_view(move |window, cx| {
            let app = App::new(
                Arc::new(fixtures::load()),
                CoreBridge::offline(),
                window,
                cx,
            );
            *slot.borrow_mut() = Some(app.clone());
            Root::new(app, window, cx)
        });
        let app = mounted.borrow_mut().take().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let _ = window.draw(cx);
        });

        // Focus composer and type 3 headings + a paragraph with *bold* + trailing space.
        cx.update(|window, cx| {
            let composer = app.read(cx).composer.clone();
            composer.update(cx, |composer, cx| composer.focus_input(window, cx));
        });
        cx.run_until_parked();

        cx.simulate_keystrokes(
            "# space H 1 enter # # space H 2 enter # # # space H 3 enter * b o l d * space",
        );
        cx.run_until_parked();

        let tokens = app.read_with(cx, |app, cx| app.composer.read(cx).token_ids_for_test(cx));
        assert_eq!(
            tokens,
            vec![
                ("h1".into(), "# ".into()),
                ("h2".into(), "## ".into()),
                ("h3".into(), "### ".into()),
                ("hide".into(), "*".into()),
                ("hide".into(), "*".into()),
            ],
            "inactive headings show H1/H2/H3 badges and inactive *bold* hides its `*` markers"
        );

        // Moving Left onto `*bold*` reveals its `*` delimiters while headings stay badges.
        cx.simulate_keystrokes("left");
        cx.run_until_parked();
        let tokens = app.read_with(cx, |app, cx| app.composer.read(cx).token_ids_for_test(cx));
        assert_eq!(
            tokens,
            vec![
                ("h1".into(), "# ".into()),
                ("h2".into(), "## ".into()),
                ("h3".into(), "### ".into()),
            ],
            "caret touching `*bold*` reveals its `*` syntax"
        );

        // Moving Up onto `### H3` reveals `### ` while `*bold*` collapses again and H1/H2 stay badges.
        cx.simulate_keystrokes("up");
        cx.run_until_parked();
        let tokens = app.read_with(cx, |app, cx| app.composer.read(cx).token_ids_for_test(cx));
        assert_eq!(
            tokens,
            vec![
                ("h1".into(), "# ".into()),
                ("h2".into(), "## ".into()),
                ("hide".into(), "*".into()),
                ("hide".into(), "*".into()),
            ],
            "caret on H3 reveals `### ` while H1/H2 stay badges and `*bold*` re-collapses"
        );

        drop(root);
    }

    #[test]
    fn notification_tag_round_trips_channel_and_ts() {
        assert_eq!(
            parse_notification_tag("msg:C0ENG:1700000001.000200"),
            Some((ChannelId::from("C0ENG"), Ts::from("1700000001.000200")))
        );
        assert_eq!(
            parse_notification_tag("later:D123:1700000002.000300"),
            Some((ChannelId::from("D123"), Ts::from("1700000002.000300")))
        );
        assert_eq!(parse_notification_tag("other:C1:1.0"), None);
        assert_eq!(parse_notification_tag("msg:C1:"), None);
    }

    #[test]
    fn broadcast_kind_detects_channel_here_and_ignores_plain() {
        let fixtures = crate::fixtures::load();
        let plain = crate::composer::content_of("hello team", &fixtures);
        assert_eq!(broadcast_kind(&plain), None);

        let here = crate::composer::content_of("<!here> standup", &fixtures);
        assert_eq!(broadcast_kind(&here), Some(BroadcastRange::Here));

        let channel = crate::composer::content_of("> <!channel> deploy", &fixtures);
        assert_eq!(broadcast_kind(&channel), Some(BroadcastRange::Channel));
    }

    #[test]
    fn draft_round_trips_underline_and_usergroup_without_leaking_u_tag_to_mrkdwn() {
        let fixtures = crate::fixtures::load();
        let doc =
            crate::editor::from_mrkdwn("<u>important</u> ping <!subteam^S0ENG> now", &fixtures);
        assert_eq!(doc.to_draft(), "<u>important</u> ping <!subteam^S0ENG> now");
        assert_eq!(doc.to_mrkdwn(), "important ping <!subteam^S0ENG> now");
        let rt = doc.to_rich_text();
        let round = crate::editor::from_rich_text(&[rt.into()], &fixtures).unwrap();
        assert_eq!(round.to_draft(), doc.to_draft());
    }
}
