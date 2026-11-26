//! Draft serialization, submission, and scheduled-send logic for [`Composer`].

use gpui_kit::gpui::{Context, Window};
use slackless_core::api::{GifItem, MessageContent};

use super::{Composer, ComposerEvent};
use crate::drafts::SendLater;
use crate::editor::{Doc, from_mrkdwn};
use crate::fixtures::Fixtures;
use crate::slash;

/// The message `doc` posts as: its `rich_text` block, with mrkdwn as the text fallback.
pub(super) fn content(doc: &Doc) -> MessageContent {
    MessageContent {
        text: doc.to_mrkdwn(),
        blocks: vec![doc.to_rich_text().into()],
        attachments: Vec::new(),
    }
}

/// The message a mrkdwn draft posts as (a scheduled message is kept as its draft text).
pub fn content_of(draft: &str, fixtures: &Fixtures) -> MessageContent {
    content(&from_mrkdwn(draft, fixtures))
}

/// Combine an optional draft document and optional staged GIF into a [`MessageContent`] payload.
pub(super) fn compose_payload(doc: Option<&Doc>, gif: Option<GifItem>) -> Option<MessageContent> {
    match (doc, gif) {
        (Some(doc), Some(gif)) => {
            let mut msg = content(doc);
            msg.attachments.push(gif.to_attachment());
            Some(msg)
        }
        (Some(doc), None) => Some(content(doc)),
        (None, Some(gif)) => Some(MessageContent {
            text: String::new(),
            blocks: Vec::new(),
            attachments: vec![gif.to_attachment()],
        }),
        (None, None) => None,
    }
}

/// Return the mrkdwn draft text if non-blank for scheduling via the Custom Schedule Send dialog.
pub(super) fn custom_schedule_text(doc: &Doc) -> Option<String> {
    let text = doc.to_mrkdwn();
    (!text.trim().is_empty()).then_some(text)
}

impl Composer {
    /// Take the draft for sending: trimmed, and cleared from the composer. `None` when there
    /// is nothing to send.
    pub(super) fn take(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Doc> {
        let doc = self.editor.doc().trimmed();
        if doc.is_blank() {
            return None;
        }
        self.editor.load(Doc::default());
        self.reset(window, cx);
        self.changed(cx);
        Some(doc)
    }

    pub(super) fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.edit_mode && !self.attachments.is_empty() {
            let files = std::mem::take(&mut self.attachments);
            let gif = self.pending_gif.take();
            let doc = self.take(window, cx);
            let msg = compose_payload(doc.as_ref(), gif);
            cx.emit(ComposerEvent::SubmitWithFiles {
                content: msg,
                files,
            });
            cx.notify();
            return;
        }
        if !self.edit_mode
            && let Some(gif) = self.pending_gif.take()
        {
            let doc = self.take(window, cx);
            let msg = compose_payload(doc.as_ref(), Some(gif)).unwrap_or_default();
            cx.emit(ComposerEvent::Submit(msg));
            cx.notify();
            return;
        }
        let Some(doc) = self.take(window, cx) else {
            return;
        };
        if self.edit_mode {
            cx.emit(ComposerEvent::Submit(content(&doc)));
            return;
        }
        let selected_slash = self.selected_slash_target.take();
        match slash::parse(doc.text()) {
            Some(mut invocation) => {
                if let Some(target) = selected_slash
                    && target.name.eq_ignore_ascii_case(&invocation.name)
                {
                    if target.is_builtin {
                        invocation = invocation.with_builtin();
                    } else {
                        invocation = invocation.with_app(target.app_id, target.app_name);
                    }
                }
                cx.emit(ComposerEvent::SlashCommand(invocation));
            }
            None => cx.emit(ComposerEvent::Submit(content(&doc))),
        }
    }

    /// Clear the draft and emit it as a scheduled message.
    pub(super) fn schedule(
        &mut self,
        when: SendLater,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(doc) = self.take(window, cx) {
            let text = doc.to_mrkdwn();
            cx.emit(ComposerEvent::Schedule { text, when });
        }
    }

    /// Ask the root to open the Custom Schedule Send dialog for the current draft (does not
    /// clear the draft yet — root clears it once confirmed).
    pub(super) fn schedule_custom(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = custom_schedule_text(self.editor.doc()) {
            cx.emit(ComposerEvent::ScheduleCustom(text));
        }
    }
}
