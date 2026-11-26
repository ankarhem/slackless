//! Messaging, reactions, slash commands, block actions, read markers, threads, drafts,
//! scheduled messages, pins, bookmarks, and saved/Later command handlers.

use super::super::*;
use super::offline::park_transient_send;
use crate::api::{
    ActionContainer, ActivityEntry, DraftHandle, MessageContent, SavedMark, new_client_msg_id,
};
use crate::outbox::{OutboxPayload, is_transient_send_error};
use crate::reducer::update_conversation_last_read;

/// Prune idle `draft_locks` entries once the per-channel lock map exceeds this threshold.
const DRAFT_LOCKS_PRUNE_THRESHOLD: usize = 64;

impl<B: Backend> Core<B> {
    pub(super) fn open_thread(&self, channel: ChannelId, thread_ts: Ts) {
        let backfill = self.backfill.clone();
        self.spawn("conversations.replies", async move {
            backfill.fetch_thread(channel, thread_ts).await
        });
    }

    pub(super) fn send_message(
        &self,
        channel: ChannelId,
        content: MessageContent,
        thread_ts: Option<Ts>,
        broadcast: bool,
    ) {
        let ctx = self.ctx();
        // Minted before the first attempt so it and any outbox retry share one id: Slack then
        // drops a retry of a post it accepted before the connection failed.
        let client_msg_id = new_client_msg_id();
        let flush = self.outbox_flush();
        self.spawn("chat.postMessage", async move {
            let outcome = post_message_and_store(
                &ctx,
                channel.clone(),
                content.clone(),
                thread_ts.clone(),
                broadcast,
                Some(client_msg_id.clone()),
            )
            .await;
            match outcome {
                Ok(()) => Ok(()),
                Err(CoreError::Api(ref err)) if is_transient_send_error(err) => {
                    let payload = if thread_ts.is_some() {
                        OutboxPayload::Reply {
                            content,
                            broadcast,
                            client_msg_id: Some(client_msg_id),
                        }
                    } else {
                        OutboxPayload::Message {
                            content,
                            client_msg_id: Some(client_msg_id),
                        }
                    };
                    park_transient_send(&flush, channel, thread_ts, payload, err).await?;
                    Ok(())
                }
                Err(err) => Err(err),
            }
        });
    }

    // Edits, deletions and reactions reach the store through their websocket echo
    // (`message_changed`, `message_deleted`, `reaction_*`), which carries the edit ts.
    pub(super) fn edit_message(&self, channel: ChannelId, ts: Ts, content: MessageContent) {
        let ctx = self.ctx();
        self.spawn("chat.update", async move {
            ctx.backend.update_message(channel, ts, content).await?;
            Ok(())
        });
    }

    pub(super) fn delete_message(&self, channel: ChannelId, ts: Ts) {
        let ctx = self.ctx();
        self.spawn("chat.delete", async move {
            Ok(ctx.backend.delete_message(channel, ts).await?)
        });
    }

    pub(super) fn add_reaction(&self, channel: ChannelId, ts: Ts, name: String) {
        let ctx = self.ctx();
        let flush = self.outbox_flush();
        self.spawn("reactions.add", async move {
            match ctx
                .backend
                .add_reaction(channel.clone(), ts.clone(), name.clone())
                .await
            {
                Ok(()) => Ok(()),
                Err(ref err) if is_transient_send_error(err) => {
                    park_transient_send(
                        &flush,
                        channel,
                        None,
                        OutboxPayload::ReactionAdd { ts, name },
                        err,
                    )
                    .await?;
                    Ok(())
                }
                Err(err) => Err(err.into()),
            }
        });
    }

    pub(super) fn remove_reaction(&self, channel: ChannelId, ts: Ts, name: String) {
        let ctx = self.ctx();
        let flush = self.outbox_flush();
        self.spawn("reactions.remove", async move {
            match ctx
                .backend
                .remove_reaction(channel.clone(), ts.clone(), name.clone())
                .await
            {
                Ok(()) => Ok(()),
                Err(ref err) if is_transient_send_error(err) => {
                    park_transient_send(
                        &flush,
                        channel,
                        None,
                        OutboxPayload::ReactionRemove { ts, name },
                        err,
                    )
                    .await?;
                    Ok(())
                }
                Err(err) => Err(err.into()),
            }
        });
    }

