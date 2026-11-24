//! The runtime thread, the command loop and the front-end handle.

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

impl Client {
    /// Starts the core for one signed-in workspace on a dedicated OS thread running a
    /// multi-thread tokio runtime.
    ///
    /// The thread hydrates the store (`client.userBoot`, `client.counts`), connects the
    /// websocket and feeds every inbound event into `store`, then serves [`Command`]s until
    /// [`Command::Shutdown`] or until every command sender is dropped. The caller opens `store`
    /// (on disk for real sessions) so the location stays a front-end decision.
    pub fn start(session: Session, store: Store) -> std::io::Result<ClientHandle> {
        let store: SharedStore = Arc::new(Mutex::new(store));
        let (command_tx, command_rx) = mpsc::unbounded();
        let (event_tx, event_rx) = mpsc::unbounded();
        let events = Emitter(event_tx);
        let thread_store = Arc::clone(&store);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("slackless-tokio")
            .enable_all()
            .build()?;
        let files = SlackApi::new(
            &session.api_base(),
            session.xoxc_token.clone(),
            &session.d_cookie,
        )
        .ok();
        let fetcher = Fetcher::new(runtime.handle().clone(), files);
        let thread = std::thread::Builder::new()
            .name("slackless-core".into())
            .spawn(move || {
                runtime.block_on(run_session(session, thread_store, command_rx, events));
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
    /// Bot profiles read through `bots.info` this session (see `attach_bot_profile`).
    bots: HashMap<BotId, BotProfile>,
    /// Bot ids already requested via `bots.info` this session (found or not): each is asked once.
    bots_asked: HashSet<BotId>,
    /// Signed-in user id learned from `client.userBoot` (used to mirror self profile/presence).
    self_user_id: Arc<Mutex<Option<UserId>>>,
    /// Cloud-synced draft ids per channel (`drafts.list` / `drafts.create` / `drafts.update` / `drafts.delete`).
    cloud_draft_ids: Arc<Mutex<HashMap<ChannelId, String>>>,
    /// Whether the RTM websocket is currently disconnected or reconnecting.
    is_offline: bool,
    /// Becomes `true` after the first RTM `Connected` transition so subsequent reconnects refresh
    /// `client.counts` and prefetch any messages missed while disconnected or asleep.
    has_connected_once: bool,
    /// Outgoing mutations persisted in SQLite `offline_queue`, flushed FIFO on reconnect.
    offline_queue: VecDeque<(i64, QueuedMutation)>,
    /// Active native Chime audio huddle session handle (at most one per workspace).
    active_huddle: Arc<Mutex<Option<crate::huddle::HuddleHandle>>>,
    /// Active Slack huddle `call_id` (`R0...`) for REST APIs (`screenhero.rooms.rename`,
    /// `rooms.startTranscription` / `rooms.stopTranscription`, `huddles.update`).
    active_huddle_call_id: Arc<Mutex<Option<String>>>,
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
            bots: cached_bots,
            bots_asked: HashSet::new(),
            self_user_id: Arc::new(Mutex::new(None)),
            cloud_draft_ids: Arc::new(Mutex::new(HashMap::new())),
            is_offline: false,
            has_connected_once: false,
            offline_queue: persisted,
            active_huddle: Arc::new(Mutex::new(None)),
            active_huddle_call_id: Arc::new(Mutex::new(None)),
        }
    }

    /// Startup hydrate: conversations, self, team and prefs from `client.userBoot`, the user
    /// directory, unread counts, then sidebar sections and custom emoji (one
    /// [`Event::SidebarChanged`] at the end). Every step reports its own failure without
    /// aborting the rest. Ephemeral messages left from a past session are dropped first.
    pub async fn hydrate(&self) {
        let ws = self.workspace.clone();
        if let Err(e) = with_store(&self.store, move |s| s.drop_ephemeral(&ws)).await {
            self.events.report("store", e.into());
        }
        match self.backend.user_boot().await {
            Ok(boot) => {
                let team = boot.team.clone();
                let prefs = boot.prefs.clone();
                let ws = self.workspace.clone();
                let stored = with_store(&self.store, move |store| {
                    // `client.userBoot` objects omit `is_member`, so a row the membership
                    // sync already marked must keep its flag: the front-end rebuilds its
                    // snapshot on this step's event, before `users.conversations` runs.
                    let ids: Vec<ChannelId> = boot
                        .channels
                        .iter()
                        .chain(&boot.ims)
                        .map(|conversation| conversation.id.clone())
                        .collect();
                    let member = store.member_flags(&ws, &ids)?;
                    let mut ids = Vec::new();
                    for mut conversation in boot.channels.into_iter().chain(boot.ims) {
                        conversation.is_member |= member.contains(&conversation.id);
                        store.upsert_conversation(&ws, &conversation)?;
                        ids.push(conversation.id.clone());
                    }
                    if let Some(me) = &boot.self_user {
                        store.upsert_user(&ws, me)?;
                    }
                    Ok::<_, StoreError>((ids, boot.self_user.map(|me| me.id)))
                })
                .await;
                match stored {
                    Ok((ids, me)) => {
                        self.events.emit(Event::Conversations(ids));
                        if let Some(me) = me {
                            if let Ok(mut guard) = self.self_user_id.lock() {
                                *guard = Some(me.clone());
                            }
                            self.events.emit(Event::UsersChanged(vec![me]));
                        }
                    }
                    Err(e) => self.events.report("hydrate", e.into()),
                }
                if let Some(team) = team {
                    let ws = self.workspace.clone();
                    let stored = with_store(&self.store, move |s| s.upsert_team(&ws, &team)).await;
                    if let Err(e) = stored {
                        self.events.report("hydrate team", e.into());
                    }
                }
                if let Some(prefs) = prefs {
                    let ws = self.workspace.clone();
                    let stored =
                        with_store(&self.store, move |s| s.replace_prefs(&ws, &prefs)).await;
                    if let Err(e) = stored {
                        self.events.report("hydrate prefs", e.into());
                    }
                }
            }
            Err(e) => self.events.report("client.userBoot", e.into()),
        }
        match self.backend.dm_list().await {
            Ok(dms) => {
                let ws = self.workspace.clone();
                let stored = with_store(&self.store, move |store| {
                    let mut ids = Vec::new();
                    for conversation in &dms {
                        // `client.userBoot` caps its channel list, so newer group DMs only
                        // ever arrive here; merging keeps boot's richer rows intact.
                        store.merge_conversation(&ws, conversation)?;
                        ids.push(conversation.id.clone());
                    }
                    Ok::<_, StoreError>(ids)
                })
                .await;
                match stored {
                    Ok(ids) if !ids.is_empty() => {
                        self.events.emit(Event::Conversations(ids));
                    }
                    Ok(_) => {}
                    Err(e) => self.events.report("client.dms", e.into()),
                }
            }
            Err(e) => self.events.report("client.dms", e.into()),
        }
        match self.backend.channel_memberships().await {
            Ok(members) => {
                let ws = self.workspace.clone();
                let ids = members.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
                let flipped =
                    with_store(&self.store, move |s| s.set_channel_memberships(&ws, &ids)).await;
                match flipped {
                    Ok(flipped) if !flipped.is_empty() => {
                        self.events.emit(Event::Conversations(flipped));
                    }
                    Ok(_) => {}
                    Err(e) => self.events.report("users.conversations", e.into()),
                }
            }
            // Slack Connect channels hosted by another org never reach `client.counts`, so
            // the GUI's membership gate needs this row state; failing it leaves the boot
            // flags (usually `false`) in place until the next hydrate.
            Err(e) => self.events.report("users.conversations", e.into()),
        }
        match self.backend.users().await {
            Ok(users) => {
                let ws = self.workspace.clone();
                let stored = with_store(&self.store, move |store| {
                    for user in &users {
                        store.upsert_user(&ws, user)?;
                    }
                    Ok::<_, StoreError>(users.into_iter().map(|user| user.id).collect())
                })
                .await;
                match stored {
                    Ok(ids) => {
                        self.events.emit(Event::UsersChanged(ids));
                    }
                    Err(e) => self.events.report("hydrate users", e.into()),
                }
            }
            Err(e) => self.events.report("users.list", e.into()),
        }
        match self.backend.counts().await {
            Ok(counts) => {
                self.prefetch_unreads(&counts);
                self.events.emit(Event::Counts(Box::new(counts)));
            }
            Err(e) => self.events.report("client.counts", e.into()),
        }
        match self.backend.channel_sections().await {
            Ok(sections) => {
                let ws = self.workspace.clone();
                let stored =
                    with_store(&self.store, move |s| s.replace_sections(&ws, &sections)).await;
                if let Err(e) = stored {
                    self.events.report("hydrate sections", e.into());
                }
            }
            Err(e) => self.events.report("users.channelSections.list", e.into()),
        }
        match self.backend.custom_emoji().await {
            Ok(emoji) => {
                let ws = self.workspace.clone();
                let emoji: Vec<(String, String)> = emoji.into_iter().collect();
                let stored =
                    with_store(&self.store, move |s| s.replace_custom_emoji(&ws, &emoji)).await;
                if let Err(e) = stored {
                    self.events.report("hydrate emoji", e.into());
                }
            }
            Err(e) => self.events.report("emoji.list", e.into()),
        }
        if let Ok(groups) = self.backend.usergroups().await {
            let active: Vec<_> = groups
                .into_iter()
                .filter(|g| !g.is_deleted() && !g.handle.is_empty())
                .collect();
            if !active.is_empty() {
                self.events.emit(Event::Usergroups(active));
            }
        }
        if let Ok(commands) = self.backend.app_commands().await {
            if !commands.is_empty() {
                self.events.emit(Event::AppCommands(commands));
            }
        }
        self.events.emit(Event::SidebarChanged);
        self.fetch_dm_previews();
        // The Activity badges need the feed's unread state; last, so the boot's own events
        // keep their order. The boot fetch skips the `messages.list` hydration (the view
        // asks for it with bodies when it opens).
        self.fetch_activity(false);
        self.fetch_saved();
        if let Some(me) = self.self_user_id.lock().ok().and_then(|g| g.clone()) {
            let (backend, events) = (self.backend.clone(), self.events.clone());
            self.spawn("dnd.info (self)", async move {
                if let Ok(status) = backend.dnd_info(me).await {
                    if status.snooze_enabled {
                        events.emit(Event::SelfDndUpdated(status));
                    }
                }
                Ok(())
            });
        }
    }

    /// Later / Saved items (`saved.list` across active, completed, and archived tabs), hydrated
    /// through `messages.list` so the Saved view and message markers stay synced across devices.
    fn fetch_saved(&self) {
        let (backend, events) = (self.backend.clone(), self.events.clone());
        self.spawn("saved.list", async move {
            let loaded = load_saved_items(&backend, &events).await;
            answer_load(&events, loaded, Event::SavedLoaded)
        });
    }

    /// The Activity feed (`activity.feed`): announces [`Event::ActivityView`] when it lands,
    /// or the same event carrying the error text when it does not. `with_bodies` also
    /// hydrates the rows through `messages.list` (the view wants bodies; the boot fetch
    /// that feeds the badges does not).
    fn fetch_activity(&self, with_bodies: bool) {
        let (backend, events) = (self.backend.clone(), self.events.clone());
        self.spawn("activity.feed", async move {
            let fetched = backend.activity_feed().await;
            match fetched {
                Ok(mut feed) => {
                    if with_bodies {
                        hydrate_activity(&backend, &events, &mut feed).await;
                    }
                    events.emit(Event::ActivityView {
                        feed: Ok(Box::new(feed)),
                    });
                    Ok(())
                }
                Err(e) => {
                    events.emit(Event::ActivityView {
                        feed: Err(e.to_string()),
                    });
                    Err(e.into())
                }
            }
        });
    }

    /// Background-prefetch the newest messages of unread conversations reported by
    /// `client.counts` whose local SQLite cache is behind `latest`, so clicking an unread channel
    /// or DM renders the new messages on frame 0 without waiting for an HTTP round-trip.
    fn prefetch_unreads(&self, counts: &crate::api::ClientCounts) {
        /// Cap per hydrate/reconnect pass so workspaces with hundreds of muted unread channels
        /// stay well within Slack's Tier-3 rate limits.
        const MAX_PREFETCH: usize = 25;

        let mut targets: Vec<(ChannelId, Option<Ts>, u32, bool)> = Vec::new();
        for entry in counts.ims.iter().chain(&counts.mpims) {
            if entry.is_unread() {
                targets.push((
                    entry.id.clone(),
                    entry.latest.clone(),
                    entry.effective_mentions(),
                    true,
                ));
            }
        }
        for entry in &counts.channels {
            if entry.is_unread() {
                targets.push((
                    entry.id.clone(),
                    entry.latest.clone(),
                    entry.effective_mentions(),
                    false,
                ));
            }
        }
        if targets.is_empty() {
            return;
        }
        // Prioritize DMs and mentions first, then newest activity first.
        targets.sort_by(|a, b| {
            b.3.cmp(&a.3)
                .then_with(|| b.2.cmp(&a.2))
                .then_with(|| b.1.cmp(&a.1))
        });
        targets.truncate(MAX_PREFETCH);

        let (backend, store, ws, events, unfurls) = (
            self.backend.clone(),
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
            self.unfurls.clone(),
        );
        self.spawn("unread prefetch", async move {
            let mut fetched = 0usize;
            for (channel, latest, _, _) in targets {
                // Skip channels whose local archive already contains `latest`.
                if let Some(ref latest_ts) = latest {
                    if !latest_ts.as_str().is_empty() {
                        let (ws_check, ch_check) = (ws.clone(), channel.clone());
                        let local_newest = with_store(&store, move |s| {
                            s.channel_history(&ws_check, &ch_check, None, 1)
                                .map(|rows| rows.into_iter().last().map(|r| r.message.ts))
                        })
                        .await?;
                        if local_newest
                            .as_ref()
                            .is_some_and(|local| local >= latest_ts)
                        {
                            continue;
                        }
                    }
                }
                if fetched >= 3 {
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                }
                let items = match wait_out_rate_limits(|| {
                    backend.recent_history(channel.clone(), crate::backend::UNREADS_HISTORY_LIMIT)
                })
                .await
                {
                    Ok(items) => items,
                    Err(_) => continue,
                };
                fetched += 1;
                if items.is_empty() {
                    continue;
                }
                unfurls.resolve(items.iter());
                let (ws_put, target) = (ws.clone(), channel.clone());
                with_store(&store, move |s| {
                    items
                        .iter()
                        .try_for_each(|message| s.upsert_message(&ws_put, &target, message))
                })
                .await?;
                events.emit(Event::StoreChanged { channel });
            }
            Ok(())
        });
    }

    /// DM-list previews: every IM/MPIM that is open (listed in Slack's DMs tab) and has
    /// nothing stored at all gets its newest message (one `conversations.history` limit-1
    /// call each, sequential and paced at the Tier-3 rate) so the DM list previews and sorts
    /// conversations never opened in slackless, as Slack does. Closed conversations stay
    /// unfetched. A fetched preview only adds one message: it touches neither the backfill's
    /// synced spans nor any completeness state, so opening the conversation still walks its
    /// full history from the newest page.
    fn fetch_dm_previews(&self) {
        let (backend, store, ws, events, unfurls) = (
            self.backend.clone(),
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
            self.unfurls.clone(),
        );
        self.spawn("dm previews", async move {
            let ws2 = ws.clone();
            let conversations = with_store(&store, move |s| s.conversations(&ws2)).await?;
            let dms: Vec<ChannelId> = conversations
                .iter()
                .filter(|c| {
                    matches!(c.kind(), ConversationKind::Im | ConversationKind::Mpim) && c.is_open
                })
                .map(|c| c.id.clone())
                .collect();
            let ws3 = ws.clone();
            let asked = dms.clone();
            let filled =
                with_store(&store, move |s| s.channels_with_messages(&ws3, &asked)).await?;
            let mut first = true;
            for channel in dms.iter().filter(|id| !filled.contains(*id)) {
                if !std::mem::take(&mut first) {
                    tokio::time::sleep(PAGE_DELAY).await;
                }
                let message =
                    wait_out_rate_limits(|| backend.latest_message(channel.clone())).await?;
                let Some(message) = message else {
                    continue;
                };
                unfurls.resolve(std::slice::from_ref(&message));
                let (ws4, target) = (ws.clone(), (*channel).clone());
                with_store(&store, move |s| s.upsert_message(&ws4, &target, &message)).await?;
                events.emit(Event::StoreChanged {
                    channel: (*channel).clone(),
                });
            }
            Ok(())
        });
    }

    pub fn handle(&mut self, command: Command) {
        if self.is_offline {
            if let Some(mutation) = QueuedMutation::from_command(&command) {
                let payload = serde_json::to_string(&mutation).unwrap_or_default();
                let id = self
                    .store
                    .lock()
                    .ok()
                    .and_then(|s| s.enqueue_offline(&self.workspace, &payload).ok())
                    .unwrap_or(0);
                self.offline_queue.push_back((id, mutation));
                self.events
                    .emit(Event::OfflineQueueChanged(self.offline_queue.len()));
                return;
            }
        }
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        match command {
            Command::OpenConversation(channel) => {
                self.open(channel.clone());
                self.merge_channel_info(channel.clone());
                self.fetch_peer_dnd(channel.clone());
                self.watch_peer_presence(channel);
            }
            Command::FetchLatestHistory(channel) => {
                let unfurls = self.unfurls.clone();
                self.spawn("conversations.history", async move {
                    let items = wait_out_rate_limits(|| {
                        backend
                            .recent_history(channel.clone(), crate::backend::UNREADS_HISTORY_LIMIT)
                    })
                    .await?;
                    unfurls.resolve(items.iter());
                    let target = channel.clone();
                    with_store(&store, move |s| {
                        items
                            .iter()
                            .try_for_each(|message| s.upsert_message(&ws, &target, message))
                    })
                    .await?;
                    events.emit(Event::StoreChanged { channel });
                    Ok(())
                });
            }
            Command::CloseConversation(channel) => {
                if let Some(task) = self.backfills.remove(&channel) {
                    task.abort();
                }
            }
            Command::ResolveUsers(ids) => self.resolve_users(ids),
            Command::ResolveBots(ids) => {
                for bot in ids {
                    if let Some(profile) = self.bots.get(&bot).cloned() {
                        self.events.emit(Event::BotProfileLoaded {
                            bot_id: bot,
                            profile,
                        });
                        continue;
                    }
                    if !self.bots_asked.insert(bot.clone()) {
                        continue;
                    }
                    let backend = self.backend.clone();
                    let store = Arc::clone(&self.store);
                    let ws = self.workspace.clone();
                    let events = self.events.clone();
                    self.spawn("bots.info", async move {
                        let profile = backend.bot_info(bot.clone()).await?;
                        let bot_store = bot.clone();
                        let profile_store = profile.clone();
                        let _ = with_store(&store, move |s| {
                            s.upsert_bot_profile(&ws, &bot_store, &profile_store)
                        })
                        .await;
                        events.emit(Event::BotProfileLoaded {
                            bot_id: bot,
                            profile,
                        });
                        Ok(())
                    });
                }
            }
            Command::SetNotifyLevel { channel, level } => {
                self.spawn("users.prefs.setNotifications", async move {
                    for &(name, value) in level.writes() {
                        backend
                            .set_notification_pref(channel.clone(), name, value)
                            .await?;
                    }
                    with_store(&store, move |s| {
                        let mut prefs = s.prefs(&ws)?.unwrap_or_default();
                        crate::notify::set_notify_level(&mut prefs, &channel, level);
                        s.replace_prefs(&ws, &prefs)
                    })
                    .await?;
                    events.emit(Event::SidebarChanged);
                    Ok(())
                });
            }
            Command::LeaveConversation(channel) => {
                self.spawn("conversations.leave", async move {
                    backend.leave_conversation(channel.clone()).await?;
                    let target = channel.clone();
                    with_store(&store, move |s| match s.conversation(&ws, &target)? {
                        Some(mut left) => {
                            left.is_member = false;
                            s.upsert_conversation(&ws, &left)
                        }
                        None => Ok(()),
                    })
                    .await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::JoinConversation(channel) => {
                self.spawn("conversations.join", async move {
                    let mut joined = backend.join_conversation(channel.clone()).await?;
                    joined.is_member = true;
                    with_store(&store, move |s| s.upsert_conversation(&ws, &joined)).await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::SetChannelTopic { channel, topic } => {
                self.spawn("conversations.setTopic", async move {
                    let updated = backend.set_topic(channel.clone(), topic).await?;
                    with_store(&store, move |s| s.merge_conversation(&ws, &updated)).await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::SetChannelPurpose { channel, purpose } => {
                self.spawn("conversations.setPurpose", async move {
                    let updated = backend.set_purpose(channel.clone(), purpose).await?;
                    with_store(&store, move |s| s.merge_conversation(&ws, &updated)).await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::OpenThread { channel, thread_ts } => {
                let backfill = self.backfill.clone();
                self.spawn("conversations.replies", async move {
                    backfill.fetch_thread(channel, thread_ts).await
                });
            }
            Command::SendMessage {
                channel,
                content,
                thread_ts,
                broadcast,
            } => {
                let unfurls = self.unfurls.clone();
                self.spawn("chat.postMessage", async move {
                    let posted = backend
                        .post_message(channel.clone(), content, thread_ts, broadcast)
                        .await?;
                    // Store the echo now; the websocket copy of it is an idempotent re-upsert.
                    if let Some(message) = posted.message {
                        unfurls.resolve(std::slice::from_ref(&message));
                        let target = channel.clone();
                        with_store(&store, move |s| s.upsert_message(&ws, &target, &message))
                            .await?;
                        events.emit(Event::StoreChanged { channel });
                    }
                    Ok(())
                });
            }
            // Edits, deletions and reactions reach the store through their websocket echo
            // (`message_changed`, `message_deleted`, `reaction_*`), which carries the edit ts.
            Command::EditMessage {
                channel,
                ts,
                content,
            } => self.spawn("chat.update", async move {
                backend.update_message(channel, ts, content).await?;
                Ok(())
            }),
            Command::DeleteMessage { channel, ts } => self.spawn("chat.delete", async move {
                Ok(backend.delete_message(channel, ts).await?)
            }),
            Command::AddReaction { channel, ts, name } => self.spawn("reactions.add", async move {
                Ok(backend.add_reaction(channel, ts, name).await?)
            }),
            Command::RemoveReaction { channel, ts, name } => {
                self.spawn("reactions.remove", async move {
                    Ok(backend.remove_reaction(channel, ts, name).await?)
                });
            }
            Command::RunCommand {
                channel,
                command,
                args,
                thread_ts,
            } => self.spawn("chat.command", async move {
                let ran = backend
                    .run_command(channel, command.clone(), args, thread_ts, ws)
                    .await;
                match ran {
                    Err(ApiError::Slack(error)) => {
                        events.emit(Event::CommandFailed { command, error });
                        Ok(())
                    }
                    ran => Ok(ran?),
                }
            }),
            Command::BlockAction {
                bot,
                app_id,
                team,
                action,
                container,
            } => self.spawn("blocks.actions", async move {
                let team = team.unwrap_or(ws);
                Ok(backend
                    .block_action(bot, app_id, team, action, container)
                    .await?)
            }),
            Command::Search(query) => self.spawn("search", async move {
                let parsed = SearchQuery::parse(&query);
                let messages =
                    with_store(&store, move |s| s.search(&ws, &parsed, SEARCH_LIMIT)).await?;
                events.emit(Event::SearchResults {
                    query: query.clone(),
                    messages,
                });
                let results = Box::new(backend.search(query.clone()).await?);
                events.emit(Event::RemoteSearchResults { query, results });
                Ok(())
            }),
            Command::MarkRead { channel, ts } => self.spawn("conversations.mark", async move {
                backend.mark_read(channel.clone(), ts.clone()).await?;
                let _ = with_store(&store, move |s| {
                    if let Some(mut conv) = s.conversation(&ws, &channel)? {
                        conv.last_read = Some(ts);
                        s.upsert_conversation(&ws, &conv)?;
                    }
                    Ok::<_, StoreError>(())
                })
                .await;
                Ok(())
            }),
            Command::MarkThreadRead {
                channel,
                thread_ts,
                ts,
            } => self.spawn("subscriptions.thread.mark", async move {
                Ok(backend.mark_thread_read(channel, thread_ts, ts).await?)
            }),
            Command::MarkAllRead => self.spawn("users.markAllRead", async move {
                backend.mark_all_read().await?;
                events.emit(Event::Counts(Box::new(backend.counts().await?)));
                Ok(())
            }),
            Command::SetThreadSubscribed {
                channel,
                thread_ts,
                last_read,
                subscribed,
            } => {
                let backfill = self.backfill.clone();
                let method = if subscribed {
                    "subscriptions.thread.add"
                } else {
                    "subscriptions.thread.remove"
                };
                self.spawn(method, async move {
                    backend
                        .set_thread_subscribed(
                            channel.clone(),
                            thread_ts.clone(),
                            last_read,
                            subscribed,
                        )
                        .await?;
                    // The stored root still carries the old follow flag.
                    backfill.fetch_thread(channel, thread_ts).await
                });
            }
            Command::FetchChannelManagers { channel } => {
                self.spawn("admin.roles.entity.listAssignments", async move {
                    let users = backend.channel_managers(channel.clone()).await?;
                    events.emit(Event::ChannelManagers { channel, users });
                    Ok(())
                });
            }
            Command::FetchThreads => {
                self.spawn("subscriptions.thread.getView", async move {
                    let view = Box::new(backend.threads_view().await?);
                    events.emit(Event::ThreadsView(view));
                    Ok(())
                });
            }
            Command::FetchActivity { with_bodies } => self.fetch_activity(with_bodies),
            Command::MarkActivity { entries, read } => {
                self.spawn("activity.mark", async move {
                    if read {
                        backend.activity_mark_read(entries).await?
                    } else {
                        backend.activity_mark_unread(entries).await?
                    }
                    Ok(())
                });
            }
            Command::OpenDm { users } => self.spawn("conversations.open", async move {
                let dm = backend.open_dm(users).await?;
                let channel = dm.id.clone();
                with_store(&store, move |s| s.upsert_conversation(&ws, &dm)).await?;
                events.emit(Event::Conversations(vec![channel.clone()]));
                events.emit(Event::ConversationOpened(channel));
                Ok(())
            }),
            Command::CreateSection { name } => {
                self.spawn("users.channelSections.create", async move {
                    backend.create_section(name).await?;
                    let sections = backend.channel_sections().await?;
                    with_store(&store, move |s| s.replace_sections(&ws, &sections)).await?;
                    events.emit(Event::SidebarChanged);
                    Ok(())
                });
            }
            Command::CreateSectionWithChannel { name, channel } => {
                self.spawn("users.channelSections.create", async move {
                    let created = backend.create_section(name).await?;
                    let section = created.channel_section_id;
                    // The channel may sit in another section: move, not just add.
                    let (probe, read_ws) = (channel.clone(), ws.clone());
                    let source = with_store(&store, move |s| {
                        s.sections(&read_ws).map(|sections| {
                            sections
                                .iter()
                                .find(|known| known.channel_ids.contains(&probe))
                                .map(|known| known.id.clone())
                        })
                    })
                    .await?;
                    backend
                        .move_channel(
                            Some((section.clone(), channel.clone())),
                            source.map(|from| (from, channel.clone())),
                        )
                        .await?;
                    let sections = backend.channel_sections().await?;
                    with_store(&store, move |s| s.replace_sections(&ws, &sections)).await?;
                    events.emit(Event::SidebarChanged);
                    Ok(())
                });
            }
            Command::RenameSection { section, name } => {
                self.spawn("users.channelSections.set", async move {
                    // `.set` clears every omitted key, so the emoji is re-read and resent.
                    let (read_ws, target) = (ws.clone(), section.clone());
                    let emoji = with_store(&store, move |s| {
                        s.sections(&read_ws).map(|sections| {
                            sections
                                .iter()
                                .find(|known| known.id == target)
                                .map(|known| known.emoji.clone())
                                .unwrap_or_default()
                        })
                    })
                    .await?;
                    backend.rename_section(section, name, emoji).await?;
                    let sections = backend.channel_sections().await?;
                    with_store(&store, move |s| s.replace_sections(&ws, &sections)).await?;
                    events.emit(Event::SidebarChanged);
                    Ok(())
                });
            }
            Command::DeleteSection { section } => {
                self.spawn("users.channelSections.delete", async move {
                    backend.delete_section(section).await?;
                    let sections = backend.channel_sections().await?;
                    with_store(&store, move |s| s.replace_sections(&ws, &sections)).await?;
                    events.emit(Event::SidebarChanged);
                    Ok(())
                });
            }
            Command::MoveChannel { channel, from, to } => {
                self.spawn("users.channelSections.channels.bulkUpdate", async move {
                    if from != to {
                        backend
                            .move_channel(
                                to.map(|to| (to, channel.clone())),
                                from.map(|from| (from, channel.clone())),
                            )
                            .await?;
                    }
                    let sections = backend.channel_sections().await?;
                    with_store(&store, move |s| s.replace_sections(&ws, &sections)).await?;
                    events.emit(Event::SidebarChanged);
                    Ok(())
                });
            }
            Command::Typing { channel, thread_ts } => {
                if let Some(Err(e)) = self.rtm.as_ref().map(|r| r.user_typing(channel, thread_ts)) {
                    events.emit(Event::Error(format!("typing: {e}")));
                }
            }
            Command::LoadPins(channel) => self.spawn("pins.list", async move {
                let pins = backend.pins(channel.clone()).await;
                answer_load(&events, pins, |pins| Event::PinsLoaded { channel, pins })
            }),
            Command::LoadBookmarks(channel) => self.spawn("bookmarks.list", async move {
                let bookmarks = backend.bookmarks(channel.clone()).await;
                answer_load(&events, bookmarks, |bookmarks| Event::BookmarksLoaded {
                    channel,
                    bookmarks,
                })
            }),
            Command::LoadFiles(filter) => self.spawn("files.list", async move {
                let files = backend.files(filter.clone()).await;
                answer_load(&events, files, |files| Event::FilesLoaded { filter, files })
            }),
            Command::AddBookmark {
                channel,
                title,
                url,
            } => {
                self.spawn("bookmarks.add", async move {
                    backend.add_bookmark(channel.clone(), title, url).await?;
                    let bookmarks = backend.bookmarks(channel.clone()).await;
                    answer_load(&events, bookmarks, |bookmarks| Event::BookmarksLoaded {
                        channel,
                        bookmarks,
                    })
                });
            }
            Command::SetPinned {
                channel,
                ts,
                pinned,
            } => {
                let method = if pinned { "pins.add" } else { "pins.remove" };
                self.spawn(method, async move {
                    backend
                        .set_pinned(channel.clone(), ts.clone(), pinned)
                        .await?;
                    let pins = backend.pins(channel.clone()).await;
                    answer_load(&events, pins, |pins| Event::PinsLoaded { channel, pins })
                });
            }
            Command::CreateChannelCanvas(channel) => {
                self.spawn("conversations.canvases.create", async move {
                    let canvas_id = backend.create_canvas(channel.clone()).await?;
                    // Refresh the stored conversation so `properties.canvas` lands; the tab
                    // row re-derives from the next snapshot.
                    let fresh = backend.conversation_info(channel.clone()).await?;
                    with_store(&store, move |s| s.merge_conversation(&ws, &fresh)).await?;
                    events.emit(Event::Conversations(vec![channel.clone()]));
                    events.emit(Event::ChannelCanvasCreated { channel, canvas_id });
                    Ok(())
                });
            }
            Command::SetDndSnooze { minutes } => {
                self.spawn("dnd.setSnooze", async move {
                    let status = backend.set_dnd_snooze(minutes).await?;
                    events.emit(Event::SelfDndUpdated(status));
                    Ok(())
                });
            }
            Command::EndDndSnooze => {
                self.spawn("dnd.endSnooze", async move {
                    let status = backend.end_dnd_snooze().await?;
                    events.emit(Event::SelfDndUpdated(status));
                    Ok(())
                });
            }
            Command::EndDnd => {
                self.spawn("dnd.endDnd", async move {
                    backend.dnd_end_dnd().await?;
                    events.emit(Event::SelfDndUpdated(slackless_types::DndStatus::default()));
                    Ok(())
                });
            }
            Command::LoadTeamDnd(users) => {
                self.spawn("dnd.teamInfo", async move {
                    let map = backend.dnd_team_info(users).await?;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs() as i64);
                    let paused = map
                        .into_iter()
                        .map(|(uid, status)| (uid, status.paused(now)))
                        .collect();
                    events.emit(Event::TeamDndLoaded(paused));
                    Ok(())
                });
            }
            Command::ScheduleMessage {
                channel,
                post_at,
                content,
                thread_ts,
            } => {
                self.spawn("chat.scheduleMessage", async move {
                    let created = backend
                        .schedule_message(channel.clone(), post_at, content, thread_ts)
                        .await?;
                    events.emit(Event::MessageScheduled {
                        channel: created.channel,
                        scheduled_message_id: created.scheduled_message_id,
                        post_at: created.post_at,
                    });
                    let list = backend.scheduled_messages().await;
                    answer_load(&events, list, Event::ScheduledMessagesLoaded)
                });
            }
            Command::LoadScheduledMessages => {
                self.spawn("chat.scheduledMessages.list", async move {
                    let list = backend.scheduled_messages().await;
                    answer_load(&events, list, Event::ScheduledMessagesLoaded)
                });
            }
            Command::DeleteScheduledMessage {
                channel,
                scheduled_message_id,
            } => {
                self.spawn("chat.deleteScheduledMessage", async move {
                    backend
                        .delete_scheduled_message(channel, scheduled_message_id)
                        .await?;
                    let list = backend.scheduled_messages().await;
                    answer_load(&events, list, Event::ScheduledMessagesLoaded)
                });
            }
            Command::UploadFiles {
                channel,
                thread_ts,
                content,
                files,
            } => {
                self.spawn("files.upload", async move {
                    let uploaded = backend
                        .upload_files(channel.clone(), thread_ts, content, files)
                        .await?;
                    events.emit(Event::FilesUploaded {
                        channel,
                        count: uploaded.len(),
                    });
                    Ok(())
                });
            }
            Command::SearchGifs { query } => {
                let method = if query.trim().is_empty() {
                    "gif.featured"
                } else {
                    "gif.search"
                };
                self.spawn(method, async move {
                    let trimmed = query.trim().to_owned();
                    let gifs = if trimmed.is_empty() {
                        backend.gif_featured(24).await?
                    } else {
                        backend.gif_search(trimmed.clone(), 24).await?
                    };
                    events.emit(Event::GifResults { query, gifs });
                    Ok(())
                });
            }
            Command::LoadSaved => self.fetch_saved(),
            Command::SaveItem {
                channel,
                ts,
                date_due,
            } => {
                self.spawn("saved.add", async move {
                    match backend
                        .saved_add(channel.clone(), ts.clone(), date_due)
                        .await
                    {
                        Ok(()) => {}
                        Err(ApiError::Slack(ref error))
                            if error == "already_saved" || error == "already_exists" =>
                        {
                            if let Some(due) = date_due {
                                backend
                                    .saved_update(channel, ts, None, Some(Some(due)))
                                    .await?;
                            }
                        }
                        Err(e) => return Err(e.into()),
                    }
                    let loaded = load_saved_items(&backend, &events).await;
                    answer_load(&events, loaded, Event::SavedLoaded)
                });
            }
            Command::UpdateSavedItem {
                channel,
                ts,
                mark,
                date_due,
            } => {
                self.spawn("saved.update", async move {
                    backend.saved_update(channel, ts, mark, date_due).await?;
                    let loaded = load_saved_items(&backend, &events).await;
                    answer_load(&events, loaded, Event::SavedLoaded)
                });
            }
            Command::DeleteSavedItem { channel, ts } => {
                self.spawn("saved.delete", async move {
                    backend.saved_delete(channel, ts).await?;
                    let loaded = load_saved_items(&backend, &events).await;
                    answer_load(&events, loaded, Event::SavedLoaded)
                });
            }
            Command::ClearCompletedSaved => {
                self.spawn("saved.clearCompleted", async move {
                    backend.saved_clear_completed().await?;
                    let loaded = load_saved_items(&backend, &events).await;
                    answer_load(&events, loaded, Event::SavedLoaded)
                });
            }
            Command::QuickSwitchSearch { query } => {
                self.spawn("edgeapi.search", async move {
                    let trimmed = query.trim().to_owned();
                    if trimmed.is_empty() {
                        return Ok(());
                    }
                    let results = backend.edge_search(ws, trimmed).await?;
                    events.emit(Event::QuickSwitchResults {
                        query,
                        results: Box::new(results),
                    });
                    Ok(())
                });
            }
            Command::SearchRemoteMessages { query, sort, page } => {
                self.spawn("search.modules.messages", async move {
                    let res = backend
                        .search_remote_messages(query.clone(), sort, page)
                        .await;
                    let emit = match res {
                        Ok(data) => Ok(Box::new(data)),
                        Err(e) => Err(e.to_string()),
                    };
                    events.emit(Event::RemoteMessagesPage {
                        query,
                        sort,
                        page,
                        result: emit,
                    });
                    Ok(())
                });
            }
            Command::SearchRemoteFiles { query, sort, page } => {
                self.spawn("search.modules.files", async move {
                    let res = backend.search_remote_files(query.clone(), sort, page).await;
                    let emit = match res {
                        Ok(data) => Ok(Box::new(data)),
                        Err(e) => Err(e.to_string()),
                    };
                    events.emit(Event::RemoteFilesPage {
                        query,
                        sort,
                        page,
                        result: emit,
                    });
                    Ok(())
                });
            }
            Command::UpdateProfile { update } => {
                let self_user_id = Arc::clone(&self.self_user_id);
                self.spawn("users.profile.set", async move {
                    let profile = backend.update_profile(update).await?;
                    let me = self_user_id.lock().ok().and_then(|g| g.clone());
                    if let Some(me) = me {
                        let me_for_store = me.clone();
                        let _ = with_store(&store, move |s| {
                            if let Some(mut user) =
                                s.users(&ws)?.into_iter().find(|u| u.id == me_for_store)
                            {
                                user.profile = profile;
                                s.upsert_user(&ws, &user)?;
                            }
                            Ok::<_, StoreError>(())
                        })
                        .await;
                        events.emit(Event::UsersChanged(vec![me]));
                    }
                    Ok(())
                });
            }
            Command::SetPresence { away } => {
                let self_user_id = Arc::clone(&self.self_user_id);
                self.spawn("presence.set", async move {
                    backend.set_presence(away).await?;
                    let me = self_user_id.lock().ok().and_then(|g| g.clone());
                    if let Some(me) = me {
                        events.emit(Event::Presence {
                            users: vec![me],
                            presence: if away {
                                crate::Presence::Away
                            } else {
                                crate::Presence::Active
                            },
                        });
                    }
                    Ok(())
                });
            }
            Command::SetUserPref { name, value } => {
                self.spawn("users.prefs.set", async move {
                    backend.set_user_pref(name, value).await?;
                    Ok(())
                });
            }
            Command::FetchUserProfile(user) => {
                self.spawn("users.profile.get", async move {
                    let (profile, labels) = tokio::join!(
                        backend.user_profile(user.clone()),
                        backend.team_profile_fields(),
                    );
                    let profile = profile?;
                    let labels = labels.unwrap_or_default();
                    let pronouns = profile.pronouns.clone().filter(|p| !p.trim().is_empty());
                    let mut fields = Vec::new();
                    if let Some(map) = profile.fields.as_ref().and_then(|v| v.as_object()) {
                        for (id, entry) in map {
                            let val = entry
                                .get("value")
                                .and_then(|v| v.as_str())
                                .map(str::trim)
                                .unwrap_or_default();
                            if val.is_empty() {
                                continue;
                            }
                            let label = entry
                                .get("label")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                                .map(str::to_owned)
                                .or_else(|| labels.get(id).cloned())
                                .unwrap_or_else(|| id.clone());
                            fields.push((label, val.to_owned()));
                        }
                    }
                    events.emit(Event::UserProfileLoaded {
                        user,
                        pronouns,
                        fields,
                    });
                    Ok(())
                });
            }
            Command::SubmitModal {
                view_id,
                hash,
                state_values,
            } => {
                self.spawn("views.submit", async move {
                    backend.views_submit(view_id, hash, state_values).await?;
                    Ok(())
                });
            }
            Command::CloseModal { view_id, hash } => {
                self.spawn("views.close", async move {
                    backend.views_close(view_id, hash).await?;
                    Ok(())
                });
            }
            Command::AddBookmarkInFolder {
                channel,
                title,
                url,
                parent_id,
            } => {
                self.spawn("bookmarks.add", async move {
                    backend
                        .add_bookmark_in_folder(channel.clone(), title, url, parent_id)
                        .await?;
                    let bookmarks = backend.bookmarks(channel.clone()).await;
                    answer_load(&events, bookmarks, |bookmarks| Event::BookmarksLoaded {
                        channel,
                        bookmarks,
                    })
                });
            }
            Command::RemoveBookmark {
                channel,
                bookmark_id,
            } => {
                self.spawn("bookmarks.remove", async move {
                    backend
                        .remove_bookmark(channel.clone(), bookmark_id)
                        .await?;
                    let bookmarks = backend.bookmarks(channel.clone()).await;
                    answer_load(&events, bookmarks, |bookmarks| Event::BookmarksLoaded {
                        channel,
                        bookmarks,
                    })
                });
            }
            Command::FetchCloudDrafts => {
                let draft_ids = Arc::clone(&self.cloud_draft_ids);
                self.spawn("drafts.list", async move {
                    let drafts = backend.drafts_list().await?;
                    if let Ok(mut guard) = draft_ids.lock() {
                        for d in &drafts {
                            guard.insert(d.channel.clone(), d.id.clone());
                        }
                    }
                    events.emit(Event::CloudDrafts(drafts));
                    Ok(())
                });
            }
            Command::SyncDraft { channel, text } => {
                let draft_ids = Arc::clone(&self.cloud_draft_ids);
                self.spawn("drafts.sync", async move {
                    let existing = draft_ids.lock().ok().and_then(|g| g.get(&channel).cloned());
                    if text.trim().is_empty() {
                        if let Some(id) = existing {
                            let _ = backend.drafts_delete(id).await;
                            if let Ok(mut guard) = draft_ids.lock() {
                                guard.remove(&channel);
                            }
                        }
                    } else if let Some(id) = existing {
                        let _ = backend.drafts_update(id, channel, text).await;
                    } else if let Ok(Some(new_id)) =
                        backend.drafts_create(channel.clone(), text).await
                    {
                        if let Ok(mut guard) = draft_ids.lock() {
                            guard.insert(channel, new_id);
                        }
                    }
                    Ok(())
                });
            }
            Command::BlockSuggestions {
                key,
                bot,
                app_id,
                team,
                action_id,
                block_id,
                value,
                container,
            } => {
                self.spawn("blocks.suggestions", async move {
                    let options = backend
                        .block_suggestions(bot, app_id, team, action_id, block_id, value, container)
                        .await?;
                    events.emit(Event::BlockSuggestions { key, options });
                    Ok(())
                });
            }
            Command::CreateChannel {
                name,
                is_private,
                purpose,
            } => {
                self.spawn("conversations.create", async move {
                    let mut created = backend.create_channel(name, is_private).await?;
                    let id = created.id.clone();
                    if let Some(purpose) = purpose.filter(|p| !p.trim().is_empty()) {
                        if let Ok(updated) = backend.set_purpose(id.clone(), purpose).await {
                            created = updated;
                        }
                    }
                    created.is_member = true;
                    let for_store = created.clone();
                    with_store(&store, move |s| s.upsert_conversation(&ws, &for_store)).await?;
                    events.emit(Event::Conversations(vec![id.clone()]));
                    events.emit(Event::ConversationOpened(id));
                    Ok(())
                });
            }
            Command::RenameChannel { channel, name } => {
                self.spawn("conversations.rename", async move {
                    let updated = backend.rename_channel(channel.clone(), name).await?;
                    with_store(&store, move |s| s.merge_conversation(&ws, &updated)).await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::ArchiveChannel(channel) => {
                self.spawn("conversations.archive", async move {
                    backend.archive_channel(channel.clone()).await?;
                    let id = channel.clone();
                    with_store(&store, move |s| {
                        if let Some(mut conv) = s.conversation(&ws, &id)? {
                            conv.is_archived = true;
                            s.upsert_conversation(&ws, &conv)?;
                        }
                        Ok::<_, StoreError>(())
                    })
                    .await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::UnarchiveChannel(channel) => {
                self.spawn("conversations.unarchive", async move {
                    backend.unarchive_channel(channel.clone()).await?;
                    let id = channel.clone();
                    with_store(&store, move |s| {
                        if let Some(mut conv) = s.conversation(&ws, &id)? {
                            conv.is_archived = false;
                            s.upsert_conversation(&ws, &conv)?;
                        }
                        Ok::<_, StoreError>(())
                    })
                    .await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::DeleteChannel(channel) => {
                self.spawn("admin.conversations.delete", async move {
                    backend.delete_channel(channel.clone()).await?;
                    let id = channel.clone();
                    with_store(&store, move |s| {
                        if let Some(mut conv) = s.conversation(&ws, &id)? {
                            conv.is_archived = true;
                            conv.is_member = false;
                            s.upsert_conversation(&ws, &conv)?;
                        }
                        Ok::<_, StoreError>(())
                    })
                    .await?;
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::ConvertChannelToPrivate(channel) => {
                self.spawn("conversations.convertToPrivate", async move {
                    let updated = backend.convert_channel_to_private(channel.clone()).await?;
                    with_store(&store, move |s| s.merge_conversation(&ws, &updated)).await?;
                    events.emit(Event::Conversations(vec![channel]));
                    events.emit(Event::SidebarChanged);
                    Ok(())
                });
            }
            Command::CreateUsergroup {
                name,
                handle,
                description,
            } => {
                self.spawn("usergroups.create", async move {
                    let _ = backend.create_usergroup(name, handle, description).await?;
                    if let Ok(groups) = backend.usergroups().await {
                        events.emit(Event::Usergroups(groups));
                    }
                    Ok(())
                });
            }
            Command::UpdateUsergroup {
                usergroup,
                name,
                handle,
                description,
            } => {
                self.spawn("usergroups.update", async move {
                    let _ = backend
                        .update_usergroup(usergroup, name, handle, description)
                        .await?;
                    if let Ok(groups) = backend.usergroups().await {
                        events.emit(Event::Usergroups(groups));
                    }
                    Ok(())
                });
            }
            Command::InviteToChannel { channel, users } => {
                if !users.is_empty() {
                    self.spawn("conversations.invite", async move {
                        let updated = backend.invite_to_channel(channel.clone(), users).await?;
                        let ws_merge = ws.clone();
                        with_store(&store, move |s| s.merge_conversation(&ws_merge, &updated))
                            .await?;
                        if let Ok(members) = backend.conversation_members(channel.clone()).await {
                            let for_counts = members.clone();
                            let counts =
                                with_store(&store, move |s| s.member_counts(&ws, &for_counts))
                                    .await
                                    .ok();
                            events.emit(Event::ConversationInfo {
                                channel: channel.clone(),
                                num_members: counts.map(|c| c.humans),
                                num_apps: counts.map(|c| c.apps),
                                members,
                            });
                        }
                        events.emit(Event::Conversations(vec![channel]));
                        Ok(())
                    });
                }
            }
            Command::RetryOfflineMutation(id) => {
                let found = self
                    .store
                    .lock()
                    .ok()
                    .and_then(|s| s.list_offline(&self.workspace).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .find(|(row_id, _)| *row_id == id)
                    .and_then(|(_, payload)| serde_json::from_str::<QueuedMutation>(&payload).ok());
                if let Some(mutation) = found {
                    self.flush_queued_mutation(id, mutation);
                }
            }
            Command::DiscardOfflineMutation(id) => {
                self.offline_queue.retain(|(row_id, _)| *row_id != id);
                if let Ok(s) = self.store.lock() {
                    let _ = s.delete_offline(id);
                }
                self.events
                    .emit(Event::OfflineQueueChanged(self.offline_queue.len()));
            }
            Command::SignOut => {
                self.spawn("auth.revoke", async move {
                    let _ = backend.auth_revoke().await;
                    Ok(())
                });
            }
            Command::AddConversationTab {
                channel,
                tab_type,
                label,
            } => {
                self.spawn("conversations.addTab", async move {
                    backend
                        .add_conversation_tab(channel.clone(), tab_type, label)
                        .await?;
                    if let Ok(fresh) = backend.conversation_info(channel.clone()).await {
                        with_store(&store, move |s| s.merge_conversation(&ws, &fresh)).await?;
                    }
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::RemoveConversationTab { channel, tab_id } => {
                self.spawn("conversations.removeTab", async move {
                    backend
                        .remove_conversation_tab(channel.clone(), tab_id)
                        .await?;
                    if let Ok(fresh) = backend.conversation_info(channel.clone()).await {
                        with_store(&store, move |s| s.merge_conversation(&ws, &fresh)).await?;
                    }
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::MoveConversationTab {
                channel,
                tab_id,
                after,
            } => {
                self.spawn("conversations.moveTab", async move {
                    backend
                        .move_conversation_tab(channel.clone(), tab_id, after)
                        .await?;
                    if let Ok(fresh) = backend.conversation_info(channel.clone()).await {
                        with_store(&store, move |s| s.merge_conversation(&ws, &fresh)).await?;
                    }
                    events.emit(Event::Conversations(vec![channel]));
                    Ok(())
                });
            }
            Command::SaveFile { file_id, date_due } => {
                self.spawn("saved.add(file)", async move {
                    match backend.save_file(file_id, date_due).await {
                        Ok(()) => {}
                        Err(ApiError::Slack(ref error))
                            if error == "already_saved" || error == "already_exists" => {}
                        Err(e) => return Err(e.into()),
                    }
                    let loaded = load_saved_items(&backend, &events).await;
                    answer_load(&events, loaded, Event::SavedLoaded)
                });
            }
            Command::DeleteFile { file_id } => {
                self.spawn("files.delete", async move {
                    backend.delete_file(file_id.clone()).await?;
                    events.emit(Event::FileDeleted { file_id });
                    Ok(())
                });
            }
            Command::LoadCanvas { file_id } => {
                self.spawn("files.info(canvas)", async move {
                    match backend.fetch_canvas_html(file_id.clone()).await {
                        Ok((title, html)) => {
                            events.emit(Event::CanvasLoaded {
                                file_id,
                                title,
                                html: Ok(html),
                            });
                        }
                        Err(err) => {
                            events.emit(Event::CanvasLoaded {
                                file_id,
                                title: "Canvas".to_owned(),
                                html: Err(err.to_string()),
                            });
                        }
                    }
                    Ok(())
                });
            }
            Command::CreateCanvas {
                title,
                markdown,
                channel,
            } => {
                self.spawn("canvases.create", async move {
                    let created = backend
                        .canvases_create(
                            Some(title.clone()),
                            (!markdown.is_empty()).then_some(markdown),
                            channel.clone(),
                        )
                        .await?;
                    events.emit(Event::CanvasCreated {
                        file_id: created.canvas_id,
                        title,
                        channel,
                    });
                    Ok(())
                });
            }
            Command::EditCanvas { file_id, op } => {
                self.spawn("canvases.edit", async move {
                    backend.canvases_edit(file_id.clone(), op).await?;
                    match backend.fetch_canvas_html(file_id.clone()).await {
                        Ok((title, html)) => {
                            events.emit(Event::CanvasLoaded {
                                file_id,
                                title,
                                html: Ok(html),
                            });
                        }
                        Err(err) => {
                            events.emit(Event::CanvasLoaded {
                                file_id,
                                title: "Canvas".to_owned(),
                                html: Err(err.to_string()),
                            });
                        }
                    }
                    Ok(())
                });
            }
            Command::RequestFilePermission {
                file_id,
                request_type,
            } => {
                self.spawn("files.requestPermission", async move {
                    backend
                        .files_request_permission(file_id.clone(), request_type)
                        .await?;
                    events.emit(Event::FilePermissionRequested { file_id });
                    Ok(())
                });
            }
            Command::LoadList { file_id } => {
                self.spawn("lists.records.list", async move {
                    let list = backend
                        .fetch_list(file_id.clone())
                        .await
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                    events.emit(Event::ListLoaded { file_id, list });
                    Ok(())
                });
            }
            Command::CreateList {
                name,
                description,
                channel,
            } => {
                self.spawn("lists.create", async move {
                    let file_id = backend
                        .lists_create(name.clone(), description, channel)
                        .await?;
                    events.emit(Event::ListCreated { file_id, name });
                    Ok(())
                });
            }
            Command::CreateListItem {
                list_id,
                initial_fields,
            } => {
                self.spawn("lists.records.create", async move {
                    let _ = backend
                        .lists_records_create(list_id.clone(), initial_fields)
                        .await?;
                    let list = backend
                        .fetch_list(list_id.clone())
                        .await
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                    events.emit(Event::ListLoaded {
                        file_id: list_id,
                        list,
                    });
                    Ok(())
                });
            }
            Command::UpdateListCell {
                list_id,
                row_id,
                cells,
            } => {
                self.spawn("lists.cells.update", async move {
                    backend
                        .lists_cells_update(list_id.clone(), row_id, cells)
                        .await?;
                    let list = backend
                        .fetch_list(list_id.clone())
                        .await
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                    events.emit(Event::ListLoaded {
                        file_id: list_id,
                        list,
                    });
                    Ok(())
                });
            }
            Command::DeleteListItem { list_id, row_id } => {
                self.spawn("lists.records.delete", async move {
                    backend
                        .lists_records_delete(list_id.clone(), row_id)
                        .await?;
                    let list = backend
                        .fetch_list(list_id.clone())
                        .await
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                    events.emit(Event::ListLoaded {
                        file_id: list_id,
                        list,
                    });
                    Ok(())
                });
            }
            Command::AddMessageToList {
                list_id,
                channel,
                ts,
            } => {
                self.spawn("lists.addToList", async move {
                    let record_id = backend
                        .lists_add_to_list(list_id.clone(), channel, ts)
                        .await?;
                    events.emit(Event::MessageAddedToList { list_id, record_id });
                    Ok(())
                });
            }
            Command::ArchiveListItem { list_id, row_id } => {
                self.spawn("lists.records.archive", async move {
                    backend
                        .lists_records_archive(list_id.clone(), row_id)
                        .await?;
                    let list = backend
                        .fetch_list(list_id.clone())
                        .await
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                    events.emit(Event::ListLoaded {
                        file_id: list_id,
                        list,
                    });
                    Ok(())
                });
            }
            Command::SetListAccess {
                list_id,
                access_level,
                channel,
            } => {
                self.spawn("slackLists.access.set", async move {
                    backend
                        .lists_access_set(list_id, access_level, channel)
                        .await?;
                    Ok(())
                });
            }
            Command::FetchListRecordComments { list_id, record_id } => {
                self.spawn("lists.getThreadsForRecord", async move {
                    let messages = backend
                        .lists_get_threads_for_record(list_id.clone(), record_id.clone())
                        .await
                        .unwrap_or_default();
                    events.emit(Event::ListRecordComments {
                        list_id,
                        record_id,
                        messages,
                    });
                    Ok(())
                });
            }
            Command::SetUserPhoto { filename, bytes } => {
                self.spawn("users.setPhoto", async move {
                    backend.users_set_photo(filename, bytes).await?;
                    Ok(())
                });
            }
            Command::LoadAppHome(channel) => {
                self.spawn("conversations.info(app_home)", async move {
                    let home = backend
                        .load_app_home(channel.clone())
                        .await
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                    events.emit(Event::AppHomeLoaded { channel, home });
                    Ok(())
                });
            }
            Command::OpenAppHome { app_id, team_id } => {
                self.spawn("apps.home.open", async move {
                    if let Some(channel) = backend.open_app_home(app_id, team_id).await? {
                        events.emit(Event::ConversationOpened(channel));
                    }
                    Ok(())
                });
            }
            Command::TripWorkflowTrigger { url, context } => {
                self.spawn("workflows.triggers.trip", async move {
                    backend.trip_workflow_trigger(url.clone(), context).await?;
                    events.emit(Event::WorkflowTripped { url });
                    Ok(())
                });
            }
            Command::SetGlobalNotificationPref { name, value, sync } => {
                self.spawn("users.prefs.setNotifications(global)", async move {
                    backend
                        .set_global_notification_pref(name, value, sync)
                        .await?;
                    Ok(())
                });
            }
            Command::LoadFilesExtras => {
                self.spawn("files.extras", async move {
                    let (favorites, collections, deleted) = tokio::join!(
                        backend.files_favorites_list(),
                        backend.files_collections_list(),
                        backend.files_recently_deleted(),
                    );
                    events.emit(Event::FilesExtrasLoaded {
                        favorites: favorites.unwrap_or_default(),
                        collections: collections.unwrap_or_default(),
                        deleted: deleted.unwrap_or_default(),
                    });
                    Ok(())
                });
            }
            Command::SetFileFavorite {
                file_id,
                collection_id,
                starred,
            } => {
                self.spawn("files.favorites", async move {
                    backend
                        .files_favorites_set(file_id, collection_id, starred)
                        .await?;
                    let (favorites, collections, deleted) = tokio::join!(
                        backend.files_favorites_list(),
                        backend.files_collections_list(),
                        backend.files_recently_deleted(),
                    );
                    events.emit(Event::FilesExtrasLoaded {
                        favorites: favorites.unwrap_or_default(),
                        collections: collections.unwrap_or_default(),
                        deleted: deleted.unwrap_or_default(),
                    });
                    Ok(())
                });
            }
            Command::CreateFileCollection { name, emoji } => {
                self.spawn("files.collections.create", async move {
                    let _ = backend.files_collections_create(name, emoji).await?;
                    let (favorites, collections, deleted) = tokio::join!(
                        backend.files_favorites_list(),
                        backend.files_collections_list(),
                        backend.files_recently_deleted(),
                    );
                    events.emit(Event::FilesExtrasLoaded {
                        favorites: favorites.unwrap_or_default(),
                        collections: collections.unwrap_or_default(),
                        deleted: deleted.unwrap_or_default(),
                    });
                    Ok(())
                });
            }
            Command::DeleteFileCollection { collection_id } => {
                self.spawn("files.collections.delete", async move {
                    backend.files_collections_delete(collection_id).await?;
                    let (favorites, collections, deleted) = tokio::join!(
                        backend.files_favorites_list(),
                        backend.files_collections_list(),
                        backend.files_recently_deleted(),
                    );
                    events.emit(Event::FilesExtrasLoaded {
                        favorites: favorites.unwrap_or_default(),
                        collections: collections.unwrap_or_default(),
                        deleted: deleted.unwrap_or_default(),
                    });
                    Ok(())
                });
            }
            Command::RestoreDeletedFile { file_id } => {
                self.spawn("files.undelete", async move {
                    backend.files_undelete(file_id.clone()).await?;
                    events.emit(Event::FileRestored { file_id });
                    let (favorites, collections, deleted) = tokio::join!(
                        backend.files_favorites_list(),
                        backend.files_collections_list(),
                        backend.files_recently_deleted(),
                    );
                    events.emit(Event::FilesExtrasLoaded {
                        favorites: favorites.unwrap_or_default(),
                        collections: collections.unwrap_or_default(),
                        deleted: deleted.unwrap_or_default(),
                    });
                    Ok(())
                });
            }
            Command::JoinHuddle { channel, audio } => {
                // Leave any currently running huddle first so only one audio session runs.
                if let Some(prev) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                {
                    prev.leave();
                }
                let active_huddle = Arc::clone(&self.active_huddle);
                let active_huddle_call_id = Arc::clone(&self.active_huddle_call_id);
                let team_id = self.workspace.clone();
                self.spawn("rooms.join", async move {
                    let joined = backend.rooms_join(channel.clone()).await?;
                    *active_huddle_call_id
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(joined.call_id.clone());
                    let creds = crate::huddle::HuddleCredentials {
                        channel: channel.clone(),
                        call_id: joined.call_id.clone(),
                        meeting_id: joined.chime.meeting.meeting_id,
                        attendee_id: joined.chime.attendee.attendee_id,
                        external_user_id: joined.chime.attendee.external_user_id,
                        join_token: joined.chime.attendee.join_token,
                        signaling_url: joined.chime.meeting.media_placement.signaling_url,
                        turn_control_url: joined.chime.meeting.media_placement.turn_control_url,
                        audio_host_url: joined.chime.meeting.media_placement.audio_host_url,
                        thread_root_ts: joined
                            .huddle
                            .thread_root_ts
                            .filter(|ts| !ts.as_str().trim().is_empty())
                            .or_else(|| {
                                joined
                                    .canvas
                                    .as_ref()
                                    .and_then(|c| c.root_thread_ts.clone())
                                    .filter(|ts| !ts.as_str().trim().is_empty())
                            }),
                        canvas_file_id: joined.canvas.as_ref().and_then(|c| c.file_id.clone()),
                        huddle_link: joined.huddle.huddle_link.or_else(|| {
                            Some(format!("https://app.slack.com/huddle/{team_id}/{channel}"))
                        }),
                    };
                    let (upd_tx, mut upd_rx) = tokio::sync::mpsc::unbounded_channel();
                    let handle = crate::huddle::spawn_huddle_session(creds, audio, upd_tx);
                    *active_huddle.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
                    let active_for_clear = Arc::clone(&active_huddle);
                    let call_id_for_clear = Arc::clone(&active_huddle_call_id);
                    tokio::spawn(async move {
                        while let Some(upd) = upd_rx.recv().await {
                            match upd {
                                crate::huddle::HuddleUpdate::State(snap) => {
                                    let done = matches!(
                                        snap.phase,
                                        crate::huddle::HuddlePhase::Idle
                                            | crate::huddle::HuddlePhase::Failed
                                    );
                                    events.emit(Event::HuddleSessionChanged(snap));
                                    if done {
                                        let _ = active_for_clear
                                            .lock()
                                            .unwrap_or_else(PoisonError::into_inner)
                                            .take();
                                        let _ = call_id_for_clear
                                            .lock()
                                            .unwrap_or_else(PoisonError::into_inner)
                                            .take();
                                    }
                                }
                                crate::huddle::HuddleUpdate::AudioLevels(levels) => {
                                    events.emit(Event::HuddleAudioLevels(levels));
                                }
                                crate::huddle::HuddleUpdate::VideoFrame { tile, frame } => {
                                    events.emit(Event::HuddleVideoFrame { tile, frame });
                                }
                                crate::huddle::HuddleUpdate::VideoTileClosed(tile) => {
                                    events.emit(Event::HuddleVideoTileClosed(tile));
                                }
                                crate::huddle::HuddleUpdate::DataEvent(ev) => {
                                    events.emit(Event::HuddleDataEvent(ev));
                                }
                            }
                        }
                    });
                    Ok(())
                });
            }
            Command::LeaveHuddle => {
                let _ = self
                    .active_huddle_call_id
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                {
                    handle.leave();
                }
            }
            Command::SetHuddleMuted(muted) => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetMuted(muted));
                }
            }
            Command::SetHuddleAudioDevices { input, output } => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetAudioDevices { input, output });
                }
            }
            Command::SetHuddleAudioProcessing { aec, ns, agc } => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetAudioProcessing { aec, ns, agc });
                }
            }
            Command::InviteToHuddle { channel, users } => {
                self.spawn("rooms.notifyMember", async move {
                    for user in users {
                        backend.rooms_notify_member(channel.clone(), user).await?;
                    }
                    Ok(())
                });
            }
            Command::CopyHuddleLink { channel } => {
                self.spawn("rooms.getLink", async move {
                    let url = backend.rooms_get_link(channel.clone()).await?;
                    events.emit(Event::HuddleLinkLoaded { channel, url });
                    Ok(())
                });
            }
            Command::TestHuddleSpeaker { output_device } => {
                let events = self.events.clone();
                tokio::task::spawn_blocking(move || {
                    if let Err(err) =
                        crate::huddle::play_speaker_test_tone(output_device.as_deref())
                    {
                        events.emit(Event::Error(format!("speaker test: {err}")));
                    }
                });
            }
            Command::SetHuddleCamera {
                enabled,
                device_index,
                blur,
            } => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetCamera {
                        enabled,
                        device_index,
                        blur,
                    });
                }
            }
            Command::SetHuddleBlurBackground(blur) => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetBlurBackground(blur));
                }
            }
            Command::SetHuddleScreenShare { source_id } => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetScreenShare { source_id });
                }
            }
            Command::SendHuddleReaction {
                emoji,
                kind,
                emoji_url,
            } => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SendReaction {
                        emoji,
                        kind,
                        emoji_url,
                    });
                }
            }
            Command::SetHuddleHandRaised(raised) => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetHandRaised(raised));
                }
            }
            Command::SetHuddleStickyEmoji(emoji) => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetStickyEmoji(emoji));
                }
            }
            Command::SendHuddleDraw(draw) => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SendDraw(draw));
                }
            }
            Command::SetHuddleJukebox(song) => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetJukebox(song));
                }
            }
            Command::MuteHuddlePeer(peer_id) => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::MutePeer(peer_id));
                }
            }
            Command::SetHuddleTopic(topic) => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetTopic(topic.clone()));
                }
                if let Some(call_id) = self
                    .active_huddle_call_id
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
                {
                    self.spawn("screenhero.rooms.rename", async move {
                        backend.rooms_rename(call_id, topic).await?;
                        Ok(())
                    });
                }
            }
            Command::SetHuddleBackground(background_id) => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetBackground(
                        (!background_id.is_empty() && background_id != "default")
                            .then(|| background_id.clone()),
                    ));
                }
                if let Some(call_id) = self
                    .active_huddle_call_id
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
                {
                    self.spawn("huddles.update", async move {
                        backend
                            .huddles_update_background(call_id, background_id)
                            .await?;
                        Ok(())
                    });
                }
            }
            Command::SetHuddleCaptions(enabled) => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetCaptionsEnabled(enabled));
                }
                if let Some(call_id) = self
                    .active_huddle_call_id
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
                {
                    self.spawn(
                        if enabled {
                            "rooms.startTranscription"
                        } else {
                            "rooms.stopTranscription"
                        },
                        async move {
                            if enabled {
                                backend.rooms_start_transcription(call_id).await?;
                            } else {
                                backend.rooms_stop_transcription(call_id).await?;
                            }
                            Ok(())
                        },
                    );
                }
            }
            Command::SetHuddleAudioOnly(enabled) => {
                if let Some(handle) = self
                    .active_huddle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    handle.send(crate::huddle::HuddleControl::SetAudioOnly(enabled));
                }
            }
            Command::DeclineHuddleInvite { channel, call_id } => {
                self.spawn("rooms.inviteResponse", async move {
                    backend
                        .rooms_invite_response(channel, call_id, "decline".to_owned())
                        .await?;
                    Ok(())
                });
            }
            // Handled by the loop.
            Command::Shutdown => {}
        }
    }

    /// Execute one persisted offline mutation; deletes the SQLite row on success or emits
    /// [`Event::OfflineMutationFailed`] on failure so the user can retry or discard it.
    fn flush_queued_mutation(&self, id: i64, mutation: QueuedMutation) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        let unfurls = self.unfurls.clone();
        let summary = mutation.summary();
        tokio::spawn(async move {
            let outcome: Result<(), CoreError> = async {
                match mutation {
                    QueuedMutation::SendMessage {
                        channel,
                        content,
                        thread_ts,
                        broadcast,
                    } => {
                        let posted = backend
                            .post_message(channel.clone(), content, thread_ts, broadcast)
                            .await?;
                        if let Some(message) = posted.message {
                            unfurls.resolve(std::slice::from_ref(&message));
                            let target = channel.clone();
                            with_store(&store, move |s| s.upsert_message(&ws, &target, &message))
                                .await?;
                            events.emit(Event::StoreChanged { channel });
                        }
                    }
                    QueuedMutation::AddReaction { channel, ts, name } => {
                        backend.add_reaction(channel, ts, name).await?;
                    }
                    QueuedMutation::RemoveReaction { channel, ts, name } => {
                        backend.remove_reaction(channel, ts, name).await?;
                    }
                    QueuedMutation::MarkRead { channel, ts } => {
                        backend.mark_read(channel, ts).await?;
                    }
                }
                Ok(())
            }
            .await;
            match outcome {
                Ok(()) => {
                    if id > 0 {
                        let _ = with_store(&store, move |s| s.delete_offline(id)).await;
                    }
                }
                Err(err) => {
                    events.emit(Event::OfflineMutationFailed(FailedMutation {
                        id,
                        summary,
                        error: err.to_string(),
                    }));
                }
            }
        });
    }

    /// Starts (or restarts) the backfill of `channel`, newest page first.
    fn open(&mut self, channel: ChannelId) {
        if self
            .backfills
            .get(&channel)
            .is_some_and(|task| !task.is_finished())
        {
            return;
        }
        let task = tokio::spawn(self.backfill.clone().run(channel.clone()));
        self.backfills.insert(channel, task.abort_handle());
    }

    async fn on_rtm(&mut self, event: RtmEvent) {
        match event {
            RtmEvent::State(state) => {
                // Subscriptions last one websocket session: (re)subscribe on every `hello`.
                match &state {
                    ConnectionState::Connected { .. } => {
                        let reconnected = self.has_connected_once;
                        self.has_connected_once = true;
                        self.is_offline = false;
                        self.subscribe_dm_presence();
                        if reconnected {
                            let (backend, store, ws, events, unfurls) = (
                                self.backend.clone(),
                                Arc::clone(&self.store),
                                self.workspace.clone(),
                                self.events.clone(),
                                self.unfurls.clone(),
                            );
                            self.spawn("reconnect counts", async move {
                                let counts = backend.counts().await?;
                                let unread_channels: Vec<(ChannelId, Option<Ts>)> = counts
                                    .ims
                                    .iter()
                                    .chain(&counts.mpims)
                                    .chain(&counts.channels)
                                    .filter(|c| c.is_unread())
                                    .take(25)
                                    .map(|c| (c.id.clone(), c.latest.clone()))
                                    .collect();
                                events.emit(Event::Counts(Box::new(counts)));
                                for (idx, (channel, latest)) in
                                    unread_channels.into_iter().enumerate()
                                {
                                    if let Some(ref latest_ts) = latest {
                                        if !latest_ts.as_str().is_empty() {
                                            let (ws_c, ch_c) = (ws.clone(), channel.clone());
                                            let local_newest = with_store(&store, move |s| {
                                                s.channel_history(&ws_c, &ch_c, None, 1).map(
                                                    |rows| {
                                                        rows.into_iter()
                                                            .last()
                                                            .map(|r| r.message.ts)
                                                    },
                                                )
                                            })
                                            .await?;
                                            if local_newest
                                                .as_ref()
                                                .is_some_and(|local| local >= latest_ts)
                                            {
                                                continue;
                                            }
                                        }
                                    }
                                    if idx >= 3 {
                                        tokio::time::sleep(std::time::Duration::from_millis(150))
                                            .await;
                                    }
                                    if let Ok(items) = wait_out_rate_limits(|| {
                                        backend.recent_history(
                                            channel.clone(),
                                            crate::backend::UNREADS_HISTORY_LIMIT,
                                        )
                                    })
                                    .await
                                    {
                                        if !items.is_empty() {
                                            unfurls.resolve(items.iter());
                                            let (ws_p, target) = (ws.clone(), channel.clone());
                                            let _ = with_store(&store, move |s| {
                                                items.iter().try_for_each(|m| {
                                                    s.upsert_message(&ws_p, &target, m)
                                                })
                                            })
                                            .await;
                                            events.emit(Event::StoreChanged { channel });
                                        }
                                    }
                                }
                                Ok(())
                            });
                        }
                        if !self.offline_queue.is_empty() {
                            let queued: Vec<(i64, QueuedMutation)> =
                                self.offline_queue.drain(..).collect();
                            self.events.emit(Event::OfflineQueueChanged(0));
                            for (id, mutation) in queued {
                                self.flush_queued_mutation(id, mutation);
                            }
                        }
                    }
                    ConnectionState::Reconnecting { .. } | ConnectionState::Disconnected { .. } => {
                        self.is_offline = true;
                    }
                    ConnectionState::Connecting => {}
                }
                self.events.emit(Event::Connection(state));
            }
            RtmEvent::Event(mut event) => {
                match &event {
                    SlackEvent::DndUpdated(ev) => {
                        self.events
                            .emit(Event::SelfDndUpdated(ev.dnd_status.clone()));
                    }
                    SlackEvent::DndUpdatedUser(ev) => {
                        if let Some(user) = &ev.user {
                            self.events.emit(Event::DndPaused {
                                user: user.clone(),
                                paused: ev.dnd_status.paused(now_secs()),
                            });
                        }
                    }
                    SlackEvent::ShRoomJoin(ev)
                    | SlackEvent::ShRoomUpdate(ev)
                    | SlackEvent::ShRoomLeave(ev) => {
                        let ended = !ev.room.is_active();
                        let participants = ev.room.participants.clone();
                        let topic = ev.room.name.clone();
                        let background_id = ev.room.background_id.clone();
                        let thread_root_ts = ev
                            .room
                            .thread_root_ts
                            .clone()
                            .filter(|ts| !ts.as_str().trim().is_empty());
                        for channel in ev.target_channels() {
                            self.events.emit(Event::HuddleUpdated {
                                channel,
                                participants: participants.clone(),
                                ended,
                                topic: topic.clone(),
                                background_id: background_id.clone(),
                                thread_root_ts: thread_root_ts.clone(),
                            });
                        }
                    }
                    SlackEvent::HuddleInvite(inv) => {
                        if !inv.channel_id.as_str().is_empty()
                            && !inv.sender_user_id.as_str().is_empty()
                        {
                            self.events.emit(Event::HuddleInvite {
                                channel: inv.channel_id.clone(),
                                inviter: inv.sender_user_id.clone(),
                                call_id: inv.call_id.clone(),
                            });
                        }
                    }
                    SlackEvent::HuddleInviteCancel(cancel) => {
                        self.events.emit(Event::HuddleInviteCancelled {
                            channel: (!cancel.channel_id.as_str().is_empty())
                                .then(|| cancel.channel_id.clone()),
                            call_id: None,
                        });
                    }
                    _ => {}
                }
                if matches!(&event, SlackEvent::EmojiChanged(_)) {
                    let (backend, store, ws, events) = (
                        self.backend.clone(),
                        Arc::clone(&self.store),
                        self.workspace.clone(),
                        self.events.clone(),
                    );
                    self.spawn("emoji.list", async move {
                        if let Ok(emoji) = backend.custom_emoji().await {
                            let pairs: Vec<(String, String)> = emoji.into_iter().collect();
                            let _ =
                                with_store(&store, move |s| s.replace_custom_emoji(&ws, &pairs))
                                    .await;
                            events.emit(Event::SidebarChanged);
                        }
                        Ok(())
                    });
                }
                if let SlackEvent::Unknown { r#type, raw } = &event {
                    if matches!(
                        r#type.as_str(),
                        "saved_added"
                            | "saved_updated"
                            | "saved_deleted"
                            | "saved_clear"
                            | "saved_due"
                    ) {
                        self.fetch_saved();
                    } else if r#type == "commands_changed" {
                        let (backend, events) = (self.backend.clone(), self.events.clone());
                        self.spawn("client.appCommands", async move {
                            if let Ok(commands) = backend.app_commands().await {
                                events.emit(Event::AppCommands(commands));
                            }
                            Ok(())
                        });
                    } else if r#type == "user_status_changed" {
                        if let Some(user_val) = raw.get("user") {
                            if let Ok(user) =
                                serde_json::from_value::<crate::types::User>(user_val.clone())
                            {
                                let id = user.id.clone();
                                let ws = self.workspace.clone();
                                let _ = with_store(&self.store, move |s| s.upsert_user(&ws, &user))
                                    .await;
                                self.events.emit(Event::UsersChanged(vec![id]));
                            }
                        }
                    } else if matches!(
                        r#type.as_str(),
                        "view_opened" | "view_open" | "view_updated" | "view_update"
                    ) {
                        let view = raw.get("view").cloned().unwrap_or_else(|| raw.clone());
                        if view.get("type").and_then(serde_json::Value::as_str) == Some("home") {
                            self.events.emit(Event::AppHomeUpdated(view));
                        } else {
                            self.events.emit(Event::ModalOpened(view));
                        }
                    } else if matches!(r#type.as_str(), "view_closed" | "view_close") {
                        let id = raw
                            .get("view")
                            .and_then(|v| v.get("id"))
                            .or_else(|| raw.get("view_id"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        self.events.emit(Event::ModalClosed(id));
                    }
                }
                if let Some(channel) = refresh_target(&event) {
                    self.refresh_conversation(channel);
                }
                self.attach_bot_profile(&mut event).await;
                self.unfurls.resolve(event_messages(&event));
                let posted = posted_message(&event);
                let ws = self.workspace.clone();
                let changed = with_store(&self.store, move |s| apply_event(s, &ws, event)).await;
                if let Some(changed) = changed {
                    self.events.emit(changed);
                }
                if let Some((channel, message)) = posted {
                    self.events.emit(Event::MessagePosted { channel, message });
                }
            }
        }
    }

    /// An ephemeral app message (Giphy's preview) carries only its `bot_id`: give it the
    /// bot's profile (`bots.info`, once per bot per session) so it shows the app's name and
    /// icon and its buttons know the app to dispatch to.
    async fn attach_bot_profile(&mut self, event: &mut SlackEvent) {
        let message = match event {
            SlackEvent::Message(MessageEvent::Message(message)) => message.as_mut(),
            SlackEvent::Message(MessageEvent::Changed(changed)) => changed.message.as_mut(),
            _ => return,
        };
        let Some(bot) = message
            .bot_id
            .clone()
            .filter(|_| message.bot_profile.is_none())
        else {
            return;
        };
        if !self.bots.contains_key(&bot) {
            match self.backend.bot_info(bot.clone()).await {
                Ok(profile) => {
                    self.bots.insert(bot.clone(), profile);
                }
                Err(e) => return self.events.report("bots.info", e.into()),
            }
        }
        message.bot_profile = self.bots.get(&bot).cloned();
    }

    /// Re-fetches a conversation whose event carries only its id.
    fn refresh_conversation(&self, channel: ChannelId) {
        let (backend, store, ws, events) = (
            self.backend.clone(),
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
        );
        self.spawn("conversations.info", async move {
            let conversation = backend.conversation_info(channel.clone()).await?;
            with_store(&store, move |s| s.upsert_conversation(&ws, &conversation)).await?;
            events.emit(Event::Conversations(vec![channel]));
            Ok(())
        });
    }

    /// Open-time `conversations.info`: metadata `client.userBoot` omits (topic, member
    /// counts) merged into the stored copy, so richer stored fields survive the refresh. The
    /// header's counts come from `conversations.members` cross-checked against the stored
    /// profiles (`Event::ConversationInfo` carries the split, plus the member id list for
    /// member panels and Slack Connect org counts); when the member list cannot be read,
    /// `conversations.info`'s raw `num_members` stands. A Slack Connect channel also resolves
    /// the connected organizations' `team.info` records, each asked once per session.
    fn merge_channel_info(&self, channel: ChannelId) {
        let (backend, store, ws, events, teams_asked) = (
            self.backend.clone(),
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
            Arc::clone(&self.teams_asked),
        );
        self.spawn("conversations.info", async move {
            let mut fresh = match backend.conversation_info(channel.clone()).await {
                Ok(fresh) => fresh,
                Err(e) => {
                    // Nothing is known; the announcement still lands so front-ends waiting on
                    // the member list stop waiting (the failure itself is reported).
                    events.emit(Event::ConversationInfo {
                        channel,
                        num_members: None,
                        num_apps: None,
                        members: Vec::new(),
                    });
                    return Err(e.into());
                }
            };
            let mut members = Vec::new();
            let mut num_apps = None;
            let mut external_teams = Vec::new();
            if let Ok(listed) = backend.conversation_members(channel.clone()).await {
                let (ws2, shared) = (ws.clone(), fresh.is_ext_shared);
                // One store read for both: the header's split and, for a Slack Connect
                // channel, the other organizations its stored people belong to. The list
                // rides the event only: stored on the row it would bloat every snapshot.
                let (counts, teams, listed) = with_store(&store, move |s| {
                    let counts = s.member_counts(&ws2, &listed)?;
                    let teams = if shared {
                        s.member_team_ids(&ws2, &listed)?
                    } else {
                        Vec::new()
                    };
                    Ok::<_, StoreError>((counts, teams, listed))
                })
                .await?;
                fresh.num_members = Some(counts.humans);
                num_apps = Some(counts.apps);
                external_teams = teams;
                members = listed;
            }
            let num_members = fresh.num_members;
            let ws2 = ws.clone();
            with_store(&store, move |s| s.merge_conversation(&ws2, &fresh)).await?;
            events.emit(Event::ConversationInfo {
                channel,
                num_members,
                num_apps,
                members,
            });
            if !external_teams.is_empty() {
                resolve_teams(backend, store, ws, events, teams_asked, external_teams).await;
            }
            Ok(())
        });
    }

    /// Open-time `dnd.info` for a DM peer: whether their notifications are paused. Transient
    /// by design, so it is re-fetched every time the DM opens and never stored.
    fn fetch_peer_dnd(&self, channel: ChannelId) {
        let (backend, store, ws, events) = (
            self.backend.clone(),
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
        );
        self.spawn("dnd.info", async move {
            let Some(peer) = im_peer(&store, &ws, &channel).await? else {
                return Ok(());
            };
            let status = backend.dnd_info(peer.clone()).await?;
            events.emit(Event::DndPaused {
                user: peer,
                paused: status.paused(now_secs()),
            });
            Ok(())
        });
    }

    /// `users.info` for people `users.list` leaves out (members of other organizations in
    /// shared channels and DMs), each id asked once per session. Found profiles are stored
    /// and announced together; a failed lookup is reported and the rest still land. The
    /// organizations the people found belong to are resolved next ([`resolve_teams`]), so a
    /// Slack Connect channel can name them on its first visit.
    fn resolve_users(&mut self, ids: Vec<UserId>) {
        let ids: Vec<UserId> = ids
            .into_iter()
            .filter(|id| self.users_asked.insert(id.clone()))
            .collect();
        if ids.is_empty() {
            return;
        }
        let (backend, store, ws, events, teams_asked) = (
            self.backend.clone(),
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
            Arc::clone(&self.teams_asked),
        );
        self.spawn("users.info", async move {
            // A full record already in the store (users.list or an earlier ask) needs no
            // call: only the store-unknown ids - another organization's people, guests -
            // are asked for.
            let ws_known = ws.clone();
            let stored = with_store(&store, move |s| {
                s.users(&ws_known).map(|users| {
                    users
                        .into_iter()
                        .map(|user| user.id)
                        .collect::<HashSet<UserId>>()
                })
            })
            .await
            .unwrap_or_default();
            let ids: Vec<UserId> = ids.into_iter().filter(|id| !stored.contains(id)).collect();
            if ids.is_empty() {
                return Ok(());
            }
            let mut users = Vec::new();
            for id in ids {
                match backend.user_info(id).await {
                    Ok(user) => users.push(user),
                    Err(e) => events.report("users.info", e.into()),
                }
            }
            if users.is_empty() {
                return Ok(());
            }
            let ids = users.iter().map(|user| user.id.clone()).collect();
            let teams: Vec<TeamId> = users
                .iter()
                .filter_map(|user| user.team_id.clone())
                .filter(|team| *team != ws)
                .collect();
            let ws2 = ws.clone();
            with_store(&store, move |s| {
                users.iter().try_for_each(|user| s.upsert_user(&ws2, user))
            })
            .await?;
            events.emit(Event::UsersChanged(ids));
            if !teams.is_empty() {
                resolve_teams(backend, store, ws, events, teams_asked, teams).await;
            }
            Ok(())
        });
    }

    /// Subscribes presence for every DM peer, so each DM row carries Slack's dot. Slack answers
    /// a `presence_sub` with the current presence of each newly subscribed id, so no per-user
    /// request is needed; each frame replaces the whole subscription, so the accumulated peers
    /// (including ones a DM open added) are resent.
    fn subscribe_dm_presence(&self) {
        /// Slack's recommended ceiling for one presence subscription.
        const PRESENCE_SUB_MAX: usize = 500;
        let Some(rtm) = self.rtm.clone() else {
            return;
        };
        let (store, ws, events) = (
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
        );
        let watched = Arc::clone(&self.presence_peers);
        self.spawn("presence_sub", async move {
            let conversations = with_store(&store, move |s| s.conversations(&ws)).await?;
            let peers = {
                let mut watched = watched.lock().unwrap_or_else(PoisonError::into_inner);
                for peer in conversations.into_iter().filter_map(|c| c.user) {
                    if !watched.contains(&peer) {
                        watched.push(peer);
                    }
                }
                watched.truncate(PRESENCE_SUB_MAX);
                watched.clone()
            };
            if let Err(e) = rtm.presence_sub(peers) {
                events.emit(Event::Error(format!("presence_sub: {e}")));
            }
            Ok(())
        });
    }

    /// Presence for a DM peer: one `users.getPresence` read the first time their DM opens,
    /// then the websocket subscription keeps it current through `presence_change` events
    /// (each `presence_sub` frame replaces the previous subscription, so the whole
    /// accumulated peer set is resent).
    fn watch_peer_presence(&self, channel: ChannelId) {
        let (backend, store, ws, events) = (
            self.backend.clone(),
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
        );
        let rtm = self.rtm.clone();
        let watched = Arc::clone(&self.presence_peers);
        self.spawn("users.getPresence", async move {
            let Some(peer) = im_peer(&store, &ws, &channel).await? else {
                return Ok(());
            };
            let newly_watched = {
                let mut watched = watched.lock().unwrap_or_else(PoisonError::into_inner);
                if watched.contains(&peer) {
                    false
                } else {
                    watched.push(peer.clone());
                    true
                }
            };
            if !newly_watched {
                return Ok(());
            }
            if let Some(rtm) = &rtm {
                let peers = watched
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                if let Err(e) = rtm.presence_sub(peers) {
                    events.emit(Event::Error(format!("presence_sub: {e}")));
                }
            }
            let presence = backend.user_presence(peer.clone()).await?;
            events.emit(Event::Presence {
                users: vec![peer],
                presence: presence.presence,
            });
            Ok(())
        });
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
        let _ = self
            .active_huddle_call_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let active = self
            .active_huddle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
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

/// Fills each Activity row's body through `messages.list`, one batched call per
/// [`api`] channel cap. Rows whose channel stays silent (or the whole call, when it fails)
/// keep no body: the view renders them without text and the failure is reported.
async fn hydrate_activity<B: Backend>(
    backend: &B,
    events: &Emitter,
    feed: &mut crate::api::ActivityFeed,
) {
    let mut by_channel: Vec<(ChannelId, Vec<Ts>)> = Vec::new();
    for item in &feed.items {
        if item.message.is_none() {
            match by_channel
                .iter_mut()
                .find(|(channel, _)| *channel == item.channel)
            {
                Some((_, stamps)) => stamps.push(item.ts.clone()),
                None => by_channel.push((item.channel.clone(), vec![item.ts.clone()])),
            }
        }
    }
    if by_channel.is_empty() {
        return;
    }
    match backend.messages_list(by_channel).await {
        Ok(bodies) => {
            for item in &mut feed.items {
                if item.message.is_none() {
                    item.message = bodies
                        .get(&item.channel)
                        .and_then(|messages| messages.iter().find(|message| message.ts == item.ts))
                        .cloned();
                }
            }
        }
        Err(e) => events.report("messages.list", e.into()),
    }
}

/// Fetch saved items across Slack's Later tabs (`saved`, `completed`, `archived`), deduplicate
/// by `(channel, ts)`, and hydrate their message bodies through `messages.list`.
async fn load_saved_items<B: Backend>(
    backend: &B,
    events: &Emitter,
) -> Result<Vec<crate::api::SavedItem>, ApiError> {
    let mut items = backend.saved_list("saved").await?;
    for filter in ["completed", "archived"] {
        if let Ok(extra) = backend.saved_list(filter).await {
            for item in extra {
                if !items
                    .iter()
                    .any(|existing| existing.channel == item.channel && existing.ts == item.ts)
                {
                    items.push(item);
                }
            }
        }
    }
    let mut by_channel: Vec<(ChannelId, Vec<Ts>)> = Vec::new();
    for item in &items {
        if item.message.is_none() {
            match by_channel
                .iter_mut()
                .find(|(channel, _)| *channel == item.channel)
            {
                Some((_, stamps)) => stamps.push(item.ts.clone()),
                None => by_channel.push((item.channel.clone(), vec![item.ts.clone()])),
            }
        }
    }
    if !by_channel.is_empty() {
        match backend.messages_list(by_channel).await {
            Ok(bodies) => {
                for item in &mut items {
                    if item.message.is_none() {
                        item.message = bodies
                            .get(&item.channel)
                            .and_then(|messages| messages.iter().find(|m| m.ts == item.ts))
                            .cloned();
                    }
                }
            }
            Err(e) => events.report("messages.list", e.into()),
        }
    }
    Ok(items)
}

/// Answers a lazy tab load with its data, or with the error text the tab shows instead of
/// loading forever; the failure is also returned for the usual report.
fn answer_load<T>(
    events: &Emitter,
    result: Result<T, ApiError>,
    answer: impl FnOnce(Result<T, String>) -> Event,
) -> Result<(), CoreError> {
    match result {
        Ok(data) => {
            events.emit(answer(Ok(data)));
            Ok(())
        }
        Err(e) => {
            events.emit(answer(Err(e.to_string())));
            Err(e.into())
        }
    }
}

/// A message someone just posted, with its channel: plain, bot and `/me` messages, files and
/// thread broadcasts, not edits, deletions or channel events (joins, topics, ...).
fn posted_message(event: &SlackEvent) -> Option<(ChannelId, Box<Message>)> {
    let SlackEvent::Message(MessageEvent::Message(message)) = event else {
        return None;
    };
    let posted = match message.subtype {
        None => true,
        Some(ref subtype) => matches!(
            subtype,
            MessageSubtype::BotMessage
                | MessageSubtype::MeMessage
                | MessageSubtype::FileShare
                | MessageSubtype::ThreadBroadcast
        ),
    };
    posted.then(|| Some((message.channel.clone()?, message.clone())))?
}

/// Conversation events without a conversation object; the store needs a fresh copy.
fn refresh_target(event: &SlackEvent) -> Option<ChannelId> {
    match event {
        SlackEvent::ChannelArchive(e)
        | SlackEvent::ChannelUnarchive(e)
        | SlackEvent::ChannelLeft(e) => Some(e.channel.clone()),
        _ => None,
    }
}

/// The peer of an IM channel as stored; `None` unless the channel is an IM with a known peer.
async fn im_peer(
    store: &SharedStore,
    workspace: &TeamId,
    channel: &ChannelId,
) -> Result<Option<UserId>, StoreError> {
    let (ws, channel) = (workspace.clone(), channel.clone());
    let stored = with_store(store, move |s| s.conversation(&ws, &channel)).await?;
    Ok(stored
        .filter(|c| c.kind() == ConversationKind::Im)
        .and_then(|c| c.user))
}

/// `team.info` for the connected organizations of shared channels (`team_ids`), each id
/// asked once per session through `asked` (found or not). Records are stored beside the
/// workspace's own team and announced with [`Event::SidebarChanged`], whose reload re-reads
/// them; a failed lookup is reported and the rest still land, so one bad org never blocks
/// the others.
async fn resolve_teams(
    backend: impl Backend,
    store: SharedStore,
    workspace: TeamId,
    events: Emitter,
    asked: Arc<Mutex<HashSet<TeamId>>>,
    team_ids: Vec<TeamId>,
) {
    let fresh: Vec<TeamId> = {
        let mut asked = asked.lock().unwrap_or_else(PoisonError::into_inner);
        team_ids
            .into_iter()
            .filter(|id| asked.insert(id.clone()))
            .collect()
    };
    if fresh.is_empty() {
        return;
    }
    let mut teams = Vec::new();
    for id in fresh {
        match backend.team_info(id).await {
            Ok(team) => teams.push(team),
            Err(e) => events.report("team.info", e.into()),
        }
    }
    if teams.is_empty() {
        return;
    }
    let outcome = with_store(&store, move |s| {
        teams
            .iter()
            .try_for_each(|team| s.upsert_team(&workspace, team))
    })
    .await;
    match outcome {
        Ok(()) => {
            events.emit(Event::SidebarChanged);
        }
        Err(e) => events.report("team.info store", e.into()),
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
