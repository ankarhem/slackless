//! Bridge between the GUI (smol, GPUI's executor) and slackless-core (tokio on its own thread),
//! following the verified pattern in `docs/research/gpui-kit-guide.md` section 13.
//!
//! Live mode ([`CoreBridge::start`]) opens the signed-in session's local store and hands both to
//! [`slackless_core::Client::start`], which owns the dedicated OS thread running the
//! tokio runtime. The core talks to this process over runtime-agnostic
//! `futures::channel::mpsc` channels: [`Command`]s in, [`Event`]s out. Incoming events are
//! drained into gpui by a `cx.spawn` task and re-emitted from an internal [`EventHub`] entity,
//! so views subscribe with gpui's native `subscribe` and always run on the UI thread.
//!
//! Fixture mode ([`CoreBridge::offline`]) has no core: [`CoreBridge::send`] drops commands and
//! the views apply their changes locally, so the app renders without Slack credentials or a
//! network connection.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::gpui::{self, AppContext as _, Context, Entity, EventEmitter, Subscription, Window};
use slackless_core::api::ConversationCounts;
use slackless_core::auth::Session;
use slackless_core::store::Store;
use slackless_core::types::{ChannelId, Conversation, Message, Team, TeamId, Ts, User, UserId};
use slackless_core::{Client, ClientHandles, Command, Event, HuddleMediaEvent, SharedStore};

use crate::fixtures::{self, Fixtures};
use crate::media::CoreHttp;
use crate::preferences::{AppearanceMode, Settings};
use crate::theme;

/// Synthetic `Message::extra` marker attached to locally preserved deleted rows when the
/// user opts into showing deleted ghost tombstones.
pub const DELETED_EXTRA_KEY: &str = "slackless_deleted";

fn row_to_message(mut row: slackless_core::store::StoredMessage) -> Message {
    if row.deleted {
        row.message
            .extra
            .insert(DELETED_EXTRA_KEY.to_owned(), serde_json::Value::Bool(true));
    }
    row.message
}

/// Minimal context for processing a background workspace's `Event::MessagePosted` (unread bump +
/// desktop notification) in O(1) SQLite PK reads rather than rebuilding a full `Fixtures` snapshot.
pub struct BgMessageContext {
    pub self_user: UserId,
    pub conversation: Option<Conversation>,
    pub sender: Option<User>,
}

/// One persisted composer draft in the workspace's local store.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedDraft {
    pub channel: ChannelId,
    pub label: String,
    pub text: String,
    pub at: u64,
}

/// Persisted per-section sidebar filter and sort preferences for one workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SectionPrefs {
    #[serde(default)]
    pub filters: HashMap<String, crate::sidebar_menu::SectionFilter>,
    #[serde(default)]
    pub sorts: HashMap<String, crate::sidebar_menu::SectionSort>,
}

/// Newest messages of a conversation the message list loads on scroll-back / live refresh
/// (older pages load seamlessly on scroll-up via `prepend_history`).
const HISTORY_LIMIT: usize = 200;

/// Newest messages loaded synchronously for frame-0 cold launch (enough to fill any viewport
/// while avoiding deserializing 500 messages before the first frame paints).
const INITIAL_HISTORY_LIMIT: usize = 80;

/// The store's local-state key for the last open conversation.
const LAST_CONVERSATION: &str = "last_conversation";

/// The store's local-state key for per-section sidebar filter and sort choices.
const SECTION_PREFS: &str = "section_prefs";

/// The store's local-state key for the window's frame (AppKit restorable state, hex-encoded).
const WINDOW_STATE: &str = "window_state";

/// The store's local-state key for the Color Mode chosen in Preferences.
const COLOR_MODE: &str = "color_mode";

/// The store's local-state key for the search pill's recent searches.
const SEARCH_HISTORY: &str = "search_history";

/// The store's local-state key for the search panel's history toggle.
const SEARCH_HISTORY_SHOWN: &str = "search_history_shown";

/// The store's local-state key for the search panel's filter toggle.
const SEARCH_FILTERS_SHOWN: &str = "search_filters_shown";

/// The store's local-state key for the recently visited conversations (the search menu's
/// "Recent places" and the history clock's menu).
const RECENT_PLACES: &str = "recent_places";

/// The store's local-state key for the Font chosen in Preferences (its family name).
const FONT: &str = "font";

/// The store's local-state key for the zoom chosen in Preferences (its level).
const ZOOM: &str = "zoom";

/// The store's local-state key for all Preferences settings (JSON).
pub(crate) const SETTINGS: &str = "preferences_settings";

/// The store's local-state key for cached `(members, apps)` counts per channel (JSON).
pub(crate) const CHANNEL_COUNTS_CACHE: &str = "channel_counts_cache";

/// The store's local-state key for per-conversation composer drafts (JSON).
const DRAFTS: &str = "drafts";

/// The store's local-state key for recently/frequently used emoji shortcodes (JSON).
const FREQUENT_EMOJI: &str = "frequent_emoji";

/// The store's local-state key for the chosen emoji skin tone (1..=6).
const EMOJI_TONE: &str = "emoji_tone";

/// The store's local-state key for the user's preferred workspace rail order (JSON array of TeamId strings).
const WORKSPACE_ORDER: &str = "workspace_order";

/// Receives core events and re-emits them to subscribed views on the UI thread. The bridge
/// emits through this entity so subscribers can use gpui's native `subscribe` machinery.
struct EventHub;

impl EventEmitter<Event> for EventHub {}

/// Receives high-frequency native huddle media frames (60 fps video, audio levels) on a dedicated
/// bounded lane completely isolated from [`EventHub`] so media traffic can never delay or back up
/// control / store-sync events.
struct HuddleMediaHub;

impl EventEmitter<HuddleMediaEvent> for HuddleMediaHub {}

/// Test controller paired with [`CoreBridge::test_harness`]: captures every outgoing [`Command`]
/// sent by GUI views and lets tests push [`Event`]s / [`HuddleMediaEvent`]s through the real GPUI
/// subscription hubs while sharing the real SQLite [`SharedStore`].
#[cfg(test)]
pub(crate) struct TestCoreHandle {
    commands: Mutex<futures::channel::mpsc::UnboundedReceiver<Command>>,
    hub: Entity<EventHub>,
    media_hub: Entity<HuddleMediaHub>,
    store: SharedStore,
}

#[cfg(test)]
impl TestCoreHandle {
    /// Drain every [`Command`] sent by the GUI since the previous call.
    pub(crate) fn drain_commands(&self) -> Vec<Command> {
        let mut rx = self.commands.lock().expect("test command receiver lock");
        let mut out = Vec::new();
        while let Ok(cmd) = rx.try_recv() {
            out.push(cmd);
        }
        out
    }

