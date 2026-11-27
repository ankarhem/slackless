//! Hardware-accelerated macOS `ScreenCaptureKit` screen capture and `SCScreenshotManager` thumbnails.

use std::ffi::{CStr, c_char, c_void};
use std::ptr::NonNull;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::ffi::{
    CaptureCallbackCtx, SlacklessScCapture, on_error_cb, on_h264_cb, on_surface_preview_cb,
    slackless_capture_thumbnails, slackless_sc_capture_force_keyframe,
    slackless_sc_capture_set_bitrate, slackless_sc_capture_start, slackless_sc_capture_stop,
};
use crate::backend::{ScreenShareConfig, VideoFrameSink};
use crate::metrics::HuddlePerfCounters;
use crate::video::VideoFrame;
use crate::{HuddleError, Result};

/// Parse a screen-share source identifier (`"monitor:<u32>"` or `"window:<u32>"`) into
/// `(is_monitor, target_id)`.
pub fn parse_screen_source_id(source_id: &str) -> Option<(bool, u32)> {
    if let Some(rest) = source_id.strip_prefix("monitor:") {
        rest.parse::<u32>().ok().map(|id| (true, id))
    } else if let Some(rest) = source_id.strip_prefix("window:") {
        rest.parse::<u32>().ok().map(|id| (false, id))
    } else {
        None
    }
}

/// Hardware-accelerated macOS `ScreenCaptureKit` (`420f`) + `VideoToolbox` H.264 High/CABAC screen share.
pub struct ScVtScreenCapture {
    handle: NonNull<SlacklessScCapture>,
}

// SAFETY: `SlacklessScCapture` is thread-safe for keyframe/bitrate control and stopped once on `Drop`.
unsafe impl Send for ScVtScreenCapture {}

impl ScVtScreenCapture {
    pub fn start(
        config: &ScreenShareConfig,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        on_preview: VideoFrameSink,
        perf: Arc<HuddlePerfCounters>,
    ) -> Result<Self> {
        let (is_monitor, target_id) = match parse_screen_source_id(&config.source_id) {
            Some((true, id)) => (1u32, id),
            Some((false, id)) => (0u32, id),
            None => (1u32, 0u32),
        };

        perf.set_tx_backend("ScreenCaptureKit + VideoToolbox H.264 High (HW)");
        let ctx = NonNull::from(Box::leak(Box::new(CaptureCallbackCtx {
            outbound_h264_tx: Some(outbound_h264_tx),
            on_preview: Box::new(move |frame| on_preview(frame)),
            perf: Some(perf),
        })));
        let mut err_buf = [0 as c_char; 512];
        // SAFETY: `ctx` ownership transfers to native side, which releases it via `on_released_cb`
        // on both start failure and `Drop` once all callbacks have drained. `err_buf` is writable
        // for its length.
        let handle = unsafe {
            slackless_sc_capture_start(
                is_monitor,
                target_id,
                config.max_w,
                config.max_h,
                config.max_fps,
                config.bitrate_bps,
                ctx.as_ptr().cast(),
                on_h264_cb,
                on_surface_preview_cb,
                on_error_cb,
                Some(on_released_cb),
                err_buf.as_mut_ptr(),
                err_buf.len(),
            )
        };
        let Some(handle) = NonNull::new(handle) else {
            // `slackless_sc_capture_start` already invoked (or deferred to stop completion)
            // `on_released_cb(ctx)`; never double-free `ctx` here.
            // SAFETY: `err_buf` starts zeroed and the native side only `snprintf`s into it, so it
            // holds a NUL-terminated string.
            let msg = unsafe { CStr::from_ptr(err_buf.as_ptr()) }
                .to_str()
                .unwrap_or("ScreenCaptureKit startup failed")
                .to_owned();
            return Err(HuddleError::Video(msg));
        };
        tracing::info!(
            source_id = %config.source_id,
            max_w = config.max_w,
            max_h = config.max_h,
            max_fps = config.max_fps,
            bitrate_kbps = config.bitrate_bps / 1000,
            "started macOS ScreenCaptureKit (420f) + VideoToolbox H.264 High HW screen capture"
        );
        Ok(Self { handle })
    }

