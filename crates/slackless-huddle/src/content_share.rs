//! Dedicated Amazon Chime `#content` attendee session for native screen sharing (`F-COLLAB-021`).
//!
//! Matches `amazon-chime-sdk-js`'s `DefaultContentShareController` bit-for-bit:
//! appends `#content` to both `attendee_id` and `join_token`, opens a second Chime Signaling
//! WebSocket + second `RTCPeerConnection` with silent Opus audio on mid 0 and H.264 video on
//! mid 1 (`track_label = "AmazonChimeExpressVideo"`, `stream_id = 2`, `group_id = 2`,
//! `receive_stream_ids = [0]`), so webcam video and screen sharing run simultaneously and appear
//! as a native screen share on official Slack desktop/web clients.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use cpal::Sample as _;
use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use tokio::sync::mpsc;
use webrtc::peer_connection::RTCPeerConnectionState;

use crate::audio::RateConverter;
use crate::audio::ring::{SpscRing, WorkerWake};
use crate::backend::{ScreenShareConfig, VideoFrameSink, platform_backend};
use crate::metrics::HuddlePerfCounters;
use crate::proto::sdk_signal_frame;
use crate::rtc::{ChimeRtcPeer, VideoSenderFeedback};
use crate::screen::{SCREEN_BITRATE_BPS, SCREEN_MAX_FPS, SCREEN_MAX_H, SCREEN_MAX_W};
use crate::signaling::{
    ChimeSignalClient, ClientMetricsSnapshot, PLACEHOLDER_STUN_RTT_MS, build_client_metric_frame,
    build_content_subscribe_frame, build_join_frame, decode_sdp_answer,
};
use crate::{HuddleCredentials, HuddleError, Result};

const CONTENT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Capacity of the lock-free stereo 48 kHz loopback PCM ring (~2.7 s of interleaved stereo i16).
const LOOPBACK_RING_CAPACITY: usize = 262_144;
/// Consumer-side high-water mark (2 s of 48 kHz stereo = `48_000 * 4` i16 samples).
const LOOPBACK_OVERFLOW_THRESHOLD: usize = 48_000 * 4;
const LOOPBACK_OVERFLOW_DROP: usize = 48_000;

/// Returns `true` when an audio input device name matches a known virtual loopback driver.
pub(crate) fn is_loopback_device_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("blackhole")
        || lower.contains("loopback")
        || lower.contains("soundflower")
        || lower.contains("vb-cable")
        || lower.contains("cable output")
        || lower.contains("monitor of")
}

/// Derive `#content` attendee credentials from base huddle credentials.
pub(crate) fn content_credentials(base_creds: HuddleCredentials) -> HuddleCredentials {
    HuddleCredentials {
        attendee_id: format!("{}#content", base_creds.attendee_id),
        join_token: format!("{}#content", base_creds.join_token),
        ..base_creds
    }
}

/// Compute an updated screen-share ABR target bitrate (clamped `800 kbps..=3.5 Mbps`) when the
/// change exceeds the 200 kbps hysteresis threshold; returns `None` when no update is needed.
pub(crate) fn compute_content_abr_bitrate(target_bps: u32, last_abr_bps: u32) -> Option<u32> {
    let clamped = target_bps.clamp(800_000, 3_500_000);
    (clamped.abs_diff(last_abr_bps) >= 200_000).then_some(clamped)
}

/// Consumer-side overflow trim for the lock-free stereo loopback ring: discards 0.5 s of stereo
/// samples (`48_000` interleaved i16) whenever queued latency exceeds 2 s (`48_000 * 4` samples).
pub(crate) fn trim_loopback_ring_overflow(ring: &SpscRing<i16>) {
    if ring.len() > LOOPBACK_OVERFLOW_THRESHOLD {
        ring.skip(LOOPBACK_OVERFLOW_DROP);
    }
}

/// Detect an installed virtual system-audio loopback input device (`BlackHole`, `Loopback`,
/// `Soundflower`, `VB-Cable`, PulseAudio/PipeWire `Monitor of ...`) for screen-share audio
/// capture (`F-COLLAB-022`).
pub fn detect_system_audio_loopback() -> Option<String> {
    let host = cpal::default_host();
    let inputs = host.input_devices().ok()?;
    for dev in inputs {
        let Some(name) = crate::audio::cpal_device_name(&dev) else {
            continue;
        };
        if is_loopback_device_name(&name) {
            return Some(name);
        }
    }
    None
}

