//! Trait seams separating the huddle session state machine (`controls`, `signaling_handlers`)
//! from concrete WebSocket, WebRTC, and OS audio/video/screen-capture hardware so the entire
//! session protocol and device-recovery logic can be tested deterministically in-process.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use webrtc::peer_connection::RTCPeerConnectionState;

use crate::audio::{AmbientMusicPlayer, AudioEngine, AudioSharedState};
use crate::backend::CameraHandle;
use crate::content_share::ContentShareSession;
use crate::metrics::HuddlePerfCounters;
use crate::proto::{
    SdkJoinAckFrame, SdkPingPongFrame, SdkSignalFrame, SdkSubscribeAckFrame, SdkTurnCredentials,
};
use crate::rtc::{
    ChimeRtcPeer, InboundAudioPacket, InboundVideoPacket, OpusTrackSender, VideoSenderFeedback,
    VideoTrackSender,
};
use crate::signaling::{ChimeSignalClient, ClientMetricsSnapshot};
use crate::video::{VideoFrame, start_camera_capture};
use crate::{HuddleAudioConfig, HuddleCredentials, HuddleError, Result};

/// Boxed `Send` future alias keeping trait signatures concise for `clippy::type_complexity`.
pub(super) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Outbound Chime signaling sink used by session handlers.
pub(super) trait SignalSink {
    fn send_frame<'a>(&'a mut self, frame: &'a SdkSignalFrame) -> BoxFuture<'a, Result<()>>;

    fn reply_ping_if_needed<'a>(&'a mut self, pp: &'a SdkPingPongFrame) -> BoxFuture<'a, bool>;
}

/// Full Chime signaling session used by handshake and the connected event loop.
pub(super) trait SignalSession: SignalSink + Sized {
    fn next_frame<'a>(&'a mut self) -> BoxFuture<'a, Result<Option<SdkSignalFrame>>>;

    fn wait_for_join_ack<'a>(
        &'a mut self,
        timeout: Duration,
        on_other: &'a mut (dyn FnMut(&mut SdkSignalFrame) + Send),
    ) -> BoxFuture<'a, Result<SdkJoinAckFrame>>;

    fn wait_for_subscribe_ack<'a>(
        &'a mut self,
        timeout: Duration,
        on_other: &'a mut (dyn FnMut(&mut SdkSignalFrame) + Send),
    ) -> BoxFuture<'a, Result<SdkSubscribeAckFrame>>;

    fn send_leave_and_close<'a>(self, drain_timeout: Duration) -> BoxFuture<'a, ()>
    where
        Self: 'a;
}

impl SignalSink for ChimeSignalClient {
    fn send_frame<'a>(&'a mut self, frame: &'a SdkSignalFrame) -> BoxFuture<'a, Result<()>> {
        Box::pin(ChimeSignalClient::send_frame(self, frame))
    }

    fn reply_ping_if_needed<'a>(&'a mut self, pp: &'a SdkPingPongFrame) -> BoxFuture<'a, bool> {
        Box::pin(ChimeSignalClient::reply_ping_if_needed(self, pp))
    }
}

impl SignalSession for ChimeSignalClient {
    fn next_frame<'a>(&'a mut self) -> BoxFuture<'a, Result<Option<SdkSignalFrame>>> {
        Box::pin(ChimeSignalClient::next_frame(self))
    }

    fn wait_for_join_ack<'a>(
        &'a mut self,
        timeout: Duration,
        on_other: &'a mut (dyn FnMut(&mut SdkSignalFrame) + Send),
    ) -> BoxFuture<'a, Result<SdkJoinAckFrame>> {
        Box::pin(ChimeSignalClient::wait_for_join_ack(
            self, timeout, on_other,
        ))
    }

    fn wait_for_subscribe_ack<'a>(
        &'a mut self,
        timeout: Duration,
        on_other: &'a mut (dyn FnMut(&mut SdkSignalFrame) + Send),
    ) -> BoxFuture<'a, Result<SdkSubscribeAckFrame>> {
        Box::pin(ChimeSignalClient::wait_for_subscribe_ack(
            self, timeout, on_other,
        ))
    }

    fn send_leave_and_close<'a>(self, drain_timeout: Duration) -> BoxFuture<'a, ()>
    where
        Self: 'a,
    {
        Box::pin(ChimeSignalClient::send_leave_and_close(self, drain_timeout))
    }
}

/// Dedicated outbound H.264 RTP track sender used by the connected loop's background video task.
pub(super) trait VideoTrackOut: Send + 'static {
    fn write_h264_frame<'a>(
        &'a self,
        annex_b: Vec<u8>,
        duration: Duration,
    ) -> BoxFuture<'a, Result<()>>;
}

impl VideoTrackOut for VideoTrackSender {
    fn write_h264_frame<'a>(
        &'a self,
        annex_b: Vec<u8>,
        duration: Duration,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(VideoTrackSender::write_h264_frame(self, annex_b, duration))
    }
}

