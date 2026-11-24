//! GPU video frame lifecycle (`HuddleVideoStore`) and Catmull-Rom drawing / reaction entries
//! shared between the sidebar mini-dock and the pop-out Huddle window.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui_kit::gpui::{App, AppContext as _, Context, Entity, RenderImage};
use slackless_core::huddle::{HuddleDrawSegment, VideoFrame, VideoTileKey};
use slackless_core::types::UserId;

/// Convert a [`VideoFrame`] into a GPU-ready GPUI [`RenderImage`] (uses NEON `vImage` on macOS
/// GPU surfaces, or clones bytes for portable BGRA frames).
pub fn frame_to_render_image(frame: &VideoFrame) -> Option<Arc<RenderImage>> {
    let (w, h, bgra) = frame.to_bgra_fit(0, 0)?;
    let rgba = image::RgbaImage::from_raw(w, h, bgra)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(rgba)
    ])))
}

/// Convert an owned [`VideoFrame`] into a [`RenderImage`] without cloning when uniquely owned.
pub fn frame_into_render_image(frame: VideoFrame) -> Option<Arc<RenderImage>> {
    let (w, h, bgra) = frame.into_bgra()?;
    let rgba = image::RgbaImage::from_raw(w, h, bgra)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(rgba)
    ])))
}

/// Display-ready video frame stored in [`HuddleVideoStore`].
///
/// - `Surface` (macOS): zero-copy `IOSurface`-backed `420f` `CVPixelBuffer` painted directly by
///   GPUI's Metal YCbCr shader (`window.paint_surface`) with zero CPU readback or atlas upload.
/// - `Image`: portable GPUI `RenderImage` (used for rounded-corner camera tiles and non-macOS /
///   software fallback frames).
#[derive(Clone)]
pub enum DisplayVideoFrame {
    Image(Arc<RenderImage>),
    #[cfg(target_os = "macos")]
    Surface(Arc<slackless_core::huddle::MacVideoSurface>),
}

impl DisplayVideoFrame {
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::Image(img) => {
                let sz = img.size(0);
                (sz.width.0.max(0) as u32, sz.height.0.max(0) as u32)
            }
            #[cfg(target_os = "macos")]
            Self::Surface(surface) => (surface.width(), surface.height()),
        }
    }

    pub fn as_render_image(&self) -> Option<&Arc<RenderImage>> {
        match self {
            Self::Image(img) => Some(img),
            #[cfg(target_os = "macos")]
            Self::Surface(_) => None,
        }
    }
}

/// RAII guard tracking open Pop-out Huddle Windows so [`HuddleVideoStore`] skips camera GPU
/// conversion and UI notifications whenever no huddle window is open to display video.
pub struct HuddlePopoutGuard {
    active_popouts: Arc<AtomicUsize>,
}

impl Drop for HuddlePopoutGuard {
    fn drop(&mut self) {
        self.active_popouts.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Separate GPUI entity owning live video/screen frames so 10–30 fps video updates notify ONLY
/// the Pop-out Huddle Window and never repaint the main window's sidebar (`HuddleDock`).
pub struct HuddleVideoStore {
    frames: HashMap<VideoTileKey, DisplayVideoFrame>,
    stale_images: Vec<Arc<RenderImage>>,
    active_popouts: Arc<AtomicUsize>,
    #[cfg(target_os = "macos")]
    pending_cam_surfaces: HashMap<VideoTileKey, Arc<slackless_core::huddle::MacVideoSurface>>,
    #[cfg(target_os = "macos")]
    cam_in_flight: HashSet<VideoTileKey>,
}

impl HuddleVideoStore {
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self {
            frames: HashMap::new(),
            stale_images: Vec::new(),
            active_popouts: Arc::new(AtomicUsize::new(0)),
            #[cfg(target_os = "macos")]
            pending_cam_surfaces: HashMap::new(),
            #[cfg(target_os = "macos")]
            cam_in_flight: HashSet::new(),
        })
    }

    /// Register a visible Pop-out Huddle Window; dropping the returned guard decrements the count.
    pub fn register_popout(&self) -> HuddlePopoutGuard {
        self.active_popouts.fetch_add(1, Ordering::AcqRel);
        HuddlePopoutGuard {
            active_popouts: Arc::clone(&self.active_popouts),
        }
    }

    pub fn update_frame(&mut self, tile: VideoTileKey, frame: &VideoFrame, cx: &mut Context<Self>) {
        let is_screen = matches!(
            tile,
            VideoTileKey::LocalScreen | VideoTileKey::RemoteScreen(_)
        );

        #[cfg(target_os = "macos")]
        if let Some(surface) = frame.mac_surface() {
            let popout_visible = self.active_popouts.load(Ordering::Acquire) > 0;
            if is_screen {
                // Zero-copy Metal GPU surface for 1080p/1200p screen shares (~50ns retain).
                self.insert_display_frame(
                    tile,
                    DisplayVideoFrame::Surface(Arc::clone(surface)),
                    popout_visible,
                    cx,
                );
                return;
            }
            // Camera card over gradient stage: skip GPU conversion completely when no huddle
            // popout window is open! When visible, run hardware `VTPixelTransferSession` scaling
            // to 480x270 on a coalescing background task so the main UI thread never blocks.
            if !popout_visible {
                return;
            }
            self.pending_cam_surfaces
                .insert(tile.clone(), Arc::clone(surface));
            if self.cam_in_flight.insert(tile.clone()) {
                self.spawn_camera_surface_convert(tile, cx);
            }
            return;
        }

        let _ = is_screen;
        if let Some(img) = frame_to_render_image(frame) {
            self.insert_display_frame(tile, DisplayVideoFrame::Image(img), true, cx);
        }
    }

