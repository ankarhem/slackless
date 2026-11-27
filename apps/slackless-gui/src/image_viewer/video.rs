//! Native video playback inside the viewer: opening the player (a cached download through the
//! workspace fetcher, or HLS straight from the url), the frame tick that feeds the stage, the
//! transport and the transcript playhead, and the transport's controls.

use std::sync::Arc;

use gpui_kit::gpui::{Context, Window};
use slackless_core::huddle::NativeVideoPlayer;

use super::playback::{Playback, VideoPlayback};
use super::{ImageViewer, ViewerVideo};

/// Number of post-pause/seek ticks before the viewer's video timer idles at 0% CPU.
const VIEWER_SETTLE_TICKS: u32 = 15;

/// What the stage and the transport read off the player between ticks.
pub(super) struct VideoState {
    pub(super) loading: bool,
    pub(super) playing: bool,
    pub(super) muted: bool,
    pub(super) ended: bool,
    pub(super) position_secs: f64,
    pub(super) duration_secs: f64,
    pub(super) rate: f32,
    pub(super) volume: f32,
    pub(super) surface: Option<Arc<crate::media::VideoSurface>>,
    pub(super) error: Option<String>,
}

impl ImageViewer {
    /// The player's state as the stage and the transport show it: a video still loading reports
    /// the fallback duration and its error, no position.
    pub(super) fn video_state(&self) -> VideoState {
        let Some(pb) = self.video_playback() else {
            return VideoState {
                loading: false,
                playing: false,
                muted: false,
                ended: false,
                position_secs: 0.0,
                duration_secs: 0.0,
                rate: 1.0,
                volume: 1.0,
                surface: None,
                error: None,
            };
        };
        let Some(player) = pb.player.as_ref() else {
            return VideoState {
                loading: pb.loading,
                playing: false,
                muted: false,
                ended: false,
                position_secs: 0.0,
                duration_secs: pb.fallback_duration_secs,
                rate: 1.0,
                volume: 1.0,
                surface: pb.last_frame.clone(),
                error: pb.error.clone(),
            };
        };
        let st = player.state();
        let duration_secs = crate::media::sanitize_secs(st.duration_secs);
        VideoState {
            loading: pb.loading,
            playing: st.playing,
            muted: st.muted,
            ended: st.ended,
            position_secs: crate::media::sanitize_secs(st.position_secs),
            duration_secs: if duration_secs > 0.0 {
                duration_secs
            } else {
                pb.fallback_duration_secs
            },
            rate: if st.rate.is_finite() && st.rate > 0.0 {
                st.rate
            } else {
                1.0
            },
            volume: if st.volume.is_finite() {
                st.volume.clamp(0.0, 1.0)
            } else {
                1.0
            },
            surface: pb.last_frame.clone(),
            error: pb.error.clone(),
        }
    }

    /// Open `video`: an inline player expanding into the viewer hands over its position and
    /// rate; the player loads in the background under the loading overlay.
    pub(super) fn start_video(
        &mut self,
        video: ViewerVideo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let generation = self.media_gen;
        let carry = crate::media::take_inline_handoff_for_viewer(&video.file_id, window, cx);
        let fallback_duration_secs = video.duration_ms.map_or(0.0, |ms| ms as f64 / 1000.0);
        let hls = crate::media::is_hls_url(&video.url);

        let fetcher = if hls {
            crate::media::active_fetcher(cx)
        } else {
            let Some(fetcher) = crate::media::active_fetcher(cx) else {
                self.playback = Some(Playback::Video(VideoPlayback {
                    loading: false,
                    player: None,
                    last_frame: None,
                    error: Some("Playback needs a signed-in workspace.".to_owned()),
                    fallback_duration_secs,
                    _load_task: None,
                    tick_task: None,
                }));
                cx.notify();
                return;
            };
            Some(fetcher)
        };

        let file_id = video.file_id;
        let url = video.url;
        let ext = video.ext;
        let fetch_url = url.clone();
        let fetch_ext = ext.clone();
        let cache_id = file_id.clone();
        let load_task = cx.spawn_in(window, async move |weak, cx| {
            let bg = cx.background_executor().clone();
            let open_res = if hls {
                let cookie = fetcher
                    .as_ref()
                    .and_then(|f| f.file_cookie_header(&fetch_url));
                bg.spawn(async move { NativeVideoPlayer::open_url(&fetch_url, cookie.as_deref()) })
                    .await
                    .map_err(|e| e.to_string())
            } else {
                let fetcher = fetcher.expect("fetcher validated for non-HLS media");
                let path = crate::media::fetch_cached_media(
                    fetcher,
                    cache_id,
                    fetch_url,
                    fetch_ext,
                    bg.clone(),
                )
                .await;
                match path {
                    Ok(path) => bg
                        .spawn(async move { NativeVideoPlayer::open(&path) })
                        .await
                        .map_err(|e| e.to_string()),
                    Err(err) => Err(err),
                }
            };
            let _ = weak.update_in(cx, |viewer, window, cx| {
                if viewer.media_gen != generation {
                    drop(open_res);
                    return;
                }
                match open_res {
                    Ok(player) => {
                        if let Some(handoff) = carry {
                            if (handoff.rate - 1.0).abs() > 0.01 {
                                player.set_rate(handoff.rate);
                            }
                            if handoff.position_secs.is_finite() && handoff.position_secs > 0.05 {
                                player.seek(handoff.position_secs);
                            }
                        }
                        player.play();
                        let initial_frame = crate::media::copy_player_surface(&player);
                        if let Some(Playback::Video(pb)) = viewer.playback.as_mut() {
                            pb.loading = false;
                            pb.error = None;
                            pb.last_frame = initial_frame;
                            pb.player = Some(player);
                        }
                        if !hls && !file_id.starts_with("bk-video-") {
                            crate::media::transcribe_media(
                                slackless_core::types::FileId::from(file_id.clone()),
                                url,
                                ext,
                                true,
                                window,
                                cx,
                            );
                        }
                        viewer.spawn_video_tick_loop(window, cx);
                        cx.notify();
                    }
                    Err(err) => {
                        if let Some(Playback::Video(pb)) = viewer.playback.as_mut() {
                            pb.loading = false;
                            pb.error = Some(err);
                        }
                        cx.notify();
                    }
                }
            });
        });
        self.playback = Some(Playback::Video(VideoPlayback {
            loading: true,
            player: None,
            last_frame: None,
            error: None,
            fallback_duration_secs,
            _load_task: Some(load_task),
            tick_task: None,
        }));
        cx.notify();
    }

