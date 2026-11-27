//! Persisted offline mutation queue (`offline_queue`) and durable send outbox (`outbox`, V13)
//! with idempotent sequential FIFO flushing on reconnect and capped-backoff re-flush after a
//! transient failure while connected.

use std::time::Duration;

use super::super::*;
use super::messaging::{mark_read_and_store, post_message_and_store};
use crate::api::new_client_msg_id;
use crate::outbox::{OutboxPayload, decode_outbox_payload, is_transient_send_error};
use crate::store::{NewOutboxItem, OutboxItem};

/// Delay before the first automatic outbox re-flush after a transient send failure.
const OUTBOX_RETRY_INITIAL: Duration = Duration::from_secs(5);
/// Ceiling of the automatic outbox re-flush backoff.
const OUTBOX_RETRY_MAX: Duration = Duration::from_secs(5 * 60);

/// The next re-flush backoff: double the previous delay, capped at [`OUTBOX_RETRY_MAX`].
pub(in crate::client) fn next_outbox_retry_delay(prev: Duration) -> Duration {
    prev.saturating_mul(2).min(OUTBOX_RETRY_MAX)
}

/// Automatic re-flush of the outbox after a flush pass stops on a transient send failure while
/// the socket stays connected. The backoff for the next retry and the single pending timer, so
/// overlapping flushes never pile up retries. A timer that fires while offline still runs its
/// pass: the HTTP API is independent of the websocket, and a failure just backs off further.
#[derive(Clone, Default)]
pub(in crate::client) struct OutboxRetry(Arc<Mutex<RetryState>>);

struct RetryState {
    /// Backoff of the next retry; starts at [`OUTBOX_RETRY_INITIAL`].
    delay: Duration,
    /// The pending timer task, if any.
    pending: Option<AbortHandle>,
    /// Monotonic counter bumped on every [`OutboxRetry::schedule`], so a flushing pass that
    /// finishes with `Flush::Proceed` does not cancel a retry armed concurrently by
    /// [`park_transient_send`].
    generation: u64,
}

impl Default for RetryState {
    fn default() -> Self {
        Self {
            delay: OUTBOX_RETRY_INITIAL,
            pending: None,
            generation: 0,
        }
    }
}

impl OutboxRetry {
    /// Current schedule generation, captured before a flush pass snapshots pending items.
    fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Schedules the single re-flush with the current backoff (then doubles it), replacing any
    /// pending timer.
    fn schedule<B: Backend>(&self, flush: OutboxFlush<B>) {
        let mut state = self.lock();
        state.generation = state.generation.wrapping_add(1);
        let delay = state.delay;
        state.delay = next_outbox_retry_delay(delay);
        state.abort_pending();
        let retry = self.clone();
        let tasks = flush.tasks.clone();
        state.pending = tasks.spawn(async move {
            tokio::time::sleep(delay).await;
            // This timer has fired: take it out (without aborting) so the pass's own outcome
            // never aborts the task running it.
            retry.lock().pending = None;
            flush.run().await;
        });
    }

    /// Cancels the pending re-flush and resets the backoff (a drained queue, or `Connected`,
    /// which flushes at once).
    pub(in crate::client) fn reset(&self) {
        let mut state = self.lock();
        state.delay = OUTBOX_RETRY_INITIAL;
        state.abort_pending();
    }

