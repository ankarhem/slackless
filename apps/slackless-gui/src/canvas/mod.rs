//! Native Slack Canvas reader & section editor (`F-COLLAB-120`, `F-COLLAB-123..126`,
//! `F-COLLAB-128..129`, `F-COLLAB-132..136`, `F-COLLAB-139..141`, `F-COLLAB-143..145`,
//! `F-COLLAB-147..148`, `F-COLLAB-150`, `F-COLLAB-152`, `F-COLLAB-155..156`, `F-COLLAB-158`,
//! `F-COLLAB-160..174`, `F-COLLAB-182..185`, `F-COLLAB-194..201`, `F-COLLAB-204`,
//! `F-SHELL-261`, `F-SHELL-264`, `F-SHELL-266`, `F-SHELL-268`, `F-SHELL-277`).

mod blocks;
mod comments;
mod editor;
mod export;
mod format;
mod keyboard;
mod mentions;
mod model;
mod parse;
mod render;
mod share;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::input::TextareaState;
use gpui_kit::gpui::{App, Entity, FocusHandle, Global, Window};
use slackless_core::api::{CanvasAccessLevel, CanvasCommentThread, CanvasEditOp, CanvasMeta};
use slackless_core::types::{ChannelId, Ts, UserId};

pub use comments::{
    CanvasCommentFilter, CanvasCommentPostFn, CanvasCommentResolveAllFn, CanvasCommentResolveFn,
    CanvasCommentsLoadFn,
};
#[cfg(test)]
pub use comments::{comment_thread_counts, filter_comment_threads};
pub use export::{canvas_export_stem, save_canvas_pdf_to_path, trigger_canvas_print};
#[cfg(test)]
pub use export::{canvas_print_allowed, canvas_to_printable_html, write_printable_html};
#[cfg(test)]
pub use keyboard::{
    CanvasActivation, activate_focused_block, step_block, toggle_section_actions_shortcut,
};
pub use keyboard::{CanvasFocusRegion, CanvasKeyboardState, cycle_region};
#[cfg(test)]
pub use mentions::{CanvasInlineSegment, parse_inline_canvas_segments};
#[cfg(test)]
pub use model::{CanvasBlock, resolve_profile_card};
pub use model::{CanvasDoc, CanvasEmbedKind, CanvasUserProfile, MediaKind};
#[cfg(test)]
pub use parse::{parse_canvas_html, parse_canvas_markdown};
pub use render::{canvas_view, live_canvas_view, read_only_badge, render_popout_canvas};
pub use share::{CanvasAccessDeleteFn, CanvasAccessSetFn, CanvasDeleteFn, ShareAccessRequest};
#[cfg(test)]
pub use share::{
    build_share_access_request, canvas_server_rule_notice, general_access_summary,
    parse_share_targets,
};

/// Callback invoked when the user clicks "Open in browser" to edit a live canvas.
pub type OpenBrowserFn = Rc<dyn Fn(&mut Window, &mut App)>;
/// Callback invoked when the user edits a live canvas (`canvases.edit`).
pub type CanvasEditFn = Rc<dyn Fn(CanvasEditOp, &mut Window, &mut App)>;
/// Callback invoked when the user pops a canvas out into its own window (`F-COLLAB-128`).
pub type CanvasPopOutFn = Rc<dyn Fn(CanvasDoc, &mut Window, &mut App)>;
/// Callback invoked when opening an inline audio or video embed (`F-COLLAB-141`).
pub type CanvasOpenMediaFn = Rc<dyn Fn(String, MediaKind, &mut Window, &mut App)>;
/// Resolver mapping a Slack `user_id` to display metadata for profile cards & mentions
/// (`F-COLLAB-147`, `F-COLLAB-150`).
pub type CanvasUserLookupFn = Rc<dyn Fn(&str) -> Option<CanvasUserProfile>>;
/// Resolver mapping a Slack `channel_id` to its display name for inline `#channel` mentions
/// (`F-COLLAB-150`).
pub type CanvasChannelLookupFn = Rc<dyn Fn(&str) -> Option<String>>;
/// Callback invoked when clicking a user profile card or `@user` mention inside a canvas
/// (`F-COLLAB-147`, `F-COLLAB-150`).
pub type CanvasOpenProfileFn = Rc<dyn Fn(UserId, &mut Window, &mut App)>;
/// Callback invoked when clicking an inline `#channel` mention inside a canvas (`F-COLLAB-150`).
pub type CanvasOpenChannelFn = Rc<dyn Fn(ChannelId, &mut Window, &mut App)>;
/// Callback invoked when opening an embedded canvas or list by `(kind, file_id, url)` (`F-COLLAB-148`).
pub type CanvasOpenEmbedFn = Rc<dyn Fn(CanvasEmbedKind, String, String, &mut Window, &mut App)>;
/// Callback invoked for Print or Save-as-PDF export (`F-COLLAB-182`, `F-COLLAB-183`).
pub type CanvasExportFn = Rc<dyn Fn(CanvasDoc, &mut Window, &mut App)>;

