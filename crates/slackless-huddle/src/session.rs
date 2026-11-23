//! Full Amazon Chime huddle session orchestrator (signaling + WebRTC + audio + metrics + teardown).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand::Rng as _;
use slackless_types::UserId;
use tokio::sync::mpsc;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

use crate::audio::{AudioEngine, AudioSharedState};
use crate::proto::sdk_signal_frame;
use crate::rtc::ChimeRtcPeer;
use crate::signaling::{
    ChimeSignalClient, build_audio_control_frame, build_audio_subscribe_frame,
    build_client_metric_frame, build_join_frame, build_leave_frame, build_pong_frame,
    decode_sdp_answer, parse_external_user_id,
};
use crate::{
    HuddleAudioConfig, HuddleAudioLevels, HuddleControl, HuddleCredentials, HuddleError,
    HuddleHandle, HuddlePhase, HuddleSessionSnapshot, HuddleUpdate,
};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Start a native audio huddle session on the current tokio runtime.
///
/// Emits [`HuddleUpdate`] events into `updates` and returns a [`HuddleHandle`] for live mute,
/// device switch, DSP toggles, and clean Chime `LEAVE` teardown.
pub fn spawn_huddle_session(
    creds: HuddleCredentials,
    config: HuddleAudioConfig,
    updates: mpsc::UnboundedSender<HuddleUpdate>,
) -> HuddleHandle {
    let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
    tokio::spawn(run_session(creds, config, ctrl_rx, updates));
    HuddleHandle::new(ctrl_tx)
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
        sharing: false,
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
    let _ = updates.send(HuddleUpdate::State(snap.clone()));

    let outcome = run_connected_loop(&creds, &mut config, &mut snap, &mut ctrl_rx, &updates).await;

    match outcome {
        Ok(()) => {
            snap.phase = HuddlePhase::Idle;
            snap.error = None;
            let _ = updates.send(HuddleUpdate::State(snap));
        }
        Err(err) => {
            snap.phase = HuddlePhase::Failed;
            snap.udp_blocked = matches!(err, HuddleError::UdpBlocked);
            snap.error = Some(err.to_string());
            let _ = updates.send(HuddleUpdate::State(snap));
        }
    }
}

