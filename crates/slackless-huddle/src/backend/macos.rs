//! macOS hardware-accelerated [`MediaBackend`] implementation (all M1/M2/M3/M4 & Intel Macs).
//!
//! Runtime-probes hardware AV1 decode (`VTIsHardwareDecodeSupported`) so M3/M4 Macs use HW AV1
//! decode while M1/M2 Macs gracefully prefer HW H.264 decode and fall back to `rav1d` for AV1.

#![cfg(target_os = "macos")]

use std::path::Path;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::portable::{LazyOpenH264Decoder, SoftwareAv1Decoder, SoftwareScreenShare};
use super::{
    AudioDeviceChangeKind, AudioDeviceMonitorHandle, AudioLoopHandle, CameraConfig, CameraHandle,
    CameraPreviewConfig, CameraPreviewHandle, HwAudioDuplexConfig, HwAudioDuplexHandle,
    MediaBackend, MediaCapabilities, ScreenShareConfig, ScreenShareHandle, SystemAudioTapHandle,
    VideoDecoderHandle,
};
use crate::audio::ring::SpscRing;
use crate::macos::{
    AvVtCameraCapture, CoreAudioDeviceMonitor, MacRoutedAudioLoop, MacSystemAudioTap,
    MacVoiceProcessingIo, ScVtScreenCapture, VtAv1Decoder, VtH264Decoder, capture_macos_thumbnails,
    has_screen_capture_permission, list_macos_cameras, measure_audio_file_rms,
    open_screen_capture_settings, play_routed_audio_oneshot, request_screen_capture_permission,
    supports_hw_av1_decode, supports_hw_h264_decode, supports_native_system_audio_tap,
};
use crate::metrics::HuddlePerfCounters;
use crate::video::{CameraDeviceInfo, VideoFrame, select_camera_index};
use crate::{HuddleError, Result};

pub struct MacOsMediaBackend;

pub static MACOS_BACKEND: MacOsMediaBackend = MacOsMediaBackend;

impl ScreenShareHandle for ScVtScreenCapture {
    fn force_keyframe(&self) {
        ScVtScreenCapture::force_keyframe(self);
    }

    fn set_target_bitrate(&self, bitrate_bps: u32) {
        ScVtScreenCapture::set_target_bitrate(self, bitrate_bps);
    }

    fn stop(&mut self) {
        ScVtScreenCapture::stop(self);
    }
}

impl CameraHandle for AvVtCameraCapture {
    fn set_blur(&self, enabled: bool) {
        AvVtCameraCapture::set_blur(self, enabled);
    }

    fn force_keyframe(&self) {
        AvVtCameraCapture::force_keyframe(self);
    }

    fn set_target_bitrate(&self, bitrate_bps: u32) {
        AvVtCameraCapture::set_target_bitrate(self, bitrate_bps);
    }

    fn set_compact(&self, compact: bool) {
        AvVtCameraCapture::set_compact(self, compact);
    }

    fn stop(&mut self) {
        AvVtCameraCapture::stop(self);
    }
}

impl CameraPreviewHandle for AvVtCameraCapture {
    fn set_blur(&self, enabled: bool) {
        AvVtCameraCapture::set_blur(self, enabled);
    }

    fn stop(&mut self) {
        AvVtCameraCapture::stop(self);
    }
}

struct MacAudioDeviceMonitor {
    _inner: CoreAudioDeviceMonitor,
}

impl AudioDeviceMonitorHandle for MacAudioDeviceMonitor {}

impl SystemAudioTapHandle for MacSystemAudioTap {}

impl AudioLoopHandle for MacRoutedAudioLoop {
    fn stop(&mut self) {
        MacRoutedAudioLoop::stop(self);
    }
}

impl HwAudioDuplexHandle for MacVoiceProcessingIo {
    fn set_processing(&self, enable_aec_ns: bool, enable_agc: bool) {
        MacVoiceProcessingIo::set_processing(self, enable_aec_ns, enable_agc);
    }
}

/// Hardware VideoToolbox H.264 decoder with **lazy** OpenH264 fallback (allocates 0 MB of SW
/// decoder memory unless HW fails).
struct MacH264Decoder {
    vt: Option<VtH264Decoder>,
    sw: LazyOpenH264Decoder,
}

impl MacH264Decoder {
    fn new() -> Self {
        Self {
            vt: VtH264Decoder::new().ok(),
            sw: LazyOpenH264Decoder::new(),
        }
    }
}

