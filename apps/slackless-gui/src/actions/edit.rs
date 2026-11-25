//! Local message edit/pin mutations and shared `MessageAction` dispatch helpers.

use std::sync::Arc;

use gpui_kit::gpui::{App, Window};
use slackless_core::api::MessageContent;
use slackless_core::types::{ChannelId, Edited, Message, UserId};

use super::{MessageChange, now_ts};
use crate::fixtures::Fixtures;
use crate::later::SaveRequest;

/// Pin or unpin `message` in `channel` (its `pinned_to` list); returns whether it is
/// pinned afterwards.
pub fn toggle_pin(message: &mut Message, channel: &ChannelId) -> bool {
    let pinned = !message.pinned_to.contains(channel);
    if pinned {
        message.pinned_to.push(channel.clone());
    } else {
        message.pinned_to.retain(|kept| kept != channel);
    }
    pinned
}

/// Apply a local edit: new rich-text blocks, mrkdwn fallback text, and the `edited` marker.
pub fn apply_edit(message: &mut Message, me: &UserId, content: MessageContent) {
    message.blocks = content.blocks;
    message.text = content.text;
    message.edited = Some(Edited {
        user: Some(me.clone()),
        ts: now_ts(),
    });
}

/// Build a canonical [`SaveRequest`] for `message` across all message surfaces (transcript,
/// thread panel, All Unreads, Threads view, and pop-outs).
pub fn build_save_request(
    fixtures: &Fixtures,
    channel: ChannelId,
    message: &Message,
    saved: bool,
    due: Option<u64>,
) -> SaveRequest {
    let author = message
        .user
        .as_ref()
        .map(|u| fixtures.user_name(u).to_owned())
        .or_else(|| message.username.clone())
        .or_else(|| message.bot_profile.as_ref().map(|b| b.name.clone()))
        .unwrap_or_else(|| "Message".to_owned());
    SaveRequest {
        channel,
        ts: message.ts.clone(),
        author,
        text: fixtures.preview(message),
        saved,
        due,
    }
}

/// Open the shared custom-reminder modal for `message`, delivering a pre-built [`SaveRequest`]
/// and human-readable `"when"` label on confirm.
pub fn open_custom_remind_for_message(
    fixtures: &Arc<Fixtures>,
    channel: ChannelId,
    message: &Message,
    window: &mut Window,
    cx: &mut App,
    on_confirm: impl Fn(SaveRequest, String, &mut Window, &mut App) + 'static,
) {
    let base = build_save_request(fixtures, channel, message, true, None);
    crate::later::open_custom_remind_dialog(window, cx, move |due, window, cx| {
        let when = crate::drafts::when_label(due, crate::drafts::now_secs());
        let mut request = base.clone();
        request.due = Some(due);
        on_confirm(request, when, window, cx);
    });
}

/// Build a live `MessageChange::BlockAction` when `live` and dispatchable, otherwise log
/// the button click with a toast.
pub fn dispatch_block_action(
    live: bool,
    message: &Message,
    channel: &ChannelId,
    action: crate::blockkit::Json,
    attachment: Option<i64>,
    window: &mut Window,
    cx: &mut App,
) -> Option<MessageChange> {
    if live
        && let Some(dispatch) =
            crate::blockkit::BlockDispatch::new(message, channel, action.clone(), attachment)
    {
        return Some(MessageChange::BlockAction(dispatch));
    }
    crate::blockkit::log_button(&action, window, cx);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_pin_flips_pinned_to() {
        let channel = ChannelId::from("C1");
        let other = ChannelId::from("C2");
        let mut message = Message {
            pinned_to: vec![other.clone()],
            ..Default::default()
        };

        assert!(toggle_pin(&mut message, &channel));
        assert_eq!(message.pinned_to, vec![other.clone(), channel.clone()]);

        assert!(!toggle_pin(&mut message, &channel));
        assert_eq!(message.pinned_to, vec![other.clone()]);
    }
}