    /// Emit a core [`Event`] through the real GPUI `EventHub` entity.
    pub(crate) fn emit(&self, event: Event, cx: &mut gpui::App) {
        self.hub.update(cx, |_, cx| cx.emit(event));
    }

    /// Emit a high-frequency [`HuddleMediaEvent`] through the real GPUI `HuddleMediaHub` entity.
    pub(crate) fn emit_media(&self, event: HuddleMediaEvent, cx: &mut gpui::App) {
        self.media_hub.update(cx, |_, cx| cx.emit(event));
    }

    /// Access the shared SQLite store backing this workspace.
    pub(crate) fn store(&self) -> &SharedStore {
        &self.store
    }
}

pub struct CoreBridge {
    /// Live mode: forwards commands into the core's command loop. Fixture mode: `None`.
    commands: Option<UnboundedSender<Command>>,
    /// Live mode: the core runtime thread handle, joined during graceful quit so active huddles
    /// flush Chime `LEAVE` before the process terminates.
    core_thread: Option<Arc<Mutex<Option<std::thread::JoinHandle<()>>>>>,
    /// Live mode: the store the core keeps current (writer connection). Fixture mode: `None`.
    store: Option<SharedStore>,
    /// Live mode: read-only WAL connections, so reads never wait on the writer's transactions.
    /// Empty in memory, where reads fall back to `store`.
    readers: Readers,
    /// Live mode: the signed-in session the core runs with.
    session: Option<Session>,
    /// Live mode: authenticated HTTP fetcher for remote images and file downloads.
    fetcher: Option<slackless_core::Fetcher>,
    /// Created by [`CoreBridge::start`], or lazily by [`CoreBridge::subscribe_in`] in fixture
    /// mode (where nothing will ever emit on it).
    hub: Option<Entity<EventHub>>,
    /// Created by [`CoreBridge::start`], or lazily by [`CoreBridge::subscribe_huddle_media_in`]
    /// in fixture mode.
    media_hub: Option<Entity<HuddleMediaHub>>,
}

impl CoreBridge {
    /// A bridge with no core behind it (fixture mode).
    pub fn offline() -> Self {
        Self {
            commands: None,
            core_thread: None,
            store: None,
            readers: Readers::default(),
            session: None,
            fetcher: None,
            hub: None,
            media_hub: None,
        }
    }

    /// A live bridge over `store` with no core behind it (commands are dropped), for tests of
    /// the store-backed paths.
    #[cfg(test)]
    pub(crate) fn with_store(session: Session, store: SharedStore) -> Self {
        Self {
            store: Some(store),
            session: Some(session),
            ..Self::offline()
        }
    }

    /// A live bridge over the archive file at `path`, opened like [`CoreBridge::start`] opens
    /// one but with no core behind it, for tests of which connection each read takes.
    #[cfg(test)]
    pub(crate) fn with_store_file(session: Session, path: &Path) -> Self {
        Self {
            store: Some(Arc::new(Mutex::new(
                Store::open(path).expect("open the test archive"),
            ))),
            readers: Readers::open(path),
            session: Some(session),
            ..Self::offline()
        }
    }

    /// Fully wired in-process test bridge: captures every outgoing [`Command`] sent by the GUI
    /// and exposes real GPUI [`EventHub`] / [`HuddleMediaHub`] emitters plus the real SQLite
    /// `store`, so `#[gpui::test]` suites verify true `UI -> Command -> Store/Event -> UI` loops
    /// instead of calling internal methods directly on an offline stub.
    #[cfg(test)]
    pub(crate) fn test_harness(
        session: Session,
        store: SharedStore,
        cx: &mut gpui::App,
    ) -> (Self, TestCoreHandle) {
        let (cmd_tx, cmd_rx) = futures::channel::mpsc::unbounded();
        let hub = cx.new(|_| EventHub);
        let media_hub = cx.new(|_| HuddleMediaHub);
        let bridge = Self {
            commands: Some(cmd_tx),
            core_thread: None,
            store: Some(Arc::clone(&store)),
            readers: Readers::default(),
            session: Some(session),
            fetcher: None,
            hub: Some(hub.clone()),
            media_hub: Some(media_hub.clone()),
        };
        let handle = TestCoreHandle {
            commands: Mutex::new(cmd_rx),
            hub,
            media_hub,
            store,
        };
        (bridge, handle)
    }