    pub(super) fn run_command(
        &self,
        channel: ChannelId,
        command: String,
        args: String,
        thread_ts: Option<Ts>,
        app_id: Option<String>,
    ) {
        /// Maximum time to wait for Slack's `chat.command` dispatch before surfacing a clear
        /// timeout failure (`F-COLLAB-685`).
        const SLASH_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

        let ctx = self.ctx();
        self.spawn("chat.command", async move {
            let fut = ctx.backend.run_command_for_app(
                channel.clone(),
                command.clone(),
                args,
                thread_ts.clone(),
                ctx.workspace,
                app_id,
            );
            let ran = match tokio::time::timeout(SLASH_COMMAND_TIMEOUT, fut).await {
                Ok(res) => res,
                Err(_) => {
                    ctx.events.emit(Event::CommandFailed {
                        command,
                        error: "command_timeout".to_owned(),
                    });
                    return Ok(());
                }
            };
            match ran {
                Ok(resp) => {
                    if let Some(text) = resp
                        .text
                        .map(|t| t.trim().to_owned())
                        .filter(|t| !t.is_empty())
                    {
                        ctx.events.emit(Event::CommandResponse {
                            channel,
                            command,
                            response_type: resp.response_type,
                            text,
                            thread_ts,
                        });
                    }
                    Ok(())
                }
                Err(ApiError::Slack(error)) => {
                    ctx.events.emit(Event::CommandFailed { command, error });
                    Ok(())
                }
                Err(ApiError::Http(err)) if err.is_timeout() => {
                    ctx.events.emit(Event::CommandFailed {
                        command,
                        error: "command_timeout".to_owned(),
                    });
                    Ok(())
                }
                Err(ApiError::RateLimited { .. }) => {
                    ctx.events.emit(Event::CommandFailed {
                        command,
                        error: "rate_limited".to_owned(),
                    });
                    Ok(())
                }
                Err(err) if err.is_auth_expired() => Err(err.into()),
                Err(err) => {
                    ctx.events.emit(Event::CommandFailed {
                        command,
                        error: err.to_string(),
                    });
                    Ok(())
                }
            }
        });
    }

    pub(super) fn block_action(
        &self,
        bot: BotId,
        app_id: String,
        team: Option<TeamId>,
        action: serde_json::Value,
        container: ActionContainer,
    ) {
        let ctx = self.ctx();
        self.spawn("blocks.actions", async move {
            let team = team.unwrap_or(ctx.workspace);
            Ok(ctx
                .backend
                .block_action(bot, app_id, team, action, container)
                .await?)
        });
    }

    pub(super) fn mark_read(&self, channel: ChannelId, ts: Ts) {
        if self.stealth_mode.load(Ordering::Relaxed) {
            return;
        }
        let ctx = self.ctx();
        self.spawn("conversations.mark", async move {
            mark_read_and_store(&ctx, channel, ts).await
        });
    }

    pub(super) fn mark_thread_read(&self, channel: ChannelId, thread_ts: Ts, ts: Ts) {
        if self.stealth_mode.load(Ordering::Relaxed) {
            return;
        }
        let ctx = self.ctx();
        self.spawn("subscriptions.thread.mark", async move {
            Ok(ctx.backend.mark_thread_read(channel, thread_ts, ts).await?)
        });
    }

    pub(super) fn mark_all_read(&self) {
        if self.stealth_mode.load(Ordering::Relaxed) {
            return;
        }
        let ctx = self.ctx();
        self.spawn("users.markAllRead", async move {
            ctx.backend.mark_all_read().await?;
            ctx.events
                .emit(Event::Counts(Box::new(ctx.backend.counts().await?)));
            Ok(())
        });
    }

    pub(super) fn set_thread_subscribed(
        &self,
        channel: ChannelId,
        thread_ts: Ts,
        last_read: Ts,
        subscribed: bool,
    ) {
        let ctx = self.ctx();
        let backfill = self.backfill.clone();
        let method = if subscribed {
            "subscriptions.thread.add"
        } else {
            "subscriptions.thread.remove"
        };
        self.spawn(method, async move {
            ctx.backend
                .set_thread_subscribed(channel.clone(), thread_ts.clone(), last_read, subscribed)
                .await?;
            // The stored root still carries the old follow flag.
            backfill.fetch_thread(channel, thread_ts).await
        });
    }

