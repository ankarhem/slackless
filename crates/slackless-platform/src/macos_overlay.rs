//! Click-through transparent desktop drawing overlay and floating panel window configuration.
//!
//! On macOS this wraps the AppKit `NSPanel` overlay compiled from `native/macos/appkit_overlay.m`
//! (`NSWindowSharingNone` so screen-share capture never echoes the overlay). On non-macOS targets
//! all functions are safe compiling no-ops.

use std::ffi::c_void;
use std::sync::Arc;

/// Cubic Bezier stroke segment rendered on the click-through desktop drawing overlay
/// (normalized `0.0..=1.0` coordinates relative to the shared monitor or window).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayDrawStroke {
    pub ax: f64,
    pub ay: f64,
    pub bx: f64,
    pub by: f64,
    pub cx: f64,
    pub cy: f64,
    pub dx: f64,
    pub dy: f64,
    pub hue: i32,
    pub alpha: f32,
    pub width: f32,
}

/// Live remote cursor dot rendered on the click-through desktop drawing overlay
/// (normalized `0.0..=1.0` coordinates relative to the shared monitor or window).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayCursorState {
    pub x: f64,
    pub y: f64,
    pub hue: i32,
    pub alpha: f32,
}

/// Interactive pointer event delivered by [`DesktopDrawOverlay::set_pen_mode`] when pen mode is
/// active over the shared monitor or window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverlayPenEvent {
    Down { x: f64, y: f64 },
    Drag { x: f64, y: f64 },
    Up { x: f64, y: f64 },
    Move { x: f64, y: f64 },
}

impl OverlayPenEvent {
    /// Normalized `(x, y)` coordinates in `0.0..=1.0`.
    pub fn coords(self) -> (f64, f64) {
        match self {
            Self::Down { x, y } | Self::Drag { x, y } | Self::Up { x, y } | Self::Move { x, y } => {
                (x, y)
            }
        }
    }
}

/// Callback invoked when interactive desktop drawing pen mode receives pointer events.
pub type OverlayPenHandler = Arc<dyn Fn(OverlayPenEvent) + Send + Sync>;

