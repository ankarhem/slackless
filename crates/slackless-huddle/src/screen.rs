//! Native screen and window capture + H.264 encoding for Slack Huddle screen sharing
//! (`F-COLLAB-021`, `F-COLLAB-024`).
//!
//! - macOS 12.3+: hardware-accelerated `ScreenCaptureKit` (`SCStream`) + Apple Silicon
//!   `VideoToolbox` H.264 Media Engine (`1920x1200` @ 30 fps, 3 Mbps, event-driven).
//! - Cross-platform fallback: tuned `xcap` + single-pass integer area-averaged downscaling +
//!   OpenH264 `ScreenContentRealTime` (`1920x1200` @ 20 fps, 2.8 Mbps, dirty-frame skip).

use crate::backend::platform_backend;
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
    // On macOS, calling `xcap::Window::all()` without Screen Recording permission logs a noisy
    // warning per window (`Screen Recording permission not granted`) and returns redacted titles.
    // Monitors still enumerate cleanly, so skip window enumeration until permission is granted.
    let can_list_windows = crate::has_screen_capture_permission();
    if can_list_windows && let Ok(windows) = xcap::Window::all() {
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

pub(crate) fn is_system_chrome_window(app: &str, title: &str) -> bool {
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
    if !crate::has_screen_capture_permission() {
        return Vec::new();
    }
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
                    if let Some(rgba) = img
                        && let Some(frame) = downscale_rgba_thumbnail(&rgba, max_w, max_h)
                    {
                        *slot = Some((format!("monitor:{id}"), frame));
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
                    if let Some(rgba) = img
                        && let Some(frame) = downscale_rgba_thumbnail(&rgba, max_w, max_h)
                    {
                        bucket.push((format!("window:{id}"), frame));
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
    let (dw, dh) = crate::video::compute_fit_dimensions(sw, sh, max_w, max_h, false);
    let raw = src.as_raw();
    let mut bgra = vec![0u8; (dw * dh * 4) as usize];
    crate::video::box_downscale_rgb::<4, _>(
        raw,
        sw as usize,
        sh as usize,
        dw as usize,
        dh as usize,
        4,
        |dx, dy, r, g, b| {
            let dst = (dy * dw as usize + dx) * 4;
            bgra[dst] = b;
            bgra[dst + 1] = g;
            bgra[dst + 2] = r;
            bgra[dst + 3] = 255;
        },
    );
    Some(VideoFrame::from_bgra(dw, dh, bgra))
}

/// Drains macOS Objective-C autoreleased objects created by `xcap` on plain Rust worker threads.
pub(crate) fn with_autorelease_pool<T>(f: impl FnOnce() -> T) -> T {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "objc")]
        unsafe extern "C" {
            // Pushing a pool has no preconditions; it only returns the token to pop.
            safe fn objc_autoreleasePoolPush() -> *mut std::ffi::c_void;
            fn objc_autoreleasePoolPop(pool: *mut std::ffi::c_void);
        }
        let pool = objc_autoreleasePoolPush();
        let res = f();
        // SAFETY: `pool` is the token pushed on this thread above and not popped yet; popping it
        // also drains any pool `f` left pushed.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_chrome_window_filters_macos_system_ui_and_keeps_user_apps() {
        for app in [
            "Dock",
            " NotificationCenter ",
            "Notiscenter",
            "Window Server",
            "Control Center",
            "Kontrollcenter",
            "SystemUIServer",
            "Spotlight",
        ] {
            assert!(is_system_chrome_window(app, "anything"), "app={app}");
        }
        for title in ["Dock", "Menubar", "StatusIndicator"] {
            assert!(
                is_system_chrome_window("SomeProcess", title),
                "title={title}"
            );
        }
        assert!(!is_system_chrome_window(
            "Firefox",
            "GitHub - Pull Requests"
        ));
        assert!(!is_system_chrome_window("Terminal", "zsh"));
    }

    #[test]
    fn downscale_rgba_thumbnail_swaps_bgra_scales_and_handles_zero_dims() {
        let zero = xcap::image::RgbaImage::new(0, 0);
        assert!(downscale_rgba_thumbnail(&zero, 160, 90).is_none());

        let mut img = xcap::image::RgbaImage::new(40, 20);
        for px in img.pixels_mut() {
            *px = xcap::image::Rgba([220, 110, 30, 128]);
        }
        assert!(downscale_rgba_thumbnail(&img, 0, 10).is_none());
        assert!(downscale_rgba_thumbnail(&img, 10, 0).is_none());

        let frame = downscale_rgba_thumbnail(&img, 20, 20).expect("thumbnail");
        assert_eq!((frame.width, frame.height), (20, 10));
        let (_, _, bgra) = frame.into_bgra().unwrap();
        assert_eq!(bgra.len(), 20 * 10 * 4);
        // RGBA(220, 110, 30, 128) -> BGRA(30, 110, 220, 255).
        assert_eq!(&bgra[0..4], &[30, 110, 220, 255]);
    }

    #[test]
    fn fast_stride_hash_is_deterministic_and_detects_changes() {
        let h_empty = fast_stride_hash(&[]);
        assert_eq!(h_empty, fast_stride_hash(&[]));

        let a = vec![42u8; 8192];
        let mut b = a.clone();
        assert_eq!(fast_stride_hash(&a), fast_stride_hash(&b));
        // Mutating a sampled byte (index 0) must change the hash.
        b[0] = 99;
        assert_ne!(fast_stride_hash(&a), fast_stride_hash(&b));
    }
}