/// Result of uploading a file into a canvas (`F-COLLAB-140`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadedCanvasFile {
    pub filename: String,
    pub url: String,
}

/// Continuation called once a file has been selected/uploaded into a canvas (`F-COLLAB-140`).
pub type CanvasUploadCompleteFn = Rc<dyn Fn(UploadedCanvasFile, &mut Window, &mut App)>;

/// Callback invoked when the user clicks "Upload file" inside a canvas (`F-COLLAB-140`).
pub type CanvasUploadFileFn = Rc<dyn Fn(CanvasUploadCompleteFn, &mut Window, &mut App)>;

/// Build a [`CanvasUserLookupFn`] backed by the workspace's [`crate::fixtures::Fixtures`] cache
/// (`F-COLLAB-147`, `F-COLLAB-150`).
pub fn user_lookup_from_fixtures(fixtures: Arc<crate::fixtures::Fixtures>) -> CanvasUserLookupFn {
    Rc::new(move |user_id: &str| {
        let uid = UserId::from(user_id);
        let user = fixtures.user(&uid)?;
        let display_name = fixtures.name_of(user).to_owned();
        let title = user
            .profile
            .title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let handle = (!user.name.is_empty()).then(|| user.name.clone());
        Some(CanvasUserProfile {
            user_id: user_id.to_owned(),
            display_name,
            title,
            handle,
        })
    })
}

/// Build a [`CanvasChannelLookupFn`] backed by the workspace's [`crate::fixtures::Fixtures`] cache
/// (`F-COLLAB-150`).
pub fn channel_lookup_from_fixtures(
    fixtures: Arc<crate::fixtures::Fixtures>,
) -> CanvasChannelLookupFn {
    Rc::new(move |channel_id: &str| {
        let cid = ChannelId::from(channel_id);
        let conv = fixtures.conversation(&cid)?;
        let name = conv.name.as_deref()?.trim();
        if name.is_empty() {
            None
        } else {
            Some(name.to_owned())
        }
    })
}

/// Optional interactive callbacks wired by the host window (`app/render.rs` or `popout/mod.rs`).
#[derive(Clone, Default)]
pub struct CanvasActions {
    pub open_browser: Option<OpenBrowserFn>,
    pub on_edit: Option<CanvasEditFn>,
    pub on_request_access: Option<OpenBrowserFn>,
    pub on_pop_out: Option<CanvasPopOutFn>,
    /// Pin this canvas as a conversation tab (`conversations.addTab`, `F-COLLAB-121`).
    pub on_pin_tab: Option<OpenBrowserFn>,
    /// Upload a local file/image into this canvas (`F-COLLAB-140`).
    pub on_upload_file: Option<CanvasUploadFileFn>,
    /// Canonical canvas URL used when copying heading anchor links and canvas permalink
    /// (`F-COLLAB-162`, `F-SHELL-267`).
    pub canvas_url: Option<String>,
    /// Open an inline audio/video embed (`F-COLLAB-141`).
    pub on_open_media: Option<CanvasOpenMediaFn>,
    /// Resolve `user_id` -> display metadata for profile cards & mentions (`F-COLLAB-147`, `F-COLLAB-150`).
    pub user_lookup: Option<CanvasUserLookupFn>,
    /// Resolve `channel_id` -> display name for inline `#channel` mentions (`F-COLLAB-150`).
    pub channel_lookup: Option<CanvasChannelLookupFn>,
    /// Open a user's profile pane when clicking a profile card or `@user` mention (`F-COLLAB-147`, `F-COLLAB-150`).
    pub on_open_profile: Option<CanvasOpenProfileFn>,
    /// Open a channel when clicking an inline `#channel` mention (`F-COLLAB-150`).
    pub on_open_channel: Option<CanvasOpenChannelFn>,
    /// Open an embedded canvas or list card (`F-COLLAB-148`).
    pub on_open_embed: Option<CanvasOpenEmbedFn>,
    /// Parsed `CanvasMeta` for live canvases (permissions, printing policy, owners/editors).
    pub meta: Option<CanvasMeta>,
    /// Live threaded comments for this canvas (`Event::CanvasCommentsLoaded`, `F-COLLAB-152`, `F-COLLAB-155`).
    pub comment_threads: Option<Vec<CanvasCommentThread>>,
    /// Trigger loading comments (`Command::LoadCanvasComments`).
    pub on_load_comments: Option<CanvasCommentsLoadFn>,
    /// Post a new root comment or reply (`Command::PostCanvasComment`, `F-COLLAB-152`).
    pub on_post_comment: Option<CanvasCommentPostFn>,
    /// Resolve or reopen one comment thread (`Command::ResolveCanvasComment`, `F-COLLAB-156`).
    pub on_resolve_comment: Option<CanvasCommentResolveFn>,
    /// Resolve all open comment threads (`Command::ResolveAllCanvasComments`, `F-COLLAB-156`).
    pub on_resolve_all_comments: Option<CanvasCommentResolveAllFn>,
    /// Grant or update canvas access (`Command::SetCanvasAccess`, `F-COLLAB-160..170`).
    pub on_set_access: Option<CanvasAccessSetFn>,
    /// Revoke canvas access (`Command::DeleteCanvasAccess`, `F-COLLAB-169`).
    pub on_delete_access: Option<CanvasAccessDeleteFn>,
    /// Permanently delete this canvas (`Command::DeleteCanvas`, `F-COLLAB-158`).
    pub on_delete_canvas: Option<CanvasDeleteFn>,
    /// Print this canvas (`F-COLLAB-182`).
    pub on_print_canvas: Option<CanvasExportFn>,
    /// Save this canvas as PDF (`F-COLLAB-183`).
    pub on_save_pdf: Option<CanvasExportFn>,
    /// Reload this canvas from Slack (`F-COLLAB-185`).
    pub on_refresh_canvas: Option<OpenBrowserFn>,
    /// Optional inline `(is_error, message)` status/error banner (surfaced via `format_canvas_error`).
    pub status_banner: Option<(bool, String)>,
    /// Optional unobtrusive "Canvas updated" notice (`F-COLLAB-185`).
    pub updated_note: Option<String>,
    /// Dismiss the "Canvas updated" notice (`F-COLLAB-185`).
    pub on_dismiss_updated: Option<OpenBrowserFn>,
}

