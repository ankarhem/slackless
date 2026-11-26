//! Native in-app video playback engine for inline chat cards and reusable video rendering/transport
//! helpers.
//!
//! On macOS this drives [`slackless_core::huddle::NativeVideoPlayer`] (`AVPlayer` +
//! `AVPlayerItemVideoOutput`) with zero-copy `420f` IOSurface `CVPixelBuffer` frames painted
//! directly through GPUI's Metal surface path (`window.paint_surface`). Only one inline video
//! plays at a time, and expanding to [`crate::image_viewer::ImageViewer`] hands off the exact
//! playback position cleanly with zero duplicate audio.

use std::cell::Cell;
use std::collections::HashSet;
use std::path::PathBuf;
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
use slackless_core::huddle::{
    NativeVideoPlayer, TranscriptionProvider, resolve_local_file_provider,
};
use slackless_core::types::{ChannelId, FileId, Ts, UserId};

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
    pub rate: f32,
    pub width: u32,
    pub height: u32,
    pub surface: Option<Arc<VideoSurface>>,
}

/// Metadata describing the currently playing inline video so the floating mini-player /
/// picture-in-picture dock can show its title, jump back to its source message, or expand cleanly
/// into [`crate::image_viewer::ImageViewer`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InlineVideoMeta {
    pub file_id: String,
    pub title: String,
    pub channel: Option<ChannelId>,
    pub message_ts: Option<Ts>,
    pub url: String,
    pub ext: String,
    pub duration_ms: Option<u64>,
    pub poster_url: Option<String>,
    pub inline_transcript: Option<String>,
    pub vtt_url: Option<String>,
    pub download: Option<(String, String)>,
    pub user: Option<UserId>,
    pub taken_at: Option<i64>,
}

/// Snapshot rendered by the floating mini-player / picture-in-picture dock.
#[derive(Clone, Debug)]
pub struct PipVideoSnapshot {
    pub meta: InlineVideoMeta,
    pub snapshot: InlineVideoSnapshot,
}

/// Carried-over inline playback state when expanding a video into [`crate::image_viewer::ImageViewer`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InlineVideoHandoff {
    pub position_secs: f64,
    pub rate: f32,
}

struct ActiveInlineVideo {
    generation: u64,
    tick_gen: u64,
    file_id: String,
    meta: InlineVideoMeta,
    loading: bool,
    player: Option<NativeVideoPlayer>,
    last_frame: Option<Arc<VideoSurface>>,
    last_rendered_epoch: u64,
    inline_in_viewport: bool,
    pip_engaged: bool,
}

#[derive(Default)]
struct InlineVideoManager {
    next_generation: u64,
    frame_epoch: u64,
    active: Option<ActiveInlineVideo>,
    /// One-shot `(file_id, handoff)` stashed when expanding from inline playback into
    /// [`crate::image_viewer::ImageViewer`] so the viewer resumes at the exact timestamp and speed.
    pending_handoff: Option<(String, InlineVideoHandoff)>,
}

impl InlineVideoManager {
    fn next_gen(&mut self) -> u64 {
        self.next_generation = self.next_generation.wrapping_add(1);
        self.next_generation
    }

    fn stop_matching(&mut self, file_id: &str) -> Option<InlineVideoHandoff> {
        let matches = self
            .active
            .as_ref()
            .is_some_and(|active| active.file_id == file_id);
        if !matches {
            return None;
        }
        self.next_gen();
        let active = self.active.take()?;
        active_resume_handoff(&active)
    }

    fn stop_all(&mut self) -> Option<(String, InlineVideoHandoff)> {
        self.next_gen();
        let active = self.active.take()?;
        let handoff = active_resume_handoff(&active).unwrap_or(InlineVideoHandoff {
            position_secs: 0.0,
            rate: 1.0,
        });
        Some((active.file_id, handoff))
    }

    fn stash_handoff(&mut self, file_id: String, handoff: InlineVideoHandoff) {
        if (handoff.position_secs.is_finite() && handoff.position_secs > 0.05)
            || (handoff.rate - 1.0).abs() > 0.01
        {
            self.pending_handoff = Some((file_id, handoff));
        } else {
            self.pending_handoff = None;
        }
    }

    fn take_handoff(&mut self, file_id: &str) -> Option<InlineVideoHandoff> {
        if self
            .pending_handoff
            .as_ref()
            .is_some_and(|(id, _)| id == file_id)
        {
            self.pending_handoff.take().map(|(_, handoff)| handoff)
        } else {
            None
        }
    }
}