    #[cfg(target_os = "macos")]
    fn spawn_camera_surface_convert(&mut self, tile: VideoTileKey, cx: &mut Context<Self>) {
        let Some(surface) = self.pending_cam_surfaces.remove(&tile) else {
            self.cam_in_flight.remove(&tile);
            return;
        };
        cx.spawn(async move |this, cx| {
            let rendered = cx
                .background_executor()
                .spawn(async move {
                    let (w, h, bgra) = surface.to_bgra_fit(480, 270)?;
                    let rgba = image::RgbaImage::from_raw(w, h, bgra)?;
                    Some(Arc::new(RenderImage::new(smallvec::smallvec![
                        image::Frame::new(rgba)
                    ])))
                })
                .await;
            let _ = this.update(cx, |store: &mut HuddleVideoStore, cx| {
                let popout_visible = store.active_popouts.load(Ordering::Acquire) > 0;
                if let Some(img) = rendered {
                    store.insert_display_frame(
                        tile.clone(),
                        DisplayVideoFrame::Image(img),
                        popout_visible,
                        cx,
                    );
                }
                if popout_visible && store.pending_cam_surfaces.contains_key(&tile) {
                    store.spawn_camera_surface_convert(tile, cx);
                } else {
                    store.pending_cam_surfaces.remove(&tile);
                    store.cam_in_flight.remove(&tile);
                }
            });
        })
        .detach();
    }

    fn insert_display_frame(
        &mut self,
        tile: VideoTileKey,
        next: DisplayVideoFrame,
        notify: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(DisplayVideoFrame::Image(old)) = self.frames.insert(tile, next) {
            self.stale_images.push(old);
            if self.stale_images.len() > 8 {
                for dropped in self.stale_images.drain(..) {
                    cx.drop_image(dropped, None);
                }
            }
        }
        if notify {
            cx.notify();
        }
    }

    pub fn close_tile(&mut self, tile: &VideoTileKey, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            self.pending_cam_surfaces.remove(tile);
        }
        if let Some(old) = self.frames.remove(tile) {
            if let DisplayVideoFrame::Image(img) = old {
                self.stale_images.push(img);
            }
            cx.notify();
        }
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            self.pending_cam_surfaces.clear();
            self.cam_in_flight.clear();
        }
        for (_, old) in self.frames.drain() {
            if let DisplayVideoFrame::Image(img) = old {
                cx.drop_image(img, None);
            }
        }
        for img in self.stale_images.drain(..) {
            cx.drop_image(img, None);
        }
        cx.notify();
    }

    /// Drain evicted `RenderImage`s so the owning `PopoutWindow` can immediately free their
    /// Metal sprite-atlas slots via `cx.drop_image(img, Some(window))`.
    pub fn drain_stale_images(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.stale_images)
    }

    pub fn frames(&self) -> &HashMap<VideoTileKey, DisplayVideoFrame> {
        &self.frames
    }
}

/// Format an elapsed duration in seconds as `MM:SS` or `H:MM:SS`.
pub fn format_elapsed(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Active fading drawing stroke on a shared screen (`PeerMessage.Draw`, `F-COLLAB-025`).
#[derive(Clone, Debug)]
pub struct DrawStrokeEntry {
    pub user: UserId,
    pub item_id: String,
    pub hue: i32,
    pub segments: Vec<HuddleDrawSegment>,
    pub updated_at_ms: u64,
    pub ended: bool,
    pub ended_at_ms: Option<u64>,
}

/// Live colored pointer cursor on a shared screen (`PeerMessage.Draw` `POSITION`).
#[derive(Clone, Debug)]
pub struct DrawCursorEntry {
    pub item_id: String,
    pub hue: i32,
    pub x: f64,
    pub y: f64,
    pub updated_at_ms: u64,
}

/// Ephemeral huddle reaction / effect entry (`PeerMessage.Reacji`).
#[derive(Clone, Debug)]
pub struct ReactionEntry {
    pub user: UserId,
    pub emoji: String,
    pub at_ms: u64,
}

/// Fit a $C^1$-smooth Catmull-Rom cubic Bézier segment from `p1` to `p2` using neighbors `p0`/`p3`,
/// clamping control handles to `0.45 * chord` so sharp bends never self-intersect.
pub fn make_catmull_rom_segment(
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
) -> HuddleDrawSegment {
    let chord_dx = p2.0 - p1.0;
    let chord_dy = p2.1 - p1.1;
    let chord_len = (chord_dx * chord_dx + chord_dy * chord_dy).sqrt();
    if chord_len < 1e-9 {
        return HuddleDrawSegment {
            a: [p1.0, p1.1],
            b: [p1.0, p1.1],
            c: [p2.0, p2.1],
            d: [p2.0, p2.1],
        };
    }
    let max_handle = chord_len * 0.45;
    let clamp_vec = |vx: f64, vy: f64| -> (f64, f64) {
        let len = (vx * vx + vy * vy).sqrt();
        if len > max_handle && len > 1e-9 {
            let s = max_handle / len;
            (vx * s, vy * s)
        } else {
            (vx, vy)
        }
    };
    let (h1x, h1y) = clamp_vec((p2.0 - p0.0) / 6.0, (p2.1 - p0.1) / 6.0);
    let (h2x, h2y) = clamp_vec((p3.0 - p1.0) / 6.0, (p3.1 - p1.1) / 6.0);
    HuddleDrawSegment {
        a: [p1.0, p1.1],
        b: [p1.0 + h1x, p1.1 + h1y],
        c: [p2.0 - h2x, p2.1 - h2y],
        d: [p2.0, p2.1],
    }
}
