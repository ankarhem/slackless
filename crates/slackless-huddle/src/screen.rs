//! Native screen and window capture (`xcap`) + H.264 encoding for Slack Huddle screen sharing
//! (`F-COLLAB-021`, `F-COLLAB-024`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;

use crate::video::{H264VideoEncoder, VideoFrame, resize_rgb_even, rgba_to_rgb_and_bgra};
use crate::{HuddleError, Result};

/// Maximum dimensions for screen share encoding (720p, even dimensions for YUV420).
pub const SCREEN_MAX_W: u32 = 1280;
pub const SCREEN_MAX_H: u32 = 720;

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

/// Active screen/window capture thread encoding H.264 frames and emitting local preview frames.
pub struct ScreenCapture {
    running: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ScreenCapture {
    pub fn start(
        source_id: String,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
    ) -> Result<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let run_flag = Arc::clone(&running);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

        let worker = std::thread::Builder::new()
            .name("slackless-huddle-screen".to_owned())
            .spawn(move || {
                let init = H264VideoEncoder::new();
                let mut enc = match init {
                    Ok(e) => {
                        let _ = ready_tx.send(Ok(()));
                        e
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };

                let mut logged_first_frame = false;
                let mut last_err_log: Option<std::time::Instant> = None;

                while run_flag.load(Ordering::Relaxed) {
                    match capture_once_result(&source_id) {
                        Ok(rgba) => {
                            let (rgb_full, _bgra_full) = rgba_to_rgb_and_bgra(rgba.as_raw());
                            let (rgb, w, h) = resize_rgb_even(
                                &rgb_full,
                                rgba.width(),
                                rgba.height(),
                                SCREEN_MAX_W,
                                SCREEN_MAX_H,
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
                                        "screen capture active"
                                    );
                                }
                                let bgra = crate::video::rgb_to_bgra(&rgb);
                                let _ = preview_tx.send(VideoFrame {
                                    width: w,
                                    height: h,
                                    bgra: Arc::new(bgra),
                                });
                                match enc.encode_rgb(&rgb, w as usize, h as usize) {
                                    Ok(h264) if !h264.is_empty() => {
                                        let _ = outbound_h264_tx.send(h264);
                                    }
                                    Ok(_) => {}
                                    Err(err) => {
                                        if last_err_log
                                            .is_none_or(|t| t.elapsed() >= Duration::from_secs(5))
                                        {
                                            last_err_log = Some(std::time::Instant::now());
                                            tracing::warn!(%err, "screen H.264 encode error");
                                        }
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            if last_err_log.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
                                last_err_log = Some(std::time::Instant::now());
                                tracing::warn!(
                                    source_id = %source_id,
                                    %err,
                                    "screen capture failed"
                                );
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(125)); // ~8 fps
                }
            })
            .map_err(|e| HuddleError::Video(e.to_string()))?;

        ready_rx
            .recv()
            .map_err(|e| HuddleError::Video(e.to_string()))??;

        Ok(Self {
            running,
            worker: Some(worker),
        })
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        self.stop();
    }
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
