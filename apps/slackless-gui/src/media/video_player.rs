//! Native in-app video playback engine for inline chat cards and reusable video rendering/transport
//! helpers.
//!
//! On macOS this drives [`slackless_core::huddle::NativeVideoPlayer`] (`AVPlayer` +
//! `AVPlayerItemVideoOutput`) with zero-copy `420f` IOSurface `CVPixelBuffer` frames painted
//! directly through GPUI's Metal surface path (`window.paint_surface`). Only one inline video
//! plays at a time, and expanding to [`crate::image_viewer::ImageViewer`] hands off the exact
//! playback position cleanly with zero duplicate audio.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, Bounds, Global, Hsla, ImageSource, InteractiveElement as _, IntoElement as _,
    MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, ParentElement as _, Pixels,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas, div, px, relative,
    rgb,
};
use slackless_core::huddle::NativeVideoPlayer;

use super::audio_player::fetch_cached_media;
use super::downloads::active_fetcher;

#[cfg(target_os = "macos")]
pub type VideoSurface = slackless_core::huddle::MacVideoSurface;

#[cfg(not(target_os = "macos"))]
#[derive(Debug)]
pub struct VideoSurface;

/// ~30 fps refresh cadence while a video is actively playing or settling after a seek.
pub(crate) const VIDEO_FRAME_INTERVAL: Duration = Duration::from_millis(33);
/// Number of frames to keep polling after a pause/seek so decoded seek frames land smoothly.
const SETTLE_TICKS: u32 = 15;

/// Snapshot of an inline chat video's current playback state.
#[derive(Clone, Debug)]
pub struct InlineVideoSnapshot {
    pub loading: bool,
    pub playing: bool,
    pub muted: bool,
    pub ended: bool,
    pub failed: bool,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
    pub surface: Option<Arc<VideoSurface>>,
}

struct ActiveInlineVideo {
    generation: u64,
    tick_gen: u64,
    file_id: String,
    loading: bool,
    player: Option<NativeVideoPlayer>,
    last_frame: Option<Arc<VideoSurface>>,
}

#[derive(Default)]
struct InlineVideoManager {
    next_generation: u64,
    active: Option<ActiveInlineVideo>,
    /// One-shot `(file_id, position_secs)` stashed when expanding from inline playback into
    /// [`crate::image_viewer::ImageViewer`] so the viewer resumes at the exact timestamp.
    pending_handoff: Option<(String, f64)>,
}

impl InlineVideoManager {
    fn next_gen(&mut self) -> u64 {
        self.next_generation = self.next_generation.wrapping_add(1);
        self.next_generation
    }

    fn stop_matching(&mut self, file_id: &str) -> Option<f64> {
        let matches = self
            .active
            .as_ref()
            .is_some_and(|active| active.file_id == file_id);
        if !matches {
            return None;
        }
        self.next_gen();
        let active = self.active.take()?;
        active_resume_position(&active)
    }

    fn stop_all(&mut self) -> Option<(String, f64)> {
        self.next_gen();
        let active = self.active.take()?;
        let pos = active_resume_position(&active).unwrap_or(0.0);
        Some((active.file_id, pos))
    }

    fn stash_handoff(&mut self, file_id: String, pos_secs: f64) {
        if pos_secs.is_finite() && pos_secs > 0.05 {
            self.pending_handoff = Some((file_id, pos_secs));
        } else {
            self.pending_handoff = None;
        }
    }

    fn take_handoff(&mut self, file_id: &str) -> Option<f64> {
        if self
            .pending_handoff
            .as_ref()
            .is_some_and(|(id, _)| id == file_id)
        {
            self.pending_handoff.take().map(|(_, pos)| pos)
        } else {
            None
        }
    }
}

impl Global for InlineVideoManager {}