    /// Like [`Self::reset`], but keeps any retry timer armed *after* `expected_generation`
    /// (e.g. an item parked while a flush pass was delivering earlier items).
    fn reset_if_unchanged(&self, expected_generation: u64) {
        let mut state = self.lock();
        if state.generation == expected_generation {
            state.delay = OUTBOX_RETRY_INITIAL;
            state.abort_pending();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RetryState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl RetryState {
    /// Aborts the pending timer, if any (dropping an [`AbortHandle`] alone does not abort).
    fn abort_pending(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.abort();
        }
    }
}

/// What a flush pass does after one item.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Flush {
    Proceed,
    /// The item is still `pending` (transient send failure, busy store), so another flush is due.
    RetryLater,
    /// Auth expired or shutdown: a retry cannot fix it.
    Stop,
}

/// One serialized FIFO flush pass over the pending outbox items, with the state the automatic
/// re-flush needs to repeat it.
#[derive(Clone)]
pub(super) struct OutboxFlush<B: Backend> {
    ctx: TaskCtx<B>,
    lock: Arc<tokio::sync::Mutex<()>>,
    retry: OutboxRetry,
    tasks: TaskTracker,
}

impl<B: Backend> OutboxFlush<B> {
    /// Delivers every pending item in order under the flush lock, then schedules the re-flush a
    /// transient failure needs or resets the backoff once the queue is drained.
    async fn run(&self) {
        let generation = self.retry.generation();
        let outcome = {
            let _guard = self.lock.lock().await;
            let ws = self.ctx.workspace.clone();
            // `self.lock` serializes every in-process delivery, so any row still marked `sending`
            // here was stranded by a failed SQLite status update on an earlier pass: recover it.
            match with_store(&self.ctx.store, move |s| {
                let _ = s.reset_outbox_sending_to_pending(&ws)?;
                s.list_outbox_pending(&ws)
            })
            .await
            {
                Ok(pending) => {
                    let mut outcome = Flush::Proceed;
                    for item in pending {
                        outcome = deliver_claimed_outbox_item(&self.ctx, item).await;
                        if outcome != Flush::Proceed {
                            break;
                        }
                    }
                    outcome
                }
                // Another process may hold the database (SQLITE_BUSY): retry later instead of
                // dropping the pass.
                Err(err) => {
                    tracing::warn!(%err, "failed to load pending outbox items");
                    Flush::RetryLater
                }
            }
        };
        match outcome {
            Flush::Proceed => self.retry.reset_if_unchanged(generation),
            Flush::RetryLater => self.retry.schedule(self.clone()),
            Flush::Stop => {}
        }
    }

    /// Delivers one item under the flush lock. Only a transient failure touches the backoff: one
    /// delivered item says nothing about the items still queued behind it.
    async fn run_one(&self, item: OutboxItem) {
        let outcome = {
            let _guard = self.lock.lock().await;
            deliver_claimed_outbox_item(&self.ctx, item).await
        };
        if outcome == Flush::RetryLater {
            self.retry.schedule(self.clone());
        }
    }
}

/// Park a send or reaction that failed transiently while online: enqueue it in the outbox and
/// arm the automatic re-flush, since no reconnect will come to flush it.
pub(super) async fn park_transient_send<B: Backend>(
    flush: &OutboxFlush<B>,
    channel: ChannelId,
    thread_ts: Option<Ts>,
    payload: OutboxPayload,
    err: &ApiError,
) -> Result<OutboxItem, CoreError> {
    let item = enqueue_outbox_with_ctx(
        &flush.ctx,
        channel,
        thread_ts,
        payload,
        Some(err.to_string()),
    )
    .await?;
    flush.retry.schedule(flush.clone());
    Ok(item)
}

/// Read marker persisted in SQLite `offline_queue` until flushed on reconnect (sends and
/// reactions queue in the outbox instead; see [`migrate_legacy_offline_sends`]).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(in crate::client) enum QueuedMutation {
    MarkRead { channel: ChannelId, ts: Ts },
}

impl QueuedMutation {
    pub(in crate::client) fn from_command(command: &Command) -> Option<Self> {
        let Command::Messaging(cmd) = command else {
            return None;
        };
        match cmd {
            MessagingCommand::MarkRead { channel, ts } => Some(Self::MarkRead {
                channel: channel.clone(),
                ts: ts.clone(),
            }),
            _ => None,
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::MarkRead { channel, .. } => format!("Mark read ({channel})"),
        }
    }
}

/// The `offline_queue` rows written before the outbox existed (sends and reactions, which the
/// outbox now holds); shape only, for the one-time move at startup.
#[derive(serde::Deserialize)]
enum LegacyQueuedSend {
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
}

impl LegacyQueuedSend {
    fn command(self) -> Command {
        Command::Messaging(match self {
            Self::SendMessage {
                channel,
                content,
                thread_ts,
                broadcast,
            } => MessagingCommand::SendMessage {
                channel,
                content,
                thread_ts,
                broadcast,
            },
            Self::AddReaction { channel, ts, name } => {
                MessagingCommand::AddReaction { channel, ts, name }
            }
            Self::RemoveReaction { channel, ts, name } => {
                MessagingCommand::RemoveReaction { channel, ts, name }
            }
        })
    }
}

