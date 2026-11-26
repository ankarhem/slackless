//! Portable cross-platform software media backend (baseline + automatic fallback for macOS,
//! Linux, and Windows).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[cfg(not(target_os = "macos"))]
use nokhwa::Camera;
#[cfg(not(target_os = "macos"))]
use nokhwa::pixel_format::RgbFormat;
#[cfg(not(target_os = "macos"))]
use nokhwa::utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType};
use openh264::decoder::Decoder as H264Decoder;
use openh264::formats::YUVSource as _;
use tokio::sync::mpsc;

#[cfg(not(target_os = "macos"))]
use super::{
    AudioDeviceChangeKind, AudioDeviceMonitorHandle, CameraConfig, CameraHandle,
    CameraPreviewConfig, CameraPreviewHandle, MediaBackend, MediaCapabilities,
};
use super::{ScreenShareConfig, ScreenShareHandle, VideoDecoderHandle, VideoFrameSink};
use crate::av1::Av1Decoder;
use crate::metrics::HuddlePerfCounters;
use crate::screen::{capture_once_result, fast_stride_hash, with_autorelease_pool};
#[cfg(not(target_os = "macos"))]
use crate::video::{
    CameraDeviceInfo, apply_portrait_blur, normalize_camera_devices, prepare_preview_frame_rgb,
    resize_rgb_even, rgb_to_bgra,
};
use crate::video::{
    H264VideoEncoder, ScreenScaleScratch, VideoFrame, downscale_rgba_area_to_bgra_and_i420,
};
use crate::{HuddleError, Result};

#[cfg(not(target_os = "macos"))]
pub struct PortableMediaBackend;

#[cfg(not(target_os = "macos"))]
pub static PORTABLE_BACKEND: PortableMediaBackend = PortableMediaBackend;

// ---------------------------------------------------------------------------
// Portable Software Screen Share (`xcap` + OpenH264 `ScreenContentRealTime`)
// ---------------------------------------------------------------------------

