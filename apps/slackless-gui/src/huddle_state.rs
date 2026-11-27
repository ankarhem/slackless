//! Shared native Slack Huddle UI state (`Entity<HuddleState>`), observed by both the thin
//! sidebar mini-dock (`HuddleDock`), the Pop-out Huddle Window (`PopoutKind::Huddle`), and the
//! always-on-top floating screen-share control panel (`HuddleFloatingPanel`).
//!
//! High-frequency video frames live in a dedicated [`HuddleVideoStore`] entity so 30–60 fps
//! camera/screen updates never trigger main-window repaints.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::gpui::{App, AppContext as _, Context, Entity, EventEmitter};
use slackless_core::huddle::{
    HuddleAudioLevels, HuddleDataEvent, HuddleDrawMessage, HuddleDrawSegment, HuddlePhase,
    HuddleReactionKind, HuddleSessionSnapshot, VideoFrame, VideoTileKey,
};
use slackless_core::types::{ChannelId, Ts, UserId};
#[cfg(target_os = "macos")]
use slackless_platform::{
    DesktopDrawOverlay, OverlayCursorState, OverlayDrawStroke, OverlayPenEvent,
};

use crate::fixtures::Fixtures;

pub use crate::huddle_video::{
    DisplayVideoFrame, DrawCursorEntry, DrawStrokeEntry, HuddlePopoutGuard, HuddleVideoStore,
    ReactionEntry, format_elapsed, frame_to_render_image, make_catmull_rom_segment,
    stroke_fade_curve,
};

/// A sample huddle fixture for offline/demo mode (`channel` + initial `participants`).
#[derive(Clone)]
pub struct FixtureHuddle {
    pub channel: ChannelId,
    pub participants: Vec<UserId>,
}

/// The bundled sample huddle: running in the first channel with up to three other members.
/// Returns `None` in live workspaces.
pub fn fixture_huddle(fixtures: &Fixtures) -> Option<FixtureHuddle> {
    if !fixtures.sample {
        return None;
    }
    let channel = fixtures.channels.first()?.id.clone();
    let participants = fixtures
        .users
        .iter()
        .map(|user| user.id.clone())
        .filter(|id| *id != fixtures.self_user)
        .take(3)
        .collect();
    Some(FixtureHuddle {
        channel,
        participants,
    })
}

fn now_secs() -> u64 {
    slackless_core::types::time::now_epoch_secs().max(0) as u64
}

fn now_ms() -> u64 {
    slackless_core::types::time::now_epoch_ms()
}

/// One line of live huddle captions: its speaker, the text so far, whether more words are
/// coming, and the detected language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptionLine {
    pub user: UserId,
    pub text: String,
    pub partial: bool,
    pub lang: Option<String>,
}

/// The effect reaction (confetti, applause, ...) a peer just sent, shown for a few seconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveEffect {
    pub user: UserId,
    pub emoji: String,
    pub at_ms: u64,
}

/// Whether a sticky emoji or reaction is the raised hand, however a client spells it.
pub fn is_hand(emoji: Option<&str>) -> bool {
    matches!(emoji, Some("raised_hand" | "\u{270B}"))
}

/// Snapshot consumed by `PopoutKind::Huddle` and `HuddleFloatingPanel` so secondary windows stay
/// in lock-step with [`HuddleState`]; shared, so a popped-out window's render reuses the copy its
/// observer took on the last change. The collections are shared with [`HuddleState`] (copied on
/// write there), so a copy of this is shallow whatever their size. Live video frames live in a
/// separate [`HuddleVideoStore`] entity so high-frequency video updates never repaint the main
/// window.
#[derive(Clone)]
pub struct HuddleViewState {
    pub session: HuddleSessionSnapshot,
    pub participants: Rc<Vec<UserId>>,
    pub levels: Rc<HuddleAudioLevels>,
    pub elapsed_secs: u64,
    pub raised_hands: Rc<HashSet<UserId>>,
    pub sticky_emojis: Rc<HashMap<UserId, String>>,
    pub reactions: Rc<Vec<ReactionEntry>>,
    pub active_effect: Option<ActiveEffect>,
    pub draw_strokes: Rc<Vec<DrawStrokeEntry>>,
    pub draw_cursors: Rc<HashMap<UserId, DrawCursorEntry>>,
    pub draw_mode: bool,
    pub draw_hue: i32,
    pub captions: Rc<Vec<CaptionLine>>,
}

/// App-level actions emitted by [`HuddleState`] (floating panel lifecycle, desktop pen draw
/// packets, and control requests from secondary windows).
pub enum HuddleStateEvent {
    ToggleMute,
    ToggleCamera,
    ToggleScreenShare,
    OpenFloatingPanel,
    CloseFloatingPanel,
    SendDraw(HuddleDrawMessage),
    OpenAudioSettings,
}

/// The room fields of a core session snapshot. `sh_room_*` updates (a peer's background, a
/// huddle thread created after joining) also write them, but the core never sees those and
/// re-emits its snapshot on every mute, topic and perf change.
struct RoomMeta {
    topic: Option<String>,
    background_id: Option<String>,
    thread_root_ts: Option<Ts>,
}

impl RoomMeta {
    fn of(session: &HuddleSessionSnapshot) -> Self {
        Self {
            topic: session.topic.clone(),
            background_id: session.background_id.clone(),
            thread_root_ts: session.thread_root_ts.clone(),
        }
    }
}

