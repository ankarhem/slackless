//! Full Amazon Chime huddle session orchestrator:
//! - Resilient signaling + auto-reconnect supervisor (1s -> 2s -> 4s backoff on unexpected drop)
//! - WebRTC DTLS-SRTP audio + multi-codec video (`AV1` via `rav1d` + `H.264` via `openh264`)
//! - Official `#content` second Chime session for native screen sharing (simultaneous with camera!)
//! - Official Chime `INDEX` + mid-2+ `Recvonly` transceivers + `SUBSCRIBE` + `REMOTE_VIDEO_UPDATE`
//! - Official Slack `PeerMessage` (`calls/client/peer_message.proto`) on `"data-channel"`
//!   (reactions, effects, GIFs, sticky emoji / hand raise, drawing on shared screens, jukebox, mute)
//! - Official AWS Chime `SdkTranscriptFrame` live captions on `"aws:chime:transcription"`

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rand::Rng as _;
use slackless_types::UserId;
use tokio::sync::mpsc;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

use crate::audio::{AmbientMusicPlayer, AudioEngine, AudioSharedState};
use crate::av1::Av1Frames;
use crate::backend::{VideoDecoderHandle, platform_backend};
use crate::content_share::ContentShareSession;
use crate::metrics::{HuddlePerfCounters, HuddlePerfSnapshot, ProcessResourceSampler};
use crate::peer_message::{
    CHIME_TRANSCRIPTION_TOPIC, SLACK_DATA_CHANNEL_TOPIC, build_capabilities_message,
    build_content_share_details_message, build_draw_message, build_jukebox_message,
    build_mute_request_message, build_reacji_message, build_sticky_emoji_awareness,
    decode_chime_transcript_frame, decode_slack_peer_message,
};
use crate::proto::{SdkAudioStreamIdInfoFrame, SdkIndexFrame, sdk_signal_frame};
use crate::rtc::{ChimeRtcPeer, InboundVideoPacket, VideoSenderFeedback};
use crate::signaling::{
    ChimeSignalClient, RemoteVideoSource, build_audio_control_frame, build_client_metric_frame,
    build_data_message_frame, build_join_frame, build_leave_frame, build_pong_frame,
    build_remote_video_update_frame, build_subscribe_frame, decode_sdp_answer_with_prev, now_ms,
    parse_external_user_id, parse_index_sources_with_lookup,
};
use crate::video::{CameraCapture, H264VideoDecoder, VideoFrame, VideoTileKey};
use crate::{
    HuddleAudioConfig, HuddleAudioLevels, HuddleControl, HuddleCredentials, HuddleDataEvent,
    HuddleError, HuddleHandle, HuddlePhase, HuddleSessionSnapshot, HuddleUpdate,
};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

struct SessionDoneGuard {
    finished: Arc<AtomicBool>,
    done: Arc<tokio::sync::Notify>,
}

impl Drop for SessionDoneGuard {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::Release);
        self.done.notify_waiters();
    }
}

/// Start a native audio/video/screen-share huddle session on the current tokio runtime.
pub fn spawn_huddle_session(
    creds: HuddleCredentials,
    config: HuddleAudioConfig,
    updates: mpsc::UnboundedSender<HuddleUpdate>,
) -> HuddleHandle {
    let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
    let finished = Arc::new(AtomicBool::new(false));
    let done = Arc::new(tokio::sync::Notify::new());
    let guard = SessionDoneGuard {
        finished: Arc::clone(&finished),
        done: Arc::clone(&done),
    };
    tokio::spawn(async move {
        let _done_guard = guard;
        run_session(creds, config, ctrl_rx, updates).await;
    });
    HuddleHandle::new(ctrl_tx, finished, done)
}

enum LoopExit {
    LeftCleanly,
    Reconnect(HuddleError),
    Fatal(HuddleError),
}

enum RemoteVideoDecoder {
    Av1 {
        frames: Av1Frames,
        dec: Box<dyn VideoDecoderHandle>,
    },
    H264(H264VideoDecoder),
}

impl RemoteVideoDecoder {
    fn for_mime(mime: &str) -> Option<Self> {
        if mime.eq_ignore_ascii_case("video/AV1") {
            platform_backend()
                .create_av1_decoder()
                .ok()
                .map(|dec| Self::Av1 {
                    frames: Av1Frames::default(),
                    dec,
                })
        } else if mime.eq_ignore_ascii_case("video/H264") {
            H264VideoDecoder::new().ok().map(Self::H264)
        } else {
            None
        }
    }

    fn backend_name(&self) -> &'static str {
        match self {
            Self::Av1 { dec, .. } => dec.backend_name(),
            Self::H264(dec) => dec.backend_name(),
        }
    }

    fn push(&mut self, pkt: &InboundVideoPacket) -> (Option<VideoFrame>, Option<String>) {
        match self {
            Self::Av1 { frames, dec } => {
                if let Some(unit) = frames.push(pkt.seq, pkt.marker, &pkt.payload) {
                    (dec.decode_unit(&unit), None)
                } else {
                    (None, None)
                }
            }
            Self::H264(dec) => (dec.push_rtp_payload(&pkt.payload, pkt.marker), None),
        }
    }
}

async fn run_session(
    creds: HuddleCredentials,
    mut config: HuddleAudioConfig,
    mut ctrl_rx: mpsc::UnboundedReceiver<HuddleControl>,
    updates: mpsc::UnboundedSender<HuddleUpdate>,
) {
    let mut snap = HuddleSessionSnapshot {
        phase: HuddlePhase::Connecting,
        channel: creds.channel.clone(),
        call_id: creds.call_id.clone(),
        attendee_id: creds.attendee_id.clone(),
        muted: config.muted,
        video: false,
        blur_background: config.blur_background,
        audio_only: false,
        sharing: false,
        share_source_id: None,
        screen_audio_device: None,
        hand_raised: false,
        sticky_emoji: None,
        captions_enabled: false,
        topic: None,
        background_id: None,
        jukebox_song: None,
        started_at_secs: now_secs(),
        thread_root_ts: creds.thread_root_ts.clone(),
        canvas_file_id: creds.canvas_file_id.clone(),
        huddle_link: creds.huddle_link.clone(),
        input_device: config.input_device.clone(),
        output_device: config.output_device.clone(),
        aec: config.aec,
        ns: config.ns,
        agc: config.agc,
        active_video_codec: None,
        error: None,
        udp_blocked: false,
        packets_sent: 0,
        packets_received: 0,
        bytes_sent: 0,
        bytes_received: 0,
        pongs_sent: 0,
        perf: HuddlePerfSnapshot::default(),
    };
    let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));

    let mut attempt = 0u32;
    const MAX_RECONNECTS: u32 = 3;

    let final_err = loop {
        match run_connected_loop(&creds, &mut config, &mut snap, &mut ctrl_rx, &updates).await {
            LoopExit::LeftCleanly => break None,
            LoopExit::Fatal(err) => break Some(err),
            LoopExit::Reconnect(err) => {
                attempt += 1;
                if attempt > MAX_RECONNECTS {
                    break Some(err);
                }
                snap.phase = HuddlePhase::Reconnecting;
                snap.error = None;
                let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                let backoff = Duration::from_secs(1u64 << (attempt - 1));
                let mut cancelled = false;
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    ctrl = ctrl_rx.recv() => {
                        if matches!(ctrl, Some(HuddleControl::Leave) | None) {
                            cancelled = true;
                        }
                    }
                }
                if cancelled {
                    break None;
                }
            }
        }
    };

    match final_err {
        None => {
            snap.phase = HuddlePhase::Idle;
            snap.video = false;
            snap.sharing = false;
            snap.jukebox_song = None;
            snap.error = None;
            let _ = updates.send(HuddleUpdate::State(Box::new(snap)));
        }
        Some(err) => {
            snap.phase = HuddlePhase::Failed;
            snap.video = false;
            snap.sharing = false;
            snap.jukebox_song = None;
            snap.udp_blocked = matches!(err, HuddleError::UdpBlocked);
            snap.error = Some(err.to_string());
            let _ = updates.send(HuddleUpdate::State(Box::new(snap)));
        }
    }
}

