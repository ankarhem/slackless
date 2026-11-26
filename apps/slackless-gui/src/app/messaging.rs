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
            let channel = self.channel.clone();
            self.messages.update(cx, |list, cx| {
                list.delete(&ts, cx);
            });
            self.bridge
                .send(MessagingCommand::DeleteMessage { channel, ts });
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
        if let Some(write) = self.bridge.remember_frequent_emoji(&frequent) {
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
        if !self.transcript_visible(cx) {
            return;
        }
        let settings = self.preferences.read(cx).settings().clone();
        let allow_mark =
            settings.mark_read != 2 && (!settings.no_bg_mark_read || self.window_active);
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
            .is_some_and(|prev| *prev == ts);
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
    }

    /// Auto-mark every currently unread card in `ThreadsView` read on Slack when the view opens,
    /// keeping the in-card `"New"` divider for the current visit while clearing badges immediately.
    pub(super) fn auto_mark_threads_view_read(&mut self, cx: &mut Context<Self>) {
        let targets = self
            .threads
            .update(cx, |threads, cx| threads.clear_all_unreads(false, cx));
        for (channel, thread_ts, ts) in targets {
            let key = (channel.clone(), thread_ts.clone());
            if self.ws.marked_threads.get(&key) != Some(&ts) {
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
    }

    /// Explicitly mark all unread threads read (`Esc` in Threads view or "Mark all read" button).
    pub(super) fn mark_all_threads_read(&mut self, cx: &mut Context<Self>) {
        let targets = self
            .threads
            .update(cx, |threads, cx| threads.clear_all_unreads(true, cx));
        for (channel, thread_ts, ts) in targets {
            let key = (channel.clone(), thread_ts.clone());
            if self.ws.marked_threads.get(&key) != Some(&ts) {
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
    }

    /// Clear the sidebar's Threads unread indicator when `ThreadsView` has the remote list and
    /// no unread cards remain.
    pub(super) fn sync_thread_sidebar_counts(&mut self, cx: &mut Context<Self>) {
        let threads = self.threads.read(cx);
        if threads.is_remote() && !threads.has_unreads() {
            self.sidebar
                .update(cx, |s, cx| s.set_thread_counts(false, 0, cx));
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
        let saved: Vec<SavedDraft> = self
            .drafts
            .read(cx)
            .drafts_for(self.workspace)
            .into_iter()
            .map(|entry| SavedDraft {
                channel: entry.channel,
                label: entry.label,
                text: entry.text,
                at: entry.at,
            })
            .collect();
        if let Some(write) = self.bridge.remember_drafts(&saved) {
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
                thread_ts,
            });
        }
        let entry = Entry {
            id: None,
            workspace: self.workspace,
            channel: channel.clone(),
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

    /// Deliver a scheduled message now; it shows in the transcript when its workspace is the
    /// shown one (a parked workspace's transcript is not touched).
    pub(super) fn deliver(
        &mut self,
        mut entry: Entry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = entry.id.take() {
            self.bridge.send(MessagingCommand::DeleteScheduledMessage {
                channel: entry.channel.clone(),
                scheduled_message_id: id,
            });
        }
        // A scheduled message is kept as its draft text; it posts as the draft would have.
        let content = composer::content_of(&entry.text, &self.fixtures);
        if entry.workspace == self.workspace {
            let mut message =
                self.fixtures
                    .local_message(&entry.channel, content.text.clone(), None);
            message.blocks = content.blocks.clone();
            let channel = entry.channel.clone();
            self.messages
                .update(cx, |list, cx| list.append(channel, message, cx));
        }
        self.bridge.send(MessagingCommand::SendMessage {
            channel: entry.channel.clone(),
            content,
            thread_ts: None,
            broadcast: false,
        });
        notify(
            "Message sent",
            format!("Delivered to {}", entry.label),
            window,
            cx,
        );
        entry.at = now_secs();
        self.drafts
            .update(cx, |drafts, cx| drafts.record_sent(entry, cx));
    }

    /// Dispatch a slash command typed in the main composer, or in the reply composer of
    /// `thread`. Live mode hands `/giphy` and commands slackless does not know to Slack
    /// (`chat.command`; the result arrives on the websocket). Fixture mode runs the ones that
    /// only touch local state (/shrug, /away, /status) and logs the rest with a toast.
    pub(super) fn run_slash(
        &mut self,
        invocation: &Invocation,
        thread: Option<(ChannelId, Ts)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let remote = invocation
            .command
            .is_none_or(|command| command.kind == SlashKind::Giphy);
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
            });
            return;
        }
        let Some(command) = invocation.command else {
            notify(
                "Unknown command",
                format!("/{} is not a valid command.", invocation.name),
                window,
                cx,
            );
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

    /// Slack's broadcast confirmation (`F-CORE-083`) plus local pre-send secret leak guard.
    pub(super) fn send(
        &mut self,
        content: MessageContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .preferences
            .read(cx)
            .settings()
            .warn_before_sending_secrets
        {
            let matches = slackless_core::detect_secrets(&content.text);
            if !matches.is_empty() {
                self.confirm_secret_send(
                    matches,
                    content.text.clone(),
                    true,
                    move |app, window, cx| {
                        app.send_after_secret_check(content.clone(), window, cx);
                    },
                    window,
                    cx,
                );
                return;
            }
        }
        self.send_after_secret_check(content, window, cx);
    }

    /// Show a confirmation dialog when outgoing text matches known API keys, tokens, or private
    /// keys. Canceling restores `cancel_text` into the main composer when `restore_main_composer`
    /// is true; "Send anyway" invokes `proceed`.
    pub(super) fn confirm_secret_send(
        &self,
        matches: Vec<slackless_core::SecretMatch>,
        cancel_text: String,
        restore_main_composer: bool,
        proceed: impl Fn(&mut App, &mut Window, &mut Context<App>) + Clone + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let summary_lines: Vec<String> = matches
            .iter()
            .map(|m| format!("\u{2022} {}: {}", m.kind, m.preview))
            .collect();
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (muted, danger, border, card_bg, mono) = (
                theme.muted_foreground,
                theme.danger,
                theme.border,
                theme.secondary.opacity(0.35),
                theme.mono_font_family.clone(),
            );
            let this_cancel = this.clone();
            let this_send = this.clone();
            let cancel_text = cancel_text.clone();
            let proceed = proceed.clone();
            let items: Vec<_> = summary_lines
                .iter()
                .map(|line| {
                    div()
                        .font_family(mono.clone())
                        .text_xs()
                        .text_color(danger)
                        .child(line.clone())
                })
                .collect();
            dialog
                .title("Possible secret detected")
                .w(px(480.))
                .child(
                    v_flex()
                        .gap_3()
                        .child(div().text_sm().text_color(muted).child(
                            "This message looks like it contains credentials or a private key. Are you sure you want to send it?",
                        ))
                        .child(
                            v_flex()
                                .gap_1()
                                .p_2p5()
                                .rounded_md()
                                .border_1()
                                .border_color(border)
                                .bg(card_bg)
                                .children(items),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .justify_end()
                                .child(
                                    preferences::form_button(
                                        Button::new("secret-guard-cancel")
                                            .outline()
                                            .label("Cancel"),
                                        cx,
                                    )
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        if restore_main_composer {
                                            let _ = this_cancel.update(cx, |app, cx| {
                                                app.composer.update(cx, |composer, cx| {
                                                    composer.load_draft(&cancel_text, cx);
                                                });
                                            });
                                        }
                                    }),
                                )
                                .child(
                                    preferences::form_button(
                                        Button::new("secret-guard-send")
                                            .danger()
                                            .label("Send anyway"),
                                        cx,
                                    )
                                    .on_click(move |_, window, cx| {
                                            window.close_dialog(cx);
                                            let _ = this_send.update(cx, |app, cx| {
                                                proceed(app, window, cx);
                                            });
                                        }),
                                ),
                        ),
                )
        });
    }

    fn send_after_secret_check(
        &mut self,
        content: MessageContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        const BROADCAST_WARN_THRESHOLD: usize = 6;
        if let Some(range) = broadcast_kind(&content) {
            let conv = self.fixtures.conversation(&self.channel);
            let is_channel = conv.is_some_and(|c| {
                matches!(
                    c.kind(),
                    ConversationKind::PublicChannel | ConversationKind::PrivateChannel
                )
            });
            let members = self
                .ws
                .member_counts
                .get(&self.channel)
                .map(|&n| n as usize)
                .or_else(|| conv.and_then(|c| c.num_members.map(|n| n as usize)))
                .or_else(|| conv.map(|c| c.members.len()))
                .unwrap_or(0);
            if is_channel && members >= BROADCAST_WARN_THRESHOLD {
                let channel_name = conv
                    .and_then(|c| c.name.clone())
                    .unwrap_or_else(|| self.channel.to_string());
                let title = match range {
                    BroadcastRange::Here => format!("Notify active members in #{channel_name}?"),
                    _ => format!("Notify everyone in #{channel_name}?"),
                };
                let body = format!(
                    "By using @{range}, you\u{2019}re about to notify {members} people in #{channel_name}. Are you sure you want to continue?"
                );
                let this = cx.weak_entity();
                let send_content = content.clone();
                let cancel_text = content.text.clone();
                window.open_dialog(cx, move |dialog, _, cx| {
                    let muted = cx.theme().muted_foreground;
                    let this_cancel = this.clone();
                    let this_send = this.clone();
                    let send_content = send_content.clone();
                    let cancel_text = cancel_text.clone();
                    dialog.title(title.clone()).w(px(460.)).child(
                        v_flex()
                            .gap_4()
                            .child(div().text_color(muted).child(body.clone()))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_end()
                                    .child(
                                        preferences::form_button(
                                            Button::new("broadcast-cancel")
                                                .outline()
                                                .label("Cancel"),
                                            cx,
                                        )
                                        .on_click(
                                            move |_, window, cx| {
                                                window.close_dialog(cx);
                                                let _ = this_cancel.update(cx, |app, cx| {
                                                    app.composer.update(cx, |composer, cx| {
                                                        composer.load_draft(&cancel_text, cx);
                                                    });
                                                });
                                            },
                                        ),
                                    )
                                    .child(
                                        preferences::form_button(
                                            Button::new("broadcast-send").primary().label("Send"),
                                            cx,
                                        )
                                        .on_click(
                                            move |_, window, cx| {
                                                window.close_dialog(cx);
                                                let _ = this_send.update(cx, |app, cx| {
                                                    app.deliver_send(send_content.clone(), cx);
                                                });
                                            },
                                        ),
                                    ),
                            ),
                    )
                });
                return;
            }
        }
        self.deliver_send(content, cx);
    }

    /// Strip tracking parameters (`utm_*`, `fbclid`, `gclid`, etc.) from outgoing text and
    /// rich-text link elements when enabled in Preferences.
    pub(super) fn sanitize_outgoing_content(
        &self,
        content: &mut MessageContent,
        cx: &Context<Self>,
    ) {
        if self.preferences.read(cx).settings().strip_tracking_params {
            crate::composer::strip_tracking_params_in_content(content);
        }
    }

    /// Send a thread reply with pre-send secret leak guard and tracking-param stripping.
    pub(super) fn send_thread_reply(
        &mut self,
        target: (ChannelId, Ts),
        mut content: MessageContent,
        broadcast: bool,
        sync_threads_view: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (channel, thread_ts) = target;
        if self
            .preferences
            .read(cx)
            .settings()
            .warn_before_sending_secrets
        {
            let matches = slackless_core::detect_secrets(&content.text);
            if !matches.is_empty() {
                self.confirm_secret_send(
                    matches,
                    content.text.clone(),
                    false,
                    move |app, _window, cx| {
                        let mut c = content.clone();
                        app.sanitize_outgoing_content(&mut c, cx);
                        app.bridge.send(MessagingCommand::SendMessage {
                            channel: channel.clone(),
                            content: c,
                            thread_ts: Some(thread_ts.clone()),
                            broadcast,
                        });
                        if sync_threads_view {
                            app.sync_thread_sidebar_counts(cx);
                        }
                    },
                    window,
                    cx,
                );
                return;
            }
        }
        self.sanitize_outgoing_content(&mut content, cx);
        self.bridge.send(MessagingCommand::SendMessage {
            channel,
            content,
            thread_ts: Some(thread_ts),
            broadcast,
        });
        if sync_threads_view {
            self.sync_thread_sidebar_counts(cx);
        }
    }

    pub(super) fn deliver_send(&mut self, mut content: MessageContent, cx: &mut Context<Self>) {
        self.sanitize_outgoing_content(&mut content, cx);
        if self.preferences.read(cx).settings().convert_emoticons && !content.text.is_empty() {
            let converted = convert_emoticons(&content.text);
            if converted != content.text {
                content.text = converted;
            }
        }
        let channel = self.channel.clone();
        // The local echo shows the same blocks & attachments the message posts with.
        let mut message = self
            .fixtures
            .local_message(&channel, content.text.clone(), None);
        message.blocks = content.blocks.clone();
        message.attachments = content.attachments.clone();
        self.messages
            .update(cx, |list, cx| list.append(channel.clone(), message, cx));
        let sent_text = if content.text.is_empty() {
            content
                .attachments
                .first()
                .and_then(|a| a.fallback.clone())
                .unwrap_or_default()
        } else {
            content.text.clone()
        };
        let entry = Entry {
            id: None,
            workspace: self.workspace,
            channel: channel.clone(),
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
        let files = valid_files;
        if self.bridge.store().is_some() {
            let count = files.len();
            let uploads = files
                .into_iter()
                .map(|f| slackless_core::api::UploadFile {
                    filename: f.filename,
                    bytes: f.bytes,
                })
                .collect();
            self.bridge.send(CollabCommand::UploadFiles {
                channel,
                thread_ts,
                content,
                files: uploads,
            });
            let label = if count == 1 { "file" } else { "files" };
            notify(
                "Uploading",
                format!("Uploading {count} {label}\u{2026}"),
                window,
                cx,
            );
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
        self.save_draft(cx);
    }

    /// Dispatch a message-level action triggered from an unread row's hover toolbar, reaction
    /// chips, image click, or context menu (`UnreadsEvent::MessageAction`).
    pub(super) fn handle_unreads_message_action(
        &mut self,
        channel: ChannelId,
        message: Message,
        action: crate::actions::MessageAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::actions::MessageAction;
        let ts = message.ts.clone();
        match action {
            MessageAction::React(name) | MessageAction::ToggleReaction(name) => {
                if let Some(add) = self
                    .unreads
                    .update(cx, |u, cx| u.toggle_reaction(&channel, &ts, &name, cx))
                {
                    self.post_change(
                        &channel,
                        &MessageChange::React {
                            ts: ts.clone(),
                            name: name.clone(),
                            add,
                        },
                    );
                }
            }
            MessageAction::PickReaction(position) => {
                self.open_picker(PickerTarget::Message { channel, ts }, position, window, cx);
            }
            MessageAction::ReplyInThread => {
                let thread_ts = message.thread_ts.clone().unwrap_or(ts);
                self.open_thread(channel, thread_ts, cx);
            }
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
                if let Some(pinned) = self
                    .unreads
                    .update(cx, |u, cx| u.toggle_pin(&channel, &ts, cx))
                {
                    self.post_change(&channel, &MessageChange::Pin { ts, pinned });
                    actions::notify_pin_toggled(pinned, String::new(), window, cx);
                }
            }
            MessageAction::MarkUnread => {
                self.post_change(&channel, &MessageChange::MarkUnread { ts: ts.clone() });
                self.apply_local_mark_unread(&channel, ts, cx);
                notify("Marked unread", String::new(), window, cx);
            }
            MessageAction::Edit => {
                self.select(channel, cx);
                self.messages.update(cx, |list, cx| list.reveal(&ts, cx));
            }
            MessageAction::Delete => {
                let this = cx.entity().downgrade();
                let fixtures = self.fixtures.clone();
                actions::confirm_delete(message, fixtures, window, cx, move |cx| {
                    let _ = this.update(cx, |app, cx| {
                        app.unreads
                            .update(cx, |u, cx| u.remove_message(&channel, &ts, cx));
                        app.post_change(&channel, &MessageChange::Delete { ts: ts.clone() });
                    });
                });
            }
            MessageAction::OpenProfile(user) => self.open_profile(user, cx),
            MessageAction::OpenImage(origin) => {
                if let Some((images, index)) = crate::image_viewer::collect_viewer_images(
                    &self.fixtures,
                    std::iter::once(&message),
                    &message.ts,
                    &origin,
                ) {
                    self.open_image_viewer(channel, images, index, window, cx);
                }
            }
            MessageAction::ViewHistory => {
                self.open_revision_history(channel, ts, window, cx);
            }
            MessageAction::AddToList => {
                self.open_add_to_list_dialog(channel, ts, window, cx);
            }
            MessageAction::SendToChannel => {}
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
            MessageAction::DismissEphemeral => {
                self.unreads
                    .update(cx, |u, cx| u.remove_message(&channel, &ts, cx));
            }
        }
    }

    /// Dispatch a message-level action triggered from a thread card message's hover toolbar,
    /// reaction chips, image click, Block Kit action, or context menu (`ThreadsEvent::MessageAction`).
    pub(super) fn handle_threads_message_action(
        &mut self,
        channel: ChannelId,
        thread_ts: Ts,
        message: Message,
        action: crate::actions::MessageAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::actions::MessageAction;
        let ts = message.ts.clone();
        match action {
            MessageAction::React(name) => {
                if self
                    .threads
                    .update(cx, |t, cx| t.add_reaction(&channel, &ts, &name, cx))
                {
                    self.post_change(
                        &channel,
                        &MessageChange::React {
                            ts: ts.clone(),
                            name,
                            add: true,
                        },
                    );
                }
            }
            MessageAction::ToggleReaction(name) => {
                if let Some(add) = self
                    .threads
                    .update(cx, |t, cx| t.toggle_reaction(&channel, &ts, &name, cx))
                {
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
            MessageAction::ReplyInThread => {
                self.threads.update(cx, |t, cx| {
                    t.activate_thread(&channel, &thread_ts, window, cx)
                });
            }
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
                if let Some(pinned) = self
                    .threads
                    .update(cx, |t, cx| t.toggle_pin(&channel, &ts, cx))
                {
                    self.post_change(&channel, &MessageChange::Pin { ts, pinned });
                    actions::notify_pin_toggled(pinned, String::new(), window, cx);
                }
            }
            MessageAction::MarkUnread => {
                let cursor = self
                    .threads
                    .read(cx)
                    .cursor_before(&channel, &thread_ts, &ts);
                self.mark_thread_unread(channel, thread_ts, cursor, cx);
                notify("Marked unread", String::new(), window, cx);
            }
            MessageAction::Edit => {
                self.threads.update(cx, |t, cx| {
                    t.start_edit(channel, &message, window, cx);
                });
            }
            MessageAction::Delete => {
                let this = cx.entity().downgrade();
                let fixtures = self.fixtures.clone();
                actions::confirm_delete(message, fixtures, window, cx, move |cx| {
                    let _ = this.update(cx, |app, cx| {
                        app.threads
                            .update(cx, |t, cx| t.remove_message(&channel, &ts, cx));
                        app.post_change(&channel, &MessageChange::Delete { ts: ts.clone() });
                    });
                });
            }
            MessageAction::OpenProfile(user) => self.open_profile(user, cx),
            MessageAction::OpenImage(origin) => {
                let images_and_ix = self
                    .threads
                    .read(cx)
                    .card_images(&channel, &thread_ts, &ts, &origin);
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
                let content = crate::composer::content_of(&message.text, &self.fixtures);
                self.bridge.send(MessagingCommand::SendMessage {
                    channel,
                    content,
                    thread_ts: Some(thread_ts),
                    broadcast: true,
                });
                window.push_notification("Sent reply to channel", cx);
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
            MessageAction::DismissEphemeral => {
                self.threads
                    .update(cx, |t, cx| t.remove_message(&channel, &ts, cx));
            }
            MessageAction::JoinHuddle => {
                if self.channel != channel {
                    self.select(channel, cx);
                }
                self.start_huddle(window, cx);
            }
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
