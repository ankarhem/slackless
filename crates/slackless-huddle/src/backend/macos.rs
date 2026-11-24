//! macOS hardware-accelerated [`MediaBackend`] implementation (all M1/M2/M3/M4 & Intel Macs).
//!
//! Runtime-probes hardware AV1 decode (`VTIsHardwareDecodeSupported`) so M3/M4 Macs use HW AV1
//! decode while M1/M2 Macs gracefully prefer HW H.264 decode and fall back to `rav1d` for AV1.

#![cfg(target_os = "macos")]

use std::sync::Arc;

use tokio::sync::mpsc;

use super::portable::{
    LazyOpenH264Decoder, SoftwareAv1Decoder, SoftwareCamera, SoftwareScreenShare,
    list_nokhwa_cameras,
};
use super::{
    AudioDeviceChangeKind, AudioDeviceMonitorHandle, CameraConfig, CameraHandle,
    HwAudioDuplexHandle, MediaBackend, MediaCapabilities, ScreenShareConfig, ScreenShareHandle,
    VideoDecoderHandle,
};
use crate::Result;
use crate::macos_media::{
    AvVtCameraCapture, CoreAudioDeviceMonitor, MacVoiceProcessingIo, ScVtScreenCapture,
    VtAv1Decoder, VtH264Decoder, capture_macos_thumbnails, list_macos_cameras,
    supports_hw_av1_decode, supports_hw_h264_decode,
};
use crate::metrics::HuddlePerfCounters;
use crate::video::{CameraDeviceInfo, VideoFrame};

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

    fn stop(&mut self) {
        AvVtCameraCapture::stop(self);
    }
}

struct MacAudioDeviceMonitor {
    _inner: CoreAudioDeviceMonitor,
}

impl AudioDeviceMonitorHandle for MacAudioDeviceMonitor {}

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
        if let Some(vt) = self.vt.as_mut() {
            if let Some(frame) = vt.decode_annex_b(unit) {
                return Some(frame);
            }
        }
        self.sw.decode_annex_b(unit)
    }
}

/// Runtime-gated VideoToolbox HW AV1 decoder (M3/M4+) with seamless lazy `rav1d` fallback
/// (always uses `rav1d` on M1/M2 without assuming M3+).
struct MacAv1Decoder {
    vt: Option<VtAv1Decoder>,
    sw: Option<SoftwareAv1Decoder>,
    vt_consecutive_failures: u32,
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
            vt,
            sw,
            vt_consecutive_failures: 0,
        })
    }
}

impl VideoDecoderHandle for MacAv1Decoder {
    fn backend_name(&self) -> &'static str {
        if self.vt.is_some() {
            "AV1 (VideoToolbox HW)"
        } else {
            "AV1 (rav1d SW)"
        }
    }

    fn decode_unit(&mut self, unit: &[u8]) -> Option<VideoFrame> {
        if let Some(vt) = self.vt.as_mut() {
            if let Some(frame) = vt.decode_temporal_unit(unit) {
                self.vt_consecutive_failures = 0;
                return Some(frame);
            }
            // Wait for initial sequence header / keyframe; only fall back to `rav1d` if HW
            // fails repeatedly on decodable units (e.g. unsupported profile/12-bit).
            self.vt_consecutive_failures = self.vt_consecutive_failures.saturating_add(1);
            if self.vt_consecutive_failures < 8 {
                return None;
            }
            tracing::info!("VideoToolbox HW AV1 decode unavailable for stream; switching to rav1d");
            self.vt = None;
        }
        if self.sw.is_none() {
            self.sw = SoftwareAv1Decoder::new().ok();
        }
        self.sw.as_mut()?.decode_unit(unit)
    }
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
            native_system_audio_tap: crate::macos_audio_tap::supports_native_system_audio_tap(),
            native_device_hotplug: true,
            hw_audio_dsp: true,
        }
    }

    fn list_cameras(&self) -> Vec<CameraDeviceInfo> {
        let native = list_macos_cameras();
        if !native.is_empty() {
            return native;
        }
        list_nokhwa_cameras()
    }

    fn capture_screen_thumbnails(
        &self,
        monitors_only: bool,
        max_w: u32,
        max_h: u32,
    ) -> Vec<(String, VideoFrame)> {
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
        match ScVtScreenCapture::start(
            &config.source_id,
            config.max_w,
            config.max_h,
            config.max_fps,
            config.bitrate_bps,
            outbound_h264_tx.clone(),
            preview_tx.clone(),
            Arc::clone(&perf),
        ) {
            Ok(hw) => Ok(Box::new(hw)),
            Err(err) => {
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
        match AvVtCameraCapture::start(
            config.device_index,
            config.max_w,
            config.max_h,
            config.max_fps,
            config.bitrate_bps,
            config.blur_background,
            outbound_h264_tx.clone(),
            preview_tx.clone(),
        ) {
            Ok(hw) => Ok(Box::new(hw)),
            Err(err) => {
                tracing::warn!(
                    %err,
                    "AVFoundation + VideoToolbox camera unavailable; falling back to nokhwa + OpenH264"
                );
                Ok(Box::new(SoftwareCamera::start(
                    config,
                    outbound_h264_tx,
                    preview_tx,
                )?))
            }
        }
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

    fn start_hw_audio_duplex(
        &self,
        sample_rate: u32,
        input_device: Option<&str>,
        output_device: Option<&str>,
        enable_aec_ns: bool,
        enable_agc: bool,
        on_capture: super::AudioCaptureCallback,
        on_render: super::AudioRenderCallback,
    ) -> Option<Result<Box<dyn HwAudioDuplexHandle>>> {
        // VoiceProcessingIO binds the system default aggregate device seamlessly (including
        // AirPods/Bluetooth hot-swaps). When the user explicitly pins custom non-default devices,
        // return `None` so `AudioEngine` uses `cpal` + `sonora` with exact device selection.
        if input_device.is_some() || output_device.is_some() {
            return None;
        }
        Some(
            MacVoiceProcessingIo::start(
                sample_rate,
                enable_aec_ns,
                enable_agc,
                move |samples| on_capture(samples),
                move |samples| on_render(samples),
            )
            .map(|v| Box::new(v) as Box<dyn HwAudioDuplexHandle>),
        )
    }
}