#[derive(Clone)]
pub(super) struct CanvasUiState {
    pub(super) editing: bool,
    pub(super) editor: Option<Entity<TextareaState>>,
    pub(super) undo_stack: Vec<String>,
    pub(super) redo_stack: Vec<String>,
    /// Local copy for fixture mode or optimistic live edits.
    pub(super) local_doc: Option<CanvasDoc>,
    pub(super) access_requested: bool,
    /// Distraction-free Reader View (`F-COLLAB-198`): hides find bar & outline chrome.
    pub(super) reader_mode: bool,
    /// Docked Canvas Comments drawer (`F-COLLAB-152`, `F-COLLAB-155..156`, `F-COLLAB-197`, `F-COLLAB-199`).
    pub(super) comments_open: bool,
    pub(super) comments: Vec<CanvasCommentThread>,
    pub(super) comments_seeded: bool,
    pub(super) comment_filter: CanvasCommentFilter,
    pub(super) comment_input: Option<Entity<TextareaState>>,
    pub(super) replying_thread: Option<Ts>,
    pub(super) reply_input: Option<Entity<TextareaState>>,
    pub(super) confirm_resolve_all: bool,
    /// Docked Share & Access panel (`F-COLLAB-160..174`).
    pub(super) share_open: bool,
    pub(super) share_target_input: Option<Entity<TextareaState>>,
    pub(super) share_access_level: CanvasAccessLevel,
    pub(super) share_without_posting: bool,
    pub(super) share_general_access: String,
    pub(super) share_limit_sharing: bool,
    pub(super) share_local_editors: Vec<UserId>,
    pub(super) share_status: Option<(bool, String)>,
    pub(super) link_copied: bool,
    /// Two-step inline Delete confirmation & fixture deleted state (`F-COLLAB-158`).
    pub(super) confirm_delete: bool,
    pub(super) deleted: bool,
    /// Collapsible "Files in this canvas" strip (`F-COLLAB-126`).
    pub(super) files_strip_open: bool,
    /// Inline accessible image description editor (`F-COLLAB-200`).
    pub(super) editing_image_block: Option<usize>,
    pub(super) image_alt_input: Option<Entity<TextareaState>>,
    /// Keyboard focus region & section actions shortcut state (`F-COLLAB-194..196`).
    pub(super) keyboard: CanvasKeyboardState,
    pub(super) focus_handle: Option<FocusHandle>,
    pub(super) pinned_tab: bool,
    /// Editor emoji quick-insert popover visibility (`F-COLLAB-139`).
    pub(super) emoji_popover_open: bool,
    /// Editor `+ Insert` slash palette visibility (`F-COLLAB-143`).
    pub(super) slash_menu_open: bool,
    /// Checklist block currently showing the inline "+ Add item" input (`F-COLLAB-135`).
    pub(super) adding_checklist_block: Option<usize>,
    pub(super) checklist_add_input: Option<Entity<TextareaState>>,
}

