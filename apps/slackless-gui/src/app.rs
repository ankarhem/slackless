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
//! composer.rs
//!   Composer::new(placeholder: SharedString, &mut Window, &mut App) -> Entity<Composer>
//!   emits ComposerEvent::Submit(String)   (trimmed, non-empty; composer already cleared)
//!   emits ComposerEvent::OpenEmojiPicker(Point<Pixels>)
//!   Composer::insert(&mut self, &str, &mut Window, &mut Context<Self>)
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
//!   emits ThreadPanelEvent::Reply { channel: ChannelId, thread_ts: Ts, text: String }
//!       (reply already shown in the panel)
//!   emits ThreadPanelEvent::PickReaction { ts: Ts, position } / ThreadPanelEvent::PickEmoji(position)
//!   emits ThreadPanelEvent::SlashCommand(Invocation)   (commands other than /shrug)
//!   ThreadPanel::add_reaction(&mut self, &Ts, name: &str, &mut Context<Self>)
//!   ThreadPanel::insert_emoji(&mut self, &str, &mut Window, &mut Context<Self>)
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
//!   ProfilePanel::open(&mut self, UserId, &mut Context<Self>)
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
//!   Composer::text(&self, &App) -> String; Composer::load_draft(&mut self, String, cx)
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
//! pins.rs (bookmarks bar under the tabs; pins panel in the right column)
//!   pub fn bookmarks(&Fixtures, &ChannelId) / pinned(&Fixtures, &ChannelId)
//!   pub fn bookmarks_bar(..) -> Div / pins_panel(..) -> Div
//!
//! activity.rs (the sidebar's Activity destination)
//!   ActivityFeed::new(Arc<Fixtures>, &mut App) -> Entity<ActivityFeed>
//!   emits ActivityEvent::Open { channel, thread_ts }; the sidebar re-emits thread items as
//!   SidebarEvent::OpenThread { channel, thread_ts }
//!   Sidebar::badge_count(&App) -> u32 drives the dock badge (slackless_core::platform)
//!
//! app.rs
//!   App::new(Arc<Fixtures>, CoreBridge, &mut Window, &mut gpui::App) -> Entity<App>
//!   actions: QuickSwitch (cmd-k), OpenPreferences (cmd-,), ShowShortcuts (cmd-/),
//!   OpenDms (cmd-shift-k), OpenActivity (cmd-shift-a), OpenDrafts (cmd-shift-d),
//!   OpenChannelBrowser (cmd-shift-l), SetStatus (cmd-shift-y), FocusSearch (cmd-g),
//!   Workspace1..9 (cmd-1..9)

use std::collections::{HashMap, HashSet};
use std::mem;
use std::rc::Rc;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, Size, TitleBar, WindowExt as _, h_flex,
    h_resizable, resizable_panel, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    self, Anchor, AppContext as _, ClipboardItem, Context, Div, Edges, Entity, FocusHandle,
    Focusable as _, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyBinding,
    ParentElement as _, Pixels, Point, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Subscription, SystemNotification, Window,
    WindowAppearance, actions, anchored, deferred, div, point, px,
};
use slackless_core::api::ConversationCounts;
use slackless_core::platform::{self, BadgeSetter, Notifier};
use slackless_core::types::{ChannelId, Conversation, ConversationKind, Message, Ts, UserId};
use slackless_core::{Command, Event, SharedStore};

use crate::actions::notify;
use crate::avatars::conversation_avatar;
use crate::bridge::CoreBridge;
use crate::browser::{ChannelBrowser, ChannelBrowserEvent};
use crate::canvas::canvas_view;
use crate::composer::{Composer, ComposerEvent};
use crate::drafts::{DraftsEvent, DraftsView, Entry, SendLater, now_secs};
use crate::emoji::{EmojiChoice, EmojiPicker, EmojiPickerEvent};
use crate::fixtures::{self, Fixtures};
use crate::huddle::{self, HuddleBar, HuddleEvent};
use crate::later::{LaterEvent, LaterView, SaveRequest};
use crate::lists::list_view;
use crate::messages::{MessageList, MessageListEvent};
use crate::pins;
use crate::preferences::{AppearanceMode, Preferences, PreferencesEvent, Section};
use crate::profile::{ProfileEvent, ProfilePanel};
use crate::search::{SearchEvent, SearchView, fixture_archive};
use crate::shortcuts::shortcuts_view;
use crate::sidebar::{NavItem, Sidebar, SidebarEvent};
use crate::slash::{Invocation, SHRUG, SlashKind};
use crate::status::{SelfState, StatusEvent, StatusPicker, UserMenu, UserMenuEvent};
use crate::switcher::{QuickSwitcher, SwitcherEvent};
use crate::theme::{self, Palette, SlackTheme};
use crate::threads::{ThreadPanel, ThreadPanelEvent};

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
        SetStatus,
        FocusSearch,
        Workspace1,
        Workspace2,
        Workspace3,
        Workspace4,
        Workspace5,
        Workspace6,
        Workspace7,
        Workspace8,
        Workspace9,
    ]
);

