//! The runtime thread, the command loop and the front-end handle.

use std::collections::{HashMap, HashSet};
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
use crate::{Command, Event, apply_event};

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
    core.shutdown();
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
}

impl<B: Backend> Core<B> {
    pub fn new(backend: B, store: SharedStore, workspace: TeamId, events: Emitter) -> Self {
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
            bots: HashMap::new(),
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
        self.events.emit(Event::SidebarChanged);
        self.fetch_dm_previews();
        // The Activity badges need the feed's unread state; last, so the boot's own events
        // keep their order. The boot fetch skips the `messages.list` hydration (the view
        // asks for it with bodies when it opens).
        self.fetch_activity(false);
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
                Ok(backend.mark_read(channel, ts).await?)
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
            Command::OpenDm { user } => self.spawn("conversations.open", async move {
                let dm = backend.open_dm(user).await?;
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
            // Handled by the loop.
            Command::Shutdown => {}
        }
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
                if matches!(state, ConnectionState::Connected { .. }) {
                    self.subscribe_dm_presence();
                }
                self.events.emit(Event::Connection(state));
            }
            RtmEvent::Event(mut event) => {
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
            .filter(|_| message.is_ephemeral && message.bot_profile.is_none())
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

    fn shutdown(&mut self) {
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
pub(crate) mod tests {
    use std::ops::RangeInclusive;
    use std::time::Duration;

    use super::*;
    use crate::api::{
        self, ChannelBookmark, ClientCounts, FilesFilter, MessageContent, Page, PinnedItem,
        PostedMessage, SearchMessages, UserBoot,
    };
    use crate::rtm::ConnectionState;
    use crate::types::{
        ChannelCanvas, ChannelSection, Conversation, ConversationProperties, DndStatus, File,
        FileId, Message, Presence, SectionKind, Team, Ts, User, UserId, UserPresence,
    };

    /// History page size of the fake.
    const PAGE: usize = 2;

    /// In-process stand-in for Slack: a fixed channel history paged by index cursors.
    #[derive(Clone, Default)]
    pub(crate) struct FakeBackend {
        state: Arc<Mutex<FakeState>>,
    }

    #[derive(Default)]
    struct FakeState {
        history: Vec<Message>,
        calls: Vec<(Option<Ts>, Option<String>)>,
        /// One thread (`conversations.replies`): the root followed by its replies.
        thread: Vec<Message>,
        /// The `subscriptions.thread.getView` answer.
        threads_view: api::ThreadsView,
        /// Sidebar sections served by `channel_sections`; `create_section` appends here.
        sections: Vec<ChannelSection>,
        reply_calls: Vec<(Ts, Option<String>)>,
        /// `subscriptions.thread.add` / `.remove` asks: thread, read cursor, follow.
        subscription_calls: Vec<(Ts, Ts, bool)>,
        /// `admin.roles.entity.listAssignments` answer: the channel's managers.
        managers: Vec<UserId>,
        rate_limited: u32,
        fail_replies: bool,
        /// `conversations.info` answer; the default echoes the id with `is_archived` set.
        info: Option<Conversation>,
        /// `conversations.info` asks, in order.
        info_calls: Vec<ChannelId>,
        /// Every `conversations.info` call fails with `channel_not_found`.
        fail_info: bool,
        /// `conversations.members` answer for every channel.
        members: Vec<UserId>,
        /// Every `conversations.members` call fails with `channel_not_found`.
        fail_members: bool,
        /// `conversations.members` asks, in order.
        member_calls: Vec<ChannelId>,
        /// `conversations.history` limit-1 asks (DM previews), in order.
        latest_calls: Vec<ChannelId>,
        /// Single-page `conversations.history` asks (`FetchLatestHistory`): (channel, limit).
        recent_calls: Vec<(ChannelId, u32)>,
        /// `conversations.mark` asks, in order.
        mark_calls: Vec<(ChannelId, Ts)>,
        /// `client.dms` answer.
        dms: Vec<Conversation>,
        /// How often `client.dms` was asked.
        dm_calls: usize,
        /// `users.conversations` answer (ids).
        memberships: Vec<ChannelId>,
        /// How often `users.conversations` was asked.
        membership_calls: usize,
        /// `dnd.info` answers keyed by user; an unconfigured user is never paused.
        dnd: HashMap<UserId, DndStatus>,
        presence: HashMap<UserId, UserPresence>,
        dnd_calls: Vec<UserId>,
        presence_calls: Vec<UserId>,
        user_info_calls: Vec<UserId>,
        /// `users.info` answers' organizations by user; others answer without one.
        user_teams: HashMap<UserId, TeamId>,
        leave_calls: Vec<ChannelId>,
        /// `conversations.setTopic` asks: (channel, topic).
        topic_calls: Vec<(ChannelId, String)>,
        /// `conversations.setPurpose` asks: (channel, purpose).
        purpose_calls: Vec<(ChannelId, String)>,
        /// `team.info` answers keyed by team id; an unconfigured team fails.
        teams: HashMap<TeamId, Team>,
        /// `team.info` asks, in order.
        team_info_calls: Vec<TeamId>,
        notification_calls: Vec<(ChannelId, &'static str, &'static str)>,
        /// `pins.list` answer.
        pins: Vec<PinnedItem>,
        /// `bookmarks.list` answer; `bookmarks.add` appends to it.
        bookmarks: Vec<ChannelBookmark>,
        /// `files.list` answer.
        files: Vec<File>,
        bookmark_adds: Vec<(ChannelId, String, String)>,
        /// `pins.add` / `pins.remove` asks: (channel, ts, pinned?).
        pin_calls: Vec<(ChannelId, Ts, bool)>,
        canvas_creations: Vec<ChannelId>,
        /// Every tab load (`pins.list`, `bookmarks.list`, `files.list`) fails.
        fail_tab_loads: bool,
        /// The `activity.feed` answer.
        activity: api::ActivityFeed,
        /// `messages.list` bodies by channel.
        activity_bodies: HashMap<ChannelId, Vec<Message>>,
        /// `activity.markRead`/`markUnread` asks: (read?, entries).
        activity_marks: Vec<(bool, Vec<api::ActivityEntry>)>,
        /// `users.channelSections.set` asks: (section id, name, emoji).
        section_renames: Vec<(String, String, String)>,
        /// `users.channelSections.delete` asks, in order.
        section_deletes: Vec<String>,
        /// `users.channelSections.channels.bulkUpdate` asks: (insert, remove).
        section_moves: Vec<SectionMove>,
    }

    /// One `users.channelSections.channels.bulkUpdate` ask: its insert and remove sides.
    type SectionMove = (Option<(String, ChannelId)>, Option<(String, ChannelId)>);

    /// The fake's canned sections: stored order is deliberately not display order (L2
    /// points at L1), and "Priority" already holds C1.
    fn base_sections() -> Vec<ChannelSection> {
        vec![
            ChannelSection {
                id: "L1".to_owned(),
                name: "Channels".to_owned(),
                kind: SectionKind::Channels,
                emoji: String::new(),
                next_id: None,
                channel_ids: Vec::new(),
            },
            ChannelSection {
                id: "L2".to_owned(),
                name: "Priority".to_owned(),
                kind: SectionKind::Standard,
                emoji: "rocket".to_owned(),
                next_id: Some("L1".to_owned()),
                channel_ids: vec![ChannelId::from("C1")],
            },
        ]
    }

    /// The mutable section `id` in `state`, seeding the canned sections first so writes to
    /// them stick (an empty override means "not yet written", so it falls back to base).
    fn effective_section<'a>(state: &'a mut FakeState, id: &str) -> &'a mut ChannelSection {
        if state.sections.is_empty() {
            state.sections = base_sections();
        }
        state
            .sections
            .iter_mut()
            .find(|section| section.id == id)
            .expect("the canned sections hold the asked id")
    }

    impl FakeBackend {
        pub fn with_history(range: RangeInclusive<u32>) -> Self {
            let fake = Self::default();
            fake.push_history(range);
            fake
        }

        /// The `subscriptions.thread.getView` answer.
        pub fn set_threads_view(&self, view: api::ThreadsView) {
            self.state.lock().unwrap().threads_view = view;
        }

        /// The `activity.feed` answer, and the `messages.list` bodies by channel.
        pub fn set_activity(
            &self,
            feed: api::ActivityFeed,
            bodies: HashMap<ChannelId, Vec<Message>>,
        ) {
            let mut state = self.state.lock().unwrap();
            state.activity = feed;
            state.activity_bodies = bodies;
        }

        /// `activity.markRead`/`markUnread` asks so far: (read?, entries).
        pub fn activity_marks(&self) -> Vec<(bool, Vec<api::ActivityEntry>)> {
            self.state.lock().unwrap().activity_marks.clone()
        }

        pub fn push_history(&self, range: RangeInclusive<u32>) {
            self.state
                .lock()
                .unwrap()
                .history
                .extend(range.map(|i| Message {
                    ts: Ts::from(format!("{i}.0")),
                    text: format!("m{i}"),
                    ..Default::default()
                }));
        }

        /// A thread rooted at `thread_ts` with one reply per item of `range` (reply `i` gets
        /// ts `{thread_ts}.{i}`), served whole or in pages by `replies_page`.
        pub fn push_thread(&self, thread_ts: &str, range: RangeInclusive<u32>) {
            let mut state = self.state.lock().unwrap();
            state.thread.push(Message {
                ts: Ts::from(thread_ts),
                text: format!("root {thread_ts}"),
                ..Default::default()
            });
            state.thread.extend(range.map(|i| Message {
                ts: Ts::from(format!("{thread_ts}.{i}")),
                thread_ts: Some(Ts::from(thread_ts)),
                text: format!("reply {i}"),
                ..Default::default()
            }));
        }

        /// Make every `replies_page` call fail with `invalid_auth`.
        pub fn fail_replies(&self) {
            self.state.lock().unwrap().fail_replies = true;
        }

        /// The `conversations.info` answer for every channel.
        pub fn set_info(&self, conversation: Conversation) {
            self.state.lock().unwrap().info = Some(conversation);
        }

        /// Make every `conversations.info` call fail with `channel_not_found`.
        pub fn fail_info(&self) {
            self.state.lock().unwrap().fail_info = true;
        }

        /// `conversations.info` asks, in order.
        pub fn info_calls(&self) -> Vec<ChannelId> {
            self.state.lock().unwrap().info_calls.clone()
        }

        /// The `conversations.members` answer for every channel.
        pub fn set_members(&self, members: Vec<UserId>) {
            self.state.lock().unwrap().members = members;
        }

        /// Make every `conversations.members` call fail with `channel_not_found`.
        pub fn fail_members(&self) {
            self.state.lock().unwrap().fail_members = true;
        }

        /// `conversations.members` asks, in order.
        pub fn member_calls(&self) -> Vec<ChannelId> {
            self.state.lock().unwrap().member_calls.clone()
        }

        /// `conversations.history` limit-1 asks (DM previews), in order.
        pub fn latest_calls(&self) -> Vec<ChannelId> {
            self.state.lock().unwrap().latest_calls.clone()
        }

        /// The `client.dms` answer.
        pub fn set_dms(&self, dms: Vec<Conversation>) {
            self.state.lock().unwrap().dms = dms;
        }

        /// How often `client.dms` was asked.
        pub fn dm_calls(&self) -> usize {
            self.state.lock().unwrap().dm_calls
        }

        /// The `users.conversations` answer: the ids of the user's channel memberships.
        pub fn set_memberships(&self, members: Vec<ChannelId>) {
            self.state.lock().unwrap().memberships = members;
        }

        /// How often `users.conversations` was asked.
        pub fn membership_calls(&self) -> usize {
            self.state.lock().unwrap().membership_calls
        }

        /// Pushes one message into the shared history (unfurl attachments included).
        pub fn push_message(&self, message: Message) {
            self.state.lock().unwrap().history.push(message);
        }

        /// The `pins.list` answer.
        pub fn set_pins(&self, pins: Vec<PinnedItem>) {
            self.state.lock().unwrap().pins = pins;
        }

        /// The `bookmarks.list` answer.
        pub fn set_bookmarks(&self, bookmarks: Vec<ChannelBookmark>) {
            self.state.lock().unwrap().bookmarks = bookmarks;
        }

        /// The `files.list` answer.
        pub fn set_files(&self, files: Vec<File>) {
            self.state.lock().unwrap().files = files;
        }

        /// Make every tab load (`pins.list`, `bookmarks.list`, `files.list`) fail.
        pub fn fail_tab_loads(&self) {
            self.state.lock().unwrap().fail_tab_loads = true;
        }

        /// One tab load's answer: `pick`'s list, or a Slack error once `fail_tab_loads`.
        fn tab_load<T: Clone>(
            &self,
            pick: impl FnOnce(&FakeState) -> &Vec<T>,
        ) -> api::Result<Vec<T>> {
            let state = self.state.lock().unwrap();
            if state.fail_tab_loads {
                return Err(ApiError::Slack("channel_not_found".into()));
            }
            Ok(pick(&state).clone())
        }

        /// The recorded `bookmarks.add` calls.
        pub fn bookmark_adds(&self) -> Vec<(ChannelId, String, String)> {
            self.state.lock().unwrap().bookmark_adds.clone()
        }

        /// The recorded `pins.add` / `pins.remove` calls.
        pub fn pin_calls(&self) -> Vec<(ChannelId, Ts, bool)> {
            self.state.lock().unwrap().pin_calls.clone()
        }

        /// The recorded `conversations.canvases.create` calls.
        pub fn canvas_creations(&self) -> Vec<ChannelId> {
            self.state.lock().unwrap().canvas_creations.clone()
        }

        /// The `dnd.info` answer for `user`.
        pub fn set_dnd(&self, user: UserId, status: DndStatus) {
            self.state.lock().unwrap().dnd.insert(user, status);
        }

        /// The `users.getPresence` answer for `user`.
        pub fn set_presence(&self, user: UserId, presence: UserPresence) {
            self.state.lock().unwrap().presence.insert(user, presence);
        }

        /// Users asked for `dnd.info`, in order.
        pub fn dnd_calls(&self) -> Vec<UserId> {
            self.state.lock().unwrap().dnd_calls.clone()
        }

        /// Users asked for `users.getPresence`, in order.
        pub fn presence_calls(&self) -> Vec<UserId> {
            self.state.lock().unwrap().presence_calls.clone()
        }

        /// Users asked for `users.info`, in order.
        pub fn user_info_calls(&self) -> Vec<UserId> {
            self.state.lock().unwrap().user_info_calls.clone()
        }

        /// Channels left through `conversations.leave`, in order.
        pub fn leave_calls(&self) -> Vec<ChannelId> {
            self.state.lock().unwrap().leave_calls.clone()
        }

        /// `users.channelSections.set` asks so far: (section id, name, emoji).
        pub fn section_renames(&self) -> Vec<(String, String, String)> {
            self.state.lock().unwrap().section_renames.clone()
        }

        /// `users.channelSections.delete` asks so far.
        pub fn section_deletes(&self) -> Vec<String> {
            self.state.lock().unwrap().section_deletes.clone()
        }

        /// `users.channelSections.channels.bulkUpdate` asks so far: (insert, remove).
        pub fn section_moves(&self) -> Vec<SectionMove> {
            self.state.lock().unwrap().section_moves.clone()
        }

        /// The recorded `conversations.setTopic` calls.
        pub fn topic_calls(&self) -> Vec<(ChannelId, String)> {
            self.state.lock().unwrap().topic_calls.clone()
        }

        /// The recorded `conversations.setPurpose` calls.
        pub fn purpose_calls(&self) -> Vec<(ChannelId, String)> {
            self.state.lock().unwrap().purpose_calls.clone()
        }

        /// The `team.info` answer for `team` (an unconfigured team still fails).
        pub fn set_team(&self, team: Team) {
            self.state
                .lock()
                .unwrap()
                .teams
                .insert(team.id.clone(), team);
        }

        /// Teams asked for `team.info`, in order.
        pub fn team_info_calls(&self) -> Vec<TeamId> {
            self.state.lock().unwrap().team_info_calls.clone()
        }

        /// The organization `users.info` answers for `user`.
        pub fn set_user_team(&self, user: UserId, team: TeamId) {
            self.state.lock().unwrap().user_teams.insert(user, team);
        }

        /// `users.prefs.setNotifications` writes, in order.
        pub fn notification_calls(&self) -> Vec<(ChannelId, &'static str, &'static str)> {
            self.state.lock().unwrap().notification_calls.clone()
        }

        pub fn messages(&self) -> Vec<Message> {
            self.state.lock().unwrap().history.clone()
        }

        /// `(latest, cursor)` of every history request.
        pub fn calls(&self) -> Vec<(Option<Ts>, Option<String>)> {
            self.state.lock().unwrap().calls.clone()
        }

        /// `(thread_ts, cursor)` of every `conversations.replies` request.
        pub fn reply_calls(&self) -> Vec<(Ts, Option<String>)> {
            self.state.lock().unwrap().reply_calls.clone()
        }

        pub fn clear_calls(&self) {
            self.state.lock().unwrap().calls.clear();
        }

        pub fn rate_limit_next(&self, n: u32) {
            self.state.lock().unwrap().rate_limited = n;
        }
    }

    impl Backend for FakeBackend {
        async fn user_boot(&self) -> api::Result<UserBoot> {
            Ok(UserBoot {
                channels: vec![Conversation {
                    id: ChannelId::from("C1"),
                    ..Default::default()
                }],
                team: Some(Team {
                    id: TeamId::from("T1"),
                    name: "Acme".into(),
                    domain: "acme".into(),
                    ..Default::default()
                }),
                prefs: Some(serde_json::json!({"display_real_names": true})),
                ..Default::default()
            })
        }

        async fn counts(&self) -> api::Result<ClientCounts> {
            Ok(ClientCounts::default())
        }

        async fn users(&self) -> api::Result<Vec<User>> {
            Ok(vec![User {
                id: UserId::from("U1"),
                ..Default::default()
            }])
        }

        async fn channel_sections(&self) -> api::Result<Vec<ChannelSection>> {
            let state = self.state.lock().unwrap();
            Ok(if state.sections.is_empty() {
                base_sections()
            } else {
                state.sections.clone()
            })
        }

        async fn custom_emoji(&self) -> api::Result<HashMap<String, String>> {
            Ok(HashMap::from([(
                "party".to_owned(),
                "https://emoji.example/party.png".to_owned(),
            )]))
        }

        async fn dm_list(&self) -> api::Result<Vec<Conversation>> {
            let mut state = self.state.lock().unwrap();
            state.dm_calls += 1;
            Ok(state.dms.clone())
        }

        async fn channel_memberships(&self) -> api::Result<Vec<Conversation>> {
            let mut state = self.state.lock().unwrap();
            state.membership_calls += 1;
            Ok(state
                .memberships
                .iter()
                .map(|id| Conversation {
                    id: id.clone(),
                    ..Default::default()
                })
                .collect())
        }

        async fn history_page(
            &self,
            _channel: ChannelId,
            latest: Option<Ts>,
            cursor: Option<String>,
        ) -> api::Result<Page<Message>> {
            let mut state = self.state.lock().unwrap();
            state.calls.push((latest.clone(), cursor.clone()));
            if state.rate_limited > 0 {
                state.rate_limited -= 1;
                return Err(ApiError::RateLimited {
                    retry_after: Duration::from_secs(5),
                });
            }
            let mut messages: Vec<Message> = state
                .history
                .iter()
                .filter(|m| latest.as_ref().is_none_or(|l| m.ts < *l))
                .cloned()
                .collect();
            messages.sort_by(|a, b| b.ts.cmp(&a.ts));
            let start: usize = cursor.map_or(0, |c| c.parse().unwrap());
            let end = (start + PAGE).min(messages.len());
            Ok(Page {
                items: messages[start..end].to_vec(),
                next_cursor: (end < messages.len()).then(|| end.to_string()),
            })
        }

        async fn replies_page(
            &self,
            _channel: ChannelId,
            thread_ts: Ts,
            cursor: Option<String>,
        ) -> api::Result<Page<Message>> {
            let mut state = self.state.lock().unwrap();
            state.reply_calls.push((thread_ts, cursor.clone()));
            if state.fail_replies {
                return Err(ApiError::AuthExpired("invalid_auth".into()));
            }
            let start: usize = cursor.map_or(0, |c| c.parse().unwrap());
            let end = (start + PAGE).min(state.thread.len());
            Ok(Page {
                items: state.thread[start..end].to_vec(),
                next_cursor: (end < state.thread.len()).then(|| end.to_string()),
            })
        }

        async fn conversation_info(&self, channel: ChannelId) -> api::Result<Conversation> {
            let mut state = self.state.lock().unwrap();
            state.info_calls.push(channel.clone());
            if state.fail_info {
                return Err(ApiError::Slack("channel_not_found".into()));
            }
            let info = state.info.clone();
            Ok(match info {
                Some(mut conversation) => {
                    conversation.id = channel;
                    conversation
                }
                None => Conversation {
                    id: channel,
                    is_archived: true,
                    ..Default::default()
                },
            })
        }

        async fn conversation_members(&self, channel: ChannelId) -> api::Result<Vec<UserId>> {
            let mut state = self.state.lock().unwrap();
            state.member_calls.push(channel);
            if state.fail_members {
                return Err(ApiError::Slack("channel_not_found".into()));
            }
            Ok(state.members.clone())
        }

        async fn latest_message(&self, channel: ChannelId) -> api::Result<Option<Message>> {
            let mut state = self.state.lock().unwrap();
            state.latest_calls.push(channel);
            Ok(state.history.iter().max_by(|a, b| a.ts.cmp(&b.ts)).cloned())
        }

        async fn recent_history(
            &self,
            channel: ChannelId,
            limit: u32,
        ) -> api::Result<Vec<Message>> {
            let mut state = self.state.lock().unwrap();
            state.recent_calls.push((channel, limit));
            let mut items = state.history.clone();
            items.sort_by(|a, b| b.ts.cmp(&a.ts));
            items.truncate(limit as usize);
            Ok(items)
        }

        async fn leave_conversation(&self, channel: ChannelId) -> api::Result<()> {
            self.state.lock().unwrap().leave_calls.push(channel);
            Ok(())
        }

        async fn join_conversation(&self, channel: ChannelId) -> api::Result<Conversation> {
            Ok(Conversation {
                id: channel,
                ..Default::default()
            })
        }

        async fn set_topic(&self, channel: ChannelId, topic: String) -> api::Result<Conversation> {
            self.state
                .lock()
                .unwrap()
                .topic_calls
                .push((channel.clone(), topic.clone()));
            Ok(Conversation {
                id: channel,
                topic: Some(crate::types::TopicPurpose {
                    value: topic,
                    ..Default::default()
                }),
                ..Default::default()
            })
        }

        async fn set_purpose(
            &self,
            channel: ChannelId,
            purpose: String,
        ) -> api::Result<Conversation> {
            self.state
                .lock()
                .unwrap()
                .purpose_calls
                .push((channel.clone(), purpose.clone()));
            Ok(Conversation {
                id: channel,
                purpose: Some(crate::types::TopicPurpose {
                    value: purpose,
                    ..Default::default()
                }),
                ..Default::default()
            })
        }

        async fn team_info(&self, team: TeamId) -> api::Result<Team> {
            let mut state = self.state.lock().unwrap();
            state.team_info_calls.push(team.clone());
            state
                .teams
                .get(&team)
                .cloned()
                .ok_or_else(|| ApiError::Slack("team_not_found".into()))
        }

        async fn set_notification_pref(
            &self,
            channel: ChannelId,
            name: &'static str,
            value: &'static str,
        ) -> api::Result<()> {
            let mut state = self.state.lock().unwrap();
            state.notification_calls.push((channel, name, value));
            Ok(())
        }

        /// Every id resolves, to a profile named `ext-<id>`.
        async fn user_info(&self, user: UserId) -> api::Result<User> {
            let mut state = self.state.lock().unwrap();
            state.user_info_calls.push(user.clone());
            Ok(User {
                name: format!("ext-{}", user.as_str()),
                team_id: state.user_teams.get(&user).cloned(),
                id: user,
                ..Default::default()
            })
        }

        async fn dnd_info(&self, user: UserId) -> api::Result<DndStatus> {
            let mut state = self.state.lock().unwrap();
            state.dnd_calls.push(user.clone());
            Ok(state.dnd.get(&user).cloned().unwrap_or_default())
        }

        async fn user_presence(&self, user: UserId) -> api::Result<UserPresence> {
            let mut state = self.state.lock().unwrap();
            state.presence_calls.push(user.clone());
            Ok(state.presence.get(&user).cloned().unwrap_or(UserPresence {
                presence: Presence::Away,
                online: None,
                auto_away: None,
                manual_away: None,
                connection_count: None,
                last_activity: None,
            }))
        }

        async fn post_message(
            &self,
            channel: ChannelId,
            content: MessageContent,
            _thread_ts: Option<Ts>,
            _broadcast: bool,
        ) -> api::Result<PostedMessage> {
            let ts = Ts::from("100.0");
            let message = Message {
                ts: ts.clone(),
                text: content.text,
                ..Default::default()
            };
            Ok(PostedMessage {
                channel,
                ts,
                message: Some(message),
                client_msg_id: None,
            })
        }

        async fn update_message(
            &self,
            _: ChannelId,
            _: Ts,
            _: MessageContent,
        ) -> api::Result<PostedMessage> {
            Ok(PostedMessage::default())
        }

        async fn delete_message(&self, _: ChannelId, _: Ts) -> api::Result<()> {
            Ok(())
        }

        async fn add_reaction(&self, _: ChannelId, _: Ts, _: String) -> api::Result<()> {
            Ok(())
        }

        async fn remove_reaction(&self, _: ChannelId, _: Ts, _: String) -> api::Result<()> {
            Ok(())
        }

        async fn run_command(
            &self,
            _: ChannelId,
            _: String,
            _: String,
            _: Option<Ts>,
            _: TeamId,
        ) -> api::Result<()> {
            Ok(())
        }

        async fn block_action(
            &self,
            _: crate::types::BotId,
            _: String,
            _: TeamId,
            _: serde_json::Value,
            _: api::ActionContainer,
        ) -> api::Result<()> {
            Ok(())
        }

        async fn bot_info(
            &self,
            bot: crate::types::BotId,
        ) -> api::Result<crate::types::BotProfile> {
            Ok(crate::types::BotProfile {
                id: bot,
                ..Default::default()
            })
        }

        async fn search(&self, _: String) -> api::Result<SearchMessages> {
            Ok(SearchMessages {
                total: 7,
                ..Default::default()
            })
        }

        async fn threads_view(&self) -> api::Result<api::ThreadsView> {
            Ok(self.state.lock().unwrap().threads_view.clone())
        }

        async fn activity_feed(&self) -> api::Result<api::ActivityFeed> {
            Ok(self.state.lock().unwrap().activity.clone())
        }

        async fn messages_list(
            &self,
            ids: Vec<(ChannelId, Vec<Ts>)>,
        ) -> api::Result<HashMap<ChannelId, Vec<Message>>> {
            let state = self.state.lock().unwrap();
            Ok(ids
                .into_iter()
                .map(|(channel, _)| {
                    let messages = state
                        .activity_bodies
                        .get(&channel)
                        .cloned()
                        .unwrap_or_default();
                    (channel, messages)
                })
                .collect())
        }

        async fn activity_mark_read(&self, entries: Vec<api::ActivityEntry>) -> api::Result<()> {
            self.state
                .lock()
                .unwrap()
                .activity_marks
                .push((true, entries));
            Ok(())
        }

        async fn activity_mark_unread(&self, entries: Vec<api::ActivityEntry>) -> api::Result<()> {
            self.state
                .lock()
                .unwrap()
                .activity_marks
                .push((false, entries));
            Ok(())
        }

        async fn open_dm(&self, user: UserId) -> api::Result<Conversation> {
            Ok(Conversation {
                id: ChannelId::from("D9"),
                is_im: true,
                user: Some(user),
                ..Default::default()
            })
        }

        async fn create_section(&self, name: String) -> api::Result<api::SectionCreated> {
            self.state.lock().unwrap().sections.push(ChannelSection {
                id: "LNEW".to_owned(),
                name,
                kind: SectionKind::Standard,
                emoji: String::new(),
                next_id: None,
                channel_ids: Vec::new(),
            });
            Ok(api::SectionCreated {
                channel_section_id: "LNEW".to_owned(),
            })
        }

        async fn rename_section(
            &self,
            section: String,
            name: String,
            emoji: String,
        ) -> api::Result<()> {
            let mut state = self.state.lock().unwrap();
            state
                .section_renames
                .push((section.clone(), name.clone(), emoji));
            let target = effective_section(&mut state, &section);
            target.name = name;
            Ok(())
        }

        async fn delete_section(&self, section: String) -> api::Result<()> {
            let mut state = self.state.lock().unwrap();
            state.section_deletes.push(section.clone());
            if state.sections.is_empty() {
                state.sections = base_sections();
            }
            state.sections.retain(|known| known.id != section);
            Ok(())
        }

        async fn move_channel(
            &self,
            insert: Option<(String, ChannelId)>,
            remove: Option<(String, ChannelId)>,
        ) -> api::Result<()> {
            let mut state = self.state.lock().unwrap();
            state.section_moves.push((insert.clone(), remove.clone()));
            if state.sections.is_empty() {
                state.sections = base_sections();
            }
            if let Some((from, channel)) = remove {
                if let Some(source) = state.sections.iter_mut().find(|s| s.id == from) {
                    source.channel_ids.retain(|id| *id != channel);
                }
            }
            if let Some((to, channel)) = insert {
                if let Some(target) = state.sections.iter_mut().find(|s| s.id == to) {
                    if !target.channel_ids.contains(&channel) {
                        target.channel_ids.push(channel);
                    }
                }
            }
            Ok(())
        }

        async fn mark_read(&self, channel: ChannelId, ts: Ts) -> api::Result<()> {
            self.state.lock().unwrap().mark_calls.push((channel, ts));
            Err(ApiError::AuthExpired("invalid_auth".into()))
        }

        async fn mark_all_read(&self) -> api::Result<()> {
            Err(ApiError::AuthExpired("invalid_auth".into()))
        }

        async fn set_thread_subscribed(
            &self,
            _: ChannelId,
            thread_ts: Ts,
            last_read: Ts,
            subscribed: bool,
        ) -> api::Result<()> {
            let mut state = self.state.lock().unwrap();
            state
                .subscription_calls
                .push((thread_ts, last_read, subscribed));
            Ok(())
        }

        async fn channel_managers(&self, _: ChannelId) -> api::Result<Vec<UserId>> {
            Ok(self.state.lock().unwrap().managers.clone())
        }

        async fn pins(&self, _: ChannelId) -> api::Result<Vec<PinnedItem>> {
            self.tab_load(|state| &state.pins)
        }

        async fn bookmarks(&self, _: ChannelId) -> api::Result<Vec<ChannelBookmark>> {
            self.tab_load(|state| &state.bookmarks)
        }

        async fn files(&self, _: FilesFilter) -> api::Result<Vec<File>> {
            self.tab_load(|state| &state.files)
        }

        async fn add_bookmark(
            &self,
            channel: ChannelId,
            title: String,
            url: String,
        ) -> api::Result<()> {
            let mut state = self.state.lock().unwrap();
            state
                .bookmark_adds
                .push((channel, title.clone(), url.clone()));
            state.bookmarks.push(ChannelBookmark {
                id: "BkNew".to_owned(),
                title,
                link: Some(url),
                kind: Some("link".to_owned()),
                ..Default::default()
            });
            Ok(())
        }

        async fn set_pinned(&self, channel: ChannelId, ts: Ts, pinned: bool) -> api::Result<()> {
            self.state
                .lock()
                .unwrap()
                .pin_calls
                .push((channel, ts, pinned));
            Ok(())
        }

        async fn create_canvas(&self, channel: ChannelId) -> api::Result<String> {
            self.state.lock().unwrap().canvas_creations.push(channel);
            Ok("F07NEW".to_owned())
        }
    }

    pub(crate) fn fake_core_parts() -> (SharedStore, Emitter, UnboundedReceiver<Event>) {
        let (tx, rx) = mpsc::unbounded();
        (
            Arc::new(Mutex::new(Store::open_in_memory().unwrap())),
            Emitter(tx),
            rx,
        )
    }

    fn c1() -> ChannelId {
        ChannelId::from("C1")
    }

    async fn next(rx: &mut UnboundedReceiver<Event>) -> Event {
        tokio::time::timeout(Duration::from_secs(5), rx.next())
            .await
            .expect("event within 5s")
            .expect("event stream open")
    }

    #[tokio::test]
    async fn tab_loads_fetch_their_data_and_emit_it() {
        let fake = FakeBackend::default();
        fake.set_pins(vec![PinnedItem {
            kind: Some("message".to_owned()),
            message: Some(Message {
                ts: Ts::from("9.0"),
                text: "pinned".into(),
                ..Default::default()
            }),
            ..Default::default()
        }]);
        fake.set_bookmarks(vec![ChannelBookmark {
            id: "Bk1".to_owned(),
            title: "Runbook".to_owned(),
            link: Some("https://acme.test/runbook".to_owned()),
            kind: Some("link".to_owned()),
            ..Default::default()
        }]);
        fake.set_files(vec![File {
            id: FileId::from("F1"),
            name: Some("spec.pdf".to_owned()),
            ..Default::default()
        }]);
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake, Arc::clone(&store), TeamId::from("T1"), events);

        core.handle(Command::LoadPins(c1()));
        let Event::PinsLoaded { channel, pins } = next(&mut rx).await else {
            panic!("expected pins");
        };
        assert_eq!(channel, c1());
        let pins = pins.expect("pins load");
        assert_eq!(
            pins[0].message.as_ref().map(|m| m.text.as_str()),
            Some("pinned")
        );

        core.handle(Command::LoadBookmarks(c1()));
        let Event::BookmarksLoaded { channel, bookmarks } = next(&mut rx).await else {
            panic!("expected bookmarks");
        };
        let bookmarks = bookmarks.expect("bookmarks load");
        assert_eq!((channel, bookmarks.len()), (c1(), 1));
        assert_eq!(bookmarks[0].title, "Runbook");

        core.handle(Command::LoadFiles(FilesFilter::channel(c1())));
        let Event::FilesLoaded { filter, files } = next(&mut rx).await else {
            panic!("expected files");
        };
        let files = files.expect("files load");
        assert_eq!(filter, FilesFilter::channel(c1()));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id.as_str(), "F1");

        // A workspace-wide load (the Files view) keeps its filter on the answer too.
        core.handle(Command::LoadFiles(FilesFilter::workspace()));
        let Event::FilesLoaded { filter, files } = next(&mut rx).await else {
            panic!("expected workspace files");
        };
        assert_eq!(filter, FilesFilter::workspace());
        assert_eq!(files.expect("files load").len(), 1);
    }