impl VideoDecoderHandle for MacH264Decoder {
    fn backend_name(&self) -> &'static str {
        if self.vt.is_some() {
            "H.264 (VideoToolbox HW)"
        } else {
            "H.264 (OpenH264 SW)"
        }
    }

    fn decode_unit(&mut self, unit: &[u8]) -> Option<VideoFrame> {
        if let Some(vt) = self.vt.as_mut()
            && let Some(frame) = vt.decode_annex_b(unit)
        {
            return Some(frame);
        }
        self.sw.decode_annex_b(unit)
    }
}

const HW_AV1_FAILURE_FALLBACK_THRESHOLD: u32 = 8;

/// Generic HW->SW fallback state machine for AV1 decoding: falls back permanently to `Sw`
/// after [`HW_AV1_FAILURE_FALLBACK_THRESHOLD`] consecutive HW decode failures, resetting the
/// failure counter on any successful HW frame.
struct Av1FallbackState<Hw, Sw> {
    hw: Option<Hw>,
    sw: Option<Sw>,
    hw_consecutive_failures: u32,
}

impl<Hw, Sw> Av1FallbackState<Hw, Sw> {
    fn new(hw: Option<Hw>, sw: Option<Sw>) -> Self {
        Self {
            hw,
            sw,
            hw_consecutive_failures: 0,
        }
    }

    fn is_hw_active(&self) -> bool {
        self.hw.is_some()
    }

    fn decode_with<F, G, H>(
        &mut self,
        unit: &[u8],
        mut decode_hw: F,
        mut make_sw: G,
        mut decode_sw: H,
    ) -> Option<VideoFrame>
    where
        F: FnMut(&mut Hw, &[u8]) -> Option<VideoFrame>,
        G: FnMut() -> Option<Sw>,
        H: FnMut(&mut Sw, &[u8]) -> Option<VideoFrame>,
    {
        if let Some(hw) = self.hw.as_mut() {
            if let Some(frame) = decode_hw(hw, unit) {
                self.hw_consecutive_failures = 0;
                return Some(frame);
            }
            // Wait for initial sequence header / keyframe; only fall back to `rav1d` if HW
            // fails repeatedly on decodable units (e.g. unsupported profile/12-bit).
            self.hw_consecutive_failures = self.hw_consecutive_failures.saturating_add(1);
            if self.hw_consecutive_failures < HW_AV1_FAILURE_FALLBACK_THRESHOLD {
                return None;
            }
            tracing::info!("VideoToolbox HW AV1 decode unavailable for stream; switching to rav1d");
            self.hw = None;
        }
        if self.sw.is_none() {
            self.sw = make_sw();
        }
        decode_sw(self.sw.as_mut()?, unit)
    }
}

/// Runtime-gated VideoToolbox HW AV1 decoder (M3/M4+) with seamless lazy `rav1d` fallback
/// (always uses `rav1d` on M1/M2 without assuming M3+).
struct MacAv1Decoder {
    state: Av1FallbackState<VtAv1Decoder, SoftwareAv1Decoder>,
}

impl MacAv1Decoder {
    fn new() -> Result<Self> {
        let vt = VtAv1Decoder::new().ok();
        // On M1/M2 where `vt` is None, initialize `rav1d` immediately; on M3+ keep it lazy.
        let sw = if vt.is_none() {
            Some(SoftwareAv1Decoder::new()?)
        } else {
            None
        };
        Ok(Self {
            state: Av1FallbackState::new(vt, sw),
        })
    }
}

impl VideoDecoderHandle for MacAv1Decoder {
    fn backend_name(&self) -> &'static str {
        if self.state.is_hw_active() {
            "AV1 (VideoToolbox HW)"
        } else {
            "AV1 (rav1d SW)"
        }
    }

    fn decode_unit(&mut self, unit: &[u8]) -> Option<VideoFrame> {
        self.state.decode_with(
            unit,
            |vt, u| vt.decode_temporal_unit(u),
            || SoftwareAv1Decoder::new().ok(),
            |sw, u| sw.decode_unit(u),
        )
    }
}

/// Resolve a [`CameraConfig`]'s requested device index against the current native `AVFoundation`
/// camera list, falling back to the primary device when `None` or unplugged.
fn resolve_camera_start_config(
    mut config: CameraConfig,
    available: &[CameraDeviceInfo],
) -> Result<CameraConfig> {
    let picked = select_camera_index(config.device_index, available)
        .ok_or_else(|| HuddleError::Video("no camera device found".into()))?;
    config.device_index = Some(picked);
    Ok(config)
}

