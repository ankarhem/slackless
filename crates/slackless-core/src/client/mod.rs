//! The runtime thread, the command loop and the front-end handle.

mod commands;
mod hydrate;
mod rtm;

use self::commands::{OutboxRetry, QueuedMutation};
use self::hydrate::*;

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::ops::Deref;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::AbortHandle;

use crate::api::{ApiError, SlackApi};
use crate::auth::Session;
use crate::backend::Backend;
use crate::backfill::{
    BACKFILL_SYNCED_KEY, Backfill, BackfillControl, PAGE_DELAY, SyncedMap, decode_synced_spans,
    wait_out_rate_limits,
};
use crate::fetcher::Fetcher;
use crate::rtm::ConnectionState;
use crate::rtm::{ConnectParams, RtmClient, RtmConfig, RtmEvent};
use crate::runtime::shared_runtime;
use crate::store::{Store, StoreError};
use crate::types::event::MessageEvent;
use crate::types::time::now_epoch_secs;
use crate::types::{
    BotId, BotProfile, ChannelId, ConversationKind, Message, MessageSubtype, SlackEvent, TeamId,
    Ts, UserId,
};
use crate::unfurl::{Unfurls, event_messages};
use crate::{
    ChannelCommand, CollabCommand, Command, Event, FailedMutation, HUDDLE_MEDIA_CHANNEL_CAPACITY,
    HuddleCommand, HuddleEvent, HuddleMediaEvent, MessagingCommand, WorkspaceCommand, apply_event,
};

/// The store shared between the core (writer) and front-ends (readers).
///
/// SQLite is blocking: front-ends should lock it off their UI thread (or keep reads short). The
/// core's own front-end queries (search, catch-up, semantic hydration) read a file-backed store
/// through private read-only connections, so however long they run they never hold this lock.
pub type SharedStore = Arc<Mutex<Store>>;

/// One front-end read of a [`SharedStore`], opened by [`read_store`].
pub(crate) enum StoreRead<'a> {
    /// A private read-only connection to the store's database file.
    Private(Store),
    /// An in-memory store, which no second connection can see, under its lock.
    Locked(MutexGuard<'a, Store>),
}

impl Deref for StoreRead<'_> {
    type Target = Store;

    fn deref(&self) -> &Store {
        match self {
            Self::Private(store) => store,
            Self::Locked(store) => store,
        }
    }
}