/// Moves `workspace`'s legacy queued sends and reactions into the outbox in queue order (each
/// with its own `client_msg_id`) and deletes their `offline_queue` rows. Read markers stay. A row
/// that is neither a legacy send nor valid JSON is left alone. Returns how many rows moved.
pub(in crate::client) fn migrate_legacy_offline_sends(
    store: &mut Store,
    workspace: &TeamId,
) -> crate::store::Result<usize> {
    let mut moved = 0;
    for (id, payload) in store.list_offline(workspace)? {
        let Ok(legacy) = serde_json::from_str::<LegacyQueuedSend>(&payload) else {
            continue;
        };
        let Some((channel, thread_ts, payload)) = outbox_entry_from_command(&legacy.command())
        else {
            continue;
        };
        let payload_json = payload.encode();
        store.enqueue_outbox(
            workspace,
            &NewOutboxItem {
                channel_id: &channel,
                thread_ts: thread_ts.as_ref(),
                kind: payload.kind(),
                payload_json: &payload_json,
                last_error: None,
            },
        )?;
        store.delete_offline(workspace, id)?;
        moved += 1;
    }
    Ok(moved)
}

/// Extract a durable V13 outbox payload from send/reply/reaction commands.
fn outbox_entry_from_command(command: &Command) -> Option<(ChannelId, Option<Ts>, OutboxPayload)> {
    let Command::Messaging(cmd) = command else {
        return None;
    };
    match cmd {
        MessagingCommand::SendMessage {
            channel,
            content,
            thread_ts,
            broadcast,
        } => {
            // One id for every flush attempt of this entry (see `post_message_and_store`).
            let client_msg_id = Some(new_client_msg_id());
            let payload = if thread_ts.is_some() {
                OutboxPayload::Reply {
                    content: content.clone(),
                    broadcast: *broadcast,
                    client_msg_id,
                }
            } else {
                OutboxPayload::Message {
                    content: content.clone(),
                    client_msg_id,
                }
            };
            Some((channel.clone(), thread_ts.clone(), payload))
        }
        MessagingCommand::ReplyThread {
            channel,
            thread_ts,
            content,
            broadcast,
        } => Some((
            channel.clone(),
            Some(thread_ts.clone()),
            OutboxPayload::Reply {
                content: content.clone(),
                broadcast: *broadcast,
                client_msg_id: Some(new_client_msg_id()),
            },
        )),
        MessagingCommand::AddReaction { channel, ts, name } => Some((
            channel.clone(),
            None,
            OutboxPayload::ReactionAdd {
                ts: ts.clone(),
                name: name.clone(),
            },
        )),
        MessagingCommand::RemoveReaction { channel, ts, name } => Some((
            channel.clone(),
            None,
            OutboxPayload::ReactionRemove {
                ts: ts.clone(),
                name: name.clone(),
            },
        )),
        _ => None,
    }
}

/// Persist a send/reply/reaction into the durable V13 `outbox` table and emit
/// [`Event::OutboxUpdated`].
pub(super) async fn enqueue_outbox_with_ctx<B: Backend>(
    ctx: &TaskCtx<B>,
    channel: ChannelId,
    thread_ts: Option<Ts>,
    payload: OutboxPayload,
    last_error: Option<String>,
) -> Result<OutboxItem, CoreError> {
    let ws = ctx.workspace.clone();
    let kind = payload.kind();
    let payload_json = payload.encode();
    let (item, all) = with_store(&ctx.store, move |s| {
        let inserted = s.enqueue_outbox(
            &ws,
            &NewOutboxItem {
                channel_id: &channel,
                thread_ts: thread_ts.as_ref(),
                kind,
                payload_json: &payload_json,
                last_error: last_error.as_deref(),
            },
        )?;
        let all = s.list_outbox(&ws)?;
        Ok((inserted, all))
    })
    .await?;
    ctx.events.emit(Event::OutboxUpdated(all));
    Ok(item)
}

async fn execute_outbox_item<B: Backend>(
    ctx: &TaskCtx<B>,
    item: &OutboxItem,
) -> Result<(), CoreError> {
    let Some(payload) = decode_outbox_payload(item) else {
        return Err(CoreError::Api(ApiError::Slack(
            "corrupt_outbox_payload".into(),
        )));
    };
    match payload {
        OutboxPayload::Message {
            content,
            client_msg_id,
        } => {
            post_message_and_store(
                ctx,
                item.channel_id.clone(),
                content,
                None,
                false,
                client_msg_id,
            )
            .await?;
        }
        OutboxPayload::Reply {
            content,
            broadcast,
            client_msg_id,
        } => {
            post_message_and_store(
                ctx,
                item.channel_id.clone(),
                content,
                item.thread_ts.clone(),
                broadcast,
                client_msg_id,
            )
            .await?;
        }
        OutboxPayload::ReactionAdd { ts, name } => {
            match ctx
                .backend
                .add_reaction(item.channel_id.clone(), ts, name)
                .await
            {
                Ok(()) => {}
                // Idempotent retry: Slack returns `already_reacted` if an earlier attempt
                // succeeded before the connection dropped.
                Err(ApiError::Slack(ref code)) if code == "already_reacted" => {}
                Err(err) => return Err(err.into()),
            }
        }
        OutboxPayload::ReactionRemove { ts, name } => {
            match ctx
                .backend
                .remove_reaction(item.channel_id.clone(), ts, name)
                .await
            {
                Ok(()) => {}
                // Idempotent retry: Slack returns `no_reaction` if an earlier attempt already
                // removed the reaction.
                Err(ApiError::Slack(ref code)) if code == "no_reaction" => {}
                Err(err) => return Err(err.into()),
            }
        }
    }
    Ok(())
}

