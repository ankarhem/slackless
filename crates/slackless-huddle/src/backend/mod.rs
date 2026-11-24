//! Pluggable cross-platform hardware/software media backend abstraction for Slackless Huddles.
//!
//! Each OS implements [`MediaBackend`] so macOS, Linux, and Windows can plug in native hardware
//! capture, encode/decode, zero-copy GPU surfaces, and event-driven device hot-plug without
//! changing the WebRTC signaling, Chime session, or GUI rendering code:
//!
//! | OS | Screen Share | Webcam | H.264 / AV1 Decode | Audio Hot-Plug |
//! |---|---|---|---|---|
//! | **macOS** (`macos.rs`) | `ScreenCaptureKit` + `VideoToolbox` (`420f`) | `AVCaptureSession` + `VideoToolbox` + `Vision`/`Metal` blur | `VideoToolbox` H.264 (all Macs) + runtime-gated M3+ HW AV1 (`rav1d` fallback on M1/M2) | CoreAudio `AudioObjectAddPropertyListenerBlock` |
//! | **Portable fallback** (`portable.rs`) | `xcap` + `OpenH264` `ScreenContentRealTime` | `nokhwa` + `OpenH264` | Lazy `OpenH264` + `rav1d` | Periodic fallback |
//! | **Linux** (future `linux.rs`) | PipeWire / XDG Portal + VA-API / NVENC | PipeWire / V4L2 + VA-API | VA-API / NVDEC (`DMA-BUF`) | PipeWire / PulseAudio subscribe |
//! | **Windows** (future `windows.rs`) | Windows Graphics Capture + Media Foundation | Media Foundation (`D3D11`) | DXVA2 / D3D11VA (`ID3D11Texture2D`) | WASAPI `IMMNotificationClient` |

#[cfg(target_os = "macos")]
mod macos;
pub(crate) mod portable;

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::Result;
use crate::metrics::HuddlePerfCounters;
use crate::video::{CameraDeviceInfo, VideoFrame};

/// Runtime-probed hardware media capabilities of the current host.
///
/// Always probed dynamically at runtime (never assumes Apple Silicon M3+ or specific GPU models).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MediaCapabilities {
    pub hw_h264_encode: bool,
    pub hw_h264_decode: bool,
    pub hw_av1_decode: bool,
    pub hw_camera_capture: bool,
    pub zero_copy_gpu_surface: bool,
    pub native_system_audio_tap: bool,
    pub native_device_hotplug: bool,
    pub hw_audio_dsp: bool,
}

impl MediaCapabilities {
    /// Returns `true` when H.264 should be advertised ahead of AV1 in WebRTC SDP negotiation.
    ///
    /// On M1/M2 Macs (or Linux/Windows GPUs with HW H.264 decode but no HW AV1 decode), offering
    /// H.264 first lets Chime/peers negotiate HW-decoded H.264 while still allowing `rav1d`
    /// fallback whenever a peer forces AV1. On M3/M4 Macs (`hw_av1_decode == true`), AV1 stays
    /// first because both AV1 and H.264 are decoded in hardware.
    pub fn prefer_h264_over_av1(&self) -> bool {
        self.hw_h264_decode && !self.hw_av1_decode
    }
}

/// Which default audio device changed on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioDeviceChangeKind {
    Input,
    Output,
}

/// Configuration for starting a screen or window share capture session.
#[derive(Debug, Clone)]
pub struct ScreenShareConfig {
    pub source_id: String,
    pub max_w: u32,
    pub max_h: u32,
    pub max_fps: u32,
    pub bitrate_bps: u32,
}

/// Configuration for starting a local webcam capture & encode session.
#[derive(Debug, Clone)]
pub struct CameraConfig {
    pub device_index: Option<usize>,
    pub max_w: u32,
    pub max_h: u32,
    pub max_fps: u32,
    pub bitrate_bps: u32,
    pub blur_background: bool,
}

/// Active screen/window share capture & encode session.
pub trait ScreenShareHandle: Send {
    fn force_keyframe(&self);
    fn set_target_bitrate(&self, bitrate_bps: u32);
    fn stop(&mut self);
}

/// Active local webcam capture & encode session.
pub trait CameraHandle: Send {
    fn set_blur(&self, enabled: bool);
    fn force_keyframe(&self);
    fn set_target_bitrate(&self, bitrate_bps: u32);
    /// Switch between compact resolution (`640x480` while screen-sharing) and standard (`1280x720`).
    fn set_compact(&self, _compact: bool) {}
    fn stop(&mut self);
}

/// Active single-stream video decoder (H.264 Annex-B access unit or AV1 temporal unit).
pub trait VideoDecoderHandle: Send {
    fn backend_name(&self) -> &'static str;
    fn decode_unit(&mut self, unit: &[u8]) -> Option<VideoFrame>;
}

