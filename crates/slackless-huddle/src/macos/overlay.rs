//! AppKit transparent desktop drawing overlay and floating panel configuration.

use std::ffi::c_void;
use std::sync::Arc;

use super::ffi::{
    OverlayCursorFfi, OverlaySegmentFfi, slackless_configure_floating_panel,
    slackless_draw_overlay_hide, slackless_draw_overlay_set_pen_mode,
    slackless_draw_overlay_update,
};
use super::screen::parse_screen_source_id;

/// Update (or hide when empty) the click-through transparent desktop drawing overlay positioned
/// over the shared monitor (`monitor:<id>`) or window (`window:<id>`).
pub fn update_desktop_draw_overlay(
    source_id: &str,
    segments: &[OverlaySegmentFfi],
    cursors: &[OverlayCursorFfi],
) {
    let Some((is_monitor, target_id)) = parse_screen_source_id(source_id) else {
        return;
    };
    let seg_ptr = if segments.is_empty() {
        std::ptr::null()
    } else {
        segments.as_ptr()
    };
    let cur_ptr = if cursors.is_empty() {
        std::ptr::null()
    } else {
        cursors.as_ptr()
    };
    unsafe {
        slackless_draw_overlay_update(
            is_monitor,
            target_id,
            seg_ptr,
            segments.len(),
            cur_ptr,
            cursors.len(),
        );
    }
}

/// Mouse event kind delivered from the interactive macOS desktop drawing overlay when pen mode is
/// enabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayMouseKind {
    Down,
    Drag,
    Up,
    Move,
}

type OverlayMouseHandler = Arc<dyn Fn(OverlayMouseKind, f64, f64) + Send + Sync>;

static OVERLAY_MOUSE_HANDLER: std::sync::OnceLock<std::sync::Mutex<Option<OverlayMouseHandler>>> =
    std::sync::OnceLock::new();

unsafe extern "C" fn on_overlay_mouse_cb(_ctx: *mut c_void, kind: u8, nx: f64, ny: f64) {
    let ev = match kind {
        0 => OverlayMouseKind::Down,
        1 => OverlayMouseKind::Drag,
        2 => OverlayMouseKind::Up,
        _ => OverlayMouseKind::Move,
    };
    let handler = OVERLAY_MOUSE_HANDLER
        .get()
        .and_then(|m| m.lock().ok())
        .and_then(|g| g.clone());
    if let Some(cb) = handler {
        cb(ev, nx, ny);
    }
}

/// Enable or disable interactive pen mode on the transparent desktop drawing overlay over the
/// shared monitor (`monitor:<id>`) or window (`window:<id>`).
pub fn set_desktop_draw_pen_mode(
    enabled: bool,
    source_id: Option<&str>,
    hue: i32,
    on_mouse: Option<OverlayMouseHandler>,
) {
    let slot = OVERLAY_MOUSE_HANDLER.get_or_init(|| std::sync::Mutex::new(None));
    if !enabled {
        if let Ok(mut g) = slot.lock() {
            *g = None;
        }
        unsafe {
            slackless_draw_overlay_set_pen_mode(false, true, 0, hue, std::ptr::null_mut(), None);
        }
        return;
    }

    let Some(src) = source_id else {
        return;
    };
    let Some((is_monitor, target_id)) = parse_screen_source_id(src) else {
        return;
    };

    if let Ok(mut g) = slot.lock() {
        *g = on_mouse;
    }
    unsafe {
        slackless_draw_overlay_set_pen_mode(
            true,
            is_monitor,
            target_id,
            hue,
            std::ptr::null_mut(),
            Some(on_overlay_mouse_cb),
        );
    }
}

/// Immediately hide and clear the click-through desktop drawing overlay.
pub fn hide_desktop_draw_overlay() {
    if let Some(slot) = OVERLAY_MOUSE_HANDLER.get()
        && let Ok(mut g) = slot.lock()
    {
        *g = None;
    }
    unsafe { slackless_draw_overlay_hide() };
}

/// Promote a GPUI window (via its AppKit `NSView` raw pointer) into an always-on-top, movable,
/// `NSWindowSharingNone` floating control panel that stays visible across Cmd+Tab and sits above
/// the transparent desktop drawing overlay.
///
/// # Safety
/// `ns_view_ptr` must be a valid `NSView *` pointer (from `raw_window_handle::AppKitWindowHandle`).
pub unsafe fn configure_floating_panel(ns_view_ptr: *mut c_void) {
    if !ns_view_ptr.is_null() {
        unsafe { slackless_configure_floating_panel(ns_view_ptr) };
    }
}