/// Single source of truth for the active native huddle's UI state.
pub struct HuddleState {
    fixtures: Arc<Fixtures>,
    session: HuddleSessionSnapshot,
    /// The room fields of the core's last snapshot (see [`HuddleState::update_session`]).
    core_room_meta: RoomMeta,
    participants: Rc<Vec<UserId>>,
    levels: Rc<HuddleAudioLevels>,
    elapsed_secs: u64,
    video_store: Entity<HuddleVideoStore>,
    raised_hands: Rc<HashSet<UserId>>,
    sticky_emojis: Rc<HashMap<UserId, String>>,
    reactions: Rc<Vec<ReactionEntry>>,
    active_effect: Option<ActiveEffect>,
    draw_strokes: Rc<Vec<DrawStrokeEntry>>,
    draw_cursors: Rc<HashMap<UserId, DrawCursorEntry>>,
    draw_mode: bool,
    draw_hue: i32,
    user_hues: HashMap<UserId, i32>,
    captions: Rc<Vec<CaptionLine>>,
    #[cfg(target_os = "macos")]
    overlay_visible: bool,
    #[cfg(target_os = "macos")]
    overlay_fade_task: Option<gpui_kit::gpui::Task<()>>,
    #[cfg(target_os = "macos")]
    desktop_pen_active: bool,
    #[cfg(target_os = "macos")]
    desktop_pen_source: Option<String>,
    #[cfg(target_os = "macos")]
    desktop_pen_task: Option<gpui_kit::gpui::Task<()>>,
    #[cfg(target_os = "macos")]
    desktop_draw_down: bool,
    #[cfg(target_os = "macos")]
    desktop_draw_pts: Vec<(f64, f64)>,
    #[cfg(target_os = "macos")]
    desktop_draw_segs: Vec<HuddleDrawSegment>,
    #[cfg(target_os = "macos")]
    desktop_draw_last_cursor_ms: u64,
}

#[cfg(target_os = "macos")]
impl Drop for HuddleState {
    fn drop(&mut self) {
        if self.overlay_visible || self.desktop_pen_active {
            DesktopDrawOverlay::hide();
            self.overlay_visible = false;
            self.desktop_pen_active = false;
        }
    }
}

/// Fade curve shared with the pop-out stage via [`crate::huddle_video::stroke_fade_curve`].
#[cfg(target_os = "macos")]
fn overlay_stroke_fade(ended_at_ms: Option<u64>, now_ms: u64) -> (f32, f32) {
    let (alpha, width_scale, _) = stroke_fade_curve(ended_at_ms, now_ms);
    (alpha, width_scale)
}

impl EventEmitter<HuddleStateEvent> for HuddleState {}

