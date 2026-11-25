//! Messaging, reactions, slash commands, block actions, read markers, threads, drafts,
//! scheduled messages, pins, bookmarks, and saved/Later command handlers.

use super::super::*;
use crate::api::{ActionContainer, ActivityEntry, MessageContent, SavedMark};

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
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        let unfurls = self.unfurls.clone();
        self.spawn("chat.postMessage", async move {
            let posted = backend
                .post_message(channel.clone(), content, thread_ts, broadcast)
                .await?;
            // Store the echo now; the websocket copy of it is an idempotent re-upsert.
            if let Some(message) = posted.message {
                unfurls.resolve(std::slice::from_ref(&message));
                let target = channel.clone();
                with_store(&store, move |s| s.upsert_message(&ws, &target, &message)).await?;
                events.emit(Event::StoreChanged { channel });
            }
            Ok(())
        });
    }

    // Edits, deletions and reactions reach the store through their websocket echo
    // (`message_changed`, `message_deleted`, `reaction_*`), which carries the edit ts.
    pub(super) fn edit_message(&self, channel: ChannelId, ts: Ts, content: MessageContent) {
        let backend = self.backend.clone();
        self.spawn("chat.update", async move {
            backend.update_message(channel, ts, content).await?;
            Ok(())
        });
    }

    pub(super) fn delete_message(&self, channel: ChannelId, ts: Ts) {
        let backend = self.backend.clone();
        self.spawn("chat.delete", async move {
            Ok(backend.delete_message(channel, ts).await?)
        });
    }

    pub(super) fn add_reaction(&self, channel: ChannelId, ts: Ts, name: String) {
        let backend = self.backend.clone();
        self.spawn("reactions.add", async move {
            Ok(backend.add_reaction(channel, ts, name).await?)
        });
    }

    pub(super) fn remove_reaction(&self, channel: ChannelId, ts: Ts, name: String) {
        let backend = self.backend.clone();
        self.spawn("reactions.remove", async move {
            Ok(backend.remove_reaction(channel, ts, name).await?)
        });
    }

    pub(super) fn run_command(
        &self,
        channel: ChannelId,
        command: String,
        args: String,
        thread_ts: Option<Ts>,
    ) {
        let backend = self.backend.clone();
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("chat.command", async move {
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
        let backend = self.backend.clone();
        let ws = self.workspace.clone();
        self.spawn("blocks.actions", async move {
            let team = team.unwrap_or(ws);
            Ok(backend
                .block_action(bot, app_id, team, action, container)
                .await?)
        });
    }

    pub(super) fn mark_read(&self, channel: ChannelId, ts: Ts) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        self.spawn("conversations.mark", async move {
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
        });
    }

    pub(super) fn mark_thread_read(&self, channel: ChannelId, thread_ts: Ts, ts: Ts) {
        let backend = self.backend.clone();
        self.spawn("subscriptions.thread.mark", async move {
            Ok(backend.mark_thread_read(channel, thread_ts, ts).await?)
        });
    }

    pub(super) fn mark_all_read(&self) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("users.markAllRead", async move {
            backend.mark_all_read().await?;
            events.emit(Event::Counts(Box::new(backend.counts().await?)));
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
        let backend = self.backend.clone();
        let backfill = self.backfill.clone();
        let method = if subscribed {
            "subscriptions.thread.add"
        } else {
            "subscriptions.thread.remove"
        };
        self.spawn(method, async move {
            backend
                .set_thread_subscribed(channel.clone(), thread_ts.clone(), last_read, subscribed)
                .await?;
            // The stored root still carries the old follow flag.
            backfill.fetch_thread(channel, thread_ts).await
        });
    }

    pub(super) fn fetch_threads(&self) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("subscriptions.thread.getView", async move {
            let view = Box::new(backend.threads_view().await?);
            events.emit(Event::ThreadsView(view));
            Ok(())
        });
    }

    pub(super) fn mark_activity(&self, entries: Vec<ActivityEntry>, read: bool) {
        let backend = self.backend.clone();
        self.spawn("activity.mark", async move {
            if read {
                backend.activity_mark_read(entries).await?
            } else {
                backend.activity_mark_unread(entries).await?
            }
            Ok(())
        });
    }

    pub(super) fn send_typing(&self, channel: ChannelId, thread_ts: Option<Ts>) {
        if let Some(Err(e)) = self.rtm.as_ref().map(|r| r.user_typing(channel, thread_ts)) {
            self.events.emit(Event::Error(format!("typing: {e}")));
        }
    }

    pub(super) fn load_pins(&self, channel: ChannelId) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("pins.list", async move {
            let pins = backend.pins(channel.clone()).await;
            answer_load(&events, pins, |pins| Event::PinsLoaded { channel, pins })
        });
    }

    pub(super) fn set_pinned(&self, channel: ChannelId, ts: Ts, pinned: bool) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        let method = if pinned { "pins.add" } else { "pins.remove" };
        self.spawn(method, async move {
            backend
                .set_pinned(channel.clone(), ts.clone(), pinned)
                .await?;
            let pins = backend.pins(channel.clone()).await;
            answer_load(&events, pins, |pins| Event::PinsLoaded { channel, pins })
        });
    }

    pub(super) fn load_bookmarks(&self, channel: ChannelId) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("bookmarks.list", async move {
            let bookmarks = backend.bookmarks(channel.clone()).await;
            answer_load(&events, bookmarks, |bookmarks| Event::BookmarksLoaded {
                channel,
                bookmarks,
            })
        });
    }

    pub(super) fn add_bookmark(&self, channel: ChannelId, title: String, url: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("bookmarks.add", async move {
            backend.add_bookmark(channel.clone(), title, url).await?;
            let bookmarks = backend.bookmarks(channel.clone()).await;
            answer_load(&events, bookmarks, |bookmarks| Event::BookmarksLoaded {
                channel,
                bookmarks,
            })
        });
    }

    pub(super) fn add_bookmark_in_folder(
        &self,
        channel: ChannelId,
        title: String,
        url: String,
        parent_id: String,
    ) {
        let backend = self.backend.clone();
        let events = self.events.clone();
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

    pub(super) fn remove_bookmark(&self, channel: ChannelId, bookmark_id: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
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

    pub(super) fn schedule_message(
        &self,
        channel: ChannelId,
        post_at: u64,
        content: MessageContent,
        thread_ts: Option<Ts>,
    ) {
        let backend = self.backend.clone();
        let events = self.events.clone();
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

    pub(super) fn load_scheduled_messages(&self) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("chat.scheduledMessages.list", async move {
            let list = backend.scheduled_messages().await;
            answer_load(&events, list, Event::ScheduledMessagesLoaded)
        });
    }

    pub(super) fn delete_scheduled_message(
        &self,
        channel: ChannelId,
        scheduled_message_id: String,
    ) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("chat.deleteScheduledMessage", async move {
            backend
                .delete_scheduled_message(channel, scheduled_message_id)
                .await?;
            let list = backend.scheduled_messages().await;
            answer_load(&events, list, Event::ScheduledMessagesLoaded)
        });
    }

    pub(super) fn fetch_cloud_drafts(&self) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        let draft_ids = Arc::clone(&self.cloud_draft_ids);
        self.spawn("drafts.list", async move {
            let drafts = backend.drafts_list().await?;
            if let Ok(mut guard) = draft_ids.lock() {
                guard.clear();
                for d in &drafts {
                    guard.insert(d.channel.clone(), d.id.clone());
                }
            }
            events.emit(Event::CloudDrafts(drafts));
            Ok(())
        });
    }

    pub(super) fn sync_draft(&self, channel: ChannelId, text: String) {
        let backend = self.backend.clone();
        let draft_ids = Arc::clone(&self.cloud_draft_ids);
        let channel_lock = {
            let mut locks = self
                .draft_locks
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            Arc::clone(
                locks
                    .entry(channel.clone())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        self.spawn("drafts.sync", async move {
            let _permit = channel_lock.lock().await;
            let existing = draft_ids.lock().ok().and_then(|g| g.get(&channel).cloned());
            if text.trim().is_empty() {
                if let Some(id) = existing
                    && backend.drafts_delete(id).await.is_ok()
                    && let Ok(mut guard) = draft_ids.lock()
                {
                    guard.remove(&channel);
                }
            } else if let Some(id) = existing {
                let _ = backend.drafts_update(id, channel, text).await;
            } else if let Ok(Some(new_id)) = backend.drafts_create(channel.clone(), text).await
                && let Ok(mut guard) = draft_ids.lock()
            {
                guard.insert(channel, new_id);
            }
            Ok(())
        });
    }

    pub(super) fn save_item(&self, channel: ChannelId, ts: Ts, date_due: Option<u64>) {
        let backend = self.backend.clone();
        let events = self.events.clone();
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

    pub(super) fn save_file(&self, file_id: String, date_due: Option<u64>) {
        let backend = self.backend.clone();
        let events = self.events.clone();
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

    pub(super) fn update_saved_item(
        &self,
        channel: ChannelId,
        ts: Ts,
        mark: Option<SavedMark>,
        date_due: Option<Option<u64>>,
    ) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("saved.update", async move {
            backend.saved_update(channel, ts, mark, date_due).await?;
            let loaded = load_saved_items(&backend, &events).await;
            answer_load(&events, loaded, Event::SavedLoaded)
        });
    }

    pub(super) fn delete_saved_item(&self, channel: ChannelId, ts: Ts) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("saved.delete", async move {
            backend.saved_delete(channel, ts).await?;
            let loaded = load_saved_items(&backend, &events).await;
            answer_load(&events, loaded, Event::SavedLoaded)
        });
    }

    pub(super) fn clear_completed_saved(&self) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("saved.clearCompleted", async move {
            backend.saved_clear_completed().await?;
            let loaded = load_saved_items(&backend, &events).await;
            answer_load(&events, loaded, Event::SavedLoaded)
        });
    }
}
