//! Outgoing messages, drafts, scheduling, slash commands, emoji/GIFs, uploads, and read state.

use super::*;

impl App {
    /// Delete the most recent message you sent in the open conversation (`Cmd+Shift+Backspace`,
    /// `F-CORE-093`), respecting `Settings.confirm_unsend`.
    pub(super) fn unsend_last_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ts) = self.messages.read(cx).latest_own_ts(cx) else {
            return;
        };
        let confirm = self.preferences.read(cx).settings().confirm_unsend;
        if confirm {
            self.messages.update(cx, |list, cx| {
                list.run_action(&ts, MessageAction::Delete, window, cx);
            });
        } else {
            // `list.delete` emits `MessageListEvent::Changed(MessageChange::Delete)`, which
            // routes through `post_change` to send `MessagingCommand::DeleteMessage` once.
            self.messages.update(cx, |list, cx| {
                list.delete(&ts, cx);
            });
            actions::notify("Message unsent", "Deleted your last message.", window, cx);
        }
    }

    /// The composer prompts with the open conversation's name, as Slack does.
    pub(super) fn sync_composer_placeholder(&mut self, cx: &mut Context<Self>) {
        // Slack's note-to-self DM prompts "Jot something down" instead of "Message <you>";
        // a private channel's prompt carries a lock instead of "#".
        let placeholder = match self.fixtures.conversation(&self.channel) {
            Some(c)
                if c.kind() == ConversationKind::Im
                    && c.user.as_ref() == Some(&self.fixtures.self_user) =>
            {
                Placeholder::Text("Jot something down".into())
            }
            Some(c) if c.kind() == ConversationKind::PrivateChannel => {
                Placeholder::Private(c.name.as_deref().unwrap_or(c.id.as_str()).to_owned().into())
            }
            _ => Placeholder::Text(
                format!(
                    "Message {}",
                    self.label(&self.channel).replacen("# ", "#", 1)
                )
                .into(),
            ),
        };
        self.composer
            .update(cx, |composer, cx| composer.set_placeholder(placeholder, cx));
    }

    /// Hand the current status / presence / pause / focus mode to the views that show it.
    pub(super) fn push_self_state(&mut self, cx: &mut Context<Self>) {
        let state = self.self_state.clone();
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_self_state(state.clone(), cx));
        let schedule = self.preferences.read(cx).settings().schedule_summary();
        self.user_menu
            .update(cx, |menu, cx| menu.set_state(state.clone(), schedule, cx));
        self.profile
            .update(cx, |profile, cx| profile.set_self_state(state, cx));
        cx.notify();
    }

    /// A one-key shortcut on the focused message, routed through the transcript's shared
    /// [`MessageAction`] handler. Inert while an overlay owns the screen (a picker, the
    /// reminder menu) or while the focus has left the root (a field, a dialog, an open
    /// menu) — on top of GPUI already withholding character keys from focused fields.
    pub(super) fn focus_message(
        &mut self,
        action: MessageAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.image_viewer.is_some()
            || self.picker.is_some()
            || self.remind_menu.is_some()
            || !window
                .focused(cx)
                .is_none_or(|focused| focused == self.focus)
        {
            return;
        }
        self.messages
            .update(cx, |list, cx| list.focus_action(action, window, cx));
    }

    /// Post a reaction, edit or deletion a view already shows to Slack.
    pub(super) fn post_change(&self, channel: &ChannelId, change: &MessageChange) {
        let channel = channel.clone();
        let cmd: Command = match change {
            MessageChange::React { ts, name, add } => {
                let (ts, name) = (ts.clone(), name.clone());
                if *add {
                    MessagingCommand::AddReaction { channel, ts, name }.into()
                } else {
                    MessagingCommand::RemoveReaction { channel, ts, name }.into()
                }
            }
            MessageChange::Edit { ts, content } => MessagingCommand::EditMessage {
                channel,
                ts: ts.clone(),
                content: content.clone(),
            }
            .into(),
            MessageChange::Delete { ts } => MessagingCommand::DeleteMessage {
                channel,
                ts: ts.clone(),
            }
            .into(),
            MessageChange::Pin { ts, pinned } => MessagingCommand::SetPinned {
                channel,
                ts: ts.clone(),
                pinned: *pinned,
            }
            .into(),
            MessageChange::MarkUnread { ts } => MessagingCommand::MarkRead {
                channel,
                ts: ts.clone(),
            }
            .into(),
            MessageChange::BlockAction(dispatch) => CollabCommand::BlockAction {
                bot: dispatch.bot.clone(),
                app_id: dispatch.app_id.clone(),
                team: dispatch.team.clone(),
                action: dispatch.action.clone(),
                container: dispatch.container.clone(),
            }
            .into(),
        };
        self.bridge.send(cmd);
    }

    /// Route a pick: composers get the emoji, messages a reaction.
    pub(super) fn pick_emoji(
        &mut self,
        choice: &EmojiChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((target, _)) = self.picker.take() else {
            return;
        };
        match target {
            PickerTarget::Composer => {
                self.composer
                    .update(cx, |composer, cx| composer.insert_emoji(choice, window, cx));
            }
            PickerTarget::ThreadComposer => {
                self.thread
                    .update(cx, |thread, cx| thread.insert_emoji(choice, window, cx));
            }
            PickerTarget::ThreadsComposer => {
                self.threads
                    .update(cx, |view, cx| view.insert_emoji(choice, window, cx));
            }
            PickerTarget::ComposeView => {
                self.compose
                    .update(cx, |view, cx| view.insert_emoji(choice, window, cx));
            }
            PickerTarget::MessageEdit => {
                self.messages
                    .update(cx, |list, cx| list.insert_edit_emoji(choice, window, cx));
            }
            PickerTarget::ThreadEdit => {
                self.thread.update(cx, |thread, cx| {
                    thread.insert_edit_emoji(choice, window, cx)
                });
            }
            PickerTarget::Message { channel, ts } => {
                if self.page == MainPage::Threads {
                    if self
                        .threads
                        .update(cx, |t, cx| t.add_reaction(&channel, &ts, &choice.name, cx))
                    {
                        self.post_change(
                            &channel,
                            &MessageChange::React {
                                ts: ts.clone(),
                                name: choice.name.clone(),
                                add: true,
                            },
                        );
                    }
                } else if self.page == MainPage::Unreads {
                    let added = self.unreads.update(cx, |u, cx| {
                        u.toggle_reaction(&channel, &ts, &choice.name, cx)
                    });
                    if let Some(add) = added {
                        self.post_change(
                            &channel,
                            &MessageChange::React {
                                ts: ts.clone(),
                                name: choice.name.clone(),
                                add,
                            },
                        );
                    }
                } else {
                    self.messages.update(cx, |list, cx| {
                        list.add_reaction(&channel, &ts, &choice.name, cx)
                    });
                }
                self.focus.focus(window, cx);
            }
            PickerTarget::ThreadMessage(ts) => {
                self.thread
                    .update(cx, |thread, cx| thread.add_reaction(&ts, &choice.name, cx));
                self.focus.focus(window, cx);
            }
        }
        let frequent = self.emoji_picker.read(cx).frequent().to_vec();
        if let Some(write) = self.bridge.remember(&FREQUENT_EMOJI, frequent.as_slice()) {
            cx.background_executor().spawn(write).detach();
        }
        if !self.fixtures.sample && !frequent.is_empty() {
            let mut map = crate::blockkit::serde_json::Map::new();
            let total = frequent.len();
            for (ix, name) in frequent.iter().take(24).enumerate() {
                map.insert(
                    name.clone(),
                    crate::blockkit::serde_json::json!((total - ix) as u64),
                );
            }
            if let Ok(encoded) = crate::blockkit::serde_json::to_string(&map) {
                self.bridge.send(WorkspaceCommand::SetUserPref {
                    name: "emoji_use".to_owned(),
                    value: encoded,
                });
            }
        }
        cx.notify();
    }

    /// Route a GIF chosen from the GIFs tab: stage it inside the active composer (matching
    /// official Slack, where clicking a GIF attaches it to the draft rather than sending).
    pub(super) fn pick_gif(
        &mut self,
        gif: &slackless_core::api::GifItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((target, _)) = self.picker.take() else {
            return;
        };
        self.emoji_picker
            .update(cx, |picker, cx| picker.release_gifs(window, cx));
        match target {
            PickerTarget::Composer => {
                self.composer.update(cx, |composer, cx| {
                    composer.stage_gif(gif.clone(), window, cx)
                });
            }
            PickerTarget::ThreadComposer => {
                self.thread
                    .update(cx, |thread, cx| thread.stage_gif(gif.clone(), window, cx));
            }
            PickerTarget::ThreadsComposer => {
                self.threads
                    .update(cx, |view, cx| view.stage_gif(gif.clone(), window, cx));
            }
            PickerTarget::ComposeView => {
                self.compose
                    .update(cx, |view, cx| view.stage_gif(gif.clone(), window, cx));
            }
            _ => {}
        }
        cx.notify();
    }

    /// Route a Giphy query from the GIFs tab through Slack's `/giphy` command.
    pub(super) fn run_giphy(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, _)) = self.picker.take() else {
            return;
        };
        self.emoji_picker
            .update(cx, |picker, cx| picker.release_gifs(window, cx));
        let trimmed = query.trim();
        if trimmed.is_empty() {
            cx.notify();
            return;
        }
        let Some(invocation) = crate::slash::parse(&format!("/giphy {trimmed}")) else {
            cx.notify();
            return;
        };
        let thread = match target {
            PickerTarget::ThreadComposer => self.thread.read(cx).open_thread(),
            PickerTarget::ThreadsComposer => self.threads.read(cx).active_thread(),
            _ => None,
        };
        self.run_slash(&invocation, thread, window, cx);
        cx.notify();
    }

    /// Like Slack, viewing a conversation with unreads marks it read up to its newest message:
    /// Slack is told (`conversations.mark`), the sidebar un-bolds it at once, and `self.fixtures`
    /// advances `last_read` so re-opening the conversation (or opening All Unreads) does not keep
    /// showing it as new/unread.
    pub(super) fn mark_read(&mut self, channel: &ChannelId, newest: Ts, cx: &mut Context<Self>) {
        let mut target_ts = newest;
        if !self.fixtures.sample {
            if let Some(counts) = self.ws.counts.get_mut(channel) {
                // If local SQLite history lags behind `client.counts.latest`, mark up to `latest`
                // so a cached pre-fetch read never sends a stale cursor and leaves the DM unread.
                if let Some(latest) = counts.latest.as_ref()
                    && *latest > target_ts
                {
                    target_ts = latest.clone();
                }
                let advances_cursor = counts
                    .last_read
                    .as_ref()
                    .is_none_or(|read| target_ts > *read);
                if !counts.is_unread() && !advances_cursor {
                    return;
                }
                counts.clear_unreads();
                counts.last_read = Some(target_ts.clone());
                if counts.latest.as_ref().is_none_or(|l| target_ts > *l) {
                    counts.latest = Some(target_ts.clone());
                }
            } else {
                self.ws.counts.insert(
                    channel.clone(),
                    ConversationCounts {
                        id: channel.clone(),
                        last_read: Some(target_ts.clone()),
                        latest: Some(target_ts.clone()),
                        ..Default::default()
                    },
                );
            }
            let record = self
                .ws
                .marked_read
                .get(channel)
                .is_none_or(|prev| target_ts > *prev);
            if record {
                self.ws
                    .marked_read
                    .insert(channel.clone(), target_ts.clone());
            }
        }
        self.ws.explicit_unread.remove(channel);
        self.bridge.send(MessagingCommand::MarkRead {
            channel: channel.clone(),
            ts: target_ts,
        });
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.mark_read(channel.clone(), cx));
        self.sync_counts_to_views(cx);
    }

    /// Mark the open conversation read if the transcript is visible and the user's preferences
    /// allow auto-marking read (`mark_read != 2`, and window focused when `no_bg_mark_read` is on).
    pub(super) fn maybe_mark_open_read(&mut self, cx: &mut Context<Self>) {
        if !self.transcript_visible(cx) || self.ws.explicit_unread.contains(&self.channel) {
            return;
        }
        let (mark_read, no_bg_mark_read) = {
            let settings = self.preferences.read(cx).settings();
            (settings.mark_read, settings.no_bg_mark_read)
        };
        let allow_mark = mark_read != 2 && (!no_bg_mark_read || self.window_active);
        if !allow_mark {
            return;
        }
        let channel = self.channel.clone();
        let newest = self
            .messages
            .read(cx)
            .newest_ts()
            .into_iter()
            .chain(self.ws.counts.get(&channel).and_then(|c| c.latest.clone()))
            .max();
        if let Some(newest) = newest {
            self.mark_read(&channel, newest, cx);
        }
    }

    /// Mirror a user-initiated "Mark as Unread" (`conversations.mark` backward to `ts`) into local
    /// counts, `fixtures`, and sidebar state so the conversation immediately shows unread again.
    pub(super) fn apply_local_mark_unread(
        &mut self,
        channel: &ChannelId,
        ts: Ts,
        cx: &mut Context<Self>,
    ) {
        self.ws.marked_read.remove(channel);
        self.ws.explicit_unread.insert(channel.clone());
        if !self.fixtures.sample {
            let entry =
                self.ws
                    .counts
                    .entry(channel.clone())
                    .or_insert_with(|| ConversationCounts {
                        id: channel.clone(),
                        ..Default::default()
                    });
            entry.last_read = Some(ts);
            entry.has_unreads = true;
            if self
                .fixtures
                .conversation(channel)
                .is_some_and(|c| matches!(c.kind(), ConversationKind::Im | ConversationKind::Mpim))
                && entry.effective_mentions() == 0
            {
                entry.mention_count = 1;
            }
        }
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.mark_unread(channel, cx));
        self.sync_counts_to_views(cx);
    }

    /// Advance a single thread's read cursor to `ts` (`subscriptions.thread.mark`), clear its
    /// unread badge in `ThreadsView`, and clear the sidebar's Threads indicator when caught up.
    pub(super) fn mark_thread_read(
        &mut self,
        channel: ChannelId,
        thread_ts: Ts,
        ts: Ts,
        clear_divider: bool,
        cx: &mut Context<Self>,
    ) {
        let key = (channel.clone(), thread_ts.clone());
        let already_marked = self
            .ws
            .marked_threads
            .get(&key)
            .is_some_and(|prev| *prev >= ts);
        if !already_marked {
            self.ws.marked_threads.insert(key, ts.clone());
            self.bridge.send(MessagingCommand::MarkThreadRead {
                channel: channel.clone(),
                thread_ts: thread_ts.clone(),
                ts: ts.clone(),
            });
        }
        self.threads.update(cx, |threads, cx| {
            threads.mark_thread_read(&channel, &thread_ts, Some(&ts), clear_divider, cx);
        });
        self.sync_thread_sidebar_counts(cx);
    }

    /// Move a thread's read cursor backward to `cursor` (marking messages after it unread).
    pub(super) fn mark_thread_unread(
        &mut self,
        channel: ChannelId,
        thread_ts: Ts,
        cursor: Ts,
        cx: &mut Context<Self>,
    ) {
        self.ws
            .marked_threads
            .insert((channel.clone(), thread_ts.clone()), cursor.clone());
        self.bridge.send(MessagingCommand::MarkThreadRead {
            channel: channel.clone(),
            thread_ts: thread_ts.clone(),
            ts: cursor.clone(),
        });
        self.threads.update(cx, |threads, cx| {
            threads.mark_thread_read(&channel, &thread_ts, Some(&cursor), false, cx);
        });
        let mentions = self.sidebar.read(cx).thread_mentions();
        self.sidebar
            .update(cx, |s, cx| s.set_thread_counts(true, mentions, cx));
        self.sync_badge(cx);
    }

    /// Auto-mark every currently unread card in `ThreadsView` read on Slack when the view opens,
    /// keeping the in-card `"New"` divider for the current visit while clearing badges immediately.
    pub(super) fn auto_mark_threads_view_read(&mut self, cx: &mut Context<Self>) {
        let targets = self
            .threads
            .update(cx, |threads, cx| threads.clear_all_unreads(false, cx));
        for (channel, thread_ts, ts) in targets {
            let key = (channel.clone(), thread_ts.clone());
            if !self
                .ws
                .marked_threads
                .get(&key)
                .is_some_and(|prev| *prev >= ts)
            {
                self.ws.marked_threads.insert(key, ts.clone());
                self.bridge.send(MessagingCommand::MarkThreadRead {
                    channel,
                    thread_ts,
                    ts,
                });
            }
        }
        self.sidebar
            .update(cx, |s, cx| s.set_thread_counts(false, 0, cx));
        self.sync_badge(cx);
    }

    /// Explicitly mark all unread threads read (`Esc` in Threads view or "Mark all read" button).
    pub(super) fn mark_all_threads_read(&mut self, cx: &mut Context<Self>) {
        let targets = self
            .threads
            .update(cx, |threads, cx| threads.clear_all_unreads(true, cx));
        for (channel, thread_ts, ts) in targets {
            let key = (channel.clone(), thread_ts.clone());
            if !self
                .ws
                .marked_threads
                .get(&key)
                .is_some_and(|prev| *prev >= ts)
            {
                self.ws.marked_threads.insert(key, ts.clone());
                self.bridge.send(MessagingCommand::MarkThreadRead {
                    channel,
                    thread_ts,
                    ts,
                });
            }
        }
        self.sidebar
            .update(cx, |s, cx| s.set_thread_counts(false, 0, cx));
        self.sync_badge(cx);
    }

    /// Clear the sidebar's Threads unread indicator when `ThreadsView` has the remote list and
    /// no unread cards remain.
    pub(super) fn sync_thread_sidebar_counts(&mut self, cx: &mut Context<Self>) {
        let threads = self.threads.read(cx);
        if threads.is_remote() && !threads.has_unreads() {
            self.sidebar
                .update(cx, |s, cx| s.set_thread_counts(false, 0, cx));
            self.sync_badge(cx);
        }
    }

    /// Esc: mark the open conversation read up to its newest message - or, while All Unreads or
    /// Threads is open, mark its unreads read.
    pub(super) fn mark_open_read(&mut self, cx: &mut Context<Self>) {
        if self.page == MainPage::Unreads {
            self.unreads.update(cx, |view, cx| {
                view.handle_key("escape", cx);
            });
            return;
        }
        if self.page == MainPage::Threads {
            self.mark_all_threads_read(cx);
            return;
        }
        let channel = self.channel.clone();
        let newest = self
            .messages
            .read(cx)
            .newest_ts()
            .into_iter()
            .chain(self.ws.counts.get(&channel).and_then(|c| c.latest.clone()))
            .max();
        if let Some(newest) = newest {
            self.mark_read(&channel, newest.clone(), cx);
            self.messages
                .update(cx, |list, cx| list.set_last_read(Some(newest), cx));
        }
    }

    /// Shift+Esc: Slack marks every conversation and thread read (`users.markAllRead`); the
    /// sidebar un-bolds at once and the core's fresh counts follow.
    pub(super) fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        self.bridge.send(MessagingCommand::MarkAllRead);
        for (id, counts) in &mut self.ws.counts {
            counts.clear_unreads();
            if let Some(latest) = counts.latest.clone() {
                counts.last_read = Some(latest.clone());
                self.ws.marked_read.insert(id.clone(), latest);
            }
        }
        let unread = self.sidebar.read(cx).unread_ids();
        self.sidebar.update(cx, |sidebar, cx| {
            for channel in unread {
                sidebar.mark_read(channel, cx);
            }
            sidebar.set_thread_counts(false, 0, cx);
        });
        self.threads.update(cx, |threads, cx| {
            threads.clear_all_unreads(true, cx);
        });
        self.sync_counts_to_views(cx);
        if self.page == MainPage::Unreads {
            self.refresh_unreads(cx);
        }
    }

    /// Header label of a conversation in the shown workspace.
    pub(super) fn label(&self, channel: &ChannelId) -> String {
        self.fixtures
            .conversation(channel)
            .map(|c| self.fixtures.conversation_label(c))
            .unwrap_or_else(|| channel.to_string())
    }

    /// Store the composer's text as the open conversation's draft (blank removes it).
    pub(super) fn save_draft(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text();
        let saved = self
            .drafts
            .read(cx)
            .draft(self.workspace, &self.channel)
            .unwrap_or_default();
        if text == saved {
            return;
        }
        let text_for_cloud = text.clone();
        let entry = Entry {
            id: None,
            workspace: self.workspace,
            channel: self.channel.clone(),
            thread_ts: None,
            label: self.label(&self.channel),
            text,
            at: now_secs(),
        };
        self.drafts
            .update(cx, |drafts, cx| drafts.set_draft(entry, cx));
        let count = self.drafts.read(cx).draft_count();
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_draft_count(count, cx));
        self.persist_drafts(cx);
        if !self.fixtures.sample {
            self.bridge.send(MessagingCommand::SyncDraft {
                channel: self.channel.clone(),
                text: text_for_cloud,
            });
        }
    }

    /// Persist the active workspace's unsent drafts into the local store off the UI thread.
    pub(super) fn persist_drafts(&self, cx: &Context<Self>) {
        self.persist_drafts_for(self.workspace, cx);
    }

    /// Persist `workspace`'s unsent drafts into that workspace's local store off the UI thread.
    pub(super) fn persist_drafts_for(&self, workspace: usize, cx: &Context<Self>) {
        let saved: Vec<SavedDraft> = self
            .drafts
            .read(cx)
            .drafts_for(workspace)
            .into_iter()
            .map(|entry| SavedDraft {
                channel: entry.channel,
                label: entry.label,
                text: entry.text,
                at: entry.at,
            })
            .collect();
        if let Some(bridge) = self.workspace_bridge(workspace)
            && let Some(write) = bridge.remember(&DRAFTS, saved.as_slice())
        {
            cx.background_executor().spawn(write).detach();
        }
    }

    /// Put the open conversation's saved draft (or nothing) in the composer.
    pub(super) fn load_draft(&mut self, cx: &mut Context<Self>) {
        let drafts = self.drafts.read(cx);
        let text = drafts
            .draft(self.workspace, &self.channel)
            .unwrap_or_default();
        let count = drafts.draft_count();
        self.composer
            .update(cx, |composer, cx| composer.load_draft(&text, cx));
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_draft_count(count, cx));
    }
}

