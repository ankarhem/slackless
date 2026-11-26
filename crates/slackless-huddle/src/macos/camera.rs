//! Hardware-accelerated macOS `AVFoundation` webcam capture + `VideoToolbox` H.264 encode +
//! `Vision`/Metal person-segmented background blur.

use std::ffi::{CStr, c_char, c_void};
use std::ptr::NonNull;

use tokio::sync::mpsc;

use super::ffi::{
    CaptureCallbackCtx, SlacklessCamCapture, SlacklessH264Callback, on_error_cb, on_h264_cb,
    on_surface_preview_cb, slackless_cam_capture_force_keyframe, slackless_cam_capture_set_bitrate,
    slackless_cam_capture_set_blur, slackless_cam_capture_set_compact, slackless_cam_capture_start,
    slackless_cam_capture_stop, slackless_list_cameras,
};
use crate::backend::{CameraConfig, VideoFrameSink};
use crate::video::{CameraDeviceInfo, VideoFrame};
use crate::{HuddleError, Result};

pub(super) unsafe extern "C" fn on_cam_enum_cb(ctx: *mut c_void, index: u32, name: *const c_char) {
    if ctx.is_null() {
        return;
    }
    // SAFETY: `ctx` is the `Vec` that `list_macos_cameras` lends `slackless_list_cameras`, which
    // calls back synchronously, one device at a time, before it returns.
    let out = unsafe { &mut *(ctx as *mut Vec<(usize, Option<String>)>) };
    let raw_name = if name.is_null() {
        None
    } else {
        // SAFETY: a non-null `name` is a NUL-terminated string valid for this call.
        unsafe { CStr::from_ptr(name) }
            .to_str()
            .ok()
            .map(str::to_owned)
    };
    out.push((index as usize, raw_name));
}

/// Enumerate cameras via native `AVCaptureDeviceDiscoverySession` (built-in, external, Continuity).
pub fn list_macos_cameras() -> Vec<CameraDeviceInfo> {
    let mut raw: Vec<(usize, Option<String>)> = Vec::new();
    // SAFETY: `raw` outlives the synchronous enumeration and is what `on_cam_enum_cb` reads
    // `ctx` as.
    unsafe {
        slackless_list_cameras(
            (&mut raw as *mut Vec<(usize, Option<String>)>).cast(),
            on_cam_enum_cb,
        );
    }
    crate::video::normalize_camera_devices(raw)
}

/// Hardware-accelerated macOS `AVCaptureSession` (`420f`) + `VideoToolbox` H.264 webcam with
/// Apple Neural Engine (`Vision`) + Metal (`CoreImage`) person-segmented background blur.
pub struct AvVtCameraCapture {
    handle: NonNull<SlacklessCamCapture>,
    ctx: NonNull<CaptureCallbackCtx>,
}

// SAFETY: `SlacklessCamCapture` synchronizes blur/keyframe/bitrate updates and stops once on Drop.
unsafe impl Send for AvVtCameraCapture {}

impl AvVtCameraCapture {
    pub fn start(
        config: &CameraConfig,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        on_preview: VideoFrameSink,
    ) -> Result<Self> {
        Self::start_inner(
            config,
            Some(outbound_h264_tx),
            Box::new(move |frame| on_preview(frame)),
        )
    }

    /// Start native `AVCaptureSession` (`420f`) frame delivery without allocating a `VideoToolbox`
    /// H.264 encoder — ideal for Preferences camera preview probes and `VideoClipRecorder` webcam
    /// capture.
    pub fn start_preview_only(
        device_index: Option<usize>,
        max_w: u32,
        max_h: u32,
        fps: u32,
        blur_background: bool,
        on_preview: impl Fn(VideoFrame) + Send + Sync + 'static,
    ) -> Result<Self> {
        let config = CameraConfig {
            device_index,
            max_w,
            max_h,
            max_fps: fps,
            bitrate_bps: 0,
            blur_background,
        };
        Self::start_inner(&config, None, Box::new(on_preview))
    }

    fn start_inner(
        config: &CameraConfig,
        outbound_h264_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
        on_preview: Box<dyn Fn(VideoFrame) + Send + Sync>,
    ) -> Result<Self> {
        let h264_cb = outbound_h264_tx
            .is_some()
            .then_some(on_h264_cb as SlacklessH264Callback);
        let ctx = NonNull::from(Box::leak(Box::new(CaptureCallbackCtx {
            outbound_h264_tx,
            on_preview,
            perf: None,
        })));
        let mut err_buf = [0 as c_char; 512];
        let dev_idx = config.device_index.map(|i| i as i32).unwrap_or(0);
        // SAFETY: `ctx` stays valid until `Drop` frees it after `slackless_cam_capture_stop`, the
        // callbacks have the signatures the capture invokes, and `err_buf` is writable for its
        // length.
        let handle = unsafe {
            slackless_cam_capture_start(
                dev_idx,
                config.max_w,
                config.max_h,
                config.max_fps,
                config.bitrate_bps,
                config.blur_background,
                ctx.as_ptr().cast(),
                h264_cb,
                on_surface_preview_cb,
                on_error_cb,
                err_buf.as_mut_ptr(),
                err_buf.len(),
            )
        };
        let Some(handle) = NonNull::new(handle) else {
            // SAFETY: the capture did not start, so nothing else holds `ctx`; reclaim the leak.
            unsafe { drop(Box::from_raw(ctx.as_ptr())) };
            // SAFETY: `err_buf` starts zeroed and the native side only `snprintf`s into it, so it
            // holds a NUL-terminated string.
            let msg = unsafe { CStr::from_ptr(err_buf.as_ptr()) }
                .to_str()
                .unwrap_or("AVFoundation camera startup failed")
                .to_owned();
            return Err(HuddleError::Video(msg));
        };
        tracing::info!(
            device_index = dev_idx,
            fps = config.max_fps,
            bitrate_kbps = config.bitrate_bps / 1000,
            blur_background = config.blur_background,
            hw_h264 = h264_cb.is_some(),
            "started macOS AVFoundation (420f) webcam capture"
        );
        Ok(Self { handle, ctx })
    }

    pub fn set_blur(&self, enabled: bool) {
        // SAFETY: `handle` is live until `Drop` stops it.
        unsafe { slackless_cam_capture_set_blur(self.handle.as_ptr(), enabled) };
    }

    pub fn force_keyframe(&self) {
        // SAFETY: `handle` is live until `Drop` stops it.
        unsafe { slackless_cam_capture_force_keyframe(self.handle.as_ptr()) };
    }

    pub fn set_target_bitrate(&self, bitrate_bps: u32) {
        // SAFETY: `handle` is live until `Drop` stops it.
        unsafe { slackless_cam_capture_set_bitrate(self.handle.as_ptr(), bitrate_bps) };
    }

    pub fn set_compact(&self, compact: bool) {
        // SAFETY: `handle` is live until `Drop` stops it.
        unsafe { slackless_cam_capture_set_compact(self.handle.as_ptr(), compact) };
    }
}

impl Drop for AvVtCameraCapture {
    fn drop(&mut self) {
        // SAFETY: `handle` came from a successful start and is stopped once, here.
        unsafe { slackless_cam_capture_stop(self.handle.as_ptr()) };
        // SAFETY: the stop drained the capture and encoder queues, so no callback can still read
        // `ctx`, which `start_inner` leaked from a `Box`.
        unsafe { drop(Box::from_raw(self.ctx.as_ptr())) };
    }
}