/// Claim, execute, and finalize one outbox item inside the serialized flush lock. Returns the
/// action for the items queued behind this one: proceed, retry later (a transient failure they
/// must not overtake), or stop (auth expired, cancelled).
async fn deliver_claimed_outbox_item<B: Backend>(ctx: &TaskCtx<B>, item: OutboxItem) -> Flush {
    let id = item.id;
    let ws = ctx.workspace.clone();
    let claimed = match with_store(&ctx.store, move |s| {
        let claimed = s.mark_outbox_sending(id)?;
        let all = if claimed {
            Some(s.list_outbox(&ws)?)
        } else {
            None
        };
        Ok((claimed, all))
    })
    .await
    {
        Ok((true, all)) => {
            if let Some(items) = all {
                ctx.events.emit(Event::OutboxUpdated(items));
            }
            true
        }
        Ok((false, _)) => false,
        Err(err) => {
            tracing::warn!(id, %err, "failed to claim outbox item for sending");
            return Flush::RetryLater;
        }
    };
    if !claimed {
        return Flush::Proceed;
    }

    match execute_outbox_item(ctx, &item).await {
        Ok(()) => {
            let ws = ctx.workspace.clone();
            match with_store(&ctx.store, move |s| {
                s.delete_outbox_item(id)?;
                s.list_outbox(&ws)
            })
            .await
            {
                Ok(all) => {
                    ctx.events.emit(Event::OutboxUpdated(all));
                }
                Err(err) => tracing::warn!(id, %err, "failed to delete sent outbox item"),
            }
            Flush::Proceed
        }
        Err(CoreError::Api(ApiError::AuthExpired(code))) => {
            let ws = ctx.workspace.clone();
            let err_msg = format!("session expired ({code})");
            match with_store(&ctx.store, move |s| {
                s.mark_outbox_failed(id, &err_msg)?;
                s.list_outbox(&ws)
            })
            .await
            {
                Ok(all) => {
                    ctx.events.emit(Event::OutboxUpdated(all));
                }
                Err(err) => tracing::warn!(id, %err, "failed to mark outbox item failed"),
            }
            ctx.events.emit(Event::AuthExpired(code));
            Flush::Stop
        }
        Err(CoreError::Cancelled) => {
            let ws = ctx.workspace.clone();
            if let Ok(all) = with_store(&ctx.store, move |s| {
                s.reset_outbox_item_pending(id)?;
                s.list_outbox(&ws)
            })
            .await
            {
                ctx.events.emit(Event::OutboxUpdated(all));
            }
            Flush::Stop
        }
        // Slack or the network may recover: back to `pending` for the scheduled re-flush (which
        // reuses the message's `client_msg_id`), and stop this pass so later items cannot overtake
        // it.
        Err(CoreError::Api(ref err)) if is_transient_send_error(err) => {
            tracing::info!(id, %err, "outbox send failed transiently; re-flushing with backoff");
            let ws = ctx.workspace.clone();
            match with_store(&ctx.store, move |s| {
                s.reset_outbox_item_pending(id)?;
                s.list_outbox(&ws)
            })
            .await
            {
                Ok(all) => {
                    ctx.events.emit(Event::OutboxUpdated(all));
                }
                Err(store_err) => {
                    tracing::warn!(id, %store_err, "failed to return outbox item to pending");
                }
            }
            Flush::RetryLater
        }
        Err(err) => {
            let ws = ctx.workspace.clone();
            let err_msg = err.to_string();
            match with_store(&ctx.store, move |s| {
                s.mark_outbox_failed(id, &err_msg)?;
                s.list_outbox(&ws)
            })
            .await
            {
                Ok(all) => {
                    ctx.events.emit(Event::OutboxUpdated(all));
                }
                Err(store_err) => {
                    tracing::warn!(id, %store_err, "failed to mark outbox item failed");
                }
            }
            Flush::Proceed
        }
    }
}

