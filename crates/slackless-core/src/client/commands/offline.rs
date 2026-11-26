//! Persisted offline mutation queue (`offline_queue`) and durable send outbox (`outbox`, V13)
//! with idempotent sequential FIFO flushing on reconnect.

use super::super::*;
use super::messaging::{mark_read_and_store, post_message_and_store};
use crate::api::new_client_msg_id;
use crate::outbox::{OutboxPayload, decode_outbox_payload, is_transient_send_error};
use crate::store::{NewOutboxItem, OutboxItem};

/// Serializable offline mutation stored in SQLite `offline_queue` until flushed on reconnect.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(in crate::client) enum QueuedMutation {
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

    pub(in crate::client) fn summary(&self) -> String {
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

/// Claim, execute, and finalize one outbox item inside the serialized flush lock.
/// Returns `false` if the caller should stop flushing further items: auth expired, cancelled, or
/// a transient failure that the items queued behind this one must not overtake.
async fn deliver_claimed_outbox_item<B: Backend>(ctx: &TaskCtx<B>, item: OutboxItem) -> bool {
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
            false
        }
    };
    if !claimed {
        return true;
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
            true
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
            false
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
            false
        }
        // Slack or the network may recover: back to `pending` for the next reconnect or flush
        // (which reuses the message's `client_msg_id`), and stop this pass so later items cannot
        // overtake it.
        Err(CoreError::Api(ref err)) if is_transient_send_error(err) => {
            tracing::info!(id, %err, "outbox send failed transiently; retrying on the next flush");
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
            false
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
            true
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
        let ctx = self.ctx();
        let lock = Arc::clone(&self.outbox_flush_lock);
        self.tasks.spawn(async move {
            let _guard = lock.lock().await;
            let ws = ctx.workspace.clone();
            let pending = match with_store(&ctx.store, move |s| s.list_outbox_pending(&ws)).await {
                Ok(items) => items,
                Err(err) => {
                    tracing::warn!(%err, "failed to load pending outbox items");
                    return;
                }
            };
            for item in pending {
                if !deliver_claimed_outbox_item(&ctx, item).await {
                    break;
                }
            }
        });
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
        let ctx = self.ctx();
        let lock = Arc::clone(&self.outbox_flush_lock);
        self.tasks.spawn(async move {
            let _guard = lock.lock().await;
            let _ = deliver_claimed_outbox_item(&ctx, item).await;
        });
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
        if let Err(err) = with_store(&self.store, move |s| s.delete_offline(id)).await {
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
                        if id > 0
                            && let Err(err) =
                                with_store(&ctx.store, move |s| s.delete_offline(id)).await
                        {
                            tracing::warn!(id, %err, "failed to delete flushed offline mutation");
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
        match mutation {
            QueuedMutation::SendMessage {
                channel,
                content,
                thread_ts,
                broadcast,
            } => {
                // Legacy rows only: sends now queue in the outbox, which keeps a `client_msg_id`.
                post_message_and_store(ctx, channel, content, thread_ts, broadcast, None).await?;
            }
            QueuedMutation::AddReaction { channel, ts, name } => {
                ctx.backend.add_reaction(channel, ts, name).await?;
            }
            QueuedMutation::RemoveReaction { channel, ts, name } => {
                ctx.backend.remove_reaction(channel, ts, name).await?;
            }
            QueuedMutation::MarkRead { channel, ts } => {
                // Never leak read receipts if Stealth Mode was enabled while queued.
                if stealth_mode.load(Ordering::Relaxed) {
                    tracing::debug!(%channel, "suppressing queued MarkRead under Stealth Mode");
                    return Ok(());
                }
                // A newer cursor already in the store (another device read further while this one
                // was offline) wins: replaying this older mark would make the conversation unread
                // again everywhere.
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
            }
        }
        Ok(())
    }
}
