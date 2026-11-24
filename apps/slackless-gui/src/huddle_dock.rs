//! Persistent sidebar mini-dock for an active native Slack Huddle (`F-COLLAB-001..059`).
//!
//! Pinned at the bottom of the sidebar column so the user can switch channels, threads, DMs, or
//! Unreads without ever losing audio controls, active-speaker visibility, or one-click pop-out.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, RenderImage, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};
use slackless_core::huddle::{
    AudioDevices, HuddleAudioLevels, HuddleDataEvent, HuddleDrawMessage, HuddleDrawSegment,
    HuddlePhase, HuddleReactionKind, HuddleSessionSnapshot, VideoFrame, VideoTileKey,
    list_audio_devices,
};
use slackless_core::types::{ChannelId, ConversationKind, UserId};

use crate::avatars::user_chip;
use crate::fixtures::Fixtures;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

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

/// Snapshot consumed by `PopoutKind::Huddle` so the Pop-out Huddle Window stays in lock-step with
/// the sidebar mini-dock. Live video frames live in a separate [`HuddleVideoStore`] entity so
/// 10–30 fps video updates never repaint the main window.
#[derive(Clone)]
pub struct HuddleViewState {
    pub session: HuddleSessionSnapshot,
    pub participants: Vec<UserId>,
    pub levels: HuddleAudioLevels,
    pub elapsed_secs: u64,
    pub raised_hands: HashSet<UserId>,
    pub sticky_emojis: HashMap<UserId, String>,
    pub reactions: Vec<ReactionEntry>,
    pub active_effect: Option<(UserId, String, u64)>,
    pub draw_strokes: Vec<DrawStrokeEntry>,
    pub draw_cursors: HashMap<UserId, DrawCursorEntry>,
    pub captions: Vec<(UserId, String, bool)>,
}

pub enum HuddleDockEvent {
    ToggleMute,
    ToggleCamera,
    ToggleScreenShare,
    Leave,
    PopOut,
    JumpToChannel(ChannelId),
    OpenThread,
    OpenCanvas(String),
    CopyLink(ChannelId),
    OpenBrowser(String),
    RetryJoin(ChannelId),
    OpenAudioSettings,
    SwitchInputDevice(Option<String>),
    SwitchOutputDevice(Option<String>),
}

pub struct HuddleDock {
    fixtures: Arc<Fixtures>,
    session: HuddleSessionSnapshot,
    participants: Vec<UserId>,
    levels: HuddleAudioLevels,
    elapsed_secs: u64,
    video_store: Entity<HuddleVideoStore>,
    raised_hands: HashSet<UserId>,
    sticky_emojis: HashMap<UserId, String>,
    reactions: Vec<ReactionEntry>,
    active_effect: Option<(UserId, String, u64)>,
    draw_strokes: Vec<DrawStrokeEntry>,
    draw_cursors: HashMap<UserId, DrawCursorEntry>,
    user_hues: HashMap<UserId, i32>,
    captions: Vec<(UserId, String, bool)>,
    menu_open: bool,
    devices: AudioDevices,
    #[cfg(target_os = "macos")]
    overlay_visible: bool,
    #[cfg(target_os = "macos")]
    overlay_fade_task: Option<gpui_kit::gpui::Task<()>>,
}

#[cfg(target_os = "macos")]
impl Drop for HuddleDock {
    fn drop(&mut self) {
        if self.overlay_visible {
            slackless_core::huddle::hide_desktop_draw_overlay();
            self.overlay_visible = false;
        }
    }
}

/// Fade curve shared with the pop-out stage: active strokes stay opaque; completed strokes hold
/// for 3.8s and ease out smoothly over 1.6s.
#[cfg(target_os = "macos")]
fn overlay_stroke_fade(ended_at_ms: Option<u64>, now_ms: u64) -> (f32, f32) {
    const HOLD_MS: f32 = 3_800.0;
    const FADE_MS: f32 = 1_600.0;
    let Some(ended_at) = ended_at_ms else {
        return (1.0, 1.0);
    };
    let elapsed = now_ms.saturating_sub(ended_at) as f32;
    if elapsed < HOLD_MS {
        return (1.0, 1.0);
    }
    if elapsed >= HOLD_MS + FADE_MS {
        return (0.0, 0.72);
    }
    let t = ((elapsed - HOLD_MS) / FADE_MS).clamp(0.0, 1.0);
    let smooth = 1.0 - t * t * (3.0 - 2.0 * t);
    (smooth, 0.72 + 0.28 * smooth)
}

impl EventEmitter<HuddleDockEvent> for HuddleDock {}