/// Opens `store` for one front-end query (search, catch-up, semantic hydration). A file-backed
/// store is read through a private read-only WAL connection, so the query runs while the core
/// applies websocket events under the store's lock (and never holds up a front-end's shared read
/// handle either): the lock is held only to look up the file.
pub(crate) fn read_store(store: &SharedStore) -> StoreRead<'_> {
    let path = store
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .path()
        .map(Path::to_path_buf);
    if let Some(path) = path {
        match Store::open_read_only(&path) {
            Ok(private) => return StoreRead::Private(private),
            Err(err) => tracing::warn!(
                path = %path.display(),
                %err,
                "could not open a private read connection; reading under the store lock"
            ),
        }
    }
    StoreRead::Locked(store.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Cloneable sender for front-end [`Command`]s into the core.
pub type CommandSender = UnboundedSender<Command>;
/// Ordered receiver for control and store-sync [`Event`]s emitted by the core.
pub type EventReceiver = UnboundedReceiver<Event>;
/// Bounded sender for high-frequency [`HuddleMediaEvent`]s (`try_send`, drops stale frames).
pub type HuddleMediaSender = mpsc::Sender<HuddleMediaEvent>;
/// Bounded receiver for high-frequency [`HuddleMediaEvent`]s emitted by active native huddles.
pub type HuddleMediaReceiver = mpsc::Receiver<HuddleMediaEvent>;

/// The orchestrator entry point.
pub struct Client;

impl Client {
    /// Starts the core for one signed-in workspace on a shared multi-thread tokio runtime.
    ///
    /// Connects the websocket immediately and hydrates the store (`client.userBoot`,
    /// `client.counts`, ...) concurrently in the background, feeding every inbound event into
    /// `store` and serving [`Command`]s until [`Command::Shutdown`] or until every command sender
    /// is dropped. The caller opens `store` (on disk for real sessions) so the location stays a
    /// front-end decision.
    pub fn start(session: Session, store: Store) -> std::io::Result<ClientHandle> {
        let api_base = session.api_base();
        Self::start_with_endpoints(
            session,
            store,
            api_base,
            crate::types::endpoints::WSS_PRIMARY.to_owned(),
        )
    }

    /// Starts the core against custom HTTP (`api_base`) and RTM WebSocket (`rtm_base_url`)
    /// endpoints. Production uses [`Client::start`]; integration tests use this seam to exercise
    /// the real [`SlackApi`], [`RtmClient`], and SQLite [`Store`] end-to-end against local mock
    /// servers without bypassing the wire adapters.
    pub fn start_with_endpoints(
        session: Session,
        store: Store,
        api_base: impl Into<String>,
        rtm_base_url: impl Into<String>,
    ) -> std::io::Result<ClientHandle> {
        let api_base = api_base.into();
        let rtm_base_url = rtm_base_url.into();
        let store: SharedStore = Arc::new(Mutex::new(store));
        let (command_tx, command_rx) = mpsc::unbounded();
        let (event_tx, event_rx) = mpsc::unbounded();
        let (huddle_media_tx, huddle_media_rx) = mpsc::channel(HUDDLE_MEDIA_CHANNEL_CAPACITY);
        let events = Emitter(event_tx);
        let thread_store = Arc::clone(&store);
        let handle = shared_runtime()?.handle().clone();
        let files = SlackApi::new(&api_base, session.xoxc_token.clone(), &session.d_cookie).ok();
        let fetcher = Fetcher::new(handle.clone(), files);
        let thread = std::thread::Builder::new()
            .name("slackless-core".into())
            .spawn(move || {
                handle.block_on(run_session(
                    session,
                    api_base,
                    rtm_base_url,
                    thread_store,
                    command_rx,
                    events,
                    huddle_media_tx,
                ));
            })?;
        Ok(ClientHandle {
            commands: command_tx,
            events: event_rx,
            huddle_media: huddle_media_rx,
            store,
            fetcher,
            thread,
        })
    }
}

/// The command sink is closed: the core has shut down.
#[derive(Debug, thiserror::Error)]
#[error("slackless core has shut down")]
pub struct Closed;

/// Unpacked channels and handles returned by [`ClientHandle::into_parts`].
pub struct ClientHandles {
    pub commands: CommandSender,
    pub events: EventReceiver,
    pub huddle_media: HuddleMediaReceiver,
    pub store: SharedStore,
    pub fetcher: Fetcher,
    pub thread: JoinHandle<()>,
}

/// A front-end's connection to a running core: a [`Command`] sender, an ordered [`Event`]
/// receiver, a bounded [`HuddleMediaEvent`] receiver, and the shared store. The channels are
/// `futures::channel::mpsc`, so any executor (GPUI's smol, a CLI's tokio) can drive them.
pub struct ClientHandle {
    commands: CommandSender,
    events: EventReceiver,
    huddle_media: HuddleMediaReceiver,
    store: SharedStore,
    fetcher: Fetcher,
    thread: JoinHandle<()>,
}

impl ClientHandle {
    pub fn send(&self, command: impl Into<Command>) -> Result<(), Closed> {
        self.commands
            .unbounded_send(command.into())
            .map_err(|_| Closed)
    }

    /// Downloads public assets (profile photos, emoji, icons) on the core runtime.
    pub fn fetcher(&self) -> Fetcher {
        self.fetcher.clone()
    }

    /// A cloneable command sender, e.g. for views that issue commands.
    pub fn commands(&self) -> CommandSender {
        self.commands.clone()
    }

    /// The ordered control/store event stream; ends when the core stops.
    pub fn events(&mut self) -> &mut EventReceiver {
        &mut self.events
    }

    /// The bounded high-frequency huddle media stream (video frames, tile close, audio levels).
    pub fn huddle_media(&mut self) -> &mut HuddleMediaReceiver {
        &mut self.huddle_media
    }

    pub fn store(&self) -> &SharedStore {
        &self.store
    }

    /// Unpacks the handle into its owned channels, store, fetcher, and runtime thread handle so
    /// front-ends can move receivers into separate drain tasks and join the thread on quit.
    pub fn into_parts(self) -> ClientHandles {
        ClientHandles {
            commands: self.commands,
            events: self.events,
            huddle_media: self.huddle_media,
            store: self.store,
            fetcher: self.fetcher,
            thread: self.thread,
        }
    }

    /// Stops the core and waits for its thread to exit.
    pub fn shutdown(self) -> std::thread::Result<()> {
        let _ = self.commands.unbounded_send(Command::Shutdown);
        self.thread.join()
    }
}

/// Failures inside the core, reported to the front-end as [`Event`]s.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("background task cancelled")]
    Cancelled,
}

