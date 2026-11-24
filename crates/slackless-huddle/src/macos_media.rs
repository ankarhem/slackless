//! Safe Rust wrappers for macOS hardware media acceleration compiled in `native/macos_media.m`:
//! - Zero-copy `MacVideoSurface` (`IOSurface`-backed `420f` `CVPixelBuffer`) + Accelerate NEON readback
//! - Runtime capability probes (supports all M1/M2/M3/M4 & Intel Macs; never assumes M3+)
//! - `ScreenCaptureKit` (`SCStream`, `420f`) + `VideoToolbox` HW H.264 High/CABAC screen share
//! - `AVFoundation` (`AVCaptureSession`, `420f`) + `VideoToolbox` HW H.264 webcam + Vision/Metal blur
//! - `VideoToolbox` HW H.264 decoder & runtime-gated M3+ HW AV1 decoder (`420f` zero-copy output)
//! - CoreAudio event-driven default input/output device hot-plug monitor

#![cfg(target_os = "macos")]

use std::ffi::{CStr, c_char, c_void};
use std::fmt;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::metrics::HuddlePerfCounters;
use crate::video::{CameraDeviceInfo, VideoFrame};
use crate::{HuddleError, Result};

#[repr(C)]
struct SlacklessScCapture {
    _private: [u8; 0],
}

#[repr(C)]
struct SlacklessCamCapture {
    _private: [u8; 0],
}

#[repr(C)]
struct SlacklessVtDecoder {
    _private: [u8; 0],
}

#[repr(C)]
struct SlacklessVtAv1Decoder {
    _private: [u8; 0],
}

#[repr(C)]
struct SlacklessAudioDeviceMonitor {
    _private: [u8; 0],
}

#[repr(C)]
struct SlacklessVpio {
    _private: [u8; 0],
}

type SlacklessThumbCallback = unsafe extern "C" fn(
    ctx: *mut c_void,
    source_id: *const c_char,
    bgra: *const u8,
    len: usize,
    width: u32,
    height: u32,
);

type SlacklessVpioCaptureCallback =
    unsafe extern "C" fn(ctx: *mut c_void, samples: *const f32, count: usize);
type SlacklessVpioRenderCallback =
    unsafe extern "C" fn(ctx: *mut c_void, samples: *mut f32, count: usize);

type SlacklessH264Callback = unsafe extern "C" fn(
    ctx: *mut c_void,
    data: *const u8,
    len: usize,
    width: u32,
    height: u32,
    encode_us: u64,
    is_keyframe: bool,
);

type SlacklessSurfaceCallback = unsafe extern "C" fn(
    ctx: *mut c_void,
    cv_pixel_buffer_retained: *mut c_void,
    width: u32,
    height: u32,
);

type SlacklessErrorCallback = unsafe extern "C" fn(ctx: *mut c_void, message: *const c_char);

type SlacklessCameraEnumCallback =
    unsafe extern "C" fn(ctx: *mut c_void, index: u32, name: *const c_char);

type SlacklessAudioDeviceChangeCallback = unsafe extern "C" fn(ctx: *mut c_void, is_input: bool);

