//! Safe Rust wrappers for macOS hardware `ScreenCaptureKit` (`SCStream`) + `VideoToolbox`
//! (`VTCompressionSession` / `VTDecompressionSession`) compiled in `native/macos_media.m`.

#![cfg(target_os = "macos")]

use std::ffi::{CStr, c_char, c_void};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::metrics::HuddlePerfCounters;
use crate::video::VideoFrame;
use crate::{HuddleError, Result};

#[repr(C)]
struct SlacklessScCapture {
    _private: [u8; 0],
}

#[repr(C)]
struct SlacklessVtDecoder {
    _private: [u8; 0],
}

type SlacklessH264Callback = unsafe extern "C" fn(
    ctx: *mut c_void,
    data: *const u8,
    len: usize,
    width: u32,
    height: u32,
    encode_us: u64,
    is_keyframe: bool,
);

type SlacklessPreviewCallback =
    unsafe extern "C" fn(ctx: *mut c_void, bgra: *const u8, len: usize, width: u32, height: u32);

type SlacklessErrorCallback = unsafe extern "C" fn(ctx: *mut c_void, message: *const c_char);

type SlacklessDecodedFrameCallback =
    unsafe extern "C" fn(ctx: *mut c_void, bgra: *const u8, len: usize, width: u32, height: u32);

unsafe extern "C" {
    fn slackless_has_screen_capture_permission() -> bool;
    fn slackless_request_screen_capture_permission() -> bool;
    fn slackless_sc_capture_start(
        is_monitor: u32,
        target_id: u32,
        max_w: u32,
        max_h: u32,
        max_fps: u32,
        bitrate_bps: u32,
        ctx: *mut c_void,
        on_h264: SlacklessH264Callback,
        on_preview: SlacklessPreviewCallback,
        on_error: SlacklessErrorCallback,
        err_buf: *mut c_char,
        err_buf_len: usize,
    ) -> *mut SlacklessScCapture;
    fn slackless_sc_capture_force_keyframe(cap: *mut SlacklessScCapture);
    fn slackless_sc_capture_stop(cap: *mut SlacklessScCapture);

    fn slackless_vt_decoder_new() -> *mut SlacklessVtDecoder;
    fn slackless_vt_decoder_decode(
        dec: *mut SlacklessVtDecoder,
        annex_b: *const u8,
        len: usize,
        ctx: *mut c_void,
        on_frame: SlacklessDecodedFrameCallback,
    ) -> bool;
    fn slackless_vt_decoder_free(dec: *mut SlacklessVtDecoder);
}

/// Returns `true` if macOS Screen Recording permission has been granted to this app bundle.
pub fn has_screen_capture_permission() -> bool {
    unsafe { slackless_has_screen_capture_permission() }
}

/// Triggers the macOS Screen Recording permission prompt (or registers the app in System Settings).
pub fn request_screen_capture_permission() -> bool {
    unsafe { slackless_request_screen_capture_permission() }
}

/// Opens macOS System Settings directly to Privacy & Security -> Screen & System Audio Recording.
pub fn open_screen_capture_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        .spawn();
}

struct CaptureCallbackCtx {
    outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
    preview_tx: mpsc::UnboundedSender<VideoFrame>,
    perf: Arc<HuddlePerfCounters>,
}

unsafe extern "C" fn on_h264_cb(
    ctx: *mut c_void,
    data: *const u8,
    len: usize,
    width: u32,
    height: u32,
    encode_us: u64,
    _is_keyframe: bool,
) {
    if ctx.is_null() || data.is_null() || len == 0 {
        return;
    }
    let state = unsafe { &*(ctx as *const CaptureCallbackCtx) };
    let slice = unsafe { std::slice::from_raw_parts(data, len) };
    state
        .perf
        .record_tx_frame(width, height, len as u64, encode_us);
    let _ = state.outbound_h264_tx.send(slice.to_vec());
}

unsafe extern "C" fn on_preview_cb(
    ctx: *mut c_void,
    bgra: *const u8,
    len: usize,
    width: u32,
    height: u32,
) {
    if ctx.is_null() || bgra.is_null() || len == 0 {
        return;
    }
    let state = unsafe { &*(ctx as *const CaptureCallbackCtx) };
    let slice = unsafe { std::slice::from_raw_parts(bgra, len) };
    let _ = state.preview_tx.send(VideoFrame {
        width,
        height,
        bgra: Arc::new(slice.to_vec()),
    });
}

unsafe extern "C" fn on_error_cb(_ctx: *mut c_void, message: *const c_char) {
    let msg = if message.is_null() {
        "SCStream stopped with unknown error"
    } else {
        unsafe { CStr::from_ptr(message) }
            .to_str()
            .unwrap_or("SCStream stopped")
    };
    tracing::warn!(error = msg, "ScreenCaptureKit stream error");
}

/// Hardware-accelerated macOS `ScreenCaptureKit` + `VideoToolbox` H.264 screen share session.
pub struct ScVtScreenCapture {
    handle: *mut SlacklessScCapture,
    ctx: *mut CaptureCallbackCtx,
}