impl HuddleDock {
    pub fn new(
        fixtures: Arc<Fixtures>,
        session: HuddleSessionSnapshot,
        participants: Vec<UserId>,
        cx: &mut App,
    ) -> Entity<Self> {
        let video_store = HuddleVideoStore::new(cx);
        cx.new(|cx| {
            // 1-second timer to advance the elapsed duration clock & expire floating reactions.
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let keep = this
                        .update(cx, |dock: &mut HuddleDock, cx| {
                            let mut dirty = false;
                            if dock.session.started_at_secs > 0
                                && matches!(
                                    dock.session.phase,
                                    HuddlePhase::Connected | HuddlePhase::Reconnecting
                                )
                            {
                                dock.elapsed_secs =
                                    now_secs().saturating_sub(dock.session.started_at_secs);
                                dirty = true;
                            }
                            let now = now_ms();
                            let react_cutoff = now.saturating_sub(4_500);
                            let before = dock.reactions.len();
                            dock.reactions.retain(|r| r.at_ms >= react_cutoff);
                            if dock.reactions.len() != before {
                                dirty = true;
                            }
                            if dock
                                .active_effect
                                .as_ref()
                                .is_some_and(|(_, _, at)| *at < now.saturating_sub(3_500))
                            {
                                dock.active_effect = None;
                                dirty = true;
                            }
                            let ended_cutoff = now.saturating_sub(5_800);
                            let stale_cutoff = now.saturating_sub(15_000);
                            let strokes_before = dock.draw_strokes.len();
                            dock.draw_strokes.retain(|s| {
                                if let Some(ended_at) = s.ended_at_ms {
                                    ended_at >= ended_cutoff
                                } else {
                                    s.updated_at_ms >= stale_cutoff
                                }
                            });
                            let cursor_cutoff = now.saturating_sub(3_000);
                            let cursors_before = dock.draw_cursors.len();
                            dock.draw_cursors
                                .retain(|_, c| c.updated_at_ms >= cursor_cutoff);
                            if dock.draw_strokes.len() != strokes_before
                                || dock.draw_cursors.len() != cursors_before
                            {
                                dirty = true;
                                dock.sync_desktop_draw_overlay(cx);
                            }
                            if dirty {
                                cx.notify();
                            }
                        })
                        .is_ok();
                    if !keep {
                        break;
                    }
                }
            })
            .detach();

