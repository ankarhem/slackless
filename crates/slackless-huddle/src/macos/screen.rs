//! Hardware-accelerated macOS `ScreenCaptureKit` screen capture and `SCScreenshotManager` thumbnails.

use std::ffi::{CStr, c_char, c_void};
use std::sync::Arc;

use tokio::sync::mpsc;

use super::ffi::{
    CaptureCallbackCtx, SlacklessScCapture, on_error_cb, on_h264_cb, on_surface_preview_cb,
    slackless_capture_thumbnails, slackless_sc_capture_force_keyframe,
    slackless_sc_capture_set_bitrate, slackless_sc_capture_start, slackless_sc_capture_stop,
};
use crate::backend::ScreenShareConfig;
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
    handle: *mut SlacklessScCapture,
    ctx: *mut CaptureCallbackCtx,
}

// SAFETY: `SlacklessScCapture` is thread-safe for keyframe/bitrate control and stopped once on `Drop`.
unsafe impl Send for ScVtScreenCapture {}

impl ScVtScreenCapture {
    pub fn start(
        config: &ScreenShareConfig,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
        perf: Arc<HuddlePerfCounters>,
    ) -> Result<Self> {
        let (is_monitor, target_id) = match parse_screen_source_id(&config.source_id) {
            Some((true, id)) => (1u32, id),
            Some((false, id)) => (0u32, id),
            None => (1u32, 0u32),
        };

        perf.set_tx_backend("ScreenCaptureKit + VideoToolbox H.264 High (HW)");
        let ctx = Box::into_raw(Box::new(CaptureCallbackCtx {
            outbound_h264_tx: Some(outbound_h264_tx),
            on_preview: Box::new(move |frame| {
                let _ = preview_tx.send(frame);
            }),
            perf: Some(perf),
        }));
        let mut err_buf = [0 as c_char; 512];
        let handle = unsafe {
            slackless_sc_capture_start(
                is_monitor,
                target_id,
                config.max_w,
                config.max_h,
                config.max_fps,
                config.bitrate_bps,
                ctx.cast(),
                on_h264_cb,
                on_surface_preview_cb,
                on_error_cb,
                err_buf.as_mut_ptr(),
                err_buf.len(),
            )
        };
        if handle.is_null() {
            unsafe {
                drop(Box::from_raw(ctx));
            }
            let msg = unsafe { CStr::from_ptr(err_buf.as_ptr()) }
                .to_str()
                .unwrap_or("ScreenCaptureKit startup failed")
                .to_owned();
            return Err(HuddleError::Video(msg));
        }
        tracing::info!(
            source_id = %config.source_id,
            max_w = config.max_w,
            max_h = config.max_h,
            max_fps = config.max_fps,
            bitrate_kbps = config.bitrate_bps / 1000,
            "started macOS ScreenCaptureKit (420f) + VideoToolbox H.264 High HW screen capture"
        );
        Ok(Self { handle, ctx })
    }

    pub fn force_keyframe(&self) {
        if !self.handle.is_null() {
            unsafe { slackless_sc_capture_force_keyframe(self.handle) };
        }
    }

    pub fn set_target_bitrate(&self, bitrate_bps: u32) {
        if !self.handle.is_null() {
            unsafe { slackless_sc_capture_set_bitrate(self.handle, bitrate_bps) };
        }
    }

    pub fn stop(&mut self) {
        if !self.handle.is_null() {
            unsafe { slackless_sc_capture_stop(self.handle) };
            self.handle = std::ptr::null_mut();
        }
        if !self.ctx.is_null() {
            unsafe { drop(Box::from_raw(self.ctx)) };
            self.ctx = std::ptr::null_mut();
        }
    }
}

impl Drop for ScVtScreenCapture {
    fn drop(&mut self) {
        self.stop();
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
    let out = unsafe { &mut *(ctx as *mut Vec<(String, VideoFrame)>) };
    let Ok(id) = unsafe { CStr::from_ptr(source_id) }.to_str() else {
        return;
    };
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
        unsafe { CStr::from_ptr(app) }.to_str().unwrap_or("")
    };
    let title_str = if title.is_null() {
        ""
    } else {
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
