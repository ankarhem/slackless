use std::collections::HashMap;
use std::time::Duration;

use rand::RngExt as _;
use tokio::sync::mpsc;
use webrtc::peer_connection::RTCPeerConnectionState;

use super::io::{
    MediaDevices, RtcController, RtcSession, SessionFactory, SignalSession, SignalSink,
};
use super::supervisor::{LoopExit, classify_handshake_err};
use crate::audio::AudioSharedState;
use crate::backend::platform_backend;
use crate::proto::{SdkAudioStreamIdInfoFrame, SdkIndexFrame};
use crate::rtc::{InboundVideoPacket, VideoSenderFeedback};
use crate::signaling::{build_join_frame, build_subscribe_frame, decode_sdp_answer_with_prev};
use crate::{HuddleAudioConfig, HuddleCredentials, HuddleError, HuddleSessionSnapshot, Result};

const HANDSHAKE_ACK_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct HandshakeResult<S, R, M> {
    pub(super) signal: S,
    pub(super) peer: R,
    pub(super) media: M,
    pub(super) shared: AudioSharedState,
    pub(super) ssrc_to_stream_id: HashMap<u32, u32>,
    pub(super) last_sdp_answer: Option<String>,
    pub(super) pending_index: Option<SdkIndexFrame>,
    pub(super) pending_audio_info: Vec<SdkAudioStreamIdInfoFrame>,
    pub(super) outbound_opus_rx: mpsc::UnboundedReceiver<bytes::Bytes>,
    pub(super) inbound_video_rx: mpsc::UnboundedReceiver<InboundVideoPacket>,
    pub(super) outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
    pub(super) outbound_h264_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    pub(super) pc_state_rx: mpsc::UnboundedReceiver<RTCPeerConnectionState>,
    pub(super) keyframe_req_tx: mpsc::UnboundedSender<u32>,
    pub(super) keyframe_req_rx: mpsc::UnboundedReceiver<u32>,
    pub(super) codec_tx: mpsc::UnboundedSender<String>,
    pub(super) codec_rx: mpsc::UnboundedReceiver<String>,
    pub(super) cam_fb_rx: mpsc::UnboundedReceiver<VideoSenderFeedback>,
    pub(super) wants_compressed: bool,
}

/// Scope guard ensuring the background audio-startup task is always stopped if the
/// handshake fails or is cancelled before ownership is transferred to `HandshakeResult`.
struct AudioStartGuard<M: MediaDevices + Send + 'static> {
    handle: Option<tokio::task::JoinHandle<Result<M>>>,
}

impl<M: MediaDevices + Send + 'static> AudioStartGuard<M> {
    fn new(handle: tokio::task::JoinHandle<Result<M>>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    async fn finish(mut self) -> std::result::Result<M, LoopExit> {
        let Some(handle) = self.handle.as_mut() else {
            return Err(LoopExit::Fatal(HuddleError::Audio(
                "audio start task already consumed".to_owned(),
            )));
        };
        // Await by mutable reference so `self.handle` remains `Some` if this future is cancelled
        // mid-flight, allowing `Drop` to spawn cleanup and stop the audio engine.
        let res = handle.await;
        self.handle = None;
        match res {
            Ok(Ok(media)) => Ok(media),
            Ok(Err(e)) => Err(LoopExit::Fatal(e)),
            Err(e) => Err(LoopExit::Fatal(HuddleError::Audio(e.to_string()))),
        }
    }

    async fn cancel_and_stop(mut self) {
        if let Some(handle) = self.handle.as_mut() {
            let res = handle.await;
            self.handle = None;
            if let Ok(Ok(mut media)) = res {
                media.stop_audio();
            }
        }
    }
}

impl<M: MediaDevices + Send + 'static> Drop for AudioStartGuard<M> {
    fn drop(&mut self) {
        // Fallback when the handshake future is dropped/cancelled mid-flight without calling
        // `finish()` or `cancel_and_stop()`.
        if let Some(handle) = self.handle.take() {
            tokio::spawn(async move {
                if let Ok(Ok(mut media)) = handle.await {
                    media.stop_audio();
                }
            });
        }
    }
}