            let started = session.started_at_secs;
            Self {
                fixtures,
                session,
                participants,
                levels: HuddleAudioLevels::default(),
                elapsed_secs: if started > 0 {
                    now_secs().saturating_sub(started)
                } else {
                    0
                },
                video_store,
                raised_hands: HashSet::new(),
                sticky_emojis: HashMap::new(),
                reactions: Vec::new(),
                active_effect: None,
                draw_strokes: Vec::new(),
                draw_cursors: HashMap::new(),
                user_hues: HashMap::new(),
                captions: Vec::new(),
                menu_open: false,
                devices: AudioDevices::default(),
                #[cfg(target_os = "macos")]
                overlay_visible: false,
                #[cfg(target_os = "macos")]
                overlay_fade_task: None,
            }
        })
    }

    #[cfg(target_os = "macos")]
    fn sync_desktop_draw_overlay(&mut self, cx: &mut Context<Self>) {
        let Some(source_id) = self
            .session
            .sharing
            .then_some(self.session.share_source_id.as_deref())
            .flatten()
            .filter(|s| !s.is_empty())
        else {
            self.overlay_fade_task = None;
            if self.overlay_visible {
                slackless_core::huddle::hide_desktop_draw_overlay();
                self.overlay_visible = false;
            }
            return;
        };

        let now = now_ms();
        let mut segs = Vec::new();
        for stroke in &self.draw_strokes {
            let (alpha, width_scale) = overlay_stroke_fade(stroke.ended_at_ms, now);
            if alpha <= 0.01 {
                continue;
            }
            let width = 5.0 * width_scale;
            for seg in &stroke.segments {
                segs.push(slackless_core::huddle::OverlaySegmentFfi {
                    ax: seg.a[0],
                    ay: seg.a[1],
                    bx: seg.b[0],
                    by: seg.b[1],
                    cx: seg.c[0],
                    cy: seg.c[1],
                    dx: seg.d[0],
                    dy: seg.d[1],
                    hue: stroke.hue,
                    alpha,
                    width,
                });
            }
        }

        let mut cursors = Vec::new();
        for cur in self.draw_cursors.values() {
            let age = now.saturating_sub(cur.updated_at_ms);
            if age >= 3_000 {
                continue;
            }
            let alpha = if age > 2_400 {
                ((3_000 - age) as f32 / 600.0).clamp(0.0, 1.0)
            } else {
                1.0
            };
            cursors.push(slackless_core::huddle::OverlayCursorFfi {
                x: cur.x,
                y: cur.y,
                hue: cur.hue,
                alpha,
            });
        }

        if segs.is_empty() && cursors.is_empty() {
            self.overlay_fade_task = None;
            if self.overlay_visible {
                slackless_core::huddle::hide_desktop_draw_overlay();
                self.overlay_visible = false;
            }
            return;
        }

        slackless_core::huddle::update_desktop_draw_overlay(source_id, &segs, &cursors);
        self.overlay_visible = true;

        if self.overlay_fade_task.is_none() {
            self.overlay_fade_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(33))
                        .await;
                    let keep = this
                        .update(cx, |dock: &mut HuddleDock, cx| {
                            dock.sync_desktop_draw_overlay(cx);
                            dock.overlay_visible
                        })
                        .unwrap_or(false);
                    if !keep {
                        break;
                    }
                }
            }));
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn sync_desktop_draw_overlay(&mut self, _cx: &mut Context<Self>) {}

    pub fn update_session(&mut self, session: HuddleSessionSnapshot, cx: &mut Context<Self>) {
        if session.started_at_secs > 0 {
            self.elapsed_secs = now_secs().saturating_sub(session.started_at_secs);
        }
        if session.hand_raised {
            self.raised_hands.insert(self.fixtures.self_user.clone());
        } else {
            self.raised_hands.remove(&self.fixtures.self_user);
        }
        let share_changed = self.session.sharing != session.sharing
            || self.session.share_source_id != session.share_source_id;
        self.session = session;
        if share_changed {
            self.sync_desktop_draw_overlay(cx);
        }
        cx.notify();
    }

    pub fn set_participants(&mut self, participants: Vec<UserId>, cx: &mut Context<Self>) {
        if self.participants != participants {
            self.participants = participants;
            cx.notify();
        }
    }

    pub fn update_levels(&mut self, levels: HuddleAudioLevels, cx: &mut Context<Self>) {
        if (self.levels.local_rms - levels.local_rms).abs() < 0.015
            && self.levels.active_speakers == levels.active_speakers
            && self.levels.muted_peers == levels.muted_peers
        {
            return;
        }
        self.levels = levels;
        cx.notify();
    }

    /// Separate video-frame store observed exclusively by the Pop-out Huddle Window.
    pub fn video_store(&self) -> Entity<HuddleVideoStore> {
        self.video_store.clone()
    }

    pub fn update_video_frame(
        &mut self,
        tile: VideoTileKey,
        frame: &VideoFrame,
        cx: &mut Context<Self>,
    ) {
        // Update ONLY `video_store` — never call `cx.notify()` on `HuddleDock` so the main
        // window doesn't repaint 10–30×/sec for video frames it never renders!
        self.video_store.update(cx, |vs, cx| {
            vs.update_frame(tile, frame, cx);
        });
    }

    pub fn close_video_tile(&mut self, tile: &VideoTileKey, cx: &mut Context<Self>) {
        self.video_store.update(cx, |vs, cx| {
            vs.close_tile(tile, cx);
        });
    }

    pub fn clear_video_frames(&mut self, cx: &mut Context<Self>) {
        self.video_store.update(cx, |vs, cx| {
            vs.clear(cx);
        });
        #[cfg(target_os = "macos")]
        {
            self.overlay_fade_task = None;
            if self.overlay_visible {
                slackless_core::huddle::hide_desktop_draw_overlay();
                self.overlay_visible = false;
            }
        }
    }

    pub fn apply_room_meta(
        &mut self,
        topic: Option<String>,
        background_id: Option<String>,
        thread_root_ts: Option<slackless_core::types::Ts>,
        cx: &mut Context<Self>,
    ) {
        let mut dirty = false;
        if let Some(t) = topic {
            let next = (!t.trim().is_empty()).then_some(t);
            if self.session.topic != next {
                self.session.topic = next;
                dirty = true;
            }
        }
        if let Some(bg) = background_id {
            let next = (!bg.trim().is_empty()).then_some(bg);
            if self.session.background_id != next {
                self.session.background_id = next;
                dirty = true;
            }
        }
        if let Some(root_ts) = thread_root_ts.filter(|ts| !ts.as_str().trim().is_empty()) {
            if self.session.thread_root_ts.as_ref() != Some(&root_ts) {
                self.session.thread_root_ts = Some(root_ts);
                dirty = true;
            }
        }
        if dirty {
            cx.notify();
        }
    }

    pub fn set_background_local(&mut self, bg: String, cx: &mut Context<Self>) {
        self.session.background_id = (!bg.trim().is_empty()).then_some(bg);
        cx.notify();
    }

    pub fn set_jukebox_local(&mut self, song: Option<String>, cx: &mut Context<Self>) {
        self.session.jukebox_song = song;
        cx.notify();
    }

    pub fn apply_data_event(&mut self, event: &HuddleDataEvent, cx: &mut Context<Self>) {
        match event {
            HuddleDataEvent::Reaction {
                user,
                emoji,
                kind,
                at_ms,
                ..
            } => {
                let ts = if *at_ms == 0 { now_ms() } else { *at_ms };
                if *kind == HuddleReactionKind::Effect {
                    self.active_effect = Some((user.clone(), emoji.clone(), ts));
                }
                self.reactions.push(ReactionEntry {
                    user: user.clone(),
                    emoji: emoji.clone(),
                    at_ms: ts,
                });
                if self.reactions.len() > 24 {
                    let excess = self.reactions.len() - 24;
                    self.reactions.drain(0..excess);
                }
            }
            HuddleDataEvent::HandRaise { user, raised } => {
                if *raised {
                    self.raised_hands.insert(user.clone());
                } else {
                    self.raised_hands.remove(user);
                }
                if *user == self.fixtures.self_user {
                    self.session.hand_raised = *raised;
                }
            }
            HuddleDataEvent::StickyEmoji { user, emoji } => {
                match emoji {
                    Some(em) if !em.is_empty() => {
                        if em == "raised_hand" || em == "✋" {
                            self.raised_hands.insert(user.clone());
                        }
                        self.sticky_emojis.insert(user.clone(), em.clone());
                    }
                    _ => {
                        self.sticky_emojis.remove(user);
                        self.raised_hands.remove(user);
                    }
                }
                if *user == self.fixtures.self_user {
                    self.session.sticky_emoji = emoji.clone();
                    self.session.hand_raised =
                        matches!(emoji.as_deref(), Some("raised_hand") | Some("✋"));
                }
            }
            HuddleDataEvent::Topic { topic, .. } => {
                self.session.topic = (!topic.is_empty()).then(|| topic.clone());
            }
            HuddleDataEvent::Caption {
                user,
                text,
                is_partial,
                ..
            } => {
                if !text.trim().is_empty() {
                    if let Some(last) = self.captions.last_mut() {
                        if last.0 == *user && last.2 {
                            last.1 = text.clone();
                            last.2 = *is_partial;
                            cx.notify();
                            return;
                        }
                    }
                    self.captions
                        .push((user.clone(), text.clone(), *is_partial));
                    if self.captions.len() > 4 {
                        let excess = self.captions.len() - 4;
                        self.captions.drain(0..excess);
                    }
                }
            }
            HuddleDataEvent::Draw { user, message, .. } => {
                let ts = now_ms();
                match message {
                    HuddleDrawMessage::ChangeHue { hue, .. } => {
                        self.user_hues.insert(user.clone(), *hue);
                    }
                    HuddleDrawMessage::New { item_id, segment } => {
                        let hue = *self.user_hues.get(user).unwrap_or(&210);
                        // Append to this user's open (un-ended) stroke on the same item, unless
                        // it went stale (>10s) without an `End` packet.
                        if let Some(open) = self.draw_strokes.iter_mut().rev().find(|s| {
                            !s.ended
                                && s.user == *user
                                && s.item_id == *item_id
                                && s.hue == hue
                                && ts.saturating_sub(s.updated_at_ms) < 10_000
                        }) {
                            open.segments.push(*segment);
                            open.updated_at_ms = ts;
                        } else {
                            self.draw_strokes.push(DrawStrokeEntry {
                                user: user.clone(),
                                item_id: item_id.clone(),
                                hue,
                                segments: vec![*segment],
                                updated_at_ms: ts,
                                ended: false,
                                ended_at_ms: None,
                            });
                            if self.draw_strokes.len() > 64 {
                                let excess = self.draw_strokes.len() - 64;
                                self.draw_strokes.drain(0..excess);
                            }
                        }
                    }
                    HuddleDrawMessage::Edit { item_id, segment } => {
                        if let Some(stroke) = self
                            .draw_strokes
                            .iter_mut()
                            .rev()
                            .find(|s| !s.ended && s.user == *user && s.item_id == *item_id)
                        {
                            // `Edit` refines the most recent cubic segment in-place.
                            if let Some(last) = stroke.segments.last_mut() {
                                *last = *segment;
                            } else {
                                stroke.segments.push(*segment);
                            }
                            stroke.updated_at_ms = ts;
                        }
                    }
                    HuddleDrawMessage::End { item_id } => {
                        if let Some(stroke) = self
                            .draw_strokes
                            .iter_mut()
                            .rev()
                            .find(|s| !s.ended && s.user == *user && s.item_id == *item_id)
                        {
                            stroke.ended = true;
                            stroke.ended_at_ms = Some(ts);
                            stroke.updated_at_ms = ts;
                        }
                    }
                    HuddleDrawMessage::Position { item_id, x, y } => {
                        let hue = *self.user_hues.get(user).unwrap_or(&210);
                        self.draw_cursors.insert(
                            user.clone(),
                            DrawCursorEntry {
                                item_id: item_id.clone(),
                                hue,
                                x: *x,
                                y: *y,
                                updated_at_ms: ts,
                            },
                        );
                    }
                }
                self.sync_desktop_draw_overlay(cx);
            }
            HuddleDataEvent::Jukebox { song_key, .. } => {
                self.session.jukebox_song = song_key.clone();
            }
            HuddleDataEvent::MutedByPeer { .. } => {
                self.session.muted = true;
            }
        }
        cx.notify();
    }

    pub fn set_muted_local(&mut self, muted: bool, cx: &mut Context<Self>) {
        if self.session.muted != muted {
            self.session.muted = muted;
            cx.notify();
        }
    }

    pub fn set_camera_local(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.session.video != on {
            self.session.video = on;
            if !on {
                self.close_video_tile(&VideoTileKey::LocalCamera, cx);
            }
            cx.notify();
        }
    }

    pub fn set_blur_local(&mut self, blur: bool, cx: &mut Context<Self>) {
        if self.session.blur_background != blur {
            self.session.blur_background = blur;
            cx.notify();
        }
    }

    pub fn set_sharing_local(&mut self, source_id: Option<String>, cx: &mut Context<Self>) {
        self.session.sharing = source_id.is_some();
        self.session.share_source_id = source_id;
        if !self.session.sharing {
            self.close_video_tile(&VideoTileKey::LocalScreen, cx);
        }
        self.sync_desktop_draw_overlay(cx);
        cx.notify();
    }

    /// Ask the main window to open Preferences directly on the Audio & video section.
    pub fn open_audio_settings(&mut self, cx: &mut Context<Self>) {
        cx.emit(HuddleDockEvent::OpenAudioSettings);
    }

    /// Commit a completed local drawing stroke so it smoothly fades out alongside remote strokes.
    pub fn commit_local_draw_stroke(
        &mut self,
        item_id: String,
        hue: i32,
        segments: Vec<HuddleDrawSegment>,
        cx: &mut Context<Self>,
    ) {
        if segments.is_empty() {
            return;
        }
        let ts = now_ms();
        self.draw_strokes.push(DrawStrokeEntry {
            user: self.fixtures.self_user.clone(),
            item_id,
            hue,
            segments,
            updated_at_ms: ts,
            ended: true,
            ended_at_ms: Some(ts),
        });
        if self.draw_strokes.len() > 64 {
            let excess = self.draw_strokes.len() - 64;
            self.draw_strokes.drain(0..excess);
        }
        self.sync_desktop_draw_overlay(cx);
        cx.notify();
    }

    pub fn set_captions_local(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.session.captions_enabled != enabled {
            self.session.captions_enabled = enabled;
            cx.notify();
        }
    }

    pub fn set_audio_only_local(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.session.audio_only != enabled {
            self.session.audio_only = enabled;
            if enabled {
                self.session.video = false;
                self.video_store.update(cx, |vs, cx| vs.clear(cx));
            }
            cx.notify();
        }
    }

    pub fn session(&self) -> &HuddleSessionSnapshot {
        &self.session
    }

    pub fn view_state(&self) -> HuddleViewState {
        HuddleViewState {
            session: self.session.clone(),
            participants: self.participants.clone(),
            levels: self.levels.clone(),
            elapsed_secs: self.elapsed_secs,
            raised_hands: self.raised_hands.clone(),
            sticky_emojis: self.sticky_emojis.clone(),
            reactions: self.reactions.clone(),
            active_effect: self.active_effect.clone(),
            draw_strokes: self.draw_strokes.clone(),
            draw_cursors: self.draw_cursors.clone(),
            captions: self.captions.clone(),
        }
    }

    fn channel_label(&self) -> String {
        self.fixtures
            .conversation(&self.session.channel)
            .map(|c| self.fixtures.conversation_label(c))
            .unwrap_or_else(|| self.session.channel.to_string())
    }

    fn browser_url(&self) -> String {
        if let Some(link) = &self.session.huddle_link {
            return link.clone();
        }
        let team = self
            .fixtures
            .workspaces
            .first()
            .map(|t| t.id.to_string())
            .unwrap_or_default();
        format!(
            "https://app.slack.com/huddle/{team}/{}",
            self.session.channel
        )
    }

    fn all_people(&self) -> Vec<UserId> {
        let mut people = self.participants.clone();
        if !people.contains(&self.fixtures.self_user) {
            people.push(self.fixtures.self_user.clone());
        }
        people
    }

    fn is_speaking(&self, user: &UserId) -> bool {
        if *user == self.fixtures.self_user {
            !self.session.muted && self.levels.local_rms > 0.03
        } else {
            self.levels
                .active_speakers
                .iter()
                .any(|(uid, lvl)| uid == user && *lvl > 0.05)
        }
    }

    fn is_muted_user(&self, user: &UserId) -> bool {
        if *user == self.fixtures.self_user {
            self.session.muted
        } else {
            self.levels.muted_peers.contains(user)
        }
    }

    fn render_menu(
        &self,
        channel: &ChannelId,
        browser_url: String,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let theme = cx.theme();
        let (card_bg, border, muted_fg, fg, hover_bg) = (
            theme.popover,
            theme.border,
            theme.muted_foreground,
            theme.popover_foreground,
            theme.accent,
        );
        let copy_ch = channel.clone();
        let canvas_id = self.session.canvas_file_id.clone();
        let active_in = self.session.input_device.clone();
        let active_out = self.session.output_device.clone();

        v_flex()
            .w_full()
            .p_2()
            .gap_1()
            .rounded_xl()
            .bg(card_bg)
            .border_1()
            .border_color(border)
            .shadow_xl()
            .text_xs()
            .text_color(fg)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted_fg)
                    .child("Microphone"),
            )
            .children(self.devices.inputs.iter().enumerate().map(|(ix, dev)| {
                let selected = active_in.as_deref() == Some(dev.name.as_str())
                    || (active_in.is_none() && dev.is_default);
                let dev_name = dev.name.clone();
                div()
                    .id(("dock-in-dev", ix))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .truncate()
                    .hover(move |el| el.bg(hover_bg))
                    .when(selected, |el| el.font_weight(FontWeight::SEMIBOLD))
                    .child(if selected {
                        format!("✓ {}", dev.name)
                    } else {
                        dev.name.clone()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::SwitchInputDevice(Some(dev_name.clone())));
                        cx.notify();
                    }))
            }))
            .child(
                div()
                    .mt_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted_fg)
                    .child("Speaker"),
            )
            .children(self.devices.outputs.iter().enumerate().map(|(ix, dev)| {
                let selected = active_out.as_deref() == Some(dev.name.as_str())
                    || (active_out.is_none() && dev.is_default);
                let dev_name = dev.name.clone();
                div()
                    .id(("dock-out-dev", ix))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .truncate()
                    .hover(move |el| el.bg(hover_bg))
                    .when(selected, |el| el.font_weight(FontWeight::SEMIBOLD))
                    .child(if selected {
                        format!("✓ {}", dev.name)
                    } else {
                        dev.name.clone()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::SwitchOutputDevice(Some(dev_name.clone())));
                        cx.notify();
                    }))
            }))
            .child(div().h(px(1.)).my_1().bg(border))
            .child(
                h_flex()
                    .id("dock-open-thread")
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover_bg))
                    .child(
                        Icon::new(IconName::MessageSquare)
                            .size(px(12.))
                            .text_color(muted_fg),
                    )
                    .child("Open huddle thread")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::OpenThread);
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .id("dock-copy-link")
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover_bg))
                    .child(Icon::new(IconName::Link).size(px(12.)).text_color(muted_fg))
                    .child("Copy huddle link")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::CopyLink(copy_ch.clone()));
                        cx.notify();
                    })),
            )
            .when_some(canvas_id, |m, fid| {
                m.child(
                    h_flex()
                        .id("dock-open-canvas")
                        .px_2()
                        .py_1()
                        .gap_2()
                        .items_center()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(move |el| el.bg(hover_bg))
                        .child(
                            Icon::new(IconName::FileText)
                                .size(px(12.))
                                .text_color(muted_fg),
                        )
                        .child("Open huddle notes")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu_open = false;
                            cx.emit(HuddleDockEvent::OpenCanvas(fid.clone()));
                            cx.notify();
                        })),
                )
            })
            .child(
                h_flex()
                    .id("dock-audio-settings")
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover_bg))
                    .child(
                        Icon::new(IconName::Settings)
                            .size(px(12.))
                            .text_color(muted_fg),
                    )
                    .child("Audio & video preferences…")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::OpenAudioSettings);
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .id("dock-open-browser")
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover_bg))
                    .child(
                        Icon::new(IconName::ExternalLink)
                            .size(px(12.))
                            .text_color(muted_fg),
                    )
                    .child("Open in browser")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::OpenBrowser(browser_url.clone()));
                        cx.notify();
                    })),
            )
    }

    fn render_action_bar(&self, pal: &DockPalette, cx: &mut Context<Self>) -> gpui_kit::gpui::Div {
        let btn_bg = pal.btn_bg;
        let btn_hover = pal.btn_hover;
        let btn_fg = pal.card_fg;
        let active_bg = pal.active_btn_bg;
        let active_fg = pal.active_btn_fg;
        let danger = pal.danger;
        let danger_hover = pal.danger_hover;
        let danger_fg = pal.danger_fg;
        let primary = pal.primary;
        let primary_hover = pal.primary_hover;
        let primary_fg = pal.primary_fg;

        let mic_btn = div()
            .id("huddle-dock-mic")
            .size(px(32.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if self.session.muted {
                danger
            } else {
                active_bg
            })
            .hover(move |el| {
                el.bg(if self.session.muted {
                    danger_hover
                } else {
                    btn_hover
                })
            })
            .child(
                Icon::new(if self.session.muted {
                    IconName::MicOff
                } else {
                    IconName::Mic
                })
                .size(px(15.))
                .text_color(if self.session.muted {
                    danger_fg
                } else {
                    active_fg
                }),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleDockEvent::ToggleMute)));

        let video_btn = div()
            .id("huddle-dock-video")
            .size(px(32.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if self.session.video {
                active_bg
            } else {
                btn_bg
            })
            .hover(move |el| el.bg(btn_hover))
            .child(
                Icon::new(if self.session.video {
                    IconName::Video
                } else {
                    IconName::VideoOff
                })
                .size(px(15.))
                .text_color(if self.session.video {
                    active_fg
                } else {
                    btn_fg
                }),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleDockEvent::ToggleCamera)));

        let share_btn = div()
            .id("huddle-dock-share")
            .size(px(32.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if self.session.sharing {
                primary
            } else {
                btn_bg
            })
            .hover(move |el| {
                el.bg(if self.session.sharing {
                    primary_hover
                } else {
                    btn_hover
                })
            })
            .child(Icon::new(IconName::ScreenShare).size(px(15.)).text_color(
                if self.session.sharing {
                    primary_fg
                } else {
                    btn_fg
                },
            ))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleDockEvent::ToggleScreenShare)));

        let more_btn = div()
            .id("huddle-dock-more")
            .size(px(32.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if self.menu_open { btn_hover } else { btn_bg })
            .hover(move |el| el.bg(btn_hover))
            .child(
                Icon::new(IconName::Ellipsis)
                    .size(px(15.))
                    .text_color(btn_fg),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.menu_open = !this.menu_open;
                if this.menu_open {
                    cx.spawn(async move |dock, cx| {
                        let devices = cx
                            .background_executor()
                            .spawn(async move { list_audio_devices() })
                            .await;
                        let _ = dock.update(cx, |dock: &mut HuddleDock, cx| {
                            dock.devices = devices;
                            cx.notify();
                        });
                    })
                    .detach();
                }
                cx.notify();
            }));

        let popout_btn = div()
            .id("huddle-dock-popout")
            .size(px(32.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(btn_bg)
            .hover(move |el| el.bg(btn_hover))
            .child(
                Icon::new(IconName::ExternalLink)
                    .size(px(15.))
                    .text_color(btn_fg),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleDockEvent::PopOut)));

        let leave_btn = h_flex()
            .id("huddle-dock-leave")
            .h(px(32.))
            .px_4()
            .rounded_lg()
            .cursor_pointer()
            .items_center()
            .justify_center()
            .bg(danger)
            .hover(move |el| el.bg(danger_hover))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(danger_fg)
            .child("Leave")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleDockEvent::Leave)));

        h_flex()
            .flex_none()
            .items_center()
            .gap_1p5()
            .child(mic_btn)
            .child(video_btn)
            .child(share_btn)
            .child(more_btn)
            .child(popout_btn)
            .child(leave_btn)
    }

    fn channel_heading(&self) -> (Option<IconName>, String) {
        let Some(conv) = self.fixtures.conversation(&self.session.channel) else {
            return (None, self.channel_label());
        };
        let label = self.fixtures.conversation_label(conv);
        match conv.kind() {
            ConversationKind::PrivateChannel => (
                Some(IconName::Lock),
                label.trim_start_matches("# ").to_owned(),
            ),
            ConversationKind::PublicChannel => {
                (None, format!("#{}", label.trim_start_matches("# ")))
            }
            ConversationKind::Im | ConversationKind::Mpim => (None, label),
        }
    }
}