// SAFETY: `SlacklessScCapture` is thread-safe for `force_keyframe` and stopped once on `Drop`.
unsafe impl Send for ScVtScreenCapture {}

impl ScVtScreenCapture {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        source_id: &str,
        max_w: u32,
        max_h: u32,
        max_fps: u32,
        bitrate_bps: u32,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
        perf: Arc<HuddlePerfCounters>,
    ) -> Result<Self> {
        let (is_monitor, target_id) = if let Some(id) = source_id
            .strip_prefix("monitor:")
            .and_then(|s| s.parse::<u32>().ok())
        {
            (1u32, id)
        } else if let Some(id) = source_id
            .strip_prefix("window:")
            .and_then(|s| s.parse::<u32>().ok())
        {
            (0u32, id)
        } else {
            (1u32, 0u32)
        };

        perf.set_tx_backend("ScreenCaptureKit + VideoToolbox (HW)");
        let ctx = Box::into_raw(Box::new(CaptureCallbackCtx {
            outbound_h264_tx,
            preview_tx,
            perf,
        }));
        let mut err_buf = [0 as c_char; 512];
        let handle = unsafe {
            slackless_sc_capture_start(
                is_monitor,
                target_id,
                max_w,
                max_h,
                max_fps,
                bitrate_bps,
                ctx.cast(),
                on_h264_cb,
                on_preview_cb,
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
            source_id,
            max_w,
            max_h,
            max_fps,
            bitrate_kbps = bitrate_bps / 1000,
            "started macOS ScreenCaptureKit + VideoToolbox HW screen capture"
        );
        Ok(Self { handle, ctx })
    }

    pub fn force_keyframe(&self) {
        if !self.handle.is_null() {
            unsafe {
                slackless_sc_capture_force_keyframe(self.handle);
            }
        }
    }

    pub fn stop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                slackless_sc_capture_stop(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
        if !self.ctx.is_null() {
            unsafe {
                drop(Box::from_raw(self.ctx));
            }
            self.ctx = std::ptr::null_mut();
        }
    }
}

impl Drop for ScVtScreenCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Hardware-accelerated macOS `VideoToolbox` H.264 Annex-B decoder producing BGRA8888 frames.
pub struct VtH264Decoder {
    handle: *mut SlacklessVtDecoder,
}

// SAFETY: `SlacklessVtDecoder` is owned exclusively and accessed via `&mut self`.
unsafe impl Send for VtH264Decoder {}

unsafe extern "C" fn on_decoded_frame_cb(
    ctx: *mut c_void,
    bgra: *const u8,
    len: usize,
    width: u32,
    height: u32,
) {
    if ctx.is_null() || bgra.is_null() || len == 0 {
        return;
    }
    let out = unsafe { &mut *(ctx as *mut Option<VideoFrame>) };
    let slice = unsafe { std::slice::from_raw_parts(bgra, len) };
    *out = Some(VideoFrame {
        width,
        height,
        bgra: Arc::new(slice.to_vec()),
    });
}

impl VtH264Decoder {
    pub fn new() -> Result<Self> {
        let handle = unsafe { slackless_vt_decoder_new() };
        if handle.is_null() {
            return Err(HuddleError::Codec(
                "failed to allocate VideoToolbox decoder".into(),
            ));
        }
        Ok(Self { handle })
    }

    pub fn decode_annex_b(&mut self, annex_b: &[u8]) -> Option<VideoFrame> {
        if self.handle.is_null() || annex_b.is_empty() {
            return None;
        }
        let mut frame: Option<VideoFrame> = None;
        let ok = unsafe {
            slackless_vt_decoder_decode(
                self.handle,
                annex_b.as_ptr(),
                annex_b.len(),
                (&mut frame as *mut Option<VideoFrame>).cast(),
                on_decoded_frame_cb,
            )
        };
        if ok { frame } else { None }
    }
}

impl Drop for VtH264Decoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                slackless_vt_decoder_free(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::H264VideoEncoder;

    #[test]
    fn videotoolbox_decodes_openh264_annex_b_frame_on_macos() {
        let mut enc = H264VideoEncoder::new_screen().expect("openh264 screen encoder");
        let w = 160usize;
        let h = 96usize;
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 3;
                rgb[off] = ((x * 255) / w) as u8;
                rgb[off + 1] = ((y * 255) / h) as u8;
                rgb[off + 2] = 180;
            }
        }
        let bitstream = enc.encode_rgb(&rgb, w, h).expect("encode keyframe");
        assert!(!bitstream.is_empty());

        let mut vt = VtH264Decoder::new().expect("VideoToolbox decoder");
        let decoded = vt
            .decode_annex_b(&bitstream)
            .expect("VideoToolbox should decode H.264 Annex-B keyframe");
        assert_eq!(decoded.width, w as u32);
        assert_eq!(decoded.height, h as u32);
        assert_eq!(decoded.bgra.len(), w * h * 4);
    }
}
