//! Local media seam: the [`MediaDevices`] effectors a huddle session drives (audio engine,
//! camera, `#content` screen share, jukebox) and their production implementation.

use std::sync::Arc;

use tokio::sync::mpsc;

use super::io::BoxFuture;
use crate::audio::{AmbientMusicPlayer, AudioEngine, AudioSharedState};
use crate::backend::{CameraHandle, VideoFrameSink};
use crate::content_share::ContentShareSession;
use crate::metrics::HuddlePerfCounters;
use crate::rtc::InboundAudioPacket;
use crate::video::start_camera_capture;
use crate::{HuddleAudioConfig, HuddleCredentials, HuddleError, Result};

/// Outcome of an audio device switch.
pub(super) enum AudioSwitch<A, I> {
    /// The requested devices are running.
    Switched(A),
    /// The requested devices failed with `err`; the previous devices are running again.
    Reverted { audio: A, err: HuddleError },
    /// Neither device set opened, or no inbound stream survived the stop: nothing is running.
    /// `revert_err` and `inbound` are `None` only in the latter case; `inbound` is parked for a
    /// later retry when present.
    Failed {
        err: HuddleError,
        revert_err: Option<HuddleError>,
        inbound: Option<I>,
    },
}

/// Result of a device operation the event loop moved off itself, delivered on `bg_done_tx`.
pub(super) enum BackgroundDone<M: MediaDevices> {
    AudioSwitch {
        new: HuddleAudioConfig,
        prev: HuddleAudioConfig,
        outcome: AudioSwitch<M::Audio, M::InboundAudio>,
    },
    /// Camera start for request `generation`; a result older than the latest request is dropped.
    CameraStart {
        generation: u64,
        result: Result<M::Camera>,
    },
    ScreenShareStart {
        generation: u64,
        source_id: String,
        result: Result<M::ScreenShare>,
    },
}

/// Local hardware and media-pipeline effectors owned by an active huddle session.
///
/// Audio and camera teardown/startup block on the OS for seconds, so every start/stop detaches
/// the hardware synchronously and does the blocking work on the blocking pool, resolving a
/// `'static` future; the connected event loop never holds the hardware while it waits.
pub(super) trait MediaDevices: 'static {
    /// Opaque inbound decoded-audio stream handle preserved across audio device hot-swaps.
    type InboundAudio: Send + 'static;
    /// Running audio hardware while it is installed.
    type Audio: Send + 'static;
    /// Running camera capture while it is installed.
    type Camera: Send + 'static;
    /// Running `#content` screen-share session while it is installed.
    type ScreenShare: Send + 'static;

    /// Stop audio and drop the hardware off the async worker thread.
    fn stop_audio(&mut self) -> BoxFuture<'static, ()>;

    /// Stop whatever audio is running and start it on `new`, falling back to `prev` when that
    /// fails (`parked` is the inbound stream of an earlier failed switch). Resolves with the
    /// hardware in its final state, not yet installed.
    fn switch_audio(
        &mut self,
        new: HuddleAudioConfig,
        prev: HuddleAudioConfig,
        parked: Option<Self::InboundAudio>,
    ) -> BoxFuture<'static, AudioSwitch<Self::Audio, Self::InboundAudio>>;

    fn install_audio(&mut self, audio: Self::Audio);

    /// Stop the running camera (if any) and start a new one; resolves with the capture, not yet
    /// installed.
    fn start_camera(
        &mut self,
        device_index: Option<usize>,
        blur: bool,
        compact: bool,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        on_preview: VideoFrameSink,
    ) -> BoxFuture<'static, Result<Self::Camera>>;

    fn install_camera(&mut self, camera: Self::Camera);

    /// Stop the running camera (if any).
    fn stop_camera(&mut self) -> BoxFuture<'static, ()>;

    fn set_camera_blur(&mut self, blur: bool);

    fn set_camera_compact(&mut self, compact: bool);

    fn force_camera_keyframe(&mut self);

    fn set_camera_target_bitrate(&mut self, bps: u32);

    /// Stop the running share (if any) and start sharing `source_id` (a whole second Chime
    /// handshake); resolves with the share, not yet installed.
    fn start_screen_share(
        &mut self,
        creds: HuddleCredentials,
        source_id: String,
        share_audio: bool,
        on_preview: VideoFrameSink,
        perf: Arc<HuddlePerfCounters>,
    ) -> BoxFuture<'static, Result<Self::ScreenShare>>;

    /// System-audio loopback device feeding the share, if any.
    fn screen_share_loopback(share: &Self::ScreenShare) -> Option<String>;

    fn install_screen_share(&mut self, share: Self::ScreenShare);

    /// Stop the running share (if any) and wait briefly for its `LEAVE` to flush.
    fn stop_screen_share(&mut self) -> BoxFuture<'static, ()>;

    /// Stop a detached screen-share handle (superseded or post-teardown) and wait briefly for its
    /// Chime `LEAVE` to flush.
    fn stop_detached_screen_share(share: Self::ScreenShare) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let _ = tokio::task::spawn_blocking(move || drop(share)).await;
        })
    }

    /// Drop a detached audio or camera handle off the Tokio async worker thread.
    fn drop_detached_audio(audio: Self::Audio) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let _ = tokio::task::spawn_blocking(move || drop(audio)).await;
        })
    }

    fn drop_detached_camera(camera: Self::Camera) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let _ = tokio::task::spawn_blocking(move || drop(camera)).await;
        })
    }

    fn set_jukebox(&mut self, output_device: Option<String>, song_key: Option<&str>);
}

