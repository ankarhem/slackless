//! CoreAudio hot-plug monitor, native `VoiceProcessingIO`, routed `AVAudioEngine` playback, and
//! macOS 14.2+ CoreAudio Process Tap system audio capture.

use std::ffi::{CStr, c_char, c_void};
use std::ptr::NonNull;
use std::sync::Arc;

use super::ffi::{
    SlacklessAudioDeviceMonitor, SlacklessAudioHandle, SlacklessAudioTap, SlacklessVpio,
    slackless_audio_device_monitor_start, slackless_audio_device_monitor_stop,
    slackless_audio_file_measure_rms, slackless_audio_file_play_oneshot,
    slackless_audio_file_start_loop, slackless_audio_file_stop_loop,
    slackless_audio_tap_is_supported, slackless_audio_tap_start, slackless_audio_tap_stop,
    slackless_vpio_set_processing, slackless_vpio_start, slackless_vpio_stop,
};
use crate::audio::ring::SpscRing;
use crate::{HuddleError, Result};

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
    handle: NonNull<SlacklessAudioDeviceMonitor>,
    ctx: NonNull<AudioHotplugCtx>,
}

// SAFETY: Stopped and joined synchronously on Drop before freeing `ctx`.
unsafe impl Send for CoreAudioDeviceMonitor {}

impl CoreAudioDeviceMonitor {
    pub fn start(on_change: impl Fn(bool) + Send + Sync + 'static) -> Option<Self> {
        let ctx = NonNull::from(Box::leak(Box::new(AudioHotplugCtx {
            callback: Box::new(on_change),
        })));
        let handle = unsafe {
            slackless_audio_device_monitor_start(ctx.as_ptr().cast(), on_audio_device_change_cb)
        };
        let Some(handle) = NonNull::new(handle) else {
            unsafe { drop(Box::from_raw(ctx.as_ptr())) };
            return None;
        };
        Some(Self { handle, ctx })
    }
}

impl Drop for CoreAudioDeviceMonitor {
    fn drop(&mut self) {
        unsafe { slackless_audio_device_monitor_stop(self.handle.as_ptr()) };
        unsafe { drop(Box::from_raw(self.ctx.as_ptr())) };
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
    handle: NonNull<SlacklessVpio>,
    ctx: NonNull<VpioCtx>,
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
        let ctx = NonNull::from(Box::leak(Box::new(VpioCtx {
            on_capture: Box::new(on_capture),
            on_render: Box::new(on_render),
        })));
        let handle = unsafe {
            slackless_vpio_start(
                sample_rate,
                enable_vp,
                enable_agc,
                ctx.as_ptr().cast(),
                on_vpio_capture_cb,
                on_vpio_render_cb,
            )
        };
        let Some(handle) = NonNull::new(handle) else {
            unsafe { drop(Box::from_raw(ctx.as_ptr())) };
            return Err(HuddleError::Audio(
                "CoreAudio VoiceProcessingIO unavailable".into(),
            ));
        };
        tracing::info!(
            sample_rate,
            enable_vp,
            enable_agc,
            "started macOS CoreAudio VoiceProcessingIO (hardware AEC/NS/AGC)"
        );
        Ok(Self { handle, ctx })
    }

    pub fn set_processing(&self, enable_vp: bool, enable_agc: bool) {
        unsafe { slackless_vpio_set_processing(self.handle.as_ptr(), enable_vp, enable_agc) };
    }
}

impl Drop for MacVoiceProcessingIo {
    fn drop(&mut self) {
        unsafe { slackless_vpio_stop(self.handle.as_ptr()) };
        unsafe { drop(Box::from_raw(self.ctx.as_ptr())) };
    }
}

// ---------------------------------------------------------------------------
// Routed Native Audio File Playback & RMS Loudness Measurement
// ---------------------------------------------------------------------------

/// Measure gated linear RMS loudness of an audio file via CoreAudio `ExtAudioFile`.
pub fn measure_audio_file_rms(path: &std::path::Path) -> Option<f32> {
    let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes()).ok()?;
    let rms = unsafe { slackless_audio_file_measure_rms(c_path.as_ptr()) };
    (rms.is_finite() && rms > 1e-5).then_some(rms)
}

/// Play a one-shot audio file (`.mp3` / `.wav`) via native `AVAudioEngine` (or retained `NSSound`
/// fallback) routed to `device_name` (or system default when `None`).
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

/// Active looping `AVAudioEngine` playback handle routed to a specific CoreAudio output device.
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

// ---------------------------------------------------------------------------
// macOS 14.2+ CoreAudio Process Tap System Audio Capture (Lock-Free SPSC Ring)
// ---------------------------------------------------------------------------

/// Returns `true` when macOS 14.2+ CoreAudio Process Taps (`CATapDescription`) are available.
pub fn supports_native_system_audio_tap() -> bool {
    unsafe { slackless_audio_tap_is_supported() }
}

struct TapRingCtx {
    ring: Arc<SpscRing<i16>>,
}

unsafe extern "C" fn on_audio_tap_pcm_cb(ctx: *mut c_void, samples: *const i16, count: usize) {
    if ctx.is_null() || samples.is_null() || count == 0 {
        return;
    }
    // Real-time CoreAudio IOProc: lock-free, allocation-free push into SPSC ring.
    let state = unsafe { &*(ctx as *const TapRingCtx) };
    let slice = unsafe { std::slice::from_raw_parts(samples, count) };
    let _ = state.ring.push_slice(slice);
}

/// Live macOS 14.2+ CoreAudio Process Tap streaming stereo `i16` PCM into a lock-free SPSC ring.
pub struct MacSystemAudioTap {
    handle: NonNull<SlacklessAudioTap>,
    ctx: NonNull<TapRingCtx>,
}

// SAFETY: `slackless_audio_tap_stop` stops and destroys the IOProc synchronously before freeing `ctx`.
unsafe impl Send for MacSystemAudioTap {}

impl MacSystemAudioTap {
    pub(crate) fn start(ring: Arc<SpscRing<i16>>) -> Result<Self> {
        let ctx = NonNull::from(Box::leak(Box::new(TapRingCtx { ring })));
        let mut err_buf = [0 as c_char; 256];
        let handle = unsafe {
            slackless_audio_tap_start(
                ctx.as_ptr().cast(),
                on_audio_tap_pcm_cb,
                err_buf.as_mut_ptr(),
                err_buf.len(),
            )
        };
        let Some(handle) = NonNull::new(handle) else {
            unsafe { drop(Box::from_raw(ctx.as_ptr())) };
            let msg = unsafe { CStr::from_ptr(err_buf.as_ptr()) }
                .to_str()
                .unwrap_or("macOS CoreAudio Process Tap failed")
                .to_owned();
            return Err(HuddleError::Audio(msg));
        };
        tracing::info!("macOS CoreAudio Process Tap started (ARC native)");
        Ok(Self { handle, ctx })
    }
}

impl Drop for MacSystemAudioTap {
    fn drop(&mut self) {
        unsafe { slackless_audio_tap_stop(self.handle.as_ptr()) };
        unsafe { drop(Box::from_raw(self.ctx.as_ptr())) };
    }
}