impl HuddleState {
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
                        .update(cx, |state: &mut HuddleState, cx| {
                            let mut dirty = false;
                            if state.session.started_at_secs > 0
                                && matches!(
                                    state.session.phase,
                                    HuddlePhase::Connected | HuddlePhase::Reconnecting
                                )
                            {
                                state.elapsed_secs =
                                    now_secs().saturating_sub(state.session.started_at_secs);
                                dirty = true;
                            }
                            let now = now_ms();
                            let react_cutoff = now.saturating_sub(4_500);
                            let before = state.reactions.len();
                            Rc::make_mut(&mut state.reactions).retain(|r| r.at_ms >= react_cutoff);
                            if state.reactions.len() != before {
                                dirty = true;
                            }
                            if state
                                .active_effect
                                .as_ref()
                                .is_some_and(|effect| effect.at_ms < now.saturating_sub(3_500))
                            {
                                state.active_effect = None;
                                dirty = true;
                            }
                            let ended_cutoff = now.saturating_sub(5_800);
                            let stale_cutoff = now.saturating_sub(15_000);
                            let strokes_before = state.draw_strokes.len();
                            Rc::make_mut(&mut state.draw_strokes).retain(|s| {
                                if let Some(ended_at) = s.ended_at_ms {
                                    ended_at >= ended_cutoff
                                } else {
                                    s.updated_at_ms >= stale_cutoff
                                }
                            });
                            let cursor_cutoff = now.saturating_sub(3_000);
                            let cursors_before = state.draw_cursors.len();
                            Rc::make_mut(&mut state.draw_cursors)
                                .retain(|_, c| c.updated_at_ms >= cursor_cutoff);
                            if state.draw_strokes.len() != strokes_before
                                || state.draw_cursors.len() != cursors_before
                            {
                                dirty = true;
                                state.sync_desktop_draw_overlay(cx);
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
                core_room_meta: RoomMeta::of(&session),
                session,
                participants: Rc::new(participants),
                levels: Rc::new(HuddleAudioLevels::default()),
                elapsed_secs: if started > 0 {
                    now_secs().saturating_sub(started)
                } else {
                    0
                },
                video_store,
                raised_hands: Rc::default(),
                sticky_emojis: Rc::default(),
                reactions: Rc::default(),
                active_effect: None,
                draw_strokes: Rc::default(),
                draw_cursors: Rc::default(),
                draw_mode: false,
                draw_hue: 210,
                user_hues: HashMap::new(),
                captions: Rc::default(),
                #[cfg(target_os = "macos")]
                overlay_visible: false,
                #[cfg(target_os = "macos")]
                overlay_fade_task: None,
                #[cfg(target_os = "macos")]
                desktop_pen_active: false,
                #[cfg(target_os = "macos")]
                desktop_pen_source: None,
                #[cfg(target_os = "macos")]
                desktop_pen_task: None,
                #[cfg(target_os = "macos")]
                desktop_draw_down: false,
                #[cfg(target_os = "macos")]
                desktop_draw_pts: Vec::new(),
                #[cfg(target_os = "macos")]
                desktop_draw_segs: Vec::new(),
                #[cfg(target_os = "macos")]
                desktop_draw_last_cursor_ms: 0,
            }
        })
    }

    pub fn fixtures(&self) -> &Arc<Fixtures> {
        &self.fixtures
    }

    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    pub fn session(&self) -> &HuddleSessionSnapshot {
        &self.session
    }

    pub fn participants(&self) -> &[UserId] {
        &self.participants
    }

    pub fn levels(&self) -> &HuddleAudioLevels {
        &self.levels
    }

    pub fn elapsed_secs(&self) -> u64 {
        self.elapsed_secs
    }

    pub fn draw_mode(&self) -> bool {
        self.draw_mode
    }

    pub fn draw_hue(&self) -> i32 {
        self.draw_hue
    }

    fn local_draw_item_id(&self) -> String {
        if !self.session.attendee_id.is_empty() {
            self.session.attendee_id.clone()
        } else {
            self.fixtures.self_user.to_string()
        }
    }

    /// Toggle pen mode on/off. Persists across Cmd+Tab / focus loss until explicitly disabled.
    pub fn toggle_draw_mode(&mut self, cx: &mut Context<Self>) {
        self.set_draw_mode(!self.draw_mode, cx);
    }

    pub fn set_draw_mode(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.draw_mode == enabled {
            return;
        }
        self.draw_mode = enabled;
        if enabled {
            let msg = HuddleDrawMessage::ChangeHue {
                item_id: self.local_draw_item_id(),
                hue: self.draw_hue,
            };
            cx.emit(HuddleStateEvent::SendDraw(msg));
        }
        self.sync_desktop_pen_mode(cx);
        self.sync_desktop_draw_overlay(cx);
        cx.notify();
    }

    pub fn set_draw_hue(&mut self, hue: i32, cx: &mut Context<Self>) {
        self.draw_hue = hue;
        let msg = HuddleDrawMessage::ChangeHue {
            item_id: self.local_draw_item_id(),
            hue,
        };
        cx.emit(HuddleStateEvent::SendDraw(msg));
        #[cfg(target_os = "macos")]
        if self.desktop_pen_active {
            // Re-sync the overlay perimeter accent color to match the new pen color.
            self.desktop_pen_active = false;
            self.sync_desktop_pen_mode(cx);
        }
        cx.notify();
    }

    /// Ask the main window to open or focus the always-on-top floating huddle control panel.
    pub fn request_open_floating_panel(&mut self, cx: &mut Context<Self>) {
        cx.emit(HuddleStateEvent::OpenFloatingPanel);
    }

    pub fn request_toggle_mute(&mut self, cx: &mut Context<Self>) {
        cx.emit(HuddleStateEvent::ToggleMute);
    }

    pub fn request_toggle_camera(&mut self, cx: &mut Context<Self>) {
        cx.emit(HuddleStateEvent::ToggleCamera);
    }

    pub fn request_toggle_screen_share(&mut self, cx: &mut Context<Self>) {
        cx.emit(HuddleStateEvent::ToggleScreenShare);
    }

    /// Ask the main window to open Preferences directly on the Audio & video section.
    pub fn open_audio_settings(&mut self, cx: &mut Context<Self>) {
        cx.emit(HuddleStateEvent::OpenAudioSettings);
    }

    #[cfg(target_os = "macos")]
    fn finish_desktop_draw_stroke(&mut self, cx: &mut Context<Self>) {
        if !self.desktop_draw_down {
            return;
        }
        self.desktop_draw_down = false;
        let segs = std::mem::take(&mut self.desktop_draw_segs);
        let item_id = self.local_draw_item_id();
        if self.desktop_draw_pts.len() == 1
            && let Some(&seg) = segs.first()
        {
            cx.emit(HuddleStateEvent::SendDraw(HuddleDrawMessage::New {
                item_id: item_id.clone(),
                segment: seg,
            }));
        }
        self.desktop_draw_pts.clear();
        cx.emit(HuddleStateEvent::SendDraw(HuddleDrawMessage::End {
            item_id: item_id.clone(),
        }));
        if !segs.is_empty() {
            self.commit_local_draw_stroke(item_id, self.draw_hue, segs, cx);
        }
    }

    #[cfg(target_os = "macos")]
    fn handle_desktop_pen_event(&mut self, event: OverlayPenEvent, cx: &mut Context<Self>) {
        if !self.draw_mode || !self.session.sharing {
            return;
        }
        let item_id = self.local_draw_item_id();
        let (nx, ny) = event.coords();
        let raw_pt = (nx.clamp(0.0, 1.0), ny.clamp(0.0, 1.0));
        match event {
            OverlayPenEvent::Down { .. } => {
                if self.desktop_draw_down {
                    self.finish_desktop_draw_stroke(cx);
                }
                self.desktop_draw_down = true;
                self.desktop_draw_pts.clear();
                self.desktop_draw_pts.push(raw_pt);
                self.desktop_draw_segs.clear();
                let dot = make_catmull_rom_segment(raw_pt, raw_pt, raw_pt, raw_pt);
                self.desktop_draw_segs.push(dot);
                cx.emit(HuddleStateEvent::SendDraw(HuddleDrawMessage::ChangeHue {
                    item_id,
                    hue: self.draw_hue,
                }));
                self.sync_desktop_draw_overlay(cx);
            }
            OverlayPenEvent::Drag { .. } => {
                if !self.desktop_draw_down {
                    return;
                }
                let Some(&prev) = self.desktop_draw_pts.last() else {
                    return;
                };
                let dx = raw_pt.0 - prev.0;
                let dy = raw_pt.1 - prev.1;
                if dx * dx + dy * dy < 0.0018 * 0.0018 {
                    return;
                }
                let pt = (
                    prev.0 * 0.18 + raw_pt.0 * 0.82,
                    prev.1 * 0.18 + raw_pt.1 * 0.82,
                );
                self.desktop_draw_pts.push(pt);
                let n = self.desktop_draw_pts.len();
                if n == 2 {
                    let p0 = self.desktop_draw_pts[0];
                    let p1 = self.desktop_draw_pts[1];
                    let seg = make_catmull_rom_segment(p0, p0, p1, p1);
                    self.desktop_draw_segs.clear();
                    self.desktop_draw_segs.push(seg);
                    cx.emit(HuddleStateEvent::SendDraw(HuddleDrawMessage::New {
                        item_id,
                        segment: seg,
                    }));
                } else {
                    let p_prev2 = if n >= 4 {
                        self.desktop_draw_pts[n - 4]
                    } else {
                        self.desktop_draw_pts[0]
                    };
                    let p_prev1 = self.desktop_draw_pts[n - 3];
                    let p_curr = self.desktop_draw_pts[n - 2];
                    let p_next = self.desktop_draw_pts[n - 1];
                    let refined = make_catmull_rom_segment(p_prev2, p_prev1, p_curr, p_next);
                    if let Some(last) = self.desktop_draw_segs.last_mut() {
                        *last = refined;
                    }
                    cx.emit(HuddleStateEvent::SendDraw(HuddleDrawMessage::Edit {
                        item_id: item_id.clone(),
                        segment: refined,
                    }));
                    let next_seg = make_catmull_rom_segment(p_prev1, p_curr, p_next, p_next);
                    self.desktop_draw_segs.push(next_seg);
                    cx.emit(HuddleStateEvent::SendDraw(HuddleDrawMessage::New {
                        item_id,
                        segment: next_seg,
                    }));
                }
                self.sync_desktop_draw_overlay(cx);
            }
            OverlayPenEvent::Up { .. } => {
                self.finish_desktop_draw_stroke(cx);
            }
            OverlayPenEvent::Move { .. } => {
                let now = now_ms();
                if now.saturating_sub(self.desktop_draw_last_cursor_ms) >= 33 {
                    self.desktop_draw_last_cursor_ms = now;
                    cx.emit(HuddleStateEvent::SendDraw(HuddleDrawMessage::Position {
                        item_id,
                        x: raw_pt.0,
                        y: raw_pt.1,
                    }));
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn sync_desktop_pen_mode(&mut self, cx: &mut Context<Self>) {
        let wanted_source = (self.draw_mode && self.session.sharing)
            .then(|| self.session.share_source_id.clone())
            .flatten()
            .filter(|s| !s.is_empty());

        let Some(src) = wanted_source else {
            if self.desktop_draw_down {
                self.finish_desktop_draw_stroke(cx);
            }
            self.desktop_pen_task = None;
            self.desktop_pen_source = None;
            if self.desktop_pen_active {
                self.desktop_pen_active = false;
                DesktopDrawOverlay::set_pen_mode(false, None, self.draw_hue, None);
            }
            return;
        };

        if self.desktop_pen_active && self.desktop_pen_source.as_deref() == Some(src.as_str()) {
            return;
        }

        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        DesktopDrawOverlay::set_pen_mode(
            true,
            Some(&src),
            self.draw_hue,
            Some(Arc::new(move |event| {
                let _ = tx.unbounded_send(event);
            })),
        );
        self.desktop_pen_active = true;
        self.desktop_pen_source = Some(src);
        self.desktop_pen_task = Some(cx.spawn(async move |this, cx| {
            use futures::StreamExt as _;
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |state: &mut HuddleState, cx| {
                        state.handle_desktop_pen_event(event, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    #[cfg(not(target_os = "macos"))]
    fn sync_desktop_pen_mode(&mut self, _cx: &mut Context<Self>) {}

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
                DesktopDrawOverlay::hide();
                self.overlay_visible = false;
            }
            return;
        };

        let now = now_ms();
        let mut segs = Vec::new();
        for stroke in self.draw_strokes.iter() {
            let (alpha, width_scale) = overlay_stroke_fade(stroke.ended_at_ms, now);
            if alpha <= 0.01 {
                continue;
            }
            let width = 5.0 * width_scale;
            for seg in &stroke.segments {
                segs.push(OverlayDrawStroke {
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
        // Paint the in-progress desktop pen stroke immediately with zero latency.
        for seg in &self.desktop_draw_segs {
            segs.push(OverlayDrawStroke {
                ax: seg.a[0],
                ay: seg.a[1],
                bx: seg.b[0],
                by: seg.b[1],
                cx: seg.c[0],
                cy: seg.c[1],
                dx: seg.d[0],
                dy: seg.d[1],
                hue: self.draw_hue,
                alpha: 1.0,
                width: 5.0,
            });
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
            cursors.push(OverlayCursorState {
                x: cur.x,
                y: cur.y,
                hue: cur.hue,
                alpha,
            });
        }

        if segs.is_empty() && cursors.is_empty() {
            self.overlay_fade_task = None;
            if self.desktop_pen_active {
                // Keep the interactive pen overlay open; just clear any faded strokes.
                DesktopDrawOverlay::update(source_id, &[], &[]);
                self.overlay_visible = false;
            } else if self.overlay_visible {
                DesktopDrawOverlay::hide();
                self.overlay_visible = false;
            }
            return;
        }

        DesktopDrawOverlay::update(source_id, &segs, &cursors);
        self.overlay_visible = true;

        if self.overlay_fade_task.is_none() {
            self.overlay_fade_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(33))
                        .await;
                    let keep = this
                        .update(cx, |state: &mut HuddleState, cx| {
                            state.sync_desktop_draw_overlay(cx);
                            state.overlay_visible
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

    pub fn update_session(&mut self, mut session: HuddleSessionSnapshot, cx: &mut Context<Self>) {
        let core_room_meta = RoomMeta::of(&session);
        if session.call_id == self.session.call_id {
            // Keep a room field shown from `sh_room_*` (or a local pick) unless the core itself
            // changed it: its re-emitted snapshot would revert it within one perf tick.
            let (last, shown) = (&self.core_room_meta, &self.session);
            if core_room_meta.topic == last.topic {
                session.topic.clone_from(&shown.topic);
            }
            if core_room_meta.background_id == last.background_id {
                session.background_id.clone_from(&shown.background_id);
            }
            if core_room_meta.thread_root_ts == last.thread_root_ts {
                session.thread_root_ts.clone_from(&shown.thread_root_ts);
            }
        }
        self.core_room_meta = core_room_meta;
        if session.started_at_secs > 0 {
            self.elapsed_secs = now_secs().saturating_sub(session.started_at_secs);
        }
        if session.hand_raised {
            Rc::make_mut(&mut self.raised_hands).insert(self.fixtures.self_user.clone());
        } else {
            Rc::make_mut(&mut self.raised_hands).remove(&self.fixtures.self_user);
        }
        let was_sharing = self.session.sharing;
        let share_changed = self.session.sharing != session.sharing
            || self.session.share_source_id != session.share_source_id;
        self.session = session;
        if share_changed {
            if !self.session.sharing {
                self.draw_mode = false;
            }
            self.sync_desktop_pen_mode(cx);
            self.sync_desktop_draw_overlay(cx);
            if !was_sharing && self.session.sharing {
                cx.emit(HuddleStateEvent::OpenFloatingPanel);
            } else if was_sharing && !self.session.sharing {
                cx.emit(HuddleStateEvent::CloseFloatingPanel);
            }
        }
        cx.notify();
    }

    pub fn set_participants(&mut self, participants: Vec<UserId>, cx: &mut Context<Self>) {
        if *self.participants != participants {
            self.participants = Rc::new(participants);
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
        self.levels = Rc::new(levels);
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
        // Update ONLY `video_store` — never call `cx.notify()` on `HuddleState` so the main
        // window / sidebar mini-dock never repaints 30–60×/sec for video frames it never renders!
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
        self.draw_mode = false;
        #[cfg(target_os = "macos")]
        {
            self.desktop_pen_task = None;
            self.desktop_pen_source = None;
            self.desktop_draw_down = false;
            self.desktop_draw_pts.clear();
            self.desktop_draw_segs.clear();
            self.overlay_fade_task = None;
            if self.overlay_visible || self.desktop_pen_active {
                DesktopDrawOverlay::hide();
                self.overlay_visible = false;
                self.desktop_pen_active = false;
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
        if let Some(root_ts) = thread_root_ts.filter(|ts| !ts.as_str().trim().is_empty())
            && self.session.thread_root_ts.as_ref() != Some(&root_ts)
        {
            self.session.thread_root_ts = Some(root_ts);
            dirty = true;
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
                    self.active_effect = Some(ActiveEffect {
                        user: user.clone(),
                        emoji: emoji.clone(),
                        at_ms: ts,
                    });
                }
                let reactions = Rc::make_mut(&mut self.reactions);
                reactions.push(ReactionEntry {
                    user: user.clone(),
                    emoji: emoji.clone(),
                    at_ms: ts,
                });
                if reactions.len() > 24 {
                    let excess = reactions.len() - 24;
                    reactions.drain(0..excess);
                }
            }
            HuddleDataEvent::HandRaise { user, raised } => {
                if *raised {
                    Rc::make_mut(&mut self.raised_hands).insert(user.clone());
                } else {
                    Rc::make_mut(&mut self.raised_hands).remove(user);
                    if self
                        .sticky_emojis
                        .get(user)
                        .is_some_and(|em| is_hand(Some(em)))
                    {
                        Rc::make_mut(&mut self.sticky_emojis).remove(user);
                    }
                }
                if *user == self.fixtures.self_user {
                    self.session.hand_raised = *raised;
                    if !*raised && is_hand(self.session.sticky_emoji.as_deref()) {
                        self.session.sticky_emoji = None;
                    }
                }
            }
            HuddleDataEvent::StickyEmoji { user, emoji } => {
                let prev_was_hand = self
                    .sticky_emojis
                    .get(user)
                    .is_some_and(|em| is_hand(Some(em)));
                match emoji {
                    Some(em) if !em.is_empty() => {
                        if is_hand(Some(em)) {
                            Rc::make_mut(&mut self.raised_hands).insert(user.clone());
                        } else if prev_was_hand {
                            Rc::make_mut(&mut self.raised_hands).remove(user);
                        }
                        Rc::make_mut(&mut self.sticky_emojis).insert(user.clone(), em.clone());
                    }
                    _ => {
                        Rc::make_mut(&mut self.sticky_emojis).remove(user);
                        Rc::make_mut(&mut self.raised_hands).remove(user);
                    }
                }
                if *user == self.fixtures.self_user {
                    self.session.sticky_emoji = emoji.clone();
                    self.session.hand_raised = is_hand(emoji.as_deref());
                }
            }
            HuddleDataEvent::Topic { topic, .. } => {
                self.session.topic = (!topic.is_empty()).then(|| topic.clone());
            }
            HuddleDataEvent::Caption {
                user,
                text,
                is_partial,
                lang,
                ..
            } => {
                if !text.trim().is_empty() {
                    let captions = Rc::make_mut(&mut self.captions);
                    if let Some(last) = captions.last_mut()
                        && last.user == *user
                        && last.partial
                    {
                        last.text = text.clone();
                        last.partial = *is_partial;
                        if lang.is_some() {
                            last.lang = lang.clone();
                        }
                        cx.notify();
                        return;
                    }
                    captions.push(CaptionLine {
                        user: user.clone(),
                        text: text.clone(),
                        partial: *is_partial,
                        lang: lang.clone(),
                    });
                    if captions.len() > 4 {
                        let excess = captions.len() - 4;
                        captions.drain(0..excess);
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
                        let strokes = Rc::make_mut(&mut self.draw_strokes);
                        if let Some(open) = strokes.iter_mut().rev().find(|s| {
                            !s.ended
                                && s.user == *user
                                && s.item_id == *item_id
                                && s.hue == hue
                                && ts.saturating_sub(s.updated_at_ms) < 10_000
                        }) {
                            open.segments.push(*segment);
                            open.updated_at_ms = ts;
                        } else {
                            strokes.push(DrawStrokeEntry {
                                user: user.clone(),
                                item_id: item_id.clone(),
                                hue,
                                segments: vec![*segment],
                                updated_at_ms: ts,
                                ended: false,
                                ended_at_ms: None,
                            });
                            strokes.drain(..strokes.len().saturating_sub(64));
                        }
                    }
                    HuddleDrawMessage::Edit { item_id, segment } => {
                        if let Some(stroke) = Rc::make_mut(&mut self.draw_strokes)
                            .iter_mut()
                            .rev()
                            .find(|s| !s.ended && s.user == *user && s.item_id == *item_id)
                        {
                            if let Some(last) = stroke.segments.last_mut() {
                                *last = *segment;
                            } else {
                                stroke.segments.push(*segment);
                            }
                            stroke.updated_at_ms = ts;
                        }
                    }
                    HuddleDrawMessage::End { item_id } => {
                        if let Some(stroke) = Rc::make_mut(&mut self.draw_strokes)
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
                        Rc::make_mut(&mut self.draw_cursors).insert(
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
        let was_sharing = self.session.sharing;
        self.session.sharing = source_id.is_some();
        self.session.share_source_id = source_id;
        if !self.session.sharing {
            self.draw_mode = false;
            self.close_video_tile(&VideoTileKey::LocalScreen, cx);
        }
        self.sync_desktop_pen_mode(cx);
        self.sync_desktop_draw_overlay(cx);
        if !was_sharing && self.session.sharing {
            cx.emit(HuddleStateEvent::OpenFloatingPanel);
        } else if was_sharing && !self.session.sharing {
            cx.emit(HuddleStateEvent::CloseFloatingPanel);
        }
        cx.notify();
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
        let strokes = Rc::make_mut(&mut self.draw_strokes);
        strokes.push(DrawStrokeEntry {
            user: self.fixtures.self_user.clone(),
            item_id,
            hue,
            segments,
            updated_at_ms: ts,
            ended: true,
            ended_at_ms: Some(ts),
        });
        strokes.drain(..strokes.len().saturating_sub(64));
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

    pub fn view_state(&self) -> Rc<HuddleViewState> {
        Rc::new(HuddleViewState {
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
            draw_mode: self.draw_mode,
            draw_hue: self.draw_hue,
            captions: self.captions.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui::{self, TestAppContext};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[cfg(target_os = "macos")]
    #[test]
    fn overlay_stroke_fade_covers_active_hold_smoothstep_and_expired() {
        assert_eq!(overlay_stroke_fade(None, 10_000), (1.0, 1.0));
        assert_eq!(overlay_stroke_fade(Some(1_000), 1_000), (1.0, 1.0));
        assert_eq!(overlay_stroke_fade(Some(1_000), 4_799), (1.0, 1.0));
        assert_eq!(overlay_stroke_fade(Some(5_000), 4_000), (1.0, 1.0));

        let (alpha, width) = overlay_stroke_fade(Some(1_000), 5_600);
        assert!((alpha - 0.5).abs() < 1e-5, "midpoint alpha={alpha}");
        assert!((width - 0.86).abs() < 1e-5, "midpoint width={width}");

        let (q_alpha, q_width) = overlay_stroke_fade(Some(1_000), 5_200);
        assert!((q_alpha - 0.84375).abs() < 1e-5, "quarter alpha={q_alpha}");
        assert!(q_width > 0.72 && q_width < 1.0);

        assert_eq!(overlay_stroke_fade(Some(1_000), 6_400), (0.0, 0.72));
        assert_eq!(overlay_stroke_fade(Some(1_000), 99_000), (0.0, 0.72));
    }

    fn make_state(cx: &mut TestAppContext) -> (Entity<HuddleState>, Rc<RefCell<Vec<String>>>) {
        let fx = Arc::new(crate::fixtures::load());
        let events: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let ev_log = events.clone();
        let state = cx.update(|cx| {
            let snap = HuddleSessionSnapshot {
                phase: HuddlePhase::Connected,
                channel: ChannelId::from("C0ENG"),
                call_id: "R0TEST".into(),
                attendee_id: "att-me".into(),
                started_at_secs: now_secs().saturating_sub(10),
                ..Default::default()
            };
            let entity = HuddleState::new(fx, snap, vec![UserId::from("U0ALICE")], cx);
            cx.subscribe(&entity, move |_, ev: &HuddleStateEvent, _| {
                let tag = match ev {
                    HuddleStateEvent::ToggleMute => "toggle_mute",
                    HuddleStateEvent::ToggleCamera => "toggle_cam",
                    HuddleStateEvent::ToggleScreenShare => "toggle_share",
                    HuddleStateEvent::OpenFloatingPanel => "open_floating",
                    HuddleStateEvent::CloseFloatingPanel => "close_floating",
                    HuddleStateEvent::SendDraw(_) => "send_draw",
                    HuddleStateEvent::OpenAudioSettings => "audio_settings",
                };
                ev_log.borrow_mut().push(tag.to_owned());
            })
            .detach();
            entity
        });
        (state, events)
    }

    #[gpui::test]
    fn reactions_effects_hand_raise_sticky_emoji_and_captions_keep_consistent_state(
        cx: &mut TestAppContext,
    ) {
        let (state, _events) = make_state(cx);
        let me = cx.update(|cx| state.read(cx).fixtures().self_user.clone());
        let peer = UserId::from("U0ALICE");

        cx.update(|cx| {
            state.update(cx, |s, cx| {
                // 1. Reaction cap (24 max) + Effect tracking.
                for i in 0..30 {
                    s.apply_data_event(
                        &HuddleDataEvent::Reaction {
                            user: peer.clone(),
                            emoji: format!("e{i}"),
                            kind: if i == 29 {
                                HuddleReactionKind::Effect
                            } else {
                                HuddleReactionKind::Standard
                            },
                            emoji_url: None,
                            at_ms: 0,
                        },
                        cx,
                    );
                }
                assert_eq!(s.view_state().reactions.len(), 24);
                assert_eq!(
                    s.view_state()
                        .active_effect
                        .as_ref()
                        .map(|e| e.emoji.as_str()),
                    Some("e29")
                );

                // 2. Regression: StickyEmoji("raised_hand") followed by StickyEmoji("coffee")
                // MUST clear `raised_hands` so peer's hand isn't stuck raised!
                s.apply_data_event(
                    &HuddleDataEvent::StickyEmoji {
                        user: peer.clone(),
                        emoji: Some("raised_hand".into()),
                    },
                    cx,
                );
                assert!(s.view_state().raised_hands.contains(&peer));
                assert_eq!(
                    s.view_state().sticky_emojis.get(&peer).map(String::as_str),
                    Some("raised_hand")
                );

                s.apply_data_event(
                    &HuddleDataEvent::StickyEmoji {
                        user: peer.clone(),
                        emoji: Some("coffee".into()),
                    },
                    cx,
                );
                assert!(
                    !s.view_state().raised_hands.contains(&peer),
                    "switching sticky emoji away from raised_hand must clear raised_hands"
                );
                assert_eq!(
                    s.view_state().sticky_emojis.get(&peer).map(String::as_str),
                    Some("coffee")
                );

                // 3. HandRaise(false) for self clears both `session.hand_raised` and `"raised_hand"` sticky.
                s.apply_data_event(
                    &HuddleDataEvent::StickyEmoji {
                        user: me.clone(),
                        emoji: Some("raised_hand".into()),
                    },
                    cx,
                );
                assert!(s.session().hand_raised);
                s.apply_data_event(
                    &HuddleDataEvent::HandRaise {
                        user: me.clone(),
                        raised: false,
                    },
                    cx,
                );
                assert!(!s.session().hand_raised);
                assert!(!s.view_state().raised_hands.contains(&me));
                assert!(!s.view_state().sticky_emojis.contains_key(&me));

                // 4. Captions: partial from same speaker updates in-place; finalizes; caps at 4.
                s.apply_data_event(
                    &HuddleDataEvent::Caption {
                        user: peer.clone(),
                        text: "Hel".into(),
                        is_partial: true,
                        at_ms: 100,
                        lang: Some("en".into()),
                    },
                    cx,
                );
                s.apply_data_event(
                    &HuddleDataEvent::Caption {
                        user: peer.clone(),
                        text: "Hello world".into(),
                        is_partial: false,
                        at_ms: 200,
                        lang: None,
                    },
                    cx,
                );
                assert_eq!(s.view_state().captions.len(), 1);
                assert_eq!(s.view_state().captions[0].text, "Hello world");
                assert_eq!(s.view_state().captions[0].lang.as_deref(), Some("en"));
                for i in 0..6 {
                    s.apply_data_event(
                        &HuddleDataEvent::Caption {
                            user: peer.clone(),
                            text: format!("line {i}"),
                            is_partial: false,
                            at_ms: 300 + i,
                            lang: None,
                        },
                        cx,
                    );
                }
                assert_eq!(s.view_state().captions.len(), 4);

                // 5. Topic, Jukebox, MutedByPeer, room meta.
                s.apply_data_event(
                    &HuddleDataEvent::Topic {
                        user: peer.clone(),
                        topic: "Incident review".into(),
                    },
                    cx,
                );
                s.apply_data_event(
                    &HuddleDataEvent::Jukebox {
                        user: peer.clone(),
                        song_key: Some("lofi".into()),
                    },
                    cx,
                );
                s.apply_data_event(
                    &HuddleDataEvent::MutedByPeer {
                        actor: peer.clone(),
                    },
                    cx,
                );
                assert_eq!(s.session().topic.as_deref(), Some("Incident review"));
                assert_eq!(s.session().jukebox_song.as_deref(), Some("lofi"));
                assert!(s.session().muted);

                s.apply_room_meta(
                    Some("Updated topic".into()),
                    Some("bg-1".into()),
                    Some(slackless_core::types::Ts::from("1700000001.000100")),
                    cx,
                );
                assert_eq!(s.session().topic.as_deref(), Some("Updated topic"));
                assert_eq!(s.session().background_id.as_deref(), Some("bg-1"));
                assert_eq!(
                    s.session().thread_root_ts.as_ref().map(|t| t.as_str()),
                    Some("1700000001.000100")
                );

                // 6. The core re-emits its snapshot on every mute/perf tick without seeing room
                // updates: the room meta above must survive it.
                let core_snap = HuddleSessionSnapshot {
                    phase: HuddlePhase::Connected,
                    channel: ChannelId::from("C0ENG"),
                    call_id: "R0TEST".into(),
                    muted: true,
                    ..Default::default()
                };
                s.update_session(core_snap.clone(), cx);
                assert_eq!(s.session().topic.as_deref(), Some("Updated topic"));
                assert_eq!(s.session().background_id.as_deref(), Some("bg-1"));
                assert_eq!(
                    s.session().thread_root_ts.as_ref().map(|t| t.as_str()),
                    Some("1700000001.000100")
                );
                // A field the core itself changes (a local background pick) wins, and a later
                // peer change still survives the core's next re-emit of its own value.
                let local_bg = HuddleSessionSnapshot {
                    background_id: Some("bg-local".into()),
                    ..core_snap.clone()
                };
                s.update_session(local_bg.clone(), cx);
                assert_eq!(s.session().background_id.as_deref(), Some("bg-local"));
                s.apply_room_meta(None, Some("bg-peer".into()), None, cx);
                s.update_session(local_bg, cx);
                assert_eq!(s.session().background_id.as_deref(), Some("bg-peer"));
                // Another call starts from the core's snapshot alone.
                s.update_session(
                    HuddleSessionSnapshot {
                        call_id: "R0NEXT".into(),
                        ..core_snap
                    },
                    cx,
                );
                assert_eq!(s.session().thread_root_ts, None);
            });
        });
    }

    #[gpui::test]
    fn draw_strokes_cursors_sharing_floating_panel_and_local_controls(cx: &mut TestAppContext) {
        let (state, events) = make_state(cx);
        let peer = UserId::from("U0ALICE");
        let seg1 = HuddleDrawSegment {
            a: [0.1, 0.1],
            b: [0.2, 0.2],
            c: [0.3, 0.3],
            d: [0.4, 0.4],
        };
        let seg2 = HuddleDrawSegment {
            a: [0.4, 0.4],
            b: [0.5, 0.5],
            c: [0.6, 0.6],
            d: [0.7, 0.7],
        };

        cx.update(|cx| {
            state.update(cx, |s, cx| {
                // Request events forward cleanly.
                s.request_toggle_mute(cx);
                s.request_toggle_camera(cx);
                s.request_toggle_screen_share(cx);
                s.request_open_floating_panel(cx);
                s.open_audio_settings(cx);

                // Remote draw protocol: ChangeHue -> New -> Edit -> End + Position.
                s.apply_data_event(
                    &HuddleDataEvent::Draw {
                        user: peer.clone(),
                        message: HuddleDrawMessage::ChangeHue {
                            item_id: "s1".into(),
                            hue: 140,
                        },
                        at_ms: 1,
                    },
                    cx,
                );
                s.apply_data_event(
                    &HuddleDataEvent::Draw {
                        user: peer.clone(),
                        message: HuddleDrawMessage::New {
                            item_id: "s1".into(),
                            segment: seg1,
                        },
                        at_ms: 2,
                    },
                    cx,
                );
                s.apply_data_event(
                    &HuddleDataEvent::Draw {
                        user: peer.clone(),
                        message: HuddleDrawMessage::Edit {
                            item_id: "s1".into(),
                            segment: seg2,
                        },
                        at_ms: 3,
                    },
                    cx,
                );
                s.apply_data_event(
                    &HuddleDataEvent::Draw {
                        user: peer.clone(),
                        message: HuddleDrawMessage::End {
                            item_id: "s1".into(),
                        },
                        at_ms: 4,
                    },
                    cx,
                );
                s.apply_data_event(
                    &HuddleDataEvent::Draw {
                        user: peer.clone(),
                        message: HuddleDrawMessage::Position {
                            item_id: "s1".into(),
                            x: 0.5,
                            y: 0.6,
                        },
                        at_ms: 5,
                    },
                    cx,
                );

                let vs = s.view_state();
                assert_eq!(vs.draw_strokes.len(), 1);
                assert_eq!(vs.draw_strokes[0].hue, 140);
                assert!(vs.draw_strokes[0].ended);
                assert_eq!(vs.draw_strokes[0].segments, vec![seg2]);
                assert!(vs.draw_cursors.contains_key(&peer));

                // Local draw mode + hue + commit stroke.
                s.toggle_draw_mode(cx);
                assert!(s.draw_mode());
                s.set_draw_hue(30, cx);
                assert_eq!(s.draw_hue(), 30);
                s.commit_local_draw_stroke("local-1".into(), 30, vec![seg1], cx);
                assert_eq!(s.view_state().draw_strokes.len(), 2);

                // Session sharing transitions emit OpenFloatingPanel then CloseFloatingPanel and
                // disable draw_mode when sharing stops.
                let mut sharing_snap = s.session().clone();
                sharing_snap.sharing = true;
                sharing_snap.share_source_id = Some("monitor:1".into());
                s.update_session(sharing_snap.clone(), cx);
                sharing_snap.sharing = false;
                sharing_snap.share_source_id = None;
                s.update_session(sharing_snap, cx);
                assert!(!s.draw_mode(), "stopping share must disable draw_mode");

                // Local optimistic setters + audio_only clears video frames.
                s.set_muted_local(true, cx);
                s.set_camera_local(true, cx);
                s.set_blur_local(true, cx);
                s.set_captions_local(true, cx);
                s.set_background_local("bg-2".into(), cx);
                s.set_jukebox_local(Some("ambient".into()), cx);
                s.set_participants(vec![peer.clone()], cx);
                s.set_audio_only_local(true, cx);
                assert!(!s.session().video);
                assert!(s.session().audio_only);
                s.clear_video_frames(cx);
            });
        });

        let log = events.borrow();
        assert!(log.contains(&"toggle_mute".to_owned()));
        assert!(log.contains(&"toggle_cam".to_owned()));
        assert!(log.contains(&"toggle_share".to_owned()));
        assert!(log.contains(&"open_floating".to_owned()));
        assert!(log.contains(&"close_floating".to_owned()));
        assert!(log.contains(&"send_draw".to_owned()));
        assert!(log.contains(&"audio_settings".to_owned()));
    }
}
