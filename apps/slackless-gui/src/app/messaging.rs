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
            self.bridge.send(Command::DeleteMessage { channel, ts });
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
        if self.picker.is_some()
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
        self.bridge.send(match change {
            MessageChange::React { ts, name, add } => {
                let (ts, name) = (ts.clone(), name.clone());
                if *add {
                    Command::AddReaction { channel, ts, name }
                } else {
                    Command::RemoveReaction { channel, ts, name }
                }
            }
            MessageChange::Edit { ts, content } => Command::EditMessage {
                channel,
                ts: ts.clone(),
                content: content.clone(),
            },
            MessageChange::Delete { ts } => Command::DeleteMessage {
                channel,
                ts: ts.clone(),
            },
            MessageChange::Pin { ts, pinned } => Command::SetPinned {
                channel,
                ts: ts.clone(),
                pinned: *pinned,
            },
            MessageChange::MarkUnread { ts } => Command::MarkRead {
                channel,
                ts: ts.clone(),
            },
            MessageChange::BlockAction(dispatch) => Command::BlockAction {
                bot: dispatch.bot.clone(),
                app_id: dispatch.app_id.clone(),
                team: dispatch.team.clone(),
                action: dispatch.action.clone(),
                container: dispatch.container.clone(),
            },
        });
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
                self.messages.update(cx, |list, cx| {
                    list.add_reaction(&channel, &ts, &choice.name, cx)
                });
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
                self.bridge.send(Command::SetUserPref {
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
    /// Slack is told (`conversations.mark`) and the sidebar un-bolds it at once. Without a
    /// counts entry (the bundled sample workspace) the sidebar's own read state moves.
    pub(super) fn mark_read(&mut self, channel: &ChannelId, newest: Ts, cx: &mut Context<Self>) {
        if let Some(counts) = self.ws.counts.get_mut(channel) {
            if !counts.has_unreads && counts.mention_count == 0 {
                return;
            }
            counts.has_unreads = false;
            counts.mention_count = 0;
            counts.last_read = Some(newest.clone());
        }
        self.bridge.send(Command::MarkRead {
            channel: channel.clone(),
            ts: newest,
        });
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.mark_read(channel.clone(), cx));
    }

    /// Esc: mark the open conversation read up to its newest message - or, while the All
    /// Unreads view is open, the group the keyboard focused.
    pub(super) fn mark_open_read(&mut self, cx: &mut Context<Self>) {
        if self.unreads_open {
            self.unreads.update(cx, |view, cx| {
                view.handle_key("escape", cx);
            });
            return;
        }
        let channel = self.channel.clone();
        if let Some(newest) = self.messages.read(cx).newest_ts() {
            self.mark_read(&channel, newest, cx);
        }
    }

    /// Shift+Esc: Slack marks every conversation and thread read (`users.markAllRead`); the
    /// sidebar un-bolds at once and the core's fresh counts follow.
    pub(super) fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        self.bridge.send(Command::MarkAllRead);
        for counts in self.ws.counts.values_mut() {
            counts.has_unreads = false;
            counts.mention_count = 0;
        }
        let unread = self.sidebar.read(cx).unread_ids();
        self.sidebar.update(cx, |sidebar, cx| {
            for channel in unread {
                sidebar.mark_read(channel, cx);
            }
        });
        if self.unreads_open {
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
            self.bridge.send(Command::SyncDraft {
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

    /// Queue a message (in a channel or inside `thread_ts`) for delivery at unix timestamp
    /// `post_at`, recording it in Drafts & sent and posting `chat.scheduleMessage` live.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn schedule_at(
        &mut self,
        channel: ChannelId,
        thread_ts: Option<Ts>,
        text: String,
        post_at: u64,
        when_label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.fixtures.sample {
            let content = composer::content_of(&text, &self.fixtures);
            self.bridge.send(Command::ScheduleMessage {
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
        notify(
            "Message scheduled",
            format!("Sends {when_label}. See Drafts & sent."),
            window,
            cx,
        );
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
            self.bridge.send(Command::DeleteScheduledMessage {
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
        self.bridge.send(Command::SendMessage {
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
            self.bridge.send(Command::RunCommand {
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
                let text = if invocation.args.is_empty() {
                    SHRUG.to_owned()
                } else {
                    format!("{} {SHRUG}", invocation.args)
                };
                self.send(MessageContent::plain(text), window, cx);
            }
            SlashKind::Away => {
                self.self_state.away = !self.self_state.away;
                if !self.fixtures.sample {
                    self.bridge.send(Command::SetPresence {
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
            kind => {
                eprintln!(
                    "slackless: slash command {kind:?} /{} args={:?} in {} (fixture mode: not run)",
                    command.name, invocation.args, self.channel
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

    /// Slack's broadcast confirmation (`F-CORE-083`): warn before posting `@channel`, `@here`,
    /// or `@everyone` to a channel with 6 or more members.
    pub(super) fn send(
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
                                        Button::new("broadcast-cancel").label("Cancel").on_click(
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
                                        Button::new("broadcast-send")
                                            .primary()
                                            .label("Send")
                                            .on_click(move |_, window, cx| {
                                                window.close_dialog(cx);
                                                let _ = this_send.update(cx, |app, cx| {
                                                    app.deliver_send(send_content.clone(), cx);
                                                });
                                            }),
                                    ),
                            ),
                    )
                });
                return;
            }
        }
        self.deliver_send(content, cx);
    }

    pub(super) fn deliver_send(&mut self, mut content: MessageContent, cx: &mut Context<Self>) {
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
        self.bridge.send(Command::SendMessage {
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
            self.bridge.send(Command::UploadFiles {
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
}