/// The event sender.
#[derive(Debug, Clone)]
pub(crate) struct Emitter(pub UnboundedSender<Event>);

impl Emitter {
    /// `false` once the front-end dropped its receiver.
    pub fn emit(&self, event: Event) -> bool {
        self.0.unbounded_send(event).is_ok()
    }

    pub fn report(&self, context: &str, err: CoreError) {
        match err {
            CoreError::Cancelled => {
                tracing::debug!(context, "core task cancelled");
            }
            CoreError::Api(ApiError::AuthExpired(code)) => {
                self.emit(Event::AuthExpired(code));
            }
            err => {
                self.emit(Event::Error(format!("{context}: {err}")));
            }
        }
    }
}

/// Structured concurrency registry: tracks all background tasks spawned by a [`Core`] instance
/// (removing each entry automatically on completion or abort) so [`Core::shutdown`] cancels every
/// in-flight task cleanly instead of leaking onto the shared runtime.
#[derive(Clone, Default)]
pub(crate) struct TaskTracker {
    inner: Arc<Mutex<TaskRegistry>>,
}

#[derive(Default)]
struct TaskRegistry {
    closed: bool,
    next_id: u64,
    handles: HashMap<u64, AbortHandle>,
}

struct RemoveOnDrop {
    inner: Arc<Mutex<TaskRegistry>>,
    id: u64,
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .handles
            .remove(&self.id);
    }
}

impl TaskTracker {
    /// Spawn `fut` on the current tokio runtime and register its [`AbortHandle`] until completion
    /// or [`Self::abort_all`]. Returns `None` once shutdown has closed the tracker.
    pub(crate) fn spawn<F>(&self, fut: F) -> Option<AbortHandle>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut reg = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if reg.closed {
            return None;
        }
        let id = reg.next_id;
        reg.next_id = reg.next_id.wrapping_add(1);
        let guard = RemoveOnDrop {
            inner: Arc::clone(&self.inner),
            id,
        };
        let handle = tokio::spawn(async move {
            let _guard = guard;
            fut.await;
        });
        let abort = handle.abort_handle();
        reg.handles.insert(id, abort.clone());
        Some(abort)
    }

    /// Close the tracker and abort every tracked background task.
    pub(crate) fn abort_all(&self) {
        let handles: Vec<AbortHandle> = {
            let mut reg = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            reg.closed = true;
            reg.handles.drain().map(|(_, h)| h).collect()
        };
        for handle in handles {
            handle.abort();
        }
    }

    #[cfg(test)]
    pub(crate) fn active_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .handles
            .len()
    }
}

/// Lightweight cloned context handed to spawned command/background tasks, eliminating repeated
/// multi-field clone preambles across handlers.
#[derive(Clone)]
pub(super) struct TaskCtx<B: Backend> {
    pub backend: B,
    pub store: SharedStore,
    pub workspace: TeamId,
    pub events: Emitter,
    pub unfurls: Unfurls<B>,
}

/// Runs a fallible store closure on tokio's blocking pool without panicking on cancellation.
pub(crate) async fn with_store<T, F>(store: &SharedStore, f: F) -> Result<T, CoreError>
where
    F: FnOnce(&mut Store) -> crate::store::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let store = Arc::clone(store);
    let task = tokio::task::spawn_blocking(move || {
        f(&mut store.lock().unwrap_or_else(PoisonError::into_inner))
    });
    match task.await {
        Ok(res) => res.map_err(CoreError::Store),
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        Err(_) => Err(CoreError::Cancelled),
    }
}

