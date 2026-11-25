use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use slackless_types::UserId;
use tokio::sync::mpsc;
use webrtc::peer_connection::RTCPeerConnectionState;

use super::controls::handle_control;
use super::handshake::perform_handshake;
use super::signaling_handlers::{
    apply_index_frame, content_share_source_type, handle_signal_frame,
};
use super::supervisor::{LoopExit, classify_handshake_err, classify_signal_close, now_secs};
use super::video_rx::{VideoDecoderThreadArgs, VideoRoutingTable, spawn_video_decoder_thread};
use crate::audio::{AmbientMusicPlayer, AudioEngine, AudioSharedState};
use crate::content_share::ContentShareSession;
use crate::metrics::{HuddlePerfCounters, ProcessResourceSampler};
use crate::peer_message::{
    SLACK_DATA_CHANNEL_TOPIC, build_capabilities_message, build_content_share_details_message,
};
use crate::rtc::{InboundAudioPacket, InboundVideoPacket, VideoSenderFeedback};
use crate::signaling::{
    ChimeSignalClient, RemoteVideoSource, build_client_metric_frame, build_data_message_frame,
    parse_external_user_id,
};
use crate::video::{CameraCapture, VideoFrame, VideoTileKey};
use crate::{
    HuddleAudioConfig, HuddleAudioLevels, HuddleControl, HuddleCredentials, HuddleError,
    HuddlePhase, HuddleSessionSnapshot, HuddleUpdate,
};

pub(super) struct SessionRuntime<'a> {
    pub(super) creds: &'a HuddleCredentials,
    pub(super) updates: &'a mpsc::UnboundedSender<HuddleUpdate>,
    pub(super) wants_compressed: bool,
    pub(super) self_user: UserId,
    pub(super) self_content_attendee: String,
    pub(super) audio_engine: AudioEngine,
    /// Preserved inbound Opus receiver if an audio hot-swap fails to restart both the new and
    /// previous devices, so a subsequent `SetAudioDevices` can recover session audio cleanly.
    pub(super) parked_inbound_opus_rx: Option<mpsc::UnboundedReceiver<InboundAudioPacket>>,
    pub(super) shared: AudioSharedState,
    pub(super) camera_capture: Option<CameraCapture>,
    pub(super) content_share: Option<ContentShareSession>,
    pub(super) music_player: Option<AmbientMusicPlayer>,
    pub(super) remote_sources: Vec<(String, RemoteVideoSource)>,
    pub(super) active_remote_tiles: HashSet<VideoTileKey>,
    pub(super) stream_users: HashMap<u32, UserId>,
    pub(super) attendee_to_user: HashMap<String, UserId>,
    pub(super) muted_peers: HashSet<UserId>,
    pub(super) remote_speakers: Vec<(UserId, f32)>,
    pub(super) ssrc_to_stream_id: HashMap<u32, u32>,
    pub(super) last_sdp_answer: Option<String>,
    pub(super) vdec_routing: Arc<Mutex<VideoRoutingTable>>,
    pub(super) vdec_audio_only: Arc<AtomicBool>,
    pub(super) vdec_clear: Arc<AtomicBool>,
    pub(super) perf: Arc<HuddlePerfCounters>,
    pub(super) outbound_opus_tx: mpsc::UnboundedSender<bytes::Bytes>,
    pub(super) outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
    pub(super) local_cam_tx: mpsc::UnboundedSender<VideoFrame>,
    pub(super) local_screen_tx: mpsc::UnboundedSender<VideoFrame>,
}

/// Announce desktop draw capability + content source details on official `"data-channel"`
/// (`desktop:screen` / `desktop:window` is required by Official Slack to enable remote drawing).
pub(super) async fn announce_screen_share_capabilities(
    attendee_id: &str,
    signal: &mut ChimeSignalClient,
    src_id: &str,
) {
    let cap_bytes = build_capabilities_message(attendee_id, true);
    let _ = signal
        .send_frame(&build_data_message_frame(
            SLACK_DATA_CHANNEL_TOPIC,
            cap_bytes,
            300_000,
        ))
        .await;
    let det_bytes =
        build_content_share_details_message(attendee_id, content_share_source_type(src_id));
    let _ = signal
        .send_frame(&build_data_message_frame(
            SLACK_DATA_CHANNEL_TOPIC,
            det_bytes,
            300_000,
        ))
        .await;
}

impl SessionRuntime<'_> {
    /// Stop any active ambient jukebox player and optionally start a new track.
    pub(super) fn set_jukebox_player(
        &mut self,
        output_device: Option<String>,
        song_key: Option<&str>,
    ) {
        if let Some(mut prev) = self.music_player.take() {
            prev.stop();
        }
        if song_key.is_some() {
            self.music_player = AmbientMusicPlayer::start(output_device, song_key).ok();
        }
    }
}

