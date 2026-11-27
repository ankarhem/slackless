//! GPU video frame lifecycle (`HuddleVideoStore`) and Catmull-Rom drawing / reaction entries
//! shared between the sidebar mini-dock and the pop-out Huddle window.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui_kit::gpui::{App, AppContext as _, Context, Entity, RenderImage};
use slackless_core::huddle::{
    AudioDeviceInfo, HuddleAudioLevels, HuddleDrawSegment, VideoFrame, VideoTileKey,
};
use slackless_core::types::{ChannelId, UserId};

use crate::fixtures::Fixtures;

/// Minimum local microphone RMS energy to highlight the signed-in user as speaking.
pub const LOCAL_SPEAKING_RMS: f32 = 0.03;

/// Minimum remote speaker audio level to highlight a peer as speaking.
pub const PEER_SPEAKING_LEVEL: f32 = 0.05;

/// Whether `user` is currently speaking according to shared local/peer thresholds.
pub fn is_user_speaking(
    is_self: bool,
    muted: bool,
    levels: &HuddleAudioLevels,
    user: &UserId,
) -> bool {
    if is_self {
        !muted && levels.local_rms > LOCAL_SPEAKING_RMS
    } else {
        levels
            .active_speakers
            .iter()
            .any(|(uid, lvl)| uid == user && *lvl > PEER_SPEAKING_LEVEL)
    }
}

/// Canonical Slack web-client huddle join URL, preferring an explicit `rooms.getLink` URL and
/// falling back to `slackless_core::types::net::huddle_fallback_url`.
pub fn huddle_browser_url(
    fixtures: &Fixtures,
    channel: &ChannelId,
    explicit: Option<&str>,
) -> String {
    if let Some(link) = explicit.filter(|l| !l.is_empty()) {
        return link.to_owned();
    }
    let team = fixtures
        .own_team()
        .map(|t| t.id.as_str())
        .unwrap_or_default();
    slackless_core::types::net::huddle_fallback_url(team, channel.as_str())
}

/// Whether `dev` matches the currently selected audio device name (or default when unset).
pub fn is_audio_device_selected(current: Option<&str>, dev: &AudioDeviceInfo) -> bool {
    current == Some(dev.name.as_str()) || (current.is_none() && dev.is_default)
}

/// Compute `(alpha, width_scale, needs_anim_frame)` for a fading huddle draw stroke.
/// Active strokes stay bright at `1.0`; completed strokes hold for 3.8s then smoothly ease out
/// over 1.6s at display refresh rate.
pub fn stroke_fade_curve(ended_at_ms: Option<u64>, now_ms: u64) -> (f32, f32, bool) {
    const HOLD_MS: f32 = 3_800.0;
    const FADE_MS: f32 = 1_600.0;
    let Some(ended_at) = ended_at_ms else {
        return (1.0, 1.0, false);
    };
    let elapsed = now_ms.saturating_sub(ended_at) as f32;
    if elapsed < HOLD_MS {
        // Request animation frames once within the last 100ms of hold so the fade starts smoothly.
        return (1.0, 1.0, elapsed >= HOLD_MS - 100.0);
    }
    if elapsed >= HOLD_MS + FADE_MS {
        return (0.0, 0.72, false);
    }
    let t = ((elapsed - HOLD_MS) / FADE_MS).clamp(0.0, 1.0);
    // Smooth cubic ease-out (`(1 - t)^2` * `(1 + 2t)` complement -> smoothstep).
    let smooth = 1.0 - t * t * (3.0 - 2.0 * t);
    let width_scale = 0.72 + 0.28 * smooth;
    (smooth, width_scale, true)
}

/// Convert a [`VideoFrame`] into a GPU-ready GPUI [`RenderImage`] (uses NEON `vImage` on macOS
/// GPU surfaces, or clones bytes for portable BGRA frames).
pub fn frame_to_render_image(frame: &VideoFrame) -> Option<Arc<RenderImage>> {
    let (w, h, bgra) = frame.to_bgra_fit(0, 0)?;
    bgra_render_image(w, h, bgra)
}

