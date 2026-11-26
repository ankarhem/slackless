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
pub use client::{
    MAX_RATE_LIMIT_RETRIES, NetLogEntry, SlackApi, clear_net_log, net_log_enabled,
    net_log_export_json, net_log_snapshot, proxy_url, set_net_log_enabled, set_proxy_url,
};
pub use error::ApiError;
pub use methods::{
    ActionContainer, ActivityEntry, ActivityFeed, ActivityItem, ActivityKind, ActivityReaction,
    AppCommand, AppHomeResponse, AuthTest, BlockSuggestionsRequest, BookmarksList, CallInfo,
    CallParticipant, CanvasAccessLevel, CanvasCommentReply, CanvasCommentThread, CanvasCreated,
    CanvasEditOp, CanvasMeta, ChannelBookmark, ChimeAttendee, ChimeCapabilities, ChimeJoinData,
    ChimeMediaPlacement, ChimeMeeting, ClientCounts, CloudDraft, ConversationCounts,
    ConversationView, DeletedFile, EdgeSearchResults, FileCollection, FilesFilter, GifItem,
    HistoryOpts, HuddleCanvasRef, ListCellInput, ListCellInputValue, ListOpts, MessageContent,
    PinnedItem, PinsList, PostedMessage, ProfileUpdate, RemoteFilePage, RemoteMessageHit,
    RemoteMessagePage, RoomJoinResponse, SavedItem, SavedItemState, SavedMark, ScheduledMessage,
    ScheduledMessageCreated, SearchChannel, SearchMatch, SearchMessages, SearchOpts,
    SearchPagination, SearchSort, SectionCreated, SlackListCellValue, SlackListChoice,
    SlackListColumn, SlackListData, SlackListRow, ThreadCounts, ThreadSubscription, ThreadsView,
    UploadFile, UploadSlot, UserBoot, ViewSubmitResponse, WorkflowStepSummary, WorkflowTriggerInfo,
    WorkflowTriggerTripResponse, canvas_comment_channel_id, extract_workflow_trigger_id,
    format_canvas_error, parse_canvas_comment_threads, parse_canvas_meta_from_file,
};
pub use pagination::{Page, paginate};

/// Result alias for this crate.
pub type Result<T, E = ApiError> = std::result::Result<T, E>;
