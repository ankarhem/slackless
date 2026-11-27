//! Message actions, inline editing, emoji picker, image viewer, and message/file sending for
//! popped-out secondary windows.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::input::TextareaState;
use gpui_kit::gpui::{
    App, AppContext as _, ClipboardItem, Context, Focusable as _, Pixels, Point, Window,
};
use slackless_core::api::MessageContent;
use slackless_core::types::{ChannelId, Message, Ts};
use slackless_core::{CollabCommand, MessagingCommand};

use crate::actions::{self, MessageAction, MessageChange};
use crate::composer;
use crate::emoji::{EmojiChoice, PickerTab};
use crate::image_viewer::{self, ImageOrigin, ImageViewer, ViewerEvent};
use crate::later::SaveRequest;

use super::{JumpHandler, PopoutKind, PopoutPickerTarget, PopoutWindow};

/// A popped-out window's send on its way through the main window's checks.
struct Outgoing {
    content: MessageContent,
    /// Staged files the message uploads with (empty for a plain message).
    files: Vec<slackless_core::api::UploadFile>,
    /// A thread reply posted to its channel too ("Send to channel"), not the composer's text.
    broadcast: bool,
}

impl PopoutWindow {
    pub(super) fn open_picker(
        &mut self,
        target: PopoutPickerTarget,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(emoji_picker) = &self.emoji_picker else {
            return;
        };
        emoji_picker.update(cx, |picker, cx| {
            picker.open_tab(PickerTab::Emoji, false, window, cx);
        });
        self.picker = Some((target, position));
        cx.notify();
    }

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
            PopoutPickerTarget::Composer => {
                if let Some(composer) = &self.composer {
                    composer.update(cx, |c, cx| c.insert_emoji(choice, window, cx));
                }
            }
            PopoutPickerTarget::Message { channel, ts } => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.ts == ts)
                    && actions::add_reaction(msg, &self.fixtures.self_user, &choice.name)
                {
                    (self.send)(
                        MessagingCommand::AddReaction {
                            channel,
                            ts,
                            name: choice.name.clone(),
                        }
                        .into(),
                    );
                }
            }
        }
        cx.notify();
    }

    pub(super) fn start_edit(
        &mut self,
        channel: ChannelId,
        message: &Message,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Seed with raw mrkdwn/rich_text source rather than plain_text so formatting & mentions
        // are never stripped during inline editing.
        let initial = crate::editor::from_rich_text(&message.blocks, &*self.fixtures)
            .map(|doc| doc.to_mrkdwn())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| message.text.clone());
        let input = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("Edit message…")
                .auto_grow(1, 6);
            state.set_value(initial, window, cx);
            state
        });
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.editing = Some((channel, message.ts.clone(), input));
        cx.notify();
    }

    pub(super) fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some((channel, ts, input)) = self.editing.take() else {
            return;
        };
        let text = input.read(cx).value().trim().to_owned();
        if text.is_empty() {
            cx.notify();
            return;
        }
        let content = composer::content_of(&text, &self.fixtures);
        if let Some(msg) = self.messages.iter_mut().find(|m| m.ts == ts) {
            actions::apply_edit(msg, &self.fixtures.self_user, content.clone());
        }
        (self.send)(
            MessagingCommand::EditMessage {
                channel,
                ts,
                content,
            }
            .into(),
        );
        cx.notify();
    }

    fn sync_later_item(
        &self,
        channel: &ChannelId,
        message: &Message,
        saved: bool,
        due: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let Some((weak_later, workspace)) = &self.later_sync else {
            return;
        };
        let Some(later) = weak_later.upgrade() else {
            return;
        };
        let author = message
            .user
            .as_ref()
            .map(|u| self.fixtures.user_name(u).to_owned())
            .or_else(|| message.username.clone())
            .unwrap_or_else(|| "Message".to_owned());
        let text = self.fixtures.preview(message);
        let request = SaveRequest {
            channel: channel.clone(),
            ts: message.ts.clone(),
            author,
            text,
            saved,
            due,
        };
        let label = self.title.clone();
        let workspace = *workspace;
        later.update(cx, |later, cx| later.save(request, workspace, label, cx));
    }

    pub(super) fn jump_handler(&self) -> JumpHandler {
        match &self.kind {
            PopoutKind::Conversation { on_jump, .. }
            | PopoutKind::Thread { on_jump, .. }
            | PopoutKind::Activity { on_jump, .. }
            | PopoutKind::Later { on_jump, .. }
            | PopoutKind::Canvas { on_jump, .. }
            | PopoutKind::Files { on_jump, .. }
            | PopoutKind::Huddle { on_jump, .. } => on_jump.clone(),
        }
    }

    fn open_image_viewer(
        &mut self,
        ts: &Ts,
        origin: ImageOrigin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((images, index)) =
            image_viewer::collect_viewer_images(&self.fixtures, &self.messages, ts, &origin)
        else {
            return;
        };
        let viewer = ImageViewer::new(
            self.fixtures.clone(),
            self.title.clone(),
            images,
            index,
            window,
            cx,
        );
        self._viewer_sub = Some(cx.subscribe_in(
            &viewer,
            window,
            |this: &mut Self, _, event: &ViewerEvent, window, cx| {
                if matches!(event, ViewerEvent::Close) {
                    this.close_image_viewer(window, cx);
                }
            },
        ));
        self.image_viewer = Some(viewer);
        cx.notify();
    }

    pub(super) fn close_image_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(viewer) = self.image_viewer.take() {
            viewer.update(cx, |v, cx| v.evict_assets(window, cx));
            self._viewer_sub = None;
            cx.notify();
        }
    }

    pub(super) fn handle_message_action(
        &mut self,
        channel: ChannelId,
        message: Message,
        action: MessageAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ts = message.ts.clone();
        match action {
            MessageAction::React(name) => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.ts == ts)
                    && actions::add_reaction(msg, &self.fixtures.self_user, &name)
                {
                    (self.send)(MessagingCommand::AddReaction { channel, ts, name }.into());
                    cx.notify();
                }
            }
            MessageAction::ToggleReaction(name) => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.ts == ts) {
                    let add = actions::toggle_reaction(msg, &self.fixtures.self_user, &name);
                    if add {
                        (self.send)(MessagingCommand::AddReaction { channel, ts, name }.into());
                    } else {
                        (self.send)(MessagingCommand::RemoveReaction { channel, ts, name }.into());
                    }
                    cx.notify();
                }
            }
            MessageAction::PickReaction(position) => {
                self.open_picker(
                    PopoutPickerTarget::Message { channel, ts },
                    position,
                    window,
                    cx,
                );
            }
            MessageAction::ReplyInThread => {
                let thread_ts = message.thread_ts.clone().unwrap_or(ts);
                let jump = self.jump_handler();
                jump(channel, Some(thread_ts), true, cx);
            }
            MessageAction::CopyLink => {
                actions::copy_link(&self.fixtures, &channel, &ts, window, cx);
            }
            MessageAction::CopyText => {
                actions::copy_message_text(&self.fixtures, &message, window, cx);
            }
            MessageAction::Forward => {
                let link = actions::permalink(&self.fixtures, &channel, &ts);
                cx.write_to_clipboard(ClipboardItem::new_string(link));
                actions::notify(
                    "Link copied",
                    "Paste in any channel to forward this message.",
                    window,
                    cx,
                );
            }
            MessageAction::SaveForLater => {
                let saved = self.saved.insert(ts.clone());
                if saved {
                    (self.send)(
                        MessagingCommand::SaveItem {
                            channel: channel.clone(),
                            ts: ts.clone(),
                            date_due: None,
                        }
                        .into(),
                    );
                } else {
                    self.saved.remove(&ts);
                    (self.send)(
                        MessagingCommand::DeleteSavedItem {
                            channel: channel.clone(),
                            ts: ts.clone(),
                        }
                        .into(),
                    );
                }
                self.sync_later_item(&channel, &message, saved, None, cx);
                actions::notify_saved_toggled(saved, String::new(), window, cx);
                cx.notify();
            }
            MessageAction::RemindMe(remind) => {
                let due = remind.at(crate::drafts::now_secs());
                self.saved.insert(ts.clone());
                (self.send)(
                    MessagingCommand::SaveItem {
                        channel: channel.clone(),
                        ts,
                        date_due: Some(due),
                    }
                    .into(),
                );
                self.sync_later_item(&channel, &message, true, Some(due), cx);
                actions::notify("Reminder set", remind.label().to_lowercase(), window, cx);
                cx.notify();
            }
            MessageAction::RemindCustom => {
                let this = cx.entity().downgrade();
                let fixtures = self.fixtures.clone();
                let msg_clone = message.clone();
                actions::open_custom_remind_for_message(
                    &fixtures,
                    channel.clone(),
                    &message,
                    window,
                    cx,
                    move |request, when, window, cx| {
                        let _ = this.update(cx, |win, cx| {
                            let due = request.due;
                            win.saved.insert(request.ts.clone());
                            (win.send)(
                                MessagingCommand::SaveItem {
                                    channel: request.channel.clone(),
                                    ts: request.ts,
                                    date_due: due,
                                }
                                .into(),
                            );
                            win.sync_later_item(&request.channel, &msg_clone, true, due, cx);
                            actions::notify("Reminder set", when, window, cx);
                            cx.notify();
                        });
                    },
                );
            }
            MessageAction::TogglePin => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.ts == ts) {
                    let pinned = actions::toggle_pin(msg, &channel);
                    (self.send)(
                        MessagingCommand::SetPinned {
                            channel,
                            ts,
                            pinned,
                        }
                        .into(),
                    );
                    actions::notify_pin_toggled(pinned, String::new(), window, cx);
                    cx.notify();
                }
            }
            MessageAction::MarkUnread => {
                let cursor = self
                    .messages
                    .iter()
                    .take_while(|m| m.ts < ts)
                    .last()
                    .map(|m| m.ts.clone())
                    .unwrap_or_else(|| crate::threads_cards::ts_just_before(&ts));
                match &self.kind {
                    PopoutKind::Thread { thread_ts, .. } => {
                        (self.send)(
                            MessagingCommand::MarkThreadRead {
                                channel,
                                thread_ts: thread_ts.clone(),
                                ts: cursor,
                            }
                            .into(),
                        );
                    }
                    _ => {
                        (self.send)(
                            MessagingCommand::MarkRead {
                                channel,
                                ts: cursor,
                            }
                            .into(),
                        );
                    }
                }
                actions::notify("Marked unread", String::new(), window, cx);
            }
            MessageAction::Edit => {
                self.start_edit(channel, &message, window, cx);
            }
            MessageAction::ViewHistory => {
                let jump = self.jump_handler();
                jump(channel, Some(ts), false, cx);
            }
            MessageAction::Delete => {
                let this = cx.entity().downgrade();
                let fixtures = self.fixtures.clone();
                actions::confirm_delete(message, fixtures, window, cx, move |cx| {
                    let _ = this.update(cx, |win, cx| {
                        win.messages.retain(|m| m.ts != ts);
                        (win.send)(
                            MessagingCommand::DeleteMessage {
                                channel: channel.clone(),
                                ts: ts.clone(),
                            }
                            .into(),
                        );
                        cx.notify();
                    });
                });
            }
            MessageAction::OpenProfile(_) => {
                let jump = self.jump_handler();
                jump(channel, Some(ts), false, cx);
            }
            MessageAction::OpenImage(origin) => {
                self.open_image_viewer(&ts, origin, window, cx);
            }
            MessageAction::BlockAction { action, attachment } => {
                if let Some(MessageChange::BlockAction(dispatch)) = actions::dispatch_block_action(
                    !self.fixtures.sample,
                    &message,
                    &channel,
                    action,
                    attachment,
                    window,
                    cx,
                ) {
                    (self.send)(
                        CollabCommand::BlockAction {
                            bot: dispatch.bot,
                            app_id: dispatch.app_id,
                            team: dispatch.team,
                            action: dispatch.action,
                            container: dispatch.container,
                        }
                        .into(),
                    );
                }
            }
            MessageAction::DismissEphemeral => {
                self.messages.retain(|m| m.ts != ts);
                cx.notify();
            }
            MessageAction::AddToList => {}
            MessageAction::JoinHuddle => {}
            MessageAction::SendToChannel => {
                if matches!(self.kind, PopoutKind::Thread { .. }) {
                    let outgoing = Outgoing {
                        content: crate::composer::content_of(&message.text, &self.fixtures),
                        files: Vec::new(),
                        broadcast: true,
                    };
                    self.send_guarded(outgoing, Some(window), cx);
                }
            }
        }
    }

    pub(super) fn send_content_guarded(
        &mut self,
        content: MessageContent,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let outgoing = Outgoing {
            content,
            files: Vec::new(),
            broadcast: false,
        };
        self.send_guarded(outgoing, window, cx);
    }

    /// Send `outgoing` after the main window's checks: the outgoing transforms, the secret
    /// warning and the `@channel` confirmation (`F-CORE-083`).
    fn send_guarded(
        &mut self,
        mut outgoing: Outgoing,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        composer::apply_outgoing_transforms(&mut outgoing.content, &self.fixtures, cx);
        let Some(window) = window else {
            self.deliver(outgoing, cx);
            return;
        };
        if composer::outgoing_guard_prefs(cx).warn_before_sending_secrets {
            let matches = slackless_core::detect_secrets(&outgoing.content.text);
            if !matches.is_empty() {
                let (this_send, this_cancel) = (cx.entity().downgrade(), cx.entity().downgrade());
                let pending = Rc::new(Cell::new(Some(outgoing)));
                let pending_send = Rc::clone(&pending);
                composer::confirm_secret_send_dialog(
                    matches,
                    window,
                    cx,
                    move |window, cx| {
                        if let Some(outgoing) = pending_send.take() {
                            let _ = this_send
                                .update(cx, |win, cx| win.guard_broadcast(outgoing, window, cx));
                        }
                    },
                    move |_, cx| {
                        if let Some(outgoing) = pending.take() {
                            let _ = this_cancel
                                .update(cx, |win, cx| win.restore_outgoing_draft(outgoing, cx));
                        }
                    },
                );
                return;
            }
        }
        self.guard_broadcast(outgoing, window, cx);
    }

    /// Put a canceled pop-out send's text and staged files back into the composer (unless it was
    /// a broadcast repost of an existing reply, which was never typed in the composer).
    fn restore_outgoing_draft(&mut self, outgoing: Outgoing, cx: &mut Context<Self>) {
        if outgoing.broadcast {
            return;
        }
        if let Some(composer) = &self.composer {
            let files: Vec<composer::PendingAttachment> = outgoing
                .files
                .into_iter()
                .map(|f| composer::PendingAttachment::new(f.filename, f.bytes.to_vec(), None))
                .collect();
            composer.update(cx, |composer, cx| {
                composer.load_draft(&outgoing.content.text, cx);
                if !files.is_empty() {
                    composer.restore_attachments(files, cx);
                }
            });
        }
    }

    /// [`Self::send_guarded`] once the text holds no secret: ask before a broadcast goes out, as
    /// the main window does; canceling puts the text and staged files back in the composer.
    fn guard_broadcast(&mut self, outgoing: Outgoing, window: &mut Window, cx: &mut Context<Self>) {
        let guard = self.send_target(cx).and_then(|(channel, _)| {
            crate::app::broadcast_guard(&outgoing.content, &self.fixtures, &channel, None)
        });
        let Some(guard) = guard else {
            self.deliver(outgoing, cx);
            return;
        };
        let (this_send, this_cancel) = (cx.entity().downgrade(), cx.entity().downgrade());
        // Taken once on Send or Cancel, so staged files are neither copied nor dropped.
        let pending = Rc::new(Cell::new(Some(outgoing)));
        let pending_send = Rc::clone(&pending);
        crate::app::confirm_broadcast(
            guard,
            move |_, cx| {
                if let Some(outgoing) = pending_send.take() {
                    let _ = this_send.update(cx, |win, cx| win.deliver(outgoing, cx));
                }
            },
            move |_, cx| {
                if let Some(outgoing) = pending.take() {
                    let _ =
                        this_cancel.update(cx, |win, cx| win.restore_outgoing_draft(outgoing, cx));
                }
            },
            window,
            cx,
        );
    }

    fn deliver(&mut self, outgoing: Outgoing, cx: &mut Context<Self>) {
        if outgoing.files.is_empty() {
            self.deliver_content(outgoing.content, outgoing.broadcast, cx);
        } else {
            self.upload(outgoing, cx);
        }
    }

    #[cfg(test)]
    pub(super) fn send_content(&mut self, mut content: MessageContent, cx: &mut Context<Self>) {
        composer::apply_outgoing_transforms(&mut content, &self.fixtures, cx);
        self.deliver_content(content, false, cx);
    }

    /// Post `content` where this window sends; `broadcast` posts a thread reply to its channel too.
    fn deliver_content(
        &mut self,
        content: MessageContent,
        broadcast: bool,
        cx: &mut Context<Self>,
    ) {
        if matches!(&self.kind, PopoutKind::Huddle { .. }) {
            let Some((channel, thread_ts)) = self.huddle_thread_target(cx) else {
                return;
            };
            let broadcast = broadcast
                || (self.composer.as_ref())
                    .is_some_and(|c| c.update(cx, |composer, cx| composer.take_also_send(cx)));
            let mut local =
                self.fixtures
                    .local_message(&channel, content.text.clone(), thread_ts.clone());
            local.blocks = content.blocks.clone();
            local.attachments = content.attachments.clone();
            self.huddle_thread_local.push(local);
            (self.send)(
                MessagingCommand::SendMessage {
                    channel,
                    content,
                    thread_ts,
                    broadcast,
                }
                .into(),
            );
            cx.notify();
            return;
        }
        let (channel, thread_ts) = match &self.kind {
            PopoutKind::Conversation { channel, .. } => (channel.clone(), None),
            PopoutKind::Thread {
                channel, thread_ts, ..
            } => (channel.clone(), Some(thread_ts.clone())),
            PopoutKind::Activity { .. }
            | PopoutKind::Later { .. }
            | PopoutKind::Canvas { .. }
            | PopoutKind::Files { .. }
            | PopoutKind::Huddle { .. } => return,
        };
        let mut local =
            self.fixtures
                .local_message(&channel, content.text.clone(), thread_ts.clone());
        local.blocks = content.blocks.clone();
        local.attachments = content.attachments.clone();
        self.messages.push(local);
        self.scroll.scroll_to_bottom();
        (self.send)(
            MessagingCommand::SendMessage {
                channel,
                content,
                thread_ts,
                broadcast,
            }
            .into(),
        );
        cx.notify();
    }

    /// Upload staged `files` with the composer's text (if any), through the same checks as a
    /// message.
    pub(super) fn send_files(
        &mut self,
        content: Option<MessageContent>,
        files: Vec<crate::composer::PendingAttachment>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if files.is_empty() && content.is_none() {
            return;
        }
        let files = files
            .into_iter()
            .map(|attachment| slackless_core::api::UploadFile {
                filename: attachment.filename,
                bytes: attachment.bytes.into(),
            })
            .collect();
        let outgoing = Outgoing {
            content: content.unwrap_or_default(),
            files,
            broadcast: false,
        };
        self.send_guarded(outgoing, window, cx);
    }

    /// Upload `outgoing`'s files with its text (if any).
    fn upload(&mut self, outgoing: Outgoing, cx: &mut Context<Self>) {
        let Some((channel, thread_ts)) = self.send_target(cx) else {
            return;
        };
        let Outgoing { content, files, .. } = outgoing;
        (self.send)(
            CollabCommand::UploadFiles {
                channel,
                thread_ts,
                content: (content != MessageContent::default()).then_some(content),
                files,
            }
            .into(),
        );
        cx.notify();
    }

    /// Where the composer sends: its conversation, its thread, or the huddle's thread.
    fn send_target(&self, cx: &App) -> Option<(ChannelId, Option<Ts>)> {
        match &self.kind {
            PopoutKind::Conversation { channel, .. } => Some((channel.clone(), None)),
            PopoutKind::Thread {
                channel, thread_ts, ..
            } => Some((channel.clone(), Some(thread_ts.clone()))),
            PopoutKind::Huddle { .. } => self.huddle_thread_target(cx),
            PopoutKind::Activity { .. }
            | PopoutKind::Later { .. }
            | PopoutKind::Canvas { .. }
            | PopoutKind::Files { .. } => None,
        }
    }
}
