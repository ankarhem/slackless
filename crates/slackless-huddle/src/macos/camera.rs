//! Hardware-accelerated macOS `AVFoundation` webcam capture + `VideoToolbox` H.264 encode +
//! `Vision`/Metal person-segmented background blur.

use std::ffi::{CStr, c_char, c_void};

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
    let out = unsafe { &mut *(ctx as *mut Vec<(usize, Option<String>)>) };
    let raw_name = if name.is_null() {
        None
    } else {
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
    handle: *mut SlacklessCamCapture,
    ctx: *mut CaptureCallbackCtx,
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
        let ctx = Box::into_raw(Box::new(CaptureCallbackCtx {
            outbound_h264_tx,
            on_preview,
            perf: None,
        }));
        let mut err_buf = [0 as c_char; 512];
        let dev_idx = config.device_index.map(|i| i as i32).unwrap_or(0);
        let handle = unsafe {
            slackless_cam_capture_start(
                dev_idx,
                config.max_w,
                config.max_h,
                config.max_fps,
                config.bitrate_bps,
                config.blur_background,
                ctx.cast(),
                h264_cb,
                on_surface_preview_cb,
                on_error_cb,
                err_buf.as_mut_ptr(),
                err_buf.len(),
            )
        };
        if handle.is_null() {
            unsafe { drop(Box::from_raw(ctx)) };
            let msg = unsafe { CStr::from_ptr(err_buf.as_ptr()) }
                .to_str()
                .unwrap_or("AVFoundation camera startup failed")
                .to_owned();
            return Err(HuddleError::Video(msg));
        }
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
        if !self.handle.is_null() {
            unsafe { slackless_cam_capture_set_blur(self.handle, enabled) };
        }
    }

    pub fn force_keyframe(&self) {
        if !self.handle.is_null() {
            unsafe { slackless_cam_capture_force_keyframe(self.handle) };
        }
    }

    pub fn set_target_bitrate(&self, bitrate_bps: u32) {
        if !self.handle.is_null() {
            unsafe { slackless_cam_capture_set_bitrate(self.handle, bitrate_bps) };
        }
    }

    pub fn set_compact(&self, compact: bool) {
        if !self.handle.is_null() {
            unsafe { slackless_cam_capture_set_compact(self.handle, compact) };
        }
    }

    pub fn stop(&mut self) {
        if !self.handle.is_null() {
            unsafe { slackless_cam_capture_stop(self.handle) };
            self.handle = std::ptr::null_mut();
        }
        if !self.ctx.is_null() {
            unsafe { drop(Box::from_raw(self.ctx)) };
            self.ctx = std::ptr::null_mut();
        }
    }
}

impl Drop for AvVtCameraCapture {
    fn drop(&mut self) {
        self.stop();
    }
}