fn active_resume_position(active: &ActiveInlineVideo) -> Option<f64> {
    let player = active.player.as_ref()?;
    let st = player.state();
    if !st.ended && st.position_secs.is_finite() && st.position_secs > 0.05 {
        Some(st.position_secs)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn copy_player_surface(player: &NativeVideoPlayer) -> Option<Arc<VideoSurface>> {
    player.copy_frame().map(Arc::new)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn copy_player_surface(_player: &NativeVideoPlayer) -> Option<Arc<VideoSurface>> {
    None
}

/// Current inline video playback snapshot for `file_id`, if it is loading, playing, or paused.
pub fn inline_video_snapshot(file_id: &str, cx: &App) -> Option<InlineVideoSnapshot> {
    let active = cx.try_global::<InlineVideoManager>()?.active.as_ref()?;
    if active.file_id != file_id {
        return None;
    }
    if active.loading {
        return Some(InlineVideoSnapshot {
            loading: true,
            playing: false,
            muted: false,
            ended: false,
            failed: false,
            position_secs: 0.0,
            duration_secs: 0.0,
            width: 0,
            height: 0,
            surface: active.last_frame.clone(),
        });
    }
    let player = active.player.as_ref()?;
    let st = player.state();
    let surface = copy_player_surface(player).or_else(|| active.last_frame.clone());
    Some(InlineVideoSnapshot {
        loading: false,
        playing: st.playing,
        muted: st.muted,
        ended: st.ended,
        failed: st.failed,
        position_secs: sanitize_secs(st.position_secs),
        duration_secs: sanitize_secs(st.duration_secs),
        width: st.width,
        height: st.height,
        surface,
    })
}

/// Start native in-app video playback inline inside the chat card for `file_id`.
///
/// Stops any previously playing inline video so only one plays at a time. Never launches
/// QuickTime or any external player on macOS.
pub fn play_inline_video(
    file_id: String,
    url: String,
    ext: String,
    start_pos_secs: Option<f64>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(fetcher) = active_fetcher(cx) else {
        crate::actions::notify(
            "Video playback",
            "Playback needs a signed-in workspace.",
            window,
            cx,
        );
        return;
    };
    let mgr = cx.default_global::<InlineVideoManager>();
    let generation = mgr.next_gen();
    mgr.pending_handoff = None;
    mgr.active = Some(ActiveInlineVideo {
        generation,
        tick_gen: 0,
        file_id: file_id.clone(),
        loading: true,
        player: None,
        last_frame: None,
    });
    window.refresh();

    let cache_id = file_id.clone();
    window
        .spawn(cx, async move |cx| {
            let bg = cx.background_executor().clone();
            let path = fetch_cached_media(fetcher, cache_id, url, ext, bg.clone()).await;
            let open_res = match path {
                Ok(path) => bg
                    .spawn(async move { NativeVideoPlayer::open(&path) })
                    .await
                    .map_err(|e| e.to_string()),
                Err(err) => Err(err),
            };
            let started = cx
                .update(|window, cx| {
                    let mgr = cx.default_global::<InlineVideoManager>();
                    let matches = mgr
                        .active
                        .as_ref()
                        .is_some_and(|a| a.generation == generation);
                    if !matches {
                        // Another video started or playback was stopped while loading; drop cleanly.
                        drop(open_res);
                        return None;
                    }
                    match open_res {
                        Ok(player) => {
                            if let Some(pos) = start_pos_secs
                                && pos.is_finite()
                                && pos > 0.05
                            {
                                player.seek(pos);
                            }
                            player.play();
                            let initial_frame = copy_player_surface(&player);
                            let mut tick_gen = 0;
                            if let Some(active) = mgr.active.as_mut() {
                                active.loading = false;
                                active.last_frame = initial_frame;
                                active.player = Some(player);
                                active.tick_gen = active.tick_gen.wrapping_add(1);
                                tick_gen = active.tick_gen;
                            }
                            window.refresh();
                            Some(tick_gen)
                        }
                        Err(err) => {
                            mgr.active = None;
                            crate::actions::notify("Video playback failed", err, window, cx);
                            window.refresh();
                            None
                        }
                    }
                })
                .ok()
                .flatten();

            if let Some(tick_gen) = started {
                run_inline_refresh_loop(generation, tick_gen, cx).await;
            }
        })
        .detach();
}

/// Toggle play/pause on the currently active inline video matching `file_id`.
pub fn toggle_inline_video_pause(file_id: &str, window: &mut Window, cx: &mut App) {
    let kick = {
        let mgr = cx.default_global::<InlineVideoManager>();
        let Some(active) = mgr.active.as_mut().filter(|a| a.file_id == file_id) else {
            return;
        };
        let Some(player) = active.player.as_ref() else {
            return;
        };
        player.toggle_play();
        if let Some(frame) = copy_player_surface(player) {
            active.last_frame = Some(frame);
        }
        active.tick_gen = active.tick_gen.wrapping_add(1);
        (active.generation, active.tick_gen)
    };
    window.refresh();
    spawn_inline_refresh_loop(kick.0, kick.1, window, cx);
}

/// Toggle mute/unmute on the currently active inline video matching `file_id`.
pub fn toggle_inline_video_mute(file_id: &str, window: &mut Window, cx: &mut App) {
    let mgr = cx.default_global::<InlineVideoManager>();
    let Some(active) = mgr.active.as_mut().filter(|a| a.file_id == file_id) else {
        return;
    };
    let Some(player) = active.player.as_ref() else {
        return;
    };
    let next_muted = !player.state().muted;
    player.set_muted(next_muted);
    window.refresh();
}

/// Seek the active inline video matching `file_id` to `secs`.
pub fn seek_inline_video(file_id: &str, secs: f64, window: &mut Window, cx: &mut App) {
    let kick = {
        let mgr = cx.default_global::<InlineVideoManager>();
        let Some(active) = mgr.active.as_mut().filter(|a| a.file_id == file_id) else {
            return;
        };
        let Some(player) = active.player.as_ref() else {
            return;
        };
        let duration = sanitize_secs(player.state().duration_secs);
        let target = if duration > 0.0 {
            sanitize_secs(secs).min(duration)
        } else {
            sanitize_secs(secs)
        };
        player.seek(target);
        if let Some(frame) = copy_player_surface(player) {
            active.last_frame = Some(frame);
        }
        active.tick_gen = active.tick_gen.wrapping_add(1);
        (active.generation, active.tick_gen)
    };
    window.refresh();
    spawn_inline_refresh_loop(kick.0, kick.1, window, cx);
}

/// Stop inline playback for `file_id`, returning its playback position in seconds (if mid-video).
pub fn stop_inline_video(file_id: &str, window: &mut Window, cx: &mut App) -> Option<f64> {
    let pos = cx
        .default_global::<InlineVideoManager>()
        .stop_matching(file_id);
    window.refresh();
    pos
}

/// Stop inline playback for `file_id` and stash its playback position so opening
/// [`crate::image_viewer::ImageViewer`] immediately resumes from the same timestamp.
pub fn stop_inline_video_for_expand(
    file_id: &str,
    window: &mut Window,
    cx: &mut App,
) -> Option<f64> {
    let mgr = cx.default_global::<InlineVideoManager>();
    let pos = mgr.stop_matching(file_id);
    if let Some(secs) = pos {
        mgr.stash_handoff(file_id.to_owned(), secs);
    }
    window.refresh();
    pos
}

/// Called by [`crate::image_viewer::ImageViewer`] when starting playback of `file_id`: stops any
/// active inline video so audio never overlaps, and returns the carried-over timestamp if the
/// same video was just playing inline (or expanded via [`stop_inline_video_for_expand`]).
pub(crate) fn take_inline_handoff_for_viewer(
    file_id: &str,
    window: &mut Window,
    cx: &mut App,
) -> Option<f64> {
    let mgr = cx.default_global::<InlineVideoManager>();
    let stashed = mgr.take_handoff(file_id);
    let stopped = mgr.stop_all();
    let had_active = stopped.is_some();
    let carry = stashed.or_else(|| {
        stopped
            .filter(|(id, pos)| id == file_id && *pos > 0.05)
            .map(|(_, pos)| pos)
    });
    if had_active {
        window.refresh();
    }
    carry
}

fn spawn_inline_refresh_loop(generation: u64, tick_gen: u64, window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            run_inline_refresh_loop(generation, tick_gen, cx).await;
        })
        .detach();
}

async fn run_inline_refresh_loop(
    generation: u64,
    tick_gen: u64,
    cx: &mut gpui_kit::gpui::AsyncWindowContext,
) {
    let mut idle_ticks = 0u32;
    loop {
        cx.background_executor().timer(VIDEO_FRAME_INTERVAL).await;
        let keep_going = cx
            .update(|window, cx| {
                let mgr = cx.default_global::<InlineVideoManager>();
                let Some(active) = mgr.active.as_mut() else {
                    return false;
                };
                if active.generation != generation || active.tick_gen != tick_gen {
                    return false;
                }
                let Some(player) = active.player.as_ref() else {
                    return false;
                };
                if let Some(frame) = copy_player_surface(player) {
                    active.last_frame = Some(frame);
                }
                let st = player.state();
                window.refresh();
                if st.failed {
                    return false;
                }
                if st.playing && !st.ended {
                    idle_ticks = 0;
                    true
                } else {
                    // Keep refreshing briefly after pause/seek/end so the final frame paints,
                    // then stop ticking to stay at 0% idle CPU.
                    idle_ticks += 1;
                    idle_ticks < SETTLE_TICKS
                }
            })
            .unwrap_or(false);
        if !keep_going {
            break;
        }
    }
}

/// Render a zero-copy native video surface (`CVPixelBuffer` via `window.paint_surface` on macOS),
/// falling back to `poster` before the first decoded frame arrives.
pub fn render_video_surface(
    surface: Option<Arc<VideoSurface>>,
    poster: Option<ImageSource>,
    radius: Pixels,
) -> AnyElement {
    let container = div()
        .relative()
        .size_full()
        .rounded(radius)
        .overflow_hidden()
        .bg(rgb(0x0B0D10));

    #[cfg(target_os = "macos")]
    if let Some(surface) = surface {
        let fw = surface.width().max(1);
        let fh = surface.height().max(1);
        let video_canvas = canvas(
            move |bounds, _, _| {
                ObjectFit::Contain.get_bounds(
                    bounds,
                    gpui_kit::gpui::size(
                        gpui_kit::gpui::DevicePixels(fw as i32),
                        gpui_kit::gpui::DevicePixels(fh as i32),
                    ),
                )
            },
            move |_, image_bounds, window, _| {
                let ptr =
                    surface.retain_cv_pixel_buffer_ptr() as *mut objc2_core_video::CVPixelBuffer;
                if let Some(nn) = std::ptr::NonNull::new(ptr) {
                    // SAFETY: `retain_cv_pixel_buffer_ptr` returned a +1 retained `420f`
                    // IOSurface-backed `CVPixelBuffer`; `CFRetained::from_raw` takes ownership
                    // and releases on Drop.
                    let cv_buf = unsafe { objc2_core_foundation::CFRetained::from_raw(nn) };
                    window.paint_surface(image_bounds, cv_buf);
                }
            },
        )
        .size_full();
        return container.child(video_canvas).into_any_element();
    }

    #[cfg(not(target_os = "macos"))]
    let _ = surface;

    container
        .when_some(poster, |el, poster| {
            el.child(
                super::Picture::new(poster)
                    .fit(ObjectFit::Contain)
                    .radius(radius),
            )
        })
        .into_any_element()
}

type SeekCallback = Rc<dyn Fn(f64, &mut Window, &mut App)>;

/// Interactive click-and-drag video progress / seek scrubber bar.
pub fn video_scrub_bar(
    id: SharedString,
    progress: f32,
    duration_secs: f64,
    track_bg: Hsla,
    fill_bg: Hsla,
    on_seek: impl Fn(f64, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let progress = progress.clamp(0.0, 1.0);
    let bounds_cell: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));
    let bounds_measure = Rc::clone(&bounds_cell);
    let bounds_down = Rc::clone(&bounds_cell);
    let bounds_move = bounds_cell;
    let on_seek: SeekCallback = Rc::new(on_seek);
    let on_seek_move = Rc::clone(&on_seek);

    let seek_at = |x_pos: Pixels, bounds: Bounds<Pixels>, duration: f64| -> Option<f64> {
        let w: f32 = bounds.size.width.into();
        if w <= 1.0 || duration <= 0.0 || !duration.is_finite() {
            return None;
        }
        let x: f32 = (x_pos - bounds.origin.x).into();
        Some(scrub_fraction_to_secs(x / w, duration))
    };

    div()
        .id(id)
        .relative()
        .flex_1()
        .min_w(px(40.))
        .h(px(18.))
        .flex()
        .items_center()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, window, cx| {
            cx.stop_propagation();
            if let Some(secs) = seek_at(ev.position.x, bounds_down.get(), duration_secs) {
                on_seek(secs, window, cx);
            }
        })
        .on_mouse_move(move |ev: &MouseMoveEvent, window, cx| {
            if ev.pressed_button == Some(MouseButton::Left)
                && let Some(secs) = seek_at(ev.position.x, bounds_move.get(), duration_secs)
            {
                on_seek_move(secs, window, cx);
            }
        })
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(
            div()
                .w_full()
                .h(px(4.))
                .rounded_full()
                .bg(track_bg)
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(progress))
                        .rounded_full()
                        .bg(fill_bg),
                ),
        )
        .child(
            canvas(
                move |bounds, _, _| {
                    bounds_measure.set(bounds);
                },
                |_, (), _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .into_any_element()
}

/// Sanitize a floating-point timestamp/duration into a non-negative finite number of seconds.
pub fn sanitize_secs(secs: f64) -> f64 {
    if secs.is_finite() && secs > 0.0 {
        secs
    } else {
        0.0
    }
}

/// Map a normalized `[0.0, 1.0]` scrub bar fraction to seconds within `duration_secs`.
pub fn scrub_fraction_to_secs(fraction: f32, duration_secs: f64) -> f64 {
    let dur = sanitize_secs(duration_secs);
    let frac = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0) as f64
    } else {
        0.0
    };
    dur * frac
}