/// Convert an owned [`VideoFrame`] into a [`RenderImage`] without cloning when uniquely owned.
pub fn frame_into_render_image(frame: VideoFrame) -> Option<Arc<RenderImage>> {
    let (w, h, bgra) = frame.into_bgra()?;
    bgra_render_image(w, h, bgra)
}

fn bgra_render_image(width: u32, height: u32, bgra: Vec<u8>) -> Option<Arc<RenderImage>> {
    if width == 0 || height == 0 {
        return None;
    }
    let rgba = image::RgbaImage::from_raw(width, height, bgra)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(rgba)
    ])))
}

/// A frame waiting for its conversion to a [`RenderImage`] on the background executor: a
/// camera's GPU surface, scaled down to its tile, or a portable BGRA frame (a software decode,
/// such as an AV1 screen share) at its own size. Either conversion copies the whole frame.
enum PendingFrame {
    #[cfg(target_os = "macos")]
    CameraSurface(Arc<slackless_core::huddle::MacVideoSurface>),
    Bgra(VideoFrame),
}

impl PendingFrame {
    fn into_render_image(self) -> Option<Arc<RenderImage>> {
        match self {
            #[cfg(target_os = "macos")]
            Self::CameraSurface(surface) => {
                let (w, h, bgra) = surface.to_bgra_fit(480, 270)?;
                bgra_render_image(w, h, bgra)
            }
            Self::Bgra(frame) => frame_into_render_image(frame),
        }
    }
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

/// RAII guard tracking open Pop-out Huddle Windows so [`HuddleVideoStore`] skips frame
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
    /// The newest frame per tile still to convert, and the tiles converting now.
    pending: HashMap<VideoTileKey, PendingFrame>,
    converting: HashSet<VideoTileKey>,
    /// Bumped on [`Self::clear`] so in-flight conversions never resurrect cleared tiles.
    clear_gen: u64,
    /// Bumped per tile on [`Self::close_tile`] so in-flight conversions never resurrect closed tiles.
    tile_gen: HashMap<VideoTileKey, u64>,
}