impl<B: Backend> Core<B> {
    /// If the client is offline and `command` is queueable, persist it in SQLite (off the async
    /// worker thread) and return `true`.
    ///
    /// Send/reply/reaction commands go into the durable V13 `outbox` table; read markers use the
    /// legacy `offline_queue` table (respecting Stealth Mode).
    pub(super) async fn enqueue_or_run(&mut self, command: &Command) -> bool {
        if !self.is_offline {
            return false;
        }
        if let Some((channel, thread_ts, payload)) = outbox_entry_from_command(command) {
            let ctx = self.ctx();
            if let Err(err) = enqueue_outbox_with_ctx(&ctx, channel, thread_ts, payload, None).await
            {
                tracing::warn!(%err, "failed to persist offline outbox entry");
                self.events
                    .emit(Event::Error(format!("outbox enqueue failed: {err}")));
            }
            return true;
        }
        if let Some(mutation) = QueuedMutation::from_command(command) {
            // Defense-in-depth: never persist read receipts while Stealth Mode is enabled.
            if self.stealth_mode.load(Ordering::Relaxed)
                && matches!(mutation, QueuedMutation::MarkRead { .. })
            {
                return true;
            }
            let payload = serde_json::to_string(&mutation).unwrap_or_default();
            let ws = self.workspace.clone();
            let id = match with_store(&self.store, move |s| s.enqueue_offline(&ws, &payload)).await
            {
                Ok(id) => id,
                Err(err) => {
                    tracing::warn!(%err, "failed to persist offline mutation");
                    0
                }
            };
            self.offline_queue.push_back((id, mutation));
            self.events
                .emit(Event::OfflineQueueChanged(self.offline_queue.len()));
            return true;
        }
        false
    }

    /// Flush all `pending` V13 outbox items for this workspace strictly sequentially in FIFO
    /// order. Serialized by `outbox_flush_lock` plus SQLite `status != 'sending'` atomic claims
    /// so overlapping reconnects or manual flushes are idempotent and never double-send.
    /// No-ops while offline so pending items remain queued for automatic flush on reconnect.
    pub(in crate::client) fn flush_outbox(&self) {
        if self.is_offline {
            return;
        }
        let flush = self.outbox_flush();
        self.tasks.spawn(async move { flush.run().await });
    }

    pub(super) fn outbox_flush(&self) -> OutboxFlush<B> {
        OutboxFlush {
            ctx: self.ctx(),
            lock: Arc::clone(&self.outbox_flush_lock),
            retry: self.outbox_retry.clone(),
            tasks: self.tasks.clone(),
        }
    }

    /// Retry a single V13 outbox item (whether `failed` or `pending`).
    /// While offline, resets the item back to `pending` so it flushes on reconnect; while online,
    /// attempts delivery immediately under the serialized flush lock.
    pub(super) async fn retry_outbox_item(&mut self, id: i64) {
        let ws = self.workspace.clone();
        let item = match with_store(&self.store, move |s| s.get_outbox_item(id)).await {
            Ok(Some(item)) if item.workspace_id == ws => item,
            Ok(_) => return,
            Err(err) => {
                tracing::warn!(id, %err, "failed to read outbox item for retry");
                return;
            }
        };
        if self.is_offline {
            let ws = self.workspace.clone();
            match with_store(&self.store, move |s| {
                s.reset_outbox_item_pending(id)?;
                s.list_outbox(&ws)
            })
            .await
            {
                Ok(all) => {
                    self.events.emit(Event::OutboxUpdated(all));
                }
                Err(err) => tracing::warn!(id, %err, "failed to reset outbox item pending"),
            }
            return;
        }
        let flush = self.outbox_flush();
        self.tasks.spawn(async move { flush.run_one(item).await });
    }

