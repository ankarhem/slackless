use std::collections::HashMap;
use std::time::Duration;

use rand::RngExt as _;
use tokio::sync::mpsc;
use webrtc::peer_connection::RTCPeerConnectionState;

use super::supervisor::{LoopExit, classify_handshake_err};
use crate::audio::{AudioEngine, AudioSharedState};
use crate::backend::platform_backend;
use crate::proto::{SdkAudioStreamIdInfoFrame, SdkIndexFrame, sdk_signal_frame};
use crate::rtc::{ChimeRtcPeer, InboundVideoPacket, VideoSenderFeedback};
use crate::signaling::{
    ChimeSignalClient, build_join_frame, build_subscribe_frame, decode_sdp_answer_with_prev,
};
use crate::video::VideoFrame;
use crate::{HuddleAudioConfig, HuddleCredentials, HuddleError, HuddleSessionSnapshot};

pub(super) struct HandshakeResult {
    pub(super) signal: ChimeSignalClient,
    pub(super) peer: ChimeRtcPeer,
    pub(super) audio_engine: AudioEngine,
    pub(super) shared: AudioSharedState,
    pub(super) ssrc_to_stream_id: HashMap<u32, u32>,
    pub(super) last_sdp_answer: Option<String>,
    pub(super) pending_index: Option<SdkIndexFrame>,
    pub(super) pending_audio_info: Vec<SdkAudioStreamIdInfoFrame>,
    pub(super) outbound_opus_tx: mpsc::UnboundedSender<bytes::Bytes>,
    pub(super) outbound_opus_rx: mpsc::UnboundedReceiver<bytes::Bytes>,
    pub(super) inbound_video_rx: mpsc::UnboundedReceiver<InboundVideoPacket>,
    pub(super) outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
    pub(super) outbound_h264_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    pub(super) local_cam_tx: mpsc::UnboundedSender<VideoFrame>,
    pub(super) local_cam_rx: mpsc::UnboundedReceiver<VideoFrame>,
    pub(super) local_screen_tx: mpsc::UnboundedSender<VideoFrame>,
    pub(super) local_screen_rx: mpsc::UnboundedReceiver<VideoFrame>,
    pub(super) pc_state_rx: mpsc::UnboundedReceiver<RTCPeerConnectionState>,
    pub(super) keyframe_req_tx: mpsc::UnboundedSender<u32>,
    pub(super) keyframe_req_rx: mpsc::UnboundedReceiver<u32>,
    pub(super) codec_tx: mpsc::UnboundedSender<String>,
    pub(super) codec_rx: mpsc::UnboundedReceiver<String>,
    pub(super) cam_fb_rx: mpsc::UnboundedReceiver<VideoSenderFeedback>,
    pub(super) wants_compressed: bool,
}