async fn run_connected_loop(
    creds: &HuddleCredentials,
    config: &mut HuddleAudioConfig,
    snap: &mut HuddleSessionSnapshot,
    ctrl_rx: &mut mpsc::UnboundedReceiver<HuddleControl>,
    updates: &mpsc::UnboundedSender<HuddleUpdate>,
) -> crate::Result<()> {
    // 1. Connect Chime signaling WebSocket and send JOIN.
    let mut signal = ChimeSignalClient::connect(creds).await?;
    let audio_session_id: u64 = rand::rng().random();
    signal
        .send_frame(&build_join_frame(audio_session_id))
        .await?;

    // 2. Await JOIN_ACK carrying TURN credentials.
    let join_ack = tokio::time::timeout(Duration::from_secs(10), async {
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
    .map_err(|_| HuddleError::Signaling("timed out waiting for JOIN_ACK".to_owned()))??;

    let turn = join_ack.turn_credentials.ok_or_else(|| {
        HuddleError::Signaling("JOIN_ACK did not include TURN credentials".to_owned())
    })?;
    let wants_compressed = join_ack.wants_compressed_sdp.unwrap_or(true);

    // 3. Build WebRTC peer with relay-only UDP TURN and gather non-trickle SDP offer.
    let (inbound_opus_tx, inbound_opus_rx) = mpsc::unbounded_channel();
    let (outbound_opus_tx, mut outbound_opus_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (pc_state_tx, mut pc_state_rx) = mpsc::unbounded_channel();
    let peer = ChimeRtcPeer::new(&turn, inbound_opus_tx, pc_state_tx).await?;
    let sdp_offer = peer.create_gathered_offer().await?;

    // 4. Send SUBSCRIBE and await SUBSCRIBE_ACK with SDP answer.
    let sub = build_audio_subscribe_frame(creds, &sdp_offer, wants_compressed, config.muted)?;
    signal.send_frame(&sub).await?;

    let sub_ack = tokio::time::timeout(Duration::from_secs(10), async {
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
    .map_err(|_| HuddleError::Signaling("timed out waiting for SUBSCRIBE_ACK".to_owned()))??;

    let sdp_answer = decode_sdp_answer(
        sub_ack.sdp_answer.as_deref(),
        sub_ack.compressed_sdp_answer.as_deref(),
    )?;
    peer.set_remote_answer(sdp_answer).await?;

    // 5. Start full-duplex audio engine (cpal + sonora AEC3/NS/AGC + Opus).
    let shared = AudioSharedState::from_config(config);
    let mut audio_engine = AudioEngine::start(
        config,
        shared.clone(),
        outbound_opus_tx.clone(),
        inbound_opus_rx,
    )?;

    // Only transition to `Connected` once WebRTC ICE + DTLS-SRTP actually connects.
    let mut ever_connected = peer.connection_state() == RTCPeerConnectionState::Connected;
    if ever_connected {
        snap.phase = HuddlePhase::Connected;
        snap.started_at_secs = now_secs();
        let _ = updates.send(HuddleUpdate::State(snap.clone()));
    }
    let connect_deadline = tokio::time::Instant::now() + Duration::from_secs(12);

    // 6. Main event loop: RTP TX, signaling RX (PING_PONG, AUDIO_STREAM_ID_INFO, AUDIO_METADATA),
    //    1s CLIENT_METRIC cadence, 100ms local RMS tick, and live control commands.
    let mut stream_users: HashMap<u32, UserId> = HashMap::new();
    let mut muted_peers: HashSet<UserId> = HashSet::new();
    let mut remote_speakers: Vec<(UserId, f32)> = Vec::new();
    let mut metric_tick = tokio::time::interval(Duration::from_secs(1));
    let mut level_tick = tokio::time::interval(Duration::from_millis(120));

    let mut leave_requested = false;
    let mut result = Ok(());

    while !leave_requested {
        let connect_timeout = async {
            if ever_connected {
                std::future::pending::<()>().await;
            } else {
                tokio::time::sleep_until(connect_deadline).await;
            }
        };
        tokio::select! {
            _ = connect_timeout => {
                result = Err(HuddleError::UdpBlocked);
                break;
            }
            Some(opus_pkt) = outbound_opus_rx.recv() => {
                let _ = peer.write_opus_frame(opus_pkt).await;
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
                            let _ = updates.send(HuddleUpdate::State(snap.clone()));
                        }
                    }
                    RTCPeerConnectionState::Disconnected => {
                        snap.phase = HuddlePhase::Reconnecting;
                        let _ = updates.send(HuddleUpdate::State(snap.clone()));
                    }
                    RTCPeerConnectionState::Failed => {
                        result = Err(HuddleError::UdpBlocked);
                        break;
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
                            result = Err(HuddleError::Protocol {
                                status: err.status.unwrap_or(500),
                                description: err.description.unwrap_or_default(),
                            });
                            break;
                        }
                    }
                    Ok(None) => {
                        break;
                    }
                    Err(e) => {
                        result = Err(e);
                        break;
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
                        let _ = updates.send(HuddleUpdate::State(snap.clone()));
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
                        let _ = updates.send(HuddleUpdate::State(snap.clone()));
                    }
                    Some(HuddleControl::Leave) | None => {
                        leave_requested = true;
                    }
                }
            }
        }
    }

    // 7. Clean teardown: send Chime LEAVE, wait briefly for LEAVE_ACK, close audio & PeerConnection.
    snap.packets_sent = peer.total_packets_sent();
    snap.packets_received = peer.total_packets_received();
    snap.phase = HuddlePhase::Leaving;
    let _ = updates.send(HuddleUpdate::State(snap.clone()));
    let _ = signal.send_frame(&build_leave_frame()).await;
    let _ = tokio::time::timeout(Duration::from_millis(800), signal.next_frame()).await;
    audio_engine.stop();
    peer.close().await;
    signal.close().await;

    result
}
