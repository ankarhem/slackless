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

/// A `local_state` key: the string [`CoreBridge::remember`] writes from a `W`, the `R` a read
/// decodes back, and `what` it names in the log line of a failed write.
pub(crate) struct LocalKey<W: ?Sized, R> {
    key: &'static str,
    what: &'static str,
    encode: fn(&W) -> Option<String>,
    decode: fn(&str) -> Option<R>,
}

impl<W: ?Sized, R> LocalKey<W, R> {
    const fn new(
        key: &'static str,
        what: &'static str,
        encode: fn(&W) -> Option<String>,
        decode: fn(&str) -> Option<R>,
    ) -> Self {
        Self {
            key,
            what,
            encode,
            decode,
        }
    }

    /// The store's key string (tests seed a `local` map by it).
    #[cfg(test)]
    pub(crate) fn name(&self) -> &'static str {
        self.key
    }

    /// The key's value in `local`, decoded; `None` when unset or malformed.
    pub(crate) fn read(&self, local: &HashMap<String, String>) -> Option<R> {
        local.get(self.key).and_then(|raw| (self.decode)(raw))
    }

    /// The key's stored string as is.
    pub(crate) fn raw<'a>(&self, local: &'a HashMap<String, String>) -> Option<&'a str> {
        local.get(self.key).map(String::as_str)
    }
}

/// JSON codecs for [`LocalKey`] values.
fn json_encode<T: serde::Serialize + ?Sized>(value: &T) -> Option<String> {
    serde_json::to_string(value).ok()
}

fn json_decode<T: serde::de::DeserializeOwned>(raw: &str) -> Option<T> {
    serde_json::from_str(raw).ok()
}

/// `Display`/`FromStr` codecs for a scalar [`LocalKey`] value.
fn text_encode<T: std::fmt::Display + ?Sized>(value: &T) -> Option<String> {
    Some(value.to_string())
}

fn text_decode<T: std::str::FromStr>(raw: &str) -> Option<T> {
    raw.parse().ok()
}

