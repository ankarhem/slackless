//! Native Slack Canvas reader & section editor (`F-COLLAB-125`, `F-COLLAB-128`, `F-COLLAB-129`,
//! `F-COLLAB-132..136`, `F-COLLAB-139..140`, `F-COLLAB-143`, `F-COLLAB-145`, `F-SHELL-261`,
//! `F-SHELL-264`, `F-SHELL-266`, `F-SHELL-268`, `F-SHELL-277`): renders both the sample
//! workspace's fixture canvas and live Slack canvases fetched via `files.info` -> `url_private`
//! HTML export (keeping `temp:C:...` section IDs), with an in-canvas Find bar, collapsible
//! Outline, interactive checklists, rich tables, emoji rendering and insertion, file uploads,
//! markdown section editor with selection formatting bar, `/` slash insert palette, per-section
//! actions menu, undo/redo, Request edit access, Pop-out window, and "Open in browser".

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

use gpui_kit::component::input::TextareaState;
use gpui_kit::gpui::{App, Entity, Global, Window};
use slackless_core::api::CanvasEditOp;

#[allow(unused_imports)]
pub use model::{CanvasBlock, CanvasDoc};
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
