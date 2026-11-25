//! Persisted offline mutation queue and sequential FIFO flushing on reconnect.

use super::super::*;
use super::messaging::{mark_read_and_store, post_message_and_store};

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
            MessagingCommand::SendMessage {
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
            MessagingCommand::AddReaction { channel, ts, name } => Some(Self::AddReaction {
                channel: channel.clone(),
                ts: ts.clone(),
                name: name.clone(),
            }),
            MessagingCommand::RemoveReaction { channel, ts, name } => Some(Self::RemoveReaction {
                channel: channel.clone(),
                ts: ts.clone(),
                name: name.clone(),
            }),
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

impl<B: Backend> Core<B> {
    /// If the client is offline and `command` is queueable, persist it in SQLite (off the async
    /// worker thread) and return `true`.
    pub(super) async fn enqueue_or_run(&mut self, command: &Command) -> bool {
        if self.is_offline
            && let Some(mutation) = QueuedMutation::from_command(command)
        {
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
                post_message_and_store(ctx, channel, content, thread_ts, broadcast).await?;
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
                mark_read_and_store(ctx, channel, ts).await?;
            }
        }
        Ok(())
    }
}