/// Active OS audio device hot-plug monitor; dropping stops listening.
pub trait AudioDeviceMonitorHandle: Send {}

/// Active OS-native full-duplex 48 kHz mono audio stream with built-in hardware AEC/NS/AGC
/// (e.g. macOS CoreAudio `VoiceProcessingIO`; future Windows WASAPI Voice Capture / Linux PipeWire).
pub trait HwAudioDuplexHandle: Send {
    fn set_processing(&self, enable_aec_ns: bool, enable_agc: bool);
}

/// Callback invoked by an OS-native hardware audio engine when captured 48 kHz mono samples arrive.
pub type AudioCaptureCallback = Arc<dyn Fn(&[f32]) + Send + Sync>;

/// Callback invoked by an OS-native hardware audio engine to pull 48 kHz mono playback samples.
pub type AudioRenderCallback = Arc<dyn Fn(&mut [f32]) + Send + Sync>;

/// Pluggable OS media backend factory.
pub trait MediaBackend: Send + Sync {
    /// Human-readable backend name for diagnostics and logging.
    fn name(&self) -> &'static str;

    /// Dynamically probed hardware capabilities of this machine.
    fn capabilities(&self) -> MediaCapabilities;

    /// Enumerate local camera devices in stable index order matching [`Self::start_camera`].
    fn list_cameras(&self) -> Vec<CameraDeviceInfo>;

    /// Capture downscaled screen or window preview thumbnails (GPU-accelerated when available).
    fn capture_screen_thumbnails(
        &self,
        monitors_only: bool,
        max_w: u32,
        max_h: u32,
    ) -> Vec<(String, VideoFrame)>;

    /// Start hardware-accelerated (or fallback) screen/window capture + H.264 encode.
    fn start_screen_share(
        &self,
        config: ScreenShareConfig,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
        perf: Arc<HuddlePerfCounters>,
    ) -> Result<Box<dyn ScreenShareHandle>>;

    /// Start hardware-accelerated (or fallback) webcam capture + H.264 encode.
    fn start_camera(
        &self,
        config: CameraConfig,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
    ) -> Result<Box<dyn CameraHandle>>;

    /// Create an H.264 Annex-B decoder (HW with lazy SW fallback).
    fn create_h264_decoder(&self) -> Result<Box<dyn VideoDecoderHandle>>;

    /// Create an AV1 temporal-unit decoder (HW on M3+/capable GPUs with seamless `rav1d` fallback).
    fn create_av1_decoder(&self) -> Result<Box<dyn VideoDecoderHandle>>;

    /// Start an OS-native event-driven default audio device change monitor if supported.
    /// Returns `None` on platforms that should fall back to periodic device checks.
    fn start_audio_device_monitor(
        &self,
        on_change: Arc<dyn Fn(AudioDeviceChangeKind) + Send + Sync>,
    ) -> Option<Box<dyn AudioDeviceMonitorHandle>>;

    /// Start an OS-native full-duplex 48 kHz mono audio stream with built-in hardware AEC/NS/AGC.
    /// Returns `None` when the caller should use the portable `cpal + sonora` pipeline instead.
    #[allow(clippy::too_many_arguments)]
    fn start_hw_audio_duplex(
        &self,
        _sample_rate: u32,
        _input_device: Option<&str>,
        _output_device: Option<&str>,
        _enable_aec_ns: bool,
        _enable_agc: bool,
        _on_capture: AudioCaptureCallback,
        _on_render: AudioRenderCallback,
    ) -> Option<Result<Box<dyn HwAudioDuplexHandle>>> {
        None
    }
}

/// Return the compiled platform's active [`MediaBackend`].
pub fn platform_backend() -> &'static dyn MediaBackend {
    #[cfg(target_os = "macos")]
    {
        &macos::MACOS_BACKEND
    }
    #[cfg(not(target_os = "macos"))]
    {
        &portable::PORTABLE_BACKEND
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m1_m2_prefers_h264_while_m3_plus_keeps_av1_first() {
        let m1 = MediaCapabilities {
            hw_h264_decode: true,
            hw_av1_decode: false,
            ..Default::default()
        };
        assert!(m1.prefer_h264_over_av1());

        let m3 = MediaCapabilities {
            hw_h264_decode: true,
            hw_av1_decode: true,
            ..Default::default()
        };
        assert!(!m3.prefer_h264_over_av1());

        let sw_only = MediaCapabilities::default();
        assert!(!sw_only.prefer_h264_over_av1());
    }

    #[test]
    fn platform_backend_exposes_consistent_capabilities() {
        let backend = platform_backend();
        assert!(!backend.name().is_empty());
        let caps = backend.capabilities();
        #[cfg(target_os = "macos")]
        {
            assert!(caps.hw_h264_encode);
            assert!(caps.zero_copy_gpu_surface);
            assert!(caps.native_device_hotplug);
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert!(!caps.zero_copy_gpu_surface);
        }
    }
}