/// Production hardware effectors wrapping CoreAudio/cpal, camera capture, `#content` screen share,
/// and ambient jukebox playback.
pub(super) struct LiveMediaDevices {
    audio_engine: Option<AudioEngine>,
    /// What an audio device switch needs to restart the engine.
    audio_shared: AudioSharedState,
    outbound_opus_tx: mpsc::UnboundedSender<bytes::Bytes>,
    camera_capture: Option<Box<dyn CameraHandle>>,
    content_share: Option<ContentShareSession>,
    music_player: Option<AmbientMusicPlayer>,
}

impl LiveMediaDevices {
    pub(super) fn new(
        audio_engine: AudioEngine,
        audio_shared: AudioSharedState,
        outbound_opus_tx: mpsc::UnboundedSender<bytes::Bytes>,
    ) -> Self {
        Self {
            audio_engine: Some(audio_engine),
            audio_shared,
            outbound_opus_tx,
            camera_capture: None,
            content_share: None,
            music_player: None,
        }
    }

    async fn stop_detached_share(share: Option<ContentShareSession>) {
        if let Some(mut share) = share {
            share.stop_and_wait().await;
        }
    }
}

impl MediaDevices for LiveMediaDevices {
    type InboundAudio = mpsc::UnboundedReceiver<InboundAudioPacket>;
    type Audio = AudioEngine;
    type Camera = Box<dyn CameraHandle>;
    type ScreenShare = ContentShareSession;

