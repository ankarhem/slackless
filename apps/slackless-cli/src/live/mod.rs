//! Live REST & realtime commands added for GUI parity:
//! - `mark-thread` (`subscriptions.thread.mark`)
//! - `saved` (`saved.list` / `saved.add` / `saved.update` / `saved.delete`)
//! - `files` (`files.list`) & `download` (`files.info` + private download)
//! - `canvas` (`read` / `create` / `edit` / `info` / `delete` / `share` / `unshare` / `comments` / `comment` / `resolve`)
//! - `list` (`fetch_list` / `lists.create` / `lists.records.create` / `lists.records.delete` / `lists.addToList`)
//! - `watch` (live RTM event stream over `wss-primary`)
//! - `huddle` (devices, cameras, screens, link, invite, join)
//! - `status` (show, set, clear), `unread`, and `backfill`

mod canvas_list;
mod channels;
mod files;
mod huddle;
mod saved;
mod session;
mod status;
mod watch;

#[cfg(test)]
mod tests;

pub use canvas_list::{
    run_canvas_comment, run_canvas_comments, run_canvas_create, run_canvas_delete, run_canvas_edit,
    run_canvas_info, run_canvas_read, run_canvas_resolve, run_canvas_share, run_canvas_unshare,
    run_list_add_item, run_list_add_message, run_list_create, run_list_delete_item, run_list_read,
};
pub use channels::{run_backfill, run_unread};
pub use files::{run_download, run_files};
pub use huddle::{
    run_huddle_cameras, run_huddle_devices, run_huddle_invite, run_huddle_join, run_huddle_link,
    run_huddle_screens,
};
pub use saved::{run_mark_thread, run_saved_add, run_saved_delete, run_saved_done, run_saved_list};
pub use status::{run_status_clear, run_status_set, run_status_show};
pub use watch::run_watch;

#[cfg(test)]
pub(crate) use canvas_list::{
    CanvasCommentFilter, CanvasResolveAction, CanvasShareTarget, build_canvas_edit_op,
    cell_to_text, classify_canvas_api_error, effective_list_format, filter_canvas_comments,
    format_canvas_comment_posted, format_canvas_comments, format_canvas_delete_result,
    format_canvas_meta, format_canvas_read, format_canvas_resolve_result,
    format_canvas_share_result, format_canvas_unshare_result, list_to_csv, list_to_json_value,
    list_to_tsv, parse_canvas_resolve_action, parse_canvas_share_target,
    select_list_primary_column, validate_canvas_comment_args, validate_canvas_share_post,
};
#[cfg(test)]
pub(crate) use channels::{
    backfill_should_stop, clamp_backfill_page_size, format_backfill_output, format_unread_json,
    summarize_unread,
};
#[cfg(test)]
pub(crate) use files::{
    clamp_files_limit, format_files_list, resolve_download_dest, sanitize_download_name,
    select_download_url,
};
#[cfg(test)]
pub(crate) use huddle::{
    build_huddle_credentials, fallback_huddle_link, format_huddle_cameras, format_huddle_devices,
    format_huddle_screens, format_huddle_state,
};
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
pub(crate) use watch::{format_watch_event, watch_reached_limit};