fn start_camera_with<L, S>(
    config: CameraConfig,
    outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
    preview_tx: mpsc::UnboundedSender<VideoFrame>,
    list_devices: L,
    start_native: S,
) -> Result<Box<dyn CameraHandle>>
where
    L: FnOnce() -> Vec<CameraDeviceInfo>,
    S: FnOnce(
        CameraConfig,
        mpsc::UnboundedSender<Vec<u8>>,
        mpsc::UnboundedSender<VideoFrame>,
    ) -> Result<Box<dyn CameraHandle>>,
{
    let available = list_devices();
    let resolved = resolve_camera_start_config(config, &available)?;
    start_native(resolved, outbound_h264_tx, preview_tx)
}

impl MediaBackend for MacOsMediaBackend {
    fn name(&self) -> &'static str {
        "macOS Native (ScreenCaptureKit / AVFoundation / VideoToolbox)"
    }

    fn capabilities(&self) -> MediaCapabilities {
        MediaCapabilities {
            hw_h264_encode: true,
            hw_h264_decode: supports_hw_h264_decode(),
            hw_av1_decode: supports_hw_av1_decode(),
            hw_camera_capture: true,
            zero_copy_gpu_surface: true,
            native_system_audio_tap: supports_native_system_audio_tap(),
            native_device_hotplug: true,
            hw_audio_dsp: true,
        }
    }

    fn has_screen_capture_permission(&self) -> bool {
        has_screen_capture_permission()
    }

    fn request_screen_capture_permission(&self) -> bool {
        request_screen_capture_permission()
    }

    fn open_screen_capture_settings(&self) {
        open_screen_capture_settings();
    }

    fn list_cameras(&self) -> Vec<CameraDeviceInfo> {
        list_macos_cameras()
    }

    fn capture_screen_thumbnails(
        &self,
        monitors_only: bool,
        max_w: u32,
        max_h: u32,
    ) -> Vec<(String, VideoFrame)> {
        if !self.has_screen_capture_permission() {
            return Vec::new();
        }
        if let Some(thumbs) = capture_macos_thumbnails(monitors_only, max_w, max_h) {
            return thumbs;
        }
        crate::screen::capture_screen_thumbnails_xcap(monitors_only, max_w, max_h)
    }

    fn start_screen_share(
        &self,
        config: ScreenShareConfig,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
        perf: Arc<HuddlePerfCounters>,
    ) -> Result<Box<dyn ScreenShareHandle>> {
        if !self.has_screen_capture_permission() {
            return Err(HuddleError::Video(
                "Screen Recording permission not granted. Grant access in System Settings > Privacy & Security > Screen Recording."
                    .into(),
            ));
        }
        match ScVtScreenCapture::start(
            &config,
            outbound_h264_tx.clone(),
            preview_tx.clone(),
            Arc::clone(&perf),
        ) {
            Ok(hw) => Ok(Box::new(hw)),
            Err(err) => {
                if !self.has_screen_capture_permission() {
                    return Err(err);
                }
                tracing::warn!(
                    source_id = %config.source_id,
                    %err,
                    "ScreenCaptureKit + VideoToolbox unavailable; falling back to tuned OpenH264"
                );
                Ok(Box::new(SoftwareScreenShare::start(
                    config,
                    outbound_h264_tx,
                    preview_tx,
                    perf,
                )?))
            }
        }
    }

    fn start_camera(
        &self,
        config: CameraConfig,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
    ) -> Result<Box<dyn CameraHandle>> {
        start_camera_with(
            config,
            outbound_h264_tx,
            preview_tx,
            list_macos_cameras,
            |cfg, h264_tx, prev_tx| {
                AvVtCameraCapture::start(&cfg, h264_tx, prev_tx)
                    .map(|hw| Box::new(hw) as Box<dyn CameraHandle>)
            },
        )
    }

    fn start_camera_preview(
        &self,
        config: CameraPreviewConfig,
        on_frame: Arc<dyn Fn(VideoFrame) + Send + Sync>,
    ) -> Result<Box<dyn CameraPreviewHandle>> {
        let available = list_macos_cameras();
        let resolved = select_camera_index(config.device_index, &available)
            .ok_or_else(|| HuddleError::Video("no camera device found".into()))?;
        let cap = AvVtCameraCapture::start_preview_only(
            Some(resolved),
            config.max_w,
            config.max_h,
            config.max_fps,
            config.blur_background,
            move |frame| on_frame(frame),
        )?;
        Ok(Box::new(cap))
    }

    fn create_h264_decoder(&self) -> Result<Box<dyn VideoDecoderHandle>> {
        Ok(Box::new(MacH264Decoder::new()))
    }

    fn create_av1_decoder(&self) -> Result<Box<dyn VideoDecoderHandle>> {
        Ok(Box::new(MacAv1Decoder::new()?))
    }

    fn start_audio_device_monitor(
        &self,
        on_change: Arc<dyn Fn(AudioDeviceChangeKind) + Send + Sync>,
    ) -> Option<Box<dyn AudioDeviceMonitorHandle>> {
        let inner = CoreAudioDeviceMonitor::start(move |is_input| {
            on_change(if is_input {
                AudioDeviceChangeKind::Input
            } else {
                AudioDeviceChangeKind::Output
            });
        })?;
        Some(Box::new(MacAudioDeviceMonitor { _inner: inner }))
    }

    fn native_system_audio_label(&self) -> Option<&'static str> {
        supports_native_system_audio_tap().then_some("macOS Native System Audio")
    }

    fn start_system_audio_tap(
        &self,
        ring: Arc<SpscRing<i16>>,
    ) -> Option<Result<Box<dyn SystemAudioTapHandle>>> {
        if !supports_native_system_audio_tap() {
            return None;
        }
        Some(MacSystemAudioTap::start(ring).map(|t| Box::new(t) as Box<dyn SystemAudioTapHandle>))
    }

    fn measure_audio_file_rms(&self, path: &Path) -> Option<f32> {
        measure_audio_file_rms(path)
    }

    fn play_routed_audio_oneshot(
        &self,
        path: &Path,
        device_name: Option<&str>,
        volume: f32,
    ) -> bool {
        play_routed_audio_oneshot(path, device_name, volume)
    }

    fn start_routed_audio_loop(
        &self,
        path: &Path,
        device_name: Option<&str>,
        volume: f32,
    ) -> Option<Box<dyn AudioLoopHandle>> {
        MacRoutedAudioLoop::start(path, device_name, volume)
            .map(|l| Box::new(l) as Box<dyn AudioLoopHandle>)
    }

    fn supports_routed_audio_files(&self) -> bool {
        true
    }

    fn start_hw_audio_duplex(
        &self,
        config: HwAudioDuplexConfig<'_>,
        on_capture: super::AudioCaptureCallback,
        on_render: super::AudioRenderCallback,
    ) -> Option<Result<Box<dyn HwAudioDuplexHandle>>> {
        if config.input_device.is_some() || config.output_device.is_some() {
            return None;
        }
        Some(
            MacVoiceProcessingIo::start(
                config.sample_rate,
                config.enable_aec_ns,
                config.enable_agc,
                move |samples| on_capture(samples),
                move |samples| on_render(samples),
            )
            .map(|v| Box::new(v) as Box<dyn HwAudioDuplexHandle>),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_frame(tag: u8) -> VideoFrame {
        VideoFrame::from_bgra(2, 2, vec![tag; 16])
    }

    #[test]
    fn av1_hw_fallback_switches_to_sw_after_eight_consecutive_failures() {
        let mut state: Av1FallbackState<&str, &str> = Av1FallbackState::new(Some("hw"), None);
        let mut sw_created = 0u32;

        // 7 consecutive HW failures keep HW active without initializing SW.
        for _ in 0..7 {
            let out = state.decode_with(
                b"bad",
                |_, _| None,
                || {
                    sw_created += 1;
                    Some("sw")
                },
                |_, _| Some(dummy_frame(2)),
            );
            assert!(out.is_none());
            assert!(state.is_hw_active());
            assert_eq!(sw_created, 0);
        }

        // A single HW success resets the consecutive failure counter.
        let ok = state.decode_with(
            b"good",
            |_, _| Some(dummy_frame(1)),
            || {
                sw_created += 1;
                Some("sw")
            },
            |_, _| Some(dummy_frame(2)),
        );
        assert!(ok.is_some());
        assert_eq!(state.hw_consecutive_failures, 0);
        assert!(state.is_hw_active());

        // Now 7 more failures still stay on HW; 8th failure permanently falls back to SW and decodes!
        for _ in 0..7 {
            assert!(
                state
                    .decode_with(
                        b"bad",
                        |_, _| None,
                        || Some("sw"),
                        |_, _| Some(dummy_frame(2))
                    )
                    .is_none()
            );
            assert!(state.is_hw_active());
        }
        let fallback_frame = state
            .decode_with(
                b"fallback",
                |_, _| None,
                || {
                    sw_created += 1;
                    Some("sw")
                },
                |_, _| Some(dummy_frame(9)),
            )
            .expect("8th failure must switch to SW and decode unit");
        assert!(!state.is_hw_active());
        assert_eq!(sw_created, 1);
        assert_eq!(fallback_frame.into_bgra().unwrap().2[0], 9);

        // Subsequent calls go straight to SW without re-invoking SW factory.
        let next = state
            .decode_with(
                b"next",
                |_, _| unreachable!("HW must no longer be invoked after fallback"),
                || {
                    sw_created += 1;
                    Some("sw")
                },
                |_, _| Some(dummy_frame(7)),
            )
            .expect("SW continues decoding");
        assert_eq!(sw_created, 1);
        assert_eq!(next.into_bgra().unwrap().2[0], 7);
    }

    struct StubCameraHandle {
        stopped: Arc<std::sync::atomic::AtomicBool>,
    }

    impl CameraHandle for StubCameraHandle {
        fn set_blur(&self, _enabled: bool) {}
        fn force_keyframe(&self) {}
        fn set_target_bitrate(&self, _bitrate_bps: u32) {}
        fn stop(&mut self) {
            self.stopped
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn sample_camera_config(device_index: Option<usize>) -> CameraConfig {
        CameraConfig {
            device_index,
            max_w: 1280,
            max_h: 720,
            max_fps: 24,
            bitrate_bps: 1_200_000,
            blur_background: true,
        }
    }

    #[test]
    fn resolve_camera_start_config_handles_preferred_fallback_and_empty_devices() {
        let devices = vec![
            CameraDeviceInfo {
                index: 0,
                name: "FaceTime HD Camera".into(),
            },
            CameraDeviceInfo {
                index: 2,
                name: "Studio Display Camera".into(),
            },
        ];

        // Empty list returns an explicit Video error without panicking.
        assert!(matches!(
            resolve_camera_start_config(sample_camera_config(None), &[]),
            Err(HuddleError::Video(_))
        ));

        // Default None picks the first available camera.
        let picked_default =
            resolve_camera_start_config(sample_camera_config(None), &devices).unwrap();
        assert_eq!(picked_default.device_index, Some(0));

        // Preferred camera present in list is preserved.
        let picked_preferred =
            resolve_camera_start_config(sample_camera_config(Some(2)), &devices).unwrap();
        assert_eq!(picked_preferred.device_index, Some(2));

        // Stale/unplugged index falls back to the primary camera.
        let picked_fallback =
            resolve_camera_start_config(sample_camera_config(Some(9)), &devices).unwrap();
        assert_eq!(picked_fallback.device_index, Some(0));
    }

    #[test]
    fn start_camera_with_resolves_device_and_propagates_native_errors_without_fallback() {
        let devices = vec![
            CameraDeviceInfo {
                index: 0,
                name: "Built-in".into(),
            },
            CameraDeviceInfo {
                index: 1,
                name: "Continuity iPhone".into(),
            },
        ];

        // When preferred device is unplugged (index 5), native starter receives fallback index 0.
        let (h264_tx, _h264_rx) = mpsc::unbounded_channel();
        let (prev_tx, _prev_rx) = mpsc::unbounded_channel();
        let seen_idx = Arc::new(std::sync::Mutex::new(None));
        let seen_idx_cb = Arc::clone(&seen_idx);
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped_cb = Arc::clone(&stopped);

        let mut handle = start_camera_with(
            sample_camera_config(Some(5)),
            h264_tx,
            prev_tx,
            || devices.clone(),
            move |cfg, _, _| {
                *seen_idx_cb.lock().unwrap() = cfg.device_index;
                Ok(Box::new(StubCameraHandle {
                    stopped: stopped_cb,
                }))
            },
        )
        .expect("start_camera_with should succeed with fallback index");
        assert_eq!(*seen_idx.lock().unwrap(), Some(0));
        handle.stop();
        assert!(stopped.load(std::sync::atomic::Ordering::Relaxed));

        // Empty device list fails before invoking native starter.
        let (h264_tx2, _) = mpsc::unbounded_channel();
        let (prev_tx2, _) = mpsc::unbounded_channel();
        let err = start_camera_with(
            sample_camera_config(None),
            h264_tx2,
            prev_tx2,
            Vec::new,
            |_, _, _| unreachable!("native starter must not run when no cameras exist"),
        )
        .err()
        .expect("empty devices must fail");
        assert!(matches!(err, HuddleError::Video(_)));

        // Native startup error propagates directly (no nokhwa fallback on macOS).
        let (h264_tx3, _) = mpsc::unbounded_channel();
        let (prev_tx3, _) = mpsc::unbounded_channel();
        let native_err = start_camera_with(
            sample_camera_config(Some(1)),
            h264_tx3,
            prev_tx3,
            || devices,
            |cfg, _, _| {
                assert_eq!(cfg.device_index, Some(1));
                Err(HuddleError::Video("camera permission denied".into()))
            },
        )
        .err()
        .expect("native error must propagate");
        assert!(native_err.to_string().contains("camera permission denied"));
    }
}
