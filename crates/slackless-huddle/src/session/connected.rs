use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slackless_types::UserId;
use tokio::sync::mpsc;
use webrtc::peer_connection::RTCPeerConnectionState;

use super::controls::{apply_background_done, handle_control};
use super::handshake::perform_handshake_with;
use super::io::{
    LiveSessionFactory, OpusTrackOut, RtcSession, SessionFactory, SignalSession, SignalSink,
    VideoTrackOut,
};
use super::media::{BackgroundDone, LiveMediaDevices, MediaDevices};
use super::signaling_handlers::{
    apply_audio_stream_info, apply_index_frame, content_share_source_type, handle_signal_frame,
};
use super::supervisor::{LoopExit, classify_handshake_err, classify_signal_close, now_secs};
use super::video_rx::{VideoDecoderThreadArgs, VideoRoutingTable, spawn_video_decoder_thread};
use crate::audio::AudioSharedState;
use crate::metrics::{HuddlePerfCounters, ProcessResourceSampler};
use crate::peer_message::{
    SLACK_DATA_CHANNEL_TOPIC, build_capabilities_message, build_content_share_details_message,
};
use crate::rtc::{InboundVideoPacket, VideoSenderFeedback};
use crate::signaling::{
    RemoteVideoSource, build_client_metric_frame, build_data_message_frame, parse_external_user_id,
};
use crate::video::VideoTileKey;
use crate::{
    HuddleAudioConfig, HuddleAudioLevels, HuddleControl, HuddleCredentials, HuddleError,
    HuddlePhase, HuddleSessionSnapshot, HuddleUpdate,
};

pub(super) struct SessionRuntime<'a, M: MediaDevices = LiveMediaDevices> {
    pub(super) creds: &'a HuddleCredentials,
    pub(super) updates: &'a mpsc::UnboundedSender<HuddleUpdate>,
    pub(super) wants_compressed: bool,
    pub(super) self_user: UserId,
    pub(super) self_content_attendee: String,
    pub(super) media: M,
    /// Preserved inbound audio stream if an audio hot-swap fails to restart both the new and
    /// previous devices, so a subsequent `SetAudioDevices` can recover session audio cleanly.
    pub(super) parked_inbound_audio: Option<M::InboundAudio>,
    /// An audio device switch owns the audio hardware until its result lands on `bg_done_tx`;
    /// `SetAudioDevices` arriving meanwhile only updates the config and is chained afterwards.
    pub(super) audio_switch_in_flight: bool,
    /// Generation of the latest camera request; results of older starts are dropped on arrival.
    pub(super) camera_gen: u64,
    /// Generation of the latest screen-share request; see `camera_gen`.
    pub(super) screen_share_gen: u64,
    pub(super) shared: AudioSharedState,
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
    /// Set at teardown so decoder workers drop their backlog instead of decoding it.
    pub(super) vdec_shutdown: Arc<AtomicBool>,
    pub(super) perf: Arc<HuddlePerfCounters>,
    pub(super) outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
    pub(super) bg_done_tx: mpsc::UnboundedSender<BackgroundDone<M>>,
}

/// Announce desktop draw capability + content source details on official `"data-channel"`
/// (`desktop:screen` / `desktop:window` is required by Official Slack to enable remote drawing).
pub(super) async fn announce_screen_share_capabilities<S: SignalSink>(
    attendee_id: &str,
    signal: &mut S,
    src_id: &str,
) {
    let cap_bytes = build_capabilities_message(attendee_id, true);
    if let Err(err) = signal
        .send_frame(&build_data_message_frame(
            SLACK_DATA_CHANNEL_TOPIC,
            cap_bytes,
            300_000,
        ))
        .await
    {
        tracing::warn!(%err, "failed to announce screen share draw capabilities");
    }
    let det_bytes =
        build_content_share_details_message(attendee_id, content_share_source_type(src_id));
    if let Err(err) = signal
        .send_frame(&build_data_message_frame(
            SLACK_DATA_CHANNEL_TOPIC,
            det_bytes,
            300_000,
        ))
        .await
    {
        tracing::warn!(%err, "failed to announce screen share source details");
    }
}