/// Human-readable status of system audio capture for screen sharing (`F-COLLAB-022`).
/// Prefers OS-native system audio tap (e.g. macOS 14.2+ CoreAudio Process Tap), then virtual loopback.
pub fn system_audio_source_label() -> Option<String> {
    platform_backend()
        .native_system_audio_label()
        .map(str::to_owned)
        .or_else(detect_system_audio_loopback)
}

/// Running `#content` screen-share session handle.
pub struct ContentShareSession {
    stop_tx: Option<tokio::sync::oneshot::Sender<()>>,
    done_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    loopback_device: Option<String>,
}

impl ContentShareSession {
    /// Name of the system audio loopback device feeding `#content` audio, if detected.
    pub fn loopback_device(&self) -> Option<&str> {
        self.loopback_device.as_deref()
    }

    /// Start a native `#content` attendee sharing `source_id`, handing local preview frames to
    /// `on_preview`. When `share_audio` is `true`, captures system audio via the platform
    /// backend's native system-audio tap or a virtual loopback device.
    pub async fn start(
        base_creds: HuddleCredentials,
        source_id: String,
        share_audio: bool,
        on_preview: VideoFrameSink,
        perf: Arc<HuddlePerfCounters>,
    ) -> Result<Self> {
        let content_creds = content_credentials(base_creds);

        tracing::info!(
            source_id = %source_id,
            attendee = %content_creds.attendee_id,
            "starting #content screen share session"
        );
        let mut sig = ChimeSignalClient::connect(&content_creds).await?;
        let audio_session_id: u64 = rand::random();
        if let Err(err) = sig.send_frame(&build_join_frame(audio_session_id)).await {
            sig.close().await;
            return Err(err);
        }

        let join_ack = match sig
            .wait_for_join_ack(CONTENT_HANDSHAKE_TIMEOUT, |_| {})
            .await
        {
            Ok(ack) => ack,
            Err(err) => {
                sig.send_leave_and_close(Duration::from_millis(300)).await;
                return Err(err);
            }
        };
        let Some(turn) = join_ack.turn_credentials.as_ref() else {
            sig.send_leave_and_close(Duration::from_millis(300)).await;
            return Err(HuddleError::Signaling(
                "content JOIN_ACK missing TURN creds".into(),
            ));
        };

        let (state_tx, mut state_rx) = mpsc::unbounded_channel();
        let rtc = match ChimeRtcPeer::new_content_sender(turn, state_tx).await {
            Ok(rtc) => Arc::new(rtc),
            Err(err) => {
                sig.send_leave_and_close(Duration::from_millis(300)).await;
                return Err(err);
            }
        };

        type ContentShareSetup = (
            mpsc::UnboundedReceiver<Vec<u8>>,
            mpsc::UnboundedReceiver<VideoSenderFeedback>,
            Box<dyn crate::backend::ScreenShareHandle>,
        );
        let setup_res: Result<ContentShareSetup> = async {
            let offer_sdp = rtc.create_gathered_offer().await?;
            let sub_frame = build_content_subscribe_frame(&content_creds, &offer_sdp, false)?;
            sig.send_frame(&sub_frame).await?;

            let sub_ack = sig
                .wait_for_subscribe_ack(CONTENT_HANDSHAKE_TIMEOUT, |_| {})
                .await?;
            let answer = decode_sdp_answer(
                sub_ack.sdp_answer.as_deref(),
                sub_ack.compressed_sdp_answer.as_deref(),
            )?;
            rtc.set_remote_answer(answer).await?;
            tracing::info!(
                source_id = %source_id,
                "#content SUBSCRIBE_ACK applied; starting screen capture"
            );

            let (h264_tx, h264_rx) = mpsc::unbounded_channel::<Vec<u8>>();
            let (video_fb_tx, video_fb_rx) = mpsc::unbounded_channel::<VideoSenderFeedback>();
            rtc.attach_video_feedback(video_fb_tx).await;
            let capture = platform_backend().start_screen_share(
                ScreenShareConfig {
                    source_id,
                    max_w: SCREEN_MAX_W,
                    max_h: SCREEN_MAX_H,
                    max_fps: SCREEN_MAX_FPS,
                    bitrate_bps: SCREEN_BITRATE_BPS,
                },
                h264_tx,
                on_preview,
                Arc::clone(&perf),
            )?;
            Ok((h264_rx, video_fb_rx, capture))
        }
        .await;

        let (mut h264_rx, mut video_fb_rx, capture) = match setup_res {
            Ok(ok) => ok,
            Err(err) => {
                sig.send_leave_and_close(Duration::from_millis(300)).await;
                rtc.close().await;
                return Err(err);
            }
        };
        let video = rtc.video_sender();
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

        let running = Arc::new(AtomicBool::new(true));
        let running_audio = Arc::clone(&running);
        let opus = rtc.opus_sender();
        let loopback_pcm = Arc::new(SpscRing::<i16>::new(LOOPBACK_RING_CAPACITY));
        let loopback_capture_running = Arc::clone(&running);
        let loopback_wake = Arc::new(WorkerWake::default());
        let loopback_wake_worker = Arc::clone(&loopback_wake);
        let loopback_pcm_tx = Arc::clone(&loopback_pcm);

        // Priority 1: Zero-driver OS-native system audio tap (`F-COLLAB-022`).
        let backend = platform_backend();
        let native_tap = if share_audio {
            match backend.start_system_audio_tap(Arc::clone(&loopback_pcm)) {
                Some(Ok(tap)) => Some(tap),
                Some(Err(err)) => {
                    tracing::warn!(
                        %err,
                        "native system audio tap unavailable; continuing without native tap"
                    );
                    None
                }
                None => None,
            }
        } else {
            None
        };

        let loopback_device = if !share_audio {
            None
        } else if native_tap.is_some() {
            backend.native_system_audio_label().map(str::to_owned)
        } else {
            detect_system_audio_loopback()
        };

        let loopback_rate = Arc::new(AtomicU32::new(
            native_tap
                .as_ref()
                .map_or(48_000, |t| t.sample_rate().clamp(8_000, 384_000)),
        ));
        let loopback_rate_writer = Arc::clone(&loopback_rate);

        if native_tap.is_none()
            && let Some(dev_name) = loopback_device.clone()
        {
            let _ = std::thread::Builder::new()
                .name("slackless-screen-audio".to_owned())
                .spawn(move || {
                    let host = cpal::default_host();
                    let Some(dev) = host.input_devices().ok().and_then(|mut it| {
                        it.find(|d| {
                            crate::audio::cpal_device_name(d).as_deref() == Some(dev_name.as_str())
                        })
                    }) else {
                        return;
                    };
                    let Ok(supported) = dev.default_input_config() else {
                        return;
                    };
                    loopback_rate_writer.store(
                        supported.sample_rate().clamp(8_000, 384_000),
                        Ordering::Release,
                    );
                    let ch = usize::from(supported.channels()).max(1);
                    let stream =
                        crate::audio::build_input_stream_any_format(&dev, supported, move |data| {
                            // Stack-buffered stereo i16: zero heap allocs or locks in the RT callback.
                            let mut buf = [0i16; 1024];
                            let mut n = 0usize;
                            for frame in data.chunks(ch) {
                                let l = frame[0];
                                let r = frame.get(1).copied().unwrap_or(l);
                                buf[n] = i16::from_sample(l.clamp(-1.0, 1.0));
                                buf[n + 1] = i16::from_sample(r.clamp(-1.0, 1.0));
                                n += 2;
                                if n == buf.len() {
                                    loopback_pcm_tx.push_slice(&buf);
                                    n = 0;
                                }
                            }
                            if n > 0 {
                                loopback_pcm_tx.push_slice(&buf[..n]);
                            }
                        });
                    let stream = match stream {
                        Ok(stream) => stream,
                        Err(err) => {
                            tracing::warn!(%err, "screen-share loopback capture unavailable");
                            return;
                        }
                    };
                    if let Err(err) = stream.play() {
                        tracing::warn!(%err, "screen-share loopback capture failed to start");
                        return;
                    }
                    loopback_wake_worker.park_until_cleared(&loopback_capture_running);
                });
        }
        // Feed live system audio (CoreAudio tap or loopback, `F-COLLAB-022`) resampled to 48 kHz
        // stereo when the capture device runs at another rate (e.g. AirPods 16/24 kHz or DAC
        // 44.1/96 kHz), or a cached 20ms stereo Opus silence packet when `share_audio == false`.
        let audio_task = tokio::spawn(async move {
            let Ok(mut enc) =
                opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio)
            else {
                return;
            };
            let _ = enc.set_bitrate(opus::Bitrate::Bits(96_000));
            let _ = enc.set_vbr(true);
            let _ = enc.set_inband_fec(true);
            let silence = vec![0i16; 960 * 2];
            let mut pcm = vec![0i16; 960 * 2];
            let mut raw_buf = vec![0i16; 4096 * 2];
            let mut resampled_fifo: std::collections::VecDeque<i16> =
                std::collections::VecDeque::with_capacity(960 * 8);
            let mut resampler_rate: u32 = 48_000;
            let mut resamplers: Option<(RateConverter, RateConverter)> = None;
            let mut l_in: Vec<f32> = Vec::new();
            let mut r_in: Vec<f32> = Vec::new();
            let mut l_out: Vec<f32> = Vec::new();
            let mut r_out: Vec<f32> = Vec::new();
            let mut out = vec![0u8; 4000];
            let cached_silence: bytes::Bytes = enc
                .encode(&silence, &mut out)
                .map(|n| bytes::Bytes::copy_from_slice(&out[..n]))
                .unwrap_or_else(|_| bytes::Bytes::from_static(&[0xFC, 0xFF, 0xFE]));
            let mut tick = tokio::time::interval(Duration::from_millis(20));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            while running_audio.load(Ordering::Relaxed) {
                tick.tick().await;
                let has_audio = if share_audio {
                    trim_loopback_ring_overflow(&loopback_pcm);
                    let src_rate = loopback_rate.load(Ordering::Acquire);
                    if src_rate == 48_000 {
                        if loopback_pcm.len() >= pcm.len() {
                            loopback_pcm.pop_slice(&mut pcm) == pcm.len()
                        } else {
                            false
                        }
                    } else {
                        if resampler_rate != src_rate || resamplers.is_none() {
                            resampler_rate = src_rate;
                            resampled_fifo.clear();
                            let chunk = (src_rate as usize / 100).max(16);
                            resamplers =
                                RateConverter::new(src_rate, 48_000, chunk)
                                    .ok()
                                    .and_then(|l| {
                                        RateConverter::new(src_rate, 48_000, chunk)
                                            .ok()
                                            .map(|r| (l, r))
                                    });
                        }
                        if let Some((conv_l, conv_r)) = resamplers.as_mut() {
                            let avail_pairs = (loopback_pcm.len() / 2).min(raw_buf.len() / 2);
                            if avail_pairs > 0 {
                                let popped =
                                    loopback_pcm.pop_slice(&mut raw_buf[..avail_pairs * 2]);
                                let pairs = popped / 2;
                                l_in.clear();
                                r_in.clear();
                                l_out.clear();
                                r_out.clear();
                                for frame in raw_buf[..pairs * 2].as_chunks::<2>().0 {
                                    l_in.push(f32::from(frame[0]) / 32_768.0);
                                    r_in.push(f32::from(frame[1]) / 32_768.0);
                                }
                                conv_l.process(&l_in, &mut l_out);
                                conv_r.process(&r_in, &mut r_out);
                                let n = l_out.len().min(r_out.len());
                                for i in 0..n {
                                    resampled_fifo
                                        .push_back(i16::from_sample(l_out[i].clamp(-1.0, 1.0)));
                                    resampled_fifo
                                        .push_back(i16::from_sample(r_out[i].clamp(-1.0, 1.0)));
                                }
                                // Keep resampled latency bounded to <= 1s.
                                while resampled_fifo.len() > 48_000 * 2 {
                                    resampled_fifo.drain(..960 * 2);
                                }
                            }
                            if resampled_fifo.len() >= pcm.len() {
                                for dst in &mut pcm {
                                    *dst = resampled_fifo.pop_front().unwrap_or(0);
                                }
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    }
                } else {
                    false
                };
                if has_audio {
                    if let Ok(n) = enc.encode(&pcm, &mut out) {
                        let _ = opus
                            .write_opus_frame(bytes::Bytes::copy_from_slice(&out[..n]))
                            .await;
                    }
                } else {
                    let _ = opus.write_opus_frame(cached_silence.clone()).await;
                }
            }
        });

        tokio::spawn(async move {
            let capture = capture;
            let native_tap = native_tap;
            let mut metric_tick = tokio::time::interval(Duration::from_secs(1));
            metric_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut connected = false;
            let mut sent_first_frame = false;
            let mut last_abr_bps: u32 = SCREEN_BITRATE_BPS;

            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    Some(fb) = video_fb_rx.recv() => {
                        match fb {
                            VideoSenderFeedback::ForceKeyframe => {
                                capture.force_keyframe();
                            }
                            VideoSenderFeedback::TargetBitrateBps(bps) => {
                                if let Some(clamped) =
                                    compute_content_abr_bitrate(bps, last_abr_bps)
                                {
                                    last_abr_bps = clamped;
                                    capture.set_target_bitrate(clamped);
                                }
                            }
                        }
                    }
                    Some(st) = state_rx.recv() => {
                        tracing::info!(state = ?st, "#content peer connection state");
                        match st {
                            RTCPeerConnectionState::Connected => {
                                connected = true;
                                // Force an immediate IDR keyframe once WebRTC media is open.
                                capture.force_keyframe();
                            }
                            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => break,
                            _ => {}
                        }
                    }
                    frame = h264_rx.recv() => {
                        let Some(annex_b) = frame else {
                            // Screen/window capture ended (e.g. shared window closed): tear down `#content`.
                            tracing::info!("#content capture channel closed; ending screen share");
                            break;
                        };
                        let (annex_b, dropped, needs_keyframe) = crate::video::trim_h264_backlog(
                            annex_b,
                            &mut h264_rx,
                            crate::video::MAX_H264_EGRESS_BACKLOG,
                        );
                        if dropped > 0 {
                            tracing::warn!(
                                dropped,
                                needs_keyframe,
                                "#content H.264 egress backlog trimmed"
                            );
                            if needs_keyframe {
                                capture.force_keyframe();
                            }
                        }
                        if connected {
                            match video.write_h264_frame(annex_b, Duration::from_millis(33)).await {
                                Ok(()) if !sent_first_frame => {
                                    sent_first_frame = true;
                                    tracing::info!("#content first H.264 screen frame sent");
                                }
                                Ok(()) => {}
                                Err(err) => {
                                    tracing::warn!(%err, "#content write_h264_frame error");
                                }
                            }
                        }
                    }
                    _ = metric_tick.tick() => {
                        let stats = ClientMetricsSnapshot {
                            stun_rtt_ms: PLACEHOLDER_STUN_RTT_MS,
                            ..Default::default()
                        };
                        let _ = sig.send_frame(&build_client_metric_frame(stats)).await;
                    }
                    res = sig.next_frame() => {
                        match res {
                            Ok(Some(frame)) => {
                                if let Some(pp) = frame.ping_pong {
                                    let _ = sig.reply_ping_if_needed(&pp).await;
                                } else if frame.r#type == Some(sdk_signal_frame::Type::Leave as i32) {
                                    break;
                                }
                            }
                            Ok(None) | Err(_) => break,
                        }
                    }
                }
            }

            running.store(false, Ordering::Release);
            loopback_wake.wake_now();
            audio_task.abort();
            // Screen capture and CoreAudio Process Tap teardown block on OS APIs: keep both off the
            // Tokio worker pool and finish them before signalling `done_tx`.
            let _ = tokio::task::spawn_blocking(move || {
                drop(native_tap);
                drop(capture);
            })
            .await;
            perf.clear_tx();
            sig.send_leave_and_close(Duration::from_millis(300)).await;
            rtc.close().await;
            let _ = done_tx.send(());
        });

        Ok(Self {
            stop_tx: Some(stop_tx),
            done_rx: Some(done_rx),
            loopback_device,
        })
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }

    /// Stop the `#content` share and wait briefly for capture/tap teardown and Chime `LEAVE` flush.
    pub async fn stop_and_wait(&mut self) {
        self.stop();
        if let Some(rx) = self.done_rx.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), rx).await;
        }
    }
}

