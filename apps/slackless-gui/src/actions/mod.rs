//! Message actions shared by the transcript (`messages`), thread panel (`threads`),
//! All Unreads (`unreads`), Threads view (`threads_view`), and pop-out windows (`popout`).
//!
//! Organized into focused submodules:
//! - [`hover`]: floating row hover toolbar and unified [`HoverOverlay`] / [`HoverHandle`] lifecycle
//! - [`menu`]: full actions popup/context menu, Slack keyboard shortcuts, and `message_focus` actions
//! - [`reactions`]: reaction pills, rich tooltips, and local reaction mutations
//! - [`toast`]: toasts, permalink/text copy helpers, delete confirmation, and row status banners
//! - [`edit`]: local edit and pin mutations plus shared `MessageAction` dispatch helpers

mod edit;
mod hover;
mod menu;
mod reactions;
mod toast;

use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::gpui::{App, Pixels, Point, Window};
use slackless_core::api::MessageContent;
use slackless_core::types::{Ts, UserId};

use crate::later::Remind;

pub use self::edit::{
    apply_edit, build_save_request, dispatch_block_action, open_custom_remind_for_message,
    toggle_pin,
};
pub use self::hover::{HoverOverlay, hover_toolbar};
pub use self::menu::{
    FocusDeleteMessage, FocusEditMessage, FocusForwardMessage, FocusMarkMessageUnread, FocusReact,
    FocusRemindMe, FocusReplyInThread, FocusSaveMessage, FocusTogglePin, MenuOrigin, actions_menu,
    bind_menu_keys,
};
pub use self::reactions::{REACTIONS_BOTTOM, add_reaction, reaction_chips, toggle_reaction};
pub use self::toast::{
    confirm_delete, copy_link, copy_message_text, ephemeral_banner, notify, notify_pin_toggled,
    notify_saved_toggled, permalink, saved_banner,
};

/// What the user asked to do with a message.
#[derive(Clone)]
pub enum MessageAction {
    /// Add the signed-in user's reaction (no-op if already reacted).
    React(String),
    /// Toggle the signed-in user's reaction (reaction chip click).
    ToggleReaction(String),
    /// Open the emoji picker anchored at this window position.
    PickReaction(Point<Pixels>),
    ReplyInThread,
    CopyLink,
    Forward,
    SaveForLater,
    /// Save for later with a reminder ("Remind me about this").
    RemindMe(Remind),
    /// Save for later with a user-entered custom local date/time reminder.
    RemindCustom,
    MarkUnread,
    Edit,
    Delete,
    /// Pin or unpin the message in its conversation ("Pin to conversation").
    TogglePin,
    /// Copy the message's text ("Copy message").
    CopyText,
    /// The author's avatar or name was clicked: show their profile.
    OpenProfile(UserId),
    /// A Block Kit button was pressed: the element as the message declared it plus its
    /// `block_id`, and the `id` of the legacy attachment holding it (Giphy's preview).
    BlockAction {
        action: crate::blockkit::Json,
        attachment: Option<i64>,
    },
    /// Dismiss an ephemeral ("Only visible to you") message locally.
    DismissEphemeral,
    /// One of the message's images was clicked: open the image viewer on it.
    OpenImage(crate::image_viewer::ImageOrigin),
    /// Open the durable local SQLite revision history log for this message.
    ViewHistory,
    /// Add this message as a new item in a Slack List (`lists.addToList`, `F-COLLAB-231`).
    AddToList,
    /// Broadcast an existing thread reply back into the parent channel (`F-CORE-106`).
    SendToChannel,
    /// Join the live huddle linked to a `huddle_thread` card (`F-COLLAB-003`).
    JoinHuddle,
}

/// Which actions apply to one message.
#[derive(Clone, Copy)]
pub struct ActionSet {
    /// The signed-in user wrote it: Edit and Delete are offered.
    pub own: bool,
    /// Reply in thread is offered (not on a thread root shown in its own panel).
    pub reply: bool,
    /// Currently saved for later: the save action reads "Remove from saved".
    pub saved: bool,
    /// Currently pinned to the conversation: the pin action reads "Unpin from
    /// conversation".
    pub pinned: bool,
    /// Message is a reply inside a thread: offer "Also send to channel" (`F-CORE-106`).
    pub in_thread: bool,
}

pub type ActionHandler = Rc<dyn Fn(MessageAction, &mut Window, &mut App)>;

/// Slack's default one-click reactions at the front of the hover bar.
pub const DEFAULT_QUICK_REACTIONS: [&str; 3] = ["white_check_mark", "eyes", "raised_hands"];

/// Whether `action` applies to a message with this action set: Edit and Delete apply to
/// the signed-in user's messages only, as their menu items do.
pub fn eligible(set: ActionSet, action: &MessageAction) -> bool {
    !matches!(action, MessageAction::Edit | MessageAction::Delete) || set.own
}

/// A change the user made to a message, already shown by the view that made it, which the
/// root must post to Slack (reactions, edits, pins and deletions are not local-only in
/// live mode).
pub enum MessageChange {
    /// `add`: the user's `name` reaction was added (else removed).
    React {
        ts: Ts,
        name: String,
        add: bool,
    },
    Edit {
        ts: Ts,
        content: MessageContent,
    },
    Delete {
        ts: Ts,
    },
    /// `pinned`: the message is now pinned to its conversation (else unpinned).
    Pin {
        ts: Ts,
        pinned: bool,
    },
    /// Move the conversation's read cursor back to `ts` so the target message and everything
    /// after it become unread (`conversations.mark`).
    MarkUnread {
        ts: Ts,
    },
    /// A Block Kit button press for the app that posted the message (`blocks.actions`).
    BlockAction(crate::blockkit::BlockDispatch),
}

/// A `Ts` for "now", matching the fixture timestamp shape.
pub fn now_ts() -> Ts {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Ts::new(format!("{}.{:06}", now.as_secs(), now.subsec_micros()))
}