pub(in crate::app) struct ScheduleSpec {
    pub channel: ChannelId,
    pub thread_ts: Option<Ts>,
    pub text: String,
    pub post_at: u64,
    pub when_label: String,
}

impl App {
    /// Queue a message (in a channel or inside `thread_ts`) for delivery at unix timestamp
    /// `post_at`, recording it in Drafts & sent and posting `chat.scheduleMessage` live.
    pub(super) fn schedule_at(
        &mut self,
        spec: ScheduleSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ScheduleSpec {
            channel,
            thread_ts,
            text,
            post_at,
            when_label,
        } = spec;
        if !self.fixtures.sample {
            let content = composer::content_of(&text, &self.fixtures);
            self.bridge.send(MessagingCommand::ScheduleMessage {
                channel: channel.clone(),
                post_at,
                content,
                thread_ts: thread_ts.clone(),
            });
        }
        let entry = Entry {
            id: None,
            workspace: self.workspace,
            channel: channel.clone(),
            thread_ts,
            label: self.label(&channel),
            text,
            at: post_at,
        };
        self.drafts
            .update(cx, |drafts, cx| drafts.schedule(entry, cx));
        self.save_draft(cx);
        cx.notify();
        // In live mode, wait for Slack's `Event::MessageScheduled` confirmation (or surface
        // `Event::Error` on rejection) instead of showing a misleading success toast up front.
        if self.fixtures.sample {
            notify(
                "Message scheduled",
                format!("Sends {when_label}. See Drafts & sent."),
                window,
                cx,
            );
        }
    }