    #[tokio::test]
    async fn failed_tab_load_answers_the_tab_with_the_error_and_reports_it() {
        let fake = FakeBackend::default();
        fake.fail_tab_loads();
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake, Arc::clone(&store), TeamId::from("T1"), events);

        core.handle(Command::LoadFiles(FilesFilter::channel(c1())));

        let Event::FilesLoaded { filter, files } = next(&mut rx).await else {
            panic!("a failed load still answers its tab");
        };
        assert_eq!(filter, FilesFilter::channel(c1()));
        assert!(
            files.is_err_and(|error| error.contains("channel_not_found")),
            "the tab gets Slack's error instead of loading forever"
        );
        assert!(
            matches!(next(&mut rx).await, Event::Error(error) if error.starts_with("files.list")),
            "the failure is reported as usual"
        );
    }

    #[tokio::test]
    async fn add_bookmark_adds_then_refetches_the_list() {
        let fake = FakeBackend::default();
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), TeamId::from("T1"), events);

        core.handle(Command::AddBookmark {
            channel: c1(),
            title: "Roadmap".into(),
            url: "https://acme.test/roadmap".into(),
        });

        let Event::BookmarksLoaded { channel, bookmarks } = next(&mut rx).await else {
            panic!("expected the refreshed list");
        };
        assert_eq!(channel, c1());
        let bookmarks = bookmarks.expect("the refreshed list");
        assert_eq!(
            bookmarks.last().map(|b| b.title.as_str()),
            Some("Roadmap"),
            "the refreshed list carries the added bookmark"
        );
        assert_eq!(
            fake.bookmark_adds(),
            vec![(
                c1(),
                "Roadmap".to_owned(),
                "https://acme.test/roadmap".to_owned()
            )]
        );
    }

    #[tokio::test]
    async fn set_pinned_calls_the_pin_method_then_refetches_the_list() {
        let fake = FakeBackend::default();
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), TeamId::from("T1"), events);
        let ts = Ts::from("1508197641.000151");

        core.handle(Command::SetPinned {
            channel: c1(),
            ts: ts.clone(),
            pinned: true,
        });

        let Event::PinsLoaded { channel, pins } = next(&mut rx).await else {
            panic!("expected the refreshed list");
        };
        assert_eq!(channel, c1());
        assert!(pins.is_ok(), "the refreshed list lands");
        assert_eq!(fake.pin_calls(), vec![(c1(), ts, true)]);
    }

    #[tokio::test]
    async fn create_channel_canvas_refreshes_the_conversation_and_announces_the_id() {
        let fake = FakeBackend::default();
        // The post-create `conversations.info` refresh carries the new canvas.
        fake.set_info(Conversation {
            properties: Some(ConversationProperties {
                canvas: Some(ChannelCanvas {
                    file_id: Some(FileId::from("F07NEW")),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        });
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), TeamId::from("T1"), events);
        let ws = TeamId::from("T1");
        store
            .lock()
            .unwrap()
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: c1(),
                    name: Some("general".into()),
                    ..Default::default()
                },
            )
            .unwrap();

        core.handle(Command::CreateChannelCanvas(c1()));

        assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
        assert_eq!(
            next(&mut rx).await,
            Event::ChannelCanvasCreated {
                channel: c1(),
                canvas_id: "F07NEW".to_owned(),
            }
        );
        assert_eq!(fake.canvas_creations(), vec![c1()]);
        let stored = store.lock().unwrap().conversation(&ws, &c1()).unwrap();
        let stored = stored.expect("the conversation stays stored");
        assert_eq!(
            stored
                .properties
                .and_then(|p| p.canvas)
                .and_then(|c| c.file_id),
            Some(FileId::from("F07NEW")),
            "the canvas id lands in the stored properties"
        );
        assert_eq!(
            stored.name.as_deref(),
            Some("general"),
            "merge keeps other fields"
        );
    }

    #[tokio::test]
    async fn hydrate_fills_conversations() {
        let (store, events, mut rx) = fake_core_parts();
        let core = Core::new(
            FakeBackend::default(),
            Arc::clone(&store),
            TeamId::from("T1"),
            events,
        );
        core.hydrate().await;
        assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
        assert_eq!(
            next(&mut rx).await,
            Event::UsersChanged(vec![UserId::from("U1")])
        );
        assert!(matches!(next(&mut rx).await, Event::Counts(_)));
        assert_eq!(next(&mut rx).await, Event::SidebarChanged);
        let store = store.lock().unwrap();
        let ws = TeamId::from("T1");
        assert_eq!(store.users(&ws).unwrap().len(), 1);
        assert_eq!(store.team(&ws).unwrap().unwrap().name, "Acme");
        let sections = store.sections(&ws).unwrap();
        let ids: Vec<_> = sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["L2", "L1"], "sections come back in display order");
        assert_eq!(
            store.prefs(&ws).unwrap(),
            Some(serde_json::json!({"display_real_names": true}))
        );
        assert_eq!(
            store.custom_emoji(&ws).unwrap(),
            vec![(
                "party".to_owned(),
                "https://emoji.example/party.png".to_owned()
            )]
        );
    }

    #[tokio::test]
    async fn commands_update_the_store_and_report_back() {
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(
            FakeBackend::default(),
            Arc::clone(&store),
            TeamId::from("T1"),
            events,
        );

        core.handle(Command::SendMessage {
            channel: c1(),
            content: MessageContent::plain("hi there"),
            thread_ts: None,
            broadcast: false,
        });
        assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
        let history = store
            .lock()
            .unwrap()
            .channel_history(&TeamId::from("T1"), &c1(), None, 10)
            .unwrap();
        assert_eq!(history[0].message.text, "hi there");

        core.handle(Command::Search("there".into()));
        let Event::SearchResults { query, messages } = next(&mut rx).await else {
            panic!("expected local results first");
        };
        assert_eq!((query.as_str(), messages.len()), ("there", 1));
        assert!(
            matches!(next(&mut rx).await, Event::RemoteSearchResults { results, .. } if results.total == 7)
        );

        core.handle(Command::MarkRead {
            channel: c1(),
            ts: Ts::from("1.0"),
        });
        assert_eq!(
            next(&mut rx).await,
            Event::AuthExpired("invalid_auth".into())
        );
    }

    #[tokio::test]
    async fn open_thread_fetches_the_full_thread_into_the_store() {
        let fake = FakeBackend::default();
        fake.push_history(1..=5);
        fake.push_thread("5.0", 1..=3);
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), TeamId::from("T1"), events);

        core.handle(Command::OpenThread {
            channel: c1(),
            thread_ts: Ts::from("5.0"),
        });
        // Root + 3 replies at PAGE messages per page: two committed, announced pages.
        assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
        assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });

        let thread = store
            .lock()
            .unwrap()
            .thread_replies(&TeamId::from("T1"), &c1(), &Ts::from("5.0"))
            .unwrap();
        let ts: Vec<_> = thread.iter().map(|m| m.message.ts.0.clone()).collect();
        assert_eq!(ts, ["5.0", "5.0.1", "5.0.2", "5.0.3"]);
        assert_eq!(
            fake.reply_calls(),
            vec![(Ts::from("5.0"), None), (Ts::from("5.0"), Some("2".into())),]
        );
    }

    #[tokio::test]
    async fn following_a_thread_tells_slack_then_rereads_the_thread() {
        let fake = FakeBackend::default();
        fake.push_thread("5.0", 1..=1);
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), TeamId::from("T1"), events);

        core.handle(Command::SetThreadSubscribed {
            channel: c1(),
            thread_ts: Ts::from("5.0"),
            last_read: Ts::from("5.0.1"),
            subscribed: true,
        });
        // The re-read commits the thread, which is what refreshes the root's follow flag.
        assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
        assert_eq!(
            fake.state.lock().unwrap().subscription_calls,
            vec![(Ts::from("5.0"), Ts::from("5.0.1"), true)]
        );
        assert_eq!(fake.reply_calls(), vec![(Ts::from("5.0"), None)]);
    }

    #[tokio::test]
    async fn fetching_channel_managers_announces_them() {
        let fake = FakeBackend::default();
        fake.state.lock().unwrap().managers = vec![UserId::from("U9")];
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake, Arc::clone(&store), TeamId::from("T1"), events);

        core.handle(Command::FetchChannelManagers { channel: c1() });
        assert_eq!(
            next(&mut rx).await,
            Event::ChannelManagers {
                channel: c1(),
                users: vec![UserId::from("U9")],
            }
        );
    }

    #[tokio::test]
    async fn open_thread_reports_api_failures() {
        let fake = FakeBackend::default();
        fake.fail_replies();
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake, Arc::clone(&store), TeamId::from("T1"), events);

        core.handle(Command::OpenThread {
            channel: c1(),
            thread_ts: Ts::from("5.0"),
        });
        assert_eq!(
            next(&mut rx).await,
            Event::AuthExpired("invalid_auth".into())
        );
    }

    #[tokio::test]
    async fn loop_applies_socket_events_in_order_and_stops_on_shutdown() {
        let (store, events, mut rx) = fake_core_parts();
        let core = Core::new(
            FakeBackend::default(),
            Arc::clone(&store),
            TeamId::from("T1"),
            events,
        );
        let (command_tx, command_rx) = mpsc::unbounded();
        let (rtm_tx, rtm_rx) = tokio::sync::mpsc::channel(8);
        let task = tokio::spawn(run(core, command_rx, Some(rtm_rx)));

        rtm_tx
            .send(RtmEvent::State(ConnectionState::Connecting))
            .await
            .unwrap();
        let message: SlackEvent = serde_json::from_str(
            r#"{"type":"message","channel":"C1","user":"U1","text":"live","ts":"5.0"}"#,
        )
        .unwrap();
        rtm_tx.send(RtmEvent::Event(message)).await.unwrap();
        let archived: SlackEvent =
            serde_json::from_str(r#"{"type":"channel_archive","channel":"C1"}"#).unwrap();
        rtm_tx.send(RtmEvent::Event(archived)).await.unwrap();
        drop(rtm_tx);

        assert_eq!(
            next(&mut rx).await,
            Event::Connection(ConnectionState::Connecting)
        );
        assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
        // The live post also goes out as itself, for notifications.
        assert!(matches!(
            next(&mut rx).await,
            Event::MessagePosted { channel, message } if channel == c1() && message.text == "live"
        ));
        assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));

        // The socket stream ended; commands are still served.
        command_tx
            .unbounded_send(Command::OpenConversation(c1()))
            .unwrap();
        // Opening also spawns the open-time `conversations.info` merge, so its
        // `ConversationInfo` event may interleave with the backfill's `StoreChanged`.
        loop {
            match next(&mut rx).await {
                Event::StoreChanged { channel: c } if c == c1() => break,
                Event::ConversationInfo { .. }
                | Event::HistoryComplete { .. }
                | Event::StoreChanged { .. } => {}
                other => panic!("unexpected event after open: {other:?}"),
            }
        }
        command_tx.unbounded_send(Command::Shutdown).unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn open_conversation_merges_member_count_without_clobbering_stored_fields() {
        let fake = FakeBackend::default();
        fake.set_info(Conversation {
            num_members: Some(42),
            ..Default::default()
        });
        // 42 members with no stored profiles: all of them humans, so the count survives the
        // human-only re-count.
        fake.set_members((1..=42).map(|i| UserId::from(format!("U{i}"))).collect());
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake, Arc::clone(&store), TeamId::from("T1"), events);
        let ws = TeamId::from("T1");
        // Stored from `client.userBoot`: no member count, but richer unread and name state.
        store
            .lock()
            .unwrap()
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: c1(),
                    name: Some("general".into()),
                    is_channel: true,
                    unread_count_display: Some(7),
                    ..Default::default()
                },
            )
            .unwrap();

        core.handle(Command::OpenConversation(c1()));

        loop {
            match next(&mut rx).await {
                Event::ConversationInfo {
                    channel,
                    num_members,
                    ..
                } if channel == c1() => {
                    assert_eq!(num_members, Some(42), "the event carries the count");
                    break;
                }
                Event::StoreChanged { .. } | Event::HistoryComplete { .. } => {}
                other => panic!("unexpected event after open: {other:?}"),
            }
        }
        let stored = store.lock().unwrap().conversation(&ws, &c1()).unwrap();
        let stored = stored.expect("the conversation stays stored");
        assert_eq!(stored.num_members, Some(42), "member count lands from info");
        assert_eq!(stored.unread_count_display, Some(7), "boot fields survive");
        assert_eq!(stored.name.as_deref(), Some("general"));
    }

    #[tokio::test]
    async fn open_conversation_counts_humans_and_apps_for_the_header() {
        let fake = FakeBackend::default();
        // Slack's raw count (17) includes apps and deactivated people; the header must not.
        fake.set_info(Conversation {
            num_members: Some(17),
            ..Default::default()
        });
        fake.set_members(vec![
            UserId::from("U1"),
            UserId::from("UBOT"),
            UserId::from("UGONE"),
            UserId::from("USLACKBOT"),
            UserId::from("UEXT"),
        ]);
        let (store, events, mut rx) = fake_core_parts();
        let ws = TeamId::from("T1");
        {
            let store = &mut *store.lock().unwrap();
            for (id, is_bot, deleted) in [
                ("U1", false, false),
                ("UBOT", true, false),
                ("UGONE", true, true),
            ] {
                store
                    .upsert_user(
                        &ws,
                        &User {
                            id: UserId::from(id),
                            is_bot,
                            deleted,
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
        }
        let mut core = Core::new(fake.clone(), Arc::clone(&store), ws.clone(), events);

        core.handle(Command::OpenConversation(c1()));

        loop {
            match next(&mut rx).await {
                Event::ConversationInfo {
                    channel,
                    num_members,
                    num_apps,
                    members,
                } if channel == c1() => {
                    assert_eq!(
                        num_members,
                        Some(2),
                        "U1 and the unknown UEXT are the humans"
                    );
                    assert_eq!(
                        num_apps,
                        Some(1),
                        "the live bot; UGONE is deactivated and USLACKBOT built-in"
                    );
                    assert_eq!(members.len(), 5, "the member id list rides along");
                    break;
                }
                Event::StoreChanged { .. } | Event::HistoryComplete { .. } => {}
                other => panic!("unexpected event after open: {other:?}"),
            }
        }
        assert_eq!(fake.member_calls(), vec![c1()]);
        let stored = store
            .lock()
            .unwrap()
            .conversation(&ws, &c1())
            .unwrap()
            .expect("the conversation stays stored");
        assert_eq!(
            stored.num_members,
            Some(2),
            "the human count lands in the stored conversation"
        );
    }

    #[tokio::test]
    async fn open_conversation_falls_back_to_infos_count_when_members_fail() {
        let fake = FakeBackend::default();
        fake.set_info(Conversation {
            num_members: Some(17),
            ..Default::default()
        });
        fake.fail_members();
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), TeamId::from("T1"), events);

        core.handle(Command::OpenConversation(c1()));

        loop {
            match next(&mut rx).await {
                Event::ConversationInfo {
                    channel,
                    num_members,
                    num_apps,
                    members,
                } if channel == c1() => {
                    assert_eq!(num_members, Some(17), "the raw count stands in");
                    assert_eq!(num_apps, None, "no member list, no app count");
                    assert!(members.is_empty(), "no member list, no ids either");
                    break;
                }
                Event::StoreChanged { .. } | Event::HistoryComplete { .. } => {}
                other => panic!("unexpected event after open: {other:?}"),
            }
        }
        assert_eq!(fake.member_calls(), vec![c1()]);
    }

    #[tokio::test]
    async fn open_conversation_of_connect_channel_resolves_the_connected_teams() {
        let fake = FakeBackend::default();
        fake.set_info(Conversation {
            is_ext_shared: true,
            num_members: Some(5),
            ..Default::default()
        });
        let members = vec![
            UserId::from("U1"),
            UserId::from("UEXT1"),
            UserId::from("UEXT2"),
        ];
        fake.set_members(members.clone());
        fake.set_team(Team {
            id: TeamId::from("T0EXT"),
            name: "Klarna".into(),
            domain: "klarna".into(),
            ..Default::default()
        });
        let (store, events, mut rx) = fake_core_parts();
        let ws = TeamId::from("T1");
        {
            let store = &mut *store.lock().unwrap();
            store
                .upsert_conversation(
                    &ws,
                    &Conversation {
                        id: c1(),
                        is_channel: true,
                        ..Default::default()
                    },
                )
                .unwrap();
            for (id, team) in [("U1", "T1"), ("UEXT1", "T0EXT"), ("UEXT2", "T0EXT")] {
                store
                    .upsert_user(
                        &ws,
                        &User {
                            id: UserId::from(id),
                            team_id: Some(TeamId::from(team)),
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
        }
        let mut core = Core::new(fake.clone(), Arc::clone(&store), ws.clone(), events);

        core.handle(Command::OpenConversation(c1()));

        let mut announced = Vec::new();
        loop {
            match next(&mut rx).await {
                Event::SidebarChanged => break,
                Event::ConversationInfo { members, .. } => announced = members,
                Event::StoreChanged { .. } | Event::HistoryComplete { .. } => {}
                other => panic!("unexpected event after open: {other:?}"),
            }
        }
        assert_eq!(announced, members, "the member ids ride the announcement");
        assert_eq!(fake.team_info_calls(), vec![TeamId::from("T0EXT")]);
        let teams = store.lock().unwrap().teams(&ws).unwrap();
        assert_eq!(
            teams
                .iter()
                .map(|team| team.name.as_str())
                .collect::<Vec<_>>(),
            ["Klarna"],
            "the connected org's record lands beside the workspace's own"
        );
    }

    #[tokio::test]
    async fn set_topic_and_purpose_merge_into_the_stored_conversation() {
        let fake = FakeBackend::default();
        let (store, events, mut rx) = fake_core_parts();
        let ws = TeamId::from("T1");
        store
            .lock()
            .unwrap()
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: c1(),
                    name: Some("general".into()),
                    is_channel: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), ws.clone(), events);

        core.handle(Command::SetChannelTopic {
            channel: c1(),
            topic: "Ship it".into(),
        });
        assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
        core.handle(Command::SetChannelPurpose {
            channel: c1(),
            purpose: "Deploys at 14:00".into(),
        });
        assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
        let stored = store
            .lock()
            .unwrap()
            .conversation(&ws, &c1())
            .unwrap()
            .expect("the conversation stays stored");
        assert_eq!(
            stored.topic.as_ref().map(|t| t.value.as_str()),
            Some("Ship it")
        );
        assert_eq!(
            stored.purpose.as_ref().map(|p| p.value.as_str()),
            Some("Deploys at 14:00")
        );
        assert_eq!(fake.topic_calls(), vec![(c1(), "Ship it".to_owned())]);
        assert_eq!(
            fake.purpose_calls(),
            vec![(c1(), "Deploys at 14:00".to_owned())]
        );
    }

    #[tokio::test]
    async fn hydrate_backfills_previews_only_for_dms_without_stored_messages() {
        let fake = FakeBackend::with_history(1..=3);
        let (store, events, mut rx) = fake_core_parts();
        let ws = TeamId::from("T1");
        let (dm1, dm2) = (ChannelId::from("D1"), ChannelId::from("D2"));
        {
            let store = &mut *store.lock().unwrap();
            for id in [&dm1, &dm2] {
                store
                    .upsert_conversation(
                        &ws,
                        &Conversation {
                            id: id.clone(),
                            is_im: true,
                            is_open: true,
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
            store
                .upsert_message(
                    &ws,
                    &dm1,
                    &Message {
                        ts: Ts::from("1700000000.000100"),
                        text: "already stored".into(),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        let mut core = Core::new(fake.clone(), Arc::clone(&store), ws.clone(), events);
        core.hydrate().await;

        // The preview lands for the message-less DM through the message-store write path.
        loop {
            match next(&mut rx).await {
                Event::StoreChanged { channel } if channel == dm2 => break,
                Event::Conversations(_)
                | Event::UsersChanged(_)
                | Event::Counts(_)
                | Event::SidebarChanged
                | Event::ActivityView { .. } => {}
                other => panic!("unexpected hydrate event: {other:?}"),
            }
        }
        assert_eq!(
            fake.latest_calls(),
            vec![dm2.clone()],
            "only the DM with nothing stored is asked; dm1 and the C1 channel are skipped"
        );
        let latest = store
            .lock()
            .unwrap()
            .latest_messages(&ws, &[dm1.clone(), dm2.clone()])
            .unwrap();
        assert_eq!(
            latest[&dm2].message.text, "m3",
            "the newest message previews"
        );
        assert_eq!(latest[&dm1].message.text, "already stored");
        assert!(
            fake.calls().is_empty(),
            "the preview never walks full history pages"
        );
        assert!(
            !core.backfills.contains_key(&dm2),
            "no backfill was started"
        );

        // Opening the DM still walks the full history from the newest page: one stored
        // preview message neither satisfies nor completes it.
        core.handle(Command::OpenConversation(dm2.clone()));
        loop {
            match next(&mut rx).await {
                Event::HistoryComplete { channel } if channel == dm2 => break,
                Event::StoreChanged { .. } | Event::ConversationInfo { .. } => {}
                other => panic!("unexpected event after open: {other:?}"),
            }
        }
        let calls = fake.calls();
        assert_eq!(
            calls.first(),
            Some(&(None, None)),
            "the walk starts at the newest page"
        );
        assert_eq!(
            calls.len(),
            2,
            "1..=3 at 2 per page: a full walk, not a limit-1 fetch"
        );
        let history = store
            .lock()
            .unwrap()
            .channel_history(&ws, &dm2, None, 10)
            .unwrap();
        assert_eq!(history.len(), 3, "the whole history is stored");
    }

    #[tokio::test]
    async fn hydrate_lists_dms_and_previews_only_open_conversations() {
        let fake = FakeBackend::default();
        fake.push_history(1..=2);
        // A group DM too new for `client.userBoot`'s capped channel list, plus a closed DM.
        fake.set_dms(vec![
            Conversation {
                id: ChannelId::from("M1"),
                name: Some("mpdm-fredrik--linus--jakob-1".into()),
                is_mpim: true,
                is_open: true,
                ..Default::default()
            },
            Conversation {
                id: ChannelId::from("D3"),
                is_im: true,
                is_open: false,
                user: Some(UserId::from("U9")),
                ..Default::default()
            },
        ]);
        let (store, events, mut rx) = fake_core_parts();
        let ws = TeamId::from("T1");
        // A closed group DM that is stored: listed by neither Slack nor this preview fetch.
        store
            .lock()
            .unwrap()
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: ChannelId::from("M2"),
                    is_mpim: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let core = Core::new(fake.clone(), Arc::clone(&store), ws.clone(), events);
        core.hydrate().await;

        loop {
            match next(&mut rx).await {
                Event::StoreChanged { channel } if channel == ChannelId::from("M1") => break,
                Event::Conversations(_)
                | Event::UsersChanged(_)
                | Event::Counts(_)
                | Event::SidebarChanged
                | Event::ActivityView { .. } => {}
                other => panic!("unexpected hydrate event: {other:?}"),
            }
        }
        assert_eq!(fake.dm_calls(), 1);
        assert_eq!(
            fake.latest_calls(),
            vec![ChannelId::from("M1")],
            "only the open group DM is asked; the closed DM and the closed stored one are skipped"
        );
        let asked = [
            ChannelId::from("M1"),
            ChannelId::from("D3"),
            ChannelId::from("M2"),
        ];
        let latest = store.lock().unwrap().latest_messages(&ws, &asked).unwrap();
        assert!(latest.contains_key(&asked[0]), "the new group DM previews");
        assert!(!latest.contains_key(&asked[1]) && !latest.contains_key(&asked[2]));
        let m1 = store
            .lock()
            .unwrap()
            .conversation(&ws, &asked[0])
            .unwrap()
            .expect("the group DM userBoot never delivered lands in the store");
        assert_eq!(m1.name.as_deref(), Some("mpdm-fredrik--linus--jakob-1"));
        assert!(m1.is_open);
        assert!(
            store
                .lock()
                .unwrap()
                .conversation(&ws, &asked[1])
                .unwrap()
                .is_some(),
            "closed DMs are still stored for naming"
        );
    }

    #[tokio::test]
    async fn hydrate_records_channel_memberships_from_users_conversations() {
        let fake = FakeBackend::default();
        // userBoot delivers C1 (its channel objects omit `is_member`); the externally hosted
        // Slack Connect channel and a previously-left one only exist in the store.
        fake.set_memberships(vec![c1(), ChannelId::from("CEXT")]);
        let (store, events, mut rx) = fake_core_parts();
        let ws = TeamId::from("T1");
        {
            let store = &mut *store.lock().unwrap();
            store
                .upsert_conversation(
                    &ws,
                    &Conversation {
                        id: ChannelId::from("CEXT"),
                        name: Some("partner_support_klarna".into()),
                        is_channel: true,
                        is_ext_shared: true,
                        ..Default::default()
                    },
                )
                .unwrap();
            store
                .upsert_conversation(
                    &ws,
                    &Conversation {
                        id: ChannelId::from("CLEFT"),
                        name: Some("avarda-checkout".into()),
                        is_channel: true,
                        is_member: true,
                        is_ext_shared: true,
                        ..Default::default()
                    },
                )
                .unwrap();
            store
                .upsert_conversation(
                    &ws,
                    &Conversation {
                        id: ChannelId::from("M1"),
                        is_mpim: true,
                        is_member: true,
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        let core = Core::new(fake.clone(), Arc::clone(&store), ws.clone(), events);
        core.hydrate().await;

        async fn run_to_sidebar(rx: &mut UnboundedReceiver<Event>) -> Vec<ChannelId> {
            let mut conversations = Vec::new();
            loop {
                match next(rx).await {
                    Event::SidebarChanged => break conversations,
                    Event::Conversations(ids) => conversations.extend(ids),
                    Event::UsersChanged(_) | Event::Counts(_) | Event::ActivityView { .. } => {}
                    other => panic!("unexpected hydrate event: {other:?}"),
                }
            }
        }
        let mut announced = run_to_sidebar(&mut rx).await;
        let mut sorted = std::mem::take(&mut announced);
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted,
            vec![c1(), ChannelId::from("CEXT"), ChannelId::from("CLEFT")],
            "exactly the flipped channels are announced"
        );
        let stored = |id: &str| {
            store
                .lock()
                .unwrap()
                .conversation(&ws, &ChannelId::from(id))
                .unwrap()
                .unwrap_or_else(|| panic!("{id} stored"))
        };
        assert!(stored("C1").is_member, "boot's omitted flag filled in");
        assert!(
            stored("CEXT").is_member,
            "the externally hosted channel counts"
        );
        assert!(!stored("CLEFT").is_member, "left channels drop out");
        assert!(stored("M1").is_member, "group DM rows are never touched");
        assert_eq!(fake.membership_calls(), 1);

        // A re-hydrate replays userBoot before syncing memberships again. The boot upsert
        // preserves stored flags now, so they never dip and the second membership sync
        // flips nothing.
        core.hydrate().await;
        let announced_again = run_to_sidebar(&mut rx).await;
        assert_eq!(
            announced_again,
            vec![c1()],
            "boot still announces its rows; no membership flips a second time"
        );
        assert_eq!(fake.membership_calls(), 2);
        assert!(
            stored("C1").is_member,
            "the boot overwrite is re-synced after it"
        );
        assert!(stored("CEXT").is_member);
        assert!(!stored("CLEFT").is_member);
    }

    #[tokio::test]
    async fn set_notify_level_writes_slack_prefs_then_mirrors_them() {
        let fake = FakeBackend::default();
        let (store, events, mut rx) = fake_core_parts();
        let ws = TeamId::from("T1");
        let mut core = Core::new(fake.clone(), Arc::clone(&store), ws.clone(), events);

        core.handle(Command::SetNotifyLevel {
            channel: c1(),
            level: crate::NotifyLevel::Mentions,
        });
        assert_eq!(next(&mut rx).await, Event::SidebarChanged);
        assert_eq!(
            fake.notification_calls(),
            vec![(c1(), "muted", "false"), (c1(), "desktop", "mention")]
        );
        let prefs = store
            .lock()
            .unwrap()
            .prefs(&ws)
            .unwrap()
            .expect("prefs stored");
        assert_eq!(
            crate::notify_levels(&prefs).get(&c1()),
            Some(&crate::NotifyLevel::Mentions)
        );
    }

    #[tokio::test]
    async fn leave_conversation_stores_it_as_left_once_slack_confirms() {
        let fake = FakeBackend::default();
        let (store, events, mut rx) = fake_core_parts();
        let ws = TeamId::from("T1");
        store
            .lock()
            .unwrap()
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: c1(),
                    is_member: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), ws.clone(), events);

        core.handle(Command::LeaveConversation(c1()));
        assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
        let stored = store.lock().unwrap().conversation(&ws, &c1()).unwrap();
        assert!(!stored.expect("stays stored").is_member, "stored as left");
        assert_eq!(fake.leave_calls(), vec![c1()]);
    }

    #[tokio::test]
    async fn join_conversation_stores_it_as_joined_once_slack_confirms() {
        let fake = FakeBackend::default();
        let (store, events, mut rx) = fake_core_parts();
        let ws = TeamId::from("T1");
        let mut core = Core::new(fake, Arc::clone(&store), ws.clone(), events);

        core.handle(Command::JoinConversation(c1()));
        assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
        let stored = store.lock().unwrap().conversation(&ws, &c1()).unwrap();
        assert!(stored.expect("stored").is_member, "stored as joined");
    }

    #[tokio::test]
    async fn resolve_users_stores_each_missing_profile_once() {
        let fake = FakeBackend::default();
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), TeamId::from("T1"), events);
        let ext = UserId::from("U9");

        core.handle(Command::ResolveUsers(vec![ext.clone(), ext.clone()]));
        assert_eq!(next(&mut rx).await, Event::UsersChanged(vec![ext.clone()]));
        let users = store.lock().unwrap().users(&TeamId::from("T1")).unwrap();
        let names: Vec<&str> = users.iter().map(|user| user.name.as_str()).collect();
        assert_eq!(names, ["ext-U9"], "the profile lands in the store");

        // Asked once per session: a repeat never reaches `users.info` again.
        core.handle(Command::ResolveUsers(vec![ext.clone()]));
        assert_eq!(fake.user_info_calls(), vec![ext]);
    }

    #[tokio::test]
    async fn resolve_users_skips_profiles_the_store_already_has() {
        let fake = FakeBackend::default();
        let (store, events, mut rx) = fake_core_parts();
        let ws = TeamId::from("T1");
        // A full record already in the store (a login-time snapshot): passing its id again -
        // a member list names everyone - asks for nothing.
        store
            .lock()
            .unwrap()
            .upsert_user(
                &ws,
                &User {
                    id: UserId::from("U1"),
                    name: "stored".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), ws, events);

        let ext = UserId::from("U9");
        core.handle(Command::ResolveUsers(vec![UserId::from("U1"), ext.clone()]));
        assert_eq!(next(&mut rx).await, Event::UsersChanged(vec![ext.clone()]));
        assert_eq!(
            fake.user_info_calls(),
            vec![ext],
            "only the unknown id is asked"
        );
    }

    #[tokio::test]
    async fn resolve_users_learns_the_organizations_of_the_people_found() {
        let fake = FakeBackend::default();
        let ext = UserId::from("UEXT");
        fake.set_user_team(ext.clone(), TeamId::from("T0EXT"));
        fake.set_team(Team {
            id: TeamId::from("T0EXT"),
            name: "Klarna".into(),
            ..Default::default()
        });
        let (store, events, mut rx) = fake_core_parts();
        let ws = TeamId::from("T1");
        let mut core = Core::new(fake.clone(), Arc::clone(&store), ws.clone(), events);

        // The external person's profile lands first, then their organization's record, so a
        // Slack Connect channel can name it on its first visit.
        core.handle(Command::ResolveUsers(vec![ext.clone()]));
        assert_eq!(next(&mut rx).await, Event::UsersChanged(vec![ext]));
        assert_eq!(next(&mut rx).await, Event::SidebarChanged);
        assert_eq!(fake.team_info_calls(), vec![TeamId::from("T0EXT")]);
        let names: Vec<String> = store
            .lock()
            .unwrap()
            .teams(&ws)
            .unwrap()
            .into_iter()
            .map(|team| team.name)
            .collect();
        assert_eq!(names, ["Klarna"]);
    }

    #[tokio::test]
    async fn open_conversation_announces_an_empty_member_list_when_info_fails() {
        let fake = FakeBackend::default();
        fake.fail_info();
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), TeamId::from("T1"), events);

        core.handle(Command::OpenConversation(c1()));

        loop {
            match next(&mut rx).await {
                Event::ConversationInfo {
                    channel,
                    num_members,
                    num_apps,
                    members,
                } if channel == c1() => {
                    assert_eq!((num_members, num_apps), (None, None), "nothing is known");
                    assert!(
                        members.is_empty(),
                        "front-ends stop waiting on a member list"
                    );
                    break;
                }
                Event::StoreChanged { .. } | Event::HistoryComplete { .. } | Event::Error(_) => {}
                other => panic!("unexpected event after open: {other:?}"),
            }
        }
        assert!(
            fake.member_calls().is_empty(),
            "no member list without the channel's info"
        );
    }

    #[tokio::test]
    async fn open_dm_fetches_peer_dnd_and_presence_once_each() {
        let fake = FakeBackend::default();
        fake.set_dnd(
            UserId::from("U2"),
            DndStatus {
                dnd_enabled: true,
                next_dnd_start_ts: Some(0),
                next_dnd_end_ts: Some(i64::MAX),
                ..Default::default()
            },
        );
        fake.set_presence(
            UserId::from("U2"),
            UserPresence {
                presence: Presence::Active,
                online: Some(true),
                auto_away: None,
                manual_away: None,
                connection_count: None,
                last_activity: None,
            },
        );
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), TeamId::from("T1"), events);
        let ws = TeamId::from("T1");
        let dm = ChannelId::from("D1");
        store
            .lock()
            .unwrap()
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: dm.clone(),
                    is_im: true,
                    is_open: true,
                    user: Some(UserId::from("U2")),
                    ..Default::default()
                },
            )
            .unwrap();

        core.handle(Command::OpenConversation(dm.clone()));
        let (mut dnd, mut presence) = (false, false);
        for _ in 0..8 {
            match next(&mut rx).await {
                Event::DndPaused { user, paused } => {
                    assert_eq!(user.as_str(), "U2");
                    assert!(paused, "inside the enabled window");
                    dnd = true;
                }
                Event::Presence { users, presence: p } => {
                    assert_eq!(users, vec![UserId::from("U2")]);
                    assert_eq!(p, Presence::Active);
                    presence = true;
                }
                Event::StoreChanged { .. }
                | Event::HistoryComplete { .. }
                | Event::Conversations(_)
                | Event::ConversationInfo { .. } => {}
                other => panic!("unexpected event after open: {other:?}"),
            }
            if dnd && presence {
                break;
            }
        }
        assert!(dnd && presence, "both peer states arrive");

        // Re-opening re-checks DND (transient by design) but never re-polls a presence the
        // websocket subscription already covers.
        core.handle(Command::OpenConversation(dm));
        loop {
            match next(&mut rx).await {
                Event::DndPaused { paused, .. } if paused => break,
                Event::StoreChanged { .. }
                | Event::HistoryComplete { .. }
                | Event::Conversations(_)
                | Event::ConversationInfo { .. } => {}
                other => panic!("unexpected event after reopen: {other:?}"),
            }
        }
        assert_eq!(fake.presence_calls(), vec![UserId::from("U2")]);
        assert_eq!(fake.dnd_calls().len(), 2, "dnd re-fetched per open");
    }

    #[tokio::test]
    async fn fetch_threads_emits_the_view() {
        let fake = FakeBackend::default();
        fake.set_threads_view(api::ThreadsView {
            total_unread_replies: 2,
            threads: vec![api::ThreadSubscription {
                root: Message {
                    ts: Ts::from("5.0"),
                    text: "root".into(),
                    ..Default::default()
                },
                latest_replies: vec![Message {
                    ts: Ts::from("5.0.1"),
                    text: "reply".into(),
                    ..Default::default()
                }],
                unread_replies: Vec::new(),
            }],
            ..Default::default()
        });
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake, Arc::clone(&store), TeamId::from("T1"), events);

        core.handle(Command::FetchThreads);

        let Event::ThreadsView(view) = next(&mut rx).await else {
            panic!("expected the threads view");
        };
        assert_eq!(view.total_unread_replies, 2);
        assert_eq!(view.threads[0].root.ts.as_str(), "5.0");
        assert_eq!(view.threads[0].latest_replies[0].text, "reply");
    }

    #[tokio::test]
    async fn fetch_activity_hydrates_rows_and_marks_route() {
        let fake = FakeBackend::default();
        let feed = api::ActivityFeed {
            items: vec![
                api::ActivityItem {
                    kind: api::ActivityKind::Mention,
                    unread: true,
                    feed_ts: Ts::from("9.1"),
                    key: "at_user-C1-9.1".into(),
                    channel: ChannelId::from("C1"),
                    ts: Ts::from("9.1"),
                    actor: Some(UserId::from("U2")),
                    ..Default::default()
                },
                api::ActivityItem {
                    kind: api::ActivityKind::Dm,
                    feed_ts: Ts::from("8.1"),
                    key: "dm-D2".into(),
                    channel: ChannelId::from("D2"),
                    ts: Ts::from("8.1"),
                    message: Some(Message {
                        ts: Ts::from("8.1"),
                        text: "inline".into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
            next_cursor: None,
        };
        let mut bodies = HashMap::new();
        bodies.insert(
            ChannelId::from("C1"),
            vec![Message {
                ts: Ts::from("9.1"),
                text: "hydrated <@U1>".into(),
                ..Default::default()
            }],
        );
        fake.set_activity(feed, bodies);
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake, Arc::clone(&store), TeamId::from("T1"), events);

        // Without bodies (the boot fetch): the ref row stays empty, the inline one keeps its
        // body, and the badge state still lands.
        core.handle(Command::FetchActivity { with_bodies: false });
        let Event::ActivityView { feed } = next(&mut rx).await else {
            panic!("expected the activity view");
        };
        let feed = feed.expect("the fake feed answers");
        assert!(feed.items[0].message.is_none());
        assert_eq!(feed.items[1].message.as_ref().unwrap().text, "inline");

        // With bodies (the view opened): the ref row is hydrated by (channel, ts).
        core.handle(Command::FetchActivity { with_bodies: true });
        let Event::ActivityView { feed } = next(&mut rx).await else {
            panic!("expected the hydrated activity view");
        };
        let feed = feed.expect("the fake feed answers");
        assert_eq!(
            feed.items[0].message.as_ref().unwrap().text,
            "hydrated <@U1>"
        );

        core.handle(Command::MarkActivity {
            entries: vec![feed.items[0].entry()],
            read: true,
        });
        // The mark runs detached; a further fetch's event (same single-threaded runtime,
        // FIFO polling) only lands after the earlier spawn completed.
        core.handle(Command::FetchActivity { with_bodies: false });
        assert!(matches!(
            next(&mut rx).await,
            Event::ActivityView { feed: Ok(_) }
        ));
        let marks = core.backend.activity_marks();
        assert_eq!(marks.len(), 1);
        assert!(marks[0].0);
        assert_eq!(marks[0].1[0].kind, "at_user");
    }

    #[tokio::test]
    async fn open_dm_stores_and_announces_the_conversation() {
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(
            FakeBackend::default(),
            Arc::clone(&store),
            TeamId::from("T1"),
            events,
        );

        core.handle(Command::OpenDm {
            user: UserId::from("U2"),
        });

        assert_eq!(
            next(&mut rx).await,
            Event::Conversations(vec![ChannelId::from("D9")])
        );
        assert_eq!(
            next(&mut rx).await,
            Event::ConversationOpened(ChannelId::from("D9"))
        );
        let stored = store
            .lock()
            .unwrap()
            .conversation(&TeamId::from("T1"), &ChannelId::from("D9"))
            .unwrap()
            .expect("the DM lands in the store");
        assert_eq!(stored.user, Some(UserId::from("U2")));
    }

    #[tokio::test]
    async fn create_section_reloads_the_stored_sections() {
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(
            FakeBackend::default(),
            Arc::clone(&store),
            TeamId::from("T1"),
            events,
        );

        core.handle(Command::CreateSection {
            name: "Quiet".into(),
        });

        assert_eq!(next(&mut rx).await, Event::SidebarChanged);
        let sections = store.lock().unwrap().sections(&TeamId::from("T1")).unwrap();
        assert!(
            sections
                .iter()
                .any(|section| section.id == "LNEW" && section.name == "Quiet"),
            "the created section is back from the section re-read"
        );
    }

    #[tokio::test]
    async fn rename_section_resends_the_stored_emoji_and_reloads() {
        let fake = FakeBackend::default();
        let (store, events, mut rx) = fake_core_parts();
        store
            .lock()
            .unwrap()
            .replace_sections(&TeamId::from("T1"), &base_sections())
            .unwrap();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), TeamId::from("T1"), events);

        core.handle(Command::RenameSection {
            section: "L2".into(),
            name: "Roadmap".into(),
        });

        assert_eq!(next(&mut rx).await, Event::SidebarChanged);
        // `.set` clears every omitted key, so the stored emoji rides along with the name.
        assert_eq!(
            fake.section_renames(),
            vec![("L2".to_owned(), "Roadmap".to_owned(), "rocket".to_owned())]
        );
        let sections = store.lock().unwrap().sections(&TeamId::from("T1")).unwrap();
        let renamed = sections.iter().find(|section| section.id == "L2").unwrap();
        assert_eq!(
            (renamed.name.as_str(), renamed.emoji.as_str()),
            ("Roadmap", "rocket")
        );
    }

    #[tokio::test]
    async fn delete_section_removes_it_and_reloads() {
        let fake = FakeBackend::default();
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), TeamId::from("T1"), events);

        core.handle(Command::DeleteSection {
            section: "L2".into(),
        });

        assert_eq!(next(&mut rx).await, Event::SidebarChanged);
        assert_eq!(fake.section_deletes(), vec!["L2".to_owned()]);
        let sections = store.lock().unwrap().sections(&TeamId::from("T1")).unwrap();
        assert!(sections.iter().all(|section| section.id != "L2"));
    }

    #[tokio::test]
    async fn move_channel_sends_the_remove_side_and_reloads() {
        let fake = FakeBackend::default();
        let (store, events, mut rx) = fake_core_parts();
        store
            .lock()
            .unwrap()
            .replace_sections(&TeamId::from("T1"), &base_sections())
            .unwrap();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), TeamId::from("T1"), events);

        // Out of the custom section, back to the Channels bucket: a remove only.
        core.handle(Command::MoveChannel {
            channel: c1(),
            from: Some("L2".into()),
            to: None,
        });
        assert_eq!(next(&mut rx).await, Event::SidebarChanged);
        assert_eq!(
            fake.section_moves(),
            vec![(None, Some(("L2".to_owned(), c1())))]
        );
        let sections = store.lock().unwrap().sections(&TeamId::from("T1")).unwrap();
        assert!(
            sections
                .iter()
                .find(|section| section.id == "L2")
                .unwrap()
                .channel_ids
                .is_empty(),
            "the re-read no longer lists the channel in the section"
        );

        // Into a custom section: an insert only ("Priority" lost C1 above).
        core.handle(Command::MoveChannel {
            channel: c1(),
            from: None,
            to: Some("L2".into()),
        });
        assert_eq!(next(&mut rx).await, Event::SidebarChanged);
        assert_eq!(
            fake.section_moves(),
            vec![
                (None, Some(("L2".to_owned(), c1()))),
                (Some(("L2".to_owned(), c1())), None),
            ]
        );
    }

    #[tokio::test]
    async fn create_section_with_channel_moves_the_channel_into_it() {
        let fake = FakeBackend::default();
        let (store, events, mut rx) = fake_core_parts();
        store
            .lock()
            .unwrap()
            .replace_sections(&TeamId::from("T1"), &base_sections())
            .unwrap();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), TeamId::from("T1"), events);

        core.handle(Command::CreateSectionWithChannel {
            name: "Quiet".into(),
            channel: c1(),
        });

        assert_eq!(next(&mut rx).await, Event::SidebarChanged);
        // The channel sat in "Priority": the move carries a remove beside the insert.
        assert_eq!(
            fake.section_moves(),
            vec![(
                Some(("LNEW".to_owned(), c1())),
                Some(("L2".to_owned(), c1()))
            )]
        );
        let sections = store.lock().unwrap().sections(&TeamId::from("T1")).unwrap();
        let fresh = sections
            .iter()
            .find(|section| section.id == "LNEW")
            .unwrap();
        assert_eq!(fresh.channel_ids, vec![c1()]);
    }

    #[tokio::test]
    async fn fetch_latest_history_stores_one_page_without_marking_read() {
        let fake = FakeBackend::with_history(1..=60);
        let (store, events, mut rx) = fake_core_parts();
        let mut core = Core::new(fake.clone(), Arc::clone(&store), TeamId::from("T1"), events);

        core.handle(Command::FetchLatestHistory(c1()));

        assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
        let state = fake.state.lock().unwrap();
        assert_eq!(state.recent_calls, vec![(c1(), 50)]);
        assert!(state.mark_calls.is_empty());
        assert!(state.calls.is_empty(), "no multi-page backfill walk");
        drop(state);

        let stored = store
            .lock()
            .unwrap()
            .channel_history(&TeamId::from("T1"), &c1(), None, 100)
            .unwrap();
        assert_eq!(stored.len(), 50);
    }
}