/// Dedicated outbound Opus RTP track writer used by the connected loop's background audio task.
pub(super) trait OpusTrackOut: Send + 'static {
    fn write_opus_frame<'a>(&'a self, opus_bytes: bytes::Bytes) -> BoxFuture<'a, Result<()>>;
}

impl OpusTrackOut for OpusTrackSender {
    fn write_opus_frame<'a>(&'a self, opus_bytes: bytes::Bytes) -> BoxFuture<'a, Result<()>> {
        Box::pin(OpusTrackSender::write_opus_frame(self, opus_bytes))
    }
}

/// WebRTC peer-connection operations needed by session control and signaling handlers.
pub(super) trait RtcController {
    fn set_remote_answer<'a>(&'a self, answer: String) -> BoxFuture<'a, Result<()>>;

    fn create_gathered_offer<'a>(&'a self) -> BoxFuture<'a, Result<String>>;

    fn prepare_video_receive_offer<'a>(
        &'a self,
        count: usize,
    ) -> BoxFuture<'a, Result<(String, Vec<String>)>>;
}

/// Full WebRTC peer session used by handshake and the connected event loop.
pub(super) trait RtcSession: RtcController + Sync {
    type VideoSender: VideoTrackOut;
    type OpusSender: OpusTrackOut;

    fn attach_video_feedback<'a>(
        &'a self,
        tx: mpsc::UnboundedSender<VideoSenderFeedback>,
    ) -> BoxFuture<'a, ()>;

    fn video_sender(&self) -> Self::VideoSender;

    fn opus_sender(&self) -> Self::OpusSender;

    fn request_keyframe<'a>(&'a self, ssrc: u32) -> BoxFuture<'a, ()>;

    fn connection_state(&self) -> RTCPeerConnectionState;

    fn total_packets_sent(&self) -> u64;

    fn total_packets_received(&self) -> u64;

    fn total_bytes_sent(&self) -> u64;

    fn total_bytes_received(&self) -> u64;

    fn take_metrics_snapshot(&self, local_rms: f32) -> ClientMetricsSnapshot;

    fn close<'a>(&'a self) -> BoxFuture<'a, ()>;
}

impl RtcController for ChimeRtcPeer {
    fn set_remote_answer<'a>(&'a self, answer: String) -> BoxFuture<'a, Result<()>> {
        Box::pin(ChimeRtcPeer::set_remote_answer(self, answer))
    }

    fn create_gathered_offer<'a>(&'a self) -> BoxFuture<'a, Result<String>> {
        Box::pin(ChimeRtcPeer::create_gathered_offer(self))
    }

    fn prepare_video_receive_offer<'a>(
        &'a self,
        count: usize,
    ) -> BoxFuture<'a, Result<(String, Vec<String>)>> {
        Box::pin(ChimeRtcPeer::prepare_video_receive_offer(self, count))
    }
}

impl RtcSession for ChimeRtcPeer {
    type VideoSender = VideoTrackSender;
    type OpusSender = OpusTrackSender;

    fn attach_video_feedback<'a>(
        &'a self,
        tx: mpsc::UnboundedSender<VideoSenderFeedback>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(ChimeRtcPeer::attach_video_feedback(self, tx))
    }

    fn video_sender(&self) -> Self::VideoSender {
        ChimeRtcPeer::video_sender(self)
    }

    fn opus_sender(&self) -> Self::OpusSender {
        ChimeRtcPeer::opus_sender(self)
    }

    fn request_keyframe<'a>(&'a self, ssrc: u32) -> BoxFuture<'a, ()> {
        Box::pin(ChimeRtcPeer::request_keyframe(self, ssrc))
    }

    fn connection_state(&self) -> RTCPeerConnectionState {
        ChimeRtcPeer::connection_state(self)
    }

    fn total_packets_sent(&self) -> u64 {
        ChimeRtcPeer::total_packets_sent(self)
    }

    fn total_packets_received(&self) -> u64 {
        ChimeRtcPeer::total_packets_received(self)
    }

    fn total_bytes_sent(&self) -> u64 {
        ChimeRtcPeer::total_bytes_sent(self)
    }

    fn total_bytes_received(&self) -> u64 {
        ChimeRtcPeer::total_bytes_received(self)
    }

    fn take_metrics_snapshot(&self, local_rms: f32) -> ClientMetricsSnapshot {
        ChimeRtcPeer::take_metrics_snapshot(self, local_rms)
    }

    fn close<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(ChimeRtcPeer::close(self))
    }
}

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

    /// Stop audio and drop the hardware (blocking); for teardown before the event loop runs.
    fn stop_audio(&mut self);

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
        local_cam_tx: mpsc::UnboundedSender<VideoFrame>,
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
        local_screen_tx: mpsc::UnboundedSender<VideoFrame>,
        perf: Arc<HuddlePerfCounters>,
    ) -> BoxFuture<'static, Result<Self::ScreenShare>>;

    /// System-audio loopback device feeding the share, if any.
    fn screen_share_loopback(share: &Self::ScreenShare) -> Option<String>;

    fn install_screen_share(&mut self, share: Self::ScreenShare);

    /// Stop the running share (if any) and wait briefly for its `LEAVE` to flush.
    fn stop_screen_share(&mut self) -> BoxFuture<'static, ()>;

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
}

