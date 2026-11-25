//! The runtime thread, the command loop and the front-end handle.

mod commands;
mod hydrate;
mod rtm;

use self::hydrate::*;

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::AbortHandle;

use crate::api::{ApiError, SlackApi};
use crate::auth::Session;
use crate::backend::Backend;
use crate::backfill::{Backfill, PAGE_DELAY, SyncedMap, wait_out_rate_limits};
use crate::fetcher::Fetcher;
use crate::rtm::ConnectionState;
use crate::rtm::{ConnectParams, RtmClient, RtmConfig, RtmEvent};
use crate::store::{SearchQuery, Store, StoreError};
use crate::types::event::MessageEvent;
use crate::types::{
    BotId, BotProfile, ChannelId, ConversationKind, Message, MessageSubtype, SlackEvent, TeamId,
    Ts, UserId,
};
use crate::unfurl::{Unfurls, event_messages};
use crate::{Command, Event, FailedMutation, apply_event};

/// Serializable offline mutation stored in SQLite `offline_queue` until flushed on reconnect.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
enum QueuedMutation {
    SendMessage {
        channel: ChannelId,
        content: crate::api::MessageContent,
        thread_ts: Option<Ts>,
        broadcast: bool,
    },
    AddReaction {
        channel: ChannelId,
        ts: Ts,
        name: String,
    },
    RemoveReaction {
        channel: ChannelId,
        ts: Ts,
        name: String,
    },
    MarkRead {
        channel: ChannelId,
        ts: Ts,
    },
}

impl QueuedMutation {
    fn from_command(command: &Command) -> Option<Self> {
        match command {
            Command::SendMessage {
                channel,
                content,
                thread_ts,
                broadcast,
            } => Some(Self::SendMessage {
                channel: channel.clone(),
                content: content.clone(),
                thread_ts: thread_ts.clone(),
                broadcast: *broadcast,
            }),
            Command::AddReaction { channel, ts, name } => Some(Self::AddReaction {
                channel: channel.clone(),
                ts: ts.clone(),
                name: name.clone(),
            }),
            Command::RemoveReaction { channel, ts, name } => Some(Self::RemoveReaction {
                channel: channel.clone(),
                ts: ts.clone(),
                name: name.clone(),
            }),
            Command::MarkRead { channel, ts } => Some(Self::MarkRead {
                channel: channel.clone(),
                ts: ts.clone(),
            }),
            _ => None,
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::SendMessage { content, .. } => {
                let text = content.text.trim();
                let preview: String = text.chars().take(48).collect();
                if text.chars().count() > 48 {
                    format!("Message \"{preview}…\"")
                } else if preview.is_empty() {
                    "Message".to_owned()
                } else {
                    format!("Message \"{preview}\"")
                }
            }
            Self::AddReaction { name, .. } => format!("Reaction :{name}:"),
            Self::RemoveReaction { name, .. } => format!("Remove reaction :{name}:"),
            Self::MarkRead { channel, .. } => format!("Mark read ({channel})"),
        }
    }
}

/// The store shared between the core (writer) and front-ends (readers).
///
/// SQLite is blocking: front-ends should lock it off their UI thread (or keep reads short).
pub type SharedStore = Arc<Mutex<Store>>;

/// Local search results returned per [`Command::Search`].
const SEARCH_LIMIT: usize = 200;

/// The orchestrator entry point.
pub struct Client;

fn shared_runtime() -> std::io::Result<&'static tokio::runtime::Runtime> {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt);
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("slackless-tokio")
        .enable_all()
        .build()?;
    Ok(RUNTIME.get_or_init(|| rt))
}