/// How often due Later reminders are checked.
const REMINDER_POLL: Duration = Duration::from_secs(10);

/// The OS integration the root uses: system notifications and the dock badge.
trait Os: Notifier + BadgeSetter {}
impl<T: Notifier + BadgeSetter> Os for T {}

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

/// The conversation tab whose body fills the main column.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConversationTab {
    Messages,
    Canvas,
    List,
}

/// Where an emoji pick goes.
#[derive(Clone)]
enum PickerTarget {
    Composer,
    ThreadComposer,
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
    /// The active Slack palette; toggled from the global bar.
    theme: SlackTheme,
    sidebar: Entity<Sidebar>,
    messages: Entity<MessageList>,
    composer: Entity<Composer>,
    thread: Entity<ThreadPanel>,
    /// The global search field in the title bar.
    search: Entity<TextareaState>,
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
    /// The theme last installed, so unrelated preference edits do not repaint the theme.
    applied_theme: (SlackTheme, Palette),
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
    /// Drafts, scheduled and sent messages; shown in place of the message column while
    /// `drafts_open`.
    drafts: Entity<DraftsView>,
    drafts_open: bool,
    /// Saved messages and reminders; shown in place of the message column while `later_open`.
    later: Entity<LaterView>,
    later_open: bool,
    /// The pinned-messages panel of the open conversation (right column, exclusive with the
    /// thread and profile panels).
    pins_open: bool,
    /// Dock badge + native notifications (`slackless-platform`).
    os: Rc<dyn Os>,
    /// The badge count last handed to the dock, so it is only set on change.
    badge: Option<u32>,
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
    /// Member counts from open-time `conversations.info`, newer than the snapshot's.
    member_counts: HashMap<ChannelId, u32>,
    /// Conversations navigated away from, newest last (Slack's back arrow, Cmd+[).
    back: Vec<ChannelId>,
    /// Conversations stepped back from, newest last (the forward arrow, Cmd+]).
    forward: Vec<ChannelId>,
    /// Recently left conversations, newest first (the history clock's "Recent" menu).
    recent: Vec<ChannelId>,
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
            let channel = first_conversation(&fixtures);
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
            let composer = Composer::new("Message".into(), window, cx);
            let search = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Describe what you are looking for")
                    .auto_grow(1, 1)
            });
            let preferences = Preferences::new(window, cx);
            let status_picker = StatusPicker::new(window, cx);
            let user_menu = UserMenu::new(fixtures.clone(), cx);
            let emoji_picker = EmojiPicker::new(fixtures.clone(), window, cx);
            let drafts = DraftsView::new(cx);
            let later = LaterView::new(cx);
            later.update(cx, |later, cx| later.seed(0, &fixtures, cx));
            cx.bind_keys([
                KeyBinding::new("cmd-k", QuickSwitch, None),
                KeyBinding::new("cmd-[", NavigateBack, None),
                KeyBinding::new("cmd-]", NavigateForward, None),
                KeyBinding::new("cmd-,", OpenPreferences, None),
                KeyBinding::new("cmd-/", ShowShortcuts, None),
                KeyBinding::new("cmd-shift-k", OpenDms, None),
                KeyBinding::new("cmd-shift-a", OpenActivity, None),
                KeyBinding::new("cmd-shift-d", OpenDrafts, None),
                KeyBinding::new("cmd-shift-l", OpenChannelBrowser, None),
                KeyBinding::new("cmd-shift-y", SetStatus, None),
                KeyBinding::new("cmd-g", FocusSearch, None),
                KeyBinding::new("cmd-1", Workspace1, None),
                KeyBinding::new("cmd-2", Workspace2, None),
                KeyBinding::new("cmd-3", Workspace3, None),
                KeyBinding::new("cmd-4", Workspace4, None),
                KeyBinding::new("cmd-5", Workspace5, None),
                KeyBinding::new("cmd-6", Workspace6, None),
                KeyBinding::new("cmd-7", Workspace7, None),
                KeyBinding::new("cmd-8", Workspace8, None),
                KeyBinding::new("cmd-9", Workspace9, None),
            ]);
            let focus = cx.focus_handle();
            focus.focus(window, cx);

            let subscriptions = vec![
                bridge.subscribe_in(window, cx, |this: &mut App, event, window, cx| {
                    this.on_core_event(event, window, cx);
                }),
                cx.subscribe_in(
                    &search,
                    window,
                    |this: &mut App, search, event: &InputEvent, _, cx| {
                        if let InputEvent::PressEnter { .. } = event {
                            let query = search.read(cx).value().to_string();
                            this.results
                                .update(cx, |results, cx| results.run(query, cx));
                            this.searching = true;
                            this.drafts_open = false;
                            this.later_open = false;
                            cx.notify();
                        }
                    },
                ),
                cx.subscribe_in(
                    &composer,
                    window,
                    |this: &mut App, _, event: &ComposerEvent, window, cx| match event {
                        ComposerEvent::Submit(text) => this.send(text.clone(), cx),
                        ComposerEvent::OpenEmojiPicker(position) => {
                            this.open_picker(PickerTarget::Composer, *position, window, cx);
                        }
                        ComposerEvent::SlashCommand(invocation) => {
                            this.run_slash(invocation, window, cx);
                        }
                        ComposerEvent::Schedule { text, when } => {
                            this.schedule(text.clone(), *when, window, cx);
                        }
                    },
                ),
                cx.subscribe_in(
                    &emoji_picker,
                    window,
                    |this: &mut App, _, event: &EmojiPickerEvent, window, cx| {
                        let EmojiPickerEvent::Pick(choice) = event;
                        this.pick_emoji(choice, window, cx);
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
                            this.self_state.status = status.clone();
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
                        LaterEvent::Open { workspace, channel } => {
                            this.switch_workspace(*workspace, window, cx);
                            this.select(channel.clone(), cx);
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
            ];
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(REMINDER_POLL).await;
                    if this
                        .update_in(cx, |app, window, cx| app.fire_reminders(window, cx))
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

            App {
                fixtures,
                bridge,
                workspace: 0,
                parked,
                channel,
                theme: SlackTheme::Dark,
                sidebar,
                messages,
                composer,
                thread,
                search,
                results,
                searching: false,
                browser,
                thread_open: false,
                tab: ConversationTab::Messages,
                huddle,
                huddle_open: false,
                preferences,
                applied_theme: (SlackTheme::Dark, Palette::Default),
                profile,
                profile_open: false,
                self_state: SelfState::default(),
                status_picker,
                user_menu,
                user_menu_open: false,
                switcher,
                emoji_picker,
                picker: None,
                drafts,
                drafts_open: false,
                later,
                later_open: false,
                pins_open: false,
                os: Rc::new(platform::current()),
                badge: None,
                history_generation: 0,
                history_complete: HashSet::new(),
                thread_generation: 0,
                counts: HashMap::new(),
                presence: fixtures::PresenceMap::default(),
                peer_dnd_paused: HashMap::new(),
                member_counts: HashMap::new(),
                back: Vec::new(),
                forward: Vec::new(),
                recent: Vec::new(),
                chosen: false,
                focus,
                workspace_subscriptions,
                _subscriptions: subscriptions,
            }
        });
        app.update(cx, |app, cx| {
            app.push_self_state(cx);
            app.sync_saved(cx);
            app.sync_later_count(cx);
            let channel = app.channel.clone();
            app.load_history(&channel, cx);
            app.sync_composer_placeholder(cx);
        });
        app
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
            let tag = format!("later-{}-{}", item.channel, item.ts);
            self.post_notification(&tag, &title, &item.text, window, cx);
        }
    }

    /// A native notification, delivered once whichever way the app runs: GPUI's
    /// `SystemNotification` when bundled, the platform notifier when unbundled (see
    /// `slackless_platform::Notifier`); mirrored as an in-app toast.
    fn post_notification(
        &mut self,
        tag: &str,
        title: &str,
        body: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.show_system_notification(SystemNotification {
            tag: tag.to_owned().into(),
            title: title.to_owned().into(),
            body: body.to_owned().into(),
            actions: Vec::new(),
        });
        self.os.notify(title, body);
        notify(title, body.to_owned(), window, cx);
    }

    /// Hand the unread count to the dock badge when it changed (empty badge at zero).
    fn sync_badge(&mut self, cx: &mut Context<Self>) {
        let count = self.sidebar.read(cx).badge_count(cx);
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
        // Live mode: show the stored thread now, then fetch its full history from Slack
        // (fixture mode: both are no-ops and the panel keeps its fixture content).
        self.load_thread(&channel, &thread_ts, cx);
        self.bridge.send(Command::OpenThread { channel, thread_ts });
        self.thread_open = true;
        self.profile_open = false;
        self.pins_open = false;
        cx.notify();
    }

    fn toggle_pins(&mut self, cx: &mut Context<Self>) {
        self.pins_open = !self.pins_open;
        if self.pins_open {
            self.thread_open = false;
            self.profile_open = false;
        }
        cx.notify();
    }

    /// Apply what the app honors from Preferences: the theme (Light / Dark / Sync with OS,
    /// tinted by the color theme), Enter-to-send, and the schedule shown in the user menu.
    fn apply_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.preferences.read(cx).settings().clone();
        let theme = match settings.mode {
            AppearanceMode::Light => SlackTheme::Light,
            AppearanceMode::Dark => SlackTheme::Dark,
            AppearanceMode::System => match window.appearance() {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => SlackTheme::Dark,
                WindowAppearance::Light | WindowAppearance::VibrantLight => SlackTheme::Light,
            },
        };
        if self.applied_theme != (theme, settings.palette) {
            self.theme = theme;
            self.applied_theme = (theme, settings.palette);
            theme::activate_with(theme, settings.palette, cx);
        }
        let enter_sends = settings.enter_sends;
        self.composer
            .update(cx, |composer, cx| composer.set_enter_sends(enter_sends, cx));
        self.thread
            .update(cx, |thread, cx| thread.set_enter_sends(enter_sends, cx));
        self.push_self_state(cx);
    }

    /// Hand the current status / presence / pause to the views that show it.
    /// Flip between Slack's dark and light palettes (the rail's moon button).
    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = match self.theme.toggle() {
            SlackTheme::Dark => AppearanceMode::Dark,
            SlackTheme::Light => AppearanceMode::Light,
        };
        self.preferences
            .update(cx, |prefs, cx| prefs.set_appearance_mode(mode, cx));
        self.apply_preferences(window, cx);
    }

    /// The composer prompts with the open conversation's name, as Slack does.
    fn sync_composer_placeholder(&mut self, cx: &mut Context<Self>) {
        let label = self.label(&self.channel).replacen("# ", "#", 1);
        self.composer.update(cx, |composer, cx| {
            composer.set_placeholder(format!("Message {label}").into(), cx)
        });
    }

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
            UserMenuEvent::ClearStatus => self.self_state.status = None,
            UserMenuEvent::SetAway(away) => self.self_state.away = *away,
            UserMenuEvent::Pause(until) => {
                self.self_state.paused_until = *until;
                self.user_menu_open = false;
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
        }
        self.push_self_state(cx);
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
        let status = self.self_state.status.clone();
        self.status_picker
            .update(cx, |picker, cx| picker.edit(status.as_ref(), window, cx));
        let picker = self.status_picker.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Set a status")
                .w(px(480.))
                .child(picker.clone())
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

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Route a pick: composers get the glyph (custom emoji as `:name:`), messages a reaction.
    fn pick_emoji(&mut self, choice: &EmojiChoice, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, _)) = self.picker.take() else {
            return;
        };
        let text = choice
            .glyph
            .clone()
            .unwrap_or_else(|| format!(":{}: ", choice.name));
        match target {
            PickerTarget::Composer => {
                self.composer
                    .update(cx, |composer, cx| composer.insert(&text, window, cx));
            }
            PickerTarget::ThreadComposer => {
                self.thread
                    .update(cx, |thread, cx| thread.insert_emoji(&text, window, cx));
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
        cx.notify();
    }

    fn open_profile(&mut self, user: UserId, cx: &mut Context<Self>) {
        self.profile
            .update(cx, |profile, cx| profile.open(user, cx));
        self.profile_open = true;
        self.thread_open = false;
        self.pins_open = false;
        cx.notify();
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
        self.searching = false;
        self.drafts_open = false;
        self.later_open = false;
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

    /// "Leave channel": it drops out of the sidebar at once and the view steps back, as in
    /// Slack; the core leaves on Slack and stores the channel as left.
    fn leave(&mut self, channel: ChannelId, window: &mut Window, cx: &mut Context<Self>) {
        self.bridge
            .send(Command::LeaveConversation(channel.clone()));
        self.counts.remove(&channel);
        self.back.retain(|c| *c != channel);
        self.forward.retain(|c| *c != channel);
        self.recent.retain(|c| *c != channel);
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
        /// Recent conversations kept for the history menu.
        const RECENT_MAX: usize = 50;
        let left = self.channel.clone();
        self.recent.retain(|c| *c != left && *c != channel);
        self.recent.insert(0, left);
        self.recent.truncate(RECENT_MAX);
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
        self.thread_open = false;
        self.tab = ConversationTab::Messages;
        self.load_draft(cx);
        self.sync_composer_placeholder(cx);
        cx.notify();
    }

    /// Live mode: fold a core event into the views.
    fn on_core_event(&mut self, event: &Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Conversations(_) | Event::UsersChanged(_) | Event::SidebarChanged => {
                self.reload_live(window, cx)
            }
            Event::Counts(counts) => {
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
            Event::ConversationInfo {
                channel,
                num_members: Some(count),
            } => {
                self.member_counts.insert(channel.clone(), *count);
                cx.notify();
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
        // Presence is transient (never stored), so the live map rides along every snapshot.
        snapshot.presence = self.presence.clone();
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
        let keep = self.chosen && fixtures.conversation(&self.channel).is_some();
        self.fixtures = fixtures;
        self.resolve_users(self.fixtures.dms.iter().filter_map(|dm| dm.user.clone()));
        if !keep {
            self.show_conversation(first_conversation(&self.fixtures), cx);
        }
        self.sync_composer_placeholder(cx);
        self.apply_preferences(window, cx);
        cx.notify();
    }

    /// Live mode: show `channel`'s stored history, read off the UI thread. A read that a newer
    /// one overtook, or whose conversation is no longer open, is dropped.
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
        // Dedupe before the snapshot lookups: a page names the same few people many times.
        let named: HashSet<UserId> = users.into_iter().collect();
        let missing: Vec<UserId> = named
            .into_iter()
            .filter(|user| self.fixtures.user(user).is_none())
            .collect();
        if !missing.is_empty() {
            self.bridge.send(Command::ResolveUsers(missing));
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
    /// Slack is told (`conversations.mark`) and the sidebar un-bolds it at once.
    fn mark_read(&mut self, channel: &ChannelId, newest: Ts, cx: &mut Context<Self>) {
        let Some(counts) = self.counts.get_mut(channel) else {
            return;
        };
        if !counts.has_unreads && counts.mention_count == 0 {
            return;
        }
        counts.has_unreads = false;
        counts.mention_count = 0;
        counts.last_read = Some(newest.clone());
        self.bridge.send(Command::MarkRead {
            channel: channel.clone(),
            ts: newest,
        });
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.mark_read(channel.clone(), cx));
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
        self.searching = false;
        self.drafts_open = false;
        self.later_open = false;
        self.thread_open = false;
        self.profile_open = false;
        self.pins_open = false;
        let fixtures = self.fixtures.clone();
        self.later
            .update(cx, |later, cx| later.seed(index, &fixtures, cx));
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
        let text = self.composer.read(cx).text(cx);
        let saved = self
            .drafts
            .read(cx)
            .draft(self.workspace, &self.channel)
            .unwrap_or_default();
        if text == saved {
            return;
        }
        let entry = Entry {
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
    }

    /// Put the open conversation's saved draft (or nothing) in the composer.
    fn load_draft(&mut self, cx: &mut Context<Self>) {
        let drafts = self.drafts.read(cx);
        let text = drafts
            .draft(self.workspace, &self.channel)
            .unwrap_or_default();
        let count = drafts.draft_count();
        self.composer
            .update(cx, |composer, cx| composer.load_draft(text, cx));
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_draft_count(count, cx));
    }

    fn open_drafts(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.drafts_open = true;
        self.later_open = false;
        self.searching = false;
        cx.notify();
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
        self.drafts_open = false;
        self.later_open = false;
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.show_nav(item, cx));
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
        let entry = Entry {
            workspace: self.workspace,
            channel: self.channel.clone(),
            label: self.label(&self.channel),
            text,
            at: when.at(now_secs()),
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
        if entry.workspace == self.workspace {
            let message = self
                .fixtures
                .local_message(&entry.channel, entry.text.clone(), None);
            let channel = entry.channel.clone();
            self.messages
                .update(cx, |list, cx| list.append(channel, message, cx));
        }
        self.bridge.send(Command::SendMessage {
            channel: entry.channel.clone(),
            text: entry.text.clone(),
            thread_ts: None,
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

    /// Dispatch a slash command. Fixture mode runs the ones that only touch local state
    /// (/shrug, /away, /status) and logs the rest with a toast.
    fn run_slash(&mut self, invocation: &Invocation, window: &mut Window, cx: &mut Context<Self>) {
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
                self.send(text, cx);
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

    fn send(&mut self, text: String, cx: &mut Context<Self>) {
        let channel = self.channel.clone();
        let message = self.fixtures.local_message(&channel, text.clone(), None);
        self.messages
            .update(cx, |list, cx| list.append(channel.clone(), message, cx));
        let entry = Entry {
            workspace: self.workspace,
            channel: channel.clone(),
            label: self.label(&channel),
            text: text.clone(),
            at: now_secs(),
        };
        self.drafts
            .update(cx, |drafts, cx| drafts.record_sent(entry, cx));
        self.bridge.send(Command::SendMessage {
            channel,
            text,
            thread_ts: None,
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
        self.sync_badge(cx);

        let conversation = self.fixtures.conversation(&self.channel).cloned();
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
        } else {
            let header = self.conversation_header(&title, &topic, conversation.as_ref(), cx);
            let tabs = self.conversation_tabs(cx);
            let this = cx.weak_entity();
            // Like Slack, no bookmark bar for a conversation with neither bookmarks nor pins.
            let bookmark_list = pins::bookmarks(&self.fixtures, &self.channel);
            let pin_count = pins::pinned(&self.fixtures, &self.channel).len();
            let bookmarks = (!bookmark_list.is_empty() || pin_count > 0).then(|| {
                pins::bookmarks_bar(
                    bookmark_list,
                    pin_count,
                    self.pins_open,
                    move |_, cx| {
                        this.update(cx, |app, cx| app.toggle_pins(cx)).ok();
                    },
                    cx,
                )
            });
            let column = v_flex().size_full().child(header);
            let column = match (&self.huddle, self.huddle_open) {
                (Some((_, bar)), true) => column.child(bar.clone()),
                _ => column,
            };
            let column = column.child(tabs).children(bookmarks);
            match self.tab {
                ConversationTab::Messages => column
                    .child(div().flex_1().min_h_0().child(self.messages.clone()))
                    .children(self.dnd_banner(conversation.as_ref(), cx))
                    .child(self.composer.clone()),
                ConversationTab::Canvas => {
                    column.child(div().flex_1().min_h_0().child(canvas_view(cx)))
                }
                ConversationTab::List => {
                    column.child(div().flex_1().min_h_0().child(list_view(cx)))
                }
            }
        };

        let mut panels = h_resizable("workspace")
            // Like Slack, opening the thread / profile / pins panel narrows the conversation,
            // never the sidebar.
            .child(
                resizable_panel()
                    .size(px(360.))
                    .size_range(px(360.)..px(640.))
                    .child(self.sidebar.clone()),
            )
            .child(resizable_panel().child(main_column));
        if self.thread_open {
            panels = panels.child(
                resizable_panel()
                    .size(px(380.))
                    .size_range(px(340.)..px(720.))
                    .child(self.thread.clone()),
            );
        } else if self.profile_open {
            panels = panels.child(
                resizable_panel()
                    .size(px(360.))
                    .size_range(px(340.)..px(720.))
                    .child(self.profile.clone()),
            );
        } else if self.pins_open {
            let this = cx.weak_entity();
            let close = move |_: &mut Window, cx: &mut gpui::App| {
                this.update(cx, |app, cx| {
                    app.pins_open = false;
                    cx.notify();
                })
                .ok();
            };
            let panel = pins::pins_panel(&self.fixtures, &self.channel, &title, close, cx);
            panels = panels.child(
                resizable_panel()
                    .size(px(380.))
                    .size_range(px(340.)..px(720.))
                    .child(panel),
            );
        }

        v_flex()
            .id("root")
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
                cx.listener(|this, _: &SetStatus, window, cx| this.open_status_picker(window, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.search.read(cx).focus_handle(cx).focus(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NavigateBack, _, cx| this.go_back(cx)))
            .on_action(cx.listener(|this, _: &NavigateForward, _, cx| this.go_forward(cx)))
            .on_action(
                cx.listener(|this, _: &Workspace1, window, cx| {
                    this.switch_workspace(0, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &Workspace2, window, cx| {
                    this.switch_workspace(1, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &Workspace3, window, cx| {
                    this.switch_workspace(2, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &Workspace4, window, cx| {
                    this.switch_workspace(3, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &Workspace5, window, cx| {
                    this.switch_workspace(4, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &Workspace6, window, cx| {
                    this.switch_workspace(5, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &Workspace7, window, cx| {
                    this.switch_workspace(6, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &Workspace8, window, cx| {
                    this.switch_workspace(7, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &Workspace9, window, cx| {
                    this.switch_workspace(8, window, cx)
                }),
            )
            .relative()
            .size_full()
            .bg(background)
            .text_color(foreground)
            .child(title_bar)
            .child(div().flex_1().min_h_0().w_full().child(panels))
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
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("user-menu")
                        .occlude()
                        .absolute()
                        .bottom(px(12.))
                        .left(px(80.))
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
    }
}

impl App {
    /// The global bar (gpui-kit `TitleBar`): history navigation on the left, the wide
    /// centered search pill, and the workspace chip, theme toggle and help on the right —
    /// the reference screenshot's top row.
    fn global_bar(&mut self, cx: &mut Context<Self>) -> TitleBar {
        let muted = cx.theme().muted;
        let muted_foreground = cx.theme().muted_foreground;
        let border = cx.theme().border;
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

        let search_pill = h_flex()
            .w(px(520.))
            .max_w_full()
            .items_center()
            .gap_2()
            .px_2()
            .py_0p5()
            .rounded_md()
            .bg(muted)
            .border_1()
            .border_color(border)
            .child(
                Icon::new(IconName::Search)
                    .text_color(muted_foreground)
                    .size(px(14.)),
            )
            .child(
                div().flex_1().min_w_0().text_sm().child(
                    Textarea::new(&self.search)
                        .appearance(false)
                        .bordered(false),
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

        TitleBar::new().child(
            h_flex()
                .flex_1()
                .items_center()
                .gap_2()
                .pr_2()
                .child(nav)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .justify_center()
                        .child(search_pill),
                )
                .child(
                    h_flex().items_center().gap_1().child(status_button).child(
                        chrome_icon(
                            "help",
                            IconName::CircleQuestionMark,
                            muted_foreground,
                            accent,
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_shortcuts(window, cx)),
                        ),
                    ),
                ),
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

        // Channels: the label already carries `# name`, like the reference header.
        let peer = conversation
            .map(|c| conversation_avatar(&self.fixtures, c, 24., background, cx))
            .unwrap_or_else(div);
        let members = conversation
            .filter(|c| {
                matches!(
                    c.kind(),
                    ConversationKind::PublicChannel | ConversationKind::PrivateChannel
                )
            })
            .and_then(|c| self.member_counts.get(&c.id).copied().or(c.num_members))
            .map(|count| {
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child(
                        Icon::new(IconName::Users)
                            .text_color(muted_foreground)
                            .size(px(14.)),
                    )
                    .child(count.to_string())
            });

        // IMs: the peer avatar + name open the peer's profile, as in Slack.
        let peer_user = conversation
            .filter(|c| c.kind() == ConversationKind::Im)
            .and_then(|c| c.user.clone());
        // Slack keeps the whole name; the topic after it truncates instead.
        let name = h_flex()
            .id("hdr-peer")
            .flex_none()
            .items_center()
            .gap_2()
            .px_1()
            .rounded_sm()
            .child(peer)
            .child(
                div()
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .child(title.to_owned()),
            )
            .when_some(peer_user, |name, user| {
                name.cursor_pointer().hover(|el| el.bg(accent)).on_click(
                    cx.listener(move |this, _, _, cx| this.open_profile(user.clone(), cx)),
                )
            });

        h_flex()
            .items_center()
            .gap_2()
            .px_4()
            .h(px(46.))
            .flex_none()
            .child(name)
            .when(!topic.is_empty(), |header| {
                header.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child(topic.to_owned()),
                )
            })
            .child(div().flex_1())
            .children(members)
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
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
                    .child(chrome_icon(
                        "hdr-bell",
                        IconName::Bell,
                        muted_foreground,
                        accent,
                    ))
                    .child(
                        chrome_icon("hdr-search", IconName::Search, muted_foreground, accent)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.search_in_conversation(window, cx)
                            })),
                    )
                    .child(self.conversation_menu(conversation, cx)),
            )
    }

    /// The tab row under the header: Messages / Canvas / List (the active one underlined) /
    /// Files & links, plus a trailing new-canvas `+`. Canvas and List are read-only fixture
    /// views; Files & links is not wired yet.
    fn conversation_tabs(&self, cx: &mut Context<Self>) -> Div {
        let border = cx.theme().border;
        let link = cx.theme().link;
        let foreground = cx.theme().foreground;
        let muted_foreground = cx.theme().muted_foreground;

        // Live conversations get Slack's tab row; the read-only Canvas / List fixture views
        // exist only in the sample workspace.
        const LIVE_TABS: [(&str, &str, IconName, Option<ConversationTab>); 3] = [
            (
                "tab-messages",
                "Messages",
                IconName::MessageSquare,
                Some(ConversationTab::Messages),
            ),
            ("tab-canvas", "Add canvas", IconName::LayoutTemplate, None),
            ("tab-files", "Files & links", IconName::Files, None),
        ];
        // Channels add Slack's Pins tab after Files & links.
        const CHANNEL_TABS: [(&str, &str, IconName, Option<ConversationTab>); 4] = [
            (
                "tab-messages",
                "Messages",
                IconName::MessageSquare,
                Some(ConversationTab::Messages),
            ),
            ("tab-canvas", "Add canvas", IconName::LayoutTemplate, None),
            ("tab-files", "Files & links", IconName::Files, None),
            ("tab-pins", "Pins", IconName::Pin, None),
        ];
        let channel = self.fixtures.conversation(&self.channel).is_some_and(|c| {
            matches!(
                c.kind(),
                ConversationKind::PublicChannel | ConversationKind::PrivateChannel
            )
        });
        const TABS: [(&str, &str, IconName, Option<ConversationTab>); 4] = [
            (
                "tab-messages",
                "Messages",
                IconName::MessageSquare,
                Some(ConversationTab::Messages),
            ),
            (
                "tab-canvas",
                "Canvas",
                IconName::LayoutTemplate,
                Some(ConversationTab::Canvas),
            ),
            (
                "tab-list",
                "List",
                IconName::ListChecks,
                Some(ConversationTab::List),
            ),
            ("tab-files", "Files & links", IconName::Files, None),
        ];
        h_flex()
            .items_stretch()
            .flex_none()
            .px_4()
            .gap_5()
            .h(px(38.))
            .border_b_1()
            .border_color(border)
            .children(
                if self.fixtures.sample {
                    &TABS[..]
                } else if channel {
                    &CHANNEL_TABS[..]
                } else {
                    &LIVE_TABS[..]
                }
                .iter()
                .map(|(id, label, icon, target)| {
                    let active = *target == Some(self.tab);
                    let tab = tab(id, label, *icon, active, foreground, muted_foreground, link);
                    match *target {
                        Some(target) => tab.on_click(cx.listener(move |this, _, _, cx| {
                            this.tab = target;
                            cx.notify();
                        })),
                        None => tab,
                    }
                }),
            )
            .child(
                div()
                    .id("tab-add")
                    .flex_none()
                    .h_full()
                    .flex()
                    .items_center()
                    .px_1()
                    .cursor_pointer()
                    .text_color(muted_foreground)
                    .hover(|el| el.text_color(foreground))
                    .child(
                        Icon::new(IconName::Plus)
                            .text_color(muted_foreground)
                            .size(px(16.)),
                    ),
            )
    }
}

/// The conversation a workspace opens on: its first DM, else its first channel.
/// The conversation to open: the one most recently read in Slack (live), else the first DM or
/// channel.
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
                SidebarEvent::ToggleTheme => this.toggle_theme(window, cx),
                SidebarEvent::OpenUserMenu => {
                    this.user_menu_open = !this.user_menu_open;
                    cx.notify();
                }
                SidebarEvent::OpenThread { channel, thread_ts } => {
                    this.open_thread(channel.clone(), thread_ts.clone(), cx);
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
                SearchEvent::Open(channel) => this.select(channel.clone(), cx),
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
                ThreadPanelEvent::Close => {
                    this.thread_open = false;
                    cx.notify();
                }
                ThreadPanelEvent::Reply {
                    channel,
                    thread_ts,
                    text,
                } => {
                    this.bridge.send(Command::SendMessage {
                        channel: channel.clone(),
                        text: text.clone(),
                        thread_ts: Some(thread_ts.clone()),
                    });
                }
                ThreadPanelEvent::SlashCommand(invocation) => {
                    this.run_slash(invocation, window, cx)
                }
                ThreadPanelEvent::Later(request) => this.save_for_later(request, cx),
            },
        ),
        cx.subscribe_in(
            &views.profile,
            window,
            |this: &mut App, _, event: &ProfileEvent, window, cx| match event {
                ProfileEvent::Close => {
                    this.profile_open = false;
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
            |this: &mut App, _, event: &SwitcherEvent, window, cx| {
                let SwitcherEvent::Open(channel) = event;
                window.close_dialog(cx);
                this.select(channel.clone(), cx);
                this.focus.focus(window, cx);
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
/// `=activity` the Activity feed, and `=notify` posts a test system notification.
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
            | "notify"
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
                        .update(cx, |composer, cx| composer.load_draft("/".into(), cx)),
                    "later" => app.open_later(cx),
                    "pins" => app.toggle_pins(cx),
                    "activity" => app.show_nav(NavItem::Activity, cx),
                    "notify" => app.post_notification(
                        "demo",
                        "slackless",
                        "Notifications are working.",
                        window,
                        cx,
                    ),
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

/// A 28px square chrome icon button.
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

/// One conversation tab; the active tab carries the link-colored underline.
fn tab(
    id: &'static str,
    label: &'static str,
    icon: IconName,
    active: bool,
    foreground: Hsla,
    muted: Hsla,
    underline: Hsla,
) -> Stateful<Div> {
    let color = if active { foreground } else { muted };
    div()
        .id(id)
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .gap_1p5()
        .px_0p5()
        .cursor_pointer()
        .text_size(px(13.))
        .text_color(color)
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .when(active, |tab| tab.border_b_2().border_color(underline))
        .hover(|el| el.text_color(foreground))
        .child(Icon::new(icon).text_color(color).size(px(14.)))
        .child(label)
}