    pub fn force_keyframe(&self) {
        // SAFETY: `handle` is live until `Drop` stops it.
        unsafe { slackless_sc_capture_force_keyframe(self.handle.as_ptr()) };
    }

    pub fn set_target_bitrate(&self, bitrate_bps: u32) {
        // SAFETY: `handle` is live until `Drop` stops it.
        unsafe { slackless_sc_capture_set_bitrate(self.handle.as_ptr(), bitrate_bps) };
    }
}

unsafe extern "C" fn on_released_cb(ctx: *mut c_void) {
    if !ctx.is_null() {
        // SAFETY: the stopped capture calls this once, after its last callback, with the `ctx`
        // that `ScVtScreenCapture::start` leaked from a `Box`.
        unsafe { drop(Box::from_raw(ctx.cast::<CaptureCallbackCtx>())) };
    }
}

impl Drop for ScVtScreenCapture {
    fn drop(&mut self) {
        // Native code releases `ctx` (stored in `cap->enc.ctx` at start) from its stop completion
        // handler, once no ScreenCaptureKit callback can still reach it.
        let handle = self.handle.as_ptr();
        // SAFETY: `handle` came from a successful start and is stopped once, here; the stop takes
        // over `self.ctx` via `cap->enc.ctx` and frees it through `on_released_cb`.
        unsafe { slackless_sc_capture_stop(handle, Some(on_released_cb)) };
    }
}

unsafe extern "C" fn on_thumb_cb(
    ctx: *mut c_void,
    source_id: *const c_char,
    bgra: *const u8,
    len: usize,
    width: u32,
    height: u32,
) {
    if ctx.is_null() || source_id.is_null() || bgra.is_null() || len == 0 {
        return;
    }
    // SAFETY: `ctx` is the `Vec` that `capture_macos_thumbnails` lends the native call, which
    // runs this under its batch mutex and cancels the batch before returning, so calls never
    // overlap or outlive the borrow.
    let out = unsafe { &mut *(ctx as *mut Vec<(String, VideoFrame)>) };
    // SAFETY: `source_id` is a NUL-terminated string valid for this call.
    let Ok(id) = unsafe { CStr::from_ptr(source_id) }.to_str() else {
        return;
    };
    // SAFETY: `bgra` holds `len` initialized bytes that stay valid for this call.
    let slice = unsafe { std::slice::from_raw_parts(bgra, len) };
    out.push((
        id.to_owned(),
        VideoFrame::from_bgra(width, height, slice.to_vec()),
    ));
}

unsafe extern "C" fn should_skip_system_window_cb(
    app: *const c_char,
    title: *const c_char,
) -> bool {
    let app_str = if app.is_null() {
        ""
    } else {
        // SAFETY: a non-null `app` is a NUL-terminated string valid for this call.
        unsafe { CStr::from_ptr(app) }.to_str().unwrap_or("")
    };
    let title_str = if title.is_null() {
        ""
    } else {
        // SAFETY: a non-null `title` is a NUL-terminated string valid for this call.
        unsafe { CStr::from_ptr(title) }.to_str().unwrap_or("")
    };
    crate::screen::is_system_chrome_window(app_str, title_str)
}

/// Capture GPU-downscaled thumbnails via macOS 14+ `SCScreenshotManager` (returns `None` on
/// macOS 12/13 or if ScreenCaptureKit is unavailable so callers fall back to `xcap`).
pub fn capture_macos_thumbnails(
    monitors_only: bool,
    max_w: u32,
    max_h: u32,
) -> Option<Vec<(String, VideoFrame)>> {
    let mut out = Vec::new();
    // SAFETY: `out` outlives the call and is what `on_thumb_cb` reads `ctx` as; both callbacks
    // have the signatures the native code invokes.
    let ok = unsafe {
        slackless_capture_thumbnails(
            monitors_only,
            max_w,
            max_h,
            (&mut out as *mut Vec<(String, VideoFrame)>).cast(),
            on_thumb_cb,
            Some(should_skip_system_window_cb),
        )
    };
    if ok && !out.is_empty() {
        Some(out)
    } else {
        None
    }
}