    /// Deliver a scheduled message now, through the send pipeline in its own workspace.
    /// Deleting the scheduled copy on Slack is deferred until `dispatch` so canceling a secret
    /// or `@channel` guard keeps the scheduled message intact.
    pub(super) fn deliver(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        if entry.workspace != self.workspace {
            self.switch_workspace(entry.workspace, window, cx);
        }
        let label = entry.label.clone();
        // A scheduled message is kept as its draft text; it posts as the draft would have.
        let msg = OutgoingMessage {
            channel: entry.channel.clone(),
            thread_ts: entry.thread_ts.clone(),
            broadcast: false,
            content: composer::content_of(&entry.text, &self.fixtures),
            files: None,
            origin: OutgoingOrigin::Scheduled(Box::new(entry)),
        };
        if self.submit(msg, window, cx) {
            notify("Message sent", format!("Delivered to {label}"), window, cx);
        }
    }
}

/// Where an [`OutgoingMessage`] originated, so cancelling a send guard restores the right
/// composer (and rolls back any optimistic thread echo) without clobbering unrelated state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum OutgoingOrigin {
    #[default]
    MainComposer,
    ThreadPanel,
    ThreadsView,
    Scheduled(Box<Entry>),
    External,
}

/// A message on its way out, with the target captured when it was submitted: a channel switch
/// while a confirmation dialog is open cannot redirect it.
#[derive(Clone)]
pub(super) struct OutgoingMessage {
    pub channel: ChannelId,
    /// `Some` for a thread reply, which `broadcast` then also posts to the channel.
    pub thread_ts: Option<Ts>,
    pub broadcast: bool,
    pub content: MessageContent,
    /// Staged files the message uploads with (`None` for a plain message), shared so a held
    /// confirmation does not copy them.
    pub files: Option<Rc<Vec<composer::PendingAttachment>>>,
    pub origin: OutgoingOrigin,
}

/// The list a message-level action was triggered from: an unread group's row or a Threads card.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageSurface {
    Unreads,
    Threads,
}

/// The message a list-surface action names: its surface, conversation, and the card a Threads
/// row belongs to.
pub(crate) struct MessageTarget {
    pub surface: MessageSurface,
    pub channel: ChannelId,
    pub thread_ts: Option<Ts>,
}

/// A New-message send waiting for [`ChannelCommand::OpenDm`] to open the DM with `users`.
pub(super) struct PendingDm {
    pub users: Vec<UserId>,
    pub content: MessageContent,
}

/// One candidate target when disambiguating a duplicate slash command (`F-COLLAB-686`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlashCandidate {
    pub app_id: Option<String>,
    pub provider_label: String,
    pub usage: String,
    pub description: String,
    pub is_builtin: bool,
}

/// Collect all distinct command targets (built-in + workspace apps) matching `invocation.name`
/// for duplicate slash-command disambiguation (`F-COLLAB-686`).
pub(crate) fn matching_slash_candidates(
    invocation: &Invocation,
    app_commands: &[slackless_core::api::AppCommand],
) -> Vec<SlashCandidate> {
    let mut out = Vec::new();
    if let Some(builtin) = invocation.command {
        out.push(SlashCandidate {
            app_id: None,
            provider_label: "Slackless Built-in".to_owned(),
            usage: builtin.usage.to_owned(),
            description: builtin.description.to_owned(),
            is_builtin: true,
        });
    }
    let mut seen = std::collections::HashSet::new();
    for cmd in app_commands {
        if !cmd.name.eq_ignore_ascii_case(&invocation.name) {
            continue;
        }
        let key = (
            cmd.app_id.clone().unwrap_or_default(),
            cmd.app_name
                .clone()
                .unwrap_or_default()
                .to_ascii_lowercase(),
        );
        if !seen.insert(key) {
            continue;
        }
        let provider_label = cmd
            .app_name
            .clone()
            .or_else(|| cmd.app_id.as_ref().map(|id| format!("App ({id})")))
            .unwrap_or_else(|| "Workspace app".to_owned());
        out.push(SlashCandidate {
            app_id: cmd.app_id.clone(),
            provider_label,
            usage: cmd.usage.clone(),
            description: cmd.description.clone(),
            is_builtin: false,
        });
    }
    out
}

