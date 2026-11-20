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
//!
//! One deviation from the guide's drain snippet: it awaits the event stream with
//! `futures::StreamExt::next`, but the `futures` crate is not a GUI dependency (only
//! `slackless-core` may name its types). The drain therefore polls the receiver's inherent
//! `try_recv` between `background_executor().timer` ticks; when the core's command receiver is
//! gone (`sender.is_closed()`), the channel is drained and the loop ends.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::gpui::{self, AppContext as _, Context, Entity, EventEmitter, Subscription, Window};
use slackless_core::api::ConversationCounts;
use slackless_core::auth::Session;
use slackless_core::store::Store;
use slackless_core::types::{ChannelId, Message, TeamId, Ts};
use slackless_core::{Client, Command, Event, SharedStore};

use crate::fixtures::{self, Fixtures};
use crate::media::CoreHttp;

/// Idle wait between event polls while the core is running. The guide's pattern would await
/// the stream directly; without a `futures` dependency a short bounded poll keeps events at
/// sub-frame latency without spinning the executor.
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(16);

/// Newest messages of a conversation the message list shows in live mode.
const HISTORY_LIMIT: usize = 500;

/// The store's local-state key for the last open conversation.
const LAST_CONVERSATION: &str = "last_conversation";

/// The store's local-state key for the window's frame (AppKit restorable state, hex-encoded).
const WINDOW_STATE: &str = "window_state";

/// Receives core events and re-emits them to subscribed views on the UI thread. The bridge
/// emits through this entity so subscribers can use gpui's native `subscribe` machinery.
struct EventHub;

impl EventEmitter<Event> for EventHub {}

pub struct CoreBridge {
    /// Live mode: forwards commands into the core's command loop. Fixture mode: `None`.
    /// The `futures::channel::mpsc::UnboundedSender` it captures is not nameable here, so it
    /// is wrapped in a closure instead of a field.
    command_sink: Option<Box<dyn Fn(Command) + Send>>,
    /// Live mode: the store the core keeps current; views re-read it to render. Fixture
    /// mode: `None`.
    store: Option<SharedStore>,
    /// Live mode: the signed-in session the core runs with.
    session: Option<Session>,
    /// Created by [`CoreBridge::start`], or lazily by [`CoreBridge::subscribe`] in fixture
    /// mode (where nothing will ever emit on it).
    hub: Option<Entity<EventHub>>,
}

impl CoreBridge {
    /// A bridge with no core behind it (fixture mode).
    pub fn offline() -> Self {
        Self {
            command_sink: None,
            store: None,
            session: None,
            hub: None,
        }
    }

    /// Starts the core live for `session`: opens its durable store and starts slackless-core on
    /// its dedicated tokio thread (guide section 13). Events flow into gpui through the drain
    /// task; views receive them via [`CoreBridge::subscribe_in`]. `None` when the store or the
    /// core cannot be started (the reason is logged).
    pub fn start(session: Session, cx: &mut gpui::App) -> Option<Self> {
        let store = open_store(&session)?;
        let handle = match Client::start(session.clone(), store) {
            Ok(handle) => handle,
            Err(e) => {
                eprintln!(
                    "slackless: could not start the core ({e}); running without a core connection"
                );
                return None;
            }
        };
        let hub = cx.new(|_| EventHub);
        let mut bridge = Self {
            command_sink: None,
            store: None,
            session: Some(session),
            hub: Some(hub.clone()),
        };

        // Remote images (`img(url)`: photos, emoji, the workspace icon) fetch through the core.
        cx.set_http_client(Arc::new(CoreHttp(handle.fetcher())));
        let (commands, mut events, store) = handle.into_parts();
        bridge.store = Some(store);
        let sender = commands.clone();
        bridge.command_sink = Some(Box::new(move |command| {
            if let Err(e) = commands.unbounded_send(command) {
                eprintln!("slackless: core has stopped; command dropped ({e})");
            }
        }));

        // Detached: dropping a gpui task cancels it, and the drain must outlive this call.
        cx.spawn(async move |cx| {
            loop {
                match events.try_recv() {
                    Ok(event) => cx.update_entity(&hub, |_, cx| cx.emit(event)),
                    Err(_) => {
                        // The core's command loop ended (it drains its command channel last),
                        // so the event channel is closed too once buffered events are gone.
                        if sender.is_closed() {
                            break;
                        }
                        cx.background_executor().timer(EVENT_POLL_INTERVAL).await;
                    }
                }
            }
        })
        .detach();

        Some(bridge)
    }

    /// Live mode: the signed-in workspace as the store has it now, with Slack's latest read
    /// state (`counts`). `None` offline.
    pub fn snapshot(&self, counts: &HashMap<ChannelId, ConversationCounts>) -> Option<Fixtures> {
        let session = self.session.as_ref()?;
        let store = self.store.as_ref()?.lock().ok()?;
        Some(fixtures::live(session, &store, counts))
    }