struct DockPalette {
    card_bg: Hsla,
    card_fg: Hsla,
    muted_fg: Hsla,
    border: Hsla,
    btn_bg: Hsla,
    btn_hover: Hsla,
    active_btn_bg: Hsla,
    active_btn_fg: Hsla,
    primary: Hsla,
    primary_hover: Hsla,
    primary_fg: Hsla,
    danger: Hsla,
    danger_hover: Hsla,
    danger_fg: Hsla,
    success: Hsla,
    warning: Hsla,
}

fn dock_palette(cx: &App) -> DockPalette {
    let theme = cx.theme();
    DockPalette {
        card_bg: theme.background,
        card_fg: theme.foreground,
        muted_fg: theme.muted_foreground,
        border: theme.border,
        btn_bg: theme.secondary,
        btn_hover: theme.secondary_hover,
        active_btn_bg: theme.foreground,
        active_btn_fg: theme.background,
        primary: theme.primary,
        primary_hover: theme.primary_hover,
        primary_fg: theme.primary_foreground,
        danger: theme.danger,
        danger_hover: theme.danger_hover,
        danger_fg: theme.danger_foreground,
        success: theme.success,
        warning: theme.warning,
    }
}

impl Render for HuddleDock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = dock_palette(cx);

        let (status_color, status_text) = match self.session.phase {
            HuddlePhase::Joining | HuddlePhase::Connecting => {
                (pal.warning, "Connecting…".to_owned())
            }
            HuddlePhase::Connected => (pal.success, format_elapsed(self.elapsed_secs)),
            HuddlePhase::Reconnecting => (pal.warning, "Reconnecting…".to_owned()),
            HuddlePhase::Leaving => (pal.muted_fg, "Leaving…".to_owned()),
            HuddlePhase::Failed => (pal.danger, "Failed".to_owned()),
            HuddlePhase::Idle => (pal.muted_fg, "Ended".to_owned()),
        };

        let channel = self.session.channel.clone();
        let (lock_icon, channel_name) = self.channel_heading();
        let browser_url = self.browser_url();
        let people = self.all_people();
        let visible_people: Vec<UserId> = people.iter().take(4).cloned().collect();
        let overflow_count = people.len().saturating_sub(visible_people.len());
        let jump_ch = channel.clone();
        let action_bar = self.render_action_bar(&pal, cx);

        let mut bar = div().id("huddle-sidebar-dock").relative().w_full().child(
            h_flex()
                .w_full()
                .h(px(46.))
                .px_3p5()
                .gap_3()
                .items_center()
                .justify_between()
                .rounded_lg()
                .bg(pal.card_bg)
                .border_1()
                .border_color(pal.border)
                .shadow_sm()
                .text_color(pal.card_fg)
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap_2p5()
                        .child(
                            h_flex()
                                .id("huddle-dock-jump")
                                .items_center()
                                .gap_1p5()
                                .min_w_0()
                                .cursor_pointer()
                                .child(Icon::new(IconName::Headphones).size(px(16.)).text_color(
                                    if self.session.phase == HuddlePhase::Connected {
                                        pal.card_fg
                                    } else {
                                        status_color
                                    },
                                ))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(pal.card_fg)
                                        .child("Huddle in"),
                                )
                                .when_some(lock_icon, |row, ic| {
                                    row.child(Icon::new(ic).size(px(13.)).text_color(pal.card_fg))
                                })
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(pal.card_fg)
                                        .child(channel_name),
                                )
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(HuddleDockEvent::JumpToChannel(jump_ch.clone()));
                                })),
                        )
                        .child(
                            h_flex()
                                .flex_none()
                                .items_center()
                                .gap_1()
                                .children(visible_people.into_iter().map(|uid| {
                                    let speaking = self.is_speaking(&uid);
                                    let user_muted = self.is_muted_user(&uid);
                                    div()
                                        .relative()
                                        .p(px(1.5))
                                        .rounded_md()
                                        .border_2()
                                        .border_color(if speaking {
                                            pal.success
                                        } else {
                                            pal.border
                                        })
                                        .child(user_chip(&self.fixtures, &uid, 20.))
                                        .when(user_muted, |el| {
                                            el.child(
                                                div()
                                                    .absolute()
                                                    .right(px(-3.))
                                                    .bottom(px(-3.))
                                                    .size(px(12.))
                                                    .rounded_full()
                                                    .bg(pal.card_bg)
                                                    .border_1()
                                                    .border_color(pal.border)
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .child(
                                                        Icon::new(IconName::MicOff)
                                                            .size(px(7.))
                                                            .text_color(pal.danger),
                                                    ),
                                            )
                                        })
                                }))
                                .when(overflow_count > 0, |row| {
                                    row.child(
                                        div()
                                            .px_1()
                                            .py_0p5()
                                            .rounded_md()
                                            .bg(pal.btn_bg)
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(pal.card_fg)
                                            .child(format!("+{overflow_count}")),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex_none()
                                .px_1p5()
                                .py_0p5()
                                .rounded_full()
                                .bg(status_color.opacity(0.14))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(status_color)
                                .child(status_text),
                        )
                        .when_some(self.session.topic.clone(), |row, topic| {
                            row.child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(pal.muted_fg)
                                    .child(topic),
                            )
                        })
                        .when_some(self.session.error.clone(), |row, err_msg| {
                            let retry_ch = channel.clone();
                            let fallback_url = browser_url.clone();
                            row.child(
                                h_flex()
                                    .items_center()
                                    .gap_1p5()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .bg(pal.danger.opacity(0.12))
                                    .border_1()
                                    .border_color(pal.danger.opacity(0.4))
                                    .child(
                                        Icon::new(IconName::TriangleAlert)
                                            .size(px(12.))
                                            .text_color(pal.danger),
                                    )
                                    .child(
                                        div()
                                            .max_w(px(200.))
                                            .truncate()
                                            .text_xs()
                                            .text_color(pal.card_fg)
                                            .child(err_msg),
                                    )
                                    .child(
                                        Button::new("huddle-dock-retry")
                                            .outline()
                                            .with_size(Size::XSmall)
                                            .label("Retry")
                                            .on_click(cx.listener(move |_, _, _, cx| {
                                                cx.emit(HuddleDockEvent::RetryJoin(
                                                    retry_ch.clone(),
                                                ));
                                            })),
                                    )
                                    .child(
                                        Button::new("huddle-dock-browser-fallback")
                                            .primary()
                                            .with_size(Size::XSmall)
                                            .label("Browser")
                                            .on_click(cx.listener(move |_, _, _, cx| {
                                                cx.emit(HuddleDockEvent::OpenBrowser(
                                                    fallback_url.clone(),
                                                ));
                                            })),
                                    ),
                            )
                        }),
                )
                .child(action_bar),
        );

        if self.menu_open {
            bar = bar.child(
                div()
                    .id("huddle-dock-menu-surface")
                    .absolute()
                    .right_2()
                    .bottom(px(52.))
                    .w(px(260.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        if this.menu_open {
                            this.menu_open = false;
                            cx.notify();
                        }
                    }))
                    .child(self.render_menu(&channel, browser_url, cx)),
            );
        }

        bar
    }
}