pub(crate) struct SoftwareScreenShare {
    running: Arc<AtomicBool>,
    force_kf: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl SoftwareScreenShare {
    pub(crate) fn start(
        config: ScreenShareConfig,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        on_preview: VideoFrameSink,
        perf: Arc<HuddlePerfCounters>,
    ) -> Result<Self> {
        perf.set_tx_backend("xcap + OpenH264 ScreenContent (SW)");
        let running = Arc::new(AtomicBool::new(true));
        let force_kf = Arc::new(AtomicBool::new(true));
        let run_flag = Arc::clone(&running);
        let kf_flag = Arc::clone(&force_kf);
        let perf_worker = Arc::clone(&perf);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);
        let source_id = config.source_id;
        let max_w = config.max_w;
        let max_h = config.max_h;

        let worker = std::thread::Builder::new()
            .name("slackless-huddle-screen".to_owned())
            .spawn(move || {
                let mut enc = match H264VideoEncoder::new_screen() {
                    Ok(e) => {
                        let _ = ready_tx.send(Ok(()));
                        e
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };

                let mut scratch = ScreenScaleScratch::default();
                let mut logged_first_frame = false;
                let mut last_err_log: Option<Instant> = None;
                let mut last_hash: Option<u64> = None;
                let mut last_sent = Instant::now() - Duration::from_secs(10);
                let frame_interval = Duration::from_millis(50); // 20 fps deadline-paced
                let mut next_tick = Instant::now();

                while run_flag.load(Ordering::Relaxed) {
                    let t0 = Instant::now();
                    let captured = with_autorelease_pool(|| capture_once_result(&source_id));
                    match captured {
                        Ok(rgba) => {
                            let (w, h) = downscale_rgba_area_to_bgra_and_i420(
                                rgba.as_raw(),
                                rgba.width(),
                                rgba.height(),
                                max_w,
                                max_h,
                                &mut scratch,
                            );
                            if w >= 2 && h >= 2 {
                                if !logged_first_frame {
                                    logged_first_frame = true;
                                    tracing::info!(
                                        source_id = %source_id,
                                        raw_w = rgba.width(),
                                        raw_h = rgba.height(),
                                        encoded_w = w,
                                        encoded_h = h,
                                        "software screen capture active (OpenH264 ScreenContentRealTime)"
                                    );
                                }
                                let hash = fast_stride_hash(&scratch.y);
                                let forced = kf_flag.swap(false, Ordering::Relaxed);
                                let changed = last_hash != Some(hash);
                                let keepalive_due =
                                    last_sent.elapsed() >= Duration::from_millis(1500);
                                if forced || changed || keepalive_due {
                                    last_hash = Some(hash);
                                    last_sent = Instant::now();
                                    if forced {
                                        enc.force_keyframe();
                                    }
                                    if changed || forced {
                                        on_preview(VideoFrame::from_bgra(
                                            w,
                                            h,
                                            scratch.bgra.clone(),
                                        ));
                                    }
                                    match enc.encode_i420(
                                        &scratch.y,
                                        &scratch.u,
                                        &scratch.v,
                                        w as usize,
                                        h as usize,
                                    ) {
                                        Ok(h264) if !h264.is_empty() => {
                                            let enc_us = t0.elapsed().as_micros() as u64;
                                            perf_worker.record_tx_frame(
                                                w,
                                                h,
                                                h264.len() as u64,
                                                enc_us,
                                            );
                                            let _ = outbound_h264_tx.send(h264);
                                        }
                                        Ok(_) => {}
                                        Err(err) => {
                                            if last_err_log.is_none_or(|t| {
                                                t.elapsed() >= Duration::from_secs(5)
                                            }) {
                                                last_err_log = Some(Instant::now());
                                                tracing::warn!(%err, "screen H.264 encode error");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            if last_err_log.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
                                last_err_log = Some(Instant::now());
                                tracing::warn!(
                                    source_id = %source_id,
                                    %err,
                                    "screen capture failed"
                                );
                            }
                        }
                    }
                    next_tick += frame_interval;
                    let now = Instant::now();
                    if next_tick > now {
                        std::thread::sleep(next_tick - now);
                    } else {
                        next_tick = now;
                    }
                }
            })
            .map_err(|e| HuddleError::Video(e.to_string()))?;

        ready_rx
            .recv()
            .map_err(|e| HuddleError::Video(e.to_string()))??;

        Ok(Self {
            running,
            force_kf,
            worker: Some(worker),
        })
    }
}

impl ScreenShareHandle for SoftwareScreenShare {
    fn force_keyframe(&self) {
        self.force_kf.store(true, Ordering::Relaxed);
    }

    fn set_target_bitrate(&self, _bitrate_bps: u32) {
        // OpenH264 wrapper uses fixed initial target bitrate; future backends override dynamically.
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for SoftwareScreenShare {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// Portable Software Camera (`nokhwa` + OpenH264, non-macOS)
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "macos"))]
pub(crate) struct SoftwareCamera {
    running: Arc<AtomicBool>,
    blur: Arc<AtomicBool>,
    force_kf: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[cfg(not(target_os = "macos"))]
impl SoftwareCamera {
    pub(crate) fn start(
        config: CameraConfig,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        on_preview: VideoFrameSink,
    ) -> Result<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let blur = Arc::new(AtomicBool::new(config.blur_background));
        let force_kf = Arc::new(AtomicBool::new(true));
        let run_flag = Arc::clone(&running);
        let blur_flag = Arc::clone(&blur);
        let kf_flag = Arc::clone(&force_kf);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);
        let idx = config.device_index.unwrap_or(0) as u32;
        let max_w = config.max_w;
        let max_h = config.max_h;

        let worker = std::thread::Builder::new()
            .name("slackless-huddle-camera".to_owned())
            .spawn(move || {
                let open = (|| -> Result<(Camera, H264VideoEncoder)> {
                    let req = RequestedFormat::new::<RgbFormat>(
                        RequestedFormatType::AbsoluteHighestFrameRate,
                    );
                    let mut cam = Camera::new(CameraIndex::Index(idx), req)
                        .map_err(|e| HuddleError::Video(format!("camera open failed: {e}")))?;
                    cam.open_stream()
                        .map_err(|e| HuddleError::Video(format!("camera stream failed: {e}")))?;
                    let enc = H264VideoEncoder::new()?;
                    Ok((cam, enc))
                })();

                let (mut cam, mut enc) = match open {
                    Ok(pair) => {
                        let _ = ready_tx.send(Ok(()));
                        pair
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };

                while run_flag.load(Ordering::Relaxed) {
                    if let Ok(frame) = cam.frame()
                        && let Ok(img) = frame.decode_image::<RgbFormat>()
                    {
                        let (mut rgb, w, h) =
                            resize_rgb_even(img.as_raw(), img.width(), img.height(), max_w, max_h);
                        if w >= 2 && h >= 2 {
                            if blur_flag.load(Ordering::Relaxed) {
                                apply_portrait_blur(&mut rgb, w as usize, h as usize);
                            }
                            let bgra = rgb_to_bgra(&rgb);
                            on_preview(VideoFrame::from_bgra(w, h, bgra));
                            if kf_flag.swap(false, Ordering::Relaxed) {
                                enc.force_keyframe();
                            }
                            if let Ok(h264) = enc.encode_rgb(&rgb, w as usize, h as usize)
                                && !h264.is_empty()
                            {
                                let _ = outbound_h264_tx.send(h264);
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(66));
                }
                let _ = cam.stop_stream();
            })
            .map_err(|e| HuddleError::Video(e.to_string()))?;

        if let Err(err) = ready_rx
            .recv()
            .map_err(|e| HuddleError::Video(e.to_string()))
            .and_then(|r| r)
        {
            running.store(false, Ordering::Relaxed);
            let _ = worker.join();
            return Err(err);
        }

        Ok(Self {
            running,
            blur,
            force_kf,
            worker: Some(worker),
        })
    }
}

#[cfg(not(target_os = "macos"))]
impl CameraHandle for SoftwareCamera {
    fn set_blur(&self, enabled: bool) {
        self.blur.store(enabled, Ordering::Relaxed);
    }

    fn force_keyframe(&self) {
        self.force_kf.store(true, Ordering::Relaxed);
    }

    fn set_target_bitrate(&self, _bitrate_bps: u32) {}

    fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl Drop for SoftwareCamera {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(not(target_os = "macos"))]
struct SoftwareCameraPreview {
    running: Arc<AtomicBool>,
    blur: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[cfg(not(target_os = "macos"))]
impl SoftwareCameraPreview {
    fn start(
        config: CameraPreviewConfig,
        on_frame: Arc<dyn Fn(VideoFrame) + Send + Sync>,
    ) -> Result<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let blur = Arc::new(AtomicBool::new(config.blur_background));
        let run_flag = Arc::clone(&running);
        let blur_flag = Arc::clone(&blur);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);
        let idx = config.device_index.unwrap_or(0) as u32;
        let max_w = config.max_w;
        let max_h = config.max_h;
        let sleep_dur = Duration::from_millis((1000 / config.max_fps.max(1)) as u64);

        let worker = std::thread::Builder::new()
            .name("slackless-cam-probe".to_owned())
            .spawn(move || {
                let open = (|| -> Result<Camera> {
                    let req = RequestedFormat::new::<RgbFormat>(
                        RequestedFormatType::AbsoluteHighestFrameRate,
                    );
                    let mut cam = Camera::new(CameraIndex::Index(idx), req)
                        .map_err(|e| HuddleError::Video(e.to_string()))?;
                    cam.open_stream()
                        .map_err(|e| HuddleError::Video(e.to_string()))?;
                    Ok(cam)
                })();
                let mut cam = match open {
                    Ok(c) => {
                        let _ = ready_tx.send(Ok(()));
                        c
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                while run_flag.load(Ordering::Relaxed) {
                    if let Ok(frame) = cam.frame()
                        && let Ok(img) = frame.decode_image::<RgbFormat>()
                        && let Some(vf) = prepare_preview_frame_rgb(
                            img.as_raw(),
                            img.width(),
                            img.height(),
                            max_w,
                            max_h,
                            blur_flag.load(Ordering::Relaxed),
                        )
                    {
                        on_frame(vf);
                    }
                    std::thread::sleep(sleep_dur);
                }
                let _ = cam.stop_stream();
            })
            .map_err(|e| HuddleError::Video(e.to_string()))?;

        if let Err(err) = ready_rx
            .recv()
            .map_err(|e| HuddleError::Video(e.to_string()))
            .and_then(|r| r)
        {
            running.store(false, Ordering::Relaxed);
            let _ = worker.join();
            return Err(err);
        }

        Ok(Self {
            running,
            blur,
            worker: Some(worker),
        })
    }
}

#[cfg(not(target_os = "macos"))]
impl CameraPreviewHandle for SoftwareCameraPreview {
    fn set_blur(&self, enabled: bool) {
        self.blur.store(enabled, Ordering::Relaxed);
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl Drop for SoftwareCameraPreview {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// Portable Software Decoders (Lazy OpenH264 & rav1d)
// ---------------------------------------------------------------------------

pub(crate) struct LazyOpenH264Decoder {
    decoder: Option<H264Decoder>,
}

impl LazyOpenH264Decoder {
    pub(crate) fn new() -> Self {
        Self { decoder: None }
    }

    pub(crate) fn decode_annex_b(&mut self, bitstream: &[u8]) -> Option<VideoFrame> {
        if self.decoder.is_none() {
            self.decoder = H264Decoder::new().ok();
        }
        let yuv = self.decoder.as_mut()?.decode(bitstream).ok()??;
        let (w, h) = yuv.dimensions();
        if w == 0 || h == 0 {
            return None;
        }
        let mut rgba = vec![0u8; w * h * 4];
        yuv.write_rgba8(&mut rgba);
        for px in rgba.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        Some(VideoFrame::from_bgra(w as u32, h as u32, rgba))
    }
}

impl VideoDecoderHandle for LazyOpenH264Decoder {
    fn backend_name(&self) -> &'static str {
        "H.264 (OpenH264 SW)"
    }

    fn decode_unit(&mut self, unit: &[u8]) -> Option<VideoFrame> {
        self.decode_annex_b(unit)
    }
}

pub(crate) struct SoftwareAv1Decoder {
    dec: Av1Decoder,
}

impl SoftwareAv1Decoder {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            dec: Av1Decoder::new()?,
        })
    }
}

impl VideoDecoderHandle for SoftwareAv1Decoder {
    fn backend_name(&self) -> &'static str {
        "AV1 (rav1d SW)"
    }

    fn decode_unit(&mut self, unit: &[u8]) -> Option<VideoFrame> {
        self.dec.decode(unit).ok().flatten()
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn list_nokhwa_cameras() -> Vec<CameraDeviceInfo> {
    let raw = nokhwa::query(ApiBackend::Auto)
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(fallback_ix, info)| {
            let index = match info.index() {
                CameraIndex::Index(i) => *i as usize,
                CameraIndex::String(_) => fallback_ix,
            };
            (index, Some(info.human_name()))
        });
    normalize_camera_devices(raw)
}

#[cfg(not(target_os = "macos"))]
impl MediaBackend for PortableMediaBackend {
    fn name(&self) -> &'static str {
        "Portable Software (OpenH264 / rav1d)"
    }

    fn capabilities(&self) -> MediaCapabilities {
        MediaCapabilities::default()
    }

    fn list_cameras(&self) -> Vec<CameraDeviceInfo> {
        list_nokhwa_cameras()
    }

    fn capture_screen_thumbnails(
        &self,
        monitors_only: bool,
        max_w: u32,
        max_h: u32,
    ) -> Vec<(String, VideoFrame)> {
        crate::screen::capture_screen_thumbnails_xcap(monitors_only, max_w, max_h)
    }

    fn start_screen_share(
        &self,
        config: ScreenShareConfig,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        on_preview: VideoFrameSink,
        perf: Arc<HuddlePerfCounters>,
    ) -> Result<Box<dyn ScreenShareHandle>> {
        Ok(Box::new(SoftwareScreenShare::start(
            config,
            outbound_h264_tx,
            on_preview,
            perf,
        )?))
    }

    fn start_camera(
        &self,
        config: CameraConfig,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        on_preview: VideoFrameSink,
    ) -> Result<Box<dyn CameraHandle>> {
        Ok(Box::new(SoftwareCamera::start(
            config,
            outbound_h264_tx,
            on_preview,
        )?))
    }

    fn start_camera_preview(
        &self,
        config: CameraPreviewConfig,
        on_frame: Arc<dyn Fn(VideoFrame) + Send + Sync>,
    ) -> Result<Box<dyn CameraPreviewHandle>> {
        Ok(Box::new(SoftwareCameraPreview::start(config, on_frame)?))
    }

    fn create_h264_decoder(&self) -> Result<Box<dyn VideoDecoderHandle>> {
        Ok(Box::new(LazyOpenH264Decoder::new()))
    }

    fn create_av1_decoder(&self) -> Result<Box<dyn VideoDecoderHandle>> {
        Ok(Box::new(SoftwareAv1Decoder::new()?))
    }

    fn start_audio_device_monitor(
        &self,
        _on_change: Arc<dyn Fn(AudioDeviceChangeKind) + Send + Sync>,
    ) -> Option<Box<dyn AudioDeviceMonitorHandle>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lazy_openh264_decoder_initializes_lazily_and_decodes_keyframe() {
        let mut dec = LazyOpenH264Decoder::new();
        assert!(dec.decoder.is_none(), "OpenH264 decoder must start lazy");
        assert_eq!(dec.backend_name(), "H.264 (OpenH264 SW)");

        // Garbage bytes return None safely.
        assert!(dec.decode_unit(&[0x00, 0x01, 0xFF]).is_none());
        assert!(dec.decoder.is_some());

        // Real encoded Annex-B frame decodes cleanly to BGRA.
        let mut enc = H264VideoEncoder::new().unwrap();
        let (w, h) = (64usize, 48usize);
        let rgb = vec![120u8; w * h * 3];
        let annex_b = enc.encode_rgb(&rgb, w, h).unwrap();
        let frame = dec
            .decode_unit(&annex_b)
            .expect("LazyOpenH264Decoder should decode valid H.264 keyframe");
        assert_eq!((frame.width, frame.height), (w as u32, h as u32));
        let (_, _, bgra) = frame.into_bgra().unwrap();
        assert_eq!(bgra.len(), w * h * 4);
    }

    #[test]
    fn software_av1_decoder_handles_empty_and_invalid_units_safely() {
        let mut dec = SoftwareAv1Decoder::new().expect("rav1d decoder");
        assert_eq!(dec.backend_name(), "AV1 (rav1d SW)");
        assert!(dec.decode_unit(&[]).is_none());
        assert!(dec.decode_unit(&[0xFF, 0xFE, 0xFD, 0xFC]).is_none());
    }
}