/// Runs an infallible store closure on tokio's blocking pool, returning `None` if cancelled during
/// shutdown (only resuming unwinds on actual panics).
pub(crate) async fn with_store_opt<T, F>(store: &SharedStore, f: F) -> Option<T>
where
    F: FnOnce(&mut Store) -> T + Send + 'static,
    T: Send + 'static,
{
    let store = Arc::clone(store);
    let task = tokio::task::spawn_blocking(move || {
        f(&mut store.lock().unwrap_or_else(PoisonError::into_inner))
    });
    match task.await {
        Ok(value) => Some(value),
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        Err(_) => None,
    }
}

async fn run_session(
    session: Session,
    api_base: String,
    rtm_base_url: String,
    store: SharedStore,
    commands: UnboundedReceiver<Command>,
    events: Emitter,
    huddle_media: HuddleMediaSender,
) {
    let api = match SlackApi::new(&api_base, session.xoxc_token.clone(), &session.d_cookie) {
        Ok(api) => api,
        Err(e) => return events.report("api client", e.into()),
    };
    let mut core = Core::new(
        api,
        store,
        session.team_id.clone(),
        events.clone(),
        huddle_media,
    )
    .await;

    // Start RTM immediately so live websocket events and front-end commands are responsive while
    // full workspace hydration runs concurrently in the background.
    let mut config =
        RtmConfig::new(ConnectParams::new(session.xoxc_token.clone()).with_team(&session.team_id));
    config.base_url = rtm_base_url;
    config.d_cookie = Some(session.d_cookie.clone());
    let rtm = match RtmClient::new(config) {
        Ok((client, rx, driver)) => {
            core.tasks.spawn(driver);
            core.rtm = Some(client);
            Some(rx)
        }
        Err(e) => {
            events.emit(Event::Error(format!("websocket: {e}")));
            None
        }
    };
    core.spawn_hydrate();
    run(core, commands, rtm).await;
}

/// Serves commands and websocket events until shutdown. Websocket events are applied in
/// arrival order; commands run as their own tasks so a slow request never stalls the socket.
pub(crate) async fn run<B: Backend>(
    mut core: Core<B>,
    mut commands: UnboundedReceiver<Command>,
    mut rtm: Option<tokio::sync::mpsc::Receiver<RtmEvent>>,
) {
    loop {
        tokio::select! {
            command = commands.next() => match command {
                None | Some(Command::Shutdown) => break,
                Some(command) => core.handle(command).await,
            },
            event = next_rtm(&mut rtm) => core.on_rtm(event).await,
        }
    }
    core.shutdown().await;
}

/// The next websocket event; pends forever once the socket stream has ended.
async fn next_rtm(rtm: &mut Option<tokio::sync::mpsc::Receiver<RtmEvent>>) -> RtmEvent {
    if let Some(rx) = rtm
        && let Some(event) = rx.recv().await
    {
        return event;
    }
    *rtm = None;
    std::future::pending().await
}

#[derive(Default)]
pub(super) struct HuddleSlot {
    /// Atomic generation counter checked lock-free by the 60Hz huddle media watcher.
    pub generation: Arc<AtomicU64>,
    pub joining: bool,
    pub handle: Option<crate::huddle::HuddleHandle>,
    pub call_id: Option<String>,
}