impl Global for InlineVideoManager {}

fn active_resume_handoff(active: &ActiveInlineVideo) -> Option<InlineVideoHandoff> {
    let player = active.player.as_ref()?;
    let st = player.state();
    let pos = if !st.ended && st.position_secs.is_finite() && st.position_secs > 0.05 {
        st.position_secs
    } else {
        0.0
    };
    let rate = if st.rate.is_finite() && st.rate > 0.0 {
        st.rate
    } else {
        1.0
    };
    if pos > 0.05 || (rate - 1.0).abs() > 0.01 {
        Some(InlineVideoHandoff {
            position_secs: pos,
            rate,
        })
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

fn snapshot_from_active(active: &ActiveInlineVideo) -> Option<InlineVideoSnapshot> {
    if active.loading {
        return Some(InlineVideoSnapshot {
            loading: true,
            playing: false,
            muted: false,
            ended: false,
            failed: false,
            position_secs: 0.0,
            duration_secs: active.meta.duration_ms.map_or(0.0, |ms| ms as f64 / 1000.0),
            rate: 1.0,
            width: 0,
            height: 0,
            surface: active.last_frame.clone(),
        });
    }
    let player = active.player.as_ref()?;
    let st = player.state();
    let surface = copy_player_surface(player).or_else(|| active.last_frame.clone());
    let rate = if st.rate.is_finite() && st.rate > 0.0 {
        st.rate
    } else {
        1.0
    };
    let dur = sanitize_secs(st.duration_secs);
    let duration_secs = if dur > 0.0 {
        dur
    } else {
        active.meta.duration_ms.map_or(0.0, |ms| ms as f64 / 1000.0)
    };
    Some(InlineVideoSnapshot {
        loading: false,
        playing: st.playing,
        muted: st.muted,
        ended: st.ended,
        failed: st.failed,
        position_secs: sanitize_secs(st.position_secs),
        duration_secs,
        rate,
        width: st.width,
        height: st.height,
        surface,
    })
}

/// Current inline video playback snapshot for `file_id`, if it is loading, playing, or paused.
pub fn inline_video_snapshot(file_id: &str, cx: &App) -> Option<InlineVideoSnapshot> {
    let active = cx.try_global::<InlineVideoManager>()?.active.as_ref()?;
    if active.file_id != file_id {
        return None;
    }
    snapshot_from_active(active)
}

/// Advance the per-frame visibility epoch at the start of `App::render` so unmounted inline video
/// cards are detected immediately on channel/page navigation or virtualized list scroll.
pub fn inline_video_begin_frame(cx: &mut App) {
    if cx.has_global::<InlineVideoManager>() {
        let mgr = cx.global_mut::<InlineVideoManager>();
        mgr.frame_epoch = mgr.frame_epoch.wrapping_add(1);
    }
}

/// Record that the inline video card for `file_id` rendered in the current frame.
pub fn mark_inline_video_rendered(file_id: &str, cx: &mut App) {
    if cx.has_global::<InlineVideoManager>() {
        let mgr = cx.global_mut::<InlineVideoManager>();
        let epoch = mgr.frame_epoch;
        if let Some(active) = mgr.active.as_mut().filter(|a| a.file_id == file_id) {
            active.last_rendered_epoch = epoch;
        }
    }
}

/// Update whether the rendered inline video card's painted bounds intersect the visible viewport.
/// Triggers a window refresh only on visibility transitions so scrolling past a mounted card opens
/// or dismisses the floating mini-player immediately.
pub fn update_inline_video_viewport_visibility(
    file_id: &str,
    in_viewport: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let changed = if cx.has_global::<InlineVideoManager>() {
        cx.global_mut::<InlineVideoManager>()
            .active
            .as_mut()
            .filter(|a| a.file_id == file_id)
            .is_some_and(|active| {
                let prev = active.inline_in_viewport;
                active.inline_in_viewport = in_viewport;
                prev != in_viewport
            })
    } else {
        false
    };
    if changed {
        window.refresh();
    }
}

/// Evaluate whether the floating mini-player / picture-in-picture dock should be shown for the
/// active inline video right now.
///
/// Engages when the active inline video is playing (or loading) and its inline card is scrolled out
/// of view or navigated away from; stays engaged while the user pauses inside the dock so they can
/// resume, expand, jump to the message, or close it; automatically hides when the inline card
/// scrolls back into view or when [`crate::image_viewer::ImageViewer`] opens.
pub fn active_pip_video(image_viewer_open: bool, cx: &mut App) -> Option<PipVideoSnapshot> {
    if !cx.has_global::<InlineVideoManager>() {
        return None;
    }
    let mgr = cx.global_mut::<InlineVideoManager>();
    let epoch = mgr.frame_epoch;
    let active = mgr.active.as_mut()?;
    if image_viewer_open {
        active.pip_engaged = false;
        return None;
    }
    let inline_visible = active.last_rendered_epoch == epoch && active.inline_in_viewport;
    if inline_visible {
        active.pip_engaged = false;
        return None;
    }
    let snap = snapshot_from_active(active)?;
    if snap.failed {
        active.pip_engaged = false;
        return None;
    }
    if snap.playing || snap.loading {
        active.pip_engaged = true;
    }
    if active.pip_engaged {
        Some(PipVideoSnapshot {
            meta: active.meta.clone(),
            snapshot: snap,
        })
    } else {
        None
    }
}

/// Stop whichever inline video is currently active (used by the floating mini-player close button).
pub fn stop_active_inline_video(window: &mut Window, cx: &mut App) {
    let had = if cx.has_global::<InlineVideoManager>() {
        cx.global_mut::<InlineVideoManager>().stop_all().is_some()
    } else {
        false
    };
    if had {
        window.refresh();
    }
}

/// Whether `url` points to an HLS playlist (`.m3u8`) that `AVPlayer` must stream directly via
/// [`NativeVideoPlayer::open_url`] instead of caching locally.
pub fn is_hls_url(url: &str) -> bool {
    let base = url.split(['?', '#']).next().unwrap_or(url);
    base.to_ascii_lowercase().ends_with(".m3u8")
}

/// Format a playback rate (`1.0`, `1.5`, `2.0`) as a compact button label (`1x`, `1.5x`, `2x`).
pub fn format_playback_rate(rate: f32) -> &'static str {
    if !rate.is_finite() || rate < 1.25 {
        "1x"
    } else if rate < 1.75 {
        "1.5x"
    } else {
        "2x"
    }
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
    play_inline_video_with_meta(
        InlineVideoMeta {
            title: file_id.clone(),
            file_id,
            url,
            ext,
            ..Default::default()
        },
        start_pos_secs,
        window,
        cx,
    );
}

/// Start native inline video playback with rich source metadata so the floating mini-player dock
/// can display its title, jump back to its message, or expand into [`crate::image_viewer::ImageViewer`].
pub fn play_inline_video_with_meta(
    meta: InlineVideoMeta,
    start_pos_secs: Option<f64>,
    window: &mut Window,
    cx: &mut App,
) {
    let file_id = meta.file_id.clone();
    let url = meta.url.clone();
    let ext = meta.ext.clone();
    let hls = is_hls_url(&url);
    let fetcher = if hls {
        active_fetcher(cx)
    } else {
        let Some(fetcher) = active_fetcher(cx) else {
            crate::actions::notify(
                "Video playback",
                "Playback needs a signed-in workspace.",
                window,
                cx,
            );
            return;
        };
        Some(fetcher)
    };
    let mgr = cx.default_global::<InlineVideoManager>();
    let generation = mgr.next_gen();
    let epoch = mgr.frame_epoch;
    mgr.pending_handoff = None;
    mgr.active = Some(ActiveInlineVideo {
        generation,
        tick_gen: 0,
        file_id: file_id.clone(),
        meta,
        loading: true,
        player: None,
        last_frame: None,
        last_rendered_epoch: epoch,
        inline_in_viewport: true,
        pip_engaged: false,
    });
    window.refresh();

    let cache_id = file_id.clone();
    let fetch_url = url.clone();
    let fetch_ext = ext.clone();
    window
        .spawn(cx, async move |cx| {
            let bg = cx.background_executor().clone();
            let open_res = if hls {
                bg.spawn(async move { NativeVideoPlayer::open_url(&fetch_url, None) })
                    .await
                    .map_err(|e| e.to_string())
            } else {
                let fetcher = fetcher.expect("fetcher validated for non-HLS media");
                let path =
                    fetch_cached_media(fetcher, cache_id, fetch_url, fetch_ext, bg.clone()).await;
                match path {
                    Ok(path) => bg
                        .spawn(async move { NativeVideoPlayer::open(&path) })
                        .await
                        .map_err(|e| e.to_string()),
                    Err(err) => Err(err),
                }
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
                            if !hls && !file_id.starts_with("bk-video-") {
                                transcribe_media(
                                    FileId::from(file_id.clone()),
                                    url,
                                    ext,
                                    true,
                                    window,
                                    cx,
                                );
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

/// Cycle playback speed (`1x -> 1.5x -> 2x -> 1x`) on the active inline video matching `file_id`.
pub fn cycle_inline_video_rate(file_id: &str, window: &mut Window, cx: &mut App) -> Option<f32> {
    let mgr = cx.default_global::<InlineVideoManager>();
    let active = mgr.active.as_mut().filter(|a| a.file_id == file_id)?;
    let player = active.player.as_ref()?;
    let rate = player.cycle_rate();
    window.refresh();
    Some(rate)
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
        .stop_matching(file_id)
        .filter(|h| h.position_secs > 0.05)
        .map(|h| h.position_secs);
    window.refresh();
    pos
}

/// Stop inline playback for `file_id` and stash its playback position and speed so opening
/// [`crate::image_viewer::ImageViewer`] immediately resumes cleanly.
pub fn stop_inline_video_for_expand(
    file_id: &str,
    window: &mut Window,
    cx: &mut App,
) -> Option<f64> {
    let mgr = cx.default_global::<InlineVideoManager>();
    let handoff = mgr.stop_matching(file_id);
    let pos = handoff
        .as_ref()
        .filter(|h| h.position_secs > 0.05)
        .map(|h| h.position_secs);
    if let Some(handoff) = handoff {
        mgr.stash_handoff(file_id.to_owned(), handoff);
    }
    window.refresh();
    pos
}

/// Called by [`crate::image_viewer::ImageViewer`] when starting playback of `file_id`: stops any
/// active inline video so audio never overlaps, and returns the carried-over timestamp & speed if
/// the same video was just playing inline (or expanded via [`stop_inline_video_for_expand`]).
pub(crate) fn take_inline_handoff_for_viewer(
    file_id: &str,
    window: &mut Window,
    cx: &mut App,
) -> Option<InlineVideoHandoff> {
    let mgr = cx.default_global::<InlineVideoManager>();
    let stashed = mgr.take_handoff(file_id);
    let stopped = mgr.stop_all();
    let had_active = stopped.is_some();
    let carry = stashed.or_else(|| {
        stopped
            .filter(|(id, h)| {
                id == file_id && (h.position_secs > 0.05 || (h.rate - 1.0).abs() > 0.01)
            })
            .map(|(_, h)| h)
    });
    if had_active {
        window.refresh();
    }
    carry
}

type DispatchTranscribeFn = Rc<dyn Fn(FileId, PathBuf, TranscriptionProvider, &mut App)>;
type PreferredProviderFn = Rc<dyn Fn(&App) -> TranscriptionProvider>;
type HasTranscriptFn = Rc<dyn Fn(&FileId, &App) -> bool>;

/// Global hook bridging media playback / card UI to the active workspace's
/// `CollabCommand::TranscribeMediaFile` pipeline without duplicating concurrent jobs.
#[derive(Default)]
pub struct TranscriptionDispatcher {
    dispatch: Option<DispatchTranscribeFn>,
    preferred_provider: Option<PreferredProviderFn>,
    has_transcript: Option<HasTranscriptFn>,
    in_progress: HashSet<FileId>,
    manual_requests: HashSet<FileId>,
}

impl Global for TranscriptionDispatcher {}

/// Install the active workspace transcription dispatch hooks (called once from `app/bootstrap.rs`).
pub fn install_transcription_dispatcher(
    dispatch: impl Fn(FileId, PathBuf, TranscriptionProvider, &mut App) + 'static,
    preferred_provider: impl Fn(&App) -> TranscriptionProvider + 'static,
    has_transcript: impl Fn(&FileId, &App) -> bool + 'static,
    cx: &mut App,
) {
    let state = cx.default_global::<TranscriptionDispatcher>();
    state.dispatch = Some(Rc::new(dispatch));
    state.preferred_provider = Some(Rc::new(preferred_provider));
    state.has_transcript = Some(Rc::new(has_transcript));
}

/// Whether `file_id` currently has an in-flight local transcription job.
pub fn is_transcribing(file_id: &FileId, cx: &App) -> bool {
    cx.try_global::<TranscriptionDispatcher>()
        .is_some_and(|d| d.in_progress.contains(file_id))
}

/// Clear `file_id` from the in-progress set when core emits `FileTranscriptReady` or
/// `FileTranscriptFailed`. Returns `true` if this transcription was explicitly triggered by the
/// user clicking "Transcribe" (vs. silent background auto-transcription on play).
pub fn finish_transcription(file_id: &FileId, cx: &mut App) -> bool {
    let state = cx.default_global::<TranscriptionDispatcher>();
    state.in_progress.remove(file_id);
    state.manual_requests.remove(file_id)
}

/// Download/cache `url` (instant cache hit if already played) and dispatch local STT transcription
/// for `file_id` via `CollabCommand::TranscribeMediaFile`.
///
/// - When `auto == true`: silent no-op if a transcript already exists, a job is already in flight,
///   or no local transcription model (`ParakeetV3` / `WhisperTurbo`) is downloaded yet.
/// - When `auto == false` (explicit button click): shows a toast guiding the user to download a
///   local model in Preferences → Audio & Video if none is ready.
pub fn transcribe_media(
    file_id: FileId,
    url: String,
    ext: String,
    auto: bool,
    window: &mut Window,
    cx: &mut App,
) {
    if is_hls_url(&url) {
        if !auto {
            crate::actions::notify(
                "Transcription unavailable",
                "HLS streams cannot be transcribed locally.",
                window,
                cx,
            );
        }
        return;
    }

    let (already_in_progress, dispatch_fn, provider_fn, has_tr_fn) = {
        let Some(state) = cx.try_global::<TranscriptionDispatcher>() else {
            return;
        };
        (
            state.in_progress.contains(&file_id),
            state.dispatch.clone(),
            state.preferred_provider.clone(),
            state.has_transcript.clone(),
        )
    };
    if already_in_progress {
        return;
    }
    let Some(dispatch_fn) = dispatch_fn else {
        return;
    };
    if auto && has_tr_fn.as_ref().is_some_and(|f| f(&file_id, cx)) {
        return;
    }

    let user_provider = provider_fn
        .as_ref()
        .map_or(TranscriptionProvider::SlackBuiltIn, |f| f(cx));
    let Some(provider) = resolve_local_file_provider(user_provider) else {
        if !auto {
            crate::actions::notify(
                "Local transcription model needed",
                "Download Parakeet v3 or Whisper Turbo in Preferences \u{2192} Audio & Video to transcribe videos locally.",
                window,
                cx,
            );
        }
        return;
    };

    let Some(fetcher) = active_fetcher(cx) else {
        if !auto {
            crate::actions::notify(
                "Transcription",
                "Transcription needs a signed-in workspace.",
                window,
                cx,
            );
        }
        return;
    };

    {
        let state = cx.default_global::<TranscriptionDispatcher>();
        state.in_progress.insert(file_id.clone());
        if !auto {
            state.manual_requests.insert(file_id.clone());
        }
    }
    window.refresh();

    let cache_id = file_id.to_string();
    window
        .spawn(cx, async move |cx| {
            let bg = cx.background_executor().clone();
            let fetched = fetch_cached_media(fetcher, cache_id, url, ext, bg).await;
            let _ = cx.update(|window, cx| match fetched {
                Ok(local_path) => {
                    dispatch_fn(file_id, local_path, provider, cx);
                }
                Err(err) => {
                    let was_manual = finish_transcription(&file_id, cx);
                    if was_manual {
                        crate::actions::notify("Transcription failed", err, window, cx);
                    }
                    window.refresh();
                }
            });
        })
        .detach();
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
pub(crate) fn seed_inline_video_for_test(
    meta: InlineVideoMeta,
    loading: bool,
    in_viewport: bool,
    cx: &mut App,
) {
    let mgr = cx.default_global::<InlineVideoManager>();
    let generation = mgr.next_gen();
    let epoch = mgr.frame_epoch;
    mgr.active = Some(ActiveInlineVideo {
        generation,
        tick_gen: 0,
        file_id: meta.file_id.clone(),
        meta,
        loading,
        player: None,
        last_frame: None,
        last_rendered_epoch: if in_viewport {
            epoch
        } else {
            epoch.wrapping_sub(1)
        },
        inline_in_viewport: in_viewport,
        pip_engaged: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_meta(file_id: &str) -> InlineVideoMeta {
        InlineVideoMeta {
            file_id: file_id.to_owned(),
            title: format!("{file_id}.mp4"),
            url: format!("https://files.slack.com/{file_id}.mp4"),
            ext: "mp4".into(),
            ..Default::default()
        }
    }

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
            meta: test_meta("F_VID_1"),
            loading: true,
            player: None,
            last_frame: None,
            last_rendered_epoch: 0,
            inline_in_viewport: true,
            pip_engaged: false,
        });
        assert_eq!(mgr.active.as_ref().unwrap().file_id, "F_VID_1");

        // Starting a second video replaces the first and advances generation.
        let g2 = mgr.next_gen();
        assert!(g2 > g1);
        mgr.active = Some(ActiveInlineVideo {
            generation: g2,
            tick_gen: 0,
            file_id: "F_VID_2".into(),
            meta: test_meta("F_VID_2"),
            loading: false,
            player: None,
            last_frame: None,
            last_rendered_epoch: 0,
            inline_in_viewport: true,
            pip_engaged: false,
        });
        assert_eq!(mgr.stop_matching("F_VID_1"), None);
        assert_eq!(mgr.active.as_ref().unwrap().file_id, "F_VID_2");

        // Stopping the matching video clears active state.
        assert_eq!(mgr.stop_matching("F_VID_2"), None);
        assert!(mgr.active.is_none());

        // One-shot handoff only matches its file_id and clears on take.
        mgr.stash_handoff(
            "F_VID_2".into(),
            InlineVideoHandoff {
                position_secs: 12.5,
                rate: 1.5,
            },
        );
        assert_eq!(mgr.take_handoff("F_OTHER"), None);
        assert_eq!(
            mgr.take_handoff("F_VID_2"),
            Some(InlineVideoHandoff {
                position_secs: 12.5,
                rate: 1.5,
            })
        );
        assert_eq!(mgr.take_handoff("F_VID_2"), None);

        // Near-zero timestamps at default 1.0x speed are ignored for handoff.
        mgr.stash_handoff(
            "F_VID_2".into(),
            InlineVideoHandoff {
                position_secs: 0.01,
                rate: 1.0,
            },
        );
        assert_eq!(mgr.take_handoff("F_VID_2"), None);

        // Rate formatting & HLS URL detection helpers.
        assert_eq!(format_playback_rate(1.0), "1x");
        assert_eq!(format_playback_rate(1.5), "1.5x");
        assert_eq!(format_playback_rate(2.0), "2x");
        assert_eq!(format_playback_rate(f32::NAN), "1x");
        assert!(is_hls_url(
            "https://files.slack.com/hls/clip/master.m3u8?t=1"
        ));
        assert!(is_hls_url("https://cdn.example.com/VIDEO.M3U8"));
        assert!(!is_hls_url("https://files.slack.com/pri/clip.mp4"));
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
            meta: test_meta("F_SYNTH"),
            loading: false,
            player: Some(player),
            last_frame: None,
            last_rendered_epoch: 0,
            inline_in_viewport: true,
            pip_engaged: false,
        });

        let stopped_pos = mgr.stop_matching("F_SYNTH");
        assert!(mgr.active.is_none());
        assert!(
            stopped_pos
                .as_ref()
                .is_some_and(|h| h.position_secs >= 0.25),
            "expected stopped position >= 0.25s, got {stopped_pos:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[gpui::test]
    fn pip_dock_engages_when_scrolled_or_navigated_away_and_clears_when_visible(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            inline_video_begin_frame(cx);
            seed_inline_video_for_test(test_meta("F_PIP"), true, true, cx);
            // Visible inline -> no PiP dock.
            assert!(active_pip_video(false, cx).is_none());

            // Advance frame without rendering the inline card (simulates navigating away / virtualized scroll out).
            inline_video_begin_frame(cx);
            let pip = active_pip_video(false, cx).expect("PiP must engage when inline card leaves");
            assert_eq!(pip.meta.file_id, "F_PIP");

            // Opening ImageViewer suppresses PiP.
            assert!(active_pip_video(true, cx).is_none());

            // Re-engage when viewer closes and card is still absent.
            let pip2 = active_pip_video(false, cx).expect("PiP re-engages while loading/playing");
            assert_eq!(pip2.meta.file_id, "F_PIP");

            // Returning to the message marks the card rendered -> PiP clears.
            inline_video_begin_frame(cx);
            mark_inline_video_rendered("F_PIP", cx);
            assert!(active_pip_video(false, cx).is_none());
        });
    }
}