pub(super) async fn perform_handshake_with<F: SessionFactory>(
    factory: &mut F,
    creds: &HuddleCredentials,
    config: &HuddleAudioConfig,
    snap: &HuddleSessionSnapshot,
    is_reconnect: bool,
) -> std::result::Result<HandshakeResult<F::Signal, F::Rtc, F::Media>, LoopExit> {
    // 1. Connect Chime signaling WebSocket and send JOIN.
    tracing::info!(
        channel = %creds.channel,
        call_id = %creds.call_id,
        attendee_id = %creds.attendee_id,
        is_reconnect,
        "connecting to Chime huddle signaling"
    );
    let mut signal = match factory.connect_signal(creds).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(%e, "Chime signaling connect failed");
            return Err(classify_handshake_err(e, is_reconnect));
        }
    };
    let audio_session_id: u64 = rand::rng().random();
    if let Err(e) = signal.send_frame(&build_join_frame(audio_session_id)).await {
        tracing::error!(%e, "failed to send Chime JOIN");
        return Err(classify_handshake_err(e, is_reconnect));
    }

    // Buffer any INDEX or AUDIO_STREAM_ID_INFO frames sent by Chime during the initial handshake
    // so screen shares / cameras already active when we join are never missed.
    let mut pending_index: Option<SdkIndexFrame> = None;
    let mut pending_audio_info: Vec<SdkAudioStreamIdInfoFrame> = Vec::new();

    // 2. Await JOIN_ACK carrying TURN credentials.
    let join_ack = {
        let mut on_other = |frame: &mut crate::proto::SdkSignalFrame| {
            if let Some(idx) = frame.index.take() {
                pending_index = Some(idx);
            }
            if let Some(info) = frame.audio_stream_id_info.take() {
                pending_audio_info.push(info);
            }
        };
        match signal
            .wait_for_join_ack(HANDSHAKE_ACK_TIMEOUT, &mut on_other)
            .await
        {
            Ok(ack) => ack,
            Err(e) => {
                tracing::error!(%e, "JOIN_ACK failed");
                return Err(classify_handshake_err(e, is_reconnect));
            }
        }
    };

    let Some(turn) = join_ack.turn_credentials else {
        signal
            .send_leave_and_close(Duration::from_millis(400))
            .await;
        return Err(classify_handshake_err(
            HuddleError::Signaling("JOIN_ACK did not include TURN credentials".to_owned()),
            is_reconnect,
        ));
    };
    // Always request plaintext SDP on SUBSCRIBE so renegotiations avoid XOR-diff ambiguity;
    // `decode_sdp_answer_with_prev` still handles compressed XOR-diff answers if Chime sends one.
    let wants_compressed = false;
    tracing::info!(
        video_limit = join_ack.video_subscription_limit.unwrap_or(0),
        server_wants_compressed = join_ack.wants_compressed_sdp.unwrap_or(false),
        turn_uris = turn.uris.len(),
        "received Chime JOIN_ACK"
    );

    // 3. Build WebRTC peer with relay-only UDP TURN and gather non-trickle SDP offer.
    let (inbound_opus_tx, inbound_opus_rx) = mpsc::unbounded_channel();
    let (outbound_opus_tx, outbound_opus_rx) = mpsc::unbounded_channel::<bytes::Bytes>();
    let (inbound_video_tx, inbound_video_rx) = mpsc::unbounded_channel::<InboundVideoPacket>();
    let (outbound_h264_tx, outbound_h264_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (pc_state_tx, pc_state_rx) = mpsc::unbounded_channel();
    let (keyframe_req_tx, keyframe_req_rx) = mpsc::unbounded_channel::<u32>();
    let (codec_tx, codec_rx) = mpsc::unbounded_channel::<String>();

    // Kick off full-duplex audio engine initialization (CoreAudio VoiceProcessingIO / cpal)
    // in parallel with WebRTC ICE gathering & Chime SUBSCRIBE/SUBSCRIBE_ACK so the ~1.5s audio
    // unit startup overlaps completely with network signaling!
    let shared = AudioSharedState::from_config(config);
    let audio_guard = AudioStartGuard::new(factory.spawn_audio_start(
        config.clone(),
        shared.clone(),
        outbound_opus_tx,
        inbound_opus_rx,
    ));

    let (cam_fb_tx, cam_fb_rx) = mpsc::unbounded_channel::<VideoSenderFeedback>();
    let backend = platform_backend();
    tracing::info!(
        backend = backend.name(),
        caps = ?backend.capabilities(),
        "huddle media backend capabilities"
    );

    let peer = match factory
        .create_rtc(&turn, inbound_opus_tx, inbound_video_tx, pc_state_tx)
        .await
    {
        Ok(peer) => peer,
        Err(e) => {
            audio_guard.cancel_and_stop().await;
            signal
                .send_leave_and_close(Duration::from_millis(400))
                .await;
            return Err(classify_handshake_err(e, is_reconnect));
        }
    };

    // Single fallible block so every post-peer failure closes `peer`, stops `audio_guard`, and
    // sends Chime `LEAVE` before returning.
    let sub_res: Result<(HashMap<u32, u32>, Option<String>)> = async {
        peer.attach_video_feedback(cam_fb_tx).await;
        let sdp_offer = peer.create_gathered_offer().await?;

        // 4. Send SUBSCRIBE and await SUBSCRIBE_ACK with SDP answer.
        let sub = build_subscribe_frame(
            creds,
            &sdp_offer,
            wants_compressed,
            config.muted,
            snap.video,
            &[],
        )?;
        signal.send_frame(&sub).await?;

        let sub_ack = {
            let mut on_other = |frame: &mut crate::proto::SdkSignalFrame| {
                if let Some(idx) = frame.index.take() {
                    pending_index = Some(idx);
                }
                if let Some(info) = frame.audio_stream_id_info.take() {
                    pending_audio_info.push(info);
                }
            };
            signal
                .wait_for_subscribe_ack(HANDSHAKE_ACK_TIMEOUT, &mut on_other)
                .await
                .inspect_err(|e| tracing::error!(%e, "initial SUBSCRIBE_ACK failed"))?
        };

        let mut ssrc_to_stream_id: HashMap<u32, u32> = HashMap::new();
        for tm in &sub_ack.tracks {
            if let (Some(ssrc), Some(sid)) = (tm.ssrc, tm.stream_id) {
                ssrc_to_stream_id.insert(ssrc, sid);
            }
        }

        let sdp_answer = decode_sdp_answer_with_prev(
            sub_ack.sdp_answer.as_deref(),
            sub_ack.compressed_sdp_answer.as_deref(),
            None,
        )?;
        let last_sdp_answer = Some(sdp_answer.clone());
        peer.set_remote_answer(sdp_answer)
            .await
            .inspect_err(|e| tracing::error!(%e, "initial set_remote_answer failed"))?;
        tracing::info!(
            tracks = sub_ack.tracks.len(),
            buffered_index = pending_index.is_some(),
            "initial SUBSCRIBE_ACK applied"
        );
        Ok((ssrc_to_stream_id, last_sdp_answer))
    }
    .await;

    let (ssrc_to_stream_id, last_sdp_answer) = match sub_res {
        Ok(ok) => ok,
        Err(e) => {
            audio_guard.cancel_and_stop().await;
            signal
                .send_leave_and_close(Duration::from_millis(400))
                .await;
            peer.close().await;
            return Err(classify_handshake_err(e, is_reconnect));
        }
    };

    // 5. Collect the full-duplex audio engine started in parallel with signaling above.
    // If local audio startup fails after WebRTC + Chime JOIN succeeded, tear down both cleanly so
    // we never leak an open peer connection or leave a ghost attendee in the meeting.
    let media = match audio_guard.finish().await {
        Ok(media) => media,
        Err(exit) => {
            signal
                .send_leave_and_close(Duration::from_millis(400))
                .await;
            peer.close().await;
            return Err(exit);
        }
    };

    Ok(HandshakeResult {
        signal,
        peer,
        media,
        shared,
        ssrc_to_stream_id,
        last_sdp_answer,
        pending_index,
        pending_audio_info,
        outbound_opus_rx,
        inbound_video_rx,
        outbound_h264_tx,
        outbound_h264_rx,
        pc_state_rx,
        keyframe_req_tx,
        keyframe_req_rx,
        codec_tx,
        codec_rx,
        cam_fb_rx,
        wants_compressed,
    })
}