unsafe extern "C" {
    fn slackless_has_screen_capture_permission() -> bool;
    fn slackless_request_screen_capture_permission() -> bool;
    fn slackless_macos_supports_hw_h264_decode() -> bool;
    fn slackless_macos_supports_hw_av1_decode() -> bool;

    fn slackless_cvpb_retain(pb: *mut c_void) -> *mut c_void;
    fn slackless_cvpb_release(pb: *mut c_void);
    fn slackless_cvpb_dimensions(pb: *mut c_void, out_w: *mut u32, out_h: *mut u32);
    fn slackless_cvpb_to_bgra(
        pb: *mut c_void,
        max_w: u32,
        max_h: u32,
        out_buf: *mut *mut u8,
        out_len: *mut usize,
        out_w: *mut u32,
        out_h: *mut u32,
    ) -> bool;
    fn slackless_free_buf(buf: *mut u8);

    fn slackless_sc_capture_start(
        is_monitor: u32,
        target_id: u32,
        max_w: u32,
        max_h: u32,
        max_fps: u32,
        bitrate_bps: u32,
        ctx: *mut c_void,
        on_h264: SlacklessH264Callback,
        on_surface_preview: SlacklessSurfaceCallback,
        on_error: SlacklessErrorCallback,
        err_buf: *mut c_char,
        err_buf_len: usize,
    ) -> *mut SlacklessScCapture;
    fn slackless_sc_capture_force_keyframe(cap: *mut SlacklessScCapture);
    fn slackless_sc_capture_set_bitrate(cap: *mut SlacklessScCapture, bitrate_bps: u32);
    fn slackless_sc_capture_stop(cap: *mut SlacklessScCapture);

    fn slackless_list_cameras(ctx: *mut c_void, cb: SlacklessCameraEnumCallback);
    fn slackless_cam_capture_start(
        device_index: i32,
        max_w: u32,
        max_h: u32,
        fps: u32,
        bitrate_bps: u32,
        blur_enabled: bool,
        ctx: *mut c_void,
        on_h264: SlacklessH264Callback,
        on_surface_preview: SlacklessSurfaceCallback,
        on_error: SlacklessErrorCallback,
        err_buf: *mut c_char,
        err_buf_len: usize,
    ) -> *mut SlacklessCamCapture;
    fn slackless_cam_capture_set_blur(cap: *mut SlacklessCamCapture, enabled: bool);
    fn slackless_cam_capture_force_keyframe(cap: *mut SlacklessCamCapture);
    fn slackless_cam_capture_set_bitrate(cap: *mut SlacklessCamCapture, bitrate_bps: u32);
    fn slackless_cam_capture_set_compact(cap: *mut SlacklessCamCapture, compact: bool);
    fn slackless_cam_capture_stop(cap: *mut SlacklessCamCapture);

    fn slackless_vt_decoder_new() -> *mut SlacklessVtDecoder;
    fn slackless_vt_decoder_decode(
        dec: *mut SlacklessVtDecoder,
        annex_b: *const u8,
        len: usize,
        ctx: *mut c_void,
        on_surface: SlacklessSurfaceCallback,
    ) -> bool;
    fn slackless_vt_decoder_free(dec: *mut SlacklessVtDecoder);

    fn slackless_vt_av1_decoder_new() -> *mut SlacklessVtAv1Decoder;
    fn slackless_vt_av1_decoder_decode(
        dec: *mut SlacklessVtAv1Decoder,
        tu: *const u8,
        len: usize,
        ctx: *mut c_void,
        on_surface: SlacklessSurfaceCallback,
    ) -> bool;
    fn slackless_vt_av1_decoder_free(dec: *mut SlacklessVtAv1Decoder);

    fn slackless_audio_device_monitor_start(
        ctx: *mut c_void,
        cb: SlacklessAudioDeviceChangeCallback,
    ) -> *mut SlacklessAudioDeviceMonitor;
    fn slackless_audio_device_monitor_stop(mon: *mut SlacklessAudioDeviceMonitor);

    fn slackless_capture_thumbnails(
        monitors_only: bool,
        max_w: u32,
        max_h: u32,
        ctx: *mut c_void,
        cb: SlacklessThumbCallback,
    ) -> bool;

    fn slackless_vpio_start(
        sample_rate: u32,
        enable_vp: bool,
        enable_agc: bool,
        ctx: *mut c_void,
        on_capture: SlacklessVpioCaptureCallback,
        on_render: SlacklessVpioRenderCallback,
    ) -> *mut SlacklessVpio;
    fn slackless_vpio_set_processing(vpio: *mut SlacklessVpio, enable_vp: bool, enable_agc: bool);
    fn slackless_vpio_stop(vpio: *mut SlacklessVpio);

    fn slackless_audio_file_play_oneshot(
        path: *const c_char,
        device_name: *const c_char,
        volume: f32,
    ) -> bool;
    fn slackless_audio_file_start_loop(
        path: *const c_char,
        device_name: *const c_char,
        volume: f32,
    ) -> *mut SlacklessAudioHandle;
    fn slackless_audio_file_stop_loop(handle: *mut SlacklessAudioHandle);

    fn slackless_draw_overlay_update(
        is_monitor: bool,
        target_id: u32,
        segs: *const OverlaySegmentFfi,
        seg_count: usize,
        cur: *const OverlayCursorFfi,
        cur_count: usize,
    );
    fn slackless_draw_overlay_hide();
}