    /// Permanently remove a V13 outbox item owned by this workspace and emit
    /// [`Event::OutboxUpdated`].
    pub(super) async fn discard_outbox_item(&mut self, id: i64) {
        let ws = self.workspace.clone();
        match with_store(&self.store, move |s| {
            let owned = s
                .get_outbox_item(id)?
                .is_some_and(|item| item.workspace_id == ws);
            if !owned {
                return Ok(None);
            }
            s.delete_outbox_item(id)?;
            Ok(Some(s.list_outbox(&ws)?))
        })
        .await
        {
            Ok(Some(all)) => {
                self.events.emit(Event::OutboxUpdated(all));
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(id, %err, "failed to discard outbox item"),
        }
    }

    pub(super) async fn retry_offline_mutation(&mut self, id: i64) {
        let ws = self.workspace.clone();
        let rows = match with_store(&self.store, move |s| s.list_offline(&ws)).await {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(%err, "failed to read offline queue for retry");
                return;
            }
        };
        let found = rows
            .into_iter()
            .find(|(row_id, _)| *row_id == id)
            .and_then(|(_, payload)| serde_json::from_str::<QueuedMutation>(&payload).ok());
        if let Some(mutation) = found {
            if self.is_offline {
                if !self.offline_queue.iter().any(|(row_id, _)| *row_id == id) {
                    self.offline_queue.push_back((id, mutation));
                    self.events
                        .emit(Event::OfflineQueueChanged(self.offline_queue.len()));
                }
            } else {
                self.flush_queued_mutation(id, mutation);
            }
        }
    }

    pub(super) async fn discard_offline_mutation(&mut self, id: i64) {
        self.offline_queue.retain(|(row_id, _)| *row_id != id);
        let ws = self.workspace.clone();
        if let Err(err) = with_store(&self.store, move |s| s.delete_offline(&ws, id)).await {
            tracing::warn!(id, %err, "failed to delete discarded offline mutation");
        }
        self.events
            .emit(Event::OfflineQueueChanged(self.offline_queue.len()));
    }

    /// Execute a batch of persisted offline mutations strictly sequentially (FIFO), deleting each
    /// SQLite row on success or emitting [`Event::OfflineMutationFailed`] on failure.
    pub(in crate::client) fn flush_queued_mutations_sequentially(
        &self,
        queued: Vec<(i64, QueuedMutation)>,
    ) {
        let ctx = self.ctx();
        let stealth_mode = Arc::clone(&self.stealth_mode);
        self.tasks.spawn(async move {
            for (id, mutation) in queued {
                let summary = mutation.summary();
                let outcome = Self::execute_queued_mutation(&ctx, &stealth_mode, mutation).await;
                match outcome {
                    Ok(()) => {
                        if id > 0 {
                            let ws = ctx.workspace.clone();
                            if let Err(err) =
                                with_store(&ctx.store, move |s| s.delete_offline(&ws, id)).await
                            {
                                tracing::warn!(id, %err, "failed to delete flushed offline mutation");
                            }
                        }
                    }
                    Err(CoreError::Api(ApiError::AuthExpired(code))) => {
                        ctx.events.emit(Event::AuthExpired(code));
                        break;
                    }
                    Err(CoreError::Cancelled) => break,
                    Err(err) => {
                        ctx.events
                            .emit(Event::OfflineMutationFailed(FailedMutation {
                                id,
                                summary,
                                error: err.to_string(),
                            }));
                    }
                }
            }
        });
    }

    /// Execute one persisted offline mutation via [`Self::flush_queued_mutations_sequentially`].
    pub(in crate::client) fn flush_queued_mutation(&self, id: i64, mutation: QueuedMutation) {
        self.flush_queued_mutations_sequentially(vec![(id, mutation)]);
    }

    pub(in crate::client) async fn execute_queued_mutation(
        ctx: &TaskCtx<B>,
        stealth_mode: &AtomicBool,
        mutation: QueuedMutation,
    ) -> Result<(), CoreError> {
        let QueuedMutation::MarkRead { channel, ts } = mutation;
        // Never leak read receipts if Stealth Mode was enabled while queued.
        if stealth_mode.load(Ordering::Relaxed) {
            tracing::debug!(%channel, "suppressing queued MarkRead under Stealth Mode");
            return Ok(());
        }
        // A newer cursor already in the store (another device read further while this one was
        // offline) wins: replaying this older mark would make the conversation unread again
        // everywhere.
        let ws = ctx.workspace.clone();
        let target = channel.clone();
        let stored = with_store(&ctx.store, move |s| s.conversation(&ws, &target)).await?;
        if stored
            .and_then(|conv| conv.last_read)
            .is_some_and(|last_read| last_read > ts)
        {
            tracing::debug!(%channel, %ts, "skipping stale queued MarkRead");
            return Ok(());
        }
        mark_read_and_store(ctx, channel, ts).await?;
        Ok(())
    }
}
