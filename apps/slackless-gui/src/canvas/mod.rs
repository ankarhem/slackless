//! Native Slack Canvas reader & section editor (`F-COLLAB-125`, `F-COLLAB-128`, `F-COLLAB-129`,
//! `F-COLLAB-132..136`, `F-COLLAB-139..141`, `F-COLLAB-143..145`, `F-COLLAB-147..148`,
//! `F-SHELL-261`, `F-SHELL-264`, `F-SHELL-266`, `F-SHELL-268`, `F-SHELL-277`): renders both the
//! sample workspace's fixture canvas and live Slack canvases fetched via `files.info` ->
//! `url_private` HTML export (keeping `temp:C:...` section IDs), with an in-canvas Find bar,
//! collapsible Outline, interactive checklists, rich tables, emoji rendering and insertion, file
//! uploads, drag-to-reorder sections, inline audio/video media cards, user profile cards,
//! embedded canvases & lists, markdown section editor with selection formatting bar, `/` slash
//! insert palette, per-section actions menu, undo/redo, Request edit access, Pop-out window, and
//! "Open in browser".

mod blocks;
mod comments;
mod editor;
mod format;
mod model;
mod parse;
mod render;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::input::TextareaState;
use gpui_kit::gpui::{App, Entity, Global, Window};
use slackless_core::api::CanvasEditOp;
use slackless_core::types::UserId;

#[allow(unused_imports)]
pub use model::{
    CanvasBlock, CanvasDoc, CanvasEmbedKind, CanvasUserProfile, MediaKind, resolve_profile_card,
};
#[cfg(test)]
pub use parse::parse_canvas_html;
#[allow(unused_imports)]
pub use parse::{parse_canvas_html_with_ids, parse_canvas_markdown};
pub use render::{canvas_view, live_canvas_view, read_only_badge, render_popout_canvas};

/// Callback invoked when the user clicks "Open in browser" to edit a live canvas.
pub type OpenBrowserFn = Rc<dyn Fn(&mut Window, &mut App)>;
/// Callback invoked when the user edits a live canvas (`canvases.edit`).
pub type CanvasEditFn = Rc<dyn Fn(CanvasEditOp, &mut Window, &mut App)>;
/// Callback invoked when the user pops a canvas out into its own window (`F-COLLAB-128`).
pub type CanvasPopOutFn = Rc<dyn Fn(CanvasDoc, &mut Window, &mut App)>;
/// Callback invoked when opening an inline audio or video embed (`F-COLLAB-141`).
pub type CanvasOpenMediaFn = Rc<dyn Fn(String, MediaKind, &mut Window, &mut App)>;
/// Resolver mapping a Slack `user_id` to display metadata for profile cards (`F-COLLAB-147`).
pub type CanvasUserLookupFn = Rc<dyn Fn(&str) -> Option<CanvasUserProfile>>;
/// Callback invoked when clicking a user profile card inside a canvas (`F-COLLAB-147`).
pub type CanvasOpenProfileFn = Rc<dyn Fn(UserId, &mut Window, &mut App)>;
/// Callback invoked when opening an embedded canvas or list by `(kind, file_id, url)` (`F-COLLAB-148`).
pub type CanvasOpenEmbedFn = Rc<dyn Fn(CanvasEmbedKind, String, String, &mut Window, &mut App)>;

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
/// (`F-COLLAB-147`).
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
    /// Canonical canvas URL used when copying heading anchor links (`F-SHELL-267`).
    pub canvas_url: Option<String>,
    /// Open an inline audio/video embed (`F-COLLAB-141`).
    pub on_open_media: Option<CanvasOpenMediaFn>,
    /// Resolve `user_id` -> display metadata for profile cards (`F-COLLAB-147`).
    pub user_lookup: Option<CanvasUserLookupFn>,
    /// Open a user's profile pane when clicking a profile card (`F-COLLAB-147`).
    pub on_open_profile: Option<CanvasOpenProfileFn>,
    /// Open an embedded canvas or list card (`F-COLLAB-148`).
    pub on_open_embed: Option<CanvasOpenEmbedFn>,
}

#[derive(Clone, Default)]
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
    /// Docked Canvas Comments drawer (`F-COLLAB-197`, `F-COLLAB-199`).
    pub(super) comments_open: bool,
    pub(super) comments: Vec<(String, String)>,
    pub(super) comment_input: Option<Entity<TextareaState>>,
    pub(super) pinned_tab: bool,
    /// Editor emoji quick-insert popover visibility (`F-COLLAB-139`).
    pub(super) emoji_popover_open: bool,
    /// Editor `+ Insert` slash palette visibility (`F-COLLAB-143`).
    pub(super) slash_menu_open: bool,
    /// Checklist block currently showing the inline "+ Add item" input (`F-COLLAB-135`).
    pub(super) adding_checklist_block: Option<usize>,
    pub(super) checklist_add_input: Option<Entity<TextareaState>>,
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