    fn stop_audio(&mut self) -> BoxFuture<'static, ()> {
        let engine = self.audio_engine.take();
        Box::pin(async move {
            if let Some(engine) = engine {
                let _ = tokio::task::spawn_blocking(move || drop(engine)).await;
            }
        })
    }

    fn switch_audio(
        &mut self,
        new: HuddleAudioConfig,
        prev: HuddleAudioConfig,
        parked: Option<Self::InboundAudio>,
    ) -> BoxFuture<'static, AudioSwitch<AudioEngine, Self::InboundAudio>> {
        let audio = self.audio_engine.take();
        let shared = self.audio_shared.clone();
        let outbound_opus_tx = self.outbound_opus_tx.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let inbound = audio.and_then(|mut engine| engine.stop()).or(parked);
                let Some(inbound) = inbound else {
                    return AudioSwitch::Failed {
                        err: HuddleError::Audio(
                            "inbound audio stream lost before device switch".into(),
                        ),
                        revert_err: None,
                        inbound: None,
                    };
                };
                match AudioEngine::start_recoverable(
                    &new,
                    shared.clone(),
                    outbound_opus_tx.clone(),
                    inbound,
                ) {
                    Ok(audio) => AudioSwitch::Switched(audio),
                    Err((err, inbound)) => {
                        match AudioEngine::start_recoverable(
                            &prev,
                            shared,
                            outbound_opus_tx,
                            inbound,
                        ) {
                            Ok(audio) => AudioSwitch::Reverted { audio, err },
                            Err((revert_err, inbound)) => AudioSwitch::Failed {
                                err,
                                revert_err: Some(revert_err),
                                inbound: Some(inbound),
                            },
                        }
                    }
                }
            })
            .await
            .unwrap_or_else(|e| AudioSwitch::Failed {
                err: HuddleError::Audio(format!("audio device switch task panicked: {e}")),
                revert_err: None,
                inbound: None,
            })
        })
    }

    fn install_audio(&mut self, audio: AudioEngine) {
        self.audio_engine = Some(audio);
    }

    fn start_camera(
        &mut self,
        device_index: Option<usize>,
        blur: bool,
        compact: bool,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        on_preview: VideoFrameSink,
    ) -> BoxFuture<'static, Result<Box<dyn CameraHandle>>> {
        let prev = self.camera_capture.take();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                drop(prev);
                start_camera_capture(device_index, blur, compact, outbound_h264_tx, on_preview)
            })
            .await
            .map_err(|e| HuddleError::Video(format!("camera start task panicked: {e}")))?
        })
    }

    fn install_camera(&mut self, camera: Box<dyn CameraHandle>) {
        self.camera_capture = Some(camera);
    }

    fn stop_camera(&mut self) -> BoxFuture<'static, ()> {
        let prev = self.camera_capture.take();
        Box::pin(async move {
            if prev.is_some() {
                let _ = tokio::task::spawn_blocking(move || drop(prev)).await;
            }
        })
    }

    fn set_camera_blur(&mut self, blur: bool) {
        if let Some(cap) = &self.camera_capture {
            cap.set_blur(blur);
        }
    }

    fn set_camera_compact(&mut self, compact: bool) {
        if let Some(cap) = &self.camera_capture {
            cap.set_compact(compact);
        }
    }

    fn force_camera_keyframe(&mut self) {
        if let Some(cap) = &self.camera_capture {
            cap.force_keyframe();
        }
    }

    fn set_camera_target_bitrate(&mut self, bps: u32) {
        if let Some(cap) = &self.camera_capture {
            cap.set_target_bitrate(bps);
        }
    }

    fn start_screen_share(
        &mut self,
        creds: HuddleCredentials,
        source_id: String,
        share_audio: bool,
        on_preview: VideoFrameSink,
        perf: Arc<HuddlePerfCounters>,
    ) -> BoxFuture<'static, Result<ContentShareSession>> {
        let prev = self.content_share.take();
        Box::pin(async move {
            Self::stop_detached_share(prev).await;
            ContentShareSession::start(creds, source_id, share_audio, on_preview, perf).await
        })
    }

    fn screen_share_loopback(share: &ContentShareSession) -> Option<String> {
        share.loopback_device().map(str::to_owned)
    }

    fn install_screen_share(&mut self, share: ContentShareSession) {
        self.content_share = Some(share);
    }

    fn stop_screen_share(&mut self) -> BoxFuture<'static, ()> {
        let share = self.content_share.take();
        Box::pin(Self::stop_detached_share(share))
    }

    fn stop_detached_screen_share(share: ContentShareSession) -> BoxFuture<'static, ()> {
        Box::pin(Self::stop_detached_share(Some(share)))
    }

    fn set_jukebox(&mut self, output_device: Option<String>, song_key: Option<&str>) {
        if let Some(mut prev) = self.music_player.take() {
            // Joining the music worker can wait for an in-flight CDN download: never block a
            // Tokio worker thread.
            tokio::task::spawn_blocking(move || prev.stop());
        }
        if song_key.is_some() {
            self.music_player = AmbientMusicPlayer::start(output_device, song_key).ok();
        }
    }
}