    pub(super) fn fetch_threads(&self) {
        let ctx = self.ctx();
        self.spawn("subscriptions.thread.getView", async move {
            let view = Box::new(ctx.backend.threads_view().await?);
            ctx.events.emit(Event::ThreadsView(view));
            Ok(())
        });
    }

    pub(super) fn mark_activity(&self, entries: Vec<ActivityEntry>, read: bool) {
        let ctx = self.ctx();
        self.spawn("activity.mark", async move {
            if read {
                ctx.backend.activity_mark_read(entries).await?
            } else {
                ctx.backend.activity_mark_unread(entries).await?
            }
            Ok(())
        });
    }

    pub(super) fn send_typing(&self, channel: ChannelId, thread_ts: Option<Ts>) {
        if self.stealth_mode.load(Ordering::Relaxed) {
            return;
        }
        if let Some(Err(e)) = self.rtm.as_ref().map(|r| r.user_typing(channel, thread_ts)) {
            self.events.emit(Event::Error(format!("typing: {e}")));
        }
    }

    pub(super) fn load_pins(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("pins.list", async move {
            let pins = ctx.backend.pins(channel.clone()).await;
            answer_load(&ctx.events, pins, |pins| Event::PinsLoaded {
                channel,
                pins,
            })
        });
    }

    pub(super) fn set_pinned(&self, channel: ChannelId, ts: Ts, pinned: bool) {
        let ctx = self.ctx();
        let method = if pinned { "pins.add" } else { "pins.remove" };
        self.spawn(method, async move {
            ctx.backend
                .set_pinned(channel.clone(), ts.clone(), pinned)
                .await?;
            let pins = ctx.backend.pins(channel.clone()).await;
            answer_load(&ctx.events, pins, |pins| Event::PinsLoaded {
                channel,
                pins,
            })
        });
    }

    pub(super) fn load_bookmarks(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("bookmarks.list", async move {
            reload_bookmarks(&ctx.backend, &ctx.events, channel).await
        });
    }

    pub(super) fn add_bookmark(&self, channel: ChannelId, title: String, url: String) {
        let ctx = self.ctx();
        self.spawn("bookmarks.add", async move {
            ctx.backend
                .add_bookmark(channel.clone(), title, url)
                .await?;
            reload_bookmarks(&ctx.backend, &ctx.events, channel).await
        });
    }

    pub(super) fn add_bookmark_in_folder(
        &self,
        channel: ChannelId,
        title: String,
        url: String,
        parent_id: String,
    ) {
        let ctx = self.ctx();
        self.spawn("bookmarks.add", async move {
            ctx.backend
                .add_bookmark_in_folder(channel.clone(), title, url, parent_id)
                .await?;
            reload_bookmarks(&ctx.backend, &ctx.events, channel).await
        });
    }

    pub(super) fn remove_bookmark(&self, channel: ChannelId, bookmark_id: String) {
        let ctx = self.ctx();
        self.spawn("bookmarks.remove", async move {
            ctx.backend
                .remove_bookmark(channel.clone(), bookmark_id)
                .await?;
            reload_bookmarks(&ctx.backend, &ctx.events, channel).await
        });
    }

