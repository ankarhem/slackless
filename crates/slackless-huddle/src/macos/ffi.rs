//! Raw C FFI declarations for `native/macos/*.m`.

use std::ffi::{CStr, c_char, c_void};
use std::sync::Arc;

use tokio::sync::mpsc;

use super::pixel::MacVideoSurface;
use crate::metrics::HuddlePerfCounters;
use crate::video::VideoFrame;

#[repr(C)]
pub(crate) struct SlacklessScCapture {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct SlacklessCamCapture {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct SlacklessVtDecoder {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct SlacklessVtAv1Decoder {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct SlacklessAudioDeviceMonitor {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct SlacklessVpio {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct SlacklessAudioHandle {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct SlacklessAudioTap {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct SlacklessVideoPlayer {
    _private: [u8; 0],
}

pub(crate) type SlacklessThumbCallback = unsafe extern "C" fn(
    ctx: *mut c_void,
    source_id: *const c_char,
    bgra: *const u8,
    len: usize,
    width: u32,
    height: u32,
);

pub(crate) type SlacklessWindowSkipCallback =
    unsafe extern "C" fn(app: *const c_char, title: *const c_char) -> bool;

pub(crate) type SlacklessVpioCaptureCallback =
    unsafe extern "C" fn(ctx: *mut c_void, samples: *const f32, count: usize);

pub(crate) type SlacklessVpioRenderCallback =
    unsafe extern "C" fn(ctx: *mut c_void, samples: *mut f32, count: usize);

pub(crate) type SlacklessAudioTapCallback =
    unsafe extern "C" fn(ctx: *mut c_void, stereo_pcm: *const i16, count: usize);

pub(crate) type SlacklessH264Callback = unsafe extern "C" fn(
    ctx: *mut c_void,
    data: *const u8,
    len: usize,
    width: u32,
    height: u32,
    encode_us: u64,
    is_keyframe: bool,
);

pub(crate) type SlacklessSurfaceCallback = unsafe extern "C" fn(
    ctx: *mut c_void,
    cv_pixel_buffer_retained: *mut c_void,
    width: u32,
    height: u32,
);

pub(crate) type SlacklessErrorCallback =
    unsafe extern "C" fn(ctx: *mut c_void, message: *const c_char);

/// Invoked by the native capture once it is stopped and freed, so `ctx` can be released knowing
/// no callback can dereference it anymore.
pub(crate) type SlacklessCaptureReleasedCallback = unsafe extern "C" fn(ctx: *mut c_void);

pub(crate) type SlacklessCameraEnumCallback =
    unsafe extern "C" fn(ctx: *mut c_void, index: u32, name: *const c_char);

pub(crate) type SlacklessAudioDeviceChangeCallback =
    unsafe extern "C" fn(ctx: *mut c_void, is_input: bool);

unsafe extern "C" {
    // Argument-free capability and permission queries: nothing for a caller to uphold.
    pub(crate) safe fn slackless_has_screen_capture_permission() -> bool;
    pub(crate) safe fn slackless_request_screen_capture_permission() -> bool;
    pub(crate) safe fn slackless_macos_supports_hw_h264_decode() -> bool;
    pub(crate) safe fn slackless_macos_supports_hw_av1_decode() -> bool;
    pub(crate) safe fn slackless_audio_tap_is_supported() -> bool;

    pub(crate) fn slackless_cvpb_retain(pb: *mut c_void) -> *mut c_void;
    pub(crate) fn slackless_cvpb_release(pb: *mut c_void);
    pub(crate) fn slackless_cvpb_dimensions(pb: *mut c_void, out_w: *mut u32, out_h: *mut u32);
    pub(crate) fn slackless_cvpb_to_bgra(
        pb: *mut c_void,
        max_w: u32,
        max_h: u32,
        out_buf: *mut *mut u8,
        out_len: *mut usize,
        out_w: *mut u32,
        out_h: *mut u32,
    ) -> bool;
    pub(crate) fn slackless_free_buf(buf: *mut u8);

    pub(crate) fn slackless_sc_capture_start(
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
    pub(crate) fn slackless_sc_capture_force_keyframe(cap: *mut SlacklessScCapture);
    pub(crate) fn slackless_sc_capture_set_bitrate(cap: *mut SlacklessScCapture, bitrate_bps: u32);
    pub(crate) fn slackless_sc_capture_stop(
        cap: *mut SlacklessScCapture,
        ctx: *mut c_void,
        on_released: Option<SlacklessCaptureReleasedCallback>,
    );

    pub(crate) fn slackless_list_cameras(ctx: *mut c_void, cb: SlacklessCameraEnumCallback);
    pub(crate) fn slackless_cam_capture_start(
        device_index: i32,
        max_w: u32,
        max_h: u32,
        fps: u32,
        bitrate_bps: u32,
        blur_enabled: bool,
        ctx: *mut c_void,
        on_h264: Option<SlacklessH264Callback>,
        on_surface_preview: SlacklessSurfaceCallback,
        on_error: SlacklessErrorCallback,
        err_buf: *mut c_char,
        err_buf_len: usize,
    ) -> *mut SlacklessCamCapture;
    pub(crate) fn slackless_cam_capture_set_blur(cap: *mut SlacklessCamCapture, enabled: bool);
    pub(crate) fn slackless_cam_capture_force_keyframe(cap: *mut SlacklessCamCapture);
    pub(crate) fn slackless_cam_capture_set_bitrate(
        cap: *mut SlacklessCamCapture,
        bitrate_bps: u32,
    );
    pub(crate) fn slackless_cam_capture_set_compact(cap: *mut SlacklessCamCapture, compact: bool);
    pub(crate) fn slackless_cam_capture_stop(cap: *mut SlacklessCamCapture);

    pub(crate) fn slackless_vt_decoder_new() -> *mut SlacklessVtDecoder;
    pub(crate) fn slackless_vt_decoder_decode(
        dec: *mut SlacklessVtDecoder,
        annex_b: *const u8,
        len: usize,
        ctx: *mut c_void,
        on_surface: SlacklessSurfaceCallback,
    ) -> bool;
    pub(crate) fn slackless_vt_decoder_free(dec: *mut SlacklessVtDecoder);

    pub(crate) fn slackless_vt_av1_decoder_new() -> *mut SlacklessVtAv1Decoder;
    pub(crate) fn slackless_vt_av1_decoder_decode(
        dec: *mut SlacklessVtAv1Decoder,
        tu: *const u8,
        len: usize,
        ctx: *mut c_void,
        on_surface: SlacklessSurfaceCallback,
    ) -> bool;
    pub(crate) fn slackless_vt_av1_decoder_free(dec: *mut SlacklessVtAv1Decoder);

    pub(crate) fn slackless_audio_device_monitor_start(
        ctx: *mut c_void,
        cb: SlacklessAudioDeviceChangeCallback,
    ) -> *mut SlacklessAudioDeviceMonitor;
    pub(crate) fn slackless_audio_device_monitor_stop(mon: *mut SlacklessAudioDeviceMonitor);

    pub(crate) fn slackless_capture_thumbnails(
        monitors_only: bool,
        max_w: u32,
        max_h: u32,
        ctx: *mut c_void,
        cb: SlacklessThumbCallback,
        should_skip_window: Option<SlacklessWindowSkipCallback>,
    ) -> bool;

    pub(crate) fn slackless_vpio_start(
        sample_rate: u32,
        enable_vp: bool,
        enable_agc: bool,
        ctx: *mut c_void,
        on_capture: SlacklessVpioCaptureCallback,
        on_render: SlacklessVpioRenderCallback,
    ) -> *mut SlacklessVpio;
    pub(crate) fn slackless_vpio_set_processing(
        vpio: *mut SlacklessVpio,
        enable_vp: bool,
        enable_agc: bool,
    );
    pub(crate) fn slackless_vpio_stop(vpio: *mut SlacklessVpio);

    pub(crate) fn slackless_audio_file_play_oneshot(
        path: *const c_char,
        device_name: *const c_char,
        volume: f32,
    ) -> bool;
    pub(crate) fn slackless_audio_file_start_loop(
        path: *const c_char,
        device_name: *const c_char,
        volume: f32,
    ) -> *mut SlacklessAudioHandle;
    pub(crate) fn slackless_audio_file_stop_loop(handle: *mut SlacklessAudioHandle);
    pub(crate) fn slackless_audio_file_measure_rms(path: *const c_char) -> f32;

    pub(crate) fn slackless_audio_tap_start(
        ctx: *mut c_void,
        on_pcm: SlacklessAudioTapCallback,
        err_buf: *mut c_char,
        err_buf_len: usize,
    ) -> *mut SlacklessAudioTap;
    pub(crate) fn slackless_audio_tap_stop(tap: *mut SlacklessAudioTap);

    pub(crate) fn slackless_video_player_open(path: *const c_char) -> *mut SlacklessVideoPlayer;
    pub(crate) fn slackless_video_player_open_url(
        url: *const c_char,
        cookie_header: *const c_char,
    ) -> *mut SlacklessVideoPlayer;
    pub(crate) fn slackless_video_player_play(player: *mut SlacklessVideoPlayer);
    pub(crate) fn slackless_video_player_pause(player: *mut SlacklessVideoPlayer);
    pub(crate) fn slackless_video_player_set_muted(player: *mut SlacklessVideoPlayer, muted: bool);
    pub(crate) fn slackless_video_player_set_volume(player: *mut SlacklessVideoPlayer, volume: f32);
    pub(crate) fn slackless_video_player_set_rate(player: *mut SlacklessVideoPlayer, rate: f32);
    pub(crate) fn slackless_video_player_seek(player: *mut SlacklessVideoPlayer, secs: f64);
    pub(crate) fn slackless_video_player_state(
        player: *mut SlacklessVideoPlayer,
        is_playing: *mut bool,
        is_muted: *mut bool,
        has_ended: *mut bool,
        pos_secs: *mut f64,
        dur_secs: *mut f64,
        out_w: *mut u32,
        out_h: *mut u32,
        is_failed: *mut bool,
        out_rate: *mut f32,
        out_volume: *mut f32,
    );
    pub(crate) fn slackless_video_player_copy_frame(
        player: *mut SlacklessVideoPlayer,
        out_w: *mut u32,
        out_h: *mut u32,
    ) -> *mut c_void;
    pub(crate) fn slackless_video_player_destroy(player: *mut SlacklessVideoPlayer);

    pub(crate) fn slackless_extract_audio_16k_mono(
        path: *const c_char,
        out_samples: *mut *mut f32,
        out_len: *mut usize,
    ) -> bool;
    pub(crate) fn slackless_free_f32_buf(buf: *mut f32);

    pub(crate) fn slackless_pdf_page_count(path: *const c_char) -> u32;
    pub(crate) fn slackless_pdf_render_page_png(
        path: *const c_char,
        page_index: u32,
        max_w: u32,
        max_h: u32,
        out_buf: *mut *mut u8,
        out_len: *mut usize,
        out_w: *mut u32,
        out_h: *mut u32,
    ) -> bool;
}

pub(crate) struct CaptureCallbackCtx {
    pub(crate) outbound_h264_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    pub(crate) on_preview: Box<dyn Fn(VideoFrame) + Send + Sync>,
    pub(crate) perf: Option<Arc<HuddlePerfCounters>>,
}

pub(crate) unsafe extern "C" fn on_h264_cb(
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
    // SAFETY: `ctx` is the `CaptureCallbackCtx` the camera or screen capture was started with,
    // freed only once the capture's stop guarantees no further callback (see their `Drop`).
    let state = unsafe { &*(ctx as *const CaptureCallbackCtx) };
    // SAFETY: the encoder passes `len` initialized bytes that stay valid for this call.
    let slice = unsafe { std::slice::from_raw_parts(data, len) };
    if let Some(perf) = &state.perf {
        perf.record_tx_frame(width, height, len as u64, encode_us);
    }
    if let Some(tx) = &state.outbound_h264_tx {
        let _ = tx.send(slice.to_vec());
    }
}

pub(crate) unsafe extern "C" fn on_surface_preview_cb(
    ctx: *mut c_void,
    cv_pb: *mut c_void,
    width: u32,
    height: u32,
) {
    if ctx.is_null() || cv_pb.is_null() {
        if !cv_pb.is_null() {
            // SAFETY: the capture hands over a +1 retain on `cv_pb` that nothing else releases.
            unsafe { slackless_cvpb_release(cv_pb) };
        }
        return;
    }
    // SAFETY: as in `on_h264_cb`, `ctx` outlives every callback of the capture it was given to.
    let state = unsafe { &*(ctx as *const CaptureCallbackCtx) };
    // SAFETY: the capture hands over a +1 retain on `cv_pb`, which the surface takes over.
    if let Some(surface) = unsafe { MacVideoSurface::from_retained(cv_pb, width, height) } {
        (state.on_preview)(VideoFrame::from_mac_surface(surface));
    }
}

pub(crate) unsafe extern "C" fn on_error_cb(_ctx: *mut c_void, message: *const c_char) {
    let msg = if message.is_null() {
        "macOS capture stream stopped with unknown error"
    } else {
        // SAFETY: a non-null `message` is a NUL-terminated string valid for this call.
        unsafe { CStr::from_ptr(message) }
            .to_str()
            .unwrap_or("macOS capture stream stopped")
    };
    tracing::warn!(error = msg, "macOS hardware media capture error");
}
