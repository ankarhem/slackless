//! Full Amazon Chime huddle session orchestrator:
//! - Resilient signaling + auto-reconnect supervisor (1s -> 2s -> 4s backoff on unexpected drop)
//! - WebRTC DTLS-SRTP audio & H.264 video (camera + `#content` screen share)
//! - Chime `INDEX` + `REMOTE_VIDEO_UPDATE` remote camera/screen-share subscription & decode
//! - Chime `DATA_MESSAGE` interactive huddle extras (reactions, hand raise, topic, captions)

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand::Rng as _;
use slackless_types::UserId;
use tokio::sync::mpsc;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

use crate::audio::{AudioEngine, AudioSharedState};
use crate::proto::sdk_signal_frame;
use crate::rtc::{ChimeRtcPeer, InboundVideoPacket};
use crate::screen::ScreenCapture;
use crate::signaling::{
    ChimeSignalClient, RemoteVideoSource, build_audio_control_frame, build_client_metric_frame,
    build_data_message_frame, build_join_frame, build_leave_frame, build_pong_frame,
    build_remote_video_update_frame, build_subscribe_frame, build_video_subscriptions,
    decode_sdp_answer, now_ms, parse_external_user_id, parse_index_sources,
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

/// Start a native audio/video/screen-share huddle session on the current tokio runtime.
///
/// Emits [`HuddleUpdate`] events into `updates` and returns a [`HuddleHandle`] for live mute,
/// camera, background blur, screen sharing, device switch, reactions/hand raise, and clean Chime
/// `LEAVE` teardown.
pub fn spawn_huddle_session(
    creds: HuddleCredentials,
    config: HuddleAudioConfig,
    updates: mpsc::UnboundedSender<HuddleUpdate>,
) -> HuddleHandle {
    let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
    tokio::spawn(run_session(creds, config, ctrl_rx, updates));
    HuddleHandle::new(ctrl_tx)
}

enum LoopExit {
    LeftCleanly,
    Reconnect(HuddleError),
    Fatal(HuddleError),
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
        muted: config.muted,
        video: false,
        blur_background: config.blur_background,
        sharing: false,
        share_source_id: None,
        hand_raised: false,
        captions_enabled: false,
        topic: None,
        started_at_secs: now_secs(),
        thread_root_ts: creds.thread_root_ts.clone(),
        canvas_file_id: creds.canvas_file_id.clone(),
        huddle_link: creds.huddle_link.clone(),
        input_device: config.input_device.clone(),
        output_device: config.output_device.clone(),
        error: None,
        udp_blocked: false,
        packets_sent: 0,
        packets_received: 0,
        pongs_sent: 0,
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
            snap.error = None;
            let _ = updates.send(HuddleUpdate::State(Box::new(snap)));
        }
        Some(err) => {
            snap.phase = HuddlePhase::Failed;
            snap.video = false;
            snap.sharing = false;
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
    let mut signal = match ChimeSignalClient::connect(creds).await {
        Ok(s) => s,
        Err(e) => return LoopExit::Fatal(e),
    };
    let audio_session_id: u64 = rand::rng().random();
    if let Err(e) = signal.send_frame(&build_join_frame(audio_session_id)).await {
        return LoopExit::Fatal(e);
    }

    // 2. Await JOIN_ACK carrying TURN credentials.
    let join_ack = match tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(frame) = signal.next_frame().await? {
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
        Ok(Err(e)) | Err(e) => return LoopExit::Fatal(e),
    };

    let Some(turn) = join_ack.turn_credentials else {
        return LoopExit::Fatal(HuddleError::Signaling(
            "JOIN_ACK did not include TURN credentials".to_owned(),
        ));
    };
    let wants_compressed = join_ack.wants_compressed_sdp.unwrap_or(true);

    // 3. Build WebRTC peer with relay-only UDP TURN and gather non-trickle SDP offer.
    let (inbound_opus_tx, inbound_opus_rx) = mpsc::unbounded_channel();
    let (outbound_opus_tx, mut outbound_opus_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (inbound_video_tx, mut inbound_video_rx) = mpsc::unbounded_channel::<InboundVideoPacket>();
    let (outbound_h264_tx, mut outbound_h264_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (local_cam_tx, mut local_cam_rx) = mpsc::unbounded_channel::<VideoFrame>();
    let (local_screen_tx, mut local_screen_rx) = mpsc::unbounded_channel::<VideoFrame>();
    let (pc_state_tx, mut pc_state_rx) = mpsc::unbounded_channel();

    let peer = match ChimeRtcPeer::new(&turn, inbound_opus_tx, inbound_video_tx, pc_state_tx).await
    {
        Ok(p) => p,
        Err(e) => return LoopExit::Fatal(e),
    };
    let sdp_offer = match peer.create_gathered_offer().await {
        Ok(s) => s,
        Err(e) => return LoopExit::Fatal(e),
    };

    // 4. Send SUBSCRIBE and await SUBSCRIBE_ACK with SDP answer.
    let sub = match build_subscribe_frame(
        creds,
        &sdp_offer,
        wants_compressed,
        config.muted,
        snap.video || snap.sharing,
        &[],
    ) {
        Ok(f) => f,
        Err(e) => return LoopExit::Fatal(e),
    };
    if let Err(e) = signal.send_frame(&sub).await {
        return LoopExit::Fatal(e);
    }

    let sub_ack = match tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(frame) = signal.next_frame().await? {
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
        Ok(Err(e)) | Err(e) => return LoopExit::Fatal(e),
    };

    let mut ssrc_to_stream_id: HashMap<u32, u32> = HashMap::new();
    for tm in &sub_ack.tracks {
        if let (Some(ssrc), Some(sid)) = (tm.ssrc, tm.stream_id) {
            ssrc_to_stream_id.insert(ssrc, sid);
        }
    }

    let sdp_answer = match decode_sdp_answer(
        sub_ack.sdp_answer.as_deref(),
        sub_ack.compressed_sdp_answer.as_deref(),
    ) {
        Ok(s) => s,
        Err(e) => return LoopExit::Fatal(e),
    };
    if let Err(e) = peer.set_remote_answer(sdp_answer).await {
        return LoopExit::Fatal(e);
    }

    // 5. Start full-duplex audio engine (cpal + sonora AEC3/NS/AGC + Opus).
    let shared = AudioSharedState::from_config(config);
    let mut audio_engine = match AudioEngine::start(
        config,
        shared.clone(),
        outbound_opus_tx.clone(),
        inbound_opus_rx,
    ) {
        Ok(e) => e,
        Err(e) => return LoopExit::Fatal(e),
    };

    let mut camera_capture: Option<CameraCapture> = None;
    let mut screen_capture: Option<ScreenCapture> = None;
    let mut remote_sources: Vec<RemoteVideoSource> = Vec::new();
    let mut remote_decoders: HashMap<u32, H264VideoDecoder> = HashMap::new();
    let mut active_remote_tiles: HashSet<VideoTileKey> = HashSet::new();

    let mut ever_connected = peer.connection_state() == RTCPeerConnectionState::Connected;
    if ever_connected {
        snap.phase = HuddlePhase::Connected;
        snap.started_at_secs = now_secs();
        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
    }
    let connect_deadline = tokio::time::Instant::now() + Duration::from_secs(12);

    let mut stream_users: HashMap<u32, UserId> = HashMap::new();
    let mut muted_peers: HashSet<UserId> = HashSet::new();
    let mut remote_speakers: Vec<(UserId, f32)> = Vec::new();
    let mut metric_tick = tokio::time::interval(Duration::from_secs(1));
    let mut level_tick = tokio::time::interval(Duration::from_millis(120));

    let self_user =
        parse_external_user_id(&creds.external_user_id).unwrap_or_else(|| UserId::from("U0SELF"));

    let mut exit_reason: Option<LoopExit> = None;

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
            Some(h264_frame) = outbound_h264_rx.recv() => {
                let dur = if screen_capture.is_some() {
                    Duration::from_millis(125)
                } else {
                    Duration::from_millis(66)
                };
                let _ = peer.write_h264_frame(h264_frame, dur).await;
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
            Some(pkt) = inbound_video_rx.recv() => {
                let decoder = remote_decoders
                    .entry(pkt.ssrc)
                    .or_insert_with(|| H264VideoDecoder::new().expect("openh264 decoder"));
                if let Some(frame) = decoder.push_rtp_payload(&pkt.payload, pkt.marker) {
                    let tile = resolve_remote_video_tile(
                        pkt.ssrc,
                        &ssrc_to_stream_id,
                        &remote_sources,
                    );
                    if let Some(tile) = tile {
                        let _ = updates.send(HuddleUpdate::VideoFrame { tile, frame });
                    }
                }
            }
            Some(pc_state) = pc_state_rx.recv() => {
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
                            let next_sources = parse_index_sources(&index, &creds.attendee_id);
                            let next_tiles: HashSet<VideoTileKey> = next_sources
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
                            if next_sources != remote_sources {
                                let subs = build_video_subscriptions(&next_sources);
                                let upd_frame = build_remote_video_update_frame(subs, Vec::new());
                                let _ = signal.send_frame(&upd_frame).await;
                                remote_sources = next_sources;
                            }
                            active_remote_tiles = next_tiles;
                        }
                        if let Some(ack) = frame.suback {
                            for tm in &ack.tracks {
                                if let (Some(ssrc), Some(sid)) = (tm.ssrc, tm.stream_id) {
                                    ssrc_to_stream_id.insert(ssrc, sid);
                                }
                            }
                            if let Ok(answer) = decode_sdp_answer(
                                ack.sdp_answer.as_deref(),
                                ack.compressed_sdp_answer.as_deref(),
                            ) {
                                let _ = peer.set_remote_answer(answer).await;
                            }
                        }
                        if let Some(dm) = frame.data_message {
                            for msg in dm.messages {
                                if let Some(ev) = parse_data_message(&msg) {
                                    if let HuddleDataEvent::Topic { topic, .. } = &ev {
                                        snap.topic = Some(topic.clone());
                                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                                    }
                                    let _ = updates.send(HuddleUpdate::DataEvent(ev));
                                }
                            }
                        }
                        if let Some(info) = frame.audio_stream_id_info {
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
                                    stream_users.insert(sid, uid.clone());
                                    if stream.muted == Some(true) {
                                        muted_peers.insert(uid);
                                    } else {
                                        muted_peers.remove(&uid);
                                    }
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
            }
            _ = level_tick.tick() => {
                let _ = updates.send(HuddleUpdate::AudioLevels(HuddleAudioLevels {
                    local_rms: shared.local_rms(),
                    active_speakers: remote_speakers.clone(),
                    muted_peers: muted_peers.iter().cloned().collect(),
                }));
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
                        shared.aec.store(aec, Ordering::Relaxed);
                        shared.ns.store(ns, Ordering::Relaxed);
                        shared.agc.store(agc, Ordering::Relaxed);
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
                            // Stop screen share if active so outbound video track carries camera cleanly.
                            if let Some(mut sc) = screen_capture.take() {
                                sc.stop();
                                snap.sharing = false;
                                snap.share_source_id = None;
                                let _ = updates.send(HuddleUpdate::VideoTileClosed(VideoTileKey::LocalScreen));
                            }
                            if let Some(mut prev) = camera_capture.take() {
                                prev.stop();
                            }
                            match CameraCapture::start(
                                device_index,
                                blur,
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
                            if !snap.sharing {
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
                    Some(HuddleControl::SetScreenShare { source_id }) => {
                        if let Some(src_id) = source_id {
                            if let Some(mut cam) = camera_capture.take() {
                                cam.stop();
                                snap.video = false;
                                let _ = updates.send(HuddleUpdate::VideoTileClosed(VideoTileKey::LocalCamera));
                            }
                            if let Some(mut prev) = screen_capture.take() {
                                prev.stop();
                            }
                            match ScreenCapture::start(
                                src_id.clone(),
                                outbound_h264_tx.clone(),
                                local_screen_tx.clone(),
                            ) {
                                Ok(cap) => {
                                    screen_capture = Some(cap);
                                    snap.sharing = true;
                                    snap.share_source_id = Some(src_id);
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
                                    snap.sharing = false;
                                    snap.share_source_id = None;
                                    snap.error = Some(err.to_string());
                                }
                            }
                        } else {
                            if let Some(mut sc) = screen_capture.take() {
                                sc.stop();
                            }
                            snap.sharing = false;
                            snap.share_source_id = None;
                            let _ = updates.send(HuddleUpdate::VideoTileClosed(VideoTileKey::LocalScreen));
                            if !snap.video {
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
                        }
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::SendReaction(emoji)) => {
                        let frame = build_data_message_frame(
                            "huddle:reaction",
                            emoji.as_bytes().to_vec(),
                            4000,
                        );
                        let _ = signal.send_frame(&frame).await;
                        let _ = updates.send(HuddleUpdate::DataEvent(HuddleDataEvent::Reaction {
                            user: self_user.clone(),
                            emoji,
                            at_ms: now_ms(),
                        }));
                    }
                    Some(HuddleControl::SetHandRaised(raised)) => {
                        snap.hand_raised = raised;
                        let payload = if raised { b"1".to_vec() } else { b"0".to_vec() };
                        let frame = build_data_message_frame("huddle:hand", payload, 30_000);
                        let _ = signal.send_frame(&frame).await;
                        let _ = updates.send(HuddleUpdate::DataEvent(HuddleDataEvent::HandRaise {
                            user: self_user.clone(),
                            raised,
                        }));
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::SetTopic(topic)) => {
                        snap.topic = (!topic.trim().is_empty()).then(|| topic.clone());
                        let frame =
                            build_data_message_frame("huddle:topic", topic.as_bytes().to_vec(), 60_000);
                        let _ = signal.send_frame(&frame).await;
                        let _ = updates.send(HuddleUpdate::DataEvent(HuddleDataEvent::Topic {
                            user: self_user.clone(),
                            topic,
                        }));
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::SetCaptionsEnabled(enabled)) => {
                        snap.captions_enabled = enabled;
                        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    Some(HuddleControl::Leave) | None => {
                        exit_reason = Some(LoopExit::LeftCleanly);
                    }
                }
            }
        }
    }

    // 7. Clean teardown: stop captures, send Chime LEAVE, wait briefly for LEAVE_ACK, close peer & WS.
    if let Some(mut cam) = camera_capture.take() {
        cam.stop();
    }
    if let Some(mut sc) = screen_capture.take() {
        sc.stop();
    }
    snap.packets_sent = peer.total_packets_sent();
    snap.packets_received = peer.total_packets_received();
    snap.phase = HuddlePhase::Leaving;
    let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
    let _ = signal.send_frame(&build_leave_frame()).await;
    let _ = tokio::time::timeout(Duration::from_millis(800), signal.next_frame()).await;
    audio_engine.stop();
    peer.close().await;
    signal.close().await;

    exit_reason.unwrap_or(LoopExit::LeftCleanly)
}

fn resolve_remote_video_tile(
    ssrc: u32,
    ssrc_to_stream: &HashMap<u32, u32>,
    sources: &[RemoteVideoSource],
) -> Option<VideoTileKey> {
    if let Some(&sid) = ssrc_to_stream.get(&ssrc) {
        if let Some(src) = sources.iter().find(|s| s.stream_id == sid) {
            return Some(if src.is_content {
                VideoTileKey::RemoteScreen(src.user_id.clone())
            } else {
                VideoTileKey::RemoteCamera(src.user_id.clone())
            });
        }
    }
    // Fallback when Chime sends a single remote video stream before track-mapping updates.
    sources.first().map(|src| {
        if src.is_content {
            VideoTileKey::RemoteScreen(src.user_id.clone())
        } else {
            VideoTileKey::RemoteCamera(src.user_id.clone())
        }
    })
}

fn parse_data_message(msg: &crate::proto::SdkDataMessagePayload) -> Option<HuddleDataEvent> {
    let topic = msg.topic.as_deref()?;
    let data = msg.data.as_deref()?;
    let text = std::str::from_utf8(data).ok()?.trim();
    let user = msg
        .sender_external_user_id
        .as_deref()
        .and_then(parse_external_user_id)
        .unwrap_or_else(|| UserId::from(msg.sender_attendee_id.as_deref().unwrap_or("U0PEER")));
    match topic {
        "huddle:reaction" if !text.is_empty() => Some(HuddleDataEvent::Reaction {
            user,
            emoji: text.to_owned(),
            at_ms: now_ms(),
        }),
        "huddle:hand" => Some(HuddleDataEvent::HandRaise {
            user,
            raised: text == "1" || text.eq_ignore_ascii_case("true"),
        }),
        "huddle:topic" => Some(HuddleDataEvent::Topic {
            user,
            topic: text.to_owned(),
        }),
        "huddle:caption" if !text.is_empty() => Some(HuddleDataEvent::Caption {
            user,
            text: text.to_owned(),
            at_ms: now_ms(),
        }),
        _ => None,
    }
}