    pub(super) fn schedule_message(
        &self,
        channel: ChannelId,
        post_at: u64,
        content: MessageContent,
        thread_ts: Option<Ts>,
    ) {
        let ctx = self.ctx();
        self.spawn("chat.scheduleMessage", async move {
            let created = ctx
                .backend
                .schedule_message(channel.clone(), post_at, content, thread_ts)
                .await?;
            ctx.events.emit(Event::MessageScheduled {
                channel: created.channel,
                scheduled_message_id: created.scheduled_message_id,
                post_at: created.post_at,
            });
            reload_scheduled_messages(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn load_scheduled_messages(&self) {
        let ctx = self.ctx();
        self.spawn("chat.scheduledMessages.list", async move {
            reload_scheduled_messages(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn delete_scheduled_message(
        &self,
        channel: ChannelId,
        scheduled_message_id: String,
    ) {
        let ctx = self.ctx();
        self.spawn("chat.deleteScheduledMessage", async move {
            ctx.backend
                .delete_scheduled_message(channel, scheduled_message_id)
                .await?;
            reload_scheduled_messages(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn fetch_cloud_drafts(&self) {
        let ctx = self.ctx();
        let cloud_drafts = Arc::clone(&self.cloud_drafts);
        self.spawn("drafts.list", async move {
            let drafts = ctx.backend.drafts_list().await?;
            {
                let mut guard = cloud_drafts.lock().unwrap_or_else(PoisonError::into_inner);
                guard.clear();
                for d in &drafts {
                    guard.insert(d.channel.clone(), d.handle());
                }
            }
            ctx.events.emit(Event::CloudDrafts(drafts));
            Ok(())
        });
    }

    pub(super) fn sync_draft(&self, channel: ChannelId, text: String) {
        let ctx = self.ctx();
        let cloud_drafts = Arc::clone(&self.cloud_drafts);
        let draft_locks = Arc::clone(&self.draft_locks);
        let channel_lock = {
            let mut locks = self
                .draft_locks
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if locks.len() > DRAFT_LOCKS_PRUNE_THRESHOLD {
                locks.retain(|k, v| k == &channel || Arc::strong_count(v) > 1);
            }
            Arc::clone(
                locks
                    .entry(channel.clone())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        self.spawn("drafts.sync", async move {
            let _permit = channel_lock.lock().await;
            let drafts = || cloud_drafts.lock().unwrap_or_else(PoisonError::into_inner);
            // After a failed write the token may be stale (another device wrote the draft): the
            // next write looks the current one up instead.
            let forget_token = |channel: &ChannelId| {
                if let Some(draft) = drafts().get_mut(channel) {
                    draft.last_updated_ts = None;
                }
            };
            let existing: Option<DraftHandle> = drafts().get(&channel).cloned();
            let result = if text.trim().is_empty() {
                if let Some(draft) = existing {
                    match ctx.backend.drafts_delete(draft).await {
                        Ok(()) => {
                            drafts().remove(&channel);
                            Ok(())
                        }
                        Err(e) if e.is_auth_expired() => Err(e),
                        Err(e) => {
                            tracing::debug!(%channel, %e, "drafts.delete failed");
                            forget_token(&channel);
                            Ok(())
                        }
                    }
                } else {
                    Ok(())
                }
            } else if let Some(draft) = existing {
                match ctx
                    .backend
                    .drafts_update(draft, channel.clone(), text)
                    .await
                {
                    Ok(updated) => {
                        drafts().insert(channel.clone(), updated);
                        Ok(())
                    }
                    Err(e) if e.is_auth_expired() => Err(e),
                    Err(e) => {
                        tracing::debug!(%channel, %e, "drafts.update failed");
                        forget_token(&channel);
                        Ok(())
                    }
                }
            } else {
                match ctx.backend.drafts_create(channel.clone(), text).await {
                    Ok(Some(created)) => {
                        drafts().insert(channel.clone(), created);
                        Ok(())
                    }
                    Ok(None) => Ok(()),
                    Err(e) if e.is_auth_expired() => Err(e),
                    Err(e) => {
                        tracing::debug!(%channel, %e, "drafts.create failed");
                        Ok(())
                    }
                }
            };
            drop(_permit);
            // Prune idle channel locks when map grows large.
            {
                let mut locks = draft_locks.lock().unwrap_or_else(PoisonError::into_inner);
                if locks.len() > DRAFT_LOCKS_PRUNE_THRESHOLD {
                    locks.retain(|_, v| Arc::strong_count(v) > 1);
                }
            }
            Ok(result?)
        });
    }

    pub(super) fn save_item(&self, channel: ChannelId, ts: Ts, date_due: Option<u64>) {
        let ctx = self.ctx();
        self.spawn("saved.add", async move {
            match ctx
                .backend
                .saved_add(channel.clone(), ts.clone(), date_due)
                .await
            {
                Ok(()) => {}
                Err(ApiError::Slack(ref error))
                    if error == "already_saved" || error == "already_exists" =>
                {
                    if let Some(due) = date_due {
                        ctx.backend
                            .saved_update(channel, ts, None, Some(Some(due)))
                            .await?;
                    }
                }
                Err(e) => return Err(e.into()),
            }
            reload_saved(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn save_file(&self, file_id: String, date_due: Option<u64>) {
        let ctx = self.ctx();
        self.spawn("saved.add(file)", async move {
            match ctx.backend.save_file(file_id, date_due).await {
                Ok(()) => {}
                Err(ApiError::Slack(ref error))
                    if error == "already_saved" || error == "already_exists" => {}
                Err(e) => return Err(e.into()),
            }
            reload_saved(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn update_saved_item(
        &self,
        channel: ChannelId,
        ts: Ts,
        mark: Option<SavedMark>,
        date_due: Option<Option<u64>>,
    ) {
        let ctx = self.ctx();
        self.spawn("saved.update", async move {
            ctx.backend
                .saved_update(channel, ts, mark, date_due)
                .await?;
            reload_saved(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn delete_saved_item(&self, channel: ChannelId, ts: Ts) {
        let ctx = self.ctx();
        self.spawn("saved.delete", async move {
            ctx.backend.saved_delete(channel, ts).await?;
            reload_saved(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn clear_completed_saved(&self) {
        let ctx = self.ctx();
        self.spawn("saved.clearCompleted", async move {
            ctx.backend.saved_clear_completed().await?;
            reload_saved(&ctx.backend, &ctx.events).await
        });
    }
}

/// Post a message to Slack, store the echoed message locally, resolve unfurls, and emit
/// [`Event::StoreChanged`]. Shared by online [`Core::send_message`] and offline queue flushing;
/// a retry passes the `client_msg_id` of the message's first attempt (`None` mints one).
pub(super) async fn post_message_and_store<B: Backend>(
    ctx: &TaskCtx<B>,
    channel: ChannelId,
    content: MessageContent,
    thread_ts: Option<Ts>,
    broadcast: bool,
    client_msg_id: Option<String>,
) -> Result<(), CoreError> {
    let posted = ctx
        .backend
        .post_message(
            channel.clone(),
            content,
            thread_ts,
            broadcast,
            client_msg_id,
        )
        .await?;
    if let Some(message) = posted.message {
        ctx.unfurls.resolve(std::slice::from_ref(&message));
        let target = channel.clone();
        let ws = ctx.workspace.clone();
        with_store(&ctx.store, move |s| {
            s.upsert_message(&ws, &target, &message)
        })
        .await?;
        ctx.events.emit(Event::StoreChanged { channel });
    }
    Ok(())
}

/// Mark a conversation read on Slack and mirror `last_read` into the local store. Shared by
/// online [`Core::mark_read`] and offline queue flushing.
pub(super) async fn mark_read_and_store<B: Backend>(
    ctx: &TaskCtx<B>,
    channel: ChannelId,
    ts: Ts,
) -> Result<(), CoreError> {
    ctx.backend.mark_read(channel.clone(), ts.clone()).await?;
    let ws = ctx.workspace.clone();
    if let Err(err) = with_store(&ctx.store, move |s| {
        update_conversation_last_read(s, &ws, &channel, ts, None)
    })
    .await
    {
        tracing::warn!(%err, "failed to persist last_read after conversations.mark");
    }
    Ok(())
}

async fn reload_bookmarks<B: Backend>(
    backend: &B,
    events: &Emitter,
    channel: ChannelId,
) -> Result<(), CoreError> {
    let bookmarks = backend.bookmarks(channel.clone()).await;
    answer_load(events, bookmarks, |bookmarks| Event::BookmarksLoaded {
        channel,
        bookmarks,
    })
}

async fn reload_scheduled_messages<B: Backend>(
    backend: &B,
    events: &Emitter,
) -> Result<(), CoreError> {
    let list = backend.scheduled_messages().await;
    answer_load(events, list, Event::ScheduledMessagesLoaded)
}

async fn reload_saved<B: Backend>(backend: &B, events: &Emitter) -> Result<(), CoreError> {
    let loaded = load_saved_items(backend, events).await;
    answer_load(events, loaded, Event::SavedLoaded)
}