/// Minimum member count that triggers Slack's `@channel`/`@here`/`@everyone` confirmation dialog.
pub(crate) const BROADCAST_WARN_THRESHOLD: usize = 6;
/// Enterprise Grid large-channel broadcast limit threshold (`F-COLLAB-1248`).
pub(crate) const ENTERPRISE_BROADCAST_LIMIT: usize = 10_000;

/// Permission & large-channel evaluation for a `@channel` / `@here` / `@everyone` mention
/// (`F-COLLAB-1248`, `F-COLLAB-1249`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BroadcastGuard {
    pub title: String,
    pub body: String,
    pub member_count: usize,
    pub is_large_enterprise: bool,
    pub is_restricted: bool,
    pub restriction_note: Option<String>,
}

/// Inspect channel properties for restricted `@channel`/`@here`/`@everyone` permissions
/// (`F-COLLAB-1249`).
pub(crate) fn is_broadcast_restricted(conversation: &Conversation) -> bool {
    let prop = |key: &str| -> Option<&serde_json::Value> {
        conversation
            .properties
            .as_ref()
            .and_then(|p| p.extra.get(key))
            .or_else(|| {
                conversation
                    .extra
                    .get("properties")
                    .and_then(|p| p.get(key))
            })
    };
    let at_channel_restricted = prop("at_channel_restricted")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let can_broadcast_false =
        prop("can_broadcast").and_then(serde_json::Value::as_bool) == Some(false);
    let broadcast_restricted_to = prop("broadcast_restricted_to").is_some_and(|v| !v.is_null());
    let posting_restricted_to = prop("posting_restricted_to").is_some_and(|v| !v.is_null());
    at_channel_restricted
        || can_broadcast_false
        || broadcast_restricted_to
        || posting_restricted_to
        || conversation.is_org_mandatory
}

/// Human-readable `@channel / @here` permission line for Channel Details -> About -> Permissions
/// (`F-COLLAB-1248`, `F-COLLAB-1249`).
pub(crate) fn mention_permission_summary(
    conversation: &Conversation,
    member_count: usize,
) -> String {
    if conversation.is_archived {
        return "@channel / @here mentions are disabled in archived channels.".to_owned();
    }
    if is_broadcast_restricted(conversation) {
        return "Broadcast mentions (@channel / @here / @everyone) are restricted to Channel Managers & Workspace Admins.".to_owned();
    }
    if member_count >= ENTERPRISE_BROADCAST_LIMIT {
        return format!(
            "Large channel ({member_count} members) \u{b7} @channel / @here mentions require confirmation and may be restricted by Enterprise policy."
        );
    }
    if conversation.is_general {
        return "Default channel \u{b7} @everyone and @channel notify all workspace members (confirmation shown).".to_owned();
    }
    "Everyone in this channel can use @channel and @here mentions.".to_owned()
}

/// Short badge shown on `@channel`/`@here`/`@everyone` rows in the composer's `@` autocomplete
/// (`F-COLLAB-1248`, `F-COLLAB-1249`).
pub(crate) fn broadcast_autocomplete_hint(
    conversation: &Conversation,
    member_count: usize,
) -> Option<String> {
    if is_broadcast_restricted(conversation) {
        return Some("Restricted".to_owned());
    }
    if member_count >= ENTERPRISE_BROADCAST_LIMIT {
        return Some(format!("{member_count} members \u{b7} Limit"));
    }
    if member_count >= BROADCAST_WARN_THRESHOLD {
        return Some(format!("{member_count} members"));
    }
    None
}

/// The `@channel` confirmation `content` needs in `channel` (`F-CORE-083`), if any, for the main
/// window and popped-out composers alike. `fresh_members`: a member count newer than the
/// snapshot's (`conversations.info`).
pub(crate) fn broadcast_guard(
    content: &MessageContent,
    fixtures: &Fixtures,
    channel: &ChannelId,
    fresh_members: Option<u32>,
) -> Option<BroadcastGuard> {
    let range = broadcast_kind(content)?;
    let conv = fixtures.conversation(channel)?;
    let members = (fresh_members.or(conv.num_members)).map_or(conv.members.len(), |n| {
        usize::try_from(n).unwrap_or(usize::MAX)
    });
    evaluate_broadcast_guard(&range, conv, members, fixtures.user(&fixtures.self_user))
}

/// Ask before `guard`'s broadcast goes out: "Send" runs `on_send`, "Cancel" `on_cancel`.
pub(crate) fn confirm_broadcast(
    guard: BroadcastGuard,
    on_send: impl Fn(&mut Window, &mut gpui::App) + 'static,
    on_cancel: impl Fn(&mut Window, &mut gpui::App) + 'static,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let (on_send, on_cancel) = (Rc::new(on_send), Rc::new(on_cancel));
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = cx.theme();
        let (muted, warning, border, card_bg) = (
            theme.muted_foreground,
            theme.warning,
            theme.border,
            theme.secondary.opacity(0.4),
        );
        let (on_send, on_cancel, on_cancel_close) =
            (on_send.clone(), on_cancel.clone(), on_cancel.clone());
        let count_badge = if guard.is_large_enterprise {
            format!(
                "{} members \u{b7} Large Enterprise channel",
                guard.member_count
            )
        } else if guard.member_count > 0 {
            format!(
                "{} {}",
                guard.member_count,
                if guard.member_count == 1 {
                    "member"
                } else {
                    "members"
                }
            )
        } else {
            "Channel broadcast".to_owned()
        };
        dialog
            .title(guard.title.clone())
            .w(px(480.))
            .on_close(move |_, window, cx| on_cancel_close(window, cx))
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(border)
                                    .bg(card_bg)
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(
                                        Icon::new(IconName::Users).size(px(12.)).text_color(muted),
                                    )
                                    .child(count_badge),
                            )
                            .when(guard.is_restricted, |row| {
                                row.child(
                                    h_flex()
                                        .items_center()
                                        .gap_1()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(warning.opacity(0.45))
                                        .bg(warning.opacity(0.14))
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(warning)
                                        .child(
                                            Icon::new(IconName::TriangleAlert)
                                                .size(px(12.))
                                                .text_color(warning),
                                        )
                                        .child("Restricted mention"),
                                )
                            }),
                    )
                    .child(div().text_sm().text_color(muted).child(guard.body.clone()))
                    .when_some(guard.restriction_note.clone(), |col, note| {
                        col.child(
                            h_flex()
                                .items_start()
                                .gap_2()
                                .p_2p5()
                                .rounded_md()
                                .border_1()
                                .border_color(warning.opacity(0.4))
                                .bg(warning.opacity(0.1))
                                .text_xs()
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .size(px(14.))
                                        .text_color(warning),
                                )
                                .child(div().flex_1().min_w_0().child(note)),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .pt_1()
                            .child(
                                preferences::form_button(
                                    Button::new("broadcast-cancel").outline().label("Cancel"),
                                    cx,
                                )
                                .debug_selector(|| "broadcast-cancel".into())
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    on_cancel(window, cx);
                                }),
                            )
                            .child(
                                preferences::form_button(
                                    if guard.is_restricted {
                                        Button::new("broadcast-send").danger().label("Send anyway")
                                    } else {
                                        Button::new("broadcast-send").primary().label("Send")
                                    },
                                    cx,
                                )
                                .debug_selector(|| "broadcast-send".into())
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    on_send(window, cx);
                                }),
                            ),
                    ),
            )
    });
}

/// Evaluate whether sending `range` in `conversation` requires a confirmation / restricted-mention
/// warning dialog (`F-COLLAB-1248`, `F-COLLAB-1249`). Returns `None` when no guard is needed.
pub(crate) fn evaluate_broadcast_guard(
    range: &BroadcastRange,
    conversation: &Conversation,
    member_count: usize,
    current_user: Option<&slackless_core::types::User>,
) -> Option<BroadcastGuard> {
    let is_channel = matches!(
        conversation.kind(),
        ConversationKind::PublicChannel | ConversationKind::PrivateChannel
    );
    if !is_channel {
        return None;
    }
    let restricted_channel = is_broadcast_restricted(conversation);
    let is_large_enterprise = member_count >= ENTERPRISE_BROADCAST_LIMIT;
    let is_admin_or_owner = current_user.is_some_and(|u| u.is_admin || u.is_owner);
    let restricted_for_user = (restricted_channel || is_large_enterprise) && !is_admin_or_owner;

    if member_count < BROADCAST_WARN_THRESHOLD && !restricted_channel {
        return None;
    }
    let channel_name = conversation
        .name
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(conversation.id.as_str());
    let title = match range {
        BroadcastRange::Here => format!("Notify active members in #{channel_name}?"),
        BroadcastRange::Everyone => format!("Notify everyone in #{channel_name}?"),
        _ => format!("Notify everyone in #{channel_name}?"),
    };
    let count_phrase = if member_count > 0 {
        format!(
            "{member_count} {}",
            if member_count == 1 {
                "person"
            } else {
                "people"
            }
        )
    } else {
        "everyone".to_owned()
    };
    let body = format!(
        "By using @{range}, you\u{2019}re about to notify {count_phrase} in #{channel_name}. Are you sure you want to continue?"
    );
    let restriction_note = if is_large_enterprise {
        Some(if is_admin_or_owner {
            format!(
                "Large Enterprise channel ({member_count} members): broadcast mentions reach the entire channel."
            )
        } else {
            format!(
                "Enterprise large-channel mention limit ({member_count} members): @{range} is restricted to Workspace Admins and Channel Managers. Slack may block or suppress this notification."
            )
        })
    } else if restricted_channel {
        Some(if is_admin_or_owner {
            "Restricted mention permissions are active in this channel; you can notify as a Workspace Admin.".to_owned()
        } else {
            format!(
                "Restricted mentions: only Workspace Admins or Channel Managers can notify @{range} in #{channel_name}. Slack may block or suppress this notification."
            )
        })
    } else {
        None
    };

    Some(BroadcastGuard {
        title,
        body,
        member_count,
        is_large_enterprise,
        is_restricted: restricted_for_user || restricted_channel,
        restriction_note,
    })
}