/// Log a recurring per-packet outbound track write failure at most once per 5 s.
#[derive(Default)]
struct WriteWarn(Option<Instant>);

impl WriteWarn {
    fn log(&mut self, err: &HuddleError, track: &'static str) {
        if self.0.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
            self.0 = Some(Instant::now());
            tracing::warn!(%err, track, "outbound track write failed");
        }
    }
}

impl<M: MediaDevices> SessionRuntime<'_, M> {
    /// Stop any active ambient jukebox player and optionally start a new track.
    pub(super) fn set_jukebox_player(
        &mut self,
        output_device: Option<String>,
        song_key: Option<&str>,
    ) {
        self.media.set_jukebox(output_device, song_key);
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
    let mut factory = LiveSessionFactory;
    run_connected_loop_with(
        &mut factory,
        creds,
        config,
        snap,
        ctrl_rx,
        pending_controls,
        updates,
        is_reconnect,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_connected_loop_with<F: SessionFactory>(
    factory: &mut F,
    creds: &HuddleCredentials,
    config: &mut HuddleAudioConfig,
    snap: &mut HuddleSessionSnapshot,
    ctrl_rx: &mut mpsc::UnboundedReceiver<HuddleControl>,
    pending_controls: &mut VecDeque<HuddleControl>,
    updates: &mpsc::UnboundedSender<HuddleUpdate>,
    is_reconnect: bool,
) -> LoopExit {
    let hs = match perform_handshake_with(factory, creds, config, snap, is_reconnect).await {
        Ok(hs) => hs,
        Err(exit) => return exit,
    };

    let mut signal = hs.signal;
    let peer = hs.peer;
    let mut outbound_opus_rx = hs.outbound_opus_rx;
    let inbound_video_rx = hs.inbound_video_rx;
    let mut outbound_h264_rx = hs.outbound_h264_rx;
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
    let vdec_shutdown = Arc::new(AtomicBool::new(false));
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
        shutdown: Arc::clone(&vdec_shutdown),
        idle_timeout: super::video_rx::TRACK_IDLE_TIMEOUT,
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

    // A slow control or signaling arm must never hold back 20 ms audio, so Opus packets are
    // written by their own task like camera video.
    let opus_tx_task = {
        let opus_sender = peer.opus_sender();
        tokio::spawn(async move {
            let mut write_warn = WriteWarn::default();
            while let Some(opus_pkt) = outbound_opus_rx.recv().await {
                if let Err(err) = opus_sender.write_opus_frame(opus_pkt).await {
                    write_warn.log(&err, "opus");
                }
            }
        })
    };

    let mut metric_tick = tokio::time::interval(Duration::from_secs(1));
    metric_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut perf_tick = tokio::time::interval(Duration::from_secs(5));
    perf_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut level_tick = tokio::time::interval(Duration::from_millis(120));
    level_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

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

    let (bg_done_tx, mut bg_done_rx) = mpsc::unbounded_channel::<BackgroundDone<F::Media>>();
    let mut state = SessionRuntime {
        creds,
        updates,
        wants_compressed: hs.wants_compressed,
        self_user,
        self_content_attendee,
        media: hs.media,
        parked_inbound_audio: None,
        audio_switch_in_flight: false,
        camera_gen: 0,
        screen_share_gen: 0,
        shared: hs.shared,
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
        vdec_shutdown,
        perf,
        outbound_h264_tx: hs.outbound_h264_tx,
        bg_done_tx,
    };

    // Apply any AUDIO_STREAM_ID_INFO and INDEX frames captured during the initial handshake.
    for info in pending_audio_info.drain(..) {
        let _ = apply_audio_stream_info(info, &mut state);
    }
    if let Some(index) = pending_index.take() {
        apply_index_frame(&index, config, snap, &peer, &mut signal, &mut state).await;
    }

    let video_tx_task = {
        let video_sender = peer.video_sender();
        tokio::spawn(async move {
            let mut write_warn = WriteWarn::default();
            while let Some(h264_frame) = outbound_h264_rx.recv().await {
                if let Err(err) = video_sender
                    .write_h264_frame(h264_frame, Duration::from_millis(42))
                    .await
                {
                    write_warn.log(&err, "camera video");
                }
            }
        })
    };

    let mut exit_reason: Option<LoopExit> = None;
    let mut last_levels_rms_bucket: u8 = 0;
    let mut last_audio_rx = crate::audio::AudioRxSnapshot::default();
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
            Some(fb) = cam_fb_rx.recv() => {
                match fb {
                    VideoSenderFeedback::ForceKeyframe => state.media.force_camera_keyframe(),
                    VideoSenderFeedback::TargetBitrateBps(bps) => {
                        state.media.set_camera_target_bitrate(bps.clamp(350_000, 1_500_000));
                    }
                }
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
            Some(done) = bg_done_rx.recv() => {
                apply_background_done(done, config, snap, &peer, &mut signal, &mut state).await;
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
                if let Err(err) = signal.send_frame(&build_client_metric_frame(stats)).await {
                    tracing::warn!(%err, "failed to send CLIENT_METRIC");
                }
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
                // Remote audio over this window: a healthy call conceals ~0% and sits near its
                // target delay; padding packets are skipped, never concealed.
                let audio_now = state.shared.rx_stats.lock().map(|s| *s).unwrap_or(last_audio_rx);
                let audio = audio_now.since(&last_audio_rx);
                last_audio_rx = audio_now;
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
                    rx_decoders = perf_snap.rx_decoders,
                    rx_seq_gaps = perf_snap.rx_seq_gaps,
                    rx_loss_pct = format_args!("{:.1}", perf_snap.rx_loss_pct),
                    audio_rx_pkts = audio.packets,
                    audio_rx_padding = audio.padding,
                    audio_rx_errors = audio.errors,
                    audio_conceal_pct = format_args!("{:.1}", audio.pct_of_played(audio.audible_concealed_samples())),
                    audio_silence_pct = format_args!("{:.1}", audio.pct_of_played(audio.silent_concealed_samples)),
                    audio_accel_pct = format_args!("{:.2}", audio.pct_of_played(audio.accelerated_samples)),
                    audio_decel_pct = format_args!("{:.2}", audio.pct_of_played(audio.decelerated_samples)),
                    audio_delay_ms = audio.current_delay_ms,
                    audio_target_ms = audio.target_delay_ms,
                    audio_underruns = audio.underruns,
                    audio_streams = audio.streams,
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
    let camera_stop = state.media.stop_camera();
    let share_stop = state.media.stop_screen_share();
    tokio::join!(camera_stop, share_stop);
    state.media.set_jukebox(None, None);
    snap.packets_sent = peer.total_packets_sent();
    snap.packets_received = peer.total_packets_received();
    snap.phase = HuddlePhase::Leaving;
    let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
    signal
        .send_leave_and_close(Duration::from_millis(800))
        .await;
    state.media.stop_audio();
    opus_tx_task.abort();
    video_tx_task.abort();
    // Workers exit without decoding their backlog and the dispatcher drops its queue instead of
    // routing it, so the join is short; it still blocks, so it runs on the blocking pool.
    state.vdec_shutdown.store(true, Ordering::Relaxed);
    vdec_fwd.abort();
    let _ = vdec_tx.send(None);
    peer.close().await;
    drop(peer);
    if let Some(handle) = vdec_handle {
        let _ = tokio::task::spawn_blocking(move || handle.join()).await;
    }
    tracing::info!("huddle session teardown complete");

    exit_reason.unwrap_or(LoopExit::LeftCleanly)
}
