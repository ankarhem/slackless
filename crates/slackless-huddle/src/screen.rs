//! Native screen and window capture + H.264 encoding for Slack Huddle screen sharing
//! (`F-COLLAB-021`, `F-COLLAB-024`).
//!
//! - macOS 12.3+: hardware-accelerated `ScreenCaptureKit` (`SCStream`) + Apple Silicon
//!   `VideoToolbox` H.264 Media Engine (`1920x1200` @ 30 fps, 3 Mbps, event-driven).
//! - Cross-platform fallback: tuned `xcap` + single-pass integer area-averaged downscaling +
//!   OpenH264 `ScreenContentRealTime` (`1920x1200` @ 20 fps, 2.8 Mbps, dirty-frame skip).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::metrics::HuddlePerfCounters;
use crate::video::{
    H264VideoEncoder, ScreenScaleScratch, VideoFrame, downscale_rgba_area_to_bgra_and_i420,
};
use crate::{HuddleError, Result};

/// Maximum dimensions for crisp 1080p/16:10 screen share encoding (even dimensions for YUV420).
pub const SCREEN_MAX_W: u32 = 1920;
pub const SCREEN_MAX_H: u32 = 1200;
/// Target maximum frame rate for hardware screen share capture.
pub const SCREEN_MAX_FPS: u32 = 30;
/// Target average H.264 video bitrate for screen sharing (3.0 Mbps).
pub const SCREEN_BITRATE_BPS: u32 = 3_000_000;

/// One capturable screen (monitor) or application window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenSource {
    /// Stable identifier: `"monitor:<id>"` or `"window:<id>"`.
    pub id: String,
    pub name: String,
    pub is_monitor: bool,
    pub width: u32,
    pub height: u32,
}

/// Enumerate all available monitors and visible non-minimized application windows.
pub fn list_screen_sources() -> Vec<ScreenSource> {
    let mut out = Vec::new();
    if let Ok(monitors) = xcap::Monitor::all() {
        for m in monitors {
            let Ok(id) = m.id() else { continue };
            let width = m.width().unwrap_or(0);
            let height = m.height().unwrap_or(0);
            if width == 0 || height == 0 {
                continue;
            }
            let primary = m.is_primary().unwrap_or(false);
            let raw_name = m
                .friendly_name()
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| m.name().ok().filter(|s| !s.trim().is_empty()))
                .unwrap_or_else(|| format!("Display {id}"));
            let name = if primary {
                format!("{raw_name} (Primary)")
            } else {
                raw_name
            };
            out.push(ScreenSource {
                id: format!("monitor:{id}"),
                name,
                is_monitor: true,
                width,
                height,
            });
        }
    }
    if let Ok(windows) = xcap::Window::all() {
        for w in windows {
            if w.is_minimized().unwrap_or(false) {
                continue;
            }
            let Ok(id) = w.id() else { continue };
            let width = w.width().unwrap_or(0);
            let height = w.height().unwrap_or(0);
            if width < 120 || height < 120 {
                continue;
            }
            let title = w.title().unwrap_or_default();
            let app = w.app_name().unwrap_or_default();
            if title.trim().is_empty() && app.trim().is_empty() {
                continue;
            }
            let name = if title.trim().is_empty() {
                app
            } else if app.trim().is_empty() || title == app {
                title
            } else {
                format!("{app} — {title}")
            };
            out.push(ScreenSource {
                id: format!("window:{id}"),
                name,
                is_monitor: false,
                width,
                height,
            });
        }
    }
    out
}

enum ScreenCaptureBackend {
    #[cfg(target_os = "macos")]
    MacOsHw(crate::macos_media::ScVtScreenCapture),
    Software {
        running: Arc<AtomicBool>,
        force_kf: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    },
}

/// Active screen/window capture session encoding H.264 frames and emitting local preview frames.
pub struct ScreenCapture {
    backend: ScreenCaptureBackend,
    perf: Arc<HuddlePerfCounters>,
}