impl App {
    /// Dispatch a slash command typed in the main composer, or in the reply composer of
    /// `thread`. Live mode hands `/giphy` and commands slackless does not know to Slack
    /// (`chat.command`; the result arrives on the websocket). When multiple apps (or a built-in
    /// and an app) register the same `/command` and the user did not already pick one in
    /// autocomplete, opens the disambiguation picker dialog (`F-COLLAB-686`).
    pub(super) fn run_slash(
        &mut self,
        invocation: &Invocation,
        thread: Option<(ChannelId, Ts)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if invocation.app_id.is_none() && invocation.app_name.is_none() && !invocation.force_builtin
        {
            let candidates = matching_slash_candidates(invocation, &self.fixtures.app_commands);
            if candidates.len() > 1 {
                self.open_slash_disambiguation_dialog(
                    invocation.clone(),
                    candidates,
                    thread,
                    window,
                    cx,
                );
                return;
            }
        }
        let is_local_builtin = invocation.command.is_some_and(|command| {
            matches!(
                command.kind,
                SlashKind::Shrug | SlashKind::Away | SlashKind::Status | SlashKind::Dnd
            )
        });
        let remote = (!invocation.force_builtin
            && (invocation.app_id.is_some() || invocation.app_name.is_some()))
            || !is_local_builtin;
        if remote && !self.fixtures.sample {
            let (channel, thread_ts) = match thread {
                Some((channel, thread_ts)) => (channel, Some(thread_ts)),
                None => (self.channel.clone(), None),
            };
            self.bridge.send(MessagingCommand::RunCommand {
                channel,
                command: format!("/{}", invocation.name),
                args: invocation.args.clone(),
                thread_ts,
                app_id: if invocation.force_builtin {
                    None
                } else {
                    invocation.app_id.clone()
                },
            });
            return;
        }
        // In fixture mode, if an explicit workspace app command matches, surface a clean preview.
        if remote && self.fixtures.sample {
            let matched_app = self
                .fixtures
                .app_commands
                .iter()
                .find(|c| {
                    c.name.eq_ignore_ascii_case(&invocation.name)
                        && (invocation.app_id.is_none() || c.app_id == invocation.app_id)
                })
                .cloned();
            if let Some(app_cmd) = matched_app {
                let via = invocation
                    .app_name
                    .as_deref()
                    .or(app_cmd.app_name.as_deref())
                    .unwrap_or("App");
                let summary = if invocation.args.is_empty() {
                    format!("Ran /{} via {via}.", invocation.name)
                } else {
                    format!("Ran /{} {} via {via}.", invocation.name, invocation.args)
                };
                let (channel, thread_ts) = match thread {
                    Some((channel, thread_ts)) => (channel, Some(thread_ts)),
                    None => (self.channel.clone(), None),
                };
                let mut eph = self
                    .fixtures
                    .local_message(&channel, summary.clone(), thread_ts);
                eph.user = None;
                eph.username = Some(via.to_owned());
                eph.is_ephemeral = true;
                self.messages
                    .update(cx, |list, cx| list.append(channel, eph, cx));
                notify(
                    &format!("/{} ({via})", invocation.name),
                    summary,
                    window,
                    cx,
                );
                return;
            }
        }
        let Some(command) = invocation.command else {
            let (title, detail) =
                crate::slash::format_command_error(&invocation.name, "unknown_command");
            notify(&title, detail, window, cx);
            return;
        };
        match command.kind {
            SlashKind::Shrug => {
                let text = crate::composer::format_shrug(&invocation.args);
                self.send(MessageContent::plain(text), window, cx);
            }
            SlashKind::Away => {
                self.self_state.away = !self.self_state.away;
                if !self.fixtures.sample {
                    self.bridge.send(WorkspaceCommand::SetPresence {
                        away: self.self_state.away,
                    });
                }
                self.push_self_state(cx);
                let state = if self.self_state.away {
                    "away"
                } else {
                    "active"
                };
                notify("/away", format!("You are now {state}."), window, cx);
            }
            SlashKind::Status if invocation.args.eq_ignore_ascii_case("clear") => {
                self.self_state.set_status(None);
                self.send_status_to_slack(None);
                self.push_self_state(cx);
                notify("/status", "Your status was cleared.", window, cx);
            }
            SlashKind::Status => self.open_status_picker(window, cx),
            SlashKind::Dnd => match parse_dnd_minutes(&invocation.args) {
                Some(None) => {
                    self.self_state.paused_until = None;
                    if !self.fixtures.sample {
                        self.bridge.send(WorkspaceCommand::EndDndSnooze);
                    }
                    self.push_self_state(cx);
                    notify("/dnd", "Notifications resumed.", window, cx);
                }
                Some(Some(minutes)) => {
                    let until = crate::messages::now_secs() + i64::from(minutes) * 60;
                    self.self_state.paused_until = Some(until);
                    if !self.fixtures.sample {
                        self.bridge.send(WorkspaceCommand::SetDndSnooze { minutes });
                    }
                    self.push_self_state(cx);
                    notify(
                        "/dnd",
                        format!("Notifications paused for {minutes} minutes."),
                        window,
                        cx,
                    );
                }
                None => {
                    notify("/dnd", "Usage: /dnd [30m | 2h | 1h30m | off]", window, cx);
                }
            },
            kind => {
                tracing::info!(
                    kind = ?kind,
                    command = %command.name,
                    args = ?invocation.args,
                    channel = %self.channel,
                    "slash command not executed in fixture mode"
                );
                notify(
                    &format!("/{}", command.name),
                    "Logged. This command runs against a live workspace.",
                    window,
                    cx,
                );
            }
        }
    }

    /// The composer's submission for the open conversation.
    pub(super) fn send(
        &mut self,
        content: MessageContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.submit(
            OutgoingMessage {
                channel: self.channel.clone(),
                thread_ts: None,
                broadcast: false,
                content,
                files: None,
                origin: OutgoingOrigin::MainComposer,
            },
            window,
            cx,
        );
    }

    /// The one send pipeline, whatever surface a message leaves from (composer, thread panel,
    /// Unreads quick reply, Drafts "Send now", "Send to channel", a DM opened for a New
    /// message): secret check, `@channel` guard, then the outgoing transforms and the core.
    /// The confirmation dialogs carry `msg`, so the target is the one captured here, not
    /// whatever conversation is open when the user confirms. Returns `false` while a dialog
    /// holds the message, so callers keep their "sent" feedback for an immediate hand-off.
    pub(super) fn submit(
        &mut self,
        msg: OutgoingMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if composer::outgoing_guard_prefs(cx).warn_before_sending_secrets {
            let matches = slackless_core::detect_secrets(&msg.content.text);
            if !matches.is_empty() {
                self.confirm_secret_send(matches, msg, window, cx);
                return false;
            }
        }
        self.guard_broadcast(msg, window, cx)
    }

    /// [`App::submit`] once the text holds no secret: Slack's `@channel` confirmation
    /// (`F-CORE-083`) for a channel big or restricted enough to warn about, else straight on.
    fn guard_broadcast(
        &mut self,
        msg: OutgoingMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let fresh_members = self.ws.member_counts.get(&msg.channel).copied();
        let Some(guard) =
            broadcast_guard(&msg.content, &self.fixtures, &msg.channel, fresh_members)
        else {
            self.dispatch(msg, cx);
            return true;
        };
        let (this_send, this_cancel) = (cx.weak_entity(), cx.weak_entity());
        // Taken once on Send or Cancel, so staged files are neither copied nor dropped.
        let pending = Rc::new(Cell::new(Some(msg)));
        confirm_broadcast(
            guard,
            {
                let pending = Rc::clone(&pending);
                move |_, cx| {
                    if let Some(msg) = pending.take() {
                        let _ = this_send.update(cx, |app, cx| app.dispatch(msg, cx));
                    }
                }
            },
            {
                let pending = Rc::clone(&pending);
                move |_, cx| {
                    if let Some(msg) = pending.take() {
                        let _ = this_cancel.update(cx, |app, cx| {
                            app.restore_canceled_send(msg, cx);
                        });
                    }
                }
            },
            window,
            cx,
        );
        false
    }