    /// Live mode: a read of `channel`'s stored history (oldest first, without deleted
    /// messages) to run off the UI thread: the newest `HISTORY_LIMIT` messages, or the
    /// `HISTORY_LIMIT` before `before` when paging older on scroll-back. `None` offline.
    pub fn history(
        &self,
        channel: ChannelId,
        before: Option<Ts>,
    ) -> Option<impl Future<Output = Vec<Message>> + Send + 'static> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.store.clone()?;
        Some(async move {
            let Ok(store) = store.lock() else {
                return Vec::new();
            };
            match store.channel_history(&team, &channel, before.as_ref(), HISTORY_LIMIT) {
                Ok(rows) => rows
                    .into_iter()
                    .filter(|row| !row.deleted)
                    .map(|row| row.message)
                    .collect(),
                Err(e) => {
                    eprintln!("slackless: could not read {channel} from the local store ({e})");
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
        let store = self.store.clone()?;
        Some(async move {
            let store = store.lock().ok()?;
            match store.latest_messages(&team, std::slice::from_ref(&channel)) {
                Ok(mut latest) => latest.remove(&channel).map(|stored| stored.message),
                Err(e) => {
                    eprintln!("slackless: could not read {channel}'s latest message ({e})");
                    None
                }
            }
        })
    }

    /// Live mode: a read of one thread's stored history (root + replies, oldest first,
    /// without deleted messages) to run off the UI thread. `None` offline.
    pub fn thread(
        &self,
        channel: ChannelId,
        thread_ts: Ts,
    ) -> Option<impl Future<Output = Vec<Message>> + Send + 'static> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.store.clone()?;
        Some(async move {
            let Ok(store) = store.lock() else {
                return Vec::new();
            };
            match store.thread_replies(&team, &channel, &thread_ts) {
                Ok(rows) => rows
                    .into_iter()
                    .filter(|row| !row.deleted)
                    .map(|row| row.message)
                    .collect(),
                Err(e) => {
                    eprintln!(
                        "slackless: could not read the thread {thread_ts} of {channel} from the local store ({e})"
                    );
                    Vec::new()
                }
            }
        })
    }

    /// Forward a command to the core. Offline, commands are dropped by design.
    pub fn send(&self, command: Command) {
        if let Some(sink) = &self.command_sink {
            sink(command);
        }
    }

    /// The store the core keeps current (live mode); views re-read it to render. `None` in
    /// fixture mode.
    pub fn store(&self) -> Option<&SharedStore> {
        self.store.as_ref()
    }

    /// The conversation last open in this workspace, restored at launch like Slack. `None`
    /// offline or before the first one.
    pub fn last_conversation(&self) -> Option<ChannelId> {
        let team = &self.session.as_ref()?.team_id;
        let store = self.store.as_ref()?.lock().ok()?;
        let id = store.local(team, LAST_CONVERSATION).ok().flatten()?;
        Some(ChannelId::from(id.as_str()))
    }

    /// Live mode: a write remembering `channel` as the open conversation, to run off the UI
    /// thread. `None` offline.
    pub fn remember_conversation(
        &self,
        channel: ChannelId,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.store.clone()?;
        Some(async move {
            let Ok(mut store) = store.lock() else {
                return;
            };
            if let Err(e) = store.set_local(&team, LAST_CONVERSATION, channel.as_str()) {
                eprintln!("slackless: could not remember the open conversation ({e})");
            }
        })
    }

    /// The window frame saved by [`Self::remember_window_state`], so the window reopens where
    /// and how it was left, like Slack's. `None` offline or before the first save.
    pub fn window_state(&self) -> Option<Vec<u8>> {
        let team = &self.session.as_ref()?.team_id;
        let store = self.store.as_ref()?.lock().ok()?;
        let hex = store.local(team, WINDOW_STATE).ok().flatten()?;
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
            .collect()
    }

    /// Live mode: a write remembering the window's native state (`Window::native_window_state`),
    /// to run off the UI thread. `None` offline.
    pub fn remember_window_state(
        &self,
        state: Vec<u8>,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.store.clone()?;
        Some(async move {
            let hex: String = state.iter().map(|byte| format!("{byte:02x}")).collect();
            let Ok(mut store) = store.lock() else {
                return;
            };
            if let Err(e) = store.set_local(&team, WINDOW_STATE, &hex) {
                eprintln!("slackless: could not remember the window frame ({e})");
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
}

/// Opens the durable store for a session. Falls back to an in-memory store when no home
/// directory is available; returns `None` only if no store can be opened at all.
fn open_store(session: &Session) -> Option<Store> {
    match default_store_path(&session.team_id) {
        Some(path) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match Store::open(&path) {
                Ok(store) => Some(store),
                Err(e) => {
                    eprintln!(
                        "slackless: could not open the local store at {} ({e}); running without a core connection",
                        path.display()
                    );
                    None
                }
            }
        }
        None => match Store::open_in_memory() {
            Ok(store) => Some(store),
            Err(e) => {
                eprintln!(
                    "slackless: could not open an in-memory store ({e}); running without a core connection"
                );
                None
            }
        },
    }
}

/// The durable store lives under the platform's application-data directory, one database per
/// workspace (the store location stays a front-end decision, per slackless-core's docs).
fn default_store_path(team_id: &TeamId) -> Option<PathBuf> {
    let mut path = PathBuf::from(std::env::var_os("HOME")?);
    if cfg!(target_os = "macos") {
        path.push("Library/Application Support/slackless");
    } else {
        path.push(".local/share/slackless");
    }
    path.push(format!("{team_id}.sqlite"));
    Some(path)
}