impl Drop for ContentShareSession {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{
        SdkErrorFrame, SdkJoinAckFrame, SdkSignalFrame, SdkSubscribeAckFrame, SdkTurnCredentials,
    };
    use crate::signaling::{build_pong_frame, encode_wire_frame, now_ms};
    use futures_util::SinkExt as _;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_hdr_async;
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
    use tokio_tungstenite::tungstenite::http::HeaderValue;

    fn sample_creds(url: String) -> HuddleCredentials {
        HuddleCredentials {
            channel: slackless_types::ChannelId::from("C1"),
            call_id: "R1".into(),
            meeting_id: "M1".into(),
            attendee_id: "att-user".into(),
            external_user_id: "T1-R1-U1".into(),
            join_token: "tok-user".into(),
            signaling_url: url,
            audio_host_url: "audio.invalid:3478".into(),
            turn_control_url: String::new(),
            thread_root_ts: None,
            canvas_file_id: None,
            huddle_link: None,
        }
    }

    #[test]
    fn loopback_device_name_detection() {
        assert!(is_loopback_device_name("BlackHole 2ch"));
        assert!(is_loopback_device_name("Loopback Audio"));
        assert!(is_loopback_device_name("Soundflower (64ch)"));
        assert!(is_loopback_device_name("VB-Cable Virtual"));
        assert!(is_loopback_device_name("CABLE Output (VB-Audio)"));
        assert!(is_loopback_device_name(
            "Monitor of Built-in Audio Analog Stereo"
        ));
        assert!(!is_loopback_device_name("MacBook Pro Microphone"));
        assert!(!is_loopback_device_name("AirPods Pro"));
    }

