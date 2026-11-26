//! Live REST & realtime commands added for GUI parity:
//! - `mark-thread` (`subscriptions.thread.mark`)
//! - `saved` (`saved.list` / `saved.add` / `saved.update` / `saved.delete`)
//! - `files` (`files.list`), `download` (`files.info` + private download), & `transcribe` (local STT + FTS5 index)
//! - `canvas` (`read` / `create` / `edit` / `info` / `delete` / `share` / `unshare` / `comments` / `comment` / `resolve`)
//! - `list` (`fetch_list` / `lists.create` / `lists.records.create` / `lists.records.delete` / `lists.addToList`)
//! - `watch` (live RTM event stream over `wss-primary`)
//! - `huddle` (devices, cameras, screens, link, invite, join, transcripts)
//! - `status` (show, set, clear), `unread`, and `backfill`

mod canvas;
mod channels;
mod files;
mod huddle;
mod list;
mod saved;
pub(crate) mod session;
mod status;
mod watch;

#[cfg(test)]
mod tests;

pub use canvas::{
    run_canvas_comment, run_canvas_comments, run_canvas_create, run_canvas_delete, run_canvas_edit,
    run_canvas_info, run_canvas_read, run_canvas_resolve, run_canvas_share, run_canvas_unshare,
};
pub use channels::{run_archive_backfill, run_backfill, run_unread};
pub use files::{run_download, run_files, run_transcribe};
pub use huddle::{
    HuddleJoinArgs, HuddleTranscriptsArgs, run_huddle_cameras, run_huddle_devices,
    run_huddle_invite, run_huddle_join, run_huddle_link, run_huddle_screens,
    run_huddle_transcripts,
};
pub use list::{
    run_list_add_item, run_list_add_message, run_list_create, run_list_delete_item, run_list_read,
};
pub use saved::{run_mark_thread, run_saved_add, run_saved_delete, run_saved_done, run_saved_list};
pub use status::{run_status_clear, run_status_set, run_status_show};
pub use watch::run_watch;

#[cfg(test)]
pub(crate) use canvas::{
    CanvasResolveAction, CanvasShareTarget, build_canvas_edit_op, classify_canvas_api_error,
    comment_filter_from_flags, filter_canvas_comments, format_canvas_comment_posted,
    format_canvas_comments, format_canvas_delete_result, format_canvas_meta, format_canvas_read,
    format_canvas_resolve_result, format_canvas_share_result, format_canvas_unshare_result,
    parse_canvas_resolve_action, parse_canvas_share_target, render_canvas_html_fallback,
    should_fallback_canvas_html, validate_canvas_comment_args, validate_canvas_share_post,
};
#[cfg(test)]
pub(crate) use channels::{
    ArchiveBackfillChannelSummary, backfill_should_stop, clamp_backfill_page_size,
    format_archive_backfill_output, format_backfill_output, format_unread_json,
    next_backfill_cursor, summarize_unread,
};
#[cfg(test)]
pub(crate) use files::{
    clamp_files_limit, create_numbered_file, format_files_list, format_transcribe_result,
    infer_media_extension, parse_transcribe_provider, sanitize_download_name, select_download_url,
    select_transcribe_media_url,
};
#[cfg(test)]
pub(crate) use huddle::{
    build_huddle_credentials, fallback_huddle_link, format_huddle_cameras,
    format_huddle_data_event, format_huddle_devices, format_huddle_screens, format_huddle_state,
    format_ms_utc,
};
#[cfg(test)]
pub(crate) use list::{effective_list_format, select_list_primary_column};
#[cfg(test)]
pub(crate) use saved::{
    format_mark_thread_output, format_saved_list, mark_thread_cursor, normalize_saved_filter,
};
#[cfg(test)]
pub(crate) use session::{classify_api_error, display_channel, resolve_channel_with};
#[cfg(test)]
pub(crate) use status::{
    compute_status_expiration, format_status_set, format_status_show, normalize_status_emoji,
};
#[cfg(test)]
pub(crate) use watch::{format_watch_event, mirror_event, watch_reached_limit};