pub(super) async fn run_connected_loop(
    creds: &HuddleCredentials,
    config: &mut HuddleAudioConfig,
    snap: &mut HuddleSessionSnapshot,
    ctrl_rx: &mut mpsc::UnboundedReceiver<HuddleControl>,
    pending_controls: &mut VecDeque<HuddleControl>,
    updates: &mpsc::UnboundedSender<HuddleUpdate>,
    is_reconnect: bool,
) -> LoopExit {
    let hs = match perform_handshake(creds, config, snap, is_reconnect).await {
        Ok(hs) => hs,
        Err(exit) => return exit,
    };

    let mut signal = hs.signal;
    let peer = hs.peer;
    let mut outbound_opus_rx = hs.outbound_opus_rx;
    let inbound_video_rx = hs.inbound_video_rx;
    let mut outbound_h264_rx = hs.outbound_h264_rx;
    let mut local_cam_rx = hs.local_cam_rx;
    let mut local_screen_rx = hs.local_screen_rx;
    let mut pc_state_rx = hs.pc_state_rx;
    let mut keyframe_req_rx = hs.keyframe_req_rx;
    let mut codec_rx = hs.codec_rx;
    let mut cam_fb_rx = hs.cam_fb_rx;
    let mut pending_index = hs.pending_index;
    let mut pending_audio_info = hs.pending_audio_info;

    let perf = Arc::new(HuddlePerfCounters::default());
    let mut res_sampler = ProcessResourceSampler::new();
    let vdec_audio_only = Arc::new(AtomicBool::new(snap.audio_only));
    let vdec_clear = Arc::new(AtomicBool::new(false));
    let vdec_routing = Arc::new(Mutex::new(VideoRoutingTable {
        ssrc_to_stream_id: hs.ssrc_to_stream_id.clone(),
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
    let vdec_handle = spawn_video_decoder_thread(VideoDecoderThreadArgs {
        inbound_video_rx: vdec_rx,
        audio_only: Arc::clone(&vdec_audio_only),
        clear_decoders: Arc::clone(&vdec_clear),
        routing: Arc::clone(&vdec_routing),
        perf: Arc::clone(&perf),
        updates: updates.clone(),
        keyframe_req_tx: hs.keyframe_req_tx,
        codec_tx: hs.codec_tx,
    });

    let mut ever_connected = peer.connection_state() == RTCPeerConnectionState::Connected;
    if ever_connected {
        snap.phase = HuddlePhase::Connected;
        snap.started_at_secs = now_secs();
        let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
    }
    let connect_deadline = tokio::time::Instant::now() + Duration::from_secs(12);

    let mut metric_tick = tokio::time::interval(Duration::from_secs(1));
    let mut perf_tick = tokio::time::interval(Duration::from_secs(5));
    let mut level_tick = tokio::time::interval(Duration::from_millis(120));

    let self_user =
        parse_external_user_id(&creds.external_user_id).unwrap_or_else(|| UserId::from("U0SELF"));
    let self_content_attendee = format!("{}#content", creds.attendee_id);

    if let Ok(mut su) = hs.shared.self_user.lock() {
        *su = self_user.clone();
    }
    if config.transcription_provider.is_local()
        && let Some(tap) = crate::transcribe::LocalTranscriber::try_start(
            config.transcription_provider,
            updates.clone(),
        )
        && let Ok(mut guard) = hs.shared.transcribe_tap.lock()
    {
        tracing::info!(
            provider = ?config.transcription_provider,
            "local multilingual transcriber active for huddle"
        );
        *guard = Some(Arc::new(tap));
    }

    let mut state = SessionRuntime {
        creds,
        updates,
        wants_compressed: hs.wants_compressed,
        self_user,
        self_content_attendee,
        audio_engine: hs.audio_engine,
        parked_inbound_opus_rx: None,
        shared: hs.shared,
        camera_capture: None,
        content_share: None,
        music_player: None,
        remote_sources: Vec::new(),
        active_remote_tiles: HashSet::new(),
        stream_users: HashMap::new(),
        attendee_to_user: HashMap::new(),
        muted_peers: HashSet::new(),
        remote_speakers: Vec::new(),
        ssrc_to_stream_id: hs.ssrc_to_stream_id,
        last_sdp_answer: hs.last_sdp_answer,
        vdec_routing,
        vdec_audio_only,
        vdec_clear,
        perf,
        outbound_opus_tx: hs.outbound_opus_tx,
        outbound_h264_tx: hs.outbound_h264_tx,
        local_cam_tx: hs.local_cam_tx,
        local_screen_tx: hs.local_screen_tx,
    };

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
                state.attendee_to_user.insert(base.to_owned(), uid.clone());
                if let Some(sid) = stream.audio_stream_id
                    && stream.dropped != Some(true)
                {
                    state.stream_users.insert(sid, uid.clone());
                    if stream.muted == Some(true) {
                        state.muted_peers.insert(uid);
                    }
                }
            }
        }
    }
    if let Some(index) = pending_index.take() {
        apply_index_frame(&index, config, snap, &peer, &mut signal, &mut state).await;
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
                exit_reason = Some(classify_handshake_err(HuddleError::UdpBlocked, is_reconnect));
            }
            Some(opus_pkt) = outbound_opus_rx.recv() => {
                let _ = peer.write_opus_frame(opus_pkt).await;
            }
            Some(fb) = cam_fb_rx.recv() => {
                if let Some(cap) = &state.camera_capture {
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
                        exit_reason = Some(if ever_connected || is_reconnect {
                            LoopExit::Reconnect {
                                err: HuddleError::UdpBlocked,
                                was_connected: ever_connected,
                            }
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
                        if let Some(exit) =
                            handle_signal_frame(frame, config, snap, &peer, &mut signal, &mut state)
                                .await
                        {
                            exit_reason = Some(exit);
                        }
                    }
                    Ok(None) => {
                        exit_reason = Some(classify_signal_close(ever_connected, is_reconnect));
                    }
                    Err(e) => {
                        exit_reason = Some(if ever_connected || is_reconnect {
                            LoopExit::Reconnect {
                                err: e,
                                was_connected: ever_connected,
                            }
                        } else {
                            LoopExit::Fatal(e)
                        });
                    }
                }
            }
            _ = metric_tick.tick() => {
                let stats = peer.take_metrics_snapshot(state.shared.local_rms());
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
                let perf_snap = state.perf.sample(dt, cpu_tenths, rss_mb);
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
                    rx_seq_gaps = perf_snap.rx_seq_gaps,
                    rx_loss_pct = format_args!("{:.1}", perf_snap.rx_loss_pct),
                    degraded = perf_snap.is_degraded(),
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
                let local_rms = state.shared.local_rms();
                let rms_bucket = if local_rms < 0.005 {
                    0
                } else {
                    ((local_rms * 32.0).round() as u8).clamp(1, 32)
                };
                let mut cur_muted: Vec<UserId> = state.muted_peers.iter().cloned().collect();
                cur_muted.sort();
                if rms_bucket != last_levels_rms_bucket
                    || state.remote_speakers != last_levels_speakers
                    || cur_muted != last_levels_muted
                {
                    last_levels_rms_bucket = rms_bucket;
                    last_levels_speakers = state.remote_speakers.clone();
                    last_levels_muted = cur_muted.clone();
                    if let Ok(mut active) = state.shared.active_remote_speaker.lock() {
                        *active = state
                            .remote_speakers
                            .iter()
                            .max_by(|a, b| a.1.total_cmp(&b.1))
                            .map(|(u, _)| u.clone());
                    }
                    let _ = updates.send(HuddleUpdate::AudioLevels(HuddleAudioLevels {
                        local_rms: if rms_bucket == 0 { 0.0 } else { local_rms },
                        active_speakers: state.remote_speakers.clone(),
                        muted_peers: cur_muted,
                    }));
                }
            }
            ctrl = async {
                if let Some(c) = pending_controls.pop_front() {
                    Some(c)
                } else {
                    ctrl_rx.recv().await
                }
            } => {
                if let Some(exit) =
                    handle_control(ctrl, config, snap, &peer, &mut signal, &mut state).await
                {
                    exit_reason = Some(exit);
                }
            }
        }
    }

    // 7. Clean teardown: stop captures, content share, music, send Chime LEAVE, close peer & WS.
    tracing::info!("tearing down huddle session cleanly");
    if let Some(mut cam) = state.camera_capture.take() {
        cam.stop();
    }
    if let Some(mut cs) = state.content_share.take() {
        cs.stop_and_wait().await;
    }
    if let Some(mut mp) = state.music_player.take() {
        mp.stop();
    }
    snap.packets_sent = peer.total_packets_sent();
    snap.packets_received = peer.total_packets_received();
    snap.phase = HuddlePhase::Leaving;
    let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
    signal
        .send_leave_and_close(Duration::from_millis(800))
        .await;
    state.audio_engine.stop();
    video_tx_task.abort();
    vdec_fwd.abort();
    let _ = vdec_tx.send(None);
    peer.close().await;
    drop(peer);
    if let Some(handle) = vdec_handle {
        let _ = handle.join();
    }
    tracing::info!("huddle session teardown complete");

    exit_reason.unwrap_or(LoopExit::LeftCleanly)
}