impl ScreenCapture {
    pub fn start(
        source_id: String,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
        perf: Arc<HuddlePerfCounters>,
    ) -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            match crate::macos_media::ScVtScreenCapture::start(
                &source_id,
                SCREEN_MAX_W,
                SCREEN_MAX_H,
                SCREEN_MAX_FPS,
                SCREEN_BITRATE_BPS,
                outbound_h264_tx.clone(),
                preview_tx.clone(),
                Arc::clone(&perf),
            ) {
                Ok(hw) => {
                    return Ok(Self {
                        backend: ScreenCaptureBackend::MacOsHw(hw),
                        perf,
                    });
                }
                Err(err) => {
                    tracing::warn!(
                        source_id = %source_id,
                        %err,
                        "ScreenCaptureKit + VideoToolbox unavailable; falling back to tuned OpenH264"
                    );
                }
            }
        }

        Self::start_software(source_id, outbound_h264_tx, preview_tx, perf)
    }

    fn start_software(
        source_id: String,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
        perf: Arc<HuddlePerfCounters>,
    ) -> Result<Self> {
        perf.set_tx_backend("xcap + OpenH264 ScreenContent (SW)");
        let running = Arc::new(AtomicBool::new(true));
        let force_kf = Arc::new(AtomicBool::new(true));
        let run_flag = Arc::clone(&running);
        let kf_flag = Arc::clone(&force_kf);
        let perf_worker = Arc::clone(&perf);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

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
                    match capture_once_result(&source_id) {
                        Ok(rgba) => {
                            let (w, h) = downscale_rgba_area_to_bgra_and_i420(
                                rgba.as_raw(),
                                rgba.width(),
                                rgba.height(),
                                SCREEN_MAX_W,
                                SCREEN_MAX_H,
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
                                        let _ = preview_tx.send(VideoFrame {
                                            width: w,
                                            height: h,
                                            bgra: Arc::new(scratch.bgra.clone()),
                                        });
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
            backend: ScreenCaptureBackend::Software {
                running,
                force_kf,
                worker: Some(worker),
            },
            perf,
        })
    }

    /// Force an immediate IDR keyframe on the next encoded screen frame.
    pub fn force_keyframe(&self) {
        match &self.backend {
            #[cfg(target_os = "macos")]
            ScreenCaptureBackend::MacOsHw(hw) => hw.force_keyframe(),
            ScreenCaptureBackend::Software { force_kf, .. } => {
                force_kf.store(true, Ordering::Relaxed);
            }
        }
    }

    pub fn stop(&mut self) {
        match &mut self.backend {
            #[cfg(target_os = "macos")]
            ScreenCaptureBackend::MacOsHw(hw) => hw.stop(),
            ScreenCaptureBackend::Software {
                running, worker, ..
            } => {
                running.store(false, Ordering::Relaxed);
                if let Some(handle) = worker.take() {
                    let _ = handle.join();
                }
            }
        }
        self.perf.clear_tx();
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Fast strided FNV-1a fingerprint over the luma plane to skip re-encoding static frames.
fn fast_stride_hash(y: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let step = (y.len() / 2048).max(1);
    for &b in y.iter().step_by(step) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub(crate) fn capture_once(source_id: &str) -> Option<xcap::image::RgbaImage> {
    capture_once_result(source_id).ok()
}

fn capture_once_result(source_id: &str) -> std::result::Result<xcap::image::RgbaImage, String> {
    if let Some(mon_id) = source_id
        .strip_prefix("monitor:")
        .and_then(|s| s.parse::<u32>().ok())
    {
        let monitor = xcap::Monitor::all()
            .map_err(|e| format!("Monitor::all failed: {e}"))?
            .into_iter()
            .find(|m| m.id().ok() == Some(mon_id))
            .ok_or_else(|| format!("monitor {mon_id} not found"))?;
        return monitor
            .capture_image()
            .map_err(|e| format!("monitor {mon_id} capture_image failed: {e}"));
    }
    if let Some(win_id) = source_id
        .strip_prefix("window:")
        .and_then(|s| s.parse::<u32>().ok())
    {
        let window = xcap::Window::all()
            .map_err(|e| format!("Window::all failed: {e}"))?
            .into_iter()
            .find(|w| w.id().ok() == Some(win_id))
            .ok_or_else(|| format!("window {win_id} not found"))?;
        return window
            .capture_image()
            .map_err(|e| format!("window {win_id} capture_image failed: {e}"));
    }
    // Fallback to primary monitor when no explicit id matches.
    let monitor = xcap::Monitor::all()
        .map_err(|e| format!("Monitor::all failed: {e}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "no monitors available".to_owned())?;
    monitor
        .capture_image()
        .map_err(|e| format!("primary monitor capture_image failed: {e}"))
}