    /// Starts the core live for `session`: opens its durable store and starts slackless-core on
    /// its dedicated tokio thread (guide section 13). Events flow into gpui through an async
    /// stream wakeup (zero polling latency, zero idle wakeups); views receive them via
    /// [`CoreBridge::subscribe_in`]. High-frequency huddle video/audio frames drain on an
    /// independent coalescing loop via [`CoreBridge::subscribe_huddle_media_in`]. `None` when the
    /// store or the core cannot be started.
    pub fn start(session: Session, cx: &mut gpui::App) -> Option<Self> {
        let (store, readers) = open_store(&session)?;
        let handle = match Client::start(session.clone(), store) {
            Ok(handle) => handle,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "could not start the core; running without a core connection"
                );
                return None;
            }
        };
        let hub = cx.new(|_| EventHub);
        let media_hub = cx.new(|_| HuddleMediaHub);
        let ClientHandles {
            commands,
            mut events,
            mut huddle_media,
            store,
            fetcher,
            thread,
        } = handle.into_parts();
        let bridge = Self {
            commands: Some(commands),
            core_thread: Some(Arc::new(Mutex::new(Some(thread)))),
            store: Some(store),
            readers,
            session: Some(session),
            fetcher: Some(fetcher),
            hub: Some(hub.clone()),
            media_hub: Some(media_hub.clone()),
        };

        // Detached: await control/store events directly via StreamExt (0ms wakeup, 0 idle CPU
        // polling) and drain any queued burst in the same tick.
        cx.spawn(async move |cx| {
            while let Some(event) = events.next().await {
                cx.update_entity(&hub, |_, cx| cx.emit(event));
                while let Ok(next_ev) = events.try_recv() {
                    cx.update_entity(&hub, |_, cx| cx.emit(next_ev));
                }
            }
        })
        .detach();

        // Dedicated high-frequency huddle media lane: coalesces queued VideoFrame / AudioLevels
        // updates within the same UI tick so 60 fps frames never lag or share the Event queue.
        cx.spawn(async move |cx| {
            while let Some(first) = huddle_media.next().await {
                let mut latest_audio = None;
                let mut video_ops: HashMap<
                    slackless_core::huddle::VideoTileKey,
                    Option<slackless_core::huddle::VideoFrame>,
                > = HashMap::new();
                let mut push = |ev: HuddleMediaEvent| match ev {
                    HuddleMediaEvent::VideoFrame { tile, frame } => {
                        video_ops.insert(tile, Some(frame));
                    }
                    HuddleMediaEvent::VideoTileClosed(tile) => {
                        video_ops.insert(tile, None);
                    }
                    HuddleMediaEvent::AudioLevels(levels) => {
                        latest_audio = Some(levels);
                    }
                };
                push(first);
                while let Ok(next_ev) = huddle_media.try_recv() {
                    push(next_ev);
                }
                cx.update_entity(&media_hub, |_, cx| {
                    for (tile, frame) in video_ops {
                        match frame {
                            Some(frame) => cx.emit(HuddleMediaEvent::VideoFrame { tile, frame }),
                            None => cx.emit(HuddleMediaEvent::VideoTileClosed(tile)),
                        }
                    }
                    if let Some(levels) = latest_audio {
                        cx.emit(HuddleMediaEvent::AudioLevels(levels));
                    }
                });
            }
        })
        .detach();

        Some(bridge)
    }

    /// Return a blocking closure that sends [`Command::Shutdown`] and joins this workspace's core
    /// thread so any active huddle flushes its Chime `LEAVE` before process exit.
    pub fn shutdown_task(&self) -> Option<Box<dyn FnOnce() + Send>> {
        let commands = self.commands.clone()?;
        let thread_slot = self.core_thread.clone()?;
        Some(Box::new(move || {
            let _ = commands.unbounded_send(Command::Shutdown);
            if let Some(handle) = thread_slot.lock().ok().and_then(|mut g| g.take()) {
                let _ = handle.join();
            }
        }))
    }

    /// The connection for reads on the UI thread.
    fn read_store(&self) -> Option<&SharedStore> {
        self.readers.ui.as_ref().or(self.store.as_ref())
    }

    /// The connection for reads off the UI thread, apart from [`Self::read_store`] so a slow
    /// one (a snapshot reload, a search, an export) never holds up a read the UI thread waits
    /// on.
    fn background_store(&self) -> Option<&SharedStore> {
        self.readers
            .background
            .as_ref()
            .or_else(|| self.read_store())
    }

    /// Lightweight startup read of just this workspace's `Team` record (1 PK query) so background
    /// workspaces populate the workspace rail without building full snapshots before frame 0.
    pub fn team_summary(&self) -> Option<Team> {
        let session = self.session.as_ref()?;
        let team = &session.team_id;
        let domain = session
            .workspace_url
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .trim_end_matches(".slack.com")
            .to_owned();
        let stored = self
            .read_store()
            .and_then(|s| s.lock().ok())
            .and_then(|s| s.team(team).ok().flatten())
            .filter(|t| !t.name.is_empty());
        Some(stored.unwrap_or_else(|| Team {
            id: team.clone(),
            name: domain.clone(),
            domain,
            ..Default::default()
        }))
    }

    /// Read all `local_state` keys for this workspace in a single SQLite query.
    pub fn all_local(&self) -> HashMap<String, String> {
        let Some(team) = self.session.as_ref().map(|s| &s.team_id) else {
            return HashMap::new();
        };
        let Some(store) = self.read_store().and_then(|s| s.lock().ok()) else {
            return HashMap::new();
        };
        store.all_local(team).unwrap_or_default()
    }

    /// Route GPUI remote image fetches (`img(url)`) and file downloads through this
    /// workspace's authenticated HTTP fetcher. Call at launch for workspace 0 and whenever
    /// switching active live workspaces.
    pub fn activate_media(&self, cx: &mut gpui::App) {
        if let Some(fetcher) = &self.fetcher {
            cx.set_http_client(Arc::new(CoreHttp(fetcher.clone())));
            crate::media::enable_downloads(fetcher.clone(), cx);
        }
    }

    /// Lightweight O(1) lookup for handling a single `Event::MessagePosted` in a background
    /// (parked) workspace without deserializing the entire workspace snapshot on the UI thread.
    pub fn bg_message_context(
        &self,
        channel: &ChannelId,
        sender_id: Option<&UserId>,
    ) -> Option<BgMessageContext> {
        let session = self.session.as_ref()?;
        let team = &session.team_id;
        let store = self.read_store()?.lock().ok()?;
        let conversation = store.conversation(team, channel).ok().flatten();
        let sender = sender_id.and_then(|u| store.user(team, u).ok().flatten());
        Some(BgMessageContext {
            self_user: session.user_id.clone(),
            conversation,
            sender,
        })
    }

    /// Live mode: the signed-in workspace as the store has it now, with Slack's latest read
    /// state (`counts`). `None` offline.
    pub fn snapshot(&self, counts: &HashMap<ChannelId, ConversationCounts>) -> Option<Fixtures> {
        let session = self.session.as_ref()?;
        let store = self.read_store()?.lock().ok()?;
        Some(fixtures::live(session, &store, counts))
    }

    /// Live mode: build a fresh store snapshot off the UI thread on the background connection.
    pub fn snapshot_async(
        &self,
        counts: HashMap<ChannelId, ConversationCounts>,
    ) -> Option<impl Future<Output = Option<Fixtures>> + Send + 'static + use<>> {
        let session = self.session.clone()?;
        let store = self.background_store()?.clone();
        Some(async move {
            let store = store.lock().ok()?;
            Some(fixtures::live(&session, &store, &counts))
        })
    }

    /// Live mode: what a first visit shows at once while [`CoreBridge::snapshot_async`] reads
    /// the workspace off the UI thread ([`fixtures::placeholder`], one stored row). `None`
    /// offline.
    pub fn placeholder(&self, counts: &HashMap<ChannelId, ConversationCounts>) -> Option<Fixtures> {
        let session = self.session.as_ref()?;
        let store = self.read_store()?.lock().ok()?;
        Some(fixtures::placeholder(session, &store, counts))
    }

    /// Live mode: a read of the stored records of `users` (a primary-key lookup each) to run
    /// off the UI thread; people the store lacks are left out. `None` offline.
    pub fn users(
        &self,
        users: Vec<UserId>,
    ) -> Option<impl Future<Output = Vec<User>> + Send + 'static + use<>> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.background_store()?.clone();
        Some(async move {
            let Ok(store) = store.lock() else {
                return Vec::new();
            };
            users
                .iter()
                .filter_map(|id| {
                    store.user(&team, id).unwrap_or_else(|e| {
                        tracing::warn!(
                            user = %id,
                            error = %e,
                            "could not read a person from the local store"
                        );
                        None
                    })
                })
                .collect()
        })
    }

    /// Synchronous single-page read of `channel`'s newest stored messages for cold-launch
    /// frame-0 seeding (bounded to `INITIAL_HISTORY_LIMIT` so cold launch stays sub-millisecond).
    /// Locally preserved deleted messages are always included (tagged with [`DELETED_EXTRA_KEY`])
    /// so the UI can render either a minimal clickable row or the expanded message.
    pub fn history_sync(&self, channel: &ChannelId) -> Vec<Message> {
        let Some(session) = self.session.as_ref() else {
            return Vec::new();
        };
        let Some(store) = self.read_store().and_then(|s| s.lock().ok()) else {
            return Vec::new();
        };
        match store.channel_history(&session.team_id, channel, None, INITIAL_HISTORY_LIMIT) {
            Ok(rows) => rows.into_iter().map(row_to_message).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Synchronous read of all persisted V13 outbox items for this workspace (FIFO order).
    pub fn outbox_sync(&self) -> Vec<slackless_core::store::OutboxItem> {
        let Some(team) = self.session.as_ref().map(|s| &s.team_id) else {
            return Vec::new();
        };
        let Some(store) = self.read_store().and_then(|s| s.lock().ok()) else {
            return Vec::new();
        };
        store.list_outbox(team).unwrap_or_default()
    }

    /// Signed-in user ID for this live workspace, if any.
    pub fn self_user_id(&self) -> Option<&UserId> {
        self.session.as_ref().map(|s| &s.user_id)
    }

    /// Persist the user's latest Catch-Up visit timestamp in SQLite `local_state` off the UI thread.
    pub fn record_catchup_visit(
        &self,
        now_secs: i64,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.store.as_ref()?.clone();
        Some(async move {
            if let Ok(mut guard) = store.lock() {
                let _ = slackless_core::catchup::record_catchup_visit(&mut guard, &team, now_secs);
            }
        })
    }

    /// Live mode: a read of `channel`'s stored history (oldest first) to run off the UI thread:
    /// the newest `HISTORY_LIMIT` messages, or the `HISTORY_LIMIT` before `before` when paging
    /// older on scroll-back. Locally preserved deleted messages are tagged with
    /// [`DELETED_EXTRA_KEY`] so the UI can show a minimal toggle row or expand them inline.
    /// `None` offline.
    pub fn history(
        &self,
        channel: ChannelId,
        before: Option<Ts>,
    ) -> Option<impl Future<Output = Vec<Message>> + Send + 'static + use<>> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.background_store()?.clone();
        Some(async move {
            let Ok(store) = store.lock() else {
                return Vec::new();
            };
            match store.channel_history(&team, &channel, before.as_ref(), HISTORY_LIMIT) {
                Ok(rows) => rows.into_iter().map(row_to_message).collect(),
                Err(e) => {
                    tracing::warn!(%channel, error = %e, "could not read channel history from local store");
                    Vec::new()
                }
            }
        })
    }

    /// Live mode: a read of `channel`'s newest undeleted top-level message (the DM list's
    /// preview) to run off the UI thread. `None` offline.
    pub fn latest(
        &self,
        channel: ChannelId,
    ) -> Option<impl Future<Output = Option<Message>> + Send + 'static> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.background_store()?.clone();
        Some(async move {
            let store = store.lock().ok()?;
            match store.latest_messages(&team, std::slice::from_ref(&channel)) {
                Ok(mut latest) => latest.remove(&channel).map(|stored| stored.message),
                Err(e) => {
                    tracing::warn!(%channel, error = %e, "could not read channel latest message from local store");
                    None
                }
            }
        })
    }

    /// Live mode: a read of up to `limit` newest undeleted messages authored by `user` across
    /// all stored conversations (for Drafts & sent -> Sent). `None` offline.
    pub fn recent_sent(
        &self,
        user: slackless_core::types::UserId,
        limit: usize,
    ) -> Option<impl Future<Output = Vec<slackless_core::store::StoredMessage>> + Send + 'static>
    {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.background_store()?.clone();
        Some(async move {
            let Ok(store) = store.lock() else {
                return Vec::new();
            };
            store
                .recent_messages_by_user(&team, &user, limit)
                .unwrap_or_default()
        })
    }

    /// Live mode: a read of one thread's stored history (root + replies, oldest first)
    /// to run off the UI thread. Locally preserved deleted messages are tagged with
    /// [`DELETED_EXTRA_KEY`] so the thread panel can show a minimal toggle row or expand them.
    /// `None` offline.
    pub fn thread(
        &self,
        channel: ChannelId,
        thread_ts: Ts,
    ) -> Option<impl Future<Output = Vec<Message>> + Send + 'static> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.background_store()?.clone();
        Some(async move {
            let Ok(store) = store.lock() else {
                return Vec::new();
            };
            match store.thread_replies(&team, &channel, &thread_ts) {
                Ok(rows) => rows.into_iter().map(row_to_message).collect(),
                Err(e) => {
                    tracing::warn!(%channel, %thread_ts, error = %e, "could not read thread from local store");
                    Vec::new()
                }
            }
        })
    }

    /// Forward a command to the core. Offline, commands are dropped by design.
    pub fn send(&self, command: impl Into<Command>) {
        if let Some(commands) = &self.commands
            && let Err(e) = commands.unbounded_send(command.into())
        {
            tracing::warn!(error = %e, "core has stopped; command dropped");
        }
    }

    /// Clone a lightweight command forwarder for popped-out secondary GPUI windows.
    pub fn forwarder(&self) -> Arc<dyn Fn(Command) + Send + Sync> {
        match self.commands.clone() {
            Some(commands) => Arc::new(move |command| {
                if let Err(e) = commands.unbounded_send(command) {
                    tracing::warn!(error = %e, "core has stopped; forwarded command dropped");
                }
            }),
            None => Arc::new(|_| {}),
        }
    }

    /// The store the core keeps current (live mode); views re-read it to render. `None` in
    /// fixture mode.
    pub fn store(&self) -> Option<&SharedStore> {
        self.store.as_ref()
    }

    /// A clone of the connection for reads off the UI thread (the writer when the store is in
    /// memory), so background queries never hold up a UI-thread read or the core's writes.
    pub fn read_store_clone(&self) -> Option<SharedStore> {
        self.background_store().cloned()
    }

    pub fn team_id(&self) -> Option<&slackless_core::types::TeamId> {
        self.session.as_ref().map(|s| &s.team_id)
    }

    /// The conversation last open in this workspace, restored at launch like Slack. `None`
    /// offline or before the first one.
    pub fn last_conversation(&self) -> Option<ChannelId> {
        let id = self.local(LAST_CONVERSATION)?;
        Some(ChannelId::from(id.as_str()))
    }

    /// Live mode: a write remembering `channel` as the open conversation, to run off the UI
    /// thread. `None` offline.
    pub fn remember_conversation(
        &self,
        channel: ChannelId,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(
            LAST_CONVERSATION,
            channel.as_str().to_owned(),
            "open conversation",
        )
    }

    /// Live mode: a write remembering the window's native state (`Window::native_window_state`),
    /// to run off the UI thread. `None` offline.
    pub fn remember_window_state(
        &self,
        state: Vec<u8>,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        let hex = state.iter().map(|byte| format!("{byte:02x}")).collect();
        self.write_local(WINDOW_STATE, hex, "window frame")
    }

    /// Live mode: a write remembering `mode` as the Color Mode, to run off the UI thread.
    /// `None` offline.
    pub fn remember_color_mode(
        &self,
        mode: AppearanceMode,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(COLOR_MODE, mode.key().to_owned(), "color mode")
    }

    /// Live mode: a write remembering the search history, to run off the UI thread. `None`
    /// offline.
    pub fn remember_search_history(
        &self,
        queries: &[String],
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(
            SEARCH_HISTORY,
            serde_json::to_string(queries).ok()?,
            "search history",
        )
    }

    /// Live mode: a write remembering the history toggle, to run off the UI thread. `None`
    /// offline.
    pub fn remember_search_history_shown(
        &self,
        on: bool,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(SEARCH_HISTORY_SHOWN, on.to_string(), "history toggle")
    }

    /// Live mode: a write remembering the filter toggle, to run off the UI thread. `None`
    /// offline.
    pub fn remember_search_filters_shown(
        &self,
        on: bool,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(SEARCH_FILTERS_SHOWN, on.to_string(), "filter toggle")
    }

    /// Live mode: a write remembering the recently visited conversations, to run off the UI
    /// thread. `None` offline.
    pub fn remember_recent_places(
        &self,
        ids: &[ChannelId],
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(
            RECENT_PLACES,
            serde_json::to_string(ids).ok()?,
            "recent places",
        )
    }

    /// Live mode: a write remembering [`theme::FONT_LABELS`]`[font]` as the Font, to run off
    /// the UI thread. `None` offline.
    pub fn remember_font(&self, font: usize) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(FONT, theme::FONT_FAMILIES[font].to_owned(), "font")
    }

    /// Live mode: a write remembering zoom level `level`, to run off the UI thread. `None`
    /// offline.
    pub fn remember_zoom(&self, level: usize) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(ZOOM, level.to_string(), "zoom")
    }

    /// The Preferences settings saved locally; `None` offline or before the first save.
    pub fn settings(&self) -> Option<Settings> {
        self.store.as_ref()?;
        serde_json::from_str(&self.local(SETTINGS)?).ok()
    }

    /// Live mode: persist Preferences `settings` locally, off the UI thread. `None` offline.
    pub fn remember_settings(
        &self,
        settings: &Settings,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(
            SETTINGS,
            serde_json::to_string(settings).ok()?,
            "preferences",
        )
    }

    /// Saved per-conversation composer drafts for this workspace; `None` offline or unset.
    pub fn drafts(&self) -> Option<Vec<SavedDraft>> {
        self.store.as_ref()?;
        serde_json::from_str(&self.local(DRAFTS)?).ok()
    }

    /// Live mode: persist per-conversation composer drafts off the UI thread. `None` offline.
    pub fn remember_drafts(
        &self,
        drafts: &[SavedDraft],
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(DRAFTS, serde_json::to_string(drafts).ok()?, "drafts")
    }

    /// Live mode: persist frequently used emoji shortcodes off the UI thread. `None` offline.
    pub fn remember_frequent_emoji(
        &self,
        list: &[String],
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(
            FREQUENT_EMOJI,
            serde_json::to_string(list).ok()?,
            "frequent emoji",
        )
    }

    /// Live mode: persist emoji skin tone (1..=6) off the UI thread. `None` offline.
    pub fn remember_emoji_tone(
        &self,
        tone: u8,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(EMOJI_TONE, tone.to_string(), "emoji skin tone")
    }

    /// Saved workspace switcher order (`TeamId` strings); `None` offline or unset.
    pub fn workspace_order(&self) -> Option<Vec<String>> {
        self.store.as_ref()?;
        serde_json::from_str(&self.local(WORKSPACE_ORDER)?).ok()
    }

    /// Live mode: persist workspace switcher order off the UI thread. `None` offline.
    pub fn remember_workspace_order(
        &self,
        team_ids: &[String],
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(
            WORKSPACE_ORDER,
            serde_json::to_string(team_ids).ok()?,
            "workspace order",
        )
    }

    /// Saved per-section sidebar filter and sort choices; `None` offline or unset.
    pub fn section_prefs(&self) -> Option<SectionPrefs> {
        self.store.as_ref()?;
        serde_json::from_str(&self.local(SECTION_PREFS)?).ok()
    }

    /// Live mode: persist per-section sidebar filter & sort choices off the UI thread.
    pub fn remember_section_prefs(
        &self,
        prefs: &SectionPrefs,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(
            SECTION_PREFS,
            serde_json::to_string(prefs).ok()?,
            "sidebar section preferences",
        )
    }

    /// This workspace's stored local-state `key`; `None` offline or when unset.
    fn local(&self, key: &str) -> Option<String> {
        let team = &self.session.as_ref()?.team_id;
        let store = self.read_store()?.lock().ok()?;
        store.local(team, key).ok().flatten()
    }

    /// Parse helpers over a pre-loaded `all_local()` map (zero extra SQLite round-trips at launch).
    pub fn parse_last_conversation(local: &HashMap<String, String>) -> Option<ChannelId> {
        local
            .get(LAST_CONVERSATION)
            .map(|id| ChannelId::from(id.as_str()))
    }

    pub fn parse_window_state(local: &HashMap<String, String>) -> Option<Vec<u8>> {
        let hex = local.get(WINDOW_STATE)?;
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
            .collect()
    }

    pub fn parse_color_mode(&self, local: &HashMap<String, String>) -> Option<AppearanceMode> {
        self.store.as_ref()?;
        Some(
            local
                .get(COLOR_MODE)
                .map(String::as_str)
                .and_then(AppearanceMode::from_key)
                .unwrap_or(AppearanceMode::System),
        )
    }

    pub fn parse_font(local: &HashMap<String, String>) -> Option<usize> {
        let family = local.get(FONT)?;
        theme::FONT_FAMILIES
            .iter()
            .position(|known| *known == family.as_str())
    }

    pub fn parse_zoom(local: &HashMap<String, String>) -> Option<usize> {
        let level: usize = local.get(ZOOM)?.parse().ok()?;
        (level < crate::zoom::FACTORS.len()).then_some(level)
    }

    pub fn parse_settings(local: &HashMap<String, String>) -> Option<Settings> {
        serde_json::from_str(local.get(SETTINGS)?).ok()
    }

    pub fn parse_search_history(local: &HashMap<String, String>) -> Option<Vec<String>> {
        serde_json::from_str(local.get(SEARCH_HISTORY)?).ok()
    }

    pub fn parse_search_history_shown(local: &HashMap<String, String>) -> Option<bool> {
        local.get(SEARCH_HISTORY_SHOWN)?.parse().ok()
    }

    pub fn parse_search_filters_shown(local: &HashMap<String, String>) -> Option<bool> {
        local.get(SEARCH_FILTERS_SHOWN)?.parse().ok()
    }

    pub fn parse_recent_places(local: &HashMap<String, String>) -> Option<Vec<ChannelId>> {
        serde_json::from_str(local.get(RECENT_PLACES)?).ok()
    }

    pub fn parse_drafts(local: &HashMap<String, String>) -> Option<Vec<SavedDraft>> {
        serde_json::from_str(local.get(DRAFTS)?).ok()
    }

    /// Live mode: a write remembering cached `(humans, apps)` member counts per channel so
    /// cold launches paint header member/app badges immediately on frame 0.
    pub fn remember_channel_counts(
        &self,
        members: &HashMap<ChannelId, u32>,
        apps: &HashMap<ChannelId, u32>,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        let mut combined: HashMap<String, (Option<u32>, Option<u32>)> = HashMap::new();
        for (ch, &n) in members {
            combined.entry(ch.to_string()).or_default().0 = Some(n);
        }
        for (ch, &a) in apps {
            combined.entry(ch.to_string()).or_default().1 = Some(a);
        }
        self.write_local(
            CHANNEL_COUNTS_CACHE,
            serde_json::to_string(&combined).ok()?,
            "channel counts cache",
        )
    }

    pub fn parse_channel_counts(
        local: &HashMap<String, String>,
    ) -> (HashMap<ChannelId, u32>, HashMap<ChannelId, u32>) {
        let mut members = HashMap::new();
        let mut apps = HashMap::new();
        let Some(raw) = local.get(CHANNEL_COUNTS_CACHE) else {
            return (members, apps);
        };
        let Ok(parsed) = serde_json::from_str::<HashMap<String, (Option<u32>, Option<u32>)>>(raw)
        else {
            return (members, apps);
        };
        for (ch, (m, a)) in parsed {
            let id = ChannelId::from(ch);
            if let Some(n) = m {
                members.insert(id.clone(), n);
            }
            if let Some(n) = a {
                apps.insert(id, n);
            }
        }
        (members, apps)
    }

    pub fn parse_frequent_emoji(local: &HashMap<String, String>) -> Option<Vec<String>> {
        serde_json::from_str(local.get(FREQUENT_EMOJI)?).ok()
    }

    pub fn parse_emoji_tone(local: &HashMap<String, String>) -> Option<u8> {
        let tone: u8 = local.get(EMOJI_TONE)?.parse().ok()?;
        (1..=6).contains(&tone).then_some(tone)
    }

    /// A write storing `value` under this workspace's local-state `key`, to run off the UI
    /// thread; a failure is logged naming `what` was not remembered. `None` offline.
    fn write_local(
        &self,
        key: &'static str,
        value: String,
        what: &'static str,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.store.clone()?;
        Some(async move {
            let Ok(mut store) = store.lock() else {
                return;
            };
            if let Err(e) = store.set_local(&team, key, &value) {
                tracing::warn!(what, error = %e, "could not persist local state");
            }
        })
    }

    /// Subscribe to the core's [`Event`]s. The handler runs on the UI thread whenever the
    /// drain task forwards an event; keep the returned [`Subscription`] alive or the
    /// subscription dies (guide section 2).
    pub fn subscribe_in<T: 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<T>,
        mut on_event: impl FnMut(&mut T, &Event, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let hub = self.hub.get_or_insert_with(|| cx.new(|_| EventHub));
        cx.subscribe_in(hub, window, move |this, _, event: &Event, window, cx| {
            on_event(this, event, window, cx);
        })
    }

    /// Subscribe to high-frequency native huddle media events (`VideoFrame`, `VideoTileClosed`,
    /// `AudioLevels`) drained on their own coalescing lane, separate from [`Event`].
    pub fn subscribe_huddle_media_in<T: 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<T>,
        mut on_event: impl FnMut(&mut T, &HuddleMediaEvent, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let hub = self
            .media_hub
            .get_or_insert_with(|| cx.new(|_| HuddleMediaHub));
        cx.subscribe_in(
            hub,
            window,
            move |this, _, event: &HuddleMediaEvent, window, cx| {
                on_event(this, event, window, cx);
            },
        )
    }
}

