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

use crate::pagination::Page;
use crate::types::ApiResponse;

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

pub(super) fn page<W, T>(resp: ApiResponse<W>, items: impl FnOnce(W) -> Vec<T>) -> Page<T> {
    let next_cursor = resp.next_cursor().map(str::to_owned);
    Page {
        items: items(resp.data),
        next_cursor,
    }
}