impl Client {
    /// Starts the core for one signed-in workspace on a shared multi-thread tokio runtime.
    ///
    /// Hydrates the store (`client.userBoot`, `client.counts`), connects the websocket and feeds
    /// every inbound event into `store`, then serves [`Command`]s until [`Command::Shutdown`] or
    /// until every command sender is dropped. The caller opens `store` (on disk for real sessions)
    /// so the location stays a front-end decision.
    pub fn start(session: Session, store: Store) -> std::io::Result<ClientHandle> {
        let store: SharedStore = Arc::new(Mutex::new(store));
        let (command_tx, command_rx) = mpsc::unbounded();
        let (event_tx, event_rx) = mpsc::unbounded();
        let events = Emitter(event_tx);
        let thread_store = Arc::clone(&store);
        let handle = shared_runtime()?.handle().clone();
        let files = SlackApi::new(
            &session.api_base(),
            session.xoxc_token.clone(),
            &session.d_cookie,
        )
        .ok();
        let fetcher = Fetcher::new(handle.clone(), files);
        let thread = std::thread::Builder::new()
            .name("slackless-core".into())
            .spawn(move || {
                handle.block_on(run_session(session, thread_store, command_rx, events));
            })?;
        Ok(ClientHandle {
            commands: command_tx,
            events: event_rx,
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

/// A front-end's connection to a running core: a [`Command`] sender, an [`Event`] receiver and
/// the shared store. The channels are `futures::channel::mpsc`, so any executor (GPUI's smol,
/// a CLI's tokio) can drive them.
pub struct ClientHandle {
    commands: UnboundedSender<Command>,
    events: UnboundedReceiver<Event>,
    store: SharedStore,
    fetcher: Fetcher,
    thread: JoinHandle<()>,
}

impl ClientHandle {
    pub fn send(&self, command: Command) -> Result<(), Closed> {
        self.commands.unbounded_send(command).map_err(|_| Closed)
    }

    /// Downloads public assets (profile photos, emoji, icons) on the core runtime.
    pub fn fetcher(&self) -> Fetcher {
        self.fetcher.clone()
    }

    /// A cloneable command sender, e.g. for views that issue commands.
    pub fn commands(&self) -> UnboundedSender<Command> {
        self.commands.clone()
    }

    /// The event stream; ends when the core stops.
    pub fn events(&mut self) -> &mut UnboundedReceiver<Event> {
        &mut self.events
    }

    pub fn store(&self) -> &SharedStore {
        &self.store
    }

    /// Splits the handle so the event receiver can move into a drain task. The runtime thread
    /// keeps running detached until a [`Command::Shutdown`] or every sender is dropped.
    pub fn into_parts(
        self,
    ) -> (
        UnboundedSender<Command>,
        UnboundedReceiver<Event>,
        SharedStore,
    ) {
        (self.commands, self.events, self.store)
    }

    /// Like [`Self::into_parts`], also returning the core runtime thread handle so the caller
    /// can join it during graceful application quit after sending [`Command::Shutdown`].
    pub fn into_parts_with_thread(
        self,
    ) -> (
        UnboundedSender<Command>,
        UnboundedReceiver<Event>,
        SharedStore,
        JoinHandle<()>,
    ) {
        (self.commands, self.events, self.store, self.thread)
    }

    /// Stops the core and waits for its thread to exit.
    pub fn shutdown(self) -> std::thread::Result<()> {
        let _ = self.commands.unbounded_send(Command::Shutdown);
        self.thread.join()
    }
}

/// Failures inside the core, reported to the front-end as [`Event`]s.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CoreError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Store(#[from] StoreError),
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
        self.emit(match err {
            CoreError::Api(ApiError::AuthExpired(code)) => Event::AuthExpired(code),
            err => Event::Error(format!("{context}: {err}")),
        });
    }
}

/// Runs `f` against the store on tokio's blocking pool.
pub(crate) async fn with_store<T, F>(store: &SharedStore, f: F) -> T
where
    F: FnOnce(&mut Store) -> T + Send + 'static,
    T: Send + 'static,
{
    let store = Arc::clone(store);
    let task = tokio::task::spawn_blocking(move || {
        f(&mut store.lock().unwrap_or_else(PoisonError::into_inner))
    });
    match task.await {
        Ok(value) => value,
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    }
}

async fn run_session(
    session: Session,
    store: SharedStore,
    commands: UnboundedReceiver<Command>,
    events: Emitter,
) {
    let api = match SlackApi::new(
        &session.api_base(),
        session.xoxc_token.clone(),
        &session.d_cookie,
    ) {
        Ok(api) => api,
        Err(e) => return events.report("api client", e.into()),
    };
    let mut core = Core::new(api, store, session.team_id.clone(), events.clone());
    core.hydrate().await;

    let mut config =
        RtmConfig::new(ConnectParams::new(session.xoxc_token.clone()).with_team(&session.team_id));
    config.d_cookie = Some(session.d_cookie.clone());
    let rtm = match RtmClient::new(config) {
        Ok((client, rx, driver)) => {
            tokio::spawn(driver);
            core.rtm = Some(client);
            Some(rx)
        }
        Err(e) => {
            events.emit(Event::Error(format!("websocket: {e}")));
            None
        }
    };
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
                Some(command) => core.handle(command),
            },
            event = next_rtm(&mut rtm) => core.on_rtm(event).await,
        }
    }
    core.shutdown().await;
}

