//! Request options, response DTOs, and internal wire structs / parsers for Slack endpoints.

mod activity;
mod apps;
mod auth;
mod canvases;
mod channels;
mod files;
mod huddles;
mod lists;
mod messaging;
#[cfg(test)]
mod tests;
mod users_search;

pub use activity::*;
pub use apps::*;
pub use auth::*;
pub use canvases::*;
pub use channels::*;
pub use files::*;
pub use huddles::*;
pub use lists::*;
pub use messaging::*;
pub use users_search::*;

use serde::{Deserialize, Serialize};

use crate::pagination::Page;
use crate::types::{ApiResponse, ChannelId};

/// Access level granted by `canvases.access.set` and `slackLists.access.set`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessLevel {
    Read,
    Write,
    Owner,
}

impl AccessLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Owner => "owner",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "read" | "view" => Some(Self::Read),
            "write" | "edit" => Some(Self::Write),
            "owner" => Some(Self::Owner),
            _ => None,
        }
    }
}

/// A canvas's or List's file comment channel: its file id with the leading `'F'` a `'C'`.
pub fn comment_channel_id(file_id: &str) -> ChannelId {
    if let Some(rest) = file_id.strip_prefix('F') {
        ChannelId::from(format!("C{rest}"))
    } else {
        ChannelId::from(file_id)
    }
}

/// `_x_reason` the web client tags message sends with.
pub(super) const REASON_MESSAGE_SEND: &str = "webapp_message_send";
/// `_x_reason` for reaction toggles.
pub(super) const REASON_REACTION: &str = "changeReactionFromUserAction";
/// `_x_mode` the web client sends on writes.
pub(super) const MODE_ONLINE: &str = "online";
/// `_x_reason` for slash commands Slack runs.
pub(super) const REASON_COMMAND: &str = "executeCommand";
/// `_x_reason` for Block Kit interactions.
pub(super) const REASON_BLOCK_ACTION: &str = "dispatch_action_to_developer";

/// The web client's per-request `client_token`: `web-<unix ms>`.
pub(super) fn client_token() -> String {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("web-{}", since_epoch.as_millis())
}

/// A fresh `client_msg_id` for `chat.postMessage`: the UUID Slack dedupes posts of one message
/// by. Mint it once per message and reuse it for every retry of that message.
pub fn new_client_msg_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(super) fn page<W, T>(resp: ApiResponse<W>, items: impl FnOnce(W) -> Vec<T>) -> Page<T> {
    let next_cursor = resp.next_cursor().map(str::to_owned);
    Page {
        items: items(resp.data),
        next_cursor,
    }
}