    #[test]
    fn content_credentials_appends_content_suffix() {
        let base = sample_creds("wss://example.invalid".into());
        let content = content_credentials(base.clone());
        assert_eq!(content.attendee_id, "att-user#content");
        assert_eq!(content.join_token, "tok-user#content");
        assert_eq!(content.meeting_id, base.meeting_id);
        assert_eq!(content.external_user_id, base.external_user_id);
    }

    #[test]
    fn abr_hysteresis_clamps_and_suppresses_small_jitter() {
        // Below hysteresis threshold (200k) -> None.
        assert_eq!(compute_content_abr_bitrate(3_100_000, 3_000_000), None);
        assert_eq!(compute_content_abr_bitrate(2_800_001, 3_000_000), None);
        // At or above threshold -> Some(clamped).
        assert_eq!(
            compute_content_abr_bitrate(2_800_000, 3_000_000),
            Some(2_800_000)
        );
        // Clamped to floor 800_000 and ceiling 3_500_000.
        assert_eq!(
            compute_content_abr_bitrate(100_000, 3_000_000),
            Some(800_000)
        );
        assert_eq!(
            compute_content_abr_bitrate(9_000_000, 3_000_000),
            Some(3_500_000)
        );
        // Already at ceiling -> no update.
        assert_eq!(compute_content_abr_bitrate(9_000_000, 3_500_000), None);
    }