/// Hex round trip of the window frame's restorable state bytes.
fn hex_encode(bytes: &[u8]) -> Option<String> {
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

/// `(members, apps)` counts per channel, stored as one JSON map of optional pairs.
type ChannelCounts = (HashMap<ChannelId, u32>, HashMap<ChannelId, u32>);

fn encode_channel_counts((members, apps): &ChannelCounts) -> Option<String> {
    let mut combined: HashMap<&ChannelId, (Option<u32>, Option<u32>)> = HashMap::new();
    for (channel, &count) in members {
        combined.entry(channel).or_default().0 = Some(count);
    }
    for (channel, &count) in apps {
        combined.entry(channel).or_default().1 = Some(count);
    }
    serde_json::to_string(&combined).ok()
}

fn decode_channel_counts(raw: &str) -> Option<ChannelCounts> {
    let parsed: HashMap<String, (Option<u32>, Option<u32>)> = serde_json::from_str(raw).ok()?;
    let mut counts = (HashMap::new(), HashMap::new());
    for (channel, (members, apps)) in parsed {
        let id = ChannelId::from(channel);
        if let Some(count) = members {
            counts.0.insert(id.clone(), count);
        }
        if let Some(count) = apps {
            counts.1.insert(id, count);
        }
    }
    Some(counts)
}

/// The conversation open last in this workspace, restored at launch like Slack.
pub(crate) const LAST_CONVERSATION: LocalKey<ChannelId, ChannelId> = LocalKey::new(
    "last_conversation",
    "open conversation",
    |channel| Some(channel.as_str().to_owned()),
    |raw| Some(ChannelId::from(raw)),
);
/// Per-section sidebar filter and sort choices.
pub(crate) const SECTION_PREFS: LocalKey<SectionPrefs, SectionPrefs> = LocalKey::new(
    "section_prefs",
    "sidebar section preferences",
    json_encode,
    json_decode,
);
/// The window's frame (AppKit restorable state, hex-encoded).
pub(crate) const WINDOW_STATE: LocalKey<[u8], Vec<u8>> =
    LocalKey::new("window_state", "window frame", hex_encode, hex_decode);
/// The Color Mode chosen in Preferences.
pub(crate) const COLOR_MODE: LocalKey<AppearanceMode, AppearanceMode> = LocalKey::new(
    "color_mode",
    "color mode",
    |mode| Some(mode.key().to_owned()),
    AppearanceMode::from_key,
);
/// The search pill's recent searches.
pub(crate) const SEARCH_HISTORY: LocalKey<[String], Vec<String>> =
    LocalKey::new("search_history", "search history", json_encode, json_decode);
/// The search panel's history and filter toggles.
pub(crate) const SEARCH_HISTORY_SHOWN: LocalKey<bool, bool> = LocalKey::new(
    "search_history_shown",
    "history toggle",
    text_encode,
    text_decode,
);
pub(crate) const SEARCH_FILTERS_SHOWN: LocalKey<bool, bool> = LocalKey::new(
    "search_filters_shown",
    "filter toggle",
    text_encode,
    text_decode,
);
/// The recently visited conversations (the search menu's "Recent places" and the history
/// clock's menu).
pub(crate) const RECENT_PLACES: LocalKey<[ChannelId], Vec<ChannelId>> =
    LocalKey::new("recent_places", "recent places", json_encode, json_decode);
/// The Font chosen in Preferences (its family name).
pub(crate) const FONT: LocalKey<usize, usize> = LocalKey::new(
    "font",
    "font",
    |font| {
        theme::FONT_FAMILIES
            .get(*font)
            .map(|family| (*family).to_owned())
    },
    |family| {
        theme::FONT_FAMILIES
            .iter()
            .position(|known| *known == family)
    },
);
/// The zoom chosen in Preferences (its level).
pub(crate) const ZOOM: LocalKey<usize, usize> = LocalKey::new(
    "zoom",
    "zoom",
    |level| Some(level.to_string()),
    |raw| {
        raw.parse()
            .ok()
            .filter(|level| *level < crate::zoom::FACTORS.len())
    },
);
/// All Preferences settings.
pub(crate) const SETTINGS: LocalKey<Settings, Settings> = LocalKey::new(
    "preferences_settings",
    "preferences",
    json_encode,
    json_decode,
);
/// Cached `(members, apps)` counts per channel.
pub(crate) const CHANNEL_COUNTS_CACHE: LocalKey<ChannelCounts, ChannelCounts> = LocalKey::new(
    "channel_counts_cache",
    "channel counts cache",
    encode_channel_counts,
    decode_channel_counts,
);
/// Per-conversation composer drafts.
pub(crate) const DRAFTS: LocalKey<[SavedDraft], Vec<SavedDraft>> =
    LocalKey::new("drafts", "drafts", json_encode, json_decode);
/// Recently and frequently used emoji shortcodes.
pub(crate) const FREQUENT_EMOJI: LocalKey<[String], Vec<String>> =
    LocalKey::new("frequent_emoji", "frequent emoji", json_encode, json_decode);
/// The chosen emoji skin tone (1..=6).
pub(crate) const EMOJI_TONE: LocalKey<u8, u8> = LocalKey::new(
    "emoji_tone",
    "emoji skin tone",
    |tone| Some(tone.to_string()),
    |raw| raw.parse().ok().filter(|tone| (1..=6).contains(tone)),
);
/// The user's preferred workspace rail order (JSON array of TeamId strings).
pub(crate) const WORKSPACE_ORDER: LocalKey<[String], Vec<String>> = LocalKey::new(
    "workspace_order",
    "workspace order",
    json_encode,
    json_decode,
);

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

    /// Immediately enqueue [`Command::Shutdown`] on this workspace's core without blocking, so
    /// multi-workspace app quit can signal every core before joining any thread.
    pub fn send_shutdown(&self) {
        if let Some(commands) = &self.commands {
            let _ = commands.unbounded_send(Command::Shutdown);
        }
    }

    /// Return a blocking closure that joins this workspace's core thread after [`Self::send_shutdown`].
    pub fn join_shutdown_task(&self) -> Option<Box<dyn FnOnce() + Send>> {
        let thread_slot = self.core_thread.clone()?;
        Some(Box::new(move || {
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
        } else {
            cx.set_http_client(Arc::new(crate::media::OfflineHttp));
            crate::media::disable_downloads(cx);
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

    /// `key`'s value for this workspace, decoded; `None` offline or when unset.
    pub fn read_local<W: ?Sized, R>(&self, key: &LocalKey<W, R>) -> Option<R> {
        let team = &self.session.as_ref()?.team_id;
        let store = self.read_store().and_then(|s| s.lock().ok())?;
        let raw = store.local(team, key.key).ok().flatten()?;
        (key.decode)(&raw)
    }

    /// Live mode: a write storing `value` under `key` for this workspace, to run off the UI
    /// thread; a failure is logged naming what was not remembered. `None` offline.
    pub fn remember<W: ?Sized, R>(
        &self,
        key: &LocalKey<W, R>,
        value: &W,
    ) -> Option<impl Future<Output = ()> + Send + use<W, R>> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.store.clone()?;
        let value = (key.encode)(value)?;
        let what = key.what;
        let key = key.key;
        Some(async move {
            let Ok(mut store) = store.lock() else {
                return;
            };
            if let Err(e) = store.set_local(&team, key, &value) {
                tracing::warn!(what, error = %e, "could not persist local state");
            }
        })
    }

    /// The Preferences settings saved locally; `None` offline or before the first save.
    pub fn settings(&self) -> Option<Settings> {
        self.read_local(&SETTINGS)
    }

    /// Saved per-section sidebar filter and sort choices; `None` offline or unset.
    pub fn section_prefs(&self) -> Option<SectionPrefs> {
        self.read_local(&SECTION_PREFS)
    }

    /// Saved workspace switcher order (`TeamId` strings); `None` offline or unset.
    pub fn workspace_order(&self) -> Option<Vec<String>> {
        self.read_local(&WORKSPACE_ORDER)
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
        assert!(bridge.read_local(&DRAFTS).is_none());
        assert!(bridge.section_prefs().is_none());
        assert!(bridge.store().is_none() && COLOR_MODE.read(&HashMap::new()).is_none());
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
        assert_eq!(LAST_CONVERSATION.read(&local), None);
        local.insert(LAST_CONVERSATION.name().into(), "C0ENG".into());
        assert_eq!(
            LAST_CONVERSATION.read(&local),
            Some(ChannelId::from("C0ENG"))
        );

        // parse_window_state: valid hex, empty, odd length, non-hex, non-ASCII
        assert_eq!(WINDOW_STATE.read(&local), None);
        local.insert(WINDOW_STATE.name().into(), "007fFF".into());
        assert_eq!(WINDOW_STATE.read(&local), Some(vec![0x00, 0x7f, 0xff]));
        local.insert(WINDOW_STATE.name().into(), String::new());
        assert_eq!(WINDOW_STATE.read(&local), Some(vec![]));
        local.insert(WINDOW_STATE.name().into(), "abc".into());
        assert_eq!(WINDOW_STATE.read(&local), None);
        local.insert(WINDOW_STATE.name().into(), "zzzz".into());
        assert_eq!(WINDOW_STATE.read(&local), None);
        local.insert(WINDOW_STATE.name().into(), "éé".into());
        assert_eq!(WINDOW_STATE.read(&local), None);

        // parse_font
        assert_eq!(FONT.read(&local), None);
        local.insert(FONT.name().into(), theme::FONT_FAMILIES[2].into());
        assert_eq!(FONT.read(&local), Some(2));
        local.insert(FONT.name().into(), "NonexistentFont".into());
        assert_eq!(FONT.read(&local), None);

        // parse_zoom bounds
        assert_eq!(ZOOM.read(&local), None);
        local.insert(ZOOM.name().into(), "0".into());
        assert_eq!(ZOOM.read(&local), Some(0));
        local.insert(
            ZOOM.name().into(),
            (crate::zoom::FACTORS.len() - 1).to_string(),
        );
        assert_eq!(ZOOM.read(&local), Some(crate::zoom::FACTORS.len() - 1));
        local.insert(ZOOM.name().into(), crate::zoom::FACTORS.len().to_string());
        assert_eq!(ZOOM.read(&local), None);
        local.insert(ZOOM.name().into(), "-1".into());
        assert_eq!(ZOOM.read(&local), None);

        // channel counts: merge & malformed JSON
        assert_eq!(CHANNEL_COUNTS_CACHE.read(&local), None);
        local.insert(
            CHANNEL_COUNTS_CACHE.name().into(),
            r#"{"C1":[12,3],"C2":[5,null],"C3":[null,2]}"#.into(),
        );
        let (members, apps) = CHANNEL_COUNTS_CACHE.read(&local).unwrap();
        assert_eq!(members.get(&ChannelId::from("C1")), Some(&12));
        assert_eq!(members.get(&ChannelId::from("C2")), Some(&5));
        assert!(!members.contains_key(&ChannelId::from("C3")));
        assert_eq!(apps.get(&ChannelId::from("C1")), Some(&3));
        assert!(!apps.contains_key(&ChannelId::from("C2")));
        assert_eq!(apps.get(&ChannelId::from("C3")), Some(&2));
        local.insert(CHANNEL_COUNTS_CACHE.name().into(), "{broken".into());
        assert_eq!(CHANNEL_COUNTS_CACHE.read(&local), None);

        // parse_emoji_tone 1..=6 bounds
        for valid in 1u8..=6 {
            local.insert(EMOJI_TONE.name().into(), valid.to_string());
            assert_eq!(EMOJI_TONE.read(&local), Some(valid));
        }
        for invalid in ["0", "7", "255", "abc"] {
            local.insert(EMOJI_TONE.name().into(), invalid.into());
            assert_eq!(EMOJI_TONE.read(&local), None, "{invalid}");
        }

        // parse_settings, search_history, toggles, recent_places, drafts, frequent_emoji
        local.insert(SETTINGS.name().into(), "{bad".into());
        assert!(SETTINGS.read(&local).is_none());
        local.insert(
            SETTINGS.name().into(),
            serde_json::to_string(&Settings::default()).unwrap(),
        );
        assert!(SETTINGS.read(&local).is_some());

        local.insert(
            SEARCH_HISTORY.name().into(),
            r#"["deploy","incident"]"#.into(),
        );
        assert_eq!(
            SEARCH_HISTORY.read(&local),
            Some(vec!["deploy".into(), "incident".into()])
        );
        local.insert(SEARCH_HISTORY_SHOWN.name().into(), "true".into());
        local.insert(SEARCH_FILTERS_SHOWN.name().into(), "false".into());
        assert_eq!(SEARCH_HISTORY_SHOWN.read(&local), Some(true));
        assert_eq!(SEARCH_FILTERS_SHOWN.read(&local), Some(false));

        local.insert(RECENT_PLACES.name().into(), r#"["C0ENG","D0ALICE"]"#.into());
        assert_eq!(
            RECENT_PLACES.read(&local),
            Some(vec![ChannelId::from("C0ENG"), ChannelId::from("D0ALICE")])
        );

        local.insert(FREQUENT_EMOJI.name().into(), r#"["tada","rocket"]"#.into());
        assert_eq!(
            FREQUENT_EMOJI.read(&local),
            Some(vec!["tada".into(), "rocket".into()])
        );

        local.insert(DRAFTS.name().into(), "not json".into());
        assert!(DRAFTS.read(&local).is_none());
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
                .remember(&LAST_CONVERSATION, &ChannelId::from("C0ENG"))
                .unwrap(),
        );
        run_ready(
            bridge
                .remember(&WINDOW_STATE, &[0xde, 0xad, 0xbe, 0xef])
                .unwrap(),
        );
        run_ready(bridge.remember(&COLOR_MODE, &AppearanceMode::Dark).unwrap());
        run_ready(
            bridge
                .remember(&SEARCH_HISTORY, &["alpha".to_owned(), "beta".to_owned()])
                .unwrap(),
        );
        run_ready(bridge.remember(&SEARCH_HISTORY_SHOWN, &false).unwrap());
        run_ready(bridge.remember(&SEARCH_FILTERS_SHOWN, &true).unwrap());
        run_ready(
            bridge
                .remember(
                    &RECENT_PLACES,
                    &[ChannelId::from("C0ENG"), ChannelId::from("D0ALICE")],
                )
                .unwrap(),
        );
        run_ready(bridge.remember(&FONT, &1).unwrap());
        run_ready(bridge.remember(&ZOOM, &4).unwrap());
        run_ready(bridge.remember(&SETTINGS, &settings).unwrap());
        run_ready(bridge.remember(&DRAFTS, drafts.as_slice()).unwrap());
        run_ready(
            bridge
                .remember(&FREQUENT_EMOJI, &["fire".to_owned(), "100".to_owned()])
                .unwrap(),
        );
        run_ready(bridge.remember(&EMOJI_TONE, &5).unwrap());
        run_ready(
            bridge
                .remember(&WORKSPACE_ORDER, &["T2".to_owned(), "T1".to_owned()])
                .unwrap(),
        );
        run_ready(bridge.remember(&SECTION_PREFS, &section_prefs).unwrap());
        run_ready(
            bridge
                .remember(&CHANNEL_COUNTS_CACHE, &(members.clone(), apps.clone()))
                .unwrap(),
        );

        let local = bridge.all_local();
        assert_eq!(
            LAST_CONVERSATION.read(&local),
            Some(ChannelId::from("C0ENG"))
        );
        assert_eq!(
            bridge.read_local(&LAST_CONVERSATION),
            Some(ChannelId::from("C0ENG"))
        );
        assert_eq!(
            WINDOW_STATE.read(&local),
            Some(vec![0xde, 0xad, 0xbe, 0xef])
        );
        assert_eq!(COLOR_MODE.read(&local), Some(AppearanceMode::Dark));
        assert_eq!(
            SEARCH_HISTORY.read(&local),
            Some(vec!["alpha".into(), "beta".into()])
        );
        assert_eq!(SEARCH_HISTORY_SHOWN.read(&local), Some(false));
        assert_eq!(SEARCH_FILTERS_SHOWN.read(&local), Some(true));
        assert_eq!(
            RECENT_PLACES.read(&local),
            Some(vec![ChannelId::from("C0ENG"), ChannelId::from("D0ALICE")])
        );
        assert_eq!(FONT.read(&local), Some(1));
        assert_eq!(ZOOM.read(&local), Some(4));
        let decoded_settings = SETTINGS.read(&local).unwrap();
        assert_eq!(decoded_settings.keywords, "deploy, urgent");
        assert!(!decoded_settings.enter_sends);
        assert_eq!(bridge.settings().unwrap().keywords, "deploy, urgent");
        assert_eq!(DRAFTS.read(&local), Some(drafts.clone()));
        assert_eq!(bridge.read_local(&DRAFTS), Some(drafts));
        assert_eq!(
            FREQUENT_EMOJI.read(&local),
            Some(vec!["fire".into(), "100".into()])
        );
        assert_eq!(EMOJI_TONE.read(&local), Some(5));
        assert_eq!(
            bridge.workspace_order(),
            Some(vec!["T2".into(), "T1".into()])
        );
        assert_eq!(bridge.section_prefs(), Some(section_prefs));
        assert_eq!(CHANNEL_COUNTS_CACHE.read(&local), Some((members, apps)));
    }
}
