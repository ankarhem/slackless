//! Slack private web-client REST API client. See `docs/research/api-*.md`.
//!
//! Auth is the web-client model: `POST <workspace>/api/<method>` with `token=<xoxc-...>` in the
//! form body and `Cookie: d=<xoxd-...>`. Method-name strings and URL helpers live in
//! [`types::endpoints`], so a renamed endpoint is a one-line edit there.
//!
//! Patchability: every response struct here tolerates missing and unknown fields; messages,
//! subtypes and blocks deserialize through `slackless_types`, whose enums carry catch-all
//! variants.

pub use slackless_types as types;

mod assets;
mod client;
mod de;
mod error;
mod methods;
mod pagination;

pub use assets::AssetClient;
pub use client::{MAX_RATE_LIMIT_RETRIES, SlackApi};
pub use error::ApiError;
pub use methods::{
    ActionContainer, ActivityEntry, ActivityFeed, ActivityItem, ActivityKind, ActivityReaction,
    AppCommand, AuthTest, BookmarksList, CanvasCreated, ChannelBookmark, ClientCounts, CloudDraft,
    ConversationCounts, ConversationView, DeletedFile, EdgeSearchResults, FileCollection,
    FilesFilter, GifItem, HistoryOpts, ListOpts, MessageContent, PinnedItem, PinsList,
    PostedMessage, ProfileUpdate, RemoteFilePage, RemoteMessageHit, RemoteMessagePage, SavedItem,
    SavedItemState, SavedMark, ScheduledMessage, ScheduledMessageCreated, SearchChannel,
    SearchMatch, SearchMessages, SearchOpts, SearchPagination, SearchSort, SectionCreated,
    SlackListCellValue, SlackListChoice, SlackListColumn, SlackListData, SlackListRow,
    ThreadCounts, ThreadSubscription, ThreadsView, UploadFile, UploadSlot, UserBoot,
};
pub use pagination::{Page, paginate};

/// Result alias for this crate.
pub type Result<T, E = ApiError> = std::result::Result<T, E>;
