//! Native screen and window capture + H.264 encoding for Slack Huddle screen sharing
//! (`F-COLLAB-021`, `F-COLLAB-024`).
//!
//! - macOS 12.3+: hardware-accelerated `ScreenCaptureKit` (`SCStream`) + Apple Silicon
//!   `VideoToolbox` H.264 Media Engine (`1920x1200` @ 30 fps, 3 Mbps, event-driven).
//! - Cross-platform fallback: tuned `xcap` + single-pass integer area-averaged downscaling +
//!   OpenH264 `ScreenContentRealTime` (`1920x1200` @ 20 fps, 2.8 Mbps, dirty-frame skip).

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::Result;
use crate::backend::{ScreenShareConfig, ScreenShareHandle, platform_backend};
use crate::metrics::HuddlePerfCounters;
use crate::video::VideoFrame;

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
            if is_system_chrome_window(&app, &title) {
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

fn is_system_chrome_window(app: &str, title: &str) -> bool {
    let app_l = app.trim().to_ascii_lowercase();
    let title_l = title.trim().to_ascii_lowercase();
    matches!(
        app_l.as_str(),
        "dock"
            | "notificationcenter"
            | "notiscenter"
            | "window server"
            | "control center"
            | "kontrollcenter"
            | "systemuiserver"
            | "spotlight"
    ) || matches!(title_l.as_str(), "dock" | "menubar" | "statusindicator")
}

/// Capture live downscaled BGRA preview thumbnails for monitors (`monitors_only == true`) or
/// visible windows (`monitors_only == false`) via the active platform [`crate::backend::MediaBackend`]
/// (GPU-accelerated `SCScreenshotManager` on macOS 14+, portable `xcap` fallback elsewhere).
pub fn capture_screen_thumbnails(
    monitors_only: bool,
    max_w: u32,
    max_h: u32,
) -> Vec<(String, VideoFrame)> {
    platform_backend().capture_screen_thumbnails(monitors_only, max_w, max_h)
}

/// Portable `xcap` + parallel CPU box-downscale thumbnail capture fallback (Linux/Windows/macOS<14).
pub(crate) fn capture_screen_thumbnails_xcap(
    monitors_only: bool,
    max_w: u32,
    max_h: u32,
) -> Vec<(String, VideoFrame)> {
    if monitors_only {
        let Ok(monitors) = xcap::Monitor::all() else {
            return Vec::new();
        };
        let mut results: Vec<Option<(String, VideoFrame)>> = vec![None; monitors.len()];
        std::thread::scope(|s| {
            for (m, slot) in monitors.into_iter().zip(results.iter_mut()) {
                s.spawn(move || {
                    let Ok(id) = m.id() else { return };
                    let img = with_autorelease_pool(|| m.capture_image().ok());
                    if let Some(rgba) = img {
                        if let Some(frame) = downscale_rgba_thumbnail(&rgba, max_w, max_h) {
                            *slot = Some((format!("monitor:{id}"), frame));
                        }
                    }
                });
            }
        });
        return results.into_iter().flatten().collect();
    }

    let Ok(windows) = xcap::Window::all() else {
        return Vec::new();
    };
    let candidates: Vec<(u32, xcap::Window)> = windows
        .into_iter()
        .filter_map(|w| {
            if w.is_minimized().unwrap_or(false) {
                return None;
            }
            let id = w.id().ok()?;
            let width = w.width().unwrap_or(0);
            let height = w.height().unwrap_or(0);
            if width < 120 || height < 120 {
                return None;
            }
            let title = w.title().unwrap_or_default();
            let app = w.app_name().unwrap_or_default();
            if (title.trim().is_empty() && app.trim().is_empty())
                || is_system_chrome_window(&app, &title)
            {
                return None;
            }
            Some((id, w))
        })
        .take(18)
        .collect();

    if candidates.is_empty() {
        return Vec::new();
    }
    let workers = candidates.len().min(4);
    let chunk_size = candidates.len().div_ceil(workers);
    let mut buckets: Vec<Vec<(String, VideoFrame)>> = vec![Vec::new(); workers];
    std::thread::scope(|s| {
        for (chunk, bucket) in candidates.chunks(chunk_size).zip(buckets.iter_mut()) {
            s.spawn(move || {
                for (id, w) in chunk {
                    let img = with_autorelease_pool(|| w.capture_image().ok());
                    if let Some(rgba) = img {
                        if let Some(frame) = downscale_rgba_thumbnail(&rgba, max_w, max_h) {
                            bucket.push((format!("window:{id}"), frame));
                        }
                    }
                }
            });
        }
    });
    buckets.into_iter().flatten().collect()
}

/// Fast box/area-averaged RGBA -> BGRA thumbnail downscaler.
fn downscale_rgba_thumbnail(
    src: &xcap::image::RgbaImage,
    max_w: u32,
    max_h: u32,
) -> Option<VideoFrame> {
    let sw = src.width();
    let sh = src.height();
    if sw == 0 || sh == 0 || max_w == 0 || max_h == 0 {
        return None;
    }
    let scale = (max_w as f64 / sw as f64)
        .min(max_h as f64 / sh as f64)
        .min(1.0);
    let dw = ((sw as f64 * scale).round() as u32).max(2);
    let dh = ((sh as f64 * scale).round() as u32).max(2);
    let raw = src.as_raw();
    let mut bgra = vec![0u8; (dw * dh * 4) as usize];
    for dy in 0..dh {
        let sy0 = ((dy as u64 * sh as u64) / dh as u64) as u32;
        let sy1 = (((dy + 1) as u64 * sh as u64) / dh as u64)
            .max(sy0 as u64 + 1)
            .min(sh as u64) as u32;
        for dx in 0..dw {
            let sx0 = ((dx as u64 * sw as u64) / dw as u64) as u32;
            let sx1 = (((dx + 1) as u64 * sw as u64) / dw as u64)
                .max(sx0 as u64 + 1)
                .min(sw as u64) as u32;
            let step_y = ((sy1 - sy0) / 4).max(1);
            let step_x = ((sx1 - sx0) / 4).max(1);
            let (mut r, mut g, mut b, mut count) = (0u32, 0u32, 0u32, 0u32);
            let mut sy = sy0;
            while sy < sy1 {
                let row = (sy * sw) as usize;
                let mut sx = sx0;
                while sx < sx1 {
                    let off = (row + sx as usize) * 4;
                    r += raw[off] as u32;
                    g += raw[off + 1] as u32;
                    b += raw[off + 2] as u32;
                    count += 1;
                    sx += step_x;
                }
                sy += step_y;
            }
            let c = count.max(1);
            let dst = ((dy * dw + dx) * 4) as usize;
            bgra[dst] = (b / c) as u8;
            bgra[dst + 1] = (g / c) as u8;
            bgra[dst + 2] = (r / c) as u8;
            bgra[dst + 3] = 255;
        }
    }
    Some(VideoFrame::from_bgra(dw, dh, bgra))
}

/// Active screen/window capture session backed by the active platform [`crate::backend::MediaBackend`].
pub struct ScreenCapture {
    backend: Box<dyn ScreenShareHandle>,
    perf: Arc<HuddlePerfCounters>,
}

impl ScreenCapture {
    pub fn start(
        source_id: String,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
        perf: Arc<HuddlePerfCounters>,
    ) -> Result<Self> {
        let backend = platform_backend().start_screen_share(
            ScreenShareConfig {
                source_id,
                max_w: SCREEN_MAX_W,
                max_h: SCREEN_MAX_H,
                max_fps: SCREEN_MAX_FPS,
                bitrate_bps: SCREEN_BITRATE_BPS,
            },
            outbound_h264_tx,
            preview_tx,
            Arc::clone(&perf),
        )?;
        Ok(Self { backend, perf })
    }

    /// Force an immediate IDR keyframe on the next encoded screen frame.
    pub fn force_keyframe(&self) {
        self.backend.force_keyframe();
    }

    /// Dynamically adjust target encoder bitrate (e.g. from RTCP REMB feedback).
    pub fn set_target_bitrate(&self, bitrate_bps: u32) {
        self.backend.set_target_bitrate(bitrate_bps);
    }

    pub fn stop(&mut self) {
        self.backend.stop();
        self.perf.clear_tx();
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Drains macOS Objective-C autoreleased objects created by `xcap` on plain Rust worker threads.
pub(crate) fn with_autorelease_pool<T>(f: impl FnOnce() -> T) -> T {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "objc")]
        unsafe extern "C" {
            fn objc_autoreleasePoolPush() -> *mut std::ffi::c_void;
            fn objc_autoreleasePoolPop(pool: *mut std::ffi::c_void);
        }
        let pool = unsafe { objc_autoreleasePoolPush() };
        let res = f();
        unsafe { objc_autoreleasePoolPop(pool) };
        res
    }
    #[cfg(not(target_os = "macos"))]
    {
        f()
    }
}

/// Fast strided FNV-1a fingerprint over the luma plane to skip re-encoding static frames.
pub(crate) fn fast_stride_hash(y: &[u8]) -> u64 {
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

pub(crate) fn capture_once_result(
    source_id: &str,
) -> std::result::Result<xcap::image::RgbaImage, String> {
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