impl MediaDevices for LiveMediaDevices {
    type InboundAudio = mpsc::UnboundedReceiver<InboundAudioPacket>;
    type Audio = AudioEngine;
    type Camera = Box<dyn CameraHandle>;
    type ScreenShare = ContentShareSession;

    fn stop_audio(&mut self) {
        drop(self.audio_engine.take());
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
        local_cam_tx: mpsc::UnboundedSender<VideoFrame>,
    ) -> BoxFuture<'static, Result<Box<dyn CameraHandle>>> {
        let prev = self.camera_capture.take();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                drop(prev);
                start_camera_capture(device_index, blur, compact, outbound_h264_tx, local_cam_tx)
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
        local_screen_tx: mpsc::UnboundedSender<VideoFrame>,
        perf: Arc<HuddlePerfCounters>,
    ) -> BoxFuture<'static, Result<ContentShareSession>> {
        let prev = self.content_share.take();
        Box::pin(async move {
            Self::stop_detached_share(prev).await;
            ContentShareSession::start(creds, source_id, share_audio, local_screen_tx, perf).await
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

    fn set_jukebox(&mut self, output_device: Option<String>, song_key: Option<&str>) {
        if let Some(mut prev) = self.music_player.take() {
            prev.stop();
        }
        if song_key.is_some() {
            self.music_player = AmbientMusicPlayer::start(output_device, song_key).ok();
        }
    }
}

impl LiveMediaDevices {
    async fn stop_detached_share(share: Option<ContentShareSession>) {
        if let Some(mut share) = share {
            share.stop_and_wait().await;
        }
    }
}

/// Factory creating signaling, WebRTC, and local audio hardware for a huddle session attempt.
pub(super) trait SessionFactory: Send {
    type Signal: SignalSession + Send + 'static;
    type Rtc: RtcSession + Send + Sync + 'static;
    type Media: MediaDevices + Send + 'static;

    fn connect_signal<'a>(
        &'a mut self,
        creds: &'a HuddleCredentials,
    ) -> BoxFuture<'a, Result<Self::Signal>>;

    fn spawn_audio_start(
        &mut self,
        config: HuddleAudioConfig,
        shared: AudioSharedState,
        outbound_opus_tx: mpsc::UnboundedSender<bytes::Bytes>,
        inbound_opus_rx: mpsc::UnboundedReceiver<InboundAudioPacket>,
    ) -> tokio::task::JoinHandle<Result<Self::Media>>;

    fn create_rtc<'a>(
        &'a mut self,
        turn: &'a SdkTurnCredentials,
        inbound_opus_tx: mpsc::UnboundedSender<InboundAudioPacket>,
        inbound_video_tx: mpsc::UnboundedSender<InboundVideoPacket>,
        pc_state_tx: mpsc::UnboundedSender<RTCPeerConnectionState>,
    ) -> BoxFuture<'a, Result<Self::Rtc>>;
}

/// Production factory wiring real `ChimeSignalClient`, `ChimeRtcPeer`, and `AudioEngine`.
pub(super) struct LiveSessionFactory;

impl SessionFactory for LiveSessionFactory {
    type Signal = ChimeSignalClient;
    type Rtc = ChimeRtcPeer;
    type Media = LiveMediaDevices;

    fn connect_signal<'a>(
        &'a mut self,
        creds: &'a HuddleCredentials,
    ) -> BoxFuture<'a, Result<Self::Signal>> {
        Box::pin(ChimeSignalClient::connect(creds))
    }

    fn spawn_audio_start(
        &mut self,
        config: HuddleAudioConfig,
        shared: AudioSharedState,
        outbound_opus_tx: mpsc::UnboundedSender<bytes::Bytes>,
        inbound_opus_rx: mpsc::UnboundedReceiver<InboundAudioPacket>,
    ) -> tokio::task::JoinHandle<Result<Self::Media>> {
        tokio::task::spawn_blocking(move || {
            let engine = AudioEngine::start(
                &config,
                shared.clone(),
                outbound_opus_tx.clone(),
                inbound_opus_rx,
            )?;
            Ok(LiveMediaDevices::new(engine, shared, outbound_opus_tx))
        })
    }

    fn create_rtc<'a>(
        &'a mut self,
        turn: &'a SdkTurnCredentials,
        inbound_opus_tx: mpsc::UnboundedSender<InboundAudioPacket>,
        inbound_video_tx: mpsc::UnboundedSender<InboundVideoPacket>,
        pc_state_tx: mpsc::UnboundedSender<RTCPeerConnectionState>,
    ) -> BoxFuture<'a, Result<Self::Rtc>> {
        Box::pin(ChimeRtcPeer::new(
            turn,
            inbound_opus_tx,
            inbound_video_tx,
            pc_state_tx,
        ))
    }
}