pub(super) async fn perform_handshake(
    creds: &HuddleCredentials,
    config: &HuddleAudioConfig,
    snap: &HuddleSessionSnapshot,
    is_reconnect: bool,
) -> std::result::Result<HandshakeResult, LoopExit> {
    // 1. Connect Chime signaling WebSocket and send JOIN.
    tracing::info!(
        channel = %creds.channel,
        call_id = %creds.call_id,
        attendee_id = %creds.attendee_id,
        is_reconnect,
        "connecting to Chime huddle signaling"
    );
    let mut signal = match ChimeSignalClient::connect(creds).await {
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
    let join_ack = match tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(frame) = signal.next_frame().await? {
            if let Some(idx) = frame.index {
                pending_index = Some(idx);
            }
            if let Some(info) = frame.audio_stream_id_info {
                pending_audio_info.push(info);
            }
            if frame.r#type == Some(sdk_signal_frame::Type::JoinAck as i32)
                && let Some(ack) = frame.joinack
            {
                return Ok(ack);
            }
            if let Some(err) = frame.error {
                return Err(HuddleError::Protocol {
                    status: err.status.unwrap_or(500),
                    description: err.description.unwrap_or_default(),
                });
            }
        }
        Err(HuddleError::Signaling(
            "signaling socket closed before JOIN_ACK".to_owned(),
        ))
    })
    .await
    .map_err(|_| HuddleError::Signaling("timed out waiting for JOIN_ACK".to_owned()))
    {
        Ok(Ok(ack)) => ack,
        Ok(Err(e)) | Err(e) => {
            tracing::error!(%e, "JOIN_ACK failed");
            return Err(classify_handshake_err(e, is_reconnect));
        }
    };

    let Some(turn) = join_ack.turn_credentials else {
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
    let (local_cam_tx, local_cam_rx) = mpsc::unbounded_channel::<VideoFrame>();
    let (local_screen_tx, local_screen_rx) = mpsc::unbounded_channel::<VideoFrame>();
    let (pc_state_tx, pc_state_rx) = mpsc::unbounded_channel();
    let (keyframe_req_tx, keyframe_req_rx) = mpsc::unbounded_channel::<u32>();
    let (codec_tx, codec_rx) = mpsc::unbounded_channel::<String>();

    // 3. Kick off full-duplex audio engine initialization (CoreAudio VoiceProcessingIO / cpal)
    // in parallel with WebRTC ICE gathering & Chime SUBSCRIBE/SUBSCRIBE_ACK so the ~1.5s audio
    // unit startup overlaps completely with network signaling!
    let shared = AudioSharedState::from_config(config);
    let audio_start_handle = {
        let audio_cfg = config.clone();
        let audio_shared = shared.clone();
        let audio_out_tx = outbound_opus_tx.clone();
        tokio::task::spawn_blocking(move || {
            AudioEngine::start(&audio_cfg, audio_shared, audio_out_tx, inbound_opus_rx)
        })
    };

    let (cam_fb_tx, cam_fb_rx) = mpsc::unbounded_channel::<VideoSenderFeedback>();
    let backend = platform_backend();
    tracing::info!(
        backend = backend.name(),
        caps = ?backend.capabilities(),
        "huddle media backend capabilities"
    );

    let peer = match ChimeRtcPeer::new(&turn, inbound_opus_tx, inbound_video_tx, pc_state_tx).await
    {
        Ok(p) => p,
        Err(e) => {
            if let Ok(Ok(mut ae)) = audio_start_handle.await {
                ae.stop();
            }
            return Err(classify_handshake_err(e, is_reconnect));
        }
    };
    peer.attach_video_feedback(cam_fb_tx).await;
    let sdp_offer = match peer.create_gathered_offer().await {
        Ok(s) => s,
        Err(e) => {
            if let Ok(Ok(mut ae)) = audio_start_handle.await {
                ae.stop();
            }
            return Err(classify_handshake_err(e, is_reconnect));
        }
    };

    // 4. Send SUBSCRIBE and await SUBSCRIBE_ACK with SDP answer.
    let sub = match build_subscribe_frame(
        creds,
        &sdp_offer,
        wants_compressed,
        config.muted,
        snap.video,
        &[],
    ) {
        Ok(f) => f,
        Err(e) => {
            if let Ok(Ok(mut ae)) = audio_start_handle.await {
                ae.stop();
            }
            return Err(classify_handshake_err(e, is_reconnect));
        }
    };
    if let Err(e) = signal.send_frame(&sub).await {
        if let Ok(Ok(mut ae)) = audio_start_handle.await {
            ae.stop();
        }
        return Err(classify_handshake_err(e, is_reconnect));
    }

    let sub_ack = match tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(frame) = signal.next_frame().await? {
            if let Some(idx) = frame.index {
                pending_index = Some(idx);
            }
            if let Some(info) = frame.audio_stream_id_info {
                pending_audio_info.push(info);
            }
            if frame.r#type == Some(sdk_signal_frame::Type::SubscribeAck as i32)
                && let Some(ack) = frame.suback
            {
                return Ok(ack);
            }
            if let Some(err) = frame.error {
                return Err(HuddleError::Protocol {
                    status: err.status.unwrap_or(500),
                    description: err.description.unwrap_or_default(),
                });
            }
        }
        Err(HuddleError::Signaling(
            "signaling socket closed before SUBSCRIBE_ACK".to_owned(),
        ))
    })
    .await
    .map_err(|_| HuddleError::Signaling("timed out waiting for SUBSCRIBE_ACK".to_owned()))
    {
        Ok(Ok(ack)) => ack,
        Ok(Err(e)) | Err(e) => {
            tracing::error!(%e, "initial SUBSCRIBE_ACK failed");
            if let Ok(Ok(mut ae)) = audio_start_handle.await {
                ae.stop();
            }
            return Err(classify_handshake_err(e, is_reconnect));
        }
    };

    let mut ssrc_to_stream_id: HashMap<u32, u32> = HashMap::new();
    for tm in &sub_ack.tracks {
        if let (Some(ssrc), Some(sid)) = (tm.ssrc, tm.stream_id) {
            ssrc_to_stream_id.insert(ssrc, sid);
        }
    }

    let sdp_answer = match decode_sdp_answer_with_prev(
        sub_ack.sdp_answer.as_deref(),
        sub_ack.compressed_sdp_answer.as_deref(),
        None,
    ) {
        Ok(s) => s,
        Err(e) => {
            if let Ok(Ok(mut ae)) = audio_start_handle.await {
                ae.stop();
            }
            return Err(classify_handshake_err(e, is_reconnect));
        }
    };
    let last_sdp_answer = Some(sdp_answer.clone());
    if let Err(e) = peer.set_remote_answer(sdp_answer).await {
        tracing::error!(%e, "initial set_remote_answer failed");
        if let Ok(Ok(mut ae)) = audio_start_handle.await {
            ae.stop();
        }
        return Err(classify_handshake_err(e, is_reconnect));
    }
    tracing::info!(
        tracks = sub_ack.tracks.len(),
        buffered_index = pending_index.is_some(),
        "initial SUBSCRIBE_ACK applied"
    );

    // 5. Collect the full-duplex audio engine started in parallel with signaling above.
    let audio_engine = match audio_start_handle.await {
        Ok(Ok(e)) => e,
        Ok(Err(e)) => return Err(LoopExit::Fatal(e)),
        Err(e) => return Err(LoopExit::Fatal(HuddleError::Audio(e.to_string()))),
    };

    Ok(HandshakeResult {
        signal,
        peer,
        audio_engine,
        shared,
        ssrc_to_stream_id,
        last_sdp_answer,
        pending_index,
        pending_audio_info,
        outbound_opus_tx,
        outbound_opus_rx,
        inbound_video_rx,
        outbound_h264_tx,
        outbound_h264_rx,
        local_cam_tx,
        local_cam_rx,
        local_screen_tx,
        local_screen_rx,
        pc_state_rx,
        keyframe_req_tx,
        keyframe_req_rx,
        codec_tx,
        codec_rx,
        cam_fb_rx,
        wants_compressed,
    })
}