/// Read-only WAL connections to a workspace archive, one per kind of caller, so a read the UI
/// thread waits on never queues behind a background read, which may take as long as it needs.
#[derive(Default)]
struct Readers {
    /// Reads on the UI thread.
    ui: Option<SharedStore>,
    /// Reads off the UI thread.
    background: Option<SharedStore>,
}

impl Readers {
    /// Opens both connections to the archive at `path`; one that fails to open falls back to
    /// the writer.
    fn open(path: &Path) -> Self {
        let open = || match Store::open_read_only(path) {
            Ok(reader) => Some(Arc::new(Mutex::new(reader))),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "could not open a read-only connection; reads share the writer"
                );
                None
            }
        };
        Self {
            ui: open(),
            background: open(),
        }
    }
}

/// Opens the durable store for a session plus its read-only connections ([`Readers`]). Falls
/// back to an in-memory store, read through the writer, when no home directory is available.
fn open_store(session: &Session) -> Option<(Store, Readers)> {
    match default_store_path(&session.team_id) {
        Some(path) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match Store::open(&path) {
                Ok(store) => Some((store, Readers::open(&path))),
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "could not open local store; running without a core connection"
                    );
                    None
                }
            }
        }
        None => match Store::open_in_memory() {
            Ok(store) => Some((store, Readers::default())),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "could not open an in-memory store; running without a core connection"
                );
                None
            }
        },
    }
}