/// Parse a screen-share target identifier (`"monitor:<u32>"` or `"window:<u32>"`) into
/// `(is_monitor, target_id)`.
fn parse_share_source_id(source_id: &str) -> Option<(bool, u32)> {
    if let Some(rest) = source_id.strip_prefix("monitor:") {
        rest.parse::<u32>().ok().map(|id| (true, id))
    } else if let Some(rest) = source_id.strip_prefix("window:") {
        rest.parse::<u32>().ok().map(|id| (false, id))
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{
        OverlayCursorState, OverlayDrawStroke, OverlayPenEvent, OverlayPenHandler,
        parse_share_source_id,
    };
    use std::ffi::c_void;
    use std::sync::{Mutex, OnceLock};

    type SlacklessOverlayMouseCallback =
        unsafe extern "C" fn(ctx: *mut c_void, kind: u8, nx: f64, ny: f64);

    unsafe extern "C" {
        fn slackless_draw_overlay_update(
            is_monitor: bool,
            target_id: u32,
            segs: *const OverlayDrawStroke,
            seg_count: usize,
            cur: *const OverlayCursorState,
            cur_count: usize,
        );
        fn slackless_draw_overlay_set_pen_mode(
            enabled: bool,
            is_monitor: bool,
            target_id: u32,
            hue: i32,
            ctx: *mut c_void,
            cb: Option<SlacklessOverlayMouseCallback>,
        );
        fn slackless_draw_overlay_hide();
        fn slackless_configure_floating_panel(ns_view_ptr: *mut c_void);
    }

    static OVERLAY_PEN_HANDLER: OnceLock<Mutex<Option<OverlayPenHandler>>> = OnceLock::new();

    unsafe extern "C" fn on_overlay_mouse_cb(_ctx: *mut c_void, kind: u8, nx: f64, ny: f64) {
        let ev = match kind {
            0 => OverlayPenEvent::Down { x: nx, y: ny },
            1 => OverlayPenEvent::Drag { x: nx, y: ny },
            2 => OverlayPenEvent::Up { x: nx, y: ny },
            _ => OverlayPenEvent::Move { x: nx, y: ny },
        };
        let handler = OVERLAY_PEN_HANDLER
            .get()
            .and_then(|m| m.lock().ok())
            .and_then(|g| g.clone());
        if let Some(cb) = handler {
            cb(ev);
        }
    }

    pub(super) fn update(
        source_id: &str,
        strokes: &[OverlayDrawStroke],
        cursors: &[OverlayCursorState],
    ) {
        let Some((is_monitor, target_id)) = parse_share_source_id(source_id) else {
            return;
        };
        let seg_ptr = if strokes.is_empty() {
            std::ptr::null()
        } else {
            strokes.as_ptr()
        };
        let cur_ptr = if cursors.is_empty() {
            std::ptr::null()
        } else {
            cursors.as_ptr()
        };
        // SAFETY: `strokes` and `cursors` are valid for their lengths; the C side copies them
        // synchronously before dispatching onto the AppKit main queue.
        unsafe {
            slackless_draw_overlay_update(
                is_monitor,
                target_id,
                seg_ptr,
                strokes.len(),
                cur_ptr,
                cursors.len(),
            );
        }
    }

    pub(super) fn set_pen_mode(
        enabled: bool,
        source_id: Option<&str>,
        hue: i32,
        on_event: Option<OverlayPenHandler>,
    ) {
        let slot = OVERLAY_PEN_HANDLER.get_or_init(|| Mutex::new(None));
        if !enabled {
            if let Ok(mut g) = slot.lock() {
                *g = None;
            }
            // SAFETY: Disabling pen mode takes no callback/context and dispatches to the main queue.
            unsafe {
                slackless_draw_overlay_set_pen_mode(
                    false,
                    true,
                    0,
                    hue,
                    std::ptr::null_mut(),
                    None,
                );
            }
            return;
        }

        let Some(src) = source_id else {
            return;
        };
        let Some((is_monitor, target_id)) = parse_share_source_id(src) else {
            return;
        };

        if let Ok(mut g) = slot.lock() {
            *g = on_event;
        }
        // SAFETY: `on_overlay_mouse_cb` is a static `extern "C"` function that reads the mutex-
        // guarded handler slot without relying on `ctx`.
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

    pub(super) fn hide() {
        if let Some(slot) = OVERLAY_PEN_HANDLER.get()
            && let Ok(mut g) = slot.lock()
        {
            *g = None;
        }
        // SAFETY: Dispatches overlay teardown onto the main queue.
        unsafe { slackless_draw_overlay_hide() };
    }

    pub(super) unsafe fn configure_floating_panel(ns_view_ptr: *mut c_void) {
        if !ns_view_ptr.is_null() {
            // SAFETY: Caller guarantees `ns_view_ptr` is a valid `NSView *`.
            unsafe { slackless_configure_floating_panel(ns_view_ptr) };
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{OverlayCursorState, OverlayDrawStroke, OverlayPenHandler, parse_share_source_id};
    use std::ffi::c_void;

    pub(super) fn update(
        source_id: &str,
        _strokes: &[OverlayDrawStroke],
        _cursors: &[OverlayCursorState],
    ) {
        let _ = parse_share_source_id(source_id);
    }

    pub(super) fn set_pen_mode(
        _enabled: bool,
        source_id: Option<&str>,
        _hue: i32,
        _on_event: Option<OverlayPenHandler>,
    ) {
        if let Some(src) = source_id {
            let _ = parse_share_source_id(src);
        }
    }

    pub(super) fn hide() {}

    pub(super) unsafe fn configure_floating_panel(_ns_view_ptr: *mut c_void) {}
}

/// Controller for the click-through transparent desktop drawing overlay positioned over a shared
/// monitor (`"monitor:<id>"`) or window (`"window:<id>"`).
#[derive(Debug, Default, Clone, Copy)]
pub struct DesktopDrawOverlay;

impl DesktopDrawOverlay {
    /// Update (or hide when empty and pen mode is off) the click-through transparent desktop
    /// drawing overlay positioned over the shared monitor (`"monitor:<id>"`) or window
    /// (`"window:<id>"`).
    pub fn update(source_id: &str, strokes: &[OverlayDrawStroke], cursors: &[OverlayCursorState]) {
        imp::update(source_id, strokes, cursors);
    }

    /// Enable or disable interactive pen mode on the transparent desktop drawing overlay over the
    /// shared monitor (`"monitor:<id>"`) or window (`"window:<id>"`).
    pub fn set_pen_mode(
        enabled: bool,
        source_id: Option<&str>,
        hue: i32,
        on_event: Option<OverlayPenHandler>,
    ) {
        imp::set_pen_mode(enabled, source_id, hue, on_event);
    }

    /// Immediately hide and clear the click-through desktop drawing overlay.
    pub fn hide() {
        imp::hide();
    }
}

/// Promote a desktop window (via its AppKit `NSView *` raw pointer on macOS) into an
/// always-on-top, movable, `NSWindowSharingNone` floating control panel that stays visible across
/// Cmd+Tab and sits above the transparent desktop drawing overlay.
///
/// No-op on non-macOS platforms.
///
/// # Safety
/// On macOS, `ns_view_ptr` must be either null or a valid `NSView *` pointer (from
/// `raw_window_handle::AppKitWindowHandle`).
pub unsafe fn configure_floating_panel(ns_view_ptr: *mut c_void) {
    // SAFETY: Caller satisfies the `NSView *` pointer contract.
    unsafe { imp::configure_floating_panel(ns_view_ptr) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_share_source_id_validates_monitor_and_window_ids() {
        assert_eq!(parse_share_source_id("monitor:0"), Some((true, 0)));
        assert_eq!(parse_share_source_id("monitor:42"), Some((true, 42)));
        assert_eq!(parse_share_source_id("window:12345"), Some((false, 12345)));
        for bad in ["", "monitor:", "window:abc", "monitor:-1", "display:1"] {
            assert_eq!(
                parse_share_source_id(bad),
                None,
                "expected None for {bad:?}"
            );
        }
    }

    #[test]
    fn desktop_draw_overlay_handles_invalid_and_empty_inputs_safely() {
        DesktopDrawOverlay::update("invalid:abc", &[], &[]);
        DesktopDrawOverlay::update("monitor:notanumber", &[], &[]);
        DesktopDrawOverlay::update("window:notanumber", &[], &[]);
        DesktopDrawOverlay::set_pen_mode(false, None, 180, None);
        DesktopDrawOverlay::hide();
        // SAFETY: Passing null is explicitly documented as a safe no-op.
        unsafe { configure_floating_panel(std::ptr::null_mut()) };
    }

    #[test]
    fn overlay_pen_event_coords_returns_normalized_pair() {
        assert_eq!(
            OverlayPenEvent::Down { x: 0.25, y: 0.75 }.coords(),
            (0.25, 0.75)
        );
        assert_eq!(
            OverlayPenEvent::Drag { x: 0.5, y: 0.5 }.coords(),
            (0.5, 0.5)
        );
        assert_eq!(OverlayPenEvent::Up { x: 1.0, y: 0.0 }.coords(), (1.0, 0.0));
        assert_eq!(
            OverlayPenEvent::Move { x: 0.1, y: 0.2 }.coords(),
            (0.1, 0.2)
        );
    }
}