pub(crate) struct Core<B: Backend> {
    backend: B,
    store: SharedStore,
    workspace: TeamId,
    events: Emitter,
    huddle_media: HuddleMediaSender,
    backfill: Backfill<B>,
    backfills: HashMap<ChannelId, AbortHandle>,
    workspace_backfill: Option<AbortHandle>,
    /// Names the foreign channels shared-message unfurls point at (shared with `backfill`).
    unfurls: Unfurls<B>,
    /// Tracks all background tasks spawned by this core so `shutdown` aborts them cleanly.
    tasks: TaskTracker,
    rtm: Option<RtmClient>,
    /// DM peers whose presence is already covered: fetched once through `users.getPresence`
    /// and subscribed on the websocket. Every `presence_sub` frame replaces the whole
    /// subscription, so the accumulated set is resent as it grows.
    presence_peers: Arc<Mutex<Vec<UserId>>>,
    /// Ids already sent to `users.info` this session (found or not): each is asked once.
    users_asked: HashSet<UserId>,
    /// Team ids already sent to `team.info` this session (found or not): each is asked
    /// once. Shared with spawned tasks, which compute the ids off the `&self` borrow.
    teams_asked: Arc<Mutex<HashSet<TeamId>>>,
    /// Bot profiles read through `bots.info` or SQLite cache this session (shared between
    /// `attach_bot_profile` and `Command::ResolveBots`).
    bots: Arc<Mutex<HashMap<BotId, BotProfile>>>,
    /// Bot ids already requested via `bots.info` this session (found or not): each is asked once.
    bots_asked: HashSet<BotId>,
    /// Signed-in user id learned from `client.userBoot` (used to mirror self profile/presence).
    self_user_id: Arc<Mutex<Option<UserId>>>,
    /// Cloud-synced draft ids per channel (`drafts.list` / `drafts.create` / `drafts.update` / `drafts.delete`).
    cloud_draft_ids: Arc<Mutex<HashMap<ChannelId, String>>>,
    /// Per-channel async mutexes serializing `SyncDraft` so concurrent edits never double-create.
    draft_locks: Arc<Mutex<HashMap<ChannelId, Arc<tokio::sync::Mutex<()>>>>>,
    /// Whether the RTM websocket is currently disconnected or reconnecting.
    is_offline: bool,
    /// Becomes `true` after the first RTM `Connected` transition so subsequent reconnects refresh
    /// `client.counts` and prefetch any messages missed while disconnected or asleep.
    has_connected_once: bool,
    /// Outgoing mutations persisted in SQLite `offline_queue`, flushed FIFO on reconnect.
    offline_queue: VecDeque<(i64, QueuedMutation)>,
    /// Serializes durable V13 `outbox` flushes per workspace so reconnect / manual flushes are
    /// strictly FIFO and idempotent.
    outbox_flush_lock: Arc<tokio::sync::Mutex<()>>,
    /// Scheduled re-flush of the outbox after a transient send failure while connected.
    outbox_retry: OutboxRetry,
    /// True Stealth Mode: suppresses outbound typing, read-cursor updates, and active presence
    /// (shared atomically with background offline-queue flushers).
    stealth_mode: Arc<AtomicBool>,
    /// When true (default), persist finalized huddle captions to local SQLite FTS5 (text-only).
    save_huddle_transcripts: Arc<AtomicBool>,
    /// Active native Chime audio huddle session slot (at most one per workspace, guarded by
    /// atomic generation counter so stale joins/watchers never clobber a newer session).
    huddle: Arc<Mutex<HuddleSlot>>,
}

