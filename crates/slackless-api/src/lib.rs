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
    MAX_FILE_BYTES, MAX_RATE_LIMIT_RETRIES, NetLogEntry, SlackApi, clear_net_log,
    install_default_crypto_provider, net_log_enabled, net_log_export_json, net_log_snapshot,
    proxy_url, set_net_log_enabled, set_proxy_url,
};
pub use error::{ApiError, DownloadError};
pub use methods::{
    ActionContainer, ActivityEntry, ActivityFeed, ActivityItem, ActivityKind, ActivityReaction,
    AppCommand, AppHomeResponse, AuthTest, BlockSuggestionsRequest, BookmarksList, CallInfo,
    CallParticipant, CanvasAccessLevel, CanvasCommentReply, CanvasCommentThread, CanvasCreated,
    CanvasEditOp, CanvasMeta, ChannelBookmark, ChimeAttendee, ChimeCapabilities, ChimeJoinData,
    ChimeMediaPlacement, ChimeMeeting, ClientCounts, CloudDraft, CommandResponse,
    ConversationCounts, DeletedFile, EdgeSearchResults, FileCollection, FilesFilter, GifItem,
    HistoryOpts, HuddleCanvasRef, ListAccessDeleteResult, ListAccessLevel, ListCellInput,
    ListCellInputValue, ListMeta, ListOpts, MessageContent, PinnedItem, PinsList, PostedMessage,
    ProfileUpdate, RemoteFilePage, RemoteMessageHit, RemoteMessagePage, RoomJoinResponse,
    SavedItem, SavedItemState, SavedMark, ScheduledMessage, ScheduledMessageCreated, SearchSort,
    SectionCreated, SlackListCellValue, SlackListChoice, SlackListColumn, SlackListData,
    SlackListRow, TeamConnectionsResponse, TeamProfileField, ThreadCounts, ThreadSubscription,
    ThreadsView, UploadFile, UploadSlot, UserBoot, ViewSubmitResponse, WorkflowStepSummary,
    WorkflowTriggerInfo, WorkflowTriggerTripResponse, canvas_comment_channel_id,
    extract_workflow_trigger_id, format_canvas_error, format_list_error, list_comment_channel_id,
    new_client_msg_id, parse_canvas_comment_threads, parse_canvas_meta_from_file, parse_list_meta,
    parse_list_record_threads,
};
pub use pagination::{Page, paginate};

/// Result alias for this crate.
pub type Result<T, E = ApiError> = std::result::Result<T, E>;