    #[test]
    fn stereo_pcm_overflow_trim_caps_ring_latency() {
        let ring = SpscRing::<i16>::new(LOOPBACK_RING_CAPACITY);
        assert_eq!(ring.push_slice(&vec![1; 48_000 * 4]), 48_000 * 4);
        trim_loopback_ring_overflow(&ring);
        assert_eq!(ring.len(), 48_000 * 4);

        assert_eq!(ring.push_slice(&[2; 100]), 100);
        assert_eq!(ring.len(), 48_000 * 4 + 100);
        // Consumer-side trim discards oldest 48_000 samples when > 48_000 * 4.
        trim_loopback_ring_overflow(&ring);
        assert_eq!(ring.len(), 48_000 * 3 + 100);
    }

    #[tokio::test]
    #[allow(clippy::result_large_err)]
    async fn content_handshake_join_and_subscribe_ack_timeout_on_stalled_ws() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut ws = accept_hdr_async(stream, |_req: &Request, mut resp: Response| {
                        resp.headers_mut().insert(
                            "Sec-WebSocket-Protocol",
                            HeaderValue::from_static("_aws_wt_session"),
                        );
                        Ok(resp)
                    })
                    .await
                    .unwrap();
                    // Hold socket open without sending JOIN_ACK or SUBSCRIBE_ACK.
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    let _ = ws.close(None).await;
                });
            }
        });

        let creds = content_credentials(sample_creds(format!("ws://{addr}/control/M1")));

        let mut sig1 = ChimeSignalClient::connect(&creds).await.unwrap();
        let err1 = sig1
            .wait_for_join_ack(Duration::from_millis(60), |_| {})
            .await
            .unwrap_err();
        assert!(
            matches!(err1, HuddleError::Signaling(ref m) if m.contains("timed out")),
            "expected JOIN_ACK timeout, got {err1:?}"
        );
        sig1.close().await;

        let mut sig2 = ChimeSignalClient::connect(&creds).await.unwrap();
        let err2 = sig2
            .wait_for_subscribe_ack(Duration::from_millis(60), |_| {})
            .await
            .unwrap_err();
        assert!(
            matches!(err2, HuddleError::Signaling(ref m) if m.contains("timed out")),
            "expected SUBSCRIBE_ACK timeout, got {err2:?}"
        );
        sig2.close().await;

        server.await.unwrap();
    }

    #[tokio::test]
    #[allow(clippy::result_large_err)]
    async fn content_handshake_handles_ack_protocol_error_and_close() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            // Connection 1: send non-ACK ping, then JOIN_ACK, then SUBSCRIBE_ACK.
            let (s1, _) = listener.accept().await.unwrap();
            let mut ws1 = accept_hdr_async(s1, |_req: &Request, mut resp: Response| {
                resp.headers_mut().insert(
                    "Sec-WebSocket-Protocol",
                    HeaderValue::from_static("_aws_wt_session"),
                );
                Ok(resp)
            })
            .await
            .unwrap();
            ws1.send(WsMessage::Binary(
                encode_wire_frame(&build_pong_frame(1)).into(),
            ))
            .await
            .unwrap();
            let join_ack = SdkSignalFrame {
                timestamp_ms: Some(now_ms()),
                r#type: Some(sdk_signal_frame::Type::JoinAck as i32),
                joinack: Some(SdkJoinAckFrame {
                    turn_credentials: Some(SdkTurnCredentials {
                        uris: vec!["turn:127.0.0.1:3478?transport=udp".into()],
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            };
            ws1.send(WsMessage::Binary(encode_wire_frame(&join_ack).into()))
                .await
                .unwrap();
            let sub_ack = SdkSignalFrame {
                timestamp_ms: Some(now_ms()),
                r#type: Some(sdk_signal_frame::Type::SubscribeAck as i32),
                suback: Some(SdkSubscribeAckFrame {
                    sdp_answer: Some("v=0\r\n".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            ws1.send(WsMessage::Binary(encode_wire_frame(&sub_ack).into()))
                .await
                .unwrap();

            // Connection 2: protocol error before JOIN_ACK.
            let (s2, _) = listener.accept().await.unwrap();
            let mut ws2 = accept_hdr_async(s2, |_req: &Request, mut resp: Response| {
                resp.headers_mut().insert(
                    "Sec-WebSocket-Protocol",
                    HeaderValue::from_static("_aws_wt_session"),
                );
                Ok(resp)
            })
            .await
            .unwrap();
            let err_frame = SdkSignalFrame {
                timestamp_ms: Some(now_ms()),
                error: Some(SdkErrorFrame {
                    status: Some(403),
                    description: Some("forbidden".into()),
                }),
                ..Default::default()
            };
            ws2.send(WsMessage::Binary(encode_wire_frame(&err_frame).into()))
                .await
                .unwrap();

            // Connection 3: immediate close before SUBSCRIBE_ACK.
            let (s3, _) = listener.accept().await.unwrap();
            let mut ws3 = accept_hdr_async(s3, |_req: &Request, mut resp: Response| {
                resp.headers_mut().insert(
                    "Sec-WebSocket-Protocol",
                    HeaderValue::from_static("_aws_wt_session"),
                );
                Ok(resp)
            })
            .await
            .unwrap();
            ws3.send(WsMessage::Close(None)).await.unwrap();
        });

        let creds = content_credentials(sample_creds(format!("ws://{addr}/control/M1")));

        let mut c1 = ChimeSignalClient::connect(&creds).await.unwrap();
        let ja = c1
            .wait_for_join_ack(Duration::from_secs(2), |_| {})
            .await
            .unwrap();
        assert!(ja.turn_credentials.is_some());
        let sa = c1
            .wait_for_subscribe_ack(Duration::from_secs(2), |_| {})
            .await
            .unwrap();
        assert_eq!(sa.sdp_answer.as_deref(), Some("v=0\r\n"));
        c1.close().await;

        let mut c2 = ChimeSignalClient::connect(&creds).await.unwrap();
        let e2 = c2
            .wait_for_join_ack(Duration::from_secs(2), |_| {})
            .await
            .unwrap_err();
        assert!(matches!(e2, HuddleError::Protocol { status: 403, .. }));
        c2.close().await;

        let mut c3 = ChimeSignalClient::connect(&creds).await.unwrap();
        let e3 = c3
            .wait_for_subscribe_ack(Duration::from_secs(2), |_| {})
            .await
            .unwrap_err();
        assert!(matches!(e3, HuddleError::Signaling(ref m) if m.contains("closed")));
        c3.close().await;

        server.await.unwrap();
    }
}