impl HuddleVideoStore {
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self {
            frames: HashMap::new(),
            stale_images: Vec::new(),
            active_popouts: Arc::new(AtomicUsize::new(0)),
            pending: HashMap::new(),
            converting: HashSet::new(),
            clear_gen: 0,
            tile_gen: HashMap::new(),
        })
    }

    /// Register a visible Pop-out Huddle Window; dropping the returned guard decrements the count.
    pub fn register_popout(&self) -> HuddlePopoutGuard {
        self.active_popouts.fetch_add(1, Ordering::AcqRel);
        HuddlePopoutGuard {
            active_popouts: Arc::clone(&self.active_popouts),
        }
    }

    pub fn active_popout_count(&self) -> usize {
        self.active_popouts.load(Ordering::Acquire)
    }

    pub fn update_frame(&mut self, tile: VideoTileKey, frame: &VideoFrame, cx: &mut Context<Self>) {
        if frame.width == 0 || frame.height == 0 {
            return;
        }
        #[cfg(target_os = "macos")]
        if let Some(surface) = frame.mac_surface() {
            if surface.width() == 0 || surface.height() == 0 {
                return;
            }
            if matches!(
                tile,
                VideoTileKey::LocalScreen | VideoTileKey::RemoteScreen(_)
            ) {
                // Invalidate any in-flight software BGRA conversion for this tile so it cannot
                // later overwrite this newer zero-copy GPU surface.
                let entry = self.tile_gen.entry(tile.clone()).or_insert(0);
                *entry = entry.wrapping_add(1);
                self.pending.remove(&tile);
                // Zero-copy Metal GPU surface for 1080p/1200p screen shares (~50ns retain).
                let popout_visible = self.active_popouts.load(Ordering::Acquire) > 0;
                self.insert_display_frame(
                    tile,
                    DisplayVideoFrame::Surface(Arc::clone(surface)),
                    popout_visible,
                    cx,
                );
            } else {
                // Camera card over gradient stage: hardware `VTPixelTransferSession` scaling to
                // 480x270.
                self.queue_conversion(tile, PendingFrame::CameraSurface(Arc::clone(surface)), cx);
            }
            return;
        }
        self.queue_conversion(tile, PendingFrame::Bgra(frame.clone()), cx);
    }

    /// Convert `frame` for `tile` on a coalescing background task, so the UI thread never
    /// copies a frame: one conversion per tile at a time, the newest frame next. Nothing is
    /// converted while no huddle pop-out window is open to show it.
    fn queue_conversion(
        &mut self,
        tile: VideoTileKey,
        frame: PendingFrame,
        cx: &mut Context<Self>,
    ) {
        if self.active_popouts.load(Ordering::Acquire) == 0 {
            return;
        }
        self.pending.insert(tile.clone(), frame);
        if self.converting.insert(tile.clone()) {
            self.spawn_conversion(tile, cx);
        }
    }

    fn spawn_conversion(&mut self, tile: VideoTileKey, cx: &mut Context<Self>) {
        let Some(frame) = self.pending.remove(&tile) else {
            self.converting.remove(&tile);
            return;
        };
        let clear_gen = self.clear_gen;
        let tile_gen = self.tile_gen.get(&tile).copied().unwrap_or(0);
        cx.spawn(async move |this, cx| {
            let rendered = cx
                .background_executor()
                .spawn(async move { frame.into_render_image() })
                .await;
            let _ = this.update(cx, |store: &mut HuddleVideoStore, cx| {
                let stale = store.clear_gen != clear_gen
                    || store.tile_gen.get(&tile).copied().unwrap_or(0) != tile_gen;
                if stale {
                    if let Some(img) = rendered {
                        cx.drop_image(img, None);
                    }
                    // If `clear_gen` matches, no new clear happened; kick off any newer pending
                    // frame for this tile that arrived after `close_tile`, else release the slot.
                    if store.clear_gen == clear_gen {
                        let popout_visible = store.active_popouts.load(Ordering::Acquire) > 0;
                        if popout_visible && store.pending.contains_key(&tile) {
                            store.spawn_conversion(tile, cx);
                        } else {
                            store.pending.remove(&tile);
                            store.converting.remove(&tile);
                        }
                    }
                    return;
                }
                let popout_visible = store.active_popouts.load(Ordering::Acquire) > 0;
                if let Some(img) = rendered {
                    store.insert_display_frame(
                        tile.clone(),
                        DisplayVideoFrame::Image(img),
                        popout_visible,
                        cx,
                    );
                }
                if popout_visible && store.pending.contains_key(&tile) {
                    store.spawn_conversion(tile, cx);
                } else {
                    store.pending.remove(&tile);
                    store.converting.remove(&tile);
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
        let entry = self.tile_gen.entry(tile.clone()).or_insert(0);
        *entry = entry.wrapping_add(1);
        self.pending.remove(tile);
        if let Some(old) = self.frames.remove(tile) {
            if let DisplayVideoFrame::Image(img) = old {
                self.stale_images.push(img);
            }
            cx.notify();
        }
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.clear_gen = self.clear_gen.wrapping_add(1);
        self.tile_gen.clear();
        self.pending.clear();
        self.converting.clear();
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

#[cfg(test)]
mod tests {
    use super::{
        DisplayVideoFrame, HuddleVideoStore, UserId, VideoFrame, VideoTileKey, format_elapsed,
        make_catmull_rom_segment,
    };
    use gpui_kit::gpui::{self, TestAppContext};

    #[gpui::test]
    fn bgra_frames_convert_off_thread_only_while_a_huddle_window_shows_video(
        cx: &mut TestAppContext,
    ) {
        let store = cx.update(HuddleVideoStore::new);
        let tile = VideoTileKey::RemoteScreen(UserId::from("U_PEER"));
        let frame = VideoFrame::from_bgra(4, 2, vec![0x7f; 4 * 2 * 4]);

        // No huddle window shows video: the frame is dropped without being copied.
        store.update(cx, |store, cx| store.update_frame(tile.clone(), &frame, cx));
        cx.run_until_parked();
        assert!(store.read_with(cx, |store, _| store.frames().is_empty()));

        // With one open, the copy runs on the background executor, not in the call.
        let _popout = store.read_with(cx, |store, _| store.register_popout());
        store.update(cx, |store, cx| store.update_frame(tile.clone(), &frame, cx));
        assert!(store.read_with(cx, |store, _| store.frames().is_empty()));
        cx.run_until_parked();
        let shown = store.read_with(cx, |store, _| {
            store.frames().get(&tile).map(DisplayVideoFrame::dimensions)
        });
        assert_eq!(shown, Some((4, 2)));

        // Closing a tile while a conversion is still in flight must drop the completed image
        // instead of resurrecting a dead tile.
        let frame2 = VideoFrame::from_bgra(8, 4, vec![0x55; 8 * 4 * 4]);
        store.update(cx, |store, cx| {
            store.update_frame(tile.clone(), &frame2, cx);
            store.close_tile(&tile, cx);
        });
        cx.run_until_parked();
        assert!(
            store.read_with(cx, |store, _| !store.frames().contains_key(&tile)),
            "in-flight conversion must not resurrect a closed tile"
        );

        // Clearing the store while a conversion is in flight must also leave frames empty.
        store.update(cx, |store, cx| {
            store.update_frame(tile.clone(), &frame2, cx);
            store.clear(cx);
        });
        cx.run_until_parked();
        assert!(
            store.read_with(cx, |store, _| store.frames().is_empty()),
            "in-flight conversion must not resurrect frames after clear()"
        );
    }

    #[test]
    fn format_elapsed_renders_mm_ss_and_h_mm_ss() {
        assert_eq!(format_elapsed(0), "00:00");
        assert_eq!(format_elapsed(9), "00:09");
        assert_eq!(format_elapsed(59), "00:59");
        assert_eq!(format_elapsed(60), "01:00");
        assert_eq!(format_elapsed(3599), "59:59");
        assert_eq!(format_elapsed(3600), "1:00:00");
        assert_eq!(format_elapsed(3661), "1:01:01");
    }

    #[test]
    fn make_catmull_rom_segment_handles_zero_chord_endpoints_and_sharp_bend_clamping() {
        // Zero chord returns degenerate segment with endpoints intact.
        let deg = make_catmull_rom_segment((0.0, 0.0), (0.5, 0.5), (0.5, 0.5), (1.0, 1.0));
        assert_eq!(deg.a, [0.5, 0.5]);
        assert_eq!(deg.b, [0.5, 0.5]);
        assert_eq!(deg.c, [0.5, 0.5]);
        assert_eq!(deg.d, [0.5, 0.5]);

        // Collinear points preserve endpoints and produce smooth interior handles.
        let seg = make_catmull_rom_segment((0.0, 0.0), (0.25, 0.0), (0.5, 0.0), (0.75, 0.0));
        assert_eq!(seg.a, [0.25, 0.0]);
        assert_eq!(seg.d, [0.5, 0.0]);
        assert!((seg.b[0] - (0.25 + 0.5 / 6.0)).abs() < 1e-9);
        assert!((seg.c[0] - (0.5 - 0.5 / 6.0)).abs() < 1e-9);

        // Extreme outlier neighbors clamp control handles to <= 0.45 * chord_len.
        let p1 = (0.4, 0.4);
        let p2 = (0.5, 0.4); // chord_len = 0.1, max_handle = 0.045
        let sharp = make_catmull_rom_segment((-10.0, 10.0), p1, p2, (10.0, -10.0));
        let h1 = ((sharp.b[0] - sharp.a[0]).powi(2) + (sharp.b[1] - sharp.a[1]).powi(2)).sqrt();
        let h2 = ((sharp.d[0] - sharp.c[0]).powi(2) + (sharp.d[1] - sharp.c[1]).powi(2)).sqrt();
        assert!((h1 - 0.045).abs() < 1e-9, "h1 clamped to 0.45*chord: {h1}");
        assert!((h2 - 0.045).abs() < 1e-9, "h2 clamped to 0.45*chord: {h2}");
    }
}
