//! Trait seams separating the huddle session state machine (`controls`, `signaling_handlers`)
//! from the concrete Chime signaling WebSocket and WebRTC peer, plus the factory that creates them
//! together with the local media of [`super::media`], so the session protocol and its recovery
//! logic can be tested deterministically in-process.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use tokio::sync::mpsc;
use webrtc::peer_connection::RTCPeerConnectionState;

use super::media::{LiveMediaDevices, MediaDevices};
use crate::audio::{AudioEngine, AudioSharedState};
use crate::proto::{
    SdkJoinAckFrame, SdkPingPongFrame, SdkSignalFrame, SdkSubscribeAckFrame, SdkTurnCredentials,
};
use crate::rtc::{
    ChimeRtcPeer, InboundAudioPacket, InboundVideoPacket, OpusTrackSender, VideoSenderFeedback,
    VideoTrackSender,
};
use crate::signaling::{ChimeSignalClient, ClientMetricsSnapshot};
use crate::{HuddleAudioConfig, HuddleCredentials, Result};

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

    /// Close the signaling transport without sending a Chime `LEAVE` frame (used on reconnect).
    fn close<'a>(self) -> BoxFuture<'a, ()>
    where
        Self: 'a,
    {
        Box::pin(async {})
    }
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

    fn close<'a>(self) -> BoxFuture<'a, ()>
    where
        Self: 'a,
    {
        Box::pin(ChimeSignalClient::close(self))
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