/// The durable store lives under the platform's application-data directory, one database per
/// workspace (the store location stays a front-end decision, per slackless-core's docs).
fn default_store_path(team_id: &TeamId) -> Option<PathBuf> {
    slackless_core::paths::archive_store_path(team_id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar_menu::{SectionFilter, SectionSort};

    #[test]
    fn saved_draft_and_section_prefs_json_round_trip() {
        let draft = SavedDraft {
            channel: ChannelId::from("C123"),
            label: "# general".into(),
            text: "Hello *world*".into(),
            at: 1_760_000_000,
        };
        let encoded = serde_json::to_string(&draft).unwrap();
        let decoded: SavedDraft = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, draft);

        let mut prefs = SectionPrefs::default();
        prefs
            .filters
            .insert("channels".into(), SectionFilter::Unreads);
        prefs.sorts.insert("channels".into(), SectionSort::Priority);
        let encoded = serde_json::to_string(&prefs).unwrap();
        let decoded: SectionPrefs = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, prefs);

        let empty: SectionPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, SectionPrefs::default());
    }

    #[test]
    fn offline_bridge_is_a_safe_noop() {
        let bridge = CoreBridge::offline();
        assert!(bridge.store().is_none());
        assert!(bridge.snapshot(&HashMap::new()).is_none());
        assert!(bridge.history(ChannelId::from("C1"), None).is_none());
        assert!(bridge.settings().is_none());
        assert!(bridge.drafts().is_none());
        assert!(bridge.section_prefs().is_none());
        assert!(bridge.parse_color_mode(&HashMap::new()).is_none());
        bridge.send(slackless_core::MessagingCommand::MarkAllRead);
    }

    fn run_ready<F: Future<Output = ()>>(fut: F) {
        use std::task::{Context, Poll, Waker};
        let mut pinned = std::pin::pin!(fut);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(pinned.as_mut().poll(&mut cx), Poll::Ready(())));
    }

    fn test_session() -> Session {
        Session {
            team_id: TeamId::from("T0TEST"),
            user_id: slackless_core::types::UserId::from("U0ME"),
            workspace_url: "https://acme.slack.com".into(),
            xoxc_token: String::new(),
            d_cookie: String::new(),
        }
    }

    fn memory_bridge() -> CoreBridge {
        let store = Arc::new(Mutex::new(
            Store::open_in_memory().expect("in-memory store"),
        ));
        CoreBridge::with_store(test_session(), store)
    }

    #[test]
    fn ui_thread_reads_do_not_wait_behind_a_background_read() {
        let dir = std::env::temp_dir().join(format!(
            "slackless-gui-bridge-readers-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bridge = CoreBridge::with_store_file(test_session(), &dir.join("T0TEST.sqlite"));
        let channel = ChannelId::from("C0ENG");
        let message = Message {
            ts: Ts::from("1700000000.000100"),
            text: "hello".into(),
            ..Default::default()
        };
        bridge
            .store()
            .unwrap()
            .lock()
            .unwrap()
            .upsert_message(&TeamId::from("T0TEST"), &channel, &message)
            .unwrap();

        // A background read (a snapshot reload, a search, an export) holds its connection.
        let background = bridge.read_store_clone().unwrap();
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let _read = background.lock().unwrap();
            held_tx.send(()).unwrap();
            // `true` once released by the UI-thread read below; the timeout is a circuit
            // breaker so a UI read stuck behind this one fails the test instead of hanging it.
            !matches!(
                done_rx.recv_timeout(std::time::Duration::from_secs(5)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            )
        });
        held_rx.recv().unwrap();
        let history = bridge.history_sync(&channel);
        drop(done_tx);

        assert!(
            holder.join().unwrap(),
            "history_sync waited for the background read to finish"
        );
        assert_eq!(
            history.into_iter().map(|m| m.ts).collect::<Vec<_>>(),
            vec![message.ts]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_decoders_handle_valid_boundary_and_malformed_inputs() {
        let mut local = HashMap::new();

        // parse_last_conversation
        assert_eq!(CoreBridge::parse_last_conversation(&local), None);
        local.insert(LAST_CONVERSATION.into(), "C0ENG".into());
        assert_eq!(
            CoreBridge::parse_last_conversation(&local),
            Some(ChannelId::from("C0ENG"))
        );

        // parse_window_state: valid hex, empty, odd length, non-hex, non-ASCII
        assert_eq!(CoreBridge::parse_window_state(&local), None);
        local.insert(WINDOW_STATE.into(), "007fFF".into());
        assert_eq!(
            CoreBridge::parse_window_state(&local),
            Some(vec![0x00, 0x7f, 0xff])
        );
        local.insert(WINDOW_STATE.into(), String::new());
        assert_eq!(CoreBridge::parse_window_state(&local), Some(vec![]));
        local.insert(WINDOW_STATE.into(), "abc".into());
        assert_eq!(CoreBridge::parse_window_state(&local), None);
        local.insert(WINDOW_STATE.into(), "zzzz".into());
        assert_eq!(CoreBridge::parse_window_state(&local), None);
        local.insert(WINDOW_STATE.into(), "éé".into());
        assert_eq!(CoreBridge::parse_window_state(&local), None);

        // parse_font
        assert_eq!(CoreBridge::parse_font(&local), None);
        local.insert(FONT.into(), theme::FONT_FAMILIES[2].into());
        assert_eq!(CoreBridge::parse_font(&local), Some(2));
        local.insert(FONT.into(), "NonexistentFont".into());
        assert_eq!(CoreBridge::parse_font(&local), None);

        // parse_zoom bounds
        assert_eq!(CoreBridge::parse_zoom(&local), None);
        local.insert(ZOOM.into(), "0".into());
        assert_eq!(CoreBridge::parse_zoom(&local), Some(0));
        local.insert(ZOOM.into(), (crate::zoom::FACTORS.len() - 1).to_string());
        assert_eq!(
            CoreBridge::parse_zoom(&local),
            Some(crate::zoom::FACTORS.len() - 1)
        );
        local.insert(ZOOM.into(), crate::zoom::FACTORS.len().to_string());
        assert_eq!(CoreBridge::parse_zoom(&local), None);
        local.insert(ZOOM.into(), "-1".into());
        assert_eq!(CoreBridge::parse_zoom(&local), None);

        // parse_channel_counts: merge & malformed JSON
        assert_eq!(
            CoreBridge::parse_channel_counts(&local),
            (HashMap::new(), HashMap::new())
        );
        local.insert(
            CHANNEL_COUNTS_CACHE.into(),
            r#"{"C1":[12,3],"C2":[5,null],"C3":[null,2]}"#.into(),
        );
        let (members, apps) = CoreBridge::parse_channel_counts(&local);
        assert_eq!(members.get(&ChannelId::from("C1")), Some(&12));
        assert_eq!(members.get(&ChannelId::from("C2")), Some(&5));
        assert!(!members.contains_key(&ChannelId::from("C3")));
        assert_eq!(apps.get(&ChannelId::from("C1")), Some(&3));
        assert!(!apps.contains_key(&ChannelId::from("C2")));
        assert_eq!(apps.get(&ChannelId::from("C3")), Some(&2));
        local.insert(CHANNEL_COUNTS_CACHE.into(), "{broken".into());
        assert_eq!(
            CoreBridge::parse_channel_counts(&local),
            (HashMap::new(), HashMap::new())
        );

        // parse_emoji_tone 1..=6 bounds
        for valid in 1u8..=6 {
            local.insert(EMOJI_TONE.into(), valid.to_string());
            assert_eq!(CoreBridge::parse_emoji_tone(&local), Some(valid));
        }
        for invalid in ["0", "7", "255", "abc"] {
            local.insert(EMOJI_TONE.into(), invalid.into());
            assert_eq!(CoreBridge::parse_emoji_tone(&local), None, "{invalid}");
        }

        // parse_settings, search_history, toggles, recent_places, drafts, frequent_emoji
        local.insert(SETTINGS.into(), "{bad".into());
        assert!(CoreBridge::parse_settings(&local).is_none());
        local.insert(
            SETTINGS.into(),
            serde_json::to_string(&Settings::default()).unwrap(),
        );
        assert!(CoreBridge::parse_settings(&local).is_some());

        local.insert(SEARCH_HISTORY.into(), r#"["deploy","incident"]"#.into());
        assert_eq!(
            CoreBridge::parse_search_history(&local),
            Some(vec!["deploy".into(), "incident".into()])
        );
        local.insert(SEARCH_HISTORY_SHOWN.into(), "true".into());
        local.insert(SEARCH_FILTERS_SHOWN.into(), "false".into());
        assert_eq!(CoreBridge::parse_search_history_shown(&local), Some(true));
        assert_eq!(CoreBridge::parse_search_filters_shown(&local), Some(false));

        local.insert(RECENT_PLACES.into(), r#"["C0ENG","D0ALICE"]"#.into());
        assert_eq!(
            CoreBridge::parse_recent_places(&local),
            Some(vec![ChannelId::from("C0ENG"), ChannelId::from("D0ALICE")])
        );

        local.insert(FREQUENT_EMOJI.into(), r#"["tada","rocket"]"#.into());
        assert_eq!(
            CoreBridge::parse_frequent_emoji(&local),
            Some(vec!["tada".into(), "rocket".into()])
        );

        local.insert(DRAFTS.into(), "not json".into());
        assert!(CoreBridge::parse_drafts(&local).is_none());
    }

    #[test]
    fn in_memory_store_round_trips_all_remember_and_all_local_decoders() {
        let bridge = memory_bridge();
        let settings = Settings {
            keywords: "deploy, urgent".into(),
            enter_sends: false,
            ..Default::default()
        };

        let drafts = vec![SavedDraft {
            channel: ChannelId::from("C0ENG"),
            label: "# engineering".into(),
            text: "draft *body*".into(),
            at: 1_760_000_123,
        }];
        let mut section_prefs = SectionPrefs::default();
        section_prefs
            .filters
            .insert("channels".into(), SectionFilter::Mentions);
        section_prefs
            .sorts
            .insert("dms".into(), SectionSort::Recency);

        let members = HashMap::from([(ChannelId::from("C0ENG"), 42u32)]);
        let apps = HashMap::from([
            (ChannelId::from("C0ENG"), 4u32),
            (ChannelId::from("C0OPS"), 1u32),
        ]);

        run_ready(
            bridge
                .remember_conversation(ChannelId::from("C0ENG"))
                .unwrap(),
        );
        run_ready(
            bridge
                .remember_window_state(vec![0xde, 0xad, 0xbe, 0xef])
                .unwrap(),
        );
        run_ready(bridge.remember_color_mode(AppearanceMode::Dark).unwrap());
        run_ready(
            bridge
                .remember_search_history(&["alpha".into(), "beta".into()])
                .unwrap(),
        );
        run_ready(bridge.remember_search_history_shown(false).unwrap());
        run_ready(bridge.remember_search_filters_shown(true).unwrap());
        run_ready(
            bridge
                .remember_recent_places(&[ChannelId::from("C0ENG"), ChannelId::from("D0ALICE")])
                .unwrap(),
        );
        run_ready(bridge.remember_font(1).unwrap());
        run_ready(bridge.remember_zoom(4).unwrap());
        run_ready(bridge.remember_settings(&settings).unwrap());
        run_ready(bridge.remember_drafts(&drafts).unwrap());
        run_ready(
            bridge
                .remember_frequent_emoji(&["fire".into(), "100".into()])
                .unwrap(),
        );
        run_ready(bridge.remember_emoji_tone(5).unwrap());
        run_ready(
            bridge
                .remember_workspace_order(&["T2".into(), "T1".into()])
                .unwrap(),
        );
        run_ready(bridge.remember_section_prefs(&section_prefs).unwrap());
        run_ready(bridge.remember_channel_counts(&members, &apps).unwrap());

        let local = bridge.all_local();
        assert_eq!(
            CoreBridge::parse_last_conversation(&local),
            Some(ChannelId::from("C0ENG"))
        );
        assert_eq!(bridge.last_conversation(), Some(ChannelId::from("C0ENG")));
        assert_eq!(
            CoreBridge::parse_window_state(&local),
            Some(vec![0xde, 0xad, 0xbe, 0xef])
        );
        assert_eq!(bridge.parse_color_mode(&local), Some(AppearanceMode::Dark));
        assert_eq!(
            CoreBridge::parse_search_history(&local),
            Some(vec!["alpha".into(), "beta".into()])
        );
        assert_eq!(CoreBridge::parse_search_history_shown(&local), Some(false));
        assert_eq!(CoreBridge::parse_search_filters_shown(&local), Some(true));
        assert_eq!(
            CoreBridge::parse_recent_places(&local),
            Some(vec![ChannelId::from("C0ENG"), ChannelId::from("D0ALICE")])
        );
        assert_eq!(CoreBridge::parse_font(&local), Some(1));
        assert_eq!(CoreBridge::parse_zoom(&local), Some(4));
        let decoded_settings = CoreBridge::parse_settings(&local).unwrap();
        assert_eq!(decoded_settings.keywords, "deploy, urgent");
        assert!(!decoded_settings.enter_sends);
        assert_eq!(bridge.settings().unwrap().keywords, "deploy, urgent");
        assert_eq!(CoreBridge::parse_drafts(&local), Some(drafts.clone()));
        assert_eq!(bridge.drafts(), Some(drafts));
        assert_eq!(
            CoreBridge::parse_frequent_emoji(&local),
            Some(vec!["fire".into(), "100".into()])
        );
        assert_eq!(CoreBridge::parse_emoji_tone(&local), Some(5));
        assert_eq!(
            bridge.workspace_order(),
            Some(vec!["T2".into(), "T1".into()])
        );
        assert_eq!(bridge.section_prefs(), Some(section_prefs));
        assert_eq!(CoreBridge::parse_channel_counts(&local), (members, apps));
    }
}