    /// Show a confirmation dialog when outgoing text matches known API keys, tokens, or private
    /// keys. `msg` waits in it: "Send anyway" continues through [`Self::guard_broadcast`]; Cancel
    /// puts its text and staged files back into the main composer when it came from there.
    pub(super) fn confirm_secret_send(
        &self,
        matches: Vec<slackless_core::SecretMatch>,
        msg: OutgoingMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending = Rc::new(Cell::new(Some(msg)));
        let summary_lines: Vec<String> = matches
            .iter()
            .map(|m| format!("\u{2022} {}: {}", m.kind, m.preview))
            .collect();
        let (this_send, this_cancel) = (cx.weak_entity(), cx.weak_entity());
        crate::ui::ConfirmDialog::new(
            "Possible secret detected",
            "This message looks like it contains credentials or a private key. Are you sure you want to send it?",
            "Send anyway",
        )
        .danger()
        .width(px(480.))
        .extra(move |cx| {
            let theme = cx.theme();
            v_flex()
                .gap_1()
                .p_2p5()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.secondary.opacity(0.35))
                .children(summary_lines.iter().map(|line| {
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .text_xs()
                        .text_color(theme.danger)
                        .child(line.clone())
                }))
                .into_any_element()
        })
        .on_cancel({
            let pending = Rc::clone(&pending);
            move |_, cx| {
                if let Some(msg) = pending.take() {
                    let _ = this_cancel.update(cx, |app, cx| {
                        app.restore_canceled_send(msg, cx);
                    });
                }
            }
        })
        .open(window, cx, move |window, cx| {
            if let Some(msg) = pending.take() {
                let _ = this_send.update(cx, |app, cx| app.guard_broadcast(msg, window, cx));
            }
        });
    }

    /// Open the duplicate slash-command disambiguation picker dialog (`F-COLLAB-686`) when
    /// multiple apps (or a built-in + an app) share `/<name>`.
    pub(super) fn open_slash_disambiguation_dialog(
        &self,
        invocation: Invocation,
        candidates: Vec<SlashCandidate>,
        thread: Option<(ChannelId, Ts)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cmd_name = invocation.name.clone();
        let args_preview = invocation.args.clone();
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (muted, border, hover) = (theme.muted_foreground, theme.border, theme.accent);
            let subtitle = if args_preview.is_empty() {
                format!("Multiple apps provide /{cmd_name}. Choose which one should run it:")
            } else {
                format!(
                    "Multiple apps provide /{cmd_name} ({args_preview}). Choose which one should run it:"
                )
            };
            let rows: Vec<_> = candidates
                .iter()
                .enumerate()
                .map(|(ix, cand)| {
                    let chosen = if cand.is_builtin {
                        invocation.clone().with_builtin()
                    } else {
                        invocation
                            .clone()
                            .with_app(cand.app_id.clone(), Some(cand.provider_label.clone()))
                    };
                    let this_row = this.clone();
                    let chosen_row = chosen.clone();
                    let thread_row = thread.clone();
                    let this_btn = this.clone();
                    let thread_btn = thread.clone();
                    h_flex()
                        .id(("slash-disambig-row", ix))
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .p_2p5()
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .cursor_pointer()
                        .hover(move |el| el.bg(hover))
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let _ = this_row.update(cx, |app, cx| {
                                app.run_slash(&chosen_row, thread_row.clone(), window, cx);
                            });
                        })
                        .child(
                            v_flex()
                                .min_w_0()
                                .flex_1()
                                .gap_0p5()
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(cand.provider_label.clone()),
                                        )
                                        .when(!cand.usage.is_empty(), |row| {
                                            row.child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child(cand.usage.clone()),
                                            )
                                        }),
                                )
                                .when(!cand.description.is_empty(), |col| {
                                    col.child(
                                        div()
                                            .min_w_0()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(cand.description.clone()),
                                    )
                                }),
                        )
                        .child(
                            div().flex_none().child(
                                preferences::form_button(
                                    Button::new(("slash-disambig-run", ix))
                                        .primary()
                                        .label("Run"),
                                    cx,
                                )
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    window.close_dialog(cx);
                                    let _ = this_btn.update(cx, |app, cx| {
                                        app.run_slash(&chosen, thread_btn.clone(), window, cx);
                                    });
                                }),
                            ),
                        )
                })
                .collect();
            dialog
                .title(format!("Choose app for /{cmd_name}"))
                .w(px(480.))
                .child(
                    v_flex()
                        .gap_3()
                        .child(div().text_sm().text_color(muted).child(subtitle))
                        .child(
                            v_flex()
                                .id("slash-disambig-list")
                                .max_h(px(320.))
                                .overflow_y_scroll()
                                .on_scroll(|_, _, cx| cx.stop_propagation())
                                .gap_2()
                                .children(rows),
                        )
                        .child(
                            h_flex().justify_end().pt_1().child(
                                preferences::form_button(
                                    Button::new("slash-disambig-cancel")
                                        .outline()
                                        .label("Cancel"),
                                    cx,
                                )
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                            ),
                        ),
                )
        });
    }

    /// Returns `true` when the live workspace RTM connection is currently disconnected or
    /// reconnecting (so outgoing messages will land directly in the durable V13 outbox).
    pub(super) fn is_connection_offline(&self) -> bool {
        matches!(
            self.ws.connection,
            Some(
                slackless_core::rtm::ConnectionState::Reconnecting { .. }
                    | slackless_core::rtm::ConnectionState::Disconnected { .. }
            )
        )
    }

    /// Hand `msg` to the core after the outgoing transforms (tracking-param strip, emoticon
    /// conversion). Staged files upload with it; a thread reply goes straight out; a top-level
    /// message also echoes into the transcript, joins the sent list and drops the composer's
    /// saved draft for its channel.
    fn dispatch(&mut self, mut msg: OutgoingMessage, cx: &mut Context<Self>) {
        composer::apply_outgoing_transforms(&mut msg.content, &self.fixtures, cx);
        if let Some(files) = msg.files.take() {
            self.upload(msg, files, cx);
            return;
        }
        let OutgoingMessage {
            channel,
            thread_ts,
            broadcast,
            content,
            files: _,
            origin,
        } = msg;
        if let OutgoingOrigin::Scheduled(entry) = origin
            && let Some(id) = entry.id
            && let Some(bridge) = self.workspace_bridge(entry.workspace)
        {
            bridge.send(MessagingCommand::DeleteScheduledMessage {
                channel: entry.channel,
                scheduled_message_id: id,
            });
        }
        let Some(thread_ts) = thread_ts else {
            self.send_to_conversation(channel, content, cx);
            return;
        };
        self.bridge.send(MessagingCommand::ReplyThread {
            channel,
            thread_ts,
            content,
            broadcast,
        });
        self.sync_thread_sidebar_counts(cx);
    }

    /// Restore the originating composer (and roll back any optimistic thread echo) when the user
    /// cancels a secret warning or `@channel` broadcast confirmation dialog.
    fn restore_canceled_send(&mut self, msg: OutgoingMessage, cx: &mut Context<Self>) {
        match msg.origin {
            OutgoingOrigin::MainComposer => {
                self.restore_composer(&msg.channel, &msg.content.text, msg.files, cx);
            }
            OutgoingOrigin::ThreadPanel => {
                if let Some(thread_ts) = &msg.thread_ts {
                    self.thread.update(cx, |panel, cx| {
                        panel.rollback_pending_reply(
                            &msg.channel,
                            thread_ts,
                            &msg.content.text,
                            cx,
                        );
                    });
                }
            }
            OutgoingOrigin::ThreadsView => {
                if let Some(thread_ts) = &msg.thread_ts {
                    self.threads.update(cx, |view, cx| {
                        view.rollback_pending_reply(&msg.channel, thread_ts, &msg.content.text, cx);
                    });
                }
            }
            OutgoingOrigin::Scheduled(entry) => {
                self.drafts
                    .update(cx, |drafts, cx| drafts.schedule(*entry, cx));
            }
            OutgoingOrigin::External => {}
        }
    }

    /// Put a canceled send back into the main composer of `channel`: its `text` and staged
    /// `files`, showing `channel` again when the user has moved on meanwhile.
    fn restore_composer(
        &mut self,
        channel: &ChannelId,
        text: &str,
        files: Option<Rc<Vec<composer::PendingAttachment>>>,
        cx: &mut Context<Self>,
    ) {
        if self.channel != *channel {
            self.select(channel.clone(), cx);
        }
        let files = files.map_or_else(Vec::new, |files| {
            Rc::try_unwrap(files).unwrap_or_else(|shared| (*shared).clone())
        });
        self.composer.update(cx, |composer, cx| {
            composer.load_draft(text, cx);
            composer.restore_attachments(files, cx);
        });
    }

    fn send_to_conversation(
        &mut self,
        channel: ChannelId,
        content: MessageContent,
        cx: &mut Context<Self>,
    ) {
        // When offline in live mode, skip the synthetic optimistic echo so the durable V13
        // inline outbox card ("Queued — will send when online") renders cleanly without
        // duplicating the message.
        if self.fixtures.sample || !self.is_connection_offline() {
            let mut message = self
                .fixtures
                .local_message(&channel, content.text.clone(), None);
            message.blocks = content.blocks.clone();
            message.attachments = content.attachments.clone();
            self.messages
                .update(cx, |list, cx| list.append(channel.clone(), message, cx));
        }
        let sent_text = if content.text.is_empty() {
            content
                .attachments
                .first()
                .and_then(|a| a.fallback.clone())
                .unwrap_or_default()
        } else {
            content.text.clone()
        };
        self.ws.explicit_unread.remove(&channel);
        let entry = Entry {
            id: None,
            workspace: self.workspace,
            channel: channel.clone(),
            thread_ts: None,
            label: self.label(&channel),
            text: sent_text,
            at: now_secs(),
        };
        self.drafts
            .update(cx, |drafts, cx| drafts.record_sent(entry, cx));
        self.bridge.send(MessagingCommand::SendMessage {
            channel,
            content,
            thread_ts: None,
            broadcast: false,
        });
        // The composer is empty now: drop any saved draft for this conversation.
        self.save_draft(cx);
    }

    /// Upload staged files (with optional message text) into `channel` (or `thread_ts`).
    pub(super) fn upload_files(
        &mut self,
        channel: ChannelId,
        thread_ts: Option<Ts>,
        content: Option<MessageContent>,
        files: Vec<composer::PendingAttachment>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if files.is_empty() {
            return;
        }
        let (valid_files, oversized): (Vec<_>, Vec<_>) = files
            .into_iter()
            .partition(|f| (f.bytes.len() as u64) <= composer::MAX_UPLOAD_BYTES);
        if !oversized.is_empty() {
            let names: Vec<_> = oversized.iter().map(|f| f.filename.as_str()).collect();
            notify(
                "File too large",
                format!(
                    "Skipped {} (exceeds Slack's 1 GB upload limit).",
                    names.join(", ")
                ),
                window,
                cx,
            );
        }
        if valid_files.is_empty() {
            return;
        }
        let count = valid_files.len();
        // The message rides the one send pipeline (secret check, `@channel` guard) with its files.
        let origin = if thread_ts.is_none() {
            OutgoingOrigin::MainComposer
        } else {
            OutgoingOrigin::External
        };
        let msg = OutgoingMessage {
            channel,
            thread_ts,
            broadcast: false,
            content: content.unwrap_or_default(),
            files: Some(Rc::new(valid_files)),
            origin,
        };
        if self.submit(msg, window, cx) && self.bridge.store().is_some() {
            let label = if count == 1 { "file" } else { "files" };
            notify(
                "Uploading",
                format!("Uploading {count} {label}\u{2026}"),
                window,
                cx,
            );
        }
        self.save_draft(cx);
    }

    /// Upload `files` with `msg`'s text (if any) to its channel or thread, or echo them in
    /// sample mode.
    fn upload(
        &mut self,
        msg: OutgoingMessage,
        files: Rc<Vec<composer::PendingAttachment>>,
        cx: &mut Context<Self>,
    ) {
        let files = Rc::try_unwrap(files).unwrap_or_else(|shared| (*shared).clone());
        let OutgoingMessage {
            channel,
            thread_ts,
            content,
            ..
        } = msg;
        let content = (content != MessageContent::default()).then_some(content);
        if self.bridge.store().is_some() {
            let uploads = files
                .into_iter()
                .map(|f| slackless_core::api::UploadFile {
                    filename: f.filename,
                    bytes: f.bytes.into(),
                })
                .collect();
            self.bridge.send(CollabCommand::UploadFiles {
                channel,
                thread_ts,
                content,
                files: uploads,
            });
        } else {
            // Fixture mode: echo the message locally with stub file entries.
            let text = content.as_ref().map(|c| c.text.clone()).unwrap_or_default();
            let mut message = self.fixtures.local_message(&channel, text, thread_ts);
            if let Some(content) = content {
                message.blocks = content.blocks;
                message.attachments = content.attachments;
            }
            message.files = files
                .into_iter()
                .enumerate()
                .map(|(i, f)| File {
                    id: slackless_core::types::FileId::from(format!("FLOCAL{i}")),
                    name: Some(f.filename.clone()),
                    title: Some(f.filename),
                    size: Some(f.bytes.len() as i64),
                    ..Default::default()
                })
                .collect();
            self.messages
                .update(cx, |list, cx| list.append(channel, message, cx));
        }
    }

    /// Dispatch a message-level action from a row's hover toolbar, reaction chips, image click,
    /// Block Kit action or context menu on one of the two list surfaces (`UnreadsEvent::MessageAction`,
    /// `ThreadsEvent::MessageAction`). `thread_ts` is the card a Threads row belongs to.
    pub(super) fn handle_message_action(
        &mut self,
        target: MessageTarget,
        message: Message,
        action: crate::actions::MessageAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::actions::MessageAction;
        let MessageTarget {
            surface,
            channel,
            thread_ts,
        } = target;
        let ts = message.ts.clone();
        match action {
            MessageAction::React(name) => {
                let add = match surface {
                    MessageSurface::Unreads => self
                        .unreads
                        .update(cx, |u, cx| u.toggle_reaction(&channel, &ts, &name, cx)),
                    MessageSurface::Threads => self
                        .threads
                        .update(cx, |t, cx| t.add_reaction(&channel, &ts, &name, cx))
                        .then_some(true),
                };
                if let Some(add) = add {
                    self.post_change(
                        &channel,
                        &MessageChange::React {
                            ts: ts.clone(),
                            name,
                            add,
                        },
                    );
                }
            }
            MessageAction::ToggleReaction(name) => {
                let add = match surface {
                    MessageSurface::Unreads => self
                        .unreads
                        .update(cx, |u, cx| u.toggle_reaction(&channel, &ts, &name, cx)),
                    MessageSurface::Threads => self
                        .threads
                        .update(cx, |t, cx| t.toggle_reaction(&channel, &ts, &name, cx)),
                };
                if let Some(add) = add {
                    self.post_change(
                        &channel,
                        &MessageChange::React {
                            ts: ts.clone(),
                            name,
                            add,
                        },
                    );
                }
            }
            MessageAction::PickReaction(position) => {
                self.open_picker(PickerTarget::Message { channel, ts }, position, window, cx);
            }
            MessageAction::ReplyInThread => match surface {
                MessageSurface::Unreads => {
                    let thread_ts = message.thread_ts.clone().unwrap_or(ts);
                    self.open_thread(channel, thread_ts, cx);
                }
                MessageSurface::Threads => self.threads.update(cx, |t, cx| {
                    t.activate_thread(&channel, &thread_ts.clone().unwrap_or_default(), window, cx)
                }),
            },
            MessageAction::CopyLink => {
                actions::copy_link(&self.fixtures, &channel, &ts, window, cx);
            }
            MessageAction::CopyText => {
                actions::copy_message_text(&self.fixtures, &message, window, cx);
            }
            MessageAction::Forward => {
                let link = actions::permalink(&self.fixtures, &channel, &ts);
                self.open_compose(&link, window, cx);
            }
            MessageAction::SaveForLater => {
                let saved = !self
                    .later
                    .read(cx)
                    .saved_in(self.workspace)
                    .iter()
                    .any(|(_, s_ts)| *s_ts == ts);
                let request =
                    actions::build_save_request(&self.fixtures, channel, &message, saved, None);
                self.save_for_later(&request, cx);
                self.sync_saved(cx);
                actions::notify_saved_toggled(saved, String::new(), window, cx);
            }
            MessageAction::RemindMe(remind) => {
                let due = Some(remind.at(crate::drafts::now_secs()));
                let request =
                    actions::build_save_request(&self.fixtures, channel, &message, true, due);
                self.save_for_later(&request, cx);
                self.sync_saved(cx);
                notify("Reminder set", remind.label().to_lowercase(), window, cx);
            }
            MessageAction::RemindCustom => {
                let this = cx.entity().downgrade();
                let fixtures = self.fixtures.clone();
                actions::open_custom_remind_for_message(
                    &fixtures,
                    channel,
                    &message,
                    window,
                    cx,
                    move |request, when, window, cx| {
                        let _ = this.update(cx, |app, cx| {
                            app.save_for_later(&request, cx);
                            app.sync_saved(cx);
                            notify("Reminder set", when, window, cx);
                        });
                    },
                );
            }
            MessageAction::TogglePin => {
                let pinned = match surface {
                    MessageSurface::Unreads => self
                        .unreads
                        .update(cx, |u, cx| u.toggle_pin(&channel, &ts, cx)),
                    MessageSurface::Threads => self
                        .threads
                        .update(cx, |t, cx| t.toggle_pin(&channel, &ts, cx)),
                };
                if let Some(pinned) = pinned {
                    self.post_change(&channel, &MessageChange::Pin { ts, pinned });
                    actions::notify_pin_toggled(pinned, String::new(), window, cx);
                }
            }
            MessageAction::MarkUnread => match surface {
                MessageSurface::Unreads => {
                    let cursor = self.unreads.read(cx).cursor_before(&channel, &ts);
                    self.post_change(&channel, &MessageChange::MarkUnread { ts: cursor.clone() });
                    self.apply_local_mark_unread(&channel, cursor, cx);
                }
                MessageSurface::Threads => {
                    let cursor = self.threads.read(cx).cursor_before(
                        &channel,
                        &thread_ts.clone().unwrap_or_default(),
                        &ts,
                    );
                    self.mark_thread_unread(channel, thread_ts.unwrap_or_default(), cursor, cx);
                }
            },
            MessageAction::Edit => match surface {
                MessageSurface::Unreads => {
                    self.select(channel, cx);
                    self.messages.update(cx, |list, cx| list.reveal(&ts, cx));
                }
                MessageSurface::Threads => self.threads.update(cx, |t, cx| {
                    t.start_edit(channel, &message, window, cx);
                }),
            },
            MessageAction::Delete => {
                let this = cx.entity().downgrade();
                let fixtures = self.fixtures.clone();
                actions::confirm_delete(message, fixtures, window, cx, move |cx| {
                    let _ = this.update(cx, |app, cx| {
                        match surface {
                            MessageSurface::Unreads => {
                                app.unreads
                                    .update(cx, |u, cx| u.remove_message(&channel, &ts, cx));
                            }
                            MessageSurface::Threads => {
                                app.threads
                                    .update(cx, |t, cx| t.remove_message(&channel, &ts, cx));
                            }
                        }
                        app.post_change(&channel, &MessageChange::Delete { ts: ts.clone() });
                    });
                });
            }
            MessageAction::OpenProfile(user) => self.open_profile(user, cx),
            MessageAction::OpenImage(origin) => {
                let images_and_ix = match surface {
                    MessageSurface::Unreads => crate::image_viewer::collect_viewer_images(
                        &self.fixtures,
                        std::iter::once(&message),
                        &message.ts,
                        &origin,
                    ),
                    MessageSurface::Threads => self.threads.read(cx).card_images(
                        &channel,
                        &thread_ts.clone().unwrap_or_default(),
                        &ts,
                        &origin,
                    ),
                };
                if let Some((images, index)) = images_and_ix {
                    self.open_image_viewer(channel, images, index, window, cx);
                }
            }
            MessageAction::ViewHistory => {
                self.open_revision_history(channel, ts, window, cx);
            }
            MessageAction::AddToList => {
                self.open_add_to_list_dialog(channel, ts, window, cx);
            }
            MessageAction::SendToChannel => {
                // Only a thread card can broadcast its reply to the channel.
                if let (MessageSurface::Threads, Some(thread_ts)) = (surface, thread_ts) {
                    let msg = OutgoingMessage {
                        channel: channel.clone(),
                        thread_ts: Some(thread_ts),
                        broadcast: true,
                        content: crate::composer::content_of(&message.text, &self.fixtures),
                        files: None,
                        origin: OutgoingOrigin::External,
                    };
                    if self.submit(msg, window, cx) {
                        window.push_notification("Sent reply to channel", cx);
                    }
                }
            }
            MessageAction::JoinHuddle => {
                if self.channel != channel {
                    self.select(channel, cx);
                }
                self.start_huddle(window, cx);
            }
            MessageAction::BlockAction { action, attachment } => {
                if let Some(change) = actions::dispatch_block_action(
                    !self.fixtures.sample,
                    &message,
                    &channel,
                    action,
                    attachment,
                    window,
                    cx,
                ) {
                    self.post_change(&channel, &change);
                }
            }
            MessageAction::DismissEphemeral => match surface {
                MessageSurface::Unreads => {
                    self.unreads
                        .update(cx, |u, cx| u.remove_message(&channel, &ts, cx));
                }
                MessageSurface::Threads => {
                    self.threads
                        .update(cx, |t, cx| t.remove_message(&channel, &ts, cx));
                }
            },
        }
    }

    /// Open a dialog to add `(channel, ts)` as an item in a Slack List (`lists.addToList`, `F-COLLAB-231`).
    pub(super) fn open_add_to_list_dialog(
        &mut self,
        channel: ChannelId,
        ts: Ts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut lists: Vec<(String, String)> = self
            .ws
            .lists
            .iter()
            .filter_map(|(id, res)| res.as_ref().ok().map(|d| (id.clone(), d.title.clone())))
            .collect();
        for (id, title) in self.files_page.read(cx).known_lists() {
            if !lists.iter().any(|(existing, _)| existing == &id) {
                lists.push((id, title));
            }
        }
        let default_id = self
            .ws
            .active_list
            .clone()
            .or_else(|| lists.first().map(|(id, _)| id.clone()))
            .unwrap_or_default();
        let id_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("List file ID (e.g. F0123456789)")
                .default_value(default_id)
                .auto_grow(1, 1)
        });
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (border, bg, hover, muted) = (
                theme.border,
                theme.background,
                theme.accent,
                theme.muted_foreground,
            );
            let input_c = id_input.clone();
            let this_ok = this.clone();
            let ch_ok = channel.clone();
            let ts_ok = ts.clone();
            let picks = lists.clone();
            let this_pick = this.clone();
            let ch_pick = channel.clone();
            let ts_pick = ts.clone();
            dialog.title("Add message to list").w(px(400.)).child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .when(!picks.is_empty(), |col| {
                        col.child(
                            v_flex()
                                .gap_1()
                                .child(div().text_xs().text_color(muted).child("Workspace lists"))
                                .children(picks.into_iter().enumerate().map(
                                    |(ix, (lid, title))| {
                                        let this_row = this_pick.clone();
                                        let ch_row = ch_pick.clone();
                                        let ts_row = ts_pick.clone();
                                        let lid_click = lid.clone();
                                        h_flex()
                                            .id(("add-to-list-pick", ix))
                                            .items_center()
                                            .justify_between()
                                            .px_2p5()
                                            .py_1p5()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(border)
                                            .cursor_pointer()
                                            .hover(move |el| el.bg(hover))
                                            .on_click(move |_, window, cx| {
                                                window.close_dialog(cx);
                                                let _ = this_row.update(cx, |app, _| {
                                                    app.bridge.send(
                                                        CollabCommand::AddMessageToList {
                                                            list_id: lid_click.clone(),
                                                            channel: ch_row.clone(),
                                                            ts: ts_row.clone(),
                                                        },
                                                    );
                                                });
                                            })
                                            .child(div().text_sm().child(title))
                                            .child(div().text_xs().text_color(muted).child(lid))
                                    },
                                )),
                        )
                    })
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("Or enter a list ID"),
                            )
                            .child(preferences::text_input_mono(&id_input, false, cx).bg(bg)),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                preferences::form_button(
                                    Button::new("add-to-list-cancel").outline().label("Cancel"),
                                    cx,
                                )
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                preferences::form_button(
                                    Button::new("add-to-list-submit")
                                        .primary()
                                        .label("Add to list"),
                                    cx,
                                )
                                .on_click(move |_, window, cx| {
                                    let list_id = input_c.read(cx).value().trim().to_owned();
                                    if list_id.is_empty() {
                                        return;
                                    }
                                    window.close_dialog(cx);
                                    let _ = this_ok.update(cx, |app, _| {
                                        app.bridge.send(CollabCommand::AddMessageToList {
                                            list_id,
                                            channel: ch_ok.clone(),
                                            ts: ts_ok.clone(),
                                        });
                                    });
                                }),
                            ),
                    ),
            )
        });
    }
}

