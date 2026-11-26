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
use crate::content_share::ContentShareSession;
use crate::metrics::HuddlePerfCounters;
use crate::proto::{
    SdkJoinAckFrame, SdkPingPongFrame, SdkSignalFrame, SdkSubscribeAckFrame, SdkTurnCredentials,
};
use crate::rtc::{
    ChimeRtcPeer, InboundAudioPacket, InboundVideoPacket, VideoSenderFeedback, VideoTrackSender,
};
use crate::signaling::{ChimeSignalClient, ClientMetricsSnapshot};
use crate::video::{CameraCapture, VideoFrame};
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

    fn attach_video_feedback<'a>(
        &'a self,
        tx: mpsc::UnboundedSender<VideoSenderFeedback>,
    ) -> BoxFuture<'a, ()>;

    fn video_sender(&self) -> Self::VideoSender;

    fn write_opus_frame<'a>(&'a self, opus_bytes: bytes::Bytes) -> BoxFuture<'a, Result<()>>;

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

    fn attach_video_feedback<'a>(
        &'a self,
        tx: mpsc::UnboundedSender<VideoSenderFeedback>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(ChimeRtcPeer::attach_video_feedback(self, tx))
    }

    fn video_sender(&self) -> Self::VideoSender {
        ChimeRtcPeer::video_sender(self)
    }

    fn write_opus_frame<'a>(&'a self, opus_bytes: bytes::Bytes) -> BoxFuture<'a, Result<()>> {
        Box::pin(ChimeRtcPeer::write_opus_frame(self, opus_bytes))
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

/// Local hardware and media-pipeline effectors owned by an active huddle session.
pub(super) trait MediaDevices {
    /// Opaque inbound decoded-audio stream handle preserved across audio device hot-swaps.
    type InboundAudio;

    fn stop_audio(&mut self) -> Option<Self::InboundAudio>;

    fn start_audio_recoverable(
        &mut self,
        config: &HuddleAudioConfig,
        shared: AudioSharedState,
        outbound_opus_tx: mpsc::UnboundedSender<bytes::Bytes>,
        inbound_rx: Self::InboundAudio,
    ) -> std::result::Result<(), (HuddleError, Self::InboundAudio)>;

    fn start_camera(
        &mut self,
        device_index: Option<usize>,
        blur: bool,
        compact: bool,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        local_cam_tx: mpsc::UnboundedSender<VideoFrame>,
    ) -> Result<()>;

    /// Stops the active camera if running; returns `true` if one was active.
    fn stop_camera(&mut self) -> bool;

    fn set_camera_blur(&mut self, blur: bool);

    fn set_camera_compact(&mut self, compact: bool);

    fn force_camera_keyframe(&mut self);

    fn set_camera_target_bitrate(&mut self, bps: u32);

    fn start_screen_share<'a>(
        &'a mut self,
        creds: HuddleCredentials,
        source_id: String,
        share_audio: bool,
        local_screen_tx: mpsc::UnboundedSender<VideoFrame>,
        perf: Arc<HuddlePerfCounters>,
    ) -> BoxFuture<'a, Result<Option<String>>>;

    fn stop_screen_share<'a>(&'a mut self) -> BoxFuture<'a, ()>;

    fn set_jukebox(&mut self, output_device: Option<String>, song_key: Option<&str>);
}

/// Production hardware effectors wrapping CoreAudio/cpal, camera capture, `#content` screen share,
/// and ambient jukebox playback.
pub(super) struct LiveMediaDevices {
    pub(super) audio_engine: AudioEngine,
    pub(super) camera_capture: Option<CameraCapture>,
    pub(super) content_share: Option<ContentShareSession>,
    pub(super) music_player: Option<AmbientMusicPlayer>,
}

impl LiveMediaDevices {
    pub(super) fn new(audio_engine: AudioEngine) -> Self {
        Self {
            audio_engine,
            camera_capture: None,
            content_share: None,
            music_player: None,
        }
    }
}

impl MediaDevices for LiveMediaDevices {
    type InboundAudio = mpsc::UnboundedReceiver<InboundAudioPacket>;

    fn stop_audio(&mut self) -> Option<Self::InboundAudio> {
        self.audio_engine.stop()
    }

    fn start_audio_recoverable(
        &mut self,
        config: &HuddleAudioConfig,
        shared: AudioSharedState,
        outbound_opus_tx: mpsc::UnboundedSender<bytes::Bytes>,
        inbound_rx: Self::InboundAudio,
    ) -> std::result::Result<(), (HuddleError, Self::InboundAudio)> {
        match AudioEngine::start_recoverable(config, shared, outbound_opus_tx, inbound_rx) {
            Ok(restarted) => {
                self.audio_engine = restarted;
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn start_camera(
        &mut self,
        device_index: Option<usize>,
        blur: bool,
        compact: bool,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        local_cam_tx: mpsc::UnboundedSender<VideoFrame>,
    ) -> Result<()> {
        if let Some(mut prev) = self.camera_capture.take() {
            prev.stop();
        }
        let cap =
            CameraCapture::start(device_index, blur, compact, outbound_h264_tx, local_cam_tx)?;
        self.camera_capture = Some(cap);
        Ok(())
    }

    fn stop_camera(&mut self) -> bool {
        if let Some(mut cap) = self.camera_capture.take() {
            cap.stop();
            true
        } else {
            false
        }
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

    fn start_screen_share<'a>(
        &'a mut self,
        creds: HuddleCredentials,
        source_id: String,
        share_audio: bool,
        local_screen_tx: mpsc::UnboundedSender<VideoFrame>,
        perf: Arc<HuddlePerfCounters>,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(async move {
            let cs =
                ContentShareSession::start(creds, source_id, share_audio, local_screen_tx, perf)
                    .await?;
            let loopback = cs.loopback_device().map(ToOwned::to_owned);
            self.content_share = Some(cs);
            Ok(loopback)
        })
    }

    fn stop_screen_share<'a>(&'a mut self) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Some(mut prev) = self.content_share.take() {
                prev.stop_and_wait().await;
            }
        })
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
            AudioEngine::start(&config, shared, outbound_opus_tx, inbound_opus_rx)
                .map(LiveMediaDevices::new)
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