/// Compute normalized `[0.0, 1.0]` playback progress for a scrub bar.
pub fn progress_fraction(position_secs: f64, duration_secs: f64) -> f32 {
    let dur = sanitize_secs(duration_secs);
    if dur <= 0.0 {
        return 0.0;
    }
    let pos = sanitize_secs(position_secs);
    (pos / dur).clamp(0.0, 1.0) as f32
}

/// Format seconds as `m:ss` (or `h:mm:ss` when >= 1 hour).
pub fn format_video_time(secs: f64) -> String {
    let total = sanitize_secs(secs).floor() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_time_and_scrub_math_helpers() {
        assert_eq!(format_video_time(0.0), "0:00");
        assert_eq!(format_video_time(-3.5), "0:00");
        assert_eq!(format_video_time(f64::NAN), "0:00");
        assert_eq!(format_video_time(5.9), "0:05");
        assert_eq!(format_video_time(65.2), "1:05");
        assert_eq!(format_video_time(3661.0), "1:01:01");

        assert_eq!(scrub_fraction_to_secs(-0.5, 120.0), 0.0);
        assert_eq!(scrub_fraction_to_secs(0.25, 120.0), 30.0);
        assert_eq!(scrub_fraction_to_secs(1.5, 120.0), 120.0);
        assert_eq!(scrub_fraction_to_secs(f32::NAN, 120.0), 0.0);
        assert_eq!(scrub_fraction_to_secs(0.5, f64::INFINITY), 0.0);

        assert_eq!(progress_fraction(0.0, 0.0), 0.0);
        assert!((progress_fraction(15.0, 60.0) - 0.25).abs() < 1e-6);
        assert_eq!(progress_fraction(90.0, 60.0), 1.0);
    }

    #[test]
    fn inline_video_manager_single_active_and_handoff() {
        let mut mgr = InlineVideoManager::default();
        let g1 = mgr.next_gen();
        mgr.active = Some(ActiveInlineVideo {
            generation: g1,
            tick_gen: 0,
            file_id: "F_VID_1".into(),
            loading: true,
            player: None,
            last_frame: None,
        });
        assert_eq!(mgr.active.as_ref().unwrap().file_id, "F_VID_1");

        // Starting a second video replaces the first and advances generation.
        let g2 = mgr.next_gen();
        assert!(g2 > g1);
        mgr.active = Some(ActiveInlineVideo {
            generation: g2,
            tick_gen: 0,
            file_id: "F_VID_2".into(),
            loading: false,
            player: None,
            last_frame: None,
        });
        assert_eq!(mgr.stop_matching("F_VID_1"), None);
        assert_eq!(mgr.active.as_ref().unwrap().file_id, "F_VID_2");

        // Stopping the matching video clears active state.
        assert_eq!(mgr.stop_matching("F_VID_2"), None);
        assert!(mgr.active.is_none());

        // One-shot handoff only matches its file_id and clears on take.
        mgr.stash_handoff("F_VID_2".into(), 12.5);
        assert_eq!(mgr.take_handoff("F_OTHER"), None);
        assert_eq!(mgr.take_handoff("F_VID_2"), Some(12.5));
        assert_eq!(mgr.take_handoff("F_VID_2"), None);

        // Near-zero timestamps are ignored for handoff.
        mgr.stash_handoff("F_VID_2".into(), 0.01);
        assert_eq!(mgr.take_handoff("F_VID_2"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn inline_video_manager_stop_returns_real_player_position() {
        use slackless_core::huddle::clips::mux_h264_mp4;
        use slackless_core::huddle::video::H264VideoEncoder;
        use std::time::Instant;

        let w = 160usize;
        let h = 96usize;
        let mut enc = H264VideoEncoder::new_screen().expect("openh264 screen encoder");
        let mut frames = Vec::new();
        for f in 0..24u8 {
            let mut rgb = vec![0u8; w * h * 3];
            for px_chunk in rgb.as_chunks_mut::<3>().0 {
                px_chunk[0] = f.saturating_mul(10);
                px_chunk[1] = 80;
                px_chunk[2] = 160;
            }
            frames.push(enc.encode_rgb(&rgb, w, h).expect("encode frame"));
        }
        let mp4 = mux_h264_mp4(w as u32, h as u32, 12, &frames).expect("mux mp4");
        let path = std::env::temp_dir().join(format!(
            "slackless-inline-video-test-{}.mp4",
            std::process::id()
        ));
        std::fs::write(&path, &mp4).expect("write temp mp4");

        let player = NativeVideoPlayer::open(&path).expect("open synthetic mp4");
        let deadline = Instant::now() + Duration::from_secs(3);
        while player.state().duration_secs <= 0.0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        player.seek(0.75);
        let seek_deadline = Instant::now() + Duration::from_secs(2);
        while player.state().position_secs < 0.25 && Instant::now() < seek_deadline {
            std::thread::sleep(Duration::from_millis(20));
        }

        let mut mgr = InlineVideoManager::default();
        let g = mgr.next_gen();
        mgr.active = Some(ActiveInlineVideo {
            generation: g,
            tick_gen: 0,
            file_id: "F_SYNTH".into(),
            loading: false,
            player: Some(player),
            last_frame: None,
        });

        let stopped_pos = mgr.stop_matching("F_SYNTH");
        assert!(mgr.active.is_none());
        assert!(
            stopped_pos.is_some_and(|p| p >= 0.25),
            "expected stopped position >= 0.25s, got {stopped_pos:?}"
        );
        let _ = std::fs::remove_file(&path);
    }
}