/// Parse `/dnd` slash-command arguments into `Some(None)` (end DND), `Some(Some(minutes))`,
/// or `None` for invalid input (`F-SHELL-032`).
pub(crate) fn parse_dnd_minutes(args: &str) -> Option<Option<u32>> {
    let trimmed = args.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Some(Some(60));
    }
    if matches!(
        trimmed.as_str(),
        "off" | "end" | "resume" | "cancel" | "stop" | "0"
    ) {
        return Some(None);
    }
    if let Ok(mins) = trimmed.parse::<u32>() {
        return (mins > 0).then(|| Some(mins.clamp(1, 1440)));
    }
    let mut total: u32 = 0;
    let mut num: u32 = 0;
    let mut saw_digit = false;
    let mut saw_unit = false;
    for ch in trimmed.chars() {
        if let Some(d) = ch.to_digit(10) {
            num = num.saturating_mul(10).saturating_add(d);
            saw_digit = true;
        } else if ch == 'h' && saw_digit {
            total = total.saturating_add(num.saturating_mul(60));
            num = 0;
            saw_digit = false;
            saw_unit = true;
        } else if ch == 'm' && saw_digit {
            total = total.saturating_add(num);
            num = 0;
            saw_digit = false;
            saw_unit = true;
        } else if ch.is_whitespace() {
            continue;
        } else {
            return None;
        }
    }
    if saw_digit && saw_unit {
        total = total.saturating_add(num);
    }
    (saw_unit && total > 0).then(|| Some(total.clamp(1, 1440)))
}

#[cfg(test)]
mod dnd_tests {
    use super::parse_dnd_minutes;

    #[test]
    fn parses_dnd_slash_command_durations() {
        assert_eq!(parse_dnd_minutes(""), Some(Some(60)));
        assert_eq!(parse_dnd_minutes("off"), Some(None));
        assert_eq!(parse_dnd_minutes("RESUME"), Some(None));
        assert_eq!(parse_dnd_minutes("30"), Some(Some(30)));
        assert_eq!(parse_dnd_minutes("45m"), Some(Some(45)));
        assert_eq!(parse_dnd_minutes("2h"), Some(Some(120)));
        assert_eq!(parse_dnd_minutes("1h30m"), Some(Some(90)));
        assert_eq!(parse_dnd_minutes("banana"), None);
    }
}