/// The next websocket event; pends forever once the socket stream has ended.
async fn next_rtm(rtm: &mut Option<tokio::sync::mpsc::Receiver<RtmEvent>>) -> RtmEvent {
    if let Some(rx) = rtm {
        if let Some(event) = rx.recv().await {
            return event;
        }
    }
    *rtm = None;
    std::future::pending().await
}

#[derive(Default)]
pub(super) struct HuddleSlot {
    pub generation: u64,
    pub joining: bool,
    pub handle: Option<crate::huddle::HuddleHandle>,
    pub call_id: Option<String>,
}

pub(crate) struct Core<B: Backend> {
    backend: B,
    store: SharedStore,
    workspace: TeamId,
    events: Emitter,
    backfill: Backfill<B>,
    backfills: HashMap<ChannelId, AbortHandle>,
    /// Names the foreign channels shared-message unfurls point at (shared with `backfill`).
    unfurls: Unfurls<B>,
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
    /// Active native Chime audio huddle session slot (at most one per workspace, guarded by
    /// generation counter so stale joins/watchers never clobber a newer session).
    huddle: Arc<Mutex<HuddleSlot>>,
}

impl<B: Backend> Core<B> {
    pub fn new(backend: B, store: SharedStore, workspace: TeamId, events: Emitter) -> Self {
        let persisted: VecDeque<(i64, QueuedMutation)> = store
            .lock()
            .ok()
            .and_then(|s| s.list_offline(&workspace).ok())
            .unwrap_or_default()
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
        let unfurls = Unfurls::new(
            backend.clone(),
            Arc::clone(&store),
            workspace.clone(),
            events.clone(),
        );
        let backfill = Backfill {
            backend: backend.clone(),
            store: Arc::clone(&store),
            workspace: workspace.clone(),
            events: events.clone(),
            synced: SyncedMap::default(),
            page_delay: PAGE_DELAY,
            unfurls: unfurls.clone(),
        };
        let cached_bots = store
            .lock()
            .ok()
            .and_then(|s| s.bot_profiles(&workspace).ok())
            .unwrap_or_default();
        Self {
            backend,
            store,
            workspace,
            events,
            backfill,
            backfills: HashMap::new(),
            unfurls,
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
            huddle: Arc::new(Mutex::new(HuddleSlot::default())),
        }
    }

    fn spawn<F>(&self, context: &'static str, task: F)
    where
        F: Future<Output = Result<(), CoreError>> + Send + 'static,
    {
        let events = self.events.clone();
        tokio::spawn(async move {
            if let Err(e) = task.await {
                events.report(context, e);
            }
        });
    }

    async fn shutdown(&mut self) {
        let active = {
            let mut slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
            slot.generation = slot.generation.wrapping_add(1);
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
        if let Some(rtm) = self.rtm.take() {
            rtm.close();
        }
    }
}

/// Unix seconds now; 0 if the clock is somehow before the epoch.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
pub(crate) mod tests;
