//! Persisted offline mutation queue and sequential FIFO flushing on reconnect.

use super::super::*;

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
    /// If the client is offline and `command` is queueable, persist it in SQLite and return `true`.
    pub(super) fn enqueue_or_run(&mut self, command: &Command) -> bool {
        if self.is_offline
            && let Some(mutation) = QueuedMutation::from_command(command)
        {
            let payload = serde_json::to_string(&mutation).unwrap_or_default();
            let id = self
                .store
                .lock()
                .ok()
                .and_then(|mut s| s.enqueue_offline(&self.workspace, &payload).ok())
                .unwrap_or(0);
            self.offline_queue.push_back((id, mutation));
            self.events
                .emit(Event::OfflineQueueChanged(self.offline_queue.len()));
            return true;
        }
        false
    }

    pub(super) fn retry_offline_mutation(&mut self, id: i64) {
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

    pub(super) fn discard_offline_mutation(&mut self, id: i64) {
        self.offline_queue.retain(|(row_id, _)| *row_id != id);
        if let Ok(mut s) = self.store.lock() {
            let _ = s.delete_offline(id);
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
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        let unfurls = self.unfurls.clone();
        tokio::spawn(async move {
            for (id, mutation) in queued {
                let summary = mutation.summary();
                let outcome = Self::execute_queued_mutation(
                    &backend, &store, &ws, &events, &unfurls, mutation,
                )
                .await;
                match outcome {
                    Ok(()) => {
                        if id > 0 {
                            let _ = with_store(&store, move |s| s.delete_offline(id)).await;
                        }
                    }
                    Err(CoreError::Api(ApiError::AuthExpired(code))) => {
                        events.emit(Event::AuthExpired(code));
                        break;
                    }
                    Err(err) => {
                        events.emit(Event::OfflineMutationFailed(FailedMutation {
                            id,
                            summary,
                            error: err.to_string(),
                        }));
                    }
                }
            }
        });
    }

    /// Execute one persisted offline mutation; deletes the SQLite row on success or emits
    /// [`Event::OfflineMutationFailed`] on failure so the user can retry or discard it.
    pub(in crate::client) fn flush_queued_mutation(&self, id: i64, mutation: QueuedMutation) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        let unfurls = self.unfurls.clone();
        let summary = mutation.summary();
        tokio::spawn(async move {
            let outcome =
                Self::execute_queued_mutation(&backend, &store, &ws, &events, &unfurls, mutation)
                    .await;
            match outcome {
                Ok(()) => {
                    if id > 0 {
                        let _ = with_store(&store, move |s| s.delete_offline(id)).await;
                    }
                }
                Err(CoreError::Api(ApiError::AuthExpired(code))) => {
                    events.emit(Event::AuthExpired(code));
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

    pub(in crate::client) async fn execute_queued_mutation(
        backend: &B,
        store: &SharedStore,
        ws: &TeamId,
        events: &Emitter,
        unfurls: &Unfurls<B>,
        mutation: QueuedMutation,
    ) -> Result<(), CoreError> {
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
                    let ws = ws.clone();
                    with_store(store, move |s| s.upsert_message(&ws, &target, &message)).await?;
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
                backend.mark_read(channel.clone(), ts.clone()).await?;
                let ws = ws.clone();
                let _ = with_store(store, move |s| {
                    if let Some(mut conv) = s.conversation(&ws, &channel)? {
                        conv.last_read = Some(ts);
                        s.upsert_conversation(&ws, &conv)?;
                    }
                    Ok::<_, StoreError>(())
                })
                .await;
            }
        }
        Ok(())
    }
}