    /// Tick the player every frame while it plays (and a few ticks after a pause or a seek),
    /// copying its surface into the stage and its playhead into the transcript drawer.
    pub(super) fn spawn_video_tick_loop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let generation = self.media_gen;
        let tick_task = cx.spawn_in(window, async move |weak, cx| {
            let mut idle_ticks = 0u32;
            loop {
                cx.background_executor()
                    .timer(crate::media::VIDEO_FRAME_INTERVAL)
                    .await;
                let keep_going = weak
                    .update(cx, |viewer, cx| {
                        if viewer.media_gen != generation {
                            return false;
                        }
                        let Some(Playback::Video(pb)) = viewer.playback.as_mut() else {
                            return false;
                        };
                        let Some(player) = pb.player.as_ref() else {
                            return false;
                        };
                        if let Some(frame) = crate::media::copy_player_surface(player) {
                            pb.last_frame = Some(frame);
                        }
                        let st = player.state();
                        if st.failed {
                            pb.error = Some("Video playback failed.".to_owned());
                        }
                        // The transport bar follows every tick; the transcript only repaints
                        // when the cue under the playhead changes.
                        if let Some((transcript, _)) = &viewer.transcript {
                            let position = crate::media::sanitize_secs(st.position_secs);
                            transcript.update(cx, |transcript, cx| {
                                transcript.set_playhead(position, st.playing, cx);
                            });
                        }
                        cx.notify();
                        if st.failed {
                            return false;
                        }
                        if st.playing && !st.ended {
                            idle_ticks = 0;
                            true
                        } else {
                            idle_ticks += 1;
                            idle_ticks < VIEWER_SETTLE_TICKS
                        }
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        });
        if let Some(Playback::Video(pb)) = self.playback.as_mut() {
            pb.tick_task = Some(tick_task);
        }
    }

    /// Toggle play/pause on the viewer's currently open video.
    pub fn toggle_video_play(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Playback::Video(pb)) = self.playback.as_mut() else {
            return;
        };
        let Some(player) = pb.player.as_ref() else {
            return;
        };
        player.toggle_play();
        if let Some(frame) = crate::media::copy_player_surface(player) {
            pb.last_frame = Some(frame);
        }
        self.spawn_video_tick_loop(window, cx);
        cx.notify();
    }

    /// Toggle mute/unmute on the viewer's currently open video.
    pub fn toggle_video_mute(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self.video_playback().and_then(|pb| pb.player.as_ref()) else {
            return;
        };
        let muted = !player.state().muted;
        player.set_muted(muted);
        cx.notify();
    }

    /// Adjust playback volume by `delta` (`-1.0..=1.0`, e.g. `±0.1`) on the viewer's open video.
    pub fn adjust_video_volume(&mut self, delta: f32, cx: &mut Context<Self>) {
        let Some(player) = self.video_playback().and_then(|pb| pb.player.as_ref()) else {
            return;
        };
        player.adjust_volume(delta);
        cx.notify();
    }

    /// Cycle playback speed (`1x -> 1.5x -> 2x -> 1x`) on the viewer's currently open video.
    pub fn cycle_video_rate(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self.video_playback().and_then(|pb| pb.player.as_ref()) else {
            return;
        };
        player.cycle_rate();
        cx.notify();
    }

    /// Seek the viewer's currently open video relative to its current position by `delta_secs`.
    pub fn seek_video_relative(
        &mut self,
        delta_secs: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cur) = self
            .video_playback()
            .and_then(|pb| pb.player.as_ref())
            .map(|p| crate::media::sanitize_secs(p.state().position_secs))
        else {
            return;
        };
        self.seek_video((cur + delta_secs).max(0.0), window, cx);
    }

    /// Seek the viewer's currently open video to `secs`.
    pub fn seek_video(&mut self, secs: f64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Playback::Video(pb)) = self.playback.as_mut() else {
            return;
        };
        let Some(player) = pb.player.as_ref() else {
            return;
        };
        let duration = crate::media::sanitize_secs(player.state().duration_secs);
        let target = if duration > 0.0 {
            crate::media::sanitize_secs(secs).min(duration)
        } else {
            crate::media::sanitize_secs(secs)
        };
        player.seek(target);
        if let Some(frame) = crate::media::copy_player_surface(player) {
            pb.last_frame = Some(frame);
        }
        self.spawn_video_tick_loop(window, cx);
        cx.notify();
    }
}