async fn run_connected_loop(
    creds: &HuddleCredentials,
    config: &mut HuddleAudioConfig,
    snap: &mut HuddleSessionSnapshot,
    ctrl_rx: &mut mpsc::UnboundedReceiver<HuddleControl>,
    updates: &mpsc::UnboundedSender<HuddleUpdate>,
) -> LoopExit {
    // 1. Connect Chime signaling WebSocket and send JOIN.
    tracing::info!(
        channel = %creds.channel,
        call_id = %creds.call_id,
        attendee_id = %creds.attendee_id,
        "connecting to Chime huddle signaling"
    );
    let mut signal = match ChimeSignalClient::connect(creds).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(%e, "Chime signaling connect failed");
            return LoopExit::Fatal(e);
        }
    };
    let audio_session_id: u64 = rand::rng().random();
    if let Err(e) = signal.send_frame(&build_join_frame(audio_session_id)).await {
        tracing::error!(%e, "failed to send Chime JOIN");
        return LoopExit::Fatal(e);
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
            if frame.r#type == Some(sdk_signal_frame::Type::JoinAck as i32) {
                if let Some(ack) = frame.joinack {
                    return Ok(ack);
                }
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
            return LoopExit::Fatal(e);
        }
    };

    let Some(turn) = join_ack.turn_credentials else {
        return LoopExit::Fatal(HuddleError::Signaling(
            "JOIN_ACK did not include TURN credentials".to_owned(),
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
    let (outbound_opus_tx, mut outbound_opus_rx) = mpsc::unbounded_channel::<bytes::Bytes>();
    let (inbound_video_tx, inbound_video_rx) = mpsc::unbounded_channel::<InboundVideoPacket>();
    let (outbound_h264_tx, mut outbound_h264_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (local_cam_tx, mut local_cam_rx) = mpsc::unbounded_channel::<VideoFrame>();
    let (local_screen_tx, mut local_screen_rx) = mpsc::unbounded_channel::<VideoFrame>();
    let (pc_state_tx, mut pc_state_rx) = mpsc::unbounded_channel();
    let (keyframe_req_tx, mut keyframe_req_rx) = mpsc::unbounded_channel::<u32>();
    let (codec_tx, mut codec_rx) = mpsc::unbounded_channel::<String>();

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

    let (cam_fb_tx, mut cam_fb_rx) = mpsc::unbounded_channel::<VideoSenderFeedback>();
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
            return LoopExit::Fatal(e);
        }
    };
    peer.attach_video_feedback(cam_fb_tx).await;
    let sdp_offer = match peer.create_gathered_offer().await {
        Ok(s) => s,
        Err(e) => {
            if let Ok(Ok(mut ae)) = audio_start_handle.await {
                ae.stop();
            }
            return LoopExit::Fatal(e);
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
            return LoopExit::Fatal(e);
        }
    };
    if let Err(e) = signal.send_frame(&sub).await {
        if let Ok(Ok(mut ae)) = audio_start_handle.await {
            ae.stop();
        }
        return LoopExit::Fatal(e);
    }

    let sub_ack = match tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(frame) = signal.next_frame().await? {
            if let Some(idx) = frame.index {
                pending_index = Some(idx);
            }
            if let Some(info) = frame.audio_stream_id_info {
                pending_audio_info.push(info);
            }
            if frame.r#type == Some(sdk_signal_frame::Type::SubscribeAck as i32) {
                if let Some(ack) = frame.suback {
                    return Ok(ack);
                }
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
            return LoopExit::Fatal(e);
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
            return LoopExit::Fatal(e);
        }
    };
    let mut last_sdp_answer = Some(sdp_answer.clone());
    if let Err(e) = peer.set_remote_answer(sdp_answer).await {
        tracing::error!(%e, "initial set_remote_answer failed");
        if let Ok(Ok(mut ae)) = audio_start_handle.await {
            ae.stop();
        }
        return LoopExit::Fatal(e);
    }
    tracing::info!(
        tracks = sub_ack.tracks.len(),
        buffered_index = pending_index.is_some(),
        "initial SUBSCRIBE_ACK applied"
    );

    // 5. Collect the full-duplex audio engine started in parallel with signaling above.
    let mut audio_engine = match audio_start_handle.await {
        Ok(Ok(e)) => e,
        Ok(Err(e)) => return LoopExit::Fatal(e),
        Err(e) => return LoopExit::Fatal(HuddleError::Audio(e.to_string())),
    };

    let perf = Arc::new(HuddlePerfCounters::default());
    let mut res_sampler = ProcessResourceSampler::new();
    let vdec_audio_only = Arc::new(AtomicBool::new(snap.audio_only));
    let vdec_clear = Arc::new(AtomicBool::new(false));
    let vdec_routing = Arc::new(Mutex::new(VideoRoutingTable {
        ssrc_to_stream_id: ssrc_to_stream_id.clone(),
        remote_sources: Vec::new(),
    }));
    let (vdec_tx, vdec_rx) = std::sync::mpsc::channel::<Option<InboundVideoPacket>>();
    let vdec_tx_fwd = vdec_tx.clone();
    let vdec_fwd = tokio::spawn(async move {
        let mut rx = inbound_video_rx;
        while let Some(pkt) = rx.recv().await {
            if vdec_tx_fwd.send(Some(pkt)).is_err() {
                break;
            }
        }
    });
    let vdec_handle = spawn_video_decoder_thread(
        vdec_rx,
        Arc::clone(&vdec_audio_only),
        Arc::clone(&vdec_clear),
        Arc::clone(&vdec_routing),
        Arc::clone(&perf),
        updates.clone(),
        keyframe_req_tx,
        codec_tx,
    );

    let mut camera_capture: Option<CameraCapture> = None;
    let mut content_share: Option<ContentShareSession> = None;
    let mut music_player: Option<AmbientMusicPlayer> = None;
    let mut remote_sources: Vec<(String, RemoteVideoSource)> = Vec::new();
    let mut active_remote_tiles: HashSet<VideoTileKey> = HashSet::new();

    let mut ever_connected = peer.connection_state() == RTCPeerConnectionState::Connected;
    if ever_connected {
        snap.phase = HuddlePhase::Connected;
        snap.started_at_secs = now_secs();
        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
    }
    let connect_deadline = tokio::time::Instant::now() + Duration::from_secs(12);

    let mut stream_users: HashMap<u32, UserId> = HashMap::new();
    let mut attendee_to_user: HashMap<String, UserId> = HashMap::new();
    let mut muted_peers: HashSet<UserId> = HashSet::new();
    let mut remote_speakers: Vec<(UserId, f32)> = Vec::new();
    let mut metric_tick = tokio::time::interval(Duration::from_secs(1));
    let mut perf_tick = tokio::time::interval(Duration::from_secs(5));
    let mut level_tick = tokio::time::interval(Duration::from_millis(120));

    let self_user =
        parse_external_user_id(&creds.external_user_id).unwrap_or_else(|| UserId::from("U0SELF"));
    let self_content_attendee = format!("{}#content", creds.attendee_id);

    // Apply any AUDIO_STREAM_ID_INFO and INDEX frames captured during the initial handshake.
    for info in pending_audio_info.drain(..) {
        for stream in info.streams {
            if let (Some(att), Some(uid)) = (
                stream.attendee_id.as_deref(),
                stream
                    .external_user_id
                    .as_deref()
                    .and_then(parse_external_user_id),
            ) {
                let base = att.strip_suffix("#content").unwrap_or(att);
                attendee_to_user.insert(base.to_owned(), uid.clone());
                if let Some(sid) = stream.audio_stream_id {
                    if stream.dropped != Some(true) {
                        stream_users.insert(sid, uid.clone());
                        if stream.muted == Some(true) {
                            muted_peers.insert(uid);
                        }
                    }
                }
            }
        }
    }
    if let Some(index) = pending_index.take() {
        apply_index_frame(
            &index,
            creds,
            wants_compressed,
            config.muted,
            snap.video,
            snap.audio_only,
            &self_content_attendee,
            &attendee_to_user,
            &peer,
            &mut signal,
            &mut remote_sources,
            &vdec_routing,
            &mut active_remote_tiles,
            updates,
        )
        .await;
    }

    let video_tx_task = {
        let video_sender = peer.video_sender();
        tokio::spawn(async move {
            while let Some(h264_frame) = outbound_h264_rx.recv().await {
                let _ = video_sender
                    .write_h264_frame(h264_frame, Duration::from_millis(42))
                    .await;
            }
        })
    };

    let mut exit_reason: Option<LoopExit> = None;
    let mut last_levels_rms_bucket: u8 = 0;
    let mut last_levels_speakers: Vec<(UserId, f32)> = Vec::new();
    let mut last_levels_muted: Vec<UserId> = Vec::new();

    while exit_reason.is_none() {
        let connect_timeout = async {
            if ever_connected {
                std::future::pending::<()>().await;
            } else {
                tokio::time::sleep_until(connect_deadline).await;
            }
        };
        tokio::select! {
            _ = connect_timeout => {
                exit_reason = Some(LoopExit::Fatal(HuddleError::UdpBlocked));
            }
            Some(opus_pkt) = outbound_opus_rx.recv() => {
                let _ = peer.write_opus_frame(opus_pkt).await;
            }
            Some(fb) = cam_fb_rx.recv() => {
                if let Some(cap) = &camera_capture {
                    match fb {
                        VideoSenderFeedback::ForceKeyframe => cap.force_keyframe(),
                        VideoSenderFeedback::TargetBitrateBps(bps) => {
                            cap.set_target_bitrate(bps.clamp(350_000, 1_500_000));
                        }
                    }
                }
            }
            Some(frame) = local_cam_rx.recv() => {
                let _ = updates.send(HuddleUpdate::VideoFrame {
                    tile: VideoTileKey::LocalCamera,
                    frame,
                });
            }
            Some(frame) = local_screen_rx.recv() => {
                let _ = updates.send(HuddleUpdate::VideoFrame {
                    tile: VideoTileKey::LocalScreen,
                    frame,
                });
            }
            Some(ssrc) = keyframe_req_rx.recv() => {
                peer.request_keyframe(ssrc).await;
            }
            Some(codec) = codec_rx.recv() => {
                if snap.active_video_codec.as_deref() != Some(codec.as_str()) {
                    snap.active_video_codec = Some(codec);
                    let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                }
            }
            Some(pc_state) = pc_state_rx.recv() => {
                tracing::info!(state = ?pc_state, "huddle RTCPeerConnection state changed");
                match pc_state {
                    RTCPeerConnectionState::Connected => {
                        if !ever_connected {
                            ever_connected = true;
                            snap.started_at_secs = now_secs();
                        }
                        if snap.phase != HuddlePhase::Connected {
                            snap.phase = HuddlePhase::Connected;
                            snap.packets_sent = peer.total_packets_sent();
                            snap.packets_received = peer.total_packets_received();
                            let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                        }
                    }
                    RTCPeerConnectionState::Disconnected => {
                        snap.phase = HuddlePhase::Reconnecting;
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    RTCPeerConnectionState::Failed => {
                        exit_reason = Some(if ever_connected {
                            LoopExit::Reconnect(HuddleError::UdpBlocked)
                        } else {
                            LoopExit::Fatal(HuddleError::UdpBlocked)
                        });
                    }
                    _ => {}
                }
            }
            frame_res = signal.next_frame() => {
                match frame_res {
                    Ok(Some(frame)) => {
                        if let Some(pp) = frame.ping_pong {
                            if let Some(id) = pp.ping_id {
                                if signal.send_frame(&build_pong_frame(id)).await.is_ok() {
                                    snap.pongs_sent = snap.pongs_sent.saturating_add(1);
                                }
                            }
                        }
                        if let Some(index) = frame.index {
                            apply_index_frame(
                                &index,
                                creds,
                                wants_compressed,
                                config.muted,
                                snap.video,
                                snap.audio_only,
                                &self_content_attendee,
                                &attendee_to_user,
                                &peer,
                                &mut signal,
                                &mut remote_sources,
                                &vdec_routing,
                                &mut active_remote_tiles,
                                updates,
                            )
                            .await;
                        }
                        if let Some(ack) = frame.suback {
                            for tm in &ack.tracks {
                                if let (Some(ssrc), Some(sid)) = (tm.ssrc, tm.stream_id) {
                                    ssrc_to_stream_id.insert(ssrc, sid);
                                }
                            }
                            if let Ok(mut rt) = vdec_routing.lock() {
                                rt.ssrc_to_stream_id = ssrc_to_stream_id.clone();
                            }
                            tracing::info!(
                                tracks = ack.tracks.len(),
                                mapped_ssrcs = ssrc_to_stream_id.len(),
                                "received renegotiation SUBSCRIBE_ACK"
                            );
                            match decode_sdp_answer_with_prev(
                                ack.sdp_answer.as_deref(),
                                ack.compressed_sdp_answer.as_deref(),
                                last_sdp_answer.as_deref(),
                            ) {
                                Ok(answer) => {
                                    last_sdp_answer = Some(answer.clone());
                                    if let Err(err) = peer.set_remote_answer(answer).await {
                                        tracing::error!(
                                            %err,
                                            "renegotiation set_remote_answer failed"
                                        );
                                    }
                                }
                                Err(err) => {
                                    tracing::error!(
                                        %err,
                                        "renegotiation decode_sdp_answer failed"
                                    );
                                }
                            }
                        }
                        if let Some(dm) = frame.data_message {
                            for msg in dm.messages {
                                for ev in parse_incoming_data_message(
                                    &msg,
                                    &creds.attendee_id,
                                ) {
                                    match &ev {
                                        HuddleDataEvent::Topic { topic, .. } => {
                                            snap.topic = (!topic.is_empty()).then(|| topic.clone());
                                            let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                                        }
                                        HuddleDataEvent::MutedByPeer { .. } => {
                                            config.muted = true;
                                            shared.muted.store(true, Ordering::Relaxed);
                                            snap.muted = true;
                                            let _ = signal.send_frame(&build_audio_control_frame(true)).await;
                                            let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                                        }
                                        HuddleDataEvent::Jukebox { song_key, .. } => {
                                            if let Some(mut prev) = music_player.take() {
                                                prev.stop();
                                            }
                                            if song_key.is_some() {
                                                music_player = AmbientMusicPlayer::start(
                                                    config.output_device.clone(),
                                                    song_key.as_deref(),
                                                )
                                                .ok();
                                            }
                                            snap.jukebox_song = song_key.clone();
                                            let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                                        }
                                        _ => {}
                                    }
                                    let _ = updates.send(HuddleUpdate::DataEvent(ev));
                                }
                            }
                        }
                        if let Some(info) = frame.audio_stream_id_info {
                            let mut new_peer_joined = false;
                            for stream in info.streams {
                                let Some(sid) = stream.audio_stream_id else {
                                    continue;
                                };
                                if stream.dropped == Some(true) {
                                    if let Some(uid) = stream_users.remove(&sid) {
                                        muted_peers.remove(&uid);
                                    }
                                    continue;
                                }
                                if let Some(uid) = stream
                                    .external_user_id
                                    .as_deref()
                                    .and_then(parse_external_user_id)
                                {
                                    if let Some(att) = stream.attendee_id.as_deref() {
                                        let base = att.strip_suffix("#content").unwrap_or(att);
                                        attendee_to_user.insert(base.to_owned(), uid.clone());
                                    }
                                    if stream_users.insert(sid, uid.clone()).is_none()
                                        && uid != self_user
                                    {
                                        new_peer_joined = true;
                                    }
                                    if stream.muted == Some(true) {
                                        muted_peers.insert(uid);
                                    } else {
                                        muted_peers.remove(&uid);
                                    }
                                }
                            }
                            // Re-announce desktop draw capabilities + content source details when
                            // a new peer joins while we are already sharing so late joiners can
                            // immediately draw on our screen share.
                            if new_peer_joined && snap.sharing {
                                if let Some(src_id) = snap.share_source_id.as_deref() {
                                    let cap_bytes =
                                        build_capabilities_message(&creds.attendee_id, true);
                                    let _ = signal
                                        .send_frame(&build_data_message_frame(
                                            SLACK_DATA_CHANNEL_TOPIC,
                                            cap_bytes,
                                            300_000,
                                        ))
                                        .await;
                                    let det_bytes = build_content_share_details_message(
                                        &creds.attendee_id,
                                        content_share_source_type(src_id),
                                    );
                                    let _ = signal
                                        .send_frame(&build_data_message_frame(
                                            SLACK_DATA_CHANNEL_TOPIC,
                                            det_bytes,
                                            300_000,
                                        ))
                                        .await;
                                }
                            }
                        }
                        if let Some(meta) = frame.audio_metadata {
                            remote_speakers.clear();
                            for st in meta.attendee_states {
                                let Some(sid) = st.audio_stream_id else {
                                    continue;
                                };
                                let Some(uid) = stream_users.get(&sid).cloned() else {
                                    continue;
                                };
                                let is_muted = st.muted.unwrap_or(false);
                                if is_muted {
                                    muted_peers.insert(uid.clone());
                                } else {
                                    muted_peers.remove(&uid);
                                    let vol = st.volume.unwrap_or(0) as f32 / 100.0;
                                    if vol > 0.05 {
                                        remote_speakers.push((uid, vol.clamp(0.0, 1.0)));
                                    }
                                }
                            }
                            remote_speakers.sort_by(|a, b| {
                                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                            });
                        }
                        if let Some(err) = frame.error {
                            exit_reason = Some(LoopExit::Fatal(HuddleError::Protocol {
                                status: err.status.unwrap_or(500),
                                description: err.description.unwrap_or_default(),
                            }));
                        }
                    }
                    Ok(None) => {
                        exit_reason = Some(if ever_connected {
                            LoopExit::Reconnect(HuddleError::Signaling(
                                "signaling socket closed unexpectedly".to_owned(),
                            ))
                        } else {
                            LoopExit::LeftCleanly
                        });
                    }
                    Err(e) => {
                        exit_reason = Some(if ever_connected {
                            LoopExit::Reconnect(e)
                        } else {
                            LoopExit::Fatal(e)
                        });
                    }
                }
            }
            _ = metric_tick.tick() => {
                let stats = peer.take_metrics_snapshot(shared.local_rms());
                let _ = signal.send_frame(&build_client_metric_frame(stats)).await;
                let tx_pkts = peer.total_packets_sent();
                let rx_pkts = peer.total_packets_received();
                let tx_bytes = peer.total_bytes_sent();
                let rx_bytes = peer.total_bytes_received();
                if snap.packets_sent != tx_pkts
                    || snap.packets_received != rx_pkts
                    || snap.bytes_sent != tx_bytes
                    || snap.bytes_received != rx_bytes
                {
                    snap.packets_sent = tx_pkts;
                    snap.packets_received = rx_pkts;
                    snap.bytes_sent = tx_bytes;
                    snap.bytes_received = rx_bytes;
                    let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                }
            }
            _ = perf_tick.tick() => {
                let (dt, cpu_tenths, rss_mb) = res_sampler.sample();
                let perf_snap = perf.sample(dt, cpu_tenths, rss_mb);
                tracing::info!(
                    cpu_pct = format_args!("{}.{}", perf_snap.cpu_tenths_pct / 10, perf_snap.cpu_tenths_pct % 10),
                    rss_mb = perf_snap.rss_mb,
                    tx_backend = perf_snap.tx_backend.as_deref().unwrap_or("none"),
                    tx_res = perf_snap.tx_resolution.as_deref().unwrap_or("-"),
                    tx_fps = format_args!("{}.{}", perf_snap.tx_fps_tenths / 10, perf_snap.tx_fps_tenths % 10),
                    tx_enc_ms = format_args!("{:.2}", perf_snap.tx_encode_us as f64 / 1000.0),
                    tx_kbps = perf_snap.tx_kbps,
                    rx_backend = perf_snap.rx_backend.as_deref().unwrap_or("none"),
                    rx_res = perf_snap.rx_resolution.as_deref().unwrap_or("-"),
                    rx_fps = format_args!("{}.{}", perf_snap.rx_fps_tenths / 10, perf_snap.rx_fps_tenths % 10),
                    rx_dec_ms = format_args!("{:.2}", perf_snap.rx_decode_us as f64 / 1000.0),
                    rx_kbps = perf_snap.rx_kbps,
                    "huddle perf"
                );
                if snap.perf != perf_snap {
                    snap.perf = perf_snap;
                    let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                }
            }
            _ = level_tick.tick() => {
                // Skip emitting 8.3 Hz audio-level UI updates during silence when neither local
                // RMS bucket nor remote speakers/muted peers changed.
                let local_rms = shared.local_rms();
                let rms_bucket = if local_rms < 0.005 {
                    0
                } else {
                    ((local_rms * 32.0).round() as u8).clamp(1, 32)
                };
                let mut cur_muted: Vec<UserId> = muted_peers.iter().cloned().collect();
                cur_muted.sort();
                if rms_bucket != last_levels_rms_bucket
                    || remote_speakers != last_levels_speakers
                    || cur_muted != last_levels_muted
                {
                    last_levels_rms_bucket = rms_bucket;
                    last_levels_speakers = remote_speakers.clone();
                    last_levels_muted = cur_muted.clone();
                    let _ = updates.send(HuddleUpdate::AudioLevels(HuddleAudioLevels {
                        local_rms: if rms_bucket == 0 { 0.0 } else { local_rms },
                        active_speakers: remote_speakers.clone(),
                        muted_peers: cur_muted,
                    }));
                }
            }
            ctrl = ctrl_rx.recv() => {
                match ctrl {
                    Some(HuddleControl::SetMuted(muted)) => {
                        config.muted = muted;
                        shared.muted.store(muted, Ordering::Relaxed);
                        snap.muted = muted;
                        let _ = signal.send_frame(&build_audio_control_frame(muted)).await;
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::SetAudioProcessing { aec, ns, agc }) => {
                        config.aec = aec;
                        config.ns = ns;
                        config.agc = agc;
                        snap.aec = aec;
                        snap.ns = ns;
                        snap.agc = agc;
                        shared.aec.store(aec, Ordering::Relaxed);
                        shared.ns.store(ns, Ordering::Relaxed);
                        shared.agc.store(agc, Ordering::Relaxed);
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::SetAudioDevices { input, output }) => {
                        config.input_device = input.clone();
                        config.output_device = output.clone();
                        snap.input_device = input;
                        snap.output_device = output;
                        if let Some(rx) = audio_engine.stop() {
                            if let Ok(restarted) = AudioEngine::start(
                                config,
                                shared.clone(),
                                outbound_opus_tx.clone(),
                                rx,
                            ) {
                                audio_engine = restarted;
                            }
                        }
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::SetCamera {
                        enabled,
                        device_index,
                        blur,
                    }) => {
                        config.camera_device = device_index;
                        config.blur_background = blur;
                        snap.blur_background = blur;
                        if enabled {
                            if let Some(mut prev) = camera_capture.take() {
                                prev.stop();
                            }
                            match CameraCapture::start(
                                device_index,
                                blur,
                                snap.sharing,
                                outbound_h264_tx.clone(),
                                local_cam_tx.clone(),
                            ) {
                                Ok(cap) => {
                                    camera_capture = Some(cap);
                                    snap.video = true;
                                    snap.error = None;
                                    if let Ok(offer) = peer.create_gathered_offer().await {
                                        if let Ok(f) = build_subscribe_frame(
                                            creds,
                                            &offer,
                                            wants_compressed,
                                            config.muted,
                                            true,
                                            &remote_sources,
                                        ) {
                                            let _ = signal.send_frame(&f).await;
                                        }
                                    }
                                }
                                Err(err) => {
                                    snap.video = false;
                                    snap.error = Some(err.to_string());
                                }
                            }
                        } else {
                            if let Some(mut cap) = camera_capture.take() {
                                cap.stop();
                            }
                            snap.video = false;
                            let _ = updates.send(HuddleUpdate::VideoTileClosed(VideoTileKey::LocalCamera));
                            if let Ok(offer) = peer.create_gathered_offer().await {
                                if let Ok(f) = build_subscribe_frame(
                                    creds,
                                    &offer,
                                    wants_compressed,
                                    config.muted,
                                    false,
                                    &remote_sources,
                                ) {
                                    let _ = signal.send_frame(&f).await;
                                }
                            }
                        }
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::SetBlurBackground(blur)) => {
                        config.blur_background = blur;
                        snap.blur_background = blur;
                        if let Some(cap) = &camera_capture {
                            cap.set_blur(blur);
                        }
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::SetScreenShare {
                        source_id,
                        share_audio,
                    }) => {
                        if let Some(mut prev) = content_share.take() {
                            prev.stop_and_wait().await;
                        }
                        if let Some(src_id) = source_id {
                            // Native `#content` second Chime session — runs simultaneously with camera!
                            match ContentShareSession::start(
                                creds.clone(),
                                src_id.clone(),
                                share_audio,
                                local_screen_tx.clone(),
                                Arc::clone(&perf),
                            )
                            .await
                            {
                                Ok(cs) => {
                                    snap.screen_audio_device =
                                        cs.loopback_device().map(ToOwned::to_owned);
                                    content_share = Some(cs);
                                    snap.sharing = true;
                                    snap.share_source_id = Some(src_id.clone());
                                    snap.error = None;
                                    if let Some(cap) = &camera_capture {
                                        cap.set_compact(true);
                                    }
                                    // Announce desktop draw capability + content source details on
                                    // official `"data-channel"` (`desktop:screen` / `desktop:window`
                                    // is required by Official Slack to enable remote drawing!).
                                    let cap_bytes =
                                        build_capabilities_message(&creds.attendee_id, true);
                                    let _ = signal
                                        .send_frame(&build_data_message_frame(
                                            SLACK_DATA_CHANNEL_TOPIC,
                                            cap_bytes,
                                            300_000,
                                        ))
                                        .await;
                                    let det_bytes = build_content_share_details_message(
                                        &creds.attendee_id,
                                        content_share_source_type(&src_id),
                                    );
                                    let _ = signal
                                        .send_frame(&build_data_message_frame(
                                            SLACK_DATA_CHANNEL_TOPIC,
                                            det_bytes,
                                            300_000,
                                        ))
                                        .await;
                                }
                                Err(err) => {
                                    tracing::error!(
                                        source_id = %src_id,
                                        %err,
                                        "failed to start screen share session"
                                    );
                                    snap.sharing = false;
                                    snap.share_source_id = None;
                                    snap.error = Some(err.to_string());
                                }
                            }
                        } else {
                            snap.sharing = false;
                            snap.share_source_id = None;
                            if let Some(cap) = &camera_capture {
                                cap.set_compact(false);
                            }
                            let cap_bytes = build_capabilities_message(&creds.attendee_id, false);
                            let _ = signal
                                .send_frame(&build_data_message_frame(
                                    SLACK_DATA_CHANNEL_TOPIC,
                                    cap_bytes,
                                    30_000,
                                ))
                                .await;
                            let _ = updates.send(HuddleUpdate::VideoTileClosed(VideoTileKey::LocalScreen));
                        }
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::SendReaction {
                        emoji,
                        kind,
                        emoji_url,
                    }) => {
                        let bytes = build_reacji_message(
                            &creds.attendee_id,
                            &emoji,
                            kind,
                            emoji_url.clone(),
                        );
                        let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 4000);
                        let _ = signal.send_frame(&frame).await;
                        let _ = updates.send(HuddleUpdate::DataEvent(HuddleDataEvent::Reaction {
                            user: self_user.clone(),
                            emoji,
                            kind,
                            emoji_url,
                            at_ms: now_ms(),
                        }));
                    }
                    Some(HuddleControl::SetHandRaised(raised)) => {
                        snap.hand_raised = raised;
                        let sticky = raised.then(|| "raised_hand".to_owned());
                        snap.sticky_emoji = sticky.clone();
                        let bytes = build_sticky_emoji_awareness(
                            &creds.attendee_id,
                            &creds.call_id,
                            &self_user,
                            sticky.as_deref(),
                            now_ms() as i64,
                        );
                        let frame =
                            build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 30_000);
                        let _ = signal.send_frame(&frame).await;
                        let _ = updates.send(HuddleUpdate::DataEvent(HuddleDataEvent::HandRaise {
                            user: self_user.clone(),
                            raised,
                        }));
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::SetStickyEmoji(emoji)) => {
                        snap.sticky_emoji = emoji.clone();
                        snap.hand_raised = emoji.as_deref() == Some("raised_hand");
                        let bytes = build_sticky_emoji_awareness(
                            &creds.attendee_id,
                            &creds.call_id,
                            &self_user,
                            emoji.as_deref(),
                            now_ms() as i64,
                        );
                        let frame =
                            build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 30_000);
                        let _ = signal.send_frame(&frame).await;
                        let _ = updates.send(HuddleUpdate::DataEvent(HuddleDataEvent::StickyEmoji {
                            user: self_user.clone(),
                            emoji,
                        }));
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::SendDraw(draw_msg)) => {
                        let bytes = build_draw_message(&creds.attendee_id, &draw_msg);
                        let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 5000);
                        let _ = signal.send_frame(&frame).await;
                        let _ = updates.send(HuddleUpdate::DataEvent(HuddleDataEvent::Draw {
                            user: self_user.clone(),
                            message: draw_msg,
                            at_ms: now_ms(),
                        }));
                    }
                    Some(HuddleControl::SetJukebox(song_key)) => {
                        if let Some(mut prev) = music_player.take() {
                            prev.stop();
                        }
                        if song_key.is_some() {
                            music_player = AmbientMusicPlayer::start(
                                config.output_device.clone(),
                                song_key.as_deref(),
                            )
                            .ok();
                        }
                        snap.jukebox_song = song_key.clone();
                        let bytes = build_jukebox_message(&creds.attendee_id, song_key.as_deref());
                        let frame =
                            build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 15_000);
                        let _ = signal.send_frame(&frame).await;
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::MutePeer(peer_id)) => {
                        let bytes = build_mute_request_message(&creds.attendee_id, &peer_id);
                        let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 5000);
                        let _ = signal.send_frame(&frame).await;
                    }
                    Some(HuddleControl::SetTopic(topic)) => {
                        snap.topic = (!topic.trim().is_empty()).then(|| topic.clone());
                        let _ = updates.send(HuddleUpdate::DataEvent(HuddleDataEvent::Topic {
                            user: self_user.clone(),
                            topic,
                        }));
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::SetBackground(bg_id)) => {
                        snap.background_id = bg_id;
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::SetCaptionsEnabled(enabled)) => {
                        snap.captions_enabled = enabled;
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::SetAudioOnly(audio_only)) => {
                        snap.audio_only = audio_only;
                        vdec_audio_only.store(audio_only, Ordering::Relaxed);
                        if audio_only {
                            if let Some(mut cap) = camera_capture.take() {
                                cap.stop();
                                snap.video = false;
                                let _ = updates.send(HuddleUpdate::VideoTileClosed(
                                    VideoTileKey::LocalCamera,
                                ));
                            }
                            let removed_mids: Vec<String> =
                                remote_sources.iter().map(|(m, _)| m.clone()).collect();
                            if !removed_mids.is_empty() {
                                let upd =
                                    build_remote_video_update_frame(Vec::new(), removed_mids);
                                let _ = signal.send_frame(&upd).await;
                            }
                            remote_sources.clear();
                            if let Ok(mut rt) = vdec_routing.lock() {
                                rt.remote_sources.clear();
                            }
                            vdec_clear.store(true, Ordering::Relaxed);
                            for tile in active_remote_tiles.drain() {
                                let _ = updates.send(HuddleUpdate::VideoTileClosed(tile));
                            }
                        }
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::Leave) | None => {
                        exit_reason = Some(LoopExit::LeftCleanly);
                    }
                }
            }
        }
    }

    // 7. Clean teardown: stop captures, content share, music, send Chime LEAVE, close peer & WS.
    tracing::info!("tearing down huddle session cleanly");
    if let Some(mut cam) = camera_capture.take() {
        cam.stop();
    }
    if let Some(mut cs) = content_share.take() {
        cs.stop_and_wait().await;
    }
    if let Some(mut mp) = music_player.take() {
        mp.stop();
    }
    snap.packets_sent = peer.total_packets_sent();
    snap.packets_received = peer.total_packets_received();
    snap.phase = HuddlePhase::Leaving;
    let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
    let _ = signal.send_frame(&build_leave_frame()).await;
    let _ = tokio::time::timeout(Duration::from_millis(800), signal.next_frame()).await;
    audio_engine.stop();
    video_tx_task.abort();
    vdec_fwd.abort();
    let _ = vdec_tx.send(None);
    peer.close().await;
    drop(peer);
    signal.close().await;
    if let Some(handle) = vdec_handle {
        let _ = handle.join();
    }
    tracing::info!("huddle session teardown complete");

    exit_reason.unwrap_or(LoopExit::LeftCleanly)
}