impl Default for CanvasUiState {
    fn default() -> Self {
        Self {
            editing: false,
            editor: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            local_doc: None,
            access_requested: false,
            reader_mode: false,
            comments_open: false,
            comments: Vec::new(),
            comments_seeded: false,
            comment_filter: CanvasCommentFilter::All,
            comment_input: None,
            replying_thread: None,
            reply_input: None,
            confirm_resolve_all: false,
            share_open: false,
            share_target_input: None,
            share_access_level: CanvasAccessLevel::Read,
            share_without_posting: false,
            share_general_access: String::new(),
            share_limit_sharing: false,
            share_local_editors: Vec::new(),
            share_status: None,
            link_copied: false,
            confirm_delete: false,
            deleted: false,
            files_strip_open: false,
            editing_image_block: None,
            image_alt_input: None,
            keyboard: CanvasKeyboardState::default(),
            focus_handle: None,
            pinned_tab: false,
            emoji_popover_open: false,
            slash_menu_open: false,
            adding_checklist_block: None,
            checklist_add_input: None,
        }
    }
}

#[derive(Default)]
struct CanvasUiStore {
    panels: HashMap<String, CanvasUiState>,
}

impl Global for CanvasUiStore {}

pub(super) fn ui_state(key: &str, cx: &App) -> CanvasUiState {
    cx.try_global::<CanvasUiStore>()
        .and_then(|s| s.panels.get(key))
        .cloned()
        .unwrap_or_default()
}

pub(super) fn mutate_ui(
    key: &str,
    f: impl FnOnce(&mut CanvasUiState),
    window: &mut Window,
    cx: &mut App,
) {
    let entry = cx
        .default_global::<CanvasUiStore>()
        .panels
        .entry(key.to_owned())
        .or_default();
    f(entry);
    window.refresh();
}

/// Cycle the active canvas's keyboard focus region (`F6` / `Shift+F6`, `F-COLLAB-194`).
pub fn cycle_canvas_focus_region(
    key: &str,
    forward: bool,
    has_outline: bool,
    window: &mut Window,
    cx: &mut App,
) -> CanvasFocusRegion {
    let mut next = CanvasFocusRegion::Header;
    mutate_ui(
        key,
        |s| {
            next = cycle_region(s.keyboard.region, forward, has_outline, s.comments_open);
            s.keyboard.region = next;
            if next == CanvasFocusRegion::Document && s.keyboard.focused_block.is_none() {
                s.keyboard.focused_block = Some(0);
            }
        },
        window,
        cx,
    );
    next
}

/// Build the most targeted [`CanvasEditOp`] for replacing a single block.
pub(super) fn section_replace_op(
    section_id: Option<String>,
    block_markdown: String,
    full_markdown: String,
) -> CanvasEditOp {
    if let Some(sid) = section_id {
        CanvasEditOp::Replace {
            section_id: Some(sid),
            markdown: block_markdown,
        }
    } else {
        CanvasEditOp::Replace {
            section_id: None,
            markdown: full_markdown,
        }
    }
}

/// Build the most targeted [`CanvasEditOp`] for inserting markdown right after a block.
pub(super) fn section_insert_after_op(
    section_id: Option<String>,
    inserted_markdown: String,
    full_markdown: String,
) -> CanvasEditOp {
    if let Some(sid) = section_id {
        CanvasEditOp::InsertAfter {
            section_id: sid,
            markdown: inserted_markdown,
        }
    } else {
        CanvasEditOp::Replace {
            section_id: None,
            markdown: full_markdown,
        }
    }
}

/// Build the most targeted [`CanvasEditOp`] for deleting a block.
pub(super) fn section_delete_op(section_id: Option<String>, full_markdown: String) -> CanvasEditOp {
    if let Some(sid) = section_id {
        CanvasEditOp::Delete { section_id: sid }
    } else {
        CanvasEditOp::Replace {
            section_id: None,
            markdown: full_markdown,
        }
    }
}

pub(super) fn commit_block_edit(
    key: &str,
    updated: CanvasDoc,
    op: CanvasEditOp,
    is_fixture: bool,
    on_edit: Option<&CanvasEditFn>,
    window: &mut Window,
    cx: &mut App,
) {
    if is_fixture {
        mutate_ui(key, |s| s.local_doc = Some(updated), window, cx);
    }
    if let Some(cb) = on_edit {
        cb(op, window, cx);
    }
}