#[repr(C)]
struct SlacklessAudioHandle {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OverlaySegmentFfi {
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

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OverlayCursorFfi {
    pub x: f64,
    pub y: f64,
    pub hue: i32,
    pub alpha: f32,
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

/// Runtime probe: does this Mac support hardware H.264 decoding via VideoToolbox?
pub fn supports_hw_h264_decode() -> bool {
    unsafe { slackless_macos_supports_hw_h264_decode() }
}

/// Runtime probe: does this Mac support hardware AV1 decoding via VideoToolbox (Apple Silicon M3+
/// on macOS 14+)? Safely returns `false` on M1/M2/Intel Macs.
pub fn supports_hw_av1_decode() -> bool {
    unsafe { slackless_macos_supports_hw_av1_decode() }
}

/// Zero-copy retained handle to a macOS `IOSurface`-backed `420f` `CVPixelBuffer`.
///
/// Passed directly to GPUI's `window.paint_surface` on Metal with zero CPU pixel readback, and
/// lazily convertible to BGRA8888 via NEON `Accelerate`/`vImage` when a CPU image is needed.
pub struct MacVideoSurface {
    pb: *mut c_void,
    width: u32,
    height: u32,
}

// SAFETY: An immutable `CVPixelBuffer` backed by an `IOSurface` is reference-counted via
// `CFRetain`/`CFRelease` and safe to pass across threads for Metal read sampling or read-only lock.
unsafe impl Send for MacVideoSurface {}
unsafe impl Sync for MacVideoSurface {}

impl fmt::Debug for MacVideoSurface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MacVideoSurface")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

impl MacVideoSurface {
    /// Wrap an already-retained `CVPixelBufferRef` (transfers +1 retain ownership).
    ///
    /// # Safety
    /// `pb` must be a non-null, retained `CVPixelBufferRef`.
    unsafe fn from_retained(pb: *mut c_void, width: u32, height: u32) -> Option<Self> {
        if pb.is_null() {
            return None;
        }
        let (mut w, mut h) = (width, height);
        if w == 0 || h == 0 {
            unsafe {
                slackless_cvpb_dimensions(pb, &mut w, &mut h);
            }
        }
        Some(Self {
            pb,
            width: w,
            height: h,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Returns a newly `CFRetain`ed `CVPixelBufferRef` pointer for GPUI `CFRetained::from_raw`.
    /// Caller owns the +1 retain count and MUST release it (e.g. via `CFRetained::from_raw`).
    pub fn retain_cv_pixel_buffer_ptr(&self) -> *mut c_void {
        unsafe { slackless_cvpb_retain(self.pb) }
    }

    /// Convert (and optionally high-quality downscale) this GPU surface to tightly-packed BGRA8888
    /// using Apple's NEON-accelerated `vImage`. Pass `0, 0` for native dimensions.
    pub fn to_bgra_fit(&self, max_w: u32, max_h: u32) -> Option<(u32, u32, Vec<u8>)> {
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        let mut out_w: u32 = 0;
        let mut out_h: u32 = 0;
        let ok = unsafe {
            slackless_cvpb_to_bgra(
                self.pb, max_w, max_h, &mut ptr, &mut len, &mut out_w, &mut out_h,
            )
        };
        if !ok || ptr.is_null() || len == 0 {
            if !ptr.is_null() {
                unsafe { slackless_free_buf(ptr) };
            }
            return None;
        }
        let vec = unsafe { std::slice::from_raw_parts(ptr, len).to_vec() };
        unsafe { slackless_free_buf(ptr) };
        Some((out_w, out_h, vec))
    }
}

impl Drop for MacVideoSurface {
    fn drop(&mut self) {
        if !self.pb.is_null() {
            unsafe { slackless_cvpb_release(self.pb) };
            self.pb = std::ptr::null_mut();
        }
    }
}

// ---------------------------------------------------------------------------
// ScreenCaptureKit + VideoToolbox HW Screen Share
// ---------------------------------------------------------------------------

struct CaptureCallbackCtx {
    outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
    preview_tx: mpsc::UnboundedSender<VideoFrame>,
    perf: Option<Arc<HuddlePerfCounters>>,
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
    if let Some(perf) = &state.perf {
        perf.record_tx_frame(width, height, len as u64, encode_us);
    }
    let _ = state.outbound_h264_tx.send(slice.to_vec());
}

unsafe extern "C" fn on_surface_preview_cb(
    ctx: *mut c_void,
    cv_pb: *mut c_void,
    width: u32,
    height: u32,
) {
    if ctx.is_null() || cv_pb.is_null() {
        if !cv_pb.is_null() {
            unsafe { slackless_cvpb_release(cv_pb) };
        }
        return;
    }
    let state = unsafe { &*(ctx as *const CaptureCallbackCtx) };
    if let Some(surface) = unsafe { MacVideoSurface::from_retained(cv_pb, width, height) } {
        let _ = state.preview_tx.send(VideoFrame::from_mac_surface(surface));
    }
}

unsafe extern "C" fn on_error_cb(_ctx: *mut c_void, message: *const c_char) {
    let msg = if message.is_null() {
        "macOS capture stream stopped with unknown error"
    } else {
        unsafe { CStr::from_ptr(message) }
            .to_str()
            .unwrap_or("macOS capture stream stopped")
    };
    tracing::warn!(error = msg, "macOS hardware media capture error");
}

/// Hardware-accelerated macOS `ScreenCaptureKit` (`420f`) + `VideoToolbox` H.264 High/CABAC screen share.
pub struct ScVtScreenCapture {
    handle: *mut SlacklessScCapture,
    ctx: *mut CaptureCallbackCtx,
}

// SAFETY: `SlacklessScCapture` is thread-safe for keyframe/bitrate control and stopped once on `Drop`.
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

        perf.set_tx_backend("ScreenCaptureKit + VideoToolbox H.264 High (HW)");
        let ctx = Box::into_raw(Box::new(CaptureCallbackCtx {
            outbound_h264_tx,
            preview_tx,
            perf: Some(perf),
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
            source_id,
            max_w,
            max_h,
            max_fps,
            bitrate_kbps = bitrate_bps / 1000,
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

// ---------------------------------------------------------------------------
// AVFoundation + VideoToolbox HW Webcam + Vision/Metal Person Blur
// ---------------------------------------------------------------------------

unsafe extern "C" fn on_cam_enum_cb(ctx: *mut c_void, index: u32, name: *const c_char) {
    if ctx.is_null() {
        return;
    }
    let out = unsafe { &mut *(ctx as *mut Vec<CameraDeviceInfo>) };
    let name_str = if name.is_null() {
        format!("Camera {}", index + 1)
    } else {
        unsafe { CStr::from_ptr(name) }
            .to_str()
            .unwrap_or("Camera")
            .to_owned()
    };
    out.push(CameraDeviceInfo {
        index: index as usize,
        name: name_str,
    });
}

/// Enumerate cameras via native `AVCaptureDeviceDiscoverySession` (built-in, external, Continuity).
pub fn list_macos_cameras() -> Vec<CameraDeviceInfo> {
    let mut out = Vec::new();
    unsafe {
        slackless_list_cameras(
            (&mut out as *mut Vec<CameraDeviceInfo>).cast(),
            on_cam_enum_cb,
        );
    }
    out
}

/// Hardware-accelerated macOS `AVCaptureSession` (`420f`) + `VideoToolbox` H.264 webcam with
/// Apple Neural Engine (`Vision`) + Metal (`CoreImage`) person-segmented background blur.
pub struct AvVtCameraCapture {
    handle: *mut SlacklessCamCapture,
    ctx: *mut CaptureCallbackCtx,
}

// SAFETY: `SlacklessCamCapture` handles atomic blur/keyframe/bitrate updates and stops once on Drop.
unsafe impl Send for AvVtCameraCapture {}

impl AvVtCameraCapture {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        device_index: Option<usize>,
        max_w: u32,
        max_h: u32,
        fps: u32,
        bitrate_bps: u32,
        blur_background: bool,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
    ) -> Result<Self> {
        let ctx = Box::into_raw(Box::new(CaptureCallbackCtx {
            outbound_h264_tx,
            preview_tx,
            perf: None,
        }));
        let mut err_buf = [0 as c_char; 512];
        let dev_idx = device_index.map(|i| i as i32).unwrap_or(0);
        let handle = unsafe {
            slackless_cam_capture_start(
                dev_idx,
                max_w,
                max_h,
                fps,
                bitrate_bps,
                blur_background,
                ctx.cast(),
                on_h264_cb,
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
            fps,
            bitrate_kbps = bitrate_bps / 1000,
            blur_background,
            "started macOS AVFoundation (420f) + VideoToolbox H.264 High HW webcam"
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

// ---------------------------------------------------------------------------
// VideoToolbox Hardware H.264 & AV1 Decoders (Zero-Copy `420f` Surface Output)
// ---------------------------------------------------------------------------

unsafe extern "C" fn on_decoded_surface_cb(
    ctx: *mut c_void,
    cv_pb: *mut c_void,
    width: u32,
    height: u32,
) {
    if ctx.is_null() || cv_pb.is_null() {
        if !cv_pb.is_null() {
            unsafe { slackless_cvpb_release(cv_pb) };
        }
        return;
    }
    let out = unsafe { &mut *(ctx as *mut Option<VideoFrame>) };
    if let Some(surface) = unsafe { MacVideoSurface::from_retained(cv_pb, width, height) } {
        *out = Some(VideoFrame::from_mac_surface(surface));
    }
}

/// Hardware-accelerated macOS `VideoToolbox` H.264 Annex-B decoder producing zero-copy `420f` surfaces.
pub struct VtH264Decoder {
    handle: *mut SlacklessVtDecoder,
}

// SAFETY: `SlacklessVtDecoder` is owned exclusively and accessed via `&mut self`.
unsafe impl Send for VtH264Decoder {}

impl VtH264Decoder {
    pub fn new() -> Result<Self> {
        let handle = unsafe { slackless_vt_decoder_new() };
        if handle.is_null() {
            return Err(HuddleError::Codec(
                "failed to allocate VideoToolbox H.264 decoder".into(),
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
                on_decoded_surface_cb,
            )
        };
        if ok { frame } else { None }
    }
}

impl Drop for VtH264Decoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { slackless_vt_decoder_free(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

/// Runtime-gated macOS `VideoToolbox` hardware AV1 decoder (M3+ on macOS 14+; returns `Err` on
/// M1/M2 so callers cleanly fall back to `rav1d`). Produces zero-copy `420f` surfaces.
pub struct VtAv1Decoder {
    handle: *mut SlacklessVtAv1Decoder,
}

// SAFETY: `SlacklessVtAv1Decoder` is owned exclusively and accessed via `&mut self`.
unsafe impl Send for VtAv1Decoder {}

impl VtAv1Decoder {
    pub fn new() -> Result<Self> {
        let handle = unsafe { slackless_vt_av1_decoder_new() };
        if handle.is_null() {
            return Err(HuddleError::Codec(
                "hardware AV1 decode not supported on this Mac".into(),
            ));
        }
        Ok(Self { handle })
    }

    pub fn decode_temporal_unit(&mut self, unit: &[u8]) -> Option<VideoFrame> {
        if self.handle.is_null() || unit.is_empty() {
            return None;
        }
        let mut frame: Option<VideoFrame> = None;
        let ok = unsafe {
            slackless_vt_av1_decoder_decode(
                self.handle,
                unit.as_ptr(),
                unit.len(),
                (&mut frame as *mut Option<VideoFrame>).cast(),
                on_decoded_surface_cb,
            )
        };
        if ok { frame } else { None }
    }
}

impl Drop for VtAv1Decoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { slackless_vt_av1_decoder_free(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

// ---------------------------------------------------------------------------
// CoreAudio Event-Driven Default Audio Device Hot-Plug Monitor
// ---------------------------------------------------------------------------

struct AudioHotplugCtx {
    callback: Box<dyn Fn(bool) + Send + Sync>,
}

unsafe extern "C" fn on_audio_device_change_cb(ctx: *mut c_void, is_input: bool) {
    if ctx.is_null() {
        return;
    }
    let state = unsafe { &*(ctx as *const AudioHotplugCtx) };
    (state.callback)(is_input);
}

/// Event-driven CoreAudio listener for default microphone/speaker hot-plug (zero polling).
pub struct CoreAudioDeviceMonitor {
    handle: *mut SlacklessAudioDeviceMonitor,
    ctx: *mut AudioHotplugCtx,
}

// SAFETY: Stopped and joined synchronously on Drop before freeing `ctx`.
unsafe impl Send for CoreAudioDeviceMonitor {}

impl CoreAudioDeviceMonitor {
    pub fn start(on_change: impl Fn(bool) + Send + Sync + 'static) -> Option<Self> {
        let ctx = Box::into_raw(Box::new(AudioHotplugCtx {
            callback: Box::new(on_change),
        }));
        let handle =
            unsafe { slackless_audio_device_monitor_start(ctx.cast(), on_audio_device_change_cb) };
        if handle.is_null() {
            unsafe { drop(Box::from_raw(ctx)) };
            return None;
        }
        Some(Self { handle, ctx })
    }
}

impl Drop for CoreAudioDeviceMonitor {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { slackless_audio_device_monitor_stop(self.handle) };
            self.handle = std::ptr::null_mut();
        }
        if !self.ctx.is_null() {
            unsafe { drop(Box::from_raw(self.ctx)) };
            self.ctx = std::ptr::null_mut();
        }
    }
}

// ---------------------------------------------------------------------------
// GPU-Downscaled Screen/Window Thumbnails (`SCScreenshotManager`, macOS 14+)
// ---------------------------------------------------------------------------

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
        )
    };
    if ok && !out.is_empty() {
        Some(out)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// CoreAudio Native `VoiceProcessingIO` Hardware-Tuned Full-Duplex Audio
// ---------------------------------------------------------------------------

type BoxedCaptureCb = Box<dyn Fn(&[f32]) + Send + Sync>;
type BoxedRenderCb = Box<dyn Fn(&mut [f32]) + Send + Sync>;

struct VpioCtx {
    on_capture: BoxedCaptureCb,
    on_render: BoxedRenderCb,
}

unsafe extern "C" fn on_vpio_capture_cb(ctx: *mut c_void, samples: *const f32, count: usize) {
    if ctx.is_null() || samples.is_null() || count == 0 {
        return;
    }
    let state = unsafe { &*(ctx as *const VpioCtx) };
    let slice = unsafe { std::slice::from_raw_parts(samples, count) };
    (state.on_capture)(slice);
}

unsafe extern "C" fn on_vpio_render_cb(ctx: *mut c_void, samples: *mut f32, count: usize) {
    if ctx.is_null() || samples.is_null() || count == 0 {
        return;
    }
    let state = unsafe { &*(ctx as *const VpioCtx) };
    let slice = unsafe { std::slice::from_raw_parts_mut(samples, count) };
    (state.on_render)(slice);
}

/// Native macOS CoreAudio `kAudioUnitSubType_VoiceProcessingIO` full-duplex 48 kHz mono stream
/// with hardware-tuned AEC, noise suppression, and AGC (skips software `sonora` DSP completely).
pub struct MacVoiceProcessingIo {
    handle: *mut SlacklessVpio,
    ctx: *mut VpioCtx,
}

// SAFETY: `slackless_vpio_stop` halts and disposes the AudioUnit synchronously before freeing `ctx`.
unsafe impl Send for MacVoiceProcessingIo {}

impl MacVoiceProcessingIo {
    pub fn start(
        sample_rate: u32,
        enable_vp: bool,
        enable_agc: bool,
        on_capture: impl Fn(&[f32]) + Send + Sync + 'static,
        on_render: impl Fn(&mut [f32]) + Send + Sync + 'static,
    ) -> Result<Self> {
        let ctx = Box::into_raw(Box::new(VpioCtx {
            on_capture: Box::new(on_capture),
            on_render: Box::new(on_render),
        }));
        let handle = unsafe {
            slackless_vpio_start(
                sample_rate,
                enable_vp,
                enable_agc,
                ctx.cast(),
                on_vpio_capture_cb,
                on_vpio_render_cb,
            )
        };
        if handle.is_null() {
            unsafe { drop(Box::from_raw(ctx)) };
            return Err(HuddleError::Audio(
                "CoreAudio VoiceProcessingIO unavailable".into(),
            ));
        }
        tracing::info!(
            sample_rate,
            enable_vp,
            enable_agc,
            "started macOS CoreAudio VoiceProcessingIO (hardware AEC/NS/AGC)"
        );
        Ok(Self { handle, ctx })
    }

    pub fn set_processing(&self, enable_vp: bool, enable_agc: bool) {
        if !self.handle.is_null() {
            unsafe { slackless_vpio_set_processing(self.handle, enable_vp, enable_agc) };
        }
    }
}

impl Drop for MacVoiceProcessingIo {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { slackless_vpio_stop(self.handle) };
            self.handle = std::ptr::null_mut();
        }
        if !self.ctx.is_null() {
            unsafe { drop(Box::from_raw(self.ctx)) };
            self.ctx = std::ptr::null_mut();
        }
    }
}

/// Play a one-shot audio file (`.mp3` / `.wav`) via native `NSSound` routed to the exact
/// CoreAudio output device matching `device_name` (or system default when `None`).
pub fn play_routed_audio_oneshot(
    path: &std::path::Path,
    device_name: Option<&str>,
    volume: f32,
) -> bool {
    let Ok(c_path) = std::ffi::CString::new(path.to_string_lossy().as_bytes()) else {
        return false;
    };
    let c_dev = device_name
        .filter(|s| !s.is_empty())
        .and_then(|s| std::ffi::CString::new(s).ok());
    let dev_ptr = c_dev.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
    unsafe { slackless_audio_file_play_oneshot(c_path.as_ptr(), dev_ptr, volume) }
}

/// Active looping `NSSound` playback handle routed to a specific CoreAudio output device.
pub struct MacRoutedAudioLoop {
    handle: *mut SlacklessAudioHandle,
}

// SAFETY: `slackless_audio_file_stop_loop` dispatches stop/release onto the main queue safely.
unsafe impl Send for MacRoutedAudioLoop {}

impl MacRoutedAudioLoop {
    pub fn start(path: &std::path::Path, device_name: Option<&str>, volume: f32) -> Option<Self> {
        let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes()).ok()?;
        let c_dev = device_name
            .filter(|s| !s.is_empty())
            .and_then(|s| std::ffi::CString::new(s).ok());
        let dev_ptr = c_dev.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
        let handle = unsafe { slackless_audio_file_start_loop(c_path.as_ptr(), dev_ptr, volume) };
        (!handle.is_null()).then_some(Self { handle })
    }

    pub fn stop(&mut self) {
        if !self.handle.is_null() {
            unsafe { slackless_audio_file_stop_loop(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

impl Drop for MacRoutedAudioLoop {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Update (or hide when empty) the click-through transparent desktop drawing overlay positioned
/// over the shared monitor (`monitor:<id>`) or window (`window:<id>`).
pub fn update_desktop_draw_overlay(
    source_id: &str,
    segments: &[OverlaySegmentFfi],
    cursors: &[OverlayCursorFfi],
) {
    let (is_monitor, target_id) = if let Some(rest) = source_id.strip_prefix("monitor:") {
        let Ok(id) = rest.parse::<u32>() else { return };
        (true, id)
    } else if let Some(rest) = source_id.strip_prefix("window:") {
        let Ok(id) = rest.parse::<u32>() else { return };
        (false, id)
    } else {
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

/// Immediately hide and clear the click-through desktop drawing overlay.
pub fn hide_desktop_draw_overlay() {
    unsafe { slackless_draw_overlay_hide() };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::H264VideoEncoder;

    #[test]
    fn videotoolbox_decodes_openh264_annex_b_to_zero_copy_surface_and_bgra() {
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
        assert!(
            decoded.mac_surface().is_some(),
            "should return zero-copy MacVideoSurface"
        );
        let (bw, bh, bgra) = decoded
            .to_bgra_fit(0, 0)
            .expect("VTPixelTransferSession / vImage conversion to BGRA should succeed");
        assert_eq!((bw, bh), (w as u32, h as u32));
        assert_eq!(bgra.len(), w * h * 4);

        // Also verify hardware downscaling preserves exact BGRA channel ordering (top-right pixel
        // has high R, low G, high B; bottom-left has low R, high G, high B).
        let (sw, sh, small_bgra) = decoded
            .to_bgra_fit(80, 48)
            .expect("VTPixelTransferSession hardware downscale to BGRA should succeed");
        assert_eq!((sw, sh), (80, 48));
        assert_eq!(small_bgra.len(), 80 * 48 * 4);
        // Top-right corner: R ≈ 255, G ≈ 0, B ≈ 180 -> BGRA = [B≈180, G≈0, R≈255, A=255]
        let tr = ((2 * sw as usize) + (sw as usize - 3)) * 4;
        assert!(
            small_bgra[tr + 2] > 180 && small_bgra[tr + 1] < 60 && small_bgra[tr] > 110,
            "expected BGRA channel order at top-right, got {:?}",
            &small_bgra[tr..tr + 4]
        );
        // Bottom-left corner: R ≈ 0, G ≈ 255, B ≈ 180 -> BGRA = [B≈180, G≈255, R≈0, A=255]
        let bl = (((sh as usize - 3) * sw as usize) + 2) * 4;
        assert!(
            small_bgra[bl + 1] > 180 && small_bgra[bl + 2] < 60 && small_bgra[bl] > 110,
            "expected BGRA channel order at bottom-left, got {:?}",
            &small_bgra[bl..bl + 4]
        );
    }

    #[test]
    fn hw_av1_probe_matches_vt_av1_decoder_availability() {
        let supported = supports_hw_av1_decode();
        let dec = VtAv1Decoder::new();
        assert_eq!(dec.is_ok(), supported);
    }

    #[test]
    fn draw_overlay_handles_empty_and_invalid_sources_safely() {
        // Invalid prefixes and unparseable IDs must safely no-op without calling into AppKit.
        update_desktop_draw_overlay("invalid:abc", &[], &[]);
        update_desktop_draw_overlay("monitor:notanumber", &[], &[]);
        update_desktop_draw_overlay("window:notanumber", &[], &[]);
        hide_desktop_draw_overlay();
    }
}