#[derive(Default)]
struct VideoRoutingTable {
    ssrc_to_stream_id: HashMap<u32, u32>,
    remote_sources: Vec<(String, RemoteVideoSource)>,
}

/// Spawn a lightweight dispatcher that routes inbound remote video RTP packets by `ssrc` to
/// parallel per-track decoder workers (H.264 VideoToolbox / AV1 VideoToolbox or `rav1d` /
/// OpenH264) so a heavy 1080p remote screen share never head-of-line-blocks other participants'
/// cameras and decoding scales smoothly across CPU cores.
#[allow(clippy::too_many_arguments)]
fn spawn_video_decoder_thread(
    inbound_video_rx: std::sync::mpsc::Receiver<Option<InboundVideoPacket>>,
    audio_only: Arc<AtomicBool>,
    clear_decoders: Arc<AtomicBool>,
    routing: Arc<Mutex<VideoRoutingTable>>,
    perf: Arc<HuddlePerfCounters>,
    updates: mpsc::UnboundedSender<HuddleUpdate>,
    keyframe_req_tx: mpsc::UnboundedSender<u32>,
    codec_tx: mpsc::UnboundedSender<String>,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("slackless-huddle-vdec".to_owned())
        .spawn(move || {
            struct TrackWorker {
                tx: std::sync::mpsc::SyncSender<InboundVideoPacket>,
                handle: std::thread::JoinHandle<()>,
            }
            let mut workers: HashMap<u32, TrackWorker> = HashMap::new();
            let mut unsupported_video_ssrcs: HashSet<u32> = HashSet::new();

            let stop_all = |workers: &mut HashMap<u32, TrackWorker>| {
                let drained: Vec<TrackWorker> = workers.drain().map(|(_, w)| w).collect();
                for w in drained {
                    drop(w.tx);
                    let _ = w.handle.join();
                }
            };

            while let Ok(Some(pkt)) = inbound_video_rx.recv() {
                if clear_decoders.swap(false, Ordering::Relaxed) {
                    stop_all(&mut workers);
                }
                if audio_only.load(Ordering::Relaxed) {
                    continue;
                }
                perf.record_rx_packet(pkt.payload.len() as u64);

                let ssrc = pkt.ssrc;
                if !workers.contains_key(&ssrc) && !unsupported_video_ssrcs.contains(&ssrc) {
                    if let Some(dec) = RemoteVideoDecoder::for_mime(&pkt.mime_type) {
                        let backend = dec.backend_name();
                        tracing::info!(
                            ssrc,
                            mid = %pkt.mid,
                            mime = %pkt.mime_type,
                            backend,
                            "initialized parallel per-track remote video decoder worker"
                        );
                        perf.set_rx_backend(backend);
                        let _ = codec_tx.send(pkt.mime_type.clone());
                        let _ = keyframe_req_tx.send(ssrc);

                        let (tx, rx) = std::sync::mpsc::sync_channel::<InboundVideoPacket>(256);
                        let w_routing = Arc::clone(&routing);
                        let w_perf = Arc::clone(&perf);
                        let w_updates = updates.clone();
                        let w_kf = keyframe_req_tx.clone();
                        if let Ok(handle) = std::thread::Builder::new()
                            .name(format!("slackless-vdec-{ssrc}"))
                            .spawn(move || {
                                run_single_track_decoder(
                                    dec, rx, w_routing, w_perf, w_updates, w_kf,
                                );
                            })
                        {
                            workers.insert(ssrc, TrackWorker { tx, handle });
                        }
                    } else {
                        unsupported_video_ssrcs.insert(ssrc);
                        tracing::warn!(
                            ssrc,
                            mid = %pkt.mid,
                            mime = %pkt.mime_type,
                            "no decoder available for inbound remote video codec"
                        );
                    }
                }

                if let Some(worker) = workers.get(&ssrc) {
                    // Non-blocking send: if one track's software decoder momentarily falls behind,
                    // drop only that track's overflow packet without stalling other tracks.
                    let _ = worker.tx.try_send(pkt);
                }
            }
            stop_all(&mut workers);
        })
        .ok()
}

