//! The runtime thread, the command loop and the front-end handle.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::AbortHandle;

use crate::api::{ApiError, SlackApi};
use crate::auth::Session;
use crate::backend::Backend;
use crate::backfill::{Backfill, PAGE_DELAY, SyncedMap};
use crate::fetcher::Fetcher;
use crate::rtm::{ConnectParams, RtmClient, RtmConfig, RtmEvent};
use crate::store::{SearchQuery, Store, StoreError};
use crate::types::{ChannelId, SlackEvent, TeamId};
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
        let fetcher = Fetcher::new(runtime.handle().clone());
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

pub(crate) struct Core<B> {
    backend: B,
    store: SharedStore,
    workspace: TeamId,
    events: Emitter,
    backfill: Backfill<B>,
    backfills: HashMap<ChannelId, AbortHandle>,
    rtm: Option<RtmClient>,
}

impl<B: Backend> Core<B> {
    pub fn new(backend: B, store: SharedStore, workspace: TeamId, events: Emitter) -> Self {
        let backfill = Backfill {
            backend: backend.clone(),
            store: Arc::clone(&store),
            workspace: workspace.clone(),
            events: events.clone(),
            synced: SyncedMap::default(),
            page_delay: PAGE_DELAY,
        };
        Self {
            backend,
            store,
            workspace,
            events,
            backfill,
            backfills: HashMap::new(),
            rtm: None,
        }
    }

    /// Startup hydrate: conversations, self, team and prefs from `client.userBoot`, the user
    /// directory, unread counts, then sidebar sections and custom emoji (one
    /// [`Event::SidebarChanged`] at the end). Every step reports its own failure without
    /// aborting the rest.
    pub async fn hydrate(&self) {
        match self.backend.user_boot().await {
            Ok(boot) => {
                let team = boot.team.clone();
                let prefs = boot.prefs.clone();
                let ws = self.workspace.clone();
                let stored = with_store(&self.store, move |store| {
                    let mut ids = Vec::new();
                    for conversation in boot.channels.iter().chain(&boot.ims) {
                        store.upsert_conversation(&ws, conversation)?;
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
    }

    pub fn handle(&mut self, command: Command) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        match command {
            Command::OpenConversation(channel) => self.open(channel),
            Command::CloseConversation(channel) => {
                if let Some(task) = self.backfills.remove(&channel) {
                    task.abort();
                }
            }
            Command::OpenThread { channel, thread_ts } => {
                let backfill = self.backfill.clone();
                self.spawn("conversations.replies", async move {
                    backfill.fetch_thread(channel, thread_ts).await
                });
            }
            Command::SendMessage {
                channel,
                text,
                thread_ts,
            } => {
                self.spawn("chat.postMessage", async move {
                    let posted = backend
                        .post_message(channel.clone(), text, thread_ts)
                        .await?;
                    // Store the echo now; the websocket copy of it is an idempotent re-upsert.
                    if let Some(message) = posted.message {
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
            Command::EditMessage { channel, ts, text } => self.spawn("chat.update", async move {
                backend.update_message(channel, ts, text).await?;
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
            Command::Typing { channel, thread_ts } => {
                if let Some(Err(e)) = self.rtm.as_ref().map(|r| r.user_typing(channel, thread_ts)) {
                    events.emit(Event::Error(format!("typing: {e}")));
                }
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
                self.events.emit(Event::Connection(state));
            }
            RtmEvent::Event(event) => {
                if let Some(channel) = refresh_target(&event) {
                    self.refresh_conversation(channel);
                }
                let ws = self.workspace.clone();
                let changed = with_store(&self.store, move |s| apply_event(s, &ws, event)).await;
                if let Some(changed) = changed {
                    self.events.emit(changed);
                }
            }
        }
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

/// Conversation events without a conversation object; the store needs a fresh copy.
fn refresh_target(event: &SlackEvent) -> Option<ChannelId> {
    match event {
        SlackEvent::ChannelArchive(e)
        | SlackEvent::ChannelUnarchive(e)
        | SlackEvent::ChannelLeft(e) => Some(e.channel.clone()),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::ops::RangeInclusive;
    use std::time::Duration;

    use super::*;
    use crate::api::{self, ClientCounts, Page, PostedMessage, SearchMessages, UserBoot};
    use crate::rtm::ConnectionState;
    use crate::types::{
        ChannelSection, Conversation, Message, SectionKind, Team, Ts, User, UserId,
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
        reply_calls: Vec<(Ts, Option<String>)>,
        rate_limited: u32,
        fail_replies: bool,
    }

    impl FakeBackend {
        pub fn with_history(range: RangeInclusive<u32>) -> Self {
            let fake = Self::default();
            fake.push_history(range);
            fake
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
            // Stored order is deliberately not display order: L2 points at L1.
            Ok(vec![
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
            ])
        }

        async fn custom_emoji(&self) -> api::Result<HashMap<String, String>> {
            Ok(HashMap::from([(
                "party".to_owned(),
                "https://emoji.example/party.png".to_owned(),
            )]))
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
            Ok(Conversation {
                id: channel,
                is_archived: true,
                ..Default::default()
            })
        }

        async fn post_message(
            &self,
            channel: ChannelId,
            text: String,
            _thread_ts: Option<Ts>,
        ) -> api::Result<PostedMessage> {
            let ts = Ts::from("100.0");
            let message = Message {
                ts: ts.clone(),
                text,
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
            _: String,
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

        async fn search(&self, _: String) -> api::Result<SearchMessages> {
            Ok(SearchMessages {
                total: 7,
                ..Default::default()
            })
        }

        async fn mark_read(&self, _: ChannelId, _: Ts) -> api::Result<()> {
            Err(ApiError::AuthExpired("invalid_auth".into()))
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
            text: "hi there".into(),
            thread_ts: None,
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
        assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));

        // The socket stream ended; commands are still served.
        command_tx
            .unbounded_send(Command::OpenConversation(c1()))
            .unwrap();
        assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
        command_tx.unbounded_send(Command::Shutdown).unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    }
}