impl<B: Backend> Core<B> {
    pub async fn new(
        backend: B,
        store: SharedStore,
        workspace: TeamId,
        events: Emitter,
        huddle_media: HuddleMediaSender,
    ) -> Self {
        let ws_init = workspace.clone();
        let init_events = events.clone();
        let (raw_offline, outbox_items, cached_bots, raw_synced) = with_store(&store, move |s| {
            // A failed read is logged instead of silently becoming an empty queue, outbox, bot
            // cache or span map.
            fn load<T: Default>(what: &str, read: crate::store::Result<T>) -> T {
                match read {
                    Ok(value) => value,
                    Err(err) => {
                        tracing::warn!(what, %err, "failed to read from the store");
                        T::default()
                    }
                }
            }
            let offline = load("offline_queue", s.list_offline(&ws_init));
            // Rows left `sending` by an unclean exit stay invisible to every flush until recovered.
            if let Err(err) = s.reset_outbox_sending_to_pending(&ws_init) {
                tracing::warn!(%err, "failed to recover outbox rows left in sending");
                init_events.report("outbox recovery", err.into());
            }
            let outbox = load("outbox", s.list_outbox(&ws_init));
            let bots = load("bot_profiles", s.bot_profiles(&ws_init));
            let synced = load(BACKFILL_SYNCED_KEY, s.local(&ws_init, BACKFILL_SYNCED_KEY));
            Ok((offline, outbox, bots, synced))
        })
        .await
        .unwrap_or_default();

        let persisted: VecDeque<(i64, QueuedMutation)> = raw_offline
            .into_iter()
            .filter_map(|(id, payload)| {
                serde_json::from_str::<QueuedMutation>(&payload)
                    .ok()
                    .map(|m| (id, m))
            })
            .collect();
        if !persisted.is_empty() {
            events.emit(Event::OfflineQueueChanged(persisted.len()));
        }
        if !outbox_items.is_empty() {
            events.emit(Event::OutboxUpdated(outbox_items));
        }

        let tasks = TaskTracker::default();
        let unfurls = Unfurls::new(
            backend.clone(),
            Arc::clone(&store),
            workspace.clone(),
            events.clone(),
            tasks.clone(),
        );
        let synced: SyncedMap = Arc::new(Mutex::new(decode_synced_spans(raw_synced)));
        let backfill = Backfill {
            backend: backend.clone(),
            store: Arc::clone(&store),
            workspace: workspace.clone(),
            events: events.clone(),
            synced,
            page_delay: PAGE_DELAY,
            unfurls: unfurls.clone(),
            control: BackfillControl::new(workspace.clone()),
        };
        Self {
            backend,
            store,
            workspace,
            events,
            huddle_media,
            backfill,
            backfills: HashMap::new(),
            workspace_backfill: None,
            unfurls,
            tasks,
            rtm: None,
            presence_peers: Arc::new(Mutex::new(Vec::new())),
            users_asked: HashSet::new(),
            teams_asked: Arc::new(Mutex::new(HashSet::new())),
            bots: Arc::new(Mutex::new(cached_bots)),
            bots_asked: HashSet::new(),
            self_user_id: Arc::new(Mutex::new(None)),
            cloud_draft_ids: Arc::new(Mutex::new(HashMap::new())),
            draft_locks: Arc::new(Mutex::new(HashMap::new())),
            is_offline: false,
            has_connected_once: false,
            offline_queue: persisted,
            outbox_flush_lock: Arc::new(tokio::sync::Mutex::new(())),
            outbox_retry: OutboxRetry::default(),
            stealth_mode: Arc::new(AtomicBool::new(false)),
            save_huddle_transcripts: Arc::new(AtomicBool::new(true)),
            huddle: Arc::new(Mutex::new(HuddleSlot::default())),
        }
    }

    pub(super) fn ctx(&self) -> TaskCtx<B> {
        TaskCtx {
            backend: self.backend.clone(),
            store: Arc::clone(&self.store),
            workspace: self.workspace.clone(),
            events: self.events.clone(),
            unfurls: self.unfurls.clone(),
        }
    }

    fn spawn<F>(&self, context: &'static str, task: F)
    where
        F: Future<Output = Result<(), CoreError>> + Send + 'static,
    {
        spawn_tracked(&self.tasks, &self.events, context, task);
    }

    async fn shutdown(&mut self) {
        let active = {
            let mut slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
            slot.generation.fetch_add(1, Ordering::AcqRel);
            slot.joining = false;
            slot.call_id = None;
            slot.handle.take()
        };
        if let Some(handle) = active {
            handle
                .leave_and_wait(std::time::Duration::from_millis(1200))
                .await;
        }
        for (_, task) in self.backfills.drain() {
            task.abort();
        }
        if let Some(task) = self.workspace_backfill.take() {
            task.abort();
        }
        self.tasks.abort_all();
        if let Some(rtm) = self.rtm.take() {
            rtm.close();
        }
    }
}

pub(super) fn spawn_tracked<F>(
    tasks: &TaskTracker,
    events: &Emitter,
    context: &'static str,
    task: F,
) where
    F: Future<Output = Result<(), CoreError>> + Send + 'static,
{
    let events = events.clone();
    tasks.spawn(async move {
        if let Err(e) = task.await {
            events.report(context, e);
        }
    });
}

/// Unix seconds now; 0 if the clock is somehow before the epoch.
fn now_secs() -> i64 {
    now_epoch_secs()
}

#[cfg(test)]
pub(crate) mod tests;