fn run_single_track_decoder(
    mut decoder: RemoteVideoDecoder,
    rx: std::sync::mpsc::Receiver<InboundVideoPacket>,
    routing: Arc<Mutex<VideoRoutingTable>>,
    perf: Arc<HuddlePerfCounters>,
    updates: mpsc::UnboundedSender<HuddleUpdate>,
    keyframe_req_tx: mpsc::UnboundedSender<u32>,
) {
    let mut decoded_first = false;
    let mut last_keyframe_req = Instant::now();
    let mut last_decode_err_log: Option<Instant> = None;

    while let Ok(pkt) = rx.recv() {
        let t0 = Instant::now();
        let (frame_opt, decode_err) = decoder.push(&pkt);
        if let Some(err) = &decode_err {
            if last_decode_err_log.is_none_or(|t| t.elapsed() >= Duration::from_secs(3)) {
                last_decode_err_log = Some(Instant::now());
                tracing::warn!(
                    ssrc = pkt.ssrc,
                    mid = %pkt.mid,
                    mime = %pkt.mime_type,
                    %err,
                    "remote video decode error; requesting keyframe"
                );
            }
        }
        let needs_keyframe = decode_err.is_some() || (!decoded_first && pkt.marker);
        if needs_keyframe && last_keyframe_req.elapsed() >= Duration::from_millis(1500) {
            last_keyframe_req = Instant::now();
            let _ = keyframe_req_tx.send(pkt.ssrc);
        }
        if let Some(frame) = frame_opt {
            let decode_us = t0.elapsed().as_micros() as u64;
            perf.record_rx_frame(frame.width, frame.height, decode_us);
            let tile = routing.lock().ok().and_then(|rt| {
                resolve_remote_video_tile(
                    pkt.ssrc,
                    &pkt.mid,
                    &rt.ssrc_to_stream_id,
                    &rt.remote_sources,
                )
            });
            if let Some(tile) = tile {
                if !decoded_first {
                    decoded_first = true;
                    tracing::info!(
                        ssrc = pkt.ssrc,
                        mid = %pkt.mid,
                        mime = %pkt.mime_type,
                        width = frame.width,
                        height = frame.height,
                        decode_us,
                        tile = ?tile,
                        "decoded first remote video frame"
                    );
                }
                let _ = updates.send(HuddleUpdate::VideoFrame { tile, frame });
            } else if last_decode_err_log.is_none_or(|t| t.elapsed() >= Duration::from_secs(3)) {
                last_decode_err_log = Some(Instant::now());
                tracing::warn!(
                    ssrc = pkt.ssrc,
                    mid = %pkt.mid,
                    "decoded remote video frame but could not map to a tile"
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_index_frame(
    index: &SdkIndexFrame,
    creds: &HuddleCredentials,
    wants_compressed: bool,
    audio_muted: bool,
    send_video: bool,
    audio_only: bool,
    self_content_attendee: &str,
    attendee_to_user: &HashMap<String, UserId>,
    peer: &ChimeRtcPeer,
    signal: &mut ChimeSignalClient,
    remote_sources: &mut Vec<(String, RemoteVideoSource)>,
    vdec_routing: &Arc<Mutex<VideoRoutingTable>>,
    active_remote_tiles: &mut HashSet<VideoTileKey>,
    updates: &mpsc::UnboundedSender<HuddleUpdate>,
) {
    // Ignore our own camera and our own `#content` screen share.
    let next_raw: Vec<RemoteVideoSource> = if audio_only {
        Vec::new()
    } else {
        parse_index_sources_with_lookup(index, &creds.attendee_id, attendee_to_user)
            .into_iter()
            .filter(|s| s.attendee_id != self_content_attendee)
            .collect()
    };

    let next_tiles: HashSet<VideoTileKey> = next_raw
        .iter()
        .map(|s| {
            if s.is_content {
                VideoTileKey::RemoteScreen(s.user_id.clone())
            } else {
                VideoTileKey::RemoteCamera(s.user_id.clone())
            }
        })
        .collect();
    for removed in active_remote_tiles.difference(&next_tiles) {
        let _ = updates.send(HuddleUpdate::VideoTileClosed(removed.clone()));
    }

    let cur_raw: Vec<&RemoteVideoSource> = remote_sources.iter().map(|(_, s)| s).collect();
    let same_sources = same_remote_video_sources(&cur_raw, &next_raw);

    if !same_sources {
        tracing::info!(
            raw_descriptors = index.sources.len(),
            deduped = next_raw.len(),
            codec_intersection = ?index.supported_receive_codec_intersection,
            sources = ?next_raw,
            "Chime INDEX updated remote video sources"
        );
        if next_raw.is_empty() {
            let removed_mids: Vec<String> = remote_sources.iter().map(|(m, _)| m.clone()).collect();
            if !removed_mids.is_empty() {
                let upd = build_remote_video_update_frame(Vec::new(), removed_mids);
                if let Err(err) = signal.send_frame(&upd).await {
                    tracing::warn!(%err, "failed to send REMOTE_VIDEO_UPDATE unsubscribe");
                }
            }
            remote_sources.clear();
            if let Ok(mut rt) = vdec_routing.lock() {
                rt.remote_sources.clear();
            }
        } else {
            match peer.prepare_video_receive_offer(next_raw.len()).await {
                Ok((offer, mids)) => {
                    let paired: Vec<(String, RemoteVideoSource)> =
                        mids.into_iter().zip(next_raw).collect();
                    tracing::info!(
                        count = paired.len(),
                        mids = ?paired.iter().map(|(m, s)| (m.as_str(), s.stream_id, s.is_content)).collect::<Vec<_>>(),
                        send_video,
                        "sending renegotiation SUBSCRIBE for remote video/screen sources"
                    );
                    match build_subscribe_frame(
                        creds,
                        &offer,
                        wants_compressed,
                        audio_muted,
                        send_video,
                        &paired,
                    ) {
                        Ok(sub_f) => {
                            if let Err(err) = signal.send_frame(&sub_f).await {
                                tracing::error!(%err, "failed to send renegotiation SUBSCRIBE");
                            } else {
                                *remote_sources = paired.clone();
                                if let Ok(mut rt) = vdec_routing.lock() {
                                    rt.remote_sources = paired;
                                }
                            }
                        }
                        Err(err) => {
                            tracing::error!(%err, "failed to build renegotiation SUBSCRIBE");
                        }
                    }
                }
                Err(err) => {
                    tracing::error!(%err, "prepare_video_receive_offer failed");
                }
            }
        }
    }
    *active_remote_tiles = next_tiles;
}

fn resolve_remote_video_tile(
    ssrc: u32,
    mid: &str,
    ssrc_to_stream: &HashMap<u32, u32>,
    sources: &[(String, RemoteVideoSource)],
) -> Option<VideoTileKey> {
    if !mid.is_empty() {
        if let Some((_, src)) = sources.iter().find(|(m, _)| m == mid) {
            return Some(if src.is_content {
                VideoTileKey::RemoteScreen(src.user_id.clone())
            } else {
                VideoTileKey::RemoteCamera(src.user_id.clone())
            });
        }
    }
    if let Some(&sid) = ssrc_to_stream.get(&ssrc) {
        if let Some((_, src)) = sources.iter().find(|(_, s)| s.stream_id == sid) {
            return Some(if src.is_content {
                VideoTileKey::RemoteScreen(src.user_id.clone())
            } else {
                VideoTileKey::RemoteCamera(src.user_id.clone())
            });
        }
    }
    sources.first().map(|(_, src)| {
        if src.is_content {
            VideoTileKey::RemoteScreen(src.user_id.clone())
        } else {
            VideoTileKey::RemoteCamera(src.user_id.clone())
        }
    })
}

fn parse_incoming_data_message(
    msg: &crate::proto::SdkDataMessagePayload,
    self_attendee_id: &str,
) -> Vec<HuddleDataEvent> {
    let topic = msg.topic.as_deref().unwrap_or("");
    let data = msg.data.as_deref().unwrap_or(&[]);
    let at_ms = msg
        .ingest_time_ns
        .map(|ns| (ns.max(0) as u64) / 1_000_000)
        .unwrap_or_else(now_ms);

    if topic == CHIME_TRANSCRIPTION_TOPIC {
        return decode_chime_transcript_frame(data, at_ms);
    }

    let sender = msg
        .sender_external_user_id
        .as_deref()
        .and_then(parse_external_user_id)
        .unwrap_or_else(|| UserId::from("U0PEER"));
    let sender_att = msg.sender_attendee_id.as_deref().unwrap_or("");

    if topic == SLACK_DATA_CHANNEL_TOPIC {
        return decode_slack_peer_message(&sender, sender_att, self_attendee_id, data, at_ms);
    }

    Vec::new()
}

/// Official Slack `ContentShareDetails.source_type` string expected by Slack desktop/web clients
/// to unlock remote drawing (`["desktop:screen", "desktop:window"]`).
fn content_share_source_type(source_id: &str) -> &'static str {
    if source_id.starts_with("monitor:") {
        "desktop:screen"
    } else {
        "desktop:window"
    }
}

/// Compare two remote video source lists by stream identity, ignoring transient
/// `max_bitrate_kbps` updates from Chime's `INDEX` frames so bitrate fluctuations never trigger
/// disruptive WebRTC SDP renegotiations.
fn same_remote_video_sources(cur: &[&RemoteVideoSource], next: &[RemoteVideoSource]) -> bool {
    cur.len() == next.len()
        && cur.iter().zip(next).all(|(a, b)| {
            a.stream_id == b.stream_id
                && a.group_id == b.group_id
                && a.attendee_id == b.attendee_id
                && a.user_id == b.user_id
                && a.is_content == b.is_content
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_share_source_type_matches_official_slack_desktop_values() {
        assert_eq!(content_share_source_type("monitor:2"), "desktop:screen");
        assert_eq!(content_share_source_type("window:12345"), "desktop:window");
    }

    #[test]
    fn same_remote_video_sources_ignores_bitrate_fluctuations() {
        let a = RemoteVideoSource {
            stream_id: 21,
            group_id: 2,
            attendee_id: "att-1".to_owned(),
            user_id: UserId::from("U1"),
            is_content: false,
            max_bitrate_kbps: 1400,
        };
        let b_same_diff_bps = RemoteVideoSource {
            max_bitrate_kbps: 888,
            ..a.clone()
        };
        let b_diff_stream = RemoteVideoSource {
            stream_id: 22,
            ..a.clone()
        };
        assert!(same_remote_video_sources(&[&a], &[b_same_diff_bps]));
        assert!(!same_remote_video_sources(&[&a], &[b_diff_stream]));
    }
}
